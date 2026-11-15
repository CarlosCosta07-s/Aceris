//! Structural parser for Nintendo 3DS homebrew executable files.
//!
//! This parser validates the standard 3DSX container layout. It does not
//! relocate segments, create an ARM11 process, or claim executable support.

use std::io::{self, Read, Seek, SeekFrom};

pub const HEADER_SIZE: usize = 0x20;
const EXTENDED_HEADER_SIZE: usize = 0x2C;
const STANDARD_RELOCATION_HEADER_SIZE: u16 = 8;
const SEGMENT_COUNT: usize = 3;
const WORD_SIZE: u64 = 4;
const PAGE_SIZE: u64 = 0x1000;
const RELOCATION_OFFSET_MASK: u32 = 0x0FFF_FFFF;
const MAX_RELOCATION_IMAGE_SIZE_BYTES: u64 = 0x1000_0000;

/// Fixed fields from a 3DSX header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxHeader {
    pub header_size_bytes: u16,
    pub relocation_header_size_bytes: u16,
    pub format_version: u32,
    pub flags: u32,
    pub code_size_bytes: u32,
    pub rodata_size_bytes: u32,
    pub data_size_bytes: u32,
    pub bss_size_bytes: u32,
}

/// Optional offsets declared by an extended 3DSX header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxExtendedHeader {
    pub smdh_offset_bytes: u32,
    pub smdh_size_bytes: u32,
    pub romfs_offset_bytes: u32,
}

/// File-backed and in-memory sizes for one loaded segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreeDsxSegment {
    pub offset_bytes: u64,
    pub file_size_bytes: u64,
    pub memory_size_bytes: u64,
}

/// Counts of the two standard relocation tables for one segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxRelocationHeader {
    pub absolute_entries: u32,
    pub relative_entries: u32,
}

/// One bounded relocation table in the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxRelocationTable {
    pub offset_bytes: u64,
    pub entry_count: u32,
}

impl ThreeDsxRelocationTable {
    pub const fn size_bytes(self) -> u64 {
        self.entry_count as u64 * WORD_SIZE
    }
}

/// Standard absolute and relative relocation tables for one segment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxSegmentRelocations {
    pub absolute: ThreeDsxRelocationTable,
    pub relative: ThreeDsxRelocationTable,
}

/// Word positions referenced by the two standard relocation tables of one segment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreeDsxSegmentRelocationWordOffsets {
    /// Word indices named by the absolute relocation table.
    pub absolute: Vec<u32>,
    /// Word indices named by the relative relocation table.
    pub relative: Vec<u32>,
}

/// Structurally validated 3DSX layout.
///
/// The code segment starts at the executable entry point. The data segment's
/// memory size includes BSS, while its file size excludes the uninitialized
/// BSS tail. Relocation records are described but not applied by this parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsxImage {
    pub header: ThreeDsxHeader,
    pub extended_header: Option<ThreeDsxExtendedHeader>,
    pub relocation_headers: [ThreeDsxRelocationHeader; SEGMENT_COUNT],
    pub code: ThreeDsxSegment,
    pub rodata: ThreeDsxSegment,
    pub data: ThreeDsxSegment,
    pub relocations: [ThreeDsxSegmentRelocations; SEGMENT_COUNT],
    pub trailing_data_offset_bytes: u64,
}

/// Materialized 3DSX segments, with the uninitialized data tail zero-filled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreeDsxLoadedSegments {
    /// File-backed executable code bytes.
    pub code: Vec<u8>,
    /// File-backed read-only data bytes.
    pub rodata: Vec<u8>,
    /// Initialized writable data bytes, excluding BSS.
    pub data: Vec<u8>,
    /// Zero-filled BSS tail of the writable data segment.
    pub bss: Vec<u8>,
}

impl ThreeDsxImage {
    /// Parses and bounds-checks a 3DSX file from a seekable reader.
    ///
    /// The currently supported relocation-header layout is the standard pair
    /// of u32 counts (8 bytes) for each of the three segments. Unknown extended
    /// header bytes are skipped, but partial extended headers are rejected.
    pub fn parse_reader<R: Read + Seek>(reader: &mut R) -> io::Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;

        let mut fixed = [0u8; HEADER_SIZE];
        reader.read_exact(&mut fixed)?;
        if &fixed[..4] != b"3DSX" {
            return Err(invalid_data("3DSX magic is missing"));
        }

        let header = ThreeDsxHeader {
            header_size_bytes: read_u16(&fixed, 0x04),
            relocation_header_size_bytes: read_u16(&fixed, 0x06),
            format_version: read_u32(&fixed, 0x08),
            flags: read_u32(&fixed, 0x0C),
            code_size_bytes: read_u32(&fixed, 0x10),
            rodata_size_bytes: read_u32(&fixed, 0x14),
            data_size_bytes: read_u32(&fixed, 0x18),
            bss_size_bytes: read_u32(&fixed, 0x1C),
        };

