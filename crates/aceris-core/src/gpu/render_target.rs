//! PICA200 render-target register decoding and Morton color-buffer storage.
//!
//! The render target is described by GPUREG_COLORBUFFER_LOC/FORMAT,
//! GPUREG_FRAMEBUFFER_BLOCK32 and GPUREG_FRAMEBUFFER_DIM. This module decodes
//! those values and provides 8x8 and 32x32 block encoders/decoders for the
//! software rasterizer's RGBA8 color output.
//!
//! Public documentation identifies both block sizes but does not specify the
//! 32x32 intra-block order. That mode is modeled as a row-major grid of tiles,
//! each using 5-bit-per-axis Morton order, and remains unverified on hardware.
//! The tentative vertical-flip flag is exposed without applying it.

use std::fmt;

use super::{GpuPhysicalMemory, GpuPhysicalMemoryError, Pica200, SoftwareRasterizer};

/// GPUREG_COLORBUFFER_FORMAT.
pub const REG_COLORBUFFER_FORMAT: u16 = 0x117;
/// GPUREG_FRAMEBUFFER_BLOCK32.
pub const REG_FRAMEBUFFER_BLOCK32: u16 = 0x11B;
/// GPUREG_COLORBUFFER_LOC.
pub const REG_COLORBUFFER_LOC: u16 = 0x11D;
/// GPUREG_FRAMEBUFFER_DIM.
pub const REG_FRAMEBUFFER_DIM: u16 = 0x11E;

const TILE_EDGE_8X8: usize = 8;
const TILE_EDGE_32X32: usize = 32;
const COLOR_LOCATION_MASK: u32 = 0x0FFF_FFFF;
const DIMENSION_WIDTH_MASK: u32 = 0x7FF;
const DIMENSION_HEIGHT_MASK: u32 = 0x3FF;

/// Raw three-bit GPUREG_COLORBUFFER_FORMAT format selector.
///
/// RGB8 is named because it appears in the Azahar framebuffer enum, but the
/// independently documented pixel-size/format pairing is not established.
/// It is therefore identified for diagnostics without enabling framebuffer
/// writes or inventing a pixel-size combination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaColorBufferFormatSelector {
    Rgba8,
    Rgb8,
    Rgb5A1,
    Rgb565,
    Rgba4,
    Reserved(u8),
}

impl PicaColorBufferFormatSelector {
    /// Decodes bits 16..18 of GPUREG_COLORBUFFER_FORMAT.
    pub const fn from_register(value: u32) -> Self {
        match value & 0x7 {
            0 => Self::Rgba8,
            1 => Self::Rgb8,
            2 => Self::Rgb5A1,
            3 => Self::Rgb565,
            4 => Self::Rgba4,
            other => Self::Reserved(other as u8),
        }
    }

    /// Returns the raw format selector.
    pub const fn raw_value(self) -> u8 {
        match self {
            Self::Rgba8 => 0,
            Self::Rgb8 => 1,
            Self::Rgb5A1 => 2,
            Self::Rgb565 => 3,
            Self::Rgba4 => 4,
            Self::Reserved(value) => value,
        }
    }
}

/// Color format selected by GPUREG_COLORBUFFER_FORMAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaColorBufferFormat {
    /// 32-bit RGBA8, stored in memory as A, B, G, R.
    Rgba8,
    /// 16-bit RGB5A1.
    Rgb5A1,
    /// 16-bit RGB565.
    Rgb565,
    /// 16-bit RGBA4.
    Rgba4,
}

