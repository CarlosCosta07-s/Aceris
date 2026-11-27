//! ARM11 process-layout validation for prepared Nintendo 3DS executable images.
//!
//! This module converts parsed code-set metadata into validated virtual
//! mappings for code, BSS, and the main-thread stack. It does not allocate
//! physical memory, configure privileged kernel state, schedule threads, or
//! start a complete console execution environment.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};

use super::PreparedArm11Image;
use super::ncch::extended_header::{Arm11MemoryType, CodeFileLayout, CodeSetInfo};
use super::ncch::service_access::ServiceAccessControl;
use crate::cpu::{
    Arm11Core, MPCORE_CPU_INTERFACE_CPU0_OFFSET, MPCORE_CPU_INTERFACE_CURRENT_OFFSET,
    MPCORE_GIC_CPU_INTERFACE_SIZE, MPCORE_GIC_DISTRIBUTOR_OFFSET, MPCORE_GIC_DISTRIBUTOR_SIZE,
    MPCORE_PRIVATE_TIMER_CPU0_ALIAS_OFFSET, MPCORE_PRIVATE_TIMER_OFFSET,
    MPCORE_PRIVATE_TIMER_REGISTER_SIZE, MPCORE_TIMER_DEFAULT_CLOCK_HZ, MpcoreInterruptController,
    MpcorePrivateTimer, MpcoreScu, SP,
};
use crate::gpu::{
    FCRAM_BASE, FCRAM_SIZE, GpuPhysicalMemory, NEW_LINEAR_BASE, NEW_LINEAR_END, OLD_LINEAR_BASE,
    OLD_LINEAR_END,
};
use crate::loader::threedsx::process::{ThreeDsxPreparedProcess, ThreeDsxProcessMemorySegment};
use crate::memory::{Bus, CachePolicy, ImmutableCodeLocation};
use aceris_common::{Error, Result as BusResult};

const PAGE_SIZE: u64 = 0x1000;
const SHARED_BACKING_PAGE_SIZE: usize = PAGE_SIZE as usize;
const ARM11_DATA_CACHE_LINE_SIZE: usize = 32;
const ARM11_DATA_CACHE_CAPACITY_BYTES: usize = 16 * 1024;
const ARM11_DATA_CACHE_WAYS: usize = 4;
const ARM11_DATA_CACHE_SET_COUNT: usize =
    ARM11_DATA_CACHE_CAPACITY_BYTES / ARM11_DATA_CACHE_LINE_SIZE / ARM11_DATA_CACHE_WAYS;
// Bounded software queue for dirty D-cache line writebacks. This is a
// deterministic back-pressure model, not a claim about undocumented silicon depth.
const ARM11_WRITE_BUFFER_CAPACITY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DataCacheKey {
    backing_id: u64,
    line_offset: u64,
}

/// Resident D-cache lines grouped by set. Each set holds at most
/// `ARM11_DATA_CACHE_WAYS` lines (LRU eviction keeps that bound), so a lookup
/// is a short linear scan instead of an ordered-tree search.
#[derive(Debug)]
struct DataCacheLines {
    sets: Vec<Vec<(DataCacheKey, Arm11DataCacheLine)>>,
    len: usize,
}

impl DataCacheLines {
    fn new() -> Self {
        Self {
            sets: (0..ARM11_DATA_CACHE_SET_COUNT)
                .map(|_| Vec::with_capacity(ARM11_DATA_CACHE_WAYS))
                .collect(),
            len: 0,
        }
    }

    #[inline]
    fn set(&self, key: &DataCacheKey) -> &[(DataCacheKey, Arm11DataCacheLine)] {
        &self.sets[Arm11CodeAddressSpace::cache_set_index(*key)]
    }

    #[inline]
    fn get(&self, key: &DataCacheKey) -> Option<&Arm11DataCacheLine> {
        self.set(key)
            .iter()
            .find(|(resident, _)| resident == key)
            .map(|(_, line)| line)
    }

    #[inline]
    fn get_mut(&mut self, key: &DataCacheKey) -> Option<&mut Arm11DataCacheLine> {
        self.sets[Arm11CodeAddressSpace::cache_set_index(*key)]
            .iter_mut()
            .find(|(resident, _)| resident == key)
            .map(|(_, line)| line)
    }

    #[cfg(test)]
    fn contains_key(&self, key: &DataCacheKey) -> bool {
        self.get(key).is_some()
    }

    fn insert(
        &mut self,
        key: DataCacheKey,
        line: Arm11DataCacheLine,
    ) -> Option<Arm11DataCacheLine> {
        if let Some(existing) = self.get_mut(&key) {
            return Some(std::mem::replace(existing, line));
        }
        self.sets[Arm11CodeAddressSpace::cache_set_index(key)].push((key, line));
        self.len += 1;
        None
    }

    fn remove(&mut self, key: &DataCacheKey) -> Option<Arm11DataCacheLine> {
        let set = &mut self.sets[Arm11CodeAddressSpace::cache_set_index(*key)];
        let position = set.iter().position(|(resident, _)| resident == key)?;
        self.len -= 1;
        Some(set.swap_remove(position).1)
    }

    fn len(&self) -> usize {
        self.len
    }

    fn keys(&self) -> impl Iterator<Item = &DataCacheKey> {
        self.sets.iter().flatten().map(|(key, _)| key)
    }

    /// Resident keys within an inclusive (backing, line offset) span, unordered.
    fn keys_in(
        &self,
        range: std::ops::RangeInclusive<DataCacheKey>,
    ) -> impl Iterator<Item = &DataCacheKey> {
        self.keys().filter(move |key| range.contains(key))
    }
}

#[cfg(test)]
impl std::ops::Index<&DataCacheKey> for DataCacheLines {
    type Output = Arm11DataCacheLine;

    fn index(&self, key: &DataCacheKey) -> &Arm11DataCacheLine {
        self.get(key).expect("resident D-cache line")
    }
}

#[derive(Clone, Debug)]
enum DataCacheTarget {
    Static {
        kind: Arm11SegmentKind,
        offset: usize,
    },
    Backing {
        backing: SharedMemoryBacking,
        offset: usize,
    },
    Physical {
        memory: GpuPhysicalMemory,
        address: u32,
    },
}

#[derive(Clone, Debug)]
struct Arm11DataCacheLine {
    bytes: [u8; ARM11_DATA_CACHE_LINE_SIZE],
    dirty: bool,
    target: DataCacheTarget,
}

#[derive(Clone, Debug)]
struct Arm11WriteBufferEntry {
    key: DataCacheKey,
    target: DataCacheTarget,
    /// Byte offset within the aligned 32-byte line.
    start: u8,
    bytes: [u8; ARM11_DATA_CACHE_LINE_SIZE],
    /// Number of valid bytes; cache-line writebacks use the full 32 bytes.
    length: u8,
}
const ADDRESS_SPACE_END: u64 = 0x1_0000_0000;
const MAIN_STACK_TOP: u32 = 0x1000_0000;
const APPLICATION_HEAP_BASE: u32 = 0x0800_0000;
const APPLICATION_HEAP_END: u64 = 0x0E00_0000;
/// Base of the per-thread local storage window and the size of one slot.
const THREAD_LOCAL_STORAGE_BASE: u32 = 0x1FF8_2000;
const IPC_TEMPORARY_BUFFER_BASE: u32 = 0x0400_0000;
const IPC_TEMPORARY_BUFFER_END: u64 = 0x0800_0000;
const THREAD_LOCAL_STORAGE_SLOT_SIZE: u32 = 0x200;
const THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE: u32 = 8;
const THREAD_LOCAL_STATIC_BUFFER_TABLE_OFFSET: u32 = 0x180;
const THREAD_LOCAL_STATIC_BUFFER_DESCRIPTOR_SIZE: u32 = 8;
const THREAD_LOCAL_STATIC_BUFFER_COUNT: u8 = 16;
/// This model supports up to 512 simultaneous threads' worth of storage.
const THREAD_LOCAL_STORAGE_MAX_PAGES: u32 = 64;
const MAX_BSS_MAPPING_SIZE_BYTES: usize = 256 * 1024 * 1024;

/// ARM11 code segment category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm11SegmentKind {
    /// Executable, read-only text pages.
    Text,
    /// Read-only, non-executable constants.
    ReadOnly,
    /// Writable, non-executable initialized data.
    Data,
    /// Writable, non-executable zero-initialized data.
    Bss,
    /// Writable, non-executable main-thread stack.
    Stack,
}

/// Errors while reserving or releasing a process application-heap mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arm11DynamicMemoryError {
    /// Address is not page-aligned.
    MisalignedAddress(u32),
    /// Size is zero or not page-aligned.
    InvalidSize(u32),
    /// Requested mapping lies outside the supported application heap window.
    OutsideApplicationHeap { address: u32, size: u32 },
    /// Requested virtual range overlaps an existing process mapping.
    OverlappingMapping,
    /// No dynamic mapping exactly matches the requested address and size.
    MappingNotFound { address: u32, size: u32 },
    /// The host could not reserve backing memory for the guest mapping.
    AllocationFailure(String),
    /// Linear memory is outside the supported virtual-to-physical windows.
    OutsideLinearMemoryRange { address: u32, size: u32 },
    /// No shared physical-memory backing was attached to this process.
    PhysicalMemoryUnavailable,
    /// Pending D-cache writeback could not be committed before unmapping memory.
    WritebackFailure(String),
    /// The exact linear mapping is not present in the process address space.
    LinearMappingNotFound { address: u32, size: u32 },
    /// A virtual shared-memory mapping would wrap the process address space.
    AddressRangeOverflow { address: u32, size: u32 },
    /// A shared-memory permission value is outside the supported R/W range.
    InvalidPermissions(u32),
}

impl fmt::Display for Arm11DynamicMemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MisalignedAddress(address) => {
                write!(
                    formatter,
                    "application heap address {address:#010x} is not page-aligned"
                )
            }
            Self::InvalidSize(size) => {
                write!(
                    formatter,
                    "application heap size {size:#010x} is zero or not page-aligned"
                )
            }
            Self::OutsideApplicationHeap { address, size } => {
                write!(
                    formatter,
                    "application heap range {address:#010x}+{size:#x} is outside the supported heap window"
                )
            }
            Self::OverlappingMapping => {
                formatter.write_str("application heap mapping overlaps existing memory")
            }
            Self::MappingNotFound { address, size } => {
                write!(
                    formatter,
                    "application heap mapping {address:#010x}+{size:#x} does not exist"
                )
            }
            Self::AllocationFailure(reason) => {
                write!(
                    formatter,
                    "could not allocate application heap backing: {reason}"
                )
            }
            Self::OutsideLinearMemoryRange { address, size } => write!(
                formatter,
                "linear mapping {address:#010x}+{size:#x} is outside supported linear memory"
            ),
            Self::PhysicalMemoryUnavailable => {
                formatter.write_str("process has no shared physical-memory backing")
            }
            Self::WritebackFailure(reason) => {
                write!(
                    formatter,
                    "could not drain the ARM11 write buffer: {reason}"
                )
            }
            Self::LinearMappingNotFound { address, size } => write!(
                formatter,
                "linear mapping {address:#010x}+{size:#x} does not exist"
            ),
            Self::AddressRangeOverflow { address, size } => write!(
                formatter,
                "shared-memory mapping {address:#010x}+{size:#x} wraps the process address space"
            ),
            Self::InvalidPermissions(permissions) => write!(
                formatter,
                "shared-memory permissions {permissions:#x} are invalid"
            ),
        }
    }
}

impl std::error::Error for Arm11DynamicMemoryError {}

/// Sparse host backing shared by one or more ARM11 virtual mappings.
///
/// The map stores only materialized 4 KiB pages. Unallocated bytes are
/// architecturally zero; the first write to a page creates its host backing.
#[derive(Debug, Default)]
struct SparseSharedMemoryStore {
    pages: BTreeMap<usize, Vec<u8>>,
}

/// Shared page backing that can be mapped into several ARM11 address spaces
/// without copying. Zeroed blocks reserve only an address range; host pages
/// are materialized when a CPU or HLE agent first writes to them.
#[derive(Clone, Debug)]
pub struct SharedMemoryBacking {
    bytes: Arc<Mutex<SparseSharedMemoryStore>>,
    offset: usize,
    length: usize,
    /// Logical size of the root allocation; views retain this value.
    root_length: usize,
    backing_id: u64,
    exclusive_versions: Arc<Mutex<BTreeMap<u32, u64>>>,
}

impl SharedMemoryBacking {
    /// Creates a zero-filled backing without allocating its byte range.
    pub fn zeroed(size: usize) -> std::result::Result<Self, String> {
        Ok(Self {
            bytes: Arc::new(Mutex::new(SparseSharedMemoryStore::default())),
            offset: 0,
            length: size,
            root_length: size,
            backing_id: crate::memory::next_memory_backing_id(),
            exclusive_versions: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Full logical length of the root allocation, including unmaterialized pages.
    pub(crate) fn root_length(&self) -> usize {
        self.root_length
    }

    /// Returns a full-range handle to this view's root allocation.
    pub(crate) fn root_view(&self) -> Self {
        Self {
            bytes: self.bytes.clone(),
            offset: 0,
            length: self.root_length,
            root_length: self.root_length,
            backing_id: self.backing_id,
            exclusive_versions: self.exclusive_versions.clone(),
        }
    }

    /// Creates another view from the root allocation, not from this view's range.
    pub(crate) fn view_from_root(&self, offset: usize, length: usize) -> Option<Self> {
        let end = offset.checked_add(length)?;
        if end > self.root_length {
            return None;
        }
        Some(Self {
            bytes: self.bytes.clone(),
            offset,
            length,
            root_length: self.root_length,
            backing_id: self.backing_id,
            exclusive_versions: self.exclusive_versions.clone(),
        })
    }

    /// Encodes the complete sparse backing root. Views must be deduplicated by
    /// backing ID by the outer memory catalog and are never serialized alone.
    pub fn encode_root_save_state(&self) -> Result<Vec<u8>, SharedMemoryStateError> {
        if self.offset != 0 || self.length != self.root_length {
            return Err(SharedMemoryStateError::NotRootView);
        }
        encode_shared_memory_root(self)
    }

    /// Creates a new root backing from a previously validated root snapshot.
    pub fn from_root_save_state(encoded: &[u8]) -> Result<Self, SharedMemoryStateError> {
        let (root_length, store, versions) = decode_shared_memory_root(encoded)?;
        Ok(Self {
            bytes: Arc::new(Mutex::new(store)),
            offset: 0,
            length: root_length,
            root_length,
            backing_id: crate::memory::next_memory_backing_id(),
            exclusive_versions: Arc::new(Mutex::new(versions)),
        })
    }

    /// Restores this complete root in place, keeping every existing alias alive.
    pub fn restore_root_save_state(
        &mut self,
        encoded: &[u8],
    ) -> Result<(), SharedMemoryStateError> {
        if self.offset != 0 || self.length != self.root_length {
            return Err(SharedMemoryStateError::NotRootView);
        }
        let (root_length, store, versions) = decode_shared_memory_root(encoded)?;
        if root_length != self.root_length {
            return Err(SharedMemoryStateError::RootLengthMismatch {
                expected: self.root_length,
                actual: root_length,
            });
        }

        let mut current_store = self
            .bytes
            .lock()
            .map_err(|_| SharedMemoryStateError::LockPoisoned)?;
        let mut current_versions = self
            .exclusive_versions
            .lock()
            .map_err(|_| SharedMemoryStateError::LockPoisoned)?;
        *current_store = store;
        *current_versions = versions;
        Ok(())
    }

    /// Number of 4 KiB pages that have actually been written.
    pub fn allocated_page_count(&self) -> usize {
        self.allocated_page_stats().0
    }

    /// Logical byte payload held by materialized 4 KiB pages, excluding allocator overhead.
    pub fn allocated_bytes(&self) -> usize {
        self.allocated_page_stats().1
    }

    fn allocated_page_stats(&self) -> (usize, usize) {
        self.bytes
            .lock()
            .map(|store| {
                let pages = store.pages.len();
                (pages, pages.saturating_mul(SHARED_BACKING_PAGE_SIZE))
            })
            .unwrap_or_default()
    }

    /// Number of exclusive-monitor granules with a recorded write generation.
    pub fn exclusive_monitor_entry_count(&self) -> usize {
        self.exclusive_versions
            .lock()
            .map(|versions| versions.len())
            .unwrap_or_default()
    }

    /// Creates a bounded view into the same allocation without copying.
    fn view(&self, offset: usize, length: usize) -> Option<Self> {
        let end = offset.checked_add(length)?;
        if end > self.length {
            return None;
        }
        let root_offset = self.offset.checked_add(offset)?;
        root_offset.checked_add(length)?;
        Some(Self {
            bytes: self.bytes.clone(),
            offset: root_offset,
            length,
            root_length: self.root_length,
            backing_id: self.backing_id,
            exclusive_versions: self.exclusive_versions.clone(),
        })
    }

    /// Returns the view size, or None if its backing lock was poisoned.
    pub fn len(&self) -> Option<usize> {
        self.bytes.lock().ok().map(|_| self.length)
    }

    /// Returns whether the backing view is empty, or None if its backing lock was poisoned.
    pub fn is_empty(&self) -> Option<bool> {
        self.len().map(|length| length == 0)
    }

    /// Reports whether two views refer to the same backing allocation.
    pub fn shares_backing_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes)
    }

    /// Reports whether two views identify the same bytes within the allocation.
    pub fn is_same_view_as(&self, other: &Self) -> bool {
        self.shares_backing_with(other)
            && self.offset == other.offset
            && self.length == other.length
    }

    /// Stable identity shared by clones and views of this allocation.
    fn backing_id(&self) -> u64 {
        self.backing_id
    }

    /// Absolute byte offset within the root allocation.
    fn root_offset(&self, offset: usize) -> Option<usize> {
        if offset > self.length {
            return None;
        }
        self.offset.checked_add(offset)
    }

    /// Absolute byte offset of this view within the shared root.
    pub(crate) fn root_base_offset(&self) -> usize {
        self.offset
    }

    fn exclusive_version(&self, offset: usize) -> u64 {
        let Some(root_offset) = self.root_offset(offset) else {
            return 0;
        };
        let granule = (root_offset as u32) & !(crate::memory::EXCLUSIVE_RESERVATION_GRANULE - 1);
        // Observing a granule registers it; writes advance only observed ones.
        self.exclusive_versions
            .lock()
            .ok()
            .map(|mut versions| *versions.entry(granule).or_insert(0))
            .unwrap_or(0)
    }

    fn read_range(&self, offset: usize, output: &mut [u8]) -> bool {
        let Some(start) = self.root_offset(offset) else {
            return false;
        };
        let Some(relative_end) = offset.checked_add(output.len()) else {
            return false;
        };
        if relative_end > self.length || start.checked_add(output.len()).is_none() {
            return false;
        }

        let Ok(store) = self.bytes.lock() else {
            return false;
        };
        let mut copied = 0usize;
        while copied < output.len() {
            let absolute = start + copied;
            let page_number = absolute / SHARED_BACKING_PAGE_SIZE;
            let page_offset = absolute % SHARED_BACKING_PAGE_SIZE;
            let count = (SHARED_BACKING_PAGE_SIZE - page_offset).min(output.len() - copied);

            if let Some(page) = store.pages.get(&page_number) {
                output[copied..copied + count]
                    .copy_from_slice(&page[page_offset..page_offset + count]);
            } else {
                output[copied..copied + count].fill(0);
            }
            copied += count;
        }
        true
    }

    /// Writes bytes into the shared view, bumping exclusive-monitor versions.
    ///
    /// Returns false when the range lies outside the view, a backing lock is
    /// poisoned, or a new page cannot be reserved.
    /// HLE services use this to publish data (for example HID state) that the
    /// guest reads through its own mapping of the same block.
    pub fn write_bytes(&self, offset: usize, input: &[u8]) -> bool {
        self.write_range(offset, input)
    }

    /// Reads bytes from the shared view. Returns false when out of range.
    ///
    /// HLE services use this to inspect memory the guest wrote through its mapping.
    pub fn read_bytes(&self, offset: usize, output: &mut [u8]) -> bool {
        self.read_range(offset, output)
    }

    fn write_range(&self, offset: usize, input: &[u8]) -> bool {
        let Some(start) = self.root_offset(offset) else {
            return false;
        };
        let Some(relative_end) = offset.checked_add(input.len()) else {
            return false;
        };
        let Some(absolute_end) = start.checked_add(input.len()) else {
            return false;
        };
        if relative_end > self.length {
            return false;
        }

        let Ok(mut store) = self.bytes.lock() else {
            return false;
        };
        if !input.is_empty() {
            let first_page = start / SHARED_BACKING_PAGE_SIZE;
            let last_page = (absolute_end - 1) / SHARED_BACKING_PAGE_SIZE;
            let missing_pages = (first_page..=last_page)
                .filter(|page| !store.pages.contains_key(page))
                .count();
            let mut pending_pages = Vec::new();
            if pending_pages.try_reserve_exact(missing_pages).is_err() {
                return false;
            }

            // Allocate every missing page before changing the backing map or
            // copying bytes, so allocation failure cannot partially write.
            for page_number in first_page..=last_page {
                if store.pages.contains_key(&page_number) {
                    continue;
                }
                let mut page = Vec::new();
                if page.try_reserve_exact(SHARED_BACKING_PAGE_SIZE).is_err() {
                    return false;
                }
                page.resize(SHARED_BACKING_PAGE_SIZE, 0);
                pending_pages.push((page_number, page));
            }
            for (page_number, page) in pending_pages {
                store.pages.insert(page_number, page);
            }
        }

        let mut copied = 0usize;
        while copied < input.len() {
            let absolute = start + copied;
            let page_number = absolute / SHARED_BACKING_PAGE_SIZE;
            let page_offset = absolute % SHARED_BACKING_PAGE_SIZE;
            let count = (SHARED_BACKING_PAGE_SIZE - page_offset).min(input.len() - copied);
            let Some(page) = store.pages.get_mut(&page_number) else {
                return false;
            };
            page[page_offset..page_offset + count].copy_from_slice(&input[copied..copied + count]);
            copied += count;
        }
        drop(store);

        if !input.is_empty() {
            if let Ok(mut versions) = self.exclusive_versions.lock() {
                let granule_size = crate::memory::EXCLUSIVE_RESERVATION_GRANULE as usize;
                let mut granule = start & !(granule_size - 1);
                while granule < absolute_end {
                    if let Some(version) = versions.get_mut(&(granule as u32)) {
                        *version = version.wrapping_add(1);
                    }
                    granule += granule_size;
                }
            }
        }
        true
    }
}

/// Errors while encoding or restoring a sparse shared-memory backing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharedMemoryStateError {
    AllocationFailed,
    LockPoisoned,
    NotRootView,
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16),
    UnsupportedFlags(u16),
    InvalidRootLength(u64),
    RootLengthMismatch { expected: usize, actual: usize },
    TooManyPages(u32),
    TooManyExclusiveMonitors(u32),
    InvalidPageIndex(u64),
    DuplicatePage(u64),
    InvalidExclusiveGranule(u32),
    DuplicateExclusiveGranule(u32),
    TooManyBackings(u32),
    InvalidCatalogEntry,
    InvalidBackingSlot(u32),
    InvalidViewRange { slot: u32, offset: u64, length: u64 },
    InvalidAddressSpaceState(&'static str),
    TooManyAddressSpaceEntries(u32),
    TooManyCacheLines(u32),
    TooManyWriteBufferEntries(u32),
    InvalidCacheTarget,
    MissingPhysicalMemory,
    TrailingData,
}

impl fmt::Display for SharedMemoryStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationFailed => {
                formatter.write_str("could not allocate shared-memory snapshot")
            }
            Self::LockPoisoned => formatter.write_str("shared-memory backing lock is poisoned"),
            Self::NotRootView => {
                formatter.write_str("only a complete shared-memory root can be serialized")
            }
            Self::Truncated => formatter.write_str("shared-memory snapshot is truncated"),
            Self::InvalidMagic => {
                formatter.write_str("shared-memory snapshot signature is invalid")
            }
            Self::UnsupportedVersion(version) => {
                write!(
                    formatter,
                    "shared-memory snapshot version {version} is unsupported"
                )
            }
            Self::UnsupportedFlags(flags) => {
                write!(
                    formatter,
                    "shared-memory snapshot flags {flags:#06X} are invalid"
                )
            }
            Self::InvalidRootLength(length) => {
                write!(formatter, "shared-memory root length {length} is invalid")
            }
            Self::RootLengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "shared-memory root length mismatch: expected {expected}, got {actual}"
                )
            }
            Self::TooManyPages(count) => {
                write!(
                    formatter,
                    "shared-memory snapshot contains too many pages: {count}"
                )
            }
            Self::TooManyExclusiveMonitors(count) => {
                write!(
                    formatter,
                    "shared-memory snapshot contains too many exclusive monitors: {count}"
                )
            }
            Self::InvalidPageIndex(page) => {
                write!(
                    formatter,
                    "shared-memory page index {page} is outside its root"
                )
            }
            Self::DuplicatePage(page) => {
                write!(
                    formatter,
                    "shared-memory page index {page} is duplicated or unordered"
                )
            }
            Self::InvalidExclusiveGranule(address) => {
                write!(
                    formatter,
                    "shared-memory exclusive granule {address:#010X} is invalid"
                )
            }
            Self::DuplicateExclusiveGranule(address) => {
                write!(
                    formatter,
                    "shared-memory exclusive granule {address:#010X} is duplicated or unordered"
                )
            }
            Self::TooManyBackings(count) => {
                write!(
                    formatter,
                    "shared-memory catalog contains too many roots: {count}"
                )
            }
            Self::InvalidCatalogEntry => {
                formatter.write_str("shared-memory catalog entry is invalid")
            }
            Self::InvalidBackingSlot(slot) => {
                write!(
                    formatter,
                    "shared-memory backing slot {slot} does not exist"
                )
            }
            Self::InvalidViewRange {
                slot,
                offset,
                length,
            } => write!(
                formatter,
                "shared-memory view {offset:#X}+{length:#X} is outside backing slot {slot}"
            ),
            Self::InvalidAddressSpaceState(reason) => {
                write!(
                    formatter,
                    "ARM11 address-space snapshot is invalid: {reason}"
                )
            }
            Self::TooManyAddressSpaceEntries(count) => {
                write!(
                    formatter,
                    "ARM11 address-space snapshot contains too many regions: {count}"
                )
            }
            Self::TooManyCacheLines(count) => {
                write!(
                    formatter,
                    "ARM11 address-space snapshot contains too many D-cache lines: {count}"
                )
            }
            Self::TooManyWriteBufferEntries(count) => {
                write!(
                    formatter,
                    "ARM11 address-space snapshot contains too many write-buffer entries: {count}"
                )
            }
            Self::InvalidCacheTarget => {
                formatter.write_str("ARM11 D-cache entry refers to an invalid backing or offset")
            }
            Self::MissingPhysicalMemory => formatter
                .write_str("ARM11 address-space snapshot requires attached physical memory"),
            Self::TrailingData => {
                formatter.write_str("shared-memory snapshot contains trailing bytes")
            }
        }
    }
}

impl std::error::Error for SharedMemoryStateError {}

const SHARED_MEMORY_STATE_MAGIC: &[u8; 8] = b"ACSHARED";
const SHARED_MEMORY_STATE_VERSION: u16 = 1;
const SHARED_MEMORY_STATE_HEADER_SIZE: usize = 28;
const MAX_SHARED_MEMORY_ROOT_BYTES: usize = 512 * 1024 * 1024;

fn encode_shared_memory_root(
    backing: &SharedMemoryBacking,
) -> Result<Vec<u8>, SharedMemoryStateError> {
    let store = backing
        .bytes
        .lock()
        .map_err(|_| SharedMemoryStateError::LockPoisoned)?;
    let versions = backing
        .exclusive_versions
        .lock()
        .map_err(|_| SharedMemoryStateError::LockPoisoned)?;
    let max_pages = backing.root_length.div_ceil(SHARED_BACKING_PAGE_SIZE);
    let max_monitors = backing
        .root_length
        .div_ceil(crate::memory::EXCLUSIVE_RESERVATION_GRANULE as usize);
    if backing.root_length == 0 || backing.root_length > MAX_SHARED_MEMORY_ROOT_BYTES {
        return Err(SharedMemoryStateError::InvalidRootLength(
            backing.root_length as u64,
        ));
    }
    if store.pages.len() > max_pages {
        return Err(SharedMemoryStateError::TooManyPages(
            u32::try_from(store.pages.len()).unwrap_or(u32::MAX),
        ));
    }
    if versions.len() > max_monitors {
        return Err(SharedMemoryStateError::TooManyExclusiveMonitors(
            u32::try_from(versions.len()).unwrap_or(u32::MAX),
        ));
    }

    let page_count = u32::try_from(store.pages.len())
        .map_err(|_| SharedMemoryStateError::TooManyPages(u32::MAX))?;
    let monitor_count = u32::try_from(versions.len())
        .map_err(|_| SharedMemoryStateError::TooManyExclusiveMonitors(u32::MAX))?;
    let page_bytes = store
        .pages
        .len()
        .checked_mul(8 + SHARED_BACKING_PAGE_SIZE)
        .ok_or(SharedMemoryStateError::AllocationFailed)?;
    let monitor_bytes = versions
        .len()
        .checked_mul(12)
        .ok_or(SharedMemoryStateError::AllocationFailed)?;
    let capacity = SHARED_MEMORY_STATE_HEADER_SIZE
        .checked_add(page_bytes)
        .and_then(|size| size.checked_add(monitor_bytes))
        .ok_or(SharedMemoryStateError::AllocationFailed)?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(capacity)
        .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
    encoded.extend_from_slice(SHARED_MEMORY_STATE_MAGIC);
    encoded.extend_from_slice(&SHARED_MEMORY_STATE_VERSION.to_le_bytes());
    encoded.extend_from_slice(&0u16.to_le_bytes());
    encoded.extend_from_slice(&(backing.root_length as u64).to_le_bytes());
    encoded.extend_from_slice(&page_count.to_le_bytes());
    encoded.extend_from_slice(&monitor_count.to_le_bytes());

    for (page_number, page) in &store.pages {
        if page.len() != SHARED_BACKING_PAGE_SIZE
            || page_number
                .checked_mul(SHARED_BACKING_PAGE_SIZE)
                .is_none_or(|start| start >= backing.root_length)
        {
            return Err(SharedMemoryStateError::InvalidPageIndex(
                *page_number as u64,
            ));
        }
        encoded.extend_from_slice(&(*page_number as u64).to_le_bytes());
        encoded.extend_from_slice(page);
    }
    for (address, generation) in versions.iter() {
        if address % crate::memory::EXCLUSIVE_RESERVATION_GRANULE != 0
            || *address as usize >= backing.root_length
        {
            return Err(SharedMemoryStateError::InvalidExclusiveGranule(*address));
        }
        encoded.extend_from_slice(&address.to_le_bytes());
        encoded.extend_from_slice(&generation.to_le_bytes());
    }
    Ok(encoded)
}

fn decode_shared_memory_root(
    encoded: &[u8],
) -> Result<(usize, SparseSharedMemoryStore, BTreeMap<u32, u64>), SharedMemoryStateError> {
    if encoded.len() < SHARED_MEMORY_STATE_HEADER_SIZE {
        return Err(SharedMemoryStateError::Truncated);
    }
    if encoded.get(..8) != Some(SHARED_MEMORY_STATE_MAGIC.as_slice()) {
        return Err(SharedMemoryStateError::InvalidMagic);
    }
    let version = read_state_u16(encoded, 8)?;
    if version != SHARED_MEMORY_STATE_VERSION {
        return Err(SharedMemoryStateError::UnsupportedVersion(version));
    }
    let flags = read_state_u16(encoded, 10)?;
    if flags != 0 {
        return Err(SharedMemoryStateError::UnsupportedFlags(flags));
    }
    let root_length_u64 = read_state_u64(encoded, 12)?;
    if root_length_u64 == 0 || root_length_u64 > MAX_SHARED_MEMORY_ROOT_BYTES as u64 {
        return Err(SharedMemoryStateError::InvalidRootLength(root_length_u64));
    }
    let root_length = usize::try_from(root_length_u64)
        .map_err(|_| SharedMemoryStateError::InvalidRootLength(root_length_u64))?;
    let page_count = read_state_u32(encoded, 20)?;
    let monitor_count = read_state_u32(encoded, 24)?;
    let max_pages = root_length.div_ceil(SHARED_BACKING_PAGE_SIZE);
    let max_monitors = root_length.div_ceil(crate::memory::EXCLUSIVE_RESERVATION_GRANULE as usize);
    if page_count as usize > max_pages {
        return Err(SharedMemoryStateError::TooManyPages(page_count));
    }
    if monitor_count as usize > max_monitors {
        return Err(SharedMemoryStateError::TooManyExclusiveMonitors(
            monitor_count,
        ));
    }

    let page_bytes = (page_count as usize)
        .checked_mul(8 + SHARED_BACKING_PAGE_SIZE)
        .ok_or(SharedMemoryStateError::Truncated)?;
    let monitor_bytes = (monitor_count as usize)
        .checked_mul(12)
        .ok_or(SharedMemoryStateError::Truncated)?;
    let expected_length = SHARED_MEMORY_STATE_HEADER_SIZE
        .checked_add(page_bytes)
        .and_then(|size| size.checked_add(monitor_bytes))
        .ok_or(SharedMemoryStateError::Truncated)?;
    if encoded.len() < expected_length {
        return Err(SharedMemoryStateError::Truncated);
    }
    if encoded.len() > expected_length {
        return Err(SharedMemoryStateError::TrailingData);
    }

    let mut store = SparseSharedMemoryStore::default();
    let mut cursor = SHARED_MEMORY_STATE_HEADER_SIZE;
    let mut previous_page = None;
    for _ in 0..page_count {
        let page_number = read_state_u64(encoded, cursor)?;
        cursor += 8;
        let page_start = usize::try_from(page_number)
            .ok()
            .and_then(|number| number.checked_mul(SHARED_BACKING_PAGE_SIZE))
            .ok_or(SharedMemoryStateError::InvalidPageIndex(page_number))?;
        if page_start >= root_length {
            return Err(SharedMemoryStateError::InvalidPageIndex(page_number));
        }
        if previous_page.is_some_and(|previous| page_number <= previous) {
            return Err(SharedMemoryStateError::DuplicatePage(page_number));
        }
        previous_page = Some(page_number);
        let page_bytes = encoded
            .get(cursor..cursor + SHARED_BACKING_PAGE_SIZE)
            .ok_or(SharedMemoryStateError::Truncated)?;
        cursor += SHARED_BACKING_PAGE_SIZE;
        let mut page = Vec::new();
        page.try_reserve_exact(SHARED_BACKING_PAGE_SIZE)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        page.extend_from_slice(page_bytes);
        store.pages.insert(page_number as usize, page);
    }

    let mut versions = BTreeMap::new();
    let mut previous_granule = None;
    for _ in 0..monitor_count {
        let address = read_state_u32(encoded, cursor)?;
        let generation = read_state_u64(encoded, cursor + 4)?;
        cursor += 12;
        if address % crate::memory::EXCLUSIVE_RESERVATION_GRANULE != 0
            || address as usize >= root_length
        {
            return Err(SharedMemoryStateError::InvalidExclusiveGranule(address));
        }
        if previous_granule.is_some_and(|previous| address <= previous) {
            return Err(SharedMemoryStateError::DuplicateExclusiveGranule(address));
        }
        previous_granule = Some(address);
        versions.insert(address, generation);
    }
    if cursor != encoded.len() {
        return Err(SharedMemoryStateError::TrailingData);
    }
    Ok((root_length, store, versions))
}

/// Per-machine catalog that serializes each shared root exactly once.
///
/// Address-space mappings, kernel memory-block objects and HLE services retain
/// a stable slot plus a root-relative view range. Clones/views of the same root
/// resolve to the same slot, preserving guest-visible aliasing after restore.
#[derive(Debug, Default)]
pub(crate) struct SharedMemoryBackingCatalog {
    slots_by_backing_id: BTreeMap<u64, u32>,
    roots: Vec<SharedMemoryBacking>,
}

impl SharedMemoryBackingCatalog {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers a view's root, deduplicating by the shared backing identity.
    pub(crate) fn register(
        &mut self,
        view: &SharedMemoryBacking,
    ) -> Result<u32, SharedMemoryStateError> {
        if let Some(slot) = self.slots_by_backing_id.get(&view.backing_id()) {
            return Ok(*slot);
        }
        if self.roots.len() >= MAX_SHARED_BACKING_CATALOG_ENTRIES {
            return Err(SharedMemoryStateError::TooManyBackings(
                u32::try_from(self.roots.len() + 1).unwrap_or(u32::MAX),
            ));
        }
        let slot = u32::try_from(self.roots.len())
            .map_err(|_| SharedMemoryStateError::TooManyBackings(u32::MAX))?;
        self.roots
            .try_reserve(1)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        self.roots.push(view.root_view());
        self.slots_by_backing_id.insert(view.backing_id(), slot);
        Ok(slot)
    }

