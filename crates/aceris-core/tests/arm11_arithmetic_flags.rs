//! Differential tests of ARM/Thumb arithmetic results and C/V propagation.
//!
//! The reference model below deliberately uses widened i64 arithmetic and
//! never shares code with the interpreter's `add_with_carry`.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

const GRID: [u32; 12] = [
    0,
    1,
    2,
    0x7FFF_FFFE,
    0x7FFF_FFFF,
    0x8000_0000,
    0x8000_0001,
    0xFFFF_FFFE,
    0xFFFF_FFFF,
    0x0000_FFFF,
    0x1234_5678,
    0xDEAD_BEEF,
];

#[derive(Clone, Copy)]
enum Op {
    Add,
    Adc,
    Sub,
    Sbc,
    Rsb,
    Rsc,
    Cmp,
    Cmn,
}

/// Returns (result, carry, overflow); carry follows ARM's "not borrow" rule.
fn reference(op: Op, a: u32, b: u32, c: bool) -> (u32, bool, bool) {
    let (ua, ub) = (i64::from(a), i64::from(b));
    let (sa, sb) = (i64::from(a as i32), i64::from(b as i32));
    let cin = i64::from(c);
    let (unsigned, signed) = match op {
        Op::Add | Op::Cmn => (ua + ub, sa + sb),
        Op::Adc => (ua + ub + cin, sa + sb + cin),
        Op::Sub | Op::Cmp => (ua - ub, sa - sb),
        Op::Sbc => (ua - ub - (1 - cin), sa - sb - (1 - cin)),
        Op::Rsb => (ub - ua, sb - sa),
        Op::Rsc => (ub - ua - (1 - cin), sb - sa - (1 - cin)),
    };
    let carry = match op {
        Op::Add | Op::Adc | Op::Cmn => unsigned > i64::from(u32::MAX),
        _ => unsigned >= 0,
    };
    let overflow = signed < i64::from(i32::MIN) || signed > i64::from(i32::MAX);
    (unsigned as u32, carry, overflow)
}

fn arm_opcode(op: Op) -> u32 {
    match op {
        Op::Sub => 0x2,
        Op::Rsb => 0x3,
        Op::Add => 0x4,
        Op::Adc => 0x5,
        Op::Sbc => 0x6,
        Op::Rsc => 0x7,
        Op::Cmp => 0xA,
        Op::Cmn => 0xB,
    }
}

fn cpu_with(a: u32, b: u32, c: bool, v: bool) -> Arm11Core {
    let mut cpu = Arm11Core::new();
    cpu.regs[0] = a;
    cpu.regs[1] = b;
    cpu.cpsr.set_flags(false, false, c, v);
    cpu
}

fn flags(cpu: &Arm11Core) -> (bool, bool, bool, bool) {
    (cpu.cpsr.n(), cpu.cpsr.z(), cpu.cpsr.c(), cpu.cpsr.v())
}

#[test]
fn arm_register_arithmetic_matches_reference_on_edge_grid() {
    let ops = [
        Op::Add,
        Op::Adc,
        Op::Sub,
        Op::Sbc,
        Op::Rsb,
        Op::Rsc,
        Op::Cmp,
        Op::Cmn,
    ];
    for op in ops {
        let compare = matches!(op, Op::Cmp | Op::Cmn);
        // <op>S r2, r0, r1 (compares have Rd = 0 by convention).
        let rd = if compare { 0 } else { 2 };
        let word = 0xE000_0001 | (arm_opcode(op) << 21) | (1 << 20) | (rd << 12);
        for &a in &GRID {
            for &b in &GRID {
                for c in [false, true] {
                    let mut memory = FlatMemory::new(0, 4);
                    memory.write32(0, word).unwrap();
                    let mut cpu = cpu_with(a, b, c, false);
                    cpu.step(&mut memory).unwrap();
                    let (value, carry, overflow) = reference(op, a, b, c);
                    let n = value >> 31 != 0;
                    assert_eq!(
                        flags(&cpu),
                        (n, value == 0, carry, overflow),
                        "{word:#010x} a={a:#x} b={b:#x} c={c}"
                    );
                    if !compare {
                        assert_eq!(cpu.regs[2], value, "{word:#010x} a={a:#x} b={b:#x}");
                    }
                }
            }
        }
    }
}

#[test]
fn arm_arithmetic_without_s_bit_leaves_flags_untouched() {
    for op in [Op::Add, Op::Adc, Op::Sub, Op::Sbc, Op::Rsb, Op::Rsc] {
        let word = 0xE000_0001 | (arm_opcode(op) << 21) | (2 << 12);
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, word).unwrap();
        let mut cpu = cpu_with(u32::MAX, 1, true, true);
        cpu.cpsr.set_flags(true, true, true, true);
        cpu.step(&mut memory).unwrap();
        assert_eq!(flags(&cpu), (true, true, true, true), "{word:#010x}");
    }
}

