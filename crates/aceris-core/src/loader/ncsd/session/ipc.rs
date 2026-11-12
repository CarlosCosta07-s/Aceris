//! Generic decoding of ARM11 IPC message headers and word sections.
//!
//! This module only describes the serialized TLS message layout. It does not
//! emulate session handles, kernel translation, request scheduling or replies.

/// Decoded fixed fields from an ARM11 IPC command header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcCommandHeader {
    raw: u32,
}

impl IpcCommandHeader {
    /// Decodes the command ID and the two word counts from a raw header.
    pub const fn decode(raw: u32) -> Self {
        Self { raw }
    }

    /// Returns the original header word without normalizing unused bits.
    pub const fn raw(self) -> u32 {
        self.raw
    }

    /// Returns the service command ID from bits 16-31.
    pub const fn command_id(self) -> u16 {
        (self.raw >> 16) as u16
    }

    /// Returns the number of fixed-size normal parameter words.
    pub const fn normal_parameter_count(self) -> u8 {
        ((self.raw >> 6) & 0x3F) as u8
    }

    /// Returns the total number of translation-section words.
    ///
    /// This is a word count, not a count of translation descriptors. A
    /// descriptor and its associated values all contribute to this field.
    pub const fn translation_word_count(self) -> u8 {
        (self.raw & 0x3F) as u8
    }

    /// Returns the currently unused bits 12-15 without interpreting them.
    pub const fn reserved_bits(self) -> u8 {
        ((self.raw >> 12) & 0x0F) as u8
    }

    /// Returns the total number of words required, including the header.
    pub const fn expected_word_count(self) -> usize {
        1 + self.normal_parameter_count() as usize
            + self.translation_word_count() as usize
    }
}

/// Failure to read a complete IPC message from a TLS word buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcMessageDecodeError {
    EmptyMessage,
    TruncatedMessage {
        header: u32,
        expected_words: usize,
        actual_words: usize,
    },
}

/// A borrowed view over the normal and translation sections of one IPC message.
///
/// Extra words in the supplied TLS buffer are intentionally excluded because
/// the header declares the exact extent of the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcMessage<'a> {
    header: IpcCommandHeader,
    normal_parameters: &'a [u32],
    translation_words: &'a [u32],
}

impl<'a> IpcMessage<'a> {
    /// Decodes a message and validates that every declared word is present.
    pub fn decode(words: &'a [u32]) -> Result<Self, IpcMessageDecodeError> {
        let raw_header = *words.first().ok_or(IpcMessageDecodeError::EmptyMessage)?;
        let header = IpcCommandHeader::decode(raw_header);
        let expected_words = header.expected_word_count();
        if words.len() < expected_words {
            return Err(IpcMessageDecodeError::TruncatedMessage {
                header: raw_header,
                expected_words,
                actual_words: words.len(),
            });
        }

        let normal_end = 1 + usize::from(header.normal_parameter_count());
        let translation_end = expected_words;
        Ok(Self {
            header,
            normal_parameters: &words[1..normal_end],
            translation_words: &words[normal_end..translation_end],
        })
    }

    pub const fn header(self) -> IpcCommandHeader {
        self.header
    }

    pub const fn normal_parameters(self) -> &'a [u32] {
        self.normal_parameters
    }

    pub const fn translation_words(self) -> &'a [u32] {
        self.translation_words
    }
}

#[cfg(test)]
mod tests {
    use super::{IpcCommandHeader, IpcMessage, IpcMessageDecodeError};

    #[test]
    fn decodes_header_counts_and_separates_message_sections() {
        let raw_header = 0x1234_0000 | (2 << 6) | 3;
        let words = [raw_header, 0xAA, 0xBB, 0x01, 0xCC, 0xDD];
        let message = IpcMessage::decode(&words).unwrap();

        assert_eq!(message.header().raw(), raw_header);
        assert_eq!(message.header().command_id(), 0x1234);
        assert_eq!(message.header().normal_parameter_count(), 2);
        assert_eq!(message.header().translation_word_count(), 3);
        assert_eq!(message.header().reserved_bits(), 0);
        assert_eq!(message.header().expected_word_count(), 6);
        assert_eq!(message.normal_parameters(), &[0xAA, 0xBB]);
        assert_eq!(message.translation_words(), &[0x01, 0xCC, 0xDD]);
    }

    #[test]
    fn reports_truncated_message_using_declared_total_word_count() {
        let words = [0x1234_0042, 0xAA, 0xBB];
        assert_eq!(
            IpcMessage::decode(&words),
            Err(IpcMessageDecodeError::TruncatedMessage {
                header: 0x1234_0042,
                expected_words: 4,
                actual_words: 3,
            })
        );
    }

    #[test]
    fn ignores_unused_tls_words_after_the_declared_message() {
        let words = [0x1234_0040, 0xAABB_CCDD, 0xDEAD_BEEF];
        let message = IpcMessage::decode(&words).unwrap();

        assert_eq!(message.normal_parameters(), &[0xAABB_CCDD]);
        assert!(message.translation_words().is_empty());
    }

    #[test]
    fn rejects_an_empty_tls_buffer() {
        assert_eq!(
            IpcMessage::decode(&[]),
            Err(IpcMessageDecodeError::EmptyMessage)
        );
    }

    #[test]
    fn header_preserves_reserved_bits_for_service_level_validation() {
        let header = IpcCommandHeader::decode(0x1234_3000);
        assert_eq!(header.reserved_bits(), 3);
        assert_eq!(header.raw(), 0x1234_3000);
    }
}
