//! Independent integration tests for A32 SSAT/USAT immediate ASR encodings.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

fn execute(word: u32, source_register: usize, source: u32, flags: u32) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, word).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[source_register] = source;
    cpu.cpsr.0 = (cpu.cpsr.0 & !(0xF000_0000 | (1 << 27))) | flags;
    cpu.step(&mut memory).unwrap();
    cpu
}

#[test]
fn ssat_asr_one_is_decoded_and_saturates_the_arithmetic_result() {
    let cpu = execute(
        0xE6A7_00D1, // SSAT r0, #8, r1, ASR #1.
        1,
        0x8000_0000,
        0xB000_0000,
    );

    assert_eq!(cpu.regs[0], 0xFFFF_FF80);
    assert!(cpu.cpsr.q());
    assert!(cpu.cpsr.n());
    assert!(!cpu.cpsr.z());
    assert!(cpu.cpsr.c());
    assert!(cpu.cpsr.v());
}

#[test]
fn usat_asr_one_uses_signed_shift_before_unsigned_clamping() {
    let cpu = execute(
        0xE6E7_20D3, // USAT r2, #7, r3, ASR #1.
        3,
        0x4000_0000,
        0x0000_0000,
    );

    assert_eq!(cpu.regs[2], 127);
    assert!(cpu.cpsr.q());
}

#[test]
fn ssat_asr_32_uses_sign_fill_and_does_not_set_q_when_in_range() {
    let cpu = execute(
        0xE6A7_0051, // SSAT r0, #8, r1, ASR #32.
        1,
        0x8000_0000,
        0x0000_0000,
    );

    assert_eq!(cpu.regs[0], u32::MAX);
    assert!(!cpu.cpsr.q());
}

#[test]
fn usat_asr_32_clamps_negative_sign_fill_to_zero() {
    let cpu = execute(
        0xE6E7_2053, // USAT r2, #7, r3, ASR #32.
        3,
        0x8000_0000,
        0x0000_0000,
    );

    assert_eq!(cpu.regs[2], 0);
    assert!(cpu.cpsr.q());
}
