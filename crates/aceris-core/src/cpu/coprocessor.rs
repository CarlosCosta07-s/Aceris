//! Coprocessor instruction dispatch (ARM encodings with bits 27:26 = 0b11, not SVC).

use super::arm::{bit, bits};
use super::cp15::Cp15Fault;
use super::{Arm11Core, StepEvent};
use crate::memory::Bus;
use aceris_common::{Error, Result};

impl Arm11Core {
    /// Routes an already condition-checked coprocessor instruction.
    pub(super) fn execute_coprocessor<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        match bits(word, 11, 8) {
            10 | 11 => self.execute_vfp(bus, word),
            15 => self.execute_cp15(bus, word),
            _ => Err(Error::Unimplemented("coprocessor instructions")),
        }
    }

    fn undefined(&self, word: u32) -> Error {
        Error::UndefinedInstruction {
            pc: self.pc(),
            word,
        }
    }

    /// MRC/MCR to CP15. Other CP15 encodings are undefined on this core.
    fn execute_cp15<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        if bits(word, 27, 24) != 0b1110 || !bit(word, 4) {
            return Err(self.undefined(word));
        }
        let opc1 = bits(word, 23, 21);
        let load = bit(word, 20);
        let crn = bits(word, 19, 16);
        let rt = bits(word, 15, 12);
        let opc2 = bits(word, 7, 5);
        let crm = bits(word, 3, 0);
        let privileged = self.cpsr.privileged();

        let fault = |fault: Cp15Fault, core: &Self| match fault {
            Cp15Fault::Undefined => core.undefined(word),
            Cp15Fault::Unsupported => Error::Unimplemented("CP15 register"),
        };

        if load {
            let value = self
                .cp15
                .read(privileged, opc1, crn, crm, opc2)
                .map_err(|f| fault(f, self))?;
            if rt == 15 {
                let (n, z, c, v) = (
                    value & (1 << 31) != 0,
                    value & (1 << 30) != 0,
                    value & (1 << 29) != 0,
                    value & (1 << 28) != 0,
                );
                self.cpsr.set_flags(n, z, c, v);
            } else {
                self.regs[rt as usize] = value;
            }
        } else {
            // Writing r15 is unpredictable; reject rather than guess.
            if rt == 15 {
                return Err(self.undefined(word));
            }
            let value = self.regs[rt as usize];
            if opc1 == 0 && crn == 7 && crm == 8 && opc2 <= 3 {
                if !privileged {
                    return Err(self.undefined(word));
                }
                let access = if opc2 == 0 || opc2 == 2 {
                    super::mmu::Access::Read
                } else {
                    super::mmu::Access::Write
                };
                self.translate_va_to_pa_register(bus, value & !0x3FF, access, opc2 < 2)?;
                return Ok(StepEvent::Executed);
            }
            self.cp15
                .write(privileged, opc1, crn, crm, opc2, value)
                .map_err(|f| fault(f, self))?;
            self.configure_exception_entry(
                self.cp15.high_vectors(),
                self.cp15.exception_big_endian(),
            );
            if opc1 == 0 && crn == 7 && crm == 0 && opc2 == 4 {
                self.waiting_for_interrupt = true;
                return Ok(StepEvent::WaitForInterrupt);
            }
        }
        Ok(StepEvent::Executed)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Arm11Core, Psr};
    use crate::memory::{Bus, FlatMemory};
    use aceris_common::Error;

    fn run(cpu: &mut Arm11Core, words: &[u32]) -> aceris_common::Result<()> {
        let mut mem = FlatMemory::new(0, 0x100);
        for (i, w) in words.iter().enumerate() {
            mem.write32(i as u32 * 4, *w).unwrap();
        }
        cpu.set_pc(0);
        for _ in words {
            cpu.step(&mut mem)?;
        }
        Ok(())
    }

    // mrc p15,0,r0,c0,c0,0 / mcr p15,0,r1,c1,c0,0 / mrc p15,0,r2,c13,c0,3
    const MRC_MIDR: u32 = 0xEE10_0F10;
    // mcr p15,0,r0,c7,c8,0 / mrc p15,0,r1,c7,c4,0
    const MCR_VA_TO_PA_PRIV_READ: u32 = 0xEE07_0F18;
    const MRC_PA_REGISTER_R1: u32 = 0xEE17_1F14;
    const MCR_SCTLR_R1: u32 = 0xEE01_1F10;
    const MRC_TLS_R2: u32 = 0xEE1D_2F70;

    #[test]
    fn va_to_pa_cp15_operations_write_and_read_par() {
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.regs[0] = 0x1234_5678;

        run(&mut cpu, &[MCR_VA_TO_PA_PRIV_READ, MRC_PA_REGISTER_R1]).unwrap();

        // MMU-disabled data translations are flat and Strongly Ordered.
        assert_eq!(cpu.regs[1], 0x1234_5100);
    }

    #[test]
    fn va_to_pa_cp15_operation_is_privileged_even_for_user_permission_checks() {
        let mut cpu = Arm11Core::new();
        cpu.regs[0] = 0x1234_5678;
        assert!(matches!(
            run(&mut cpu, &[MCR_VA_TO_PA_PRIV_READ]),
            Err(Error::UndefinedInstruction { .. })
        ));
    }

    #[test]
    fn mrc_reads_midr_in_privileged_mode() {
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        run(&mut cpu, &[MRC_MIDR]).unwrap();
        assert_eq!(cpu.regs[0], super::super::cp15::MIDR);
    }

    #[test]
    fn user_mode_cannot_read_midr_but_can_read_tls() {
        let mut cpu = Arm11Core::new();
        cpu.cp15.set_tpidruro(0x1234);
        assert!(matches!(
            run(&mut cpu, &[MRC_MIDR]),
            Err(Error::UndefinedInstruction { .. })
        ));
        run(&mut cpu, &[MRC_TLS_R2]).unwrap();
        assert_eq!(cpu.regs[2], 0x1234);
    }

    #[test]
    fn sctlr_write_configures_exception_vectors() {
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.regs[1] = 1 << 13;
        run(&mut cpu, &[MCR_SCTLR_R1]).unwrap();
        cpu.set_svc_exception_entry_enabled(true);
        run(&mut cpu, &[0xEF00_0001]).unwrap();
        assert_eq!(cpu.pc(), 0xFFFF_0008);
    }

    #[test]
    fn mrc_to_r15_updates_flags() {
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.cp15.set_tpidruro(0xA000_0000);
        // mrc p15,0,pc,c13,c0,3
        run(&mut cpu, &[0xEE1D_FF70]).unwrap();
        assert!(cpu.cpsr.0 & (1 << 31) != 0, "N set");
        assert!(cpu.cpsr.0 & (1 << 30) == 0, "Z clear");
        assert!(cpu.cpsr.0 & (1 << 29) != 0, "C set");
        assert!(cpu.cpsr.0 & (1 << 28) == 0, "V clear");
    }

    #[test]
    fn non_transfer_cp15_forms_are_undefined() {
        let mut cpu = Arm11Core::new();
        // cdp p15,0,c0,c0,c0,0
        assert!(matches!(
            run(&mut cpu, &[0xEE00_0F00]),
            Err(Error::UndefinedInstruction { .. })
        ));
    }

    #[test]
    fn other_coprocessors_remain_unimplemented() {
        let mut cpu = Arm11Core::new();
        assert!(matches!(
            run(&mut cpu, &[0xEE00_0000]),
            Err(Error::Unimplemented(_))
        ));
    }
}
