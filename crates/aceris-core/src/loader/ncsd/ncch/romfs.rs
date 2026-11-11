//! Structural parser for the Nintendo 3DS RomFS IVFC and Level 3 data.
//!
//! This module validates container and table metadata, resolves the documented
//! physical IVFC level layout, and reads the Level 3 tree. It does not decrypt
//! NCCH content, verify SHA-256 hashes, or extract files.

use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

/// SHA-256 incremental used by RomFS IVFC integrity validation.
pub mod sha256;

use self::sha256::Sha256;

pub const IVFC_HEADER_SIZE: usize = 0x5C;
/// Physical start of the RomFS Level 3 image inside an IVFC container.
pub const LEVEL3_DATA_OFFSET_BYTES: u64 = 0x1000;
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
    IvfcHashMismatch { level: u8, block_index: u64 },
    InvalidLevel3Header(&'static str),
    InvalidTree(&'static str),
    InvalidDirectoryEntry(&'static str),
    InvalidFileEntry(&'static str),
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
            Self::IvfcHashMismatch { level, block_index } => write!(
                formatter,
                "RomFS IVFC Level {level} hash mismatch at block {block_index}"
            ),
            Self::InvalidLevel3Header(reason) => {
                write!(formatter, "invalid RomFS Level 3 header: {reason}")
            }
            Self::InvalidTree(reason) => write!(formatter, "invalid RomFS tree: {reason}"),
            Self::InvalidDirectoryEntry(reason) => {
                write!(formatter, "invalid RomFS directory entry: {reason}")
            }
            Self::InvalidFileEntry(reason) => {
                write!(formatter, "invalid RomFS file entry: {reason}")
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

    /// Resolves the documented physical order of the IVFC hash-tree regions.
    ///
    /// RomFS stores Level 3 at 0x1000, followed by Level 1 and then Level 2.
    /// Each following region starts at the next block boundary of the region
    /// immediately before it.
    pub fn physical_layout(&self, available_size_bytes: u64) -> Result<IvfcLayout, RomFsError> {
        if available_size_bytes < IVFC_HEADER_SIZE as u64 {
            return Err(RomFsError::InvalidIvfcHeader("header is truncated"));
        }

        let master_hash_start = align_up(
            (IVFC_HEADER_SIZE as u64)
                .checked_add(u64::from(self.optional_info_size_bytes))
                .ok_or(RomFsError::InvalidIvfcHeader(
                    "optional-info range overflows",
                ))?,
            0x20,
        )
        .ok_or(RomFsError::InvalidIvfcHeader(
            "master-hash alignment overflows",
        ))?;
        let master_hash_end = master_hash_start
            .checked_add(u64::from(self.master_hash_size_bytes))
            .ok_or(RomFsError::InvalidIvfcHeader("master-hash range overflows"))?;
        if master_hash_end > LEVEL3_DATA_OFFSET_BYTES {
            return Err(RomFsError::InvalidIvfcHeader(
                "master-hash area overlaps the Level 3 image",
            ));
        }

        let master_hash = IvfcRegion {
            offset_bytes: master_hash_start,
            size_bytes: u64::from(self.master_hash_size_bytes),
        };
        let level3 = IvfcRegion {
            offset_bytes: LEVEL3_DATA_OFFSET_BYTES,
            size_bytes: self.level3.hash_data_size_bytes,
        };
        let level3_end = level3
            .end_offset_bytes()
            .ok_or(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "physical Level 3 range overflows",
            })?;
        if level3_end > available_size_bytes {
            return Err(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "physical Level 3 range exceeds the RomFS region",
            });
        }

        let level1_start = align_up(level3_end, self.level3.block_size_bytes).ok_or(
            RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "physical Level 1 alignment overflows",
            },
        )?;
        let level1 = IvfcRegion {
            offset_bytes: level1_start,
            size_bytes: self.level1.hash_data_size_bytes,
        };
        let level1_end = level1
            .end_offset_bytes()
            .ok_or(RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "physical Level 1 range overflows",
            })?;
        if level1_end > available_size_bytes {
            return Err(RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "physical Level 1 range exceeds the RomFS region",
            });
        }

        let level2_start = align_up(level1_end, self.level1.block_size_bytes).ok_or(
            RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "physical Level 2 alignment overflows",
            },
        )?;
        let level2 = IvfcRegion {
            offset_bytes: level2_start,
            size_bytes: self.level2.hash_data_size_bytes,
        };
        let level2_end = level2
            .end_offset_bytes()
            .ok_or(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "physical Level 2 range overflows",
            })?;
        if level2_end > available_size_bytes {
            return Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "physical Level 2 range exceeds the RomFS region",
            });
        }

        Ok(IvfcLayout {
            master_hash,
            level3,
            level1,
            level2,
        })
    }

    /// Resolves the physical Level 3 region and validates the complete IVFC layout.
    pub fn level3_region(&self, available_size_bytes: u64) -> Result<IvfcRegion, RomFsError> {
        Ok(self.physical_layout(available_size_bytes)?.level3)
    }

    /// Verifies the complete SHA-256 chain from the master hash to Level 3.
    ///
    /// The reader must start at the IVFC header and expose cleartext RomFS bytes.
    /// Each stored digest is checked against the corresponding next-level block,
    /// with the final partial block padded with zero bytes to its declared size.
    pub fn verify_integrity<R: Read + Seek>(
        &self,
        inner: R,
        available_size_bytes: u64,
    ) -> Result<(), RomFsError> {
        let layout = self.physical_layout(available_size_bytes)?;
        let mut image = RomFsRangeReader::new(inner, 0, available_size_bytes)
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;

        verify_ivfc_hash_table(
            &mut image,
            layout.level1,
            self.level1.block_size_bytes,
            layout.master_hash,
            1,
            available_size_bytes,
        )?;
        verify_ivfc_hash_table(
            &mut image,
            layout.level2,
            self.level2.block_size_bytes,
            layout.level1,
            2,
            available_size_bytes,
        )?;
        verify_ivfc_hash_table(
            &mut image,
            layout.level3,
            self.level3.block_size_bytes,
            layout.level2,
            3,
            available_size_bytes,
        )?;

        Ok(())
    }

    /// Opens the Level 3 image as an isolated, bounded stream.
    ///
    /// The input stream must begin at the IVFC header (RomFS region byte zero).
    /// The returned reader exposes only the Level 3 range.
    pub fn open_level3_reader<R: Read + Seek>(
        &self,
        inner: R,
        available_size_bytes: u64,
    ) -> Result<RomFsLevel3Reader<RomFsRangeReader<R>>, RomFsError> {
        let region = self.level3_region(available_size_bytes)?;
        let bounded = RomFsRangeReader::new(inner, region.offset_bytes, region.size_bytes)
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;
        RomFsLevel3Reader::new(bounded, region.size_bytes)
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

/// A physical byte region selected from an IVFC image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IvfcRegion {
    pub offset_bytes: u64,
    pub size_bytes: u64,
}

