//! PICA200 physical depth/stencil buffer storage.
//!
//! The depth plane is laid out in 8x8 Morton tiles, with tiles ordered by
//! tile row and then tile column. Pixels are little-endian: D16 stores 16-bit
//! normalized depth; D24 stores a 24-bit normalized value; D24S8 stores the
//! same low 24 depth bits followed by one stencil byte.
//!
//! This module only models storage and access permissions. Stencil comparison
//! and stencil operations remain in the fragment tests/pipeline work.

use super::physical_memory::{GpuPhysicalMemory, GpuPhysicalMemoryError};
use super::pica200::{
    Pica200, REG_DEPTHBUFFER_FORMAT, REG_DEPTHBUFFER_LOC, REG_DEPTHBUFFER_READ,
    REG_DEPTHBUFFER_WRITE,
};

const TILE_EDGE: usize = 8;
const TILE_PIXELS: usize = TILE_EDGE * TILE_EDGE;

/// Depth formats accepted by GPUREG_DEPTHBUFFER_FORMAT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaDepthBufferFormat {
    D16,
    D24,
    D24S8,
}

impl PicaDepthBufferFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::D16 => 2,
            Self::D24 => 3,
            Self::D24S8 => 4,
        }
    }

    fn decode(register: u32) -> Result<Self, PicaDepthBufferError> {
        match register & 0x3 {
            0 => Ok(Self::D16),
            2 => Ok(Self::D24),
            3 => Ok(Self::D24S8),
            value => Err(PicaDepthBufferError::UnsupportedFormat { value: value as u8 }),
        }
    }

    fn depth_max(self) -> u32 {
        match self {
            Self::D16 => 0xFFFF,
            Self::D24 | Self::D24S8 => 0x00FF_FFFF,
        }
    }

    /// Quantizes a fragment depth to the precision physically stored by this format.
    pub fn quantize_depth(self, value: f32) -> f32 {
        let value = value.clamp(0.0, 1.0);
        let maximum = self.depth_max();
        ((value * maximum as f32).round() as u32) as f32 / maximum as f32
    }

    fn decode_depth(self, bytes: &[u8]) -> f32 {
        let value = match self {
            Self::D16 => u32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            Self::D24 | Self::D24S8 => {
                u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
            }
        };
        value as f32 / self.depth_max() as f32
    }

    fn encode_depth(self, value: f32, bytes: &mut [u8]) {
        let scaled = (value.clamp(0.0, 1.0) * self.depth_max() as f32).round() as u32;
        match self {
            Self::D16 => {
                bytes[..2].copy_from_slice(&(scaled as u16).to_le_bytes());
            }
            Self::D24 | Self::D24S8 => {
                bytes[0] = scaled as u8;
                bytes[1] = (scaled >> 8) as u8;
                bytes[2] = (scaled >> 16) as u8;
            }
        }
    }
}

/// Decoded physical depth-buffer configuration for one PICA framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaDepthBufferConfig {
    pub address: u32,
    pub width: u32,
    pub height: u32,
    pub format: PicaDepthBufferFormat,
    pub depth_read_enabled: bool,
    pub stencil_read_enabled: bool,
    pub depth_write_enabled: bool,
    pub stencil_write_enabled: bool,
}

impl PicaDepthBufferConfig {
    /// Decodes the live depth-buffer registers. A zero depth-buffer address
    /// means that no physical depth surface has been configured.
    pub fn from_pica(
        pica: &Pica200,
        width: u32,
        height: u32,
    ) -> Result<Option<Self>, PicaDepthBufferError> {
        let location = pica.register(REG_DEPTHBUFFER_LOC).unwrap_or(0);
        if location == 0 {
            return Ok(None);
        }
        if width == 0 || height == 0 {
            return Err(PicaDepthBufferError::InvalidDimensions { width, height });
        }
        let format =
            PicaDepthBufferFormat::decode(pica.register(REG_DEPTHBUFFER_FORMAT).unwrap_or(0))?;
        let read = pica.register(REG_DEPTHBUFFER_READ).unwrap_or(0);
        let write = pica.register(REG_DEPTHBUFFER_WRITE).unwrap_or(0);
        Ok(Some(Self {
            address: (location & 0x0FFF_FFFF) << 3,
            width,
            height,
            format,
            depth_read_enabled: read & (1 << 1) != 0,
            stencil_read_enabled: read & 1 != 0,
            depth_write_enabled: write & (1 << 1) != 0,
            stencil_write_enabled: write & 1 != 0,
        }))
    }

