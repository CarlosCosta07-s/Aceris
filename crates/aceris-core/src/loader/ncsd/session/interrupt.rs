//! User-bindable ARM11 interrupt registrations.
//!
//! The controller models the public SVC contract: only SGI 15 and hardware
//! interrupt IDs 0x20..=0x7F may be bound, one synchronization object per ID.
//! Device delivery and process-local handle validation are performed by the
//! owning process session.

/// ARM11 SGI used by the debugger/FIQ abstraction and exposed to BindInterrupt.
pub const DEBUG_FIQ_INTERRUPT_ID: u32 = 0x0F;
/// First public hardware interrupt ID.
pub const FIRST_HARDWARE_INTERRUPT_ID: u32 = 0x20;
/// Last public hardware interrupt ID.
pub const LAST_HARDWARE_INTERRUPT_ID: u32 = 0x7F;
/// Priority value that disables an interrupt.
pub const DISABLED_INTERRUPT_PRIORITY: u8 = 0x0F;

/// Kind of process-local synchronization object attached to an interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptTargetKind {
    Event,
    Semaphore,
}

/// A guest registration made through SVC BindInterrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptBinding {
    pub interrupt_id: u8,
    pub handle: u32,
    pub priority: u8,
    pub level_high_active: bool,
    pub target: InterruptTargetKind,
}

/// Errors returned while binding or unbinding an interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptBindingError {
    InvalidInterrupt(u32),
    InvalidPriority(i32),
    InvalidCombination,
    AlreadyBound(u32),
    NotBound(u32),
    HandleMismatch { interrupt_id: u32, handle: u32 },
}

/// Outcome of delivering a hardware interrupt to a process-local registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptDispatchOutcome {
    NotBound,
    Disabled,
    Delivered { awakened_threads: usize },
    StaleBinding,
}

/// Per-process view of the ARM11 user-bindable interrupt table.
///
/// Public hardware interrupt sources are global devices, but the guest-facing
/// registration is associated with the process synchronization object table.
/// Aceris currently runs one application process at a time.
#[derive(Clone, Debug)]
pub struct Arm11InterruptController {
    bindings: [Option<InterruptBinding>; LAST_HARDWARE_INTERRUPT_ID as usize + 1],
}

impl Default for Arm11InterruptController {
    fn default() -> Self {
        Self {
            bindings: [None; LAST_HARDWARE_INTERRUPT_ID as usize + 1],
        }
    }
}

impl Arm11InterruptController {
    /// Returns whether an interrupt ID is user-bindable on the documented ARM11 API.
    pub const fn is_bindable_interrupt(interrupt_id: u32) -> bool {
        interrupt_id == DEBUG_FIQ_INTERRUPT_ID
            || (interrupt_id >= FIRST_HARDWARE_INTERRUPT_ID
                && interrupt_id <= LAST_HARDWARE_INTERRUPT_ID)
    }

    /// Registers one event or semaphore for an interrupt ID.
    pub fn bind(
        &mut self,
        interrupt_id: u32,
        handle: u32,
        priority: i32,
        level_high_active: bool,
        target: InterruptTargetKind,
    ) -> Result<(), InterruptBindingError> {
        if !Self::is_bindable_interrupt(interrupt_id) {
            return Err(InterruptBindingError::InvalidInterrupt(interrupt_id));
        }
        if !(0..=i32::from(DISABLED_INTERRUPT_PRIORITY)).contains(&priority) {
            return Err(InterruptBindingError::InvalidPriority(priority));
        }
        if (target == InterruptTargetKind::Semaphore && level_high_active)
            || (interrupt_id == DEBUG_FIQ_INTERRUPT_ID && level_high_active)
        {
            return Err(InterruptBindingError::InvalidCombination);
        }

        let slot = &mut self.bindings[interrupt_id as usize];
        if slot.is_some() {
            return Err(InterruptBindingError::AlreadyBound(interrupt_id));
        }

        *slot = Some(InterruptBinding {
            interrupt_id: interrupt_id as u8,
            handle,
            priority: priority as u8,
            level_high_active,
            target,
        });
        Ok(())
    }

