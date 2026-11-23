//! ARM11 MPCore Generic Interrupt Controller for the single emulated application CPU.
//!
//! The model exposes the 128 interrupt IDs used by the original 3DS private
//! peripheral region. It implements the distributor's enable/pending/active,
//! priority, target and trigger-configuration registers, plus the CPU-interface
//! acknowledge/EOI path. Only CPU0 is currently scheduled by Aceris; secondary
//! CPU interfaces and inter-processor delivery are intentionally not claimed.

/// Offset of the current-CPU interface within the MPCore private region.
pub const MPCORE_CPU_INTERFACE_CURRENT_OFFSET: u32 = 0x0100;
/// Offset of the CPU0 interface alias within the MPCore private region.
pub const MPCORE_CPU_INTERFACE_CPU0_OFFSET: u32 = 0x0200;
/// Offset of the global interrupt distributor within the MPCore private region.
pub const MPCORE_GIC_DISTRIBUTOR_OFFSET: u32 = 0x1000;
/// Size of the distributor register aperture.
pub const MPCORE_GIC_DISTRIBUTOR_SIZE: u32 = 0x1000;
/// Size of the private CPU-interface register aperture.
pub const MPCORE_GIC_CPU_INTERFACE_SIZE: u32 = 0x0100;
/// Number of interrupt IDs represented by the original 3DS peripheral block.
pub const MPCORE_GIC_INTERRUPT_COUNT: usize = 128;
/// Private timer interrupt ID (PPI 29).
pub const MPCORE_PRIVATE_TIMER_INTERRUPT_ID: u32 = 29;
/// Spurious interrupt identifier returned by the CPU interface.
pub const MPCORE_GIC_SPURIOUS_INTERRUPT_ID: u32 = 0x03FF;

const GICD_CTLR: u32 = 0x000;
const GICD_TYPER: u32 = 0x004;
const GICD_ISENABLER_BASE: u32 = 0x100;
const GICD_ICENABLER_BASE: u32 = 0x180;
const GICD_ISPENDR_BASE: u32 = 0x200;
const GICD_ICPENDR_BASE: u32 = 0x280;
const GICD_ISACTIVER_BASE: u32 = 0x300;
const GICD_ICACTIVER_BASE: u32 = 0x380;
const GICD_IPRIORITYR_BASE: u32 = 0x400;
const GICD_ITARGETSR_BASE: u32 = 0x800;
const GICD_ICFGR_BASE: u32 = 0xC00;
const GICD_PIDR2: u32 = 0xFE8;

const GICC_CTLR: u32 = 0x00;
const GICC_PMR: u32 = 0x04;
const GICC_BPR: u32 = 0x08;
const GICC_IAR: u32 = 0x0C;
const GICC_EOIR: u32 = 0x10;
const GICC_RPR: u32 = 0x14;
const GICC_HPPIR: u32 = 0x18;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MpcoreInterruptController {
    distributor_enabled: bool,
    cpu_interface_enabled: bool,
    priority_mask: u8,
    binary_point: u8,
    enabled: [bool; MPCORE_GIC_INTERRUPT_COUNT],
    pending: [bool; MPCORE_GIC_INTERRUPT_COUNT],
    active: [bool; MPCORE_GIC_INTERRUPT_COUNT],
    priorities: [u8; MPCORE_GIC_INTERRUPT_COUNT],
    targets: [u8; MPCORE_GIC_INTERRUPT_COUNT],
    edge_triggered: [bool; MPCORE_GIC_INTERRUPT_COUNT],
    private_timer_input: bool,
    active_stack: Vec<u32>,
}

