//! Search window (⌥F7) and Spotlight search (⌃⌥F7).

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, uniform_list, AnyWindowHandle, App, Bounds, Context, Entity, FocusHandle,
    FontWeight, MouseButton, MouseDownEvent, ScrollStrategy, SharedString, Task, TitlebarOptions,
    UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowOptions,
};

use fc_core::search::{self as core_search, ContentQuery, SearchQuery};

use crate::actions::ui;
use crate::colors::Colors;
use crate::text_input::{self, InputStyle, TextInput};
use crate::workspace::Workspace;

const DEPTHS: [(Option<usize>, &str); 5] = [(Some(0), "Current folder"), (Some(1), "1 level"), (Some(2), "2 levels"), (Some(4), "4 levels"), (None, "All levels")];

pub struct SearchView {
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    roots: Vec<PathBuf>,
    spotlight: bool,
    name: Entity<TextInput>,
    text: Entity<TextInput>,
    name_regex: bool,
    case_sensitive: bool,
    text_regex: bool,
    text_case: bool,
    whole_words: bool,
    include_hidden: bool,
    spotlight_content: bool,
    depth: Option<usize>,
    results: Arc<Mutex<Vec<PathBuf>>>,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    status: String,
    shown: usize,
    selected: Option<usize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _poll: Task<()>,
    _appearance: gpui::Subscription,
}

pub fn open(roots: Vec<PathBuf>, spotlight: bool, workspace: WeakEntity<Workspace>, workspace_window: AnyWindowHandle, cx: &mut App) {
    open_with(roots, spotlight, None, workspace, workspace_window, cx);
}

/// Opens the search window, optionally pre-filled with a name query that starts at once.
pub fn open_with(
    roots: Vec<PathBuf>,
    spotlight: bool,
    initial: Option<String>,
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    cx: &mut App,
) {
    let bounds = Bounds::centered(None, size(px(760.), px(640.)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(if spotlight { "Spotlight Search" } else { "Search" }.into()), appears_transparent: true, traffic_light_position: Some(gpui::point(px(16.), px(16.))) }),
            window_min_size: Some(size(px(560.), px(420.))),
            focus: true,
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let mut view = SearchView::new(roots, spotlight, workspace, workspace_window, window, cx);
                if let Some(text) = initial {
                    view.name.update(cx, |i, cx| i.set_text(text, cx));
                    view.start(cx);
                }
                view
            })
        },
    );
}

