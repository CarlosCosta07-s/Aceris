//! Independent integration tests for ARM11 exception entry and return paths.

use aceris_core::cpu::{Arm11Core, Psr};
use aceris_core::memory::{Bus, FlatMemory};

#[test]
fn irq_waits_until_unmasked_and_exception_return_restores_the_interrupted_context() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, 0xE1A0_0000).unwrap(); // MOV r0, r0
    memory.write32(0x18, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4

    let mut cpu = Arm11Core::new();
    cpu.set_irq_line(true);
    cpu.cpsr.0 |= Psr::I;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);

    cpu.cpsr.0 &= !Psr::I;
    let interrupted = cpu.cpsr;
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert_eq!(cpu.pc(), 0x18);
    assert_eq!(cpu.regs[14], 8);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_ne!(cpu.cpsr.0 & (Psr::A | Psr::I), 0);

    cpu.set_irq_line(false);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.cpsr, interrupted);
}

#[test]
fn fiq_has_priority_over_irq_and_banks_the_exception_state() {
    let mut memory = FlatMemory::new(0, 4);
    let mut cpu = Arm11Core::new();
    cpu.set_irq_line(true);
    cpu.set_fiq_line(true);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);
    assert_eq!(cpu.pc(), 0x1C);
    assert_eq!(cpu.regs[14], 4);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(
        cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F),
        Psr::A | Psr::I | Psr::F
    );
}

#[test]
fn exception_entry_obeys_high_vector_and_exception_endianness_controls() {
    let mut memory = FlatMemory::new(0, 4);
    let mut cpu = Arm11Core::new();
    cpu.configure_exception_entry(true, true);
    cpu.set_irq_line(true);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.pc(), 0xFFFF_0018);
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert!(cpu.cpsr.big_endian());
}

#[test]
fn undefined_instruction_enters_undefined_mode_and_saves_return_state() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, 0xE7F0_00F0).unwrap(); // Permanently undefined A32 encoding.
    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::UNDEFINED_MODE);
    assert_eq!(cpu.pc(), 0x04);
    assert_eq!(cpu.regs[14], 4);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
}

#[test]
fn data_abort_latches_the_external_fault_address_and_status() {
    let mut memory = FlatMemory::new(0, 0x100);
    memory.write32(0, 0xE591_0000).unwrap(); // LDR r0, [r1]
    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.regs[1] = 0x1000;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 8);
    assert_eq!(cpu.cp15().dfar(), 0x1000);
    assert_eq!(cpu.cp15().dfsr(), 0x8);
}
#[test]
fn data_abort_handler_return_restores_cpsr_and_retries_the_faulting_instruction() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0, 0xE591_0000).unwrap(); // LDR r0, [r1]
    memory.write32(0x10, 0xE25E_F008).unwrap(); // SUBS pc, lr, #8

    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = 0x1000;
    cpu.cpsr.set_flags(true, false, true, false);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 8);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().dfar(), 0x1000);
    assert_eq!(cpu.regs[0], 0x1234_5678);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert_eq!(cpu.pc(), 0);
    assert_eq!(cpu.saved_program_status(), None);
    assert_eq!(cpu.regs[0], 0x1234_5678);
}

#[test]
fn prefetch_abort_handler_return_restores_cpsr_and_retries_the_fetch_address() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0x0C, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4

    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.set_pc(0x1000);
    cpu.cpsr.set_flags(false, true, false, true);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert_eq!(cpu.pc(), 0x0C);
    assert_eq!(cpu.regs[14], 0x1004);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().ifar(), 0x1000);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert_eq!(cpu.pc(), 0x1000);
    assert_eq!(cpu.saved_program_status(), None);
}

#[test]
fn irq_during_thumb_execution_saves_and_restores_thumb_state() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write16(0x00, 0x2007).unwrap(); // MOVS r0, #7
    memory.write32(0x18, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4

    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_thumb(true);
    let interrupted = cpu.cpsr;
    cpu.set_irq_line(true);

    // The pending IRQ is accepted before the current Thumb instruction runs.
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x18);
    assert_eq!(cpu.regs[14], 4);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);

    cpu.set_irq_line(false);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x00);

    // Returning from IRQ resumes the same halfword instead of skipping it.
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[0], 7);
    assert_eq!(cpu.pc(), 0x02);
}

#[test]
fn fiq_during_thumb_execution_saves_and_restores_thumb_state() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write16(0x02, 0x2009).unwrap(); // MOVS r0, #9
    memory.write32(0x1C, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4

    let mut cpu = Arm11Core::new();
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x02);
    let interrupted = cpu.cpsr;
    cpu.set_fiq_line(true);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x1C);
    assert_eq!(cpu.regs[14], 0x06);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(
        cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F),
        Psr::A | Psr::I | Psr::F
    );

    cpu.set_fiq_line(false);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x02);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[0], 9);
    assert_eq!(cpu.pc(), 0x04);
}

#[test]
fn undefined_during_thumb_saves_halfword_return_and_restores_thumb_state() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write16(0x00, 0xDE00).unwrap(); // Permanently undefined T16 encoding.
    memory.write32(0x04, 0xE25E_F002).unwrap(); // SUBS pc, lr, #2

    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.cpsr.set_thumb(true);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::UNDEFINED_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x04);
    assert_eq!(cpu.regs[14], 0x02);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x00);
}

#[test]
fn data_abort_during_thumb_restores_state_and_returns_to_faulting_halfword() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write16(0x00, 0x6808).unwrap(); // LDR r0, [r1, #0]
    memory.write32(0x10, 0xE25E_F008).unwrap(); // SUBS pc, lr, #8

    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.cpsr.set_thumb(true);
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = 0x1000;
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x08);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().dfar(), 0x1000);
    assert_eq!(cpu.cp15().dfsr(), 0x8);
    assert_eq!(cpu.regs[0], 0x1234_5678);
    assert_eq!(cpu.regs[1], 0x1000);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x00);
    assert_eq!(cpu.regs[0], 0x1234_5678);
}

#[test]
fn prefetch_abort_during_thumb_restores_state_and_returns_to_faulting_address() {
    let mut memory = FlatMemory::new(0, 0x40);
    memory.write32(0x0C, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4

    let mut cpu = Arm11Core::new();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x1000);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x0C);
    assert_eq!(cpu.regs[14], 0x1004);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().ifar(), 0x1000);
    assert_eq!(cpu.cp15().ifsr(), 0x8);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x1000);
}