    /// Returns the slot assigned to a backing that has already been registered.
    pub(crate) fn slot_for(
        &self,
        view: &SharedMemoryBacking,
    ) -> Result<u32, SharedMemoryStateError> {
        self.slots_by_backing_id
            .get(&view.backing_id())
            .copied()
            .ok_or(SharedMemoryStateError::InvalidCatalogEntry)
    }

    /// Recreates a bounded view over a catalog root.
    pub(crate) fn view(
        &self,
        slot: u32,
        offset: usize,
        length: usize,
    ) -> Result<SharedMemoryBacking, SharedMemoryStateError> {
        let root = self
            .roots
            .get(slot as usize)
            .ok_or(SharedMemoryStateError::InvalidBackingSlot(slot))?;
        root.view_from_root(offset, length)
            .ok_or(SharedMemoryStateError::InvalidViewRange {
                slot,
                offset: offset as u64,
                length: length as u64,
            })
    }

    pub(crate) fn root_count(&self) -> usize {
        self.roots.len()
    }

    /// Wraps this catalog as the shared-backing section in ACRPAYLD.
    pub(crate) fn encode_section(
        &self,
    ) -> Result<crate::save_state::SaveStateSection, SharedMemoryStateError> {
        crate::save_state::SaveStateSection::new(
            crate::save_state::section_id::SHARED_BACKINGS,
            SHARED_BACKING_CATALOG_VERSION,
            self.encode()?,
        )
        .map_err(|_| SharedMemoryStateError::InvalidCatalogEntry)
    }

    /// Restores a catalog section before any mapping or kernel handle is rebuilt.
    pub(crate) fn decode_section(
        section: &crate::save_state::SaveStateSection,
    ) -> Result<Self, SharedMemoryStateError> {
        if section.id() != crate::save_state::section_id::SHARED_BACKINGS
            || section.version() != SHARED_BACKING_CATALOG_VERSION
        {
            return Err(SharedMemoryStateError::InvalidCatalogEntry);
        }
        Self::decode(section.payload())
    }

    /// Encodes roots in slot order. Each sparse root is emitted only once.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, SharedMemoryStateError> {
        if self.roots.len() > MAX_SHARED_BACKING_CATALOG_ENTRIES {
            return Err(SharedMemoryStateError::TooManyBackings(
                self.roots.len() as u32
            ));
        }

        let mut roots = Vec::new();
        roots
            .try_reserve_exact(self.roots.len())
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        let mut total_size = SHARED_BACKING_CATALOG_HEADER_SIZE;
        for root in &self.roots {
            let snapshot = root.encode_root_save_state()?;
            total_size = total_size
                .checked_add(8)
                .and_then(|size| size.checked_add(snapshot.len()))
                .ok_or(SharedMemoryStateError::AllocationFailed)?;
            if total_size > MAX_SHARED_MEMORY_CATALOG_BYTES {
                return Err(SharedMemoryStateError::AllocationFailed);
            }
            roots.push(snapshot);
        }

        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(total_size)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        encoded.extend_from_slice(SHARED_BACKING_CATALOG_MAGIC);
        encoded.extend_from_slice(&SHARED_BACKING_CATALOG_VERSION.to_le_bytes());
        encoded.extend_from_slice(&0u16.to_le_bytes());
        encoded.extend_from_slice(&(self.roots.len() as u32).to_le_bytes());
        for root in roots {
            encoded.extend_from_slice(&(root.len() as u64).to_le_bytes());
            encoded.extend_from_slice(&root);
        }
        debug_assert_eq!(encoded.len(), total_size);
        Ok(encoded)
    }

    /// Decodes every root before exposing any slot.
    pub(crate) fn decode(encoded: &[u8]) -> Result<Self, SharedMemoryStateError> {
        if encoded.len() > MAX_SHARED_MEMORY_CATALOG_BYTES {
            return Err(SharedMemoryStateError::AllocationFailed);
        }
        if encoded.len() < SHARED_BACKING_CATALOG_HEADER_SIZE {
            return Err(SharedMemoryStateError::Truncated);
        }
        if encoded.get(..8) != Some(SHARED_BACKING_CATALOG_MAGIC.as_slice()) {
            return Err(SharedMemoryStateError::InvalidMagic);
        }
        let version = read_state_u16(encoded, 8)?;
        if version != SHARED_BACKING_CATALOG_VERSION {
            return Err(SharedMemoryStateError::UnsupportedVersion(version));
        }
        let flags = read_state_u16(encoded, 10)?;
        if flags != 0 {
            return Err(SharedMemoryStateError::UnsupportedFlags(flags));
        }
        let root_count = read_state_u32(encoded, 12)?;
        if root_count as usize > MAX_SHARED_BACKING_CATALOG_ENTRIES {
            return Err(SharedMemoryStateError::TooManyBackings(root_count));
        }

        let mut roots = Vec::new();
        roots
            .try_reserve_exact(root_count as usize)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        let mut slots_by_backing_id = BTreeMap::new();
        let mut cursor = SHARED_BACKING_CATALOG_HEADER_SIZE;
        for slot in 0..root_count {
            let length = read_state_u64(encoded, cursor)?;
            cursor += 8;
            let length =
                usize::try_from(length).map_err(|_| SharedMemoryStateError::InvalidCatalogEntry)?;
            let end = cursor
                .checked_add(length)
                .ok_or(SharedMemoryStateError::Truncated)?;
            let root_bytes = encoded
                .get(cursor..end)
                .ok_or(SharedMemoryStateError::Truncated)?;
            let root = SharedMemoryBacking::from_root_save_state(root_bytes)?;
            if slots_by_backing_id
                .insert(root.backing_id(), slot)
                .is_some()
            {
                return Err(SharedMemoryStateError::InvalidCatalogEntry);
            }
            roots.push(root);
            cursor = end;
        }
        if cursor != encoded.len() {
            return Err(SharedMemoryStateError::TrailingData);
        }
        Ok(Self {
            slots_by_backing_id,
            roots,
        })
    }
}

const SHARED_BACKING_CATALOG_MAGIC: &[u8; 8] = b"ACSHCATL";
const SHARED_BACKING_CATALOG_VERSION: u16 = 1;
const SHARED_BACKING_CATALOG_HEADER_SIZE: usize = 16;
const MAX_SHARED_BACKING_CATALOG_ENTRIES: usize = 4096;
const MAX_SHARED_MEMORY_CATALOG_BYTES: usize = 512 * 1024 * 1024;

fn read_state_u16(bytes: &[u8], offset: usize) -> Result<u16, SharedMemoryStateError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(SharedMemoryStateError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_state_u32(bytes: &[u8], offset: usize) -> Result<u32, SharedMemoryStateError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(SharedMemoryStateError::Truncated)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_state_u64(bytes: &[u8], offset: usize) -> Result<u64, SharedMemoryStateError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(SharedMemoryStateError::Truncated)?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

#[cfg(test)]
mod shared_memory_state_tests {
    use super::*;

    #[test]
    fn shared_backing_catalog_deduplicates_aliases_and_restores_slots() {
        let root = SharedMemoryBacking::zeroed(0x8000).unwrap();
        let first_view = root.view_from_root(0x1000, 0x1000).unwrap();
        let second_view = root.view_from_root(0x3000, 0x0800).unwrap();
        root.write_bytes(0x1010, &[1, 2, 3]);
        root.write_bytes(0x3010, &[4, 5, 6]);

        let mut catalog = SharedMemoryBackingCatalog::new();
        let root_slot = catalog.register(&root).unwrap();
        let first_slot = catalog.register(&first_view).unwrap();
        let second_slot = catalog.register(&second_view).unwrap();
        assert_eq!(root_slot, first_slot);
        assert_eq!(root_slot, second_slot);
        assert_eq!(catalog.root_count(), 1);

        let encoded = catalog.encode().unwrap();
        let restored = SharedMemoryBackingCatalog::decode(&encoded).unwrap();
        assert_eq!(restored.root_count(), 1);
        let restored_first = restored.view(first_slot, 0x1000, 0x1000).unwrap();
        let restored_second = restored.view(second_slot, 0x3000, 0x0800).unwrap();
        let mut first_bytes = [0u8; 3];
        let mut second_bytes = [0u8; 3];
        restored_first.read_bytes(0x10, &mut first_bytes);
        restored_second.read_bytes(0x10, &mut second_bytes);
        assert_eq!(first_bytes, [1, 2, 3]);
        assert_eq!(second_bytes, [4, 5, 6]);
        assert!(restored_first.shares_backing_with(&restored_second));
    }

    #[test]
    fn shared_backing_catalog_rejects_trailing_data_and_invalid_view_ranges() {
        let root = SharedMemoryBacking::zeroed(0x2000).unwrap();
        let mut catalog = SharedMemoryBackingCatalog::new();
        let slot = catalog.register(&root).unwrap();
        let encoded = catalog.encode().unwrap();
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(matches!(
            SharedMemoryBackingCatalog::decode(&trailing),
            Err(SharedMemoryStateError::TrailingData)
        ));

        let restored = SharedMemoryBackingCatalog::decode(&encoded).unwrap();
        assert!(matches!(
            restored.view(slot, 0x1000, 0x2000),
            Err(SharedMemoryStateError::InvalidViewRange { .. })
        ));
        assert!(matches!(
            restored.view(1, 0, 1),
            Err(SharedMemoryStateError::InvalidBackingSlot(1))
        ));
    }

    #[test]
    fn shared_backing_snapshot_restores_sparse_bytes_and_view_aliases() {
        let root = SharedMemoryBacking::zeroed(0x8000).unwrap();
        let mut view = root.view_from_root(0x2000, 0x1000).unwrap();
        root.write_bytes(0x2010, &[1, 2, 3, 4]);
        view.write_bytes(0x11, &[0xAA]);

        let encoded = root.encode_root_save_state().unwrap();
        let restored_root = SharedMemoryBacking::from_root_save_state(&encoded).unwrap();
        let restored_view = restored_root.view_from_root(0x2000, 0x1000).unwrap();
        let mut bytes = [0u8; 4];
        restored_root.read_bytes(0x2010, &mut bytes);
        assert_eq!(bytes, [1, 0xAA, 3, 4]);
        assert_eq!(restored_root.allocated_page_count(), 1);
        assert_eq!(restored_root.root_length(), 0x8000);
        assert_eq!(restored_view.root_base_offset(), 0x2000);
        assert!(restored_root.shares_backing_with(&restored_view));
    }

    #[test]
    fn shared_backing_restore_updates_existing_aliases_only_after_validation() {
        let mut root = SharedMemoryBacking::zeroed(0x4000).unwrap();
        let alias = root.view_from_root(0x1000, 0x1000).unwrap();
        root.write_bytes(0x1004, &[1, 2, 3]);
        let before = root.encode_root_save_state().unwrap();

        let replacement = SharedMemoryBacking::zeroed(0x4000).unwrap();
        replacement.write_bytes(0x1004, &[7, 8, 9]);
        let replacement_bytes = replacement.encode_root_save_state().unwrap();

        let mut truncated = replacement_bytes.clone();
        truncated.pop();
        assert!(root.restore_root_save_state(&truncated).is_err());
        let mut unchanged = [0u8; 3];
        alias.read_bytes(4, &mut unchanged);
        assert_eq!(unchanged, [1, 2, 3]);

        root.restore_root_save_state(&replacement_bytes).unwrap();
        let mut updated = [0u8; 3];
        alias.read_bytes(4, &mut updated);
        assert_eq!(updated, [7, 8, 9]);
        assert_eq!(root.encode_root_save_state().unwrap(), replacement_bytes);
        assert_ne!(before, replacement_bytes);
    }

    #[test]
    fn shared_backing_snapshot_rejects_views_and_out_of_range_records() {
        let root = SharedMemoryBacking::zeroed(0x2000).unwrap();
        // The malformed page-index record below must exist in the snapshot.
        root.write_bytes(0, &[0xA5]);
        let view = root.view_from_root(0x1000, 0x1000).unwrap();
        assert!(matches!(
            view.encode_root_save_state(),
            Err(SharedMemoryStateError::NotRootView)
        ));

        let encoded = root.encode_root_save_state().unwrap();
        let mut malformed = encoded.clone();
        malformed[SHARED_MEMORY_STATE_HEADER_SIZE..SHARED_MEMORY_STATE_HEADER_SIZE + 8]
            .copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            SharedMemoryBacking::from_root_save_state(&malformed),
            Err(SharedMemoryStateError::InvalidPageIndex(_))
        ));
    }
}
impl PartialEq for SharedMemoryBacking {
    fn eq(&self, other: &Self) -> bool {
        self.is_same_view_as(other)
    }
}

impl Eq for SharedMemoryBacking {}

#[derive(Clone, Debug)]
enum DynamicMemoryBacking {
    Private(SharedMemoryBacking),
    Shared(SharedMemoryBacking),
}

impl DynamicMemoryBacking {
    fn len(&self) -> usize {
        match self {
            Self::Private(backing) | Self::Shared(backing) => backing.length,
        }
    }

    fn backing_handle(&self) -> SharedMemoryBacking {
        self.backing_ref().clone()
    }

    fn backing_ref(&self) -> &SharedMemoryBacking {
        match self {
            Self::Private(backing) | Self::Shared(backing) => backing,
        }
    }

    /// Promotes the allocation to shareable backing and returns a bounded view.
    fn shared_view(&mut self, offset: usize, length: usize) -> Option<SharedMemoryBacking> {
        if let Self::Private(backing) = self {
            *self = Self::Shared(backing.clone());
        }
        match self {
            Self::Private(_) => None,
            Self::Shared(backing) => backing.view(offset, length),
        }
    }
}

#[derive(Debug)]
struct DynamicMemoryRegion {
    address: u32,
    backing: DynamicMemoryBacking,
    /// 3DS MemoryPermission bits: bit 0 read, bit 1 write.
    permissions: u32,
}

impl DynamicMemoryRegion {
    fn len(&self) -> usize {
        self.backing.len()
    }

    fn end_address(&self) -> u64 {
        u64::from(self.address) + self.len() as u64
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SharedMemoryRegion {
    address: u32,
    backing: SharedMemoryBacking,
    permissions: u32,
}

impl SharedMemoryRegion {
    fn size(&self) -> Option<u32> {
        u32::try_from(self.backing.len()?).ok()
    }

    fn end_address(&self) -> Option<u64> {
        Some(u64::from(self.address) + u64::from(self.size()?))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinearMemoryRegion {
    virtual_base: u32,
    physical_base: u32,
    size: u32,
}

impl LinearMemoryRegion {
    fn virtual_end(self) -> u64 {
        u64::from(self.virtual_base) + u64::from(self.size)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IpcTemporaryMappingRegion {
    alias_address: u32,
    mapping_base: u32,
    mapping_size: u32,
    reservation_base: u32,
    reservation_size: u32,
}

impl IpcTemporaryMappingRegion {
    fn reservation_end(self) -> u64 {
        u64::from(self.reservation_base) + u64::from(self.reservation_size)
    }
}

#[derive(Clone, Copy, Debug)]
enum MemoryLocation {
    Static {
        kind: Arm11SegmentKind,
        offset: usize,
    },
    Dynamic {
        index: usize,
        offset: usize,
    },
    Shared {
        index: usize,
        offset: usize,
    },
    Physical {
        index: usize,
        address: u32,
    },
}

/// Avoids a heap allocation for the small memory stores emitted by ARM11
/// scalar instructions while retaining an unbounded fallback for block writes.
const INLINE_WRITE_LOCATION_CAPACITY: usize = 8;

enum ValidatedWriteLocations {
    Inline {
        entries: [Option<MemoryLocation>; INLINE_WRITE_LOCATION_CAPACITY],
        len: usize,
    },
    Heap(Vec<MemoryLocation>),
}

impl ValidatedWriteLocations {
    fn with_capacity(capacity: usize) -> Self {
        if capacity <= INLINE_WRITE_LOCATION_CAPACITY {
            Self::Inline {
                entries: [None; INLINE_WRITE_LOCATION_CAPACITY],
                len: 0,
            }
        } else {
            Self::Heap(Vec::with_capacity(capacity))
        }
    }

    fn push(&mut self, location: MemoryLocation) {
        match self {
            Self::Inline { entries, len } => {
                debug_assert!(*len < entries.len());
                entries[*len] = Some(location);
                *len += 1;
            }
            Self::Heap(entries) => entries.push(location),
        }
    }

    fn iter(&self) -> impl Iterator<Item = MemoryLocation> + '_ {
        let mut index = 0;
        std::iter::from_fn(move || {
            let location = match self {
                Self::Inline { entries, len } => {
                    if index < *len {
                        entries[index]
                    } else {
                        None
                    }
                }
                Self::Heap(entries) => entries.get(index).copied(),
            };
            index += 1;
            location
        })
    }
}

impl fmt::Display for Arm11SegmentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => formatter.write_str("text"),
            Self::ReadOnly => formatter.write_str("read-only"),
            Self::Data => formatter.write_str("data"),
            Self::Bss => formatter.write_str("bss"),
            Self::Stack => formatter.write_str("stack"),
        }
    }
}

/// Simplified 3DS virtual memory states represented by QueryMemory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Arm11MemoryState {
    Free = 0,
    Code = 4,
    Private = 5,
    Shared = 6,
}

/// Result for one contiguous region returned by the ARM11 QueryMemory service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm11MemoryRegionInfo {
    pub base_address: u32,
    pub size_bytes: u32,
    /// Bit 0 = read, bit 1 = write, bit 2 = execute.
    pub permissions: u32,
    pub state: Arm11MemoryState,
    /// PageInfo flags: bit 0 locked, bit 1 changed. No page tracking yet.
    pub page_flags: u32,
}

/// One validated ARM11 virtual-memory segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm11MemorySegment {
    /// First virtual address of the mapping.
    pub address: u32,
    /// Bytes initialized from the ExeFS code image, before page padding.
    pub initialized_size_bytes: u32,
    /// Page-aligned virtual mapping length.
    pub mapped_size_bytes: u32,
    /// Whether user-mode writes are permitted.
    pub writable: bool,
    /// Whether the segment may be used for instruction fetch.
    pub executable: bool,
}

impl Arm11MemorySegment {
    /// Returns the exclusive end address as a 64-bit value.
    ///
    /// The value 0x1_0000_0000 denotes the exclusive end of the 32-bit space.
    pub fn end_address(self) -> u64 {
        u64::from(self.address) + u64::from(self.mapped_size_bytes)
    }

    /// Returns true when this segment has no mapped pages.
    pub const fn is_empty(self) -> bool {
        self.mapped_size_bytes == 0
    }
}

/// Validated process metadata ready for the later ARM11 memory mapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm11ProcessLayout {
    /// Eight-byte application name from the NCCH System Control Info.
    pub application_title: [u8; 8],
    /// Initial instruction address, set to the start of the text segment.
    pub entry_point: u32,
    /// Executable, read-only text mapping.
    pub text: Arm11MemorySegment,
    /// Read-only, non-executable mapping.
    pub read_only: Arm11MemorySegment,
    /// Writable, non-executable initialized-data mapping.
    pub data: Arm11MemorySegment,
    /// Writable, non-executable zero-filled tail of the writable CodeSet.
    pub bss: Arm11MemorySegment,
    /// Exact BSS size in bytes from the NCCH System Control Info.
    pub bss_size_bytes: u32,
    /// Writable, non-executable main-thread stack mapping.
    pub stack: Arm11MemorySegment,
    /// Initial stack pointer at the top of the main-thread stack.
    pub initial_stack_pointer: u32,
    /// Exact main-thread stack request in bytes from the NCCH System Control Info.
    pub stack_size_bytes: u32,
    /// Page-aligned lengths used to divide ExeFS code.
    pub code_file_layout: CodeFileLayout,
}

/// Errors returned when executable metadata cannot describe a safe process layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arm11ProcessLayoutError {
    InvalidCodeFileLayout,
    EmptyTextSegment,
    SegmentImageSizeMismatch {
        segment: Arm11SegmentKind,
        expected_bytes: usize,
        actual_bytes: usize,
    },
    SegmentAddressUnaligned {
        segment: Arm11SegmentKind,
        address: u32,
    },
    SegmentRangeOverflow {
        segment: Arm11SegmentKind,
        address: u32,
        mapped_size_bytes: u32,
    },
    OverlappingSegments {
        first: Arm11SegmentKind,
        second: Arm11SegmentKind,
    },
    BssTooLarge {
        requested_size_bytes: u32,
        maximum_size_bytes: usize,
    },
    BssAllocationFailure(String),
    StackTooLarge {
        requested_size_bytes: u32,
        maximum_size_bytes: u32,
    },
    StackAllocationFailure(String),
}

impl fmt::Display for Arm11ProcessLayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCodeFileLayout => {
                formatter.write_str("NCCH code-set metadata has an invalid page layout")
            }
            Self::EmptyTextSegment => {
                formatter.write_str("ARM11 executable has an empty text segment")
            }
            Self::SegmentImageSizeMismatch {
                segment,
                expected_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "prepared {segment} segment has {actual_bytes} bytes, expected {expected_bytes}"
            ),
            Self::SegmentAddressUnaligned { segment, address } => write!(
                formatter,
                "ARM11 {segment} address 0x{address:08X} is not page-aligned"
            ),
            Self::SegmentRangeOverflow {
                segment,
                address,
                mapped_size_bytes,
            } => write!(
                formatter,
                "ARM11 {segment} range 0x{address:08X}+0x{mapped_size_bytes:X} exceeds the 32-bit address space"
            ),
            Self::OverlappingSegments { first, second } => {
                write!(formatter, "ARM11 {first} and {second} mappings overlap")
            }
            Self::BssTooLarge {
                requested_size_bytes,
                maximum_size_bytes,
            } => write!(
                formatter,
                "ARM11 BSS mapping {requested_size_bytes} bytes exceeds the {maximum_size_bytes}-byte safety limit"
            ),
            Self::BssAllocationFailure(reason) => {
                write!(formatter, "could not allocate ARM11 BSS memory: {reason}")
            }
            Self::StackTooLarge {
                requested_size_bytes,
                maximum_size_bytes,
            } => write!(
                formatter,
                "ARM11 stack size {requested_size_bytes} bytes exceeds the {maximum_size_bytes}-byte virtual stack region"
            ),
            Self::StackAllocationFailure(reason) => {
                write!(formatter, "could not allocate ARM11 stack memory: {reason}")
            }
        }
    }
}

impl std::error::Error for Arm11ProcessLayoutError {}

impl PreparedArm11Image {
    /// Validates code-set addresses and produces the process mapping description.
    ///
    /// The prepared segment vectors contain page-aligned bytes. This method
    /// checks their lengths against the extended header, verifies page-aligned
    /// virtual starts, and rejects overlapping or wrapping address ranges.
    pub fn process_layout(
        &self,
    ) -> std::result::Result<Arm11ProcessLayout, Arm11ProcessLayoutError> {
        let extended_header = &self.extended_header;
        let code_file_layout = extended_header
            .code_file_layout()
            .ok_or(Arm11ProcessLayoutError::InvalidCodeFileLayout)?;

        validate_image_segment(
            Arm11SegmentKind::Text,
            &self.text,
            code_file_layout.text_size_bytes,
        )?;
        validate_image_segment(
            Arm11SegmentKind::ReadOnly,
            &self.read_only,
            code_file_layout.read_only_size_bytes,
        )?;
        validate_image_segment(
            Arm11SegmentKind::Data,
            &self.data,
            code_file_layout.data_size_bytes,
        )?;

        if self.text.is_empty() {
            return Err(Arm11ProcessLayoutError::EmptyTextSegment);
        }

        let text = make_segment(
            Arm11SegmentKind::Text,
            extended_header.text_code_set,
            code_file_layout.text_size_bytes,
            false,
            true,
        )?;
        let read_only = make_segment(
            Arm11SegmentKind::ReadOnly,
            extended_header.read_only_code_set,
            code_file_layout.read_only_size_bytes,
            false,
            false,
        )?;
        let data = make_segment(
            Arm11SegmentKind::Data,
            extended_header.data_code_set,
            code_file_layout.data_size_bytes,
            true,
            false,
        )?;
        let bss = make_bss_segment(
            extended_header.data_code_set,
            code_file_layout.data_size_bytes,
            code_file_layout.data_memory_size_bytes,
        )?;
        if u64::from(bss.mapped_size_bytes) > MAX_BSS_MAPPING_SIZE_BYTES as u64 {
            return Err(Arm11ProcessLayoutError::BssTooLarge {
                requested_size_bytes: bss.mapped_size_bytes,
                maximum_size_bytes: MAX_BSS_MAPPING_SIZE_BYTES,
            });
        }
        let stack = make_stack_segment(extended_header.stack_size_bytes)?;

        validate_no_overlap([
            (Arm11SegmentKind::Text, text),
            (Arm11SegmentKind::ReadOnly, read_only),
            (Arm11SegmentKind::Data, data),
            (Arm11SegmentKind::Bss, bss),
            (Arm11SegmentKind::Stack, stack),
        ])?;

        Ok(Arm11ProcessLayout {
            application_title: extended_header.application_title,
            entry_point: text.address,
            text,
            read_only,
            data,
            bss,
            bss_size_bytes: extended_header.bss_size_bytes,
            stack,
            initial_stack_pointer: MAIN_STACK_TOP,
            stack_size_bytes: extended_header.stack_size_bytes,
            code_file_layout,
        })
    }
}

/// A bootstrap ARM11 address space containing the prepared code segments.
///
/// This bus maps text, read-only data, initialized writable data, the
/// zero-filled BSS tail, the main-thread stack, regular heap allocations and
/// shared-memory views. Hardware registers and privileged kernel regions are
/// not mapped here.
#[derive(Debug)]
pub struct Arm11CodeAddressSpace {
    layout: Arm11ProcessLayout,
    memory_type: Arm11MemoryType,
    /// Service ACL from the NCCH exheader; 3DSX homebrew has no exheader ACL.
    service_access_control: Option<ServiceAccessControl>,
    text: Vec<u8>,
    read_only: Vec<u8>,
    data: Vec<u8>,
    bss: Vec<u8>,
    stack: Vec<u8>,
    dynamic_regions: Vec<DynamicMemoryRegion>,
    shared_regions: Vec<SharedMemoryRegion>,
    /// Linear CPU mappings that alias the GPU-visible physical FCRAM backing.
    linear_regions: Vec<LinearMemoryRegion>,
    gpu_memory: Option<GpuPhysicalMemory>,
    /// Immutable identity cached at attach time, avoiding an RwLock lookup on
    /// every CPU access used to form a physical D-cache tag.
    gpu_memory_backing_id: Option<u64>,
    /// Shared ARM11 MPCore Snoop Control Unit registers for this system.
    mpcore_scu: Option<MpcoreScu>,
    /// Core-0 MPCore private timer, driven by the session's virtual clock.
    mpcore_private_timer: MpcorePrivateTimer,
    /// ARM11 MPCore distributor and CPU-interface state for the application core.
    mpcore_interrupt_controller: MpcoreInterruptController,
    /// Stable identities for the process' private static segments.
    static_backing_ids: [u64; 5],
    /// Write-back ARM11 data-cache lines, keyed by backing identity and offset.
    data_cache: DataCacheLines,
    /// Last ACTLR.SMP value published to the SCU per core; the CPU reports it
    /// after every instruction, so repeats must not take the SCU lock.
    published_smp_mode: [Option<bool>; 2],
    /// Fast memory mode: guest loads and stores go straight to the backing
    /// store without modelling the D-cache or write buffer (the approach used
    /// by Citra/Azahar and PPSSPP). Every agent then sees one coherent memory.
    fast_memory: bool,
    /// Set by any access to MPCore interrupt, timer or SCU registers.
    interrupt_state_touched: std::cell::Cell<bool>,
    /// Per-set least-recently-used order, oldest line first.
    data_cache_lru: Vec<VecDeque<DataCacheKey>>,
    /// FIFO of dirty D-cache lines awaiting visibility in physical memory.
    write_buffer: VecDeque<Arm11WriteBufferEntry>,
    /// Per-eight-byte exclusive monitor generations for mapped virtual memory.
    exclusive_versions: BTreeMap<u32, u64>,
    /// Index of the static segment that resolved the last address (`locate`
    /// tries it first; the layout never changes after construction).
    static_segment_hint: std::cell::Cell<u8>,
    /// Occupancy of each 0x200-byte thread-local-storage slot.
    tls_slots_in_use: Vec<bool>,
    /// Temporary IPC aliases and their surrounding unmapped guard pages.
    ipc_temporary_regions: Vec<IpcTemporaryMappingRegion>,
}

/// Snapshot of guest memory reservations and host-side allocations.
///
/// Virtual sizes describe the guest address-space layout. Sparse backing
/// bytes and cache payloads describe host allocations owned by this process;
/// allocator/map bookkeeping and unrelated host subsystems are not included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Arm11MemoryUsageReport {
    /// Capacity reserved by the host vectors for static code/data/BSS/stack.
    pub static_segment_allocated_bytes: usize,
    /// Guest-visible application-heap address space reserved by ControlMemory.
    pub application_heap_virtual_bytes: usize,
    /// All dynamic regions, including TLS pages.
    pub dynamic_region_virtual_bytes: usize,
    /// Guest virtual size of mapped shared-memory views; aliases are not deduplicated.
    pub shared_mapping_virtual_bytes: usize,
    /// Unique physical FCRAM bytes visible through linear aliases.
    /// Overlapping old/new virtual aliases are counted only once.
    pub linear_physical_allocated_bytes: usize,
    /// Number of distinct sparse backing roots referenced by this address space.
    pub unique_sparse_backing_count: usize,
    /// Materialized 4 KiB payload pages across distinct backing roots.
    pub sparse_backing_pages: usize,
    /// Bytes held by materialized sparse page payloads.
    pub sparse_backing_allocated_bytes: usize,
    /// Exclusive-monitor generation entries in shared backing roots.
    pub sparse_exclusive_monitor_entries: usize,
    /// Resident ARM11 data-cache lines.
    pub data_cache_lines: usize,
    /// Payload bytes held by resident ARM11 data-cache lines.
    pub data_cache_bytes: usize,
    /// Occupancy and reserved capacity of the bounded D-cache write buffer.
    pub write_buffer_entries: usize,
    pub write_buffer_capacity: usize,
    pub write_buffer_reserved_bytes: usize,
    /// Virtual exclusive-monitor generations local to this address space.
    pub virtual_exclusive_monitor_entries: usize,
    /// TLS slot occupancy.
    pub tls_slots_total: usize,
    pub tls_slots_in_use: usize,
    /// Number of IPC temporary mappings and physical FCRAM/VRAM pages.
    pub ipc_temporary_mapping_count: usize,
    pub gpu_physical_pages: usize,
    pub gpu_physical_allocated_bytes: usize,
}

const ADDRESS_SPACE_STATE_MAGIC: &[u8; 8] = b"ACADDRSP";
const ADDRESS_SPACE_STATE_VERSION: u16 = 2;
const ADDRESS_SPACE_STATE_MAX_BYTES: usize = 512 * 1024 * 1024;
const ADDRESS_SPACE_STATE_MAX_REGIONS: usize = 4096;
const ADDRESS_SPACE_STATE_MAX_TLS_SLOTS: usize =
    THREAD_LOCAL_STORAGE_MAX_PAGES as usize * THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE as usize;

fn address_state_write<T>(
    result: std::result::Result<T, crate::cpu::state::CpuStateError>,
) -> Result<T, SharedMemoryStateError> {
    result.map_err(|_| SharedMemoryStateError::AllocationFailed)
}

fn address_state_read<T>(
    result: std::result::Result<T, crate::cpu::state::CpuStateError>,
) -> Result<T, SharedMemoryStateError> {
    result.map_err(|_| SharedMemoryStateError::Truncated)
}

fn address_state_write_segment_kind(
    writer: &mut crate::cpu::state::StateWriter,
    kind: Arm11SegmentKind,
) -> Result<(), SharedMemoryStateError> {
    let value = match kind {
        Arm11SegmentKind::Text => 0,
        Arm11SegmentKind::ReadOnly => 1,
        Arm11SegmentKind::Data => 2,
        Arm11SegmentKind::Bss => 3,
        Arm11SegmentKind::Stack => 4,
    };
    address_state_write(writer.write_u8(value))
}

fn address_state_read_segment_kind(
    reader: &mut crate::cpu::state::StateReader<'_>,
) -> Result<Arm11SegmentKind, SharedMemoryStateError> {
    match address_state_read(reader.read_u8())? {
        0 => Ok(Arm11SegmentKind::Text),
        1 => Ok(Arm11SegmentKind::ReadOnly),
        2 => Ok(Arm11SegmentKind::Data),
        3 => Ok(Arm11SegmentKind::Bss),
        4 => Ok(Arm11SegmentKind::Stack),
        _ => Err(SharedMemoryStateError::InvalidAddressSpaceState(
            "unknown static segment kind",
        )),
    }
}

fn address_state_read_count(
    reader: &mut crate::cpu::state::StateReader<'_>,
    maximum: usize,
) -> Result<usize, SharedMemoryStateError> {
    let count = address_state_read(reader.read_u32())?;
    if count as usize > maximum {
        return Err(SharedMemoryStateError::TooManyAddressSpaceEntries(count));
    }
    Ok(count as usize)
}

fn address_state_read_static_bytes(
    reader: &mut crate::cpu::state::StateReader<'_>,
    expected: usize,
) -> Result<Vec<u8>, SharedMemoryStateError> {
    let encoded_length = address_state_read(reader.read_u32())? as usize;
    if encoded_length != expected {
        return Err(SharedMemoryStateError::InvalidAddressSpaceState(
            "static segment size does not match the loaded executable",
        ));
    }
    let source = address_state_read(reader.read_bytes(encoded_length))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(encoded_length)
        .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
    bytes.extend_from_slice(source);
    Ok(bytes)
}

fn address_state_validate_region_range(
    address: u32,
    size: usize,
) -> Result<u64, SharedMemoryStateError> {
    if address as u64 % PAGE_SIZE != 0 || size == 0 || size as u64 % PAGE_SIZE != 0 {
        return Err(SharedMemoryStateError::InvalidAddressSpaceState(
            "region address and size must be non-zero and page-aligned",
        ));
    }
    let end = u64::from(address)
        .checked_add(size as u64)
        .filter(|end| *end <= ADDRESS_SPACE_END)
        .ok_or(SharedMemoryStateError::InvalidAddressSpaceState(
            "region wraps the guest address space",
        ))?;
    Ok(end)
}

fn address_state_check_non_overlapping(
    layout: &Arm11ProcessLayout,
    dynamic: &[DynamicMemoryRegion],
    shared: &[SharedMemoryRegion],
    linear: &[LinearMemoryRegion],
) -> Result<(), SharedMemoryStateError> {
    let mut ranges = Vec::new();
    let capacity = 5usize
        .checked_add(dynamic.len())
        .and_then(|count| count.checked_add(shared.len()))
        .and_then(|count| count.checked_add(linear.len()))
        .ok_or(SharedMemoryStateError::AllocationFailed)?;
    ranges
        .try_reserve_exact(capacity)
        .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
    for segment in [
        layout.text,
        layout.read_only,
        layout.data,
        layout.bss,
        layout.stack,
    ] {
        if !segment.is_empty() {
            ranges.push((u64::from(segment.address), segment.end_address()));
        }
    }
    for region in dynamic {
        ranges.push((
            u64::from(region.address),
            u64::from(region.address) + region.len() as u64,
        ));
    }
    for region in shared {
        let length = region.backing.length;
        address_state_validate_region_range(region.address, length)?;
        ranges.push((
            u64::from(region.address),
            u64::from(region.address) + length as u64,
        ));
    }
    for region in linear {
        address_state_validate_region_range(region.virtual_base, region.size as usize)?;
        ranges.push((u64::from(region.virtual_base), region.virtual_end()));
    }
    ranges.sort_unstable_by_key(|range| range.0);
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(SharedMemoryStateError::InvalidAddressSpaceState(
            "guest virtual regions overlap",
        ));
    }
    Ok(())
}

impl Arm11CodeAddressSpace {
    /// Adds every shared backing reachable from this address space to a
    /// machine-wide catalog. Repeated aliases resolve to the same root slot.
    pub(crate) fn collect_shared_backings(
        &self,
        catalog: &mut SharedMemoryBackingCatalog,
    ) -> Result<(), SharedMemoryStateError> {
        for region in &self.dynamic_regions {
            catalog.register(region.backing.backing_ref())?;
        }
        for region in &self.shared_regions {
            catalog.register(&region.backing)?;
        }
        for key in self.data_cache.keys() {
            if let Some(line) = self.data_cache.get(key) {
                if let DataCacheTarget::Backing { backing, .. } = &line.target {
                    catalog.register(backing)?;
                }
            }
        }
        for entry in &self.write_buffer {
            if let DataCacheTarget::Backing { backing, .. } = &entry.target {
                catalog.register(backing)?;
            }
        }
        Ok(())
    }

