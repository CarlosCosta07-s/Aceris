//! Structural parser for the Nintendo 3DS 0x400-byte NCCH extended header.
//!
//! This exposes the System Control Info code-set metadata and preserves the
//! Access Control Info bytes for later kernel/process-loader work.

use std::fmt;
use std::io::Read;

pub const EXTENDED_HEADER_SIZE: usize = 0x400;
pub const SYSTEM_CONTROL_INFO_SIZE: usize = 0x200;
pub const ACCESS_CONTROL_INFO_SIZE: usize = 0x200;
pub const CODE_PAGE_SIZE: u64 = 0x1000;

const FLAGS_OFFSET: usize = 0x0D;
const REMASTER_VERSION_OFFSET: usize = 0x0E;
const TEXT_CODE_SET_OFFSET: usize = 0x10;
const STACK_SIZE_OFFSET: usize = 0x1C;
const READ_ONLY_CODE_SET_OFFSET: usize = 0x20;
const DATA_CODE_SET_OFFSET: usize = 0x30;
const BSS_SIZE_OFFSET: usize = 0x3C;
const DEPENDENCIES_OFFSET: usize = 0x40;
const DEPENDENCY_COUNT: usize = 48;
const SYSTEM_INFO_OFFSET: usize = 0x1C0;
const SAVE_DATA_SIZE_OFFSET: usize = SYSTEM_INFO_OFFSET;
const JUMP_ID_OFFSET: usize = SYSTEM_INFO_OFFSET + 0x08;
const ACCESS_CONTROL_INFO_OFFSET: usize = SYSTEM_CONTROL_INFO_SIZE;

/// One ARM11 code-set descriptor from the System Control Info.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodeSetInfo {
    /// Virtual address at which the segment is mapped.
    pub address: u32,
    /// Number of 0x1000-byte pages reserved for the segment.
    pub physical_size_pages: u32,
    /// Number of initialized bytes stored in ExeFS.
    pub size_bytes: u32,
}

impl CodeSetInfo {
    /// Returns the reserved physical size in bytes, if it does not overflow.
    pub fn physical_size_bytes(self) -> Option<u64> {
        u64::from(self.physical_size_pages).checked_mul(CODE_PAGE_SIZE)
    }

    /// Returns the exclusive end of the reserved virtual range, if representable.
    pub fn end_address(self) -> Option<u32> {
        self.physical_size_bytes()
            .and_then(|size| u32::try_from(size).ok())
            .and_then(|size| self.address.checked_add(size))
    }
}

/// Parsed System Control Info and raw Access Control Info from an NCCH exheader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NcchExtendedHeader {
    /// Eight-byte application title field.
    pub application_title: [u8; 8],
    /// Raw System Control Info flag byte. Bit 0 marks compressed ExeFS/.code.
    pub flags: u8,
    pub remaster_version: u16,
    pub text_code_set: CodeSetInfo,
    pub stack_size_bytes: u32,
    pub read_only_code_set: CodeSetInfo,
    pub data_code_set: CodeSetInfo,
    pub bss_size_bytes: u32,
    /// Up to 48 dependent title/program IDs; zero entries are unused.
    pub dependencies: [u64; DEPENDENCY_COUNT],
    pub save_data_size_bytes: u64,
    pub jump_id: u64,
    /// Raw 0x200-byte Access Control Info, retained for later validation.
    pub access_control_info: [u8; ACCESS_CONTROL_INFO_SIZE],
}

impl NcchExtendedHeader {
    /// Parses the first 0x400 bytes of an extended header.
    pub fn parse(data: &[u8]) -> Result<Self, ExtendedHeaderError> {
        if data.len() < EXTENDED_HEADER_SIZE {
            return Err(truncated(data));
        }

        let application_title = read_array::<8>(data, 0).ok_or_else(|| truncated(data))?;
        let flags = *data.get(FLAGS_OFFSET).ok_or_else(|| truncated(data))?;
        let remaster_version =
            read_u16_le(data, REMASTER_VERSION_OFFSET).ok_or_else(|| truncated(data))?;
        let text_code_set =
            read_code_set_info(data, TEXT_CODE_SET_OFFSET).ok_or_else(|| truncated(data))?;
        let stack_size_bytes =
            read_u32_le(data, STACK_SIZE_OFFSET).ok_or_else(|| truncated(data))?;
        let read_only_code_set =
            read_code_set_info(data, READ_ONLY_CODE_SET_OFFSET).ok_or_else(|| truncated(data))?;
        let data_code_set =
            read_code_set_info(data, DATA_CODE_SET_OFFSET).ok_or_else(|| truncated(data))?;
        let bss_size_bytes = read_u32_le(data, BSS_SIZE_OFFSET).ok_or_else(|| truncated(data))?;

        let mut dependencies = [0u64; DEPENDENCY_COUNT];
        for (index, dependency) in dependencies.iter_mut().enumerate() {
            *dependency = read_u64_le(data, DEPENDENCIES_OFFSET + index * 8)
                .ok_or_else(|| truncated(data))?;
        }

        let save_data_size_bytes =
            read_u64_le(data, SAVE_DATA_SIZE_OFFSET).ok_or_else(|| truncated(data))?;
        let jump_id = read_u64_le(data, JUMP_ID_OFFSET).ok_or_else(|| truncated(data))?;
        let access_control_info =
            read_array::<ACCESS_CONTROL_INFO_SIZE>(data, ACCESS_CONTROL_INFO_OFFSET)
                .ok_or_else(|| truncated(data))?;

        Ok(Self {
            application_title,
            flags,
            remaster_version,
            text_code_set,
            stack_size_bytes,
            read_only_code_set,
            data_code_set,
            bss_size_bytes,
            dependencies,
            save_data_size_bytes,
            jump_id,
            access_control_info,
        })
    }

