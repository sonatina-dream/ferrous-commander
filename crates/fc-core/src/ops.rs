//! File operations (SPEC §5): copy, move, delete, create, duplicate, links, sizes.
//!
//! Long-running operations report progress and ask questions through [`OpObserver`],
//! so the same code serves the UI (prompts) and tests (scripted answers).

use std::fs::{self, File, FileTimes};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Copy or move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Copy,
    Move,
}

/// Errors that stop an operation before or during execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// Nothing selected.
    EmptySelection,
    /// A folder would be copied/moved into itself or a subfolder of itself.
    IntoItself(PathBuf),
    /// Source and destination are the same item.
    SameFile(PathBuf),
    /// The destination folder is read-only.
    ReadOnlyTarget(PathBuf),
    /// The name is empty or contains `/` where not allowed.
    InvalidName(String),
    /// An item with this name already exists.
    AlreadyExists(PathBuf),
    /// The user chose Abort/Cancel.
    Aborted,
    Io(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::EmptySelection => write!(f, "Nothing is selected. Select the files or folders to act on."),
            OpError::IntoItself(p) => write!(f, "You can't copy or move \"{}\" into itself or one of its subfolders.", p.display()),
            OpError::SameFile(p) => write!(f, "\"{}\" is already there. Choose another destination.", p.display()),
            OpError::ReadOnlyTarget(p) => write!(f, "The destination \"{}\" is read-only.", p.display()),
            OpError::InvalidName(n) => write!(f, "\"{n}\" isn't a valid name."),
            OpError::AlreadyExists(p) => write!(f, "\"{}\" already exists. Choose a different name.", p.display()),
            OpError::Aborted => write!(f, "The operation was cancelled."),
            OpError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OpError {}

impl From<io::Error> for OpError {
    fn from(e: io::Error) -> Self {
        OpError::Io(e.to_string())
    }
}

/// Answer to "an item with this name already exists".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    Overwrite,
    OverwriteAll,
    Skip,
    SkipAll,
    /// Keep both: the incoming item gets a free `name N` name.
    KeepBoth,
    /// Folders only: copy into the existing folder.
    Merge,
    Cancel,
}

/// Answer to a per-item error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorChoice {
    Retry,
    Skip,
    SkipAll,
    Abort,
}

/// Progress snapshot of a running operation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_items: u64,
    pub done_items: u64,
    pub current: PathBuf,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total_bytes > 0 {
            (self.done_bytes as f64 / self.total_bytes as f64).min(1.0) as f32
        } else if self.total_items > 0 {
            (self.done_items as f64 / self.total_items as f64).min(1.0) as f32
        } else {
            0.0
        }
    }
}

/// Callbacks a running operation uses to report and ask.
pub trait OpObserver {
    fn progress(&mut self, _p: &Progress) {}
    fn conflict(&mut self, src: &Path, dst: &Path, is_dir: bool) -> ConflictChoice;
    fn error(&mut self, path: &Path, err: &io::Error) -> ErrorChoice;
    fn cancelled(&self) -> bool {
        false
    }
    /// Optional counter the copy loop increments per chunk (live byte progress).
    fn byte_counter(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicU64>> {
        None
    }
    /// Optional flag checked between chunks, so a large file stops promptly.
    fn cancel_flag(&self) -> Option<std::sync::Arc<AtomicBool>> {
        None
    }
}

/// Observer with fixed answers. Handy for tests and non-interactive callers.
pub struct AutoObserver {
    pub on_conflict: ConflictChoice,
    pub on_error: ErrorChoice,
    pub cancel: Option<std::sync::Arc<AtomicBool>>,
    pub last: Progress,
}

impl AutoObserver {
    pub fn new(on_conflict: ConflictChoice, on_error: ErrorChoice) -> Self {
        AutoObserver { on_conflict, on_error, cancel: None, last: Progress::default() }
    }
}

impl OpObserver for AutoObserver {
    fn progress(&mut self, p: &Progress) {
        self.last = p.clone();
    }
    fn conflict(&mut self, _src: &Path, _dst: &Path, _is_dir: bool) -> ConflictChoice {
        self.on_conflict
    }
    fn error(&mut self, _path: &Path, _err: &io::Error) -> ErrorChoice {
        self.on_error
    }
    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed))
    }
    fn cancel_flag(&self) -> Option<std::sync::Arc<AtomicBool>> {
        self.cancel.clone()
    }
}

/// Concrete (source → destination) pairs of a copy or move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferPlan {
    pub kind: TransferKind,
    pub pairs: Vec<(PathBuf, PathBuf)>,
    /// Folder to create before transferring (several items into a new path).
    pub create_dir: Option<PathBuf>,
}

/// Outcome counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub done: u64,
    pub skipped: u64,
    pub failed: u64,
    /// Top-level destination paths that were written (used to select results).
    pub outputs: Vec<PathBuf>,
}

