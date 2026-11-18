//! Bounded raw TextureCopy transfers for the GSP GX queue.
//!
//! TextureCopy bypasses pixel-format conversion. In gap mode, source and
//! destination line widths/gaps are independent 16-byte units; transfer size
//! counts copied bytes, not skipped gap bytes. The implementation stages all
//! input before writing output so overlapping ranges are deterministic.

use super::{GpuDmaAddressSpace, GpuPhysicalMemory, VRAM_BASE, VRAM_SIZE};

const TEXTURE_COPY_GAP_MODE: u32 = 1 << 2;
const TEXTURE_COPY_ENGINE_MODE: u32 = 1 << 3;
const GSP_VRAM_ALIAS: u32 = 0x1F00_0000;

/// Defensive maximum for one raw TextureCopy operation.
pub const MAX_TEXTURE_COPY_BYTES: usize = 64 * 1024 * 1024;

/// One raw GSP TextureCopy request.
///
/// The two parameter words pack line width in the low halfword and the
/// following gap in the high halfword. Both values use 16-byte units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureCopyRequest {
    pub source_address: u32,
    pub destination_address: u32,
    pub size_bytes: u32,
    pub source_line_width_and_gap: u32,
    pub destination_line_width_and_gap: u32,
    pub flags: u32,
}

/// Summary of a completed TextureCopy operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureCopyReport {
    pub bytes_transferred: u32,
    pub source_rows_transferred: u32,
    pub destination_rows_transferred: u32,
    pub used_gaps: bool,
}

/// Validation or memory error from the TextureCopy software backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureCopyError {
    TextureCopyModeNotEnabled,
    InvalidSize {
        size_bytes: u32,
        minimum_bytes: u32,
    },
    TransferTooLarge {
        size_bytes: u32,
        maximum_bytes: usize,
    },
    MisalignedAddress {
        address: u32,
    },
    MisalignedSize {
        size_bytes: u32,
    },
    MissingLineWidth,
    AddressOverflow {
        base: u32,
        offset: u64,
    },
    InvalidAddress {
        address: u32,
    },
    AllocationFailed,
    PhysicalMemory(String),
}

impl std::fmt::Display for TextureCopyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextureCopyModeNotEnabled => {
                formatter.write_str("TextureCopy mode flag is not enabled")
            }
            Self::InvalidSize {
                size_bytes,
                minimum_bytes,
            } => write!(
                formatter,
                "TextureCopy size {size_bytes} is below the minimum {minimum_bytes}"
            ),
            Self::TransferTooLarge {
                size_bytes,
                maximum_bytes,
            } => write!(
                formatter,
                "TextureCopy size {size_bytes} exceeds the limit {maximum_bytes}"
            ),
            Self::MisalignedAddress { address } => {
                write!(
                    formatter,
                    "TextureCopy address {address:#010x} is not 8-byte aligned"
                )
            }
            Self::MisalignedSize { size_bytes } => {
                write!(
                    formatter,
                    "TextureCopy size {size_bytes} is not 8-byte aligned"
                )
            }
            Self::MissingLineWidth => formatter.write_str(
                "TextureCopy gap mode requires non-zero source and destination line widths",
            ),
            Self::AddressOverflow { base, offset } => write!(
                formatter,
                "TextureCopy row address overflows from {base:#010x} with offset {offset:#x}"
            ),
            Self::InvalidAddress { address } => {
                write!(
                    formatter,
                    "TextureCopy address range at {address:#010x} is not mapped"
                )
            }
            Self::AllocationFailed => formatter.write_str("TextureCopy staging allocation failed"),
            Self::PhysicalMemory(reason) => {
                write!(formatter, "TextureCopy memory failed: {reason}")
            }
        }
    }
}

impl std::error::Error for TextureCopyError {}