impl Default for MpcoreInterruptController {
    fn default() -> Self {
        let mut enabled = [false; MPCORE_GIC_INTERRUPT_COUNT];
        // SGIs are always enabled on the ARM GIC and cannot be disabled
        // through GICD_ICENABLER0.
        enabled[..16].fill(true);

        let mut targets = [0u8; MPCORE_GIC_INTERRUPT_COUNT];
        // PPIs are private to the current CPU. In this one-core model the
        // readable target is CPU0, and writes to PPI target fields are ignored.
        targets[16..32].fill(1);

        let mut edge_triggered = [false; MPCORE_GIC_INTERRUPT_COUNT];
        edge_triggered[..16].fill(true);
        edge_triggered[29] = true;
        edge_triggered[30] = true;

        Self {
            distributor_enabled: false,
            cpu_interface_enabled: false,
            priority_mask: 0xF0,
            binary_point: 3,
            enabled,
            pending: [false; MPCORE_GIC_INTERRUPT_COUNT],
            active: [false; MPCORE_GIC_INTERRUPT_COUNT],
            priorities: [0; MPCORE_GIC_INTERRUPT_COUNT],
            targets,
            edge_triggered,
            private_timer_input: false,
            active_stack: Vec::new(),
        }
    }
}

impl MpcoreInterruptController {
    /// Returns whether an address is in the CPU0 private-interface window.
    pub const fn is_cpu_interface_offset(offset: u32) -> bool {
        (offset >= MPCORE_CPU_INTERFACE_CURRENT_OFFSET
            && offset < MPCORE_CPU_INTERFACE_CURRENT_OFFSET + MPCORE_GIC_CPU_INTERFACE_SIZE)
            || (offset >= MPCORE_CPU_INTERFACE_CPU0_OFFSET
                && offset < MPCORE_CPU_INTERFACE_CPU0_OFFSET + MPCORE_GIC_CPU_INTERFACE_SIZE)
    }

    /// Returns whether the distributor currently asserts the CPU0 IRQ output.
    pub fn irq_asserted(&self) -> bool {
        self.distributor_enabled
            && self.cpu_interface_enabled
            && self.highest_pending_interrupt().is_some()
    }

    /// Samples the private timer's interrupt output on PPI 29.
    ///
    /// The default timer trigger type is edge-sensitive. Changing the trigger
    /// configuration to level-sensitive causes the pending state to follow
    /// the asserted timer output until the guest clears it.
    pub fn set_private_timer_irq_level(&mut self, asserted: bool) {
        let interrupt_id = MPCORE_PRIVATE_TIMER_INTERRUPT_ID as usize;
        if self.edge_triggered[interrupt_id] {
            if asserted && !self.private_timer_input {
                self.pending[interrupt_id] = true;
            }
        } else {
            self.pending[interrupt_id] = asserted && !self.active[interrupt_id];
        }
        self.private_timer_input = asserted;
    }

    /// Reads one byte from the memory-mapped distributor register window.
    ///
    /// The distributor supports byte accesses. Unknown/reserved offsets read
    /// as zero, while the caller is responsible for range-checking the window.
    pub fn read_distributor_byte(&self, offset: u32) -> u8 {
        let register = offset & !3;
        let shift = (offset & 3) * 8;
        (self.read_distributor_word(register) >> shift) as u8
    }

    /// Writes one byte to the memory-mapped distributor register window.
    ///
    /// Set/clear registers use write-one semantics. Read-only and reserved
    /// fields ignore writes.
    pub fn write_distributor_byte(&mut self, offset: u32, value: u8) {
        if in_byte_register_range(offset, GICD_IPRIORITYR_BASE, 128) {
            let interrupt_id = (offset - GICD_IPRIORITYR_BASE) as usize;
            self.priorities[interrupt_id] = value >> 4;
            return;
        }
        if in_byte_register_range(offset, GICD_ITARGETSR_BASE, 128) {
            let interrupt_id = (offset - GICD_ITARGETSR_BASE) as usize;
            // The target mask is hardwired for SGIs and PPIs. PPI target
            // fields resolve to the current CPU (CPU0 in this implementation).
            if interrupt_id >= 32 {
                self.targets[interrupt_id] = value & 0x0F;
            }
            return;
        }
        if in_byte_register_range(offset, GICD_ICFGR_BASE, 32) {
            let byte_index = (offset - GICD_ICFGR_BASE) as usize;
            let first_interrupt = (byte_index / 4) * 16 + (byte_index % 4) * 4;
            for field in 0..4 {
                let interrupt_id = first_interrupt + field;
                if interrupt_id < 16 {
                    continue;
                }
                self.edge_triggered[interrupt_id] = value & (1 << (field * 2 + 1)) != 0;
                self.refresh_level_pending(interrupt_id);
            }
            return;
        }

        let register = offset & !3;
        let shift = (offset & 3) * 8;
        // GICD_CTLR only implements bit 0 in byte lane 0. Writes to the
        // remaining byte lanes must not clear the enable bit.
        if register == GICD_CTLR && shift != 0 {
            return;
        }
        self.write_distributor_word(register, u32::from(value) << shift);
    }

