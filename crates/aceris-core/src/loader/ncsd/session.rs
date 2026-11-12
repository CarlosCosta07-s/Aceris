//! ARM11 process execution session.
//!
//! This module joins the validated NCSD loader, protected process memory,
//! independent ARM11 thread contexts, a cooperative scheduler and HLE SVCs.
//! Privileged CPU modes, blocking synchronization waits, timers, heap
//! resource accounting and peripheral mappings remain outside this bootstrap.

pub mod gsp_service;
pub mod ipc;
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
use crate::gpu::{
    FCRAM_BASE, GpuDmaAddressSpace, GpuPhysicalMemory, NEW_LINEAR_BASE, NEW_LINEAR_END,
    OLD_LINEAR_BASE, OLD_LINEAR_END,
};
use crate::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use crate::loader::ncsd::ncch::extended_header::Arm11MemoryType;
use crate::loader::ncsd::process::{Arm11CodeAddressSpace, Arm11DynamicMemoryError};
use crate::loader::ncsd::{NcsdImageReader, NcsdImageReaderError};
use crate::memory::Bus;
use kernel::{Arm11KernelObjectTable, EventResetType, KernelHandleError};
use svc::{
    Arm11SvcDispatcher, Arm11SvcDisposition, SVC_CANCEL_TIMER, SVC_CLEAR_EVENT, SVC_CLEAR_TIMER,
    SVC_CLOSE_HANDLE, SVC_CONTROL_MEMORY, SVC_CREATE_EVENT, SVC_CREATE_MEMORY_BLOCK,
    SVC_CREATE_MUTEX, SVC_CREATE_SEMAPHORE, SVC_CREATE_THREAD, SVC_CREATE_TIMER, SVC_EXIT_THREAD,
    SVC_GET_THREAD_PRIORITY, SVC_MAP_MEMORY_BLOCK, SVC_QUERY_MEMORY, SVC_RELEASE_MUTEX,
    SVC_RELEASE_SEMAPHORE, SVC_SET_THREAD_PRIORITY, SVC_SET_TIMER, SVC_SIGNAL_EVENT,
    SVC_SLEEP_THREAD, SVC_UNMAP_MEMORY_BLOCK, SVC_WAIT_SYNCHRONIZATION1,
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

/// Nintendo 3DS result: anonymous shared memory is forbidden for APPLICATION processes.
pub const RESULT_MEMORY_BLOCK_NOT_ALLOWED: u32 = 0xD920_07EA;

const MEMOP_FREE: u32 = 1;
const MEMOP_ALLOC: u32 = 3;
const MEMOP_LINEAR: u32 = 0x0001_0000;
const OLD_LINEAR_VIRTUAL_TO_PHYSICAL_OFFSET: u32 = 0x0C00_0000;
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
    /// The current core executed CP15 WFI and is waiting for an IRQ or FIQ.
    WaitForInterrupt,
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
    gpu_memory: Option<GpuPhysicalMemory>,
    gpu_address_space: Option<GpuDmaAddressSpace>,
    instruction_count: u64,
}

