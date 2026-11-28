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

/// Semantic launcher surfaces. These tokens keep the library, settings and
/// in-game overlays on one Aceris palette across desktop and Android.
pub const SURFACE_BASE: u32 = PURPLE_DARK;
pub const SURFACE_HEADER: u32 = 0x211032;
pub const SURFACE_PANEL: u32 = BACKGROUND;
pub const SURFACE_CARD: u32 = 0x351A4A;
pub const SURFACE_RAISED: u32 = 0x402054;
pub const SURFACE_SELECTED: u32 = PURPLE_LIGHT;
pub const SURFACE_BUTTON: u32 = 0x3A214B;
pub const BORDER: u32 = 0x644078;
/// Secondary text with sufficient contrast on the dark launcher surfaces.
pub const TEXT_MUTED: u32 = 0xC7B8D3;
/// Supporting text for less prominent hints and metadata.
pub const TEXT_SUBTLE: u32 = 0xA998B8;
/// Background for enabled/healthy states; green remains a status color only.
pub const SUCCESS_SURFACE: u32 = 0x23432D;

/// Converts an RGB colour token to the channel order used by RGBA renderers.
pub const fn rgb(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

/// Formats a colour as `#RRGGBB`.
pub fn hex(color: u32) -> String {
    format!("#{:06X}", color & 0xFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_palette_uses_rgb_channel_order() {
        assert_eq!(rgb(ACCENT_RED), [0xE5, 0x17, 0x3F]);
        assert_eq!(rgb(SURFACE_BASE), [0x1A, 0x0B, 0x2E]);
        assert_eq!(rgb(TEXT_MUTED), [0xC7, 0xB8, 0xD3]);
        assert_eq!(hex(SURFACE_SELECTED), "#4A1D5E");
    }

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
