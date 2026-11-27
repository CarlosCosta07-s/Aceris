//! Sparse backing for ARM11 physical RAM regions used by devices.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use crate::save_state::{SaveStateSection, section_id};

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
    InvalidLinearAllocationSize { size: u32 },
    LinearAllocationOutOfMemory { size: u32 },
    LinearAllocationNotFound { address: u32, size: u32 },
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
            Self::InvalidLinearAllocationSize { size } => write!(
                formatter,
                "linear FCRAM allocation size {size:#x} is zero or not page-aligned"
            ),
            Self::LinearAllocationOutOfMemory { size } => write!(
                formatter,
                "no contiguous FCRAM range is available for {size:#x} bytes"
            ),
            Self::LinearAllocationNotFound { address, size } => write!(
                formatter,
                "linear FCRAM allocation {address:#010x}+{size:#x} does not exist"
            ),
        }
    }
}

impl std::error::Error for GpuPhysicalMemoryError {}

/// Errors while encoding or restoring the sparse GPU-visible physical RAM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuPhysicalMemoryStateError {
    AllocationFailed,
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16),
    UnsupportedFlags(u16),
    TooManyPages(u32),
    TooManyAllocations(u32),
    TooManyExclusiveMonitors(u32),
    InvalidPhysicalPage(u32),
    DuplicatePhysicalPage(u32),
    InvalidLinearAllocation { address: u32, size: u32 },
    OverlappingLinearAllocation { address: u32 },
    DuplicateLinearAllocation(u32),
    InvalidExclusiveGranule(u32),
    DuplicateExclusiveGranule(u32),
    InvalidSection,
    TrailingData,
}

impl fmt::Display for GpuPhysicalMemoryStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationFailed => formatter.write_str("could not allocate physical-memory snapshot"),
            Self::Truncated => formatter.write_str("physical-memory snapshot is truncated"),
            Self::InvalidMagic => formatter.write_str("physical-memory snapshot signature is invalid"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "physical-memory snapshot version {version} is unsupported")
            }
            Self::UnsupportedFlags(flags) => {
                write!(formatter, "physical-memory snapshot flags {flags:#06X} are invalid")
            }
            Self::TooManyPages(count) => {
                write!(formatter, "physical-memory snapshot contains too many pages: {count}")
            }
            Self::TooManyAllocations(count) => {
                write!(formatter, "physical-memory snapshot contains too many linear allocations: {count}")
            }
            Self::TooManyExclusiveMonitors(count) => write!(
                formatter,
                "physical-memory snapshot contains too many exclusive monitors: {count}"
            ),
            Self::InvalidPhysicalPage(page) => {
                write!(formatter, "physical-memory page {page:#010X} is outside VRAM/FCRAM")
            }
            Self::DuplicatePhysicalPage(page) => {
                write!(formatter, "physical-memory page {page:#010X} is duplicated or unordered")
            }
            Self::InvalidLinearAllocation { address, size } => write!(
                formatter,
                "linear FCRAM allocation {address:#010X}+{size:#X} is invalid"
            ),
            Self::OverlappingLinearAllocation { address } => write!(
                formatter,
                "linear FCRAM allocation at {address:#010X} overlaps a previous allocation"
            ),
            Self::DuplicateLinearAllocation(address) => {
                write!(formatter, "linear FCRAM allocation at {address:#010X} is duplicated")
            }
            Self::InvalidExclusiveGranule(address) => {
                write!(formatter, "exclusive-monitor granule {address:#010X} is invalid")
            }
            Self::DuplicateExclusiveGranule(address) => {
                write!(formatter, "exclusive-monitor granule {address:#010X} is duplicated or unordered")
            }
            Self::InvalidSection => formatter.write_str("save-state section is not GPU physical memory v1"),
            Self::TrailingData => formatter.write_str("physical-memory snapshot contains trailing bytes"),
        }
    }
}

impl std::error::Error for GpuPhysicalMemoryStateError {}

const SAVE_STATE_MAGIC: &[u8; 8] = b"ACGPUMEM";
const SAVE_STATE_VERSION: u16 = 1;
const SAVE_STATE_HEADER_SIZE: usize = 24;
const MAX_PHYSICAL_PAGE_COUNT: usize =
    (VRAM_SIZE as usize + FCRAM_SIZE as usize) / PHYSICAL_PAGE_SIZE;
const MAX_LINEAR_ALLOCATION_COUNT: usize = FCRAM_SIZE as usize / PHYSICAL_PAGE_SIZE;
const MAX_EXCLUSIVE_MONITOR_COUNT: usize =
    (VRAM_SIZE as usize + FCRAM_SIZE as usize)
        / crate::memory::EXCLUSIVE_RESERVATION_GRANULE as usize;

