//! Frontend presentation contracts shared by desktop and Android.
//!
//! The module deliberately contains no window-system or Android dependencies:
//! hosts decide the actual surface size and ask the core for matching screen
//! rectangles and virtual-control geometry.

/// Requested screen orientation: follow the available surface dimensions.
pub const ORIENTATION_AUTO: u32 = 0;
/// Explicit portrait layout (the two LCDs are stacked).
pub const ORIENTATION_PORTRAIT: u32 = 1;
/// Explicit landscape layout (the two LCDs are side by side).
pub const ORIENTATION_LANDSCAPE: u32 = 2;

/// Stable C-compatible rectangle in host-surface coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Placement of the two LCDs in a host-owned surface.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenLayout {
    /// Resolved orientation (1 = portrait, 2 = landscape).
    pub orientation: u32,
    pub top: ScreenRect,
    pub bottom: ScreenRect,
}

/// User-facing arrangement for the two physical LCDs.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenLayoutMode {
    /// Select stacked or side-by-side using the available surface aspect ratio.
    #[default]
    Auto = 0,
    /// Both screens horizontally, top first.
    SideBySide = 1,
    /// Both screens vertically, top first.
    Vertical = 2,
    /// Both screens vertically, with the top LCD enlarged.
    TopEnlarged = 3,
    /// Both screens vertically, with the touch LCD enlarged.
    BottomEnlarged = 4,
    /// Show only the top LCD.
    TopOnly = 5,
    /// Show only the touch LCD.
    BottomOnly = 6,
}

impl ScreenLayoutMode {
    /// Cycles through the concrete layouts exposed by the in-game menu.
    pub const fn next(self) -> Self {
        match self {
            Self::Auto => Self::SideBySide,
            Self::SideBySide => Self::Vertical,
            Self::Vertical => Self::TopEnlarged,
            Self::TopEnlarged => Self::BottomEnlarged,
            Self::BottomEnlarged => Self::TopOnly,
            Self::TopOnly => Self::BottomOnly,
            Self::BottomOnly => Self::SideBySide,
        }
    }
}

/// Returns the natural logical canvas size for a concrete presentation mode.
///
/// Desktop uses this to resize its client area without fitting a portrait layout
/// into a short landscape window. Android normally fits the mode to its surface.
pub const fn screen_layout_canvas_size(mode: ScreenLayoutMode) -> (u32, u32) {
    match mode {
        ScreenLayoutMode::Auto | ScreenLayoutMode::SideBySide => (736, 240),
        ScreenLayoutMode::Vertical => (400, 496),
        ScreenLayoutMode::TopEnlarged => (480, 544),
        ScreenLayoutMode::BottomEnlarged => (400, 556),
        ScreenLayoutMode::TopOnly => (400, 240),
        ScreenLayoutMode::BottomOnly => (320, 240),
    }
}

/// Computes the legacy orientation-selected layout.
pub fn compute_screen_layout(
    view_width: u32,
    view_height: u32,
    requested_orientation: u32,
) -> ScreenLayout {
    let mode = match requested_orientation {
        ORIENTATION_PORTRAIT => ScreenLayoutMode::Vertical,
        ORIENTATION_LANDSCAPE => ScreenLayoutMode::SideBySide,
        _ => ScreenLayoutMode::Auto,
    };
    compute_screen_layout_for_mode(view_width, view_height, mode, false)
}

