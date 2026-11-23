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
use crate::memory::{Bus, CachePolicy};
use aceris_common::{Error, Result as BusResult};

const PAGE_SIZE: u64 = 0x1000;
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

/// Shared page backing that can be mapped into several ARM11 address spaces
/// without copying. Anonymous blocks start zeroed; heap-backed views retain
/// the bytes that were already present in the source allocation.
#[derive(Clone, Debug)]
pub struct SharedMemoryBacking {
    bytes: Arc<Mutex<Vec<u8>>>,
    offset: usize,
    length: usize,
    backing_id: u64,
    exclusive_versions: Arc<Mutex<BTreeMap<u32, u64>>>,
}

impl SharedMemoryBacking {
    /// Allocates a zero-filled shared backing of exactly `size` bytes.
    pub fn zeroed(size: usize) -> std::result::Result<Self, String> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|error| error.to_string())?;
        bytes.resize(size, 0);
        Ok(Self::from_vec(bytes))
    }

    fn from_vec(bytes: Vec<u8>) -> Self {
        let length = bytes.len();
        Self {
            bytes: Arc::new(Mutex::new(bytes)),
            offset: 0,
            length,
            backing_id: crate::memory::next_memory_backing_id(),
            exclusive_versions: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Creates a bounded view into the same allocation without copying.
    fn view(&self, offset: usize, length: usize) -> Option<Self> {
        let end = offset.checked_add(length)?;
        if end > self.length {
            return None;
        }
        Some(Self {
            bytes: self.bytes.clone(),
            offset: self.offset.checked_add(offset)?,
            length,
            backing_id: self.backing_id,
            exclusive_versions: self.exclusive_versions.clone(),
        })
    }

    /// Returns the view size, or None if its backing lock was poisoned.
    pub fn len(&self) -> Option<usize> {
        self.bytes.lock().ok().map(|_| self.length)
    }

    /// Returns whether the backing view is empty, or None if its lock was poisoned.
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

    fn root_base_offset(&self) -> usize {
        self.offset
    }

    fn exclusive_version(&self, offset: usize) -> u64 {
        let Some(root_offset) = self.root_offset(offset) else {
            return 0;
        };
        let granule = (root_offset as u32) & !(crate::memory::EXCLUSIVE_RESERVATION_GRANULE - 1);
        self.exclusive_versions
            .lock()
            .ok()
            .and_then(|versions| versions.get(&granule).copied())
            .unwrap_or(0)
    }

    fn read_range(&self, offset: usize, output: &mut [u8]) -> bool {
        let Some(start) = self.root_offset(offset) else {
            return false;
        };
        if offset
            .checked_add(output.len())
            .is_none_or(|end| end > self.length)
        {
            return false;
        }
        let Ok(bytes) = self.bytes.lock() else {
            return false;
        };
        let Some(source) = bytes.get(start..start + output.len()) else {
            return false;
        };
        output.copy_from_slice(source);
        true
    }

    /// Writes bytes into the shared view, bumping exclusive-monitor versions.
    ///
    /// Returns false when the range lies outside the view or the lock is poisoned.
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
        if offset
            .checked_add(input.len())
            .is_none_or(|end| end > self.length)
        {
            return false;
        }
        let Ok(mut bytes) = self.bytes.lock() else {
            return false;
        };
        let Some(destination) = bytes.get_mut(start..start + input.len()) else {
            return false;
        };
        destination.copy_from_slice(input);
        drop(bytes);

        if !input.is_empty() {
            let absolute_end = start + input.len();
            if let Ok(mut versions) = self.exclusive_versions.lock() {
                let granule_size = crate::memory::EXCLUSIVE_RESERVATION_GRANULE as usize;
                let mut granule = start & !(granule_size - 1);
                while granule < absolute_end {
                    let version = versions.entry(granule as u32).or_insert(0);
                    *version = version.wrapping_add(1);
                    granule += granule_size;
                }
            }
        }
        true
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
    /// Shared ARM11 MPCore Snoop Control Unit registers for this system.
    mpcore_scu: Option<MpcoreScu>,
    /// Core-0 MPCore private timer, driven by the session's virtual clock.
    mpcore_private_timer: MpcorePrivateTimer,
    /// ARM11 MPCore distributor and CPU-interface state for the application core.
    mpcore_interrupt_controller: MpcoreInterruptController,
    /// Stable identities for the process' private static segments.
    static_backing_ids: [u64; 5],
    /// Write-back ARM11 data-cache lines, keyed by backing identity and offset.
    data_cache: BTreeMap<DataCacheKey, Arm11DataCacheLine>,
    /// Per-set least-recently-used order, oldest line first.
    data_cache_lru: Vec<VecDeque<DataCacheKey>>,
    /// FIFO of dirty D-cache lines awaiting visibility in physical memory.
    write_buffer: VecDeque<Arm11WriteBufferEntry>,
    /// Per-eight-byte exclusive monitor generations for mapped virtual memory.
    exclusive_versions: BTreeMap<u32, u64>,
    /// Occupancy of each 0x200-byte thread-local-storage slot.
    tls_slots_in_use: Vec<bool>,
    /// Temporary IPC aliases and their surrounding unmapped guard pages.
    ipc_temporary_regions: Vec<IpcTemporaryMappingRegion>,
}

impl Arm11CodeAddressSpace {
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
            mpcore_scu: None,
            mpcore_private_timer: MpcorePrivateTimer::default(),
            mpcore_interrupt_controller: MpcoreInterruptController::default(),
            static_backing_ids: std::array::from_fn(|_| crate::memory::next_memory_backing_id()),
            data_cache: BTreeMap::new(),
            data_cache_lru: (0..ARM11_DATA_CACHE_SET_COUNT)
                .map(|_| VecDeque::with_capacity(ARM11_DATA_CACHE_WAYS))
                .collect(),
            write_buffer: VecDeque::with_capacity(ARM11_WRITE_BUFFER_CAPACITY),
            exclusive_versions: BTreeMap::new(),
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
            mpcore_scu: None,
            mpcore_private_timer: MpcorePrivateTimer::default(),
            mpcore_interrupt_controller: MpcoreInterruptController::default(),
            static_backing_ids: std::array::from_fn(|_| crate::memory::next_memory_backing_id()),
            data_cache: BTreeMap::new(),
            data_cache_lru: (0..ARM11_DATA_CACHE_SET_COUNT)
                .map(|_| VecDeque::with_capacity(ARM11_DATA_CACHE_WAYS))
                .collect(),
            write_buffer: VecDeque::with_capacity(ARM11_WRITE_BUFFER_CAPACITY),
            exclusive_versions: BTreeMap::new(),
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
            .map_or(true, |access| access.allows_bytes(service_name))
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
        self.gpu_memory = Some(memory);
    }

    /// Attaches the system-wide MPCore SCU peripheral visible to this process.
    pub fn attach_mpcore_scu(&mut self, scu: MpcoreScu) {
        self.mpcore_scu = Some(scu);
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

    fn mpcore_private_offset(&self, address: u32) -> Option<u32> {
        address.checked_sub(crate::cpu::MPCORE_PRIVATE_MEMORY_BASE)
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
        (MPCORE_GIC_DISTRIBUTOR_OFFSET..MPCORE_GIC_DISTRIBUTOR_OFFSET + MPCORE_GIC_DISTRIBUTOR_SIZE)
            .contains(&private_offset)
            .then_some(private_offset - MPCORE_GIC_DISTRIBUTOR_OFFSET)
    }

    fn read_mpcore_cpu_interface_register(&mut self, address: u32) -> BusResult<u32> {
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
        let offset = self
            .mpcore_gic_distributor_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        Ok(self
            .mpcore_interrupt_controller
            .read_distributor_byte(offset))
    }

    fn write_mpcore_gic_distributor_byte(&mut self, address: u32, value: u8) -> BusResult<()> {
        let offset = self
            .mpcore_gic_distributor_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        self.mpcore_interrupt_controller
            .write_distributor_byte(offset, value);
        Ok(())
    }

    fn read_mpcore_gic_distributor_word(&self, address: u32) -> BusResult<u32> {
        let mut bytes = [0u8; 4];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read_mpcore_gic_distributor_byte(address.wrapping_add(index as u32))?;
        }
        Ok(u32::from_le_bytes(bytes))
    }

    fn write_mpcore_gic_distributor_word(&mut self, address: u32, value: u32) -> BusResult<()> {
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write_mpcore_gic_distributor_byte(address.wrapping_add(index as u32), byte)?;
        }
        Ok(())
    }

    fn read_mpcore_private_timer_register(&self, address: u32) -> BusResult<u32> {
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
        let offset = address.checked_sub(crate::cpu::MPCORE_PRIVATE_MEMORY_BASE)?;
        (offset < crate::cpu::SCU_REGISTER_WINDOW_SIZE).then_some(offset & !3)
    }

    fn read_mpcore_scu_register(&self, address: u32) -> BusResult<u32> {
        let offset = self
            .mpcore_scu_register_offset(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        self.mpcore_scu
            .as_ref()
            .and_then(|scu| scu.read_register(offset))
            .ok_or(Error::InvalidAccess { addr: address })
    }

    fn write_mpcore_scu_register(&mut self, address: u32, value: u32) -> BusResult<()> {
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
        if permissions > 3 || permissions == 2 {
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
        if address % PAGE_SIZE as u32 != 0 {
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
        self.flush_data_cache_range(address, size)
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
            || virtual_base % PAGE_SIZE as u32 != 0
            || physical_base % PAGE_SIZE as u32 != 0
            || size % PAGE_SIZE as u32 != 0
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
    pub fn allocate_application_heap_region(
        &mut self,
        address: u32,
        size: u32,
    ) -> std::result::Result<u32, Arm11DynamicMemoryError> {
        if u64::from(address) % PAGE_SIZE != 0 {
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
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(backing_size)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;
        bytes.resize(backing_size, 0);
        self.dynamic_regions
            .try_reserve(1)
            .map_err(|error| Arm11DynamicMemoryError::AllocationFailure(error.to_string()))?;
        self.dynamic_regions.push(DynamicMemoryRegion {
            address,
            backing: DynamicMemoryBacking::Private(SharedMemoryBacking::from_vec(bytes)),
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
                if slot as u32 % THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE == 0 {
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
        self.flush_data_cache_range(address, size)
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

    fn locate(&self, address: u32) -> Option<MemoryLocation> {
        let segments = [
            (Arm11SegmentKind::Text, self.layout.text),
            (Arm11SegmentKind::ReadOnly, self.layout.read_only),
            (Arm11SegmentKind::Data, self.layout.data),
            (Arm11SegmentKind::Bss, self.layout.bss),
            (Arm11SegmentKind::Stack, self.layout.stack),
        ];

        for (kind, segment) in segments {
            if segment.is_empty() || address < segment.address {
                continue;
            }

            let offset = u64::from(address) - u64::from(segment.address);
            if offset < u64::from(segment.mapped_size_bytes) {
                let offset = usize::try_from(offset).ok()?;
                return Some(MemoryLocation::Static { kind, offset });
            }
        }

        if let Some((index, region)) = self.linear_regions.iter().enumerate().find(|(_, region)| {
            address >= region.virtual_base && u64::from(address) < region.virtual_end()
        }) {
            let offset = address - region.virtual_base;
            return Some(MemoryLocation::Physical {
                index,
                address: region.physical_base + offset,
            });
        }

        // A translated ARM11 access reaches this bus with its physical
        // address. Resolve registered linear FCRAM backing in both directions
        // so MMU-enabled CPU accesses and device accesses share the same cache.
        if let Some((index, _)) = self.linear_regions.iter().enumerate().find(|(_, region)| {
            address >= region.physical_base
                && u64::from(address) < u64::from(region.physical_base) + u64::from(region.size)
        }) {
            return Some(MemoryLocation::Physical { index, address });
        }

        if let Some((index, region)) = self.shared_regions.iter().enumerate().find(|(_, region)| {
            address >= region.address
                && region
                    .end_address()
                    .is_some_and(|end| u64::from(address) < end)
        }) {
            let offset = usize::try_from(address - region.address).ok()?;
            return Some(MemoryLocation::Shared { index, offset });
        }

        self.dynamic_regions
            .iter()
            .enumerate()
            .find_map(|(index, region)| {
                if address < region.address {
                    return None;
                }
                let offset = u64::from(address) - u64::from(region.address);
                (offset < region.len() as u64).then(|| {
                    usize::try_from(offset)
                        .ok()
                        .map(|offset| MemoryLocation::Dynamic { index, offset })
                })?
            })
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
            MemoryLocation::Static { .. }
            | MemoryLocation::Dynamic { .. }
            | MemoryLocation::Physical { .. } => true,
            MemoryLocation::Shared { index, .. } => self
                .shared_regions
                .get(index)
                .is_some_and(|region| region.permissions & 1 != 0),
        }
    }

    fn location_is_writable(&self, location: MemoryLocation) -> bool {
        match location {
            MemoryLocation::Static { kind, .. } => self.segment_is_writable(kind),
            MemoryLocation::Dynamic { .. } | MemoryLocation::Physical { .. } => true,
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

    fn write_bytes(&mut self, address: u32, bytes: &[u8]) -> BusResult<()> {
        self.write_bytes_with_cache_policy(address, bytes, CachePolicy::WriteBack)
    }

    fn write_bytes_with_cache_policy(
        &mut self,
        address: u32,
        bytes: &[u8],
        policy: CachePolicy,
    ) -> BusResult<()> {
        if policy == CachePolicy::WriteBack && self.try_write_cached_hit(address, bytes) {
            self.invalidate_exclusive_range(address, bytes.len());
            return Ok(());
        }

        let mut locations = Vec::with_capacity(bytes.len());

        // Validate the complete range before mutating cache state or backing.
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

        for (byte, location) in bytes.iter().zip(locations) {
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
                    backing_id: memory.backing_id(),
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
                let backing_id = self.gpu_memory.as_ref()?.backing_id();
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

    /// Cacheable read that hits a resident line entirely inside one line.
    ///
    /// Returns `None` (leaving all state untouched) for anything that needs the
    /// full path: a miss, a line-crossing access, or an unreadable location.
    fn try_read_cached_hit(&mut self, address: u32, len: usize) -> Option<u32> {
        let location = self.locate(address)?;
        if !self.location_is_readable(location) {
            return None;
        }
        let (key, offset) = self.cache_key_for(location)?;
        if offset + len > ARM11_DATA_CACHE_LINE_SIZE {
            return None;
        }
        let line = self.data_cache.get(&key)?;
        let mut bytes = [0u8; 4];
        bytes[..len].copy_from_slice(line.bytes.get(offset..offset + len)?);
        self.touch_data_cache_line(key);
        Some(u32::from_le_bytes(bytes))
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
        if self.data_cache.contains_key(&key) {
            self.touch_data_cache_line(key);
        } else {
            let mut bytes = self.read_cache_target_line(target)?;
            self.overlay_write_buffer(key, &mut bytes);
            self.insert_data_cache_line(
                key,
                Arm11DataCacheLine {
                    bytes,
                    dirty: false,
                    target: target.clone(),
                },
            )?;
        }

        self.data_cache
            .get(&key)
            .and_then(|line| line.bytes.get(offset).copied())
            .ok_or(Error::InvalidAccess { addr: 0 })
    }

    fn write_cached_byte(
        &mut self,
        target: &DataCacheTarget,
        key: DataCacheKey,
        offset: usize,
        value: u8,
    ) -> BusResult<()> {
        let _ = self.read_cached_byte(target, key, offset)?;
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
            if !self.location_is_readable(location) {
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

            let (mut key, _target, mut offset) =
                self.cache_location(location).ok_or(Error::InvalidAccess {
                    addr: current_address,
                })?;
            let mut remaining = region_end - current;
            while remaining > 0 {
                lines.insert(key);
                let covered = remaining.min((ARM11_DATA_CACHE_LINE_SIZE - offset) as u64);
                remaining -= covered;
                offset = 0;
                key.line_offset += ARM11_DATA_CACHE_LINE_SIZE as u64;
            }

            current = region_end;
        }

        Ok(lines)
    }

    /// Cleans and invalidates every cache line intersecting a process virtual range.
    /// The complete range is checked before any line is written back or evicted.
    pub fn flush_data_cache_range(&mut self, address: u32, size_bytes: u32) -> BusResult<usize> {
        let lines = self.cache_lines_for_range(address, size_bytes)?;
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
        let lines = self.cache_lines_for_range(address, size_bytes)?;
        let mut evicted = 0usize;

        for key in lines {
            if self.remove_data_cache_line(key).is_some() {
                evicted += 1;
            }
        }

        Ok(evicted)
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
            let version = self.exclusive_versions.entry(key).or_insert(0);
            *version = version.wrapping_add(1);
            granule += u64::from(crate::memory::EXCLUSIVE_RESERVATION_GRANULE);
        }
    }

    fn validate_exclusive_access(address: u32, size: usize) -> BusResult<()> {
        if !matches!(size, 1 | 2 | 4 | 8) || address as usize % size != 0 {
            return Err(Error::InvalidAccess { addr: address });
        }
        Ok(())
    }
}

impl Bus for Arm11CodeAddressSpace {
    fn read8(&mut self, address: u32) -> BusResult<u8> {
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
            if let Some(value) = self.try_read_cached_hit(address, 1) {
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
        if let Some(value) = self.try_read_cached_hit(address, 2) {
            return Ok(value as u16);
        }
        let lo = self.read8(address)?;
        let hi = self.read8(address.wrapping_add(1))?;
        Ok(u16::from_le_bytes([lo, hi]))
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
        if let Some(value) = self.try_read_cached_hit(address, 4) {
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
            if let Some(value) = self.try_read_cached_hit(address, 4) {
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
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8_with_cache_policy(address.wrapping_add(index as u32), byte, policy)?;
        }
        Ok(())
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
        if let Some(scu) = &self.mpcore_scu {
            scu.set_cpu_smp_mode(core_id, enabled);
        }
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
        assert_eq!(
            address_space.map_shared_memory_region(0x0800_0000, backing.clone(), 2),
            Err(Arm11DynamicMemoryError::InvalidPermissions(2))
        );
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
