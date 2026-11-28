//! Shared design-system metrics for the Aceris application UI.
//!
//! This module contains layout and typography roles only. Brand colors remain
//! defined exclusively in `brand`, so a UI redesign cannot accidentally
//! introduce a second palette.

/// Geometry for the shared landscape launcher shell. Coordinates use the
/// 1000-unit logical width from the responsive viewport; desktop and Android
/// may adapt the surrounding shell without changing brand colours.
pub mod chrome {
    /// Height reserved for the shared landscape top bar.
    pub const HEADER_HEIGHT: f32 = 72.0;
    /// Vertical inset and height of top-bar controls.
    pub const HEADER_BUTTON_Y: f32 = 12.0;
    pub const HEADER_BUTTON_HEIGHT: f32 = 48.0;

    /// Brand lockup and primary destinations.
    pub const BRAND_X: f32 = 20.0;
    pub const BRAND_WIDTH: f32 = 112.0;
    pub const HOME_X: f32 = 144.0;
    pub const HOME_WIDTH: f32 = 88.0;
    pub const RECENT_X: f32 = 238.0;
    pub const RECENT_WIDTH: f32 = 100.0;
    pub const GAMES_X: f32 = 344.0;
    pub const GAMES_WIDTH: f32 = 96.0;
    pub const FAVORITES_X: f32 = 446.0;
    pub const FAVORITES_WIDTH: f32 = 122.0;
    pub const HOMEBREW_X: f32 = 574.0;
    pub const HOMEBREW_WIDTH: f32 = 132.0;
    pub const IMPORT_X: f32 = 730.0;
    pub const IMPORT_WIDTH: f32 = 126.0;
    pub const SETTINGS_X: f32 = 868.0;
    pub const SETTINGS_WIDTH: f32 = 112.0;
}

/// Shared geometry for the landscape game-library grid. These values are
/// consumed by rendering, pointer hit-testing and controller navigation.
pub mod library {
    /// The console-style collection uses three wider cards per row.
    pub const GRID_COLUMNS: usize = 3;
    pub const GRID_ORIGIN_X: f32 = 28.0;
    pub const GRID_TOP: f32 = 170.0;
    pub const GRID_CARD_WIDTH: f32 = 222.0;
    pub const GRID_CARD_HEIGHT: f32 = 166.0;
    pub const GRID_GAP_X: f32 = 18.0;
    pub const GRID_GAP_Y: f32 = 12.0;
    pub const GRID_STEP_X: f32 = GRID_CARD_WIDTH + GRID_GAP_X;
    pub const GRID_STEP_Y: f32 = GRID_CARD_HEIGHT + GRID_GAP_Y;
}

/// Consistent spacing steps in logical UI units (4-unit base grid).
pub mod spacing {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Shared shape metrics for panels, cards and controls.
pub mod shape {
    pub const PANEL_RADIUS: f32 = 10.0;
    pub const PANEL_BORDER_RADIUS: f32 = 9.0;
    pub const PANEL_CONTENT_RADIUS: f32 = 8.0;
    pub const PANEL_SHADOW_X: f32 = 2.0;
    pub const PANEL_SHADOW_Y: f32 = 3.0;
    pub const PANEL_CONTENT_INSET: f32 = 1.0;
}

/// Semantic text roles. Factors preserve the current readable size hierarchy
/// while making the scale source consistent across all shared screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypographyRole {
    Display,
    Heading,
    Body,
    Label,
    Caption,
}

impl TypographyRole {
    const fn factor(self) -> f32 {
        match self {
            Self::Display => 2.1,
            Self::Heading => 1.7,
            Self::Body => 1.25,
            Self::Label => 1.0,
            Self::Caption => 0.9,
        }
    }

    const fn minimum(self) -> f32 {
        match self {
            Self::Display => 3.0,
            Self::Heading | Self::Body => 2.0,
            Self::Label | Self::Caption => 1.0,
        }
    }
}