impl IvfcRegion {
    pub const fn end_offset_bytes(self) -> Option<u64> {
        self.offset_bytes.checked_add(self.size_bytes)
    }
}

/// Physical locations of the master hash and three RomFS IVFC levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IvfcLayout {
    pub master_hash: IvfcRegion,
    pub level3: IvfcRegion,
    pub level1: IvfcRegion,
    pub level2: IvfcRegion,
}

/// A seekable reader restricted to a physical range of its underlying stream.
#[derive(Debug)]
pub struct RomFsRangeReader<R> {
    inner: R,
    start: u64,
    size: u64,
    position: u64,
}

impl<R: Read + Seek> RomFsRangeReader<R> {
    /// Creates a bounded view after confirming the complete range exists.
    pub fn new(mut inner: R, start: u64, size: u64) -> io::Result<Self> {
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RomFS range has zero length",
            ));
        }

        let end = start
            .checked_add(size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "RomFS range overflows"))?;
        let available = inner.seek(SeekFrom::End(0))?;
        if end > available {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "RomFS range extends beyond the underlying stream",
            ));
        }

        let actual = inner.seek(SeekFrom::Start(start))?;
        if actual != start {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the RomFS range",
            ));
        }

        Ok(Self {
            inner,
            start,
            size,
            position: 0,
        })
    }

    pub const fn len(&self) -> u64 {
        self.size
    }

    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    pub const fn position(&self) -> u64 {
        self.position
    }
}

impl<R: Read + Seek> Read for RomFsRangeReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.position == self.size {
            return Ok(0);
        }

        let remaining = self.size - self.position;
        let readable = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let count = self.inner.read(&mut buffer[..readable])?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for RomFsRangeReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
        };

        if !(0..=i128::from(self.size)).contains(&target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek is outside the RomFS range",
            ));
        }

        let relative = u64::try_from(target).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid RomFS range position")
        })?;
        let absolute = self.start.checked_add(relative).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute RomFS range position overflows",
            )
        })?;
        let actual = self.inner.seek(SeekFrom::Start(absolute))?;
        if actual != absolute {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested RomFS range position",
            ));
        }

        self.position = relative;
        Ok(relative)
    }
}

/// A directory metadata record in the Level 3 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomFsDirectoryEntry {
    /// Byte offset of this record within the directory metadata table.
    pub metadata_offset_bytes: u32,
    pub parent_directory_offset_bytes: u32,
    pub next_sibling_directory_offset_bytes: u32,
    pub first_child_directory_offset_bytes: u32,
    pub first_file_offset_bytes: u32,
    pub next_hash_directory_offset_bytes: u32,
    /// UTF-16LE directory name.
    pub name: String,
}

/// A file metadata record in the Level 3 image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomFsFileEntry {
    /// Byte offset of this record within the file metadata table.
    pub metadata_offset_bytes: u32,
    pub parent_directory_offset_bytes: u32,
    pub next_sibling_file_offset_bytes: u32,
    /// Offset relative to the beginning of the Level 3 file-data area.
    pub data_offset_bytes: u64,
    pub data_size_bytes: u64,
    pub next_hash_file_offset_bytes: u32,
    /// UTF-16LE filename.
    pub name: String,
}

/// A directory included in a validated RomFS tree walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomFsTreeDirectory {
    /// Canonical path relative to the RomFS root, using forward slashes.
    /// The root directory is represented by an empty path.
    pub path: String,
    pub entry: RomFsDirectoryEntry,
}

/// A file included in a validated RomFS tree walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomFsTreeFile {
    /// Canonical path relative to the RomFS root, using forward slashes.
    pub path: String,
    pub entry: RomFsFileEntry,
}

/// Flattened directory and file entries discovered from the RomFS root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RomFsTree {
    pub directories: Vec<RomFsTreeDirectory>,
    pub files: Vec<RomFsTreeFile>,
}

/// Reader for an already-isolated logical RomFS Level 3 image.
///
/// The supplied stream must start at Level 3 byte zero. IVFC physical layout,
/// encryption, and hash verification are deliberately outside this reader.
#[derive(Debug)]
pub struct RomFsLevel3Reader<R> {
    inner: R,
    header: Level3Header,
    level3_size_bytes: u64,
}

impl<R: Read + Seek> RomFsLevel3Reader<R> {
    /// Reads and validates the Level 3 header at stream offset zero.
    pub fn new(mut inner: R, level3_size_bytes: u64) -> Result<Self, RomFsError> {
        let actual = inner
            .seek(SeekFrom::Start(0))
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;
        if actual != 0 {
            return Err(RomFsError::ReaderFailure(
                "reader did not seek to Level 3 offset zero".to_owned(),
            ));
        }

        let mut bytes = [0; LEVEL3_HEADER_SIZE];
        inner
            .read_exact(&mut bytes)
            .map_err(|_| RomFsError::InvalidLevel3Header("header is truncated"))?;
        let header = Level3Header::parse(&bytes, level3_size_bytes)?;

        Ok(Self {
            inner,
            header,
            level3_size_bytes,
        })
    }

    /// Returns the validated Level 3 table layout.
    pub const fn header(&self) -> &Level3Header {
        &self.header
    }

    /// Returns the declared logical size of the Level 3 image.
    pub const fn len(&self) -> u64 {
        self.level3_size_bytes
    }

    /// Returns true when the declared Level 3 image is empty.
    pub const fn is_empty(&self) -> bool {
        self.level3_size_bytes == 0
    }

    /// Reads a directory record by its offset within the directory metadata table.
    pub fn read_directory_entry(
        &mut self,
        metadata_offset_bytes: u32,
    ) -> Result<RomFsDirectoryEntry, RomFsError> {
        let table_size = u64::from(self.header.directory_metadata_size_bytes);
        let offset = u64::from(metadata_offset_bytes);
        validate_entry_offset(offset, table_size).map_err(RomFsError::InvalidDirectoryEntry)?;

        let fixed = read_metadata_fixed::<0x18, _>(
            &mut self.inner,
            u64::from(self.header.directory_metadata_offset_bytes),
            offset,
            table_size,
        )
        .map_err(RomFsError::InvalidDirectoryEntry)?;

        let parent = u32::from_le_bytes(fixed[0x00..0x04].try_into().expect("fixed field"));
        let sibling = u32::from_le_bytes(fixed[0x04..0x08].try_into().expect("fixed field"));
        let child = u32::from_le_bytes(fixed[0x08..0x0C].try_into().expect("fixed field"));
        let first_file = u32::from_le_bytes(fixed[0x0C..0x10].try_into().expect("fixed field"));
        let next_hash = u32::from_le_bytes(fixed[0x10..0x14].try_into().expect("fixed field"));
        let name_size = u32::from_le_bytes(fixed[0x14..0x18].try_into().expect("fixed field"));

        validate_optional_reference(parent, table_size)
            .map_err(RomFsError::InvalidDirectoryEntry)?;
        validate_optional_reference(sibling, table_size)
            .map_err(RomFsError::InvalidDirectoryEntry)?;
        validate_optional_reference(child, table_size)
            .map_err(RomFsError::InvalidDirectoryEntry)?;
        validate_optional_reference(next_hash, table_size)
            .map_err(RomFsError::InvalidDirectoryEntry)?;
        validate_optional_reference(first_file, u64::from(self.header.file_metadata_size_bytes))
            .map_err(RomFsError::InvalidDirectoryEntry)?;

        let name = read_metadata_name(
            &mut self.inner,
            u64::from(self.header.directory_metadata_offset_bytes),
            offset,
            table_size,
            0x18,
            name_size,
        )
        .map_err(RomFsError::InvalidDirectoryEntry)?;

        Ok(RomFsDirectoryEntry {
            metadata_offset_bytes,
            parent_directory_offset_bytes: parent,
            next_sibling_directory_offset_bytes: sibling,
            first_child_directory_offset_bytes: child,
            first_file_offset_bytes: first_file,
            next_hash_directory_offset_bytes: next_hash,
            name,
        })
    }

