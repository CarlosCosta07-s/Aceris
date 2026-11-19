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
use crate::cpu::{Arm11Core, MpcoreScu, StepEvent};
use crate::gpu::{
    FCRAM_BASE, GpuDmaAddressSpace, GpuPhysicalMemory, NEW_LINEAR_BASE, NEW_LINEAR_END,
    OLD_LINEAR_BASE, OLD_LINEAR_END,
};
use crate::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use crate::loader::ncsd::ncch::extended_header::Arm11MemoryType;
use crate::loader::ncsd::process::{
    Arm11CodeAddressSpace, Arm11DynamicMemoryError, Arm11ProcessLayoutError, SharedMemoryBacking,
};
use crate::loader::ncsd::{NcsdImageReader, NcsdImageReaderError};
use crate::loader::threedsx::process::ThreeDsxPreparedProcess;
use crate::memory::Bus;
use kernel::{
    Arm11KernelObjectTable, CURRENT_PROCESS_HANDLE, CURRENT_THREAD_HANDLE, EventResetType,
    KernelHandleError, ServiceSessionKind,
};
use svc::{
    Arm11SvcDispatcher, Arm11SvcDisposition, SVC_CANCEL_TIMER, SVC_CLEAR_EVENT, SVC_CLEAR_TIMER,
    SVC_CLOSE_HANDLE, SVC_CONNECT_TO_PORT, SVC_CONTROL_MEMORY, SVC_CREATE_EVENT,
    SVC_CREATE_MEMORY_BLOCK, SVC_CREATE_MUTEX, SVC_CREATE_SEMAPHORE, SVC_CREATE_THREAD,
    SVC_CREATE_TIMER, SVC_DUPLICATE_HANDLE, SVC_EXIT_THREAD, SVC_GET_HANDLE_INFO,
    SVC_GET_PROCESS_INFO, SVC_GET_SYSTEM_TICK, SVC_GET_THREAD_INFO, SVC_GET_THREAD_PRIORITY,
    SVC_MAP_MEMORY_BLOCK, SVC_QUERY_MEMORY, SVC_RELEASE_MUTEX, SVC_RELEASE_SEMAPHORE,
    SVC_SEND_SYNC_REQUEST, SVC_SET_THREAD_PRIORITY, SVC_SET_TIMER, SVC_SIGNAL_EVENT,
    SVC_SLEEP_THREAD, SVC_UNMAP_MEMORY_BLOCK, SVC_WAIT_SYNCHRONIZATION_N,
    SVC_WAIT_SYNCHRONIZATION1,
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

/// Kernel result returned when the guest passes a null handle-array pointer.
pub const RESULT_INVALID_POINTER: u32 = 0xD8E0_07F6;

/// Kernel result returned when a timeout is negative and is not -1.
pub const RESULT_INVALID_TIMEOUT: u32 = 0xD8E0_07FD;

/// Kernel OutOfRange result, returned for an invalid handle count.
pub const RESULT_OUT_OF_RANGE: u32 = 0xE0E0_1BFD;

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
    /// A WFI instruction is waiting for an IRQ or FIQ, even when masked.
    WaitForInterrupt,
    /// The current core is waiting for an ARMv6K event.
    WaitForEvent,
    /// A guest SEV instruction retired and must be broadcast by the system scheduler.
    SendEvent,
    /// An exception return completed and must wake other MP11 CPUs' event registers.
    ExceptionReturn,
    /// No thread is currently eligible to execute.
    NoRunnableThread,
    /// A synchronous IPC request must be dispatched by the system service layer.
    IpcRequest,
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

/// Failure while applying type-1 IPC static-buffer copies to a receiving thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcStaticBufferTranslationError {
    /// The message's translation section is malformed.
    TranslationDecode(ipc::IpcTranslationSectionError),
    /// The translation section contains no static-buffer descriptors.
    NoStaticBufferParameters,
    /// A non-static-buffer translation descriptor was supplied.
    UnsupportedTranslationParameter { descriptor: u32 },
    /// The destination thread does not belong to this process.
    UnknownDestinationThread(ThreadId),
    /// The destination thread has already exited.
    ExitedDestinationThread(ThreadId),
    /// The destination thread has no live, allocated TLS slot.
    InvalidDestinationTls { address: u32 },
    /// The receiver's registered slot is smaller than the requested transfer.
    DestinationBufferTooSmall {
        buffer_index: u8,
        capacity: u32,
        requested: u32,
    },
    /// The destination range is null, unmapped, or not writable.
    DestinationRangeNotWritable { address: u32, size_bytes: u32 },
    /// The sender's source range is not readable.
    SourceRangeUnavailable { address: u32, size_bytes: u32 },
    /// The host could not allocate storage for a staged transfer.
    AllocationFailure,
    /// A prevalidated destination failed while being written.
    DestinationWriteFailed { address: u32 },
}

/// Failure while materializing IPC type-2/3 physical buffer chunk tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcPhysicalBufferTranslationError {
    TranslationDecode(ipc::IpcTranslationSectionError),
    NoPhysicalBufferParameters,
    UnsupportedTranslationParameter { descriptor: u32 },
    UnknownDestinationThread(ThreadId),
    ExitedDestinationThread(ThreadId),
    InvalidDestinationTls { address: u32 },
    InvalidDestinationBuffer { address: u32, capacity: u32 },
    DestinationBufferNotPhysical { address: u32 },
    DestinationBufferTooSmall { capacity: u32, required: u32 },
    SourceRangeNotPhysical { address: u32, size_bytes: u32 },
    SourcePermissionsInsufficient { address: u32, size_bytes: u32 },
    AllocationFailure,
    DestinationWriteFailed { address: u32 },
}

/// One translated homogeneous type-0 handle group in an IPC message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm11IpcHandleGroup {
    /// Offset of the type-0 descriptor within the translation section.
    pub word_offset: usize,
    /// Receiver-side handles corresponding to the original handle array.
    pub handles: Vec<Option<u32>>,
}

/// A mapped type-5/6/7 buffer kept alive between an IPC request and its reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arm11IpcTemporaryBuffer {
    /// Offset of the descriptor within the message translation section.
    pub word_offset: usize,
    /// Original address in the sending process.
    pub source_address: u32,
    /// Alias address in the receiving process, preserving the source page offset.
    pub mapped_address: u32,
    /// Logical buffer length, excluding page padding.
    pub size_bytes: u32,
    /// Source-process access flags: 1=read, 2=write, 3=read/write.
    pub permissions: u8,
}

