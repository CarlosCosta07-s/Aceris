//! ARM11 MPCore per-CPU private timer.
//!
//! This is the core-0 timer bank at 0x17E00600 used by the 3DS application
//! core. The model is driven by the emulated peripheral clock, not wall time.
//! The timer's interrupt output is exposed separately so the owning process
//! session can feed it into the MPCore distributor as private IRQ 29.

/// Offset of the application-core private timer bank in the MPCore region.
pub const MPCORE_PRIVATE_TIMER_OFFSET: u32 = 0x0600;
/// Fixed CPU0 alias of the private timer bank in the MPCore region.
pub const MPCORE_PRIVATE_TIMER_CPU0_ALIAS_OFFSET: u32 = 0x0700;
/// Size of the four word-accessible private-timer registers.
pub const MPCORE_PRIVATE_TIMER_REGISTER_SIZE: u32 = 0x0010;
/// Default Old 3DS private-timer input clock in hertz.
///
/// The ARM11 MPCore TRM specifies the private timer interval as
/// ((prescaler + 1) * (load + 1) * 2) / CPU_CLK_frequency.
pub const MPCORE_TIMER_DEFAULT_CLOCK_HZ: u32 = 134_055_928;

/// Offset of the Timer Load Register.
pub const TIMER_LOAD_REGISTER_OFFSET: u32 = 0x00;
/// Offset of the Timer Counter Register.
pub const TIMER_COUNTER_REGISTER_OFFSET: u32 = 0x04;
/// Offset of the Timer Control Register.
pub const TIMER_CONTROL_REGISTER_OFFSET: u32 = 0x08;
/// Offset of the Timer Interrupt Status Register.
pub const TIMER_INTERRUPT_STATUS_REGISTER_OFFSET: u32 = 0x0C;

const TIMER_ENABLE: u32 = 1 << 0;
const TIMER_AUTO_RELOAD: u32 = 1 << 1;
const TIMER_INTERRUPT_ENABLE: u32 = 1 << 2;
const TIMER_PRESCALER_MASK: u32 = 0x0000_FF00;
const TIMER_CONTROL_WRITABLE_MASK: u32 =
    TIMER_PRESCALER_MASK | TIMER_INTERRUPT_ENABLE | TIMER_AUTO_RELOAD | TIMER_ENABLE;
const NANOSECONDS_PER_SECOND: u128 = 1_000_000_000;

/// Deterministic ARM11 MPCore private timer state for one CPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MpcorePrivateTimer {
    load: u32,
    counter: u32,
    control: u32,
    interrupt_status: bool,
    /// Fractional peripheral cycles, represented over 1e9.
    clock_fraction: u128,
    /// Peripheral cycles retained until the prescaler emits one timer tick.
    prescaler_remainder: u16,
    /// A one-shot timer stops after its first underflow until reloaded.
    expired: bool,
}

impl MpcorePrivateTimer {
    /// Reads an aligned 32-bit timer register.
    ///
    /// Returns None for unaligned or reserved register offsets.
    pub fn read_register(&self, offset: u32) -> Option<u32> {
        if offset & 3 != 0 {
            return None;
        }
        match offset {
            TIMER_LOAD_REGISTER_OFFSET => Some(self.load),
            TIMER_COUNTER_REGISTER_OFFSET => Some(self.counter),
            TIMER_CONTROL_REGISTER_OFFSET => Some(self.control),
            TIMER_INTERRUPT_STATUS_REGISTER_OFFSET => {
                Some(u32::from(self.interrupt_status))
            }
            _ => None,
        }
    }

    /// Writes an aligned 32-bit timer register.
    ///
    /// Reserved control bits are ignored. The interrupt status register is
    /// write-one-to-clear, matching the MPCore timer programming model.
    /// Returns false for unaligned or reserved register offsets.
    pub fn write_register(&mut self, offset: u32, value: u32) -> bool {
        if offset & 3 != 0 {
            return false;
        }

        match offset {
            TIMER_LOAD_REGISTER_OFFSET => {
                self.load = value;
                self.counter = value;
                self.expired = false;
                true
            }
            TIMER_COUNTER_REGISTER_OFFSET => {
                self.counter = value;
                self.expired = false;
                true
            }
            TIMER_CONTROL_REGISTER_OFFSET => {
                let previous_enable = self.control & TIMER_ENABLE != 0;
                let previous_prescaler = self.control & TIMER_PRESCALER_MASK;
                self.control = value & TIMER_CONTROL_WRITABLE_MASK;
                let current_enable = self.control & TIMER_ENABLE != 0;
                let current_prescaler = self.control & TIMER_PRESCALER_MASK;

                if previous_enable != current_enable || previous_prescaler != current_prescaler {
                    self.clock_fraction = 0;
                    self.prescaler_remainder = 0;
                }
                true
            }
            TIMER_INTERRUPT_STATUS_REGISTER_OFFSET => {
                if value & 1 != 0 {
                    self.interrupt_status = false;
                }
                true
            }
            _ => false,
        }
    }

