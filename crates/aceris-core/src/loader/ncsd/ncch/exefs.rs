//! Structural parser for a Nintendo 3DS ExeFS header.
//!
//! The fixed header has ten 16-byte file slots, 0x20 reserved bytes, and ten
//! SHA-256 digests in reverse slot order. Offsets in each file slot are relative
//! to the first byte after the 0x200-byte ExeFS header.
//!
//! This parser validates metadata only. It does not decrypt NCCH content,
//! extract files, or calculate/verify file hashes.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

pub const HEADER_SIZE: usize = 0x200;
pub const FILE_ENTRY_COUNT: usize = 10;

const FILE_ENTRY_SIZE: usize = 0x10;
const FILE_TABLE_OFFSET: usize = 0x00;
const HASH_TABLE_OFFSET: usize = 0xC0;
const HASH_SIZE: usize = 0x20;
const HASH_SLOT_COUNT: usize = FILE_ENTRY_COUNT;

/// A file declared by one ExeFS header slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExeFsFile {
    /// Original slot in the ExeFS file table (0 through 9).
    pub entry_index: u8,
    /// ASCII filename without its trailing NUL padding.
    pub name: String,
    /// Absolute offset from the start of ExeFS, including its 0x200-byte header.
    pub offset_bytes: u64,
    /// File length in bytes.
    pub size_bytes: u64,
    /// SHA-256 digest stored in the header. The digest is not verified here.
    pub sha256: [u8; HASH_SIZE],
}

impl ExeFsFile {
    /// Returns the exclusive end offset, or None if a caller-built value overflows.
    pub const fn end_offset_bytes(&self) -> Option<u64> {
        self.offset_bytes.checked_add(self.size_bytes)
    }

    /// Opens a reader restricted to this file's declared byte range.
    ///
    /// The supplied reader must expose the ExeFS region starting at offset zero.
    /// Its visible length is checked before the file reader is created.
    pub fn reader<R: Read + Seek>(self, inner: R) -> io::Result<ExeFsFileReader<R>> {
        ExeFsFileReader::new(inner, self)
    }
}

/// A seekable reader restricted to one ExeFS file.
#[derive(Debug)]
pub struct ExeFsFileReader<R> {
    inner: R,
    start: u64,
    size: u64,
    position: u64,
}

impl<R: Read + Seek> ExeFsFileReader<R> {
    fn new(mut inner: R, file: ExeFsFile) -> io::Result<Self> {
        if file.size_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ExeFS file has zero length",
            ));
        }

        let end = file.end_offset_bytes().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "ExeFS file range overflows")
        })?;
        let available = inner.seek(SeekFrom::End(0))?;
        if end > available {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "ExeFS file extends beyond the supplied region",
            ));
        }

        let actual = inner.seek(SeekFrom::Start(file.offset_bytes))?;
        if actual != file.offset_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested ExeFS file",
            ));
        }

        Ok(Self {
            inner,
            start: file.offset_bytes,
            size: file.size_bytes,
            position: 0,
        })
    }

    /// Returns the visible length of the file.
    pub const fn len(&self) -> u64 {
        self.size
    }

    /// Returns true when the file has no bytes.
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the current position relative to the beginning of the file.
    pub const fn position(&self) -> u64 {
        self.position
    }
}

impl<R: Read + Seek> Read for ExeFsFileReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position == self.size {
            return Ok(0);
        }

        let remaining = self.size - self.position;
        let readable = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let bytes_read = self.inner.read(&mut buffer[..readable])?;
        self.position += bytes_read as u64;
        Ok(bytes_read)
    }
}

impl<R: Read + Seek> Seek for ExeFsFileReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
        };

        if !(0..=i128::from(self.size)).contains(&target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek is outside the ExeFS file",
            ));
        }

        let relative = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative ExeFS file position is invalid",
            )
        })?;
        let absolute = self.start.checked_add(relative).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute ExeFS file position overflows",
            )
        })?;

        let actual = self.inner.seek(SeekFrom::Start(absolute))?;
        if actual != absolute {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested file position",
            ));
        }

        self.position = relative;
        Ok(relative)
    }
}

