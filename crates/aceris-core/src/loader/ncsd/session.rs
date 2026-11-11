//! ARM11 process execution session.
//!
//! This module connects the validated NCSD loader, a process address space,
//! the CPU interpreter, and a small HLE SVC dispatcher. It does not yet model
//! privileged CPU modes, multiple threads, timers, synchronization, or devices.

pub mod svc;
/// ARM11 per-thread contexts and cooperative scheduling primitives.
pub mod thread;
pub use thread::{Arm11ThreadScheduler, ThreadId, ThreadInfo, ThreadSchedulerError, ThreadState};

use std::io::{Read, Seek};

use crate::Result;
use crate::cpu::{Arm11Core, StepEvent};
use crate::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use crate::loader::ncsd::process::Arm11CodeAddressSpace;
use crate::loader::ncsd::{NcsdImageReader, NcsdImageReaderError};
use svc::{Arm11SvcDispatcher, Arm11SvcDisposition};

/// Reason an instruction-budgeted run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm11RunStopReason {
    /// The requested instruction budget was consumed.
    InstructionBudgetExhausted,
    /// An SVC was reached but has no implemented handler.
    SupervisorCall { number: u32 },
    /// The process called ExitProcess and can no longer execute.
    ProcessExited,
}

/// Result of one call to Arm11ProcessSession::run_instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm11RunReport {
    /// Instructions successfully executed in this call, including SVC instructions.
    pub instructions_executed: u64,
    /// Why execution stopped.
    pub stop_reason: Arm11RunStopReason,
}

/// An initialized ARM11 CPU paired with its process address space and SVC state.
#[derive(Debug)]
pub struct Arm11ProcessSession {
    core: Arm11Core,
    address_space: Arm11CodeAddressSpace,
    svc_dispatcher: Arm11SvcDispatcher,
    instruction_count: u64,
}

impl Arm11ProcessSession {
    /// Creates a session and initializes PC and SP from the process layout.
    pub fn from_address_space(address_space: Arm11CodeAddressSpace) -> Self {
        let mut core = Arm11Core::new();
        address_space.initialize_core(&mut core);
        Self {
            core,
            address_space,
            svc_dispatcher: Arm11SvcDispatcher::default(),
            instruction_count: 0,
        }
    }