    fn save_state_write_cache_target(
        &self,
        target: &DataCacheTarget,
        catalog: &SharedMemoryBackingCatalog,
        writer: &mut crate::cpu::state::StateWriter,
    ) -> Result<(), SharedMemoryStateError> {
        match target {
            DataCacheTarget::Static { kind, offset } => {
                address_state_write(writer.write_u8(0))?;
                address_state_write_segment_kind(writer, *kind)?;
                address_state_write(writer.write_u64(*offset as u64))?;
            }
            DataCacheTarget::Backing { backing, offset } => {
                address_state_write(writer.write_u8(1))?;
                address_state_write(writer.write_u32(catalog.slot_for(backing)?))?;
                address_state_write(writer.write_u64(backing.root_base_offset() as u64))?;
                address_state_write(writer.write_u64(backing.length as u64))?;
                address_state_write(writer.write_u64(*offset as u64))?;
            }
            DataCacheTarget::Physical { address, .. } => {
                address_state_write(writer.write_u8(2))?;
                address_state_write(writer.write_u32(*address))?;
            }
        }
        Ok(())
    }

    fn save_state_read_cache_target(
        &self,
        catalog: &SharedMemoryBackingCatalog,
        reader: &mut crate::cpu::state::StateReader<'_>,
    ) -> Result<(DataCacheKey, DataCacheTarget), SharedMemoryStateError> {
        match address_state_read(reader.read_u8())? {
            0 => {
                let kind = address_state_read_segment_kind(reader)?;
                let offset = usize::try_from(address_state_read(reader.read_u64())?)
                    .map_err(|_| SharedMemoryStateError::InvalidCacheTarget)?;
                let bytes = self.bytes_for(kind);
                if !offset.is_multiple_of(ARM11_DATA_CACHE_LINE_SIZE)
                    || offset
                        .checked_add(ARM11_DATA_CACHE_LINE_SIZE)
                        .is_none_or(|end| end > bytes.len())
                {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                let key = DataCacheKey {
                    backing_id: self.static_backing_ids[Self::static_backing_slot(kind)],
                    line_offset: offset as u64,
                };
                Ok((key, DataCacheTarget::Static { kind, offset }))
            }
            1 => {
                let slot = address_state_read(reader.read_u32())?;
                let root_offset = usize::try_from(address_state_read(reader.read_u64())?)
                    .map_err(|_| SharedMemoryStateError::InvalidCacheTarget)?;
                let view_length = usize::try_from(address_state_read(reader.read_u64())?)
                    .map_err(|_| SharedMemoryStateError::InvalidCacheTarget)?;
                let offset = usize::try_from(address_state_read(reader.read_u64())?)
                    .map_err(|_| SharedMemoryStateError::InvalidCacheTarget)?;
                let backing = catalog.view(slot, root_offset, view_length)?;
                let root_line = backing
                    .root_offset(offset)
                    .filter(|root| root.is_multiple_of(ARM11_DATA_CACHE_LINE_SIZE))
                    .ok_or(SharedMemoryStateError::InvalidCacheTarget)?;
                if offset
                    .checked_add(ARM11_DATA_CACHE_LINE_SIZE)
                    .is_none_or(|end| end > backing.length)
                {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                let key = DataCacheKey {
                    backing_id: backing.backing_id(),
                    line_offset: root_line as u64,
                };
                Ok((key, DataCacheTarget::Backing { backing, offset }))
            }
            2 => {
                let address = address_state_read(reader.read_u32())?;
                let memory = self
                    .gpu_memory
                    .as_ref()
                    .ok_or(SharedMemoryStateError::MissingPhysicalMemory)?
                    .clone();
                let backing_id = self
                    .gpu_memory_backing_id
                    .ok_or(SharedMemoryStateError::MissingPhysicalMemory)?;
                if !address.is_multiple_of(ARM11_DATA_CACHE_LINE_SIZE as u32)
                    || memory
                        .validate_range(address, ARM11_DATA_CACHE_LINE_SIZE)
                        .is_err()
                {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                let key = DataCacheKey {
                    backing_id,
                    line_offset: u64::from(address),
                };
                Ok((key, DataCacheTarget::Physical { memory, address }))
            }
            _ => Err(SharedMemoryStateError::InvalidCacheTarget),
        }
    }

    /// Encodes all mutable guest-visible process memory state. Immutable layout
    /// metadata comes from the already validated boot image; host identity IDs
    /// and lookup hints are rebuilt when the state is restored.
    pub(crate) fn encode_memory_state(
        &self,
        catalog: &SharedMemoryBackingCatalog,
    ) -> Result<Vec<u8>, SharedMemoryStateError> {
        use crate::cpu::state::StateWriter;
        let mut writer = StateWriter::with_capacity(16 * 1024)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        address_state_write(writer.write_bytes(ADDRESS_SPACE_STATE_MAGIC))?;
        address_state_write(writer.write_u16(ADDRESS_SPACE_STATE_VERSION))?;
        address_state_write(writer.write_u16(0))?;
        for bytes in [
            self.text.as_slice(),
            self.read_only.as_slice(),
            self.data.as_slice(),
            self.bss.as_slice(),
            self.stack.as_slice(),
        ] {
            let length = u32::try_from(bytes.len()).map_err(|_| {
                SharedMemoryStateError::InvalidAddressSpaceState(
                    "static segment exceeds the format limit",
                )
            })?;
            address_state_write(writer.write_u32(length))?;
            address_state_write(writer.write_bytes(bytes))?;
        }

        address_state_write(writer.write_bool(self.fast_memory))?;
        address_state_write(writer.write_bool(self.interrupt_state_touched.get()))?;
        for mode in self.published_smp_mode {
            match mode {
                None => address_state_write(writer.write_u8(0))?,
                Some(false) => address_state_write(writer.write_u8(1))?,
                Some(true) => address_state_write(writer.write_u8(2))?,
            }
        }

        if self.dynamic_regions.len() > ADDRESS_SPACE_STATE_MAX_REGIONS
            || self.shared_regions.len() > ADDRESS_SPACE_STATE_MAX_REGIONS
            || self.linear_regions.len() > ADDRESS_SPACE_STATE_MAX_REGIONS
            || self.ipc_temporary_regions.len() > ADDRESS_SPACE_STATE_MAX_REGIONS
            || self.exclusive_versions.len() > ADDRESS_SPACE_STATE_MAX_REGIONS * 1024
            || self.tls_slots_in_use.len() > ADDRESS_SPACE_STATE_MAX_TLS_SLOTS
        {
            return Err(SharedMemoryStateError::TooManyAddressSpaceEntries(
                u32::try_from(
                    self.dynamic_regions.len()
                        + self.shared_regions.len()
                        + self.linear_regions.len()
                        + self.ipc_temporary_regions.len(),
                )
                .unwrap_or(u32::MAX),
            ));
        }
        address_state_write(writer.write_u32(self.dynamic_regions.len() as u32))?;
        for region in &self.dynamic_regions {
            address_state_validate_region_range(region.address, region.len())?;
            if region.permissions > 3 {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "dynamic-region permissions are invalid",
                ));
            }
            address_state_write(writer.write_u32(region.address))?;
            let backing = region.backing.backing_ref();
            let backing_kind = match &region.backing {
                DynamicMemoryBacking::Private(_) => 0,
                DynamicMemoryBacking::Shared(_) => 1,
            };
            address_state_write(writer.write_u8(backing_kind))?;
            address_state_write(writer.write_u32(catalog.slot_for(backing)?))?;
            address_state_write(writer.write_u64(backing.root_base_offset() as u64))?;
            address_state_write(writer.write_u64(backing.length as u64))?;
            address_state_write(writer.write_u32(region.permissions))?;
        }

        address_state_write(writer.write_u32(self.shared_regions.len() as u32))?;
        for region in &self.shared_regions {
            address_state_validate_region_range(region.address, region.backing.length)?;
            if region.permissions > 3 {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "shared-region permissions are invalid",
                ));
            }
            address_state_write(writer.write_u32(region.address))?;
            address_state_write(writer.write_u32(catalog.slot_for(&region.backing)?))?;
            address_state_write(writer.write_u64(region.backing.root_base_offset() as u64))?;
            address_state_write(writer.write_u64(region.backing.length as u64))?;
            address_state_write(writer.write_u32(region.permissions))?;
        }

