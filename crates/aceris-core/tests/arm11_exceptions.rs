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

fn mmu_enabled_thumb_machine() -> (Arm11Core, FlatMemory) {
    let mut memory = FlatMemory::new(0, 0x10_0000);
    // L1 table at 0x4000: identity-map VA 0..1 MiB as an RW section.
    memory.write32(0x4000, 0b10 | (0b11 << 10)).unwrap();
    // MCR p15,0,r0,c2,c0,0 (TTBR0), MCR p15,0,r1,c3,c0,0 (DACR),
    // MCR p15,0,r2,c1,c0,0 (SCTLR.M | SCTLR.XP).
    memory.write32(0x00, 0xEE02_0F10).unwrap();
    memory.write32(0x04, 0xEE03_1F10).unwrap();
    memory.write32(0x08, 0xEE01_2F10).unwrap();

    let mut cpu = Arm11Core::new();
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    cpu.set_fault_exception_entry_enabled(true);
    cpu.regs[0] = 0x4000;
    cpu.regs[1] = 0b01; // Domain 0 = client.
    cpu.regs[2] = 1 | (1 << 23); // Enable MMU and ARMv6 XP descriptors.

    for _ in 0..3 {
        cpu.step(&mut memory).unwrap();
    }
    assert!(cpu.cp15().mmu_enabled());
    (cpu, memory)
}

#[test]
fn thumb_prefetch_translation_abort_preserves_thumb_state_and_return_address() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write32(0x0C, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4
    let faulting_address = 0x0010_0000; // L1 descriptor is absent.
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(faulting_address);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x0C);
    assert_eq!(cpu.regs[14], faulting_address + 4);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().ifar(), faulting_address);
    assert_eq!(cpu.cp15().ifsr(), 0x5);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), faulting_address);
}

#[test]
fn thumb_data_translation_abort_preserves_thumb_state_and_faulting_instruction() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write16(0x100, 0x6808).unwrap(); // LDR r0, [r1, #0]
    memory.write32(0x10, 0xE25E_F008).unwrap(); // SUBS pc, lr, #8
    let faulting_data_address = 0x0010_0000; // L1 descriptor is absent.
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = faulting_data_address;
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().dfar(), faulting_data_address);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.regs[0], 0x1234_5678);
    assert_eq!(cpu.regs[1], faulting_data_address);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x100);
    assert_eq!(cpu.regs[0], 0x1234_5678);
}

fn enable_mmu_high_vectors(
    cpu: &mut Arm11Core,
    memory: &mut FlatMemory,
    exception_big_endian: bool,
) {
    // With TTBCR.N=1, TTBR0 is the 8 KiB low-address table and TTBR1
    // is the 16 KiB high-address table. Both descriptors identity-map the
    // low region or map the high vector section onto physical 0..1 MiB.
    memory.write32(0xC000, 0b10 | (0b11 << 10)).unwrap();
    memory.write32(0x7FFC, 0b10 | (0b11 << 10)).unwrap();

    // Configure TTBR0=0xC000, TTBR1=0x4000 and TTBCR.N=1 through guest MCRs.
    memory.write32(0x0C, 0xEE02_0F10).unwrap(); // MCR p15,0,r0,c2,c0,0
    memory.write32(0x10, 0xEE02_3F30).unwrap(); // MCR p15,0,r3,c2,c0,1
    memory.write32(0x14, 0xEE02_4F50).unwrap(); // MCR p15,0,r4,c2,c0,2
    memory.write32(0x18, 0xEE01_2F10).unwrap(); // MCR p15,0,r2,c1,c0,0

    cpu.regs[0] = 0xC000;
    cpu.regs[3] = 0x4000;
    cpu.regs[4] = 1;
    let exception_endian_bit = if exception_big_endian { 1 << 15 } else { 0 };
    cpu.regs[2] = 1 | (1 << 23) | (1 << 13) | exception_endian_bit;

    for _ in 0..4 {
        cpu.step(memory).unwrap();
    }
    assert!(cpu.cp15().mmu_enabled());
    assert!(cpu.cp15().high_vectors());
    assert_eq!(cpu.cp15().exception_big_endian(), exception_big_endian);
    assert_eq!(cpu.cp15().ttbr0(), 0xC000);
    assert_eq!(cpu.cp15().ttbr1(), 0x4000);
    assert_eq!(cpu.cp15().ttbcr_n(), 1);
}

#[test]
fn irq_during_thumb_uses_high_vector_through_enabled_mmu() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    enable_mmu_high_vectors(&mut cpu, &mut memory, false);
    memory.write16(0x100, 0x2007).unwrap(); // MOVS r0, #7
    memory.write32(0x000F_0018, 0xE25E_F004).unwrap(); // PA for high IRQ vector

    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    let interrupted = cpu.cpsr;
    cpu.set_irq_line(true);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0xFFFF_0018);
    assert!(!cpu.cpsr.big_endian());
    assert_eq!(cpu.regs[14], 0x104);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);

    cpu.set_irq_line(false);
    cpu.step(&mut memory).unwrap(); // Fetch high vector through MmuBus and return.
    // The high-vector handler returned through MMU translation and restored
    // the original Supervisor/Thumb context.
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x100);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[0], 7);
    assert_eq!(cpu.pc(), 0x102);
}

