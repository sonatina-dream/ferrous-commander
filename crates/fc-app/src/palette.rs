//! Command palette (⌘P): fuzzy-search commands, places, favorites and recent folders.
//! `/` or `~` jumps to a path, `>` runs a shell command in Terminal.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    div, ease_out_quint, prelude::*, px, Animation, AnimationExt, AnyElement, Context, Entity, MouseButton,
    Subscription, Window,
};

use fc_core::fuzzy;
use fc_core::keymap::Action;

use crate::actions::ui;
use crate::colors::Colors;
use crate::icons;
use crate::look::{self, keycap};
use crate::state::AppState;
use crate::text_input::{Changed, TextInput};
use crate::workspace::Workspace;

#[derive(Clone, Debug, PartialEq)]
pub enum PaletteItem {
    Action(Action),
    Go { label: String, path: PathBuf, icon: &'static str, group: &'static str },
    OpenPath(PathBuf),
    Shell(String),
    Find(String),
}

pub struct Palette {
    pub input: Entity<TextInput>,
    pub items: Vec<PaletteItem>,
    pub selected: usize,
    pub generation: u64,
    _changed: Subscription,
}

/// Commands that make no sense from the palette.
const HIDDEN: &[Action] = &[Action::OpenItem, Action::EnterItem, Action::SelectItem, Action::SelectItemWithSize, Action::SwitchPanel];

/// Shown when the query is empty.
const FEATURED: &[Action] = &[
    Action::NewFolder,
    Action::Search,
    Action::Copy,
    Action::Move,
    Action::ToggleHidden,
    Action::Compress,
    Action::ShowTerminal,
    Action::Preferences,
];

