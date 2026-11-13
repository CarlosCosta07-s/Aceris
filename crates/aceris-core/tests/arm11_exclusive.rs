//! Independent integration tests for A32 ARMv6/ARMv6K exclusive accesses.

use aceris_core::Error;
use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

const DATA: u32 = 0x400;
const INITIAL: u32 = 0x1122_3344;

fn machine(program: &[u32]) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x1000);
    for (index, instruction) in program.iter().enumerate() {
        memory.write32(index as u32 * 4, *instruction).unwrap();
    }
    memory.write32(DATA, INITIAL).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[1] = DATA;
    (cpu, memory)
}

#[test]
fn ldrex_allows_base_and_destination_to_be_the_same_register() {
    let (mut cpu, mut memory) = machine(&[0xE191_1F9F]); // LDREX r1,[r1]

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[1], INITIAL);
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn strex_allows_base_and_source_to_be_the_same_register() {
    let (mut cpu, mut memory) = machine(&[
        0xE191_0F9F, // LDREX r0,[r1]
        0xE181_2F91, // STREX r2,r1,[r1]
    ]);

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[2], 0);
    assert_eq!(memory.read32(DATA).unwrap(), DATA);
    assert_eq!(cpu.pc(), 8);
}

#[test]
fn exclusive_instructions_reject_pc_in_every_architectural_operand_without_side_effects() {
    for instruction in [
        0xE191_FF9F, // LDREX pc,[r1] ; PC as Rt.
        0xE19F_0F9F, // LDREX r0,[pc] ; PC as Rn.
        0xE181_FF93, // STREX pc,r3,[r1] ; PC as Rd.
        0xE181_2F9F, // STREX r2,pc,[r1] ; PC as Rt.
        0xE18F_2F93, // STREX r2,r3,[pc] ; PC as Rn.
        0xE1B1_EF9F, // LDREXD lr,lr+1,[r1] ; pair starts at LR.
        0xE1A1_2F9E, // STREXD r2,lr,lr+1,[r1] ; pair starts at LR.
    ] {
        let (mut cpu, mut memory) = machine(&[instruction]);
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;
        let memory_before = memory.read32(DATA).unwrap();

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
            memory.read32(DATA).unwrap(),
            memory_before,
            "{instruction:#010x}"
        );
    }
}