    /// Loads partition zero from an NCSD image and initializes its ARM11 session.
    ///
    /// AES keys must be supplied by the caller. This API does not derive or
    /// persist keys. The loader validates NCSD, NCCH, ExeFS and code layout.
    pub fn load_primary_ncsd<R: Read + Seek>(
        reader: R,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> std::result::Result<Self, NcsdImageReaderError> {
        let address_space = NcsdImageReader::open(reader)?
            .load_primary_arm11_address_space(menu_info_key, content_key)?;
        Ok(Self::from_address_space(address_space))
    }

    /// Returns the ARM11 register state.
    pub const fn core(&self) -> &Arm11Core {
        &self.core
    }

    /// Returns the process address space.
    pub const fn address_space(&self) -> &Arm11CodeAddressSpace {
        &self.address_space
    }

    /// Returns whether ExitProcess has already terminated this session.
    pub const fn process_exited(&self) -> bool {
        self.svc_dispatcher.process_exited()
    }

    /// Returns the total number of successfully executed instructions.
    pub const fn instruction_count(&self) -> u64 {
        self.instruction_count
    }

    /// Executes one raw CPU instruction without dispatching an SVC.
    ///
    /// Use run_instructions for normal execution through the HLE SVC dispatcher.
    pub fn step(&mut self) -> Result<StepEvent> {
        let event = self.core.step(&mut self.address_space)?;
        self.instruction_count = self.instruction_count.saturating_add(1);
        Ok(event)
    }

    /// Runs no more than instruction_budget instructions, dispatching supported SVCs.
    ///
    /// A zero budget performs no work. Implemented synchronous SVCs continue
    /// execution within the remaining budget. Unknown SVCs stop the run and
    /// are returned explicitly. Once ExitProcess is handled, later calls
    /// return ProcessExited without advancing the CPU.
    ///
    /// Errors from instruction fetch or memory access are propagated, and
    /// instruction_count retains successful steps completed before the error.
    pub fn run_instructions(&mut self, instruction_budget: usize) -> Result<Arm11RunReport> {
        if self.svc_dispatcher.process_exited() {
            return Ok(Arm11RunReport {
                instructions_executed: 0,
                stop_reason: Arm11RunStopReason::ProcessExited,
            });
        }

        let mut executed = 0u64;
        for _ in 0..instruction_budget {
            match self.step()? {
                StepEvent::Executed => executed = executed.saturating_add(1),
                StepEvent::Svc(number) => {
                    executed = executed.saturating_add(1);
                    match self.svc_dispatcher.dispatch(number, &mut self.core.regs) {
                        Arm11SvcDisposition::Resume => {}
                        Arm11SvcDisposition::ProcessExited => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::ProcessExited,
                            });
                        }
                        Arm11SvcDisposition::Unhandled { number } => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::SupervisorCall { number },
                            });
                        }
                    }
                }
            }
        }

        Ok(Arm11RunReport {
            instructions_executed: executed,
            stop_reason: Arm11RunStopReason::InstructionBudgetExhausted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::ncsd::PreparedArm11Image;
    use crate::loader::ncsd::ncch::extended_header::{CodeSetInfo, NcchExtendedHeader};

    fn test_session_with_program(instructions: &[u32]) -> Arm11ProcessSession {
        let mut text = vec![0u8; 0x1000];
        for (index, instruction) in instructions.iter().copied().enumerate() {
            let start = index * 4;
            text[start..start + 4].copy_from_slice(&instruction.to_le_bytes());
        }
        let prepared = PreparedArm11Image {
            extended_header: NcchExtendedHeader {
                application_title: *b"ACERIS!!",
                flags: 0,
                remaster_version: 0,
                text_code_set: CodeSetInfo {
                    address: 0x0010_0000,
                    physical_size_pages: 1,
                    size_bytes: 0x1000,
                },
                stack_size_bytes: 0x1000,
                read_only_code_set: CodeSetInfo::default(),
                data_code_set: CodeSetInfo::default(),
                bss_size_bytes: 0,
                dependencies: [0; 48],
                save_data_size_bytes: 0,
                jump_id: 0,
                access_control_info: [0; 0x200],
            },
            text,
            read_only: Vec::new(),
            data: Vec::new(),
        };
        let address_space = Arm11CodeAddressSpace::from_prepared(prepared).unwrap();
        Arm11ProcessSession::from_address_space(address_space)
    }

    fn test_session() -> Arm11ProcessSession {
        test_session_with_program(&[
            0xE3A0_0007, // MOV r0, #7
            0xE3A0_1005, // MOV r1, #5
            0xEF00_0042, // SVC #0x42
        ])
    }

    #[test]
    fn initializes_entry_pc_and_main_thread_sp() {
        let session = test_session();
        assert_eq!(session.core().pc(), 0x0010_0000);
        assert_eq!(session.core().regs[13], 0x1000_0000);
        assert_eq!(session.instruction_count(), 0);
    }

    #[test]
    fn executes_until_unknown_svc_and_returns_its_number() {
        let mut session = test_session();
        let report = session.run_instructions(16).unwrap();
        assert_eq!(session.core().regs[0], 7);
        assert_eq!(session.core().regs[1], 5);
        assert_eq!(session.core().pc(), 0x0010_000C);
        assert_eq!(session.instruction_count(), 3);
        assert_eq!(report.instructions_executed, 3);
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::SupervisorCall { number: 0x42 }
        );
    }

    #[test]
    fn dispatches_get_current_processor_number_and_resumes_execution() {
        let mut session = test_session_with_program(&[
            0xE3A0_0007, // MOV r0, #7
            0xEF00_0011, // SVC #0x11 - GetCurrentProcessorNumber
            0xE3A0_1009, // MOV r1, #9
        ]);

        let report = session.run_instructions(3).unwrap();

        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], 9);
        assert_eq!(session.core().pc(), 0x0010_000C);
        assert_eq!(report.instructions_executed, 3);
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert!(!session.process_exited());
    }

    #[test]
    fn exit_process_stops_execution_and_is_terminal() {
        let mut session = test_session_with_program(&[
            0xE3A0_00FF, // MOV r0, #255
            0xEF00_0003, // SVC #0x03 - ExitProcess
            0xE3A0_1009, // MOV r1, #9 - must not run
        ]);

        let report = session.run_instructions(16).unwrap();

        assert_eq!(report.instructions_executed, 2);
        assert_eq!(report.stop_reason, Arm11RunStopReason::ProcessExited);
        assert_eq!(session.core().regs[0], 255);
        assert_eq!(session.core().regs[1], 0);
        assert_eq!(session.core().pc(), 0x0010_0008);
        assert!(session.process_exited());

        let second_report = session.run_instructions(16).unwrap();
        assert_eq!(second_report.instructions_executed, 0);
        assert_eq!(second_report.stop_reason, Arm11RunStopReason::ProcessExited);
        assert_eq!(session.core().pc(), 0x0010_0008);
    }

    #[test]
    fn respects_instruction_budget() {
        let mut session = test_session();
        let report = session.run_instructions(1).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 7);
        assert_eq!(session.core().regs[1], 0);
        assert_eq!(session.core().pc(), 0x0010_0004);
        assert_eq!(session.instruction_count(), 1);
    }

    #[test]
    fn zero_budget_does_not_change_state() {
        let mut session = test_session();
        let pc = session.core().pc();
        let report = session.run_instructions(0).unwrap();
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(session.core().pc(), pc);
        assert_eq!(session.instruction_count(), 0);
    }
}
