//! ARM11 process execution session.
//!
//! This module joins the validated NCSD loader, protected process memory,
//! independent ARM11 thread contexts, a cooperative scheduler and HLE SVCs.
//! Privileged CPU modes, hardware timers, synchronization objects, heap
//! allocation and peripheral mappings are still outside this bootstrap.

pub mod svc;
/// ARM11 per-thread contexts and cooperative scheduling primitives.
pub mod thread;
pub use thread::{
    Arm11ThreadScheduler, MAIN_THREAD_ID, ThreadId, ThreadInfo, ThreadSchedulerError, ThreadState,
};

use std::io::{Read, Seek};

use crate::Result;
use crate::cpu::{Arm11Core, StepEvent};
use crate::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use crate::loader::ncsd::process::Arm11CodeAddressSpace;
use crate::loader::ncsd::{NcsdImageReader, NcsdImageReaderError};
use svc::{
    Arm11SvcDispatcher, Arm11SvcDisposition, SVC_CREATE_THREAD, SVC_EXIT_THREAD,
    SVC_GET_THREAD_PRIORITY, SVC_SET_THREAD_PRIORITY, SVC_SLEEP_THREAD,
};

/// Main-thread priority used by the current process bootstrap.
pub const DEFAULT_MAIN_THREAD_PRIORITY: i32 = 0x30;

/// Nintendo 3DS result: invalid enum or argument value.
pub const RESULT_INVALID_ENUM: u32 = 0xD8E0_07ED;

/// Nintendo 3DS result: misaligned address.
pub const RESULT_MISALIGNED_ADDRESS: u32 = 0xD8E0_07F1;

/// Nintendo 3DS result: invalid or unknown handle.
pub const RESULT_INVALID_HANDLE: u32 = 0xD8E0_07F7;

/// Nintendo 3DS result: invalid user virtual address.
pub const RESULT_INVALID_ADDRESS: u32 = 0xE0E0_1BF5;

/// Nintendo 3DS result: no more free kernel synchronization/thread objects.
pub const RESULT_OUT_OF_THREAD_RESOURCES: u32 = 0xC860_1801;

/// Reason an instruction-budgeted run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm11RunStopReason {
    /// The requested instruction budget was consumed.
    InstructionBudgetExhausted,
    /// An SVC was reached but has no implemented handler.
    SupervisorCall { number: u32 },
    /// A process-scoped ExitProcess call was handled.
    ProcessExited,
    /// No thread is currently eligible to execute.
    NoRunnableThread,
    /// A void SVC could not be completed by the current bootstrap.
    KernelServiceError { number: u32, result: u32 },
}

/// Result of one call to [\`Arm11ProcessSession::run_instructions\`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm11RunReport {
    /// Instructions successfully executed in this call, including SVC instructions.
    pub instructions_executed: u64,
    /// Why execution stopped.
    pub stop_reason: Arm11RunStopReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SvcRunDisposition {
    Resume,
    ProcessExited,
    Unhandled(u32),
    NoRunnableThread,
    KernelServiceError { number: u32, result: u32 },
}

/// An initialized ARM11 process with an address space, per-thread CPU contexts,
/// a cooperative scheduler and a process-local SVC dispatcher.
#[derive(Debug)]
pub struct Arm11ProcessSession {
    scheduler: Arm11ThreadScheduler,
    address_space: Arm11CodeAddressSpace,
    svc_dispatcher: Arm11SvcDispatcher,
    instruction_count: u64,
}

