//! Disk Map window (`SPEC-DiskMap.md`): a sunburst of what fills a disk or folder, a
//! sidebar listing the pointed folder, and a collector for deleting what is not needed.
//! It is a separate window; the file panels are not affected by it.

use std::cell::Cell;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    anchored, canvas, deferred, div, AnimationExt, point, prelude::*, px, relative, size, uniform_list, AnyElement, AnyWindowHandle, App,
    Bounds, ClickEvent, Context, Div, ExternalPaths, FocusHandle, FontWeight, Global, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, PathBuilder, PathPromptOptions, Pixels, Point, ScrollStrategy, SharedString, Stateful,
    Subscription, Task, TitlebarOptions, UniformListScrollHandle, WeakEntity, Window, WindowBounds, WindowHandle,
    WindowOptions,
};

use fc_core::diskmap::{self as dm, History, Node, NodeId, NodeKind, Row, ScanProgress, Segment, SegmentKind, Special, Tree, ROOT};
use fc_core::format::{format_size, format_time_friendly, group_digits, SizeMode};
use fc_core::{ops, platform, volumes};

use crate::actions::ui;
use crate::colors::Colors;
use crate::icons;
use crate::look::{self, FileKind};
use crate::workspace::{DragPreview, DraggedFiles, Workspace};

/// How long the map takes to bloom open after navigating (§5.5).
const BLOOM: Duration = Duration::from_millis(320);
/// Seconds between Delete and the actual deletion (§8).
const COUNTDOWN_SECS: u64 = 5;
const SIDEBAR_WIDTH: f32 = 300.;
const ROW_HEIGHT: f32 = 30.;
/// Gap between rings, in points.
const RING_GAP: f32 = 1.5;
const FDA_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// The one Disk Map window, if open.
#[derive(Default)]
struct OpenDiskMap(Option<WindowHandle<DiskMap>>);

impl Global for OpenDiskMap {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Disks and Folders.
    Start,
    Scanning,
    Map,
}

/// Something that can be pointed at or selected on the map or in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Node(NodeId),
    /// The "smaller objects" of a folder.
    Smaller(NodeId),
    Special(Special),
}

impl Pick {
    fn of(kind: SegmentKind) -> Pick {
        match kind {
            SegmentKind::Node(id) => Pick::Node(id),
            SegmentKind::Smaller { parent, .. } => Pick::Smaller(parent),
            SegmentKind::Special(s) => Pick::Special(s),
        }
    }
}

/// An item dragged from the map or the list towards the collector.
#[derive(Clone)]
pub struct DraggedItem {
    pub id: NodeId,
}

struct ScanJob {
    target: PathBuf,
    label: String,
    progress: Arc<ScanProgress>,
    result: Arc<Mutex<Option<Result<Tree, dm::ScanError>>>>,
    started: Instant,
    /// Bytes used on the volume when scanning a volume root (drives the progress ring).
    expected: Option<u64>,
    /// Rescan of one folder of the current tree (§3, ⌘R).
    graft_at: Option<NodeId>,
}

/// Per item: deleted, or why not. Filled in by the deletion thread when it finishes.
type DeletionResults = Arc<Mutex<Option<Vec<(NodeId, Result<(), String>)>>>>;

struct Deletion {
    count: usize,
    done: Arc<AtomicUsize>,
    results: DeletionResults,
    permanent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cmd {
    Open,
    ShowInPanel,
    ShowInFinder,
    QuickLook,
    Terminal,
    Collect,
    Uncollect,
    Rescan,
    Expand,
    Info,
    Separator,
}

struct ContextMenu {
    position: Point<Pixels>,
    pick: Pick,
    items: Vec<(&'static str, Cmd, bool)>,
    id: usize,
}

struct StartVolume {
    volume: volumes::Volume,
    purgeable: u64,
}

pub struct DiskMap {
    workspace: WeakEntity<Workspace>,
    workspace_window: AnyWindowHandle,
    /// "This folder" on the start page: the active panel's folder (§1).
    suggested: Option<PathBuf>,
    page: Page,
    /// Why the last scan failed, and whether Full Disk Access might help.
    error: Option<(String, bool)>,
    volumes: Vec<StartVolume>,
    scan: Option<ScanJob>,
    tree: Option<Tree>,
    /// Bumped whenever `tree` is replaced, so cached layouts are rebuilt.
    generation: u64,
    history: History,
    selected: Option<Pick>,
    /// What the pointer is over on the map.
    hover: Option<Pick>,
    center_hover: bool,
    /// Row under the pointer in the list (highlights its segment).
    list_hover: Option<NodeId>,
    /// Folder whose "smaller objects" are expanded in the list.
    expanded: Option<NodeId>,
    segments: Vec<Segment>,
    seg_index: HashMap<NodeId, usize>,
    seg_key: (NodeId, u64, u64),
    bloom: Option<Instant>,
    map_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Sidebar rows as last rendered, the folder they belong to, and the pointed one.
    rows: Vec<Row>,
    row_folder: NodeId,
    row_pointed: Option<Pick>,
    collector_open: bool,
    /// Delete immediately instead of moving to the Trash.
    permanent: bool,
    countdown: Option<Instant>,
    deletion: Option<Deletion>,
    failed: HashMap<NodeId, String>,
    toast: Option<(String, bool, Instant)>,
    menu: Option<ContextMenu>,
    menu_seq: usize,
    list_scroll: UniformListScrollHandle,
    focus: FocusHandle,
    home: Option<PathBuf>,
    _poll: Task<()>,
    _appearance: Subscription,
}

/// Opens the Disk Map window, or brings it forward with a new suggested folder.
pub fn open(suggested: Option<PathBuf>, workspace: WeakEntity<Workspace>, workspace_window: AnyWindowHandle, cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenDiskMap>().and_then(|o| o.0) {
        let (s, w) = (suggested.clone(), workspace.clone());
        let reused = handle.update(cx, move |this, window, cx| {
            this.suggested = s;
            this.workspace = w;
            this.workspace_window = workspace_window;
            window.activate_window();
            cx.notify();
        });
        if reused.is_ok() {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(1080.), px(720.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Disk Map".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(16.))) }),
            window_min_size: Some(size(px(760.), px(520.))),
            focus: true,
            ..Default::default()
        },
        |window, cx| cx.new(|cx| DiskMap::new(suggested, workspace, workspace_window, window, cx)),
    );
    if let Ok(handle) = handle {
        cx.set_global(OpenDiskMap(Some(handle)));
    }
}

fn sz(bytes: u64) -> String {
    format_size(bytes, SizeMode::Dynamic)
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x).powi(3)
}

/// Display name of a scan target ("Macintosh HD" for `/`).
fn label_for(path: &Path) -> String {
    if path == Path::new("/") {
        return volumes::list_volumes().into_iter().find(|v| v.is_root).map(|v| v.name).unwrap_or_else(|| "/".into());
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

fn load_volumes() -> Vec<StartVolume> {
    crate::workspace::visible_volumes()
        .into_iter()
        .map(|volume| {
            let purgeable = dm::volume_summary(&volume.path, 0).map(|s| s.purgeable).unwrap_or(0);
            StartVolume { volume, purgeable }
        })
        .collect()
}

/// File-type tile for a tree node.
fn tile_kind(n: &Node) -> FileKind {
    let ext = n.name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, e)| e).unwrap_or("");
    match n.kind {
        NodeKind::Folder => FileKind::Folder,
        NodeKind::Package if ext.eq_ignore_ascii_case("app") => FileKind::App,
        NodeKind::Package => FileKind::Package,
        NodeKind::Symlink => FileKind::Link,
        NodeKind::File => look::kind_for_extension(ext, false),
    }
}

/// Colour of a segment (§5.3): folders take their hue from where they sit on the circle,
/// so a subtree keeps one colour family; files are grey.
fn segment_color(tree: &Tree, seg: &Segment, c: &Colors) -> Hsla {
    let depth = f32::from(seg.depth) - 1.0;
    let hue = (seg.mid() / TAU + 0.58).fract();
    let base = if c.dark { 0.52 } else { 0.58 };
    match seg.kind {
        SegmentKind::Node(id) => {
            let n = tree.node(id);
            if n.restricted {
                return gpui::hsla(0.76, 0.34, if c.dark { 0.56 } else { 0.64 }, 0.7);
            }
            match n.kind {
                NodeKind::Folder => gpui::hsla(hue, (0.68 - 0.05 * depth).max(0.3), (base + 0.04 * depth).min(0.82), 1.0),
                NodeKind::Package => gpui::hsla(hue, 0.3, (base + 0.06 + 0.035 * depth).min(0.84), 1.0),
                NodeKind::File | NodeKind::Symlink => {
                    let l = if c.dark { 0.42 + 0.035 * depth } else { 0.72 + 0.025 * depth };
                    gpui::hsla(0.0, 0.0, l.min(0.9), 1.0)
                }
            }
        }
        SegmentKind::Smaller { .. } => gpui::hsla(0.0, 0.0, if c.dark { 0.6 } else { 0.55 }, 0.45),
        SegmentKind::Special(Special::Purgeable) => gpui::hsla(0.37, 0.32, if c.dark { 0.55 } else { 0.66 }, 1.0),
        SegmentKind::Special(Special::Hidden) => gpui::hsla(0.0, 0.0, if c.dark { 0.3 } else { 0.48 }, 1.0),
    }
}

fn lighten(mut c: Hsla, by: f32) -> Hsla {
    c.l = (c.l + by).min(0.95);
    c
}

/// Outer radius of the map inside `bounds`.
fn map_radius(b: Bounds<Pixels>) -> f32 {
    (f32::from(b.size.width).min(f32::from(b.size.height)) / 2.0 - 16.0).max(40.0)
}

/// A filled ring segment (an annulus slice) around `center`; angles clockwise from 12 o'clock.
fn annulus(center: Point<Pixels>, r_in: f32, r_out: f32, start: f32, sweep: f32) -> Option<gpui::Path<Pixels>> {
    if sweep <= 0.0 || r_out <= r_in {
        return None;
    }
    let steps = ((sweep.to_degrees() / 2.5).ceil() as usize).max(1);
    let at = |r: f32, a: f32| point(center.x + px(r * a.sin()), center.y - px(r * a.cos()));
    let mut b = PathBuilder::fill();
    b.move_to(at(r_out, start));
    for i in 1..=steps {
        b.line_to(at(r_out, start + sweep * i as f32 / steps as f32));
    }
    for i in (0..=steps).rev() {
        b.line_to(at(r_in, start + sweep * i as f32 / steps as f32));
    }
    b.close();
    b.build().ok()
}

fn paint_map(bounds: Bounds<Pixels>, segments: &[(Segment, Hsla)], center_fill: Hsla, t: f32, window: &mut Window) {
    let center = bounds.center();
    let count = dm::ring_count(&segments.iter().map(|(s, _)| *s).collect::<Vec<_>>());
    let (r0, rings) = dm::ring_radii(map_radius(bounds), count);
    if let Some(p) = annulus(center, 0.0, (r0 - RING_GAP).max(1.0), 0.0, TAU) {
        window.paint_path(p, center_fill);
    }
    for (seg, color) in segments {
        let (a, b) = rings[seg.depth as usize - 1];
        let (r_in, r_out) = (a + RING_GAP / 2.0, b - RING_GAP / 2.0);
        // A hairline between neighbours, never more than a fifth of a thin segment.
        let pad = (0.6 / r_out).min(seg.sweep * 0.2);
        if let Some(p) = annulus(center, r_in, r_out, (seg.start + pad) * t, (seg.sweep - 2.0 * pad) * t) {
            window.paint_path(p, *color);
        }
    }
}

fn paint_progress(bounds: Bounds<Pixels>, fraction: Option<f32>, spin: f32, track: Hsla, fill: Hsla, window: &mut Window) {
    let center = bounds.center();
    let r = f32::from(bounds.size.width.min(bounds.size.height)) / 2.0;
    if let Some(p) = annulus(center, r - 12.0, r, 0.0, TAU) {
        window.paint_path(p, track);
    }
    let (start, sweep) = match fraction {
        Some(f) => (0.0, f.max(0.01) * TAU),
        None => ((spin * 0.8).fract() * TAU, TAU * 0.28),
    };
    if let Some(p) = annulus(center, r - 12.0, r, start, sweep) {
        window.paint_path(p, fill);
    }
}

impl DiskMap {
    fn new(suggested: Option<PathBuf>, workspace: WeakEntity<Workspace>, workspace_window: AnyWindowHandle, window: &mut Window, cx: &mut Context<Self>) -> DiskMap {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let poll = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            if this.update(cx, |this, cx| this.poll(cx)).is_err() {
                break;
            }
        });
        let appearance = cx.observe_window_appearance(window, |_, window, _| window.refresh());
        DiskMap {
            workspace,
            workspace_window,
            suggested,
            page: Page::Start,
            error: None,
            volumes: load_volumes(),
            scan: None,
            tree: None,
            generation: 0,
            history: History::default(),
            selected: None,
            hover: None,
            center_hover: false,
            list_hover: None,
            expanded: None,
            segments: Vec::new(),
            seg_index: HashMap::new(),
            seg_key: (ROOT, u64::MAX, u64::MAX),
            bloom: None,
            map_bounds: Rc::new(Cell::new(Bounds::default())),
            rows: Vec::new(),
            row_folder: ROOT,
            row_pointed: None,
            collector_open: false,
            permanent: false,
            countdown: None,
            deletion: None,
            failed: HashMap::new(),
            toast: None,
            menu: None,
            menu_seq: 0,
            list_scroll: UniformListScrollHandle::new(),
            focus,
            home: std::env::var_os("HOME").map(PathBuf::from),
            _poll: poll,
            _appearance: appearance,
        }
    }

