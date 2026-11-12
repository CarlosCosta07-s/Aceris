//! ARM11 CPU core.
//!
//! Current scope: an interpreter for ARM (A32) and classic Thumb instruction
//! sets with banked ARMv6 exception-mode registers and per-mode SPSR storage.
//! CP15, VFPv2, and ARMv6 short-descriptor MMU state live in dedicated modules.

mod arm;
mod condition;
mod coprocessor;
mod cp15;
mod faults;
mod mmu;
mod psr;
mod shifter;
mod thumb;
mod vfp;

pub use condition::Condition;
pub use cp15::Cp15;
pub use psr::Psr;
pub use vfp::Vfp;

use crate::memory::Bus;
use aceris_common::{Error, Result};

/// Register indices with a special role.
pub const SP: usize = 13;
pub const LR: usize = 14;
pub const PC: usize = 15;

const ARM_INSTRUCTION_SIZE: u32 = 4;
const THUMB_INSTRUCTION_SIZE: u32 = 2;

/// What happened when the CPU executed one instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepEvent {
    /// A CPU step completed, either by executing an instruction or accepting an interrupt.
    Executed,
    /// A supervisor-call instruction was executed; the payload is its encoded
    /// comment field. Hardware exception entry is optional and disabled by default
    /// so HLE service dispatch can intercept this event.
    Svc(u32),
    /// A BKPT instruction was encountered. The immediate is retained for a
    /// debugger or the process exception layer.
    Breakpoint(u16),
}

/// One outstanding exclusive-access reservation for this processor context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExclusiveReservation {
    address: u32,
    size: u8,
    version: u64,
}

/// Register banks shared by ARMv6 User/System and exception modes.
#[derive(Clone, Debug, Default)]
struct BankedRegisters {
    user_r8_r12: [u32; 5],
    user_sp_lr: [u32; 2],
    fiq_r8_r14: [u32; 7],
    irq_sp_lr: [u32; 2],
    svc_sp_lr: [u32; 2],
    abort_sp_lr: [u32; 2],
    undefined_sp_lr: [u32; 2],
}

/// Saved program status registers belonging to exception modes.
#[derive(Clone, Debug, Default)]
struct SavedStatusRegisters {
    fiq: Option<Psr>,
    irq: Option<Psr>,
    svc: Option<Psr>,
    abort: Option<Psr>,
    undefined: Option<Psr>,
}

impl SavedStatusRegisters {
    fn get(&self, mode: u32) -> Option<Psr> {
        match mode {
            Psr::FIQ_MODE => self.fiq,
            Psr::IRQ_MODE => self.irq,
            Psr::SUPERVISOR_MODE => self.svc,
            Psr::ABORT_MODE => self.abort,
            Psr::UNDEFINED_MODE => self.undefined,
            _ => None,
        }
    }

    fn set(&mut self, mode: u32, value: Psr) -> bool {
        let slot = match mode {
            Psr::FIQ_MODE => &mut self.fiq,
            Psr::IRQ_MODE => &mut self.irq,
            Psr::SUPERVISOR_MODE => &mut self.svc,
            Psr::ABORT_MODE => &mut self.abort,
            Psr::UNDEFINED_MODE => &mut self.undefined,
            _ => return false,
        };
        *slot = Some(value);
        true
    }
}

/// One ARM11 processor core with banked exception-mode registers.
#[derive(Clone, Debug, Default)]
pub struct Arm11Core {
    /// `r0`-`r15`. While an instruction executes, `r15` holds the address of
    /// that instruction; operand reads of `pc` add the pipeline offset.
    pub regs: [u32; 16],
    pub cpsr: Psr,
    branch_taken: bool,
    exclusive_reservation: Option<ExclusiveReservation>,
    banked: BankedRegisters,
    saved_status: SavedStatusRegisters,
    high_vectors_enabled: bool,
    exception_big_endian: bool,
    svc_exception_entry_enabled: bool,
    irq_line_asserted: bool,
    fiq_line_asserted: bool,
    cp15: Cp15,
    tlb: mmu::TranslationLookasideBuffer,
    vfp: Vfp,
    fault_exception_entry_enabled: bool,
    /// Set while one step runs: the fault being reported came from the fetch.
    prefetch_fault: bool,
    /// Set while one step runs: the MMU already latched FSR/FAR for this fault.
    fault_latched: bool,
}

