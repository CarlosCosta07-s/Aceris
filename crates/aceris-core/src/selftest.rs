//! Built-in CPU self-test.
//!
//! Runs a tiny hand-assembled program on the interpreter. It exercises ARM
//! code, a call into Thumb code with `BLX`, and the return to ARM with `BX`.
//! Every front-end can call this to prove the shared core works on its
//! platform.

use crate::cpu::{Arm11Core, StepEvent};
use crate::memory::{Bus, FlatMemory};
use aceris_common::{Error, Result};

/// Address of the Thumb routine. Bit 0 is set when it is used as a branch
/// target so that `BLX` enters Thumb state.
const THUMB_ENTRY: u32 = 0x20;

/// ARM section. Sums 1 through 10 into `r0`, calls the Thumb routine, then
/// issues `SWI #0`.
const ARM_CODE: [u32; 8] = [
    0xE3A0_0000,                     // 0x00  MOV   r0, #0
    0xE3A0_100A,                     // 0x04  MOV   r1, #10
    0xE080_0001,                     // 0x08  loop: ADD r0, r0, r1
    0xE251_1001,                     // 0x0C  SUBS  r1, r1, #1
    0x1AFF_FFFC,                     // 0x10  BNE   loop
    0xE3A0_2000 | (THUMB_ENTRY | 1), // 0x14  MOV   r2, #(THUMB_ENTRY | 1)
    0xE12F_FF32,                     // 0x18  BLX   r2
    0xEF00_0000,                     // 0x1C  SWI   #0
];

/// Thumb section. Adds 5 to `r0`, doubles it and returns to ARM state.
const THUMB_CODE: [u16; 3] = [
    0x3005, // ADD   r0, #5
    0x0040, // LSL   r0, r0, #1
    0x4770, // BX    lr
];

/// Value the program leaves in `r0`: (1 + 2 + ... + 10 + 5) * 2.
pub const EXPECTED: u32 = 120;

const MAX_STEPS: usize = 1_000;

/// Runs the self-test and returns the computed value ([`EXPECTED`] on success).
pub fn run() -> Result<u32> {
    let mut memory = FlatMemory::new(0, 0x100);
    for (i, word) in ARM_CODE.iter().enumerate() {
        memory.write32(i as u32 * 4, *word)?;
    }
    for (i, half) in THUMB_CODE.iter().enumerate() {
        memory.write16(THUMB_ENTRY + i as u32 * 2, *half)?;
    }

    let mut cpu = Arm11Core::new();
    for _ in 0..MAX_STEPS {
        if cpu.step(&mut memory)? == StepEvent::Svc(0) {
            if cpu.cpsr.thumb() {
                return Err(Error::SelfTest("did not return to ARM state"));
            }
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
    fn runs_arm_and_thumb_code_and_returns_to_arm() {
        assert_eq!(run().unwrap(), EXPECTED);
    }

    #[test]
    fn expected_value_matches_the_program_description() {
        assert_eq!(EXPECTED, (55 + 5) * 2);
    }
}
