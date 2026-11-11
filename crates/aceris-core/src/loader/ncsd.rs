//! Structural parser for Nintendo 3DS NCSD cartridge images.
//!
//! The parser reads only the public container header. It does not verify the
//! RSA signature, decrypt partitions, or claim that partition contents are
//! executable. Those checks belong to later loading stages.

use aceris_common::{Error, Result};

pub const HEADER_SIZE: usize = 0x200;
pub const PARTITION_COUNT: usize = 8;

const MAGIC_OFFSET: usize = 0x100;
const MEDIA_SIZE_OFFSET: usize = 0x104;
const MEDIA_ID_OFFSET: usize = 0x108;
const FS_TYPES_OFFSET: usize = 0x110;
const CRYPT_TYPES_OFFSET: usize = 0x118;
const PARTITION_TABLE_OFFSET: usize = 0x120;
const FLAGS_OFFSET: usize = 0x188;
const PARTITION_IDS_OFFSET: usize = 0x190;
const BASE_MEDIA_UNIT_SIZE: u64 = 0x200;

/// One populated entry from the NCSD partition table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NcsdPartition {
    /// Original partition-table slot (0 through 7).
    pub index: u8,
    /// Byte offset of the partition from the start of the image.
    pub offset_bytes: u64,
    /// Partition length in bytes.
    pub size_bytes: u64,
    /// Raw filesystem type byte from the NCSD header.
    pub fs_type: u8,
    /// Raw crypto type byte from the NCSD header.
    pub crypt_type: u8,
    /// Partition identifier stored in the header.
    pub id: u64,
}

impl NcsdPartition {
    /// Returns the exclusive end offset, or None if a caller-built value overflows.
    pub const fn end_offset_bytes(self) -> Option<u64> {
        self.offset_bytes.checked_add(self.size_bytes)
    }
}

/// Validated structural information from an NCSD header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NcsdHeader {
    pub media_size_units: u32,
    pub image_size_bytes: u64,
    pub media_id: [u8; 8],
    pub media_unit_size: u64,
    pub flags: [u8; 8],
    pub partitions: Vec<NcsdPartition>,
}

