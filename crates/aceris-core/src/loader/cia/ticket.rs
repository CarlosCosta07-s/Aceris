//! Structural parser for Nintendo 3DS ticket (TIK) data.
//!
//! The parser extracts ticket metadata and preserves the encrypted title key.
//! It does not verify RSA/ECDSA signatures or decrypt the title key.

use std::fmt;

/// Fixed size of the 3DS ticket-data area after its aligned signature block.
pub const TICKET_DATA_SIZE: usize = 0x210;
/// Maximum supported ticket size for the largest signature type.
pub const MAX_TICKET_SIZE_BYTES: usize = 0x240 + TICKET_DATA_SIZE;

const SIGNATURE_TYPE_OFFSET: usize = 0;
const TICKET_VERSION_OFFSET: usize = 0x7C;
const CA_CRL_VERSION_OFFSET: usize = 0x7D;
const SIGNER_CRL_VERSION_OFFSET: usize = 0x7E;
const ENCRYPTED_TITLE_KEY_OFFSET: usize = 0x7F;
const TICKET_ID_OFFSET: usize = 0x90;
const CONSOLE_ID_OFFSET: usize = 0x98;
const TITLE_ID_OFFSET: usize = 0x9C;
const TITLE_VERSION_OFFSET: usize = 0xA6;
const LICENSE_TYPE_OFFSET: usize = 0xB0;
const COMMON_KEY_INDEX_OFFSET: usize = 0xB1;

/// Signature algorithms recognized by the 3DS ticket format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketSignatureType {
    Rsa4096Sha1,
    Rsa2048Sha1,
    EcdsaSha1,
    Rsa4096Sha256,
    Rsa2048Sha256,
    EcdsaSha256,
}

impl TicketSignatureType {
    fn from_raw(value: u32) -> Option<Self> {
        match value {
            0x0001_0000 => Some(Self::Rsa4096Sha1),
            0x0001_0001 => Some(Self::Rsa2048Sha1),
            0x0001_0002 => Some(Self::EcdsaSha1),
            0x0001_0003 => Some(Self::Rsa4096Sha256),
            0x0001_0004 => Some(Self::Rsa2048Sha256),
            0x0001_0005 => Some(Self::EcdsaSha256),
            _ => None,
        }
    }

    const fn signature_size(self) -> usize {
        match self {
            Self::Rsa4096Sha1 | Self::Rsa4096Sha256 => 0x200,
            Self::Rsa2048Sha1 | Self::Rsa2048Sha256 => 0x100,
            Self::EcdsaSha1 | Self::EcdsaSha256 => 0x3C,
        }
    }

    /// Returns the signature-type value stored in big-endian form.
    pub const fn raw_value(self) -> u32 {
        match self {
            Self::Rsa4096Sha1 => 0x0001_0000,
            Self::Rsa2048Sha1 => 0x0001_0001,
            Self::EcdsaSha1 => 0x0001_0002,
            Self::Rsa4096Sha256 => 0x0001_0003,
            Self::Rsa2048Sha256 => 0x0001_0004,
            Self::EcdsaSha256 => 0x0001_0005,
        }
    }
}

/// Parsed fields from a 3DS ticket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TicketHeader {
    pub signature_type: TicketSignatureType,
    /// Size of the signature header, including alignment padding.
    pub signature_data_size_bytes: usize,
    pub issuer: [u8; 0x40],
    pub version: u8,
    pub ca_crl_version: u8,
    pub signer_crl_version: u8,
    /// The title key as stored in the ticket; it remains encrypted.
    pub encrypted_title_key: [u8; 16],
    pub ticket_id: u64,
    pub console_id: u32,
    pub title_id: u64,
    pub title_version: u16,
    pub license_type: u8,
    pub common_key_index: u8,
    /// End of the fixed ticket data; extra bytes are not interpreted.
    pub parsed_size_bytes: usize,
}

