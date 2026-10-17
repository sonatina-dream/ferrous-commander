//! Disk usage analysis for the Disk Map tool (`SPEC-DiskMap.md` §3–§8): a parallel scanner
//! that builds a size tree, the sunburst layout and its hit testing, the sidebar's rows,
//! navigation history, and the collector's bookkeeping. Nothing here draws pixels.

// objc 0.2 macros test a `cargo-clippy` feature this crate does not declare.
#![allow(unexpected_cfgs)]

use std::collections::HashSet;
use std::f32::consts::TAU;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime};

/// Index of a node in its [`Tree`].
pub type NodeId = u32;

/// The scan root is always node 0.
pub const ROOT: NodeId = 0;

/// Rings drawn around the centre disc (§5.1).
pub const MAX_RINGS: usize = 7;

/// Ring geometry never assumes fewer rings than this, so shallow folders don't balloon.
pub const MIN_RINGS: usize = 3;

/// Smallest sweep drawn as its own segment: 0.6°, i.e. 1/600 of the map root (§5.2).
pub const MIN_SWEEP: f32 = 0.6 * std::f32::consts::PI / 180.0;

/// Smallest "smaller objects" or pseudo-item segment worth drawing: 0.1°.
const MIN_GROUP_SWEEP: f32 = 0.1 * std::f32::consts::PI / 180.0;

/// Centre disc radius as a fraction of the outer radius.
const CENTER_FRACTION: f32 = 0.22;

/// Each ring is this much narrower than the one inside it.
const RING_RATIO: f32 = 0.86;

/// Hidden space below this fraction of capacity (0.5%) is not worth a segment (§3.1).
const HIDDEN_MIN_FRACTION: u64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Folder,
    /// A folder macOS shows as one document (`.app`, `.photoslibrary` …).
    Package,
    File,
    Symlink,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: Box<str>,
    pub parent: Option<NodeId>,
    /// Sorted by size, largest first; ties by name.
    pub children: Vec<NodeId>,
    pub kind: NodeKind,
    /// Allocated bytes of this node and everything below it.
    pub size: u64,
    /// `size` minus collected descendants: what the map and list show.
    pub shown: u64,
    /// Files below (1 for a file itself).
    pub items: u64,
    /// The folder could not be listed.
    pub restricted: bool,
    /// A later path to an already counted hard-linked inode (size 0).
    pub hard_link: bool,
    pub collected: bool,
    /// Detached by a deletion or replaced by a rescan.
    pub removed: bool,
}

impl Node {
    fn new(name: impl Into<Box<str>>, parent: Option<NodeId>, kind: NodeKind, size: u64) -> Node {
        let leaf = matches!(kind, NodeKind::File | NodeKind::Symlink);
        Node {
            name: name.into(),
            parent,
            children: Vec::new(),
            kind,
            size,
            shown: size,
            items: u64::from(leaf),
            restricted: false,
            hard_link: false,
            collected: false,
            removed: false,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, NodeKind::Folder | NodeKind::Package)
    }

    /// Whether the map draws rings for this node's children.
    fn descends(&self) -> bool {
        self.kind == NodeKind::Folder && !self.restricted && !self.children.is_empty()
    }
}

/// Space on the scanned volume that no scanned file accounts for (§3.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VolumeSummary {
    pub capacity: u64,
    pub free: u64,
    pub purgeable: u64,
    pub hidden: u64,
}

impl VolumeSummary {
    /// `important_free` is the free space macOS reports for important use, which includes
    /// purgeable space; `scanned` is the size of the scan.
    pub fn compute(capacity: u64, free: u64, important_free: Option<u64>, scanned: u64) -> VolumeSummary {
        let purgeable = important_free.map(|i| i.saturating_sub(free)).unwrap_or(0);
        let used = capacity.saturating_sub(free);
        let hidden = used.saturating_sub(purgeable).saturating_sub(scanned);
        let hidden = if capacity > 0 && hidden.saturating_mul(HIDDEN_MIN_FRACTION) >= capacity { hidden } else { 0 };
        VolumeSummary { capacity, free, purgeable, hidden }
    }

    pub fn used(&self) -> u64 {
        self.capacity.saturating_sub(self.free)
    }

    pub fn amount(&self, s: Special) -> u64 {
        match s {
            Special::Purgeable => self.purgeable,
            Special::Hidden => self.hidden,
        }
    }
}

/// Pseudo-items shown at a volume's root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Special {
    Purgeable,
    Hidden,
}

impl Special {
    pub const ALL: [Special; 2] = [Special::Purgeable, Special::Hidden];

    pub fn label(self) -> &'static str {
        match self {
            Special::Purgeable => "purgeable space",
            Special::Hidden => "hidden space",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Special::Purgeable => {
                "Space macOS can reclaim on its own when it is needed, such as local Time Machine snapshots and caches. Nothing needs to be done about it."
            }
            Special::Hidden => {
                "Space used by other volumes in the same container, system files, snapshots, and folders this app could not read. Giving Ferrous Commander Full Disk Access reveals more of it."
            }
        }
    }
}

/// Why an item cannot go to the collector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    ScanRoot,
    Protected(String),
    Gone,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::ScanRoot => f.write_str("The scanned folder itself can't be collected"),
            Refusal::Protected(name) => write!(f, "“{name}” is needed by macOS or your account and can't be collected"),
            Refusal::Gone => f.write_str("This item no longer exists"),
        }
    }
}

/// The result of a scan.
#[derive(Clone, Debug)]
pub struct Tree {
    root_path: PathBuf,
    nodes: Vec<Node>,
    /// Top-most collected items, in the order they were collected.
    collected: Vec<NodeId>,
    pub summary: Option<VolumeSummary>,
    pub scanned_at: SystemTime,
    version: u64,
}

