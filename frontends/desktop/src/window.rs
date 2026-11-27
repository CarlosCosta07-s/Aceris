//! Desktop window support without external crates.
//!
//! The platform backends talk to the system's window library through raw FFI
//! (loaded at run time on Linux, linked on Windows/macOS, so the build needs no
//! development packages). Backends: X11 (Linux, runs in CI-less tests), Win32 and
//! Cocoa (written but never run: see their module docs); other platforms report
//! [`WindowError::Unsupported`].
//!
//! Layout: the top LCD (400x240) is always left of the bottom LCD (320x240).
//! The two screens share a 736x240 gameplay canvas and are scaled together.

use aceris_core::input::{CIRCLE_PAD_LIMIT, InputState, pad, touch_from_surface_point};
use aceris_core::video::{
    DualScreenDisplay,
    frontend::{
        ORIENTATION_LANDSCAPE, ScreenLayoutMode, compute_screen_layout,
        compute_screen_layout_for_mode, screen_layout_canvas_size,
    },
};
use std::fmt;

mod native_input;

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub use x11::Window;

#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
pub use win32::Window;

#[cfg(target_os = "macos")]
mod cocoa;
#[cfg(target_os = "macos")]
pub use cocoa::Window;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub use unsupported::Window;

// Type-checks the Windows and macOS backends on any host (`cargo check` with
// `RUSTFLAGS="--cfg aceris_typecheck_native"`). They are never linked or run
// there; see docs/AI_HANDOFF.md.
#[cfg(all(
    aceris_typecheck_native,
    not(target_os = "windows"),
    not(target_os = "macos")
))]
#[allow(dead_code)]
mod cocoa;
#[cfg(all(
    aceris_typecheck_native,
    not(target_os = "windows"),
    not(target_os = "macos")
))]
#[allow(dead_code)]
mod win32;

/// Width of the home/library canvas in pixels.
pub const CANVAS_WIDTH: usize = 960;
/// Height of the home/library canvas in pixels.
pub const CANVAS_HEIGHT: usize = 600;
const TOP_WIDTH: usize = 400;
const BOTTOM_WIDTH: usize = 320;
const SCREEN_GAP: usize = 16;
/// Width of the desktop gameplay canvas: top LCD + gutter + bottom LCD.
pub const GAME_CANVAS_WIDTH: usize = TOP_WIDTH + SCREEN_GAP + BOTTOM_WIDTH;
/// Height of the desktop gameplay canvas; both LCDs share the same row.
pub const GAME_CANVAS_HEIGHT: usize = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// This platform has no window backend yet.
    #[cfg_attr(
        any(target_os = "linux", target_os = "windows", target_os = "macos"),
        allow(dead_code)
    )]
    Unsupported,
    /// The window system could not be reached or used.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Backend(String),
}

impl fmt::Display for WindowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("windows are not supported on this platform yet")
            }
            Self::Backend(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for WindowError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum WindowEvent {
    /// The user asked to close the window.
    CloseRequested,
    /// Part of the window was exposed and should be redrawn.
    Redraw,
    /// A navigation or editing key was pressed.
    Key(Key),
    /// A printable Unicode character supplied by the platform text-input path.
    Text(char),
    /// The primary mouse button was pressed at window pixel coordinates.
    Click { x: i32, y: i32 },
    /// The mouse wheel moved: negative is up (towards the list start).
    Scroll(i32),
    /// A mapped controller key changed state.
    Game { key: GameKey, pressed: bool },
    /// The primary mouse button is held (`down`) or was released at a window
    /// pixel; sent for motion while held and on release.
    Pointer { x: i32, y: i32, down: bool },
    /// The window lost keyboard focus; held input must be released.
    FocusLost,
}

/// A controller input produced by a keyboard key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum GameKey {
    /// One or more hardware pad bits (`aceris_core::input::pad`).
    Button(u32),
    /// A circle-pad direction: each axis is -1, 0 or 1 (Y up is positive).
    Stick { x: i32, y: i32 },
}