/// Executes a raw copy from the GSP-visible source to destination ranges.
///
/// When bit 2 is clear, the operation is contiguous and the line parameter
/// words are ignored. In gap mode, source and destination bytes are consumed
/// as one logical stream, but each side wraps at its own line width and skips
/// its own gap. Every physical row is validated before the first write.
pub fn execute_texture_copy(
    memory: &mut GpuPhysicalMemory,
    address_space: &GpuDmaAddressSpace,
    request: TextureCopyRequest,
) -> Result<TextureCopyReport, TextureCopyError> {
    if request.flags & TEXTURE_COPY_ENGINE_MODE == 0 {
        return Err(TextureCopyError::TextureCopyModeNotEnabled);
    }
    if request.source_address & 7 != 0 {
        return Err(TextureCopyError::MisalignedAddress {
            address: request.source_address,
        });
    }
    if request.destination_address & 7 != 0 {
        return Err(TextureCopyError::MisalignedAddress {
            address: request.destination_address,
        });
    }
    if request.size_bytes & 7 != 0 {
        return Err(TextureCopyError::MisalignedSize {
            size_bytes: request.size_bytes,
        });
    }

    let used_gaps = request.flags & TEXTURE_COPY_GAP_MODE != 0;
    let minimum_bytes = if used_gaps { 192 } else { 16 };
    if request.size_bytes < minimum_bytes {
        return Err(TextureCopyError::InvalidSize {
            size_bytes: request.size_bytes,
            minimum_bytes,
        });
    }
    if request.size_bytes as usize > MAX_TEXTURE_COPY_BYTES {
        return Err(TextureCopyError::TransferTooLarge {
            size_bytes: request.size_bytes,
            maximum_bytes: MAX_TEXTURE_COPY_BYTES,
        });
    }

    let size_bytes = request.size_bytes as usize;
    let (source_line_bytes, destination_line_bytes, source_gap, destination_gap) =
        if used_gaps {
            let source_line = (request.source_line_width_and_gap & 0xFFFF) as usize * 16;
            let destination_line =
                (request.destination_line_width_and_gap & 0xFFFF) as usize * 16;
            if source_line == 0 || destination_line == 0 {
                return Err(TextureCopyError::MissingLineWidth);
            }
            (
                source_line,
                destination_line,
                (request.source_line_width_and_gap >> 16) as usize * 16,
                (request.destination_line_width_and_gap >> 16) as usize * 16,
            )
        } else {
            (size_bytes, size_bytes, 0, 0)
        };

    let source_stride = source_line_bytes.checked_add(source_gap).ok_or(
        TextureCopyError::AddressOverflow {
            base: request.source_address,
            offset: u64::MAX,
        },
    )?;
    let destination_stride = destination_line_bytes.checked_add(destination_gap).ok_or(
        TextureCopyError::AddressOverflow {
            base: request.destination_address,
            offset: u64::MAX,
        },
    )?;
    let source_row_count = row_count(size_bytes, source_line_bytes);
    let destination_row_count = row_count(size_bytes, destination_line_bytes);

    // Validate every source and destination row independently before reading
    // any data or changing the destination.
    for row in 0..source_row_count {
        let count = row_size(size_bytes, source_line_bytes, row);
        let source = row_address(request.source_address, source_stride, row)?;
        resolve_texture_copy_range(memory, address_space, source, count)?;
    }
    for row in 0..destination_row_count {
        let count = row_size(size_bytes, destination_line_bytes, row);
        let destination = row_address(request.destination_address, destination_stride, row)?;
        resolve_texture_copy_range(memory, address_space, destination, count)?;
    }

    // Stage the complete logical byte stream before writing, preserving
    // deterministic behavior for overlapping source and destination ranges.
    let mut staged = Vec::new();
    staged
        .try_reserve_exact(size_bytes)
        .map_err(|_| TextureCopyError::AllocationFailed)?;
    staged.resize(size_bytes, 0);

    let mut copied = 0usize;
    for row in 0..source_row_count {
        let count = row_size(size_bytes, source_line_bytes, row);
        let source = row_address(request.source_address, source_stride, row)?;
        let physical_source = resolve_texture_copy_range(memory, address_space, source, count)?;
        memory
            .read(physical_source, &mut staged[copied..copied + count])
            .map_err(|error| TextureCopyError::PhysicalMemory(error.to_string()))?;
        copied += count;
    }

    copied = 0;
    for row in 0..destination_row_count {
        let count = row_size(size_bytes, destination_line_bytes, row);
        let destination = row_address(request.destination_address, destination_stride, row)?;
        let physical_destination =
            resolve_texture_copy_range(memory, address_space, destination, count)?;
        memory
            .write(physical_destination, &staged[copied..copied + count])
            .map_err(|error| TextureCopyError::PhysicalMemory(error.to_string()))?;
        copied += count;
    }

    Ok(TextureCopyReport {
        bytes_transferred: request.size_bytes,
        source_rows_transferred: source_row_count as u32,
        destination_rows_transferred: destination_row_count as u32,
        used_gaps,
    })
}

