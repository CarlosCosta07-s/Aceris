//! Process-side GSP client registration and interrupt delivery.
//!
//! The registry is owned by the service layer, not by the guest process. It
//! connects a registered client slot to a process-mapped GSP shared-memory
//! block and a process-local kernel event handle. Shared-memory creation,
//! mapping and IPC command decoding remain responsibilities of later layers.

use std::fmt;

use super::Arm11ProcessSession;
use super::kernel::KernelHandleError;
use crate::gpu::{
    GSP_INTERRUPT_QUEUE_CLIENT_COUNT, GSP_INTERRUPT_QUEUE_SIZE, GspInterruptId,
    GspInterruptQueueError, GspInterruptQueuePushResult, GspInterruptRelayQueue,
};

/// Complete GSP shared-memory size used by the current GX and relay layouts.
pub const GSP_SHARED_MEMORY_SIZE: usize = 0x1000;

/// A service-side association between a GSP client and its guest resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspClientRegistration {
    shared_memory_base: u32,
    event_handle: u32,
}

impl GspClientRegistration {
    /// Guest virtual base of the already-mapped GSP shared-memory block.
    pub const fn shared_memory_base(self) -> u32 {
        self.shared_memory_base
    }

    /// Process-local event handle signaled after a relay event is queued.
    pub const fn event_handle(self) -> u32 {
        self.event_handle
    }
}

/// Errors raised while registering a client or delivering a relay interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GspServiceError {
    InvalidClientId { client_id: u8 },
    ClientAlreadyRegistered { client_id: u8 },
    ClientNotRegistered { client_id: u8 },
    MisalignedSharedMemoryBase { address: u32 },
    SharedMemoryUnavailable { address: u32 },
    AddressOverflow { base: u32, offset: u32 },
    InvalidEventHandle { handle: u32 },
    InvalidRelayQueue { source: GspInterruptQueueError },
    EventSignalFailed { source: KernelHandleError },
}

impl fmt::Display for GspServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GspServiceError {}

/// Report from delivering one interrupt through a registered GSP client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspInterruptDeliveryReport {
    pub client_id: u8,
    pub interrupt: GspInterruptId,
    pub push_result: GspInterruptQueuePushResult,
    /// True only when this delivery actually signaled the registered event.
    pub event_signaled: bool,
    pub pending_interrupts: u8,
    pub missed_pdc0: u32,
    pub missed_pdc1: u32,
    pub error_code: u8,
}

/// Service-side registry for the four GSP shared-memory client slots.
///
/// This type deliberately does not create shared memory or decode IPC. A
/// caller must first map a writable 0x1000-byte block into the process and
/// create the event object, then register those existing guest resources here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GspServiceRegistry {
    clients: [Option<GspClientRegistration>; GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize],
}

impl Default for GspServiceRegistry {
    fn default() -> Self {
        Self {
            clients: [None; GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize],
        }
    }
}

impl GspServiceRegistry {
    pub const fn new() -> Self {
        Self {
            clients: [None; GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize],
        }
    }

    /// Registers a client against existing process memory and an event handle.
    ///
    /// The entire 0x1000-byte shared block must already be writable and
    /// 0x1000-aligned. The selected relay queue header must also be valid.
    pub fn register_client(
        &mut self,
        session: &mut Arm11ProcessSession,
        client_id: u8,
        shared_memory_base: u32,
        event_handle: u32,
    ) -> Result<(), GspServiceError> {
        let index = Self::client_index(client_id)?;
        if self.clients[index].is_some() {
            return Err(GspServiceError::ClientAlreadyRegistered { client_id });
        }
        if shared_memory_base & 0xFFF != 0 {
            return Err(GspServiceError::MisalignedSharedMemoryBase {
                address: shared_memory_base,
            });
        }
        if shared_memory_base
            .checked_add(GSP_SHARED_MEMORY_SIZE as u32)
            .is_none()
            || !session
                .address_space
                .is_writable_range(shared_memory_base, GSP_SHARED_MEMORY_SIZE)
        {
            return Err(GspServiceError::SharedMemoryUnavailable {
                address: shared_memory_base,
            });
        }
        if session
            .kernel_objects
            .event_reset_type(event_handle)
            .is_none()
        {
            return Err(GspServiceError::InvalidEventHandle {
                handle: event_handle,
            });
        }

        let queue_offset = GspInterruptRelayQueue::client_queue_offset(client_id)
            .map_err(|source| GspServiceError::InvalidRelayQueue { source })?;
        let queue_address = shared_memory_base.checked_add(queue_offset).ok_or(
            GspServiceError::AddressOverflow {
                base: shared_memory_base,
                offset: queue_offset,
            },
        )?;
        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session
            .read_memory_bytes(queue_address, &mut bytes)
            .map_err(|_| GspServiceError::SharedMemoryUnavailable {
                address: queue_address,
            })?;
        GspInterruptRelayQueue::parse(&bytes)
            .map_err(|source| GspServiceError::InvalidRelayQueue { source })?;

        self.clients[index] = Some(GspClientRegistration {
            shared_memory_base,
            event_handle,
        });
        Ok(())
    }

