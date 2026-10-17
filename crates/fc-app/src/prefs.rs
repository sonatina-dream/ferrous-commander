//! Preferences window: General, View, Appearance, Hotkeys, Advanced.

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Entity, FocusHandle, FontWeight, Global, KeyDownEvent,
    MouseButton, SharedString, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
};

use fc_core::format::SizeMode;
use fc_core::keymap::{Action, ActionGroup, Chord};
use fc_core::settings::{Settings, ViewerChoice};
use fc_core::theme;

use crate::actions::ui;
use crate::colors::{hsla, Colors};
use crate::state::{self, AppState};
use crate::text_input::{self, Changed, InputStyle, TextInput};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General,
    View,
    Appearance,
    Hotkeys,
    Advanced,
}

impl Page {
    pub const ALL: [Page; 5] = [Page::General, Page::View, Page::Appearance, Page::Hotkeys, Page::Advanced];

    fn icon(self) -> &'static str {
        match self {
            Page::General => crate::icons::GEAR,
            Page::View => crate::icons::LIST,
            Page::Appearance => crate::icons::SPARKLE,
            Page::Hotkeys => crate::icons::COMMAND,
            Page::Advanced => crate::icons::DISK,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::View => "View",
            Page::Appearance => "Appearance",
            Page::Hotkeys => "Hotkeys",
            Page::Advanced => "Advanced",
        }
    }
}

#[derive(Default)]
struct PrefsWindow(Option<WindowHandle<PrefsView>>);

impl Global for PrefsWindow {}

pub struct PrefsView {
    pub page: Page,
    focus: FocusHandle,
    /// Hotkey slot waiting for a key press: (command, 0 = primary / 1 = secondary).
    capture: Option<(Action, usize)>,
    editor: Entity<TextInput>,
    _appearance: gpui::Subscription,
}

/// Opens (or focuses) the preferences window on `page`.
pub fn open(page: Page, cx: &mut App) {
    if !cx.has_global::<PrefsWindow>() {
        cx.set_global(PrefsWindow::default());
    }
    if let Some(handle) = cx.global::<PrefsWindow>().0 {
        let ok = handle.update(cx, |view, window, cx| {
            view.page = page;
            window.activate_window();
            cx.notify();
        });
        if ok.is_ok() {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(820.), px(620.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Settings".into()), appears_transparent: true, traffic_light_position: Some(gpui::point(px(16.), px(16.))) }),
            window_min_size: Some(size(px(680.), px(460.))),
            focus: true,
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus);
                let current = cx.global::<AppState>().settings.general.editor.clone().unwrap_or_default();
                let editor = cx.new(|cx| TextInput::new(cx, current, "Default text editor (leave empty), or an app name such as Visual Studio Code"));
                cx.subscribe(&editor, |_this: &mut PrefsView, input, _: &Changed, cx| {
                    let text = input.read(cx).text().trim().to_string();
                    update_settings(cx, |s| s.general.editor = (!text.is_empty()).then_some(text));
                })
                .detach();
                let appearance = cx.observe_window_appearance(window, |_, window, _| window.refresh());
                PrefsView { page, focus, capture: None, editor, _appearance: appearance }
            })
        },
    );
    if let Ok(h) = handle {
        cx.global_mut::<PrefsWindow>().0 = Some(h);
    }
}

/// Applies a settings change, saves it, refreshes all windows and reloads listings.
fn update_settings(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    let before = cx.global::<AppState>().settings.list_options();
    {
        let state = cx.global_mut::<AppState>();
        f(&mut state.settings);
        state.save_settings();
    }
    if cx.global::<AppState>().settings.list_options() != before {
        state::reload_all_workspaces(cx);
    }
    cx.refresh_windows();
}

fn checkbox(id: impl Into<SharedString>, label: &str, on: bool, c: &Colors, cx: &mut Context<PrefsView>, f: impl Fn(&mut Settings) + 'static) -> impl IntoElement {
    let id: SharedString = id.into();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(3.))
        .cursor_pointer()
        .child(
            div()
                .size(px(15.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { c.accent } else { c.border })
                .bg(if on { c.accent } else { c.bg })
                .flex()
                .items_center()
                .justify_center()
                .text_color(c.on_accent)
                .text_size(px(10.))
                .child(if on { "✓" } else { "" }),
        )
        .child(div().flex_1().min_w(px(0.)).child(label.to_string()))
        .on_mouse_down(MouseButton::Left, cx.listener(move |_this, _, _w, cx| update_settings(cx, &f)))
}

fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str)],
    current: T,
    c: &Colors,
    cx: &mut Context<PrefsView>,
    set: fn(&mut Settings, T),
) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().gap(px(4.));
    for (i, (value, label)) in options.iter().copied().enumerate() {
        let on = value == current;
        row = row.child(
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(if on { c.accent } else { c.border })
                .when(on, |d| d.bg(c.accent).text_color(c.on_accent))
                .cursor_pointer()
                .child(label)
                .on_mouse_down(MouseButton::Left, cx.listener(move |_this, _, _w, cx| update_settings(cx, |s| set(s, value)))),
        );
    }
    row
}

fn section(title: &str, c: &Colors) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pb(px(12.))
        .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted).pb(px(4.)).child(title.to_string()))
}

fn row(label: &str, control: impl IntoElement, c: &Colors) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(3.))
        .child(div().w(px(170.)).flex_none().text_color(c.muted).child(label.to_string()))
        .child(div().flex_1().min_w(px(0.)).child(control))
}

/// Converts a GPUI keystroke into a core chord.
fn chord_from_keystroke(ks: &gpui::Keystroke) -> Option<Chord> {
    let key = ks.key.to_lowercase();
    if matches!(key.as_str(), "shift" | "control" | "alt" | "platform" | "function" | "") {
        return None;
    }
    let mut text = String::new();
    if ks.modifiers.control { text.push_str("ctrl-"); }
    if ks.modifiers.alt { text.push_str("alt-"); }
    if ks.modifiers.shift { text.push_str("shift-"); }
    if ks.modifiers.platform { text.push_str("cmd-"); }
    text.push_str(&key);
    Chord::parse(&text).ok()
}

