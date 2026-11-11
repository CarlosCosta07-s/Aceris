//! Structural parser for Nintendo 3DS NCSD cartridge images.
//!
//! The parser reads only the public container header. It does not verify the
//! RSA signature, decrypt partitions, or claim that partition contents are
//! executable. Those checks belong to later loading stages.

use aceris_common::{Error, Result};
use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

/// Structural NCCH parser for content stored inside an NCSD partition.
pub mod ncch;
/// Validated ARM11 process mappings derived from a prepared executable image.
pub mod process;
/// CPU and memory session for executing a loaded ARM11 application.
pub mod session;
pub use session::{Arm11ProcessSession, Arm11RunReport, Arm11RunStopReason};

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

    /// Opens a reader restricted to this partition's byte range.
    ///
    /// Reads stop at the partition boundary and seeks are relative to the
    /// beginning of the partition. The underlying reader is owned by the
    /// returned value, preventing outside code from moving it past the range.
    pub fn reader<R: Read + Seek>(self, inner: R) -> io::Result<NcsdPartitionReader<R>> {
        NcsdPartitionReader::new(inner, self)
    }

    /// Opens this partition as a structurally validated NCCH container.
    ///
    /// The returned value keeps the parsed header paired with the bounded
    /// partition reader. Content decryption and signature verification are
    /// separate operations on the returned value.
    pub fn open_ncch<R: Read + Seek>(
        self,
        inner: R,
    ) -> std::result::Result<NcsdNcchReader<R>, NcsdNcchError> {
        let mut reader = self.reader(inner).map_err(NcsdNcchError::PartitionReader)?;
        let header = ncch::NcchHeader::parse_reader(&mut reader, self.size_bytes)
            .map_err(NcsdNcchError::InvalidNcchHeader)?;

        Ok(NcsdNcchReader {
            partition: self,
            header,
            reader,
        })
    }
}

/// A seekable reader whose visible address space is one NCSD partition.
#[derive(Debug)]
pub struct NcsdPartitionReader<R> {
    inner: R,
    index: u8,
    start: u64,
    size: u64,
    position: u64,
}

impl<R: Read + Seek> NcsdPartitionReader<R> {
    fn new(mut inner: R, partition: NcsdPartition) -> io::Result<Self> {
        if partition.size_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NCSD partition has zero length",
            ));
        }

        let partition_end = partition.end_offset_bytes().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "NCSD partition range overflows",
            )
        })?;
        let image_len = inner.seek(SeekFrom::End(0))?;
        if partition_end > image_len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "NCSD partition extends beyond the underlying image",
            ));
        }

        let actual = inner.seek(SeekFrom::Start(partition.offset_bytes))?;
        if actual != partition.offset_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested partition offset",
            ));
        }

        Ok(Self {
            inner,
            index: partition.index,
            start: partition.offset_bytes,
            size: partition.size_bytes,
            position: 0,
        })
    }

    /// Returns the original NCSD partition-table slot.
    pub const fn partition_index(&self) -> u8 {
        self.index
    }

    /// Returns the absolute start offset of this partition in the image.
    pub const fn start_offset_bytes(&self) -> u64 {
        self.start
    }

    /// Returns the absolute exclusive end offset of this partition in the image.
    pub const fn end_offset_bytes(&self) -> u64 {
        self.start + self.size
    }

    /// Returns the current absolute position in the underlying image.
    pub const fn absolute_position(&self) -> u64 {
        self.start + self.position
    }

    /// Returns the visible length of the partition.
    pub const fn len(&self) -> u64 {
        self.size
    }

    /// Returns true when the partition has no bytes.
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the number of bytes remaining before the partition boundary.
    pub const fn remaining(&self) -> u64 {
        self.size - self.position
    }

    /// Returns the current position relative to the partition start.
    pub const fn position(&self) -> u64 {
        self.position
    }
}

impl<R: Read + Seek> Read for NcsdPartitionReader<R> {
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

impl<R: Read + Seek> Seek for NcsdPartitionReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
        };

        if !(0..=i128::from(self.size)).contains(&target) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek is outside the NCSD partition",
            ));
        }

        let relative = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "relative partition position is invalid",
            )
        })?;
        let absolute = self.start.checked_add(relative).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute partition position overflows",
            )
        })?;

        let actual = self.inner.seek(SeekFrom::Start(absolute))?;
        if actual != absolute {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested position",
            ));
        }

        self.position = relative;
        Ok(relative)
    }
}

/// Errors produced while opening an NCSD partition as an NCCH container.
#[derive(Debug)]
pub enum NcsdNcchError {
    /// The bounded NCSD partition reader could not be created.
    PartitionReader(io::Error),
    /// The partition does not contain a valid structural NCCH header.
    InvalidNcchHeader(ncch::NcchError),
}

impl fmt::Display for NcsdNcchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartitionReader(source) => {
                write!(formatter, "could not open NCSD partition: {source}")
            }
            Self::InvalidNcchHeader(source) => {
                write!(
                    formatter,
                    "partition does not contain a valid NCCH header: {source}"
                )
            }
        }
    }
}

impl std::error::Error for NcsdNcchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PartitionReader(source) => Some(source),
            Self::InvalidNcchHeader(source) => Some(source),
        }
    }
}

/// An NCCH header paired with the bounded reader for its containing NCSD partition.
///
/// Keeping these values together prevents offsets from the NCCH header from
/// accidentally being applied to the full cartridge image.
#[derive(Debug)]
pub struct NcsdNcchReader<R> {
    partition: NcsdPartition,
    header: ncch::NcchHeader,
    reader: NcsdPartitionReader<R>,
}

/// ARM11 executable bytes prepared from an NCSD/NCCH program partition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedArm11Image {
    /// Verified and parsed NCCH extended header, including BSS size and addresses.
    pub extended_header: ncch::extended_header::NcchExtendedHeader,
    /// Page-aligned ARM11 text bytes from ExeFS .code.
    pub text: Vec<u8>,
    /// Page-aligned read-only bytes from ExeFS .code.
    pub read_only: Vec<u8>,
    /// Page-aligned initialized-data bytes from ExeFS .code.
    pub data: Vec<u8>,
}

