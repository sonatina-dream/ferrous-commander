//! Panel and tab state: location, entries, cursor, selection, history, locks
//! (SPEC §2, §4, §6, §7). This module is UI-agnostic.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::entry::{list_dir, parent_entry, read_entry, Entry};
use crate::format::SelectionStats;
use crate::pattern::Mask;
use crate::sort::{sort_entries, SortSpec};

/// File-list presentation (SPEC §2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Full,
    Brief,
    Thumbs,
}

/// Tab lock (SPEC §6). Stored as 0/1/2 like the observed `lockState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub enum LockState {
    #[default]
    Unlocked,
    /// Navigating elsewhere opens a new tab.
    Locked,
    /// Locked against closing, but the folder may change.
    LockedDirChangesAllowed,
}

impl From<u8> for LockState {
    fn from(v: u8) -> Self {
        match v {
            1 => LockState::Locked,
            2 => LockState::LockedDirChangesAllowed,
            _ => LockState::Unlocked,
        }
    }
}

impl From<LockState> for u8 {
    fn from(v: LockState) -> u8 {
        match v {
            LockState::Unlocked => 0,
            LockState::Locked => 1,
            LockState::LockedDirChangesAllowed => 2,
        }
    }
}

impl LockState {
    pub fn next(self) -> LockState {
        match self {
            LockState::Unlocked => LockState::Locked,
            LockState::Locked => LockState::LockedDirChangesAllowed,
            LockState::LockedDirChangesAllowed => LockState::Unlocked,
        }
    }

    pub fn is_locked(self) -> bool {
        self != LockState::Unlocked
    }
}

/// What a tab shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    Dir(PathBuf),
    /// Search results fed to the panel ("virtual folder").
    Results { title: String, root: PathBuf, items: Vec<PathBuf> },
}

/// Listing options that come from preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListOptions {
    pub show_hidden: bool,
    pub folders_first: bool,
}

impl Default for ListOptions {
    fn default() -> Self {
        ListOptions { show_hidden: false, folders_first: true }
    }
}

const HISTORY_LIMIT: usize = 64;

/// Back/forward stacks of one tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
}

impl History {
    /// Records leaving `from` for a new location; clears the forward stack.
    pub fn push(&mut self, from: PathBuf) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
            if self.back.len() > HISTORY_LIMIT {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

/// Result of activating the cursor row.
#[derive(Clone, Debug, PartialEq)]
pub enum OpenOutcome {
    Navigated,
    /// A file (or package) the UI should open with its default application.
    OpenFile(PathBuf),
    Nothing,
}

/// One tab: a location plus its entries, cursor and selection.
#[derive(Clone, Debug)]
pub struct Tab {
    pub location: Location,
    pub entries: Vec<Entry>,
    pub cursor: usize,
    pub selected: HashSet<PathBuf>,
    pub view_mode: ViewMode,
    pub sort: SortSpec,
    pub lock: LockState,
    pub history: History,
    /// Recursively computed folder sizes (Space / "Calculate folder size").
    pub dir_sizes: HashMap<PathBuf, u64>,
}

fn read_location(location: &Location, opts: ListOptions) -> io::Result<Vec<Entry>> {
    match location {
        Location::Dir(dir) => {
            let mut entries = list_dir(dir, opts.show_hidden)?;
            if let Some(parent) = parent_entry(dir) {
                entries.push(parent);
            }
            Ok(entries)
        }
        Location::Results { root, items, .. } => Ok(items
            .iter()
            .filter_map(|p| {
                let mut e = read_entry(p).ok()?;
                e.name = p
                    .strip_prefix(root)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string_lossy().into_owned());
                Some(e)
            })
            .collect()),
    }
}

impl Tab {
    /// A tab at `dir`. Call [`Tab::reload`] to populate entries.
    pub fn new(dir: PathBuf) -> Tab {
        Tab {
            location: Location::Dir(dir),
            entries: Vec::new(),
            cursor: 0,
            selected: HashSet::new(),
            view_mode: ViewMode::Full,
            sort: SortSpec::default(),
            lock: LockState::Unlocked,
            history: History::default(),
            dir_sizes: HashMap::new(),
        }
    }

