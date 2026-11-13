//! Independent integration tests for A32 SSAT/USAT immediate ASR encodings.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

fn execute(word: u32, source_register: usize, source: u32, flags: u32) -> Arm11Core {
    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, word).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[source_register] = source;
    cpu.cpsr.0 = (cpu.cpsr.0 & !(0xF000_0000 | (1 << 27))) | flags;
    cpu.step(&mut memory).unwrap();
    cpu
}

#[test]
fn ssat_asr_one_is_decoded_and_saturates_the_arithmetic_result() {
    let cpu = execute(
        0xE6A7_00D1, // SSAT r0, #8, r1, ASR #1.
        1,
        0x8000_0000,
        0xB000_0000,
    );

    assert_eq!(cpu.regs[0], 0xFFFF_FF80);
    assert!(cpu.cpsr.q());
    assert!(cpu.cpsr.n());
    assert!(!cpu.cpsr.z());
    assert!(cpu.cpsr.c());
    assert!(cpu.cpsr.v());
}

#[test]
fn usat_asr_one_uses_signed_shift_before_unsigned_clamping() {
    let cpu = execute(
        0xE6E7_20D3, // USAT r2, #7, r3, ASR #1.
        3,
        0x4000_0000,
        0x0000_0000,
    );

    assert_eq!(cpu.regs[2], 127);
    assert!(cpu.cpsr.q());
}

#[test]
fn ssat_asr_32_uses_sign_fill_and_does_not_set_q_when_in_range() {
    let cpu = execute(
        0xE6A7_0051, // SSAT r0, #8, r1, ASR #32.
        1,
        0x8000_0000,
        0x0000_0000,
    );

    assert_eq!(cpu.regs[0], u32::MAX);
    assert!(!cpu.cpsr.q());
}

#[test]
fn usat_asr_32_clamps_negative_sign_fill_to_zero() {
    let cpu = execute(
        0xE6E7_2053, // USAT r2, #7, r3, ASR #32.
        3,
        0x8000_0000,
        0x0000_0000,
    );

    assert_eq!(cpu.regs[2], 0);
    assert!(cpu.cpsr.q());
}
fn encode_saturation(
    signed: bool,
    saturation: u32,
    rd: u32,
    rm: u32,
    shift: u32,
    arithmetic: bool,
) -> u32 {
    let opcode = if signed { 0xE6A0_0010 } else { 0xE6E0_0010 };
    opcode
        | (saturation << 16)
        | (rd << 12)
        | (shift << 7)
        | (u32::from(arithmetic) << 6)
        | rm
}

fn reference_shift(source: u32, encoded_shift: u32, arithmetic: bool) -> i64 {
    if !arithmetic {
        return i64::from(source.wrapping_shl(encoded_shift) as i32);
    }

    let amount = if encoded_shift == 0 {
        32
    } else {
        encoded_shift
    };
    if amount == 32 {
        if source & 0x8000_0000 != 0 {
            -1
        } else {
            0
        }
    } else {
        i64::from((source as i32) >> amount)
    }
}

fn reference_saturation(value: i64, signed: bool, saturation: u32) -> (u32, bool) {
    let (minimum, maximum) = if signed {
        let width = saturation + 1;
        if width == 32 {
            (i64::from(i32::MIN), i64::from(i32::MAX))
        } else {
            (-(1i64 << (width - 1)), (1i64 << (width - 1)) - 1)
        }
    } else {
        let maximum = if saturation == 0 {
            0
        } else {
            (1i64 << saturation) - 1
        };
        (0, maximum)
    };

    let result = value.clamp(minimum, maximum);
    (result as u32, result != value)
}

#[test]
fn ssat_usat_shift_matrix_matches_an_independent_reference() {
    const SOURCES: [u32; 8] = [
        0,
        1,
        0x7FFF_FFFF,
        0x8000_0000,
        0xFFFF_FFFF,
        0x4000_0000,
        0xC000_0000,
        0x1234_5678,
    ];

    for signed in [true, false] {
        for arithmetic in [false, true] {
            for shift in 0..=31 {
                for saturation in 0..=31 {
                    for source in SOURCES {
                        let shifted = reference_shift(source, shift, arithmetic);
                        let (expected, saturated) =
                            reference_saturation(shifted, signed, saturation);
                        let instruction =
                            encode_saturation(signed, saturation, 0, 1, shift, arithmetic);

                        for q_before in [false, true] {
                            let mut memory = FlatMemory::new(0, 4);
                            memory.write32(0, instruction).unwrap();

                            let mut cpu = Arm11Core::new();
                            cpu.set_pc(0);
                            cpu.regs[1] = source;
                            cpu.cpsr.set_flags(true, false, true, true);
                            if q_before {
                                cpu.cpsr.set_q();
                            }

                            cpu.step(&mut memory).unwrap();

                            let context = format!(
                                "signed={signed} arithmetic={arithmetic} shift={shift} saturation={saturation} source={source:#010x} q_before={q_before}"
                            );
                            assert_eq!(cpu.regs[0], expected, "{context}");
                            assert_eq!(cpu.cpsr.q(), q_before || saturated, "{context}");
                            assert_eq!(
                                (cpu.cpsr.n(), cpu.cpsr.z(), cpu.cpsr.c(), cpu.cpsr.v()),
                                (true, false, true, true),
                                "{context}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn ssat_usat_reject_pc_operands_without_side_effects() {
    use aceris_core::Error;

    let instructions = [
        encode_saturation(true, 7, 15, 1, 0, false),
        encode_saturation(true, 7, 0, 15, 0, true),
        encode_saturation(false, 0, 15, 1, 0, true),
        encode_saturation(false, 31, 0, 15, 0, false),
    ];

    for instruction in instructions {
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, instruction).unwrap();

        let mut cpu = Arm11Core::new();
        cpu.set_pc(0);
        cpu.regs[0] = 0x1357_9BDF;
        cpu.regs[1] = 0x8000_0001;
        cpu.cpsr.set_flags(true, false, true, true);
        cpu.cpsr.set_q();
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;

        assert!(matches!(
            cpu.step(&mut memory),
            Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(cpu.pc(), 0, "{instruction:#010x}");
    }
}
