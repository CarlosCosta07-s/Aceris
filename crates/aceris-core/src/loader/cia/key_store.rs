//! Caller-supplied common-key material for decrypting 3DS ticket title keys.
//!
//! The ticket index selects one of six entries for AES keyslot 0x3D. Retail
//! entries are KeyY values combined with the caller-supplied KeyX. A caller may
//! also supply an already-derived normal key for a development configuration.
//! Aceris never extracts, embeds, downloads, or persists console key material.
//!
//! Key material remains ordinary process memory. This type redacts its Debug
//! output, but it does not promise secure memory erasure or protect against
//! process-memory inspection.

use std::fmt;

use crate::loader::ncsd::ncch::crypto::derive_normal_key;

use super::ticket::TicketHeader;

/// Number of common-key entries addressable by a 3DS ticket.
pub const COMMON_KEY_COUNT: usize = 6;

#[derive(Clone, Copy)]
enum TicketCommonKeyMaterial {
    KeyY([u8; 16]),
    Normal([u8; 16]),
}

/// Caller-owned KeyX and ticket-selected common-key values.
#[derive(Default)]
pub struct TicketCommonKeyStore {
    key_x: Option<[u8; 16]>,
    entries: [Option<TicketCommonKeyMaterial>; COMMON_KEY_COUNT],
}

impl TicketCommonKeyStore {
    /// Supplies the KeyX value used by the retail ticket common-key slot.
    ///
    /// Replacing the value affects all subsequently derived normal keys that
    /// were configured through set_key_y.
    pub fn set_key_x(&mut self, key_x: [u8; 16]) {
        self.key_x = Some(key_x);
    }

    /// Removes the currently configured KeyX value.
    pub fn clear_key_x(&mut self) {
        self.key_x = None;
    }

    /// Returns whether a KeyX value is configured.
    pub const fn has_key_x(&self) -> bool {
        self.key_x.is_some()
    }

    /// Supplies a KeyY value for a ticket common-key index.
    pub fn set_key_y(&mut self, index: u8, key_y: [u8; 16]) -> Result<(), TicketKeyError> {
        let slot = Self::key_y_slot(index)?;
        self.entries[slot] = Some(TicketCommonKeyMaterial::KeyY(key_y));
        Ok(())
    }

    /// Supplies an already-derived normal key for a ticket common-key index.
    ///
    /// This is useful for configurations where the platform provides the
    /// normal key directly, rather than the KeyX/KeyY pair.
    pub fn set_normal_key(
        &mut self,
        index: u8,
        normal_key: [u8; 16],
    ) -> Result<(), TicketKeyError> {
        let slot = Self::key_y_slot(index)?;
        self.entries[slot] = Some(TicketCommonKeyMaterial::Normal(normal_key));
        Ok(())
    }

    /// Removes the key material configured at an index.
    pub fn clear_key(&mut self, index: u8) -> Result<bool, TicketKeyError> {
        let slot = Self::key_y_slot(index)?;
        Ok(self.entries[slot].take().is_some())
    }

    /// Returns whether key material is configured for this index.
    pub fn has_key(&self, index: u8) -> Result<bool, TicketKeyError> {
        let slot = Self::key_y_slot(index)?;
        Ok(self.entries[slot].is_some())
    }

    /// Derives or returns the AES-128 normal key selected by a ticket index.
    ///
    /// KeyY entries require the caller-supplied KeyX. Direct normal-key entries
    /// do not. The returned key is a copy intended for a caller-owned
    /// cryptographic operation; this store does not retain a derived-key cache.
    pub fn derive_normal_key(&self, index: u8) -> Result<[u8; 16], TicketKeyError> {
        let slot = Self::key_y_slot(index)?;
        match self.entries[slot] {
            Some(TicketCommonKeyMaterial::Normal(normal_key)) => Ok(normal_key),
            Some(TicketCommonKeyMaterial::KeyY(key_y)) => {
                let key_x = self.key_x.ok_or(TicketKeyError::MissingKeyX)?;
                Ok(derive_normal_key(key_x, key_y))
            }
            None => Err(TicketKeyError::MissingKeyMaterial { index }),
        }
    }

    /// Resolves the normal key selected by a parsed ticket.
    pub fn derive_ticket_normal_key(
        &self,
        ticket: &TicketHeader,
    ) -> Result<[u8; 16], TicketKeyError> {
        self.derive_normal_key(ticket.common_key_index)
    }

    /// Decrypts a ticket title key using only caller-provided common-key material.
    pub fn decrypt_ticket_title_key(
        &self,
        ticket: &TicketHeader,
    ) -> Result<super::crypto::TitleKey, TicketKeyError> {
        let normal_key = self.derive_ticket_normal_key(ticket)?;
        Ok(ticket.decrypt_title_key(normal_key))
    }

    fn key_y_slot(index: u8) -> Result<usize, TicketKeyError> {
        let slot = usize::from(index);
        if slot >= COMMON_KEY_COUNT {
            return Err(TicketKeyError::UnsupportedIndex(index));
        }
        Ok(slot)
    }
}

impl fmt::Debug for TicketCommonKeyStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TicketCommonKeyStore")
            .field("key_x_configured", &self.key_x.is_some())
            .field(
                "entry_configured",
                &self.entries.map(|entry| entry.is_some()),
            )
            .finish()
    }
}

/// Failures while resolving the ticket common-key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TicketKeyError {
    UnsupportedIndex(u8),
    MissingKeyX,
    MissingKeyMaterial { index: u8 },
}