        address_state_write(writer.write_u32(self.linear_regions.len() as u32))?;
        for region in &self.linear_regions {
            address_state_validate_region_range(region.virtual_base, region.size as usize)?;
            if !region.physical_base.is_multiple_of(PAGE_SIZE as u32)
                || !region.size.is_multiple_of(PAGE_SIZE as u32)
            {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "linear mapping is not page-aligned",
                ));
            }
            address_state_write(writer.write_u32(region.virtual_base))?;
            address_state_write(writer.write_u32(region.physical_base))?;
            address_state_write(writer.write_u32(region.size))?;
        }

        // Preserve each set's LRU order together with the dirty cache-line bytes.
        if self.data_cache_lru.len() != ARM11_DATA_CACHE_SET_COUNT {
            return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                "D-cache LRU set count is inconsistent",
            ));
        }
        let mut encoded_cache_lines = 0usize;
        for (set_index, lru) in self.data_cache_lru.iter().enumerate() {
            if lru.len() > ARM11_DATA_CACHE_WAYS
                || lru.len() != self.data_cache.sets[set_index].len()
            {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "D-cache lines and LRU metadata disagree",
                ));
            }
            encoded_cache_lines += lru.len();
        }
        if encoded_cache_lines > ARM11_DATA_CACHE_CAPACITY_BYTES / ARM11_DATA_CACHE_LINE_SIZE {
            return Err(SharedMemoryStateError::TooManyCacheLines(
                encoded_cache_lines as u32,
            ));
        }
        address_state_write(writer.write_u32(encoded_cache_lines as u32))?;
        for (set_index, lru) in self.data_cache_lru.iter().enumerate() {
            address_state_write(writer.write_u8(lru.len() as u8))?;
            for key in lru {
                if Self::cache_set_index(*key) != set_index {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                let line = self
                    .data_cache
                    .get(key)
                    .ok_or(SharedMemoryStateError::InvalidCacheTarget)?;
                address_state_write(writer.write_u64(key.line_offset))?;
                self.save_state_write_cache_target(&line.target, catalog, &mut writer)?;
                address_state_write(writer.write_bool(line.dirty))?;
                address_state_write(writer.write_bytes(&line.bytes))?;
            }
        }

        if self.write_buffer.len() > ARM11_WRITE_BUFFER_CAPACITY {
            return Err(SharedMemoryStateError::TooManyWriteBufferEntries(
                self.write_buffer.len() as u32,
            ));
        }
        address_state_write(writer.write_u32(self.write_buffer.len() as u32))?;
        for entry in &self.write_buffer {
            let key = self.cache_key_for_snapshot_target(&entry.target)?;
            if key != entry.key
                || entry.length == 0
                || usize::from(entry.start) + usize::from(entry.length) > ARM11_DATA_CACHE_LINE_SIZE
            {
                return Err(SharedMemoryStateError::InvalidCacheTarget);
            }
            self.save_state_write_cache_target(&entry.target, catalog, &mut writer)?;
            address_state_write(writer.write_u8(entry.start))?;
            address_state_write(writer.write_u8(entry.length))?;
            address_state_write(writer.write_bytes(&entry.bytes))?;
        }

        address_state_write(writer.write_u32(self.exclusive_versions.len() as u32))?;
        for (address, generation) in &self.exclusive_versions {
            if !address.is_multiple_of(crate::memory::EXCLUSIVE_RESERVATION_GRANULE) {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "virtual exclusive-monitor address is unaligned",
                ));
            }
            address_state_write(writer.write_u32(*address))?;
            address_state_write(writer.write_u64(*generation))?;
        }

        address_state_write(writer.write_u32(self.tls_slots_in_use.len() as u32))?;
        for occupied in &self.tls_slots_in_use {
            address_state_write(writer.write_bool(*occupied))?;
        }

        address_state_write(writer.write_u32(self.ipc_temporary_regions.len() as u32))?;
        for region in &self.ipc_temporary_regions {
            for value in [
                region.alias_address,
                region.mapping_base,
                region.mapping_size,
                region.reservation_base,
                region.reservation_size,
            ] {
                address_state_write(writer.write_u32(value))?;
            }
        }

        let encoded = writer.into_bytes();
        if encoded.len() > ADDRESS_SPACE_STATE_MAX_BYTES {
            return Err(SharedMemoryStateError::AllocationFailed);
        }
        Ok(encoded)
    }

    /// Restores mutable process-memory state only after every region, backing,
    /// cache line and pending writeback has been decoded and validated.
    pub(crate) fn restore_memory_state(
        &mut self,
        encoded: &[u8],
        catalog: &SharedMemoryBackingCatalog,
    ) -> Result<(), SharedMemoryStateError> {
        use crate::cpu::state::StateReader;
        if encoded.len() > ADDRESS_SPACE_STATE_MAX_BYTES {
            return Err(SharedMemoryStateError::AllocationFailed);
        }
        let mut reader = StateReader::new(encoded);
        if address_state_read(reader.read_bytes(8))? != ADDRESS_SPACE_STATE_MAGIC {
            return Err(SharedMemoryStateError::InvalidMagic);
        }
        let version = address_state_read(reader.read_u16())?;
        if version == 0 || version > ADDRESS_SPACE_STATE_VERSION {
            return Err(SharedMemoryStateError::UnsupportedVersion(version));
        }
        if address_state_read(reader.read_u16())? != 0 {
            return Err(SharedMemoryStateError::UnsupportedFlags(1));
        }

        let text = address_state_read_static_bytes(&mut reader, self.text.len())?;
        let read_only = address_state_read_static_bytes(&mut reader, self.read_only.len())?;
        let data = address_state_read_static_bytes(&mut reader, self.data.len())?;
        let bss = address_state_read_static_bytes(&mut reader, self.bss.len())?;
        let stack = address_state_read_static_bytes(&mut reader, self.stack.len())?;
        let fast_memory = address_state_read(reader.read_bool())?;
        let interrupt_state_touched = address_state_read(reader.read_bool())?;
        let mut published_smp_mode = [None; 2];
        for mode in &mut published_smp_mode {
            *mode = match address_state_read(reader.read_u8())? {
                0 => None,
                1 => Some(false),
                2 => Some(true),
                _ => {
                    return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                        "SMP publication state is invalid",
                    ));
                }
            };
        }

        let dynamic_count = address_state_read_count(&mut reader, ADDRESS_SPACE_STATE_MAX_REGIONS)?;
        let mut dynamic_regions = Vec::new();
        dynamic_regions
            .try_reserve_exact(dynamic_count)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        for _ in 0..dynamic_count {
            let address = address_state_read(reader.read_u32())?;
            let kind = address_state_read(reader.read_u8())?;
            let slot = address_state_read(reader.read_u32())?;
            let root_offset = usize::try_from(address_state_read(reader.read_u64())?)
                .map_err(|_| SharedMemoryStateError::InvalidCatalogEntry)?;
            let length = usize::try_from(address_state_read(reader.read_u64())?)
                .map_err(|_| SharedMemoryStateError::InvalidCatalogEntry)?;
            let permissions = if version >= 2 {
                address_state_read(reader.read_u32())?
            } else {
                3
            };
            if permissions > 3 {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "dynamic-region permissions are invalid",
                ));
            }
            address_state_validate_region_range(address, length)?;
            let backing = catalog.view(slot, root_offset, length)?;
            let backing = match kind {
                0 => DynamicMemoryBacking::Private(backing),
                1 => DynamicMemoryBacking::Shared(backing),
                _ => {
                    return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                        "dynamic backing kind is invalid",
                    ));
                }
            };
            dynamic_regions.push(DynamicMemoryRegion {
                address,
                backing,
                permissions,
            });
        }

        let shared_count = address_state_read_count(&mut reader, ADDRESS_SPACE_STATE_MAX_REGIONS)?;
        let mut shared_regions = Vec::new();
        shared_regions
            .try_reserve_exact(shared_count)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        for _ in 0..shared_count {
            let address = address_state_read(reader.read_u32())?;
            let slot = address_state_read(reader.read_u32())?;
            let root_offset = usize::try_from(address_state_read(reader.read_u64())?)
                .map_err(|_| SharedMemoryStateError::InvalidCatalogEntry)?;
            let length = usize::try_from(address_state_read(reader.read_u64())?)
                .map_err(|_| SharedMemoryStateError::InvalidCatalogEntry)?;
            let permissions = address_state_read(reader.read_u32())?;
            address_state_validate_region_range(address, length)?;
            if permissions > 3 {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "shared-region permissions are invalid",
                ));
            }
            shared_regions.push(SharedMemoryRegion {
                address,
                backing: catalog.view(slot, root_offset, length)?,
                permissions,
            });
        }

        let linear_count = address_state_read_count(&mut reader, ADDRESS_SPACE_STATE_MAX_REGIONS)?;
        let mut linear_regions = Vec::new();
        linear_regions
            .try_reserve_exact(linear_count)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        for _ in 0..linear_count {
            let virtual_base = address_state_read(reader.read_u32())?;
            let physical_base = address_state_read(reader.read_u32())?;
            let size = address_state_read(reader.read_u32())?;
            address_state_validate_region_range(virtual_base, size as usize)?;
            if !physical_base.is_multiple_of(PAGE_SIZE as u32)
                || !size.is_multiple_of(PAGE_SIZE as u32)
            {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "linear mapping is not page-aligned",
                ));
            }
            let memory = self
                .gpu_memory
                .as_ref()
                .ok_or(SharedMemoryStateError::MissingPhysicalMemory)?;
            memory
                .validate_range(physical_base, size as usize)
                .map_err(|_| {
                    SharedMemoryStateError::InvalidAddressSpaceState(
                        "linear mapping physical range is invalid",
                    )
                })?;
            if !memory.has_linear_fcram_allocation(physical_base, size) {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "linear mapping has no matching FCRAM allocation",
                ));
            }
            let virtual_end = u64::from(virtual_base) + u64::from(size);
            let expected_physical = if u64::from(virtual_base) >= u64::from(OLD_LINEAR_BASE)
                && virtual_end <= OLD_LINEAR_END
            {
                u64::from(virtual_base) + 0x0C00_0000
            } else if u64::from(virtual_base) >= u64::from(NEW_LINEAR_BASE)
                && virtual_end <= NEW_LINEAR_END
            {
                u64::from(FCRAM_BASE) + (u64::from(virtual_base) - u64::from(NEW_LINEAR_BASE))
            } else {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "linear mapping virtual range is outside the supported windows",
                ));
            };
            if expected_physical != u64::from(physical_base) {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "linear mapping virtual and physical ranges disagree",
                ));
            }
            linear_regions.push(LinearMemoryRegion {
                virtual_base,
                physical_base,
                size,
            });
        }

        address_state_check_non_overlapping(
            &self.layout,
            &dynamic_regions,
            &shared_regions,
            &linear_regions,
        )?;

        let declared_cache_lines = address_state_read_count(
            &mut reader,
            ARM11_DATA_CACHE_CAPACITY_BYTES / ARM11_DATA_CACHE_LINE_SIZE,
        )?;
        let mut data_cache = DataCacheLines::new();
        let mut data_cache_lru = (0..ARM11_DATA_CACHE_SET_COUNT)
            .map(|_| VecDeque::with_capacity(ARM11_DATA_CACHE_WAYS))
            .collect::<Vec<_>>();
        let mut restored_cache_lines = 0usize;
        for set_index in 0..ARM11_DATA_CACHE_SET_COUNT {
            let count = address_state_read(reader.read_u8())? as usize;
            if count > ARM11_DATA_CACHE_WAYS {
                return Err(SharedMemoryStateError::TooManyCacheLines(count as u32));
            }
            for _ in 0..count {
                let (key, target) = self.save_state_read_cache_target(catalog, &mut reader)?;
                if Self::cache_set_index(key) != set_index || data_cache.get(&key).is_some() {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                let dirty = address_state_read(reader.read_bool())?;
                let source = address_state_read(reader.read_bytes(ARM11_DATA_CACHE_LINE_SIZE))?;
                let mut bytes = [0u8; ARM11_DATA_CACHE_LINE_SIZE];
                bytes.copy_from_slice(source);
                data_cache.insert(
                    key,
                    Arm11DataCacheLine {
                        bytes,
                        dirty,
                        target,
                    },
                );
                data_cache_lru[set_index].push_back(key);
                restored_cache_lines += 1;
            }
        }
        if restored_cache_lines != declared_cache_lines {
            return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                "D-cache line count does not match per-set records",
            ));
        }

        let write_buffer_count =
            address_state_read_count(&mut reader, ARM11_WRITE_BUFFER_CAPACITY)?;
        let mut write_buffer = VecDeque::with_capacity(ARM11_WRITE_BUFFER_CAPACITY);
        for _ in 0..write_buffer_count {
            let (key, target) = self.save_state_read_cache_target(catalog, &mut reader)?;
            let start = address_state_read(reader.read_u8())?;
            let length = address_state_read(reader.read_u8())?;
            if length == 0 || usize::from(start) + usize::from(length) > ARM11_DATA_CACHE_LINE_SIZE
            {
                return Err(SharedMemoryStateError::InvalidCacheTarget);
            }
            let source = address_state_read(reader.read_bytes(ARM11_DATA_CACHE_LINE_SIZE))?;
            let mut bytes = [0u8; ARM11_DATA_CACHE_LINE_SIZE];
            bytes.copy_from_slice(source);
            write_buffer.push_back(Arm11WriteBufferEntry {
                key,
                target,
                start,
                bytes,
                length,
            });
        }

        let exclusive_count =
            address_state_read_count(&mut reader, ADDRESS_SPACE_STATE_MAX_REGIONS * 1024)?;
        let mut exclusive_versions = BTreeMap::new();
        for _ in 0..exclusive_count {
            let address = address_state_read(reader.read_u32())?;
            let generation = address_state_read(reader.read_u64())?;
            if !address.is_multiple_of(crate::memory::EXCLUSIVE_RESERVATION_GRANULE)
                || exclusive_versions.insert(address, generation).is_some()
            {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "virtual exclusive monitor is invalid or duplicated",
                ));
            }
        }

        let tls_count = address_state_read_count(&mut reader, ADDRESS_SPACE_STATE_MAX_TLS_SLOTS)?;
        let mut tls_slots_in_use = Vec::new();
        tls_slots_in_use
            .try_reserve_exact(tls_count)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        for _ in 0..tls_count {
            tls_slots_in_use.push(address_state_read(reader.read_bool())?);
        }

        let ipc_count = address_state_read_count(&mut reader, ADDRESS_SPACE_STATE_MAX_REGIONS)?;
        let mut ipc_temporary_regions = Vec::new();
        ipc_temporary_regions
            .try_reserve_exact(ipc_count)
            .map_err(|_| SharedMemoryStateError::AllocationFailed)?;
        for _ in 0..ipc_count {
            let region = IpcTemporaryMappingRegion {
                alias_address: address_state_read(reader.read_u32())?,
                mapping_base: address_state_read(reader.read_u32())?,
                mapping_size: address_state_read(reader.read_u32())?,
                reservation_base: address_state_read(reader.read_u32())?,
                reservation_size: address_state_read(reader.read_u32())?,
            };
            let reservation_end = u64::from(region.reservation_base)
                .checked_add(u64::from(region.reservation_size))
                .ok_or(SharedMemoryStateError::InvalidAddressSpaceState(
                    "IPC guard reservation wraps the address space",
                ))?;
            if !region.mapping_base.is_multiple_of(PAGE_SIZE as u32)
                || !region.reservation_base.is_multiple_of(PAGE_SIZE as u32)
                || !region.mapping_size.is_multiple_of(PAGE_SIZE as u32)
                || region.reservation_size
                    != region
                        .mapping_size
                        .checked_add(2 * PAGE_SIZE as u32)
                        .unwrap_or(0)
                || region.alias_address < region.mapping_base
                || u64::from(region.alias_address)
                    >= u64::from(region.mapping_base) + u64::from(region.mapping_size)
                || reservation_end > IPC_TEMPORARY_BUFFER_END
                || !shared_regions.iter().any(|shared| {
                    shared.address == region.mapping_base
                        && shared.backing.length == region.mapping_size as usize
                        && shared.permissions == 3
                })
            {
                return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                    "IPC temporary mapping metadata is inconsistent",
                ));
            }
            ipc_temporary_regions.push(region);
        }
        reader
            .finish()
            .map_err(|_| SharedMemoryStateError::TrailingData)?;

        // All data above is staged; the live address space is changed only here.
        self.text = text;
        self.read_only = read_only;
        self.data = data;
        self.bss = bss;
        self.stack = stack;
        self.fast_memory = fast_memory;
        self.interrupt_state_touched.set(interrupt_state_touched);
        self.published_smp_mode = published_smp_mode;
        self.dynamic_regions = dynamic_regions;
        self.shared_regions = shared_regions;
        self.linear_regions = linear_regions;
        self.data_cache = data_cache;
        self.data_cache_lru = data_cache_lru;
        self.write_buffer = write_buffer;
        self.exclusive_versions = exclusive_versions;
        self.tls_slots_in_use = tls_slots_in_use;
        self.ipc_temporary_regions = ipc_temporary_regions;
        self.static_segment_hint.set(4);
        Ok(())
    }

    pub(crate) fn encode_save_state_section(
        &self,
        catalog: &SharedMemoryBackingCatalog,
    ) -> Result<crate::save_state::SaveStateSection, SharedMemoryStateError> {
        crate::save_state::SaveStateSection::new(
            crate::save_state::section_id::PROCESS_MEMORY,
            ADDRESS_SPACE_STATE_VERSION,
            self.encode_memory_state(catalog)?,
        )
        .map_err(|_| {
            SharedMemoryStateError::InvalidAddressSpaceState(
                "could not wrap process memory section",
            )
        })
    }

    pub(crate) fn restore_save_state_section(
        &mut self,
        section: &crate::save_state::SaveStateSection,
        catalog: &SharedMemoryBackingCatalog,
    ) -> Result<(), SharedMemoryStateError> {
        if section.id() != crate::save_state::section_id::PROCESS_MEMORY
            || section.version() == 0
            || section.version() > ADDRESS_SPACE_STATE_VERSION
        {
            return Err(SharedMemoryStateError::InvalidAddressSpaceState(
                "process-memory section ID or version is unsupported",
            ));
        }
        self.restore_memory_state(section.payload(), catalog)
    }

    fn cache_key_for_snapshot_target(
        &self,
        target: &DataCacheTarget,
    ) -> Result<DataCacheKey, SharedMemoryStateError> {
        match target {
            DataCacheTarget::Static { kind, offset } => {
                if !offset.is_multiple_of(ARM11_DATA_CACHE_LINE_SIZE)
                    || offset
                        .checked_add(ARM11_DATA_CACHE_LINE_SIZE)
                        .is_none_or(|end| end > self.bytes_for(*kind).len())
                {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                Ok(DataCacheKey {
                    backing_id: self.static_backing_ids[Self::static_backing_slot(*kind)],
                    line_offset: *offset as u64,
                })
            }
            DataCacheTarget::Backing { backing, offset } => {
                let root_offset = backing
                    .root_offset(*offset)
                    .filter(|root| root.is_multiple_of(ARM11_DATA_CACHE_LINE_SIZE))
                    .ok_or(SharedMemoryStateError::InvalidCacheTarget)?;
                if offset
                    .checked_add(ARM11_DATA_CACHE_LINE_SIZE)
                    .is_none_or(|end| end > backing.length)
                {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                Ok(DataCacheKey {
                    backing_id: backing.backing_id(),
                    line_offset: root_offset as u64,
                })
            }
            DataCacheTarget::Physical { memory, address } => {
                if !address.is_multiple_of(ARM11_DATA_CACHE_LINE_SIZE as u32)
                    || memory
                        .validate_range(*address, ARM11_DATA_CACHE_LINE_SIZE)
                        .is_err()
                    || self.gpu_memory_backing_id.is_none()
                {
                    return Err(SharedMemoryStateError::InvalidCacheTarget);
                }
                Ok(DataCacheKey {
                    backing_id: self.gpu_memory_backing_id.unwrap(),
                    line_offset: u64::from(*address),
                })
            }
        }
    }

    /// Creates a segmented address space from a verified prepared executable.
    pub fn from_prepared(
        prepared: PreparedArm11Image,
    ) -> std::result::Result<Self, Arm11ProcessLayoutError> {
        let memory_type = prepared.extended_header.arm11_memory_type();
        let service_access_control = Some(ServiceAccessControl::parse(
            &prepared.extended_header.access_control_info,
        ));
        let layout = prepared.process_layout()?;
        let bss_size = usize::try_from(layout.bss.mapped_size_bytes)
            .map_err(|_| Arm11ProcessLayoutError::InvalidCodeFileLayout)?;
        let mut bss = Vec::new();
        bss.try_reserve_exact(bss_size)
            .map_err(|error| Arm11ProcessLayoutError::BssAllocationFailure(error.to_string()))?;
        bss.resize(bss_size, 0);

        let stack_size = usize::try_from(layout.stack.mapped_size_bytes).map_err(|_| {
            Arm11ProcessLayoutError::StackTooLarge {
                requested_size_bytes: layout.stack.mapped_size_bytes,
                maximum_size_bytes: MAIN_STACK_TOP,
            }
        })?;
        let mut stack = Vec::new();
        stack
            .try_reserve_exact(stack_size)
            .map_err(|error| Arm11ProcessLayoutError::StackAllocationFailure(error.to_string()))?;
        stack.resize(stack_size, 0);

        Ok(Self {
            layout,
            memory_type,
            service_access_control,
            text: prepared.text,
            read_only: prepared.read_only,
            data: prepared.data,
            bss,
            stack,
            dynamic_regions: Vec::new(),
            shared_regions: Vec::new(),
            linear_regions: Vec::new(),
            gpu_memory: None,
            gpu_memory_backing_id: None,
            mpcore_scu: None,
            mpcore_private_timer: MpcorePrivateTimer::default(),
            mpcore_interrupt_controller: MpcoreInterruptController::default(),
            static_backing_ids: std::array::from_fn(|_| crate::memory::next_memory_backing_id()),
            data_cache: DataCacheLines::new(),
            published_smp_mode: [None; 2],
            fast_memory: false,
            interrupt_state_touched: std::cell::Cell::new(false),
            data_cache_lru: (0..ARM11_DATA_CACHE_SET_COUNT)
                .map(|_| VecDeque::with_capacity(ARM11_DATA_CACHE_WAYS))
                .collect(),
            write_buffer: VecDeque::with_capacity(ARM11_WRITE_BUFFER_CAPACITY),
            exclusive_versions: BTreeMap::new(),
            static_segment_hint: std::cell::Cell::new(4),
            tls_slots_in_use: Vec::new(),
            ipc_temporary_regions: Vec::new(),
        })
    }

    /// Creates an ARM11 address space from a validated, relocated 3DSX process.
    ///
    /// 3DSX Data and its BSS tail are one contiguous mapping. The adapter keeps
    /// that backing as a single writable Data region so byte addresses and
    /// relocation results remain unchanged; the exact BSS boundary and size
    /// remain available in the process layout metadata.
    pub fn from_3dsx_prepared(
        prepared: ThreeDsxPreparedProcess,
    ) -> std::result::Result<Self, Arm11ProcessLayoutError> {
        prepared
            .validate()
            .map_err(|_| Arm11ProcessLayoutError::InvalidCodeFileLayout)?;

        let (source_layout, text, read_only, data, stack) = prepared.into_runtime_parts();
        let to_segment = |segment: ThreeDsxProcessMemorySegment| Arm11MemorySegment {
            address: segment.address,
            initialized_size_bytes: segment.initialized_size_bytes,
            mapped_size_bytes: segment.mapped_size_bytes,
            writable: segment.writable,
            executable: segment.executable,
        };

        let text_segment = to_segment(source_layout.code);
        let read_only_segment = to_segment(source_layout.read_only);
        let data_segment = to_segment(source_layout.data);
        let stack_segment = to_segment(source_layout.stack);
        let initialized_total = usize::try_from(source_layout.code.initialized_size_bytes)
            .ok()
            .and_then(|code_size| {
                usize::try_from(source_layout.read_only.initialized_size_bytes)
                    .ok()
                    .and_then(|read_only_size| code_size.checked_add(read_only_size))
            })
            .and_then(|total| {
                usize::try_from(source_layout.data.initialized_size_bytes)
                    .ok()
                    .and_then(|data_size| total.checked_add(data_size))
            })
            .ok_or(Arm11ProcessLayoutError::InvalidCodeFileLayout)?;
        let code_file_layout = CodeFileLayout {
            text_size_bytes: source_layout.code.initialized_size_bytes as usize,
            read_only_size_bytes: source_layout.read_only.initialized_size_bytes as usize,
            data_size_bytes: source_layout.data.initialized_size_bytes as usize,
            data_memory_size_bytes: source_layout.data.mapped_size_bytes as usize,
            total_size_bytes: initialized_total,
        };

        let layout = Arm11ProcessLayout {
            // The 3DSX header has no application-title identifier.
            application_title: [0; 8],
            entry_point: source_layout.entry_point,
            text: text_segment,
            read_only: read_only_segment,
            data: data_segment,
            // BSS bytes are already contiguous inside the writable Data buffer.
            bss: Arm11MemorySegment {
                address: source_layout.data.address,
                initialized_size_bytes: 0,
                mapped_size_bytes: 0,
                writable: true,
                executable: false,
            },
            bss_size_bytes: source_layout.bss_size_bytes,
            stack: stack_segment,
            initial_stack_pointer: source_layout.initial_stack_pointer,
            stack_size_bytes: source_layout.stack.mapped_size_bytes,
            code_file_layout,
        };

        Ok(Self {
            layout,
            memory_type: Arm11MemoryType::Application,
            service_access_control: None,
            text,
            read_only,
            data,
            bss: Vec::new(),
            stack,
            dynamic_regions: Vec::new(),
            shared_regions: Vec::new(),
            linear_regions: Vec::new(),
            gpu_memory: None,
            gpu_memory_backing_id: None,
            mpcore_scu: None,
            mpcore_private_timer: MpcorePrivateTimer::default(),
            mpcore_interrupt_controller: MpcoreInterruptController::default(),
            static_backing_ids: std::array::from_fn(|_| crate::memory::next_memory_backing_id()),
            data_cache: DataCacheLines::new(),
            published_smp_mode: [None; 2],
            fast_memory: false,
            interrupt_state_touched: std::cell::Cell::new(false),
            data_cache_lru: (0..ARM11_DATA_CACHE_SET_COUNT)
                .map(|_| VecDeque::with_capacity(ARM11_DATA_CACHE_WAYS))
                .collect(),
            write_buffer: VecDeque::with_capacity(ARM11_WRITE_BUFFER_CAPACITY),
            exclusive_versions: BTreeMap::new(),
            static_segment_hint: std::cell::Cell::new(4),
            tls_slots_in_use: Vec::new(),
            ipc_temporary_regions: Vec::new(),
        })
    }

    /// Returns the validated process metadata associated with this address space.
    pub const fn layout(&self) -> &Arm11ProcessLayout {
        &self.layout
    }

    /// Returns the ARM11 FCRAM region assigned to this process.
    pub const fn memory_type(&self) -> Arm11MemoryType {
        self.memory_type
    }

    /// Checks whether the NCCH exheader permits access to a service.
    ///
    /// A 3DSX homebrew process has no NCCH exheader and therefore no ACL to
    /// enforce in this bootstrap. NCCH processes are restricted to the
    /// exact service names present in their parsed Access Control Info.
    pub fn allows_service_access(&self, service_name: &[u8]) -> bool {
        self.service_access_control
            .as_ref()
            .is_none_or(|access| access.allows_bytes(service_name))
    }

    /// Returns the parsed NCCH service ACL, or None for 3DSX homebrew.
    pub fn service_access_control(&self) -> Option<&ServiceAccessControl> {
        self.service_access_control.as_ref()
    }

    /// Initializes the ARM11 core's entry PC and main-thread stack pointer.
    ///
    /// This does not configure privileged CPU modes, exception vectors,
    /// coprocessors, scheduler state, or peripheral registers.
    pub fn initialize_core(&self, core: &mut Arm11Core) {
        core.set_pc(self.layout.entry_point);
        core.regs[SP] = self.layout.initial_stack_pointer;
    }

    /// Attaches the shared physical backing used by linear CPU mappings and devices.
    pub fn attach_gpu_memory(&mut self, memory: GpuPhysicalMemory) {
        self.gpu_memory_backing_id = Some(memory.backing_id());
        self.gpu_memory = Some(memory);
    }

    /// Attaches the system-wide MPCore SCU peripheral visible to this process.
    pub fn attach_mpcore_scu(&mut self, scu: MpcoreScu) {
        self.mpcore_scu = Some(scu);
        self.published_smp_mode = [None; 2];
    }

    /// Advances the application core's MPCore private timer using virtual time.
    ///
    /// The default Old 3DS private-timer input clock is half the CPU clock.
    /// Timer expiration is sampled by the private interrupt distributor as PPI 29.
    pub fn advance_mpcore_private_timer(&mut self, elapsed_ns: u64) {
        self.mpcore_private_timer
            .advance_time(elapsed_ns, MPCORE_TIMER_DEFAULT_CLOCK_HZ);
        self.sync_mpcore_timer_interrupt();
    }

    /// Returns the IRQ level delivered by the MPCore distributor to CPU0.
    pub fn mpcore_irq_asserted(&self) -> bool {
        self.mpcore_interrupt_controller.irq_asserted()
    }

    fn sync_mpcore_timer_interrupt(&mut self) {
        self.mpcore_interrupt_controller
            .set_private_timer_irq_level(self.mpcore_private_timer.irq_asserted());
    }

    /// Offset inside the 8 KiB MPCore private window. Every register window
    /// lies inside it, so one compare rejects ordinary memory accesses.
    #[inline]
    fn mpcore_private_offset(&self, address: u32) -> Option<u32> {
        let offset = address.wrapping_sub(crate::cpu::MPCORE_PRIVATE_MEMORY_BASE);
        (offset < crate::cpu::MPCORE_PRIVATE_MEMORY_SIZE).then_some(offset)
    }

    fn mpcore_private_timer_register_offset(&self, address: u32) -> Option<u32> {
        let private_offset = self.mpcore_private_offset(address)?;
        let timer_offset = if (MPCORE_PRIVATE_TIMER_OFFSET
            ..MPCORE_PRIVATE_TIMER_OFFSET + MPCORE_PRIVATE_TIMER_REGISTER_SIZE)
            .contains(&private_offset)
        {
            private_offset - MPCORE_PRIVATE_TIMER_OFFSET
        } else if (MPCORE_PRIVATE_TIMER_CPU0_ALIAS_OFFSET
            ..MPCORE_PRIVATE_TIMER_CPU0_ALIAS_OFFSET + MPCORE_PRIVATE_TIMER_REGISTER_SIZE)
            .contains(&private_offset)
        {
            // CPU0's fixed private-timer alias at 0x17E00700.
            private_offset - MPCORE_PRIVATE_TIMER_CPU0_ALIAS_OFFSET
        } else {
            return None;
        };
        Some(timer_offset)
    }

    fn mpcore_cpu_interface_register_offset(&self, address: u32) -> Option<u32> {
        let private_offset = self.mpcore_private_offset(address)?;
        if (MPCORE_CPU_INTERFACE_CURRENT_OFFSET
            ..MPCORE_CPU_INTERFACE_CURRENT_OFFSET + MPCORE_GIC_CPU_INTERFACE_SIZE)
            .contains(&private_offset)
        {
            Some(private_offset - MPCORE_CPU_INTERFACE_CURRENT_OFFSET)
        } else if (MPCORE_CPU_INTERFACE_CPU0_OFFSET
            ..MPCORE_CPU_INTERFACE_CPU0_OFFSET + MPCORE_GIC_CPU_INTERFACE_SIZE)
            .contains(&private_offset)
        {
            Some(private_offset - MPCORE_CPU_INTERFACE_CPU0_OFFSET)
        } else {
            None
        }
    }

    fn mpcore_gic_distributor_register_offset(&self, address: u32) -> Option<u32> {
        let private_offset = self.mpcore_private_offset(address)?;
        if (MPCORE_GIC_DISTRIBUTOR_OFFSET
            ..MPCORE_GIC_DISTRIBUTOR_OFFSET + MPCORE_GIC_DISTRIBUTOR_SIZE)
            .contains(&private_offset)
        {
            Some(private_offset - MPCORE_GIC_DISTRIBUTOR_OFFSET)
        } else {
            None
        }
    }

    fn read_mpcore_cpu_interface_register(&mut self, address: u32) -> BusResult<u32> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_cpu_interface_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        if offset & 3 != 0 {
            return Err(Error::InvalidAccess { addr: address });
        }
        self.mpcore_interrupt_controller
            .read_cpu_interface_register(offset)
            .ok_or(Error::InvalidAccess { addr: address })
    }

    fn write_mpcore_cpu_interface_register(&mut self, address: u32, value: u32) -> BusResult<()> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_cpu_interface_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        if !self
            .mpcore_interrupt_controller
            .write_cpu_interface_register(offset, value)
        {
            return Err(Error::InvalidAccess { addr: address });
        }
        Ok(())
    }

    fn read_mpcore_gic_distributor_byte(&self, address: u32) -> BusResult<u8> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_gic_distributor_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        Ok(self
            .mpcore_interrupt_controller
            .read_distributor_byte(offset))
    }

    fn write_mpcore_gic_distributor_byte(&mut self, address: u32, value: u8) -> BusResult<()> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_gic_distributor_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        self.mpcore_interrupt_controller
            .write_distributor_byte(offset, value);
        Ok(())
    }

    fn read_mpcore_gic_distributor_word(&self, address: u32) -> BusResult<u32> {
        self.interrupt_state_touched.set(true);
        let mut bytes = [0u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read_mpcore_gic_distributor_byte(address.wrapping_add(index as u32))?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn write_mpcore_gic_distributor_word(&mut self, address: u32, value: u32) -> BusResult<()> {
        self.interrupt_state_touched.set(true);
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write_mpcore_gic_distributor_byte(address.wrapping_add(index as u32), byte)?;
        }
        Ok(())
    }

    fn read_mpcore_private_timer_register(&self, address: u32) -> BusResult<u32> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_private_timer_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        if offset & 3 != 0 {
            return Err(Error::InvalidAccess { addr: address });
        }
        self.mpcore_private_timer
            .read_register(offset)
            .ok_or(Error::InvalidAccess { addr: address })
    }

    fn write_mpcore_private_timer_register(&mut self, address: u32, value: u32) -> BusResult<()> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_private_timer_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        if offset & 3 != 0 || !self.mpcore_private_timer.write_register(offset, value) {
            return Err(Error::InvalidAccess { addr: address });
        }
        self.sync_mpcore_timer_interrupt();
        Ok(())
    }

    fn mpcore_scu_register_offset(&self, address: u32) -> Option<u32> {
        let offset = self.mpcore_private_offset(address)?;
        (offset < crate::cpu::SCU_REGISTER_WINDOW_SIZE).then_some(offset & !3)
    }

    fn read_mpcore_scu_register(&self, address: u32) -> BusResult<u32> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_scu_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        self.mpcore_scu
            .as_ref()
            .and_then(|scu| scu.read_register(offset))
            .ok_or(Error::InvalidAccess { addr: address })
    }

    fn write_mpcore_scu_register(&mut self, address: u32, value: u32) -> BusResult<()> {
        self.interrupt_state_touched.set(true);
        let offset = self
            .mpcore_scu_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        match self.mpcore_scu.as_ref() {
            Some(scu) if scu.write_register(offset, value) => Ok(()),
            _ => Err(Error::InvalidAccess { addr: address }),
        }
    }

    /// Maps a shared memory block into this process at a page-aligned address.
    ///
    /// Bit 0 grants read access and bit 1 grants write access. All mappings
    /// refer to the same backing allocation, so writes are visible across
    /// address spaces that map the same block.
    pub fn map_shared_memory_region(
        &mut self,
        address: u32,
        backing: SharedMemoryBacking,
        permissions: u32,
    ) -> std::result::Result<(), Arm11DynamicMemoryError> {
        if u64::from(address) % PAGE_SIZE != 0 {
            return Err(Arm11DynamicMemoryError::MisalignedAddress(address));
        }
        if permissions > 3 {
            return Err(Arm11DynamicMemoryError::InvalidPermissions(permissions));
        }
        let size = backing
            .len()
            .and_then(|length| u32::try_from(length).ok())
            .ok_or(Arm11DynamicMemoryError::InvalidSize(0))?;
        if size == 0 || u64::from(size) % PAGE_SIZE != 0 {
            return Err(Arm11DynamicMemoryError::InvalidSize(size));
        }
        let end = u64::from(address)
            .checked_add(u64::from(size))
            .filter(|end| *end <= ADDRESS_SPACE_END)
            .ok_or(Arm11DynamicMemoryError::AddressRangeOverflow { address, size })?;

        let overlaps_static = [
            self.layout.text,
            self.layout.read_only,
            self.layout.data,
            self.layout.bss,
            self.layout.stack,
        ]
        .iter()
        .any(|segment| {
            !segment.is_empty()
                && u64::from(address) < segment.end_address()
                && end > u64::from(segment.address)
        });
        let overlaps_dynamic = self.dynamic_regions.iter().any(|region| {
            let region_end = u64::from(region.address) + region.len() as u64;
            u64::from(address) < region_end && end > u64::from(region.address)
        });
        let overlaps_shared = self.shared_regions.iter().any(|region| {
            region.end_address().is_some_and(|region_end| {
                u64::from(address) < region_end && end > u64::from(region.address)
            })
        });
        let overlaps_linear = self.linear_regions.iter().any(|region| {
            u64::from(address) < region.virtual_end() && end > u64::from(region.virtual_base)
        });
        if overlaps_static || overlaps_dynamic || overlaps_shared || overlaps_linear {
            return Err(Arm11DynamicMemoryError::OverlappingMapping);
        }

        self.shared_regions
            .try_reserve(1)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;
        self.shared_regions.push(SharedMemoryRegion {
            address,
            backing,
            permissions,
        });
        self.shared_regions.sort_by_key(|region| region.address);
        Ok(())
    }

    /// Changes R/W permissions for one complete heap allocation or shared mapping.
    ///
    /// The current VM model stores permissions per mapping, so partial-page or
    /// subrange reprotection is rejected rather than silently changing a larger
    /// or smaller region than the guest requested.
    pub fn protect_memory_region(
        &mut self,
        address: u32,
        size: u32,
        permissions: u32,
    ) -> std::result::Result<(), Arm11DynamicMemoryError> {
        if !address.is_multiple_of(PAGE_SIZE as u32) {
            return Err(Arm11DynamicMemoryError::MisalignedAddress(address));
        }
        if size == 0 || !size.is_multiple_of(PAGE_SIZE as u32) {
            return Err(Arm11DynamicMemoryError::InvalidSize(size));
        }
        if permissions > 3 {
            return Err(Arm11DynamicMemoryError::InvalidPermissions(permissions));
        }
        let end = u64::from(address)
            .checked_add(u64::from(size))
            .filter(|end| *end <= ADDRESS_SPACE_END)
            .ok_or(Arm11DynamicMemoryError::AddressRangeOverflow { address, size })?;

        if address >= APPLICATION_HEAP_BASE && end <= APPLICATION_HEAP_END {
            if let Some(region) = self
                .dynamic_regions
                .iter_mut()
                .find(|region| region.address == address && region.len() == size as usize)
            {
                region.permissions = permissions;
                return Ok(());
            }
        }

        if let Some(region) = self
            .shared_regions
            .iter_mut()
            .find(|region| region.address == address && region.size() == Some(size))
        {
            region.permissions = permissions;
            return Ok(());
        }

        Err(Arm11DynamicMemoryError::MappingNotFound { address, size })
    }

    /// Maps one IPC temporary buffer into the 0x04000000 alias window.
    ///
    /// The backing must be page-aligned and large enough for the source page
    /// offset plus the logical buffer size. One unmapped guard page is reserved
    /// before and after every alias. The returned address preserves the source
    /// buffer's offset within its first page. Receiver permissions are always RW.
    pub fn map_temporary_ipc_buffer(
        &mut self,
        backing: SharedMemoryBacking,
        source_page_offset: u32,
        size_bytes: u32,
    ) -> std::result::Result<u32, Arm11DynamicMemoryError> {
        let backing_size = backing
            .len()
            .and_then(|length| u32::try_from(length).ok())
            .ok_or(Arm11DynamicMemoryError::InvalidSize(0))?;
        if backing_size == 0
            || u64::from(backing_size) % PAGE_SIZE != 0
            || size_bytes == 0
            || source_page_offset >= PAGE_SIZE as u32
            || u64::from(source_page_offset) + u64::from(size_bytes) > u64::from(backing_size)
        {
            return Err(Arm11DynamicMemoryError::InvalidSize(size_bytes));
        }

        let reservation_size = backing_size.checked_add((PAGE_SIZE as u32) * 2).ok_or(
            Arm11DynamicMemoryError::AddressRangeOverflow {
                address: IPC_TEMPORARY_BUFFER_BASE,
                size: backing_size,
            },
        )?;
        self.ipc_temporary_regions
            .try_reserve(1)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;

        let mut reservation_base = IPC_TEMPORARY_BUFFER_BASE;
        loop {
            let reservation_end = u64::from(reservation_base) + u64::from(reservation_size);
            if reservation_end > IPC_TEMPORARY_BUFFER_END {
                return Err(Arm11DynamicMemoryError::AllocationFailure(
                    "IPC temporary-buffer address window exhausted".to_string(),
                ));
            }

            if let Some(overlap) = self.ipc_temporary_regions.iter().find(|region| {
                u64::from(reservation_base) < region.reservation_end()
                    && reservation_end > u64::from(region.reservation_base)
            }) {
                let next = overlap.reservation_end();
                let aligned = next.checked_add(PAGE_SIZE - 1).ok_or(
                    Arm11DynamicMemoryError::AddressRangeOverflow {
                        address: reservation_base,
                        size: reservation_size,
                    },
                )? & !(PAGE_SIZE - 1);
                reservation_base = u32::try_from(aligned).map_err(|_| {
                    Arm11DynamicMemoryError::AllocationFailure(
                        "IPC temporary-buffer address window exhausted".to_string(),
                    )
                })?;
                continue;
            }

            let free_region = self.query_memory(reservation_base);
            let free_start = u64::from(free_region.base_address);
            let free_end = free_start + u64::from(free_region.size_bytes);
            if free_region.state == Arm11MemoryState::Free
                && u64::from(reservation_base) >= free_start
                && reservation_end <= free_end
            {
                let mapping_base = reservation_base.checked_add(PAGE_SIZE as u32).ok_or(
                    Arm11DynamicMemoryError::AddressRangeOverflow {
                        address: reservation_base,
                        size: reservation_size,
                    },
                )?;
                self.map_shared_memory_region(mapping_base, backing, 3)?;
                self.ipc_temporary_regions.push(IpcTemporaryMappingRegion {
                    alias_address: mapping_base + source_page_offset,
                    mapping_base,
                    mapping_size: backing_size,
                    reservation_base,
                    reservation_size,
                });
                return Ok(mapping_base + source_page_offset);
            }

            let next = free_end.max(u64::from(reservation_base) + PAGE_SIZE);
            let aligned = next.checked_add(PAGE_SIZE - 1).ok_or(
                Arm11DynamicMemoryError::AddressRangeOverflow {
                    address: reservation_base,
                    size: reservation_size,
                },
            )? & !(PAGE_SIZE - 1);
            reservation_base = u32::try_from(aligned).map_err(|_| {
                Arm11DynamicMemoryError::AllocationFailure(
                    "IPC temporary-buffer address window exhausted".to_string(),
                )
            })?;
        }
    }

    /// Removes an IPC temporary mapping by its exact returned alias pointer.
    pub fn unmap_temporary_ipc_buffer(
        &mut self,
        alias_address: u32,
    ) -> std::result::Result<SharedMemoryBacking, Arm11DynamicMemoryError> {
        let Some(index) = self
            .ipc_temporary_regions
            .iter()
            .position(|region| region.alias_address == alias_address)
        else {
            return Err(Arm11DynamicMemoryError::MappingNotFound {
                address: alias_address,
                size: 0,
            });
        };
        let region = self.ipc_temporary_regions[index];
        let backing = self.unmap_shared_memory_region(region.mapping_base, region.mapping_size)?;
        self.ipc_temporary_regions.remove(index);
        Ok(backing)
    }

    /// Creates a zero-copy block view over a page-aligned regular-heap range.
    ///
    /// This bootstrap only accepts a range contained within one existing
    /// ControlMemory heap allocation. Static, TLS, shared and linear mappings
    /// are intentionally excluded until their ownership semantics are modeled.
    pub fn shared_memory_backing_for_heap_range(
        &mut self,
        address: u32,
        size: u32,
    ) -> std::result::Result<SharedMemoryBacking, Arm11DynamicMemoryError> {
        if !address.is_multiple_of(PAGE_SIZE as u32) {
            return Err(Arm11DynamicMemoryError::MisalignedAddress(address));
        }
        if size == 0 || u64::from(size) % PAGE_SIZE != 0 {
            return Err(Arm11DynamicMemoryError::InvalidSize(size));
        }
        let end = u64::from(address)
            .checked_add(u64::from(size))
            .ok_or(Arm11DynamicMemoryError::OutsideApplicationHeap { address, size })?;
        if address < APPLICATION_HEAP_BASE || end > APPLICATION_HEAP_END {
            return Err(Arm11DynamicMemoryError::OutsideApplicationHeap { address, size });
        }

        let Some(region) = self
            .dynamic_regions
            .iter_mut()
            .find(|region| address >= region.address && end <= region.end_address())
        else {
            return Err(Arm11DynamicMemoryError::MappingNotFound { address, size });
        };
        let offset = usize::try_from(address - region.address)
            .map_err(|_| Arm11DynamicMemoryError::InvalidSize(size))?;
        region
            .backing
            .shared_view(offset, size as usize)
            .ok_or(Arm11DynamicMemoryError::MappingNotFound { address, size })
    }

    /// Unmaps one exact shared mapping without destroying the shared backing.
    pub fn unmap_shared_memory_region(
        &mut self,
        address: u32,
        size: u32,
    ) -> std::result::Result<SharedMemoryBacking, Arm11DynamicMemoryError> {
        let Some(index) = self
            .shared_regions
            .iter()
            .position(|region| region.address == address && region.size() == Some(size))
        else {
            return Err(Arm11DynamicMemoryError::MappingNotFound { address, size });
        };
        self.flush_data_cache_range_for_kernel(address, size)
            .map_err(|error| Arm11DynamicMemoryError::WritebackFailure(error.to_string()))?;
        self.drain_arm11_write_buffer()
            .map_err(|error| Arm11DynamicMemoryError::WritebackFailure(error.to_string()))?;
        self.invalidate_exclusive_range(address, size as usize);
        Ok(self.shared_regions.remove(index).backing)
    }

    /// Returns whether a mapping at `address` uses the supplied shared block.
    pub fn shared_mapping_matches(&self, address: u32, backing: &SharedMemoryBacking) -> bool {
        self.shared_regions
            .iter()
            .find(|region| region.address == address)
            .is_some_and(|region| region.backing.is_same_view_as(backing))
    }

    /// Maps a reserved linear-memory interval into the process address space.
    ///
    /// The virtual and physical bases must follow one of the documented 3DS
    /// linear aliases. The physical range must already be reserved by the
    /// system's FCRAM allocator, and all CPU loads/stores use that same backing.
    pub fn map_linear_memory_region(
        &mut self,
        virtual_base: u32,
        physical_base: u32,
        size: u32,
    ) -> std::result::Result<(), Arm11DynamicMemoryError> {
        if size == 0
            || !virtual_base.is_multiple_of(PAGE_SIZE as u32)
            || !physical_base.is_multiple_of(PAGE_SIZE as u32)
            || !size.is_multiple_of(PAGE_SIZE as u32)
        {
            return Err(Arm11DynamicMemoryError::InvalidSize(size));
        }

        let virtual_end = u64::from(virtual_base) + u64::from(size);
        let expected_physical = if u64::from(virtual_base) >= u64::from(OLD_LINEAR_BASE)
            && virtual_end <= OLD_LINEAR_END
        {
            u64::from(virtual_base) + 0x0C00_0000
        } else if u64::from(virtual_base) >= u64::from(NEW_LINEAR_BASE)
            && virtual_end <= NEW_LINEAR_END
        {
            u64::from(FCRAM_BASE) + (u64::from(virtual_base) - u64::from(NEW_LINEAR_BASE))
        } else {
            return Err(Arm11DynamicMemoryError::OutsideLinearMemoryRange {
                address: virtual_base,
                size,
            });
        };
        let physical_end = u64::from(physical_base) + u64::from(size);
        if expected_physical != u64::from(physical_base)
            || physical_end > u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE)
        {
            return Err(Arm11DynamicMemoryError::OutsideLinearMemoryRange {
                address: virtual_base,
                size,
            });
        }

        let memory = self
            .gpu_memory
            .as_ref()
            .ok_or(Arm11DynamicMemoryError::PhysicalMemoryUnavailable)?;
        memory
            .validate_range(physical_base, size as usize)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;
        if !memory.has_linear_fcram_allocation(physical_base, size) {
            return Err(Arm11DynamicMemoryError::LinearMappingNotFound {
                address: virtual_base,
                size,
            });
        }

        let overlaps_static = [
            self.layout.text,
            self.layout.read_only,
            self.layout.data,
            self.layout.bss,
            self.layout.stack,
        ]
        .iter()
        .any(|segment| {
            !segment.is_empty()
                && u64::from(virtual_base) < segment.end_address()
                && virtual_end > u64::from(segment.address)
        });
        let overlaps_dynamic = self.dynamic_regions.iter().any(|region| {
            let region_end = u64::from(region.address) + region.len() as u64;
            u64::from(virtual_base) < region_end && virtual_end > u64::from(region.address)
        });
        let overlaps_shared = self.shared_regions.iter().any(|region| {
            region.end_address().is_some_and(|region_end| {
                u64::from(virtual_base) < region_end && virtual_end > u64::from(region.address)
            })
        });
        let overlaps_linear = self.linear_regions.iter().any(|region| {
            u64::from(virtual_base) < region.virtual_end()
                && virtual_end > u64::from(region.virtual_base)
        });
        if overlaps_static || overlaps_dynamic || overlaps_shared || overlaps_linear {
            return Err(Arm11DynamicMemoryError::OverlappingMapping);
        }

        self.linear_regions
            .try_reserve(1)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;
        self.linear_regions.push(LinearMemoryRegion {
            virtual_base,
            physical_base,
            size,
        });
        self.linear_regions
            .sort_by_key(|region| region.virtual_base);
        Ok(())
    }

    /// Unmaps one exact linear interval without releasing its physical FCRAM.
    pub fn unmap_linear_memory_region(
        &mut self,
        virtual_base: u32,
        physical_base: u32,
        size: u32,
    ) -> std::result::Result<(), Arm11DynamicMemoryError> {
        let Some(index) = self.linear_regions.iter().position(|region| {
            region.virtual_base == virtual_base
                && region.physical_base == physical_base
                && region.size == size
        }) else {
            return Err(Arm11DynamicMemoryError::LinearMappingNotFound {
                address: virtual_base,
                size,
            });
        };
        self.flush_data_cache_range(virtual_base, size)
            .map_err(|error| Arm11DynamicMemoryError::WritebackFailure(error.to_string()))?;
        self.drain_arm11_write_buffer()
            .map_err(|error| Arm11DynamicMemoryError::WritebackFailure(error.to_string()))?;
        self.invalidate_exclusive_range(virtual_base, size as usize);
        self.linear_regions.remove(index);
        Ok(())
    }

    /// Maps one zero-initialized RW/NX region in the 3DS application heap window.
    ///
    /// The supported virtual heap window is 0x08000000..0x0E000000, matching
    /// libctru's default application-heap range. Backing bytes are allocated
    /// lazily by the host and are not a model of physical FCRAM allocation.
    /// Finds the first page-aligned gap large enough for a regular application-heap allocation.
    ///
    /// The search accounts for static executable segments and every dynamic,
    /// shared and linear mapping already visible to this process.
    fn first_fit_application_heap_address(
        &self,
        size: u32,
    ) -> std::result::Result<u32, Arm11DynamicMemoryError> {
        if size == 0 || u64::from(size) % PAGE_SIZE != 0 {
            return Err(Arm11DynamicMemoryError::InvalidSize(size));
        }

        let mut candidate = u64::from(APPLICATION_HEAP_BASE);
        loop {
            let end = candidate + u64::from(size);
            if end > APPLICATION_HEAP_END {
                return Err(Arm11DynamicMemoryError::OutsideApplicationHeap {
                    address: candidate.min(APPLICATION_HEAP_END) as u32,
                    size,
                });
            }

            let mut next_candidate = candidate;
            {
                let mut consider_overlap = |region_start: u64, region_end: u64| {
                    if candidate < region_end && end > region_start {
                        let aligned_end =
                            region_end.saturating_add(PAGE_SIZE - 1) / PAGE_SIZE * PAGE_SIZE;
                        next_candidate = next_candidate.max(aligned_end);
                    }
                };

                for segment in [
                    self.layout.text,
                    self.layout.read_only,
                    self.layout.data,
                    self.layout.bss,
                    self.layout.stack,
                ] {
                    if !segment.is_empty() {
                        consider_overlap(u64::from(segment.address), segment.end_address());
                    }
                }
                for region in &self.dynamic_regions {
                    consider_overlap(u64::from(region.address), region.end_address());
                }
                for region in &self.shared_regions {
                    if let Some(region_end) = region.end_address() {
                        consider_overlap(u64::from(region.address), region_end);
                    }
                }
                for region in &self.linear_regions {
                    consider_overlap(u64::from(region.virtual_base), region.virtual_end());
                }
            }

            if next_candidate == candidate {
                return Ok(candidate as u32);
            }
            candidate = next_candidate;
        }
    }

    pub fn allocate_application_heap_region(
        &mut self,
        address: u32,
        size: u32,
    ) -> std::result::Result<u32, Arm11DynamicMemoryError> {
        if size == 0 || u64::from(size) % PAGE_SIZE != 0 {
            return Err(Arm11DynamicMemoryError::InvalidSize(size));
        }
        // An addr0 of zero is a kernel placement hint: choose the first free
        // page-aligned range instead of treating it as an invalid heap address.
        let address = if address == 0 {
            self.first_fit_application_heap_address(size)?
        } else {
            address
        };
        if u64::from(address) % PAGE_SIZE != 0 {
            return Err(Arm11DynamicMemoryError::MisalignedAddress(address));
        }

        let end = u64::from(address)
            .checked_add(u64::from(size))
            .ok_or(Arm11DynamicMemoryError::OutsideApplicationHeap { address, size })?;
        if address < APPLICATION_HEAP_BASE || end > APPLICATION_HEAP_END {
            return Err(Arm11DynamicMemoryError::OutsideApplicationHeap { address, size });
        }

        let static_segments = [
            self.layout.text,
            self.layout.read_only,
            self.layout.data,
            self.layout.bss,
            self.layout.stack,
        ];
        if static_segments.iter().any(|segment| {
            !segment.is_empty()
                && u64::from(address) < segment.end_address()
                && end > u64::from(segment.address)
        }) {
            return Err(Arm11DynamicMemoryError::OverlappingMapping);
        }

        if self.dynamic_regions.iter().any(|region| {
            let region_end = u64::from(region.address) + region.len() as u64;
            u64::from(address) < region_end && end > u64::from(region.address)
        }) || self.shared_regions.iter().any(|region| {
            region.end_address().is_some_and(|region_end| {
                u64::from(address) < region_end && end > u64::from(region.address)
            })
        }) || self.linear_regions.iter().any(|region| {
            u64::from(address) < region.virtual_end() && end > u64::from(region.virtual_base)
        }) {
            return Err(Arm11DynamicMemoryError::OverlappingMapping);
        }

        let backing_size =
            usize::try_from(size).map_err(|_| Arm11DynamicMemoryError::InvalidSize(size))?;
        let backing = SharedMemoryBacking::zeroed(backing_size)
            .map_err(Arm11DynamicMemoryError::AllocationFailure)?;
        self.dynamic_regions
            .try_reserve(1)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;
        self.dynamic_regions.push(DynamicMemoryRegion {
            address,
            backing: DynamicMemoryBacking::Private(backing),
            permissions: 3,
        });
        self.dynamic_regions.sort_by_key(|region| region.address);

        Ok(address)
    }

    /// Reserves one zeroed 0x200-byte thread-local-storage slot and returns its
    /// address (the value a thread's `TPIDRURO` points at).
    ///
    /// Slots live in the 0x1FF82000 window, eight per page, and are reused after
    /// [`release_thread_local_storage`](Self::release_thread_local_storage).
    pub fn allocate_thread_local_storage(
        &mut self,
    ) -> std::result::Result<u32, Arm11DynamicMemoryError> {
        let slot = match self.tls_slots_in_use.iter().position(|used| !used) {
            Some(slot) => slot,
            None => {
                let slot = self.tls_slots_in_use.len();
                let page = slot as u32 / THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE;
                if page >= THREAD_LOCAL_STORAGE_MAX_PAGES {
                    return Err(Arm11DynamicMemoryError::AllocationFailure(
                        "thread-local-storage window exhausted".to_string(),
                    ));
                }
                if (slot as u32).is_multiple_of(THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE) {
                    let address = THREAD_LOCAL_STORAGE_BASE + page * PAGE_SIZE as u32;
                    self.dynamic_regions.try_reserve(1).map_err(|error| {
                        Arm11DynamicMemoryError::AllocationFailure(error.to_string())
                    })?;
                    self.dynamic_regions.push(DynamicMemoryRegion {
                        address,
                        backing: DynamicMemoryBacking::Private(
                            SharedMemoryBacking::zeroed(PAGE_SIZE as usize).map_err(|error| {
                                Arm11DynamicMemoryError::AllocationFailure(error)
                            })?,
                        ),
                        permissions: 3,
                    });
                    self.dynamic_regions.sort_by_key(|region| region.address);
                }
                self.tls_slots_in_use.push(false);
                slot
            }
        };
        self.tls_slots_in_use[slot] = true;
        let address = THREAD_LOCAL_STORAGE_BASE
            + (slot as u32 / THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE) * PAGE_SIZE as u32
            + (slot as u32 % THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE) * THREAD_LOCAL_STORAGE_SLOT_SIZE;
        // A reused slot must not leak the previous thread's data.
        for offset in 0..THREAD_LOCAL_STORAGE_SLOT_SIZE {
            self.write_bytes(address + offset, &[0])
                .expect("thread-local-storage pages are mapped when a slot is allocated");
        }
        Ok(address)
    }

    /// Returns a slot obtained from `allocate_thread_local_storage` to the pool.
    /// Unknown or already-free addresses are ignored.
    pub fn release_thread_local_storage(&mut self, address: u32) {
        let Some(offset) = address.checked_sub(THREAD_LOCAL_STORAGE_BASE) else {
            return;
        };
        let (page, within) = (offset / PAGE_SIZE as u32, offset % PAGE_SIZE as u32);
        if within % THREAD_LOCAL_STORAGE_SLOT_SIZE != 0 {
            return;
        }
        let slot = (page * THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE
            + within / THREAD_LOCAL_STORAGE_SLOT_SIZE) as usize;
        if let Some(used) = self.tls_slots_in_use.get_mut(slot) {
            *used = false;
        }
    }

    /// Returns the address of a static-buffer descriptor entry in a live TLS slot.
    ///
    /// The 16 IPC static-buffer entries occupy TLS offsets 0x180..0x200. A
    /// descriptor consists of the receiver buffer address followed by its capacity.
    /// Stale, unallocated, misaligned or out-of-range TLS addresses are rejected.
    pub fn thread_local_static_buffer_descriptor_address(
        &self,
        tls_address: u32,
        buffer_index: u8,
    ) -> Option<u32> {
        if buffer_index >= THREAD_LOCAL_STATIC_BUFFER_COUNT {
            return None;
        }
        let offset = tls_address.checked_sub(THREAD_LOCAL_STORAGE_BASE)?;
        if offset % THREAD_LOCAL_STORAGE_SLOT_SIZE != 0 {
            return None;
        }
        let slot = usize::try_from(offset / THREAD_LOCAL_STORAGE_SLOT_SIZE).ok()?;
        if !self.tls_slots_in_use.get(slot).copied().unwrap_or(false) {
            return None;
        }
        let entry_offset = THREAD_LOCAL_STATIC_BUFFER_TABLE_OFFSET
            .checked_add(u32::from(buffer_index) * THREAD_LOCAL_STATIC_BUFFER_DESCRIPTOR_SIZE)?;
        tls_address.checked_add(entry_offset)
    }

    /// Releases one dynamic application-heap mapping by exact base and size.
    ///
    /// Static executable, data, BSS and main-stack mappings cannot be released
    /// through this function.
    pub fn free_application_heap_region(
        &mut self,
        address: u32,
        size: u32,
    ) -> std::result::Result<(), Arm11DynamicMemoryError> {
        let in_heap_window = address >= APPLICATION_HEAP_BASE
            && u64::from(address) + u64::from(size) <= APPLICATION_HEAP_END;
        let Some(index) = self
            .dynamic_regions
            .iter()
            .position(|region| region.address == address && region.len() == size as usize)
            .filter(|_| in_heap_window)
        else {
            return Err(Arm11DynamicMemoryError::MappingNotFound { address, size });
        };
        let backing = self.dynamic_regions[index].backing.backing_ref();
        if self
            .shared_regions
            .iter()
            .any(|mapping| mapping.backing.shares_backing_with(backing))
        {
            return Err(Arm11DynamicMemoryError::OverlappingMapping);
        }
        self.flush_data_cache_range_for_kernel(address, size)
            .map_err(|error| Arm11DynamicMemoryError::WritebackFailure(error.to_string()))?;
        self.drain_arm11_write_buffer()
            .map_err(|error| Arm11DynamicMemoryError::WritebackFailure(error.to_string()))?;
        self.invalidate_exclusive_range(address, size as usize);
        self.dynamic_regions.remove(index);
        Ok(())
    }

    /// Returns true when an address belongs to a mapped executable segment.
    ///
    /// Bit zero is ignored so a Thumb function pointer can be validated using
    /// the same mapping check as its aligned instruction address.
    pub fn is_executable_address(&self, address: u32) -> bool {
        let instruction_address = address & !1;
        let Some(location) = self.locate(instruction_address) else {
            return false;
        };
        self.location_is_executable(location)
    }

    /// Returns true when every byte in a range is mapped and writable.
    ///
    /// The range may span adjacent writable static segments or dynamic heap
    /// mappings. Empty, wrapping, or partially unmapped ranges are rejected.
    pub fn is_writable_range(&self, address: u32, size: usize) -> bool {
        if size == 0 {
            return false;
        }

        let Some(end) = u64::from(address).checked_add(size as u64) else {
            return false;
        };
        if end > ADDRESS_SPACE_END {
            return false;
        }

        let mut cursor = u64::from(address);
        while cursor < end {
            let Ok(current_address) = u32::try_from(cursor) else {
                return false;
            };
            let Some(location) = self.locate(current_address) else {
                return false;
            };
            if !self.location_is_writable(location) {
                return false;
            }

            let Some(region_end) = self.location_end(location) else {
                return false;
            };
            let next = region_end.min(end);
            if next <= cursor {
                return false;
            }
            cursor = next;
        }

        true
    }

    /// Returns whether every byte in a range is mapped and readable.
    ///
    /// Empty, wrapping, or partially unmapped ranges are rejected. Shared
    /// mappings require their read-permission bit; ordinary process and linear
    /// memory are readable in this bootstrap model.
    pub fn is_readable_range(&self, address: u32, size: usize) -> bool {
        if size == 0 {
            return false;
        }

        let Some(end) = u64::from(address).checked_add(size as u64) else {
            return false;
        };
        if end > ADDRESS_SPACE_END {
            return false;
        }

        let mut cursor = u64::from(address);
        while cursor < end {
            let Ok(current_address) = u32::try_from(cursor) else {
                return false;
            };
            let Some(location) = self.locate(current_address) else {
                return false;
            };
            if !self.location_is_readable(location) {
                return false;
            }

            let Some(region_end) = self.location_end(location) else {
                return false;
            };
            let next = region_end.min(end);
            if next <= cursor {
                return false;
            }
            cursor = next;
        }

        true
    }

    /// Returns the physically contiguous FCRAM chunks for a wholly linear range.
    ///
    /// Every byte must be backed by a registered linear-memory mapping. Adjacent
    /// virtual mappings whose physical addresses are also adjacent are coalesced.
    /// Private heap, static image, TLS and shared-block views intentionally have
    /// no fabricated physical address in this model.
    pub fn linear_physical_chunks_for_range(
        &self,
        address: u32,
        size: u32,
    ) -> Option<Vec<(u32, u32)>> {
        if size == 0 {
            return None;
        }
        let end = u64::from(address).checked_add(u64::from(size))?;
        if end > ADDRESS_SPACE_END {
            return None;
        }

        let mut chunks: Vec<(u32, u32)> = Vec::new();
        let mut cursor = u64::from(address);
        while cursor < end {
            let current = u32::try_from(cursor).ok()?;
            let Some(MemoryLocation::Physical {
                address: physical_address,
                ..
            }) = self.locate(current)
            else {
                return None;
            };
            let location = self.locate(current)?;
            let region_end = self.location_end(location)?;
            let chunk_end = region_end.min(end);
            let chunk_size = u32::try_from(chunk_end - cursor).ok()?;
            if chunk_size == 0 {
                return None;
            }

            let extends_previous =
                chunks
                    .last()
                    .is_some_and(|(previous_address, previous_size)| {
                        previous_address.checked_add(*previous_size) == Some(physical_address)
                    });
            if extends_previous {
                let previous = chunks.last_mut()?;
                previous.1 = previous.1.checked_add(chunk_size)?;
            } else {
                chunks.try_reserve(1).ok()?;
                chunks.push((physical_address, chunk_size));
            }
            cursor = chunk_end;
        }

        Some(chunks)
    }

    /// Queries the mapped or free virtual region containing an address.
    ///
    /// Adjacent mappings with identical permissions and state are coalesced.
    /// Unmapped addresses return the complete free gap between neighbouring
    /// mappings. Page lock/dirty tracking is not implemented, so page_flags
    /// is currently zero for every region.
    pub fn query_memory(&self, address: u32) -> Arm11MemoryRegionInfo {
        let mut regions = Vec::with_capacity(5 + self.dynamic_regions.len());
        let segments = [
            (self.layout.text, Arm11MemoryState::Code),
            (self.layout.read_only, Arm11MemoryState::Code),
            (self.layout.data, Arm11MemoryState::Private),
            (self.layout.bss, Arm11MemoryState::Private),
            (self.layout.stack, Arm11MemoryState::Private),
        ];

        for (segment, state) in segments {
            if segment.is_empty() {
                continue;
            }
            let mut permissions = 1;
            if segment.writable {
                permissions |= 2;
            }
            if segment.executable {
                permissions |= 4;
            }
            regions.push(Arm11MemoryRegionInfo {
                base_address: segment.address,
                size_bytes: segment.mapped_size_bytes,
                permissions,
                state,
                page_flags: 0,
            });
        }

        for region in &self.linear_regions {
            regions.push(Arm11MemoryRegionInfo {
                base_address: region.virtual_base,
                size_bytes: region.size,
                permissions: 3,
                state: Arm11MemoryState::Private,
                page_flags: 0,
            });
        }

        for region in &self.shared_regions {
            regions.push(Arm11MemoryRegionInfo {
                base_address: region.address,
                size_bytes: region
                    .size()
                    .expect("shared memory block sizes are immutable"),
                permissions: region.permissions,
                state: Arm11MemoryState::Shared,
                page_flags: 0,
            });
        }

        for region in &self.dynamic_regions {
            regions.push(Arm11MemoryRegionInfo {
                base_address: region.address,
                size_bytes: u32::try_from(region.len())
                    .expect("dynamic mappings are limited to u32-sized regions"),
                permissions: 3,
                state: Arm11MemoryState::Private,
                page_flags: 0,
            });
        }

        regions.sort_by_key(|region| region.base_address);
        let mut merged: Vec<Arm11MemoryRegionInfo> = Vec::with_capacity(regions.len());
        for region in regions {
            if let Some(previous) = merged.last_mut() {
                let previous_end =
                    u64::from(previous.base_address) + u64::from(previous.size_bytes);
                if previous_end == u64::from(region.base_address)
                    && previous.permissions == region.permissions
                    && previous.state == region.state
                {
                    previous.size_bytes = previous.size_bytes.saturating_add(region.size_bytes);
                    continue;
                }
            }
            merged.push(region);
        }

        if let Some(region) = merged.iter().find(|region| {
            u64::from(address) >= u64::from(region.base_address)
                && u64::from(address)
                    < u64::from(region.base_address) + u64::from(region.size_bytes)
        }) {
            return *region;
        }

        let address64 = u64::from(address);
        let previous_end = merged
            .iter()
            .map(|region| u64::from(region.base_address) + u64::from(region.size_bytes))
            .filter(|end| *end <= address64)
            .max()
            .unwrap_or(0);
        let next_base = merged
            .iter()
            .map(|region| u64::from(region.base_address))
            .filter(|base| *base > address64)
            .min()
            .unwrap_or(ADDRESS_SPACE_END);
        let free_base = previous_end.min(address64);
        let free_size = next_base.saturating_sub(free_base);
        Arm11MemoryRegionInfo {
            base_address: u32::try_from(free_base).unwrap_or(address),
            size_bytes: u32::try_from(free_size).unwrap_or(u32::MAX),
            permissions: 0,
            state: Arm11MemoryState::Free,
            page_flags: 0,
        }
    }

    /// Returns the backing bytes for one mapped code segment.
    pub fn segment_bytes(&self, kind: Arm11SegmentKind) -> &[u8] {
        self.bytes_for(kind)
    }

    /// Finds the last linear mapping whose virtual base does not exceed the address.
    ///
    /// Linear mappings are kept sorted and non-overlapping in virtual address
    /// order by map/unmap operations.
    fn linear_region_at_virtual_address(
        &self,
        address: u32,
    ) -> Option<(usize, &LinearMemoryRegion)> {
        let index = self
            .linear_regions
            .partition_point(|region| region.virtual_base <= address)
            .checked_sub(1)?;
        let region = self.linear_regions.get(index)?;
        (u64::from(address) < region.virtual_end()).then_some((index, region))
    }

    /// Resolves an FCRAM physical address through either documented linear alias.
    ///
    /// Every valid linear mapping is a subrange of one of two affine windows.
    /// Deriving those two possible virtual addresses avoids scanning all process
    /// mappings on every MMU-translated memory access. The old alias sorts before
    /// the new alias, matching the former first-match behavior when both exist.
    fn linear_region_at_physical_address(&self, address: u32) -> Option<MemoryLocation> {
        let fcram_end = u64::from(FCRAM_BASE) + u64::from(FCRAM_SIZE);
        if address < FCRAM_BASE || u64::from(address) >= fcram_end {
            return None;
        }

        if let Some(old_virtual) = address.checked_sub(0x0C00_0000)
            && old_virtual >= OLD_LINEAR_BASE
            && u64::from(old_virtual) < OLD_LINEAR_END
            && let Some((index, region)) = self.linear_region_at_virtual_address(old_virtual)
        {
            let offset = old_virtual - region.virtual_base;
            if u64::from(region.physical_base) + u64::from(offset) == u64::from(address) {
                return Some(MemoryLocation::Physical { index, address });
            }
        }

        let new_virtual = NEW_LINEAR_BASE.checked_add(address - FCRAM_BASE)?;
        if u64::from(new_virtual) < NEW_LINEAR_END
            && let Some((index, region)) = self.linear_region_at_virtual_address(new_virtual)
        {
            let offset = new_virtual - region.virtual_base;
            if u64::from(region.physical_base) + u64::from(offset) == u64::from(address) {
                return Some(MemoryLocation::Physical { index, address });
            }
        }

        None
    }

    #[inline]
    fn static_segment_hit(&self, index: u8, address: u32) -> Option<MemoryLocation> {
        let (kind, segment) = match index {
            0 => (Arm11SegmentKind::Text, &self.layout.text),
            1 => (Arm11SegmentKind::ReadOnly, &self.layout.read_only),
            2 => (Arm11SegmentKind::Data, &self.layout.data),
            3 => (Arm11SegmentKind::Bss, &self.layout.bss),
            _ => (Arm11SegmentKind::Stack, &self.layout.stack),
        };
        let offset = address.checked_sub(segment.address)?;
        (offset < segment.mapped_size_bytes).then_some(MemoryLocation::Static {
            kind,
            offset: offset as usize,
        })
    }

    fn locate(&self, address: u32) -> Option<MemoryLocation> {
        // Static segments are disjoint, so trying the last hit first returns
        // the same location the in-order scan would.
        let hint = self.static_segment_hint.get();
        if let Some(location) = self.static_segment_hit(hint, address) {
            return Some(location);
        }
        for index in (0..5u8).filter(|index| *index != hint) {
            if let Some(location) = self.static_segment_hit(index, address) {
                self.static_segment_hint.set(index);
                return Some(location);
            }
        }

        // Translated ARM11 accesses arrive here with a physical FCRAM
        // address. That range is disjoint from both virtual linear-alias
        // windows, so resolve it first and avoid a guaranteed-miss virtual
        // mapping search on every physical-memory access.
        if let Some(location) = self.linear_region_at_physical_address(address) {
            return Some(location);
        }

        if let Some((index, region)) = self.linear_region_at_virtual_address(address) {
            let offset = address - region.virtual_base;
            return Some(MemoryLocation::Physical {
                index,
                address: region.physical_base + offset,
            });
        }

        let shared_index = self
            .shared_regions
            .partition_point(|region| region.address <= address)
            .checked_sub(1);
        if let Some(index) = shared_index {
            let region = &self.shared_regions[index];
            if region
                .end_address()
                .is_some_and(|end| u64::from(address) < end)
            {
                let offset = usize::try_from(address - region.address).ok()?;
                return Some(MemoryLocation::Shared { index, offset });
            }
        }

        let dynamic_index = self
            .dynamic_regions
            .partition_point(|region| region.address <= address)
            .checked_sub(1)?;
        let region = self.dynamic_regions.get(dynamic_index)?;
        let offset = u64::from(address) - u64::from(region.address);
        if offset < region.len() as u64 {
            return Some(MemoryLocation::Dynamic {
                index: dynamic_index,
                offset: usize::try_from(offset).ok()?,
            });
        }

        None
    }

    fn bytes_for(&self, kind: Arm11SegmentKind) -> &[u8] {
        match kind {
            Arm11SegmentKind::Text => &self.text,
            Arm11SegmentKind::ReadOnly => &self.read_only,
            Arm11SegmentKind::Data => &self.data,
            Arm11SegmentKind::Bss => &self.bss,
            Arm11SegmentKind::Stack => &self.stack,
        }
    }

    fn bytes_for_mut(&mut self, kind: Arm11SegmentKind) -> &mut [u8] {
        match kind {
            Arm11SegmentKind::Text => &mut self.text,
            Arm11SegmentKind::ReadOnly => &mut self.read_only,
            Arm11SegmentKind::Data => &mut self.data,
            Arm11SegmentKind::Bss => &mut self.bss,
            Arm11SegmentKind::Stack => &mut self.stack,
        }
    }

    fn location_is_readable(&self, location: MemoryLocation) -> bool {
        match location {
            MemoryLocation::Static { .. } | MemoryLocation::Physical { .. } => true,
            MemoryLocation::Dynamic { index, .. } => self
                .dynamic_regions
                .get(index)
                .is_some_and(|region| region.permissions & 1 != 0),
            MemoryLocation::Shared { index, .. } => self
                .shared_regions
                .get(index)
                .is_some_and(|region| region.permissions & 1 != 0),
        }
    }

    fn location_is_writable(&self, location: MemoryLocation) -> bool {
        match location {
            MemoryLocation::Static { kind, .. } => self.segment_is_writable(kind),
            MemoryLocation::Physical { .. } => true,
            MemoryLocation::Dynamic { index, .. } => self
                .dynamic_regions
                .get(index)
                .is_some_and(|region| region.permissions & 2 != 0),
            MemoryLocation::Shared { index, .. } => self
                .shared_regions
                .get(index)
                .is_some_and(|region| region.permissions & 2 != 0),
        }
    }

    fn location_is_executable(&self, location: MemoryLocation) -> bool {
        match location {
            MemoryLocation::Static { kind, .. } => self.segment_is_executable(kind),
            MemoryLocation::Dynamic { .. }
            | MemoryLocation::Shared { .. }
            | MemoryLocation::Physical { .. } => false,
        }
    }

    fn location_end(&self, location: MemoryLocation) -> Option<u64> {
        match location {
            MemoryLocation::Static { kind, .. } => {
                let segment = match kind {
                    Arm11SegmentKind::Text => self.layout.text,
                    Arm11SegmentKind::ReadOnly => self.layout.read_only,
                    Arm11SegmentKind::Data => self.layout.data,
                    Arm11SegmentKind::Bss => self.layout.bss,
                    Arm11SegmentKind::Stack => self.layout.stack,
                };
                Some(segment.end_address())
            }
            MemoryLocation::Dynamic { index, .. } => {
                let region = self.dynamic_regions.get(index)?;
                Some(u64::from(region.address) + region.len() as u64)
            }
            MemoryLocation::Shared { index, .. } => self.shared_regions.get(index)?.end_address(),
            MemoryLocation::Physical { index, .. } => {
                Some(self.linear_regions.get(index)?.virtual_end())
            }
        }
    }

    fn segment_is_writable(&self, kind: Arm11SegmentKind) -> bool {
        match kind {
            Arm11SegmentKind::Text => self.layout.text.writable,
            Arm11SegmentKind::ReadOnly => self.layout.read_only.writable,
            Arm11SegmentKind::Data => self.layout.data.writable,
            Arm11SegmentKind::Bss => self.layout.bss.writable,
            Arm11SegmentKind::Stack => self.layout.stack.writable,
        }
    }

    fn segment_is_executable(&self, kind: Arm11SegmentKind) -> bool {
        match kind {
            Arm11SegmentKind::Text => self.layout.text.executable,
            Arm11SegmentKind::ReadOnly => self.layout.read_only.executable,
            Arm11SegmentKind::Data => self.layout.data.executable,
            Arm11SegmentKind::Bss => self.layout.bss.executable,
            Arm11SegmentKind::Stack => self.layout.stack.executable,
        }
    }

    /// Whether loads and stores bypass the D-cache and write-buffer model.
    pub fn fast_memory(&self) -> bool {
        self.fast_memory
    }

    /// Switches between the precise cache model and fast direct memory.
    /// Enabling writes back every dirty line and drains the write buffer
    /// first, so no guest store is lost.
    pub fn set_fast_memory(&mut self, enabled: bool) -> BusResult<()> {
        if enabled && !self.fast_memory {
            let mut keys: Vec<DataCacheKey> = self.data_cache.keys().copied().collect();
            keys.sort_unstable();
            for key in keys {
                if let Some(line) = self.remove_data_cache_line(key) {
                    if line.dirty {
                        self.write_cache_target_range(&line.target, 0, &line.bytes)?;
                    }
                }
            }
            self.drain_arm11_write_buffer()?;
        }
        self.fast_memory = enabled;
        Ok(())
    }

    /// Reads `out.len()` bytes from one location without the cache model.
    #[inline]
    fn read_location_direct(&self, location: MemoryLocation, out: &mut [u8]) -> bool {
        match location {
            MemoryLocation::Static { kind, offset } => {
                match self.bytes_for(kind).get(offset..offset + out.len()) {
                    Some(source) => {
                        out.copy_from_slice(source);
                        true
                    }
                    None => false,
                }
            }
            MemoryLocation::Dynamic { index, offset } => self
                .dynamic_regions
                .get(index)
                .is_some_and(|region| region.backing.backing_ref().read_range(offset, out)),
            MemoryLocation::Shared { index, offset } => self
                .shared_regions
                .get(index)
                .is_some_and(|region| region.backing.read_range(offset, out)),
            MemoryLocation::Physical { address, .. } => self
                .gpu_memory
                .as_ref()
                .is_some_and(|memory| memory.read(address, out).is_ok()),
        }
    }

    /// Writes bytes to one location without the cache model.
    #[inline]
    fn write_location_direct(&mut self, location: MemoryLocation, bytes: &[u8]) -> bool {
        match location {
            MemoryLocation::Static { kind, offset } => {
                match self
                    .bytes_for_mut(kind)
                    .get_mut(offset..offset + bytes.len())
                {
                    Some(destination) => {
                        destination.copy_from_slice(bytes);
                        true
                    }
                    None => false,
                }
            }
            MemoryLocation::Dynamic { index, offset } => self
                .dynamic_regions
                .get(index)
                .is_some_and(|region| region.backing.backing_ref().write_range(offset, bytes)),
            MemoryLocation::Shared { index, offset } => self
                .shared_regions
                .get(index)
                .is_some_and(|region| region.backing.write_range(offset, bytes)),
            MemoryLocation::Physical { address, .. } => self
                .gpu_memory
                .as_mut()
                .is_some_and(|memory| memory.write(address, bytes).is_ok()),
        }
    }

    /// Fast-memory scalar load. `None` sends the access down the precise
    /// byte path (region boundary, unmapped byte or MPCore window).
    #[inline]
    fn fast_read(&self, address: u32, len: usize) -> Option<u32> {
        let location = self.locate(address)?;
        if !self.location_is_readable(location) {
            return None;
        }
        let mut bytes = [0u8; 4];
        self.read_location_direct(location, &mut bytes[..len])
            .then(|| u32::from_le_bytes(bytes))
    }

    /// Fast-memory scalar store; returns false to fall back to the byte path.
    #[inline]
    fn fast_write(&mut self, address: u32, bytes: &[u8]) -> bool {
        let Some(location) = self.locate(address) else {
            return false;
        };
        if !self.location_is_writable(location) || !self.write_location_direct(location, bytes) {
            return false;
        }
        self.invalidate_exclusive_range(address, bytes.len());
        true
    }

    fn write_bytes(&mut self, address: u32, bytes: &[u8]) -> BusResult<()> {
        self.write_bytes_with_cache_policy(address, bytes, CachePolicy::WriteBack)
    }

    fn write_bytes_with_cache_policy(
        &mut self,
        address: u32,
        bytes: &[u8],
        policy: CachePolicy,
    ) -> BusResult<()> {
        if self.fast_memory && bytes.len() <= 8 && self.fast_write(address, bytes) {
            return Ok(());
        }
        if policy == CachePolicy::WriteBack
            && (self.try_write_cached_hit(address, bytes)
                || self.try_write_cached_miss(address, bytes)?)
        {
            self.invalidate_exclusive_range(address, bytes.len());
            return Ok(());
        }

        let mut locations = ValidatedWriteLocations::with_capacity(bytes.len());

        // Validate the complete range before mutating cache state or backing.
        // Scalar ARM11 stores (1, 2, 4 and 8 bytes) stay entirely on the stack.
        for index in 0..bytes.len() {
            let offset =
                u32::try_from(index).map_err(|_| Error::InvalidAccess { addr: address })?;
            let current_address = address
                .checked_add(offset)
                .ok_or(Error::InvalidAccess { addr: address })?;
            let location = self.locate(current_address).ok_or(Error::InvalidAccess {
                addr: current_address,
            })?;
            if !self.location_is_writable(location) {
                return Err(Error::InvalidAccess {
                    addr: current_address,
                });
            }
            locations.push(location);
        }

        for (byte, location) in bytes.iter().zip(locations.iter()) {
            let (key, target, byte_offset) = self
                .cache_location(location)
                .ok_or(Error::InvalidAccess { addr: address })?;
            match policy {
                CachePolicy::NonCacheable => {
                    self.write_uncached_byte(&target, byte_offset, *byte)?;
                }
                CachePolicy::NormalNonCacheable | CachePolicy::WriteThrough => {
                    self.write_normal_buffered_byte(&target, key, byte_offset, *byte)?;
                }
                CachePolicy::WriteBack => {
                    self.write_cached_byte(&target, key, byte_offset, *byte)?;
                }
                CachePolicy::WriteBackNoAllocate => {
                    self.write_back_no_allocate_byte(&target, key, byte_offset, *byte)?;
                }
            }
        }

        self.invalidate_exclusive_range(address, bytes.len());
        Ok(())
    }

    /// Returns true if any byte in a small scalar access targets an MPCore
    /// register window. Such accesses retain the byte/device-specific path.
    fn range_touches_mpcore_registers(&self, address: u32, length: usize) -> bool {
        let start = u64::from(address);
        let window = u64::from(crate::cpu::MPCORE_PRIVATE_MEMORY_BASE);
        if start + length as u64 <= window
            || start >= window + u64::from(crate::cpu::MPCORE_PRIVATE_MEMORY_SIZE)
        {
            return false;
        }
        (0..length).any(|index| {
            let Some(current) = address.checked_add(index as u32) else {
                return true;
            };
            self.mpcore_private_timer_register_offset(current).is_some()
                || self.mpcore_cpu_interface_register_offset(current).is_some()
                || self
                    .mpcore_gic_distributor_register_offset(current)
                    .is_some()
                || self.mpcore_scu_register_offset(current).is_some()
        })
    }

    fn static_backing_slot(kind: Arm11SegmentKind) -> usize {
        match kind {
            Arm11SegmentKind::Text => 0,
            Arm11SegmentKind::ReadOnly => 1,
            Arm11SegmentKind::Data => 2,
            Arm11SegmentKind::Bss => 3,
            Arm11SegmentKind::Stack => 4,
        }
    }

    /// Resolves a virtual or physical location to an identity that survives
    /// aliases, process mappings and shared-memory views.
    fn cache_location(
        &self,
        location: MemoryLocation,
    ) -> Option<(DataCacheKey, DataCacheTarget, usize)> {
        match location {
            MemoryLocation::Static { kind, offset } => {
                let line_offset = offset & !(ARM11_DATA_CACHE_LINE_SIZE - 1);
                let key = DataCacheKey {
                    backing_id: self.static_backing_ids[Self::static_backing_slot(kind)],
                    line_offset: line_offset as u64,
                };
                Some((
                    key,
                    DataCacheTarget::Static {
                        kind,
                        offset: line_offset,
                    },
                    offset - line_offset,
                ))
            }
            MemoryLocation::Dynamic { index, offset } => {
                let backing = self.dynamic_regions.get(index)?.backing.backing_handle();
                self.cache_location_for_backing(backing, offset)
            }
            MemoryLocation::Shared { index, offset } => {
                let backing = self.shared_regions.get(index)?.backing.clone();
                self.cache_location_for_backing(backing, offset)
            }
            MemoryLocation::Physical {
                address: physical_address,
                ..
            } => {
                let memory = self.gpu_memory.as_ref()?.clone();
                let line_address = physical_address & !((ARM11_DATA_CACHE_LINE_SIZE as u32) - 1);
                let key = DataCacheKey {
                    backing_id: self.gpu_memory_backing_id?,
                    line_offset: u64::from(line_address),
                };
                Some((
                    key,
                    DataCacheTarget::Physical {
                        memory,
                        address: line_address,
                    },
                    (physical_address - line_address) as usize,
                ))
            }
        }
    }

    fn cache_location_for_backing(
        &self,
        backing: SharedMemoryBacking,
        offset: usize,
    ) -> Option<(DataCacheKey, DataCacheTarget, usize)> {
        let root_offset = backing.root_offset(offset)?;
        let root_line = root_offset & !(ARM11_DATA_CACHE_LINE_SIZE - 1);
        let view_line = root_line.checked_sub(backing.root_base_offset())?;
        let key = DataCacheKey {
            backing_id: backing.backing_id(),
            line_offset: root_line as u64,
        };
        Some((
            key,
            DataCacheTarget::Backing {
                backing,
                offset: view_line,
            },
            root_offset - root_line,
        ))
    }

    fn cache_set_index(key: DataCacheKey) -> usize {
        ((key.backing_id ^ (key.line_offset / ARM11_DATA_CACHE_LINE_SIZE as u64)) as usize)
            & (ARM11_DATA_CACHE_SET_COUNT - 1)
    }

    fn touch_data_cache_line(&mut self, key: DataCacheKey) {
        let set_index = Self::cache_set_index(key);
        if let Some(set) = self.data_cache_lru.get_mut(set_index) {
            // Already the most recently used entry: the reorder below would be a no-op.
            if set.back() == Some(&key) {
                return;
            }
            if let Some(position) = set.iter().position(|entry| *entry == key) {
                set.remove(position);
            }
            set.push_back(key);
        }
    }

    /// Cache key and in-line offset for a location, without cloning the backing.
    fn cache_key_for(&self, location: MemoryLocation) -> Option<(DataCacheKey, usize)> {
        let line_mask = ARM11_DATA_CACHE_LINE_SIZE - 1;
        match location {
            MemoryLocation::Static { kind, offset } => Some((
                DataCacheKey {
                    backing_id: self.static_backing_ids[Self::static_backing_slot(kind)],
                    line_offset: (offset & !line_mask) as u64,
                },
                offset & line_mask,
            )),
            MemoryLocation::Dynamic { index, offset } => {
                let backing = self.dynamic_regions.get(index)?.backing.backing_ref();
                Self::cache_key_for_backing(backing, offset)
            }
            MemoryLocation::Shared { index, offset } => {
                let backing = &self.shared_regions.get(index)?.backing;
                Self::cache_key_for_backing(backing, offset)
            }
            MemoryLocation::Physical {
                address: physical_address,
                ..
            } => {
                let backing_id = self.gpu_memory_backing_id?;
                let line_address = physical_address & !(line_mask as u32);
                Some((
                    DataCacheKey {
                        backing_id,
                        line_offset: u64::from(line_address),
                    },
                    (physical_address - line_address) as usize,
                ))
            }
        }
    }

    fn cache_key_for_backing(
        backing: &SharedMemoryBacking,
        offset: usize,
    ) -> Option<(DataCacheKey, usize)> {
        let root_offset = backing.root_offset(offset)?;
        let root_line = root_offset & !(ARM11_DATA_CACHE_LINE_SIZE - 1);
        Some((
            DataCacheKey {
                backing_id: backing.backing_id(),
                line_offset: root_line as u64,
            },
            root_offset - root_line,
        ))
    }

    /// Write-back store that hits a resident line entirely inside one line.
    ///
    /// Mirrors the per-byte `write_cached_byte` path for the common case and
    /// returns `false` (state untouched) when the full path is required.
    fn try_write_cached_hit(&mut self, address: u32, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let Some(location) = self.locate(address) else {
            return false;
        };
        if !self.location_is_writable(location) {
            return false;
        }
        let Some((key, offset)) = self.cache_key_for(location) else {
            return false;
        };
        if offset + bytes.len() > ARM11_DATA_CACHE_LINE_SIZE {
            return false;
        }
        let Some(line) = self.data_cache.get_mut(&key) else {
            return false;
        };
        line.bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
        line.dirty = true;
        self.touch_data_cache_line(key);
        true
    }

    /// Handles a small write-back store that misses in the D-cache but stays
    /// inside one resident-line-sized span. Every guest byte is validated
    /// against the same backing line before that line is loaded or modified.
    fn try_write_cached_miss(&mut self, address: u32, bytes: &[u8]) -> BusResult<bool> {
        if bytes.is_empty() || bytes.len() > INLINE_WRITE_LOCATION_CAPACITY {
            return Ok(false);
        }

        let Some(location) = self.locate(address) else {
            return Ok(false);
        };
        if !self.location_is_writable(location) {
            return Ok(false);
        }
        let Some((key, offset)) = self.cache_key_for(location) else {
            return Ok(false);
        };
        let Some(end) = offset.checked_add(bytes.len()) else {
            return Ok(false);
        };
        if end > ARM11_DATA_CACHE_LINE_SIZE {
            return Ok(false);
        }

        // Do not assume neighboring virtual addresses share a backing store.
        // Verify every byte before filling the line, so cross-region writes
        // preserve the general path's validate-before-mutate guarantee.
        for index in 1..bytes.len() {
            let current_address = address
                .checked_add(index as u32)
                .ok_or(Error::InvalidAccess { addr: address })?;
            let Some(current_location) = self.locate(current_address) else {
                return Ok(false);
            };
            if !self.location_is_writable(current_location) {
                return Ok(false);
            }
            let Some((current_key, current_offset)) = self.cache_key_for(current_location) else {
                return Ok(false);
            };
            if current_key != key || current_offset != offset + index {
                return Ok(false);
            }
        }

        let Some((target_key, target, target_offset)) = self.cache_location(location) else {
            return Ok(false);
        };
        debug_assert_eq!(target_key, key);
        debug_assert_eq!(target_offset, offset);

        let mut line_bytes = self.read_cache_target_line(&target)?;
        self.overlay_write_buffer(key, &mut line_bytes);
        line_bytes[offset..end].copy_from_slice(bytes);
        self.insert_data_cache_line(
            key,
            Arm11DataCacheLine {
                bytes: line_bytes,
                dirty: true,
                target,
            },
        )?;

        Ok(true)
    }

    /// Reads immutable bytes from the prepared static text segment.
    ///
    /// Guest writes cannot modify this executable backing, so instruction
    /// fetch can bypass the data-cache model entirely.
    #[inline]
    fn fetch_text_bytes<const N: usize>(&self, address: u32) -> Option<[u8; N]> {
        let segment = self.layout.text;
        let offset = usize::try_from(address.checked_sub(segment.address)?).ok()?;
        let bytes = self.text.get(offset..offset.checked_add(N)?)?;
        if offset + N > segment.mapped_size_bytes as usize {
            return None;
        }
        bytes.try_into().ok()
    }

    /// Reads a cacheable scalar access with one address resolution and one
    /// D-cache lookup, filling the line directly on a miss when the access
    /// does not cross a cache-line boundary.
    fn read_cached_value(&mut self, address: u32, len: usize) -> BusResult<Option<u32>> {
        if !(1..=4).contains(&len) {
            return Ok(None);
        }
        if self.fast_memory {
            if let Some(value) = self.fast_read(address, len) {
                return Ok(Some(value));
            }
        }
        let Some(location) = self.locate(address) else {
            return Ok(None);
        };
        if !self.location_is_readable(location) {
            return Ok(None);
        }
        let Some((key, offset)) = self.cache_key_for(location) else {
            return Ok(None);
        };
        let Some(end) = offset.checked_add(len) else {
            return Ok(None);
        };
        if end > ARM11_DATA_CACHE_LINE_SIZE {
            return Ok(None);
        }

        let mut value_bytes = [0u8; 4];
        if let Some(line) = self.data_cache.get(&key) {
            let Some(source) = line.bytes.get(offset..end) else {
                return Ok(None);
            };
            value_bytes[..len].copy_from_slice(source);
            self.touch_data_cache_line(key);
            return Ok(Some(u32::from_le_bytes(value_bytes)));
        }

        let Some((target_key, target, target_offset)) = self.cache_location(location) else {
            return Ok(None);
        };
        debug_assert_eq!(target_key, key);
        debug_assert_eq!(target_offset, offset);

        let mut line_bytes = self.read_cache_target_line(&target)?;
        self.overlay_write_buffer(key, &mut line_bytes);
        let Some(source) = line_bytes.get(offset..end) else {
            return Ok(None);
        };
        value_bytes[..len].copy_from_slice(source);
        self.insert_data_cache_line(
            key,
            Arm11DataCacheLine {
                bytes: line_bytes,
                dirty: false,
                target,
            },
        )?;

        Ok(Some(u32::from_le_bytes(value_bytes)))
    }

    fn read_cache_target_line(
        &self,
        target: &DataCacheTarget,
    ) -> BusResult<[u8; ARM11_DATA_CACHE_LINE_SIZE]> {
        let mut bytes = [0u8; ARM11_DATA_CACHE_LINE_SIZE];
        match target {
            DataCacheTarget::Static { kind, offset } => {
                let source = self
                    .bytes_for(*kind)
                    .get(*offset..*offset + ARM11_DATA_CACHE_LINE_SIZE)
                    .ok_or(Error::InvalidAccess { addr: 0 })?;
                bytes.copy_from_slice(source);
            }
            DataCacheTarget::Backing { backing, offset } => {
                if !backing.read_range(*offset, &mut bytes) {
                    return Err(Error::InvalidAccess { addr: 0 });
                }
            }
            DataCacheTarget::Physical { memory, address } => {
                memory
                    .read(*address, &mut bytes)
                    .map_err(|_| Error::InvalidAccess { addr: *address })?;
            }
        }
        Ok(bytes)
    }

    fn write_cache_target_range(
        &mut self,
        target: &DataCacheTarget,
        start: usize,
        bytes: &[u8],
    ) -> BusResult<()> {
        match target {
            DataCacheTarget::Static { kind, offset } => {
                let begin = offset
                    .checked_add(start)
                    .ok_or(Error::InvalidAccess { addr: 0 })?;
                let end = begin
                    .checked_add(bytes.len())
                    .ok_or(Error::InvalidAccess { addr: 0 })?;
                let destination = self
                    .bytes_for_mut(*kind)
                    .get_mut(begin..end)
                    .ok_or(Error::InvalidAccess { addr: 0 })?;
                destination.copy_from_slice(bytes);
            }
            DataCacheTarget::Backing { backing, offset } => {
                let begin = offset
                    .checked_add(start)
                    .ok_or(Error::InvalidAccess { addr: 0 })?;
                if !backing.write_range(begin, bytes) {
                    return Err(Error::InvalidAccess { addr: 0 });
                }
            }
            DataCacheTarget::Physical { memory, address } => {
                let physical_address = address
                    .checked_add(start as u32)
                    .ok_or(Error::InvalidAccess { addr: *address })?;
                let mut memory = memory.clone();
                memory
                    .write(physical_address, bytes)
                    .map_err(|_| Error::InvalidAccess {
                        addr: physical_address,
                    })?;
            }
        }
        Ok(())
    }

    /// Inserts a line into the four-way set-associative cache. Dirty victims
    /// are written back through the backing identity retained by the line.
    fn insert_data_cache_line(
        &mut self,
        key: DataCacheKey,
        line: Arm11DataCacheLine,
    ) -> BusResult<()> {
        let set_index = Self::cache_set_index(key);
        let victim = {
            let set = self
                .data_cache_lru
                .get(set_index)
                .ok_or(Error::InvalidAccess { addr: 0 })?;
            if set.len() >= ARM11_DATA_CACHE_WAYS {
                set.front().copied()
            } else {
                None
            }
        };

        if let Some(victim_key) = victim {
            // Preserve the resident line until a dirty writeback has been
            // accepted by the FIFO. A failed back-pressure drain must not
            // silently discard modified cache state.
            if let Some(victim_line) = self.data_cache.get(&victim_key).cloned() {
                if victim_line.dirty {
                    self.enqueue_data_cache_writeback(
                        victim_key,
                        victim_line.target,
                        victim_line.bytes,
                    )?;
                }
            }
            self.remove_data_cache_line(victim_key);
        }

        if let Some(set) = self.data_cache_lru.get_mut(set_index) {
            set.push_back(key);
        }
        self.data_cache.insert(key, line);
        Ok(())
    }

    fn remove_data_cache_line(&mut self, key: DataCacheKey) -> Option<Arm11DataCacheLine> {
        let set_index = Self::cache_set_index(key);
        if let Some(set) = self.data_cache_lru.get_mut(set_index) {
            if let Some(position) = set.iter().position(|entry| *entry == key) {
                set.remove(position);
            }
        }
        self.data_cache.remove(&key)
    }

    fn overlay_write_buffer(&self, key: DataCacheKey, output: &mut [u8]) {
        for entry in &self.write_buffer {
            if entry.key != key {
                continue;
            }
            let start = usize::from(entry.start);
            let length = usize::from(entry.length);
            let end = start.saturating_add(length).min(output.len());
            if start >= end {
                continue;
            }
            output[start..end].copy_from_slice(&entry.bytes[..end - start]);
        }
    }

    fn enqueue_data_cache_writeback(
        &mut self,
        key: DataCacheKey,
        target: DataCacheTarget,
        bytes: [u8; ARM11_DATA_CACHE_LINE_SIZE],
    ) -> BusResult<()> {
        if self.write_buffer.len() >= ARM11_WRITE_BUFFER_CAPACITY {
            self.drain_write_buffer_entries(1)?;
        }
        self.write_buffer.push_back(Arm11WriteBufferEntry {
            key,
            target,
            start: 0,
            bytes,
            length: ARM11_DATA_CACHE_LINE_SIZE as u8,
        });
        Ok(())
    }

    /// Queues one bufferable Normal-memory store. Adjacent stores coalesce only
    /// when they target the same backing line and occupy consecutive bytes.
    fn enqueue_buffered_normal_store(
        &mut self,
        key: DataCacheKey,
        target: DataCacheTarget,
        offset: usize,
        value: u8,
    ) -> BusResult<()> {
        if let Some(entry) = self.write_buffer.back_mut() {
            let start = usize::from(entry.start);
            let length = usize::from(entry.length);
            if entry.key == key && length < ARM11_DATA_CACHE_LINE_SIZE && start + length == offset {
                entry.bytes[offset] = value;
                entry.length += 1;
                return Ok(());
            }
        }

        if self.write_buffer.len() >= ARM11_WRITE_BUFFER_CAPACITY {
            self.drain_write_buffer_entries(1)?;
        }
        let mut bytes = [0; ARM11_DATA_CACHE_LINE_SIZE];
        bytes[offset] = value;
        self.write_buffer.push_back(Arm11WriteBufferEntry {
            key,
            target,
            start: offset as u8,
            bytes,
            length: 1,
        });
        Ok(())
    }

    fn drain_write_buffer_entries(&mut self, maximum: usize) -> BusResult<usize> {
        let mut drained = 0;
        while drained < maximum {
            let Some(entry) = self.write_buffer.front().cloned() else {
                break;
            };
            let start = usize::from(entry.start);
            let end = start + usize::from(entry.length);
            self.write_cache_target_range(&entry.target, start, &entry.bytes[start..end])?;
            self.write_buffer.pop_front();
            drained += 1;
        }
        Ok(drained)
    }

    /// Completes queued ARM11 D-cache writebacks and bufferable stores in FIFO order.
    pub fn drain_arm11_write_buffer(&mut self) -> BusResult<usize> {
        self.drain_write_buffer_entries(usize::MAX)
    }

    /// Number of queued Normal-memory stores and dirty cache-line writebacks.
    pub fn pending_write_buffer_entries(&self) -> usize {
        self.write_buffer.len()
    }

    fn read_normal_buffered_byte(
        &self,
        target: &DataCacheTarget,
        key: DataCacheKey,
        offset: usize,
    ) -> BusResult<u8> {
        let mut bytes = self.read_cache_target_line(target)?;
        self.overlay_write_buffer(key, &mut bytes);
        bytes
            .get(offset)
            .copied()
            .ok_or(Error::InvalidAccess { addr: 0 })
    }

    /// Device/Strongly Ordered reads do not forward from Normal-memory stores:
    /// older buffered writes must complete before the read reaches the bus.
    fn read_ordered_byte(&mut self, target: &DataCacheTarget, offset: usize) -> BusResult<u8> {
        self.drain_arm11_write_buffer()?;
        self.read_cache_target_line(target)?
            .get(offset)
            .copied()
            .ok_or(Error::InvalidAccess { addr: 0 })
    }

    fn write_uncached_byte(
        &mut self,
        target: &DataCacheTarget,
        offset: usize,
        value: u8,
    ) -> BusResult<()> {
        self.drain_arm11_write_buffer()?;
        self.write_cache_target_range(target, offset, &[value])
    }

    /// Queues a bufferable Normal-memory store. A resident line is updated
    /// immediately for this CPU, while other agents see it only after drain.
    /// Queues a bufferable Normal-memory store. A resident line is updated
    /// immediately for this CPU, while other agents see it only after drain.
    fn write_normal_buffered_byte(
        &mut self,
        target: &DataCacheTarget,
        key: DataCacheKey,
        offset: usize,
        value: u8,
    ) -> BusResult<()> {
        if self.fast_memory {
            return self.write_cache_target_range(target, offset, &[value]);
        }
        let resident = if let Some(line) = self.data_cache.get_mut(&key) {
            let was_dirty = line.dirty;
            line.bytes[offset] = value;
            line.dirty = true;
            Some((was_dirty, line.bytes, line.target.clone()))
        } else {
            None
        };

        if let Some((was_dirty, bytes, line_target)) = resident {
            if was_dirty {
                self.enqueue_data_cache_writeback(key, line_target, bytes)?;
            } else {
                self.enqueue_buffered_normal_store(key, target.clone(), offset, value)?;
            }
            if let Some(line) = self.data_cache.get_mut(&key) {
                line.dirty = false;
            }
            self.touch_data_cache_line(key);
            Ok(())
        } else {
            self.enqueue_buffered_normal_store(key, target.clone(), offset, value)
        }
    }

    fn read_cached_byte(
        &mut self,
        target: &DataCacheTarget,
        key: DataCacheKey,
        offset: usize,
    ) -> BusResult<u8> {
        if self.fast_memory {
            return self
                .read_cache_target_line(target)?
                .get(offset)
                .copied()
                .ok_or(Error::InvalidAccess { addr: 0 });
        }
        let _ = key;
        // Resolve a resident line with one tree lookup. The previous
        // contains_key + get sequence walked the BTreeMap twice on every hit.
        if let Some(value) = self
            .data_cache
            .get(&key)
            .and_then(|line| line.bytes.get(offset).copied())
        {
            self.touch_data_cache_line(key);
            return Ok(value);
        }

        let mut bytes = self.read_cache_target_line(target)?;
        self.overlay_write_buffer(key, &mut bytes);
        let value = bytes
            .get(offset)
            .copied()
            .ok_or(Error::InvalidAccess { addr: 0 })?;
        self.insert_data_cache_line(
            key,
            Arm11DataCacheLine {
                bytes,
                dirty: false,
                target: target.clone(),
            },
        )?;

        Ok(value)
    }

    fn write_cached_byte(
        &mut self,
        target: &DataCacheTarget,
        key: DataCacheKey,
        offset: usize,
        value: u8,
    ) -> BusResult<()> {
        if self.fast_memory {
            return self.write_cache_target_range(target, offset, &[value]);
        }
        // A resident write needs only one mutable lookup; loading the old byte
        // first added no architectural effect because this operation replaces it.
        if let Some(line) = self.data_cache.get_mut(&key) {
            let byte = line
                .bytes
                .get_mut(offset)
                .ok_or(Error::InvalidAccess { addr: 0 })?;
            *byte = value;
            line.dirty = true;
            self.touch_data_cache_line(key);
            return Ok(());
        }

        // On a miss, allocate/fill the line first so untouched bytes and any
        // pending write-buffer overlays retain their existing semantics.
        self.read_cached_byte(target, key, offset)?;
        let line = self
            .data_cache
            .get_mut(&key)
            .ok_or(Error::InvalidAccess { addr: 0 })?;
        line.bytes[offset] = value;
        line.dirty = true;
        Ok(())
    }

    fn write_back_no_allocate_byte(
        &mut self,
        target: &DataCacheTarget,
        key: DataCacheKey,
        offset: usize,
        value: u8,
    ) -> BusResult<()> {
        if self.fast_memory {
            return self.write_cache_target_range(target, offset, &[value]);
        }
        if let Some(line) = self.data_cache.get_mut(&key) {
            line.bytes[offset] = value;
            line.dirty = true;
            self.touch_data_cache_line(key);
            Ok(())
        } else {
            self.enqueue_buffered_normal_store(key, target.clone(), offset, value)
        }
    }

    fn cache_lines_for_range(
        &self,
        address: u32,
        size_bytes: u32,
    ) -> BusResult<std::collections::BTreeSet<DataCacheKey>> {
        self.cache_lines_for_range_with_access(address, size_bytes, true)
    }

    fn cache_lines_for_kernel_maintenance(
        &self,
        address: u32,
        size_bytes: u32,
    ) -> BusResult<std::collections::BTreeSet<DataCacheKey>> {
        self.cache_lines_for_range_with_access(address, size_bytes, false)
    }

    fn cache_lines_for_range_with_access(
        &self,
        address: u32,
        size_bytes: u32,
        require_read_permission: bool,
    ) -> BusResult<std::collections::BTreeSet<DataCacheKey>> {
        let mut lines = std::collections::BTreeSet::new();
        if size_bytes == 0 {
            return Ok(lines);
        }

        let end = u64::from(address) + u64::from(size_bytes);
        if end > u64::from(u32::MAX) + 1 {
            return Err(Error::InvalidAccess { addr: address });
        }

        let mut current = u64::from(address);
        while current < end {
            let current_address =
                u32::try_from(current).map_err(|_| Error::InvalidAccess { addr: address })?;
            let location = self.locate(current_address).ok_or(Error::InvalidAccess {
                addr: current_address,
            })?;
            if require_read_permission && !self.location_is_readable(location) {
                return Err(Error::InvalidAccess {
                    addr: current_address,
                });
            }

            let region_end = self.location_end(location).unwrap_or(end).min(end);
            if region_end <= current {
                return Err(Error::InvalidAccess {
                    addr: current_address,
                });
            }

            let (mut key, _target, offset) =
                self.cache_location(location).ok_or(Error::InvalidAccess {
                    addr: current_address,
                })?;
            // Only resident lines matter to every caller: filter the (at most
            // 512) resident lines by span instead of enumerating each
            // possible line of a large range.
            let remaining = region_end - current;
            let line_size = ARM11_DATA_CACHE_LINE_SIZE as u64;
            let line_count = (offset as u64 + remaining).div_ceil(line_size);
            let first = key;
            key.line_offset += line_size * (line_count - 1);
            for resident in self.data_cache.keys_in(first..=key) {
                lines.insert(*resident);
            }

            current = region_end;
        }

        Ok(lines)
    }

    /// Cleans and invalidates every cache line intersecting a process virtual range.
    /// The complete range is checked before any line is written back or evicted.
    pub fn flush_data_cache_range(&mut self, address: u32, size_bytes: u32) -> BusResult<usize> {
        self.flush_data_cache_range_with_access(address, size_bytes, true)
    }

    /// Kernel-side writeback for unmap/free/protect operations. Unlike a guest cache
    /// operation, VM maintenance must work on write-only and no-access mappings.
    pub fn flush_data_cache_range_for_kernel(
        &mut self,
        address: u32,
        size_bytes: u32,
    ) -> BusResult<usize> {
        self.flush_data_cache_range_with_access(address, size_bytes, false)
    }

    fn flush_data_cache_range_with_access(
        &mut self,
        address: u32,
        size_bytes: u32,
        require_read_permission: bool,
    ) -> BusResult<usize> {
        let lines = if require_read_permission {
            self.cache_lines_for_range(address, size_bytes)?
        } else {
            self.cache_lines_for_kernel_maintenance(address, size_bytes)?
        };
        let mut written_back = 0usize;

        for key in lines {
            let pending = self
                .data_cache
                .get(&key)
                .map(|line| (line.dirty, line.bytes, line.target.clone()));
            if let Some((dirty, bytes, target)) = pending {
                if dirty {
                    self.enqueue_data_cache_writeback(key, target, bytes)?;
                    written_back += 1;
                }
                self.remove_data_cache_line(key);
            }
        }

        Ok(written_back)
    }

    /// Writes dirty lines back while keeping the lines resident and clean.
    pub fn store_data_cache_range(&mut self, address: u32, size_bytes: u32) -> BusResult<usize> {
        let lines = self.cache_lines_for_range(address, size_bytes)?;
        let mut written_back = 0usize;

        for key in lines {
            let dirty = self
                .data_cache
                .get(&key)
                .filter(|line| line.dirty)
                .map(|line| (line.bytes, line.target.clone()));
            if let Some((bytes, target)) = dirty {
                self.enqueue_data_cache_writeback(key, target, bytes)?;
                if let Some(line) = self.data_cache.get_mut(&key) {
                    line.dirty = false;
                }
                written_back += 1;
            }
        }

        Ok(written_back)
    }

    /// Invalidates cache lines without writing dirty data back to their backing.
    pub fn invalidate_data_cache_range(
        &mut self,
        address: u32,
        size_bytes: u32,
    ) -> BusResult<usize> {
        self.invalidate_data_cache_range_with_access(address, size_bytes, true)
    }

    /// Kernel-side invalidation after a mapping loses read permission.
    pub fn invalidate_data_cache_range_for_kernel(
        &mut self,
        address: u32,
        size_bytes: u32,
    ) -> BusResult<usize> {
        self.invalidate_data_cache_range_with_access(address, size_bytes, false)
    }

    fn invalidate_data_cache_range_with_access(
        &mut self,
        address: u32,
        size_bytes: u32,
        require_read_permission: bool,
    ) -> BusResult<usize> {
        let lines = self.cache_lines_for_range_with_access(
            address,
            size_bytes,
            require_read_permission,
        )?;
        let mut evicted = 0usize;

        for key in lines {
            if self.remove_data_cache_line(key).is_some() {
                evicted += 1;
            }
        }

        Ok(evicted)
    }

    /// Writes back and invalidates cached lines of every mapping of one shared
    /// backing, so an HLE service reads what the guest stored through its cache.
    pub fn flush_shared_backing_data_cache(
        &mut self,
        backing: &SharedMemoryBacking,
    ) -> BusResult<usize> {
        let ranges: Vec<(u32, u32)> = self
            .shared_regions
            .iter()
            .filter(|region| region.backing.shares_backing_with(backing))
            .filter_map(|region| Some((region.address, region.size()?)))
            .collect();
        let mut flushed = 0;
        for (address, size) in ranges {
            flushed += self.flush_data_cache_range(address, size)?;
        }
        Ok(flushed)
    }

    /// Invalidates cached lines of every mapping of one shared backing.
    ///
    /// An HLE service that publishes into shared memory is not an emulated
    /// process, so it models the coherent agent whose store the cores observe:
    /// stale lines in this process' cache are dropped before the guest reads.
    pub fn invalidate_shared_backing_data_cache(
        &mut self,
        backing: &SharedMemoryBacking,
    ) -> BusResult<usize> {
        let ranges: Vec<(u32, u32)> = self
            .shared_regions
            .iter()
            .filter(|region| region.backing.shares_backing_with(backing))
            .filter_map(|region| Some((region.address, region.size()?)))
            .collect();
        let mut evicted = 0;
        for (address, size) in ranges {
            evicted += self.invalidate_data_cache_range(address, size)?;
        }
        Ok(evicted)
    }

    /// Returns a snapshot of guest virtual reservations and host backing usage.
    ///
    /// A heap-backed shared view is counted once by backing identity, even when
    /// the same host pages are visible through both heap and shared mappings.
    /// Virtual sizes remain separate so aliasing is visible in reports.
    pub fn memory_usage_report(&self) -> Arm11MemoryUsageReport {
        let static_segment_allocated_bytes = self
            .text
            .capacity()
            .saturating_add(self.read_only.capacity())
            .saturating_add(self.data.capacity())
            .saturating_add(self.bss.capacity())
            .saturating_add(self.stack.capacity());

        let application_heap_virtual_bytes = self
            .dynamic_regions
            .iter()
            .filter(|region| {
                region.address >= APPLICATION_HEAP_BASE
                    && u64::from(region.address) < APPLICATION_HEAP_END
            })
            .fold(0usize, |total, region| total.saturating_add(region.len()));
        let dynamic_region_virtual_bytes = self
            .dynamic_regions
            .iter()
            .fold(0usize, |total, region| total.saturating_add(region.len()));
        let shared_mapping_virtual_bytes =
            self.shared_regions.iter().fold(0usize, |total, region| {
                total.saturating_add(region.backing.length)
            });

        let mut linear_physical_ranges = self
            .linear_regions
            .iter()
            .map(|region| {
                let start = u64::from(region.physical_base);
                let end = start.saturating_add(u64::from(region.size));
                (start, end)
            })
            .collect::<Vec<_>>();
        linear_physical_ranges.sort_unstable_by_key(|(start, _)| *start);
        let mut linear_physical_allocated_bytes = 0usize;
        let mut previous_physical_end = 0u64;
        for (start, end) in linear_physical_ranges {
            if end <= previous_physical_end {
                continue;
            }
            let unique_start = start.max(previous_physical_end);
            linear_physical_allocated_bytes =
                linear_physical_allocated_bytes.saturating_add((end - unique_start) as usize);
            previous_physical_end = end;
        }

        let mut unique_backings = BTreeMap::<u64, &SharedMemoryBacking>::new();
        for region in &self.dynamic_regions {
            let backing = region.backing.backing_ref();
            unique_backings
                .entry(backing.backing_id())
                .or_insert(backing);
        }
        for region in &self.shared_regions {
            let backing = &region.backing;
            unique_backings
                .entry(backing.backing_id())
                .or_insert(backing);
        }

        let (
            sparse_backing_pages,
            sparse_backing_allocated_bytes,
            sparse_exclusive_monitor_entries,
        ) = unique_backings
            .values()
            .fold((0usize, 0usize, 0usize), |totals, backing| {
                let (pages, bytes) = backing.allocated_page_stats();
                (
                    totals.0.saturating_add(pages),
                    totals.1.saturating_add(bytes),
                    totals
                        .2
                        .saturating_add(backing.exclusive_monitor_entry_count()),
                )
            });
        let gpu_physical_pages = self
            .gpu_memory
            .as_ref()
            .map_or(0, GpuPhysicalMemory::allocated_page_count);

        Arm11MemoryUsageReport {
            static_segment_allocated_bytes,
            application_heap_virtual_bytes,
            dynamic_region_virtual_bytes,
            shared_mapping_virtual_bytes,
            linear_physical_allocated_bytes,
            unique_sparse_backing_count: unique_backings.len(),
            sparse_backing_pages,
            sparse_backing_allocated_bytes,
            sparse_exclusive_monitor_entries,
            data_cache_lines: self.data_cache.len(),
            data_cache_bytes: self
                .data_cache
                .len()
                .saturating_mul(ARM11_DATA_CACHE_LINE_SIZE),
            write_buffer_entries: self.write_buffer.len(),
            write_buffer_capacity: self.write_buffer.capacity(),
            write_buffer_reserved_bytes: self
                .write_buffer
                .capacity()
                .saturating_mul(std::mem::size_of::<Arm11WriteBufferEntry>()),
            virtual_exclusive_monitor_entries: self.exclusive_versions.len(),
            tls_slots_total: self.tls_slots_in_use.len(),
            tls_slots_in_use: self.tls_slots_in_use.iter().filter(|used| **used).count(),
            ipc_temporary_mapping_count: self.ipc_temporary_regions.len(),
            gpu_physical_pages,
            gpu_physical_allocated_bytes: gpu_physical_pages
                .saturating_mul(SHARED_BACKING_PAGE_SIZE),
        }
    }

    /// Number of resident ARM11 data-cache lines (useful for diagnostics/tests).
    pub fn data_cache_line_count(&self) -> usize {
        self.data_cache.len()
    }

    fn exclusive_version(&self, address: u32) -> u64 {
        let virtual_version = self
            .exclusive_versions
            .get(&(address & !(crate::memory::EXCLUSIVE_RESERVATION_GRANULE - 1)))
            .copied()
            .unwrap_or(0);
        let backing_version = match self.locate(address) {
            Some(MemoryLocation::Physical {
                address: physical, ..
            }) => self
                .gpu_memory
                .as_ref()
                .map_or(0, |memory| memory.exclusive_version(physical)),
            Some(MemoryLocation::Shared { index, offset }) => self
                .shared_regions
                .get(index)
                .map_or(0, |region| region.backing.exclusive_version(offset)),
            Some(MemoryLocation::Dynamic { index, offset }) => {
                self.dynamic_regions.get(index).map_or(0, |region| {
                    region.backing.backing_handle().exclusive_version(offset)
                })
            }
            Some(MemoryLocation::Static { .. }) | None => 0,
        };
        virtual_version.wrapping_add(backing_version)
    }

    fn invalidate_exclusive_range(&mut self, address: u32, size: usize) {
        if size == 0 {
            return;
        }
        let end = u64::from(address) + size as u64;
        let mut granule = u64::from(address & !(crate::memory::EXCLUSIVE_RESERVATION_GRANULE - 1));
        while granule < end {
            let key = granule as u32;
            if let Some(version) = self.exclusive_versions.get_mut(&key) {
                *version = version.wrapping_add(1);
            }
            granule += u64::from(crate::memory::EXCLUSIVE_RESERVATION_GRANULE);
        }
    }

    fn validate_exclusive_access(address: u32, size: usize) -> BusResult<()> {
        if !matches!(size, 1 | 2 | 4 | 8) || !(address as usize).is_multiple_of(size) {
            return Err(Error::InvalidAccess { addr: address });
        }
        Ok(())
    }
}