    /// Reads an aligned CPU-interface register. IAR acknowledges the highest
    /// eligible interrupt and moves it from pending to active.
    pub fn read_cpu_interface_register(&mut self, offset: u32) -> Option<u32> {
        if offset & 3 != 0 {
            return None;
        }
        match offset {
            GICC_CTLR => Some(u32::from(self.cpu_interface_enabled)),
            GICC_PMR => Some(u32::from(self.priority_mask)),
            GICC_BPR => Some(u32::from(self.binary_point)),
            GICC_IAR => Some(self.acknowledge_interrupt()),
            GICC_EOIR => Some(0),
            GICC_RPR => Some(u32::from(self.running_priority())),
            GICC_HPPIR => Some(
                self.highest_pending_interrupt()
                    .unwrap_or(MPCORE_GIC_SPURIOUS_INTERRUPT_ID),
            ),
            _ => Some(0),
        }
    }

    /// Writes an aligned CPU-interface register.
    pub fn write_cpu_interface_register(&mut self, offset: u32, value: u32) -> bool {
        if offset & 3 != 0 {
            return false;
        }
        match offset {
            GICC_CTLR => self.cpu_interface_enabled = value & 1 != 0,
            GICC_PMR => self.priority_mask = (value as u8) & 0xF0,
            GICC_BPR => self.binary_point = (value as u8) & 0x07,
            GICC_EOIR => self.end_interrupt(value & 0x03FF),
            _ => {}
        }
        true
    }

    /// Returns whether one interrupt ID is enabled.
    pub fn interrupt_enabled(&self, interrupt_id: u32) -> Option<bool> {
        self.enabled
            .get(usize::try_from(interrupt_id).ok()?)
            .copied()
    }

    /// Returns whether one interrupt ID is pending.
    pub fn interrupt_pending(&self, interrupt_id: u32) -> Option<bool> {
        self.pending
            .get(usize::try_from(interrupt_id).ok()?)
            .copied()
    }

    /// Returns whether one interrupt ID is active.
    pub fn interrupt_active(&self, interrupt_id: u32) -> Option<bool> {
        self.active
            .get(usize::try_from(interrupt_id).ok()?)
            .copied()
    }

    /// Returns the programmed priority nibble for one interrupt ID.
    pub fn interrupt_priority(&self, interrupt_id: u32) -> Option<u8> {
        self.priorities
            .get(usize::try_from(interrupt_id).ok()?)
            .map(|priority| priority << 4)
    }

    fn read_distributor_word(&self, offset: u32) -> u32 {
        match offset {
            GICD_CTLR => u32::from(self.distributor_enabled),
            // ITLinesNumber=3 advertises 128 IDs; CPUNumber=1 advertises
            // two MPCore CPU interfaces, as configured on the original 3DS.
            // Aceris currently schedules CPU0 only, but the hardware profile
            // reported to the guest must still describe the two-core cluster.
            GICD_TYPER => 3 | (1 << 5),
            GICD_PIDR2 => 0x04,
            0xFE0 => 0x90,
            0xFE4 => 0x13,
            0xFEC => 0x00,
            0xFF0 => 0x0D,
            0xFF4 => 0xF0,
            0xFF8 => 0x05,
            0xFFC => 0xB1,
            _ if is_bit_register(offset, GICD_ISENABLER_BASE) => {
                self.pack_bits(offset, GICD_ISENABLER_BASE, &self.enabled)
            }
            _ if is_bit_register(offset, GICD_ICENABLER_BASE) => {
                self.pack_bits(offset, GICD_ICENABLER_BASE, &self.enabled)
            }
            _ if is_bit_register(offset, GICD_ISPENDR_BASE) => {
                self.pack_bits(offset, GICD_ISPENDR_BASE, &self.pending)
            }
            _ if is_bit_register(offset, GICD_ICPENDR_BASE) => {
                self.pack_bits(offset, GICD_ICPENDR_BASE, &self.pending)
            }
            _ if is_bit_register(offset, GICD_ISACTIVER_BASE) => {
                self.pack_bits(offset, GICD_ISACTIVER_BASE, &self.active)
            }
            _ if is_bit_register(offset, GICD_ICACTIVER_BASE) => {
                self.pack_bits(offset, GICD_ICACTIVER_BASE, &self.active)
            }
            _ if in_register_range(offset, GICD_IPRIORITYR_BASE, 128) => {
                self.pack_nibbles(offset, GICD_IPRIORITYR_BASE, &self.priorities)
            }
            _ if in_register_range(offset, GICD_ITARGETSR_BASE, 128) => {
                self.pack_bytes(offset, GICD_ITARGETSR_BASE, &self.targets)
            }
            _ if in_register_range(offset, GICD_ICFGR_BASE, 8) => self.pack_trigger_config(offset),
            _ => 0,
        }
    }

