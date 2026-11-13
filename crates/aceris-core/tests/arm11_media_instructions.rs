//! Independent integration tests for ARMv6 A32 CLZ operand restrictions.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

#[test]
fn clz_counts_leading_zeroes_without_modifying_condition_flags() {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xE16F_0F11).unwrap(); // CLZ r0, r1

    let mut cpu = Arm11Core::new();
    cpu.regs[1] = 0x0000_8000;
    cpu.cpsr.set_flags(true, false, true, true);
    let flags_before = cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[0], 16);
    assert_eq!(
        cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V),
        flags_before
    );
}

#[test]
fn clz_rejects_pc_in_either_operand_without_side_effects() {
    // CLZ pc, r1 ; CLZ r0, pc.
    for instruction in [0xE16F_FF11, 0xE16F_0F1F] {
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, instruction).unwrap();

        let mut cpu = Arm11Core::new();
        cpu.regs[0] = 0x1234_5678;
        cpu.regs[1] = 0x0000_8000;
        cpu.cpsr.set_flags(true, false, true, true);
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;

        assert!(matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(cpu.pc(), 0, "{instruction:#010x}");
    }
}
