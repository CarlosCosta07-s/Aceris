//! Structural parser for the 3DS Title Metadata (TMD) format.
//!
//! The parser validates the signed-data layout, fixed header, content-info
//! table, content records, and their SHA-256 digests. It does not verify the
//! RSA/ECDSA signature: that requires a trusted certificate chain and keys.

use std::collections::BTreeSet;
use std::fmt;

use crate::loader::ncsd::ncch::romfs::sha256::Sha256;

const SIGNATURE_TYPE_OFFSET: usize = 0;
const SIGNATURE_DATA_FIXED_SIZE: usize = 4;
const HEADER_SIZE: usize = 0xC4;
const CONTENT_INFO_RECORD_SIZE: usize = 0x24;
const CONTENT_INFO_RECORD_COUNT: usize = 64;
const CONTENT_INFO_TABLE_SIZE: usize = CONTENT_INFO_RECORD_SIZE * CONTENT_INFO_RECORD_COUNT;
const CONTENT_RECORD_SIZE: usize = 0x30;
const CONTENT_INFO_TABLE_OFFSET: usize = HEADER_SIZE;
const CONTENT_RECORDS_OFFSET: usize = CONTENT_INFO_TABLE_OFFSET + CONTENT_INFO_TABLE_SIZE;
const SECTION_ALIGNMENT: usize = 0x40;
const CONTENT_INDEX_BITMAP_SIZE: usize = 0x2000;

/// Signature algorithms accepted by the TMD container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TmdSignatureType {
    Rsa4096Sha1,
    Rsa2048Sha1,
    EcdsaSha1,
    Rsa4096Sha256,
    Rsa2048Sha256,
    EcdsaSha256,
}

impl TmdSignatureType {
    fn from_raw(value: u32) -> Option<Self> {
        match value {
            0x0001_0000 => Some(Self::Rsa4096Sha1),
            0x0001_0001 => Some(Self::Rsa2048Sha1),
            0x0001_0002 => Some(Self::EcdsaSha1),
            0x0001_0003 => Some(Self::Rsa4096Sha256),
            0x0001_0004 => Some(Self::Rsa2048Sha256),
            0x0001_0005 => Some(Self::EcdsaSha256),
            _ => None,
        }
    }

    const fn signature_size(self) -> usize {
        match self {
            Self::Rsa4096Sha1 | Self::Rsa4096Sha256 => 0x200,
            Self::Rsa2048Sha1 | Self::Rsa2048Sha256 => 0x100,
            Self::EcdsaSha1 | Self::EcdsaSha256 => 0x3C,
        }
    }

    /// Returns the big-endian signature type value stored on disk.
    pub const fn raw_value(self) -> u32 {
        match self {
            Self::Rsa4096Sha1 => 0x0001_0000,
            Self::Rsa2048Sha1 => 0x0001_0001,
            Self::EcdsaSha1 => 0x0001_0002,
            Self::Rsa4096Sha256 => 0x0001_0003,
            Self::Rsa2048Sha256 => 0x0001_0004,
            Self::EcdsaSha256 => 0x0001_0005,
        }
    }
}

/// One of the 64 fixed TMD content-info records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TmdContentInfoRecord {
    /// First content index described by this info record.
    pub content_index_offset: u16,
    /// Number of content chunk records covered by this digest.
    pub content_command_count: u16,
    /// SHA-256 digest of the next group of content chunk records.
    pub hash: [u8; 32],
}

/// One content entry declared by the TMD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmdContentRecord {
    pub content_id: u32,
    pub content_index: u16,
    pub content_type: u16,
    pub size_bytes: u64,
    pub sha256: [u8; 32],
}

/// Structural TMD data after bounds and table-hash validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmdHeader {
    pub signature_type: TmdSignatureType,
    pub signature_data_size_bytes: usize,
    pub issuer: [u8; 0x40],
    pub version: u8,
    pub ca_crl_version: u8,
    pub signer_crl_version: u8,
    pub system_version: u64,
    pub title_id: u64,
    pub title_type: u32,
    pub group_id: u16,
    pub access_rights: u32,
    pub title_version: u16,
    pub content_count: u16,
    pub boot_content_index: u16,
    pub content_info_records_hash: [u8; 32],
    pub content_info_records: Vec<TmdContentInfoRecord>,
    pub contents: Vec<TmdContentRecord>,
    /// Number of bytes occupied by the signature, header and tables.
    /// Trailing data, if any, is deliberately left uninterpreted.
    pub parsed_size_bytes: usize,
}