impl Bus for Arm11CodeAddressSpace {
    fn read8(&mut self, address: u32) -> BusResult<u8> {
        if self.fast_memory {
            if let Some(value) = self.fast_read(address, 1) {
                return Ok(value as u8);
            }
        }
        self.read8_with_cache_policy(address, CachePolicy::WriteBack)
    }

    fn read8_with_cache_policy(&mut self, address: u32, policy: CachePolicy) -> BusResult<u8> {
        if self.mpcore_private_timer_register_offset(address).is_some()
            || self.mpcore_cpu_interface_register_offset(address).is_some()
        {
            return Err(Error::InvalidAccess { addr: address });
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.read_mpcore_gic_distributor_byte(address);
        }
        if self.mpcore_scu_register_offset(address).is_some() {
            let register = self.read_mpcore_scu_register(address)?;
            return Ok((register >> ((address & 3) * 8)) as u8);
        }
        if matches!(
            policy,
            CachePolicy::WriteThrough | CachePolicy::WriteBack | CachePolicy::WriteBackNoAllocate
        ) {
            if let Some(value) = self.read_cached_value(address, 1)? {
                return Ok(value as u8);
            }
        }
        let location = self
            .locate(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        if !self.location_is_readable(location) {
            return Err(Error::InvalidAccess { addr: address });
        }
        let (key, target, offset) = self
            .cache_location(location)
            .ok_or(Error::InvalidAccess { addr: address })?;
        match policy {
            CachePolicy::NonCacheable => self.read_ordered_byte(&target, offset),
            CachePolicy::NormalNonCacheable => self.read_normal_buffered_byte(&target, key, offset),
            CachePolicy::WriteThrough
            | CachePolicy::WriteBack
            | CachePolicy::WriteBackNoAllocate => self.read_cached_byte(&target, key, offset),
        }
    }

    fn read16(&mut self, address: u32) -> BusResult<u16> {
        if self.mpcore_private_timer_register_offset(address).is_some()
            || self.mpcore_cpu_interface_register_offset(address).is_some()
        {
            return Err(Error::InvalidAccess { addr: address });
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            let low = self.read_mpcore_gic_distributor_byte(address)?;
            let high = self.read_mpcore_gic_distributor_byte(address.wrapping_add(1))?;
            return Ok(u16::from_le_bytes([low, high]));
        }
        if let Some(value) = self.read_cached_value(address, 2)? {
            return Ok(value as u16);
        }
        let lo = self.read8(address)?;
        let hi = self.read8(address.wrapping_add(1))?;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    fn read16_with_cache_policy(&mut self, address: u32, policy: CachePolicy) -> BusResult<u16> {
        if matches!(
            policy,
            CachePolicy::WriteThrough | CachePolicy::WriteBack | CachePolicy::WriteBackNoAllocate
        ) && !self.range_touches_mpcore_registers(address, 2)
        {
            if let Some(value) = self.read_cached_value(address, 2)? {
                return Ok(value as u16);
            }
        }

        let low = self.read8_with_cache_policy(address, policy)?;
        let high = self.read8_with_cache_policy(address.wrapping_add(1), policy)?;
        Ok(u16::from_le_bytes([low, high]))
    }

    fn fetch16(&mut self, address: u32) -> BusResult<u16> {
        if let Some(value) = self.fetch_text_bytes::<2>(address) {
            return Ok(u16::from_le_bytes(value));
        }
        self.read16(address)
    }

    fn fetch32(&mut self, address: u32) -> BusResult<u32> {
        if let Some(value) = self.fetch_text_bytes::<4>(address) {
            return Ok(u32::from_le_bytes(value));
        }
        self.read32(address)
    }

    fn immutable_instruction_location(
        &mut self,
        address: u32,
        size: u32,
    ) -> Option<ImmutableCodeLocation> {
        let text = self.layout.text;
        if size == 0 || !text.executable || text.writable {
            return None;
        }
        let offset = usize::try_from(address.checked_sub(text.address)?).ok()?;
        let end = offset.checked_add(usize::try_from(size).ok()?)?;
        if end > text.mapped_size_bytes as usize || end > self.text.len() {
            return None;
        }
        Some(ImmutableCodeLocation {
            backing_id: self.static_backing_ids[0],
            offset: offset as u64,
        })
    }

    fn read32(&mut self, address: u32) -> BusResult<u32> {
        if self.mpcore_private_timer_register_offset(address).is_some() {
            return self.read_mpcore_private_timer_register(address);
        }
        if self.mpcore_cpu_interface_register_offset(address).is_some() {
            return self.read_mpcore_cpu_interface_register(address);
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.read_mpcore_gic_distributor_word(address);
        }
        if let Some(value) = self.read_cached_value(address, 4)? {
            return Ok(value);
        }
        let mut bytes = [0u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read8(address.wrapping_add(index as u32))?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn write8(&mut self, address: u32, value: u8) -> BusResult<()> {
        if self.mpcore_private_timer_register_offset(address).is_some()
            || self.mpcore_cpu_interface_register_offset(address).is_some()
        {
            return Err(Error::InvalidAccess { addr: address });
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.write_mpcore_gic_distributor_byte(address, value);
        }
        if self.mpcore_scu_register_offset(address).is_some() {
            return self.write8_with_cache_policy(address, value, CachePolicy::NonCacheable);
        }
        self.write_bytes(address, &[value])
    }

    fn write8_with_cache_policy(
        &mut self,
        address: u32,
        value: u8,
        policy: CachePolicy,
    ) -> BusResult<()> {
        if self.mpcore_private_timer_register_offset(address).is_some()
            || self.mpcore_cpu_interface_register_offset(address).is_some()
        {
            return Err(Error::InvalidAccess { addr: address });
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.write_mpcore_gic_distributor_byte(address, value);
        }
        if self.mpcore_scu_register_offset(address).is_some() {
            let register = self.read_mpcore_scu_register(address)?;
            let shift = (address & 3) * 8;
            let mask = 0xFFu32 << shift;
            let updated = (register & !mask) | (u32::from(value) << shift);
            return self.write_mpcore_scu_register(address, updated);
        }
        self.write_bytes_with_cache_policy(address, &[value], policy)
    }

    fn read32_with_cache_policy(&mut self, address: u32, policy: CachePolicy) -> BusResult<u32> {
        if self.mpcore_private_timer_register_offset(address).is_some() {
            return self.read_mpcore_private_timer_register(address);
        }
        if self.mpcore_cpu_interface_register_offset(address).is_some() {
            return self.read_mpcore_cpu_interface_register(address);
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.read_mpcore_gic_distributor_word(address);
        }
        if self.mpcore_scu_register_offset(address).is_some() && address & 3 == 0 {
            return self.read_mpcore_scu_register(address);
        }
        if matches!(
            policy,
            CachePolicy::WriteThrough | CachePolicy::WriteBack | CachePolicy::WriteBackNoAllocate
        ) {
            if let Some(value) = self.read_cached_value(address, 4)? {
                return Ok(value);
            }
        }
        let mut bytes = [0u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read8_with_cache_policy(address.wrapping_add(index as u32), policy)?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn write32_with_cache_policy(
        &mut self,
        address: u32,
        value: u32,
        policy: CachePolicy,
    ) -> BusResult<()> {
        if self.mpcore_private_timer_register_offset(address).is_some() {
            return self.write_mpcore_private_timer_register(address, value);
        }
        if self.mpcore_cpu_interface_register_offset(address).is_some() {
            return self.write_mpcore_cpu_interface_register(address, value);
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.write_mpcore_gic_distributor_word(address, value);
        }
        if self.mpcore_scu_register_offset(address).is_some() && address & 3 == 0 {
            return self.write_mpcore_scu_register(address, value);
        }
        let bytes = value.to_le_bytes();
        if self.range_touches_mpcore_registers(address, bytes.len()) {
            for (index, byte) in bytes.into_iter().enumerate() {
                self.write8_with_cache_policy(address.wrapping_add(index as u32), byte, policy)?;
            }
            Ok(())
        } else {
            self.write_bytes_with_cache_policy(address, &bytes, policy)
        }
    }

    fn write16_with_cache_policy(
        &mut self,
        address: u32,
        value: u16,
        policy: CachePolicy,
    ) -> BusResult<()> {
        let bytes = value.to_le_bytes();
        if self.range_touches_mpcore_registers(address, bytes.len()) {
            for (index, byte) in bytes.into_iter().enumerate() {
                self.write8_with_cache_policy(address.wrapping_add(index as u32), byte, policy)?;
            }
            Ok(())
        } else {
            self.write_bytes_with_cache_policy(address, &bytes, policy)
        }
    }

    fn write16(&mut self, address: u32, value: u16) -> BusResult<()> {
        if self.mpcore_private_timer_register_offset(address).is_some()
            || self.mpcore_cpu_interface_register_offset(address).is_some()
        {
            return Err(Error::InvalidAccess { addr: address });
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            let bytes = value.to_le_bytes();
            self.write_mpcore_gic_distributor_byte(address, bytes[0])?;
            return self.write_mpcore_gic_distributor_byte(address.wrapping_add(1), bytes[1]);
        }
        if self.mpcore_scu_register_offset(address).is_some() {
            return Bus::write16_with_cache_policy(self, address, value, CachePolicy::NonCacheable);
        }
        self.write_bytes(address, &value.to_le_bytes())
    }

    fn write32(&mut self, address: u32, value: u32) -> BusResult<()> {
        if self.mpcore_private_timer_register_offset(address).is_some() {
            return self.write_mpcore_private_timer_register(address, value);
        }
        if self.mpcore_cpu_interface_register_offset(address).is_some() {
            return self.write_mpcore_cpu_interface_register(address, value);
        }
        if self
            .mpcore_gic_distributor_register_offset(address)
            .is_some()
        {
            return self.write_mpcore_gic_distributor_word(address, value);
        }
        if self.mpcore_scu_register_offset(address).is_some() && address & 3 == 0 {
            return self.write_mpcore_scu_register(address, value);
        }
        self.write_bytes(address, &value.to_le_bytes())
    }

    fn drain_write_buffer(&mut self) -> BusResult<()> {
        self.drain_arm11_write_buffer().map(|_| ())
    }

    fn data_memory_barrier(&mut self) -> BusResult<()> {
        self.drain_arm11_write_buffer().map(|_| ())
    }

    fn instruction_sync_barrier(&mut self) -> BusResult<()> {
        // Instruction fetch is not independently cached/pipelined in this interpreter.
        Ok(())
    }

    fn arm11_scu_enabled(&mut self) -> bool {
        self.mpcore_scu.as_ref().is_some_and(MpcoreScu::is_enabled)
    }

    fn set_arm11_smp_mode(&mut self, core_id: u32, enabled: bool) {
        let Some(slot) = usize::try_from(core_id)
            .ok()
            .and_then(|index| self.published_smp_mode.get_mut(index))
        else {
            return;
        };
        if *slot == Some(enabled) {
            return;
        }
        if let Some(scu) = &self.mpcore_scu {
            scu.set_cpu_smp_mode(core_id, enabled);
            *slot = Some(enabled);
        }
    }

    fn take_interrupt_state_touched(&mut self) -> bool {
        self.interrupt_state_touched.replace(false)
    }

    fn exclusive_read(&mut self, address: u32, size: usize) -> BusResult<(u64, u64)> {
        Self::validate_exclusive_access(address, size)?;
        let mut value = 0u64;
        for offset in 0..size {
            let current = address
                .checked_add(offset as u32)
                .ok_or(Error::InvalidAccess { addr: address })?;
            value |= u64::from(self.read8(current)?) << (offset * 8);
        }
        // Register the granule so later stores advance its generation.
        self.exclusive_versions
            .entry(address & !(crate::memory::EXCLUSIVE_RESERVATION_GRANULE - 1))
            .or_insert(0);
        Ok((value, self.exclusive_version(address)))
    }

    fn exclusive_write(
        &mut self,
        address: u32,
        size: usize,
        value: u64,
        reservation: Option<u64>,
    ) -> BusResult<bool> {
        Self::validate_exclusive_access(address, size)?;
        if !self.is_writable_range(address, size) {
            return Err(Error::InvalidAccess { addr: address });
        }
        if reservation != Some(self.exclusive_version(address)) {
            return Ok(false);
        }
        self.write_bytes(address, &value.to_le_bytes()[..size])?;
        Ok(true)
    }
}

fn make_bss_segment(
    data_code_set: CodeSetInfo,
    initialized_data_size: usize,
    total_data_memory_size: usize,
) -> std::result::Result<Arm11MemorySegment, Arm11ProcessLayoutError> {
    let bss_size = total_data_memory_size
        .checked_sub(initialized_data_size)
        .ok_or(Arm11ProcessLayoutError::InvalidCodeFileLayout)?;
    let mapped_size_bytes =
        u32::try_from(bss_size).map_err(|_| Arm11ProcessLayoutError::InvalidCodeFileLayout)?;

    if mapped_size_bytes == 0 {
        return Ok(Arm11MemorySegment {
            address: data_code_set.address,
            initialized_size_bytes: 0,
            mapped_size_bytes: 0,
            writable: true,
            executable: false,
        });
    }

    let initialized_data_size = u32::try_from(initialized_data_size)
        .map_err(|_| Arm11ProcessLayoutError::InvalidCodeFileLayout)?;
    let address = data_code_set
        .address
        .checked_add(initialized_data_size)
        .ok_or(Arm11ProcessLayoutError::SegmentRangeOverflow {
            segment: Arm11SegmentKind::Bss,
            address: data_code_set.address,
            mapped_size_bytes,
        })?;

    if u64::from(address) % PAGE_SIZE != 0 {
        return Err(Arm11ProcessLayoutError::SegmentAddressUnaligned {
            segment: Arm11SegmentKind::Bss,
            address,
        });
    }

    let end = u64::from(address) + u64::from(mapped_size_bytes);
    if end > ADDRESS_SPACE_END {
        return Err(Arm11ProcessLayoutError::SegmentRangeOverflow {
            segment: Arm11SegmentKind::Bss,
            address,
            mapped_size_bytes,
        });
    }

    Ok(Arm11MemorySegment {
        address,
        initialized_size_bytes: 0,
        mapped_size_bytes,
        writable: true,
        executable: false,
    })
}

fn make_stack_segment(
    requested_size_bytes: u32,
) -> std::result::Result<Arm11MemorySegment, Arm11ProcessLayoutError> {
    let aligned_size = u64::from(requested_size_bytes)
        .checked_add(PAGE_SIZE - 1)
        .ok_or(Arm11ProcessLayoutError::StackTooLarge {
            requested_size_bytes,
            maximum_size_bytes: MAIN_STACK_TOP,
        })?
        & !(PAGE_SIZE - 1);

    if aligned_size > u64::from(MAIN_STACK_TOP) {
        return Err(Arm11ProcessLayoutError::StackTooLarge {
            requested_size_bytes,
            maximum_size_bytes: MAIN_STACK_TOP,
        });
    }

    let mapped_size_bytes =
        u32::try_from(aligned_size).map_err(|_| Arm11ProcessLayoutError::StackTooLarge {
            requested_size_bytes,
            maximum_size_bytes: MAIN_STACK_TOP,
        })?;
    let address = MAIN_STACK_TOP - mapped_size_bytes;

    Ok(Arm11MemorySegment {
        address,
        initialized_size_bytes: 0,
        mapped_size_bytes,
        writable: true,
        executable: false,
    })
}

fn validate_image_segment(
    segment: Arm11SegmentKind,
    bytes: &[u8],
    expected_size: usize,
) -> std::result::Result<(), Arm11ProcessLayoutError> {
    if bytes.len() != expected_size {
        return Err(Arm11ProcessLayoutError::SegmentImageSizeMismatch {
            segment,
            expected_bytes: expected_size,
            actual_bytes: bytes.len(),
        });
    }
    Ok(())
}

fn make_segment(
    segment: Arm11SegmentKind,
    code_set: CodeSetInfo,
    mapped_size_bytes: usize,
    writable: bool,
    executable: bool,
) -> std::result::Result<Arm11MemorySegment, Arm11ProcessLayoutError> {
    let mapped_size_bytes = u32::try_from(mapped_size_bytes)
        .map_err(|_| Arm11ProcessLayoutError::InvalidCodeFileLayout)?;

    if mapped_size_bytes == 0 {
        return Ok(Arm11MemorySegment {
            address: code_set.address,
            initialized_size_bytes: code_set.size_bytes,
            mapped_size_bytes,
            writable,
            executable,
        });
    }

    if u64::from(code_set.address) % PAGE_SIZE != 0 {
        return Err(Arm11ProcessLayoutError::SegmentAddressUnaligned {
            segment,
            address: code_set.address,
        });
    }

    let end = u64::from(code_set.address) + u64::from(mapped_size_bytes);
    if end > ADDRESS_SPACE_END {
        return Err(Arm11ProcessLayoutError::SegmentRangeOverflow {
            segment,
            address: code_set.address,
            mapped_size_bytes,
        });
    }

    Ok(Arm11MemorySegment {
        address: code_set.address,
        initialized_size_bytes: code_set.size_bytes,
        mapped_size_bytes,
        writable,
        executable,
    })
}

fn validate_no_overlap(
    segments: [(Arm11SegmentKind, Arm11MemorySegment); 5],
) -> std::result::Result<(), Arm11ProcessLayoutError> {
    let mut non_empty = segments
        .into_iter()
        .filter(|(_, segment)| !segment.is_empty())
        .collect::<Vec<_>>();
    non_empty.sort_by_key(|(_, segment)| segment.address);

    for pair in non_empty.windows(2) {
        let (first_kind, first) = pair[0];
        let (second_kind, second) = pair[1];
        if first.end_address() > u64::from(second.address) {
            return Err(Arm11ProcessLayoutError::OverlappingSegments {
                first: first_kind,
                second: second_kind,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::ncsd::ncch::extended_header::NcchExtendedHeader;

    fn prepared_image() -> PreparedArm11Image {
        PreparedArm11Image {
            extended_header: NcchExtendedHeader {
                application_title: *b"ACERIS!!",
                flags: 0,
                remaster_version: 0,
                text_code_set: CodeSetInfo {
                    address: 0x0010_0000,
                    physical_size_pages: 2,
                    size_bytes: 0x1800,
                },
                stack_size_bytes: 0x4000,
                read_only_code_set: CodeSetInfo {
                    address: 0x0010_2000,
                    physical_size_pages: 1,
                    size_bytes: 0x800,
                },
                data_code_set: CodeSetInfo {
                    address: 0x0010_3000,
                    physical_size_pages: 3,
                    size_bytes: 0x1000,
                },
                bss_size_bytes: 0x2000,
                dependencies: [0; 48],
                save_data_size_bytes: 0,
                jump_id: 0,
                access_control_info: [0; 0x200],
            },
            text: vec![0x11; 0x2000],
            read_only: vec![0x22; 0x1000],
            data: vec![0x33; 0x1000],
        }
    }

    #[test]
    fn guest_arm11_code_configures_scu_and_enters_smp_mode() {
        let mut prepared = prepared_image();
        // STR r2,[r0] ; MCR p15,0,r1,c1,c0,1 (ACTLR)
        prepared.text[..4].copy_from_slice(&0xE580_2000u32.to_le_bytes());
        prepared.text[4..8].copy_from_slice(&0xEE01_1F30u32.to_le_bytes());
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared).unwrap();
        let scu = MpcoreScu::default();
        address_space.attach_mpcore_scu(scu.clone());

        let mut cpu = Arm11Core::new();
        address_space.initialize_core(&mut cpu);
        cpu.switch_mode(crate::cpu::Psr::SUPERVISOR_MODE).unwrap();
        cpu.regs[0] = crate::cpu::MPCORE_PRIVATE_MEMORY_BASE;
        cpu.regs[1] = 1 << 5;
        cpu.regs[2] = 0x0000_1FFF;

        assert!(!scu.is_enabled());
        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert!(scu.is_enabled());
        assert_eq!(scu.control_register(), 0x0000_1FFF);

        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert!(cpu.cp15().smp_mode_enabled());
        assert_eq!(
            address_space
                .read32(crate::cpu::MPCORE_PRIVATE_MEMORY_BASE + 4)
                .unwrap(),
            0b1_0001
        );
    }

    #[test]
    fn mpcore_scu_registers_are_mapped_on_the_arm11_process_bus() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let scu = MpcoreScu::default();
        address_space.attach_mpcore_scu(scu.clone());
        let base = crate::cpu::MPCORE_PRIVATE_MEMORY_BASE;

        assert_eq!(address_space.read32(base).unwrap(), 0x0000_1FFE);
        assert_eq!(address_space.read32(base + 4).unwrap(), 0b01);
        assert!(!Bus::arm11_scu_enabled(&mut address_space));

        // Guest software enables the SCU through its architected control bit.
        address_space.write32(base, 0x0000_1FFF).unwrap();
        assert!(Bus::arm11_scu_enabled(&mut address_space));
        assert_eq!(scu.control_register(), 0x0000_1FFF);

        // ACTLR.SMP is reflected in the read-only SCU configuration register.
        Bus::set_arm11_smp_mode(&mut address_space, 0, true);
        assert_eq!(address_space.read32(base + 4).unwrap(), 0b1_0001);
        address_space.write32(base + 4, 0).unwrap();
        assert_eq!(address_space.read32(base + 4).unwrap(), 0b1_0001);

        // The write-only invalidate command is accounted for, but no tag RAM
        // maintenance is claimed until the emulator has per-core L1 tag arrays.
        address_space.write32(base + 0x0C, u32::MAX).unwrap();
        assert_eq!(scu.invalidate_all_request_count(), 1);
        assert_eq!(address_space.read32(base + 0x0C).unwrap(), 0);

        assert!(matches!(
            address_space.read32(base + 0x10),
            Err(Error::InvalidAccess { .. })
        ));
    }

    #[test]
    fn process_memory_snapshot_round_trips_static_heap_and_shared_aliases() {
        let mut original = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let heap = APPLICATION_HEAP_BASE;
        original
            .allocate_application_heap_region(heap, 0x2000)
            .unwrap();
        original.protect_memory_region(heap, 0x2000, 1).unwrap();
        let mut heap_view = original
            .shared_memory_backing_for_heap_range(heap, 0x2000)
            .unwrap();
        heap_view.write_bytes(0x20, &[1, 2, 3, 4]);

        let shared_root = SharedMemoryBacking::zeroed(0x2000).unwrap();
        shared_root.write_bytes(0x100, &[9, 8, 7]);
        original
            .map_shared_memory_region(0x0600_0000, shared_root.clone(), 3)
            .unwrap();
        original
            .map_shared_memory_region(0x0600_2000, shared_root.clone(), 2)
            .unwrap();

        let mut catalog = SharedMemoryBackingCatalog::new();
        original.collect_shared_backings(&mut catalog).unwrap();
        let catalog_bytes = catalog.encode().unwrap();
        let restored_catalog = SharedMemoryBackingCatalog::decode(&catalog_bytes).unwrap();
        let encoded = original.encode_memory_state(&catalog).unwrap();

        let mut restored = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        restored
            .restore_memory_state(&encoded, &restored_catalog)
            .unwrap();
        assert_eq!(restored.dynamic_regions.len(), 1);
        assert_eq!(restored.shared_regions.len(), 2);
        assert!(restored.is_readable_range(heap, 0x2000));
        assert!(!restored.is_writable_range(heap, 0x2000));
        assert!(!restored.is_readable_range(0x0600_2000, 0x1000));
        assert!(restored.is_writable_range(0x0600_2000, 0x1000));

        let dynamic = restored.dynamic_regions[0].backing.backing_ref();
        let mut heap_bytes = [0u8; 4];
        dynamic.read_bytes(0x20, &mut heap_bytes);
        assert_eq!(heap_bytes, [1, 2, 3, 4]);

        let first = &restored.shared_regions[0].backing;
        let second = &restored.shared_regions[1].backing;
        assert!(first.shares_backing_with(second));
        let mut shared_bytes = [0u8; 3];
        second.read_bytes(0x100, &mut shared_bytes);
        assert_eq!(shared_bytes, [9, 8, 7]);

        let section = original.encode_save_state_section(&catalog).unwrap();
        assert_eq!(section.id(), crate::save_state::section_id::PROCESS_MEMORY);
    }

    #[test]
    fn malformed_process_memory_snapshot_does_not_replace_live_regions() {
        let mut original = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        original
            .allocate_application_heap_region(APPLICATION_HEAP_BASE, 0x2000)
            .unwrap();
        let mut catalog = SharedMemoryBackingCatalog::new();
        original.collect_shared_backings(&mut catalog).unwrap();
        let before = original.encode_memory_state(&catalog).unwrap();

        let mut truncated = before.clone();
        truncated.pop();
        assert!(original.restore_memory_state(&truncated, &catalog).is_err());
        assert_eq!(original.encode_memory_state(&catalog).unwrap(), before);

        let mut bad_count = before.clone();
        let mut count_offset = 12usize;
        for _ in 0..5 {
            let length = u32::from_le_bytes(
                bad_count[count_offset..count_offset + 4]
                    .try_into()
                    .unwrap(),
            ) as usize;
            count_offset += 4 + length;
        }
        count_offset += 4; // fast-memory, interrupt marker and two SMP options
        bad_count[count_offset..count_offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(original.restore_memory_state(&bad_count, &catalog).is_err());
        assert_eq!(original.encode_memory_state(&catalog).unwrap(), before);

        let mut trailing = before.clone();
        trailing.push(0);
        assert!(original.restore_memory_state(&trailing, &catalog).is_err());
        assert_eq!(original.encode_memory_state(&catalog).unwrap(), before);
    }

    #[test]
    fn shared_memory_blocks_alias_between_process_address_spaces() {
        let backing = SharedMemoryBacking::zeroed(0x1000).unwrap();
        let mut first = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut second = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        first
            .map_shared_memory_region(0x0800_0000, backing.clone(), 3)
            .unwrap();
        second
            .map_shared_memory_region(0x0900_0000, backing.clone(), 1)
            .unwrap();

        first.write32(0x0800_0000, 0x1234_5678).unwrap();
        assert_eq!(first.read32(0x0800_0000).unwrap(), 0x1234_5678);
        // The second process has an independent D-cache and cannot see the
        // first process' dirty line until it is explicitly cleaned and drained.
        assert_eq!(second.read32(0x0900_0000).unwrap(), 0);
        first.flush_data_cache_range(0x0800_0000, 4).unwrap();
        first.drain_arm11_write_buffer().unwrap();
        // The peer's clean line remains stale until that process invalidates
        // its own cache entry using its own virtual mapping.
        assert_eq!(second.read32(0x0900_0000).unwrap(), 0);
        assert_eq!(
            second.invalidate_data_cache_range(0x0900_0000, 4).unwrap(),
            1
        );
        assert_eq!(second.read32(0x0900_0000).unwrap(), 0x1234_5678);
        assert!(!second.is_writable_range(0x0900_0000, 4));
        assert!(second.write32(0x0900_0000, 0xFFFF_FFFF).is_err());
        assert_eq!(first.read32(0x0800_0000).unwrap(), 0x1234_5678);

        let region = second.query_memory(0x0900_0000);
        assert_eq!(region.base_address, 0x0900_0000);
        assert_eq!(region.size_bytes, 0x1000);
        assert_eq!(region.permissions, 1);
        assert_eq!(
            second.unmap_shared_memory_region(0x0900_0000, 0x1000),
            Ok(backing.clone())
        );
        assert!(second.read32(0x0900_0000).is_err());
        assert_eq!(first.read32(0x0800_0000).unwrap(), 0x1234_5678);
        assert!(first.shared_mapping_matches(0x0800_0000, &backing));
    }

    #[test]
    fn shared_memory_cache_uses_one_tag_for_aliases_and_invalidates_other_processes() {
        let backing = SharedMemoryBacking::zeroed(0x1000).unwrap();
        let mut first = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut second = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        first
            .map_shared_memory_region(0x0800_0000, backing.clone(), 3)
            .unwrap();
        first
            .map_shared_memory_region(0x0A00_0000, backing.clone(), 3)
            .unwrap();
        second
            .map_shared_memory_region(0x0900_0000, backing.clone(), 3)
            .unwrap();

        first.write32(0x0800_0000, 0xA1B2_C3D4).unwrap();
        // Two virtual aliases in one process resolve to the same backing line.
        assert_eq!(first.read32(0x0A00_0000).unwrap(), 0xA1B2_C3D4);
        assert_eq!(first.data_cache_line_count(), 1);

        // Prime a clean stale line in the other process.
        assert_eq!(second.read32(0x0900_0000).unwrap(), 0);
        assert_eq!(second.data_cache_line_count(), 1);

        first.flush_data_cache_range(0x0800_0000, 4).unwrap();
        first.data_memory_barrier().unwrap();

        // Publication updates the shared backing, but the peer owns a separate
        // cache and must invalidate it explicitly before observing the update.
        assert_eq!(second.read32(0x0900_0000).unwrap(), 0);
        assert_eq!(
            second.invalidate_data_cache_range(0x0900_0000, 4).unwrap(),
            1
        );
        assert_eq!(second.read32(0x0900_0000).unwrap(), 0xA1B2_C3D4);
        assert_eq!(second.data_cache_line_count(), 1);
    }

    #[test]
    fn shared_memory_subviews_publish_multiple_lines_and_invalidate_through_peer_alias() {
        let backing = SharedMemoryBacking::zeroed(0x2000).unwrap();
        let second_page = backing.view(0x1000, 0x1000).unwrap();
        let mut publisher = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut consumer = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        publisher
            .map_shared_memory_region(0x0800_0000, backing, 3)
            .unwrap();
        consumer
            .map_shared_memory_region(0x0900_0000, second_page.clone(), 3)
            .unwrap();
        consumer
            .map_shared_memory_region(0x0B00_0000, second_page, 3)
            .unwrap();

        // The consumer maps only the backing's second page. The two bytes
        // straddle a 32-byte cache-line boundary in that shared subview.
        assert_eq!(consumer.read8(0x0900_001F).unwrap(), 0);
        assert_eq!(consumer.read8(0x0900_0020).unwrap(), 0);
        assert_eq!(consumer.data_cache_line_count(), 2);

        publisher.write8(0x0800_101F, 0xA5).unwrap();
        publisher.write8(0x0800_1020, 0x5A).unwrap();
        assert_eq!(publisher.store_data_cache_range(0x0800_101F, 2).unwrap(), 2);
        assert_eq!(publisher.data_cache_line_count(), 2);
        assert_eq!(publisher.pending_write_buffer_entries(), 2);

        // The producer's DMB completes both line writebacks, but does not
        // snoop or discard the consumer's private clean cache lines.
        publisher.data_memory_barrier().unwrap();
        assert_eq!(publisher.pending_write_buffer_entries(), 0);
        assert_eq!(consumer.read8(0x0900_001F).unwrap(), 0);
        assert_eq!(consumer.read8(0x0900_0020).unwrap(), 0);

        // Invalidation through a second virtual alias must evict the same
        // backing-identified lines used by reads through the first alias.
        assert_eq!(
            consumer
                .invalidate_data_cache_range(0x0B00_001F, 2)
                .unwrap(),
            2
        );
        assert_eq!(consumer.read8(0x0900_001F).unwrap(), 0xA5);
        assert_eq!(consumer.read8(0x0900_0020).unwrap(), 0x5A);
        assert_eq!(consumer.data_cache_line_count(), 2);
    }

    #[test]
    fn shared_memory_writeback_invalidates_peer_exclusive_reservation_across_subviews() {
        let backing = SharedMemoryBacking::zeroed(0x2000).unwrap();
        let second_page = backing.view(0x1000, 0x1000).unwrap();
        let mut publisher = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut consumer = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        publisher
            .map_shared_memory_region(0x0800_0000, backing, 3)
            .unwrap();
        consumer
            .map_shared_memory_region(0x0900_0000, second_page, 3)
            .unwrap();

        // The consumer reserves a word through a view whose offset differs
        // from the publisher's full-backing mapping.
        let (_, reservation) = consumer.exclusive_read(0x0900_0000, 4).unwrap();
        publisher.write32(0x0800_1000, 0xCAFE_BABE).unwrap();
        publisher.flush_data_cache_range(0x0800_1000, 4).unwrap();
        assert_eq!(publisher.pending_write_buffer_entries(), 1);

        // Publishing the CPU cache line updates the shared exclusive-monitor
        // generation even though the consumer owns a distinct D-cache.
        publisher.data_memory_barrier().unwrap();
        assert!(
            !consumer
                .exclusive_write(0x0900_0000, 4, 0xDEAD_BEEF, Some(reservation))
                .unwrap()
        );

        // Reservation invalidation does not implicitly invalidate the peer's
        // ordinary data-cache copy; the process must perform both operations.
        assert_eq!(consumer.read32(0x0900_0000).unwrap(), 0);
        assert_eq!(
            consumer
                .invalidate_data_cache_range(0x0900_0000, 4)
                .unwrap(),
            1
        );
        assert_eq!(consumer.read32(0x0900_0000).unwrap(), 0xCAFE_BABE);
    }

    #[test]
    fn dynamic_heap_backing_is_shared_with_mapped_views_and_cache_generations() {
        let mut owner = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut peer = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let heap = owner
            .allocate_application_heap_region(0x0800_0000, 0x1000)
            .unwrap();
        let backing = owner
            .shared_memory_backing_for_heap_range(heap, 0x1000)
            .unwrap();
        peer.map_shared_memory_region(0x0900_0000, backing, 3)
            .unwrap();

        owner.write32(heap + 0x80, 0x5566_7788).unwrap();
        assert_eq!(peer.read32(0x0900_0080).unwrap(), 0);
        owner.flush_data_cache_range(heap + 0x80, 4).unwrap();
        owner.data_memory_barrier().unwrap();
        assert_eq!(peer.read32(0x0900_0080).unwrap(), 0);
        peer.invalidate_data_cache_range(0x0900_0080, 4).unwrap();
        assert_eq!(peer.read32(0x0900_0080).unwrap(), 0x5566_7788);
    }

    #[test]
    fn static_segments_use_backing_cache_and_publish_only_after_clean() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let data_address = address_space.layout.data.address;

        address_space.write32(data_address, 0xDEAD_BEEF).unwrap();
        assert_eq!(address_space.read32(data_address).unwrap(), 0xDEAD_BEEF);
        assert_eq!(
            &address_space.segment_bytes(Arm11SegmentKind::Data)[..4],
            &[0x33; 4]
        );
        assert_eq!(address_space.data_cache_line_count(), 1);

        address_space
            .flush_data_cache_range(data_address, 4)
            .unwrap();
        address_space.data_memory_barrier().unwrap();
        assert_eq!(
            &address_space.segment_bytes(Arm11SegmentKind::Data)[..4],
            &0xDEAD_BEEFu32.to_le_bytes()
        );
        assert_eq!(address_space.data_cache_line_count(), 0);
    }

    #[test]
    fn shared_memory_mapping_rejects_invalid_ranges_permissions_and_overlaps() {
        let backing = SharedMemoryBacking::zeroed(0x1000).unwrap();
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        assert_eq!(
            address_space.map_shared_memory_region(0x0800_0001, backing.clone(), 3),
            Err(Arm11DynamicMemoryError::MisalignedAddress(0x0800_0001))
        );
        assert_eq!(
            address_space.map_shared_memory_region(0x0800_0000, backing.clone(), 4),
            Err(Arm11DynamicMemoryError::InvalidPermissions(4))
        );
        address_space
            .map_shared_memory_region(0x0800_0000, backing.clone(), 2)
            .unwrap();
        assert!(address_space.is_writable_range(0x0800_0000, 0x1000));
        assert!(!address_space.is_readable_range(0x0800_0000, 0x1000));
        address_space
            .unmap_shared_memory_region(0x0800_0000, 0x1000)
            .unwrap();
        let oversized_backing = SharedMemoryBacking::zeroed(0x2000).unwrap();
        assert_eq!(
            address_space.map_shared_memory_region(0xFFFF_F000, oversized_backing, 3),
            Err(Arm11DynamicMemoryError::AddressRangeOverflow {
                address: 0xFFFF_F000,
                size: 0x2000,
            })
        );
        address_space
            .map_shared_memory_region(0x0800_0000, backing.clone(), 3)
            .unwrap();
        assert_eq!(
            address_space.map_shared_memory_region(0x0800_0000, backing, 3),
            Err(Arm11DynamicMemoryError::OverlappingMapping)
        );
    }

    #[test]
    fn arm11_core_fetches_and_executes_from_the_prepared_text_mapping() {
        let mut prepared = prepared_image();
        prepared.text[..4].copy_from_slice(&0xE3A0_0007u32.to_le_bytes());

        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared).unwrap();
        let mut cpu = crate::cpu::Arm11Core::new();
        address_space.initialize_core(&mut cpu);
        assert_eq!(cpu.regs[SP], MAIN_STACK_TOP);

        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert_eq!(cpu.regs[0], 7);
        assert_eq!(cpu.pc(), 0x0010_0004);
    }

    #[test]
    fn arm11_exclusive_instructions_use_process_memory_monitor_end_to_end() {
        // LDREX r0,[r1] ; STREX r2,r3,[r1] ; LDREX r0,[r1] ;
        // STR r4,[r1,#4] ; STREX r2,r3,[r1].
        let mut prepared = prepared_image();
        let program: [u32; 5] = [
            0xE191_0F9F,
            0xE181_2F93,
            0xE191_0F9F,
            0xE581_4004,
            0xE181_2F93,
        ];
        for (index, instruction) in program.into_iter().enumerate() {
            let start = index * 4;
            prepared.text[start..start + 4].copy_from_slice(&instruction.to_le_bytes());
        }

        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared).unwrap();
        let data_address = address_space.layout.data.address;
        address_space.write32(data_address, 0x1122_3344).unwrap();
        address_space.write32(data_address + 4, 0).unwrap();

        let mut cpu = Arm11Core::new();
        address_space.initialize_core(&mut cpu);
        cpu.regs[1] = data_address;
        cpu.regs[3] = 0xAABB_CCDD;
        cpu.regs[4] = 0x5566_7788;

        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert_eq!(cpu.regs[0], 0x1122_3344);
        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert_eq!(cpu.regs[2], 0);
        assert_eq!(address_space.read32(data_address).unwrap(), 0xAABB_CCDD);

        cpu.regs[3] = 0xDEAD_BEEF;
        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert_eq!(cpu.regs[0], 0xAABB_CCDD);
        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );

        assert_eq!(cpu.regs[2], 1);
        assert_eq!(address_space.read32(data_address).unwrap(), 0xAABB_CCDD);
        assert_eq!(address_space.read32(data_address + 4).unwrap(), 0x5566_7788);
    }

    #[test]
    fn address_space_enforces_code_permissions_and_allows_data_writes() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let text_address = address_space.layout.text.address;
        let read_only_address = address_space.layout.read_only.address;
        let data_address = address_space.layout.data.address;

        assert!(matches!(
            address_space.write8(text_address, 0),
            Err(crate::Error::InvalidAccess { .. })
        ));
        assert!(matches!(
            address_space.write8(read_only_address, 0),
            Err(crate::Error::InvalidAccess { .. })
        ));

        address_space.write32(data_address, 0xDEAD_BEEF).unwrap();
        assert_eq!(address_space.read32(data_address).unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn bss_memory_is_zero_initialized_and_writable() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let bss_address = address_space.layout.bss.address;
        let bss_size = address_space.layout.bss.mapped_size_bytes as usize;

        assert!(
            address_space
                .segment_bytes(Arm11SegmentKind::Bss)
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(bss_size, 0x2000);

        address_space.write32(bss_address, 0x1234_5678).unwrap();
        assert_eq!(address_space.read32(bss_address).unwrap(), 0x1234_5678);
    }

    #[test]
    fn initialized_data_and_bss_form_one_contiguous_writable_region() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let data_end = address_space.layout.data.end_address() as u32;

        assert_eq!(address_space.layout.bss.address, data_end);
        let write_address = data_end - 2;
        address_space.write32(write_address, 0xAABB_CCDD).unwrap();
        assert_eq!(address_space.read32(write_address).unwrap(), 0xAABB_CCDD);
    }

    #[test]
    fn rejects_bss_mappings_above_the_bootstrap_memory_limit() {
        let mut prepared = prepared_image();
        prepared.extended_header.bss_size_bytes = 0x1000_1000;
        prepared.extended_header.data_code_set.physical_size_pages = 0x10002;

        assert!(matches!(
            prepared.process_layout(),
            Err(Arm11ProcessLayoutError::BssTooLarge { .. })
        ));
    }

    #[test]
    fn wide_writes_are_atomic_when_the_range_crosses_into_read_only_memory() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let data_start = address_space.layout.data.address;
        let original = address_space.read8(data_start).unwrap();

        assert!(address_space.write32(data_start - 2, 0xAABB_CCDD).is_err());
        assert_eq!(address_space.read8(data_start).unwrap(), original);

        // Larger block writes take the heap-backed fallback and must retain
        // the same validate-before-mutate behavior.
        assert!(
            address_space
                .write_bytes(data_start - 1, &[0xA5; INLINE_WRITE_LOCATION_CAPACITY + 1])
                .is_err()
        );
        assert_eq!(address_space.read8(data_start).unwrap(), original);
    }

    #[test]
    fn validates_executable_entrypoints_and_thumb_function_pointers() {
        let address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        assert!(address_space.is_executable_address(0x0010_0000));
        assert!(address_space.is_executable_address(0x0010_0001));
        assert!(!address_space.is_executable_address(0x0010_2000));
        assert!(!address_space.is_executable_address(0x000F_FFFF));
    }

    #[test]
    fn validates_writable_ranges_without_crossing_unmapped_or_read_only_memory() {
        let address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        assert!(address_space.is_writable_range(0x0010_3000, 0x1000));
        assert!(address_space.is_writable_range(0x0010_3FFC, 8));
        assert!(!address_space.is_writable_range(0x0010_2FFC, 8));
        assert!(address_space.is_writable_range(0x0010_4000, 1));
        assert!(!address_space.is_writable_range(0x0010_6000, 1));
        assert!(!address_space.is_writable_range(0x0010_3000, 0));
    }

    #[test]
    fn gpu_backing_identity_is_cached_when_attached_and_used_for_physical_tags() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        let backing_id = memory.backing_id();

        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        assert_eq!(address_space.gpu_memory_backing_id, Some(backing_id));
        let location = address_space.locate(physical_base).unwrap();
        let (key, offset) = address_space.cache_key_for(location).unwrap();
        assert_eq!(key.backing_id, backing_id);
        assert_eq!(key.line_offset, u64::from(physical_base));
        assert_eq!(offset, 0);

        // Reattaching a different backing must update the cached identity,
        // otherwise new physical accesses could alias old D-cache tags.
        let replacement = GpuPhysicalMemory::new();
        let replacement_id = replacement.backing_id();
        address_space.attach_gpu_memory(replacement);
        assert_ne!(replacement_id, backing_id);
        assert_eq!(address_space.gpu_memory_backing_id, Some(replacement_id));
    }

    #[test]
    fn arm11_data_cache_writeback_and_invalidation_cohere_with_gpu_memory() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        // CPU stores remain dirty in its private cache until a clean/flush.
        address_space.write32(virtual_base, 0x1122_3344).unwrap();
        assert_eq!(address_space.read32(virtual_base).unwrap(), 0x1122_3344);
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0);
        assert_eq!(address_space.data_cache_line_count(), 1);

        // StoreProcessDataCache enqueues a writeback but retains a clean resident line.
        assert_eq!(
            address_space
                .store_data_cache_range(virtual_base, 4)
                .unwrap(),
            1
        );
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0);
        assert_eq!(address_space.read32(virtual_base).unwrap(), 0x1122_3344);
        assert_eq!(
            address_space
                .read8_with_cache_policy(virtual_base, CachePolicy::NormalNonCacheable)
                .unwrap(),
            0x44
        );
        address_space.data_memory_barrier().unwrap();
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0x1122_3344);
        assert_eq!(address_space.pending_write_buffer_entries(), 0);
        assert_eq!(address_space.data_cache_line_count(), 1);