    fn write_distributor_word(&mut self, offset: u32, value: u32) {
        match offset {
            GICD_CTLR => self.distributor_enabled = value & 1 != 0,
            _ if is_bit_register(offset, GICD_ISENABLER_BASE) => {
                self.apply_enable_bits(offset, GICD_ISENABLER_BASE, value, true);
            }
            _ if is_bit_register(offset, GICD_ICENABLER_BASE) => {
                self.apply_enable_bits(offset, GICD_ICENABLER_BASE, value, false);
            }
            _ if is_bit_register(offset, GICD_ISPENDR_BASE) => {
                self.apply_pending_bits(offset, GICD_ISPENDR_BASE, value, true);
            }
            _ if is_bit_register(offset, GICD_ICPENDR_BASE) => {
                self.apply_pending_bits(offset, GICD_ICPENDR_BASE, value, false);
            }
            _ if is_bit_register(offset, GICD_ISACTIVER_BASE) => {
                self.apply_active_bits(offset, GICD_ISACTIVER_BASE, value, true);
            }
            _ if is_bit_register(offset, GICD_ICACTIVER_BASE) => {
                self.apply_active_bits(offset, GICD_ICACTIVER_BASE, value, false);
            }
            _ => {}
        }
    }

    fn pack_bits(
        &self,
        offset: u32,
        base: u32,
        values: &[bool; MPCORE_GIC_INTERRUPT_COUNT],
    ) -> u32 {
        let first = ((offset - base) / 4) as usize * 32;
        let mut result = 0u32;
        for bit in 0..32 {
            if values[first + bit] {
                result |= 1 << bit;
            }
        }
        result
    }

    fn apply_enable_bits(&mut self, offset: u32, base: u32, value: u32, enabled: bool) {
        let first = ((offset - base) / 4) as usize * 32;
        for bit in 0..32 {
            if value & (1 << bit) == 0 {
                continue;
            }
            let interrupt_id = first + bit;
            if interrupt_id < 16 {
                self.enabled[interrupt_id] = true;
            } else {
                self.enabled[interrupt_id] = enabled;
            }
        }
    }

    fn apply_pending_bits(&mut self, offset: u32, base: u32, value: u32, pending: bool) {
        let first = ((offset - base) / 4) as usize * 32;
        for bit in 0..32 {
            if value & (1 << bit) == 0 {
                continue;
            }
            let interrupt_id = first + bit;
            self.pending[interrupt_id] = pending;
            if !pending {
                self.refresh_level_pending(interrupt_id);
            }
        }
    }

    fn apply_active_bits(&mut self, offset: u32, base: u32, value: u32, active: bool) {
        let first = ((offset - base) / 4) as usize * 32;
        for bit in 0..32 {
            if value & (1 << bit) == 0 {
                continue;
            }
            let interrupt_id = first + bit;
            if active {
                if !self.active[interrupt_id] {
                    self.active[interrupt_id] = true;
                    self.active_stack.push(interrupt_id as u32);
                    self.refresh_level_pending(interrupt_id);
                }
            } else {
                self.end_interrupt(interrupt_id as u32);
            }
        }
    }