        let header_size = u64::from(header.header_size_bytes);
        if header_size < HEADER_SIZE as u64 {
            return Err(invalid_data("3DSX header is shorter than 0x20 bytes"));
        }
        if header_size > file_len {
            return Err(invalid_data("3DSX header extends beyond the file"));
        }
        if header_size > HEADER_SIZE as u64 && header_size < EXTENDED_HEADER_SIZE as u64 {
            return Err(invalid_data(
                "3DSX extended header is shorter than its 12-byte field block",
            ));
        }
        if header.relocation_header_size_bytes != STANDARD_RELOCATION_HEADER_SIZE {
            return Err(invalid_data(
                "3DSX relocation headers must use the standard 8-byte layout",
            ));
        }
        if header.code_size_bytes == 0 {
            return Err(invalid_data("3DSX code segment is empty"));
        }
        if header.bss_size_bytes > header.data_size_bytes {
            return Err(invalid_data("3DSX BSS size exceeds the data segment size"));
        }

        let extended_header = if header_size >= EXTENDED_HEADER_SIZE as u64 {
            reader.seek(SeekFrom::Start(HEADER_SIZE as u64))?;
            let mut bytes = [0u8; EXTENDED_HEADER_SIZE - HEADER_SIZE];
            reader.read_exact(&mut bytes)?;
            Some(ThreeDsxExtendedHeader {
                smdh_offset_bytes: read_u32(&bytes, 0),
                smdh_size_bytes: read_u32(&bytes, 4),
                romfs_offset_bytes: read_u32(&bytes, 8),
            })
        } else {
            None
        };

        let relocation_headers_start = header_size;
        let relocation_headers_end = checked_file_end(
            relocation_headers_start,
            (SEGMENT_COUNT as u64)
                .checked_mul(u64::from(header.relocation_header_size_bytes))
                .ok_or_else(|| invalid_data("3DSX relocation-header range overflows"))?,
            file_len,
            "relocation headers",
        )?;

        reader.seek(SeekFrom::Start(relocation_headers_start))?;
        let mut relocation_headers = [ThreeDsxRelocationHeader::default(); SEGMENT_COUNT];
        for entry in &mut relocation_headers {
            let mut bytes = [0u8; STANDARD_RELOCATION_HEADER_SIZE as usize];
            reader.read_exact(&mut bytes)?;
            entry.absolute_entries = read_u32(&bytes, 0);
            entry.relative_entries = read_u32(&bytes, 4);
        }

        let code_file_size = u64::from(header.code_size_bytes);
        let rodata_file_size = u64::from(header.rodata_size_bytes);
        let data_file_size = u64::from(header.data_size_bytes - header.bss_size_bytes);
        let code_end = checked_file_end(
            relocation_headers_end,
            code_file_size,
            file_len,
            "code segment",
        )?;
        let rodata_end = checked_file_end(code_end, rodata_file_size, file_len, "rodata segment")?;
        let data_end = checked_file_end(rodata_end, data_file_size, file_len, "data segment")?;

        let code = ThreeDsxSegment {
            offset_bytes: relocation_headers_end,
            file_size_bytes: code_file_size,
            memory_size_bytes: code_file_size,
        };
        let rodata = ThreeDsxSegment {
            offset_bytes: code_end,
            file_size_bytes: rodata_file_size,
            memory_size_bytes: rodata_file_size,
        };
        let data = ThreeDsxSegment {
            offset_bytes: rodata_end,
            file_size_bytes: data_file_size,
            memory_size_bytes: u64::from(header.data_size_bytes),
        };

        let mut relocation_cursor = data_end;
        let mut relocations = [ThreeDsxSegmentRelocations::default(); SEGMENT_COUNT];
        for (index, counts) in relocation_headers.iter().enumerate() {
            let absolute_size = u64::from(counts.absolute_entries)
                .checked_mul(WORD_SIZE)
                .ok_or_else(|| invalid_data("3DSX absolute relocation table overflows"))?;
            let absolute_end = checked_file_end(
                relocation_cursor,
                absolute_size,
                file_len,
                "absolute relocation table",
            )?;
            let relative_size = u64::from(counts.relative_entries)
                .checked_mul(WORD_SIZE)
                .ok_or_else(|| invalid_data("3DSX relative relocation table overflows"))?;
            let relative_end = checked_file_end(
                absolute_end,
                relative_size,
                file_len,
                "relative relocation table",
            )?;

            relocations[index] = ThreeDsxSegmentRelocations {
                absolute: ThreeDsxRelocationTable {
                    offset_bytes: relocation_cursor,
                    entry_count: counts.absolute_entries,
                },
                relative: ThreeDsxRelocationTable {
                    offset_bytes: absolute_end,
                    entry_count: counts.relative_entries,
                },
            };
            relocation_cursor = relative_end;
        }

        let segment_file_sizes = [
            code.file_size_bytes,
            rodata.file_size_bytes,
            data.file_size_bytes,
        ];
        for (index, segment_file_size_bytes) in segment_file_sizes.into_iter().enumerate() {
            validate_relocation_table(
                reader,
                relocations[index].absolute,
                segment_file_size_bytes,
            )?;
            validate_relocation_table(
                reader,
                relocations[index].relative,
                segment_file_size_bytes,
            )?;
        }

        validate_extended_sections(extended_header, relocation_cursor, file_len)?;

