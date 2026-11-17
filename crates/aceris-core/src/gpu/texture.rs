//! PICA200 2D texture-unit decoding and software sampling.
//!
//! Texture texels are stored in 8x8 tiles, with Morton/Z-order inside each
//! tile. This module implements the documented uncompressed formats and the
//! nearest/bilinear sampling modes. ETC1/ETC1A4, HILO8, packed 4-bit formats,
//! cube maps, projection/shadow textures and mip-level selection remain
//! explicit unsupported cases; they are never silently decoded as another
//! format.
//!
//! The sampler is deliberately independent of rasterization. The fragment
//! stage can consume its RGBA output once interpolated texture coordinates are
//! available in the fragment pipeline.

use super::physical_memory::{GpuPhysicalMemory, GpuPhysicalMemoryError};
use super::pica200::Pica200;

/// PICA register selecting enabled texture units and shared texture routing.
pub const REG_TEXUNIT_CONFIG: u16 = 0x080;
/// Number of ordinary sampled 2D texture units.
pub const PICA_TEXTURE_UNIT_COUNT: usize = 3;
const TILE_EDGE: usize = 8;
const TILE_PIXELS: usize = TILE_EDGE * TILE_EDGE;

/// Register addresses for border color, dimensions, parameters, base address
/// and pixel format, respectively, for texture units 0, 1 and 2.
const UNIT_REGISTERS: [[u16; 5]; PICA_TEXTURE_UNIT_COUNT] = [
    [0x081, 0x082, 0x083, 0x085, 0x08E],
    [0x091, 0x092, 0x093, 0x095, 0x096],
    [0x099, 0x09A, 0x09B, 0x09D, 0x09E],
];

/// Uncompressed texture formats implemented by the software sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTextureFormat {
    Rgba8,
    Rgb8,
    Rgba5551,
    Rgb565,
    Rgba4444,
    IntensityAlpha8,
    Intensity8,
    Alpha8,
    IntensityAlpha44,
}

impl PicaTextureFormat {
    fn decode(unit: usize, value: u32) -> Result<Self, PicaTextureError> {
        match value & 0xF {
            0x0 => Ok(Self::Rgba8),
            0x1 => Ok(Self::Rgb8),
            0x2 => Ok(Self::Rgba5551),
            0x3 => Ok(Self::Rgb565),
            0x4 => Ok(Self::Rgba4444),
            0x5 => Ok(Self::IntensityAlpha8),
            0x7 => Ok(Self::Intensity8),
            0x8 => Ok(Self::Alpha8),
            0x9 => Ok(Self::IntensityAlpha44),
            other => Err(PicaTextureError::UnsupportedFormat {
                unit,
                format: other as u8,
            }),
        }
    }

    const fn bytes_per_texel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
            Self::Rgba5551 | Self::Rgb565 | Self::Rgba4444 | Self::IntensityAlpha8 => 2,
            Self::Intensity8 | Self::Alpha8 | Self::IntensityAlpha44 => 1,
        }
    }

    fn decode_texel(self, data: &[u8]) -> [u8; 4] {
        match self {
            // PICA RGBA8 is stored in little-endian ABGR byte order.
            Self::Rgba8 => [data[3], data[2], data[1], data[0]],
            Self::Rgb8 => [data[0], data[1], data[2], 255],
            Self::Rgba5551 => {
                let packed = u16::from_le_bytes([data[0], data[1]]);
                [
                    expand_5((packed >> 11) & 0x1F),
                    expand_5((packed >> 6) & 0x1F),
                    expand_5((packed >> 1) & 0x1F),
                    if packed & 1 != 0 { 255 } else { 0 },
                ]
            }
            Self::Rgb565 => {
                let packed = u16::from_le_bytes([data[0], data[1]]);
                [
                    expand_5((packed >> 11) & 0x1F),
                    expand_6((packed >> 5) & 0x3F),
                    expand_5(packed & 0x1F),
                    255,
                ]
            }
            Self::Rgba4444 => {
                let packed = u16::from_le_bytes([data[0], data[1]]);
                [
                    expand_4((packed >> 12) & 0xF),
                    expand_4((packed >> 8) & 0xF),
                    expand_4((packed >> 4) & 0xF),
                    expand_4(packed & 0xF),
                ]
            }
            Self::IntensityAlpha8 => [data[0], data[0], data[0], data[1]],
            Self::Intensity8 => [data[0], data[0], data[0], 255],
            Self::Alpha8 => [255, 255, 255, data[0]],
            Self::IntensityAlpha44 => {
                let intensity = (data[0] >> 4) * 17;
                let alpha = (data[0] & 0xF) * 17;
                [intensity, intensity, intensity, alpha]
            }
        }
    }
}