impl Workspace {
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        let input = cx.new(|cx| TextInput::new(cx, "", "Search commands, folders and favorites…").with_context("Palette").bare().text_size(16.));
        let sub = cx.subscribe(&input, |ws: &mut Workspace, _, _: &Changed, cx| {
            ws.refresh_palette(cx);
            cx.notify();
        });
        window.focus(&input.read(cx).focus.clone());
        self.palette_generation += 1;
        self.palette = Some(Palette { input, items: Vec::new(), selected: 0, generation: self.palette_generation, _changed: sub });
        self.refresh_palette(cx);
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.restore_focus(window);
            cx.notify();
        }
    }

    fn places_for_palette(&self, cx: &Context<Self>) -> Vec<PaletteItem> {
        let mut out: Vec<PaletteItem> = crate::workspace::places()
            .into_iter()
            .map(|(label, path)| PaletteItem::Go { label: label.to_string(), icon: place_icon(label), path, group: "Place" })
            .collect();
        for v in &self.volumes {
            out.push(PaletteItem::Go { label: v.name.clone(), path: v.path.clone(), icon: icons::DISK, group: "Volume" });
        }
        for f in &cx.global::<AppState>().settings.favorites {
            out.push(PaletteItem::Go { label: display_name(f), path: f.clone(), icon: icons::STAR, group: "Favorite" });
        }
        for p in self.panels[self.active].folder_history.iter().take(12) {
            out.push(PaletteItem::Go { label: display_name(p), path: p.clone(), icon: icons::CLOCK, group: "Recent" });
        }
        out
    }

    /// Recomputes results for the current query.
    pub fn refresh_palette(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else { return };
        let query = palette.input.read(cx).text().trim().to_string();
        let mut items: Vec<PaletteItem> = Vec::new();

        if let Some(cmd) = query.strip_prefix('>') {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                items.push(PaletteItem::Shell(cmd.to_string()));
            }
        } else if query.starts_with('/') || query.starts_with('~') {
            let path = fc_core::ops::expand_tilde(std::path::Path::new(&query));
            items.push(PaletteItem::OpenPath(path.clone()));
            // Offer matching children of the typed folder.
            let (dir, prefix) = if query.ends_with('/') {
                (path.clone(), String::new())
            } else {
                (path.parent().map(|p| p.to_path_buf()).unwrap_or_default(), path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default())
            };
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let mut children: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .filter(|p| p.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy().to_lowercase();
                        !n.starts_with('.') && n.starts_with(&prefix)
                    }))
                    .collect();
                children.sort();
                for c in children.into_iter().take(8) {
                    items.push(PaletteItem::Go { label: display_name(&c), path: c, icon: icons::FOLDER, group: "Folder" });
                }
            }
        } else if query.is_empty() {
            items.extend(FEATURED.iter().map(|a| PaletteItem::Action(*a)));
            items.extend(self.places_for_palette(cx).into_iter().filter(|i| matches!(i, PaletteItem::Go { group: "Recent", .. })).take(5));
        } else {
            let mut scored: Vec<(i32, PaletteItem)> = Vec::new();
            for a in Action::ALL.iter().copied().filter(|a| !HIDDEN.contains(a)) {
                if let Some(s) = fuzzy::score(&query, a.label()) {
                    scored.push((s + 4, PaletteItem::Action(a)));
                }
            }
            for item in self.places_for_palette(cx) {
                if let PaletteItem::Go { label, path, .. } = &item {
                    let s = fuzzy::score(&query, label).or_else(|| fuzzy::score(&query, &path.to_string_lossy()).map(|s| s - 10));
                    if let Some(s) = s {
                        scored.push((s, item));
                    }
                }
            }
            scored.sort_by_key(|s| std::cmp::Reverse(s.0));
            let mut seen: Vec<PaletteItem> = Vec::new();
            for (_, item) in scored {
                if !seen.contains(&item) {
                    seen.push(item);
                }
            }
            items = seen.into_iter().take(40).collect();
            items.push(PaletteItem::Find(query.clone()));
        }

        if let Some(p) = self.palette.as_mut() {
            p.items = items;
            p.selected = 0;
        }
    }

    pub fn palette_step(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.as_mut() {
            let n = p.items.len();
            if n > 0 {
                p.selected = if down { (p.selected + 1) % n } else { (p.selected + n - 1) % n };
            }
            cx.notify();
        }
    }

    pub fn palette_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.palette.as_ref().and_then(|p| p.items.get(p.selected).cloned()) else { return };
        self.run_palette_item(item, window, cx);
    }

    pub fn run_palette_item(&mut self, item: PaletteItem, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(window, cx);
        match item {
            PaletteItem::Action(a) => self.run(a, window, cx),
            PaletteItem::Go { path, .. } => self.navigate(self.active, path, None, window, cx),
            PaletteItem::OpenPath(path) => {
                if path.is_dir() {
                    self.navigate(self.active, path, None, window, cx);
                } else if path.exists() {
                    let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("/"));
                    self.navigate(self.active, parent, Some(&path), window, cx);
                } else {
                    self.notify_toast(format!("\"{}\" doesn't exist", path.display()), true);
                }
            }
            PaletteItem::Shell(cmd) => {
                let dir = self.tab().working_dir().to_path_buf();
                if let Err(e) = fc_core::platform::run_in_terminal(&dir, &cmd) {
                    self.notify_toast(e.to_string(), true);
                }
            }
            PaletteItem::Find(text) => {
                let roots = vec![self.tab().working_dir().to_path_buf()];
                crate::search::open_with(roots, false, Some(text), cx.entity().downgrade(), window.window_handle(), cx);
            }
        }
    }
}

fn display_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn place_icon(label: &str) -> &'static str {
    match label {
        "Home" => icons::HOME,
        "Desktop" => icons::DESKTOP,
        "Documents" => icons::DOCS,
        "Downloads" => icons::DOWNLOAD,
        "Applications" => icons::APPS,
        _ => icons::DISK,
    }
}