impl Tree {
    fn from_nodes(root_path: PathBuf, mut nodes: Vec<Node>) -> Tree {
        finish(&mut nodes);
        Tree { root_path, nodes, collected: Vec::new(), summary: None, scanned_at: SystemTime::now(), version: 0 }
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn root(&self) -> &Node {
        self.node(ROOT)
    }

    /// Changes on every mutation, so views can cache layouts.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Number of folders that could not be listed.
    pub fn restricted_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.restricted && !n.removed).count()
    }

    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut at = id;
        while let Some(p) = self.node(at).parent {
            names.push(&*self.node(at).name);
            at = p;
        }
        let mut path = self.root_path.clone();
        for name in names.into_iter().rev() {
            path.push(name);
        }
        path
    }

    /// The node at `path`, if it is part of the tree.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let rel = path.strip_prefix(&self.root_path).ok()?;
        let mut at = ROOT;
        for comp in rel.components() {
            let name = comp.as_os_str().to_str()?;
            at = *self.node(at).children.iter().find(|&&c| &*self.node(c).name == name)?;
        }
        Some(at)
    }

    /// Parent, grandparent … up to the root.
    pub fn ancestors(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.node(id).parent, |&p| self.node(p).parent)
    }

    /// True when `a` is a strict ancestor of `b`.
    pub fn is_ancestor(&self, a: NodeId, b: NodeId) -> bool {
        self.ancestors(b).any(|x| x == a)
    }

    /// Still attached to the tree (neither it nor an ancestor was removed).
    pub fn is_alive(&self, id: NodeId) -> bool {
        (id as usize) < self.nodes.len() && !self.node(id).removed && self.ancestors(id).all(|a| !self.node(a).removed)
    }

    /// Children the map and list show: not collected and not removed.
    pub fn visible_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.node(id).children.iter().copied().filter(|&c| {
            let n = self.node(c);
            !n.collected && !n.removed
        })
    }

    pub fn collected(&self) -> &[NodeId] {
        &self.collected
    }

    pub fn collected_bytes(&self) -> u64 {
        self.collected.iter().map(|&c| self.node(c).shown).sum()
    }

    pub fn can_collect(&self, id: NodeId, home: Option<&Path>) -> Result<(), Refusal> {
        if id == ROOT {
            return Err(Refusal::ScanRoot);
        }
        if !self.is_alive(id) {
            return Err(Refusal::Gone);
        }
        let path = self.path(id);
        if is_protected(&path, home) {
            return Err(Refusal::Protected(self.node(id).name.to_string()));
        }
        Ok(())
    }

    /// Moves `id` to the collector, releasing any collected descendants first.
    /// Returns false if it was already there.
    pub fn collect(&mut self, id: NodeId) -> bool {
        if self.node(id).collected || id == ROOT {
            return false;
        }
        let inside: Vec<NodeId> = self.collected.iter().copied().filter(|&c| self.is_ancestor(id, c)).collect();
        for c in inside {
            self.uncollect(c);
        }
        let shown = self.node(id).shown;
        for a in self.ancestors(id).collect::<Vec<_>>() {
            self.nodes[a as usize].shown -= shown;
        }
        self.nodes[id as usize].collected = true;
        self.collected.push(id);
        self.version += 1;
        true
    }

    /// Puts a collected item back on the map.
    pub fn uncollect(&mut self, id: NodeId) {
        if !self.node(id).collected {
            return;
        }
        let shown = self.node(id).shown;
        for a in self.ancestors(id).collect::<Vec<_>>() {
            self.nodes[a as usize].shown += shown;
        }
        self.nodes[id as usize].collected = false;
        self.collected.retain(|&c| c != id);
        self.version += 1;
    }

    /// Detaches a deleted item and shrinks its ancestors.
    pub fn remove(&mut self, id: NodeId) {
        if id == ROOT || !self.is_alive(id) {
            return;
        }
        let (size, shown, items, collected) = {
            let n = self.node(id);
            (n.size, n.shown, n.items, n.collected)
        };
        for a in self.ancestors(id).collect::<Vec<_>>() {
            let n = &mut self.nodes[a as usize];
            n.size -= size;
            n.items -= items;
            if !collected {
                n.shown -= shown;
            }
        }
        if let Some(p) = self.node(id).parent {
            self.nodes[p as usize].children.retain(|&c| c != id);
        }
        let n = &mut self.nodes[id as usize];
        n.removed = true;
        n.collected = false;
        self.prune_collected();
        self.version += 1;
    }

    /// Replaces the subtree at `at` with a fresh scan of the same folder (§3, ⌘R).
    pub fn graft(&mut self, at: NodeId, sub: Tree) {
        let old = self.node(at).clone();
        for &c in &old.children {
            self.nodes[c as usize].removed = true;
        }
        let offset = self.nodes.len() as NodeId - 1;
        let remap = |id: NodeId| if id == ROOT { at } else { id + offset };
        let mut sub_nodes = sub.nodes.into_iter();
        let new_root = sub_nodes.next().expect("a tree has a root");
        for mut n in sub_nodes {
            n.parent = n.parent.map(remap);
            n.children.iter_mut().for_each(|c| *c = remap(*c));
            self.nodes.push(n);
        }
        {
            let n = &mut self.nodes[at as usize];
            n.children = new_root.children.iter().map(|&c| remap(c)).collect();
            n.size = new_root.size;
            n.shown = new_root.size;
            n.items = new_root.items;
            n.restricted = new_root.restricted;
        }
        let d_size = new_root.size as i128 - old.size as i128;
        let d_items = new_root.items as i128 - old.items as i128;
        let d_shown = if old.collected { 0 } else { new_root.size as i128 - old.shown as i128 };
        let ancestors: Vec<NodeId> = self.ancestors(at).collect();
        for &a in &ancestors {
            let n = &mut self.nodes[a as usize];
            n.size = (n.size as i128 + d_size).max(0) as u64;
            n.items = (n.items as i128 + d_items).max(0) as u64;
            n.shown = (n.shown as i128 + d_shown).max(0) as u64;
        }
        for &a in &ancestors {
            self.sort_children_of(a);
        }
        self.prune_collected();
        self.version += 1;
    }

    fn prune_collected(&mut self) {
        let alive: Vec<NodeId> = self.collected.iter().copied().filter(|&c| self.is_alive(c)).collect();
        self.collected = alive;
    }

    fn sort_children_of(&mut self, id: NodeId) {
        let mut children = std::mem::take(&mut self.nodes[id as usize].children);
        sort_ids(&self.nodes, &mut children);
        self.nodes[id as usize].children = children;
    }
}

