//! 3DSX-specific process layout and memory materialization.
//!
//! This module deliberately does not construct NCCH metadata or modify the
//! NCSD process implementation. It prepares a validated, page-aligned image
//! and an explicitly requested main stack for a later ARM11-session adapter.

use std::io;

use super::{ThreeDsxImage, ThreeDsxLoadedSegments};

/// Caller-supplied main stack reservation for a 3DSX process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxStackRequest {
    /// First virtual address of the stack reservation.
    pub base_address: u32,
    /// Requested stack bytes; the mapped size is rounded up to a 0x1000 page.
    pub size_bytes: u32,
}

/// One virtual mapping in a prepared 3DSX process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxProcessMemorySegment {
    /// First virtual address of the mapping.
    pub address: u32,
    /// Bytes initialized from the 3DSX file (zero for the stack).
    pub initialized_size_bytes: u32,
    /// Page-aligned size of the virtual mapping.
    pub mapped_size_bytes: u32,
    /// Whether user-mode writes are permitted.
    pub writable: bool,
    /// Whether instruction fetch is permitted.
    pub executable: bool,
}

/// Validated 3DSX virtual layout, including the implicit entry point and BSS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsxProcessLayout {
    /// The 3DSX entry point is the first byte of Code.
    pub entry_point: u32,
    /// Executable, read-only Code mapping.
    pub code: ThreeDsxProcessMemorySegment,
    /// Read-only, non-executable ROData mapping.
    pub read_only: ThreeDsxProcessMemorySegment,
    /// Writable, non-executable Data mapping, including its BSS tail.
    pub data: ThreeDsxProcessMemorySegment,
    /// Start of BSS within the Data mapping, before page padding.
    pub bss_address: u64,
    /// Exact BSS size declared by the 3DSX header.
    pub bss_size_bytes: u32,
    /// Writable, non-executable main-thread stack mapping.
    pub stack: ThreeDsxProcessMemorySegment,
    /// Initial stack pointer at the top of the mapped stack.
    pub initial_stack_pointer: u32,
    /// Sum of page-aligned Code, ROData, Data and stack reservations.
    pub total_mapped_size_bytes: u64,
}

/// Materialized memory backing for a validated 3DSX process layout.
///
/// Code, ROData and Data include zero-filled page padding. The Data buffer
/// contains initialized bytes, then BSS, then page padding. The Stack is
/// fully zero-initialized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsxPreparedProcess {
    pub layout: ThreeDsxProcessLayout,
    pub code: Vec<u8>,
    pub read_only: Vec<u8>,
    pub data: Vec<u8>,
    pub stack: Vec<u8>,
    /// Immutable copy of the prepared layout, used to detect post-relocation address edits.
    validation_layout: ThreeDsxProcessLayout,
}