/// Computes the placement of each physical screen for a user-selected mode.
///
/// All modes preserve the 3DS LCD aspect ratios. The swap option exchanges
/// positions without changing screen identity, so touch follows the bottom LCD.
pub fn compute_screen_layout_for_mode(
    view_width: u32,
    view_height: u32,
    mode: ScreenLayoutMode,
    swap_screens: bool,
) -> ScreenLayout {
    const TOP_W: f32 = 400.0;
    const TOP_H: f32 = 240.0;
    const BOTTOM_W: f32 = 320.0;
    const BOTTOM_H: f32 = 240.0;
    const GAP: f32 = 16.0;

    let resolved = match mode {
        ScreenLayoutMode::Auto if view_width >= view_height => ScreenLayoutMode::SideBySide,
        ScreenLayoutMode::Auto => ScreenLayoutMode::Vertical,
        concrete => concrete,
    };

    let (orientation, logical_width, logical_height, top, bottom) = match resolved {
        ScreenLayoutMode::Auto => unreachable!("Auto is resolved above"),
        ScreenLayoutMode::SideBySide => (
            ORIENTATION_LANDSCAPE,
            TOP_W + GAP + BOTTOM_W,
            TOP_H,
            (0.0, 0.0, TOP_W, TOP_H),
            (TOP_W + GAP, 0.0, BOTTOM_W, BOTTOM_H),
        ),
        ScreenLayoutMode::Vertical => (
            ORIENTATION_PORTRAIT,
            TOP_W,
            TOP_H + GAP + BOTTOM_H,
            (0.0, 0.0, TOP_W, TOP_H),
            ((TOP_W - BOTTOM_W) * 0.5, TOP_H + GAP, BOTTOM_W, BOTTOM_H),
        ),
        ScreenLayoutMode::TopEnlarged => (
            ORIENTATION_PORTRAIT,
            480.0,
            288.0 + GAP + BOTTOM_H,
            (0.0, 0.0, 480.0, 288.0),
            (80.0, 288.0 + GAP, BOTTOM_W, BOTTOM_H),
        ),
        ScreenLayoutMode::BottomEnlarged => (
            ORIENTATION_PORTRAIT,
            400.0,
            TOP_H + GAP + 300.0,
            (0.0, 0.0, TOP_W, TOP_H),
            (0.0, TOP_H + GAP, 400.0, 300.0),
        ),
        ScreenLayoutMode::TopOnly => (
            ORIENTATION_LANDSCAPE,
            TOP_W,
            TOP_H,
            (0.0, 0.0, TOP_W, TOP_H),
            (0.0, 0.0, 0.0, 0.0),
        ),
        ScreenLayoutMode::BottomOnly => (
            ORIENTATION_LANDSCAPE,
            BOTTOM_W,
            BOTTOM_H,
            (0.0, 0.0, 0.0, 0.0),
            (0.0, 0.0, BOTTOM_W, BOTTOM_H),
        ),
    };

    if view_width == 0 || view_height == 0 {
        return ScreenLayout {
            orientation,
            ..ScreenLayout::default()
        };
    }

    let scale = (view_width as f32 / logical_width).min(view_height as f32 / logical_height);
    let fitted_width = logical_width * scale;
    let fitted_height = logical_height * scale;
    let origin_x = (view_width as f32 - fitted_width) * 0.5;
    let origin_y = (view_height as f32 - fitted_height) * 0.5;

    let rect = |(x, y, width, height): (f32, f32, f32, f32)| {
        if width == 0.0 || height == 0.0 {
            return ScreenRect::default();
        }
        let left = (origin_x + x * scale).round() as i32;
        let top = (origin_y + y * scale).round() as i32;
        let right = (origin_x + (x + width) * scale).round() as i32;
        let bottom = (origin_y + (y + height) * scale).round() as i32;
        ScreenRect {
            x: left,
            y: top,
            width: (right - left).max(1) as u32,
            height: (bottom - top).max(1) as u32,
        }
    };

    let mut top = rect(top);
    let mut bottom = rect(bottom);
    if swap_screens {
        std::mem::swap(&mut top, &mut bottom);
    }
    ScreenLayout {
        orientation,
        top,
        bottom,
    }
}

/// Button bits returned by the virtual-control hit test.
pub mod buttons {
    pub const UP: u32 = 1 << 0;
    pub const DOWN: u32 = 1 << 1;
    pub const LEFT: u32 = 1 << 2;
    pub const RIGHT: u32 = 1 << 3;
    pub const A: u32 = 1 << 4;
    pub const B: u32 = 1 << 5;
    pub const X: u32 = 1 << 6;
    pub const Y: u32 = 1 << 7;
    pub const L: u32 = 1 << 8;
    pub const R: u32 = 1 << 9;
    pub const START: u32 = 1 << 10;
    pub const SELECT: u32 = 1 << 11;
    pub const ALL: u32 = (1 << 12) - 1;
}

#[derive(Clone, Copy)]
struct ButtonShape {
    bit: u32,
    x: f32,
    y: f32,
    radius: f32,
    label: &'static str,
}

