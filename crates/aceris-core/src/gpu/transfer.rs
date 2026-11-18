//! Basic GSP/PICA200 DisplayTransfer conversion between tiled and linear buffers.
//!
//! The implementation supports same-sized images, 8x8 Morton tiles, vertical
//! flip, and the five documented transfer pixel formats. Scaling, cropping,
//! 32x32 blocks, TextureCopy, and tiled-to-tiled transfers fail explicitly.
//! The full source and destination ranges are validated and staged before the
//! destination is modified.

use std::fmt;

use super::{GpuPhysicalMemory, GpuPhysicalMemoryError};

pub const DISPLAY_TRANSFER_FLIP_VERTICAL: u32 = 1 << 0;
pub const DISPLAY_TRANSFER_LINEAR_TO_TILED: u32 = 1 << 1;
pub const DISPLAY_TRANSFER_CROP: u32 = 1 << 2;
pub const DISPLAY_TRANSFER_TEXTURE_COPY: u32 = 1 << 3;
pub const DISPLAY_TRANSFER_TILED_TO_TILED: u32 = 1 << 5;
pub const DISPLAY_TRANSFER_BLOCKS_32X32: u32 = 1 << 16;
pub const MAX_DISPLAY_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

const TILE_EDGE: usize = 8;
const TILE_PIXELS: usize = TILE_EDGE * TILE_EDGE;
const MIN_OUTPUT_WIDTH: u16 = 64;
const MIN_OUTPUT_HEIGHT: u16 = 16;

/// Pixel format selectors stored in the DisplayTransfer flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DisplayTransferFormat {
    Rgba8 = 0,
    Rgb8 = 1,
    Rgb565 = 2,
    Rgb5A1 = 3,
    Rgba4 = 4,
}

impl DisplayTransferFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
            Self::Rgb565 | Self::Rgb5A1 | Self::Rgba4 => 2,
        }
    }

    fn from_bits(value: u32) -> Result<Self, DisplayTransferError> {
        match value {
            0 => Ok(Self::Rgba8),
            1 => Ok(Self::Rgb8),
            2 => Ok(Self::Rgb565),
            3 => Ok(Self::Rgb5A1),
            4 => Ok(Self::Rgba4),
            other => Err(DisplayTransferError::UnsupportedFormat(other as u8)),
        }
    }

    fn decode(self, bytes: &[u8]) -> [u8; 4] {
        match self {
            Self::Rgba8 => [bytes[3], bytes[2], bytes[1], bytes[0]],
            Self::Rgb8 => [bytes[2], bytes[1], bytes[0], 255],
            Self::Rgb565 => {
                let value = u16::from_le_bytes([bytes[0], bytes[1]]);
                [
                    expand_5((value >> 11) & 31),
                    expand_6((value >> 5) & 63),
                    expand_5(value & 31),
                    255,
                ]
            }
            Self::Rgb5A1 => {
                let value = u16::from_le_bytes([bytes[0], bytes[1]]);
                [
                    expand_5((value >> 11) & 31),
                    expand_5((value >> 6) & 31),
                    expand_5((value >> 1) & 31),
                    if value & 1 == 0 { 0 } else { 255 },
                ]
            }
            Self::Rgba4 => {
                let value = u16::from_le_bytes([bytes[0], bytes[1]]);
                [
                    expand_4((value >> 12) & 15),
                    expand_4((value >> 8) & 15),
                    expand_4((value >> 4) & 15),
                    expand_4(value & 15),
                ]
            }
        }
    }

    fn encode(self, rgba: [u8; 4], output: &mut [u8]) {
        let [r, g, b, a] = rgba;
        match self {
            Self::Rgba8 => output.copy_from_slice(&[a, b, g, r]),
            Self::Rgb8 => output.copy_from_slice(&[b, g, r]),
            Self::Rgb565 => {
                let value = (reduce(r, 5) << 11) | (reduce(g, 6) << 5) | reduce(b, 5);
                output.copy_from_slice(&value.to_le_bytes());
            }
            Self::Rgb5A1 => {
                let value = (reduce(r, 5) << 11)
                    | (reduce(g, 5) << 6)
                    | (reduce(b, 5) << 1)
                    | u16::from(a >= 128);
                output.copy_from_slice(&value.to_le_bytes());
            }
            Self::Rgba4 => {
                let value =
                    (reduce(r, 4) << 12) | (reduce(g, 4) << 8) | (reduce(b, 4) << 4) | reduce(a, 4);
                output.copy_from_slice(&value.to_le_bytes());
            }
        }
    }
}

