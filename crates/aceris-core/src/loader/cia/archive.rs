//! Streaming access to the sections and content entries of a CIA archive.
//!
//! The archive reader keeps the original seekable source and exposes each
//! selected content as a bounded stream. It never buffers the complete CIA
//! content section in memory.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

use crate::loader::ncsd::ncch::romfs::sha256::Sha256;
use crate::loader::ncsd::ncch::{HEADER_SIZE as NCCH_HEADER_SIZE, NcchError, NcchHeader};

use super::crypto::{Aes128CbcReader, TitleKey, cia_content_iv};
use super::ticket::{MAX_TICKET_SIZE_BYTES, TicketError, TicketHeader};
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
    InvalidTicket(TicketError),
    InvalidNcchHeader(NcchError),
    TicketSectionTooLarge {
        declared_size: u64,
        maximum_size: usize,
    },
    TicketTitleIdMismatch {
        ticket_title_id: u64,
        tmd_title_id: u64,
    },
    TmdSectionTooLarge {
        declared_size: u64,
        maximum_size: usize,
    },
    AllocationFailed,
    ContentNotIncluded(u16),
    ContentNotEncrypted(u16),
    ContentSizeNotBlockAligned(u16),
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
            Self::InvalidTicket(error) => write!(formatter, "invalid CIA ticket: {error}"),
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
            Self::TicketSectionTooLarge {
                declared_size,
                maximum_size,
            } => write!(
                formatter,
                "CIA ticket section size {declared_size} exceeds the structural limit of {maximum_size} bytes"
            ),
            Self::TicketTitleIdMismatch {
                ticket_title_id,
                tmd_title_id,
            } => write!(
                formatter,
                "CIA ticket title ID {ticket_title_id:#018x} does not match TMD title ID {tmd_title_id:#018x}"
            ),
            Self::ContentNotIncluded(index) => {
                write!(formatter, "CIA does not contain content index {index}")
            }
            Self::ContentNotEncrypted(index) => {
                write!(
                    formatter,
                    "CIA content index {index} is not marked as encrypted"
                )
            }
            Self::ContentSizeNotBlockAligned(index) => write!(
                formatter,
                "encrypted CIA content index {index} has a size that is not aligned to 16-byte AES blocks"
            ),
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
            Self::InvalidTicket(error) => Some(error),
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
    ticket: Option<TicketHeader>,
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

        let ticket = if header.ticket.size_bytes == 0 {
            None
        } else {
            if header.ticket.size_bytes > MAX_TICKET_SIZE_BYTES as u64 {
                return Err(CiaArchiveError::TicketSectionTooLarge {
                    declared_size: header.ticket.size_bytes,
                    maximum_size: MAX_TICKET_SIZE_BYTES,
                });
            }

            let ticket_size = usize::try_from(header.ticket.size_bytes).map_err(|_| {
                CiaArchiveError::TicketSectionTooLarge {
                    declared_size: header.ticket.size_bytes,
                    maximum_size: MAX_TICKET_SIZE_BYTES,
                }
            })?;
            let mut ticket_bytes = Vec::new();
            ticket_bytes
                .try_reserve_exact(ticket_size)
                .map_err(|_| CiaArchiveError::AllocationFailed)?;
            ticket_bytes.resize(ticket_size, 0);

            let actual_ticket_position = reader
                .seek(SeekFrom::Start(header.ticket.offset_bytes))
                .map_err(CiaArchiveError::Reader)?;
            if actual_ticket_position != header.ticket.offset_bytes {
                return Err(CiaArchiveError::Reader(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "source did not seek to the CIA ticket section",
                )));
            }
            reader
                .read_exact(&mut ticket_bytes)
                .map_err(CiaArchiveError::Reader)?;
            Some(TicketHeader::parse(&ticket_bytes).map_err(CiaArchiveError::InvalidTicket)?)
        };

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
        if let Some(ticket) = &ticket {
            if ticket.title_id != title_metadata.title_id {
                return Err(CiaArchiveError::TicketTitleIdMismatch {
                    ticket_title_id: ticket.title_id,
                    tmd_title_id: title_metadata.title_id,
                });
            }
        }
        let contents = build_content_locations(&header, &title_metadata)?;

        Ok(Self {
            reader,
            header,
            ticket,
            title_metadata,
            contents,
        })
    }

    /// Returns the validated fixed CIA header.
    pub const fn header(&self) -> &CiaHeader {
        &self.header
    }

    /// Returns parsed ticket data, when the CIA has a ticket section.
    pub const fn ticket(&self) -> Option<&TicketHeader> {
        self.ticket.as_ref()
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

    /// Opens an encrypted CIA content as a streaming cleartext reader.
    ///
    /// The caller supplies the already-decrypted title key. The TMD content
    /// index determines the CBC IV; contents not marked encrypted are rejected
    /// to prevent accidental double-decryption.
    pub fn open_decrypted_content_reader(
        &mut self,
        content_index: u16,
        title_key: &TitleKey,
    ) -> Result<Option<Aes128CbcReader<CiaContentReader<'_, R>>>, CiaArchiveError> {
        let Some(location) = self.content_location(content_index) else {
            return Ok(None);
        };
        if location.content_type & CONTENT_TYPE_ENCRYPTED == 0 {
            return Err(CiaArchiveError::ContentNotEncrypted(content_index));
        }
        if location.size_bytes % 16 != 0 {
            return Err(CiaArchiveError::ContentSizeNotBlockAligned(content_index));
        }

        let source = self
            .open_content_reader(content_index)?
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
        let decrypted = Aes128CbcReader::new(
            source,
            *title_key.as_bytes(),
            cia_content_iv(location.content_index),
            location.size_bytes,
        )
        .map_err(CiaArchiveError::Reader)?;

        Ok(Some(decrypted))
    }

    /// Decrypts one encrypted CIA content in bounded memory and verifies its TMD SHA-256.
    ///
    /// The title key must have been obtained from the ticket using caller-
    /// supplied common-key material. The method does not verify ticket/TMD
    /// signatures and does not decrypt NCCH-internal AES-CTR regions.
    pub fn verify_encrypted_content(
        &mut self,
        content_index: u16,
        title_key: &TitleKey,
    ) -> Result<(), CiaArchiveError> {
        let (expected_size, expected_hash) = {
            let record = self
                .title_metadata
                .content_by_index(content_index)
                .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
            if record.content_type & CONTENT_TYPE_ENCRYPTED == 0 {
                return Err(CiaArchiveError::ContentNotEncrypted(content_index));
            }
            (record.size_bytes, record.sha256)
        };

        let mut decrypted = self
            .open_decrypted_content_reader(content_index, title_key)?
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;
        verify_content_stream_bytes(content_index, expected_size, expected_hash, &mut decrypted)
    }

    /// Verifies and opens an NCCH inside an encrypted CIA content.
    ///
    /// The encrypted CIA stream is decrypted and checked against its TMD
    /// SHA-256 in a first pass. A fresh stream is then opened and its NCCH
    /// header is parsed; the header bytes are retained in front of the reader
    /// so callers receive the complete cleartext content from offset zero.
    ///
    /// The returned reader is sequential because the outer AES-CBC stream
    /// does not provide random access. NCCH-internal AES-CTR regions remain
    /// encrypted and must be opened through the NCCH crypto APIs separately.
    pub fn open_verified_decrypted_ncch_content_reader(
        &mut self,
        content_index: u16,
        title_key: &TitleKey,
    ) -> Result<Option<CiaDecryptedContentNcchReader<'_, R>>, CiaArchiveError> {
        let Some(location) = self.content_location(content_index) else {
            return Ok(None);
        };

        self.verify_encrypted_content(content_index, title_key)?;
        let mut decrypted = self
            .open_decrypted_content_reader(content_index, title_key)?
            .ok_or(CiaArchiveError::ContentNotIncluded(content_index))?;

        let mut header_bytes = [0u8; NCCH_HEADER_SIZE];
        decrypted.read_exact(&mut header_bytes).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                CiaArchiveError::InvalidNcchHeader(NcchError::InvalidHeader("header is truncated"))
            } else {
                CiaArchiveError::Reader(error)
            }
        })?;
        let header = NcchHeader::parse(&header_bytes, location.size_bytes)
            .map_err(CiaArchiveError::InvalidNcchHeader)?;

        Ok(Some(CiaDecryptedContentNcchReader {
            location,
            header,
            reader: PrefixedReader::new(header_bytes, decrypted),
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

/// A streaming reader that replays a parsed prefix before reading its source.
struct PrefixedReader<R> {
    prefix: [u8; NCCH_HEADER_SIZE],
    prefix_position: usize,
    reader: R,
}

impl<R> PrefixedReader<R> {
    fn new(prefix: [u8; NCCH_HEADER_SIZE], reader: R) -> Self {
        Self {
            prefix,
            prefix_position: 0,
            reader,
        }
    }
}

impl<R: Read> Read for PrefixedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.prefix_position < self.prefix.len() {
            let remaining = self.prefix.len() - self.prefix_position;
            let count = buffer.len().min(remaining);
            let end = self.prefix_position + count;
            buffer[..count].copy_from_slice(&self.prefix[self.prefix_position..end]);
            self.prefix_position = end;
            return Ok(count);
        }
        self.reader.read(buffer)
    }
}

/// A TMD-verified, decrypted NCCH content from an encrypted CIA archive.
///
/// The NCCH header is parsed before construction, and the original header
/// bytes are replayed to the caller as the first bytes of the stream.
pub struct CiaDecryptedContentNcchReader<'a, R> {
    location: CiaContentLocation,
    header: NcchHeader,
    reader: PrefixedReader<Aes128CbcReader<CiaContentReader<'a, R>>>,
}

