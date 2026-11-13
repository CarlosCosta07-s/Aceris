//! Differential ARMv6 DSP multiply coverage.
//!
//! Expected values are calculated from signed lane arithmetic and wide integer
//! intermediates, independently of the instruction decoder implementation.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

const HALFWORDS: [i32; 7] = [i16::MIN as i32, -32767, -1, 0, 1, 32766, i16::MAX as i32];

const WORDS: [u32; 8] = [
    0,
    1,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF,
    0x1234_5678,
    0x8765_4321,
    0xA5A5_5A5A,
];

const PACKED: [u32; 8] = [
    0x0000_0000,
    0x0001_0001,
    0xFFFF_FFFF,
    0x7FFF_7FFF,
    0x8000_8000,
    0x7FFF_8000,
    0x8000_7FFF,
    0x1234_FEDC,
];

fn machine(instruction: u32, registers: &[(usize, u32)], q: bool) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, instruction).unwrap();

    let mut cpu = Arm11Core::new();
    for &(register, value) in registers {
        cpu.regs[register] = value;
    }
    cpu.cpsr.set_flags(true, false, true, true);
    if q {
        cpu.cpsr.set_q();
    }
    cpu.step(&mut memory).unwrap();
    cpu
}

fn assert_nzcv_preserved(cpu: &Arm11Core) {
    assert!(cpu.cpsr.n());
    assert!(!cpu.cpsr.z());
    assert!(cpu.cpsr.c());
    assert!(cpu.cpsr.v());
}

fn signed_half(value: u32, top: bool) -> i64 {
    let half = if top { value >> 16 } else { value & 0xFFFF };
    i64::from(half as u16 as i16)
}

fn packed(low: i32, high: i32) -> u32 {
    ((high as i16 as u16 as u32) << 16) | (low as i16 as u16 as u32)
}

fn half_multiply(base: u32, rd: usize, rn: usize, rs: usize, rm: usize, x: bool, y: bool) -> u32 {
    base | ((rd as u32) << 16)
        | ((rn as u32) << 12)
        | ((rs as u32) << 8)
        | (u32::from(y) << 6)
        | (u32::from(x) << 5)
        | rm as u32
}

fn dual_multiply(
    base: u32,
    rd: usize,
    ra: usize,
    rs: usize,
    rm: usize,
    subtract: bool,
    exchange: bool,
) -> u32 {
    base | ((rd as u32) << 16)
        | ((ra as u32) << 12)
        | ((rs as u32) << 8)
        | (u32::from(subtract) << 6)
        | (u32::from(exchange) << 5)
        | rm as u32
}