        // A GPU/DMA write changes physical memory, while the CPU keeps its cached view.
        memory.write_u32(physical_base, 0xAABB_CCDD).unwrap();
        assert_eq!(address_space.read32(virtual_base).unwrap(), 0x1122_3344);

        // Invalidate discards the stale clean line; the next CPU read refills from GPU memory.
        assert_eq!(
            address_space
                .invalidate_data_cache_range(virtual_base, 4)
                .unwrap(),
            1
        );
        assert_eq!(address_space.read32(virtual_base).unwrap(), 0xAABB_CCDD);

        // FlushProcessDataCache writes back dirty data and invalidates the line.
        address_space.write32(virtual_base, 0x5566_7788).unwrap();
        assert_eq!(
            address_space
                .flush_data_cache_range(virtual_base, 4)
                .unwrap(),
            1
        );
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0xAABB_CCDD);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        assert_eq!(address_space.data_cache_line_count(), 0);
        assert_eq!(
            address_space
                .read8_with_cache_policy(virtual_base, CachePolicy::NormalNonCacheable)
                .unwrap(),
            0x88
        );
        address_space.drain_arm11_write_buffer().unwrap();
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0x5566_7788);
    }

    #[test]
    fn arm11_normal_noncacheable_stores_are_buffered_but_device_stores_are_synchronous() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        // Four byte stores from the default wide-access helper coalesce into one
        // pending transaction without allocating a D-cache line.
        address_space
            .write32_with_cache_policy(virtual_base, 0x1122_3344, CachePolicy::NormalNonCacheable)
            .unwrap();
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        assert_eq!(address_space.data_cache_line_count(), 0);
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0);

        // A CPU read forwards its own buffered Normal-memory stores.
        assert_eq!(
            address_space
                .read32_with_cache_policy(virtual_base, CachePolicy::NormalNonCacheable)
                .unwrap(),
            0x1122_3344
        );
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0);

        // DMB completes the buffered Normal-memory transaction for other agents.
        address_space.data_memory_barrier().unwrap();
        assert_eq!(address_space.pending_write_buffer_entries(), 0);
        assert_eq!(memory.read_u32(physical_base).unwrap(), 0x1122_3344);

        // Device/Strongly Ordered stores remain synchronous and do not enter
        // the Normal-memory write buffer.
        address_space
            .write32_with_cache_policy(virtual_base + 4, 0xAABB_CCDD, CachePolicy::NonCacheable)
            .unwrap();
        assert_eq!(memory.read_u32(physical_base + 4).unwrap(), 0xAABB_CCDD);
        assert_eq!(address_space.pending_write_buffer_entries(), 0);
    }

    #[test]
    fn arm11_write_buffer_is_bounded_fifo_and_dmb_completes_all_writebacks() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        // Dirty nine distinct lines and clean them in ascending address order.
        for line in 0..(ARM11_WRITE_BUFFER_CAPACITY + 1) {
            address_space
                .write8(
                    virtual_base + (line * ARM11_DATA_CACHE_LINE_SIZE) as u32,
                    (line + 1) as u8,
                )
                .unwrap();
        }
        assert_eq!(
            address_space
                .flush_data_cache_range(
                    virtual_base,
                    ((ARM11_WRITE_BUFFER_CAPACITY + 1) * ARM11_DATA_CACHE_LINE_SIZE) as u32,
                )
                .unwrap(),
            ARM11_WRITE_BUFFER_CAPACITY + 1
        );

        // The ninth enqueue applies back-pressure by completing the oldest line.
        assert_eq!(
            address_space.pending_write_buffer_entries(),
            ARM11_WRITE_BUFFER_CAPACITY
        );
        let mut first = [0u8; 1];
        let mut second = [0u8; 1];
        memory.read(physical_base, &mut first).unwrap();
        memory
            .read(
                physical_base + ARM11_DATA_CACHE_LINE_SIZE as u32,
                &mut second,
            )
            .unwrap();
        assert_eq!(first, [1]);
        assert_eq!(second, [0]);

        // A DMB makes all older queued writebacks visible to physical agents.
        address_space.data_memory_barrier().unwrap();
        assert_eq!(address_space.pending_write_buffer_entries(), 0);
        for line in 0..(ARM11_WRITE_BUFFER_CAPACITY + 1) {
            let mut actual = [0u8; 1];
            memory
                .read(
                    physical_base + (line * ARM11_DATA_CACHE_LINE_SIZE) as u32,
                    &mut actual,
                )
                .unwrap();
            assert_eq!(actual, [(line + 1) as u8]);
        }
    }

    #[test]
    fn arm11_data_cache_obeys_noncacheable_and_write_through_mmu_policies() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        memory.write(physical_base, &[0x11]).unwrap();
        assert_eq!(
            address_space
                .read8_with_cache_policy(virtual_base, CachePolicy::WriteBack)
                .unwrap(),
            0x11
        );
        assert_eq!(address_space.data_cache_line_count(), 1);

        // Device/Strongly Ordered aliases bypass the resident D-cache line.
        // The cached Normal-memory alias remains stale until invalidated.
        memory.write(physical_base, &[0x22]).unwrap();
        assert_eq!(
            address_space
                .read8_with_cache_policy(physical_base, CachePolicy::NonCacheable)
                .unwrap(),
            0x22
        );
        assert_eq!(
            address_space
                .read8_with_cache_policy(virtual_base, CachePolicy::WriteBack)
                .unwrap(),
            0x11
        );

        address_space
            .invalidate_data_cache_range(virtual_base, 1)
            .unwrap();
        assert_eq!(address_space.data_cache_line_count(), 0);

        // Write-through stores are buffered Normal-memory transactions and do
        // not allocate a line on a write miss.
        address_space
            .write8_with_cache_policy(virtual_base, 0x33, CachePolicy::WriteThrough)
            .unwrap();
        let mut backing = [0u8; 1];
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x22]);
        assert_eq!(address_space.data_cache_line_count(), 0);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);

        // A write-through read may allocate a clean line and forwards pending
        // stores; subsequent writes update that resident line and queue order.
        assert_eq!(
            address_space
                .read8_with_cache_policy(physical_base, CachePolicy::WriteThrough)
                .unwrap(),
            0x33
        );
        address_space
            .write8_with_cache_policy(virtual_base, 0x44, CachePolicy::WriteThrough)
            .unwrap();
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x22]);
        assert_eq!(
            address_space
                .read8_with_cache_policy(physical_base, CachePolicy::WriteThrough)
                .unwrap(),
            0x44
        );
        assert_eq!(address_space.data_cache_line_count(), 1);
        assert_eq!(address_space.pending_write_buffer_entries(), 2);
        address_space.data_memory_barrier().unwrap();
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x44]);
        assert_eq!(address_space.pending_write_buffer_entries(), 0);

        // Write-back/no-write-allocate queues a store on a miss without
        // allocating a line. A subsequent read forwards it into the cache.
        address_space
            .invalidate_data_cache_range(virtual_base, 1)
            .unwrap();
        address_space
            .write8_with_cache_policy(virtual_base, 0x55, CachePolicy::WriteBackNoAllocate)
            .unwrap();
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x44]);
        assert_eq!(address_space.data_cache_line_count(), 0);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);

        assert_eq!(
            address_space
                .read8_with_cache_policy(virtual_base, CachePolicy::WriteBackNoAllocate)
                .unwrap(),
            0x55
        );
        address_space
            .write8_with_cache_policy(virtual_base, 0x66, CachePolicy::WriteBackNoAllocate)
            .unwrap();
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x44]);
        assert_eq!(
            address_space
                .store_data_cache_range(virtual_base, 1)
                .unwrap(),
            1
        );
        assert_eq!(address_space.pending_write_buffer_entries(), 2);
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x44]);
        address_space.drain_arm11_write_buffer().unwrap();
        memory.read(physical_base, &mut backing).unwrap();
        assert_eq!(backing, [0x66]);
    }

    #[test]
    fn arm11_cache_eviction_keeps_dirty_victim_when_writeback_drain_fails() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let backing = SharedMemoryBacking::zeroed(0x2000).unwrap();

        // Poison the shared backing lock so the oldest pending write cannot
        // complete. This makes the bounded write-buffer back-pressure fail.
        let poison = backing.clone();
        assert!(
            std::thread::spawn(move || {
                let _guard = poison.bytes.lock().unwrap();
                panic!("intentional backing lock poison for writeback regression");
            })
            .join()
            .is_err()
        );

        let victim_key = DataCacheKey {
            backing_id: backing.backing_id(),
            line_offset: 0,
        };
        let victim_target = DataCacheTarget::Backing {
            backing: backing.clone(),
            offset: 0,
        };
        let set_index = Arm11CodeAddressSpace::cache_set_index(victim_key);
        address_space.data_cache.insert(
            victim_key,
            Arm11DataCacheLine {
                bytes: [0xA5; ARM11_DATA_CACHE_LINE_SIZE],
                dirty: true,
                target: victim_target.clone(),
            },
        );
        address_space.data_cache_lru[set_index].push_back(victim_key);

        // Fill the remaining three ways with clean lines that collide with
        // the dirty LRU victim. The replacement must now evict that victim.
        for way in 1..ARM11_DATA_CACHE_WAYS {
            let line_offset = way * ARM11_DATA_CACHE_SET_COUNT * ARM11_DATA_CACHE_LINE_SIZE;
            let key = DataCacheKey {
                backing_id: victim_key.backing_id,
                line_offset: line_offset as u64,
            };
            address_space.data_cache.insert(
                key,
                Arm11DataCacheLine {
                    bytes: [0; ARM11_DATA_CACHE_LINE_SIZE],
                    dirty: false,
                    target: DataCacheTarget::Backing {
                        backing: backing.clone(),
                        offset: line_offset,
                    },
                },
            );
            address_space.data_cache_lru[set_index].push_back(key);
        }

        for value in 0..ARM11_WRITE_BUFFER_CAPACITY {
            let mut bytes = [0; ARM11_DATA_CACHE_LINE_SIZE];
            bytes[0] = value as u8;
            address_space.write_buffer.push_back(Arm11WriteBufferEntry {
                key: victim_key,
                target: victim_target.clone(),
                start: 0,
                bytes,
                length: 1,
            });
        }

        // The replacement is one full set-span beyond all four resident
        // lines, so it collides with the dirty victim's set.
        let replacement_key = DataCacheKey {
            backing_id: victim_key.backing_id,
            line_offset: ARM11_DATA_CACHE_WAYS as u64
                * ARM11_DATA_CACHE_SET_COUNT as u64
                * ARM11_DATA_CACHE_LINE_SIZE as u64,
        };
        let replacement_target = DataCacheTarget::Backing {
            backing,
            offset: ARM11_DATA_CACHE_WAYS * ARM11_DATA_CACHE_SET_COUNT * ARM11_DATA_CACHE_LINE_SIZE,
        };

        assert!(matches!(
            address_space.insert_data_cache_line(
                replacement_key,
                Arm11DataCacheLine {
                    bytes: [0; ARM11_DATA_CACHE_LINE_SIZE],
                    dirty: false,
                    target: replacement_target,
                }
            ),
            Err(Error::InvalidAccess { .. })
        ));
        assert!(address_space.data_cache.contains_key(&victim_key));
        assert!(address_space.data_cache[&victim_key].dirty);
        assert_eq!(
            address_space.data_cache_lru[set_index].front(),
            Some(&victim_key)
        );
        assert_eq!(
            address_space.write_buffer.len(),
            ARM11_WRITE_BUFFER_CAPACITY
        );
    }

    #[test]
    fn arm11_data_cache_uses_four_ways_and_writes_back_lru_victims() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x5000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x5000)
            .unwrap();

        // A 4-way, 128-set cache maps lines separated by 4 KiB to the same set.
        for way in 0..ARM11_DATA_CACHE_WAYS {
            let offset = (way * ARM11_DATA_CACHE_SET_COUNT * ARM11_DATA_CACHE_LINE_SIZE) as u32;
            address_space
                .write8(virtual_base + offset, 0xA0 + way as u8)
                .unwrap();
        }
        assert_eq!(address_space.data_cache_line_count(), ARM11_DATA_CACHE_WAYS);

        // Touch way 0 to make way 1 the least-recently-used dirty victim.
        assert_eq!(address_space.read8(virtual_base).unwrap(), 0xA0);
        let fifth_offset = (ARM11_DATA_CACHE_WAYS
            * ARM11_DATA_CACHE_SET_COUNT
            * ARM11_DATA_CACHE_LINE_SIZE) as u32;
        address_space
            .write8(virtual_base + fifth_offset, 0xA4)
            .unwrap();

        assert_eq!(address_space.data_cache_line_count(), ARM11_DATA_CACHE_WAYS);
        // Way 1 was evicted into the write buffer; the device-visible backing is still old.
        let way_one_physical =
            physical_base + (ARM11_DATA_CACHE_SET_COUNT * ARM11_DATA_CACHE_LINE_SIZE) as u32;
        let mut evicted_value = [0u8; 1];
        memory.read(way_one_physical, &mut evicted_value).unwrap();
        assert_eq!(evicted_value, [0]);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        address_space.drain_arm11_write_buffer().unwrap();
        memory.read(way_one_physical, &mut evicted_value).unwrap();
        assert_eq!(evicted_value, [0xA1]);

        // Way 0 remained resident and dirty, so backing memory is still unchanged.
        let mut resident_value = [0u8; 1];
        memory.read(physical_base, &mut resident_value).unwrap();
        assert_eq!(resident_value, [0]);
        assert_eq!(address_space.read8(virtual_base).unwrap(), 0xA0);
    }

    #[test]
    fn arm11_cached_hit_fast_paths_match_the_byte_path_and_keep_lru_order() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x6000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x6000)
            .unwrap();

        // A wide read miss fills one line and returns the requested scalar.
        memory
            .write(physical_base + 0x80, &0x8877_6655u32.to_le_bytes())
            .unwrap();
        assert_eq!(
            address_space.read32(virtual_base + 0x80).unwrap(),
            0x8877_6655
        );
        assert_eq!(address_space.data_cache_line_count(), 1);
        assert_eq!(address_space.read16(virtual_base + 0x82).unwrap(), 0x8877);

        // The cache-policy halfword read also fills and returns a line miss.
        memory.write(physical_base + 0xA0, &[0x34, 0x12]).unwrap();
        assert_eq!(
            address_space
                .read16_with_cache_policy(virtual_base + 0xA0, CachePolicy::WriteBack)
                .unwrap(),
            0x1234
        );

        // A miss spanning two cache lines must retain the byte-wise fallback.
        memory
            .write(physical_base + 30, &[0xAA, 0xBB, 0xCC, 0xDD])
            .unwrap();
        assert_eq!(
            address_space.read32(virtual_base + 30).unwrap(),
            0xDDCC_BBAA
        );

        // MMU-policy scalar stores use the bulk validator and fill one line.
        let policy_store = virtual_base + 0xC0;
        address_space
            .write32_with_cache_policy(policy_store, 0xA1B2_C3D4, CachePolicy::WriteBack)
            .unwrap();
        let mut policy_backing = [0xFFu8; 4];
        memory
            .read(physical_base + 0xC0, &mut policy_backing)
            .unwrap();
        assert_eq!(policy_backing, [0; 4]);
        assert_eq!(
            address_space
                .read32_with_cache_policy(policy_store, CachePolicy::WriteBack)
                .unwrap(),
            0xA1B2_C3D4
        );
        address_space
            .write16_with_cache_policy(policy_store + 4, 0x8877, CachePolicy::WriteBack)
            .unwrap();
        assert_eq!(
            address_space
                .read16_with_cache_policy(policy_store + 4, CachePolicy::WriteBack)
                .unwrap(),
            0x8877
        );

        // Miss, then hit: a word store followed by byte, half-word and word loads.
        address_space
            .write32(virtual_base + 4, 0x4433_2211)
            .unwrap();
        assert_eq!(address_space.read8(virtual_base + 5).unwrap(), 0x22);
        assert_eq!(address_space.read16(virtual_base + 6).unwrap(), 0x4433);
        assert_eq!(address_space.read32(virtual_base + 4).unwrap(), 0x4433_2211);
        // Still dirty in the cache; the device-visible backing is unchanged.
        let mut backing = [0xFFu8; 4];
        memory.read(physical_base + 4, &mut backing).unwrap();
        assert_eq!(backing, [0; 4]);

        // A word that crosses a 32-byte line boundary takes the byte path.
        address_space
            .write32(virtual_base + 30, 0xDDCC_BBAA)
            .unwrap();
        assert_eq!(
            address_space.read32(virtual_base + 30).unwrap(),
            0xDDCC_BBAA
        );
        assert_eq!(address_space.read8(virtual_base + 32).unwrap(), 0xCC);

        // Hits keep the LRU order: re-reading way 0 makes way 1 the victim.
        let way = (ARM11_DATA_CACHE_SET_COUNT * ARM11_DATA_CACHE_LINE_SIZE) as u32;
        for index in 0..ARM11_DATA_CACHE_WAYS as u32 {
            address_space
                .write32(virtual_base + 0x1000 + index * way, 0xA0 + index)
                .unwrap();
        }
        assert_eq!(address_space.read32(virtual_base + 0x1000).unwrap(), 0xA0);
        address_space
            .write32(
                virtual_base + 0x1000 + ARM11_DATA_CACHE_WAYS as u32 * way,
                0xA4,
            )
            .unwrap();
        address_space.drain_arm11_write_buffer().unwrap();
        let mut evicted = [0u8; 4];
        memory
            .read(physical_base + 0x1000 + way, &mut evicted)
            .unwrap();
        assert_eq!(u32::from_le_bytes(evicted), 0xA1);
        memory.read(physical_base + 0x1000, &mut evicted).unwrap();
        assert_eq!(u32::from_le_bytes(evicted), 0, "way 0 stayed resident");
    }

    #[test]
    fn arm11_data_cache_reaches_its_full_capacity_across_all_sets() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        // Linear FCRAM allocations are page-sized; 0x5000 covers 0x4020 bytes used here.
        let mapped_size = 0x5000usize;
        let physical_base = memory.allocate_linear_fcram(mapped_size as u32).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, mapped_size as u32)
            .unwrap();

        // Fill all 128 sets with four distinct dirty lines apiece.
        for line_index in 0..(ARM11_DATA_CACHE_CAPACITY_BYTES / ARM11_DATA_CACHE_LINE_SIZE) {
            let offset = (line_index * ARM11_DATA_CACHE_LINE_SIZE) as u32;
            address_space
                .write8(virtual_base + offset, (line_index as u8).wrapping_add(1))
                .unwrap();
        }

        assert_eq!(
            address_space.data_cache_line_count(),
            ARM11_DATA_CACHE_CAPACITY_BYTES / ARM11_DATA_CACHE_LINE_SIZE
        );
        assert!(
            address_space
                .data_cache_lru
                .iter()
                .all(|set| set.len() == ARM11_DATA_CACHE_WAYS)
        );

        // The next line collides with set zero and must evict exactly one line.
        let overflow_offset = ARM11_DATA_CACHE_CAPACITY_BYTES as u32;
        address_space
            .write8(virtual_base + overflow_offset, 0xEE)
            .unwrap();
        assert_eq!(
            address_space.data_cache_line_count(),
            ARM11_DATA_CACHE_CAPACITY_BYTES / ARM11_DATA_CACHE_LINE_SIZE
        );
        assert_eq!(
            address_space.read8(virtual_base + overflow_offset).unwrap(),
            0xEE
        );

        // The evicted first line is pending until the write buffer is drained.
        let mut evicted_value = [0u8; 1];
        memory.read(physical_base, &mut evicted_value).unwrap();
        assert_eq!(evicted_value, [0]);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        address_space.drain_arm11_write_buffer().unwrap();
        memory.read(physical_base, &mut evicted_value).unwrap();
        assert_eq!(evicted_value, [1]);
    }

    #[test]
    fn arm11_data_cache_maintenance_handles_cross_line_ranges_atomically() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        // A 32-bit store beginning at byte 30 dirties two adjacent cache lines.
        let crossing_address = virtual_base + (ARM11_DATA_CACHE_LINE_SIZE as u32) - 2;
        address_space
            .write32(crossing_address, 0x1122_3344)
            .unwrap();
        assert_eq!(address_space.data_cache_line_count(), 2);

        let backing_address = physical_base + ARM11_DATA_CACHE_LINE_SIZE as u32 - 2;
        let mut backing_before = [0xFF; 4];
        memory.read(backing_address, &mut backing_before).unwrap();
        assert_eq!(backing_before, [0, 0, 0, 0]);

        // The invalid tail range touches the first line but extends past the mapping.
        // Validation must fail before either dirty line is written back or evicted.
        assert!(
            address_space
                .flush_data_cache_range(virtual_base + 0x0FFF, 2)
                .is_err()
        );
        assert_eq!(address_space.data_cache_line_count(), 2);
        let mut backing_after_rejection = [0xFF; 4];
        memory
            .read(backing_address, &mut backing_after_rejection)
            .unwrap();
        assert_eq!(backing_after_rejection, [0, 0, 0, 0]);

        // The valid cross-line range cleans and invalidates both resident lines.
        assert_eq!(
            address_space
                .flush_data_cache_range(crossing_address, 4)
                .unwrap(),
            2
        );
        assert_eq!(address_space.data_cache_line_count(), 0);
        assert_eq!(address_space.pending_write_buffer_entries(), 2);
        let mut backing_after_flush = [0; 4];
        memory
            .read(backing_address, &mut backing_after_flush)
            .unwrap();
        assert_eq!(backing_after_flush, [0, 0, 0, 0]);
        address_space.drain_arm11_write_buffer().unwrap();
        memory
            .read(backing_address, &mut backing_after_flush)
            .unwrap();
        assert_eq!(backing_after_flush, [0x44, 0x33, 0x22, 0x11]);
    }

    #[test]
    fn arm11_data_cache_is_shared_by_old_and_new_virtual_aliases() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let old_virtual_base = physical_base - 0x0C00_0000;
        let new_virtual_base = NEW_LINEAR_BASE + (physical_base - FCRAM_BASE);
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(old_virtual_base, physical_base, 0x1000)
            .unwrap();
        address_space
            .map_linear_memory_region(new_virtual_base, physical_base, 0x1000)
            .unwrap();

        // Both virtual windows must resolve to the same physically tagged line.
        address_space.write8(old_virtual_base, 0xAB).unwrap();
        assert_eq!(address_space.read8(new_virtual_base).unwrap(), 0xAB);
        assert_eq!(address_space.data_cache_line_count(), 1);
        let mut backing_byte = [0xFF; 1];
        memory.read(physical_base, &mut backing_byte).unwrap();
        assert_eq!(backing_byte, [0]);

        // Cache maintenance through the new alias cleans the line created via the old alias.
        assert_eq!(
            address_space
                .store_data_cache_range(new_virtual_base, 1)
                .unwrap(),
            1
        );
        memory.read(physical_base, &mut backing_byte).unwrap();
        assert_eq!(backing_byte, [0]);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        address_space.drain_arm11_write_buffer().unwrap();
        memory.read(physical_base, &mut backing_byte).unwrap();
        assert_eq!(backing_byte, [0xAB]);

        // A device-side write remains stale in both CPU aliases until invalidation.
        memory.write(physical_base, &[0xCD]).unwrap();
        assert_eq!(address_space.read8(old_virtual_base).unwrap(), 0xAB);
        assert_eq!(
            address_space
                .invalidate_data_cache_range(new_virtual_base, 1)
                .unwrap(),
            1
        );
        assert_eq!(address_space.read8(old_virtual_base).unwrap(), 0xCD);
    }

    #[test]
    fn device_writes_to_linear_memory_invalidate_cpu_exclusive_reservations() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());
        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x1000)
            .unwrap();

        // Undisturbed reservation succeeds.
        let (_, token) = address_space.exclusive_read(virtual_base, 4).unwrap();
        assert!(
            address_space
                .exclusive_write(virtual_base, 4, 1, Some(token))
                .unwrap()
        );

        // A device store to the same granule (not through the CPU) breaks it.
        let (_, token) = address_space.exclusive_read(virtual_base, 4).unwrap();
        memory.write_u32(physical_base + 4, 0xFEED).unwrap();
        assert!(
            !address_space
                .exclusive_write(virtual_base, 4, 2, Some(token))
                .unwrap()
        );
        assert_eq!(address_space.read32(virtual_base).unwrap(), 1);

        // A device store to a different granule leaves the reservation intact.
        let (_, token) = address_space.exclusive_read(virtual_base, 4).unwrap();
        memory.write_u32(physical_base + 0x20, 0xFEED).unwrap();
        assert!(
            address_space
                .exclusive_write(virtual_base, 4, 3, Some(token))
                .unwrap()
        );
    }

    #[test]
    fn linear_process_mapping_aliases_the_shared_gpu_physical_backing() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x2000).unwrap();
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());

        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x2000)
            .unwrap();
        address_space
            .write32(virtual_base + 0x100, 0xCAFE_BABE)
            .unwrap();
        assert_eq!(
            address_space.read32(virtual_base + 0x100).unwrap(),
            0xCAFE_BABE
        );
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0);

        // Store queues the dirty line while leaving a clean CPU copy resident.
        assert_eq!(
            address_space
                .store_data_cache_range(virtual_base + 0x100, 4)
                .unwrap(),
            1
        );
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0);
        assert_eq!(address_space.pending_write_buffer_entries(), 1);
        address_space.drain_arm11_write_buffer().unwrap();
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0xCAFE_BABE);

        // A device write to the same line does not snoop the resident CPU copy.
        memory
            .write_u32(physical_base + 0x104, 0x1234_5678)
            .unwrap();
        assert_eq!(address_space.read32(virtual_base + 0x104).unwrap(), 0);

        // Invalidation through the process alias exposes the device's new bytes.
        assert_eq!(
            address_space
                .invalidate_data_cache_range(virtual_base + 0x104, 4)
                .unwrap(),
            1
        );
        assert_eq!(
            address_space.read32(virtual_base + 0x104).unwrap(),
            0x1234_5678
        );
        assert!(address_space.is_writable_range(virtual_base, 0x2000));
        assert_eq!(
            address_space.query_memory(virtual_base).state,
            Arm11MemoryState::Private
        );

        address_space
            .unmap_linear_memory_region(virtual_base, physical_base, 0x2000)
            .unwrap();
        assert!(!address_space.is_writable_range(virtual_base, 4));
        assert!(matches!(
            address_space.read32(virtual_base),
            Err(Error::InvalidAccess { addr }) if addr == virtual_base
        ));
        memory.free_linear_fcram(physical_base, 0x2000).unwrap();
    }

    #[test]
    fn linear_physical_chunks_coalesce_contiguous_aliases_and_reject_host_backings() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x2000).unwrap();
        let second_physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        assert_eq!(second_physical_base, physical_base + 0x2000);
        let virtual_base = physical_base - 0x0C00_0000;
        address_space.attach_gpu_memory(memory.clone());

        address_space
            .map_linear_memory_region(virtual_base, physical_base, 0x2000)
            .unwrap();
        address_space
            .map_linear_memory_region(virtual_base + 0x2000, second_physical_base, 0x1000)
            .unwrap();

        assert_eq!(
            address_space.linear_physical_chunks_for_range(virtual_base + 0x1FF0, 0x30),
            Some(vec![(physical_base + 0x1FF0, 0x30)])
        );
        assert!(address_space.is_readable_range(virtual_base, 0x3000));
        assert!(address_space.is_writable_range(virtual_base, 0x3000));
        assert_eq!(
            address_space.linear_physical_chunks_for_range(0x0010_3000, 0x20),
            None
        );
        assert_eq!(
            address_space.linear_physical_chunks_for_range(virtual_base, 0),
            None
        );

        memory.free_linear_fcram(physical_base, 0x2000).unwrap();
        memory
            .free_linear_fcram(second_physical_base, 0x1000)
            .unwrap();
    }

    #[test]
    fn linear_mapping_rejects_wrong_alias_and_unreserved_physical_range() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        address_space.attach_gpu_memory(memory.clone());

        assert!(matches!(
            address_space.map_linear_memory_region(0x1400_0000, FCRAM_BASE, 0x1000),
            Err(Arm11DynamicMemoryError::LinearMappingNotFound { .. })
        ));

        let physical = memory.allocate_linear_fcram(0x1000).unwrap();
        assert!(matches!(
            address_space.map_linear_memory_region(0x1400_0000, physical + 0x1000, 0x1000),
            Err(Arm11DynamicMemoryError::OutsideLinearMemoryRange { .. })
        ));
        memory.free_linear_fcram(physical, 0x1000).unwrap();
    }

    #[test]
    fn ipc_temporary_buffer_mappings_preserve_page_offset_and_guard_pages() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let first_backing = SharedMemoryBacking::zeroed(0x2000).unwrap();
        let second_backing = SharedMemoryBacking::zeroed(0x1000).unwrap();

        let first = address_space
            .map_temporary_ipc_buffer(first_backing, 0x234, 0x1200)
            .unwrap();
        assert_eq!(first, 0x0400_1234);
        assert_eq!(
            address_space.query_memory(0x0400_0000).state,
            Arm11MemoryState::Free
        );
        assert_eq!(
            address_space.query_memory(0x0400_1000).state,
            Arm11MemoryState::Shared
        );
        assert_eq!(
            address_space.query_memory(0x0400_3000).state,
            Arm11MemoryState::Free
        );

        address_space.write32(first, 0xCAFE_BABE).unwrap();
        let second = address_space
            .map_temporary_ipc_buffer(second_backing, 0x010, 0x100)
            .unwrap();
        assert_eq!(second, 0x0400_5010);

        address_space.unmap_temporary_ipc_buffer(first).unwrap();
        assert_eq!(
            address_space.query_memory(0x0400_1000).state,
            Arm11MemoryState::Free
        );
        let reused = address_space
            .map_temporary_ipc_buffer(SharedMemoryBacking::zeroed(0x1000).unwrap(), 0x034, 0x20)
            .unwrap();
        assert_eq!(reused, 0x0400_1034);
        assert_eq!(
            address_space.unmap_temporary_ipc_buffer(0x0400_1000),
            Err(Arm11DynamicMemoryError::MappingNotFound {
                address: 0x0400_1000,
                size: 0,
            })
        );
        address_space.unmap_temporary_ipc_buffer(second).unwrap();
        address_space.unmap_temporary_ipc_buffer(reused).unwrap();
    }

    #[test]
    fn ipc_temporary_buffer_mapping_rejects_invalid_backing_extent() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        assert!(matches!(
            address_space.map_temporary_ipc_buffer(
                SharedMemoryBacking::zeroed(0x1000).unwrap(),
                0xFF0,
                0x100,
            ),
            Err(Arm11DynamicMemoryError::InvalidSize(_))
        ));
        assert!(matches!(
            address_space.map_temporary_ipc_buffer(
                SharedMemoryBacking::zeroed(0x1000).unwrap(),
                0x1000,
                0x10,
            ),
            Err(Arm11DynamicMemoryError::InvalidSize(_))
        ));
    }

    #[test]
    fn thread_local_storage_slots_are_zeroed_reused_and_protected() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let first = address_space.allocate_thread_local_storage().unwrap();
        let second = address_space.allocate_thread_local_storage().unwrap();
        assert_eq!((first, second), (0x1FF8_2000, 0x1FF8_2200));

        address_space.write32(second + 0x1FC, 0xDEAD_BEEF).unwrap();
        address_space.release_thread_local_storage(second);
        let reused = address_space.allocate_thread_local_storage().unwrap();
        assert_eq!(reused, second);
        assert_eq!(address_space.read32(second + 0x1FC).unwrap(), 0);

        // Slot nine opens the second page.
        for _ in 2..8 {
            address_space.allocate_thread_local_storage().unwrap();
        }
        assert_eq!(
            address_space.allocate_thread_local_storage().unwrap(),
            0x1FF8_3000
        );

        // The guest cannot unmap TLS through the heap-free path.
        assert!(matches!(
            address_space.free_application_heap_region(0x1FF8_2000, 0x1000),
            Err(Arm11DynamicMemoryError::MappingNotFound { .. })
        ));
        assert_eq!(address_space.read32(first).unwrap(), 0);
    }

    #[test]
    fn shared_backing_materializes_only_pages_touched_by_writes() {
        let backing = SharedMemoryBacking::zeroed(0x3000).unwrap();
        assert_eq!(backing.allocated_page_count(), 0);
        assert_eq!(backing.allocated_bytes(), 0);

        let mut untouched = vec![0xFF; 0x3000];
        assert!(backing.read_bytes(0, &mut untouched));
        assert_eq!(untouched, vec![0; 0x3000]);
        assert_eq!(backing.allocated_page_count(), 0);

        // This write crosses a 4 KiB boundary and materializes only two pages.
        assert!(backing.write_bytes(0x0FFE, &[1, 2, 3, 4]));
        assert_eq!(backing.allocated_page_count(), 2);
        assert_eq!(backing.allocated_bytes(), 0x2000);

        let mut written = [0; 4];
        assert!(backing.read_bytes(0x0FFE, &mut written));
        assert_eq!(written, [1, 2, 3, 4]);
        // Plain stores never grow the monitor map; only observed granules count.
        assert_eq!(backing.exclusive_monitor_entry_count(), 0);
        assert_eq!(backing.exclusive_version(0x0FFE), 0);
        assert!(backing.write_bytes(0x0FFE, &[5]));
        assert_eq!(backing.exclusive_version(0x0FFE), 1);
        assert_eq!(backing.exclusive_monitor_entry_count(), 1);
        assert!(!backing.write_bytes(0x2FFF, &[0xAA, 0xBB]));
        assert_eq!(backing.allocated_page_count(), 2);
    }

    #[test]
    fn memory_usage_report_deduplicates_heap_shared_alias_backing() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let heap = address_space
            .allocate_application_heap_region(0x0800_0000, 0x3000)
            .unwrap();
        let shared_view = address_space
            .shared_memory_backing_for_heap_range(heap + 0x1000, 0x1000)
            .unwrap();
        address_space
            .map_shared_memory_region(0x0900_0000, shared_view.clone(), 3)
            .unwrap();

        assert!(shared_view.write_bytes(0x20, &0xCAFE_BABEu32.to_le_bytes()));
        assert_eq!(address_space.read32(0x0900_0020).unwrap(), 0xCAFE_BABE);

        let report = address_space.memory_usage_report();
        assert_eq!(report.application_heap_virtual_bytes, 0x3000);
        assert_eq!(report.shared_mapping_virtual_bytes, 0x1000);
        assert_eq!(report.unique_sparse_backing_count, 1);
        assert_eq!(report.sparse_backing_pages, 1);
        assert_eq!(report.sparse_backing_allocated_bytes, 0x1000);
    }

    #[test]
    fn application_heap_mapping_is_zeroed_writable_and_non_executable() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let base = address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();

        assert_eq!(base, 0x0800_0000);
        assert!(address_space.is_writable_range(base, 0x2000));
        assert!(address_space.is_writable_range(base + 0x1FFC, 4));
        assert!(!address_space.is_executable_address(base));
        assert_eq!(address_space.read8(base).unwrap(), 0);

        address_space.write32(base, 0xAABB_CCDD).unwrap();
        assert_eq!(address_space.read32(base).unwrap(), 0xAABB_CCDD);

        address_space
            .free_application_heap_region(base, 0x2000)
            .unwrap();
        assert!(!address_space.is_writable_range(base, 1));
        assert!(address_space.read8(base).is_err());
    }

    #[test]
    fn heap_and_shared_mappings_enforce_reprotection_permissions() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let heap = address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();

        address_space
            .protect_memory_region(heap, 0x2000, 1)
            .unwrap();
        assert!(address_space.is_readable_range(heap, 0x2000));
        assert!(!address_space.is_writable_range(heap, 0x2000));
        assert!(address_space.write32(heap, 0x1111_2222).is_err());

        address_space
            .protect_memory_region(heap, 0x2000, 2)
            .unwrap();
        assert!(!address_space.is_readable_range(heap, 0x2000));
        assert!(address_space.is_writable_range(heap, 0x2000));
        address_space.write32(heap, 0x3333_4444).unwrap();
        assert!(address_space.read32(heap).is_err());

        address_space
            .protect_memory_region(heap, 0x2000, 0)
            .unwrap();
        assert!(!address_space.is_readable_range(heap, 0x2000));
        assert!(!address_space.is_writable_range(heap, 0x2000));

        address_space
            .protect_memory_region(heap, 0x2000, 3)
            .unwrap();
        assert!(address_space.is_readable_range(heap, 0x2000));
        assert!(address_space.is_writable_range(heap, 0x2000));
        assert_eq!(address_space.read32(heap).unwrap(), 0x3333_4444);

        let shared = SharedMemoryBacking::zeroed(0x1000).unwrap();
        address_space
            .map_shared_memory_region(0x0600_0000, shared, 2)
            .unwrap();
        assert!(!address_space.is_readable_range(0x0600_0000, 0x1000));
        assert!(address_space.is_writable_range(0x0600_0000, 0x1000));
        address_space
            .protect_memory_region(0x0600_0000, 0x1000, 1)
            .unwrap();
        assert!(address_space.is_readable_range(0x0600_0000, 0x1000));
        assert!(!address_space.is_writable_range(0x0600_0000, 0x1000));

        assert!(
            address_space
                .protect_memory_region(heap, 0x1000, 3)
                .is_err()
        );
        address_space.protect_memory_region(heap, 0x2000, 0).unwrap();
        address_space.free_application_heap_region(heap, 0x2000).unwrap();
        assert!(!address_space.is_readable_range(heap, 1));
        assert!(!address_space.is_writable_range(heap, 1));
    }

    #[test]
    fn application_heap_zero_address_uses_first_fit_without_overlapping_existing_mappings() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();
        address_space
            .allocate_application_heap_region(0x0800_4000, 0x1000)
            .unwrap();

        // The 12 KiB request overlaps the first allocation, then the second
        // allocation after the first gap; first-fit must advance past both.
        let automatic = address_space
            .allocate_application_heap_region(0, 0x3000)
            .unwrap();
        assert_eq!(automatic, 0x0800_5000);
        assert!(address_space.is_writable_range(automatic, 0x3000));

        // A subsequent allocation uses the next available page-aligned gap.
        let next = address_space
            .allocate_application_heap_region(0, 0x1000)
            .unwrap();
        assert_eq!(next, 0x0800_2000);
        assert!(address_space.is_writable_range(next, 0x1000));
    }

    #[test]
    fn application_heap_mapping_rejects_overlap_and_invalid_ranges() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();

        assert_eq!(
            address_space.allocate_application_heap_region(0x0800_1000, 0x1000),
            Err(Arm11DynamicMemoryError::OverlappingMapping)
        );
        assert_eq!(
            address_space.allocate_application_heap_region(0x0800_0001, 0x1000),
            Err(Arm11DynamicMemoryError::MisalignedAddress(0x0800_0001))
        );
        assert_eq!(
            address_space.allocate_application_heap_region(0x0800_3000, 0),
            Err(Arm11DynamicMemoryError::InvalidSize(0))
        );
        assert!(matches!(
            address_space.allocate_application_heap_region(0x0E00_0000, 0x1000),
            Err(Arm11DynamicMemoryError::OutsideApplicationHeap { .. })
        ));
    }

    #[test]
    fn application_heap_can_be_allocated_in_multiple_non_overlapping_regions() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();
        address_space
            .allocate_application_heap_region(0x0800_4000, 0x3000)
            .unwrap();

        assert!(address_space.is_writable_range(0x0800_0000, 0x2000));
        assert!(address_space.is_writable_range(0x0800_4000, 0x3000));
        assert!(!address_space.is_writable_range(0x0800_2000, 1));
    }

    #[test]
    fn instruction_fetch_reads_text_directly_and_matches_data_reads() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let text = address_space.layout.text;
        assert!(!text.is_empty());

        let by_fetch = address_space.fetch32(text.address).unwrap();
        let by_read = address_space.read32(text.address).unwrap();
        assert_eq!(by_fetch, by_read);
        let half = address_space.fetch16(text.address).unwrap();
        assert_eq!(u32::from(half), by_read & 0xFFFF);

        // Outside the text segment the fetch falls back to the data path.
        let stack = address_space.layout.stack;
        assert_eq!(
            address_space.fetch32(stack.address).unwrap(),
            address_space.read32(stack.address).unwrap()
        );
        // A fetch that straddles the end of text is not served directly.
        let end = text.address + text.mapped_size_bytes;
        assert_eq!(
            address_space.fetch_text_bytes::<4>(end.wrapping_sub(2)),
            None
        );
    }

    #[test]
    fn locate_uses_ordered_lookup_for_static_shared_and_dynamic_regions() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        // Insert heap and shared mappings in reverse address order. Their
        // vectors are sorted at insertion and lookup must honour exact ends/gaps.
        address_space
            .allocate_application_heap_region(0x0800_4000, 0x2000)
            .unwrap();
        address_space
            .allocate_application_heap_region(0x0800_0000, 0x1000)
            .unwrap();
        address_space
            .map_shared_memory_region(0x0900_4000, SharedMemoryBacking::zeroed(0x1000).unwrap(), 3)
            .unwrap();
        address_space
            .map_shared_memory_region(0x0900_0000, SharedMemoryBacking::zeroed(0x1000).unwrap(), 1)
            .unwrap();

        assert!(matches!(
            address_space.locate(0x0010_1FFF),
            Some(MemoryLocation::Static {
                kind: Arm11SegmentKind::Text,
                offset: 0x1FFF,
            })
        ));
        assert!(matches!(
            address_space.locate(0x0010_2000),
            Some(MemoryLocation::Static {
                kind: Arm11SegmentKind::ReadOnly,
                offset: 0,
            })
        ));
        assert!(matches!(
            address_space.locate(0x0010_3000),
            Some(MemoryLocation::Static {
                kind: Arm11SegmentKind::Data,
                offset: 0,
            })
        ));
        assert!(matches!(
            address_space.locate(0x0010_4000),
            Some(MemoryLocation::Static {
                kind: Arm11SegmentKind::Bss,
                offset: 0,
            })
        ));
        assert!(matches!(
            address_space.locate(0x0FFF_C000),
            Some(MemoryLocation::Static {
                kind: Arm11SegmentKind::Stack,
                offset: 0,
            })
        ));

        assert!(matches!(
            address_space.locate(0x0800_0FFF),
            Some(MemoryLocation::Dynamic {
                index: 0,
                offset: 0x0FFF
            })
        ));
        assert!(address_space.locate(0x0800_2000).is_none());
        assert!(matches!(
            address_space.locate(0x0800_5FFF),
            Some(MemoryLocation::Dynamic {
                index: 1,
                offset: 0x1FFF
            })
        ));
        assert!(address_space.locate(0x0800_6000).is_none());

        assert!(matches!(
            address_space.locate(0x0900_0FFF),
            Some(MemoryLocation::Shared {
                index: 0,
                offset: 0x0FFF
            })
        ));
        assert!(address_space.locate(0x0900_2000).is_none());
        assert!(matches!(
            address_space.locate(0x0900_4FFF),
            Some(MemoryLocation::Shared {
                index: 1,
                offset: 0x0FFF
            })
        ));
        assert!(address_space.locate(0x0900_5000).is_none());
    }

    #[test]
    fn locate_resolves_linear_aliases_and_physical_fcram_with_ordered_lookup() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x2000).unwrap();
        let old_virtual_base = physical_base - 0x0C00_0000;
        let new_virtual_base = NEW_LINEAR_BASE + (physical_base - FCRAM_BASE);
        address_space.attach_gpu_memory(memory.clone());

        // Insert in reverse virtual order. Sorting and binary lookup must still
        // select the old alias first when an MMU access supplies the physical address.
        address_space
            .map_linear_memory_region(new_virtual_base, physical_base, 0x2000)
            .unwrap();
        address_space
            .map_linear_memory_region(old_virtual_base, physical_base, 0x2000)
            .unwrap();

        assert_eq!(
            address_space
                .memory_usage_report()
                .linear_physical_allocated_bytes,
            0x2000
        );

        assert!(matches!(
            address_space.locate(old_virtual_base),
            Some(MemoryLocation::Physical {
                index: 0,
                address,
            }) if address == physical_base
        ));
        assert!(matches!(
            address_space.locate(old_virtual_base + 0x1FFF),
            Some(MemoryLocation::Physical {
                index: 0,
                address,
            }) if address == physical_base + 0x1FFF
        ));
        assert!(matches!(
            address_space.locate(new_virtual_base),
            Some(MemoryLocation::Physical {
                index: 1,
                address,
            }) if address == physical_base
        ));
        assert!(matches!(
            address_space.locate(physical_base),
            Some(MemoryLocation::Physical {
                index: 0,
                address,
            }) if address == physical_base
        ));
        assert!(matches!(
            address_space.locate(physical_base + 0x1FFF),
            Some(MemoryLocation::Physical {
                index: 0,
                address,
            }) if address == physical_base + 0x1FFF
        ));
        assert!(address_space.locate(physical_base + 0x2000).is_none());
    }

    #[test]
    fn address_space_rejects_unmapped_addresses() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        assert!(matches!(
            address_space.read8(0x000F_FFFF),
            Err(crate::Error::InvalidAccess { addr: 0x000F_FFFF })
        ));
    }

    #[test]
    fn maps_a_zeroed_main_thread_stack_below_the_documented_top() {
        let address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let stack = address_space.layout.stack;

        assert_eq!(stack.address, 0x0FFF_C000);
        assert_eq!(stack.mapped_size_bytes, 0x4000);
        assert_eq!(address_space.layout.initial_stack_pointer, MAIN_STACK_TOP);
        assert!(stack.writable);
        assert!(!stack.executable);
        assert!(
            address_space
                .segment_bytes(Arm11SegmentKind::Stack)
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn main_thread_stack_is_writable_and_rejects_access_above_its_top() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();
        let stack = address_space.layout.stack;

        address_space
            .write32(MAIN_STACK_TOP - 4, 0xCAFE_BABE)
            .unwrap();
        assert_eq!(
            address_space.read32(MAIN_STACK_TOP - 4).unwrap(),
            0xCAFE_BABE
        );
        assert!(address_space.read8(MAIN_STACK_TOP).is_err());
        assert!(address_space.write8(stack.address - 1, 0).is_err());
    }

    #[test]
    fn rejects_a_stack_larger_than_its_reserved_virtual_region() {
        let mut prepared = prepared_image();
        prepared.extended_header.stack_size_bytes = MAIN_STACK_TOP + 1;

        assert!(matches!(
            prepared.process_layout(),
            Err(Arm11ProcessLayoutError::StackTooLarge { .. })
        ));
    }

    #[test]
    fn builds_arm11_mappings_with_expected_permissions_and_sizes() {
        let prepared = prepared_image();
        let layout = prepared.process_layout().unwrap();

        assert_eq!(layout.application_title, *b"ACERIS!!");
        assert_eq!(layout.entry_point, 0x0010_0000);
        assert_eq!(layout.text.address, 0x0010_0000);
        assert_eq!(layout.text.mapped_size_bytes, 0x2000);
        assert!(!layout.text.writable);
        assert!(layout.text.executable);
        assert_eq!(layout.read_only.address, 0x0010_2000);
        assert_eq!(layout.read_only.mapped_size_bytes, 0x1000);
        assert!(!layout.read_only.writable);
        assert!(!layout.read_only.executable);
        assert_eq!(layout.data.address, 0x0010_3000);
        assert_eq!(layout.data.mapped_size_bytes, 0x1000);
        assert!(layout.data.writable);
        assert!(!layout.data.executable);
        assert_eq!(layout.bss.address, 0x0010_4000);
        assert_eq!(layout.bss.mapped_size_bytes, 0x2000);
        assert!(layout.bss.writable);
        assert!(!layout.bss.executable);
        assert_eq!(layout.bss_size_bytes, 0x2000);
        assert_eq!(layout.stack.address, 0x0FFF_C000);
        assert_eq!(layout.stack.mapped_size_bytes, 0x4000);
        assert_eq!(layout.initial_stack_pointer, 0x1000_0000);
        assert_eq!(layout.stack_size_bytes, 0x4000);
    }

    #[test]
    fn rejects_a_segment_address_that_is_not_page_aligned() {
        let mut prepared = prepared_image();
        prepared.extended_header.read_only_code_set.address += 4;

        assert_eq!(
            prepared.process_layout(),
            Err(Arm11ProcessLayoutError::SegmentAddressUnaligned {
                segment: Arm11SegmentKind::ReadOnly,
                address: 0x0010_2004,
            })
        );
    }

    #[test]
    fn rejects_overlapping_virtual_segments() {
        let mut prepared = prepared_image();
        prepared.extended_header.read_only_code_set.address = 0x0010_1000;

        assert_eq!(
            prepared.process_layout(),
            Err(Arm11ProcessLayoutError::OverlappingSegments {
                first: Arm11SegmentKind::Text,
                second: Arm11SegmentKind::ReadOnly,
            })
        );
    }

    #[test]
    fn rejects_a_prepared_segment_with_the_wrong_size() {
        let mut prepared = prepared_image();
        prepared.data.pop();

        assert_eq!(
            prepared.process_layout(),
            Err(Arm11ProcessLayoutError::SegmentImageSizeMismatch {
                segment: Arm11SegmentKind::Data,
                expected_bytes: 0x1000,
                actual_bytes: 0x0FFF,
            })
        );
    }

    #[test]
    fn rejects_an_empty_text_segment() {
        let mut prepared = prepared_image();
        prepared.extended_header.text_code_set.size_bytes = 0;
        prepared.extended_header.text_code_set.physical_size_pages = 0;
        prepared.text.clear();

        assert_eq!(
            prepared.process_layout(),
            Err(Arm11ProcessLayoutError::EmptyTextSegment)
        );
    }

    #[test]
    fn validates_ranges_near_the_32_bit_address_boundary() {
        let mut prepared = prepared_image();
        prepared.extended_header.text_code_set.address = 0xFFFF_D000;

        let layout = prepared.process_layout().unwrap();
        assert_eq!(layout.text.end_address(), 0xFFFF_F000);
    }
}
