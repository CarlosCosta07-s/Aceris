//! ARM11 application-thread contexts and cooperative scheduling primitives.
//!
//! The scheduler keeps a separate CPU register context for every thread and
//! models the 3DS priority ordering. It is intentionally not wired to SVC
//! CreateThread yet: the process address space still needs per-thread stack
//! reservation and validation before guest code may run on a new context.

use std::collections::BTreeMap;
use std::fmt;

use crate::cpu::Arm11Core;

/// Highest schedulable ARM11 thread priority.
pub const HIGHEST_THREAD_PRIORITY: u8 = 0;

/// Lowest schedulable ARM11 thread priority.
pub const LOWEST_THREAD_PRIORITY: u8 = 0x3F;

/// Processor ID used by the current single-application-core implementation.
pub const APPLICATION_PROCESSOR_ID: i32 = 0;

/// Processor ID meaning the process default core.
pub const DEFAULT_PROCESSOR_ID: i32 = -2;

/// Internal identifier used by the bootstrap thread.
pub const MAIN_THREAD_ID: ThreadId = ThreadId(1);

/// Internal thread identifier. It is not yet a guest-visible kernel handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(u32);

impl ThreadId {
    /// Returns the numeric value of this internal thread identifier.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Current lifecycle state of an application thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    /// Created but not yet started by the guest.
    Created,
    /// Eligible to be selected by the scheduler.
    Ready,
    /// Currently owns the emulated application CPU.
    Running,
    /// Waiting for a virtual-clock deadline.
    Sleeping { wake_deadline_ns: u64 },
    /// Blocked until a future synchronization object wakes it.
    Waiting,
    /// Thread has permanently exited.
    Exited,
}

/// Public, copyable view of a managed thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadInfo {
    pub id: ThreadId,
    pub entrypoint: u32,
    pub argument: u32,
    pub stack_top: u32,
    pub priority: u8,
    pub processor_id: i32,
    pub state: ThreadState,
}

/// Failures produced by thread creation and scheduling operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadSchedulerError {
    InvalidPriority(i32),
    UnsupportedProcessorId(i32),
    UnalignedArmEntrypoint(u32),
    InvalidStackPointer(u32),
    ThreadIdExhausted,
    UnknownThread(ThreadId),
    InvalidState {
        thread_id: ThreadId,
        state: ThreadState,
        operation: &'static str,
    },
    NoCurrentThread,
    InvalidSleepDuration(i64),
}

impl fmt::Display for ThreadSchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPriority(priority) => write!(
                formatter,
                "thread priority {priority} is outside the supported range 0..=0x3F"
            ),
            Self::UnsupportedProcessorId(processor_id) => write!(
                formatter,
                "processor ID {processor_id} is unavailable in the single-core scheduler"
            ),
            Self::UnalignedArmEntrypoint(address) => write!(
                formatter,
                "ARM thread entrypoint {address:#010x} is not word-aligned"
            ),
            Self::InvalidStackPointer(address) => write!(
                formatter,
                "thread stack top {address:#010x} must be nonzero and 8-byte aligned"
            ),
            Self::ThreadIdExhausted => formatter.write_str("thread identifier space exhausted"),
            Self::UnknownThread(thread_id) => {
                write!(formatter, "unknown thread identifier {}", thread_id.raw())
            }
            Self::InvalidState {
                thread_id,
                state,
                operation,
            } => write!(
                formatter,
                "cannot {operation} thread {} while it is {state:?}",
                thread_id.raw()
            ),
            Self::NoCurrentThread => formatter.write_str("there is no currently running thread"),
            Self::InvalidSleepDuration(duration) => {
                write!(formatter, "unsupported thread sleep duration {duration}")
            }
        }
    }
}

impl std::error::Error for ThreadSchedulerError {}

#[derive(Debug)]
struct ThreadControlBlock {
    info: ThreadInfo,
    core: Arm11Core,
    ready_order: Option<u64>,
}

