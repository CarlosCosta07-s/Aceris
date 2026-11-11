//! Initial ARM11 supervisor-call identifiers and process state.
//!
//! SVC argument registers are dispatched by Arm11ProcessSession, which has
//! access to the address space and scheduler needed to validate operations.

/// Control process memory mappings (allocation and release).
pub const SVC_CONTROL_MEMORY: u32 = 0x01;

/// Exit the current process.
pub const SVC_EXIT_PROCESS: u32 = 0x03;

/// Create a new thread in the current process.
pub const SVC_CREATE_THREAD: u32 = 0x08;

/// Exit the current thread.
pub const SVC_EXIT_THREAD: u32 = 0x09;

/// Yield or sleep the current thread.
pub const SVC_SLEEP_THREAD: u32 = 0x0A;

/// Get the priority of a thread.
pub const SVC_GET_THREAD_PRIORITY: u32 = 0x0B;

/// Set the priority of a thread.
pub const SVC_SET_THREAD_PRIORITY: u32 = 0x0C;

/// Return the current application processor ID.
pub const SVC_GET_PROCESSOR_ID: u32 = 0x11;

/// Result of dispatching a process-scoped SVC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm11SvcDisposition {
    /// The service completed synchronously and execution may continue.
    Resume,
    /// The process has exited and must no longer execute instructions.
    ProcessExited,
    /// No handler exists for this service number.
    Unhandled { number: u32 },
}

/// Process-scoped state shared by the ARM11 SVC dispatcher.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arm11SvcDispatcher {
    process_exited: bool,
}

impl Arm11SvcDispatcher {
    /// Returns whether the process has already exited.
    pub const fn process_exited(&self) -> bool {
        self.process_exited
    }

    /// Dispatches SVCs that do not require thread or memory manager access.
    pub fn dispatch(&mut self, number: u32, registers: &mut [u32; 16]) -> Arm11SvcDisposition {
        if self.process_exited {
            return Arm11SvcDisposition::ProcessExited;
        }

        match number {
            SVC_EXIT_PROCESS => {
                self.process_exited = true;
                Arm11SvcDisposition::ProcessExited
            }
            SVC_GET_PROCESSOR_ID => {
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
    fn processor_id_is_returned_in_r0() {
        let mut dispatcher = Arm11SvcDispatcher::default();
        let mut registers = [0xA5A5_A5A5; 16];

        assert_eq!(
            dispatcher.dispatch(SVC_GET_PROCESSOR_ID, &mut registers),
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
            dispatcher.dispatch(SVC_GET_PROCESSOR_ID, &mut registers),
            Arm11SvcDisposition::ProcessExited
        );
    }

    #[test]
    fn thread_services_are_deferred_to_the_session_dispatcher() {
        let mut dispatcher = Arm11SvcDispatcher::default();
        let mut registers = [0; 16];

        for number in [
            SVC_CONTROL_MEMORY,
            SVC_CREATE_THREAD,
            SVC_EXIT_THREAD,
            SVC_SLEEP_THREAD,
            SVC_GET_THREAD_PRIORITY,
            SVC_SET_THREAD_PRIORITY,
        ] {
            assert_eq!(
                dispatcher.dispatch(number, &mut registers),
                Arm11SvcDisposition::Unhandled { number }
            );
        }
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