/// Converts a logical viewport scale and text role into a safe integer glyph scale.
pub fn typography_scale(role: TypographyRole, viewport_scale: f32) -> usize {
    if !viewport_scale.is_finite() || viewport_scale <= 0.0 {
        return 1;
    }
    (viewport_scale * role.factor())
        .round()
        .max(role.minimum())
        .min(12.0) as usize
}

/// Normalized logical dimensions shared by the desktop and Android renderers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    pub is_portrait: bool,
    pub scale: f32,
    pub logical_width: f32,
    pub logical_height: f32,
}

impl Viewport {
    /// Creates a viewport for the Aceris launcher, rejecting unsafe dimensions.
    pub fn from_size(width: u32, height: u32) -> Option<Self> {
        if width < 160 || height < 160 || width > 4096 || height > 4096 {
            return None;
        }

        let is_portrait = height >= width;
        let logical_width = if is_portrait { 400.0 } else { 1000.0 };
        let scale = width as f32 / logical_width;
        let logical_height = height as f32 / scale;
        if !scale.is_finite()
            || scale <= 0.0
            || !logical_height.is_finite()
            || logical_height <= 0.0
        {
            return None;
        }

        Some(Self {
            width,
            height,
            is_portrait,
            scale,
            logical_width,
            logical_height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_uses_shared_landscape_and_portrait_design_grids() {
        let landscape = Viewport::from_size(1920, 1080).unwrap();
        assert!(!landscape.is_portrait);
        assert_eq!(landscape.logical_width, 1000.0);
        assert_eq!(landscape.logical_height, 562.5);
        assert_eq!(landscape.scale, 1.92);

        let portrait = Viewport::from_size(1080, 1920).unwrap();
        assert!(portrait.is_portrait);
        assert_eq!(portrait.logical_width, 400.0);
        assert_eq!(portrait.logical_height, 1920.0 / 2.7);
        assert_eq!(portrait.scale, 2.7);
    }

    #[test]
    fn landscape_library_grid_uses_three_wide_console_cards() {
        assert_eq!(library::GRID_COLUMNS, 3);
        assert!(library::GRID_CARD_WIDTH > 168.0);
        assert!(library::GRID_CARD_HEIGHT > 148.0);
        let collection_width = library::GRID_COLUMNS as f32 * library::GRID_CARD_WIDTH
            + (library::GRID_COLUMNS - 1) as f32 * library::GRID_GAP_X;
        assert_eq!(collection_width, 702.0);
    }

    #[test]
    fn viewport_rejects_dimensions_outside_safe_limits() {
        assert!(Viewport::from_size(159, 600).is_none());
        assert!(Viewport::from_size(600, 159).is_none());
        assert!(Viewport::from_size(4097, 600).is_none());
        assert!(Viewport::from_size(600, 4097).is_none());
    }

    #[test]
    fn typography_roles_keep_a_clear_hierarchy_at_small_and_large_sizes() {
        let scale = |role| typography_scale(role, 1.0);
        assert!(scale(TypographyRole::Display) > scale(TypographyRole::Heading));
        assert_eq!(scale(TypographyRole::Heading), scale(TypographyRole::Body));
        assert!(scale(TypographyRole::Body) > scale(TypographyRole::Caption));

        let large_body = typography_scale(TypographyRole::Body, 5.0);
        let large_caption = typography_scale(TypographyRole::Caption, 5.0);
        assert!(large_body > large_caption);

        assert_eq!(typography_scale(TypographyRole::Display, 0.1), 3);
        assert_eq!(typography_scale(TypographyRole::Caption, f32::NAN), 1);
        assert!(typography_scale(TypographyRole::Display, 100.0) <= 12);
    }

    #[test]
    fn shape_tokens_keep_one_shared_card_language() {
        assert!(shape::PANEL_RADIUS > shape::PANEL_BORDER_RADIUS);
        assert!(shape::PANEL_BORDER_RADIUS > shape::PANEL_CONTENT_RADIUS);
        assert!(shape::PANEL_SHADOW_X > 0.0);
        assert!(shape::PANEL_SHADOW_Y > 0.0);
    }
}