    /// Back to Disks and Folders (smoke run).
    pub(crate) fn show_start_page(&mut self, cx: &mut Context<Self>) {
        self.show_start(cx);
    }

    pub(crate) fn page(&self) -> Page {
        self.page
    }

    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    pub(crate) fn current(&self) -> NodeId {
        self.history.current()
    }

    /// Points at the item at `path` as if the pointer were over it (demo recorder).
    pub(crate) fn point_at(&mut self, path: Option<&Path>, cx: &mut Context<Self>) {
        self.hover = path.and_then(|p| self.find(p)).map(Pick::Node);
        cx.notify();
    }

    /// Collects the item at `path` (demo recorder).
    pub(crate) fn collect_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        match self.find(path) {
            Some(id) => self.collect(id, cx),
            None => false,
        }
    }

    pub(crate) fn set_collector_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.collector_open = open;
        cx.notify();
    }

    fn find(&self, path: &Path) -> Option<NodeId> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.tree.as_ref()?.find(&path)
    }

    /// Countdown or deletion in progress.
    pub(crate) fn is_busy(&self) -> bool {
        self.countdown.is_some() || self.deletion.is_some()
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some((text.into(), error, Instant::now() + Duration::from_millis(3200)));
    }

    fn report(&mut self, r: std::io::Result<()>) {
        if let Err(e) = r {
            self.toast(e.to_string(), true);
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let finished = self.scan.as_ref().and_then(|job| job.result.lock().unwrap().take());
        if let Some(result) = finished {
            let job = self.scan.take().expect("a finished scan has a job");
            self.finish_scan(job, result);
            changed = true;
        } else if self.page == Page::Scanning {
            changed = true;
        }
        if let Some(t0) = self.countdown {
            changed = true;
            if t0.elapsed() >= Duration::from_secs(COUNTDOWN_SECS) {
                self.countdown = None;
                self.begin_deletion();
            }
        }
        let deleted = self.deletion.as_ref().and_then(|d| d.results.lock().unwrap().take());
        if let Some(results) = deleted {
            self.finish_deletion(results);
            changed = true;
        } else if self.deletion.is_some() {
            changed = true;
        }
        if self.toast.as_ref().is_some_and(|t| Instant::now() > t.2) {
            self.toast = None;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    // ----- scanning ---------------------------------------------------------------------

    /// Scans `target` from scratch (Disks and Folders, ⇧⌘R).
    pub(crate) fn start_scan(&mut self, target: PathBuf, cx: &mut Context<Self>) {
        self.spawn_scan(target, None, cx);
    }

    fn spawn_scan(&mut self, target: PathBuf, graft_at: Option<NodeId>, cx: &mut Context<Self>) {
        if self.is_busy() {
            self.toast("Wait for the deletion to finish first", true);
            cx.notify();
            return;
        }
        if let Some(job) = self.scan.take() {
            job.progress.cancel();
        }
        let target = std::fs::canonicalize(&target).unwrap_or(target);
        let expected = if graft_at.is_none() { dm::volume_summary(&target, 0).map(|s| s.used()) } else { None };
        let progress = Arc::new(ScanProgress::default());
        let result = Arc::new(Mutex::new(None));
        {
            let (progress, result, target) = (progress.clone(), result.clone(), target.clone());
            std::thread::spawn(move || {
                let out = dm::scan(&target, &progress);
                *result.lock().unwrap() = Some(out);
            });
        }
        self.scan = Some(ScanJob { label: label_for(&target), target, progress, result, started: Instant::now(), expected, graft_at });
        self.page = Page::Scanning;
        self.error = None;
        self.menu = None;
        cx.notify();
    }

    fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = self.scan.take() {
            job.progress.cancel();
            self.page = if job.graft_at.is_some() && self.tree.is_some() { Page::Map } else { Page::Start };
            cx.notify();
        }
    }

    fn finish_scan(&mut self, job: ScanJob, result: Result<Tree, dm::ScanError>) {
        match result {
            Ok(new) => {
                match (job.graft_at, self.tree.as_mut()) {
                    (Some(at), Some(tree)) if tree.is_alive(at) => {
                        let kept: Vec<PathBuf> = tree.collected().iter().map(|&c| tree.path(c)).collect();
                        tree.graft(at, new);
                        restore_collected(tree, &kept);
                        if tree.summary.is_some() {
                            tree.summary = dm::volume_summary(tree.root_path(), tree.root().size);
                        }
                        self.history.prune(tree);
                    }
                    _ => {
                        let mut new = new;
                        match self.tree.as_ref().filter(|old| old.root_path() == new.root_path()) {
                            Some(old) => {
                                let kept: Vec<PathBuf> = old.collected().iter().map(|&c| old.path(c)).collect();
                                self.history = self.history.remap(old, &new);
                                self.selected = match self.selected {
                                    Some(Pick::Node(id)) => new.find(&old.path(id)).map(Pick::Node),
                                    other => other,
                                };
                                restore_collected(&mut new, &kept);
                            }
                            None => {
                                self.history = History::default();
                                self.selected = None;
                            }
                        }
                        self.tree = Some(new);
                        self.generation += 1;
                        self.failed.clear();
                    }
                }
                self.page = Page::Map;
                self.hover = None;
                self.expanded = None;
                self.bloom = Some(Instant::now());
            }
            Err(dm::ScanError::Cancelled) => {}
            Err(e) => {
                let msg = format!("Can't scan “{}”: {e}", job.label);
                if job.graft_at.is_some() && self.tree.is_some() {
                    self.page = Page::Map;
                    self.toast(msg, true);
                } else {
                    self.error = Some((msg, e.is_permission()));
                    self.page = Page::Start;
                }
            }
        }
        let _ = job.target;
    }

    fn rescan_current(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = &self.tree else { return };
        let cur = self.history.current();
        if cur == ROOT {
            self.rescan_all(cx);
        } else {
            let path = tree.path(cur);
            self.spawn_scan(path, Some(cur), cx);
        }
    }

    fn rescan_folder(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some(tree) = &self.tree else { return };
        if id == ROOT {
            self.rescan_all(cx);
        } else {
            let path = tree.path(id);
            self.spawn_scan(path, Some(id), cx);
        }
    }

    fn rescan_all(&mut self, cx: &mut Context<Self>) {
        if let Some(tree) = &self.tree {
            let root = tree.root_path().to_path_buf();
            self.spawn_scan(root, None, cx);
        }
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Scan".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.start_scan(p, cx));
            }
        })
        .detach();
    }

    fn show_start(&mut self, cx: &mut Context<Self>) {
        self.volumes = load_volumes();
        self.page = Page::Start;
        self.menu = None;
        cx.notify();
    }

    fn drop_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        match paths.iter().find(|p| p.is_dir()) {
            Some(p) => self.start_scan(p.clone(), cx),
            None => {
                self.toast("Drop a folder or a disk to scan it", true);
                cx.notify();
            }
        }
    }

    // ----- navigation -------------------------------------------------------------------

    fn ensure_layout(&mut self) {
        let Some(tree) = &self.tree else {
            self.segments.clear();
            self.seg_index.clear();
            return;
        };
        let key = (self.history.current(), tree.version(), self.generation);
        if key != self.seg_key {
            self.segments = dm::layout(tree, key.0);
            self.seg_index = self.segments.iter().enumerate().filter_map(|(i, s)| s.node().map(|n| (n, i))).collect();
            self.seg_key = key;
        }
    }

    fn after_nav(&mut self, select: Option<Pick>, cx: &mut Context<Self>) {
        self.selected = select;
        self.hover = None;
        self.center_hover = false;
        self.list_hover = None;
        self.expanded = None;
        self.menu = None;
        self.bloom = Some(Instant::now());
        self.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn open_folder(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some(tree) = &self.tree else { return };
        let n = tree.node(id);
        if !n.is_dir() || !tree.is_alive(id) || n.collected {
            return;
        }
        if n.restricted {
            let msg = format!("“{}” could not be read. Full Disk Access may be needed.", n.name);
            self.toast(msg, true);
            cx.notify();
            return;
        }
        self.history.open(id);
        self.after_nav(None, cx);
    }

    pub(crate) fn go_up(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = &self.tree else { return };
        let cur = self.history.current();
        if let Some(parent) = tree.node(cur).parent {
            self.history.open(parent);
            self.after_nav(Some(Pick::Node(cur)), cx);
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(tree) = &self.tree
            && self.history.go_back(tree)
        {
            self.after_nav(None, cx);
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(tree) = &self.tree
            && self.history.go_forward(tree)
        {
            self.after_nav(None, cx);
        }
    }

    fn select(&mut self, pick: Pick, cx: &mut Context<Self>) {
        self.selected = Some(pick);
        if let Some(ix) = self.row_picks().iter().position(|&p| p == pick) {
            self.list_scroll.scroll_to_item(ix, ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn row_pick(&self, row: Row) -> Pick {
        match row {
            Row::Node(id) => Pick::Node(id),
            Row::Smaller { .. } => Pick::Smaller(self.row_folder),
            Row::Special(s, _) => Pick::Special(s),
        }
    }

    fn row_picks(&self) -> Vec<Pick> {
        self.rows.iter().map(|&r| self.row_pick(r)).collect()
    }

    fn step_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let picks = self.row_picks();
        if picks.is_empty() {
            return;
        }
        let last = picks.len() as isize - 1;
        let next = match self.selected.and_then(|s| picks.iter().position(|&p| p == s)) {
            None if delta > 0 => 0,
            None => last,
            Some(i) => (i as isize + delta).clamp(0, last),
        };
        self.select(picks[next as usize], cx);
    }

    fn activate(&mut self, pick: Pick, cx: &mut Context<Self>) {
        match pick {
            Pick::Node(id) => self.open_folder(id, cx),
            Pick::Smaller(folder) => self.expand_smaller(folder, cx),
            Pick::Special(_) => self.select(pick, cx),
        }
    }

    fn expand_smaller(&mut self, folder: NodeId, cx: &mut Context<Self>) {
        if folder != self.history.current() {
            self.open_folder(folder, cx);
        }
        self.expanded = Some(folder);
        self.selected = None;
        cx.notify();
    }

    fn path_of(&self, id: NodeId) -> Option<PathBuf> {
        self.tree.as_ref().map(|t| t.path(id))
    }

    fn reveal(&mut self, id: NodeId) {
        if let Some(p) = self.path_of(id) {
            let r = platform::reveal_in_finder(&[p]);
            self.report(r);
        }
    }

    fn quick_look(&mut self, id: NodeId) {
        if let Some(p) = self.path_of(id) {
            let r = platform::quick_look(&[p]);
            self.report(r);
        }
    }

    /// Reveals the item in the main window's active panel (§7).
    fn show_in_panel(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let Some(path) = self.path_of(id) else { return };
        let ws = self.workspace.clone();
        let _ = self.workspace_window.update(cx, move |_, window, cx| {
            let _ = ws.update(cx, |ws, cx| {
                let side = ws.active;
                let parent = path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone());
                ws.navigate(side, parent, Some(&path), window, cx);
            });
            window.activate_window();
        });
    }

    // ----- collector --------------------------------------------------------------------

    pub(crate) fn collect(&mut self, id: NodeId, cx: &mut Context<Self>) -> bool {
        if self.is_busy() {
            self.toast("Wait for the deletion to finish first", true);
            cx.notify();
            return false;
        }
        let Some(tree) = self.tree.as_mut() else { return false };
        if let Err(refusal) = tree.can_collect(id, self.home.as_deref()) {
            let msg = refusal.to_string();
            self.toast(msg, true);
            cx.notify();
            return false;
        }
        tree.collect(id);
        self.failed.remove(&id);
        if self.selected == Some(Pick::Node(id)) {
            self.selected = None;
        }
        self.hover = None;
        self.list_hover = None;
        cx.notify();
        true
    }

    fn uncollect(&mut self, id: NodeId, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if let Some(tree) = self.tree.as_mut() {
            tree.uncollect(id);
            self.failed.remove(&id);
            cx.notify();
        }
    }

    fn uncollect_all(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if let Some(tree) = self.tree.as_mut() {
            for id in tree.collected().to_vec() {
                tree.uncollect(id);
            }
            self.failed.clear();
            cx.notify();
        }
    }

    fn toggle_collect_selected(&mut self, cx: &mut Context<Self>) {
        let Some(tree) = &self.tree else { return };
        match self.selected {
            Some(Pick::Node(id)) if tree.node(id).collected => self.uncollect(id, cx),
            Some(Pick::Node(id)) => {
                self.collect(id, cx);
            }
            Some(Pick::Smaller(_)) => {
                self.toast("Expand “smaller objects” to collect individual items", true);
                cx.notify();
            }
            Some(Pick::Special(_)) => {
                self.toast("This space can't be deleted from here", true);
                cx.notify();
            }
            None => {}
        }
    }

    fn start_countdown(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() || self.tree.as_ref().is_none_or(|t| t.collected().is_empty()) {
            return;
        }
        self.countdown = Some(Instant::now());
        cx.notify();
    }

    fn stop_countdown(&mut self, cx: &mut Context<Self>) {
        self.countdown = None;
        cx.notify();
    }

    /// Deletes the collected items now, skipping the countdown (smoke run).
    pub(crate) fn delete_collected_now(&mut self, permanent: bool, cx: &mut Context<Self>) {
        self.permanent = permanent;
        self.countdown = None;
        self.begin_deletion();
        cx.notify();
    }

    fn begin_deletion(&mut self) {
        let Some(tree) = &self.tree else { return };
        if self.deletion.is_some() || tree.collected().is_empty() {
            return;
        }
        let items: Vec<(NodeId, PathBuf)> = tree.collected().iter().map(|&id| (id, tree.path(id))).collect();
        let permanent = self.permanent;
        let done = Arc::new(AtomicUsize::new(0));
        let results: DeletionResults = Arc::new(Mutex::new(None));
        self.deletion = Some(Deletion { count: items.len(), done: done.clone(), results: results.clone(), permanent });
        std::thread::spawn(move || {
            let mut out = Vec::with_capacity(items.len());
            for (id, path) in items {
                let r = if permanent { ops::remove_any(&path) } else { ops::move_to_trash(&path) };
                out.push((id, r.map_err(|e| e.to_string())));
                done.fetch_add(1, Ordering::Relaxed);
            }
            *results.lock().unwrap() = Some(out);
        });
    }

    fn finish_deletion(&mut self, results: Vec<(NodeId, Result<(), String>)>) {
        let Some(deletion) = self.deletion.take() else { return };
        let Some(tree) = self.tree.as_mut() else { return };
        let (mut bytes, mut ok, mut failed) = (0u64, 0usize, 0usize);
        for (id, r) in results {
            match r {
                Ok(()) => {
                    bytes += tree.node(id).size;
                    tree.remove(id);
                    self.failed.remove(&id);
                    ok += 1;
                }
                Err(e) => {
                    self.failed.insert(id, e);
                    failed += 1;
                }
            }
        }
        if tree.summary.is_some() {
            tree.summary = dm::volume_summary(tree.root_path(), tree.root().size);
        }
        self.history.prune(tree);
        if let Some(Pick::Node(id)) = self.selected
            && !tree.is_alive(id)
        {
            self.selected = None;
        }
        let items = |n: usize| if n == 1 { "1 item".to_string() } else { format!("{n} items") };
        let mut msg = if deletion.permanent { format!("Freed {}", sz(bytes)) } else { format!("Moved {} ({}) to the Trash", items(ok), sz(bytes)) };
        if failed > 0 {
            msg = format!("{msg}. {} could not be deleted", items(failed));
            self.collector_open = true;
        }
        self.toast(msg, failed > 0);
    }

    // ----- context menu -----------------------------------------------------------------

    fn open_menu(&mut self, pick: Pick, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(tree) = &self.tree else { return };
        let busy = self.is_busy();
        let items = match pick {
            Pick::Node(id) => {
                let n = tree.node(id);
                let dir = n.is_dir();
                let mut v = vec![("Open", Cmd::Open, !n.collected && !(dir && n.restricted))];
                v.push(("Show in Panel", Cmd::ShowInPanel, true));
                v.push(("Show in Finder", Cmd::ShowInFinder, true));
                v.push(("Quick Look", Cmd::QuickLook, true));
                if dir {
                    v.push(("Open in Terminal", Cmd::Terminal, true));
                }
                v.push(("", Cmd::Separator, false));
                if n.collected {
                    v.push(("Remove from Collector", Cmd::Uncollect, !busy));
                } else {
                    v.push(("Move to Collector", Cmd::Collect, !busy && tree.can_collect(id, self.home.as_deref()).is_ok()));
                }
                if dir && !n.collected {
                    v.push(("", Cmd::Separator, false));
                    v.push(("Rescan This Folder", Cmd::Rescan, !busy));
                }
                v
            }
            Pick::Smaller(_) => vec![("Expand “smaller objects”", Cmd::Expand, true)],
            Pick::Special(_) => vec![("More Info", Cmd::Info, true)],
        };
        self.menu_seq += 1;
        self.menu = Some(ContextMenu { position, pick, items, id: self.menu_seq });
        cx.notify();
    }

    fn run_cmd(&mut self, cmd: Cmd, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        window.focus(&self.focus);
        let node = match pick {
            Pick::Node(id) => Some(id),
            _ => None,
        };
        match (cmd, node) {
            (Cmd::Open, Some(id)) => {
                let is_dir = self.tree.as_ref().is_some_and(|t| t.node(id).is_dir());
                if is_dir {
                    self.open_folder(id, cx);
                } else if let Some(p) = self.path_of(id) {
                    let r = platform::open_default(&p);
                    self.report(r);
                }
            }
            (Cmd::ShowInPanel, Some(id)) => self.show_in_panel(id, cx),
            (Cmd::ShowInFinder, Some(id)) => self.reveal(id),
            (Cmd::QuickLook, Some(id)) => self.quick_look(id),
            (Cmd::Terminal, Some(id)) => {
                if let Some(p) = self.path_of(id) {
                    let r = platform::open_terminal(&p);
                    self.report(r);
                }
            }
            (Cmd::Collect, Some(id)) => {
                self.collect(id, cx);
            }
            (Cmd::Uncollect, Some(id)) => self.uncollect(id, cx),
            (Cmd::Rescan, Some(id)) => self.rescan_folder(id, cx),
            (Cmd::Expand, _) => {
                if let Pick::Smaller(folder) = pick {
                    self.expand_smaller(folder, cx);
                }
            }
            (Cmd::Info, _) => self.select(pick, cx),
            _ => {}
        }
        cx.notify();
    }

    // ----- input ------------------------------------------------------------------------

    fn map_hit(&self, pos: Point<Pixels>) -> dm::Hit {
        let b = self.map_bounds.get();
        let c = b.center();
        dm::hit_test(&self.segments, map_radius(b), f32::from(pos.x - c.x), f32::from(pos.y - c.y))
    }

    fn map_mouse_move(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (hover, center) = match self.map_hit(pos) {
            dm::Hit::Center => (None, true),
            dm::Hit::Segment(i) => (Some(Pick::of(self.segments[i].kind)), false),
            dm::Hit::Nothing => (None, false),
        };
        if hover != self.hover || center != self.center_hover {
            self.hover = hover;
            self.center_hover = center;
            cx.notify();
        }
    }

    fn map_click(&mut self, ev: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.menu = None;
        let Some(tree) = &self.tree else { return };
        match self.map_hit(ev.position()) {
            dm::Hit::Center => self.go_up(cx),
            dm::Hit::Segment(i) => {
                let pick = Pick::of(self.segments[i].kind);
                if ev.modifiers().platform {
                    if let Pick::Node(id) = pick {
                        self.reveal(id);
                    }
                    return;
                }
                match pick {
                    Pick::Node(id) if tree.node(id).is_dir() && !tree.node(id).restricted => self.open_folder(id, cx),
                    Pick::Smaller(folder) => self.expand_smaller(folder, cx),
                    _ => self.select(pick, cx),
                }
            }
            dm::Hit::Nothing => {
                self.selected = None;
                cx.notify();
            }
        }
    }

    fn row_press(&mut self, pick: Pick, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.menu = None;
        if ev.modifiers.platform {
            if let Pick::Node(id) = pick {
                self.reveal(id);
            }
            return;
        }
        match pick {
            Pick::Smaller(folder) => self.expand_smaller(folder, cx),
            Pick::Node(id) if ev.click_count >= 2 => {
                let is_dir = self.tree.as_ref().is_some_and(|t| t.node(id).is_dir());
                if is_dir {
                    self.open_folder(id, cx);
                } else if let Some(p) = self.path_of(id) {
                    let r = platform::open_default(&p);
                    self.report(r);
                    cx.notify();
                }
            }
            _ => self.select(pick, cx),
        }
    }

    fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = ev.keystroke.modifiers;
        let key = ev.keystroke.key.as_str();
        let cmd = m.platform;
        let plain = !m.platform && !m.control && !m.alt && !m.shift;
        if self.menu.is_some() {
            if key == "escape" {
                self.menu = None;
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        if cmd && key == "w" {
            window.remove_window();
            cx.stop_propagation();
            return;
        }
        let handled = match self.page {
            Page::Start => match key {
                "o" if cmd => {
                    self.choose_folder(window, cx);
                    true
                }
                "enter" if plain && self.tree.is_some() => {
                    self.page = Page::Map;
                    cx.notify();
                    true
                }
                _ => false,
            },
            Page::Scanning => match key {
                "escape" if plain => {
                    self.cancel_scan(cx);
                    true
                }
                _ => false,
            },
            Page::Map => self.map_key(key, cmd, m.shift, plain, window, cx),
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn map_key(&mut self, key: &str, cmd: bool, shift: bool, plain: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match key {
            "up" if cmd && shift => self.show_start(cx),
            "up" if cmd => self.go_up(cx),
            "backspace" if cmd => self.toggle_collect_selected(cx),
            "backspace" if plain => self.go_up(cx),
            "[" if cmd => self.go_back(cx),
            "]" if cmd => self.go_forward(cx),
            "r" | "R" if cmd && shift => self.rescan_all(cx),
            "r" if cmd => self.rescan_current(cx),
            "o" if cmd => self.choose_folder(window, cx),
            "up" if plain => self.step_selection(-1, cx),
            "down" if plain => self.step_selection(1, cx),
            "pageup" => self.step_selection(-12, cx),
            "pagedown" => self.step_selection(12, cx),
            "home" => self.step_selection(-isize::MAX / 2, cx),
            "end" => self.step_selection(isize::MAX / 2, cx),
            "enter" if plain => {
                if let Some(p) = self.selected {
                    self.activate(p, cx);
                }
            }
            "space" if plain => {
                if let Some(Pick::Node(id)) = self.selected {
                    self.quick_look(id);
                    cx.notify();
                }
            }
            "escape" if plain => {
                if self.collector_open {
                    self.collector_open = false;
                } else {
                    self.selected = None;
                }
                cx.notify();
            }
            _ => return false,
        }
        true
    }

    /// Folder the sidebar shows and the item pointed at in it (§5.4): the hovered folder,
    /// or a hovered file's folder, or the current folder.
    fn sidebar_folder(&self) -> (NodeId, Option<Pick>) {
        let cur = self.history.current();
        let Some(tree) = &self.tree else { return (cur, None) };
        match self.hover {
            Some(Pick::Node(id)) => {
                let n = tree.node(id);
                if n.is_dir() && !n.restricted && !n.children.is_empty() {
                    (id, None)
                } else {
                    (n.parent.unwrap_or(cur), Some(Pick::Node(id)))
                }
            }
            Some(Pick::Smaller(folder)) => (folder, Some(Pick::Smaller(folder))),
            Some(p @ Pick::Special(_)) => (cur, Some(p)),
            None => (cur, None),
        }
    }

    /// Colour of a node as drawn on the map, for list swatches.
    fn node_color(&self, tree: &Tree, id: NodeId, c: &Colors) -> Hsla {
        if let Some(&i) = self.seg_index.get(&id) {
            return segment_color(tree, &self.segments[i], c);
        }
        let n = tree.node(id);
        match n.kind {
            NodeKind::Folder | NodeKind::Package => c.folder,
            _ => gpui::hsla(0.0, 0.0, if c.dark { 0.5 } else { 0.7 }, 1.0),
        }
    }

    fn special_color(&self, s: Special, c: &Colors) -> Hsla {
        let tree = self.tree.as_ref().expect("map page has a tree");
        segment_color(tree, &Segment { kind: SegmentKind::Special(s), depth: 1, start: 0.0, sweep: 0.0 }, c)
    }

    // ----- rendering --------------------------------------------------------------------

    fn render_titlebar(&mut self, c: &Colors, cx: &mut Context<Self>) -> Div {
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(46.))
            .pl(px(84.))
            .pr(px(12.))
            .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
                if ev.click_count == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(look::icon(icons::SUNBURST, 15., c.accent))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).flex_none().child("Disk Map"));

        if self.page == Page::Map
            && let Some(tree) = &self.tree
        {
            let cur = self.history.current();
            let (back, fwd, up) = (self.history.can_go_back(tree), self.history.can_go_forward(tree), cur != ROOT);
            let nav = |id: &'static str, glyph: &'static str, enabled: bool, c: &Colors, cx: &mut Context<Self>, f: fn(&mut DiskMap, &mut Context<DiskMap>)| {
                look::ghost_button(id, glyph, c, false, enabled).when(enabled, |d| {
                    d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
                        cx.stop_propagation();
                        f(this, cx);
                    }))
                })
            };
            bar = bar
                .child(div().w(px(8.)))
                .child(nav("dm-back", icons::BACK, back, c, cx, |this, cx| this.go_back(cx)))
                .child(nav("dm-fwd", icons::FORWARD, fwd, c, cx, |this, cx| this.go_forward(cx)))
                .child(nav("dm-up", icons::UP, up, c, cx, |this, cx| this.go_up(cx)));

            let mut chain: Vec<NodeId> = tree.ancestors(cur).collect();
            chain.reverse();
            chain.push(cur);
            let shown: Vec<Option<NodeId>> = if chain.len() > 5 {
                std::iter::once(Some(chain[0])).chain(std::iter::once(None)).chain(chain[chain.len() - 3..].iter().map(|&i| Some(i))).collect()
            } else {
                chain.iter().map(|&i| Some(i)).collect()
            };
            let mut crumbs = div().flex().items_center().gap(px(2.)).min_w(px(0.)).overflow_hidden();
            for (ix, crumb) in shown.into_iter().enumerate() {
                if ix > 0 {
                    crumbs = crumbs.child(look::icon(icons::CHEVRON_RIGHT, 11., c.muted.opacity(0.7)));
                }
                crumbs = crumbs.child(match crumb {
                    None => div().px(px(4.)).text_color(c.muted).child("…").into_any_element(),
                    Some(id) => {
                        let last = id == cur;
                        div()
                            .id(("dm-crumb", ix))
                            .px(px(8.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .text_size(px(12.5))
                            .whitespace_nowrap()
                            .max_w(px(200.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(last, |d| d.font_weight(FontWeight::SEMIBOLD).bg(c.sunken))
                            .when(!last, |d| {
                                d.text_color(c.muted).cursor_pointer().hover(|s| s.bg(c.hover)).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
                                    cx.stop_propagation();
                                    this.history.open(id);
                                    this.after_nav(None, cx);
                                }))
                            })
                            .child(tree.node(id).name.to_string())
                            .into_any_element()
                    }
                });
            }
            bar = bar.child(div().w(px(6.))).child(crumbs);
        }
        bar = bar.child(div().flex_1());
        if self.page == Page::Map {
            bar = bar
                .child(pill_button("dm-rescan", Some(icons::REFRESH), "Rescan", false, !self.is_busy(), c, cx, |this, _w, cx| this.rescan_current(cx)))
                .child(pill_button("dm-sources", Some(icons::DISK), "Disks and Folders", false, true, c, cx, |this, _w, cx| this.show_start(cx)));
        }
        bar
    }

    fn render_start(&mut self, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().w_full().max_w(px(720.)).flex().flex_col().gap(px(10.)).px(px(20.)).pt(px(8.)).pb(px(24.));
        col = col
            .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child("Disks and Folders"))
            .child(div().text_color(c.muted).child("Choose a disk or folder to see what fills it. You can also drop a folder here."));

        if let Some((msg, fda)) = self.error.clone() {
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(c.danger.opacity(0.12))
                    .border_1()
                    .border_color(c.danger.opacity(0.35))
                    .child(look::icon(icons::WARNING, 16., c.danger))
                    .child(div().flex_1().min_w(px(0.)).child(msg))
                    .when(fda, |d| {
                        d.child(pill_button("dm-fda", None, "Full Disk Access…", false, true, c, cx, |this, _w, cx| {
                            let r = platform::open_url(FDA_URL);
                            this.report(r);
                            cx.notify();
                        }))
                    }),
            );
        }

        if let Some(tree) = &self.tree {
            let r = tree.root();
            let when = format_time_friendly(Some(tree.scanned_at), SystemTime::now());
            col = col.child(look::section_label("Last scan", c)).child(source_row(
                "dm-last".into(),
                icons::SUNBURST,
                r.name.to_string(),
                format!("{} · {} · scanned {when}", tree.root_path().display(), sz(r.size)),
                None,
                "View",
                c,
                cx,
                |this, _w, cx| {
                    this.page = Page::Map;
                    this.bloom = Some(Instant::now());
                    cx.notify();
                },
            ));
        }

        col = col.child(look::section_label("Folders", c));
        let home = self.home.clone();
        if let Some(s) = self.suggested.clone().filter(|s| Some(s) != home.as_ref()) {
            let target = s.clone();
            col = col.child(source_row("dm-this".into(), icons::FOLDER_OPEN, format!("This folder: {}", label_for(&s)), s.display().to_string(), None, "Scan", c, cx, move |this, _w, cx| this.start_scan(target.clone(), cx)));
        }
        if let Some(h) = home {
            let target = h.clone();
            col = col.child(source_row("dm-home".into(), icons::HOME, "Home".into(), h.display().to_string(), None, "Scan", c, cx, move |this, _w, cx| this.start_scan(target.clone(), cx)));
        }
        col = col.child(source_row("dm-choose".into(), icons::FOLDER, "Choose Folder…".into(), "Pick any folder to scan".into(), None, "Choose", c, cx, |this, w, cx| this.choose_folder(w, cx)));

        col = col.child(look::section_label("Disks", c));
        for (ix, v) in self.volumes.iter().enumerate() {
            let vol = &v.volume;
            let used = vol.total.saturating_sub(vol.free);
            let frac = |b: u64| if vol.total > 0 { b as f32 / vol.total as f32 } else { 0.0 };
            let mut sub = format!("{} used of {}", sz(used), sz(vol.total));
            if v.purgeable > 0 {
                sub = format!("{sub} · {} free + {} purgeable", sz(vol.free), sz(v.purgeable));
            }
            let target = vol.path.clone();
            col = col.child(source_row(
                SharedString::from(format!("dm-vol-{ix}")),
                icons::DRIVE,
                vol.name.clone(),
                sub,
                Some((frac(used.saturating_sub(v.purgeable)), frac(v.purgeable))),
                "Scan",
                c,
                cx,
                move |this, _w, cx| this.start_scan(target.clone(), cx),
            ));
        }

        let tint = c.accent.opacity(0.08);
        div()
            .id("dm-start")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(tint))
            .drag_over::<DraggedFiles>(move |s, _, _, _| s.bg(tint))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _w, cx| this.drop_paths(paths.paths(), cx)))
            .on_drop(cx.listener(|this, files: &DraggedFiles, _w, cx| this.drop_paths(&files.paths, cx)))
            .child(col)
            .into_any_element()
    }

    fn render_scanning(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(job) = &self.scan else { return div().flex_1().into_any_element() };
        window.request_animation_frame();
        let items = job.progress.items.load(Ordering::Relaxed);
        let bytes = job.progress.bytes.load(Ordering::Relaxed);
        let fraction = job.expected.filter(|&e| e > 0).map(|e| (bytes as f32 / e as f32).min(0.99));
        let elapsed = job.started.elapsed();
        let spin = elapsed.as_secs_f32();
        let (track, fill) = (c.sunken, c.accent);
        let current = job.progress.current();
        let current = current.strip_prefix(&job.target).map(|p| p.display().to_string()).unwrap_or_default();
        let big = match fraction {
            Some(f) => format!("{:.0}%", f * 100.0),
            None => sz(bytes),
        };
        let secs = elapsed.as_secs();
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .child(
                div()
                    .relative()
                    .size(px(168.))
                    .child(canvas(|_, _, _| {}, move |b, _, window, _| paint_progress(b, fraction, spin, track, fill, window)).absolute().size_full())
                    .child(div().absolute().size_full().flex().items_center().justify_center().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(big)),
            )
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(format!("Scanning “{}”…", job.label)))
            .child(div().text_color(c.muted).child(format!("{} items · {} · {}:{:02}", group_digits(items), sz(bytes), secs / 60, secs % 60)))
            .child(div().max_w(px(560.)).text_size(px(11.5)).text_color(c.muted.opacity(0.8)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(current))
            .child(pill_button("dm-cancel", None, "Cancel", false, true, c, cx, |this, _w, cx| this.cancel_scan(cx)))
            .into_any_element()
    }

    fn render_map_page(&mut self, c: &Colors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_layout();
        let t = match self.bloom {
            Some(t0) => {
                let x = (t0.elapsed().as_secs_f32() / BLOOM.as_secs_f32()).min(1.0);
                if x < 1.0 {
                    window.request_animation_frame();
                } else {
                    self.bloom = None;
                }
                ease_out_cubic(x)
            }
            None => 1.0,
        };
        let Some(tree) = &self.tree else { return div().into_any_element() };
        let cur = self.history.current();

        // Brighten the pointed item and everything inside it, and the selected segment.
        let lit = match self.hover {
            Some(Pick::Node(id)) => Some(id),
            _ => self.list_hover,
        };
        let painted: Vec<(Segment, Hsla)> = self
            .segments
            .iter()
            .map(|s| {
                let col = segment_color(tree, s, c);
                let pick = Pick::of(s.kind);
                let on = match (s.kind, lit) {
                    (SegmentKind::Node(id), Some(h)) => id == h || tree.is_ancestor(h, id),
                    _ => self.hover == Some(pick),
                };
                let col = if self.selected == Some(pick) { lighten(col, 0.16) } else { col };
                (*s, if on { lighten(col, 0.1) } else { col })
            })
            .collect();
        let center_fill = if self.center_hover && cur != ROOT { c.accent.opacity(0.24) } else if c.dark { gpui::white().opacity(0.07) } else { gpui::black().opacity(0.05) };
        let cell = self.map_bounds.clone();
        let map_canvas = canvas(move |b, _, _| cell.set(b), move |b, _, window, _| paint_map(b, &painted, center_fill, t, window)).absolute().size_full();

        // Centre label.
        let r0 = dm::ring_radii(map_radius(self.map_bounds.get()), dm::ring_count(&self.segments)).0;
        let root_node = tree.node(cur);
        let (title, detail) = if self.center_hover && cur != ROOT {
            let parent = root_node.parent.map(|p| tree.node(p).name.to_string()).unwrap_or_default();
            (format!("↑ {parent}"), "Back to the enclosing folder".to_string())
        } else {
            match self.hover {
                Some(Pick::Node(id)) => {
                    let n = tree.node(id);
                    (n.name.to_string(), sz(n.shown))
                }
                Some(Pick::Smaller(folder)) => {
                    let (count, bytes) = self.segments.iter().find_map(|s| match s.kind {
                        SegmentKind::Smaller { parent, count, bytes } if parent == folder => Some((count, bytes)),
                        _ => None,
                    }).unwrap_or((0, 0));
                    (format!("{count} smaller objects"), sz(bytes))
                }
                Some(Pick::Special(s)) => (s.label().to_string(), tree.summary.map(|v| sz(v.amount(s))).unwrap_or_default()),
                None if root_node.shown == 0 => (root_node.name.to_string(), "This folder is empty".to_string()),
                None => (root_node.name.to_string(), sz(root_node.shown)),
            }
        };
        let label = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .max_w(px((r0 * 1.62).max(60.)))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(div().max_w_full().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(title))
                    .child(div().max_w_full().text_size(px(12.)).text_color(c.muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(detail)),
            );

        let drag = match self.hover {
            Some(Pick::Node(id)) if id != ROOT => Some((DraggedItem { id }, tree.node(id).name.to_string())),
            _ => None,
        };
        let map_area = div()
            .id("dm-map")
            .relative()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .when(self.hover.is_some() || (self.center_hover && cur != ROOT), |d| d.cursor_pointer())
            .child(map_canvas)
            .child(label)
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _w, cx| this.map_mouse_move(ev.position, cx)))
            .on_hover(cx.listener(|this, hovered: &bool, _w, cx| {
                if !*hovered && (this.hover.is_some() || this.center_hover) {
                    this.hover = None;
                    this.center_hover = false;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|this, ev: &ClickEvent, w, cx| this.map_click(ev, w, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                this.map_mouse_move(ev.position, cx);
                if let Some(p) = this.hover {
                    this.open_menu(p, ev.position, cx);
                }
            }))
            .when_some(drag, |d, (item, name)| d.on_drag(item, move |_, _, _, cx| cx.new(|_| DragPreview { label: name.clone() })));

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(div().flex_1().min_h(px(0.)).flex().child(map_area).child(self.render_sidebar(c, cx)))
            .child(self.render_collector(c, cx))
            .into_any_element()
    }

    fn render_sidebar(&mut self, c: &Colors, cx: &mut Context<Self>) -> Div {
        let (folder, pointed) = self.sidebar_folder();
        let cur = self.history.current();
        let Some(tree) = &self.tree else { return div() };
        self.rows = dm::rows(tree, folder, self.expanded == Some(folder));
        self.row_folder = folder;
        self.row_pointed = pointed;
        let n = tree.node(folder);
        let swatch = if folder == cur { c.accent } else { self.node_color(tree, folder, c) };
        let files = if n.items == 1 { "1 file".to_string() } else { format!("{} files", group_digits(n.items)) };

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(c.border.opacity(0.6))
            .child(div().size(px(10.)).rounded_full().bg(swatch).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(n.name.to_string()))
                    .child(div().text_size(px(11.5)).text_color(c.muted).child(if folder == cur { files } else { format!("{files} · pointed") })),
            )
            .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(sz(n.shown)));

        let list = uniform_list("dm-rows", self.rows.len(), cx.processor(|this, range: Range<usize>, window, cx| this.render_rows(range, window, cx)))
            .track_scroll(self.list_scroll.clone())
            .flex_1();
        let footer = self.render_footer(c, cx);

        div()
            .w(px(SIDEBAR_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .mt(px(4.))
            .mr(px(12.))
            .mb(px(10.))
            .rounded(px(14.))
            .overflow_hidden()
            .bg(c.bg.opacity(0.94))
            .border_1()
            .border_color(c.border)
            .shadow(look::soft_shadow(c))
            .child(header)
            .child(div().flex_1().min_h(px(0.)).flex().flex_col().child(list))
            .child(footer)
    }

    fn render_rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let c = Colors::get(window, cx);
        let Some(tree) = &self.tree else { return Vec::new() };
        let mut out = Vec::new();
        for ix in range {
            let Some(&row) = self.rows.get(ix) else { break };
            let pick = self.row_pick(row);
            let selected = self.selected == Some(pick);
            let pointed = self.row_pointed == Some(pick) || matches!(row, Row::Node(id) if self.list_hover == Some(id));
            let (swatch, tile, name, size, restricted, node) = match row {
                Row::Node(id) => {
                    let n = tree.node(id);
                    let name = if n.hard_link { format!("{} (hard link)", n.name) } else { n.name.to_string() };
                    (self.node_color(tree, id, &c), Some(tile_kind(n)), name, sz(n.shown), n.restricted, Some(id))
                }
                Row::Smaller { count, bytes } => (gpui::hsla(0.0, 0.0, 0.6, 0.6), None, format!("smaller objects… ({})", group_digits(count as u64)), sz(bytes), false, None),
                Row::Special(s, bytes) => (self.special_color(s, &c), None, format!("{}…", s.label()), sz(bytes), false, None),
            };
            let el = div()
                .id(("dm-row", ix))
                .w_full()
                .h(px(ROW_HEIGHT))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .when(selected, |d| d.bg(c.accent.opacity(0.2)))
                .when(pointed && !selected, |d| d.bg(c.hover))
                .when(!selected, |d| d.hover(|s| s.bg(c.hover)))
                .child(div().size(px(9.)).rounded_full().flex_none().bg(swatch))
                .child(match (tile, node) {
                    (Some(k), Some(id)) => look::entry_tile(&tree.path(id), k, 18., &c, false, cx).into_any_element(),
                    (Some(k), None) => look::file_tile(k, 18., &c, false).into_any_element(),
                    (None, _) => div().size(px(18.)).flex_none().flex().items_center().justify_center().child(look::icon(if matches!(row, Row::Smaller { .. }) { icons::GRID } else { icons::INFO }, 13., c.muted)).into_any_element(),
                })
                .child(div().flex_1().min_w(px(0.)).whitespace_nowrap().overflow_hidden().text_ellipsis().when(node.is_none(), |d| d.text_color(c.muted)).child(name))
                .when(restricted, |d| d.child(look::icon(icons::LOCK, 12., c.muted)))
                .child(div().flex_none().text_size(px(12.)).text_color(c.muted).child(size))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, w, cx| this.row_press(pick, ev, w, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _w, cx| this.open_menu(pick, ev.position, cx)))
                .when_some(node, |d, id| {
                    d.on_hover(cx.listener(move |this, hovered: &bool, _w, cx| {
                        let next = if *hovered { Some(id) } else if this.list_hover == Some(id) { None } else { this.list_hover };
                        if next != this.list_hover {
                            this.list_hover = next;
                            cx.notify();
                        }
                    }))
                    .on_drag(DraggedItem { id }, {
                        let label = tree.node(id).name.to_string();
                        move |_, _, _, cx| cx.new(|_| DragPreview { label: label.clone() })
                    })
                });
            out.push(el.into_any_element());
        }
        out
    }

    fn render_footer(&mut self, c: &Colors, cx: &mut Context<Self>) -> Div {
        let tree = self.tree.as_ref().expect("map page has a tree");
        let cur = self.history.current();
        let footer = div().flex_none().flex().flex_col().gap(px(6.)).px(px(12.)).py(px(10.)).border_t_1().border_color(c.border.opacity(0.6)).text_size(px(11.5));
        let special = match (self.selected, self.row_pointed) {
            (Some(Pick::Special(s)), _) | (_, Some(Pick::Special(s))) => Some(s),
            _ => None,
        };
        if let Some(s) = special {
            return footer.child(div().font_weight(FontWeight::SEMIBOLD).child(s.label().to_string())).child(div().text_color(c.muted).child(s.explanation()));
        }
        if cur == ROOT
            && let Some(s) = tree.summary
        {
            let total = s.capacity.max(1) as f32;
            let used = s.used().saturating_sub(s.purgeable);
            let bar = div()
                .h(px(8.))
                .w_full()
                .rounded_full()
                .overflow_hidden()
                .bg(c.sunken)
                .flex()
                .child(div().h_full().w(relative(used as f32 / total)).bg(c.accent))
                .child(div().h_full().w(relative(s.purgeable as f32 / total)).bg(self.special_color(Special::Purgeable, c)));
            let legend = |color: Hsla, text: String| div().flex().items_center().gap(px(5.)).child(div().size(px(7.)).rounded_full().bg(color)).child(text);
            return footer.child(bar).child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(10.))
                    .text_color(c.muted)
                    .child(legend(c.accent, format!("{} used", sz(used))))
                    .child(legend(self.special_color(Special::Purgeable, c), format!("{} purgeable", sz(s.purgeable))))
                    .child(legend(c.sunken, format!("{} free of {}", sz(s.free), sz(s.capacity)))),
            );
        }
        let restricted = tree.restricted_count();
        let when = format_time_friendly(Some(tree.scanned_at), SystemTime::now());
        footer
            .child(div().text_color(c.muted).child(format!("{} in “{}” · scanned {when}", sz(tree.root().size), tree.root().name)))
            .when(restricted > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_color(c.muted)
                        .child(look::icon(icons::LOCK, 11., c.muted))
                        .child(div().flex_1().child(if restricted == 1 { "1 folder could not be read".to_string() } else { format!("{} folders could not be read", group_digits(restricted as u64)) }))
                        .child(
                            div().id("dm-fda-link").text_color(c.accent).cursor_pointer().child("Full Disk Access…").on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| {
                                let r = platform::open_url(FDA_URL);
                                this.report(r);
                                cx.notify();
                            })),
                        ),
                )
            })
    }

    fn render_collector(&mut self, c: &Colors, cx: &mut Context<Self>) -> Stateful<Div> {
        let tree = self.tree.as_ref().expect("map page has a tree");
        let items: Vec<NodeId> = tree.collected().to_vec();
        let total = tree.collected_bytes();
        let count_text = |n: usize| if n == 1 { "1 item".to_string() } else { format!("{n} items") };
        let busy = self.is_busy();
        let verb = if self.permanent { "deleted permanently" } else { "moved to the Trash" };

        let (title, sub) = if let Some(d) = &self.deletion {
            (format!("Deleting… {} of {}", d.done.load(Ordering::Relaxed), d.count), "Please wait".to_string())
        } else if let Some(t0) = self.countdown {
            let left = COUNTDOWN_SECS.saturating_sub(t0.elapsed().as_secs());
            (format!("Deleting in {left}…"), format!("{} ({}) will be {verb}", count_text(items.len()), sz(total)))
        } else if items.is_empty() {
            ("Drag items here to collect them".to_string(), "Nothing is deleted until you click Delete".to_string())
        } else {
            (format!("{} collected · {}", count_text(items.len()), sz(total)), format!("Click Delete and they will be {verb}"))
        };

        let mut panel = div().flex().flex_col();
        if self.collector_open && !items.is_empty() {
            let mut list = div().id("dm-collected").max_h(px(176.)).overflow_y_scroll().flex().flex_col().py(px(4.)).border_b_1().border_color(c.border.opacity(0.6));
            for (ix, &id) in items.iter().enumerate() {
                let n = tree.node(id);
                let rel = tree.path(id).strip_prefix(tree.root_path()).map(|p| p.display().to_string()).unwrap_or_default();
                let err = self.failed.get(&id).cloned();
                let selected = self.selected == Some(Pick::Node(id));
                list = list.child(
                    div()
                        .id(("dm-col-row", ix))
                        .h(px(30.))
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .when(selected, |d| d.bg(c.accent.opacity(0.2)))
                        .child(look::entry_tile(&tree.path(id), tile_kind(n), 18., c, false, cx))
                        .child(div().flex_none().max_w(px(220.)).whitespace_nowrap().overflow_hidden().text_ellipsis().child(n.name.to_string()))
                        .child(div().flex_1().min_w(px(0.)).text_size(px(11.5)).whitespace_nowrap().overflow_hidden().text_ellipsis().text_color(if err.is_some() { c.danger } else { c.muted }).child(err.unwrap_or(rel)))
                        .child(div().flex_none().text_size(px(12.)).text_color(c.muted).child(sz(n.shown)))
                        .child(
                            look::ghost_button(SharedString::from(format!("dm-uncollect-{ix}")), icons::CLOSE, c, false, !busy)
                                .when(!busy, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
                                    cx.stop_propagation();
                                    this.uncollect(id, cx);
                                }))),
                        )
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                            w.focus(&this.focus);
                            this.selected = Some(Pick::Node(id));
                            cx.notify();
                        })),
                );
            }
            panel = panel.child(list);
        }

        let mode = {
            let seg = |id: &'static str, text: &'static str, on: bool, cx: &mut Context<Self>, permanent: bool| {
                div()
                    .id(id)
                    .px(px(9.))
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .text_size(px(11.5))
                    .when(on, |d| d.bg(c.bg).shadow(look::soft_shadow(c)).font_weight(FontWeight::SEMIBOLD))
                    .when(!on && !busy, |d| d.cursor_pointer().text_color(c.muted).hover(|s| s.bg(c.hover)))
                    .when(!on && busy, |d| d.text_color(c.muted.opacity(0.6)))
                    .child(text)
                    .when(!busy, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _w, cx| {
                        this.permanent = permanent;
                        cx.notify();
                    })))
            };
            div()
                .flex()
                .flex_none()
                .p(px(2.))
                .gap(px(2.))
                .rounded(px(8.))
                .bg(c.sunken)
                .child(seg("dm-mode-trash", "Move to Trash", !self.permanent, cx, false))
                .child(seg("dm-mode-delete", "Delete immediately", self.permanent, cx, true))
        };

        let action = if self.deletion.is_some() {
            div().into_any_element()
        } else if self.countdown.is_some() {
            pill_button("dm-stop", None, "Stop", true, true, c, cx, |this, _w, cx| this.stop_countdown(cx)).into_any_element()
        } else {
            pill_button("dm-delete", Some(icons::TRASH), "Delete", true, !items.is_empty(), c, cx, |this, _w, cx| this.start_countdown(cx)).into_any_element()
        };

        let progress = self.deletion.as_ref().map(|d| d.done.load(Ordering::Relaxed) as f32 / d.count.max(1) as f32).or_else(|| {
            self.countdown.map(|t0| (t0.elapsed().as_secs_f32() / COUNTDOWN_SECS as f32).min(1.0))
        });
        let tile_bg = if self.permanent && !items.is_empty() { c.danger.opacity(0.16) } else { c.accent.opacity(0.14) };
        let tile_fg = if self.permanent && !items.is_empty() { c.danger } else { c.accent };
        let strip = div()
            .h(px(60.))
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().size(px(34.)).rounded(px(10.)).bg(tile_bg).flex().flex_none().items_center().justify_center().child(look::icon(icons::TRASH, 17., tile_fg)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(title))
                    .child(div().text_size(px(11.5)).text_color(c.muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(sub))
                    .when_some(progress, |d, f| {
                        d.child(div().mt(px(2.)).h(px(3.)).w_full().rounded_full().bg(c.sunken).child(div().h_full().rounded_full().w(relative(f)).bg(if self.permanent { c.danger } else { c.accent })))
                    }),
            )
            .child(mode)
            .child(action)
            .when(!items.is_empty(), |d| {
                let open = self.collector_open;
                d.child(
                    look::ghost_button("dm-expand-collector", icons::CHEVRON_DOWN, c, open, true).on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| {
                        cx.stop_propagation();
                        this.collector_open = !this.collector_open;
                        cx.notify();
                    })),
                )
            });
        if self.collector_open && items.len() > 1 && !busy {
            panel = panel.child(
                div().flex().justify_end().px(px(14.)).pt(px(6.)).child(
                    div().id("dm-remove-all").text_size(px(11.5)).text_color(c.accent).cursor_pointer().child("Remove All").on_mouse_down(MouseButton::Left, cx.listener(|this, _, _w, cx| this.uncollect_all(cx))),
                ),
            );
        }

        let (accent, drop_bg) = (c.accent, c.accent.opacity(0.1));
        div()
            .id("dm-collector")
            .flex_none()
            .mx(px(12.))
            .mb(px(12.))
            .rounded(px(14.))
            .overflow_hidden()
            .bg(c.bg.opacity(0.94))
            .border_1()
            .border_color(c.border)
            .shadow(look::soft_shadow(c))
            .drag_over::<DraggedItem>(move |s, _, _, _| s.border_color(accent).bg(drop_bg))
            .on_drop(cx.listener(|this, item: &DraggedItem, _w, cx| {
                this.collect(item.id, cx);
            }))
            .child(panel)
            .child(strip)
    }

    fn render_menu(&mut self, c: &Colors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let pick = menu.pick;
        let mut list = div()
            .id("dm-menu")
            .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                this.menu = None;
                cx.notify();
            }))
            .min_w(px(210.))
            .p(px(5.))
            .rounded(px(12.))
            .bg(c.surface)
            .border_1()
            .border_color(c.border)
            .shadow(look::soft_shadow(c))
            .text_size(px(13.))
            .text_color(c.text);
        for (ix, &(label, cmd, enabled)) in menu.items.iter().enumerate() {
            list = list.child(if cmd == Cmd::Separator {
                div().my(px(4.)).mx(px(8.)).h(px(1.)).bg(c.border).into_any_element()
            } else {
                div()
                    .id(("dm-menu-item", ix))
                    .px(px(10.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .when(!enabled, |d| d.text_color(c.muted.opacity(0.7)))
                    .when(enabled, |d| {
                        d.cursor_pointer().hover(|s| s.bg(c.accent).text_color(c.on_accent)).on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.run_cmd(cmd, pick, w, cx);
                        }))
                    })
                    .child(label)
                    .into_any_element()
            });
        }
        Some(
            deferred(anchored().position(menu.position).snap_to_window_with_margin(px(8.)).child(list.with_animation(
                ("dm-menu-in", menu.id),
                gpui::Animation::new(Duration::from_millis(140)).with_easing(gpui::ease_out_quint()),
                |el, t| el.opacity(t),
            )))
            .with_priority(2)
            .into_any_element(),
        )
    }
}

