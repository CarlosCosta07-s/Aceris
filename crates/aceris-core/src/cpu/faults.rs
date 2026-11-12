//! Hardware exception entry for undefined instructions and memory aborts.
//!
//! Disabled by default: the HLE kernel prefers to see these conditions as
//! errors. A guest-kernel configuration enables it with
//! [`Arm11Core::set_fault_exception_entry_enabled`].
//!
//! Mapping from interpreter errors: `UndefinedInstruction` enters the Undefined
//! exception (vector 0x04); `InvalidAccess` raised while fetching enters the
//! Prefetch Abort (0x0C) and otherwise the Data Abort (0x10). Other errors
//! (`Unimplemented`, `SelfTest`) describe emulator limits, not guest behavior,
//! and are still returned. Registers and CPSR are rolled back to their values
//! before the faulting instruction; memory writes already performed by a
//! partially completed store are not undone.

use super::{Arm11Core, Psr, StepEvent};
use aceris_common::{Error, Result};

/// FSR status for a precise external abort, used when the bus (not the MMU) faulted.
const FSR_EXTERNAL_ABORT: u32 = 0x8;

impl Arm11Core {
    /// Enables or disables hardware entry for undefined-instruction and abort exceptions.
    pub fn set_fault_exception_entry_enabled(&mut self, enabled: bool) {
        self.fault_exception_entry_enabled = enabled;
    }

    pub(super) fn take_fault_exception(
        &mut self,
        (regs, cpsr): ([u32; 16], Psr),
        error: Error,
    ) -> Result<StepEvent> {
        let size = if cpsr.thumb() { 2 } else { 4 };
        self.regs = regs;
        let pc = regs[super::PC];
        match error {
            Error::UndefinedInstruction { .. } => {
                self.cpsr = cpsr;
                self.enter_exception(Psr::UNDEFINED_MODE, 0x04, pc.wrapping_add(size), Psr::I)?;
            }
            Error::InvalidAccess { addr } => {
                self.cpsr = cpsr;
                if self.prefetch_fault {
                    if !self.fault_latched {
                        self.cp15.latch_prefetch_fault(FSR_EXTERNAL_ABORT, addr);
                    }
                    self.enter_exception(
                        Psr::ABORT_MODE,
                        0x0C,
                        pc.wrapping_add(4),
                        Psr::I | Psr::A,
                    )?;
                } else {
                    if !self.fault_latched {
                        self.cp15.latch_data_fault(FSR_EXTERNAL_ABORT, addr, false);
                    }
                    self.enter_exception(
                        Psr::ABORT_MODE,
                        0x10,
                        pc.wrapping_add(8),
                        Psr::I | Psr::A,
                    )?;
                }
            }
            other => return Err(other),
        }
        Ok(StepEvent::Executed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Bus, FlatMemory};

    fn machine(program: &[u32]) -> (Arm11Core, FlatMemory) {
        let mut mem = FlatMemory::new(0, 0x1000);
        for (i, word) in program.iter().enumerate() {
            mem.write32(0x100 + i as u32 * 4, *word).unwrap();
        }
        let mut cpu = Arm11Core::new();
        cpu.set_fault_exception_entry_enabled(true);
        cpu.set_pc(0x100);
        (cpu, mem)
    }

    #[test]
    fn disabled_by_default_so_faults_stay_errors() {
        let mut mem = FlatMemory::new(0, 0x1000);
        mem.write32(0, 0xE7F0_00F0).unwrap(); // permanently undefined
        let mut cpu = Arm11Core::new();
        assert!(matches!(
            cpu.step(&mut mem),
            Err(Error::UndefinedInstruction { .. })
        ));
    }

    #[test]
    fn undefined_instruction_enters_the_undefined_exception() {
        let (mut cpu, mut mem) = machine(&[0xE7F0_00F0]);
        let before = cpu.cpsr;
        assert_eq!(cpu.step(&mut mem).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::UNDEFINED_MODE);
        assert_eq!(cpu.pc(), 0x04);
        assert_eq!(cpu.regs[14], 0x104);
        assert_eq!(cpu.saved_program_status(), Some(before));
        assert_ne!(cpu.cpsr.0 & Psr::I, 0);
    }

    #[test]
    fn thumb_undefined_return_address_is_the_next_halfword() {
        let (mut cpu, mut mem) = machine(&[0]);
        mem.write16(0x100, 0xDE00).unwrap(); // UDF #0
        cpu.cpsr.set_thumb(true);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.cpsr.mode(), Psr::UNDEFINED_MODE);
        assert_eq!(cpu.regs[14], 0x102);
        assert!(!cpu.cpsr.thumb());
    }

    #[test]
    fn data_abort_restores_registers_and_latches_the_fault() {
        // ldmia r0!, {r1,r2}: the second word lies outside memory.
        let (mut cpu, mut mem) = machine(&[0xE8B0_0006]);
        cpu.regs[0] = 0xFFC;
        cpu.regs[1] = 0x1234;
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert_eq!(cpu.pc(), 0x10);
        assert_eq!(cpu.regs[14], 0x108);
        assert_eq!(cpu.cp15.dfar(), 0x1000);
        assert_eq!(cpu.cp15.dfsr(), FSR_EXTERNAL_ABORT);
        assert_ne!(cpu.cpsr.0 & Psr::A, 0);
        // r0/r1 are user-bank registers shared with the abort mode.
        assert_eq!((cpu.regs[0], cpu.regs[1]), (0xFFC, 0x1234));
    }

    #[test]
    fn prefetch_abort_uses_its_own_vector_and_return_address() {
        let (mut cpu, mut mem) = machine(&[]);
        cpu.set_pc(0x2000);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert_eq!(cpu.pc(), 0x0C);
        assert_eq!(cpu.regs[14], 0x2004);
        assert_eq!(cpu.cp15.ifar(), 0x2000);
    }

    #[test]
    fn high_vectors_apply_to_fault_exceptions() {
        let (mut cpu, mut mem) = machine(&[0xE7F0_00F0]);
        cpu.configure_exception_entry(true, false);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.pc(), 0xFFFF_0004);
    }

    #[test]
    fn emulator_limits_are_not_turned_into_guest_exceptions() {
        // An unimplemented coprocessor is our gap, not a guest fault.
        let (mut cpu, mut mem) = machine(&[0xEE00_0000]);
        assert!(matches!(cpu.step(&mut mem), Err(Error::Unimplemented(_))));
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
    }

    #[test]
    fn mmu_faults_keep_the_status_latched_by_the_translation() {
        let mut mem = FlatMemory::new(0, 0x8000);
        // L1 table at 0x4000: VA 0..1 MiB maps to PA 0; everything else faults.
        mem.write32(0x4000, 0b10 | (0b11 << 10)).unwrap();
        let mut cpu = Arm11Core::new();
        cpu.set_fault_exception_entry_enabled(true);
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.cp15.write(true, 0, 2, 0, 0, 0x4000).unwrap();
        cpu.cp15.write(true, 0, 3, 0, 0, 0b01).unwrap();
        cpu.cp15.write(true, 0, 1, 0, 0, 1 | (1 << 23)).unwrap();
        cpu.set_pc(0x7000_0000);
        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert_eq!(cpu.cp15.ifsr(), 0x5);
        assert_eq!(cpu.cp15.ifar(), 0x7000_0000);
        assert_eq!(cpu.regs[14], 0x7000_0004);
    }
}
