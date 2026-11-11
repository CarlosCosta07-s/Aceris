//! Integration tests for opening NCSD partitions through the image reader.
//!
//! These exercise sparse original partition-table indices and the raw bounded
//! stream used by later NCCH, filesystem, and metadata loading stages.

use std::io::{Cursor, Read, Seek, SeekFrom};

use aceris_core::loader::ncsd::{
    NcsdImageReader, NcsdImageReaderError, ncch::romfs::sha256::Sha256,
};

const MEDIA_UNIT: usize = 0x200;
const IMAGE_SIZE_UNITS: usize = 0x80;

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn write_partition(bytes: &mut [u8], index: usize, offset_units: u32, size_units: u32, id: u64) {
    let entry = 0x120 + index * 8;
    write_u32(bytes, entry, offset_units);
    write_u32(bytes, entry + 4, size_units);
    bytes[0x110 + index] = (index + 1) as u8;
    bytes[0x118 + index] = 0;
    write_u64(bytes, 0x190 + index * 8, id);
}

fn image_with_sparse_ncch_partition() -> Vec<u8> {
    let mut image = image_with_sparse_partitions();
    let partition_start = 0x20 * MEDIA_UNIT;

    image[partition_start + 0x100..partition_start + 0x104].copy_from_slice(b"NCCH");
    write_u32(&mut image, partition_start + 0x104, 0x08);

    image
}

fn image_with_sparse_partitions() -> Vec<u8> {
    let mut image = vec![0; IMAGE_SIZE_UNITS * MEDIA_UNIT];
    image[0x100..0x104].copy_from_slice(b"NCSD");
    write_u32(&mut image, 0x104, IMAGE_SIZE_UNITS as u32);

    // Deliberately leave slots 0, 1, 3 and 4 empty.
    write_partition(&mut image, 2, 0x20, 0x08, 0x2233_4455_6677_8899);
    write_partition(&mut image, 5, 0x40, 0x10, 0x5566_7788_99AA_BBCC);

    let primary_start = 0x20 * MEDIA_UNIT;
    image[primary_start..primary_start + 12].copy_from_slice(b"MAIN-PART-02");

    let auxiliary_start = 0x40 * MEDIA_UNIT;
    image[auxiliary_start..auxiliary_start + 11].copy_from_slice(b"AUX-PART-05");
    image[auxiliary_start + 0x1FFC..auxiliary_start + 0x2000].copy_from_slice(b"END5");

    image
}

fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data).unwrap();
    hasher.finalize()
}

fn image_with_verified_exefs_and_secondary_partition() -> (Vec<u8>, Vec<u8>) {
    const PARTITION_OFFSET: usize = 0x20 * MEDIA_UNIT;
    const AUXILIARY_OFFSET: usize = 0x60 * MEDIA_UNIT;
    const EXEFS_OFFSET: usize = 0x2000;
    const EXEFS_SIZE: usize = 0x4000;

    let code = b"ACERIS-VERIFIED-CODE".to_vec();
    let mut image = vec![0u8; IMAGE_SIZE_UNITS * MEDIA_UNIT];
    image[0x100..0x104].copy_from_slice(b"NCSD");
    write_u32(&mut image, 0x104, IMAGE_SIZE_UNITS as u32);
    write_partition(&mut image, 2, 0x20, 0x40, 0x2233_4455_6677_8899);
    write_partition(&mut image, 5, 0x60, 0x10, 0x5566_7788_99AA_BBCC);

    let mut exefs = vec![0u8; EXEFS_SIZE];
    exefs[..5].copy_from_slice(b".code");
    write_u32(&mut exefs, 8, 0);
    write_u32(&mut exefs, 12, code.len() as u32);
    exefs[0x200..0x200 + code.len()].copy_from_slice(&code);
    exefs[0x1E0..0x200].copy_from_slice(&sha256_digest(&code));

    let exefs_superblock_hash = sha256_digest(&exefs[..0x200]);
    let ncch_start = PARTITION_OFFSET;
    image[ncch_start + 0x100..ncch_start + 0x104].copy_from_slice(b"NCCH");
    write_u32(&mut image, ncch_start + 0x104, 0x40);
    image[ncch_start + 0x18F] = 0x04;
    write_u32(&mut image, ncch_start + 0x1A0, 0x10);
    write_u32(&mut image, ncch_start + 0x1A4, 0x20);
    write_u32(&mut image, ncch_start + 0x1A8, 1);
    image[ncch_start + 0x1C0..ncch_start + 0x1E0].copy_from_slice(&exefs_superblock_hash);

    let exefs_start = ncch_start + EXEFS_OFFSET;
    image[exefs_start..exefs_start + EXEFS_SIZE].copy_from_slice(&exefs);
    image[AUXILIARY_OFFSET..AUXILIARY_OFFSET + 11].copy_from_slice(b"AUX-PART-05");

    (image, code)
}

