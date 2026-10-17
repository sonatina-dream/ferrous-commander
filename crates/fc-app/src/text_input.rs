//! A single-line text field driven by key-down events.
//!
//! Supports typing, cursor movement (with ⌥ word jumps and ⌘ line jumps), selection via
//! ⇧ or ⌘A, and clipboard cut/copy/paste. IME composition is not supported.

use gpui::{
    div, prelude::*, px, App, ClipboardItem, Context, FocusHandle, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, SharedString, Window,
};

/// Single-line editable text.
pub struct TextInput {
    pub focus: FocusHandle,
    text: String,
    /// Cursor as a byte offset (always on a char boundary).
    cursor: usize,
    /// Selection anchor; the selection spans anchor..cursor.
    anchor: Option<usize>,
    placeholder: SharedString,
    key_context: &'static str,
    monospace: bool,
    /// Draw without a box (for fields embedded in larger chrome).
    bare: bool,
    font_size: f32,
    /// Height of a bare field; defaults to the font size plus padding.
    height: Option<f32>,
}

/// Emitted after every edit.
pub struct Changed;

impl gpui::EventEmitter<Changed> for TextInput {}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, text: impl Into<String>, placeholder: &str) -> TextInput {
        let text = text.into();
        TextInput {
            focus: cx.focus_handle(),
            cursor: text.len(),
            anchor: if text.is_empty() { None } else { Some(0) },
            text,
            placeholder: placeholder.to_string().into(),
            key_context: "TextInput",
            monospace: false,
            bare: false,
            font_size: 13.0,
            height: None,
        }
    }

    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    pub fn text_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    pub fn with_context(mut self, ctx: &'static str) -> Self {
        self.key_context = ctx;
        self
    }

    pub fn monospace(mut self) -> Self {
        self.monospace = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.anchor = None;
        cx.emit(Changed);
        cx.notify();
    }

    /// Selects everything (dialogs open with the default value selected).
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
        cx.notify();
    }

    /// Selects the file stem so typing replaces the name but keeps the extension.
    pub fn select_stem(&mut self, cx: &mut Context<Self>) {
        let name_start = self.text.rfind('/').map(|i| i + 1).unwrap_or(0);
        let name = &self.text[name_start..];
        let stem_len = match name.rfind('.') {
            Some(0) | None => name.len(),
            Some(i) => i,
        };
        self.anchor = Some(name_start);
        self.cursor = name_start + stem_len;
        cx.notify();
    }

    /// Inserts at the cursor, replacing the selection.
    pub fn insert(&mut self, s: &str, cx: &mut Context<Self>) {
        self.delete_selection();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        cx.emit(Changed);
        cx.notify();
    }

    fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.cursor {
            None
        } else {
            Some((a.min(self.cursor), a.max(self.cursor)))
        }
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection() {
            self.text.replace_range(s..e, "");
            self.cursor = s;
            self.anchor = None;
            true
        } else {
            self.anchor = None;
            false
        }
    }

    fn prev_boundary(&self, from: usize) -> usize {
        self.text[..from].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_boundary(&self, from: usize) -> usize {
        self.text[from..].chars().next().map(|c| from + c.len_utf8()).unwrap_or(self.text.len())
    }

    fn word_left(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && !self.text[..i].chars().next_back().is_some_and(char::is_alphanumeric) {
            i = self.prev_boundary(i);
        }
        while i > 0 && self.text[..i].chars().next_back().is_some_and(char::is_alphanumeric) {
            i = self.prev_boundary(i);
        }
        i
    }

    fn word_right(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.text.len() && !self.text[i..].chars().next().is_some_and(char::is_alphanumeric) {
            i = self.next_boundary(i);
        }
        while i < self.text.len() && self.text[i..].chars().next().is_some_and(char::is_alphanumeric) {
            i = self.next_boundary(i);
        }
        i
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = pos;
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let shift = m.shift;
        let handled = match ks.key.as_str() {
            "left" => {
                let target = if m.platform {
                    0
                } else if m.alt {
                    self.word_left(self.cursor)
                } else if let (Some((s, _)), false) = (self.selection(), shift) {
                    s
                } else {
                    self.prev_boundary(self.cursor)
                };
                self.move_to(target, shift);
                true
            }
            "right" => {
                let target = if m.platform {
                    self.text.len()
                } else if m.alt {
                    self.word_right(self.cursor)
                } else if let (Some((_, e)), false) = (self.selection(), shift) {
                    e
                } else {
                    self.next_boundary(self.cursor)
                };
                self.move_to(target, shift);
                true
            }
            "home" | "up" => {
                self.move_to(0, shift);
                ks.key == "home"
            }
            "end" | "down" => {
                self.move_to(self.text.len(), shift);
                ks.key == "end"
            }
            "backspace" => {
                if !self.delete_selection() && self.cursor > 0 {
                    let start = if m.platform {
                        0
                    } else if m.alt {
                        self.word_left(self.cursor)
                    } else {
                        self.prev_boundary(self.cursor)
                    };
                    self.text.replace_range(start..self.cursor, "");
                    self.cursor = start;
                }
                cx.emit(Changed);
                true
            }
            "delete" => {
                if !self.delete_selection() && self.cursor < self.text.len() {
                    let end = if m.alt { self.word_right(self.cursor) } else { self.next_boundary(self.cursor) };
                    self.text.replace_range(self.cursor..end, "");
                }
                cx.emit(Changed);
                true
            }
            "a" if m.platform => {
                self.anchor = Some(0);
                self.cursor = self.text.len();
                true
            }
            "c" | "x" if m.platform => {
                if let Some((s, e)) = self.selection() {
                    cx.write_to_clipboard(ClipboardItem::new_string(self.text[s..e].to_string()));
                    if ks.key == "x" {
                        self.delete_selection();
                        cx.emit(Changed);
                    }
                }
                true
            }
            "v" if m.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let line = text.replace(['\n', '\r'], " ");
                    self.insert(&line, cx);
                }
                true
            }
            _ => {
                if m.platform || m.control || m.function {
                    false
                } else if let Some(ch) = ks.key_char.as_ref().filter(|c| !c.chars().any(char::is_control)) {
                    let ch = ch.clone();
                    self.insert(&ch, cx);
                    true
                } else {
                    false
                }
            }
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, _ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.anchor = None;
        self.cursor = self.text.len();
        cx.notify();
    }
}