impl NcsdHeader {
    /// Parses an NCSD header and validates its declared partition ranges.
    ///
    /// The input slice must contain the complete 0x200-byte header. The file
    /// length is checked independently from the declared image size so trimmed
    /// images can still be accepted when all referenced partitions are present.
    pub fn parse(data: &[u8], file_len: u64) -> Result<Self> {
        if data.len() < HEADER_SIZE || file_len < HEADER_SIZE as u64 {
            return Err(Error::InvalidNcsdHeader("header is truncated"));
        }

        if data.get(MAGIC_OFFSET..MAGIC_OFFSET + 4) != Some(&b"NCSD"[..]) {
            return Err(Error::InvalidNcsdHeader("NCSD magic is missing"));
        }

        let media_size_units = read_u32_le(data, MEDIA_SIZE_OFFSET)
            .ok_or(Error::InvalidNcsdHeader("media-size field is truncated"))?;
        if media_size_units == 0 {
            return Err(Error::InvalidNcsdHeader("declared image size is zero"));
        }

        let media_id = read_array::<8>(data, MEDIA_ID_OFFSET)
            .ok_or(Error::InvalidNcsdHeader("media identifier is truncated"))?;
        let flags = read_array::<8>(data, FLAGS_OFFSET)
            .ok_or(Error::InvalidNcsdHeader("flags field is truncated"))?;

        // NCSD flag byte 6 stores the base-2 exponent for the media unit.
        let shift_factor = 1u64
            .checked_shl(u32::from(flags[6]))
            .ok_or(Error::InvalidNcsdHeader("media-unit shift is too large"))?;
        let media_unit_size = BASE_MEDIA_UNIT_SIZE
            .checked_mul(shift_factor)
            .ok_or(Error::InvalidNcsdHeader("media-unit size overflows"))?;
        let image_size_bytes = u64::from(media_size_units)
            .checked_mul(media_unit_size)
            .ok_or(Error::InvalidNcsdHeader("declared image size overflows"))?;

        if image_size_bytes < HEADER_SIZE as u64 {
            return Err(Error::InvalidNcsdHeader(
                "declared image is smaller than its header",
            ));
        }

        let mut partitions = Vec::with_capacity(PARTITION_COUNT);
        let mut ranges = Vec::with_capacity(PARTITION_COUNT);

        for index in 0..PARTITION_COUNT {
            let entry_offset = PARTITION_TABLE_OFFSET + index * 8;
            let offset_units = read_u32_le(data, entry_offset)
                .ok_or(Error::InvalidNcsdHeader("partition table is truncated"))?;
            let size_units = read_u32_le(data, entry_offset + 4)
                .ok_or(Error::InvalidNcsdHeader("partition table is truncated"))?;

            if offset_units == 0 && size_units == 0 {
                continue;
            }
            if offset_units == 0 || size_units == 0 {
                return Err(Error::InvalidNcsdPartition {
                    index,
                    reason: "offset and size must both be zero or both be non-zero",
                });
            }

            let offset_bytes = u64::from(offset_units)
                .checked_mul(media_unit_size)
                .ok_or(Error::InvalidNcsdPartition {
                    index,
                    reason: "offset overflows",
                })?;
            let size_bytes = u64::from(size_units)
                .checked_mul(media_unit_size)
                .ok_or(Error::InvalidNcsdPartition {
                    index,
                    reason: "size overflows",
                })?;
            let end_bytes = offset_bytes
                .checked_add(size_bytes)
                .ok_or(Error::InvalidNcsdPartition {
                    index,
                    reason: "end offset overflows",
                })?;

            if offset_bytes < HEADER_SIZE as u64 {
                return Err(Error::InvalidNcsdPartition {
                    index,
                    reason: "partition overlaps the NCSD header",
                });
            }
            if end_bytes > image_size_bytes {
                return Err(Error::InvalidNcsdPartition {
                    index,
                    reason: "partition exceeds the declared image size",
                });
            }
            if end_bytes > file_len {
                return Err(Error::InvalidNcsdPartition {
                    index,
                    reason: "partition extends beyond the actual file",
                });
            }

            let fs_type = data[FS_TYPES_OFFSET + index];
            let crypt_type = data[CRYPT_TYPES_OFFSET + index];
            let id_offset = PARTITION_IDS_OFFSET + index * 8;
            let id = read_u64_le(data, id_offset)
                .ok_or(Error::InvalidNcsdHeader("partition ID table is truncated"))?;

            partitions.push(NcsdPartition {
                index: index as u8,
                offset_bytes,
                size_bytes,
                fs_type,
                crypt_type,
                id,
            });
            ranges.push((offset_bytes, end_bytes, index));
        }

        ranges.sort_unstable_by_key(|range| range.0);
        for pair in ranges.windows(2) {
            let previous = pair[0];
            let current = pair[1];
            if current.0 < previous.1 {
                return Err(Error::OverlappingNcsdPartitions {
                    first: previous.2,
                    second: current.2,
                });
            }
        }

        Ok(Self {
            media_size_units,
            image_size_bytes,
            media_id,
            media_unit_size,
            flags,
            partitions,
        })
    }

    /// Returns a populated partition by its original table index.
    pub fn partition(&self, index: u8) -> Option<&NcsdPartition> {
        self.partitions.iter().find(|partition| partition.index == index)
    }

    /// Returns partition zero, conventionally the executable game content.
    pub fn primary_partition(&self) -> Option<&NcsdPartition> {
        self.partition(0)
    }
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
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

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_partition(data: &mut [u8], index: usize, offset_units: u32, size_units: u32) {
        let entry = PARTITION_TABLE_OFFSET + index * 8;
        write_u32(data, entry, offset_units);
        write_u32(data, entry + 4, size_units);
    }

    fn valid_header() -> Vec<u8> {
        let mut data = vec![0; HEADER_SIZE];
        data[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"NCSD");
        write_u32(&mut data, MEDIA_SIZE_OFFSET, 0x80);
        data[MEDIA_ID_OFFSET..MEDIA_ID_OFFSET + 8].copy_from_slice(b"ACERIS01");
        data[FS_TYPES_OFFSET] = 1;
        data[CRYPT_TYPES_OFFSET] = 0;
        write_partition(&mut data, 0, 0x20, 0x20);
        write_partition(&mut data, 1, 0x40, 0x10);
        data[PARTITION_IDS_OFFSET..PARTITION_IDS_OFFSET + 8]
            .copy_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        data
    }

