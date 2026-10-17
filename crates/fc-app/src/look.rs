//! Shared visual language: file-type tiles, keycaps, chips, buttons and shadows.

use gpui::{div, point, prelude::*, px, svg, BoxShadow, Div, Hsla, SharedString, Stateful, Svg};

use fc_core::keymap::Action;
use fc_core::{Entry, EntryKind};

use crate::colors::{hsla, Colors};
use crate::icons;

/// Visual category of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Parent,
    Folder,
    App,
    Package,
    Image,
    Media,
    Archive,
    Code,
    Pdf,
    Doc,
    Link,
    Exec,
    Other,
}

pub fn kind_of(e: &Entry) -> FileKind {
    match e.kind {
        EntryKind::Parent => return FileKind::Parent,
        EntryKind::Symlink { .. } => return FileKind::Link,
        EntryKind::Dir if e.package && e.extension().eq_ignore_ascii_case("app") => return FileKind::App,
        EntryKind::Dir if e.package => return FileKind::Package,
        EntryKind::Dir => return FileKind::Folder,
        EntryKind::File => {}
    }
    match e.extension().to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp" | "svg" | "ico" | "icns" | "psd" | "raw" => FileKind::Image,
        "mp3" | "m4a" | "aac" | "wav" | "aiff" | "flac" | "ogg" | "mp4" | "m4v" | "mov" | "mkv" | "avi" | "webm" => FileKind::Media,
        "zip" | "gz" | "tgz" | "tar" | "bz2" | "xz" | "7z" | "rar" | "dmg" | "iso" | "pkg" => FileKind::Archive,
        "pdf" => FileKind::Pdf,
        "txt" | "md" | "markdown" | "rtf" | "doc" | "docx" | "pages" | "csv" | "log" | "xls" | "xlsx" | "key" | "ppt" | "pptx" => FileKind::Doc,
        "rs" | "swift" | "c" | "h" | "cpp" | "hpp" | "cc" | "m" | "mm" | "py" | "rb" | "go" | "java" | "kt" | "js" | "mjs" | "ts" | "tsx" | "jsx"
        | "json" | "toml" | "yaml" | "yml" | "xml" | "html" | "css" | "scss" | "sh" | "zsh" | "bash" | "plist" | "lock" | "sql" | "lua" => FileKind::Code,
        _ if e.is_executable() => FileKind::Exec,
        _ => FileKind::Other,
    }
}

impl FileKind {
    pub fn icon(self) -> &'static str {
        match self {
            FileKind::Parent => icons::UP,
            FileKind::Folder => icons::FOLDER,
            FileKind::App => icons::APP,
            FileKind::Package => icons::ZIP,
            FileKind::Image => icons::IMAGE,
            FileKind::Media => icons::MEDIA,
            FileKind::Archive => icons::ARCHIVE,
            FileKind::Code => icons::FILE_CODE,
            FileKind::Pdf | FileKind::Doc => icons::FILE_TEXT,
            FileKind::Link => icons::LINK,
            FileKind::Exec => icons::TERMINAL,
            FileKind::Other => icons::FILE,
        }
    }

    /// Muted hue per type: soft pastels on dark themes, deeper tones on light ones.
    pub fn tint(self, c: &Colors) -> Hsla {
        let (dark, light) = match self {
            FileKind::Parent | FileKind::Other => return c.muted,
            FileKind::Folder => return c.folder,
            FileKind::App => (0xb3abd9, 0x5d52a0),
            FileKind::Package => (0xaab0d6, 0x5a63a3),
            FileKind::Image => (0xe0a3c2, 0xa84f7b),
            FileKind::Media => (0xb9a6e0, 0x6f56b0),
            FileKind::Archive => (0xdcc08a, 0x8f6a22),
            FileKind::Code => (0x93cfae, 0x2f7d55),
            FileKind::Pdf => (0xe39a9a, 0xa83a3a),
            FileKind::Doc => (0x9eb8dc, 0x3f66a0),
            FileKind::Link => (0x92cbc7, 0x2f7f7a),
            FileKind::Exec => (0xe0b08a, 0x9c5a28),
        };
        hsla(if c.dark { dark } else { light })
    }
}