fn button_shapes(width: u32, height: u32, orientation: u32) -> [ButtonShape; 12] {
    let w = width as f32;
    let h = height as f32;
    let unit = w.min(h);
    let radius = (unit * 0.048).clamp(15.0, 38.0);
    let is_landscape = orientation == ORIENTATION_LANDSCAPE
        || (orientation == ORIENTATION_AUTO && width >= height);
    let (left_x, right_x, control_y) = if is_landscape {
        (w * 0.105, w * 0.895, h * 0.76)
    } else {
        (w * 0.17, w * 0.83, h * 0.76)
    };
    let d = radius * 1.65;
    let face_d = radius * 1.5;
    let shoulder_y = (h * 0.13).max(radius + 4.0);
    let small = (radius * 0.78).max(12.0);
    [
        ButtonShape {
            bit: buttons::UP,
            x: left_x,
            y: control_y - d,
            radius,
            label: "U",
        },
        ButtonShape {
            bit: buttons::DOWN,
            x: left_x,
            y: control_y + d,
            radius,
            label: "D",
        },
        ButtonShape {
            bit: buttons::LEFT,
            x: left_x - d,
            y: control_y,
            radius,
            label: "L",
        },
        ButtonShape {
            bit: buttons::RIGHT,
            x: left_x + d,
            y: control_y,
            radius,
            label: "R",
        },
        ButtonShape {
            bit: buttons::A,
            x: right_x + face_d,
            y: control_y,
            radius,
            label: "A",
        },
        ButtonShape {
            bit: buttons::B,
            x: right_x,
            y: control_y + face_d,
            radius,
            label: "B",
        },
        ButtonShape {
            bit: buttons::X,
            x: right_x,
            y: control_y - face_d,
            radius,
            label: "X",
        },
        ButtonShape {
            bit: buttons::Y,
            x: right_x - face_d,
            y: control_y,
            radius,
            label: "Y",
        },
        ButtonShape {
            bit: buttons::L,
            x: left_x,
            y: shoulder_y,
            radius: small,
            label: "L",
        },
        ButtonShape {
            bit: buttons::R,
            x: right_x,
            y: shoulder_y,
            radius: small,
            label: "R",
        },
        ButtonShape {
            bit: buttons::START,
            x: w * 0.54,
            y: h * 0.94,
            radius: small,
            label: "S",
        },
        ButtonShape {
            bit: buttons::SELECT,
            x: w * 0.46,
            y: h * 0.94,
            radius: small,
            label: "E",
        },
    ]
}

/// Returns the button bit under a touch point, or zero when no control is hit.
pub fn hit_test_virtual_control(width: u32, height: u32, x: i32, y: i32, orientation: u32) -> u32 {
    if width == 0 || height == 0 || x < 0 || y < 0 {
        return 0;
    }
    let x = x as f32;
    let y = y as f32;
    button_shapes(width, height, orientation)
        .into_iter()
        .find(|shape| {
            let dx = x - shape.x;
            let dy = y - shape.y;
            dx * dx + dy * dy <= shape.radius * shape.radius
        })
        .map_or(0, |shape| shape.bit)
}

/// Renders a transparent RGBA8 virtual gamepad overlay for a host surface.
///
/// The overlay uses the original Aceris purple, red, green and text colours.
/// Returns None for zero or excessive dimensions.
pub fn render_virtual_pad_overlay(
    width: u32,
    height: u32,
    orientation: u32,
    pressed_mask: u32,
) -> Option<Vec<u8>> {
    let mut pixels = Vec::new();
    render_virtual_pad_overlay_into(width, height, orientation, pressed_mask, &mut pixels)
        .then_some(pixels)
}

