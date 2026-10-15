//! Inline SVG icons (original line art) served through GPUI's asset pipeline so
//! `svg()` can tint them with the current text colour.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub struct Assets;

macro_rules! icon {
    ($body:literal) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        )
    };
}

macro_rules! filled {
    ($body:literal) => {
        concat!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="black">"#, $body, "</svg>")
    };
}

pub const FOLDER: &str = "icons/folder.svg";
pub const FILE: &str = "icons/file.svg";
pub const FILE_TEXT: &str = "icons/file-text.svg";
pub const FILE_CODE: &str = "icons/file-code.svg";
pub const IMAGE: &str = "icons/image.svg";
pub const ARCHIVE: &str = "icons/archive.svg";
pub const MEDIA: &str = "icons/media.svg";
pub const APP: &str = "icons/app.svg";
pub const LINK: &str = "icons/link.svg";
pub const UP: &str = "icons/up.svg";
pub const BACK: &str = "icons/back.svg";
pub const FORWARD: &str = "icons/forward.svg";
pub const LIST: &str = "icons/list.svg";
pub const COLUMNS: &str = "icons/columns.svg";
pub const GRID: &str = "icons/grid.svg";
pub const EYE: &str = "icons/eye.svg";
pub const INFO: &str = "icons/info.svg";
pub const SEARCH: &str = "icons/search.svg";
pub const ZIP: &str = "icons/zip.svg";
pub const TERMINAL: &str = "icons/terminal.svg";
pub const QUEUE: &str = "icons/queue.svg";
pub const HIDDEN: &str = "icons/hidden.svg";
pub const STAR: &str = "icons/star.svg";
pub const CLOCK: &str = "icons/clock.svg";
pub const EJECT: &str = "icons/eject.svg";
pub const DRIVE: &str = "icons/drive.svg";
pub const LOCK: &str = "icons/lock.svg";
pub const CLOSE: &str = "icons/close.svg";
pub const PLUS: &str = "icons/plus.svg";
pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
pub const QUICKLOOK: &str = "icons/quicklook.svg";
pub const GEAR: &str = "icons/gear.svg";
pub const HOME: &str = "icons/home.svg";
pub const DESKTOP: &str = "icons/desktop.svg";
pub const DOWNLOAD: &str = "icons/download.svg";
pub const DOCS: &str = "icons/docs.svg";
pub const APPS: &str = "icons/apps.svg";
pub const DISK: &str = "icons/disk.svg";
pub const COMMAND: &str = "icons/command.svg";
pub const COPY: &str = "icons/copy.svg";
pub const MOVE: &str = "icons/move.svg";
pub const TRASH: &str = "icons/trash.svg";
pub const CHECK: &str = "icons/check.svg";
pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
pub const STAR_FILL: &str = "icons/star-fill.svg";
pub const SPARKLE: &str = "icons/sparkle.svg";
pub const FOLDER_PLUS: &str = "icons/folder-plus.svg";
pub const FILE_PLUS: &str = "icons/file-plus.svg";
pub const REFRESH: &str = "icons/refresh.svg";
pub const SIDEBAR: &str = "icons/sidebar.svg";
pub const WARNING: &str = "icons/warning.svg";
pub const PEN: &str = "icons/pen.svg";
pub const SERVER: &str = "icons/server.svg";
pub const TAG: &str = "icons/tag.svg";
pub const SWAP: &str = "icons/swap.svg";
pub const PROMPT: &str = "icons/prompt.svg";
pub const FOLDER_OPEN: &str = "icons/folder-open.svg";
pub const SCISSORS: &str = "icons/scissors.svg";
pub const CLIPBOARD: &str = "icons/clipboard.svg";

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let svg: &'static str = match path {
            FOLDER => filled!(r#"<path d="M2.5 6.5a2 2 0 0 1 2-2h4.6l2 2.2h8.4a2 2 0 0 1 2 2v9.8a2 2 0 0 1-2 2h-15a2 2 0 0 1-2-2z"/>"#),
            FILE => icon!(r#"<path d="M6 2.8h8l4.5 4.5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-16.5a1 1 0 0 1 1-1z"/><path d="M14 2.8v4.5h4.5"/>"#),
            FILE_TEXT => icon!(r#"<path d="M6 2.8h8l4.5 4.5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-16.5a1 1 0 0 1 1-1z"/><path d="M14 2.8v4.5h4.5M8.5 12h7M8.5 15.5h7M8.5 8.5h3"/>"#),
            FILE_CODE => icon!(r#"<path d="M6 2.8h8l4.5 4.5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-16.5a1 1 0 0 1 1-1z"/><path d="M10 11.5l-2.2 2.3 2.2 2.3M14 11.5l2.2 2.3-2.2 2.3"/>"#),
            IMAGE => icon!(r#"<rect x="3" y="4.5" width="18" height="15" rx="2"/><circle cx="8.5" cy="9.5" r="1.6"/><path d="M21 16l-5-5-8.5 8.5"/>"#),
            ARCHIVE => icon!(r#"<rect x="4" y="3" width="16" height="18" rx="1.5"/><path d="M12 3v2M12 7v2M12 11v2"/><rect x="10.3" y="13" width="3.4" height="4" rx="0.8"/>"#),
            MEDIA => icon!(r#"<rect x="3" y="4.5" width="18" height="15" rx="2"/><path d="M10 9l5 3-5 3z"/>"#),
            APP => icon!(r#"<rect x="3.5" y="3.5" width="17" height="17" rx="4"/><path d="M8.5 15.5l3.5-7 3.5 7M9.8 13h4.4"/>"#),
            LINK => icon!(r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#),
            UP => icon!(r#"<path d="M12 19V5M5.5 11.5L12 5l6.5 6.5"/>"#),
            BACK => icon!(r#"<path d="M15 5l-7 7 7 7"/>"#),
            FORWARD => icon!(r#"<path d="M9 5l7 7-7 7"/>"#),
            LIST => icon!(r#"<path d="M8.5 6.5h12M8.5 12h12M8.5 17.5h12"/><path d="M4 6.5h.5M4 12h.5M4 17.5h.5"/>"#),
            COLUMNS => icon!(r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M15 4v16"/>"#),
            GRID => icon!(r#"<rect x="3.5" y="3.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="3.5" width="7" height="7" rx="1.5"/><rect x="3.5" y="13.5" width="7" height="7" rx="1.5"/><rect x="13.5" y="13.5" width="7" height="7" rx="1.5"/>"#),
            EYE => icon!(r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>"#),
            HIDDEN => icon!(r#"<path d="M3 3l18 18"/><path d="M10.6 5.6A9.6 9.6 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a17 17 0 0 1-3 3.8M6.5 7.4A17 17 0 0 0 2.5 12S6 18.5 12 18.5a9 9 0 0 0 4.2-1"/><path d="M9.9 9.9a3 3 0 0 0 4.2 4.2"/>"#),
            INFO => icon!(r#"<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.5v.5"/>"#),
            SEARCH => icon!(r#"<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.3-4.3"/>"#),
            ZIP => icon!(r#"<path d="M4 7.5L12 3l8 4.5v9L12 21l-8-4.5z"/><path d="M4 7.5l8 4.5 8-4.5M12 12v9"/>"#),
            TERMINAL => icon!(r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M7 9l3 3-3 3M12.5 15H17"/>"#),
            QUEUE => icon!(r#"<path d="M4 6h16M4 12h10M4 18h7"/><path d="M17 14v6M14 17h6"/>"#),
            STAR => icon!(r#"<path d="M12 3.5l2.6 5.4 5.9.8-4.3 4.1 1 5.8L12 16.8l-5.2 2.8 1-5.8-4.3-4.1 5.9-.8z"/>"#),
            CLOCK => icon!(r#"<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>"#),
            EJECT => icon!(r#"<path d="M12 5l7 8H5z"/><path d="M5 18h14"/>"#),
            DRIVE => icon!(r#"<rect x="3" y="7" width="18" height="10" rx="2"/><path d="M7 14h.5M3 12h18"/>"#),
            LOCK => icon!(r#"<rect x="5" y="11" width="14" height="9.5" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>"#),
            CLOSE => icon!(r#"<path d="M7 7l10 10M17 7L7 17"/>"#),
            PLUS => icon!(r#"<path d="M12 5.5v13M5.5 12h13"/>"#),
            CHEVRON_DOWN => icon!(r#"<path d="M6.5 9.5L12 15l5.5-5.5"/>"#),
            QUICKLOOK => icon!(r#"<circle cx="10.5" cy="10.5" r="6.5"/><path d="M20 20l-4.6-4.6"/><path d="M7.8 10.5s1-2 2.7-2 2.7 2 2.7 2-1 2-2.7 2-2.7-2-2.7-2z"/>"#),
            GEAR => icon!(r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>"#),
            HOME => icon!(r#"<path d="M3.5 10.5L12 4l8.5 6.5V19a1.5 1.5 0 0 1-1.5 1.5h-4v-6h-6v6H5A1.5 1.5 0 0 1 3.5 19z"/>"#),
            DESKTOP => icon!(r#"<rect x="3" y="4" width="18" height="12.5" rx="2"/><path d="M8.5 20.5h7M12 16.5v4"/>"#),
            DOWNLOAD => icon!(r#"<path d="M12 4v11M7 10.5l5 5 5-5"/><path d="M4.5 19.5h15"/>"#),
            DOCS => icon!(r#"<path d="M7 3.5h7l4.5 4.5v11a1.5 1.5 0 0 1-1.5 1.5H7A1.5 1.5 0 0 1 5.5 19V5A1.5 1.5 0 0 1 7 3.5z"/><path d="M9 12.5h6M9 16h6"/>"#),
            APPS => icon!(r#"<rect x="4" y="4" width="6.5" height="6.5" rx="1.8"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.8"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.8"/><circle cx="16.75" cy="16.75" r="3.25"/>"#),
            DISK => icon!(r#"<rect x="3" y="5" width="18" height="14" rx="3"/><path d="M3 14.5h18"/><circle cx="17" cy="16.8" r=".6"/>"#),
            COMMAND => icon!(r#"<path d="M9 9V6.5A2.5 2.5 0 1 0 6.5 9H9zm0 0h6m-6 0v6m6-6V6.5A2.5 2.5 0 1 1 17.5 9H15zm0 0v6m0 0h-6m6 0v2.5a2.5 2.5 0 1 0 2.5-2.5H15zm-6 0H6.5A2.5 2.5 0 1 0 9 17.5V15z"/>"#),
            COPY => icon!(r#"<rect x="8.5" y="8.5" width="11.5" height="11.5" rx="2"/><path d="M15.5 8.5V6A2 2 0 0 0 13.5 4H6a2 2 0 0 0-2 2v7.5a2 2 0 0 0 2 2h2.5"/>"#),
            MOVE => icon!(r#"<path d="M4 12h13M13 7l5 5-5 5"/><path d="M20.5 5v14"/>"#),
            TRASH => icon!(r#"<path d="M4.5 7h15M10 11v6M14 11v6"/><path d="M6.5 7l.8 11.5A2 2 0 0 0 9.3 20.5h5.4a2 2 0 0 0 2-1.9L17.5 7M9 7V4.5h6V7"/>"#),
            CHECK => icon!(r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#),
            CHEVRON_RIGHT => icon!(r#"<path d="M9.5 6l6 6-6 6"/>"#),
            STAR_FILL => filled!(r#"<path d="M12 3.2l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17.2l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z"/>"#),
            SPARKLE => filled!(r#"<path d="M12 2.5l1.9 5.6 5.6 1.9-5.6 1.9L12 17.5l-1.9-5.6L4.5 10l5.6-1.9z"/><path d="M18.5 15l.8 2.2 2.2.8-2.2.8-.8 2.2-.8-2.2-2.2-.8 2.2-.8z"/>"#),
            FOLDER_PLUS => icon!(r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2.3h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 10.5v5M9.5 13h5"/>"#),
            FILE_PLUS => icon!(r#"<path d="M6 2.8h8l4.5 4.5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-16.5a1 1 0 0 1 1-1z"/><path d="M12 11v6M9 14h6"/>"#),
            REFRESH => icon!(r#"<path d="M20 11a8 8 0 0 0-14.8-3.5M4 4.5v3.5h3.5"/><path d="M4 13a8 8 0 0 0 14.8 3.5M20 19.5V16h-3.5"/>"#),
            SIDEBAR => icon!(r#"<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><path d="M9 4.5v15"/>"#),
            WARNING => icon!(r#"<path d="M12 4l9 15.5H3z"/><path d="M12 10v4.5M12 17v.2"/>"#),
            PEN => icon!(r#"<path d="M14.5 5.5l4 4L8 20H4v-4z"/><path d="M12.5 7.5l4 4"/>"#),
            SERVER => icon!(r#"<rect x="3.5" y="4" width="17" height="6.5" rx="1.8"/><rect x="3.5" y="13.5" width="17" height="6.5" rx="1.8"/><path d="M7 7.2h.5M7 16.7h.5"/>"#),
            TAG => icon!(r#"<path d="M3.5 12.3V4.5a1 1 0 0 1 1-1h7.8l8.2 8.2a1.4 1.4 0 0 1 0 2l-6.6 6.6a1.4 1.4 0 0 1-2 0z"/><circle cx="8" cy="8" r="1.3"/>"#),
            SWAP => icon!(r#"<path d="M7 4L3.5 7.5 7 11M3.5 7.5h13M17 13l3.5 3.5L17 20M20.5 16.5h-13"/>"#),
            PROMPT => icon!(r#"<path d="M5 7l5 5-5 5M12.5 17.5h6.5"/>"#),
            FOLDER_OPEN => icon!(r#"<path d="M3 18.5V6a1.5 1.5 0 0 1 1.5-1.5h4.3l2 2.3H18a1.5 1.5 0 0 1 1.5 1.5V10"/><path d="M3 18.5l2.6-7.2A1.5 1.5 0 0 1 7 10.3h13.4a1 1 0 0 1 .9 1.3L19 18.5z"/>"#),
            SCISSORS => icon!(r#"<circle cx="6.5" cy="6.5" r="2.8"/><circle cx="6.5" cy="17.5" r="2.8"/><path d="M8.6 8.4L20 18M8.6 15.6L20 6"/>"#),
            CLIPBOARD => icon!(r#"<rect x="5" y="4.5" width="14" height="16.5" rx="2"/><path d="M9 4.5V3.5h6v1M9 4.5h6v2.5H9z"/><path d="M8.5 12h7M8.5 15.5h5"/>"#),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
