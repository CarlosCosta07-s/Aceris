//! Bounded decoder for Nintendo's backwards LZ (BLZ) code-compression format.
//!
//! NCCH ExeFS documents optional reverse-LZSS compression for .code, selected
//! by the extended-header flag. This module implements the documented BLZ
//! footer and backwards LZ10-style token layout; it does not implement a
//! forward LZ10/LZ11 stream.
//!
//! Format references:
//! - https://3dbrew.org/wiki/ExeFS
//! - https://reverseengineering.wiki/index.php/Format:Nintendo_backward_LZ_(BLZ_code_compression)

use std::fmt;

const TRAILER_SIZE: usize = 8;
const MIN_HEADER_SIZE: usize = TRAILER_SIZE;
const HEADER_PADDING_BYTE: u8 = 0xFF;
const COMPRESSED_LENGTH_MASK: u32 = 0x00FF_FFFF;

/// Maximum output accepted by this decoder (256 MiB).
///
/// The caller should also validate the expected size against the executable's
/// code-set metadata before calling the decoder.
pub const MAX_DECOMPRESSED_SIZE: usize = 256 * 1024 * 1024;

/// Errors returned when a backwards-LZ stream is malformed or exceeds limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlzError {
    InputTooShort {
        actual_size: usize,
    },
    InvalidHeaderLength {
        header_length: usize,
        input_size: usize,
    },
    InvalidCompressedLength {
        compressed_length: usize,
        header_length: usize,
        input_size: usize,
    },
    InvalidHeaderPadding,
    OutputSizeOverflow,
    OutputSizeMismatch {
        expected_size: usize,
        declared_size: usize,
    },
    OutputTooLarge {
        requested_size: usize,
        maximum_size: usize,
    },
    CompressedInputExhausted,
    TruncatedBackReference,
    BackReferenceOutOfBounds {
        distance: usize,
        available_output: usize,
    },
    BackReferenceExceedsOutput {
        length: usize,
        available_output: usize,
    },
    UnexpectedCompressedBytes {
        remaining: usize,
    },
}

impl fmt::Display for BlzError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooShort { actual_size } => write!(
                formatter,
                "BLZ input is too short: {actual_size} bytes available, need at least {TRAILER_SIZE}"
            ),
            Self::InvalidHeaderLength {
                header_length,
                input_size,
            } => write!(
                formatter,
                "invalid BLZ header length {header_length} for an input of {input_size} bytes"
            ),
            Self::InvalidCompressedLength {
                compressed_length,
                header_length,
                input_size,
            } => write!(
                formatter,
                "invalid BLZ compressed length {compressed_length} (header {header_length}, input {input_size})"
            ),
            Self::InvalidHeaderPadding => {
                formatter.write_str("BLZ extended trailer padding is not filled with 0xFF")
            }
            Self::OutputSizeOverflow => {
                formatter.write_str("BLZ decompressed size overflows the host address space")
            }
            Self::OutputSizeMismatch {
                expected_size,
                declared_size,
            } => write!(
                formatter,
                "BLZ output size mismatch: code metadata expects {expected_size} bytes, trailer declares {declared_size}"
            ),
            Self::OutputTooLarge {
                requested_size,
                maximum_size,
            } => write!(
                formatter,
                "BLZ output size {requested_size} exceeds the {maximum_size}-byte safety limit"
            ),
            Self::CompressedInputExhausted => {
                formatter.write_str("BLZ compressed stream ended before the output was complete")
            }
            Self::TruncatedBackReference => {
                formatter.write_str("BLZ back-reference is missing one of its two bytes")
            }
            Self::BackReferenceOutOfBounds {
                distance,
                available_output,
            } => write!(
                formatter,
                "BLZ back-reference distance {distance} exceeds {available_output} bytes of initialized output"
            ),
            Self::BackReferenceExceedsOutput {
                length,
                available_output,
            } => write!(
                formatter,
                "BLZ back-reference length {length} exceeds the remaining {available_output} output bytes"
            ),
            Self::UnexpectedCompressedBytes { remaining } => write!(
                formatter,
                "BLZ stream left {remaining} compressed bytes unread"
            ),
        }
    }
}