impl Arm11ProcessSession {
    /// Creates a process session, initializes the main CPU context and scheduler.
    pub fn from_address_space(mut address_space: Arm11CodeAddressSpace) -> Self {
        let mut main_core = Arm11Core::new();
        address_space.initialize_core(&mut main_core);
        main_core.enable_vfp();
        let main_tls = address_space
            .allocate_thread_local_storage()
            .expect("a fresh address space has room for the main thread's TLS");
        main_core.cp15_mut().set_tpidruro(main_tls);
        let scheduler = Arm11ThreadScheduler::new(main_core, DEFAULT_MAIN_THREAD_PRIORITY)
            .expect("the built-in main-thread priority is valid");

        Self {
            scheduler,
            address_space,
            svc_dispatcher: Arm11SvcDispatcher::default(),
            kernel_objects: Arm11KernelObjectTable::default(),
            wait_deadlines: BTreeMap::new(),
            gpu_memory: None,
            gpu_address_space: None,
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

    /// Connects this process to the shared physical memory and GPU DMA map.
    ///
    /// Cloned handles share their backing state, so ARM11 loads/stores, GPU
    /// memory reads and LCD scanout observe the same physical FCRAM bytes.
    pub fn attach_gpu_services(
        &mut self,
        memory: GpuPhysicalMemory,
        address_space: GpuDmaAddressSpace,
    ) {
        self.address_space.attach_gpu_memory(memory.clone());
        self.gpu_memory = Some(memory);
        self.gpu_address_space = Some(address_space);
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

    /// Reads a guest word through the process's permission-aware bus.
    pub fn read_memory_u32(&mut self, address: u32) -> Result<u32> {
        self.address_space.read32(address)
    }

    /// Writes a guest word through the process's permission-aware bus.
    pub fn write_memory_u32(&mut self, address: u32, value: u32) -> Result<()> {
        self.address_space.write32(address, value)
    }

    /// Reads a byte range through the permission-aware ARM11 process bus.
    pub fn read_memory_bytes(&mut self, address: u32, output: &mut [u8]) -> Result<()> {
        for (index, byte) in output.iter_mut().enumerate() {
            let offset =
                u32::try_from(index).map_err(|_| crate::Error::InvalidAccess { addr: address })?;
            let current = address
                .checked_add(offset)
                .ok_or(crate::Error::InvalidAccess { addr: address })?;
            *byte = self.address_space.read8(current)?;
        }
        Ok(())
    }

    /// Writes a byte range through the permission-aware ARM11 process bus.
    ///
    /// The complete range is validated before the first byte is changed, which
    /// prevents a malformed shared-memory header from causing a partial write.
    pub fn write_memory_bytes(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        if !self.address_space.is_writable_range(address, bytes.len()) {
            return Err(crate::Error::InvalidAccess { addr: address });
        }
        for (index, byte) in bytes.iter().copied().enumerate() {
            let offset =
                u32::try_from(index).map_err(|_| crate::Error::InvalidAccess { addr: address })?;
            let current = address
                .checked_add(offset)
                .ok_or(crate::Error::InvalidAccess { addr: address })?;
            self.address_space.write8(current, byte)?;
        }
        Ok(())
    }

    /// Creates a zero-filled, page-aligned shared-memory block in the process kernel table.
    pub fn create_memory_block(
        &mut self,
        size: u32,
        owner_permissions: u32,
        other_permissions: u32,
    ) -> std::result::Result<u32, KernelHandleError> {
        self.kernel_objects
            .create_memory_block(size, owner_permissions, other_permissions)
    }

    /// Imports a copyable shared-memory handle from another process session.
    ///
    /// The backing is cloned by reference, never by copying its bytes. The
    /// owner/peer permission pair is reversed for the receiving process so
    /// my_permissions always refers to the current holder in MapMemoryBlock.
    /// This is the object-table primitive for IPC handle translation; it does
    /// not decode IPC descriptors or implement MOVE_HANDLE semantics.
    pub fn import_memory_block_handle(
        &mut self,
        source: &Arm11ProcessSession,
        source_handle: u32,
    ) -> std::result::Result<u32, KernelHandleError> {
        let backing = source.kernel_objects.memory_block_backing(source_handle)?;
        let (source_owner_permissions, source_peer_permissions) = source
            .kernel_objects
            .memory_block_permissions(source_handle)
            .ok_or(KernelHandleError::WrongType)?;

        self.kernel_objects.create_memory_block_with_backing(
            backing,
            source_peer_permissions,
            source_owner_permissions,
        )
    }

    /// Moves a shared-memory handle from another process session into this one.
    ///
    /// The destination receives the same backing and reversed owner/peer
    /// permissions, then the source handle is closed. If source closure fails,
    /// the newly created destination handle is closed to avoid a partial move.
    /// This is limited to memory blocks; IPC descriptor decoding and transfer
    /// of other kernel object types remain separate work.
    pub fn move_memory_block_handle(
        &mut self,
        source: &mut Arm11ProcessSession,
        source_handle: u32,
    ) -> std::result::Result<u32, KernelHandleError> {
        let destination_handle = self.import_memory_block_handle(source, source_handle)?;
        if let Err(error) = source.kernel_objects.close_handle(source_handle) {
            let _ = self.kernel_objects.close_handle(destination_handle);
            return Err(error);
        }
        Ok(destination_handle)
    }

    /// Applies a decoded IPC type-0 handle translation for one memory block.
    ///
    /// A zero source handle is the IPC null-handle sentinel and performs no
    /// object transfer. COPY creates a destination handle while preserving the
    /// source; MOVE creates the destination handle and closes the source.
    /// Other object types and multi-handle descriptors remain unsupported.
    pub fn translate_memory_block_handle_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        descriptor: u32,
        source_handle: u32,
    ) -> std::result::Result<Option<u32>, kernel::IpcHandleTranslationError> {
        let translation = kernel::IpcHandleTranslationDescriptor::decode(descriptor)?;
        if translation.requests_process_id() {
            return Err(kernel::IpcHandleTranslationError::ProcessIdTranslationUnsupported);
        }
        if translation.handle_count() != 1 {
            return Err(kernel::IpcHandleTranslationError::HandleCountMismatch {
                expected: 1,
                actual: usize::from(translation.handle_count()),
            });
        }
        if source_handle == 0 {
            return Ok(None);
        }

        let destination_handle = if translation.moves_handle() {
            self.move_memory_block_handle(source, source_handle)
        } else {
            self.import_memory_block_handle(source, source_handle)
        }
        .map_err(kernel::IpcHandleTranslationError::Kernel)?;

        Ok(Some(destination_handle))
    }

    /// Applies a decoded IPC type-0 descriptor to an array of memory-block handles.
    ///
    /// The descriptor's handle count must match the supplied slice. Null handles
    /// remain null. All non-null handles are type-checked before the destination
    /// table is changed. MOVE rejects duplicate source handles, imports every
    /// destination handle first, and only then closes the source handles.
    ///
    /// This intentionally supports arrays containing memory blocks only; a
    /// mixed array of kernel object types is not treated as a successful transfer.
    pub fn translate_memory_block_handles_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        descriptor: u32,
        source_handles: &[u32],
    ) -> std::result::Result<Vec<Option<u32>>, kernel::IpcHandleTranslationError> {
        let translation = kernel::IpcHandleTranslationDescriptor::decode(descriptor)?;
        if translation.requests_process_id() {
            return Err(kernel::IpcHandleTranslationError::ProcessIdTranslationUnsupported);
        }
        if usize::from(translation.handle_count()) != source_handles.len() {
            return Err(kernel::IpcHandleTranslationError::HandleCountMismatch {
                expected: translation.handle_count(),
                actual: source_handles.len(),
            });
        }

        for (index, source_handle) in source_handles.iter().copied().enumerate() {
            if source_handle == 0 {
                continue;
            }

            source
                .kernel_objects
                .memory_block_backing(source_handle)
                .map_err(kernel::IpcHandleTranslationError::Kernel)?;
            source
                .kernel_objects
                .memory_block_permissions(source_handle)
                .ok_or(kernel::IpcHandleTranslationError::Kernel(
                    KernelHandleError::WrongType,
                ))?;

            if translation.moves_handle()
                && source_handles[..index].contains(&source_handle)
            {
                return Err(kernel::IpcHandleTranslationError::DuplicateMoveHandle {
                    handle: source_handle,
                });
            }
        }

        let mut destination_handles = Vec::with_capacity(source_handles.len());
        for source_handle in source_handles.iter().copied() {
            if source_handle == 0 {
                destination_handles.push(None);
                continue;
            }

            match self.import_memory_block_handle(source, source_handle) {
                Ok(destination_handle) => destination_handles.push(Some(destination_handle)),
                Err(error) => {
                    for handle in destination_handles.iter().flatten().copied() {
                        let _ = self.kernel_objects.close_handle(handle);
                    }
                    return Err(kernel::IpcHandleTranslationError::Kernel(error));
                }
            }
        }

        if translation.moves_handle() {
            for source_handle in source_handles.iter().copied().filter(|handle| *handle != 0) {
                if let Err(error) = source.kernel_objects.close_handle(source_handle) {
                    for handle in destination_handles.iter().flatten().copied() {
                        let _ = self.kernel_objects.close_handle(handle);
                    }
                    return Err(kernel::IpcHandleTranslationError::Kernel(error));
                }
            }
        }

        Ok(destination_handles)
    }

    /// Maps a memory-block handle into this process with the requested R/W permissions.
    pub fn map_memory_block(
        &mut self,
        handle: u32,
        address: u32,
        my_permissions: u32,
        other_permissions: u32,
    ) -> std::result::Result<(), KernelHandleError> {
        let backing = self.kernel_objects.memory_block_backing(handle)?;
        let (owner_permissions, peer_permissions) = self
            .kernel_objects
            .memory_block_permissions(handle)
            .ok_or(KernelHandleError::WrongType)?;
        if my_permissions > 3
            || other_permissions > 3
            || my_permissions == 2
            || other_permissions == 2
            || my_permissions & !owner_permissions != 0
            || other_permissions & !peer_permissions != 0
        {
            return Err(KernelHandleError::InvalidMemoryPermission);
        }
        self.address_space
            .map_shared_memory_region(address, backing, my_permissions)
            .map_err(|_| KernelHandleError::InvalidMemoryBlockMapping)
    }

    /// Unmaps the exact mapping associated with a memory-block handle.
    pub fn unmap_memory_block(
        &mut self,
        handle: u32,
        address: u32,
    ) -> std::result::Result<(), KernelHandleError> {
        let backing = self.kernel_objects.memory_block_backing(handle)?;
        if !self.address_space.shared_mapping_matches(address, &backing) {
            return Err(KernelHandleError::InvalidMemoryBlockMapping);
        }
        let size = backing
            .len()
            .and_then(|size| u32::try_from(size).ok())
            .ok_or(KernelHandleError::InvalidMemoryBlockMapping)?;
        self.address_space
            .unmap_shared_memory_region(address, size)
            .map_err(|_| KernelHandleError::InvalidMemoryBlockMapping)?;
        Ok(())
    }

    /// Signals a process-local event handle from a device or HLE service.
    ///
    /// This uses the same reset and waiter semantics as SVC SignalEvent. It is
    /// the host-side entry point for services such as GSP; it does not create
    /// handles or bypass process-local event type validation.
    pub fn signal_event_handle(
        &mut self,
        handle: u32,
    ) -> std::result::Result<(), KernelHandleError> {
        let awakened = self.kernel_objects.signal_event(handle)?;
        self.wake_synchronization_waiters(awakened);
        Ok(())
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

    /// Drives the current thread's external IRQ input.
    ///
    /// An asserted IRQ wakes a core in CP15 WFI even when CPSR.I masks IRQ
    /// exception entry. In that case the guest resumes without taking IRQ.
    pub fn set_irq_line(&mut self, asserted: bool) {
        if let Some(core) = self.scheduler.current_core_mut() {
            core.set_irq_line(asserted);
        }
    }

    /// Drives the current thread's external FIQ input.
    ///
    /// An asserted FIQ wakes a core in CP15 WFI even when CPSR.F masks FIQ
    /// exception entry. In that case the guest resumes without taking FIQ.
    pub fn set_fiq_line(&mut self, asserted: bool) {
        if let Some(core) = self.scheduler.current_core_mut() {
            core.set_fiq_line(asserted);
        }
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
        if event != StepEvent::WaitingForInterrupt {
            self.instruction_count = self.instruction_count.saturating_add(1);
        }
        Ok(event)
    }

    /// Advances the virtual scheduler clock and wakes expired sleeping threads.
    ///
    /// This is an explicit virtual-time hook, not a claim that hardware timers
    /// or real-time scheduling are already emulated.
    pub fn advance_time(&mut self, nanoseconds: u64) -> Option<ThreadId> {
        self.scheduler.advance_time(nanoseconds);
        let now = self.scheduler.virtual_time_ns();
        let timer_awakened = self.kernel_objects.advance_time(now);
        self.wake_synchronization_waiters(timer_awakened);
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
                StepEvent::WaitForInterrupt => {
                    executed = executed.saturating_add(1);
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::WaitForInterrupt,
                    });
                }
                StepEvent::WaitingForInterrupt => {
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::WaitForInterrupt,
                    });
                }
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
            SVC_CREATE_MEMORY_BLOCK => self.svc_create_memory_block(caller),
            SVC_CONTROL_MEMORY => self.svc_control_memory(caller),
            SVC_MAP_MEMORY_BLOCK => self.svc_map_memory_block(caller),
            SVC_UNMAP_MEMORY_BLOCK => self.svc_unmap_memory_block(caller),
            SVC_QUERY_MEMORY => self.svc_query_memory(caller),
            SVC_CREATE_THREAD => self.svc_create_thread(caller),
            SVC_CREATE_MUTEX => self.svc_create_mutex(caller),
            SVC_RELEASE_MUTEX => self.svc_release_mutex(caller),
            SVC_CREATE_SEMAPHORE => self.svc_create_semaphore(caller),
            SVC_RELEASE_SEMAPHORE => self.svc_release_semaphore(caller),
            SVC_WAIT_SYNCHRONIZATION1 => self.svc_wait_synchronization1(caller),
            SVC_CREATE_TIMER => self.svc_create_timer(caller),
            SVC_SET_TIMER => self.svc_set_timer(caller),
            SVC_CANCEL_TIMER => self.svc_cancel_timer(caller),
            SVC_CLEAR_TIMER => self.svc_clear_timer(caller),
            SVC_CREATE_EVENT => self.svc_create_event(caller),
            SVC_SIGNAL_EVENT => self.svc_signal_event(caller),
            SVC_CLEAR_EVENT => self.svc_clear_event(caller),
            SVC_CLOSE_HANDLE => self.svc_close_handle(caller),
            SVC_EXIT_THREAD => match self.exit_current_thread(caller) {
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

    /// Handles CreateMemoryBlock: r0 = output handle pointer, r1 = source address,
    /// r2 = size, r3 = this-process permissions, r4 = peer-process permissions.
    ///
    /// Anonymous allocation is restricted to SYSTEM/BASE processes. A non-zero
    /// source must be a page-aligned range inside one mapped regular-heap
    /// allocation; its backing is shared without copying. Static, TLS, shared
    /// alias and linear-memory source ranges remain unsupported.
    fn svc_create_memory_block(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let output_address = registers[0];
        let source_address = registers[1];
        let size = registers[2];
        let owner_permissions = registers[3];
        let other_permissions = registers[4];

        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        let handle = if source_address == 0 {
            if !matches!(
                self.address_space.memory_type(),
                Arm11MemoryType::System | Arm11MemoryType::Base
            ) {
                return self.write_svc_result(caller, RESULT_MEMORY_BLOCK_NOT_ALLOWED);
            }
            self.create_memory_block(size, owner_permissions, other_permissions)
                .map_err(kernel_handle_result_code)
        } else {
            let backing = match self
                .address_space
                .shared_memory_backing_for_heap_range(source_address, size)
            {
                Ok(backing) => backing,
                Err(error) => {
                    return self.write_svc_result(caller, dynamic_memory_result_code(error));
                }
            };
            self.kernel_objects
                .create_memory_block_with_backing(backing, owner_permissions, other_permissions)
                .map_err(kernel_handle_result_code)
        };

        let handle = match handle {
            Ok(handle) => handle,
            Err(result) => return self.write_svc_result(caller, result),
        };
        if self.address_space.write32(output_address, handle).is_err() {
            let _ = self.kernel_objects.close_handle(handle);
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        self.write_svc_result(caller, 0)
    }

    /// Handles MapMemoryBlock: r0 = handle, r1 = address,
    /// r2 = this-process permissions, r3 = peer-process permissions.
    fn svc_map_memory_block(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let result = self
            .map_memory_block(registers[0], registers[1], registers[2], registers[3])
            .map(|()| 0)
            .unwrap_or_else(kernel_handle_result_code);
        self.write_svc_result(caller, result)
    }

    /// Handles UnmapMemoryBlock: r0 = handle, r1 = mapping base address.
    fn svc_unmap_memory_block(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let result = self
            .unmap_memory_block(registers[0], registers[1])
            .map(|()| 0)
            .unwrap_or_else(kernel_handle_result_code);
        self.write_svc_result(caller, result)
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

    /// Handles the raw ARM11 ControlMemory SVC register ABI for RW application
    /// heap and original-model linear FCRAM allocations. Shared, mirrored
    /// mappings and executable permissions are not implemented.
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
            (operation, 0, MEMPERM_READ_WRITE) if operation == (MEMOP_ALLOC | MEMOP_LINEAR) => {
                self.allocate_linear_memory_region(address, size)
            }
            (operation, 0, MEMPERM_NONE) if operation == (MEMOP_FREE | MEMOP_LINEAR) => {
                self.free_linear_memory_region(address, size)
            }
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

    fn allocate_linear_memory_region(
        &mut self,
        address_hint: u32,
        size: u32,
    ) -> std::result::Result<u32, u32> {
        if size == 0 || size % 0x1000 != 0 {
            return Err(RESULT_MISALIGNED_ADDRESS);
        }
        let mut memory = self.gpu_memory.clone().ok_or(RESULT_OUT_OF_MEMORY)?;
        let mut dma = self.gpu_address_space.clone().ok_or(RESULT_OUT_OF_MEMORY)?;
        let requested_mapping = if address_hint == 0 {
            None
        } else {
            let hint_end = u64::from(address_hint) + u64::from(size);
            if u64::from(address_hint) >= u64::from(OLD_LINEAR_BASE) && hint_end <= OLD_LINEAR_END {
                Some((
                    address_hint,
                    address_hint + OLD_LINEAR_VIRTUAL_TO_PHYSICAL_OFFSET,
                ))
            } else if u64::from(address_hint) >= u64::from(NEW_LINEAR_BASE)
                && hint_end <= NEW_LINEAR_END
            {
                Some((address_hint, FCRAM_BASE + (address_hint - NEW_LINEAR_BASE)))
            } else {
                return Err(RESULT_INVALID_ADDRESS);
            }
        };

        let physical_base = match requested_mapping {
            Some((_, requested_physical)) => memory
                .allocate_linear_fcram_at(requested_physical, size)
                .map_err(|_| RESULT_OUT_OF_MEMORY)?,
            None => memory
                .allocate_linear_fcram(size)
                .map_err(|_| RESULT_OUT_OF_MEMORY)?,
        };
        let virtual_base = requested_mapping.map_or_else(
            || physical_base - OLD_LINEAR_VIRTUAL_TO_PHYSICAL_OFFSET,
            |(requested_virtual, _)| requested_virtual,
        );

        if let Err(error) =
            self.address_space
                .map_linear_memory_region(virtual_base, physical_base, size)
        {
            let _ = memory.free_linear_fcram(physical_base, size);
            return Err(dynamic_memory_result_code(error));
        }

        if let Err(error) = dma.map_linear_region(virtual_base, size) {
            let _ =
                self.address_space
                    .unmap_linear_memory_region(virtual_base, physical_base, size);
            let _ = memory.free_linear_fcram(physical_base, size);
            return Err(match error {
                crate::gpu::GpuAddressSpaceError::MisalignedMapping { .. } => {
                    RESULT_MISALIGNED_ADDRESS
                }
                crate::gpu::GpuAddressSpaceError::OutsideLinearWindow { .. } => {
                    RESULT_INVALID_ADDRESS
                }
                _ => RESULT_INVALID_ADDRESS,
            });
        }

        Ok(virtual_base)
    }

    fn free_linear_memory_region(
        &mut self,
        virtual_base: u32,
        size: u32,
    ) -> std::result::Result<u32, u32> {
        if size == 0 || size % 0x1000 != 0 {
            return Err(RESULT_MISALIGNED_ADDRESS);
        }
        let mut memory = self.gpu_memory.clone().ok_or(RESULT_INVALID_ADDRESS)?;
        let mut dma = self
            .gpu_address_space
            .clone()
            .ok_or(RESULT_INVALID_ADDRESS)?;

        let mapping = dma
            .mappings()
            .into_iter()
            .find(|mapping| mapping.virtual_base() == virtual_base && mapping.size() == size)
            .ok_or(RESULT_INVALID_ADDRESS)?;
        if !memory.has_linear_fcram_allocation(mapping.physical_base(), size) {
            return Err(RESULT_INVALID_ADDRESS);
        }

        self.address_space
            .unmap_linear_memory_region(virtual_base, mapping.physical_base(), size)
            .map_err(dynamic_memory_result_code)?;
        dma.unmap_linear_region(virtual_base, size)
            .map_err(|_| RESULT_INVALID_ADDRESS)?;
        memory
            .free_linear_fcram(mapping.physical_base(), size)
            .map_err(|_| RESULT_INVALID_ADDRESS)?;
        Ok(virtual_base)
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
                match self.address_space.allocate_thread_local_storage() {
                    Err(_) => Err(RESULT_OUT_OF_THREAD_RESOURCES),
                    Ok(tls) => match self
                        .scheduler
                        .create_thread(entrypoint, argument, stack_top, priority, processor_id)
                        .map_err(thread_scheduler_result_code)
                    {
                        Ok(id) => {
                            self.scheduler
                                .core_mut(id)
                                .expect("new thread context exists")
                                .cp15_mut()
                                .set_tpidruro(tls);
                            Ok(id)
                        }
                        Err(code) => {
                            self.address_space.release_thread_local_storage(tls);
                            Err(code)
                        }
                    },
                }
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

    /// Exits the calling thread and returns its thread-local-storage slot to the pool.
    fn exit_current_thread(
        &mut self,
        caller: ThreadId,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        if let Some(core) = self.scheduler.core(caller) {
            let tls = core.cp15().tpidruro();
            self.address_space.release_thread_local_storage(tls);
        }
        self.scheduler.exit_current()
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

    /// Handles CreateTimer: r0 points to the output handle and r1 is ResetType.
    fn svc_create_timer(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let output_address = registers[0];
        let Some(reset_type) = EventResetType::from_raw(registers[1]) else {
            return self.write_svc_result(caller, RESULT_INVALID_ENUM);
        };
        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        let handle = match self.kernel_objects.insert_timer(reset_type) {
            Ok(handle) => handle,
            Err(error) => return self.write_svc_result(caller, kernel_handle_result_code(error)),
        };
        if self.address_space.write32(output_address, handle).is_err() {
            let _ = self.kernel_objects.close_handle(handle);
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        self.write_svc_result(caller, 0)
    }

    /// Handles SetTimer: r0=handle, r1=initial low, r2=interval low,
    /// r3=initial high and r4=interval high.
    fn svc_set_timer(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let initial_ns = (u64::from(registers[1]) | (u64::from(registers[3]) << 32)) as i64;
        let interval_ns = (u64::from(registers[2]) | (u64::from(registers[4]) << 32)) as i64;
        let now_ns = self.scheduler.virtual_time_ns();

        match self
            .kernel_objects
            .set_timer(registers[0], now_ns, initial_ns, interval_ns)
        {
            Ok(()) => {
                let awakened = self.kernel_objects.advance_time(now_ns);
                self.wake_synchronization_waiters(awakened);
                self.write_svc_result(caller, 0)
            }
            Err(error) => self.write_svc_result(caller, kernel_handle_result_code(error)),
        }
    }

    /// Handles CancelTimer using the handle in r0.
    fn svc_cancel_timer(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs[0];
        match self.kernel_objects.cancel_timer(handle) {
            Ok(()) => self.write_svc_result(caller, 0),
            Err(error) => self.write_svc_result(caller, kernel_handle_result_code(error)),
        }
    }

    /// Handles ClearTimer using the handle in r0.
    fn svc_clear_timer(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs[0];
        match self.kernel_objects.clear_timer(handle) {
            Ok(()) => self.write_svc_result(caller, 0),
            Err(error) => self.write_svc_result(caller, kernel_handle_result_code(error)),
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
        match self.signal_event_handle(handle) {
            Ok(()) => self.write_svc_result(caller, 0),
            Err(error) => self.write_svc_result(caller, kernel_handle_result_code(error)),
        }
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
        KernelHandleError::InvalidCount
        | KernelHandleError::InvalidDuration
        | KernelHandleError::CountLimitExceeded
        | KernelHandleError::InvalidMemoryPermission => RESULT_INVALID_ENUM,
        KernelHandleError::InvalidMemoryBlockSize => RESULT_MISALIGNED_ADDRESS,
        KernelHandleError::OutOfMemory => RESULT_OUT_OF_MEMORY,
        KernelHandleError::InvalidMemoryBlockMapping => RESULT_INVALID_ADDRESS,
    }
}

fn dynamic_memory_result_code(error: Arm11DynamicMemoryError) -> u32 {
    match error {
        Arm11DynamicMemoryError::MisalignedAddress(_) | Arm11DynamicMemoryError::InvalidSize(_) => {
            RESULT_MISALIGNED_ADDRESS
        }
        Arm11DynamicMemoryError::AllocationFailure(_) => RESULT_OUT_OF_MEMORY,
        Arm11DynamicMemoryError::OutsideApplicationHeap { .. }
        | Arm11DynamicMemoryError::OutsideLinearMemoryRange { .. }
        | Arm11DynamicMemoryError::PhysicalMemoryUnavailable
        | Arm11DynamicMemoryError::OverlappingMapping
        | Arm11DynamicMemoryError::MappingNotFound { .. }
        | Arm11DynamicMemoryError::LinearMappingNotFound { .. }
        | Arm11DynamicMemoryError::AddressRangeOverflow { .. } => RESULT_INVALID_ADDRESS,
        Arm11DynamicMemoryError::InvalidPermissions(_) => RESULT_INVALID_ENUM,
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
        test_session_with_program_and_data_and_memory_type(instructions, 1)
    }

    fn test_session_with_program_and_data_and_memory_type(
        instructions: &[(usize, u32)],
        memory_type: u8,
    ) -> Arm11ProcessSession {
        let mut text = vec![0u8; 0x1000];
        for (offset, instruction) in instructions.iter().copied() {
            text[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
        }
        let mut access_control_info = [0; 0x200];
        let other_capabilities = 0xFF00_0000 | (u32::from(memory_type) << 8);
        access_control_info[0x170..0x174].copy_from_slice(&other_capabilities.to_le_bytes());

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
                access_control_info,
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
    fn cp15_wfi_stops_guest_execution_and_a_masked_irq_wakes_it() {
        const MCR_WFI: u32 = 0xEE07_0F90; // MCR p15,0,r0,c7,c0,4
        const MOV_R0_7: u32 = 0xE3A0_0007;

        let mut session = test_session_with_program(&[MCR_WFI, MOV_R0_7]);
        let core = session.scheduler.current_core_mut().unwrap();
        core.switch_mode(crate::cpu::Psr::SUPERVISOR_MODE).unwrap();
        core.cpsr.set_interrupt_masks(0b110, true); // Mask both IRQ and FIQ.

        let entered = session.run_instructions(8).unwrap();
        assert_eq!(entered.instructions_executed, 1);
        assert_eq!(entered.stop_reason, Arm11RunStopReason::WaitForInterrupt);
        assert_eq!(session.core().pc(), 0x0010_0004);
        assert_eq!(session.instruction_count(), 1);

        // Repeated scheduler runs while the input line is low consume no
        // guest instructions and leave the PC parked after the WFI MCR.
        let asleep = session.run_instructions(8).unwrap();
        assert_eq!(asleep.instructions_executed, 0);
        assert_eq!(asleep.stop_reason, Arm11RunStopReason::WaitForInterrupt);
        assert_eq!(session.core().pc(), 0x0010_0004);
        assert_eq!(session.instruction_count(), 1);

        // A masked interrupt still completes WFI. Since CPSR.I is set, the
        // next instruction runs instead of entering the IRQ exception vector.
        session.set_irq_line(true);
        let resumed = session.run_instructions(1).unwrap();
        assert_eq!(resumed.instructions_executed, 1);
        assert_eq!(
            resumed.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 7);
        assert_eq!(session.core().pc(), 0x0010_0008);
        assert_eq!(session.instruction_count(), 2);
        session.set_irq_line(false);
    }

    #[test]
    fn cp15_wfi_unmasked_irq_wakes_and_enters_the_irq_vector() {
        const MCR_WFI: u32 = 0xEE07_0F90;
        let mut session = test_session_with_program(&[MCR_WFI, 0xE3A0_0001]);
        let core = session.scheduler.current_core_mut().unwrap();
        core.switch_mode(crate::cpu::Psr::SUPERVISOR_MODE).unwrap();
        core.cpsr.set_interrupt_masks(0b110, true);
        core.cpsr.set_interrupt_masks(0b010, false); // IRQ unmasked, FIQ masked.

        assert_eq!(
            session.run_instructions(4).unwrap().stop_reason,
            Arm11RunStopReason::WaitForInterrupt
        );
        session.set_irq_line(true);
        let resumed = session.run_instructions(1).unwrap();
        assert_eq!(resumed.instructions_executed, 1);
        assert_eq!(session.core().pc(), 0x18);
        assert_eq!(session.core().cpsr.mode(), crate::cpu::Psr::IRQ_MODE);
        session.set_irq_line(false);
    }

    #[test]
    fn main_thread_can_execute_vfp_instructions() {
        let mut session = test_session_with_program(&[
            0xEE07_0A10, // VMOV s14, r0
            0xEE37_7A07, // VADD.F32 s14, s14, s14
            0xEF00_0042, // SVC #0x42
        ]);
        session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs[0] = 1.5f32.to_bits();
        session.run_instructions(16).unwrap();
        assert_eq!(f32::from_bits(session.core().vfp().s(14)), 3.0);
    }

    #[test]
    fn create_memory_block_svc_creates_a_zeroed_block_that_can_be_mapped_and_unmapped() {
        let mut session = test_session_with_program_and_data_and_memory_type(
            &[
                (0, 0xEF00_001E), // SVC CreateMemoryBlock
                (4, 0xEF00_001F), // SVC MapMemoryBlock
                (8, 0xEF00_0020), // SVC UnmapMemoryBlock
            ],
            2, // SYSTEM memory permits anonymous shared-memory allocation.
        );
        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = 0x0020_0000;
            registers[1] = 0;
            registers[2] = 0x1000;
            registers[3] = 3;
            registers[4] = 1;
        }

        let created = session.run_instructions(1).unwrap();
        assert_eq!(created.instructions_executed, 1);
        assert_eq!(
            created.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 0);
        let handle = session.read_memory_u32(0x0020_0000).unwrap();
        assert!(handle >= 0x100);

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0800_0000;
            registers[2] = 3;
            registers[3] = 1;
        }
        let mapped = session.run_instructions(1).unwrap();
        assert_eq!(mapped.instructions_executed, 1);
        assert_eq!(session.core().regs[0], 0);
        let region = session.address_space().query_memory(0x0800_0000);
        assert_eq!(
            region.state,
            crate::loader::ncsd::process::Arm11MemoryState::Shared
        );
        assert_eq!(region.permissions, 3);
        assert_eq!(session.read_memory_u32(0x0800_0000).unwrap(), 0);

        session.write_memory_u32(0x0800_0000, 0xA1B2_C3D4).unwrap();
        assert_eq!(session.read_memory_u32(0x0800_0000).unwrap(), 0xA1B2_C3D4);

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0800_0000;
        }
        let unmapped = session.run_instructions(1).unwrap();
        assert_eq!(unmapped.instructions_executed, 1);
        assert_eq!(session.core().regs[0], 0);
        assert!(session.read_memory_u32(0x0800_0000).is_err());
        assert_eq!(session.kernel_objects.handle_count(), 1);
    }

    #[test]
    fn create_memory_block_svc_rejects_unsupported_source_and_invalid_output() {
        let mut session = test_session();
        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = 0xDEAD_0000;
            registers[1] = 0;
            registers[2] = 0x1000;
            registers[3] = 3;
            registers[4] = 1;
        }
        assert_eq!(
            session.svc_create_memory_block(MAIN_THREAD_ID),
            SvcRunDisposition::Resume
        );
        assert_eq!(session.core().regs[0], RESULT_INVALID_ADDRESS);
        assert_eq!(session.kernel_objects.handle_count(), 0);

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = 0x0020_0000;
            registers[1] = 0;
            registers[2] = 0x1000;
            registers[3] = 3;
            registers[4] = 1;
        }
        session.svc_create_memory_block(MAIN_THREAD_ID);
        assert_eq!(session.core().regs[0], RESULT_MEMORY_BLOCK_NOT_ALLOWED);
        assert_eq!(session.kernel_objects.handle_count(), 0);
    }

    #[test]
    fn create_memory_block_svc_shares_existing_regular_heap_pages_without_copying() {
        let mut session = test_session_with_program(&[0xEF00_001E, 0xEF00_001F]);
        session
            .address_space
            .allocate_application_heap_region(0x0800_0000, 0x2000)
            .unwrap();
        session.write_memory_u32(0x0800_1000, 0x1122_3344).unwrap();

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = 0x0020_0000;
            registers[1] = 0x0800_1000;
            registers[2] = 0x1000;
            registers[3] = 3;
            registers[4] = 1;
        }
        let report = session.run_instructions(1).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(session.core().regs[0], 0);
        let handle = session.read_memory_u32(0x0020_0000).unwrap();

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0900_0000;
            registers[2] = 3;
            registers[3] = 1;
        }
        let mapped = session.run_instructions(1).unwrap();
        assert_eq!(mapped.instructions_executed, 1);
        assert_eq!(session.core().regs[0], 0);
        let alias_region = session.address_space().query_memory(0x0900_0000);
        assert_eq!(
            alias_region.state,
            crate::loader::ncsd::process::Arm11MemoryState::Shared
        );
        assert_eq!(session.read_memory_u32(0x0900_0000).unwrap(), 0x1122_3344);

        session.write_memory_u32(0x0900_0000, 0xAABB_CCDD).unwrap();
        assert_eq!(session.read_memory_u32(0x0800_1000).unwrap(), 0xAABB_CCDD);
    }

    #[test]
    fn create_memory_block_svc_rejects_non_heap_source_ranges() {
        let mut session = test_session_with_program(&[0xEF00_001E]);
        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = 0x0020_0000;
            registers[1] = 0x0010_0000;
            registers[2] = 0x1000;
            registers[3] = 3;
            registers[4] = 1;
        }
        session.svc_create_memory_block(MAIN_THREAD_ID);
        assert_eq!(session.core().regs[0], RESULT_INVALID_ADDRESS);
        assert_eq!(session.kernel_objects.handle_count(), 0);
    }

    #[test]
    fn memory_block_mapping_svc_validates_handle_permissions_and_unmapping() {
        let mut session = test_session();
        let handle = session.create_memory_block(0x1000, 3, 1).unwrap();

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0800_0000;
            registers[2] = 3;
            registers[3] = 1;
        }
        assert_eq!(
            session.svc_map_memory_block(MAIN_THREAD_ID),
            SvcRunDisposition::Resume
        );
        assert_eq!(session.core().regs[0], 0);
        session.write_memory_u32(0x0800_0000, 0x5566_7788).unwrap();

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0810_0000;
            registers[2] = 3;
            registers[3] = 3;
        }
        session.svc_map_memory_block(MAIN_THREAD_ID);
        assert_eq!(
            session.core().regs[0],
            RESULT_INVALID_ENUM,
            "mapping must not grant more than the peer permission recorded by the block"
        );

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0810_0000;
            registers[2] = 2;
            registers[3] = 1;
        }
        session.svc_map_memory_block(MAIN_THREAD_ID);
        assert_eq!(
            session.core().regs[0],
            RESULT_INVALID_ENUM,
            "write-only permission is not a valid ARM11 memory permission"
        );

        {
            let registers = &mut session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs;
            registers[0] = handle;
            registers[1] = 0x0800_0000;
        }
        session.svc_unmap_memory_block(MAIN_THREAD_ID);
        assert_eq!(session.core().regs[0], 0);
        assert!(session.read_memory_u32(0x0800_0000).is_err());
    }

    #[test]
    fn memory_block_handle_maps_aliases_and_unmaps_without_destroying_backing() {
        let mut session = test_session();
        let handle = session.create_memory_block(0x1000, 3, 1).unwrap();

        session.map_memory_block(handle, 0x0800_0000, 3, 1).unwrap();
        session.map_memory_block(handle, 0x0810_0000, 1, 1).unwrap();

        session.write_memory_u32(0x0800_0000, 0xA1B2_C3D4).unwrap();
        assert_eq!(session.read_memory_u32(0x0810_0000).unwrap(), 0xA1B2_C3D4);
        assert!(session.write_memory_u32(0x0810_0000, 0xDEAD_BEEF).is_err());

        session.unmap_memory_block(handle, 0x0810_0000).unwrap();
        assert!(session.read_memory_u32(0x0810_0000).is_err());
        assert_eq!(session.read_memory_u32(0x0800_0000).unwrap(), 0xA1B2_C3D4);
        session.unmap_memory_block(handle, 0x0800_0000).unwrap();
        assert!(session.read_memory_u32(0x0800_0000).is_err());
    }

    #[test]
    fn imported_memory_block_handle_shares_backing_and_reverses_owner_permissions() {
        let mut owner = test_session();
        let owner_handle = owner.create_memory_block(0x1000, 3, 1).unwrap();
        owner
            .map_memory_block(owner_handle, 0x0800_0000, 3, 1)
            .unwrap();
        owner
            .write_memory_u32(0x0800_0000, 0xCAFE_BABE)
            .unwrap();

        let mut peer = test_session();
        let peer_handle = peer
            .import_memory_block_handle(&owner, owner_handle)
            .unwrap();
        owner.kernel_objects.close_handle(owner_handle).unwrap();
        peer.map_memory_block(peer_handle, 0x0900_0000, 1, 3)
            .unwrap();

        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0xCAFE_BABE);
        assert!(!peer.address_space.is_writable_range(0x0900_0000, 4));
        assert!(peer.write_memory_u32(0x0900_0000, 0xDEAD_BEEF).is_err());

        owner
            .write_memory_u32(0x0800_0000, 0x1234_5678)
            .unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0x1234_5678);

        assert_eq!(
            peer.import_memory_block_handle(&owner, 0xDEAD),
            Err(KernelHandleError::InvalidHandle)
        );
    }

    #[test]
    fn moved_memory_block_handle_closes_source_and_preserves_shared_backing() {
        let mut owner = test_session();
        let owner_handle = owner.create_memory_block(0x1000, 3, 1).unwrap();
        owner
            .map_memory_block(owner_handle, 0x0800_0000, 3, 1)
            .unwrap();
        owner
            .write_memory_u32(0x0800_0000, 0xCAFE_BABE)
            .unwrap();

        let mut peer = test_session();
        let peer_handle = peer
            .move_memory_block_handle(&mut owner, owner_handle)
            .unwrap();
        assert_eq!(
            owner.kernel_objects.memory_block_backing(owner_handle),
            Err(KernelHandleError::InvalidHandle)
        );

        peer.map_memory_block(peer_handle, 0x0900_0000, 1, 3)
            .unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0xCAFE_BABE);

        owner
            .write_memory_u32(0x0800_0000, 0x1234_5678)
            .unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0x1234_5678);
        assert!(!peer.address_space.is_writable_range(0x0900_0000, 0x1000));
    }

    #[test]
    fn ipc_copy_handle_descriptor_duplicates_memory_block_without_closing_source() {
        let mut source = test_session();
        let source_handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();

        let copied = destination
            .translate_memory_block_handle_from_ipc(&mut source, 0, source_handle)
            .unwrap()
            .unwrap();

        assert_ne!(copied, source_handle);
        assert!(source.kernel_objects.memory_block_backing(source_handle).is_ok());
        assert!(destination.kernel_objects.memory_block_backing(copied).is_ok());
    }

    #[test]
    fn ipc_move_handle_descriptor_closes_source_and_keeps_destination() {
        let mut source = test_session();
        let source_handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();

        let moved = destination
            .translate_memory_block_handle_from_ipc(&mut source, 0x10, source_handle)
            .unwrap()
            .unwrap();

        assert_eq!(
            source.kernel_objects.memory_block_backing(source_handle),
            Err(KernelHandleError::InvalidHandle)
        );
        assert!(destination.kernel_objects.memory_block_backing(moved).is_ok());
    }

    #[test]
    fn ipc_null_handle_does_not_create_or_close_an_object() {
        let mut source = test_session();
        let mut destination = test_session();

        assert_eq!(
            destination
                .translate_memory_block_handle_from_ipc(&mut source, 0x10, 0)
                .unwrap(),
            None
        );
    }

    #[test]
    fn ipc_handle_translation_rejects_unsupported_descriptors() {
        let mut source = test_session();
        let handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();

        assert_eq!(
            destination.translate_memory_block_handle_from_ipc(&mut source, 0x20, handle),
            Err(kernel::IpcHandleTranslationError::ProcessIdTranslationUnsupported)
        );
        assert_eq!(
            destination.translate_memory_block_handle_from_ipc(&mut source, 0x0400_0000, handle),
            Err(kernel::IpcHandleTranslationError::HandleCountMismatch {
                expected: 1,
                actual: 2,
            })
        );
        assert_eq!(
            destination.translate_memory_block_handle_from_ipc(&mut source, 0x11, handle),
            Err(kernel::IpcHandleTranslationError::InvalidDescriptor)
        );
    }

    #[test]
    fn imported_memory_block_handle_rejects_non_memory_handles() {
        let mut owner = test_session();
        let event = owner
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut peer = test_session();

        assert_eq!(
            peer.import_memory_block_handle(&owner, event),
            Err(KernelHandleError::WrongType)
        );
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
    fn control_memory_allocates_linear_fcram_and_shares_bytes_with_gpu_dma() {
        let mut session = test_session_with_program(&[
            0xEF00_0001, // SVC ControlMemory: allocate linear
            0xEF00_0001, // SVC ControlMemory: free linear
        ]);
        let mut memory = GpuPhysicalMemory::new();
        let dma = GpuDmaAddressSpace::new();
        session.attach_gpu_services(memory.clone(), dma.clone());

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC | MEMOP_LINEAR;
            core.regs[1] = 0; // addr0: let the kernel choose an address
            core.regs[2] = 0; // addr1
            core.regs[3] = 0x2000; // two contiguous pages
            core.regs[4] = MEMPERM_READ_WRITE;
        }

        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        let virtual_base = session.core().regs[1];
        assert_eq!(virtual_base, OLD_LINEAR_BASE);
        assert!(
            session
                .address_space
                .is_writable_range(virtual_base, 0x2000)
        );
        assert_eq!(dma.mappings().len(), 1);

        let physical_base = virtual_base + OLD_LINEAR_VIRTUAL_TO_PHYSICAL_OFFSET;
        assert!(memory.has_linear_fcram_allocation(physical_base, 0x2000));
        session
            .write_memory_u32(virtual_base + 0x100, 0xCAFE_BABE)
            .unwrap();
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0xCAFE_BABE);

        memory
            .write_u32(physical_base + 0x104, 0x1234_5678)
            .unwrap();
        assert_eq!(
            session.read_memory_u32(virtual_base + 0x104).unwrap(),
            0x1234_5678
        );

        let resolved = dma
            .resolve_physical_range(&memory, virtual_base + 0x100, 8)
            .unwrap();
        assert_eq!(resolved.physical_address, physical_base + 0x100);

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_FREE | MEMOP_LINEAR;
            core.regs[1] = virtual_base;
            core.regs[2] = 0;
            core.regs[3] = 0x2000;
            core.regs[4] = MEMPERM_NONE;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], virtual_base);
        assert!(dma.mappings().is_empty());
        assert!(!memory.has_linear_fcram_allocation(physical_base, 0x2000));
        assert!(!session.address_space.is_writable_range(virtual_base, 4));
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0);
    }

    #[test]
    fn control_memory_honors_the_new_linear_window_address_hint() {
        let mut session = test_session_with_program(&[0xEF00_0001, 0xEF00_0001]);
        let memory = GpuPhysicalMemory::new();
        let dma = GpuDmaAddressSpace::new();
        session.attach_gpu_services(memory.clone(), dma.clone());
        let requested_virtual = NEW_LINEAR_BASE + 0x8000;
        let expected_physical = FCRAM_BASE + 0x8000;

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC | MEMOP_LINEAR;
            core.regs[1] = requested_virtual;
            core.regs[2] = 0;
            core.regs[3] = 0x2000;
            core.regs[4] = MEMPERM_READ_WRITE;
        }
        session.run_instructions(1).unwrap();

        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], requested_virtual);
        assert!(memory.has_linear_fcram_allocation(expected_physical, 0x2000));
        assert_eq!(dma.mappings().len(), 1);
        assert_eq!(
            dma.resolve_physical_range(&memory, requested_virtual + 0x20, 4)
                .unwrap()
                .physical_address,
            expected_physical + 0x20
        );

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_FREE | MEMOP_LINEAR;
            core.regs[1] = requested_virtual;
            core.regs[2] = 0;
            core.regs[3] = 0x2000;
            core.regs[4] = MEMPERM_NONE;
        }
        session.run_instructions(1).unwrap();

        assert_eq!(session.core().regs[0], 0);
        assert!(dma.mappings().is_empty());
        assert!(!memory.has_linear_fcram_allocation(expected_physical, 0x2000));
    }

    #[test]
    fn control_memory_linear_requires_gpu_memory_services_and_valid_alignment() {
        let mut session = test_session_with_program(&[
            0xEF00_0001, // SVC ControlMemory without attached GPU services
            0xEF00_0001, // SVC ControlMemory with misaligned size
        ]);

        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC | MEMOP_LINEAR;
            core.regs[1] = 0;
            core.regs[2] = 0;
            core.regs[3] = 0x1000;
            core.regs[4] = MEMPERM_READ_WRITE;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], RESULT_OUT_OF_MEMORY);
        assert!(!session.address_space.is_writable_range(OLD_LINEAR_BASE, 1));

        let memory = GpuPhysicalMemory::new();
        let dma = GpuDmaAddressSpace::new();
        session.attach_gpu_services(memory.clone(), dma.clone());
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = MEMOP_ALLOC | MEMOP_LINEAR;
            core.regs[1] = 0;
            core.regs[2] = 0;
            core.regs[3] = 0x1001;
            core.regs[4] = MEMPERM_READ_WRITE;
        }
        session.run_instructions(1).unwrap();
        assert_eq!(session.core().regs[0], RESULT_MISALIGNED_ADDRESS);
        assert!(dma.mappings().is_empty());
        assert!(!memory.has_linear_fcram_allocation(FCRAM_BASE, 0x1000));
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
    fn each_thread_gets_its_own_tls_slot_visible_through_cp15() {
        use crate::memory::Bus;
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEE1D_5F70), // MRC p15,0,r5,c13,c0,3 (TPIDRURO)
            (0x04, 0xE585_6000), // STR r6,[r5]
            (0x08, 0xEF00_0008), // SVC CreateThread
            (0x0C, 0xEF00_0042), // SVC 0x42 (main resumes here)
            (0x20, 0xEF00_0009), // SVC ExitThread (child)
        ]);
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[6] = 0x1234_5678;
            main.regs[0] = 0x20;
            main.regs[1] = 0x0010_0020;
            main.regs[2] = 0;
            main.regs[3] = 0x0020_2000;
            main.regs[4] = 0;
        }
        assert_eq!(session.core().cp15().tpidruro(), 0x1FF8_2000);

        // MRC, STR, CreateThread: the child is now current with its own slot.
        session.run_instructions(3).unwrap();
        assert_eq!(
            session.address_space.read32(0x1FF8_2000).unwrap(),
            0x1234_5678
        );
        assert_eq!(session.core().cp15().tpidruro(), 0x1FF8_2200);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[5],
            0x1FF8_2000
        );

        // ExitThread frees the child's slot for the next thread.
        session.run_instructions(1).unwrap();
        assert_eq!(
            session
                .address_space
                .allocate_thread_local_storage()
                .unwrap(),
            0x1FF8_2200
        );
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
    fn timer_svcs_create_arm_and_wake_a_waiting_thread_on_virtual_deadline() {
        let mut session = test_session_with_program(&[
            0xEF00_001A, // CreateTimer
            0xEF00_0024, // WaitSynchronization1
            0xEF00_001B, // SetTimer
        ]);
        let main = MAIN_THREAD_ID;
        {
            let core = session.scheduler.core_mut(main).unwrap();
            core.regs[0] = 0x0020_0000;
            core.regs[1] = 0;
        }
        session.dispatch_svc(SVC_CREATE_TIMER);
        assert_eq!(session.scheduler.core(main).unwrap().regs[0], 0);
        let timer = session.address_space.read32(0x0020_0000).unwrap();

        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        assert_eq!(session.current_thread_id(), Some(child));
        {
            let core = session.scheduler.core_mut(child).unwrap();
            core.regs[0] = timer;
            core.regs[2] = u32::MAX;
            core.regs[3] = u32::MAX;
        }
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(session.current_thread_id(), Some(main));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );

        {
            let core = session.scheduler.core_mut(main).unwrap();
            core.regs[0] = timer;
            core.regs[1] = 100;
            core.regs[2] = 0;
            core.regs[3] = 0;
            core.regs[4] = 0;
        }
        session.dispatch_svc(SVC_SET_TIMER);
        assert_eq!(session.core().regs[0], 0);

        session.advance_time(99);
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );
        session.advance_time(1);

        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Running
        );
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], 0);
        assert_eq!(session.kernel_objects.timer_is_signaled(timer), Some(false));
    }

    #[test]
    fn timer_svc_cancel_and_clear_are_distinct_operations() {
        let mut session = test_session_with_program(&[0xEF00_001A, 0xEF00_001B]);
        let main = MAIN_THREAD_ID;
        {
            let core = session.scheduler.core_mut(main).unwrap();
            core.regs[0] = 0x0020_0000;
            core.regs[1] = 1;
        }
        session.dispatch_svc(SVC_CREATE_TIMER);
        let timer = session.address_space.read32(0x0020_0000).unwrap();

        {
            let core = session.scheduler.core_mut(main).unwrap();
            core.regs[0] = timer;
            core.regs[1] = 0;
            core.regs[2] = 10;
            core.regs[3] = 0;
            core.regs[4] = 0;
        }
        session.dispatch_svc(SVC_SET_TIMER);
        assert_eq!(session.kernel_objects.timer_is_signaled(timer), Some(true));

        session.scheduler.core_mut(main).unwrap().regs[0] = timer;
        session.dispatch_svc(SVC_CLEAR_TIMER);
        assert_eq!(session.kernel_objects.timer_is_signaled(timer), Some(false));

        // ClearTimer returns its result in r0, so reload the handle for the
        // following CancelTimer call just as a guest program must do.
        session.scheduler.core_mut(main).unwrap().regs[0] = timer;
        session.dispatch_svc(SVC_CANCEL_TIMER);
        session.advance_time(100);
        assert_eq!(session.kernel_objects.timer_is_signaled(timer), Some(false));
    }

    #[test]
    fn finite_wait_timeout_wakes_thread_and_removes_it_from_object_queue() {
        let mut session = test_session_with_program(&[0xEF00_0024]);
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
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
    fn process_service_event_signal_rejects_invalid_and_non_event_handles() {
        let mut session = test_session();
        let mutex = session
            .kernel_objects
            .insert_mutex(false, MAIN_THREAD_ID)
            .unwrap();

        assert_eq!(
            session.signal_event_handle(0xDEAD),
            Err(KernelHandleError::InvalidHandle)
        );
        assert_eq!(
            session.signal_event_handle(mutex),
            Err(KernelHandleError::WrongType)
        );
    }

    #[test]
    fn successful_object_signal_cancels_a_pending_finite_timeout() {
        let mut session = test_session_with_program(&[0xEF00_0024]);
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
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

        session.signal_event_handle(event).unwrap();
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
