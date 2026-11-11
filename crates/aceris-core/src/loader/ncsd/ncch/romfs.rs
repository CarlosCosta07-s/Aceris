//! Structural parser for the Nintendo 3DS RomFS IVFC and Level 3 headers.
//!
//! This module validates container and table metadata only. It does not decrypt
//! NCCH content, locate physical IVFC levels, verify SHA-256 hashes, walk the
//! directory tree, or extract files.

use std::fmt;
use std::io::Read;

pub const IVFC_HEADER_SIZE: usize = 0x5C;
pub const LEVEL3_HEADER_SIZE: usize = 0x28;
pub const SHA256_SIZE: u64 = 0x20;
pub const DIRECTORY_METADATA_FIXED_SIZE: u64 = 0x18;
pub const FILE_METADATA_FIXED_SIZE: u64 = 0x20;

const IVFC_MAGIC: &[u8; 4] = b"IVFC";
const IVFC_VERSION: u32 = 0x0001_0000;

/// A logical hash-tree level described by the IVFC header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IvfcLevel {
    /// Logical offset recorded by the IVFC descriptor.
    pub logical_offset_bytes: u64,
    /// Number of bytes in this level as recorded by the descriptor.
    pub hash_data_size_bytes: u64,
    /// Base-2 exponent for this level's hash block size.
    pub block_size_log2: u32,
    /// Computed block size in bytes.
    pub block_size_bytes: u64,
}

impl IvfcLevel {
    /// Returns the exclusive logical end, or None if the range overflows.
    pub const fn logical_end_bytes(self) -> Option<u64> {
        self.logical_offset_bytes
            .checked_add(self.hash_data_size_bytes)
    }
}

/// Structural information from the fixed RomFS IVFC header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IvfcHeader {
    pub version: u32,
    pub master_hash_size_bytes: u32,
    pub level1: IvfcLevel,
    pub level2: IvfcLevel,
    pub level3: IvfcLevel,
    pub optional_info_size_bytes: u32,
}

/// Structural information from the RomFS Level 3 table header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level3Header {
    pub header_size_bytes: u32,
    pub directory_hash_offset_bytes: u32,
    pub directory_hash_size_bytes: u32,
    pub directory_metadata_offset_bytes: u32,
    pub directory_metadata_size_bytes: u32,
    pub file_hash_offset_bytes: u32,
    pub file_hash_size_bytes: u32,
    pub file_metadata_offset_bytes: u32,
    pub file_metadata_size_bytes: u32,
    pub file_data_offset_bytes: u32,
}

/// Errors reported while validating RomFS metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RomFsError {
    InvalidIvfcHeader(&'static str),
    InvalidIvfcLevel { index: u8, reason: &'static str },
    InvalidLevel3Header(&'static str),
    ReaderFailure(String),
}

impl fmt::Display for RomFsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIvfcHeader(reason) => {
                write!(formatter, "invalid RomFS IVFC header: {reason}")
            }
            Self::InvalidIvfcLevel { index, reason } => {
                write!(formatter, "invalid RomFS IVFC level {index}: {reason}")
            }
            Self::InvalidLevel3Header(reason) => {
                write!(formatter, "invalid RomFS Level 3 header: {reason}")
            }
            Self::ReaderFailure(reason) => {
                write!(formatter, "RomFS reader failure: {reason}")
            }
        }
    }
}

impl std::error::Error for RomFsError {}

