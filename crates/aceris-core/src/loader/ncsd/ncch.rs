//! Structural parser and cleartext integrity checks for Nintendo 3DS NCCH.
//!
//! This module parses the fixed 0x200-byte header, validates declared content
//! regions, and exposes ExeFS/RomFS metadata readers. RomFS integrity validation
//! checks the NCCH superblock hash and internal IVFC chain. It does not verify
//! RSA signatures or decrypt NCCH content.

use std::fmt;
use std::io::{Read, Seek, SeekFrom};

/// Original AES-128, CTR mode, and NCCH cryptographic parameter primitives.
pub mod crypto;
/// Structural ExeFS header parser for NCCH executable files.
pub mod exefs;
/// Structural RomFS IVFC and Level 3 header parser.
pub mod romfs;

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
const EXTENDED_HEADER_OFFSET: u64 = HEADER_SIZE as u64;
const EXTENDED_HEADER_HASH_SIZE: u64 = 0x400;
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

/// Errors reported while validating the cleartext NCCH extended header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NcchIntegrityError {
    ExtendedHeaderTooSmall {
        declared_size: u32,
        required_size: u64,
    },
    ExtendedHeaderHashMismatch,
    ReaderFailure(String),
}

impl fmt::Display for NcchIntegrityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExtendedHeaderTooSmall {
                declared_size,
                required_size,
            } => write!(
                formatter,
                "NCCH extended header is too small: declared {declared_size} bytes, need at least {required_size}"
            ),
            Self::ExtendedHeaderHashMismatch => {
                write!(formatter, "NCCH extended header SHA-256 hash mismatch")
            }
            Self::ReaderFailure(reason) => {
                write!(formatter, "NCCH extended header reader failure: {reason}")
            }
        }
    }
}

