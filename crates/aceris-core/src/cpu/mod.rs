//! ARM11 CPU core.
//!
//! Current scope: an interpreter for ARM (A32) and classic Thumb instruction
//! sets with banked ARMv6 exception-mode registers and per-mode SPSR storage.
//! Exception entry/return, CP15, MMU and VFP remain separate milestones.

mod arm;
mod condition;
mod psr;
mod shifter;
mod thumb;

pub use condition::Condition;
pub use psr::Psr;

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
    /// A normal instruction completed.
    Executed,
    /// A supervisor call was executed and Supervisor exception entry completed.
    /// The payload is the instruction's 24-bit (A32) or 8-bit (Thumb) comment.
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
#[derive(Clone, Debug)]
pub struct Arm11Core {
    /// `r0`-`r15`. While an instruction executes, `r15` holds the address of
    /// that instruction; operand reads of `pc` add the pipeline offset.
    pub regs: [u32; 16],
    pub cpsr: Psr,
    branch_taken: bool,
    exclusive_reservation: Option<ExclusiveReservation>,
    banked: BankedRegisters,
    saved_status: SavedStatusRegisters,
    active_mode: u32,
    high_vectors_enabled: bool,
    exception_big_endian: bool,
}

impl Default for Arm11Core {
    fn default() -> Self {
        Self {
            regs: [0; 16],
            cpsr: Psr::default(),
            branch_taken: false,
            exclusive_reservation: None,
            banked: BankedRegisters::default(),
            saved_status: SavedStatusRegisters::default(),
            active_mode: Psr::USER_MODE,
            high_vectors_enabled: false,
            exception_big_endian: false,
        }
    }
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
        let old_mode = self.active_mode;
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

        self.active_mode = mode;
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
    /// This is an integration point for SCTLR.V and SCTLR.EE; both default to
    /// the low-vector, little-endian reset profile until CP15 is implemented.
    pub fn configure_exception_entry(
        &mut self,
        high_vectors_enabled: bool,
        exception_big_endian: bool,
    ) {
        self.high_vectors_enabled = high_vectors_enabled;
        self.exception_big_endian = exception_big_endian;
    }

    /// Enters the ARMv6 Supervisor exception and preserves the previous CPSR.
    ///
    /// The current exception entry profile uses the configured vector base,
    /// switches to ARM state, and masks IRQ. The SVC vector is at offset 0x08.
    fn enter_svc_exception(&mut self, return_address: u32) -> Result<()> {
        let previous_cpsr = self.cpsr;
        self.switch_mode(Psr::SUPERVISOR_MODE)?;
        self.set_saved_program_status(previous_cpsr)?;
        self.regs[LR] = return_address;

        let mut entered_cpsr = previous_cpsr.0 & !(Psr::MODE_MASK | (1 << 5) | (1 << 7) | (1 << 9));
        entered_cpsr |= Psr::SUPERVISOR_MODE | (1 << 7);
        if self.exception_big_endian {
            entered_cpsr |= 1 << 9;
        }
        self.cpsr.0 = entered_cpsr;

        let vector_base = if self.high_vectors_enabled {
            0xFFFF_0000
        } else {
            0
        };
        self.branch(vector_base + 0x08);
        Ok(())
    }

    /// Returns the SPSR belonging to the active exception mode, if one exists.
    pub fn saved_program_status(&self) -> Option<Psr> {
        self.saved_status.get(self.active_mode)
    }

    /// Stores the SPSR belonging to the active exception mode.
    /// User and System modes do not have an SPSR.
    pub fn set_saved_program_status(&mut self, value: Psr) -> Result<()> {
        if !self.saved_status.set(self.active_mode, value) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: self.cpsr.mode(),
            });
        }
        Ok(())
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
    pub fn step<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<StepEvent> {
        let pc = self.pc();
        self.branch_taken = false;

        let (event, size) = if self.cpsr.thumb() {
            let half = u32::from(bus.read16(pc)?);
            (self.execute_thumb(bus, half)?, THUMB_INSTRUCTION_SIZE)
        } else {
            let word = bus.read32(pc)?;
            (self.execute_arm(bus, word)?, ARM_INSTRUCTION_SIZE)
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
    fn invalid_processor_modes_are_rejected_without_changing_the_active_bank() {
        let mut cpu = Arm11Core::new();
        cpu.regs[SP] = 0x1234;
        assert!(cpu.switch_mode(0x15).is_err());
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(cpu.regs[SP], 0x1234);
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