    /// Unregisters one client. The caller remains responsible for unmapping
    /// shared memory and closing any event handle it owns.
    pub fn unregister_client(
        &mut self,
        client_id: u8,
    ) -> Result<GspClientRegistration, GspServiceError> {
        let index = Self::client_index(client_id)?;
        self.clients[index]
            .take()
            .ok_or(GspServiceError::ClientNotRegistered { client_id })
    }

    /// Returns the current registration for a client slot, if present.
    pub fn client_registration(&self, client_id: u8) -> Option<GspClientRegistration> {
        Self::client_index(client_id)
            .ok()
            .and_then(|index| self.clients[index])
    }

    /// Returns the number of currently registered GSP clients.
    pub fn registered_client_count(&self) -> usize {
        self.clients
            .iter()
            .filter(|client| client.is_some())
            .count()
    }

    /// Appends an interrupt to the client's guest relay queue and signals its
    /// kernel event only when the queue accepted the interrupt.
    ///
    /// The handle is revalidated on every delivery so a guest-side CloseHandle
    /// cannot leave a stale registration silently signaling a different object.
    pub fn deliver_interrupt(
        &self,
        session: &mut Arm11ProcessSession,
        client_id: u8,
        interrupt: GspInterruptId,
    ) -> Result<GspInterruptDeliveryReport, GspServiceError> {
        let registration = self
            .client_registration(client_id)
            .ok_or(GspServiceError::ClientNotRegistered { client_id })?;
        if session
            .kernel_objects
            .event_reset_type(registration.event_handle)
            .is_none()
        {
            return Err(GspServiceError::InvalidEventHandle {
                handle: registration.event_handle,
            });
        }

        let queue_offset = GspInterruptRelayQueue::client_queue_offset(client_id)
            .map_err(|source| GspServiceError::InvalidRelayQueue { source })?;
        let queue_address = registration
            .shared_memory_base
            .checked_add(queue_offset)
            .ok_or(GspServiceError::AddressOverflow {
                base: registration.shared_memory_base,
                offset: queue_offset,
            })?;
        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session
            .read_memory_bytes(queue_address, &mut bytes)
            .map_err(|_| GspServiceError::SharedMemoryUnavailable {
                address: queue_address,
            })?;
        let mut queue = GspInterruptRelayQueue::parse(&bytes)
            .map_err(|source| GspServiceError::InvalidRelayQueue { source })?;

        let push_result = queue.push(interrupt);
        session
            .write_memory_bytes(queue_address, &queue.encode())
            .map_err(|_| GspServiceError::SharedMemoryUnavailable {
                address: queue_address,
            })?;

        let event_signaled = if matches!(push_result, GspInterruptQueuePushResult::Queued { .. }) {
            session
                .signal_event_handle(registration.event_handle)
                .map_err(|source| GspServiceError::EventSignalFailed { source })?;
            true
        } else {
            false
        };

        Ok(GspInterruptDeliveryReport {
            client_id,
            interrupt,
            push_result,
            event_signaled,
            pending_interrupts: queue.number_interrupts(),
            missed_pdc0: queue.missed_pdc0(),
            missed_pdc1: queue.missed_pdc1(),
            error_code: queue.error_code(),
        })
    }

    fn client_index(client_id: u8) -> Result<usize, GspServiceError> {
        if client_id >= GSP_INTERRUPT_QUEUE_CLIENT_COUNT {
            return Err(GspServiceError::InvalidClientId { client_id });
        }
        Ok(usize::from(client_id))
    }
}

#[cfg(test)]
mod tests {
    use super::super::Arm11CodeAddressSpace;
    use super::super::kernel::EventResetType;
    use super::*;
    use crate::loader::ncsd::PreparedArm11Image;
    use crate::loader::ncsd::ncch::extended_header::{CodeSetInfo, NcchExtendedHeader};

    const SHARED_MEMORY_BASE: u32 = 0x0020_0000;

