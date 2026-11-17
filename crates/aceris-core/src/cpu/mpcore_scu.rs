//! ARM11 MPCore Snoop Control Unit register model.
//!
//! The original 3DS exposes the MPCore private peripheral region at
//! 0x17E00000. This module models the SCU control/configuration registers used
//! to select the AMP/SMP cacheability rules. It does not implement multiple
//! physical CPU cores, tag RAM maintenance, or cache-to-cache snooping.

use std::sync::{Arc, Mutex};

/// Physical base of the ARM11 MPCore private peripheral region on Nintendo 3DS.
pub const MPCORE_PRIVATE_MEMORY_BASE: u32 = 0x17E0_0000;
/// Size of the ARM11 MPCore private peripheral region on Nintendo 3DS.
pub const MPCORE_PRIVATE_MEMORY_SIZE: u32 = 0x0000_2000;

/// Offset of the SCU Control Register.
pub const SCU_CONTROL_REGISTER_OFFSET: u32 = 0x00;
/// Offset of the read-only SCU Configuration Register.
pub const SCU_CONFIGURATION_REGISTER_OFFSET: u32 = 0x04;
/// Offset of the SCU CPU Status Register.
pub const SCU_CPU_STATUS_REGISTER_OFFSET: u32 = 0x08;
/// Offset of the write-only SCU Invalidate All Register.
pub const SCU_INVALIDATE_ALL_REGISTER_OFFSET: u32 = 0x0C;

const SCU_CONTROL_RESET: u32 = 0x0000_1FFE;
const SCU_CONTROL_WRITABLE: u32 = 0x0000_1FFF;
const SCU_CPU_COUNT_ENCODING: u32 = 0b01;
const SCU_CPU_COUNT_MASK: u32 = 0b11;
const SCU_CPU_SMP_MASK: u32 = 0b1111;

/// Shared register state for the MPCore Snoop Control Unit.
#[derive(Clone, Debug)]
pub struct MpcoreScu {
    state: Arc<Mutex<MpcoreScuState>>,
}

#[derive(Debug)]
struct MpcoreScuState {
    control: u32,
    cpu_status: u32,
    /// One bit per CPU, representing ACTLR.SMP for that CPU.
    smp_cpus: u32,
    /// Number of guest writes to the invalidate-all command register.
    invalidate_all_requests: u64,
}

impl Default for MpcoreScuState {
    fn default() -> Self {
        Self {
            control: SCU_CONTROL_RESET,
            cpu_status: 0,
            smp_cpus: 0,
            invalidate_all_requests: 0,
        }
    }
}

impl Default for MpcoreScu {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(MpcoreScuState::default())),
        }
    }
}

