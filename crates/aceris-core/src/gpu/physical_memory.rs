//! Sparse backing for ARM11 physical RAM regions used by devices.

use std::collections::BTreeMap;
use std::fmt;

/// ARM11 physical base of the two VRAM banks.
pub const VRAM_BASE: u32 = 0x1800_0000;
/// Combined VRAM size on the original 3DS.
pub const VRAM_SIZE: u32 = 0x0060_0000;
/// ARM11 physical base of original-model FCRAM.
pub const FCRAM_BASE: u32 = 0x2000_0000;
/// Original-model FCRAM size.
pub const FCRAM_SIZE: u32 = 0x0800_0000;
/// Physical page size used for sparse backing.
pub const PHYSICAL_PAGE_SIZE: usize = 0x1000;

const ADDRESS_SPACE_END: u64 = 0x1_0000_0000;

/// Failure while accessing a physical region that is not currently mapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuPhysicalMemoryError {
    UnmappedAddress { address: u32, length: usize },
    AddressRangeOverflow { address: u32, length: usize },
}

impl fmt::Display for GpuPhysicalMemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnmappedAddress { address, length } => write!(
                formatter,
                "physical range {address:#010x}..+{length:#x} is outside VRAM/FCRAM"
            ),
            Self::AddressRangeOverflow { address, length } => write!(
                formatter,
                "physical range {address:#010x}..+{length:#x} overflows the 32-bit address space"
            ),
        }
    }
}

impl std::error::Error for GpuPhysicalMemoryError {}

/// Sparse physical backing visible to the GPU and display controllers.
///
/// The original 3DS address map exposes 6 MiB of VRAM at 0x18000000 and
/// 128 MiB of FCRAM at 0x20000000. Backing pages are allocated on first write;
/// reading an unallocated page returns zero without reserving host memory.
///
/// This is a physical-device memory model only. Virtual-to-physical process
/// mappings and the New 3DS 128 MiB FCRAM extension are separate future work.
#[derive(Clone, Debug, Default)]
pub struct GpuPhysicalMemory {
    pages: BTreeMap<u32, Box<[u8; PHYSICAL_PAGE_SIZE]>>,
}

impl GpuPhysicalMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of 4 KiB backing pages currently materialized.
    pub fn allocated_page_count(&self) -> usize {
        self.pages.len()
    }

    /// Checks that the complete range belongs to one supported physical region.
    pub fn validate_range(
        &self,
        address: u32,
        length: usize,
    ) -> Result<(), GpuPhysicalMemoryError> {
        let end = u64::from(address)
            .checked_add(length as u64)
            .ok_or(GpuPhysicalMemoryError::AddressRangeOverflow { address, length })?;
        if end > ADDRESS_SPACE_END {
            return Err(GpuPhysicalMemoryError::AddressRangeOverflow { address, length });
        }

        let start = u64::from(address);
        let vram_end = u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
        let fcram_end = u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE);
        let in_vram = start >= u64::from(VRAM_BASE) && end <= vram_end;
        let in_fcram = start >= u64::from(FCRAM_BASE) && end <= fcram_end;
        if in_vram || in_fcram {
            Ok(())
        } else {
            Err(GpuPhysicalMemoryError::UnmappedAddress { address, length })
        }
    }

    /// Reads a physical byte range. Unallocated backing pages read as zero.
    pub fn read(&self, address: u32, output: &mut [u8]) -> Result<(), GpuPhysicalMemoryError> {
        self.validate_range(address, output.len())?;
        let mut copied = 0usize;
        while copied < output.len() {
            let current = address + copied as u32;
            let page_number = current / PHYSICAL_PAGE_SIZE as u32;
            let page_offset = current as usize % PHYSICAL_PAGE_SIZE;
            let count = (PHYSICAL_PAGE_SIZE - page_offset).min(output.len() - copied);
            if let Some(page) = self.pages.get(&page_number) {
                output[copied..copied + count]
                    .copy_from_slice(&page[page_offset..page_offset + count]);
            } else {
                output[copied..copied + count].fill(0);
            }
            copied += count;
        }
        Ok(())
    }

    /// Writes a physical byte range, allocating only the touched pages.
    pub fn write(&mut self, address: u32, input: &[u8]) -> Result<(), GpuPhysicalMemoryError> {
        self.validate_range(address, input.len())?;
        let mut copied = 0usize;
        while copied < input.len() {
            let current = address + copied as u32;
            let page_number = current / PHYSICAL_PAGE_SIZE as u32;
            let page_offset = current as usize % PHYSICAL_PAGE_SIZE;
            let count = (PHYSICAL_PAGE_SIZE - page_offset).min(input.len() - copied);
            let page = self
                .pages
                .entry(page_number)
                .or_insert_with(|| Box::new([0; PHYSICAL_PAGE_SIZE]));
            page[page_offset..page_offset + count].copy_from_slice(&input[copied..copied + count]);
            copied += count;
        }
        Ok(())
    }

    pub fn read_u32(&self, address: u32) -> Result<u32, GpuPhysicalMemoryError> {
        let mut bytes = [0u8; 4];
        self.read(address, &mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn write_u32(&mut self, address: u32, value: u32) -> Result<(), GpuPhysicalMemoryError> {
        self.write(address, &value.to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unallocated_physical_pages_read_as_zero_without_allocating() {
        let memory = GpuPhysicalMemory::new();
        let mut bytes = [0xFF; 16];
        memory.read(FCRAM_BASE + 0x1000, &mut bytes).unwrap();

        assert_eq!(bytes, [0; 16]);
        assert_eq!(memory.allocated_page_count(), 0);
    }

    #[test]
    fn vram_and_fcram_ranges_are_sparse_and_little_endian() {
        let mut memory = GpuPhysicalMemory::new();
        memory.write_u32(VRAM_BASE + 0x1FFC, 0xDEAD_BEEF).unwrap();
        memory.write_u32(FCRAM_BASE + 0x2000, 0x1234_5678).unwrap();

        assert_eq!(memory.read_u32(VRAM_BASE + 0x1FFC).unwrap(), 0xDEAD_BEEF);
        assert_eq!(memory.read_u32(FCRAM_BASE + 0x2000).unwrap(), 0x1234_5678);
        assert_eq!(memory.allocated_page_count(), 2);
    }

    #[test]
    fn invalid_ranges_are_rejected_before_any_page_is_allocated() {
        let mut memory = GpuPhysicalMemory::new();
        let error = memory.write_u32(0x1900_0000, 1).unwrap_err();
        assert!(matches!(
            error,
            GpuPhysicalMemoryError::UnmappedAddress { .. }
        ));
        assert_eq!(memory.allocated_page_count(), 0);

        assert!(matches!(
            memory.validate_range(u32::MAX - 1, 4),
            Err(GpuPhysicalMemoryError::AddressRangeOverflow { .. })
        ));
        assert_eq!(memory.allocated_page_count(), 0);
    }

    #[test]
    fn a_range_cannot_cross_between_disjoint_physical_regions() {
        let mut memory = GpuPhysicalMemory::new();
        let address = VRAM_BASE + VRAM_SIZE - 2;
        assert!(memory.write(address, &[1, 2, 3, 4]).is_err());
        assert_eq!(memory.allocated_page_count(), 0);
    }
}
