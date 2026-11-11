//! Structural parser for a Nintendo 3DS NCCH content container.
//!
//! This module parses the fixed 0x200-byte header and validates the declared
//! content regions. It does not verify RSA signatures, decrypt content, verify
//! hashes, or parse the extended header, ExeFS, or RomFS filesystems.

use std::fmt;
use std::io::{self, Read};

pub const HEADER_SIZE: usize = 0x200;

const MAGIC_OFFSET: usize = 0x100;
const CONTENT_SIZE_OFFSET: usize = 0x104;
const PARTITION_ID_OFFSET: usize = 0x108;
const MAKER_CODE_OFFSET: usize = 0x110;
const VERSION_OFFSET: usize = 0x112;
const PROGRAM_ID_OFFSET: usize = 0x118;
const LOGO_HASH_OFFSET: usize = 0x130;
const PRODUCT_CODE_OFFSET: usize = 0x150;
const EXTENDED_HEADER_HASH_OFFSET: usize = 0x160;
const EXTENDED_HEADER_SIZE_OFFSET: usize = 0x180;
const FLAGS_OFFSET: usize = 0x188;
const PLAIN_REGION_OFFSET: usize = 0x190;
const LOGO_REGION_OFFSET: usize = 0x198;
const EXEFS_REGION_OFFSET: usize = 0x1A0;
const ROMFS_REGION_OFFSET: usize = 0x1B0;
const EXEFS_HASH_OFFSET: usize = 0x1A8;
const ROMFS_HASH_OFFSET: usize = 0x1B8;
const EXEFS_SUPERBLOCK_HASH_OFFSET: usize = 0x1C0;
const ROMFS_SUPERBLOCK_HASH_OFFSET: usize = 0x1E0;
const BASE_MEDIA_UNIT_SIZE: u64 = 0x200;
const NO_CRYPTO_FLAG: u8 = 0x04;

/// The type of a byte range declared by the NCCH header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Plain,
    Logo,
    ExeFs,
    RomFs,
}

impl RegionKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain region",
            Self::Logo => "logo region",
            Self::ExeFs => "ExeFS",
            Self::RomFs => "RomFS",
        }
    }
}

/// A byte range within an NCCH container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NcchRegion {
    pub kind: RegionKind,
    /// Offset relative to the beginning of the NCCH container.
    pub offset_bytes: u64,
    /// Declared region length in bytes.
    pub size_bytes: u64,
    /// Number of initial bytes covered by the region's superblock hash.
    /// This is zero for regions without a dedicated hash-region field.
    pub hash_region_size_bytes: u64,
}

impl NcchRegion {
    /// Returns the exclusive end offset, or None if a caller-built value overflows.
    pub const fn end_offset_bytes(self) -> Option<u64> {
        self.offset_bytes.checked_add(self.size_bytes)
    }
}

/// Errors reported by the NCCH structural parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NcchError {
    InvalidHeader(&'static str),
    InvalidRegion {
        kind: RegionKind,
        reason: &'static str,
    },
    OverlappingRegions {
        first: RegionKind,
        second: RegionKind,
    },
}

impl fmt::Display for NcchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader(reason) => write!(formatter, "invalid NCCH header: {reason}"),
            Self::InvalidRegion { kind, reason } => {
                write!(formatter, "invalid NCCH {}: {reason}", kind.name())
            }
            Self::OverlappingRegions { first, second } => write!(
                formatter,
                "NCCH {} overlaps {}",
                first.name(),
                second.name()
            ),
        }
    }
}

impl std::error::Error for NcchError {}

/// Validated structural information from an NCCH header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NcchHeader {
    pub content_size_units: u32,
    pub content_size_bytes: u64,
    pub partition_id: [u8; 8],
    pub maker_code: [u8; 2],
    pub version: u16,
    pub program_id: u64,
    pub product_code: [u8; 16],
    pub extended_header_size: u32,
    pub flags: [u8; 8],
    pub media_unit_size: u64,
    pub logo_region_hash: [u8; 32],
    pub extended_header_hash: [u8; 32],
    pub exefs_superblock_hash: [u8; 32],
    pub romfs_superblock_hash: [u8; 32],
    pub plain_region: Option<NcchRegion>,
    pub logo_region: Option<NcchRegion>,
    pub exefs: Option<NcchRegion>,
    pub romfs: Option<NcchRegion>,
}