/// One GSP DisplayTransfer command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayTransferRequest {
    pub input_address: u32,
    pub output_address: u32,
    pub input_width: u16,
    pub input_height: u16,
    pub output_width: u16,
    pub output_height: u16,
    pub flags: u32,
}

/// Arrangement used for one side of the transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayTransferLayout {
    Linear,
    Tiled8x8Morton,
}

/// Summary of a completed software transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayTransferReport {
    pub width: u16,
    pub height: u16,
    pub source_format: DisplayTransferFormat,
    pub destination_format: DisplayTransferFormat,
    pub source_layout: DisplayTransferLayout,
    pub destination_layout: DisplayTransferLayout,
    pub vertical_flip: bool,
    pub pixels_transferred: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayTransferError {
    MisalignedAddress {
        address: u32,
    },
    InvalidDimensions {
        input_width: u16,
        input_height: u16,
        output_width: u16,
        output_height: u16,
    },
    OutputDimensionsTooSmall {
        width: u16,
        height: u16,
    },
    UnsupportedFormat(u8),
    UnsupportedScaling(u8),
    UnsupportedCropMode,
    UnsupportedBlock32,
    UnsupportedTextureCopyMode,
    UnsupportedTiledToTiledMode,
    SizeOverflow,
    TransferTooLarge {
        bytes: usize,
        maximum: usize,
    },
    AllocationFailed,
    PhysicalMemory(GpuPhysicalMemoryError),
}

impl fmt::Display for DisplayTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MisalignedAddress { address } => {
                write!(
                    f,
                    "DisplayTransfer address {address:#010x} is not 8-byte aligned"
                )
            }
            Self::InvalidDimensions {
                input_width,
                input_height,
                output_width,
                output_height,
            } => write!(
                f,
                "DisplayTransfer dimensions {input_width}x{input_height} -> {output_width}x{output_height} are unsupported"
            ),
            Self::OutputDimensionsTooSmall { width, height } => {
                write!(f, "DisplayTransfer tiled-to-linear output {width}x{height} is below 64x16")
            }
            Self::UnsupportedFormat(value) => {
                write!(f, "DisplayTransfer pixel format {value} is unsupported")
            }
            Self::UnsupportedScaling(value) => {
                write!(f, "DisplayTransfer scaling mode {value} is unsupported")
            }
            Self::UnsupportedCropMode => f.write_str(
                "DisplayTransfer output width is smaller than input width but crop mode is disabled",
            ),
            Self::UnsupportedBlock32 => f.write_str("DisplayTransfer 32x32 blocks are unsupported"),
            Self::UnsupportedTextureCopyMode => {
                f.write_str("TextureCopy mode is not a DisplayTransfer")
            }
            Self::UnsupportedTiledToTiledMode => {
                f.write_str("DisplayTransfer linear-to-tiled and tiled-to-tiled flags cannot be combined")
            }
            Self::SizeOverflow => f.write_str("DisplayTransfer buffer size overflows"),
            Self::TransferTooLarge { bytes, maximum } => {
                write!(
                    f,
                    "DisplayTransfer requires {bytes} bytes; limit is {maximum}"
                )
            }
            Self::AllocationFailed => f.write_str("DisplayTransfer staging allocation failed"),
            Self::PhysicalMemory(error) => write!(f, "DisplayTransfer memory failed: {error}"),
        }
    }
}

impl std::error::Error for DisplayTransferError {}

impl From<GpuPhysicalMemoryError> for DisplayTransferError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

