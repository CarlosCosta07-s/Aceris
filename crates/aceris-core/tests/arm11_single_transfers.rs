//! Independent integration tests for A32 single-transfer operand restrictions.

use aceris_core::Error;
use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x400;
const INITIAL_WORD: u32 = 0x1122_3344;

fn machine(instruction: u32) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, instruction).unwrap();
    memory.write32(DATA, INITIAL_WORD).unwrap();
    memory.write32(DATA + 4, 0x5566_7788).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[0] = 0xAABB_CCDD;
    cpu.regs[1] = DATA;
    cpu.regs[2] = 0x0102_0304;
    (cpu, memory)
}

fn assert_undefined_without_side_effects(instruction: u32) {
    let (mut cpu, mut memory) = machine(instruction);
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;
    let data_before = (
        memory.read32(DATA).unwrap(),
        memory.read32(DATA + 4).unwrap(),
    );

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ), "{instruction:#010x}");

    assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
    assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
    assert_eq!(
        (
            memory.read32(DATA).unwrap(),
            memory.read32(DATA + 4).unwrap()
        ),
        data_before,
        "{instruction:#010x}"
    );
}

#[test]
fn writeback_transfers_reject_pc_base_and_base_equal_to_transfer_register() {
    // LDR r1,[pc],#4 ; STR r1,[pc],#4 ;
    // LDR r1,[r1,#4]! ; STR r1,[r1,#4]!
    for instruction in [0xE49F_1004, 0xE48F_1004, 0xE5B1_1004, 0xE5A1_1004] {
        assert_undefined_without_side_effects(instruction);
    }
}

#[test]
fn register_offset_transfers_reject_pc_as_rm() {
    // LDR r0,[r1,pc] ; STR r0,[r1,pc].
    for instruction in [0xE791_000F, 0xE781_000F] {
        assert_undefined_without_side_effects(instruction);
    }
}

#[test]
fn byte_and_halfword_stores_reject_pc_as_the_transfer_register() {
    // STRB pc,[r1] ; STRH pc,[r1].
    for instruction in [0xE5C1_F000, 0xE1C1_F0B0] {
        assert_undefined_without_side_effects(instruction);
    }
}

#[test]
fn pc_relative_literal_word_load_without_writeback_remains_valid() {
    let instruction = 0xE59F_0000; // LDR r0,[pc,#0]
    let (mut cpu, mut memory) = machine(instruction);
    memory.write32(8, 0xDEAD_BEEF).unwrap();

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[0], 0xDEAD_BEEF);
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.regs[1], DATA);
}
