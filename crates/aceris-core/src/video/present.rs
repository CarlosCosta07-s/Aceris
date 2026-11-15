//! Writes a rendered RGBA8 image into physical memory as a PDC scanout framebuffer.
//!
//! This is the exact inverse of the LCD controller's row decoder: rows of
//! `width` pixels, `stride` bytes apart, in one of the PDC pixel formats. It
//! follows this crate's scanout convention (row-major, no panel rotation and
//! no tiling), so what it writes is what `LcdController::scanout` reads back.
//! The hardware's display-transfer unit, with its tiled render-target input
//! and rotated output, is not modeled.
//!
//! Narrowing to 565/5551/4444 rounds to the nearest representable value, so
//! expanding a written pixel with the scanout decoder returns the same 5/6/4-bit
//! level. Rgb8 drops alpha, and Rgb5A1 keeps alpha as a threshold at 128.
//!
//! The whole destination range is validated before the first byte is written,
//! so a failure never leaves a half-written frame. Bytes between rows (when the
//! stride exceeds the row size) are left untouched.

use std::fmt;

use super::lcd::LcdPixelFormat;
use crate::gpu::{GpuPhysicalMemory, GpuPhysicalMemoryError, SoftwareRasterizer};

/// Failure while writing a framebuffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramebufferWriteError {
    /// The stride is zero, not a multiple of 8, has bit 31 set, or is shorter than a row.
    InvalidStride {
        stride: u32,
        row_bytes: usize,
    },
    /// The framebuffer size does not fit in the address space arithmetic.
    SizeOverflow,
    PhysicalMemory(GpuPhysicalMemoryError),
}

impl fmt::Display for FramebufferWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStride { stride, row_bytes } => write!(
                formatter,
                "framebuffer stride {stride} is invalid for {row_bytes}-byte rows"
            ),
            Self::SizeOverflow => write!(formatter, "framebuffer size overflows"),
            Self::PhysicalMemory(error) => write!(formatter, "framebuffer write failed: {error}"),
        }
    }
}

impl std::error::Error for FramebufferWriteError {}

impl From<GpuPhysicalMemoryError> for FramebufferWriteError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

/// Smallest valid stride for `width` pixels: the row size rounded up to a multiple of 8.
pub fn minimum_framebuffer_stride(width: usize, format: LcdPixelFormat) -> usize {
    (width * format.bytes_per_pixel()).div_ceil(8) * 8
}

/// Narrows an 8-bit channel to `bits` bits, rounding to nearest.
fn reduce(value: u8, bits: u32) -> u16 {
    let max = (1u32 << bits) - 1;
    ((u32::from(value) * max + 127) / 255) as u16
}