/// Failures encountered while preparing an ARM11 executable image.
#[derive(Debug)]
pub enum NcsdExecutableLoadError {
    MissingExtendedHeader,
    InvalidCodeLayout,
    CodeImageTooLarge {
        requested_size: usize,
        maximum_size: usize,
    },
    CodeFileSizeMismatch {
        expected_size: usize,
        actual_size: usize,
    },
    ExtendedHeaderIntegrity(ncch::NcchIntegrityError),
    ExtendedHeaderParse(ncch::extended_header::ExtendedHeaderError),
    ProcessLayout(process::Arm11ProcessLayoutError),
    ExeFs(ncch::exefs::ExeFsError),
    Blz(ncch::blz::BlzError),
    ReaderFailure(String),
    AllocationFailure(String),
}

impl fmt::Display for NcsdExecutableLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingExtendedHeader => {
                formatter.write_str("NCCH program does not contain an extended header")
            }
            Self::InvalidCodeLayout => {
                formatter.write_str("NCCH extended header contains an invalid ARM11 code layout")
            }
            Self::CodeImageTooLarge {
                requested_size,
                maximum_size,
            } => write!(
                formatter,
                "ARM11 code image size {requested_size} exceeds the {maximum_size}-byte safety limit"
            ),
            Self::CodeFileSizeMismatch {
                expected_size,
                actual_size,
            } => write!(
                formatter,
                "ExeFS .code has {actual_size} bytes, expected {expected_size} from the extended header"
            ),
            Self::ExtendedHeaderIntegrity(error) => {
                write!(formatter, "NCCH extended-header integrity failed: {error}")
            }
            Self::ExtendedHeaderParse(error) => {
                write!(formatter, "could not parse NCCH extended header: {error}")
            }
            Self::ProcessLayout(error) => {
                write!(formatter, "ARM11 process layout validation failed: {error}")
            }
            Self::ExeFs(error) => write!(formatter, "ExeFS loading failed: {error}"),
            Self::Blz(error) => write!(formatter, "ExeFS .code decompression failed: {error}"),
            Self::ReaderFailure(reason) => {
                write!(formatter, "could not read ExeFS .code: {reason}")
            }
            Self::AllocationFailure(reason) => {
                write!(
                    formatter,
                    "could not allocate the ARM11 code image: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for NcsdExecutableLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ExtendedHeaderIntegrity(error) => Some(error),
            Self::ExtendedHeaderParse(error) => Some(error),
            Self::ProcessLayout(error) => Some(error),
            Self::ExeFs(error) => Some(error),
            Self::Blz(error) => Some(error),
            _ => None,
        }
    }
}

/// One ExeFS file reader returned after its NCCH superblock and file digest validate.
pub type VerifiedNcsdExeFsFileReader<R> =
    ncch::exefs::ExeFsFileReader<ncch::romfs::RomFsRangeReader<NcsdPartitionReader<R>>>;

/// Decrypted ExeFS file reader returned after NCCH and file hashes validate.
pub type DecryptedVerifiedNcsdExeFsFileReader<R> =
    ncch::exefs::ExeFsFileReader<ncch::crypto::NcchCtrReader<NcsdPartitionReader<R>>>;

/// Decrypted extended-header reader returned after the NCCH SHA-256 check.
pub type DecryptedVerifiedNcsdExtendedHeaderReader<R> =
    ncch::crypto::NcchCtrReader<NcsdPartitionReader<R>>;

/// RomFS Level 3 reader returned after NCCH superblock and IVFC hashes validate.
pub type VerifiedNcsdRomFsLevel3Reader<R> = ncch::romfs::RomFsLevel3Reader<
    ncch::romfs::RomFsRangeReader<ncch::romfs::RomFsRangeReader<NcsdPartitionReader<R>>>,
>;

impl<R: Read + Seek> NcsdNcchReader<R> {
    /// Returns the NCSD partition that contains this NCCH container.
    pub const fn partition(&self) -> NcsdPartition {
        self.partition
    }

    /// Returns the structurally validated NCCH header.
    pub const fn header(&self) -> &ncch::NcchHeader {
        &self.header
    }

    /// Provides mutable access to the bounded partition reader.
    pub fn reader_mut(&mut self) -> &mut NcsdPartitionReader<R> {
        &mut self.reader
    }

    /// Verifies the ExeFS superblock and all populated file hashes.
    pub fn verify_exefs_integrity(
        &mut self,
    ) -> std::result::Result<Option<()>, ncch::exefs::ExeFsError> {
        self.header.verify_exefs_integrity(&mut self.reader)
    }

    /// Opens one ExeFS file only after its superblock and file hash validate.
    ///
    /// The returned stream remains bounded to the selected file.
    pub fn open_verified_exefs_file_reader(
        self,
        file_name: &str,
    ) -> std::result::Result<Option<VerifiedNcsdExeFsFileReader<R>>, ncch::exefs::ExeFsError> {
        self.header
            .open_verified_exefs_file_reader(self.reader, file_name)
    }

