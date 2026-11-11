//! ARM11 process execution session.
//!
//! This module joins the validated NCSD loader, protected process memory,
//! independent ARM11 thread contexts, a cooperative scheduler and HLE SVCs.
//! Privileged CPU modes, blocking synchronization waits, timers, heap
//! resource accounting and peripheral mappings remain outside this bootstrap.

pub mod kernel;
pub mod svc;
/// ARM11 per-thread contexts and cooperative scheduling primitives.
pub mod thread;
pub use thread::{
    Arm11ThreadScheduler, MAIN_THREAD_ID, ThreadId, ThreadInfo, ThreadSchedulerError, ThreadState,
};

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use crate::Result;
use crate::cpu::{Arm11Core, StepEvent};
use crate::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use crate::loader::ncsd::process::{Arm11CodeAddressSpace, Arm11DynamicMemoryError};
use crate::loader::ncsd::{NcsdImageReader, NcsdImageReaderError};
use crate::memory::Bus;
use kernel::{Arm11KernelObjectTable, EventResetType, KernelHandleError};
use svc::{
    Arm11SvcDispatcher, Arm11SvcDisposition, SVC_CLEAR_EVENT, SVC_CLOSE_HANDLE, SVC_CONTROL_MEMORY,
    SVC_CREATE_EVENT, SVC_CREATE_MUTEX, SVC_CREATE_SEMAPHORE, SVC_CREATE_THREAD, SVC_EXIT_THREAD,
    SVC_GET_THREAD_PRIORITY, SVC_QUERY_MEMORY, SVC_RELEASE_MUTEX, SVC_RELEASE_SEMAPHORE,
    SVC_SET_THREAD_PRIORITY, SVC_SIGNAL_EVENT, SVC_SLEEP_THREAD, SVC_WAIT_SYNCHRONIZATION1,
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

/// Nintendo 3DS result: kernel application-memory allocation failed.
pub const RESULT_OUT_OF_MEMORY: u32 = 0xD860_07F3;

const MEMOP_FREE: u32 = 1;
const MEMOP_ALLOC: u32 = 3;
const MEMPERM_NONE: u32 = 0;
const MEMPERM_READ_WRITE: u32 = 3;

/// Nintendo 3DS result: no more free kernel synchronization/thread objects.
pub const RESULT_OUT_OF_THREAD_RESOURCES: u32 = 0xC860_1801;

/// Nintendo 3DS result: a synchronization wait timed out.
pub const RESULT_TIMEOUT: u32 = 0x0940_1BFE;

/// Reason an instruction-budgeted run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm11RunStopReason {
    /// The requested instruction budget was consumed.
    InstructionBudgetExhausted,
    /// An SVC was reached but has no implemented handler.
    SupervisorCall { number: u32 },
    /// A process-scoped ExitProcess call was handled.
    ProcessExited,
    /// A BKPT instruction stopped execution for a debugger or exception layer.
    Breakpoint(u16),
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
    kernel_objects: Arm11KernelObjectTable,
    /// Finite WaitSynchronization1 deadlines mapped to their object handles.
    wait_deadlines: BTreeMap<ThreadId, (u32, u64)>,
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
            kernel_objects: Arm11KernelObjectTable::default(),
            wait_deadlines: BTreeMap::new(),
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

    /// Resolves a guest-visible thread handle to its internal scheduler identifier.
    pub fn thread_id_for_handle(&self, handle: u32) -> Option<ThreadId> {
        let current = self.current_thread_id().unwrap_or(MAIN_THREAD_ID);
        self.kernel_objects.thread_id(handle, current)
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
        self.scheduler.advance_time(nanoseconds);
        let now = self.scheduler.virtual_time_ns();
        let expired = self
            .wait_deadlines
            .iter()
            .filter_map(|(thread, (_, deadline))| (*deadline <= now).then_some(*thread))
            .collect::<Vec<_>>();

        for thread in expired {
            if self.wait_deadlines.remove(&thread).is_some() {
                self.kernel_objects.remove_waiter(thread);
                if let Some(core) = self.scheduler.core_mut(thread) {
                    core.regs[0] = RESULT_TIMEOUT;
                }
                let _ = self.scheduler.wake_thread(thread);
            }
        }

        self.scheduler.current_thread_id()
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
                StepEvent::Breakpoint(immediate) => {
                    executed = executed.saturating_add(1);
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::Breakpoint(immediate),
                    });
                }
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
            SVC_CONTROL_MEMORY => self.svc_control_memory(caller),
            SVC_QUERY_MEMORY => self.svc_query_memory(caller),
            SVC_CREATE_THREAD => self.svc_create_thread(caller),
            SVC_CREATE_MUTEX => self.svc_create_mutex(caller),
            SVC_RELEASE_MUTEX => self.svc_release_mutex(caller),
            SVC_CREATE_SEMAPHORE => self.svc_create_semaphore(caller),
            SVC_RELEASE_SEMAPHORE => self.svc_release_semaphore(caller),
            SVC_WAIT_SYNCHRONIZATION1 => self.svc_wait_synchronization1(caller),
            SVC_CREATE_EVENT => self.svc_create_event(caller),
            SVC_SIGNAL_EVENT => self.svc_signal_event(caller),
            SVC_CLEAR_EVENT => self.svc_clear_event(caller),
            SVC_CLOSE_HANDLE => self.svc_close_handle(caller),
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
                let code = match self.thread_id_for_handle(handle) {
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

    /// Handles QueryMemory: r2 is the queried address; r0-r5 receive result and region.
    fn svc_query_memory(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let address = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[2];
        let region = self.address_space.query_memory(address);
        let core = self
            .scheduler
            .core_mut(caller)
            .expect("caller context remains allocated");
        core.regs[0] = 0;
        core.regs[1] = region.base_address;
        core.regs[2] = region.size_bytes;
        core.regs[3] = region.permissions;
        core.regs[4] = region.state as u32;
        core.regs[5] = region.page_flags;
        SvcRunDisposition::Resume
    }

    /// Handles the raw ARM11 ControlMemory SVC register ABI for ordinary
    /// RW application-heap allocations and exact frees. Shared, linear,
    /// mirrored mappings and executable permissions are not implemented.
    ///
    /// The SVC enters with operation in r0, addr0 in r1, addr1 in r2, size in
    /// r3 and permissions in r4. The result is returned in r0 and addr_out in r1.
    fn svc_control_memory(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs;
        let operation = registers[0];
        let address = registers[1];
        let source_address = registers[2];
        let size = registers[3];
        let permissions = registers[4];

        let result = match (operation, source_address, permissions) {
            (MEMOP_ALLOC, 0, MEMPERM_READ_WRITE) => self
                .address_space
                .allocate_application_heap_region(address, size)
                .map_err(dynamic_memory_result_code),
            (MEMOP_FREE, 0, MEMPERM_NONE) => self
                .address_space
                .free_application_heap_region(address, size)
                .map(|()| address)
                .map_err(dynamic_memory_result_code),
            _ => Err(RESULT_INVALID_ENUM),
        };

        let core = self
            .scheduler
            .core_mut(caller)
            .expect("caller context remains allocated");
        match result {
            Ok(output_address) => {
                core.regs[0] = 0;
                core.regs[1] = output_address;
            }
            Err(code) => core.regs[0] = code,
        }
        SvcRunDisposition::Resume
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
                let handle = match self.kernel_objects.insert_thread(id) {
                    Ok(handle) => handle,
                    Err(KernelHandleError::Exhausted) => {
                        return self
                            .write_create_thread_error(caller, RESULT_OUT_OF_THREAD_RESOURCES);
                    }
                    Err(_) => {
                        return self.write_create_thread_error(caller, RESULT_INVALID_HANDLE);
                    }
                };
                let core = self
                    .scheduler
                    .core_mut(caller)
                    .expect("caller context remains allocated");
                core.regs[0] = 0;
                core.regs[1] = handle;
                SvcRunDisposition::Resume
            }
            Err(code) => self.write_create_thread_error(caller, code),
        }
    }

    /// Handles CreateMutex: r0 points to the output handle and r1 is initialLocked.
    fn svc_create_mutex(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let output_address = registers[0];
        let initially_locked = registers[1];
        if initially_locked > 1 {
            return self.write_svc_result(caller, RESULT_INVALID_ENUM);
        }
        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        let handle = match self
            .kernel_objects
            .insert_mutex(initially_locked != 0, caller)
        {
            Ok(handle) => handle,
            Err(error) => return self.write_svc_result(caller, kernel_handle_result_code(error)),
        };
        if self.address_space.write32(output_address, handle).is_err() {
            let _ = self.kernel_objects.close_handle(handle);
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        self.write_svc_result(caller, 0)
    }

    /// Handles ReleaseMutex using the handle in r0.
    fn svc_release_mutex(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs[0];
        match self.kernel_objects.release_mutex(handle, caller) {
            Ok(Some(waiter)) => {
                self.wake_synchronization_waiters([waiter]);
                self.write_svc_result(caller, 0)
            }
            Ok(None) => self.write_svc_result(caller, 0),
            Err(error) => self.write_svc_result(caller, kernel_handle_result_code(error)),
        }
    }

    /// Handles CreateSemaphore: r0 is output pointer, r1 initial count, r2 maximum.
    fn svc_create_semaphore(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let output_address = registers[0];
        let initial_count = registers[1] as i32;
        let maximum_count = registers[2] as i32;
        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        let handle = match self
            .kernel_objects
            .insert_semaphore(initial_count, maximum_count)
        {
            Ok(handle) => handle,
            Err(error) => return self.write_svc_result(caller, kernel_handle_result_code(error)),
        };
        if self.address_space.write32(output_address, handle).is_err() {
            let _ = self.kernel_objects.close_handle(handle);
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        self.write_svc_result(caller, 0)
    }

    /// Handles ReleaseSemaphore: r0 output pointer, r1 handle, r2 release count.
    fn svc_release_semaphore(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let output_address = registers[0];
        let handle = registers[1];
        let release_count = registers[2];
        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        let awakened = match self.kernel_objects.release_semaphore(handle, release_count) {
            Ok(awakened) => awakened,
            Err(error) => return self.write_svc_result(caller, kernel_handle_result_code(error)),
        };
        let awakened_count = awakened.len() as u32;
        self.wake_synchronization_waiters(awakened);
        if self
            .address_space
            .write32(output_address, awakened_count)
            .is_err()
        {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        self.write_svc_result(caller, 0)
    }

    /// Handles WaitSynchronization1 for immediate and infinite waits.
    ///
    /// r0 is the synchronization handle and r2-r3 contain the signed 64-bit
    /// timeout in nanoseconds. A finite positive timeout is not scheduled yet.
    fn svc_wait_synchronization1(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let handle = registers[0];
        let timeout = (u64::from(registers[2]) | (u64::from(registers[3]) << 32)) as i64;

        match self.kernel_objects.try_wait_synchronization(handle, caller) {
            Ok(true) => return self.write_svc_result(caller, 0),
            Ok(false) => {}
            Err(error) => {
                return self.write_svc_result(caller, kernel_handle_result_code(error));
            }
        }

        if timeout == 0 {
            return self.write_svc_result(caller, RESULT_TIMEOUT);
        }
        if timeout < -1 {
            return self.write_svc_result(caller, RESULT_INVALID_ENUM);
        }
        if let Err(error) = self.kernel_objects.enqueue_waiter(handle, caller) {
            return self.write_svc_result(caller, kernel_handle_result_code(error));
        }
        if timeout > 0 {
            let deadline = self
                .scheduler
                .virtual_time_ns()
                .saturating_add(timeout as u64);
            self.wait_deadlines.insert(caller, (handle, deadline));
        }

        match self.scheduler.wait_current() {
            Ok(Some(_)) => SvcRunDisposition::Resume,
            Ok(None) => SvcRunDisposition::NoRunnableThread,
            Err(_) => {
                self.wait_deadlines.remove(&caller);
                self.kernel_objects.remove_waiter(caller);
                SvcRunDisposition::KernelServiceError {
                    number: SVC_WAIT_SYNCHRONIZATION1,
                    result: RESULT_INVALID_ENUM,
                }
            }
        }
    }

    /// Completes an object handoff and makes its blocked thread(s) schedulable.
    fn wake_synchronization_waiters(&mut self, waiters: impl IntoIterator<Item = ThreadId>) {
        for waiter in waiters {
            self.wait_deadlines.remove(&waiter);
            if let Some(core) = self.scheduler.core_mut(waiter) {
                core.regs[0] = 0;
            }
            let _ = self.scheduler.wake_thread(waiter);
        }
    }

    /// Handles CreateEvent: r0 is the writable guest pointer for the handle,
    /// r1 is ResetType (0 = one-shot, 1 = sticky).
    fn svc_create_event(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs;
        let output_address = registers[0];
        let reset_type = match EventResetType::from_raw(registers[1]) {
            Some(reset_type) => reset_type,
            None => return self.write_svc_result(caller, RESULT_INVALID_ENUM),
        };

        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        let handle = match self.kernel_objects.insert_event(reset_type) {
            Ok(handle) => handle,
            Err(KernelHandleError::Exhausted) => {
                return self.write_svc_result(caller, RESULT_OUT_OF_THREAD_RESOURCES);
            }
            Err(_) => return self.write_svc_result(caller, RESULT_INVALID_HANDLE),
        };

        if self.address_space.write32(output_address, handle).is_err() {
            let _ = self.kernel_objects.close_handle(handle);
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        self.write_svc_result(caller, 0)
    }

    /// Handles SignalEvent using the event handle in r0.
    fn svc_signal_event(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[0];
        let awakened = match self.kernel_objects.signal_event(handle) {
            Ok(awakened) => awakened,
            Err(error) => return self.write_svc_result(caller, kernel_handle_result_code(error)),
        };
        self.wake_synchronization_waiters(awakened);
        self.write_svc_result(caller, 0)
    }

    /// Handles ClearEvent using the event handle in r0.
    fn svc_clear_event(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[0];
        let result = self
            .kernel_objects
            .clear_event(handle)
            .map(|()| 0)
            .unwrap_or_else(kernel_handle_result_code);
        self.write_svc_result(caller, result)
    }

    /// Handles CloseHandle using the process-local object handle in r0.
    fn svc_close_handle(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[0];
        let result = self
            .kernel_objects
            .close_handle(handle)
            .map(|()| 0)
            .unwrap_or_else(kernel_handle_result_code);
        self.write_svc_result(caller, result)
    }

    fn write_svc_result(&mut self, caller: ThreadId, result: u32) -> SvcRunDisposition {
        self.scheduler
            .core_mut(caller)
            .expect("caller context remains allocated")
            .regs[0] = result;
        SvcRunDisposition::Resume
    }

    fn write_create_thread_error(&mut self, caller: ThreadId, result: u32) -> SvcRunDisposition {
        self.scheduler
            .core_mut(caller)
            .expect("caller context remains allocated")
            .regs[0] = result;
        SvcRunDisposition::Resume
    }
}

fn kernel_handle_result_code(error: KernelHandleError) -> u32 {
    match error {
        KernelHandleError::Exhausted => RESULT_OUT_OF_THREAD_RESOURCES,
        KernelHandleError::InvalidHandle
        | KernelHandleError::WrongType
        | KernelHandleError::NotOwner
        | KernelHandleError::WaitersPending => RESULT_INVALID_HANDLE,
        KernelHandleError::InvalidCount | KernelHandleError::CountLimitExceeded => {
            RESULT_INVALID_ENUM
        }
    }
}

fn dynamic_memory_result_code(error: Arm11DynamicMemoryError) -> u32 {
    match error {
        Arm11DynamicMemoryError::MisalignedAddress(_) | Arm11DynamicMemoryError::InvalidSize(_) => {
            RESULT_MISALIGNED_ADDRESS
        }
        Arm11DynamicMemoryError::AllocationFailure(_) => RESULT_OUT_OF_MEMORY,
        Arm11DynamicMemoryError::OutsideApplicationHeap { .. }
        | Arm11DynamicMemoryError::OverlappingMapping
        | Arm11DynamicMemoryError::MappingNotFound { .. } => RESULT_INVALID_ADDRESS,
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
        ThreadSchedulerError::InvalidStackPointer(0) => RESULT_INVALID_ADDRESS,
        ThreadSchedulerError::InvalidStackPointer(_) => RESULT_MISALIGNED_ADDRESS,
        ThreadSchedulerError::ThreadIdExhausted => RESULT_OUT_OF_THREAD_RESOURCES,
        ThreadSchedulerError::UnknownThread(_) => RESULT_INVALID_HANDLE,
    }
}

#[cfg(test)]
mod tests {
    use super::kernel::CURRENT_THREAD_HANDLE;
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
    fn thumb_breakpoint_stops_the_session_with_its_immediate() {
        let mut session = test_session_with_program(&[0x0000_BE5A]);
        session
            .scheduler
            .core_mut(MAIN_THREAD_ID)
            .unwrap()
            .cpsr
            .set_thumb(true);

        let report = session.run_instructions(10).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(report.stop_reason, Arm11RunStopReason::Breakpoint(0x5A));
        assert_eq!(session.instruction_count(), 1);
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
    fn query_memory_reports_text_permissions_and_free_gaps() {
        let mut session = test_session_with_program(&[0xEF00_0002]);
        session.scheduler.current_core_mut().unwrap().regs[2] = 0x0010_0000;

        session.dispatch_svc(SVC_QUERY_MEMORY);
        let registers = session.core().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0x0010_0000);
        assert_eq!(registers[2], 0x1000);
        assert_eq!(registers[3], 5);
        assert_eq!(registers[4], 4);
        assert_eq!(registers[5], 0);

        session.scheduler.current_core_mut().unwrap().regs[2] = 0x0018_0000;
        session.dispatch_svc(SVC_QUERY_MEMORY);
        let registers = session.core().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0x0010_1000);
        assert_eq!(registers[2], 0x000F_F000);
        assert_eq!(registers[3], 0);
        assert_eq!(registers[4], 0);
    }

    #[test]
    fn query_memory_reports_private_heap_permissions_after_control_memory() {
        let mut session = test_session_with_program(&[0xEF00_0002]);
        session
            .address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();
        session.scheduler.current_core_mut().unwrap().regs[2] = 0x0800_1000;

        session.dispatch_svc(SVC_QUERY_MEMORY);
        let registers = session.core().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0x0800_0000);
        assert_eq!(registers[2], 0x2000);
        assert_eq!(registers[3], 3);
        assert_eq!(registers[4], 5);
    }

    #[test]
    fn control_memory_allocates_and_frees_application_heap_through_raw_svc_registers() {
        let mut session = test_session_with_program(&[
            0xEF00_0001, // SVC ControlMemory
            0xEF00_0001, // SVC ControlMemory (free)
        ]);
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC;
            core.regs[1] = 0x0800_0000; // addr0
            core.regs[2] = 0; // addr1
            core.regs[3] = 0x2000; // 8 KiB
            core.regs[4] = MEMPERM_READ_WRITE;
        }

        let allocated = session.run_instructions(1).unwrap();
        assert_eq!(
            allocated.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], 0x0800_0000);
        assert!(session.address_space.is_writable_range(0x0800_0000, 0x2000));

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_FREE;
            core.regs[1] = 0x0800_0000;
            core.regs[2] = 0;
            core.regs[3] = 0x2000;
            core.regs[4] = MEMPERM_NONE;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], 0x0800_0000);
        assert!(!session.address_space.is_writable_range(0x0800_0000, 1));
    }

    #[test]
    fn control_memory_heap_can_supply_a_secondary_thread_stack() {
        let mut session = test_session_with_program(&[
            0xEF00_0001, // SVC ControlMemory - allocate heap
            0xEF00_0008, // SVC CreateThread using the heap as its stack
        ]);
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC;
            core.regs[1] = 0x0800_0000;
            core.regs[2] = 0;
            core.regs[3] = 0x4000;
            core.regs[4] = MEMPERM_READ_WRITE;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        let heap_base = session.core().regs[1];
        assert_eq!(heap_base, 0x0800_0000);

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = 0x20;
            core.regs[1] = 0x0010_0020;
            core.regs[2] = 0xCAFE_BABE;
            core.regs[3] = heap_base + 0x4000;
            core.regs[4] = 0;
        }
        session.run_instructions(1).unwrap();

        assert_eq!(session.scheduler.thread_count(), 2);
        assert_eq!(session.current_thread_id().unwrap().raw(), 2);
    }

    #[test]
    fn control_memory_rejects_executable_permissions() {
        let mut session = test_session_with_program(&[0xEF00_0001]);
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC;
            core.regs[1] = 0x0800_0000;
            core.regs[2] = 0;
            core.regs[3] = 0x1000;
            core.regs[4] = MEMPERM_READ_WRITE | 4;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], RESULT_INVALID_ENUM);
        assert!(!session.address_space.is_writable_range(0x0800_0000, 1));
    }

    #[test]
    fn create_thread_preempts_immediately_and_exit_thread_restores_the_main_context() {
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEF00_0008), // SVC CreateThread
            (0x04, 0xE1A0_0001), // MOV r0, r1 (main resumes here)
            (0x08, 0xEF00_0009), // SVC ExitThread (main)
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
        let main_context = session.scheduler.core(MAIN_THREAD_ID).unwrap();
        assert_eq!(main_context.regs[0], 0);
        let child_handle = main_context.regs[1];
        assert_ne!(child_handle, 2);
        assert_eq!(session.scheduler.thread_count(), 2);
        let child_id = session.thread_id_for_handle(child_handle).unwrap();
        assert_eq!(session.current_thread_id(), Some(child_id));
        assert_eq!(session.core().regs[0], 0xCAFE_BABE);
        assert_eq!(
            session.thread_info(child_id).unwrap().state,
            ThreadState::Running
        );

        let child_exit = session.run_instructions(2).unwrap();
        assert_eq!(
            child_exit.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(child_id).unwrap().state,
            ThreadState::Exited
        );

        let main_exit = session.run_instructions(2).unwrap();
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
            main.regs[1] = CURRENT_THREAD_HANDLE;
        }

        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], DEFAULT_MAIN_THREAD_PRIORITY as u32);

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = CURRENT_THREAD_HANDLE;
            main.regs[1] = 0x10;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.scheduler.priority(MAIN_THREAD_ID).unwrap(), 0x10);

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[1] = CURRENT_THREAD_HANDLE;
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
    fn mutex_svcs_create_release_and_transfer_ownership_to_waiter() {
        let mut session = test_session_with_program(&[
            0xEF00_0013, // SVC CreateMutex
            0xEF00_0024, // SVC WaitSynchronization1
            0xEF00_0014, // SVC ReleaseMutex
        ]);
        let main = MAIN_THREAD_ID;

        {
            let core = session.scheduler.core_mut(main).unwrap();
            core.regs[0] = 0x0020_0000;
            core.regs[1] = 1;
        }
        session.dispatch_svc(SVC_CREATE_MUTEX);
        assert_eq!(session.scheduler.core(main).unwrap().regs[0], 0);
        let handle = session.address_space.read32(0x0020_0000).unwrap();
        assert_eq!(session.kernel_objects.mutex_owner(handle), Some(main));

        // A higher-priority child becomes current and waits on the owned mutex.
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        assert_eq!(session.current_thread_id(), Some(child));
        {
            let core = session.scheduler.core_mut(child).unwrap();
            core.regs[0] = handle;
            core.regs[2] = u32::MAX;
            core.regs[3] = u32::MAX;
        }
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );
        assert_eq!(session.current_thread_id(), Some(main));

        // The owner releases; the mutex is handed directly to the FIFO waiter.
        session.scheduler.core_mut(main).unwrap().regs[0] = handle;
        session.dispatch_svc(SVC_RELEASE_MUTEX);

        assert_eq!(session.kernel_objects.mutex_owner(handle), Some(child));
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], 0);
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Running
        );
    }

    #[test]
    fn finite_wait_timeout_wakes_thread_and_removes_it_from_object_queue() {
        let mut session = test_session_with_program(&[0xEF00_0024]);
        let event = session.kernel_objects.insert_event(EventResetType::OneShot).unwrap();
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        assert_eq!(session.current_thread_id(), Some(child));

        {
            let core = session.scheduler.core_mut(child).unwrap();
            core.regs[0] = event;
            core.regs[2] = 100;
            core.regs[3] = 0;
        }
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );

        session.advance_time(99);
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], event);

        session.advance_time(1);
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Running
        );
        assert_eq!(
            session.scheduler.core(child).unwrap().regs[0],
            RESULT_TIMEOUT
        );
        assert_eq!(session.kernel_objects.close_handle(event), Ok(()));
    }

    #[test]
    fn successful_object_signal_cancels_a_pending_finite_timeout() {
        let mut session = test_session_with_program(&[0xEF00_0024]);
        let event = session.kernel_objects.insert_event(EventResetType::OneShot).unwrap();
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();

        {
            let core = session.scheduler.core_mut(child).unwrap();
            core.regs[0] = event;
            core.regs[2] = 100;
            core.regs[3] = 0;
        }
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        let awakened = session.kernel_objects.signal_event(event).unwrap();
        session.wake_synchronization_waiters(awakened);
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], 0);

        session.advance_time(100);
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], 0);
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Running
        );
    }

    #[test]
    fn semaphore_svcs_validate_create_release_and_wait_counts() {
        let mut session = test_session_with_program(&[0xEF00_0015, 0xEF00_0016]);
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = 0x0020_0000;
            core.regs[1] = 1;
            core.regs[2] = 2;
        }
        session.dispatch_svc(SVC_CREATE_SEMAPHORE);
        assert_eq!(session.core().regs[0], 0);
        let handle = session.address_space.read32(0x0020_0000).unwrap();
        assert_eq!(
            session.kernel_objects.semaphore_counts(handle),
            Some((1, 2))
        );

        assert_eq!(
            session
                .kernel_objects
                .try_wait_synchronization(handle, MAIN_THREAD_ID),
            Ok(true)
        );
        assert_eq!(
            session.kernel_objects.semaphore_counts(handle),
            Some((0, 2))
        );

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = 0x0020_0004;
            core.regs[1] = handle;
            core.regs[2] = 1;
        }
        session.dispatch_svc(SVC_RELEASE_SEMAPHORE);
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.address_space.read32(0x0020_0004).unwrap(), 0);
        assert_eq!(
            session.kernel_objects.semaphore_counts(handle),
            Some((1, 2))
        );
    }

    #[test]
    fn wait_synchronization1_supports_immediate_timeout_and_infinite_wait_handoff() {
        let mut session = test_session_with_program(&[0xEF00_0024]);
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x3F, 0)
            .unwrap();

        {
            let core = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            core.regs[0] = event;
            core.regs[2] = 0;
            core.regs[3] = 0;
        }
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_TIMEOUT
        );

        {
            let core = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            core.regs[0] = event;
            core.regs[2] = u32::MAX;
            core.regs[3] = u32::MAX;
        }
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(
            session.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Waiting
        );

        session.scheduler.core_mut(child).unwrap().regs[0] = event;
        session.dispatch_svc(SVC_SIGNAL_EVENT);
        assert_eq!(session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0], 0);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Running
        );
    }

    #[test]
    fn create_event_writes_a_process_local_handle_and_preserves_reset_mode() {
        let mut session = test_session_with_program(&[0xEF00_0017]);
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = 0x0020_0000;
            main.regs[1] = 1; // RESET_STICKY
        }

        session.run_instructions(1).unwrap();

        assert_eq!(session.core().regs[0], 0);
        let handle = session.address_space.read32(0x0020_0000).unwrap();
        assert_ne!(handle, 0);
        assert_eq!(
            session.kernel_objects.event_reset_type(handle),
            Some(EventResetType::Sticky)
        );
        assert_eq!(
            session.kernel_objects.event_is_signaled(handle),
            Some(false)
        );
    }

    #[test]
    fn event_signal_clear_and_close_svcs_update_the_typed_handle_table() {
        let mut session = test_session_with_program(&[
            0xEF00_0018, // SVC SignalEvent
            0xEF00_0019, // SVC ClearEvent
            0xEF00_0023, // SVC CloseHandle
            0xEF00_0019, // SVC ClearEvent after close
        ]);
        let handle = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        session.scheduler.current_core_mut().unwrap().regs[0] = handle;

        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.kernel_objects.event_is_signaled(handle), Some(true));

        session.scheduler.current_core_mut().unwrap().regs[0] = handle;
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(
            session.kernel_objects.event_is_signaled(handle),
            Some(false)
        );

        session.scheduler.current_core_mut().unwrap().regs[0] = handle;
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.kernel_objects.event_is_signaled(handle), None);

        session.scheduler.current_core_mut().unwrap().regs[0] = handle;
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], RESULT_INVALID_HANDLE);
    }

    #[test]
    fn create_event_rejects_invalid_reset_modes_and_unwritable_output_pointers() {
        let mut invalid_mode = test_session_with_program(&[0xEF00_0017]);
        {
            let main = invalid_mode.scheduler.current_core_mut().unwrap();
            main.regs[0] = 0x0020_0000;
            main.regs[1] = 2; // PULSE is timer-only in the current event model.
        }
        invalid_mode.run_instructions(1).unwrap();
        assert_eq!(invalid_mode.core().regs[0], RESULT_INVALID_ENUM);
        assert_eq!(invalid_mode.kernel_objects.handle_count(), 0);

        let mut invalid_pointer = test_session_with_program(&[0xEF00_0017]);
        {
            let main = invalid_pointer.scheduler.current_core_mut().unwrap();
            main.regs[0] = 0xDEAD;
            main.regs[1] = 0;
        }
        invalid_pointer.run_instructions(1).unwrap();
        assert_eq!(invalid_pointer.core().regs[0], RESULT_INVALID_ADDRESS);
        assert_eq!(invalid_pointer.kernel_objects.handle_count(), 0);
    }

    #[test]
    fn thread_svc_uses_opaque_thread_handles_and_current_thread_pseudo_handle() {
        let mut session = test_session_with_program(&[
            0xEF00_0008, // SVC CreateThread
            0xEF00_000B, // SVC GetThreadPriority
        ]);
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = 0x3F;
            main.regs[1] = 0x0010_0020;
            main.regs[2] = 0xCAFE_BABE;
            main.regs[3] = 0x0020_2000;
            main.regs[4] = 0;
        }

        session.run_instructions(1).unwrap();
        let child_handle = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[1];
        assert_ne!(child_handle, 2);
        let child_id = session.thread_id_for_handle(child_handle).unwrap();
        assert_eq!(session.thread_info(child_id).unwrap().priority, 0x3F);

        session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs[1] = CURRENT_THREAD_HANDLE;
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], DEFAULT_MAIN_THREAD_PRIORITY as u32);
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