    /// Folder shown by the tab; for search results, the search root.
    pub fn working_dir(&self) -> &Path {
        match &self.location {
            Location::Dir(p) => p,
            Location::Results { root, .. } => root,
        }
    }

    pub fn is_results(&self) -> bool {
        matches!(self.location, Location::Results { .. })
    }

    /// Tab caption: folder name, `/` for the root, or the results title.
    pub fn title(&self) -> String {
        match &self.location {
            Location::Dir(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
            Location::Results { title, .. } => title.clone(),
        }
    }

    pub fn cursor_entry(&self) -> Option<&Entry> {
        self.entries.get(self.cursor)
    }

    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.path == path && !e.is_parent())
    }

    fn commit(&mut self, location: Location, mut entries: Vec<Entry>, opts: ListOptions, focus: Option<&Path>) {
        self.dir_sizes.clear();
        sort_entries(&mut entries, self.sort, opts.folders_first, &self.dir_sizes);
        self.location = location;
        self.entries = entries;
        self.selected.clear();
        self.cursor = focus.and_then(|p| self.index_of(p)).unwrap_or(0);
    }

    /// Re-reads the current location, keeping the cursor item and the selection
    /// (items that disappeared are dropped).
    pub fn reload(&mut self, opts: ListOptions) -> io::Result<()> {
        let mut entries = read_location(&self.location, opts)?;
        sort_entries(&mut entries, self.sort, opts.folders_first, &self.dir_sizes);
        let cursor_path = self.cursor_entry().map(|e| (e.path.clone(), e.is_parent()));
        let old_cursor = self.cursor;
        self.entries = entries;
        let present: HashSet<&PathBuf> = self.entries.iter().map(|e| &e.path).collect();
        self.selected.retain(|p| present.contains(p));
        self.dir_sizes.retain(|p, _| present.contains(p));
        self.cursor = match cursor_path {
            Some((_, true)) => 0,
            Some((p, false)) => self.index_of(&p).unwrap_or(old_cursor),
            None => old_cursor,
        };
        self.clamp_cursor();
        Ok(())
    }

    /// Re-sorts without touching the disk.
    pub fn resort(&mut self, opts: ListOptions) {
        let cursor_path = self.cursor_entry().map(|e| e.path.clone());
        sort_entries(&mut self.entries, self.sort, opts.folders_first, &self.dir_sizes);
        if let Some(p) = cursor_path
            && let Some(ix) = self.entries.iter().position(|e| e.path == p) {
                self.cursor = ix;
            }
    }

    pub fn set_sort(&mut self, sort: SortSpec, opts: ListOptions) {
        self.sort = sort;
        self.resort(opts);
    }

