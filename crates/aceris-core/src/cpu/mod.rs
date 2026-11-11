//! ARM11 CPU core.
//!
//! Current scope: an interpreter for the ARM (A32) and Thumb instruction sets
//! in a single user-mode register bank. Banked registers, privileged modes,
//! coprocessors and VFP are still to come.

mod arm;
mod condition;
mod psr;
mod shifter;
mod thumb;

pub use condition::Condition;
pub use psr::Psr;

use crate::memory::Bus;
use aceris_common::Result;

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
    /// A supervisor call was executed; the payload is its 24-bit comment
    /// field. `pc` already points to the following instruction.
    Svc(u32),
    /// A BKPT instruction was encountered. The immediate is retained for a
    /// debugger or the process exception layer.
    Breakpoint(u16),
}

/// A single ARM11 core.
#[derive(Clone, Debug, Default)]
pub struct Arm11Core {
    /// `r0`-`r15`. While an instruction executes, `r15` holds the address of
    /// that instruction; operand reads of `pc` add the pipeline offset.
    pub regs: [u32; 16],
    pub cpsr: Psr,
    branch_taken: bool,
}

impl Arm11Core {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pc(&self) -> u32 {
        self.regs[PC]
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.regs[PC] = pc;
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