impl ThreeDsxPreparedProcess {
    /// Revalidates the public layout and buffers before they are installed in a process address space.
    ///
    /// The fields of this type are public so loaders and frontends can inspect
    /// the prepared image. Callers must not treat that as a guarantee that the
    /// values remained unchanged after preparation. This method checks segment
    /// sizes, addresses, permissions, overlap, BSS placement, entry point,
    /// stack pointer and zero-filled regions as one atomic precondition.
    pub fn validate(&self) -> io::Result<()> {
        if self.layout != self.validation_layout {
            return Err(invalid_data(
                "3DSX process layout changed after relocation and preparation",
            ));
        }
        let layout = &self.layout;
        let code_end = validate_prepared_segment(
            "Code",
            layout.code,
            &self.code,
            false,
            true,
            false,
        )?;
        let read_only_end = validate_prepared_segment(
            "ROData",
            layout.read_only,
            &self.read_only,
            false,
            false,
            true,
        )?;
        let data_end = validate_prepared_segment(
            "Data",
            layout.data,
            &self.data,
            true,
            false,
            true,
        )?;
        let stack_end = validate_prepared_segment(
            "Stack",
            layout.stack,
            &self.stack,
            true,
            false,
            false,
        )?;

        if layout.code.initialized_size_bytes == 0 {
            return Err(invalid_data("3DSX Code segment is empty"));
        }
        if layout.entry_point != layout.code.address
            || u64::from(layout.entry_point) >= code_end
        {
            return Err(invalid_data(
                "3DSX entry point is outside the executable Code mapping",
            ));
        }
        if layout.stack.initialized_size_bytes != 0
            || stack_end > u64::from(u32::MAX)
            || u64::from(layout.initial_stack_pointer) != stack_end
            || layout.initial_stack_pointer % 8 != 0
        {
            return Err(invalid_data(
                "3DSX initial stack pointer does not match the aligned stack top",
            ));
        }

        let expected_bss_address = u64::from(layout.data.address)
            .checked_add(u64::from(layout.data.initialized_size_bytes))
            .ok_or_else(|| invalid_data("3DSX BSS address overflows"))?;
        if layout.bss_address != expected_bss_address {
            return Err(invalid_data(
                "3DSX BSS does not immediately follow initialized Data",
            ));
        }
        let bss_end = layout
            .bss_address
            .checked_add(u64::from(layout.bss_size_bytes))
            .ok_or_else(|| invalid_data("3DSX BSS range overflows"))?;
        if bss_end > data_end {
            return Err(invalid_data(
                "3DSX BSS extends beyond the Data memory mapping",
            ));
        }

        let mut ranges = [
            (u64::from(layout.code.address), code_end, "Code"),
            (
                u64::from(layout.read_only.address),
                read_only_end,
                "ROData",
            ),
            (u64::from(layout.data.address), data_end, "Data"),
            (u64::from(layout.stack.address), stack_end, "Stack"),
        ];
        ranges.sort_unstable_by_key(|range| range.0);
        let mut previous: Option<(u64, &str)> = None;
        for (start, end, name) in ranges {
            if start == end {
                continue;
            }
            if let Some((previous_end, previous_name)) = previous {
                if start < previous_end {
                    return Err(invalid_data(format!(
                        "3DSX {previous_name} and {name} mappings overlap",
                    )));
                }
            }
            previous = Some((end, name));
        }

        let expected_total = [
            layout.code.mapped_size_bytes,
            layout.read_only.mapped_size_bytes,
            layout.data.mapped_size_bytes,
            layout.stack.mapped_size_bytes,
        ]
        .into_iter()
        .try_fold(0u64, |total, size| total.checked_add(u64::from(size)))
        .ok_or_else(|| invalid_data("3DSX mapped memory total overflows"))?;
        if layout.total_mapped_size_bytes != expected_total {
            return Err(invalid_data(
                "3DSX mapped memory total does not match its segments",
            ));
        }

        ensure_zeroed_tail("Code", &self.code, layout.code.initialized_size_bytes)?;
        ensure_zeroed_tail(
            "ROData",
            &self.read_only,
            layout.read_only.initialized_size_bytes,
        )?;
        ensure_zeroed_tail("Data", &self.data, layout.data.initialized_size_bytes)?;
        ensure_zeroed_tail("Stack", &self.stack, 0)?;

        Ok(())
    }
}

fn validate_prepared_segment(
    name: &'static str,
    segment: ThreeDsxProcessMemorySegment,
    bytes: &[u8],
    writable: bool,
    executable: bool,
    allow_empty: bool,
) -> io::Result<u64> {
    if u64::from(segment.address) % super::PAGE_SIZE != 0 {
        return Err(invalid_data(format!(
            "3DSX {name} base is not page-aligned",
        )));
    }
    if u64::from(segment.mapped_size_bytes) % super::PAGE_SIZE != 0 {
        return Err(invalid_data(format!(
            "3DSX {name} mapping size is not page-aligned",
        )));
    }
    if !allow_empty && segment.mapped_size_bytes == 0 {
        return Err(invalid_data(format!("3DSX {name} mapping is empty")));
    }
    if segment.initialized_size_bytes > segment.mapped_size_bytes {
        return Err(invalid_data(format!(
            "3DSX {name} initialized size exceeds its mapping",
        )));
    }
    if bytes.len() != segment.mapped_size_bytes as usize {
        return Err(invalid_data(format!(
            "3DSX {name} buffer has {} bytes, expected {}",
            bytes.len(),
            segment.mapped_size_bytes,
        )));
    }
    if segment.writable != writable || segment.executable != executable {
        return Err(invalid_data(format!(
            "3DSX {name} mapping permissions are invalid",
        )));
    }

    let end = u64::from(segment.address)
        .checked_add(u64::from(segment.mapped_size_bytes))
        .ok_or_else(|| invalid_data(format!("3DSX {name} address range overflows")))?;
    if end > 0x1_0000_0000 {
        return Err(invalid_data(format!(
            "3DSX {name} mapping exceeds the 32-bit address space",
        )));
    }
    Ok(end)
}

