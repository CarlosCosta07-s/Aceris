//! Independent integration tests for A32 SWP/SWPB operand constraints.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x400;

fn machine(instruction: u32) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x1000);
    memory.write32(0, instruction).unwrap();
    memory.write32(DATA, 0x1122_3344).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[1] = DATA;
    cpu.regs[2] = 0xAABB_CCDD;
    cpu.regs[3] = 0x5566_7788;
    cpu.regs[4] = 0x0000_00AA;
    cpu.regs[5] = DATA;
    (cpu, memory)
}

#[test]
fn swp_allows_rd_to_alias_rm_and_performs_the_atomic_exchange() {
    let instruction = 0xE101_2092; // SWP r2, r2, [r1]
    let (mut cpu, mut memory) = machine(instruction);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[2], 0x1122_3344);
    assert_eq!(memory.read32(DATA).unwrap(), 0xAABB_CCDD);
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn swpb_exchanges_only_the_addressed_byte() {
    let instruction = 0xE145_3094; // SWPB r3, r4, [r5]
    let (mut cpu, mut memory) = machine(instruction);

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[3], 0x44);
    assert_eq!(memory.read32(DATA).unwrap(), 0x1122_33AA);
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn swp_and_swpb_reject_pc_and_base_aliases_without_side_effects() {
    // SWP r0,r2,[pc] ; SWP pc,r2,[r1] ; SWP r0,pc,[r1] ;
    // SWP r1,r2,[r1] ; SWP r0,r1,[r1] ; SWPB r3,pc,[r5].
    for instruction in [
        0xE10F_0092,
        0xE101_F092,
        0xE101_009F,
        0xE101_1092,
        0xE101_0091,
        0xE145_309F,
    ] {
        let (mut cpu, mut memory) = machine(instruction);
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;
        let memory_before = memory.read32(DATA).unwrap();

        assert!(matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ), "{instruction:#010x}");

        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(memory.read32(DATA).unwrap(), memory_before, "{instruction:#010x}");
    }
}

#[test]
fn swp_operand_rejection_does_not_depend_on_current_mode() {
    let instruction = 0xE101_009F; // SWP r0, pc, [r1]
    let (mut cpu, mut memory) = machine(instruction);
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(memory.read32(DATA).unwrap(), 0x1122_3344);
}
