//! Structural parser for the public CIA (CTR Importable Archive) container.
//!
//! This module validates the fixed header and the byte ranges of its sections.
//! It does not parse tickets or title metadata, verify signatures, decrypt
//! content, or claim that any contained program is executable.

use std::io;

/// Streaming access to CIA content sections.
pub mod archive;
/// Structural TMD parser used by CIA archives.
pub mod tmd;

pub use archive::{
    CiaArchiveError, CiaArchiveReader, CiaContentLocation, CiaContentNcchReader, CiaContentReader,
};

pub const HEADER_SIZE: usize = 0x2020;
pub const CONTENT_INDEX_OFFSET: usize = 0x20;
pub const CONTENT_INDEX_SIZE: usize = 0x2000;
const SECTION_ALIGNMENT: u64 = 0x40;

/// A section in a CIA archive, expressed as a byte range from the file start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiaSection {
    pub offset_bytes: u64,
    pub size_bytes: u64,
}

impl CiaSection {
    /// Returns the exclusive end of this section.
    pub fn end_offset_bytes(self) -> Option<u64> {
        self.offset_bytes.checked_add(self.size_bytes)
    }
}

/// Validated structural information from a CIA header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiaHeader {
    pub header_size_bytes: u32,
    pub archive_type: u16,
    pub format_version: u16,
    pub certificate_chain: CiaSection,
    pub ticket: CiaSection,
    pub title_metadata: CiaSection,
    pub content: CiaSection,
    /// Absent when the header declares no metadata section.
    pub meta: Option<CiaSection>,
    content_index_bitmap: [u8; CONTENT_INDEX_SIZE],
}

impl CiaHeader {
    /// Parses a complete fixed CIA header and validates every declared section
    /// against the actual file length.
    ///
    /// The CIA header is little-endian. Its sections begin at 64-byte
    /// boundaries. When the optional metadata section is absent, the archive
    /// may end immediately after the content section without trailing padding.
    pub fn parse(data: &[u8], file_len: u64) -> io::Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(invalid_data(
                "CIA header is shorter than the fixed 0x2020-byte header",
            ));
        }
        if file_len < HEADER_SIZE as u64 {
            return Err(invalid_data("CIA file is shorter than its fixed header"));
        }

        let header_size_bytes = read_u32(data, 0x00)?;
        if header_size_bytes < HEADER_SIZE as u32 {
            return Err(invalid_data(
                "CIA header-size field is smaller than 0x2020 bytes",
            ));
        }
        if u64::from(header_size_bytes) > file_len {
            return Err(invalid_data(
                "CIA header-size field extends beyond the file",
            ));
        }

        let archive_type = read_u16(data, 0x04)?;
        let format_version = read_u16(data, 0x06)?;
        let certificate_size = u64::from(read_u32(data, 0x08)?);
        let ticket_size = u64::from(read_u32(data, 0x0C)?);
        let title_metadata_size = u64::from(read_u32(data, 0x10)?);
        let meta_size = u64::from(read_u32(data, 0x14)?);
        let content_size = read_u64(data, 0x18)?;

        let certificate_chain = section_after(
            u64::from(header_size_bytes),
            certificate_size,
            file_len,
            "certificate chain",
        )?;
        let ticket = section_after(
            certificate_chain
                .end_offset_bytes()
                .ok_or_else(|| invalid_data("CIA certificate-chain range overflows"))?,
            ticket_size,
            file_len,
            "ticket",
        )?;
        let title_metadata = section_after(
            ticket
                .end_offset_bytes()
                .ok_or_else(|| invalid_data("CIA ticket range overflows"))?,
            title_metadata_size,
            file_len,
            "title metadata",
        )?;
        let content = section_after(
            title_metadata
                .end_offset_bytes()
                .ok_or_else(|| invalid_data("CIA title-metadata range overflows"))?,
            content_size,
            file_len,
            "content",
        )?;

        let meta = if meta_size == 0 {
            None
        } else {
            Some(section_after(
                content
                    .end_offset_bytes()
                    .ok_or_else(|| invalid_data("CIA content range overflows"))?,
                meta_size,
                file_len,
                "metadata",
            )?)
        };

        let content_index_bitmap = data
            [CONTENT_INDEX_OFFSET..CONTENT_INDEX_OFFSET + CONTENT_INDEX_SIZE]
            .try_into()
            .expect("the fixed header length was checked");

        Ok(Self {
            header_size_bytes,
            archive_type,
            format_version,
            certificate_chain,
            ticket,
            title_metadata,
            content,
            meta,
            content_index_bitmap,
        })
    }

    /// Returns the raw 0x2000-byte content-index bitmap.
    pub const fn content_index_bitmap(&self) -> &[u8; CONTENT_INDEX_SIZE] {
        &self.content_index_bitmap
    }

    /// Returns whether a content index is selected by the CIA bitmap.
    ///
    /// Index zero is the most significant bit of the first bitmap byte.
    pub const fn content_index_is_present(&self, content_index: u16) -> bool {
        let index = content_index as usize;
        let byte = self.content_index_bitmap[index / 8];
        let mask = 0x80 >> (index % 8);
        byte & mask != 0
    }

    /// Parses the TMD section and checks that every CIA-selected content index
    /// has a corresponding TMD content record.
    pub fn parse_title_metadata(&self, data: &[u8]) -> Result<tmd::TmdHeader, tmd::TmdError> {
        let tmd = tmd::TmdHeader::parse(data)?;
        tmd.validate_cia_content_index_bitmap(&self.content_index_bitmap)?;
        Ok(tmd)
    }

    /// Counts set bits in the content-index bitmap.
    pub fn present_content_count(&self) -> u32 {
        self.content_index_bitmap
            .iter()
            .map(|byte| byte.count_ones())
            .sum()
    }
}

