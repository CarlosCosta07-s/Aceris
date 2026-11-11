//! Process-local ARM11 kernel handles and synchronization objects.
//!
//! This first synchronization slice models event object lifetime and signal
//! state. Blocking waits, waiter queues, timers and reference-counted kernel
//! objects are deliberately not represented as implemented.

use std::collections::BTreeMap;

use super::thread::ThreadId;

/// Pseudo-handle that always identifies the calling thread.
pub const CURRENT_THREAD_HANDLE: u32 = 0xFFFF_8000;

const FIRST_HANDLE: u32 = 0x100;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct KernelEvent {
    reset_type: EventResetType,
    signaled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KernelObject {
    Thread(ThreadId),
    Event(KernelEvent),
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
    /// Inserts a newly created thread and returns its guest-visible handle.
    pub fn insert_thread(
        &mut self,
        thread_id: ThreadId,
    ) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Thread(thread_id))
    }

    /// Creates an unsignaled event and returns its guest-visible handle.
    pub fn insert_event(
        &mut self,
        reset_type: EventResetType,
    ) -> Result<u32, KernelHandleError> {
        self.insert(KernelObject::Event(KernelEvent {
            reset_type,
            signaled: false,
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

    /// Sets an event's signal state.
    pub fn signal_event(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        let event = self.event_mut(handle)?;
        event.signaled = true;
        Ok(())
    }

    /// Clears an event's signal state.
    pub fn clear_event(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        let event = self.event_mut(handle)?;
        event.signaled = false;
        Ok(())
    }

    /// Closes a regular process-local handle.
    ///
    /// Pseudo-handles are not stored in the table and therefore cannot be closed.
    pub fn close_handle(&mut self, handle: u32) -> Result<(), KernelHandleError> {
        self.objects
            .remove(&handle)
            .map(|_| ())
            .ok_or(KernelHandleError::InvalidHandle)
    }

    /// Returns the number of currently open regular handles.
    pub fn handle_count(&self) -> usize {
        self.objects.len()
    }

    fn event_mut(&mut self, handle: u32) -> Result<&mut KernelEvent, KernelHandleError> {
        match self.objects.get_mut(&handle) {
            Some(KernelObject::Event(event)) => Ok(event),
            Some(KernelObject::Thread(_)) => Err(KernelHandleError::WrongType),
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
        let mut scheduler = super::super::thread::Arm11ThreadScheduler::new(
            crate::cpu::Arm11Core::new(),
            0x30,
        )
        .unwrap();
        if value == 1 {
            super::super::thread::MAIN_THREAD_ID
        } else {
            scheduler
                .create_thread(0x1000, 0, 0x2000, 0x30, 0)
                .unwrap()
        }
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
    fn closed_handles_are_not_reused() {
        let mut table = Arm11KernelObjectTable::default();
        let first = table.insert_event(EventResetType::Sticky).unwrap();
        table.close_handle(first).unwrap();
        let second = table.insert_event(EventResetType::Sticky).unwrap();

        assert_ne!(first, second);
    }
}