impl TmdHeader {
    /// Parses a complete TMD header and its declared content tables.
    ///
    /// All multi-byte fields are big-endian except fields documented as
    /// little-endian by the TMD format. This parser does not consume or verify
    /// the signature bytes.
    pub fn parse(data: &[u8]) -> Result<Self, TmdError> {
        let signature_type_raw = read_u32_be(data, SIGNATURE_TYPE_OFFSET)?;
        let signature_type = TmdSignatureType::from_raw(signature_type_raw)
            .ok_or(TmdError::UnsupportedSignatureType(signature_type_raw))?;
        let signature_data_size_bytes = aligned_signature_data_size(signature_type)?;

        let header_start = signature_data_size_bytes;
        let content_info_start = checked_add(header_start, CONTENT_INFO_TABLE_OFFSET)?;
        let content_records_start = checked_add(header_start, CONTENT_RECORDS_OFFSET)?;

        ensure_range(data, 0, signature_data_size_bytes, "signature data")?;
        ensure_range(data, header_start, HEADER_SIZE, "TMD header")?;
        ensure_range(
            data,
            content_info_start,
            CONTENT_INFO_TABLE_SIZE,
            "content-info table",
        )?;

        let header = &data[header_start..content_info_start];
        let content_count = read_u16_be(header, 0x9E)?;
        let content_records_size = usize::from(content_count)
            .checked_mul(CONTENT_RECORD_SIZE)
            .ok_or(TmdError::SizeOverflow)?;
        ensure_range(
            data,
            content_records_start,
            content_records_size,
            "content records",
        )?;
        let parsed_size_bytes = checked_add(content_records_start, content_records_size)?;

        let content_info_bytes = &data[content_info_start..content_records_start];
        let expected_info_hash = read_array::<32>(header, 0xA4)?;
        let actual_info_hash =
            Sha256::digest(content_info_bytes).map_err(|_| TmdError::HashInputTooLong)?;
        if actual_info_hash != expected_info_hash {
            return Err(TmdError::ContentInfoTableHashMismatch);
        }

        let mut content_info_records = Vec::with_capacity(CONTENT_INFO_RECORD_COUNT);
        let mut covered_content_count = 0usize;
        for index in 0..CONTENT_INFO_RECORD_COUNT {
            let offset = index * CONTENT_INFO_RECORD_SIZE;
            let record = parse_content_info_record(
                &content_info_bytes[offset..offset + CONTENT_INFO_RECORD_SIZE],
            )?;
            let count = usize::from(record.content_command_count);
            if count != 0 {
                let group_end = covered_content_count
                    .checked_add(count)
                    .ok_or(TmdError::SizeOverflow)?;
                if group_end > usize::from(content_count) {
                    return Err(TmdError::ContentInfoCountMismatch {
                        declared: usize::from(content_count),
                        covered: group_end,
                    });
                }

                let group_start_byte = content_records_start
                    .checked_add(
                        covered_content_count
                            .checked_mul(CONTENT_RECORD_SIZE)
                            .ok_or(TmdError::SizeOverflow)?,
                    )
                    .ok_or(TmdError::SizeOverflow)?;
                let group_size = count
                    .checked_mul(CONTENT_RECORD_SIZE)
                    .ok_or(TmdError::SizeOverflow)?;
                let group_bytes = &data[group_start_byte..group_start_byte + group_size];
                let actual_hash =
                    Sha256::digest(group_bytes).map_err(|_| TmdError::HashInputTooLong)?;
                if actual_hash != record.hash {
                    return Err(TmdError::ContentGroupHashMismatch {
                        record_index: index,
                    });
                }
                covered_content_count = group_end;
            }
            content_info_records.push(record);
        }

        if covered_content_count != usize::from(content_count) {
            return Err(TmdError::ContentInfoCountMismatch {
                declared: usize::from(content_count),
                covered: covered_content_count,
            });
        }

        let mut contents = Vec::with_capacity(usize::from(content_count));
        let mut seen_indices = BTreeSet::new();
        for index in 0..usize::from(content_count) {
            let offset = content_records_start + index * CONTENT_RECORD_SIZE;
            let record = parse_content_record(&data[offset..offset + CONTENT_RECORD_SIZE])?;
            if !seen_indices.insert(record.content_index) {
                return Err(TmdError::DuplicateContentIndex(record.content_index));
            }
            contents.push(record);
        }

        let boot_content_index = read_u16_be(header, 0xA0)?;
        if content_count != 0 && !seen_indices.contains(&boot_content_index) {
            return Err(TmdError::BootContentNotFound(boot_content_index));
        }

        Ok(Self {
            signature_type,
            signature_data_size_bytes,
            issuer: read_array::<0x40>(header, 0x00)?,
            version: header[0x40],
            ca_crl_version: header[0x41],
            signer_crl_version: header[0x42],
            system_version: read_u64_be(header, 0x44)?,
            title_id: read_u64_be(header, 0x4C)?,
            title_type: read_u32_be(header, 0x54)?,
            group_id: read_u16_be(header, 0x58)?,
            access_rights: read_u32_be(header, 0x98)?,
            title_version: read_u16_be(header, 0x9C)?,
            content_count,
            boot_content_index,
            content_info_records_hash: expected_info_hash,
            content_info_records,
            contents,
            parsed_size_bytes,
        })
    }

