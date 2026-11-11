//! Streaming access to the sections and content entries of a CIA archive.
//!
//! The archive reader keeps the original seekable source and exposes each
//! selected content as a bounded stream. It never buffers the complete CIA
//! content section in memory.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

use crate::loader::ncsd::ncch::{NcchError, NcchHeader};
use crate::loader::ncsd::ncch::romfs::sha256::Sha256;

use super::tmd::{CONTENT_TYPE_ENCRYPTED, MAX_TMD_SIZE_BYTES, TmdError, TmdHeader};
use super::{CiaHeader, HEADER_SIZE};

const CONTENT_ALIGNMENT: u64 = 0x40;
const HASH_BUFFER_SIZE: usize = 0x1_0000;

/// Location of one TMD content entry inside the CIA content section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiaContentLocation {
    pub content_id: u32,
    pub content_index: u16,
    pub content_type: u16,
    pub offset_bytes: u64,
    pub size_bytes: u64,
}

/// Errors returned while opening, reading, or verifying a CIA archive.
#[derive(Debug)]
pub enum CiaArchiveError {
    Reader(io::Error),
    InvalidHeader(io::Error),
    InvalidTmd(TmdError),
    InvalidNcchHeader(NcchError),
    TmdSectionTooLarge {
        declared_size: u64,
        maximum_size: usize,
    },
    AllocationFailed,
    ContentNotIncluded(u16),
    ContentRequiresDecryption(u16),
    ContentStreamHasTrailingBytes(u16),
    ContentRangeOutsideSection {
        content_index: u16,
    },
    ContentTruncated {
        content_index: u16,
        expected_size: u64,
        actual_size: u64,
    },
    ContentHashMismatch {
        content_index: u16,
    },
    HashInputTooLong,
}

impl fmt::Display for CiaArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(error) => write!(formatter, "CIA reader failed: {error}"),
            Self::InvalidHeader(error) => write!(formatter, "invalid CIA header: {error}"),
            Self::InvalidTmd(error) => write!(formatter, "invalid CIA title metadata: {error}"),
            Self::InvalidNcchHeader(error) => {
                write!(formatter, "invalid NCCH content in CIA archive: {error}")
            }
            Self::TmdSectionTooLarge {
                declared_size,
                maximum_size,
            } => write!(
                formatter,
                "CIA TMD section size {declared_size} exceeds the structural limit of {maximum_size} bytes"
            ),
            Self::AllocationFailed => {
                formatter.write_str("could not allocate memory for CIA title metadata")
            }
            Self::ContentNotIncluded(index) => {
                write!(formatter, "CIA does not contain content index {index}")
            }
            Self::ContentRequiresDecryption(index) => write!(
                formatter,
                "CIA content index {index} is encrypted; decrypt its AES-CBC layer before checking the TMD hash"
            ),
            Self::ContentStreamHasTrailingBytes(index) => write!(
                formatter,
                "content stream for CIA index {index} contains bytes beyond the TMD-declared size"
            ),
            Self::ContentRangeOutsideSection { content_index } => write!(
                formatter,
                "CIA content index {content_index} extends outside the declared content section"
            ),
            Self::ContentTruncated {
                content_index,
                expected_size,
                actual_size,
            } => write!(
                formatter,
                "CIA content index {content_index} is truncated: read {actual_size} of {expected_size} bytes"
            ),
            Self::ContentHashMismatch { content_index } => write!(
                formatter,
                "CIA content index {content_index} does not match its TMD SHA-256 digest"
            ),
            Self::HashInputTooLong => {
                formatter.write_str("CIA content is too large for SHA-256 input length")
            }
        }
    }
}

impl std::error::Error for CiaArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reader(error) | Self::InvalidHeader(error) => Some(error),
            Self::InvalidTmd(error) => Some(error),
            Self::InvalidNcchHeader(error) => Some(error),
            _ => None,
        }
    }
}

/// An opened CIA archive with validated metadata and lazy content access.
///
/// Opening reads the fixed CIA header and its TMD section only. Content bytes
/// remain in the caller-provided source until a content reader is requested.
#[derive(Debug)]
pub struct CiaArchiveReader<R> {
    reader: R,
    header: CiaHeader,
    title_metadata: TmdHeader,
    contents: Vec<CiaContentLocation>,
}