/// Sparse physical backing visible to the GPU and display controllers.
///
/// The original 3DS address map exposes 6 MiB of VRAM at 0x18000000 and
/// 128 MiB of FCRAM at 0x20000000. Backing pages are allocated on first write;
/// reading an unallocated page returns zero without reserving host memory.
///
/// Clones share the same physical backing so CPU linear mappings, GPU DMA and
/// LCD scanout observe one coherent byte store. CPU cache behavior is modeled
/// separately and is not implied by this shared backing.
#[derive(Debug)]
struct GpuPhysicalMemoryInner {
    backing_id: u64,
    pages: BTreeMap<u32, Box<[u8; PHYSICAL_PAGE_SIZE]>>,
    /// Exact contiguous FCRAM ranges reserved for ControlMemory linear allocations.
    linear_allocations: BTreeMap<u32, u32>,
    /// Write generation per exclusive-reservation granule, bumped by every
    /// physical write so CPU exclusive monitors observe DMA/GPU/LCD stores.
    exclusive_versions: BTreeMap<u32, u64>,
}

impl Default for GpuPhysicalMemoryInner {
    fn default() -> Self {
        Self {
            backing_id: crate::memory::next_memory_backing_id(),
            pages: BTreeMap::new(),
            linear_allocations: BTreeMap::new(),
            exclusive_versions: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GpuPhysicalMemory {
    inner: Arc<RwLock<GpuPhysicalMemoryInner>>,
}

impl GpuPhysicalMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stable identity shared by every clone of this physical backing.
    pub(crate) fn backing_id(&self) -> u64 {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .backing_id
    }

    /// Number of 4 KiB backing pages currently materialized.
    pub fn allocated_page_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pages
            .len()
    }

    /// Reserves a contiguous, page-aligned physical FCRAM range.
    ///
    /// The allocator skips both pages already materialized by other devices and
    /// all outstanding linear reservations. The returned range reads as zero
    /// until the process or a device writes to it.
    pub fn allocate_linear_fcram(&mut self, size: u32) -> Result<u32, GpuPhysicalMemoryError> {
        if size == 0 || !(size as usize).is_multiple_of(PHYSICAL_PAGE_SIZE) {
            return Err(GpuPhysicalMemoryError::InvalidLinearAllocationSize { size });
        }
        if size > FCRAM_SIZE {
            return Err(GpuPhysicalMemoryError::LinearAllocationOutOfMemory { size });
        }

        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let page_count = size / PHYSICAL_PAGE_SIZE as u32;
        let first_page = FCRAM_BASE / PHYSICAL_PAGE_SIZE as u32;
        let total_pages = FCRAM_SIZE / PHYSICAL_PAGE_SIZE as u32;
        let mut candidate = 0u32;

        while candidate
            .checked_add(page_count)
            .is_some_and(|end| end <= total_pages)
        {
            let physical_page = first_page + candidate;
            let candidate_end = physical_page + page_count;
            let materialized_conflict = inner
                .pages
                .range(physical_page..candidate_end)
                .next()
                .map(|(page, _)| *page);
            let reserved_conflict =
                inner
                    .linear_allocations
                    .iter()
                    .find_map(|(base, allocated_size)| {
                        let reserved_start = *base / PHYSICAL_PAGE_SIZE as u32;
                        let reserved_end =
                            reserved_start + *allocated_size / PHYSICAL_PAGE_SIZE as u32;
                        (physical_page < reserved_end && candidate_end > reserved_start)
                            .then_some(reserved_end)
                    });

            if materialized_conflict.is_none() && reserved_conflict.is_none() {
                let address = physical_page * PHYSICAL_PAGE_SIZE as u32;
                inner.linear_allocations.insert(address, size);
                return Ok(address);
            }

            let next_candidate_page = materialized_conflict
                .map(|page| page + 1)
                .into_iter()
                .chain(reserved_conflict)
                .max()
                .unwrap_or(candidate + 1);
            candidate = next_candidate_page.saturating_sub(first_page);
        }

        Err(GpuPhysicalMemoryError::LinearAllocationOutOfMemory { size })
    }

    /// Reserves a caller-selected contiguous, page-aligned physical FCRAM range.
    ///
    /// This is used when ControlMemory receives a non-zero linear virtual hint.
    /// It never relocates the request: an occupied or non-FCRAM range fails.
    pub fn allocate_linear_fcram_at(
        &mut self,
        address: u32,
        size: u32,
    ) -> Result<u32, GpuPhysicalMemoryError> {
        if size == 0 || !(size as usize).is_multiple_of(PHYSICAL_PAGE_SIZE) {
            return Err(GpuPhysicalMemoryError::InvalidLinearAllocationSize { size });
        }
        if size > FCRAM_SIZE {
            return Err(GpuPhysicalMemoryError::LinearAllocationOutOfMemory { size });
        }
        self.validate_range(address, size as usize)?;
        if address < FCRAM_BASE
            || u64::from(address) + u64::from(size) > u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE)
        {
            return Err(GpuPhysicalMemoryError::LinearAllocationOutOfMemory { size });
        }

        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first_page = address / PHYSICAL_PAGE_SIZE as u32;
        let page_count = size / PHYSICAL_PAGE_SIZE as u32;
        let end_page = first_page + page_count;
        let materialized_conflict = inner.pages.range(first_page..end_page).next().is_some();
        let reserved_conflict = inner
            .linear_allocations
            .iter()
            .any(|(base, allocated_size)| {
                let reserved_start = *base / PHYSICAL_PAGE_SIZE as u32;
                let reserved_end = reserved_start + *allocated_size / PHYSICAL_PAGE_SIZE as u32;
                first_page < reserved_end && end_page > reserved_start
            });
        if materialized_conflict || reserved_conflict {
            return Err(GpuPhysicalMemoryError::LinearAllocationOutOfMemory { size });
        }

        inner.linear_allocations.insert(address, size);
        Ok(address)
    }