    /// Reads a file record and validates its data range against the Level 3 image.
    pub fn read_file_entry(
        &mut self,
        metadata_offset_bytes: u32,
    ) -> Result<RomFsFileEntry, RomFsError> {
        let table_size = u64::from(self.header.file_metadata_size_bytes);
        let offset = u64::from(metadata_offset_bytes);
        validate_entry_offset(offset, table_size).map_err(RomFsError::InvalidFileEntry)?;

        let fixed = read_metadata_fixed::<0x20, _>(
            &mut self.inner,
            u64::from(self.header.file_metadata_offset_bytes),
            offset,
            table_size,
        )
        .map_err(RomFsError::InvalidFileEntry)?;

        let parent = u32::from_le_bytes(fixed[0x00..0x04].try_into().expect("fixed field"));
        let sibling = u32::from_le_bytes(fixed[0x04..0x08].try_into().expect("fixed field"));
        let data_offset = u64::from_le_bytes(fixed[0x08..0x10].try_into().expect("fixed field"));
        let data_size = u64::from_le_bytes(fixed[0x10..0x18].try_into().expect("fixed field"));
        let next_hash = u32::from_le_bytes(fixed[0x18..0x1C].try_into().expect("fixed field"));
        let name_size = u32::from_le_bytes(fixed[0x1C..0x20].try_into().expect("fixed field"));

        validate_optional_reference(parent, u64::from(self.header.directory_metadata_size_bytes))
            .map_err(RomFsError::InvalidFileEntry)?;
        validate_optional_reference(sibling, table_size).map_err(RomFsError::InvalidFileEntry)?;
        validate_optional_reference(next_hash, table_size).map_err(RomFsError::InvalidFileEntry)?;

        let file_data_size = self
            .level3_size_bytes
            .checked_sub(u64::from(self.header.file_data_offset_bytes))
            .ok_or(RomFsError::InvalidFileEntry(
                "file-data offset exceeds Level 3 image",
            ))?;
        let data_end = data_offset
            .checked_add(data_size)
            .ok_or(RomFsError::InvalidFileEntry("file-data range overflows"))?;
        if data_end > file_data_size {
            return Err(RomFsError::InvalidFileEntry(
                "file-data range exceeds the Level 3 file-data area",
            ));
        }

        let name = read_metadata_name(
            &mut self.inner,
            u64::from(self.header.file_metadata_offset_bytes),
            offset,
            table_size,
            0x20,
            name_size,
        )
        .map_err(RomFsError::InvalidFileEntry)?;

        Ok(RomFsFileEntry {
            metadata_offset_bytes,
            parent_directory_offset_bytes: parent,
            next_sibling_file_offset_bytes: sibling,
            data_offset_bytes: data_offset,
            data_size_bytes: data_size,
            next_hash_file_offset_bytes: next_hash,
            name,
        })
    }

    /// Traverses the root directory (metadata offset zero) without recursion.
    ///
    /// Directory and file sibling chains are followed independently. The walk
    /// rejects cycles, reused metadata records, broken parent links, unsafe path
    /// components, and duplicate paths. Hash chains are not needed to enumerate
    /// the tree and are not traversed by this method.
    pub fn read_tree(&mut self) -> Result<RomFsTree, RomFsError> {
        const NO_ENTRY: u32 = u32::MAX;

        let root = self.read_directory_entry(0)?;
        if root.parent_directory_offset_bytes != 0 {
            return Err(RomFsError::InvalidTree(
                "root directory does not point to itself as parent",
            ));
        }
        if !root.name.is_empty() {
            return Err(RomFsError::InvalidTree("root directory name is not empty"));
        }

        let mut tree = RomFsTree::default();
        let mut seen_directories = HashSet::new();
        let mut seen_files = HashSet::new();
        let mut seen_paths = HashSet::new();

        seen_directories.insert(0);
        seen_paths.insert(String::new());

        let mut pending = vec![(root, String::new())];

        while let Some((directory, directory_path)) = pending.pop() {
            let directory_offset = directory.metadata_offset_bytes;
            tree.directories.push(RomFsTreeDirectory {
                path: directory_path.clone(),
                entry: directory.clone(),
            });

            let mut file_offset = directory.first_file_offset_bytes;
            while file_offset != NO_ENTRY {
                if !seen_files.insert(file_offset) {
                    return Err(RomFsError::InvalidTree(
                        "file metadata record is referenced more than once or forms a cycle",
                    ));
                }

                let file = self.read_file_entry(file_offset)?;
                if file.parent_directory_offset_bytes != directory_offset {
                    return Err(RomFsError::InvalidTree(
                        "file parent does not match the directory containing it",
                    ));
                }
                validate_path_component(&file.name).map_err(RomFsError::InvalidTree)?;

                let path = append_path_component(&directory_path, &file.name);
                if !seen_paths.insert(path.clone()) {
                    return Err(RomFsError::InvalidTree(
                        "multiple entries resolve to the same path",
                    ));
                }

                file_offset = file.next_sibling_file_offset_bytes;
                tree.files.push(RomFsTreeFile { path, entry: file });
            }

            let mut child_offset = directory.first_child_directory_offset_bytes;
            let mut children = Vec::new();
            while child_offset != NO_ENTRY {
                if !seen_directories.insert(child_offset) {
                    return Err(RomFsError::InvalidTree(
                        "directory metadata record is referenced more than once or forms a cycle",
                    ));
                }

                let child = self.read_directory_entry(child_offset)?;
                if child.parent_directory_offset_bytes != directory_offset {
                    return Err(RomFsError::InvalidTree(
                        "directory parent does not match the directory containing it",
                    ));
                }
                validate_path_component(&child.name).map_err(RomFsError::InvalidTree)?;

                let path = append_path_component(&directory_path, &child.name);
                if !seen_paths.insert(path.clone()) {
                    return Err(RomFsError::InvalidTree(
                        "multiple entries resolve to the same path",
                    ));
                }

                child_offset = child.next_sibling_directory_offset_bytes;
                children.push((child, path));
            }

            for child in children.into_iter().rev() {
                pending.push(child);
            }
        }

        Ok(tree)
    }

