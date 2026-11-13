//! Independent reference tests for ARM register-controlled barrel shifts.
//!
//! Expected values are calculated here rather than by reusing the core shifter.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

#[derive(Clone, Copy, Debug)]
enum Shift {
    Lsl,
    Lsr,
    Asr,
    Ror,
}

impl Shift {
    const ALL: [Self; 4] = [Self::Lsl, Self::Lsr, Self::Asr, Self::Ror];

    const fn encoding(self) -> u32 {
        match self {
            Self::Lsl => 0,
            Self::Lsr => 1,
            Self::Asr => 2,
            Self::Ror => 3,
        }
    }
}

fn reference_shift(kind: Shift, value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 {
        return (value, carry_in);
    }

    match kind {
        Shift::Lsl => match amount {
            1..=31 => (
                value.wrapping_shl(amount),
                value & (1 << (32 - amount)) != 0,
            ),
            32 => (0, value & 1 != 0),
            _ => (0, false),
        },
        Shift::Lsr => match amount {
            1..=31 => (value >> amount, value & (1 << (amount - 1)) != 0),
            32 => (0, value & (1 << 31) != 0),
            _ => (0, false),
        },
        Shift::Asr => {
            let sign = value & (1 << 31) != 0;
            if amount >= 32 {
                (if sign { u32::MAX } else { 0 }, sign)
            } else {
                (
                    ((value as i32) >> amount) as u32,
                    value & (1 << (amount - 1)) != 0,
                )
            }
        }
        Shift::Ror => {
            let result = value.rotate_right(amount & 31);
            (result, result & (1 << 31) != 0)
        }
    }
}

/// Encodes MOVS r4, r1, <shift> r2.
fn movs_register_shift(kind: Shift) -> u32 {
    0xE1B0_4010 | (2 << 8) | (kind.encoding() << 5) | 1
}

fn execute_shift(kind: Shift, value: u32, amount: u32, carry: bool, overflow: bool) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, movs_register_shift(kind)).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[1] = value;
    cpu.regs[2] = amount;
    cpu.cpsr.set_flags(false, false, carry, overflow);
    cpu.step(&mut memory).unwrap();
    cpu
}

fn flags(cpu: &Arm11Core) -> (bool, bool, bool, bool) {
    (cpu.cpsr.n(), cpu.cpsr.z(), cpu.cpsr.c(), cpu.cpsr.v())
}

#[test]
fn arm_register_shifts_match_independent_reference_at_boundaries() {
    const VALUES: [u32; 9] = [
        0,
        1,
        2,
        0x7FFF_FFFF,
        0x8000_0000,
        0x8000_0001,
        0xFFFF_FFFE,
        0xFFFF_FFFF,
        0x1234_5678,
    ];
    const AMOUNTS: [u32; 12] = [0, 1, 2, 7, 15, 16, 31, 32, 33, 63, 64, 0xFF];

    for kind in Shift::ALL {
        for value in VALUES {
            for amount in AMOUNTS {
                for carry_in in [false, true] {
                    for overflow_in in [false, true] {
                        let (expected, carry_out) =
                            reference_shift(kind, value, amount, carry_in);
                        let cpu = execute_shift(kind, value, amount, carry_in, overflow_in);
                        let expected_flags = (
                            expected >> 31 != 0,
                            expected == 0,
                            carry_out,
                            overflow_in,
                        );

                        assert_eq!(
                            cpu.regs[4], expected,
                            "{kind:?}: value={value:#010x}, amount={amount}"
                        );
                        assert_eq!(
                            flags(&cpu),
                            expected_flags,
                            "{kind:?}: value={value:#010x}, amount={amount}, carry={carry_in}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn arm_register_shift_by_zero_preserves_carry_and_value() {
    for kind in Shift::ALL {
        for (value, carry, overflow) in [
            (0x8000_0001, false, false),
            (0x7FFF_FFFF, true, true),
            (0, true, false),
            (u32::MAX, false, true),
        ] {
            let cpu = execute_shift(kind, value, 0, carry, overflow);
            assert_eq!(cpu.regs[4], value, "{kind:?}");
            assert_eq!(
                flags(&cpu),
                (value >> 31 != 0, value == 0, carry, overflow),
                "{kind:?}"
            );
        }
    }
}
