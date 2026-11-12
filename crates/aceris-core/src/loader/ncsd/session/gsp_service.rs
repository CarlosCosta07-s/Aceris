//! Process-side GSP client registration and interrupt delivery.
//!
//! The registry is owned by the service layer, not by the guest process. It
//! connects a registered client slot to a process-mapped GSP shared-memory
//! block and a process-local kernel event handle. Shared-memory creation,
//! mapping and IPC command dispatch remain responsibilities of later layers.

use std::fmt;

use super::Arm11ProcessSession;
use super::kernel::KernelHandleError;
use crate::gpu::{
    GSP_INTERRUPT_QUEUE_CLIENT_COUNT, GSP_INTERRUPT_QUEUE_SIZE, GspInterruptId,
    GspInterruptQueueError, GspInterruptQueuePushResult, GspInterruptRelayQueue,
};

/// IPC command headers exposed by the `gsp::Gpu` service.
pub const GSP_IPC_TRIGGER_CMD_REQ_QUEUE_HEADER: u32 = 0x000C_0000;
pub const GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER: u32 = 0x0013_0042;
pub const GSP_IPC_UNREGISTER_INTERRUPT_RELAY_QUEUE_HEADER: u32 = 0x0014_0000;
pub const GSP_IPC_TRY_ACQUIRE_RIGHT_HEADER: u32 = 0x0015_0002;
pub const GSP_IPC_ACQUIRE_RIGHT_HEADER: u32 = 0x0016_0042;
pub const GSP_IPC_RELEASE_RIGHT_HEADER: u32 = 0x0017_0000;

/// Descriptor for one copied kernel handle in an IPC translation section.
pub const GSP_IPC_ONE_HANDLE_DESCRIPTOR: u32 = 0;

/// A decoded request to the GSP GPU service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GspIpcRequest {
    /// Start processing entries in the caller's GX command queue.
    TriggerCmdReqQueue,
    /// Register the caller's event and flags for relay queue delivery.
    RegisterInterruptRelayQueue { flags: u32, event_handle: u32 },
    /// Remove the caller's interrupt relay queue registration.
    UnregisterInterruptRelayQueue,
    /// Attempt to acquire rendering rights without waiting for another client.
    TryAcquireRight { process_handle: u32 },
    /// Acquire rendering rights, waiting when another client owns them.
    AcquireRight { flags: u32, process_handle: u32 },
    /// Release rendering rights currently held by the caller.
    ReleaseRight,
}

/// Errors returned when decoding a GSP GPU service IPC request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GspIpcRequestError {
    EmptyMessage,
    UnsupportedCommandHeader {
        header: u32,
    },
    TruncatedMessage {
        header: u32,
        expected_words: usize,
        actual_words: usize,
    },
    InvalidHandleDescriptor {
        descriptor: u32,
    },
}

impl fmt::Display for GspIpcRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GspIpcRequestError {}