/// Returns exact input/output span sizes after validating the transfer modes.
/// Useful to resolve guest virtual addresses before executing the transfer.
pub fn display_transfer_buffer_sizes(
    request: DisplayTransferRequest,
) -> Result<(usize, usize), DisplayTransferError> {
    validate_request(request)?;
    let source_format = DisplayTransferFormat::from_bits((request.flags >> 8) & 7)?;
    let destination_format = DisplayTransferFormat::from_bits((request.flags >> 12) & 7)?;
    let linear_to_tiled = request.flags & DISPLAY_TRANSFER_LINEAR_TO_TILED != 0;
    let tiled_to_tiled = request.flags & DISPLAY_TRANSFER_TILED_TO_TILED != 0;
    let source_layout = if linear_to_tiled {
        DisplayTransferLayout::Linear
    } else {
        DisplayTransferLayout::Tiled8x8Morton
    };
    let destination_layout = if linear_to_tiled || tiled_to_tiled {
        DisplayTransferLayout::Tiled8x8Morton
    } else {
        DisplayTransferLayout::Linear
    };
    let source_bytes = storage_size(
        usize::from(request.input_width),
        usize::from(request.input_height),
        source_format,
        source_layout,
    )?;
    let destination_bytes = storage_size(
        usize::from(request.output_width),
        usize::from(request.output_height),
        destination_format,
        destination_layout,
    )?;
    for bytes in [source_bytes, destination_bytes] {
        if bytes > MAX_DISPLAY_TRANSFER_BYTES {
            return Err(DisplayTransferError::TransferTooLarge {
                bytes,
                maximum: MAX_DISPLAY_TRANSFER_BYTES,
            });
        }
    }
    Ok((source_bytes, destination_bytes))
}

/// Executes one validated transfer. The destination is written only after the
/// entire input is read and the converted output has been prepared.
pub fn execute_display_transfer(
    memory: &mut GpuPhysicalMemory,
    request: DisplayTransferRequest,
) -> Result<DisplayTransferReport, DisplayTransferError> {
    let (source_bytes, destination_bytes) = display_transfer_buffer_sizes(request)?;
    if request.input_address & 7 != 0 {
        return Err(DisplayTransferError::MisalignedAddress {
            address: request.input_address,
        });
    }
    if request.output_address & 7 != 0 {
        return Err(DisplayTransferError::MisalignedAddress {
            address: request.output_address,
        });
    }
    memory.validate_range(request.input_address, source_bytes)?;
    memory.validate_range(request.output_address, destination_bytes)?;

    let source_format = DisplayTransferFormat::from_bits((request.flags >> 8) & 7)?;
    let destination_format = DisplayTransferFormat::from_bits((request.flags >> 12) & 7)?;
    let linear_to_tiled = request.flags & DISPLAY_TRANSFER_LINEAR_TO_TILED != 0;
    let tiled_to_tiled = request.flags & DISPLAY_TRANSFER_TILED_TO_TILED != 0;
    let source_layout = if linear_to_tiled {
        DisplayTransferLayout::Linear
    } else {
        DisplayTransferLayout::Tiled8x8Morton
    };
    let destination_layout = if linear_to_tiled || tiled_to_tiled {
        DisplayTransferLayout::Tiled8x8Morton
    } else {
        DisplayTransferLayout::Linear
    };
    let vertical_flip = request.flags & DISPLAY_TRANSFER_FLIP_VERTICAL != 0;
    let input_width = usize::from(request.input_width);
    let input_height = usize::from(request.input_height);
    let output_width = usize::from(request.output_width);
    let output_height = usize::from(request.output_height);
    let source_tiles_x = tile_count(input_width)?;
    let destination_tiles_x = tile_count(output_width)?;

    let mut source = Vec::new();
    source
        .try_reserve_exact(source_bytes)
        .map_err(|_| DisplayTransferError::AllocationFailed)?;
    source.resize(source_bytes, 0);
    memory.read(request.input_address, &mut source)?;

    let mut output = Vec::new();
    output
        .try_reserve_exact(destination_bytes)
        .map_err(|_| DisplayTransferError::AllocationFailed)?;
    output.resize(destination_bytes, 0);

    let source_bpp = source_format.bytes_per_pixel();
    let destination_bpp = destination_format.bytes_per_pixel();
    for y in 0..output_height {
        let source_y = if vertical_flip {
            input_height - 1 - y
        } else {
            y
        };
        for x in 0..output_width {
            let src = pixel_offset(
                x,
                source_y,
                input_width,
                source_tiles_x,
                source_bpp,
                source_layout,
            )?;
            let dst = pixel_offset(
                x,
                y,
                output_width,
                destination_tiles_x,
                destination_bpp,
                destination_layout,
            )?;
            let rgba = source_format.decode(&source[src..src + source_bpp]);
            destination_format.encode(rgba, &mut output[dst..dst + destination_bpp]);
        }
    }

    memory.write(request.output_address, &output)?;
    Ok(DisplayTransferReport {
        width: request.output_width,
        height: request.output_height,
        source_format,
        destination_format,
        source_layout,
        destination_layout,
        vertical_flip,
        pixels_transferred: (output_width * output_height) as u64,
        bytes_read: source_bytes as u64,
        bytes_written: destination_bytes as u64,
    })
}

