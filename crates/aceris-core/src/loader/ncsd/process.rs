//! ARM11 process-layout validation for prepared Nintendo 3DS executable images.
//!
//! This module converts parsed code-set metadata into validated virtual
//! mappings for code, BSS, and the main-thread stack. It does not allocate
//! physical memory, configure privileged kernel state, schedule threads, or
//! start a complete console execution environment.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use super::PreparedArm11Image;
use super::ncch::extended_header::{Arm11MemoryType, CodeFileLayout, CodeSetInfo};
use crate::cpu::{Arm11Core, SP};
use crate::gpu::{
    FCRAM_BASE, FCRAM_SIZE, GpuPhysicalMemory, NEW_LINEAR_BASE, NEW_LINEAR_END, OLD_LINEAR_BASE,
    OLD_LINEAR_END,
};
use crate::memory::Bus;
use aceris_common::{Error, Result as BusResult};

const PAGE_SIZE: u64 = 0x1000;
const ADDRESS_SPACE_END: u64 = 0x1_0000_0000;
const MAIN_STACK_TOP: u32 = 0x1000_0000;
const APPLICATION_HEAP_BASE: u32 = 0x0800_0000;
const APPLICATION_HEAP_END: u64 = 0x0E00_0000;
/// Base of the per-thread local storage window and the size of one slot.
const THREAD_LOCAL_STORAGE_BASE: u32 = 0x1FF8_2000;
const THREAD_LOCAL_STORAGE_SLOT_SIZE: u32 = 0x200;
const THREAD_LOCAL_STORAGE_SLOTS_PER_PAGE: u32 = 8;
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

/// Shared, zero-initialized memory backing that can be mapped into several
/// ARM11 address spaces without copying its contents.
#[derive(Clone, Debug)]
pub struct SharedMemoryBacking {
    bytes: Arc<Mutex<Vec<u8>>>,
    offset: usize,
    length: usize,
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
        })
    }

    /// Returns the view size, or None if its backing lock was poisoned.
    pub fn len(&self) -> Option<usize> {
        self.bytes.lock().ok().map(|_| self.length)
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

    fn read_byte(&self, offset: usize) -> Option<u8> {
        if offset >= self.length {
            return None;
        }
        self.bytes
            .lock()
            .ok()?
            .get(self.offset.checked_add(offset)?)
            .copied()
    }

    fn write_byte(&self, offset: usize, value: u8) -> bool {
        if offset >= self.length {
            return false;
        }
        let Ok(mut bytes) = self.bytes.lock() else {
            return false;
        };
        let Some(byte) = self
            .offset
            .checked_add(offset)
            .and_then(|absolute| bytes.get_mut(absolute))
        else {
            return false;
        };
        *byte = value;
        true
    }
}

impl PartialEq for SharedMemoryBacking {
    fn eq(&self, other: &Self) -> bool {
        self.is_same_view_as(other)
    }
}

impl Eq for SharedMemoryBacking {}

#[derive(Debug)]
enum DynamicMemoryBacking {
    Private(Vec<u8>),
    Shared(SharedMemoryBacking),
}

impl DynamicMemoryBacking {
    fn len(&self) -> usize {
        match self {
            Self::Private(bytes) => bytes.len(),
            Self::Shared(backing) => backing.length,
        }
    }

    fn read_byte(&self, offset: usize) -> Option<u8> {
        match self {
            Self::Private(bytes) => bytes.get(offset).copied(),
            Self::Shared(backing) => backing.read_byte(offset),
        }
    }

    fn write_byte(&mut self, offset: usize, value: u8) -> bool {
        match self {
            Self::Private(bytes) => {
                let Some(byte) = bytes.get_mut(offset) else {
                    return false;
                };
                *byte = value;
                true
            }
            Self::Shared(backing) => backing.write_byte(offset, value),
        }
    }