#[test]
fn arm_logical_flag_forms_take_shifter_carry_and_keep_overflow() {
    // MOVS r2, r1, LSL #1 : carry is the bit shifted out, V is preserved.
    for v in [false, true] {
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, 0xE1B0_2081).unwrap();
        let mut cpu = cpu_with(0, 0x8000_0000, false, v);
        cpu.step(&mut memory).unwrap();
        assert_eq!(cpu.regs[2], 0);
        assert_eq!(flags(&cpu), (false, true, true, v));
    }
    // ANDS with an immediate that has no rotation keeps C.
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, 0xE210_20FF).unwrap();
    let mut cpu = cpu_with(0x8000_0000, 0, true, true);
    cpu.step(&mut memory).unwrap();
    assert_eq!(flags(&cpu), (false, true, true, true));
}

#[test]
fn arm_multiply_sets_only_n_and_z() {
    // MULS r2, r0, r1 ; MLAS r2, r0, r1, r3.
    for (word, expected) in [(0xE012_0190u32, 0x8000_0000u32), (0xE032_3190, 0x8000_0001)] {
        for (c, v) in [(false, false), (true, true), (true, false), (false, true)] {
            let mut memory = FlatMemory::new(0, 4);
            memory.write32(0, word).unwrap();
            let mut cpu = cpu_with(0x4000_0000, 2, c, v);
            cpu.regs[3] = 1;
            cpu.step(&mut memory).unwrap();
            assert_eq!(cpu.regs[2], expected);
            assert_eq!(flags(&cpu), (true, false, c, v));
        }
    }
}

#[test]
fn arm_long_multiply_matches_reference_and_preserves_c_v() {
    // UMULLS/SMULLS/UMLALS/SMLALS RdLo=r2, RdHi=r3, Rm=r0, Rs=r1.
    let forms = [(0xE093_2190u32, false, false), (0xE0D3_2190, true, false)];
    let acc_forms = [(0xE0B3_2190u32, false, true), (0xE0F3_2190, true, true)];
    for (word, signed, accumulate) in forms.into_iter().chain(acc_forms) {
        for &a in &GRID {
            for &b in &GRID {
                let mut memory = FlatMemory::new(0, 4);
                memory.write32(0, word).unwrap();
                let mut cpu = cpu_with(a, b, true, true);
                cpu.regs[2] = 0xFFFF_FFFF;
                cpu.regs[3] = 0x8000_0000;
                cpu.step(&mut memory).unwrap();
                let product = if signed {
                    (i128::from(a as i32) * i128::from(b as i32)) as u64
                } else {
                    (u128::from(a) * u128::from(b)) as u64
                };
                let acc = if accumulate {
                    0x8000_0000_FFFF_FFFFu64
                } else {
                    0
                };
                let expected = product.wrapping_add(acc);
                let got = u64::from(cpu.regs[3]) << 32 | u64::from(cpu.regs[2]);
                assert_eq!(got, expected, "{word:#010x} a={a:#x} b={b:#x}");
                assert_eq!(
                    flags(&cpu),
                    (expected >> 63 != 0, expected == 0, true, true),
                    "{word:#010x} a={a:#x} b={b:#x}"
                );
            }
        }
    }
}

fn run_thumb(half: u16, a: u32, b: u32, c: bool, v: bool) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write16(0, half).unwrap();
    let mut cpu = cpu_with(a, b, c, v);
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0);
    cpu.step(&mut memory).unwrap();
    cpu
}

#[test]
fn thumb_low_register_arithmetic_matches_reference_on_edge_grid() {
    // (halfword with Rd=r0 Rs=r1, op, reads old r0 as a, result goes to r0?)
    let cases = [
        (0x4148u16, Op::Adc, true), // ADCS r0, r1
        (0x4188, Op::Sbc, true),    // SBCS r0, r1
        (0x4288, Op::Cmp, false),   // CMP r0, r1
        (0x42C8, Op::Cmn, false),   // CMN r0, r1
    ];
    for (half, op, writes) in cases {
        for &a in &GRID {
            for &b in &GRID {
                for c in [false, true] {
                    let cpu = run_thumb(half, a, b, c, false);
                    let (value, carry, overflow) = reference(op, a, b, c);
                    assert_eq!(
                        flags(&cpu),
                        (value >> 31 != 0, value == 0, carry, overflow),
                        "{half:#06x} a={a:#x} b={b:#x} c={c}"
                    );
                    assert_eq!(cpu.regs[0], if writes { value } else { a });
                }
            }
        }
    }
    // NEGS r0, r1 is RSBS r0, r1, #0.
    for &b in &GRID {
        let cpu = run_thumb(0x4248, 0xAAAA_AAAA, b, false, false);
        let (value, carry, overflow) = reference(Op::Rsb, b, 0, false);
        assert_eq!(cpu.regs[0], value);
        assert_eq!(flags(&cpu), (value >> 31 != 0, value == 0, carry, overflow));
    }
}