impl Arm11ProcessSession {
    /// Creates a process session, initializes the main CPU context and scheduler.
    pub fn from_address_space(address_space: Arm11CodeAddressSpace) -> Self {
        let mut main_core = Arm11Core::new();
        address_space.initialize_core(&mut main_core);
        let scheduler = Arm11ThreadScheduler::new(main_core, DEFAULT_MAIN_THREAD_PRIORITY)
            .expect("the built-in main-thread priority is valid");

        Self {
            scheduler,
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

    /// Returns the currently running core, or the saved main-thread context
    /// when the process has no runnable thread.
    pub fn core(&self) -> &Arm11Core {
        self.scheduler
            .current_core()
            .or_else(|| self.scheduler.core(MAIN_THREAD_ID))
            .expect("the main thread context is retained for the process lifetime")
    }

    /// Returns the current core context when a thread is runnable.
    pub fn current_core(&self) -> Option<&Arm11Core> {
        self.scheduler.current_core()
    }

    /// Returns the validated process address space.
    pub const fn address_space(&self) -> &Arm11CodeAddressSpace {
        &self.address_space
    }

    /// Returns the currently selected thread identifier.
    pub const fn current_thread_id(&self) -> Option<ThreadId> {
        self.scheduler.current_thread_id()
    }

    /// Returns a thread's process-local handle information.
    pub fn thread_info(&self, id: ThreadId) -> Option<ThreadInfo> {
        self.scheduler.thread_info(id)
    }

    /// Returns whether ExitProcess has already terminated this session.
    pub const fn process_exited(&self) -> bool {
        self.svc_dispatcher.process_exited()
    }

    /// Returns the total number of successfully executed instructions.
    pub const fn instruction_count(&self) -> u64 {
        self.instruction_count
    }

    /// Executes one instruction on the currently selected thread.
    ///
    /// This low-level method reports SVC instructions to the caller without
    /// dispatching them. Use run_instructions for kernel-service processing.
    pub fn step(&mut self) -> Result<StepEvent> {
        let core = self
            .scheduler
            .current_core_mut()
            .ok_or(crate::Error::Unimplemented("no runnable ARM11 thread"))?;
        let event = core.step(&mut self.address_space)?;
        self.instruction_count = self.instruction_count.saturating_add(1);
        Ok(event)
    }

    /// Advances the virtual scheduler clock and wakes expired sleeping threads.
    ///
    /// This is an explicit virtual-time hook, not a claim that hardware timers
    /// or real-time scheduling are already emulated.
    pub fn advance_time(&mut self, nanoseconds: u64) -> Option<ThreadId> {
        self.scheduler.advance_time(nanoseconds)
    }

    /// Runs no more than instruction_budget instructions and dispatches supported SVCs.
    ///
    /// CreateThread validates the entrypoint and the top eight writable bytes
    /// of the supplied stack before creating a CPU context. The guest remains
    /// responsible for reserving a suitably sized, non-overlapping stack.
    ///
    /// A positive SleepThread duration blocks the current thread against the
    /// virtual clock; the caller advances that clock with advance_time.
    pub fn run_instructions(&mut self, instruction_budget: usize) -> Result<Arm11RunReport> {
        if self.svc_dispatcher.process_exited() {
            return Ok(Arm11RunReport {
                instructions_executed: 0,
                stop_reason: Arm11RunStopReason::ProcessExited,
            });
        }
        if instruction_budget == 0 {
            return Ok(Arm11RunReport {
                instructions_executed: 0,
                stop_reason: Arm11RunStopReason::InstructionBudgetExhausted,
            });
        }

        let mut executed = 0u64;
        for _ in 0..instruction_budget {
            if self.scheduler.current_thread_id().is_none() {
                return Ok(Arm11RunReport {
                    instructions_executed: executed,
                    stop_reason: Arm11RunStopReason::NoRunnableThread,
                });
            }

            match self.step()? {
                StepEvent::Executed => executed = executed.saturating_add(1),
                StepEvent::Svc(number) => {
                    executed = executed.saturating_add(1);
                    match self.dispatch_svc(number) {
                        SvcRunDisposition::Resume => {}
                        SvcRunDisposition::ProcessExited => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::ProcessExited,
                            });
                        }
                        SvcRunDisposition::Unhandled(number) => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::SupervisorCall { number },
                            });
                        }
                        SvcRunDisposition::NoRunnableThread => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::NoRunnableThread,
                            });
                        }
                        SvcRunDisposition::KernelServiceError { number, result } => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::KernelServiceError {
                                    number,
                                    result,
                                },
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

    fn dispatch_svc(&mut self, number: u32) -> SvcRunDisposition {
        let Some(caller) = self.scheduler.current_thread_id() else {
            return SvcRunDisposition::NoRunnableThread;
        };

        match number {
            SVC_CREATE_THREAD => self.svc_create_thread(caller),
            SVC_EXIT_THREAD => match self.scheduler.exit_current() {
                Ok(Some(_)) => SvcRunDisposition::Resume,
                Ok(None) => SvcRunDisposition::NoRunnableThread,
                Err(_) => SvcRunDisposition::KernelServiceError {
                    number,
                    result: RESULT_INVALID_ENUM,
                },
            },
            SVC_SLEEP_THREAD => {
                let registers = self
                    .scheduler
                    .core(caller)
                    .expect("current thread context exists")
                    .regs;
                let duration = (u64::from(registers[0]) | (u64::from(registers[1]) << 32)) as i64;
                match self.scheduler.sleep_current(duration) {
                    Ok(Some(_)) => SvcRunDisposition::Resume,
                    Ok(None) => SvcRunDisposition::NoRunnableThread,
                    Err(ThreadSchedulerError::InvalidSleepDuration(_)) => {
                        SvcRunDisposition::KernelServiceError {
                            number,
                            result: RESULT_INVALID_ENUM,
                        }
                    }
                    Err(_) => SvcRunDisposition::KernelServiceError {
                        number,
                        result: RESULT_INVALID_ENUM,
                    },
                }
            }
            SVC_GET_THREAD_PRIORITY => {
                let handle = self
                    .scheduler
                    .core(caller)
                    .expect("current thread context exists")
                    .regs[1];
                let result = self
                    .scheduler
                    .thread_id_for_handle(handle)
                    .and_then(|id| self.scheduler.priority(id).ok());
                let core = self
                    .scheduler
                    .core_mut(caller)
                    .expect("caller context remains allocated");
                match result {
                    Some(priority) => {
                        core.regs[0] = 0;
                        core.regs[1] = u32::from(priority);
                    }
                    None => core.regs[0] = RESULT_INVALID_HANDLE,
                }
                SvcRunDisposition::Resume
            }
            SVC_SET_THREAD_PRIORITY => {
                let registers = self
                    .scheduler
                    .core(caller)
                    .expect("current thread context exists")
                    .regs;
                let handle = registers[0];
                let priority = registers[1] as i32;
                let code = match self.scheduler.thread_id_for_handle(handle) {
                    Some(id) => self
                        .scheduler
                        .set_priority(id, priority)
                        .err()
                        .map(thread_scheduler_result_code)
                        .unwrap_or(0),
                    None => RESULT_INVALID_HANDLE,
                };
                self.scheduler
                    .core_mut(caller)
                    .expect("caller context remains allocated")
                    .regs[0] = code;
                SvcRunDisposition::Resume
            }
            _ => {
                let Some(core) = self.scheduler.current_core_mut() else {
                    return SvcRunDisposition::NoRunnableThread;
                };
                match self.svc_dispatcher.dispatch(number, &mut core.regs) {
                    Arm11SvcDisposition::Resume => SvcRunDisposition::Resume,
                    Arm11SvcDisposition::ProcessExited => SvcRunDisposition::ProcessExited,
                    Arm11SvcDisposition::Unhandled { number } => {
                        SvcRunDisposition::Unhandled(number)
                    }
                }
            }
        }
    }

    fn svc_create_thread(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs;
        let priority = registers[0] as i32;
        let entrypoint = registers[1];
        let argument = registers[2];
        let stack_top = registers[3];
        let processor_id = registers[4] as i32;

        let result = if stack_top == 0 || stack_top & 7 != 0 {
            Err(if stack_top == 0 {
                RESULT_INVALID_ADDRESS
            } else {
                RESULT_MISALIGNED_ADDRESS
            })
        } else if !self.address_space.is_executable_address(entrypoint) {
            Err(RESULT_INVALID_ADDRESS)
        } else {
            let Some(stack_start) = stack_top.checked_sub(8) else {
                return self.write_create_thread_error(caller, RESULT_INVALID_ADDRESS);
            };
            if !self.address_space.is_writable_range(stack_start, 8) {
                Err(RESULT_INVALID_ADDRESS)
            } else {
                self.scheduler
                    .create_thread(entrypoint, argument, stack_top, priority, processor_id)
                    .map_err(thread_scheduler_result_code)
            }
        };

        match result {
            Ok(id) => {
                let core = self
                    .scheduler
                    .core_mut(caller)
                    .expect("caller context remains allocated");
                core.regs[0] = 0;
                core.regs[1] = id.raw();
                SvcRunDisposition::Resume
            }
            Err(code) => self.write_create_thread_error(caller, code),
        }
    }

    fn write_create_thread_error(&mut self, caller: ThreadId, result: u32) -> SvcRunDisposition {
        self.scheduler
            .core_mut(caller)
            .expect("caller context remains allocated")
            .regs[0] = result;
        SvcRunDisposition::Resume
    }
}

