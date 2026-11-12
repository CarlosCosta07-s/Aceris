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
        1 + self.normal_parameter_count() as usize + self.translation_word_count() as usize
    }
}

/// One decoded parameter from the IPC translation section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcTranslationParameter<'a> {
    /// A type-0 handle transfer, including its untouched handle words.
    Handles {
        descriptor: u32,
        word_offset: usize,
        handle_count: u8,
        move_handles: bool,
        requests_process_id: bool,
        handles: &'a [u32],
    },
    /// A type-1 static-buffer copy targeting the receiver's TLS static-buffer slot.
    StaticBuffer {
        descriptor: u32,
        word_offset: usize,
        buffer_index: u8,
        size_bytes: u32,
        source_address: u32,
    },
    /// A type-5/6/7 temporary buffer mapping with source access permissions.
    MappedBuffer {
        descriptor: u32,
        word_offset: usize,
        permissions: u8,
        size_bytes: u32,
        source_address: u32,
    },
    /// Type-2/3 physical chunk table written into a page-aligned receiver static buffer.
    PhysicalBufferTable {
        descriptor: u32,
        word_offset: usize,
        buffer_index: u8,
        size_bytes: u32,
        source_address: u32,
        read_only: bool,
    },
}

/// Errors found while walking a complete IPC translation section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcTranslationSectionError {
    InvalidDescriptor {
        descriptor: u32,
    },
    UnsupportedTranslationType {
        descriptor: u32,
        translation_type: u8,
    },
    TruncatedParameter {
        descriptor: u32,
        expected_words: usize,
        available_words: usize,
    },
}