/// Maps an X11 keysym to a controller input.
///
/// Arrows are the D-pad; I/J/K/L the circle pad; X/Z/S/A are A/B/X/Y;
/// Q/W are L/R; 1/2 are ZL/ZR; Return is Start and Backspace is Select.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn game_key_from_keysym(keysym: u64) -> Option<GameKey> {
    let button = |bit| Some(GameKey::Button(bit));
    match keysym {
        0xFF51 => button(pad::LEFT),
        0xFF52 => button(pad::UP),
        0xFF53 => button(pad::RIGHT),
        0xFF54 => button(pad::DOWN),
        0xFF0D | 0xFF8D => button(pad::START),
        0xFF08 => button(pad::SELECT),
        0x78 => button(pad::A),
        0x7A => button(pad::B),
        0x73 => button(pad::X),
        0x61 => button(pad::Y),
        0x71 => button(pad::L),
        0x77 => button(pad::R),
        0x31 => button(pad::ZL),
        0x32 => button(pad::ZR),
        0x69 => Some(GameKey::Stick { x: 0, y: 1 }),
        0x6B => Some(GameKey::Stick { x: 0, y: -1 }),
        0x6A => Some(GameKey::Stick { x: -1, y: 0 }),
        0x6C => Some(GameKey::Stick { x: 1, y: 0 }),
        _ => None,
    }
}

/// Tracks keyboard-driven input and writes it into an [`InputState`].
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyboardInput {
    /// Held circle-pad direction keys: left, right, up, down.
    stick_keys: [bool; 4],
}

impl KeyboardInput {
    pub fn apply(&mut self, key: GameKey, pressed: bool, input: &mut InputState) {
        match key {
            GameKey::Button(bits) => {
                if pressed {
                    input.press(bits);
                } else {
                    input.release(bits);
                }
            }
            GameKey::Stick { x, y } => {
                let slot = match (x, y) {
                    (-1, 0) => 0,
                    (1, 0) => 1,
                    (0, 1) => 2,
                    _ => 3,
                };
                self.stick_keys[slot] = pressed;
                let axis = |negative: bool, positive: bool| {
                    (i32::from(positive) - i32::from(negative)) * i32::from(CIRCLE_PAD_LIMIT)
                };
                input.set_circle_pad(
                    axis(self.stick_keys[0], self.stick_keys[1]),
                    axis(self.stick_keys[3], self.stick_keys[2]),
                );
            }
        }
    }

    /// Forgets held keys, for example on focus loss.
    pub fn reset(&mut self, input: &mut InputState) {
        *self = Self::default();
        input.reset();
    }
}

/// Converts a point in the legacy side-by-side game window into bottom-screen touch.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn touch_from_window_point(x: i32, y: i32, scale: usize) -> Option<(u16, u16)> {
    touch_from_window_point_with_layout(
        x,
        y,
        scale,
        ScreenLayoutMode::SideBySide,
        false,
    )
}

/// Converts a game-window point using the current screen layout and screen swap.
pub fn touch_from_window_point_with_layout(
    x: i32,
    y: i32,
    scale: usize,
    mode: ScreenLayoutMode,
    swap_screens: bool,
) -> Option<(u16, u16)> {
    let scale = i32::try_from(scale.max(1)).ok()?;
    let (width, height) = screen_layout_canvas_size(mode);
    let layout = compute_screen_layout_for_mode(width, height, mode, swap_screens);
    touch_from_surface_point(&layout, x.div_euclid(scale), y.div_euclid(scale))
}

/// Keys the front-end reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Enter,
    Backspace,
    Escape,
}