/// Decodes GSP relay, queue and rendering-right IPC requests.
///
/// The kernel is expected to have translated handles before the message is
/// delivered to this service. The input slice may include unused TLS command
/// buffer words after the parameters declared by the IPC header.
pub fn decode_gsp_ipc_request(words: &[u32]) -> Result<GspIpcRequest, GspIpcRequestError> {
    let header = *words.first().ok_or(GspIpcRequestError::EmptyMessage)?;

    match header {
        GSP_IPC_TRIGGER_CMD_REQ_QUEUE_HEADER => Ok(GspIpcRequest::TriggerCmdReqQueue),
        GSP_IPC_UNREGISTER_INTERRUPT_RELAY_QUEUE_HEADER => {
            Ok(GspIpcRequest::UnregisterInterruptRelayQueue)
        }
        GSP_IPC_TRY_ACQUIRE_RIGHT_HEADER => {
            if words.len() < 3 {
                return Err(GspIpcRequestError::TruncatedMessage {
                    header,
                    expected_words: 3,
                    actual_words: words.len(),
                });
            }
            let descriptor = words[1];
            if descriptor != GSP_IPC_ONE_HANDLE_DESCRIPTOR {
                return Err(GspIpcRequestError::InvalidHandleDescriptor { descriptor });
            }

            Ok(GspIpcRequest::TryAcquireRight {
                process_handle: words[2],
            })
        }
        GSP_IPC_ACQUIRE_RIGHT_HEADER => {
            if words.len() < 4 {
                return Err(GspIpcRequestError::TruncatedMessage {
                    header,
                    expected_words: 4,
                    actual_words: words.len(),
                });
            }
            let descriptor = words[2];
            if descriptor != GSP_IPC_ONE_HANDLE_DESCRIPTOR {
                return Err(GspIpcRequestError::InvalidHandleDescriptor { descriptor });
            }

            Ok(GspIpcRequest::AcquireRight {
                flags: words[1],
                process_handle: words[3],
            })
        }
        GSP_IPC_RELEASE_RIGHT_HEADER => Ok(GspIpcRequest::ReleaseRight),
        GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER => {
            if words.len() < 4 {
                return Err(GspIpcRequestError::TruncatedMessage {
                    header,
                    expected_words: 4,
                    actual_words: words.len(),
                });
            }
            let descriptor = words[2];
            if descriptor != GSP_IPC_ONE_HANDLE_DESCRIPTOR {
                return Err(GspIpcRequestError::InvalidHandleDescriptor { descriptor });
            }

            Ok(GspIpcRequest::RegisterInterruptRelayQueue {
                flags: words[1],
                event_handle: words[3],
            })
        }
        _ => Err(GspIpcRequestError::UnsupportedCommandHeader { header }),
    }
}

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
    InvalidClientId {
        client_id: u8,
    },
    ClientAlreadyRegistered {
        client_id: u8,
    },
    ClientNotRegistered {
        client_id: u8,
    },
    NoFreeClientSlots,
    MisalignedSharedMemoryBase {
        address: u32,
    },
    SharedMemoryUnavailable {
        address: u32,
    },
    AddressOverflow {
        base: u32,
        offset: u32,
    },
    InvalidEventHandle {
        handle: u32,
    },
    InvalidRelayQueue {
        source: GspInterruptQueueError,
    },
    EventSignalFailed {
        source: KernelHandleError,
    },
    RenderingRightRequired {
        client_id: u8,
        interrupt: GspInterruptId,
    },
    RenderingRightNotOwned {
        client_id: u8,
    },
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
    rendering_client: Option<u8>,
}

impl Default for GspServiceRegistry {
    fn default() -> Self {
        Self {
            clients: [None; GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize],
            rendering_client: None,
        }
    }
}

impl GspServiceRegistry {
    pub const fn new() -> Self {
        Self {
            clients: [None; GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize],
            rendering_client: None,
        }
    }