impl<R: Read + Seek> CiaArchiveReader<R> {
    /// Opens an archive, validates its header and TMD, and computes the
    /// byte-range of every content selected by the CIA content-index bitmap.
    pub fn open(mut reader: R) -> Result<Self, CiaArchiveError> {
        let file_len = reader
            .seek(SeekFrom::End(0))
            .map_err(CiaArchiveError::Reader)?;
        let start = reader
            .seek(SeekFrom::Start(0))
            .map_err(CiaArchiveError::Reader)?;
        if start != 0 {
            return Err(CiaArchiveError::Reader(io::Error::new(
                io::ErrorKind::InvalidData,
                "source did not seek to the beginning of the CIA archive",
            )));
        }

        let mut header_bytes = [0u8; HEADER_SIZE];
        reader
            .read_exact(&mut header_bytes)
            .map_err(CiaArchiveError::Reader)?;
        let header =
            CiaHeader::parse(&header_bytes, file_len).map_err(CiaArchiveError::InvalidHeader)?;

        let tmd_section = header.title_metadata;
        if tmd_section.size_bytes > MAX_TMD_SIZE_BYTES as u64 {
            return Err(CiaArchiveError::TmdSectionTooLarge {
                declared_size: tmd_section.size_bytes,
                maximum_size: MAX_TMD_SIZE_BYTES,
            });
        }
        let tmd_size = usize::try_from(tmd_section.size_bytes).map_err(|_| {
            CiaArchiveError::TmdSectionTooLarge {
                declared_size: tmd_section.size_bytes,
                maximum_size: MAX_TMD_SIZE_BYTES,
            }
        })?;

        let mut tmd_bytes = Vec::new();
        tmd_bytes
            .try_reserve_exact(tmd_size)
            .map_err(|_| CiaArchiveError::AllocationFailed)?;
        tmd_bytes.resize(tmd_size, 0);

        let actual_tmd_position = reader
            .seek(SeekFrom::Start(tmd_section.offset_bytes))
            .map_err(CiaArchiveError::Reader)?;
        if actual_tmd_position != tmd_section.offset_bytes {
            return Err(CiaArchiveError::Reader(io::Error::new(
                io::ErrorKind::InvalidData,
                "source did not seek to the CIA TMD section",
            )));
        }
        reader
            .read_exact(&mut tmd_bytes)
            .map_err(CiaArchiveError::Reader)?;

        let title_metadata = header
            .parse_title_metadata(&tmd_bytes)
            .map_err(CiaArchiveError::InvalidTmd)?;
        let contents = build_content_locations(&header, &title_metadata)?;

        Ok(Self {
            reader,
            header,
            title_metadata,
            contents,
        })
    }

    /// Returns the validated fixed CIA header.
    pub const fn header(&self) -> &CiaHeader {
        &self.header
    }

    /// Returns the parsed TMD and its validated content tables.
    pub const fn title_metadata(&self) -> &TmdHeader {
        &self.title_metadata
    }

    /// Returns the locations of CIA-selected contents, ordered by content index.
    pub fn contents(&self) -> &[CiaContentLocation] {
        &self.contents
    }

    /// Returns the location of a selected content index.
    pub fn content_location(&self, content_index: u16) -> Option<CiaContentLocation> {
        self.contents
            .binary_search_by_key(&content_index, |content| content.content_index)
            .ok()
            .map(|position| self.contents[position])
    }

    /// Opens a selected content as a stream bounded to its exact byte range.
    ///
    /// The returned reader borrows this archive mutably, preventing another
    /// operation from changing the underlying cursor while the stream is open.
    pub fn open_content_reader(
        &mut self,
        content_index: u16,
    ) -> Result<Option<CiaContentReader<'_, R>>, CiaArchiveError> {
        let Some(location) = self.content_location(content_index) else {
            return Ok(None);
        };

        let actual = self
            .reader
            .seek(SeekFrom::Start(location.offset_bytes))
            .map_err(CiaArchiveError::Reader)?;
        if actual != location.offset_bytes {
            return Err(CiaArchiveError::Reader(io::Error::new(
                io::ErrorKind::InvalidData,
                "source did not seek to the selected CIA content",
            )));
        }