/// Decodes every parameter in a translation section without touching guest memory.
///
/// This recognizes type 0 handles, type 1 static-buffer copies, type 2/3
/// physical chunk tables and type 5/6/7 temporary buffer mappings. The parser
/// validates descriptor structure only; process-memory validation and transfer
/// are performed by the session transport layer.
pub fn decode_translation_section(
    words: &[u32],
) -> Result<Vec<IpcTranslationParameter<'_>>, IpcTranslationSectionError> {
    let mut parameters = Vec::new();
    let mut cursor = 0usize;

    while cursor < words.len() {
        let descriptor = words[cursor];
        cursor += 1;
        let translation_type = ((descriptor >> 1) & 0x7) as u8;

        let payload_words = match translation_type {
            0 => {
                let decoded = super::kernel::IpcHandleTranslationDescriptor::decode(descriptor)
                    .map_err(|_| IpcTranslationSectionError::InvalidDescriptor { descriptor })?;
                usize::from(decoded.handle_count())
            }
            1 | 2 | 3 | 5 | 6 | 7 => 1,
            other => {
                return Err(IpcTranslationSectionError::UnsupportedTranslationType {
                    descriptor,
                    translation_type: other,
                });
            }
        };

        let available_words = words.len() - cursor;
        if available_words < payload_words {
            return Err(IpcTranslationSectionError::TruncatedParameter {
                descriptor,
                expected_words: payload_words,
                available_words,
            });
        }
        let payload_end = cursor + payload_words;
        let payload = &words[cursor..payload_end];

        let parameter = match translation_type {
            0 => {
                let decoded = super::kernel::IpcHandleTranslationDescriptor::decode(descriptor)
                    .map_err(|_| IpcTranslationSectionError::InvalidDescriptor { descriptor })?;
                IpcTranslationParameter::Handles {
                    descriptor,
                    word_offset: cursor - payload_words - 1,
                    handle_count: decoded.handle_count(),
                    move_handles: decoded.moves_handle(),
                    requests_process_id: decoded.requests_process_id(),
                    handles: payload,
                }
            }
            1 => {
                // Bit 0 and bits 4-9 are reserved in the known static-buffer form.
                if descriptor & 0x0000_03F1 != 0 {
                    return Err(IpcTranslationSectionError::InvalidDescriptor { descriptor });
                }
                IpcTranslationParameter::StaticBuffer {
                    descriptor,
                    word_offset: cursor - payload_words - 1,
                    buffer_index: ((descriptor >> 10) & 0x0F) as u8,
                    size_bytes: descriptor >> 14,
                    source_address: payload[0],
                }
            }
            2 | 3 => {
                if descriptor & 1 != 0 || descriptor >> 8 == 0 {
                    return Err(IpcTranslationSectionError::InvalidDescriptor { descriptor });
                }
                IpcTranslationParameter::PhysicalBufferTable {
                    descriptor,
                    word_offset: cursor - payload_words - 1,
                    buffer_index: ((descriptor >> 4) & 0x0F) as u8,
                    size_bytes: descriptor >> 8,
                    source_address: payload[0],
                    read_only: translation_type == 3,
                }
            }
            5..=7 => {
                if descriptor & 1 != 0 || descriptor >> 4 == 0 {
                    return Err(IpcTranslationSectionError::InvalidDescriptor { descriptor });
                }
                IpcTranslationParameter::MappedBuffer {
                    descriptor,
                    word_offset: cursor - payload_words - 1,
                    permissions: translation_type & 0x03,
                    size_bytes: descriptor >> 4,
                    source_address: payload[0],
                }
            }
            _ => unreachable!("translation type was checked above"),
        };

        parameters.push(parameter);
        cursor = payload_end;
    }

    Ok(parameters)
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

    /// Decodes the complete translation section into typed, borrowed parameters.
    pub fn translation_parameters(
        self,
    ) -> Result<Vec<IpcTranslationParameter<'a>>, IpcTranslationSectionError> {
        decode_translation_section(self.translation_words)
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

    #[test]
    fn decodes_handle_static_buffer_and_mapped_buffer_parameters() {
        let handles = [0x1111, 0x2222];
        let words = [
            0x0400_0000, // COPY two handles.
            handles[0],
            handles[1],
            0x0008_0C02, // Static buffer 3, 0x20 bytes.
            0x0030_0000,
            0x0001_000A, // Read-only mapped buffer, 0x1000 bytes.
            0x0040_0000,
            0x0000_200C, // Write-only mapped buffer, 0x200 bytes.
            0x0050_0000,
            0x0000_300E, // Read/write mapped buffer, 0x300 bytes.
            0x0060_0000,
        ];

        let parameters = super::decode_translation_section(&words).unwrap();
        assert_eq!(parameters.len(), 5);
        assert!(matches!(
            parameters[0],
            super::IpcTranslationParameter::Handles {
                handle_count: 2,
                move_handles: false,
                requests_process_id: false,
                handles: actual,
                ..
            } if actual == handles.as_slice()
        ));
        assert!(matches!(
            parameters[1],
            super::IpcTranslationParameter::StaticBuffer {
                buffer_index: 3,
                size_bytes: 0x20,
                source_address: 0x0030_0000,
                ..
            }
        ));
        assert!(matches!(
            parameters[2],
            super::IpcTranslationParameter::MappedBuffer {
                permissions: 1,
                size_bytes: 0x1000,
                source_address: 0x0040_0000,
                ..
            }
        ));
        assert!(matches!(
            parameters[3],
            super::IpcTranslationParameter::MappedBuffer {
                permissions: 2,
                size_bytes: 0x200,
                source_address: 0x0050_0000,
                ..
            }
        ));
        assert!(matches!(
            parameters[4],
            super::IpcTranslationParameter::MappedBuffer {
                permissions: 3,
                size_bytes: 0x300,
                source_address: 0x0060_0000,
                ..
            }
        ));
    }

    #[test]
    fn rejects_truncated_translation_payloads() {
        assert_eq!(
            super::decode_translation_section(&[0x0400_0000, 0x1234]),
            Err(super::IpcTranslationSectionError::TruncatedParameter {
                descriptor: 0x0400_0000,
                expected_words: 2,
                available_words: 1,
            })
        );
        assert_eq!(
            super::decode_translation_section(&[0x0008_0C02]),
            Err(super::IpcTranslationSectionError::TruncatedParameter {
                descriptor: 0x0008_0C02,
                expected_words: 1,
                available_words: 0,
            })
        );
    }

    #[test]
    fn decodes_type_two_and_three_physical_buffer_table_descriptors() {
        let type_two = (0x400u32 << 8) | (3 << 4) | 0x04;
        let type_three = (0x200u32 << 8) | (2 << 4) | 0x06;
        let words = [
            type_two,
            0x1400_0100,
            type_three,
            0x1400_2000,
        ];

        let parameters = super::decode_translation_section(&words).unwrap();
        assert_eq!(parameters.len(), 2);
        assert!(matches!(
            parameters[0],
            super::IpcTranslationParameter::PhysicalBufferTable {
                buffer_index: 3,
                size_bytes: 0x400,
                source_address: 0x1400_0100,
                read_only: false,
                word_offset: 0,
                ..
            }
        ));
        assert!(matches!(
            parameters[1],
            super::IpcTranslationParameter::PhysicalBufferTable {
                buffer_index: 2,
                size_bytes: 0x200,
                source_address: 0x1400_2000,
                read_only: true,
                word_offset: 2,
                ..
            }
        ));
    }

    #[test]
    fn rejects_zero_sized_or_reserved_physical_buffer_descriptors() {
        for descriptor in [0x0000_0004, 0x0000_0006, 0x0000_0005, 0x0000_0007] {
            assert_eq!(
                super::decode_translation_section(&[descriptor, 0]),
                Err(super::IpcTranslationSectionError::InvalidDescriptor { descriptor })
            );
        }
    }

    #[test]
    fn rejects_reserved_static_buffer_descriptor_bits() {
        let descriptor = 0x0008_0C12;
        assert_eq!(
            super::decode_translation_section(&[descriptor, 0x0030_0000]),
            Err(super::IpcTranslationSectionError::InvalidDescriptor { descriptor })
        );
    }

    #[test]
    fn rejects_reserved_mapped_buffer_descriptor_bit() {
        let descriptor = 0x0001_000B;
        assert_eq!(
            super::decode_translation_section(&[descriptor, 0x0040_0000]),
            Err(super::IpcTranslationSectionError::InvalidDescriptor { descriptor })
        );
    }

    #[test]
    fn rejects_zero_length_mapped_buffers() {
        for descriptor in [0x0A, 0x0C, 0x0E] {
            assert_eq!(
                super::decode_translation_section(&[descriptor, 0x0040_0000]),
                Err(super::IpcTranslationSectionError::InvalidDescriptor { descriptor })
            );
        }
    }

    #[test]
    fn decodes_process_id_translation_as_a_type_zero_parameter() {
        let parameters = super::decode_translation_section(&[0x20, 0xFFFF_FFFF]).unwrap();
        assert!(matches!(
            parameters.as_slice(),
            [super::IpcTranslationParameter::Handles {
                handle_count: 1,
                move_handles: false,
                requests_process_id: true,
                handles,
                ..
            }] if handles.first() == Some(&0xFFFF_FFFF)
        ));
        assert_eq!(
            super::decode_translation_section(&[0x30, 0]),
            Err(super::IpcTranslationSectionError::InvalidDescriptor { descriptor: 0x30 })
        );
    }

    #[test]
    fn an_empty_translation_section_contains_no_parameters() {
        assert!(super::decode_translation_section(&[]).unwrap().is_empty());
    }

    #[test]
    fn message_exposes_typed_translation_parameters() {
        let words = [0x1234_0002, 0x0000_4002, 0x0040_0000];
        let message = IpcMessage::decode(&words).unwrap();
        assert!(matches!(
            message.translation_parameters().unwrap().as_slice(),
            [super::IpcTranslationParameter::StaticBuffer {
                buffer_index: 0,
                size_bytes: 1,
                source_address: 0x0040_0000,
                ..
            }]
        ));
    }
}