    /// Registers a client in the lowest available slot and returns its ID.
    ///
    /// This deterministic first-free policy belongs to Aceris' service layer;
    /// the caller still owns shared-memory creation, mapping and event creation.
    /// A failed validation leaves the registry unchanged.
    pub fn register_next_available_client(
        &mut self,
        session: &mut Arm11ProcessSession,
        shared_memory_base: u32,
        event_handle: u32,
    ) -> Result<u8, GspServiceError> {
        let index = self
            .clients
            .iter()
            .position(|client| client.is_none())
            .ok_or(GspServiceError::NoFreeClientSlots)?;
        let client_id = u8::try_from(index).map_err(|_| GspServiceError::NoFreeClientSlots)?;

        self.register_client(session, client_id, shared_memory_base, event_handle)?;
        Ok(client_id)
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
        let registration = self.clients[index]
            .take()
            .ok_or(GspServiceError::ClientNotRegistered { client_id })?;
        if self.rendering_client == Some(client_id) {
            self.rendering_client = None;
        }
        Ok(registration)
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

    /// Attempts to grant exclusive GPU rendering rights to a registered client.
    ///
    /// Returns `true` when the caller now owns the right, including when it
    /// already held it. Returns `false` when another client owns the right.
    /// This non-blocking primitive is the basis for GSP TryAcquireRight;
    /// blocking AcquireRight needs the IPC wait/notification layer.
    pub fn try_acquire_rendering_right(&mut self, client_id: u8) -> Result<bool, GspServiceError> {
        let index = Self::client_index(client_id)?;
        if self.clients[index].is_none() {
            return Err(GspServiceError::ClientNotRegistered { client_id });
        }

        match self.rendering_client {
            None => {
                self.rendering_client = Some(client_id);
                Ok(true)
            }
            Some(owner) if owner == client_id => Ok(true),
            Some(_) => Ok(false),
        }
    }

    /// Releases GPU rendering rights held by the specified client.
    pub fn release_rendering_right(&mut self, client_id: u8) -> Result<(), GspServiceError> {
        let index = Self::client_index(client_id)?;
        if self.clients[index].is_none() {
            return Err(GspServiceError::ClientNotRegistered { client_id });
        }
        if self.rendering_client != Some(client_id) {
            return Err(GspServiceError::RenderingRightNotOwned { client_id });
        }
        self.rendering_client = None;
        Ok(())
    }

    /// Returns whether a registered client currently owns rendering rights.
    pub fn owns_rendering_right(&self, client_id: u8) -> bool {
        Self::client_index(client_id).is_ok()
            && self.clients[usize::from(client_id)].is_some()
            && self.rendering_client == Some(client_id)
    }

    /// Returns whether a client is eligible to receive a particular interrupt.
    ///
    /// PDC0 and PDC1 are broadcast to all registered clients. All other GSP
    /// interrupts are routed only to the client holding rendering rights.
    pub fn client_may_receive_interrupt(
        &self,
        client_id: u8,
        interrupt: GspInterruptId,
    ) -> Result<bool, GspServiceError> {
        let index = Self::client_index(client_id)?;
        if self.clients[index].is_none() {
            return Err(GspServiceError::ClientNotRegistered { client_id });
        }
        Ok(interrupt.is_pdc() || self.rendering_client == Some(client_id))
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
        let index = Self::client_index(client_id)?;
        let registration =
            self.clients[index].ok_or(GspServiceError::ClientNotRegistered { client_id })?;
        if !self.client_may_receive_interrupt(client_id, interrupt)? {
            return Err(GspServiceError::RenderingRightRequired {
                client_id,
                interrupt,
            });
        }
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
    fn gsp_ipc_decoder_recognizes_supported_command_headers() {
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_TRIGGER_CMD_REQ_QUEUE_HEADER]),
            Ok(GspIpcRequest::TriggerCmdReqQueue)
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_UNREGISTER_INTERRUPT_RELAY_QUEUE_HEADER]),
            Ok(GspIpcRequest::UnregisterInterruptRelayQueue)
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_RELEASE_RIGHT_HEADER]),
            Ok(GspIpcRequest::ReleaseRight)
        );
        assert_eq!(
            decode_gsp_ipc_request(&[
                GSP_IPC_ACQUIRE_RIGHT_HEADER,
                1,
                GSP_IPC_ONE_HANDLE_DESCRIPTOR,
                0x6789,
            ]),
            Ok(GspIpcRequest::AcquireRight {
                flags: 1,
                process_handle: 0x6789,
            })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[
                GSP_IPC_TRY_ACQUIRE_RIGHT_HEADER,
                GSP_IPC_ONE_HANDLE_DESCRIPTOR,
                0x5678,
            ]),
            Ok(GspIpcRequest::TryAcquireRight {
                process_handle: 0x5678,
            })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[
                GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER,
                0xA5A5_0001,
                GSP_IPC_ONE_HANDLE_DESCRIPTOR,
                0x1234,
            ]),
            Ok(GspIpcRequest::RegisterInterruptRelayQueue {
                flags: 0xA5A5_0001,
                event_handle: 0x1234,
            })
        );
    }

    #[test]
    fn gsp_ipc_decoder_rejects_empty_truncated_and_malformed_messages() {
        assert_eq!(
            decode_gsp_ipc_request(&[]),
            Err(GspIpcRequestError::EmptyMessage)
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER, 0]),
            Err(GspIpcRequestError::TruncatedMessage {
                header: GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER,
                expected_words: 4,
                actual_words: 2,
            })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER, 0, 2, 0x1234,]),
            Err(GspIpcRequestError::InvalidHandleDescriptor { descriptor: 2 })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_TRY_ACQUIRE_RIGHT_HEADER, 0]),
            Err(GspIpcRequestError::TruncatedMessage {
                header: GSP_IPC_TRY_ACQUIRE_RIGHT_HEADER,
                expected_words: 3,
                actual_words: 2,
            })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_TRY_ACQUIRE_RIGHT_HEADER, 2, 0x5678]),
            Err(GspIpcRequestError::InvalidHandleDescriptor { descriptor: 2 })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[
                GSP_IPC_ACQUIRE_RIGHT_HEADER,
                0,
                GSP_IPC_ONE_HANDLE_DESCRIPTOR,
            ]),
            Err(GspIpcRequestError::TruncatedMessage {
                header: GSP_IPC_ACQUIRE_RIGHT_HEADER,
                expected_words: 4,
                actual_words: 3,
            })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[GSP_IPC_ACQUIRE_RIGHT_HEADER, 0, 2, 0x6789,]),
            Err(GspIpcRequestError::InvalidHandleDescriptor { descriptor: 2 })
        );
        assert_eq!(
            decode_gsp_ipc_request(&[0x0020_0040, 0, 0]),
            Err(GspIpcRequestError::UnsupportedCommandHeader {
                header: 0x0020_0040
            })
        );
    }

    #[test]
    fn gsp_ipc_decoder_ignores_unused_tls_words_after_declared_parameters() {
        assert_eq!(
            decode_gsp_ipc_request(&[
                GSP_IPC_REGISTER_INTERRUPT_RELAY_QUEUE_HEADER,
                0x55,
                GSP_IPC_ONE_HANDLE_DESCRIPTOR,
                0x100,
                0xDEAD_BEEF,
                0xCAFE_BABE,
            ]),
            Ok(GspIpcRequest::RegisterInterruptRelayQueue {
                flags: 0x55,
                event_handle: 0x100,
            })
        );
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
    fn automatic_registration_allocates_free_slots_and_reuses_unregistered_slots() {
        let mut session = test_session();
        let events = (0..5)
            .map(|_| {
                session
                    .kernel_objects
                    .insert_event(EventResetType::Sticky)
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut registry = GspServiceRegistry::new();

        // Invalid resources must not reserve the first slot.
        assert_eq!(
            registry.register_next_available_client(&mut session, SHARED_MEMORY_BASE, u32::MAX,),
            Err(GspServiceError::InvalidEventHandle { handle: u32::MAX })
        );
        assert_eq!(registry.registered_client_count(), 0);

        for (expected_id, event) in events[..4].iter().copied().enumerate() {
            assert_eq!(
                registry
                    .register_next_available_client(&mut session, SHARED_MEMORY_BASE, event)
                    .unwrap(),
                expected_id as u8
            );
        }
        assert_eq!(registry.registered_client_count(), 4);
        assert_eq!(
            registry.register_next_available_client(&mut session, SHARED_MEMORY_BASE, events[4],),
            Err(GspServiceError::NoFreeClientSlots)
        );

        registry.unregister_client(1).unwrap();
        assert_eq!(
            registry
                .register_next_available_client(&mut session, SHARED_MEMORY_BASE, events[4])
                .unwrap(),
            1
        );
        assert_eq!(registry.registered_client_count(), 4);
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

        assert!(registry.try_acquire_rendering_right(1).unwrap());
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
    fn rendering_right_is_exclusive_controls_interrupt_routing_and_is_released() {
        let mut session = test_session();
        let first_event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let second_event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut registry = GspServiceRegistry::new();
        registry
            .register_client(&mut session, 0, SHARED_MEMORY_BASE, first_event)
            .unwrap();
        registry
            .register_client(&mut session, 1, SHARED_MEMORY_BASE, second_event)
            .unwrap();

        assert!(
            !registry
                .client_may_receive_interrupt(0, GspInterruptId::Dma)
                .unwrap()
        );
        assert!(
            registry
                .client_may_receive_interrupt(0, GspInterruptId::Pdc0)
                .unwrap()
        );
        assert!(registry.try_acquire_rendering_right(0).unwrap());
        assert!(registry.owns_rendering_right(0));
        assert!(registry.try_acquire_rendering_right(0).unwrap());
        assert!(!registry.try_acquire_rendering_right(1).unwrap());
        assert!(
            !registry
                .client_may_receive_interrupt(1, GspInterruptId::P3d)
                .unwrap()
        );
        assert!(
            registry
                .client_may_receive_interrupt(1, GspInterruptId::Pdc1)
                .unwrap()
        );

        assert_eq!(
            registry.deliver_interrupt(&mut session, 1, GspInterruptId::Dma),
            Err(GspServiceError::RenderingRightRequired {
                client_id: 1,
                interrupt: GspInterruptId::Dma,
            })
        );
        registry.release_rendering_right(0).unwrap();
        assert!(!registry.owns_rendering_right(0));
        assert!(registry.try_acquire_rendering_right(1).unwrap());
        assert!(registry.owns_rendering_right(1));

        registry.unregister_client(1).unwrap();
        assert!(!registry.owns_rendering_right(1));
        assert!(registry.try_acquire_rendering_right(0).unwrap());
    }

    #[test]
    fn releasing_rendering_right_from_another_client_is_rejected() {
        let mut session = test_session();
        let first_event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let second_event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let mut registry = GspServiceRegistry::new();
        registry
            .register_client(&mut session, 0, SHARED_MEMORY_BASE, first_event)
            .unwrap();
        registry
            .register_client(&mut session, 1, SHARED_MEMORY_BASE, second_event)
            .unwrap();
        assert!(registry.try_acquire_rendering_right(0).unwrap());

        assert_eq!(
            registry.release_rendering_right(1),
            Err(GspServiceError::RenderingRightNotOwned { client_id: 1 })
        );
        assert!(registry.owns_rendering_right(0));
    }

    #[test]
    fn missed_pdc_and_full_queue_events_do_not_signal_registered_handles() {
        let mut session = test_session();
        let pdc_event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();
        let full_event = session
            .kernel_objects
            .insert_event(EventResetType::Sticky)
            .unwrap();

        let mut pdc_queue = GspInterruptRelayQueue::new();
        for _ in 0..0x20 {
            assert!(matches!(
                pdc_queue.push(GspInterruptId::Pdc0),
                GspInterruptQueuePushResult::Queued { .. }
            ));
        }
        let mut full_queue = GspInterruptRelayQueue::new();
        for _ in 0..0x34 {
            assert!(matches!(
                full_queue.push(GspInterruptId::P3d),
                GspInterruptQueuePushResult::Queued { .. }
            ));
        }

        let client_zero_offset = GspInterruptRelayQueue::client_queue_offset(0).unwrap();
        let client_one_offset = GspInterruptRelayQueue::client_queue_offset(1).unwrap();
        session
            .write_memory_bytes(SHARED_MEMORY_BASE + client_zero_offset, &pdc_queue.encode())
            .unwrap();
        session
            .write_memory_bytes(SHARED_MEMORY_BASE + client_one_offset, &full_queue.encode())
            .unwrap();

        let mut registry = GspServiceRegistry::new();
        registry
            .register_client(&mut session, 0, SHARED_MEMORY_BASE, pdc_event)
            .unwrap();
        registry
            .register_client(&mut session, 1, SHARED_MEMORY_BASE, full_event)
            .unwrap();
        assert!(registry.try_acquire_rendering_right(1).unwrap());

        let missed = registry
            .deliver_interrupt(&mut session, 0, GspInterruptId::Pdc1)
            .unwrap();
        assert_eq!(
            missed.push_result,
            GspInterruptQueuePushResult::MissedPdc {
                interrupt: GspInterruptId::Pdc1,
                total: 1,
            }
        );
        assert!(!missed.event_signaled);

        let full = registry
            .deliver_interrupt(&mut session, 1, GspInterruptId::Dma)
            .unwrap();
        assert_eq!(full.push_result, GspInterruptQueuePushResult::Full);
        assert!(!full.event_signaled);

        assert_eq!(
            session.kernel_objects.event_is_signaled(pdc_event),
            Some(false)
        );
        assert_eq!(
            session.kernel_objects.event_is_signaled(full_event),
            Some(false)
        );

        let mut pdc_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session
            .read_memory_bytes(SHARED_MEMORY_BASE + client_zero_offset, &mut pdc_bytes)
            .unwrap();
        let pdc_queue = GspInterruptRelayQueue::parse(&pdc_bytes).unwrap();
        assert_eq!(pdc_queue.missed_pdc1(), 1);

        let mut full_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session
            .read_memory_bytes(SHARED_MEMORY_BASE + client_one_offset, &mut full_bytes)
            .unwrap();
        let full_queue = GspInterruptRelayQueue::parse(&full_bytes).unwrap();
        assert_eq!(
            full_queue.error_code(),
            crate::gpu::GSP_INTERRUPT_QUEUE_FULL_ERROR
        );
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
        assert_eq!(
            registry.register_client(&mut session, 0, SHARED_MEMORY_BASE + 0x10000, event),
            Err(GspServiceError::SharedMemoryUnavailable {
                address: SHARED_MEMORY_BASE + 0x10000,
            })
        );
        assert_eq!(
            registry.deliver_interrupt(&mut session, 4, GspInterruptId::Dma),
            Err(GspServiceError::InvalidClientId { client_id: 4 })
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
        assert!(registry.try_acquire_rendering_right(0).unwrap());
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