pub fn icon(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

/// Rounded, tinted square holding a file-type glyph.
pub fn file_tile(kind: FileKind, size: f32, c: &Colors, on_cursor: bool) -> Div {
    let tint = kind.tint(c);
    let (bg, fg) = if on_cursor {
        (c.cursor_text.opacity(0.12), c.cursor_text)
    } else {
        (tint.opacity(if c.dark { 0.17 } else { 0.13 }), tint)
    };
    div()
        .size(px(size))
        .flex_none()
        .rounded(px((size * 0.28).round()))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .child(icon(kind.icon(), (size * 0.62).round(), fg))
}

/// Small key label, e.g. "F5" or "⌘P".
pub fn keycap(text: impl Into<SharedString>, c: &Colors) -> Div {
    div()
        .px(px(5.))
        .h(px(17.))
        .min_w(px(17.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(c.border)
        .border_b_2()
        .bg(c.sunken)
        .text_size(px(10.5))
        .text_color(c.muted)
        .font_family("Menlo")
        .child(text.into())
}

/// Rounded label chip.
pub fn chip(text: impl Into<SharedString>, c: &Colors, accent: bool) -> Div {
    div()
        .px(px(8.))
        .h(px(20.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded_full()
        .when(accent, |d| d.bg(c.accent.opacity(0.16)).text_color(c.accent))
        .when(!accent, |d| d.bg(c.sunken).text_color(c.muted))
        .text_size(px(11.))
        .whitespace_nowrap()
        .child(text.into())
}

/// Round, borderless icon button.
pub fn ghost_button(id: impl Into<SharedString>, icon_path: &'static str, c: &Colors, active: bool, enabled: bool) -> Stateful<Div> {
    let id: SharedString = id.into();
    div()
        .id(id)
        .size(px(30.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(9.))
        .when(active, |d| d.bg(c.accent.opacity(0.18)))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(c.hover)))
        .child(icon(icon_path, 16., if !enabled { c.muted.opacity(0.45) } else if active { c.accent } else { c.chrome_text }))
}

/// Uppercase section caption (sidebar, palette).
pub fn section_label(text: &str, c: &Colors) -> Div {
    div()
        .px(px(10.))
        .pt(px(14.))
        .pb(px(5.))
        .text_size(px(10.5))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(c.muted.opacity(0.85))
        .child(text.to_uppercase())
}

pub fn soft_shadow(c: &Colors) -> Vec<BoxShadow> {
    vec![BoxShadow { color: c.shadow, offset: point(px(0.), px(8.)), blur_radius: px(24.), spread_radius: px(-6.) }]
}

/// Card shadow; `glow` (0–1) adds an accent halo for the active panel.
pub fn card_shadow(c: &Colors, glow: f32) -> Vec<BoxShadow> {
    let mut v = soft_shadow(c);
    if glow > 0.0 {
        let strength = if c.dark { 0.14 } else { 0.2 };
        v.push(BoxShadow { color: c.accent.opacity(strength * glow), offset: point(px(0.), px(0.)), blur_radius: px(22.), spread_radius: px(0.) });
    }
    v
}

/// Glyph for a command, used by menus and the command palette.
pub fn action_icon(a: Action) -> &'static str {
    use Action as A;
    match a {
        A::CopyFiles | A::Copy | A::Queue | A::CopyItemName | A::CopyItemPath => icons::COPY,
        A::CutFiles => icons::SCISSORS,
        A::PasteFiles => icons::CLIPBOARD,
        A::MoveHere => icons::MOVE,
        A::Move | A::Rename => icons::MOVE,
        A::Delete | A::DeleteForce => icons::TRASH,
        A::NewFolder => icons::FOLDER_PLUS,
        A::NewFile => icons::FILE_PLUS,
        A::View | A::QuickLook => icons::EYE,
        A::Edit => icons::PEN,
        A::OpenItem | A::EnterItem => icons::FOLDER_OPEN,
        A::Compress | A::CompressWithOptions | A::Uncompress => icons::ZIP,
        A::ShowInfo | A::ShowFinderInfo => icons::INFO,
        A::Search | A::SearchSpotlight => icons::SEARCH,
        A::ShowTerminal | A::OpenTerminalApp | A::PasteItemName | A::PasteItemPath | A::ToggleCommandLine => icons::TERMINAL,
        A::GoHome => icons::HOME,
        A::GoDesktop => icons::DESKTOP,
        A::GoDownloads => icons::DOWNLOAD,
        A::GoDocuments => icons::DOCS,
        A::GoApplications | A::GoSystemApplications | A::GoUtilities => icons::APPS,
        A::GoBack => icons::BACK,
        A::GoForward => icons::FORWARD,
        A::GoTop => icons::UP,
        A::GoServer | A::ShowFTP => icons::SERVER,
        A::ShowFavorites => icons::STAR,
        A::Eject | A::ChangeDriveLeft | A::ChangeDriveRight => icons::DISK,
        A::ReloadContent => icons::REFRESH,
        A::ToggleHidden => icons::HIDDEN,
        A::ViewModeFull => icons::LIST,
        A::ViewModeBrief => icons::COLUMNS,
        A::ViewModeThumbs => icons::GRID,
        A::SwapPanels | A::SwitchPanel | A::EqualizePanels => icons::SWAP,
        A::ToggleDiskList => icons::SIDEBAR,
        A::ToggleRightPanel => icons::COLUMNS,
        A::ShowQueue => icons::QUEUE,
        A::Preferences => icons::GEAR,
        A::MakeLink | A::MakeAlias => icons::LINK,
        A::ToggleTabLock => icons::LOCK,
        A::NewTab | A::CloneTab | A::NewWindow => icons::PLUS,
        A::CloseTab => icons::CLOSE,
        A::Select | A::SelectAll | A::SelectItem | A::SelectItemWithSize | A::InvertSelect | A::Unselect | A::UnselectAll => icons::CHECK,
        A::FolderSize => icons::DISK,
        _ => icons::SPARKLE,
    }
}