    fn clamp_cursor(&mut self) {
        if self.entries.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len() - 1;
        }
    }

    /// Navigates to `dir`, pushing the current folder onto the back stack. On a read
    /// error nothing changes.
    pub fn navigate(&mut self, dir: PathBuf, opts: ListOptions) -> io::Result<()> {
        self.navigate_focus(dir, opts, None)
    }

    /// Like [`Tab::navigate`] but puts the cursor on `focus` if it is listed.
    pub fn navigate_focus(&mut self, dir: PathBuf, opts: ListOptions, focus: Option<&Path>) -> io::Result<()> {
        let location = Location::Dir(dir);
        let entries = read_location(&location, opts)?;
        let from = self.working_dir().to_path_buf();
        if Location::Dir(from.clone()) != location || self.is_results() {
            self.history.push(from);
        }
        self.commit(location, entries, opts, focus);
        Ok(())
    }

    /// Shows search results as a virtual folder.
    pub fn show_results(&mut self, title: String, root: PathBuf, items: Vec<PathBuf>, opts: ListOptions) {
        let location = Location::Results { title, root, items };
        let entries = read_location(&location, opts).unwrap_or_default();
        self.history.push(self.working_dir().to_path_buf());
        self.commit(location, entries, opts, None);
    }

    /// Goes to the enclosing folder and puts the cursor on the folder just left.
    pub fn go_up(&mut self, opts: ListOptions) -> io::Result<bool> {
        let (target, focus) = match &self.location {
            Location::Dir(dir) => match dir.parent() {
                Some(parent) => (parent.to_path_buf(), dir.clone()),
                None => return Ok(false),
            },
            Location::Results { root, .. } => (root.clone(), root.clone()),
        };
        self.navigate_focus(target, opts, Some(&focus))?;
        Ok(true)
    }

    pub fn go_back(&mut self, opts: ListOptions) -> io::Result<bool> {
        let Some(target) = self.history.back.pop() else { return Ok(false) };
        let location = Location::Dir(target.clone());
        match read_location(&location, opts) {
            Ok(entries) => {
                let from = self.working_dir().to_path_buf();
                self.history.forward.push(from.clone());
                self.commit(location, entries, opts, Some(&from));
                Ok(true)
            }
            Err(e) => {
                self.history.back.push(target);
                Err(e)
            }
        }
    }

    pub fn go_forward(&mut self, opts: ListOptions) -> io::Result<bool> {
        let Some(target) = self.history.forward.pop() else { return Ok(false) };
        let location = Location::Dir(target.clone());
        match read_location(&location, opts) {
            Ok(entries) => {
                self.history.back.push(self.working_dir().to_path_buf());
                self.commit(location, entries, opts, None);
                Ok(true)
            }
            Err(e) => {
                self.history.forward.push(target);
                Err(e)
            }
        }
    }

    /// Moves the cursor by `delta` rows. With `extend`, the row being left is marked
    /// first (orthodox Shift+arrow behavior).
    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        if self.entries.is_empty() {
            return;
        }
        if extend {
            self.mark_cursor(true);
        }
        let last = self.entries.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last) as usize;
    }

    pub fn set_cursor(&mut self, ix: usize) {
        self.cursor = ix;
        self.clamp_cursor();
    }

    pub fn cursor_to_start(&mut self, extend: bool) {
        self.move_cursor(-(self.entries.len() as isize), extend);
    }

    pub fn cursor_to_end(&mut self, extend: bool) {
        self.move_cursor(self.entries.len() as isize, extend);
    }

    fn mark_cursor(&mut self, on: bool) {
        if let Some(e) = self.entries.get(self.cursor)
            && !e.is_parent() {
                if on {
                    self.selected.insert(e.path.clone());
                } else {
                    self.selected.remove(&e.path);
                }
            }
    }

    pub fn is_selected(&self, entry: &Entry) -> bool {
        self.selected.contains(&entry.path)
    }

    /// Toggles the selection of row `ix` (⌘-click). `..` is never selected.
    pub fn toggle_index(&mut self, ix: usize) {
        if let Some(e) = self.entries.get(ix) {
            if e.is_parent() {
                return;
            }
            if !self.selected.remove(&e.path) {
                self.selected.insert(e.path.clone());
            }
        }
    }

    /// Toggles the cursor row and advances one row (Space / § behavior).
    pub fn toggle_cursor_and_advance(&mut self) {
        self.toggle_index(self.cursor);
        self.move_cursor(1, false);
    }

    /// Selects rows between `a` and `b` inclusive (⇧-click).
    pub fn select_range(&mut self, a: usize, b: usize) {
        let (lo, hi) = (a.min(b), a.max(b));
        for e in self.entries.iter().skip(lo).take(hi - lo + 1) {
            if !e.is_parent() {
                self.selected.insert(e.path.clone());
            }
        }
    }

    fn eligible(e: &Entry, files_only: bool) -> bool {
        !e.is_parent() && !(files_only && e.is_dir_like())
    }

    /// Adds entries matching `mask`; returns how many became selected.
    pub fn select_mask(&mut self, mask: &Mask, files_only: bool) -> usize {
        let mut added = 0;
        for e in &self.entries {
            if Self::eligible(e, files_only) && mask.matches(&e.name) && self.selected.insert(e.path.clone()) {
                added += 1;
            }
        }
        added
    }

    /// Removes entries matching `mask`; returns how many were unselected.
    pub fn unselect_mask(&mut self, mask: &Mask, files_only: bool) -> usize {
        let mut removed = 0;
        for e in &self.entries {
            if Self::eligible(e, files_only) && mask.matches(&e.name) && self.selected.remove(&e.path) {
                removed += 1;
            }
        }
        removed
    }

    pub fn invert_selection(&mut self, files_only: bool) {
        for e in &self.entries {
            if Self::eligible(e, files_only) && !self.selected.remove(&e.path) {
                self.selected.insert(e.path.clone());
            }
        }
    }

    pub fn select_all(&mut self, files_only: bool) {
        for e in &self.entries {
            if Self::eligible(e, files_only) {
                self.selected.insert(e.path.clone());
            }
        }
    }

    pub fn unselect_all(&mut self) {
        self.selected.clear();
    }

    /// What an operation acts on: the selection in display order, or the cursor item.
    pub fn targets(&self) -> Vec<Entry> {
        if !self.selected.is_empty() {
            return self.entries.iter().filter(|e| self.selected.contains(&e.path)).cloned().collect();
        }
        match self.cursor_entry() {
            Some(e) if !e.is_parent() => vec![e.clone()],
            _ => Vec::new(),
        }
    }

    pub fn target_paths(&self) -> Vec<PathBuf> {
        self.targets().into_iter().map(|e| e.path).collect()
    }

    /// Known size of an entry: file length or computed folder size.
    pub fn entry_size(&self, e: &Entry) -> Option<u64> {
        if e.is_dir_like() {
            self.dir_sizes.get(&e.path).copied()
        } else {
            Some(e.size)
        }
    }

    pub fn set_dir_size(&mut self, path: PathBuf, bytes: u64, opts: ListOptions) {
        self.dir_sizes.insert(path, bytes);
        if self.sort.key == crate::sort::SortKey::Size {
            self.resort(opts);
        }
    }

    /// Footer counts (SPEC §2.4).
    pub fn stats(&self) -> SelectionStats {
        let mut s = SelectionStats::default();
        for e in self.entries.iter().filter(|e| !e.is_parent()) {
            let bytes = self.entry_size(e).unwrap_or(0);
            let selected = self.selected.contains(&e.path);
            if e.is_dir_like() {
                s.total_dirs += 1;
                if selected {
                    s.selected_dirs += 1;
                }
            } else {
                s.total_files += 1;
                if selected {
                    s.selected_files += 1;
                }
            }
            s.total_bytes += bytes;
            if selected {
                s.selected_bytes += bytes;
            }
        }
        s
    }

    /// Type-ahead: moves the cursor to the first row whose name starts with `prefix`
    /// (case-insensitive). Returns false when nothing matches.
    pub fn type_ahead(&mut self, prefix: &str) -> bool {
        if prefix.is_empty() {
            return false;
        }
        let p = prefix.to_lowercase();
        match self.entries.iter().position(|e| !e.is_parent() && e.name.to_lowercase().starts_with(&p)) {
            Some(ix) => {
                self.cursor = ix;
                true
            }
            None => false,
        }
    }
}