#[test]
fn image_reader_opens_a_sparse_partition_by_its_original_table_index() {
    let image = Cursor::new(image_with_sparse_partitions());
    let reader = NcsdImageReader::open(image).unwrap();

    assert_eq!(reader.header().partitions.len(), 2);
    assert!(reader.header().partition(0).is_none());
    assert_eq!(
        reader.header().partition(5).unwrap().id,
        0x5566_7788_99AA_BBCC
    );

    let mut partition = reader.open_partition_reader(5).unwrap();
    assert_eq!(partition.len(), 0x10 * MEDIA_UNIT as u64);
    assert_eq!(partition.position(), 0);

    let mut marker = [0u8; 11];
    partition.read_exact(&mut marker).unwrap();
    assert_eq!(&marker, b"AUX-PART-05");

    assert_eq!(partition.seek(SeekFrom::End(-4)).unwrap(), 0x1FFC);
    let mut end_marker = [0u8; 4];
    partition.read_exact(&mut end_marker).unwrap();
    assert_eq!(&end_marker, b"END5");
}

#[test]
fn image_reader_can_borrow_multiple_partitions_without_reopening_the_image() {
    let image = Cursor::new(image_with_sparse_partitions());
    let mut reader = NcsdImageReader::open(image).unwrap();

    {
        let mut primary = reader.borrow_partition_reader(2).unwrap();
        let mut marker = [0u8; 12];
        primary.read_exact(&mut marker).unwrap();
        assert_eq!(&marker, b"MAIN-PART-02");
        assert_eq!(primary.remaining(), primary.len() - 12);
    }

    {
        let mut auxiliary = reader.borrow_partition_reader(5).unwrap();
        let mut marker = [0u8; 11];
        auxiliary.read_exact(&mut marker).unwrap();
        assert_eq!(&marker, b"AUX-PART-05");
    }

    assert_eq!(reader.header().partitions.len(), 2);
    assert_eq!(reader.header().partition(2).unwrap().index, 2);
    assert_eq!(reader.header().partition(5).unwrap().index, 5);
}

#[test]
fn image_reader_keeps_metadata_available_after_a_borrowed_partition_error() {
    let image = Cursor::new(image_with_sparse_partitions());
    let mut reader = NcsdImageReader::open(image).unwrap();

    let error = reader.borrow_partition_reader(0).unwrap_err();
    assert!(matches!(
        error,
        NcsdImageReaderError::MissingPartition { index: 0 }
    ));

    assert_eq!(reader.header().partitions.len(), 2);
    let mut primary = reader.borrow_partition_reader(2).unwrap();
    let mut marker = [0u8; 12];
    primary.read_exact(&mut marker).unwrap();
    assert_eq!(&marker, b"MAIN-PART-02");
}

#[test]
fn image_reader_can_borrow_ncch_then_open_another_raw_partition() {
    let image = Cursor::new(image_with_sparse_ncch_partition());
    let mut reader = NcsdImageReader::open(image).unwrap();

    {
        let opened = reader.borrow_partition_ncch(2).unwrap();
        assert_eq!(opened.partition().index, 2);
        assert_eq!(
            opened.header().content_size_bytes,
            (0x08 * MEDIA_UNIT) as u64
        );
    }

    {
        let mut auxiliary = reader.borrow_partition_reader(5).unwrap();
        let mut marker = [0u8; 11];
        auxiliary.read_exact(&mut marker).unwrap();
        assert_eq!(&marker, b"AUX-PART-05");
    }

    assert_eq!(reader.header().partitions.len(), 2);
}

#[test]
fn borrowed_ncch_reader_verifies_exefs_and_reuses_the_same_image() {
    let (image, expected_code) = image_with_verified_exefs_and_secondary_partition();
    let mut reader = NcsdImageReader::open(Cursor::new(image)).unwrap();

    {
        let ncch = reader.borrow_partition_ncch(2).unwrap();
        let mut code_reader = ncch
            .open_verified_exefs_file_reader(".code")
            .unwrap()
            .unwrap();

        let mut actual_code = Vec::new();
        code_reader.read_to_end(&mut actual_code).unwrap();
        assert_eq!(actual_code, expected_code);
    }

    {
        let mut auxiliary = reader.borrow_partition_reader(5).unwrap();
        let mut marker = [0u8; 11];
        auxiliary.read_exact(&mut marker).unwrap();
        assert_eq!(&marker, b"AUX-PART-05");
    }

    assert_eq!(reader.header().partitions.len(), 2);
}

#[test]
fn image_reader_reports_the_original_index_for_an_empty_partition() {
    let image = Cursor::new(image_with_sparse_partitions());
    let error = NcsdImageReader::open(image)
        .unwrap()
        .open_partition_reader(0)
        .unwrap_err();

    assert!(matches!(
        error,
        NcsdImageReaderError::MissingPartition { index: 0 }
    ));
}
