//! Integration tests for bounded NCSD partition access.
//!
//! The reader API is public because cartridge-loading stages consume
//! partitions without copying an entire .3DS/.CCI image into memory.

use std::io::{Cursor, Read, Seek, SeekFrom};

use aceris_core::loader::ncsd::NcsdPartition;

#[derive(Debug)]
struct VirtualImage {
    length: u64,
    position: u64,
}

impl VirtualImage {
    fn new(length: u64) -> Self {
        Self {
            length,
            position: 0,
        }
    }
}

impl Read for VirtualImage {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.length.saturating_sub(self.position);
        let count = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));

        for (index, byte) in buffer[..count].iter_mut().enumerate() {
            *byte = (self.position + index as u64) as u8;
        }

        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for VirtualImage {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let target = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.length) + i128::from(offset),
        };

        if !(0..=i128::from(self.length)).contains(&target) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "virtual image seek is outside its range",
            ));
        }

        self.position = u64::try_from(target).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "virtual image position is invalid",
            )
        })?;
        Ok(self.position)
    }
}

fn partition(offset_bytes: u64, size_bytes: u64) -> NcsdPartition {
    NcsdPartition {
        index: 0,
        offset_bytes,
        size_bytes,
        fs_type: 0,
        crypt_type: 0,
        id: 0,
    }
}

#[test]
fn partition_reader_never_exposes_bytes_outside_its_range() {
    let image = Cursor::new(b"0123456789".to_vec());
    let mut reader = partition(2, 4).reader(image).unwrap();

    assert_eq!(reader.len(), 4);
    assert!(!reader.is_empty());
    assert_eq!(reader.position(), 0);

    let mut contents = Vec::new();
    reader.read_to_end(&mut contents).unwrap();

    assert_eq!(contents, b"2345");
    assert_eq!(reader.position(), 4);

    let mut extra = [0u8; 1];
    assert_eq!(reader.read(&mut extra).unwrap(), 0);
}

#[test]
fn partition_reader_seeks_relative_to_partition_start() {
    let image = Cursor::new((0u8..10).collect::<Vec<_>>());
    let mut reader = partition(2, 6).reader(image).unwrap();

    assert_eq!(reader.seek(SeekFrom::End(-2)).unwrap(), 4);

    let mut tail = [0u8; 2];
    reader.read_exact(&mut tail).unwrap();
    assert_eq!(tail, [6, 7]);
    assert_eq!(reader.position(), 6);

    assert_eq!(reader.seek(SeekFrom::Start(1)).unwrap(), 1);
    assert_eq!(reader.seek(SeekFrom::Current(2)).unwrap(), 3);
    assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 6);
}

#[test]
fn partition_reader_reports_remaining_bytes_after_reads_and_seeks() {
    let image = Cursor::new(b"0123456789".to_vec());
    let mut reader = partition(2, 6).reader(image).unwrap();

    assert_eq!(reader.remaining(), 6);

    let mut prefix = [0u8; 2];
    reader.read_exact(&mut prefix).unwrap();
    assert_eq!(prefix, *b"23");
    assert_eq!(reader.remaining(), 4);

    assert_eq!(reader.seek(SeekFrom::End(-1)).unwrap(), 5);
    assert_eq!(reader.remaining(), 1);

    let mut last = [0u8; 1];
    reader.read_exact(&mut last).unwrap();
    assert_eq!(last, *b"7");
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn partition_reader_rejects_seeks_outside_the_partition() {
    let image = Cursor::new(b"0123456789".to_vec());
    let mut reader = partition(2, 4).reader(image).unwrap();

    let error = reader.seek(SeekFrom::Start(5)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(reader.position(), 0);

    let error = reader.seek(SeekFrom::Current(-1)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(reader.position(), 0);
}

#[test]
fn partition_reader_handles_offsets_above_four_gibibytes() {
    let offset_bytes = u64::from(u32::MAX) + 0x200;
    let size_bytes = 4;
    let image_len = offset_bytes + size_bytes;
    let selected = NcsdPartition {
        index: 0,
        offset_bytes,
        size_bytes,
        fs_type: 0,
        crypt_type: 0,
        id: 0,
    };

    let mut reader = selected.reader(VirtualImage::new(image_len)).unwrap();
    assert_eq!(reader.len(), size_bytes);
    assert_eq!(reader.seek(SeekFrom::Start(1)).unwrap(), 1);

    let mut bytes = [0u8; 3];
    reader.read_exact(&mut bytes).unwrap();
    assert_eq!(
        bytes,
        [
            (offset_bytes + 1) as u8,
            (offset_bytes + 2) as u8,
            (offset_bytes + 3) as u8
        ]
    );
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn partition_reader_rejects_extreme_relative_seeks_without_changing_position() {
    let image = Cursor::new(b"0123456789".to_vec());
    let mut reader = partition(2, 4).reader(image).unwrap();

    for from in [
        SeekFrom::Current(i64::MIN),
        SeekFrom::Current(i64::MAX),
        SeekFrom::End(i64::MIN),
        SeekFrom::End(i64::MAX),
    ] {
        let error = reader.seek(from).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(reader.position(), 0);
    }

    let mut first = [0u8; 1];
    reader.read_exact(&mut first).unwrap();
    assert_eq!(first, *b"2");
    assert_eq!(reader.remaining(), 3);
}

#[test]
fn partition_reader_rejects_empty_and_overflowing_ranges() {
    let image = Cursor::new(vec![0u8; 16]);

    let empty_error = partition(0, 0).reader(image.clone()).unwrap_err();
    assert_eq!(empty_error.kind(), std::io::ErrorKind::InvalidInput);

    let overflow_error = partition(u64::MAX - 1, 4).reader(image).unwrap_err();
    assert_eq!(overflow_error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn partition_reader_rejects_ranges_beyond_the_underlying_image() {
    let image = Cursor::new(b"012345".to_vec());

    let error = partition(2, 5).reader(image).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
}

#[test]
fn partition_reader_accepts_a_range_ending_exactly_at_image_end() {
    let image = Cursor::new(b"012345".to_vec());
    let mut reader = partition(2, 4).reader(image).unwrap();

    let mut contents = Vec::new();
    reader.read_to_end(&mut contents).unwrap();
    assert_eq!(contents, b"2345");
}
