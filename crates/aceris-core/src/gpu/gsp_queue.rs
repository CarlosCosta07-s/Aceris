//! GSP GX shared-memory command queue decoding.
//!
//! This module models the documented queue header and the 15-entry ring.
//! The system dispatcher consumes ProcessCommandList and MemoryFill entries
//! through the guest TriggerCmdReqQueue path. Other GX command families,
//! busy arbitration and cache maintenance remain incomplete.

use std::fmt;

/// Offset of the first GX command queue within GSP shared memory.
pub const GSP_GX_COMMAND_QUEUE_OFFSET: u32 = 0x800;
/// Distance between per-client GX queues.
pub const GSP_GX_COMMAND_QUEUE_CLIENT_STRIDE: u32 = 0x200;
/// Number of client GX queues in one GSP shared-memory block.
pub const GSP_GX_COMMAND_QUEUE_CLIENT_COUNT: u8 = 4;
/// Size of one complete GX command queue slot.
pub const GSP_GX_COMMAND_QUEUE_SIZE: usize = 0x200;
/// Size of the queue header.
pub const GSP_GX_COMMAND_QUEUE_HEADER_SIZE: usize = 8;
/// Size of one GX command entry.
pub const GSP_GX_COMMAND_ENTRY_SIZE: usize = 0x20;
/// Maximum number of commands in the ring.
pub const GSP_GX_COMMAND_QUEUE_CAPACITY: u8 = 15;
/// GX command ID for ProcessCommandList.
pub const GSP_GX_COMMAND_PROCESS_COMMAND_LIST: u8 = 0x01;
/// GX command ID for MemoryFill.
pub const GSP_GX_COMMAND_MEMORY_FILL: u8 = 0x02;
/// GSP result for an invalid GPU-visible address.
pub const GSP_INVALID_ADDRESS_RESULT: u32 = 0xE0E0_2BF5;

const GSP_QUEUE_STATUS_HALTED: u8 = 0x01;
const GSP_QUEUE_STATUS_ERROR: u8 = 0x80;
const GSP_QUEUE_FLAG_HALT_UNTIL_TRIGGER: u8 = 0x01;
const GSP_COMMAND_FLAG_STOP_AFTER: u8 = 0x01;
const GSP_COMMAND_FLAG_FAIL_IF_BUSY: u8 = 0x01;

/// One decoded eight-word GX command entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspGxCommand {
    pub command_id: u8,
    /// Header byte 2 bit 0: halt queue processing after this entry.
    pub stop_after: bool,
    /// Header byte 3 bit 0: reject if GSP is busy.
    ///
    /// The first Aceris queue processor records this flag but does not yet
    /// model GSP busy-state arbitration.
    pub fail_if_busy: bool,
    /// Seven command-specific parameter words.
    pub arguments: [u32; 7],
}

impl GspGxCommand {
    fn parse(bytes: &[u8]) -> Self {
        let command_id = bytes[0];
        let stop_after = bytes[2] & GSP_COMMAND_FLAG_STOP_AFTER != 0;
        let fail_if_busy = bytes[3] & GSP_COMMAND_FLAG_FAIL_IF_BUSY != 0;
        let mut arguments = [0u32; 7];
        for (index, argument) in arguments.iter_mut().enumerate() {
            let offset = 4 + index * 4;
            *argument = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
        }
        Self {
            command_id,
            stop_after,
            fail_if_busy,
            arguments,
        }
    }

    /// ProcessCommandList's documented command fields.
    pub const fn process_command_list(self) -> Option<GspProcessCommandList> {
        if self.command_id != GSP_GX_COMMAND_PROCESS_COMMAND_LIST {
            return None;
        }
        Some(GspProcessCommandList {
            buffer_address: self.arguments[0],
            buffer_size_bytes: self.arguments[1],
            update_gas_blend: self.arguments[2] != 0,
            flush_buffer: self.arguments[6] != 0,
        })
    }