#[test]
fn fiq_during_thumb_uses_high_vector_through_enabled_mmu() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    enable_mmu_high_vectors(&mut cpu, &mut memory, false);
    memory.write16(0x100, 0x2009).unwrap(); // MOVS r0, #9
    memory.write32(0x000F_001C, 0xE25E_F004).unwrap(); // PA for high FIQ vector

    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    let interrupted = cpu.cpsr;
    cpu.set_fiq_line(true);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0xFFFF_001C);
    assert!(!cpu.cpsr.big_endian());
    assert_eq!(cpu.regs[14], 0x104);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(
        cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F),
        Psr::A | Psr::I | Psr::F
    );

    cpu.set_fiq_line(false);
    cpu.step(&mut memory).unwrap(); // Fetch high vector through MmuBus and return.
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x100);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[0], 9);
    assert_eq!(cpu.pc(), 0x102);
}

#[test]
fn thumb_data_translation_abort_uses_high_vector_through_enabled_mmu() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    enable_mmu_high_vectors(&mut cpu, &mut memory, true);
    memory.write16(0x100, 0x6808).unwrap(); // LDR r0, [r1, #0]
    // VA 0xFFFF0010 maps to PA 0x000F0010 through the high-vector section.
    memory.write32(0x000F_0010, 0xE25E_F008).unwrap(); // SUBS pc, lr, #8

    let faulting_data_address = 0x0010_0000; // TTBR0 descriptor is absent.
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = faulting_data_address;
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert!(cpu.cpsr.big_endian()); // SCTLR.EE selects big-endian exception state.
    assert_eq!(cpu.pc(), 0xFFFF_0010);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().dfar(), faulting_data_address);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[0], 0x1234_5678);
    assert_eq!(cpu.regs[1], faulting_data_address);

    cpu.step(&mut memory).unwrap(); // Fetch handler through MMU and return.
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert!(!cpu.cpsr.big_endian());
    assert_eq!(cpu.pc(), 0x100);
    assert_eq!(cpu.saved_program_status(), None);
    assert_eq!(cpu.regs[0], 0x1234_5678);
    assert_eq!(cpu.regs[1], faulting_data_address);
}

#[test]
fn thumb_prefetch_translation_abort_uses_high_vector_through_enabled_mmu() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    enable_mmu_high_vectors(&mut cpu, &mut memory, true);
    // VA 0xFFFF000C maps to PA 0x000F000C through the high-vector section.
    memory.write32(0x000F_000C, 0xE25E_F004).unwrap(); // SUBS pc, lr, #4

    let faulting_address = 0x0010_0000; // TTBR0 descriptor is absent.
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(faulting_address);
    let interrupted = cpu.cpsr;

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert!(cpu.cpsr.big_endian());
    assert_eq!(cpu.pc(), 0xFFFF_000C);
    assert_eq!(cpu.regs[14], faulting_address + 4);
    assert_eq!(cpu.saved_program_status(), Some(interrupted));
    assert_eq!(cpu.cp15().ifar(), faulting_address);
    assert_eq!(cpu.cp15().ifsr(), 0x5);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);

    cpu.step(&mut memory).unwrap(); // Fetch handler through MMU and return.
    assert_eq!(cpu.cpsr, interrupted);
    assert!(cpu.cpsr.thumb());
    assert!(!cpu.cpsr.big_endian());
    assert_eq!(cpu.pc(), faulting_address);
    assert_eq!(cpu.saved_program_status(), None);
}

#[test]
fn a32_ldmia_translation_abort_keeps_registers_and_writeback_atomic() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write32(0x100, 0xE8B0_0006).unwrap(); // LDMIA r0!, {r1,r2}

    let base = 0x000F_FFFC; // First word mapped; second word is in unmapped VA section.
    let first_value = 0x1122_3344;
    memory.write32(base, first_value).unwrap();
    cpu.set_pc(0x100);
    cpu.regs[0] = base;
    cpu.regs[1] = 0xAAAA_AAAA;
    cpu.regs[2] = 0xBBBB_BBBB;
    let interrupted_regs = cpu.regs;
    let interrupted_cpsr = cpu.cpsr;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
    assert_eq!(memory.read32(base).unwrap(), first_value);
}

#[test]
fn a32_stmia_translation_abort_preserves_completed_store_without_writeback() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write32(0x100, 0xE8A0_0006).unwrap(); // STMIA r0!, {r1,r2}

    let base = 0x000F_FFFC; // First word mapped; second word is in unmapped VA section.
    let first_sentinel = 0xDEAD_BEEF;
    memory.write32(base, first_sentinel).unwrap();
    cpu.set_pc(0x100);
    cpu.regs[0] = base;
    cpu.regs[1] = 0x1122_3344;
    cpu.regs[2] = 0x5566_7788;
    let interrupted_regs = cpu.regs;
    let interrupted_cpsr = cpu.cpsr;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x805);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
    // A completed earlier STM beat is not rolled back by exception entry.
    assert_eq!(memory.read32(base).unwrap(), interrupted_regs[1]);
}

