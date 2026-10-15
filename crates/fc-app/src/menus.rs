//! In-window popup menus: file context menu, drive list, favorites, history, tabs.

use std::path::PathBuf;

use gpui::{
    anchored, deferred, div, prelude::*, px, AnimationExt, AnyElement, Context, MouseButton, Pixels, Point, Window,
};

use fc_core::keymap::Action;
use fc_core::{LockState, SortKey};

use crate::colors::Colors;
use crate::workspace::Workspace;

/// What a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuCmd {
    Run(Action),
    Navigate(usize, PathBuf),
    SelectVolume(usize, PathBuf),
    CloseTab(usize, usize),
    CloseOtherTabs(usize),
    DuplicateTab(usize, usize),
    SetLock(usize, usize, LockState),
    AddFavorite(PathBuf),
    RemoveFavorite(PathBuf),
    EjectVolume(PathBuf),
    ToggleColumn(SortKey),
    SortBy(usize, SortKey),
}

#[derive(Clone, Debug)]
pub enum MenuEntry {
    Item { label: String, shortcut: Option<String>, cmd: MenuCmd, enabled: bool, checked: bool },
    Header(String),
    Separator,
}

impl MenuEntry {
    fn selectable(&self) -> bool {
        matches!(self, MenuEntry::Item { enabled: true, .. })
    }
}

pub struct PopupMenu {
    pub position: Point<Pixels>,
    pub items: Vec<MenuEntry>,
    pub selected: Option<usize>,
    /// Unique per opened menu (keys the open animation).
    pub id: usize,
}

static NEXT_MENU_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);

impl PopupMenu {
    pub fn new(position: Point<Pixels>, items: Vec<MenuEntry>) -> PopupMenu {
        let id = NEXT_MENU_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        PopupMenu { position, items, selected: None, id }
    }

    pub fn step(&mut self, down: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let mut i = self.selected.unwrap_or(if down { n - 1 } else { 0 });
        for _ in 0..n {
            i = if down { (i + 1) % n } else { (i + n - 1) % n };
            if self.items[i].selectable() {
                self.selected = Some(i);
                return;
            }
        }
    }

    pub fn selected_cmd(&self) -> Option<MenuCmd> {
        match self.items.get(self.selected?)? {
            MenuEntry::Item { cmd, enabled: true, .. } => Some(cmd.clone()),
            _ => None,
        }
    }
}