fn thread_scheduler_result_code(error: ThreadSchedulerError) -> u32 {
    match error {
        ThreadSchedulerError::InvalidPriority(_)
        | ThreadSchedulerError::UnsupportedProcessorId(_)
        | ThreadSchedulerError::InvalidState { .. }
        | ThreadSchedulerError::InvalidSleepDuration(_)
        | ThreadSchedulerError::NoCurrentThread => RESULT_INVALID_ENUM,
        ThreadSchedulerError::UnalignedArmEntrypoint(_) => RESULT_MISALIGNED_ADDRESS,
        ThreadSchedulerError::InvalidStackPointer(0) => {
            RESULT_INVALID_ADDRESS
        }
        ThreadSchedulerError::InvalidStackPointer(_) => RESULT_MISALIGNED_ADDRESS,
        ThreadSchedulerError::ThreadIdExhausted => RESULT_OUT_OF_THREAD_RESOURCES,
        ThreadSchedulerError::UnknownThread(_) => RESULT_INVALID_HANDLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::ncsd::PreparedArm11Image;
    use crate::loader::ncsd::ncch::extended_header::{CodeSetInfo, NcchExtendedHeader};

    fn test_session_with_program_and_data(instructions: &[(usize, u32)]) -> Arm11ProcessSession {
        let mut text = vec![0u8; 0x1000];
        for (offset, instruction) in instructions.iter().copied() {
            text[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
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
                data_code_set: CodeSetInfo {
                    address: 0x0020_0000,
                    physical_size_pages: 2,
                    size_bytes: 0x2000,
                },
                bss_size_bytes: 0,
                dependencies: [0; 48],
                save_data_size_bytes: 0,
                jump_id: 0,
                access_control_info: [0; 0x200],
            },
            text,
            read_only: Vec::new(),
            data: vec![0; 0x2000],
        };
        let address_space = Arm11CodeAddressSpace::from_prepared(prepared).unwrap();
        Arm11ProcessSession::from_address_space(address_space)
    }

    fn test_session_with_program(instructions: &[u32]) -> Arm11ProcessSession {
        let entries = instructions
            .iter()
            .copied()
            .enumerate()
            .map(|(index, word)| (index * 4, word))
            .collect::<Vec<_>>();
        test_session_with_program_and_data(&entries)
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
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
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
    fn dispatches_processor_id_and_resumes_execution() {
        let mut session = test_session_with_program(&[
            0xE3A0_0007, // MOV r0, #7
            0xEF00_0011, // SVC #0x11 - GetProcessorID
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
    fn creates_starts_and_exits_a_guest_thread_through_svc() {
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEF00_0008), // SVC CreateThread
            (0x04, 0xE1A0_0001), // MOV r0, r1
            (0x08, 0xEF00_0009), // SVC StartThread
            (0x0C, 0xEF00_0009), // SVC ExitThread (main)
            (0x20, 0xE3A0_0009), // MOV r0, #9 (child)
            (0x24, 0xEF00_0009), // SVC ExitThread (child)
        ]);
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = 0x20;
            main.regs[1] = 0x0010_0020;
            main.regs[2] = 0xCAFE_BABE;
            main.regs[3] = 0x0020_2000;
            main.regs[4] = 0;
        }

        let created = session.run_instructions(1).unwrap();
        assert_eq!(
            created.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 0);
        let child_handle = session.core().regs[1];
        assert_eq!(child_handle, 2);
        assert_eq!(session.scheduler.thread_count(), 2);

        let started = session.run_instructions(2).unwrap();
        assert_eq!(
            started.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(
            session.current_thread_id(),
            Some(
                session
                    .scheduler
                    .thread_id_for_handle(child_handle)
                    .unwrap()
            )
        );
        assert_eq!(session.core().regs[0], 0xCAFE_BABE);

        let child_exit = session.run_instructions(2).unwrap();
        assert_eq!(
            child_exit.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        let main_exit = session.run_instructions(1).unwrap();
        assert_eq!(main_exit.stop_reason, Arm11RunStopReason::NoRunnableThread);
        assert_eq!(session.current_thread_id(), None);
    }

    #[test]
    fn create_thread_rejects_non_executable_entrypoints_and_unmapped_stacks() {
        let mut session = test_session_with_program(&[0xEF00_0008]);
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = 0x20;
            main.regs[1] = 0x0020_0000; // Writable data, not executable.
            main.regs[2] = 0;
            main.regs[3] = 0x0020_2000;
            main.regs[4] = 0;
        }
        let report = session.run_instructions(1).unwrap();
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], RESULT_INVALID_ADDRESS);
        assert_eq!(session.scheduler.thread_count(), 1);
    }

    #[test]
    fn get_and_set_thread_priority_use_the_documented_register_abi() {
        let mut session = test_session_with_program(&[
            0xEF00_000B, // SVC GetThreadPriority
            0xEF00_000C, // SVC SetThreadPriority
            0xEF00_000B, // SVC GetThreadPriority
        ]);
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[1] = MAIN_THREAD_ID.raw();
        }

        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], DEFAULT_MAIN_THREAD_PRIORITY as u32);

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = MAIN_THREAD_ID.raw();
            main.regs[1] = 0x10;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.scheduler.priority(MAIN_THREAD_ID).unwrap(), 0x10);

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[1] = MAIN_THREAD_ID.raw();
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], 0x10);
    }

    #[test]
    fn invalid_thread_handle_returns_the_documented_result_code() {
        let mut session = test_session_with_program(&[0xEF00_000B]);
        session.scheduler.current_core_mut().unwrap().regs[1] = 0xDEAD;
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], RESULT_INVALID_HANDLE);
    }

    #[test]
    fn sleeping_last_thread_leaves_no_runnable_context_until_time_advances() {
        let mut session = test_session_with_program(&[0xEF00_000A]);
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = 100;
            core.regs[1] = 0;
        }

        let report = session.run_instructions(4).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(report.stop_reason, Arm11RunStopReason::NoRunnableThread);

        assert_eq!(session.advance_time(99), None);
        assert_eq!(session.current_thread_id(), None);
        assert_eq!(session.advance_time(1), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Running
        );
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