#[test]
fn thumb_ldmia_translation_abort_keeps_registers_and_writeback_atomic() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write16(0x100, 0xC806).unwrap(); // LDMIA r0!, {r1,r2}

    let base = 0x000F_FFFC; // First word mapped; second word is in unmapped VA section.
    let first_value = 0x1234_5678;
    memory.write32(base, first_value).unwrap();
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    cpu.regs[0] = base;
    cpu.regs[1] = 0xAAAA_AAAA;
    cpu.regs[2] = 0xBBBB_BBBB;
    let interrupted_regs = cpu.regs;
    let interrupted_cpsr = cpu.cpsr;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
    assert_eq!(memory.read32(base).unwrap(), first_value);
}

#[test]
fn thumb_stmia_translation_abort_preserves_completed_store_without_writeback() {
    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write16(0x100, 0xC006).unwrap(); // STMIA r0!, {r1,r2}

    let base = 0x000F_FFFC; // First word mapped; second word is in unmapped VA section.
    let first_sentinel = 0xCAFEBABE;
    memory.write32(base, first_sentinel).unwrap();
    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    cpu.regs[0] = base;
    cpu.regs[1] = 0x1122_3344;
    cpu.regs[2] = 0x5566_7788;
    let interrupted_regs = cpu.regs;
    let interrupted_cpsr = cpu.cpsr;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x805);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
    // A completed earlier STM beat is not rolled back by exception entry.
    assert_eq!(memory.read32(base).unwrap(), interrupted_regs[1]);
}