    /// Removes a registration only when the supplied handle matches it.
    pub fn unbind(&mut self, interrupt_id: u32, handle: u32) -> Result<(), InterruptBindingError> {
        if !Self::is_bindable_interrupt(interrupt_id) {
            return Err(InterruptBindingError::InvalidInterrupt(interrupt_id));
        }

        let slot = &mut self.bindings[interrupt_id as usize];
        let Some(binding) = *slot else {
            return Err(InterruptBindingError::NotBound(interrupt_id));
        };
        if binding.handle != handle {
            return Err(InterruptBindingError::HandleMismatch {
                interrupt_id,
                handle,
            });
        }

        *slot = None;
        Ok(())
    }

    /// Returns the current binding for a supported interrupt ID.
    pub fn binding(&self, interrupt_id: u32) -> Option<InterruptBinding> {
        self.bindings
            .get(usize::try_from(interrupt_id).ok()?)
            .copied()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Arm11InterruptController, DISABLED_INTERRUPT_PRIORITY, InterruptBindingError,
        InterruptTargetKind,
    };

    #[test]
    fn only_debug_fiq_and_public_hardware_interrupts_are_bindable() {
        let mut controller = Arm11InterruptController::default();
        for interrupt_id in [0, 14, 16, 31, 0x80, u32::MAX] {
            assert_eq!(
                controller.bind(interrupt_id, 0x100, 4, false, InterruptTargetKind::Event),
                Err(InterruptBindingError::InvalidInterrupt(interrupt_id))
            );
        }

        controller
            .bind(0x0F, 0x100, 4, false, InterruptTargetKind::Event)
            .unwrap();
        controller
            .bind(0x28, 0x104, 4, false, InterruptTargetKind::Event)
            .unwrap();
        controller
            .bind(0x7F, 0x108, 4, false, InterruptTargetKind::Event)
            .unwrap();
    }

    #[test]
    fn duplicate_registration_and_mismatched_unbind_are_rejected() {
        let mut controller = Arm11InterruptController::default();
        controller
            .bind(0x2A, 0x100, 4, false, InterruptTargetKind::Event)
            .unwrap();

        assert_eq!(
            controller.bind(0x2A, 0x104, 4, false, InterruptTargetKind::Event),
            Err(InterruptBindingError::AlreadyBound(0x2A))
        );
        assert_eq!(
            controller.unbind(0x2A, 0x104),
            Err(InterruptBindingError::HandleMismatch {
                interrupt_id: 0x2A,
                handle: 0x104,
            })
        );
        assert_eq!(controller.binding(0x2A).unwrap().handle, 0x100);
        controller.unbind(0x2A, 0x100).unwrap();
        assert_eq!(controller.binding(0x2A), None);
    }

    #[test]
    fn validates_priorities_and_semaphore_level_mode() {
        let mut controller = Arm11InterruptController::default();
        for priority in [-1, 16, i32::MAX] {
            assert_eq!(
                controller.bind(0x2A, 0x100, priority, false, InterruptTargetKind::Event),
                Err(InterruptBindingError::InvalidPriority(priority))
            );
        }
        assert_eq!(
            controller.bind(0x2A, 0x100, 4, true, InterruptTargetKind::Semaphore),
            Err(InterruptBindingError::InvalidCombination)
        );
        assert_eq!(
            controller.bind(0x0F, 0x100, 4, true, InterruptTargetKind::Event),
            Err(InterruptBindingError::InvalidCombination)
        );

        controller
            .bind(
                0x2A,
                0x100,
                i32::from(DISABLED_INTERRUPT_PRIORITY),
                false,
                InterruptTargetKind::Semaphore,
            )
            .unwrap();
    }
}