        Ok(Self {
            header,
            extended_header,
            relocation_headers,
            code,
            rodata,
            data,
            relocations,
            trailing_data_offset_bytes: relocation_cursor,
        })
    }

    /// Expands relocation skip/patch runs into word indices for all three segments.
    ///
    /// This method only decodes table positions. It does not interpret the
    /// relocated words, calculate target addresses, or apply patches. The
    /// caller provides a global cap to prevent hostile files from requesting
    /// an unbounded amount of host memory.
    pub fn read_relocation_word_offsets<R: Read + Seek>(
        &self,
        reader: &mut R,
        maximum_patched_words: usize,
    ) -> io::Result<[ThreeDsxSegmentRelocationWordOffsets; SEGMENT_COUNT]> {
        let segments = [self.code, self.rodata, self.data];
        let mut total_patched_words = 0usize;
        let mut offsets = std::array::from_fn(|_| ThreeDsxSegmentRelocationWordOffsets::default());

        for (index, segment) in segments.into_iter().enumerate() {
            let tables = self.relocations[index];
            offsets[index].absolute = read_relocation_word_offsets(
                reader,
                tables.absolute,
                segment.file_size_bytes,
                maximum_patched_words.saturating_sub(total_patched_words),
            )?;
            total_patched_words = total_patched_words
                .checked_add(offsets[index].absolute.len())
                .ok_or_else(|| invalid_data("3DSX relocation patch count overflows"))?;

            offsets[index].relative = read_relocation_word_offsets(
                reader,
                tables.relative,
                segment.file_size_bytes,
                maximum_patched_words.saturating_sub(total_patched_words),
            )?;
            total_patched_words = total_patched_words
                .checked_add(offsets[index].relative.len())
                .ok_or_else(|| invalid_data("3DSX relocation patch count overflows"))?;
        }

        Ok(offsets)
    }

    /// Applies the standard absolute and relative relocation tables to loaded segments.
    ///
    /// Segment bases are supplied by the caller and must be page-aligned,
    /// non-overlapping 32-bit virtual ranges. Image offsets in relocation words
    /// address the page-rounded Code, ROData and Data mappings in that order.
    /// The Data mapping includes its BSS tail, so relocations may point into
    /// BSS, but relocation *sites* must always be backed by file bytes.
    ///
    /// Absolute relocations accept subtype 0. Relative relocations accept
    /// subtype 0 (signed 32-bit place-relative) and subtype 1 (signed PREL31).
    /// Unknown subtypes and overlapping absolute/relative patch sites are
    /// rejected. The loaded segment buffers are modified only after all
    /// layout, table-offset and subtype checks succeed.
    pub fn apply_relocations<R: Read + Seek>(
        &self,
        reader: &mut R,
        loaded: &mut ThreeDsxLoadedSegments,
        segment_bases: [u32; SEGMENT_COUNT],
        maximum_patched_words: usize,
    ) -> io::Result<usize> {
        validate_loaded_segments(self, loaded)?;

        let segment_memory_sizes = [
            self.code.memory_size_bytes,
            self.rodata.memory_size_bytes,
            self.data.memory_size_bytes,
        ];
        let [code_aligned_size, rodata_aligned_size, data_aligned_size] =
            segment_memory_sizes.map(align_to_page);
        let aligned_sizes = [code_aligned_size?, rodata_aligned_size?, data_aligned_size?];
        validate_segment_bases(segment_bases, aligned_sizes)?;

        let total_image_size = aligned_sizes
            .into_iter()
            .try_fold(0u64, |total, size| total.checked_add(size))
            .ok_or_else(|| invalid_data("3DSX aligned image size overflows"))?;
        if total_image_size > MAX_RELOCATION_IMAGE_SIZE_BYTES {
            return Err(invalid_data(
                "3DSX aligned image exceeds the 28-bit relocation offset space",
            ));
        }

        let offsets = self.read_relocation_word_offsets(reader, maximum_patched_words)?;
        for segment_offsets in &offsets {
            if segment_offsets
                .absolute
                .iter()
                .any(|offset| segment_offsets.relative.binary_search(offset).is_ok())
            {
                return Err(invalid_data(
                    "3DSX word is listed in both absolute and relative relocation tables",
                ));
            }
        }

        let source_segments: [&[u8]; SEGMENT_COUNT] = [&loaded.code, &loaded.rodata, &loaded.data];
        let relocation_count = offsets
            .iter()
            .try_fold(0usize, |total, tables| {
                total
                    .checked_add(tables.absolute.len())
                    .and_then(|count| count.checked_add(tables.relative.len()))
            })
            .ok_or_else(|| invalid_data("3DSX patched-word count overflows"))?;
        let mut patches = Vec::new();
        patches
            .try_reserve_exact(relocation_count)
            .map_err(|error| {
                io::Error::other(format!("could not allocate 3DSX patch list: {error}"))
            })?;

        for segment_index in 0..SEGMENT_COUNT {
            prepare_relocation_words(
                source_segments[segment_index],
                segment_index,
                &offsets[segment_index].absolute,
                RelocationTableKind::Absolute,
                segment_bases,
                aligned_sizes,
                &mut patches,
            )?;
            prepare_relocation_words(
                source_segments[segment_index],
                segment_index,
                &offsets[segment_index].relative,
                RelocationTableKind::Relative,
                segment_bases,
                aligned_sizes,
                &mut patches,
            )?;
        }

        // All metadata, sites, targets and subtypes have been validated. The
        // following writes therefore cannot fail halfway through relocation.
        for (segment_index, byte_offset, relocated_word) in &patches {
            let target = match segment_index {
                0 => &mut loaded.code,
                1 => &mut loaded.rodata,
                2 => &mut loaded.data,
                _ => unreachable!("relocation segment index is bounded by SEGMENT_COUNT"),
            };
            target[*byte_offset..*byte_offset + WORD_SIZE as usize]
                .copy_from_slice(&relocated_word.to_le_bytes());
        }

        Ok(patches.len())
    }

    /// Reads all file-backed segments and allocates a zero-filled BSS tail.
    ///
    /// The caller supplies a total memory limit for the executable's code,
    /// read-only data, initialized data and BSS. Relocations are validated by
    /// the parser but are not applied here.
    pub fn load_segments<R: Read + Seek>(
        &self,
        reader: &mut R,
        maximum_memory_size_bytes: u64,
    ) -> io::Result<ThreeDsxLoadedSegments> {
        let total_memory_size_bytes = self
            .code
            .memory_size_bytes
            .checked_add(self.rodata.memory_size_bytes)
            .and_then(|size| size.checked_add(self.data.memory_size_bytes))
            .ok_or_else(|| invalid_data("3DSX total segment memory size overflows"))?;
        if total_memory_size_bytes > maximum_memory_size_bytes {
            return Err(invalid_data(format!(
                "3DSX requires {total_memory_size_bytes} bytes of segment memory, exceeding the {maximum_memory_size_bytes}-byte limit"
            )));
        }

        let code = read_segment(reader, self.code, "code")?;
        let rodata = read_segment(reader, self.rodata, "rodata")?;
        let data = read_segment(reader, self.data, "data")?;
        let bss_size_bytes = self
            .data
            .memory_size_bytes
            .checked_sub(self.data.file_size_bytes)
            .ok_or_else(|| invalid_data("3DSX BSS size exceeds the data segment"))?;
        let bss = allocate_zeroed_bytes(bss_size_bytes, "BSS")?;

        Ok(ThreeDsxLoadedSegments {
            code,
            rodata,
            data,
            bss,
        })
    }
}