        Ok(Some(CiaContentReader {
            reader: &mut self.reader,
            start_bytes: location.offset_bytes,
            size_bytes: location.size_bytes,
            position_bytes: 0,
        }))
    }

    /// Verifies and opens the NCCH header of a selected, cleartext CIA content.
    ///
    /// This checks the TMD SHA-256 first, then parses the NCCH header from the
    /// bounded content reader. It does not decrypt the outer CIA AES-CBC layer
    /// or NCCH-internal regions; encrypted CIA contents are rejected.
    pub fn open_verified_ncch_content_reader(
        &mut self,
        content_index: u16,
    ) -> Result<Option<CiaContentNcchReader<'_, R>>, CiaArchiveError> {
        let Some(location) = self.content_location(content_index) else {
            return Ok(None);
        };

        self.verify_content(content_index)?;
        let mut reader = self
            .open_content_reader(content_index)?
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
        let header = NcchHeader::parse_reader(&mut reader, location.size_bytes)
            .map_err(CiaArchiveError::InvalidNcchHeader)?;
        reader
            .seek(SeekFrom::Start(0))
            .map_err(CiaArchiveError::Reader)?;

        Ok(Some(CiaContentNcchReader {
            location,
            header,
            reader,
        }))
    }

    /// Verifies selected content directly from the CIA when it is not encrypted.
    ///
    /// TMD hashes apply after the outer CIA AES-CBC layer has been removed.
    /// Encrypted entries must use `verify_content_stream` with a stream
    /// supplied by the caller after decryption.
    pub fn verify_content(&mut self, content_index: u16) -> Result<(), CiaArchiveError> {
        let record = self
            .title_metadata
            .content_by_index(content_index)
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
        let expected_size = record.size_bytes;
        let expected_hash = record.sha256;
        let content_type = record.content_type;

        if self.content_location(content_index).is_none() {
            return Err(CiaArchiveError::ContentNotIncluded(content_index));
        }
        if content_type & CONTENT_TYPE_ENCRYPTED != 0 {
            return Err(CiaArchiveError::ContentRequiresDecryption(content_index));
        }

        let mut content_reader = self
            .open_content_reader(content_index)?
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
        verify_content_stream_bytes(
            content_index,
            expected_size,
            expected_hash,
            &mut content_reader,
        )
    }

    /// Verifies a caller-provided content stream against its TMD size and hash.
    ///
    /// The supplied stream must expose the bytes after any outer CIA AES-CBC
    /// decryption. The entire stream is checked with bounded memory; streams
    /// shorter or longer than the TMD-declared size are rejected.
    pub fn verify_content_stream<S: Read>(
        &self,
        content_index: u16,
        mut stream: S,
    ) -> Result<(), CiaArchiveError> {
        let record = self
            .title_metadata
            .content_by_index(content_index)
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
        if self.content_location(content_index).is_none() {
            return Err(CiaArchiveError::ContentNotIncluded(content_index));
        }

        verify_content_stream_bytes(content_index, record.size_bytes, record.sha256, &mut stream)
    }
}

fn verify_content_stream_bytes<R: Read>(
    content_index: u16,
    expected_size: u64,
    expected_hash: [u8; 32],
    reader: &mut R,
) -> Result<(), CiaArchiveError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; HASH_BUFFER_SIZE];
    let mut total_read = 0u64;

    while total_read < expected_size {
        let remaining = expected_size - total_read;
        let requested = usize::try_from(remaining.min(HASH_BUFFER_SIZE as u64))
            .expect("bounded by the fixed hash buffer");
        let read = reader
            .read(&mut buffer[..requested])
            .map_err(CiaArchiveError::Reader)?;
        if read == 0 {
            return Err(CiaArchiveError::ContentTruncated {
                content_index,
                expected_size,
                actual_size: total_read,
            });
        }

        hasher
            .update(&buffer[..read])
            .map_err(|_| CiaArchiveError::HashInputTooLong)?;
        total_read += read as u64;
    }

    let mut trailing_byte = [0u8; 1];
    if reader
        .read(&mut trailing_byte)
        .map_err(CiaArchiveError::Reader)?
        != 0
    {
        return Err(CiaArchiveError::ContentStreamHasTrailingBytes(
            content_index,
        ));
    }

    if hasher.finalize() != expected_hash {
        return Err(CiaArchiveError::ContentHashMismatch { content_index });
    }
    Ok(())
}