#[test]
fn a32_ldmia_translation_abort_on_third_or_fourth_beat_is_atomic() {
    for (base, register_list, transfer_count) in
        [(0x000F_FFF8, 0x000E, 3usize), (0x000F_FFF4, 0x001E, 4usize)]
    {
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write32(0x100, 0xE8B0_0000 | register_list).unwrap(); // LDMIA r0!, {r1-rN}

        for index in 0..transfer_count - 1 {
            memory
                .write32(base + index as u32 * 4, 0x1100_0000 + index as u32)
                .unwrap();
        }

        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(5).skip(1) {
            *value = 0xA000_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert_eq!(cpu.pc(), 0x10);
        assert_eq!(cpu.regs[14], 0x108);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
        assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
        assert_eq!(cpu.cp15().dfsr(), 0x5);
        assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
        assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
        for index in 0..transfer_count - 1 {
            assert_eq!(
                memory.read32(base + index as u32 * 4).unwrap(),
                0x1100_0000 + index as u32
            );
        }
    }
}

#[test]
fn a32_stmia_translation_abort_on_third_or_fourth_beat_keeps_completed_stores() {
    for (base, register_list, transfer_count) in
        [(0x000F_FFF8, 0x000E, 3usize), (0x000F_FFF4, 0x001E, 4usize)]
    {
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write32(0x100, 0xE8A0_0000 | register_list).unwrap(); // STMIA r0!, {r1-rN}

        for index in 0..transfer_count - 1 {
            memory
                .write32(base + index as u32 * 4, 0xDEAD_BEEF)
                .unwrap();
        }

        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(5).skip(1) {
            *value = 0x2200_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert_eq!(cpu.pc(), 0x10);
        assert_eq!(cpu.regs[14], 0x108);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
        assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
        assert_eq!(cpu.cp15().dfsr(), 0x805);
        assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
        assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
        for index in 0..transfer_count - 1 {
            assert_eq!(
                memory.read32(base + index as u32 * 4).unwrap(),
                interrupted_regs[index + 1]
            );
        }
    }
}

#[test]
fn thumb_ldmia_translation_abort_on_third_or_fourth_beat_is_atomic() {
    for (base, register_list, transfer_count) in
        [(0x000F_FFF8, 0x0E, 3usize), (0x000F_FFF4, 0x1E, 4usize)]
    {
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write16(0x100, 0xC800 | register_list).unwrap(); // LDMIA r0!, {r1-rN}

        for index in 0..transfer_count - 1 {
            memory
                .write32(base + index as u32 * 4, 0x3300_0000 + index as u32)
                .unwrap();
        }

        cpu.cpsr.set_thumb(true);
        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(5).skip(1) {
            *value = 0xA000_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert!(!cpu.cpsr.thumb());
        assert_eq!(cpu.pc(), 0x10);
        assert_eq!(cpu.regs[14], 0x108);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
        assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
        assert_eq!(cpu.cp15().dfsr(), 0x5);
        assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
        assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
        for index in 0..transfer_count - 1 {
            assert_eq!(
                memory.read32(base + index as u32 * 4).unwrap(),
                0x3300_0000 + index as u32
            );
        }
    }
}

#[test]
fn thumb_stmia_translation_abort_on_third_or_fourth_beat_keeps_completed_stores() {
    for (base, register_list, transfer_count) in
        [(0x000F_FFF8, 0x0E, 3usize), (0x000F_FFF4, 0x1E, 4usize)]
    {
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write16(0x100, 0xC000 | register_list).unwrap(); // STMIA r0!, {r1-rN}

        for index in 0..transfer_count - 1 {
            memory.write32(base + index as u32 * 4, 0xCAFEBABE).unwrap();
        }

        cpu.cpsr.set_thumb(true);
        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(5).skip(1) {
            *value = 0x4400_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert!(!cpu.cpsr.thumb());
        assert_eq!(cpu.pc(), 0x10);
        assert_eq!(cpu.regs[14], 0x108);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
        assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
        assert_eq!(cpu.cp15().dfsr(), 0x805);
        assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
        assert_eq!(cpu.regs[..13], interrupted_regs[..13]);
        for index in 0..transfer_count - 1 {
            assert_eq!(
                memory.read32(base + index as u32 * 4).unwrap(),
                interrupted_regs[index + 1]
            );
        }
    }
}

#[test]
fn thumb_ldmia_full_low_register_list_translation_abort_is_atomic() {
    const REGISTER_LIST: u32 = 0xFF; // r0-r7; r0 is the lowest register and base.
    const TRANSFER_COUNT: u32 = 8;
    const FAULT_ADDRESS: u32 = 0x0010_0000;
    let base = FAULT_ADDRESS - (TRANSFER_COUNT - 1) * 4;

    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write16(0x100, 0xC800 | REGISTER_LIST).unwrap(); // LDMIA r0!, {r0-r7}

    for index in 0..TRANSFER_COUNT - 1 {
        memory
            .write32(base + index * 4, 0x3300_0000 + index)
            .unwrap();
    }

    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    cpu.regs[0] = base;
    for (register, value) in cpu.regs.iter_mut().enumerate().take(8).skip(1) {
        *value = 0xA000_0000 + register as u32;
    }
    let interrupted_regs = cpu.regs;
    let interrupted_cpsr = cpu.cpsr;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), FAULT_ADDRESS);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[..8], interrupted_regs[..8]);
    for index in 0..TRANSFER_COUNT - 1 {
        assert_eq!(
            memory.read32(base + index * 4).unwrap(),
            0x3300_0000 + index
        );
    }

    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    assert_eq!(cpu.regs[13], interrupted_regs[13]);
    assert_eq!(cpu.regs[14], interrupted_regs[14]);
}

#[test]
fn thumb_stmia_full_low_register_list_translation_abort_keeps_completed_stores() {
    const REGISTER_LIST: u32 = 0xFF; // r0-r7; r0 is the lowest register and base.
    const TRANSFER_COUNT: u32 = 8;
    const FAULT_ADDRESS: u32 = 0x0010_0000;
    let base = FAULT_ADDRESS - (TRANSFER_COUNT - 1) * 4;

    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write16(0x100, 0xC000 | REGISTER_LIST).unwrap(); // STMIA r0!, {r0-r7}

    for index in 0..TRANSFER_COUNT - 1 {
        memory.write32(base + index * 4, 0xCAFEBABE).unwrap();
    }

    cpu.cpsr.set_thumb(true);
    cpu.set_pc(0x100);
    cpu.regs[0] = base;
    for (register, value) in cpu.regs.iter_mut().enumerate().take(8).skip(1) {
        *value = 0x4400_0000 + register as u32;
    }
    let interrupted_regs = cpu.regs;
    let interrupted_cpsr = cpu.cpsr;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert!(!cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), FAULT_ADDRESS);
    assert_eq!(cpu.cp15().dfsr(), 0x805);
    assert_eq!(cpu.cpsr.0 & (Psr::A | Psr::I), Psr::A | Psr::I);
    assert_eq!(cpu.regs[..8], interrupted_regs[..8]);
    for index in 0..TRANSFER_COUNT - 1 {
        assert_eq!(
            memory.read32(base + index * 4).unwrap(),
            interrupted_regs[index as usize]
        );
    }

    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    assert_eq!(cpu.regs[13], interrupted_regs[13]);
    assert_eq!(cpu.regs[14], interrupted_regs[14]);
}

