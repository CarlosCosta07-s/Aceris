//! Independent integration tests for ARMv6 byte/halfword media instructions.

use aceris_core::cpu::Arm11Core;
use aceris_core::memory::{Bus, FlatMemory};

fn execute(program: &[u32], setup: impl FnOnce(&mut Arm11Core)) -> Arm11Core {
    let mut memory = FlatMemory::new(0, program.len() * 4 + 4);
    for (index, instruction) in program.iter().enumerate() {
        memory.write32(index as u32 * 4, *instruction).unwrap();
    }

    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_flags(true, false, true, false);
    setup(&mut cpu);
    for _ in program {
        cpu.step(&mut memory).unwrap();
    }
    cpu
}

fn assert_nzcv(cpu: &Arm11Core, expected: (bool, bool, bool, bool)) {
    assert_eq!(
        (cpu.cpsr.n(), cpu.cpsr.z(), cpu.cpsr.c(), cpu.cpsr.v()),
        expected
    );
}

fn rotate_right(value: u32, rotation: u32) -> u32 {
    value.rotate_right(rotation * 8)
}

#[test]
fn rev_rev16_and_revsh_match_byte_order_reference_without_flag_changes() {
    let cpu = execute(&[0xE6BF_0F31, 0xE6BF_2FB3, 0xE6FF_4FB5], |cpu| {
        cpu.regs[1] = 0x1234_5678;
        cpu.regs[3] = 0x1234_5678;
        cpu.regs[5] = 0x0000_80FE;
    });

    assert_eq!(cpu.regs[0], 0x7856_3412);
    assert_eq!(cpu.regs[2], 0x3412_7856);
    assert_eq!(cpu.regs[4], 0xFFFF_FE80);
    assert_nzcv(&cpu, (true, false, true, false));
    assert!(!cpu.cpsr.q());
    assert_eq!(cpu.cpsr.ge(), 0);
}

#[test]
fn signed_and_unsigned_extensions_apply_rotation_and_correct_lane_signs() {
    let program = [
        0xE6AF_0471, // SXTB r0,r1,ror #8
        0xE6BF_2871, // SXTH r2,r1,ror #16
        0xE6EF_3C71, // UXTB r3,r1,ror #24
        0xE6FF_4471, // UXTH r4,r1,ror #8
        0xE6AF_5871, // SXTB r5,r1,ror #16
        0xE68F_6C71, // SXTB16 r6,r1,ror #24
        0xE6CF_7C71, // UXTB16 r7,r1,ror #24
    ];
    let cpu = execute(&program, |cpu| {
        cpu.regs[1] = 0x80FE_1234;
    });

    assert_eq!(cpu.regs[0], 0x0000_0012);
    assert_eq!(cpu.regs[2], 0xFFFF_80FE);
    assert_eq!(cpu.regs[3], 0x0000_0080);
    assert_eq!(cpu.regs[4], 0x0000_FE12);
    assert_eq!(cpu.regs[5], 0xFFFF_FFFE);
    let rotated = rotate_right(0x80FE_1234, 3);
    let expected_low = (rotated & 0xFF) as u8 as i8 as i16 as u16;
    let expected_high = ((rotated >> 16) & 0xFF) as u8 as i8 as i16 as u16;
    assert_eq!(
        cpu.regs[6],
        (u32::from(expected_high) << 16) | u32::from(expected_low)
    );
    assert_eq!(
        cpu.regs[7],
        (((rotated >> 16) & 0xFF) << 16) | (rotated & 0xFF)
    );
    assert_nzcv(&cpu, (true, false, true, false));
    assert!(!cpu.cpsr.q());
}

#[test]
fn extend_and_add_forms_wrap_at_32_bits_and_preserve_status_flags() {
    let cpu = execute(
        &[0xE6A1_0872, 0xE6B4_3872, 0xE6E6_5072, 0xE6F6_7072],
        |cpu| {
            cpu.regs[1] = 100;
            cpu.regs[2] = 0x80FE_1234;
            cpu.regs[4] = 0x0001_0000;
            cpu.regs[6] = 0x0001_0000;
        },
    );

    assert_eq!(cpu.regs[0], 98);
    assert_eq!(cpu.regs[3], 0x0000_80FE);
    assert_eq!(cpu.regs[5], 0x0001_0034);
    assert_eq!(cpu.regs[7], 0x0001_1234);
    assert_nzcv(&cpu, (true, false, true, false));
    assert!(!cpu.cpsr.q());
}

#[test]
fn scalar_and_packed_saturation_match_signed_and_unsigned_limits() {
    let cpu = execute(
        &[
            0xE6A7_0011, // SSAT r0,#8,r1
            0xE6E7_2193, // USAT r2,#7,r3,LSL #1
            0xE6A7_0F31, // SSAT16 r0,#8,r1
            0xE6E7_2F33, // USAT16 r2,#7,r3
        ],
        |cpu| {
            cpu.regs[1] = 384;
            cpu.regs[3] = 128;
            cpu.cpsr.set_flags(true, false, true, false);
        },
    );

    // The first two instructions saturate 384 and 256 to signed/unsigned 127.
    // The packed forms then clamp each selected halfword independently.
    assert_eq!(cpu.regs[0], 0x0000_007F);
    assert_eq!(cpu.regs[2], 0x0000_007F);
    assert!(cpu.cpsr.q());
    assert_nzcv(&cpu, (true, false, true, false));
}

#[test]
fn pkhbt_pkhtb_and_sel_match_halfword_and_ge_lane_selection() {
    let cpu = execute(&[0xE681_0412, 0xE684_3455, 0xE684_6FB5], |cpu| {
        cpu.regs[1] = 0x1122_3344;
        cpu.regs[2] = 0xAABB_CCDD;
        cpu.regs[4] = 0x5566_7788;
        cpu.regs[5] = 0x8000_1234;
        cpu.cpsr.set_ge(0b0101);
    });

    assert_eq!(cpu.regs[0], 0xBBCC_3344);
    assert_eq!(cpu.regs[3], 0x5566_0012);
    assert_eq!(cpu.regs[6], 0x8066_1288);
    assert_eq!(cpu.cpsr.ge(), 0b0101);
    assert_nzcv(&cpu, (true, false, true, false));
    assert!(!cpu.cpsr.q());
}

#[test]
fn media_instructions_reject_pc_operands_before_register_or_flag_changes() {
    // REV pc,r1 ; SXTB r0,pc ; PKHBT pc,r1,r2 ; SEL r0,pc,r2.
    for instruction in [0xE6BF_FF31, 0xE6AF_007F, 0xE681_F012, 0xE68F_0FB2] {
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, instruction).unwrap();
        let mut cpu = Arm11Core::new();
        cpu.regs[0] = 0x1234_5678;
        cpu.regs[1] = 0x80FE_1234;
        cpu.regs[2] = 0xAABB_CCDD;
        cpu.cpsr.set_flags(true, false, true, false);
        cpu.cpsr.set_q();
        cpu.cpsr.set_ge(0b1010);
        let registers_before = cpu.regs;
        let status_before = cpu.cpsr;

        assert!(matches!(
            cpu.step(&mut memory),
            Err(aceris_core::Error::UndefinedInstruction { pc: 0, word }) if word == instruction
        ));
        assert_eq!(cpu.regs, registers_before, "{instruction:#010x}");
        assert_eq!(cpu.cpsr, status_before, "{instruction:#010x}");
        assert_eq!(cpu.pc(), 0);
    }
}
