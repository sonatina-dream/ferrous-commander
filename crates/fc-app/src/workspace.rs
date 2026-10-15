//! The main window: two panels, command routing, jobs, watching and persistence.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    point, px, size, App, AppContext, Bounds, ClipboardItem, Context, Entity, FocusHandle, KeyDownEvent,
    Pixels, Point, ScrollStrategy, Task, TitlebarOptions, UniformListScrollHandle, Window, WindowBounds,
    WindowOptions,
};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use fc_core::archive::{self, CompressionLevel};
use fc_core::keymap::Action;
use fc_core::ops::{self, TransferKind};
use fc_core::panel::{CloseTabError, OpenOutcome};
use fc_core::pattern::Mask;
use fc_core::queue::{Answer, JobKind, Snapshot};
use fc_core::settings::{PanelSession, Session, ViewerChoice, WindowFrame};
use fc_core::volumes::{self, Volume};
use fc_core::{platform, Entry, ListOptions, LockState, Panel, SortKey, ViewMode};

use crate::dialogs::{Dialog, InfoState};
use crate::menus::{MenuCmd, MenuEntry, PopupMenu};
use crate::state::AppState;
use crate::text_input::TextInput;

/// Transient status message at the bottom of the window.
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: Instant,
}

/// Items dragged between panels.
#[derive(Clone)]
pub struct DraggedFiles {
    pub side: usize,
    pub paths: Vec<PathBuf>,
}

/// Floating label shown while dragging.
pub struct DragPreview {
    pub label: String,
}

impl gpui::Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{div, ParentElement, Styled};
        let c = crate::colors::Colors::get(window, cx);
        div()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(c.accent)
            .text_color(c.on_accent)
            .text_size(px(12.))
            .shadow_md()
            .child(self.label.clone())
    }
}

pub struct Workspace {
    pub focus: FocusHandle,
    pub dialog_focus: FocusHandle,
    pub prompt_focus: FocusHandle,
    pub menu_focus: FocusHandle,
    pub panels: [Panel; 2],
    pub active: usize,
    pub split: f32,
    pub dragging_split: bool,
    pub scroll: [UniformListScrollHandle; 2],
    pub list_bounds: [Rc<Cell<Bounds<Pixels>>>; 2],
    pub window_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub brief_first_col: [usize; 2],
    /// Rows produced by the last Full-mode render of each list (layout sanity checks).
    pub rendered_rows: Cell<[usize; 2]>,
    /// Frames rendered (diagnostics).
    pub frames: Cell<u64>,
    pub command: Entity<TextInput>,
    pub command_history: Vec<String>,
    history_cursor: Option<usize>,
    pub dialog: Option<Dialog>,
    pub menu: Option<PopupMenu>,
    pub type_ahead: String,
    type_ahead_at: Instant,
    pub toast: Option<Toast>,
    pub jobs: Snapshot,
    seen_prompt: bool,
    watcher: Option<RecommendedWatcher>,
    watched: Vec<PathBuf>,
    dirty: Arc<Mutex<HashSet<PathBuf>>>,
    pub volumes: Vec<Volume>,
    volumes_at: Instant,
    session_dirty: bool,
    session_saved_at: Instant,
    size_results: Arc<Mutex<Vec<(PathBuf, u64)>>>,
    size_cancel: Arc<AtomicBool>,
    /// Gliding cursor highlight per panel (row index, animated).
    pub cursor_spring: [crate::anim::Spring; 2],
    pub snap_cursor: [bool; 2],
    pub last_frame: Instant,
    pub last_offsets: [f32; 2],
    /// Bumped on every folder change (row fade-in animations key off it).
    pub nav_gen: [u64; 2],
    pub nav_at: [Instant; 2],
    /// Bumped when the active panel changes (glow animation).
    pub active_gen: u64,
    pub palette: Option<crate::palette::Palette>,
    pub palette_generation: u64,
    /// Bumped whenever a dialog or toast appears (pop-in animations).
    pub dialog_gen: u64,
    pub toast_gen: u64,
    _tick: Task<()>,
    /// Redraws when macOS switches between light and dark.
    _appearance: gpui::Subscription,
}