#[test]
fn a32_ldm_translation_abort_in_all_addressing_modes_keeps_registers_and_writeback_atomic() {
    for (mode, pre_index, up) in [("IA", 0, 1), ("IB", 1, 1), ("DA", 0, 0), ("DB", 1, 0)] {
        for (register_list, transfer_count) in [(0x000E, 3usize), (0x001E, 4usize)] {
            let start = 0x0010_0000 - (transfer_count as u32 - 1) * 4;
            let size = transfer_count as u32 * 4;
            let base = match (pre_index, up) {
                (0, 1) => start,
                (1, 1) => start - 4,
                (0, 0) => start + size - 4,
                (1, 0) => start + size,
                _ => unreachable!(),
            };
            let instruction = 0xE800_0000
                | (pre_index << 24)
                | (up << 23)
                | (1 << 21)
                | (1 << 20)
                | register_list;
            let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
            memory.write32(0x100, instruction).unwrap();

            for index in 0..transfer_count - 1 {
                memory
                    .write32(start + index as u32 * 4, 0x1100_0000 + index as u32)
                    .unwrap();
            }

            cpu.set_pc(0x100);
            cpu.regs[0] = base;
            for (register, value) in cpu.regs.iter_mut().enumerate().take(5).skip(1) {
                *value = 0xA000_0000 + register as u32;
            }
            let interrupted_regs = cpu.regs;
            let interrupted_cpsr = cpu.cpsr;

            cpu.step(&mut memory).unwrap();

            assert_eq!(
                cpu.cpsr.mode(),
                Psr::ABORT_MODE,
                "{mode}, count={transfer_count}"
            );
            assert_eq!(cpu.pc(), 0x10, "{mode}, count={transfer_count}");
            assert_eq!(cpu.regs[14], 0x108, "{mode}, count={transfer_count}");
            assert_eq!(
                cpu.saved_program_status(),
                Some(interrupted_cpsr),
                "{mode}, count={transfer_count}"
            );
            assert_eq!(
                cpu.cp15().dfar(),
                0x0010_0000,
                "{mode}, count={transfer_count}"
            );
            assert_eq!(cpu.cp15().dfsr(), 0x5, "{mode}, count={transfer_count}");
            assert_eq!(
                cpu.regs[..13],
                interrupted_regs[..13],
                "{mode}, count={transfer_count}"
            );
        }
    }
}

#[test]
fn a32_stm_translation_abort_in_all_addressing_modes_keeps_completed_stores_without_writeback() {
    for (mode, pre_index, up) in [("IA", 0, 1), ("IB", 1, 1), ("DA", 0, 0), ("DB", 1, 0)] {
        for (register_list, transfer_count) in [(0x000E, 3usize), (0x001E, 4usize)] {
            let start = 0x0010_0000 - (transfer_count as u32 - 1) * 4;
            let size = transfer_count as u32 * 4;
            let base = match (pre_index, up) {
                (0, 1) => start,
                (1, 1) => start - 4,
                (0, 0) => start + size - 4,
                (1, 0) => start + size,
                _ => unreachable!(),
            };
            let instruction =
                0xE800_0000 | (pre_index << 24) | (up << 23) | (1 << 21) | register_list;
            let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
            memory.write32(0x100, instruction).unwrap();

            for index in 0..transfer_count - 1 {
                memory
                    .write32(start + index as u32 * 4, 0xDEAD_BEEF)
                    .unwrap();
            }

            cpu.set_pc(0x100);
            cpu.regs[0] = base;
            for (register, value) in cpu.regs.iter_mut().enumerate().take(5).skip(1) {
                *value = 0x2200_0000 + register as u32;
            }
            let interrupted_regs = cpu.regs;
            let interrupted_cpsr = cpu.cpsr;

            cpu.step(&mut memory).unwrap();

            assert_eq!(
                cpu.cpsr.mode(),
                Psr::ABORT_MODE,
                "{mode}, count={transfer_count}"
            );
            assert_eq!(cpu.pc(), 0x10, "{mode}, count={transfer_count}");
            assert_eq!(cpu.regs[14], 0x108, "{mode}, count={transfer_count}");
            assert_eq!(
                cpu.saved_program_status(),
                Some(interrupted_cpsr),
                "{mode}, count={transfer_count}"
            );
            assert_eq!(
                cpu.cp15().dfar(),
                0x0010_0000,
                "{mode}, count={transfer_count}"
            );
            assert_eq!(cpu.cp15().dfsr(), 0x805, "{mode}, count={transfer_count}");
            assert_eq!(
                cpu.regs[..13],
                interrupted_regs[..13],
                "{mode}, count={transfer_count}"
            );

            for index in 0..transfer_count - 1 {
                assert_eq!(
                    memory.read32(start + index as u32 * 4).unwrap(),
                    interrupted_regs[index + 1],
                    "{mode}, beat={index}, count={transfer_count}"
                );
            }
        }
    }
}

