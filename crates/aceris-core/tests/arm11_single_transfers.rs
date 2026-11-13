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
    cpu.regs[5] = DATA + 0x20;
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

    assert!(
        matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ),
        "{instruction:#010x}"
    );

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
fn halfword_register_transfers_reject_pc_as_the_base() {
    // LDRH/LDRSB/LDRSH r0,[pc,r2] ; STRH r0,[pc,r2] ; STRH r0,[pc,#0].
    for instruction in [
        0xE19F_00B2,
        0xE19F_00D2,
        0xE19F_00F2,
        0xE18F_00B2,
        0xE1CF_00B0,
    ] {
        assert_undefined_without_side_effects(instruction);
    }
}

#[test]
fn signed_halfword_and_doubleword_pc_relative_literal_loads_remain_valid() {
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, 0xE1DF_08D0).unwrap(); // LDRSB r0,[pc,#0x80]
    memory.write32(4, 0xE1DF_18F0).unwrap(); // LDRSH r1,[pc,#0x80]
    memory.write32(8, 0xE1CF_28D0).unwrap(); // LDRD r2,r3,[pc,#0x80]
    memory.write8(0x88, 0x80).unwrap();
    memory.write16(0x8C, 0x8001).unwrap();
    memory.write32(0x90, 0x1122_3344).unwrap();
    memory.write32(0x94, 0x5566_7788).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[0], 0xFFFF_FF80);
    assert_eq!(cpu.regs[1], 0xFFFF_8001);
    assert_eq!(cpu.regs[2], 0x1122_3344);
    assert_eq!(cpu.regs[3], 0x5566_7788);
    assert_eq!(cpu.pc(), 12);
}

#[test]
fn halfword_transfers_reject_pc_offsets_and_invalid_writeback_bases() {
    // LDRH r0,[r1,pc] ; LDRH r0,[pc,#0]! ; LDRH r1,[r1,#0]!.
    for instruction in [0xE191_00BF, 0xE1FF_00B0, 0xE1F1_10B0] {
        assert_undefined_without_side_effects(instruction);
    }
}

#[test]
fn pc_relative_literal_halfword_load_without_writeback_remains_valid() {
    let instruction = 0xE1DF_00B0; // LDRH r0,[pc,#0]
    let (mut cpu, mut memory) = machine(instruction);
    memory.write16(8, 0xABCD).unwrap();

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[0], 0xABCD);
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.regs[1], DATA);
}

#[test]
fn doubleword_transfers_reject_writeback_aliases_and_overlapping_offsets() {
    // LDRD r4,r5,[r1,r4] ; LDRD r4,r5,[r1,r5] ;
    // LDRD r4,r5,[r5,#4]! ; STRD r4,r5,[r5,#4]!.
    // The final form uses reserved P=0,W=1.
    for instruction in [
        0xE181_40D4,
        0xE181_40D5,
        0xE1E5_40D4,
        0xE1E5_40F4,
        0xE0E1_40D0,
    ] {
        assert_undefined_without_side_effects(instruction);
    }
}

#[test]
fn strd_register_offset_allows_rm_to_alias_either_transfer_source() {
    // STRD r4,r5,[r1,r4] and STRD r4,r5,[r1,r5].
    for (instruction, first, second) in
        [(0xE181_40F4, 8, 0x5566_7788), (0xE181_40F5, 0x1122_3344, 8)]
    {
        let (mut cpu, mut memory) = machine(instruction);
        cpu.regs[4] = first;
        cpu.regs[5] = second;
        cpu.step(&mut memory).unwrap();

        assert_eq!(memory.read32(DATA + 8).unwrap(), first);
        assert_eq!(memory.read32(DATA + 12).unwrap(), second);
        assert_eq!(cpu.regs[1], DATA);
    }
}

#[test]
fn doubleword_load_without_writeback_allows_base_to_alias_second_destination() {
    let instruction = 0xE1C5_40D0; // LDRD r4,r5,[r5]
    let (mut cpu, mut memory) = machine(instruction);
    cpu.regs[5] = DATA;
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[4], INITIAL_WORD);
    assert_eq!(cpu.regs[5], 0x5566_7788);
    assert_eq!(cpu.pc(), 4);
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
