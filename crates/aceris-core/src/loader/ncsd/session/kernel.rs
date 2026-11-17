//! Process-local ARM11 kernel handles and synchronization objects.
//!
//! The process-local table models typed handles, shared object identity for
//! duplicated handles, FIFO synchronization waiters, mutex ownership,
//! semaphore counts, events and virtual-clock timers. Explicit kernel refcount
//! metadata and generation-tagged handle recycling remain future work.

use std::collections::{BTreeMap, VecDeque};

use super::thread::ThreadId;
use crate::loader::ncsd::process::SharedMemoryBacking;

/// Pseudo-handle that always identifies the calling thread.
pub const CURRENT_THREAD_HANDLE: u32 = 0xFFFF_8000;
/// Pseudo-handle that always identifies the calling process.
pub const CURRENT_PROCESS_HANDLE: u32 = 0xFFFF_8001;

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
    /// An operation cannot proceed while the referenced object has pending waiters.
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
struct KernelThread {
    id: ThreadId,
    terminated: bool,
    waiters: VecDeque<ThreadId>,
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
    ServiceManager {
        client_registered: bool,
    },
    GspGpu {
        client_id: Option<u8>,
        shared_memory_handle: Option<u32>,
        event_handle: Option<u32>,
        flags: u32,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum KernelObject {
    Thread(KernelThread),
    Event(KernelEvent),
    Mutex(KernelMutex),
    Semaphore(KernelSemaphore),
    Timer(KernelTimer),
    MemoryBlock(KernelMemoryBlock),
    ServiceSession(ServiceSessionKind),
}

/// Process-local table mapping guest-visible handles to typed kernel objects.
///
/// The first handle allocated for an object is its stable table key. Duplicate
/// handles map to that key, so closing one handle never changes object identity.
/// Closed object anchors remain in the retired table while aliases or internal
/// synchronization references still exist. Guest-visible handles are monotonic
/// and are not reused after close.
#[derive(Debug, PartialEq, Eq)]
pub struct Arm11KernelObjectTable {
    /// Objects whose original handle is still open.
    objects: BTreeMap<u32, KernelObject>,
    /// Closed object anchors retained by aliases or internal references.
    retired_objects: BTreeMap<u32, KernelObject>,
    /// Duplicate guest handles mapped to their stable object anchor.
    handle_aliases: BTreeMap<u32, u32>,
    /// Handle used by each thread when it queued on a particular object.
    waiter_handles: BTreeMap<(ThreadId, u32), u32>,
    /// Original handle and object anchor pinned by each pending wait-all.
    wait_all_refs: BTreeMap<ThreadId, Vec<(u32, u32)>>,
    /// Highest simultaneous count of open guest-visible handles in this process.
    high_water_handle_count: usize,
    next_handle: u32,
    /// Handle that most recently handed an object to a blocked thread, so a
    /// WaitSynchronizationN waiter can report which handle released it.
    handoffs: BTreeMap<ThreadId, u32>,
}

impl Default for Arm11KernelObjectTable {
    fn default() -> Self {
        Self {
            objects: BTreeMap::new(),
            retired_objects: BTreeMap::new(),
            handle_aliases: BTreeMap::new(),
            waiter_handles: BTreeMap::new(),
            wait_all_refs: BTreeMap::new(),
            high_water_handle_count: 0,
            next_handle: FIRST_HANDLE,
            handoffs: BTreeMap::new(),
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
        match self.object(handle) {
            Some(KernelObject::MemoryBlock(block)) => Ok(block.backing.clone()),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    /// Returns the creator and peer-process permissions for a memory block.
    pub fn memory_block_permissions(&self, handle: u32) -> Option<(u32, u32)> {
        match self.object(handle) {
            Some(KernelObject::MemoryBlock(block)) => {
                Some((block.owner_permissions, block.other_permissions))
            }
            _ => None,
        }
    }

    /// Creates a process-local service-manager client session.
    pub fn insert_service_manager_session(&mut self) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::ServiceSession(
            ServiceSessionKind::ServiceManager {
                client_registered: false,
            },
        ))
    }

    /// Marks a service-manager client session as registered with srv:.
    pub fn register_service_manager_client(
        &mut self,
        handle: u32,
    ) -> Result<(), KernelHandleError> {
        match self.object_mut(handle) {
            Some(KernelObject::ServiceSession(ServiceSessionKind::ServiceManager {
                client_registered,
            })) => {
                *client_registered = true;
                Ok(())
            }
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    /// Reports whether a service-manager session completed RegisterClient.
    pub fn service_manager_client_registered(&self, handle: u32) -> Option<bool> {
        match self.object(handle) {
            Some(KernelObject::ServiceSession(ServiceSessionKind::ServiceManager {
                client_registered,
            })) => Some(*client_registered),
            _ => None,
        }
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
        match self.object(handle) {
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
        match self.object_mut(handle) {
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

    /// Inserts a newly created, initially unsignaled thread object.
    pub fn insert_thread(&mut self, thread_id: ThreadId) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Thread(KernelThread {
            id: thread_id,
            terminated: false,
            waiters: VecDeque::new(),
        }))
    }

    /// Marks a thread object signaled when its owning thread permanently exits.
    ///
    /// Thread objects are manual-reset synchronization objects: every waiter
    /// is released, and future waits continue to succeed while the handle stays
    /// open. A thread may have more than one handle in the process table.
    pub fn signal_thread_exit(&mut self, thread_id: ThreadId) -> Vec<ThreadId> {
        let mut awakened = Vec::new();
        let mut handoffs = Vec::new();

        for (handle, object) in self
            .objects
            .iter_mut()
            .chain(self.retired_objects.iter_mut())
        {
            let KernelObject::Thread(thread) = object else {
                continue;
            };
            if thread.id != thread_id {
                continue;
            }

            thread.terminated = true;
            for waiter in thread.waiters.drain(..) {
                handoffs.push((waiter, *handle));
                if !awakened.contains(&waiter) {
                    awakened.push(waiter);
                }
            }
        }

        for (waiter, object_key) in handoffs {
            let handle = self
                .waiter_handles
                .remove(&(waiter, object_key))
                .unwrap_or(object_key);
            self.handoffs.insert(waiter, handle);
        }
        awakened
    }

    /// Returns whether a thread handle has observed permanent thread exit.
    pub fn thread_is_terminated(&self, handle: u32) -> Option<bool> {
        match self.object(handle) {
            Some(KernelObject::Thread(thread)) => Some(thread.terminated),
            _ => None,
        }
    }

    /// Returns the target thread associated with a regular thread handle.
    pub fn thread_handle_target(&self, handle: u32) -> Option<ThreadId> {
        match self.object(handle) {
            Some(KernelObject::Thread(thread)) => Some(thread.id),
            _ => None,
        }
    }

    /// Counts the process-local handles that keep a thread object alive.
    pub fn thread_handle_count(&self, thread_id: ThreadId) -> usize {
        let primary_handles = self
            .objects
            .values()
            .filter(
                |object| matches!(object, KernelObject::Thread(thread) if thread.id == thread_id),
            )
            .count();
        let duplicate_handles = self
            .handle_aliases
            .values()
            .filter_map(|key| self.object_by_key(*key))
            .filter(
                |object| matches!(object, KernelObject::Thread(thread) if thread.id == thread_id),
            )
            .count();
        primary_handles + duplicate_handles
    }

    /// Returns whether a handle is still open to guest SVC calls.
    pub fn is_open_handle(&self, handle: u32) -> bool {
        self.objects.contains_key(&handle) || self.handle_aliases.contains_key(&handle)
    }

    /// Returns the number of live objects, including objects pinned by waits.
    pub fn retained_object_count(&self) -> usize {
        self.objects.len() + self.retired_objects.len()
    }

    /// Reports the reference count that this process-local table currently models.
    ///
    /// The count includes open primary/duplicate handles, registered synchronization
    /// waiters, wait-all pins and mutex ownership. It deliberately excludes the
    /// temporary reference a GetHandleInfo SVC implementation would use itself.
    pub fn handle_reference_count(&self, handle: u32) -> Result<u64, KernelHandleError> {
        let object_key = self
            .object_key(handle)
            .ok_or(KernelHandleError::InvalidHandle)?;

        let primary_handle: u64 = if self.objects.contains_key(&object_key) {
            1
        } else {
            0
        };
        let duplicate_handles = self
            .handle_aliases
            .values()
            .filter(|key| **key == object_key)
            .count() as u64;
        let waiter_references = self
            .waiter_handles
            .keys()
            .filter(|(_, key)| *key == object_key)
            .count() as u64;
        let wait_all_references = self
            .wait_all_refs
            .values()
            .flat_map(|handles| handles.iter())
            .filter(|(_, key)| *key == object_key)
            .count() as u64;
        let owner_reference = if matches!(
            self.object_by_key(object_key),
            Some(KernelObject::Mutex(KernelMutex { owner: Some(_), .. }))
        ) {
            1
        } else {
            0
        };

        Ok(primary_handle
            .saturating_add(duplicate_handles)
            .saturating_add(waiter_references)
            .saturating_add(wait_all_references)
            .saturating_add(owner_reference))
    }

    /// Creates a new process-local handle that refers to the same kernel object.
    pub fn duplicate_handle(&mut self, original: u32) -> Result<u32, KernelHandleError> {
        let object_key = self
            .object_key(original)
            .ok_or(KernelHandleError::InvalidHandle)?;
        let duplicate = self.allocate_handle()?;
        self.handle_aliases.insert(duplicate, object_key);
        self.update_handle_high_water_mark();
        Ok(duplicate)
    }

    /// Pins every distinct guest handle in a wait-all request until the request
    /// completes or is cancelled. Aliases share one pinned object, but retaining
    /// each supplied handle preserves lookup after one or more aliases are closed.
    pub fn retain_wait_all_handles(
        &mut self,
        handles: &[u32],
        thread: ThreadId,
    ) -> Result<(), KernelHandleError> {
        let mut retained = Vec::new();
        for handle in handles.iter().copied() {
            let object_key = self
                .object_key(handle)
                .ok_or(KernelHandleError::InvalidHandle)?;
            if !retained
                .iter()
                .any(|(saved_handle, _)| *saved_handle == handle)
            {
                retained.push((handle, object_key));
            }
        }
        self.wait_all_refs.insert(thread, retained);
        Ok(())
    }

    /// Releases a wait-all request's internal references and reaps retired
    /// objects that no longer have registered waiters or wait-all pins.
    pub fn release_wait_all_handles(&mut self, thread: ThreadId) -> bool {
        let released = self.wait_all_refs.remove(&thread).is_some();
        self.collect_retired_objects();
        released
    }

    fn has_wait_all_reference(&self, object_key: u32) -> bool {
        self.wait_all_refs
            .values()
            .any(|handles| handles.iter().any(|(_, key)| *key == object_key))
    }

    fn wait_all_object_key(&self, handle: u32, thread: ThreadId) -> Option<u32> {
        self.object_key(handle).or_else(|| {
            self.wait_all_refs.get(&thread).and_then(|handles| {
                handles
                    .iter()
                    .find(|(wait_handle, _)| *wait_handle == handle)
                    .map(|(_, object_key)| *object_key)
            })
        })
    }

    fn wait_all_object(&self, handle: u32, thread: ThreadId) -> Option<&KernelObject> {
        let object_key = self.wait_all_object_key(handle, thread)?;
        self.object_by_key(object_key)
    }

    fn wait_all_object_mut(&mut self, handle: u32, thread: ThreadId) -> Option<&mut KernelObject> {
        let object_key = self.wait_all_object_key(handle, thread)?;
        self.object_mut_by_key(object_key)
    }

    fn collect_retired_objects(&mut self) {
        let pinned_objects = self
            .wait_all_refs
            .values()
            .flat_map(|handles| handles.iter().map(|(_, key)| *key))
            .collect::<std::collections::BTreeSet<_>>();
        let aliased_objects = self
            .handle_aliases
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        self.retired_objects.retain(|object_key, object| {
            pinned_objects.contains(object_key)
                || aliased_objects.contains(object_key)
                || object_has_internal_references(object)
        });
    }

    /// Releases every mutex owned by a terminating thread.
    ///
    /// Ownership is transferred to the oldest waiter on each mutex, just as
    /// ReleaseMutex does. A wait-any thread is handed at most one mutex: once
    /// selected, it is removed from every other synchronization queue before
    /// the next owned mutex is processed.
    pub fn release_owned_mutexes(&mut self, owner: ThreadId) -> Vec<ThreadId> {
        let owned_handles = self
            .objects
            .iter()
            .chain(self.retired_objects.iter())
            .filter_map(|(object_key, object)| {
                matches!(object, KernelObject::Mutex(mutex) if mutex.owner == Some(owner))
                    .then_some(*object_key)
            })
            .collect::<Vec<_>>();
        let mut awakened = Vec::new();

        for handle in owned_handles {
            let waiter = match self.object_mut_by_key(handle) {
                Some(KernelObject::Mutex(mutex)) => {
                    let waiter = mutex.waiters.pop_front();
                    mutex.owner = waiter;
                    waiter
                }
                _ => None,
            };

            let Some(waiter) = waiter else {
                continue;
            };
            let waiter_handle = self
                .waiter_handles
                .remove(&(waiter, handle))
                .unwrap_or(handle);
            self.handoffs.insert(waiter, waiter_handle);
            self.remove_waiter(waiter);
            if !awakened.contains(&waiter) {
                awakened.push(waiter);
            }
        }

        self.collect_retired_objects();
        awakened
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

        match self.object(handle) {
            Some(KernelObject::Thread(thread)) => Some(thread.id),
            _ => None,
        }
    }

    /// Returns the reset mode of an event handle, if it is a valid event.
    pub fn event_reset_type(&self, handle: u32) -> Option<EventResetType> {
        match self.object(handle) {
            Some(KernelObject::Event(event)) => Some(event.reset_type),
            _ => None,
        }
    }

    /// Returns the signal state of an event handle, if it is a valid event.
    pub fn event_is_signaled(&self, handle: u32) -> Option<bool> {
        match self.object(handle) {
            Some(KernelObject::Event(event)) => Some(event.signaled),
            _ => None,
        }
    }

    /// Returns the signal state of a timer handle, if it is a valid timer.
    pub fn timer_is_signaled(&self, handle: u32) -> Option<bool> {
        match self.object(handle) {
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
        match self.object_mut(handle) {
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
            Some(KernelObject::Thread(thread)) => Ok(thread.terminated),
            Some(KernelObject::MemoryBlock(_)) | Some(KernelObject::ServiceSession(_)) => {
                Err(KernelHandleError::WrongType)
            }
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    /// Atomically acquires every synchronization object in a wait-all request.
    ///
    /// Readiness is checked for the complete set before any one-shot event,
    /// timer, mutex or semaphore is consumed. Duplicate handles are treated as
    /// one object, so listing the same handle twice cannot consume it twice.
    /// An empty set remains pending, matching the kernel's zero-handle wait.
    pub fn try_wait_synchronization_all(
        &mut self,
        handles: &[u32],
        thread: ThreadId,
    ) -> Result<bool, KernelHandleError> {
        if handles.is_empty() {
            return Ok(false);
        }

        let mut all_ready = true;
        let mut checked_objects = std::collections::BTreeSet::new();
        for handle in handles.iter().copied() {
            let object_key = self
                .wait_all_object_key(handle, thread)
                .ok_or(KernelHandleError::InvalidHandle)?;
            if !checked_objects.insert(object_key) {
                continue;
            }

            let ready = match self.wait_all_object(handle, thread) {
                Some(KernelObject::Event(event)) => event.signaled,
                Some(KernelObject::Mutex(mutex)) => mutex.owner.is_none(),
                Some(KernelObject::Semaphore(semaphore)) => semaphore.count > 0,
                Some(KernelObject::Timer(timer)) => timer.signaled,
                Some(KernelObject::Thread(thread)) => thread.terminated,
                Some(KernelObject::MemoryBlock(_)) | Some(KernelObject::ServiceSession(_)) => {
                    return Err(KernelHandleError::WrongType);
                }
                None => return Err(KernelHandleError::InvalidHandle),
            };
            all_ready &= ready;
        }

        if !all_ready {
            return Ok(false);
        }

        // The preflight above guarantees that every operation below can
        // complete. No guest code or scheduler callback can interleave this
        // acquisition because the process kernel is driven cooperatively.
        let mut acquired_objects = std::collections::BTreeSet::new();
        for handle in handles.iter().copied() {
            let object_key = self
                .wait_all_object_key(handle, thread)
                .expect("wait-all preflight validated every object");
            if !acquired_objects.insert(object_key) {
                continue;
            }

            match self
                .wait_all_object_mut(handle, thread)
                .expect("wait-all preflight validated every unique handle")
            {
                KernelObject::Event(event) => {
                    if event.reset_type == EventResetType::OneShot {
                        event.signaled = false;
                    }
                }
                KernelObject::Mutex(mutex) => mutex.owner = Some(thread),
                KernelObject::Semaphore(semaphore) => semaphore.count -= 1,
                KernelObject::Timer(timer) => {
                    if timer.reset_type == EventResetType::OneShot {
                        timer.signaled = false;
                    }
                }
                // Thread objects are sticky: termination is never consumed.
                KernelObject::Thread(_) => {}
                KernelObject::MemoryBlock(_) | KernelObject::ServiceSession(_) => {
                    unreachable!("wait-all preflight rejected non-waitable objects")
                }
            }
        }

        Ok(true)
    }

    /// Adds a thread to a synchronization object's FIFO wait queue.
    pub fn enqueue_waiter(
        &mut self,
        handle: u32,
        thread: ThreadId,
    ) -> Result<(), KernelHandleError> {
        let object_key = self
            .object_key(handle)
            .ok_or(KernelHandleError::InvalidHandle)?;
        let inserted = {
            let waiters = match self.object_mut_by_key(object_key) {
                Some(KernelObject::Event(event)) => &mut event.waiters,
                Some(KernelObject::Mutex(mutex)) => &mut mutex.waiters,
                Some(KernelObject::Semaphore(semaphore)) => &mut semaphore.waiters,
                Some(KernelObject::Timer(timer)) => &mut timer.waiters,
                Some(KernelObject::Thread(thread)) => &mut thread.waiters,
                Some(KernelObject::MemoryBlock(_)) | Some(KernelObject::ServiceSession(_)) => {
                    return Err(KernelHandleError::WrongType);
                }
                None => return Err(KernelHandleError::InvalidHandle),
            };
            if waiters.contains(&thread) {
                false
            } else {
                waiters.push_back(thread);
                true
            }
        };
        if inserted {
            self.waiter_handles.insert((thread, object_key), handle);
        }
        Ok(())
    }

    /// Returns and clears the handle that most recently released `thread`.
    pub fn take_handoff(&mut self, thread: ThreadId) -> Option<u32> {
        self.handoffs.remove(&thread)
    }

    /// Removes one thread from every synchronization-object wait queue and
    /// releases its wait-all pins and alias-specific waiter metadata.
    ///
    /// This is used when a finite wait times out, so a later signal cannot
    /// wake a thread that has already returned RESULT_TIMEOUT.
    pub fn remove_waiter(&mut self, thread: ThreadId) -> bool {
        let mut removed = false;
        for object in self
            .objects
            .values_mut()
            .chain(self.retired_objects.values_mut())
        {
            let waiters = match object {
                KernelObject::Event(event) => &mut event.waiters,
                KernelObject::Mutex(mutex) => &mut mutex.waiters,
                KernelObject::Semaphore(semaphore) => &mut semaphore.waiters,
                KernelObject::Timer(timer) => &mut timer.waiters,
                KernelObject::Thread(thread) => &mut thread.waiters,
                KernelObject::MemoryBlock(_) | KernelObject::ServiceSession(_) => continue,
            };
            let previous_len = waiters.len();
            waiters.retain(|waiter| *waiter != thread);
            removed |= waiters.len() != previous_len;
        }
        let previous_handle_count = self.waiter_handles.len();
        self.waiter_handles
            .retain(|(waiter, _), _| *waiter != thread);
        removed |= self.waiter_handles.len() != previous_handle_count;
        let released_wait_all = self.wait_all_refs.remove(&thread).is_some();
        removed |= released_wait_all;
        self.collect_retired_objects();
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
        let mut handed_off: Vec<(ThreadId, u32)> = Vec::new();

        for (handle, object) in self
            .objects
            .iter_mut()
            .chain(self.retired_objects.iter_mut())
        {
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
                        handed_off.push((waiter, *handle));
                    } else {
                        timer.signaled = true;
                    }
                }
                EventResetType::Sticky => {
                    timer.signaled = true;
                    for waiter in timer.waiters.drain(..) {
                        awakened.push(waiter);
                        handed_off.push((waiter, *handle));
                    }
                }
            }
        }

        for (waiter, object_key) in handed_off {
            let handle = self
                .waiter_handles
                .remove(&(waiter, object_key))
                .unwrap_or(object_key);
            self.handoffs.insert(waiter, handle);
        }
        awakened
    }

    /// Sets an event signaled and returns the threads released by the signal.
    ///
    /// The session layer validates guest SVC handles before calling this
    /// method. HLE devices may also signal a retired event whose only
    /// remaining references belong to already-blocked guest threads.
    pub fn signal_event(&mut self, handle: u32) -> Result<Vec<ThreadId>, KernelHandleError> {
        let object_key = self
            .object_key(handle)
            .or_else(|| self.retired_objects.contains_key(&handle).then_some(handle))
            .ok_or(KernelHandleError::InvalidHandle)?;
        let event = match self.object_mut_by_key(object_key) {
            Some(KernelObject::Event(event)) => event,
            Some(_) => return Err(KernelHandleError::WrongType),
            None => return Err(KernelHandleError::InvalidHandle),
        };
        let released: Vec<ThreadId> = match event.reset_type {
            EventResetType::Sticky => {
                event.signaled = true;
                event.waiters.drain(..).collect()
            }
            EventResetType::OneShot => {
                if let Some(waiter) = event.waiters.pop_front() {
                    event.signaled = false;
                    vec![waiter]
                } else {
                    event.signaled = true;
                    Vec::new()
                }
            }
        };
        for waiter in &released {
            let waiter_handle = self
                .waiter_handles
                .remove(&(*waiter, object_key))
                .unwrap_or(handle);
            self.handoffs.insert(*waiter, waiter_handle);
        }
        Ok(released)
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
        let object_key = self
            .object_key(handle)
            .ok_or(KernelHandleError::InvalidHandle)?;
        let next_owner = {
            let mutex = self.mutex_mut(handle)?;
            if mutex.owner != Some(owner) {
                return Err(KernelHandleError::NotOwner);
            }
            let next_owner = mutex.waiters.pop_front();
            mutex.owner = next_owner;
            next_owner
        };
        if let Some(next) = next_owner {
            let waiter_handle = self
                .waiter_handles
                .remove(&(next, object_key))
                .unwrap_or(handle);
            self.handoffs.insert(next, waiter_handle);
        }
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
        let object_key = self
            .object_key(handle)
            .ok_or(KernelHandleError::InvalidHandle)?;
        let awakened = {
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
            awakened
        };
        for waiter in &awakened {
            let waiter_handle = self
                .waiter_handles
                .remove(&(*waiter, object_key))
                .unwrap_or(handle);
            self.handoffs.insert(*waiter, waiter_handle);
        }
        Ok(awakened)
    }

    /// Returns a mutex's current owner, if the handle refers to a mutex.
    pub fn mutex_owner(&self, handle: u32) -> Option<ThreadId> {
        match self.object(handle) {
            Some(KernelObject::Mutex(mutex)) => mutex.owner,
            _ => None,
        }
    }

    /// Returns the current and maximum count of a semaphore handle.
    pub fn semaphore_counts(&self, handle: u32) -> Option<(u32, u32)> {
        match self.object(handle) {
            Some(KernelObject::Semaphore(semaphore)) => Some((semaphore.count, semaphore.maximum)),
            _ => None,
        }
    }

    /// Closes a regular process-local handle.
    ///
    /// The guest-visible handle is removed immediately. The object remains
    /// alive while another duplicate handle, owner, waiter or wait-all pin
    /// references it; the final reference release reclaims the object.
    /// Pseudo-handles are not stored in the table and cannot be closed.
    pub fn close_handle(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        if self.handle_aliases.remove(&handle).is_some() {
            self.collect_retired_objects();
            return Ok(());
        }

        let object = self
            .objects
            .remove(&handle)
            .ok_or(KernelHandleError::InvalidHandle)?;
        let has_aliases = self.handle_aliases.values().any(|key| *key == handle);
        let keep_alive = has_aliases
            || object_has_internal_references(&object)
            || self.has_wait_all_reference(handle);
        if keep_alive {
            self.retired_objects.insert(handle, object);
        }
        self.collect_retired_objects();
        Ok(())
    }

    /// Returns the number of currently open regular handles.
    pub fn handle_count(&self) -> usize {
        self.objects.len() + self.handle_aliases.len()
    }

    /// Returns the highest simultaneous open-handle count observed by this table.
    pub fn handle_high_water_mark(&self) -> usize {
        self.high_water_handle_count
    }

    fn update_handle_high_water_mark(&mut self) {
        self.high_water_handle_count = self.high_water_handle_count.max(self.handle_count());
    }

    fn mutex_mut(&mut self, handle: u32) -> Result<&mut KernelMutex, KernelHandleError> {
        match self.object_mut(handle) {
            Some(KernelObject::Mutex(mutex)) => Ok(mutex),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn semaphore_mut(&mut self, handle: u32) -> Result<&mut KernelSemaphore, KernelHandleError> {
        match self.object_mut(handle) {
            Some(KernelObject::Semaphore(semaphore)) => Ok(semaphore),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn timer_mut(&mut self, handle: u32) -> Result<&mut KernelTimer, KernelHandleError> {
        match self.object_mut(handle) {
            Some(KernelObject::Timer(timer)) => Ok(timer),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn event_mut(&mut self, handle: u32) -> Result<&mut KernelEvent, KernelHandleError> {
        match self.object_mut(handle) {
            Some(KernelObject::Event(event)) => Ok(event),
            Some(_) => Err(KernelHandleError::WrongType),
            None => Err(KernelHandleError::InvalidHandle),
        }
    }

    fn object_key(&self, handle: u32) -> Option<u32> {
        if self.objects.contains_key(&handle) {
            Some(handle)
        } else {
            self.handle_aliases.get(&handle).copied()
        }
    }

    fn object_by_key(&self, object_key: u32) -> Option<&KernelObject> {
        self.objects
            .get(&object_key)
            .or_else(|| self.retired_objects.get(&object_key))
    }

    fn object_mut_by_key(&mut self, object_key: u32) -> Option<&mut KernelObject> {
        if self.objects.contains_key(&object_key) {
            self.objects.get_mut(&object_key)
        } else {
            self.retired_objects.get_mut(&object_key)
        }
    }

    fn object(&self, handle: u32) -> Option<&KernelObject> {
        let object_key = self.object_key(handle)?;
        self.object_by_key(object_key)
    }

    fn object_mut(&mut self, handle: u32) -> Option<&mut KernelObject> {
        let object_key = self.object_key(handle)?;
        self.object_mut_by_key(object_key)
    }

    fn allocate_handle(&mut self) -> Result<u32, KernelHandleError> {
        let mut handle = self.next_handle;
        while self.objects.contains_key(&handle)
            || self.retired_objects.contains_key(&handle)
            || self.handle_aliases.contains_key(&handle)
            || handle == CURRENT_THREAD_HANDLE
            || handle == CURRENT_PROCESS_HANDLE
        {
            handle = handle.checked_add(1).ok_or(KernelHandleError::Exhausted)?;
        }

        self.next_handle = handle.checked_add(1).ok_or(KernelHandleError::Exhausted)?;
        Ok(handle)
    }

    fn insert(&mut self, object: KernelObject) -> Result<u32, KernelHandleError> {
        let handle = self.allocate_handle()?;
        self.objects.insert(handle, object);
        self.update_handle_high_water_mark();
        Ok(handle)
    }
}

fn object_has_internal_references(object: &KernelObject) -> bool {
    match object {
        KernelObject::Thread(thread) => !thread.waiters.is_empty(),
        KernelObject::Event(event) => !event.waiters.is_empty(),
        // The current mutex owner is also an internal user of the object. A
        // closed mutex must survive a handoff until that owner exits/releases it.
        KernelObject::Mutex(mutex) => mutex.owner.is_some() || !mutex.waiters.is_empty(),
        KernelObject::Semaphore(semaphore) => !semaphore.waiters.is_empty(),
        KernelObject::Timer(timer) => !timer.waiters.is_empty(),
        KernelObject::MemoryBlock(_) | KernelObject::ServiceSession(_) => false,
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
    fn thread_handles_are_counted_until_closed() {
        let mut table = Arm11KernelObjectTable::default();
        let target = thread_id(2);
        let handle = table.insert_thread(target).unwrap();

        assert_eq!(table.thread_handle_target(handle), Some(target));
        assert_eq!(table.thread_handle_count(target), 1);
        table.close_handle(handle).unwrap();
        assert_eq!(table.thread_handle_count(target), 0);
    }

    #[test]
    fn retired_mutex_survives_owner_handoff_until_new_owner_exits() {
        let mut scheduler =
            super::super::thread::Arm11ThreadScheduler::new(crate::cpu::Arm11Core::new(), 0x30)
                .unwrap();
        let owner = scheduler.create_thread(0x1000, 0, 0x2000, 0x30, 0).unwrap();
        let waiter = scheduler.create_thread(0x2000, 0, 0x3000, 0x30, 0).unwrap();
        let mut table = Arm11KernelObjectTable::default();
        let mutex = table.insert_mutex(true, owner).unwrap();
        table.enqueue_waiter(mutex, waiter).unwrap();

        assert_eq!(table.close_handle(mutex), Ok(()));
        assert_eq!(table.retained_object_count(), 1);

        assert_eq!(table.release_owned_mutexes(owner), vec![waiter]);
        assert_eq!(table.retained_object_count(), 1);

        assert!(table.release_owned_mutexes(waiter).is_empty());
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn thread_exit_releases_owned_mutexes_and_deduplicates_wait_any_waiters() {
        let mut scheduler =
            super::super::thread::Arm11ThreadScheduler::new(crate::cpu::Arm11Core::new(), 0x30)
                .unwrap();
        let owner = scheduler.create_thread(0x1000, 0, 0x2000, 0x30, 0).unwrap();
        let first_waiter = scheduler.create_thread(0x2000, 0, 0x3000, 0x30, 0).unwrap();
        let second_waiter = scheduler.create_thread(0x3000, 0, 0x4000, 0x30, 0).unwrap();
        let mut table = Arm11KernelObjectTable::default();
        let first_mutex = table.insert_mutex(true, owner).unwrap();
        let second_mutex = table.insert_mutex(true, owner).unwrap();

        table.enqueue_waiter(first_mutex, first_waiter).unwrap();
        table.enqueue_waiter(second_mutex, first_waiter).unwrap();
        table.enqueue_waiter(second_mutex, second_waiter).unwrap();

        let awakened = table.release_owned_mutexes(owner);
        assert_eq!(awakened, vec![first_waiter, second_waiter]);
        assert_eq!(table.mutex_owner(first_mutex), Some(first_waiter));
        assert_eq!(table.mutex_owner(second_mutex), Some(second_waiter));
        assert_eq!(table.take_handoff(first_waiter), Some(first_mutex));
        assert_eq!(table.take_handoff(second_waiter), Some(second_mutex));
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
    fn handle_high_water_mark_tracks_peak_across_create_duplicate_and_close() {
        let mut table = Arm11KernelObjectTable::default();
        assert_eq!(table.handle_count(), 0);
        assert_eq!(table.handle_high_water_mark(), 0);

        let event = table.insert_event(EventResetType::Sticky).unwrap();
        assert_eq!(table.handle_count(), 1);
        assert_eq!(table.handle_high_water_mark(), 1);

        let duplicate = table.duplicate_handle(event).unwrap();
        assert_eq!(table.handle_count(), 2);
        assert_eq!(table.handle_high_water_mark(), 2);

        table.close_handle(duplicate).unwrap();
        assert_eq!(table.handle_count(), 1);
        assert_eq!(table.handle_high_water_mark(), 2);

        let semaphore = table.insert_semaphore(0, 2).unwrap();
        assert_ne!(semaphore, duplicate);
        assert_eq!(table.handle_count(), 2);
        assert_eq!(table.handle_high_water_mark(), 2);

        let timer = table.insert_timer(EventResetType::OneShot).unwrap();
        assert_eq!(table.handle_count(), 3);
        assert_eq!(table.handle_high_water_mark(), 3);

        table.close_handle(event).unwrap();
        table.close_handle(semaphore).unwrap();
        table.close_handle(timer).unwrap();
        assert_eq!(table.handle_count(), 0);
        assert_eq!(table.handle_high_water_mark(), 3);
    }

    #[test]
    fn handle_reference_count_includes_mutex_owner_until_release() {
        let owner = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let mutex = table.insert_mutex(true, owner).unwrap();

        assert_eq!(table.handle_reference_count(mutex), Ok(2));
        let duplicate = table.duplicate_handle(mutex).unwrap();
        assert_eq!(table.handle_reference_count(duplicate), Ok(3));

        table.close_handle(mutex).unwrap();
        assert_eq!(table.handle_reference_count(duplicate), Ok(2));
        assert_eq!(table.release_mutex(duplicate, owner), Ok(None));
        assert_eq!(table.handle_reference_count(duplicate), Ok(1));

        table.close_handle(duplicate).unwrap();
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn handle_reference_count_tracks_aliases_waiters_and_wait_all_pins() {
        let waiter = thread_id(1);
        let wait_all_thread = thread_id(2);
        let mut table = Arm11KernelObjectTable::default();
        let event = table.insert_event(EventResetType::Sticky).unwrap();
        let duplicate = table.duplicate_handle(event).unwrap();

        assert_eq!(table.handle_reference_count(event), Ok(2));
        table.enqueue_waiter(duplicate, waiter).unwrap();
        assert_eq!(table.handle_reference_count(event), Ok(3));

        table
            .retain_wait_all_handles(&[event, duplicate], wait_all_thread)
            .unwrap();
        assert_eq!(table.handle_reference_count(duplicate), Ok(5));

        table.close_handle(event).unwrap();
        assert_eq!(table.handle_reference_count(duplicate), Ok(4));
        assert!(table.remove_waiter(waiter));
        assert_eq!(table.handle_reference_count(duplicate), Ok(3));
        assert!(table.release_wait_all_handles(wait_all_thread));
        assert_eq!(table.handle_reference_count(duplicate), Ok(1));

        table.close_handle(duplicate).unwrap();
        assert_eq!(
            table.handle_reference_count(duplicate),
            Err(KernelHandleError::InvalidHandle)
        );
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn duplicate_handles_preserve_identity_for_all_current_object_types() {
        let owner = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();

        let mutex = table.insert_mutex(true, owner).unwrap();
        let mutex_alias = table.duplicate_handle(mutex).unwrap();
        assert_eq!(table.mutex_owner(mutex_alias), Some(owner));
        assert_eq!(table.release_mutex(mutex_alias, owner), Ok(None));
        assert_eq!(table.mutex_owner(mutex), None);
        assert_eq!(table.try_wait_synchronization(mutex, owner), Ok(true));
        assert_eq!(table.mutex_owner(mutex_alias), Some(owner));
        table.release_mutex(mutex_alias, owner).unwrap();
        table.close_handle(mutex).unwrap();
        table.close_handle(mutex_alias).unwrap();

        let semaphore = table.insert_semaphore(1, 2).unwrap();
        let semaphore_alias = table.duplicate_handle(semaphore).unwrap();
        assert_eq!(
            table.try_wait_synchronization(semaphore_alias, owner),
            Ok(true)
        );
        assert_eq!(table.semaphore_counts(semaphore), Some((0, 2)));
        assert_eq!(table.release_semaphore(semaphore, 1), Ok(Vec::new()));
        assert_eq!(table.semaphore_counts(semaphore_alias), Some((1, 2)));
        table.close_handle(semaphore).unwrap();
        table.close_handle(semaphore_alias).unwrap();

        let timer = table.insert_timer(EventResetType::OneShot).unwrap();
        let timer_alias = table.duplicate_handle(timer).unwrap();
        table.set_timer(timer_alias, 0, 10, 0).unwrap();
        assert!(table.advance_time(10).is_empty());
        assert_eq!(table.timer_is_signaled(timer), Some(true));
        assert_eq!(table.try_wait_synchronization(timer, owner), Ok(true));
        assert_eq!(table.timer_is_signaled(timer_alias), Some(false));
        table.close_handle(timer).unwrap();
        table.close_handle(timer_alias).unwrap();

        let memory = table.create_memory_block(0x1000, 3, 1).unwrap();
        let memory_alias = table.duplicate_handle(memory).unwrap();
        let primary_backing = table.memory_block_backing(memory).unwrap();
        let alias_backing = table.memory_block_backing(memory_alias).unwrap();
        assert!(primary_backing.shares_backing_with(&alias_backing));
        assert_eq!(table.memory_block_permissions(memory_alias), Some((3, 1)));
        table.close_handle(memory).unwrap();
        assert!(table.memory_block_backing(memory_alias).is_ok());
        table.close_handle(memory_alias).unwrap();

        let service = table.insert_service_manager_session().unwrap();
        let service_alias = table.duplicate_handle(service).unwrap();
        table
            .register_service_manager_client(service_alias)
            .unwrap();
        assert_eq!(table.service_manager_client_registered(service), Some(true));
        table.close_handle(service).unwrap();
        assert_eq!(
            table.service_manager_client_registered(service_alias),
            Some(true)
        );
        table.close_handle(service_alias).unwrap();

        let gsp = table.insert_gsp_gpu_session().unwrap();
        let gsp_alias = table.duplicate_handle(gsp).unwrap();
        table
            .set_gsp_session_registration(gsp_alias, Some(7), Some(0x200), Some(0x300), 5)
            .unwrap();
        assert_eq!(
            table.service_session(gsp),
            Some(ServiceSessionKind::GspGpu {
                client_id: Some(7),
                shared_memory_handle: Some(0x200),
                event_handle: Some(0x300),
                flags: 5,
            })
        );
        table.close_handle(gsp).unwrap();
        assert!(table.service_session(gsp_alias).is_some());
        table.close_handle(gsp_alias).unwrap();

        assert_eq!(table.handle_count(), 0);
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn duplicate_handles_share_event_identity_and_last_close_reclaims_object() {
        let mut table = Arm11KernelObjectTable::default();
        let original = table.insert_event(EventResetType::Sticky).unwrap();
        let duplicate = table.duplicate_handle(original).unwrap();

        assert_ne!(original, duplicate);
        assert_eq!(table.handle_count(), 2);
        assert_eq!(table.retained_object_count(), 1);
        assert!(table.is_open_handle(duplicate));
        assert_eq!(table.signal_event(duplicate), Ok(Vec::new()));
        assert_eq!(table.event_is_signaled(original), Some(true));

        assert_eq!(table.close_handle(original), Ok(()));
        assert!(!table.is_open_handle(original));
        assert!(table.is_open_handle(duplicate));
        assert_eq!(table.event_is_signaled(duplicate), Some(true));
        assert_eq!(table.retained_object_count(), 1);

        assert_eq!(table.clear_event(duplicate), Ok(()));
        assert_eq!(table.event_is_signaled(duplicate), Some(false));
        assert_eq!(table.close_handle(duplicate), Ok(()));
        assert_eq!(table.handle_count(), 0);
        assert_eq!(table.retained_object_count(), 0);
        assert_eq!(
            table.duplicate_handle(duplicate),
            Err(KernelHandleError::InvalidHandle)
        );
    }

    #[test]
    fn waiter_using_duplicate_handle_receives_that_handle_on_handoff() {
        let waiter = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let original = table.insert_event(EventResetType::OneShot).unwrap();
        let duplicate = table.duplicate_handle(original).unwrap();
        table.enqueue_waiter(duplicate, waiter).unwrap();

        assert_eq!(table.close_handle(original), Ok(()));
        assert_eq!(table.retained_object_count(), 1);
        assert_eq!(table.signal_event(duplicate), Ok(vec![waiter]));
        assert_eq!(table.take_handoff(waiter), Some(duplicate));
        assert!(!table.remove_waiter(waiter));
        assert_eq!(table.close_handle(duplicate), Ok(()));
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn wait_all_pins_every_alias_even_when_all_handles_are_closed() {
        let waiter = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let original = table.insert_event(EventResetType::Sticky).unwrap();
        let duplicate = table.duplicate_handle(original).unwrap();
        let handles = [original, duplicate];

        assert_eq!(
            table.try_wait_synchronization_all(&handles, waiter),
            Ok(false)
        );
        table.retain_wait_all_handles(&handles, waiter).unwrap();
        assert_eq!(table.close_handle(original), Ok(()));
        assert_eq!(table.close_handle(duplicate), Ok(()));
        assert_eq!(table.handle_count(), 0);
        assert_eq!(table.retained_object_count(), 1);

        assert_eq!(table.signal_event(original), Ok(Vec::new()));
        assert_eq!(
            table.try_wait_synchronization_all(&handles, waiter),
            Ok(true)
        );
        assert!(table.release_wait_all_handles(waiter));
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn duplicate_thread_handles_are_counted_until_every_alias_closes() {
        let mut table = Arm11KernelObjectTable::default();
        let target = thread_id(2);
        let original = table.insert_thread(target).unwrap();
        let duplicate = table.duplicate_handle(original).unwrap();

        assert_eq!(table.thread_handle_count(target), 2);
        table.close_handle(original).unwrap();
        assert_eq!(table.thread_handle_count(target), 1);
        assert_eq!(table.thread_handle_target(duplicate), Some(target));
        table.close_handle(duplicate).unwrap();
        assert_eq!(table.thread_handle_count(target), 0);
    }

    #[test]
    fn close_handle_retires_event_until_waiter_reference_is_released() {
        let waiter = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let event = table.insert_event(EventResetType::OneShot).unwrap();
        table.enqueue_waiter(event, waiter).unwrap();

        assert_eq!(table.close_handle(event), Ok(()));
        assert!(!table.is_open_handle(event));
        assert_eq!(table.handle_count(), 0);
        assert_eq!(table.retained_object_count(), 1);

        assert_eq!(table.signal_event(event).unwrap(), vec![waiter]);
        assert!(!table.remove_waiter(waiter));
        assert_eq!(table.retained_object_count(), 0);
    }

    #[test]
    fn wait_all_reference_keeps_closed_handle_alive_until_atomic_acquire() {
        let waiter = thread_id(1);
        let mut table = Arm11KernelObjectTable::default();
        let event = table.insert_event(EventResetType::Sticky).unwrap();

        assert_eq!(
            table.try_wait_synchronization_all(&[event], waiter),
            Ok(false)
        );
        table.retain_wait_all_handles(&[event], waiter).unwrap();
        assert_eq!(table.close_handle(event), Ok(()));
        assert!(!table.is_open_handle(event));
        assert_eq!(table.retained_object_count(), 1);

        assert!(table.signal_event(event).unwrap().is_empty());
        assert_eq!(
            table.try_wait_synchronization_all(&[event], waiter),
            Ok(true)
        );
        assert!(table.release_wait_all_handles(waiter));
        assert_eq!(table.retained_object_count(), 0);
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
        assert_eq!(table.close_handle(one_shot), Ok(()));
        assert!(!table.is_open_handle(one_shot));
        assert_eq!(table.retained_object_count(), 1);
        assert_eq!(table.signal_event(one_shot).unwrap(), vec![second]);
        assert!(!table.remove_waiter(second));
        assert_eq!(table.retained_object_count(), 0);

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
        assert_eq!(table.close_handle(timer), Ok(()));
        assert!(!table.is_open_handle(timer));
        assert_eq!(table.retained_object_count(), 1);
        assert!(table.remove_waiter(second));
        assert_eq!(table.retained_object_count(), 0);
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