impl PicaColorBufferFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb5A1 | Self::Rgb565 | Self::Rgba4 => 2,
        }
    }

    /// Rounds an RGBA8 pixel to what this format can store: encoding and
    /// decoding it again. Alpha reads back as 255 for formats without alpha.
    pub(super) fn quantize(self, rgba: [u8; 4]) -> [u8; 4] {
        let size = self.bytes_per_pixel();
        let mut stored = [0u8; 4];
        self.encode_pixel(rgba, &mut stored[..size]);
        self.decode_pixel(&stored[..size])
    }

    fn decode_register(value: u32) -> Result<Self, PicaRenderTargetError> {
        let pixel_size = (value & 0x3) as u8;
        let selector = PicaColorBufferFormatSelector::from_register(value >> 16);
        let format = selector.raw_value();
        match (pixel_size, selector) {
            (2, PicaColorBufferFormatSelector::Rgba8) => Ok(Self::Rgba8),
            (0, PicaColorBufferFormatSelector::Rgb5A1) => Ok(Self::Rgb5A1),
            (0, PicaColorBufferFormatSelector::Rgb565) => Ok(Self::Rgb565),
            (0, PicaColorBufferFormatSelector::Rgba4) => Ok(Self::Rgba4),
            _ => Err(PicaRenderTargetError::UnsupportedColorFormat { pixel_size, format }),
        }
    }
}

/// Render-target block size selected by GPUREG_FRAMEBUFFER_BLOCK32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaRenderBlockMode {
    Blocks8x8,
    Blocks32x32,
}

/// Decoded PICA200 color render-target state.
///
/// The physical base address is reconstructed from the register's address
/// units (the stored value is the physical address shifted right by three).
/// Width is stored directly; height is stored as height minus one.
///
/// The hardware documentation labels bit 24 of the dimension register as a
/// possible vertical-flip control. The flag is retained here but is not applied
/// by the software codec until its effect is independently validated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaRenderTargetConfig {
    pub color_buffer_address: u32,
    pub width: u32,
    pub height: u32,
    pub format: PicaColorBufferFormat,
    pub block_mode: PicaRenderBlockMode,
    pub vertical_flip_enabled: bool,
}

impl PicaRenderTargetConfig {
    /// Decodes the four framebuffer registers without touching physical memory.
    pub fn from_registers(
        color_location: u32,
        color_format: u32,
        block_format: u32,
        dimensions: u32,
    ) -> Result<Self, PicaRenderTargetError> {
        if color_location & !COLOR_LOCATION_MASK != 0 {
            return Err(PicaRenderTargetError::InvalidColorLocationRegister(
                color_location,
            ));
        }

        let width = dimensions & DIMENSION_WIDTH_MASK;
        let height = ((dimensions >> 12) & DIMENSION_HEIGHT_MASK) + 1;
        if width == 0 {
            return Err(PicaRenderTargetError::InvalidDimensions { width, height });
        }

        let format = PicaColorBufferFormat::decode_register(color_format)?;
        let block_mode = if block_format & 1 == 0 {
            PicaRenderBlockMode::Blocks8x8
        } else {
            PicaRenderBlockMode::Blocks32x32
        };

        Ok(Self {
            color_buffer_address: color_location << 3,
            width,
            height,
            format,
            block_mode,
            vertical_flip_enabled: dimensions & (1 << 24) == 0,
        })
    }

    /// Returns the number of bytes occupied by the padded block grid.
    ///
    /// Width and height need not be exact multiples of the selected block edge;
    /// the final tile row/column is padded in the backing storage.
    pub fn tiled_storage_size(self) -> Result<usize, PicaRenderTargetError> {
        let tile_edge = self.block_edge();
        let tiles_x = tile_count(self.width as usize, tile_edge)?;
        let tiles_y = tile_count(self.height as usize, tile_edge)?;
        let tile_pixels = tile_edge
            .checked_mul(tile_edge)
            .ok_or(PicaRenderTargetError::SizeOverflow)?;
        tiles_x
            .checked_mul(tiles_y)
            .and_then(|tiles| tiles.checked_mul(tile_pixels))
            .and_then(|pixels| pixels.checked_mul(self.format.bytes_per_pixel()))
            .ok_or(PicaRenderTargetError::SizeOverflow)
    }