impl MpcoreScu {
    /// Returns whether SCU-controlled cache coherence is enabled.
    pub fn is_enabled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .control
            & 1
            != 0
    }

    /// Returns the current SCU control register value.
    pub fn control_register(&self) -> u32 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .control
    }

    /// Returns the SCU configuration register for the two-CPU 3DS MPCore.
    ///
    /// Bits [1:0] report two CPUs, bits [7:4] report each CPU's SMP state, and
    /// the implemented 16 KiB tag RAM size fields read as zero.
    pub fn configuration_register(&self) -> u32 {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        SCU_CPU_COUNT_ENCODING | ((state.smp_cpus & SCU_CPU_SMP_MASK) << 4)
    }

    /// Updates the configuration status corresponding to one CPU's ACTLR.SMP.
    pub fn set_cpu_smp_mode(&self, core_id: u32, enabled: bool) {
        if core_id >= 4 {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bit = 1u32 << core_id;
        if enabled {
            state.smp_cpus |= bit;
        } else {
            state.smp_cpus &= !bit;
        }
    }

    /// Number of invalidate-all commands written by the guest.
    ///
    /// The register is tracked for diagnostics. The emulator does not yet have
    /// per-core L1 tag RAMs to invalidate, so this counter is not cache
    /// maintenance and does not discard or write back data-cache lines.
    pub fn invalidate_all_request_count(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate_all_requests
    }

    /// Reads one aligned, modeled SCU register.
    ///
    /// Returns None for addresses outside the register subset implemented by
    /// this model, allowing the CPU bus to report an unmapped peripheral access.
    pub fn read_register(&self, offset: u32) -> Option<u32> {
        if offset & 3 != 0 {
            return None;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match offset {
            SCU_CONTROL_REGISTER_OFFSET => Some(state.control),
            SCU_CONFIGURATION_REGISTER_OFFSET => {
                Some(SCU_CPU_COUNT_ENCODING | ((state.smp_cpus & SCU_CPU_SMP_MASK) << 4))
            }
            SCU_CPU_STATUS_REGISTER_OFFSET => Some(state.cpu_status),
            // The Invalidate All register is write-only and therefore RAZ here.
            SCU_INVALIDATE_ALL_REGISTER_OFFSET => Some(0),
            _ => None,
        }
    }

    /// Writes one aligned, modeled SCU register.
    ///
    /// Configuration writes are ignored because the register is read-only.
    /// CPU status is retained as a limited register value; it does not power
    /// down or start an independently scheduled CPU.
    pub fn write_register(&self, offset: u32, value: u32) -> bool {
        if offset & 3 != 0 {
            return false;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match offset {
            SCU_CONTROL_REGISTER_OFFSET => {
                state.control =
                    (state.control & !SCU_CONTROL_WRITABLE) | (value & SCU_CONTROL_WRITABLE);
                true
            }
            SCU_CONFIGURATION_REGISTER_OFFSET => true,
            SCU_CPU_STATUS_REGISTER_OFFSET => {
                state.cpu_status = value & SCU_CPU_COUNT_MASK;
                true
            }
            SCU_INVALIDATE_ALL_REGISTER_OFFSET => {
                state.invalidate_all_requests = state.invalidate_all_requests.wrapping_add(1);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_values_match_two_cpu_arm11_mpcore_profile() {
        let scu = MpcoreScu::default();
        assert_eq!(scu.control_register(), 0x0000_1FFE);
        assert!(!scu.is_enabled());
        assert_eq!(scu.configuration_register(), 0b01);
    }

    #[test]
    fn control_register_enable_bit_controls_scu_coherence_gate() {
        let scu = MpcoreScu::default();
        assert!(scu.write_register(SCU_CONTROL_REGISTER_OFFSET, 0xFFFF_FFFF));
        assert!(scu.is_enabled());
        assert_eq!(scu.control_register(), 0x0000_1FFF);

        assert!(scu.write_register(SCU_CONTROL_REGISTER_OFFSET, 0));
        assert!(!scu.is_enabled());
        assert_eq!(scu.control_register(), 0);
    }

    #[test]
    fn configuration_reports_per_cpu_smp_mode_and_is_read_only() {
        let scu = MpcoreScu::default();
        scu.set_cpu_smp_mode(0, true);
        scu.set_cpu_smp_mode(1, true);
        assert_eq!(scu.configuration_register(), 0b11_0001);

        assert!(scu.write_register(SCU_CONFIGURATION_REGISTER_OFFSET, 0));
        assert_eq!(scu.configuration_register(), 0b11_0001);

        scu.set_cpu_smp_mode(0, false);
        assert_eq!(scu.configuration_register(), 0b10_0001);
    }

    #[test]
    fn invalidate_all_is_write_only_and_does_not_claim_tag_ram_maintenance() {
        let scu = MpcoreScu::default();
        assert_eq!(
            scu.read_register(SCU_INVALIDATE_ALL_REGISTER_OFFSET),
            Some(0)
        );
        assert!(scu.write_register(SCU_INVALIDATE_ALL_REGISTER_OFFSET, u32::MAX));
        assert_eq!(scu.invalidate_all_request_count(), 1);
    }

    #[test]
    fn unsupported_or_unaligned_registers_are_rejected() {
        let scu = MpcoreScu::default();
        assert_eq!(scu.read_register(0x10), None);
        assert_eq!(scu.read_register(1), None);
        assert!(!scu.write_register(0x10, 0));
        assert!(!scu.write_register(1, 0));
    }
}
