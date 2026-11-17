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

/// GPUREG_TEXUNITn_LOD: bias in bits 0..12, max level in 16..19, min level in
/// 24..27. Layout from public documentation, not verified on hardware.
const LOD_REGISTERS: [u16; PICA_TEXTURE_UNIT_COUNT] = [0x084, 0x094, 0x09C];

/// Texture formats implemented by the software sampler.
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
    /// 4-bit intensity, two texels per byte (even texel in the low nibble).
    Intensity4,
    /// 4-bit alpha, two texels per byte (even texel in the low nibble).
    Alpha4,
    /// ETC1 4x4 blocks, 8 bytes each.
    Etc1,
    /// ETC1 color block preceded by an 8-byte 4-bit alpha block.
    Etc1A4,
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
            0xA => Ok(Self::Intensity4),
            0xB => Ok(Self::Alpha4),
            0xC => Ok(Self::Etc1),
            0xD => Ok(Self::Etc1A4),
            other => Err(PicaTextureError::UnsupportedFormat {
                unit,
                format: other as u8,
            }),
        }
    }

    /// Storage size of one texel in bits (block formats are averaged).
    const fn bits_per_texel(self) -> usize {
        match self {
            Self::Intensity4 | Self::Alpha4 | Self::Etc1 => 4,
            Self::Etc1A4 => 8,
            other => other.bytes_per_texel() * 8,
        }
    }

    const fn bytes_per_texel(self) -> usize {
        match self {
            // Sub-byte and block formats are addressed through bits_per_texel.
            Self::Intensity4 | Self::Alpha4 | Self::Etc1 | Self::Etc1A4 => 0,
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
            // Handled by the nibble and block readers; never reached through
            // the byte-addressed path.
            Self::Intensity4 | Self::Alpha4 | Self::Etc1 | Self::Etc1A4 => [0; 4],
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
    /// Highest mip level stored after level 0 (0 means no mipmaps).
    pub max_level: u8,
    /// Lowest mip level the sampler may select.
    pub min_level: u8,
    /// LOD bias as signed fixed point with 8 fractional bits.
    pub lod_bias_fixed: i16,
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
                max_level: 0,
                min_level: 0,
                lod_bias_fixed: 0,
                border_color,
            });
        }

        let dimensions = pica.register(registers[1]).unwrap_or(0);
        let parameters = pica.register(registers[2]).unwrap_or(0);
        let address_register = pica.register(registers[3]).unwrap_or(0);
        let format_register = pica.register(registers[4]).unwrap_or(0);
        let lod = pica.register(LOD_REGISTERS[unit]).unwrap_or(0);

        let texture_type = (parameters >> 28) & 0x7;
        if unit == 0 && texture_type != 0 {
            return Err(PicaTextureError::UnsupportedType {
                unit,
                texture_type: texture_type as u8,
            });
        }

        let width = ((dimensions >> 16) & 0x7FF) as u16;
        let height = (dimensions & 0x7FF) as u16;
        if !(8..=1024).contains(&width)
            || !(8..=1024).contains(&height)
            || !width.is_power_of_two()
            || !height.is_power_of_two()
        {
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
            max_level: ((lod >> 16) & 0xF) as u8,
            min_level: ((lod >> 24) & 0xF) as u8,
            // Sign-extend the 13-bit field.
            lod_bias_fixed: (((lod & 0x1FFF) << 3) as u16 as i16) >> 3,
            border_color,
        })
    }

    /// Samples normalized texture coordinates using the selected min/mag filter.
    ///
    /// This samples mip level zero. Use [`Self::sample_lod`] when the caller
    /// has a level-of-detail estimate.
    pub fn sample(
        &self,
        memory: &GpuPhysicalMemory,
        s: f32,
        t: f32,
        minifying: bool,
    ) -> Result<[f32; 4], PicaTextureError> {
        self.check_sample_input(s, t)?;
        let filter = if minifying {
            self.minification_filter
        } else {
            self.magnification_filter
        };
        self.sample_level(memory, 0, filter, s, t)
    }

    /// Samples with an explicit level of detail (log2 of texels per pixel).
    ///
    /// A LOD at or below zero magnifies on level 0 with the magnification
    /// filter. Otherwise the level is chosen after adding the guest bias and
    /// clamping to the guest min/max level and to the levels that fit the
    /// texture; with linear mip filtering the two neighbouring levels are
    /// blended, otherwise the nearest level is used. Level layout, rounding
    /// and the LOD register encoding are not verified on hardware.
    pub fn sample_lod(
        &self,
        memory: &GpuPhysicalMemory,
        s: f32,
        t: f32,
        lod: f32,
    ) -> Result<[f32; 4], PicaTextureError> {
        self.check_sample_input(s, t)?;
        if !lod.is_finite() {
            return Err(PicaTextureError::InvalidCoordinates { s, t });
        }
        let lod = lod + f32::from(self.lod_bias_fixed) / 256.0;
        if lod <= 0.0 {
            return self.sample_level(memory, 0, self.magnification_filter, s, t);
        }
        let top = f32::from(self.highest_level().min(self.max_level));
        let bottom = f32::from(self.min_level).min(top);
        let lod = lod.clamp(bottom, top);
        let filter = self.minification_filter;
        if self.mipmap_filter_linear {
            let lower = lod.floor();
            let upper = lod.ceil();
            let first = self.sample_level(memory, lower as usize, filter, s, t)?;
            if lower == upper {
                return Ok(first);
            }
            let second = self.sample_level(memory, upper as usize, filter, s, t)?;
            let amount = lod - lower;
            Ok(std::array::from_fn(|channel| {
                first[channel] + (second[channel] - first[channel]) * amount
            }))
        } else {
            self.sample_level(memory, lod.round() as usize, filter, s, t)
        }
    }

    /// Largest level whose dimensions are still at least one texel.
    fn highest_level(&self) -> u8 {
        self.width.max(self.height).ilog2() as u8
    }

    fn check_sample_input(&self, s: f32, t: f32) -> Result<(), PicaTextureError> {
        if !self.enabled {
            return Err(PicaTextureError::Disabled { unit: self.unit });
        }
        if !s.is_finite() || !t.is_finite() {
            return Err(PicaTextureError::InvalidCoordinates { s, t });
        }
        Ok(())
    }

    fn sample_level(
        &self,
        memory: &GpuPhysicalMemory,
        level: usize,
        filter: PicaTextureFilter,
        s: f32,
        t: f32,
    ) -> Result<[f32; 4], PicaTextureError> {
        let geometry = self.level_geometry(level)?;
        let rgba = match filter {
            PicaTextureFilter::Nearest => self.sample_nearest(memory, geometry, s, t)?,
            PicaTextureFilter::Linear => self.sample_linear(memory, geometry, s, t)?,
        };
        Ok(rgba.map(|channel| f32::from(channel) / 255.0))
    }

    /// Address and size of a mip level. Levels follow each other in memory,
    /// each rounded up to whole 8x8 tiles.
    fn level_geometry(&self, level: usize) -> Result<LevelGeometry, PicaTextureError> {
        let mut address = u64::from(self.address);
        let mut width = usize::from(self.width);
        let mut height = usize::from(self.height);
        for _ in 0..level {
            let tiles = width.div_ceil(TILE_EDGE) * height.div_ceil(TILE_EDGE);
            address += (tiles * TILE_PIXELS * self.format.bits_per_texel() / 8) as u64;
            width = (width / 2).max(1);
            height = (height / 2).max(1);
        }
        let address = u32::try_from(address).map_err(|_| PicaTextureError::AddressOverflow)?;
        Ok(LevelGeometry {
            address,
            width,
            height,
        })
    }

    fn sample_nearest(
        &self,
        memory: &GpuPhysicalMemory,
        level: LevelGeometry,
        s: f32,
        t: f32,
    ) -> Result<[u8; 4], PicaTextureError> {
        let Some(s) = wrap_coordinate(s, self.wrap_s) else {
            return Ok(self.border_color);
        };
        let Some(t) = wrap_coordinate(t, self.wrap_t) else {
            return Ok(self.border_color);
        };

        let mut x = (s * level.width as f32).floor() as i64;
        let mut y = (t * level.height as f32).floor() as i64;
        if s == 1.0 {
            x = level.width as i64 - 1;
        }
        if t == 1.0 {
            y = level.height as i64 - 1;
        }
        self.read_wrapped_texel(memory, level, x, y)
    }

    fn sample_linear(
        &self,
        memory: &GpuPhysicalMemory,
        level: LevelGeometry,
        s: f32,
        t: f32,
    ) -> Result<[u8; 4], PicaTextureError> {
        let Some(s) = wrap_coordinate(s, self.wrap_s) else {
            return Ok(self.border_color);
        };
        let Some(t) = wrap_coordinate(t, self.wrap_t) else {
            return Ok(self.border_color);
        };

        let x = f64::from(s) * level.width as f64 - 0.5;
        let y = f64::from(t) * level.height as f64 - 0.5;
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

        let c00 = self.read_wrapped_texel(memory, level, x0, y0)?;
        let c10 = self.read_wrapped_texel(memory, level, x1, y0)?;
        let c01 = self.read_wrapped_texel(memory, level, x0, y1)?;
        let c11 = self.read_wrapped_texel(memory, level, x1, y1)?;

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
        level: LevelGeometry,
        x: i64,
        y: i64,
    ) -> Result<[u8; 4], PicaTextureError> {
        let Some(x) = wrap_index(x, level.width, self.wrap_s) else {
            return Ok(self.border_color);
        };
        let Some(y) = wrap_index(y, level.height, self.wrap_t) else {
            return Ok(self.border_color);
        };

        let tiles_per_row = level.width.div_ceil(TILE_EDGE);
        let tile_index = (y / TILE_EDGE)
            .checked_mul(tiles_per_row)
            .and_then(|value| value.checked_add(x / TILE_EDGE))
            .ok_or(PicaTextureError::AddressOverflow)?;
        let in_tile_x = x & 7;
        let in_tile_y = y & 7;
        let format = self.format;
        let tile_bytes = TILE_PIXELS * format.bits_per_texel() / 8;
        let tile_address = tile_index
            .checked_mul(tile_bytes)
            .and_then(|offset| u64::from(level.address).checked_add(offset as u64))
            .filter(|&value| value <= u64::from(u32::MAX))
            .ok_or(PicaTextureError::AddressOverflow)? as u32;
        let read = |offset: usize, bytes: &mut [u8]| {
            let address = tile_address
                .checked_add(offset as u32)
                .ok_or(PicaTextureError::AddressOverflow)?;
            memory
                .read(address, bytes)
                .map_err(PicaTextureError::Memory)
        };

        match format {
            PicaTextureFormat::Intensity4 | PicaTextureFormat::Alpha4 => {
                let texel = morton_index(in_tile_x, in_tile_y);
                let mut byte = [0u8; 1];
                read(texel / 2, &mut byte)?;
                let nibble = if texel % 2 == 0 {
                    byte[0] & 0xF
                } else {
                    byte[0] >> 4
                };
                let value = nibble * 17;
                Ok(if format == PicaTextureFormat::Intensity4 {
                    [value, value, value, 255]
                } else {
                    [255, 255, 255, value]
                })
            }
            PicaTextureFormat::Etc1 | PicaTextureFormat::Etc1A4 => {
                let block = (in_tile_y / 4) * 2 + in_tile_x / 4;
                let block_bytes = tile_bytes / 4;
                let mut raw = [0u8; 16];
                read(block * block_bytes, &mut raw[..block_bytes])?;
                let (alpha, color) = if format == PicaTextureFormat::Etc1A4 {
                    (Some(&raw[0..8]), &raw[8..16])
                } else {
                    (None, &raw[0..8])
                };
                let pixel = (in_tile_x & 3) * 4 + (in_tile_y & 3);
                let [r, g, b] = etc1_texel(u64::from_le_bytes(color.try_into().unwrap()), pixel);
                let a = alpha.map_or(255, |bytes| {
                    let word = u64::from_le_bytes(bytes.try_into().unwrap());
                    (((word >> (pixel * 4)) & 0xF) as u8) * 17
                });
                Ok([r, g, b, a])
            }
            _ => {
                let texel_bytes = format.bytes_per_texel();
                let texel = morton_index(in_tile_x, in_tile_y);
                let mut bytes = [0u8; 4];
                read(texel * texel_bytes, &mut bytes[..texel_bytes])?;
                Ok(format.decode_texel(&bytes[..texel_bytes]))
            }
        }
    }
}