#[test]
fn a32_ldm_translation_abort_with_thirteen_registers_preserves_all_destinations_in_all_modes() {
    const REGISTER_LIST: u32 = 0x3FFE; // r1-r13; r0 is the writeback base.
    const TRANSFER_COUNT: u32 = 13;
    const SIZE: u32 = TRANSFER_COUNT * 4;
    let start = 0x0010_0000 - (TRANSFER_COUNT - 1) * 4;

    for (mode, pre_index, up) in [("IA", 0, 1), ("IB", 1, 1), ("DA", 0, 0), ("DB", 1, 0)] {
        let base = match (pre_index, up) {
            (0, 1) => start,
            (1, 1) => start - 4,
            (0, 0) => start + SIZE - 4,
            (1, 0) => start + SIZE,
            _ => unreachable!(),
        };
        let instruction =
            0xE800_0000 | (pre_index << 24) | (up << 23) | (1 << 21) | (1 << 20) | REGISTER_LIST;
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write32(0x100, instruction).unwrap();

        for index in 0..TRANSFER_COUNT - 1 {
            memory
                .write32(start + index * 4, 0x7100_0000 + index)
                .unwrap();
        }

        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(14).skip(1) {
            *value = 0xA100_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE, "{mode}");
        assert_eq!(cpu.pc(), 0x10, "{mode}");
        assert_eq!(cpu.regs[14], 0x108, "{mode}");
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr), "{mode}");
        assert_eq!(cpu.cp15().dfar(), 0x0010_0000, "{mode}");
        assert_eq!(cpu.cp15().dfsr(), 0x5, "{mode}");
        assert_eq!(cpu.regs[..13], interrupted_regs[..13], "{mode}");
        for index in 0..TRANSFER_COUNT - 1 {
            assert_eq!(
                memory.read32(start + index * 4).unwrap(),
                0x7100_0000 + index,
                "{mode}, beat={index}"
            );
        }

        // SP is banked on entry to Abort; inspect the interrupted SVC bank too.
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!(cpu.regs[13], interrupted_regs[13], "{mode}, SVC SP");
    }
}

#[test]
fn a32_stm_translation_abort_with_thirteen_registers_keeps_completed_stores_in_all_modes() {
    const REGISTER_LIST: u32 = 0x3FFE; // r1-r13; r0 is the writeback base.
    const TRANSFER_COUNT: u32 = 13;
    const SIZE: u32 = TRANSFER_COUNT * 4;
    let start = 0x0010_0000 - (TRANSFER_COUNT - 1) * 4;

    for (mode, pre_index, up) in [("IA", 0, 1), ("IB", 1, 1), ("DA", 0, 0), ("DB", 1, 0)] {
        let base = match (pre_index, up) {
            (0, 1) => start,
            (1, 1) => start - 4,
            (0, 0) => start + SIZE - 4,
            (1, 0) => start + SIZE,
            _ => unreachable!(),
        };
        let instruction = 0xE800_0000 | (pre_index << 24) | (up << 23) | (1 << 21) | REGISTER_LIST;
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write32(0x100, instruction).unwrap();

        for index in 0..TRANSFER_COUNT - 1 {
            memory.write32(start + index * 4, 0xDEAD_BEEF).unwrap();
        }

        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(14).skip(1) {
            *value = 0xC100_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE, "{mode}");
        assert_eq!(cpu.pc(), 0x10, "{mode}");
        assert_eq!(cpu.regs[14], 0x108, "{mode}");
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr), "{mode}");
        assert_eq!(cpu.cp15().dfar(), 0x0010_0000, "{mode}");
        assert_eq!(cpu.cp15().dfsr(), 0x805, "{mode}");
        assert_eq!(cpu.regs[..13], interrupted_regs[..13], "{mode}");
        for index in 0..TRANSFER_COUNT - 1 {
            assert_eq!(
                memory.read32(start + index * 4).unwrap(),
                interrupted_regs[index as usize + 1],
                "{mode}, beat={index}"
            );
        }

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!(cpu.regs[13], interrupted_regs[13], "{mode}, SVC SP");
    }
}

#[test]
fn a32_ldm_full_register_list_translation_abort_is_atomic_in_all_modes() {
    // r1-r14 plus PC: the last (PC) beat crosses into the unmapped section.
    const REGISTER_LIST: u32 = 0xFFFE;
    const TRANSFER_COUNT: u32 = 15;
    const SIZE: u32 = TRANSFER_COUNT * 4;
    const FAULT_ADDRESS: u32 = 0x0010_0000;
    let start = FAULT_ADDRESS - (TRANSFER_COUNT - 1) * 4;

    for (mode, pre_index, up) in [("IA", 0, 1), ("IB", 1, 1), ("DA", 0, 0), ("DB", 1, 0)] {
        let base = match (pre_index, up) {
            (0, 1) => start,
            (1, 1) => start - 4,
            (0, 0) => start + SIZE - 4,
            (1, 0) => start + SIZE,
            _ => unreachable!(),
        };
        let instruction =
            0xE800_0000 | (pre_index << 24) | (up << 23) | (1 << 21) | (1 << 20) | REGISTER_LIST;
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write32(0x100, instruction).unwrap();

        for index in 0..TRANSFER_COUNT - 1 {
            memory
                .write32(start + index * 4, 0x7100_0000 + index)
                .unwrap();
        }

        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(15).skip(1) {
            *value = 0xA100_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE, "{mode}");
        assert_eq!(cpu.pc(), 0x10, "{mode}");
        assert_eq!(cpu.regs[14], 0x108, "{mode}");
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr), "{mode}");
        assert_eq!(cpu.cp15().dfar(), FAULT_ADDRESS, "{mode}");
        assert_eq!(cpu.cp15().dfsr(), 0x5, "{mode}");
        assert_eq!(cpu.regs[..13], interrupted_regs[..13], "{mode}");
        for index in 0..TRANSFER_COUNT - 1 {
            assert_eq!(
                memory.read32(start + index * 4).unwrap(),
                0x7100_0000 + index,
                "{mode}, beat={index}"
            );
        }

        // SP/LR are banked on Abort entry; the interrupted SVC bank is intact.
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!(cpu.regs[13], interrupted_regs[13], "{mode}, SVC SP");
        assert_eq!(cpu.regs[14], interrupted_regs[14], "{mode}, SVC LR");
    }
}

