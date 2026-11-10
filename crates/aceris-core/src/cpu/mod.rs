//! ARM11 CPU core (work in progress).

mod condition;
mod psr;

pub use condition::Condition;
pub use psr::Psr;

use crate::memory::Bus;
use aceris_common::{Error, Result};

/// Index of the program counter in the register file.
pub const PC: usize = 15;

const ARM_INSTRUCTION_SIZE: u32 = 4;

/// A single ARM11 core.
#[derive(Clone, Debug, Default)]
pub struct Arm11Core {
    pub regs: [u32; 16],
    pub cpsr: Psr,
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

    /// Fetches and executes one ARM instruction.
    ///
    /// Only fetch and condition-code evaluation exist so far: instructions
    /// whose condition fails are skipped, everything else reports
    /// [`Error::Unimplemented`].
    pub fn step<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<()> {
        let word = bus.read32(self.pc())?;
        let condition = Condition::from_bits((word >> 28) as u8);

        if !condition.passes(self.cpsr) {
            self.set_pc(self.pc().wrapping_add(ARM_INSTRUCTION_SIZE));
            return Ok(());
        }

        Err(Error::Unimplemented("ARM instruction execution"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

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

        cpu.step(&mut mem).unwrap();
        assert_eq!(cpu.pc(), 4);
    }

    #[test]
    fn executable_instruction_is_reported_as_unimplemented() {
        // `MOV r0, r0` with the always condition.
        let mut mem = memory_with(0xE1A0_0000);
        let mut cpu = Arm11Core::new();

        assert!(matches!(cpu.step(&mut mem), Err(Error::Unimplemented(_))));
        assert_eq!(cpu.pc(), 0);
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
}