    /// MemoryFill's two start/value/end triples and packed control halfwords.
    pub const fn memory_fill(self) -> Option<GspMemoryFill> {
        if self.command_id != GSP_GX_COMMAND_MEMORY_FILL {
            return None;
        }
        Some(GspMemoryFill {
            buffers: [
                GspMemoryFillBuffer {
                    start_address: self.arguments[0],
                    fill_value: self.arguments[1],
                    end_address: self.arguments[2],
                    control: self.arguments[6] as u16,
                },
                GspMemoryFillBuffer {
                    start_address: self.arguments[3],
                    fill_value: self.arguments[4],
                    end_address: self.arguments[5],
                    control: (self.arguments[6] >> 16) as u16,
                },
            ],
        })
    }
}

/// Decoded arguments for GX command ID 0x01.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspProcessCommandList {
    pub buffer_address: u32,
    pub buffer_size_bytes: u32,
    pub update_gas_blend: bool,
    pub flush_buffer: bool,
}

/// One of the two optional buffers written by a GX MemoryFill command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspMemoryFillBuffer {
    pub start_address: u32,
    pub fill_value: u32,
    pub end_address: u32,
    pub control: u16,
}

impl GspMemoryFillBuffer {
    /// Bit 0 requests a fill; bits 8-9 select 16-, 24- or 32-bit elements.
    pub const fn is_triggered(self) -> bool {
        self.control & 1 != 0
    }

    pub const fn element_width_bytes(self) -> Option<usize> {
        match (self.control >> 8) & 3 {
            0 => Some(2),
            1 | 3 => Some(3),
            2 => Some(4),
            _ => None,
        }
    }
}

/// Decoded arguments for GX command ID 0x02.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspMemoryFill {
    pub buffers: [GspMemoryFillBuffer; 2],
}

/// Queue decoding or state-transition error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GspGxCommandQueueError {
    InvalidQueueSize {
        actual: usize,
        minimum: usize,
    },
    InvalidClientId {
        client_id: u8,
    },
    InvalidCommandIndex {
        index: u8,
    },
    TooManyPendingCommands {
        count: u8,
    },
    UnsupportedCommand {
        command_id: u8,
    },
    InvalidCommandListAddress {
        address: u32,
        size_bytes: u32,
    },
    InvalidCommandListSize {
        size_bytes: u32,
    },
    InvalidMemoryFillRange {
        start_address: u32,
        end_address: u32,
    },
    UnsupportedMemoryFillWidth {
        control: u16,
    },
    CommandListTooLarge {
        size_bytes: u32,
        maximum_bytes: usize,
    },
    CommandListSubmission {
        reason: String,
    },
    ProcessMemory(String),
    AddressOverflow {
        base: u32,
        offset: u64,
    },
}

impl fmt::Display for GspGxCommandQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GspGxCommandQueueError {}

/// Snapshot of a GSP GX ring, with pending entries in consumption order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GspGxCommandQueue {
    command_index: u8,
    command_count: u8,
    status: u8,
    flags: u8,
    last_result: u32,
    pending: Vec<GspGxCommand>,
}

impl GspGxCommandQueue {
    /// Parses the complete 0x200-byte client queue slot.
    pub fn parse(bytes: &[u8]) -> Result<Self, GspGxCommandQueueError> {
        if bytes.len() < GSP_GX_COMMAND_QUEUE_SIZE {
            return Err(GspGxCommandQueueError::InvalidQueueSize {
                actual: bytes.len(),
                minimum: GSP_GX_COMMAND_QUEUE_SIZE,
            });
        }

        let command_index = bytes[0];
        let command_count = bytes[1];
        if command_index >= GSP_GX_COMMAND_QUEUE_CAPACITY {
            return Err(GspGxCommandQueueError::InvalidCommandIndex {
                index: command_index,
            });
        }
        if command_count > GSP_GX_COMMAND_QUEUE_CAPACITY {
            return Err(GspGxCommandQueueError::TooManyPendingCommands {
                count: command_count,
            });
        }

        let status = bytes[2];
        let flags = bytes[3];
        let last_result = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let mut pending = Vec::with_capacity(usize::from(command_count));

        for queue_offset in 0..command_count {
            let slot = (usize::from(command_index) + usize::from(queue_offset))
                % usize::from(GSP_GX_COMMAND_QUEUE_CAPACITY);
            let start = GSP_GX_COMMAND_QUEUE_HEADER_SIZE + slot * GSP_GX_COMMAND_ENTRY_SIZE;
            let end = start + GSP_GX_COMMAND_ENTRY_SIZE;
            pending.push(GspGxCommand::parse(&bytes[start..end]));
        }

        Ok(Self {
            command_index,
            command_count,
            status,
            flags,
            last_result,
            pending,
        })
    }