fn read_relocation_word_offsets<R: Read + Seek>(
    reader: &mut R,
    table: ThreeDsxRelocationTable,
    segment_size_bytes: u64,
    remaining_patch_budget: usize,
) -> io::Result<Vec<u32>> {
    let segment_word_count = segment_size_bytes / WORD_SIZE;
    reader.seek(SeekFrom::Start(table.offset_bytes))?;

    let mut offsets = Vec::new();
    let mut current_word = 0u64;
    for _ in 0..table.entry_count {
        let mut bytes = [0u8; WORD_SIZE as usize];
        reader.read_exact(&mut bytes)?;
        let skip_words = u64::from(read_u16(&bytes, 0));
        let patch_words = usize::from(read_u16(&bytes, 2));
        current_word = current_word
            .checked_add(skip_words)
            .ok_or_else(|| invalid_data("3DSX relocation word offset overflows"))?;
        let patch_end = current_word
            .checked_add(patch_words as u64)
            .ok_or_else(|| invalid_data("3DSX relocation patch range overflows"))?;
        if patch_end > segment_word_count {
            return Err(invalid_data(
                "3DSX relocation patch range exceeds its segment",
            ));
        }
        let new_len = offsets
            .len()
            .checked_add(patch_words)
            .ok_or_else(|| invalid_data("3DSX relocation patch count overflows"))?;
        if new_len > remaining_patch_budget {
            return Err(invalid_data(format!(
                "3DSX relocation patch count exceeds the remaining limit of {remaining_patch_budget}"
            )));
        }
        offsets.try_reserve(patch_words).map_err(|error| {
            io::Error::other(format!("could not allocate relocation indices: {error}"))
        })?;
        for word in current_word..patch_end {
            offsets.push(
                u32::try_from(word)
                    .map_err(|_| invalid_data("3DSX relocation word index does not fit in u32"))?,
            );
        }
        current_word = patch_end;
    }

    Ok(offsets)
}

#[derive(Clone, Copy)]
enum RelocationTableKind {
    Absolute,
    Relative,
}

fn align_to_page(size_bytes: u64) -> io::Result<u64> {
    size_bytes
        .checked_add(PAGE_SIZE - 1)
        .map(|size| size & !(PAGE_SIZE - 1))
        .ok_or_else(|| invalid_data("3DSX aligned segment size overflows"))
}