/// Single-application-core, priority-based cooperative thread scheduler.
///
/// Lower numeric priorities run first. Threads at the same priority are
/// selected in FIFO ready order. Time is virtual and advances only when the
/// caller invokes [\`advance_time\`](Self::advance_time).
#[derive(Debug)]
pub struct Arm11ThreadScheduler {
    threads: BTreeMap<ThreadId, ThreadControlBlock>,
    current: Option<ThreadId>,
    next_thread_id: u32,
    next_ready_order: u64,
    virtual_time_ns: u64,
}

impl Arm11ThreadScheduler {
    /// Creates a scheduler with the already-running main thread.
    pub fn new(
        main_core: Arm11Core,
        main_priority: i32,
    ) -> std::result::Result<Self, ThreadSchedulerError> {
        let priority = validate_priority(main_priority)?;
        let main_info = ThreadInfo {
            id: MAIN_THREAD_ID,
            entrypoint: main_core.pc(),
            argument: main_core.regs[0],
            stack_top: main_core.regs[13],
            priority,
            processor_id: APPLICATION_PROCESSOR_ID,
            state: ThreadState::Running,
        };
        let main_thread = ThreadControlBlock {
            info: main_info,
            core: main_core,
            ready_order: None,
        };
        let mut threads = BTreeMap::new();
        threads.insert(MAIN_THREAD_ID, main_thread);

        Ok(Self {
            threads,
            current: Some(MAIN_THREAD_ID),
            next_thread_id: MAIN_THREAD_ID.raw() + 1,
            next_ready_order: 0,
            virtual_time_ns: 0,
        })
    }

    /// Creates a separate ARM/Thumb CPU context in the Created state.
    ///
    /// Processor ID 0 selects the currently modelled application core.
    /// Processor ID -2 resolves to the process default (currently core 0).
    /// Other processor IDs are rejected rather than pretending that multiple
    /// application cores are already emulated.
    ///
    /// The caller must validate that the entrypoint is executable and that the
    /// supplied stack range is mapped writable before starting this thread.
    pub fn create_thread(
        &mut self,
        entrypoint: u32,
        argument: u32,
        stack_top: u32,
        priority: i32,
        processor_id: i32,
    ) -> std::result::Result<ThreadId, ThreadSchedulerError> {
        let priority = validate_priority(priority)?;
        let resolved_processor_id = match processor_id {
            APPLICATION_PROCESSOR_ID | DEFAULT_PROCESSOR_ID => APPLICATION_PROCESSOR_ID,
            other => return Err(ThreadSchedulerError::UnsupportedProcessorId(other)),
        };

        if stack_top == 0 || stack_top & 7 != 0 {
            return Err(ThreadSchedulerError::InvalidStackPointer(stack_top));
        }

        let thumb = entrypoint & 1 != 0;
        let instruction_address = entrypoint & !1;
        if !thumb && instruction_address & 3 != 0 {
            return Err(ThreadSchedulerError::UnalignedArmEntrypoint(
                instruction_address,
            ));
        }

        let id = ThreadId(self.next_thread_id);
        self.next_thread_id = self
            .next_thread_id
            .checked_add(1)
            .ok_or(ThreadSchedulerError::ThreadIdExhausted)?;

        let mut core = Arm11Core::new();
        core.set_pc(instruction_address);
        core.regs[0] = argument;
        core.regs[13] = stack_top;
        core.cpsr.set_thumb(thumb);

        let info = ThreadInfo {
            id,
            entrypoint: instruction_address,
            argument,
            stack_top,
            priority,
            processor_id: resolved_processor_id,
            state: ThreadState::Created,
        };
        self.threads.insert(
            id,
            ThreadControlBlock {
                info,
                core,
                ready_order: None,
            },
        );
        Ok(id)
    }

    /// Marks a newly-created thread Ready and schedules it if it outranks the
    /// current thread.
    pub fn start_thread(
        &mut self,
        id: ThreadId,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let state = self
            .threads
            .get(&id)
            .ok_or(ThreadSchedulerError::UnknownThread(id))?
            .info
            .state;
        if state != ThreadState::Created {
            return Err(ThreadSchedulerError::InvalidState {
                thread_id: id,
                state,
                operation: "start",
            });
        }

        let order = self.allocate_ready_order();
        let thread = self.threads.get_mut(&id).expect("thread was checked above");
        thread.info.state = ThreadState::Ready;
        thread.ready_order = Some(order);
        self.reschedule_if_needed();
        Ok(self.current)
    }