impl std::error::Error for NcchIntegrityError {}

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

    /// Verifies the SHA-256 hash of the cleartext extended-header data.
    ///
    /// The NCCH format stores the extended header at offset 0x200 and hashes
    /// its first 0x400 bytes. The reader must expose cleartext NCCH content
    /// from offset zero; this method does not decrypt the extended header or
    /// verify the NCCH RSA signature.
    ///
    /// Returns None when the NCCH header declares no extended header.
    pub fn verify_extended_header_integrity<R: Read + Seek>(
        &self,
        reader: &mut R,
    ) -> Result<Option<()>, NcchIntegrityError> {
        if self.extended_header_size == 0 {
            return Ok(None);
        }

        if u64::from(self.extended_header_size) < EXTENDED_HEADER_HASH_SIZE {
            return Err(NcchIntegrityError::ExtendedHeaderTooSmall {
                declared_size: self.extended_header_size,
                required_size: EXTENDED_HEADER_HASH_SIZE,
            });
        }

        verify_extended_header_hash(reader, &self.extended_header_hash)?;
        Ok(Some(()))
    }

    /// Opens the 0x400-byte extended-header hash region after its SHA-256 hash passes.
    ///
    /// The reader must expose cleartext NCCH content from offset zero. The
    /// returned reader is bounded to the bytes covered by the extended-header
    /// hash. This does not decrypt content or verify the NCCH RSA signature.
    ///
    /// Returns None when the NCCH header declares no extended header.
    pub fn open_verified_extended_header_reader<R: Read + Seek>(
        &self,
        mut reader: R,
    ) -> Result<Option<romfs::RomFsRangeReader<R>>, NcchIntegrityError> {
        if self
            .verify_extended_header_integrity(&mut reader)?
            .is_none()
        {
            return Ok(None);
        }

        romfs::RomFsRangeReader::new(reader, EXTENDED_HEADER_OFFSET, EXTENDED_HEADER_HASH_SIZE)
            .map(Some)
            .map_err(|error| NcchIntegrityError::ReaderFailure(error.to_string()))
    }

    /// Builds the documented initial CTR for an encrypted NCCH region.
    ///
    /// Version 1 counters use the selected region's absolute offset within
    /// the NCCH container; versions 0 and 2 use a region marker instead.
    pub fn initial_ctr_for_region(
        &self,
        region: crypto::NcchCtrRegion,
    ) -> Result<[u8; 16], crypto::NcchCryptoError> {
        let offset = match region {
            crypto::NcchCtrRegion::ExtendedHeader => {
                if self.extended_header_size == 0 {
                    return Err(crypto::NcchCryptoError::RegionNotPresent(region));
                }
                HEADER_SIZE as u64
            }
            crypto::NcchCtrRegion::ExeFs => {
                self.exefs
                    .ok_or(crypto::NcchCryptoError::RegionNotPresent(region))?
                    .offset_bytes
            }
            crypto::NcchCtrRegion::RomFs => {
                self.romfs
                    .ok_or(crypto::NcchCryptoError::RegionNotPresent(region))?
                    .offset_bytes
            }
        };

        crypto::initial_ctr(self.partition_id, self.version, region, offset)
    }

    /// Opens one encrypted NCCH region as a bounded, seekable plaintext reader.
    ///
    /// The caller must supply the AES-128 content key derived from lawful
    /// system material. No key material is stored or discovered by Aceris.
    /// For NoCrypto content, do not call this method; use a clear bounded
    /// region reader instead. Returns None when the selected region is absent.
    pub fn open_decrypted_region_reader<R: Read + Seek>(
        &self,
        reader: R,
        region: crypto::NcchCtrRegion,
        key: [u8; crypto::AES_BLOCK_SIZE],
    ) -> Result<Option<crypto::NcchCtrReader<R>>, crypto::NcchCtrReaderError> {
        if self.declares_no_crypto() {
            return Err(crypto::NcchCtrReaderError::ContentAlreadyClear);
        }

        let (offset, size) = match region {
            crypto::NcchCtrRegion::ExtendedHeader => {
                if self.extended_header_size == 0 {
                    return Ok(None);
                }
                (HEADER_SIZE as u64, u64::from(self.extended_header_size))
            }
            crypto::NcchCtrRegion::ExeFs => {
                let Some(region) = self.exefs else {
                    return Ok(None);
                };
                (region.offset_bytes, region.size_bytes)
            }
            crypto::NcchCtrRegion::RomFs => {
                let Some(region) = self.romfs else {
                    return Ok(None);
                };
                (region.offset_bytes, region.size_bytes)
            }
        };

        let initial_counter = self
            .initial_ctr_for_region(region)
            .map_err(crypto::NcchCtrReaderError::Crypto)?;
        crypto::NcchCtrReader::new(reader, offset, size, key, initial_counter).map(Some)
    }

    /// Returns the content keyslot selected by NCCH flag byte 3.
    ///
    /// This identifies the slot only; it does not provide console-specific
    /// KeyX material or decrypt the content.
    pub fn content_keyslot(&self) -> Result<u8, crypto::NcchCryptoError> {
        crypto::content_keyslot(self.flags[3])
    }

    /// Returns the two low-order content-form bits from NCCH flag byte 5.
    pub const fn content_form_type(&self) -> u8 {
        self.flags[5] & 0x03
    }

    /// Returns the content-type field from NCCH flag byte 5.
    pub const fn content_type(&self) -> u8 {
        self.flags[5] >> 2
    }

    /// Opens the RomFS Level 3 filesystem directly from the declared NCCH region.
    ///
    /// The NCCH reader must expose cleartext content from offset zero. This
    /// method bounds the RomFS region, reads IVFC metadata, then returns only
    /// the documented Level 3 byte range. It does not decrypt or verify hashes.
    pub fn open_romfs_level3_reader<R: Read + Seek>(
        &self,
        reader: R,
    ) -> Result<
        Option<romfs::RomFsLevel3Reader<romfs::RomFsRangeReader<romfs::RomFsRangeReader<R>>>>,
        romfs::RomFsError,
    > {
        let Some(region) = self.romfs else {
            return Ok(None);
        };

        let mut romfs_reader =
            romfs::RomFsRangeReader::new(reader, region.offset_bytes, region.size_bytes)
                .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
        let ivfc = romfs::IvfcHeader::parse_reader(&mut romfs_reader, region.size_bytes)?;
        ivfc.open_level3_reader(romfs_reader, region.size_bytes)
            .map(Some)
    }

    /// Verifies the internal IVFC SHA-256 chain inside the declared NCCH RomFS region.
    ///
    /// The reader must expose cleartext NCCH bytes from offset zero. This does
    /// not decrypt content or verify the separate NCCH RomFS superblock hash.
    ///
    /// Returns None when the NCCH header has no RomFS region.
    pub fn verify_romfs_ivfc_integrity<R: Read + Seek>(
        &self,
        reader: R,
    ) -> Result<Option<()>, romfs::RomFsError> {
        let Some(region) = self.romfs else {
            return Ok(None);
        };

        let mut romfs_reader =
            romfs::RomFsRangeReader::new(reader, region.offset_bytes, region.size_bytes)
                .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
        let ivfc = romfs::IvfcHeader::parse_reader(&mut romfs_reader, region.size_bytes)?;
        ivfc.verify_integrity(romfs_reader, region.size_bytes)?;
        Ok(Some(()))
    }

    /// Verifies the cleartext NCCH RomFS superblock hash and its internal IVFC chain.
    ///
    /// The reader must expose cleartext NCCH bytes from offset zero. The
    /// superblock hash covers the prefix whose length is declared in NCCH media
    /// units; IVFC verification then checks all three internal hash levels.
    /// This does not decrypt content or verify the NCCH RSA signature.
    ///
    /// Returns None when the NCCH header has no RomFS region.
    pub fn verify_romfs_integrity<R: Read + Seek>(
        &self,
        reader: R,
    ) -> Result<Option<()>, romfs::RomFsError> {
        let Some(region) = self.romfs else {
            return Ok(None);
        };

        let mut romfs_reader =
            romfs::RomFsRangeReader::new(reader, region.offset_bytes, region.size_bytes)
                .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
        verify_romfs_superblock_hash(
            &mut romfs_reader,
            &self.romfs_superblock_hash,
            region.hash_region_size_bytes,
        )?;

        let ivfc = romfs::IvfcHeader::parse_reader(&mut romfs_reader, region.size_bytes)?;
        ivfc.verify_integrity_reader(&mut romfs_reader, region.size_bytes)?;
        Ok(Some(()))
    }

    /// Opens the RomFS Level 3 filesystem only after all NCCH and IVFC hashes pass.
    ///
    /// The reader must expose cleartext NCCH bytes from offset zero. This checks
    /// the NCCH RomFS superblock hash first, then the complete internal IVFC
    /// chain, and only then returns the bounded Level 3 reader. It does not
    /// decrypt NCCH content or verify the NCCH RSA signature.
    ///
    /// Returns None when the NCCH header has no RomFS region.
    pub fn open_verified_romfs_level3_reader<R: Read + Seek>(
        &self,
        reader: R,
    ) -> Result<
        Option<romfs::RomFsLevel3Reader<romfs::RomFsRangeReader<romfs::RomFsRangeReader<R>>>>,
        romfs::RomFsError,
    > {
        let Some(region) = self.romfs else {
            return Ok(None);
        };

        let mut romfs_reader =
            romfs::RomFsRangeReader::new(reader, region.offset_bytes, region.size_bytes)
                .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
        verify_romfs_superblock_hash(
            &mut romfs_reader,
            &self.romfs_superblock_hash,
            region.hash_region_size_bytes,
        )?;

        let ivfc = romfs::IvfcHeader::parse_reader(&mut romfs_reader, region.size_bytes)?;
        ivfc.verify_integrity_reader(&mut romfs_reader, region.size_bytes)?;
        ivfc.open_level3_reader(romfs_reader, region.size_bytes)
            .map(Some)
    }

    /// Parses the RomFS IVFC header from the declared NCCH RomFS region.
    ///
    /// The reader must expose NCCH bytes from offset zero. This reads only the
    /// fixed IVFC header; it does not decrypt content, locate physical IVFC
    /// levels, or verify any hashes.
    ///
    /// Returns None when the NCCH header has no RomFS region.
    pub fn parse_romfs_ivfc_header<R: Read + Seek>(
        &self,
        reader: &mut R,
    ) -> Result<Option<romfs::IvfcHeader>, romfs::RomFsError> {
        let Some(region) = self.romfs else {
            return Ok(None);
        };

        let actual = reader
            .seek(SeekFrom::Start(region.offset_bytes))
            .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
        if actual != region.offset_bytes {
            return Err(romfs::RomFsError::ReaderFailure(
                "reader did not seek to the declared RomFS offset".to_owned(),
            ));
        }

        romfs::IvfcHeader::parse_reader(reader, region.size_bytes).map(Some)
    }

    /// Parses the ExeFS header from the NCCH container.
    ///
    /// The reader must expose the NCCH bytes from offset zero. Its ExeFS region
    /// must already be readable (cleartext); this method does not decrypt NCCH
    /// contents or verify the ExeFS superblock hash.
    ///
    /// Returns `Ok(None)` when the NCCH header does not declare an ExeFS region.
    pub fn parse_exefs_header<R: Read + Seek>(
        &self,
        reader: &mut R,
    ) -> Result<Option<exefs::ExeFsHeader>, exefs::ExeFsError> {
        let Some(region) = self.exefs else {
            return Ok(None);
        };

        let actual = reader
            .seek(SeekFrom::Start(region.offset_bytes))
            .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))?;
        if actual != region.offset_bytes {
            return Err(exefs::ExeFsError::ReaderFailure(
                "reader did not seek to the declared ExeFS offset".to_owned(),
            ));
        }

        exefs::ExeFsHeader::parse_reader(reader, region.size_bytes).map(Some)
    }

    /// Verifies the NCCH ExeFS superblock hash and every declared file hash.
    ///
    /// The reader must expose cleartext NCCH bytes from offset zero. The
    /// superblock hash covers the prefix declared by the NCCH ExeFS hash-region
    /// field. Each populated ExeFS entry is then checked against its own SHA-256
    /// digest. This does not decrypt NCCH content or verify its RSA signature.
    ///
    /// Returns None when the NCCH header has no ExeFS region.
    pub fn verify_exefs_integrity<R: Read + Seek>(
        &self,
        reader: &mut R,
    ) -> Result<Option<()>, exefs::ExeFsError> {
        let Some(region) = self.exefs else {
            return Ok(None);
        };

        verify_exefs_superblock_hash(reader, region, &self.exefs_superblock_hash)?;
        let header = self.parse_exefs_header(reader)?.ok_or_else(|| {
            exefs::ExeFsError::ReaderFailure(
                "NCCH ExeFS region disappeared from validated header".to_owned(),
            )
        })?;

        for file in &header.files {
            verify_exefs_file_hash(reader, region, file)?;
        }

        Ok(Some(()))
    }

    /// Opens one ExeFS file after validating its superblock and file digest.
    ///
    /// The reader must expose cleartext NCCH bytes from offset zero. The NCCH
    /// ExeFS superblock hash is checked before its file table is parsed; then
    /// only the requested file's SHA-256 digest is checked. The returned reader
    /// is restricted to that file's byte range. Other ExeFS files are not
    /// verified by this operation.
    ///
    /// Returns None when the NCCH header does not declare an ExeFS region.
    /// This does not decrypt NCCH content or verify its RSA signature.
    pub fn open_verified_exefs_file_reader<R: Read + Seek>(
        &self,
        reader: R,
        file_name: &str,
    ) -> Result<Option<exefs::ExeFsFileReader<romfs::RomFsRangeReader<R>>>, exefs::ExeFsError> {
        let Some(region) = self.exefs else {
            return Ok(None);
        };

        let mut reader = reader;
        verify_exefs_superblock_hash(&mut reader, region, &self.exefs_superblock_hash)?;

        let header = self.parse_exefs_header(&mut reader)?.ok_or_else(|| {
            exefs::ExeFsError::ReaderFailure(
                "NCCH ExeFS region disappeared from validated header".to_owned(),
            )
        })?;
        let file = header
            .file(file_name)
            .cloned()
            .ok_or_else(|| exefs::ExeFsError::FileNotFound(file_name.to_owned()))?;

        verify_exefs_file_hash(&mut reader, region, &file)?;

        let bounded_region =
            romfs::RomFsRangeReader::new(reader, region.offset_bytes, region.size_bytes)
                .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))?;
        file.reader(bounded_region)
            .map(Some)
            .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))
    }

    /// Returns true when the NCCH NoCrypto flag is set.
    ///
    /// This reports the header flag only; it does not decrypt or validate data.
    pub const fn declares_no_crypto(&self) -> bool {
        self.flags[7] & NO_CRYPTO_FLAG != 0
    }
}