/// Renders the virtual controls into a caller-owned buffer, retaining its allocation.
///
/// Returns false for invalid dimensions or when the output buffer cannot be reserved.
/// On failure the caller's buffer is left unchanged.
pub fn render_virtual_pad_overlay_into(
    width: u32,
    height: u32,
    orientation: u32,
    pressed_mask: u32,
    pixels: &mut Vec<u8>,
) -> bool {
    use aceris_common::brand;

    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return false;
    }
    let Some(byte_len) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
    else {
        return false;
    };
    if pixels.capacity() < byte_len {
        let additional = byte_len.saturating_sub(pixels.len());
        if pixels.try_reserve_exact(additional).is_err() {
            return false;
        }
    }
    pixels.resize(byte_len, 0);
    pixels.fill(0);

    for shape in button_shapes(width, height, orientation) {
        let pressed = pressed_mask & shape.bit != 0;
        let radius = shape.radius;
        let left = (shape.x - radius - 1.0).floor().max(0.0) as i32;
        let top = (shape.y - radius - 1.0).floor().max(0.0) as i32;
        let right = (shape.x + radius + 1.0).ceil().min(width as f32) as i32;
        let bottom = (shape.y + radius + 1.0).ceil().min(height as f32) as i32;
        let inner_radius = (radius - 2.0).max(0.0);
        for py in top..bottom {
            for px in left..right {
                let dx = px as f32 + 0.5 - shape.x;
                let dy = py as f32 + 0.5 - shape.y;
                let distance_squared = dx * dx + dy * dy;
                if distance_squared > radius * radius {
                    continue;
                }
                let border = distance_squared >= inner_radius * inner_radius;
                let (color, alpha) = if pressed {
                    (brand::ACCENT_RED, 218u8)
                } else if border {
                    (brand::TEXT, 220u8)
                } else {
                    (brand::PURPLE_DARK, 154u8)
                };
                let at = (py as usize * width as usize + px as usize) * 4;
                pixels[at..at + 4].copy_from_slice(&[
                    (color >> 16) as u8,
                    (color >> 8) as u8,
                    color as u8,
                    alpha,
                ]);
            }
        }
        draw_glyph(
            pixels,
            width as usize,
            height as usize,
            shape.x.round() as i32,
            shape.y.round() as i32,
            shape.label,
        );
    }

    true
}

/// Identifies which later circles could cover each button.
///
/// The layout normally keeps controls separate. Precomputing the small set of
/// geometrically overlapping circles avoids testing every later control for
/// every pixel in every button, while retaining exact draw-order behavior on
/// compact surfaces where circles can overlap.
fn later_circle_overlap_masks(shapes: &[ButtonShape; 12]) -> [u16; 12] {
    let mut masks = [0u16; 12];
    for (index, shape) in shapes.iter().enumerate() {
        for (later_index, later) in shapes.iter().enumerate().skip(index + 1) {
            let dx = shape.x - later.x;
            let dy = shape.y - later.y;
            let combined_radius = shape.radius + later.radius;
            if dx * dx + dy * dy <= combined_radius * combined_radius {
                masks[index] |= 1u16 << later_index;
            }
        }
    }
    masks
}

/// Blends the virtual controls directly into an RGBA frame without a full-screen overlay.
///
/// Only button bounds are visited. For overlapping controls, the last shape in
/// the normal draw order wins, matching the transparent-overlay renderer.
pub fn blend_virtual_pad_overlay_into(
    frame: &mut [u8],
    width: u32,
    height: u32,
    orientation: u32,
    pressed_mask: u32,
) -> bool {
    blend_virtual_pad_overlay_into_with_opacity(
        frame,
        width,
        height,
        orientation,
        pressed_mask,
        255,
    )
}