impl std::error::Error for BlzError {}

/// Decompresses a backwards-LZ stream to the size declared by the code-set metadata.
///
/// A zero size-increment trailer marks a raw (uncompressed) payload. For a
/// compressed payload, the final eight bytes contain a little-endian word
/// (header length in the high byte, compressed length in the low 24 bits)
/// followed by the output-size increment. The uncompressed prefix is copied
/// verbatim; the remaining stream is decoded from end to beginning.
///
/// The expected output size is supplied by the parsed NCCH extended header so
/// malformed trailers cannot request arbitrary allocations.
pub fn decompress(input: &[u8], expected_output_size: usize) -> Result<Vec<u8>, BlzError> {
    if expected_output_size > MAX_DECOMPRESSED_SIZE {
        return Err(BlzError::OutputTooLarge {
            requested_size: expected_output_size,
            maximum_size: MAX_DECOMPRESSED_SIZE,
        });
    }

    if input.len() < TRAILER_SIZE {
        return Err(BlzError::InputTooShort {
            actual_size: input.len(),
        });
    }

    let trailer_offset = input.len() - TRAILER_SIZE;
    let compression_info = read_u32_le(input, trailer_offset);
    let extra_size = read_u32_le(input, trailer_offset + 4);

    if extra_size == 0 {
        if input.len() != expected_output_size {
            return Err(BlzError::OutputSizeMismatch {
                expected_size: expected_output_size,
                declared_size: input.len(),
            });
        }
        return Ok(input.to_vec());
    }

    let declared_size = input
        .len()
        .checked_add(usize::try_from(extra_size).map_err(|_| BlzError::OutputSizeOverflow)?)
        .ok_or(BlzError::OutputSizeOverflow)?;
    if declared_size != expected_output_size {
        return Err(BlzError::OutputSizeMismatch {
            expected_size: expected_output_size,
            declared_size,
        });
    }

    let header_length = (compression_info >> 24) as usize;
    let compressed_length = (compression_info & COMPRESSED_LENGTH_MASK) as usize;
    if !(MIN_HEADER_SIZE..=input.len()).contains(&header_length) {
        return Err(BlzError::InvalidHeaderLength {
            header_length,
            input_size: input.len(),
        });
    }
    if compressed_length < header_length || compressed_length > input.len() {
        return Err(BlzError::InvalidCompressedLength {
            compressed_length,
            header_length,
            input_size: input.len(),
        });
    }

    let compressed_start = input.len() - compressed_length;
    let mut source_position = input.len() - header_length;
    if source_position < compressed_start {
        return Err(BlzError::InvalidCompressedLength {
            compressed_length,
            header_length,
            input_size: input.len(),
        });
    }

    if header_length > TRAILER_SIZE {
        let padding_start = input.len() - header_length;
        let padding_end = input.len() - TRAILER_SIZE;
        if input[padding_start..padding_end]
            .iter()
            .any(|byte| *byte != HEADER_PADDING_BYTE)
        {
            return Err(BlzError::InvalidHeaderPadding);
        }
    }

    let prefix_length = compressed_start;
    let mut output = vec![0u8; expected_output_size];
    output[..prefix_length].copy_from_slice(&input[..prefix_length]);
    let mut output_position = expected_output_size;

    while output_position > prefix_length {
        if source_position <= compressed_start {
            return Err(BlzError::CompressedInputExhausted);
        }

        source_position -= 1;
        let mut flags = input[source_position];

        for _ in 0..8 {
            if output_position == prefix_length {
                break;
            }

            if flags & 0x80 != 0 {
                if source_position.saturating_sub(compressed_start) < 2 {
                    return Err(BlzError::TruncatedBackReference);
                }

                source_position -= 1;
                let first = input[source_position];
                source_position -= 1;
                let second = input[source_position];

                let length = usize::from(first >> 4) + 3;
                let distance = ((usize::from(first & 0x0F) << 8) | usize::from(second)) + 3;
                let available_output = output_position - prefix_length;
                if length > available_output {
                    return Err(BlzError::BackReferenceExceedsOutput {
                        length,
                        available_output,
                    });
                }

                for _ in 0..length {
                    output_position -= 1;
                    let source_index = output_position
                        .checked_add(distance)
                        .filter(|index| *index < output.len())
                        .ok_or(BlzError::BackReferenceOutOfBounds {
                            distance,
                            available_output: output.len() - output_position,
                        })?;
                    output[output_position] = output[source_index];
                }
            } else {
                if source_position <= compressed_start {
                    return Err(BlzError::CompressedInputExhausted);
                }
                source_position -= 1;
                output_position -= 1;
                output[output_position] = input[source_position];
            }

            flags <<= 1;
        }
    }

    if source_position != compressed_start {
        return Err(BlzError::UnexpectedCompressedBytes {
            remaining: source_position - compressed_start,
        });
    }

    Ok(output)
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repeated_a_stream() -> Vec<u8> {
        // Backwards operations: 3 literals, one 18-byte reference (distance 3),
        // 4 literals, then 5 literals. The decoded result is 30 bytes of 'A'.
        let mut read_order = vec![
            0x10, b'A', b'A', b'A', 0xF0, 0x00, b'A', b'A', b'A', b'A', 0x00, b'A', b'A', b'A',
            b'A', b'A',
        ];
        read_order.reverse();

        let compressed_length = (read_order.len() + TRAILER_SIZE) as u32;
        read_order.extend_from_slice(&((8u32 << 24) | compressed_length).to_le_bytes());
        read_order.extend_from_slice(&6u32.to_le_bytes());
        read_order
    }

    #[test]
    fn decompresses_backward_literals_and_overlapping_references() {
        let input = repeated_a_stream();
        assert_eq!(input.len() + 6, 30);

        let output = decompress(&input, 30).unwrap();
        assert_eq!(output, vec![b'A'; 30]);
    }

    #[test]
    fn returns_uncompressed_data_when_size_increment_is_zero() {
        let input = b"uncompressed test payload";
        let mut data = input.to_vec();
        data.extend_from_slice(&0u32.to_le_bytes());

        assert_eq!(decompress(&data, data.len()).unwrap(), data);
    }

    #[test]
    fn rejects_truncated_trailers() {
        assert_eq!(
            decompress(&[0; TRAILER_SIZE - 1], 0),
            Err(BlzError::InputTooShort {
                actual_size: TRAILER_SIZE - 1,
            })
        );
    }

    #[test]
    fn rejects_output_size_mismatches() {
        let input = repeated_a_stream();
        assert_eq!(
            decompress(&input, 31),
            Err(BlzError::OutputSizeMismatch {
                expected_size: 31,
                declared_size: 30,
            })
        );
    }

    #[test]
    fn rejects_back_references_outside_initialized_output() {
        let mut read_order = vec![0x80, 0xF0, 0xFF];
        read_order.reverse();
        let compressed_length = (read_order.len() + TRAILER_SIZE) as u32;
        read_order.extend_from_slice(&((8u32 << 24) | compressed_length).to_le_bytes());
        read_order.extend_from_slice(&7u32.to_le_bytes());

        assert!(matches!(
            decompress(&read_order, 18),
            Err(BlzError::BackReferenceOutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_outputs_above_the_safety_limit() {
        assert_eq!(
            decompress(&[0; TRAILER_SIZE], MAX_DECOMPRESSED_SIZE + 1),
            Err(BlzError::OutputTooLarge {
                requested_size: MAX_DECOMPRESSED_SIZE + 1,
                maximum_size: MAX_DECOMPRESSED_SIZE,
            })
        );
    }

    #[test]
    fn rejects_invalid_header_lengths() {
        let mut input = repeated_a_stream();
        let trailer_offset = input.len() - TRAILER_SIZE;
        input[trailer_offset + 3] = 7;

        assert!(matches!(
            decompress(&input, 30),
            Err(BlzError::InvalidHeaderLength { .. })
        ));
    }
}
