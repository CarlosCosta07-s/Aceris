//! Desktop window support without external crates.
//!
//! The platform backends talk to the system's window library through raw FFI
//! (loaded at run time, so the build needs no development packages). Only the
//! X11 backend exists so far; other platforms report [`WindowError::Unsupported`].
//!
//! Layout: the top LCD (400x240) is always left of the bottom LCD (320x240).
//! The two screens share a 736x240 gameplay canvas and are scaled together.

use aceris_core::video::{
    DualScreenDisplay,
    frontend::{ORIENTATION_LANDSCAPE, compute_screen_layout},
};
use std::fmt;

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub use x11::Window;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::Window;

/// Width of the home/library canvas in pixels.
pub const CANVAS_WIDTH: usize = 400;
/// Height of the home/library canvas in pixels.
pub const CANVAS_HEIGHT: usize = 480;
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
    #[cfg_attr(target_os = "linux", allow(dead_code))]
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
    /// A navigation key was pressed.
    Key(Key),
    /// The primary mouse button was pressed at window pixel coordinates.
    Click { x: i32, y: i32 },
    /// The mouse wheel moved: negative is up (towards the list start).
    Scroll(i32),
}

/// Keys the front-end reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Enter,
    Escape,
}

/// Composes both LCDs side by side into a BGRX canvas, scaled by `scale`.
///
/// The top LCD is always on the left; the bottom LCD is always on the right.
/// The returned buffer is `GAME_CANVAS_WIDTH * scale` by
/// `GAME_CANVAS_HEIGHT * scale`, in little-endian BGRX byte order.
pub fn compose_bgrx(display: &DualScreenDisplay, scale: usize) -> Vec<u8> {
    let scale = scale.max(1);
    let mut canvas =
        [0u8, 0, 0, 0xFF].repeat(GAME_CANVAS_WIDTH * GAME_CANVAS_HEIGHT);
    let mut blit =
        |surface: &aceris_core::video::ScreenSurface, origin_x: usize, origin_y: usize| {
            for (y, row) in surface
                .pixels()
                .chunks_exact(surface.stride_bytes())
                .enumerate()
            {
                let start = ((origin_y + y) * GAME_CANVAS_WIDTH + origin_x) * 4;
                for (x, pixel) in row.chunks_exact(4).enumerate() {
                    let at = start + x * 4;
                    canvas[at..at + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 0xFF]);
                }
            }
        };
    let layout = compute_screen_layout(
        GAME_CANVAS_WIDTH as u32,
        GAME_CANVAS_HEIGHT as u32,
        ORIENTATION_LANDSCAPE,
    );
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

    if scale == 1 {
        return canvas;
    }
    let (width, height) = (GAME_CANVAS_WIDTH * scale, GAME_CANVAS_HEIGHT * scale);
    let mut scaled = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let source_row = &canvas[(y / scale) * GAME_CANVAS_WIDTH * 4..]
            [..GAME_CANVAS_WIDTH * 4];
        for pixel in source_row.chunks_exact(4) {
            for _ in 0..scale {
                scaled.extend_from_slice(pixel);
            }
        }
    }
    scaled
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
        assert_eq!(
            pixel(&scaled, 3, 416 * 3, 0),
            [0, 0, 0, 255]
        );
    }
}