    /// Ensures every content bit selected by the enclosing CIA has a matching
    /// TMD content record. TMD entries not selected by the CIA are permitted.
    pub fn validate_cia_content_index_bitmap(
        &self,
        bitmap: &[u8; CONTENT_INDEX_BITMAP_SIZE],
    ) -> Result<(), TmdError> {
        let declared_indices = self
            .contents
            .iter()
            .map(|content| content.content_index)
            .collect::<BTreeSet<_>>();

        for (byte_index, byte) in bitmap.iter().copied().enumerate() {
            for bit_index in 0..8 {
                if byte & (1 << bit_index) == 0 {
                    continue;
                }
                let raw_index = byte_index * 8 + bit_index;
                let content_index =
                    u16::try_from(raw_index).expect("CIA bitmap has exactly 65536 bits");
                if !declared_indices.contains(&content_index) {
                    return Err(TmdError::CiaContentIndexMissingFromTmd(content_index));
                }
            }
        }
        Ok(())
    }

    /// Returns the declared content entry for an index, if present.
    pub fn content_by_index(&self, content_index: u16) -> Option<&TmdContentRecord> {
        self.contents
            .iter()
            .find(|content| content.content_index == content_index)
    }
}

/// Failures while parsing or cross-checking TMD metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TmdError {
    UnsupportedSignatureType(u32),
    Truncated { section: &'static str },
    SizeOverflow,
    HashInputTooLong,
    ContentInfoTableHashMismatch,
    ContentGroupHashMismatch { record_index: usize },
    ContentInfoCountMismatch { declared: usize, covered: usize },
    DuplicateContentIndex(u16),
    BootContentNotFound(u16),
    CiaContentIndexMissingFromTmd(u16),
}

impl fmt::Display for TmdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSignatureType(value) => {
                write!(formatter, "unsupported TMD signature type {value:#010x}")
            }
            Self::Truncated { section } => write!(formatter, "TMD is truncated in {section}"),
            Self::SizeOverflow => formatter.write_str("TMD section size overflows the host range"),
            Self::HashInputTooLong => formatter.write_str("TMD hash input is too large"),
            Self::ContentInfoTableHashMismatch => {
                formatter.write_str("TMD content-info table SHA-256 does not match")
            }
            Self::ContentGroupHashMismatch { record_index } => write!(
                formatter,
                "TMD content group {record_index} SHA-256 does not match"
            ),
            Self::ContentInfoCountMismatch { declared, covered } => write!(
                formatter,
                "TMD declares {declared} contents but content-info records cover {covered}"
            ),
            Self::DuplicateContentIndex(index) => {
                write!(formatter, "TMD contains duplicate content index {index}")
            }
            Self::BootContentNotFound(index) => {
                write!(formatter, "TMD boot content index {index} is not declared")
            }
            Self::CiaContentIndexMissingFromTmd(index) => write!(
                formatter,
                "CIA selects content index {index}, but the TMD does not declare it"
            ),
        }
    }
}

impl std::error::Error for TmdError {}

fn parse_content_info_record(data: &[u8]) -> Result<TmdContentInfoRecord, TmdError> {
    Ok(TmdContentInfoRecord {
        content_index_offset: read_u16_be(data, 0)?,
        content_command_count: read_u16_be(data, 2)?,
        hash: read_array::<32>(data, 4)?,
    })
}