fn verify_extended_header_hash<R: Read + Seek>(
    reader: &mut R,
    expected_hash: &[u8; 32],
) -> Result<(), NcchIntegrityError> {
    let original_position = reader
        .stream_position()
        .map_err(|error| NcchIntegrityError::ReaderFailure(error.to_string()))?;

    reader
        .seek(SeekFrom::Start(EXTENDED_HEADER_OFFSET))
        .map_err(|error| NcchIntegrityError::ReaderFailure(error.to_string()))?;

    let hash_result = (|| {
        let mut hasher = romfs::sha256::Sha256::new();
        let mut remaining = EXTENDED_HEADER_HASH_SIZE;
        let mut buffer = [0; 4096];

        while remaining > 0 {
            let amount = remaining.min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..amount])
                .map_err(|error| NcchIntegrityError::ReaderFailure(error.to_string()))?;
            hasher
                .update(&buffer[..amount])
                .map_err(|error| NcchIntegrityError::ReaderFailure(error.to_string()))?;
            remaining -= amount as u64;
        }

        Ok::<[u8; 32], NcchIntegrityError>(hasher.finalize())
    })();

    reader
        .seek(SeekFrom::Start(original_position))
        .map_err(|error| NcchIntegrityError::ReaderFailure(error.to_string()))?;

    let actual_hash = hash_result?;
    if &actual_hash != expected_hash {
        return Err(NcchIntegrityError::ExtendedHeaderHashMismatch);
    }

    Ok(())
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

