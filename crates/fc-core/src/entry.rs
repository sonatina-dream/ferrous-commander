//! Directory entries: what a panel row knows about one file-system item.

use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What kind of item a row represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// The synthetic `..` row.
    Parent,
    Dir,
    File,
    /// A symbolic link; `to_dir` is true when the link resolves to a directory.
    Symlink { to_dir: bool },
}

/// Directory extensions that macOS presents as single documents ("packages").
const PACKAGE_EXTENSIONS: &[&str] = &[
    "app", "appex", "bundle", "framework", "plugin", "kext", "pkg", "mpkg", "xpc", "prefpane",
    "qlgenerator", "mdimporter", "saver", "wdgt", "photoslibrary", "musiclibrary", "rtfd", "pages",
    "numbers", "key", "xcodeproj", "xcworkspace", "playground", "scptd", "band", "logicx",
    "fcpbundle", "imovielibrary", "dsym", "docset", "sparsebundle", "xcarchive",
];

/// One row of a file panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Display name. For search results this can be a relative path.
    pub name: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Byte size for files and links; 0 for directories (see `Tab::entry_size`).
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub added: Option<SystemTime>,
    pub mode: u32,
    pub flags: u32,
    pub uid: u32,
    pub gid: u32,
    pub hidden: bool,
    pub package: bool,
}

impl Entry {
    /// True for the `..` row.
    pub fn is_parent(&self) -> bool {
        self.kind == EntryKind::Parent
    }

    /// True for directories and links that resolve to directories (including packages).
    pub fn is_dir_like(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Dir | EntryKind::Parent | EntryKind::Symlink { to_dir: true }
        )
    }

    /// True when Return should enter the item instead of opening it.
    pub fn is_navigable(&self) -> bool {
        self.is_dir_like() && !self.package
    }

    /// True when the item is grouped with folders by "show folders on top".
    pub fn groups_as_folder(&self) -> bool {
        self.is_dir_like() && !self.package
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, EntryKind::Symlink { .. })
    }

    /// Extension without the dot. Folders (other than packages) and dot-files have none.
    pub fn extension(&self) -> &str {
        if self.is_parent() || (self.is_dir_like() && !self.package) {
            return "";
        }
        split_extension(&self.name).1
    }

    /// Name without its extension (see [`Entry::extension`]).
    pub fn stem(&self) -> &str {
        if self.extension().is_empty() {
            &self.name
        } else {
            split_extension(&self.name).0
        }
    }

    pub fn is_executable(&self) -> bool {
        self.kind == EntryKind::File && self.mode & 0o111 != 0
    }

    /// Short human-readable kind, used by the "kind" column and Get Info.
    pub fn kind_label(&self) -> String {
        match self.kind {
            EntryKind::Parent => return "Parent folder".into(),
            EntryKind::Symlink { to_dir: true } => return "Alias (folder)".into(),
            EntryKind::Symlink { to_dir: false } => return "Symbolic link".into(),
            EntryKind::Dir if !self.package => return "Folder".into(),
            _ => {}
        }
        let ext = self.extension().to_ascii_lowercase();
        if self.package {
            return match ext.as_str() {
                "app" => "Application".into(),
                "framework" => "Framework".into(),
                "bundle" | "plugin" | "appex" => "Bundle".into(),
                _ => "Package".into(),
            };
        }
        if let Some(label) = kind_for_extension(&ext) {
            return label.into();
        }
        if ext.is_empty() {
            if self.is_executable() {
                "Unix executable".into()
            } else {
                "Document".into()
            }
        } else {
            format!("{} file", ext.to_ascii_uppercase())
        }
    }
}

/// Splits `name` into (stem, extension) on the last dot that is not the first character.
pub fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) if i + 1 == name.len() => (name, ""),
        Some(i) => (&name[..i], &name[i + 1..]),
    }
}

