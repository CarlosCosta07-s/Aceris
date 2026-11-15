use aceris_core::{System, loader::ImageFormat};
use std::io::Cursor;

fn minimal_3dsx(instructions: &[u32]) -> Vec<u8> {
    let code_size = instructions.len() * 4;
    let mut image = vec![0u8; 0x20 + 3 * 8 + code_size];
    image[..4].copy_from_slice(b"3DSX");
    image[0x04..0x06].copy_from_slice(&0x20u16.to_le_bytes());
    image[0x06..0x08].copy_from_slice(&8u16.to_le_bytes());
    image[0x10..0x14].copy_from_slice(&(code_size as u32).to_le_bytes());

    // The file-backed Code segment follows the fixed header and three
    // standard relocation headers.
    let code_start = 0x20 + 3 * 8;
    for (index, instruction) in instructions.iter().copied().enumerate() {
        let offset = code_start + index * 4;
        image[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
    }

    image
}

#[test]
fn launches_3dsx_through_the_arm11_runtime_and_executes_guest_code() {
    let path = std::env::temp_dir().join(format!(
        "aceris-homebrew-runtime-{}.3dsx",
        std::process::id()
    ));
    let image = minimal_3dsx(&[
        0xE3A0_0007, // MOV r0, #7
        0xE3A0_1005, // MOV r1, #5
        0xE080_2001, // ADD r2, r0, r1
    ]);
    std::fs::write(&path, image).unwrap();

    let mut system = System::new();
    let loaded = system.load_executable_file(&path);
    let _ = std::fs::remove_file(&path);

    assert_eq!(loaded.unwrap(), ImageFormat::ThreeDsx);
    assert_eq!(system.cpu().pc(), 0x0010_0000);
    let report = system.run_instructions(3).unwrap();

    assert_eq!(report.instructions_executed, 3);
    assert_eq!(system.cpu().regs[0], 7);
    assert_eq!(system.cpu().regs[1], 5);
    assert_eq!(system.cpu().regs[2], 12);
}

#[test]
fn malformed_3dsx_does_not_replace_the_active_process() {
    let mut system = System::new();
    system
        .load_3dsx_reader(Cursor::new(minimal_3dsx(&[0xE3A0_0007])))
        .unwrap();
    let original_pc = system.cpu().pc();

    assert!(
        system
            .load_3dsx_reader(Cursor::new(b"3DSX".to_vec()))
            .is_err()
    );
    assert_eq!(system.image(), Some(ImageFormat::ThreeDsx));
    assert_eq!(system.cpu().pc(), original_pc);
}