fn sort_ids(nodes: &[Node], ids: &mut [NodeId]) {
    ids.sort_by(|&a, &b| {
        let (a, b) = (&nodes[a as usize], &nodes[b as usize]);
        b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
    });
}

/// Sums sizes and item counts bottom-up and sorts children. Children always have larger
/// ids than their parent, so one reverse pass is enough.
fn finish(nodes: &mut [Node]) {
    for id in (1..nodes.len()).rev() {
        if let Some(p) = nodes[id].parent {
            let (size, items) = (nodes[id].size, nodes[id].items);
            nodes[p as usize].size += size;
            nodes[p as usize].items += items;
        }
    }
    for id in 0..nodes.len() {
        nodes[id].shown = nodes[id].size;
        if nodes[id].children.len() > 1 {
            let mut children = std::mem::take(&mut nodes[id].children);
            sort_ids(nodes, &mut children);
            nodes[id].children = children;
        }
    }
}

/// Paths the collector refuses (§8): system folders, `/Users`, the home folder, its
/// Library, and every folder that contains the home folder.
pub fn is_protected(path: &Path, home: Option<&Path>) -> bool {
    const SYSTEM: &[&str] = &[
        "/", "/System", "/Library", "/Applications", "/Users", "/usr", "/bin", "/sbin", "/private", "/private/var",
        "/private/etc", "/private/tmp", "/etc", "/var", "/tmp", "/Volumes", "/opt", "/cores",
    ];
    if SYSTEM.iter().any(|s| path == Path::new(s)) {
        return true;
    }
    match home {
        Some(h) => path == h.join("Library") || h.starts_with(path),
        None => false,
    }
}

// ----- scanning ---------------------------------------------------------------------------

/// Live counters of a running scan; also how to cancel it.
#[derive(Debug, Default)]
pub struct ScanProgress {
    pub items: AtomicU64,
    pub bytes: AtomicU64,
    cancelled: AtomicBool,
    current: Mutex<PathBuf>,
}

impl ScanProgress {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The folder being read right now.
    pub fn current(&self) -> PathBuf {
        self.current.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

#[derive(Debug)]
pub enum ScanError {
    Cancelled,
    NotADirectory(PathBuf),
    Io(PathBuf, io::Error),
}

impl ScanError {
    /// The folder exists but this process may not read it (Full Disk Access, permissions).
    pub fn is_permission(&self) -> bool {
        matches!(self, ScanError::Io(_, e) if e.kind() == io::ErrorKind::PermissionDenied)
    }
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Cancelled => f.write_str("The scan was cancelled"),
            ScanError::NotADirectory(p) => write!(f, "{} is not a folder", p.display()),
            ScanError::Io(_, e) if e.kind() == io::ErrorKind::PermissionDenied => {
                f.write_str("Permission denied. The folder may need Full Disk Access.")
            }
            ScanError::Io(_, e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ScanError {}

struct Queue {
    jobs: Vec<(NodeId, PathBuf)>,
    /// Folders queued or being read.
    pending: usize,
}

struct Shared<'a> {
    nodes: Mutex<Vec<Node>>,
    queue: Mutex<Queue>,
    ready: Condvar,
    links: Mutex<HashSet<(u64, u64)>>,
    mounts: HashSet<PathBuf>,
    progress: &'a ScanProgress,
}

struct Child {
    name: String,
    path: PathBuf,
    kind: NodeKind,
    size: u64,
    hard_link: bool,
}

/// Scans the folder at `root` (§3). Runs on the calling thread plus helpers, one per
/// core (at most 8); read `progress` from elsewhere while it runs.
pub fn scan(root: &Path, progress: &ScanProgress) -> Result<Tree, ScanError> {
    let root = fs::canonicalize(root).map_err(|e| ScanError::Io(root.to_path_buf(), e))?;
    let meta = fs::symlink_metadata(&root).map_err(|e| ScanError::Io(root.clone(), e))?;
    if !meta.is_dir() {
        return Err(ScanError::NotADirectory(root));
    }
    fs::read_dir(&root).map_err(|e| ScanError::Io(root.clone(), e))?;

    let name = display_name(&root);
    let kind = if crate::entry::is_package_dir(&name) { NodeKind::Package } else { NodeKind::Folder };
    let shared = Shared {
        nodes: Mutex::new(vec![Node::new(name, None, kind, allocated(&meta))]),
        queue: Mutex::new(Queue { jobs: vec![(ROOT, root.clone())], pending: 1 }),
        ready: Condvar::new(),
        links: Mutex::new(HashSet::new()),
        mounts: mount_points().into_iter().filter(|m| *m != root).collect(),
        progress,
    };
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 1..workers {
            s.spawn(|| work(&shared));
        }
        work(&shared);
    });
    if progress.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    let nodes = shared.nodes.into_inner().unwrap_or_else(|e| e.into_inner());
    let mut tree = Tree::from_nodes(root, nodes);
    tree.summary = volume_summary(&tree.root_path, tree.root().size);
    Ok(tree)
}

fn work(sh: &Shared) {
    loop {
        let (id, path) = {
            let mut q = sh.queue.lock().unwrap();
            loop {
                if sh.progress.is_cancelled() {
                    sh.ready.notify_all();
                    return;
                }
                if let Some(job) = q.jobs.pop() {
                    break job;
                }
                if q.pending == 0 {
                    return;
                }
                q = sh.ready.wait_timeout(q, Duration::from_millis(50)).unwrap().0;
            }
        };
        if let Ok(mut current) = sh.progress.current.try_lock() {
            current.clone_from(&path);
        }
        let listing = list_dir(sh, &path);
        let mut dirs = Vec::new();
        {
            let mut nodes = sh.nodes.lock().unwrap();
            match listing {
                Err(_) => nodes[id as usize].restricted = true,
                Ok(children) => {
                    for c in children {
                        let cid = nodes.len() as NodeId;
                        let mut node = Node::new(c.name, Some(id), c.kind, c.size);
                        node.hard_link = c.hard_link;
                        nodes.push(node);
                        nodes[id as usize].children.push(cid);
                        if matches!(c.kind, NodeKind::Folder | NodeKind::Package) {
                            dirs.push((cid, c.path));
                        }
                    }
                }
            }
        }
        let mut q = sh.queue.lock().unwrap();
        q.pending += dirs.len();
        q.pending -= 1;
        q.jobs.extend(dirs);
        sh.ready.notify_all();
    }
}

fn list_dir(sh: &Shared, path: &Path) -> io::Result<Vec<Child>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(path)? {
        let Ok(entry) = entry else { continue };
        // DirEntry::metadata does not follow symlinks.
        let Ok(meta) = entry.metadata() else { continue };
        let ft = meta.file_type();
        let child_path = entry.path();
        if ft.is_dir() && sh.mounts.contains(&child_path) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut size = allocated(&meta);
        let mut hard_link = false;
        if !ft.is_dir() && meta.nlink() > 1 && !sh.links.lock().unwrap().insert((meta.dev(), meta.ino())) {
            size = 0;
            hard_link = true;
        }
        let kind = if ft.is_dir() {
            if crate::entry::is_package_dir(&name) { NodeKind::Package } else { NodeKind::Folder }
        } else if ft.is_symlink() {
            NodeKind::Symlink
        } else {
            NodeKind::File
        };
        sh.progress.items.fetch_add(1, Ordering::Relaxed);
        sh.progress.bytes.fetch_add(size, Ordering::Relaxed);
        out.push(Child { name, path: child_path, kind, size, hard_link });
    }
    Ok(out)
}