/// Errors produced while parsing an ExeFS header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExeFsError {
    HeaderTruncated,
    RegionTooSmall,
    InvalidHashRegion,
    SuperblockHashMismatch,
    FileHashMismatch(String),
    ReaderFailure(String),
    InvalidEntry { index: usize, reason: &'static str },
    DuplicateFileName(String),
    OverlappingFiles { first: String, second: String },
}

impl fmt::Display for ExeFsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTruncated => write!(formatter, "ExeFS header is truncated"),
            Self::RegionTooSmall => write!(formatter, "ExeFS region is smaller than its header"),
            Self::InvalidHashRegion => write!(formatter, "ExeFS hash region is empty"),
            Self::SuperblockHashMismatch => {
                write!(
                    formatter,
                    "NCCH ExeFS superblock SHA-256 hash does not match"
                )
            }
            Self::FileHashMismatch(name) => {
                write!(formatter, "ExeFS file hash mismatch: {name}")
            }
            Self::ReaderFailure(reason) => {
                write!(formatter, "could not read ExeFS header: {reason}")
            }
            Self::InvalidEntry { index, reason } => {
                write!(formatter, "invalid ExeFS file entry {index}: {reason}")
            }
            Self::DuplicateFileName(name) => {
                write!(formatter, "duplicate ExeFS filename: {name}")
            }
            Self::OverlappingFiles { first, second } => {
                write!(formatter, "ExeFS files {first} and {second} overlap")
            }
        }
    }
}

impl std::error::Error for ExeFsError {}

/// Validated file metadata from an ExeFS header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExeFsHeader {
    /// Total size of the containing ExeFS region.
    pub region_size_bytes: u64,
    /// Populated entries in original header-slot order.
    pub files: Vec<ExeFsFile>,
}

impl ExeFsHeader {
    /// Parses a fixed header and validates file ranges against the ExeFS region.
    ///
    /// `region_size_bytes` includes the fixed 0x200-byte ExeFS header.
    pub fn parse(data: &[u8], region_size_bytes: u64) -> Result<Self, ExeFsError> {
        if data.len() < HEADER_SIZE {
            return Err(ExeFsError::HeaderTruncated);
        }
        if region_size_bytes < HEADER_SIZE as u64 {
            return Err(ExeFsError::RegionTooSmall);
        }

        let data_region_size = region_size_bytes - HEADER_SIZE as u64;
        let mut files = Vec::with_capacity(FILE_ENTRY_COUNT);

        for index in 0..FILE_ENTRY_COUNT {
            let entry_offset = FILE_TABLE_OFFSET + index * FILE_ENTRY_SIZE;
            let raw_name =
                read_array::<8>(data, entry_offset).ok_or(ExeFsError::HeaderTruncated)?;
            let offset = read_u32_le(data, entry_offset + 8).ok_or(ExeFsError::HeaderTruncated)?;
            let size = read_u32_le(data, entry_offset + 12).ok_or(ExeFsError::HeaderTruncated)?;

            if raw_name.iter().all(|byte| *byte == 0) && offset == 0 && size == 0 {
                continue;
            }

            let name = parse_file_name(&raw_name)
                .map_err(|reason| ExeFsError::InvalidEntry { index, reason })?;
            if size == 0 {
                return Err(ExeFsError::InvalidEntry {
                    index,
                    reason: "file size is zero",
                });
            }

            let end =
                u64::from(offset)
                    .checked_add(u64::from(size))
                    .ok_or(ExeFsError::InvalidEntry {
                        index,
                        reason: "file range overflows",
                    })?;
            if end > data_region_size {
                return Err(ExeFsError::InvalidEntry {
                    index,
                    reason: "file extends beyond the ExeFS region",
                });
            }

            let hash_slot = HASH_SLOT_COUNT - 1 - index;
            let hash_offset = HASH_TABLE_OFFSET + hash_slot * HASH_SIZE;
            let sha256 =
                read_array::<HASH_SIZE>(data, hash_offset).ok_or(ExeFsError::HeaderTruncated)?;

            let offset_bytes = (HEADER_SIZE as u64).checked_add(u64::from(offset)).ok_or(
                ExeFsError::InvalidEntry {
                    index,
                    reason: "absolute file offset overflows",
                },
            )?;

            if files.iter().any(|file: &ExeFsFile| file.name == name) {
                return Err(ExeFsError::DuplicateFileName(name));
            }

            files.push(ExeFsFile {
                entry_index: index as u8,
                name,
                offset_bytes,
                size_bytes: u64::from(size),
                sha256,
            });
        }

        let mut ranges: Vec<&ExeFsFile> = files.iter().collect();
        ranges.sort_unstable_by_key(|file| file.offset_bytes);
        for pair in ranges.windows(2) {
            let previous = pair[0];
            let current = pair[1];
            let previous_end = previous
                .end_offset_bytes()
                .ok_or(ExeFsError::InvalidEntry {
                    index: usize::from(previous.entry_index),
                    reason: "file end offset overflows",
                })?;
            if current.offset_bytes < previous_end {
                return Err(ExeFsError::OverlappingFiles {
                    first: previous.name.clone(),
                    second: current.name.clone(),
                });
            }
        }

        Ok(Self {
            region_size_bytes,
            files,
        })
    }