fn validate_loaded_segments(
    image: &ThreeDsxImage,
    loaded: &ThreeDsxLoadedSegments,
) -> io::Result<()> {
    let expected = [
        image.code.file_size_bytes,
        image.rodata.file_size_bytes,
        image.data.file_size_bytes,
        image
            .data
            .memory_size_bytes
            .checked_sub(image.data.file_size_bytes)
            .ok_or_else(|| invalid_data("3DSX BSS size exceeds Data memory size"))?,
    ];
    let actual = [
        loaded.code.len(),
        loaded.rodata.len(),
        loaded.data.len(),
        loaded.bss.len(),
    ];

    for (name, expected_bytes, actual_bytes) in [
        ("Code", expected[0], actual[0]),
        ("ROData", expected[1], actual[1]),
        ("Data", expected[2], actual[2]),
        ("BSS", expected[3], actual[3]),
    ] {
        if expected_bytes != actual_bytes as u64 {
            return Err(invalid_data(format!(
                "loaded 3DSX {name} has {actual_bytes} bytes, expected {expected_bytes}"
            )));
        }
    }

    Ok(())
}

fn validate_segment_bases(
    bases: [u32; SEGMENT_COUNT],
    sizes: [u64; SEGMENT_COUNT],
) -> io::Result<()> {
    let mut ranges = Vec::with_capacity(SEGMENT_COUNT);
    for index in 0..SEGMENT_COUNT {
        if u64::from(bases[index]) % PAGE_SIZE != 0 {
            return Err(invalid_data(format!(
                "3DSX segment {index} base is not page-aligned"
            )));
        }
        let end = u64::from(bases[index])
            .checked_add(sizes[index])
            .ok_or_else(|| invalid_data("3DSX segment virtual range overflows"))?;
        if end > 0x1_0000_0000 {
            return Err(invalid_data(
                "3DSX segment virtual range exceeds the 32-bit address space",
            ));
        }
        if sizes[index] != 0 {
            ranges.push((u64::from(bases[index]), end, index));
        }
    }

    ranges.sort_unstable_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(invalid_data(format!(
                "3DSX virtual segments {} and {} overlap",
                pair[0].2, pair[1].2
            )));
        }
    }

    Ok(())
}

fn resolve_image_offset(
    image_offset: u32,
    segment_bases: [u32; SEGMENT_COUNT],
    aligned_sizes: [u64; SEGMENT_COUNT],
) -> io::Result<u32> {
    let image_offset = u64::from(image_offset);
    let code_end = aligned_sizes[0];
    let rodata_end = code_end
        .checked_add(aligned_sizes[1])
        .ok_or_else(|| invalid_data("3DSX ROData image offset overflows"))?;
    let data_end = rodata_end
        .checked_add(aligned_sizes[2])
        .ok_or_else(|| invalid_data("3DSX Data image offset overflows"))?;

    let (segment_index, segment_offset) = if image_offset < code_end {
        (0, image_offset)
    } else if image_offset < rodata_end {
        (1, image_offset - code_end)
    } else if image_offset < data_end {
        (2, image_offset - rodata_end)
    } else {
        return Err(invalid_data(format!(
            "3DSX relocation target {image_offset:#x} is outside the aligned image"
        )));
    };

    let address = u64::from(segment_bases[segment_index])
        .checked_add(segment_offset)
        .ok_or_else(|| invalid_data("3DSX relocated target address overflows"))?;
    u32::try_from(address)
        .map_err(|_| invalid_data("3DSX relocated target exceeds the 32-bit address space"))
}

fn prepare_relocation_words(
    source_bytes: &[u8],
    source_segment_index: usize,
    word_offsets: &[u32],
    table_kind: RelocationTableKind,
    segment_bases: [u32; SEGMENT_COUNT],
    aligned_sizes: [u64; SEGMENT_COUNT],
    patches: &mut Vec<(usize, usize, u32)>,
) -> io::Result<()> {
    for &word_index in word_offsets {
        let byte_offset_u64 = u64::from(word_index)
            .checked_mul(WORD_SIZE)
            .ok_or_else(|| invalid_data("3DSX relocation site offset overflows"))?;
        let byte_offset = usize::try_from(byte_offset_u64)
            .map_err(|_| invalid_data("3DSX relocation site does not fit host memory"))?;
        let end = byte_offset
            .checked_add(WORD_SIZE as usize)
            .ok_or_else(|| invalid_data("3DSX relocation site range overflows"))?;
        let word_bytes = source_bytes
            .get(byte_offset..end)
            .ok_or_else(|| invalid_data("3DSX relocation site is outside loaded segment bytes"))?;
        let original_word =
            u32::from_le_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);

        let subtype = original_word >> 28;
        let image_offset = original_word & RELOCATION_OFFSET_MASK;
        let target_address = resolve_image_offset(image_offset, segment_bases, aligned_sizes)?;

        let source_address = u64::from(segment_bases[source_segment_index])
            .checked_add(byte_offset_u64)
            .ok_or_else(|| invalid_data("3DSX relocation source address overflows"))?;
        let source_address = u32::try_from(source_address)
            .map_err(|_| invalid_data("3DSX relocation source exceeds the 32-bit address space"))?;

        let relocated_word = match table_kind {
            RelocationTableKind::Absolute => {
                if subtype != 0 {
                    return Err(invalid_data(format!(
                        "unsupported 3DSX absolute relocation subtype {subtype}"
                    )));
                }
                target_address
            }
            RelocationTableKind::Relative => {
                let difference = i64::from(target_address) - i64::from(source_address);
                match subtype {
                    0 => {
                        let signed = i32::try_from(difference).map_err(|_| {
                            invalid_data(
                                "3DSX relative relocation does not fit signed 32-bit range",
                            )
                        })?;
                        signed as u32
                    }
                    1 => {
                        const PREL31_MIN: i64 = -(1i64 << 30);
                        const PREL31_MAX: i64 = (1i64 << 30) - 1;
                        if !(PREL31_MIN..=PREL31_MAX).contains(&difference) {
                            return Err(invalid_data(
                                "3DSX PREL31 relocation does not fit signed 31-bit range",
                            ));
                        }
                        (difference as i32 as u32) & 0x7FFF_FFFF
                    }
                    _ => {
                        return Err(invalid_data(format!(
                            "unsupported 3DSX relative relocation subtype {subtype}"
                        )));
                    }
                }
            }
        };

        patches.push((source_segment_index, byte_offset, relocated_word));
    }

    Ok(())
}