/// Resolves the destination field of the copy/move dialog (SPEC §5.3).
///
/// * `dest` relative paths resolve against `base_dir` (the source panel's folder).
/// * An existing folder receives the items under their own names.
/// * A non-existing path is the new name for a single item, or a folder to create for several.
pub fn plan_transfer(
    kind: TransferKind,
    sources: &[PathBuf],
    dest: &Path,
    base_dir: &Path,
) -> Result<TransferPlan, OpError> {
    if sources.is_empty() {
        return Err(OpError::EmptySelection);
    }
    let dest = expand_tilde(dest);
    let dest = if dest.is_absolute() { dest } else { base_dir.join(dest) };
    let dest = normalize(&dest);

    let mut create_dir = None;
    let pairs: Vec<(PathBuf, PathBuf)> = if dest.is_dir() {
        sources
            .iter()
            .map(|s| (s.clone(), dest.join(s.file_name().unwrap_or_default())))
            .collect()
    } else if sources.len() == 1 {
        vec![(sources[0].clone(), dest.clone())]
    } else {
        create_dir = Some(dest.clone());
        sources
            .iter()
            .map(|s| (s.clone(), dest.join(s.file_name().unwrap_or_default())))
            .collect()
    };

    for (src, dst) in &pairs {
        let src_n = normalize(src);
        if src_n == *dst || (kind == TransferKind::Copy && same_inode(src, dst)) {
            return Err(OpError::SameFile(dst.clone()));
        }
        let src_is_real_dir = fs::symlink_metadata(src).map(|m| m.is_dir()).unwrap_or(false);
        if src_is_real_dir && dst.starts_with(&src_n) {
            return Err(OpError::IntoItself(src.clone()));
        }
    }
    let target_dir = create_dir.clone().unwrap_or_else(|| {
        pairs[0].1.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"))
    });
    if let Ok(meta) = fs::metadata(&target_dir)
        && (meta.permissions().readonly() || !is_writable(&target_dir)) {
            return Err(OpError::ReadOnlyTarget(target_dir));
        }
    Ok(TransferPlan { kind, pairs, create_dir })
}

/// True when both paths exist and are the same file-system object.
pub fn same_inode(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

fn is_writable(dir: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// Expands a leading `~` to the home folder.
pub fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| p.to_path_buf());
    }
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    p.to_path_buf()
}

/// Lexically removes `.` and `..` components (does not touch the disk).
pub fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() { PathBuf::from("/") } else { out }
}

/// Counts bytes and items below the given roots without following symlinks.
pub fn measure(paths: &[PathBuf]) -> (u64, u64) {
    let mut bytes = 0;
    let mut items = 0;
    for p in paths {
        walk_measure(p, &mut bytes, &mut items);
    }
    (bytes, items)
}

fn walk_measure(p: &Path, bytes: &mut u64, items: &mut u64) {
    let Ok(meta) = fs::symlink_metadata(p) else { return };
    *items += 1;
    if meta.is_dir() {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                walk_measure(&e.path(), bytes, items);
            }
        }
    } else {
        *bytes += meta.len();
    }
}

/// Recursive folder size: (bytes, files, folders). Stops early when `cancel` is set.
pub fn folder_size(path: &Path, cancel: &AtomicBool) -> (u64, u64, u64) {
    let mut acc = (0u64, 0u64, 0u64);
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(meta) = e.path().symlink_metadata() else { continue };
            if meta.is_dir() {
                acc.2 += 1;
                stack.push(e.path());
            } else {
                acc.0 += meta.len();
                acc.1 += 1;
            }
        }
    }
    acc
}

/// `name 2.ext`, `name 3.ext`, … — first free sibling name (Keep both).
pub fn unique_name(path: &Path) -> PathBuf {
    numbered_sibling(path, "", 2)
}

/// `name copy.ext`, then `name copy 2.ext`, … — Duplicate naming.
pub fn duplicate_name(path: &Path) -> PathBuf {
    let candidate = with_suffix(path, " copy");
    if fs::symlink_metadata(&candidate).is_err() {
        return candidate;
    }
    numbered_sibling(path, " copy", 2)
}

fn split_name(path: &Path) -> (String, String) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let is_dir = fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    if is_dir && !crate::entry::split_extension(&name).1.eq_ignore_ascii_case("app") {
        return (name, String::new());
    }
    let (stem, ext) = crate::entry::split_extension(&name);
    if ext.is_empty() { (name.clone(), String::new()) } else { (stem.to_string(), format!(".{ext}")) }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let (stem, ext) = split_name(path);
    path.with_file_name(format!("{stem}{suffix}{ext}"))
}

fn numbered_sibling(path: &Path, suffix: &str, start: u32) -> PathBuf {
    let (stem, ext) = split_name(path);
    let mut n = start;
    loop {
        let candidate = path.with_file_name(format!("{stem}{suffix} {n}{ext}"));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        n += 1;
    }
}

/// Executes copy/move plans with prompts and progress.
struct Transfer<'a, O: OpObserver> {
    observer: &'a mut O,
    progress: Progress,
    overwrite_all: bool,
    skip_all_conflicts: bool,
    skip_all_errors: bool,
    report: Report,
}