/// Renders the open popup menu, if any.
pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let menu = ws.menu.as_ref()?;
    let c = Colors::get(window, cx);
    let max_height = (f32::from(window.viewport_size().height) - 16.0).max(120.0);
    let mut list = div()
        .id("popup-menu")
        .key_context("Menu")
        .track_focus(&ws.menu_focus)
        .on_action(cx.listener(|this, _: &crate::actions::ui::MenuUp, _w, cx| this.menu_step(false, cx)))
        .on_action(cx.listener(|this, _: &crate::actions::ui::MenuDown, _w, cx| this.menu_step(true, cx)))
        .on_action(cx.listener(|this, _: &crate::actions::ui::Confirm, w, cx| this.menu_confirm(w, cx)))
        .on_action(cx.listener(|this, _: &crate::actions::ui::Cancel, w, cx| this.close_menu(w, cx)))
        .on_mouse_down_out(cx.listener(|this, _, w, cx| this.close_menu(w, cx)))
        .min_w(px(240.))
        .max_w(px(520.))
        .max_h(px(max_height))
        .overflow_y_scroll()
        .p(px(5.))
        .rounded(px(12.))
        .bg(c.surface)
        .border_1()
        .border_color(c.border)
        .shadow(vec![gpui::BoxShadow { color: c.shadow, offset: gpui::point(px(0.), px(14.)), blur_radius: px(36.), spread_radius: px(-8.) }])
        .text_size(px(13.))
        .text_color(c.text);

    for (ix, item) in menu.items.iter().enumerate() {
        list = list.child(match item {
            MenuEntry::Separator => div().my(px(4.)).mx(px(8.)).h(px(1.)).bg(c.border).into_any_element(),
            MenuEntry::Header(text) => div()
                .px(px(10.))
                .pt(px(6.))
                .pb(px(3.))
                .text_size(px(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c.muted)
                .child(text.to_uppercase())
                .into_any_element(),
            MenuEntry::Item { label, shortcut, cmd, enabled, checked } => {
                let glyph = menu_icon(cmd);
                let cmd = cmd.clone();
                let enabled = *enabled;
                let selected = menu.selected == Some(ix);
                div()
                    .id(("menu-item", ix))
                    .group("menu-item")
                    .px(px(8.))
                    .h(px(26.))
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .when(!enabled, |d| d.text_color(c.muted.opacity(0.7)))
                    .when(selected && enabled, |d| d.bg(c.accent).text_color(c.on_accent))
                    .when(enabled && !selected, |d| d.hover(|s| s.bg(c.accent.opacity(0.16))))
                    .child(div().w(px(16.)).flex().justify_center().child(if *checked {
                        crate::look::icon(crate::icons::CHECK, 13., if selected { c.on_accent } else { c.accent }).into_any_element()
                    } else if let Some(g) = glyph {
                        crate::look::icon(g, 14., if selected { c.on_accent } else if enabled { c.muted } else { c.muted.opacity(0.5) }).into_any_element()
                    } else {
                        div().into_any_element()
                    }))
                    .child(div().flex_1().min_w(px(0.)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(label.clone()))
                    .child(div().text_size(px(11.)).font_family("Menlo").opacity(0.6).child(shortcut.clone().unwrap_or_default()))
                    .when(enabled, |d| {
                        d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.run_menu_cmd(cmd.clone(), w, cx);
                        }))
                    })
                    .into_any_element()
            }
        });
    }

    let generation = menu.id;
    Some(
        deferred(
            anchored().position(menu.position).snap_to_window_with_margin(px(8.)).child(list.with_animation(
                ("menu-in", generation),
                gpui::Animation::new(std::time::Duration::from_millis(160)).with_easing(gpui::ease_out_quint()),
                |el, t| el.opacity(t).mt(px((1.0 - t) * -6.0)),
            )),
        )
        .with_priority(2)
        .into_any_element(),
    )
}

/// Leading glyph for a menu command.
fn menu_icon(cmd: &MenuCmd) -> Option<&'static str> {
    use crate::icons;
    Some(match cmd {
        MenuCmd::Run(a) => crate::look::action_icon(*a),
        MenuCmd::Navigate(..) => icons::FOLDER,
        MenuCmd::SelectVolume(..) => icons::DISK,
        MenuCmd::EjectVolume(_) => icons::EJECT,
        MenuCmd::CloseTab(..) | MenuCmd::CloseOtherTabs(_) => icons::CLOSE,
        MenuCmd::DuplicateTab(..) => icons::PLUS,
        MenuCmd::SetLock(..) => icons::LOCK,
        MenuCmd::AddFavorite(_) | MenuCmd::RemoveFavorite(_) => icons::STAR,
        MenuCmd::ToggleColumn(_) | MenuCmd::SortBy(..) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    fn item(enabled: bool) -> MenuEntry {
        MenuEntry::Item { label: "x".into(), shortcut: None, cmd: MenuCmd::Run(Action::View), enabled, checked: false }
    }

    #[test]
    fn keyboard_stepping_skips_disabled_and_separators() {
        let mut m = PopupMenu::new(point(px(0.), px(0.)), vec![MenuEntry::Header("h".into()), item(true), MenuEntry::Separator, item(false), item(true)]);
        m.step(true);
        assert_eq!(m.selected, Some(1));
        m.step(true);
        assert_eq!(m.selected, Some(4));
        m.step(true);
        assert_eq!(m.selected, Some(1), "wraps");
        m.step(false);
        assert_eq!(m.selected, Some(4));
        assert_eq!(m.selected_cmd(), Some(MenuCmd::Run(Action::View)));
    }
}