const fn expand_4(value: u16) -> u8 {
    ((value << 4) | value) as u8
}

const fn expand_5(value: u16) -> u8 {
    ((value << 3) | (value >> 2)) as u8
}

const fn expand_6(value: u16) -> u8 {
    ((value << 2) | (value >> 4)) as u8
}

/// Texture-coordinate wrapping mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTextureWrap {
    ClampToEdge,
    ClampToBorder,
    Repeat,
    MirroredRepeat,
}

impl PicaTextureWrap {
    fn decode(unit: usize, axis: PicaTextureAxis, value: u32) -> Result<Self, PicaTextureError> {
        match value {
            0 => Ok(Self::ClampToEdge),
            1 => Ok(Self::ClampToBorder),
            2 => Ok(Self::Repeat),
            3 => Ok(Self::MirroredRepeat),
            other => Err(PicaTextureError::UnsupportedWrap {
                unit,
                axis,
                mode: other as u8,
            }),
        }
    }
}

/// Texture-coordinate axis used in configuration errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTextureAxis {
    S,
    T,
}

/// Texture filtering mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTextureFilter {
    Nearest,
    Linear,
}

impl PicaTextureFilter {
    fn decode(unit: usize, minifying: bool, value: u32) -> Result<Self, PicaTextureError> {
        match value {
            0 => Ok(Self::Nearest),
            1 => Ok(Self::Linear),
            other => Err(PicaTextureError::UnsupportedFilter {
                unit,
                minifying,
                value: other as u8,
            }),
        }
    }
}

/// Decoded PICA200 texture-unit state for a 2D texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaTextureUnit {
    pub unit: usize,
    pub enabled: bool,
    pub address: u32,
    pub width: u16,
    pub height: u16,
    pub format: PicaTextureFormat,
    pub wrap_s: PicaTextureWrap,
    pub wrap_t: PicaTextureWrap,
    pub magnification_filter: PicaTextureFilter,
    pub minification_filter: PicaTextureFilter,
    pub mipmap_filter_linear: bool,
    pub border_color: [u8; 4],
}

impl PicaTextureUnit {
    /// Decodes one of the three guest-configured 2D texture units.
    ///
    /// The address register stores the physical base in 8-byte units. Width
    /// and height are taken directly from the documented 11-bit fields.
    pub fn from_pica(pica: &Pica200, unit: usize) -> Result<Self, PicaTextureError> {
        let registers = UNIT_REGISTERS
            .get(unit)
            .ok_or(PicaTextureError::InvalidUnit { unit })?;
        let config = pica.register(REG_TEXUNIT_CONFIG).unwrap_or(0);
        let enabled = config & (1 << unit) != 0;
        let border = pica.register(registers[0]).unwrap_or(0);
        let border_color = [
            border as u8,
            (border >> 8) as u8,
            (border >> 16) as u8,
            (border >> 24) as u8,
        ];
        if !enabled {
            // Disabled units may legitimately have zero dimensions and an
            // uninitialized format register. Preserve their disabled state
            // without interpreting irrelevant register contents.
            return Ok(Self {
                unit,
                enabled: false,
                address: 0,
                width: 0,
                height: 0,
                format: PicaTextureFormat::Rgba8,
                wrap_s: PicaTextureWrap::ClampToEdge,
                wrap_t: PicaTextureWrap::ClampToEdge,
                magnification_filter: PicaTextureFilter::Nearest,
                minification_filter: PicaTextureFilter::Nearest,
                mipmap_filter_linear: false,
                border_color,
            });
        }

        let dimensions = pica.register(registers[1]).unwrap_or(0);
        let parameters = pica.register(registers[2]).unwrap_or(0);
        let address_register = pica.register(registers[3]).unwrap_or(0);
        let format_register = pica.register(registers[4]).unwrap_or(0);

        let texture_type = (parameters >> 28) & 0x7;
        if unit == 0 && texture_type != 0 {
            return Err(PicaTextureError::UnsupportedType {
                unit,
                texture_type: texture_type as u8,
            });
        }

        let width = ((dimensions >> 16) & 0x7FF) as u16;
        let height = (dimensions & 0x7FF) as u16;
        if width == 0 || height == 0 {
            return Err(PicaTextureError::InvalidDimensions {
                unit,
                width,
                height,
            });
        }

        Ok(Self {
            unit,
            enabled,
            address: (address_register & 0x0FFF_FFFF) << 3,
            width,
            height,
            format: PicaTextureFormat::decode(unit, format_register)?,
            wrap_t: PicaTextureWrap::decode(unit, PicaTextureAxis::T, (parameters >> 8) & 0x7)?,
            wrap_s: PicaTextureWrap::decode(unit, PicaTextureAxis::S, (parameters >> 12) & 0x7)?,
            magnification_filter: PicaTextureFilter::decode(unit, false, (parameters >> 1) & 1)?,
            minification_filter: PicaTextureFilter::decode(unit, true, (parameters >> 2) & 1)?,
            mipmap_filter_linear: parameters & (1 << 24) != 0,
            border_color,
        })
    }

