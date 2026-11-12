//! CP15 system control coprocessor state for the ARM11 core.
//!
//! Only register storage and access rules live here. The core does not
//! implement an MMU or caches: translation and cache-maintenance operations are
//! accepted (so guest kernels can run) but have no architectural effect, and
//! the stored translation-table registers are only readable back.

/// Why a CP15 access could not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cp15Fault {
    /// The access is not permitted (privilege) or the register is read-only/write-only.
    Undefined,
    /// A register this core does not model yet.
    Unsupported,
}

/// Main ID register value reported by the core (ARM11 MPCore family).
pub const MIDR: u32 = 0x410F_B024;

const SCTLR_RESET: u32 = 0x0005_0078;
/// Writable SCTLR bits: M, A, C, Z, I, V, RR and EE.
const SCTLR_WRITABLE: u32 = 0x0000_F807;
const SCTLR_V: u32 = 1 << 13;
const SCTLR_EE: u32 = 1 << 15;
const CPACR_WRITABLE: u32 = 0x00F0_0000;

/// CP15 register file of one core.
#[derive(Clone, Debug)]
pub struct Cp15 {
    core_id: u32,
    sctlr: u32,
    actlr: u32,
    cpacr: u32,
    ttbr0: u32,
    ttbr1: u32,
    ttbcr: u32,
    dacr: u32,
    dfsr: u32,
    ifsr: u32,
    dfar: u32,
    ifar: u32,
    context_id: u32,
    tpidrurw: u32,
    tpidruro: u32,
    tpidrprw: u32,
}

impl Default for Cp15 {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Cp15 {
    pub fn new(core_id: u32) -> Self {
        Self {
            core_id: core_id & 0xF,
            sctlr: SCTLR_RESET,
            actlr: 0,
            cpacr: 0,
            ttbr0: 0,
            ttbr1: 0,
            ttbcr: 0,
            dacr: 0,
            dfsr: 0,
            ifsr: 0,
            dfar: 0,
            ifar: 0,
            context_id: 0,
            tpidrurw: 0,
            tpidruro: 0,
            tpidrprw: 0,
        }
    }

    /// Raw system control register.
    pub fn sctlr(&self) -> u32 {
        self.sctlr
    }

    /// SCTLR.V: exceptions use the high vector base 0xFFFF0000.
    pub fn high_vectors(&self) -> bool {
        self.sctlr & SCTLR_V != 0
    }

    /// SCTLR.EE: exceptions are taken with big-endian data.
    pub fn exception_big_endian(&self) -> bool {
        self.sctlr & SCTLR_EE != 0
    }

    /// Whether CPACR grants full access to both cp10 and cp11 (VFP).
    pub fn vfp_access_enabled(&self) -> bool {
        (self.cpacr >> 20) & 0xF == 0xF
    }

    /// Grants full cp10/cp11 access, as a kernel does before running VFP code.
    pub fn grant_vfp_access(&mut self) {
        self.cpacr |= CPACR_WRITABLE;
    }

    pub fn tpidruro(&self) -> u32 {
        self.tpidruro
    }

    /// Sets the user read-only thread ID, as the kernel does on context switch.
    pub fn set_tpidruro(&mut self, value: u32) {
        self.tpidruro = value;
    }

    pub fn tpidrurw(&self) -> u32 {
        self.tpidrurw
    }

    pub(super) fn read(
        &self,
        privileged: bool,
        opc1: u32,
        crn: u32,
        crm: u32,
        opc2: u32,
    ) -> Result<u32, Cp15Fault> {
        if opc1 != 0 {
            return Err(Cp15Fault::Unsupported);
        }
        let user_ok = matches!((crn, crm, opc2), (13, 0, 2 | 3));
        if !privileged && !user_ok {
            return Err(Cp15Fault::Undefined);
        }
        Ok(match (crn, crm, opc2) {
            (0, 0, 0) => MIDR,
            (0, 0, 5) => 0x8000_0000 | self.core_id,
            (1, 0, 0) => self.sctlr,
            (1, 0, 1) => self.actlr,
            (1, 0, 2) => self.cpacr,
            (2, 0, 0) => self.ttbr0,
            (2, 0, 1) => self.ttbr1,
            (2, 0, 2) => self.ttbcr,
            (3, 0, 0) => self.dacr,
            (5, 0, 0) => self.dfsr,
            (5, 0, 1) => self.ifsr,
            (6, 0, 0) => self.dfar,
            (6, 0, 2) => self.ifar,
            (13, 0, 1) => self.context_id,
            (13, 0, 2) => self.tpidrurw,
            (13, 0, 3) => self.tpidruro,
            (13, 0, 4) => self.tpidrprw,
            _ => return Err(Cp15Fault::Unsupported),
        })
    }