    #[test]
    fn parses_header_and_partition_table() {
        let data = valid_header();
        let parsed = NcsdHeader::parse(&data, 0x1_0000).unwrap();

        assert_eq!(parsed.media_size_units, 0x80);
        assert_eq!(parsed.image_size_bytes, 0x1_0000);
        assert_eq!(parsed.media_unit_size, 0x200);
        assert_eq!(parsed.media_id, *b"ACERIS01");
        assert_eq!(parsed.partitions.len(), 2);

        let primary = parsed.primary_partition().unwrap();
        assert_eq!(primary.index, 0);
        assert_eq!(primary.offset_bytes, 0x4000);
        assert_eq!(primary.size_bytes, 0x4000);
        assert_eq!(primary.end_offset_bytes(), Some(0x8000));
        assert_eq!(primary.fs_type, 1);
        assert_eq!(primary.crypt_type, 0);
        assert_eq!(primary.id, 0x1122_3344_5566_7788);

        let secondary = parsed.partition(1).unwrap();
        assert_eq!(secondary.offset_bytes, 0x8000);
        assert_eq!(secondary.size_bytes, 0x2000);
        assert!(parsed.partition(2).is_none());
    }

    #[test]
    fn applies_media_unit_shift_from_flags() {
        let mut data = valid_header();
        write_u32(&mut data, MEDIA_SIZE_OFFSET, 0x20);
        data[FLAGS_OFFSET + 6] = 2;
        write_partition(&mut data, 0, 8, 8);
        write_partition(&mut data, 1, 16, 4);

        let parsed = NcsdHeader::parse(&data, 0x1_0000).unwrap();
        assert_eq!(parsed.media_unit_size, 0x800);
        assert_eq!(parsed.image_size_bytes, 0x1_0000);
        assert_eq!(parsed.primary_partition().unwrap().offset_bytes, 0x4000);
    }

    #[test]
    fn accepts_trimmed_image_when_all_partitions_are_present() {
        let data = valid_header();
        let parsed = NcsdHeader::parse(&data, 0xA000).unwrap();
        assert_eq!(parsed.partitions.len(), 2);
        assert!(parsed.image_size_bytes > 0xA000);
    }

    #[test]
    fn rejects_truncated_header_and_bad_magic() {
        assert!(matches!(
            NcsdHeader::parse(&[0; HEADER_SIZE - 1], 0x1_0000),
            Err(Error::InvalidNcsdHeader("header is truncated"))
        ));

        let mut data = valid_header();
        data[MAGIC_OFFSET] = b'X';
        assert!(matches!(
            NcsdHeader::parse(&data, 0x1_0000),
            Err(Error::InvalidNcsdHeader("NCSD magic is missing"))
        ));
    }

    #[test]
    fn rejects_media_unit_shift_overflow() {
        let mut data = valid_header();
        data[FLAGS_OFFSET + 6] = 55;
        assert!(matches!(
            NcsdHeader::parse(&data, u64::MAX),
            Err(Error::InvalidNcsdHeader("media-unit size overflows"))
        ));
    }

    #[test]
    fn rejects_half_empty_partition_entries() {
        let mut data = valid_header();
        write_partition(&mut data, 2, 0x60, 0);
        assert!(matches!(
            NcsdHeader::parse(&data, 0x1_0000),
            Err(Error::InvalidNcsdPartition { index: 2, .. })
        ));
    }

    #[test]
    fn rejects_partition_outside_declared_image_or_file() {
        let mut data = valid_header();
        write_partition(&mut data, 2, 0x7F, 2);
        assert!(matches!(
            NcsdHeader::parse(&data, 0x1_0000),
            Err(Error::InvalidNcsdPartition { index: 2, .. })
        ));

        let data = valid_header();
        assert!(matches!(
            NcsdHeader::parse(&data, 0x8FFF),
            Err(Error::InvalidNcsdPartition { index: 1, .. })
        ));
    }

    #[test]
    fn rejects_overlapping_partitions() {
        let mut data = valid_header();
        write_partition(&mut data, 1, 0x3F, 0x10);
        assert!(matches!(
            NcsdHeader::parse(&data, 0x1_0000),
            Err(Error::OverlappingNcsdPartitions {
                first: 0,
                second: 1
            })
        ));
    }
}