/// Bytes allocated on disk ("size on disk").
fn allocated(meta: &fs::Metadata) -> u64 {
    meta.blocks() * 512
}

fn display_name(path: &Path) -> String {
    if path == Path::new("/") {
        return crate::volumes::list_volumes().into_iter().find(|v| v.is_root).map(|v| v.name).unwrap_or_else(|| "/".into());
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Every mount point in the system mount table.
#[cfg(target_os = "macos")]
pub fn mount_points() -> Vec<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    let mut buf: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo points `buf` at a libc-owned array of `n` entries.
    let n = unsafe { libc::getmntinfo(&mut buf, libc::MNT_NOWAIT) };
    if n <= 0 || buf.is_null() {
        return Vec::new();
    }
    // SAFETY: as above; the array stays valid until the next getmntinfo call on this thread.
    let list = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    list.iter()
        .map(|st| {
            // SAFETY: f_mntonname is NUL-terminated by the kernel.
            let name = unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) };
            PathBuf::from(OsStr::from_bytes(name.to_bytes()))
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
pub fn mount_points() -> Vec<PathBuf> {
    Vec::new()
}

/// Volume figures for a scan of a volume's mount point; `None` for ordinary folders.
pub fn volume_summary(root: &Path, scanned: u64) -> Option<VolumeSummary> {
    let (free, capacity, mount) = crate::volumes::disk_space(root)?;
    (mount == root).then(|| VolumeSummary::compute(capacity, free, important_free(root), scanned))
}

/// Free space for important use, which counts purgeable space as free.
#[cfg(target_os = "macos")]
fn important_free(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use objc::runtime::{Object, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {
        static NSURLVolumeAvailableCapacityForImportantUsageKey: *mut Object;
    }

    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: autoreleased Foundation objects inside a local pool; out-parameters are
    // plain object pointers that stay valid until the pool drains.
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let s: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()];
        let url: *mut Object = msg_send![class!(NSURL), fileURLWithPath: s];
        let mut value: *mut Object = std::ptr::null_mut();
        let mut error: *mut Object = std::ptr::null_mut();
        let ok: BOOL = msg_send![url, getResourceValue: &mut value forKey: NSURLVolumeAvailableCapacityForImportantUsageKey error: &mut error];
        let out = if ok == YES && !value.is_null() {
            let n: i64 = msg_send![value, longLongValue];
            u64::try_from(n).ok().filter(|&n| n > 0)
        } else {
            None
        };
        let _: () = msg_send![pool, drain];
        out
    }
}

#[cfg(not(target_os = "macos"))]
fn important_free(_path: &Path) -> Option<u64> {
    None
}

// ----- sunburst ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Node(NodeId),
    /// The children of `parent` too small to draw on their own (§5.2).
    Smaller { parent: NodeId, count: u32, bytes: u64 },
    Special(Special),
}

/// One ring segment. Angles are radians clockwise from 12 o'clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub kind: SegmentKind,
    /// Ring number, 1 = innermost.
    pub depth: u8,
    pub start: f32,
    pub sweep: f32,
}

impl Segment {
    pub fn node(&self) -> Option<NodeId> {
        match self.kind {
            SegmentKind::Node(id) => Some(id),
            _ => None,
        }
    }

    pub fn mid(&self) -> f32 {
        self.start + self.sweep / 2.0
    }

    pub fn contains_angle(&self, a: f32) -> bool {
        a >= self.start && a < self.start + self.sweep
    }
}

/// Bytes the ring-1 circle stands for when `root` is the map root: its shown size, plus
/// the volume's purgeable and hidden space at the scan root.
fn circle_bytes(tree: &Tree, root: NodeId) -> u64 {
    let extra = match (root, tree.summary) {
        (ROOT, Some(s)) => s.purgeable + s.hidden,
        _ => 0,
    };
    tree.node(root).shown + extra
}