    /// Promotes the allocation to shareable backing and returns a bounded view.
    fn shared_view(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Option<SharedMemoryBacking> {
        if let Self::Private(bytes) = self {
            let bytes = std::mem::take(bytes);
            *self = Self::Shared(SharedMemoryBacking::from_vec(bytes));
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
/// zero-filled BSS tail, and the main-thread stack. Shared memory, hardware
/// registers, and privileged kernel regions are not mapped here.
#[derive(Debug)]
pub struct Arm11CodeAddressSpace {
    layout: Arm11ProcessLayout,
    memory_type: Arm11MemoryType,
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
    /// Per-eight-byte exclusive monitor generations for mapped virtual memory.
    exclusive_versions: BTreeMap<u32, u64>,
    /// Occupancy of each 0x200-byte thread-local-storage slot.
    tls_slots_in_use: Vec<bool>,
}

impl Arm11CodeAddressSpace {
    /// Creates a segmented address space from a verified prepared executable.
    pub fn from_prepared(
        prepared: PreparedArm11Image,
    ) -> std::result::Result<Self, Arm11ProcessLayoutError> {
        let memory_type = prepared.extended_header.arm11_memory_type();
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
            text: prepared.text,
            read_only: prepared.read_only,
            data: prepared.data,
            bss,
            stack,
            dynamic_regions: Vec::new(),
            shared_regions: Vec::new(),
            linear_regions: Vec::new(),
            gpu_memory: None,
            exclusive_versions: BTreeMap::new(),
            tls_slots_in_use: Vec::new(),
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

        let Some(region) = self.dynamic_regions.iter_mut().find(|region| {
            address >= region.address && end <= region.end_address()
        }) else {
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
            backing: DynamicMemoryBacking::Private(bytes),
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
                        backing: DynamicMemoryBacking::Private(vec![0; PAGE_SIZE as usize]),
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

    fn bytes_for_location(&self, location: MemoryLocation) -> &[u8] {
        match location {
            MemoryLocation::Static { kind, .. } => self.bytes_for(kind),
            MemoryLocation::Dynamic { .. }
            | MemoryLocation::Shared { .. }
            | MemoryLocation::Physical { .. } => {
                unreachable!("dynamic, shared and physical locations are read from their backing")
            }
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
        let mut locations = Vec::with_capacity(bytes.len());

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
            match location {
                MemoryLocation::Static { kind, offset } => {
                    self.bytes_for_mut(kind)[offset] = *byte;
                }
                MemoryLocation::Dynamic { index, offset } => {
                    if !self.dynamic_regions[index].backing.write_byte(offset, *byte) {
                        return Err(Error::InvalidAccess { addr: address });
                    }
                }
                MemoryLocation::Shared { index, offset } => {
                    if !self.shared_regions[index].backing.write_byte(offset, *byte) {
                        return Err(Error::InvalidAccess { addr: address });
                    }
                }
                MemoryLocation::Physical {
                    address: physical_address,
                    ..
                } => {
                    let memory = self
                        .gpu_memory
                        .as_mut()
                        .ok_or(Error::InvalidAccess { addr: address })?;
                    memory
                        .write(physical_address, &[*byte])
                        .map_err(|_| Error::InvalidAccess { addr: address })?;
                }
            }
        }

        self.invalidate_exclusive_range(address, bytes.len());
        Ok(())
    }

    fn exclusive_version(&self, address: u32) -> u64 {
        self.exclusive_versions
            .get(&(address & !(crate::memory::EXCLUSIVE_RESERVATION_GRANULE - 1)))
            .copied()
            .unwrap_or(0)
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
        let location = self
            .locate(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        match location {
            MemoryLocation::Dynamic { index, offset } => self.dynamic_regions[index]
                .backing
                .read_byte(offset)
                .ok_or(Error::InvalidAccess { addr: address }),
            MemoryLocation::Shared { index, offset } => {
                let region = &self.shared_regions[index];
                if region.permissions & 1 == 0 {
                    return Err(Error::InvalidAccess { addr: address });
                }
                region
                    .backing
                    .read_byte(offset)
                    .ok_or(Error::InvalidAccess { addr: address })
            }
            MemoryLocation::Physical {
                address: physical_address,
                ..
            } => {
                let mut byte = [0u8; 1];
                self.gpu_memory
                    .as_ref()
                    .ok_or(Error::InvalidAccess { addr: address })?
                    .read(physical_address, &mut byte)
                    .map_err(|_| Error::InvalidAccess { addr: address })?;
                Ok(byte[0])
            }
            MemoryLocation::Static { offset, .. } => self
                .bytes_for_location(location)
                .get(offset)
                .copied()
                .ok_or(Error::InvalidAccess { addr: address }),
        }
    }

    fn write8(&mut self, address: u32, value: u8) -> BusResult<()> {
        self.write_bytes(address, &[value])
    }

    fn write16(&mut self, address: u32, value: u16) -> BusResult<()> {
        self.write_bytes(address, &value.to_le_bytes())
    }

    fn write32(&mut self, address: u32, value: u32) -> BusResult<()> {
        self.write_bytes(address, &value.to_le_bytes())
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
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0xCAFE_BABE);

        memory
            .write_u32(physical_base + 0x104, 0x1234_5678)
            .unwrap();
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