fn build_content_locations(
    header: &CiaHeader,
    title_metadata: &TmdHeader,
) -> Result<Vec<CiaContentLocation>, CiaArchiveError> {
    let mut selected = title_metadata
        .contents
        .iter()
        .filter(|content| header.content_index_is_present(content.content_index))
        .collect::<Vec<_>>();
    selected.sort_unstable_by_key(|content| content.content_index);

    let content_section = header.content;
    let section_end =
        content_section
            .end_offset_bytes()
            .ok_or(CiaArchiveError::ContentRangeOutsideSection {
                content_index: selected.first().map_or(0, |content| content.content_index),
            })?;

    let mut locations = Vec::with_capacity(selected.len());
    let mut next_offset = content_section.offset_bytes;

    for (position, record) in selected.iter().enumerate() {
        let end_offset = next_offset.checked_add(record.size_bytes).ok_or(
            CiaArchiveError::ContentRangeOutsideSection {
                content_index: record.content_index,
            },
        )?;
        if next_offset > section_end || end_offset > section_end {
            return Err(CiaArchiveError::ContentRangeOutsideSection {
                content_index: record.content_index,
            });
        }

        locations.push(CiaContentLocation {
            content_id: record.content_id,
            content_index: record.content_index,
            content_type: record.content_type,
            offset_bytes: next_offset,
            size_bytes: record.size_bytes,
        });

        if position + 1 < selected.len() {
            next_offset =
                align_content(end_offset).ok_or(CiaArchiveError::ContentRangeOutsideSection {
                    content_index: record.content_index,
                })?;
        }
    }

    Ok(locations)
}

fn align_content(offset: u64) -> Option<u64> {
    offset
        .checked_add(CONTENT_ALIGNMENT - 1)
        .map(|aligned| aligned & !(CONTENT_ALIGNMENT - 1))
}

/// A TMD-verified NCCH content paired with its bounded source reader.
#[derive(Debug)]
pub struct CiaContentNcchReader<'a, R> {
    location: CiaContentLocation,
    header: NcchHeader,
    reader: CiaContentReader<'a, R>,
}

impl<R: Read + Seek> CiaContentNcchReader<'_, R> {
    /// Returns the TMD content location.
    pub const fn location(&self) -> CiaContentLocation {
        self.location
    }

    /// Returns the structurally validated NCCH header.
    pub const fn header(&self) -> &NcchHeader {
        &self.header
    }

    /// Returns the bounded reader, positioned at the beginning of the NCCH.
    pub fn reader_mut(&mut self) -> &mut CiaContentReader<'_, R> {
        &mut self.reader
    }
}

/// A seekable reader restricted to one CIA content byte range.
#[derive(Debug)]
pub struct CiaContentReader<'a, R> {
    reader: &'a mut R,
    start_bytes: u64,
    size_bytes: u64,
    position_bytes: u64,
}

impl<R: Read + Seek> CiaContentReader<'_, R> {
    /// Returns the declared content length.
    pub const fn len(&self) -> u64 {
        self.size_bytes
    }

    /// Returns true when the content has no bytes.
    pub const fn is_empty(&self) -> bool {
        self.size_bytes == 0
    }

    /// Returns the current position relative to the content start.
    pub const fn position(&self) -> u64 {
        self.position_bytes
    }
}

impl<R: Read + Seek> Read for CiaContentReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position_bytes == self.size_bytes {
            return Ok(0);
        }

        let remaining = self.size_bytes - self.position_bytes;
        let readable = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let read = self.reader.read(&mut buffer[..readable])?;
        self.position_bytes += read as u64;
        Ok(read)
    }
}