#[test]
fn thumb_add_sub_forms_match_reference() {
    for &a in &GRID {
        for &b in &GRID {
            // ADDS r2, r0, r1 / SUBS r2, r0, r1 (register forms).
            for (half, op) in [(0x1842u16, Op::Add), (0x1A42, Op::Sub)] {
                let mut memory = FlatMemory::new(0, 4);
                memory.write16(0, half).unwrap();
                let mut cpu = cpu_with(a, b, false, false);
                cpu.cpsr.set_thumb(true);
                cpu.set_pc(0);
                cpu.step(&mut memory).unwrap();
                let (value, carry, overflow) = reference(op, a, b, false);
                assert_eq!(cpu.regs[2], value);
                assert_eq!(flags(&cpu), (value >> 31 != 0, value == 0, carry, overflow));
            }
        }
        // Immediate forms: ADDS/SUBS r2, r0, #7 and ADDS/SUBS/CMP r0, #200.
        for (half, op, imm, dest) in [
            (0x1DC2u16, Op::Add, 7u32, 2usize),
            (0x1FC2, Op::Sub, 7, 2),
            (0x30C8, Op::Add, 200, 0),
            (0x38C8, Op::Sub, 200, 0),
        ] {
            let cpu = run_thumb(half, a, 0, true, true);
            let (value, carry, overflow) = reference(op, a, imm, true);
            assert_eq!(cpu.regs[dest], value, "{half:#06x} a={a:#x}");
            assert_eq!(flags(&cpu), (value >> 31 != 0, value == 0, carry, overflow));
        }
        let cpu = run_thumb(0x28C8, a, 0, false, false); // CMP r0, #200
        let (value, carry, overflow) = reference(Op::Cmp, a, 200, false);
        assert_eq!(cpu.regs[0], a);
        assert_eq!(flags(&cpu), (value >> 31 != 0, value == 0, carry, overflow));
    }
}

#[test]
fn thumb_high_register_add_and_mov_do_not_touch_flags() {
    // ADD r0, r8 : T2 high-register form.
    let mut memory = FlatMemory::new(0, 4);
    memory.write16(0, 0x4440).unwrap();
    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0);
    cpu.regs[0] = u32::MAX;
    cpu.regs[8] = 2;
    cpu.cpsr.set_flags(true, true, false, true);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[0], 1);
    assert_eq!(flags(&cpu), (true, true, false, true));
}

#[test]
fn thumb_mul_and_logical_ops_keep_c_and_v() {
    // MULS r0, r1 (Rd=r0, Rm=r1): halfword 0x4348.
    for (c, v) in [(false, true), (true, false)] {
        let cpu = run_thumb(0x4348, 0x4000_0000, 2, c, v);
        assert_eq!(cpu.regs[0], 0x8000_0000);
        assert_eq!(flags(&cpu), (true, false, c, v));
    }
}
fn arm_immediate_instruction(op: Op, rotate: u32, immediate: u32) -> u32 {
    let compare = matches!(op, Op::Cmp | Op::Cmn);
    let rd = if compare { 0 } else { 2 };
    0xE200_0000 | (arm_opcode(op) << 21) | (1 << 20) | (rd << 12) | (rotate << 8) | immediate
}