fn ensure_zeroed_tail(
    name: &'static str,
    bytes: &[u8],
    initialized_size_bytes: u32,
) -> io::Result<()> {
    let initialized_size = initialized_size_bytes as usize;
    let tail = bytes
        .get(initialized_size..)
        .ok_or_else(|| invalid_data(format!("3DSX {name} initialized size exceeds its buffer")))?;
    if tail.iter().any(|byte| *byte != 0) {
        return Err(invalid_data(format!(
            "3DSX {name} zero-filled tail contains non-zero bytes",
        )));
    }
    Ok(())
}

impl ThreeDsxImage {
    /// Creates a page-aligned process image from loaded and (when present) relocated segments.
    ///
    /// The caller must run apply_relocations before this method when relocation
    /// tables are present. Virtual bases and the stack reservation are supplied
    /// explicitly; no NCCH header, memory type, stack size or process metadata
    /// is fabricated for the 3DSX format.
    pub fn prepare_process(
        &self,
        loaded: ThreeDsxLoadedSegments,
        segment_bases: [u32; 3],
        stack_request: ThreeDsxStackRequest,
        maximum_memory_size_bytes: u64,
    ) -> io::Result<ThreeDsxPreparedProcess> {
        super::validate_loaded_segments(self, &loaded)?;
        let has_relocations = self
            .relocation_headers
            .iter()
            .any(|header| header.absolute_entries != 0 || header.relative_entries != 0);
        if has_relocations && !loaded.relocations_applied {
            return Err(invalid_data(
                "3DSX relocation tables must be applied before process preparation",
            ));
        }

        let [code_size, read_only_size, data_size] = [
            super::align_to_page(self.code.memory_size_bytes)?,
            super::align_to_page(self.rodata.memory_size_bytes)?,
            super::align_to_page(self.data.memory_size_bytes)?,
        ];
        let segment_sizes = [code_size, read_only_size, data_size];
        super::validate_segment_bases(segment_bases, segment_sizes)?;

        if stack_request.size_bytes == 0 {
            return Err(invalid_data("3DSX main stack size must be non-zero"));
        }
        if u64::from(stack_request.base_address) % super::PAGE_SIZE != 0 {
            return Err(invalid_data("3DSX main stack base is not page-aligned"));
        }
        let stack_size = super::align_to_page(u64::from(stack_request.size_bytes))?;
        let stack_end = u64::from(stack_request.base_address)
            .checked_add(stack_size)
            .ok_or_else(|| invalid_data("3DSX main stack address range overflows"))?;
        if stack_end > u32::MAX as u64 {
            return Err(invalid_data(
                "3DSX main stack top does not fit the ARM11 stack-pointer register",
            ));
        }
        for index in 0..3 {
            let segment_start = u64::from(segment_bases[index]);
            let segment_end = segment_start
                .checked_add(segment_sizes[index])
                .ok_or_else(|| invalid_data("3DSX segment address range overflows"))?;
            let stack_start = u64::from(stack_request.base_address);
            if segment_sizes[index] != 0 && stack_start < segment_end && segment_start < stack_end {
                return Err(invalid_data(format!(
                    "3DSX main stack overlaps segment {index}"
                )));
            }
        }

        let total_mapped_size_bytes = segment_sizes
            .into_iter()
            .chain(std::iter::once(stack_size))
            .try_fold(0u64, |total, size| total.checked_add(size))
            .ok_or_else(|| invalid_data("3DSX total process memory size overflows"))?;
        if total_mapped_size_bytes > maximum_memory_size_bytes {
            return Err(invalid_data(format!(
                "3DSX process requires {total_mapped_size_bytes} bytes, exceeding the {maximum_memory_size_bytes}-byte limit"
            )));
        }

        let mut code = super::allocate_zeroed_bytes(code_size, "mapped Code")?;
        let mut read_only = super::allocate_zeroed_bytes(read_only_size, "mapped ROData")?;
        let mut data = super::allocate_zeroed_bytes(data_size, "mapped Data")?;
        let stack = super::allocate_zeroed_bytes(stack_size, "main stack")?;

        code[..loaded.code.len()].copy_from_slice(&loaded.code);
        read_only[..loaded.rodata.len()].copy_from_slice(&loaded.rodata);
        data[..loaded.data.len()].copy_from_slice(&loaded.data);
        let bss_start = loaded.data.len();
        let bss_end = bss_start
            .checked_add(loaded.bss.len())
            .ok_or_else(|| invalid_data("3DSX BSS destination range overflows"))?;
        data.get_mut(bss_start..bss_end)
            .ok_or_else(|| invalid_data("3DSX BSS does not fit the mapped Data segment"))?
            .copy_from_slice(&loaded.bss);

        let code_initialized_size = u32::try_from(self.code.file_size_bytes)
            .map_err(|_| invalid_data("3DSX Code size does not fit u32"))?;
        let read_only_initialized_size = u32::try_from(self.rodata.file_size_bytes)
            .map_err(|_| invalid_data("3DSX ROData size does not fit u32"))?;
        let data_initialized_size = u32::try_from(self.data.file_size_bytes)
            .map_err(|_| invalid_data("3DSX Data size does not fit u32"))?;
        let code_mapped_size = u32::try_from(code_size)
            .map_err(|_| invalid_data("3DSX Code mapping does not fit u32"))?;
        let read_only_mapped_size = u32::try_from(read_only_size)
            .map_err(|_| invalid_data("3DSX ROData mapping does not fit u32"))?;
        let data_mapped_size = u32::try_from(data_size)
            .map_err(|_| invalid_data("3DSX Data mapping does not fit u32"))?;
        let stack_mapped_size = u32::try_from(stack_size)
            .map_err(|_| invalid_data("3DSX stack mapping does not fit u32"))?;
        let bss_address = u64::from(segment_bases[2])
            .checked_add(self.data.file_size_bytes)
            .ok_or_else(|| invalid_data("3DSX BSS virtual address overflows"))?;

        let layout = ThreeDsxProcessLayout {
            entry_point: segment_bases[0],
            code: ThreeDsxProcessMemorySegment {
                address: segment_bases[0],
                initialized_size_bytes: code_initialized_size,
                mapped_size_bytes: code_mapped_size,
                writable: false,
                executable: true,
            },
            read_only: ThreeDsxProcessMemorySegment {
                address: segment_bases[1],
                initialized_size_bytes: read_only_initialized_size,
                mapped_size_bytes: read_only_mapped_size,
                writable: false,
                executable: false,
            },
            data: ThreeDsxProcessMemorySegment {
                address: segment_bases[2],
                initialized_size_bytes: data_initialized_size,
                mapped_size_bytes: data_mapped_size,
                writable: true,
                executable: false,
            },
            bss_address,
            bss_size_bytes: self.header.bss_size_bytes,
            stack: ThreeDsxProcessMemorySegment {
                address: stack_request.base_address,
                initialized_size_bytes: 0,
                mapped_size_bytes: stack_mapped_size,
                writable: true,
                executable: false,
            },
            initial_stack_pointer: stack_end as u32,
            total_mapped_size_bytes,
        };
        let prepared = ThreeDsxPreparedProcess {
            validation_layout: layout.clone(),
            layout,
            code,
            read_only,
            data,
            stack,
        };
        prepared.validate()?;
        Ok(prepared)
    }
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn minimal_image() -> Vec<u8> {
        let mut image = vec![0u8; 68];
        image[..4].copy_from_slice(b"3DSX");
        image[4..6].copy_from_slice(&32u16.to_le_bytes());
        image[6..8].copy_from_slice(&8u16.to_le_bytes());
        image[16..20].copy_from_slice(&4u32.to_le_bytes());
        image[20..24].copy_from_slice(&4u32.to_le_bytes());
        image[24..28].copy_from_slice(&8u32.to_le_bytes());
        image[28..32].copy_from_slice(&4u32.to_le_bytes());
        image[32..56].fill(0);
        image[56..60].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        image[60..64].copy_from_slice(&[0x55, 0x66, 0x77, 0x88]);
        image[64..68].copy_from_slice(&[0x99, 0xAA, 0xBB, 0xCC]);
        image
    }