    /// Reads exactly the fixed header from the current reader position.
    ///
    /// The reader should already be positioned at the beginning of ExeFS data.
    pub fn parse_reader<R: Read>(
        reader: &mut R,
        region_size_bytes: u64,
    ) -> Result<Self, ExeFsError> {
        let mut header = [0; HEADER_SIZE];
        reader
            .read_exact(&mut header)
            .map_err(|error| ExeFsError::ReaderFailure(error.to_string()))?;
        Self::parse(&header, region_size_bytes)
    }

    /// Looks up a file by its exact, case-sensitive ExeFS name.
    pub fn file(&self, name: &str) -> Option<&ExeFsFile> {
        self.files.iter().find(|file| file.name == name)
    }
}

fn parse_file_name(raw: &[u8; 8]) -> Result<String, &'static str> {
    let name_end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    if name_end == 0 {
        return Err("filename is empty");
    }
    if raw[name_end..].iter().any(|byte| *byte != 0) {
        return Err("filename contains data after its NUL terminator");
    }

    let name_bytes = &raw[..name_end];
    if name_bytes
        .iter()
        .any(|byte| !byte.is_ascii_graphic() || *byte == b'/' || *byte == b'\\')
    {
        return Err("filename contains a non-printable or path-separator byte");
    }

    // All bytes were checked as printable ASCII above.
    Ok(String::from_utf8(name_bytes.to_vec()).expect("printable ASCII is valid UTF-8"))
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    read_array::<4>(data, offset).map(u32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Seek, SeekFrom};

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_entry(
        data: &mut [u8],
        index: usize,
        name: &[u8],
        offset: u32,
        size: u32,
        hash_fill: u8,
    ) {
        let base = FILE_TABLE_OFFSET + index * FILE_ENTRY_SIZE;
        data[base..base + name.len()].copy_from_slice(name);
        write_u32(data, base + 8, offset);
        write_u32(data, base + 12, size);

        let hash_slot = HASH_SLOT_COUNT - 1 - index;
        let hash_offset = HASH_TABLE_OFFSET + hash_slot * HASH_SIZE;
        data[hash_offset..hash_offset + HASH_SIZE].fill(hash_fill);
    }

    fn write_name(data: &mut [u8], base: usize, name: &[u8]) {
        let mut field = [0; 8];
        field[..name.len()].copy_from_slice(name);
        data[base..base + 8].copy_from_slice(&field);
    }

    fn valid_header() -> Vec<u8> {
        let mut data = vec![0; HEADER_SIZE];
        write_entry(&mut data, 0, b".code", 0, 0x100, 0xA0);
        write_entry(&mut data, 1, b"icon", 0x200, 0x20, 0xA1);
        write_entry(&mut data, 9, b"logo", 0x300, 0x30, 0xA9);
        data
    }

    #[test]
    fn parses_files_and_reverse_order_hash_slots() {
        let data = valid_header();
        let parsed = ExeFsHeader::parse(&data, 0x600).unwrap();

        assert_eq!(parsed.region_size_bytes, 0x600);
        assert_eq!(parsed.files.len(), 3);

        let code = parsed.file(".code").unwrap();
        assert_eq!(code.entry_index, 0);
        assert_eq!(code.offset_bytes, 0x200);
        assert_eq!(code.size_bytes, 0x100);
        assert_eq!(code.sha256, [0xA0; HASH_SIZE]);

        let icon = parsed.file("icon").unwrap();
        assert_eq!(icon.entry_index, 1);
        assert_eq!(icon.offset_bytes, 0x400);
        assert_eq!(icon.sha256, [0xA1; HASH_SIZE]);

        let logo = parsed.file("logo").unwrap();
        assert_eq!(logo.entry_index, 9);
        assert_eq!(logo.offset_bytes, 0x500);
        assert_eq!(logo.sha256, [0xA9; HASH_SIZE]);
        assert!(parsed.file("ICON").is_none());
    }

    #[test]
    fn supports_all_ten_header_slots() {
        let mut data = vec![0; HEADER_SIZE];
        for index in 0..FILE_ENTRY_COUNT {
            let name = [b'A' + index as u8];
            write_entry(
                &mut data,
                index,
                &name,
                (index as u32) * 0x20,
                0x10,
                index as u8,
            );
        }

        let parsed = ExeFsHeader::parse(&data, 0x600).unwrap();
        assert_eq!(parsed.files.len(), FILE_ENTRY_COUNT);
        assert_eq!(parsed.files[9].entry_index, 9);
        assert_eq!(parsed.files[9].sha256, [9; HASH_SIZE]);
    }

    #[test]
    fn accepts_an_empty_exefs_region_header() {
        let data = vec![0; HEADER_SIZE];
        let parsed = ExeFsHeader::parse(&data, HEADER_SIZE as u64).unwrap();
        assert!(parsed.files.is_empty());
    }

    #[test]
    fn parses_header_from_a_reader_without_loading_file_data() {
        let data = valid_header();
        let mut reader = Cursor::new(data);
        let parsed = ExeFsHeader::parse_reader(&mut reader, 0x600).unwrap();

        assert_eq!(reader.position(), HEADER_SIZE as u64);
        assert_eq!(parsed.file(".code").unwrap().size_bytes, 0x100);
    }

    #[test]
    fn rejects_truncated_headers_and_regions_smaller_than_header() {
        assert_eq!(
            ExeFsHeader::parse(&[0; HEADER_SIZE - 1], 0x600),
            Err(ExeFsError::HeaderTruncated)
        );
        assert_eq!(
            ExeFsHeader::parse(&[0; HEADER_SIZE], (HEADER_SIZE - 1) as u64),
            Err(ExeFsError::RegionTooSmall)
        );

        let mut reader = Cursor::new(vec![0; HEADER_SIZE - 1]);
        assert!(matches!(
            ExeFsHeader::parse_reader(&mut reader, 0x600),
            Err(ExeFsError::ReaderFailure(_))
        ));
    }

    #[test]
    fn rejects_invalid_and_partially_empty_entries() {
        let mut data = valid_header();
        let base = FILE_TABLE_OFFSET + 2 * FILE_ENTRY_SIZE;
        data[base..base + 8].copy_from_slice(b"../bad!!");
        write_u32(&mut data, base + 8, 0x100);
        write_u32(&mut data, base + 12, 0x10);
        assert!(matches!(
            ExeFsHeader::parse(&data, 0x600),
            Err(ExeFsError::InvalidEntry { index: 2, .. })
        ));

        data = valid_header();
        let base = FILE_TABLE_OFFSET + 2 * FILE_ENTRY_SIZE;
        data[base..base + 8].copy_from_slice(b"unused\0X");
        write_u32(&mut data, base + 8, 0x100);
        write_u32(&mut data, base + 12, 0x10);
        assert!(matches!(
            ExeFsHeader::parse(&data, 0x600),
            Err(ExeFsError::InvalidEntry { index: 2, .. })
        ));

        data = valid_header();
        let base = FILE_TABLE_OFFSET + 2 * FILE_ENTRY_SIZE;
        write_name(&mut data, base, b"zero");
        assert!(matches!(
            ExeFsHeader::parse(&data, 0x600),
            Err(ExeFsError::InvalidEntry {
                index: 2,
                reason: "file size is zero"
            })
        ));
    }

    #[test]
    fn rejects_duplicate_names_out_of_bounds_and_overlaps() {
        let mut data = valid_header();
        let base = FILE_TABLE_OFFSET + 2 * FILE_ENTRY_SIZE;
        write_name(&mut data, base, b"icon");
        write_u32(&mut data, base + 8, 0x300);
        write_u32(&mut data, base + 12, 0x10);
        assert_eq!(
            ExeFsHeader::parse(&data, 0x600),
            Err(ExeFsError::DuplicateFileName("icon".to_owned()))
        );

        data = valid_header();
        let base = FILE_TABLE_OFFSET + 2 * FILE_ENTRY_SIZE;
        write_name(&mut data, base, b"extra");
        write_u32(&mut data, base + 8, 0x3F0);
        write_u32(&mut data, base + 12, 0x20);
        assert!(matches!(
            ExeFsHeader::parse(&data, 0x600),
            Err(ExeFsError::InvalidEntry {
                index: 2,
                reason: "file extends beyond the ExeFS region"
            })
        ));

        data = valid_header();
        let base = FILE_TABLE_OFFSET + 2 * FILE_ENTRY_SIZE;
        write_name(&mut data, base, b"overlap");
        write_u32(&mut data, base + 8, 0x80);
        write_u32(&mut data, base + 12, 0x20);
        assert!(matches!(
            ExeFsHeader::parse(&data, 0x600),
            Err(ExeFsError::OverlappingFiles { .. })
        ));
    }

    #[test]
    fn file_reader_cannot_read_or_seek_past_the_file() {
        let parsed = ExeFsHeader::parse(&valid_header(), 0x600).unwrap();
        let mut image = vec![0xEE; 0x600];
        image[0x200..0x300].copy_from_slice(&(0..0x100).map(|byte| byte as u8).collect::<Vec<_>>());

        let file = parsed.file(".code").unwrap().clone();
        let mut reader = file.reader(Cursor::new(image)).unwrap();
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();

        assert_eq!(
            actual,
            (0..0x100).map(|byte| byte as u8).collect::<Vec<_>>()
        );
        assert_eq!(reader.len(), 0x100);
        assert_eq!(reader.position(), 0x100);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);

        assert_eq!(reader.seek(SeekFrom::End(-4)).unwrap(), 0xFC);
        let mut tail = [0; 8];
        assert_eq!(reader.read(&mut tail).unwrap(), 4);
        assert_eq!(&tail[..4], &[0xFC, 0xFD, 0xFE, 0xFF]);
    }

    #[test]
    fn file_reader_rejects_invalid_caller_supplied_ranges() {
        let empty = ExeFsFile {
            entry_index: 0,
            name: "empty".to_owned(),
            offset_bytes: 0,
            size_bytes: 0,
            sha256: [0; HASH_SIZE],
        };
        assert_eq!(
            empty.reader(Cursor::new(vec![0; 8])).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );

        let overflowing = ExeFsFile {
            entry_index: 0,
            name: "overflow".to_owned(),
            offset_bytes: u64::MAX - 3,
            size_bytes: 8,
            sha256: [0; HASH_SIZE],
        };
        assert_eq!(
            overflowing
                .reader(Cursor::new(vec![0; 8]))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        let outside = ExeFsFile {
            entry_index: 0,
            name: "outside".to_owned(),
            offset_bytes: 4,
            size_bytes: 8,
            sha256: [0; HASH_SIZE],
        };
        assert_eq!(
            outside.reader(Cursor::new(vec![0; 8])).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn accepts_file_ending_at_exact_region_boundary() {
        let mut data = vec![0; HEADER_SIZE];
        write_entry(&mut data, 0, b"last", 0x100, 0x100, 0x55);

        let parsed = ExeFsHeader::parse(&data, 0x400).unwrap();
        assert_eq!(parsed.file("last").unwrap().end_offset_bytes(), Some(0x400));
    }
}