fn encode_pixel(format: LcdPixelFormat, rgba: [u8; 4], out: &mut Vec<u8>) {
    let [r, g, b, a] = rgba;
    match format {
        LcdPixelFormat::Rgba8 => out.extend_from_slice(&[a, b, g, r]),
        LcdPixelFormat::Rgb8 => out.extend_from_slice(&[b, g, r]),
        LcdPixelFormat::Rgb565 => {
            let value = (reduce(r, 5) << 11) | (reduce(g, 6) << 5) | reduce(b, 5);
            out.extend_from_slice(&value.to_le_bytes());
        }
        LcdPixelFormat::Rgb5A1 => {
            let value = (reduce(r, 5) << 11)
                | (reduce(g, 5) << 6)
                | (reduce(b, 5) << 1)
                | u16::from(a >= 128);
            out.extend_from_slice(&value.to_le_bytes());
        }
        LcdPixelFormat::Rgba4 => {
            let value =
                (reduce(r, 4) << 12) | (reduce(g, 4) << 8) | (reduce(b, 4) << 4) | reduce(a, 4);
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
}

/// Writes the rasterizer's color buffer to `base_address` as a PDC framebuffer.
///
/// `stride` is the byte distance between rows and must satisfy the PDC rules
/// (a nonzero multiple of 8, at least one row). Use
/// `minimum_framebuffer_stride` for a tightly packed image.
pub fn present_to_framebuffer(
    rasterizer: &SoftwareRasterizer,
    memory: &mut GpuPhysicalMemory,
    base_address: u32,
    stride: u32,
    format: LcdPixelFormat,
) -> Result<(), FramebufferWriteError> {
    let width = rasterizer.width();
    let height = rasterizer.height();
    let row_bytes = width
        .checked_mul(format.bytes_per_pixel())
        .ok_or(FramebufferWriteError::SizeOverflow)?;
    let stride_bytes = stride as usize;
    if stride == 0 || stride & 0x7 != 0 || stride & 0x8000_0000 != 0 || stride_bytes < row_bytes {
        return Err(FramebufferWriteError::InvalidStride { stride, row_bytes });
    }

    let total = (height - 1)
        .checked_mul(stride_bytes)
        .and_then(|bytes| bytes.checked_add(row_bytes))
        .ok_or(FramebufferWriteError::SizeOverflow)?;
    memory.validate_range(base_address, total)?;

    let color = rasterizer.color_buffer();
    let mut row = Vec::with_capacity(row_bytes);
    for y in 0..height {
        row.clear();
        for x in 0..width {
            let offset = (y * width + x) * 4;
            encode_pixel(
                format,
                [
                    color[offset],
                    color[offset + 1],
                    color[offset + 2],
                    color[offset + 3],
                ],
                &mut row,
            );
        }
        // The range check above covers this address arithmetic.
        memory.write(base_address + (y * stride_bytes) as u32, &row)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        ClipDepthRange, DepthFunction, FCRAM_BASE, GeometryPipeline, GeometryVertex, Matrix4,
        Viewport,
    };
    use crate::video::lcd::{expand_4, expand_5, expand_6};
    use crate::video::{
        DualScreenDisplay, LcdController, LcdPanel, PDC_CONTROL, PDC_FRAMEBUFFER_A_FIRST,
        PDC_FRAMEBUFFER_A_SECOND, PDC_FRAMEBUFFER_B_FIRST, PDC_FRAMEBUFFER_B_SECOND,
        PDC_FRAMEBUFFER_FORMAT, PDC_FRAMEBUFFER_STRIDE, TOP_SCREEN_HEIGHT, TOP_SCREEN_WIDTH,
    };

    const BASE: u32 = FCRAM_BASE + 0x1_0000;
    /// PDC_ENABLE | PDC_OUTPUT_ENABLE
    const ENABLED: u32 = 1 | (1 << 16);

    fn rasterizer(width: usize, height: usize, clear: [u8; 4]) -> SoftwareRasterizer {
        let mut rasterizer = SoftwareRasterizer::new(width, height).unwrap();
        rasterizer.clear(clear, 1.0).unwrap();
        rasterizer
    }

    /// A top-screen-sized target with a colored triangle over the lower-left half.
    fn rendered_top_screen() -> SoftwareRasterizer {
        let mut target = rasterizer(TOP_SCREEN_WIDTH, TOP_SCREEN_HEIGHT, [0, 0, 0, 255]);
        let viewport =
            Viewport::new(0.0, 0.0, TOP_SCREEN_WIDTH as f32, TOP_SCREEN_HEIGHT as f32).unwrap();
        let pipeline = GeometryPipeline::new(
            Matrix4::identity(),
            viewport,
            ClipDepthRange::NegativeOneToOne,
        );
        pipeline
            .draw_triangle(
                [
                    GeometryVertex::new([-1.0, -1.0, 0.0, 1.0], [250, 40, 10, 255]),
                    GeometryVertex::new([1.0, -1.0, 0.0, 1.0], [10, 250, 40, 255]),
                    GeometryVertex::new([-1.0, 1.0, 0.0, 1.0], [40, 10, 250, 255]),
                ],
                &mut target,
                DepthFunction::Less,
            )
            .unwrap();
        target
    }

    fn scan_top_left(
        memory: &GpuPhysicalMemory,
        address: u32,
        format: LcdPixelFormat,
        stride: u32,
    ) -> Vec<u8> {
        let mut controller = LcdController::new();
        for register in [PDC_FRAMEBUFFER_A_FIRST, PDC_FRAMEBUFFER_A_SECOND] {
            controller
                .write_register(LcdPanel::Top, register, address)
                .unwrap();
        }
        for register in [PDC_FRAMEBUFFER_B_FIRST, PDC_FRAMEBUFFER_B_SECOND] {
            controller
                .write_register(LcdPanel::Top, register, address)
                .unwrap();
        }
        controller
            .write_register(LcdPanel::Top, PDC_FRAMEBUFFER_FORMAT, format as u32)
            .unwrap();
        controller
            .write_register(LcdPanel::Top, PDC_FRAMEBUFFER_STRIDE, stride)
            .unwrap();
        controller
            .write_register(LcdPanel::Top, PDC_CONTROL, ENABLED)
            .unwrap();
        let mut display = DualScreenDisplay::new().unwrap();
        controller.scanout(memory, &mut display).unwrap();
        display.top_left().pixels().to_vec()
    }

    #[test]
    fn rgba8_round_trips_exactly_through_lcd_scanout() {
        let target = rendered_top_screen();
        let stride = minimum_framebuffer_stride(TOP_SCREEN_WIDTH, LcdPixelFormat::Rgba8) as u32;
        let mut memory = GpuPhysicalMemory::new();
        present_to_framebuffer(&target, &mut memory, BASE, stride, LcdPixelFormat::Rgba8).unwrap();

        let scanned = scan_top_left(&memory, BASE, LcdPixelFormat::Rgba8, stride);
        assert_eq!(scanned, target.color_buffer());
        // The image is not uniform, so row/column order mistakes would show up.
        assert_ne!(target.color_at(2, 236), target.color_at(397, 3));
    }

    #[test]
    fn rgb8_round_trips_with_opaque_alpha() {
        let target = rendered_top_screen();
        let stride = minimum_framebuffer_stride(TOP_SCREEN_WIDTH, LcdPixelFormat::Rgb8) as u32;
        let mut memory = GpuPhysicalMemory::new();
        present_to_framebuffer(&target, &mut memory, BASE, stride, LcdPixelFormat::Rgb8).unwrap();

        let scanned = scan_top_left(&memory, BASE, LcdPixelFormat::Rgb8, stride);
        assert_eq!(scanned, target.color_buffer());
    }

    #[test]
    fn narrow_formats_land_within_one_quantization_step_of_the_source() {
        let target = rendered_top_screen();
        for (format, tolerance) in [
            (LcdPixelFormat::Rgb565, 5i32),
            (LcdPixelFormat::Rgb5A1, 5),
            (LcdPixelFormat::Rgba4, 9),
        ] {
            let stride = minimum_framebuffer_stride(TOP_SCREEN_WIDTH, format) as u32;
            let mut memory = GpuPhysicalMemory::new();
            present_to_framebuffer(&target, &mut memory, BASE, stride, format).unwrap();
            let scanned = scan_top_left(&memory, BASE, format, stride);
            for (index, (&got, &want)) in scanned.iter().zip(target.color_buffer()).enumerate() {
                let is_alpha = index % 4 == 3;
                if format == LcdPixelFormat::Rgb565 && is_alpha {
                    continue; // 565 has no alpha; the scanout reports it opaque
                }
                assert!(
                    (i32::from(got) - i32::from(want)).abs() <= tolerance,
                    "{format:?} byte {index}: {got} vs {want}"
                );
            }
        }
    }

    #[test]
    fn reduce_inverts_the_scanout_expansion_for_every_level() {
        for level in 0..16u16 {
            assert_eq!(reduce(expand_4(level), 4), level);
        }
        for level in 0..32u16 {
            assert_eq!(reduce(expand_5(level), 5), level);
        }
        for level in 0..64u16 {
            assert_eq!(reduce(expand_6(level), 6), level);
        }
        assert_eq!(reduce(0, 5), 0);
        assert_eq!(reduce(255, 5), 31);
    }

    #[test]
    fn packed_bytes_match_the_scanout_component_order() {
        let mut memory = GpuPhysicalMemory::new();
        let cases: [([u8; 4], LcdPixelFormat, &[u8]); 4] = [
            (
                [0x11, 0x22, 0x33, 0x44],
                LcdPixelFormat::Rgba8,
                &[0x44, 0x33, 0x22, 0x11],
            ),
            (
                [0x11, 0x22, 0x33, 0x44],
                LcdPixelFormat::Rgb8,
                &[0x33, 0x22, 0x11],
            ),
            ([0xFF, 0, 0, 0xFF], LcdPixelFormat::Rgb5A1, &[0x01, 0xF8]),
            (
                [0xAA, 0xBB, 0xCC, 0xDD],
                LcdPixelFormat::Rgba4,
                &[0xCD, 0xAB],
            ),
        ];
        for (rgba, format, expected) in cases {
            let target = rasterizer(1, 1, rgba);
            let stride = minimum_framebuffer_stride(1, format) as u32;
            present_to_framebuffer(&target, &mut memory, BASE, stride, format).unwrap();
            let mut written = vec![0u8; expected.len()];
            memory.read(BASE, &mut written).unwrap();
            assert_eq!(written, expected, "{format:?}");
        }
    }

    #[test]
    fn alpha_for_rgb5a1_is_a_threshold_at_128() {
        let mut memory = GpuPhysicalMemory::new();
        for (alpha, bit) in [(127u8, 0u8), (128, 1)] {
            let target = rasterizer(1, 1, [0, 0, 0, alpha]);
            present_to_framebuffer(&target, &mut memory, BASE, 8, LcdPixelFormat::Rgb5A1).unwrap();
            let mut written = [0u8; 2];
            memory.read(BASE, &mut written).unwrap();
            assert_eq!(written[0] & 1, bit, "alpha {alpha}");
        }
    }

    #[test]
    fn stride_padding_between_rows_is_left_untouched() {
        let mut memory = GpuPhysicalMemory::new();
        memory.write(BASE, &[0xEE; 64]).unwrap();
        let target = rasterizer(2, 2, [1, 2, 3, 4]); // Rgba8 rows are 8 bytes.
        present_to_framebuffer(&target, &mut memory, BASE, 16, LcdPixelFormat::Rgba8).unwrap();

        let mut bytes = [0u8; 32];
        memory.read(BASE, &mut bytes).unwrap();
        assert_eq!(&bytes[..8], &[4, 3, 2, 1, 4, 3, 2, 1]);
        assert_eq!(&bytes[8..16], &[0xEE; 8], "padding after row 0");
        assert_eq!(&bytes[16..24], &[4, 3, 2, 1, 4, 3, 2, 1]);
        assert_eq!(&bytes[24..32], &[0xEE; 8], "nothing past the last row");
    }

    #[test]
    fn invalid_strides_are_rejected_without_writing() {
        let target = rasterizer(4, 2, [9, 9, 9, 9]); // 16-byte Rgba8 rows
        for stride in [0u32, 12, 8, 0x8000_0010] {
            let mut memory = GpuPhysicalMemory::new();
            memory.write(BASE, &[0xEE; 32]).unwrap();
            assert!(
                matches!(
                    present_to_framebuffer(
                        &target,
                        &mut memory,
                        BASE,
                        stride,
                        LcdPixelFormat::Rgba8
                    ),
                    Err(FramebufferWriteError::InvalidStride { .. })
                ),
                "stride {stride:#x}"
            );
            let mut bytes = [0u8; 32];
            memory.read(BASE, &mut bytes).unwrap();
            assert_eq!(bytes, [0xEE; 32]);
        }
    }

    #[test]
    fn unmapped_destination_is_reported_before_any_write() {
        let target = rasterizer(2, 2, [1, 1, 1, 1]);
        let mut memory = GpuPhysicalMemory::new();
        assert!(matches!(
            present_to_framebuffer(&target, &mut memory, 0x1000, 16, LcdPixelFormat::Rgba8),
            Err(FramebufferWriteError::PhysicalMemory(
                GpuPhysicalMemoryError::UnmappedAddress { .. }
            ))
        ));
    }

    #[test]
    fn a_framebuffer_that_runs_past_the_address_space_is_rejected() {
        let mut memory = GpuPhysicalMemory::new();
        let target = rasterizer(2, 2, [7, 7, 7, 7]);
        // The first 16 bytes fit below u32::MAX, but the second row (64 bytes later) does not.
        assert!(matches!(
            present_to_framebuffer(
                &target,
                &mut memory,
                u32::MAX - 15,
                64,
                LcdPixelFormat::Rgba8
            ),
            Err(FramebufferWriteError::PhysicalMemory(_))
        ));
    }

    #[test]
    fn minimum_stride_rounds_up_to_a_multiple_of_eight() {
        assert_eq!(minimum_framebuffer_stride(400, LcdPixelFormat::Rgba8), 1600);
        assert_eq!(minimum_framebuffer_stride(3, LcdPixelFormat::Rgb8), 16); // 9 -> 16
        assert_eq!(minimum_framebuffer_stride(5, LcdPixelFormat::Rgb565), 16); // 10 -> 16
        assert_eq!(minimum_framebuffer_stride(4, LcdPixelFormat::Rgb565), 8);
    }
}
