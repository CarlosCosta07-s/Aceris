//! Independent integration tests for the A32 unconditional instruction space.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
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

#[test]
fn cps_rejects_nonzero_fixed_bit_before_user_mode_no_op() {
    // CPS #16 with reserved fixed bit 16 set. Validation must happen even
    // in User mode, where a correctly encoded CPS instruction has no effect.
    const INVALID_CPS: u32 = 0xF103_0010;
    let (cpu, result) = execute(INVALID_CPS);

    assert!(matches!(
        result,
        Err(Error::UndefinedInstruction {
            pc: 0,
            word: INVALID_CPS
        })
    ));
    assert_eq!(cpu.pc(), 0);
    assert_eq!(cpu.cpsr.mode(), aceris_core::cpu::Psr::USER_MODE);
}

#[test]
fn cps_mode_change_and_interrupt_mask_operations_preserve_unselected_state() {
    // CPS #IRQ switches the mode but leaves every A/I/F mask and NZCV flag intact.
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xF102_0012).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.cpsr = Psr(Psr::SUPERVISOR_MODE | Psr::N | Psr::C | Psr::A | Psr::F);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert_eq!(
        cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V),
        Psr::N | Psr::C
    );
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F), Psr::A | Psr::F);
    assert_eq!(cpu.pc(), 4);

    // CPSID i sets only I; the previously-set A/F masks remain set.
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xF10C_0080).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.cpsr = Psr(Psr::SUPERVISOR_MODE | Psr::A | Psr::F);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
    assert_eq!(
        cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F),
        Psr::A | Psr::I | Psr::F
    );

    // CPSIE i clears only I; A/F and the current exception mode are untouched.
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xF108_0080).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.cpsr = Psr(Psr::IRQ_MODE | Psr::A | Psr::I | Psr::F);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F), Psr::A | Psr::F);

    // Correctly encoded CPS is a no-op in User mode.
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xF10C_0080).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.cpsr = Psr(Psr::USER_MODE | Psr::N | Psr::V);
    let status_before = cpu.cpsr;
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn pld_register_offset_accepts_immediate_shift_but_rejects_reserved_bit_four() {
    // LSL #3 with Rm=r2 is valid; the shift amount and type occupy bits 11:5.
    let valid = 0xF7D1_F182;
    let (cpu, result) = execute(valid);
    assert!(result.is_ok());
    assert_eq!(cpu.pc(), 4);

    // Register-controlled shift form (bit[4]=1) is not an Addressing Mode 2 offset.
    let invalid = 0xF7D1_F192;
    let (cpu, result) = execute(invalid);
    assert!(matches!(
        result,
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == invalid
    ));
    assert_eq!(cpu.pc(), 0);
}