fn validate_request(request: DisplayTransferRequest) -> Result<(), DisplayTransferError> {
    if request.input_width == 0
        || request.input_height == 0
        || request.output_width == 0
        || request.output_height == 0
        || request.output_width > request.input_width
        || request.output_height > request.input_height
    {
        return Err(DisplayTransferError::InvalidDimensions {
            input_width: request.input_width,
            input_height: request.input_height,
            output_width: request.output_width,
            output_height: request.output_height,
        });
    }

    let linear_to_tiled = request.flags & DISPLAY_TRANSFER_LINEAR_TO_TILED != 0;
    let tiled_to_tiled = request.flags & DISPLAY_TRANSFER_TILED_TO_TILED != 0;
    if linear_to_tiled && tiled_to_tiled {
        return Err(DisplayTransferError::UnsupportedTiledToTiledMode);
    }
    if request.flags & DISPLAY_TRANSFER_TEXTURE_COPY != 0 {
        return Err(DisplayTransferError::UnsupportedTextureCopyMode);
    }
    if request.output_width < request.input_width
        && request.flags & DISPLAY_TRANSFER_CROP == 0
    {
        return Err(DisplayTransferError::UnsupportedCropMode);
    }
    if request.flags & DISPLAY_TRANSFER_BLOCKS_32X32 != 0 {
        return Err(DisplayTransferError::UnsupportedBlock32);
    }
    let scale = (request.flags >> 24) & 3;
    if scale != 0 {
        return Err(DisplayTransferError::UnsupportedScaling(scale as u8));
    }

    // The documented 64x16 minimum applies to the common tiled-to-linear
    // path. Tiled-to-tiled and linear-to-tiled transfers use their own
    // address layouts and are not constrained by this output rule.
    if !linear_to_tiled
        && !tiled_to_tiled
        && (request.output_width < MIN_OUTPUT_WIDTH
            || request.output_height < MIN_OUTPUT_HEIGHT)
    {
        return Err(DisplayTransferError::OutputDimensionsTooSmall {
            width: request.output_width,
            height: request.output_height,
        });
    }
    Ok(())
}

fn storage_size(
    width: usize,
    height: usize,
    format: DisplayTransferFormat,
    layout: DisplayTransferLayout,
) -> Result<usize, DisplayTransferError> {
    let pixels = match layout {
        DisplayTransferLayout::Linear => width
            .checked_mul(height)
            .ok_or(DisplayTransferError::SizeOverflow)?,
        DisplayTransferLayout::Tiled8x8Morton => tile_count(width)?
            .checked_mul(tile_count(height)?)
            .and_then(|tiles| tiles.checked_mul(TILE_PIXELS))
            .ok_or(DisplayTransferError::SizeOverflow)?,
    };
    pixels
        .checked_mul(format.bytes_per_pixel())
        .ok_or(DisplayTransferError::SizeOverflow)
}

fn pixel_offset(
    x: usize,
    y: usize,
    width: usize,
    tiles_x: usize,
    bytes_per_pixel: usize,
    layout: DisplayTransferLayout,
) -> Result<usize, DisplayTransferError> {
    match layout {
        DisplayTransferLayout::Linear => y
            .checked_mul(width)
            .and_then(|index| index.checked_add(x))
            .and_then(|index| index.checked_mul(bytes_per_pixel))
            .ok_or(DisplayTransferError::SizeOverflow),
        DisplayTransferLayout::Tiled8x8Morton => {
            let tile = (y / TILE_EDGE)
                .checked_mul(tiles_x)
                .and_then(|index| index.checked_add(x / TILE_EDGE))
                .ok_or(DisplayTransferError::SizeOverflow)?;
            tile.checked_mul(TILE_PIXELS)
                .and_then(|pixel| pixel.checked_add(morton_index(x & 7, y & 7)))
                .and_then(|pixel| pixel.checked_mul(bytes_per_pixel))
                .ok_or(DisplayTransferError::SizeOverflow)
        }
    }
}

fn tile_count(dimension: usize) -> Result<usize, DisplayTransferError> {
    dimension
        .checked_add(TILE_EDGE - 1)
        .map(|rounded| rounded / TILE_EDGE)
        .filter(|count| *count != 0)
        .ok_or(DisplayTransferError::SizeOverflow)
}

