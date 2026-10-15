//! Colour presets (SPEC §9 Appearance). Palettes are original to this project.

/// Colours as 0xRRGGBB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    pub dark: bool,
    // File list (the seven user-facing colours).
    /// Panel card surface.
    pub background: u32,
    pub alt_background: u32,
    pub text: u32,
    pub cursor: u32,
    pub cursor_text: u32,
    pub mark: u32,
    pub mark_text: u32,
    // Window chrome.
    pub chrome: u32,
    pub chrome_text: u32,
    pub muted_text: u32,
    pub border: u32,
    pub accent: u32,
    /// Second accent, used for gradients and highlights.
    pub accent2: u32,
    /// Text and icons drawn on top of `accent`.
    pub on_accent: u32,
    pub folder: u32,
    pub danger: u32,
    pub success: u32,
    /// Window background gradient, top to bottom.
    pub bg_top: u32,
    pub bg_bottom: u32,
}

pub const BLACK: Palette = Palette {
    name: "Black",
    dark: true,
    background: 0x101011,
    alt_background: 0x141416,
    text: 0xececef,
    cursor: 0xdcdce1,
    cursor_text: 0x000000,
    mark: 0x2a2a2e,
    mark_text: 0xffffff,
    chrome: 0x000000,
    chrome_text: 0xe5e5ea,
    muted_text: 0x8e8e93,
    border: 0x252527,
    accent: 0xf2f2f7,
    accent2: 0x8e8e93,
    on_accent: 0x000000,
    folder: 0xb4c2d3,
    danger: 0xff6961,
    success: 0x6fcf8f,
    bg_top: 0x111113,
    bg_bottom: 0x030303,
};

pub const SILVER: Palette = Palette {
    name: "Silver",
    dark: false,
    background: 0xffffff,
    alt_background: 0xf7f7f9,
    text: 0x1c1c1e,
    cursor: 0x1c1c1e,
    cursor_text: 0xffffff,
    mark: 0xe6e6eb,
    mark_text: 0x000000,
    chrome: 0xececf0,
    chrome_text: 0x1c1c1e,
    muted_text: 0x6e6e73,
    border: 0xdcdce1,
    accent: 0x1c1c1e,
    accent2: 0x8e8e93,
    on_accent: 0xffffff,
    folder: 0x5c7088,
    danger: 0xd70015,
    success: 0x248a3d,
    bg_top: 0xf7f7f9,
    bg_bottom: 0xe1e1e6,
};

pub const WHITE: Palette = Palette {
    name: "White",
    dark: false,
    background: 0xffffff,
    alt_background: 0xfafafb,
    text: 0x1c1c1e,
    cursor: 0x1c1c1e,
    cursor_text: 0xffffff,
    mark: 0xececf0,
    mark_text: 0x000000,
    chrome: 0xffffff,
    chrome_text: 0x1c1c1e,
    muted_text: 0x6e6e73,
    border: 0xe5e5ea,
    accent: 0x1c1c1e,
    accent2: 0xaeaeb2,
    on_accent: 0xffffff,
    folder: 0x5c7088,
    danger: 0xd70015,
    success: 0x248a3d,
    bg_top: 0xffffff,
    bg_bottom: 0xf2f2f5,
};

pub const CLASSIC_BLUE: Palette = Palette {
    name: "Classic Blue",
    dark: true,
    background: 0x10247a,
    alt_background: 0x132a86,
    text: 0x7fe3ff,
    cursor: 0x2fa6b5,
    cursor_text: 0x061238,
    mark: 0x183596,
    mark_text: 0xffe45c,
    chrome: 0x0b1a5c,
    chrome_text: 0xe8f1ff,
    muted_text: 0x9cb2e6,
    border: 0x2c46a8,
    accent: 0x4fd1e0,
    accent2: 0xffe45c,
    on_accent: 0x061238,
    folder: 0xffffff,
    danger: 0xff7a6b,
    success: 0x7dffa8,
    bg_top: 0x0d1d66,
    bg_bottom: 0x071242,
};

pub const PHOSPHOR: Palette = Palette {
    name: "Phosphor",
    dark: true,
    background: 0x0b120b,
    alt_background: 0x0e170e,
    text: 0x55e06f,
    cursor: 0x1f6b2c,
    cursor_text: 0xd8ffd8,
    mark: 0x1a2c1a,
    mark_text: 0xf5d547,
    chrome: 0x0f1a10,
    chrome_text: 0x8ef59f,
    muted_text: 0x4f8f5a,
    border: 0x1e3a22,
    accent: 0x55e06f,
    accent2: 0xb7f75b,
    on_accent: 0x04120a,
    folder: 0xa6f7b3,
    danger: 0xff6e5a,
    success: 0x55e06f,
    bg_top: 0x0c160d,
    bg_bottom: 0x050a05,
};

/// Selectable presets. "System" resolves to Black or Silver at runtime.
pub const PRESETS: &[Palette] = &[BLACK, SILVER, WHITE, CLASSIC_BLUE, PHOSPHOR];

/// Theme names offered in Preferences, including "System".
pub fn theme_names() -> Vec<&'static str> {
    std::iter::once("System").chain(PRESETS.iter().map(|p| p.name)).collect()
}

/// Resolves a theme name for the current system appearance. Unknown names (including
/// presets from earlier versions) fall back to the system palette.
pub fn resolve(name: &str, system_is_dark: bool) -> Palette {
    if let Some(p) = PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
        return *p;
    }
    if system_is_dark { BLACK } else { SILVER }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution() {
        assert_eq!(resolve("System", true).name, "Black");
        assert_eq!(resolve("System", false).name, "Silver");
        assert_eq!(resolve("classic blue", false).name, "Classic Blue");
        assert_eq!(resolve("Aurora", false).name, "Silver", "retired presets fall back");
        assert_eq!(theme_names().len(), PRESETS.len() + 1);
    }

    /// WCAG relative luminance contrast between two colours.
    fn contrast(a: u32, b: u32) -> f64 {
        fn lum(c: u32) -> f64 {
            let ch = |v: u32| {
                let s = v as f64 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch((c >> 16) & 0xff) + 0.7152 * ch((c >> 8) & 0xff) + 0.0722 * ch(c & 0xff)
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn readable_contrast() {
        for p in PRESETS {
            assert!(contrast(p.text, p.background) >= 4.5, "{} text", p.name);
            assert!(contrast(p.chrome_text, p.bg_top) >= 4.5, "{} chrome text", p.name);
            assert!(contrast(p.muted_text, p.background) >= 3.0, "{} muted", p.name);
            assert!(contrast(p.cursor_text, p.cursor) >= 4.5, "{} cursor", p.name);
            assert!(contrast(p.mark_text, p.mark) >= 3.0, "{} mark", p.name);
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "{} on accent", p.name);
        }
    }

    #[test]
    fn default_presets_are_neutral_grays() {
        // Black and Silver chrome must be unsaturated (R = G = B within a small margin).
        for p in [BLACK, SILVER, WHITE] {
            for c in [p.background, p.chrome, p.bg_top, p.bg_bottom, p.border, p.accent, p.cursor] {
                let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
                let spread = r.max(g).max(b) - r.min(g).min(b);
                assert!(spread <= 6, "{}: {c:06x} is tinted", p.name);
            }
        }
    }
}
