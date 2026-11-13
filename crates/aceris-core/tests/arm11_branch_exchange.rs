//! Independent integration tests for A32 BX/BLX register PC rules.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

#[test]
fn bx_pc_uses_the_architectural_pc_operand_and_remains_valid_in_armv6() {
    let instruction = 0xE12F_FF1F; // BX pc
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, instruction).unwrap();
    memory.write32(8, 0xE1A0_0000).unwrap(); // MOV r0, r0

    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_flags(true, false, true, true);
    let lr_before = cpu.regs[14];
    let flags_before = cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.pc(), 8);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.regs[14], lr_before);
    assert_eq!(
        cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V),
        flags_before
    );
}

#[test]
fn blx_pc_is_rejected_before_link_or_branch_side_effects() {
    let instruction = 0xE12F_FF3F; // BLX pc
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, instruction).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.regs[14] = 0x1234_5678;
    cpu.cpsr.set_flags(true, false, true, true);
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(cpu.pc(), 0);
}

#[test]
fn bxj_uses_the_armv6_trivial_jazelle_fallback_and_interworks_to_thumb() {
    let instruction = 0xE12F_FF20; // BXJ r0
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, instruction).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.regs[0] = 0x21;
    cpu.regs[14] = 0x1234_5678;
    cpu.cpsr.set_flags(true, false, true, true);
    let flags_before = cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.pc(), 0x20);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.regs[14], 0x1234_5678);
    assert_eq!(
        cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V),
        flags_before
    );
}

#[test]
fn bxj_pc_falls_back_to_the_same_pc_relative_arm_branch_as_bx_pc() {
    let instruction = 0xE12F_FF2F; // BXJ pc
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, instruction).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_flags(false, true, true, false);
    let flags_before = cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.pc(), 8);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(
        cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V),
        flags_before
    );
}
