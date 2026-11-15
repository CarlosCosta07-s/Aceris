//! Structural parser for Nintendo 3DS homebrew executable files.
//!
//! This parser validates the standard 3DSX container layout. It does not
//! relocate segments, create an ARM11 process, or claim executable support.

use std::io::{self, Read, Seek, SeekFrom};

pub const HEADER_SIZE: usize = 0x20;
const EXTENDED_HEADER_SIZE: usize = 0x2C;
const STANDARD_RELOCATION_HEADER_SIZE: u16 = 8;
const SEGMENT_COUNT: usize = 3;
const WORD_SIZE: u64 = 4;

/// Fixed fields from a 3DSX header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxHeader {
    pub header_size_bytes: u16,
    pub relocation_header_size_bytes: u16,
    pub format_version: u32,
    pub flags: u32,
    pub code_size_bytes: u32,
    pub rodata_size_bytes: u32,
    pub data_size_bytes: u32,
    pub bss_size_bytes: u32,
}

/// Optional offsets declared by an extended 3DSX header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxExtendedHeader {
    pub smdh_offset_bytes: u32,
    pub smdh_size_bytes: u32,
    pub romfs_offset_bytes: u32,
}

/// File-backed and in-memory sizes for one loaded segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxSegment {
    pub offset_bytes: u64,
    pub file_size_bytes: u64,
    pub memory_size_bytes: u64,
}

/// Counts of the two standard relocation tables for one segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxRelocationHeader {
    pub absolute_entries: u32,
    pub relative_entries: u32,
}

/// One bounded relocation table in the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxRelocationTable {
    pub offset_bytes: u64,
    pub entry_count: u32,
}

impl ThreeDsxRelocationTable {
    pub const fn size_bytes(self) -> u64 {
        self.entry_count as u64 * WORD_SIZE
    }
}

/// Standard absolute and relative relocation tables for one segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxSegmentRelocations {
    pub absolute: ThreeDsxRelocationTable,
    pub relative: ThreeDsxRelocationTable,
}

/// Structurally validated 3DSX layout.
///
/// The code segment starts at the executable entry point. The data segment's
/// memory size includes BSS, while its file size excludes the uninitialized
/// BSS tail. Relocation records are described but not applied by this parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsxImage {
    pub header: ThreeDsxHeader,
    pub extended_header: Option<ThreeDsxExtendedHeader>,
    pub relocation_headers: [ThreeDsxRelocationHeader; SEGMENT_COUNT],
    pub code: ThreeDsxSegment,
    pub rodata: ThreeDsxSegment,
    pub data: ThreeDsxSegment,
    pub relocations: [ThreeDsxSegmentRelocations; SEGMENT_COUNT],
    pub trailing_data_offset_bytes: u64,
}

impl ThreeDsxImage {
    /// Parses and bounds-checks a 3DSX file from a seekable reader.
    ///
    /// The currently supported relocation-header layout is the standard pair
    /// of u32 counts (8 bytes) for each of the three segments. Unknown extended
    /// header bytes are skipped, but partial extended headers are rejected.
    pub fn parse_reader<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;

        let mut fixed = [0u8; HEADER_SIZE];
        reader.read_exact(&mut fixed)?;
        if &fixed[..4] != b"3DSX" {
            return Err(invalid_data("3DSX magic is missing"));
        }

        let header = ThreeDsxHeader {
            header_size_bytes: read_u16(&fixed, 0x04),
            relocation_header_size_bytes: read_u16(&fixed, 0x06),
            format_version: read_u32(&fixed, 0x08),
            flags: read_u32(&fixed, 0x0C),
            code_size_bytes: read_u32(&fixed, 0x10),
            rodata_size_bytes: read_u32(&fixed, 0x14),
            data_size_bytes: read_u32(&fixed, 0x18),
            bss_size_bytes: read_u32(&fixed, 0x1C),
        };

        let header_size = u64::from(header.header_size_bytes);
        if header_size < HEADER_SIZE as u64 {
            return Err(invalid_data("3DSX header is shorter than 0x20 bytes"));
        }
        if header_size > file_len {
            return Err(invalid_data("3DSX header extends beyond the file"));
        }
        if header_size > HEADER_SIZE as u64 && header_size < EXTENDED_HEADER_SIZE as u64 {
            return Err(invalid_data(
                "3DSX extended header is shorter than its 12-byte field block",
            ));
        }
        if header.relocation_header_size_bytes != STANDARD_RELOCATION_HEADER_SIZE {
            return Err(invalid_data(
                "3DSX relocation headers must use the standard 8-byte layout",
            ));
        }
        if header.code_size_bytes == 0 {
            return Err(invalid_data("3DSX code segment is empty"));
        }
        if header.bss_size_bytes > header.data_size_bytes {
            return Err(invalid_data("3DSX BSS size exceeds the data segment size"));
        }