    fn test_session() -> Arm11ProcessSession {
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
                    address: SHARED_MEMORY_BASE,
                    physical_size_pages: 1,
                    size_bytes: GSP_SHARED_MEMORY_SIZE as u32,
                },
                bss_size_bytes: 0,
                dependencies: [0; 48],
                save_data_size_bytes: 0,
                jump_id: 0,
                access_control_info: [0; 0x200],
            },
            text: vec![0; 0x1000],
            read_only: Vec::new(),
            data: vec![0; GSP_SHARED_MEMORY_SIZE],
        };
        let address_space = Arm11CodeAddressSpace::from_prepared(prepared).unwrap();
        Arm11ProcessSession::from_address_space(address_space)
    }

    #[test]
    fn registration_binds_client_to_mapped_shared_memory_and_event_handle() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut registry = GspServiceRegistry::new();

        registry
            .register_client(&mut session, 2, SHARED_MEMORY_BASE, event)
            .unwrap();

        assert_eq!(registry.registered_client_count(), 1);
        assert_eq!(
            registry.client_registration(2),
            Some(GspClientRegistration {
                shared_memory_base: SHARED_MEMORY_BASE,
                event_handle: event,
            })
        );
        assert_eq!(
            registry.unregister_client(2).unwrap(),
            GspClientRegistration {
                shared_memory_base: SHARED_MEMORY_BASE,
                event_handle: event,
            }
        );
        assert_eq!(registry.registered_client_count(), 0);
    }

    #[test]
    fn delivery_updates_guest_queue_and_signals_the_registered_event() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut registry = GspServiceRegistry::new();
        registry
            .register_client(&mut session, 1, SHARED_MEMORY_BASE, event)
            .unwrap();

        let report = registry
            .deliver_interrupt(&mut session, 1, GspInterruptId::P3d)
            .unwrap();

        assert_eq!(
            report.push_result,
            GspInterruptQueuePushResult::Queued { slot: 0 }
        );
        assert!(report.event_signaled);
        assert_eq!(report.pending_interrupts, 1);
        assert_eq!(session.kernel_objects.event_is_signaled(event), Some(true));

        let queue_address = SHARED_MEMORY_BASE + GSP_INTERRUPT_QUEUE_SIZE as u32;
        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session
            .read_memory_bytes(queue_address, &mut bytes)
            .unwrap();
        let queue = GspInterruptRelayQueue::parse(&bytes).unwrap();
        assert_eq!(queue.pending_interrupts().unwrap(), [GspInterruptId::P3d]);
    }

    #[test]
    fn ignored_pdc_interrupt_does_not_signal_the_event() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut shared_block = [0u8; GSP_SHARED_MEMORY_SIZE];
        let queue_offset = GSP_INTERRUPT_QUEUE_SIZE;
        shared_block[queue_offset + 3] = crate::gpu::GSP_INTERRUPT_QUEUE_IGNORE_PDC;
        session
            .write_memory_bytes(SHARED_MEMORY_BASE, &shared_block)
            .unwrap();

        let mut registry = GspServiceRegistry::new();
        registry
            .register_client(&mut session, 1, SHARED_MEMORY_BASE, event)
            .unwrap();
        let report = registry
            .deliver_interrupt(&mut session, 1, GspInterruptId::Pdc0)
            .unwrap();

        assert_eq!(report.push_result, GspInterruptQueuePushResult::IgnoredPdc);
        assert!(!report.event_signaled);
        assert_eq!(session.kernel_objects.event_is_signaled(event), Some(false));
    }

    #[test]
    fn invalid_resources_and_duplicate_registration_are_rejected() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::OneShot)
            .unwrap();
        let mutex = session
            .kernel_objects
            .insert_mutex(false, super::super::MAIN_THREAD_ID)
            .unwrap();
        let mut registry = GspServiceRegistry::new();

        assert_eq!(
            registry.register_client(&mut session, 4, SHARED_MEMORY_BASE, event),
            Err(GspServiceError::InvalidClientId { client_id: 4 })
        );
        assert!(matches!(
            registry.register_client(&mut session, 0, SHARED_MEMORY_BASE + 1, event),
            Err(GspServiceError::MisalignedSharedMemoryBase { .. })
        ));
        assert_eq!(
            registry.register_client(&mut session, 0, SHARED_MEMORY_BASE, mutex),
            Err(GspServiceError::InvalidEventHandle { handle: mutex })
        );
        registry
            .register_client(&mut session, 0, SHARED_MEMORY_BASE, event)
            .unwrap();
        assert_eq!(
            registry.register_client(&mut session, 0, SHARED_MEMORY_BASE, event),
            Err(GspServiceError::ClientAlreadyRegistered { client_id: 0 })
        );
        assert_eq!(
            registry.deliver_interrupt(&mut session, 3, GspInterruptId::Dma),
            Err(GspServiceError::ClientNotRegistered { client_id: 3 })
        );
    }

    #[test]
    fn closed_event_handle_is_rejected_before_guest_queue_is_modified() {
        let mut session = test_session();
        let event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut registry = GspServiceRegistry::new();
        registry
            .register_client(&mut session, 0, SHARED_MEMORY_BASE, event)
            .unwrap();
        session.kernel_objects.close_handle(event).unwrap();

        assert_eq!(
            registry.deliver_interrupt(&mut session, 0, GspInterruptId::Dma),
            Err(GspServiceError::InvalidEventHandle { handle: event })
        );

        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session
            .read_memory_bytes(SHARED_MEMORY_BASE, &mut bytes)
            .unwrap();
        let queue = GspInterruptRelayQueue::parse(&bytes).unwrap();
        assert!(queue.pending_interrupts().unwrap().is_empty());
    }
}