impl PrefsView {
    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((action, slot)) = self.capture else { return };
        let ks = &ev.keystroke;
        let no_mods = !ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.shift && !ks.modifiers.platform;
        if ks.key == "escape" && no_mods {
            self.capture = None;
        } else if (ks.key == "backspace" || ks.key == "delete") && no_mods {
            self.set_binding(action, slot, None, cx);
        } else if let Some(chord) = chord_from_keystroke(ks) {
            self.set_binding(action, slot, Some(chord), cx);
        } else {
            return;
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn set_binding(&mut self, action: Action, slot: usize, chord: Option<Chord>, cx: &mut Context<Self>) {
        self.capture = None;
        let state = cx.global_mut::<AppState>();
        state.keymap.set_binding(action, slot, chord);
        state.save_keymap();
        state::install_key_bindings(cx);
        state::install_menus(cx);
        cx.refresh_windows();
    }

    fn page_general(&mut self, s: &Settings, c: &Colors, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .child(
                section("Behavior", c)
                    .child(checkbox("g-trash", "Move to Trash when deleting (⇧F8 deletes permanently)", s.general.move_to_trash, c, cx, |s| s.general.move_to_trash = !s.general.move_to_trash))
                    .child(checkbox("g-space", "Calculate folder size on Space", s.general.folder_size_on_space, c, cx, |s| s.general.folder_size_on_space = !s.general.folder_size_on_space))
                    .child(checkbox("g-files", "Use + / − / * selection for files only", s.general.select_files_only, c, cx, |s| s.general.select_files_only = !s.general.select_files_only))
                    .child(checkbox("g-root", "Go to the root when changing drive", s.general.go_root_on_drive_change, c, cx, |s| s.general.go_root_on_drive_change = !s.general.go_root_on_drive_change)),
            )
            .child(
                section("Applications", c)
                    .child(row(
                        "View files with",
                        segmented("viewer", &[(ViewerChoice::Internal, "Internal viewer"), (ViewerChoice::QuickLook, "Quick Look"), (ViewerChoice::Preview, "Preview")], s.general.viewer, c, cx, |s, v| s.general.viewer = v),
                        c,
                    ))
                    .child(row("Edit files with", div().relative().w_full().max_w(px(420.)).child(crate::probe::probe("prefs.editor")).child(self.editor.clone()), c)),
            )
    }

    fn page_view(&mut self, s: &Settings, c: &Colors, cx: &mut Context<Self>) -> gpui::Div {
        let sizes = [(SizeMode::Dynamic, "Dynamic (KB, MB…)"), (SizeMode::Bytes, "Bytes")];
        let width = s.view.brief_column_width;
        div()
            .child(
                section("Files", c)
                    .child(checkbox("v-hidden", "Show hidden files", s.view.show_hidden, c, cx, |s| s.view.show_hidden = !s.view.show_hidden))
                    .child(checkbox("v-top", "Show folders on top", s.view.folders_on_top, c, cx, |s| s.view.folders_on_top = !s.view.folders_on_top))
                    .child(checkbox("v-parent", "Show the \"..\" row to go up a folder", s.view.show_parent_entry, c, cx, |s| s.view.show_parent_entry = !s.view.show_parent_entry))
                    .child(checkbox("v-tabs", "Always show the tab bar", s.view.always_show_tab_bar, c, cx, |s| s.view.always_show_tab_bar = !s.view.always_show_tab_bar))
                    .child(row("Size in columns", segmented("size-col", &sizes, s.view.size_in_columns, c, cx, |s, v| s.view.size_in_columns = v), c))
                    .child(row("Size in header/footer", segmented("size-foot", &sizes, s.view.size_in_footer, c, cx, |s, v| s.view.size_in_footer = v), c))
                    .child(row(
                        "Brief column width",
                        segmented("brief-w", &[(160.0f32, "Narrow"), (220.0, "Normal"), (300.0, "Wide")], if width < 190.0 { 160.0 } else if width < 260.0 { 220.0 } else { 300.0 }, c, cx, |s, v| s.view.brief_column_width = v),
                        c,
                    )),
            )
            .child(
                section("Window", c)
                    .child(checkbox("v-right", "Right panel (two panels side by side)", s.view.show_right_panel, c, cx, |s| s.view.show_right_panel = !s.view.show_right_panel))
                    .child(checkbox("v-toolbar", "Toolbar", s.view.show_toolbar, c, cx, |s| s.view.show_toolbar = !s.view.show_toolbar))
                    .child(checkbox("v-disklist", "Drive bar (disk list)", s.view.show_disk_list, c, cx, |s| s.view.show_disk_list = !s.view.show_disk_list))
                    .child(checkbox("v-diskinfo", "Free space (disk info)", s.view.show_disk_info, c, cx, |s| s.view.show_disk_info = !s.view.show_disk_info))
                    .child(checkbox("v-path", "Path bar", s.view.show_path_bar, c, cx, |s| s.view.show_path_bar = !s.view.show_path_bar))
                    .child(checkbox("v-sel", "Selection info (footer)", s.view.show_selection_info, c, cx, |s| s.view.show_selection_info = !s.view.show_selection_info))
                    .child(checkbox("v-cmd", "Command line", s.view.show_command_line, c, cx, |s| s.view.show_command_line = !s.view.show_command_line))
                    .child(checkbox("v-keys", "Function-key buttons", s.view.show_buttons, c, cx, |s| s.view.show_buttons = !s.view.show_buttons)),
            )
    }

    fn page_appearance(&mut self, s: &Settings, c: &Colors, cx: &mut Context<Self>) -> gpui::Div {
        let mut themes = div().flex().flex_wrap().gap(px(10.));
        for name in theme::theme_names() {
            let on = s.appearance.theme.eq_ignore_ascii_case(name);
            let p = theme::resolve(name, c.palette.dark);
            let swatch = |v: u32| div().size(px(16.)).rounded(px(3.)).bg(hsla(v));
            themes = themes.child(
                div()
                    .id(SharedString::from(format!("theme-{name}")))
                    .w(px(170.))
                    .p(px(8.))
                    .rounded(px(8.))
                    .border_2()
                    .border_color(if on { c.accent } else { c.border })
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(name.to_string()))
                    .child(
                        div()
                            .flex()
                            .gap(px(3.))
                            .child(swatch(p.background))
                            .child(swatch(p.text))
                            .child(swatch(p.cursor))
                            .child(swatch(p.mark_text))
                            .child(swatch(p.folder))
                            .child(swatch(p.chrome)),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |_this, _, _w, cx| {
                        let n = name.to_string();
                        update_settings(cx, |s| s.appearance.theme = n);
                    })),
            );
        }
        let font = s.appearance.font_size;
        let vfont = s.appearance.viewer_font_size;
        div()
            .child(section("Colour preset", c).child(themes).child(div().text_size(px(12.)).text_color(c.muted).child("\"System\" follows the macOS light/dark appearance.")))
            .child(
                section("Fonts", c)
                    .child(row("File list size", segmented("font", &[(11.0f32, "11"), (12.0, "12"), (13.0, "13"), (14.0, "14"), (16.0, "16")], font, c, cx, |s, v| s.appearance.font_size = v), c))
                    .child(row("Viewer size", segmented("vfont", &[(11.0f32, "11"), (12.0, "12"), (13.0, "13"), (14.0, "14"), (16.0, "16")], vfont, c, cx, |s, v| s.appearance.viewer_font_size = v), c)),
            )
    }

    fn page_advanced(&mut self, s: &Settings, c: &Colors, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .child(
                section("Ask for confirmation before", c)
                    .child(checkbox("a-del", "Moving items to the Trash", s.advanced.confirm_delete, c, cx, |s| s.advanced.confirm_delete = !s.advanced.confirm_delete))
                    .child(checkbox("a-nonempty", "Deleting folders that are not empty", s.advanced.confirm_delete_nonempty, c, cx, |s| s.advanced.confirm_delete_nonempty = !s.advanced.confirm_delete_nonempty))
                    .child(checkbox("a-ext", "Changing a file's extension", s.advanced.confirm_change_extension, c, cx, |s| s.advanced.confirm_change_extension = !s.advanced.confirm_change_extension)),
            )
            .child(
                section("Save on quit", c)
                    .child(checkbox("a-panels", "Panels, tabs and window position", s.advanced.save_panels_state, c, cx, |s| s.advanced.save_panels_state = !s.advanced.save_panels_state))
                    .child(checkbox("a-history", "Folder history", s.advanced.save_folder_history, c, cx, |s| s.advanced.save_folder_history = !s.advanced.save_folder_history))
                    .child(checkbox("a-cmd", "Command-line history", s.advanced.save_command_line, c, cx, |s| s.advanced.save_command_line = !s.advanced.save_command_line)),
            )
            .child(section("Files", c).child(div().text_color(c.muted).child(format!(
                "Settings, shortcuts and the session are stored in {}",
                cx.global::<AppState>().config_dir.display()
            ))))
    }

    fn page_hotkeys(&mut self, c: &Colors, cx: &mut Context<Self>) -> gpui::Div {
        let keymap = cx.global::<AppState>().keymap.clone();
        let mut list = div().flex().flex_col();
        for group in ActionGroup::ALL {
            list = list.child(div().pt(px(10.)).pb(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(c.muted).child(group.title()));
            for action in Action::ALL.iter().copied().filter(|a| a.group() == group) {
                let chords = keymap.chords_for(action);
                let mut line = div()
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .border_b_1()
                    .border_color(c.border.opacity(0.5))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(action.label()));
                for slot in 0..2 {
                    let capturing = self.capture == Some((action, slot));
                    let text = if capturing {
                        "Press keys…".to_string()
                    } else {
                        chords.get(slot).map(|c| c.display()).unwrap_or_else(|| "—".into())
                    };
                    line = line.child(
                        div()
                            .id(SharedString::from(format!("hk-{}-{slot}", action.id())))
                            .w(px(118.))
                            .flex_none()
                            .h(px(22.))
                            .ml(px(6.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if capturing { c.accent } else { c.border })
                            .when(capturing, |d| d.bg(c.accent.opacity(0.15)))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .child(text)
                            .when(action == Action::OpenItem && slot == 1, |d| d.relative().child(crate::probe::probe("prefs.hotkey.cell")))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                                this.capture = Some((action, slot));
                                w.focus(&this.focus);
                                cx.notify();
                            })),
                    );
                }
                list = list.child(line);
            }
        }
        let button = |id: &'static str, label: &'static str, c: &Colors| {
            div().id(id).px(px(12.)).h(px(26.)).flex().items_center().rounded(px(6.)).border_1().border_color(c.border).cursor_pointer().hover(|s| s.bg(c.hover)).child(label)
        };
        div()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .pb(px(8.))
                    .child(button("hk-reset", "Reset to Defaults", c).on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _w, cx| {
                        let state = cx.global_mut::<AppState>();
                        state.keymap = fc_core::Keymap::defaults();
                        state.save_keymap();
                        state::install_key_bindings(cx);
                        state::install_menus(cx);
                        cx.refresh_windows();
                    })))
                    .child(button("hk-import", "Import from Commander One", c).on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _w, cx| {
                        if let Some(json) = fc_core::settings::commander_one_keymap_json()
                            && let Ok((keymap, _)) = fc_core::Keymap::from_commander_one_json(&json) {
                                let state = cx.global_mut::<AppState>();
                                state.keymap = keymap.with_additions();
                                state.save_keymap();
                                state::install_key_bindings(cx);
                                state::install_menus(cx);
                                cx.refresh_windows();
                            }
                    })))
                    .child(div().relative().flex_1().min_w(px(220.)).text_size(px(12.)).text_color(c.muted).child(crate::probe::probe("prefs.hint")).child("Click a shortcut, then press keys. ⌫ clears, Esc cancels.")),
            )
            .child(list)
    }
}