    /// Advances the timer using elapsed virtual time and the selected
    /// peripheral clock. The caller supplies the hardware clock rate so a
    /// future New 3DS clock-mode model can change it without changing timer
    /// register semantics.
    ///
    /// A load value of N expires after N+1 prescaled timer ticks. In auto-
    /// reload mode, each expiration reloads the load register. The interrupt
    /// status is sticky until software writes one to its status bit.
    pub fn advance_time(&mut self, elapsed_ns: u64, peripheral_clock_hz: u32) {
        if elapsed_ns == 0 || peripheral_clock_hz == 0 || self.is_stopped() {
            return;
        }

        let scaled_cycles = u128::from(elapsed_ns)
            .saturating_mul(u128::from(peripheral_clock_hz))
            .saturating_add(self.clock_fraction);
        let peripheral_cycles = scaled_cycles / NANOSECONDS_PER_SECOND;
        self.clock_fraction = scaled_cycles % NANOSECONDS_PER_SECOND;
        if peripheral_cycles == 0 {
            return;
        }

        let prescaler = u128::from((self.control & TIMER_PRESCALER_MASK) >> 8);
        let divisor = prescaler + 1;
        let accumulated = peripheral_cycles + u128::from(self.prescaler_remainder);
        let timer_ticks = accumulated / divisor;
        self.prescaler_remainder = (accumulated % divisor) as u16;
        if timer_ticks == 0 {
            return;
        }

        self.apply_timer_ticks(timer_ticks);
    }

    /// Whether the timer is currently driving its private IRQ output (ID 29).
    pub const fn irq_asserted(&self) -> bool {
        self.interrupt_status && (self.control & TIMER_INTERRUPT_ENABLE != 0)
    }

    /// Whether the guest-visible interrupt status flag is set.
    pub const fn interrupt_pending(&self) -> bool {
        self.interrupt_status
    }

    /// Returns the current counter value.
    pub const fn counter(&self) -> u32 {
        self.counter
    }

    /// Returns the current load value.
    pub const fn load(&self) -> u32 {
        self.load
    }

    /// Returns the current control register.
    pub const fn control(&self) -> u32 {
        self.control
    }

    fn is_stopped(&self) -> bool {
        self.control & TIMER_ENABLE == 0 || self.expired
    }

    fn apply_timer_ticks(&mut self, ticks: u128) {
        // The first expiration occurs after counter + 1 ticks. This also
        // handles a programmed counter of zero without a special fast path.
        let ticks_until_expiration = u128::from(self.counter) + 1;
        if ticks < ticks_until_expiration {
            self.counter -= ticks as u32;
            return;
        }

        self.interrupt_status = true;
        let remaining_ticks = ticks - ticks_until_expiration;
        if self.control & TIMER_AUTO_RELOAD == 0 {
            self.counter = 0;
            self.expired = true;
            return;
        }

        let reload_period = u128::from(self.load) + 1;
        let remainder = remaining_ticks % reload_period;
        self.counter = self.load - remainder as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MPCORE_TIMER_DEFAULT_CLOCK_HZ, MpcorePrivateTimer, TIMER_CONTROL_REGISTER_OFFSET,
        TIMER_COUNTER_REGISTER_OFFSET, TIMER_INTERRUPT_STATUS_REGISTER_OFFSET,
        TIMER_LOAD_REGISTER_OFFSET,
    };

    const ENABLE: u32 = 1;
    const AUTO_RELOAD: u32 = 1 << 1;
    const IRQ_ENABLE: u32 = 1 << 2;

    fn advance_cycles(timer: &mut MpcorePrivateTimer, cycles: u64) {
        // Choose a duration whose integer conversion produces exactly the
        // requested peripheral-cycle count at the default 3DS clock.
        let numerator = u128::from(cycles) * 1_000_000_000;
        let nanoseconds = numerator.div_ceil(u128::from(MPCORE_TIMER_DEFAULT_CLOCK_HZ));
        timer.advance_time(
            u64::try_from(nanoseconds).unwrap(),
            MPCORE_TIMER_DEFAULT_CLOCK_HZ,
        );
    }

    #[test]
    fn timer_registers_reset_to_zero_and_reject_reserved_offsets() {
        let timer = MpcorePrivateTimer::default();
        assert_eq!(timer.read_register(TIMER_LOAD_REGISTER_OFFSET), Some(0));
        assert_eq!(timer.read_register(TIMER_COUNTER_REGISTER_OFFSET), Some(0));
        assert_eq!(timer.read_register(TIMER_CONTROL_REGISTER_OFFSET), Some(0));
        assert_eq!(
            timer.read_register(TIMER_INTERRUPT_STATUS_REGISTER_OFFSET),
            Some(0)
        );
        assert_eq!(timer.read_register(0x10), None);
        assert_eq!(timer.read_register(1), None);
    }