fn morton_index(x: usize, y: usize) -> usize {
    let mut index = 0usize;
    for bit in 0..3 {
        index |= ((x >> bit) & 1) << (bit * 2);
        index |= ((y >> bit) & 1) << (bit * 2 + 1);
    }
    index
}

fn reduce(value: u8, bits: u32) -> u16 {
    let maximum = (1u32 << bits) - 1;
    ((u32::from(value) * maximum + 127) / 255) as u16
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::FCRAM_BASE;

    const WIDTH: u16 = 64;
    const HEIGHT: u16 = 64;
    const SOURCE: u32 = FCRAM_BASE + 0x20_000;
    const DESTINATION: u32 = FCRAM_BASE + 0x40_000;

    fn patterned_linear() -> Vec<u8> {
        let mut bytes = vec![0; usize::from(WIDTH) * usize::from(HEIGHT) * 4];
        for y in 0..usize::from(HEIGHT) {
            for x in 0..usize::from(WIDTH) {
                let offset = (y * usize::from(WIDTH) + x) * 4;
                bytes[offset..offset + 4].copy_from_slice(&[x as u8, y as u8, (x ^ y) as u8, 255]);
            }
        }
        bytes
    }

    fn request(input: u32, output: u32, flags: u32) -> DisplayTransferRequest {
        DisplayTransferRequest {
            input_address: input,
            output_address: output,
            input_width: WIDTH,
            input_height: HEIGHT,
            output_width: WIDTH,
            output_height: HEIGHT,
            flags,
        }
    }

    fn make_tiled(memory: &mut GpuPhysicalMemory) -> Vec<u8> {
        let linear = patterned_linear();
        memory.write(SOURCE, &linear).unwrap();
        execute_display_transfer(
            memory,
            request(SOURCE, DESTINATION, DISPLAY_TRANSFER_LINEAR_TO_TILED),
        )
        .unwrap();
        linear
    }

    #[test]
    fn tiled_to_linear_round_trips_the_morton_grid() {
        let mut memory = GpuPhysicalMemory::new();
        let expected = make_tiled(&mut memory);
        let report =
            execute_display_transfer(&mut memory, request(DESTINATION, SOURCE, 0)).unwrap();
        let mut actual = vec![0; expected.len()];
        memory.read(SOURCE, &mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(report.source_layout, DisplayTransferLayout::Tiled8x8Morton);
        assert_eq!(report.destination_layout, DisplayTransferLayout::Linear);
        assert_eq!(report.pixels_transferred, 4096);
    }

    #[test]
    fn vertical_flip_reverses_rows_only() {
        let mut memory = GpuPhysicalMemory::new();
        let expected = make_tiled(&mut memory);
        execute_display_transfer(
            &mut memory,
            request(DESTINATION, SOURCE, DISPLAY_TRANSFER_FLIP_VERTICAL),
        )
        .unwrap();
        let mut actual = vec![0; expected.len()];
        memory.read(SOURCE, &mut actual).unwrap();
        for y in 0..usize::from(HEIGHT) {
            let src = (usize::from(HEIGHT) - 1 - y) * usize::from(WIDTH) * 4;
            let dst = y * usize::from(WIDTH) * 4;
            assert_eq!(
                &actual[dst..dst + usize::from(WIDTH) * 4],
                &expected[src..src + usize::from(WIDTH) * 4]
            );
        }
    }

    #[test]
    fn transfer_converts_to_all_five_output_formats() {
        let mut memory = GpuPhysicalMemory::new();
        let _ = make_tiled(&mut memory);
        for format in 0..=4u32 {
            let request = request(
                DESTINATION,
                SOURCE,
                (format << 12) | DISPLAY_TRANSFER_LINEAR_TO_TILED,
            );
            // The input layout is linear for bit 1, so seed that path directly.
            let expected = patterned_linear();
            memory.write(DESTINATION, &expected).unwrap();
            let report = execute_display_transfer(&mut memory, request).unwrap();
            assert_eq!(report.destination_format as u8, format as u8);
        }
    }

    #[test]
    fn unsupported_modes_leave_destination_untouched() {
        let mut memory = GpuPhysicalMemory::new();
        let sentinel = [0xA5; 16];
        memory.write(DESTINATION, &sentinel).unwrap();
        for flags in [
            DISPLAY_TRANSFER_BLOCKS_32X32,
            DISPLAY_TRANSFER_TEXTURE_COPY,
            1 << 24,
            DISPLAY_TRANSFER_LINEAR_TO_TILED | DISPLAY_TRANSFER_TILED_TO_TILED,
        ] {
            assert!(
                execute_display_transfer(&mut memory, request(SOURCE, DESTINATION, flags)).is_err()
            );
            let mut actual = [0; 16];
            memory.read(DESTINATION, &mut actual).unwrap();
            assert_eq!(actual, sentinel);
        }
    }

    #[test]
    fn tiled_to_tiled_transfer_preserves_the_tiled_pixel_order() {
        let mut memory = GpuPhysicalMemory::new();
        let _ = make_tiled(&mut memory);
        let byte_count = storage_size(
            usize::from(WIDTH),
            usize::from(HEIGHT),
            DisplayTransferFormat::Rgba8,
            DisplayTransferLayout::Tiled8x8Morton,
        )
        .unwrap();
        let mut expected = vec![0; byte_count];
        memory.read(DESTINATION, &mut expected).unwrap();

        let report = execute_display_transfer(
            &mut memory,
            request(
                DESTINATION,
                SOURCE,
                DISPLAY_TRANSFER_TILED_TO_TILED,
            ),
        )
        .unwrap();

        let mut actual = vec![0; byte_count];
        memory.read(SOURCE, &mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(report.source_layout, DisplayTransferLayout::Tiled8x8Morton);
        assert_eq!(report.destination_layout, DisplayTransferLayout::Tiled8x8Morton);
        assert_eq!(report.pixels_transferred, 4096);
    }

    #[test]
    fn tiled_to_linear_crop_copies_the_requested_top_left_rectangle() {
        let mut memory = GpuPhysicalMemory::new();
        let expected = make_tiled(&mut memory);
        let mut cropped = request(DESTINATION, SOURCE, DISPLAY_TRANSFER_CROP);
        cropped.output_width = 48;
        cropped.output_height = 32;

        let report = execute_display_transfer(&mut memory, cropped).unwrap();
        let mut actual = vec![0; 48 * 32 * 4];
        memory.read(SOURCE, &mut actual).unwrap();

        for y in 0..32usize {
            let expected_start = y * usize::from(WIDTH) * 4;
            let actual_start = y * 48 * 4;
            assert_eq!(
                &actual[actual_start..actual_start + 48 * 4],
                &expected[expected_start..expected_start + 48 * 4],
            );
        }
        assert_eq!(report.width, 48);
        assert_eq!(report.height, 32);
        assert_eq!(report.pixels_transferred, 48 * 32);
        assert_eq!(report.bytes_read, 64 * 64 * 4);
        assert_eq!(report.bytes_written, 48 * 32 * 4);
    }

    #[test]
    fn horizontal_crop_requires_the_guest_crop_flag() {
        let mut memory = GpuPhysicalMemory::new();
        let _ = make_tiled(&mut memory);
        let mut cropped = request(DESTINATION, SOURCE, 0);
        cropped.output_width = 48;
        cropped.output_height = 32;

        let sentinel = [0xA5; 32];
        memory.write(SOURCE, &sentinel).unwrap();
        assert_eq!(
            execute_display_transfer(&mut memory, cropped),
            Err(DisplayTransferError::UnsupportedCropMode)
        );
        let mut actual = [0; 32];
        memory.read(SOURCE, &mut actual).unwrap();
        assert_eq!(actual, sentinel);
    }

    #[test]
    fn invalid_source_range_leaves_destination_untouched() {
        let mut memory = GpuPhysicalMemory::new();
        let sentinel = [0x5A; 16];
        memory.write(DESTINATION, &sentinel).unwrap();
        let mut bad = request(SOURCE, DESTINATION, 0);
        bad.input_address = 0x1400_0000;
        assert!(matches!(
            execute_display_transfer(&mut memory, bad),
            Err(DisplayTransferError::PhysicalMemory(_))
        ));
        let mut actual = [0; 16];
        memory.read(DESTINATION, &mut actual).unwrap();
        assert_eq!(actual, sentinel);
    }

    #[test]
    fn rejects_tiled_to_linear_output_below_documented_dimension_minimum() {
        let mut bad = request(SOURCE, DESTINATION, 0);
        bad.output_width = 48;
        assert_eq!(
            display_transfer_buffer_sizes(bad),
            Err(DisplayTransferError::OutputDimensionsTooSmall {
                width: 48,
                height: 64,
            })
        );
    }
}