    pub const fn command_index(&self) -> u8 {
        self.command_index
    }

    pub const fn command_count(&self) -> u8 {
        self.command_count
    }

    pub const fn status(&self) -> u8 {
        self.status
    }

    pub const fn flags(&self) -> u8 {
        self.flags
    }

    pub const fn last_result(&self) -> u32 {
        self.last_result
    }

    pub const fn is_halted(&self) -> bool {
        self.status & GSP_QUEUE_STATUS_HALTED != 0
            || self.flags & GSP_QUEUE_FLAG_HALT_UNTIL_TRIGGER != 0
    }

    pub fn next_command(&self) -> Option<GspGxCommand> {
        self.pending.first().copied()
    }

    /// Marks the next entry consumed. GSP advances index and decrements count
    /// before dispatching the command, so the model follows that ordering.
    pub fn consume_next(&mut self) -> Option<GspGxCommand> {
        let command = self.pending.first().copied()?;
        self.pending.remove(0);
        self.command_index = (self.command_index + 1) % GSP_GX_COMMAND_QUEUE_CAPACITY;
        self.command_count = self.command_count.saturating_sub(1);
        Some(command)
    }

    /// Sets the queue's halted flag and the halt-until-trigger latch.
    pub fn halt(&mut self) {
        self.status |= GSP_QUEUE_STATUS_HALTED;
        self.flags |= GSP_QUEUE_FLAG_HALT_UNTIL_TRIGGER;
    }

    /// Records a queue error. None preserves the previous result value when
    /// the failure does not have an established GSP result code yet.
    pub fn mark_error(&mut self, result: Option<u32>) {
        self.status |= GSP_QUEUE_STATUS_ERROR;
        if let Some(result) = result {
            self.last_result = result;
        }
    }

    /// Clears the halt latch when the guest triggers the GX command queue.
    ///
    /// The error bit and last result are retained for guest-side diagnostics.
    pub fn reset_halt_for_trigger(&mut self) {
        self.status &= !GSP_QUEUE_STATUS_HALTED;
        self.flags &= !GSP_QUEUE_FLAG_HALT_UNTIL_TRIGGER;
    }

    /// Serializes only the eight-byte queue header for write-back to shared RAM.
    pub fn encode_header(&self) -> [u8; GSP_GX_COMMAND_QUEUE_HEADER_SIZE] {
        let mut header = [0u8; GSP_GX_COMMAND_QUEUE_HEADER_SIZE];
        header[0] = self.command_index;
        header[1] = self.command_count;
        header[2] = self.status;
        header[3] = self.flags;
        header[4..8].copy_from_slice(&self.last_result.to_le_bytes());
        header
    }

