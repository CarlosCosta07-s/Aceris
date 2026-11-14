//! Dual-screen LCD output surfaces.
//!
//! The top LCD exposes independent left/right eye buffers for stereoscopic
//! output. The bottom LCD has one buffer. Frontends consume tightly packed
//! RGBA8 surfaces; the core does not depend on a windowing system.

pub mod font;
pub mod frontend;
pub mod intro;
mod lcd;
pub mod library;
pub mod png;
pub use lcd::{
    LcdController, LcdControllerError, LcdPanel, LcdPixelFormat, LcdScanoutReport, PDC_CONTROL,
    PDC_FRAMEBUFFER_A_FIRST, PDC_FRAMEBUFFER_A_SECOND, PDC_FRAMEBUFFER_B_FIRST,
    PDC_FRAMEBUFFER_B_SECOND, PDC_FRAMEBUFFER_FORMAT, PDC_FRAMEBUFFER_SELECT,
    PDC_FRAMEBUFFER_STRIDE,
};

use std::fmt;

/// Logical top LCD width in pixels.
pub const TOP_SCREEN_WIDTH: usize = 400;
/// Logical top LCD height in pixels.
pub const TOP_SCREEN_HEIGHT: usize = 240;
/// Logical bottom LCD width in pixels.
pub const BOTTOM_SCREEN_WIDTH: usize = 320;
/// Logical bottom LCD height in pixels.
pub const BOTTOM_SCREEN_HEIGHT: usize = 240;
/// Bytes per RGBA8 pixel.
pub const RGBA8_BYTES_PER_PIXEL: usize = 4;

/// One independently addressable LCD target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenTarget {
    TopLeft,
    TopRight,
    Bottom,
}

/// Failure while creating or updating an LCD output surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenSurfaceError {
    InvalidDimensions { width: usize, height: usize },
    AllocationFailed,
    InvalidPixel { x: usize, y: usize },
    InvalidBufferLength { expected: usize, actual: usize },
}

impl fmt::Display for ScreenSurfaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDimensions { width, height } => {
                write!(formatter, "invalid LCD surface dimensions {width}x{height}")
            }
            Self::AllocationFailed => formatter.write_str("could not allocate LCD surface"),
            Self::InvalidPixel { x, y } => {
                write!(
                    formatter,
                    "LCD pixel coordinate ({x}, {y}) is out of bounds"
                )
            }
            Self::InvalidBufferLength { expected, actual } => {
                write!(
                    formatter,
                    "LCD buffer has {actual} bytes, expected {expected}"
                )
            }
        }
    }
}

impl std::error::Error for ScreenSurfaceError {}

