//! Visual identity shared by every frontend (see `docs/IDENTIDADE_VISUAL.md`).
//!
//! The palette is taken from the acerola icon (`assets/icon/aceris.svg`).
//! Colours are `0xRRGGBB` so frontends can convert them to their own formats.

/// Window and application title.
pub const APP_TITLE: &str = "Aceris";

/// Deep purple used for backgrounds (icon gradient start).
pub const PURPLE_LIGHT: u32 = 0x4A1D5E;
/// Darkest purple (icon gradient end).
pub const PURPLE_DARK: u32 = 0x1A0B2E;
/// Adaptive-icon / splash background; matches `ic_launcher_background`.
pub const BACKGROUND: u32 = 0x2A1240;
/// Primary accent: the acerola red.
pub const ACCENT_RED: u32 = 0xE5173F;
/// Highlight red (top of the fruit).
pub const ACCENT_RED_LIGHT: u32 = 0xFF6B6B;
/// Shade red (bottom of the fruit).
pub const ACCENT_RED_DARK: u32 = 0x8E0A2B;
/// Secondary accent: the leaf green.
pub const ACCENT_GREEN: u32 = 0x3E8E3A;
/// Light leaf green.
pub const ACCENT_GREEN_LIGHT: u32 = 0x86D95F;
/// Near-white for text on dark backgrounds.
pub const TEXT: u32 = 0xF5EEF8;

/// Formats a colour as `#RRGGBB`.
pub fn hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_formats_with_padding() {
        assert_eq!(hex(BACKGROUND), "#2A1240");
        assert_eq!(hex(0x0000FF), "#0000FF");
    }

    /// The palette must stay in sync with the icon source of truth.
    #[test]
    fn palette_matches_icon_svg() {
        let svg = include_str!("../../../assets/icon/aceris.svg").to_uppercase();
        for color in [
            PURPLE_LIGHT,
            PURPLE_DARK,
            ACCENT_RED,
            ACCENT_RED_LIGHT,
            ACCENT_RED_DARK,
            ACCENT_GREEN,
            ACCENT_GREEN_LIGHT,
        ] {
            assert!(
                svg.contains(&hex(color)),
                "{} missing from icon",
                hex(color)
            );
        }
    }

    #[test]
    fn android_background_matches_palette() {
        let xml = include_str!("../../../frontends/android/res/values/ic_launcher_background.xml");
        assert!(xml.to_uppercase().contains(&hex(BACKGROUND)));
    }
}
