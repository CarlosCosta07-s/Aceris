//! Versioned save-state container format.
//!
//! This module validates the on-disk envelope only. It does not capture or
//! restore the complete System. Its payload must come from a future, complete
//! machine snapshot before any frontend exposes save/load actions.

use std::fmt;

/// File signature for Aceris save-state containers.
const MAGIC: &[u8; 8] = b"ACRSTATE";
/// Version 1 stores a fixed header followed by an opaque machine-state payload.
const FORMAT_VERSION: u16 = 1;
/// Header fields are encoded in little-endian order.
pub const SAVE_STATE_HEADER_SIZE: usize = 88;
/// Initial defensive limit, applied before allocating decoded payload memory.
pub const MAX_SAVE_STATE_PAYLOAD_BYTES: usize = 512 * 1024 * 1024;

/// Identifies the exact boot image associated with a save state.
///
/// The content digest is mandatory so homebrew formats without a validated
/// Title ID can still be identified. A Title ID is additional metadata only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveStateIdentity {
    pub title_id: Option<u64>,
    /// SHA-256 of the exact complete image bytes, never a path or display name.
    pub content_sha256: [u8; 32],
}

impl SaveStateIdentity {
    fn is_valid(self) -> bool {
        self.title_id != Some(0) && self.content_sha256 != [0; 32]
    }
}

/// Frame-boundary metadata stored outside the opaque machine-state payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveStateMetadata {
    pub identity: SaveStateIdentity,
    pub frame_number: u64,
    pub virtual_time_ns: u64,
}

/// Validated save-state envelope and its opaque payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateFile {
    metadata: SaveStateMetadata,
    payload: Vec<u8>,
}

impl SaveStateFile {
    /// Creates an envelope after validating identity and payload bounds.
    pub fn new(metadata: SaveStateMetadata, payload: Vec<u8>) -> Result<Self, SaveStateError> {
        validate_identity(metadata.identity)?;
        validate_payload_size(payload.len() as u64)?;
        Ok(Self { metadata, payload })
    }