/// Segments of the sunburst centred on `root` (§5.1, §5.2), in pre-order.
pub fn layout(tree: &Tree, root: NodeId) -> Vec<Segment> {
    let total = circle_bytes(tree, root);
    let mut out = Vec::new();
    if total == 0 {
        return out;
    }
    let span = (TAU as f64 * tree.node(root).shown as f64 / total as f64) as f32;
    place(tree, root, 0.0, span, 1, &mut out);
    if root == ROOT
        && let Some(s) = tree.summary
    {
        let mut at = span;
        for sp in Special::ALL {
            let bytes = s.amount(sp);
            if bytes == 0 {
                continue;
            }
            let sweep = (TAU as f64 * bytes as f64 / total as f64) as f32;
            if sweep >= MIN_GROUP_SWEEP {
                out.push(Segment { kind: SegmentKind::Special(sp), depth: 1, start: at, sweep });
            }
            at += sweep;
        }
    }
    out
}

fn place(tree: &Tree, parent: NodeId, start: f32, sweep: f32, depth: usize, out: &mut Vec<Segment>) {
    if depth > MAX_RINGS {
        return;
    }
    let p = tree.node(parent);
    if p.shown == 0 {
        return;
    }
    let mut at = start;
    let (mut count, mut bytes, mut small_sweep) = (0u32, 0u64, 0f32);
    for c in tree.visible_children(parent) {
        let n = tree.node(c);
        let sw = (sweep as f64 * n.shown as f64 / p.shown as f64) as f32;
        if sw >= MIN_SWEEP {
            out.push(Segment { kind: SegmentKind::Node(c), depth: depth as u8, start: at, sweep: sw });
            if n.descends() {
                place(tree, c, at, sw, depth + 1, out);
            }
            at += sw;
        } else if n.shown > 0 {
            count += 1;
            bytes += n.shown;
            small_sweep += sw;
        }
    }
    if count > 0 && small_sweep >= MIN_GROUP_SWEEP {
        out.push(Segment { kind: SegmentKind::Smaller { parent, count, bytes }, depth: depth as u8, start: at, sweep: small_sweep });
    }
}

/// How many rings the geometry is laid out for: the deepest ring in use, within
/// [`MIN_RINGS`, `MAX_RINGS`] (§5.1).
pub fn ring_count(segments: &[Segment]) -> usize {
    segments.iter().map(|s| s.depth as usize).max().unwrap_or(1).clamp(MIN_RINGS, MAX_RINGS)
}

/// Radius of the centre disc and the inner and outer radius of each of `rings` rings,
/// together filling `outer`.
pub fn ring_radii(outer: f32, rings: usize) -> (f32, Vec<(f32, f32)>) {
    let rings = rings.clamp(1, MAX_RINGS);
    let center = outer * CENTER_FRACTION;
    let span = outer - center;
    let first = span * (1.0 - RING_RATIO) / (1.0 - RING_RATIO.powi(rings as i32));
    let mut out = Vec::with_capacity(rings);
    let (mut r, mut w) = (center, first);
    for _ in 0..rings {
        out.push((r, r + w));
        r += w;
        w *= RING_RATIO;
    }
    out[rings - 1].1 = outer;
    (center, out)
}

/// Angle of a point relative to the centre, clockwise from 12 o'clock, in `[0, 2π)`.
/// Screen coordinates: `dy` grows downwards.
pub fn angle_of(dx: f32, dy: f32) -> f32 {
    let a = dx.atan2(-dy);
    if a < 0.0 { a + TAU } else { a }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Center,
    Segment(usize),
    Nothing,
}

/// What lies under a point `(dx, dy)` from the centre of a map of radius `outer`.
pub fn hit_test(segments: &[Segment], outer: f32, dx: f32, dy: f32) -> Hit {
    let r = dx.hypot(dy);
    let (center, rings) = ring_radii(outer, ring_count(segments));
    if r <= center {
        return Hit::Center;
    }
    let Some(depth) = rings.iter().position(|&(a, b)| r >= a && r < b) else { return Hit::Nothing };
    let angle = angle_of(dx, dy);
    segments
        .iter()
        .position(|s| s.depth as usize == depth + 1 && s.contains_angle(angle))
        .map_or(Hit::Nothing, Hit::Segment)
}

// ----- sidebar ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Node(NodeId),
    Smaller { count: u32, bytes: u64 },
    Special(Special, u64),
}

/// The sidebar rows for `folder` (§6): children in size order, the ones too small for the
/// map grouped into "smaller objects" unless `expanded`, then the volume's pseudo-items.
pub fn rows(tree: &Tree, folder: NodeId, expanded: bool) -> Vec<Row> {
    let total = circle_bytes(tree, folder);
    let mut out = Vec::new();
    let mut small = Vec::new();
    for c in tree.visible_children(folder) {
        let shown = tree.node(c).shown;
        // Same rule as the map's ring 1: under 1/600 of the circle is "small".
        if expanded || (shown as u128) * 600 >= total as u128 && total > 0 {
            out.push(Row::Node(c));
        } else {
            small.push(c);
        }
    }
    match small.len() {
        0 => {}
        1 => out.push(Row::Node(small[0])),
        n => out.push(Row::Smaller { count: n as u32, bytes: small.iter().map(|&c| tree.node(c).shown).sum() }),
    }
    if folder == ROOT
        && let Some(s) = tree.summary
    {
        for sp in Special::ALL {
            if s.amount(sp) > 0 {
                out.push(Row::Special(sp, s.amount(sp)));
            }
        }
    }
    out
}

// ----- navigation ---------------------------------------------------------------------------

/// Back/forward history of opened folders (§7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    back: Vec<NodeId>,
    forward: Vec<NodeId>,
    current: NodeId,
}

impl Default for History {
    fn default() -> Self {
        History::new(ROOT)
    }
}

impl History {
    pub fn new(at: NodeId) -> History {
        History { back: Vec::new(), forward: Vec::new(), current: at }
    }

    pub fn current(&self) -> NodeId {
        self.current
    }

    pub fn can_go_back(&self, tree: &Tree) -> bool {
        self.back.iter().any(|&id| tree.is_alive(id))
    }

