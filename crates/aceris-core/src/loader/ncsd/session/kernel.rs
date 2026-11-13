//! Process-local ARM11 kernel handles and synchronization objects.
//!
//! The process-local table models typed handles, FIFO synchronization waiters,
//! mutex ownership, semaphore counts, events and virtual-clock timers. Object
//! reference counting and kernel handle generations remain future work.

use std::collections::{BTreeMap, VecDeque};

use super::thread::ThreadId;
use crate::loader::ncsd::process::SharedMemoryBacking;

/// Pseudo-handle that always identifies the calling thread.
pub const CURRENT_THREAD_HANDLE: u32 = 0xFFFF_8000;

const FIRST_HANDLE: u32 = 0x100;

/// Decoded type-0 IPC handle translation descriptor.
///
/// Type 0 transfers one or more kernel handles. Bit 4 selects MOVE instead
/// of COPY, bit 5 requests a process-ID pseudo-handle, and bits 26-31 encode
/// the number of handles minus one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcHandleTranslationDescriptor {
    handle_count: u8,
    move_handle: bool,
    process_id: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IpcHandleTranslationError {
    InvalidDescriptor,
    ProcessIdTranslationUnsupported,
    HandleCountMismatch { expected: u8, actual: usize },
    DuplicateMoveHandle { handle: u32 },
    Kernel(KernelHandleError),
}

impl IpcHandleTranslationDescriptor {
    /// Decodes a type-0 descriptor and rejects reserved or contradictory flags.
    pub const fn decode(descriptor: u32) -> Result<Self, IpcHandleTranslationError> {
        let translation_type = (descriptor >> 1) & 0x7;
        let reserved_bits = descriptor & 0x03FF_FFC1;
        let move_handle = descriptor & (1 << 4) != 0;
        let process_id = descriptor & (1 << 5) != 0;
        if translation_type != 0 || reserved_bits != 0 || (move_handle && process_id) {
            return Err(IpcHandleTranslationError::InvalidDescriptor);
        }

        Ok(Self {
            handle_count: (((descriptor >> 26) & 0x3F) as u8) + 1,
            move_handle,
            process_id,
        })
    }

    pub const fn handle_count(self) -> u8 {
        self.handle_count
    }

    pub const fn moves_handle(self) -> bool {
        self.move_handle
    }

    pub const fn requests_process_id(self) -> bool {
        self.process_id
    }
}

/// Reset behavior supported by ARM11 event objects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventResetType {
    /// A successful wait consumes the signal and resets the event.
    OneShot,
    /// The event remains signaled until explicitly cleared.
    Sticky,
}

impl EventResetType {
    /// Converts the guest ResetType value into a supported event mode.
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::OneShot),
            1 => Some(Self::Sticky),
            _ => None,
        }
    }
}

/// Failures from process-local kernel handle operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelHandleError {
    /// The process-local handle space is exhausted.
    Exhausted,
    /// No object exists for the supplied handle.
    InvalidHandle,
    /// The handle exists, but refers to a different object type.
    WrongType,
    /// A mutex can only be released by its owning thread.
    NotOwner,
    /// A semaphore count is negative, zero when invalid, or exceeds its limit.
    InvalidCount,
    /// Releasing the requested amount would exceed the semaphore maximum.
    CountLimitExceeded,
    /// A timer duration is negative or cannot be represented.
    InvalidDuration,
    /// A handle with blocked waiters cannot be closed by this bootstrap yet.
    WaitersPending,
    /// A shared-memory block size is zero or not page-aligned.
    InvalidMemoryBlockSize,
    /// A shared-memory permission value is outside the supported R/W range.
    InvalidMemoryPermission,
    /// Host allocation for a shared-memory block failed.
    OutOfMemory,
    /// The selected address or block does not describe a valid mapping.
    InvalidMemoryBlockMapping,
}

#[derive(Debug, PartialEq, Eq)]
struct KernelEvent {
    reset_type: EventResetType,
    signaled: bool,
    waiters: VecDeque<ThreadId>,
}

#[derive(Debug, PartialEq, Eq)]
struct KernelMutex {
    owner: Option<ThreadId>,
    waiters: VecDeque<ThreadId>,
}

#[derive(Debug, PartialEq, Eq)]
struct KernelSemaphore {
    count: u32,
    maximum: u32,
    waiters: VecDeque<ThreadId>,
}