/// Visual parameters supplied by the owner (keeps the field theme-agnostic).
#[derive(Clone, Copy)]
pub struct InputStyle {
    pub text: Hsla,
    pub muted: Hsla,
    pub bg: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
}

thread_local! {
    static STYLE: std::cell::Cell<Option<InputStyle>> = const { std::cell::Cell::new(None) };
}

/// Sets the style used by inputs rendered during this frame.
pub fn set_style(style: InputStyle) {
    STYLE.with(|s| s.set(Some(style)));
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = STYLE.with(|s| s.get()).unwrap_or(InputStyle {
            text: gpui::black(),
            muted: gpui::black().opacity(0.4),
            bg: gpui::white(),
            border: gpui::black().opacity(0.2),
            accent: gpui::blue(),
            selection: gpui::blue().opacity(0.25),
        });
        let focused = self.focus.is_focused(window);
        let sel = self.selection();
        let cursor = self.cursor;

        let body = if self.text.is_empty() {
            div()
                .flex()
                .items_center()
                .child(if focused { div().w(px(1.5)).h(px((self.font_size + 3.0).round())).bg(style.accent) } else { div() })
                .child(div().text_color(style.muted).child(self.placeholder.clone()))
        } else {
            let (a, b) = sel.unwrap_or((cursor, cursor));
            let before = self.text[..a].to_string();
            let selected = self.text[a..b].to_string();
            let after = self.text[b..].to_string();
            let caret_height = (self.font_size + 3.0).round();
            let caret = || div().w(px(1.5)).h(px(caret_height)).bg(style.accent).flex_none();
            let cursor_at_start = sel.is_some() && cursor == a;
            div()
                .flex()
                .items_center()
                .whitespace_nowrap()
                .child(before)
                .when(focused && (sel.is_none() || cursor_at_start), |d| d.child(caret()))
                .when(!selected.is_empty(), |d| {
                    d.child(div().bg(if focused { style.selection } else { style.muted.opacity(0.2) }).child(selected))
                })
                .when(focused && sel.is_some() && !cursor_at_start, |d| d.child(caret()))
                .child(after)
        };

        let field = div()
            .id(("text-input", cx.entity_id().as_u64() as usize))
            .key_context(self.key_context)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .flex()
            .items_center()
            .text_color(style.text)
            .text_size(px(self.font_size))
            .overflow_hidden()
            .when(self.monospace, |d| d.font_family("Menlo"))
            .cursor_text();
        let field = if self.bare {
            field.h(px(self.height.unwrap_or(self.font_size + 14.))).w_full()
        } else {
            field
                .h(px(32.))
                .px(px(10.))
                .rounded(px(8.))
                .bg(style.bg)
                .border_1()
                .border_color(if focused { style.accent } else { style.border })
                .when(focused, |d| {
                    d.shadow(vec![gpui::BoxShadow {
                        color: style.accent.opacity(0.22),
                        offset: gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(3.),
                    }])
                })
        };
        field
            .child(body)
    }
}

impl gpui::Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}