    /// Encodes the rasterizer color buffer into 8x8 Morton-tiled storage.
    pub fn encode_rasterizer(
        self,
        rasterizer: &SoftwareRasterizer,
    ) -> Result<Vec<u8>, PicaRenderTargetError> {
        self.validate_rasterizer_dimensions(rasterizer)?;
        let output_len = self.tiled_storage_size()?;
        let bytes_per_pixel = self.format.bytes_per_pixel();
        let tile_edge = self.block_edge();
        let tiles_x = tile_count(self.width as usize, tile_edge)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(output_len)
            .map_err(|_| PicaRenderTargetError::AllocationFailed)?;
        output.resize(output_len, 0);

        let source = rasterizer.color_buffer();
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let source_offset = (y * self.width as usize + x) * 4;
                let destination_offset =
                    tiled_pixel_offset(x, y, tiles_x, bytes_per_pixel, tile_edge)?;
                self.format.encode_pixel(
                    [
                        source[source_offset],
                        source[source_offset + 1],
                        source[source_offset + 2],
                        source[source_offset + 3],
                    ],
                    &mut output[destination_offset..destination_offset + bytes_per_pixel],
                );
            }
        }
        Ok(output)
    }

    /// Writes the encoded render target into validated physical GPU memory.
    ///
    /// The complete padded destination range is validated before the first
    /// write. The write also uses the memory backend's range validation.
    pub fn write_rasterizer(
        self,
        rasterizer: &SoftwareRasterizer,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<PicaRenderTargetWriteReport, PicaRenderTargetError> {
        self.validate_rasterizer_dimensions(rasterizer)?;
        let bytes_written = self.tiled_storage_size()?;
        memory.validate_range(self.color_buffer_address, bytes_written)?;
        let encoded = self.encode_rasterizer(rasterizer)?;
        memory.write(self.color_buffer_address, &encoded)?;

        let tile_edge = self.block_edge();
        let tiles_x = tile_count(self.width as usize, tile_edge)?;
        let tiles_y = tile_count(self.height as usize, tile_edge)?;
        Ok(PicaRenderTargetWriteReport {
            width: self.width,
            height: self.height,
            tiles_written: (tiles_x * tiles_y) as u64,
            bytes_written: encoded.len() as u64,
        })
    }

    /// Reads and untile-decodes the color target into tightly packed RGBA8.
    pub fn read_rgba8(self, memory: &GpuPhysicalMemory) -> Result<Vec<u8>, PicaRenderTargetError> {
        let storage_size = self.tiled_storage_size()?;
        memory.validate_range(self.color_buffer_address, storage_size)?;
        let mut tiled = Vec::new();
        tiled
            .try_reserve_exact(storage_size)
            .map_err(|_| PicaRenderTargetError::AllocationFailed)?;
        tiled.resize(storage_size, 0);
        memory.read(self.color_buffer_address, &mut tiled)?;

        let pixel_count = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(PicaRenderTargetError::SizeOverflow)?;
        let output_len = pixel_count
            .checked_mul(4)
            .ok_or(PicaRenderTargetError::SizeOverflow)?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(output_len)
            .map_err(|_| PicaRenderTargetError::AllocationFailed)?;
        rgba.resize(output_len, 0);

        let tile_edge = self.block_edge();
        let tiles_x = tile_count(self.width as usize, tile_edge)?;
        let bytes_per_pixel = self.format.bytes_per_pixel();
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let source_offset = tiled_pixel_offset(x, y, tiles_x, bytes_per_pixel, tile_edge)?;
                let destination_offset = (y * self.width as usize + x) * 4;
                let pixel = self
                    .format
                    .decode_pixel(&tiled[source_offset..source_offset + bytes_per_pixel]);
                rgba[destination_offset..destination_offset + 4].copy_from_slice(&pixel);
            }
        }
        Ok(rgba)
    }

    fn validate_rasterizer_dimensions(
        self,
        rasterizer: &SoftwareRasterizer,
    ) -> Result<(), PicaRenderTargetError> {
        if rasterizer.width() != self.width as usize || rasterizer.height() != self.height as usize
        {
            return Err(PicaRenderTargetError::RasterizerDimensionsMismatch {
                target_width: self.width,
                target_height: self.height,
                rasterizer_width: rasterizer.width(),
                rasterizer_height: rasterizer.height(),
            });
        }
        Ok(())
    }

    const fn block_edge(self) -> usize {
        match self.block_mode {
            PicaRenderBlockMode::Blocks8x8 => TILE_EDGE_8X8,
            PicaRenderBlockMode::Blocks32x32 => TILE_EDGE_32X32,
        }
    }
}