/// Renders the palette overlay, if open.
pub fn render(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let palette = ws.palette.as_ref()?;
    let c = Colors::get(window, cx);
    let keymap = cx.global::<AppState>().keymap.clone();
    let query_empty = palette.input.read(cx).text().is_empty();

    let mut list = div().id("palette-list").flex().flex_col().max_h(px(392.)).overflow_y_scroll().px(px(6.)).pb(px(6.));
    if query_empty {
        list = list.child(look::section_label("Suggested", &c));
    }
    for (ix, item) in palette.items.iter().enumerate() {
        let selected = ix == palette.selected;
        let (glyph, tint, label, detail, right): (&'static str, gpui::Hsla, String, String, AnyElement) = match item {
            PaletteItem::Action(a) => {
                let keys = keymap.chords_for(*a).first().map(|ch| ch.display());
                (
                    look::action_icon(*a),
                    c.accent,
                    a.label().to_string(),
                    a.group().title().to_string(),
                    match keys {
                        Some(k) => keycap(k, &c).into_any_element(),
                        None => div().into_any_element(),
                    },
                )
            }
            PaletteItem::Go { label, path, icon, group } => (
                icon,
                c.folder,
                label.clone(),
                path.display().to_string(),
                look::chip(*group, &c, false).into_any_element(),
            ),
            PaletteItem::OpenPath(p) => (icons::FOLDER_OPEN, c.folder, format!("Open {}", p.display()), "Go to this path".into(), keycap("↩", &c).into_any_element()),
            PaletteItem::Shell(cmd) => (icons::PROMPT, c.success, format!("Run “{cmd}”"), "In Terminal, from the current folder".into(), keycap("↩", &c).into_any_element()),
            PaletteItem::Find(q) => (icons::SEARCH, c.accent2, format!("Search for “{q}”"), "Find files in this folder and below".into(), keycap("⌥F7", &c).into_any_element()),
        };
        let run = item.clone();
        list = list.child(
            div()
                .id(("palette-item", ix))
                .flex()
                .items_center()
                .gap(px(11.))
                .h(px(42.))
                .px(px(10.))
                .rounded(px(9.))
                .cursor_pointer()
                .when(selected, |d| d.bg(c.accent.opacity(0.16)))
                .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .rounded(px(8.))
                        .bg(tint.opacity(if selected { 0.28 } else { 0.15 }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(look::icon(glyph, 15., tint)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(13.5)).text_color(if selected { c.text } else { c.text.opacity(0.92) }).overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
                        .child(div().text_size(px(11.)).text_color(c.muted).overflow_hidden().text_ellipsis().whitespace_nowrap().child(detail)),
                )
                .child(right)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                    cx.stop_propagation();
                    this.run_palette_item(run.clone(), w, cx);
                })),
        );
    }
    if palette.items.is_empty() {
        list = list.child(div().p(px(18.)).text_color(c.muted).child("No matches"));
    }

    let generation = palette.generation;
    let card = div()
        .id("palette")
        .w(px(640.))
        .max_w(relative_width())
        .rounded(px(16.))
        .bg(c.surface)
        .border_1()
        .border_color(c.border)
        .shadow(vec![
            gpui::BoxShadow { color: c.shadow, offset: gpui::point(px(0.), px(24.)), blur_radius: px(60.), spread_radius: px(-10.) },
            gpui::BoxShadow { color: c.accent.opacity(if c.dark { 0.07 } else { 0.1 }), offset: gpui::point(px(0.), px(0.)), blur_radius: px(40.), spread_radius: px(0.) },
        ])
        .overflow_hidden()
        .on_action(cx.listener(|this, _: &ui::MenuUp, _w, cx| this.palette_step(false, cx)))
        .on_action(cx.listener(|this, _: &ui::MenuDown, _w, cx| this.palette_step(true, cx)))
        .on_action(cx.listener(|this, _: &ui::Confirm, w, cx| this.palette_confirm(w, cx)))
        .on_action(cx.listener(|this, _: &ui::Cancel, w, cx| this.close_palette(w, cx)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(56.))
                .px(px(16.))
                .border_b_1()
                .border_color(c.border)
                .child(look::icon(icons::COMMAND, 18., c.accent))
                .child(div().flex_1().min_w(px(0.)).child(palette.input.clone()))
                .child(keycap("esc", &c)),
        )
        .child(list)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .h(px(34.))
                .px(px(16.))
                .border_t_1()
                .border_color(c.border)
                .bg(c.sunken)
                .text_size(px(11.))
                .text_color(c.muted)
                .child(div().flex().items_center().gap(px(5.)).child(keycap("↑↓", &c)).child("move"))
                .child(div().flex().items_center().gap(px(5.)).child(keycap("↩", &c)).child("run"))
                .child(div().flex_1())
                .child(div().flex().items_center().gap(px(5.)).child(keycap("/", &c)).child("path"))
                .child(div().flex().items_center().gap(px(5.)).child(keycap(">", &c)).child("shell")),
        );

    Some(
        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(c.scrim)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(78.))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| this.close_palette(w, cx)))
            .child(
                card.with_animation(
                    ("palette-in", generation as usize),
                    Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                    |el, t| el.opacity(t).mt(px((1.0 - t) * -14.0)),
                ),
            )
            .into_any_element(),
    )
}

fn relative_width() -> gpui::DefiniteLength {
    gpui::relative(0.9)
}