fn restore_collected(tree: &mut Tree, paths: &[PathBuf]) {
    for p in paths {
        if let Some(id) = tree.find(p) {
            tree.collect(id);
        }
    }
}

/// A rounded text button with an optional leading icon.
#[allow(clippy::too_many_arguments)]
fn pill_button(
    id: &'static str,
    glyph: Option<&'static str>,
    label: &'static str,
    primary: bool,
    enabled: bool,
    c: &Colors,
    cx: &mut Context<DiskMap>,
    f: fn(&mut DiskMap, &mut Window, &mut Context<DiskMap>),
) -> Stateful<Div> {
    let fg = if !enabled { c.muted } else if primary { c.on_accent } else { c.text };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(12.))
        .h(px(28.))
        .rounded(px(8.))
        .border_1()
        .border_color(if primary && enabled { c.accent } else { c.border })
        .bg(if primary && enabled { c.accent } else { c.surface })
        .text_color(fg)
        .text_size(px(12.5))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.opacity(0.88)))
        .when_some(glyph, |d, g| d.child(look::icon(g, 13., fg)))
        .child(label)
        .when(enabled, |d| {
            d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
                cx.stop_propagation();
                f(this, w, cx);
            }))
        })
}

/// One target on Disks and Folders: icon, name, detail, optional capacity gauge
/// (used, purgeable fractions) and a button. Clicking anywhere on it runs `on`.
#[allow(clippy::too_many_arguments)]
fn source_row(
    id: SharedString,
    glyph: &'static str,
    title: String,
    subtitle: String,
    gauge: Option<(f32, f32)>,
    button: &'static str,
    c: &Colors,
    cx: &mut Context<DiskMap>,
    on: impl Fn(&mut DiskMap, &mut Window, &mut Context<DiskMap>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id.clone())
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .rounded(px(12.))
        .bg(c.bg.opacity(0.94))
        .border_1()
        .border_color(c.border)
        .cursor_pointer()
        .hover(|s| s.border_color(c.accent.opacity(0.6)))
        .child(crate::probe::probe(format!("diskmap.{id}")))
        .child(div().size(px(34.)).rounded(px(10.)).bg(c.accent.opacity(0.12)).flex().flex_none().items_center().justify_center().child(look::icon(glyph, 17., c.accent)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(div().font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(title))
                .when_some(gauge, |d, (used, purgeable)| {
                    d.child(
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded_full()
                            .overflow_hidden()
                            .bg(c.sunken)
                            .flex()
                            .child(div().h_full().w(relative(used.clamp(0.0, 1.0))).bg(c.accent))
                            .child(div().h_full().w(relative(purgeable.clamp(0.0, 1.0))).bg(c.accent.opacity(0.4))),
                    )
                })
                .child(div().text_size(px(11.5)).text_color(c.muted).whitespace_nowrap().overflow_hidden().text_ellipsis().child(subtitle)),
        )
        .child(div().flex_none().px(px(12.)).h(px(26.)).flex().items_center().rounded(px(7.)).bg(c.accent).text_color(c.on_accent).text_size(px(12.5)).child(button))
        // Fires on press, like every button in the main window: a click that needs press and
        // release on the same element can get lost.
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| {
            cx.stop_propagation();
            on(this, w, cx);
        }))
}