    fn pack_nibbles(
        &self,
        offset: u32,
        base: u32,
        values: &[u8; MPCORE_GIC_INTERRUPT_COUNT],
    ) -> u32 {
        let first = (offset - base) as usize;
        let mut result = 0u32;
        for byte in 0..4 {
            result |= u32::from(values[first + byte] << 4) << (byte * 8);
        }
        result
    }

    fn pack_bytes(&self, offset: u32, base: u32, values: &[u8; MPCORE_GIC_INTERRUPT_COUNT]) -> u32 {
        let first = (offset - base) as usize;
        let mut result = 0u32;
        for byte in 0..4 {
            result |= u32::from(values[first + byte]) << (byte * 8);
        }
        result
    }

    fn pack_trigger_config(&self, offset: u32) -> u32 {
        let first = ((offset - GICD_ICFGR_BASE) / 4) as usize * 16;
        let mut result = 0u32;
        for index in 0..16 {
            let trigger = u32::from(self.edge_triggered[first + index]) << 1;
            result |= trigger << (index * 2);
        }
        result
    }

    fn highest_pending_interrupt(&self) -> Option<u32> {
        if !self.distributor_enabled || !self.cpu_interface_enabled {
            return None;
        }
        let running_group = self
            .active_stack
            .iter()
            .filter_map(|interrupt_id| self.priorities.get(*interrupt_id as usize).copied())
            .map(|priority| self.priority_group(priority))
            .min();

        (0..MPCORE_GIC_INTERRUPT_COUNT)
            .filter(|interrupt_id| {
                let priority = self.priorities[*interrupt_id];
                let group = self.priority_group(priority);
                self.pending[*interrupt_id]
                    && self.enabled[*interrupt_id]
                    && !self.active[*interrupt_id]
                    && self.priority_is_unmasked(priority)
                    && self.targets_cpu0(*interrupt_id)
                    && running_group.map_or(true, |active_group| group < active_group)
            })
            .min_by_key(|interrupt_id| (self.priorities[*interrupt_id], *interrupt_id))
            .map(|interrupt_id| interrupt_id as u32)
    }

    fn priority_is_unmasked(&self, priority: u8) -> bool {
        (priority << 4) < self.priority_mask
    }

    fn priority_group(&self, priority: u8) -> u8 {
        // Aceris models the four implemented priority bits in [7:4] of
        // the architectural 8-bit priority field. BPR values 0..=3 keep
        // all four bits in the group field; values 4..=7 progressively
        // move the low implemented bits into subpriority.
        priority >> self.binary_point.saturating_sub(3)
    }

    fn targets_cpu0(&self, interrupt_id: usize) -> bool {
        if interrupt_id < 32 {
            true
        } else {
            self.targets[interrupt_id] & 1 != 0
        }
    }

    fn acknowledge_interrupt(&mut self) -> u32 {
        let Some(interrupt_id) = self.highest_pending_interrupt() else {
            return MPCORE_GIC_SPURIOUS_INTERRUPT_ID;
        };
        let index = interrupt_id as usize;
        self.pending[index] = false;
        self.active[index] = true;
        self.active_stack.push(interrupt_id);
        if !self.edge_triggered[index] {
            self.refresh_level_pending(index);
        }
        interrupt_id
    }

    fn end_interrupt(&mut self, interrupt_id: u32) {
        let Ok(index) = usize::try_from(interrupt_id) else {
            return;
        };
        if index >= MPCORE_GIC_INTERRUPT_COUNT || !self.active[index] {
            return;
        }
        self.active[index] = false;
        if let Some(stack_index) = self
            .active_stack
            .iter()
            .rposition(|active| *active == interrupt_id)
        {
            self.active_stack.remove(stack_index);
        }
        self.refresh_level_pending(index);
    }

    fn running_priority(&self) -> u32 {
        self.active_stack
            .iter()
            .filter_map(|interrupt_id| self.priorities.get(*interrupt_id as usize).copied())
            .min()
            .map(|priority| u32::from(priority << 4))
            .unwrap_or(0xF0)
    }

    fn refresh_level_pending(&mut self, interrupt_id: usize) {
        if interrupt_id == MPCORE_PRIVATE_TIMER_INTERRUPT_ID as usize
            && !self.edge_triggered[interrupt_id]
        {
            self.pending[interrupt_id] = self.private_timer_input && !self.active[interrupt_id];
        }
    }
}

