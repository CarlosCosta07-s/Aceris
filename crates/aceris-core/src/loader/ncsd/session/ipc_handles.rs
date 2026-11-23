//! Type-0 IPC kernel-handle translation for ARM11 messages.
//!
//! The current HLE services execute inside the application process context.
//! This module still applies the kernel's COPY_HANDLE/MOVE_HANDLE rules at the
//! message boundary, so service code never receives unchecked guest handle IDs.
//! Process-ID descriptors are replaced with the sender's virtual process ID.
//! Null/pseudo handles retain their special values; independent process handle
//! namespaces remain future work.

use std::collections::BTreeSet;

use super::ipc::{IpcMessage, IpcTranslationParameter};
use super::kernel::{
    Arm11KernelObjectTable, CURRENT_PROCESS_HANDLE, CURRENT_THREAD_HANDLE,
    IpcHandleTranslationDescriptor, IpcHandleTranslationError,
};

/// Validates and translates every type-0 handle descriptor in one IPC message.
///
/// COPY_HANDLE creates receiver-side aliases while preserving the sender's
/// handles. MOVE_HANDLE creates all receiver aliases first, then closes the
/// sender handles. If any validation or allocation fails, newly created
/// aliases are closed and the input message is left unchanged.
pub fn translate_message_handles(
    objects: &mut Arm11KernelObjectTable,
    process_id: u32,
    words: &mut [u32],
) -> Result<(), IpcHandleTranslationError> {
    let message =
        IpcMessage::decode(words).map_err(|_| IpcHandleTranslationError::InvalidDescriptor)?;
    let parameters = message
        .translation_parameters()
        .map_err(|_| IpcHandleTranslationError::InvalidDescriptor)?;

    let translation_start = 1 + usize::from(message.header().normal_parameter_count());
    let mut transfers = Vec::new();
    let mut process_id_payloads = Vec::new();
    let mut moved_handles = BTreeSet::new();

    for parameter in parameters {
        let IpcTranslationParameter::Handles {
            descriptor,
            word_offset,
            handles,
            ..
        } = parameter
        else {
            continue;
        };

        let decoded = IpcHandleTranslationDescriptor::decode(descriptor)?;
        if usize::from(decoded.handle_count()) != handles.len() {
            return Err(IpcHandleTranslationError::HandleCountMismatch {
                expected: decoded.handle_count(),
                actual: handles.len(),
            });
        }

        if decoded.requests_process_id() {
            if decoded.handle_count() != 1 {
                return Err(IpcHandleTranslationError::InvalidDescriptor);
            }
            process_id_payloads.push(translation_start + word_offset + 1);
            continue;
        }

        let mut source_handles = Vec::with_capacity(handles.len());
        for (offset, handle) in handles.iter().copied().enumerate() {
            if handle == 0 || handle == CURRENT_PROCESS_HANDLE || handle == CURRENT_THREAD_HANDLE {
                if decoded.moves_handle() && handle != 0 {
                    return Err(IpcHandleTranslationError::Kernel(
                        super::kernel::KernelHandleError::InvalidHandle,
                    ));
                }
                continue;
            }
            if !objects.is_open_handle(handle) {
                return Err(IpcHandleTranslationError::Kernel(
                    super::kernel::KernelHandleError::InvalidHandle,
                ));
            }
            if decoded.moves_handle() && !moved_handles.insert(handle) {
                return Err(IpcHandleTranslationError::DuplicateMoveHandle { handle });
            }
            source_handles.push((offset, handle));
        }

        transfers.push((
            translation_start + word_offset + 1,
            decoded.moves_handle(),
            source_handles,
        ));
    }

    let mut replacements = Vec::new();
    for (payload_start, move_handles, source_handles) in &transfers {
        for (offset, source_handle) in source_handles.iter().copied() {
            match objects.duplicate_handle(source_handle) {
                Ok(receiver_handle) => {
                    replacements.push((
                        payload_start + offset,
                        source_handle,
                        receiver_handle,
                        *move_handles,
                    ));
                }
                Err(error) => {
                    for (_, _, receiver_handle, _) in replacements.drain(..) {
                        let _ = objects.close_handle(receiver_handle);
                    }
                    return Err(IpcHandleTranslationError::Kernel(error));
                }
            }
        }
    }

    for (_, source_handle, _, move_handles) in &replacements {
        if *move_handles {
            // All sources were verified open before allocation, and no
            // concurrent guest code can mutate this process-local table here.
            if let Err(error) = objects.close_handle(*source_handle) {
                for (_, _, receiver_handle, _) in replacements.drain(..) {
                    let _ = objects.close_handle(receiver_handle);
                }
                return Err(IpcHandleTranslationError::Kernel(error));
            }
        }
    }

    for (word_index, _, receiver_handle, _) in replacements {
        words[word_index] = receiver_handle;
    }
    for word_index in process_id_payloads {
        words[word_index] = process_id;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::translate_message_handles;
    use crate::loader::ncsd::session::kernel::{
        Arm11KernelObjectTable, CURRENT_PROCESS_HANDLE, CURRENT_THREAD_HANDLE, EventResetType,
        IpcHandleTranslationError, KernelHandleError,
    };

    fn one_handle_message(descriptor: u32, handle: u32) -> Vec<u32> {
        let header = (0x1234 << 16) | 2;
        vec![header, descriptor, handle]
    }

    #[test]
    fn copy_handle_creates_a_distinct_alias_and_preserves_the_sender_handle() {
        let mut objects = Arm11KernelObjectTable::default();
        let source = objects.insert_event(EventResetType::Sticky).unwrap();
        let mut message = one_handle_message(0, source);

        translate_message_handles(&mut objects, 1, &mut message).unwrap();

        let receiver = message[2];
        assert_ne!(receiver, source);
        assert!(objects.is_open_handle(source));
        assert!(objects.is_open_handle(receiver));
        assert_eq!(
            objects.event_reset_type(receiver),
            Some(EventResetType::Sticky)
        );
        objects.signal_event(receiver).unwrap();
        assert_eq!(objects.event_is_signaled(source), Some(true));
    }

    #[test]
    fn move_handle_transfers_the_guest_visible_handle_and_preserves_object_identity() {
        let mut objects = Arm11KernelObjectTable::default();
        let source = objects.insert_event(EventResetType::OneShot).unwrap();
        let mut message = one_handle_message(1 << 4, source);

        translate_message_handles(&mut objects, 1, &mut message).unwrap();

        let receiver = message[2];
        assert_ne!(receiver, source);
        assert!(!objects.is_open_handle(source));
        assert!(objects.is_open_handle(receiver));
        assert_eq!(
            objects.event_reset_type(receiver),
            Some(EventResetType::OneShot)
        );
    }

    #[test]
    fn invalid_handle_leaves_message_and_object_table_unchanged() {
        let mut objects = Arm11KernelObjectTable::default();
        let mut message = one_handle_message(0, 0xDEAD_BEEF);
        let original = message.clone();
        let initial_count = objects.handle_count();

        assert_eq!(
            translate_message_handles(&mut objects, 1, &mut message),
            Err(IpcHandleTranslationError::Kernel(
                KernelHandleError::InvalidHandle
            ))
        );
        assert_eq!(message, original);
        assert_eq!(objects.handle_count(), initial_count);
    }

    #[test]
    fn duplicate_move_handle_is_rejected_before_any_alias_is_created() {
        let mut objects = Arm11KernelObjectTable::default();
        let source = objects.insert_event(EventResetType::Sticky).unwrap();
        let mut message = vec![(0x1234 << 16) | 3, (1 << 26) | (1 << 4), source, source];
        let initial_count = objects.handle_count();

        assert_eq!(
            translate_message_handles(&mut objects, 1, &mut message),
            Err(IpcHandleTranslationError::DuplicateMoveHandle { handle: source })
        );
        assert_eq!(objects.handle_count(), initial_count);
        assert!(objects.is_open_handle(source));
    }

    #[test]
    fn process_id_descriptor_is_replaced_with_the_senders_process_id() {
        let mut objects = Arm11KernelObjectTable::default();
        let mut message = one_handle_message(1 << 5, 0xDEAD_BEEF);

        translate_message_handles(&mut objects, 0x1234_5678, &mut message).unwrap();

        assert_eq!(message[2], 0x1234_5678);
        assert_eq!(message[1], 1 << 5);
    }

    #[test]
    fn process_id_descriptor_must_describe_exactly_one_placeholder() {
        let mut objects = Arm11KernelObjectTable::default();
        let mut message = vec![(0x1234 << 16) | 3, (1 << 26) | (1 << 5), 7, 8];
        let original = message.clone();

        assert_eq!(
            translate_message_handles(&mut objects, 1, &mut message),
            Err(IpcHandleTranslationError::InvalidDescriptor)
        );
        assert_eq!(message, original);
    }

    #[test]
    fn null_handle_descriptor_preserves_the_zero_sentinel() {
        let mut objects = Arm11KernelObjectTable::default();
        let mut message = one_handle_message(0, 0);
        let original = message.clone();

        translate_message_handles(&mut objects, 1, &mut message).unwrap();

        assert_eq!(message, original);
    }

    #[test]
    fn current_process_and_thread_pseudo_handles_are_preserved() {
        let mut objects = Arm11KernelObjectTable::default();
        for handle in [CURRENT_PROCESS_HANDLE, CURRENT_THREAD_HANDLE] {
            let mut message = one_handle_message(0, handle);
            let original = message.clone();
            translate_message_handles(&mut objects, 1, &mut message).unwrap();
            assert_eq!(message, original);
        }
    }
}