    #[test]
    fn load_and_counter_writes_restart_the_one_shot_countdown() {
        let mut timer = MpcorePrivateTimer::default();
        assert!(timer.write_register(TIMER_LOAD_REGISTER_OFFSET, 7));
        assert_eq!(timer.counter(), 7);
        assert!(timer.write_register(TIMER_COUNTER_REGISTER_OFFSET, 3));
        assert_eq!(timer.counter(), 3);
        assert!(timer.write_register(TIMER_CONTROL_REGISTER_OFFSET, ENABLE));
        advance_cycles(&mut timer, 3);
        assert_eq!(timer.counter(), 0);
        assert!(!timer.interrupt_pending());
        advance_cycles(&mut timer, 1);
        assert!(timer.interrupt_pending());
        assert_eq!(timer.counter(), 0);
    }

    #[test]
    fn one_shot_interrupt_is_sticky_and_write_one_to_clear() {
        let mut timer = MpcorePrivateTimer::default();
        timer.write_register(TIMER_LOAD_REGISTER_OFFSET, 0);
        timer.write_register(TIMER_CONTROL_REGISTER_OFFSET, ENABLE | IRQ_ENABLE);
        advance_cycles(&mut timer, 1);
        assert!(timer.interrupt_pending());
        assert!(timer.irq_asserted());

        advance_cycles(&mut timer, 100);
        assert!(timer.interrupt_pending());
        assert!(timer.irq_asserted());

        timer.write_register(TIMER_INTERRUPT_STATUS_REGISTER_OFFSET, 0);
        assert!(timer.interrupt_pending());
        timer.write_register(TIMER_INTERRUPT_STATUS_REGISTER_OFFSET, 1);
        assert!(!timer.interrupt_pending());
        assert!(!timer.irq_asserted());
    }

    #[test]
    fn timer_can_count_without_generating_an_irq() {
        let mut timer = MpcorePrivateTimer::default();
        timer.write_register(TIMER_LOAD_REGISTER_OFFSET, 1);
        timer.write_register(TIMER_CONTROL_REGISTER_OFFSET, ENABLE);
        advance_cycles(&mut timer, 2);
        assert_eq!(timer.counter(), 0);
        advance_cycles(&mut timer, 1);
        assert!(timer.interrupt_pending());
        assert!(!timer.irq_asserted());
    }

    #[test]
    fn auto_reload_preserves_the_remainder_after_multiple_expirations() {
        let mut timer = MpcorePrivateTimer::default();
        timer.write_register(TIMER_LOAD_REGISTER_OFFSET, 2);
        timer.write_register(
            TIMER_CONTROL_REGISTER_OFFSET,
            ENABLE | AUTO_RELOAD | IRQ_ENABLE,
        );

        advance_cycles(&mut timer, 10);
        assert!(timer.interrupt_pending());
        assert!(timer.irq_asserted());
        // Period is three ticks: after 10 ticks, three full periods elapsed
        // and one tick has elapsed from the next period.
        assert_eq!(timer.counter(), 1);
    }

    #[test]
    fn prescaler_divides_peripheral_cycles_and_retains_partial_divisions() {
        let mut timer = MpcorePrivateTimer::default();
        timer.write_register(TIMER_LOAD_REGISTER_OFFSET, 0);
        // Prescaler 2 divides the peripheral clock by three.
        timer.write_register(TIMER_CONTROL_REGISTER_OFFSET, ENABLE | (2 << 8));
        advance_cycles(&mut timer, 2);
        assert!(!timer.interrupt_pending());
        advance_cycles(&mut timer, 1);
        assert!(timer.interrupt_pending());
    }

    #[test]
    fn fractional_clock_cycles_accumulate_across_short_virtual_time_steps() {
        let mut timer = MpcorePrivateTimer::default();
        timer.write_register(TIMER_LOAD_REGISTER_OFFSET, 0);
        timer.write_register(TIMER_CONTROL_REGISTER_OFFSET, ENABLE | IRQ_ENABLE);
        for _ in 0..7 {
            timer.advance_time(1, MPCORE_TIMER_DEFAULT_CLOCK_HZ);
        }
        assert!(!timer.interrupt_pending());
        timer.advance_time(1, MPCORE_TIMER_DEFAULT_CLOCK_HZ);
        assert!(timer.interrupt_pending());
        assert!(timer.irq_asserted());
    }

    #[test]
    fn control_reserved_bits_are_ignored_and_status_is_write_one_to_clear() {
        let mut timer = MpcorePrivateTimer::default();
        timer.write_register(
            TIMER_CONTROL_REGISTER_OFFSET,
            u32::MAX,
        );
        assert_eq!(timer.control(), 0x0000_FF07);
        assert!(!timer.write_register(0x10, 0));
        assert!(!timer.write_register(3, 0));
    }
}