enum Step {
    Done,
    Skipped,
}

impl<'a, O: OpObserver> Transfer<'a, O> {
    fn check_cancel(&self) -> Result<(), OpError> {
        if self.observer.cancelled() { Err(OpError::Aborted) } else { Ok(()) }
    }

    /// Runs `op` until it succeeds or the user skips/aborts.
    fn attempt<T>(&mut self, path: &Path, mut op: impl FnMut() -> io::Result<T>) -> Result<Option<T>, OpError> {
        loop {
            self.check_cancel()?;
            match op() {
                Ok(v) => return Ok(Some(v)),
                Err(e) => {
                    if self.skip_all_errors {
                        self.report.failed += 1;
                        return Ok(None);
                    }
                    match self.observer.error(path, &e) {
                        ErrorChoice::Retry => continue,
                        ErrorChoice::Skip => {
                            self.report.failed += 1;
                            return Ok(None);
                        }
                        ErrorChoice::SkipAll => {
                            self.skip_all_errors = true;
                            self.report.failed += 1;
                            return Ok(None);
                        }
                        ErrorChoice::Abort => return Err(OpError::Aborted),
                    }
                }
            }
        }
    }

    /// Decides what to do when `dst` exists. Returns the destination to write, or None to skip.
    fn resolve_conflict(&mut self, src: &Path, dst: &Path, top_level: bool) -> Result<Option<(PathBuf, bool)>, OpError> {
        let Ok(dst_meta) = fs::symlink_metadata(dst) else { return Ok(Some((dst.to_path_buf(), false))) };
        let src_is_dir = fs::symlink_metadata(src).map(|m| m.is_dir()).unwrap_or(false);
        let both_dirs = src_is_dir && dst_meta.is_dir();
        if both_dirs && !top_level {
            return Ok(Some((dst.to_path_buf(), false))); // nested folders merge silently
        }
        if self.overwrite_all {
            return Ok(Some((dst.to_path_buf(), !both_dirs)));
        }
        if self.skip_all_conflicts {
            self.report.skipped += 1;
            return Ok(None);
        }
        match self.observer.conflict(src, dst, both_dirs) {
            ConflictChoice::Overwrite => Ok(Some((dst.to_path_buf(), !both_dirs))),
            ConflictChoice::OverwriteAll => {
                self.overwrite_all = true;
                Ok(Some((dst.to_path_buf(), !both_dirs)))
            }
            ConflictChoice::Merge => Ok(Some((dst.to_path_buf(), false))),
            ConflictChoice::Skip => {
                self.report.skipped += 1;
                Ok(None)
            }
            ConflictChoice::SkipAll => {
                self.skip_all_conflicts = true;
                self.report.skipped += 1;
                Ok(None)
            }
            ConflictChoice::KeepBoth => Ok(Some((unique_name(dst), false))),
            ConflictChoice::Cancel => Err(OpError::Aborted),
        }
    }

    fn copy_tree(&mut self, src: &Path, dst: &Path, top_level: bool) -> Result<Step, OpError> {
        self.check_cancel()?;
        let Some(meta) = self.attempt(src, || fs::symlink_metadata(src))? else { return Ok(Step::Skipped) };
        let Some((dst, replace)) = self.resolve_conflict(src, dst, top_level)? else { return Ok(Step::Skipped) };
        if replace {
            let dst2 = dst.clone();
            if self.attempt(&dst, || remove_any(&dst2))?.is_none() {
                return Ok(Step::Skipped);
            }
        }
        self.progress.current = src.to_path_buf();
        if top_level {
            self.report.outputs.push(dst.clone());
        }

        if meta.file_type().is_symlink() {
            let target = match self.attempt(src, || fs::read_link(src))? {
                Some(t) => t,
                None => return Ok(Step::Skipped),
            };
            let d = dst.clone();
            if self.attempt(src, || std::os::unix::fs::symlink(&target, &d))?.is_none() {
                return Ok(Step::Skipped);
            }
            self.item_done(0);
            return Ok(Step::Done);
        }

        if meta.is_dir() {
            if !dst.is_dir() {
                let d = dst.clone();
                if self.attempt(&dst, || fs::create_dir(&d))?.is_none() {
                    return Ok(Step::Skipped);
                }
            }
            let children: Vec<PathBuf> = match self.attempt(src, || fs::read_dir(src))? {
                Some(rd) => rd.flatten().map(|e| e.path()).collect(),
                None => return Ok(Step::Skipped),
            };
            let mut all_ok = true;
            for child in children {
                let name = child.file_name().unwrap_or_default().to_owned();
                if let Step::Skipped = self.copy_tree(&child, &dst.join(name), false)? {
                    all_ok = false;
                }
            }
            let _ = fs::set_permissions(&dst, meta.permissions());
            let _ = set_times(&dst, &meta);
            self.item_done(0);
            return Ok(if all_ok { Step::Done } else { Step::Skipped });
        }

        let d = dst.clone();
        match self.attempt(src, || copy_file_contents(src, &d)) {
            Ok(Some(_)) => {
                let _ = fs::set_permissions(&dst, meta.permissions());
                let _ = set_times(&dst, &meta);
                self.item_done(0);
                Ok(Step::Done)
            }
            Ok(None) => {
                let _ = fs::remove_file(&dst);
                Ok(Step::Skipped)
            }
            Err(e) => {
                let _ = fs::remove_file(&dst);
                Err(e)
            }
        }
    }

