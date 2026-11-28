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

    /// Contextual details rail for the selected title.
    pub const DETAILS_PANEL_X: f32 = 760.0;
    pub const DETAILS_PANEL_TOP: f32 = 162.0;
    pub const DETAILS_PANEL_WIDTH: f32 = 212.0;
    pub const DETAILS_PANEL_HEIGHT: f32 = 310.0;
    pub const DETAILS_CENTER_X: f32 = 866.0;
    pub const DETAILS_ART_X: f32 = 820.0;
    pub const DETAILS_ART_TOP: f32 = 190.0;
    pub const DETAILS_ART_SIZE: f32 = 92.0;
    pub const DETAILS_BUTTON_X: f32 = 772.0;
    pub const DETAILS_BUTTON_WIDTH: f32 = 188.0;
    pub const DETAILS_BUTTON_HEIGHT: f32 = 32.0;

    pub fn details_panel_height(logical_height: f32) -> f32 {
        if logical_height < 520.0 {
            (logical_height - 178.0).clamp(240.0, DETAILS_PANEL_HEIGHT)
        } else {
            DETAILS_PANEL_HEIGHT
        }
    }

    pub fn details_art_size(logical_height: f32) -> f32 {
        if logical_height < 520.0 {
            64.0
        } else {
            DETAILS_ART_SIZE
        }
    }

    pub fn details_art_top(logical_height: f32) -> f32 {
        if logical_height < 520.0 {
            188.0
        } else {
            DETAILS_ART_TOP
        }
    }

    pub fn details_title_y(logical_height: f32) -> f32 {
        if logical_height < 520.0 { 256.0 } else { 292.0 }
    }

    pub fn details_metadata_y(logical_height: f32) -> f32 {
        if logical_height < 520.0 { 274.0 } else { 310.0 }
    }

    pub fn details_button_height(logical_height: f32) -> f32 {
        if logical_height < 520.0 {
            24.0
        } else {
            DETAILS_BUTTON_HEIGHT
        }
    }

    pub fn details_button_top(index: usize, logical_height: f32) -> f32 {
        if logical_height < 520.0 {
            292.0 + index as f32 * 30.0
        } else {
            330.0 + index as f32 * 36.0
        }
    }
}

/// Shared geometry for the console-style landscape Home dashboard.
pub mod home {
    /// Five square-cover entries fit across the landscape collection shelf.
    pub const VISIBLE_GAME_CARDS: usize = 5;
    pub const CAROUSEL_X: f32 = 28.0;
    pub const CAROUSEL_TOP: f32 = 316.0;
    pub const GAME_CARD_WIDTH: f32 = 172.0;
    pub const GAME_CARD_HEIGHT: f32 = 164.0;
    pub const GAME_CARD_GAP: f32 = 16.0;
    pub const GAME_CARD_STEP: f32 = GAME_CARD_WIDTH + GAME_CARD_GAP;
    pub const CAROUSEL_BOTTOM: f32 = CAROUSEL_TOP + GAME_CARD_HEIGHT;
    pub const SUMMARY_TOP: f32 = 492.0;
    pub const SUMMARY_HEIGHT: f32 = 64.0;
    pub const COMPACT_HEIGHT: f32 = 520.0;

    pub fn carousel_top(logical_height: f32) -> f32 {
        if logical_height < COMPACT_HEIGHT {
            292.0
        } else {
            CAROUSEL_TOP
        }
    }

    pub fn game_card_height(logical_height: f32) -> f32 {
        if logical_height < COMPACT_HEIGHT {
            144.0
        } else {
            GAME_CARD_HEIGHT
        }
    }

    pub fn summary_visible(logical_height: f32) -> bool {
        logical_height >= SUMMARY_TOP + SUMMARY_HEIGHT
    }
}

/// Shared geometry for landscape Settings navigation and its touch targets.
pub mod settings {
    pub const TAB_TOP: f32 = 132.0;
    pub const TAB_HEIGHT: f32 = 44.0;
    pub const TAB_TEXT_Y: f32 = 151.0;
    pub const TAB_MARKER_TOP: f32 = 171.0;
    pub const TAB_X: f32 = 40.0;
    pub const TAB_WIDTH: f32 = 217.0;
    pub const TAB_STEP: f32 = 231.0;
    pub const HIT_LEFT: f32 = 28.0;
    pub const HIT_TOP: f32 = 132.0;
    pub const HIT_BOTTOM: f32 = 178.0;
    pub const HIT_STEP: f32 = 234.0;
    pub const HIT_RIGHT: f32 = 730.0;
    pub const SECTION_COUNT: usize = 3;

    pub fn tab_x(index: usize) -> f32 {
        TAB_X + index as f32 * TAB_STEP
    }

    /// Maps the shared touch region to Library, Performance, or Controls.
    pub fn section_index_at(x: f32, y: f32) -> Option<usize> {
        if !(HIT_TOP..HIT_BOTTOM).contains(&y) || !(HIT_LEFT..HIT_RIGHT).contains(&x) {
            return None;
        }
        let index = ((x - HIT_LEFT) / HIT_STEP).floor() as usize;
        (index < SECTION_COUNT).then_some(index)
    }
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
    fn landscape_settings_tabs_share_their_render_and_touch_geometry() {
        assert_eq!(settings::tab_x(0), 40.0);
        assert_eq!(settings::tab_x(1), 271.0);
        assert_eq!(settings::tab_x(2), 502.0);
        assert_eq!(settings::section_index_at(100.0, 150.0), Some(0));
        assert_eq!(settings::section_index_at(300.0, 150.0), Some(1));
        assert_eq!(settings::section_index_at(600.0, 150.0), Some(2));
        assert_eq!(settings::section_index_at(729.0, 150.0), Some(2));
        assert_eq!(settings::section_index_at(730.0, 150.0), None);
        assert_eq!(settings::section_index_at(100.0, 131.0), None);
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
    fn landscape_home_fits_five_square_cover_cards_and_summary() {
        assert_eq!(home::VISIBLE_GAME_CARDS, 5);
        let shelf_width = home::VISIBLE_GAME_CARDS as f32 * home::GAME_CARD_WIDTH
            + (home::VISIBLE_GAME_CARDS - 1) as f32 * home::GAME_CARD_GAP;
        assert!(shelf_width <= 944.0);
        assert!(home::CAROUSEL_BOTTOM < home::SUMMARY_TOP);
        assert!(home::SUMMARY_TOP + home::SUMMARY_HEIGHT <= 562.5);
    }

    #[test]
    fn compact_landscape_metrics_keep_home_and_details_actions_visible() {
        let compact_height = 450.0;
        assert_eq!(home::carousel_top(compact_height), 292.0);
        assert_eq!(home::game_card_height(compact_height), 144.0);
        assert!(!home::summary_visible(compact_height));
        assert!(
            home::carousel_top(compact_height) + home::game_card_height(compact_height)
                <= compact_height
        );

        assert_eq!(library::details_art_size(compact_height), 64.0);
        assert_eq!(library::details_button_height(compact_height), 24.0);
        let last_button_bottom = library::details_button_top(3, compact_height)
            + library::details_button_height(compact_height);
        assert!(last_button_bottom < compact_height);
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