impl IvfcHeader {
    /// Parses the fixed 0x5C-byte IVFC header.
    ///
    /// Logical level offsets are preserved as declared. This parser does not
    /// infer physical file positions from them.
    pub fn parse(data: &[u8], available_size_bytes: u64) -> Result<Self, RomFsError> {
        if data.len() < IVFC_HEADER_SIZE || available_size_bytes < IVFC_HEADER_SIZE as u64 {
            return Err(RomFsError::InvalidIvfcHeader("header is truncated"));
        }

        if data.get(0..4) != Some(IVFC_MAGIC.as_slice()) {
            return Err(RomFsError::InvalidIvfcHeader("IVFC magic is missing"));
        }

        let version = read_u32_le(data, 0x04)
            .ok_or(RomFsError::InvalidIvfcHeader("version field is truncated"))?;
        if version != IVFC_VERSION {
            return Err(RomFsError::InvalidIvfcHeader(
                "unsupported IVFC header version",
            ));
        }

        let master_hash_size_bytes = read_u32_le(data, 0x08).ok_or(
            RomFsError::InvalidIvfcHeader("master hash size is truncated"),
        )?;
        if u64::from(master_hash_size_bytes) < SHA256_SIZE
            || u64::from(master_hash_size_bytes) % SHA256_SIZE != 0
        {
            return Err(RomFsError::InvalidIvfcHeader(
                "master hash size is not a whole number of SHA-256 digests",
            ));
        }

        let level1 = parse_level(data, 0x0C, 1)?;
        let level2 = parse_level(data, 0x24, 2)?;
        let level3 = parse_level(data, 0x3C, 3)?;

        if level1.hash_data_size_bytes < SHA256_SIZE
            || level1.hash_data_size_bytes % SHA256_SIZE != 0
        {
            return Err(RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "hash data size is not a whole number of SHA-256 digests",
            });
        }
        if level2.hash_data_size_bytes < SHA256_SIZE
            || level2.hash_data_size_bytes % SHA256_SIZE != 0
        {
            return Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "hash data size is not a whole number of SHA-256 digests",
            });
        }
        if level3.hash_data_size_bytes < LEVEL3_HEADER_SIZE as u64 {
            return Err(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "level is too small to contain its fixed header",
            });
        }

        Ok(Self {
            version,
            master_hash_size_bytes,
            level1,
            level2,
            level3,
            optional_info_size_bytes: read_u32_le(data, 0x58).ok_or(
                RomFsError::InvalidIvfcHeader("optional-info size is truncated"),
            )?,
        })
    }

    /// Reads the fixed IVFC header from the reader's current position.
    pub fn parse_reader<R: Read>(
        reader: &mut R,
        available_size_bytes: u64,
    ) -> Result<Self, RomFsError> {
        let mut header = [0; IVFC_HEADER_SIZE];
        reader
            .read_exact(&mut header)
            .map_err(|_| RomFsError::InvalidIvfcHeader("header is truncated"))?;
        Self::parse(&header, available_size_bytes)
    }
}

impl Level3Header {
    /// Parses and bounds-checks the fixed Level 3 table header.
    ///
    /// The input slice begins at Level 3 byte zero. The length argument is the
    /// available logical Level 3 length from its IVFC descriptor.
    pub fn parse(data: &[u8], level3_size_bytes: u64) -> Result<Self, RomFsError> {
        if data.len() < LEVEL3_HEADER_SIZE || level3_size_bytes < LEVEL3_HEADER_SIZE as u64 {
            return Err(RomFsError::InvalidLevel3Header("header is truncated"));
        }

        let header_size_bytes = read_u32_le(data, 0x00).ok_or(RomFsError::InvalidLevel3Header(
            "header-size field is truncated",
        ))?;
        if header_size_bytes != LEVEL3_HEADER_SIZE as u32 {
            return Err(RomFsError::InvalidLevel3Header(
                "fixed header size is not 0x28 bytes",
            ));
        }

        let parsed = Self {
            header_size_bytes,
            directory_hash_offset_bytes: read_u32_le(data, 0x04).ok_or(
                RomFsError::InvalidLevel3Header("directory hash offset is truncated"),
            )?,
            directory_hash_size_bytes: read_u32_le(data, 0x08).ok_or(
                RomFsError::InvalidLevel3Header("directory hash size is truncated"),
            )?,
            directory_metadata_offset_bytes: read_u32_le(data, 0x0C).ok_or(
                RomFsError::InvalidLevel3Header("directory metadata offset is truncated"),
            )?,
            directory_metadata_size_bytes: read_u32_le(data, 0x10).ok_or(
                RomFsError::InvalidLevel3Header("directory metadata size is truncated"),
            )?,
            file_hash_offset_bytes: read_u32_le(data, 0x14).ok_or(
                RomFsError::InvalidLevel3Header("file hash offset is truncated"),
            )?,
            file_hash_size_bytes: read_u32_le(data, 0x18).ok_or(
                RomFsError::InvalidLevel3Header("file hash size is truncated"),
            )?,
            file_metadata_offset_bytes: read_u32_le(data, 0x1C).ok_or(
                RomFsError::InvalidLevel3Header("file metadata offset is truncated"),
            )?,
            file_metadata_size_bytes: read_u32_le(data, 0x20).ok_or(
                RomFsError::InvalidLevel3Header("file metadata size is truncated"),
            )?,
            file_data_offset_bytes: read_u32_le(data, 0x24).ok_or(
                RomFsError::InvalidLevel3Header("file data offset is truncated"),
            )?,
        };

        let tables = [
            (
                parsed.directory_hash_offset_bytes,
                parsed.directory_hash_size_bytes,
            ),
            (
                parsed.directory_metadata_offset_bytes,
                parsed.directory_metadata_size_bytes,
            ),
            (parsed.file_hash_offset_bytes, parsed.file_hash_size_bytes),
            (
                parsed.file_metadata_offset_bytes,
                parsed.file_metadata_size_bytes,
            ),
        ];

        let mut previous_end = u64::from(LEVEL3_HEADER_SIZE as u32);
        for (index, (offset, size)) in tables.into_iter().enumerate() {
            let start = u64::from(offset);
            let end = start
                .checked_add(u64::from(size))
                .ok_or(RomFsError::InvalidLevel3Header("table range overflows"))?;

            if start < u64::from(LEVEL3_HEADER_SIZE as u32) {
                return Err(RomFsError::InvalidLevel3Header(
                    "table starts inside the fixed header",
                ));
            }
            if start < previous_end {
                return Err(RomFsError::InvalidLevel3Header(
                    "tables overlap or are out of order",
                ));
            }
            if end > level3_size_bytes {
                return Err(RomFsError::InvalidLevel3Header(
                    "table extends beyond the Level 3 data",
                ));
            }
            if (index == 0 || index == 2) && size % 4 != 0 {
                return Err(RomFsError::InvalidLevel3Header(
                    "hash table length is not aligned to 32-bit bucket entries",
                ));
            }

            previous_end = end;
        }

        let file_data_offset = u64::from(parsed.file_data_offset_bytes);
        if file_data_offset < previous_end {
            return Err(RomFsError::InvalidLevel3Header(
                "file data starts before the file metadata table ends",
            ));
        }
        if file_data_offset > level3_size_bytes {
            return Err(RomFsError::InvalidLevel3Header(
                "file data offset exceeds the Level 3 data",
            ));
        }

        Ok(parsed)
    }
}