impl Workspace {
    pub fn new(session: Option<Session>, window: &mut Window, cx: &mut Context<Self>) -> Workspace {
        let home = dirs_home();
        let session = session.unwrap_or_else(|| {
            let mut s = Session::default();
            s.left.tabs.push(fc_core::settings::TabSession { path: home.clone(), mode: ViewMode::Full, sort: Default::default(), lock: LockState::Unlocked });
            s.right = s.left.clone();
            s
        });
        let opts = cx.global::<AppState>().settings.list_options();
        let mut panels = [session.left.restore(&home), session.right.restore(&home)];
        for panel in &mut panels {
            for tab in &mut panel.tabs {
                if tab.reload(opts).is_err() {
                    let _ = tab.navigate(home.clone(), opts);
                }
            }
        }
        let command = cx.new(|cx| TextInput::new(cx, "", "Command").with_context("CommandLine").monospace());
        let focus = cx.focus_handle();
        window.focus(&focus);

        let tick = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            if this.update_in(cx, |this, window, cx| this.tick(window, cx)).is_err() {
                break;
            }
        });

        let appearance = cx.observe_window_appearance(window, |_, window, _| window.refresh());
        let mut ws = Workspace {
            focus,
            dialog_focus: cx.focus_handle(),
            prompt_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            panels,
            active: session.active_side.min(1),
            split: session.split.clamp(0.15, 0.85),
            dragging_split: false,
            scroll: [UniformListScrollHandle::new(), UniformListScrollHandle::new()],
            list_bounds: [Rc::new(Cell::new(Bounds::default())), Rc::new(Cell::new(Bounds::default()))],
            window_bounds: Rc::new(Cell::new(Bounds::default())),
            brief_first_col: [0, 0],
            rendered_rows: Cell::new([0, 0]),
            frames: Cell::new(0),
            command,
            command_history: session.command_history.clone(),
            history_cursor: None,
            dialog: None,
            menu: None,
            type_ahead: String::new(),
            type_ahead_at: Instant::now(),
            toast: None,
            jobs: Snapshot::default(),
            seen_prompt: false,
            watcher: None,
            watched: Vec::new(),
            dirty: Arc::new(Mutex::new(HashSet::new())),
            volumes: visible_volumes(),
            volumes_at: Instant::now(),
            session_dirty: false,
            session_saved_at: Instant::now(),
            size_results: Arc::new(Mutex::new(Vec::new())),
            size_cancel: Arc::new(AtomicBool::new(false)),
            cursor_spring: Default::default(),
            snap_cursor: [true, true],
            last_frame: Instant::now(),
            last_offsets: [0.0, 0.0],
            nav_gen: [0, 0],
            nav_at: [Instant::now(), Instant::now()],
            active_gen: 0,
            palette: None,
            palette_generation: 0,
            dialog_gen: 0,
            toast_gen: 0,
            _tick: tick,
            _appearance: appearance,
        };
        let dirty = ws.dirty.clone();
        ws.watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let mut d = dirty.lock().unwrap();
                for p in event.paths {
                    if let Some(parent) = p.parent() {
                        d.insert(parent.to_path_buf());
                    }
                    d.insert(p);
                }
            }
        })
        .ok();
        ws.sync_watches();
        ws.update_title(window);
        ws.window_bounds.set(window.bounds());
        let weak = cx.entity().downgrade();
        cx.global_mut::<crate::state::Registry>().workspaces.push(weak);
        ws
    }

    // ----- accessors -------------------------------------------------------------------

    pub fn opts(cx: &App) -> ListOptions {
        cx.global::<AppState>().settings.list_options()
    }

    pub fn tab(&self) -> &fc_core::Tab {
        self.panels[self.active].tab()
    }

    fn tab_mut(&mut self) -> &mut fc_core::Tab {
        self.panels[self.active].tab_mut()
    }

    fn other(&self) -> usize {
        1 - self.active
    }

    pub fn row_height(cx: &App) -> f32 {
        cx.global::<AppState>().settings.appearance.font_size.round() + 9.0
    }

    fn page_rows(&self, side: usize, cx: &App) -> usize {
        let h: f32 = self.list_bounds[side].get().size.height.into();
        ((h / Self::row_height(cx)).floor() as usize).max(1)
    }

    /// Columns in thumbs mode for `side`.
    pub fn thumb_columns(&self, side: usize) -> usize {
        let w: f32 = self.list_bounds[side].get().size.width.into();
        ((w / crate::render::THUMB_CELL).floor() as usize).max(1)
    }

    fn update_title(&self, window: &mut Window) {
        let path = self.tab().working_dir().display().to_string();
        window.set_window_title(&format!("{} — {path}", self.tab().title()));
    }

    pub fn notify_toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast_gen += 1;
        self.toast = Some(Toast { text: text.into(), error, until: Instant::now() + Duration::from_secs(if error { 6 } else { 3 }) });
    }

    fn report<E: std::fmt::Display>(&mut self, result: Result<(), E>) {
        if let Err(e) = result {
            self.notify_toast(e.to_string(), true);
        }
    }

    // ----- periodic work ---------------------------------------------------------------

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut changed = false;
        let snapshot = cx.global::<AppState>().jobs.snapshot();
        if snapshot.generation != self.jobs.generation {
            let finished = snapshot.finished != self.jobs.finished;
            let had_prompt = self.jobs.prompt.is_some();
            self.jobs = snapshot;
            if finished {
                self.reload_all(cx);
                self.report_failures();
            }
            if self.jobs.prompt.is_some() && !had_prompt {
                self.seen_prompt = true;
                window.focus(&self.prompt_focus);
            } else if self.jobs.prompt.is_none() && self.seen_prompt {
                self.seen_prompt = false;
                self.restore_dialog_or_panel_focus(window, cx);
            }
            changed = true;
        }

        let dirty: Vec<PathBuf> = std::mem::take(&mut *self.dirty.lock().unwrap()).into_iter().collect();
        if !dirty.is_empty() {
            let opts = Self::opts(cx);
            for side in 0..2 {
                let dir = self.panels[side].tab().working_dir().to_path_buf();
                if dirty.iter().any(|p| p == &dir) {
                    let _ = self.panels[side].tab_mut().reload(opts);
                    changed = true;
                }
            }
        }

        let sizes: Vec<(PathBuf, u64)> = std::mem::take(&mut *self.size_results.lock().unwrap());
        if !sizes.is_empty() {
            let opts = Self::opts(cx);
            for (path, bytes) in sizes {
                for panel in &mut self.panels {
                    let tab = panel.tab_mut();
                    if tab.entries.iter().any(|e| e.path == path) {
                        tab.set_dir_size(path.clone(), bytes, opts);
                    }
                }
            }
            changed = true;
        }

        if !self.type_ahead.is_empty() && self.type_ahead_at.elapsed() > Duration::from_millis(1200) {
            self.type_ahead.clear();
            changed = true;
        }
        if self.toast.as_ref().is_some_and(|t| Instant::now() > t.until) {
            self.toast = None;
            changed = true;
        }
        if self.volumes_at.elapsed() > Duration::from_secs(4) {
            let vols = visible_volumes();
            if vols.iter().map(|v| &v.path).ne(self.volumes.iter().map(|v| &v.path)) {
                changed = true;
            }
            self.volumes = vols;
            self.volumes_at = Instant::now();
        }
        if self.session_dirty && self.session_saved_at.elapsed() > Duration::from_secs(2) {
            self.save_session(window, cx);
        }
        self.window_bounds.set(window.bounds());
        if changed {
            cx.notify();
        }
    }

    fn report_failures(&mut self) {
        let failures: Vec<String> = self
            .jobs
            .jobs
            .iter()
            .filter_map(|j| match &j.state {
                fc_core::queue::JobState::Failed(msg) => Some(format!("{}: {msg}", j.title)),
                _ => None,
            })
            .collect();
        if let Some(last) = failures.last() {
            self.notify_toast(last.clone(), true);
        }
    }

    fn sync_watches(&mut self) {
        let wanted: Vec<PathBuf> = {
            let mut v: Vec<PathBuf> = self.panels.iter().map(|p| p.tab().working_dir().to_path_buf()).collect();
            v.dedup();
            v
        };
        if wanted == self.watched {
            return;
        }
        if let Some(w) = self.watcher.as_mut() {
            for old in &self.watched {
                let _ = w.unwatch(old);
            }
            for new in &wanted {
                let _ = w.watch(new, RecursiveMode::NonRecursive);
            }
        }
        self.watched = wanted;
    }

    pub fn capture_session(&self, cx: &App) -> Session {
        let s = &cx.global::<AppState>().settings.advanced;
        let b = self.window_bounds.get();
        Session {
            left: PanelSession::capture(&self.panels[0], s.save_folder_history),
            right: PanelSession::capture(&self.panels[1], s.save_folder_history),
            active_side: self.active,
            split: self.split,
            command_history: if s.save_command_line { self.command_history.iter().rev().take(50).rev().cloned().collect() } else { Vec::new() },
            window: (b.size.width > px(100.)).then(|| WindowFrame {
                x: b.origin.x.into(),
                y: b.origin.y.into(),
                width: b.size.width.into(),
                height: b.size.height.into(),
            }),
        }
    }

    pub fn save_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_bounds.set(window.bounds());
        self.persist_session(cx);
    }

    /// Writes the session using the last known window frame.
    pub fn persist_session(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<AppState>();
        if state.settings.advanced.save_panels_state {
            let session = self.capture_session(cx);
            let _ = session.save_to(&cx.global::<AppState>().config_dir);
        }
        self.session_dirty = false;
        self.session_saved_at = Instant::now();
    }

    fn mark_session(&mut self) {
        self.session_dirty = true;
    }

    // ----- refresh & navigation --------------------------------------------------------

    pub fn reload_all(&mut self, cx: &mut Context<Self>) {
        let opts = Self::opts(cx);
        for panel in &mut self.panels {
            let _ = panel.tab_mut().reload(opts);
        }
        cx.notify();
    }

    pub fn reveal_cursor(&mut self, side: usize) {
        let tab = self.panels[side].tab();
        match tab.view_mode {
            ViewMode::Full => self.scroll[side].scroll_to_item(tab.cursor, ScrollStrategy::Top),
            ViewMode::Thumbs => {
                let cols = self.thumb_columns(side);
                self.scroll[side].scroll_to_item(tab.cursor / cols, ScrollStrategy::Top)
            }
            ViewMode::Brief => {}
        }
    }

    /// Navigates `side` (respecting tab locks); errors become toasts.
    pub fn navigate(&mut self, side: usize, dir: PathBuf, focus: Option<&Path>, window: &mut Window, cx: &mut Context<Self>) {
        let opts = Self::opts(cx);
        let dir = ops::normalize(&dir);
        match self.panels[side].navigate_focus(dir.clone(), opts, focus) {
            Ok(()) => {
                self.after_navigation(side, window);
            }
            Err(e) => self.notify_toast(format!("Can't open \"{}\": {e}", dir.display()), true),
        }
        cx.notify();
    }

    fn after_navigation(&mut self, side: usize, window: &mut Window) {
        self.nav_gen[side] += 1;
        self.nav_at[side] = Instant::now();
        self.snap_cursor[side] = true;
        self.brief_first_col[side] = 0;
        self.type_ahead.clear();
        self.size_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.reveal_cursor(side);
        self.sync_watches();
        self.update_title(window);
        self.mark_session();
    }

    fn go(&mut self, dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        match dir {
            Some(d) if d.is_dir() => self.navigate(self.active, d, None, window, cx),
            Some(d) => self.notify_toast(format!("\"{}\" doesn't exist", d.display()), true),
            None => {}
        }
    }

    pub fn restore_focus(&mut self, window: &mut Window) {
        window.focus(&self.focus);
    }

    /// "Feed to Panel": shows search results in the active tab.
    pub fn feed_results(&mut self, items: Vec<PathBuf>, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let opts = Self::opts(cx);
        let side = self.active;
        self.panels[side].tab_mut().show_results("Search Results".into(), root, items, opts);
        self.after_navigation(side, window);
        window.focus(&self.focus);
        cx.notify();
    }

    // ----- input handlers --------------------------------------------------------------

    pub fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let side = self.active;
        self.type_ahead.clear();
        self.panels[side].tab_mut().move_cursor(delta, extend);
        self.reveal_cursor(side);
        cx.notify();
    }

    pub fn on_ui_action(&mut self, action: UiNav, window: &mut Window, cx: &mut Context<Self>) {
        let side = self.active;
        let mode = self.tab().view_mode;
        let page = self.page_rows(side, cx) as isize;
        let cols = self.thumb_columns(side) as isize;
        let (vertical, horizontal) = match mode {
            ViewMode::Full => (1, 0),
            ViewMode::Brief => (1, page),
            ViewMode::Thumbs => (cols, 1),
        };
        match action {
            UiNav::Up(extend) => self.move_cursor(-vertical, extend, cx),
            UiNav::Down(extend) => self.move_cursor(vertical, extend, cx),
            UiNav::Left => {
                if horizontal > 0 {
                    self.move_cursor(-horizontal, false, cx)
                }
            }
            UiNav::Right => {
                if horizontal > 0 {
                    self.move_cursor(horizontal, false, cx)
                }
            }
            UiNav::PageUp(extend) => self.move_cursor(-(page - 1).max(1) * if mode == ViewMode::Thumbs { cols } else { 1 }, extend, cx),
            UiNav::PageDown(extend) => self.move_cursor((page - 1).max(1) * if mode == ViewMode::Thumbs { cols } else { 1 }, extend, cx),
            UiNav::Home(extend) => {
                self.panels[side].tab_mut().cursor_to_start(extend);
                self.reveal_cursor(side);
                cx.notify();
            }
            UiNav::End(extend) => {
                self.panels[side].tab_mut().cursor_to_end(extend);
                self.reveal_cursor(side);
                cx.notify();
            }
            UiNav::GoUp => self.run(Action::GoTop, window, cx),
            UiNav::ClearTypeAhead => {
                if self.type_ahead.is_empty() {
                    self.panels[side].tab_mut().unselect_all();
                }
                self.type_ahead.clear();
                cx.notify();
            }
        }
    }

    /// Type-ahead find on printable keys that no binding consumed.
    pub fn on_panel_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        if ks.modifiers.platform || ks.modifiers.control || ks.modifiers.function {
            return;
        }
        let Some(ch) = ks.key_char.as_ref().filter(|c| !c.is_empty() && !c.chars().any(char::is_control)) else { return };
        if self.type_ahead_at.elapsed() > Duration::from_millis(1200) {
            self.type_ahead.clear();
        }
        self.type_ahead.push_str(ch);
        self.type_ahead_at = Instant::now();
        let side = self.active;
        let tab = self.panels[side].tab_mut();
        if !tab.type_ahead(&self.type_ahead) {
            let single = ch.clone();
            if tab.type_ahead(&single) {
                self.type_ahead = single;
            }
        }
        self.reveal_cursor(side);
        cx.stop_propagation();
        cx.notify();
    }

    pub fn activate_side(&mut self, side: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.active != side {
            self.active = side;
            self.active_gen += 1;
            self.type_ahead.clear();
            self.update_title(window);
            self.mark_session();
        }
        self.menu = None;
        if self.dialog.is_none() && self.jobs.prompt.is_none() {
            window.focus(&self.focus);
        }
        cx.notify();
    }

    pub fn row_mouse_down(&mut self, side: usize, ix: usize, ev: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_side(side, window, cx);
        let tab = self.panels[side].tab_mut();
        if ev.modifiers.platform {
            tab.toggle_index(ix);
            tab.set_cursor(ix);
        } else if ev.modifiers.shift {
            let from = tab.cursor;
            tab.select_range(from, ix);
            tab.set_cursor(ix);
        } else {
            tab.set_cursor(ix);
            if ev.click_count >= 2 {
                self.run(Action::OpenItem, window, cx);
            }
        }
        cx.notify();
    }

    pub fn row_right_click(&mut self, side: usize, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_side(side, window, cx);
        let tab = self.panels[side].tab_mut();
        let clicked_selected = tab.entries.get(ix).is_some_and(|e| tab.selected.contains(&e.path));
        if !clicked_selected {
            tab.unselect_all();
            tab.set_cursor(ix);
        }
        let menu = self.file_context_menu(position, cx);
        self.open_menu(menu, window, cx);
    }

    pub fn drop_files(&mut self, side: usize, files: &DraggedFiles, window: &mut Window, cx: &mut Context<Self>) {
        if files.side == side {
            return;
        }
        let dest = self.panels[side].tab().working_dir().to_path_buf();
        let kind = if window.modifiers().platform { TransferKind::Move } else { TransferKind::Copy };
        self.start_transfer(kind, files.paths.clone(), dest, cx);
    }

    pub fn drop_external(&mut self, side: usize, paths: &[PathBuf], _window: &mut Window, cx: &mut Context<Self>) {
        let dest = self.panels[side].tab().working_dir().to_path_buf();
        self.start_transfer(TransferKind::Copy, paths.to_vec(), dest, cx);
    }

    fn start_transfer(&mut self, kind: TransferKind, sources: Vec<PathBuf>, dest: PathBuf, cx: &mut Context<Self>) {
        if sources.is_empty() {
            return;
        }
        let base_dir = sources[0].parent().map(Path::to_path_buf).unwrap_or_else(|| dest.clone());
        let jobs = cx.global::<AppState>().jobs.clone();
        jobs.start(JobKind::Transfer { kind, sources, dest, base_dir });
        cx.notify();
    }

    /// Sets the left panel's share of the width while the splitter is dragged.
    pub fn split_drag(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.split = fraction.clamp(0.15, 0.85);
        self.mark_session();
        cx.notify();
    }

    // ----- command line ----------------------------------------------------------------

    pub fn execute_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.command.read(cx).text().trim().to_string();
        self.history_cursor = None;
        if text.is_empty() {
            self.restore_focus(window);
            return;
        }
        if self.command_history.last() != Some(&text) {
            self.command_history.push(text.clone());
        }
        self.command.update(cx, |c, cx| c.set_text("", cx));
        let cwd = self.tab().working_dir().to_path_buf();
        if text == "cd" || text.starts_with("cd ") {
            let target = platform::resolve_cd_target(&cwd, text.strip_prefix("cd").unwrap_or(""));
            self.navigate(self.active, target, None, window, cx);
            self.restore_focus(window);
        } else {
            let result = platform::run_in_terminal(&cwd, &text);
            self.report(result);
        }
        self.mark_session();
        cx.notify();
    }

    pub fn command_history_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.command_history.is_empty() {
            return;
        }
        let n = self.command_history.len();
        let next = match (self.history_cursor, back) {
            (None, true) => Some(n - 1),
            (None, false) => None,
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(_), false) => None,
        };
        self.history_cursor = next;
        let text = next.map(|i| self.command_history[i].clone()).unwrap_or_default();
        self.command.update(cx, |c, cx| c.set_text(text, cx));
    }

    fn paste_to_command_line(&mut self, full_path: bool, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.tab().targets();
        let text: Vec<String> = targets
            .iter()
            .map(|e| {
                let s = if full_path { e.path.to_string_lossy().into_owned() } else { e.name.clone() };
                if s.contains([' ', '\'', '"', '(', ')', '&', ';']) { platform::shell_quote(&s) } else { s }
            })
            .collect();
        if text.is_empty() {
            return;
        }
        let mut joined = text.join(" ");
        joined.push(' ');
        self.command.update(cx, |c, cx| c.insert(&joined, cx));
        let _ = window;
    }

    // ----- menus -----------------------------------------------------------------------

    pub fn open_menu(&mut self, menu: PopupMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = Some(menu);
        window.focus(&self.menu_focus);
        cx.notify();
    }

    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.restore_focus(window);
            cx.notify();
        }
    }

    pub fn menu_step(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(menu) = self.menu.as_mut() {
            menu.step(down);
            cx.notify();
        }
    }

    pub fn menu_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cmd = self.menu.as_ref().and_then(|m| m.selected_cmd());
        if let Some(cmd) = cmd {
            self.run_menu_cmd(cmd, window, cx);
        }
    }

    pub fn run_menu_cmd(&mut self, cmd: MenuCmd, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.restore_focus(window);
        let opts = Self::opts(cx);
        match cmd {
            MenuCmd::Run(action) => self.run(action, window, cx),
            MenuCmd::Navigate(side, path) => {
                self.active = side;
                self.navigate(side, path, None, window, cx);
            }
            MenuCmd::SelectVolume(side, path) => {
                self.active = side;
                let go_root = cx.global::<AppState>().settings.general.go_root_on_drive_change;
                let target = if go_root || path != Path::new("/") { path } else { dirs_home() };
                self.navigate(side, target, None, window, cx);
            }
            MenuCmd::CloseTab(side, ix) => self.close_tab(side, ix, window, cx),
            MenuCmd::CloseOtherTabs(side) => {
                self.panels[side].close_other_tabs();
                self.after_navigation(side, window);
            }
            MenuCmd::DuplicateTab(side, ix) => {
                self.panels[side].select_tab(ix);
                self.panels[side].clone_tab();
                self.after_navigation(side, window);
            }
            MenuCmd::SetLock(side, ix, lock) => {
                if let Some(t) = self.panels[side].tabs.get_mut(ix) {
                    t.lock = lock;
                }
                self.mark_session();
            }
            MenuCmd::AddFavorite(path) => {
                let state = cx.global_mut::<AppState>();
                if !state.settings.favorites.contains(&path) {
                    state.settings.favorites.push(path);
                    state.save_settings();
                }
            }
            MenuCmd::RemoveFavorite(path) => {
                let state = cx.global_mut::<AppState>();
                state.settings.favorites.retain(|p| p != &path);
                state.save_settings();
            }
            MenuCmd::EjectVolume(path) => self.eject(path, window, cx),
            MenuCmd::ToggleColumn(key) => {
                let state = cx.global_mut::<AppState>();
                let cols = &mut state.settings.view_columns;
                if let Some(i) = cols.iter().position(|k| *k == key) {
                    if key != SortKey::Name {
                        cols.remove(i);
                    }
                } else {
                    cols.push(key);
                    cols.sort_by_key(|k| SortKey::ALL.iter().position(|a| a == k));
                }
                state.save_settings();
                cx.refresh_windows();
            }
            MenuCmd::SortBy(side, key) => {
                let tab = self.panels[side].tab_mut();
                let spec = tab.sort.toggled(key);
                tab.set_sort(spec, opts);
                self.mark_session();
            }
        }
        cx.notify();
    }

    fn shortcut(cx: &App, action: Action) -> Option<String> {
        cx.global::<AppState>().keymap.chords_for(action).first().map(|c| c.display())
    }

    fn item(cx: &App, label: &str, action: Action, enabled: bool) -> MenuEntry {
        MenuEntry::Item { label: label.to_string(), shortcut: Self::shortcut(cx, action), cmd: MenuCmd::Run(action), enabled, checked: false }
    }

    /// ⌘V / ⌥⌘V: pastes files from the clipboard into the active folder. Files cut with
    /// ⌘X (or pasted with ⌥⌘V) move; others copy. Copies into their own folder duplicate.
    fn paste(&mut self, force_move: bool, cx: &mut Context<Self>) {
        let dest = self.tab().working_dir().to_path_buf();
        let state = cx.global::<AppState>();
        let files: Vec<PathBuf> = state.pasteboard.read_files().into_iter().filter(|p| p.exists()).collect();
        if files.is_empty() {
            self.notify_toast("The clipboard has no files to paste.", true);
            return;
        }
        let from_cut = state
            .cut
            .as_ref()
            .is_some_and(|(paths, count)| *count == state.pasteboard.change_count() && fc_core::pasteboard::same_files(paths, &files));
        let moving = force_move || from_cut;
        let jobs = state.jobs.clone();
        let n = files.len();
        let (here, elsewhere): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|p| p.parent() == Some(dest.as_path()));
        if moving {
            if elsewhere.is_empty() {
                self.notify_toast("Those items are already in this folder.", false);
            } else {
                jobs.start(JobKind::Transfer { kind: TransferKind::Move, sources: elsewhere, dest: dest.clone(), base_dir: dest });
                self.notify_toast(format!("Moving {} here", if n == 1 { "1 item".to_string() } else { format!("{n} items") }), false);
            }
            if from_cut {
                cx.global_mut::<AppState>().cut = None;
            }
        } else {
            if !here.is_empty() {
                jobs.start(JobKind::Duplicate { items: here });
            }
            if !elsewhere.is_empty() {
                jobs.start(JobKind::Transfer { kind: TransferKind::Copy, sources: elsewhere, dest: dest.clone(), base_dir: dest });
            }
            self.notify_toast(format!("Pasting {}", if n == 1 { "1 item".to_string() } else { format!("{n} items") }), false);
        }
        cx.notify();
    }

    fn clipboard_has_files(cx: &App) -> bool {
        !cx.global::<AppState>().pasteboard.read_files().is_empty()
    }

    /// Right-click on empty space in a panel.
    pub fn background_menu(&self, position: Point<Pixels>, cx: &App) -> PopupMenu {
        let can_paste = Self::clipboard_has_files(cx);
        PopupMenu::new(
            position,
            vec![
                Self::item(cx, "Paste", Action::PasteFiles, can_paste),
                Self::item(cx, "Move Items Here", Action::MoveHere, can_paste),
                MenuEntry::Separator,
                Self::item(cx, "New Folder", Action::NewFolder, true),
                Self::item(cx, "New File", Action::NewFile, true),
                MenuEntry::Separator,
                Self::item(cx, "Select All", Action::SelectAll, true),
                Self::item(cx, "Open in Terminal", Action::ShowTerminal, true),
                Self::item(cx, "Reload", Action::ReloadContent, true),
            ],
        )
    }

    fn file_context_menu(&self, position: Point<Pixels>, cx: &App) -> PopupMenu {
        let targets = self.tab().targets();
        let one = targets.len() == 1;
        let any = !targets.is_empty();
        let archive = one && archive::detect_format(&targets[0].path).is_some();
        let names = if one { format!("\"{}\"", targets[0].name) } else { format!("{} items", targets.len()) };
        let items = vec![
            Self::item(cx, "Open", Action::OpenItem, any),
            Self::item(cx, "View", Action::View, one),
            Self::item(cx, "Edit", Action::Edit, one && !targets[0].is_dir_like()),
            Self::item(cx, "Quick Look", Action::QuickLook, any),
            MenuEntry::Separator,
            Self::item(cx, "Copy", Action::CopyFiles, any),
            Self::item(cx, "Cut", Action::CutFiles, any),
            Self::item(cx, "Paste", Action::PasteFiles, Self::clipboard_has_files(cx)),
            MenuEntry::Separator,
            Self::item(cx, &format!("Copy {names} to Other Panel"), Action::Copy, any),
            Self::item(cx, "Rename / Move…", Action::Move, any),
            Self::item(cx, "Duplicate", Action::Duplicate, any),
            Self::item(cx, "Make Alias", Action::MakeAlias, any),
            Self::item(cx, "Make Symlink", Action::MakeLink, any),
            Self::item(cx, "New Folder with Selection", Action::NewFolder, true),
            MenuEntry::Separator,
            Self::item(cx, "Compress", Action::Compress, any),
            Self::item(cx, "Extract", Action::Uncompress, archive),
            MenuEntry::Separator,
            Self::item(cx, if one { "Copy Name" } else { "Copy Names" }, Action::CopyItemName, any),
            Self::item(cx, if one { "Copy Path" } else { "Copy Paths" }, Action::CopyItemPath, any),
            MenuEntry::Separator,
            Self::item(cx, "Get Info", Action::ShowInfo, one),
            Self::item(cx, "Show in Finder", Action::ShowInFinder, any),
            Self::item(cx, "Calculate Folder Size", Action::FolderSize, any),
            MenuEntry::Separator,
            Self::item(cx, "Move to Trash", Action::Delete, any),
            Self::item(cx, "Delete Permanently", Action::DeleteForce, any),
        ];
        PopupMenu::new(position, items)
    }

    pub fn drive_menu(&self, side: usize, position: Point<Pixels>) -> PopupMenu {
        let mut items = vec![MenuEntry::Header("Volumes".into())];
        let mode = fc_core::format::SizeMode::Dynamic;
        for v in &self.volumes {
            items.push(MenuEntry::Item {
                label: format!("{}   —   {} free", v.name, fc_core::format::format_size(v.free, mode)),
                shortcut: None,
                cmd: MenuCmd::SelectVolume(side, v.path.clone()),
                enabled: true,
                checked: volumes::volume_for(self.panels[side].tab().working_dir(), &self.volumes).is_some_and(|cur| cur.path == v.path),
            });
        }
        items.push(MenuEntry::Separator);
        items.push(MenuEntry::Header("Places".into()));
        for (label, path) in places() {
            items.push(MenuEntry::Item { label: label.into(), shortcut: None, cmd: MenuCmd::Navigate(side, path), enabled: true, checked: false });
        }
        let ejectable: Vec<&Volume> = self.volumes.iter().filter(|v| v.ejectable).collect();
        if !ejectable.is_empty() {
            items.push(MenuEntry::Separator);
            for v in ejectable {
                items.push(MenuEntry::Item { label: format!("Eject \"{}\"", v.name), shortcut: None, cmd: MenuCmd::EjectVolume(v.path.clone()), enabled: true, checked: false });
            }
        }
        PopupMenu::new(position, items)
    }

    pub fn favorites_menu(&self, side: usize, position: Point<Pixels>, cx: &App) -> PopupMenu {
        let favorites = cx.global::<AppState>().settings.favorites.clone();
        let current = self.panels[side].tab().working_dir().to_path_buf();
        let mut items = vec![MenuEntry::Header("Favorites".into())];
        if favorites.is_empty() {
            items.push(MenuEntry::Item { label: "No favorites yet".into(), shortcut: None, cmd: MenuCmd::Run(Action::ReloadContent), enabled: false, checked: false });
        }
        for f in &favorites {
            items.push(MenuEntry::Item { label: f.display().to_string(), shortcut: None, cmd: MenuCmd::Navigate(side, f.clone()), enabled: f.is_dir(), checked: f == &current });
        }
        items.push(MenuEntry::Separator);
        if favorites.contains(&current) {
            items.push(MenuEntry::Item { label: "Remove Current Folder from Favorites".into(), shortcut: None, cmd: MenuCmd::RemoveFavorite(current), enabled: true, checked: false });
        } else {
            items.push(MenuEntry::Item { label: "Add Current Folder to Favorites".into(), shortcut: None, cmd: MenuCmd::AddFavorite(current), enabled: true, checked: false });
        }
        PopupMenu::new(position, items)
    }

    pub fn history_menu(&self, side: usize, position: Point<Pixels>) -> PopupMenu {
        let mut items = vec![MenuEntry::Header("Recent Folders".into())];
        let history = &self.panels[side].folder_history;
        if history.is_empty() {
            items.push(MenuEntry::Item { label: "No history yet".into(), shortcut: None, cmd: MenuCmd::Run(Action::ReloadContent), enabled: false, checked: false });
        }
        for p in history.iter().take(25) {
            items.push(MenuEntry::Item { label: p.display().to_string(), shortcut: None, cmd: MenuCmd::Navigate(side, p.clone()), enabled: p.is_dir(), checked: false });
        }
        PopupMenu::new(position, items)
    }

    pub fn tab_menu(&self, side: usize, ix: usize, position: Point<Pixels>) -> PopupMenu {
        let lock = self.panels[side].tabs.get(ix).map(|t| t.lock).unwrap_or_default();
        let lock_item = |label: &str, state: LockState| MenuEntry::Item {
            label: label.into(),
            shortcut: None,
            cmd: MenuCmd::SetLock(side, ix, state),
            enabled: true,
            checked: lock == state,
        };
        PopupMenu::new(
            position,
            vec![
                MenuEntry::Item { label: "Duplicate Tab".into(), shortcut: None, cmd: MenuCmd::DuplicateTab(side, ix), enabled: true, checked: false },
                MenuEntry::Item { label: "Close Tab".into(), shortcut: None, cmd: MenuCmd::CloseTab(side, ix), enabled: self.panels[side].tabs.len() > 1, checked: false },
                MenuEntry::Item { label: "Close Other Tabs".into(), shortcut: None, cmd: MenuCmd::CloseOtherTabs(side), enabled: self.panels[side].tabs.len() > 1, checked: false },
                MenuEntry::Separator,
                lock_item("Unlocked", LockState::Unlocked),
                lock_item("Lock This Tab", LockState::Locked),
                lock_item("Lock, Allow Folder Changes", LockState::LockedDirChangesAllowed),
            ],
        )
    }

    pub fn header_menu(&self, position: Point<Pixels>, cx: &App) -> PopupMenu {
        let cols = cx.global::<AppState>().settings.view_columns.clone();
        let mut items = vec![MenuEntry::Header("Columns".into())];
        for key in SortKey::ALL {
            items.push(MenuEntry::Item {
                label: key.header().into(),
                shortcut: None,
                cmd: MenuCmd::ToggleColumn(key),
                enabled: key != SortKey::Name,
                checked: cols.contains(&key),
            });
        }
        PopupMenu::new(position, items)
    }

    // ----- tabs ------------------------------------------------------------------------

    pub fn close_tab(&mut self, side: usize, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.panels[side].close_tab(ix, false) {
            Ok(()) => self.after_navigation(side, window),
            Err(CloseTabError::LastTab) => {}
            Err(CloseTabError::Locked) => self.open_dialog(Dialog::CloseLockedTab { side, ix }, window, cx),
        }
        cx.notify();
    }

    pub fn select_tab(&mut self, side: usize, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = side;
        self.panels[side].select_tab(ix);
        self.after_navigation(side, window);
        window.focus(&self.focus);
        cx.notify();
    }

    // ----- dialogs ---------------------------------------------------------------------

    pub fn open_dialog(&mut self, dialog: Dialog, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.palette = None;
        if self.dialog.is_none() {
            self.dialog_gen += 1;
        }
        let input = dialog.input();
        self.dialog = Some(dialog);
        match input {
            Some(input) => window.focus(&input.read(cx).focus.clone()),
            None => window.focus(&self.dialog_focus),
        }
        cx.notify();
    }

    pub fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog = None;
        self.restore_focus(window);
        cx.notify();
    }

    fn input(&self, text: &str, placeholder: &str, cx: &mut Context<Self>) -> Entity<TextInput> {
        let text = text.to_string();
        let placeholder = placeholder.to_string();
        cx.new(|cx| TextInput::new(cx, text, &placeholder))
    }

    /// Answers the pending job prompt.
    pub fn answer_prompt(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) {
        cx.global::<AppState>().jobs.answer(answer);
        self.jobs = cx.global::<AppState>().jobs.snapshot();
        if self.jobs.prompt.is_none() {
            self.seen_prompt = false;
            self.restore_dialog_or_panel_focus(window, cx);
        } else {
            window.focus(&self.prompt_focus);
        }
        cx.notify();
    }

    /// After a prompt closes, focus the open dialog (if any) or the panels.
    fn restore_dialog_or_panel_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.dialog.as_ref().map(|d| d.input()) {
            Some(Some(input)) => window.focus(&input.read(cx).focus.clone()),
            Some(None) => window.focus(&self.dialog_focus),
            None => self.restore_focus(window),
        }
    }

    fn eject(&mut self, mount: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        for side in 0..2 {
            if self.panels[side].tab().working_dir().starts_with(&mount) {
                self.navigate(side, dirs_home(), None, window, cx);
            }
        }
        let result = volumes::eject(&mount);
        match result {
            Ok(()) => self.notify_toast(format!("Ejected \"{}\"", mount.display()), false),
            Err(e) => self.notify_toast(format!("Couldn't eject: {e}"), true),
        }
        self.volumes = visible_volumes();
    }

    fn compute_sizes(&mut self, paths: Vec<PathBuf>) {
        self.size_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.size_cancel.clone();
        let out = self.size_results.clone();
        std::thread::spawn(move || {
            for p in paths {
                let (bytes, _, _) = ops::folder_size(&p, &cancel);
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                out.lock().unwrap().push((p, bytes));
            }
        });
    }

    fn targets_or_toast(&mut self) -> Option<Vec<Entry>> {
        let t = self.tab().targets();
        if t.is_empty() {
            self.notify_toast("Nothing is selected. Select the files or folders to act on.", true);
            None
        } else {
            Some(t)
        }
    }

    // ----- the command dispatcher ------------------------------------------------------

    /// Executes a command from the key map, menus, buttons or context menus.
    pub fn run(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        let opts = Self::opts(cx);
        let side = self.active;
        let files_only = cx.global::<AppState>().settings.general.select_files_only;
        self.menu = None;

        // Pattern keys type into an active type-ahead instead of opening dialogs.
        if matches!(action, A::Select | A::Unselect | A::InvertSelect | A::SelectItem)
            && !self.type_ahead.is_empty()
            && self.type_ahead_at.elapsed() < Duration::from_millis(1200)
        {
            cx.propagate();
            return;
        }

        match action {
            A::OpenItem | A::EnterItem => {
                let before = self.tab().working_dir().to_path_buf();
                let outcome = if action == A::OpenItem {
                    self.panels[side].open_cursor(opts)
                } else {
                    self.panels[side].enter_cursor(opts)
                };
                match outcome {
                    Ok(OpenOutcome::Navigated) => {
                        if self.tab().working_dir() != before {
                            self.after_navigation(side, window);
                        }
                    }
                    Ok(OpenOutcome::OpenFile(path)) => {
                        let r = platform::open_default(&path);
                        self.report(r);
                    }
                    Ok(OpenOutcome::Nothing) => {}
                    Err(e) => self.notify_toast(format!("Can't open: {e}"), true),
                }
            }
            A::View => {
                let Some(entry) = self.tab().cursor_entry().cloned().filter(|e| !e.is_parent()) else { return };
                let choice = cx.global::<AppState>().settings.general.viewer;
                let result = match choice {
                    ViewerChoice::Internal => {
                        crate::viewer::open(entry.path.clone(), cx);
                        Ok(())
                    }
                    ViewerChoice::QuickLook => platform::quick_look(std::slice::from_ref(&entry.path)),
                    ViewerChoice::Preview => platform::open_in_preview(&entry.path),
                };
                self.report(result);
            }
            A::Edit => {
                let Some(entry) = self.tab().cursor_entry().cloned().filter(|e| !e.is_parent() && !e.is_dir_like()) else { return };
                let editor = cx.global::<AppState>().settings.general.editor.clone();
                let r = platform::edit(&entry.path, editor.as_deref());
                self.report(r);
            }
            A::Copy | A::Move | A::Queue => {
                let Some(targets) = self.targets_or_toast() else { return };
                let kind = if action == A::Move { TransferKind::Move } else { TransferKind::Copy };
                let dest = self.panels[self.other()].tab().working_dir().to_path_buf();
                let mut dest_text = dest.to_string_lossy().into_owned();
                if !dest_text.ends_with('/') {
                    dest_text.push('/');
                }
                let input = self.input(&dest_text, "Destination", cx);
                let sources = targets.into_iter().map(|e| e.path).collect();
                let base_dir = self.tab().working_dir().to_path_buf();
                self.open_dialog(Dialog::Transfer { kind, sources, base_dir, input, enqueue: action == A::Queue }, window, cx);
            }
            A::CopyFiles | A::CutFiles => {
                let Some(targets) = self.targets_or_toast() else { return };
                let paths: Vec<PathBuf> = targets.into_iter().map(|e| e.path).collect();
                let n = paths.len();
                let what = if n == 1 { format!("\"{}\"", paths[0].file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()) } else { format!("{n} items") };
                let state = cx.global_mut::<AppState>();
                if !state.pasteboard.write_files(&paths) {
                    self.notify_toast("Couldn't put the files on the clipboard.", true);
                    return;
                }
                if action == A::CutFiles {
                    let count = state.pasteboard.change_count();
                    state.cut = Some((paths, count));
                    self.notify_toast(format!("Cut {what} — press ⌘V in another folder to move"), false);
                } else {
                    state.cut = None;
                    self.notify_toast(format!("Copied {what} — press ⌘V to paste, here or in Finder"), false);
                }
            }
            A::PasteFiles | A::MoveHere => self.paste(action == A::MoveHere, cx),
            A::Rename => {
                let Some(entry) = self.tab().cursor_entry().cloned().filter(|e| !e.is_parent()) else { return };
                let input = self.input(&entry.name, "New name", cx);
                input.update(cx, |i, cx| i.select_stem(cx));
                self.open_dialog(Dialog::Rename { path: entry.path, input }, window, cx);
            }
            A::NewFolder => {
                let selection: Vec<PathBuf> = if self.tab().selected.is_empty() { Vec::new() } else { self.tab().target_paths() };
                let name = if selection.is_empty() { "New Folder" } else { "New Folder With Items" };
                let input = self.input(name, "Folder name", cx);
                input.update(cx, |i, cx| i.select_all(cx));
                let dir = self.tab().working_dir().to_path_buf();
                self.open_dialog(Dialog::NewFolder { dir, input, with: selection }, window, cx);
            }
            A::NewFile => {
                let input = self.input("untitled.txt", "File name", cx);
                input.update(cx, |i, cx| i.select_stem(cx));
                let dir = self.tab().working_dir().to_path_buf();
                self.open_dialog(Dialog::NewFile { dir, input }, window, cx);
            }
            A::Delete | A::DeleteForce => {
                let Some(targets) = self.targets_or_toast() else { return };
                let s = &cx.global::<AppState>().settings;
                let permanent = action == A::DeleteForce || !s.general.move_to_trash;
                let paths: Vec<PathBuf> = targets.iter().map(|e| e.path.clone()).collect();
                let nonempty = targets.iter().filter(|e| e.groups_as_folder() && std::fs::read_dir(&e.path).map(|mut d| d.next().is_some()).unwrap_or(false)).count();
                if s.advanced.confirm_delete || permanent || (nonempty > 0 && s.advanced.confirm_delete_nonempty) {
                    self.open_dialog(Dialog::Delete { paths, permanent, nonempty_dirs: nonempty }, window, cx);
                } else {
                    cx.global::<AppState>().jobs.start(JobKind::Delete { paths, permanent });
                }
            }
            A::Duplicate => {
                let Some(targets) = self.targets_or_toast() else { return };
                cx.global::<AppState>().jobs.start(JobKind::Duplicate { items: targets.into_iter().map(|e| e.path).collect() });
            }
            A::MakeLink => {
                let Some(targets) = self.targets_or_toast() else { return };
                let dest = self.panels[self.other()].tab().working_dir().to_path_buf();
                let paths: Vec<PathBuf> = targets.into_iter().map(|e| e.path).collect();
                match ops::make_symlinks(&paths, &dest) {
                    Ok(links) => self.notify_toast(format!("Created {} symlink(s) in \"{}\"", links.len(), dest.display()), false),
                    Err(e) => self.notify_toast(e.to_string(), true),
                }
                self.reload_all(cx);
            }
            A::MakeAlias => {
                let Some(targets) = self.targets_or_toast() else { return };
                let dest = self.panels[self.other()].tab().working_dir().to_path_buf();
                let paths: Vec<PathBuf> = targets.into_iter().map(|e| e.path).collect();
                let r = platform::make_aliases(&paths, &dest);
                self.report(r);
                self.reload_all(cx);
            }
            A::Compress => {
                let Some(targets) = self.targets_or_toast() else { return };
                let paths: Vec<PathBuf> = targets.into_iter().map(|e| e.path).collect();
                let name = archive::default_archive_name(&paths, self.tab().working_dir());
                let out = self.panels[self.other()].tab().working_dir().join(&name);
                let out = if out.exists() { ops::unique_name(&out) } else { out };
                cx.global::<AppState>().jobs.start(JobKind::Compress { items: paths, out, level: CompressionLevel::Normal });
            }
            A::CompressWithOptions => {
                let Some(targets) = self.targets_or_toast() else { return };
                let items: Vec<PathBuf> = targets.into_iter().map(|e| e.path).collect();
                let name = archive::default_archive_name(&items, self.tab().working_dir());
                let path = self.panels[self.other()].tab().working_dir().join(name);
                let input = self.input(&path.to_string_lossy(), "Archive path", cx);
                input.update(cx, |i, cx| i.select_stem(cx));
                self.open_dialog(Dialog::Compress { items, input, level: CompressionLevel::Normal }, window, cx);
            }
            A::Uncompress => {
                let Some(entry) = self.tab().cursor_entry().cloned().filter(|e| !e.is_parent()) else { return };
                if archive::detect_format(&entry.path).is_none() {
                    self.notify_toast("Extract works with ZIP, TAR and TAR.GZ archives.", true);
                    return;
                }
                let dest_dir = self.panels[self.other()].tab().working_dir().to_path_buf();
                cx.global::<AppState>().jobs.start(JobKind::Extract { archive: entry.path, dest_dir });
            }
            A::ShowInfo => {
                let Some(entry) = self.tab().cursor_entry().cloned().filter(|e| !e.is_parent()) else { return };
                let state = InfoState::new(entry);
                if state.entry.is_dir_like() {
                    let path = state.entry.path.clone();
                    let results = state.size.clone();
                    std::thread::spawn(move || {
                        let r = ops::folder_size(&path, &AtomicBool::new(false));
                        *results.lock().unwrap() = Some(r);
                    });
                }
                self.open_dialog(Dialog::Info(state), window, cx);
            }
            A::ShowFinderInfo => {
                let Some(entry) = self.tab().cursor_entry().cloned().filter(|e| !e.is_parent()) else { return };
                let r = platform::finder_info(&entry.path);
                self.report(r);
            }
            A::ShowInFinder => {
                let paths = self.tab().target_paths();
                let paths = if paths.is_empty() { vec![self.tab().working_dir().to_path_buf()] } else { paths };
                let r = platform::reveal_in_finder(&paths);
                self.report(r);
            }
            A::QuickLook => {
                let Some(targets) = self.targets_or_toast() else { return };
                let r = platform::quick_look(&targets.into_iter().map(|e| e.path).collect::<Vec<_>>());
                self.report(r);
            }
            A::FolderSize => {
                let dirs: Vec<PathBuf> = self.tab().targets().into_iter().filter(|e| e.is_dir_like()).map(|e| e.path).collect();
                let dirs = if dirs.is_empty() {
                    self.tab().entries.iter().filter(|e| e.groups_as_folder() && !e.is_parent()).map(|e| e.path.clone()).collect()
                } else {
                    dirs
                };
                self.compute_sizes(dirs);
            }
            A::SelectItemWithSize => {
                if let Some(e) = self.tab().cursor_entry().cloned()
                    && e.is_dir_like() && !e.is_parent() && cx.global::<AppState>().settings.general.folder_size_on_space {
                        self.compute_sizes(vec![e.path]);
                    }
                self.tab_mut().toggle_cursor_and_advance();
                self.reveal_cursor(side);
            }
            A::SelectItem => {
                self.tab_mut().toggle_cursor_and_advance();
                self.reveal_cursor(side);
            }
            A::Select | A::Unselect => {
                let input = self.input("*", "Mask, e.g. *.jpg;*.png", cx);
                input.update(cx, |i, cx| i.select_all(cx));
                self.open_dialog(Dialog::Pattern { select: action == A::Select, input }, window, cx);
            }
            A::InvertSelect => self.tab_mut().invert_selection(files_only),
            A::SelectAll => self.tab_mut().select_all(files_only),
            A::UnselectAll => self.tab_mut().unselect_all(),
            A::GoBack | A::GoForward => {
                let r = if action == A::GoBack { self.tab_mut().go_back(opts) } else { self.tab_mut().go_forward(opts) };
                match r {
                    Ok(true) => self.after_navigation(side, window),
                    Ok(false) => {}
                    Err(e) => self.notify_toast(format!("Can't open: {e}"), true),
                }
            }
            A::GoTop => match self.panels[side].go_up(opts) {
                Ok(true) => self.after_navigation(side, window),
                Ok(false) => {}
                Err(e) => self.notify_toast(format!("Can't open: {e}"), true),
            },
            A::GoHome => self.go(Some(dirs_home()), window, cx),
            A::GoDesktop => self.go(Some(dirs_home().join("Desktop")), window, cx),
            A::GoDocuments => self.go(Some(dirs_home().join("Documents")), window, cx),
            A::GoDownloads => self.go(Some(dirs_home().join("Downloads")), window, cx),
            A::GoApplications => self.go(Some(PathBuf::from("/Applications")), window, cx),
            A::GoSystemApplications => self.go(Some(PathBuf::from("/System/Applications")), window, cx),
            A::GoUtilities => {
                let p = PathBuf::from("/System/Applications/Utilities");
                self.go(Some(if p.is_dir() { p } else { PathBuf::from("/Applications/Utilities") }), window, cx)
            }
            A::GoFolder => {
                let input = self.input(&self.tab().working_dir().to_string_lossy(), "Folder path", cx);
                input.update(cx, |i, cx| i.select_all(cx));
                self.open_dialog(Dialog::GoTo { input }, window, cx);
            }
            A::GoServer | A::ShowFTP => {
                let input = self.input("smb://", "Server address, e.g. smb://192.168.0.3", cx);
                self.open_dialog(Dialog::Server { input }, window, cx);
            }
            A::ReloadContent => {
                let r = self.tab_mut().reload(opts);
                self.report(r);
                self.volumes = visible_volumes();
            }
            A::ChangeDriveLeft | A::ChangeDriveRight => {
                let s = if action == A::ChangeDriveLeft { 0 } else { 1 };
                self.active = s;
                let b = self.window_bounds.get();
                let x = if s == 0 { px(12.) } else { b.size.width * self.split + px(12.) };
                let menu = self.drive_menu(s, point(x, px(84.)));
                self.open_menu(menu, window, cx);
                return;
            }
            A::ShowFavorites => {
                let menu = self.favorites_menu(side, point(px(40.), px(120.)), cx);
                self.open_menu(menu, window, cx);
                return;
            }
            A::Eject => {
                let cur = volumes::volume_for(self.tab().working_dir(), &self.volumes);
                match cur {
                    Some(v) if v.ejectable => self.eject(v.path, window, cx),
                    _ => self.notify_toast("The current volume can't be ejected.", true),
                }
            }
            A::SwitchPanel => {
                self.active = self.other();
                self.active_gen += 1;
                self.type_ahead.clear();
                self.update_title(window);
            }
            A::NewTab => {
                let dir = self.tab().working_dir().to_path_buf();
                let r = self.panels[side].new_tab(dir, opts);
                self.report(r);
                self.after_navigation(side, window);
            }
            A::CloneTab => {
                self.panels[side].clone_tab();
                self.after_navigation(side, window);
            }
            A::CloseTab => {
                let ix = self.panels[side].active;
                self.close_tab(side, ix, window, cx);
            }
            A::NextTab => {
                self.panels[side].next_tab();
                self.after_navigation(side, window);
            }
            A::PrevTab => {
                self.panels[side].prev_tab();
                self.after_navigation(side, window);
            }
            A::ToggleTabLock => {
                let tab = self.tab_mut();
                tab.lock = tab.lock.next();
                let label = match tab.lock {
                    LockState::Unlocked => "Tab unlocked",
                    LockState::Locked => "Tab locked",
                    LockState::LockedDirChangesAllowed => "Tab locked (folder changes allowed)",
                };
                self.notify_toast(label, false);
                self.mark_session();
            }
            A::SwapPanels => {
                self.panels.swap(0, 1);
                self.active = self.other();
                self.scroll.swap(0, 1);
                self.mark_session();
            }
            A::EqualizePanels => {
                self.split = 0.5;
                self.mark_session();
            }
            A::NewWindow => {
                let session = self.capture_session(cx);
                open_window(Some(Session { window: None, ..session }), cx);
            }
            A::ViewModeFull | A::ViewModeBrief | A::ViewModeThumbs => {
                self.tab_mut().view_mode = match action {
                    A::ViewModeFull => ViewMode::Full,
                    A::ViewModeBrief => ViewMode::Brief,
                    _ => ViewMode::Thumbs,
                };
                self.brief_first_col[side] = 0;
                self.reveal_cursor(side);
                self.mark_session();
            }
            A::SortByName | A::SortByExtension | A::SortBySize | A::SortByDate | A::SortByCreated | A::SortByAdded
            | A::SortByAccessed | A::SortByKind | A::SortByPermissions => {
                let key = SortKey::from_id(action.id()).expect("sort action ids are sort keys");
                let spec = self.tab().sort.toggled(key);
                self.tab_mut().set_sort(spec, opts);
                self.reveal_cursor(side);
                self.mark_session();
            }
            A::SortAscending | A::SortDescending => {
                let mut spec = self.tab().sort;
                spec.ascending = action == A::SortAscending;
                self.tab_mut().set_sort(spec, opts);
                self.mark_session();
            }
            A::ToggleHidden | A::ToggleButtons | A::ToggleCommandLine | A::ToggleDiskList | A::ToggleDiskInfo
            | A::TogglePathBar | A::ToggleSelectionInfo => {
                let state = cx.global_mut::<AppState>();
                let v = &mut state.settings.view;
                let flag = match action {
                    A::ToggleHidden => &mut v.show_hidden,
                    A::ToggleButtons => &mut v.show_buttons,
                    A::ToggleCommandLine => &mut v.show_command_line,
                    A::ToggleDiskList => &mut v.show_sidebar,
                    A::ToggleDiskInfo => &mut v.show_disk_info,
                    A::TogglePathBar => &mut v.show_path_bar,
                    _ => &mut v.show_selection_info,
                };
                *flag = !*flag;
                state.save_settings();
                if action == A::ToggleHidden {
                    let hidden = cx.global::<AppState>().settings.view.show_hidden;
                    self.notify_toast(if hidden { "Showing hidden files" } else { "Hiding hidden files" }, false);
                    self.reload_all(cx);
                }
                cx.refresh_windows();
            }
            A::FullScreen => window.toggle_fullscreen(),
            A::ShowQueue => self.open_dialog(Dialog::Queue, window, cx),
            A::Search | A::SearchSpotlight => {
                let roots = {
                    let dirs: Vec<PathBuf> = self.tab().selected.iter().filter(|p| p.is_dir()).cloned().collect();
                    if dirs.is_empty() { vec![self.tab().working_dir().to_path_buf()] } else { dirs }
                };
                crate::search::open(roots, action == A::SearchSpotlight, cx.entity().downgrade(), window.window_handle(), cx);
            }
            A::ShowTerminal => {
                let r = platform::open_terminal(self.tab().working_dir());
                self.report(r);
            }
            A::CopyItemName | A::CopyItemPath => {
                let Some(targets) = self.targets_or_toast() else { return };
                let text: Vec<String> = targets
                    .iter()
                    .map(|e| if action == A::CopyItemName { e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { e.path.to_string_lossy().into_owned() })
                    .collect();
                cx.write_to_clipboard(ClipboardItem::new_string(text.join("\n")));
                self.notify_toast(format!("Copied {} to the clipboard", if text.len() == 1 { "1 item".to_string() } else { format!("{} items", text.len()) }), false);
            }
            A::PasteItemName | A::PasteItemPath => self.paste_to_command_line(action == A::PasteItemPath, window, cx),
            A::Preferences => crate::prefs::open(crate::prefs::Page::General, cx),
        }
        cx.notify();
    }

    /// Handles a dialog's primary button (Enter).
    pub fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else { return };
        let opts = Self::opts(cx);
        let jobs = cx.global::<AppState>().jobs.clone();
        let mut reopen: Option<Dialog> = None;
        match dialog {
            Dialog::Transfer { kind, sources, base_dir, input, enqueue } => {
                let dest = PathBuf::from(input.read(cx).text().trim());
                match ops::plan_transfer(kind, &sources, &dest, &base_dir) {
                    Ok(_) => {
                        let job = JobKind::Transfer { kind, sources, dest, base_dir };
                        if enqueue {
                            jobs.enqueue(job);
                            self.notify_toast("Added to the operations queue", false);
                        } else {
                            jobs.start(job);
                        }
                        self.tab_mut().unselect_all();
                    }
                    Err(e) => {
                        self.notify_toast(e.to_string(), true);
                        reopen = Some(Dialog::Transfer { kind, sources, base_dir, input, enqueue });
                    }
                }
            }
            Dialog::Rename { path, input } => {
                let new_name = input.read(cx).text().trim().to_string();
                let old_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if new_name.contains('/') {
                    // A path: treat as a move.
                    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                    jobs.start(JobKind::Transfer { kind: TransferKind::Move, sources: vec![path], dest: PathBuf::from(new_name), base_dir });
                } else if cx.global::<AppState>().settings.advanced.confirm_change_extension
                    && path.is_file()
                    && ops::changes_extension(&old_name, &new_name)
                {
                    reopen = Some(Dialog::ConfirmRenameExt { path, new_name });
                } else {
                    self.finish_rename(&path, &new_name, cx);
                }
            }
            Dialog::ConfirmRenameExt { path, new_name } => self.finish_rename(&path, &new_name, cx),
            Dialog::NewFolder { dir, input, with } => {
                let name = input.read(cx).text().to_string();
                let result = if with.is_empty() { ops::create_folder(&dir, &name) } else { ops::create_folder_with(&dir, &name, &with) };
                match result {
                    Ok(_) => {
                        let _ = self.tab_mut().reload(opts);
                        // Focus the first component of the created path ("a" for "a/b/c").
                        let first = PathBuf::from(name.trim()).components().next().map(|c| c.as_os_str().to_owned()).unwrap_or_default();
                        let top = dir.join(first);
                        if let Some(ix) = self.tab().index_of(&top) {
                            self.tab_mut().set_cursor(ix);
                        }
                        self.reveal_cursor(self.active);
                    }
                    Err(e) => {
                        self.notify_toast(e.to_string(), true);
                        reopen = Some(Dialog::NewFolder { dir, input, with });
                    }
                }
            }
            Dialog::NewFile { dir, input } => {
                let name = input.read(cx).text().to_string();
                match ops::create_file(&dir, &name) {
                    Ok(path) => {
                        let _ = self.tab_mut().reload(opts);
                        if let Some(ix) = self.tab().index_of(&path) {
                            self.tab_mut().set_cursor(ix);
                        }
                        self.reveal_cursor(self.active);
                        let editor = cx.global::<AppState>().settings.general.editor.clone();
                        let r = platform::edit(&path, editor.as_deref());
                        self.report(r);
                    }
                    Err(e) => {
                        self.notify_toast(e.to_string(), true);
                        reopen = Some(Dialog::NewFile { dir, input });
                    }
                }
            }
            Dialog::Delete { paths, permanent, .. } => {
                jobs.start(JobKind::Delete { paths, permanent });
                self.tab_mut().unselect_all();
            }
            Dialog::Pattern { select, input } => {
                let mask = Mask::parse(input.read(cx).text());
                let files_only = cx.global::<AppState>().settings.general.select_files_only;
                let n = if select { self.tab_mut().select_mask(&mask, files_only) } else { self.tab_mut().unselect_mask(&mask, files_only) };
                self.notify_toast(format!("{} {n} item(s)", if select { "Selected" } else { "Unselected" }), false);
            }
            Dialog::GoTo { input } => {
                let text = input.read(cx).text().trim().to_string();
                let target = platform::resolve_cd_target(self.tab().working_dir(), &text);
                if target.is_dir() {
                    self.navigate(self.active, target, None, window, cx);
                } else if target.exists() {
                    let parent = target.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
                    self.navigate(self.active, parent, Some(&target), window, cx);
                } else {
                    self.notify_toast(format!("\"{}\" doesn't exist", target.display()), true);
                    reopen = Some(Dialog::GoTo { input });
                }
            }
            Dialog::Server { input } => {
                let url = input.read(cx).text().trim().to_string();
                let r = platform::open_url(&url);
                self.report(r);
            }
            Dialog::Compress { items, input, level } => {
                let out = PathBuf::from(input.read(cx).text().trim());
                if out.exists() {
                    self.notify_toast(format!("\"{}\" already exists", out.display()), true);
                    reopen = Some(Dialog::Compress { items, input, level });
                } else {
                    jobs.start(JobKind::Compress { items, out, level });
                }
            }
            Dialog::CloseLockedTab { side, ix } => {
                let _ = self.panels[side].close_tab(ix, true);
                self.after_navigation(side, window);
            }
            Dialog::QuitWithJobs => {
                self.save_session(window, cx);
                cx.quit();
            }
            Dialog::Info(state) => reopen = Some(Dialog::Info(state)),
            Dialog::Queue | Dialog::Message { .. } => {}
        }
        if let Some(d) = reopen {
            self.open_dialog(d, window, cx);
        } else {
            self.close_dialog(window, cx);
        }
    }

    fn finish_rename(&mut self, path: &Path, new_name: &str, cx: &mut Context<Self>) {
        match ops::rename_in_place(path, new_name) {
            Ok(new_path) => {
                let _ = self.tab_mut().reload(Self::opts(cx));
                if let Some(ix) = self.tab().index_of(&new_path) {
                    self.tab_mut().set_cursor(ix);
                }
                self.reveal_cursor(self.active);
            }
            Err(e) => self.notify_toast(e.to_string(), true),
        }
    }

    /// Window-level quit: confirm when jobs are running.
    pub fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx.global::<AppState>().jobs.has_active() {
            self.open_dialog(Dialog::QuitWithJobs, window, cx);
        } else {
            self.save_session(window, cx);
            cx.quit();
        }
    }
}

