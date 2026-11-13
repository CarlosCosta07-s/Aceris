//! Exhaustive A32 conditional-execution checks over every NZCV combination.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

fn condition_passes(condition: u32, n: bool, z: bool, c: bool, v: bool) -> bool {
    match condition {
        0x0 => z,
        0x1 => !z,
        0x2 => c,
        0x3 => !c,
        0x4 => n,
        0x5 => !n,
        0x6 => v,
        0x7 => !v,
        0x8 => c && !z,
        0x9 => !c || z,
        0xA => n == v,
        0xB => n != v,
        0xC => !z && n == v,
        0xD => z || n != v,
        0xE => true,
        _ => unreachable!("0xF selects the unconditional encoding space"),
    }
}

fn flags_from_index(index: u32) -> (bool, bool, bool, bool) {
    (
        index & 0b1000 != 0,
        index & 0b0100 != 0,
        index & 0b0010 != 0,
        index & 0b0001 != 0,
    )
}

#[test]
fn all_a32_conditions_match_every_nzcv_combination() {
    // MOV{cond} r2, #0x5A; S=0 must preserve all CPSR bits.
    const MOV_IMMEDIATE_R2: u32 = 0x03A2_005A;
    const INITIAL_R2: u32 = 0xA55A_3CC3;

    for condition in 0..=0xE {
        for flags_index in 0..16 {
            let (n, z, c, v) = flags_from_index(flags_index);
            let expected_to_execute = condition_passes(condition, n, z, c, v);
            let instruction = (condition << 28) | MOV_IMMEDIATE_R2;

            let mut memory = FlatMemory::new(0, 4);
            memory.write32(0, instruction).unwrap();

            let mut cpu = Arm11Core::new();
            cpu.set_pc(0);
            cpu.regs[2] = INITIAL_R2;
            cpu.cpsr.set_flags(n, z, c, v);
            let status_before = cpu.cpsr;

            cpu.step(&mut memory).unwrap();

            assert_eq!(
                cpu.regs[2],
                if expected_to_execute {
                    0x5A
                } else {
                    INITIAL_R2
                },
                "condition={condition:#x} NZCV={n}{z}{c}{v}"
            );
            assert_eq!(
                cpu.cpsr, status_before,
                "condition={condition:#x} NZCV={n}{z}{c}{v}"
            );
            assert_eq!(cpu.pc(), 4, "condition={condition:#x} NZCV={n}{z}{c}{v}");
        }
    }
}

#[test]
fn failed_a32_conditions_suppress_undefined_instruction_exceptions() {
    // TST with S=0 occupies a reserved A32 miscellaneous encoding.
    const RESERVED_COMPARE_ENCODING: u32 = 0x0100_0000;

    for condition in 0..=0xD {
        for flags_index in 0..16 {
            let (n, z, c, v) = flags_from_index(flags_index);
            let should_execute = condition_passes(condition, n, z, c, v);
            let instruction = (condition << 28) | RESERVED_COMPARE_ENCODING;

            let mut memory = FlatMemory::new(0, 4);
            memory.write32(0, instruction).unwrap();

            let mut cpu = Arm11Core::new();
            cpu.set_pc(0);
            cpu.regs[0] = 0x1234_5678;
            cpu.cpsr.set_flags(n, z, c, v);
            let registers_before = cpu.regs;
            let status_before = cpu.cpsr;

            let result = cpu.step(&mut memory);
            if should_execute {
                assert!(
                    matches!(
                        result,
                        Err(Error::UndefinedInstruction { pc: 0, word })
                            if word == instruction
                    ),
                    "condition={condition:#x} NZCV={n}{z}{c}{v}"
                );
                assert_eq!(cpu.regs, registers_before);
                assert_eq!(cpu.cpsr, status_before);
                assert_eq!(cpu.pc(), 0);
            } else {
                assert!(
                    result.is_ok(),
                    "failed condition must suppress decode: condition={condition:#x} NZCV={n}{z}{c}{v}: {result:?}"
                );
                assert_eq!(cpu.regs, registers_before);
                assert_eq!(cpu.cpsr, status_before);
                assert_eq!(cpu.pc(), 4);
            }
        }
    }
}

#[test]
fn unconditional_condition_code_is_not_part_of_the_conditional_table() {
    // 0xF is the unconditional encoding space, not an ordinary AL condition.
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xF100_0000).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.cpsr = Psr::default();

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction {
            pc: 0,
            word: 0xF100_0000
        })
    ));
    assert_eq!(cpu.pc(), 0);
}