fn kind_for_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "txt" | "text" => "Plain text",
        "md" | "markdown" => "Markdown document",
        "rtf" => "Rich text document",
        "pdf" => "PDF document",
        "doc" | "docx" => "Word document",
        "xls" | "xlsx" => "Excel spreadsheet",
        "ppt" | "pptx" => "PowerPoint presentation",
        "csv" => "CSV document",
        "json" => "JSON document",
        "xml" | "plist" => "XML document",
        "yaml" | "yml" | "toml" | "ini" | "cfg" | "conf" => "Configuration file",
        "html" | "htm" => "HTML document",
        "css" => "CSS stylesheet",
        "js" | "mjs" | "cjs" => "JavaScript source",
        "ts" | "tsx" => "TypeScript source",
        "rs" => "Rust source",
        "swift" => "Swift source",
        "c" | "h" => "C source",
        "cpp" | "cc" | "hpp" | "cxx" => "C++ source",
        "m" | "mm" => "Objective-C source",
        "py" => "Python script",
        "rb" => "Ruby script",
        "go" => "Go source",
        "java" => "Java source",
        "kt" => "Kotlin source",
        "sh" | "zsh" | "bash" | "command" => "Shell script",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "heic" | "heif" => "HEIF image",
        "webp" => "WebP image",
        "tif" | "tiff" => "TIFF image",
        "bmp" => "BMP image",
        "svg" => "SVG image",
        "ico" | "icns" => "Icon",
        "psd" => "Photoshop document",
        "mp3" => "MP3 audio",
        "m4a" | "aac" => "AAC audio",
        "wav" | "aiff" | "aif" | "flac" => "Audio",
        "mp4" | "m4v" => "MPEG-4 movie",
        "mov" => "QuickTime movie",
        "mkv" | "avi" | "webm" => "Video",
        "zip" => "ZIP archive",
        "gz" | "tgz" => "Gzip archive",
        "tar" => "Tar archive",
        "bz2" | "xz" | "7z" | "rar" => "Archive",
        "dmg" => "Disk image",
        "iso" => "ISO disk image",
        "ttf" | "otf" | "woff" | "woff2" => "Font",
        "log" => "Log file",
        "sqlite" | "db" => "Database",
        "dylib" | "so" | "a" => "Library",
        _ => return None,
    })
}

fn is_package_dir(name: &str) -> bool {
    let ext = split_extension(name).1;
    !ext.is_empty() && PACKAGE_EXTENSIONS.iter().any(|p| p.eq_ignore_ascii_case(ext))
}

#[cfg(target_os = "macos")]
fn st_flags(meta: &Metadata) -> u32 {
    use std::os::macos::fs::MetadataExt as _;
    meta.st_flags()
}

#[cfg(not(target_os = "macos"))]
fn st_flags(_meta: &Metadata) -> u32 {
    0
}

/// Date the item was added to its folder (macOS `ATTR_CMN_ADDEDTIME`).
#[cfg(target_os = "macos")]
pub fn added_time(path: &Path) -> Option<SystemTime> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: attrlist is a plain C struct; all-zero is a valid initial state.
    let mut request: libc::attrlist = unsafe { std::mem::zeroed() };
    request.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    request.commonattr = libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_ADDEDTIME;
    // Layout: u32 length, attribute_set_t (5 × u32), timespec (2 × i64).
    let mut buf = [0u8; 64];
    // SAFETY: the buffer is large enough for the requested attributes and outlives the call.
    let rc = unsafe {
        libc::getattrlist(
            c_path.as_ptr(),
            &mut request as *mut libc::attrlist as *mut libc::c_void,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            libc::FSOPT_NOFOLLOW,
        )
    };
    if rc != 0 {
        return None;
    }
    let returned_common = u32::from_ne_bytes(buf[4..8].try_into().ok()?);
    if returned_common & libc::ATTR_CMN_ADDEDTIME == 0 {
        return None;
    }
    let secs = i64::from_ne_bytes(buf[24..32].try_into().ok()?);
    let nanos = i64::from_ne_bytes(buf[32..40].try_into().ok()?);
    if secs < 0 {
        return None;
    }
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::new(secs as u64, nanos.clamp(0, 999_999_999) as u32))
}

