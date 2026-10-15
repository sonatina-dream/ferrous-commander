//! GPUI colours derived from the active `fc_core::theme::Palette`.

use gpui::{rgb, App, Hsla, Window, WindowAppearance};

use fc_core::theme::{self, Palette};

use crate::state::AppState;

/// Resolved colours for one frame.
#[derive(Clone, Copy)]
pub struct Colors {
    pub palette: Palette,
    pub dark: bool,
    /// Panel card surface.
    pub bg: Hsla,
    pub alt_bg: Hsla,
    pub text: Hsla,
    pub cursor: Hsla,
    pub cursor_text: Hsla,
    pub mark: Hsla,
    pub mark_text: Hsla,
    pub chrome_text: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    pub accent2: Hsla,
    /// Text and icons on `accent` backgrounds.
    pub on_accent: Hsla,
    pub folder: Hsla,
    pub danger: Hsla,
    pub success: Hsla,
    pub bg_top: Hsla,
    pub bg_bottom: Hsla,
    /// Translucent overlay for hover states.
    pub hover: Hsla,
    /// Faint fill for sunken areas (inputs, sidebars).
    pub sunken: Hsla,
    /// Modal backdrop.
    pub scrim: Hsla,
    /// Raised surfaces (dialogs, menus, palette).
    pub surface: Hsla,
    /// Drop shadow colour.
    pub shadow: Hsla,
}

pub fn hsla(v: u32) -> Hsla {
    rgb(v).into()
}

impl Colors {
    pub fn from_palette(p: Palette) -> Colors {
        let dark = p.dark;
        let surface = if dark { hsla(p.background).blend(gpui::white().opacity(0.035)) } else { gpui::white() };
        Colors {
            palette: p,
            dark,
            bg: hsla(p.background),
            alt_bg: hsla(p.alt_background),
            text: hsla(p.text),
            cursor: hsla(p.cursor),
            cursor_text: hsla(p.cursor_text),
            mark: hsla(p.mark),
            mark_text: hsla(p.mark_text),
            chrome_text: hsla(p.chrome_text),
            muted: hsla(p.muted_text),
            border: hsla(p.border),
            accent: hsla(p.accent),
            accent2: hsla(p.accent2),
            on_accent: hsla(p.on_accent),
            folder: hsla(p.folder),
            danger: hsla(p.danger),
            success: hsla(p.success),
            bg_top: hsla(p.bg_top),
            bg_bottom: hsla(p.bg_bottom),
            hover: if dark { gpui::white().opacity(0.06) } else { gpui::black().opacity(0.045) },
            sunken: if dark { gpui::black().opacity(0.22) } else { gpui::black().opacity(0.035) },
            scrim: gpui::black().opacity(if dark { 0.5 } else { 0.28 }),
            surface,
            shadow: gpui::black().opacity(if dark { 0.45 } else { 0.14 }),
        }
    }

    /// Colours for the current theme and window appearance.
    pub fn get(window: &Window, cx: &App) -> Colors {
        let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
        let name = cx.global::<AppState>().settings.appearance.theme.clone();
        Colors::from_palette(theme::resolve(&name, dark))
    }
}
