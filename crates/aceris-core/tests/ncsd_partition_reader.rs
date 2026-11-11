//! Integration tests for bounded NCSD partition access.
//!
//! The reader API is public because cartridge-loading stages consume
//! partitions without copying an entire .3DS/.CCI image into memory.

use std::io::{Cursor, Read, Seek, SeekFrom};

use aceris_core::loader::ncsd::NcsdPartition;

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
fn partition_reader_rejects_empty_and_overflowing_ranges() {
    let image = Cursor::new(vec![0u8; 16]);

    let empty_error = partition(0, 0).reader(image.clone()).unwrap_err();
    assert_eq!(empty_error.kind(), std::io::ErrorKind::InvalidInput);

    let overflow_error = partition(u64::MAX - 1, 4).reader(image).unwrap_err();
    assert_eq!(overflow_error.kind(), std::io::ErrorKind::InvalidInput);
}