fn verify_exefs_superblock_hash<R: Read + Seek>(
    reader: &mut R,
    region: NcchRegion,
    expected_hash: &[u8; 32],
) -> Result<(), exefs::ExeFsError> {
    if region.hash_region_size_bytes == 0 {
        return Err(exefs::ExeFsError::InvalidHashRegion);
    }

    let actual = reader
        .seek(SeekFrom::Start(region.offset_bytes))
        .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))?;
    if actual != region.offset_bytes {
        return Err(exefs::ExeFsError::ReaderFailure(
            "reader did not seek to the declared ExeFS offset".to_owned(),
        ));
    }

    let mut hasher = romfs::sha256::Sha256::new();
    let mut remaining = region.hash_region_size_bytes;
    let mut buffer = [0u8; 8 * 1024];

    while remaining != 0 {
        let amount = remaining.min(buffer.len() as u64) as usize;
        reader
            .read_exact(&mut buffer[..amount])
            .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))?;
        hasher.update(&buffer[..amount]).map_err(|_| {
            exefs::ExeFsError::ReaderFailure(
                "ExeFS superblock exceeds the SHA-256 message limit".to_owned(),
            )
        })?;
        remaining -= amount as u64;
    }

    if &hasher.finalize() != expected_hash {
        return Err(exefs::ExeFsError::SuperblockHashMismatch);
    }

    Ok(())
}