impl<R: Read + Seek> Seek for CiaContentReader<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position_bytes) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.size_bytes) + i128::from(offset),
        };
        if !(0..=i128::from(self.size_bytes)).contains(&target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek is outside the CIA content range",
            ));
        }

        let relative = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "CIA content position is invalid",
            )
        })?;
        let absolute = self.start_bytes.checked_add(relative).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "CIA content absolute position overflows",
            )
        })?;
        let actual = self.reader.seek(SeekFrom::Start(absolute))?;
        if actual != absolute {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source did not seek to the requested CIA content position",
            ));
        }

        self.position_bytes = relative;
        Ok(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TEST_SIGNATURE_SIZE: usize = 0x140;
    const TEST_TMD_HEADER_SIZE: usize = 0xC4;
    const TEST_CONTENT_INFO_SIZE: usize = 0x24;
    const TEST_CONTENT_INFO_COUNT: usize = 64;
    const TEST_CONTENT_RECORD_SIZE: usize = 0x30;
    const TEST_CONTENT_INFO_OFFSET: usize = TEST_TMD_HEADER_SIZE;
    const TEST_CONTENT_RECORDS_OFFSET: usize =
        TEST_CONTENT_INFO_OFFSET + TEST_CONTENT_INFO_SIZE * TEST_CONTENT_INFO_COUNT;

    fn align(value: u64) -> u64 {
        (value + CONTENT_ALIGNMENT - 1) & !(CONTENT_ALIGNMENT - 1)
    }

    fn write_be_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn write_be_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn write_be_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }

    fn write_le_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_le_u64(data: &mut [u8], offset: usize, value: u64) {
        data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn make_tmd(contents: &[(u32, u16, u16, &[u8])]) -> Vec<u8> {
        let records_size = contents.len() * TEST_CONTENT_RECORD_SIZE;
        let mut data = vec![0u8; TEST_SIGNATURE_SIZE + TEST_CONTENT_RECORDS_OFFSET + records_size];
        write_be_u32(&mut data, 0, 0x0001_0004);

        let header_start = TEST_SIGNATURE_SIZE;
        let header = &mut data[header_start..header_start + TEST_TMD_HEADER_SIZE];
        write_be_u64(header, 0x4C, 0x0004_0000_1234_5678);
        write_be_u16(header, 0x9E, contents.len() as u16);
        if let Some((_, boot_index, _, _)) = contents.first() {
            write_be_u16(header, 0xA0, *boot_index);
        }

        let info_start = header_start + TEST_CONTENT_INFO_OFFSET;
        let records_start = header_start + TEST_CONTENT_RECORDS_OFFSET;
        if let Some((_, first_index, _, _)) = contents.first() {
            write_be_u16(&mut data, info_start, *first_index);
            write_be_u16(&mut data, info_start + 2, contents.len() as u16);
        }

        for (position, (content_id, content_index, content_type, bytes)) in
            contents.iter().enumerate()
        {
            let record_offset = records_start + position * TEST_CONTENT_RECORD_SIZE;
            write_be_u32(&mut data, record_offset, *content_id);
            write_be_u16(&mut data, record_offset + 4, *content_index);
            write_be_u16(&mut data, record_offset + 6, *content_type);
            write_be_u64(&mut data, record_offset + 8, bytes.len() as u64);
            let digest = Sha256::digest(bytes).unwrap();
            data[record_offset + 0x10..record_offset + 0x30].copy_from_slice(&digest);
        }

        if !contents.is_empty() {
            let group_hash =
                Sha256::digest(&data[records_start..records_start + records_size]).unwrap();
            data[info_start + 4..info_start + 0x24].copy_from_slice(&group_hash);
        }
        let info_hash = Sha256::digest(
            &data[info_start..info_start + TEST_CONTENT_INFO_SIZE * TEST_CONTENT_INFO_COUNT],
        )
        .unwrap();
        data[header_start + 0xA4..header_start + 0xC4].copy_from_slice(&info_hash);
        data
    }

    fn make_cia(contents: &[(u32, u16, u16, &[u8])]) -> (Vec<u8>, Vec<(u16, u64)>) {
        let tmd = make_tmd(contents);
        let tmd_offset = align(HEADER_SIZE as u64);
        let content_offset = align(tmd_offset + tmd.len() as u64);

        let mut ordered = contents.iter().collect::<Vec<_>>();
        ordered.sort_unstable_by_key(|content| content.1);

        let mut cursor = content_offset;
        let mut locations = Vec::with_capacity(ordered.len());
        for (position, entry) in ordered.iter().enumerate() {
            locations.push((entry.1, cursor));
            cursor += entry.3.len() as u64;
            if position + 1 < ordered.len() {
                cursor = align(cursor);
            }
        }
        let content_size = cursor - content_offset;
        let mut data = vec![0u8; usize::try_from(cursor).unwrap()];

        write_le_u32(&mut data, 0x00, HEADER_SIZE as u32);
        write_le_u32(&mut data, 0x10, tmd.len() as u32);
        write_le_u64(&mut data, 0x18, content_size);

        for (_, content_index, _, _) in contents {
            let byte_index = usize::from(*content_index) / 8;
            let bit_index = usize::from(*content_index) % 8;
            data[0x20 + byte_index] |= 0x80 >> bit_index;
        }

        let tmd_start = usize::try_from(tmd_offset).unwrap();
        data[tmd_start..tmd_start + tmd.len()].copy_from_slice(&tmd);

        for (entry, (_, offset)) in ordered.iter().zip(locations.iter()) {
            let bytes = entry.3;
            let start = usize::try_from(*offset).unwrap();
            data[start..start + bytes.len()].copy_from_slice(bytes);
        }

        (data, locations)
    }

    #[test]
    fn opens_cia_and_reads_selected_contents_from_aligned_ranges() {
        let main = b"main executable bytes";
        let manual = b"optional manual bytes";
        let (data, _) = make_cia(&[(0x1000, 0, 0, main), (0x2000, 2, 0x4000, manual)]);
        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();

        assert_eq!(archive.title_metadata().title_id, 0x0004_0000_1234_5678);
        assert_eq!(archive.contents().len(), 2);
        assert_eq!(archive.content_location(1), None);
        assert_eq!(archive.content_location(0).unwrap().offset_bytes % 0x40, 0);
        assert_eq!(archive.content_location(2).unwrap().offset_bytes % 0x40, 0);

        {
            let mut main_reader = archive.open_content_reader(0).unwrap().unwrap();
            let mut main_bytes = Vec::new();
            main_reader.read_to_end(&mut main_bytes).unwrap();
            assert_eq!(main_bytes, main);
            assert_eq!(main_reader.read(&mut [0u8; 1]).unwrap(), 0);
        }

        {
            let mut manual_reader = archive.open_content_reader(2).unwrap().unwrap();
            let mut manual_bytes = Vec::new();
            manual_reader.read_to_end(&mut manual_bytes).unwrap();
            assert_eq!(manual_bytes, manual);
        }

        archive.verify_content(0).unwrap();
        archive.verify_content(2).unwrap();
    }

    #[test]
    fn opens_a_tmd_verified_ncch_from_a_cleartext_cia_content() {
        let mut content = vec![0u8; 0x1_0000];
        content[0x100..0x104].copy_from_slice(b"NCCH");
        write_le_u32(&mut content, 0x104, 0x80);

        let (data, _) = make_cia(&[(0x3000, 0, 0, &content)]);
        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        let mut ncch = archive
            .open_verified_ncch_content_reader(0)
            .unwrap()
            .unwrap();

        assert_eq!(ncch.location().content_index, 0);
        assert_eq!(ncch.header().content_size_bytes, 0x1_0000);
        assert_eq!(ncch.reader_mut().position(), 0);

        ncch.reader_mut().seek(SeekFrom::Start(0x100)).unwrap();
        let mut magic = [0u8; 4];
        ncch.reader_mut().read_exact(&mut magic).unwrap();
        assert_eq!(&magic, b"NCCH");
    }

    #[test]
    fn rejects_non_ncch_bytes_after_verifying_the_tmd_content_hash() {
        let content = vec![0u8; 0x1_0000];
        let (data, _) = make_cia(&[(0x3000, 0, 0, &content)]);
        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();

        assert!(matches!(
            archive.open_verified_ncch_content_reader(0),
            Err(CiaArchiveError::InvalidNcchHeader(_))
        ));
    }

    #[test]
    fn encrypted_content_requires_decryption_before_tmd_hash_verification() {
        let cleartext = b"decrypted content bytes";
        let (data, _) = make_cia(&[(0x1000, 0, CONTENT_TYPE_ENCRYPTED, cleartext)]);
        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();

        assert!(matches!(
            archive.verify_content(0),
            Err(CiaArchiveError::ContentRequiresDecryption(0))
        ));
        archive
            .verify_content_stream(0, Cursor::new(cleartext))
            .unwrap();
        let wrong_cleartext = vec![0xFF; cleartext.len()];
        assert!(matches!(
            archive.verify_content_stream(0, Cursor::new(wrong_cleartext)),
            Err(CiaArchiveError::ContentHashMismatch { content_index: 0 })
        ));
    }

    #[test]
    fn rejects_content_whose_bytes_do_not_match_the_tmd_hash() {
        let (mut data, locations) = make_cia(&[(0x1000, 0, 0, b"original content")]);
        let content_offset = usize::try_from(locations[0].1).unwrap();
        data[content_offset] ^= 0xFF;

        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();

        assert!(matches!(
            archive.verify_content(0),
            Err(CiaArchiveError::ContentHashMismatch { content_index: 0 })
        ));
    }

    #[test]
    fn content_reader_cannot_seek_before_or_after_its_own_range() {
        let (data, _) = make_cia(&[(0x1000, 0, 1, b"first"), (0x2000, 2, 1, b"second")]);
        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        let mut reader = archive.open_content_reader(0).unwrap().unwrap();

        assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 5);
        assert_eq!(reader.read(&mut [0u8; 1]).unwrap(), 0);
        assert!(reader.seek(SeekFrom::Current(1)).is_err());
        assert!(reader.seek(SeekFrom::Start(u64::MAX)).is_err());
    }
}