impl SearchView {
    fn new(
        roots: Vec<PathBuf>,
        spotlight: bool,
        workspace: WeakEntity<Workspace>,
        workspace_window: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SearchView {
        let name = cx.new(|cx| TextInput::new(cx, "", if spotlight { "Spotlight query" } else { "Name or mask, e.g. *.pdf;report" }).with_context("SearchField"));
        let text = cx.new(|cx| TextInput::new(cx, "", "Text inside files (optional)").with_context("SearchField"));
        window.focus(&name.read(cx).focus.clone());
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                break;
            }
        });
        let appearance = cx.observe_window_appearance(window, |_, window, _| window.refresh());
        SearchView {
            workspace,
            workspace_window,
            roots,
            spotlight,
            name,
            text,
            name_regex: false,
            case_sensitive: false,
            text_regex: false,
            text_case: false,
            whole_words: false,
            include_hidden: true,
            spotlight_content: true,
            depth: None,
            results: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            status: String::new(),
            shown: 0,
            selected: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _poll: poll,
            _appearance: appearance,
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let n = self.results.lock().unwrap().len();
        let running = self.running.load(Ordering::Relaxed);
        if n != self.shown || (!running && self.status.starts_with("Searching")) {
            self.shown = n;
            self.status = if running {
                format!("Searching… {n} found")
            } else if self.cancel.load(Ordering::Relaxed) {
                format!("Stopped. {n} found")
            } else if n == 0 {
                "No results".to_string()
            } else {
                format!("{n} found")
            };
            cx.notify();
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.running.load(Ordering::Relaxed) {
            self.cancel.store(true, Ordering::Relaxed);
            return;
        }
        self.results.lock().unwrap().clear();
        self.shown = 0;
        self.selected = None;
        self.cancel = Arc::new(AtomicBool::new(false));
        let results = self.results.clone();
        let running = self.running.clone();
        let cancel = self.cancel.clone();
        let roots = self.roots.clone();

        if self.spotlight {
            let query = self.name.read(cx).text().to_string();
            if query.trim().is_empty() {
                self.status = "Type something to search for".into();
                cx.notify();
                return;
            }
            let in_content = self.spotlight_content;
            running.store(true, Ordering::Relaxed);
            std::thread::spawn(move || {
                if let Ok(found) = core_search::spotlight(&query, &roots, in_content)
                    && !cancel.load(Ordering::Relaxed) {
                        results.lock().unwrap().extend(found);
                    }
                running.store(false, Ordering::Relaxed);
            });
        } else {
            let text = self.text.read(cx).text().to_string();
            let query = SearchQuery {
                roots,
                name: self.name.read(cx).text().to_string(),
                name_regex: self.name_regex,
                case_sensitive: self.case_sensitive,
                depth: self.depth,
                content: (!text.is_empty()).then_some(ContentQuery { text, regex: self.text_regex, case_sensitive: self.text_case, whole_words: self.whole_words }),
                include_hidden: self.include_hidden,
            };
            let compiled = match core_search::compile(query) {
                Ok(c) => c,
                Err(e) => {
                    self.status = e.to_string();
                    cx.notify();
                    return;
                }
            };
            running.store(true, Ordering::Relaxed);
            std::thread::spawn(move || {
                let mut batch = Vec::new();
                let mut last_flush = std::time::Instant::now();
                compiled.run(&cancel, |p| {
                    batch.push(p);
                    if last_flush.elapsed() > Duration::from_millis(100) {
                        results.lock().unwrap().append(&mut batch);
                        last_flush = std::time::Instant::now();
                    }
                });
                results.lock().unwrap().append(&mut batch);
                running.store(false, Ordering::Relaxed);
            });
        }
        self.status = "Searching…".into();
        cx.notify();
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.results.lock().unwrap().get(self.selected?).cloned()
    }

    fn with_workspace(&self, cx: &mut App, f: impl FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static) {
        let ws = self.workspace.clone();
        let _ = self.workspace_window.update(cx, move |_, window, cx| {
            let _ = ws.update(cx, |ws, cx| f(ws, window, cx));
            window.activate_window();
        });
    }

    fn go_to_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path() else { return };
        let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("/"));
        self.with_workspace(cx, move |ws, window, cx| {
            let side = ws.active;
            ws.navigate(side, parent, Some(&path), window, cx);
        });
    }

    fn view_file(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_path() {
            crate::viewer::open(path, cx);
        }
    }

    fn feed(&mut self, cx: &mut Context<Self>) {
        let items = self.results.lock().unwrap().clone();
        if items.is_empty() {
            return;
        }
        let root = self.roots.first().cloned().unwrap_or_else(|| PathBuf::from("/"));
        self.with_workspace(cx, move |ws, window, cx| ws.feed_results(items, root, window, cx));
    }

    fn rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let c = Colors::get(window, cx);
        let results = self.results.lock().unwrap();
        let root = self.roots.first().cloned().unwrap_or_default();
        range
            .filter_map(|ix| {
                let p = results.get(ix)?;
                let rel = p.strip_prefix(&root).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string());
                let selected = self.selected == Some(ix);
                Some(
                    div()
                        .id(("result", ix))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(28.))
                        .px(px(10.))
                        .when(selected, |d| d.bg(c.cursor).text_color(c.cursor_text))
                        .when(!selected && ix % 2 == 1, |d| d.bg(c.alt_bg))
                        .child(crate::look::entry_tile(
                            p,
                            fc_core::entry::read_entry(p).map(|e| crate::look::kind_of(&e)).unwrap_or(crate::look::FileKind::Other),
                            18.,
                            &c,
                            selected,
                            cx,
                        ))
                        .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(rel))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                            this.selected = Some(ix);
                            if ev.click_count >= 2 {
                                this.go_to_file(cx);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn step_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let n = self.results.lock().unwrap().len();
        if n == 0 {
            return;
        }
        let next = match self.selected {
            None => 0,
            Some(i) if down => (i + 1).min(n - 1),
            Some(i) => i.saturating_sub(1),
        };
        self.selected = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Top);
        cx.notify();
    }
}

fn check(id: &'static str, label: &'static str, on: bool, c: &Colors, cx: &mut Context<SearchView>, f: fn(&mut SearchView)) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .cursor_pointer()
        .child(
            div()
                .size(px(14.))
                .rounded(px(3.))
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
        .child(label)
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
            f(this);
            cx.notify();
        }))
}