fn verify_exefs_file_hash<R: Read + Seek>(
    reader: &mut R,
    region: NcchRegion,
    file: &exefs::ExeFsFile,
) -> Result<(), exefs::ExeFsError> {
    let file_offset = region
        .offset_bytes
        .checked_add(file.offset_bytes)
        .ok_or_else(|| {
            exefs::ExeFsError::ReaderFailure("ExeFS file offset overflows".to_owned())
        })?;
    let actual = reader
        .seek(SeekFrom::Start(file_offset))
        .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))?;
    if actual != file_offset {
        return Err(exefs::ExeFsError::ReaderFailure(
            "reader did not seek to the declared ExeFS file offset".to_owned(),
        ));
    }

    let mut hasher = romfs::sha256::Sha256::new();
    let mut remaining = file.size_bytes;
    let mut buffer = [0u8; 8 * 1024];

    while remaining != 0 {
        let amount = remaining.min(buffer.len() as u64) as usize;
        reader
            .read_exact(&mut buffer[..amount])
            .map_err(|error| exefs::ExeFsError::ReaderFailure(error.to_string()))?;
        hasher.update(&buffer[..amount]).map_err(|_| {
            exefs::ExeFsError::ReaderFailure(
                "ExeFS file exceeds the SHA-256 message limit".to_owned(),
            )
        })?;
        remaining -= amount as u64;
    }

    if hasher.finalize() != file.sha256 {
        return Err(exefs::ExeFsError::FileHashMismatch(file.name.clone()));
    }

    Ok(())
}