    /// Opens the encrypted NCCH extended header after verifying its SHA-256.
    ///
    /// The caller supplies the AES-128 menu-info key. The reader remains
    /// bounded to the containing NCSD partition and declared NCCH header.
    pub fn open_decrypted_verified_extended_header_reader(
        self,
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> std::result::Result<
        Option<DecryptedVerifiedNcsdExtendedHeaderReader<R>>,
        ncch::NcchIntegrityError,
    > {
        self.header
            .open_decrypted_verified_extended_header_reader(self.reader, menu_info_key)
    }

    /// Opens a file from encrypted ExeFS after validating both SHA-256 layers.
    ///
    /// The menu-info and content AES-128 keys are supplied by the caller.
    /// The returned stream is restricted to the selected file and decrypts
    /// bytes as they are read.
    pub fn open_decrypted_verified_exefs_file_reader(
        self,
        file_name: &str,
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
        content_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> std::result::Result<Option<DecryptedVerifiedNcsdExeFsFileReader<R>>, ncch::exefs::ExeFsError>
    {
        self.header.open_decrypted_verified_exefs_file_reader(
            self.reader,
            file_name,
            menu_info_key,
            content_key,
        )
    }

    /// Prepares the verified ARM11 code image from this NCSD program partition.
    ///
    /// The caller supplies the menu-info and content AES-128 keys. For NCCH
    /// NoCrypto images they are not used; cleartext integrity readers are used
    /// instead. The extended header and ExeFS .code hashes are always checked.
    ///
    /// If the extended-header compression flag is set, .code is decompressed
    /// using the bounded backwards-LZ decoder. The resulting bytes are split
    /// into page-aligned text, read-only, and initialized-data vectors. BSS is
    /// not part of .code and remains described by extended_header.bss_size_bytes.
    ///
    /// This prepares the image only; it does not map memory, configure the
    /// ARM11 process, or start CPU execution.
    pub fn prepare_arm11_executable_image(
        mut self,
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
        content_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> std::result::Result<PreparedArm11Image, NcsdExecutableLoadError> {
        let is_cleartext = self.header.declares_no_crypto();

        let extended_header = if is_cleartext {
            let mut reader = self
                .header
                .open_verified_extended_header_reader(&mut self.reader)
                .map_err(NcsdExecutableLoadError::ExtendedHeaderIntegrity)?
                .ok_or(NcsdExecutableLoadError::MissingExtendedHeader)?;
            ncch::extended_header::NcchExtendedHeader::parse_reader(&mut reader)
                .map_err(NcsdExecutableLoadError::ExtendedHeaderParse)?
        } else {
            let mut reader = self
                .header
                .open_decrypted_verified_extended_header_reader(&mut self.reader, menu_info_key)
                .map_err(NcsdExecutableLoadError::ExtendedHeaderIntegrity)?
                .ok_or(NcsdExecutableLoadError::MissingExtendedHeader)?;
            ncch::extended_header::NcchExtendedHeader::parse_reader(&mut reader)
                .map_err(NcsdExecutableLoadError::ExtendedHeaderParse)?
        };

        let layout = extended_header
            .code_file_layout()
            .filter(|layout| layout.total_size_bytes != 0)
            .ok_or(NcsdExecutableLoadError::InvalidCodeLayout)?;
        let maximum_size = ncch::blz::MAX_DECOMPRESSED_SIZE;
        if layout.total_size_bytes > maximum_size {
            return Err(NcsdExecutableLoadError::CodeImageTooLarge {
                requested_size: layout.total_size_bytes,
                maximum_size,
            });
        }

        let stored_code = if is_cleartext {
            let mut reader = self
                .header
                .open_verified_exefs_file_reader(&mut self.reader, ".code")
                .map_err(NcsdExecutableLoadError::ExeFs)?
                .ok_or_else(|| {
                    NcsdExecutableLoadError::ExeFs(ncch::exefs::ExeFsError::FileNotFound(
                        ".code".to_owned(),
                    ))
                })?;
            read_verified_code_file(&mut reader, maximum_size)?
        } else {
            let mut reader = self
                .header
                .open_decrypted_verified_exefs_file_reader(
                    &mut self.reader,
                    ".code",
                    menu_info_key,
                    content_key,
                )
                .map_err(NcsdExecutableLoadError::ExeFs)?
                .ok_or_else(|| {
                    NcsdExecutableLoadError::ExeFs(ncch::exefs::ExeFsError::FileNotFound(
                        ".code".to_owned(),
                    ))
                })?;
            read_verified_code_file(&mut reader, maximum_size)?
        };

        let mut code = if extended_header.compresses_exefs_code() {
            ncch::blz::decompress(&stored_code, layout.total_size_bytes)
                .map_err(NcsdExecutableLoadError::Blz)?
        } else {
            if stored_code.len() != layout.total_size_bytes {
                return Err(NcsdExecutableLoadError::CodeFileSizeMismatch {
                    expected_size: layout.total_size_bytes,
                    actual_size: stored_code.len(),
                });
            }
            stored_code
        };

        let text_end = layout.text_size_bytes;
        let read_only_end = text_end
            .checked_add(layout.read_only_size_bytes)
            .ok_or(NcsdExecutableLoadError::InvalidCodeLayout)?;
        if code.len() != layout.total_size_bytes || read_only_end > code.len() {
            return Err(NcsdExecutableLoadError::InvalidCodeLayout);
        }

        let data = code.split_off(read_only_end);
        let read_only = code.split_off(text_end);
        let text = code;

        Ok(PreparedArm11Image {
            extended_header,
            text,
            read_only,
            data,
        })
    }

    /// Loads the verified executable image into a bootstrap ARM11 address space.
    ///
    /// This is the single Rust entry point for NCSD partition -> NCCH/ExeFS ->
    /// verified and optionally decompressed .code -> text/rodata/data/BSS/stack.
    /// Menu-info and content keys are supplied by the caller and are never
    /// discovered or stored by Aceris.
    ///
    /// The resulting bus can be passed directly to Arm11Core. Call
    /// Arm11CodeAddressSpace::initialize_core to set the initial PC and SP.
    /// Privileged CPU modes, exception vectors, scheduler state, shared memory,
    /// and hardware peripherals are not initialized by this bootstrap loader.
    pub fn load_arm11_address_space(
        self,
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
        content_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> std::result::Result<process::Arm11CodeAddressSpace, NcsdExecutableLoadError> {
        let prepared = self.prepare_arm11_executable_image(menu_info_key, content_key)?;
        process::Arm11CodeAddressSpace::from_prepared(prepared)
            .map_err(NcsdExecutableLoadError::ProcessLayout)
    }

    /// Opens RomFS Level 3 only after the NCCH superblock and IVFC hashes validate.
    ///
    /// An empty result means the NCCH header does not declare a RomFS region.
    /// This does not decrypt NCCH content or verify its RSA signature.
    pub fn open_verified_romfs_level3_reader(
        self,
    ) -> std::result::Result<Option<VerifiedNcsdRomFsLevel3Reader<R>>, ncch::romfs::RomFsError>
    {
        self.header.open_verified_romfs_level3_reader(self.reader)
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

            let offset_bytes = u64::from(offset_units).checked_mul(media_unit_size).ok_or(
                Error::InvalidNcsdPartition {
                    index,
                    reason: "offset overflows",
                },
            )?;
            let size_bytes = u64::from(size_units).checked_mul(media_unit_size).ok_or(
                Error::InvalidNcsdPartition {
                    index,
                    reason: "size overflows",
                },
            )?;
            let end_bytes =
                offset_bytes
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
        self.partitions
            .iter()
            .find(|partition| partition.index == index)
    }

    /// Returns partition zero, conventionally the executable game content.
    pub fn primary_partition(&self) -> Option<&NcsdPartition> {
        self.partition(0)
    }
}

/// A file-backed NCSD image with a parsed header and lazy partition access.
///
/// The image bytes remain in the supplied reader; this type reads only the
/// 0x200-byte NCSD header initially. Opening a partition creates a bounded
/// reader over the original source without copying the cartridge into memory.
#[derive(Debug)]
pub struct NcsdImageReader<R> {
    header: NcsdHeader,
    reader: R,
}

impl<R: Read + Seek> NcsdImageReader<R> {
    /// Opens and structurally validates an NCSD image from any seekable reader.
    pub fn open(mut reader: R) -> std::result::Result<Self, NcsdImageReaderError> {
        let file_len = reader
            .seek(SeekFrom::End(0))
            .map_err(NcsdImageReaderError::Reader)?;
        if file_len < HEADER_SIZE as u64 {
            return Err(NcsdImageReaderError::InvalidHeader(
                Error::InvalidNcsdHeader("header is truncated"),
            ));
        }

        let position = reader
            .seek(SeekFrom::Start(0))
            .map_err(NcsdImageReaderError::Reader)?;
        if position != 0 {
            return Err(NcsdImageReaderError::Reader(io::Error::new(
                io::ErrorKind::InvalidData,
                "underlying reader did not seek to the beginning of the NCSD image",
            )));
        }

        let mut header_bytes = [0u8; HEADER_SIZE];
        reader
            .read_exact(&mut header_bytes)
            .map_err(NcsdImageReaderError::Reader)?;
        let header = NcsdHeader::parse(&header_bytes, file_len)
            .map_err(NcsdImageReaderError::InvalidHeader)?;

        Ok(Self { header, reader })
    }

    /// Returns the validated NCSD metadata without moving the underlying reader.
    pub const fn header(&self) -> &NcsdHeader {
        &self.header
    }

    /// Borrows a bounded reader for any populated partition by its original table index.
    ///
    /// Unlike open_partition_reader, this method keeps the NCSD image reader
    /// available. Drop the returned reader before borrowing another partition.
    /// The borrow prevents simultaneous access to the underlying image.
    pub fn borrow_partition_reader(
        &mut self,
        index: u8,
    ) -> std::result::Result<NcsdPartitionReader<&mut R>, NcsdImageReaderError> {
        let partition = self
            .header
            .partition(index)
            .copied()
            .ok_or(NcsdImageReaderError::MissingPartition { index })?;
        partition
            .reader(&mut self.reader)
            .map_err(NcsdImageReaderError::Reader)
    }

    /// Borrows a partition and validates its NCCH header without consuming the image reader.
    ///
    /// The returned NCCH reader is bounded to the selected partition. Drop it
    /// before borrowing another partition from this image.
    pub fn borrow_partition_ncch(
        &mut self,
        index: u8,
    ) -> std::result::Result<NcsdNcchReader<&mut R>, NcsdImageReaderError> {
        let partition = self
            .header
            .partition(index)
            .copied()
            .ok_or(NcsdImageReaderError::MissingPartition { index })?;
        partition
            .open_ncch(&mut self.reader)
            .map_err(NcsdImageReaderError::InvalidNcch)
    }

    /// Opens any populated partition by its original NCSD table index.
    ///
    /// This is a raw, bounded partition stream: it does not assume that the
    /// partition contains NCCH. The image reader is consumed and the returned
    /// reader cannot seek or read outside the selected partition.
    pub fn open_partition_reader(
        self,
        index: u8,
    ) -> std::result::Result<NcsdPartitionReader<R>, NcsdImageReaderError> {
        let partition = self
            .header
            .partition(index)
            .copied()
            .ok_or(NcsdImageReaderError::MissingPartition { index })?;
        partition
            .reader(self.reader)
            .map_err(NcsdImageReaderError::Reader)
    }

    /// Opens the partition with the given original table index as an NCCH container.
    ///
    /// The image reader is consumed so the returned partition owns the underlying
    /// source and remains bounded to the selected partition.
    pub fn open_partition_ncch(
        self,
        index: u8,
    ) -> std::result::Result<NcsdNcchReader<R>, NcsdImageReaderError> {
        let partition = self
            .header
            .partition(index)
            .copied()
            .ok_or(NcsdImageReaderError::MissingPartition { index })?;
        partition
            .open_ncch(self.reader)
            .map_err(NcsdImageReaderError::InvalidNcch)
    }

    /// Opens partition zero, conventionally the executable game-content partition.
    pub fn open_primary_ncch(self) -> std::result::Result<NcsdNcchReader<R>, NcsdImageReaderError> {
        self.open_partition_ncch(0)
    }

    /// Loads the primary NCSD program directly into a bootstrap ARM11 address space.
    ///
    /// This is the shortest file-backed loading path: open the image reader,
    /// supply the menu-info and content AES keys, and receive a ready-to-use
    /// Bus. NCSD, NCCH, ExeFS, hashes, optional BLZ, BSS, and stack are all
    /// validated or prepared by the underlying Rust loader stages.
    pub fn load_primary_arm11_address_space(
        self,
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
        content_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> std::result::Result<process::Arm11CodeAddressSpace, NcsdImageReaderError> {
        self.open_primary_ncch()?
            .load_arm11_address_space(menu_info_key, content_key)
            .map_err(NcsdImageReaderError::ExecutableLoad)
    }
}

/// Errors produced while opening an NCSD image and selecting one of its NCCH partitions.
#[derive(Debug)]
pub enum NcsdImageReaderError {
    /// The underlying image could not be read or positioned.
    Reader(io::Error),
    /// The NCSD header or its partition table is invalid.
    InvalidHeader(Error),
    /// The requested original table slot is not populated.
    MissingPartition { index: u8 },
    /// The selected partition does not contain a structurally valid NCCH header.
    InvalidNcch(NcsdNcchError),
    /// The selected NCSD program partition could not be prepared for ARM11.
    ExecutableLoad(NcsdExecutableLoadError),
}

impl fmt::Display for NcsdImageReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reader(error) => write!(formatter, "could not read NCSD image: {error}"),
            Self::InvalidHeader(error) => write!(formatter, "invalid NCSD image: {error}"),
            Self::MissingPartition { index } => {
                write!(formatter, "NCSD partition table slot {index} is empty")
            }
            Self::InvalidNcch(error) => {
                write!(
                    formatter,
                    "selected NCSD partition is not a valid NCCH: {error}"
                )
            }
            Self::ExecutableLoad(error) => {
                write!(
                    formatter,
                    "could not load ARM11 program from NCSD image: {error}"
                )
            }
        }
    }
}

impl std::error::Error for NcsdImageReaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reader(error) => Some(error),
            Self::InvalidHeader(error) => Some(error),
            Self::InvalidNcch(error) => Some(error),
            Self::ExecutableLoad(error) => Some(error),
            Self::MissingPartition { .. } => None,
        }
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

fn read_verified_code_file<R: Read + Seek>(
    reader: &mut ncch::exefs::ExeFsFileReader<R>,
    maximum_size: usize,
) -> std::result::Result<Vec<u8>, NcsdExecutableLoadError> {
    let stored_size =
        usize::try_from(reader.len()).map_err(|_| NcsdExecutableLoadError::CodeImageTooLarge {
            requested_size: usize::MAX,
            maximum_size,
        })?;
    if stored_size > maximum_size {
        return Err(NcsdExecutableLoadError::CodeImageTooLarge {
            requested_size: stored_size,
            maximum_size,
        });
    }

    let mut code = Vec::new();
    code.try_reserve_exact(stored_size)
        .map_err(|error| NcsdExecutableLoadError::AllocationFailure(error.to_string()))?;
    code.resize(stored_size, 0);
    reader
        .read_exact(&mut code)
        .map_err(|error| NcsdExecutableLoadError::ReaderFailure(error.to_string()))?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Seek, SeekFrom};

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