impl NcchHeader {
    /// Parses a header slice and validates all declared byte ranges.
    ///
    /// `file_len` is the available length of the NCCH container, not the
    /// length of the containing NCSD image. When parsing a partition, pass
    /// the partition length and provide bytes read from its bounded reader.
    pub fn parse(data: &[u8], file_len: u64) -> Result<Self, NcchError> {
        if data.len() < HEADER_SIZE || file_len < HEADER_SIZE as u64 {
            return Err(NcchError::InvalidHeader("header is truncated"));
        }

        if data.get(MAGIC_OFFSET..MAGIC_OFFSET + 4) != Some(&b"NCCH"[..]) {
            return Err(NcchError::InvalidHeader("NCCH magic is missing"));
        }

        let content_size_units = read_u32_le(data, CONTENT_SIZE_OFFSET)
            .ok_or(NcchError::InvalidHeader("content-size field is truncated"))?;
        if content_size_units == 0 {
            return Err(NcchError::InvalidHeader("content size is zero"));
        }

        let flags = read_array::<8>(data, FLAGS_OFFSET)
            .ok_or(NcchError::InvalidHeader("flags field is truncated"))?;
        let shift = u32::from(flags[6]);
        let shift_factor = 1u64
            .checked_shl(shift)
            .ok_or(NcchError::InvalidHeader("media-unit shift is too large"))?;
        let media_unit_size = BASE_MEDIA_UNIT_SIZE
            .checked_mul(shift_factor)
            .ok_or(NcchError::InvalidHeader("media-unit size overflows"))?;
        let content_size_bytes = u64::from(content_size_units)
            .checked_mul(media_unit_size)
            .ok_or(NcchError::InvalidHeader("content size overflows"))?;

        if content_size_bytes < HEADER_SIZE as u64 {
            return Err(NcchError::InvalidHeader(
                "declared content is smaller than the header",
            ));
        }
        if content_size_bytes > file_len {
            return Err(NcchError::InvalidHeader(
                "declared content exceeds the available container",
            ));
        }

        let extended_header_size = read_u32_le(data, EXTENDED_HEADER_SIZE_OFFSET).ok_or(
            NcchError::InvalidHeader("extended-header size field is truncated"),
        )?;
        let extended_header_end = (HEADER_SIZE as u64)
            .checked_add(u64::from(extended_header_size))
            .ok_or(NcchError::InvalidHeader("extended-header range overflows"))?;
        if extended_header_end > content_size_bytes {
            return Err(NcchError::InvalidHeader(
                "extended header exceeds the declared content",
            ));
        }

        let plain_region = read_region(
            data,
            RegionKind::Plain,
            PLAIN_REGION_OFFSET,
            None,
            media_unit_size,
            content_size_bytes,
        )?;
        let logo_region = read_region(
            data,
            RegionKind::Logo,
            LOGO_REGION_OFFSET,
            None,
            media_unit_size,
            content_size_bytes,
        )?;
        let exefs = read_region(
            data,
            RegionKind::ExeFs,
            EXEFS_REGION_OFFSET,
            Some(EXEFS_HASH_OFFSET),
            media_unit_size,
            content_size_bytes,
        )?;
        let romfs = read_region(
            data,
            RegionKind::RomFs,
            ROMFS_REGION_OFFSET,
            Some(ROMFS_HASH_OFFSET),
            media_unit_size,
            content_size_bytes,
        )?;

        let mut regions = [plain_region, logo_region, exefs, romfs]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();

        for region in &regions {
            let region_end = region.end_offset_bytes().ok_or(NcchError::InvalidRegion {
                kind: region.kind,
                reason: "end offset overflows",
            })?;
            if region.offset_bytes < extended_header_end && region_end > HEADER_SIZE as u64 {
                return Err(NcchError::InvalidRegion {
                    kind: region.kind,
                    reason: "overlaps the NCCH header or extended header",
                });
            }
        }

        regions.sort_unstable_by_key(|region| region.offset_bytes);
        for pair in regions.windows(2) {
            let previous = pair[0];
            let current = pair[1];
            let previous_end = previous
                .end_offset_bytes()
                .ok_or(NcchError::InvalidRegion {
                    kind: previous.kind,
                    reason: "end offset overflows",
                })?;
            if current.offset_bytes < previous_end {
                return Err(NcchError::OverlappingRegions {
                    first: previous.kind,
                    second: current.kind,
                });
            }
        }

        Ok(Self {
            content_size_units,
            content_size_bytes,
            partition_id: read_array::<8>(data, PARTITION_ID_OFFSET)
                .ok_or(NcchError::InvalidHeader("partition ID is truncated"))?,
            maker_code: read_array::<2>(data, MAKER_CODE_OFFSET)
                .ok_or(NcchError::InvalidHeader("maker code is truncated"))?,
            version: read_u16_le(data, VERSION_OFFSET)
                .ok_or(NcchError::InvalidHeader("version field is truncated"))?,
            program_id: read_u64_le(data, PROGRAM_ID_OFFSET)
                .ok_or(NcchError::InvalidHeader("program ID is truncated"))?,
            product_code: read_array::<16>(data, PRODUCT_CODE_OFFSET)
                .ok_or(NcchError::InvalidHeader("product code is truncated"))?,
            extended_header_size,
            flags,
            media_unit_size,
            logo_region_hash: read_array::<32>(data, LOGO_HASH_OFFSET)
                .ok_or(NcchError::InvalidHeader("logo hash is truncated"))?,
            extended_header_hash: read_array::<32>(data, EXTENDED_HEADER_HASH_OFFSET).ok_or(
                NcchError::InvalidHeader("extended-header hash is truncated"),
            )?,
            exefs_superblock_hash: read_array::<32>(data, EXEFS_SUPERBLOCK_HASH_OFFSET)
                .ok_or(NcchError::InvalidHeader("ExeFS hash is truncated"))?,
            romfs_superblock_hash: read_array::<32>(data, ROMFS_SUPERBLOCK_HASH_OFFSET)
                .ok_or(NcchError::InvalidHeader("RomFS hash is truncated"))?,
            plain_region,
            logo_region,
            exefs,
            romfs,
        })
    }