        let extended_header = if header_size >= EXTENDED_HEADER_SIZE as u64 {
            reader.seek(SeekFrom::Start(HEADER_SIZE as u64))?;
            let mut bytes = [0u8; EXTENDED_HEADER_SIZE - HEADER_SIZE];
            reader.read_exact(&mut bytes)?;
            Some(ThreeDsxExtendedHeader {
                smdh_offset_bytes: read_u32(&bytes, 0),
                smdh_size_bytes: read_u32(&bytes, 4),
                romfs_offset_bytes: read_u32(&bytes, 8),
            })
        } else {
            None
        };

        let relocation_headers_start = header_size;
        let relocation_headers_end = checked_file_end(
            relocation_headers_start,
            (SEGMENT_COUNT as u64)
                .checked_mul(u64::from(header.relocation_header_size_bytes))
                .ok_or_else(|| invalid_data("3DSX relocation-header range overflows"))?,
            file_len,
            "relocation headers",
        )?;

        reader.seek(SeekFrom::Start(relocation_headers_start))?;
        let mut relocation_headers = [ThreeDsxRelocationHeader::default(); SEGMENT_COUNT];
        for entry in &mut relocation_headers {
            let mut bytes = [0u8; STANDARD_RELOCATION_HEADER_SIZE as usize];
            reader.read_exact(&mut bytes)?;
            entry.absolute_entries = read_u32(&bytes, 0);
            entry.relative_entries = read_u32(&bytes, 4);
        }

        let code_file_size = u64::from(header.code_size_bytes);
        let rodata_file_size = u64::from(header.rodata_size_bytes);
        let data_file_size = u64::from(header.data_size_bytes - header.bss_size_bytes);
        let code_end = checked_file_end(
            relocation_headers_end,
            code_file_size,
            file_len,
            "code segment",
        )?;
        let rodata_end = checked_file_end(code_end, rodata_file_size, file_len, "rodata segment")?;
        let data_end = checked_file_end(rodata_end, data_file_size, file_len, "data segment")?;

        let code = ThreeDsxSegment {
            offset_bytes: relocation_headers_end,
            file_size_bytes: code_file_size,
            memory_size_bytes: code_file_size,
        };
        let rodata = ThreeDsxSegment {
            offset_bytes: code_end,
            file_size_bytes: rodata_file_size,
            memory_size_bytes: rodata_file_size,
        };
        let data = ThreeDsxSegment {
            offset_bytes: rodata_end,
            file_size_bytes: data_file_size,
            memory_size_bytes: u64::from(header.data_size_bytes),
        };

        let mut relocation_cursor = data_end;
        let mut relocations = [ThreeDsxSegmentRelocations::default(); SEGMENT_COUNT];
        for (index, counts) in relocation_headers.iter().enumerate() {
            let absolute_size = u64::from(counts.absolute_entries)
                .checked_mul(WORD_SIZE)
                .ok_or_else(|| invalid_data("3DSX absolute relocation table overflows"))?;
            let absolute_end = checked_file_end(
                relocation_cursor,
                absolute_size,
                file_len,
                "absolute relocation table",
            )?;
            let relative_size = u64::from(counts.relative_entries)
                .checked_mul(WORD_SIZE)
                .ok_or_else(|| invalid_data("3DSX relative relocation table overflows"))?;
            let relative_end = checked_file_end(
                absolute_end,
                relative_size,
                file_len,
                "relative relocation table",
            )?;

            relocations[index] = ThreeDsxSegmentRelocations {
                absolute: ThreeDsxRelocationTable {
                    offset_bytes: relocation_cursor,
                    entry_count: counts.absolute_entries,
                },
                relative: ThreeDsxRelocationTable {
                    offset_bytes: absolute_end,
                    entry_count: counts.relative_entries,
                },
            };
            relocation_cursor = relative_end;
        }

        validate_extended_sections(extended_header, relocation_cursor, file_len)?;

        Ok(Self {
            header,
            extended_header,
            relocation_headers,
            code,
            rodata,
            data,
            relocations,
            trailing_data_offset_bytes: relocation_cursor,
        })
    }
}