impl Arm11Core {
    pub fn new() -> Self {
        Self::default()
    }

    /// Changes the active processor mode while preserving all banked registers.
    ///
    /// User and System share r13-r14; FIQ has private r8-r14; IRQ, Supervisor,
    /// Abort and Undefined each have private r13-r14. Registers r0-r7 are
    /// common to every mode.
    pub fn switch_mode(&mut self, mode: u32) -> Result<()> {
        if !Psr::is_valid_mode(mode) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: mode,
            });
        }
        let old_mode = self.cpsr.mode();
        if old_mode == mode {
            self.cpsr.0 = (self.cpsr.0 & !Psr::MODE_MASK) | mode;
            return Ok(());
        }

        if old_mode == Psr::FIQ_MODE {
            self.banked.fiq_r8_r14.copy_from_slice(&self.regs[8..15]);
        } else {
            self.banked.user_r8_r12.copy_from_slice(&self.regs[8..13]);
            self.save_sp_lr(old_mode);
        }

        if mode == Psr::FIQ_MODE {
            self.regs[8..15].copy_from_slice(&self.banked.fiq_r8_r14);
        } else {
            if old_mode == Psr::FIQ_MODE {
                self.regs[8..13].copy_from_slice(&self.banked.user_r8_r12);
            }
            self.load_sp_lr(mode);
        }

        self.cpsr.0 = (self.cpsr.0 & !Psr::MODE_MASK) | mode;
        Ok(())
    }

    fn save_sp_lr(&mut self, mode: u32) {
        let values = [self.regs[SP], self.regs[LR]];
        let bank = match mode {
            Psr::USER_MODE | Psr::SYSTEM_MODE => &mut self.banked.user_sp_lr,
            Psr::IRQ_MODE => &mut self.banked.irq_sp_lr,
            Psr::SUPERVISOR_MODE => &mut self.banked.svc_sp_lr,
            Psr::ABORT_MODE => &mut self.banked.abort_sp_lr,
            Psr::UNDEFINED_MODE => &mut self.banked.undefined_sp_lr,
            _ => return,
        };
        *bank = values;
    }

    fn load_sp_lr(&mut self, mode: u32) {
        let values = match mode {
            Psr::USER_MODE | Psr::SYSTEM_MODE => self.banked.user_sp_lr,
            Psr::IRQ_MODE => self.banked.irq_sp_lr,
            Psr::SUPERVISOR_MODE => self.banked.svc_sp_lr,
            Psr::ABORT_MODE => self.banked.abort_sp_lr,
            Psr::UNDEFINED_MODE => self.banked.undefined_sp_lr,
            _ => return,
        };
        self.regs[SP] = values[0];
        self.regs[LR] = values[1];
    }

    /// Configures the CP15-controlled exception vector and endianness state.
    ///
    /// Kept in sync automatically when the guest writes SCTLR through CP15; both
    /// default to the low-vector, little-endian reset profile.
    pub fn configure_exception_entry(
        &mut self,
        high_vectors_enabled: bool,
        exception_big_endian: bool,
    ) {
        self.high_vectors_enabled = high_vectors_enabled;
        self.exception_big_endian = exception_big_endian;
    }

    /// Enables or disables hardware SVC exception entry.
    ///
    /// Disabled by default so the HLE session can intercept StepEvent::Svc
    /// and dispatch OS services without requiring a mapped vector page.
    pub fn set_svc_exception_entry_enabled(&mut self, enabled: bool) {
        self.svc_exception_entry_enabled = enabled;
    }

    /// Drives the external IRQ input. The line remains asserted until the
    /// interrupt controller deasserts it; CPSR.I masks acceptance, not the line.
    pub fn set_irq_line(&mut self, asserted: bool) {
        self.irq_line_asserted = asserted;
    }

    /// Drives the external FIQ input. FIQ has priority over IRQ when both
    /// lines are asserted and CPSR.F permits its acceptance.
    pub fn set_fiq_line(&mut self, asserted: bool) {
        self.fiq_line_asserted = asserted;
    }

    /// Enters an ARMv6 exception, banking the old CPSR and selecting its vector.
    fn enter_exception(
        &mut self,
        mode: u32,
        vector_offset: u32,
        return_address: u32,
        interrupt_masks: u32,
    ) -> Result<()> {
        let previous_cpsr = self.cpsr;
        self.switch_mode(mode)?;
        self.set_saved_program_status(previous_cpsr)?;
        self.regs[LR] = return_address;

        let mut entered_cpsr = previous_cpsr.0 & !(Psr::MODE_MASK | Psr::T | Psr::E);
        entered_cpsr |= mode | interrupt_masks;
        if self.exception_big_endian {
            entered_cpsr |= Psr::E;
        }
        self.cpsr.0 = entered_cpsr;

        let vector_base: u32 = if self.high_vectors_enabled {
            0xFFFF_0000
        } else {
            0
        };
        self.branch(vector_base.wrapping_add(vector_offset));
        Ok(())
    }

    /// Enters the ARMv6 Supervisor exception and preserves the previous CPSR.
    /// SVC uses vector offset 0x08 and masks IRQ on entry.
    fn enter_svc_exception(&mut self, return_address: u32) -> Result<()> {
        self.enter_exception(Psr::SUPERVISOR_MODE, 0x08, return_address, Psr::I)
    }

    /// Accepts a pending FIQ before IRQ, provided its CPSR mask is clear.
    /// Returns true when an exception was entered instead of fetching an
    /// instruction. The interrupt input remains level-sensitive.
    fn service_pending_interrupt(&mut self) -> Result<bool> {
        let return_address = self.pc().wrapping_add(4);
        if self.fiq_line_asserted && self.cpsr.0 & Psr::F == 0 {
            self.enter_exception(Psr::FIQ_MODE, 0x1C, return_address, Psr::I | Psr::F)?;
            return Ok(true);
        }
        if self.irq_line_asserted && self.cpsr.0 & Psr::I == 0 {
            self.enter_exception(Psr::IRQ_MODE, 0x18, return_address, Psr::I)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Returns the SPSR belonging to the active exception mode, if one exists.
    pub fn saved_program_status(&self) -> Option<Psr> {
        self.saved_status.get(self.cpsr.mode())
    }

    /// Stores the SPSR belonging to the active exception mode.
    /// User and System modes do not have an SPSR.
    pub fn set_saved_program_status(&mut self, value: Psr) -> Result<()> {
        if !self.saved_status.set(self.cpsr.mode(), value) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: self.cpsr.mode(),
            });
        }
        Ok(())
    }

    /// CP15 system control state.
    pub fn cp15(&self) -> &Cp15 {
        &self.cp15
    }

    /// Mutable CP15 state, e.g. for the kernel layer to set the thread ID on a context switch.
    pub fn cp15_mut(&mut self) -> &mut Cp15 {
        &mut self.cp15
    }

    pub fn pc(&self) -> u32 {
        self.regs[PC]
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.regs[PC] = pc;
    }

    /// Clears the processor's local exclusive-access reservation.
    pub fn clear_exclusive_monitor(&mut self) {
        self.exclusive_reservation = None;
    }

    /// Fetches and executes one instruction.
    ///
    /// With fault-exception entry enabled (see `set_fault_exception_entry_enabled`),
    /// undefined instructions and aborts become hardware exceptions instead of errors.
    pub fn step<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<StepEvent> {
        if !self.fault_exception_entry_enabled {
            return self.step_instruction(bus);
        }
        let snapshot = (self.regs, self.cpsr);
        self.prefetch_fault = false;
        self.fault_latched = false;
        match self.step_instruction(bus) {
            Err(error) => self.take_fault_exception(snapshot, error),
            ok => ok,
        }
    }

    fn step_instruction<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<StepEvent> {
        let pc = self.pc();
        self.branch_taken = false;

        if self.service_pending_interrupt()? {
            return Ok(StepEvent::Executed);
        }

        if self.cp15.mmu_enabled() {
            return self.step_translated(bus, pc);
        }

        let thumb = self.cpsr.thumb();
        let fetched = if thumb {
            bus.read16(pc).map(u32::from)
        } else {
            bus.read32(pc)
        };
        let instruction = fetched.inspect_err(|_| self.prefetch_fault = true)?;
        let (event, size) = if thumb {
            (
                self.execute_thumb(bus, instruction)?,
                THUMB_INSTRUCTION_SIZE,
            )
        } else {
            (self.execute_arm(bus, instruction)?, ARM_INSTRUCTION_SIZE)
        };

        if !self.branch_taken {
            self.regs[PC] = pc.wrapping_add(size);
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;
    use aceris_common::Error;

    fn memory_with(word: u32) -> FlatMemory {
        let mut mem = FlatMemory::new(0, 16);
        mem.write32(0, word).unwrap();
        mem
    }

    #[test]
    fn mode_switches_preserve_user_system_fiq_and_exception_register_banks() {
        let mut cpu = Arm11Core::new();
        cpu.regs[8..13].copy_from_slice(&[8, 9, 10, 11, 12]);
        cpu.regs[SP] = 0x1000;
        cpu.regs[LR] = 0x1004;

        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[0, 0, 0, 0, 0]);
        cpu.regs[8..15].copy_from_slice(&[0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xF13, 0xF14]);

        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[8, 9, 10, 11, 12]);
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0, 0));
        cpu.regs[SP] = 0x2000;
        cpu.regs[LR] = 0x2004;

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0, 0));
        cpu.regs[SP] = 0x3000;
        cpu.regs[LR] = 0x3004;

        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[8, 9, 10, 11, 12]);
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x1000, 0x1004));

        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x2000, 0x2004));
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x3000, 0x3004));

        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        assert_eq!(
            &cpu.regs[8..15],
            &[0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xF13, 0xF14]
        );
        cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[8, 9, 10, 11, 12]);
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x1000, 0x1004));
    }

    #[test]
    fn saved_program_status_registers_are_private_to_exception_modes() {
        let mut cpu = Arm11Core::new();
        assert_eq!(cpu.saved_program_status(), None);
        assert!(cpu.set_saved_program_status(Psr(0xA000_0010)).is_err());

        let fiq_spsr = Psr(0xF000_0011);
        let irq_spsr = Psr(0xA000_0012);
        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        cpu.set_saved_program_status(fiq_spsr).unwrap();
        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        cpu.set_saved_program_status(irq_spsr).unwrap();
        assert_eq!(cpu.saved_program_status(), Some(irq_spsr));

        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        assert_eq!(cpu.saved_program_status(), Some(fiq_spsr));
        cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        assert_eq!(cpu.saved_program_status(), None);
    }

    #[test]
    fn cpsr_mode_is_the_single_source_of_truth_for_banks_and_spsr() {
        let mut cpu = Arm11Core::new();
        let supervisor_spsr = Psr(0xA000_0010);

        // CPSR is public and may be restored by a context loader. Subsequent
        // mode-sensitive operations must follow it rather than stale metadata.
        cpu.cpsr = Psr(Psr::SUPERVISOR_MODE);
        cpu.regs[SP] = 0x3000;
        cpu.regs[LR] = 0x3004;
        cpu.set_saved_program_status(supervisor_spsr).unwrap();

        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0, 0));
        assert_eq!(cpu.saved_program_status(), None);

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x3000, 0x3004));
        assert_eq!(cpu.saved_program_status(), Some(supervisor_spsr));
    }

    #[test]
    fn invalid_processor_modes_are_rejected_without_changing_the_active_bank() {
        let mut cpu = Arm11Core::new();
        cpu.regs[SP] = 0x1234;
        assert!(cpu.switch_mode(0x15).is_err());
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(cpu.regs[SP], 0x1234);
    }

    #[test]
    fn irq_waits_for_unmasking_then_enters_irq_vector_and_returns() {
        let mut memory = FlatMemory::new(0, 0x40);
        memory.write32(0, 0xE1A0_0000).unwrap(); // MOV r0,r0
        memory.write32(0x18, 0xE25E_F004).unwrap(); // SUBS pc,lr,#4
        let mut cpu = Arm11Core::new();
        cpu.set_irq_line(true);
        cpu.cpsr.0 |= Psr::I;

        // A masked IRQ remains asserted while the current instruction runs.
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);

        cpu.cpsr.0 &= !Psr::I;
        let interrupted_status = cpu.cpsr;
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
        assert_eq!(cpu.pc(), 0x18);
        assert_eq!(cpu.regs[LR], 8);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_status));
        assert_ne!(cpu.cpsr.0 & Psr::I, 0);

        // Deassert the level before the handler returns to the interrupted PC.
        cpu.set_irq_line(false);
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
        assert_eq!(cpu.cpsr, interrupted_status);
    }

    #[test]
    fn fiq_has_priority_and_sets_both_interrupt_masks() {
        let mut memory = FlatMemory::new(0, 4);
        let mut cpu = Arm11Core::new();
        cpu.set_irq_line(true);
        cpu.set_fiq_line(true);
        let interrupted_status = cpu.cpsr;

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);
        assert_eq!(cpu.pc(), 0x1C);
        assert_eq!(cpu.regs[LR], 4);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_status));
        assert_eq!(cpu.cpsr.0 & (Psr::I | Psr::F), Psr::I | Psr::F);
    }

    #[test]
    fn masked_fiq_does_not_block_an_unmasked_irq() {
        let mut memory = FlatMemory::new(0, 4);
        let mut cpu = Arm11Core::new();
        cpu.cpsr.0 |= Psr::F;
        cpu.set_irq_line(true);
        cpu.set_fiq_line(true);

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
        assert_eq!(cpu.pc(), 0x18);
        assert_ne!(cpu.cpsr.0 & Psr::I, 0);
        assert_ne!(cpu.cpsr.0 & Psr::F, 0);
    }

    #[test]
    fn interrupt_entry_uses_high_vectors_and_exception_endianness() {
        let mut memory = FlatMemory::new(0, 4);
        let mut cpu = Arm11Core::new();
        cpu.configure_exception_entry(true, true);
        cpu.set_irq_line(true);

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 0xFFFF_0018);
        assert!(cpu.cpsr.big_endian());
        assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    }

    #[test]
    fn instruction_with_failing_condition_is_skipped() {
        // `BNE` encoding; Z is set, so it must not execute.
        let mut mem = memory_with(0x1A00_0000);
        let mut cpu = Arm11Core::new();
        cpu.cpsr = Psr::from_flags(false, true, false, false);

        assert_eq!(cpu.step(&mut mem).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
    }

    #[test]
    fn fetch_from_unmapped_memory_fails() {
        let mut mem = FlatMemory::new(0x1000, 16);
        let mut cpu = Arm11Core::new();
        assert!(matches!(
            cpu.step(&mut mem),
            Err(Error::InvalidAccess { .. })
        ));
    }

    #[test]
    fn thumb_state_fetches_halfwords() {
        let mut mem = memory_with(0x2105_2007); // MOV r0,#7 ; MOV r1,#5
        let mut cpu = Arm11Core::new();
        cpu.cpsr.set_thumb(true);
        cpu.step(&mut mem).unwrap();
        cpu.step(&mut mem).unwrap();
        assert_eq!((cpu.regs[0], cpu.regs[1], cpu.pc()), (7, 5, 4));
    }
}