    /// Samples normalized texture coordinates using the selected min/mag filter.
    ///
    /// This implementation samples mip level zero. LOD calculation and
    /// mip-level selection are kept explicit as future work.
    pub fn sample(
        &self,
        memory: &GpuPhysicalMemory,
        s: f32,
        t: f32,
        minifying: bool,
    ) -> Result<[f32; 4], PicaTextureError> {
        if !self.enabled {
            return Err(PicaTextureError::Disabled { unit: self.unit });
        }
        if !s.is_finite() || !t.is_finite() {
            return Err(PicaTextureError::InvalidCoordinates { s, t });
        }

        let filter = if minifying {
            self.minification_filter
        } else {
            self.magnification_filter
        };
        let rgba = match filter {
            PicaTextureFilter::Nearest => self.sample_nearest(memory, s, t)?,
            PicaTextureFilter::Linear => self.sample_linear(memory, s, t)?,
        };
        Ok(rgba.map(|channel| f32::from(channel) / 255.0))
    }

    fn sample_nearest(
        &self,
        memory: &GpuPhysicalMemory,
        s: f32,
        t: f32,
    ) -> Result<[u8; 4], PicaTextureError> {
        let Some(s) = wrap_coordinate(s, self.wrap_s) else {
            return Ok(self.border_color);
        };
        let Some(t) = wrap_coordinate(t, self.wrap_t) else {
            return Ok(self.border_color);
        };

        let mut x = (s * f32::from(self.width)).floor() as i64;
        let mut y = (t * f32::from(self.height)).floor() as i64;
        if s == 1.0 {
            x = i64::from(self.width) - 1;
        }
        if t == 1.0 {
            y = i64::from(self.height) - 1;
        }
        self.read_wrapped_texel(memory, x, y)
    }

    fn sample_linear(
        &self,
        memory: &GpuPhysicalMemory,
        s: f32,
        t: f32,
    ) -> Result<[u8; 4], PicaTextureError> {
        let Some(s) = wrap_coordinate(s, self.wrap_s) else {
            return Ok(self.border_color);
        };
        let Some(t) = wrap_coordinate(t, self.wrap_t) else {
            return Ok(self.border_color);
        };

        let x = f64::from(s) * f64::from(self.width) - 0.5;
        let y = f64::from(t) * f64::from(self.height) - 0.5;
        if !x.is_finite() || !y.is_finite() {
            return Err(PicaTextureError::InvalidCoordinates { s, t });
        }
        let x_floor = x.floor();
        let y_floor = y.floor();
        let x0 = x_floor as i64;
        let y0 = y_floor as i64;
        let x1 = x0.saturating_add(1);
        let y1 = y0.saturating_add(1);
        let amount_x = (x - x_floor) as f32;
        let amount_y = (y - y_floor) as f32;

        let c00 = self.read_wrapped_texel(memory, x0, y0)?;
        let c10 = self.read_wrapped_texel(memory, x1, y0)?;
        let c01 = self.read_wrapped_texel(memory, x0, y1)?;
        let c11 = self.read_wrapped_texel(memory, x1, y1)?;

        let mut result = [0; 4];
        for channel in 0..4 {
            let top = f32::from(c00[channel])
                + (f32::from(c10[channel]) - f32::from(c00[channel])) * amount_x;
            let bottom = f32::from(c01[channel])
                + (f32::from(c11[channel]) - f32::from(c01[channel])) * amount_x;
            result[channel] = (top + (bottom - top) * amount_y).round().clamp(0.0, 255.0) as u8;
        }
        Ok(result)
    }

