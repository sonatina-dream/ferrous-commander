//! Sorting of panel entries (SPEC §2.3).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::entry::Entry;

/// Column a panel can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    Name,
    Extension,
    Size,
    /// Modification date.
    Date,
    Created,
    Added,
    Accessed,
    Kind,
    Permissions,
}

impl SortKey {
    /// All keys in the order of their ⌥⌘1…⌥⌘9 hotkeys.
    pub const ALL: [SortKey; 9] = [
        SortKey::Name,
        SortKey::Extension,
        SortKey::Size,
        SortKey::Date,
        SortKey::Created,
        SortKey::Added,
        SortKey::Accessed,
        SortKey::Kind,
        SortKey::Permissions,
    ];

    /// Stable identifier, shared with the session file and column ids.
    pub fn id(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Extension => "extension",
            SortKey::Size => "size",
            SortKey::Date => "date",
            SortKey::Created => "created",
            SortKey::Added => "added",
            SortKey::Accessed => "accessed",
            SortKey::Kind => "kind",
            SortKey::Permissions => "permissions",
        }
    }

    pub fn from_id(id: &str) -> Option<SortKey> {
        SortKey::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Column header caption.
    pub fn header(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Extension => "Ext",
            SortKey::Size => "Size",
            SortKey::Date => "Modified",
            SortKey::Created => "Created",
            SortKey::Added => "Added",
            SortKey::Accessed => "Opened",
            SortKey::Kind => "Kind",
            SortKey::Permissions => "Permissions",
        }
    }
}

/// Sort key plus direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    pub key: SortKey,
    pub ascending: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec { key: SortKey::Name, ascending: true }
    }
}

impl SortSpec {
    /// Header-click / hotkey semantics: the same key flips direction, a new key sorts ascending.
    pub fn toggled(self, key: SortKey) -> SortSpec {
        if self.key == key {
            SortSpec { key, ascending: !self.ascending }
        } else {
            SortSpec { key, ascending: true }
        }
    }
}

/// Case-insensitive "natural" comparison: digit runs compare by numeric value,
/// so `file2` < `file10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let mut da = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    da.push(c);
                    ai.next();
                }
                let mut db = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    db.push(c);
                    bi.next();
                }
                let ta = da.trim_start_matches('0');
                let tb = db.trim_start_matches('0');
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                let la = ca.to_lowercase();
                let lb = cb.to_lowercase();
                let ord = la.cmp(lb);
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
    // Equal ignoring case: fall back to a case-sensitive, deterministic order.
    a.cmp(b)
}

fn compare_by_key(
    a: &Entry,
    b: &Entry,
    key: SortKey,
    dir_sizes: &HashMap<PathBuf, u64>,
) -> Ordering {
    let size = |e: &Entry| {
        if e.groups_as_folder() || e.package {
            dir_sizes.get(&e.path).copied().unwrap_or(0)
        } else {
            e.size
        }
    };
    match key {
        SortKey::Name => Ordering::Equal,
        SortKey::Extension => natural_cmp(a.extension(), b.extension()),
        SortKey::Size => size(a).cmp(&size(b)),
        SortKey::Date => a.modified.cmp(&b.modified),
        SortKey::Created => a.created.cmp(&b.created),
        SortKey::Added => a.added.cmp(&b.added),
        SortKey::Accessed => a.accessed.cmp(&b.accessed),
        SortKey::Kind => a.kind_label().cmp(&b.kind_label()),
        SortKey::Permissions => (a.mode & 0o7777).cmp(&(b.mode & 0o7777)),
    }
}

