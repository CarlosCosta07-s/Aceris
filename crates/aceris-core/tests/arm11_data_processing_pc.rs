//! A32 data-processing instructions that write the program counter.

use aceris_core::Error;
use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

#[test]
fn arm_data_processing_write_pc_without_s_bit_branches_with_arm_alignment() {
    // MOV pc, r0 writes an A32 PC; it must not use BX interworking semantics.
    const MOV_PC_R0: u32 = 0xE1A0_F000;

    for target in [0x0000_1000, 0x0000_1001, 0x0000_1002, 0x0000_1003] {
        let mut memory = FlatMemory::new(0, 4);
        memory.write32(0, MOV_PC_R0).unwrap();

        let mut cpu = Arm11Core::new();
        cpu.set_pc(0);
        cpu.regs[0] = target;
        cpu.cpsr = Psr(Psr::SUPERVISOR_MODE | Psr::N | Psr::C | Psr::V);
        let status_before = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.pc(), target & !3, "target={target:#010x}");
        assert!(!cpu.cpsr.thumb(), "MOV pc must remain in A32");
        assert_eq!(cpu.cpsr, status_before, "target={target:#010x}");
        assert_eq!(cpu.regs[0], target);
    }
}

#[test]
fn movs_pc_restores_spsr_and_aligns_target_for_restored_thumb_state() {
    // MOVS pc, r0 performs an exception return rather than an ordinary branch.
    const MOVS_PC_R0: u32 = 0xE1B0_F000;

    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, MOVS_PC_R0).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.cpsr = Psr(Psr::SUPERVISOR_MODE | Psr::A | Psr::I);
    cpu.set_saved_program_status(Psr(Psr::USER_MODE | Psr::T | Psr::N | Psr::C))
        .unwrap();
    cpu.regs[0] = 0x0000_2003;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.pc(), 0x0000_2002);
    assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
    assert!(cpu.cpsr.thumb());
    assert!(cpu.cpsr.n());
    assert!(!cpu.cpsr.z());
    assert!(cpu.cpsr.c());
    assert!(!cpu.cpsr.v());
    assert_eq!(cpu.saved_program_status(), None);
    assert!(cpu.take_exception_return_wakeup());
    assert!(!cpu.take_exception_return_wakeup());
}

#[test]
fn movs_pc_without_active_spsr_is_rejected_without_side_effects() {
    const MOVS_PC_R0: u32 = 0xE1B0_F000;

    let mut memory = FlatMemory::new(0, 4);
    memory.write32(0, MOVS_PC_R0).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.set_pc(0);
    cpu.regs[0] = 0x0000_3003;
    cpu.cpsr = Psr(Psr::USER_MODE | Psr::N | Psr::C);
    let registers_before = cpu.regs;
    let status_before = cpu.cpsr;

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::UndefinedInstruction { pc: 0, word: MOVS_PC_R0 })
    ));
    assert_eq!(cpu.regs, registers_before);
    assert_eq!(cpu.cpsr, status_before);
    assert!(!cpu.take_exception_return_wakeup());
}