#[derive(Debug, PartialEq, Eq)]
struct KernelTimer {
    reset_type: EventResetType,
    signaled: bool,
    deadline_ns: Option<u64>,
    interval_ns: u64,
    waiters: VecDeque<ThreadId>,
}

#[derive(Debug, PartialEq, Eq)]
struct KernelMemoryBlock {
    backing: SharedMemoryBacking,
    owner_permissions: u32,
    other_permissions: u32,
}

/// HLE service endpoint attached to a process-local session handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceSessionKind {
    ServiceManager,
    GspGpu {
        client_id: Option<u8>,
        shared_memory_handle: Option<u32>,
        event_handle: Option<u32>,
        flags: u32,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum KernelObject {
    Thread(ThreadId),
    Event(KernelEvent),
    Mutex(KernelMutex),
    Semaphore(KernelSemaphore),
    Timer(KernelTimer),
    MemoryBlock(KernelMemoryBlock),
    ServiceSession(ServiceSessionKind),
}

/// Process-local table mapping guest-visible handles to typed kernel objects.
///
/// Handles are monotonically allocated and are not reused after close. This
/// avoids stale-handle aliasing in the current bootstrap; kernel generations
/// and reference counting are not yet emulated.
#[derive(Debug, PartialEq, Eq)]
pub struct Arm11KernelObjectTable {
    objects: BTreeMap<u32, KernelObject>,
    next_handle: u32,
}

impl Default for Arm11KernelObjectTable {
    fn default() -> Self {
        Self {
            objects: BTreeMap::new(),
            next_handle: FIRST_HANDLE,
        }
    }
}

impl Arm11KernelObjectTable {
    /// Creates a zero-initialized, page-aligned shared-memory block.
    ///
    /// Permission values use bit 0 for read and bit 1 for write. The shared
    /// backing can be mapped into multiple process address spaces without copying.
    pub fn create_memory_block(
        &mut self,
        size: u32,
        owner_permissions: u32,
        other_permissions: u32,
    ) -> Result<u32, KernelHandleError> {
        if size == 0 || size & 0xFFF != 0 {
            return Err(KernelHandleError::InvalidMemoryBlockSize);
        }
        if owner_permissions > 3
            || other_permissions > 3
            || owner_permissions == 2
            || other_permissions == 2
        {
            return Err(KernelHandleError::InvalidMemoryPermission);
        }
        let backing = SharedMemoryBacking::zeroed(size as usize)
            .map_err(|_| KernelHandleError::OutOfMemory)?;
        self.create_memory_block_with_backing(backing, owner_permissions, other_permissions)
    }

    /// Creates a memory-block handle over an existing shared backing without copying.
    pub fn create_memory_block_with_backing(
        &mut self,
        backing: SharedMemoryBacking,
        owner_permissions: u32,
        other_permissions: u32,
    ) -> Result<u32, KernelHandleError> {
        let size = backing.len().ok_or(KernelHandleError::OutOfMemory)?;
        if size == 0 || size & 0xFFF != 0 {
            return Err(KernelHandleError::InvalidMemoryBlockSize);
        }
        if owner_permissions > 3
            || other_permissions > 3
            || owner_permissions == 2
            || other_permissions == 2
        {
            return Err(KernelHandleError::InvalidMemoryPermission);
        }
        self.insert(KernelObject::MemoryBlock(KernelMemoryBlock {
            backing,
            owner_permissions,
            other_permissions,
        }))
    }

    /// Retrieves the shared backing associated with a memory-block handle.
    pub fn memory_block_backing(
        &self,
        handle: u32,
    ) -> Result<SharedMemoryBacking, KernelHandleError> {
        match self.objects.get(&handle) {
            Some(KernelObject::MemoryBlock(block)) => Ok(block.backing.clone()),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    /// Returns the creator and peer-process permissions for a memory block.
    pub fn memory_block_permissions(&self, handle: u32) -> Option<(u32, u32)> {
        match self.objects.get(&handle) {
            Some(KernelObject::MemoryBlock(block)) => {
                Some((block.owner_permissions, block.other_permissions))
            }
            _ => None,
        }
    }

    /// Creates a process-local service-manager client session.
    pub fn insert_service_manager_session(&mut self) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::ServiceSession(ServiceSessionKind::ServiceManager))
    }

    /// Creates a process-local GSP GPU client session.
    pub fn insert_gsp_gpu_session(&mut self) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::ServiceSession(ServiceSessionKind::GspGpu {
            client_id: None,
            shared_memory_handle: None,
            event_handle: None,
            flags: 0,
        }))
    }

    /// Returns the service endpoint state associated with a guest handle.
    pub fn service_session(&self, handle: u32) -> Option<ServiceSessionKind> {
        match self.objects.get(&handle) {
            Some(KernelObject::ServiceSession(kind)) => Some(*kind),
            _ => None,
        }
    }

    /// Updates the registration data retained by a GSP service session.
    pub fn set_gsp_session_registration(
        &mut self,
        handle: u32,
        client_id: Option<u8>,
        shared_memory_handle: Option<u32>,
        event_handle: Option<u32>,
        flags: u32,
    ) -> Result<(), KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::ServiceSession(ServiceSessionKind::GspGpu {
                client_id: current_client_id,
                shared_memory_handle: current_memory_handle,
                event_handle: current_event_handle,
                flags: current_flags,
            })) => {
                *current_client_id = client_id;
                *current_memory_handle = shared_memory_handle;
                *current_event_handle = event_handle;
                *current_flags = flags;
                Ok(())
            }
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    /// Inserts a newly created thread and returns its guest-visible handle.
    pub fn insert_thread(&mut self, thread_id: ThreadId) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Thread(thread_id))
    }

    /// Creates an unsignaled event and returns its guest-visible handle.
    pub fn insert_event(&mut self, reset_type: EventResetType) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Event(KernelEvent {
            reset_type,
            signaled: false,
            waiters: VecDeque::new(),
        }))
    }

    /// Creates an inactive timer with the selected auto/manual reset behavior.
    pub fn insert_timer(&mut self, reset_type: EventResetType) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Timer(KernelTimer {
            reset_type,
            signaled: false,
            deadline_ns: None,
            interval_ns: 0,
            waiters: VecDeque::new(),
        }))
    }

    /// Creates a mutex and optionally assigns initial ownership to the creator.
    pub fn insert_mutex(
        &mut self,
        initially_locked: bool,
        creator: ThreadId,
    ) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Mutex(KernelMutex {
            owner: initially_locked.then_some(creator),
            waiters: VecDeque::new(),
        }))
    }

    /// Creates a semaphore after validating its initial and maximum counts.
    pub fn insert_semaphore(
        &mut self,
        initial_count: i32,
        maximum_count: i32,
    ) -> Result<u32, KernelHandleError> {
        if initial_count < 0 || maximum_count <= 0 || initial_count > maximum_count {
            return Err(KernelHandleError::InvalidCount);
        }
        self.insert(KernelObject::Semaphore(KernelSemaphore {
            count: initial_count as u32,
            maximum: maximum_count as u32,
            waiters: VecDeque::new(),
        }))
    }

    /// Resolves a guest thread handle, including the current-thread pseudo-handle.
    pub fn thread_id(&self, handle: u32, current_thread: ThreadId) -> Option<ThreadId> {
        if handle == CURRENT_THREAD_HANDLE {
            return Some(current_thread);
        }

        match self.objects.get(&handle) {
            Some(KernelObject::Thread(thread_id)) => Some(*thread_id),
            _ => None,
        }
    }

    /// Returns the reset mode of an event handle, if it is a valid event.
    pub fn event_reset_type(&self, handle: u32) -> Option<EventResetType> {
        match self.objects.get(&handle) {
            Some(KernelObject::Event(event)) => Some(event.reset_type),
            _ => None,
        }
    }

    /// Returns the signal state of an event handle, if it is a valid event.
    pub fn event_is_signaled(&self, handle: u32) -> Option<bool> {
        match self.objects.get(&handle) {
            Some(KernelObject::Event(event)) => Some(event.signaled),
            _ => None,
        }
    }

    /// Returns the signal state of a timer handle, if it is a valid timer.
    pub fn timer_is_signaled(&self, handle: u32) -> Option<bool> {
        match self.objects.get(&handle) {
            Some(KernelObject::Timer(timer)) => Some(timer.signaled),
            _ => None,
        }
    }

    /// Arms a timer relative to the current virtual clock.
    ///
    /// A zero initial duration expires on the next call to advance_time at the
    /// current time. A zero interval makes this a one-shot timer; positive
    /// intervals schedule periodic expirations. Negative durations are invalid.
    pub fn set_timer(
        &mut self,
        handle: u32,
        now_ns: u64,
        initial_ns: i64,
        interval_ns: i64,
    ) -> Result<(), KernelHandleError> {
        if initial_ns < 0 || interval_ns < 0 {
            return Err(KernelHandleError::InvalidDuration);
        }
        let timer = self.timer_mut(handle)?;
        timer.signaled = false;
        timer.deadline_ns = Some(now_ns.saturating_add(initial_ns as u64));
        timer.interval_ns = interval_ns as u64;
        Ok(())
    }

    /// Stops future expirations without changing an already signaled state.
    pub fn cancel_timer(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        let timer = self.timer_mut(handle)?;
        timer.deadline_ns = None;
        timer.interval_ns = 0;
        Ok(())
    }

    /// Clears a timer's signaled state without changing its active deadline.
    pub fn clear_timer(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        let timer = self.timer_mut(handle)?;
        timer.signaled = false;
        Ok(())
    }

    /// Attempts to acquire a synchronization object without blocking.
    ///
    /// One-shot events reset after a successful wait; sticky events remain
    /// signaled. Mutex acquisition records the current thread as owner, and
    /// semaphore acquisition consumes one count.
    pub fn try_wait_synchronization(
        &mut self,
        handle: u32,
        thread: ThreadId,
    ) -> Result<bool, KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::Event(event)) => {
                if !event.signaled {
                    return Ok(false);
                }
                if event.reset_type == EventResetType::OneShot {
                    event.signaled = false;
                }
                Ok(true)
            }
            Some(KernelObject::Mutex(mutex)) => match mutex.owner {
                None => {
                    mutex.owner = Some(thread);
                    Ok(true)
                }
                Some(_) => Ok(false),
            },
            Some(KernelObject::Semaphore(semaphore)) => {
                if semaphore.count == 0 {
                    return Ok(false);
                }
                semaphore.count -= 1;
                Ok(true)
            }
            Some(KernelObject::Timer(timer)) => {
                if !timer.signaled {
                    return Ok(false);
                }
                if timer.reset_type == EventResetType::OneShot {
                    timer.signaled = false;
                }
                Ok(true)
            }
            Some(KernelObject::Thread(_)) | Some(KernelObject::MemoryBlock(_)) => {
                Err(KernelHandleError::WrongType)
            }
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    /// Adds a thread to a synchronization object's FIFO wait queue.
    pub fn enqueue_waiter(
        &mut self,
        handle: u32,
        thread: ThreadId,
    ) -> Result<(), KernelHandleError> {
        let waiters = match self.objects.get_mut(&handle) {
            Some(KernelObject::Event(event)) => &mut event.waiters,
            Some(KernelObject::Mutex(mutex)) => &mut mutex.waiters,
            Some(KernelObject::Semaphore(semaphore)) => &mut semaphore.waiters,
            Some(KernelObject::Timer(timer)) => &mut timer.waiters,
            Some(KernelObject::Thread(_)) | Some(KernelObject::MemoryBlock(_)) => {
                return Err(KernelHandleError::WrongType);
            }
            None => return Err(KernelHandleError::InvalidHandle),
        };
        if !waiters.contains(&thread) {
            waiters.push_back(thread);
        }
        Ok(())
    }

    /// Removes one thread from every synchronization-object wait queue.
    ///
    /// This is used when a finite wait times out, so a later signal cannot
    /// wake a thread that has already returned RESULT_TIMEOUT.
    pub fn remove_waiter(&mut self, thread: ThreadId) -> bool {
        let mut removed = false;
        for object in self.objects.values_mut() {
            let waiters = match object {
                KernelObject::Event(event) => &mut event.waiters,
                KernelObject::Mutex(mutex) => &mut mutex.waiters,
                KernelObject::Semaphore(semaphore) => &mut semaphore.waiters,
                KernelObject::Timer(timer) => &mut timer.waiters,
                KernelObject::Thread(_) | KernelObject::MemoryBlock(_) => continue,
            };
            let previous_len = waiters.len();
            waiters.retain(|waiter| *waiter != thread);
            removed |= waiters.len() != previous_len;
        }
        removed
    }

    /// Advances timers to the supplied virtual clock value.
    ///
    /// Auto-reset timers release at most one FIFO waiter per expiration. Sticky
    /// timers release every waiter and remain signaled until ClearTimer. When
    /// several periodic ticks elapse between scheduler updates, expirations are
    /// coalesced into one signal and the next deadline is advanced beyond now.
    pub fn advance_time(&mut self, now_ns: u64) -> Vec<ThreadId> {
        let mut awakened = Vec::new();

        for object in self.objects.values_mut() {
            let KernelObject::Timer(timer) = object else {
                continue;
            };
            let Some(deadline_ns) = timer.deadline_ns else {
                continue;
            };
            if deadline_ns > now_ns {
                continue;
            }

            if timer.interval_ns == 0 {
                timer.deadline_ns = None;
            } else {
                let elapsed = now_ns.saturating_sub(deadline_ns);
                let steps = elapsed
                    .checked_div(timer.interval_ns)
                    .unwrap_or(0)
                    .saturating_add(1);
                let next = deadline_ns.saturating_add(timer.interval_ns.saturating_mul(steps));
                timer.deadline_ns = (next > now_ns).then_some(next);
            }

            match timer.reset_type {
                EventResetType::OneShot => {
                    if let Some(waiter) = timer.waiters.pop_front() {
                        timer.signaled = false;
                        awakened.push(waiter);
                    } else {
                        timer.signaled = true;
                    }
                }
                EventResetType::Sticky => {
                    timer.signaled = true;
                    awakened.extend(timer.waiters.drain(..));
                }
            }
        }

        awakened
    }

    /// Sets an event signaled and returns the threads released by the signal.
    pub fn signal_event(&mut self, handle: u32) -> Result<Vec<ThreadId>, KernelHandleError> {
        let event = self.event_mut(handle)?;
        match event.reset_type {
            EventResetType::Sticky => {
                event.signaled = true;
                Ok(event.waiters.drain(..).collect())
            }
            EventResetType::OneShot => {
                if let Some(waiter) = event.waiters.pop_front() {
                    event.signaled = false;
                    Ok(vec![waiter])
                } else {
                    event.signaled = true;
                    Ok(Vec::new())
                }
            }
        }
    }

    /// Clears an event's signal state.
    pub fn clear_event(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        let event = self.event_mut(handle)?;
        event.signaled = false;
        Ok(())
    }

    /// Releases a mutex and transfers ownership to its oldest queued waiter.
    pub fn release_mutex(
        &mut self,
        handle: u32,
        owner: ThreadId,
    ) -> Result<Option<ThreadId>, KernelHandleError> {
        let mutex = self.mutex_mut(handle)?;
        if mutex.owner != Some(owner) {
            return Err(KernelHandleError::NotOwner);
        }
        let next_owner = mutex.waiters.pop_front();
        mutex.owner = next_owner;
        Ok(next_owner)
    }

    /// Releases semaphore units, waking queued waiters before increasing count.
    ///
    /// Returns the number of waiters handed a unit immediately.
    pub fn release_semaphore(
        &mut self,
        handle: u32,
        release_count: u32,
    ) -> Result<Vec<ThreadId>, KernelHandleError> {
        if release_count == 0 {
            return Err(KernelHandleError::InvalidCount);
        }
        let semaphore = self.semaphore_mut(handle)?;
        let waiter_count = release_count.min(semaphore.waiters.len() as u32);
        let units_to_store = release_count - waiter_count;
        if semaphore.count.saturating_add(units_to_store) > semaphore.maximum {
            return Err(KernelHandleError::CountLimitExceeded);
        }

        let mut awakened = Vec::with_capacity(waiter_count as usize);
        for _ in 0..waiter_count {
            if let Some(waiter) = semaphore.waiters.pop_front() {
                awakened.push(waiter);
            }
        }
        semaphore.count += units_to_store;
        Ok(awakened)
    }

    /// Returns a mutex's current owner, if the handle refers to a mutex.
    pub fn mutex_owner(&self, handle: u32) -> Option<ThreadId> {
        match self.objects.get(&handle) {
            Some(KernelObject::Mutex(mutex)) => mutex.owner,
            _ => None,
        }
    }

    /// Returns the current and maximum count of a semaphore handle.
    pub fn semaphore_counts(&self, handle: u32) -> Option<(u32, u32)> {
        match self.objects.get(&handle) {
            Some(KernelObject::Semaphore(semaphore)) => Some((semaphore.count, semaphore.maximum)),
            _ => None,
        }
    }

    /// Closes a regular process-local handle.
    ///
    /// Pseudo-handles are not stored in the table and therefore cannot be closed.
    pub fn close_handle(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        let has_waiters = match self.objects.get(&handle) {
            Some(KernelObject::Event(event)) => !event.waiters.is_empty(),
            Some(KernelObject::Mutex(mutex)) => !mutex.waiters.is_empty(),
            Some(KernelObject::Semaphore(semaphore)) => !semaphore.waiters.is_empty(),
            Some(KernelObject::Timer(timer)) => !timer.waiters.is_empty(),
            Some(KernelObject::Thread(_) | KernelObject::MemoryBlock(_) | KernelObject::ServiceSession(_)) => false,
            None => return Err(KernelHandleError::InvalidHandle),
        };
        if has_waiters {
            return Err(KernelHandleError::WaitersPending);
        }
        self.objects.remove(&handle);
        Ok(())
    }

    /// Returns the number of currently open regular handles.
    pub fn handle_count(&self) -> usize {
        self.objects.len()
    }

    fn mutex_mut(&mut self, handle: u32) -> Result<&mut KernelMutex, KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::Mutex(mutex)) => Ok(mutex),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn semaphore_mut(&mut self, handle: u32) -> Result<&mut KernelSemaphore, KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::Semaphore(semaphore)) => Ok(semaphore),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn timer_mut(&mut self, handle: u32) -> Result<&mut KernelTimer, KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::Timer(timer)) => Ok(timer),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn event_mut(&mut self, handle: u32) -> Result<&mut KernelEvent, KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::Event(event)) => Ok(event),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn insert(&mut self, object: KernelObject) -> Result<u32, KernelHandleError> {
        let mut handle = self.next_handle;
        while self.objects.contains_key(&handle) || handle == CURRENT_THREAD_HANDLE {
            handle = handle.checked_add(1).ok_or(KernelHandleError::Exhausted)?;
        }

        self.next_handle = handle.checked_add(1).ok_or(KernelHandleError::Exhausted)?;
        self.objects.insert(handle, object);
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_id(value: u32) -> ThreadId {
        // ThreadId intentionally exposes no public constructor.
        let mut scheduler =
            super::super::thread::Arm11ThreadScheduler::new(crate::cpu::Arm11Core::new(), 0x30)
                .unwrap();
        if value == 1 {
            super::super::thread::MAIN_THREAD_ID
        } else {
            scheduler.create_thread(0x1000, 0, 0x2000, 0x30, 0).unwrap()
        }
    }

    #[test]
    fn memory_blocks_validate_size_permissions_and_preserve_shared_backing() {
        let mut table = Arm11KernelObjectTable::default();
        let handle = table.create_memory_block(0x1000, 3, 1).unwrap();

        assert_eq!(table.memory_block_permissions(handle), Some((3, 1)));
        assert_eq!(
            table.create_memory_block(0, 3, 1),
            Err(KernelHandleError::InvalidMemoryBlockSize)
        );
        assert_eq!(
            table.create_memory_block(0x1000, 4, 1),
            Err(KernelHandleError::InvalidMemoryPermission)
        );
        assert_eq!(
            table.create_memory_block(0x1000, 2, 1),
            Err(KernelHandleError::InvalidMemoryPermission)
        );
        assert_eq!(
            table.create_memory_block(0x1000, 3, 2),
            Err(KernelHandleError::InvalidMemoryPermission)
        );

        let backing = table.memory_block_backing(handle).unwrap();
        let mapped_reference = backing.clone();
        assert!(backing.shares_backing_with(&mapped_reference));
        assert_eq!(backing.len(), Some(0x1000));
        assert_eq!(table.close_handle(handle), Ok(()));
        assert_eq!(
            table.memory_block_backing(handle),
            Err(KernelHandleError::InvalidHandle)
        );
        assert_eq!(mapped_reference.len(), Some(0x1000));
    }

    #[test]
    fn allocates_typed_handles_and_resolves_the_current_thread_pseudo_handle() {
        let current = thread_id(1);
        let child = thread_id(2);
        let mut table = Arm11KernelObjectTable::default();

        let thread_handle = table.insert_thread(child).unwrap();
        let event_handle = table.insert_event(EventResetType::Sticky).unwrap();

        assert_ne!(thread_handle, event_handle);
        assert_eq!(table.thread_id(thread_handle, current), Some(child));
        assert_eq!(
            table.thread_id(CURRENT_THREAD_HANDLE, current),
            Some(current)
        );
        assert_eq!(table.thread_id(event_handle, current), None);
    }

    #[test]
    fn event_signal_clear_and_close_preserve_object_type_safety() {
        let mut table = Arm11KernelObjectTable::default();
        let event = table.insert_event(EventResetType::OneShot).unwrap();

        assert_eq!(table.event_is_signaled(event), Some(false));
        assert_eq!(table.event_reset_type(event), Some(EventResetType::OneShot));
        table.signal_event(event).unwrap();
        assert_eq!(table.event_is_signaled(event), Some(true));
        table.clear_event(event).unwrap();
        assert_eq!(table.event_is_signaled(event), Some(false));
        assert_eq!(table.close_handle(event), Ok(()));
        assert_eq!(table.event_is_signaled(event), None);
        assert_eq!(
            table.close_handle(CURRENT_THREAD_HANDLE),
            Err(KernelHandleError::InvalidHandle)
        );
    }

    #[test]
    fn event_operations_reject_thread_handles_and_unknown_handles() {
        let current = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let thread_handle = table.insert_thread(current).unwrap();

        assert_eq!(
            table.signal_event(thread_handle),
            Err(KernelHandleError::WrongType)
        );
        assert_eq!(
            table.clear_event(0xDEAD),
            Err(KernelHandleError::InvalidHandle)
        );
        assert_eq!(table.handle_count(), 1);
    }

    #[test]
    fn mutex_ownership_is_transferred_only_to_the_current_owner_or_next_waiter() {
        let owner = thread_id(1);
        let waiter = thread_id(2);
        let mut table = Arm11KernelObjectTable::default();
        let mutex = table.insert_mutex(true, owner).unwrap();

        assert_eq!(table.mutex_owner(mutex), Some(owner));
        assert!(!table.try_wait_synchronization(mutex, waiter).unwrap());
        table.enqueue_waiter(mutex, waiter).unwrap();
        assert_eq!(
            table.release_mutex(mutex, waiter),
            Err(KernelHandleError::NotOwner)
        );
        assert_eq!(table.release_mutex(mutex, owner), Ok(Some(waiter)));
        assert_eq!(table.mutex_owner(mutex), Some(waiter));
        assert_eq!(table.release_mutex(mutex, waiter), Ok(None));
        assert_eq!(table.mutex_owner(mutex), None);
    }

    #[test]
    fn semaphore_validates_counts_and_wakes_fifo_waiters_before_storing_units() {
        let first = thread_id(1);
        let second = thread_id(2);
        let third = thread_id(3);
        let mut table = Arm11KernelObjectTable::default();

        assert_eq!(
            table.insert_semaphore(-1, 2),
            Err(KernelHandleError::InvalidCount)
        );
        assert_eq!(
            table.insert_semaphore(3, 2),
            Err(KernelHandleError::InvalidCount)
        );

        let semaphore = table.insert_semaphore(1, 2).unwrap();
        assert!(table.try_wait_synchronization(semaphore, first).unwrap());
        assert!(!table.try_wait_synchronization(semaphore, first).unwrap());
        table.enqueue_waiter(semaphore, first).unwrap();
        table.enqueue_waiter(semaphore, second).unwrap();

        assert_eq!(
            table.release_semaphore(semaphore, 5),
            Err(KernelHandleError::CountLimitExceeded)
        );
        assert_eq!(table.semaphore_counts(semaphore), Some((0, 2)));

        assert_eq!(
            table.release_semaphore(semaphore, 2).unwrap(),
            vec![first, second]
        );
        assert_eq!(table.semaphore_counts(semaphore), Some((0, 2)));
        assert!(!table.try_wait_synchronization(semaphore, third).unwrap());
        assert_eq!(table.semaphore_counts(semaphore), Some((0, 2)));
    }

    #[test]
    fn removing_a_timed_out_waiter_prevents_a_stale_signal_wakeup() {
        let waiter = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let event = table.insert_event(EventResetType::OneShot).unwrap();
        table.enqueue_waiter(event, waiter).unwrap();

        assert!(table.remove_waiter(waiter));
        assert!(!table.remove_waiter(waiter));
        assert!(table.signal_event(event).unwrap().is_empty());
        assert_eq!(table.event_is_signaled(event), Some(true));
        assert_eq!(table.close_handle(event), Ok(()));
    }

    #[test]
    fn one_shot_event_hands_a_signal_to_one_waiter_and_sticky_wakes_all() {
        let first = thread_id(1);
        let second = thread_id(2);
        let mut table = Arm11KernelObjectTable::default();
        let one_shot = table.insert_event(EventResetType::OneShot).unwrap();
        table.enqueue_waiter(one_shot, first).unwrap();
        table.enqueue_waiter(one_shot, second).unwrap();
        assert_eq!(table.signal_event(one_shot).unwrap(), vec![first]);
        assert_eq!(table.event_is_signaled(one_shot), Some(false));
        assert_eq!(
            table.close_handle(one_shot),
            Err(KernelHandleError::WaitersPending)
        );

        let sticky = table.insert_event(EventResetType::Sticky).unwrap();
        table.enqueue_waiter(sticky, first).unwrap();
        table.enqueue_waiter(sticky, second).unwrap();
        assert_eq!(table.signal_event(sticky).unwrap(), vec![first, second]);
        assert_eq!(table.event_is_signaled(sticky), Some(true));
        assert_eq!(table.try_wait_synchronization(sticky, first), Ok(true));
        assert_eq!(table.event_is_signaled(sticky), Some(true));
    }

    #[test]
    fn one_shot_timer_expiration_releases_one_fifo_waiter_and_consumes_signal() {
        let first = thread_id(1);
        let second = thread_id(2);
        let mut table = Arm11KernelObjectTable::default();
        let timer = table.insert_timer(EventResetType::OneShot).unwrap();

        table.enqueue_waiter(timer, first).unwrap();
        table.enqueue_waiter(timer, second).unwrap();
        table.set_timer(timer, 100, 10, 0).unwrap();

        assert!(table.advance_time(109).is_empty());
        assert_eq!(table.advance_time(110), vec![first]);
        assert_eq!(table.timer_is_signaled(timer), Some(false));
        assert!(table.advance_time(500).is_empty());
        assert_eq!(
            table.close_handle(timer),
            Err(KernelHandleError::WaitersPending)
        );
    }

    #[test]
    fn periodic_sticky_timer_coalesces_missed_ticks_and_can_be_cleared_or_cancelled() {
        let first = thread_id(1);
        let second = thread_id(2);
        let mut table = Arm11KernelObjectTable::default();
        let timer = table.insert_timer(EventResetType::Sticky).unwrap();

        table.enqueue_waiter(timer, first).unwrap();
        table.enqueue_waiter(timer, second).unwrap();
        table.set_timer(timer, 0, 5, 5).unwrap();
        assert_eq!(table.advance_time(5), vec![first, second]);
        assert_eq!(table.timer_is_signaled(timer), Some(true));
        assert!(table.try_wait_synchronization(timer, first).unwrap());

        table.clear_timer(timer).unwrap();
        assert_eq!(table.timer_is_signaled(timer), Some(false));
        table.enqueue_waiter(timer, first).unwrap();
        assert_eq!(table.advance_time(35), vec![first]);
        assert_eq!(table.timer_is_signaled(timer), Some(true));

        table.cancel_timer(timer).unwrap();
        table.clear_timer(timer).unwrap();
        assert!(table.advance_time(100).is_empty());
        assert_eq!(table.timer_is_signaled(timer), Some(false));
    }

    #[test]
    fn timer_rejects_negative_durations_and_wrong_handle_types() {
        let mut table = Arm11KernelObjectTable::default();
        let timer = table.insert_timer(EventResetType::OneShot).unwrap();
        let event = table.insert_event(EventResetType::OneShot).unwrap();

        assert_eq!(
            table.set_timer(timer, 0, -1, 0),
            Err(KernelHandleError::InvalidDuration)
        );
        assert_eq!(
            table.set_timer(event, 0, 1, 0),
            Err(KernelHandleError::WrongType)
        );
        assert_eq!(
            table.cancel_timer(0xDEAD),
            Err(KernelHandleError::InvalidHandle)
        );
    }

    #[test]
    fn closed_handles_are_not_reused() {
        let mut table = Arm11KernelObjectTable::default();
        let first = table.insert_event(EventResetType::Sticky).unwrap();
        table.close_handle(first).unwrap();
        let second = table.insert_event(EventResetType::Sticky).unwrap();

        assert_ne!(first, second);
    }
}