/// Composes both LCDs side by side into a BGRX canvas, scaled by `scale`.
///
/// The top LCD is always on the left; the bottom LCD is always on the right.
/// The returned buffer is `GAME_CANVAS_WIDTH * scale` by
/// `GAME_CANVAS_HEIGHT * scale`, in little-endian BGRX byte order.
/// Composes the requested screen layout into a reusable BGRX output buffer.
pub fn compose_bgrx_layout_into(
    display: &DualScreenDisplay,
    scale: usize,
    mode: ScreenLayoutMode,
    swap_screens: bool,
    output: &mut Vec<u8>,
) -> bool {
    let scale = scale.max(1);
    if mode == ScreenLayoutMode::SideBySide && !swap_screens {
        return compose_bgrx_into(display, scale, output);
    }

    let (logical_width, logical_height) = screen_layout_canvas_size(mode);
    let Some(width) = (logical_width as usize).checked_mul(scale) else {
        output.clear();
        return false;
    };
    let Some(height) = (logical_height as usize).checked_mul(scale) else {
        output.clear();
        return false;
    };
    let Some(byte_len) = width
        .checked_mul(height)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
    else {
        output.clear();
        return false;
    };
    if output.capacity() < byte_len {
        let additional = byte_len.saturating_sub(output.len());
        if output.try_reserve_exact(additional).is_err() {
            output.clear();
            return false;
        }
    }
    output.resize(byte_len, 0);
    for pixel in output.chunks_exact_mut(4) {
        pixel[3] = 0xFF;
    }

    let layout = compute_screen_layout_for_mode(
        logical_width,
        logical_height,
        mode,
        swap_screens,
    );
    let mut blit = |surface: &aceris_core::video::ScreenSurface, rect: aceris_core::video::frontend::ScreenRect| {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        let Some(destination_width) = (rect.width as usize).checked_mul(scale) else {
            return;
        };
        let Some(destination_height) = (rect.height as usize).checked_mul(scale) else {
            return;
        };
        let origin_x = rect.x.max(0) as usize * scale;
        let origin_y = rect.y.max(0) as usize * scale;
        for destination_y in 0..destination_height {
            let source_y = destination_y * surface.height() / destination_height;
            let source_row = source_y * surface.stride_bytes();
            let output_row = ((origin_y + destination_y) * width + origin_x) * 4;
            for destination_x in 0..destination_width {
                let source_x = destination_x * surface.width() / destination_width;
                let source_at = source_row + source_x * 4;
                let output_at = output_row + destination_x * 4;
                let pixel = &surface.pixels()[source_at..source_at + 4];
                output[output_at..output_at + 4]
                    .copy_from_slice(&[pixel[2], pixel[1], pixel[0], 0xFF]);
            }
        }
    };

    blit(display.top_left(), layout.top);
    blit(display.bottom(), layout.bottom);
    true
}

/// Composes the requested screen layout into an allocated BGRX buffer (test helper).
#[cfg(test)]
pub fn compose_bgrx_layout(
    display: &DualScreenDisplay,
    scale: usize,
    mode: ScreenLayoutMode,
    swap_screens: bool,
) -> Vec<u8> {
    let mut frame = Vec::new();
    assert!(compose_bgrx_layout_into(
        display,
        scale,
        mode,
        swap_screens,
        &mut frame,
    ));
    frame
}

#[cfg(test)]
pub fn compose_bgrx(display: &DualScreenDisplay, scale: usize) -> Vec<u8> {
    let mut frame = Vec::new();
    compose_bgrx_into(display, scale, &mut frame);
    frame
}