    fn item_done(&mut self, _bytes: u64) {
        self.progress.done_items += 1;
        self.observer.progress(&self.progress);
    }

    fn add_bytes(&mut self, n: u64) {
        self.progress.done_bytes += n;
    }

    fn move_item(&mut self, src: &Path, dst: &Path) -> Result<Step, OpError> {
        self.check_cancel()?;
        if same_inode(src, dst) {
            // Case-only rename on a case-insensitive volume.
            let d = dst.to_path_buf();
            return Ok(match self.attempt(src, || fs::rename(src, &d))? {
                Some(()) => {
                    self.report.outputs.push(dst.to_path_buf());
                    Step::Done
                }
                None => Step::Skipped,
            });
        }
        let Some((dst, replace)) = self.resolve_conflict(src, dst, true)? else { return Ok(Step::Skipped) };
        let dst_exists_as_dir = dst.is_dir() && !replace;
        if !dst_exists_as_dir {
            if replace {
                let d = dst.clone();
                if self.attempt(&dst, || remove_any(&d))?.is_none() {
                    return Ok(Step::Skipped);
                }
            }
            match fs::rename(src, &dst) {
                Ok(()) => {
                    let (b, i) = measure(std::slice::from_ref(&dst));
                    self.add_bytes(b);
                    self.progress.done_items += i;
                    self.observer.progress(&self.progress);
                    self.report.outputs.push(dst.clone());
                    return Ok(Step::Done);
                }
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {}
                Err(e) => {
                    // Surface as a normal per-item error with retry.
                    let mut first = Some(e);
                    let d = dst.clone();
                    let res = self.attempt(src, || match first.take() {
                        Some(err) => Err(err),
                        None => fs::rename(src, &d),
                    })?;
                    if res.is_some() {
                        self.report.outputs.push(dst.clone());
                        return Ok(Step::Done);
                    }
                    return Ok(Step::Skipped);
                }
            }
        }
        // Cross-volume move, or merging into an existing folder: copy, then delete.
        let step = self.copy_tree(src, &dst, false)?;
        if let Step::Done = step {
            self.report.outputs.push(dst.clone());
            let s = src.to_path_buf();
            self.attempt(src, || remove_any(&s))?;
            Ok(Step::Done)
        } else {
            Ok(Step::Skipped)
        }
    }
}

/// Copies file data in chunks, honoring the observer's cancellation through the
/// thread-local progress hook installed by `execute_transfer`.
fn copy_file_contents(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut input = File::open(src)?;
    let mut output = File::create(dst)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        output.write_all(&buf[..n])?;
        total += n as u64;
        CHUNK_HOOK.with(|h| {
            if let Some(hook) = h.borrow_mut().as_mut() {
                hook(n as u64)
            } else {
                Ok(())
            }
        })?;
    }
    output.flush()?;
    Ok(total)
}

type ChunkHook = Box<dyn FnMut(u64) -> io::Result<()>>;

thread_local! {
    static CHUNK_HOOK: std::cell::RefCell<Option<ChunkHook>> = const { std::cell::RefCell::new(None) };
}

fn set_times(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let mut times = FileTimes::new();
    if let Ok(m) = meta.modified() {
        times = times.set_modified(m);
    }
    if let Ok(a) = meta.accessed() {
        times = times.set_accessed(a);
    }
    File::open(path)?.set_times(times)
}

/// Removes a file, symlink or folder tree.
pub fn remove_any(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}