impl Pica200 {
    /// Decodes the live color render-target configuration.
    pub fn render_target_config(&self) -> Result<PicaRenderTargetConfig, PicaRenderTargetError> {
        let read = |register| {
            self.register(register)
                .ok_or(PicaRenderTargetError::MissingRegister { register })
        };
        PicaRenderTargetConfig::from_registers(
            read(REG_COLORBUFFER_LOC)?,
            read(REG_COLORBUFFER_FORMAT)?,
            read(REG_FRAMEBUFFER_BLOCK32)?,
            read(REG_FRAMEBUFFER_DIM)?,
        )
    }
}

/// Counts the row-major tiles needed to cover a nonzero dimension.
fn tile_count(dimension: usize, tile_edge: usize) -> Result<usize, PicaRenderTargetError> {
    dimension
        .checked_add(tile_edge - 1)
        .map(|rounded| rounded / tile_edge)
        .filter(|count| *count != 0)
        .ok_or(PicaRenderTargetError::SizeOverflow)
}

/// Computes the byte offset for one logical pixel in row-major tile order and
/// Morton order within the selected square tile.
fn tiled_pixel_offset(
    x: usize,
    y: usize,
    tiles_x: usize,
    bytes_per_pixel: usize,
    tile_edge: usize,
) -> Result<usize, PicaRenderTargetError> {
    let tile_x = x / tile_edge;
    let tile_y = y / tile_edge;
    let tile_index = tile_y
        .checked_mul(tiles_x)
        .and_then(|index| index.checked_add(tile_x))
        .ok_or(PicaRenderTargetError::SizeOverflow)?;
    let tile_pixels = tile_edge
        .checked_mul(tile_edge)
        .ok_or(PicaRenderTargetError::SizeOverflow)?;
    let morton = morton_index(x % tile_edge, y % tile_edge, tile_edge);
    tile_index
        .checked_mul(tile_pixels)
        .and_then(|pixel| pixel.checked_add(morton))
        .and_then(|pixel| pixel.checked_mul(bytes_per_pixel))
        .ok_or(PicaRenderTargetError::SizeOverflow)
}

/// Interleaves the X/Y coordinate bits into a Morton index.
fn morton_index(x: usize, y: usize, tile_edge: usize) -> usize {
    let bits = tile_edge.trailing_zeros() as usize;
    let mut index = 0usize;
    for bit in 0..bits {
        index |= ((x >> bit) & 1) << (bit * 2);
        index |= ((y >> bit) & 1) << (bit * 2 + 1);
    }
    index
}

impl PicaColorBufferFormat {
    fn encode_pixel(self, rgba: [u8; 4], output: &mut [u8]) {
        let [red, green, blue, alpha] = rgba;
        match self {
            Self::Rgba8 => output.copy_from_slice(&[alpha, blue, green, red]),
            Self::Rgb5A1 => {
                let value = (reduce(red, 5) << 11)
                    | (reduce(green, 5) << 6)
                    | (reduce(blue, 5) << 1)
                    | u16::from(alpha >= 128);
                output.copy_from_slice(&value.to_le_bytes());
            }
            Self::Rgb565 => {
                let value = (reduce(red, 5) << 11) | (reduce(green, 6) << 5) | reduce(blue, 5);
                output.copy_from_slice(&value.to_le_bytes());
            }
            Self::Rgba4 => {
                let value = (reduce(red, 4) << 12)
                    | (reduce(green, 4) << 8)
                    | (reduce(blue, 4) << 4)
                    | reduce(alpha, 4);
                output.copy_from_slice(&value.to_le_bytes());
            }
        }
    }