const fn is_bit_register(offset: u32, base: u32) -> bool {
    offset >= base && offset < base + 0x10 && offset & 3 == 0
}

const fn in_register_range(offset: u32, base: u32, count: u32) -> bool {
    offset >= base && offset < base + count * 4 && offset & 3 == 0
}

const fn in_byte_register_range(offset: u32, base: u32, count: u32) -> bool {
    offset >= base && offset < base + count
}

#[cfg(test)]
mod tests {
    use super::{
        GICC_BPR, GICC_CTLR, GICC_EOIR, GICC_HPPIR, GICC_IAR, GICC_PMR, GICC_RPR,
        GICD_CTLR, GICD_ICACTIVER_BASE, GICD_ICENABLER_BASE, GICD_ICFGR_BASE,
        GICD_IPRIORITYR_BASE, GICD_ISACTIVER_BASE, GICD_ISENABLER_BASE,
        GICD_ISPENDR_BASE, MPCORE_GIC_SPURIOUS_INTERRUPT_ID,
        MPCORE_PRIVATE_TIMER_INTERRUPT_ID, MpcoreInterruptController,
    };

    const TIMER_BIT: u32 = 1 << MPCORE_PRIVATE_TIMER_INTERRUPT_ID;

    fn enable_timer_irq(controller: &mut MpcoreInterruptController) {
        controller.write_distributor_byte(GICD_CTLR, 1);
        controller.write_distributor_byte(GICD_ISENABLER_BASE + 3, (TIMER_BIT >> 24) as u8);
        controller.write_cpu_interface_register(GICC_CTLR, 1);
    }

    #[test]
    fn distributor_type_reports_two_cpus_and_128_interrupt_ids() {
        let controller = MpcoreInterruptController::default();
        // ITLinesNumber=3 means 128 implemented interrupt IDs.
        // CPUNumber=1 means two CPU interfaces are implemented.
        assert_eq!(controller.read_distributor_byte(0x004), 0x23);
    }

    #[test]
    fn private_timer_pending_is_gated_by_both_distributor_and_cpu_interface() {
        let mut controller = MpcoreInterruptController::default();
        controller.set_private_timer_irq_level(true);

        assert!(!controller.irq_asserted());
        assert_eq!(
            controller.read_cpu_interface_register(GICC_HPPIR),
            Some(MPCORE_GIC_SPURIOUS_INTERRUPT_ID)
        );

        controller.write_distributor_byte(GICD_CTLR, 1);
        assert!(!controller.irq_asserted());
        controller.write_distributor_byte(GICD_ISENABLER_BASE + 3, (TIMER_BIT >> 24) as u8);
        assert!(!controller.irq_asserted());
        controller.write_cpu_interface_register(GICC_CTLR, 1);

        assert!(controller.irq_asserted());
        assert_eq!(
            controller.read_cpu_interface_register(GICC_HPPIR),
            Some(MPCORE_PRIVATE_TIMER_INTERRUPT_ID)
        );
    }

    #[test]
    fn acknowledge_moves_pending_to_active_and_eoi_releases_the_interrupt() {
        let mut controller = MpcoreInterruptController::default();
        enable_timer_irq(&mut controller);
        controller.set_private_timer_irq_level(true);

        assert_eq!(
            controller.read_cpu_interface_register(GICC_IAR),
            Some(MPCORE_PRIVATE_TIMER_INTERRUPT_ID)
        );
        assert_eq!(controller.interrupt_pending(29), Some(false));
        assert_eq!(controller.interrupt_active(29), Some(true));
        assert_eq!(controller.read_cpu_interface_register(GICC_RPR), Some(0));
        assert!(!controller.irq_asserted());

        assert!(controller.write_cpu_interface_register(GICC_EOIR, 29));
        assert_eq!(controller.interrupt_active(29), Some(false));
        assert!(!controller.irq_asserted());
    }

