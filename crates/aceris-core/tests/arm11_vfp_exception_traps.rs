//! Independent integration tests for scalar VFP11 asynchronous exception bounce.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

const VMSR_FPSCR_R0: u32 = 0xEEE1_0A10;
const VDIV_S0_S1_S2: u32 = 0xEE80_0A81;
const VADD_S3_S1_S2: u32 = 0xEE70_1A81;
const VSQRT_S0_S1: u32 = 0xEEB1_0AE0;
const VMRS_R4_FPEXC: u32 = 0xEEF8_4A10;
const VMSR_FPEXC_R5: u32 = 0xEEE8_5A10;
const VMRS_R6_FPINST: u32 = 0xEEF9_6A10;

fn machine(program: &[u32]) -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x100);
    for (index, instruction) in program.iter().enumerate() {
        memory.write32(index as u32 * 4, *instruction).unwrap();
    }

    let mut cpu = Arm11Core::new();
    cpu.enable_vfp();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    (cpu, memory)
}

#[test]
fn enabled_divide_by_zero_records_fpinst_and_bounces_the_next_vfp_operation() {
    let (mut cpu, mut memory) = machine(&[
        VMSR_FPSCR_R0,
        VDIV_S0_S1_S2,
        VADD_S3_S1_S2,
        VMRS_R4_FPEXC,
        VMSR_FPEXC_R5,
        VMRS_R6_FPINST,
    ]);
    cpu.regs[0] = 1 << 9; // FPSCR.DZE
    cpu.vfp_mut().set_s(0, 0xDEAD_BEEF);
    cpu.vfp_mut().set_s(1, 1.0f32.to_bits());
    cpu.vfp_mut().set_s(2, 0.0f32.to_bits());
    cpu.vfp_mut().set_s(3, 0xCAFE_BABE);

    cpu.step(&mut memory).unwrap(); // VMSR FPSCR
    assert_eq!(cpu.pc(), 4);
    cpu.step(&mut memory).unwrap(); // VDIV completes with an asynchronous bounce pending.

    assert_eq!(cpu.pc(), 8);
    assert_eq!(
        cpu.vfp().s(0),
        0xDEAD_BEEF,
        "trapped destination is unchanged"
    );
    assert_eq!(
        cpu.vfp().fpscr() & (1 << 1),
        0,
        "DZC is left for support code"
    );
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_eq!(cpu.vfp().fpinst(), VDIV_S0_S1_S2);

    // The next ordinary VFP operation is bounced through Undefined.
    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 8, word }) if word == VADD_S3_S1_S2
    ));
    assert_eq!(cpu.pc(), 8);
    assert_eq!(cpu.vfp().s(3), 0xCAFE_BABE);

    // Exception support code can inspect FPEXC, clear EX, and retrieve FPINST.
    // Move to the handler's instruction stream after the simulated bounce.
    cpu.set_pc(12);
    cpu.step(&mut memory).unwrap(); // VMRS r4,FPEXC is allowed while EX is set.
    assert_ne!(cpu.regs[4] & (1 << 31), 0);
    cpu.regs[5] = cpu.regs[4] & !(1 << 31);
    cpu.step(&mut memory).unwrap(); // VMSR FPEXC,r5 clears the exceptional state.
    assert_eq!(cpu.vfp().fpexc() & (1 << 31), 0);
    cpu.step(&mut memory).unwrap(); // VMRS r6,FPINST
    assert_eq!(cpu.regs[6], VDIV_S0_S1_S2);

    cpu.set_pc(8);
    cpu.step(&mut memory).unwrap(); // Retry the previously bounced VADD.
    assert_eq!(cpu.vfp().s(3), 1.0f32.to_bits());
    assert_eq!(cpu.pc(), 12);
}

#[test]
fn bounced_fpenable_instruction_is_rewritten_to_al_condition() {
    const VDIV_EQ_S0_S1_S2: u32 = 0x0E80_0A81;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_EQ_S0_S1_S2]);
    cpu.regs[0] = 1 << 9; // FPSCR.DZE
    cpu.cpsr.0 |= Psr::Z;
    cpu.vfp_mut().set_s(1, 1.0f32.to_bits());
    cpu.vfp_mut().set_s(2, 0.0f32.to_bits());

    cpu.step(&mut memory).unwrap(); // VMSR FPSCR
    cpu.step(&mut memory).unwrap(); // Conditional VDIV executes and bounces.

    assert_eq!(cpu.vfp().fpinst(), VDIV_S0_S1_S2);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0);
}