impl TicketHeader {
    /// Parses a ticket's aligned signature header and fixed 0x210-byte data.
    ///
    /// The supplied slice may contain trailing bytes, which are deliberately
    /// left uninterpreted. The ticket signature itself is not verified.
    pub fn parse(data: &[u8]) -> Result<Self, TicketError> {
        let signature_type_raw = read_u32_be(data, SIGNATURE_TYPE_OFFSET)?;
        let signature_type = TicketSignatureType::from_raw(signature_type_raw)
            .ok_or(TicketError::UnsupportedSignatureType(signature_type_raw))?;
        let signature_data_size_bytes = aligned_signature_data_size(signature_type)?;

        ensure_range(data, 0, signature_data_size_bytes, "signature data")?;
        let ticket_data_start = signature_data_size_bytes;
        ensure_range(data, ticket_data_start, TICKET_DATA_SIZE, "ticket data")?;
        let ticket_data_end = ticket_data_start
            .checked_add(TICKET_DATA_SIZE)
            .ok_or(TicketError::SizeOverflow)?;
        let ticket_data = &data[ticket_data_start..ticket_data_end];

        Ok(Self {
            signature_type,
            signature_data_size_bytes,
            issuer: read_array::<0x40>(ticket_data, 0)?,
            version: ticket_data[TICKET_VERSION_OFFSET],
            ca_crl_version: ticket_data[CA_CRL_VERSION_OFFSET],
            signer_crl_version: ticket_data[SIGNER_CRL_VERSION_OFFSET],
            encrypted_title_key: read_array::<16>(ticket_data, ENCRYPTED_TITLE_KEY_OFFSET)?,
            ticket_id: read_u64_be(ticket_data, TICKET_ID_OFFSET)?,
            console_id: read_u32_be(ticket_data, CONSOLE_ID_OFFSET)?,
            title_id: read_u64_be(ticket_data, TITLE_ID_OFFSET)?,
            title_version: read_u16_be(ticket_data, TITLE_VERSION_OFFSET)?,
            license_type: ticket_data[LICENSE_TYPE_OFFSET],
            common_key_index: ticket_data[COMMON_KEY_INDEX_OFFSET],
            parsed_size_bytes: ticket_data_end,
        })
    }

    /// Builds the 16-byte AES-CBC IV used to decrypt this ticket's title key.
    ///
    /// The 64-bit title ID is encoded in big-endian order in the first half;
    /// the remaining eight bytes are zero. This prepares the IV only and does
    /// not decrypt or alter the stored encrypted title key.
    pub const fn title_key_iv(&self) -> [u8; 16] {
        let title_id = self.title_id.to_be_bytes();
        [
            title_id[0],
            title_id[1],
            title_id[2],
            title_id[3],
            title_id[4],
            title_id[5],
            title_id[6],
            title_id[7],
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]
    }
}

fn aligned_signature_data_size(signature_type: TicketSignatureType) -> Result<usize, TicketError> {
    4usize
        .checked_add(signature_type.signature_size())
        .and_then(|size| size.checked_add(0x3F))
        .map(|size| size & !0x3F)
        .ok_or(TicketError::SizeOverflow)
}

fn ensure_range(
    data: &[u8],
    offset: usize,
    size: usize,
    section: &'static str,
) -> Result<(), TicketError> {
    let end = offset.checked_add(size).ok_or(TicketError::SizeOverflow)?;
    if end > data.len() {
        return Err(TicketError::Truncated { section });
    }
    Ok(())
}

fn read_u16_be(data: &[u8], offset: usize) -> Result<u16, TicketError> {
    let bytes = data
        .get(offset..offset.checked_add(2).ok_or(TicketError::SizeOverflow)?)
        .ok_or(TicketError::Truncated {
            section: "ticket field",
        })?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32_be(data: &[u8], offset: usize) -> Result<u32, TicketError> {
    let bytes = data
        .get(offset..offset.checked_add(4).ok_or(TicketError::SizeOverflow)?)
        .ok_or(TicketError::Truncated {
            section: "ticket field",
        })?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64_be(data: &[u8], offset: usize) -> Result<u64, TicketError> {
    let bytes = data
        .get(offset..offset.checked_add(8).ok_or(TicketError::SizeOverflow)?)
        .ok_or(TicketError::Truncated {
            section: "ticket field",
        })?;
    Ok(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N], TicketError> {
    data.get(offset..offset.checked_add(N).ok_or(TicketError::SizeOverflow)?)
        .ok_or(TicketError::Truncated {
            section: "ticket field",
        })?
        .try_into()
        .map_err(|_| TicketError::Truncated {
            section: "ticket field",
        })
}

/// Failures while parsing 3DS ticket data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TicketError {
    UnsupportedSignatureType(u32),
    Truncated { section: &'static str },
    SizeOverflow,
}

impl fmt::Display for TicketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSignatureType(value) => {
                write!(formatter, "unsupported ticket signature type {value:#010x}")
            }
            Self::Truncated { section } => write!(formatter, "ticket is truncated in {section}"),
            Self::SizeOverflow => {
                formatter.write_str("ticket section size overflows the host range")
            }
        }
    }
}