    /// Returns the byte offset of a client's queue in shared memory.
    pub fn client_queue_offset(client_id: u8) -> Result<u32, GspGxCommandQueueError> {
        if client_id >= GSP_GX_COMMAND_QUEUE_CLIENT_COUNT {
            return Err(GspGxCommandQueueError::InvalidClientId { client_id });
        }
        Ok(GSP_GX_COMMAND_QUEUE_OFFSET + u32::from(client_id) * GSP_GX_COMMAND_QUEUE_CLIENT_STRIDE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_command(
        bytes: &mut [u8],
        slot: usize,
        command_id: u8,
        stop_after: bool,
        fail_if_busy: bool,
        arguments: [u32; 7],
    ) {
        let start = GSP_GX_COMMAND_QUEUE_HEADER_SIZE + slot * GSP_GX_COMMAND_ENTRY_SIZE;
        bytes[start] = command_id;
        bytes[start + 2] = u8::from(stop_after);
        bytes[start + 3] = u8::from(fail_if_busy);
        for (index, argument) in arguments.into_iter().enumerate() {
            let offset = start + 4 + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&argument.to_le_bytes());
        }
    }

    #[test]
    fn parses_process_command_list_and_preserves_queue_metadata() {
        let mut bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        bytes[0] = 2;
        bytes[1] = 1;
        bytes[4..8].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        write_command(
            &mut bytes,
            2,
            GSP_GX_COMMAND_PROCESS_COMMAND_LIST,
            true,
            false,
            [0x1400_4000, 0x200, 1, 0, 0, 0, 1],
        );

        let queue = GspGxCommandQueue::parse(&bytes).unwrap();
        let command = queue.next_command().unwrap();
        assert_eq!(command.command_id, GSP_GX_COMMAND_PROCESS_COMMAND_LIST);
        assert!(command.stop_after);
        assert!(!command.fail_if_busy);
        assert_eq!(
            command.process_command_list(),
            Some(GspProcessCommandList {
                buffer_address: 0x1400_4000,
                buffer_size_bytes: 0x200,
                update_gas_blend: true,
                flush_buffer: true,
            })
        );
        assert_eq!(queue.last_result(), 0x1234_5678);
    }

    #[test]
    fn ring_parser_wraps_across_the_final_slot() {
        let mut bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        bytes[0] = 14;
        bytes[1] = 2;
        write_command(&mut bytes, 14, 1, false, false, [1, 2, 3, 4, 5, 6, 7]);
        write_command(&mut bytes, 0, 2, false, false, [8, 9, 10, 11, 12, 13, 14]);

        let mut queue = GspGxCommandQueue::parse(&bytes).unwrap();
        assert_eq!(queue.next_command().unwrap().command_id, 1);
        assert_eq!(queue.consume_next().unwrap().command_id, 1);
        assert_eq!(queue.command_index(), 0);
        assert_eq!(queue.command_count(), 1);
        assert_eq!(queue.next_command().unwrap().command_id, 2);
        assert_eq!(queue.consume_next().unwrap().command_id, 2);
        assert_eq!(queue.command_index(), 1);
        assert_eq!(queue.command_count(), 0);
        assert_eq!(&queue.encode_header()[0..2], &[1, 0]);
    }

    #[test]
    fn queue_halt_and_error_flags_are_encoded_independently() {
        let bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        let mut queue = GspGxCommandQueue::parse(&bytes).unwrap();
        queue.halt();
        assert!(queue.is_halted());
        queue.mark_error(Some(GSP_INVALID_ADDRESS_RESULT));
        let header = queue.encode_header();
        assert_eq!(header[2], GSP_QUEUE_STATUS_HALTED | GSP_QUEUE_STATUS_ERROR);
        assert_eq!(header[3], GSP_QUEUE_FLAG_HALT_UNTIL_TRIGGER);
        assert_eq!(
            u32::from_le_bytes(header[4..8].try_into().unwrap()),
            GSP_INVALID_ADDRESS_RESULT
        );
        queue.reset_halt_for_trigger();
        assert!(!queue.is_halted());
        assert_eq!(queue.status(), GSP_QUEUE_STATUS_ERROR);
    }

    #[test]
    fn rejects_invalid_queue_sizes_counts_indices_and_client_ids() {
        assert!(matches!(
            GspGxCommandQueue::parse(&[0; GSP_GX_COMMAND_QUEUE_SIZE - 1]),
            Err(GspGxCommandQueueError::InvalidQueueSize { .. })
        ));

        let mut bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        bytes[0] = GSP_GX_COMMAND_QUEUE_CAPACITY;
        assert!(matches!(
            GspGxCommandQueue::parse(&bytes),
            Err(GspGxCommandQueueError::InvalidCommandIndex { .. })
        ));

        bytes[0] = 0;
        bytes[1] = GSP_GX_COMMAND_QUEUE_CAPACITY + 1;
        assert!(matches!(
            GspGxCommandQueue::parse(&bytes),
            Err(GspGxCommandQueueError::TooManyPendingCommands { .. })
        ));
        assert_eq!(
            GspGxCommandQueue::client_queue_offset(3).unwrap(),
            GSP_GX_COMMAND_QUEUE_OFFSET + 3 * GSP_GX_COMMAND_QUEUE_CLIENT_STRIDE
        );
        assert!(matches!(
            GspGxCommandQueue::client_queue_offset(4),
            Err(GspGxCommandQueueError::InvalidClientId { .. })
        ));
    }
}