#[test]
fn enabled_invalid_operation_sets_fpexc_ioc_without_committing_the_result() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VSQRT_S0_S1]);
    cpu.regs[0] = 1 << 8; // FPSCR.IOE
    cpu.vfp_mut().set_s(0, 0x1234_5678);
    cpu.vfp_mut().set_s(1, (-1.0f32).to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0x1234_5678);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0);
    assert_ne!(cpu.vfp().fpexc() & 1, 0, "FPEXC.IOC is set");
    assert_eq!(cpu.vfp().fpinst(), VSQRT_S0_S1);
    assert_eq!(
        cpu.vfp().fpscr() & 1,
        0,
        "FPSCR.IOC is left for support code"
    );
}

#[test]
fn divide_by_zero_trap_enable_does_not_bounce_a_finite_division() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_S0_S1_S2]);
    cpu.regs[0] = 1 << 9; // FPSCR.DZE
    cpu.vfp_mut().set_s(1, 3.0f32.to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 1.5f32.to_bits());
    assert_eq!(cpu.vfp().fpexc() & (1 << 31), 0);
    assert_eq!(cpu.vfp().fpscr() & (1 << 9), 1 << 9);
}

#[test]
fn overflow_enable_bounces_and_preserves_the_scalar_destination() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_S0_S1_S2]);
    cpu.regs[0] = 1 << 10; // FPSCR.OFE
    cpu.vfp_mut().set_s(0, 0xDEAD_BEEF);
    cpu.vfp_mut().set_s(1, f32::MAX.to_bits());
    cpu.vfp_mut().set_s(2, 0.5f32.to_bits());

    cpu.step(&mut memory).unwrap(); // Enable overflow exceptions.
    cpu.step(&mut memory).unwrap(); // Scalar division overflows and bounces.

    assert_eq!(cpu.vfp().s(0), 0xDEAD_BEEF);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_ne!(cpu.vfp().fpexc() & (1 << 2), 0, "FPEXC.OFC is set");
    assert_eq!(
        cpu.vfp().fpscr() & (1 << 2),
        0,
        "support code owns FPSCR.OFC"
    );
    assert_eq!(cpu.vfp().fpinst(), VDIV_S0_S1_S2);
}

#[test]
fn underflow_enable_bounces_even_when_the_tiny_result_is_exact() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_S0_S1_S2]);
    cpu.regs[0] = 1 << 11; // FPSCR.UFE
    cpu.vfp_mut().set_s(0, 0xCAFE_BABE);
    cpu.vfp_mut().set_s(1, f32::MIN_POSITIVE.to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap(); // Enable underflow exceptions.
    cpu.step(&mut memory).unwrap(); // Exact subnormal result still underflows.

    assert_eq!(cpu.vfp().s(0), 0xCAFE_BABE);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_ne!(cpu.vfp().fpexc() & (1 << 3), 0, "FPEXC.UFC is set");
    assert_eq!(
        cpu.vfp().fpscr() & (1 << 3),
        0,
        "support code owns FPSCR.UFC"
    );
    assert_eq!(cpu.vfp().fpinst(), VDIV_S0_S1_S2);
}

#[test]
fn flush_to_zero_ignores_underflow_trap_and_returns_positive_zero() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_S0_S1_S2]);
    cpu.regs[0] = (1 << 11) | (1 << 24); // FPSCR.UFE | FPSCR.FZ
    cpu.vfp_mut().set_s(1, (-f32::MIN_POSITIVE).to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0, "FZ flushes to positive zero");
    assert_eq!(
        cpu.vfp().fpexc() & (1 << 31),
        0,
        "UFE is ignored in FZ mode"
    );
    assert_ne!(cpu.vfp().fpscr() & (1 << 3), 0, "FPSCR.UFC is set");
    assert_eq!(cpu.vfp().fpinst(), 0xEE00_0A00);
}

