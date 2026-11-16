//! Manual regression test for the user-supplied Super Mario 64 3DS CIA.
//!
//! The archive is deliberately not stored in this repository or CI. Set
//! ACERIS_REAL_CIA to a local path and run this ignored test explicitly.

use std::{fs::File, path::PathBuf};

use aceris_core::{
    Error, System,
    loader::cia::{CiaArchiveReader, tmd::CONTENT_TYPE_ENCRYPTED},
};

const EXPECTED_TITLE_ID: u64 = 0x0004_0000_0FAA_5C00;
const EXPECTED_CONTENT_OFFSET: u64 = 0x3900;
const EXPECTED_CONTENT_SIZE: u64 = 0x770C00;

#[test]
#[ignore = "requires the maintainer-supplied CIA at ACERIS_REAL_CIA"]
fn validates_real_super_mario_64_luigi_cia_and_reports_inner_ncch_encryption() {
    let path = std::env::var_os("ACERIS_REAL_CIA")
        .map(PathBuf::from)
        .expect("set ACERIS_REAL_CIA to the local .cia path");

    let mut archive = CiaArchiveReader::open(File::open(&path).unwrap()).unwrap();
    let title_id = archive.title_metadata().title_id;
    let boot_content_index = archive.title_metadata().boot_content_index;

    assert_eq!(title_id, EXPECTED_TITLE_ID);
    assert_eq!(boot_content_index, 0);

    let ticket = archive.ticket().expect("the CIA contains a ticket");
    assert_eq!(ticket.title_id, title_id);
    assert_eq!(ticket.common_key_index, 0);

    let location = archive
        .content_location(boot_content_index)
        .expect("the boot content is selected by the CIA bitmap");
    assert_eq!(location.offset_bytes, EXPECTED_CONTENT_OFFSET);
    assert_eq!(location.size_bytes, EXPECTED_CONTENT_SIZE);
    assert_eq!(location.content_type & CONTENT_TYPE_ENCRYPTED, 0);

    // This verifies the full content against its TMD SHA-256 and parses the
    // NCCH header without requiring the user-owned NCCH AES keys.
    let ncch = archive
        .open_verified_ncch_content_reader(boot_content_index)
        .unwrap()
        .expect("the boot content contains an NCCH");
    assert_eq!(ncch.header().program_id, title_id);
    assert!(!ncch.header().declares_no_crypto());

    // The default file launcher must stop at the explicit missing-key
    // boundary. It must not replace an existing process on this failure.
    let mut system = System::new();
    let error = system.load_executable_file(&path).unwrap_err();
    assert!(matches!(error, Error::Unimplemented(_)));
    assert_eq!(system.image(), None);

    eprintln!(
        "CIA validated: title={title_id:#018x}, content_index={}, content_bytes={:#x};          outer CIA content is clear, inner NCCH encryption requires caller-supplied keys",
        location.content_index,
        location.size_bytes
    );
}