/// Location and size of one mip level.
#[derive(Clone, Copy, Debug)]
struct LevelGeometry {
    address: u32,
    width: usize,
    height: usize,
}

const ETC1_MODIFIERS: [[i16; 4]; 8] = [
    [2, 8, -2, -8],
    [5, 17, -5, -17],
    [9, 29, -9, -29],
    [13, 42, -13, -42],
    [18, 60, -18, -60],
    [24, 80, -24, -80],
    [33, 106, -33, -106],
    [47, 183, -47, -183],
];

/// Decodes one ETC1 texel. `word` is the 64-bit block with the fields laid out
/// as in the public ETC1 definition; `pixel` is `x * 4 + y` within the block.
/// The 3DS stores the word little-endian; that byte order is unverified.
fn etc1_texel(word: u64, pixel: usize) -> [u8; 3] {
    // The public definition numbers bytes most-significant first, so byte 0
    // of the block is bits 63..56 of the word.
    let byte = |index: usize| ((word >> (56 - index * 8)) & 0xFF) as i32;
    let flags = byte(3);
    let differential = flags & 2 != 0;
    let flip = flags & 1 != 0;
    let (base1, base2) = if differential {
        let channel = |index: usize| {
            let first = byte(index) >> 3;
            let delta = ((byte(index) & 7) << 5) as i8 as i32 >> 5;
            (
                (first << 3) | (first >> 2),
                (((first + delta) & 0x1F) << 3) | (((first + delta) & 0x1F) >> 2),
            )
        };
        let (r1, r2) = channel(0);
        let (g1, g2) = channel(1);
        let (b1, b2) = channel(2);
        ([r1, g1, b1], [r2, g2, b2])
    } else {
        let channel = |index: usize| {
            let high = byte(index) >> 4;
            let low = byte(index) & 0xF;
            ((high << 4) | high, (low << 4) | low)
        };
        let (r1, r2) = channel(0);
        let (g1, g2) = channel(1);
        let (b1, b2) = channel(2);
        ([r1, g1, b1], [r2, g2, b2])
    };

    let x = pixel / 4;
    let y = pixel % 4;
    let second = if flip { y >= 2 } else { x >= 2 };
    let (base, table) = if second {
        (base2, (flags >> 2) & 7)
    } else {
        (base1, (flags >> 5) & 7)
    };
    let indices = (word & 0xFFFF_FFFF) as u32;
    let lsb = (indices >> pixel) & 1;
    let msb = (indices >> (pixel + 16)) & 1;
    let modifier = i32::from(ETC1_MODIFIERS[table as usize][(msb * 2 + lsb) as usize]);
    base.map(|value| (value + modifier).clamp(0, 255) as u8)
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
            max_level: 0,
            min_level: 0,
            lod_bias_fixed: 0,
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
    fn rejects_texture_dimensions_outside_pica_power_of_two_range() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        pica.write_register(0x082, (12 << 16) | 8).unwrap();
        assert!(matches!(
            PicaTextureUnit::from_pica(&pica, 0),
            Err(PicaTextureError::InvalidDimensions {
                unit: 0,
                width: 12,
                height: 8
            })
        ));

        pica.write_register(0x082, (1024 << 16) | 8).unwrap();
        assert!(PicaTextureUnit::from_pica(&pica, 0).is_ok());
        pica.write_register(0x082, (1032 << 16) | 8).unwrap();
        assert!(matches!(
            PicaTextureUnit::from_pica(&pica, 0),
            Err(PicaTextureError::InvalidDimensions {
                unit: 0,
                width: 1032,
                height: 8
            })
        ));
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
                tiled[offset..offset + 4].copy_from_slice(&[255, 0, y as u8 * 20, x as u8 * 20]);
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
    fn reports_unsupported_hilo_format() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        pica.write_register(0x082, (8 << 16) | 8).unwrap();
        pica.write_register(0x083, 0).unwrap();
        pica.write_register(0x08E, 0x6).unwrap();
        assert!(matches!(
            PicaTextureUnit::from_pica(&pica, 0),
            Err(PicaTextureError::UnsupportedFormat {
                unit: 0,
                format: 0x6
            })
        ));
    }

    fn mip_unit(max_level: u8) -> PicaTextureUnit {
        let mut unit = sample_unit(PicaTextureFormat::Intensity8);
        unit.width = 16;
        unit.height = 16;
        unit.max_level = max_level;
        unit
    }

    /// Level 0 (16x16) = 10, level 1 (8x8) = 100, level 2 (4x4, one padded
    /// tile) = 200, each rounded up to whole 8x8 tiles.
    fn mip_memory() -> GpuPhysicalMemory {
        let mut memory = GpuPhysicalMemory::new();
        memory.write(VRAM_BASE, &[10; 256]).unwrap();
        memory.write(VRAM_BASE + 256, &[100; 64]).unwrap();
        memory.write(VRAM_BASE + 320, &[200; 64]).unwrap();
        memory
    }

    fn level(value: u8) -> [f32; 4] {
        let v = f32::from(value) / 255.0;
        [v, v, v, 1.0]
    }

    #[test]
    fn sample_lod_selects_the_mip_level_and_clamps_to_the_guest_maximum() {
        let memory = mip_memory();
        let unit = mip_unit(2);
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 0.0).unwrap(), level(10));
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, -3.0).unwrap(), level(10));
        // 0.4 rounds to level 0; 0.6 and 1.0 select level 1.
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 0.4).unwrap(), level(10));
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 0.6).unwrap(), level(100));
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 1.0).unwrap(), level(100));
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 2.0).unwrap(), level(200));
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 9.0).unwrap(), level(200));
        // Without stored mip levels every LOD reads level 0.
        assert_eq!(
            mip_unit(0).sample_lod(&memory, 0.5, 0.5, 3.0).unwrap(),
            level(10)
        );
    }

    #[test]
    fn sample_lod_applies_bias_min_level_and_linear_mip_blending() {
        let memory = mip_memory();
        let mut unit = mip_unit(2);
        unit.lod_bias_fixed = -256;
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 2.0).unwrap(), level(100));

        let mut unit = mip_unit(2);
        unit.min_level = 1;
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 0.6).unwrap(), level(100));

        let mut unit = mip_unit(2);
        unit.mipmap_filter_linear = true;
        let blended = unit.sample_lod(&memory, 0.5, 0.5, 1.5).unwrap();
        assert!((blended[0] - 150.0 / 255.0).abs() < 1.0e-6);
        assert_eq!(unit.sample_lod(&memory, 0.5, 0.5, 1.0).unwrap(), level(100));
    }

    #[test]
    fn sample_lod_rejects_non_finite_lod_and_disabled_units() {
        let memory = mip_memory();
        let mut unit = mip_unit(2);
        assert!(unit.sample_lod(&memory, 0.5, 0.5, f32::NAN).is_err());
        unit.enabled = false;
        assert!(matches!(
            unit.sample_lod(&memory, 0.5, 0.5, 0.0),
            Err(PicaTextureError::Disabled { .. })
        ));
    }

    #[test]
    fn lod_registers_decode_levels_and_signed_bias() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        pica.write_register(0x082, (16 << 16) | 16).unwrap();
        pica.write_register(0x083, 0).unwrap();
        pica.write_register(0x08E, 7).unwrap();
        pica.write_register(0x084, (3 << 16) | (1 << 24) | 0x1F00)
            .unwrap();
        let unit = PicaTextureUnit::from_pica(&pica, 0).unwrap();
        assert_eq!((unit.max_level, unit.min_level), (3, 1));
        assert_eq!(unit.lod_bias_fixed, -256);
        pica.write_register(0x084, 0x0100).unwrap();
        assert_eq!(
            PicaTextureUnit::from_pica(&pica, 0).unwrap().lod_bias_fixed,
            256
        );
    }

    #[test]
    fn four_bit_intensity_and_alpha_formats_read_nibbles_in_morton_order() {
        let mut memory = GpuPhysicalMemory::new();
        let even = morton_index(2, 5);
        let odd = morton_index(3, 5);
        assert_eq!((even % 2, odd % 2), (0, 1));
        let mut tile = [0u8; 32];
        tile[even / 2] |= 0x3;
        tile[odd / 2] |= 0xC << 4;
        memory.write(VRAM_BASE, &tile).unwrap();
        for format in [PicaTextureFormat::Intensity4, PicaTextureFormat::Alpha4] {
            let unit = sample_unit(format);
            let low = unit.sample(&memory, 2.5 / 8.0, 5.5 / 8.0, false).unwrap();
            let high = unit.sample(&memory, 3.5 / 8.0, 5.5 / 8.0, false).unwrap();
            if format == PicaTextureFormat::Intensity4 {
                assert_eq!(low, [51.0 / 255.0, 51.0 / 255.0, 51.0 / 255.0, 1.0]);
                assert_eq!(high[0], 204.0 / 255.0);
            } else {
                assert_eq!(low, [1.0, 1.0, 1.0, 51.0 / 255.0]);
                assert_eq!(high[3], 204.0 / 255.0);
            }
        }
    }

    /// Builds an ETC1 block from its public byte layout (byte 0 first) and
    /// returns it in the little-endian order the sampler expects.
    fn etc1_block(bytes: [u8; 8]) -> [u8; 8] {
        u64::from_be_bytes(bytes).to_le_bytes()
    }

    #[test]
    fn etc1_individual_mode_applies_per_subblock_tables_and_pixel_indices() {
        // R1=0x8 R2=0xF, G1=0x4 G2=0x0, B1=B2=0; tables 0 and 1; no flip.
        let mut bytes = [0x8F, 0x40, 0x00, 0b0000_0100, 0, 0, 0, 0];
        // Pixel (x=0, y=1) is index 1: both index bits set -> -8 on table 0.
        let word_indices: u32 = (1 << 1) | (1 << 17);
        bytes[4..8].copy_from_slice(&word_indices.to_be_bytes());
        let mut memory = GpuPhysicalMemory::new();
        memory.write(VRAM_BASE, &etc1_block(bytes)).unwrap();
        let unit = sample_unit(PicaTextureFormat::Etc1);
        let rgba = |x: f32, y: f32| {
            unit.sample(&memory, x / 8.0, y / 8.0, false)
                .unwrap()
                .map(|channel| (channel * 255.0).round() as i32)
        };
        // Left sub-block, index 0: +2 on base (0x88, 0x44, 0).
        assert_eq!(rgba(0.5, 0.5), [138, 70, 2, 255]);
        // Left sub-block, index 3: -8 (clamped at zero for blue).
        assert_eq!(rgba(0.5, 1.5), [128, 60, 0, 255]);
        // Right sub-block uses table 1 (+5): (0xFF, 0, 0) clamps to (255, 5, 5).
        assert_eq!(rgba(2.5, 0.5), [255, 5, 5, 255]);
    }

    #[test]
    fn etc1_differential_mode_and_flip_split_the_block() {
        // R1=16, dR=+1; G and B base 16 with zero delta; differential, flip.
        let bytes = [0x81, 0x80, 0x80, 0b0000_0011, 0, 0, 0, 0];
        let mut memory = GpuPhysicalMemory::new();
        memory.write(VRAM_BASE, &etc1_block(bytes)).unwrap();
        let unit = sample_unit(PicaTextureFormat::Etc1);
        let rgb = |x: f32, y: f32| {
            let c = unit.sample(&memory, x / 8.0, y / 8.0, false).unwrap();
            [c[0], c[1], c[2]].map(|channel| (channel * 255.0).round() as i32)
        };
        // Base 1: 5-bit 16 -> 132; base 2: 5-bit 17 -> 140; modifier +2.
        assert_eq!(rgb(0.5, 0.5), [134, 134, 134]);
        assert_eq!(rgb(0.5, 2.5), [142, 134, 134]);
    }

    #[test]
    fn etc1_blocks_are_placed_in_z_order_inside_the_tile_and_a4_adds_alpha() {
        let color = etc1_block([0x88, 0x88, 0x88, 0, 0, 0, 0, 0]);
        let mut memory = GpuPhysicalMemory::new();
        // ETC1A4: 8-byte alpha block, then the color block; block 1 is x 4..8.
        let mut tile = vec![0u8; 64];
        tile[16..24].copy_from_slice(&0xAu64.to_le_bytes());
        tile[24..32].copy_from_slice(&color);
        memory.write(VRAM_BASE, &tile).unwrap();
        let unit = sample_unit(PicaTextureFormat::Etc1A4);
        let texel = unit.sample(&memory, 4.5 / 8.0, 0.5 / 8.0, false).unwrap();
        assert_eq!(texel[3], 170.0 / 255.0);
        assert_eq!((texel[0] * 255.0).round() as i32, 0x88 + 2);
        // Block 0 is empty memory: alpha 0.
        assert_eq!(
            unit.sample(&memory, 0.5 / 8.0, 0.5 / 8.0, false).unwrap()[3],
            0.0
        );
    }
}