#[test]
fn smlaxy_matches_signed_halfword_reference_and_sets_only_sticky_q_on_overflow() {
    for x in [false, true] {
        for y in [false, true] {
            let instruction = half_multiply(0xE100_0080, 4, 2, 1, 0, x, y);
            for &rm_low in &HALFWORDS {
                for &rm_high in &HALFWORDS {
                    for &rs_low in &HALFWORDS {
                        for &rs_high in &HALFWORDS {
                            let rm = packed(rm_low, rm_high);
                            let rs = packed(rs_low, rs_high);
                            for accumulator in [i32::MIN, -1, 0, i32::MAX] {
                                for q_before in [false, true] {
                                    let product = i64::from(if x { rm_high } else { rm_low })
                                        * i64::from(if y { rs_high } else { rs_low });
                                    let wide = product + i64::from(accumulator);
                                    let overflow =
                                        wide < i64::from(i32::MIN) || wide > i64::from(i32::MAX);
                                    let cpu = machine(
                                        instruction,
                                        &[(0, rm), (1, rs), (2, accumulator as u32)],
                                        q_before,
                                    );
                                    assert_eq!(cpu.regs[4], wide as u32);
                                    assert_eq!(cpu.cpsr.q(), q_before || overflow);
                                    assert_nzcv_preserved(&cpu);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn smulxy_matches_signed_halfword_products_without_changing_flags() {
    for x in [false, true] {
        for y in [false, true] {
            let instruction = half_multiply(0xE160_0080, 4, 0, 1, 0, x, y);
            for &rm_low in &HALFWORDS {
                for &rm_high in &HALFWORDS {
                    for &rs_low in &HALFWORDS {
                        for &rs_high in &HALFWORDS {
                            let rm = packed(rm_low, rm_high);
                            let rs = packed(rs_low, rs_high);
                            let product = i64::from(if x { rm_high } else { rm_low })
                                * i64::from(if y { rs_high } else { rs_low });
                            let cpu = machine(instruction, &[(0, rm), (1, rs)], true);
                            assert_eq!(cpu.regs[4], product as u32);
                            assert!(cpu.cpsr.q());
                            assert_nzcv_preserved(&cpu);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn smlalxy_accumulates_into_the_64_bit_destination_pair() {
    for x in [false, true] {
        for y in [false, true] {
            let instruction = half_multiply(0xE140_0080, 4, 5, 1, 0, x, y);
            for &rm_low in &HALFWORDS {
                for &rm_high in &HALFWORDS {
                    for &rs_low in &HALFWORDS {
                        for &rs_high in &HALFWORDS {
                            for accumulator in
                                [0u64, u64::MAX, 0x7FFF_FFFF_FFFF_FFFF, 0x8000_0000_0000_0000]
                            {
                                let rm = packed(rm_low, rm_high);
                                let rs = packed(rs_low, rs_high);
                                let product = i64::from(if x { rm_high } else { rm_low })
                                    * i64::from(if y { rs_high } else { rs_low });
                                let expected = accumulator.wrapping_add(product as u64);
                                let cpu = machine(
                                    instruction,
                                    &[
                                        (0, rm),
                                        (1, rs),
                                        (4, (accumulator >> 32) as u32),
                                        (5, accumulator as u32),
                                    ],
                                    true,
                                );
                                assert_eq!(cpu.regs[5], expected as u32);
                                assert_eq!(cpu.regs[4], (expected >> 32) as u32);
                                assert!(cpu.cpsr.q());
                                assert_nzcv_preserved(&cpu);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn smlawy_and_smulwy_match_the_signed_word_by_halfword_reference() {
    for top in [false, true] {
        for &word in &WORDS {
            for &halfword in &HALFWORDS {
                let rm = word;
                let rs = packed(halfword, -halfword);
                let selected = if top {
                    i64::from((rs >> 16) as u16 as i16)
                } else {
                    i64::from(rs as u16 as i16)
                };
                let product_high = (i64::from(word as i32) * selected) >> 16;

                let smulwy = half_multiply(0xE120_0080, 4, 0, 1, 0, true, top);
                let smulwy = smulwy | (1 << 5);
                let smul = machine(smulwy, &[(0, rm), (1, rs)], true);
                assert_eq!(smul.regs[4], product_high as u32);
                assert!(smul.cpsr.q());
                assert_nzcv_preserved(&smul);

                for accumulator in [i32::MIN, -1, 0, i32::MAX] {
                    for q_before in [false, true] {
                        let instruction = half_multiply(0xE120_0080, 4, 2, 1, 0, false, top);
                        let wide = product_high + i64::from(accumulator);
                        let overflow = wide < i64::from(i32::MIN) || wide > i64::from(i32::MAX);
                        let cpu = machine(
                            instruction,
                            &[(0, rm), (1, rs), (2, accumulator as u32)],
                            q_before,
                        );
                        assert_eq!(cpu.regs[4], wide as u32);
                        assert_eq!(cpu.cpsr.q(), q_before || overflow);
                        assert_nzcv_preserved(&cpu);
                    }
                }
            }
        }
    }
}

#[test]
fn smlad_smladx_smusd_and_smusdx_match_dual_halfword_reference() {
    for subtract in [false, true] {
        for exchange in [false, true] {
            let instruction = dual_multiply(0xE700_0010, 4, 2, 1, 0, subtract, exchange);
            let unaccumulated = dual_multiply(0xE700_0010, 4, 15, 1, 0, subtract, exchange);
            for &rm in &PACKED {
                for &rs in &PACKED {
                    let rhs_low = if exchange {
                        signed_half(rs, true)
                    } else {
                        signed_half(rs, false)
                    };
                    let rhs_high = if exchange {
                        signed_half(rs, false)
                    } else {
                        signed_half(rs, true)
                    };
                    let products = signed_half(rm, false) * rhs_low
                        + if subtract {
                            -(signed_half(rm, true) * rhs_high)
                        } else {
                            signed_half(rm, true) * rhs_high
                        };
                    for accumulator in [i32::MIN, -1, 0, i32::MAX] {
                        for q_before in [false, true] {
                            let wide = products + i64::from(accumulator);
                            let overflow = wide < i64::from(i32::MIN) || wide > i64::from(i32::MAX);
                            let cpu = machine(
                                instruction,
                                &[(0, rm), (1, rs), (2, accumulator as u32)],
                                q_before,
                            );
                            assert_eq!(cpu.regs[4], wide as u32);
                            assert_eq!(cpu.cpsr.q(), q_before || overflow);
                            assert_nzcv_preserved(&cpu);
                        }
                    }

                    let unaccumulated_wide = products;
                    let unaccumulated_overflow = unaccumulated_wide < i64::from(i32::MIN)
                        || unaccumulated_wide > i64::from(i32::MAX);
                    let cpu = machine(unaccumulated, &[(0, rm), (1, rs)], false);
                    assert_eq!(cpu.regs[4], unaccumulated_wide as u32);
                    assert_eq!(cpu.cpsr.q(), !subtract && unaccumulated_overflow);
                    assert_nzcv_preserved(&cpu);
                }
            }
        }
    }
}

#[test]
fn smlald_and_smlsld_accumulate_dual_products_into_64_bits() {
    for subtract in [false, true] {
        for exchange in [false, true] {
            let instruction = dual_multiply(0xE740_0010, 4, 5, 1, 0, subtract, exchange);
            for &rm in &PACKED {
                for &rs in &PACKED {
                    for accumulator in
                        [0u64, u64::MAX, 0x7FFF_FFFF_FFFF_FFFF, 0x8000_0000_0000_0000]
                    {
                        let rhs_low = if exchange {
                            signed_half(rs, true)
                        } else {
                            signed_half(rs, false)
                        };
                        let rhs_high = if exchange {
                            signed_half(rs, false)
                        } else {
                            signed_half(rs, true)
                        };
                        let first = signed_half(rm, false) * rhs_low;
                        let second = signed_half(rm, true) * rhs_high;
                        let products = if subtract {
                            first - second
                        } else {
                            first + second
                        };
                        let expected = accumulator.wrapping_add(products as u64);
                        let cpu = machine(
                            instruction,
                            &[
                                (0, rm),
                                (1, rs),
                                (4, (accumulator >> 32) as u32),
                                (5, accumulator as u32),
                            ],
                            true,
                        );
                        assert_eq!(cpu.regs[5], expected as u32);
                        assert_eq!(cpu.regs[4], (expected >> 32) as u32);
                        assert!(cpu.cpsr.q());
                        assert_nzcv_preserved(&cpu);
                    }
                }
            }
        }
    }
}

#[test]
fn smmul_smmla_and_smmls_match_wide_signed_products_with_optional_rounding() {
    for round in [false, true] {
        for subtract in [false, true] {
            let base = 0xE750_0010 | (u32::from(round) << 5);
            let instruction = base
                | (4 << 16)
                | (2 << 12)
                | (1 << 8)
                | (u32::from(subtract) << 6)
                | (u32::from(subtract) << 7);
            let no_accumulator = base | (4 << 16) | (15 << 12) | (1 << 8);
            for &rm in &WORDS {
                for &rs in &WORDS {
                    let product = i128::from(rm as i32) * i128::from(rs as i32);
                    let rounding = if round { 1i128 << 31 } else { 0 };
                    let accumulator = i32::MIN;
                    let wide = if subtract {
                        (i128::from(accumulator) << 32) - product + rounding
                    } else {
                        (i128::from(accumulator) << 32) + product + rounding
                    };
                    let cpu = machine(
                        instruction,
                        &[(0, rm), (1, rs), (2, accumulator as u32)],
                        true,
                    );
                    assert_eq!(cpu.regs[4], (wide >> 32) as u32);
                    assert!(cpu.cpsr.q());
                    assert_nzcv_preserved(&cpu);

                    let smmul_wide = product + rounding;
                    let smmul = machine(no_accumulator, &[(0, rm), (1, rs)], false);
                    assert_eq!(smmul.regs[4], (smmul_wide >> 32) as u32);
                    assert!(!smmul.cpsr.q());
                    assert_nzcv_preserved(&smmul);
                }
            }
        }
    }
}

#[test]
fn usad8_and_usada8_match_bytewise_absolute_difference_reference() {
    for &lhs in &WORDS {
        for &rhs in &WORDS {
            let mut distance = 0u32;
            for lane in 0..4 {
                let a = (lhs >> (lane * 8)) & 0xFF;
                let b = (rhs >> (lane * 8)) & 0xFF;
                distance = distance.wrapping_add(a.abs_diff(b));
            }
            let usad8 = dual_multiply(0xE780_0010, 4, 15, 1, 0, false, false);
            let cpu = machine(usad8, &[(0, lhs), (1, rhs)], true);
            assert_eq!(cpu.regs[4], distance);
            assert!(cpu.cpsr.q());
            assert_nzcv_preserved(&cpu);

            for accumulator in [0, 1, 0x7FFF_FFFF, u32::MAX] {
                let usada8 = dual_multiply(0xE780_0010, 4, 2, 1, 0, false, false);
                let cpu = machine(usada8, &[(0, lhs), (1, rhs), (2, accumulator)], false);
                assert_eq!(cpu.regs[4], distance.wrapping_add(accumulator));
                assert!(!cpu.cpsr.q());
                assert_nzcv_preserved(&cpu);
            }
        }
    }
}

#[test]
fn umaal_matches_independent_two_word_accumulator_reference() {
    const VALUES: [u32; 8] = [
        0,
        1,
        2,
        0x0000_FFFF,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_0000,
        u32::MAX,
    ];
    // UMAAL r0,r1,r2,r3 = UInt(r2)*UInt(r3) + UInt(r0) + UInt(r1).
    const INSTRUCTION: u32 = 0xE041_0392;

    for &lhs in &VALUES {
        for &rhs in &VALUES {
            for &acc_low in &VALUES {
                for &acc_high in &VALUES {
                    let expected = u64::from(lhs) * u64::from(rhs)
                        + u64::from(acc_low)
                        + u64::from(acc_high);
                    let cpu = machine(
                        INSTRUCTION,
                        &[(0, acc_low), (1, acc_high), (2, lhs), (3, rhs)],
                        true,
                    );

                    assert_eq!(cpu.regs[0], expected as u32);
                    assert_eq!(cpu.regs[1], (expected >> 32) as u32);
                    assert!(cpu.cpsr.q());
                    assert_nzcv_preserved(&cpu);
                }
            }
        }
    }
}

#[test]
fn umaal_rejects_overlapping_destination_registers_without_side_effects() {
    // UMAAL r1,r1,r2,r3 encodes RdLo == RdHi.
    let instruction = 0xE041_1392;
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, instruction).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = 0xAABB_CCDD;
    cpu.regs[2] = 0xFFFF_FFFF;
    cpu.regs[3] = 0xFFFF_FFFF;
    cpu.cpsr.set_flags(true, false, true, true);
    cpu.cpsr.set_q();
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    assert!(matches!(
        cpu.step(&mut memory),
        Err(aceris_core::Error::UndefinedInstruction { pc: 0, word }) if word == instruction
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert_eq!(cpu.pc(), 0);
}
