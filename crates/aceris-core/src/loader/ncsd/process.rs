//! ARM11 process-layout validation for prepared Nintendo 3DS executable images.
//!
//! This module converts parsed code-set metadata into validated virtual
//! mappings. It deliberately does not choose a kernel-owned stack address,
//! allocate physical memory, or start the CPU; those are later process/kernel
//! responsibilities.

use std::fmt;

use super::PreparedArm11Image;
use super::ncch::extended_header::{CodeFileLayout, CodeSetInfo};
use crate::memory::Bus;
use aceris_common::{Error, Result as BusResult};

const PAGE_SIZE: u64 = 0x1000;
const ADDRESS_SPACE_END: u64 = 0x1_0000_0000;
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
}

impl fmt::Display for Arm11SegmentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => formatter.write_str("text"),
            Self::ReadOnly => formatter.write_str("read-only"),
            Self::Data => formatter.write_str("data"),
        }
    }
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
    /// Exact main-thread stack request in bytes. The kernel chooses its address.
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

        validate_no_overlap([
            (Arm11SegmentKind::Text, text),
            (Arm11SegmentKind::ReadOnly, read_only),
            (Arm11SegmentKind::Data, data),
            (Arm11SegmentKind::Bss, bss),
        ])?;

        Ok(Arm11ProcessLayout {
            application_title: extended_header.application_title,
            entry_point: text.address,
            text,
            read_only,
            data,
            bss,
            bss_size_bytes: extended_header.bss_size_bytes,
            stack_size_bytes: extended_header.stack_size_bytes,
            code_file_layout,
        })
    }
}

/// A bootstrap ARM11 address space containing the prepared code segments.
///
/// This bus maps text, read-only data, initialized writable data, and the
/// zero-filled BSS tail. The main-thread stack, shared memory, and hardware
/// registers are not mapped until their kernel and device-memory rules exist.
#[derive(Debug)]
pub struct Arm11CodeAddressSpace {
    layout: Arm11ProcessLayout,
    text: Vec<u8>,
    read_only: Vec<u8>,
    data: Vec<u8>,
    bss: Vec<u8>,
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

        Ok(Self {
            layout,
            text: prepared.text,
            read_only: prepared.read_only,
            data: prepared.data,
            bss,
        })
    }

    /// Returns the validated process metadata associated with this address space.
    pub const fn layout(&self) -> &Arm11ProcessLayout {
        &self.layout
    }

    /// Returns the backing bytes for one mapped code segment.
    pub fn segment_bytes(&self, kind: Arm11SegmentKind) -> &[u8] {
        self.bytes_for(kind)
    }

    fn locate(&self, address: u32) -> Option<(Arm11SegmentKind, usize)> {
        let segments = [
            (Arm11SegmentKind::Text, self.layout.text),
            (Arm11SegmentKind::ReadOnly, self.layout.read_only),
            (Arm11SegmentKind::Data, self.layout.data),
            (Arm11SegmentKind::Bss, self.layout.bss),
        ];

        for (kind, segment) in segments {
            if segment.is_empty() || address < segment.address {
                continue;
            }

            let offset = u64::from(address) - u64::from(segment.address);
            if offset < u64::from(segment.mapped_size_bytes) {
                return usize::try_from(offset).ok().map(|offset| (kind, offset));
            }
        }

        None
    }

    fn bytes_for(&self, kind: Arm11SegmentKind) -> &[u8] {
        match kind {
            Arm11SegmentKind::Text => &self.text,
            Arm11SegmentKind::ReadOnly => &self.read_only,
            Arm11SegmentKind::Data => &self.data,
            Arm11SegmentKind::Bss => &self.bss,
        }
    }

    fn bytes_for_mut(&mut self, kind: Arm11SegmentKind) -> &mut [u8] {
        match kind {
            Arm11SegmentKind::Text => &mut self.text,
            Arm11SegmentKind::ReadOnly => &mut self.read_only,
            Arm11SegmentKind::Data => &mut self.data,
            Arm11SegmentKind::Bss => &mut self.bss,
        }
    }

    fn segment_is_writable(&self, kind: Arm11SegmentKind) -> bool {
        match kind {
            Arm11SegmentKind::Text => self.layout.text.writable,
            Arm11SegmentKind::ReadOnly => self.layout.read_only.writable,
            Arm11SegmentKind::Data => self.layout.data.writable,
            Arm11SegmentKind::Bss => self.layout.bss.writable,
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
            let (kind, segment_offset) =
                self.locate(current_address).ok_or(Error::InvalidAccess {
                    addr: current_address,
                })?;

            if !self.segment_is_writable(kind) {
                return Err(Error::InvalidAccess {
                    addr: current_address,
                });
            }

            locations.push((kind, segment_offset));
        }

        for (byte, (kind, segment_offset)) in bytes.iter().zip(locations) {
            self.bytes_for_mut(kind)[segment_offset] = *byte;
        }

        Ok(())
    }
}

impl Bus for Arm11CodeAddressSpace {
    fn read8(&mut self, address: u32) -> BusResult<u8> {
        let (kind, offset) = self
            .locate(address)
            .ok_or(Error::InvalidAccess { addr: address })?;
        self.bytes_for(kind)
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
}

fn make_bss_segment(
    data_code_set: CodeSetInfo,
    initialized_data_size: usize,
    total_data_memory_size: usize,
) -> std::result::Result<Arm11MemorySegment, Arm11ProcessLayoutError> {
    let bss_size = total_data_memory_size
        .checked_sub(initialized_data_size)
        .ok_or(Arm11ProcessLayoutError::InvalidCodeFileLayout)?;
    let mapped_size_bytes = u32::try_from(bss_size)
        .map_err(|_| Arm11ProcessLayoutError::InvalidCodeFileLayout)?;

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
    segments: [(Arm11SegmentKind, Arm11MemorySegment); 4],
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
        cpu.set_pc(address_space.layout().entry_point);

        assert_eq!(
            cpu.step(&mut address_space).unwrap(),
            crate::cpu::StepEvent::Executed
        );
        assert_eq!(cpu.regs[0], 7);
        assert_eq!(cpu.pc(), 0x0010_0004);
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

        assert!(address_space
            .segment_bytes(Arm11SegmentKind::Bss)
            .iter()
            .all(|byte| *byte == 0));
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
        prepared.extended_header.bss_size_bytes = 0x1010_0000;
        prepared.extended_header.data_code_set.physical_size_pages = 0x1011;

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
    fn address_space_rejects_unmapped_addresses() {
        let mut address_space = Arm11CodeAddressSpace::from_prepared(prepared_image()).unwrap();

        assert!(matches!(
            address_space.read8(0x000F_FFFF),
            Err(crate::Error::InvalidAccess { addr: 0x000F_FFFF })
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
