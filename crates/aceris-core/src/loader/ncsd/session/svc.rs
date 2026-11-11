//! Initial ARM11 supervisor-call services.
//!
//! Only services whose behavior can be represented by the current single-core,
//! single-thread bootstrap are handled here. Unknown services are returned to
//! the caller instead of being silently treated as successful.

/// Exit the current process (Nintendo 3DS ARM11 SVC 0x03).
pub const SVC_EXIT_PROCESS: u32 = 0x03;

/// Return the processor identifier for the current thread (SVC 0x11).
pub const SVC_GET_CURRENT_PROCESSOR_NUMBER: u32 = 0x11;

/// Result of dispatching one SVC instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm11SvcDisposition {
    /// The service completed synchronously and execution may continue.
    Resume,
    /// The current process has exited and must no longer execute instructions.
    ProcessExited,
    /// No handler exists for this service number.
    Unhandled { number: u32 },
}

/// Minimal process-scoped SVC dispatcher for the bootstrap execution session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arm11SvcDispatcher {
    process_exited: bool,
}

impl Arm11SvcDispatcher {
    /// Returns whether the process has already exited.
    pub const fn process_exited(&self) -> bool {
        self.process_exited
    }

    /// Dispatches an SVC using the ARM11 user-mode register convention.
    ///
    /// The current session models a single application core, numbered zero.
    /// GetCurrentProcessorNumber therefore returns zero in r0.
    pub fn dispatch(&mut self, number: u32, registers: &mut [u32; 16]) -> Arm11SvcDisposition {
        if self.process_exited {
            return Arm11SvcDisposition::ProcessExited;
        }

        match number {
            SVC_EXIT_PROCESS => {
                self.process_exited = true;
                Arm11SvcDisposition::ProcessExited
            }
            SVC_GET_CURRENT_PROCESSOR_NUMBER => {
                registers[0] = 0;
                Arm11SvcDisposition::Resume
            }
            _ => Arm11SvcDisposition::Unhandled { number },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_processor_number_is_returned_in_r0() {
        let mut dispatcher = Arm11SvcDispatcher::default();
        let mut registers = [0xA5A5_A5A5; 16];

        assert_eq!(
            dispatcher.dispatch(SVC_GET_CURRENT_PROCESSOR_NUMBER, &mut registers),
            Arm11SvcDisposition::Resume
        );
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0xA5A5_A5A5);
        assert!(!dispatcher.process_exited());
    }

    #[test]
    fn exit_process_is_terminal_for_the_dispatcher() {
        let mut dispatcher = Arm11SvcDispatcher::default();
        let mut registers = [0; 16];

        assert_eq!(
            dispatcher.dispatch(SVC_EXIT_PROCESS, &mut registers),
            Arm11SvcDisposition::ProcessExited
        );
        assert!(dispatcher.process_exited());
        assert_eq!(
            dispatcher.dispatch(SVC_GET_CURRENT_PROCESSOR_NUMBER, &mut registers),
            Arm11SvcDisposition::ProcessExited
        );
    }

    #[test]
    fn unknown_services_are_not_reported_as_successful() {
        let mut dispatcher = Arm11SvcDispatcher::default();
        let mut registers = [0; 16];

        assert_eq!(
            dispatcher.dispatch(0x42, &mut registers),
            Arm11SvcDisposition::Unhandled { number: 0x42 }
        );
    }
}