    pub(super) fn write(
        &mut self,
        privileged: bool,
        opc1: u32,
        crn: u32,
        crm: u32,
        opc2: u32,
        value: u32,
    ) -> Result<(), Cp15Fault> {
        if opc1 != 0 {
            return Err(Cp15Fault::Unsupported);
        }
        let user_ok = matches!((crn, crm, opc2), (13, 0, 2) | (7, 10, 4 | 5));
        if !privileged && !user_ok {
            return Err(Cp15Fault::Undefined);
        }
        match (crn, crm, opc2) {
            (1, 0, 0) => {
                self.sctlr = (self.sctlr & !SCTLR_WRITABLE) | (value & SCTLR_WRITABLE);
            }
            (1, 0, 1) => self.actlr = value,
            (1, 0, 2) => self.cpacr = value & CPACR_WRITABLE,
            (2, 0, 0) => self.ttbr0 = value,
            (2, 0, 1) => self.ttbr1 = value,
            (2, 0, 2) => self.ttbcr = value & 0x7,
            (3, 0, 0) => self.dacr = value,
            (5, 0, 0) => self.dfsr = value,
            (5, 0, 1) => self.ifsr = value,
            (6, 0, 0) => self.dfar = value,
            (6, 0, 2) => self.ifar = value,
            (13, 0, 1) => self.context_id = value,
            (13, 0, 2) => self.tpidrurw = value,
            (13, 0, 4) => self.tpidrprw = value,
            // Cache maintenance, barriers and wait-for-interrupt: no modeled state.
            (7, 0, 4)
            | (7, 5, 0 | 1 | 2 | 4 | 6 | 7)
            | (7, 6, 0..=2)
            | (7, 7, 0)
            | (7, 10, 0 | 1 | 2 | 4 | 5)
            | (7, 14, 0..=2) => {}
            // TLB maintenance: nothing is cached.
            (8, 5..=7, 0..=2) => {}
            _ => return Err(Cp15Fault::Unsupported),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_state_matches_documented_profile() {
        let cp15 = Cp15::new(2);
        assert_eq!(cp15.read(true, 0, 0, 0, 0), Ok(MIDR));
        assert_eq!(cp15.read(true, 0, 0, 0, 5), Ok(0x8000_0002));
        assert!(!cp15.high_vectors());
        assert!(!cp15.exception_big_endian());
        assert!(!cp15.vfp_access_enabled());
    }

    #[test]
    fn sctlr_write_keeps_reserved_bits() {
        let mut cp15 = Cp15::default();
        cp15.write(true, 0, 1, 0, 0, 0xFFFF_FFFF).unwrap();
        assert_eq!(cp15.sctlr(), SCTLR_RESET | SCTLR_WRITABLE);
        assert!(cp15.high_vectors() && cp15.exception_big_endian());
        cp15.write(true, 0, 1, 0, 0, 0).unwrap();
        assert_eq!(cp15.sctlr(), SCTLR_RESET & !SCTLR_WRITABLE);
    }

    #[test]
    fn user_mode_reaches_only_thread_registers_and_barriers() {
        let mut cp15 = Cp15::default();
        cp15.write(false, 0, 13, 0, 2, 7).unwrap();
        cp15.set_tpidruro(9);
        assert_eq!(cp15.read(false, 0, 13, 0, 2), Ok(7));
        assert_eq!(cp15.read(false, 0, 13, 0, 3), Ok(9));
        assert_eq!(cp15.write(false, 0, 13, 0, 3, 1), Err(Cp15Fault::Undefined));
        assert_eq!(cp15.write(false, 0, 7, 10, 5, 0), Ok(()));
        assert_eq!(cp15.read(false, 0, 1, 0, 0), Err(Cp15Fault::Undefined));
        assert_eq!(cp15.write(false, 0, 1, 0, 0, 1), Err(Cp15Fault::Undefined));
    }

    #[test]
    fn privileged_registers_round_trip() {
        let mut cp15 = Cp15::default();
        cp15.write(true, 0, 2, 0, 0, 0x1234_5000).unwrap();
        cp15.write(true, 0, 3, 0, 0, 0x55).unwrap();
        cp15.write(true, 0, 1, 0, 2, 0xFFFF_FFFF).unwrap();
        assert_eq!(cp15.read(true, 0, 2, 0, 0), Ok(0x1234_5000));
        assert_eq!(cp15.read(true, 0, 3, 0, 0), Ok(0x55));
        assert!(cp15.vfp_access_enabled());
    }

    #[test]
    fn unknown_registers_are_reported_not_ignored() {
        let mut cp15 = Cp15::default();
        assert_eq!(cp15.read(true, 0, 9, 0, 0), Err(Cp15Fault::Unsupported));
        assert_eq!(cp15.write(true, 1, 1, 0, 0, 0), Err(Cp15Fault::Unsupported));
        assert_eq!(cp15.write(true, 0, 7, 9, 9, 0), Err(Cp15Fault::Unsupported));
    }
}