fn parse_level(data: &[u8], offset: usize, index: u8) -> Result<IvfcLevel, RomFsError> {
    let invalid = |reason| RomFsError::InvalidIvfcLevel { index, reason };
    let logical_offset_bytes =
        read_u64_le(data, offset).ok_or_else(|| invalid("logical offset is truncated"))?;
    let hash_data_size_bytes =
        read_u64_le(data, offset + 8).ok_or_else(|| invalid("hash data size is truncated"))?;
    let block_size_log2 =
        read_u32_le(data, offset + 16).ok_or_else(|| invalid("block-size field is truncated"))?;
    let block_size_bytes = 1u64
        .checked_shl(block_size_log2)
        .filter(|size| *size >= SHA256_SIZE)
        .ok_or_else(|| invalid("block-size exponent is invalid"))?;

    logical_offset_bytes
        .checked_add(hash_data_size_bytes)
        .ok_or_else(|| invalid("logical range overflows"))?;

    if hash_data_size_bytes == 0 {
        return Err(invalid("level has zero length"));
    }

    Ok(IvfcLevel {
        logical_offset_bytes,
        hash_data_size_bytes,
        block_size_log2,
        block_size_bytes,
    })
}

fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset.checked_add(4)?)?
        .try_into()
        .ok()
        .map(u32::from_le_bytes)
}