    /// Checks whether an exact FCRAM allocation is still reserved.
    pub fn has_linear_fcram_allocation(&self, address: u32, size: u32) -> bool {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .linear_allocations
            .get(&address)
            .is_some_and(|allocated_size| *allocated_size == size)
    }

    /// Releases an exact linear FCRAM reservation and clears its sparse pages.
    pub fn free_linear_fcram(
        &mut self,
        address: u32,
        size: u32,
    ) -> Result<(), GpuPhysicalMemoryError> {
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.linear_allocations.get(&address).copied() != Some(size) {
            return Err(GpuPhysicalMemoryError::LinearAllocationNotFound { address, size });
        }

        let first_page = address / PHYSICAL_PAGE_SIZE as u32;
        let page_count = size / PHYSICAL_PAGE_SIZE as u32;
        let end_page = first_page.saturating_add(page_count);
        inner
            .pages
            .retain(|page, _| *page < first_page || *page >= end_page);
        let end = u64::from(address) + u64::from(size);
        let granule_size = crate::memory::EXCLUSIVE_RESERVATION_GRANULE;
        let mut granule = address & !(granule_size - 1);
        while u64::from(granule) < end {
            if let Some(exclusive) = inner.exclusive_versions.get_mut(&granule) {
                *exclusive = exclusive.wrapping_add(1);
            }
            granule = granule.saturating_add(granule_size);
        }
        inner.linear_allocations.remove(&address);
        Ok(())
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
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut copied = 0usize;
        while copied < output.len() {
            let current = address + copied as u32;
            let page_number = current / PHYSICAL_PAGE_SIZE as u32;
            let page_offset = current as usize % PHYSICAL_PAGE_SIZE;
            let count = (PHYSICAL_PAGE_SIZE - page_offset).min(output.len() - copied);
            if let Some(page) = inner.pages.get(&page_number) {
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
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut copied = 0usize;
        while copied < input.len() {
            let current = address + copied as u32;
            let page_number = current / PHYSICAL_PAGE_SIZE as u32;
            let page_offset = current as usize % PHYSICAL_PAGE_SIZE;
            let count = (PHYSICAL_PAGE_SIZE - page_offset).min(input.len() - copied);
            let page = inner
                .pages
                .entry(page_number)
                .or_insert_with(|| Box::new([0; PHYSICAL_PAGE_SIZE]));
            page[page_offset..page_offset + count].copy_from_slice(&input[copied..copied + count]);
            copied += count;
        }

        if !input.is_empty() {
            let end = u64::from(address) + input.len() as u64;
            let granule = crate::memory::EXCLUSIVE_RESERVATION_GRANULE;
            let mut current = u64::from(address & !(granule - 1));
            while current < end {
                // Only granules some monitor has observed carry a generation;
                // bumping an unobserved granule could never be compared.
                if let Some(version) = inner.exclusive_versions.get_mut(&(current as u32)) {
                    *version = version.wrapping_add(1);
                }
                current += u64::from(granule);
            }
        }
        Ok(())
    }

    /// Generation counter of the exclusive-reservation granule containing `address`.
    ///
    /// It changes whenever any agent writes to that granule through this
    /// shared backing, which is how a CPU local monitor loses its reservation.
    ///
    /// Reading a generation registers the granule as observed; writes only
    /// advance observed granules, which keeps ordinary stores off the map.
    pub fn exclusive_version(&self, address: u32) -> u64 {
        let granule = crate::memory::EXCLUSIVE_RESERVATION_GRANULE;
        let key = address & !(granule - 1);
        if let Some(version) = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exclusive_versions
            .get(&key)
        {
            return *version;
        }
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .exclusive_versions
            .entry(key)
            .or_insert(0)
    }

    /// Encodes sparse physical pages, linear FCRAM reservations and exclusive-monitor generations.
    ///
    /// The host backing identity is intentionally not persisted. Restore updates
    /// the existing shared Arc in place, preserving every clone held by the CPU,
    /// GPU DMA space and LCD controller.
    pub fn encode_save_state(&self) -> Result<Vec<u8>, GpuPhysicalMemoryStateError> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        validate_snapshot_contents(&inner)?;

        let page_count = u32::try_from(inner.pages.len())
            .map_err(|_| GpuPhysicalMemoryStateError::TooManyPages(u32::MAX))?;
        let allocation_count = u32::try_from(inner.linear_allocations.len())
            .map_err(|_| GpuPhysicalMemoryStateError::TooManyAllocations(u32::MAX))?;
        let exclusive_count = u32::try_from(inner.exclusive_versions.len())
            .map_err(|_| GpuPhysicalMemoryStateError::TooManyExclusiveMonitors(u32::MAX))?;

        let page_bytes = inner
            .pages
            .len()
            .checked_mul(4 + PHYSICAL_PAGE_SIZE)
            .ok_or(GpuPhysicalMemoryStateError::AllocationFailed)?;
        let allocation_bytes = inner
            .linear_allocations
            .len()
            .checked_mul(8)
            .ok_or(GpuPhysicalMemoryStateError::AllocationFailed)?;
        let exclusive_bytes = inner
            .exclusive_versions
            .len()
            .checked_mul(12)
            .ok_or(GpuPhysicalMemoryStateError::AllocationFailed)?;
        let capacity = SAVE_STATE_HEADER_SIZE
            .checked_add(page_bytes)
            .and_then(|size| size.checked_add(allocation_bytes))
            .and_then(|size| size.checked_add(exclusive_bytes))
            .ok_or(GpuPhysicalMemoryStateError::AllocationFailed)?;

        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(capacity)
            .map_err(|_| GpuPhysicalMemoryStateError::AllocationFailed)?;
        encoded.extend_from_slice(SAVE_STATE_MAGIC);
        encoded.extend_from_slice(&SAVE_STATE_VERSION.to_le_bytes());
        encoded.extend_from_slice(&0u16.to_le_bytes());
        encoded.extend_from_slice(&page_count.to_le_bytes());
        encoded.extend_from_slice(&allocation_count.to_le_bytes());
        encoded.extend_from_slice(&exclusive_count.to_le_bytes());

        for (page_number, page) in &inner.pages {
            encoded.extend_from_slice(&page_number.to_le_bytes());
            encoded.extend_from_slice(page.as_slice());
        }
        for (address, size) in &inner.linear_allocations {
            encoded.extend_from_slice(&address.to_le_bytes());
            encoded.extend_from_slice(&size.to_le_bytes());
        }
        for (address, version) in &inner.exclusive_versions {
            encoded.extend_from_slice(&address.to_le_bytes());
            encoded.extend_from_slice(&version.to_le_bytes());
        }
        debug_assert_eq!(encoded.len(), capacity);
        Ok(encoded)
    }

    /// Restores the sparse memory backing transactionally while keeping all clones alive.
    ///
    /// The complete replacement maps are parsed and validated before the shared
    /// inner object is changed. A failure leaves every existing alias untouched.
    pub fn restore_save_state(
        &mut self,
        encoded: &[u8],
    ) -> Result<(), GpuPhysicalMemoryStateError> {
        let backing_id = self.backing_id();
        let mut restored = decode_snapshot(encoded, backing_id)?;
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Retain this identity because address-space and DMA cache tags may hold it.
        restored.backing_id = inner.backing_id;
        *inner = restored;
        Ok(())
    }

    /// Wraps this snapshot in the GPU_PHYSICAL_MEMORY section of ACRPAYLD.
    pub fn encode_save_state_section(
        &self,
    ) -> Result<SaveStateSection, GpuPhysicalMemoryStateError> {
        let payload = self.encode_save_state()?;
        SaveStateSection::new(section_id::GPU_PHYSICAL_MEMORY, SAVE_STATE_VERSION, payload)
            .map_err(|_| GpuPhysicalMemoryStateError::InvalidSection)
    }

    /// Restores only a section whose ID and version match this component.
    pub fn restore_save_state_section(
        &mut self,
        section: &SaveStateSection,
    ) -> Result<(), GpuPhysicalMemoryStateError> {
        if section.id() != section_id::GPU_PHYSICAL_MEMORY
            || section.version() != SAVE_STATE_VERSION
        {
            return Err(GpuPhysicalMemoryStateError::InvalidSection);
        }
        self.restore_save_state(section.payload())
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

fn physical_page_is_valid(page_number: u32) -> bool {
    let Some(address) = page_number.checked_mul(PHYSICAL_PAGE_SIZE as u32) else {
        return false;
    };
    let end = u64::from(address) + PHYSICAL_PAGE_SIZE as u64;
    let vram_end = u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
    let fcram_end = u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE);
    (address >= VRAM_BASE && end <= vram_end)
        || (address >= FCRAM_BASE && end <= fcram_end)
}

fn linear_allocation_is_valid(address: u32, size: u32) -> bool {
    size != 0
        && address.is_multiple_of(PHYSICAL_PAGE_SIZE as u32)
        && size.is_multiple_of(PHYSICAL_PAGE_SIZE as u32)
        && address >= FCRAM_BASE
        && u64::from(address) + u64::from(size)
            <= u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE)
}

fn exclusive_granule_is_valid(address: u32) -> bool {
    address.is_multiple_of(crate::memory::EXCLUSIVE_RESERVATION_GRANULE)
        && ((address >= VRAM_BASE
            && u64::from(address) + u64::from(crate::memory::EXCLUSIVE_RESERVATION_GRANULE)
                <= u64::from(VRAM_BASE) + u64::from(VRAM_SIZE))
            || (address >= FCRAM_BASE
                && u64::from(address)
                    + u64::from(crate::memory::EXCLUSIVE_RESERVATION_GRANULE)
                    <= u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE)))
}

