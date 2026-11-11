//! External regression tests for ARM11 execution contracts.
//!
//! These tests intentionally live outside the implementation modules so that
//! the shared CPU API is exercised in the same way as a frontend would use it.

use aceris_core::cpu::{Arm11Core, StepEvent};
use aceris_core::memory::{Bus, FlatMemory};

#[test]
fn arm_add_reports_signed_overflow_without_unsigned_carry() {
    let mut memory = FlatMemory::new(0, 4);
    // ADDS r2, r0, r1
    memory.write32(0, 0xE090_2001).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.regs[0] = 0x7FFF_FFFF;
    cpu.regs[1] = 1;

    assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    assert_eq!(cpu.regs[2], 0x8000_0000);
    assert!(cpu.cpsr.n());
    assert!(!cpu.cpsr.z());
    assert!(!cpu.cpsr.c());
    assert!(cpu.cpsr.v());
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn arm_and_thumb_branch_exchange_switch_instruction_sets() {
    let mut memory = FlatMemory::new(0, 0x40);

    // ARM: BX r1. The odd target selects Thumb at address 0x08.
    memory.write32(0x00, 0xE12F_FF11).unwrap();
    // Thumb: MOVS r2, #7; MOVS r1, #0x10; BX r1.
    memory.write16(0x08, 0x2207).unwrap();
    memory.write16(0x0A, 0x2110).unwrap();
    memory.write16(0x0C, 0x4708).unwrap();
    // ARM: MOV r3, #42.
    memory.write32(0x10, 0xE3A0_302A).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.regs[1] = 0x09;

    assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x08);

    assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    assert_eq!(cpu.regs[2], 7);
    assert_eq!(cpu.pc(), 0x0A);

    assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    assert_eq!(cpu.regs[1], 0x10);
    assert_eq!(cpu.pc(), 0x0C);

    assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);

    assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    assert_eq!(cpu.regs[3], 42);
    assert_eq!(cpu.pc(), 0x14);
}