fn validate_extended_sections(
    extended: Option<ThreeDsxExtendedHeader>,
    minimum_data_offset: u64,
    file_len: u64,
) -> io::Result<()> {
    let Some(extended) = extended else {
        return Ok(());
    };

    let smdh_offset = u64::from(extended.smdh_offset_bytes);
    let smdh_size = u64::from(extended.smdh_size_bytes);
    if smdh_size == 0 {
        if smdh_offset != 0 {
            return Err(invalid_data(
                "3DSX SMDH offset is set while its size is zero",
            ));
        }
    } else {
        if smdh_offset < minimum_data_offset {
            return Err(invalid_data(
                "3DSX SMDH data overlaps executable or relocation data",
            ));
        }
        checked_file_end(smdh_offset, smdh_size, file_len, "SMDH data")?;
    }

    let romfs_offset = u64::from(extended.romfs_offset_bytes);
    if romfs_offset != 0 {
        if romfs_offset < minimum_data_offset {
            return Err(invalid_data(
                "3DSX RomFS offset overlaps executable or relocation data",
            ));
        }
        if romfs_offset > file_len {
            return Err(invalid_data("3DSX RomFS offset extends beyond the file"));
        }
        if smdh_size != 0 {
            let smdh_end = smdh_offset
                .checked_add(smdh_size)
                .ok_or_else(|| invalid_data("3DSX SMDH range overflows"))?;
            if romfs_offset < smdh_end {
                return Err(invalid_data("3DSX RomFS overlaps SMDH data"));
            }
        }
    }

    Ok(())
}

fn checked_file_end(
    start: u64,
    size: u64,
    file_len: u64,
    section_name: &'static str,
) -> io::Result<u64> {
    let end = start
        .checked_add(size)
        .ok_or_else(|| invalid_data(format!("3DSX {section_name} range overflows")))?;
    if end > file_len {
        return Err(invalid_data(format!(
            "3DSX {section_name} ends at {end:#x}, beyond file length {file_len:#x}"
        )));
    }
    Ok(end)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn minimal_image(header_size: u16, smdh: Option<(u32, u32, u32)>) -> Vec<u8> {
        let mut image = vec![0u8; usize::from(header_size)];
        image[..4].copy_from_slice(b"3DSX");
        put_u16(&mut image, 4, header_size);
        put_u16(&mut image, 6, STANDARD_RELOCATION_HEADER_SIZE);
        put_u32(&mut image, 8, 0);
        put_u32(&mut image, 12, 0);
        put_u32(&mut image, 16, 4);
        put_u32(&mut image, 20, 4);
        put_u32(&mut image, 24, 8);
        put_u32(&mut image, 28, 4);

        if let Some((smdh_offset, smdh_size, romfs_offset)) = smdh {
            put_u32(&mut image, 32, smdh_offset);
            put_u32(&mut image, 36, smdh_size);
            put_u32(&mut image, 40, romfs_offset);
        }

        // Code has one absolute relocation; rodata has one relative relocation.
        image.extend_from_slice(&1u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&1u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());

        image.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        image.extend_from_slice(&[0x55, 0x66, 0x77, 0x88]);
        image.extend_from_slice(&[0x99, 0xAA, 0xBB, 0xCC]);
        image.extend_from_slice(&[0, 0, 0, 0]); // code absolute relocation
        image.extend_from_slice(&[0, 0, 0, 0]); // rodata relative relocation
        image
    }

    #[test]
    fn parses_standard_segments_bss_and_relocation_ranges() {
        let image = minimal_image(HEADER_SIZE as u16, None);
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();

        assert_eq!(parsed.header.format_version, 0);
        assert_eq!(parsed.code.offset_bytes, 56);
        assert_eq!(parsed.code.file_size_bytes, 4);
        assert_eq!(parsed.rodata.offset_bytes, 60);
        assert_eq!(parsed.data.offset_bytes, 64);
        assert_eq!(parsed.data.file_size_bytes, 4);
        assert_eq!(parsed.data.memory_size_bytes, 8);
        assert_eq!(parsed.relocations[0].absolute.offset_bytes, 68);
        assert_eq!(parsed.relocations[0].absolute.entry_count, 1);
        assert_eq!(parsed.relocations[1].relative.offset_bytes, 72);
        assert_eq!(parsed.relocations[1].relative.entry_count, 1);
        assert_eq!(parsed.trailing_data_offset_bytes, 76);
    }

    #[test]
    fn parses_extended_header_and_bounds_optional_sections() {
        let mut image = minimal_image(EXTENDED_HEADER_SIZE as u16, Some((88, 4, 92)));
        image.extend_from_slice(&[1, 2, 3, 4]); // SMDH
        image.extend_from_slice(&[5, 6, 7, 8]); // RomFS

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        assert_eq!(
            parsed.extended_header,
            Some(ThreeDsxExtendedHeader {
                smdh_offset_bytes: 88,
                smdh_size_bytes: 4,
                romfs_offset_bytes: 92,
            })
        );
    }

    #[test]
    fn rejects_partial_extended_header() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u16(&mut image, 4, 40);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn rejects_bss_larger_than_the_data_segment() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u32(&mut image, 28, 9);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn rejects_truncated_segment_or_relocation_data() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        image.truncate(image.len() - 1);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());

        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u32(&mut image, 32, u32::MAX);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn rejects_optional_sections_that_overlap_relocations() {
        let mut image = minimal_image(EXTENDED_HEADER_SIZE as u16, Some((72, 4, 0)));
        image.extend_from_slice(&[0; 4]);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }
}
