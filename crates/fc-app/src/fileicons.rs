//! Real application icons for the file lists: rendered once per app on a background
//! thread (`fc_core::fileicon`) and kept for the session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AsyncApp, Global, Image, ImageFormat};

/// Pixels per side: sharp up to 64 pt on a Retina display.
const ICON_PX: u32 = 128;

/// `None` while loading, or when the app has no icon.
#[derive(Default)]
struct IconCache(HashMap<PathBuf, Option<Arc<Image>>>);

impl Global for IconCache {}

/// How many icons have loaded (smoke run).
pub fn loaded(cx: &App) -> usize {
    cx.try_global::<IconCache>().map_or(0, |c| c.0.values().filter(|v| v.is_some()).count())
}

/// The icon of the application at `path`, once it has loaded. The first request starts
/// loading it in the background; windows redraw when it arrives.
pub fn app_icon(path: &Path, cx: &mut App) -> Option<Arc<Image>> {
    if let Some(slot) = cx.default_global::<IconCache>().0.get(path) {
        return slot.clone();
    }
    cx.global_mut::<IconCache>().0.insert(path.to_path_buf(), None);
    let path = path.to_path_buf();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let source = path.clone();
        let png = cx.background_executor().spawn(async move { fc_core::fileicon::icon_png(&source, ICON_PX) }).await;
        if let Some(png) = png {
            let _ = cx.update(|cx| {
                cx.global_mut::<IconCache>().0.insert(path, Some(Arc::new(Image::from_bytes(ImageFormat::Png, png))));
                cx.refresh_windows();
            });
        }
    })
    .detach();
    None
}