    /// Returns the complete padded 8x8 tile-grid allocation size.
    pub fn tiled_storage_size(self) -> Result<usize, PicaDepthBufferError> {
        tile_count(self.width as usize)?
            .checked_mul(tile_count(self.height as usize)?)
            .and_then(|tiles| tiles.checked_mul(TILE_PIXELS))
            .and_then(|pixels| pixels.checked_mul(self.format.bytes_per_pixel()))
            .ok_or(PicaDepthBufferError::SizeOverflow)
    }

    /// Reads the depth plane. When the guest disables depth-buffer reads,
    /// fragments begin with the configured clear value rather than exposing
    /// stale physical memory to the depth test.
    pub fn read_depth_values(
        self,
        memory: &GpuPhysicalMemory,
        clear_depth: f32,
    ) -> Result<Vec<f32>, PicaDepthBufferError> {
        if !clear_depth.is_finite() || !(0.0..=1.0).contains(&clear_depth) {
            return Err(PicaDepthBufferError::InvalidDepth { depth: clear_depth });
        }
        let pixel_count = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(PicaDepthBufferError::SizeOverflow)?;
        if !self.depth_read_enabled {
            let mut values = Vec::new();
            values
                .try_reserve_exact(pixel_count)
                .map_err(|_| PicaDepthBufferError::AllocationFailed)?;
            values.resize(pixel_count, clear_depth);
            return Ok(values);
        }

        let raw = self.read_raw(memory)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(pixel_count)
            .map_err(|_| PicaDepthBufferError::AllocationFailed)?;
        values.resize(pixel_count, clear_depth);

        let tiles_x = tile_count(self.width as usize)?;
        let bytes_per_pixel = self.format.bytes_per_pixel();
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let offset = tiled_pixel_offset(x, y, tiles_x, bytes_per_pixel)?;
                let destination = y * self.width as usize + x;
                values[destination] = self
                    .format
                    .decode_depth(&raw[offset..offset + bytes_per_pixel]);
            }
        }
        Ok(values)
    }

    /// Writes only the depth plane, preserving D24S8 stencil bytes.
    ///
    /// The complete physical range is validated before the first write.
    /// If the guest disabled depth writes, this operation is a no-op.
    pub fn write_depth_values(
        self,
        values: &[f32],
        memory: &mut GpuPhysicalMemory,
    ) -> Result<u64, PicaDepthBufferError> {
        if !self.depth_write_enabled {
            return Ok(0);
        }
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(PicaDepthBufferError::SizeOverflow)?;
        if values.len() != expected {
            return Err(PicaDepthBufferError::BufferLengthMismatch {
                expected,
                actual: values.len(),
            });
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(PicaDepthBufferError::InvalidDepth { depth: f32::NAN });
        }

        let storage_size = self.tiled_storage_size()?;
        memory.validate_range(self.address, storage_size)?;
        // Read the existing bytes so depth updates preserve the stencil lane
        // in D24S8 surfaces even when no stencil operation is performed here.
        let mut raw = self.read_raw(memory)?;
        let tiles_x = tile_count(self.width as usize)?;
        let bytes_per_pixel = self.format.bytes_per_pixel();
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let source = y * self.width as usize + x;
                let offset = tiled_pixel_offset(x, y, tiles_x, bytes_per_pixel)?;
                self.format
                    .encode_depth(values[source], &mut raw[offset..offset + bytes_per_pixel]);
            }
        }
        memory.write(self.address, &raw)?;
        Ok(raw.len() as u64)
    }

    /// Reads stencil values for D24S8 if stencil reads are enabled.
    pub fn read_stencil_values(
        self,
        memory: &GpuPhysicalMemory,
    ) -> Result<Option<Vec<u8>>, PicaDepthBufferError> {
        if self.format != PicaDepthBufferFormat::D24S8 || !self.stencil_read_enabled {
            return Ok(None);
        }
        let raw = self.read_raw(memory)?;
        let pixel_count = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(PicaDepthBufferError::SizeOverflow)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(pixel_count)
            .map_err(|_| PicaDepthBufferError::AllocationFailed)?;
        values.resize(pixel_count, 0);
        let tiles_x = tile_count(self.width as usize)?;
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let source = y * self.width as usize + x;
                let offset = tiled_pixel_offset(x, y, tiles_x, 4)?;
                values[source] = raw[offset + 3];
            }
        }
        Ok(Some(values))
    }

    /// Writes stencil values for D24S8, preserving the 24-bit depth lane.
    pub fn write_stencil_values(
        self,
        values: &[u8],
        memory: &mut GpuPhysicalMemory,
    ) -> Result<u64, PicaDepthBufferError> {
        if self.format != PicaDepthBufferFormat::D24S8 {
            return Err(PicaDepthBufferError::StencilRequiresD24S8);
        }
        if !self.stencil_write_enabled {
            return Ok(0);
        }
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(PicaDepthBufferError::SizeOverflow)?;
        if values.len() != expected {
            return Err(PicaDepthBufferError::BufferLengthMismatch {
                expected,
                actual: values.len(),
            });
        }

        let mut raw = self.read_raw(memory)?;
        let tiles_x = tile_count(self.width as usize)?;
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let source = y * self.width as usize + x;
                let offset = tiled_pixel_offset(x, y, tiles_x, 4)?;
                raw[offset + 3] = values[source];
            }
        }
        memory.write(self.address, &raw)?;
        Ok(raw.len() as u64)
    }

    fn read_raw(self, memory: &GpuPhysicalMemory) -> Result<Vec<u8>, PicaDepthBufferError> {
        let storage_size = self.tiled_storage_size()?;
        memory.validate_range(self.address, storage_size)?;
        let mut raw = Vec::new();
        raw.try_reserve_exact(storage_size)
            .map_err(|_| PicaDepthBufferError::AllocationFailed)?;
        raw.resize(storage_size, 0);
        memory.read(self.address, &mut raw)?;
        Ok(raw)
    }
}