    fn image_with_primary_ncch() -> Vec<u8> {
        const PARTITION_OFFSET: usize = 0x4000;
        const PARTITION_SIZE: usize = 0x4000;

        let mut image = vec![0; PARTITION_OFFSET + PARTITION_SIZE];
        image[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"NCSD");
        write_u32(&mut image, MEDIA_SIZE_OFFSET, 0x40);
        write_partition(&mut image, 0, 0x20, 0x20);

        let ncch_start = PARTITION_OFFSET;
        image[ncch_start + 0x100..ncch_start + 0x104].copy_from_slice(b"NCCH");
        write_u32(&mut image, ncch_start + 0x104, 0x20);
        image[ncch_start + 0x118..ncch_start + 0x120]
            .copy_from_slice(&0x0004_0000_1234_5678u64.to_le_bytes());
        image
    }

    fn sha256_digest(data: &[u8]) -> [u8; 32] {
        let mut hasher = ncch::romfs::sha256::Sha256::new();
        hasher.update(data).unwrap();
        hasher.finalize()
    }

    fn compressed_test_code(segments: &[(u8, usize)]) -> Vec<u8> {
        enum Operation {
            Literal(u8),
            BackReference(usize),
        }

        let mut operations = Vec::new();
        for (byte, segment_size) in segments {
            assert!(*segment_size >= 3);
            operations.extend((0..3).map(|_| Operation::Literal(*byte)));
            let mut remaining = segment_size - 3;
            while remaining >= 3 {
                let length = remaining.min(18);
                operations.push(Operation::BackReference(length));
                remaining -= length;
            }
            operations.extend((0..remaining).map(|_| Operation::Literal(*byte)));
        }

        let mut decode_order = Vec::new();
        for group in operations.chunks(8) {
            let flags = group
                .iter()
                .enumerate()
                .fold(0u8, |flags, (index, operation)| {
                    if matches!(operation, Operation::BackReference(_)) {
                        flags | (0x80 >> index)
                    } else {
                        flags
                    }
                });
            decode_order.push(flags);
            for operation in group {
                match operation {
                    Operation::Literal(byte) => decode_order.push(*byte),
                    Operation::BackReference(length) => {
                        let encoded = (*length - 3) << 12;
                        decode_order.push((encoded >> 8) as u8);
                        decode_order.push(encoded as u8);
                    }
                }
            }
        }

        decode_order.reverse();
        let compressed_length = decode_order.len() + 8;
        let output_size: usize = segments.iter().map(|(_, size)| size).sum();
        let extra_size = output_size - compressed_length;
        let info = (8u32 << 24) | u32::try_from(compressed_length).unwrap();

        decode_order.extend_from_slice(&info.to_le_bytes());
        decode_order.extend_from_slice(&u32::try_from(extra_size).unwrap().to_le_bytes());
        decode_order
    }

