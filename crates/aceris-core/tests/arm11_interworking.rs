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

#[test]
fn arm_word_store_and_load_round_trip_through_guest_memory() {
    let mut memory = FlatMemory::new(0, 0x40);
    // MOV r0, #0x5A; MOV r1, #0x20; STR r0, [r1]; LDR r2, [r1].
    memory.write32(0x00, 0xE3A0_005A).unwrap();
    memory.write32(0x04, 0xE3A0_1020).unwrap();
    memory.write32(0x08, 0xE581_0000).unwrap();
    memory.write32(0x0C, 0xE591_2000).unwrap();

    let mut cpu = Arm11Core::new();
    for _ in 0..4 {
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    }

    assert_eq!(memory.read32(0x20).unwrap(), 0x5A);
    assert_eq!(cpu.regs[2], 0x5A);
    assert_eq!(cpu.pc(), 0x10);
}

#[test]
fn thumb_word_store_and_load_round_trip_through_guest_memory() {
    let mut memory = FlatMemory::new(0, 0x40);
    // MOVS r0, #0x5A; MOVS r1, #0x20; STR r0, [r1]; LDR r2, [r1].
    memory.write16(0x00, 0x205A).unwrap();
    memory.write16(0x02, 0x2120).unwrap();
    memory.write16(0x04, 0x6008).unwrap();
    memory.write16(0x06, 0x680A).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_thumb(true);
    for _ in 0..4 {
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
    }

    assert_eq!(memory.read32(0x20).unwrap(), 0x5A);
    assert_eq!(cpu.regs[2], 0x5A);
    assert_eq!(cpu.pc(), 0x08);
}