    pub fn can_go_forward(&self, tree: &Tree) -> bool {
        self.forward.iter().any(|&id| tree.is_alive(id))
    }

    pub fn open(&mut self, id: NodeId) {
        if id != self.current {
            self.back.push(self.current);
            self.forward.clear();
            self.current = id;
        }
    }

    pub fn go_back(&mut self, tree: &Tree) -> bool {
        while let Some(id) = self.back.pop() {
            if tree.is_alive(id) && id != self.current {
                self.forward.push(self.current);
                self.current = id;
                return true;
            }
        }
        false
    }

    pub fn go_forward(&mut self, tree: &Tree) -> bool {
        while let Some(id) = self.forward.pop() {
            if tree.is_alive(id) && id != self.current {
                self.back.push(self.current);
                self.current = id;
                return true;
            }
        }
        false
    }

    /// Drops deleted folders; if the current one is gone, moves to its nearest survivor.
    pub fn prune(&mut self, tree: &Tree) {
        self.back.retain(|&id| tree.is_alive(id));
        self.forward.retain(|&id| tree.is_alive(id));
        if !tree.is_alive(self.current) {
            self.current = std::iter::once(self.current).chain(tree.ancestors(self.current)).find(|&a| tree.is_alive(a)).unwrap_or(ROOT);
        }
    }

