//! Built-in CPU self-test.
//!
//! Runs a tiny hand-assembled ARM program on the interpreter. Every front-end
//! can call this to prove the shared core works on its platform.

use crate::cpu::{Arm11Core, StepEvent};
use crate::memory::{Bus, FlatMemory};
use aceris_common::{Error, Result};

/// Sums the integers 1 through 10 into `r0`, then issues `SWI #0`.
const PROGRAM: [u32; 6] = [
    0xE3A0_0000, // MOV   r0, #0
    0xE3A0_100A, // MOV   r1, #10
    0xE080_0001, // loop: ADD r0, r0, r1
    0xE251_1001, // SUBS  r1, r1, #1
    0x1AFF_FFFC, // BNE   loop
    0xEF00_0000, // SWI   #0
];

const EXPECTED: u32 = 55;
const MAX_STEPS: usize = 1_000;

/// Runs the self-test and returns the computed value (always 55 on success).
pub fn run() -> Result<u32> {
    let mut memory = FlatMemory::new(0, 0x100);
    for (i, word) in PROGRAM.iter().enumerate() {
        memory.write32(i as u32 * 4, *word)?;
    }

    let mut cpu = Arm11Core::new();
    for _ in 0..MAX_STEPS {
        if cpu.step(&mut memory)? == StepEvent::Svc(0) {
            return if cpu.regs[0] == EXPECTED {
                Ok(cpu.regs[0])
            } else {
                Err(Error::SelfTest("unexpected result"))
            };
        }
    }
    Err(Error::SelfTest("program did not finish"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_one_to_ten() {
        assert_eq!(run().unwrap(), 55);
    }
}