/// Tightly packed RGBA8 screen image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSurface {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl ScreenSurface {
    /// Allocates an opaque-black RGBA8 surface.
    pub fn new(width: usize, height: usize) -> Result<Self, ScreenSurfaceError> {
        if width == 0 || height == 0 {
            return Err(ScreenSurfaceError::InvalidDimensions { width, height });
        }
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(RGBA8_BYTES_PER_PIXEL))
            .ok_or(ScreenSurfaceError::InvalidDimensions { width, height })?;
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(byte_len)
            .map_err(|_| ScreenSurfaceError::AllocationFailed)?;
        pixels.resize(byte_len, 0);
        for alpha in pixels.iter_mut().skip(3).step_by(4) {
            *alpha = 0xFF;
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub const fn stride_bytes(&self) -> usize {
        self.width * RGBA8_BYTES_PER_PIXEL
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Encodes the surface as an RGBA8 PNG image.
    pub fn to_png(&self) -> Vec<u8> {
        png::encode_rgba8(self.width, self.height, &self.pixels)
            .expect("surface dimensions and pixel buffer always agree")
    }

    pub fn copy_from_rgba8(&mut self, pixels: &[u8]) -> Result<(), ScreenSurfaceError> {
        if pixels.len() != self.pixels.len() {
            return Err(ScreenSurfaceError::InvalidBufferLength {
                expected: self.pixels.len(),
                actual: pixels.len(),
            });
        }
        self.pixels.copy_from_slice(pixels);
        Ok(())
    }

    pub fn clear(&mut self, rgba: [u8; 4]) {
        for pixel in self.pixels.chunks_exact_mut(RGBA8_BYTES_PER_PIXEL) {
            pixel.copy_from_slice(&rgba);
        }
    }

    pub fn write_pixel(
        &mut self,
        x: usize,
        y: usize,
        rgba: [u8; 4],
    ) -> Result<(), ScreenSurfaceError> {
        if x >= self.width || y >= self.height {
            return Err(ScreenSurfaceError::InvalidPixel { x, y });
        }
        let offset = (y * self.width + x) * RGBA8_BYTES_PER_PIXEL;
        self.pixels[offset..offset + RGBA8_BYTES_PER_PIXEL].copy_from_slice(&rgba);
        Ok(())
    }
}

/// The three independently addressable LCD output surfaces.
#[derive(Clone, Debug)]
pub struct DualScreenDisplay {
    top_left: ScreenSurface,
    top_right: ScreenSurface,
    bottom: ScreenSurface,
    frame_number: u64,
}

impl Default for DualScreenDisplay {
    fn default() -> Self {
        Self::new().expect("fixed Nintendo 3DS LCD surfaces fit host memory")
    }
}

impl DualScreenDisplay {
    pub fn new() -> Result<Self, ScreenSurfaceError> {
        Ok(Self {
            top_left: ScreenSurface::new(TOP_SCREEN_WIDTH, TOP_SCREEN_HEIGHT)?,
            top_right: ScreenSurface::new(TOP_SCREEN_WIDTH, TOP_SCREEN_HEIGHT)?,
            bottom: ScreenSurface::new(BOTTOM_SCREEN_WIDTH, BOTTOM_SCREEN_HEIGHT)?,
            frame_number: 0,
        })
    }

    pub const fn top_left(&self) -> &ScreenSurface {
        &self.top_left
    }

    pub const fn top_right(&self) -> &ScreenSurface {
        &self.top_right
    }

    pub const fn bottom(&self) -> &ScreenSurface {
        &self.bottom
    }

    pub const fn frame_number(&self) -> u64 {
        self.frame_number
    }

    pub fn surface_mut(&mut self, target: ScreenTarget) -> &mut ScreenSurface {
        match target {
            ScreenTarget::TopLeft => &mut self.top_left,
            ScreenTarget::TopRight => &mut self.top_right,
            ScreenTarget::Bottom => &mut self.bottom,
        }
    }

    pub fn present(&mut self) -> u64 {
        self.frame_number = self.frame_number.saturating_add(1);
        self.frame_number
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_both_screens_and_both_top_stereo_views() {
        let display = DualScreenDisplay::new().unwrap();
        assert_eq!(
            (display.top_left().width(), display.top_left().height()),
            (400, 240)
        );
        assert_eq!(
            (display.top_right().width(), display.top_right().height()),
            (400, 240)
        );
        assert_eq!(
            (display.bottom().width(), display.bottom().height()),
            (320, 240)
        );
    }

    #[test]
    fn writes_pixels_and_rejects_out_of_bounds_coordinates() {
        let mut surface = ScreenSurface::new(2, 2).unwrap();
        surface.write_pixel(1, 1, [1, 2, 3, 4]).unwrap();
        assert_eq!(&surface.pixels()[12..16], &[1, 2, 3, 4]);
        assert_eq!(
            surface.write_pixel(2, 0, [0; 4]),
            Err(ScreenSurfaceError::InvalidPixel { x: 2, y: 0 })
        );
    }

    #[test]
    fn surfaces_are_independent_and_presentation_is_monotonic() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([1, 2, 3, 4]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([5, 6, 7, 8]);
        assert_eq!(&display.top_left().pixels()[..4], &[1, 2, 3, 4]);
        assert_eq!(&display.top_right().pixels()[..4], &[0, 0, 0, 255]);
        assert_eq!(&display.bottom().pixels()[..4], &[5, 6, 7, 8]);
        assert_eq!(display.present(), 1);
        assert_eq!(display.present(), 2);
    }
}