    fn read_wrapped_texel(
        &self,
        memory: &GpuPhysicalMemory,
        x: i64,
        y: i64,
    ) -> Result<[u8; 4], PicaTextureError> {
        let Some(x) = wrap_index(x, usize::from(self.width), self.wrap_s) else {
            return Ok(self.border_color);
        };
        let Some(y) = wrap_index(y, usize::from(self.height), self.wrap_t) else {
            return Ok(self.border_color);
        };

        let texels_per_row = usize::from(self.width).div_ceil(TILE_EDGE);
        let tile_x = x / TILE_EDGE;
        let tile_y = y / TILE_EDGE;
        let tile_index = tile_y
            .checked_mul(texels_per_row)
            .and_then(|value| value.checked_add(tile_x))
            .ok_or(PicaTextureError::AddressOverflow)?;
        let morton = morton_index(x & 7, y & 7);
        let texel_index = tile_index
            .checked_mul(TILE_PIXELS)
            .and_then(|value| value.checked_add(morton))
            .ok_or(PicaTextureError::AddressOverflow)?;
        let byte_offset = texel_index
            .checked_mul(self.format.bytes_per_texel())
            .ok_or(PicaTextureError::AddressOverflow)?;
        let address = u64::from(self.address)
            .checked_add(byte_offset as u64)
            .filter(|&value| value <= u64::from(u32::MAX))
            .ok_or(PicaTextureError::AddressOverflow)? as u32;

        let mut bytes = [0u8; 4];
        let byte_count = self.format.bytes_per_texel();
        memory
            .read(address, &mut bytes[..byte_count])
            .map_err(PicaTextureError::Memory)?;
        Ok(self.format.decode_texel(&bytes[..byte_count]))
    }
}

fn wrap_coordinate(value: f32, mode: PicaTextureWrap) -> Option<f32> {
    match mode {
        PicaTextureWrap::ClampToEdge => Some(value.clamp(0.0, 1.0)),
        // Keep the coordinate outside [0, 1] so the individual bilinear
        // taps can blend border color with edge texels. Nearest sampling still
        // resolves to border through wrap_index.
        PicaTextureWrap::ClampToBorder => Some(value),
        PicaTextureWrap::Repeat => Some(value.rem_euclid(1.0)),
        PicaTextureWrap::MirroredRepeat => {
            let mirrored = value.rem_euclid(2.0);
            Some(if mirrored > 1.0 {
                2.0 - mirrored
            } else {
                mirrored
            })
        }
    }
}

fn wrap_index(value: i64, extent: usize, mode: PicaTextureWrap) -> Option<usize> {
    let extent = i64::try_from(extent).ok()?;
    match mode {
        PicaTextureWrap::ClampToEdge => Some(value.clamp(0, extent - 1) as usize),
        PicaTextureWrap::ClampToBorder => {
            if value < 0 || value >= extent {
                None
            } else {
                Some(value as usize)
            }
        }
        PicaTextureWrap::Repeat => Some(value.rem_euclid(extent) as usize),
        PicaTextureWrap::MirroredRepeat => {
            let period = extent.checked_mul(2)?;
            let position = value.rem_euclid(period);
            Some(if position >= extent {
                (period - position - 1) as usize
            } else {
                position as usize
            })
        }
    }
}

fn morton_index(x: usize, y: usize) -> usize {
    let mut index = 0usize;
    for bit in 0..3 {
        index |= ((x >> bit) & 1) << (bit * 2);
        index |= ((y >> bit) & 1) << (bit * 2 + 1);
    }
    index
}

/// Errors from texture configuration, memory reads or sampling.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaTextureError {
    InvalidUnit {
        unit: usize,
    },
    Disabled {
        unit: usize,
    },
    UnsupportedType {
        unit: usize,
        texture_type: u8,
    },
    UnsupportedFormat {
        unit: usize,
        format: u8,
    },
    UnsupportedWrap {
        unit: usize,
        axis: PicaTextureAxis,
        mode: u8,
    },
    UnsupportedFilter {
        unit: usize,
        minifying: bool,
        value: u8,
    },
    InvalidDimensions {
        unit: usize,
        width: u16,
        height: u16,
    },
    InvalidCoordinates {
        s: f32,
        t: f32,
    },
    AddressOverflow,
    Memory(GpuPhysicalMemoryError),
}