fn tile_count(dimension: usize) -> Result<usize, PicaDepthBufferError> {
    dimension
        .checked_add(TILE_EDGE - 1)
        .map(|value| value / TILE_EDGE)
        .filter(|value| *value != 0)
        .ok_or(PicaDepthBufferError::SizeOverflow)
}

fn tiled_pixel_offset(
    x: usize,
    y: usize,
    tiles_x: usize,
    bytes_per_pixel: usize,
) -> Result<usize, PicaDepthBufferError> {
    let tile_index = (y / TILE_EDGE)
        .checked_mul(tiles_x)
        .and_then(|value| value.checked_add(x / TILE_EDGE))
        .ok_or(PicaDepthBufferError::SizeOverflow)?;
    let morton = morton_index(x & 7, y & 7);
    tile_index
        .checked_mul(TILE_PIXELS)
        .and_then(|value| value.checked_add(morton))
        .and_then(|value| value.checked_mul(bytes_per_pixel))
        .ok_or(PicaDepthBufferError::SizeOverflow)
}

fn morton_index(x: usize, y: usize) -> usize {
    let mut index = 0;
    for bit in 0..3 {
        index |= ((x >> bit) & 1) << (bit * 2);
        index |= ((y >> bit) & 1) << (bit * 2 + 1);
    }
    index
}

/// Errors from decoding or accessing physical PICA depth/stencil storage.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaDepthBufferError {
    UnsupportedFormat { value: u8 },
    InvalidDimensions { width: u32, height: u32 },
    InvalidDepth { depth: f32 },
    SizeOverflow,
    AllocationFailed,
    BufferLengthMismatch { expected: usize, actual: usize },
    StencilRequiresD24S8,
    Memory(GpuPhysicalMemoryError),
}

impl std::fmt::Display for PicaDepthBufferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedFormat { value } => {
                write!(formatter, "PICA depth buffer format {value} is unsupported")
            }
            Self::InvalidDimensions { width, height } => {
                write!(
                    formatter,
                    "PICA depth buffer dimensions {width}x{height} are invalid"
                )
            }
            Self::InvalidDepth { depth } => {
                write!(formatter, "PICA depth value {depth} is invalid")
            }
            Self::SizeOverflow => formatter.write_str("PICA depth buffer size overflowed"),
            Self::AllocationFailed => formatter.write_str("could not allocate PICA depth buffer"),
            Self::BufferLengthMismatch { expected, actual } => write!(
                formatter,
                "PICA depth buffer has {actual} values; expected {expected}"
            ),
            Self::StencilRequiresD24S8 => {
                formatter.write_str("stencil access requires the PICA D24S8 format")
            }
            Self::Memory(error) => write!(formatter, "PICA depth memory access failed: {error}"),
        }
    }
}

impl From<GpuPhysicalMemoryError> for PicaDepthBufferError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::Memory(error)
    }
}

