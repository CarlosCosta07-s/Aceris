//! Manual boot tests for locally supplied Nintendo 3DS CIA homebrew.
//!
//! Archives are deliberately not stored in this repository or CI. Set the
//! corresponding environment variable and run the ignored test explicitly.

use std::{fs::File, path::PathBuf};

use aceris_core::{
    System,
    loader::{
        ImageFormat,
        cia::{CiaArchiveReader, tmd::CONTENT_TYPE_ENCRYPTED},
    },
};

const EXPECTED_TITLE_ID: u64 = 0x0004_0000_0FAA_5C00;
const EXPECTED_CONTENT_OFFSET: u64 = 0x3900;
const EXPECTED_CONTENT_SIZE: u64 = 0x770C00;

#[test]
#[ignore = "requires the maintainer-supplied CIA at ACERIS_REAL_CIA"]
fn validates_and_runs_real_super_mario_64_luigi_cia_without_console_keys() {
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

    let location = archive
        .content_location(boot_content_index)
        .expect("the boot content is selected by the CIA bitmap");
    assert_eq!(location.offset_bytes, EXPECTED_CONTENT_OFFSET);
    assert_eq!(location.size_bytes, EXPECTED_CONTENT_SIZE);
    assert_eq!(location.content_type & CONTENT_TYPE_ENCRYPTED, 0);

    // This checks the complete content against the TMD SHA-256, then parses
    // the NCCH header. This title sets NoCrypto, so no console key is needed.
    {
        let ncch = archive
            .open_verified_ncch_content_reader(boot_content_index)
            .unwrap()
            .expect("the boot content contains an NCCH");
        assert_eq!(ncch.header().program_id, title_id);
        assert!(ncch.header().declares_no_crypto());

        // Also force the NCCH extended-header hash, ExeFS superblock hash,
        // .code digest and BLZ preparation path before replacing the System.
        let address_space = ncch.load_arm11_address_space([0; 16], [0; 16]).unwrap();
        eprintln!(
            "Prepared real ARM11 image: entry={:#010x}, stack_top={:#010x}, stack_size={:#x}",
            address_space.layout().entry_point,
            address_space.layout().initial_stack_pointer,
            address_space.layout().stack_size_bytes
        );
    }

    // Exercise the same default launcher a user selects in the desktop UI.
    let mut system = System::new();
    assert_eq!(
        system.load_executable_file(&path).unwrap(),
        ImageFormat::Cia
    );
    assert_eq!(system.image(), Some(ImageFormat::Cia));

    let frame = system.run_frame_with_budget(50_000).unwrap();
    assert!(
        frame.instructions_executed > 0,
        "the loaded ARM11 guest did not execute any instructions"
    );
    eprintln!(
        "Real homebrew frame: instructions={}, CPU stop={:?}, GSP command lists={}, LCD scanouts={}",
        frame.instructions_executed,
        frame.cpu_stop_reason,
        frame.gpu_command_lists_submitted,
        frame.lcd_screens_scanned_out
    );
}

/// Manual boot test for the maintainer-supplied Lua Player Plus Cookie Clicker CIA.
/// The file remains local and is never uploaded to CI or committed.
#[test]
#[ignore = "requires the maintainer-supplied Cookie Clicker CIA at ACERIS_COOKIE_CLICKER_CIA"]
fn validates_and_runs_real_cookie_clicker_cia_without_console_keys() {
    const EXPECTED_TITLE_ID: u64 = 0x0004_0000_00F2_3F00;

    let path = std::env::var_os("ACERIS_COOKIE_CLICKER_CIA")
        .map(PathBuf::from)
        .expect("set ACERIS_COOKIE_CLICKER_CIA to the local Cookie Clicker .cia path");

    let mut archive = CiaArchiveReader::open(File::open(&path).unwrap()).unwrap();
    let title_id = archive.title_metadata().title_id;
    let boot_content_index = archive.title_metadata().boot_content_index;

    assert_eq!(title_id, EXPECTED_TITLE_ID);
    assert_eq!(boot_content_index, 0);
    let ticket = archive.ticket().expect("the CIA contains a ticket");
    assert_eq!(ticket.title_id, title_id);

    let location = archive
        .content_location(boot_content_index)
        .expect("the boot content is selected by the CIA bitmap");
    assert_eq!(
        location.content_type & CONTENT_TYPE_ENCRYPTED,
        0,
        "the Cookie Clicker boot content is expected to be stored without outer CIA encryption"
    );

    {
        let ncch = archive
            .open_verified_ncch_content_reader(boot_content_index)
            .unwrap()
            .expect("the boot content contains an NCCH");
        assert_eq!(ncch.header().program_id, title_id);
        assert!(ncch.header().declares_no_crypto());

        let address_space = ncch.load_arm11_address_space([0; 16], [0; 16]).unwrap();
        assert!(
            address_space
                .service_access_control()
                .is_some_and(|access| access.allows("dsp::DSP")),
            "Cookie Clicker NCCH ACI must grant dsp::DSP"
        );
        eprintln!(
            "Prepared Cookie Clicker ARM11 image: entry={:#010x}, stack_top={:#010x}, stack_size={:#x}",
            address_space.layout().entry_point,
            address_space.layout().initial_stack_pointer,
            address_space.layout().stack_size_bytes
        );
    }

    let mut system = System::new();
    assert_eq!(
        system.load_executable_file(&path).unwrap(),
        ImageFormat::Cia
    );
    assert_eq!(system.image(), Some(ImageFormat::Cia));

    let frame = system.run_frame_with_budget(50_000).unwrap();
    assert!(
        frame.instructions_executed > 0,
        "the loaded Cookie Clicker ARM11 guest did not execute any instructions"
    );
    eprintln!(
        "Cookie Clicker frame: instructions={}, CPU stop={:?}, GSP command lists={}, LCD scanouts={}",
        frame.instructions_executed,
        frame.cpu_stop_reason,
        frame.gpu_command_lists_submitted,
        frame.lcd_screens_scanned_out
    );
}