    /// Reads a header from the current reader position without loading the container.
    pub fn parse_reader<R: Read>(reader: &mut R, file_len: u64) -> Result<Self, NcchError> {
        let mut header = [0; HEADER_SIZE];
        reader
            .read_exact(&mut header)
            .map_err(|_| NcchError::InvalidHeader("header is truncated"))?;
        Self::parse(&header, file_len)
    }

    /// Returns the two low-order content-form bits from NCCH flag byte 5.
    pub const fn content_form_type(&self) -> u8 {
        self.flags[5] & 0x03
    }

    /// Returns the content-type field from NCCH flag byte 5.
    pub const fn content_type(&self) -> u8 {
        self.flags[5] >> 2
    }

    /// Returns true when the NCCH NoCrypto flag is set.
    ///
    /// This reports the header flag only; it does not decrypt or validate data.
    pub const fn declares_no_crypto(&self) -> bool {
        self.flags[7] & NO_CRYPTO_FLAG != 0
    }
}

fn read_region(
    data: &[u8],
    kind: RegionKind,
    descriptor_offset: usize,
    hash_offset: Option<usize>,
    media_unit_size: u64,
    content_size_bytes: u64,
) -> Result<Option<NcchRegion>, NcchError> {
    let offset_units = read_u32_le(data, descriptor_offset)
        .ok_or(NcchError::InvalidHeader("region offset field is truncated"))?;
    let size_units = read_u32_le(data, descriptor_offset + 4)
        .ok_or(NcchError::InvalidHeader("region size field is truncated"))?;
    let hash_units = match hash_offset {
        Some(offset) => read_u32_le(data, offset).ok_or(NcchError::InvalidHeader(
            "hash-region size field is truncated",
        ))?,
        None => 0,
    };

    if offset_units == 0 && size_units == 0 {
        if hash_units != 0 {
            return Err(NcchError::InvalidRegion {
                kind,
                reason: "hash region exists without a data region",
            });
        }
        return Ok(None);
    }

    if offset_units == 0 || size_units == 0 {
        return Err(NcchError::InvalidRegion {
            kind,
            reason: "offset and size must both be zero or both be non-zero",
        });
    }

    let offset_bytes =
        u64::from(offset_units)
            .checked_mul(media_unit_size)
            .ok_or(NcchError::InvalidRegion {
                kind,
                reason: "offset overflows",
            })?;
    let size_bytes =
        u64::from(size_units)
            .checked_mul(media_unit_size)
            .ok_or(NcchError::InvalidRegion {
                kind,
                reason: "size overflows",
            })?;
    let end_bytes = offset_bytes
        .checked_add(size_bytes)
        .ok_or(NcchError::InvalidRegion {
            kind,
            reason: "end offset overflows",
        })?;
    if offset_bytes < HEADER_SIZE as u64 {
        return Err(NcchError::InvalidRegion {
            kind,
            reason: "starts inside the NCCH header",
        });
    }
    if end_bytes > content_size_bytes {
        return Err(NcchError::InvalidRegion {
            kind,
            reason: "exceeds the declared NCCH content",
        });
    }

    let hash_region_size_bytes =
        u64::from(hash_units)
            .checked_mul(media_unit_size)
            .ok_or(NcchError::InvalidRegion {
                kind,
                reason: "hash-region size overflows",
            })?;
    if hash_region_size_bytes > size_bytes {
        return Err(NcchError::InvalidRegion {
            kind,
            reason: "hash region is larger than its data region",
        });
    }

    Ok(Some(NcchRegion {
        kind,
        offset_bytes,
        size_bytes,
        hash_region_size_bytes,
    }))
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
    use super::NcsdPartition;
    use std::io::Cursor;

    fn write_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn valid_header() -> Vec<u8> {
        let mut data = vec![0; HEADER_SIZE];
        data[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"NCCH");
        write_u32(&mut data, CONTENT_SIZE_OFFSET, 0x80);
        data[PARTITION_ID_OFFSET..PARTITION_ID_OFFSET + 8].copy_from_slice(b"ACERIS01");
        data[MAKER_CODE_OFFSET..MAKER_CODE_OFFSET + 2].copy_from_slice(b"01");
        write_u16(&mut data, VERSION_OFFSET, 2);
        data[PROGRAM_ID_OFFSET..PROGRAM_ID_OFFSET + 8]
            .copy_from_slice(&0x0004_0000_1234_5678u64.to_le_bytes());
        data[PRODUCT_CODE_OFFSET..PRODUCT_CODE_OFFSET + 16].copy_from_slice(b"CTR-P-ACERIS0001");
        write_u32(&mut data, EXTENDED_HEADER_SIZE_OFFSET, 0x400);
        data[FLAGS_OFFSET + 5] = 0b0000_1001;
        data[FLAGS_OFFSET + 7] = NO_CRYPTO_FLAG;

        write_u32(&mut data, PLAIN_REGION_OFFSET, 0x8);
        write_u32(&mut data, PLAIN_REGION_OFFSET + 4, 0x2);
        write_u32(&mut data, LOGO_REGION_OFFSET, 0xA);
        write_u32(&mut data, LOGO_REGION_OFFSET + 4, 0x1);
        write_u32(&mut data, EXEFS_REGION_OFFSET, 0x10);
        write_u32(&mut data, EXEFS_REGION_OFFSET + 4, 0x10);
        write_u32(&mut data, EXEFS_HASH_OFFSET, 0x2);
        write_u32(&mut data, ROMFS_REGION_OFFSET, 0x20);
        write_u32(&mut data, ROMFS_REGION_OFFSET + 4, 0x20);
        write_u32(&mut data, ROMFS_HASH_OFFSET, 0x2);
        data
    }

    #[test]
    fn parses_ncch_header_and_regions() {
        let data = valid_header();
        let parsed = NcchHeader::parse(&data, 0x1_0000).unwrap();

        assert_eq!(parsed.content_size_units, 0x80);
        assert_eq!(parsed.content_size_bytes, 0x1_0000);
        assert_eq!(parsed.media_unit_size, 0x200);
        assert_eq!(parsed.partition_id, *b"ACERIS01");
        assert_eq!(parsed.maker_code, *b"01");
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.program_id, 0x0004_0000_1234_5678);
        assert_eq!(&parsed.product_code, b"CTR-P-ACERIS0001");
        assert_eq!(parsed.extended_header_size, 0x400);
        assert_eq!(parsed.content_form_type(), 1);
        assert_eq!(parsed.content_type(), 2);
        assert!(parsed.declares_no_crypto());

        let plain = parsed.plain_region.unwrap();
        assert_eq!(plain.offset_bytes, 0x1000);
        assert_eq!(plain.size_bytes, 0x400);

        let logo = parsed.logo_region.unwrap();
        assert_eq!(logo.offset_bytes, 0x1400);
        assert_eq!(logo.size_bytes, 0x200);

        let exefs = parsed.exefs.unwrap();
        assert_eq!(exefs.offset_bytes, 0x2000);
        assert_eq!(exefs.size_bytes, 0x2000);
        assert_eq!(exefs.hash_region_size_bytes, 0x400);

        let romfs = parsed.romfs.unwrap();
        assert_eq!(romfs.offset_bytes, 0x4000);
        assert_eq!(romfs.size_bytes, 0x4000);
        assert_eq!(romfs.hash_region_size_bytes, 0x400);
    }

    #[test]
    fn reads_header_from_a_bounded_partition_reader() {
        let data = valid_header();
        let mut container = vec![0xAA; 0x1000];
        container.extend_from_slice(&data);
        container.resize(0x1000 + 0x1_0000, 0xBB);

        let mut partition = NcsdPartition {
            index: 0,
            offset_bytes: 0x1000,
            size_bytes: 0x1_0000,
            fs_type: 1,
            crypt_type: 0,
            id: 0,
        }
        .reader(Cursor::new(container))
        .unwrap();
        let parsed = NcchHeader::parse_reader(&mut partition, 0x1_0000).unwrap();

        assert_eq!(parsed.program_id, 0x0004_0000_1234_5678);
    }

    #[test]
    fn applies_media_unit_shift() {
        let mut data = valid_header();
        write_u32(&mut data, CONTENT_SIZE_OFFSET, 0x20);
        data[FLAGS_OFFSET + 6] = 2;
        write_u32(&mut data, PLAIN_REGION_OFFSET, 2);
        write_u32(&mut data, PLAIN_REGION_OFFSET + 4, 1);
        write_u32(&mut data, LOGO_REGION_OFFSET, 0);
        write_u32(&mut data, LOGO_REGION_OFFSET + 4, 0);
        write_u32(&mut data, EXEFS_REGION_OFFSET, 4);
        write_u32(&mut data, EXEFS_REGION_OFFSET + 4, 4);
        write_u32(&mut data, EXEFS_HASH_OFFSET, 1);
        write_u32(&mut data, ROMFS_REGION_OFFSET, 8);
        write_u32(&mut data, ROMFS_REGION_OFFSET + 4, 8);
        write_u32(&mut data, ROMFS_HASH_OFFSET, 1);

        let parsed = NcchHeader::parse(&data, 0x1_0000).unwrap();
        assert_eq!(parsed.media_unit_size, 0x800);
        assert_eq!(parsed.content_size_bytes, 0x1_0000);
        assert_eq!(parsed.exefs.unwrap().offset_bytes, 0x2000);
    }

    #[test]
    fn rejects_truncated_header_and_bad_magic() {
        assert_eq!(
            NcchHeader::parse(&[0; HEADER_SIZE - 1], 0x1_0000),
            Err(NcchError::InvalidHeader("header is truncated"))
        );

        let mut data = valid_header();
        data[MAGIC_OFFSET] = b'X';
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidHeader("NCCH magic is missing"))
        );
    }

    #[test]
    fn rejects_content_larger_than_available_container() {
        let data = valid_header();
        assert_eq!(
            NcchHeader::parse(&data, 0x8000),
            Err(NcchError::InvalidHeader(
                "declared content exceeds the available container"
            ))
        );
    }

    #[test]
    fn rejects_media_unit_shift_and_size_overflow() {
        let mut data = valid_header();
        data[FLAGS_OFFSET + 6] = 56;
        assert_eq!(
            NcchHeader::parse(&data, u64::MAX),
            Err(NcchError::InvalidHeader("media-unit size overflows"))
        );

        data[FLAGS_OFFSET + 6] = 24;
        write_u32(&mut data, CONTENT_SIZE_OFFSET, u32::MAX);
        assert_eq!(
            NcchHeader::parse(&data, u64::MAX),
            Err(NcchError::InvalidHeader("content size overflows"))
        );
    }

    #[test]
    fn rejects_extended_header_outside_content() {
        let mut data = valid_header();
        write_u32(&mut data, CONTENT_SIZE_OFFSET, 1);
        assert_eq!(
            NcchHeader::parse(&data, 0x200),
            Err(NcchError::InvalidHeader(
                "extended header exceeds the declared content"
            ))
        );

        write_u32(&mut data, CONTENT_SIZE_OFFSET, 0x80);
        write_u32(&mut data, EXTENDED_HEADER_SIZE_OFFSET, 0x1_0000);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidHeader(
                "extended header exceeds the declared content"
            ))
        );
    }

    #[test]
    fn rejects_half_empty_and_header_overlapping_regions() {
        let mut data = valid_header();
        write_u32(&mut data, LOGO_REGION_OFFSET, 0);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidRegion {
                kind: RegionKind::Logo,
                reason: "offset and size must both be zero or both be non-zero",
            })
        );

        data = valid_header();
        write_u32(&mut data, PLAIN_REGION_OFFSET, 2);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidRegion {
                kind: RegionKind::Plain,
                reason: "overlaps the NCCH header or extended header",
            })
        );
    }

    #[test]
    fn rejects_regions_outside_content_and_overlapping_each_other() {
        let mut data = valid_header();
        write_u32(&mut data, ROMFS_REGION_OFFSET, 0x7F);
        write_u32(&mut data, ROMFS_REGION_OFFSET + 4, 2);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidRegion {
                kind: RegionKind::RomFs,
                reason: "exceeds the declared NCCH content",
            })
        );

        data = valid_header();
        write_u32(&mut data, LOGO_REGION_OFFSET, 0x9);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::OverlappingRegions {
                first: RegionKind::Plain,
                second: RegionKind::Logo,
            })
        );
    }

    #[test]
    fn rejects_invalid_hash_region_sizes() {
        let mut data = valid_header();
        write_u32(&mut data, EXEFS_HASH_OFFSET, 0x11);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidRegion {
                kind: RegionKind::ExeFs,
                reason: "hash region is larger than its data region",
            })
        );

        data = valid_header();
        write_u32(&mut data, EXEFS_REGION_OFFSET, 0);
        write_u32(&mut data, EXEFS_REGION_OFFSET + 4, 0);
        assert_eq!(
            NcchHeader::parse(&data, 0x1_0000),
            Err(NcchError::InvalidRegion {
                kind: RegionKind::ExeFs,
                reason: "hash region exists without a data region",
            })
        );
    }

    #[test]
    fn rejects_short_header_reads_from_reader() {
        let mut reader = Cursor::new(vec![0; HEADER_SIZE - 1]);
        assert_eq!(
            NcchHeader::parse_reader(&mut reader, 0x1_0000),
            Err(NcchError::InvalidHeader("header is truncated"))
        );
    }
}