    fn image_with_arm11_code(compressed: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        const PARTITION_OFFSET: usize = 0x4000;
        const PARTITION_SIZE: usize = 0x8000;
        const EXEFS_OFFSET: usize = 0x2000;
        const EXEFS_SIZE: usize = 0x4000;
        const CODE_SEGMENT_SIZE: usize = 0x1000;

        let text = vec![0x11; CODE_SEGMENT_SIZE];
        let read_only = vec![0x22; CODE_SEGMENT_SIZE];
        let data = vec![0x33; CODE_SEGMENT_SIZE];
        let code = if compressed {
            compressed_test_code(&[
                (0x33, CODE_SEGMENT_SIZE),
                (0x22, CODE_SEGMENT_SIZE),
                (0x11, CODE_SEGMENT_SIZE),
            ])
        } else {
            [text.as_slice(), read_only.as_slice(), data.as_slice()].concat()
        };

        let mut image = vec![0u8; PARTITION_OFFSET + PARTITION_SIZE];
        image[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"NCSD");
        let media_size_units = u32::try_from(image.len() / 0x200).unwrap();
        write_u32(&mut image, MEDIA_SIZE_OFFSET, media_size_units);
        write_partition(&mut image, 0, 0x20, 0x40);

        let ncch_start = PARTITION_OFFSET;
        image[ncch_start + 0x100..ncch_start + 0x104].copy_from_slice(b"NCCH");
        write_u32(&mut image, ncch_start + 0x104, 0x40);
        write_u32(&mut image, ncch_start + 0x180, 0x400);
        image[ncch_start + 0x18F] = 0x04;
        write_u32(&mut image, ncch_start + 0x1A0, 0x10);
        write_u32(&mut image, ncch_start + 0x1A4, 0x20);
        write_u32(&mut image, ncch_start + 0x1A8, 1);

        let mut extended_header = vec![0u8; 0x400];
        extended_header[0..8].copy_from_slice(b"ACERIS!!");
        if compressed {
            extended_header[0x0D] = 0x01;
        }
        write_u32(&mut extended_header, 0x10, 0x0010_0000);
        write_u32(&mut extended_header, 0x1C, 0x4000);
        write_u32(&mut extended_header, 0x14, 1);
        write_u32(&mut extended_header, 0x18, CODE_SEGMENT_SIZE as u32);
        write_u32(&mut extended_header, 0x20, 0x0010_1000);
        write_u32(&mut extended_header, 0x24, 1);
        write_u32(&mut extended_header, 0x28, CODE_SEGMENT_SIZE as u32);
        write_u32(&mut extended_header, 0x30, 0x0010_2000);
        write_u32(&mut extended_header, 0x34, 2);
        write_u32(&mut extended_header, 0x38, CODE_SEGMENT_SIZE as u32);
        write_u32(&mut extended_header, 0x3C, 0x20);

        let extended_hash = sha256_digest(&extended_header);
        image[ncch_start + 0x160..ncch_start + 0x180].copy_from_slice(&extended_hash);
        image[ncch_start + 0x200..ncch_start + 0x600].copy_from_slice(&extended_header);

        let mut exefs = vec![0u8; EXEFS_SIZE];
        exefs[..5].copy_from_slice(b".code");
        write_u32(&mut exefs, 12, u32::try_from(code.len()).unwrap());
        exefs[0x200..0x200 + code.len()].copy_from_slice(&code);
        let code_hash = sha256_digest(&code);
        exefs[0x1E0..0x200].copy_from_slice(&code_hash);
        let superblock_hash = sha256_digest(&exefs[..0x200]);
        image[ncch_start + 0x1C0..ncch_start + 0x1E0].copy_from_slice(&superblock_hash);

        let exefs_start = ncch_start + EXEFS_OFFSET;
        image[exefs_start..exefs_start + EXEFS_SIZE].copy_from_slice(&exefs);

        (image, text, read_only, data)
    }