fn parse_content_record(data: &[u8]) -> Result<TmdContentRecord, TmdError> {
    Ok(TmdContentRecord {
        content_id: read_u32_be(data, 0)?,
        content_index: read_u16_be(data, 4)?,
        content_type: read_u16_be(data, 6)?,
        size_bytes: read_u64_be(data, 8)?,
        sha256: read_array::<32>(data, 0x10)?,
    })
}

fn aligned_signature_data_size(signature_type: TmdSignatureType) -> Result<usize, TmdError> {
    let raw_size = SIGNATURE_DATA_FIXED_SIZE
        .checked_add(signature_type.signature_size())
        .ok_or(TmdError::SizeOverflow)?;
    raw_size
        .checked_add(SECTION_ALIGNMENT - 1)
        .map(|size| size & !(SECTION_ALIGNMENT - 1))
        .ok_or(TmdError::SizeOverflow)
}

fn checked_add(left: usize, right: usize) -> Result<usize, TmdError> {
    left.checked_add(right).ok_or(TmdError::SizeOverflow)
}

fn ensure_range(
    data: &[u8],
    offset: usize,
    size: usize,
    section: &'static str,
) -> Result<(), TmdError> {
    let end = checked_add(offset, size)?;
    if end > data.len() {
        return Err(TmdError::Truncated { section });
    }
    Ok(())
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], TmdError> {
    let bytes = data
        .get(offset..checked_add(offset, N)?)
        .ok_or(TmdError::Truncated {
            section: "fixed fields",
        })?;
    bytes.try_into().map_err(|_| TmdError::Truncated {
        section: "fixed fields",
    })
}