    fn decode_pixel(self, input: &[u8]) -> [u8; 4] {
        match self {
            Self::Rgba8 => [input[3], input[2], input[1], input[0]],
            Self::Rgb5A1 => {
                let value = u16::from_le_bytes([input[0], input[1]]);
                [
                    expand_5((value >> 11) & 0x1F),
                    expand_5((value >> 6) & 0x1F),
                    expand_5((value >> 1) & 0x1F),
                    if value & 1 == 0 { 0 } else { 255 },
                ]
            }
            Self::Rgb565 => {
                let value = u16::from_le_bytes([input[0], input[1]]);
                [
                    expand_5((value >> 11) & 0x1F),
                    expand_6((value >> 5) & 0x3F),
                    expand_5(value & 0x1F),
                    255,
                ]
            }
            Self::Rgba4 => {
                let value = u16::from_le_bytes([input[0], input[1]]);
                [
                    expand_4((value >> 12) & 0xF),
                    expand_4((value >> 8) & 0xF),
                    expand_4((value >> 4) & 0xF),
                    expand_4(value & 0xF),
                ]
            }
        }
    }
}

fn reduce(value: u8, bits: u32) -> u16 {
    let max = (1u32 << bits) - 1;
    ((u32::from(value) * max + 127) / 255) as u16
}

fn expand_4(value: u16) -> u8 {
    (value * 17) as u8
}

fn expand_5(value: u16) -> u8 {
    ((u32::from(value) * 255 + 15) / 31) as u8
}

fn expand_6(value: u16) -> u8 {
    ((u32::from(value) * 255 + 31) / 63) as u8
}

/// Summary of one completed render-target write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaRenderTargetWriteReport {
    pub width: u32,
    pub height: u32,
    pub tiles_written: u64,
    pub bytes_written: u64,
}

/// Failures while decoding or writing a PICA200 color render target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaRenderTargetError {
    MissingRegister {
        register: u16,
    },
    InvalidColorLocationRegister(u32),
    InvalidDimensions {
        width: u32,
        height: u32,
    },
    UnsupportedColorFormat {
        pixel_size: u8,
        format: u8,
    },
    UnsupportedBlockMode {
        value: u32,
    },
    RasterizerDimensionsMismatch {
        target_width: u32,
        target_height: u32,
        rasterizer_width: usize,
        rasterizer_height: usize,
    },
    SizeOverflow,
    AllocationFailed,
    PhysicalMemory(GpuPhysicalMemoryError),
}

impl fmt::Display for PicaRenderTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRegister { register } => {
                write!(
                    formatter,
                    "PICA200 render-target register {register:#05x} is unavailable"
                )
            }
            Self::InvalidColorLocationRegister(value) => write!(
                formatter,
                "PICA200 color-buffer location {value:#010x} has reserved high bits set"
            ),
            Self::InvalidDimensions { width, height } => {
                write!(
                    formatter,
                    "PICA200 render target has invalid dimensions {width}x{height}"
                )
            }
            Self::UnsupportedColorFormat { pixel_size, format } => write!(
                formatter,
                "unsupported PICA200 color-buffer format: pixel-size {pixel_size}, format {format}"
            ),
            Self::UnsupportedBlockMode { value } => {
                write!(
                    formatter,
                    "PICA200 {value}x{value} render blocks are not supported"
                )
            }
            Self::RasterizerDimensionsMismatch {
                target_width,
                target_height,
                rasterizer_width,
                rasterizer_height,
            } => write!(
                formatter,
                "rasterizer is {rasterizer_width}x{rasterizer_height}, expected PICA200 target {target_width}x{target_height}"
            ),
            Self::SizeOverflow => formatter.write_str("PICA200 color-buffer size overflows"),
            Self::AllocationFailed => {
                formatter.write_str("could not allocate PICA200 color-buffer staging storage")
            }
            Self::PhysicalMemory(error) => {
                write!(
                    formatter,
                    "PICA200 color-buffer memory access failed: {error}"
                )
            }
        }
    }
}