    pub fn metadata(&self) -> SaveStateMetadata {
        self.metadata
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Builds the outer save-state file from a validated section table.
    pub fn from_sections(
        metadata: SaveStateMetadata,
        sections: &SaveStatePayload,
    ) -> Result<Self, SaveStateError> {
        Self::new(metadata, sections.encode()?)
    }

    /// Decodes the section table stored in this file's payload.
    pub fn decode_sections(&self) -> Result<SaveStatePayload, SaveStateError> {
        SaveStatePayload::decode(&self.payload)
    }

    /// Encodes the versioned little-endian header and a CRC32-protected payload.
    pub fn encode(&self) -> Result<Vec<u8>, SaveStateError> {
        validate_identity(self.metadata.identity)?;
        validate_payload_size(self.payload.len() as u64)?;

        let capacity = SAVE_STATE_HEADER_SIZE
            .checked_add(self.payload.len())
            .ok_or(SaveStateError::InvalidLength)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| SaveStateError::AllocationFailed)?;

        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(SAVE_STATE_HEADER_SIZE as u16).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved flags
        bytes.extend_from_slice(&self.metadata.identity.title_id.unwrap_or(0).to_le_bytes());
        bytes.extend_from_slice(&self.metadata.identity.content_sha256);
        bytes.extend_from_slice(&self.metadata.frame_number.to_le_bytes());
        bytes.extend_from_slice(&self.metadata.virtual_time_ns.to_le_bytes());
        bytes.extend_from_slice(&(self.payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&crc32(&self.payload).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // reserved header word
        bytes.extend_from_slice(&self.payload);
        debug_assert_eq!(bytes.len(), capacity);
        Ok(bytes)
    }

    /// Decodes a file only when its format, integrity and exact image identity match.
    ///
    /// The complete header and length are validated before allocating the payload.
    pub fn decode(
        encoded: &[u8],
        expected_identity: SaveStateIdentity,
    ) -> Result<Self, SaveStateError> {
        if encoded.len() < SAVE_STATE_HEADER_SIZE {
            return Err(SaveStateError::TruncatedHeader);
        }
        if &encoded[..8] != MAGIC {
            return Err(SaveStateError::InvalidMagic);
        }

        let version = read_u16(encoded, 8);
        if version != FORMAT_VERSION {
            return Err(SaveStateError::UnsupportedVersion(version));
        }
        let header_size = read_u16(encoded, 10);
        if usize::from(header_size) != SAVE_STATE_HEADER_SIZE {
            return Err(SaveStateError::InvalidHeaderLength(header_size));
        }
        let flags = read_u32(encoded, 12);
        if flags != 0 {
            return Err(SaveStateError::UnsupportedFlags(flags));
        }

        let encoded_title_id = read_u64(encoded, 16);
        let identity = SaveStateIdentity {
            title_id: (encoded_title_id != 0).then_some(encoded_title_id),
            content_sha256: read_array_32(encoded, 24),
        };
        validate_identity(identity)?;
        validate_identity(expected_identity)?;
        if identity != expected_identity {
            return Err(SaveStateError::GameMismatch);
        }

        let frame_number = read_u64(encoded, 56);
        let virtual_time_ns = read_u64(encoded, 64);
        let payload_len = read_u64(encoded, 72);
        validate_payload_size(payload_len)?;
        let payload_len = usize::try_from(payload_len)
            .map_err(|_| SaveStateError::PayloadTooLarge(payload_len))?;
        let expected_len = SAVE_STATE_HEADER_SIZE
            .checked_add(payload_len)
            .ok_or(SaveStateError::InvalidLength)?;
        if encoded.len() != expected_len {
            return Err(SaveStateError::InvalidLength);
        }

        let expected_crc = read_u32(encoded, 80);
        let reserved = read_u32(encoded, 84);
        if reserved != 0 {
            return Err(SaveStateError::UnsupportedFlags(reserved));
        }
        let payload_bytes = &encoded[SAVE_STATE_HEADER_SIZE..];
        if crc32(payload_bytes) != expected_crc {
            return Err(SaveStateError::ChecksumMismatch);
        }

        let mut payload = Vec::new();
        payload
            .try_reserve_exact(payload_len)
            .map_err(|_| SaveStateError::AllocationFailed)?;
        payload.extend_from_slice(payload_bytes);

        Ok(Self {
            metadata: SaveStateMetadata {
                identity,
                frame_number,
                virtual_time_ns,
            },
            payload,
        })
    }
}

/// Stable IDs reserved for the first machine-state components.
pub mod section_id {
    /// Frame counter, virtual clock and versioned machine metadata.
    pub const SYSTEM_METADATA: u32 = 1;
    /// ARM11 CPU registers, execution mode and internal pipeline state.
    pub const ARM11_CPU: u32 = 2;
    /// Guest process address spaces and physical memory contents.
    pub const PROCESS_MEMORY: u32 = 3;
    /// Kernel threads, handles, events, mutexes and scheduler state.
    pub const KERNEL: u32 = 4;
    /// PICA200 registers, command state and guest rendering state.
    pub const PICA200: u32 = 5;
    /// GSP queues, LCD scanout and display timing state.
    pub const GSP_LCD: u32 = 6;
    /// DSP RAM, pipes, voices, semaphores and pending samples.
    pub const DSP: u32 = 7;
    /// CSND channels, capture state and pending sample output.
    pub const CSND: u32 = 8;
    /// HID, input and APT state visible to the guest.
    pub const INPUT_APT: u32 = 9;
    /// Guest filesystem sessions and pending filesystem operations.
    pub const FILESYSTEM: u32 = 10;
    /// Sparse VRAM/FCRAM pages, linear reservations and physical exclusive monitors.
    pub const GPU_PHYSICAL_MEMORY: u32 = 11;
    /// Deduplicated roots referenced by process mappings, kernel handles and HLE services.
    pub const SHARED_BACKINGS: u32 = 12;
    /// ARM11 process-local pending waits, address arbiters and interrupt bindings.
    pub const ARM11_PROCESS_RUNTIME: u32 = 13;
}

/// Signature for the section table inside a save-state payload.
const SECTIONED_PAYLOAD_MAGIC: &[u8; 8] = b"ACRPAYLD";
/// Payload table header: magic, section count and reserved flags.
pub const SAVE_STATE_PAYLOAD_HEADER_SIZE: usize = 16;
/// Each section header stores ID, version, flags, length, CRC-32 and reserved data.
pub const SAVE_STATE_SECTION_HEADER_SIZE: usize = 24;
/// Maximum independently versioned machine components in one state.
pub const MAX_SAVE_STATE_SECTIONS: usize = 32;

/// One independently versioned component of a machine snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveStateSection {
    id: u32,
    version: u16,
    payload: Vec<u8>,
}

impl SaveStateSection {
    /// Creates a section. IDs and versions are non-zero and payloads are bounded.
    pub fn new(id: u32, version: u16, payload: Vec<u8>) -> Result<Self, SaveStateError> {
        validate_section_fields(id, version, payload.len() as u64)?;
        Ok(Self {
            id,
            version,
            payload,
        })
    }

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn version(&self) -> u16 {
        self.version
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// Deterministic section table for a machine snapshot.
///
/// The outer SaveStateFile protects the entire payload. This inner table
/// additionally protects each subsystem independently and permits serializers
/// to evolve on their own version numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SaveStatePayload {
    sections: Vec<SaveStateSection>,
}

impl SaveStatePayload {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sections(&self) -> &[SaveStateSection] {
        &self.sections
    }