#[test]
fn a32_stm_full_register_list_translation_abort_keeps_completed_stores_in_all_modes() {
    // r1-r14 plus PC: the final PC store faults after fourteen completed beats.
    const REGISTER_LIST: u32 = 0xFFFE;
    const TRANSFER_COUNT: u32 = 15;
    const SIZE: u32 = TRANSFER_COUNT * 4;
    const FAULT_ADDRESS: u32 = 0x0010_0000;
    let start = FAULT_ADDRESS - (TRANSFER_COUNT - 1) * 4;

    for (mode, pre_index, up) in [("IA", 0, 1), ("IB", 1, 1), ("DA", 0, 0), ("DB", 1, 0)] {
        let base = match (pre_index, up) {
            (0, 1) => start,
            (1, 1) => start - 4,
            (0, 0) => start + SIZE - 4,
            (1, 0) => start + SIZE,
            _ => unreachable!(),
        };
        let instruction = 0xE800_0000 | (pre_index << 24) | (up << 23) | (1 << 21) | REGISTER_LIST;
        let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
        memory.write32(0x100, instruction).unwrap();

        for index in 0..TRANSFER_COUNT - 1 {
            memory.write32(start + index * 4, 0xDEAD_BEEF).unwrap();
        }

        cpu.set_pc(0x100);
        cpu.regs[0] = base;
        for (register, value) in cpu.regs.iter_mut().enumerate().take(15).skip(1) {
            *value = 0xC100_0000 + register as u32;
        }
        let interrupted_regs = cpu.regs;
        let interrupted_cpsr = cpu.cpsr;

        cpu.step(&mut memory).unwrap();

        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE, "{mode}");
        assert_eq!(cpu.pc(), 0x10, "{mode}");
        assert_eq!(cpu.regs[14], 0x108, "{mode}");
        assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr), "{mode}");
        assert_eq!(cpu.cp15().dfar(), FAULT_ADDRESS, "{mode}");
        assert_eq!(cpu.cp15().dfsr(), 0x805, "{mode}");
        assert_eq!(cpu.regs[..13], interrupted_regs[..13], "{mode}");
        for index in 0..TRANSFER_COUNT - 1 {
            assert_eq!(
                memory.read32(start + index * 4).unwrap(),
                interrupted_regs[index as usize + 1],
                "{mode}, beat={index}"
            );
        }

        // The failed final beat must not commit writeback to the base register.
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!(cpu.regs[13], interrupted_regs[13], "{mode}, SVC SP");
        assert_eq!(cpu.regs[14], interrupted_regs[14], "{mode}, SVC LR");
    }
}

fn enter_fiq_with_distinct_banks_for_abort(cpu: &mut Arm11Core) -> ([u32; 7], [u32; 7]) {
    let user = [
        0xA000_0008,
        0xA000_0009,
        0xA000_000A,
        0xA000_000B,
        0xA000_000C,
        0xA000_000D,
        0xA000_000E,
    ];
    let fiq = [
        0xF000_0008,
        0xF000_0009,
        0xF000_000A,
        0xF000_000B,
        0xF000_000C,
        0xF000_000D,
        0xF000_000E,
    ];

    cpu.switch_mode(Psr::USER_MODE).unwrap();
    cpu.regs[8..15].copy_from_slice(&user);
    cpu.switch_mode(Psr::FIQ_MODE).unwrap();
    cpu.regs[8..15].copy_from_slice(&fiq);
    (user, fiq)
}

#[test]
fn a32_ldm_user_bank_data_abort_preserves_user_and_fiq_banks() {
    const BASE: u32 = 0x000F_FFF0;
    const INSTRUCTION: u32 = 0xE8D0_7F00; // LDMIA r0, {r8-r14}^

    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    let (user_values, fiq_values) = enter_fiq_with_distinct_banks_for_abort(&mut cpu);
    memory.write32(0x100, INSTRUCTION).unwrap();

    // Four words are mapped; the fifth word at 0x00100000 faults.
    for lane in 0..4 {
        memory.write32(BASE + lane * 4, 0x5100_0000 + lane).unwrap();
    }

    cpu.regs[0] = BASE;
    cpu.set_pc(0x100);
    let interrupted_cpsr = cpu.cpsr;
    let shared_before = cpu.regs[..8].to_vec();

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.regs[..8], shared_before);
    assert_eq!(cpu.regs[0], BASE);

    cpu.switch_mode(Psr::USER_MODE).unwrap();
    assert_eq!(&cpu.regs[8..15], &user_values);

    cpu.switch_mode(Psr::FIQ_MODE).unwrap();
    assert_eq!(&cpu.regs[8..15], &fiq_values);
}