/// Alpha-blends the virtual gamepad overlay using the requested overall opacity.
///
/// Opacity 0 leaves the game frame unchanged; 255 preserves the original overlay strength.
pub fn blend_virtual_pad_overlay_into_with_opacity(
    frame: &mut [u8],
    width: u32,
    height: u32,
    orientation: u32,
    pressed_mask: u32,
    opacity: u8,
) -> bool {
    use aceris_common::brand;

    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return false;
    }
    let Some(byte_len) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
    else {
        return false;
    };
    if frame.len() != byte_len {
        return false;
    }

    let shapes = button_shapes(width, height, orientation);
    let later_overlap_masks = later_circle_overlap_masks(&shapes);
    for (index, shape) in shapes.iter().enumerate() {
        let pressed = pressed_mask & shape.bit != 0;
        let radius_squared = shape.radius * shape.radius;
        let inner_radius = (shape.radius - 2.0).max(0.0);
        let inner_radius_squared = inner_radius * inner_radius;
        let left = (shape.x - shape.radius - 1.0).floor().max(0.0) as i32;
        let top = (shape.y - shape.radius - 1.0).floor().max(0.0) as i32;
        let right = (shape.x + shape.radius + 1.0).ceil().min(width as f32) as i32;
        let bottom = (shape.y + shape.radius + 1.0).ceil().min(height as f32) as i32;
        let glyph_cx = shape.x.round() as i32;
        let glyph_cy = shape.y.round() as i32;

        for y in top..bottom {
            for x in left..right {
                let dx = x as f32 + 0.5 - shape.x;
                let dy = y as f32 + 0.5 - shape.y;
                let distance_squared = dx * dx + dy * dy;
                if distance_squared > radius_squared {
                    continue;
                }

                // The original overlay is drawn in shape order, so a later
                // circle replaces the earlier pixel instead of alpha-stacking.
                let mut candidates = later_overlap_masks[index];
                let mut overwritten = false;
                while candidates != 0 {
                    let later_index = candidates.trailing_zeros() as usize;
                    candidates &= candidates - 1;
                    let later = &shapes[later_index];
                    let later_dx = x as f32 + 0.5 - later.x;
                    let later_dy = y as f32 + 0.5 - later.y;
                    if later_dx * later_dx + later_dy * later_dy
                        <= later.radius * later.radius
                    {
                        overwritten = true;
                        break;
                    }
                }
                if overwritten {
                    continue;
                }

                let (color, alpha) = if glyph_contains(shape.label, glyph_cx, glyph_cy, x, y) {
                    ([0xF5, 0xEE, 0xF8], 0xF4u32)
                } else if pressed {
                    (
                        [
                            (brand::ACCENT_RED >> 16) as u8,
                            (brand::ACCENT_RED >> 8) as u8,
                            brand::ACCENT_RED as u8,
                        ],
                        218,
                    )
                } else if distance_squared >= inner_radius_squared {
                    (
                        [
                            (brand::TEXT >> 16) as u8,
                            (brand::TEXT >> 8) as u8,
                            brand::TEXT as u8,
                        ],
                        220,
                    )
                } else {
                    (
                        [
                            (brand::PURPLE_DARK >> 16) as u8,
                            (brand::PURPLE_DARK >> 8) as u8,
                            brand::PURPLE_DARK as u8,
                        ],
                        154,
                    )
                };

                let alpha = alpha * u32::from(opacity) / 255;
                if alpha == 0 {
                    continue;
                }
                let at = (y as usize * width as usize + x as usize) * 4;
                let inverse = 255 - alpha;
                for channel in 0..3 {
                    frame[at + channel] = ((u32::from(color[channel]) * alpha
                        + u32::from(frame[at + channel]) * inverse)
                        / 255) as u8;
                }
            }
        }
    }

    true
}

fn glyph_rows(label: &str) -> &'static [([u8; 7], i32)] {
    match label {
        "A" => &[(
            [
                0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
            ],
            5,
        )],
        "B" => &[(
            [
                0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
            ],
            5,
        )],
        "X" => &[(
            [
                0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b01010, 0b10001,
            ],
            5,
        )],
        "Y" => &[(
            [
                0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
            ],
            5,
        )],
        "L" => &[(
            [
                0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
            ],
            5,
        )],
        "R" => &[(
            [
                0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
            ],
            5,
        )],
        "U" => &[(
            [
                0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
            ],
            5,
        )],
        "D" => &[(
            [
                0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
            ],
            5,
        )],
        "S" => &[(
            [
                0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
            ],
            5,
        )],
        "E" => &[(
            [
                0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
            ],
            5,
        )],
        _ => &[],
    }
}

fn glyph_contains(label: &str, cx: i32, cy: i32, x: i32, y: i32) -> bool {
    glyph_rows(label).iter().any(|(rows, glyph_width)| {
        let column = x - (cx - *glyph_width / 2);
        let row = y - (cy - 3);
        if column < 0 || row < 0 || column >= *glyph_width || row >= 7 {
            return false;
        }
        rows[row as usize] & (1u8 << (*glyph_width - 1 - column)) != 0
    })
}

