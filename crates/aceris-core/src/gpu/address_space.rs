//! Explicit GPU-visible linear-memory address translation.
//!
//! PICA200 consumes physical addresses. User processes normally submit linear
//! virtual addresses to GSP, which translates them to FCRAM physical addresses
//! before programming device registers. This table records only mappings that
//! the caller has independently confirmed were allocated by the process.

use std::fmt;

use super::physical_memory::{
    FCRAM_BASE, FCRAM_SIZE, GpuPhysicalMemory, GpuPhysicalMemoryError,
};

/// Original 3DS linear-memory virtual window.
pub const OLD_LINEAR_BASE: u32 = 0x1400_0000;
/// Exclusive end of the original 3DS linear-memory virtual window.
pub const OLD_LINEAR_END: u64 = 0x1C00_0000;
/// Newer linear-memory virtual window used by newer system software.
pub const NEW_LINEAR_BASE: u32 = 0x3000_0000;
/// Exclusive end of the original-model portion of the newer linear window.
pub const NEW_LINEAR_END: u64 = 0x3800_0000;
/// Page granularity for ControlMemory linear mappings.
pub const LINEAR_PAGE_SIZE: u32 = 0x1000;

const ADDRESS_SPACE_END: u64 = 0x1_0000_0000;
const FCRAM_END: u64 = FCRAM_BASE as u64 + FCRAM_SIZE as u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuLinearMapping {
    virtual_base: u32,
    physical_base: u32,
    size: u32,
}

impl GpuLinearMapping {
    pub const fn virtual_base(self) -> u32 {
        self.virtual_base
    }

    pub const fn physical_base(self) -> u32 {
        self.physical_base
    }

    pub const fn size(self) -> u32 {
        self.size
    }

    pub const fn virtual_end(self) -> u64 {
        self.virtual_base as u64 + self.size as u64
    }

    pub const fn physical_end(self) -> u64 {
        self.physical_base as u64 + self.size as u64
    }
}

/// One successfully translated DMA range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedGpuRange {
    pub virtual_address: u32,
    pub physical_address: u32,
    pub length: usize,
}

/// Failure to register or translate a GPU-visible linear-memory range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuAddressSpaceError {
    EmptyMapping,
    MisalignedMapping { address: u32, size: u32 },
    OutsideLinearWindow { address: u32, size: u32 },
    AddressOverflow { address: u32, size: usize },
    OverlappingVirtualMapping,
    OverlappingPhysicalMapping,
    MappingNotFound { address: u32, size: u32 },
    UnmappedVirtualRange { address: u32, size: usize },
    PhysicalMemory(GpuPhysicalMemoryError),
}

impl fmt::Display for GpuAddressSpaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GpuAddressSpaceError {}

impl From<GpuPhysicalMemoryError> for GpuAddressSpaceError {
    fn from(error: GpuPhysicalMemoryError) -> Self {
        Self::PhysicalMemory(error)
    }
}

/// Maps explicitly registered 3DS linear virtual ranges to FCRAM physical RAM.
///
/// The original 3DS mapping is VA + 0x0C000000. The newer 0x30000000 mapping
/// aliases FCRAM starting at FCRAM_BASE. This table does not allocate guest
/// memory and does not infer whether a process owns a range: callers must
/// register a range only after the corresponding guest memory operation succeeds.
#[derive(Clone, Debug, Default)]
pub struct GpuDmaAddressSpace {
    mappings: Vec<GpuLinearMapping>,
}

