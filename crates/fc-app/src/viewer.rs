//! Internal viewer window (F3): text, hex, image and folder summary.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    div, img, prelude::*, px, size, uniform_list, App, Bounds, Context, Entity, FocusHandle, FontWeight,
    KeyDownEvent, MouseButton, ObjectFit, ScrollStrategy, SharedString, Task, TitlebarOptions,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions,
};

use fc_core::format::{self, SizeMode};
use fc_core::viewer::{self, Document, ViewerMode};

use crate::actions::ui;
use crate::colors::Colors;
use crate::state::AppState;
use crate::text_input::{self, InputStyle, TextInput};

pub struct Viewer {
    path: PathBuf,
    mode: ViewerMode,
    doc: Option<Document>,
    lines: Vec<String>,
    error: Option<String>,
    folder: Arc<Mutex<Option<(u64, u64, u64)>>>,
    focus: FocusHandle,
    find: Entity<TextInput>,
    find_visible: bool,
    find_case: bool,
    hit: Option<(usize, usize)>,
    scroll: UniformListScrollHandle,
    _poll: Option<Task<()>>,
    _appearance: gpui::Subscription,
}

pub fn open(path: PathBuf, cx: &mut App) {
    let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bounds = Bounds::centered(None, size(px(980.), px(720.)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(format!("{title} — Viewer").into()), appears_transparent: true, traffic_light_position: Some(gpui::point(px(16.), px(16.))) }),
            window_min_size: Some(size(px(420.), px(300.))),
            focus: true,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Viewer::new(path, window, cx)),
    );
}

