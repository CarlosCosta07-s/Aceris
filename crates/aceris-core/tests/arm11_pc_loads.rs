//! Independent integration tests for A32 loads that target the program counter.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x400;

fn machine(instruction: u32, loaded_value: u32) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, instruction).unwrap();
    memory.write32(DATA, loaded_value).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[1] = DATA;
    (cpu, memory)
}

#[test]
fn ldr_pc_rejects_halfword_aligned_arm_target_without_writeback_or_branch() {
    let instruction = 0xE591_F000; // LDR pc, [r1]
    let (mut cpu, mut memory) = machine(instruction, 0x42);
    let registers_before = cpu.regs;
    let data_before = memory.read32(DATA).unwrap();

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(memory.read32(DATA).unwrap(), data_before);
    assert!(!cpu.cpsr.thumb());
}

#[test]
fn ldm_pc_rejects_halfword_aligned_arm_target_before_writeback() {
    let instruction = 0xE8B1_8000; // LDMIA r1!, {pc}
    let (mut cpu, mut memory) = machine(instruction, 0x42);
    let registers_before = cpu.regs;
    let data_before = memory.read32(DATA).unwrap();

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(memory.read32(DATA).unwrap(), data_before);
}

#[test]
fn word_pc_loads_still_interwork_to_thumb_targets() {
    for target in [0x41, 0x43] {
        let mut load = machine(0xE591_F000, target);
        load.0.step(&mut load.1).unwrap();
        assert!(load.0.cpsr.thumb());
        assert_eq!(load.0.pc(), target & !1);
    }
}

#[test]
fn halfword_and_signed_byte_loads_reject_pc_as_the_destination() {
    // LDRH pc,[r1] ; LDRSB pc,[r1] ; LDRSH pc,[r1].
    for instruction in [0xE1D1_F0B0, 0xE1D1_F0D0, 0xE1D1_F0F0] {
        let (mut cpu, mut memory) = machine(instruction, 0x1234_5678);
        let registers_before = cpu.regs;
        let data_before = memory.read32(DATA).unwrap();

        assert!(matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(memory.read32(DATA).unwrap(), data_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
    }
}