    fn relocatable_image() -> Vec<u8> {
        let mut image = minimal_image();
        image[32..36].copy_from_slice(&1u32.to_le_bytes());
        image[56..60].copy_from_slice(&0x0000_1000u32.to_le_bytes());
        image.extend_from_slice(&0u16.to_le_bytes());
        image.extend_from_slice(&1u16.to_le_bytes());
        image
    }

    fn prepare(
        stack: ThreeDsxStackRequest,
        maximum_memory_size_bytes: u64,
    ) -> io::Result<ThreeDsxPreparedProcess> {
        let image = minimal_image();
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image))?;
        let loaded = parsed.load_segments(&mut Cursor::new(&image), 16)?;
        parsed.prepare_process(
            loaded,
            [0x0010_0000, 0x0020_0000, 0x0030_0000],
            stack,
            maximum_memory_size_bytes,
        )
    }

    #[test]
    fn builds_page_aligned_memory_and_zeros_bss_padding_and_stack() {
        let prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();

        assert_eq!(prepared.layout.entry_point, 0x0010_0000);
        assert_eq!(prepared.layout.code.mapped_size_bytes, 0x1000);
        assert!(prepared.layout.code.executable);
        assert!(!prepared.layout.code.writable);
        assert_eq!(prepared.layout.read_only.address, 0x0020_0000);
        assert!(!prepared.layout.read_only.executable);
        assert_eq!(prepared.layout.data.address, 0x0030_0000);
        assert_eq!(prepared.layout.bss_address, 0x0030_0004);
        assert_eq!(prepared.layout.bss_size_bytes, 4);
        assert_eq!(prepared.layout.initial_stack_pointer, 0x0400_2000);
        assert_eq!(prepared.layout.total_mapped_size_bytes, 0x5000);

        assert_eq!(&prepared.code[..4], &[0x11, 0x22, 0x33, 0x44]);
        assert!(prepared.code[4..].iter().all(|byte| *byte == 0));
        assert_eq!(&prepared.read_only[..4], &[0x55, 0x66, 0x77, 0x88]);
        assert!(prepared.read_only[4..].iter().all(|byte| *byte == 0));
        assert_eq!(&prepared.data[..4], &[0x99, 0xAA, 0xBB, 0xCC]);
        assert!(prepared.data[4..].iter().all(|byte| *byte == 0));
        assert!(prepared.stack.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn prepared_segments_preserve_the_complete_3dsx_mapping_contract() {
        let prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();

        prepared.validate().unwrap();
        let layout = &prepared.layout;
        assert_eq!(prepared.code.len(), layout.code.mapped_size_bytes as usize);
        assert_eq!(
            prepared.read_only.len(),
            layout.read_only.mapped_size_bytes as usize
        );
        assert_eq!(prepared.data.len(), layout.data.mapped_size_bytes as usize);
        assert_eq!(
            prepared.stack.len(),
            layout.stack.mapped_size_bytes as usize
        );

        assert!(layout.code.executable);
        assert!(!layout.code.writable);
        assert!(!layout.read_only.executable);
        assert!(!layout.read_only.writable);
        assert!(!layout.data.executable);
        assert!(layout.data.writable);
        assert!(!layout.stack.executable);
        assert!(layout.stack.writable);

        assert_eq!(
            layout.bss_address,
            u64::from(layout.data.address) + u64::from(layout.data.initialized_size_bytes)
        );
        let bss_end = layout.bss_address + u64::from(layout.bss_size_bytes);
        let data_end = u64::from(layout.data.address) + u64::from(layout.data.mapped_size_bytes);
        assert!(bss_end <= data_end);
        assert_eq!(
            layout.total_mapped_size_bytes,
            u64::from(layout.code.mapped_size_bytes)
                + u64::from(layout.read_only.mapped_size_bytes)
                + u64::from(layout.data.mapped_size_bytes)
                + u64::from(layout.stack.mapped_size_bytes)
        );
        assert!(
            prepared.data[layout.data.initialized_size_bytes as usize..]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn rejects_prepared_process_with_modified_buffer_length() {
        let mut prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();
        prepared.code.pop();

        assert_eq!(
            prepared.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_prepared_process_when_relocated_segment_bases_are_changed() {
        let mut prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();
        prepared.layout.code.address += 0x1000;
        prepared.layout.entry_point += 0x1000;

        assert_eq!(
            prepared.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_prepared_process_with_modified_permissions() {
        let mut prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();
        prepared.layout.code.writable = true;

        assert_eq!(
            prepared.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_prepared_process_with_bss_outside_data() {
        let mut prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();
        prepared.layout.bss_address += 4;

        assert_eq!(
            prepared.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_prepared_process_with_nonzero_bss_or_padding() {
        let mut prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();
        prepared.data[4] = 0xA5;

        assert_eq!(
            prepared.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_prepared_process_with_overlapping_stack() {
        let mut prepared = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x5000,
        )
        .unwrap();
        prepared.layout.stack.address = prepared.layout.data.address;
        prepared.layout.initial_stack_pointer =
            prepared.layout.data.address + prepared.layout.data.mapped_size_bytes;

        assert_eq!(
            prepared.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn requires_relocations_before_preparing_a_process() {
        let image = relocatable_image();
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();

        let error = parsed
            .prepare_process(
                loaded,
                [0x0010_0000, 0x0020_0000, 0x0030_0000],
                ThreeDsxStackRequest {
                    base_address: 0x0400_0000,
                    size_bytes: 0x2000,
                },
                0x5000,
            )
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn prepares_relocated_image_and_rejects_a_second_relocation_pass() {
        let image = relocatable_image();
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        let bases = [0x0010_0000, 0x0020_0000, 0x0030_0000];

        assert_eq!(
            parsed
                .apply_relocations(&mut Cursor::new(&image), &mut loaded, bases, 1)
                .unwrap(),
            1
        );
        assert_eq!(
            parsed
                .apply_relocations(&mut Cursor::new(&image), &mut loaded, bases, 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let prepared = parsed
            .prepare_process(
                loaded,
                bases,
                ThreeDsxStackRequest {
                    base_address: 0x0400_0000,
                    size_bytes: 0x2000,
                },
                0x5000,
            )
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(prepared.code[..4].try_into().unwrap()),
            0x0020_0000
        );
    }

    #[test]
    fn rejects_stack_overlap_with_program_segment() {
        let error = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0010_0000,
                size_bytes: 0x1000,
            },
            0x5000,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_stack_memory_above_caller_limit() {
        let error = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0000,
                size_bytes: 0x2000,
            },
            0x4FFF,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_unaligned_stack_and_unrepresentable_stack_pointer() {
        let unaligned = prepare(
            ThreeDsxStackRequest {
                base_address: 0x0400_0001,
                size_bytes: 0x1000,
            },
            0x5000,
        )
        .unwrap_err();
        assert_eq!(unaligned.kind(), io::ErrorKind::InvalidData);

        let unrepresentable = prepare(
            ThreeDsxStackRequest {
                base_address: 0xFFFF_F000,
                size_bytes: 0x2000,
            },
            0x6000,
        )
        .unwrap_err();
        assert_eq!(unrepresentable.kind(), io::ErrorKind::InvalidData);
    }
}