#[cfg(not(target_os = "macos"))]
pub fn added_time(_path: &Path) -> Option<SystemTime> {
    None
}

/// Builds an [`Entry`] for `path` without following a final symlink.
pub fn read_entry(path: &Path) -> io::Result<Entry> {
    let meta = fs::symlink_metadata(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    Ok(entry_from_metadata(name, path.to_path_buf(), &meta))
}

fn entry_from_metadata(name: String, path: PathBuf, meta: &Metadata) -> Entry {
    let file_type = meta.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Symlink {
            to_dir: fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false),
        }
    } else if file_type.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    let flags = st_flags(meta);
    let package = kind == EntryKind::Dir && is_package_dir(&name);
    let hidden = name.starts_with('.') || flags & libc::UF_HIDDEN != 0;
    Entry {
        size: if kind == EntryKind::Dir { 0 } else { meta.len() },
        modified: meta.modified().ok(),
        created: meta.created().ok(),
        accessed: meta.accessed().ok(),
        added: added_time(&path),
        mode: meta.mode(),
        flags,
        uid: meta.uid(),
        gid: meta.gid(),
        hidden,
        package,
        kind,
        name,
        path,
    }
}

/// The synthetic `..` row for `dir`, or `None` at the file-system root.
pub fn parent_entry(dir: &Path) -> Option<Entry> {
    let parent = dir.parent()?;
    let meta = fs::metadata(parent).ok();
    Some(Entry {
        name: "..".into(),
        path: parent.to_path_buf(),
        kind: EntryKind::Parent,
        size: 0,
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        created: None,
        accessed: None,
        added: None,
        mode: meta.as_ref().map(|m| m.mode()).unwrap_or(0o040755),
        flags: 0,
        uid: 0,
        gid: 0,
        hidden: false,
        package: false,
    })
}

/// Lists `dir` (without `..`). Hidden items are skipped unless `show_hidden`.
/// Items that vanish or cannot be stat'ed while listing are skipped.
pub fn list_dir(dir: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let path = item.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        let entry = entry_from_metadata(name, path, &meta);
        if entry.hidden && !show_hidden {
            continue;
        }
        out.push(entry);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_rules() {
        assert_eq!(split_extension("archive.tar.gz"), ("archive.tar", "gz"));
        assert_eq!(split_extension(".zshrc"), (".zshrc", ""));
        assert_eq!(split_extension("README"), ("README", ""));
        assert_eq!(split_extension("trailing."), ("trailing.", ""));
    }

    #[test]
    fn lists_and_classifies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), b"hello").unwrap();
        fs::write(root.join(".hidden"), b"x").unwrap();
        fs::create_dir(root.join("Folder")).unwrap();
        fs::create_dir(root.join("Tool.app")).unwrap();
        std::os::unix::fs::symlink(root.join("Folder"), root.join("link")).unwrap();

        let visible = list_dir(root, false).unwrap();
        let names: Vec<_> = visible.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&".hidden"));
        assert_eq!(list_dir(root, true).unwrap().len(), 5);

        let find = |n: &str| visible.iter().find(|e| e.name == n).unwrap().clone();
        let txt = find("a.txt");
        assert_eq!(txt.kind, EntryKind::File);
        assert_eq!(txt.size, 5);
        assert_eq!(txt.extension(), "txt");
        assert_eq!(txt.kind_label(), "Plain text");

        let app = find("Tool.app");
        assert!(app.package && app.is_dir_like() && !app.is_navigable());
        assert_eq!(app.kind_label(), "Application");

        let link = find("link");
        assert_eq!(link.kind, EntryKind::Symlink { to_dir: true });
        assert!(link.is_navigable());

        let folder = find("Folder");
        assert_eq!(folder.extension(), "");
        assert_eq!(folder.kind_label(), "Folder");
    }

    #[test]
    fn parent_row() {
        assert!(parent_entry(Path::new("/")).is_none());
        let p = parent_entry(Path::new("/usr/bin")).unwrap();
        assert!(p.is_parent());
        assert_eq!(p.path, PathBuf::from("/usr"));
    }
}