fn row_count(total_bytes: usize, line_bytes: usize) -> usize {
    (total_bytes + line_bytes - 1) / line_bytes
}

fn row_size(total_bytes: usize, line_bytes: usize, row: usize) -> usize {
    (total_bytes - row * line_bytes).min(line_bytes)
}

fn row_address(base: u32, stride: usize, row: usize) -> Result<u32, TextureCopyError> {
    let offset =
        (stride as u64)
            .checked_mul(row as u64)
            .ok_or(TextureCopyError::AddressOverflow {
                base,
                offset: u64::MAX,
            })?;
    let address = u64::from(base)
        .checked_add(offset)
        .ok_or(TextureCopyError::AddressOverflow { base, offset })?;
    u32::try_from(address).map_err(|_| TextureCopyError::AddressOverflow { base, offset })
}

fn resolve_texture_copy_range(
    memory: &GpuPhysicalMemory,
    address_space: &GpuDmaAddressSpace,
    address: u32,
    length: usize,
) -> Result<u32, TextureCopyError> {
    let end = u64::from(address)
        .checked_add(length as u64)
        .ok_or(TextureCopyError::InvalidAddress { address })?;
    if end > (u32::MAX as u64) + 1 {
        return Err(TextureCopyError::InvalidAddress { address });
    }

    let start = u64::from(address);
    let physical_vram_end = u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
    let alias_vram_end = u64::from(GSP_VRAM_ALIAS) + u64::from(VRAM_SIZE);
    let physical_address = if start >= u64::from(VRAM_BASE) && end <= physical_vram_end {
        address
    } else if start >= u64::from(GSP_VRAM_ALIAS) && end <= alias_vram_end {
        VRAM_BASE + (address - GSP_VRAM_ALIAS)
    } else {
        address_space
            .resolve_physical_range(memory, address, length)
            .map_err(|_| TextureCopyError::InvalidAddress { address })?
            .physical_address
    };

    memory
        .validate_range(physical_address, length)
        .map_err(|_| TextureCopyError::InvalidAddress { address })?;
    Ok(physical_address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::FCRAM_BASE;

    fn mapped_linear_memory() -> GpuDmaAddressSpace {
        let mut address_space = GpuDmaAddressSpace::new();
        address_space
            .map_linear_region(0x1400_0000, 0x1_0000)
            .unwrap();
        address_space
    }

    #[test]
    fn requires_texture_copy_engine_mode_flag() {
        let mut memory = GpuPhysicalMemory::new();
        let request = TextureCopyRequest {
            source_address: 0x1400_0000,
            destination_address: 0x1400_1000,
            size_bytes: 16,
            source_line_width_and_gap: 0,
            destination_line_width_and_gap: 0,
            flags: 0,
        };
        assert!(matches!(
            execute_texture_copy(&mut memory, &GpuDmaAddressSpace::new(), request),
            Err(TextureCopyError::TextureCopyModeNotEnabled)
        ));
    }

    #[test]
    fn contiguous_copy_transfers_raw_bytes_without_format_conversion() {
        let mut memory = GpuPhysicalMemory::new();
        let address_space = mapped_linear_memory();
        let source = 0x1400_0100;
        let destination = 0x1400_1000;
        let expected: Vec<u8> = (0..64).map(|value| (value * 3) as u8).collect();
        memory.write(FCRAM_BASE + 0x100, &expected).unwrap();

        let report = execute_texture_copy(
            &mut memory,
            &address_space,
            TextureCopyRequest {
                source_address: source,
                destination_address: destination,
                size_bytes: expected.len() as u32,
                source_line_width_and_gap: 0,
                destination_line_width_and_gap: 0,
                flags: TEXTURE_COPY_ENGINE_MODE,
            },
        )
        .unwrap();

        let mut actual = vec![0; expected.len()];
        memory.read(FCRAM_BASE + 0x1000, &mut actual).unwrap();
        assert_eq!(actual.as_slice(), expected.as_slice());
        assert_eq!(
            report,
            TextureCopyReport {
                bytes_transferred: 64,
                rows_transferred: 1,
                used_gaps: false,
            }
        );
    }

    #[test]
    fn gap_mode_copies_rows_and_skips_independent_source_and_destination_gaps() {
        let mut memory = GpuPhysicalMemory::new();
        let address_space = mapped_linear_memory();
        let source = 0x1400_2000;
        let destination = 0x1400_4000;
        let source_bytes: Vec<u8> = (0..272).map(|value| value as u8).collect();
        memory.write(FCRAM_BASE + 0x2000, &source_bytes).unwrap();
        memory.write(FCRAM_BASE + 0x4000, &[0xCC; 192]).unwrap();

        let report = execute_texture_copy(
            &mut memory,
            &address_space,
            TextureCopyRequest {
                source_address: source,
                destination_address: destination,
                size_bytes: 192,
                source_line_width_and_gap: 0x0001_0002,
                destination_line_width_and_gap: 0x0000_0002,
                flags: TEXTURE_COPY_ENGINE_MODE | TEXTURE_COPY_GAP_MODE,
            },
        )
        .unwrap();

        let mut actual = vec![0; 192];
        memory.read(FCRAM_BASE + 0x4000, &mut actual).unwrap();
        let mut expected = Vec::new();
        for row in 0..6 {
            let start = row * 48;
            expected.extend_from_slice(&source_bytes[start..start + 32]);
        }
        assert_eq!(actual, expected);
        assert_eq!(report.bytes_transferred, 192);
        assert_eq!(report.source_rows_transferred, 6);
        assert_eq!(report.destination_rows_transferred, 6);
        assert!(report.used_gaps);
    }

    #[test]
    fn validates_all_rows_before_modifying_destination() {
        let mut memory = GpuPhysicalMemory::new();
        let source = VRAM_BASE + VRAM_SIZE - 32;
        let destination = VRAM_BASE + 0x1000;
        memory.write(source, &[0x5A; 16]).unwrap();
        memory.write(destination, &[0xA5; 192]).unwrap();

        let result = execute_texture_copy(
            &mut memory,
            &GpuDmaAddressSpace::new(),
            TextureCopyRequest {
                source_address: source,
                destination_address: destination,
                size_bytes: 192,
                source_line_width_and_gap: 0x0001_0001,
                destination_line_width_and_gap: 0x0000_0001,
                flags: TEXTURE_COPY_ENGINE_MODE | TEXTURE_COPY_GAP_MODE,
            },
        );
        assert!(matches!(
            result,
            Err(TextureCopyError::InvalidAddress { .. })
        ));

        let mut actual = vec![0; 192];
        memory.read(destination, &mut actual).unwrap();
        assert_eq!(actual, [0xA5; 192]);
    }

    #[test]
    fn gap_mode_copies_different_source_and_destination_line_widths() {
        let mut memory = GpuPhysicalMemory::new();
        let address_space = mapped_linear_memory();
        let source = 0x1400_2000;
        let destination = 0x1400_4000;
        let source_line = 32usize;
        let source_gap = 16usize;
        let destination_line = 16usize;
        let destination_gap = 16usize;
        let size_bytes = 192usize;

        // Six 32-byte source rows with a 16-byte gap between rows.
        let stream: Vec<u8> = (0..size_bytes).map(|value| (value * 7) as u8).collect();
        let mut source_storage = vec![0xEE; 6 * (source_line + source_gap)];
        for row in 0..6 {
            let source_offset = row * (source_line + source_gap);
            let stream_offset = row * source_line;
            source_storage[source_offset..source_offset + source_line]
                .copy_from_slice(&stream[stream_offset..stream_offset + source_line]);
        }
        memory
            .write(FCRAM_BASE + 0x2000, &source_storage)
            .unwrap();

        let destination_storage_bytes = 12 * (destination_line + destination_gap);
        memory
            .write(FCRAM_BASE + 0x4000, &vec![0xCC; destination_storage_bytes])
            .unwrap();

        let report = execute_texture_copy(
            &mut memory,
            &address_space,
            TextureCopyRequest {
                source_address: source,
                destination_address: destination,
                size_bytes: size_bytes as u32,
                source_line_width_and_gap: 0x0001_0002,
                destination_line_width_and_gap: 0x0001_0001,
                flags: TEXTURE_COPY_ENGINE_MODE | TEXTURE_COPY_GAP_MODE,
            },
        )
        .unwrap();

        let mut actual = vec![0; destination_storage_bytes];
        memory.read(FCRAM_BASE + 0x4000, &mut actual).unwrap();
        for row in 0..12 {
            let output_offset = row * (destination_line + destination_gap);
            let stream_offset = row * destination_line;
            assert_eq!(
                &actual[output_offset..output_offset + destination_line],
                &stream[stream_offset..stream_offset + destination_line],
            );
            assert_eq!(
                &actual[output_offset + destination_line..output_offset + destination_line + destination_gap],
                &[0xCC; 16],
                "destination gap for row {row} must remain untouched"
            );
        }
        assert_eq!(report.bytes_transferred, size_bytes as u32);
        assert_eq!(report.source_rows_transferred, 6);
        assert_eq!(report.destination_rows_transferred, 12);
        assert!(report.used_gaps);
    }

    #[test]
    fn rejects_zero_and_unaligned_transfer_sizes() {
        let mut memory = GpuPhysicalMemory::new();
        let base = 0x1400_2000;
        let zero = execute_texture_copy(
            &mut memory,
            &GpuDmaAddressSpace::new(),
            TextureCopyRequest {
                source_address: base,
                destination_address: base + 0x100,
                size_bytes: 0,
                source_line_width_and_gap: 0,
                destination_line_width_and_gap: 0,
                flags: TEXTURE_COPY_ENGINE_MODE,
            },
        );
        assert!(matches!(
            zero,
            Err(TextureCopyError::InvalidSize {
                size_bytes: 0,
                minimum_bytes: 16
            })
        ));

        let unaligned = execute_texture_copy(
            &mut memory,
            &GpuDmaAddressSpace::new(),
            TextureCopyRequest {
                source_address: base,
                destination_address: base + 0x100,
                size_bytes: 17,
                source_line_width_and_gap: 0,
                destination_line_width_and_gap: 0,
                flags: TEXTURE_COPY_ENGINE_MODE,
            },
        );
        assert!(matches!(
            unaligned,
            Err(TextureCopyError::MisalignedSize { size_bytes: 17 })
        ));
    }

    #[test]
    fn source_and_destination_ranges_may_use_vram_aliases() {
        let mut memory = GpuPhysicalMemory::new();
        let source = 0x1F00_1000;
        let destination = 0x1F00_2000;
        let expected = [0x3C; 32];
        memory.write(VRAM_BASE + 0x1000, &expected).unwrap();

        execute_texture_copy(
            &mut memory,
            &GpuDmaAddressSpace::new(),
            TextureCopyRequest {
                source_address: source,
                destination_address: destination,
                size_bytes: 32,
                source_line_width_and_gap: 0,
                destination_line_width_and_gap: 0,
                flags: TEXTURE_COPY_ENGINE_MODE,
            },
        )
        .unwrap();

        let mut actual = [0; 32];
        memory.read(VRAM_BASE + 0x2000, &mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn overlapping_ranges_use_staged_copy_semantics() {
        let mut memory = GpuPhysicalMemory::new();
        let address_space = mapped_linear_memory();
        let source = 0x1400_3000;
        let expected: Vec<u8> = (0..64).map(|value| (value + 1) as u8).collect();
        memory.write(FCRAM_BASE + 0x3000, &expected).unwrap();

        execute_texture_copy(
            &mut memory,
            &address_space,
            TextureCopyRequest {
                source_address: source,
                destination_address: source + 8,
                size_bytes: 64,
                source_line_width_and_gap: 0,
                destination_line_width_and_gap: 0,
                flags: TEXTURE_COPY_ENGINE_MODE,
            },
        )
        .unwrap();

        let mut actual = [0; 64];
        memory.read(FCRAM_BASE + 0x3008, &mut actual).unwrap();
        assert_eq!(actual.as_slice(), expected.as_slice());
    }
}