impl Render for PrefsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get(window, cx);
        text_input::set_style(InputStyle { text: c.text, muted: c.muted, bg: c.bg, border: c.border, accent: c.accent, selection: c.accent.opacity(0.28) });
        let s = cx.global::<AppState>().settings.clone();
        let page = self.page;

        let mut sidebar = div().relative().child(crate::probe::probe("prefs.sidebar")).w(px(190.)).flex_none().flex().flex_col().gap(px(2.)).px(px(10.)).pt(px(52.)).pb(px(10.));
        for p in Page::ALL {
            let on = p == page;
            sidebar = sidebar.child(
                div()
                    .id(p.title())
                    .px(px(10.))
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(c.accent.opacity(0.16)).text_color(c.accent))
                    .when(!on, |d| d.hover(|s| s.bg(c.hover)))
                    .child(crate::look::icon(p.icon(), 15., if on { c.accent } else { c.muted }))
                    .child(p.title())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
                        this.page = p;
                        this.capture = None;
                        cx.notify();
                    })),
            );
        }

        let content = match page {
            Page::General => self.page_general(&s, &c, cx),
            Page::View => self.page_view(&s, &c, cx),
            Page::Appearance => self.page_appearance(&s, &c, cx),
            Page::Hotkeys => self.page_hotkeys(&c, cx),
            Page::Advanced => self.page_advanced(&s, &c, cx),
        };

        div()
            .id("prefs")
            .relative()
            .child(crate::probe::probe("prefs.root"))
            .key_context("Preferences")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|_, _: &ui::CloseWindow, w, _| w.remove_window()))
            .size_full()
            .flex()
            .bg(gpui::linear_gradient(180., gpui::linear_color_stop(c.bg_top, 0.), gpui::linear_color_stop(c.bg_bottom, 1.)))
            .text_color(c.text)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .child(sidebar)
            .child(
                div()
                    .id("prefs-content")
                    .relative()
                    .child(crate::probe::probe("prefs.content"))
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .mt(px(12.))
                    .mr(px(12.))
                    .mb(px(12.))
                    .p(px(22.))
                    .rounded(px(14.))
                    .bg(c.bg.opacity(0.94))
                    .border_1()
                    .border_color(c.border)
                    .shadow(crate::look::soft_shadow(&c))
                    .child(div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).pb(px(16.)).child(page.title()))
                    .child(content),
            )
    }
}