impl fmt::Display for TicketKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedIndex(index) => write!(
                formatter,
                "ticket common-key index {index} is outside the supported range 0..{COMMON_KEY_COUNT}"
            ),
            Self::MissingKeyX => formatter.write_str("ticket common-key KeyX is not configured"),
            Self::MissingKeyMaterial { index } => write!(
                formatter,
                "ticket common-key material for index {index} is not configured"
            ),
        }
    }
}

impl std::error::Error for TicketKeyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::ncsd::ncch::crypto::Aes128;

    const TEST_KEY_X: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F,
    ];
    const TEST_KEY_Y: [u8; 16] = [
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E,
        0x1F,
    ];
    const EXPECTED_NORMAL_KEY: [u8; 16] = [
        0x09, 0xC4, 0xBD, 0xCE, 0x56, 0x98, 0x07, 0x81, 0xE4, 0xE4, 0xFF, 0x89, 0x09, 0x9D, 0x41,
        0x62,
    ];

    #[test]
    fn derives_the_normal_key_from_caller_supplied_material() {
        let mut keys = TicketCommonKeyStore::default();
        keys.set_key_x(TEST_KEY_X);
        keys.set_key_y(2, TEST_KEY_Y).unwrap();

        assert_eq!(keys.derive_normal_key(2), Ok(EXPECTED_NORMAL_KEY));
    }

    #[test]
    fn accepts_a_caller_supplied_normal_key_without_key_x() {
        let mut keys = TicketCommonKeyStore::default();
        keys.set_normal_key(0, EXPECTED_NORMAL_KEY).unwrap();

        assert_eq!(keys.derive_normal_key(0), Ok(EXPECTED_NORMAL_KEY));
    }

    #[test]
    fn reports_missing_key_material_without_guessing_a_default() {
        let keys = TicketCommonKeyStore::default();

        assert_eq!(
            keys.derive_normal_key(0),
            Err(TicketKeyError::MissingKeyMaterial { index: 0 })
        );

        let mut keys = TicketCommonKeyStore::default();
        keys.set_key_y(3, TEST_KEY_Y).unwrap();
        assert_eq!(keys.derive_normal_key(3), Err(TicketKeyError::MissingKeyX));
    }

    #[test]
    fn rejects_indices_outside_the_ticket_key_table() {
        let mut keys = TicketCommonKeyStore::default();

        assert_eq!(
            keys.set_key_y(COMMON_KEY_COUNT as u8, TEST_KEY_Y),
            Err(TicketKeyError::UnsupportedIndex(COMMON_KEY_COUNT as u8))
        );
        assert_eq!(
            keys.set_normal_key(u8::MAX, TEST_KEY_Y),
            Err(TicketKeyError::UnsupportedIndex(u8::MAX))
        );
        assert_eq!(
            keys.derive_normal_key(u8::MAX),
            Err(TicketKeyError::UnsupportedIndex(u8::MAX))
        );
    }

    #[test]
    fn clearing_key_material_updates_the_available_slots() {
        let mut keys = TicketCommonKeyStore::default();
        keys.set_key_x(TEST_KEY_X);
        keys.set_key_y(1, TEST_KEY_Y).unwrap();

        assert!(keys.has_key_x());
        assert_eq!(keys.has_key(1), Ok(true));
        assert_eq!(keys.clear_key(1), Ok(true));
        assert_eq!(keys.clear_key(1), Ok(false));
        assert_eq!(keys.has_key(1), Ok(false));

        keys.clear_key_x();
        assert!(!keys.has_key_x());
    }

    #[test]
    fn decrypts_ticket_title_key_from_the_configured_normal_key() {
        let normal_key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
            0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
        ];
        let expected_title_key = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
            0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        ];
        let mut ticket = TicketHeader {
            signature_type: super::super::ticket::TicketSignatureType::Rsa2048Sha256,
            signature_data_size_bytes: 0x140,
            issuer: [0; 0x40],
            version: 1,
            ca_crl_version: 0,
            signer_crl_version: 0,
            encrypted_title_key: [0; 16],
            ticket_id: 0,
            console_id: 0,
            title_id: 0x0001_0203_0405_0607,
            title_version: 0,
            license_type: 0,
            common_key_index: 0,
            parsed_size_bytes: 0x350,
        };

        let iv = ticket.title_key_iv();
        let mut input = expected_title_key;
        for (byte, previous) in input.iter_mut().zip(iv) {
            *byte ^= previous;
        }
        ticket.encrypted_title_key = Aes128::new(normal_key).encrypt_block(input);

        let mut keys = TicketCommonKeyStore::default();
        keys.set_normal_key(0, normal_key).unwrap();

        let decrypted = keys.decrypt_ticket_title_key(&ticket).unwrap();

        assert_eq!(decrypted.as_bytes(), &expected_title_key);
        assert_eq!(format!("{decrypted:?}"), "TitleKey([REDACTED])");
    }

    #[test]
    fn debug_output_does_not_disclose_key_bytes() {
        let mut keys = TicketCommonKeyStore::default();
        keys.set_key_x(TEST_KEY_X);
        keys.set_key_y(2, TEST_KEY_Y).unwrap();

        let debug = format!("{keys:?}");

        assert!(debug.contains("key_x_configured: true"));
        assert!(debug.contains("entry_configured"));
        assert!(!debug.contains("000102030405060708090a0b0c0d0e0f"));
        assert!(!debug.contains("101112131415161718191a1b1c1d1e1f"));
    }
}