/// Errors while creating or completing temporary IPC buffer mappings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcTemporaryBufferTranslationError {
    TranslationDecode(ipc::IpcTranslationSectionError),
    UnknownDestinationThread(ThreadId),
    ExitedDestinationThread(ThreadId),
    SourceRangeUnavailable { address: u32, size_bytes: u32 },
    SourceRangeNotWritable { address: u32, size_bytes: u32 },
    AllocationFailure,
    Mapping(Arm11DynamicMemoryError),
    DestinationWriteFailed { address: u32 },
    SourceWriteBackFailed { address: u32, size_bytes: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SvcRunDisposition {
    Resume,
    ProcessExited,
    Unhandled(u32),
    NoRunnableThread,
    KernelServiceError { number: u32, result: u32 },
    IpcRequest,
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
    /// Handle lists of threads blocked in WaitSynchronizationN wait-any mode.
    multi_waits: BTreeMap<ThreadId, Vec<u32>>,
    /// Handle sets of threads blocked in WaitSynchronizationN wait-all mode.
    wait_all_waits: BTreeMap<ThreadId, Vec<u32>>,
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
            multi_waits: BTreeMap::new(),
            wait_all_waits: BTreeMap::new(),
            gpu_memory: None,
            gpu_address_space: None,
            instruction_count: 0,
        }
    }

    /// Creates an ARM11 process session from a validated 3DSX homebrew image.
    ///
    /// The same scheduler, main-thread context, TLS allocation and SVC
    /// dispatcher used by NCCH processes are used for 3DSX programs.
    pub fn from_3dsx_prepared(
        prepared: ThreeDsxPreparedProcess,
    ) -> std::result::Result<Self, Arm11ProcessLayoutError> {
        let address_space = Arm11CodeAddressSpace::from_3dsx_prepared(prepared)?;
        Ok(Self::from_address_space(address_space))
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

    /// Adds the process-side alias for a physical linear-memory allocation.
    /// System-level ControlMemory normally updates both this map and the GPU DMA map.
    pub fn map_linear_memory_region(
        &mut self,
        virtual_base: u32,
        physical_base: u32,
        size: u32,
    ) -> std::result::Result<(), Arm11DynamicMemoryError> {
        self.address_space
            .map_linear_memory_region(virtual_base, physical_base, size)
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

    /// Connects the process bus to the system-wide ARM11 MPCore SCU.
    pub fn attach_mpcore_scu(&mut self, scu: MpcoreScu) {
        self.address_space.attach_mpcore_scu(scu);
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

    /// Performs the ARM11 FlushProcessDataCache operation for this process.
    ///
    /// Dirty lines intersecting the virtual range are written back to shared
    /// physical memory and then invalidated. The full range is validated before
    /// any cache line is modified.
    pub fn flush_process_data_cache(&mut self, address: u32, size_bytes: u32) -> Result<usize> {
        self.address_space
            .flush_data_cache_range(address, size_bytes)
    }

    /// Cleans dirty process data-cache lines while retaining them as clean.
    pub fn store_process_data_cache(&mut self, address: u32, size_bytes: u32) -> Result<usize> {
        self.address_space
            .store_data_cache_range(address, size_bytes)
    }

    /// Invalidates process data-cache lines without writing dirty data back.
    pub fn invalidate_process_data_cache(
        &mut self,
        address: u32,
        size_bytes: u32,
    ) -> Result<usize> {
        self.address_space
            .invalidate_data_cache_range(address, size_bytes)
    }

    /// Drops cached lines of every guest mapping of a backing an HLE service just wrote.
    pub fn invalidate_shared_backing_cache(
        &mut self,
        backing: &SharedMemoryBacking,
    ) -> Result<usize> {
        self.address_space
            .invalidate_shared_backing_data_cache(backing)
    }

    /// Completes the process ARM11 D-cache writeback queue in FIFO order.
    pub fn drain_write_buffer(&mut self) -> Result<usize> {
        self.address_space.drain_arm11_write_buffer()
    }

    /// Applies the process data-memory barrier ordering point (DMB).
    ///
    /// The current backing model completes all older buffered stores before
    /// the call returns. The CP15 DSB path uses the same completion primitive.
    pub fn data_memory_barrier(&mut self) -> Result<()> {
        crate::memory::Bus::data_memory_barrier(&mut self.address_space)
    }

    /// Number of D-cache writeback entries still waiting for physical memory.
    pub fn pending_write_buffer_entries(&self) -> usize {
        self.address_space.pending_write_buffer_entries()
    }

    /// Returns the number of resident data-cache lines for diagnostics/tests.
    pub fn data_cache_line_count(&self) -> usize {
        self.address_space.data_cache_line_count()
    }

    /// Returns the current thread's TLS base, as exposed by CP15 TPIDRURO.
    pub fn current_tls_address(&self) -> Option<u32> {
        let thread = self.scheduler.current_thread_id()?;
        self.scheduler
            .core(thread)
            .map(|core| core.cp15().tpidruro())
    }

    /// Copies the active thread's ARM11 register bank for a host-side SVC bridge.
    pub fn current_registers(&self) -> Option<[u32; 16]> {
        self.scheduler.current_core().map(|core| core.regs)
    }

    /// Updates one register in a selected live thread context.
    pub fn set_thread_register(&mut self, thread: ThreadId, index: usize, value: u32) -> bool {
        let Some(core) = self.scheduler.core_mut(thread) else {
            return false;
        };
        let Some(register) = core.regs.get_mut(index) else {
            return false;
        };
        *register = value;
        true
    }

    /// Updates one register in the currently running thread.
    pub fn set_current_register(&mut self, index: usize, value: u32) -> bool {
        let Some(thread) = self.scheduler.current_thread_id() else {
            return false;
        };
        self.set_thread_register(thread, index, value)
    }

    /// Creates the `srv:` service-manager endpoint in this process.
    pub fn create_service_manager_session(
        &mut self,
    ) -> std::result::Result<u32, KernelHandleError> {
        self.kernel_objects.insert_service_manager_session()
    }

    /// Marks this process as registered with the service manager on this session.
    pub fn register_service_manager_client(
        &mut self,
        handle: u32,
    ) -> std::result::Result<(), KernelHandleError> {
        self.kernel_objects.register_service_manager_client(handle)
    }

    /// Reports whether this service-manager session completed RegisterClient.
    pub fn service_manager_client_registered(&self, handle: u32) -> Option<bool> {
        self.kernel_objects
            .service_manager_client_registered(handle)
    }

    /// Reports the current signal state of a process-local event handle.
    pub fn event_handle_is_signaled(&self, handle: u32) -> Option<bool> {
        self.kernel_objects.event_is_signaled(handle)
    }

    /// Clears a process-local event handle from a device or HLE service.
    pub fn clear_event_handle(
        &mut self,
        handle: u32,
    ) -> std::result::Result<(), KernelHandleError> {
        self.kernel_objects.clear_event(handle)
    }

    /// Creates a `gsp::Gpu` client endpoint in this process.
    pub fn create_gsp_gpu_session(&mut self) -> std::result::Result<u32, KernelHandleError> {
        self.kernel_objects.insert_gsp_gpu_session()
    }

    /// Creates a client endpoint for a stateless HLE service in this process.
    pub fn create_hle_session(
        &mut self,
        kind: ServiceSessionKind,
    ) -> std::result::Result<u32, KernelHandleError> {
        self.kernel_objects.insert_hle_session(kind)
    }

    /// Creates an unlocked mutex handle owned by an HLE service (for example the APT lock).
    pub fn create_service_mutex(&mut self) -> std::result::Result<u32, KernelHandleError> {
        let creator = self.scheduler.current_thread_id().unwrap_or(MAIN_THREAD_ID);
        self.kernel_objects.insert_mutex(false, creator)
    }

    /// Creates a `hid:USER` client endpoint in this process.
    pub fn create_hid_session(&mut self) -> std::result::Result<u32, KernelHandleError> {
        self.kernel_objects.insert_hid_session()
    }

    /// Returns the shared backing of a memory-block handle for HLE services.
    pub fn memory_block_backing(
        &self,
        handle: u32,
    ) -> std::result::Result<SharedMemoryBacking, KernelHandleError> {
        self.kernel_objects.memory_block_backing(handle)
    }

    /// Current emulated ARM11 system tick (268,111,856 Hz, virtual time).
    pub fn system_tick(&self) -> u64 {
        const TICKS_PER_SECOND: u128 = 268_111_856;
        let elapsed_ns = u128::from(self.scheduler.virtual_time_ns());
        ((elapsed_ns * TICKS_PER_SECOND) / 1_000_000_000) as u64
    }

    /// Creates an event handle for a process-local service or device client.
    pub fn create_event_handle(
        &mut self,
        reset_type: EventResetType,
    ) -> std::result::Result<u32, KernelHandleError> {
        self.kernel_objects.insert_event(reset_type)
    }

    /// Returns service-session metadata for a guest-visible handle.
    pub fn service_session_kind(&self, handle: u32) -> Option<ServiceSessionKind> {
        self.kernel_objects.service_session(handle)
    }

    /// Stores the GSP registration associated with one service session.
    pub fn set_gsp_session_registration(
        &mut self,
        handle: u32,
        client_id: Option<u8>,
        shared_memory_handle: Option<u32>,
        event_handle: Option<u32>,
        flags: u32,
    ) -> std::result::Result<(), KernelHandleError> {
        self.kernel_objects.set_gsp_session_registration(
            handle,
            client_id,
            shared_memory_handle,
            event_handle,
            flags,
        )
    }

    /// Closes a process-local handle and applies KThread last-handle semantics.
    ///
    /// A live thread is terminated when its final thread handle closes. An
    /// exited thread's saved CPU context is reclaimed when its final handle
    /// closes. Objects already pinned by a synchronization wait survive handle
    /// closure until that wait completes, times out or the waiting thread exits.
    pub fn close_handle(&mut self, handle: u32) -> std::result::Result<(), KernelHandleError> {
        let thread_target = self.kernel_objects.thread_handle_target(handle);
        if let Some(target) = thread_target {
            if self.scheduler.thread_info(target).is_none() {
                return Err(KernelHandleError::InvalidHandle);
            }
        }

        self.kernel_objects.close_handle(handle)?;
        if let Some(target) = thread_target {
            if self.kernel_objects.thread_handle_count(target) == 0 {
                match self.scheduler.thread_info(target).map(|info| info.state) {
                    Some(ThreadState::Exited) => {
                        self.scheduler.reap_exited_thread(target);
                    }
                    Some(_) => {
                        self.terminate_thread_context(target)
                            .map_err(|_| KernelHandleError::InvalidHandle)?;
                    }
                    None => return Err(KernelHandleError::InvalidHandle),
                }
            }
        }

        self.service_wait_all_waiters();
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

            if translation.moves_handle() && source_handles[..index].contains(&source_handle) {
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

    /// Translates every type-0 group in a mixed IPC translation section.
    ///
    /// Only memory-block handles are currently transferable. All descriptors,
    /// source object types, counts and MOVE duplicates across the full section
    /// are validated before the destination table is modified. Destination
    /// handles for all groups are created before any MOVE source is closed.
    fn translate_memory_block_handle_groups_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        parameters: &[ipc::IpcTranslationParameter<'_>],
    ) -> std::result::Result<Vec<Arm11IpcHandleGroup>, kernel::IpcHandleTranslationError> {
        let mut groups = Vec::new();
        groups.try_reserve_exact(parameters.len()).map_err(|_| {
            kernel::IpcHandleTranslationError::Kernel(KernelHandleError::OutOfMemory)
        })?;

        let mut moved_source_handles = Vec::new();
        let mut total_non_null_handles = 0usize;

        for parameter in parameters {
            let ipc::IpcTranslationParameter::Handles {
                descriptor,
                word_offset,
                handles,
                ..
            } = parameter
            else {
                continue;
            };
            let translation = kernel::IpcHandleTranslationDescriptor::decode(*descriptor)?;
            if translation.requests_process_id() {
                return Err(kernel::IpcHandleTranslationError::ProcessIdTranslationUnsupported);
            }
            if usize::from(translation.handle_count()) != handles.len() {
                return Err(kernel::IpcHandleTranslationError::HandleCountMismatch {
                    expected: translation.handle_count(),
                    actual: handles.len(),
                });
            }
            total_non_null_handles = total_non_null_handles
                .checked_add(handles.iter().filter(|handle| **handle != 0).count())
                .ok_or(kernel::IpcHandleTranslationError::Kernel(
                    KernelHandleError::OutOfMemory,
                ))?;

            for source_handle in handles.iter().copied().filter(|handle| *handle != 0) {
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

                if translation.moves_handle() {
                    if moved_source_handles.contains(&source_handle) {
                        return Err(kernel::IpcHandleTranslationError::DuplicateMoveHandle {
                            handle: source_handle,
                        });
                    }
                    moved_source_handles.try_reserve(1).map_err(|_| {
                        kernel::IpcHandleTranslationError::Kernel(KernelHandleError::OutOfMemory)
                    })?;
                    moved_source_handles.push(source_handle);
                }
            }
            groups.push(Arm11IpcHandleGroup {
                word_offset: *word_offset,
                handles: Vec::new(),
            });
        }

        if groups.is_empty() {
            return Ok(groups);
        }

        let mut created_destination_handles = Vec::new();
        created_destination_handles
            .try_reserve_exact(total_non_null_handles)
            .map_err(|_| {
                kernel::IpcHandleTranslationError::Kernel(KernelHandleError::OutOfMemory)
            })?;

        let mut group_index = 0usize;
        for parameter in parameters {
            let ipc::IpcTranslationParameter::Handles {
                descriptor,
                handles,
                ..
            } = parameter
            else {
                continue;
            };
            let mut translated_handles = Vec::new();
            translated_handles
                .try_reserve_exact(handles.len())
                .map_err(|_| {
                    for handle in created_destination_handles.iter().copied() {
                        let _ = self.kernel_objects.close_handle(handle);
                    }
                    kernel::IpcHandleTranslationError::Kernel(KernelHandleError::OutOfMemory)
                })?;

            for source_handle in handles.iter().copied() {
                if source_handle == 0 {
                    translated_handles.push(None);
                    continue;
                }
                match self.import_memory_block_handle(source, source_handle) {
                    Ok(destination_handle) => {
                        created_destination_handles.push(destination_handle);
                        translated_handles.push(Some(destination_handle));
                    }
                    Err(error) => {
                        for handle in created_destination_handles.iter().copied() {
                            let _ = self.kernel_objects.close_handle(handle);
                        }
                        return Err(kernel::IpcHandleTranslationError::Kernel(error));
                    }
                }
            }
            groups[group_index].handles = translated_handles;
            group_index += 1;

            // The descriptor is decoded again below only when MOVE sources close.
            let _ = descriptor;
        }

        for parameter in parameters {
            let ipc::IpcTranslationParameter::Handles {
                descriptor,
                handles,
                ..
            } = parameter
            else {
                continue;
            };
            let translation = kernel::IpcHandleTranslationDescriptor::decode(*descriptor)?;
            if translation.moves_handle() {
                for source_handle in handles.iter().copied().filter(|handle| *handle != 0) {
                    if let Err(error) = source.kernel_objects.close_handle(source_handle) {
                        for destination_handle in created_destination_handles.iter().copied() {
                            let _ = self.kernel_objects.close_handle(destination_handle);
                        }
                        return Err(kernel::IpcHandleTranslationError::Kernel(error));
                    }
                }
            }
        }

        Ok(groups)
    }

    /// Translates every memory-block handle group in a decoded IPC message.
    ///
    /// Type-0 groups are transferred in their original order. Other decoded
    /// translation families are left to their dedicated helpers. The returned
    /// offsets identify the descriptor words to replace in the receiver's
    /// message buffer with the translated handle values.
    ///
    /// All type-0 source handles and MOVE duplicates are validated before the
    /// destination handle table is changed. All destination handles are created
    /// before any MOVE source is closed.
    pub fn translate_memory_block_handle_groups_from_ipc_message(
        &mut self,
        source: &mut Arm11ProcessSession,
        message: ipc::IpcMessage<'_>,
    ) -> std::result::Result<Vec<Arm11IpcHandleGroup>, kernel::IpcHandleTranslationError> {
        let parameters = message
            .translation_parameters()
            .map_err(|_| kernel::IpcHandleTranslationError::InvalidDescriptor)?;
        self.translate_memory_block_handle_groups_from_ipc(source, &parameters)
    }

    /// Translates a translation section containing one type-0 descriptor and its handles.
    ///
    /// The generic IPC decoder has already bounded the section to the word count
    /// declared by the message header. This adapter requires the entire section
    /// to contain exactly one type-0 descriptor followed by its handle array.
    /// Additional descriptor groups and mixed kernel-object arrays remain unsupported.
    pub fn translate_memory_block_translation_section_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        message: ipc::IpcMessage<'_>,
    ) -> std::result::Result<Vec<Option<u32>>, kernel::IpcHandleTranslationError> {
        let parameters = message
            .translation_parameters()
            .map_err(|_| kernel::IpcHandleTranslationError::InvalidDescriptor)?;
        let [
            ipc::IpcTranslationParameter::Handles {
                descriptor,
                handles,
                ..
            },
        ] = parameters.as_slice()
        else {
            return Err(kernel::IpcHandleTranslationError::InvalidDescriptor);
        };

        self.translate_memory_block_handles_from_ipc(source, *descriptor, handles)
    }

    /// Applies every type-1 IPC static-buffer copy to a specific receiving thread.
    ///
    /// The receiver's 16-entry descriptor table is read from that thread's live TLS
    /// slot. All static-buffer capacities, destination ranges and source bytes are
    /// validated/staged before the first destination byte is changed. Other
    /// translation families are left for their corresponding transport helpers.
    pub fn copy_static_buffer_translation_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        destination_thread: ThreadId,
        message: ipc::IpcMessage<'_>,
    ) -> std::result::Result<Vec<(usize, u32)>, IpcStaticBufferTranslationError> {
        let parameters = message
            .translation_parameters()
            .map_err(IpcStaticBufferTranslationError::TranslationDecode)?;
        if parameters.is_empty() {
            return Err(IpcStaticBufferTranslationError::NoStaticBufferParameters);
        }

        let thread_info = self.scheduler.thread_info(destination_thread).ok_or(
            IpcStaticBufferTranslationError::UnknownDestinationThread(destination_thread),
        )?;
        if thread_info.state == ThreadState::Exited {
            return Err(IpcStaticBufferTranslationError::ExitedDestinationThread(
                destination_thread,
            ));
        }
        let tls_address = self
            .scheduler
            .core(destination_thread)
            .map(|core| core.cp15().tpidruro())
            .ok_or(IpcStaticBufferTranslationError::UnknownDestinationThread(
                destination_thread,
            ))?;
        if tls_address == 0 {
            return Err(IpcStaticBufferTranslationError::InvalidDestinationTls {
                address: tls_address,
            });
        }

        struct PendingStaticBufferCopy {
            word_offset: usize,
            destination_address: u32,
            bytes: Vec<u8>,
        }

        let mut pending = Vec::new();
        let mut found_static_buffer = false;
        let mut first_unsupported_descriptor = None;
        pending
            .try_reserve_exact(parameters.len())
            .map_err(|_| IpcStaticBufferTranslationError::AllocationFailure)?;

        for parameter in parameters {
            let (word_offset, buffer_index, size_bytes, source_address) = match parameter {
                ipc::IpcTranslationParameter::StaticBuffer {
                    word_offset,
                    buffer_index,
                    size_bytes,
                    source_address,
                    ..
                } => (word_offset, buffer_index, size_bytes, source_address),
                ipc::IpcTranslationParameter::Handles { descriptor, .. }
                | ipc::IpcTranslationParameter::MappedBuffer { descriptor, .. }
                | ipc::IpcTranslationParameter::PhysicalBufferTable { descriptor, .. } => {
                    first_unsupported_descriptor.get_or_insert(descriptor);
                    continue;
                }
            };
            found_static_buffer = true;

            let entry_address = self
                .address_space
                .thread_local_static_buffer_descriptor_address(tls_address, buffer_index)
                .ok_or(IpcStaticBufferTranslationError::InvalidDestinationTls {
                    address: tls_address,
                })?;
            let capacity_address = entry_address.checked_add(4).ok_or(
                IpcStaticBufferTranslationError::InvalidDestinationTls {
                    address: entry_address,
                },
            )?;
            let destination_address = self.read_memory_u32(entry_address).map_err(|_| {
                IpcStaticBufferTranslationError::InvalidDestinationTls {
                    address: entry_address,
                }
            })?;
            let capacity = self.read_memory_u32(capacity_address).map_err(|_| {
                IpcStaticBufferTranslationError::InvalidDestinationTls {
                    address: capacity_address,
                }
            })?;

            if size_bytes > capacity {
                return Err(IpcStaticBufferTranslationError::DestinationBufferTooSmall {
                    buffer_index,
                    capacity,
                    requested: size_bytes,
                });
            }

            if size_bytes == 0 {
                pending.push(PendingStaticBufferCopy {
                    word_offset,
                    destination_address,
                    bytes: Vec::new(),
                });
                continue;
            }

            let destination_range_is_writable =
                usize::try_from(size_bytes).ok().is_some_and(|size| {
                    destination_address != 0
                        && self
                            .address_space
                            .is_writable_range(destination_address, size)
                });
            if !destination_range_is_writable {
                return Err(
                    IpcStaticBufferTranslationError::DestinationRangeNotWritable {
                        address: destination_address,
                        size_bytes,
                    },
                );
            }

            let size = usize::try_from(size_bytes)
                .map_err(|_| IpcStaticBufferTranslationError::AllocationFailure)?;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(size)
                .map_err(|_| IpcStaticBufferTranslationError::AllocationFailure)?;
            bytes.resize(size, 0);
            source
                .read_memory_bytes(source_address, &mut bytes)
                .map_err(
                    |_| IpcStaticBufferTranslationError::SourceRangeUnavailable {
                        address: source_address,
                        size_bytes,
                    },
                )?;
            pending.push(PendingStaticBufferCopy {
                word_offset,
                destination_address,
                bytes,
            });
        }

        if !found_static_buffer {
            return Err(first_unsupported_descriptor.map_or(
                IpcStaticBufferTranslationError::NoStaticBufferParameters,
                |descriptor| IpcStaticBufferTranslationError::UnsupportedTranslationParameter {
                    descriptor,
                },
            ));
        }

        let mut translated = Vec::new();
        translated
            .try_reserve_exact(pending.len())
            .map_err(|_| IpcStaticBufferTranslationError::AllocationFailure)?;
        for copy in &pending {
            translated.push((copy.word_offset, copy.destination_address));
        }

        for copy in pending {
            self.write_memory_bytes(copy.destination_address, &copy.bytes)
                .map_err(
                    |_| IpcStaticBufferTranslationError::DestinationWriteFailed {
                        address: copy.destination_address,
                    },
                )?;
        }

        Ok(translated)
    }

    /// Materializes type-2/3 IPC physical chunk tables in the receiver's static buffers.
    ///
    /// Only ranges backed by the process's explicit linear FCRAM mappings are
    /// accepted. This never invents a physical address for private host Vec,
    /// executable, TLS or ordinary shared-memory backing. The returned pairs
    /// contain each descriptor's word offset in the translation section and
    /// the physical address of its receiver-side chunk table.
    pub fn translate_physical_buffer_tables_from_ipc(
        &mut self,
        source: &Arm11ProcessSession,
        destination_thread: ThreadId,
        message: ipc::IpcMessage<'_>,
    ) -> std::result::Result<Vec<(usize, u32)>, IpcPhysicalBufferTranslationError> {
        let parameters = message
            .translation_parameters()
            .map_err(IpcPhysicalBufferTranslationError::TranslationDecode)?;
        if parameters.is_empty() {
            return Err(IpcPhysicalBufferTranslationError::NoPhysicalBufferParameters);
        }

        let thread_info = self.scheduler.thread_info(destination_thread).ok_or(
            IpcPhysicalBufferTranslationError::UnknownDestinationThread(destination_thread),
        )?;
        if thread_info.state == ThreadState::Exited {
            return Err(IpcPhysicalBufferTranslationError::ExitedDestinationThread(
                destination_thread,
            ));
        }
        let tls_address = self
            .scheduler
            .core(destination_thread)
            .map(|core| core.cp15().tpidruro())
            .ok_or(IpcPhysicalBufferTranslationError::UnknownDestinationThread(
                destination_thread,
            ))?;
        if tls_address == 0 {
            return Err(IpcPhysicalBufferTranslationError::InvalidDestinationTls {
                address: tls_address,
            });
        }

        struct PendingPhysicalTable {
            destination_address: u32,
            bytes: Vec<u8>,
        }

        let mut pending = Vec::new();
        let mut translated = Vec::new();
        pending
            .try_reserve_exact(parameters.len())
            .map_err(|_| IpcPhysicalBufferTranslationError::AllocationFailure)?;
        translated
            .try_reserve_exact(parameters.len())
            .map_err(|_| IpcPhysicalBufferTranslationError::AllocationFailure)?;

        for parameter in parameters {
            let (word_offset, buffer_index, size_bytes, source_address, read_only) = match parameter
            {
                ipc::IpcTranslationParameter::PhysicalBufferTable {
                    word_offset,
                    buffer_index,
                    size_bytes,
                    source_address,
                    read_only,
                    ..
                } => (
                    word_offset,
                    buffer_index,
                    size_bytes,
                    source_address,
                    read_only,
                ),
                _ => continue,
            };

            let entry_address = self
                .address_space
                .thread_local_static_buffer_descriptor_address(tls_address, buffer_index)
                .ok_or(IpcPhysicalBufferTranslationError::InvalidDestinationTls {
                    address: tls_address,
                })?;
            let capacity_address = entry_address.checked_add(4).ok_or(
                IpcPhysicalBufferTranslationError::InvalidDestinationTls {
                    address: entry_address,
                },
            )?;
            let destination_address = self.read_memory_u32(entry_address).map_err(|_| {
                IpcPhysicalBufferTranslationError::InvalidDestinationTls {
                    address: entry_address,
                }
            })?;
            let capacity = self.read_memory_u32(capacity_address).map_err(|_| {
                IpcPhysicalBufferTranslationError::InvalidDestinationTls {
                    address: capacity_address,
                }
            })?;

            if destination_address == 0
                || destination_address & 0xFFF != 0
                || capacity == 0
                || capacity > 0x1000
            {
                return Err(
                    IpcPhysicalBufferTranslationError::InvalidDestinationBuffer {
                        address: destination_address,
                        capacity,
                    },
                );
            }

            let source_size = usize::try_from(size_bytes)
                .map_err(|_| IpcPhysicalBufferTranslationError::AllocationFailure)?;
            let source_permissions_valid = source
                .address_space
                .is_readable_range(source_address, source_size)
                && (read_only
                    || source
                        .address_space
                        .is_writable_range(source_address, source_size));
            if !source_permissions_valid {
                return Err(
                    IpcPhysicalBufferTranslationError::SourcePermissionsInsufficient {
                        address: source_address,
                        size_bytes,
                    },
                );
            }

            let source_chunks = source
                .address_space
                .linear_physical_chunks_for_range(source_address, size_bytes)
                .ok_or(IpcPhysicalBufferTranslationError::SourceRangeNotPhysical {
                    address: source_address,
                    size_bytes,
                })?;
            let table_size = source_chunks
                .len()
                .checked_mul(8)
                .and_then(|size| u32::try_from(size).ok())
                .ok_or(IpcPhysicalBufferTranslationError::AllocationFailure)?;
            if table_size > capacity {
                return Err(
                    IpcPhysicalBufferTranslationError::DestinationBufferTooSmall {
                        capacity,
                        required: table_size,
                    },
                );
            }
            if !self
                .address_space
                .is_writable_range(destination_address, table_size as usize)
            {
                return Err(
                    IpcPhysicalBufferTranslationError::InvalidDestinationBuffer {
                        address: destination_address,
                        capacity,
                    },
                );
            }

            let destination_chunks = self
                .address_space
                .linear_physical_chunks_for_range(destination_address, capacity)
                .ok_or(
                    IpcPhysicalBufferTranslationError::DestinationBufferNotPhysical {
                        address: destination_address,
                    },
                )?;
            if destination_chunks.len() != 1
                || destination_chunks[0].0 & 0xFFF != 0
                || destination_chunks[0].1 < capacity
            {
                return Err(
                    IpcPhysicalBufferTranslationError::DestinationBufferNotPhysical {
                        address: destination_address,
                    },
                );
            }

            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(table_size as usize)
                .map_err(|_| IpcPhysicalBufferTranslationError::AllocationFailure)?;
            for (physical_address, chunk_size) in source_chunks {
                bytes.extend_from_slice(&physical_address.to_le_bytes());
                bytes.extend_from_slice(&chunk_size.to_le_bytes());
            }

            let table_physical_address = destination_chunks[0].0;
            pending.push(PendingPhysicalTable {
                destination_address,
                bytes,
            });
            translated.push((word_offset, table_physical_address));
        }

        for table in pending {
            self.write_memory_bytes(table.destination_address, &table.bytes)
                .map_err(
                    |_| IpcPhysicalBufferTranslationError::DestinationWriteFailed {
                        address: table.destination_address,
                    },
                )?;
        }

        Ok(translated)
    }

    /// Maps every type-5/6/7 request buffer into the receiving thread's address space.
    ///
    /// Type 5 copies readable input bytes into a read/write receiver alias.
    /// Type 6 reserves a zeroed output alias and requires a writable source.
    /// Type 7 stages readable input and verifies source write permission when
    /// the reply completes. All aliases are rolled back if any descriptor fails.
    pub fn map_temporary_buffer_translations_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        destination_thread: ThreadId,
        message: ipc::IpcMessage<'_>,
    ) -> std::result::Result<Vec<Arm11IpcTemporaryBuffer>, IpcTemporaryBufferTranslationError> {
        let parameters = message
            .translation_parameters()
            .map_err(IpcTemporaryBufferTranslationError::TranslationDecode)?;
        let thread_info = self.scheduler.thread_info(destination_thread).ok_or(
            IpcTemporaryBufferTranslationError::UnknownDestinationThread(destination_thread),
        )?;
        if thread_info.state == ThreadState::Exited {
            return Err(IpcTemporaryBufferTranslationError::ExitedDestinationThread(
                destination_thread,
            ));
        }

        let mut mappings: Vec<Arm11IpcTemporaryBuffer> = Vec::new();
        mappings
            .try_reserve_exact(parameters.len())
            .map_err(|_| IpcTemporaryBufferTranslationError::AllocationFailure)?;

        for parameter in parameters {
            let (word_offset, permissions, size_bytes, source_address) = match parameter {
                ipc::IpcTranslationParameter::MappedBuffer {
                    word_offset,
                    permissions,
                    size_bytes,
                    source_address,
                    ..
                } => (word_offset, permissions, size_bytes, source_address),
                _ => continue,
            };

            let source_size = usize::try_from(size_bytes)
                .map_err(|_| IpcTemporaryBufferTranslationError::AllocationFailure)?;
            if permissions & 1 != 0
                && !source
                    .address_space
                    .is_readable_range(source_address, source_size)
            {
                for mapping in mappings.iter().copied() {
                    let _ = self
                        .address_space
                        .unmap_temporary_ipc_buffer(mapping.mapped_address);
                }
                return Err(IpcTemporaryBufferTranslationError::SourceRangeUnavailable {
                    address: source_address,
                    size_bytes,
                });
            }
            if permissions == 2
                && !source
                    .address_space
                    .is_writable_range(source_address, source_size)
            {
                for mapping in mappings.iter().copied() {
                    let _ = self
                        .address_space
                        .unmap_temporary_ipc_buffer(mapping.mapped_address);
                }
                return Err(IpcTemporaryBufferTranslationError::SourceRangeNotWritable {
                    address: source_address,
                    size_bytes,
                });
            }

            let page_offset = source_address & 0xFFF;
            let required_size = u64::from(page_offset) + u64::from(size_bytes);
            let mapping_size = required_size
                .checked_add(0xFFF)
                .map(|size| size & !0xFFF)
                .and_then(|size| u32::try_from(size).ok())
                .ok_or(IpcTemporaryBufferTranslationError::AllocationFailure)?;
            let backing = SharedMemoryBacking::zeroed(mapping_size as usize)
                .map_err(|_| IpcTemporaryBufferTranslationError::AllocationFailure)?;
            let mapped_address =
                match self
                    .address_space
                    .map_temporary_ipc_buffer(backing, page_offset, size_bytes)
                {
                    Ok(address) => address,
                    Err(error) => {
                        for mapping in mappings.iter().copied() {
                            let _ = self
                                .address_space
                                .unmap_temporary_ipc_buffer(mapping.mapped_address);
                        }
                        return Err(IpcTemporaryBufferTranslationError::Mapping(error));
                    }
                };

            if permissions & 1 != 0 {
                let mut bytes = Vec::new();
                if bytes.try_reserve_exact(source_size).is_err() {
                    let _ = self
                        .address_space
                        .unmap_temporary_ipc_buffer(mapped_address);
                    for mapping in mappings.iter().copied() {
                        let _ = self
                            .address_space
                            .unmap_temporary_ipc_buffer(mapping.mapped_address);
                    }
                    return Err(IpcTemporaryBufferTranslationError::AllocationFailure);
                }
                bytes.resize(source_size, 0);
                if source
                    .read_memory_bytes(source_address, &mut bytes)
                    .is_err()
                {
                    let _ = self
                        .address_space
                        .unmap_temporary_ipc_buffer(mapped_address);
                    for mapping in mappings.iter().copied() {
                        let _ = self
                            .address_space
                            .unmap_temporary_ipc_buffer(mapping.mapped_address);
                    }
                    return Err(IpcTemporaryBufferTranslationError::SourceRangeUnavailable {
                        address: source_address,
                        size_bytes,
                    });
                }
                if self.write_memory_bytes(mapped_address, &bytes).is_err() {
                    let _ = self
                        .address_space
                        .unmap_temporary_ipc_buffer(mapped_address);
                    for mapping in mappings.iter().copied() {
                        let _ = self
                            .address_space
                            .unmap_temporary_ipc_buffer(mapping.mapped_address);
                    }
                    return Err(IpcTemporaryBufferTranslationError::DestinationWriteFailed {
                        address: mapped_address,
                    });
                }
            }

            mappings.push(Arm11IpcTemporaryBuffer {
                word_offset,
                source_address,
                mapped_address,
                size_bytes,
                permissions,
            });
        }

        Ok(mappings)
    }

    /// Completes temporary buffer transfers when the receiver sends its IPC reply.
    ///
    /// Writable type-6/type-7 buffers are staged first, then copied back to the
    /// original sender. Type-5 input buffers are never copied back. Every alias
    /// is unmapped even if validation or write-back fails.
    pub fn complete_temporary_buffer_translations_from_ipc(
        &mut self,
        source: &mut Arm11ProcessSession,
        mappings: &[Arm11IpcTemporaryBuffer],
    ) -> std::result::Result<(), IpcTemporaryBufferTranslationError> {
        let transfer_result = (|| {
            let mut pending = Vec::new();
            pending
                .try_reserve_exact(mappings.len())
                .map_err(|_| IpcTemporaryBufferTranslationError::AllocationFailure)?;

            for mapping in mappings.iter().copied() {
                if mapping.permissions & 2 == 0 {
                    continue;
                }
                let size = usize::try_from(mapping.size_bytes)
                    .map_err(|_| IpcTemporaryBufferTranslationError::AllocationFailure)?;
                if !source
                    .address_space
                    .is_writable_range(mapping.source_address, size)
                {
                    return Err(IpcTemporaryBufferTranslationError::SourceRangeNotWritable {
                        address: mapping.source_address,
                        size_bytes: mapping.size_bytes,
                    });
                }
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(size)
                    .map_err(|_| IpcTemporaryBufferTranslationError::AllocationFailure)?;
                bytes.resize(size, 0);
                self.read_memory_bytes(mapping.mapped_address, &mut bytes)
                    .map_err(
                        |_| IpcTemporaryBufferTranslationError::SourceRangeUnavailable {
                            address: mapping.mapped_address,
                            size_bytes: mapping.size_bytes,
                        },
                    )?;
                pending.push((mapping.source_address, mapping.size_bytes, bytes));
            }

            for (address, size_bytes, bytes) in pending {
                source.write_memory_bytes(address, &bytes).map_err(|_| {
                    IpcTemporaryBufferTranslationError::SourceWriteBackFailed {
                        address,
                        size_bytes,
                    }
                })?;
            }
            Ok(())
        })();

        for mapping in mappings.iter().copied() {
            let _ = self
                .address_space
                .unmap_temporary_ipc_buffer(mapping.mapped_address);
        }

        transfer_result
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
        // 0x1000_0000 is the kernel's DONTCARE value for peer permissions.
        // It leaves the block's stored peer permission unchanged.
        let peer_is_dontcare = other_permissions == 0x1000_0000;
        if my_permissions > 3
            || (!peer_is_dontcare && other_permissions > 3)
            || my_permissions == 2
            || other_permissions == 2
            || my_permissions & !owner_permissions != 0
            || (!peer_is_dontcare && other_permissions & !peer_permissions != 0)
        {
            return Err(KernelHandleError::InvalidMemoryPermission);
        }
        // The GSP HLE endpoint maps its returned shared block before replying.
        // Accept the guest's subsequent MapMemoryBlock call only when it names
        // that exact backing at the exact address; all conflicting mappings fail.
        if self.address_space.shared_mapping_matches(address, &backing) {
            return Ok(());
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
        self.service_wait_all_waiters();
        Ok(())
    }

    /// Returns the currently selected thread identifier.
    pub const fn current_thread_id(&self) -> Option<ThreadId> {
        self.scheduler.current_thread_id()
    }

    /// Suspends the current ARM11 thread while an IPC request waits on a service.
    pub fn wait_current_thread_for_ipc(
        &mut self,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        self.scheduler.wait_current()
    }

    /// Re-queues a thread whose asynchronous IPC reply has been written to TLS.
    pub fn wake_thread_for_ipc(
        &mut self,
        thread: ThreadId,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        self.scheduler.wake_thread(thread)
    }

    /// Writes an IPC reply to a specific thread's TLS and sets its SVC result.
    ///
    /// The thread must still own a valid TLS mapping. The scheduler wake is a
    /// separate operation so callers can write a complete reply before it runs.
    pub fn write_ipc_response_to_thread(&mut self, thread: ThreadId, words: &[u32]) -> Result<()> {
        let tls_address = self
            .scheduler
            .core(thread)
            .map(|core| core.cp15().tpidruro())
            .filter(|address| *address != 0)
            .ok_or(crate::Error::InvalidAccess { addr: 0 })?;
        let buffer_address = tls_address
            .checked_add(0x80)
            .ok_or(crate::Error::InvalidAccess { addr: tls_address })?;
        let byte_count = words
            .len()
            .checked_mul(4)
            .ok_or(crate::Error::InvalidAccess {
                addr: buffer_address,
            })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(byte_count)
            .map_err(|_| crate::Error::Io(std::io::Error::other("IPC reply allocation failed")))?;
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        self.write_memory_bytes(buffer_address, &bytes)?;
        let core = self
            .scheduler
            .core_mut(thread)
            .ok_or(crate::Error::InvalidAccess { addr: tls_address })?;
        core.regs[0] = 0;
        Ok(())
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
    /// An asserted FIQ wakes a core in WFI even when CPSR.F masks FIQ
    /// exception entry. In that case the guest resumes without taking FIQ.
    pub fn set_fiq_line(&mut self, asserted: bool) {
        if let Some(core) = self.scheduler.current_core_mut() {
            core.set_fiq_line(asserted);
        }
    }

    /// Signals an external ARMv6K event to the currently running CPU context.
    ///
    /// The system layer also exposes this hook to device and multi-core schedulers.
    pub fn signal_cpu_event(&mut self) -> Result<()> {
        let core = self
            .scheduler
            .current_core_mut()
            .ok_or(crate::Error::Unimplemented("no runnable ARM11 thread"))?;
        core.signal_event();
        Ok(())
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
        let mut event = core.step(&mut self.address_space)?;
        if core.take_exception_return_wakeup() {
            event = StepEvent::ExceptionReturn;
        }
        if !matches!(
            event,
            StepEvent::WaitingForInterrupt | StepEvent::WaitingForEvent
        ) {
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
        // A timer without a wait-any FIFO waiter may still complete a wait-all
        // request. Check object sets before applying deadlines at this instant.
        self.service_wait_all_waiters();
        let expired = self
            .wait_deadlines
            .iter()
            .filter_map(|(thread, (_, deadline))| (*deadline <= now).then_some(*thread))
            .collect::<Vec<_>>();

        for thread in expired {
            if self.wait_deadlines.remove(&thread).is_some() {
                self.multi_waits.remove(&thread);
                self.wait_all_waits.remove(&thread);
                self.kernel_objects.take_handoff(thread);
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
                StepEvent::WaitForEvent => {
                    executed = executed.saturating_add(1);
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::WaitForEvent,
                    });
                }
                StepEvent::WaitingForEvent => {
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::WaitForEvent,
                    });
                }
                StepEvent::SendEvent => {
                    executed = executed.saturating_add(1);
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::SendEvent,
                    });
                }
                StepEvent::ExceptionReturn => {
                    executed = executed.saturating_add(1);
                    return Ok(Arm11RunReport {
                        instructions_executed: executed,
                        stop_reason: Arm11RunStopReason::ExceptionReturn,
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
                        SvcRunDisposition::IpcRequest => {
                            return Ok(Arm11RunReport {
                                instructions_executed: executed,
                                stop_reason: Arm11RunStopReason::IpcRequest,
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
            SVC_CONNECT_TO_PORT => self.svc_connect_to_port(caller),
            SVC_SEND_SYNC_REQUEST => self.svc_send_sync_request(caller),
            SVC_CREATE_MEMORY_BLOCK => self.svc_create_memory_block(caller),
            SVC_CONTROL_MEMORY => self.svc_control_memory(caller),
            SVC_MAP_MEMORY_BLOCK => self.svc_map_memory_block(caller),
            SVC_UNMAP_MEMORY_BLOCK => self.svc_unmap_memory_block(caller),
            SVC_QUERY_MEMORY => self.svc_query_memory(caller),
            0x52 => self.svc_invalidate_process_data_cache(caller),
            0x53 => self.svc_store_process_data_cache(caller),
            0x54 => self.svc_flush_process_data_cache(caller),
            SVC_CREATE_THREAD => self.svc_create_thread(caller),
            SVC_CREATE_MUTEX => self.svc_create_mutex(caller),
            SVC_RELEASE_MUTEX => self.svc_release_mutex(caller),
            SVC_CREATE_SEMAPHORE => self.svc_create_semaphore(caller),
            SVC_RELEASE_SEMAPHORE => self.svc_release_semaphore(caller),
            SVC_WAIT_SYNCHRONIZATION1 => self.svc_wait_synchronization1(caller),
            SVC_WAIT_SYNCHRONIZATION_N => self.svc_wait_synchronization_n(caller),
            SVC_CREATE_TIMER => self.svc_create_timer(caller),
            SVC_SET_TIMER => self.svc_set_timer(caller),
            SVC_CANCEL_TIMER => self.svc_cancel_timer(caller),
            SVC_CLEAR_TIMER => self.svc_clear_timer(caller),
            SVC_CREATE_EVENT => self.svc_create_event(caller),
            SVC_SIGNAL_EVENT => self.svc_signal_event(caller),
            SVC_CLEAR_EVENT => self.svc_clear_event(caller),
            SVC_CLOSE_HANDLE => self.svc_close_handle(caller),
            SVC_DUPLICATE_HANDLE => self.svc_duplicate_handle(caller),
            SVC_GET_HANDLE_INFO => self.svc_get_handle_info(caller),
            SVC_GET_PROCESS_INFO => self.svc_get_process_info(caller),
            SVC_GET_SYSTEM_TICK => self.svc_get_system_tick(caller),
            SVC_GET_THREAD_INFO => self.svc_get_thread_info(caller),

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

    /// Handles ConnectToPort: r0 = output handle pointer, r1 = four-byte port name.
    /// The current bootstrap provides the service-manager port only.
    fn svc_connect_to_port(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let output_address = registers[0];
        let name_address = registers[1];
        if !self.address_space.is_writable_range(output_address, 4) {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }

        let mut port_name = [0u8; 4];
        if self
            .read_memory_bytes(name_address, &mut port_name)
            .is_err()
        {
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        if &port_name != b"srv:" {
            return self.write_svc_result(caller, RESULT_INVALID_ENUM);
        }

        let handle = match self.create_service_manager_session() {
            Ok(handle) => handle,
            Err(error) => return self.write_svc_result(caller, kernel_handle_result_code(error)),
        };
        if self.address_space.write32(output_address, handle).is_err() {
            let _ = self.kernel_objects.close_handle(handle);
            return self.write_svc_result(caller, RESULT_INVALID_ADDRESS);
        }
        self.write_svc_result(caller, 0)
    }

    /// Defers the request to the system-level service dispatcher.
    fn svc_send_sync_request(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs[0];
        if self.kernel_objects.service_session(handle).is_none() {
            return self.write_svc_result(caller, RESULT_INVALID_HANDLE);
        }
        SvcRunDisposition::IpcRequest
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

    /// Handles InvalidateProcessDataCache (0x52), StoreProcessDataCache (0x53),
    /// and FlushProcessDataCache (0x54) for the current-process pseudo-handle.
    fn svc_cache_maintenance(&mut self, caller: ThreadId, operation: u8) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        if registers[0] != CURRENT_PROCESS_HANDLE {
            return self.write_svc_result(caller, RESULT_INVALID_HANDLE);
        }
        let result = match operation {
            0x52 => self.invalidate_process_data_cache(registers[1], registers[2]),
            0x53 => self
                .store_process_data_cache(registers[1], registers[2])
                .and_then(|lines| self.drain_write_buffer().map(|_| lines)),
            0x54 => self
                .flush_process_data_cache(registers[1], registers[2])
                .and_then(|lines| self.drain_write_buffer().map(|_| lines)),
            _ => unreachable!("cache maintenance operation is selected by its SVC number"),
        };
        self.write_svc_result(caller, result.map(|_| 0).unwrap_or(RESULT_INVALID_ADDRESS))
    }

    fn svc_invalidate_process_data_cache(&mut self, caller: ThreadId) -> SvcRunDisposition {
        self.svc_cache_maintenance(caller, 0x52)
    }

    fn svc_store_process_data_cache(&mut self, caller: ThreadId) -> SvcRunDisposition {
        self.svc_cache_maintenance(caller, 0x53)
    }

    fn svc_flush_process_data_cache(&mut self, caller: ThreadId) -> SvcRunDisposition {
        self.svc_cache_maintenance(caller, 0x54)
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

        // A released physical allocation may be reused at the same address.
        // Discard its resident cache lines before removing the alias and freeing
        // the backing, so a dirty victim cannot later overwrite the new owner.
        self.address_space
            .invalidate_data_cache_range(virtual_base, size)
            .map_err(|_| RESULT_INVALID_ADDRESS)?;

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
                        // CreateThread must be rolled back completely if no
                        // kernel handle can be allocated for the new context.
                        let _ = self.terminate_thread_context(id);
                        return self
                            .write_create_thread_error(caller, RESULT_OUT_OF_THREAD_RESOURCES);
                    }
                    Err(_) => {
                        let _ = self.terminate_thread_context(id);
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
        self.terminate_thread_context(caller)
    }

    /// Terminates a thread, abandons its kernel waits and mutexes, signals its
    /// thread objects, then reclaims its CPU context when no thread handle
    /// keeps that context observable.
    fn terminate_thread_context(
        &mut self,
        target: ThreadId,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let info = self
            .scheduler
            .thread_info(target)
            .ok_or(ThreadSchedulerError::UnknownThread(target))?;
        if info.state == ThreadState::Exited {
            return Ok(self.scheduler.current_thread_id());
        }

        if let Some(core) = self.scheduler.core(target) {
            let tls = core.cp15().tpidruro();
            self.address_space.release_thread_local_storage(tls);
        }

        // A forcibly closed thread may have been blocked on any wait shape.
        // Remove it from all queues and bookkeeping before it can be reaped.
        self.kernel_objects.remove_waiter(target);
        self.kernel_objects.take_handoff(target);
        self.wait_deadlines.remove(&target);
        self.multi_waits.remove(&target);
        self.wait_all_waits.remove(&target);

        // Publish Exited before releasing resources or waking observers.
        let next = self.scheduler.terminate_thread(target)?;

        // 3DS ExitThread releases every mutex owned by the exiting thread.
        let mutex_waiters = self.kernel_objects.release_owned_mutexes(target);
        self.wake_synchronization_waiters(mutex_waiters);

        let thread_waiters = self.kernel_objects.signal_thread_exit(target);
        self.wake_synchronization_waiters(thread_waiters);
        self.service_wait_all_waiters();

        if target != MAIN_THREAD_ID && self.kernel_objects.thread_handle_count(target) == 0 {
            self.scheduler.reap_exited_thread(target);
        }

        Ok(self.scheduler.current_thread_id().or(next))
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
                self.service_wait_all_waiters();
                self.write_svc_result(caller, 0)
            }
            Ok(None) => {
                self.service_wait_all_waiters();
                self.write_svc_result(caller, 0)
            }
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
        self.service_wait_all_waiters();
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

    /// Handles the raw ARM11 SVC ABI for WaitSynchronizationN.
    ///
    /// The libctru C wrapper keeps its `out` pointer in a callee-saved register
    /// and writes the returned `r1` value through that pointer after the SVC.
    /// At the SVC boundary, `r0/r4` hold the signed 64-bit timeout (low/high),
    /// `r1` is the guest handle-array pointer, `r2` is the handle count and
    /// `r3` is the wait-all flag. The kernel returns the result in `r0` and
    /// the selected index in `r1`. Since the wrapper stores `r1` even when
    /// the SVC fails, the handler initializes it to zero before validation.
    ///
    /// Wait-all preflights and acquires the complete set atomically. Blocked
    /// requests are rechecked when an object changes state and do not consume
    /// one-shot signals until every handle is ready.
    fn svc_wait_synchronization_n(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("caller context exists")
            .regs;
        let handles_address = registers[1];
        let count = registers[2] as i32;
        let wait_all = registers[3] != 0;
        let timeout = (u64::from(registers[0]) | (u64::from(registers[4]) << 32)) as i64;

        // The raw SVC ABI returns the selected index in r1. Initialize it
        // before any validation because the guest's libctru veneer copies r1
        // to its public out pointer even when the service returns an error.
        if let Some(core) = self.scheduler.core_mut(caller) {
            core.regs[1] = 0;
        }

        if handles_address == 0 {
            return self.write_svc_result(caller, RESULT_INVALID_POINTER);
        }
        if !(0..=0x40).contains(&count) {
            return self.write_svc_result(caller, RESULT_OUT_OF_RANGE);
        }
        if timeout < -1 {
            return self.write_svc_result(caller, RESULT_INVALID_TIMEOUT);
        }
        let mut handles = Vec::with_capacity(count as usize);
        for index in 0..count as u32 {
            match self
                .address_space
                .read32(handles_address.wrapping_add(index * 4))
            {
                Ok(handle) => handles.push(handle),
                Err(_) => return self.write_svc_result(caller, RESULT_INVALID_ADDRESS),
            }
        }

        if wait_all {
            match self
                .kernel_objects
                .try_wait_synchronization_all(&handles, caller)
            {
                Ok(true) => return self.write_svc_result(caller, 0),
                Ok(false) => {}
                Err(error) => {
                    return self.write_svc_result(caller, kernel_handle_result_code(error));
                }
            }

            if timeout == 0 {
                return self.write_svc_result(caller, RESULT_TIMEOUT);
            }

            if let Err(error) = self
                .kernel_objects
                .retain_wait_all_handles(&handles, caller)
            {
                return self.write_svc_result(caller, kernel_handle_result_code(error));
            }
            self.wait_all_waits.insert(caller, handles.clone());
            if timeout > 0 {
                let deadline = self
                    .scheduler
                    .virtual_time_ns()
                    .saturating_add(timeout as u64);
                let first = handles.first().copied().unwrap_or(0);
                self.wait_deadlines.insert(caller, (first, deadline));
            }

            return match self.scheduler.wait_current() {
                Ok(Some(_)) => SvcRunDisposition::Resume,
                Ok(None) => SvcRunDisposition::NoRunnableThread,
                Err(_) => {
                    self.wait_deadlines.remove(&caller);
                    self.wait_all_waits.remove(&caller);
                    self.kernel_objects.release_wait_all_handles(caller);
                    SvcRunDisposition::KernelServiceError {
                        number: SVC_WAIT_SYNCHRONIZATION_N,
                        result: RESULT_INVALID_ENUM,
                    }
                }
            };
        }

        for (index, handle) in handles.iter().enumerate() {
            match self
                .kernel_objects
                .try_wait_synchronization(*handle, caller)
            {
                Ok(true) => {
                    if let Some(core) = self.scheduler.core_mut(caller) {
                        core.regs[1] = index as u32;
                    }
                    return self.write_svc_result(caller, 0);
                }
                Ok(false) => {}
                Err(error) => {
                    return self.write_svc_result(caller, kernel_handle_result_code(error));
                }
            }
        }

        if timeout == 0 {
            return self.write_svc_result(caller, RESULT_TIMEOUT);
        }
        for handle in &handles {
            if let Err(error) = self.kernel_objects.enqueue_waiter(*handle, caller) {
                self.kernel_objects.remove_waiter(caller);
                return self.write_svc_result(caller, kernel_handle_result_code(error));
            }
        }
        self.multi_waits.insert(caller, handles.clone());
        if timeout > 0 {
            let deadline = self
                .scheduler
                .virtual_time_ns()
                .saturating_add(timeout as u64);
            let first = handles.first().copied().unwrap_or(0);
            self.wait_deadlines.insert(caller, (first, deadline));
        }

        match self.scheduler.wait_current() {
            Ok(Some(_)) => SvcRunDisposition::Resume,
            Ok(None) => SvcRunDisposition::NoRunnableThread,
            Err(_) => {
                self.wait_deadlines.remove(&caller);
                self.multi_waits.remove(&caller);
                self.kernel_objects.remove_waiter(caller);
                SvcRunDisposition::KernelServiceError {
                    number: SVC_WAIT_SYNCHRONIZATION_N,
                    result: RESULT_INVALID_ENUM,
                }
            }
        }
    }

    /// Completes an object handoff and makes its blocked thread(s) schedulable.
    fn wake_synchronization_waiters(&mut self, waiters: impl IntoIterator<Item = ThreadId>) {
        for waiter in waiters {
            self.wait_deadlines.remove(&waiter);
            let released_by = self.kernel_objects.take_handoff(waiter);
            let index = self.multi_waits.remove(&waiter).map(|handles| {
                released_by
                    .and_then(|handle| handles.iter().position(|candidate| *candidate == handle))
                    .unwrap_or(0) as u32
            });
            // Drop this wait's references from every object. This is required
            // even for WaitSynchronization1, whose queue contains only one
            // waiter entry and may now live in the retired-object table.
            self.kernel_objects.remove_waiter(waiter);
            if let Some(core) = self.scheduler.core_mut(waiter) {
                core.regs[0] = 0;
                if let Some(index) = index {
                    core.regs[1] = index;
                }
            }
            let _ = self.scheduler.wake_thread(waiter);
        }
    }

    /// Rechecks blocked wait-all requests after synchronization state changes.
    ///
    /// Each successful request acquires its complete handle set atomically.
    /// Failed readiness checks leave every object untouched; invalidated
    /// handles complete the request with the corresponding kernel error.
    fn service_wait_all_waiters(&mut self) {
        let waiting_threads = self.wait_all_waits.keys().copied().collect::<Vec<_>>();
        let mut completed = Vec::new();

        for thread in waiting_threads {
            let Some(handles) = self.wait_all_waits.get(&thread).cloned() else {
                continue;
            };

            match self
                .kernel_objects
                .try_wait_synchronization_all(&handles, thread)
            {
                Ok(true) => completed.push((thread, 0)),
                Ok(false) => {}
                Err(error) => completed.push((thread, kernel_handle_result_code(error))),
            }
        }

        for (thread, result) in completed {
            self.wait_all_waits.remove(&thread);
            self.wait_deadlines.remove(&thread);
            self.kernel_objects.release_wait_all_handles(thread);
            if let Some(core) = self.scheduler.core_mut(thread) {
                core.regs[0] = result;
                // Wait-all has no single selected handle index.
                core.regs[1] = 0;
            }
            let _ = self.scheduler.wake_thread(thread);
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
                self.service_wait_all_waiters();
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
        if !self.kernel_objects.is_open_handle(handle) {
            return self.write_svc_result(caller, RESULT_INVALID_HANDLE);
        }
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

    /// Handles GetHandleInfo using r1 = handle and r2 = information type.
    ///
    /// The raw SVC returns its 64-bit output in r1-r2 and the Result in r0.
    /// Type 0 returns the creation tick for the current-process pseudo-handle;
    /// for another valid object, the kernel leaves the r5-r6 output seed intact.
    /// Type 1 reports the modeled object refcount, sign-extended to 64 bits.
    /// Type 2 is unimplemented by the 3DS kernel and likewise preserves r5-r6.
    /// Type 0x32107 follows the documented constant-zero behavior.
    fn svc_get_handle_info(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs;
        let handle = registers[1];
        let info_type = registers[2];
        let output_seed = u64::from(registers[5]) | (u64::from(registers[6]) << 32);
        let valid_handle = handle == CURRENT_PROCESS_HANDLE
            || handle == CURRENT_THREAD_HANDLE
            || self.kernel_objects.is_open_handle(handle);

        let (result, value) = if !valid_handle {
            (RESULT_INVALID_HANDLE, None)
        } else {
            match info_type {
                0 if handle == CURRENT_PROCESS_HANDLE => (0, Some(0)),
                // Non-process handles do not have process creation timestamps.
                0 | 2 => (0, Some(output_seed)),
                1 => match self.kernel_objects.handle_reference_count(handle) {
                    Ok(count) => {
                        let signed_count = i64::from(count as i32);
                        (0, Some(signed_count as u64))
                    }
                    Err(error) => (kernel_handle_result_code(error), None),
                },
                0x0003_2107 => (0, Some(0)),
                _ => (RESULT_INVALID_ENUM, None),
            }
        };

        if let Some(value) = value {
            if let Some(core) = self.scheduler.core_mut(caller) {
                core.regs[1] = value as u32;
                core.regs[2] = (value >> 32) as u32;
            }
        }
        self.write_svc_result(caller, result)
    }

    /// Handles GetSystemTick and returns a 64-bit counter in r0-r1.
    ///
    /// The emulated counter is derived from deterministic scheduler virtual time,
    /// not host wall-clock time. The base ARM11 tick rate is 268,111,856 Hz.
    fn svc_get_system_tick(&mut self, caller: ThreadId) -> SvcRunDisposition {
        const TICKS_PER_SECOND: u128 = 268_111_856;
        let elapsed_ns = u128::from(self.scheduler.virtual_time_ns());
        let ticks = (elapsed_ns * TICKS_PER_SECOND) / 1_000_000_000;

        let core = self
            .scheduler
            .core_mut(caller)
            .expect("current thread context exists");
        core.regs[0] = ticks as u32;
        core.regs[1] = (ticks >> 32) as u32;
        SvcRunDisposition::Resume
    }

    /// Handles GetThreadInfo using r1 = thread handle and r2 = information type.
    ///
    /// Current 3DS kernels always reject this SVC after checking the handle type:
    /// thread handles return InvalidEnum, while non-thread/invalid handles return
    /// InvalidHandle. The information type and output registers are not consumed.
    fn svc_get_thread_info(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[1];
        let result = if self.kernel_objects.thread_id(handle, caller).is_some() {
            RESULT_INVALID_ENUM
        } else {
            RESULT_INVALID_HANDLE
        };
        self.write_svc_result(caller, result)
    }

    /// Handles GetProcessInfo using r1 = process handle and r2 = information type.
    ///
    /// The raw SVC returns the Result in r0 and the 64-bit output in r1-r2.
    /// Types 4 and 5 expose the current and peak counts of regular guest-visible
    /// handles for the current process. Process memory statistics (types 0-3)
    /// are not modelled yet and are rejected instead of returning guessed data.
    fn svc_get_process_info(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let registers = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs;
        let process = registers[1];
        let info_type = registers[2];

        let (result, value) = if process != CURRENT_PROCESS_HANDLE {
            (RESULT_INVALID_HANDLE, 0)
        } else {
            match info_type {
                4 => (0, self.kernel_objects.handle_count() as u64),
                5 => (0, self.kernel_objects.handle_high_water_mark() as u64),
                _ => (RESULT_INVALID_ENUM, 0),
            }
        };

        if let Some(core) = self.scheduler.core_mut(caller) {
            core.regs[1] = value as u32;
            core.regs[2] = (value >> 32) as u32;
        }
        self.write_svc_result(caller, result)
    }

    /// Handles DuplicateHandle using the input handle in r1.
    ///
    /// The raw SVC ABI returns the result in r0 and the duplicate handle in r1.
    /// The guest-facing libctru wrapper writes r1 through its output pointer
    /// after the SVC; the pointer is not itself passed in r0 at this boundary.
    fn svc_duplicate_handle(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let original = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[1];

        // Initialize r1 on failure because the ABI exposes it as an output.
        if let Some(core) = self.scheduler.core_mut(caller) {
            core.regs[1] = 0;
        }

        match self.kernel_objects.duplicate_handle(original) {
            Ok(duplicate) => {
                if let Some(core) = self.scheduler.core_mut(caller) {
                    core.regs[1] = duplicate;
                }
                self.write_svc_result(caller, 0)
            }
            Err(error) => self.write_svc_result(caller, kernel_handle_result_code(error)),
        }
    }

    /// Handles CloseHandle using the process-local object handle in r0.
    fn svc_close_handle(&mut self, caller: ThreadId) -> SvcRunDisposition {
        let handle = self
            .scheduler
            .core(caller)
            .expect("current thread context exists")
            .regs[0];
        let result = self
            .close_handle(handle)
            .map(|()| 0)
            .unwrap_or_else(kernel_handle_result_code);
        // Closing the final handle to the calling thread terminates it. In
        // that case there is no guest context to receive a return value.
        let caller_exited = self
            .scheduler
            .thread_info(caller)
            .is_none_or(|info| info.state == ThreadState::Exited);
        if caller_exited {
            return if self.scheduler.current_thread_id().is_some() {
                SvcRunDisposition::Resume
            } else {
                SvcRunDisposition::NoRunnableThread
            };
        }
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
        | Arm11DynamicMemoryError::AddressRangeOverflow { .. }
        | Arm11DynamicMemoryError::WritebackFailure(_) => RESULT_INVALID_ADDRESS,
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
    use super::kernel::{CURRENT_PROCESS_HANDLE, CURRENT_THREAD_HANDLE};
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
    fn flush_process_data_cache_svc_uses_current_process_pseudo_handle() {
        let mut session = test_session();
        let mut memory = GpuPhysicalMemory::new();
        let physical = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_address = physical - 0x0C00_0000;
        session.address_space.attach_gpu_memory(memory.clone());
        session
            .address_space
            .map_linear_memory_region(virtual_address, physical, 0x1000)
            .unwrap();
        let baseline_cache_lines = session.data_cache_line_count();
        session
            .write_memory_bytes(virtual_address, &[0x12, 0x34, 0x56, 0x78])
            .unwrap();
        assert_eq!(memory.read_u32(physical).unwrap(), 0);

        session.set_current_register(0, CURRENT_PROCESS_HANDLE);
        session.set_current_register(1, virtual_address);
        session.set_current_register(2, 4);
        session.dispatch_svc(0x54);

        assert_eq!(session.current_registers().unwrap()[0], 0);
        assert_eq!(memory.read_u32(physical).unwrap(), 0x7856_3412);
        assert_eq!(session.data_cache_line_count(), baseline_cache_lines);
    }

    #[test]
    fn store_and_invalidate_process_data_cache_svcs_have_distinct_effects() {
        let mut session = test_session();
        let mut memory = GpuPhysicalMemory::new();
        let physical = memory.allocate_linear_fcram(0x1000).unwrap();
        let virtual_address = physical - 0x0C00_0000;
        session.address_space.attach_gpu_memory(memory.clone());
        session
            .address_space
            .map_linear_memory_region(virtual_address, physical, 0x1000)
            .unwrap();

        let baseline_cache_lines = session.data_cache_line_count();
        session
            .write_memory_bytes(virtual_address, &[0x10, 0x20, 0x30, 0x40])
            .unwrap();
        session.set_current_register(0, CURRENT_PROCESS_HANDLE);
        session.set_current_register(1, virtual_address);
        session.set_current_register(2, 4);
        session.dispatch_svc(0x53); // StoreProcessDataCache
        assert_eq!(session.current_registers().unwrap()[0], 0);
        assert_eq!(memory.read_u32(physical).unwrap(), 0x4030_2010);
        assert_eq!(session.data_cache_line_count(), baseline_cache_lines + 1);

        // A device write is hidden by the still-resident clean CPU line.
        memory.write_u32(physical, 0xAABB_CCDD).unwrap();
        assert_eq!(
            session.read_memory_u32(virtual_address).unwrap(),
            0x4030_2010
        );

        session.set_current_register(0, CURRENT_PROCESS_HANDLE);
        session.set_current_register(1, virtual_address);
        session.set_current_register(2, 4);
        session.dispatch_svc(0x52); // InvalidateProcessDataCache
        assert_eq!(session.current_registers().unwrap()[0], 0);
        assert_eq!(session.data_cache_line_count(), baseline_cache_lines);
        assert_eq!(
            session.read_memory_u32(virtual_address).unwrap(),
            0xAABB_CCDD
        );

        // Invalidating a dirty line discards it rather than writing it back.
        session
            .write_memory_u32(virtual_address, 0xDEAD_BEEF)
            .unwrap();
        assert_eq!(memory.read_u32(physical).unwrap(), 0xAABB_CCDD);
        session.set_current_register(0, CURRENT_PROCESS_HANDLE);
        session.set_current_register(1, virtual_address);
        session.set_current_register(2, 4);
        session.dispatch_svc(0x52);
        assert_eq!(session.current_registers().unwrap()[0], 0);
        assert_eq!(memory.read_u32(physical).unwrap(), 0xAABB_CCDD);
        assert_eq!(
            session.read_memory_u32(virtual_address).unwrap(),
            0xAABB_CCDD
        );

        // The current-process pseudo-handle is checked, not silently ignored.
        session.set_current_register(0, CURRENT_THREAD_HANDLE);
        session.set_current_register(1, virtual_address);
        session.set_current_register(2, 4);
        session.dispatch_svc(0x54);
        assert_eq!(
            session.current_registers().unwrap()[0],
            RESULT_INVALID_HANDLE
        );
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
    fn a32_wfe_stops_session_until_an_external_cpu_event_arrives() {
        let mut session = test_session_with_program(&[0xE320_F002, 0xE3A0_0007]);

        let entered = session.run_instructions(8).unwrap();
        assert_eq!(entered.instructions_executed, 1);
        assert_eq!(entered.stop_reason, Arm11RunStopReason::WaitForEvent);
        assert_eq!(session.core().pc(), 0x0010_0004);
        assert_eq!(session.instruction_count(), 1);

        let asleep = session.run_instructions(8).unwrap();
        assert_eq!(asleep.instructions_executed, 0);
        assert_eq!(asleep.stop_reason, Arm11RunStopReason::WaitForEvent);
        assert_eq!(session.core().pc(), 0x0010_0004);
        assert_eq!(session.instruction_count(), 1);

        session.signal_cpu_event().unwrap();
        let resumed = session.run_instructions(1).unwrap();
        assert_eq!(resumed.instructions_executed, 1);
        assert_eq!(
            resumed.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 7);
        assert_eq!(session.core().pc(), 0x0010_0008);
        assert_eq!(session.instruction_count(), 2);
    }

    #[test]
    fn a32_sev_is_reported_to_the_system_scheduler() {
        let mut session = test_session_with_program(&[0xE320_F004, 0xE3A0_0007]);

        let report = session.run_instructions(8).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(report.stop_reason, Arm11RunStopReason::SendEvent);
        assert_eq!(session.core().pc(), 0x0010_0004);

        let resumed = session.run_instructions(1).unwrap();
        assert_eq!(resumed.instructions_executed, 1);
        assert_eq!(
            resumed.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.core().regs[0], 7);
    }

    #[test]
    fn exception_return_is_reported_to_the_mpcore_scheduler() {
        let mut session = test_session_with_program(&[0xE1B0_F00E]);
        let core = session.scheduler.current_core_mut().unwrap();
        core.switch_mode(crate::cpu::Psr::SUPERVISOR_MODE).unwrap();
        core.set_saved_program_status(crate::cpu::Psr(crate::cpu::Psr::USER_MODE))
            .unwrap();
        core.regs[14] = 0x0010_0040;

        let report = session.run_instructions(8).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(report.stop_reason, Arm11RunStopReason::ExceptionReturn);
        assert_eq!(session.core().pc(), 0x0010_0040);
        assert_eq!(session.core().cpsr.mode(), crate::cpu::Psr::USER_MODE);
        assert_eq!(session.instruction_count(), 1);
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
        owner.write_memory_u32(0x0800_0000, 0xCAFE_BABE).unwrap();

        let mut peer = test_session();
        let peer_handle = peer
            .import_memory_block_handle(&owner, owner_handle)
            .unwrap();
        owner.kernel_objects.close_handle(owner_handle).unwrap();
        peer.map_memory_block(peer_handle, 0x0900_0000, 1, 3)
            .unwrap();

        // The imported process has its own cache. The owner's dirty line is
        // not visible until the owner cleans and drains its write buffer.
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0);
        owner.flush_process_data_cache(0x0800_0000, 4).unwrap();
        owner.drain_write_buffer().unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0);
        peer.invalidate_process_data_cache(0x0900_0000, 4).unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0xCAFE_BABE);
        assert!(!peer.address_space.is_writable_range(0x0900_0000, 4));
        assert!(peer.write_memory_u32(0x0900_0000, 0xDEAD_BEEF).is_err());

        owner.write_memory_u32(0x0800_0000, 0x1234_5678).unwrap();
        owner.flush_process_data_cache(0x0800_0000, 4).unwrap();
        owner.drain_write_buffer().unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0xCAFE_BABE);
        peer.invalidate_process_data_cache(0x0900_0000, 4).unwrap();
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
        owner.write_memory_u32(0x0800_0000, 0xCAFE_BABE).unwrap();

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
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0);
        owner.flush_process_data_cache(0x0800_0000, 4).unwrap();
        owner.drain_write_buffer().unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0);
        peer.invalidate_process_data_cache(0x0900_0000, 4).unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0xCAFE_BABE);

        owner.write_memory_u32(0x0800_0000, 0x1234_5678).unwrap();
        owner.flush_process_data_cache(0x0800_0000, 4).unwrap();
        owner.drain_write_buffer().unwrap();
        assert_eq!(peer.read_memory_u32(0x0900_0000).unwrap(), 0xCAFE_BABE);
        peer.invalidate_process_data_cache(0x0900_0000, 4).unwrap();
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

        assert!(
            source
                .kernel_objects
                .memory_block_backing(source_handle)
                .is_ok()
        );
        assert!(
            destination
                .kernel_objects
                .memory_block_backing(copied)
                .is_ok()
        );
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
        assert!(
            destination
                .kernel_objects
                .memory_block_backing(moved)
                .is_ok()
        );
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
    fn ipc_copy_handle_array_translates_memory_blocks_and_preserves_null_entries() {
        let mut source = test_session();
        let first = source.create_memory_block(0x1000, 3, 1).unwrap();
        let second = source.create_memory_block(0x1000, 1, 3).unwrap();
        let mut destination = test_session();

        let translated = destination
            .translate_memory_block_handles_from_ipc(
                &mut source,
                0x0800_0000, // COPY three handles.
                &[first, 0, second],
            )
            .unwrap();

        assert_eq!(translated.len(), 3);
        let copied_first = translated[0].unwrap();
        assert_eq!(translated[1], None);
        let copied_second = translated[2].unwrap();
        assert!(source.kernel_objects.memory_block_backing(first).is_ok());
        assert!(source.kernel_objects.memory_block_backing(second).is_ok());
        assert_eq!(
            destination
                .kernel_objects
                .memory_block_permissions(copied_first),
            Some((1, 3))
        );
        assert_eq!(
            destination
                .kernel_objects
                .memory_block_permissions(copied_second),
            Some((3, 1))
        );
    }

    #[test]
    fn ipc_move_handle_array_closes_all_sources_after_importing_destinations() {
        let mut source = test_session();
        let first = source.create_memory_block(0x1000, 3, 1).unwrap();
        let second = source.create_memory_block(0x1000, 1, 3).unwrap();
        let mut destination = test_session();

        let translated = destination
            .translate_memory_block_handles_from_ipc(
                &mut source,
                0x0800_0010, // MOVE three handles.
                &[first, 0, second],
            )
            .unwrap();

        assert_eq!(translated.len(), 3);
        let moved_first = translated[0].unwrap();
        assert_eq!(translated[1], None);
        let moved_second = translated[2].unwrap();
        assert_eq!(
            source.kernel_objects.memory_block_backing(first),
            Err(KernelHandleError::InvalidHandle)
        );
        assert_eq!(
            source.kernel_objects.memory_block_backing(second),
            Err(KernelHandleError::InvalidHandle)
        );
        assert!(
            destination
                .kernel_objects
                .memory_block_backing(moved_first)
                .is_ok()
        );
        assert!(
            destination
                .kernel_objects
                .memory_block_backing(moved_second)
                .is_ok()
        );
    }

    #[test]
    fn ipc_handle_array_validates_every_source_before_creating_destination_handles() {
        let mut source = test_session();
        let memory_block = source.create_memory_block(0x1000, 3, 1).unwrap();
        let event = source
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut destination = test_session();

        assert_eq!(
            destination.translate_memory_block_handles_from_ipc(
                &mut source,
                0x0400_0000, // COPY two handles.
                &[memory_block, event],
            ),
            Err(kernel::IpcHandleTranslationError::Kernel(
                KernelHandleError::WrongType
            ))
        );
        assert_eq!(destination.kernel_objects.handle_count(), 0);
        assert!(
            source
                .kernel_objects
                .memory_block_backing(memory_block)
                .is_ok()
        );
    }

    #[test]
    fn ipc_move_handle_array_rejects_duplicate_non_null_source_handles() {
        let mut source = test_session();
        let handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();

        assert_eq!(
            destination.translate_memory_block_handles_from_ipc(
                &mut source,
                0x0400_0010, // MOVE two handles.
                &[handle, handle],
            ),
            Err(kernel::IpcHandleTranslationError::DuplicateMoveHandle { handle })
        );
        assert_eq!(destination.kernel_objects.handle_count(), 0);
        assert!(source.kernel_objects.memory_block_backing(handle).is_ok());
    }

    #[test]
    fn ipc_handle_array_rejects_descriptor_count_mismatch() {
        let mut source = test_session();
        let handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();

        assert_eq!(
            destination.translate_memory_block_handles_from_ipc(
                &mut source,
                0x0400_0000, // Descriptor declares two handles.
                &[handle],
            ),
            Err(kernel::IpcHandleTranslationError::HandleCountMismatch {
                expected: 2,
                actual: 1,
            })
        );
        assert_eq!(destination.kernel_objects.handle_count(), 0);
        assert!(source.kernel_objects.memory_block_backing(handle).is_ok());
    }

    #[test]
    fn ipc_message_translation_section_moves_memory_block_handle_array() {
        let mut source = test_session();
        let first = source.create_memory_block(0x1000, 3, 1).unwrap();
        let second = source.create_memory_block(0x1000, 1, 3).unwrap();
        let mut destination = test_session();
        let words = [
            0x1234_0004, // Command 0x1234, four translation words.
            0x0800_0010, // MOVE three handles.
            first,
            0,
            second,
        ];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        let translated = destination
            .translate_memory_block_translation_section_from_ipc(&mut source, message)
            .unwrap();

        assert_eq!(translated.len(), 3);
        assert!(translated[0].is_some());
        assert_eq!(translated[1], None);
        assert!(translated[2].is_some());
        assert_eq!(
            source.kernel_objects.memory_block_backing(first),
            Err(KernelHandleError::InvalidHandle)
        );
        assert_eq!(
            source.kernel_objects.memory_block_backing(second),
            Err(KernelHandleError::InvalidHandle)
        );

        let empty_message = ipc::IpcMessage::decode(&[0x1234_0000]).unwrap();
        assert_eq!(
            destination
                .translate_memory_block_translation_section_from_ipc(&mut source, empty_message,),
            Err(kernel::IpcHandleTranslationError::InvalidDescriptor)
        );
    }

    #[test]
    fn ipc_message_translation_section_translates_multiple_handle_groups() {
        let mut source = test_session();
        let copy_source = source.create_memory_block(0x1000, 3, 1).unwrap();
        let move_source = source.create_memory_block(0x1000, 1, 3).unwrap();
        let mut destination = test_session();

        // COPY one handle, then a static-buffer descriptor, then MOVE one handle.
        let words = [
            0x1234_0006,
            0x0000_0000,
            copy_source,
            0x0001_4002,
            0x0020_0000,
            0x0000_0010,
            move_source,
        ];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        let groups = destination
            .translate_memory_block_handle_groups_from_ipc_message(&mut source, message)
            .unwrap();

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].word_offset, 0);
        assert_eq!(groups[1].word_offset, 4);
        assert_eq!(groups[0].handles.len(), 1);
        assert_eq!(groups[1].handles.len(), 1);

        let copied = groups[0].handles[0].unwrap();
        let moved = groups[1].handles[0].unwrap();
        assert!(
            source
                .kernel_objects
                .memory_block_backing(copy_source)
                .is_ok()
        );
        assert_eq!(
            source.kernel_objects.memory_block_backing(move_source),
            Err(KernelHandleError::InvalidHandle)
        );
        assert!(
            destination
                .kernel_objects
                .memory_block_backing(copied)
                .is_ok()
        );
        assert!(
            destination
                .kernel_objects
                .memory_block_backing(moved)
                .is_ok()
        );
    }

    #[test]
    fn ipc_message_translation_section_rejects_duplicate_moves_across_groups_atomically() {
        let mut source = test_session();
        let handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();

        let words = [0x1234_0004, 0x0000_0010, handle, 0x0000_0010, handle];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        assert_eq!(
            destination.translate_memory_block_handle_groups_from_ipc_message(&mut source, message),
            Err(kernel::IpcHandleTranslationError::DuplicateMoveHandle { handle })
        );
        assert_eq!(destination.kernel_objects.handle_count(), 0);
        assert!(source.kernel_objects.memory_block_backing(handle).is_ok());
    }

    #[test]
    fn ipc_static_buffer_copy_uses_receiving_thread_tls_descriptor() {
        let mut source = test_session();
        source
            .write_memory_bytes(0x0020_0000, &[1, 2, 3, 4, 5])
            .unwrap();
        let mut destination = test_session();
        let tls = destination
            .scheduler
            .core(MAIN_THREAD_ID)
            .unwrap()
            .cp15()
            .tpidruro();
        destination
            .write_memory_bytes(0x0020_1000, &[0xAA; 8])
            .unwrap();
        destination
            .write_memory_u32(tls + 0x180, 0x0020_1000)
            .unwrap();
        destination.write_memory_u32(tls + 0x184, 8).unwrap();

        let words = [0x1234_0002, 0x0001_4002, 0x0020_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();
        destination
            .copy_static_buffer_translation_from_ipc(&mut source, MAIN_THREAD_ID, message)
            .unwrap();

        let mut copied = [0; 8];
        destination
            .read_memory_bytes(0x0020_1000, &mut copied)
            .unwrap();
        assert_eq!(copied, [1, 2, 3, 4, 5, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn ipc_static_buffer_copy_rejects_small_receiver_capacity_without_writing() {
        let mut source = test_session();
        source
            .write_memory_bytes(0x0020_0000, &[1, 2, 3, 4, 5])
            .unwrap();
        let mut destination = test_session();
        let tls = destination
            .scheduler
            .core(MAIN_THREAD_ID)
            .unwrap()
            .cp15()
            .tpidruro();
        destination
            .write_memory_bytes(0x0020_1000, &[0xAA; 8])
            .unwrap();
        destination
            .write_memory_u32(tls + 0x180, 0x0020_1000)
            .unwrap();
        destination.write_memory_u32(tls + 0x184, 4).unwrap();

        let words = [0x1234_0002, 0x0001_4002, 0x0020_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();
        assert_eq!(
            destination.copy_static_buffer_translation_from_ipc(
                &mut source,
                MAIN_THREAD_ID,
                message,
            ),
            Err(IpcStaticBufferTranslationError::DestinationBufferTooSmall {
                buffer_index: 0,
                capacity: 4,
                requested: 5,
            })
        );

        let mut unchanged = [0; 8];
        destination
            .read_memory_bytes(0x0020_1000, &mut unchanged)
            .unwrap();
        assert_eq!(unchanged, [0xAA; 8]);
    }

    #[test]
    fn ipc_static_buffer_copy_stages_all_sources_before_writing_any_destination() {
        let mut source = test_session();
        source
            .write_memory_bytes(0x0020_0000, &[1, 2, 3, 4])
            .unwrap();
        let mut destination = test_session();
        let tls = destination
            .scheduler
            .core(MAIN_THREAD_ID)
            .unwrap()
            .cp15()
            .tpidruro();
        destination
            .write_memory_bytes(0x0020_1000, &[0xAA; 16])
            .unwrap();
        destination
            .write_memory_u32(tls + 0x180, 0x0020_1000)
            .unwrap();
        destination.write_memory_u32(tls + 0x184, 4).unwrap();
        destination
            .write_memory_u32(tls + 0x188, 0x0020_1008)
            .unwrap();
        destination.write_memory_u32(tls + 0x18C, 4).unwrap();

        let words = [
            0x1234_0004,
            0x0001_0002,
            0x0020_0000,
            0x0001_0402,
            0x0F00_0000,
        ];
        let message = ipc::IpcMessage::decode(&words).unwrap();
        assert_eq!(
            destination.copy_static_buffer_translation_from_ipc(
                &mut source,
                MAIN_THREAD_ID,
                message,
            ),
            Err(IpcStaticBufferTranslationError::SourceRangeUnavailable {
                address: 0x0F00_0000,
                size_bytes: 4,
            })
        );

        let mut unchanged = [0; 16];
        destination
            .read_memory_bytes(0x0020_1000, &mut unchanged)
            .unwrap();
        assert_eq!(unchanged, [0xAA; 16]);
    }

    #[test]
    fn ipc_static_buffer_copy_rejects_non_static_translation_groups() {
        let mut source = test_session();
        let mut destination = test_session();
        let words = [0x1234_0002, 0x0000_010A, 0x0020_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();
        assert_eq!(
            destination.copy_static_buffer_translation_from_ipc(
                &mut source,
                MAIN_THREAD_ID,
                message,
            ),
            Err(
                IpcStaticBufferTranslationError::UnsupportedTranslationParameter {
                    descriptor: 0x0000_010A,
                }
            )
        );
    }

    #[test]
    fn ipc_type_two_builds_physical_chunk_table_in_receiver_static_buffer() {
        let mut source = test_session();
        let mut destination = test_session();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let destination_physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        assert_eq!(destination_physical_base, physical_base + 0x1000);
        let dma = GpuDmaAddressSpace::new();
        source.attach_gpu_services(memory.clone(), dma.clone());
        destination.attach_gpu_services(memory.clone(), dma);

        source
            .address_space
            .map_linear_memory_region(0x1400_0000, physical_base, 0x1000)
            .unwrap();
        destination
            .address_space
            .map_linear_memory_region(0x1400_1000, destination_physical_base, 0x1000)
            .unwrap();
        source
            .write_memory_bytes(0x1400_0100, &[0x5A; 0x40])
            .unwrap();

        let tls = destination
            .scheduler
            .core(MAIN_THREAD_ID)
            .unwrap()
            .cp15()
            .tpidruro();
        destination
            .write_memory_u32(tls + 0x180 + 3 * 8, 0x1400_1000)
            .unwrap();
        destination
            .write_memory_u32(tls + 0x184 + 3 * 8, 0x1000)
            .unwrap();

        let descriptor = (0x40u32 << 8) | (3 << 4) | 0x04;
        let words = [0x1234_0002, descriptor, 0x1400_0100];
        let message = ipc::IpcMessage::decode(&words).unwrap();
        let translated = destination
            .translate_physical_buffer_tables_from_ipc(&source, MAIN_THREAD_ID, message)
            .unwrap();

        assert_eq!(translated, vec![(0, destination_physical_base)]);
        assert_eq!(
            destination.read_memory_u32(0x1400_1000).unwrap(),
            physical_base + 0x100
        );
        assert_eq!(destination.read_memory_u32(0x1400_1004).unwrap(), 0x40);

        // The receiver's CPU writes are write-back cached until it stores the range.
        assert_eq!(memory.read_u32(destination_physical_base).unwrap(), 0);
        assert_eq!(
            destination
                .store_process_data_cache(0x1400_1000, 8)
                .unwrap(),
            1
        );
        // The direct session helper enqueues a writeback; drain it before
        // asserting visibility to the independent physical-memory observer.
        destination.drain_write_buffer().unwrap();
        assert_eq!(
            memory.read_u32(destination_physical_base).unwrap(),
            physical_base + 0x100
        );
        assert_eq!(
            memory.read_u32(destination_physical_base + 4).unwrap(),
            0x40
        );
    }

    #[test]
    fn ipc_type_three_requires_readable_physical_source_and_page_aligned_static_table() {
        let mut source = test_session();
        let mut destination = test_session();
        let mut memory = GpuPhysicalMemory::new();
        let physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        let destination_physical_base = memory.allocate_linear_fcram(0x1000).unwrap();
        assert_eq!(destination_physical_base, physical_base + 0x1000);
        let dma = GpuDmaAddressSpace::new();
        source.attach_gpu_services(memory.clone(), dma.clone());
        destination.attach_gpu_services(memory.clone(), dma);
        source
            .address_space
            .map_linear_memory_region(0x1400_0000, physical_base, 0x1000)
            .unwrap();
        destination
            .address_space
            .map_linear_memory_region(0x1400_1000, destination_physical_base, 0x1000)
            .unwrap();

        let tls = destination
            .scheduler
            .core(MAIN_THREAD_ID)
            .unwrap()
            .cp15()
            .tpidruro();
        destination
            .write_memory_u32(tls + 0x180, 0x1400_1001)
            .unwrap();
        destination.write_memory_u32(tls + 0x184, 0x1000).unwrap();

        let descriptor = (0x20u32 << 8) | 0x06;
        let words = [0x1234_0002, descriptor, 0x1400_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();
        assert_eq!(
            destination
                .translate_physical_buffer_tables_from_ipc(&source, MAIN_THREAD_ID, message,),
            Err(
                IpcPhysicalBufferTranslationError::InvalidDestinationBuffer {
                    address: 0x1400_1001,
                    capacity: 0x1000,
                }
            )
        );
    }

    #[test]
    fn ipc_type_five_maps_read_only_input_and_does_not_copy_back() {
        let mut source = test_session();
        source
            .write_memory_bytes(0x0020_0000, &[1, 2, 3, 4])
            .unwrap();
        let mut destination = test_session();
        let words = [0x1234_0002, 0x0000_004A, 0x0020_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        let mappings = destination
            .map_temporary_buffer_translations_from_ipc(&mut source, MAIN_THREAD_ID, message)
            .unwrap();
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].mapped_address, 0x0400_1000);
        let mut input = [0; 4];
        destination
            .read_memory_bytes(mappings[0].mapped_address, &mut input)
            .unwrap();
        assert_eq!(input, [1, 2, 3, 4]);
        destination
            .write_memory_bytes(mappings[0].mapped_address, &[9, 9, 9, 9])
            .unwrap();
        destination
            .complete_temporary_buffer_translations_from_ipc(&mut source, &mappings)
            .unwrap();

        let mut unchanged = [0; 4];
        source
            .read_memory_bytes(0x0020_0000, &mut unchanged)
            .unwrap();
        assert_eq!(unchanged, [1, 2, 3, 4]);
        assert!(!destination.address_space.is_writable_range(0x0400_1000, 4));
    }

    #[test]
    fn ipc_type_six_maps_zeroed_output_and_copies_back_on_reply() {
        let mut source = test_session();
        source
            .write_memory_bytes(0x0020_0000, &[1, 2, 3, 4])
            .unwrap();
        let mut destination = test_session();
        let words = [0x1234_0002, 0x0000_004C, 0x0020_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        let mappings = destination
            .map_temporary_buffer_translations_from_ipc(&mut source, MAIN_THREAD_ID, message)
            .unwrap();
        let mut output = [0xFF; 4];
        destination
            .read_memory_bytes(mappings[0].mapped_address, &mut output)
            .unwrap();
        assert_eq!(output, [0; 4]);
        destination
            .write_memory_bytes(mappings[0].mapped_address, &[5, 6, 7, 8])
            .unwrap();
        destination
            .complete_temporary_buffer_translations_from_ipc(&mut source, &mappings)
            .unwrap();

        let mut copied_back = [0; 4];
        source
            .read_memory_bytes(0x0020_0000, &mut copied_back)
            .unwrap();
        assert_eq!(copied_back, [5, 6, 7, 8]);
    }

    #[test]
    fn ipc_type_seven_stages_input_and_copies_output_back() {
        let mut source = test_session();
        source
            .write_memory_bytes(0x0020_0000, &[1, 2, 3, 4])
            .unwrap();
        let mut destination = test_session();
        let words = [0x1234_0002, 0x0000_004E, 0x0020_0000];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        let mappings = destination
            .map_temporary_buffer_translations_from_ipc(&mut source, MAIN_THREAD_ID, message)
            .unwrap();
        let mut input = [0; 4];
        destination
            .read_memory_bytes(mappings[0].mapped_address, &mut input)
            .unwrap();
        assert_eq!(input, [1, 2, 3, 4]);
        destination
            .write_memory_bytes(mappings[0].mapped_address, &[8, 7, 6, 5])
            .unwrap();
        destination
            .complete_temporary_buffer_translations_from_ipc(&mut source, &mappings)
            .unwrap();

        let mut copied_back = [0; 4];
        source
            .read_memory_bytes(0x0020_0000, &mut copied_back)
            .unwrap();
        assert_eq!(copied_back, [8, 7, 6, 5]);
    }

    #[test]
    fn ipc_memory_block_adapter_rejects_extra_translation_parameter_groups() {
        let mut source = test_session();
        let handle = source.create_memory_block(0x1000, 3, 1).unwrap();
        let mut destination = test_session();
        let words = [
            0x1234_0004, // One handle descriptor plus one static-buffer descriptor.
            0,
            handle,
            0x0000_4002,
            0x0040_0000,
        ];
        let message = ipc::IpcMessage::decode(&words).unwrap();

        assert_eq!(
            destination.translate_memory_block_translation_section_from_ipc(&mut source, message,),
            Err(kernel::IpcHandleTranslationError::InvalidDescriptor)
        );
        assert_eq!(destination.kernel_objects.handle_count(), 0);
        assert!(source.kernel_objects.memory_block_backing(handle).is_ok());
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
        let baseline_cache_lines = session.data_cache_line_count();
        session
            .write_memory_u32(virtual_base + 0x100, 0xCAFE_BABE)
            .unwrap();
        // CPU stores remain dirty until a cache-maintenance SVC/operation.
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0);
        session
            .store_process_data_cache(virtual_base + 0x100, 4)
            .unwrap();
        // The helper enqueues the writeback; drain it before the device reads.
        session.drain_write_buffer().unwrap();
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0xCAFE_BABE);

        // An external DMA/GPU write does not snoop the resident CPU line.
        memory
            .write_u32(physical_base + 0x104, 0x1234_5678)
            .unwrap();
        assert_eq!(session.read_memory_u32(virtual_base + 0x104).unwrap(), 0);
        session
            .invalidate_process_data_cache(virtual_base + 0x104, 4)
            .unwrap();
        assert_eq!(
            session.read_memory_u32(virtual_base + 0x104).unwrap(),
            0x1234_5678
        );

        // A dirty line must not survive the release/reuse of its physical pages.
        session
            .write_memory_u32(virtual_base + 0x108, 0xDEAD_BEEF)
            .unwrap();
        assert!(session.data_cache_line_count() > 0);

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
        assert_eq!(session.data_cache_line_count(), baseline_cache_lines);
        assert_eq!(memory.read_u32(physical_base + 0x100).unwrap(), 0);
        assert_eq!(memory.read_u32(physical_base + 0x108).unwrap(), 0);
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
    fn wait_synchronization1_waits_for_thread_exit_and_thread_object_stays_signaled() {
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEF00_0024), // SVC WaitSynchronization1 (main)
            (0x04, 0xE120_0070), // BKPT #0 (main resumes)
            (0x20, 0xEF00_0009), // SVC ExitThread (target)
        ]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x3F, 0)
            .unwrap();
        let target_handle = session.kernel_objects.insert_thread(target).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = target_handle;
            main.regs[2] = u32::MAX;
            main.regs[3] = u32::MAX;
        }

        let blocked = session.run_instructions(1).unwrap();
        assert_eq!(
            blocked.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(
            session.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Waiting
        );
        assert_eq!(session.current_thread_id(), Some(target));

        let exited = session.run_instructions(1).unwrap();
        assert_eq!(
            exited.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(
            session.thread_info(target).unwrap().state,
            ThreadState::Exited
        );
        assert_eq!(
            session.kernel_objects.thread_is_terminated(target_handle),
            Some(true)
        );
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(session.core().regs[0], 0);

        // Thread objects are manual-reset: every later wait succeeds too.
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = target_handle;
            main.regs[2] = 0;
            main.regs[3] = 0;
        }
        assert!(matches!(
            session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1),
            SvcRunDisposition::Resume
        ));
        assert_eq!(session.core().regs[0], 0);
    }

    #[test]
    fn wait_synchronization_n_wait_any_reports_exited_thread_index() {
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEF00_0025), // SVC WaitSynchronizationN (main)
            (0x04, 0xE120_0070), // BKPT #0 (main resumes)
            (0x20, 0xEF00_0009), // SVC ExitThread (target)
        ]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x3F, 0)
            .unwrap();
        let target_handle = session.kernel_objects.insert_thread(target).unwrap();
        session
            .address_space
            .write32(0x0020_0100, target_handle)
            .unwrap();

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = u32::MAX; // timeout low
            main.regs[1] = 0x0020_0100; // handle array
            main.regs[2] = 1;
            main.regs[3] = 0; // wait-any
            main.regs[4] = u32::MAX; // timeout high (-1)
        }

        let blocked = session.run_instructions(1).unwrap();
        assert_eq!(
            blocked.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.current_thread_id(), Some(target));

        let exited = session.run_instructions(1).unwrap();
        assert_eq!(
            exited.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!((session.core().regs[0], session.core().regs[1]), (0, 0));
        assert_eq!(
            session.kernel_objects.thread_is_terminated(target_handle),
            Some(true)
        );
    }

    #[test]
    fn wait_synchronization_n_wait_all_combines_thread_exit_with_event() {
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEF00_0025), // SVC WaitSynchronizationN (main)
            (0x04, 0xE120_0070), // BKPT #0 (main resumes)
            (0x20, 0xEF00_0009), // SVC ExitThread (target)
        ]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x3F, 0)
            .unwrap();
        let target_handle = session.kernel_objects.insert_thread(target).unwrap();
        let event_handle = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        session
            .address_space
            .write32(0x0020_0100, event_handle)
            .unwrap();
        session
            .address_space
            .write32(0x0020_0104, target_handle)
            .unwrap();

        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = u32::MAX;
            main.regs[1] = 0x0020_0100;
            main.regs[2] = 2;
            main.regs[3] = 1; // wait-all
            main.regs[4] = u32::MAX;
        }

        let blocked = session.run_instructions(1).unwrap();
        assert_eq!(
            blocked.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(session.current_thread_id(), Some(target));

        let target_exit = session.run_instructions(1).unwrap();
        assert_eq!(
            target_exit.stop_reason,
            Arm11RunStopReason::NoRunnableThread
        );
        assert_eq!(session.current_thread_id(), None);
        assert_eq!(
            session.kernel_objects.thread_is_terminated(target_handle),
            Some(true)
        );
        assert_eq!(session.event_handle_is_signaled(event_handle), Some(false));

        // Thread exit alone does not partially complete the wait-all set.
        session.signal_event_handle(event_handle).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(session.event_handle_is_signaled(event_handle), Some(false));
        assert_eq!((session.core().regs[0], session.core().regs[1]), (0, 0));

        let resumed = session.run_instructions(1).unwrap();
        assert_eq!(resumed.stop_reason, Arm11RunStopReason::Breakpoint(0));
    }

    #[test]
    fn exit_thread_releases_every_owned_mutex_to_waiting_threads() {
        let mut session = test_session_with_program(&[0xEF00_0009]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x30, 0)
            .unwrap();
        let target_handle = session.kernel_objects.insert_thread(target).unwrap();
        let mutex = session.kernel_objects.insert_mutex(true, target).unwrap();
        session
            .kernel_objects
            .enqueue_waiter(mutex, MAIN_THREAD_ID)
            .unwrap();

        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        session.scheduler.wait_current().unwrap();
        assert_eq!(session.current_thread_id(), Some(target));

        assert!(matches!(
            session.dispatch_svc(SVC_EXIT_THREAD),
            SvcRunDisposition::Resume
        ));
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.kernel_objects.mutex_owner(mutex),
            Some(MAIN_THREAD_ID)
        );
        assert_eq!(
            session.kernel_objects.thread_is_terminated(target_handle),
            Some(true)
        );
        assert_eq!(
            session.thread_info(target).unwrap().state,
            ThreadState::Exited
        );
    }

    #[test]
    fn close_handle_keeps_wait_one_object_alive_until_waiter_wakes() {
        let mut session = test_session_with_program_and_data(&[
            (0x00, 0xEF00_0024), // SVC WaitSynchronization1 (main)
            (0x04, 0xE120_0070), // BKPT #0
            (0x20, 0xE120_0070), // BKPT #0 (other thread)
        ]);
        let other = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x3F, 0)
            .unwrap();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[0] = event;
            main.regs[2] = u32::MAX;
            main.regs[3] = u32::MAX;
        }

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1);
        assert_eq!(session.current_thread_id(), Some(other));
        assert_eq!(
            session.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Waiting
        );

        session.close_handle(event).unwrap();
        assert!(!session.kernel_objects.is_open_handle(event));
        assert_eq!(session.kernel_objects.retained_object_count(), 1);

        // An HLE producer can still signal the retired object held by the wait.
        session.signal_event_handle(event).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.kernel_objects.retained_object_count(), 0);

        // Guest code cannot use the closed handle for a new SignalEvent call.
        session.scheduler.current_core_mut().unwrap().regs[0] = event;
        assert!(matches!(
            session.dispatch_svc(SVC_SIGNAL_EVENT),
            SvcRunDisposition::Resume
        ));
        assert_eq!(session.core().regs[0], RESULT_INVALID_HANDLE);
    }

    #[test]
    fn closing_wait_one_handle_then_timing_out_releases_retired_object() {
        let mut session = test_session_with_program(&[0xEF00_0024]);
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = event;
            main.regs[2] = 10;
            main.regs[3] = 0;
        }

        assert_eq!(
            session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION1),
            SvcRunDisposition::NoRunnableThread
        );
        session.close_handle(event).unwrap();
        assert_eq!(session.kernel_objects.retained_object_count(), 1);

        assert_eq!(session.advance_time(10), Some(MAIN_THREAD_ID));
        assert_eq!(session.core().regs[0], RESULT_TIMEOUT);
        assert_eq!(session.kernel_objects.retained_object_count(), 0);
    }

    #[test]
    fn closing_last_thread_handle_keeps_wait_all_reference_until_other_objects_signal() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x30, 0)
            .unwrap();
        let target_handle = session.kernel_objects.insert_thread(target).unwrap();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        session.address_space.write32(0x0020_0100, event).unwrap();
        session
            .address_space
            .write32(0x0020_0104, target_handle)
            .unwrap();
        {
            let main = session.scheduler.current_core_mut().unwrap();
            main.regs[0] = u32::MAX;
            main.regs[1] = 0x0020_0100;
            main.regs[2] = 2;
            main.regs[3] = 1;
            main.regs[4] = u32::MAX;
        }

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(target));
        assert_eq!(
            session.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Waiting
        );

        session.close_handle(target_handle).unwrap();

        assert_eq!(session.current_thread_id(), None);
        assert_eq!(session.thread_info(target), None);
        assert_eq!(session.scheduler.thread_count(), 1);
        assert!(session.wait_all_waits.contains_key(&MAIN_THREAD_ID));
        assert_eq!(session.event_handle_is_signaled(event), Some(false));
        assert_eq!(session.kernel_objects.retained_object_count(), 2);

        session.signal_event_handle(event).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(session.core().regs[0], 0);
        assert!(!session.wait_all_waits.contains_key(&MAIN_THREAD_ID));
        assert_eq!(session.kernel_objects.retained_object_count(), 1);
    }

    #[test]
    fn closing_final_handle_after_exit_reclaims_saved_thread_context() {
        let mut session = test_session_with_program(&[0xEF00_0009]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x30, 0)
            .unwrap();
        let target_handle = session.kernel_objects.insert_thread(target).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        session.scheduler.yield_current().unwrap();
        assert_eq!(session.current_thread_id(), Some(target));
        session.dispatch_svc(SVC_EXIT_THREAD);

        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(target).unwrap().state,
            ThreadState::Exited
        );
        assert_eq!(session.scheduler.thread_count(), 2);

        session.close_handle(target_handle).unwrap();
        assert_eq!(session.thread_info(target), None);
        assert_eq!(session.scheduler.thread_count(), 1);
    }

    #[test]
    fn close_handle_from_the_target_thread_terminates_without_returning_to_it() {
        let mut session = test_session_with_program(&[0xEF00_0023]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x30, 0)
            .unwrap();
        let handle = session.kernel_objects.insert_thread(target).unwrap();

        session.scheduler.yield_current().unwrap();
        assert_eq!(session.current_thread_id(), Some(target));
        session.scheduler.core_mut(target).unwrap().regs[0] = handle;

        assert!(matches!(
            session.dispatch_svc(SVC_CLOSE_HANDLE),
            SvcRunDisposition::Resume
        ));
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(session.thread_info(target), None);
        assert_eq!(session.scheduler.thread_count(), 1);
    }

    #[test]
    fn get_handle_info_svc_returns_64_bit_reference_count_in_r1_r2() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let duplicate = session.kernel_objects.duplicate_handle(event).unwrap();

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            // libctru preserves the out pointer outside the SVC boundary;
            // r1 and r2 are the actual handle/type inputs at the kernel.
            main.regs[0] = 0xDEAD_BEEF;
            main.regs[1] = duplicate;
            main.regs[2] = 1;
        }
        assert!(matches!(
            session.dispatch_svc(SVC_GET_HANDLE_INFO),
            SvcRunDisposition::Resume
        ));
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(u64::from(registers[1]) | (u64::from(registers[2]) << 32), 2);

        // The current process was created at virtual tick zero.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 0;
        }
        session.dispatch_svc(SVC_GET_HANDLE_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0);
        assert_eq!(registers[2], 0);

        // A non-process object leaves the r5-r6 output seed unchanged for type 0.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = event;
            main.regs[2] = 0;
            main.regs[5] = 0x1122_3344;
            main.regs[6] = 0x5566_7788;
        }
        session.dispatch_svc(SVC_GET_HANDLE_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(
            u64::from(registers[1]) | (u64::from(registers[2]) << 32),
            0x5566_7788_1122_3344
        );

        // Type 2 is an unimplemented kernel query and preserves r5-r6 too.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = duplicate;
            main.regs[2] = 2;
            main.regs[5] = 0x89AB_CDEF;
            main.regs[6] = 0x0123_4567;
        }
        session.dispatch_svc(SVC_GET_HANDLE_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(
            u64::from(registers[1]) | (u64::from(registers[2]) << 32),
            0x0123_4567_89AB_CDEF
        );

        // The documented special type always returns zero.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = event;
            main.regs[2] = 0x0003_2107;
        }
        session.dispatch_svc(SVC_GET_HANDLE_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0);
        assert_eq!(registers[2], 0);

        // Invalid handles and undefined information types retain distinct errors.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = 0xDEAD_BEEF;
            main.regs[2] = 2;
        }
        session.dispatch_svc(SVC_GET_HANDLE_INFO);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_INVALID_HANDLE
        );

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = duplicate;
            main.regs[2] = 0xFFFF;
        }
        session.dispatch_svc(SVC_GET_HANDLE_INFO);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_INVALID_ENUM
        );
    }

    #[test]
    fn get_system_tick_svc_returns_virtual_64_bit_arm11_ticks() {
        let mut session = test_session();

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[0] = 0xAAAA_AAAA;
            main.regs[1] = 0xBBBB_BBBB;
        }
        assert!(matches!(
            session.dispatch_svc(SVC_GET_SYSTEM_TICK),
            SvcRunDisposition::Resume
        ));
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(registers[1], 0);

        // 17 virtual seconds exceed 32 bits at the base ARM11 tick rate.
        session.advance_time(17_000_000_000);
        session.dispatch_svc(SVC_GET_SYSTEM_TICK);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        let actual = u64::from(registers[0]) | (u64::from(registers[1]) << 32);
        let expected = (17_000_000_000u128 * 268_111_856u128 / 1_000_000_000) as u64;
        assert_eq!(actual, expected);
        assert_ne!(registers[1], 0);
    }

    #[test]
    fn get_thread_info_svc_matches_kernel_handle_type_checks() {
        let mut session = test_session();
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x30, 0)
            .unwrap();
        let thread_handle = session.kernel_objects.insert_thread(target).unwrap();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();

        // A real thread handle always yields InvalidEnum, regardless of type.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[0] = 0xDEAD_BEEF;
            main.regs[1] = thread_handle;
            main.regs[2] = 0xFFFF;
        }
        assert!(matches!(
            session.dispatch_svc(SVC_GET_THREAD_INFO),
            SvcRunDisposition::Resume
        ));
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_INVALID_ENUM
        );

        // The current-thread pseudo-handle is also a thread handle.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_THREAD_HANDLE;
            main.regs[2] = 0;
        }
        session.dispatch_svc(SVC_GET_THREAD_INFO);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_INVALID_ENUM
        );

        // Non-thread objects, the process pseudo-handle and invalid handles
        // all take the kernel's InvalidHandle path.
        for handle in [event, CURRENT_PROCESS_HANDLE, 0xDEAD_BEEF] {
            session.scheduler.core_mut(MAIN_THREAD_ID).unwrap().regs[1] = handle;
            session.dispatch_svc(SVC_GET_THREAD_INFO);
            assert_eq!(
                session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
                RESULT_INVALID_HANDLE
            );
        }
    }

    #[test]
    fn get_process_info_svc_reports_current_and_peak_handle_counts() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        session.kernel_objects.duplicate_handle(event).unwrap();

        // Type 4 returns the live count; type 5 returns the historical peak.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[0] = 0xDEAD_BEEF; // unused at the raw SVC boundary
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 4;
        }
        assert!(matches!(
            session.dispatch_svc(SVC_GET_PROCESS_INFO),
            SvcRunDisposition::Resume
        ));
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(u64::from(registers[1]) | (u64::from(registers[2]) << 32), 2);

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 5;
        }
        session.dispatch_svc(SVC_GET_PROCESS_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        assert_eq!(u64::from(registers[1]) | (u64::from(registers[2]) << 32), 2);

        // Closing one alias decreases the current count but never the peak.
        session.close_handle(event).unwrap();
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 4;
        }
        session.dispatch_svc(SVC_GET_PROCESS_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(u64::from(registers[1]) | (u64::from(registers[2]) << 32), 1);

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 5;
        }
        session.dispatch_svc(SVC_GET_PROCESS_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(u64::from(registers[1]) | (u64::from(registers[2]) << 32), 2);

        // Further allocations can establish a new high-water mark.
        session.kernel_objects.insert_semaphore(0, 2).unwrap();
        session
            .kernel_objects
            .insert_timer(EventResetType::OneShot)
            .unwrap();
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 5;
        }
        session.dispatch_svc(SVC_GET_PROCESS_INFO);
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(u64::from(registers[1]) | (u64::from(registers[2]) << 32), 3);

        // The current-process pseudo-handle is the only process handle modeled.
        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = 0xDEAD_BEEF;
            main.regs[2] = 4;
        }
        session.dispatch_svc(SVC_GET_PROCESS_INFO);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_INVALID_HANDLE
        );

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            main.regs[1] = CURRENT_PROCESS_HANDLE;
            main.regs[2] = 0; // Process memory usage is not modelled.
        }
        session.dispatch_svc(SVC_GET_PROCESS_INFO);
        assert_eq!(
            session.scheduler.core(MAIN_THREAD_ID).unwrap().regs[0],
            RESULT_INVALID_ENUM
        );
    }

    #[test]
    fn duplicate_handle_svc_uses_raw_register_abi_and_shares_object() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();

        {
            let main = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            // r0 is not an output pointer at the raw SVC boundary.
            main.regs[0] = 0xDEAD_BEEF;
            main.regs[1] = event;
        }

        assert!(matches!(
            session.dispatch_svc(SVC_DUPLICATE_HANDLE),
            SvcRunDisposition::Resume
        ));
        let registers = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!(registers[0], 0);
        let duplicate = registers[1];
        assert_ne!(duplicate, event);
        assert_eq!(session.kernel_objects.handle_count(), 2);
        assert_eq!(session.kernel_objects.retained_object_count(), 1);

        assert_eq!(
            session.kernel_objects.signal_event(duplicate),
            Ok(Vec::new())
        );
        assert_eq!(session.kernel_objects.event_is_signaled(event), Some(true));

        session.close_handle(event).unwrap();
        assert!(!session.kernel_objects.is_open_handle(event));
        assert!(session.kernel_objects.is_open_handle(duplicate));
        assert_eq!(
            session.kernel_objects.event_is_signaled(duplicate),
            Some(true)
        );

        session.close_handle(duplicate).unwrap();
        assert_eq!(session.kernel_objects.handle_count(), 0);
        assert_eq!(session.kernel_objects.retained_object_count(), 0);
    }

    #[test]
    fn closing_one_of_multiple_thread_handles_does_not_terminate_target() {
        let mut session = test_session_with_program(&[0xEF00_0009]);
        let target = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x30, 0)
            .unwrap();
        let first_handle = session.kernel_objects.insert_thread(target).unwrap();
        let second_handle = session.kernel_objects.insert_thread(target).unwrap();

        assert_eq!(session.kernel_objects.thread_handle_count(target), 2);
        session.close_handle(first_handle).unwrap();
        assert_eq!(session.kernel_objects.thread_handle_count(target), 1);
        assert_eq!(
            session.thread_info(target).unwrap().state,
            ThreadState::Ready
        );
        assert_eq!(session.scheduler.thread_count(), 2);

        session.close_handle(second_handle).unwrap();
        assert_eq!(session.thread_info(target), None);
        assert_eq!(session.scheduler.thread_count(), 1);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
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

    fn wait_n_setup(
        session: &mut Arm11ProcessSession,
        thread: ThreadId,
        handles: &[u32],
        timeout: i64,
    ) {
        for (index, handle) in handles.iter().enumerate() {
            session
                .address_space
                .write32(0x0020_0100 + index as u32 * 4, *handle)
                .unwrap();
        }
        let core = session.scheduler.core_mut(thread).unwrap();
        core.regs[0] = timeout as u32;
        core.regs[1] = 0x0020_0100;
        core.regs[2] = handles.len() as u32;
        core.regs[3] = 0;
        core.regs[4] = (timeout >> 32) as u32;
    }

    fn two_events(session: &mut Arm11ProcessSession) -> (u32, u32) {
        (
            session
                .kernel_objects
                .insert_event(EventResetType::OneShot)
                .unwrap(),
            session
                .kernel_objects
                .insert_event(EventResetType::OneShot)
                .unwrap(),
        )
    }

    /// Guest-side reproduction of libctru's public C ABI veneer:
    /// out, handles, count and wait_all arrive in r0-r3; the i64 timeout is
    /// passed on the guest stack; the veneer moves it into SVC r0/r4, invokes
    /// SVC 0x25, then writes the returned SVC r1 through the original out ptr.
    fn libctru_wait_n_wrapper_session() -> Arm11ProcessSession {
        test_session_with_program(&[
            0xE52D_5004, // STR r5, [sp, #-4]!
            0xE52D_4004, // STR r4, [sp, #-4]!
            0xE1A0_5000, // MOV r5, r0 (preserve the public out pointer)
            0xE59D_0008, // LDR r0, [sp, #8] (timeout low)
            0xE59D_400C, // LDR r4, [sp, #12] (timeout high)
            0xEF00_0025, // SVC 0x25
            0xE585_1000, // STR r1, [r5] (publish selected index to *out)
            0xE49D_4004, // LDR r4, [sp], #4
            0xE49D_5004, // LDR r5, [sp], #4
            0xE12F_FF1E, // BX lr
            0xE120_0070, // BKPT #0 (return sentinel)
        ])
    }

    #[test]
    fn wait_synchronization_n_runs_the_guest_abi_veneer_and_writes_out_index() {
        let mut session = libctru_wait_n_wrapper_session();
        let (first, second) = two_events(&mut session);
        let out_address = 0x0020_0180;
        let handles_address = 0x0020_0100;
        let stack_pointer = 0x0020_1F00;
        let timeout = (1_i64 << 32) + 100;

        session
            .address_space
            .write32(out_address, 0xCAFE_BABE)
            .unwrap();
        session
            .address_space
            .write32(handles_address, first)
            .unwrap();
        session
            .address_space
            .write32(handles_address + 4, second)
            .unwrap();
        session
            .address_space
            .write32(stack_pointer, timeout as u32)
            .unwrap();
        session
            .address_space
            .write32(stack_pointer + 4, (timeout >> 32) as u32)
            .unwrap();

        {
            let core = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            // Public libctru C signature at function entry.
            core.regs[0] = out_address;
            core.regs[1] = handles_address;
            core.regs[2] = 2;
            core.regs[3] = 0;
            core.regs[4] = 0x4444_4444;
            core.regs[5] = 0x5555_5555;
            core.regs[13] = stack_pointer;
            core.regs[14] = 0x0010_0028;
        }

        // The guest executes the prologue and SVC. Both handles are initially
        // unsignaled, so the ARM11 thread blocks with the full 64-bit deadline.
        let report = session.run_instructions(6).unwrap();
        assert_eq!(report.stop_reason, Arm11RunStopReason::NoRunnableThread);
        assert_eq!(
            session.wait_deadlines.get(&MAIN_THREAD_ID),
            Some(&(first, timeout as u64))
        );
        assert_eq!(
            session.address_space.read32(out_address).unwrap(),
            0xCAFE_BABE
        );

        // Signalling handle 1 resumes the same guest wrapper. It observes the
        // selected index in r1 and stores it through the original C out pointer.
        session.signal_event_handle(second).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!((session.core().regs[0], session.core().regs[1]), (0, 1));

        let epilogue = session.run_instructions(5).unwrap();
        assert_eq!(epilogue.stop_reason, Arm11RunStopReason::Breakpoint(0));
        assert_eq!(session.read_memory_u32(out_address).unwrap(), 1);
        assert_eq!(session.core().regs[4], 0x4444_4444);
        assert_eq!(session.core().regs[5], 0x5555_5555);
        assert_eq!(session.core().regs[13], stack_pointer);
    }

    #[test]
    fn wait_synchronization_n_guest_abi_uses_both_timeout_words_and_zeroes_out_on_error() {
        let mut session = libctru_wait_n_wrapper_session();
        let (event, _) = two_events(&mut session);
        let out_address = 0x0020_0180;
        let handles_address = 0x0020_0100;
        let stack_pointer = 0x0020_1F00;
        let timeout = -2_i64;

        session
            .address_space
            .write32(out_address, 0xCAFE_BABE)
            .unwrap();
        session
            .address_space
            .write32(handles_address, event)
            .unwrap();
        session
            .address_space
            .write32(stack_pointer, timeout as u32)
            .unwrap();
        session
            .address_space
            .write32(stack_pointer + 4, (timeout >> 32) as u32)
            .unwrap();

        {
            let core = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            core.regs[0] = out_address;
            core.regs[1] = handles_address;
            core.regs[2] = 1;
            core.regs[3] = 0;
            core.regs[4] = 0x4444_4444;
            core.regs[5] = 0x5555_5555;
            core.regs[13] = stack_pointer;
            core.regs[14] = 0x0010_0028;
        }

        // If the high timeout word were ignored, 0xFFFF_FFFE could be
        // misinterpreted as a large positive timeout. The actual signed 64-bit
        // value is -2, which the 3DS kernel rejects with InvalidTimeout.
        let report = session.run_instructions(11).unwrap();
        assert_eq!(report.stop_reason, Arm11RunStopReason::Breakpoint(0));
        assert_eq!(session.core().regs[0], RESULT_INVALID_TIMEOUT);
        // The wrapper always stores r1, so errors must return index zero.
        assert_eq!(session.read_memory_u32(out_address).unwrap(), 0);
        assert_eq!(session.core().regs[4], 0x4444_4444);
        assert_eq!(session.core().regs[5], 0x5555_5555);
        assert_eq!(session.core().regs[13], stack_pointer);
    }

    #[test]
    fn wait_synchronization_n_returns_the_first_signaled_handle_index_immediately() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (first, second) = two_events(&mut session);
        session.signal_event_handle(second).unwrap();
        wait_n_setup(&mut session, MAIN_THREAD_ID, &[first, second], -1);

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);

        assert_eq!(session.core().regs[0], 0);
        assert_eq!(session.core().regs[1], 1);
        // The one-shot signal was consumed by the wait.
        assert_eq!(
            session
                .kernel_objects
                .try_wait_synchronization(second, MAIN_THREAD_ID),
            Ok(false)
        );
    }

    #[test]
    fn wait_synchronization_n_zero_timeout_and_argument_errors() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (first, second) = two_events(&mut session);
        wait_n_setup(&mut session, MAIN_THREAD_ID, &[first, second], 0);
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.core().regs[0], RESULT_TIMEOUT);

        wait_n_setup(&mut session, MAIN_THREAD_ID, &[first, 0xDEAD], -1);
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(
            session.core().regs[0],
            kernel_handle_result_code(KernelHandleError::InvalidHandle)
        );

        wait_n_setup(&mut session, MAIN_THREAD_ID, &[first], -1);
        session.scheduler.current_core_mut().unwrap().regs[2] = 0x41;
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.core().regs[0], RESULT_OUT_OF_RANGE);

        wait_n_setup(&mut session, MAIN_THREAD_ID, &[first], -2);
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.core().regs[0], RESULT_INVALID_TIMEOUT);
        // The libctru wrapper stores r1 into *out even on failure.
        assert_eq!(session.core().regs[1], 0);

        wait_n_setup(&mut session, MAIN_THREAD_ID, &[first], -1);
        session.scheduler.current_core_mut().unwrap().regs[1] = 0;
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.core().regs[0], RESULT_INVALID_POINTER);
        assert_eq!(session.core().regs[1], 0);
    }

    #[test]
    fn wait_synchronization_n_blocks_and_reports_the_releasing_handle() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (first, second) = two_events(&mut session);
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        wait_n_setup(&mut session, child, &[first, second], -1);

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        session.signal_event_handle(second).unwrap();
        assert_eq!(session.current_thread_id(), Some(child));
        let regs = session.scheduler.core(child).unwrap().regs;
        assert_eq!((regs[0], regs[1]), (0, 1));

        // The thread left the first event's queue: signalling it only latches.
        session.signal_event_handle(first).unwrap();
        assert_eq!(
            session
                .kernel_objects
                .try_wait_synchronization(first, MAIN_THREAD_ID),
            Ok(true)
        );
    }

    #[test]
    fn wait_synchronization_n_finite_timeout_expires_and_cleans_every_queue() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (first, second) = two_events(&mut session);
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        wait_n_setup(&mut session, child, &[first, second], 100);
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        session.advance_time(100);
        assert_eq!(
            session.scheduler.core(child).unwrap().regs[0],
            RESULT_TIMEOUT
        );

        // Neither event may wake the thread afterwards.
        session.signal_event_handle(first).unwrap();
        session.signal_event_handle(second).unwrap();
        assert_eq!(
            session.scheduler.core(child).unwrap().regs[0],
            RESULT_TIMEOUT
        );
    }

    #[test]
    fn wait_synchronization_n_reports_timer_expiry_index() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        let timer = session
            .kernel_objects
            .insert_timer(EventResetType::OneShot)
            .unwrap();
        session
            .kernel_objects
            .set_timer(timer, session.scheduler.virtual_time_ns(), 50, 0)
            .unwrap();
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        wait_n_setup(&mut session, child, &[event, timer], -1);
        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        session.advance_time(50);

        let regs = session.scheduler.core(child).unwrap().regs;
        assert_eq!((regs[0], regs[1]), (0, 1));
    }

    #[test]
    fn wait_synchronization_n_wait_all_does_not_consume_a_partial_signal_set() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (first, second) = two_events(&mut session);
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        wait_n_setup(&mut session, child, &[first, second], -1);
        session.scheduler.core_mut(child).unwrap().regs[3] = 1;

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );

        session.signal_event_handle(first).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );
        assert_eq!(session.event_handle_is_signaled(first), Some(true));
        assert_eq!(session.event_handle_is_signaled(second), Some(false));

        session.signal_event_handle(second).unwrap();
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Running
        );
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], 0);
        assert_eq!(session.scheduler.core(child).unwrap().regs[1], 0);
        assert_eq!(session.event_handle_is_signaled(first), Some(false));
        assert_eq!(session.event_handle_is_signaled(second), Some(false));
    }

    #[test]
    fn wait_synchronization_n_wait_all_timeout_preserves_unacquired_signals() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (first, second) = two_events(&mut session);
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        wait_n_setup(&mut session, child, &[first, second], 100);
        session.scheduler.core_mut(child).unwrap().regs[3] = 1;

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        session.signal_event_handle(first).unwrap();
        assert_eq!(session.event_handle_is_signaled(first), Some(true));
        session.advance_time(99);
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );

        session.advance_time(1);
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(
            session.scheduler.core(child).unwrap().regs[0],
            RESULT_TIMEOUT
        );
        assert_eq!(session.scheduler.core(child).unwrap().regs[1], 0);
        assert_eq!(session.event_handle_is_signaled(first), Some(true));
        assert_eq!(session.event_handle_is_signaled(second), Some(false));
        assert!(!session.wait_all_waits.contains_key(&child));
    }

    #[test]
    fn wait_synchronization_n_wait_all_acquires_mixed_objects_atomically() {
        let mut session = test_session_with_program(&[0xEF00_0025]);
        let (event, _) = two_events(&mut session);
        let semaphore = session.kernel_objects.insert_semaphore(0, 1).unwrap();
        let child = session
            .scheduler
            .create_thread(0x0010_0020, 0, 0x0020_2000, 0x20, 0)
            .unwrap();
        wait_n_setup(&mut session, child, &[event, semaphore], -1);
        session.scheduler.core_mut(child).unwrap().regs[3] = 1;

        session.dispatch_svc(SVC_WAIT_SYNCHRONIZATION_N);
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        session.signal_event_handle(event).unwrap();
        assert_eq!(session.event_handle_is_signaled(event), Some(true));
        assert_eq!(
            session.kernel_objects.semaphore_counts(semaphore),
            Some((0, 1))
        );
        assert_eq!(
            session.thread_info(child).unwrap().state,
            ThreadState::Waiting
        );

        {
            let core = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            core.regs[0] = 0x0020_0000;
            core.regs[1] = semaphore;
            core.regs[2] = 1;
        }
        session.dispatch_svc(SVC_RELEASE_SEMAPHORE);
        assert_eq!(session.current_thread_id(), Some(child));
        assert_eq!(session.scheduler.core(child).unwrap().regs[0], 0);
        assert_eq!(session.scheduler.core(child).unwrap().regs[1], 0);
        assert_eq!(session.event_handle_is_signaled(event), Some(false));
        assert_eq!(
            session.kernel_objects.semaphore_counts(semaphore),
            Some((0, 1))
        );
    }

    #[test]
    fn wait_synchronization_n_wait_all_runs_through_the_guest_abi_veneer() {
        let mut session = libctru_wait_n_wrapper_session();
        let (first, second) = two_events(&mut session);
        let out_address = 0x0020_0180;
        let handles_address = 0x0020_0100;
        let stack_pointer = 0x0020_1F00;
        let timeout = -1_i64;

        session
            .address_space
            .write32(out_address, 0xCAFE_BABE)
            .unwrap();
        session
            .address_space
            .write32(handles_address, first)
            .unwrap();
        session
            .address_space
            .write32(handles_address + 4, second)
            .unwrap();
        session
            .address_space
            .write32(stack_pointer, timeout as u32)
            .unwrap();
        session
            .address_space
            .write32(stack_pointer + 4, (timeout >> 32) as u32)
            .unwrap();

        {
            let core = session.scheduler.core_mut(MAIN_THREAD_ID).unwrap();
            core.regs[0] = out_address;
            core.regs[1] = handles_address;
            core.regs[2] = 2;
            core.regs[3] = 1;
            core.regs[4] = 0x4444_4444;
            core.regs[5] = 0x5555_5555;
            core.regs[13] = stack_pointer;
            core.regs[14] = 0x0010_0028;
        }

        let report = session.run_instructions(6).unwrap();
        assert_eq!(report.stop_reason, Arm11RunStopReason::NoRunnableThread);
        assert_eq!(
            session.address_space.read32(out_address).unwrap(),
            0xCAFE_BABE
        );

        session.signal_event_handle(first).unwrap();
        assert_eq!(session.current_thread_id(), None);
        assert_eq!(
            session.address_space.read32(out_address).unwrap(),
            0xCAFE_BABE
        );
        session.signal_event_handle(second).unwrap();
        assert_eq!(session.current_thread_id(), Some(MAIN_THREAD_ID));

        let regs = session.scheduler.core(MAIN_THREAD_ID).unwrap().regs;
        assert_eq!((regs[0], regs[1]), (0, 0));

        let epilogue = session.run_instructions(5).unwrap();
        assert_eq!(epilogue.stop_reason, Arm11RunStopReason::Breakpoint(0));
        assert_eq!(session.read_memory_u32(out_address).unwrap(), 0);
        assert_eq!(session.core().regs[4], 0x4444_4444);
        assert_eq!(session.core().regs[5], 0x5555_5555);
        assert_eq!(session.core().regs[13], stack_pointer);
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

    #[test]
    fn connect_to_port_svc_creates_the_service_manager_handle() {
        let mut session = test_session_with_program(&[0xEF00_002D]);
        session.write_memory_bytes(0x0020_1000, b"srv:").unwrap();
        {
            let core = session.scheduler.current_core_mut().unwrap();
            core.regs[0] = 0x0020_0000;
            core.regs[1] = 0x0020_1000;
        }

        let report = session.run_instructions(1).unwrap();
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(report.instructions_executed, 1);
        let handle = session.read_memory_u32(0x0020_0000).unwrap();
        assert!(matches!(
            session.service_session_kind(handle),
            Some(ServiceSessionKind::ServiceManager {
                client_registered: false,
            })
        ));
    }

    #[test]
    fn send_sync_request_svc_yields_to_the_system_ipc_dispatcher() {
        let mut session = test_session_with_program(&[0xEF00_0032]);
        let service_handle = session.create_gsp_gpu_session().unwrap();
        session.scheduler.current_core_mut().unwrap().regs[0] = service_handle;

        let report = session.run_instructions(1).unwrap();
        assert_eq!(report.stop_reason, Arm11RunStopReason::IpcRequest);
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(
            session.service_session_kind(service_handle),
            Some(ServiceSessionKind::GspGpu {
                client_id: None,
                shared_memory_handle: None,
                event_handle: None,
                flags: 0,
            })
        );
    }
}