    /// Reads exactly 0x400 bytes from the current reader position.
    pub fn parse_reader<R: Read>(reader: &mut R) -> Result<Self, ExtendedHeaderError> {
        let mut data = [0u8; EXTENDED_HEADER_SIZE];
        reader
            .read_exact(&mut data)
            .map_err(|error| ExtendedHeaderError::ReaderFailure(error.to_string()))?;
        Self::parse(&data)
    }

    /// Returns true when the extended header requests ExeFS/.code decompression.
    pub const fn compresses_exefs_code(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Returns true when the title is marked as an SD application.
    pub const fn is_sd_application(&self) -> bool {
        self.flags & 0x02 != 0
    }
}

/// Errors reported while parsing an NCCH extended header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtendedHeaderError {
    HeaderTruncated { actual_size: usize },
    ReaderFailure(String),
}

impl fmt::Display for ExtendedHeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTruncated { actual_size } => write!(
                formatter,
                "NCCH extended header is truncated: {actual_size} bytes available, need at least {EXTENDED_HEADER_SIZE}"
            ),
            Self::ReaderFailure(reason) => {
                write!(formatter, "could not read NCCH extended header: {reason}")
            }
        }
    }
}

impl std::error::Error for ExtendedHeaderError {}

fn truncated(data: &[u8]) -> ExtendedHeaderError {
    ExtendedHeaderError::HeaderTruncated {
        actual_size: data.len(),
    }
}

fn read_code_set_info(data: &[u8], offset: usize) -> Option<CodeSetInfo> {
    Some(CodeSetInfo {
        address: read_u32_le(data, offset)?,
        physical_size_pages: read_u32_le(data, offset + 4)?,
        size_bytes: read_u32_le(data, offset + 8)?,
    })
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

fn read_u16_le(data: &[u8], offset: usize) -> Option<u16> {
    read_array::<2>(data, offset).map(u16::from_le_bytes)
}

fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    read_array::<4>(data, offset).map(u32::from_le_bytes)
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    read_array::<8>(data, offset).map(u64::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn valid_extended_header() -> Vec<u8> {
        let mut data = vec![0; EXTENDED_HEADER_SIZE];
        data[..8].copy_from_slice(b"ACERIS!!");
        data[FLAGS_OFFSET] = 0x03;
        write_u16(&mut data, REMASTER_VERSION_OFFSET, 7);

        write_u32(&mut data, TEXT_CODE_SET_OFFSET, 0x0010_0000);
        write_u32(&mut data, TEXT_CODE_SET_OFFSET + 4, 2);
        write_u32(&mut data, TEXT_CODE_SET_OFFSET + 8, 0x1800);
        write_u32(&mut data, STACK_SIZE_OFFSET, 0x4000);

        write_u32(&mut data, READ_ONLY_CODE_SET_OFFSET, 0x0010_3000);
        write_u32(&mut data, READ_ONLY_CODE_SET_OFFSET + 4, 1);
        write_u32(&mut data, READ_ONLY_CODE_SET_OFFSET + 8, 0x800);

        write_u32(&mut data, DATA_CODE_SET_OFFSET, 0x0010_4000);
        write_u32(&mut data, DATA_CODE_SET_OFFSET + 4, 3);
        write_u32(&mut data, DATA_CODE_SET_OFFSET + 8, 0x1000);
        write_u32(&mut data, BSS_SIZE_OFFSET, 0x2000);

        write_u64(&mut data, DEPENDENCIES_OFFSET, 0x0004_0000_1234_5678);
        write_u64(&mut data, SAVE_DATA_SIZE_OFFSET, 0x20000);
        write_u64(&mut data, JUMP_ID_OFFSET, 0x0004_0000_8765_4321);
        data[ACCESS_CONTROL_INFO_OFFSET..].fill(0xA5);
        data
    }

    #[test]
    fn parses_system_control_and_access_control_info() {
        let header = NcchExtendedHeader::parse(&valid_extended_header()).unwrap();

        assert_eq!(&header.application_title, b"ACERIS!!");
        assert!(header.compresses_exefs_code());
        assert!(header.is_sd_application());
        assert_eq!(header.remaster_version, 7);
        assert_eq!(
            header.text_code_set,
            CodeSetInfo {
                address: 0x0010_0000,
                physical_size_pages: 2,
                size_bytes: 0x1800,
            }
        );
        assert_eq!(header.text_code_set.physical_size_bytes(), Some(0x2000));
        assert_eq!(header.text_code_set.end_address(), Some(0x0010_2000));
        assert_eq!(header.stack_size_bytes, 0x4000);
        assert_eq!(header.read_only_code_set.size_bytes, 0x800);
        assert_eq!(header.data_code_set.address, 0x0010_4000);
        assert_eq!(header.bss_size_bytes, 0x2000);
        assert_eq!(header.dependencies[0], 0x0004_0000_1234_5678);
        assert_eq!(header.save_data_size_bytes, 0x20000);
        assert_eq!(header.jump_id, 0x0004_0000_8765_4321);
        assert!(header.access_control_info.iter().all(|byte| *byte == 0xA5));
    }

    #[test]
    fn rejects_a_truncated_extended_header() {
        assert_eq!(
            NcchExtendedHeader::parse(&[0; EXTENDED_HEADER_SIZE - 1]),
            Err(ExtendedHeaderError::HeaderTruncated {
                actual_size: EXTENDED_HEADER_SIZE - 1,
            })
        );
    }

    #[test]
    fn parses_extended_header_from_a_reader() {
        let data = valid_extended_header();
        let parsed = NcchExtendedHeader::parse_reader(&mut std::io::Cursor::new(data)).unwrap();
        assert!(parsed.compresses_exefs_code());
    }
}