fn read_segment<R: Read + Seek>(
    reader: &mut R,
    segment: ThreeDsxSegment,
    name: &'static str,
) -> io::Result<Vec<u8>> {
    let mut bytes = allocate_zeroed_bytes(segment.file_size_bytes, name)?;
    reader.seek(SeekFrom::Start(segment.offset_bytes))?;
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn allocate_zeroed_bytes(size_bytes: u64, name: &'static str) -> io::Result<Vec<u8>> {
    let size = usize::try_from(size_bytes)
        .map_err(|_| invalid_data(format!("3DSX {name} size does not fit in host memory")))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|error| io::Error::other(format!("could not allocate 3DSX {name}: {error}")))?;
    bytes.resize(size, 0);
    Ok(bytes)
}

/// Validates the skip/patch word runs in one relocation table without applying them.
fn validate_relocation_table<R: Read + Seek>(
    reader: &mut R,
    table: ThreeDsxRelocationTable,
    segment_size_bytes: u64,
) -> io::Result<()> {
    let segment_word_count = segment_size_bytes / WORD_SIZE;
    reader.seek(SeekFrom::Start(table.offset_bytes))?;
    let mut current_word = 0u64;

    for _ in 0..table.entry_count {
        let mut bytes = [0u8; WORD_SIZE as usize];
        reader.read_exact(&mut bytes)?;
        let skip_words = u64::from(read_u16(&bytes, 0));
        let patch_words = u64::from(read_u16(&bytes, 2));
        current_word = current_word
            .checked_add(skip_words)
            .ok_or_else(|| invalid_data("3DSX relocation word offset overflows"))?;
        let patch_end = current_word
            .checked_add(patch_words)
            .ok_or_else(|| invalid_data("3DSX relocation patch range overflows"))?;

        if patch_end > segment_word_count {
            return Err(invalid_data(format!(
                "3DSX relocation range {current_word}..{patch_end} exceeds segment word count {segment_word_count}"
            )));
        }
        current_word = patch_end;
    }

    Ok(())
}

fn validate_extended_sections(
    extended: Option<ThreeDsxExtendedHeader>,
    minimum_data_offset: u64,
    file_len: u64,
) -> io::Result<()> {
    let Some(extended) = extended else {
        return Ok(());
    };

    let smdh_offset = u64::from(extended.smdh_offset_bytes);
    let smdh_size = u64::from(extended.smdh_size_bytes);
    if smdh_size == 0 {
        if smdh_offset != 0 {
            return Err(invalid_data(
                "3DSX SMDH offset is set while its size is zero",
            ));
        }
    } else {
        if smdh_offset < minimum_data_offset {
            return Err(invalid_data(
                "3DSX SMDH data overlaps executable or relocation data",
            ));
        }
        checked_file_end(smdh_offset, smdh_size, file_len, "SMDH data")?;
    }

    let romfs_offset = u64::from(extended.romfs_offset_bytes);
    if romfs_offset != 0 {
        if romfs_offset < minimum_data_offset {
            return Err(invalid_data(
                "3DSX RomFS offset overlaps executable or relocation data",
            ));
        }
        if romfs_offset > file_len {
            return Err(invalid_data("3DSX RomFS offset extends beyond the file"));
        }
        if smdh_size != 0 {
            let smdh_end = smdh_offset
                .checked_add(smdh_size)
                .ok_or_else(|| invalid_data("3DSX SMDH range overflows"))?;
            if romfs_offset < smdh_end {
                return Err(invalid_data("3DSX RomFS overlaps SMDH data"));
            }
        }
    }

    Ok(())
}