impl Viewer {
    fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Viewer {
        let mode = viewer::detect_mode(&path);
        let focus = cx.focus_handle();
        window.focus(&focus);
        let find = cx.new(|cx| TextInput::new(cx, "", "Find").with_context("ViewerFind"));
        let appearance = cx.observe_window_appearance(window, |_, window, _| window.refresh());
        let mut v = Viewer {
            path,
            mode,
            doc: None,
            lines: Vec::new(),
            error: None,
            folder: Arc::new(Mutex::new(None)),
            focus,
            find,
            find_visible: false,
            find_case: false,
            hit: None,
            scroll: UniformListScrollHandle::new(),
            _poll: None,
            _appearance: appearance,
        };
        v.load(window, cx);
        v
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            ViewerMode::Folder => {
                let path = self.path.clone();
                let out = self.folder.clone();
                std::thread::spawn(move || {
                    let r = fc_core::ops::folder_size(&path, &AtomicBool::new(false));
                    *out.lock().unwrap() = Some(r);
                });
                self._poll = Some(cx.spawn_in(window, async move |this, cx| loop {
                    cx.background_executor().timer(Duration::from_millis(200)).await;
                    let done = this.update(cx, |v, cx| {
                        cx.notify();
                        v.folder.lock().unwrap().is_some()
                    });
                    if !matches!(done, Ok(false)) {
                        break;
                    }
                }));
            }
            ViewerMode::Image => {}
            ViewerMode::Text | ViewerMode::Hex => {
                if self.doc.is_none() {
                    match Document::load(&self.path) {
                        Ok(doc) => self.doc = Some(doc),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                if self.mode == ViewerMode::Text && self.lines.is_empty()
                    && let Some(doc) = &self.doc {
                        self.lines = doc.text_lines();
                    }
            }
        }
    }

    fn set_mode(&mut self, mode: ViewerMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == ViewerMode::Folder || (mode == ViewerMode::Image && !matches!(viewer::detect_mode(&self.path), ViewerMode::Image)) {
            return;
        }
        self.mode = mode;
        self.hit = None;
        self.load(window, cx);
        cx.notify();
    }

    fn find_next(&mut self, cx: &mut Context<Self>) {
        let needle = self.find.read(cx).text().to_string();
        if self.mode != ViewerMode::Text {
            return;
        }
        // With no previous hit, start "after the end" so the search wraps to the top.
        let from = self.hit.unwrap_or((self.lines.len().saturating_sub(1), usize::MAX / 2));
        self.hit = viewer::find_text(&self.lines, &needle, self.find_case, from);
        if let Some((line, _)) = self.hit {
            self.scroll.scroll_to_item(line, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        if m.platform || m.control || m.alt {
            return;
        }
        match ev.keystroke.key.as_str() {
            "t" => self.set_mode(ViewerMode::Text, window, cx),
            "h" => self.set_mode(ViewerMode::Hex, window, cx),
            "i" => self.set_mode(ViewerMode::Image, window, cx),
            "home" => self.scroll.scroll_to_item(0, ScrollStrategy::Top),
            "end" => {
                let n = self.row_count();
                self.scroll.scroll_to_item(n.saturating_sub(1), ScrollStrategy::Top)
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Index of the widest row, so long lines can scroll horizontally.
    fn longest_row(&self) -> Option<usize> {
        match self.mode {
            ViewerMode::Text => self.lines.iter().enumerate().max_by_key(|(_, l)| l.chars().count()).map(|(i, _)| i),
            _ => None,
        }
    }

    fn row_count(&self) -> usize {
        match self.mode {
            ViewerMode::Text => self.lines.len(),
            ViewerMode::Hex => self.doc.as_ref().map(|d| d.hex_line_count()).unwrap_or(0),
            _ => 0,
        }
    }

    fn rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let c = Colors::get(window, cx);
        let font = cx.global::<AppState>().settings.appearance.viewer_font_size;
        let gutter = format!("{}", self.row_count().max(1)).len() as f32 * font * 0.62 + 14.0;
        range
            .map(|ix| {
                let (text, number) = match self.mode {
                    ViewerMode::Hex => (self.doc.as_ref().map(|d| d.hex_line(ix)).unwrap_or_default(), None),
                    _ => (self.lines.get(ix).cloned().unwrap_or_default(), Some(ix + 1)),
                };
                let hit = self.hit.is_some_and(|(l, _)| l == ix);
                div()
                    .flex()
                    .h(px(font + 6.))
                    .whitespace_nowrap()
                    .when(hit, |d| d.bg(c.mark))
                    .when_some(number, |d, n| {
                        d.child(div().w(px(gutter)).flex_none().pr(px(10.)).flex().justify_end().text_color(c.muted).child(n.to_string()))
                    })
                    .child(div().text_color(if hit { c.mark_text } else { c.text }).child(if text.is_empty() { " ".to_string() } else { text }))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get(window, cx);
        text_input::set_style(InputStyle { text: c.text, muted: c.muted, bg: c.bg, border: c.border, accent: c.accent, selection: c.accent.opacity(0.28) });
        let font = cx.global::<AppState>().settings.appearance.viewer_font_size;
        let file_size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let is_image = matches!(viewer::detect_mode(&self.path), ViewerMode::Image);
        let mode = self.mode;

        let mode_button = |label: &'static str, m: ViewerMode, enabled: bool, cx: &mut Context<Self>| {
            div()
                .id(label)
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .when(mode == m, |d| d.bg(c.accent).text_color(c.on_accent))
                .when(mode != m && enabled, |d| d.cursor_pointer().hover(|s| s.bg(c.hover)))
                .when(!enabled, |d| d.text_color(c.muted.opacity(0.5)))
                .child(label)
                .when(enabled, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| this.set_mode(m, w, cx))))
        };

        let content: gpui::AnyElement = if let Some(err) = &self.error {
            div().p(px(20.)).text_color(c.danger).child(format!("Couldn't open the file: {err}")).into_any_element()
        } else {
            match mode {
                ViewerMode::Image => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(12.))
                    .child(img(self.path.clone()).size_full().object_fit(ObjectFit::Contain))
                    .into_any_element(),
                ViewerMode::Folder => {
                    let text = match *self.folder.lock().unwrap() {
                        Some((bytes, files, dirs)) => format!(
                            "Total space occupied:\n{} ({} bytes)\nin {files} file(s), in {dirs} folder(s)",
                            format::format_size(bytes, SizeMode::Dynamic),
                            format::group_digits(bytes)
                        ),
                        None => "Calculating…".to_string(),
                    };
                    div().p(px(24.)).text_size(px(15.)).child(text).into_any_element()
                }
                ViewerMode::Text | ViewerMode::Hex => {
                    let n = self.row_count();
                    uniform_list("viewer-lines", n, cx.processor(|this, range: Range<usize>, window, cx| this.rows(range, window, cx)))
                        .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained)
                        .with_width_from_item(self.longest_row())
                        .track_scroll(self.scroll.clone())
                        .size_full()
                        .px(px(10.))
                        .py(px(6.))
                        .font_family("Menlo")
                        .text_size(px(font))
                        .into_any_element()
                }
            }
        };

        let truncated = self.doc.as_ref().is_some_and(|d| d.truncated);
        div()
            .id("viewer")
            .key_context("Viewer")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_action(cx.listener(|_, _: &ui::CloseViewer, w, _| w.remove_window()))
            .on_action(cx.listener(|_, _: &ui::CloseWindow, w, _| w.remove_window()))
            .on_action(cx.listener(|this, _: &ui::ViewerFind, w, cx| {
                this.find_visible = true;
                w.focus(&this.find.read(cx).focus.clone());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ui::ViewerFindNext, _w, cx| this.find_next(cx)))
            .on_action(cx.listener(|this, _: &ui::Cancel, w, cx| {
                this.find_visible = false;
                w.focus(&this.focus);
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::linear_gradient(180., gpui::linear_color_stop(c.bg_top, 0.), gpui::linear_color_stop(c.bg_bottom, 1.)))
            .text_color(c.text)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(46.))
                    .pl(px(84.))
                    .pr(px(12.))
                    .on_mouse_down(MouseButton::Left, |ev: &gpui::MouseDownEvent, window, _| {
                        if ev.click_count == 2 {
                            window.titlebar_double_click();
                        }
                    })
                    .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().child(self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(c.muted)
                            .child(self.path.parent().map(|p| p.display().to_string()).unwrap_or_default()),
                    )
                    .when(mode != ViewerMode::Folder, |d| {
                        d.child(mode_button("Text", ViewerMode::Text, true, cx))
                            .child(mode_button("Hex", ViewerMode::Hex, true, cx))
                            .child(mode_button("Image", ViewerMode::Image, is_image, cx))
                    }),
            )
            .when(self.find_visible, |d| {
                let case = self.find_case;
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(6.))
                        .child(div().w(px(320.)).child(self.find.clone()))
                        .child(
                            div()
                                .id("find-case")
                                .px(px(8.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .when(case, |d| d.bg(c.accent).text_color(c.on_accent))
                                .child("Aa")
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| {
                                    this.find_case = !this.find_case;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .id("find-next")
                                .px(px(10.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .bg(c.accent)
                                .text_color(c.on_accent)
                                .child("Next")
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| this.find_next(cx))),
                        )
                        .child(div().text_color(c.muted).text_size(px(12.)).child(match self.hit {
                            Some((l, _)) => SharedString::from(format!("Line {}", l + 1)),
                            None => SharedString::from(""),
                        })),
                )
            })
            .child(
                div().flex_1().min_h(px(0.)).px(px(10.)).child(
                    div()
                        .size_full()
                        .rounded(px(12.))
                        .overflow_hidden()
                        .bg(c.bg.opacity(0.94))
                        .border_1()
                        .border_color(c.border)
                        .shadow(crate::look::soft_shadow(&c))
                        .child(content),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(14.))
                    .h(px(30.))
                    .px(px(14.))
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .child(format!("{} ({} bytes)", format::format_size(file_size, SizeMode::Dynamic), format::group_digits(file_size)))
                    .when(mode == ViewerMode::Text, |d| d.child(format!("{} line(s) · UTF-8", self.lines.len())))
                    .when(truncated, |d| d.child(div().text_color(c.danger).child(format!("Showing the first {}", format::format_size(viewer::VIEW_LIMIT, SizeMode::Dynamic)))))
                    .overflow_hidden()
                    .child(div().flex_1().min_w(px(0.)))
                    .child(div().flex_none().overflow_hidden().whitespace_nowrap().child("T text · H hex · I image · ⌘F find · Esc close")),
            )
    }
}