fn read_u64_le(data: &[u8], offset: usize) -> Option<u64> {
    data.get(offset..offset.checked_add(8)?)?
        .try_into()
        .ok()
        .map(u64::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn valid_ivfc_header() -> Vec<u8> {
        let mut data = vec![0; IVFC_HEADER_SIZE];
        data[..4].copy_from_slice(IVFC_MAGIC);
        write_u32(&mut data, 0x04, IVFC_VERSION);
        write_u32(&mut data, 0x08, 0x20);

        for (offset, logical, size) in [(0x0C, 0, 0x20), (0x24, 0x20, 0x20), (0x3C, 0x40, 0x100)] {
            write_u64(&mut data, offset, logical);
            write_u64(&mut data, offset + 8, size);
            write_u32(&mut data, offset + 16, 12);
        }

        data
    }

    fn valid_level3_header() -> Vec<u8> {
        let mut data = vec![0; LEVEL3_HEADER_SIZE];
        write_u32(&mut data, 0x00, 0x28);
        write_u32(&mut data, 0x04, 0x28);
        write_u32(&mut data, 0x08, 0x08);
        write_u32(&mut data, 0x0C, 0x30);
        write_u32(&mut data, 0x10, 0x20);
        write_u32(&mut data, 0x14, 0x50);
        write_u32(&mut data, 0x18, 0x08);
        write_u32(&mut data, 0x1C, 0x58);
        write_u32(&mut data, 0x20, 0x20);
        write_u32(&mut data, 0x24, 0x78);
        data
    }

    #[test]
    fn parses_ivfc_header_and_all_three_levels() {
        let data = valid_ivfc_header();
        let parsed = IvfcHeader::parse(&data, 0x4000).unwrap();

        assert_eq!(parsed.version, IVFC_VERSION);
        assert_eq!(parsed.master_hash_size_bytes, 0x20);
        assert_eq!(parsed.level1.logical_offset_bytes, 0);
        assert_eq!(parsed.level1.hash_data_size_bytes, 0x20);
        assert_eq!(parsed.level1.block_size_bytes, 0x1000);
        assert_eq!(parsed.level2.logical_offset_bytes, 0x20);
        assert_eq!(parsed.level2.hash_data_size_bytes, 0x20);
        assert_eq!(parsed.level3.logical_offset_bytes, 0x40);
        assert_eq!(parsed.level3.hash_data_size_bytes, 0x100);
        assert_eq!(parsed.level3.block_size_log2, 12);
        assert_eq!(parsed.optional_info_size_bytes, 0);
    }

    #[test]
    fn reads_ivfc_header_from_a_stream() {
        let data = valid_ivfc_header();
        let mut reader = Cursor::new(data);
        let parsed = IvfcHeader::parse_reader(&mut reader, 0x4000).unwrap();

        assert_eq!(reader.position(), IVFC_HEADER_SIZE as u64);
        assert_eq!(parsed.level3.hash_data_size_bytes, 0x100);
    }

    #[test]
    fn rejects_truncated_ivfc_header_and_bad_magic() {
        assert_eq!(
            IvfcHeader::parse(&[0; IVFC_HEADER_SIZE - 1], 0x4000),
            Err(RomFsError::InvalidIvfcHeader("header is truncated"))
        );

        let mut data = valid_ivfc_header();
        data[0] = b'X';
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcHeader("IVFC magic is missing"))
        );
    }

    #[test]
    fn rejects_unsupported_version_and_invalid_master_hash_size() {
        let mut data = valid_ivfc_header();
        write_u32(&mut data, 0x04, 0x20000);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcHeader(
                "unsupported IVFC header version"
            ))
        );

        data = valid_ivfc_header();
        write_u32(&mut data, 0x08, 0x21);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcHeader(
                "master hash size is not a whole number of SHA-256 digests"
            ))
        );
    }

    #[test]
    fn rejects_invalid_hash_level_sizes() {
        let mut data = valid_ivfc_header();
        write_u64(&mut data, 0x14, 0x21);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "hash data size is not a whole number of SHA-256 digests"
            })
        );

        data = valid_ivfc_header();
        write_u64(&mut data, 0x2C, 0x10);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "hash data size is not a whole number of SHA-256 digests"
            })
        );
    }

    #[test]
    fn rejects_invalid_level3_size_block_exponent_and_logical_overflow() {
        let mut data = valid_ivfc_header();
        write_u64(&mut data, 0x44, 0x20);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "level is too small to contain its fixed header"
            })
        );

        data = valid_ivfc_header();
        write_u32(&mut data, 0x3C + 16, 4);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "block-size exponent is invalid"
            })
        );

        data = valid_ivfc_header();
        write_u64(&mut data, 0x3C, u64::MAX - 4);
        assert_eq!(
            IvfcHeader::parse(&data, 0x4000),
            Err(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "logical range overflows"
            })
        );
    }

    #[test]
    fn parses_level3_table_ranges() {
        let data = valid_level3_header();
        let parsed = Level3Header::parse(&data, 0x100).unwrap();

        assert_eq!(parsed.header_size_bytes, 0x28);
        assert_eq!(parsed.directory_hash_offset_bytes, 0x28);
        assert_eq!(parsed.directory_hash_size_bytes, 0x08);
        assert_eq!(parsed.directory_metadata_offset_bytes, 0x30);
        assert_eq!(parsed.directory_metadata_size_bytes, 0x20);
        assert_eq!(parsed.file_hash_offset_bytes, 0x50);
        assert_eq!(parsed.file_hash_size_bytes, 0x08);
        assert_eq!(parsed.file_metadata_offset_bytes, 0x58);
        assert_eq!(parsed.file_metadata_size_bytes, 0x20);
        assert_eq!(parsed.file_data_offset_bytes, 0x78);
    }

    #[test]
    fn rejects_wrong_level3_header_size_and_truncated_input() {
        assert_eq!(
            Level3Header::parse(&[0; LEVEL3_HEADER_SIZE - 1], 0x100),
            Err(RomFsError::InvalidLevel3Header("header is truncated"))
        );

        let mut data = valid_level3_header();
        write_u32(&mut data, 0, 0x30);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "fixed header size is not 0x28 bytes"
            ))
        );
    }

    #[test]
    fn rejects_overlapping_or_out_of_order_level3_tables() {
        let mut data = valid_level3_header();
        write_u32(&mut data, 0x0C, 0x2C);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "tables overlap or are out of order"
            ))
        );
    }

    #[test]
    fn rejects_level3_tables_outside_the_declared_level() {
        let mut data = valid_level3_header();
        write_u32(&mut data, 0x20, 0x200);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "table extends beyond the Level 3 data"
            ))
        );
    }

    #[test]
    fn rejects_unaligned_hash_tables_and_invalid_file_data_offset() {
        let mut data = valid_level3_header();
        write_u32(&mut data, 0x08, 0x06);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "hash table length is not aligned to 32-bit bucket entries"
            ))
        );

        data = valid_level3_header();
        write_u32(&mut data, 0x24, 0x70);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "file data starts before the file metadata table ends"
            ))
        );

        data = valid_level3_header();
        write_u32(&mut data, 0x24, 0x101);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "file data offset exceeds the Level 3 data"
            ))
        );
    }

    #[test]
    fn rejects_tables_that_start_inside_the_fixed_header() {
        let mut data = valid_level3_header();
        write_u32(&mut data, 0x04, 0x20);
        assert_eq!(
            Level3Header::parse(&data, 0x100),
            Err(RomFsError::InvalidLevel3Header(
                "table starts inside the fixed header"
            ))
        );
    }

    #[test]
    fn ncch_reads_ivfc_header_from_the_declared_romfs_region() {
        use crate::loader::ncsd::ncch::NcchHeader;

        let mut ncch_header = vec![0; 0x200];
        ncch_header[0x100..0x104].copy_from_slice(b"NCCH");
        write_u32(&mut ncch_header, 0x104, 0x80);
        write_u32(&mut ncch_header, 0x180, 0x400);
        write_u32(&mut ncch_header, 0x1B0, 0x20);
        write_u32(&mut ncch_header, 0x1B4, 0x20);
        write_u32(&mut ncch_header, 0x1B8, 0x02);

        let parsed = NcchHeader::parse(&ncch_header, 0x1_0000).unwrap();
        let mut image = vec![0; 0x1_0000];
        let ivfc = valid_ivfc_header();
        image[0x4000..0x4000 + IVFC_HEADER_SIZE].copy_from_slice(&ivfc);

        let mut reader = Cursor::new(image);
        let parsed_ivfc = parsed
            .parse_romfs_ivfc_header(&mut reader)
            .unwrap()
            .unwrap();

        assert_eq!(parsed_ivfc.level3.hash_data_size_bytes, 0x100);
        assert_eq!(reader.position(), 0x4000 + IVFC_HEADER_SIZE as u64);
    }

    #[test]
    fn ncch_returns_none_when_romfs_is_not_declared() {
        use crate::loader::ncsd::ncch::NcchHeader;

        let mut ncch_header = vec![0; 0x200];
        ncch_header[0x100..0x104].copy_from_slice(b"NCCH");
        write_u32(&mut ncch_header, 0x104, 0x80);
        write_u32(&mut ncch_header, 0x180, 0x400);

        let parsed = NcchHeader::parse(&ncch_header, 0x1_0000).unwrap();
        let mut reader = Cursor::new(Vec::new());

        assert_eq!(parsed.parse_romfs_ivfc_header(&mut reader).unwrap(), None);
        assert_eq!(reader.position(), 0);
    }
}