    /// Returns a thread's current priority.
    pub fn priority(&self, id: ThreadId) -> std::result::Result<u8, ThreadSchedulerError> {
        self.threads
            .get(&id)
            .map(|thread| thread.info.priority)
            .ok_or(ThreadSchedulerError::UnknownThread(id))
    }

    /// Changes a thread's priority and immediately checks for priority preemption.
    pub fn set_priority(
        &mut self,
        id: ThreadId,
        priority: i32,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let priority = validate_priority(priority)?;
        let thread = self
            .threads
            .get_mut(&id)
            .ok_or(ThreadSchedulerError::UnknownThread(id))?;
        if thread.info.state == ThreadState::Exited {
            return Err(ThreadSchedulerError::InvalidState {
                thread_id: id,
                state: thread.info.state,
                operation: "change priority for",
            });
        }
        thread.info.priority = priority;
        self.reschedule_if_needed();
        Ok(self.current)
    }

    /// Yields the current thread and chooses the highest-priority Ready thread.
    ///
    /// Equal-priority threads rotate in FIFO order. If no other thread is
    /// ready, the yielding thread is selected again.
    pub fn yield_current(&mut self) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let id = self.current.ok_or(ThreadSchedulerError::NoCurrentThread)?;
        let order = self.allocate_ready_order();
        let current = self
            .threads
            .get_mut(&id)
            .expect("current thread must exist");
        current.info.state = ThreadState::Ready;
        current.ready_order = Some(order);
        self.current = None;
        self.schedule_next();
        Ok(self.current)
    }

    /// Exits the current thread and selects another Ready thread, if one exists.
    pub fn exit_current(&mut self) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let id = self.current.ok_or(ThreadSchedulerError::NoCurrentThread)?;
        let current = self
            .threads
            .get_mut(&id)
            .expect("current thread must exist");
        current.info.state = ThreadState::Exited;
        current.ready_order = None;
        self.current = None;
        self.schedule_next();
        Ok(self.current)
    }

    /// Blocks the current thread until it is explicitly woken.
    pub fn wait_current(&mut self) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let id = self.current.ok_or(ThreadSchedulerError::NoCurrentThread)?;
        let current = self
            .threads
            .get_mut(&id)
            .expect("current thread must exist");
        current.info.state = ThreadState::Waiting;
        current.ready_order = None;
        self.current = None;
        self.schedule_next();
        Ok(self.current)
    }

    /// Wakes a thread that is in the Waiting state.
    pub fn wake_thread(
        &mut self,
        id: ThreadId,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        let state = self
            .threads
            .get(&id)
            .ok_or(ThreadSchedulerError::UnknownThread(id))?
            .info
            .state;
        if state != ThreadState::Waiting {
            return Err(ThreadSchedulerError::InvalidState {
                thread_id: id,
                state,
                operation: "wake",
            });
        }

        let order = self.allocate_ready_order();
        let thread = self.threads.get_mut(&id).expect("thread was checked above");
        thread.info.state = ThreadState::Ready;
        thread.ready_order = Some(order);
        self.reschedule_if_needed();
        Ok(self.current)
    }

    /// Sleeps the current thread for a virtual duration in nanoseconds.
    ///
    /// Values 0, -1 and -2 are the documented yield forms. On this single-core
    /// scheduler they all yield locally; migration semantics are not modelled.
    pub fn sleep_current(
        &mut self,
        nanoseconds: i64,
    ) -> std::result::Result<Option<ThreadId>, ThreadSchedulerError> {
        match nanoseconds {
            -2..=0 => self.yield_current(),
            value if value > 0 => {
                let id = self.current.ok_or(ThreadSchedulerError::NoCurrentThread)?;
                let deadline = self.virtual_time_ns.saturating_add(value as u64);
                let current = self
                    .threads
                    .get_mut(&id)
                    .expect("current thread must exist");
                current.info.state = ThreadState::Sleeping {
                    wake_deadline_ns: deadline,
                };
                current.ready_order = None;
                self.current = None;
                self.schedule_next();
                Ok(self.current)
            }
            value => Err(ThreadSchedulerError::InvalidSleepDuration(value)),
        }
    }

    /// Advances virtual time and wakes every thread whose sleep deadline passed.
    pub fn advance_time(&mut self, nanoseconds: u64) -> Option<ThreadId> {
        self.virtual_time_ns = self.virtual_time_ns.saturating_add(nanoseconds);
        let expired = self
            .threads
            .iter()
            .filter_map(|(id, thread)| match thread.info.state {
                ThreadState::Sleeping { wake_deadline_ns }
                    if wake_deadline_ns <= self.virtual_time_ns =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        for id in expired {
            let order = self.allocate_ready_order();
            let thread = self
                .threads
                .get_mut(&id)
                .expect("expired thread must exist");
            thread.info.state = ThreadState::Ready;
            thread.ready_order = Some(order);
        }

        self.reschedule_if_needed();
        self.current
    }

    /// Returns immutable information about one thread.
    pub fn thread_info(&self, id: ThreadId) -> Option<ThreadInfo> {
        self.threads.get(&id).map(|thread| thread.info)
    }

    /// Returns the currently selected thread identifier.
    pub const fn current_thread_id(&self) -> Option<ThreadId> {
        self.current
    }

    /// Returns the virtual scheduler clock in nanoseconds.
    pub const fn virtual_time_ns(&self) -> u64 {
        self.virtual_time_ns
    }

    /// Returns a thread's saved CPU context.
    pub fn core(&self, id: ThreadId) -> Option<&Arm11Core> {
        self.threads.get(&id).map(|thread| &thread.core)
    }

    /// Returns the selected thread's CPU context, if a thread is running.
    pub fn current_core(&self) -> Option<&Arm11Core> {
        self.current.and_then(|id| self.core(id))
    }

    /// Returns the selected thread's mutable CPU context.
    pub fn current_core_mut(&mut self) -> Option<&mut Arm11Core> {
        let id = self.current?;
        self.threads.get_mut(&id).map(|thread| &mut thread.core)
    }

    /// Returns the number of known threads, including exited threads.
    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    fn allocate_ready_order(&mut self) -> u64 {
        let order = self.next_ready_order;
        self.next_ready_order = self.next_ready_order.saturating_add(1);
        order
    }

    fn best_ready_thread(&self) -> Option<ThreadId> {
        self.threads
            .iter()
            .filter(|(_, thread)| thread.info.state == ThreadState::Ready)
            .min_by_key(|(_, thread)| {
                (thread.info.priority, thread.ready_order.unwrap_or(u64::MAX))
            })
            .map(|(id, _)| *id)
    }

    fn schedule_next(&mut self) {
        if self.current.is_some() {
            return;
        }
        if let Some(id) = self.best_ready_thread() {
            let thread = self.threads.get_mut(&id).expect("ready thread must exist");
            thread.info.state = ThreadState::Running;
            thread.ready_order = None;
            self.current = Some(id);
        }
    }

    fn reschedule_if_needed(&mut self) {
        let Some(current_id) = self.current else {
            self.schedule_next();
            return;
        };
        let current_priority = self
            .threads
            .get(&current_id)
            .expect("current thread must exist")
            .info
            .priority;
        let Some(ready_id) = self.best_ready_thread() else {
            return;
        };
        let ready_priority = self
            .threads
            .get(&ready_id)
            .expect("ready thread must exist")
            .info
            .priority;

        if ready_priority < current_priority {
            let order = self.allocate_ready_order();
            let current = self
                .threads
                .get_mut(&current_id)
                .expect("current thread must exist");
            current.info.state = ThreadState::Ready;
            current.ready_order = Some(order);
            self.current = None;
            self.schedule_next();
        }
    }
}

fn validate_priority(priority: i32) -> std::result::Result<u8, ThreadSchedulerError> {
    if !(i32::from(HIGHEST_THREAD_PRIORITY)..=i32::from(LOWEST_THREAD_PRIORITY)).contains(&priority)
    {
        return Err(ThreadSchedulerError::InvalidPriority(priority));
    }
    Ok(priority as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_core() -> Arm11Core {
        let mut core = Arm11Core::new();
        core.set_pc(0x0010_0000);
        core.regs[13] = 0x1000_0000;
        core
    }

    fn scheduler_with_priority(priority: i32) -> Arm11ThreadScheduler {
        Arm11ThreadScheduler::new(main_core(), priority).unwrap()
    }

    fn scheduler() -> Arm11ThreadScheduler {
        scheduler_with_priority(0x30)
    }

    fn create(scheduler: &mut Arm11ThreadScheduler, entrypoint: u32, priority: i32) -> ThreadId {
        scheduler
            .create_thread(entrypoint, 0x1234, 0x2000_1000, priority, 0)
            .unwrap()
    }

    #[test]
    fn main_thread_starts_running_with_its_own_context() {
        let scheduler = scheduler();
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(scheduler.current_core().unwrap().pc(), 0x0010_0000);
        assert_eq!(
            scheduler.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Running
        );
    }

    #[test]
    fn creates_an_independent_arm_thread_context() {
        let mut scheduler = scheduler();
        let id = create(&mut scheduler, 0x0020_0000, 0x20);
        let core = scheduler.core(id).unwrap();

        assert_eq!(core.pc(), 0x0020_0000);
        assert_eq!(core.regs[0], 0x1234);
        assert_eq!(core.regs[13], 0x2000_1000);
        assert!(!core.cpsr.thumb());
        assert_eq!(
            scheduler.thread_info(id).unwrap().state,
            ThreadState::Created
        );
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
    }

    #[test]
    fn initializes_thumb_thread_context_from_entrypoint_bit_zero() {
        let mut scheduler = scheduler();
        let id = create(&mut scheduler, 0x0020_0001, 0x20);
        let core = scheduler.core(id).unwrap();

        assert_eq!(core.pc(), 0x0020_0000);
        assert!(core.cpsr.thumb());
    }

    #[test]
    fn default_processor_id_resolves_to_the_application_core() {
        let mut scheduler = scheduler();
        let id = scheduler
            .create_thread(0x0020_0000, 0, 0x2000_1000, 0x20, DEFAULT_PROCESSOR_ID)
            .unwrap();
        assert_eq!(
            scheduler.thread_info(id).unwrap().processor_id,
            APPLICATION_PROCESSOR_ID
        );
    }

    #[test]
    fn rejects_unavailable_processor_ids() {
        let mut scheduler = scheduler();
        assert_eq!(
            scheduler.create_thread(0x0020_0000, 0, 0x2000_1000, 0x20, 1),
            Err(ThreadSchedulerError::UnsupportedProcessorId(1))
        );
    }

    #[test]
    fn rejects_invalid_priority_and_stack_alignment() {
        let mut scheduler = scheduler();
        assert_eq!(
            scheduler.create_thread(0x0020_0000, 0, 0x2000_1000, 0x40, 0),
            Err(ThreadSchedulerError::InvalidPriority(0x40))
        );
        assert_eq!(
            scheduler.create_thread(0x0020_0000, 0, 0x2000_1004, 0x20, 0),
            Err(ThreadSchedulerError::InvalidStackPointer(0x2000_1004))
        );
    }

    #[test]
    fn rejects_unaligned_arm_entrypoints_but_accepts_thumb_entrypoints() {
        let mut scheduler = scheduler();
        assert_eq!(
            scheduler.create_thread(0x0020_0002, 0, 0x2000_1000, 0x20, 0),
            Err(ThreadSchedulerError::UnalignedArmEntrypoint(0x0020_0002))
        );
        assert!(
            scheduler
                .create_thread(0x0020_0003, 0, 0x2000_1000, 0x20, 0)
                .is_ok()
        );
    }

    #[test]
    fn lower_numeric_priority_preempts_the_current_thread() {
        let mut scheduler = scheduler();
        let normal = create(&mut scheduler, 0x0020_0000, 0x30);
        let urgent = create(&mut scheduler, 0x0030_0000, 0x10);

        scheduler.start_thread(normal).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
        scheduler.start_thread(urgent).unwrap();

        assert_eq!(scheduler.current_thread_id(), Some(urgent));
        assert_eq!(
            scheduler.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Ready
        );
    }

    #[test]
    fn equal_priority_threads_are_selected_in_fifo_ready_order() {
        let mut scheduler = scheduler_with_priority(0x20);
        let first = create(&mut scheduler, 0x0020_0000, 0x20);
        let second = create(&mut scheduler, 0x0030_0000, 0x20);

        scheduler.start_thread(first).unwrap();
        scheduler.start_thread(second).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));

        scheduler.yield_current().unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(first));
        scheduler.yield_current().unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(second));
        scheduler.yield_current().unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
    }

    #[test]
    fn changing_a_ready_threads_priority_can_preempt() {
        let mut scheduler = scheduler();
        let other = create(&mut scheduler, 0x0020_0000, 0x30);
        scheduler.start_thread(other).unwrap();

        scheduler.set_priority(other, 0x10).unwrap();

        assert_eq!(scheduler.current_thread_id(), Some(other));
        assert_eq!(scheduler.priority(other).unwrap(), 0x10);
    }

    #[test]
    fn exited_threads_cannot_be_restarted() {
        let mut scheduler = scheduler();
        let other = create(&mut scheduler, 0x0020_0000, 0x20);
        scheduler.start_thread(other).unwrap();
        scheduler.exit_current().unwrap();

        assert_eq!(
            scheduler.thread_info(other).unwrap().state,
            ThreadState::Exited
        );
        assert_eq!(
            scheduler.start_thread(other),
            Err(ThreadSchedulerError::InvalidState {
                thread_id: other,
                state: ThreadState::Exited,
                operation: "start",
            })
        );
    }

    #[test]
    fn sleeping_threads_wake_only_after_the_virtual_deadline() {
        let mut scheduler = scheduler_with_priority(0x10);
        let other = create(&mut scheduler, 0x0020_0000, 0x20);
        scheduler.start_thread(other).unwrap();

        scheduler.sleep_current(100).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(other));
        assert!(matches!(
            scheduler.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Sleeping {
                wake_deadline_ns: 100
            }
        ));

        scheduler.advance_time(99);
        assert_eq!(scheduler.current_thread_id(), Some(other));
        scheduler.advance_time(1);
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
        assert_eq!(
            scheduler.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Running
        );
    }

    #[test]
    fn zero_and_documented_negative_sleep_values_yield() {
        let mut scheduler = scheduler();
        let other = create(&mut scheduler, 0x0020_0000, 0x30);
        scheduler.start_thread(other).unwrap();

        scheduler.sleep_current(0).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(other));
        scheduler.sleep_current(-1).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
        scheduler.sleep_current(-2).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(other));
    }

    #[test]
    fn waiting_threads_resume_only_when_woken() {
        let mut scheduler = scheduler_with_priority(0x10);
        let other = create(&mut scheduler, 0x0020_0000, 0x20);
        scheduler.start_thread(other).unwrap();

        scheduler.wait_current().unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(other));
        assert_eq!(
            scheduler.thread_info(MAIN_THREAD_ID).unwrap().state,
            ThreadState::Waiting
        );

        scheduler.wake_thread(MAIN_THREAD_ID).unwrap();
        assert_eq!(scheduler.current_thread_id(), Some(MAIN_THREAD_ID));
    }

    #[test]
    fn unsupported_sleep_values_are_rejected() {
        let mut scheduler = scheduler();
        assert_eq!(
            scheduler.sleep_current(-3),
            Err(ThreadSchedulerError::InvalidSleepDuration(-3))
        );
    }
}
