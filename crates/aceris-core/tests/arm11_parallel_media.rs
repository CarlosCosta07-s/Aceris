//! Differential coverage for all ARMv6 parallel add/subtract variants.
//!
//! The reference model operates on signed/unsigned lane values independently
//! from the interpreter implementation and checks the CPSR GE/flag contract.

use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

#[derive(Clone, Copy, Debug)]
enum Arithmetic {
    SignedModulo,
    SignedSaturating,
    SignedHalving,
    UnsignedModulo,
    UnsignedSaturating,
    UnsignedHalving,
}

impl Arithmetic {
    const ALL: [Self; 6] = [
        Self::SignedModulo,
        Self::SignedSaturating,
        Self::SignedHalving,
        Self::UnsignedModulo,
        Self::UnsignedSaturating,
        Self::UnsignedHalving,
    ];

    fn encoding(self) -> u32 {
        match self {
            Self::SignedModulo => 0b001,
            Self::SignedSaturating => 0b010,
            Self::SignedHalving => 0b011,
            Self::UnsignedModulo => 0b101,
            Self::UnsignedSaturating => 0b110,
            Self::UnsignedHalving => 0b111,
        }
    }

    fn updates_ge(self) -> bool {
        matches!(self, Self::SignedModulo | Self::UnsignedModulo)
    }
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Add,
    Subtract,
    AddSubtractExchange,
    SubtractAddExchange,
}

impl Operation {
    fn encoding(self) -> u32 {
        match self {
            Self::Add => 0b00,
            Self::AddSubtractExchange => 0b01,
            Self::SubtractAddExchange => 0b10,
            Self::Subtract => 0b11,
        }
    }

    fn adds(self, lane: usize, lane_count: usize) -> bool {
        match self {
            Self::Add => true,
            Self::Subtract => false,
            Self::AddSubtractExchange => lane == lane_count - 1,
            Self::SubtractAddExchange => lane != lane_count - 1,
        }
    }

    fn exchanges(self) -> bool {
        matches!(
            self,
            Self::AddSubtractExchange | Self::SubtractAddExchange
        )
    }
}

fn operations(width: u32) -> &'static [Operation] {
    const BYTE: [Operation; 2] = [Operation::Add, Operation::Subtract];
    const HALFWORD: [Operation; 4] = [
        Operation::Add,
        Operation::AddSubtractExchange,
        Operation::SubtractAddExchange,
        Operation::Subtract,
    ];
    if width == 8 {
        &BYTE
    } else {
        &HALFWORD
    }
}

fn signed_lane(value: u32, width: u32) -> i64 {
    let sign = 1u32 << (width - 1);
    if value & sign == 0 {
        i64::from(value)
    } else {
        i64::from(value) - (1i64 << width)
    }
}

fn encode(arithmetic: Arithmetic, width: u32, operation: Operation) -> u32 {
    // Parallel media encodings: cond=AL, bits[27:24]=0110,
    // Rn=r1, Rd=r2, bits[11:8]=1111, Rm=r3.
    0xE600_0F10
        | (arithmetic.encoding() << 20)
        | (1 << 16)
        | (2 << 12)
        | (if width == 8 { 1 << 7 } else { 0 })
        | (operation.encoding() << 5)
        | 3
}

fn reference(
    arithmetic: Arithmetic,
    width: u32,
    operation: Operation,
    lhs: u32,
    rhs: u32,
    ge_before: u8,
) -> (u32, u8) {
    let lane_count = 32 / width;
    let lane_mask = (1u32 << width) - 1;
    let signed_min = -(1i64 << (width - 1));
    let signed_max = (1i64 << (width - 1)) - 1;
    let mut result = 0u32;
    let mut ge = 0u8;

    for lane in 0..lane_count {
        let rhs_lane = if operation.exchanges() {
            lane_count - lane - 1
        } else {
            lane
        };
        let a = (lhs >> (lane * width)) & lane_mask;
        let b = (rhs >> (rhs_lane * width)) & lane_mask;
        let add = operation.adds(lane, lane_count);

        let (lane_result, ge_set) = match arithmetic {
            Arithmetic::SignedModulo => {
                let value = signed_lane(a, width)
                    + if add {
                        signed_lane(b, width)
                    } else {
                        -signed_lane(b, width)
                    };
                (value as u32 & lane_mask, Some(value >= 0))
            }
            Arithmetic::UnsignedModulo => {
                if add {
                    let value = u64::from(a) + u64::from(b);
                    (
                        value as u32 & lane_mask,
                        Some(value > u64::from(lane_mask)),
                    )
                } else {
                    (a.wrapping_sub(b) & lane_mask, Some(a >= b))
                }
            }
            Arithmetic::SignedSaturating => {
                let value = signed_lane(a, width)
                    + if add {
                        signed_lane(b, width)
                    } else {
                        -signed_lane(b, width)
                    };
                (value.clamp(signed_min, signed_max) as u32 & lane_mask, None)
            }
            Arithmetic::UnsignedSaturating => {
                let value = i64::from(a)
                    + if add {
                        i64::from(b)
                    } else {
                        -i64::from(b)
                    };
                (
                    value.clamp(0, i64::from(lane_mask)) as u32 & lane_mask,
                    None,
                )
            }
            Arithmetic::SignedHalving => {
                let value = signed_lane(a, width)
                    + if add {
                        signed_lane(b, width)
                    } else {
                        -signed_lane(b, width)
                    };
                ((value >> 1) as u32 & lane_mask, None)
            }
            Arithmetic::UnsignedHalving => {
                let value = i64::from(a)
                    + if add {
                        i64::from(b)
                    } else {
                        -i64::from(b)
                    };
                ((value >> 1) as u32 & lane_mask, None)
            }
        };

        result |= lane_result << (lane * width);
        if let Some(set) = ge_set {
            if set {
                if width == 16 {
                    ge |= 0b11 << (lane * 2);
                } else {
                    ge |= 1 << lane;
                }
            }
        }
    }

    (
        result,
        if arithmetic.updates_ge() {
            ge
        } else {
            ge_before
        },
    )
}