    /// The same history in `new`, matched by path (after a full rescan).
    pub fn remap(&self, old: &Tree, new: &Tree) -> History {
        let map = |id: NodeId| new.find(&old.path(id));
        History {
            back: self.back.iter().filter_map(|&id| map(id)).collect(),
            forward: self.forward.iter().filter_map(|&id| map(id)).collect(),
            current: map(self.current).unwrap_or(ROOT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Builds a tree from `(relative path, own size)` pairs; folders are implied.
    fn synthetic(spec: &[(&str, u64)]) -> Tree {
        let mut nodes = vec![Node::new("root", None, NodeKind::Folder, 0)];
        for (path, size) in spec {
            let parts: Vec<&str> = path.split('/').collect();
            let mut at = ROOT;
            for (i, part) in parts.iter().enumerate() {
                let leaf = i == parts.len() - 1;
                let existing = nodes[at as usize].children.iter().copied().find(|&c| &*nodes[c as usize].name == *part);
                at = match existing {
                    Some(c) => c,
                    None => {
                        let id = nodes.len() as NodeId;
                        let kind = if !leaf || part.ends_with('/') { NodeKind::Folder } else { NodeKind::File };
                        nodes.push(Node::new(*part, Some(at), kind, if leaf { *size } else { 0 }));
                        nodes[at as usize].children.push(id);
                        id
                    }
                };
            }
        }
        Tree::from_nodes(PathBuf::from("/data"), nodes)
    }

    fn id(t: &Tree, rel: &str) -> NodeId {
        t.find(&Path::new("/data").join(rel)).unwrap_or_else(|| panic!("no {rel}"))
    }

    fn blocks(p: &Path) -> u64 {
        fs::symlink_metadata(p).unwrap().blocks() * 512
    }

    fn write(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn scan_sums_allocated_sizes_and_sorts_by_size() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("big.bin"), 300_000);
        write(&root.join("docs/a.txt"), 20_000);
        write(&root.join("docs/deep/b.txt"), 90_000);
        write(&root.join("small.txt"), 10);
        let t = scan(root, &ScanProgress::default()).unwrap();

        let r = t.root();
        let own = blocks(&fs::canonicalize(root).unwrap());
        let children: u64 = r.children.iter().map(|&c| t.node(c).size).sum();
        assert_eq!(r.size, own + children);
        assert_eq!(r.items, 4);
        let names: Vec<&str> = r.children.iter().map(|&c| &*t.node(c).name).collect();
        assert_eq!(names, ["big.bin", "docs", "small.txt"]);
        let docs = t.find(&t.root_path().join("docs")).unwrap();
        assert_eq!(t.node(docs).size, blocks(&root.join("docs")) + blocks(&root.join("docs/a.txt")) + blocks(&root.join("docs/deep")) + blocks(&root.join("docs/deep/b.txt")));
        assert_eq!(t.path(docs), t.root_path().join("docs"));
        assert_eq!(t.node(t.find(&t.root_path().join("big.bin")).unwrap()).size, blocks(&root.join("big.bin")));
        assert!(t.summary.is_none(), "a temp folder is not a volume root");
    }

    #[test]
    fn hard_links_count_once_and_symlinks_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("huge.bin"), 500_000);
        let root = dir.path();
        write(&root.join("a.bin"), 100_000);
        fs::hard_link(root.join("a.bin"), root.join("b.bin")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        let t = scan(root, &ScanProgress::default()).unwrap();

        let a = t.node(t.find(&t.root_path().join("a.bin")).unwrap());
        let b = t.node(t.find(&t.root_path().join("b.bin")).unwrap());
        assert_eq!(a.size + b.size, blocks(&root.join("a.bin")), "one inode, counted once");
        assert!(a.hard_link ^ b.hard_link);
        let link = t.node(t.find(&t.root_path().join("link")).unwrap());
        assert_eq!(link.kind, NodeKind::Symlink);
        assert!(link.children.is_empty());
        assert!(t.root().size < 500_000);
    }

    #[test]
    fn unreadable_folders_are_kept_and_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("locked/secret.bin"), 50_000);
        write(&root.join("open.txt"), 100);
        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        let result = scan(root, &ScanProgress::default());
        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        let t = result.unwrap();
        let locked = t.node(t.find(&t.root_path().join("locked")).unwrap());
        // Root can read anything; only check the flag when the listing really failed.
        if locked.children.is_empty() {
            assert!(locked.restricted);
            assert_eq!(t.restricted_count(), 1);
        }
        assert!(t.find(&t.root_path().join("open.txt")).is_some());
    }

    #[test]
    fn scans_can_be_cancelled_and_reject_files() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("x/y.txt"), 10);
        let p = ScanProgress::default();
        p.cancel();
        assert!(matches!(scan(dir.path(), &p), Err(ScanError::Cancelled)));
        assert!(matches!(scan(&dir.path().join("x/y.txt"), &ScanProgress::default()), Err(ScanError::NotADirectory(_))));
        assert!(matches!(scan(&dir.path().join("missing"), &ScanProgress::default()), Err(ScanError::Io(..))));
    }

    #[test]
    fn layout_is_proportional_nested_and_depth_capped() {
        let t = synthetic(&[("a/x", 300), ("a/y", 100), ("b", 400), ("c/d/e/f/g/h/i/j/k", 200)]);
        let segs = layout(&t, ROOT);
        let ring1: Vec<&Segment> = segs.iter().filter(|s| s.depth == 1).collect();
        let total: f32 = ring1.iter().map(|s| s.sweep).sum();
        assert!((total - TAU).abs() < 1e-4, "{total}");
        let seg = |rel: &str| *segs.iter().find(|s| s.node() == Some(id(&t, rel))).unwrap();
        let (a, b) = (seg("a"), seg("b"));
        assert!((a.sweep - b.sweep).abs() < 1e-5, "a and b are both 400 bytes");
        assert_eq!(a.start, 0.0, "the largest-first order puts a (400, before b by name) at 12 o'clock");
        let (x, y) = (seg("a/x"), seg("a/y"));
        assert_eq!(x.depth, 2);
        assert!((x.sweep - 3.0 * y.sweep).abs() < 1e-4);
        assert!(x.start >= a.start && y.start + y.sweep <= a.start + a.sweep + 1e-5);
        assert_eq!(segs.iter().map(|s| s.depth).max(), Some(MAX_RINGS as u8));
        assert_eq!(ring_count(&segs), MAX_RINGS);
    }

    #[test]
    fn tiny_children_merge_into_one_smaller_objects_segment() {
        let mut spec = vec![("big", 10_000u64)];
        let names: Vec<String> = (0..20).map(|i| format!("f/s{i}")).collect();
        spec.push(("f/large", 5_000));
        for n in &names {
            spec.push((n.as_str(), 2)); // 2/15_040 of the circle, well under 1/600
        }
        let t = synthetic(&spec);
        let segs = layout(&t, ROOT);
        let f = *segs.iter().find(|s| s.node() == Some(id(&t, "f"))).unwrap();
        let large = *segs.iter().find(|s| s.node() == Some(id(&t, "f/large"))).unwrap();
        let smaller: Vec<&Segment> = segs.iter().filter(|s| matches!(s.kind, SegmentKind::Smaller { .. })).collect();
        assert_eq!(smaller.len(), 1);
        let s = smaller[0];
        assert_eq!(s.kind, SegmentKind::Smaller { parent: id(&t, "f"), count: 20, bytes: 40 });
        assert_eq!(s.depth, 2);
        assert!((s.start - (large.start + large.sweep)).abs() < 1e-5, "after the visible siblings");
        assert!(s.start + s.sweep <= f.start + f.sweep + 1e-4);

        let rows = rows(&t, id(&t, "f"), false);
        assert_eq!(rows, [Row::Node(id(&t, "f/large")), Row::Smaller { count: 20, bytes: 40 }]);
        assert_eq!(super::rows(&t, id(&t, "f"), true).len(), 21);
    }

    #[test]
    fn a_single_small_child_is_listed_by_itself() {
        let t = synthetic(&[("big", 10_000), ("tiny", 1)]);
        assert_eq!(rows(&t, ROOT, false), [Row::Node(id(&t, "big")), Row::Node(id(&t, "tiny"))]);
    }

    #[test]
    fn hit_testing_finds_centre_segments_and_gaps() {
        let t = synthetic(&[("a", 100), ("b", 100)]);
        let segs = layout(&t, ROOT);
        let outer = 200.0;
        assert_eq!(ring_count(&segs), MIN_RINGS, "two files: one ring used, three laid out");
        let (center, rings) = ring_radii(outer, ring_count(&segs));
        assert_eq!(rings.len(), MIN_RINGS);
        assert!((rings[MIN_RINGS - 1].1 - outer).abs() < 1e-3);
        assert!(rings.windows(2).all(|w| w[0].1 == w[1].0 && (w[1].1 - w[1].0) < (w[0].1 - w[0].0)));
        assert_eq!(hit_test(&segs, outer, 1.0, 1.0), Hit::Center);
        let mid1 = (rings[0].0 + rings[0].1) / 2.0;
        // a covers 12 → 6 o'clock (right half), b the left half.
        let right = hit_test(&segs, outer, mid1, 0.0);
        assert_eq!(right, Hit::Segment(segs.iter().position(|s| s.node() == Some(id(&t, "a"))).unwrap()));
        let left = hit_test(&segs, outer, -mid1, 0.0);
        assert_eq!(left, Hit::Segment(segs.iter().position(|s| s.node() == Some(id(&t, "b"))).unwrap()));
        let mid2 = (rings[1].0 + rings[1].1) / 2.0;
        assert_eq!(hit_test(&segs, outer, mid2, 0.0), Hit::Nothing, "a is a file: no ring 2");
        assert_eq!(hit_test(&segs, outer, outer + 5.0, 0.0), Hit::Nothing);
        assert!((angle_of(0.0, -1.0)).abs() < 1e-6);
        assert!((angle_of(1.0, 0.0) - TAU / 4.0).abs() < 1e-6);
        assert!((angle_of(-1.0, 0.0) - 3.0 * TAU / 4.0).abs() < 1e-6);
        assert!((center - outer * CENTER_FRACTION).abs() < 1e-4);
    }

    #[test]
    fn collecting_and_removing_keep_sizes_consistent() {
        let mut t = synthetic(&[("p/q/x", 100), ("p/q/y", 50), ("p/z", 25), ("w", 10)]);
        let (p, q, x) = (id(&t, "p"), id(&t, "p/q"), id(&t, "p/q/x"));
        assert!(t.collect(x));
        assert_eq!((t.node(q).shown, t.node(p).shown, t.root().shown), (50, 75, 85));
        assert_eq!(t.root().size, 185);
        assert_eq!(t.visible_children(q).count(), 1);
        assert_eq!(t.collected_bytes(), 100);

        // Collecting the parent releases the child: the collector only holds top-most items.
        assert!(t.collect(q));
        assert_eq!(t.collected(), [q]);
        assert!(!t.node(x).collected);
        assert_eq!((t.node(q).shown, t.node(p).shown, t.root().shown), (150, 25, 35));
        t.uncollect(q);
        assert_eq!((t.node(p).shown, t.root().shown), (175, 185));

        t.collect(x);
        t.remove(x);
        assert!(t.collected().is_empty());
        assert_eq!((t.node(q).size, t.node(q).shown, t.node(p).size, t.root().size, t.root().shown), (50, 50, 75, 85, 85));
        assert_eq!(t.root().items, 3);
        assert!(!t.is_alive(x));
        assert!(t.find(Path::new("/data/p/q/x")).is_none());

        t.remove(q);
        assert_eq!((t.node(p).size, t.root().size), (25, 35));
    }

    #[test]
    fn the_collector_refuses_roots_and_protected_paths() {
        let t = synthetic(&[("p/x", 1)]);
        let home = Path::new("/Users/someone");
        assert_eq!(t.can_collect(ROOT, Some(home)), Err(Refusal::ScanRoot));
        assert_eq!(t.can_collect(id(&t, "p/x"), Some(home)), Ok(()));
        for p in ["/", "/System", "/Library", "/Users", "/Users/someone", "/Users/someone/Library", "/private/var", "/Applications"] {
            assert!(is_protected(Path::new(p), Some(home)), "{p}");
        }
        for p in ["/Users/someone/Downloads", "/Users/someone/Library/Caches", "/Applications/Some.app", "/opt/homebrew"] {
            assert!(!is_protected(Path::new(p), Some(home)), "{p}");
        }
    }

    #[test]
    fn history_skips_deleted_folders() {
        let mut t = synthetic(&[("a/b/c/f", 1), ("d/f", 1)]);
        let (a, b, c, d) = (id(&t, "a"), id(&t, "a/b"), id(&t, "a/b/c"), id(&t, "d"));
        let mut h = History::default();
        h.open(a);
        h.open(b);
        h.open(c);
        assert!(h.go_back(&t));
        assert_eq!(h.current(), b);
        h.open(d);
        assert!(!h.can_go_forward(&t), "opening a folder clears forward");
        t.remove(b);
        assert!(h.go_back(&t));
        assert_eq!(h.current(), a, "b was deleted and is skipped");
        assert!(h.go_back(&t));
        assert_eq!(h.current(), ROOT);
        let mut h2 = History::new(c);
        h2.prune(&t);
        assert_eq!(h2.current(), a, "nearest surviving ancestor");
    }

    #[test]
    fn rescanning_a_subtree_grafts_it_and_fixes_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("keep.bin"), 40_000);
        write(&root.join("sub/one.bin"), 30_000);
        let mut t = scan(root, &ScanProgress::default()).unwrap();
        let sub = t.find(&t.root_path().join("sub")).unwrap();
        let before = t.root().size;
        let mut h = History::new(sub);

        write(&root.join("sub/two.bin"), 200_000);
        let fresh = scan(&t.path(sub), &ScanProgress::default()).unwrap();
        t.graft(sub, fresh);
        let added = blocks(&root.join("sub/two.bin"));
        assert_eq!(t.root().size, before + added);
        assert_eq!(t.root().shown, t.root().size);
        assert_eq!(t.root().items, 3);
        let two = t.find(&t.root_path().join("sub/two.bin")).unwrap();
        assert_eq!(t.path(two), t.root_path().join("sub/two.bin"));
        assert_eq!(t.node(t.root().children[0]).name.as_ref(), "sub", "re-sorted: sub is now the largest");
        h.prune(&t);
        assert_eq!(h.current(), sub);
        assert_eq!(t.layout_check(), Ok(()));
    }

    impl Tree {
        /// Every alive node's size equals its own size plus its alive children's sizes.
        fn layout_check(&self) -> Result<(), String> {
            for (i, n) in self.nodes.iter().enumerate() {
                if !self.is_alive(i as NodeId) || n.children.is_empty() {
                    continue;
                }
                let kids: u64 = n.children.iter().map(|&c| self.node(c).size).sum();
                if kids > n.size {
                    return Err(format!("{} smaller than its children", n.name));
                }
            }
            Ok(())
        }
    }

    #[test]
    fn volume_summary_splits_used_space() {
        let s = VolumeSummary::compute(1000, 200, Some(300), 600);
        assert_eq!((s.purgeable, s.hidden, s.used()), (100, 100, 800));
        let s = VolumeSummary::compute(1000, 200, None, 799);
        assert_eq!(s.hidden, 0, "1 byte is under 0.5% of capacity");
        let s = VolumeSummary::compute(1000, 200, None, 900);
        assert_eq!(s.hidden, 0, "never negative");
    }

    #[test]
    fn volume_root_gets_pseudo_items_after_real_children() {
        let mut t = synthetic(&[("a", 600)]);
        t.summary = Some(VolumeSummary { capacity: 1000, free: 200, purgeable: 100, hidden: 100 });
        let segs = layout(&t, ROOT);
        let kinds: Vec<SegmentKind> = segs.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [SegmentKind::Node(id(&t, "a")), SegmentKind::Special(Special::Purgeable), SegmentKind::Special(Special::Hidden)]);
        assert!((segs[0].sweep - TAU * 0.75).abs() < 1e-4);
        assert_eq!(rows(&t, ROOT, false), [Row::Node(id(&t, "a")), Row::Special(Special::Purgeable, 100), Row::Special(Special::Hidden, 100)]);
    }
}
