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
    /// SHA-256 of the canonical boot content, never a path or display name.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStateError {
    TruncatedHeader,
    InvalidMagic,
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
        MAX_SAVE_STATE_PAYLOAD_BYTES, SAVE_STATE_HEADER_SIZE, SaveStateError, SaveStateFile,
        SaveStateIdentity, SaveStateMetadata, crc32,
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