impl GpuDmaAddressSpace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mappings(&self) -> &[GpuLinearMapping] {
        &self.mappings
    }

    /// Registers one page-aligned, successfully allocated linear-memory range.
    pub fn map_linear_region(
        &mut self,
        virtual_base: u32,
        size: u32,
    ) -> Result<GpuLinearMapping, GpuAddressSpaceError> {
        if size == 0 {
            return Err(GpuAddressSpaceError::EmptyMapping);
        }
        if virtual_base % LINEAR_PAGE_SIZE != 0 || size % LINEAR_PAGE_SIZE != 0 {
            return Err(GpuAddressSpaceError::MisalignedMapping {
                address: virtual_base,
                size,
            });
        }

        let virtual_end = u64::from(virtual_base)
            .checked_add(u64::from(size))
            .ok_or(GpuAddressSpaceError::OutsideLinearWindow {
                address: virtual_base,
                size,
            })?;
        let physical_base = Self::linear_physical_base(virtual_base, size)?;

        if self.mappings.iter().any(|mapping| {
            u64::from(virtual_base) < mapping.virtual_end()
                && virtual_end > u64::from(mapping.virtual_base)
        }) {
            return Err(GpuAddressSpaceError::OverlappingVirtualMapping);
        }

        let physical_end = u64::from(physical_base) + u64::from(size);
        if self.mappings.iter().any(|mapping| {
            u64::from(physical_base) < mapping.physical_end()
                && physical_end > u64::from(mapping.physical_base)
        }) {
            return Err(GpuAddressSpaceError::OverlappingPhysicalMapping);
        }

        let mapping = GpuLinearMapping {
            virtual_base,
            physical_base,
            size,
        };
        self.mappings.push(mapping);
        self.mappings.sort_by_key(|item| item.virtual_base);
        Ok(mapping)
    }

    /// Removes a mapping only when both its virtual base and size match exactly.
    pub fn unmap_linear_region(
        &mut self,
        virtual_base: u32,
        size: u32,
    ) -> Result<(), GpuAddressSpaceError> {
        let Some(index) = self
            .mappings
            .iter()
            .position(|mapping| mapping.virtual_base == virtual_base && mapping.size == size)
        else {
            return Err(GpuAddressSpaceError::MappingNotFound {
                address: virtual_base,
                size,
            });
        };
        self.mappings.remove(index);
        Ok(())
    }

    /// Translates and validates a complete byte range for a GPU DMA read/write.
    ///
    /// The requested range must be wholly contained in one explicitly
    /// registered mapping and in the physical FCRAM backing.
    pub fn resolve_physical_range(
        &self,
        memory: &GpuPhysicalMemory,
        virtual_address: u32,
        length: usize,
    ) -> Result<ResolvedGpuRange, GpuAddressSpaceError> {
        if length == 0 {
            return Err(GpuAddressSpaceError::UnmappedVirtualRange {
                address: virtual_address,
                size: length,
            });
        }
        let virtual_end = u64::from(virtual_address)
            .checked_add(length as u64)
            .ok_or(GpuAddressSpaceError::AddressOverflow {
                address: virtual_address,
                size: length,
            })?;
        if virtual_end > ADDRESS_SPACE_END {
            return Err(GpuAddressSpaceError::AddressOverflow {
                address: virtual_address,
                size: length,
            });
        }

        let Some(mapping) = self.mappings.iter().find(|mapping| {
            u64::from(virtual_address) >= u64::from(mapping.virtual_base)
                && virtual_end <= mapping.virtual_end()
        }) else {
            return Err(GpuAddressSpaceError::UnmappedVirtualRange {
                address: virtual_address,
                size: length,
            });
        };

        let offset = u64::from(virtual_address) - u64::from(mapping.virtual_base);
        let physical = u64::from(mapping.physical_base) + offset;
        let physical_address = u32::try_from(physical).map_err(|_| {
            GpuAddressSpaceError::AddressOverflow {
                address: virtual_address,
                size: length,
            }
        })?;
        memory.validate_range(physical_address, length)?;
        Ok(ResolvedGpuRange {
            virtual_address,
            physical_address,
            length,
        })
    }

    fn linear_physical_base(
        virtual_base: u32,
        size: u32,
    ) -> Result<u32, GpuAddressSpaceError> {
        let start = u64::from(virtual_base);
        let end = start + u64::from(size);
        let physical = if start >= u64::from(OLD_LINEAR_BASE) && end <= OLD_LINEAR_END {
            start + 0x0C00_0000
        } else if start >= u64::from(NEW_LINEAR_BASE) && end <= NEW_LINEAR_END {
            u64::from(FCRAM_BASE) + (start - u64::from(NEW_LINEAR_BASE))
        } else {
            return Err(GpuAddressSpaceError::OutsideLinearWindow {
                address: virtual_base,
                size,
            });
        };
        let physical_end = physical + u64::from(size);
        if physical < u64::from(FCRAM_BASE) || physical_end > FCRAM_END {
            return Err(GpuAddressSpaceError::OutsideLinearWindow {
                address: virtual_base,
                size,
            });
        }
        u32::try_from(physical).map_err(|_| {
            GpuAddressSpaceError::OutsideLinearWindow {
                address: virtual_base,
                size,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_linear_window_translates_to_fcram_with_documented_offset() {
        let mut space = GpuDmaAddressSpace::new();
        let memory = GpuPhysicalMemory::new();
        space.map_linear_region(0x1400_3000, 0x2000).unwrap();

        let resolved = space
            .resolve_physical_range(&memory, 0x1400_3100, 0x40)
            .unwrap();
        assert_eq!(resolved.physical_address, FCRAM_BASE + 0x3100);
        assert_eq!(resolved.length, 0x40);
    }

    #[test]
    fn newer_linear_window_aliases_the_start_of_fcram() {
        let mut space = GpuDmaAddressSpace::new();
        let memory = GpuPhysicalMemory::new();
        space.map_linear_region(0x3000_5000, 0x1000).unwrap();

        let resolved = space
            .resolve_physical_range(&memory, 0x3000_5120, 0x20)
            .unwrap();
        assert_eq!(resolved.physical_address, FCRAM_BASE + 0x5120);
    }

    #[test]
    fn unregistered_ranges_and_ranges_crossing_mapping_edges_are_rejected() {
        let mut space = GpuDmaAddressSpace::new();
        let memory = GpuPhysicalMemory::new();
        space.map_linear_region(0x1400_0000, 0x1000).unwrap();

        assert!(matches!(
            space.resolve_physical_range(&memory, 0x1400_1000, 4),
            Err(GpuAddressSpaceError::UnmappedVirtualRange { .. })
        ));
        assert!(matches!(
            space.resolve_physical_range(&memory, 0x1400_0FFC, 8),
            Err(GpuAddressSpaceError::UnmappedVirtualRange { .. })
        ));
    }

    #[test]
    fn mappings_reject_bad_alignment_and_out_of_window_ranges() {
        let mut space = GpuDmaAddressSpace::new();
        assert!(matches!(
            space.map_linear_region(0x1400_0001, 0x1000),
            Err(GpuAddressSpaceError::MisalignedMapping { .. })
        ));
        assert!(matches!(
            space.map_linear_region(0x1000_0000, 0x1000),
            Err(GpuAddressSpaceError::OutsideLinearWindow { .. })
        ));
    }

    #[test]
    fn physical_aliases_cannot_be_registered_twice() {
        let mut space = GpuDmaAddressSpace::new();
        space.map_linear_region(0x1400_0000, 0x1000).unwrap();

        assert!(matches!(
            space.map_linear_region(0x3000_0000, 0x1000),
            Err(GpuAddressSpaceError::OverlappingPhysicalMapping)
        ));
    }

    #[test]
    fn exact_unmap_allows_a_linear_range_to_be_registered_again() {
        let mut space = GpuDmaAddressSpace::new();
        space.map_linear_region(0x1400_0000, 0x1000).unwrap();
        space.unmap_linear_region(0x1400_0000, 0x1000).unwrap();

        assert_eq!(space.mappings().len(), 0);
        assert!(space.map_linear_region(0x3000_0000, 0x1000).is_ok());
    }
}