impl<R: Read + Seek> CiaDecryptedContentNcchReader<'_, R> {
    /// Returns the TMD content location.
    pub const fn location(&self) -> CiaContentLocation {
        self.location
    }

    /// Returns the structurally validated NCCH header.
    pub const fn header(&self) -> &NcchHeader {
        &self.header
    }

    /// Returns the complete cleartext NCCH stream, starting at byte zero.
    pub fn reader_mut(&mut self) -> &mut dyn Read {
        &mut self.reader
    }
}

impl<R: Read + Seek> Read for CiaDecryptedContentNcchReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

/// A TMD-verified NCCH content paired with its bounded source reader.
#[derive(Debug)]
pub struct CiaContentNcchReader<'a, R> {
    location: CiaContentLocation,
    header: NcchHeader,
    reader: CiaContentReader<'a, R>,
}

impl<'a, R: Read + Seek> CiaContentNcchReader<'a, R> {
    /// Returns the TMD content location.
    pub const fn location(&self) -> CiaContentLocation {
        self.location
    }

    /// Returns the structurally validated NCCH header.
    pub const fn header(&self) -> &NcchHeader {
        &self.header
    }

    /// Returns the bounded reader, positioned at the beginning of the NCCH.
    pub fn reader_mut<'b>(&'b mut self) -> &'b mut CiaContentReader<'a, R> {
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
    use crate::loader::ncsd::ncch::crypto::Aes128;
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
        make_cia_with_ticket(contents, &[])
    }

    fn make_cia_with_ticket(
        contents: &[(u32, u16, u16, &[u8])],
        ticket: &[u8],
    ) -> (Vec<u8>, Vec<(u16, u64)>) {
        let tmd = make_tmd(contents);
        let ticket_offset = align(HEADER_SIZE as u64);
        let tmd_offset = align(ticket_offset + ticket.len() as u64);
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
        write_le_u32(&mut data, 0x0C, ticket.len() as u32);
        write_le_u32(&mut data, 0x10, tmd.len() as u32);
        write_le_u64(&mut data, 0x18, content_size);

        for (_, content_index, _, _) in contents {
            let byte_index = usize::from(*content_index) / 8;
            let bit_index = usize::from(*content_index) % 8;
            data[0x20 + byte_index] |= 0x80 >> bit_index;
        }

        let ticket_start = usize::try_from(ticket_offset).unwrap();
        data[ticket_start..ticket_start + ticket.len()].copy_from_slice(ticket);

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
    fn reads_ticket_metadata_from_cia_and_matches_the_tmd_title_id() {
        let content = b"test content";
        let ticket = make_test_ticket(0x0004_0000_1234_5678);
        let (data, _) = make_cia_with_ticket(&[(0x3000, 0, 0, content)], &ticket);
        let archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        let parsed = archive.ticket().unwrap();

        assert_eq!(parsed.title_id, archive.title_metadata().title_id);
        assert_eq!(parsed.encrypted_title_key, [0xA5; 16]);
        assert_eq!(parsed.common_key_index, 2);
    }

    #[test]
    fn rejects_ticket_title_id_that_does_not_match_the_tmd() {
        let content = b"test content";
        let ticket = make_test_ticket(0x0004_0000_8765_4321);
        let (data, _) = make_cia_with_ticket(&[(0x3000, 0, 0, content)], &ticket);

        assert!(matches!(
            CiaArchiveReader::open(Cursor::new(data)),
            Err(CiaArchiveError::TicketTitleIdMismatch { .. })
        ));
    }

    fn make_test_ticket(title_id: u64) -> Vec<u8> {
        const SIGNATURE_DATA_SIZE: usize = 0x140;
        let mut ticket = vec![0u8; SIGNATURE_DATA_SIZE + super::super::ticket::TICKET_DATA_SIZE];
        ticket[..4].copy_from_slice(&0x0001_0004u32.to_be_bytes());
        let body = &mut ticket[SIGNATURE_DATA_SIZE..];
        body[0x7C] = 1;
        body[0x7F..0x8F].fill(0xA5);
        body[0x90..0x98].copy_from_slice(&0x1122_3344_5566_7788u64.to_be_bytes());
        body[0x98..0x9C].copy_from_slice(&0x1234_5678u32.to_be_bytes());
        body[0x9C..0xA4].copy_from_slice(&title_id.to_be_bytes());
        body[0xA6..0xA8].copy_from_slice(&0x0102u16.to_be_bytes());
        body[0xB0] = 1;
        body[0xB1] = 2;
        ticket
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

    fn encrypt_cia_content(key: [u8; 16], content_index: u16, cleartext: &[u8]) -> Vec<u8> {
        assert_eq!(cleartext.len() % 16, 0);
        let cipher = Aes128::new(key);
        let mut previous = cia_content_iv(content_index);
        let mut encrypted = Vec::with_capacity(cleartext.len());

        for chunk in cleartext.chunks_exact(16) {
            let mut block: [u8; 16] = chunk.try_into().unwrap();
            for (byte, prior) in block.iter_mut().zip(previous) {
                *byte ^= prior;
            }
            let ciphertext = cipher.encrypt_block(block);
            encrypted.extend_from_slice(&ciphertext);
            previous = ciphertext;
        }

        encrypted
    }

    #[test]
    fn decrypts_encrypted_cia_content_as_a_stream_and_verifies_its_tmd_hash() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
            0x0E, 0x0F,
        ];
        let title_key = TitleKey::new(key);
        let cleartext: Vec<u8> = (0..0x100).map(|value| (value * 17) as u8).collect();
        let content_index = 0x1234;
        let (mut data, locations) =
            make_cia(&[(0x4000, content_index, CONTENT_TYPE_ENCRYPTED, &cleartext)]);
        let ciphertext = encrypt_cia_content(key, content_index, &cleartext);
        let offset = usize::try_from(locations[0].1).unwrap();
        data[offset..offset + ciphertext.len()].copy_from_slice(&ciphertext);

        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        {
            let mut decrypted = archive
                .open_decrypted_content_reader(content_index, &title_key)
                .unwrap()
                .unwrap();
            let mut actual = Vec::new();
            decrypted.read_to_end(&mut actual).unwrap();
            assert_eq!(actual, cleartext);
        }

        archive
            .verify_encrypted_content(content_index, &title_key)
            .unwrap();
    }

    #[test]
    fn verifies_and_opens_an_encrypted_ncch_without_consuming_its_header() {
        let key = [0x29; 16];
        let title_key = TitleKey::new(key);
        let mut cleartext = vec![0u8; 0x1_0000];
        cleartext[0x100..0x104].copy_from_slice(b"NCCH");
        write_le_u32(&mut cleartext, 0x104, 0x80);

        let content_index = 0x0123;
        let (mut data, locations) =
            make_cia(&[(0x5100, content_index, CONTENT_TYPE_ENCRYPTED, &cleartext)]);
        let ciphertext = encrypt_cia_content(key, content_index, &cleartext);
        let offset = usize::try_from(locations[0].1).unwrap();
        data[offset..offset + ciphertext.len()].copy_from_slice(&ciphertext);

        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        let mut ncch = archive
            .open_verified_decrypted_ncch_content_reader(content_index, &title_key)
            .unwrap()
            .unwrap();

        assert_eq!(ncch.location().content_index, content_index);
        assert_eq!(ncch.header().content_size_bytes, cleartext.len() as u64);

        let mut reopened_content = Vec::new();
        ncch.reader_mut()
            .read_to_end(&mut reopened_content)
            .unwrap();
        assert_eq!(reopened_content, cleartext);
    }

    #[test]
    fn encrypted_ncch_open_rejects_a_wrong_title_key_before_parsing() {
        let key = [0x71; 16];
        let title_key = TitleKey::new(key);
        let mut cleartext = vec![0u8; 0x1_0000];
        cleartext[0x100..0x104].copy_from_slice(b"NCCH");
        write_le_u32(&mut cleartext, 0x104, 0x80);

        let content_index = 0x0034;
        let (mut data, locations) =
            make_cia(&[(0x5200, content_index, CONTENT_TYPE_ENCRYPTED, &cleartext)]);
        let ciphertext = encrypt_cia_content(key, content_index, &cleartext);
        let offset = usize::try_from(locations[0].1).unwrap();
        data[offset..offset + ciphertext.len()].copy_from_slice(&ciphertext);

        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        let wrong_title_key = TitleKey::new([0x72; 16]);

        assert!(matches!(
            archive.open_verified_decrypted_ncch_content_reader(content_index, &wrong_title_key),
            Err(CiaArchiveError::ContentHashMismatch {
                content_index: 0x0034
            })
        ));
    }

    #[test]
    fn encrypted_ncch_open_rejects_a_validly_hashed_non_ncch_payload() {
        let key = [0x83; 16];
        let title_key = TitleKey::new(key);
        let cleartext = vec![0u8; 0x1_0000];
        let content_index = 0x0035;
        let (mut data, locations) =
            make_cia(&[(0x5300, content_index, CONTENT_TYPE_ENCRYPTED, &cleartext)]);
        let ciphertext = encrypt_cia_content(key, content_index, &cleartext);
        let offset = usize::try_from(locations[0].1).unwrap();
        data[offset..offset + ciphertext.len()].copy_from_slice(&ciphertext);

        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();

        assert!(matches!(
            archive.open_verified_decrypted_ncch_content_reader(content_index, &title_key),
            Err(CiaArchiveError::InvalidNcchHeader(_))
        ));
    }

    #[test]
    fn rejects_wrong_title_keys_and_double_decryption() {
        let key = [0x5A; 16];
        let cleartext = vec![0x3C; 0x40];
        let content_index = 7;
        let (mut encrypted_cia, locations) =
            make_cia(&[(0x4100, content_index, CONTENT_TYPE_ENCRYPTED, &cleartext)]);
        let ciphertext = encrypt_cia_content(key, content_index, &cleartext);
        let offset = usize::try_from(locations[0].1).unwrap();
        encrypted_cia[offset..offset + ciphertext.len()].copy_from_slice(&ciphertext);
        let mut archive = CiaArchiveReader::open(Cursor::new(encrypted_cia)).unwrap();

        let wrong_title_key = TitleKey::new([0xA5; 16]);
        assert!(matches!(
            archive.verify_encrypted_content(content_index, &wrong_title_key),
            Err(CiaArchiveError::ContentHashMismatch { content_index: 7 })
        ));

        let (mut clear_cia, _) = make_cia(&[(0x4200, 0, 0, &cleartext)]);
        let mut clear_archive = CiaArchiveReader::open(Cursor::new(&mut clear_cia[..])).unwrap();
        assert!(matches!(
            clear_archive.open_decrypted_content_reader(0, &wrong_title_key),
            Err(CiaArchiveError::ContentNotEncrypted(0))
        ));
    }

    #[test]
    fn rejects_encrypted_content_with_a_partial_aes_block() {
        let bytes = [0x33; 17];
        let (data, _) = make_cia(&[(0x4300, 0, CONTENT_TYPE_ENCRYPTED, &bytes)]);
        let mut archive = CiaArchiveReader::open(Cursor::new(data)).unwrap();
        let title_key = TitleKey::new([0x44; 16]);

        assert!(matches!(
            archive.open_decrypted_content_reader(0, &title_key),
            Err(CiaArchiveError::ContentSizeNotBlockAligned(0))
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