/// Fixed list-navigation commands.
#[derive(Clone, Copy)]
pub enum UiNav {
    Up(bool),
    Down(bool),
    Left,
    Right,
    PageUp(bool),
    PageDown(bool),
    Home(bool),
    End(bool),
    GoUp,
    ClearTypeAhead,
}

/// Mounted volumes; the demo recorder shows only the startup volume.
pub fn visible_volumes() -> Vec<Volume> {
    let all = volumes::list_volumes();
    if std::env::var_os("FC_DEMO").is_some() { all.into_iter().filter(|v| v.is_root).collect() } else { all }
}

pub fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

pub fn places() -> Vec<(&'static str, PathBuf)> {
    let home = dirs_home();
    vec![
        ("Home", home.clone()),
        ("Desktop", home.join("Desktop")),
        ("Documents", home.join("Documents")),
        ("Downloads", home.join("Downloads")),
        ("Applications", PathBuf::from("/Applications")),
        ("Computer root ( / )", PathBuf::from("/")),
    ]
}

/// Opens a workspace window (restoring `session` when given).
pub fn open_window(session: Option<Session>, cx: &mut App) {
    let frame = session.as_ref().and_then(|s| s.window);
    let bounds = match frame {
        Some(f) if f.width >= 600.0 && f.height >= 400.0 => {
            Bounds::new(point(px(f.x), px(f.y)), size(px(f.width), px(f.height)))
        }
        _ => Bounds::centered(None, size(px(1240.), px(780.)), cx),
    };
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(fc_core::settings::APP_NAME.into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(18.), px(18.))),
            }),
            window_min_size: Some(size(px(880.), px(520.))),
            focus: true,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| Workspace::new(session, window, cx)),
    );
    if let Err(e) = result {
        eprintln!("could not open window: {e}");
    }
    cx.activate(true);
}