#[test]
fn a32_stm_user_bank_data_abort_keeps_completed_user_stores_and_both_banks() {
    const BASE: u32 = 0x000F_FFF0;
    const INSTRUCTION: u32 = 0xE8C0_7F00; // STMIA r0, {r8-r14}^

    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    let (user_values, fiq_values) = enter_fiq_with_distinct_banks_for_abort(&mut cpu);
    memory.write32(0x100, INSTRUCTION).unwrap();

    for lane in 0..4 {
        memory.write32(BASE + lane * 4, 0xDEAD_BEEF).unwrap();
    }

    cpu.regs[0] = BASE;
    cpu.set_pc(0x100);
    let interrupted_cpsr = cpu.cpsr;
    let shared_before = cpu.regs[..8].to_vec();

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x805);
    assert_eq!(cpu.regs[..8], shared_before);
    assert_eq!(cpu.regs[0], BASE);

    // The four successful stores must use User-bank r8-r11, not FIQ r8-r11.
    for lane in 0..4 {
        assert_eq!(
            memory.read32(BASE + lane * 4).unwrap(),
            user_values[lane as usize],
            "completed user-bank store {lane}"
        );
    }

    cpu.switch_mode(Psr::USER_MODE).unwrap();
    assert_eq!(&cpu.regs[8..15], &user_values);

    cpu.switch_mode(Psr::FIQ_MODE).unwrap();
    assert_eq!(&cpu.regs[8..15], &fiq_values);
}

#[test]
fn a32_ldm_exception_return_data_abort_preserves_spsr_and_retries_after_abort_handler() {
    const BASE: u32 = 0x000F_FFF8;
    const INSTRUCTION: u32 = 0xE8F4_8003; // LDMIA r4!, {r0,r1,pc}^
    const SAVED_STATUS: Psr = Psr(Psr::USER_MODE | Psr::T | Psr::N | Psr::C);

    let (mut cpu, mut memory) = mmu_enabled_thumb_machine();
    memory.write32(0x100, INSTRUCTION).unwrap();
    memory.write32(BASE, 0xA0A0_0000).unwrap();
    memory.write32(BASE + 4, 0xB1B1_0001).unwrap();
    memory.write16(0x300, 0x2209).unwrap(); // MOVS r2, #9 in Thumb state.

    // The third LDM beat is at 0x00100000 and initially has no L1 descriptor.
    cpu.regs[0] = 0x1111_1111;
    cpu.regs[1] = 0x2222_2222;
    cpu.regs[4] = BASE;
    cpu.set_saved_program_status(SAVED_STATUS).unwrap();
    cpu.set_pc(0x100);
    let interrupted_cpsr = cpu.cpsr;
    let interrupted_regs = cpu.regs;

    cpu.step(&mut memory).unwrap();

    assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
    assert_eq!(cpu.pc(), 0x10);
    assert_eq!(cpu.regs[14], 0x108);
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));
    assert_eq!(cpu.cp15().dfar(), 0x0010_0000);
    assert_eq!(cpu.cp15().dfsr(), 0x5);
    assert_eq!(cpu.regs[0], interrupted_regs[0]);
    assert_eq!(cpu.regs[1], interrupted_regs[1]);
    assert_eq!(cpu.regs[4], BASE);

    // Confirm the fault did not consume or overwrite the Supervisor SPSR.
    cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
    assert_eq!(cpu.saved_program_status(), Some(SAVED_STATUS));
    cpu.switch_mode(Psr::ABORT_MODE).unwrap();
    assert_eq!(cpu.saved_program_status(), Some(interrupted_cpsr));

    // Map VA 0x00100000 onto PA 0, where the third LDM word is placed.
    // The failed translation was not cached, so the retry sees this descriptor.
    memory.write32(0x4004, 0b10 | (0b11 << 10)).unwrap();
    memory.write32(0x0000_0000, 0x0000_0301).unwrap();
    memory.write32(0x10, 0xE25E_F008).unwrap(); // SUBS pc, lr, #8

    cpu.step(&mut memory).unwrap(); // Abort handler returns to the original LDM.
    assert_eq!(cpu.cpsr, interrupted_cpsr);
    assert_eq!(cpu.pc(), 0x100);
    assert_eq!(cpu.regs[4], BASE);

    cpu.step(&mut memory).unwrap(); // Retried LDM returns through its original SPSR.
    assert_eq!(cpu.cpsr, SAVED_STATUS);
    assert!(cpu.cpsr.thumb());
    assert_eq!(cpu.pc(), 0x300);
    assert_eq!(cpu.regs[0], 0xA0A0_0000);
    assert_eq!(cpu.regs[1], 0xB1B1_0001);
    assert_eq!(cpu.regs[4], BASE + 12);
    assert_eq!(cpu.saved_program_status(), None);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.regs[2], 9);
    assert_eq!(cpu.pc(), 0x302);
}
