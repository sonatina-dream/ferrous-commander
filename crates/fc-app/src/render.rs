//! Drawing the workspace window — the "Aurora" design: a frosted window with a custom
//! title bar and omnibox, a places sidebar, floating panel cards with a gliding
//! cursor, colour-coded file tiles, a keycap dock and floating job cards.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    canvas, div, ease_out_quint, img, linear_color_stop, linear_gradient, point, prelude::*, px, relative,
    Animation, AnimationExt, AnyElement, BoxShadow, Context, ExternalPaths, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, ObjectFit, ScrollWheelEvent, SharedString, Window,
};

use fc_core::format::{self, SizeMode};
use fc_core::keymap::Action;
use fc_core::settings::Settings;
use fc_core::{Entry, Location, SortKey, ViewMode};

use crate::actions::{self, ui};
use crate::colors::Colors;
use crate::look::{self, chip, file_tile, ghost_button, icon, keycap, section_label};
use crate::state::{AppState, PANEL_CONTEXT};
use crate::text_input::{self, InputStyle};
use crate::workspace::{places, DragPreview, DraggedFiles, UiNav, Workspace};
use crate::{dialogs, icons, menus, palette};

/// Width of a cell in thumbs mode.
pub const THUMB_CELL: f32 = 128.0;
pub const THUMB_HEIGHT: f32 = 126.0;
pub const SIDEBAR_WIDTH: f32 = 214.0;
const TITLEBAR_HEIGHT: f32 = 50.0;
/// Rows animate in for this long after a folder change.
const FRESH: Duration = Duration::from_millis(420);

fn col_width(key: SortKey) -> f32 {
    match key {
        SortKey::Name => 0.0,
        SortKey::Extension => 56.0,
        SortKey::Size => 108.0,
        SortKey::Date | SortKey::Created | SortKey::Added | SortKey::Accessed => 128.0,
        SortKey::Permissions => 96.0,
        SortKey::Kind => 140.0,
    }
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
    )
}

fn cell_text(tab: &fc_core::Tab, e: &Entry, key: SortKey, mode: SizeMode, now: SystemTime) -> String {
    if e.is_parent() && key != SortKey::Name {
        return String::new();
    }
    match key {
        SortKey::Name => e.name.clone(),
        SortKey::Extension => e.extension().to_string(),
        SortKey::Size => match tab.entry_size(e) {
            Some(b) => format::format_size(b, mode),
            None => "—".into(),
        },
        SortKey::Date => format::format_time_friendly(e.modified, now),
        SortKey::Created => format::format_time_friendly(e.created, now),
        SortKey::Added => format::format_time_friendly(e.added, now),
        SortKey::Accessed => format::format_time_friendly(e.accessed, now),
        SortKey::Kind => e.kind_label(),
        SortKey::Permissions => format::permissions_string(e.mode, e.kind),
    }
}