#[test]
fn flush_to_zero_detects_tininess_before_rounding() {
    const VMUL_S0_S1_S2: u32 = 0xEE20_0A81;
    let program = [VMSR_FPSCR_R0, VMUL_S0_S1_S2];

    // min_normal * (1 - 2^-24) lies just below the minimum normal value,
    // exactly halfway between min-normal and the largest subnormal. RN-even
    // rounds it to min-normal when FZ is clear; VFP11 FZ detects tininess
    // before rounding and flushes it to positive zero.
    let (mut flushed, mut flushed_memory) = machine(&program);
    flushed.regs[0] = (1 << 11) | (1 << 24); // FPSCR.UFE | FPSCR.FZ
    flushed.vfp_mut().set_s(1, f32::MIN_POSITIVE.to_bits());
    flushed.vfp_mut().set_s(2, 0x3F7F_FFFF); // largest f32 below 1.0
    flushed.step(&mut flushed_memory).unwrap();
    flushed.step(&mut flushed_memory).unwrap();

    assert_eq!(
        flushed.vfp().s(0),
        0,
        "FZ flushes the pre-rounding tiny result"
    );
    assert_eq!(
        flushed.vfp().fpexc() & (1 << 31),
        0,
        "UFE is ignored with FZ"
    );
    assert_ne!(flushed.vfp().fpscr() & (1 << 3), 0, "FPSCR.UFC is set");

    // Without FZ the same exact result rounds up to min-normal. Tininess is
    // tested after rounding, so UFC remains clear and no bounce occurs.
    let (mut rounded, mut rounded_memory) = machine(&program);
    rounded.regs[0] = 1 << 11; // FPSCR.UFE
    rounded.vfp_mut().set_s(1, f32::MIN_POSITIVE.to_bits());
    rounded.vfp_mut().set_s(2, 0x3F7F_FFFF);
    rounded.step(&mut rounded_memory).unwrap();
    rounded.step(&mut rounded_memory).unwrap();

    assert_eq!(rounded.vfp().s(0), f32::MIN_POSITIVE.to_bits());
    assert_eq!(rounded.vfp().fpexc() & (1 << 31), 0);
    assert_eq!(rounded.vfp().fpscr() & (1 << 3), 0);
}

#[test]
fn double_underflow_below_host_f64_range_bounces_when_fz_is_clear() {
    const VMUL_D0_D1_D2: u32 = 0xEE21_0B02;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_D0_D1_D2]);
    cpu.regs[0] = 1 << 11; // FPSCR.UFE
    cpu.vfp_mut().set_d(0, 0xDEAD_BEEF_CAFE_BABE);
    cpu.vfp_mut().set_d(1, f64::MIN_POSITIVE.to_bits());
    cpu.vfp_mut().set_d(2, 2.0f64.powi(-53).to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().d(0), 0xDEAD_BEEF_CAFE_BABE);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_ne!(cpu.vfp().fpexc() & (1 << 3), 0, "FPEXC.UFC is set");
    assert_eq!(cpu.vfp().fpscr() & (1 << 3), 0);
}

#[test]
fn double_underflow_below_host_f64_range_flushes_to_positive_zero_with_fz() {
    const VMUL_D0_D1_D2: u32 = 0xEE21_0B02;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_D0_D1_D2]);
    cpu.regs[0] = (1 << 11) | (1 << 24); // FPSCR.UFE | FPSCR.FZ
    cpu.vfp_mut().set_d(1, f64::MIN_POSITIVE.to_bits());
    cpu.vfp_mut().set_d(2, 2.0f64.powi(-53).to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().d(0), 0);
    assert_eq!(cpu.vfp().fpexc() & (1 << 31), 0, "UFE is ignored with FZ");
    assert_ne!(cpu.vfp().fpscr() & (1 << 3), 0, "FPSCR.UFC is set");
}

#[test]
fn input_subnormal_with_ide_bounces_and_preserves_registers_when_fz_is_clear() {
    const VMUL_S0_S1_S2: u32 = 0xEE20_0A81;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_S0_S1_S2]);
    cpu.regs[0] = 1 << 15; // FPSCR.IDE
    cpu.vfp_mut().set_s(0, 0xDEAD_BEEF);
    cpu.vfp_mut().set_s(1, (-f32::from_bits(1)).to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0xDEAD_BEEF);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_ne!(cpu.vfp().fpscr() & (1 << 7), 0, "FPSCR.IDC is set");
    assert_eq!(cpu.vfp().fpexc() & ((1 << 7) | (1 << 3) | (1 << 2) | 1), 0);
    assert_eq!(cpu.vfp().fpinst(), VMUL_S0_S1_S2);
}