    fn encrypt_arm11_code_image(
        mut image: Vec<u8>,
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
        content_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> Vec<u8> {
        const PARTITION_OFFSET: usize = 0x4000;
        const PARTITION_SIZE: usize = 0x8000;
        const EXEFS_OFFSET: usize = 0x2000;

        let ncch_start = PARTITION_OFFSET;
        image[ncch_start + 0x18F] = 0;
        let header = ncch::NcchHeader::parse(
            &image[ncch_start..ncch_start + ncch::HEADER_SIZE],
            PARTITION_SIZE as u64,
        )
        .unwrap();

        let extended_header_counter = header
            .initial_ctr_for_region(ncch::crypto::NcchCtrRegion::ExtendedHeader)
            .unwrap();
        let extended_header_start = ncch_start + 0x200;
        ncch::crypto::Aes128Ctr::new(menu_info_key, extended_header_counter)
            .apply_keystream(&mut image[extended_header_start..extended_header_start + 0x400])
            .unwrap();

        let exefs_counter = header
            .initial_ctr_for_region(ncch::crypto::NcchCtrRegion::ExeFs)
            .unwrap();
        let exefs_start = ncch_start + EXEFS_OFFSET;
        let stored_code_size = u32::from_le_bytes(
            image[exefs_start + 12..exefs_start + 16]
                .try_into()
                .unwrap(),
        ) as usize;

        ncch::crypto::Aes128Ctr::new(menu_info_key, exefs_counter)
            .apply_keystream(&mut image[exefs_start..exefs_start + 0x200])
            .unwrap();
        let code_start = exefs_start + 0x200;
        ncch::crypto::Aes128Ctr::apply_keystream_at_offset(
            content_key,
            exefs_counter,
            0x200,
            &mut image[code_start..code_start + stored_code_size],
        )
        .unwrap();

        image
    }

    fn image_with_verified_exefs_file() -> Vec<u8> {
        const PARTITION_OFFSET: usize = 0x4000;
        const PARTITION_SIZE: usize = 0x4000;
        const EXEFS_OFFSET: usize = 0x2000;
        const EXEFS_SIZE: usize = 0x2000;

        let mut image = vec![0; PARTITION_OFFSET + PARTITION_SIZE];
        image[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"NCSD");
        write_u32(&mut image, MEDIA_SIZE_OFFSET, 0x40);
        write_partition(&mut image, 0, 0x20, 0x20);

        let ncch_start = PARTITION_OFFSET;
        image[ncch_start + 0x100..ncch_start + 0x104].copy_from_slice(b"NCCH");
        write_u32(&mut image, ncch_start + 0x104, 0x20);
        write_u32(&mut image, ncch_start + 0x1A0, 0x10);
        write_u32(&mut image, ncch_start + 0x1A4, 0x10);
        write_u32(&mut image, ncch_start + 0x1A8, 1);

        let mut exefs = vec![0; EXEFS_SIZE];
        exefs[..5].copy_from_slice(b".code");
        write_u32(&mut exefs, 12, 0x10);

        let code: Vec<u8> = (0..0x10).map(|byte| byte as u8).collect();
        exefs[0x200..0x210].copy_from_slice(&code);
        let code_hash = sha256_digest(&code);
        exefs[0x1E0..0x200].copy_from_slice(&code_hash);

        let superblock_hash = sha256_digest(&exefs[..0x200]);
        image[ncch_start + 0x1C0..ncch_start + 0x1E0].copy_from_slice(&superblock_hash);

        let exefs_start = ncch_start + EXEFS_OFFSET;
        image[exefs_start..exefs_start + EXEFS_SIZE].copy_from_slice(&exefs);

        image
    }

    fn image_with_encrypted_verified_exefs_file(
        menu_info_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
        content_key: [u8; ncch::crypto::AES_BLOCK_SIZE],
    ) -> Vec<u8> {
        const PARTITION_OFFSET: usize = 0x4000;
        const EXEFS_OFFSET: usize = 0x2000;
        const EXEFS_SIZE: usize = 0x2000;

        let mut image = image_with_verified_exefs_file();
        let ncch_start = PARTITION_OFFSET;
        image[ncch_start + 0x112..ncch_start + 0x114].copy_from_slice(&2u16.to_le_bytes());

        let header =
            ncch::NcchHeader::parse(&image[ncch_start..ncch_start + ncch::HEADER_SIZE], 0x4000)
                .unwrap();
        let counter = header
            .initial_ctr_for_region(ncch::crypto::NcchCtrRegion::ExeFs)
            .unwrap();

        let exefs_start = ncch_start + EXEFS_OFFSET;
        let exefs_end = exefs_start + EXEFS_SIZE;
        let mut encrypted_exefs = image[exefs_start..exefs_end].to_vec();
        ncch::crypto::Aes128Ctr::new(menu_info_key, counter)
            .apply_keystream(&mut encrypted_exefs[..0x200])
            .unwrap();
        ncch::crypto::Aes128Ctr::apply_keystream_at_offset(
            content_key,
            counter,
            0x200,
            &mut encrypted_exefs[0x200..0x210],
        )
        .unwrap();
        image[exefs_start..exefs_end].copy_from_slice(&encrypted_exefs);

        image
    }

    #[test]
    fn loads_a_complete_bootstrap_arm11_address_space_from_ncsd() {
        let (image, expected_text, expected_read_only, expected_data) =
            image_with_arm11_code(false);
        let address_space = NcsdImageReader::open(Cursor::new(image))
            .unwrap()
            .load_primary_arm11_address_space([0x41; 16], [0x42; 16])
            .unwrap();

        assert_eq!(
            address_space.segment_bytes(process::Arm11SegmentKind::Text),
            expected_text
        );
        assert_eq!(
            address_space.segment_bytes(process::Arm11SegmentKind::ReadOnly),
            expected_read_only
        );
        assert_eq!(
            address_space.segment_bytes(process::Arm11SegmentKind::Data),
            expected_data
        );
        assert!(
            address_space
                .segment_bytes(process::Arm11SegmentKind::Bss)
                .iter()
                .all(|byte| *byte == 0)
        );
        assert!(
            address_space
                .segment_bytes(process::Arm11SegmentKind::Stack)
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(address_space.layout().entry_point, 0x0010_0000);
        assert_eq!(address_space.layout().initial_stack_pointer, 0x1000_0000);
        assert_eq!(address_space.layout().stack_size_bytes, 0x4000);
    }

    #[test]
    fn prepares_uncompressed_arm11_code_segments_from_ncsd() {
        let (image, expected_text, expected_read_only, expected_data) =
            image_with_arm11_code(false);
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();

        let prepared = partition
            .open_ncch(Cursor::new(image))
            .unwrap()
            .prepare_arm11_executable_image([0x41; 16], [0x42; 16])
            .unwrap();

        assert_eq!(prepared.text, expected_text);
        assert_eq!(prepared.read_only, expected_read_only);
        assert_eq!(prepared.data, expected_data);
        assert_eq!(prepared.extended_header.bss_size_bytes, 0x20);
        assert!(!prepared.extended_header.compresses_exefs_code());
    }

    #[test]
    fn prepares_blz_compressed_arm11_code_segments_from_ncsd() {
        let (image, expected_text, expected_read_only, expected_data) = image_with_arm11_code(true);
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();

        let prepared = partition
            .open_ncch(Cursor::new(image))
            .unwrap()
            .prepare_arm11_executable_image([0x41; 16], [0x42; 16])
            .unwrap();

        assert_eq!(prepared.text, expected_text);
        assert_eq!(prepared.read_only, expected_read_only);
        assert_eq!(prepared.data, expected_data);
        assert_eq!(prepared.extended_header.bss_size_bytes, 0x20);
        assert!(prepared.extended_header.compresses_exefs_code());
    }

    #[test]
    fn prepares_encrypted_arm11_code_with_distinct_menu_and_content_keys() {
        let (image, expected_text, expected_read_only, expected_data) =
            image_with_arm11_code(false);
        let menu_info_key = [0x61; ncch::crypto::AES_BLOCK_SIZE];
        let content_key = [0x62; ncch::crypto::AES_BLOCK_SIZE];
        let image = encrypt_arm11_code_image(image, menu_info_key, content_key);
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();

        let prepared = partition
            .open_ncch(Cursor::new(image))
            .unwrap()
            .prepare_arm11_executable_image(menu_info_key, content_key)
            .unwrap();

        assert_eq!(prepared.text, expected_text);
        assert_eq!(prepared.read_only, expected_read_only);
        assert_eq!(prepared.data, expected_data);
    }

    #[test]
    fn opens_decrypted_verified_exefs_file_through_the_full_ncsd_ncch_chain() {
        let menu_info_key = [0x42; ncch::crypto::AES_BLOCK_SIZE];
        let content_key = [0x43; ncch::crypto::AES_BLOCK_SIZE];
        let image = image_with_encrypted_verified_exefs_file(menu_info_key, content_key);
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();
        let expected: Vec<u8> = (0..0x10).map(|byte| byte as u8).collect();

        let mut reader = partition
            .open_ncch(Cursor::new(image))
            .unwrap()
            .open_decrypted_verified_exefs_file_reader(".code", menu_info_key, content_key)
            .unwrap()
            .unwrap();

        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();

        assert_eq!(reader.len(), 0x10);
        assert_eq!(actual, expected);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn opens_verified_exefs_file_through_the_full_ncsd_ncch_chain() {
        let image = image_with_verified_exefs_file();
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();
        let expected: Vec<u8> = (0..0x10).map(|byte| byte as u8).collect();

        let mut reader = partition
            .open_ncch(Cursor::new(image))
            .unwrap()
            .open_verified_exefs_file_reader(".code")
            .unwrap()
            .unwrap();

        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();

        assert_eq!(reader.len(), 0x10);
        assert_eq!(actual, expected);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn opens_ncsd_from_a_seekable_reader_and_exposes_partition_metadata() {
        let (image, _, _, _) = image_with_arm11_code(false);
        let reader = NcsdImageReader::open(Cursor::new(image)).unwrap();

        assert_eq!(reader.header().partitions.len(), 1);
        assert_eq!(reader.header().primary_partition().unwrap().index, 0);
    }

    #[test]
    fn opens_primary_ncch_directly_from_a_file_backed_ncsd_reader() {
        let (image, _, _, _) = image_with_arm11_code(false);
        let expected_partition = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64)
            .unwrap()
            .primary_partition()
            .copied()
            .unwrap();

        let opened = NcsdImageReader::open(Cursor::new(image))
            .unwrap()
            .open_primary_ncch()
            .unwrap();

        assert_eq!(opened.partition(), expected_partition);
        assert_eq!(
            opened.header().content_size_bytes,
            expected_partition.size_bytes
        );
    }

    #[test]
    fn reports_an_empty_partition_slot_without_panicking() {
        let (image, _, _, _) = image_with_arm11_code(false);
        let error = NcsdImageReader::open(Cursor::new(image))
            .unwrap()
            .open_partition_ncch(7)
            .err()
            .unwrap();

        assert!(matches!(
            error,
            NcsdImageReaderError::MissingPartition { index: 7 }
        ));
    }

    #[test]
    fn rejects_a_truncated_ncsd_reader_header() {
        let error = NcsdImageReader::open(Cursor::new(vec![0u8; HEADER_SIZE - 1])).unwrap_err();

        assert!(matches!(
            error,
            NcsdImageReaderError::InvalidHeader(Error::InvalidNcsdHeader("header is truncated"))
        ));
    }

    #[test]
    fn opens_primary_partition_as_a_bounded_ncch_container() {
        let image = image_with_primary_ncch();
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();

        let mut opened = partition.open_ncch(Cursor::new(image.clone())).unwrap();

        assert_eq!(opened.partition(), partition);
        assert_eq!(opened.header().content_size_bytes, partition.size_bytes);
        assert_eq!(opened.header().program_id, 0x0004_0000_1234_5678);
        assert!(opened.verify_exefs_integrity().unwrap().is_none());

        let opened_for_exefs = partition.open_ncch(Cursor::new(image.clone())).unwrap();
        assert!(
            opened_for_exefs
                .open_verified_exefs_file_reader(".code")
                .unwrap()
                .is_none()
        );

        let opened_for_romfs = partition.open_ncch(Cursor::new(image)).unwrap();
        assert!(
            opened_for_romfs
                .open_verified_romfs_level3_reader()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rejects_a_partition_without_a_valid_ncch_header() {
        let mut image = image_with_primary_ncch();
        image[0x4000 + 0x100..0x4000 + 0x104].copy_from_slice(b"NOPE");
        let parsed = NcsdHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let partition = *parsed.primary_partition().unwrap();

        let error = partition.open_ncch(Cursor::new(image)).unwrap_err();

        assert!(matches!(
            &error,
            NcsdNcchError::InvalidNcchHeader(ncch::NcchError::InvalidHeader(
                "NCCH magic is missing"
            ))
        ));
        assert!(std::error::Error::source(&error).is_some());
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

    #[test]
    fn partition_reader_never_reads_past_the_partition() {
        let image: Vec<u8> = (0..64).collect();
        let expected = image[8..20].to_vec();
        let partition = NcsdPartition {
            index: 0,
            offset_bytes: 8,
            size_bytes: 12,
            fs_type: 1,
            crypt_type: 0,
            id: 0,
        };
        let mut reader = partition.reader(Cursor::new(image)).unwrap();
        let mut actual = Vec::new();

        reader.read_to_end(&mut actual).unwrap();

        assert_eq!(actual, expected);
        assert_eq!(reader.position(), 12);
        assert_eq!(reader.stream_position().unwrap(), 12);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn partition_reader_seeks_relative_to_its_own_start() {
        let image: Vec<u8> = (0..64).collect();
        let partition = NcsdPartition {
            index: 3,
            offset_bytes: 8,
            size_bytes: 12,
            fs_type: 1,
            crypt_type: 0,
            id: 0,
        };
        let mut reader = partition.reader(Cursor::new(image)).unwrap();

        assert_eq!(reader.seek(SeekFrom::Start(10)).unwrap(), 10);
        let mut tail = [0; 2];
        assert_eq!(reader.read(&mut tail).unwrap(), 2);
        assert_eq!(tail, [18, 19]);

        assert_eq!(reader.seek(SeekFrom::End(-4)).unwrap(), 8);
        assert_eq!(reader.seek(SeekFrom::Current(2)).unwrap(), 10);
    }

    #[test]
    fn partition_reader_rejects_out_of_range_seeks() {
        let partition = NcsdPartition {
            index: 0,
            offset_bytes: 8,
            size_bytes: 12,
            fs_type: 1,
            crypt_type: 0,
            id: 0,
        };
        let mut reader = partition.reader(Cursor::new(vec![0; 64])).unwrap();

        assert_eq!(
            reader.seek(SeekFrom::Start(13)).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            reader.seek(SeekFrom::Current(-1)).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(
            reader.seek(SeekFrom::End(1)).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        assert_eq!(reader.seek(SeekFrom::Start(0)).unwrap(), 0);
    }

    #[test]
    fn partition_reader_rejects_empty_and_overflowing_ranges() {
        let empty = NcsdPartition {
            index: 0,
            offset_bytes: 0,
            size_bytes: 0,
            fs_type: 0,
            crypt_type: 0,
            id: 0,
        };
        assert_eq!(
            empty.reader(Cursor::new(vec![0; 8])).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );

        let overflowing = NcsdPartition {
            index: 0,
            offset_bytes: u64::MAX - 3,
            size_bytes: 8,
            fs_type: 0,
            crypt_type: 0,
            id: 0,
        };
        assert_eq!(
            overflowing
                .reader(Cursor::new(vec![0; 8]))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