    #[test]
    fn edge_pending_is_latched_until_acknowledged_and_requires_a_new_edge() {
        let mut controller = MpcoreInterruptController::default();
        enable_timer_irq(&mut controller);
        controller.set_private_timer_irq_level(true);
        assert!(controller.interrupt_pending(29).unwrap());

        assert_eq!(
            controller.read_cpu_interface_register(GICC_IAR),
            Some(MPCORE_PRIVATE_TIMER_INTERRUPT_ID)
        );
        controller.write_cpu_interface_register(GICC_EOIR, 29);
        assert!(!controller.interrupt_pending(29).unwrap());

        // A still-high input is not a second edge.
        controller.set_private_timer_irq_level(true);
        assert!(!controller.interrupt_pending(29).unwrap());

        controller.set_private_timer_irq_level(false);
        controller.set_private_timer_irq_level(true);
        assert!(controller.interrupt_pending(29).unwrap());
    }

    #[test]
    fn priority_mask_and_priority_register_gate_delivery() {
        let mut controller = MpcoreInterruptController::default();
        enable_timer_irq(&mut controller);
        controller.write_distributor_byte(GICD_IPRIORITYR_BASE + 29, 0xA0);
        controller.set_private_timer_irq_level(true);

        assert!(!controller.irq_asserted());
        controller.write_cpu_interface_register(GICC_PMR, 0xB0);
        assert!(controller.irq_asserted());
    }

    #[test]
    fn guest_can_reconfigure_the_timer_ppi_as_level_sensitive() {
        let mut controller = MpcoreInterruptController::default();
        enable_timer_irq(&mut controller);
        controller.set_private_timer_irq_level(true);

        // PPI 29 occupies bits 26-27 in GICD_ICFGR1. Clear bit 27
        // while preserving the other fields in the register. Because the
        // private-timer input is still high, level mode remains pending.
        let cfg1 = controller.read_distributor_byte(GICD_ICFGR_BASE + 4 + 3);
        controller.write_distributor_byte(GICD_ICFGR_BASE + 4 + 3, cfg1 & !0x08);
        assert!(controller.interrupt_pending(29).unwrap());

        // Acknowledge, then clear the timer line; the active interrupt is not
        // pending again once the level-sensitive input is low.
        assert_eq!(
            controller.read_cpu_interface_register(GICC_IAR),
            Some(MPCORE_PRIVATE_TIMER_INTERRUPT_ID)
        );
        controller.set_private_timer_irq_level(false);
        controller.write_cpu_interface_register(GICC_EOIR, 29);
        assert!(!controller.interrupt_pending(29).unwrap());
    }

    #[test]
    fn binary_point_splits_only_the_implemented_priority_bits() {
        let mut controller = MpcoreInterruptController::default();
        assert_eq!(controller.priority_group(0b1010), 0b1010);

        controller.write_cpu_interface_register(GICC_BPR, 4);
        assert_eq!(controller.priority_group(0b1010), 0b0101);

        controller.write_cpu_interface_register(GICC_BPR, 6);
        assert_eq!(controller.priority_group(0b1010), 0b0001);

        controller.write_cpu_interface_register(GICC_BPR, 7);
        assert_eq!(controller.priority_group(0b1010), 0);
    }

    #[test]
    fn pending_interrupt_preempts_only_with_a_higher_priority_group() {
        let mut controller = MpcoreInterruptController::default();
        controller.write_distributor_byte(GICD_CTLR, 1);
        controller.write_distributor_byte(GICD_ISENABLER_BASE + 3, 0x60);
        controller.write_cpu_interface_register(GICC_CTLR, 1);
        controller.write_distributor_byte(GICD_IPRIORITYR_BASE + 29, 0xA0);
        controller.write_distributor_byte(GICD_IPRIORITYR_BASE + 30, 0xB0);
        controller.set_private_timer_irq_level(true);

        assert_eq!(
            controller.read_cpu_interface_register(GICC_IAR),
            Some(MPCORE_PRIVATE_TIMER_INTERRUPT_ID)
        );
        controller.write_distributor_byte(GICD_ISPENDR_BASE + 3, 0x40);

        // With the reset BPR value (3), the implemented priority nibble is
        // the whole group field. Priority B is lower than active priority A
        // and cannot preempt it.
        assert!(!controller.irq_asserted());

        // BPR=4 groups priorities A and B together, so subpriority cannot
        // preempt the active priority group either.
        controller.write_cpu_interface_register(GICC_BPR, 4);
        assert!(!controller.irq_asserted());

        // Move interrupt 30 to group 4, which is higher than active group 5.
        controller.write_distributor_byte(GICD_IPRIORITYR_BASE + 30, 0x90);
        assert!(controller.irq_asserted());
    }