/// Why a tab could not be closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTabError {
    LastTab,
    /// The tab is locked; ask the user and call again with `force`.
    Locked,
}

/// One side of the window: an ordered list of tabs, one active.
#[derive(Clone, Debug)]
pub struct Panel {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Recently visited folders, most recent first (folder-history popup).
    pub folder_history: Vec<PathBuf>,
}

const FOLDER_HISTORY_LIMIT: usize = 30;

impl Panel {
    pub fn new(dir: PathBuf) -> Panel {
        Panel { tabs: vec![Tab::new(dir)], active: 0, folder_history: Vec::new() }
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    pub fn record_history(&mut self, dir: &Path) {
        self.folder_history.retain(|p| p != dir);
        self.folder_history.insert(0, dir.to_path_buf());
        self.folder_history.truncate(FOLDER_HISTORY_LIMIT);
    }

    /// Navigates the active tab, or opens a new tab when it is fully locked.
    pub fn navigate(&mut self, dir: PathBuf, opts: ListOptions) -> io::Result<()> {
        self.navigate_focus(dir, opts, None)
    }

    pub fn navigate_focus(&mut self, dir: PathBuf, opts: ListOptions, focus: Option<&Path>) -> io::Result<()> {
        if self.tab().lock == LockState::Locked {
            let mut tab = Tab::new(dir.clone());
            tab.view_mode = self.tab().view_mode;
            tab.sort = self.tab().sort;
            tab.navigate_focus(dir.clone(), opts, focus)?;
            tab.history = History::default();
            self.tabs.insert(self.active + 1, tab);
            self.active += 1;
        } else {
            self.tab_mut().navigate_focus(dir.clone(), opts, focus)?;
        }
        self.record_history(&dir);
        Ok(())
    }

    pub fn go_up(&mut self, opts: ListOptions) -> io::Result<bool> {
        let (target, focus) = match &self.tab().location {
            Location::Dir(dir) => match dir.parent() {
                Some(p) => (p.to_path_buf(), dir.clone()),
                None => return Ok(false),
            },
            Location::Results { root, .. } => (root.clone(), root.clone()),
        };
        self.navigate_focus(target, opts, Some(&focus))?;
        Ok(true)
    }

    /// Return / double-click on the cursor row.
    pub fn open_cursor(&mut self, opts: ListOptions) -> io::Result<OpenOutcome> {
        let Some(entry) = self.tab().cursor_entry().cloned() else { return Ok(OpenOutcome::Nothing) };
        if entry.is_parent() {
            self.go_up(opts)?;
            return Ok(OpenOutcome::Navigated);
        }
        if entry.is_navigable() {
            self.navigate(entry.path, opts)?;
            return Ok(OpenOutcome::Navigated);
        }
        Ok(OpenOutcome::OpenFile(entry.path))
    }

    /// ⌥Return: enters packages too.
    pub fn enter_cursor(&mut self, opts: ListOptions) -> io::Result<OpenOutcome> {
        match self.tab().cursor_entry().cloned() {
            Some(e) if e.is_dir_like() && !e.is_parent() => {
                self.navigate(e.path, opts)?;
                Ok(OpenOutcome::Navigated)
            }
            _ => self.open_cursor(opts),
        }
    }

    /// Opens a new tab at `dir` right after the active one and activates it.
    pub fn new_tab(&mut self, dir: PathBuf, opts: ListOptions) -> io::Result<()> {
        let mut tab = Tab::new(dir);
        tab.view_mode = self.tab().view_mode;
        tab.sort = self.tab().sort;
        tab.reload(opts)?;
        self.tabs.insert(self.active + 1, tab);
        self.active += 1;
        Ok(())
    }

    /// Duplicates the active tab (location, entries, cursor, mode, sort, history).
    pub fn clone_tab(&mut self) {
        let mut tab = self.tab().clone();
        tab.lock = LockState::Unlocked;
        tab.selected.clear();
        self.tabs.insert(self.active + 1, tab);
        self.active += 1;
    }

    pub fn close_tab(&mut self, ix: usize, force: bool) -> Result<(), CloseTabError> {
        if self.tabs.len() <= 1 {
            return Err(CloseTabError::LastTab);
        }
        if ix >= self.tabs.len() {
            return Ok(());
        }
        if self.tabs[ix].lock.is_locked() && !force {
            return Err(CloseTabError::Locked);
        }
        self.tabs.remove(ix);
        if self.active > ix || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        Ok(())
    }

    /// Closes every unlocked tab except the active one.
    pub fn close_other_tabs(&mut self) {
        let active = self.active;
        let mut new_active = 0;
        let mut kept = Vec::with_capacity(self.tabs.len());
        for (i, tab) in self.tabs.drain(..).enumerate() {
            if i == active {
                new_active = kept.len();
                kept.push(tab);
            } else if tab.lock.is_locked() {
                kept.push(tab);
            }
        }
        self.tabs = kept;
        self.active = new_active;
    }

    pub fn select_tab(&mut self, ix: usize) {
        if ix < self.tabs.len() {
            self.active = ix;
        }
    }

    pub fn next_tab(&mut self) {
        self.active = (self.active + 1) % self.tabs.len();
    }

    pub fn prev_tab(&mut self) {
        self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir(r.join("docs")).unwrap();
        fs::create_dir(r.join("docs/inner")).unwrap();
        fs::write(r.join("a.txt"), b"aaaa").unwrap();
        fs::write(r.join("b.md"), b"bb").unwrap();
        fs::write(r.join("c.txt"), b"c").unwrap();
        fs::write(r.join("docs/readme.txt"), b"r").unwrap();
        tmp
    }

    fn opts() -> ListOptions {
        ListOptions::default()
    }

    fn names(t: &Tab) -> Vec<String> {
        t.entries.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn load_sort_and_stats() {
        let tmp = fixture();
        let mut tab = Tab::new(tmp.path().to_path_buf());
        tab.reload(opts()).unwrap();
        assert_eq!(names(&tab), ["..", "docs", "a.txt", "b.md", "c.txt"]);
        let s = tab.stats();
        assert_eq!((s.total_files, s.total_dirs, s.total_bytes), (3, 1, 7));
        assert_eq!(tab.targets().len(), 0, "cursor on .. gives no targets");
    }

    #[test]
    fn navigation_history_and_go_up_focus() {
        let tmp = fixture();
        let root = tmp.path().to_path_buf();
        let mut tab = Tab::new(root.clone());
        tab.reload(opts()).unwrap();
        tab.navigate(root.join("docs"), opts()).unwrap();
        tab.navigate(root.join("docs/inner"), opts()).unwrap();
        assert_eq!(tab.history.back, vec![root.clone(), root.join("docs")]);

        assert!(tab.go_back(opts()).unwrap());
        assert_eq!(tab.working_dir(), root.join("docs"));
        assert_eq!(tab.cursor_entry().unwrap().name, "inner", "back focuses the folder we came from");
        assert!(tab.go_forward(opts()).unwrap());
        assert_eq!(tab.working_dir(), root.join("docs/inner"));

        tab.go_back(opts()).unwrap();
        tab.navigate(root.clone(), opts()).unwrap();
        assert!(tab.history.forward.is_empty(), "new navigation truncates forward stack");

        tab.navigate(root.join("docs"), opts()).unwrap();
        tab.go_up(opts()).unwrap();
        assert_eq!(tab.working_dir(), root);
        assert_eq!(tab.cursor_entry().unwrap().name, "docs");
    }

    #[test]
    fn failed_navigation_changes_nothing() {
        let tmp = fixture();
        let mut tab = Tab::new(tmp.path().to_path_buf());
        tab.reload(opts()).unwrap();
        let before = tab.working_dir().to_path_buf();
        assert!(tab.navigate(tmp.path().join("missing"), opts()).is_err());
        assert_eq!(tab.working_dir(), before);
        assert!(tab.history.back.is_empty());
    }

    #[test]
    fn selection_model() {
        let tmp = fixture();
        let mut tab = Tab::new(tmp.path().to_path_buf());
        tab.reload(opts()).unwrap();

        assert_eq!(tab.select_mask(&Mask::parse("*.txt"), false), 2);
        assert_eq!(tab.targets().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a.txt", "c.txt"]);
        assert_eq!(tab.unselect_mask(&Mask::parse("a*"), false), 1);
        tab.invert_selection(false);
        let mut sel: Vec<_> = tab.targets().into_iter().map(|e| e.name).collect();
        sel.sort();
        assert_eq!(sel, ["a.txt", "b.md", "docs"]);

        tab.unselect_all();
        tab.select_all(true);
        assert!(tab.targets().iter().all(|e| !e.is_dir_like()), "files-only skips folders");

        tab.unselect_all();
        tab.select_mask(&Mask::parse("*"), true);
        assert_eq!(tab.targets().len(), 3);

        tab.unselect_all();
        tab.set_cursor(0);
        tab.toggle_cursor_and_advance();
        assert!(tab.selected.is_empty(), ".. is never selected");
        assert_eq!(tab.cursor, 1);
        tab.toggle_cursor_and_advance();
        assert_eq!(tab.targets()[0].name, "docs");

        tab.unselect_all();
        tab.set_cursor(2);
        tab.move_cursor(1, true);
        tab.move_cursor(1, true);
        assert_eq!(tab.targets().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["a.txt", "b.md"]);
        assert_eq!(tab.cursor, 4);

        tab.unselect_all();
        tab.select_range(4, 0);
        assert_eq!(tab.targets().len(), 4);
        let s = tab.stats();
        assert_eq!((s.selected_files, s.selected_dirs, s.selected_bytes), (3, 1, 7));
    }

    #[test]
    fn reload_keeps_cursor_and_drops_vanished_selection() {
        let tmp = fixture();
        let mut tab = Tab::new(tmp.path().to_path_buf());
        tab.reload(opts()).unwrap();
        let c = tab.index_of(&tmp.path().join("c.txt")).unwrap();
        tab.set_cursor(c);
        tab.select_mask(&Mask::parse("*.txt"), false);
        fs::remove_file(tmp.path().join("a.txt")).unwrap();
        fs::write(tmp.path().join("0first.txt"), b"").unwrap();
        tab.reload(opts()).unwrap();
        assert_eq!(tab.cursor_entry().unwrap().name, "c.txt");
        assert_eq!(tab.selected.len(), 1);
    }

    #[test]
    fn type_ahead_finds_prefix() {
        let tmp = fixture();
        let mut tab = Tab::new(tmp.path().to_path_buf());
        tab.reload(opts()).unwrap();
        assert!(tab.type_ahead("B"));
        assert_eq!(tab.cursor_entry().unwrap().name, "b.md");
        assert!(!tab.type_ahead("zzz"));
        assert_eq!(tab.cursor_entry().unwrap().name, "b.md");
    }

    #[test]
    fn tabs_locks_and_cycling() {
        let tmp = fixture();
        let root = tmp.path().to_path_buf();
        let mut panel = Panel::new(root.clone());
        panel.tab_mut().reload(opts()).unwrap();
        assert_eq!(panel.close_tab(0, false), Err(CloseTabError::LastTab));

        panel.tab_mut().lock = LockState::Locked;
        panel.navigate(root.join("docs"), opts()).unwrap();
        assert_eq!(panel.tabs.len(), 2, "locked tab spawns a new tab");
        assert_eq!(panel.tabs[0].working_dir(), root);
        assert_eq!(panel.active, 1);
        assert_eq!(panel.tab().working_dir(), root.join("docs"));

        panel.select_tab(0);
        panel.tab_mut().lock = LockState::LockedDirChangesAllowed;
        panel.navigate(root.join("docs"), opts()).unwrap();
        assert_eq!(panel.tabs.len(), 2, "dir changes allowed: navigates in place");
        assert_eq!(panel.close_tab(0, false), Err(CloseTabError::Locked));
        assert_eq!(panel.close_tab(0, true), Ok(()));
        assert_eq!(panel.tabs.len(), 1);

        panel.clone_tab();
        panel.new_tab(root.clone(), opts()).unwrap();
        assert_eq!(panel.tabs.len(), 3);
        assert_eq!(panel.active, 2);
        panel.next_tab();
        assert_eq!(panel.active, 0, "wraps forward");
        panel.prev_tab();
        assert_eq!(panel.active, 2, "wraps backward");

        assert_eq!(panel.folder_history.first(), Some(&root.join("docs")));
    }

    #[test]
    fn open_cursor_semantics() {
        let tmp = fixture();
        let root = tmp.path().to_path_buf();
        fs::create_dir(root.join("Tool.app")).unwrap();
        let mut panel = Panel::new(root.clone());
        panel.tab_mut().reload(opts()).unwrap();
        let app = panel.tab().index_of(&root.join("Tool.app")).unwrap();
        panel.tab_mut().set_cursor(app);
        assert_eq!(panel.open_cursor(opts()).unwrap(), OpenOutcome::OpenFile(root.join("Tool.app")));
        assert_eq!(panel.enter_cursor(opts()).unwrap(), OpenOutcome::Navigated);
        assert_eq!(panel.tab().working_dir(), root.join("Tool.app"));
        panel.tab_mut().set_cursor(0);
        assert_eq!(panel.open_cursor(opts()).unwrap(), OpenOutcome::Navigated);
        assert_eq!(panel.tab().working_dir(), root);
        assert_eq!(panel.tab().cursor_entry().unwrap().name, "Tool.app");
    }

    #[test]
    fn results_location() {
        let tmp = fixture();
        let root = tmp.path().to_path_buf();
        let mut tab = Tab::new(root.clone());
        tab.reload(opts()).unwrap();
        tab.show_results("Search Results".into(), root.clone(), vec![root.join("docs/readme.txt"), root.join("a.txt")], opts());
        assert!(tab.is_results());
        let mut n = names(&tab);
        n.sort();
        assert_eq!(n, ["a.txt", "docs/readme.txt"]);
        tab.go_up(opts()).unwrap();
        assert_eq!(tab.working_dir(), root);
        assert!(!tab.is_results());
    }
}