impl std::error::Error for TicketError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(signature_type: TicketSignatureType) -> Vec<u8> {
        let signature_size = aligned_signature_data_size(signature_type).unwrap();
        let mut data = vec![0u8; signature_size + TICKET_DATA_SIZE];
        data[0..4].copy_from_slice(&signature_type.raw_value().to_be_bytes());
        let ticket = &mut data[signature_size..];
        ticket[0..0x40].fill(b'I');
        ticket[TICKET_VERSION_OFFSET] = 1;
        ticket[CA_CRL_VERSION_OFFSET] = 2;
        ticket[SIGNER_CRL_VERSION_OFFSET] = 3;
        ticket[ENCRYPTED_TITLE_KEY_OFFSET..ENCRYPTED_TITLE_KEY_OFFSET + 16]
            .copy_from_slice(&[0xA5; 16]);
        ticket[TICKET_ID_OFFSET..TICKET_ID_OFFSET + 8]
            .copy_from_slice(&0x1122_3344_5566_7788u64.to_be_bytes());
        ticket[CONSOLE_ID_OFFSET..CONSOLE_ID_OFFSET + 4]
            .copy_from_slice(&0x1234_5678u32.to_be_bytes());
        ticket[TITLE_ID_OFFSET..TITLE_ID_OFFSET + 8]
            .copy_from_slice(&0x0004_0000_1234_5678u64.to_be_bytes());
        ticket[TITLE_VERSION_OFFSET..TITLE_VERSION_OFFSET + 2]
            .copy_from_slice(&0x0102u16.to_be_bytes());
        ticket[LICENSE_TYPE_OFFSET] = 1;
        ticket[COMMON_KEY_INDEX_OFFSET] = 2;
        data
    }

    #[test]
    fn parses_ticket_metadata_and_keeps_the_title_key_encrypted() {
        let data = fixture(TicketSignatureType::Rsa2048Sha256);
        let ticket = TicketHeader::parse(&data).unwrap();

        assert_eq!(ticket.signature_type, TicketSignatureType::Rsa2048Sha256);
        assert_eq!(ticket.signature_data_size_bytes, 0x140);
        assert_eq!(ticket.issuer, [b'I'; 0x40]);
        assert_eq!(ticket.version, 1);
        assert_eq!(ticket.ca_crl_version, 2);
        assert_eq!(ticket.signer_crl_version, 3);
        assert_eq!(ticket.encrypted_title_key, [0xA5; 16]);
        assert_eq!(ticket.ticket_id, 0x1122_3344_5566_7788);
        assert_eq!(ticket.console_id, 0x1234_5678);
        assert_eq!(ticket.title_id, 0x0004_0000_1234_5678);
        assert_eq!(ticket.title_version, 0x0102);
        assert_eq!(ticket.license_type, 1);
        assert_eq!(ticket.common_key_index, 2);
        assert_eq!(ticket.parsed_size_bytes, data.len());
        assert_eq!(
            ticket.title_key_iv(),
            [
                0x00, 0x04, 0x00, 0x00, 0x12, 0x34, 0x56, 0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ]
        );
    }

    #[test]
    fn accepts_all_documented_signature_alignments() {
        for signature_type in [
            TicketSignatureType::Rsa4096Sha1,
            TicketSignatureType::Rsa2048Sha1,
            TicketSignatureType::EcdsaSha1,
            TicketSignatureType::Rsa4096Sha256,
            TicketSignatureType::Rsa2048Sha256,
            TicketSignatureType::EcdsaSha256,
        ] {
            let data = fixture(signature_type);
            let ticket = TicketHeader::parse(&data).unwrap();
            assert_eq!(
                ticket.signature_type,
                signature_type,
                "signature type {:#010x}",
                signature_type.raw_value()
            );
        }
    }

    #[test]
    fn rejects_unknown_signature_types() {
        let mut data = fixture(TicketSignatureType::Rsa2048Sha256);
        data[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());

        assert_eq!(
            TicketHeader::parse(&data),
            Err(TicketError::UnsupportedSignatureType(0xDEAD_BEEF))
        );
    }

    #[test]
    fn rejects_truncated_signature_and_ticket_data() {
        let data = fixture(TicketSignatureType::Rsa2048Sha256);
        assert_eq!(
            TicketHeader::parse(&data[..0x100]),
            Err(TicketError::Truncated {
                section: "signature data"
            })
        );
        assert_eq!(
            TicketHeader::parse(&data[..data.len() - 1]),
            Err(TicketError::Truncated {
                section: "ticket data"
            })
        );
    }
}