impl std::error::Error for PicaRenderTargetError {}

impl From<GpuPhysicalMemoryError> for PicaRenderTargetError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::FCRAM_BASE;
    use crate::gpu::{
        ClipDepthRange, DepthFunction, GeometryPipeline, GeometryVertex, Matrix4, Viewport,
    };

    fn target_config(
        address: u32,
        width: u32,
        height: u32,
        format: PicaColorBufferFormat,
        block_mode: PicaRenderBlockMode,
    ) -> PicaRenderTargetConfig {
        let (pixel_size, format_bits) = match format {
            PicaColorBufferFormat::Rgba8 => (2u32, 0u32),
            PicaColorBufferFormat::Rgb5A1 => (0, 2),
            PicaColorBufferFormat::Rgb565 => (0, 3),
            PicaColorBufferFormat::Rgba4 => (0, 4),
        };
        PicaRenderTargetConfig::from_registers(
            address >> 3,
            pixel_size | (format_bits << 16),
            u32::from(block_mode == PicaRenderBlockMode::Blocks32x32),
            ((height - 1) << 12) | width,
        )
        .unwrap()
    }

    fn patterned_rasterizer(width: usize, height: usize) -> SoftwareRasterizer {
        let mut rasterizer = SoftwareRasterizer::new(width, height).unwrap();
        rasterizer.clear([3, 5, 7, 255], 1.0).unwrap();
        let viewport = Viewport::new(0.0, 0.0, width as f32, height as f32).unwrap();
        let pipeline = GeometryPipeline::new(
            Matrix4::identity(),
            viewport,
            ClipDepthRange::NegativeOneToOne,
        );
        pipeline
            .draw_triangle(
                [
                    GeometryVertex::new([-1.0, -1.0, 0.0, 1.0], [240, 10, 20, 255]),
                    GeometryVertex::new([1.0, -1.0, 0.0, 1.0], [10, 240, 30, 255]),
                    GeometryVertex::new([-1.0, 1.0, 0.0, 1.0], [20, 30, 240, 255]),
                ],
                &mut rasterizer,
                DepthFunction::Less,
            )
            .unwrap();
        rasterizer
    }

    #[test]
    fn decodes_live_render_target_registers() {
        let address = FCRAM_BASE + 0x20_000;
        let mut gpu = Pica200::new();
        gpu.write_register(REG_COLORBUFFER_LOC, address >> 3)
            .unwrap();
        gpu.write_register(REG_COLORBUFFER_FORMAT, 2).unwrap();
        gpu.write_register(REG_FRAMEBUFFER_BLOCK32, 0).unwrap();
        gpu.write_register(REG_FRAMEBUFFER_DIM, ((240 - 1) << 12) | 400)
            .unwrap();

        let target = gpu.render_target_config().unwrap();
        assert_eq!(target.color_buffer_address, address);
        assert_eq!((target.width, target.height), (400, 240));
        assert_eq!(target.format, PicaColorBufferFormat::Rgba8);
        assert_eq!(target.block_mode, PicaRenderBlockMode::Blocks8x8);
        assert!(target.vertical_flip_enabled);
    }

    #[test]
    fn color_buffer_format_selector_table_names_all_three_bit_values() {
        let expected = [
            PicaColorBufferFormatSelector::Rgba8,
            PicaColorBufferFormatSelector::Rgb8,
            PicaColorBufferFormatSelector::Rgb5A1,
            PicaColorBufferFormatSelector::Rgb565,
            PicaColorBufferFormatSelector::Rgba4,
            PicaColorBufferFormatSelector::Reserved(5),
            PicaColorBufferFormatSelector::Reserved(6),
            PicaColorBufferFormatSelector::Reserved(7),
        ];

        for (selector, expected_format) in expected.into_iter().enumerate() {
            assert_eq!(
                PicaColorBufferFormatSelector::from_register(selector as u32),
                expected_format
            );
        }

        // RGB8 is a known format name, but its PICA internal pixel-size
        // combination is unresolved and must remain fail-closed.
        for pixel_size in [0u8, 2u8] {
            assert_eq!(
                PicaRenderTargetConfig::from_registers(
                    0,
                    u32::from(pixel_size) | (1 << 16),
                    0,
                    8,
                ),
                Err(PicaRenderTargetError::UnsupportedColorFormat {
                    pixel_size,
                    format: 1,
                })
            );
        }
    }

    #[test]
    fn rejects_zero_width_and_unsupported_format_register_combinations() {
        assert_eq!(
            PicaRenderTargetConfig::from_registers(0, 2, 0, 240 << 12),
            Err(PicaRenderTargetError::InvalidDimensions {
                width: 0,
                height: 241,
            })
        );
        assert_eq!(
            PicaRenderTargetConfig::from_registers(0, 7 << 16, 0, 8),
            Err(PicaRenderTargetError::UnsupportedColorFormat {
                pixel_size: 0,
                format: 7,
            })
        );
    }

    #[test]
    fn morton_index_is_a_bijection_over_eight_by_eight_and_32_by_32_tiles() {
        for tile_edge in [TILE_EDGE_8X8, TILE_EDGE_32X32] {
            let tile_pixels = tile_edge * tile_edge;
            let mut indices = Vec::with_capacity(tile_pixels);
            for y in 0..tile_edge {
                for x in 0..tile_edge {
                    indices.push(morton_index(x, y, tile_edge));
                }
            }
            indices.sort_unstable();
            assert_eq!(indices, (0..tile_pixels).collect::<Vec<_>>());
        }

        assert_eq!(morton_index(1, 0, TILE_EDGE_8X8), 1);
        assert_eq!(morton_index(0, 1, TILE_EDGE_8X8), 2);
        assert_eq!(morton_index(1, 1, TILE_EDGE_8X8), 3);
        assert_eq!(morton_index(2, 0, TILE_EDGE_8X8), 4);
        assert_eq!(morton_index(0, 2, TILE_EDGE_8X8), 8);
        assert_eq!(morton_index(16, 0, TILE_EDGE_32X32), 256);
        assert_eq!(morton_index(0, 16, TILE_EDGE_32X32), 512);
    }

    #[test]
    fn rgba8_rasterizer_round_trips_through_morton_tiles_and_physical_memory() {
        let rasterizer = patterned_rasterizer(16, 8);
        let target = target_config(
            FCRAM_BASE + 0x40_000,
            16,
            8,
            PicaColorBufferFormat::Rgba8,
            PicaRenderBlockMode::Blocks8x8,
        );
        let mut memory = GpuPhysicalMemory::new();
        let report = target.write_rasterizer(&rasterizer, &mut memory).unwrap();

        assert_eq!(report.width, 16);
        assert_eq!(report.height, 8);
        assert_eq!(report.tiles_written, 2);
        assert_eq!(report.bytes_written, 16 * 8 * 4);
        assert_eq!(
            target.read_rgba8(&memory).unwrap(),
            rasterizer.color_buffer()
        );
    }

    #[test]
    fn packed_pica_formats_round_trip_with_only_quantization_error() {
        let rasterizer = patterned_rasterizer(8, 8);
        for format in [
            PicaColorBufferFormat::Rgb5A1,
            PicaColorBufferFormat::Rgb565,
            PicaColorBufferFormat::Rgba4,
        ] {
            let target = target_config(
                FCRAM_BASE + 0x80_000,
                8,
                8,
                format,
                PicaRenderBlockMode::Blocks8x8,
            );
            let mut memory = GpuPhysicalMemory::new();
            target.write_rasterizer(&rasterizer, &mut memory).unwrap();
            let decoded = target.read_rgba8(&memory).unwrap();
            for (source, round_trip) in rasterizer
                .color_buffer()
                .chunks_exact(4)
                .zip(decoded.chunks_exact(4))
            {
                let tolerance = match format {
                    PicaColorBufferFormat::Rgb5A1 => 5,
                    PicaColorBufferFormat::Rgb565 => 5,
                    PicaColorBufferFormat::Rgba4 => 9,
                    PicaColorBufferFormat::Rgba8 => 0,
                };
                for channel in 0..3 {
                    assert!(
                        source[channel].abs_diff(round_trip[channel]) <= tolerance,
                        "{format:?}: {source:?} -> {round_trip:?}"
                    );
                }
                if matches!(
                    format,
                    PicaColorBufferFormat::Rgb5A1 | PicaColorBufferFormat::Rgb565
                ) {
                    assert_eq!(round_trip[3], 255);
                }
            }
        }
    }

    #[test]
    fn rgba8_rasterizer_round_trips_through_32_by_32_morton_tiles() {
        let rasterizer = patterned_rasterizer(64, 32);
        let target = target_config(
            FCRAM_BASE + 0xC0_000,
            64,
            32,
            PicaColorBufferFormat::Rgba8,
            PicaRenderBlockMode::Blocks32x32,
        );
        let mut memory = GpuPhysicalMemory::new();
        let report = target.write_rasterizer(&rasterizer, &mut memory).unwrap();

        assert_eq!(target.tiled_storage_size().unwrap(), 64 * 32 * 4);
        assert_eq!(report.width, 64);
        assert_eq!(report.height, 32);
        assert_eq!(report.tiles_written, 2);
        assert_eq!(report.bytes_written, 64 * 32 * 4);
        assert_eq!(
            target.read_rgba8(&memory).unwrap(),
            rasterizer.color_buffer()
        );
    }

    #[test]
    fn rejects_dimension_mismatch_before_writing_memory() {
        let rasterizer = patterned_rasterizer(8, 8);
        let target = target_config(
            FCRAM_BASE + 0x100_000,
            16,
            8,
            PicaColorBufferFormat::Rgba8,
            PicaRenderBlockMode::Blocks8x8,
        );
        let mut memory = GpuPhysicalMemory::new();
        assert!(matches!(
            target.write_rasterizer(&rasterizer, &mut memory),
            Err(PicaRenderTargetError::RasterizerDimensionsMismatch { .. })
        ));
        let mut bytes = [0xA5; 4];
        memory
            .read(target.color_buffer_address, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0; 4]);
    }

    #[test]
    fn quantize_rounds_to_each_formats_precision_and_alpha() {
        let pixel = [100, 150, 200, 77];
        assert_eq!(PicaColorBufferFormat::Rgba8.quantize(pixel), pixel);
        // 565 has no alpha: it always reads back as opaque.
        let rgb565 = PicaColorBufferFormat::Rgb565.quantize(pixel);
        assert_eq!(rgb565[3], 255);
        assert_ne!(rgb565[..3], pixel[..3]);
        // 5A1 keeps one alpha bit.
        assert_eq!(PicaColorBufferFormat::Rgb5A1.quantize(pixel)[3], 0);
        assert_eq!(
            PicaColorBufferFormat::Rgb5A1.quantize([0, 0, 0, 200])[3],
            255
        );
        // 4-bit channels expand by repetition, so quantizing is idempotent.
        let rgba4 = PicaColorBufferFormat::Rgba4.quantize(pixel);
        assert_eq!(rgba4.map(|channel| channel % 17), [0; 4]);
        for format in [
            PicaColorBufferFormat::Rgb565,
            PicaColorBufferFormat::Rgb5A1,
            PicaColorBufferFormat::Rgba4,
        ] {
            let once = format.quantize(pixel);
            assert_eq!(format.quantize(once), once, "{format:?}");
        }
    }
}