impl Drop for DiskMap {
    fn drop(&mut self) {
        // Closing the window stops a running scan; a running deletion finishes on its own.
        if let Some(job) = &self.scan {
            job.progress.cancel();
        }
    }
}

impl Render for DiskMap {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::get(window, cx);
        let body = match self.page {
            Page::Start => self.render_start(&c, cx),
            Page::Scanning => self.render_scanning(&c, window, cx),
            Page::Map => self.render_map_page(&c, window, cx),
        };
        let titlebar = self.render_titlebar(&c, cx);
        let menu = self.render_menu(&c, cx);
        let toast = self.toast.as_ref().map(|(text, error, _)| {
            div().absolute().bottom(px(if self.page == Page::Map { 92. } else { 24. })).left_0().w_full().flex().justify_center().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(8.))
                    .max_w(px(640.))
                    .rounded(px(10.))
                    .bg(c.surface)
                    .border_1()
                    .border_color(if *error { c.danger.opacity(0.5) } else { c.border })
                    .shadow(look::soft_shadow(&c))
                    .child(look::icon(if *error { icons::WARNING } else { icons::CHECK }, 14., if *error { c.danger } else { c.success }))
                    .child(text.clone()),
            )
        });
        div()
            .id("diskmap")
            .relative()
            .key_context("DiskMap")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &ui::CloseWindow, w, _| w.remove_window()))
            .on_key_down(cx.listener(Self::key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::linear_gradient(180., gpui::linear_color_stop(c.bg_top, 0.), gpui::linear_color_stop(c.bg_bottom, 1.)))
            .text_color(c.text)
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .child(titlebar)
            .child(body)
            .children(toast)
            .children(menu)
    }
}