fn checked_file_end(
    start: u64,
    size: u64,
    file_len: u64,
    section_name: &'static str,
) -> io::Result<u64> {
    let end = start
        .checked_add(size)
        .ok_or_else(|| invalid_data(format!("3DSX {section_name} range overflows")))?;
    if end > file_len {
        return Err(invalid_data(format!(
            "3DSX {section_name} ends at {end:#x}, beyond file length {file_len:#x}"
        )));
    }
    Ok(end)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn enable_code_absolute_relocation(image: &mut [u8], image_offset: u32) {
        put_u32(image, 56, image_offset);
        put_u16(image, 68, 0);
        put_u16(image, 70, 1);
    }

    fn enable_rodata_relative_relocation(image: &mut [u8], image_offset: u32) {
        put_u32(image, 60, image_offset);
        put_u16(image, 72, 0);
        put_u16(image, 74, 1);
    }

    fn minimal_image(header_size: u16, smdh: Option<(u32, u32, u32)>) -> Vec<u8> {
        let mut image = vec![0u8; usize::from(header_size)];
        image[..4].copy_from_slice(b"3DSX");
        put_u16(&mut image, 4, header_size);
        put_u16(&mut image, 6, STANDARD_RELOCATION_HEADER_SIZE);
        put_u32(&mut image, 8, 0);
        put_u32(&mut image, 12, 0);
        put_u32(&mut image, 16, 4);
        put_u32(&mut image, 20, 4);
        put_u32(&mut image, 24, 8);
        put_u32(&mut image, 28, 4);

        if let Some((smdh_offset, smdh_size, romfs_offset)) = smdh {
            put_u32(&mut image, 32, smdh_offset);
            put_u32(&mut image, 36, smdh_size);
            put_u32(&mut image, 40, romfs_offset);
        }

        // Code has one absolute relocation; rodata has one relative relocation.
        image.extend_from_slice(&1u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&1u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());
        image.extend_from_slice(&0u32.to_le_bytes());

        image.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        image.extend_from_slice(&[0x55, 0x66, 0x77, 0x88]);
        image.extend_from_slice(&[0x99, 0xAA, 0xBB, 0xCC]);
        image.extend_from_slice(&[0, 0, 0, 0]); // code absolute relocation
        image.extend_from_slice(&[0, 0, 0, 0]); // rodata relative relocation
        image
    }

    #[test]
    fn parses_standard_segments_bss_and_relocation_ranges() {
        let image = minimal_image(HEADER_SIZE as u16, None);
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();

        assert_eq!(parsed.header.format_version, 0);
        assert_eq!(parsed.code.offset_bytes, 56);
        assert_eq!(parsed.code.file_size_bytes, 4);
        assert_eq!(parsed.rodata.offset_bytes, 60);
        assert_eq!(parsed.data.offset_bytes, 64);
        assert_eq!(parsed.data.file_size_bytes, 4);
        assert_eq!(parsed.data.memory_size_bytes, 8);
        assert_eq!(parsed.relocations[0].absolute.offset_bytes, 68);
        assert_eq!(parsed.relocations[0].absolute.entry_count, 1);
        assert_eq!(parsed.relocations[1].relative.offset_bytes, 72);
        assert_eq!(parsed.relocations[1].relative.entry_count, 1);
        assert_eq!(parsed.trailing_data_offset_bytes, 76);
    }

    #[test]
    fn parses_extended_header_and_bounds_optional_sections() {
        let mut image = minimal_image(EXTENDED_HEADER_SIZE as u16, Some((88, 4, 92)));
        image.extend_from_slice(&[1, 2, 3, 4]); // SMDH
        image.extend_from_slice(&[5, 6, 7, 8]); // RomFS

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        assert_eq!(
            parsed.extended_header,
            Some(ThreeDsxExtendedHeader {
                smdh_offset_bytes: 88,
                smdh_size_bytes: 4,
                romfs_offset_bytes: 92,
            })
        );
    }

    #[test]
    fn rejects_partial_extended_header() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u16(&mut image, 4, 40);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn rejects_bss_larger_than_the_data_segment() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u32(&mut image, 28, 9);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn rejects_relocation_runs_that_target_uninitialized_bss() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        // The data segment has one file-backed word and one BSS word.
        // Its relocation table must not patch the second (zero-filled) word.
        put_u32(&mut image, 48, 1);
        image.extend_from_slice(&1u16.to_le_bytes());
        image.extend_from_slice(&1u16.to_le_bytes());

        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn expands_absolute_and_relative_relocation_runs_to_word_indices() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u16(&mut image, 68, 0);
        put_u16(&mut image, 70, 1);
        put_u16(&mut image, 72, 0);
        put_u16(&mut image, 74, 1);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let offsets = parsed
            .read_relocation_word_offsets(&mut Cursor::new(&image), 2)
            .unwrap();

        assert_eq!(offsets[0].absolute, [0]);
        assert!(offsets[0].relative.is_empty());
        assert!(offsets[1].absolute.is_empty());
        assert_eq!(offsets[1].relative, [0]);
        assert!(offsets[2].absolute.is_empty());
        assert!(offsets[2].relative.is_empty());
    }

    #[test]
    fn rejects_relocation_word_expansion_above_caller_limit() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u16(&mut image, 68, 0);
        put_u16(&mut image, 70, 1);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let error = parsed
            .read_relocation_word_offsets(&mut Cursor::new(&image), 0)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn applies_absolute_relocation_to_rodata_virtual_address() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        enable_code_absolute_relocation(&mut image, 0x1000);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        let patched = parsed
            .apply_relocations(
                &mut Cursor::new(&image),
                &mut loaded,
                [0x0010_0000, 0x0020_0000, 0x0030_0000],
                1,
            )
            .unwrap();

        assert_eq!(patched, 1);
        assert_eq!(
            u32::from_le_bytes(loaded.code[..4].try_into().unwrap()),
            0x0020_0000
        );
    }

    #[test]
    fn applies_relative_32_relocation_between_segments() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        enable_rodata_relative_relocation(&mut image, 0x2000);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        let patched = parsed
            .apply_relocations(
                &mut Cursor::new(&image),
                &mut loaded,
                [0x0010_0000, 0x0020_0000, 0x0030_0000],
                1,
            )
            .unwrap();

        assert_eq!(patched, 1);
        assert_eq!(
            u32::from_le_bytes(loaded.rodata[..4].try_into().unwrap()),
            0x0010_0000
        );
    }

    #[test]
    fn applies_prel31_relocation_and_keeps_the_reserved_bit_clear() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        enable_rodata_relative_relocation(&mut image, 0x1000_0000);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        parsed
            .apply_relocations(
                &mut Cursor::new(&image),
                &mut loaded,
                [0x0010_0000, 0x0020_0000, 0x0030_0000],
                1,
            )
            .unwrap();

        assert_eq!(
            u32::from_le_bytes(loaded.rodata[..4].try_into().unwrap()),
            0x7FF0_0000
        );
    }

    #[test]
    fn absolute_relocation_can_target_the_zero_filled_bss_tail() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        enable_code_absolute_relocation(&mut image, 0x2004);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        parsed
            .apply_relocations(
                &mut Cursor::new(&image),
                &mut loaded,
                [0x0010_0000, 0x0020_0000, 0x0030_0000],
                1,
            )
            .unwrap();

        assert_eq!(
            u32::from_le_bytes(loaded.code[..4].try_into().unwrap()),
            0x0030_0004
        );
        assert_eq!(loaded.bss, [0, 0, 0, 0]);
    }

    #[test]
    fn rejects_unknown_subtypes_without_partially_applying_prior_patches() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        enable_code_absolute_relocation(&mut image, 0x1000);
        enable_rodata_relative_relocation(&mut image, 0x2000);
        put_u32(&mut image, 60, 0x2000_2000);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        let original_code = loaded.code.clone();
        let result = parsed.apply_relocations(
            &mut Cursor::new(&image),
            &mut loaded,
            [0x0010_0000, 0x0020_0000, 0x0030_0000],
            2,
        );

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(loaded.code, original_code);
    }

    #[test]
    fn rejects_relocation_targets_outside_the_aligned_image() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        enable_code_absolute_relocation(&mut image, 0x0FFF_FFFF);

        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();
        let error = parsed
            .apply_relocations(
                &mut Cursor::new(&image),
                &mut loaded,
                [0x0010_0000, 0x0020_0000, 0x0030_0000],
                1,
            )
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_unaligned_and_overlapping_segment_bases() {
        let image = minimal_image(HEADER_SIZE as u16, None);
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let mut loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();

        let unaligned = parsed.apply_relocations(
            &mut Cursor::new(&image),
            &mut loaded,
            [0x0010_0000, 0x0020_0001, 0x0030_0000],
            2,
        );
        assert_eq!(unaligned.unwrap_err().kind(), io::ErrorKind::InvalidData);

        let overlapping = parsed.apply_relocations(
            &mut Cursor::new(&image),
            &mut loaded,
            [0x0010_0000, 0x0010_0800, 0x0030_0000],
            2,
        );
        assert_eq!(overlapping.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn loads_file_segments_and_zero_fills_bss_within_memory_limit() {
        let image = minimal_image(HEADER_SIZE as u16, None);
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let loaded = parsed.load_segments(&mut Cursor::new(&image), 16).unwrap();

        assert_eq!(loaded.code, [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(loaded.rodata, [0x55, 0x66, 0x77, 0x88]);
        assert_eq!(loaded.data, [0x99, 0xAA, 0xBB, 0xCC]);
        assert_eq!(loaded.bss, [0, 0, 0, 0]);
    }

    #[test]
    fn rejects_segment_materialization_above_memory_limit() {
        let image = minimal_image(HEADER_SIZE as u16, None);
        let parsed = ThreeDsxImage::parse_reader(&mut Cursor::new(&image)).unwrap();
        let error = parsed
            .load_segments(&mut Cursor::new(&image), 15)
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_relocation_runs_that_extend_past_the_segment() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        // The code segment is one word; skip one word and then request another.
        put_u16(&mut image, 68, 1);
        put_u16(&mut image, 70, 1);

        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn accepts_relocation_runs_ending_at_the_segment_boundary() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        // One patched word exactly covers the four-byte code segment.
        put_u16(&mut image, 68, 0);
        put_u16(&mut image, 70, 1);

        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_ok());
    }

    #[test]
    fn rejects_truncated_segment_or_relocation_data() {
        let mut image = minimal_image(HEADER_SIZE as u16, None);
        image.truncate(image.len() - 1);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());

        let mut image = minimal_image(HEADER_SIZE as u16, None);
        put_u32(&mut image, 32, u32::MAX);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }

    #[test]
    fn rejects_optional_sections_that_overlap_relocations() {
        let mut image = minimal_image(EXTENDED_HEADER_SIZE as u16, Some((72, 4, 0)));
        image.extend_from_slice(&[0; 4]);
        assert!(ThreeDsxImage::parse_reader(&mut Cursor::new(image)).is_err());
    }
}
