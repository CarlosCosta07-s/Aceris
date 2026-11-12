//! PICA200 index-buffer decoding and raw index fetching.
//!
//! The register contains a byte offset from the global vertex-array base and
//! selects unsigned-byte or unsigned-short indices. This helper returns raw
//! indices only; applying GPUREG_VERTEX_OFFSET and resolving the physical base
//! remain explicit responsibilities of the draw-command integration layer.

use super::{GpuPhysicalMemory, GpuPhysicalMemoryError};

pub const MAX_INDICES_PER_READ: usize = 1_048_576;

/// Element width selected by bit 31 of the index-buffer configuration register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexElementWidth {
    UnsignedByte,
    UnsignedShort,
}

impl IndexElementWidth {
    pub const fn byte_width(self) -> usize {
        match self {
            Self::UnsignedByte => 1,
            Self::UnsignedShort => 2,
        }
    }
}

/// Decoded index-buffer configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexBufferLayout {
    byte_offset: u32,
    element_width: IndexElementWidth,
}

impl IndexBufferLayout {
    /// Decodes bits 0..27 as a byte offset and bit 31 as the element width.
    pub const fn from_register(value: u32) -> Self {
        let element_width = if value & (1 << 31) == 0 {
            IndexElementWidth::UnsignedByte
        } else {
            IndexElementWidth::UnsignedShort
        };
        Self {
            byte_offset: value & 0x0FFF_FFFF,
            element_width,
        }
    }

    pub const fn byte_offset(self) -> u32 {
        self.byte_offset
    }

    pub const fn element_width(self) -> IndexElementWidth {
        self.element_width
    }

    /// Reads a contiguous range of raw indices from sparse physical memory.
    ///
    /// physical_base must already be a decoded byte address. The returned
    /// values are not adjusted by the separate vertex-offset register.
    pub fn read_indices(
        self,
        memory: &GpuPhysicalMemory,
        physical_base: u32,
        first_index: u32,
        count: usize,
    ) -> Result<Vec<u32>, IndexBufferError> {
        if count > MAX_INDICES_PER_READ {
            return Err(IndexBufferError::TooManyIndices { count });
        }
        if count == 0 {
            return Ok(Vec::new());
        }

        let width = self.element_width.byte_width();
        let first_offset = u64::from(first_index).checked_mul(width as u64).ok_or(
            IndexBufferError::AddressOverflow {
                base: physical_base,
                offset: u64::MAX,
            },
        )?;
        let relative_offset = u64::from(self.byte_offset)
            .checked_add(first_offset)
            .ok_or(IndexBufferError::AddressOverflow {
                base: physical_base,
                offset: u64::MAX,
            })?;
        let absolute_address = u64::from(physical_base)
            .checked_add(relative_offset)
            .ok_or(IndexBufferError::AddressOverflow {
                base: physical_base,
                offset: relative_offset,
            })?;
        let address =
            u32::try_from(absolute_address).map_err(|_| IndexBufferError::AddressOverflow {
                base: physical_base,
                offset: relative_offset,
            })?;
        let byte_count = count
            .checked_mul(width)
            .ok_or(IndexBufferError::AddressOverflow {
                base: physical_base,
                offset: relative_offset,
            })?;

        let mut bytes = vec![0u8; byte_count];
        memory.read(address, &mut bytes)?;

        let indices = match self.element_width {
            IndexElementWidth::UnsignedByte => bytes.into_iter().map(u32::from).collect(),
            IndexElementWidth::UnsignedShort => bytes
                .chunks_exact(2)
                .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
                .collect(),
        };
        Ok(indices)
    }
}

/// Failure while reading a PICA200 index buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexBufferError {
    TooManyIndices { count: usize },
    AddressOverflow { base: u32, offset: u64 },
    PhysicalMemory(GpuPhysicalMemoryError),
}

impl std::fmt::Display for IndexBufferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for IndexBufferError {}

impl From<GpuPhysicalMemoryError> for IndexBufferError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::FCRAM_BASE;

    #[test]
    fn decodes_index_buffer_offset_and_width() {
        let byte = IndexBufferLayout::from_register(0x0000_0120);
        assert_eq!(byte.byte_offset(), 0x120);
        assert_eq!(byte.element_width(), IndexElementWidth::UnsignedByte);

        let halfword = IndexBufferLayout::from_register(0x8000_0450);
        assert_eq!(halfword.byte_offset(), 0x450);
        assert_eq!(halfword.element_width(), IndexElementWidth::UnsignedShort);
    }

    #[test]
    fn reads_unsigned_byte_indices_and_applies_first_index_as_byte_offset() {
        let mut memory = GpuPhysicalMemory::new();
        let base = FCRAM_BASE + 0x1000;
        memory.write(base + 0x20, &[0, 2, 7, 11, 13, 0]).unwrap();

        let layout = IndexBufferLayout::from_register(0x20);
        assert_eq!(
            layout.read_indices(&memory, base, 1, 3).unwrap(),
            vec![2, 7, 11]
        );
    }

    #[test]
    fn reads_unsigned_short_indices_as_little_endian_values() {
        let mut memory = GpuPhysicalMemory::new();
        let base = FCRAM_BASE + 0x2000;
        memory
            .write(
                base + 0x40,
                &[0x00, 0x00, 0x34, 0x12, 0x78, 0x56, 0xBC, 0x9A],
            )
            .unwrap();

        let layout = IndexBufferLayout::from_register(0x8000_0040);
        assert_eq!(
            layout.read_indices(&memory, base, 1, 3).unwrap(),
            vec![0x1234, 0x5678, 0x9ABC]
        );
    }

    #[test]
    fn empty_index_range_does_not_read_memory() {
        let memory = GpuPhysicalMemory::new();
        let layout = IndexBufferLayout::from_register(0x20);
        assert_eq!(
            layout.read_indices(&memory, 0, 0, 0).unwrap(),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn rejects_index_ranges_outside_mapped_physical_memory() {
        let memory = GpuPhysicalMemory::new();
        let layout = IndexBufferLayout::from_register(0);
        assert!(matches!(
            layout.read_indices(&memory, 0x1000, 0, 1),
            Err(IndexBufferError::PhysicalMemory(
                GpuPhysicalMemoryError::UnmappedAddress { .. }
            ))
        ));
    }

    #[test]
    fn rejects_unbounded_index_counts_before_allocating() {
        let memory = GpuPhysicalMemory::new();
        let layout = IndexBufferLayout::from_register(0);
        assert_eq!(
            layout.read_indices(&memory, FCRAM_BASE, 0, MAX_INDICES_PER_READ + 1),
            Err(IndexBufferError::TooManyIndices {
                count: MAX_INDICES_PER_READ + 1
            })
        );
    }
}