impl std::fmt::Display for PicaTextureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUnit { unit } => write!(formatter, "PICA texture unit {unit} is invalid"),
            Self::Disabled { unit } => write!(formatter, "PICA texture unit {unit} is disabled"),
            Self::UnsupportedType { unit, texture_type } => write!(
                formatter,
                "PICA texture unit {unit} uses unsupported texture type {texture_type}"
            ),
            Self::UnsupportedFormat { unit, format } => write!(
                formatter,
                "PICA texture unit {unit} uses unsupported format {format:#x}"
            ),
            Self::UnsupportedWrap { unit, axis, mode } => write!(
                formatter,
                "PICA texture unit {unit} uses unsupported {axis:?} wrap mode {mode}"
            ),
            Self::UnsupportedFilter {
                unit,
                minifying,
                value,
            } => write!(
                formatter,
                "PICA texture unit {unit} uses unsupported {} filter {value}",
                if *minifying {
                    "minification"
                } else {
                    "magnification"
                }
            ),
            Self::InvalidDimensions {
                unit,
                width,
                height,
            } => write!(
                formatter,
                "PICA texture unit {unit} has invalid dimensions {width}x{height}"
            ),
            Self::InvalidCoordinates { s, t } => {
                write!(
                    formatter,
                    "PICA texture coordinates ({s}, {t}) are not finite"
                )
            }
            Self::AddressOverflow => {
                formatter.write_str("PICA texture address calculation overflowed")
            }
            Self::Memory(error) => write!(formatter, "PICA texture memory read failed: {error}"),
        }
    }
}