#[test]
fn arm_immediate_arithmetic_matches_reference_for_rotated_operands_and_carry_inputs() {
    // A32 Operand2 is ROR(Imm8, 2 * rotate). Include rotated zero values:
    // arithmetic instructions must report the ALU carry, not shifter carry.
    const IMMEDIATES: [(u32, u32); 9] = [
        (0, 0x00),
        (0, 0x01),
        (0, 0xFF),
        (1, 0x80),
        (3, 0xA5),
        (4, 0xFF),
        (7, 0x80),
        (8, 0x00),
        (12, 0x01),
    ];
    let ops = [
        Op::Add,
        Op::Adc,
        Op::Sub,
        Op::Sbc,
        Op::Rsb,
        Op::Rsc,
        Op::Cmp,
        Op::Cmn,
    ];

    for op in ops {
        let compare = matches!(op, Op::Cmp | Op::Cmn);
        for &a in &GRID {
            for (rotate, imm8) in IMMEDIATES {
                let operand = imm8.rotate_right(rotate * 2);
                for carry_in in [false, true] {
                    for overflow_in in [false, true] {
                        let word = arm_immediate_instruction(op, rotate, imm8);
                        let mut memory = FlatMemory::new(0, 4);
                        memory.write32(0, word).unwrap();

                        let mut cpu = cpu_with(a, 0, carry_in, overflow_in);
                        cpu.set_pc(0);
                        cpu.step(&mut memory).unwrap();

                        let (value, carry, overflow) = reference(op, a, operand, carry_in);
                        let expected_n = value >> 31 != 0;
                        assert_eq!(
                            flags(&cpu),
                            (expected_n, value == 0, carry, overflow),
                            "{word:#010x} a={a:#010x} operand={operand:#010x} carry_in={carry_in} overflow_in={overflow_in}"
                        );
                        if compare {
                            assert_eq!(cpu.regs[0], a, "{word:#010x}");
                        } else {
                            assert_eq!(
                                cpu.regs[2], value,
                                "{word:#010x} a={a:#010x} operand={operand:#010x}"
                            );
                        }
                    }
                }
            }
        }
    }
}
fn arm_immediate_logical_instruction(opcode: u32, rotate: u32, immediate: u32) -> u32 {
    let compare = matches!(opcode, 0x8 | 0x9);
    let rd = if compare { 0 } else { 2 };
    0xE200_0000 | (opcode << 21) | (1 << 20) | (rd << 12) | (rotate << 8) | immediate
}

#[test]
fn arm_immediate_logical_flags_match_rotated_shifter_carry_reference() {
    const IMMEDIATES: [(u32, u32); 9] = [
        (0, 0x00),
        (0, 0x81),
        (1, 0x80),
        (2, 0x01),
        (4, 0xFF),
        (7, 0x80),
        (8, 0x00),
        (12, 0x01),
        (15, 0x80),
    ];
    // AND, EOR, TST, TEQ, ORR, MOV, BIC and MVN.
    const OPCODES: [u32; 8] = [0x0, 0x1, 0x8, 0x9, 0xC, 0xD, 0xE, 0xF];

    for opcode in OPCODES {
        let compare = matches!(opcode, 0x8 | 0x9);
        for &a in &GRID {
            for (rotate, imm8) in IMMEDIATES {
                let operand = imm8.rotate_right(rotate * 2);
                let result = match opcode {
                    0x0 | 0x8 => a & operand,
                    0x1 | 0x9 => a ^ operand,
                    0xC => a | operand,
                    0xD => operand,
                    0xE => a & !operand,
                    0xF => !operand,
                    _ => unreachable!(),
                };
                let expected_carry = if rotate == 0 {
                    None
                } else {
                    Some(operand >> 31 != 0)
                };

                for carry_in in [false, true] {
                    for overflow_in in [false, true] {
                        let instruction = arm_immediate_logical_instruction(opcode, rotate, imm8);
                        let mut memory = FlatMemory::new(0, 4);
                        memory.write32(0, instruction).unwrap();

                        let mut cpu = cpu_with(a, 0, carry_in, overflow_in);
                        cpu.set_pc(0);
                        cpu.step(&mut memory).unwrap();

                        let carry = expected_carry.unwrap_or(carry_in);
                        assert_eq!(
                            flags(&cpu),
                            (result >> 31 != 0, result == 0, carry, overflow_in),
                            "{instruction:#010x} a={a:#010x} operand={operand:#010x} carry_in={carry_in} overflow_in={overflow_in}"
                        );
                        if compare {
                            assert_eq!(cpu.regs[0], a, "{instruction:#010x}");
                        } else {
                            assert_eq!(
                                cpu.regs[2], result,
                                "{instruction:#010x} a={a:#010x} operand={operand:#010x}"
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn arm_add_uses_instruction_address_plus_eight_when_pc_is_rn() {
    // ADD r2, pc, #4: PC is the address of the instruction plus eight.
    let base = 0x8000_0000;
    let mut memory = FlatMemory::new(base, 4);
    memory.write32(base, 0xE28F_2004).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(base);
    cpu.cpsr.set_flags(true, false, true, false);
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[2], base.wrapping_add(8 + 4));
    assert_eq!(flags(&cpu), (true, false, true, false));
    assert_eq!(cpu.pc(), base + 4);

    // ADDS r2, pc, #0x80 crosses INT32_MAX using PC+8 as Rn.
    let base = 0x7FFF_FF80;
    let mut memory = FlatMemory::new(base, 4);
    memory.write32(base, 0xE29F_2080).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(base);
    cpu.cpsr.set_flags(false, false, false, false);
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.regs[2], 0x8000_0008);
    assert_eq!(flags(&cpu), (true, false, false, true));
    assert_eq!(cpu.pc(), base + 4);
}