fn button(id: &'static str, label: impl Into<SharedString>, primary: bool, enabled: bool, c: &Colors, cx: &mut Context<SearchView>, f: fn(&mut SearchView, &mut Context<SearchView>)) -> impl IntoElement {
    div()
        .id(id)
        .px(px(14.))
        .h(px(28.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(if primary { c.accent } else { c.border })
        .bg(if primary { c.accent } else { c.surface })
        .text_color(if primary { c.on_accent } else if enabled { c.text } else { c.muted })
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.opacity(0.88)))
        .child(label.into())
        .when(enabled, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| f(this, cx))))
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get(window, cx);
        text_input::set_style(InputStyle { text: c.text, muted: c.muted, bg: c.bg, border: c.border, accent: c.accent, selection: c.accent.opacity(0.28) });
        let running = self.running.load(Ordering::Relaxed);
        let n = self.results.lock().unwrap().len();
        let has_sel = self.selected.is_some();
        let roots_label = self.roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ");
        let label = |t: &'static str| div().w(px(90.)).flex_none().text_color(c.muted).child(t);

        let mut form = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(16.))
            .mx(px(12.))
            .rounded(px(14.))
            .bg(c.bg.opacity(0.94))
            .border_1()
            .border_color(c.border)
            .shadow(crate::look::soft_shadow(&c));
        form = form.child(div().flex().items_center().child(label("Search in")).child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().child(roots_label)));
        form = form.child(div().flex().items_center().child(label(if self.spotlight { "Query" } else { "Name" })).child(div().flex_1().min_w(px(0.)).child(self.name.clone())));
        if self.spotlight {
            form = form.child(div().flex().gap(px(16.)).pl(px(90.)).child(check("sp-content", "Match file contents too", self.spotlight_content, &c, cx, |s| s.spotlight_content = !s.spotlight_content)));
        } else {
            form = form
                .child(
                    div()
                        .flex()
                        .gap(px(16.))
                        .pl(px(90.))
                        .child(check("name-regex", "RegEx", self.name_regex, &c, cx, |s| s.name_regex = !s.name_regex))
                        .child(check("name-case", "Case sensitive", self.case_sensitive, &c, cx, |s| s.case_sensitive = !s.case_sensitive))
                        .child(check("hidden", "Include hidden", self.include_hidden, &c, cx, |s| s.include_hidden = !s.include_hidden)),
                )
                .child(div().flex().items_center().child(label("Text")).child(div().flex_1().min_w(px(0.)).child(self.text.clone())))
                .child(
                    div()
                        .flex()
                        .gap(px(16.))
                        .pl(px(90.))
                        .child(check("text-regex", "RegEx", self.text_regex, &c, cx, |s| s.text_regex = !s.text_regex))
                        .child(check("text-case", "Case sensitive", self.text_case, &c, cx, |s| s.text_case = !s.text_case))
                        .child(check("words", "Entire words", self.whole_words, &c, cx, |s| s.whole_words = !s.whole_words)),
                );
            let mut depths = div().flex().flex_wrap().gap(px(4.));
            for (i, (depth, text)) in DEPTHS.into_iter().enumerate() {
                let on = self.depth == depth;
                depths = depths.child(
                    div()
                        .id(("depth", i))
                        .px(px(8.))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .border_1()
                        .border_color(if on { c.accent } else { c.border })
                        .when(on, |d| d.bg(c.accent).text_color(c.on_accent))
                        .child(text)
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
                            this.depth = depth;
                            cx.notify();
                        })),
                );
            }
            form = form.child(div().flex().items_center().child(label("Depth")).child(depths));
        }
        form = form.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().min_w(px(0.)).text_color(c.muted).child(self.status.clone()))
                .child(button("start", if running { "Stop" } else { "Search" }, true, true, &c, cx, |this, cx| this.start(cx))),
        );

        div()
            .id("search")
            .relative()
            .child(crate::probe::probe("search.root"))
            .key_context("Search")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ui::Confirm, _w, cx| this.start(cx)))
            .on_action(cx.listener(|_, _: &ui::CloseWindow, w, _| w.remove_window()))
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _w, cx| match ev.keystroke.key.as_str() {
                "down" if !ev.keystroke.modifiers.platform => this.step_selection(true, cx),
                "up" if !ev.keystroke.modifiers.platform => this.step_selection(false, cx),
                _ => {}
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::linear_gradient(180., gpui::linear_color_stop(c.bg_top, 0.), gpui::linear_color_stop(c.bg_bottom, 1.)))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .h(px(46.))
                    .pl(px(84.))
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
                        if ev.click_count == 2 {
                            window.titlebar_double_click();
                        }
                    })
                    .child(crate::look::icon(crate::icons::SEARCH, 15., c.accent))
                    .child(if self.spotlight { "Spotlight Search" } else { "Search" }),
            )
            .text_color(c.text)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .child(form)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .mx(px(12.))
                    .mt(px(10.))
                    .rounded(px(14.))
                    .overflow_hidden()
                    .bg(c.bg.opacity(0.94))
                    .border_1()
                    .border_color(c.border)
                    .child(uniform_list("results", n, cx.processor(|this, range: Range<usize>, window, cx| this.rows(range, window, cx))).track_scroll(self.scroll.clone()).size_full()),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).font_weight(FontWeight::NORMAL).text_color(c.muted).child("Double-click a result to show it in the active panel."))
                    .child(button("view", "View", false, has_sel, &c, cx, |this, cx| this.view_file(cx)))
                    .child(button("goto", "Go to File", false, has_sel, &c, cx, |this, cx| this.go_to_file(cx)))
                    .child(div().relative().child(crate::probe::probe("search.feed")).child(button("feed", "Feed to Panel", false, n > 0, &c, cx, |this, cx| this.feed(cx)))),
            )
    }
}
