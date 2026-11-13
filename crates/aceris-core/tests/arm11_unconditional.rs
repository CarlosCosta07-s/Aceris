//! Independent integration tests for the A32 unconditional instruction space.

use aceris_core::Error;
use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

fn execute(instruction: u32) -> (Arm11Core, aceris_core::Result<aceris_core::cpu::StepEvent>) {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, instruction).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    let result = cpu.step(&mut memory);
    (cpu, result)
}

#[test]
fn reserved_unconditional_encodings_are_undefined_without_side_effects() {
    for instruction in [
        0xF000_0000, // Reserved unconditional data-processing space.
        0xFE01_0F10, // MCR2 p15, 0, r0, c1, c0, 0.
        0xFE10_0F10, // MRC2 p15, 0, r0, c0, c0, 0.
        0xFE00_0F00, // CDP2 p15, 0, c0, c0, c0, 0.
        0xFC00_0F00, // STC2/LDC2 p15 space.
        0xFC81_2F00, // MCRR2 p15, 0, r2, r1, c0.
        0xFCA1_2F00, // MRRC2 p15, 0, r2, r1, c0.
    ] {
        let (cpu, result) = execute(instruction);
        assert!(
            matches!(
                result,
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
            ),
            "{instruction:#010x}"
        );
        assert_eq!(cpu.pc(), 0, "{instruction:#010x}");
        assert_eq!(cpu.cpsr.mode(), aceris_core::cpu::Psr::USER_MODE);
    }
}

#[test]
fn pld_rejects_pc_as_register_offset_but_keeps_pc_relative_literal_form() {
    let invalid = 0xF751_F00F; // PLD [r1, -pc]
    let (cpu, result) = execute(invalid);
    assert!(matches!(
        result,
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == invalid
    ));
    assert_eq!(cpu.pc(), 0);

    let literal = 0xF5DF_F020; // PLD [pc, #32]
    let (cpu, result) = execute(literal);
    assert!(result.is_ok());
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn unsupported_non_cp15_unconditional_coprocessor_forms_remain_explicit_limits() {
    // CP14 debug coprocessor instructions are not implemented by this core.
    for instruction in [0xFE00_0E00, 0xFC00_0E00, 0xFC81_2E00, 0xFCA1_2E00] {
        let (_, result) = execute(instruction);
        assert!(
            matches!(result, Err(Error::Unimplemented(_))),
            "{instruction:#010x}"
        );
    }
}