/// Composes both LCDs directly into a reusable, scaled BGRX frame buffer.
///
/// Returns false if the requested dimensions overflow or the output allocation fails.
pub fn compose_bgrx_into(display: &DualScreenDisplay, scale: usize, output: &mut Vec<u8>) -> bool {
    let scale = scale.max(1);
    let Some(width) = GAME_CANVAS_WIDTH.checked_mul(scale) else {
        output.clear();
        return false;
    };
    let Some(height) = GAME_CANVAS_HEIGHT.checked_mul(scale) else {
        output.clear();
        return false;
    };
    let Some(byte_len) = width
        .checked_mul(height)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
    else {
        output.clear();
        return false;
    };
    if output.capacity() < byte_len {
        let additional = byte_len.saturating_sub(output.len());
        if output.try_reserve_exact(additional).is_err() {
            output.clear();
            return false;
        }
    }
    output.resize(byte_len, 0);
    let layout = compute_screen_layout(
        GAME_CANVAS_WIDTH as u32,
        GAME_CANVAS_HEIGHT as u32,
        ORIENTATION_LANDSCAPE,
    );
    let top = display.top_left();
    let bottom = display.bottom();
    let gap_start_x = layout.top.x as usize + top.width();
    let gap_end_x = layout.bottom.x as usize;
    let covers_canvas_except_gutter = layout.top.x == 0
        && layout.top.y == 0
        && layout.bottom.y == 0
        && layout.top.width as usize == top.width()
        && layout.top.height as usize == top.height()
        && layout.bottom.width as usize == bottom.width()
        && layout.bottom.height as usize == bottom.height()
        && top.height() == GAME_CANVAS_HEIGHT
        && bottom.height() == GAME_CANVAS_HEIGHT
        && top.stride_bytes() == top.width() * 4
        && bottom.stride_bytes() == bottom.width() * 4
        && gap_start_x <= gap_end_x
        && layout
            .bottom
            .x
            .try_into()
            .ok()
            .and_then(|x: usize| x.checked_add(bottom.width()))
            == Some(GAME_CANVAS_WIDTH);
    if covers_canvas_except_gutter && gap_end_x <= GAME_CANVAS_WIDTH {
        // Both LCDs cover the full canvas height and all columns except this
        // gutter at every integer output scale. The blits overwrite the LCD
        // pixels, so only the gap needs to be initialized.
        let gap_start_x = gap_start_x * scale;
        let gap_end_x = gap_end_x * scale;
        let opaque_black = [0, 0, 0, 0xFF];
        for y in 0..height {
            let row_start = (y * width + gap_start_x) * 4;
            let row_end = (y * width + gap_end_x) * 4;
            for pixel in output[row_start..row_end].chunks_exact_mut(4) {
                pixel.copy_from_slice(&opaque_black);
            }
        }
    } else {
        output.fill(0);
        for pixel in output.chunks_exact_mut(4) {
            pixel[3] = 0xFF;
        }
    }

    let mut blit =
        |surface: &aceris_core::video::ScreenSurface, origin_x: usize, origin_y: usize| {
            for (y, row) in surface
                .pixels()
                .chunks_exact(surface.stride_bytes())
                .enumerate()
            {
                let scaled_y = (origin_y + y) * scale;
                let scaled_x = origin_x * scale;
                if scale == 1 {
                    let row_start = (scaled_y * width + scaled_x) * 4;
                    let row_end = row_start + row.len();
                    for (destination, pixel) in output[row_start..row_end]
                        .chunks_exact_mut(4)
                        .zip(row.chunks_exact(4))
                    {
                        destination[0] = pixel[2];
                        destination[1] = pixel[1];
                        destination[2] = pixel[0];
                        destination[3] = 0xFF;
                    }
                    continue;
                }
                // Expand one contiguous output row first. Copying that row
                // vertically is more cache-friendly than writing each source
                // pixel as a tall column across the destination.
                let expanded_row_start = (scaled_y * width + scaled_x) * 4;
                for (x, pixel) in row.chunks_exact(4).enumerate() {
                    let color = [pixel[2], pixel[1], pixel[0], 0xFF];
                    let column_start = expanded_row_start + x * scale * 4;
                    for column_offset in 0..scale {
                        let at = column_start + column_offset * 4;
                        output[at..at + 4].copy_from_slice(&color);
                    }
                }
                let expanded_row_end = expanded_row_start + row.len() * scale;
                let expanded_row_bytes = expanded_row_end - expanded_row_start;
                for row_offset in 1..scale {
                    let destination_start =
                        ((scaled_y + row_offset) * width + scaled_x) * 4;
                    if destination_start >= expanded_row_end {
                        // The destination rows are disjoint from the first
                        // expanded row, so avoid the overlap-safe memmove path.
                        let (before_destination, destination_and_after) =
                            output.split_at_mut(destination_start);
                        destination_and_after[..expanded_row_bytes].copy_from_slice(
                            &before_destination[expanded_row_start..expanded_row_end],
                        );
                    } else {
                        // Keep the general path safe if a future layout makes
                        // the vertically replicated rows overlap in memory.
                        output.copy_within(expanded_row_start..expanded_row_end, destination_start);
                    }
                }
            }
        };
    blit(
        display.top_left(),
        layout.top.x as usize,
        layout.top.y as usize,
    );
    blit(
        display.bottom(),
        layout.bottom.x as usize,
        layout.bottom.y as usize,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use aceris_core::video::ScreenTarget;

    fn pixel(buffer: &[u8], scale: usize, x: usize, y: usize) -> [u8; 4] {
        let at = (y * GAME_CANVAS_WIDTH * scale + x) * 4;
        buffer[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn each_layout_composes_the_correct_physical_lcd_and_bottom_touch_stays_aligned() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 255]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([10, 20, 30, 255]);

        for mode in [
            ScreenLayoutMode::SideBySide,
            ScreenLayoutMode::Vertical,
            ScreenLayoutMode::TopEnlarged,
            ScreenLayoutMode::BottomEnlarged,
            ScreenLayoutMode::TopOnly,
            ScreenLayoutMode::BottomOnly,
        ] {
            let (logical_width, logical_height) = screen_layout_canvas_size(mode);
            let layout = compute_screen_layout_for_mode(
                logical_width,
                logical_height,
                mode,
                false,
            );
            let canvas = compose_bgrx_layout(&display, 2, mode, false);
            assert_eq!(
                canvas.len(),
                logical_width as usize * 2 * logical_height as usize * 2 * 4,
                "{mode:?}"
            );

            for (rect, expected) in [
                (layout.top, [3, 2, 1, 255]),
                (layout.bottom, [30, 20, 10, 255]),
            ] {
                if rect.width == 0 || rect.height == 0 {
                    continue;
                }
                let x = (rect.x as usize + rect.width as usize / 2) * 2;
                let y = (rect.y as usize + rect.height as usize / 2) * 2;
                let at = (y * logical_width as usize * 2 + x) * 4;
                assert_eq!(&canvas[at..at + 4], &expected, "{mode:?}");
            }

            if layout.bottom.width > 0 {
                let x = layout.bottom.x * 2;
                let y = layout.bottom.y * 2;
                assert_eq!(
                    touch_from_window_point_with_layout(x, y, 2, mode, false),
                    Some((0, 0)),
                    "{mode:?}"
                );
            } else {
                assert_eq!(
                    touch_from_window_point_with_layout(0, 0, 2, mode, false),
                    None,
                    "{mode:?}"
                );
            }
        }
    }

    #[test]
    fn swapped_layout_keeps_the_bottom_screen_touchable_at_its_new_position() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 255]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([10, 20, 30, 255]);
        let mode = ScreenLayoutMode::Vertical;
        let (width, height) = screen_layout_canvas_size(mode);
        let layout = compute_screen_layout_for_mode(width, height, mode, true);
        let canvas = compose_bgrx_layout(&display, 1, mode, true);
        let at = (layout.bottom.y as usize * width as usize + layout.bottom.x as usize) * 4;
        assert_eq!(&canvas[at..at + 4], &[30, 20, 10, 255]);
        assert_eq!(
            touch_from_window_point_with_layout(
                layout.bottom.x,
                layout.bottom.y,
                1,
                mode,
                true
            ),
            Some((0, 0))
        );
    }

    #[test]
    fn screens_are_always_side_by_side_with_a_fixed_gap() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 255]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([10, 20, 30, 255]);
        let canvas = compose_bgrx(&display, 1);
        assert_eq!(canvas.len(), GAME_CANVAS_WIDTH * GAME_CANVAS_HEIGHT * 4);
        // RGBA is reordered to BGRX.
        assert_eq!(pixel(&canvas, 1, 0, 0), [3, 2, 1, 255]);
        // A 16-pixel gutter separates the left/top LCD from the right/bottom LCD.
        assert_eq!(pixel(&canvas, 1, 400, 120), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 415, 120), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 416, 0), [30, 20, 10, 255]);
        assert_eq!(pixel(&canvas, 1, 735, 239), [30, 20, 10, 255]);
    }

    #[test]
    fn native_scale_overwrites_reused_buffer_and_keeps_the_gutter_opaque() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 0]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([4, 5, 6, 0]);
        let mut output = vec![0xA5; GAME_CANVAS_WIDTH * GAME_CANVAS_HEIGHT * 4];

        assert!(compose_bgrx_into(&display, 1, &mut output));

        assert!(output.chunks_exact(4).all(|pixel| pixel[3] == 0xFF));
        assert_eq!(pixel(&output, 1, 0, 0), [3, 2, 1, 0xFF]);
        assert_eq!(pixel(&output, 1, 400, 120), [0, 0, 0, 0xFF]);
        assert_eq!(pixel(&output, 1, 415, 120), [0, 0, 0, 0xFF]);
        assert_eq!(pixel(&output, 1, 416, 0), [6, 5, 4, 0xFF]);
    }

    #[test]
    fn scaled_composition_overwrites_reused_buffer_and_keeps_the_gutter_opaque() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 0]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([4, 5, 6, 0]);

        let mut output = Vec::new();
        for scale in [2, 3, 4] {
            let width = GAME_CANVAS_WIDTH * scale;
            let height = GAME_CANVAS_HEIGHT * scale;
            let byte_len = width * height * 4;
            output.resize(byte_len, 0xA5);
            output.fill(0xA5);

            assert!(compose_bgrx_into(&display, scale, &mut output));
            assert!(
                output.chunks_exact(4).all(|pixel| pixel[3] == 0xFF),
                "non-opaque pixel at scale {scale}"
            );
            assert_eq!(pixel(&output, scale, 0, 0), [3, 2, 1, 0xFF]);
            assert_eq!(pixel(&output, scale, 416 * scale, 0), [6, 5, 4, 0xFF]);
            for row in output.chunks_exact(width * 4) {
                assert!(
                    row[400 * scale * 4..416 * scale * 4]
                        .chunks_exact(4)
                        .all(|pixel| pixel == [0, 0, 0, 0xFF]),
                    "non-black gutter pixel at scale {scale}"
                );
            }
        }
    }

    #[test]
    fn row_major_scaled_composition_matches_reference_pixel_for_pixel() {
        let mut display = DualScreenDisplay::new().unwrap();
        for target in [ScreenTarget::TopLeft, ScreenTarget::Bottom] {
            let surface = display.surface_mut(target);
            let mut pattern = Vec::with_capacity(surface.pixels().len());
            for y in 0..surface.height() {
                for x in 0..surface.width() {
                    pattern.extend_from_slice(&[
                        x.wrapping_mul(3) as u8,
                        y.wrapping_mul(5) as u8,
                        (x ^ y) as u8,
                        (x.wrapping_add(y)) as u8,
                    ]);
                }
            }
            surface.copy_from_rgba8(&pattern).unwrap();
        }

        let layout = compute_screen_layout(
            GAME_CANVAS_WIDTH as u32,
            GAME_CANVAS_HEIGHT as u32,
            ORIENTATION_LANDSCAPE,
        );
        for scale in [2, 3, 4] {
            let width = GAME_CANVAS_WIDTH * scale;
            let height = GAME_CANVAS_HEIGHT * scale;
            let mut expected = vec![0; width * height * 4];
            for pixel in expected.chunks_exact_mut(4) {
                pixel[3] = 0xFF;
            }

            for (surface, rect) in [
                (display.top_left(), layout.top),
                (display.bottom(), layout.bottom),
            ] {
                for (source_y, row) in surface
                    .pixels()
                    .chunks_exact(surface.stride_bytes())
                    .enumerate()
                {
                    for (source_x, pixel) in row.chunks_exact(4).enumerate() {
                        let color = [pixel[2], pixel[1], pixel[0], 0xFF];
                        for row_offset in 0..scale {
                            let y = (rect.y as usize + source_y) * scale + row_offset;
                            for column_offset in 0..scale {
                                let x = (rect.x as usize + source_x) * scale + column_offset;
                                let at = (y * width + x) * 4;
                                expected[at..at + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }

            let mut actual = vec![0xA5; width * height * 4];
            assert!(compose_bgrx_into(&display, scale, &mut actual));
            assert_eq!(actual, expected, "scaled composition differs at scale {scale}");
        }
    }

    #[test]
    fn integer_scaling_replicates_pixels() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .write_pixel(1, 0, [9, 8, 7, 255])
            .unwrap();
        display
            .surface_mut(ScreenTarget::Bottom)
            .write_pixel(1, 0, [4, 5, 6, 255])
            .unwrap();
        let scaled = compose_bgrx(&display, 3);
        assert_eq!(
            scaled.len(),
            GAME_CANVAS_WIDTH * 3 * GAME_CANVAS_HEIGHT * 3 * 4
        );
        for (x, y) in [(3, 0), (5, 0), (3, 2), (5, 2)] {
            assert_eq!(pixel(&scaled, 3, x, y), [7, 8, 9, 255], "({x},{y})");
        }
        assert_eq!(pixel(&scaled, 3, 2, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&scaled, 3, 6, 0), [0, 0, 0, 255]);
        for x in [416 * 3 + 3, 416 * 3 + 5] {
            assert_eq!(pixel(&scaled, 3, x, 0), [6, 5, 4, 255]);
        }
        // The second LCD starts at x=416 and is scaled without a vertical offset.
        assert_eq!(pixel(&scaled, 3, 416 * 3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn compose_bgrx_into_reuses_the_scaled_output_allocation() {
        let display = DualScreenDisplay::new().unwrap();
        let mut output = Vec::new();
        assert!(compose_bgrx_into(&display, 2, &mut output));
        let allocation = output.as_ptr();
        let capacity = output.capacity();
        let expected = compose_bgrx(&display, 2);
        assert_eq!(output, expected);
        assert!(compose_bgrx_into(&display, 2, &mut output));
        assert_eq!(output.as_ptr(), allocation);
        assert_eq!(output.capacity(), capacity);
        assert!(!compose_bgrx_into(&display, usize::MAX, &mut output));
        assert!(output.is_empty());
    }

    #[test]
    fn keysyms_map_to_controller_inputs() {
        assert_eq!(game_key_from_keysym(0xFF52), Some(GameKey::Button(pad::UP)));
        assert_eq!(game_key_from_keysym(0x78), Some(GameKey::Button(pad::A)));
        assert_eq!(
            game_key_from_keysym(0xFF0D),
            Some(GameKey::Button(pad::START))
        );
        assert_eq!(
            game_key_from_keysym(0x69),
            Some(GameKey::Stick { x: 0, y: 1 })
        );
        assert_eq!(game_key_from_keysym(0x7E), None);
    }

    #[test]
    fn keyboard_buttons_and_stick_combine_opposite_keys() {
        let mut input = InputState::default();
        let mut keyboard = KeyboardInput::default();
        keyboard.apply(GameKey::Button(pad::A), true, &mut input);
        keyboard.apply(GameKey::Stick { x: 1, y: 0 }, true, &mut input);
        keyboard.apply(GameKey::Stick { x: 0, y: 1 }, true, &mut input);
        let state = input.snapshot();
        assert_eq!(state.held, pad::A);
        assert_eq!(state.circle_pad, (156, 156));
        // Opposite keys cancel; releasing one leaves the other axis.
        keyboard.apply(GameKey::Stick { x: -1, y: 0 }, true, &mut input);
        assert_eq!(input.snapshot().circle_pad, (0, 156));
        keyboard.apply(GameKey::Stick { x: 0, y: 1 }, false, &mut input);
        keyboard.apply(GameKey::Stick { x: -1, y: 0 }, false, &mut input);
        keyboard.apply(GameKey::Stick { x: 1, y: 0 }, false, &mut input);
        assert_eq!(input.snapshot().circle_pad, (0, 0));
        keyboard.reset(&mut input);
        assert_eq!(input.snapshot().held, 0);
    }

    #[test]
    fn window_points_map_onto_the_bottom_screen_at_any_scale() {
        let layout = compute_screen_layout(
            GAME_CANVAS_WIDTH as u32,
            GAME_CANVAS_HEIGHT as u32,
            ORIENTATION_LANDSCAPE,
        );
        for scale in [1, 2, 3] {
            let x = layout.bottom.x * scale as i32;
            let y = layout.bottom.y * scale as i32;
            assert_eq!(touch_from_window_point(x, y, scale), Some((0, 0)));
            assert_eq!(
                touch_from_window_point(x + scale as i32 * 160, y + scale as i32 * 120, scale),
                Some((160, 120))
            );
            // The top screen is not touch sensitive.
            let top = (layout.top.x * scale as i32, layout.top.y * scale as i32);
            assert_eq!(touch_from_window_point(top.0, top.1, scale), None);
        }
    }
}