/// Executes a copy or move plan.
pub fn execute_transfer<O: OpObserver>(plan: &TransferPlan, observer: &mut O) -> Result<Report, OpError> {
    let sources: Vec<PathBuf> = plan.pairs.iter().map(|(s, _)| s.clone()).collect();
    let (total_bytes, total_items) = measure(&sources);
    if let Some(dir) = &plan.create_dir {
        fs::create_dir_all(dir)?;
    }

    // Chunked copies report bytes and observe cancellation through a thread-local hook,
    // so the copy loop does not need to own the observer.
    let shared_bytes = std::rc::Rc::new(std::cell::Cell::new(0u64));
    {
        let sb = shared_bytes.clone();
        let live = observer.byte_counter();
        let cancel = observer.cancel_flag();
        CHUNK_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |n| {
                sb.set(sb.get() + n);
                if let Some(live) = &live {
                    live.fetch_add(n, Ordering::Relaxed);
                }
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
                } else {
                    Ok(())
                }
            }))
        });
    }

    struct Bridge<'b, O: OpObserver> {
        inner: &'b mut O,
        bytes: std::rc::Rc<std::cell::Cell<u64>>,
    }
    impl<O: OpObserver> OpObserver for Bridge<'_, O> {
        fn progress(&mut self, p: &Progress) {
            let mut p = p.clone();
            p.done_bytes = p.done_bytes.max(self.bytes.get());
            self.inner.progress(&p);
        }
        fn conflict(&mut self, src: &Path, dst: &Path, is_dir: bool) -> ConflictChoice {
            self.inner.conflict(src, dst, is_dir)
        }
        fn error(&mut self, path: &Path, err: &io::Error) -> ErrorChoice {
            if self.inner.cancelled() {
                return ErrorChoice::Abort;
            }
            self.inner.error(path, err)
        }
        fn cancelled(&self) -> bool {
            self.inner.cancelled()
        }
    }

    let mut bridge = Bridge { inner: observer, bytes: shared_bytes.clone() };
    let mut t = Transfer {
        observer: &mut bridge,
        progress: Progress { total_bytes, total_items, ..Default::default() },
        overwrite_all: false,
        skip_all_conflicts: false,
        skip_all_errors: false,
        report: Report::default(),
    };
    // Announce the totals up front so a long single-file copy shows real progress.
    let initial = t.progress.clone();
    t.observer.progress(&initial);
    let mut result = Ok(());
    for (src, dst) in &plan.pairs {
        if let Some(parent) = dst.parent()
            && !parent.exists() {
                let _ = fs::create_dir_all(parent);
            }
        let step = match plan.kind {
            TransferKind::Copy => t.copy_tree(src, dst, true),
            TransferKind::Move => t.move_item(src, dst),
        };
        t.progress.done_bytes = t.progress.done_bytes.max(shared_bytes.get());
        match step {
            Ok(Step::Done) => t.report.done += 1,
            Ok(Step::Skipped) => {}
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    CHUNK_HOOK.with(|h| *h.borrow_mut() = None);
    let report = std::mem::take(&mut t.report);
    result.map(|_| report)
}

/// Deletes items: to the Trash, or permanently.
pub fn delete_items<O: OpObserver>(paths: &[PathBuf], permanent: bool, observer: &mut O) -> Result<Report, OpError> {
    let mut report = Report::default();
    let mut progress = Progress { total_items: paths.len() as u64, ..Default::default() };
    observer.progress(&progress);
    let mut skip_all = false;
    for p in paths {
        if observer.cancelled() {
            return Err(OpError::Aborted);
        }
        progress.current = p.clone();
        loop {
            let res = if permanent { remove_any(p) } else { move_to_trash(p) };
            match res {
                Ok(()) => {
                    report.done += 1;
                    break;
                }
                Err(e) => {
                    if skip_all {
                        report.failed += 1;
                        break;
                    }
                    match observer.error(p, &e) {
                        ErrorChoice::Retry => continue,
                        ErrorChoice::Skip => {
                            report.failed += 1;
                            break;
                        }
                        ErrorChoice::SkipAll => {
                            skip_all = true;
                            report.failed += 1;
                            break;
                        }
                        ErrorChoice::Abort => return Err(OpError::Aborted),
                    }
                }
            }
        }
        progress.done_items += 1;
        observer.progress(&progress);
    }
    Ok(report)
}

/// Moves one item to the user's Trash (NSFileManager, no Finder automation needed).
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut ctx = trash::TrashContext::default();
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path).map_err(|e| io::Error::other(e.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(path).map_err(|e| io::Error::other(e.to_string()))
    }
}

fn validate_name(name: &str, allow_slash: bool) -> Result<(), OpError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." || (!allow_slash && trimmed.contains('/')) || trimmed.contains('\0') {
        return Err(OpError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Creates a folder `name` in `dir` (`a/b` creates nested folders).
pub fn create_folder(dir: &Path, name: &str) -> Result<PathBuf, OpError> {
    validate_name(name, true)?;
    let path = dir.join(name.trim());
    if fs::symlink_metadata(&path).is_ok() {
        return Err(OpError::AlreadyExists(path));
    }
    if name.contains('/') { fs::create_dir_all(&path)? } else { fs::create_dir(&path)? }
    Ok(path)
}

/// Creates folder `name` in `dir` and moves `items` into it ("New Folder with Selection").
pub fn create_folder_with(dir: &Path, name: &str, items: &[PathBuf]) -> Result<PathBuf, OpError> {
    let folder = create_folder(dir, name)?;
    for item in items {
        let dst = folder.join(item.file_name().unwrap_or_default());
        fs::rename(item, dst)?;
    }
    Ok(folder)
}

/// Creates an empty file `name` in `dir`.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf, OpError> {
    validate_name(name, false)?;
    let path = dir.join(name.trim());
    match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(OpError::AlreadyExists(path)),
        Err(e) => Err(e.into()),
    }
}