fn read_u16_be(data: &[u8], offset: usize) -> Result<u16, TmdError> {
    let bytes = read_array::<2>(data, offset)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u32_be(data: &[u8], offset: usize) -> Result<u32, TmdError> {
    let bytes = read_array::<4>(data, offset)?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64_be(data: &[u8], offset: usize) -> Result<u64, TmdError> {
    let bytes = read_array::<8>(data, offset)?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(contents: &[(u32, u16, u16, u64, [u8; 32])]) -> Vec<u8> {
        let signature_size = aligned_signature_data_size(TmdSignatureType::Rsa2048Sha256).unwrap();
        let content_records_size = contents.len() * CONTENT_RECORD_SIZE;
        let mut data = vec![0u8; signature_size + CONTENT_RECORDS_OFFSET + content_records_size];
        data[0..4].copy_from_slice(&TmdSignatureType::Rsa2048Sha256.raw_value().to_be_bytes());

        let header_start = signature_size;
        let header = &mut data[header_start..header_start + HEADER_SIZE];
        header[0x40] = 1;
        header[0x41] = 2;
        header[0x42] = 3;
        header[0x44..0x4C].copy_from_slice(&0x0004_0000_0000_0002u64.to_be_bytes());
        header[0x4C..0x54].copy_from_slice(&0x0004_0000_1234_5678u64.to_be_bytes());
        header[0x54..0x58].copy_from_slice(&1u32.to_be_bytes());
        header[0x58..0x5A].copy_from_slice(&0x1234u16.to_be_bytes());
        header[0x98..0x9C].copy_from_slice(&0x0000_0003u32.to_be_bytes());
        header[0x9C..0x9E].copy_from_slice(&7u16.to_be_bytes());
        header[0x9E..0xA0].copy_from_slice(&(contents.len() as u16).to_be_bytes());
        if let Some((_, index, _, _, _)) = contents.first() {
            header[0xA0..0xA2].copy_from_slice(&index.to_be_bytes());
        }

        let info_start = header_start + CONTENT_INFO_TABLE_OFFSET;
        let records_start = header_start + CONTENT_RECORDS_OFFSET;
        if !contents.is_empty() {
            let info = &mut data[info_start..info_start + CONTENT_INFO_RECORD_SIZE];
            info[0..2].copy_from_slice(&contents[0].1.to_be_bytes());
            info[2..4].copy_from_slice(&(contents.len() as u16).to_be_bytes());
        }

        for (index, (content_id, content_index, content_type, size_bytes, hash)) in
            contents.iter().enumerate()
        {
            let offset = records_start + index * CONTENT_RECORD_SIZE;
            data[offset..offset + 4].copy_from_slice(&content_id.to_be_bytes());
            data[offset + 4..offset + 6].copy_from_slice(&content_index.to_be_bytes());
            data[offset + 6..offset + 8].copy_from_slice(&content_type.to_be_bytes());
            data[offset + 8..offset + 0x10].copy_from_slice(&size_bytes.to_be_bytes());
            data[offset + 0x10..offset + 0x30].copy_from_slice(hash);
        }

        if !contents.is_empty() {
            let group_hash =
                Sha256::digest(&data[records_start..records_start + content_records_size]).unwrap();
            data[info_start + 4..info_start + 0x24].copy_from_slice(&group_hash);
        }
        let info_hash =
            Sha256::digest(&data[info_start..info_start + CONTENT_INFO_TABLE_SIZE]).unwrap();
        data[header_start + 0xA4..header_start + 0xC4].copy_from_slice(&info_hash);
        data
    }

    #[test]
    fn parses_header_content_records_and_verified_hash_tables() {
        let data = fixture(&[(0x1234, 0, 1, 0x2000, [0x5A; 32])]);

        let tmd = TmdHeader::parse(&data).unwrap();

        assert_eq!(tmd.signature_type, TmdSignatureType::Rsa2048Sha256);
        assert_eq!(tmd.signature_data_size_bytes, 0x140);
        assert_eq!(tmd.title_id, 0x0004_0000_1234_5678);
        assert_eq!(tmd.title_version, 7);
        assert_eq!(tmd.content_count, 1);
        assert_eq!(tmd.boot_content_index, 0);
        assert_eq!(tmd.contents[0].content_id, 0x1234);
        assert_eq!(tmd.contents[0].content_index, 0);
        assert_eq!(tmd.contents[0].size_bytes, 0x2000);
        assert_eq!(tmd.parsed_size_bytes, data.len());
    }

    #[test]
    fn parses_all_documented_signature_data_alignments() {
        assert_eq!(
            aligned_signature_data_size(TmdSignatureType::Rsa4096Sha256).unwrap(),
            0x240
        );
        assert_eq!(
            aligned_signature_data_size(TmdSignatureType::Rsa2048Sha256).unwrap(),
            0x140
        );
        assert_eq!(
            aligned_signature_data_size(TmdSignatureType::EcdsaSha256).unwrap(),
            0x40
        );
    }

    #[test]
    fn rejects_unknown_signature_types() {
        let data = [0, 0, 0, 0];
        assert_eq!(
            TmdHeader::parse(&data),
            Err(TmdError::UnsupportedSignatureType(0))
        );
    }

    #[test]
    fn rejects_content_info_table_hash_mismatch() {
        let mut data = fixture(&[(1, 0, 1, 1, [0; 32])]);
        let header_start = aligned_signature_data_size(TmdSignatureType::Rsa2048Sha256).unwrap();
        data[header_start + 0xA4] ^= 0xFF;

        assert_eq!(
            TmdHeader::parse(&data),
            Err(TmdError::ContentInfoTableHashMismatch)
        );
    }

    #[test]
    fn rejects_content_group_hash_mismatch() {
        let mut data = fixture(&[(1, 0, 1, 1, [0; 32])]);
        let records_start = aligned_signature_data_size(TmdSignatureType::Rsa2048Sha256).unwrap()
            + CONTENT_RECORDS_OFFSET;
        data[records_start + 8] ^= 0x01;

        assert!(matches!(
            TmdHeader::parse(&data),
            Err(TmdError::ContentGroupHashMismatch { record_index: 0 })
        ));
    }

    #[test]
    fn checks_cia_bitmap_indices_against_tmd_records() {
        let data = fixture(&[(1, 2, 1, 1, [0; 32])]);
        let tmd = TmdHeader::parse(&data).unwrap();
        let mut bitmap = [0u8; CONTENT_INDEX_BITMAP_SIZE];
        bitmap[0] = 1 << 2;

        assert_eq!(tmd.validate_cia_content_index_bitmap(&bitmap), Ok(()));

        bitmap[0] = 1;
        assert_eq!(
            tmd.validate_cia_content_index_bitmap(&bitmap),
            Err(TmdError::CiaContentIndexMissingFromTmd(0))
        );
    }

    #[test]
    fn rejects_duplicate_content_indices() {
        let data = fixture(&[(1, 0, 1, 1, [0; 32]), (2, 0, 1, 1, [0; 32])]);

        assert_eq!(
            TmdHeader::parse(&data),
            Err(TmdError::DuplicateContentIndex(0))
        );
    }

    #[test]
    fn rejects_truncated_content_records() {
        let mut data = fixture(&[(1, 0, 1, 1, [0; 32])]);
        data.pop();

        assert_eq!(
            TmdHeader::parse(&data),
            Err(TmdError::Truncated {
                section: "content records"
            })
        );
    }
}
