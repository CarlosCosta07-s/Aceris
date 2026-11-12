//! Desktop window support without external crates.
//!
//! The platform backends talk to the system's window library through raw FFI
//! (loaded at run time, so the build needs no development packages). Only the
//! X11 backend exists so far; other platforms report [`WindowError::Unsupported`].
//!
//! Layout: the top LCD (400x240) sits above the bottom LCD (320x240), which is
//! centered horizontally on a 400x480 canvas that is scaled by an integer factor.

use aceris_core::video::DualScreenDisplay;
use std::fmt;

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub use x11::Window;

#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(not(target_os = "linux"))]
pub use unsupported::Window;

/// Width of the composed canvas in pixels (the top LCD width).
pub const CANVAS_WIDTH: usize = 400;
/// Height of the composed canvas in pixels (both LCDs stacked).
pub const CANVAS_HEIGHT: usize = 480;
const TOP_HEIGHT: usize = 240;
const BOTTOM_WIDTH: usize = 320;

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
}

/// Composes both LCDs into a BGRX canvas scaled by `scale` (nearest neighbor).
///
/// The returned buffer is `CANVAS_WIDTH * scale` by `CANVAS_HEIGHT * scale`
/// pixels, four bytes each, in the byte order of a little-endian 0x00RRGGBB pixel.
pub fn compose_bgrx(display: &DualScreenDisplay, scale: usize) -> Vec<u8> {
    let scale = scale.max(1);
    let mut canvas = [0u8, 0, 0, 0xFF].repeat(CANVAS_WIDTH * CANVAS_HEIGHT);
    let mut blit =
        |surface: &aceris_core::video::ScreenSurface, origin_x: usize, origin_y: usize| {
            for (y, row) in surface
                .pixels()
                .chunks_exact(surface.stride_bytes())
                .enumerate()
            {
                let start = ((origin_y + y) * CANVAS_WIDTH + origin_x) * 4;
                for (x, pixel) in row.chunks_exact(4).enumerate() {
                    let at = start + x * 4;
                    canvas[at..at + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], 0xFF]);
                }
            }
        };
    blit(display.top_left(), 0, 0);
    blit(
        display.bottom(),
        (CANVAS_WIDTH - BOTTOM_WIDTH) / 2,
        TOP_HEIGHT,
    );

    if scale == 1 {
        return canvas;
    }
    let (width, height) = (CANVAS_WIDTH * scale, CANVAS_HEIGHT * scale);
    let mut scaled = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let source_row = &canvas[(y / scale) * CANVAS_WIDTH * 4..][..CANVAS_WIDTH * 4];
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
        let at = (y * CANVAS_WIDTH * scale + x) * 4;
        buffer[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn screens_are_stacked_with_the_bottom_lcd_centered() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 255]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([10, 20, 30, 255]);
        let canvas = compose_bgrx(&display, 1);
        assert_eq!(canvas.len(), CANVAS_WIDTH * CANVAS_HEIGHT * 4);
        // RGBA is reordered to BGRX.
        assert_eq!(pixel(&canvas, 1, 0, 0), [3, 2, 1, 255]);
        assert_eq!(pixel(&canvas, 1, 399, 239), [3, 2, 1, 255]);
        // Margins beside the bottom LCD stay black; the LCD itself starts at x=40.
        assert_eq!(pixel(&canvas, 1, 39, 240), [0, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 1, 40, 240), [30, 20, 10, 255]);
        assert_eq!(pixel(&canvas, 1, 359, 479), [30, 20, 10, 255]);
        assert_eq!(pixel(&canvas, 1, 360, 479), [0, 0, 0, 255]);
    }

    #[test]
    fn integer_scaling_replicates_pixels() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .write_pixel(1, 0, [9, 8, 7, 255])
            .unwrap();
        let scaled = compose_bgrx(&display, 3);
        assert_eq!(scaled.len(), CANVAS_WIDTH * 3 * CANVAS_HEIGHT * 3 * 4);
        for (x, y) in [(3, 0), (5, 0), (3, 2), (5, 2)] {
            assert_eq!(pixel(&scaled, 3, x, y), [7, 8, 9, 255], "({x},{y})");
        }
        assert_eq!(pixel(&scaled, 3, 2, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&scaled, 3, 6, 0), [0, 0, 0, 255]);
    }
}