    #[test]
    fn byte_writes_to_priority_target_and_configuration_preserve_neighbor_lanes() {
        let mut controller = MpcoreInterruptController::default();
        controller.write_distributor_byte(GICD_IPRIORITYR_BASE + 29, 0xA0);
        assert_eq!(controller.interrupt_priority(29), Some(0xA0));
        assert_eq!(controller.interrupt_priority(28), Some(0));
        assert_eq!(
            controller.read_distributor_byte(GICD_IPRIORITYR_BASE + 29),
            0xA0
        );

        controller.write_distributor_byte(0x800 + 32, 1);
        assert_eq!(controller.read_distributor_byte(0x800 + 32), 1);
        assert_eq!(controller.read_distributor_byte(0x800 + 33), 0);

        // Change only the PPI 29 trigger bit. The neighboring PPI 30
        // configuration must remain edge-sensitive.
        let config_byte = GICD_ICFGR_BASE + 7;
        controller.write_distributor_byte(config_byte, 0x20);
        let config = controller.read_distributor_byte(config_byte);
        assert_eq!(config & 0x08, 0);
        assert_ne!(config & 0x20, 0);
        assert!(!controller.interrupt_pending(29).unwrap());
    }

    #[test]
    fn active_set_and_clear_registers_preserve_interrupt_lifecycle() {
        let mut controller = MpcoreInterruptController::default();
        controller.write_distributor_byte(GICD_ISACTIVER_BASE + 3, 0x20);
        assert_eq!(controller.interrupt_active(29), Some(true));
        assert_eq!(
            controller.read_distributor_byte(GICD_ISACTIVER_BASE + 3) & 0x20,
            0x20
        );

        controller.write_distributor_byte(GICD_ICACTIVER_BASE + 3, 0x20);
        assert_eq!(controller.interrupt_active(29), Some(false));
        assert_eq!(
            controller.read_distributor_byte(GICD_ISACTIVER_BASE + 3) & 0x20,
            0
        );
    }

    #[test]
    fn register_windows_stop_at_the_128_interrupt_limit() {
        let mut controller = MpcoreInterruptController::default();

        // The fourth 32-ID bank contains IDs 96..127. The next bank is
        // outside the advertised GICD_TYPER range and must remain reserved.
        controller.write_distributor_byte(GICD_ISENABLER_BASE + 0x0F, 0x80);
        assert_eq!(controller.interrupt_enabled(127), Some(true));
        assert_eq!(
            controller.read_distributor_byte(GICD_ISENABLER_BASE + 0x10),
            0
        );
        controller.write_distributor_byte(GICD_ISENABLER_BASE + 0x10, 0xFF);
        assert_eq!(controller.interrupt_enabled(127), Some(true));

        // ICFGR has eight 32-bit registers for 128 interrupts. The final
        // byte programs IDs 124..127; the following byte is reserved.
        controller.write_distributor_byte(GICD_ICFGR_BASE + 31, 0xAA);
        assert_eq!(controller.read_distributor_byte(GICD_ICFGR_BASE + 31), 0xAA);
        assert_eq!(controller.read_distributor_byte(GICD_ICFGR_BASE + 32), 0);
    }

    #[test]
    fn clear_registers_cannot_disable_sgis_or_write_read_only_state() {
        let mut controller = MpcoreInterruptController::default();
        controller.write_distributor_byte(GICD_ICENABLER_BASE, 0xFF);
        assert_eq!(controller.interrupt_enabled(0), Some(true));
        assert_eq!(controller.interrupt_enabled(7), Some(true));
        assert_eq!(controller.interrupt_enabled(8), Some(false));

        let old_type = controller.read_distributor_byte(0x004);
        controller.write_distributor_byte(0x004, 0xFF);
        assert_eq!(controller.read_distributor_byte(0x004), old_type);
    }
}