    pub fn section(&self, id: u32) -> Option<&SaveStateSection> {
        self.sections.iter().find(|section| section.id == id)
    }

    /// Inserts a section while rejecting duplicate IDs and aggregate overflow.
    pub fn insert(&mut self, section: SaveStateSection) -> Result<(), SaveStateError> {
        validate_section_fields(section.id, section.version, section.payload.len() as u64)?;
        if self
            .sections
            .iter()
            .any(|existing| existing.id == section.id)
        {
            return Err(SaveStateError::DuplicateSectionId(section.id));
        }
        if self.sections.len() >= MAX_SAVE_STATE_SECTIONS {
            return Err(SaveStateError::TooManySections(
                (self.sections.len() + 1) as u32,
            ));
        }
        let added_size = SAVE_STATE_SECTION_HEADER_SIZE
            .checked_add(section.payload.len())
            .ok_or(SaveStateError::InvalidLength)?;
        let total_size = self
            .encoded_len()?
            .checked_add(added_size)
            .ok_or(SaveStateError::InvalidLength)?;
        validate_payload_size(total_size as u64)?;
        self.sections
            .try_reserve(1)
            .map_err(|_| SaveStateError::AllocationFailed)?;
        self.sections.push(section);
        Ok(())
    }

    /// Encodes sections in insertion order for reproducible save-state files.
    pub fn encode(&self) -> Result<Vec<u8>, SaveStateError> {
        let total_size = self.encoded_len()?;
        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(total_size)
            .map_err(|_| SaveStateError::AllocationFailed)?;
        encoded.extend_from_slice(SECTIONED_PAYLOAD_MAGIC);
        encoded.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&0u32.to_le_bytes());

        for section in &self.sections {
            encoded.extend_from_slice(&section.id.to_le_bytes());
            encoded.extend_from_slice(&section.version.to_le_bytes());
            encoded.extend_from_slice(&0u16.to_le_bytes());
            encoded.extend_from_slice(&(section.payload.len() as u64).to_le_bytes());
            encoded.extend_from_slice(&crc32(&section.payload).to_le_bytes());
            encoded.extend_from_slice(&0u32.to_le_bytes());
            encoded.extend_from_slice(&section.payload);
        }
        debug_assert_eq!(encoded.len(), total_size);
        Ok(encoded)
    }

