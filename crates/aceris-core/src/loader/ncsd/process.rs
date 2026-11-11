//! ARM11 process-layout validation for prepared Nintendo 3DS executable images.
//!
//! This module converts parsed code-set metadata into validated virtual
//! mappings for code, BSS, and the main-thread stack. It does not allocate
//! physical memory, configure privileged kernel state, schedule threads, or
//! start a complete console execution environment.

use std::collections::BTreeMap;
use std::fmt;

use super::PreparedArm11Image;
use super::ncch::extended_header::{CodeFileLayout, CodeSetInfo};
use crate::cpu::{Arm11Core, SP};
use crate::memory::Bus;
use aceris_common::{Error, Result as BusResult};

const PAGE_SIZE: u64 = 0x1000;
const ADDRESS_SPACE_END: u64 = 0x1_0000_0000;
const MAIN_STACK_TOP: u32 = 0x1000_0000;
const APPLICATION_HEAP_BASE: u32 = 0x0800_0000;
const APPLICATION_HEAP_END: u64 = 0x0E00_0000;
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
        }
    }
}

impl std::error::Error for Arm11DynamicMemoryError {}

#[derive(Debug)]
struct DynamicMemoryRegion {
    address: u32,
    bytes: Vec<u8>,
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
    text: Vec<u8>,
    read_only: Vec<u8>,
    data: Vec<u8>,
    bss: Vec<u8>,
    stack: Vec<u8>,
    dynamic_regions: Vec<DynamicMemoryRegion>,
    /// Per-eight-byte exclusive monitor generations for mapped virtual memory.
    exclusive_versions: BTreeMap<u32, u64>,
}

impl Arm11CodeAddressSpace {
    /// Creates a segmented address space from a verified prepared executable.
    pub fn from_prepared(
        prepared: PreparedArm11Image,
    ) -> std::result::Result<Self, Arm11ProcessLayoutError> {
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
            text: prepared.text,
            read_only: prepared.read_only,
            data: prepared.data,
            bss,
            stack,
            dynamic_regions: Vec::new(),
            exclusive_versions: BTreeMap::new(),
        })
    }

    /// Returns the validated process metadata associated with this address space.
    pub const fn layout(&self) -> &Arm11ProcessLayout {
        &self.layout
    }

    /// Initializes the ARM11 core's entry PC and main-thread stack pointer.
    ///
    /// This does not configure privileged CPU modes, exception vectors,
    /// coprocessors, scheduler state, or peripheral registers.
    pub fn initialize_core(&self, core: &mut Arm11Core) {
        core.set_pc(self.layout.entry_point);
        core.regs[SP] = self.layout.initial_stack_pointer;
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
            let region_end = u64::from(region.address) + region.bytes.len() as u64;
            u64::from(address) < region_end && end > u64::from(region.address)
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
        self.dynamic_regions
            .push(DynamicMemoryRegion { address, bytes });
        self.dynamic_regions.sort_by_key(|region| region.address);

        Ok(address)
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
        let Some(index) = self
            .dynamic_regions
            .iter()
            .position(|region| region.address == address && region.bytes.len() == size as usize)
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

        for region in &self.dynamic_regions {
            regions.push(Arm11MemoryRegionInfo {
                base_address: region.address,
                size_bytes: u32::try_from(region.bytes.len())
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

        self.dynamic_regions
            .iter()
            .enumerate()
            .find_map(|(index, region)| {
                if address < region.address {
                    return None;
                }
                let offset = u64::from(address) - u64::from(region.address);
                (offset < region.bytes.len() as u64).then(|| {
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
            MemoryLocation::Dynamic { index, .. } => &self.dynamic_regions[index].bytes,
        }
    }

    fn bytes_for_location_mut(&mut self, location: MemoryLocation) -> &mut [u8] {
        match location {
            MemoryLocation::Static { kind, .. } => self.bytes_for_mut(kind),
            MemoryLocation::Dynamic { index, .. } => &mut self.dynamic_regions[index].bytes,
        }
    }

    fn location_is_writable(&self, location: MemoryLocation) -> bool {
        match location {
            MemoryLocation::Static { kind, .. } => self.segment_is_writable(kind),
            MemoryLocation::Dynamic { .. } => true,
        }
    }

    fn location_is_executable(&self, location: MemoryLocation) -> bool {
        match location {
            MemoryLocation::Static { kind, .. } => self.segment_is_executable(kind),
            MemoryLocation::Dynamic { .. } => false,
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
                Some(u64::from(region.address) + region.bytes.len() as u64)
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
            self.bytes_for_location_mut(location)[match location {
                MemoryLocation::Static { offset, .. } | MemoryLocation::Dynamic { offset, .. } => {
                    offset
                }
            }] = *byte;
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
        let offset = match location {
            MemoryLocation::Static { offset, .. } | MemoryLocation::Dynamic { offset, .. } => {
                offset
            }
        };
        self.bytes_for_location(location)
            .get(offset)
            .copied()
            .ok_or(Error::InvalidAccess { addr: address })
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
