//! Shared design-system metrics for the Aceris application UI.
//!
//! This module contains layout and typography roles only. Brand colors remain
//! defined exclusively in `brand`, so a UI redesign cannot accidentally
//! introduce a second palette.

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
    pub const PANEL_HIGHLIGHT_INSET_X: f32 = 7.0;
    pub const PANEL_HIGHLIGHT_Y: f32 = 2.0;
    pub const PANEL_HIGHLIGHT_HEIGHT: f32 = 1.0;
    pub const PANEL_BOTTOM_INSET: f32 = 2.0;
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
            Self::Display => 1.8,
            Self::Heading => 1.55,
            Self::Body => 1.12,
            Self::Label => 1.0,
            Self::Caption => 0.92,
        }
    }

    const fn minimum(self) -> f32 {
        match self {
            Self::Display | Self::Heading => 2.0,
            Self::Body | Self::Label | Self::Caption => 1.0,
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
    fn viewport_rejects_dimensions_outside_safe_limits() {
        assert!(Viewport::from_size(159, 600).is_none());
        assert!(Viewport::from_size(600, 159).is_none());
        assert!(Viewport::from_size(4097, 600).is_none());
        assert!(Viewport::from_size(600, 4097).is_none());
    }

    #[test]
    fn typography_roles_keep_a_clear_hierarchy_at_small_and_large_sizes() {
        let scale = |role| typography_scale(role, 1.0);
        assert!(scale(TypographyRole::Display) > scale(TypographyRole::Body));
        assert!(scale(TypographyRole::Body) > scale(TypographyRole::Caption));
        assert_eq!(typography_scale(TypographyRole::Display, 0.1), 2);
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
