//! CP15 system control coprocessor state for the ARM11 core.
//!
//! CP15 owns system-control state and queues TLB-maintenance operations for the
//! ARMv6 MMU. Cache maintenance and some barriers still have no modeled memory
//! effect and remain explicit limitations.

/// TLB maintenance requested by a privileged MCR to CP15 c8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TlbMaintenance {
    All,
    MvaAsid { mva: u32, asid: u8 },
    Asid { asid: u8 },
    MvaAllAsids { mva: u32 },
}

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
/// MP11 cache type: separate 16 KiB, four-way, 8-word-line I/D caches.
pub const CTR_MP11: u32 = 0x1D15_2152;
/// MP11 unified TLB type: zero instruction-lockable and eight data/unified lockable entries.
pub const TLBTR_MP11: u32 = 0x0000_0800;

const SCTLR_RESET: u32 = 0x0005_0078;
/// Writable SCTLR bits: M, A, C, Z, I, V, RR, EE and XP.
const SCTLR_WRITABLE: u32 = 0x0080_F807;
const SCTLR_M: u32 = 1;
const SCTLR_XP: u32 = 1 << 23;
const FSR_WRITE_NOT_READ: u32 = 1 << 11;
const SCTLR_V: u32 = 1 << 13;
const SCTLR_EE: u32 = 1 << 15;
const CPACR_WRITABLE: u32 = 0x00F0_0000;
const TLB_LOCKDOWN_WRITABLE: u32 = 0x9C00_0000;

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
    pending_tlb_maintenance: Option<TlbMaintenance>,
    tlb_lockdown: u32,
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
            pending_tlb_maintenance: None,
            tlb_lockdown: 0,
        }
    }

    /// Raw system control register.
    pub fn sctlr(&self) -> u32 {
        self.sctlr
    }

    /// SCTLR.M: address translation is enabled.
    pub fn mmu_enabled(&self) -> bool {
        self.sctlr & SCTLR_M != 0
    }

    /// SCTLR.XP: ARMv6 page-table format (AP[1:0] plus APX/XN, no subpages).
    pub fn extended_page_tables(&self) -> bool {
        self.sctlr & SCTLR_XP != 0
    }

    pub fn ttbr0(&self) -> u32 {
        self.ttbr0
    }

    pub fn ttbr1(&self) -> u32 {
        self.ttbr1
    }

    /// TTBCR.N: the number of top address bits routed to TTBR0 (0-7).
    pub fn ttbcr_n(&self) -> u32 {
        self.ttbcr & 7
    }

    pub fn dacr(&self) -> u32 {
        self.dacr
    }

    /// ASID from CONTEXTIDR[7:0], used for non-global TLB entries.
    pub(super) fn asid(&self) -> u8 {
        self.context_id as u8
    }

    /// CP15 c10, c0, 0 controls the TLB's eight lockdown slots.
    pub(super) fn tlb_preserve(&self) -> bool {
        self.tlb_lockdown & (1 << 31) != 0
    }

    pub(super) fn tlb_victim(&self) -> usize {
        ((self.tlb_lockdown >> 26) & 7) as usize
    }

    /// Takes the pending maintenance command after the issuing instruction.
    pub(super) fn take_tlb_maintenance(&mut self) -> Option<TlbMaintenance> {
        self.pending_tlb_maintenance.take()
    }

    pub fn dfsr(&self) -> u32 {
        self.dfsr
    }

    pub fn dfar(&self) -> u32 {
        self.dfar
    }

    pub fn ifsr(&self) -> u32 {
        self.ifsr
    }

    pub fn ifar(&self) -> u32 {
        self.ifar
    }

    /// Records a data-abort cause the way the hardware latches DFSR/DFAR.
    pub(super) fn latch_data_fault(&mut self, status: u32, address: u32, write: bool) {
        self.dfsr = status | if write { FSR_WRITE_NOT_READ } else { 0 };
        self.dfar = address;
    }

    /// Records a prefetch-abort cause (IFSR/IFAR).
    pub(super) fn latch_prefetch_fault(&mut self, status: u32, address: u32) {
        self.ifsr = status;
        self.ifar = address;
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
            (0, 0, 1) => CTR_MP11,
            (0, 0, 3) => TLBTR_MP11,
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
            (10, 0, 0) => self.tlb_lockdown,
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
            (10, 0, 0) => self.tlb_lockdown = value & TLB_LOCKDOWN_WRITABLE,
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
            // ARM11 MPCore has one unified TLB. c5 (instruction), c6
            // (data), and c7 (unified) therefore affect the same entries.
            (8, 5..=7, 0) => {
                self.pending_tlb_maintenance = Some(TlbMaintenance::All);
            }
            (8, 5..=7, 1) => {
                self.pending_tlb_maintenance = Some(TlbMaintenance::MvaAsid {
                    mva: value & 0xFFFF_F000,
                    asid: value as u8,
                });
            }
            (8, 5..=7, 2) => {
                self.pending_tlb_maintenance = Some(TlbMaintenance::Asid { asid: value as u8 });
            }
            (8, 5..=7, 3) => {
                self.pending_tlb_maintenance = Some(TlbMaintenance::MvaAllAsids {
                    mva: value & 0xFFFF_F000,
                });
            }
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
    fn mp11_cache_and_tlb_type_registers_report_the_documented_configuration() {
        let cp15 = Cp15::default();
        assert_eq!(cp15.read(true, 0, 0, 0, 0), Ok(MIDR));
        assert_eq!(cp15.read(true, 0, 0, 0, 1), Ok(CTR_MP11));
        assert_eq!(cp15.read(true, 0, 0, 0, 3), Ok(TLBTR_MP11));

        assert_eq!(cp15.read(false, 0, 0, 0, 1), Err(Cp15Fault::Undefined));
        assert_eq!(cp15.read(false, 0, 0, 0, 3), Err(Cp15Fault::Undefined));
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
    fn tlb_maintenance_operations_decode_all_arm11_forms() {
        let mut cp15 = Cp15::default();

        cp15.write(true, 0, 8, 5, 0, 0).unwrap();
        assert_eq!(cp15.take_tlb_maintenance(), Some(TlbMaintenance::All));

        cp15.write(true, 0, 8, 6, 1, 0x1234_567A).unwrap();
        assert_eq!(
            cp15.take_tlb_maintenance(),
            Some(TlbMaintenance::MvaAsid {
                mva: 0x1234_5000,
                asid: 0x7A,
            })
        );

        cp15.write(true, 0, 8, 7, 2, 0x0000_003C).unwrap();
        assert_eq!(
            cp15.take_tlb_maintenance(),
            Some(TlbMaintenance::Asid { asid: 0x3C })
        );

        cp15.write(true, 0, 8, 7, 3, 0x8765_43FE).unwrap();
        assert_eq!(
            cp15.take_tlb_maintenance(),
            Some(TlbMaintenance::MvaAllAsids { mva: 0x8765_4000 })
        );
    }

    #[test]
    fn tlb_maintenance_is_privileged_and_reserved_encodings_are_not_ignored() {
        let mut cp15 = Cp15::default();
        assert_eq!(cp15.write(false, 0, 8, 7, 0, 0), Err(Cp15Fault::Undefined));
        assert_eq!(cp15.write(true, 0, 8, 4, 0, 0), Err(Cp15Fault::Unsupported));
        assert_eq!(cp15.write(true, 0, 8, 7, 4, 0), Err(Cp15Fault::Unsupported));
    }

    #[test]
    fn tlb_lockdown_register_preserves_only_documented_fields() {
        let mut cp15 = Cp15::default();
        // P is bit 31; Victim occupies bits 28:26. The rest is SBZ/UNP.
        let value = (1 << 31) | (5 << 26) | 0x0000_001F;
        cp15.write(true, 0, 10, 0, 0, value).unwrap();
        assert_eq!(
            cp15.read(true, 0, 10, 0, 0),
            Ok((1 << 31) | (5 << 26))
        );
        assert!(cp15.tlb_preserve());
        assert_eq!(cp15.tlb_victim(), 5);

        // With P clear, Victim is still readable but page walks use the
        // two-way set-associative region. Bit 0 must not enable preservation.
        cp15.write(true, 0, 10, 0, 0, (6 << 26) | 1).unwrap();
        assert!(!cp15.tlb_preserve());
        assert_eq!(cp15.tlb_victim(), 6);
        assert_eq!(cp15.write(false, 0, 10, 0, 0, 0), Err(Cp15Fault::Undefined));
        assert_eq!(cp15.read(false, 0, 10, 0, 0), Err(Cp15Fault::Undefined));
    }

    #[test]
    fn unknown_registers_are_reported_not_ignored() {
        let mut cp15 = Cp15::default();
        assert_eq!(cp15.read(true, 0, 9, 0, 0), Err(Cp15Fault::Unsupported));
        assert_eq!(cp15.write(true, 1, 1, 0, 0, 0), Err(Cp15Fault::Unsupported));
        assert_eq!(cp15.write(true, 0, 7, 9, 9, 0), Err(Cp15Fault::Unsupported));
    }
}