fn execute(instruction: u32, lhs: u32, rhs: u32, ge_before: u8, q_before: bool) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, instruction).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.regs[1] = lhs;
    cpu.regs[2] = 0xDEAD_BEEF;
    cpu.regs[3] = rhs;
    cpu.cpsr.set_ge(ge_before);
    cpu.cpsr.set_q();
    if !q_before {
        cpu.cpsr.0 &= !Psr::Q;
    }
    cpu.cpsr.set_flags(true, false, true, true);
    cpu.step(&mut memory).unwrap();
    cpu
}

#[test]
fn all_36_parallel_arithmetic_encodings_match_the_independent_lane_model() {
    const BYTE_VALUES: [u32; 8] = [
        0x0000_0000,
        0x0101_0101,
        0x7F7F_7F7F,
        0x8080_8080,
        0xFF00_7F80,
        0x807F_FF01,
        0x7F80_01FF,
        0xFFFF_FFFF,
    ];
    const HALFWORD_VALUES: [u32; 8] = [
        0x0000_0000,
        0x0001_0001,
        0x7FFF_7FFF,
        0x8000_8000,
        0xFFFF_FFFF,
        0x7FFF_8000,
        0x8000_7FFF,
        0xFFFF_0001,
    ];

    for arithmetic in Arithmetic::ALL {
        for width in [8, 16] {
            let values = if width == 8 {
                &BYTE_VALUES[..]
            } else {
                &HALFWORD_VALUES[..]
            };
            for operation in operations(width) {
                let instruction = encode(arithmetic, width, *operation);
                for &lhs in values {
                    for &rhs in values {
                        for ge_before in [0b0000, 0b0101, 0b1010, 0b1111] {
                            for q_before in [false, true] {
                                let (expected, expected_ge) = reference(
                                    arithmetic,
                                    width,
                                    *operation,
                                    lhs,
                                    rhs,
                                    ge_before,
                                );
                                let cpu = execute(
                                    instruction,
                                    lhs,
                                    rhs,
                                    ge_before,
                                    q_before,
                                );
                                let context = format!(
                                    "{arithmetic:?}/{width}/{operation:?} lhs={lhs:#010x} rhs={rhs:#010x} GE={ge_before:04b} Q={q_before}"
                                );

                                assert_eq!(cpu.regs[2], expected, "{context}");
                                assert_eq!(cpu.cpsr.ge(), expected_ge, "{context}");
                                assert_eq!(cpu.cpsr.q(), q_before, "{context}");
                                assert!(cpu.cpsr.n() && !cpu.cpsr.z() && cpu.cpsr.c() && cpu.cpsr.v(), "{context}");
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn parallel_instruction_encodings_reject_pc_in_each_register_operand() {
    // SADD8 r2,r1,pc ; SADD16 pc,r1,r3 ; QSUB16 r2,pc,r3.
    for instruction in [0xE611_2F9F, 0xE611_FF13, 0xE62F_2F13] {
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, instruction).unwrap();
        let mut cpu = Arm11Core::new();
        cpu.regs[1] = 0x7F80_01FF;
        cpu.regs[2] = 0x1234_5678;
        cpu.regs[3] = 0x8000_FF7F;
        cpu.cpsr.set_ge(0b1010);
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;

        assert!(matches!(
            cpu.step(&mut memory),
            Err(aceris_core::Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
    }
}