fn validate_snapshot_contents(
    inner: &GpuPhysicalMemoryInner,
) -> Result<(), GpuPhysicalMemoryStateError> {
    if inner.pages.len() > MAX_PHYSICAL_PAGE_COUNT {
        return Err(GpuPhysicalMemoryStateError::TooManyPages(
            inner.pages.len() as u32,
        ));
    }
    if inner.linear_allocations.len() > MAX_LINEAR_ALLOCATION_COUNT {
        return Err(GpuPhysicalMemoryStateError::TooManyAllocations(
            inner.linear_allocations.len() as u32,
        ));
    }
    if inner.exclusive_versions.len() > MAX_EXCLUSIVE_MONITOR_COUNT {
        return Err(GpuPhysicalMemoryStateError::TooManyExclusiveMonitors(
            inner.exclusive_versions.len() as u32,
        ));
    }

    for page in inner.pages.keys() {
        if !physical_page_is_valid(*page) {
            return Err(GpuPhysicalMemoryStateError::InvalidPhysicalPage(*page));
        }
    }

    let mut previous_end = None;
    for (address, size) in &inner.linear_allocations {
        if !linear_allocation_is_valid(*address, *size) {
            return Err(GpuPhysicalMemoryStateError::InvalidLinearAllocation {
                address: *address,
                size: *size,
            });
        }
        if previous_end.is_some_and(|end| *address < end) {
            return Err(GpuPhysicalMemoryStateError::OverlappingLinearAllocation {
                address: *address,
            });
        }
        previous_end = address.checked_add(*size);
    }

    for address in inner.exclusive_versions.keys() {
        if !exclusive_granule_is_valid(*address) {
            return Err(GpuPhysicalMemoryStateError::InvalidExclusiveGranule(
                *address,
            ));
        }
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, GpuPhysicalMemoryStateError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, GpuPhysicalMemoryStateError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, GpuPhysicalMemoryStateError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

fn decode_snapshot(
    encoded: &[u8],
    backing_id: u64,
) -> Result<GpuPhysicalMemoryInner, GpuPhysicalMemoryStateError> {
    if encoded.len() < SAVE_STATE_HEADER_SIZE {
        return Err(GpuPhysicalMemoryStateError::Truncated);
    }
    if encoded.get(..8) != Some(SAVE_STATE_MAGIC.as_slice()) {
        return Err(GpuPhysicalMemoryStateError::InvalidMagic);
    }
    let version = read_u16(encoded, 8)?;
    if version != SAVE_STATE_VERSION {
        return Err(GpuPhysicalMemoryStateError::UnsupportedVersion(version));
    }
    let flags = read_u16(encoded, 10)?;
    if flags != 0 {
        return Err(GpuPhysicalMemoryStateError::UnsupportedFlags(flags));
    }
    let page_count = read_u32(encoded, 12)?;
    let allocation_count = read_u32(encoded, 16)?;
    let exclusive_count = read_u32(encoded, 20)?;
    if page_count as usize > MAX_PHYSICAL_PAGE_COUNT {
        return Err(GpuPhysicalMemoryStateError::TooManyPages(page_count));
    }
    if allocation_count as usize > MAX_LINEAR_ALLOCATION_COUNT {
        return Err(GpuPhysicalMemoryStateError::TooManyAllocations(
            allocation_count,
        ));
    }
    if exclusive_count as usize > MAX_EXCLUSIVE_MONITOR_COUNT {
        return Err(GpuPhysicalMemoryStateError::TooManyExclusiveMonitors(
            exclusive_count,
        ));
    }

    let page_bytes = (page_count as usize)
        .checked_mul(4 + PHYSICAL_PAGE_SIZE)
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    let allocation_bytes = (allocation_count as usize)
        .checked_mul(8)
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    let exclusive_bytes = (exclusive_count as usize)
        .checked_mul(12)
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    let expected_length = SAVE_STATE_HEADER_SIZE
        .checked_add(page_bytes)
        .and_then(|length| length.checked_add(allocation_bytes))
        .and_then(|length| length.checked_add(exclusive_bytes))
        .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
    if encoded.len() < expected_length {
        return Err(GpuPhysicalMemoryStateError::Truncated);
    }
    if encoded.len() > expected_length {
        return Err(GpuPhysicalMemoryStateError::TrailingData);
    }

    let mut cursor = SAVE_STATE_HEADER_SIZE;
    let mut pages = BTreeMap::new();
    let mut previous_page = None;
    for _ in 0..page_count {
        let page_number = read_u32(encoded, cursor)?;
        cursor += 4;
        if !physical_page_is_valid(page_number) {
            return Err(GpuPhysicalMemoryStateError::InvalidPhysicalPage(
                page_number,
            ));
        }
        if previous_page.is_some_and(|previous| page_number <= previous) {
            return Err(GpuPhysicalMemoryStateError::DuplicatePhysicalPage(
                page_number,
            ));
        }
        previous_page = Some(page_number);

        let page_bytes = encoded
            .get(cursor..cursor + PHYSICAL_PAGE_SIZE)
            .ok_or(GpuPhysicalMemoryStateError::Truncated)?;
        cursor += PHYSICAL_PAGE_SIZE;
        let mut page = Vec::new();
        page.try_reserve_exact(PHYSICAL_PAGE_SIZE)
            .map_err(|_| GpuPhysicalMemoryStateError::AllocationFailed)?;
        page.extend_from_slice(page_bytes);
        let boxed: Box<[u8; PHYSICAL_PAGE_SIZE]> = page
            .into_boxed_slice()
            .try_into()
            .map_err(|_| GpuPhysicalMemoryStateError::AllocationFailed)?;
        pages.insert(page_number, boxed);
    }

    let mut linear_allocations = BTreeMap::new();
    let mut previous_end = None;
    for _ in 0..allocation_count {
        let address = read_u32(encoded, cursor)?;
        let size = read_u32(encoded, cursor + 4)?;
        cursor += 8;
        if !linear_allocation_is_valid(address, size) {
            return Err(GpuPhysicalMemoryStateError::InvalidLinearAllocation {
                address,
                size,
            });
        }
        if previous_end.is_some_and(|end| address < end) {
            return Err(GpuPhysicalMemoryStateError::OverlappingLinearAllocation {
                address,
            });
        }
        previous_end = address.checked_add(size);
        if linear_allocations.insert(address, size).is_some() {
            return Err(GpuPhysicalMemoryStateError::DuplicateLinearAllocation(
                address,
            ));
        }
    }

    let mut exclusive_versions = BTreeMap::new();
    let mut previous_granule = None;
    for _ in 0..exclusive_count {
        let address = read_u32(encoded, cursor)?;
        let generation = read_u64(encoded, cursor + 4)?;
        cursor += 12;
        if !exclusive_granule_is_valid(address) {
            return Err(GpuPhysicalMemoryStateError::InvalidExclusiveGranule(
                address,
            ));
        }
        if previous_granule.is_some_and(|previous| address <= previous) {
            return Err(GpuPhysicalMemoryStateError::DuplicateExclusiveGranule(
                address,
            ));
        }
        previous_granule = Some(address);
        exclusive_versions.insert(address, generation);
    }

    Ok(GpuPhysicalMemoryInner {
        backing_id,
        pages,
        linear_allocations,
        exclusive_versions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn physical_memory_snapshot_round_trips_sparse_pages_allocations_and_exclusives() {
        let mut original = GpuPhysicalMemory::new();
        let alias = original.clone();
        let fcram_address = FCRAM_BASE + 0x8000;
        let vram_address = VRAM_BASE + 0x3000;
        original.allocate_linear_fcram_at(fcram_address, 0x3000).unwrap();
        original.write(fcram_address + 0x20, &[1, 2, 3, 4, 5]).unwrap();
        original.write(vram_address + 0x10, &[9, 8, 7]).unwrap();
        assert_eq!(original.exclusive_version(fcram_address + 0x20), 0);
        original.write(fcram_address + 0x21, &[0xAA]).unwrap();
        let expected_exclusive = original.exclusive_version(fcram_address + 0x20);
        let encoded = original.encode_save_state().unwrap();

        let mut restored = GpuPhysicalMemory::new();
        let restored_alias = restored.clone();
        restored.restore_save_state(&encoded).unwrap();

        let mut first = [0u8; 5];
        let mut second = [0u8; 3];
        restored.read(fcram_address + 0x20, &mut first).unwrap();
        restored_alias.read(vram_address + 0x10, &mut second).unwrap();
        assert_eq!(first, [1, 0xAA, 3, 4, 5]);
        assert_eq!(second, [9, 8, 7]);
        assert!(restored.has_linear_fcram_allocation(fcram_address, 0x3000));
        assert_eq!(
            restored.exclusive_version(fcram_address + 0x20),
            expected_exclusive
        );
        assert_eq!(restored.allocated_page_count(), 2);
        assert_eq!(restored.encode_save_state().unwrap(), encoded);

        // A clone made before restore sees the new contents through the same Arc.
        let mut alias_read = [0u8; 1];
        restored_alias
            .read(fcram_address + 0x20, &mut alias_read)
            .unwrap();
        assert_eq!(alias_read, [1]);
    }

    #[test]
    fn malformed_physical_memory_snapshot_does_not_change_shared_aliases() {
        let mut memory = GpuPhysicalMemory::new();
        let alias = memory.clone();
        let address = FCRAM_BASE + 0x1000;
        memory.write_u32(address, 0xDEAD_BEEF).unwrap();
        let before = memory.read_u32(address).unwrap();
        let encoded = memory.encode_save_state().unwrap();

        assert!(memory
            .restore_save_state(&encoded[..encoded.len() - 1])
            .is_err());
        assert_eq!(memory.read_u32(address).unwrap(), before);
        assert_eq!(alias.read_u32(address).unwrap(), before);

        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(memory.restore_save_state(&trailing).is_err());
        assert_eq!(memory.read_u32(address).unwrap(), before);

        let mut invalid_flags = encoded;
        invalid_flags[10..12].copy_from_slice(&1u16.to_le_bytes());
        assert!(memory.restore_save_state(&invalid_flags).is_err());
        assert_eq!(memory.read_u32(address).unwrap(), before);
    }

    #[test]
    fn physical_memory_snapshot_rejects_invalid_and_duplicate_page_entries() {
        let mut memory = GpuPhysicalMemory::new();
        memory.write_u32(FCRAM_BASE + 0x1000, 1).unwrap();
        memory.write_u32(FCRAM_BASE + 0x3000, 2).unwrap();
        let encoded = memory.encode_save_state().unwrap();

        let mut invalid_page = encoded.clone();
        invalid_page[SAVE_STATE_HEADER_SIZE..SAVE_STATE_HEADER_SIZE + 4]
            .copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            memory.restore_save_state(&invalid_page),
            Err(GpuPhysicalMemoryStateError::InvalidPhysicalPage(_))
        ));

        let first_page_number = read_u32(&encoded, SAVE_STATE_HEADER_SIZE).unwrap();
        let first_entry_size = 4 + PHYSICAL_PAGE_SIZE;
        let mut duplicate_page = encoded;
        duplicate_page[SAVE_STATE_HEADER_SIZE + first_entry_size
            ..SAVE_STATE_HEADER_SIZE + first_entry_size + 4]
            .copy_from_slice(&first_page_number.to_le_bytes());
        assert!(matches!(
            memory.restore_save_state(&duplicate_page),
            Err(GpuPhysicalMemoryStateError::DuplicatePhysicalPage(_))
        ));
    }

    #[test]
    fn gpu_physical_memory_can_be_wrapped_as_a_versioned_save_state_section() {
        let mut memory = GpuPhysicalMemory::new();
        memory.write_u32(FCRAM_BASE + 0x1000, 0x1234_5678).unwrap();
        let section = memory.encode_save_state_section().unwrap();
        let mut restored = GpuPhysicalMemory::new();
        restored.restore_save_state_section(&section).unwrap();
        assert_eq!(restored.read_u32(FCRAM_BASE + 0x1000).unwrap(), 0x1234_5678);
    }

    #[test]
    fn writes_bump_the_exclusive_version_of_every_touched_granule() {
        let mut memory = GpuPhysicalMemory::new();
        let base = FCRAM_BASE + 0x1000;
        // Observing a granule (as LDREX does) registers it with generation 0.
        assert_eq!(memory.exclusive_version(base), 0);
        assert_eq!(memory.exclusive_version(base + 8), 0);
        assert_eq!(memory.exclusive_version(base + 16), 0);

        memory.write(base + 6, &[1, 2, 3, 4]).unwrap(); // spans two 8-byte granules
        assert_eq!(memory.exclusive_version(base), 1);
        assert_eq!(memory.exclusive_version(base + 7), 1);
        assert_eq!(memory.exclusive_version(base + 8), 1);
        assert_eq!(memory.exclusive_version(base + 16), 0);

        // A clone observes the same counters, and reads never change them.
        let clone = memory.clone();
        let mut byte = [0u8];
        clone.read(base, &mut byte).unwrap();
        assert_eq!(clone.exclusive_version(base), 1);
        memory.write(base, &[9]).unwrap();
        assert_eq!(clone.exclusive_version(base), 2);
    }

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
    fn cloned_physical_memory_handles_share_the_same_backing() {
        let mut writer = GpuPhysicalMemory::new();
        let reader = writer.clone();
        writer.write_u32(FCRAM_BASE + 0x4000, 0xCAFE_BABE).unwrap();
        assert_eq!(reader.read_u32(FCRAM_BASE + 0x4000).unwrap(), 0xCAFE_BABE);
    }

    #[test]
    fn linear_fcram_allocation_is_contiguous_zeroed_and_reusable() {
        let mut memory = GpuPhysicalMemory::new();
        memory.write_u32(FCRAM_BASE, 0xDEAD_BEEF).unwrap();

        let allocation = memory.allocate_linear_fcram(0x3000).unwrap();
        assert_eq!(allocation, FCRAM_BASE + 0x1000);
        let mut bytes = [0xFF; 8];
        memory.read(allocation, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 8]);
        assert!(memory.has_linear_fcram_allocation(allocation, 0x3000));
        assert!(memory.allocate_linear_fcram(0x2000).is_ok());

        memory.write_u32(allocation + 0x1000, 0x1234_5678).unwrap();
        memory.free_linear_fcram(allocation, 0x3000).unwrap();
        let mut cleared = [0xFF; 4];
        memory.read(allocation + 0x1000, &mut cleared).unwrap();
        assert_eq!(cleared, [0; 4]);
        assert_eq!(memory.allocate_linear_fcram(0x3000).unwrap(), allocation);
    }

    #[test]
    fn linear_fcram_allocator_can_reserve_a_requested_physical_range() {
        let mut memory = GpuPhysicalMemory::new();
        let requested = FCRAM_BASE + 0x4000;
        assert_eq!(
            memory.allocate_linear_fcram_at(requested, 0x2000).unwrap(),
            requested
        );
        assert!(matches!(
            memory.allocate_linear_fcram_at(requested, 0x1000),
            Err(GpuPhysicalMemoryError::LinearAllocationOutOfMemory { .. })
        ));
        assert!(matches!(
            memory.allocate_linear_fcram_at(FCRAM_BASE - 0x1000, 0x1000),
            Err(GpuPhysicalMemoryError::UnmappedAddress { .. })
        ));
    }

    #[test]
    fn linear_fcram_allocator_rejects_bad_sizes_and_exact_free_mismatches() {
        let mut memory = GpuPhysicalMemory::new();
        assert!(matches!(
            memory.allocate_linear_fcram(0),
            Err(GpuPhysicalMemoryError::InvalidLinearAllocationSize { .. })
        ));
        assert!(matches!(
            memory.allocate_linear_fcram(0x1001),
            Err(GpuPhysicalMemoryError::InvalidLinearAllocationSize { .. })
        ));

        let allocation = memory.allocate_linear_fcram(0x1000).unwrap();
        assert!(matches!(
            memory.free_linear_fcram(allocation, 0x2000),
            Err(GpuPhysicalMemoryError::LinearAllocationNotFound { .. })
        ));
        assert!(memory.has_linear_fcram_allocation(allocation, 0x1000));
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