/// "1 file", "3 files".
fn plural(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// Paths currently cut with ⌘X (drawn dimmed until pasted).
fn cut_set(cx: &gpui::App) -> std::collections::HashSet<PathBuf> {
    cx.global::<AppState>().cut.as_ref().map(|(p, _)| p.iter().cloned().collect()).unwrap_or_default()
}

fn columns(s: &Settings) -> Vec<SortKey> {
    std::iter::once(SortKey::Name).chain(s.view_columns.iter().copied().filter(|k| *k != SortKey::Name)).collect()
}

/// Keeps the columns that fit `list_width`, in order, leaving the name at least 150 px.
/// Narrow panels drop trailing columns instead of letting them run off the edge.
fn fit_columns(cols: Vec<SortKey>, list_width: f32) -> Vec<SortKey> {
    if list_width <= 1.0 {
        return cols;
    }
    // Row chrome: margins (2×6), padding (2×8), file tile (22) and its gap (10).
    let mut used = 12.0 + 16.0 + 22.0 + 10.0 + 150.0;
    let mut kept = vec![SortKey::Name];
    for key in cols.into_iter().filter(|k| *k != SortKey::Name) {
        let w = col_width(key) + 10.0;
        if used + w <= list_width {
            kept.push(key);
            used += w;
        }
    }
    kept
}

/// Small accent check shown on marked items.
fn check_badge(c: &Colors) -> gpui::Div {
    div()
        .absolute()
        .top(px(-4.))
        .right(px(-4.))
        .size(px(13.))
        .rounded_full()
        .bg(c.accent)
        .border_1()
        .border_color(c.bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(icons::CHECK, 9., c.on_accent))
}

impl Workspace {
    fn on_nav(&mut self, nav: UiNav, window: &mut Window, cx: &mut Context<Self>) {
        self.on_ui_action(nav, window, cx);
    }

    /// Advances the cursor springs; returns true while anything is still moving.
    fn animate(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        let mut moving = false;
        for side in 0..2 {
            let tab = self.panels[side].tab();
            let target = tab.cursor as f32;
            // A cursor that just appeared starts where it is instead of gliding in.
            if tab.cursor_visible != self.cursor_shown[side] {
                self.cursor_shown[side] = tab.cursor_visible;
                self.snap_cursor[side] = true;
            }
            let spring = &mut self.cursor_spring[side];
            if self.snap_cursor[side] || (target - spring.value).abs() > 40.0 {
                spring.snap(target);
                self.snap_cursor[side] = false;
            } else if spring.step(target, dt) {
                moving = true;
            }
            let offset: f32 = self.scroll[side].0.borrow().base_handle.offset().y.into();
            if (offset - self.last_offsets[side]).abs() > 0.5 {
                self.last_offsets[side] = offset;
                moving = true;
            }
        }
        moving || self.nav_at.iter().any(|t| t.elapsed() < FRESH)
    }

    // ----- title bar -------------------------------------------------------------------

    fn render_titlebar(&mut self, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab();
        let mode = tab.view_mode;
        let (can_back, can_fwd) = (tab.history.can_go_back(), tab.history.can_go_forward());
        let title = tab.title();
        let path = tab.working_dir().display().to_string();
        let s = &cx.global::<AppState>().settings;
        let hidden = s.view.show_hidden;
        let sidebar = s.view.show_sidebar;
        let active_jobs = self.jobs.active().count();

        let nav = |id: &'static str, glyph: &'static str, action: Action, enabled: bool, on: bool, c: &Colors, cx: &mut Context<Self>| {
            ghost_button(id, glyph, c, on, enabled).when(enabled, |d| {
                d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    w.focus(&this.focus);
                    this.run(action, w, cx);
                }))
            })
        };

        let mut segmented = div().flex().items_center().p(px(3.)).gap(px(2.)).rounded(px(10.)).bg(c.sunken).border_1().border_color(c.border.opacity(0.6));
        for (id, glyph, action, m) in [
            ("seg-full", icons::LIST, Action::ViewModeFull, ViewMode::Full),
            ("seg-brief", icons::COLUMNS, Action::ViewModeBrief, ViewMode::Brief),
            ("seg-thumbs", icons::GRID, Action::ViewModeThumbs, ViewMode::Thumbs),
        ] {
            let on = mode == m;
            segmented = segmented.child(
                div()
                    .id(id)
                    .w(px(30.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(c.bg).shadow(look::soft_shadow(c)))
                    .when(!on, |d| d.hover(|s| s.bg(c.hover)))
                    .child(icon(glyph, 14., if on { c.accent } else { c.muted }))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.run(action, w, cx);
                    })),
            );
        }

        let omnibox = div()
            .id("omnibox")
            .flex()
            .items_center()
            .gap(px(9.))
            .w(px(520.))
            .max_w(relative(1.0))
            .h(px(32.))
            .px(px(12.))
            .rounded(px(10.))
            .bg(c.sunken)
            .border_1()
            .border_color(c.border.opacity(0.7))
            .cursor_pointer()
            .hover(|s| s.border_color(c.accent.opacity(0.6)))
            .child(icon(icons::SEARCH, 14., c.muted))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c.chrome_text)
                    .flex_none()
                    .max_w(px(200.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(title),
            )
            .child(div().flex_1().min_w(px(0.)).text_size(px(12.)).text_color(c.muted).overflow_hidden().text_ellipsis().whitespace_nowrap().child(path))
            .child(keycap("⌘P", c))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.open_palette(w, cx);
            }));

        div()
            .id("titlebar")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(TITLEBAR_HEIGHT))
            .pl(px(88.))
            .pr(px(12.))
            .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
                if ev.click_count == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(nav("tb-sidebar", icons::SIDEBAR, Action::ToggleDiskList, true, sidebar, c, cx))
            .child(div().w(px(6.)))
            .child(nav("tb-back", icons::BACK, Action::GoBack, can_back, false, c, cx))
            .child(nav("tb-fwd", icons::FORWARD, Action::GoForward, can_fwd, false, c, cx))
            .child(nav("tb-up", icons::UP, Action::GoTop, true, false, c, cx))
            .child(div().flex_1().min_w(px(20.)).flex().justify_center().px(px(12.)).child(omnibox))
            .child(segmented)
            .child(div().w(px(6.)))
            .child(nav("tb-hidden", if hidden { icons::EYE } else { icons::HIDDEN }, Action::ToggleHidden, true, hidden, c, cx))
            .child(nav("tb-term", icons::TERMINAL, Action::ShowTerminal, true, self.terminal_open, c, cx))
            .child(
                div()
                    .relative()
                    .child(nav("tb-queue", icons::QUEUE, Action::ShowQueue, true, active_jobs > 0, c, cx))
                    .when(active_jobs > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(1.))
                                .right(px(1.))
                                .min_w(px(15.))
                                .h(px(15.))
                                .px(px(4.))
                                .rounded_full()
                                .bg(c.accent)
                                .text_color(c.on_accent)
                                .text_size(px(9.5))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(active_jobs.to_string()),
                        )
                    }),
            )
            .child(div().relative().child(crate::probe::probe("main.gear")).child(nav("tb-gear", icons::GEAR, Action::Preferences, true, false, c, cx)))
    }

    // ----- sidebar ---------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn sidebar_item(
        &self,
        id: SharedString,
        glyph: &'static str,
        tint: gpui::Hsla,
        label: String,
        path: PathBuf,
        c: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let current = self.tab().working_dir() == path;
        let side = self.active;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(9.))
            .h(px(30.))
            .px(px(10.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(current, |d| d.bg(c.accent.opacity(0.16)).text_color(c.accent))
            .when(!current, |d| d.text_color(c.chrome_text).hover(|s| s.bg(c.hover)))
            .child(icon(glyph, 15., if current { c.accent } else { tint }))
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                this.navigate(side, path.clone(), None, w, cx);
            }))
    }

    fn render_sidebar(&mut self, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div()
            .id("sidebar")
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .px(px(10.))
            .pb(px(12.))
            .overflow_y_scroll()
            .text_size(px(13.));

        bar = bar.child(section_label("Places", c));
        for (label, path) in places() {
            let glyph = match label {
                "Home" => icons::HOME,
                "Desktop" => icons::DESKTOP,
                "Documents" => icons::DOCS,
                "Downloads" => icons::DOWNLOAD,
                "Applications" => icons::APPS,
                _ => icons::DISK,
            };
            let label = if label.starts_with("Computer") { "Computer".to_string() } else { label.to_string() };
            bar = bar.child(self.sidebar_item(SharedString::from(format!("place-{label}")), glyph, c.accent2, label, path, c, cx));
        }

        bar = bar.child(section_label("Volumes", c));
        let volumes = self.volumes.clone();
        for v in volumes {
            let used = if v.total > 0 { 1.0 - v.free as f32 / v.total as f32 } else { 0.0 };
            let (from, to) = if used > 0.9 { (c.danger, c.danger) } else { (c.accent, c.accent2) };
            let ejectable = v.ejectable;
            let eject_path = v.path.clone();
            let free = format::format_size(v.free, SizeMode::Dynamic);
            let item = self
                .sidebar_item(SharedString::from(format!("vol-{}", v.name)), icons::DISK, c.folder, v.name.clone(), v.path.clone(), c, cx)
                .group("vol");
            bar = bar.child(
                div()
                    .flex()
                    .flex_col()
                    .child(item.when(ejectable, |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("eject-{}", v.name)))
                                .invisible()
                                .group_hover("vol", |s| s.visible())
                                .size(px(20.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .hover(|s| s.bg(c.hover))
                                .child(icon(icons::EJECT, 11., c.muted))
                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.run_menu_cmd(crate::menus::MenuCmd::EjectVolume(eject_path.clone()), w, cx);
                                })),
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .pl(px(34.))
                            .pr(px(10.))
                            .pb(px(6.))
                            .child(
                                div().flex_1().h(px(4.)).rounded_full().bg(c.border).child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .w(relative(used.clamp(0.02, 1.0)))
                                        .bg(linear_gradient(90., linear_color_stop(from, 0.), linear_color_stop(to, 1.))),
                                ),
                            )
                            .child(div().text_size(px(10.5)).text_color(c.muted).whitespace_nowrap().child(format!("{free} free"))),
                    ),
            );
        }

        let favorites = cx.global::<AppState>().settings.favorites.clone();
        let current = self.tab().working_dir().to_path_buf();
        bar = bar.child(section_label("Favorites", c));
        for f in favorites.iter() {
            let label = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| f.display().to_string());
            bar = bar.child(self.sidebar_item(SharedString::from(format!("fav-{}", f.display())), icons::STAR_FILL, c.accent2, label, f.clone(), c, cx));
        }
        if !favorites.contains(&current) {
            bar = bar.child(
                div()
                    .id("fav-add")
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_color(c.muted)
                    .text_size(px(12.))
                    .hover(|s| s.bg(c.hover).text_color(c.chrome_text))
                    .child(icon(icons::PLUS, 13., c.muted))
                    .child("Add current folder")
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.run_menu_cmd(crate::menus::MenuCmd::AddFavorite(current.clone()), w, cx);
                    })),
            );
        }

        let recent: Vec<PathBuf> = self.panels[self.active]
            .folder_history
            .iter()
            .filter(|p| p.as_path() != self.tab().working_dir())
            .take(5)
            .cloned()
            .collect();
        if !recent.is_empty() {
            bar = bar.child(section_label("Recent", c));
            for p in recent {
                let label = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
                bar = bar.child(self.sidebar_item(SharedString::from(format!("recent-{}", p.display())), icons::CLOCK, c.muted, label, p, c, cx));
            }
        }
        bar
    }

    // ----- panel card ------------------------------------------------------------------

    /// Button that hides the right panel (it can be shown again from Settings ▸ View).
    fn close_panel_button(&self, side: usize, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        ghost_button(SharedString::from(format!("close-panel-{side}")), icons::CLOSE, c, false, true).size(px(26.)).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.run(Action::ToggleRightPanel, w, cx);
            }),
        )
    }

    fn render_tabs(&mut self, side: usize, closable: bool, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = &self.panels[side];
        let active_panel = side == self.active;
        let mut strip = div()
            .id(("tabs", side))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .h(px(38.))
            .px(px(8.))
            .overflow_x_scroll()
            .text_size(px(12.));
        for (ix, tab) in panel.tabs.iter().enumerate() {
            let selected = ix == panel.active;
            let locked = tab.lock.is_locked();
            strip = strip.child(
                div()
                    .id(SharedString::from(format!("tab-{side}-{ix}")))
                    .group("tab")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(26.))
                    .pl(px(10.))
                    .pr(px(5.))
                    .max_w(px(190.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(if active_panel { c.accent.opacity(0.16) } else { c.sunken }).text_color(if active_panel { c.accent } else { c.text }))
                    .when(!selected, |d| d.text_color(c.muted).hover(|s| s.bg(c.hover)))
                    .child(icon(if locked { icons::LOCK } else { icons::FOLDER }, 11., if selected && active_panel { c.accent } else { c.folder.opacity(0.8) }))
                    .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(tab.title()))
                    .child(
                        div()
                            .id(SharedString::from(format!("tab-close-{side}-{ix}")))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .invisible()
                            .group_hover("tab", |s| s.visible())
                            .hover(|s| s.bg(c.hover))
                            .child(icon(icons::CLOSE, 9., c.muted))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.close_tab(side, ix, w, cx);
                            })),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| this.select_tab(side, ix, w, cx)))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, w, cx| this.close_tab(side, ix, w, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                        let menu = this.tab_menu(side, ix, ev.position);
                        this.open_menu(menu, w, cx);
                    })),
            );
        }
        strip
            .child(ghost_button(SharedString::from(format!("tab-new-{side}")), icons::PLUS, c, false, true).size(px(24.)).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.active = side;
                    this.run(Action::NewTab, w, cx);
                }),
            ))
            .when(closable, |d| d.child(div().flex_1()).child(self.close_panel_button(side, c, cx)))
    }

    fn render_card_header(&mut self, side: usize, show_new_tab: bool, closable: bool, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.panels[side].tab();
        let is_favorite = cx.global::<AppState>().settings.favorites.iter().any(|f| f == tab.working_dir());
        let mut crumbs = div().flex().items_center().flex_1().min_w(px(0.)).overflow_hidden().gap(px(2.));
        match &tab.location {
            Location::Dir(dir) => {
                let root_name = self.volumes.iter().find(|v| v.is_root).map(|v| v.name.clone()).unwrap_or_else(|| "/".into());
                let mut acc = PathBuf::from("/");
                let mut parts: Vec<(String, PathBuf)> = vec![(root_name, acc.clone())];
                for comp in dir.components().skip(1) {
                    acc.push(comp.as_os_str());
                    parts.push((comp.as_os_str().to_string_lossy().into_owned(), acc.clone()));
                }
                // Collapse long paths: root › … › last three.
                let n = parts.len();
                let shown: Vec<(usize, Option<(String, PathBuf)>)> = if n > 4 {
                    let mut v = vec![(0, Some(parts[0].clone())), (1, None)];
                    v.extend((n - 2..n).map(|i| (i, Some(parts[i].clone()))));
                    v
                } else {
                    parts.iter().cloned().enumerate().map(|(i, p)| (i, Some(p))).collect()
                };
                for (k, (i, part)) in shown.into_iter().enumerate() {
                    if k > 0 {
                        crumbs = crumbs.child(icon(icons::CHEVRON_RIGHT, 10., c.muted.opacity(0.7)));
                    }
                    let Some((label, path)) = part else {
                        crumbs = crumbs.child(div().px(px(4.)).text_color(c.muted).child("…"));
                        continue;
                    };
                    let last = i + 1 == n;
                    crumbs = crumbs.child(
                        div()
                            .id(SharedString::from(format!("crumb-{side}-{i}")))
                            .flex()
                            .when(last, |d| d.flex_none())
                            .when(!last, |d| d.flex_shrink().min_w(px(28.)))
                            .items_center()
                            .gap(px(5.))
                            .h(px(24.))
                            .px(px(7.))
                            .rounded(px(7.))
                            .max_w(px(220.))
                            .cursor_pointer()
                            .when(i == 0, |d| d.child(icon(icons::DISK, 12., c.muted)))
                            .when(last, |d| d.font_weight(FontWeight::SEMIBOLD).text_color(c.text))
                            .when(!last, |d| d.text_color(c.muted))
                            .hover(|s| s.bg(c.hover).text_color(c.text))
                            .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                                cx.stop_propagation();
                                this.active = side;
                                this.navigate(side, path.clone(), None, w, cx);
                            })),
                    );
                }
            }
            Location::Results { title, root, items } => {
                crumbs = crumbs
                    .child(icon(icons::SEARCH, 13., c.accent))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c.text).px(px(4.)).child(title.clone()))
                    .child(div().text_color(c.muted).overflow_hidden().text_ellipsis().whitespace_nowrap().child(format!("{} item(s) in {}", items.len(), root.display())));
            }
        }
        let current = tab.working_dir().to_path_buf();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(42.))
            .pl(px(8.))
            .pr(px(6.))
            .text_size(px(12.5))
            .child(crumbs)
            .child(
                ghost_button(SharedString::from(format!("fav-{side}")), if is_favorite { icons::STAR_FILL } else { icons::STAR }, c, false, true)
                    .size(px(26.))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        let cmd = if is_favorite { crate::menus::MenuCmd::RemoveFavorite(current.clone()) } else { crate::menus::MenuCmd::AddFavorite(current.clone()) };
                        this.run_menu_cmd(cmd, w, cx);
                    })),
            )
            .child(ghost_button(SharedString::from(format!("hist-{side}")), icons::CLOCK, c, false, true).size(px(26.)).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    this.active = side;
                    let menu = this.history_menu(side, ev.position);
                    this.open_menu(menu, w, cx);
                }),
            ))
            .child(ghost_button(SharedString::from(format!("drive-{side}")), icons::DISK, c, false, true).size(px(26.)).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                    cx.stop_propagation();
                    this.active = side;
                    let menu = this.drive_menu(side, ev.position);
                    this.open_menu(menu, w, cx);
                }),
            ))
            .when(show_new_tab, |d| {
                d.child(ghost_button(SharedString::from(format!("newtab-{side}")), icons::PLUS, c, false, true).size(px(26.)).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.active = side;
                        this.run(Action::NewTab, w, cx);
                    }),
                ))
            })
            .when(closable, |d| d.child(self.close_panel_button(side, c, cx)))
    }

    fn render_column_header(&mut self, side: usize, cols: &[SortKey], c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let sort = self.panels[side].tab().sort;
        let mut row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(26.))
            .mx(px(6.))
            .px(px(8.))
            .text_size(px(10.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c.muted.opacity(0.9))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, w, cx| {
                let menu = this.header_menu(ev.position, cx);
                this.open_menu(menu, w, cx);
            }))
            .child(div().w(px(22.)).flex_none());
        for &key in cols {
            let on = sort.key == key;
            let cell = div()
                .id(SharedString::from(format!("hdr-{side}-{}", key.id())))
                .h_full()
                .flex()
                .items_center()
                .gap(px(4.))
                .cursor_pointer()
                .when(on, |d| d.text_color(c.accent))
                .hover(|s| s.text_color(c.text))
                .child(key.header().to_uppercase())
                .when(on, |d| d.child(if sort.ascending { "↑" } else { "↓" }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.run_menu_cmd(crate::menus::MenuCmd::SortBy(side, key), w, cx);
                }));
            row = row.child(if key == SortKey::Name {
                cell.flex_1()
            } else {
                cell.w(px(col_width(key))).flex_none().when(key == SortKey::Size, |d| d.justify_end())
            });
        }
        row
    }

    fn full_rows(&mut self, side: usize, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = Colors::get(window, cx);
        let list_width: f32 = self.list_bounds[side].get().size.width.into();
        let (cols, size_mode, font) = {
            let s = &cx.global::<AppState>().settings;
            (fit_columns(columns(s), list_width), s.view.size_in_columns, s.appearance.font_size)
        };
        let show_ext_col = cols.contains(&SortKey::Extension);
        let row_h = Workspace::row_height(cx);
        let active_panel = side == self.active;
        let fresh = self.nav_at[side].elapsed() < FRESH;
        let generation = self.nav_gen[side] as usize;
        let now = SystemTime::now();
        let tab = self.panels[side].tab();
        let max_size = tab.entries.iter().filter(|e| !e.is_parent()).map(|e| tab.entry_size(e).unwrap_or(0)).max().unwrap_or(0).max(1);
        let selected_paths: Vec<PathBuf> = tab.targets().into_iter().map(|e| e.path).collect();
        let cut = cut_set(cx);
        let mut counts = self.rendered_rows.get();
        counts[side] = range.len();
        self.rendered_rows.set(counts);

        let mut out = Vec::with_capacity(range.len());
        for ix in range {
            let tab = self.panels[side].tab();
            let Some(e) = tab.entries.get(ix).cloned() else { continue };
            let marked = tab.is_selected(&e);
            let on_pill = tab.cursor_visible && ix == tab.cursor && active_panel;
            let kind = look::kind_of(&e);
            let fg = if on_pill { c.cursor_text } else if marked { c.mark_text } else { c.text };
            let meta = if on_pill { c.cursor_text.opacity(0.78) } else { c.muted };
            let name = if show_ext_col && !e.extension().is_empty() { e.stem().to_string() } else { e.name.clone() };
            let base_opacity = if e.hidden { 0.55 } else { 1.0 } * if cut.contains(&e.path) { 0.45 } else { 1.0 };

            let editor = self.rename_field(side, &e.path, &c);
            let renaming = editor.is_some();
            let tile = file_tile(kind, 22., &c, on_pill).relative().when(marked, |d| d.child(check_badge(&c)));
            let label: AnyElement = match editor {
                Some(field) => field.into_any_element(),
                None => div()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .when(marked || e.groups_as_folder(), |d| d.font_weight(FontWeight::MEDIUM))
                    .child(name)
                    .into_any_element(),
            };
            // Icon and name: dragging here moves the items; the rest of the row draws a band.
            let handle = div()
                .id(("name", ix))
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w(px(0.))
                .overflow_hidden()
                .when(renaming, |d| d.flex_1())
                .child(tile)
                .child(label);
            let mut row = div()
                .id(("row", ix))
                .w_full()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(row_h))
                .px(px(8.))
                .rounded(px(8.))
                .text_color(fg)
                .text_size(px(font))
                .opacity(base_opacity)
                .when(marked && !on_pill, |d| d.bg(c.mark))
                .when(!on_pill && !marked, |d| d.hover(|s| s.bg(c.hover)))
                .child(self.wire_handle(handle, side, ix, &e, &selected_paths, cx))
                .when(!renaming, |d| d.child(div().flex_1().min_w(px(0.))));
            for &key in cols.iter().skip(1) {
                let text = cell_text(tab, &e, key, size_mode, now);
                if key == SortKey::Size {
                    let frac = if e.is_parent() { 0.0 } else { tab.entry_size(&e).map(|b| (b as f64 / max_size as f64).sqrt() as f32).unwrap_or(0.0) };
                    row = row.child(
                        div()
                            .w(px(col_width(key)))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(8.))
                            .when(frac > 0.0, |d| {
                                d.child(
                                    div().w(px(32.)).h(px(4.)).rounded_full().bg(meta.opacity(0.18)).child(
                                        div().h_full().rounded_full().w(relative(frac.clamp(0.06, 1.0))).bg(if on_pill { c.cursor_text.opacity(0.9) } else { c.accent.opacity(0.8) }),
                                    ),
                                )
                            })
                            .child(div().text_size(px(font - 1.)).text_color(meta).whitespace_nowrap().child(text)),
                    );
                } else {
                    row = row.child(
                        div()
                            .w(px(col_width(key)))
                            .flex_none()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(font - 1.))
                            .text_color(meta)
                            .child(text),
                    );
                }
            }
            let row = self.wire_row(row, side, ix, &e, &c, cx);
            out.push(if fresh && ix < 80 {
                row.with_animation(
                    ("row-in", generation * 100_000 + ix),
                    Animation::new(Duration::from_millis(150 + ix.min(14) as u64 * 18)).with_easing(ease_out_quint()),
                    move |el, t| el.opacity(t * base_opacity).ml(px((1. - t) * 14.)),
                )
                .into_any_element()
            } else {
                row.into_any_element()
            });
        }
        out
    }

    /// The inline name editor, when `path` in `side` is being renamed.
    fn rename_field(&self, side: usize, path: &Path, c: &Colors) -> Option<gpui::Div> {
        let r = self.rename.as_ref().filter(|r| r.side == side && r.path == path)?;
        Some(
            div()
                .flex_1()
                .min_w(px(60.))
                .px(px(4.))
                .rounded(px(5.))
                .bg(c.bg)
                .border_1()
                .border_color(c.accent)
                .text_color(c.text)
                .font_weight(FontWeight::NORMAL)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(r.input.clone()),
        )
    }

    /// Clicks, the context menu and folder drop targets, shared by all list modes. A press
    /// on the row outside its name handle starts a selection band.
    fn wire_row(&self, row: gpui::Stateful<gpui::Div>, side: usize, ix: usize, e: &Entry, c: &Colors, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let row = row
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                cx.stop_propagation();
                this.row_mouse_down(side, ix, false, ev, w, cx)
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                cx.stop_propagation();
                this.row_right_click(side, ix, ev.position, w, cx)
            }));
        if !e.is_navigable() {
            return row;
        }
        // Folders (and `..`) take dropped items.
        let folder = e.path.clone();
        let (f1, f2, f3) = (folder.clone(), folder.clone(), folder);
        let tint = c.accent.opacity(0.22);
        row.drag_over::<DraggedFiles>(move |style, files, _, _| {
            if fc_core::ops::drop_action(&files.paths, &f1, false, false).is_some() { style.bg(tint) } else { style }
        })
        .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(tint))
        .on_drop(cx.listener(move |this, files: &DraggedFiles, w, cx| this.drop_into_folder(side, f2.clone(), files, w, cx)))
        .on_drop(cx.listener(move |this, paths: &ExternalPaths, _w, cx| this.drop_external_into(f3.clone(), paths.paths(), cx)))
    }

    /// The icon and name of a row: pressing selects it, dragging moves the items.
    fn wire_handle(&self, handle: gpui::Stateful<gpui::Div>, side: usize, ix: usize, e: &Entry, selected: &[PathBuf], cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let handle = handle.on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
            cx.stop_propagation();
            this.row_mouse_down(side, ix, true, ev, w, cx)
        }));
        let renaming = self.rename.as_ref().is_some_and(|r| r.side == side && r.path == e.path);
        if e.is_parent() || renaming {
            return handle;
        }
        let paths = if self.panels[side].tab().is_selected(e) { selected.to_vec() } else { vec![e.path.clone()] };
        let label = if paths.len() == 1 { e.name.clone() } else { format!("{} items", paths.len()) };
        handle.on_drag(DraggedFiles { side, paths }, move |_, _offset, _w, cx| cx.new(|_| DragPreview { label: label.clone() }))
    }

    fn render_brief(&mut self, side: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = Colors::get(window, cx);
        let (col_w, font) = {
            let s = &cx.global::<AppState>().settings;
            (s.view.brief_column_width.max(110.0), s.appearance.font_size)
        };
        let row_h = Workspace::row_height(cx);
        let bounds = self.list_bounds[side].get();
        let h: f32 = bounds.size.height.into();
        let w: f32 = bounds.size.width.into();
        let rows = ((h / row_h).floor() as usize).max(1);
        let visible = ((w / col_w).floor() as usize).max(1);
        let tab = self.panels[side].tab();
        let n = tab.entries.len();
        let cursor_col = tab.cursor / rows;
        let first = &mut self.brief_first_col[side];
        if cursor_col < *first {
            *first = cursor_col;
        } else if cursor_col >= *first + visible {
            *first = cursor_col + 1 - visible;
        }
        let first = *first;
        let total_cols = n.div_ceil(rows).max(1);
        let active_panel = side == self.active;
        let selected_paths: Vec<PathBuf> = tab.targets().into_iter().map(|e| e.path).collect();
        let cut = cut_set(cx);

        let mut grid = div().flex().flex_row().size_full().px(px(4.)).overflow_hidden();
        for col in first..(first + visible + 1).min(total_cols) {
            let mut column = div().flex().flex_col().w(px(col_w)).flex_none().h_full();
            for ix in (col * rows)..((col + 1) * rows).min(n) {
                let tab = self.panels[side].tab();
                let e = tab.entries[ix].clone();
                let marked = tab.is_selected(&e);
                let is_cursor = tab.cursor_visible && ix == tab.cursor;
                let on_pill = is_cursor && active_panel;
                let fg = if on_pill { c.cursor_text } else if marked { c.mark_text } else { c.text };
                let editor = self.rename_field(side, &e.path, &c);
                let renaming = editor.is_some();
                let label: AnyElement = match editor {
                    Some(field) => field.into_any_element(),
                    None => div().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(e.name.clone()).into_any_element(),
                };
                let handle = div()
                    .id(("brief-name", ix))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w(px(0.))
                    .overflow_hidden()
                    .when(renaming, |d| d.flex_1())
                    .child(file_tile(look::kind_of(&e), 18., &c, on_pill).relative().when(marked, |d| d.child(check_badge(&c))))
                    .child(label);
                let row = div()
                    .id(("brief", ix))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(row_h))
                    .mx(px(2.))
                    .px(px(6.))
                    .rounded(px(7.))
                    .text_color(fg)
                    .text_size(px(font))
                    .when(on_pill, |d| d.bg(c.cursor))
                    .when(is_cursor && !active_panel, |d| d.border_1().border_color(c.cursor.opacity(0.5)))
                    .when(marked && !on_pill, |d| d.bg(c.mark))
                    .when(!on_pill && !marked, |d| d.hover(|s| s.bg(c.hover)))
                    .opacity(if e.hidden { 0.55 } else { 1.0 } * if cut.contains(&e.path) { 0.45 } else { 1.0 })
                    .child(self.wire_handle(handle, side, ix, &e, &selected_paths, cx))
                    .when(!renaming, |d| d.child(div().flex_1().min_w(px(0.))));
                column = column.child(self.wire_row(row, side, ix, &e, &c, cx));
            }
            grid = grid.child(column);
        }
        grid.on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, w, cx| {
            let d = ev.delta.pixel_delta(px(20.));
            let dy: f32 = if d.y.abs() > d.x.abs() { d.y.into() } else { d.x.into() };
            if dy.abs() > 2.0 {
                let rows = rows as isize;
                this.active = side;
                w.focus(&this.focus);
                this.move_cursor(if dy < 0.0 { rows } else { -rows }, false, cx);
            }
        }))
        .into_any_element()
    }

    fn thumb_rows(&mut self, side: usize, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = Colors::get(window, cx);
        let cols = self.thumb_columns(side);
        let active_panel = side == self.active;
        let tab = self.panels[side].tab();
        let selected_paths: Vec<PathBuf> = tab.targets().into_iter().map(|e| e.path).collect();
        let cut = cut_set(cx);
        let n = tab.entries.len();
        let size_mode = cx.global::<AppState>().settings.view.size_in_columns;
        let mut out = Vec::new();
        for row in range {
            let mut line = div().flex().flex_row().h(px(THUMB_HEIGHT)).px(px(4.));
            for ix in (row * cols)..((row + 1) * cols).min(n) {
                let tab = self.panels[side].tab();
                let e = tab.entries[ix].clone();
                let marked = tab.is_selected(&e);
                let is_cursor = tab.cursor_visible && ix == tab.cursor;
                let on_pill = is_cursor && active_panel;
                let kind = look::kind_of(&e);
                let picture: AnyElement = if is_image(&e.path) {
                    div()
                        .size(px(68.))
                        .rounded(px(10.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(c.border)
                        .shadow(look::soft_shadow(&c))
                        .child(img(e.path.clone()).size_full().object_fit(ObjectFit::Cover))
                        .into_any_element()
                } else {
                    file_tile(kind, 60., &c, on_pill).into_any_element()
                };
                let meta = if e.is_parent() {
                    String::new()
                } else if e.is_dir_like() {
                    e.kind_label()
                } else {
                    format::format_size(e.size, size_mode)
                };
                let name_el: AnyElement = match self.rename_field(side, &e.path, &c) {
                    Some(field) => div().w_full().px(px(4.)).flex().child(field).into_any_element(),
                    None => div()
                        .w_full()
                        .px(px(8.))
                        .text_size(px(11.5))
                        .text_color(if on_pill { c.cursor_text } else if marked { c.mark_text } else { c.text })
                        .text_center()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(e.name.clone())
                        .into_any_element(),
                };
                let content = div()
                    .id(("thumb-body", ix))
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(5.))
                    .child(div().h(px(70.)).flex().items_center().justify_center().relative().child(picture).when(marked, |d| d.child(check_badge(&c))))
                    .child(name_el)
                    .child(div().text_size(px(10.)).text_color(if on_pill { c.cursor_text.opacity(0.75) } else { c.muted }).child(meta));
                let cell = div()
                    .id(("thumb", ix))
                    .w(px(THUMB_CELL - 6.))
                    .h(px(THUMB_HEIGHT - 8.))
                    .m(px(3.))
                    .rounded(px(12.))
                    .when(on_pill, |d| {
                        d.bg(c.cursor).shadow(vec![BoxShadow { color: c.cursor.opacity(0.45), offset: point(px(0.), px(6.)), blur_radius: px(16.), spread_radius: px(-4.) }])
                    })
                    .when(is_cursor && !active_panel, |d| d.border_1().border_color(c.cursor.opacity(0.5)))
                    .when(marked && !on_pill, |d| d.bg(c.mark).border_1().border_color(c.accent.opacity(0.6)))
                    .when(!on_pill && !marked, |d| d.hover(|s| s.bg(c.hover)))
                    .opacity(if e.hidden { 0.55 } else { 1.0 } * if cut.contains(&e.path) { 0.45 } else { 1.0 })
                    .child(self.wire_handle(content, side, ix, &e, &selected_paths, cx));
                line = line.child(self.wire_row(cell, side, ix, &e, &c, cx));
            }
            out.push(line.into_any_element());
        }
        out
    }

    fn render_list(&mut self, side: usize, s: &Settings, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_cell = self.list_bounds[side].clone();
        let tab = self.panels[side].tab();
        let mode = tab.view_mode;
        let n = tab.entries.len();
        let only_parent = n <= 1 && tab.entries.first().is_none_or(|e| e.is_parent());
        let cols = fit_columns(columns(s), self.list_bounds[side].get().size.width.into());
        let active = side == self.active;
        let row_h = Workspace::row_height(cx);
        let drop_tint = c.accent.opacity(0.07);

        let body: AnyElement = match mode {
            ViewMode::Full => gpui::uniform_list(("full", side), n, cx.processor(move |this, range: Range<usize>, window, cx| this.full_rows(side, range, window, cx)))
                .track_scroll(self.scroll[side].clone())
                .size_full()
                .px(px(6.))
                .into_any_element(),
            ViewMode::Brief => self.render_brief(side, window, cx),
            ViewMode::Thumbs => {
                let rows = n.div_ceil(self.thumb_columns(side)).max(1);
                gpui::uniform_list(("thumbs", side), rows, cx.processor(move |this, range: Range<usize>, window, cx| this.thumb_rows(side, range, window, cx)))
                    .track_scroll(self.scroll[side].clone())
                    .size_full()
                    .into_any_element()
            }
        };

        // The gliding cursor (Full mode): drawn behind the rows, positioned from the spring
        // and the list's scroll offset.
        let pill = (mode == ViewMode::Full && n > 0 && self.panels[side].tab().cursor_visible).then(|| {
            let offset: f32 = self.scroll[side].0.borrow().base_handle.offset().y.into();
            let top = self.cursor_spring[side].value * row_h + offset;
            div()
                .absolute()
                .left(px(6.))
                .right(px(6.))
                .top(px(top))
                .h(px(row_h))
                .rounded(px(8.))
                .when(active, |d| {
                    d.bg(linear_gradient(90., linear_color_stop(c.cursor, 0.), linear_color_stop(c.cursor.blend(c.accent2.opacity(0.22)), 1.)))
                        .shadow(vec![BoxShadow { color: c.cursor.opacity(0.42), offset: point(px(0.), px(4.)), blur_radius: px(14.), spread_radius: px(-3.) }])
                })
                .when(!active, |d| d.border_1().border_color(c.cursor.opacity(0.5)).bg(c.cursor.opacity(0.07)))
        });

        // The selection band, in list coordinates (content coordinates minus the scroll).
        let band = self.marquee.as_ref().filter(|m| m.side == side && m.band.is_drag()).map(|m| {
            let scrolled = if mode == ViewMode::Brief { 0.0 } else { -f32::from(self.scroll[side].0.borrow().base_handle.offset().y) };
            let b = m.band;
            div()
                .absolute()
                .left(px(b.left()))
                .top(px(b.top() - scrolled))
                .w(px(b.right() - b.left()))
                .h(px(b.bottom() - b.top()))
                .rounded(px(3.))
                .bg(c.accent.opacity(0.12))
                .border_1()
                .border_color(c.accent.opacity(0.75))
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .when(mode == ViewMode::Full, |d| d.child(self.render_column_header(side, &cols, c, cx)))
            .child(
                div()
                    .id(("list", side))
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_hidden()
                    .child(canvas(move |bounds, _, _| bounds_cell.set(bounds), |_, _, _, _| {}).absolute().size_full())
                    .children(pill)
                    .child(body)
                    .children(band)
                    .when(only_parent, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(70.))
                                .w_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(8.))
                                .child(div().size(px(56.)).rounded(px(16.)).bg(c.folder.opacity(0.12)).flex().items_center().justify_center().child(icon(icons::FOLDER_OPEN, 28., c.folder)))
                                .child(div().text_size(px(13.)).text_color(c.text).child("This folder is empty"))
                                .child(div().flex().items_center().gap(px(6.)).text_size(px(11.5)).text_color(c.muted).child("Press").child(keycap("F7", c)).child("to create a folder")),
                        )
                    })
                    .drag_over::<DraggedFiles>(move |style, files, _, _| if files.side != side { style.bg(drop_tint) } else { style })
                    .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_tint))
                    .on_drop(cx.listener(move |this, files: &DraggedFiles, w, cx| this.drop_files(side, files, w, cx)))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, w, cx| this.drop_external(side, paths.paths(), w, cx)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, w, cx| this.background_mouse_down(side, ev, w, cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, w, cx| {
                        this.commit_rename(w, cx);
                        this.activate_side(side, w, cx);
                        let menu = this.background_menu(ev.position, cx);
                        this.open_menu(menu, w, cx);
                    })),
            )
    }

    fn render_footer(&mut self, side: usize, s: &Settings, c: &Colors) -> impl IntoElement {
        let tab = self.panels[side].tab();
        let stats = tab.stats();
        let mode = s.view.size_in_footer;
        let finding = side == self.active && !self.type_ahead.is_empty();
        let cursor = tab.cursor_entry().filter(|e| !e.is_parent()).cloned();
        let selected = stats.selected_files + stats.selected_dirs;
        let free = fc_core::volumes::disk_space(tab.working_dir()).map(|(free, _, _)| format::format_size(free, mode));
        let now = SystemTime::now();

        let info: AnyElement = match cursor {
            Some(e) => {
                let size = if e.is_dir_like() { tab.entry_size(&e).map(|b| format::format_size(b, mode)) } else { Some(format::format_size(e.size, mode)) };
                let mut detail = vec![e.kind_label()];
                if let Some(sz) = size {
                    detail.push(sz);
                }
                let when = format::format_time_friendly(e.modified, now);
                if !when.is_empty() {
                    detail.push(when);
                }
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .min_w(px(0.))
                    .flex_1()
                    .overflow_hidden()
                    .child(file_tile(look::kind_of(&e), 16., c, false))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(c.text)
                            .min_w(px(40.))
                            .max_w(px(220.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(e.name.clone()),
                    )
                    .child(div().flex_1().min_w(px(0.)).text_color(c.muted).overflow_hidden().text_ellipsis().whitespace_nowrap().child(detail.join(" · ")))
                    .into_any_element()
            }
            None => div().flex_1().text_color(c.muted).child(plural(stats.total_files + stats.total_dirs, "item")).into_any_element(),
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(34.))
            .px(px(12.))
            .overflow_hidden()
            .border_t_1()
            .border_color(c.border.opacity(0.7))
            .text_size(px(11.5))
            .child(info)
            .when(finding, |d| d.child(chip(format!("Find: {}", self.type_ahead), c, true)))
            .child(if selected > 0 {
                chip(format!("{selected} selected · {}", format::format_size(stats.selected_bytes, mode)), c, true)
            } else {
                chip(format!("{} · {}", plural(stats.total_files, "file"), plural(stats.total_dirs, "folder")), c, false)
            })
            .when_some(free, |d, f| d.child(chip(format!("{f} free"), c, false).relative().child(crate::probe::probe(format!("main.footer.free{side}")))))
    }

    fn render_card(&mut self, side: usize, s: &Settings, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let active = side == self.active;
        let show_tabs = self.panels[side].tabs.len() > 1 || s.view.always_show_tab_bar;
        let closable = side == 1 && s.view.show_right_panel;
        let card = div()
            .relative()
            .child(crate::probe::probe(format!("main.card{side}")))
            .flex()
            .flex_col()
            .size_full()
            .min_w(px(260.))
            .overflow_hidden()
            .rounded(px(14.))
            .bg(c.bg.opacity(if c.dark { 0.92 } else { 0.95 }))
            .border_1()
            .border_color(if active { c.accent.opacity(if c.dark { 0.3 } else { 0.4 }) } else { c.border.opacity(0.8) })
            .shadow(look::card_shadow(c, if active { 1.0 } else { 0.0 }))
            .when(show_tabs, |d| d.child(self.render_tabs(side, closable && !s.view.show_path_bar, c, cx)))
            .when(s.view.show_path_bar, |d| d.child(self.render_card_header(side, !show_tabs, closable, c, cx)))
            .child(self.render_list(side, s, c, window, cx))
            .when(s.view.show_selection_info, |d| d.child(self.render_footer(side, s, c)));
        if active {
            let colors = *c;
            card.with_animation(
                ("glow", self.active_gen as usize),
                Animation::new(Duration::from_millis(650)).with_easing(ease_out_quint()),
                move |el, t| el.shadow(look::card_shadow(&colors, 1.0 + 1.6 * (1.0 - t))),
            )
            .into_any_element()
        } else {
            card.into_any_element()
        }
    }

    // ----- dock ------------------------------------------------------------------------

    fn render_dock(&mut self, s: &Settings, c: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let keymap = cx.global::<AppState>().keymap.clone();
        let dir = self.tab().working_dir().to_path_buf();
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
        let mut dock = div().flex().flex_none().items_center().gap(px(10.)).h(px(54.)).px(px(12.)).pb(px(4.)).overflow_hidden();
        if s.view.show_command_line {
            dock = dock.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .flex_1()
                    .min_w(px(220.))
                    .h(px(38.))
                    .px(px(12.))
                    .rounded(px(11.))
                    .bg(c.bg.opacity(if c.dark { 0.75 } else { 0.85 }))
                    .border_1()
                    .border_color(c.border.opacity(0.8))
                    .on_action(cx.listener(|this, _: &ui::Confirm, w, cx| this.execute_command(w, cx)))
                    .on_action(cx.listener(|this, _: &ui::FocusPanels, w, cx| {
                        this.restore_focus(w);
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &ui::HistoryPrev, _w, cx| this.command_history_step(true, cx)))
                    .on_action(cx.listener(|this, _: &ui::HistoryNext, _w, cx| this.command_history_step(false, cx)))
                    .child(icon(icons::PROMPT, 15., c.accent))
                    .child(
                        div()
                            .px(px(7.))
                            .h(px(20.))
                            .flex()
                            .flex_none()
                            .items_center()
                            .max_w(px(180.))
                            .overflow_hidden()
                            .rounded(px(6.))
                            .bg(c.accent.opacity(0.14))
                            .text_color(c.accent)
                            .text_size(px(11.5))
                            .font_family("Menlo")
                            .whitespace_nowrap()
                            .child(folder),
                    )
                    .child(div().flex_1().min_w(px(0.)).child(self.command.clone())),
            );
        } else {
            dock = dock.child(div().flex_1());
        }
        if s.view.show_buttons {
            for (action, label) in [
                (Action::View, "View"),
                (Action::Edit, "Edit"),
                (Action::Copy, "Copy"),
                (Action::Move, "Move"),
                (Action::NewFolder, "Folder"),
                (Action::Delete, "Delete"),
            ] {
                let key = keymap.chords_for(action).first().map(|c| c.display()).unwrap_or_default();
                let danger = action == Action::Delete;
                let (hover_bg, hover_fg) = if danger { (c.danger.opacity(0.16), c.danger) } else { (c.hover, c.chrome_text) };
                dock = dock.child(
                    div()
                        .id(SharedString::from(format!("fkey-{}", action.id())))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(7.))
                        .h(px(34.))
                        .px(px(9.))
                        .rounded(px(9.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(c.chrome_text)
                        .hover(move |st| st.bg(hover_bg).text_color(hover_fg))
                        .child(keycap(key, c))
                        .child(label)
                        .when(danger, |d| d.relative().child(crate::probe::probe("main.fkey.delete")))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            w.focus(&this.focus);
                            this.run(action, w, cx);
                        })),
                );
            }
        }
        dock
    }

    // ----- embedded terminal -----------------------------------------------------------

    fn render_terminal(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let term = self.terminal.clone().filter(|_| self.terminal_open)?;
        let (focused, cwd) = {
            let t = term.read(cx);
            (t.focus.is_focused(window), t.session.cwd())
        };
        let home = crate::workspace::dirs_home();
        let where_ = cwd
            .map(|p| match p.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => p.display().to_string(),
            })
            .unwrap_or_default();
        let button = |id: &'static str, glyph: &'static str, tip: &'static str, c: &Colors| {
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .h(px(22.))
                .px(px(7.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(c.muted)
                .hover(|st| st.bg(c.hover).text_color(c.chrome_text))
                .child(icon(glyph, 12., c.muted))
                .child(tip)
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(32.))
            .pl(px(12.))
            .pr(px(6.))
            .border_b_1()
            .border_color(c.border.opacity(0.6))
            .text_size(px(12.))
            .child(icon(icons::TERMINAL, 13., if focused { c.accent } else { c.muted }))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c.text).child("Terminal"))
            .child(div().flex_1().min_w(px(0.)).text_color(c.muted).font_family("Menlo").text_size(px(11.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(where_))
            .child(button("term-cd", icons::FOLDER, "cd to panel", c).on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.terminal_to_panel(w, cx);
            })))
            .child(button("term-show", icons::FOLDER_OPEN, "Show in panel", c).on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.panel_to_terminal(w, cx);
            })))
            .child(button("term-app", icons::TERMINAL, "Terminal app", c).on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.run(Action::OpenTerminalApp, w, cx);
            })))
            .child(ghost_button("term-close", icons::CLOSE, c, false, true).size(px(24.)).on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                cx.stop_propagation();
                this.toggle_terminal(w, cx);
            })));
        Some(
            div()
                .id("terminal-panel")
                .relative()
                .child(crate::probe::probe("main.terminal"))
                .flex()
                .flex_col()
                .flex_none()
                .h(px(self.terminal_height))
                .mx(px(12.))
                .mt(px(2.))
                .mb(px(4.))
                .overflow_hidden()
                .rounded(px(14.))
                .bg(c.bg.opacity(if c.dark { 0.92 } else { 0.95 }))
                .border_1()
                .border_color(if focused { c.accent.opacity(if c.dark { 0.3 } else { 0.4 }) } else { c.border.opacity(0.8) })
                .shadow(look::card_shadow(c, if focused { 1.0 } else { 0.0 }))
                .child(header)
                .child(div().flex_1().min_h(px(0.)).child(term))
                // Drag the top edge to resize.
                .child(
                    div()
                        .id("terminal-resize")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(5.))
                        .cursor_row_resize()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| {
                            cx.stop_propagation();
                            this.dragging_terminal = true;
                        })),
                )
                .into_any_element(),
        )
    }

    // ----- floating overlays -----------------------------------------------------------

    fn render_job_cards(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let active: Vec<_> = self.jobs.active().take(4).cloned().collect();
        if active.is_empty() {
            return None;
        }
        let waiting = self.jobs.prompt.as_ref().map(|(id, _)| *id);
        let mut stack = div().absolute().right(px(16.)).bottom(px(66.)).w(px(360.)).flex().flex_col().gap(px(8.));
        for job in active {
            let id = job.id;
            let pct = job.progress.fraction();
            let glyph = if job.title.contains("Trash") || job.title.starts_with("Deleting") {
                icons::TRASH
            } else if job.title.starts_with("Moving") {
                icons::MOVE
            } else if job.title.starts_with("Compressing") || job.title.starts_with("Extracting") {
                icons::ZIP
            } else {
                icons::COPY
            };
            let detail = if waiting == Some(id) {
                "Waiting for your answer".to_string()
            } else if job.state == fc_core::queue::JobState::Queued {
                "Queued".to_string()
            } else if job.progress.total_bytes > 0 {
                format!(
                    "{} of {} · {:.0}%",
                    format::format_size(job.progress.done_bytes.min(job.progress.total_bytes), SizeMode::Dynamic),
                    format::format_size(job.progress.total_bytes, SizeMode::Dynamic),
                    pct * 100.0
                )
            } else {
                format!("{} of {} items", job.progress.done_items, job.progress.total_items)
            };
            let shimmer = div()
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(70.))
                .bg(linear_gradient(90., linear_color_stop(c.on_accent.opacity(0.0), 0.), linear_color_stop(c.on_accent.opacity(0.4), 1.)))
                .with_animation(("shimmer", id as usize), Animation::new(Duration::from_millis(1400)).repeat(), |el, t| el.left(relative(t * 1.3 - 0.3)));
            let card = div()
                .p(px(12.))
                .rounded(px(13.))
                .bg(c.surface)
                .border_1()
                .border_color(c.border)
                .shadow(look::soft_shadow(c))
                .flex()
                .flex_col()
                .gap(px(9.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().size(px(30.)).rounded(px(9.)).bg(c.accent.opacity(0.16)).flex().items_center().justify_center().child(icon(glyph, 15., c.accent)))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(c.text).overflow_hidden().text_ellipsis().whitespace_nowrap().child(job.title.clone()))
                                .child(div().text_size(px(11.)).text_color(c.muted).child(detail)),
                        )
                        .child(ghost_button(SharedString::from(format!("job-cancel-{id}")), icons::CLOSE, c, false, true).size(px(24.)).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_this, _, _w, cx| {
                                cx.stop_propagation();
                                cx.global::<AppState>().jobs.cancel(id);
                            }),
                        )),
                )
                .child(
                    div()
                        .relative()
                        .h(px(6.))
                        .w_full()
                        .rounded_full()
                        .bg(c.border)
                        .overflow_hidden()
                        .child(div().h_full().rounded_full().w(relative(pct.clamp(0.02, 1.0))).bg(linear_gradient(90., linear_color_stop(c.accent, 0.), linear_color_stop(c.accent2, 1.))))
                        .child(shimmer),
                );
            stack = stack.child(card.with_animation(
                ("job-in", id as usize),
                Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()),
                |el, t| el.opacity(t).mr(px((1.0 - t) * -24.0)),
            ));
        }
        Some(stack)
    }

    fn render_toast(&self, c: &Colors) -> Option<impl IntoElement> {
        let toast = self.toast.as_ref()?;
        let (glyph, tint) = if toast.error { (icons::WARNING, c.danger) } else { (icons::CHECK, c.success) };
        Some(
            div().absolute().bottom(px(66.)).left_0().right_0().flex().justify_center().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .max_w(px(720.))
                    .h(px(36.))
                    .pl(px(8.))
                    .pr(px(16.))
                    .rounded_full()
                    .bg(c.surface)
                    .border_1()
                    .border_color(if toast.error { c.danger.opacity(0.6) } else { c.border })
                    .shadow(look::soft_shadow(c))
                    .overflow_hidden()
                    .text_size(px(12.5))
                    .text_color(c.text)
                    .child(div().size(px(22.)).flex_none().rounded_full().bg(tint.opacity(0.18)).flex().items_center().justify_center().child(icon(glyph, 12., tint)))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(toast.text.clone()))
                    .with_animation(
                        ("toast", self.toast_gen as usize),
                        Animation::new(Duration::from_millis(280)).with_easing(ease_out_quint()),
                        |el, t| el.opacity(t).mt(px((1.0 - t) * 16.0)),
                    ),
            ),
        )
    }
}