fn draw_glyph(pixels: &mut [u8], width: usize, height: usize, cx: i32, cy: i32, label: &str) {
    let glyphs = glyph_rows(label);
    for (rows, glyph_width) in glyphs {
        let start_x = cx - *glyph_width / 2;
        let start_y = cy - 3;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..*glyph_width {
                if *bits & (1u8 << (*glyph_width - 1 - column)) == 0 {
                    continue;
                }
                let x = start_x + column;
                let y = start_y + row as i32;
                if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                    continue;
                }
                let at = (y as usize * width + x as usize) * 4;
                pixels[at..at + 4].copy_from_slice(&[0xF5, 0xEE, 0xF8, 0xF4]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concrete_layout_modes_preserve_aspect_and_fit_the_surface() {
        let modes = [
            ScreenLayoutMode::SideBySide,
            ScreenLayoutMode::Vertical,
            ScreenLayoutMode::TopEnlarged,
            ScreenLayoutMode::BottomEnlarged,
            ScreenLayoutMode::TopOnly,
            ScreenLayoutMode::BottomOnly,
        ];
        for mode in modes {
            for (width, height) in [(1080, 2400), (2400, 1080), (720, 1280)] {
                let layout = compute_screen_layout_for_mode(width, height, mode, false);
                for rect in [layout.top, layout.bottom] {
                    if rect.width == 0 || rect.height == 0 {
                        continue;
                    }
                    assert!(rect.x >= 0 && rect.y >= 0, "{mode:?}: {rect:?}");
                    assert!(rect.x as u32 + rect.width <= width, "{mode:?}: {rect:?}");
                    assert!(rect.y as u32 + rect.height <= height, "{mode:?}: {rect:?}");
                }
                if layout.top.width != 0 {
                    let ratio = layout.top.width as f32 / layout.top.height as f32;
                    assert!((ratio - (400.0 / 240.0)).abs() < 0.02, "{mode:?}: {ratio}");
                }
                if layout.bottom.width != 0 {
                    let ratio = layout.bottom.width as f32 / layout.bottom.height as f32;
                    assert!((ratio - (320.0 / 240.0)).abs() < 0.02, "{mode:?}: {ratio}");
                }
            }
        }
    }

    #[test]
    fn single_screen_modes_hide_the_other_physical_lcd() {
        let top = compute_screen_layout_for_mode(1080, 2400, ScreenLayoutMode::TopOnly, false);
        assert!(top.top.width > 0);
        assert_eq!(top.bottom, ScreenRect::default());

        let bottom =
            compute_screen_layout_for_mode(1080, 2400, ScreenLayoutMode::BottomOnly, false);
        assert_eq!(bottom.top, ScreenRect::default());
        assert!(bottom.bottom.width > 0);
    }

    #[test]
    fn swapping_screens_changes_positions_but_preserves_bottom_touch_mapping() {
        let normal =
            compute_screen_layout_for_mode(1080, 2400, ScreenLayoutMode::Vertical, false);
        let swapped =
            compute_screen_layout_for_mode(1080, 2400, ScreenLayoutMode::Vertical, true);
        assert_eq!(normal.top, swapped.bottom);
        assert_eq!(normal.bottom, swapped.top);
        assert_eq!(
            crate::input::touch_from_surface_point(&swapped, swapped.bottom.x, swapped.bottom.y),
            Some((0, 0))
        );
    }

    #[test]
    fn auto_mode_cycles_to_a_concrete_side_by_side_mode() {
        assert_eq!(ScreenLayoutMode::Auto.next(), ScreenLayoutMode::SideBySide);
    }

    #[test]
    fn natural_canvas_sizes_match_each_layout_mode() {
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::Auto), (736, 240));
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::SideBySide), (736, 240));
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::Vertical), (400, 496));
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::TopEnlarged), (480, 544));
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::BottomEnlarged), (400, 556));
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::TopOnly), (400, 240));
        assert_eq!(screen_layout_canvas_size(ScreenLayoutMode::BottomOnly), (320, 240));
    }

    #[test]
    fn layout_mode_cycles_through_all_six_user_selectable_modes() {
        let mut mode = ScreenLayoutMode::SideBySide;
        let mut seen = vec![mode];
        for _ in 0..5 {
            mode = mode.next();
            seen.push(mode);
        }
        assert_eq!(
            seen,
            vec![
                ScreenLayoutMode::SideBySide,
                ScreenLayoutMode::Vertical,
                ScreenLayoutMode::TopEnlarged,
                ScreenLayoutMode::BottomEnlarged,
                ScreenLayoutMode::TopOnly,
                ScreenLayoutMode::BottomOnly,
            ]
        );
        assert_eq!(mode.next(), ScreenLayoutMode::SideBySide);
    }

    #[test]
    fn auto_orientation_follows_surface_and_explicit_mode_wins() {
        assert_eq!(
            compute_screen_layout(1080, 2400, ORIENTATION_AUTO).orientation,
            ORIENTATION_PORTRAIT
        );
        assert_eq!(
            compute_screen_layout(2400, 1080, ORIENTATION_AUTO).orientation,
            ORIENTATION_LANDSCAPE
        );
        assert_eq!(
            compute_screen_layout(1080, 2400, ORIENTATION_LANDSCAPE).orientation,
            ORIENTATION_LANDSCAPE
        );
    }

    #[test]
    fn explicit_portrait_stacks_the_bottom_screen_below_the_top() {
        let layout = compute_screen_layout(1080, 2400, ORIENTATION_PORTRAIT);
        assert_eq!(layout.orientation, ORIENTATION_PORTRAIT);
        assert!(layout.bottom.y >= layout.top.y + layout.top.height as i32);
        assert!(layout.bottom.x > layout.top.x);
    }

    #[test]
    fn explicit_landscape_keeps_screens_side_by_side_even_in_a_tall_surface() {
        let layout = compute_screen_layout(1080, 2400, ORIENTATION_LANDSCAPE);
        assert_eq!(layout.orientation, ORIENTATION_LANDSCAPE);
        assert_eq!(layout.bottom.y, layout.top.y);
        assert!(layout.bottom.x >= layout.top.x + layout.top.width as i32);
    }

    #[test]
    fn portrait_and_landscape_layouts_fit_inside_the_surface() {
        for (width, height, mode) in [
            (1080, 2400, ORIENTATION_PORTRAIT),
            (2400, 1080, ORIENTATION_LANDSCAPE),
            (720, 1280, ORIENTATION_AUTO),
        ] {
            let layout = compute_screen_layout(width, height, mode);
            for rect in [layout.top, layout.bottom] {
                assert!(rect.x >= 0 && rect.y >= 0);
                assert!(rect.x as u32 + rect.width <= width);
                assert!(rect.y as u32 + rect.height <= height);
            }
        }
    }

    #[test]
    fn virtual_controls_hit_test_and_render_with_transparency() {
        let width = 1080;
        let height = 2400;
        let layout = button_shapes(width, height, ORIENTATION_PORTRAIT);
        let a = layout
            .iter()
            .find(|button| button.bit == buttons::A)
            .unwrap();
        assert_eq!(
            hit_test_virtual_control(
                width,
                height,
                a.x.round() as i32,
                a.y.round() as i32,
                ORIENTATION_PORTRAIT
            ),
            buttons::A
        );
        let overlay =
            render_virtual_pad_overlay(width, height, ORIENTATION_PORTRAIT, buttons::A).unwrap();
        assert_eq!(overlay.len(), width as usize * height as usize * 4);
        assert!(overlay.chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(overlay.chunks_exact(4).any(|pixel| pixel[3] > 0));
        assert!(render_virtual_pad_overlay(0, height, ORIENTATION_PORTRAIT, 0).is_none());
    }

    fn blend_overlay_reference(frame: &mut [u8], overlay: &[u8]) {
        for (pixel, over) in frame.chunks_exact_mut(4).zip(overlay.chunks_exact(4)) {
            let alpha = u32::from(over[3]);
            if alpha == 0 {
                continue;
            }
            for channel in 0..3 {
                pixel[channel] = ((u32::from(over[channel]) * alpha
                    + u32::from(pixel[channel]) * (255 - alpha))
                    / 255) as u8;
            }
        }
    }

    #[test]
    fn virtual_overlay_opacity_scales_the_whole_overlay_including_glyphs() {
        let (width, height) = (480, 800);
        let make_frame = || vec![40u8, 80, 120, 255].repeat(width as usize * height as usize);
        let original = make_frame();

        let mut hidden = original.clone();
        assert!(blend_virtual_pad_overlay_into_with_opacity(
            &mut hidden,
            width,
            height,
            ORIENTATION_PORTRAIT,
            buttons::A,
            0,
        ));
        assert_eq!(hidden, original);

        let mut half = original.clone();
        assert!(blend_virtual_pad_overlay_into_with_opacity(
            &mut half,
            width,
            height,
            ORIENTATION_PORTRAIT,
            buttons::A,
            128,
        ));

        let mut full = original.clone();
        assert!(blend_virtual_pad_overlay_into_with_opacity(
            &mut full,
            width,
            height,
            ORIENTATION_PORTRAIT,
            buttons::A,
            255,
        ));
        let difference = |left: &[u8], right: &[u8]| {
            left.iter()
                .zip(right)
                .map(|(a, b)| a.abs_diff(*b) as u64)
                .sum::<u64>()
        };
        assert!(difference(&half, &original) > 0);
        assert!(difference(&half, &original) < difference(&full, &original));
        assert!(
            half.chunks_exact(4)
                .zip(original.chunks_exact(4))
                .all(|(actual, expected)| actual[3] == expected[3])
        );
    }

    #[test]
    fn direct_virtual_overlay_matches_full_frame_reference() {
        let compact_shapes = button_shapes(64, 48, ORIENTATION_AUTO);
        assert!(
            later_circle_overlap_masks(&compact_shapes)
                .iter()
                .any(|mask| *mask != 0),
            "compact layouts should exercise overlap preservation"
        );

        for (width, height, orientation) in [
            (64, 48, ORIENTATION_AUTO),
            (480, 800, ORIENTATION_PORTRAIT),
            (1280, 720, ORIENTATION_LANDSCAPE),
            (1080, 2400, ORIENTATION_AUTO),
        ] {
            for pressed_mask in [
                0,
                buttons::A,
                buttons::B | buttons::X | buttons::UP,
                buttons::ALL,
            ] {
                let mut expected = (0..width as usize * height as usize)
                    .flat_map(|index| {
                        let base = (index as u32).wrapping_mul(37);
                        [
                            (base & 0xFF) as u8,
                            ((base >> 3) & 0xFF) as u8,
                            ((base >> 7) & 0xFF) as u8,
                            255,
                        ]
                    })
                    .collect::<Vec<_>>();
                let mut actual = expected.clone();
                let overlay =
                    render_virtual_pad_overlay(width, height, orientation, pressed_mask).unwrap();
                blend_overlay_reference(&mut expected, &overlay);

                assert!(blend_virtual_pad_overlay_into(
                    &mut actual,
                    width,
                    height,
                    orientation,
                    pressed_mask
                ));
                assert_eq!(actual, expected, "{width}x{height}, mask={pressed_mask:#X}");
            }
        }
    }

    #[test]
    fn direct_virtual_overlay_rejects_invalid_frames_without_writing() {
        let mut frame = vec![0x5A; 64];
        let previous = frame.clone();
        assert!(!blend_virtual_pad_overlay_into(
            &mut frame,
            0,
            4,
            ORIENTATION_AUTO,
            buttons::A
        ));
        assert_eq!(frame, previous);
        assert!(!blend_virtual_pad_overlay_into(
            &mut frame,
            5,
            4,
            ORIENTATION_AUTO,
            buttons::A
        ));
        assert_eq!(frame, previous);
    }

    #[test]
    fn virtual_overlay_into_reuses_storage_and_matches_allocating_renderer() {
        let (width, height) = (480, 800);
        let expected =
            render_virtual_pad_overlay(width, height, ORIENTATION_PORTRAIT, buttons::A).unwrap();
        let mut pixels = Vec::new();
        assert!(render_virtual_pad_overlay_into(
            width,
            height,
            ORIENTATION_PORTRAIT,
            buttons::A,
            &mut pixels
        ));
        assert_eq!(pixels, expected);

        let allocation = pixels.as_ptr();
        let capacity = pixels.capacity();
        let expected_pressed =
            render_virtual_pad_overlay(width, height, ORIENTATION_PORTRAIT, buttons::B).unwrap();
        assert!(render_virtual_pad_overlay_into(
            width,
            height,
            ORIENTATION_PORTRAIT,
            buttons::B,
            &mut pixels
        ));
        assert_eq!(pixels, expected_pressed);
        assert_eq!(pixels.as_ptr(), allocation);
        assert_eq!(pixels.capacity(), capacity);

        let previous = pixels.clone();
        assert!(!render_virtual_pad_overlay_into(
            0,
            height,
            ORIENTATION_PORTRAIT,
            0,
            &mut pixels
        ));
        assert_eq!(pixels, previous);
    }
}