fn section_after(
    previous_end: u64,
    size_bytes: u64,
    file_len: u64,
    section_name: &'static str,
) -> io::Result<CiaSection> {
    let offset_bytes = align_section(previous_end).ok_or_else(|| {
        invalid_data(format!(
            "CIA {section_name} offset overflows the address range"
        ))
    })?;
    let end = offset_bytes.checked_add(size_bytes).ok_or_else(|| {
        invalid_data(format!(
            "CIA {section_name} range overflows the address range"
        ))
    })?;

    if end > file_len {
        return Err(invalid_data(format!(
            "CIA {section_name} section ends at {end:#x}, beyond file length {file_len:#x}"
        )));
    }

    Ok(CiaSection {
        offset_bytes,
        size_bytes,
    })
}

fn align_section(offset: u64) -> Option<u64> {
    offset
        .checked_add(SECTION_ALIGNMENT - 1)
        .map(|aligned| aligned & !(SECTION_ALIGNMENT - 1))
}

fn read_u16(data: &[u8], offset: usize) -> io::Result<u16> {
    let bytes = data
        .get(offset..offset + 2)
        .ok_or_else(|| invalid_data("CIA header is missing a 16-bit field"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> io::Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .ok_or_else(|| invalid_data("CIA header is missing a 32-bit field"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> io::Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .ok_or_else(|| invalid_data("CIA header is missing a 64-bit field"))?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_header() -> Vec<u8> {
        let mut data = vec![0; HEADER_SIZE];
        write_u32(&mut data, 0x00, HEADER_SIZE as u32);
        data
    }

    fn write_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn parses_section_offsets_and_sizes_with_64_byte_alignment() {
        let mut data = empty_header();
        write_u16(&mut data, 0x04, 0x1234);
        write_u16(&mut data, 0x06, 2);
        write_u32(&mut data, 0x08, 1);
        write_u32(&mut data, 0x0C, 2);
        write_u32(&mut data, 0x10, 3);
        write_u32(&mut data, 0x14, 0x400);
        write_u64(&mut data, 0x18, 5);
        data[CONTENT_INDEX_OFFSET] = 0x80;
        data[CONTENT_INDEX_OFFSET + 1] = 0x03;

        let header = CiaHeader::parse(&data, 0x2540).unwrap();

        assert_eq!(header.header_size_bytes, HEADER_SIZE as u32);
        assert_eq!(header.archive_type, 0x1234);
        assert_eq!(header.format_version, 2);
        assert_eq!(
            header.certificate_chain,
            CiaSection {
                offset_bytes: 0x2040,
                size_bytes: 1
            }
        );
        assert_eq!(
            header.ticket,
            CiaSection {
                offset_bytes: 0x2080,
                size_bytes: 2
            }
        );
        assert_eq!(
            header.title_metadata,
            CiaSection {
                offset_bytes: 0x20C0,
                size_bytes: 3
            }
        );
        assert_eq!(
            header.content,
            CiaSection {
                offset_bytes: 0x2100,
                size_bytes: 5
            }
        );
        assert_eq!(
            header.meta,
            Some(CiaSection {
                offset_bytes: 0x2140,
                size_bytes: 0x400
            })
        );
        assert_eq!(header.present_content_count(), 3);
        assert_eq!(header.content_index_bitmap()[0], 0x80);
    }

    #[test]
    fn accepts_archives_without_the_optional_metadata_section() {
        let mut data = empty_header();
        write_u64(&mut data, 0x18, 0x10);

        let header = CiaHeader::parse(&data, 0x2050).unwrap();

        assert_eq!(
            header.content,
            CiaSection {
                offset_bytes: 0x2040,
                size_bytes: 0x10
            }
        );
        assert_eq!(header.meta, None);
    }

    #[test]
    fn rejects_a_truncated_fixed_header() {
        let data = vec![0; HEADER_SIZE - 1];
        let error = CiaHeader::parse(&data, HEADER_SIZE as u64).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_a_header_size_smaller_than_the_fixed_header() {
        let mut data = empty_header();
        write_u32(&mut data, 0x00, 0x2000);

        let error = CiaHeader::parse(&data, 0x3000).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_sections_that_extend_past_the_file() {
        let mut data = empty_header();
        write_u32(&mut data, 0x08, 1);

        let error = CiaHeader::parse(&data, 0x2040).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_overflowing_content_ranges() {
        let mut data = empty_header();
        write_u64(&mut data, 0x18, u64::MAX);

        let error = CiaHeader::parse(&data, u64::MAX).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