fn verify_romfs_superblock_hash<R: Read + Seek>(
    reader: &mut R,
    expected_hash: &[u8; 32],
    hash_region_size_bytes: u64,
) -> Result<(), romfs::RomFsError> {
    if hash_region_size_bytes == 0 {
        return Err(romfs::RomFsError::InvalidIvfcHeader(
            "NCCH RomFS superblock hash region is empty",
        ));
    }

    let start = reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
    if start != 0 {
        return Err(romfs::RomFsError::ReaderFailure(
            "could not seek to the start of the bounded NCCH RomFS region".to_owned(),
        ));
    }

    let mut hasher = romfs::sha256::Sha256::new();
    let mut remaining = hash_region_size_bytes;
    let mut buffer = [0u8; 8 * 1024];

    while remaining != 0 {
        let amount = remaining.min(buffer.len() as u64) as usize;
        reader
            .read_exact(&mut buffer[..amount])
            .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
        hasher.update(&buffer[..amount]).map_err(|_| {
            romfs::RomFsError::InvalidIvfcHeader(
                "NCCH RomFS superblock exceeds the SHA-256 message limit",
            )
        })?;
        remaining -= amount as u64;
    }

    if &hasher.finalize() != expected_hash {
        return Err(romfs::RomFsError::NcchSuperblockHashMismatch);
    }

    let position = reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| romfs::RomFsError::ReaderFailure(error.to_string()))?;
    if position != 0 {
        return Err(romfs::RomFsError::ReaderFailure(
            "could not rewind the bounded NCCH RomFS reader".to_owned(),
        ));
    }

    Ok(())
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
    fn parsed_ncch_exposes_content_keyslot_and_region_counters() {
        let parsed = NcchHeader::parse(&valid_header(), 0x1_0000).unwrap();

        assert_eq!(parsed.content_keyslot(), Ok(0x2c));
        assert_eq!(
            parsed.initial_ctr_for_region(crypto::NcchCtrRegion::ExtendedHeader),
            Ok([
                0x31, 0x30, 0x53, 0x49, 0x52, 0x45, 0x43, 0x41, 0x01, 0, 0, 0, 0, 0, 0, 0,
            ])
        );
        assert_eq!(
            parsed.initial_ctr_for_region(crypto::NcchCtrRegion::ExeFs),
            Ok([
                0x31, 0x30, 0x53, 0x49, 0x52, 0x45, 0x43, 0x41, 0x02, 0, 0, 0, 0, 0, 0, 0,
            ])
        );
        assert_eq!(
            parsed.initial_ctr_for_region(crypto::NcchCtrRegion::RomFs),
            Ok([
                0x31, 0x30, 0x53, 0x49, 0x52, 0x45, 0x43, 0x41, 0x03, 0, 0, 0, 0, 0, 0, 0,
            ])
        );
    }

    #[test]
    fn ncch_header_opens_exefs_as_decrypted_bounded_region() {
        use std::io::Read;

        let mut header_bytes = valid_header();
        header_bytes[FLAGS_OFFSET + 7] &= !NO_CRYPTO_FLAG;
        let parsed = NcchHeader::parse(&header_bytes, 0x1_0000).unwrap();
        let region = parsed.exefs.unwrap();
        let key = [0x37; crypto::AES_BLOCK_SIZE];
        let initial_counter = parsed
            .initial_ctr_for_region(crypto::NcchCtrRegion::ExeFs)
            .unwrap();

        let plaintext: Vec<u8> = (0..region.size_bytes)
            .map(|index| (index.wrapping_mul(19) & 0xff) as u8)
            .collect();
        let mut ciphertext = plaintext.clone();
        crypto::Aes128Ctr::new(key, initial_counter)
            .apply_keystream(&mut ciphertext)
            .unwrap();

        let mut image = vec![0; 0x1_0000];
        image[..HEADER_SIZE].copy_from_slice(&header_bytes);
        let start = region.offset_bytes as usize;
        image[start..start + ciphertext.len()].copy_from_slice(&ciphertext);

        let mut reader = parsed
            .open_decrypted_region_reader(Cursor::new(image), crypto::NcchCtrRegion::ExeFs, key)
            .unwrap()
            .unwrap();
        assert_eq!(reader.len(), region.size_bytes);

        let mut decoded = Vec::new();
        reader.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, plaintext);
    }

    #[test]
    fn ncch_header_refuses_ctr_reader_for_no_crypto_content() {
        let parsed = NcchHeader::parse(&valid_header(), 0x1_0000).unwrap();
        assert!(matches!(
            parsed.open_decrypted_region_reader(
                Cursor::new(vec![0; 0x1_0000]),
                crypto::NcchCtrRegion::ExeFs,
                [0x37; crypto::AES_BLOCK_SIZE],
            ),
            Err(crypto::NcchCtrReaderError::ContentAlreadyClear)
        ));
    }

    #[test]
    fn version_one_header_counter_uses_absolute_exefs_offset() {
        let mut header = valid_header();
        write_u16(&mut header, VERSION_OFFSET, 1);
        let parsed = NcchHeader::parse(&header, 0x1_0000).unwrap();

        assert_eq!(
            parsed.initial_ctr_for_region(crypto::NcchCtrRegion::ExeFs),
            Ok([
                b'A', b'C', b'E', b'R', b'I', b'S', b'0', b'1', 0, 0, 0, 0, 0, 0x00, 0x20, 0x00,
            ])
        );
    }

    #[test]
    fn parsed_ncch_rejects_unknown_content_crypto_method() {
        let mut header = valid_header();
        header[FLAGS_OFFSET + 3] = 0x02;
        let parsed = NcchHeader::parse(&header, 0x1_0000).unwrap();

        assert_eq!(
            parsed.content_keyslot(),
            Err(crypto::NcchCryptoError::UnsupportedCryptoMethod(0x02))
        );
    }

    fn sha256_digest(data: &[u8]) -> [u8; 32] {
        let mut hasher = romfs::sha256::Sha256::new();
        hasher.update(data).unwrap();
        hasher.finalize()
    }

    fn valid_extended_header_image() -> (Vec<u8>, NcchHeader, Vec<u8>) {
        let mut image = vec![0; 0x1_0000];
        let header = valid_header();
        image[..HEADER_SIZE].copy_from_slice(&header);

        let extended_header: Vec<u8> = (0..EXTENDED_HEADER_HASH_SIZE)
            .map(|byte| (byte.wrapping_mul(17) & 0xFF) as u8)
            .collect();
        let start = EXTENDED_HEADER_OFFSET as usize;
        let end = start + extended_header.len();
        image[start..end].copy_from_slice(&extended_header);
        let digest = sha256_digest(&extended_header);
        image[EXTENDED_HEADER_HASH_OFFSET..EXTENDED_HEADER_HASH_OFFSET + 32]
            .copy_from_slice(&digest);

        let parsed = NcchHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        (image, parsed, extended_header)
    }

    fn valid_exefs_image() -> (Vec<u8>, NcchHeader) {
        let mut ncch_header = valid_header();
        write_u32(&mut ncch_header, EXEFS_HASH_OFFSET, 1);

        let exefs_start = 0x2000;
        let mut exefs_region = vec![0; 0x400];
        exefs_region[..5].copy_from_slice(b".code");
        write_u32(&mut exefs_region, 12, 0x100);

        let code: Vec<u8> = (0..0x100).map(|byte| byte as u8).collect();
        exefs_region[0x200..0x300].copy_from_slice(&code);
        let code_hash = sha256_digest(&code);
        exefs_region[0x1E0..0x200].copy_from_slice(&code_hash);
        let superblock_hash = sha256_digest(&exefs_region[..0x200]);

        let mut image = vec![0; 0x1_0000];
        image[..HEADER_SIZE].copy_from_slice(&ncch_header);
        image[ROMFS_SUPERBLOCK_HASH_OFFSET..ROMFS_SUPERBLOCK_HASH_OFFSET + 32].fill(0);
        image[EXEFS_SUPERBLOCK_HASH_OFFSET..EXEFS_SUPERBLOCK_HASH_OFFSET + 32]
            .copy_from_slice(&superblock_hash);
        image[exefs_start..exefs_start + exefs_region.len()].copy_from_slice(&exefs_region);

        let parsed = NcchHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        (image, parsed)
    }

    #[test]
    fn verifies_and_opens_the_bounded_extended_header() {
        let (image, parsed, expected) = valid_extended_header_image();
        let mut source = Cursor::new(image.clone());

        assert_eq!(
            parsed
                .verify_extended_header_integrity(&mut source)
                .unwrap(),
            Some(())
        );
        assert_eq!(source.position(), 0);

        let mut reader = parsed
            .open_verified_extended_header_reader(Cursor::new(image))
            .unwrap()
            .unwrap();
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();

        assert_eq!(actual, expected);
        assert_eq!(actual.len(), EXTENDED_HEADER_HASH_SIZE as usize);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn rejects_an_extended_header_with_a_corrupt_hash_region() {
        let (mut image, parsed, _) = valid_extended_header_image();
        image[EXTENDED_HEADER_OFFSET as usize + 0x20] ^= 0x80;

        assert_eq!(
            parsed.verify_extended_header_integrity(&mut Cursor::new(image)),
            Err(NcchIntegrityError::ExtendedHeaderHashMismatch)
        );
    }

    #[test]
    fn rejects_a_declared_extended_header_smaller_than_the_hashed_region() {
        let mut header = valid_header();
        write_u32(&mut header, EXTENDED_HEADER_SIZE_OFFSET, 0x200);
        let parsed = NcchHeader::parse(&header, 0x1_0000).unwrap();

        assert_eq!(
            parsed.verify_extended_header_integrity(&mut Cursor::new(vec![0; 0x1_0000])),
            Err(NcchIntegrityError::ExtendedHeaderTooSmall {
                declared_size: 0x200,
                required_size: EXTENDED_HEADER_HASH_SIZE,
            })
        );
    }

    #[test]
    fn reports_no_extended_header_when_the_declared_size_is_zero() {
        let mut header = valid_header();
        write_u32(&mut header, EXTENDED_HEADER_SIZE_OFFSET, 0);
        let parsed = NcchHeader::parse(&header, 0x1_0000).unwrap();

        assert_eq!(
            parsed.verify_extended_header_integrity(&mut Cursor::new(vec![0; 0x1_0000])),
            Ok(None)
        );
        assert!(
            parsed
                .open_verified_extended_header_reader(Cursor::new(vec![0; 0x1_0000]))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn opens_a_verified_exefs_file_as_a_bounded_reader() {
        let (image, parsed) = valid_exefs_image();
        let expected: Vec<u8> = (0..0x100).map(|byte| byte as u8).collect();

        let mut reader = parsed
            .open_verified_exefs_file_reader(Cursor::new(image), ".code")
            .unwrap()
            .unwrap();

        assert_eq!(reader.len(), 0x100);
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn rejects_opening_an_exefs_file_with_a_corrupt_superblock() {
        let (mut image, parsed) = valid_exefs_image();
        image[0x2000 + 0x10] ^= 0x80;

        assert_eq!(
            parsed
                .open_verified_exefs_file_reader(Cursor::new(image), ".code")
                .err(),
            Some(exefs::ExeFsError::SuperblockHashMismatch)
        );
    }

    #[test]
    fn rejects_opening_an_exefs_file_with_corrupt_contents() {
        let (mut image, parsed) = valid_exefs_image();
        image[0x2000 + 0x200 + 0x10] ^= 0x80;

        assert_eq!(
            parsed
                .open_verified_exefs_file_reader(Cursor::new(image), ".code")
                .err(),
            Some(exefs::ExeFsError::FileHashMismatch(".code".to_owned()))
        );
    }

    #[test]
    fn reports_a_missing_exefs_file_by_name() {
        let (image, parsed) = valid_exefs_image();

        assert_eq!(
            parsed
                .open_verified_exefs_file_reader(Cursor::new(image), "missing")
                .err(),
            Some(exefs::ExeFsError::FileNotFound("missing".to_owned()))
        );
    }

    #[test]
    fn verifies_exefs_superblock_and_every_file_hash() {
        let (image, parsed) = valid_exefs_image();
        let mut reader = Cursor::new(image);

        assert_eq!(parsed.verify_exefs_integrity(&mut reader), Ok(Some(())));
    }

    #[test]
    fn rejects_exefs_superblock_corruption_before_parsing_its_header() {
        let (mut image, parsed) = valid_exefs_image();
        image[0x2000 + 0x10] ^= 0x80;
        let mut reader = Cursor::new(image);

        assert_eq!(
            parsed.verify_exefs_integrity(&mut reader),
            Err(exefs::ExeFsError::SuperblockHashMismatch)
        );
    }

    #[test]
    fn rejects_exefs_file_content_with_a_mismatched_digest() {
        let (mut image, parsed) = valid_exefs_image();
        image[0x2000 + 0x200 + 0x10] ^= 0x80;
        let mut reader = Cursor::new(image);

        assert_eq!(
            parsed.verify_exefs_integrity(&mut reader),
            Err(exefs::ExeFsError::FileHashMismatch(".code".to_owned()))
        );
    }

    #[test]
    fn reports_an_empty_exefs_hash_region() {
        let (mut image, _) = valid_exefs_image();
        write_u32(&mut image, EXEFS_HASH_OFFSET, 0);
        let parsed = NcchHeader::parse(&image[..HEADER_SIZE], image.len() as u64).unwrap();
        let mut reader = Cursor::new(image);

        assert_eq!(
            parsed.verify_exefs_integrity(&mut reader),
            Err(exefs::ExeFsError::InvalidHashRegion)
        );
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
    fn parses_exefs_header_at_the_declared_ncch_offset() {
        let ncch_data = valid_header();
        let parsed = NcchHeader::parse(&ncch_data, 0x1_0000).unwrap();

        let mut container = vec![0; 0x1_0000];
        let exefs_start = 0x2000;
        let mut exefs_header = vec![0; exefs::HEADER_SIZE];
        exefs_header[..5].copy_from_slice(b".code");
        exefs_header[12..16].copy_from_slice(&0x100u32.to_le_bytes());
        exefs_header[0x1E0..0x200].fill(0x5A);
        container[exefs_start..exefs_start + exefs::HEADER_SIZE].copy_from_slice(&exefs_header);

        let mut reader = Cursor::new(container);
        let exefs = parsed.parse_exefs_header(&mut reader).unwrap().unwrap();

        assert_eq!(reader.position(), (exefs_start + exefs::HEADER_SIZE) as u64);
        assert_eq!(exefs.files.len(), 1);
        assert_eq!(exefs.file(".code").unwrap().size_bytes, 0x100);
        assert_eq!(exefs.file(".code").unwrap().sha256, [0x5A; 32]);
    }

    #[test]
    fn returns_none_when_ncch_has_no_exefs_region() {
        let mut data = valid_header();
        write_u32(&mut data, EXEFS_REGION_OFFSET, 0);
        write_u32(&mut data, EXEFS_REGION_OFFSET + 4, 0);
        write_u32(&mut data, EXEFS_HASH_OFFSET, 0);
        let parsed = NcchHeader::parse(&data, 0x1_0000).unwrap();

        let mut reader = Cursor::new(Vec::new());
        assert_eq!(parsed.parse_exefs_header(&mut reader).unwrap(), None);
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn reads_header_from_a_bounded_partition_reader() {
        let data = valid_header();
        let mut container = vec![0xAA; 0x1000];
        container.extend_from_slice(&data);
        container.resize(0x1000 + 0x1_0000, 0xBB);

        let mut partition = crate::loader::ncsd::NcsdPartition {
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