    /// Opens a file as a bounded reader over the Level 3 file-data area.
    ///
    /// The returned reader borrows this Level 3 reader and cannot read or seek
    /// outside the file's declared data range.
    pub fn open_file<'a>(
        &'a mut self,
        file: &RomFsFileEntry,
    ) -> io::Result<RomFsFileReader<'a, R>> {
        let file_data_size = self
            .level3_size_bytes
            .checked_sub(u64::from(self.header.file_data_offset_bytes))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid file-data offset")
            })?;
        let data_end = file
            .data_offset_bytes
            .checked_add(file.data_size_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file range overflows"))?;
        if data_end > file_data_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file range exceeds the Level 3 file-data area",
            ));
        }

        let start = u64::from(self.header.file_data_offset_bytes)
            .checked_add(file.data_offset_bytes)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file offset overflows"))?;
        let actual = self.inner.seek(SeekFrom::Start(start))?;
        if actual != start {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the file data",
            ));
        }

        Ok(RomFsFileReader {
            inner: &mut self.inner,
            start,
            size: file.data_size_bytes,
            position: 0,
        })
    }

    /// Reads one directory hash-bucket head. None represents the 0xFFFFFFFF sentinel.
    pub fn read_directory_hash_bucket(&mut self, index: u32) -> Result<Option<u32>, RomFsError> {
        let value = self.read_hash_bucket(
            u64::from(self.header.directory_hash_offset_bytes),
            u64::from(self.header.directory_hash_size_bytes),
            index,
        )?;
        Ok((value != u32::MAX).then_some(value))
    }

    /// Reads one file hash-bucket head. None represents the 0xFFFFFFFF sentinel.
    pub fn read_file_hash_bucket(&mut self, index: u32) -> Result<Option<u32>, RomFsError> {
        let value = self.read_hash_bucket(
            u64::from(self.header.file_hash_offset_bytes),
            u64::from(self.header.file_hash_size_bytes),
            index,
        )?;
        Ok((value != u32::MAX).then_some(value))
    }

    fn read_hash_bucket(
        &mut self,
        table_offset: u64,
        table_size: u64,
        index: u32,
    ) -> Result<u32, RomFsError> {
        let byte_offset =
            u64::from(index)
                .checked_mul(4)
                .ok_or(RomFsError::InvalidLevel3Header(
                    "hash bucket offset overflows",
                ))?;
        if byte_offset
            .checked_add(4)
            .is_none_or(|end| end > table_size)
        {
            return Err(RomFsError::InvalidLevel3Header(
                "hash bucket index exceeds its table",
            ));
        }

        let absolute =
            table_offset
                .checked_add(byte_offset)
                .ok_or(RomFsError::InvalidLevel3Header(
                    "hash bucket position overflows",
                ))?;
        let mut bytes = [0; 4];
        self.inner
            .seek(SeekFrom::Start(absolute))
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;
        self.inner
            .read_exact(&mut bytes)
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;
        Ok(u32::from_le_bytes(bytes))
    }
}

/// A seekable reader restricted to one RomFS file's data range.
#[derive(Debug)]
pub struct RomFsFileReader<'a, R> {
    inner: &'a mut R,
    start: u64,
    size: u64,
    position: u64,
}

impl<R: Read + Seek> Read for RomFsFileReader<'_, R> {
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

impl<R: Read + Seek> Seek for RomFsFileReader<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
        };

        if !(0..=i128::from(self.size)).contains(&target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek is outside the RomFS file",
            ));
        }

        let relative = u64::try_from(target).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid RomFS file position")
        })?;
        let absolute = self.start.checked_add(relative).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute RomFS file position overflows",
            )
        })?;
        let actual = self.inner.seek(SeekFrom::Start(absolute))?;
        if actual != absolute {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested RomFS file position",
            ));
        }

        self.position = relative;
        Ok(relative)
    }
}

fn validate_path_component(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("non-root entry has an empty name");
    }
    if name == "." || name == ".." {
        return Err("entry name is a dot path component");
    }
    if name
        .chars()
        .any(|character| matches!(character, '/' | '\\' | '\0'))
    {
        return Err("entry name contains a path separator or NUL");
    }
    Ok(())
}

fn append_path_component(parent: &str, component: &str) -> String {
    if parent.is_empty() {
        component.to_owned()
    } else {
        let mut path = String::with_capacity(parent.len() + 1 + component.len());
        path.push_str(parent);
        path.push('/');
        path.push_str(component);
        path
    }
}

fn validate_entry_offset(offset: u64, table_size: u64) -> Result<(), &'static str> {
    if offset % 4 != 0 {
        return Err("metadata record offset is not 4-byte aligned");
    }
    if offset >= table_size {
        return Err("metadata record offset exceeds its table");
    }
    Ok(())
}

fn validate_optional_reference(value: u32, table_size: u64) -> Result<(), &'static str> {
    if value == u32::MAX {
        return Ok(());
    }
    if value % 4 != 0 {
        return Err("metadata reference is not 4-byte aligned");
    }
    if u64::from(value) >= table_size {
        return Err("metadata reference exceeds its table");
    }
    Ok(())
}

fn read_metadata_fixed<const N: usize, R: Read + Seek>(
    reader: &mut R,
    table_start: u64,
    entry_offset: u64,
    table_size: u64,
) -> Result<[u8; N], &'static str> {
    let end = entry_offset
        .checked_add(N as u64)
        .ok_or("metadata record range overflows")?;
    if end > table_size {
        return Err("metadata record fixed fields exceed their table");
    }
    let absolute = table_start
        .checked_add(entry_offset)
        .ok_or("metadata record position overflows")?;
    reader
        .seek(SeekFrom::Start(absolute))
        .map_err(|_| "could not seek to metadata record")?;
    let mut bytes = [0; N];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| "metadata record is truncated")?;
    Ok(bytes)
}

fn read_metadata_name<R: Read + Seek>(
    reader: &mut R,
    table_start: u64,
    entry_offset: u64,
    table_size: u64,
    fixed_size: u64,
    name_size_bytes: u32,
) -> Result<String, &'static str> {
    if name_size_bytes % 2 != 0 {
        return Err("UTF-16LE name length is not even");
    }

    let padded_name_size = u64::from(name_size_bytes)
        .checked_add(3)
        .ok_or("padded name length overflows")?
        & !3;
    let name_offset = entry_offset
        .checked_add(fixed_size)
        .ok_or("name offset overflows")?;
    let end = name_offset
        .checked_add(padded_name_size)
        .ok_or("name range overflows")?;
    if end > table_size {
        return Err("name extends beyond its metadata table");
    }

    let absolute = table_start
        .checked_add(name_offset)
        .ok_or("name position overflows")?;
    reader
        .seek(SeekFrom::Start(absolute))
        .map_err(|_| "could not seek to metadata name")?;

    let byte_len = usize::try_from(name_size_bytes).map_err(|_| "name is too large")?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(byte_len)
        .map_err(|_| "could not allocate metadata name buffer")?;
    bytes.resize(byte_len, 0);
    reader
        .read_exact(&mut bytes)
        .map_err(|_| "metadata name is truncated")?;

    let code_units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&code_units).map_err(|_| "metadata name contains invalid UTF-16")
}