    /// Validates and decodes the entire section table before returning any section.
    pub fn decode(encoded: &[u8]) -> Result<Self, SaveStateError> {
        validate_payload_size(encoded.len() as u64)?;
        if encoded.len() < SAVE_STATE_PAYLOAD_HEADER_SIZE {
            return Err(SaveStateError::TruncatedPayloadHeader);
        }
        if &encoded[..8] != SECTIONED_PAYLOAD_MAGIC {
            return Err(SaveStateError::InvalidPayloadMagic);
        }
        let section_count = read_u32(encoded, 8);
        if section_count as usize > MAX_SAVE_STATE_SECTIONS {
            return Err(SaveStateError::TooManySections(section_count));
        }
        let flags = read_u32(encoded, 12);
        if flags != 0 {
            return Err(SaveStateError::UnsupportedFlags(flags));
        }

        let mut sections = Vec::new();
        sections
            .try_reserve_exact(section_count as usize)
            .map_err(|_| SaveStateError::AllocationFailed)?;
        let mut cursor = SAVE_STATE_PAYLOAD_HEADER_SIZE;

        for _ in 0..section_count {
            let header_end = cursor
                .checked_add(SAVE_STATE_SECTION_HEADER_SIZE)
                .ok_or(SaveStateError::InvalidLength)?;
            if encoded.get(cursor..header_end).is_none() {
                return Err(SaveStateError::TruncatedSectionHeader);
            }

            let id = read_u32(encoded, cursor);
            let version = read_u16(encoded, cursor + 4);
            let section_flags = read_u16(encoded, cursor + 6);
            let section_length = read_u64(encoded, cursor + 8);
            let expected_crc = read_u32(encoded, cursor + 16);
            let reserved = read_u32(encoded, cursor + 20);
            validate_section_fields(id, version, section_length)?;
            if section_flags != 0 {
                return Err(SaveStateError::UnsupportedSectionFlags(section_flags));
            }
            if reserved != 0 {
                return Err(SaveStateError::UnsupportedFlags(reserved));
            }
            if sections
                .iter()
                .any(|existing: &SaveStateSection| existing.id == id)
            {
                return Err(SaveStateError::DuplicateSectionId(id));
            }

            let payload_length = usize::try_from(section_length)
                .map_err(|_| SaveStateError::SectionTooLarge(section_length))?;
            let payload_end = header_end
                .checked_add(payload_length)
                .ok_or(SaveStateError::InvalidLength)?;
            let payload_bytes = encoded
                .get(header_end..payload_end)
                .ok_or(SaveStateError::InvalidLength)?;
            if crc32(payload_bytes) != expected_crc {
                return Err(SaveStateError::SectionChecksumMismatch(id));
            }

            let mut payload = Vec::new();
            payload
                .try_reserve_exact(payload_length)
                .map_err(|_| SaveStateError::AllocationFailed)?;
            payload.extend_from_slice(payload_bytes);
            sections.push(SaveStateSection {
                id,
                version,
                payload,
            });
            cursor = payload_end;
        }

        if cursor != encoded.len() {
            return Err(SaveStateError::InvalidLength);
        }
        Ok(Self { sections })
    }

    fn encoded_len(&self) -> Result<usize, SaveStateError> {
        if self.sections.len() > MAX_SAVE_STATE_SECTIONS {
            return Err(SaveStateError::TooManySections(self.sections.len() as u32));
        }
        let mut size = SAVE_STATE_PAYLOAD_HEADER_SIZE;
        for section in &self.sections {
            validate_section_fields(section.id, section.version, section.payload.len() as u64)?;
            size = size
                .checked_add(SAVE_STATE_SECTION_HEADER_SIZE)
                .and_then(|size| size.checked_add(section.payload.len()))
                .ok_or(SaveStateError::InvalidLength)?;
            validate_payload_size(size as u64)?;
        }
        Ok(size)
    }
}

fn validate_section_fields(id: u32, version: u16, payload_size: u64) -> Result<(), SaveStateError> {
    if id == 0 {
        return Err(SaveStateError::InvalidSectionId);
    }
    if version == 0 {
        return Err(SaveStateError::InvalidSectionVersion);
    }
    if payload_size > MAX_SAVE_STATE_PAYLOAD_BYTES as u64 {
        return Err(SaveStateError::SectionTooLarge(payload_size));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStateError {
    TruncatedHeader,
    InvalidMagic,
    TruncatedPayloadHeader,
    InvalidPayloadMagic,
    TruncatedSectionHeader,
    InvalidSectionId,
    InvalidSectionVersion,
    TooManySections(u32),
    DuplicateSectionId(u32),
    SectionTooLarge(u64),
    UnsupportedSectionFlags(u16),
    SectionChecksumMismatch(u32),
    UnsupportedVersion(u16),
    InvalidHeaderLength(u16),
    UnsupportedFlags(u32),
    InvalidIdentity,
    GameMismatch,
    PayloadTooLarge(u64),
    InvalidLength,
    ChecksumMismatch,
    AllocationFailed,
}

impl fmt::Display for SaveStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader => f.write_str("save-state header is truncated"),
            Self::InvalidMagic => f.write_str("save-state file signature is invalid"),
            Self::TruncatedPayloadHeader => f.write_str("save-state payload header is truncated"),
            Self::InvalidPayloadMagic => f.write_str("save-state payload signature is invalid"),
            Self::TruncatedSectionHeader => f.write_str("save-state section header is truncated"),
            Self::InvalidSectionId => f.write_str("save-state section ID must be non-zero"),
            Self::InvalidSectionVersion => {
                f.write_str("save-state section version must be non-zero")
            }
            Self::TooManySections(count) => write!(
                f,
                "save-state contains {count} sections; maximum is {MAX_SAVE_STATE_SECTIONS}"
            ),
            Self::DuplicateSectionId(id) => write!(f, "save-state section ID {id} is duplicated"),
            Self::SectionTooLarge(size) => {
                write!(f, "save-state section size {size} exceeds the limit")
            }
            Self::UnsupportedSectionFlags(flags) => {
                write!(
                    f,
                    "save-state section contains unsupported flags {flags:#06X}"
                )
            }
            Self::SectionChecksumMismatch(id) => {
                write!(f, "save-state section {id} checksum does not match")
            }
            Self::UnsupportedVersion(version) => {
                write!(f, "save-state version {version} is not supported")
            }
            Self::InvalidHeaderLength(length) => {
                write!(f, "save-state header length {length} is invalid")
            }
            Self::UnsupportedFlags(flags) => {
                write!(f, "save-state contains unsupported flags {flags:#010X}")
            }
            Self::InvalidIdentity => f.write_str(
                "save-state identity needs a non-zero SHA-256 and a valid optional Title ID",
            ),
            Self::GameMismatch => f.write_str("save-state belongs to a different game image"),
            Self::PayloadTooLarge(size) => {
                write!(
                    f,
                    "save-state payload of {size} bytes exceeds the supported limit"
                )
            }
            Self::InvalidLength => f.write_str("save-state file length does not match its header"),
            Self::ChecksumMismatch => f.write_str("save-state payload checksum does not match"),
            Self::AllocationFailed => f.write_str("could not allocate save-state payload"),
        }
    }
}