/// Renames `path` in place to `new_name`.
pub fn rename_in_place(path: &Path, new_name: &str) -> Result<PathBuf, OpError> {
    validate_name(new_name, false)?;
    let target = path.with_file_name(new_name.trim());
    if target == path {
        return Ok(target);
    }
    let case_only = target.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if fs::symlink_metadata(&target).is_ok() && !case_only {
        return Err(OpError::AlreadyExists(target));
    }
    fs::rename(path, &target)?;
    Ok(target)
}

/// True when renaming `old` → `new` changes a file's extension (confirmation prompt).
pub fn changes_extension(old: &str, new: &str) -> bool {
    let (_, a) = crate::entry::split_extension(old);
    let (_, b) = crate::entry::split_extension(new);
    !a.is_empty() && !a.eq_ignore_ascii_case(b)
}

/// Duplicates each item next to itself (`name copy`, `name copy 2`, …).
pub fn duplicate<O: OpObserver>(items: &[PathBuf], observer: &mut O) -> Result<Vec<PathBuf>, OpError> {
    let mut outputs = Vec::new();
    for item in items {
        let target = duplicate_name(item);
        let plan = TransferPlan { kind: TransferKind::Copy, pairs: vec![(item.clone(), target.clone())], create_dir: None };
        execute_transfer(&plan, observer)?;
        outputs.push(target);
    }
    Ok(outputs)
}

/// Creates symlinks to `items` inside `dest_dir`.
pub fn make_symlinks(items: &[PathBuf], dest_dir: &Path) -> Result<Vec<PathBuf>, OpError> {
    let mut out = Vec::new();
    for item in items {
        let link = dest_dir.join(item.file_name().unwrap_or_default());
        if fs::symlink_metadata(&link).is_ok() {
            return Err(OpError::AlreadyExists(link));
        }
        std::os::unix::fs::symlink(item, &link)?;
        out.push(link);
    }
    Ok(out)
}

/// Sets permission bits (lower 12 bits of `mode`).
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o7777))
}

