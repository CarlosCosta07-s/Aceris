//! Independent integration tests for ARMv6 LDM/STM User-bank transfer semantics.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x100;

fn user_values() -> [u32; 7] {
    [
        0xA000_0008,
        0xA000_0009,
        0xA000_000A,
        0xA000_000B,
        0xA000_000C,
        0xA000_000D,
        0xA000_000E,
    ]
}

fn fiq_values() -> [u32; 7] {
    [
        0xF000_0008,
        0xF000_0009,
        0xF000_000A,
        0xF000_000B,
        0xF000_000C,
        0xF000_000D,
        0xF000_000E,
    ]
}

fn enter_fiq_with_distinct_user_and_fiq_banks(cpu: &mut Arm11Core) {
    let user = user_values();
    cpu.regs[8..15].copy_from_slice(&user);
    cpu.switch_mode(Psr::FIQ_MODE).unwrap();
    cpu.regs[8..15].copy_from_slice(&fiq_values());
    cpu.regs[0] = DATA;
}

#[test]
fn stm_user_bank_stores_user_registers_while_preserving_active_fiq_registers() {
    let mut memory = FlatMemory::new(0, 0x200);
    memory.write32(0, 0xE8C0_7F00).unwrap(); // STMIA r0, {r8-r14}^

    let mut cpu = Arm11Core::new();
    enter_fiq_with_distinct_user_and_fiq_banks(&mut cpu);
    let fiq_before = cpu.regs[8..15].to_vec();

    cpu.step(&mut memory).unwrap();

    for (lane, expected) in user_values().iter().enumerate() {
        assert_eq!(
            memory.read32(DATA + (lane as u32 * 4)).unwrap(),
            *expected,
            "User bank register r{}",
            lane + 8
        );
    }
    assert_eq!(&cpu.regs[8..15], fiq_before.as_slice());
    assert_eq!(cpu.regs[0], DATA);
    assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);
}

#[test]
fn ldm_user_bank_loads_user_registers_without_overwriting_active_fiq_bank() {
    let mut memory = FlatMemory::new(0, 0x200);
    memory.write32(0, 0xE8D0_7F00).unwrap(); // LDMIA r0, {r8-r14}^
    for (lane, value) in user_values().iter().enumerate() {
        memory.write32(DATA + (lane as u32 * 4), *value).unwrap();
    }

    let mut cpu = Arm11Core::new();
    enter_fiq_with_distinct_user_and_fiq_banks(&mut cpu);
    let fiq_before = cpu.regs[8..15].to_vec();

    cpu.step(&mut memory).unwrap();

    assert_eq!(&cpu.regs[8..15], fiq_before.as_slice());
    assert_eq!(cpu.regs[0], DATA);
    assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);

    cpu.switch_mode(Psr::USER_MODE).unwrap();
    assert_eq!(cpu.regs[8..15], user_values());
}

#[test]
fn user_bank_transfer_rejects_writeback_and_user_mode_execution() {
    let mut memory = FlatMemory::new(0, 0x200);
    let mut cpu = Arm11Core::new();
    enter_fiq_with_distinct_user_and_fiq_banks(&mut cpu);
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    // STMIA r0!,{r8}^: writeback is not permitted for a User-bank transfer.
    let writeback_instruction = 0xE8E0_0100;
    memory.write32(0, writeback_instruction).unwrap();
    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == writeback_instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(memory.read32(DATA).unwrap(), 0);

    // LDMIA r0,{r8}^ is not available in User mode.
    cpu.switch_mode(Psr::USER_MODE).unwrap();
    let user_registers_before = cpu.regs;
    let user_status_before = cpu.cpsr;
    let user_instruction = 0xE8D0_0100;
    memory.write32(0, user_instruction).unwrap();
    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == user_instruction
    ));
    assert_eq!(cpu.regs, user_registers_before);
    assert_eq!(cpu.cpsr, user_status_before);
}