impl std::error::Error for SaveStateError {}

fn validate_identity(identity: SaveStateIdentity) -> Result<(), SaveStateError> {
    if identity.is_valid() {
        Ok(())
    } else {
        Err(SaveStateError::InvalidIdentity)
    }
}

fn validate_payload_size(size: u64) -> Result<(), SaveStateError> {
    if size > MAX_SAVE_STATE_PAYLOAD_BYTES as u64 {
        Err(SaveStateError::PayloadTooLarge(size))
    } else {
        Ok(())
    }
}

fn read_array_32(bytes: &[u8], offset: usize) -> [u8; 32] {
    bytes[offset..offset + 32]
        .try_into()
        .expect("validated save-state header range")
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        bytes[offset..offset + 2]
            .try_into()
            .expect("validated header range"),
    )
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated header range"),
    )
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("validated header range"),
    )
}

/// IEEE CRC-32. Used for accidental-corruption detection, not authentication.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_SAVE_STATE_PAYLOAD_BYTES, MAX_SAVE_STATE_SECTIONS, SAVE_STATE_HEADER_SIZE,
        SAVE_STATE_PAYLOAD_HEADER_SIZE, SAVE_STATE_SECTION_HEADER_SIZE, SaveStateError,
        SaveStateFile, SaveStateIdentity, SaveStateMetadata, SaveStatePayload, SaveStateSection,
        crc32,
    };

    fn identity() -> SaveStateIdentity {
        SaveStateIdentity {
            title_id: Some(0x0004_0000_1234_5600),
            content_sha256: [0xA5; 32],
        }
    }

    fn sample_file(identity: SaveStateIdentity) -> SaveStateFile {
        SaveStateFile::new(
            SaveStateMetadata {
                identity,
                frame_number: 1234,
                virtual_time_ns: 20_555_555_000,
            },
            b"synthetic machine state payload".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn outer_file_round_trips_the_inner_section_table() {
        let mut sections = SaveStatePayload::new();
        sections
            .insert(
                SaveStateSection::new(
                    super::section_id::ARM11_CPU,
                    1,
                    b"synthetic-cpu-state".to_vec(),
                )
                .unwrap(),
            )
            .unwrap();
        sections
            .insert(
                SaveStateSection::new(
                    super::section_id::PROCESS_MEMORY,
                    1,
                    b"synthetic-memory-state".to_vec(),
                )
                .unwrap(),
            )
            .unwrap();
        let metadata = SaveStateMetadata {
            identity: identity(),
            frame_number: 42,
            virtual_time_ns: 123_456_789,
        };

        let file = SaveStateFile::from_sections(metadata, &sections).unwrap();
        let encoded = file.encode().unwrap();
        let decoded = SaveStateFile::decode(&encoded, identity()).unwrap();
        assert_eq!(decoded.metadata(), metadata);
        assert_eq!(decoded.decode_sections().unwrap(), sections);
    }

    #[test]
    fn sectioned_payload_round_trips_independent_versions_and_bytes() {
        let mut payload = SaveStatePayload::new();
        payload
            .insert(SaveStateSection::new(1, 1, b"cpu-state".to_vec()).unwrap())
            .unwrap();
        payload
            .insert(SaveStateSection::new(4, 3, b"gpu-state".to_vec()).unwrap())
            .unwrap();

        let encoded = payload.encode().unwrap();
        let decoded = SaveStatePayload::decode(&encoded).unwrap();
        assert_eq!(decoded, payload);
        assert_eq!(decoded.section(1).unwrap().version(), 1);
        assert_eq!(decoded.section(4).unwrap().payload(), b"gpu-state");
    }

    #[test]
    fn sectioned_payload_rejects_duplicate_ids_and_zero_metadata() {
        let mut payload = SaveStatePayload::new();
        payload
            .insert(SaveStateSection::new(7, 1, Vec::new()).unwrap())
            .unwrap();
        assert_eq!(
            payload.insert(SaveStateSection::new(7, 2, Vec::new()).unwrap()),
            Err(SaveStateError::DuplicateSectionId(7))
        );
        assert_eq!(
            SaveStateSection::new(0, 1, Vec::new()),
            Err(SaveStateError::InvalidSectionId)
        );
        assert_eq!(
            SaveStateSection::new(1, 0, Vec::new()),
            Err(SaveStateError::InvalidSectionVersion)
        );
    }

    #[test]
    fn sectioned_payload_rejects_corruption_truncation_and_trailing_bytes() {
        let mut payload = SaveStatePayload::new();
        payload
            .insert(SaveStateSection::new(3, 1, b"memory".to_vec()).unwrap())
            .unwrap();
        let encoded = payload.encode().unwrap();

        let mut corrupt = encoded.clone();
        corrupt[SAVE_STATE_PAYLOAD_HEADER_SIZE + SAVE_STATE_SECTION_HEADER_SIZE] ^= 1;
        assert_eq!(
            SaveStatePayload::decode(&corrupt),
            Err(SaveStateError::SectionChecksumMismatch(3))
        );
        assert_eq!(
            SaveStatePayload::decode(&encoded[..SAVE_STATE_PAYLOAD_HEADER_SIZE - 1]),
            Err(SaveStateError::TruncatedPayloadHeader)
        );
        assert_eq!(
            SaveStatePayload::decode(&encoded[..encoded.len() - 1]),
            Err(SaveStateError::InvalidLength)
        );
        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(
            SaveStatePayload::decode(&trailing),
            Err(SaveStateError::InvalidLength)
        );
    }

    #[test]
    fn sectioned_payload_rejects_duplicate_ids_in_untrusted_files() {
        let mut payload = SaveStatePayload::new();
        payload
            .insert(SaveStateSection::new(2, 1, vec![9]).unwrap())
            .unwrap();
        let mut encoded = payload.encode().unwrap();
        let first_section = SAVE_STATE_PAYLOAD_HEADER_SIZE;
        let duplicate = encoded[first_section..].to_vec();
        encoded.extend_from_slice(&duplicate);
        encoded[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            SaveStatePayload::decode(&encoded),
            Err(SaveStateError::DuplicateSectionId(2))
        );
    }

    #[test]
    fn sectioned_payload_enforces_count_limit_and_reserved_flags() {
        let mut payload = SaveStatePayload::new();
        for id in 1..=MAX_SAVE_STATE_SECTIONS as u32 {
            payload
                .insert(SaveStateSection::new(id, 1, Vec::new()).unwrap())
                .unwrap();
        }
        assert_eq!(
            payload.insert(SaveStateSection::new(100, 1, Vec::new()).unwrap()),
            Err(SaveStateError::TooManySections(
                (MAX_SAVE_STATE_SECTIONS + 1) as u32
            ))
        );

        let mut encoded = payload.encode().unwrap();
        encoded[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            SaveStatePayload::decode(&encoded),
            Err(SaveStateError::UnsupportedFlags(1))
        );
    }

    #[test]
    fn crc32_matches_the_standard_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn versioned_container_round_trips_metadata_and_payload() {
        let file = sample_file(identity());
        let encoded = file.encode().unwrap();
        assert_eq!(encoded.len(), SAVE_STATE_HEADER_SIZE + file.payload().len());
        assert_eq!(SaveStateFile::decode(&encoded, identity()).unwrap(), file);
    }

    #[test]
    fn homebrew_without_a_title_id_can_use_a_content_fingerprint() {
        let homebrew = SaveStateIdentity {
            title_id: None,
            content_sha256: [0x3C; 32],
        };
        let file = sample_file(homebrew);
        let encoded = file.encode().unwrap();
        assert_eq!(SaveStateFile::decode(&encoded, homebrew).unwrap(), file);
    }

    #[test]
    fn decoder_rejects_truncated_headers_and_payloads() {
        let encoded = sample_file(identity()).encode().unwrap();
        assert_eq!(
            SaveStateFile::decode(&encoded[..16], identity()),
            Err(SaveStateError::TruncatedHeader)
        );
        assert_eq!(
            SaveStateFile::decode(&encoded[..encoded.len() - 1], identity()),
            Err(SaveStateError::InvalidLength)
        );
    }

    #[test]
    fn decoder_rejects_trailing_data_and_corrupt_payloads() {
        let encoded = sample_file(identity()).encode().unwrap();
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            SaveStateFile::decode(&trailing, identity()),
            Err(SaveStateError::InvalidLength)
        );

        let mut corrupt = encoded;
        corrupt[SAVE_STATE_HEADER_SIZE] ^= 0x80;
        assert_eq!(
            SaveStateFile::decode(&corrupt, identity()),
            Err(SaveStateError::ChecksumMismatch)
        );
    }

    #[test]
    fn decoder_rejects_unknown_versions_flags_and_bad_signatures() {
        let encoded = sample_file(identity()).encode().unwrap();

        let mut version = encoded.clone();
        version[8..10].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            SaveStateFile::decode(&version, identity()),
            Err(SaveStateError::UnsupportedVersion(2))
        );

        let mut flags = encoded.clone();
        flags[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            SaveStateFile::decode(&flags, identity()),
            Err(SaveStateError::UnsupportedFlags(1))
        );

        let mut magic = encoded;
        magic[0] ^= 0xFF;
        assert_eq!(
            SaveStateFile::decode(&magic, identity()),
            Err(SaveStateError::InvalidMagic)
        );
    }

    #[test]
    fn decoder_rejects_a_different_title_or_content_revision() {
        let encoded = sample_file(identity()).encode().unwrap();
        let wrong_title = SaveStateIdentity {
            title_id: Some(identity().title_id.unwrap() + 1),
            ..identity()
        };
        assert_eq!(
            SaveStateFile::decode(&encoded, wrong_title),
            Err(SaveStateError::GameMismatch)
        );

        let wrong_content = SaveStateIdentity {
            content_sha256: [0x5A; 32],
            ..identity()
        };
        assert_eq!(
            SaveStateFile::decode(&encoded, wrong_content),
            Err(SaveStateError::GameMismatch)
        );
    }

    #[test]
    fn decoder_rejects_payload_lengths_above_the_limit_before_allocating() {
        let mut encoded = sample_file(identity()).encode().unwrap();
        let oversized = (MAX_SAVE_STATE_PAYLOAD_BYTES as u64) + 1;
        encoded[72..80].copy_from_slice(&oversized.to_le_bytes());
        assert_eq!(
            SaveStateFile::decode(&encoded, identity()),
            Err(SaveStateError::PayloadTooLarge(oversized))
        );
    }

    #[test]
    fn encoder_rejects_missing_content_fingerprints_and_zero_title_ids() {
        for invalid in [
            SaveStateIdentity {
                title_id: None,
                content_sha256: [0; 32],
            },
            SaveStateIdentity {
                title_id: Some(0),
                content_sha256: [0xA5; 32],
            },
        ] {
            assert_eq!(
                SaveStateFile::new(
                    SaveStateMetadata {
                        identity: invalid,
                        frame_number: 0,
                        virtual_time_ns: 0,
                    },
                    Vec::new(),
                ),
                Err(SaveStateError::InvalidIdentity)
            );
        }
    }
}