fn import_commander_one(this: &mut Workspace, cx: &mut Context<Workspace>) {
    match fc_core::settings::commander_one_keymap_json() {
        Some(json) => match fc_core::Keymap::from_commander_one_json(&json) {
            Ok((keymap, skipped)) => {
                let n = keymap.bindings.len();
                let state = cx.global_mut::<AppState>();
                state.keymap = keymap.with_additions();
                state.save_keymap();
                crate::state::install_key_bindings(cx);
                crate::state::install_menus(cx);
                this.notify_toast(format!("Imported {n} shortcuts from Commander One ({} skipped)", skipped.len()), false);
            }
            Err(e) => this.notify_toast(format!("Couldn't read Commander One's shortcuts: {e}"), true),
        },
        None => this.notify_toast("Commander One's preferences weren't found on this Mac.", true),
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frames.set(self.frames.get() + 1);
        if self.animate() {
            window.request_animation_frame();
        }
        let c = Colors::get(window, cx);
        text_input::set_style(InputStyle { text: c.text, muted: c.muted, bg: c.bg, border: c.border, accent: c.accent, selection: c.accent.opacity(0.28) });
        let s = cx.global::<AppState>().settings.clone();
        let dual = s.view.show_right_panel;
        if !dual && self.active == 1 {
            self.active = 0;
        }
        let split = {
            let viewport: f32 = window.viewport_size().width.into();
            let usable = (viewport - if s.view.show_sidebar { SIDEBAR_WIDTH } else { 0.0 } - 40.0).max(1.0);
            let min = (280.0 / usable).min(0.5);
            self.split.clamp(min, 1.0 - min)
        };

        let left = self.render_card(0, &s, &c, window, cx);
        let mut cards: Vec<AnyElement> = Vec::new();
        if dual {
            let right = self.render_card(1, &s, &c, window, cx);
            cards.push(div().h_full().w(relative(split)).child(left).into_any_element());
            cards.push(
                div()
                    .id("splitter")
                    .w(px(12.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_col_resize()
                    .group("splitter")
                    .child(div().w(px(3.)).h(px(44.)).rounded_full().group_hover("splitter", |st| st.bg(c.accent.opacity(0.6))))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| {
                        cx.stop_propagation();
                        this.dragging_split = true;
                    }))
                    .into_any_element(),
            );
            cards.push(div().h_full().flex_1().min_w(px(0.)).child(right).into_any_element());
        } else {
            cards.push(div().h_full().flex_1().min_w(px(0.)).child(left).into_any_element());
        }
        let terminal = self.render_terminal(&c, window, cx);
        let panels = actions::route(
            div()
                .id("panels")
                // While a name is edited in place, panel shortcuts (Space, ⌫, arrows…) must
                // reach the text field instead.
                .key_context(if self.rename.is_some() { "PaneRenaming" } else { PANEL_CONTEXT })
                .track_focus(&self.focus)
                .flex()
                .flex_row()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .pt(px(2.))
                .pb(px(4.))
                .pr(px(12.))
                .pl(px(if s.view.show_sidebar { 4. } else { 12. }))
                .on_key_down(cx.listener(Self::on_panel_key_down))
                .on_action(cx.listener(|this, _: &ui::CursorUp, w, cx| this.on_nav(UiNav::Up(false), w, cx)))
                .on_action(cx.listener(|this, _: &ui::CursorDown, w, cx| this.on_nav(UiNav::Down(false), w, cx)))
                .on_action(cx.listener(|this, _: &ui::CursorLeft, w, cx| this.on_nav(UiNav::Left, w, cx)))
                .on_action(cx.listener(|this, _: &ui::CursorRight, w, cx| this.on_nav(UiNav::Right, w, cx)))
                .on_action(cx.listener(|this, _: &ui::PageUp, w, cx| this.on_nav(UiNav::PageUp(false), w, cx)))
                .on_action(cx.listener(|this, _: &ui::PageDown, w, cx| this.on_nav(UiNav::PageDown(false), w, cx)))
                .on_action(cx.listener(|this, _: &ui::CursorHome, w, cx| this.on_nav(UiNav::Home(false), w, cx)))
                .on_action(cx.listener(|this, _: &ui::CursorEnd, w, cx| this.on_nav(UiNav::End(false), w, cx)))
                .on_action(cx.listener(|this, _: &ui::ExtendUp, w, cx| this.on_nav(UiNav::Up(true), w, cx)))
                .on_action(cx.listener(|this, _: &ui::ExtendDown, w, cx| this.on_nav(UiNav::Down(true), w, cx)))
                .on_action(cx.listener(|this, _: &ui::ExtendPageUp, w, cx| this.on_nav(UiNav::PageUp(true), w, cx)))
                .on_action(cx.listener(|this, _: &ui::ExtendPageDown, w, cx| this.on_nav(UiNav::PageDown(true), w, cx)))
                .on_action(cx.listener(|this, _: &ui::ExtendHome, w, cx| this.on_nav(UiNav::Home(true), w, cx)))
                .on_action(cx.listener(|this, _: &ui::ExtendEnd, w, cx| this.on_nav(UiNav::End(true), w, cx)))
                .on_action(cx.listener(|this, _: &ui::GoUpKey, w, cx| this.on_nav(UiNav::GoUp, w, cx)))
                .on_action(cx.listener(|this, _: &ui::ClearTypeAhead, w, cx| this.on_nav(UiNav::ClearTypeAhead, w, cx)))
                .on_action(cx.listener(|this, _: &ui::Confirm, w, cx| this.commit_rename(w, cx)))
                .on_action(cx.listener(|this, _: &ui::Cancel, w, cx| this.cancel_rename(w, cx)))
                .children(cards),
            cx,
            Workspace::run,
        );

        div()
            .id("workspace")
            .child(crate::probe::probe("main.root"))
            .key_context("Workspace")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(linear_gradient(180., linear_color_stop(c.bg_top, 0.), linear_color_stop(c.bg_bottom, 1.)))
            .text_color(c.chrome_text)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .on_action(cx.listener(|this, _: &ui::Quit, w, cx| this.quit(w, cx)))
            .on_action(cx.listener(|this, _: &ui::OpenPalette, w, cx| this.open_palette(w, cx)))
            .on_action(cx.listener(|this, _: &ui::ToggleTerminal, w, cx| this.toggle_terminal(w, cx)))
            .on_action(cx.listener(|this, _: &ui::CloseWindow, w, cx| {
                this.save_session(w, cx);
                w.remove_window();
            }))
            .on_action(cx.listener(|_, _: &ui::Minimize, w, _| w.minimize_window()))
            .on_action(cx.listener(|_, _: &ui::Zoom, w, _| w.zoom_window()))
            .on_action(cx.listener(|_, _: &ui::Hide, _, cx| cx.hide()))
            .on_action(cx.listener(|_, _: &ui::HideOthers, _, cx| cx.hide_other_apps()))
            .on_action(cx.listener(|_, _: &ui::ShowAll, _, cx| cx.unhide_other_apps()))
            .on_action(cx.listener(|this, _: &ui::About, w, cx| {
                let d = dialogs::Dialog::Message {
                    title: format!("Ferrous Commander {}", env!("CARGO_PKG_VERSION")),
                    body: "A dual-panel file manager for macOS written in Rust with GPUI. Built clean-room from a behavioral specification (SPEC.md).".into(),
                };
                this.open_dialog(d, w, cx);
            }))
            .on_action(cx.listener(|_, _: &ui::ShowHotkeys, _, cx| crate::prefs::open(crate::prefs::Page::Hotkeys, cx)))
            .on_action(cx.listener(|this, _: &ui::ImportCommanderOneKeymap, _, cx| import_commander_one(this, cx)))
            .on_action(cx.listener(|this, _: &ui::ResetKeymap, _, cx| {
                let state = cx.global_mut::<AppState>();
                state.keymap = fc_core::Keymap::defaults();
                state.save_keymap();
                crate::state::install_key_bindings(cx);
                crate::state::install_menus(cx);
                this.notify_toast("Shortcuts reset to defaults", false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ui::RevealConfig, _, cx| {
                let dir = cx.global::<AppState>().config_dir.clone();
                let _ = std::fs::create_dir_all(&dir);
                if let Err(e) = fc_core::platform::reveal_in_finder(&[dir]) {
                    this.notify_toast(e.to_string(), true);
                }
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, w, cx| this.mouse_moved(ev.position, ev.pressed_button == Some(MouseButton::Left), w, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _w, cx| this.mouse_released(cx)))
            .child(self.render_titlebar(&c, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.))
                    .when(s.view.show_sidebar, |d| d.child(self.render_sidebar(&c, cx)))
                    .child(panels),
            )
            .children(terminal)
            .when(s.view.show_command_line || s.view.show_buttons, |d| d.child(self.render_dock(&s, &c, cx)))
            .children(self.render_job_cards(&c, cx))
            .children(self.render_toast(&c))
            .children(dialogs::render(self, window, cx))
            .children(dialogs::render_prompt(self, window, cx))
            .children(palette::render(self, window, cx))
            .children(menus::render(self, window, cx))
    }
}