impl std::error::Error for PicaDepthBufferError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::physical_memory::{FCRAM_BASE, VRAM_BASE};

    fn config(format: PicaDepthBufferFormat, width: u32, height: u32) -> PicaDepthBufferConfig {
        PicaDepthBufferConfig {
            address: VRAM_BASE,
            width,
            height,
            format,
            depth_read_enabled: true,
            stencil_read_enabled: true,
            depth_write_enabled: true,
            stencil_write_enabled: true,
        }
    }

    #[test]
    fn decodes_guest_depth_buffer_registers_and_access_permissions() {
        let mut pica = Pica200::new();
        pica.write_register(REG_DEPTHBUFFER_LOC, 0x0123_4567)
            .unwrap();
        pica.write_register(REG_DEPTHBUFFER_FORMAT, 3).unwrap();
        pica.write_register(REG_DEPTHBUFFER_READ, 3).unwrap();
        pica.write_register(REG_DEPTHBUFFER_WRITE, 2).unwrap();

        let decoded = PicaDepthBufferConfig::from_pica(&pica, 400, 240)
            .unwrap()
            .unwrap();
        assert_eq!(decoded.address, 0x091A_2B38);
        assert_eq!(decoded.width, 400);
        assert_eq!(decoded.height, 240);
        assert_eq!(decoded.format, PicaDepthBufferFormat::D24S8);
        assert!(decoded.depth_read_enabled);
        assert!(decoded.stencil_read_enabled);
        assert!(decoded.depth_write_enabled);
        assert!(!decoded.stencil_write_enabled);
    }

    #[test]
    fn a_zero_guest_address_disables_physical_depth_storage() {
        assert_eq!(
            PicaDepthBufferConfig::from_pica(&Pica200::new(), 8, 8).unwrap(),
            None
        );
    }

    #[test]
    fn round_trips_d16_values_in_morton_tile_order() {
        let mut memory = GpuPhysicalMemory::new();
        let config = config(PicaDepthBufferFormat::D16, 16, 8);
        let values = (0..128)
            .map(|index| index as f32 / 127.0)
            .collect::<Vec<_>>();
        config.write_depth_values(&values, &mut memory).unwrap();
        let read = config.read_depth_values(&memory, 1.0).unwrap();
        for (expected, actual) in values.iter().zip(read.iter()) {
            assert!((expected - actual).abs() <= 1.0 / 65535.0);
        }
        assert_eq!(config.tiled_storage_size().unwrap(), 16 * 8 * 2);
    }

    #[test]
    fn round_trips_d24_and_preserves_d24s8_stencil_lane() {
        let mut memory = GpuPhysicalMemory::new();
        let d24 = config(PicaDepthBufferFormat::D24, 8, 8);
        let values = vec![0.5; 64];
        d24.write_depth_values(&values, &mut memory).unwrap();
        let read = d24.read_depth_values(&memory, 1.0).unwrap();
        assert!((read[0] - 0.5).abs() < 1.0 / 0x00FF_FFFF as f32);

        let d24s8 = config(PicaDepthBufferFormat::D24S8, 8, 8);
        let stencil = (0..64).map(|value| value as u8).collect::<Vec<_>>();
        d24s8.write_stencil_values(&stencil, &mut memory).unwrap();
        d24s8
            .write_depth_values(&vec![0.25; 64], &mut memory)
            .unwrap();
        assert_eq!(
            d24s8.read_stencil_values(&memory).unwrap().unwrap(),
            stencil
        );
        let depths = d24s8.read_depth_values(&memory, 1.0).unwrap();
        assert!((depths[0] - 0.25).abs() < 1.0 / 0x00FF_FFFF as f32);
    }

    #[test]
    fn respects_guest_read_and_write_permissions() {
        let mut memory = GpuPhysicalMemory::new();
        let mut config = config(PicaDepthBufferFormat::D16, 8, 8);
        config.depth_read_enabled = false;
        config.depth_write_enabled = false;
        assert_eq!(
            config.read_depth_values(&memory, 0.75).unwrap(),
            vec![0.75; 64]
        );
        assert_eq!(
            config
                .write_depth_values(&vec![0.25; 64], &mut memory)
                .unwrap(),
            0
        );
        assert!(memory.read_u32(VRAM_BASE).is_ok());
    }

    #[test]
    fn rejects_reserved_depth_format_and_non_finite_writes() {
        let mut pica = Pica200::new();
        pica.write_register(REG_DEPTHBUFFER_LOC, 1).unwrap();
        pica.write_register(REG_DEPTHBUFFER_FORMAT, 1).unwrap();
        assert_eq!(
            PicaDepthBufferConfig::from_pica(&pica, 8, 8),
            Err(PicaDepthBufferError::UnsupportedFormat { value: 1 })
        );

        let mut memory = GpuPhysicalMemory::new();
        assert!(matches!(
            config(PicaDepthBufferFormat::D24, 8, 8)
                .write_depth_values(&[f32::NAN; 64], &mut memory),
            Err(PicaDepthBufferError::InvalidDepth { .. })
        ));
    }

    #[test]
    fn physical_address_must_fit_the_supported_memory_regions() {
        let config = PicaDepthBufferConfig {
            address: FCRAM_BASE,
            width: 8,
            height: 8,
            format: PicaDepthBufferFormat::D16,
            depth_read_enabled: true,
            stencil_read_enabled: false,
            depth_write_enabled: true,
            stencil_write_enabled: false,
        };
        assert!(config.tiled_storage_size().is_ok());
    }
}