#[test]
fn input_subnormal_with_ide_bounces_in_flush_to_zero_mode() {
    const VMUL_S0_S1_S2: u32 = 0xEE20_0A81;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_S0_S1_S2]);
    cpu.regs[0] = (1 << 15) | (1 << 24); // FPSCR.IDE | FPSCR.FZ
    cpu.vfp_mut().set_s(0, 0xCAFE_BABE);
    cpu.vfp_mut().set_s(1, (-f32::from_bits(1)).to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0xCAFE_BABE);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0);
    assert_ne!(cpu.vfp().fpscr() & (1 << 7), 0);
    assert_eq!(cpu.vfp().fpinst(), VMUL_S0_S1_S2);
}

#[test]
fn flush_to_zero_replaces_negative_subnormal_input_with_positive_zero() {
    const VMUL_S0_S1_S2: u32 = 0xEE20_0A81;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_S0_S1_S2]);
    cpu.regs[0] = 1 << 24; // FPSCR.FZ; IDE is clear.
    cpu.vfp_mut().set_s(0, 0x1234_5678);
    cpu.vfp_mut().set_s(1, (-f32::from_bits(1)).to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0, "the input is flushed to positive zero");
    assert_eq!(cpu.vfp().fpexc() & (1 << 31), 0, "IDE is disabled");
    assert_ne!(cpu.vfp().fpscr() & (1 << 7), 0, "FPSCR.IDC is set");
}

#[test]
fn input_subnormal_without_fz_or_ide_is_processed_and_sets_idc() {
    const VMUL_S0_S1_S2: u32 = 0xEE20_0A81;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_S0_S1_S2]);
    cpu.regs[0] = 0; // Both FZ and IDE are clear.
    cpu.vfp_mut().set_s(1, (-f32::from_bits(1)).to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), (-f32::from_bits(2)).to_bits());
    assert_eq!(cpu.vfp().fpexc() & (1 << 31), 0);
    assert_ne!(cpu.vfp().fpscr() & (1 << 7), 0, "FPSCR.IDC is sticky");
}

#[test]
fn inexact_enable_bounces_only_when_the_scalar_result_is_inexact() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_S0_S1_S2]);
    cpu.regs[0] = 1 << 12; // FPSCR.IXE
    cpu.vfp_mut().set_s(0, 0xDEAD_BEEF);
    cpu.vfp_mut().set_s(1, 1.0f32.to_bits());
    cpu.vfp_mut().set_s(2, 3.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0xDEAD_BEEF);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_ne!(cpu.vfp().fpscr() & (1 << 4), 0, "FPSCR.IXC is set");
    assert_eq!(cpu.vfp().fpexc() & ((1 << 7) | (1 << 3) | (1 << 2) | 1), 0);
    assert_eq!(cpu.vfp().fpinst(), VDIV_S0_S1_S2);
}

#[test]
fn inexact_enable_does_not_bounce_an_exact_scalar_result() {
    const VMUL_S0_S1_S2: u32 = 0xEE20_0A81;
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VMUL_S0_S1_S2]);
    cpu.regs[0] = 1 << 12; // FPSCR.IXE
    cpu.vfp_mut().set_s(0, 0xDEAD_BEEF);
    cpu.vfp_mut().set_s(1, 2.0f32.to_bits());
    cpu.vfp_mut().set_s(2, 2.0f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 4.0f32.to_bits());
    assert_eq!(cpu.vfp().fpexc() & (1 << 31), 0);
    assert_eq!(cpu.vfp().fpscr() & (1 << 4), 0);
}

#[test]
fn overflow_has_priority_over_inexact_when_both_enables_are_set() {
    let (mut cpu, mut memory) = machine(&[VMSR_FPSCR_R0, VDIV_S0_S1_S2]);
    cpu.regs[0] = (1 << 12) | (1 << 10); // FPSCR.IXE | FPSCR.OFE
    cpu.vfp_mut().set_s(0, 0xDEAD_BEEF);
    cpu.vfp_mut().set_s(1, f32::MAX.to_bits());
    cpu.vfp_mut().set_s(2, 0.5f32.to_bits());

    cpu.step(&mut memory).unwrap();
    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.vfp().s(0), 0xDEAD_BEEF);
    assert_ne!(cpu.vfp().fpexc() & (1 << 31), 0, "FPEXC.EX is set");
    assert_ne!(cpu.vfp().fpexc() & (1 << 2), 0, "FPEXC.OFC is set");
    assert_eq!(cpu.vfp().fpscr() & (1 << 4), 0, "IXE is lower priority");
    assert_eq!(cpu.vfp().fpinst(), VDIV_S0_S1_S2);
}