/// Sorts `entries` in place. The `..` row always stays first; with `folders_first`
/// folders precede files and each group is sorted on its own.
pub fn sort_entries(
    entries: &mut [Entry],
    spec: SortSpec,
    folders_first: bool,
    dir_sizes: &HashMap<PathBuf, u64>,
) {
    entries.sort_by(|a, b| {
        if a.is_parent() != b.is_parent() {
            return if a.is_parent() { Ordering::Less } else { Ordering::Greater };
        }
        if folders_first && a.groups_as_folder() != b.groups_as_folder() {
            return if a.groups_as_folder() { Ordering::Less } else { Ordering::Greater };
        }
        let primary = compare_by_key(a, b, spec.key, dir_sizes);
        let ord = if primary == Ordering::Equal {
            natural_cmp(&a.name, &b.name)
        } else {
            primary
        };
        if spec.ascending { ord } else { ord.reverse() }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryKind;
    use std::time::{Duration, SystemTime};

    fn e(name: &str, kind: EntryKind, size: u64, age: u64) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            kind,
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(age)),
            created: None,
            accessed: None,
            added: None,
            mode: if kind == EntryKind::Dir { 0o40755 } else { 0o100644 },
            flags: 0,
            uid: 0,
            gid: 0,
            hidden: false,
            package: false,
        }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a", "B"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "B", "file1", "File2", "file10"]);
        // Numerically equal runs still order deterministically.
        assert_ne!(natural_cmp("x007", "x7"), Ordering::Equal);
        assert_eq!(natural_cmp("x007", "x7"), natural_cmp("x007", "x7"));
    }

    #[test]
    fn parent_first_and_folders_on_top() {
        let mut v = vec![
            e("zeta.txt", EntryKind::File, 1, 1),
            e("Alpha", EntryKind::Dir, 0, 1),
            e("..", EntryKind::Parent, 0, 1),
            e("beta.md", EntryKind::File, 3, 1),
            e("omega", EntryKind::Dir, 0, 1),
        ];
        let none = HashMap::new();
        sort_entries(&mut v, SortSpec::default(), true, &none);
        assert_eq!(names(&v), ["..", "Alpha", "omega", "beta.md", "zeta.txt"]);

        sort_entries(&mut v, SortSpec { key: SortKey::Name, ascending: false }, true, &none);
        assert_eq!(names(&v), ["..", "omega", "Alpha", "zeta.txt", "beta.md"]);

        sort_entries(&mut v, SortSpec::default(), false, &none);
        assert_eq!(names(&v), ["..", "Alpha", "beta.md", "omega", "zeta.txt"]);
    }

    #[test]
    fn size_and_date_keys_with_name_tiebreak() {
        let mut v = vec![
            e("b", EntryKind::File, 10, 3),
            e("a", EntryKind::File, 10, 2),
            e("c", EntryKind::File, 5, 1),
        ];
        let none = HashMap::new();
        sort_entries(&mut v, SortSpec { key: SortKey::Size, ascending: true }, true, &none);
        assert_eq!(names(&v), ["c", "a", "b"]);
        sort_entries(&mut v, SortSpec { key: SortKey::Date, ascending: false }, true, &none);
        assert_eq!(names(&v), ["b", "a", "c"]);
    }

    #[test]
    fn computed_folder_sizes_participate() {
        let mut v = vec![e("small", EntryKind::Dir, 0, 1), e("big", EntryKind::Dir, 0, 1)];
        let mut sizes = HashMap::new();
        sizes.insert(PathBuf::from("/x/big"), 900);
        sizes.insert(PathBuf::from("/x/small"), 10);
        sort_entries(&mut v, SortSpec { key: SortKey::Size, ascending: false }, true, &sizes);
        assert_eq!(names(&v), ["big", "small"]);
    }

    #[test]
    fn toggling() {
        let s = SortSpec::default();
        assert_eq!(s.toggled(SortKey::Name), SortSpec { key: SortKey::Name, ascending: false });
        assert_eq!(s.toggled(SortKey::Size), SortSpec { key: SortKey::Size, ascending: true });
        for k in SortKey::ALL {
            assert_eq!(SortKey::from_id(k.id()), Some(k));
        }
    }
}