fn verify_ivfc_hash_table<R: Read + Seek>(
    reader: &mut R,
    data_region: IvfcRegion,
    block_size_bytes: u64,
    hash_region: IvfcRegion,
    level: u8,
    available_size_bytes: u64,
) -> Result<(), RomFsError> {
    if block_size_bytes > available_size_bytes {
        return Err(RomFsError::InvalidIvfcLevel {
            index: level,
            reason: "hash block size exceeds the RomFS image size",
        });
    }

    let block_count = data_region.size_bytes / block_size_bytes
        + u64::from(data_region.size_bytes % block_size_bytes != 0);
    let expected_hash_size =
        block_count
            .checked_mul(SHA256_SIZE)
            .ok_or(RomFsError::InvalidIvfcLevel {
                index: level,
                reason: "expected hash table size overflows",
            })?;
    if expected_hash_size != hash_region.size_bytes {
        return Err(RomFsError::InvalidIvfcLevel {
            index: level,
            reason: "hash table size does not match the number of data blocks",
        });
    }

    const IO_BUFFER_SIZE: usize = 8 * 1024;
    let mut data_buffer = [0u8; IO_BUFFER_SIZE];
    let zero_buffer = [0u8; IO_BUFFER_SIZE];
    let mut data_offset = 0u64;

    for block_index in 0..block_count {
        let hash_offset = hash_region
            .offset_bytes
            .checked_add(block_index.checked_mul(SHA256_SIZE).ok_or(
                RomFsError::InvalidIvfcLevel {
                    index: level,
                    reason: "hash entry offset overflows",
                },
            )?)
            .ok_or(RomFsError::InvalidIvfcLevel {
                index: level,
                reason: "hash entry offset overflows",
            })?;
        reader
            .seek(SeekFrom::Start(hash_offset))
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;

        let mut expected_digest = [0u8; SHA256_SIZE as usize];
        reader
            .read_exact(&mut expected_digest)
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;

        let block_data_size = (data_region.size_bytes - data_offset).min(block_size_bytes);
        let block_offset = data_region.offset_bytes.checked_add(data_offset).ok_or(
            RomFsError::InvalidIvfcLevel {
                index: level,
                reason: "data block offset overflows",
            },
        )?;
        reader
            .seek(SeekFrom::Start(block_offset))
            .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;

        let mut hasher = Sha256::new();
        let mut actual_remaining = block_data_size;
        while actual_remaining != 0 {
            let amount = actual_remaining.min(IO_BUFFER_SIZE as u64) as usize;
            reader
                .read_exact(&mut data_buffer[..amount])
                .map_err(|error| RomFsError::ReaderFailure(error.to_string()))?;
            hasher
                .update(&data_buffer[..amount])
                .map_err(|_| RomFsError::InvalidIvfcLevel {
                    index: level,
                    reason: "hash block exceeds the SHA-256 message limit",
                })?;
            actual_remaining -= amount as u64;
        }

        let mut padding_remaining = block_size_bytes - block_data_size;
        while padding_remaining != 0 {
            let amount = padding_remaining.min(IO_BUFFER_SIZE as u64) as usize;
            hasher
                .update(&zero_buffer[..amount])
                .map_err(|_| RomFsError::InvalidIvfcLevel {
                    index: level,
                    reason: "padded hash block exceeds the SHA-256 message limit",
                })?;
            padding_remaining -= amount as u64;
        }

        if hasher.finalize() != expected_digest {
            return Err(RomFsError::IvfcHashMismatch { level, block_index });
        }

        data_offset =
            data_offset
                .checked_add(block_data_size)
                .ok_or(RomFsError::InvalidIvfcLevel {
                    index: level,
                    reason: "processed data offset overflows",
                })?;
    }

    Ok(())
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
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
    fn resolves_level3_to_the_documented_physical_offset() {
        let header = IvfcHeader::parse(&valid_ivfc_header(), 0x4000).unwrap();
        let region = header.level3_region(0x4000).unwrap();

        assert_eq!(region.offset_bytes, 0x1000);
        assert_eq!(region.size_bytes, 0x100);
        assert_eq!(region.end_offset_bytes(), Some(0x1100));
    }

    #[test]
    fn resolves_all_ivfc_regions_in_documented_physical_order() {
        let header = IvfcHeader::parse(&valid_ivfc_header(), 0x4000).unwrap();
        let layout = header.physical_layout(0x4000).unwrap();

        assert_eq!(layout.master_hash.offset_bytes, 0x60);
        assert_eq!(layout.master_hash.size_bytes, 0x20);
        assert_eq!(layout.level3.offset_bytes, 0x1000);
        assert_eq!(layout.level3.size_bytes, 0x100);
        assert_eq!(layout.level1.offset_bytes, 0x2000);
        assert_eq!(layout.level1.size_bytes, 0x20);
        assert_eq!(layout.level2.offset_bytes, 0x3000);
        assert_eq!(layout.level2.size_bytes, 0x20);
    }

    #[test]
    fn aligns_each_following_level_to_the_preceding_level_block_size() {
        let mut bytes = valid_ivfc_header();
        write_u64(&mut bytes, 0x44, 0x1800);
        write_u32(&mut bytes, 0x1C, 12);
        write_u32(&mut bytes, 0x3C + 16, 12);
        write_u32(&mut bytes, 0x0C + 16, 13);

        let header = IvfcHeader::parse(&bytes, 0x5000).unwrap();
        let layout = header.physical_layout(0x5000).unwrap();

        assert_eq!(layout.level3.offset_bytes, 0x1000);
        assert_eq!(layout.level3.end_offset_bytes(), Some(0x2800));
        assert_eq!(layout.level1.offset_bytes, 0x3000);
        assert_eq!(layout.level1.end_offset_bytes(), Some(0x3020));
        assert_eq!(layout.level2.offset_bytes, 0x4000);
        assert_eq!(layout.level2.end_offset_bytes(), Some(0x4020));
    }

    #[test]
    fn rejects_level1_and_level2_regions_outside_the_romfs_image() {
        let header = IvfcHeader::parse(&valid_ivfc_header(), 0x4000).unwrap();
        assert_eq!(
            header.physical_layout(0x201F),
            Err(RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "physical Level 1 range exceeds the RomFS region",
            })
        );
        assert_eq!(
            header.physical_layout(0x301F),
            Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "physical Level 2 range exceeds the RomFS region",
            })
        );
    }

    #[test]
    fn rejects_ivfc_level_alignment_overflow() {
        let mut bytes = valid_ivfc_header();
        write_u64(&mut bytes, 0x44, 0x8000_0000_0000_0000);
        write_u32(&mut bytes, 0x3C + 16, 63);
        let header = IvfcHeader::parse(&bytes, u64::MAX).unwrap();
        assert_eq!(
            header.physical_layout(u64::MAX),
            Err(RomFsError::InvalidIvfcLevel {
                index: 1,
                reason: "physical Level 1 alignment overflows",
            })
        );

        bytes = valid_ivfc_header();
        write_u64(&mut bytes, 0x44, 0x8000_0000_0000_0000);
        write_u32(&mut bytes, 0x0C + 16, 63);
        let header = IvfcHeader::parse(&bytes, u64::MAX).unwrap();
        assert_eq!(
            header.physical_layout(u64::MAX),
            Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "physical Level 2 alignment overflows",
            })
        );
    }

    fn valid_integrity_ivfc_header() -> Vec<u8> {
        let mut data = vec![0; IVFC_HEADER_SIZE];
        data[..4].copy_from_slice(IVFC_MAGIC);
        write_u32(&mut data, 0x04, IVFC_VERSION);
        write_u32(&mut data, 0x08, 0x40);

        for (offset, logical, size, block_log2) in [
            (0x0C, 0, 0x40, 5),
            (0x24, 0x40, 0x40, 5),
            (0x3C, 0x80, 0x64, 6),
        ] {
            write_u64(&mut data, offset, logical);
            write_u64(&mut data, offset + 8, size);
            write_u32(&mut data, offset + 16, block_log2);
        }

        data
    }

    fn build_hash_table(data: &[u8], block_size: usize) -> Vec<u8> {
        let mut hashes = Vec::new();
        for block in data.chunks(block_size) {
            let mut hasher = Sha256::new();
            hasher.update(block).unwrap();
            let padding = vec![0; block_size - block.len()];
            hasher.update(&padding).unwrap();
            hashes.extend_from_slice(&hasher.finalize());
        }
        hashes
    }

    fn valid_integrity_ivfc_image() -> (Vec<u8>, IvfcHeader, IvfcLayout) {
        let header = IvfcHeader::parse(&valid_integrity_ivfc_header(), 0x2000).unwrap();
        let layout = header.physical_layout(0x2000).unwrap();
        let level3 = (0..0x64)
            .map(|index| (index as u8).wrapping_mul(17).wrapping_add(3))
            .collect::<Vec<_>>();
        let level2 = build_hash_table(&level3, 0x40);
        let level1 = build_hash_table(&level2, 0x20);
        let master_hash = build_hash_table(&level1, 0x20);

        assert_eq!(master_hash.len() as u64, layout.master_hash.size_bytes);
        assert_eq!(level1.len() as u64, layout.level1.size_bytes);
        assert_eq!(level2.len() as u64, layout.level2.size_bytes);
        assert_eq!(level3.len() as u64, layout.level3.size_bytes);

        let mut image = vec![0; layout.level2.end_offset_bytes().unwrap() as usize];
        let copy_region = |image: &mut [u8], region: IvfcRegion, contents: &[u8]| {
            let start = region.offset_bytes as usize;
            image[start..start + contents.len()].copy_from_slice(contents);
        };

        copy_region(&mut image, layout.master_hash, &master_hash);
        copy_region(&mut image, layout.level1, &level1);
        copy_region(&mut image, layout.level2, &level2);
        copy_region(&mut image, layout.level3, &level3);

        (image, header, layout)
    }

    #[test]
    fn verifies_complete_ivfc_hash_chain_with_partial_final_block_padding() {
        let (image, header, _) = valid_integrity_ivfc_image();
        header
            .verify_integrity(Cursor::new(image.clone()), image.len() as u64)
            .unwrap();
    }

    #[test]
    fn reports_level_and_block_for_ivfc_hash_mismatches() {
        let (mut image, header, layout) = valid_integrity_ivfc_image();
        image[layout.level3.offset_bytes as usize + 3] ^= 0x80;
        assert_eq!(
            header.verify_integrity(Cursor::new(image.clone()), image.len() as u64),
            Err(RomFsError::IvfcHashMismatch {
                level: 3,
                block_index: 0,
            })
        );

        let (mut image, header, layout) = valid_integrity_ivfc_image();
        image[layout.level2.offset_bytes as usize + 2] ^= 0x40;
        assert_eq!(
            header.verify_integrity(Cursor::new(image.clone()), image.len() as u64),
            Err(RomFsError::IvfcHashMismatch {
                level: 2,
                block_index: 0,
            })
        );

        let (mut image, header, layout) = valid_integrity_ivfc_image();
        image[layout.level1.offset_bytes as usize + 1] ^= 0x20;
        assert_eq!(
            header.verify_integrity(Cursor::new(image.clone()), image.len() as u64),
            Err(RomFsError::IvfcHashMismatch {
                level: 1,
                block_index: 0,
            })
        );

        let (mut image, header, layout) = valid_integrity_ivfc_image();
        image[layout.master_hash.offset_bytes as usize] ^= 0x01;
        assert_eq!(
            header.verify_integrity(Cursor::new(image.clone()), image.len() as u64),
            Err(RomFsError::IvfcHashMismatch {
                level: 1,
                block_index: 0,
            })
        );
    }

    #[test]
    fn rejects_hash_tables_with_an_incorrect_number_of_digests() {
        let mut bytes = valid_integrity_ivfc_header();
        write_u32(&mut bytes, 0x08, 0x20);
        write_u64(&mut bytes, 0x14, 0x20);
        let header = IvfcHeader::parse(&bytes, 0x2000).unwrap();
        let layout = header.physical_layout(0x2000).unwrap();
        let level1_data = vec![0xA5; SHA256_SIZE as usize];
        let master_hash = Sha256::digest(&level1_data).unwrap();
        let mut image = vec![0; layout.level2.end_offset_bytes().unwrap() as usize];

        let master_start = layout.master_hash.offset_bytes as usize;
        image[master_start..master_start + master_hash.len()].copy_from_slice(&master_hash);
        let level1_start = layout.level1.offset_bytes as usize;
        image[level1_start..level1_start + level1_data.len()].copy_from_slice(&level1_data);

        assert_eq!(
            header.verify_integrity(Cursor::new(image.clone()), image.len() as u64),
            Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "hash table size does not match the number of data blocks",
            })
        );
    }

    #[test]
    fn rejects_hash_blocks_larger_than_the_romfs_image() {
        let (image, _, _) = valid_integrity_ivfc_image();
        let mut bytes = valid_integrity_ivfc_header();
        write_u32(&mut bytes, 0x24 + 16, 63);
        let header = IvfcHeader::parse(&bytes, image.len() as u64).unwrap();

        assert_eq!(
            header.verify_integrity(Cursor::new(image.clone()), image.len() as u64),
            Err(RomFsError::InvalidIvfcLevel {
                index: 2,
                reason: "hash block size exceeds the RomFS image size",
            })
        );
    }

    #[test]
    fn opens_only_the_level3_physical_range() {
        let header = IvfcHeader::parse(&valid_ivfc_header(), 0x4000).unwrap();
        let mut image = vec![0xAA; 0x4000];
        let level3 = valid_level3_header();
        image[0x1000..0x1000 + LEVEL3_HEADER_SIZE].copy_from_slice(&level3);

        let reader = header
            .open_level3_reader(Cursor::new(image), 0x4000)
            .unwrap();
        assert_eq!(reader.len(), 0x100);
        assert_eq!(reader.header().directory_hash_offset_bytes, 0x28);
    }

    #[test]
    fn rejects_level3_physical_range_outside_the_romfs_region() {
        let mut bytes = valid_ivfc_header();
        write_u64(&mut bytes, 0x44, 0x4000);
        let header = IvfcHeader::parse(&bytes, 0x4FFF).unwrap();
        assert_eq!(
            header.level3_region(0x4FFF),
            Err(RomFsError::InvalidIvfcLevel {
                index: 3,
                reason: "physical Level 3 range exceeds the RomFS region",
            })
        );
    }

    #[test]
    fn range_reader_rejects_outside_reads_and_seeks() {
        use std::io::{Read, Seek, SeekFrom};

        let image = (0..32).collect::<Vec<u8>>();
        let mut reader = RomFsRangeReader::new(Cursor::new(image), 8, 4).unwrap();
        let mut bytes = [0; 8];
        assert_eq!(reader.read(&mut bytes).unwrap(), 4);
        assert_eq!(&bytes[..4], &[8, 9, 10, 11]);
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
        assert_eq!(
            reader.seek(SeekFrom::Start(5)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 4);
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

    fn valid_level3_image() -> Vec<u8> {
        let mut data = vec![0; 0x80];
        write_u32(&mut data, 0x00, 0x28);
        write_u32(&mut data, 0x04, 0x28);
        write_u32(&mut data, 0x28, u32::MAX);
        write_u32(&mut data, 0x08, 0x04);
        write_u32(&mut data, 0x0C, 0x2C);
        write_u32(&mut data, 0x10, 0x18);
        write_u32(&mut data, 0x14, 0x44);
        write_u32(&mut data, 0x18, 0x04);
        write_u32(&mut data, 0x1C, 0x48);
        write_u32(&mut data, 0x20, 0x24);
        write_u32(&mut data, 0x24, 0x6C);

        // Directory metadata at 0x2C: root directory, with one file.
        write_u32(&mut data, 0x2C, 0x00);
        write_u32(&mut data, 0x30, u32::MAX);
        write_u32(&mut data, 0x34, u32::MAX);
        write_u32(&mut data, 0x38, 0x00);
        write_u32(&mut data, 0x3C, u32::MAX);
        write_u32(&mut data, 0x40, 0x00);

        // File hash bucket at 0x44 points to the first file record.
        write_u32(&mut data, 0x44, 0x00);

        // File metadata at 0x48, UTF-16LE name "xy", data at offset 2.
        write_u32(&mut data, 0x48, 0x00);
        write_u32(&mut data, 0x4C, u32::MAX);
        data[0x50..0x58].copy_from_slice(&2u64.to_le_bytes());
        data[0x58..0x60].copy_from_slice(&5u64.to_le_bytes());
        write_u32(&mut data, 0x60, u32::MAX);
        write_u32(&mut data, 0x64, 4);
        data[0x68..0x6C].copy_from_slice(&[b'x', 0, b'y', 0]);

        data[0x6E..0x73].copy_from_slice(b"HELLO");
        data
    }

    fn valid_nested_level3_image() -> Vec<u8> {
        let mut data = vec![0; 0x93];
        write_u32(&mut data, 0x00, 0x28);
        write_u32(&mut data, 0x04, 0x28);
        write_u32(&mut data, 0x08, 0x04);
        write_u32(&mut data, 0x0C, 0x2C);
        write_u32(&mut data, 0x10, 0x38);
        write_u32(&mut data, 0x14, 0x64);
        write_u32(&mut data, 0x18, 0x04);
        write_u32(&mut data, 0x1C, 0x68);
        write_u32(&mut data, 0x20, 0x24);
        write_u32(&mut data, 0x24, 0x8C);

        // Empty directory hash bucket.
        write_u32(&mut data, 0x28, u32::MAX);

        // Root directory at metadata offset zero; child directory at 0x18.
        write_u32(&mut data, 0x2C, 0);
        write_u32(&mut data, 0x30, u32::MAX);
        write_u32(&mut data, 0x34, 0x18);
        write_u32(&mut data, 0x38, u32::MAX);
        write_u32(&mut data, 0x3C, u32::MAX);
        write_u32(&mut data, 0x40, 0);

        // Child directory named "sub", with file metadata record zero.
        write_u32(&mut data, 0x44, 0);
        write_u32(&mut data, 0x48, u32::MAX);
        write_u32(&mut data, 0x4C, u32::MAX);
        write_u32(&mut data, 0x50, 0);
        write_u32(&mut data, 0x54, u32::MAX);
        write_u32(&mut data, 0x58, 6);
        data[0x5C..0x62].copy_from_slice(&[b's', 0, b'u', 0, b'b', 0]);

        // File hash bucket and a file named "xy" under directory offset 0x18.
        write_u32(&mut data, 0x64, 0);
        write_u32(&mut data, 0x68, 0x18);
        write_u32(&mut data, 0x6C, u32::MAX);
        data[0x70..0x78].copy_from_slice(&2u64.to_le_bytes());
        data[0x78..0x80].copy_from_slice(&5u64.to_le_bytes());
        write_u32(&mut data, 0x80, u32::MAX);
        write_u32(&mut data, 0x84, 4);
        data[0x88..0x8C].copy_from_slice(&[b'x', 0, b'y', 0]);

        data[0x8E..0x93].copy_from_slice(b"HELLO");
        data
    }

    #[test]
    fn reads_root_directory_and_file_metadata() {
        let image = valid_level3_image();
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x80).unwrap();

        let root = reader.read_directory_entry(0).unwrap();
        assert_eq!(root.metadata_offset_bytes, 0);
        assert_eq!(root.parent_directory_offset_bytes, 0);
        assert_eq!(root.first_file_offset_bytes, 0);
        assert_eq!(root.name, "");

        let file = reader.read_file_entry(0).unwrap();
        assert_eq!(file.metadata_offset_bytes, 0);
        assert_eq!(file.parent_directory_offset_bytes, 0);
        assert_eq!(file.data_offset_bytes, 2);
        assert_eq!(file.data_size_bytes, 5);
        assert_eq!(file.name, "xy");
        assert_eq!(reader.read_file_hash_bucket(0).unwrap(), Some(0));
        assert_eq!(reader.read_directory_hash_bucket(0).unwrap(), None);
    }

    #[test]
    fn file_reader_is_bounded_to_file_data() {
        use std::io::{Read, Seek, SeekFrom};

        let image = valid_level3_image();
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x80).unwrap();
        let file = reader.read_file_entry(0).unwrap();
        let mut contents = Vec::new();

        {
            let mut file_reader = reader.open_file(&file).unwrap();
            file_reader.read_to_end(&mut contents).unwrap();
            assert_eq!(file_reader.seek(SeekFrom::End(0)).unwrap(), 5);
            assert_eq!(
                file_reader.seek(SeekFrom::Start(6)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }

        assert_eq!(contents, b"HELLO");
    }

    #[test]
    fn rejects_invalid_directory_offsets_references_and_utf16() {
        let mut image = valid_level3_image();
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image.clone()), 0x80).unwrap();

        assert!(matches!(
            reader.read_directory_entry(1),
            Err(RomFsError::InvalidDirectoryEntry(
                "metadata record offset is not 4-byte aligned"
            ))
        ));

        image[0x34..0x38].copy_from_slice(&0x100u32.to_le_bytes());
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x80).unwrap();
        assert!(matches!(
            reader.read_directory_entry(0),
            Err(RomFsError::InvalidDirectoryEntry(
                "metadata reference exceeds its table"
            ))
        ));

        let mut image = valid_level3_image();
        image[0x68..0x6C].copy_from_slice(&[0x00, 0xD8, 0x00, 0x00]);
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x80).unwrap();
        assert!(matches!(
            reader.read_file_entry(0),
            Err(RomFsError::InvalidFileEntry(
                "metadata name contains invalid UTF-16"
            ))
        ));
    }

    #[test]
    fn rejects_file_metadata_and_file_data_outside_their_ranges() {
        let mut image = valid_level3_image();
        image[0x50..0x58].copy_from_slice(&u64::MAX.to_le_bytes());
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x80).unwrap();
        assert!(matches!(
            reader.read_file_entry(0),
            Err(RomFsError::InvalidFileEntry("file-data range overflows"))
        ));

        let mut image = valid_level3_image();
        image[0x64..0x68].copy_from_slice(&0x100u32.to_le_bytes());
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x80).unwrap();
        assert!(matches!(
            reader.read_file_entry(0),
            Err(RomFsError::InvalidFileEntry(
                "name extends beyond its metadata table"
            ))
        ));
    }

    #[test]
    fn traverses_nested_directories_and_files_into_stable_paths() {
        let image = valid_nested_level3_image();
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x93).unwrap();
        let tree = reader.read_tree().unwrap();

        assert_eq!(tree.directories.len(), 2);
        assert_eq!(tree.directories[0].path, "");
        assert_eq!(tree.directories[1].path, "sub");
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.files[0].path, "sub/xy");
        assert_eq!(tree.files[0].entry.parent_directory_offset_bytes, 0x18);

        let mut contents = Vec::new();
        reader
            .open_file(&tree.files[0].entry)
            .unwrap()
            .read_to_end(&mut contents)
            .unwrap();
        assert_eq!(contents, b"HELLO");
    }

    #[test]
    fn rejects_directory_and_file_sibling_cycles() {
        let mut image = valid_nested_level3_image();
        write_u32(&mut image, 0x48, 0x18);
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x93).unwrap();
        assert_eq!(
            reader.read_tree(),
            Err(RomFsError::InvalidTree(
                "directory metadata record is referenced more than once or forms a cycle"
            ))
        );

        let mut image = valid_nested_level3_image();
        write_u32(&mut image, 0x6C, 0);
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x93).unwrap();
        assert_eq!(
            reader.read_tree(),
            Err(RomFsError::InvalidTree(
                "file metadata record is referenced more than once or forms a cycle"
            ))
        );
    }

    #[test]
    fn rejects_parent_mismatches_and_unsafe_path_components() {
        let mut image = valid_nested_level3_image();
        write_u32(&mut image, 0x44, 0x10);
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x93).unwrap();
        assert_eq!(
            reader.read_tree(),
            Err(RomFsError::InvalidTree(
                "directory parent does not match the directory containing it"
            ))
        );

        let mut image = valid_nested_level3_image();
        write_u32(&mut image, 0x58, 4);
        image[0x5C..0x60].copy_from_slice(&[b'.', 0, b'.', 0]);
        let mut reader = RomFsLevel3Reader::new(Cursor::new(image), 0x93).unwrap();
        assert_eq!(
            reader.read_tree(),
            Err(RomFsError::InvalidTree(
                "entry name is a dot path component"
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
    fn ncch_opens_level3_directly_from_its_romfs_region() {
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
        let level3 = valid_level3_header();
        image[0x5000..0x5000 + LEVEL3_HEADER_SIZE].copy_from_slice(&level3);

        let reader = parsed
            .open_romfs_level3_reader(Cursor::new(image))
            .unwrap()
            .unwrap();

        assert_eq!(reader.len(), 0x100);
        assert_eq!(reader.header().directory_hash_offset_bytes, 0x28);
    }

    #[test]
    fn ncch_verifies_ivfc_integrity_at_the_declared_romfs_offset() {
        use crate::loader::ncsd::ncch::NcchHeader;

        let mut ncch_header = vec![0; 0x200];
        ncch_header[0x100..0x104].copy_from_slice(b"NCCH");
        write_u32(&mut ncch_header, 0x104, 0x30);
        write_u32(&mut ncch_header, 0x180, 0x400);
        write_u32(&mut ncch_header, 0x1B0, 0x20);
        write_u32(&mut ncch_header, 0x1B4, 0x10);
        write_u32(&mut ncch_header, 0x1B8, 0x02);

        let parsed = NcchHeader::parse(&ncch_header, 0x6000).unwrap();
        let (romfs_image, _, layout) = valid_integrity_ivfc_image();
        let mut container = vec![0; 0x6000];
        container[..0x200].copy_from_slice(&ncch_header);
        container[0x4000..0x4000 + romfs_image.len()].copy_from_slice(&romfs_image);

        assert_eq!(
            parsed
                .verify_romfs_ivfc_integrity(Cursor::new(container.clone()))
                .unwrap(),
            Some(())
        );

        container[0x4000 + layout.level3.offset_bytes as usize + 5] ^= 0x40;
        assert_eq!(
            parsed.verify_romfs_ivfc_integrity(Cursor::new(container)),
            Err(RomFsError::IvfcHashMismatch {
                level: 3,
                block_index: 0,
            })
        );
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
        assert_eq!(
            parsed.verify_romfs_ivfc_integrity(Cursor::new(Vec::new())).unwrap(),
            None
        );
        assert_eq!(reader.position(), 0);
    }
}