/// Sets or clears the user-immutable ("Locked") flag.
pub fn set_locked(path: &Path, locked: bool) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let current = {
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::MetadataExt;
            fs::symlink_metadata(path)?.st_flags()
        }
        #[cfg(not(target_os = "macos"))]
        {
            0u32
        }
    };
    let flags = if locked { current | libc::UF_IMMUTABLE } else { current & !libc::UF_IMMUTABLE };
    let c = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
    // SAFETY: valid NUL-terminated path.
    if unsafe { libc::chflags(c.as_ptr(), flags as _) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// User and group names for Get Info.
pub fn owner_names(uid: u32, gid: u32) -> (String, String) {
    // SAFETY: getpwuid/getgrgid return pointers into static storage or null; we copy
    // the strings immediately. Called from the UI thread only.
    unsafe {
        let user = {
            let pw = libc::getpwuid(uid);
            if pw.is_null() { uid.to_string() } else { std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned() }
        };
        let group = {
            let gr = libc::getgrgid(gid);
            if gr.is_null() { gid.to_string() } else { std::ffi::CStr::from_ptr((*gr).gr_name).to_string_lossy().into_owned() }
        };
        (user, group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("src/sub")).unwrap();
        fs::write(r.join("src/a.txt"), b"alpha").unwrap();
        fs::write(r.join("src/sub/b.txt"), b"beta").unwrap();
        fs::create_dir(r.join("dst")).unwrap();
        t
    }

    fn auto() -> AutoObserver {
        AutoObserver::new(ConflictChoice::Cancel, ErrorChoice::Abort)
    }

    #[test]
    fn plan_resolution_rules() {
        let t = tree();
        let r = t.path();
        let a = r.join("src/a.txt");
        let sub = r.join("src/sub");

        let p = plan_transfer(TransferKind::Copy, std::slice::from_ref(&a), &r.join("dst"), r).unwrap();
        assert_eq!(p.pairs, vec![(a.clone(), r.join("dst/a.txt"))]);

        let p = plan_transfer(TransferKind::Copy, std::slice::from_ref(&a), Path::new("dst/renamed.txt"), r).unwrap();
        assert_eq!(p.pairs[0].1, r.join("dst/renamed.txt"), "relative path resolves against base dir");

        let p = plan_transfer(TransferKind::Copy, &[a.clone(), sub.clone()], &r.join("newdir"), r).unwrap();
        assert_eq!(p.create_dir, Some(r.join("newdir")));
        assert_eq!(p.pairs[1].1, r.join("newdir/sub"));

        assert_eq!(
            plan_transfer(TransferKind::Copy, &[r.join("src")], &r.join("src/sub"), r),
            Err(OpError::IntoItself(r.join("src")))
        );
        assert_eq!(
            plan_transfer(TransferKind::Copy, std::slice::from_ref(&a), &r.join("src"), r),
            Err(OpError::SameFile(a.clone()))
        );
        assert_eq!(plan_transfer(TransferKind::Copy, &[], r, r), Err(OpError::EmptySelection));
    }

    #[test]
    fn copy_tree_preserves_content_and_mtime() {
        let t = tree();
        let r = t.path();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        File::open(r.join("src/a.txt")).unwrap().set_times(FileTimes::new().set_modified(old)).unwrap();
        let plan = plan_transfer(TransferKind::Copy, &[r.join("src")], &r.join("dst"), r).unwrap();
        let mut obs = auto();
        let rep = execute_transfer(&plan, &mut obs).unwrap();
        assert_eq!(rep.done, 1);
        assert_eq!(fs::read(r.join("dst/src/sub/b.txt")).unwrap(), b"beta");
        assert_eq!(fs::metadata(r.join("dst/src/a.txt")).unwrap().modified().unwrap(), old);
        assert!(r.join("src/a.txt").exists(), "copy keeps the source");
        assert_eq!(obs.last.total_bytes, 9);
        assert_eq!(obs.last.done_bytes, 9);
    }

    #[test]
    fn totals_are_reported_before_the_first_item_finishes() {
        let t = tree();
        let r = t.path();
        struct First(Option<Progress>);
        impl OpObserver for First {
            fn progress(&mut self, p: &Progress) {
                self.0.get_or_insert_with(|| p.clone());
            }
            fn conflict(&mut self, _: &Path, _: &Path, _: bool) -> ConflictChoice {
                ConflictChoice::Cancel
            }
            fn error(&mut self, _: &Path, _: &io::Error) -> ErrorChoice {
                ErrorChoice::Abort
            }
        }
        let plan = plan_transfer(TransferKind::Copy, &[r.join("src")], &r.join("dst"), r).unwrap();
        let mut obs = First(None);
        execute_transfer(&plan, &mut obs).unwrap();
        let first = obs.0.unwrap();
        assert_eq!((first.total_bytes, first.done_items), (9, 0), "first report carries totals, nothing done yet");
    }

    #[test]
    fn conflict_policies() {
        let t = tree();
        let r = t.path();
        fs::write(r.join("dst/a.txt"), b"old").unwrap();
        let src = r.join("src/a.txt");
        let plan = plan_transfer(TransferKind::Copy, std::slice::from_ref(&src), &r.join("dst"), r).unwrap();

        let mut skip = AutoObserver::new(ConflictChoice::Skip, ErrorChoice::Abort);
        let rep = execute_transfer(&plan, &mut skip).unwrap();
        assert_eq!((rep.done, rep.skipped), (0, 1));
        assert_eq!(fs::read(r.join("dst/a.txt")).unwrap(), b"old");

        let mut keep = AutoObserver::new(ConflictChoice::KeepBoth, ErrorChoice::Abort);
        execute_transfer(&plan, &mut keep).unwrap();
        assert_eq!(fs::read(r.join("dst/a 2.txt")).unwrap(), b"alpha");
        execute_transfer(&plan, &mut keep).unwrap();
        assert!(r.join("dst/a 3.txt").exists());

        let mut over = AutoObserver::new(ConflictChoice::Overwrite, ErrorChoice::Abort);
        execute_transfer(&plan, &mut over).unwrap();
        assert_eq!(fs::read(r.join("dst/a.txt")).unwrap(), b"alpha");

        let mut cancel = AutoObserver::new(ConflictChoice::Cancel, ErrorChoice::Abort);
        assert_eq!(execute_transfer(&plan, &mut cancel), Err(OpError::Aborted));
    }

    #[test]
    fn folder_merge_on_conflict() {
        let t = tree();
        let r = t.path();
        fs::create_dir_all(r.join("dst/src/sub")).unwrap();
        fs::write(r.join("dst/src/keep.txt"), b"k").unwrap();
        fs::write(r.join("dst/src/sub/b.txt"), b"old").unwrap();
        let plan = plan_transfer(TransferKind::Copy, &[r.join("src")], &r.join("dst"), r).unwrap();

        struct MergeThenOverwrite(Vec<bool>);
        impl OpObserver for MergeThenOverwrite {
            fn conflict(&mut self, _s: &Path, _d: &Path, is_dir: bool) -> ConflictChoice {
                self.0.push(is_dir);
                if is_dir { ConflictChoice::Merge } else { ConflictChoice::OverwriteAll }
            }
            fn error(&mut self, _p: &Path, _e: &io::Error) -> ErrorChoice {
                ErrorChoice::Abort
            }
        }
        let mut obs = MergeThenOverwrite(Vec::new());
        execute_transfer(&plan, &mut obs).unwrap();
        assert_eq!(obs.0, vec![true, false], "one folder prompt, then OverwriteAll silences the rest");
        assert!(r.join("dst/src/keep.txt").exists());
        assert_eq!(fs::read(r.join("dst/src/sub/b.txt")).unwrap(), b"beta");
        assert_eq!(fs::read(r.join("dst/src/a.txt")).unwrap(), b"alpha");
    }

    #[test]
    fn move_and_rename() {
        let t = tree();
        let r = t.path();
        let plan = plan_transfer(TransferKind::Move, &[r.join("src/a.txt")], &r.join("dst"), r).unwrap();
        execute_transfer(&plan, &mut auto()).unwrap();
        assert!(!r.join("src/a.txt").exists());
        assert!(r.join("dst/a.txt").exists());

        let plan = plan_transfer(TransferKind::Move, &[r.join("src/sub")], Path::new("moved"), &r.join("src")).unwrap();
        let rep = execute_transfer(&plan, &mut auto()).unwrap();
        assert_eq!(rep.outputs, vec![r.join("src/moved")]);
        assert!(r.join("src/moved/b.txt").exists());
    }

    #[test]
    fn symlinks_are_copied_as_links() {
        let t = tree();
        let r = t.path();
        std::os::unix::fs::symlink("a.txt", r.join("src/link")).unwrap();
        let plan = plan_transfer(TransferKind::Copy, &[r.join("src/link")], &r.join("dst"), r).unwrap();
        execute_transfer(&plan, &mut auto()).unwrap();
        assert_eq!(fs::read_link(r.join("dst/link")).unwrap(), PathBuf::from("a.txt"));
    }

    #[test]
    fn error_prompt_skip_and_abort() {
        let t = tree();
        let r = t.path();
        let unreadable = r.join("src/secret.txt");
        fs::write(&unreadable, b"x").unwrap();
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&unreadable).is_ok() {
            return; // running as root; permissions are not enforced
        }
        let plan = plan_transfer(TransferKind::Copy, &[unreadable.clone(), r.join("src/a.txt")], &r.join("dst"), r).unwrap();
        let mut skip = AutoObserver::new(ConflictChoice::Cancel, ErrorChoice::Skip);
        let rep = execute_transfer(&plan, &mut skip).unwrap();
        assert_eq!((rep.done, rep.failed), (1, 1));
        let mut abort = AutoObserver::new(ConflictChoice::Overwrite, ErrorChoice::Abort);
        assert_eq!(execute_transfer(&plan, &mut abort), Err(OpError::Aborted));
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn naming_helpers() {
        let t = tree();
        let r = t.path();
        let a = r.join("src/a.txt");
        assert_eq!(duplicate_name(&a), r.join("src/a copy.txt"));
        fs::write(r.join("src/a copy.txt"), b"").unwrap();
        assert_eq!(duplicate_name(&a), r.join("src/a copy 2.txt"));
        assert_eq!(duplicate_name(&r.join("src/sub")), r.join("src/sub copy"));
        assert_eq!(unique_name(&a), r.join("src/a 2.txt"));
        assert!(changes_extension("a.txt", "a.md"));
        assert!(!changes_extension("a.txt", "b.TXT"));
        assert!(!changes_extension("Makefile", "Makefile.bak"));
    }

    #[test]
    fn create_rename_duplicate_link() {
        let t = tree();
        let r = t.path();
        let f = create_folder(r, "New").unwrap();
        assert!(f.is_dir());
        assert_eq!(create_folder(r, "New"), Err(OpError::AlreadyExists(r.join("New"))));
        assert!(matches!(create_folder(r, " "), Err(OpError::InvalidName(_))));
        create_folder(r, "x/y/z").unwrap();
        assert!(r.join("x/y/z").is_dir());

        let file = create_file(r, "empty.txt").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().len(), 0);
        assert!(matches!(create_file(r, "empty.txt"), Err(OpError::AlreadyExists(_))));

        let renamed = rename_in_place(&file, "full.txt").unwrap();
        assert!(renamed.exists() && !file.exists());
        assert!(matches!(rename_in_place(&renamed, "a/b"), Err(OpError::InvalidName(_))));

        let dups = duplicate(&[r.join("src")], &mut auto()).unwrap();
        assert!(dups[0].join("sub/b.txt").exists());

        let links = make_symlinks(&[r.join("src/a.txt")], &r.join("dst")).unwrap();
        assert_eq!(fs::read_link(&links[0]).unwrap(), r.join("src/a.txt"));
        assert!(make_symlinks(&[r.join("src/a.txt")], &r.join("dst")).is_err());

        let folder = create_folder_with(r, "Bundle", &[r.join("src/a.txt")]).unwrap();
        assert!(folder.join("a.txt").exists());
    }

    #[test]
    fn permanent_delete_and_sizes() {
        let t = tree();
        let r = t.path();
        let flag = AtomicBool::new(false);
        assert_eq!(folder_size(&r.join("src"), &flag), (9, 2, 1));
        let rep = delete_items(&[r.join("src")], true, &mut auto()).unwrap();
        assert_eq!(rep.done, 1);
        assert!(!r.join("src").exists());
    }

    #[test]
    fn modes_and_lock_flag() {
        let t = tree();
        let a = t.path().join("src/a.txt");
        set_mode(&a, 0o600).unwrap();
        assert_eq!(fs::metadata(&a).unwrap().permissions().mode() & 0o777, 0o600);
        set_locked(&a, true).unwrap();
        assert!(fs::remove_file(&a).is_err(), "locked files cannot be removed");
        set_locked(&a, false).unwrap();
        fs::remove_file(&a).unwrap();
    }
}