impl std::error::Error for PicaTextureError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::physical_memory::VRAM_BASE;

    fn sample_unit(format: PicaTextureFormat) -> PicaTextureUnit {
        PicaTextureUnit {
            unit: 0,
            enabled: true,
            address: VRAM_BASE,
            width: 8,
            height: 8,
            format,
            wrap_s: PicaTextureWrap::ClampToEdge,
            wrap_t: PicaTextureWrap::ClampToEdge,
            magnification_filter: PicaTextureFilter::Nearest,
            minification_filter: PicaTextureFilter::Nearest,
            mipmap_filter_linear: false,
            border_color: [7, 11, 13, 17],
        }
    }

    #[test]
    fn disabled_texture_units_ignore_uninitialized_dimensions_and_format() {
        let pica = Pica200::new();
        let unit = PicaTextureUnit::from_pica(&pica, 1).unwrap();
        assert!(!unit.enabled);
        assert_eq!(
            unit.sample(&GpuPhysicalMemory::new(), 0.5, 0.5, false),
            Err(PicaTextureError::Disabled { unit: 1 })
        );
    }

    #[test]
    fn pica_texture_registers_decode_address_dimensions_and_filters() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        pica.write_register(0x081, 0x8040_2010).unwrap();
        pica.write_register(0x082, (64 << 16) | 32).unwrap();
        pica.write_register(
            0x083,
            (1 << 1) | (1 << 2) | (2 << 8) | (3 << 12) | (1 << 24),
        )
        .unwrap();
        pica.write_register(0x085, 0x0123_4567).unwrap();
        pica.write_register(0x08E, 0).unwrap();

        let unit = PicaTextureUnit::from_pica(&pica, 0).unwrap();
        assert!(unit.enabled);
        assert_eq!(unit.address, 0x091A_2B38);
        assert_eq!((unit.width, unit.height), (64, 32));
        assert_eq!(unit.wrap_t, PicaTextureWrap::Repeat);
        assert_eq!(unit.wrap_s, PicaTextureWrap::MirroredRepeat);
        assert_eq!(unit.magnification_filter, PicaTextureFilter::Linear);
        assert_eq!(unit.minification_filter, PicaTextureFilter::Linear);
        assert!(unit.mipmap_filter_linear);
        assert_eq!(unit.border_color, [0x10, 0x20, 0x40, 0x80]);
    }

    #[test]
    fn samples_rgba8888_from_morton_ordered_8x8_tile() {
        let mut memory = GpuPhysicalMemory::new();
        let morton = morton_index(3, 5);
        memory
            .write(VRAM_BASE + (morton * 4) as u32, &[40, 30, 20, 10])
            .unwrap();
        let unit = sample_unit(PicaTextureFormat::Rgba8);
        assert_eq!(
            unit.sample(&memory, 3.5 / 8.0, 5.5 / 8.0, false).unwrap(),
            [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0]
        );
    }

    #[test]
    fn decodes_rgb_and_packed_color_formats() {
        assert_eq!(
            PicaTextureFormat::Rgb8.decode_texel(&[1, 2, 3]),
            [1, 2, 3, 255]
        );
        assert_eq!(
            PicaTextureFormat::Rgba5551.decode_texel(&0xFFFFu16.to_le_bytes()),
            [255; 4]
        );
        assert_eq!(
            PicaTextureFormat::Rgb565.decode_texel(&0xFFFFu16.to_le_bytes()),
            [255, 255, 255, 255]
        );
        assert_eq!(
            PicaTextureFormat::Rgba4444.decode_texel(&0xFFFFu16.to_le_bytes()),
            [255; 4]
        );
    }

    #[test]
    fn decodes_intensity_alpha_and_alpha_only_formats() {
        assert_eq!(
            PicaTextureFormat::IntensityAlpha8.decode_texel(&[64, 192]),
            [64, 64, 64, 192]
        );
        assert_eq!(
            PicaTextureFormat::Intensity8.decode_texel(&[90]),
            [90, 90, 90, 255]
        );
        assert_eq!(
            PicaTextureFormat::Alpha8.decode_texel(&[70]),
            [255, 255, 255, 70]
        );
        assert_eq!(
            PicaTextureFormat::IntensityAlpha44.decode_texel(&[0xA3]),
            [170, 170, 170, 51]
        );
    }

    #[test]
    fn supports_repeat_mirrored_repeat_and_border_sampling() {
        let mut memory = GpuPhysicalMemory::new();
        let mut tiled = vec![0; TILE_PIXELS * 4];
        for y in 0..8 {
            for x in 0..8 {
                let offset = morton_index(x, y) * 4;
                tiled[offset..offset + 4]
                    .copy_from_slice(&[255, 0, y as u8 * 20, x as u8 * 20]);
            }
        }
        memory.write(VRAM_BASE, &tiled).unwrap();

        let mut unit = sample_unit(PicaTextureFormat::Rgba8);
        unit.wrap_s = PicaTextureWrap::Repeat;
        let repeated = unit
            .sample(&memory, 1.0 + 1.5 / 8.0, 0.5 / 8.0, false)
            .unwrap();
        assert_eq!(repeated[0], 20.0 / 255.0);

        unit.wrap_s = PicaTextureWrap::MirroredRepeat;
        let mirrored = unit
            .sample(&memory, 1.0 + 1.5 / 8.0, 0.5 / 8.0, false)
            .unwrap();
        assert_eq!(mirrored[0], 120.0 / 255.0);

        unit.wrap_s = PicaTextureWrap::ClampToBorder;
        assert_eq!(
            unit.sample(&memory, -0.1, 0.5, false).unwrap(),
            [7.0 / 255.0, 11.0 / 255.0, 13.0 / 255.0, 17.0 / 255.0]
        );
    }

    #[test]
    fn linear_filter_interpolates_four_texels_and_edge_clamp_is_stable() {
        let mut memory = GpuPhysicalMemory::new();
        let mut tiled = vec![0; TILE_PIXELS * 4];
        for (x, color) in [(0u8, [255, 0, 0, 0]), (1u8, [255, 0, 0, 255])] {
            let offset = morton_index(x as usize, 0) * 4;
            tiled[offset..offset + 4].copy_from_slice(&color);
        }
        memory.write(VRAM_BASE, &tiled).unwrap();

        let mut unit = sample_unit(PicaTextureFormat::Rgba8);
        unit.magnification_filter = PicaTextureFilter::Linear;
        let filtered = unit.sample(&memory, 1.0 / 8.0, 0.5 / 8.0, false).unwrap();
        assert!((filtered[0] - 128.0 / 255.0).abs() < 1.0e-6);
        assert_eq!(&filtered[1..], &[0.0, 0.0, 1.0]);
        assert_eq!(
            unit.sample(&memory, 0.0, 0.0, false).unwrap(),
            [0.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn reports_unsupported_compressed_and_packed_formats() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        pica.write_register(0x082, (8 << 16) | 8).unwrap();
        pica.write_register(0x083, 0).unwrap();
        pica.write_register(0x08E, 0xC).unwrap();
        assert!(matches!(
            PicaTextureUnit::from_pica(&pica, 0),
            Err(PicaTextureError::UnsupportedFormat {
                unit: 0,
                format: 0xC
            })
        ));
    }
}
