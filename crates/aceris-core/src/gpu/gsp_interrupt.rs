//! GSP per-client interrupt relay queue shared-memory model.
//!
//! The queue occupies one 0x40-byte block per GSP client. It records interrupt
//! identifiers in a 0x34-entry ring, tracks missed PDC0/PDC1 events separately,
//! and preserves the guest-configured ignore-PDC bit. Event-handle signaling,
//! client registration, and routing by GPU rendering rights belong to the
//! service/session layer and are intentionally not inferred here.

use std::fmt;

/// Number of GSP clients supported by the shared-memory relay block.
pub const GSP_INTERRUPT_QUEUE_CLIENT_COUNT: u8 = 4;
/// Stride and total size of one interrupt relay queue.
pub const GSP_INTERRUPT_QUEUE_CLIENT_STRIDE: u32 = 0x40;
pub const GSP_INTERRUPT_QUEUE_SIZE: usize = 0x40;
/// Number of interrupt identifiers stored in the relay ring.
pub const GSP_INTERRUPT_QUEUE_CAPACITY: usize = 0x34;
/// PDC interrupts stop entering the ring at this count and are accumulated.
pub const GSP_INTERRUPT_QUEUE_PDC_THRESHOLD: u8 = 0x20;
/// Error code written when a non-PDC interrupt arrives at a full ring.
pub const GSP_INTERRUPT_QUEUE_FULL_ERROR: u8 = 0x01;
/// Interrupt relay queue configuration bit that suppresses PDC0/PDC1.
pub const GSP_INTERRUPT_QUEUE_IGNORE_PDC: u8 = 1;

/// GSP interrupt identifiers stored in the relay queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GspInterruptId {
    Psc0 = 0,
    Psc1 = 1,
    Pdc0 = 2,
    Pdc1 = 3,
    Ppf = 4,
    P3d = 5,
    Dma = 6,
}

impl GspInterruptId {
    pub const fn raw(self) -> u8 {
        self as u8
    }

    pub const fn is_pdc(self) -> bool {
        matches!(self, Self::Pdc0 | Self::Pdc1)
    }

    pub const fn is_valid(raw: u8) -> bool {
        raw <= Self::Dma as u8
    }
}

impl TryFrom<u8> for GspInterruptId {
    type Error = GspInterruptQueueError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Psc0),
            1 => Ok(Self::Psc1),
            2 => Ok(Self::Pdc0),
            3 => Ok(Self::Pdc1),
            4 => Ok(Self::Ppf),
            5 => Ok(Self::P3d),
            6 => Ok(Self::Dma),
            _ => Err(GspInterruptQueueError::InvalidInterruptId { value }),
        }
    }
}

/// Errors while reading a relay queue from guest shared memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GspInterruptQueueError {
    InvalidQueueSize { actual: usize, minimum: usize },
    InvalidClientId { client_id: u8 },
    InvalidIndex { index: u8 },
    InvalidCount { count: u8 },
    InvalidInterruptId { value: u8 },
}

impl fmt::Display for GspInterruptQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GspInterruptQueueError {}

/// Result of attempting to append one interrupt to a relay queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GspInterruptQueuePushResult {
    /// The event was stored and the caller should signal the registered event.
    Queued { slot: u8 },
    /// PDC was disabled by the guest's ignore-PDC configuration flag.
    IgnoredPdc,
    /// The event was not stored; the corresponding missed-PDC counter increased.
    MissedPdc { interrupt: GspInterruptId, total: u32 },
    /// A non-PDC event arrived while all ring slots were occupied.
    Full,
}

/// Guest-visible GSP relay ring for one registered client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GspInterruptRelayQueue {
    index: u8,
    number_interrupts: u8,
    error_code: u8,
    config: u8,
    missed_pdc0: u32,
    missed_pdc1: u32,
    slots: [u8; GSP_INTERRUPT_QUEUE_CAPACITY],
}

impl Default for GspInterruptRelayQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl GspInterruptRelayQueue {
    /// Creates the cleared queue installed by RegisterInterruptRelayQueue.
    pub const fn new() -> Self {
        Self {
            index: 0,
            number_interrupts: 0,
            error_code: 0,
            config: 0,
            missed_pdc0: 0,
            missed_pdc1: 0,
            slots: [0; GSP_INTERRUPT_QUEUE_CAPACITY],
        }
    }

    /// Parses one 0x40-byte interrupt relay queue from shared memory.
    ///
    /// Only the header is validated here; inactive ring slots are opaque bytes
    /// owned by the guest and are preserved when the queue is encoded again.
    pub fn parse(bytes: &[u8]) -> Result<Self, GspInterruptQueueError> {
        if bytes.len() < GSP_INTERRUPT_QUEUE_SIZE {
            return Err(GspInterruptQueueError::InvalidQueueSize {
                actual: bytes.len(),
                minimum: GSP_INTERRUPT_QUEUE_SIZE,
            });
        }
        let index = bytes[0];
        if usize::from(index) >= GSP_INTERRUPT_QUEUE_CAPACITY {
            return Err(GspInterruptQueueError::InvalidIndex { index });
        }
        let number_interrupts = bytes[1];
        if usize::from(number_interrupts) > GSP_INTERRUPT_QUEUE_CAPACITY {
            return Err(GspInterruptQueueError::InvalidCount {
                count: number_interrupts,
            });
        }
        let mut slots = [0u8; GSP_INTERRUPT_QUEUE_CAPACITY];
        slots.copy_from_slice(&bytes[12..GSP_INTERRUPT_QUEUE_SIZE]);
        Ok(Self {
            index,
            number_interrupts,
            error_code: bytes[2],
            config: bytes[3],
            missed_pdc0: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            missed_pdc1: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            slots,
        })
    }

    pub const fn index(&self) -> u8 {
        self.index
    }

    pub const fn number_interrupts(&self) -> u8 {
        self.number_interrupts
    }

    pub const fn error_code(&self) -> u8 {
        self.error_code
    }

    pub const fn config(&self) -> u8 {
        self.config
    }

    pub const fn ignores_pdc(&self) -> bool {
        self.config & GSP_INTERRUPT_QUEUE_IGNORE_PDC != 0
    }

    pub const fn missed_pdc0(&self) -> u32 {
        self.missed_pdc0
    }

    pub const fn missed_pdc1(&self) -> u32 {
        self.missed_pdc1
    }

    /// Changes only the guest-configured PDC suppression bit.
    pub fn set_ignores_pdc(&mut self, ignored: bool) {
        if ignored {
            self.config |= GSP_INTERRUPT_QUEUE_IGNORE_PDC;
        } else {
            self.config &= !GSP_INTERRUPT_QUEUE_IGNORE_PDC;
        }
    }

    /// Appends one event using the ring's current read index and pending count.
    ///
    /// Successful insertion returns the slot to write back and tells the
    /// service layer to signal the registered event handle. PDC events follow
    /// their lower queue threshold and missed counters; generic overflow sets
    /// the documented queue-full error code.
    pub fn push(&mut self, interrupt: GspInterruptId) -> GspInterruptQueuePushResult {
        if interrupt.is_pdc() {
            if self.ignores_pdc() {
                return GspInterruptQueuePushResult::IgnoredPdc;
            }
            if self.number_interrupts >= GSP_INTERRUPT_QUEUE_PDC_THRESHOLD {
                let counter = match interrupt {
                    GspInterruptId::Pdc0 => &mut self.missed_pdc0,
                    GspInterruptId::Pdc1 => &mut self.missed_pdc1,
                    _ => unreachable!(),
                };
                *counter = counter.wrapping_add(1);
                return GspInterruptQueuePushResult::MissedPdc {
                    interrupt,
                    total: *counter,
                };
            }
        }

        if usize::from(self.number_interrupts) >= GSP_INTERRUPT_QUEUE_CAPACITY {
            self.error_code = GSP_INTERRUPT_QUEUE_FULL_ERROR;
            return GspInterruptQueuePushResult::Full;
        }

        let slot = (usize::from(self.index) + usize::from(self.number_interrupts))
            % GSP_INTERRUPT_QUEUE_CAPACITY;
        self.slots[slot] = interrupt.raw();
        self.number_interrupts += 1;
        GspInterruptQueuePushResult::Queued { slot: slot as u8 }
    }

    /// Returns the queued identifiers in guest consumption order.
    pub fn pending_interrupts(&self) -> Result<Vec<GspInterruptId>, GspInterruptQueueError> {
        (0..self.number_interrupts)
            .map(|offset| {
                let slot = (usize::from(self.index) + usize::from(offset))
                    % GSP_INTERRUPT_QUEUE_CAPACITY;
                GspInterruptId::try_from(self.slots[slot])
            })
            .collect()
    }

    /// Serializes the exact 0x40-byte guest-visible layout.
    pub fn encode(&self) -> [u8; GSP_INTERRUPT_QUEUE_SIZE] {
        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        bytes[0] = self.index;
        bytes[1] = self.number_interrupts;
        bytes[2] = self.error_code;
        bytes[3] = self.config;
        bytes[4..8].copy_from_slice(&self.missed_pdc0.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.missed_pdc1.to_le_bytes());
        bytes[12..].copy_from_slice(&self.slots);
        bytes
    }

    /// Returns the address of one client's relay queue within GSP shared memory.
    pub fn client_queue_offset(client_id: u8) -> Result<u32, GspInterruptQueueError> {
        if client_id >= GSP_INTERRUPT_QUEUE_CLIENT_COUNT {
            return Err(GspInterruptQueueError::InvalidClientId { client_id });
        }
        Ok(u32::from(client_id) * GSP_INTERRUPT_QUEUE_CLIENT_STRIDE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_layout_matches_the_0x40_byte_relay_block() {
        let queue = GspInterruptRelayQueue::new();
        let bytes = queue.encode();
        assert_eq!(bytes.len(), 0x40);
        assert_eq!(&bytes[..12], &[0; 12]);
        assert!(bytes[12..].iter().all(|byte| *byte == 0));
        assert_eq!(GspInterruptRelayQueue::parse(&bytes).unwrap(), queue);
        assert_eq!(GspInterruptRelayQueue::client_queue_offset(0).unwrap(), 0);
        assert_eq!(GspInterruptRelayQueue::client_queue_offset(3).unwrap(), 0xC0);
        assert!(matches!(
            GspInterruptRelayQueue::client_queue_offset(4),
            Err(GspInterruptQueueError::InvalidClientId { client_id: 4 })
        ));
    }

    #[test]
    fn parser_reads_exact_queue_extent_from_a_larger_shared_block() {
        let mut shared_block = [0xCC; GSP_INTERRUPT_QUEUE_SIZE + 0x100];
        shared_block[0] = 3;
        shared_block[1] = 0;
        shared_block[12 + 3] = GspInterruptId::Ppf.raw();
        let queue = GspInterruptRelayQueue::parse(&shared_block).unwrap();
        assert_eq!(queue.encode().len(), GSP_INTERRUPT_QUEUE_SIZE);
        assert_eq!(queue.encode()[12 + 3], GspInterruptId::Ppf.raw());
    }

    #[test]
    fn appends_events_in_ring_order_and_wraps_at_slot_0x33() {
        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        bytes[0] = 0x33;
        bytes[1] = 1;
        bytes[12 + 0x33] = GspInterruptId::P3d.raw();
        let mut queue = GspInterruptRelayQueue::parse(&bytes).unwrap();

        assert_eq!(
            queue.push(GspInterruptId::Dma),
            GspInterruptQueuePushResult::Queued { slot: 0 }
        );
        assert_eq!(
            queue.pending_interrupts().unwrap(),
            [GspInterruptId::P3d, GspInterruptId::Dma]
        );
        let encoded = queue.encode();
        assert_eq!(encoded[12 + 0x33], GspInterruptId::P3d.raw());
        assert_eq!(encoded[12], GspInterruptId::Dma.raw());
        assert_eq!(encoded[1], 2);
    }

    #[test]
    fn pdc_events_stop_at_0x20_and_increment_independent_missed_counters() {
        let mut queue = GspInterruptRelayQueue::new();
        for index in 0..GSP_INTERRUPT_QUEUE_PDC_THRESHOLD {
            let interrupt = if index % 2 == 0 {
                GspInterruptId::Pdc0
            } else {
                GspInterruptId::Pdc1
            };
            assert!(matches!(
                queue.push(interrupt),
                GspInterruptQueuePushResult::Queued { .. }
            ));
        }
        assert_eq!(
            queue.push(GspInterruptId::Pdc0),
            GspInterruptQueuePushResult::MissedPdc {
                interrupt: GspInterruptId::Pdc0,
                total: 1,
            }
        );
        assert_eq!(
            queue.push(GspInterruptId::Pdc1),
            GspInterruptQueuePushResult::MissedPdc {
                interrupt: GspInterruptId::Pdc1,
                total: 1,
            }
        );
        assert_eq!(queue.number_interrupts(), GSP_INTERRUPT_QUEUE_PDC_THRESHOLD);
        assert_eq!(queue.missed_pdc0(), 1);
        assert_eq!(queue.missed_pdc1(), 1);
    }

    #[test]
    fn ignore_pdc_drops_events_without_changing_ring_or_missed_counters() {
        let mut queue = GspInterruptRelayQueue::new();
        queue.set_ignores_pdc(true);
        assert_eq!(
            queue.push(GspInterruptId::Pdc0),
            GspInterruptQueuePushResult::IgnoredPdc
        );
        assert_eq!(
            queue.push(GspInterruptId::Pdc1),
            GspInterruptQueuePushResult::IgnoredPdc
        );
        assert_eq!(queue.number_interrupts(), 0);
        assert_eq!(queue.missed_pdc0(), 0);
        assert_eq!(queue.missed_pdc1(), 0);
        assert_eq!(queue.encode()[3] & GSP_INTERRUPT_QUEUE_IGNORE_PDC, 1);
    }

    #[test]
    fn non_pdc_events_can_fill_all_slots_then_set_queue_full_error() {
        let mut queue = GspInterruptRelayQueue::new();
        for _ in 0..GSP_INTERRUPT_QUEUE_CAPACITY {
            assert!(matches!(
                queue.push(GspInterruptId::P3d),
                GspInterruptQueuePushResult::Queued { .. }
            ));
        }
        assert_eq!(queue.number_interrupts(), GSP_INTERRUPT_QUEUE_CAPACITY as u8);
        assert_eq!(queue.push(GspInterruptId::Dma), GspInterruptQueuePushResult::Full);
        assert_eq!(queue.number_interrupts(), GSP_INTERRUPT_QUEUE_CAPACITY as u8);
        assert_eq!(queue.error_code(), GSP_INTERRUPT_QUEUE_FULL_ERROR);
    }

    #[test]
    fn parses_header_bounds_and_preserves_inactive_slots() {
        assert!(matches!(
            GspInterruptRelayQueue::parse(&[0; GSP_INTERRUPT_QUEUE_SIZE - 1]),
            Err(GspInterruptQueueError::InvalidQueueSize { .. })
        ));
        let mut bytes = [0xA5; GSP_INTERRUPT_QUEUE_SIZE];
        bytes[0] = 0;
        bytes[1] = 0;
        assert!(matches!(
            GspInterruptRelayQueue::parse(&bytes),
            Ok(_)
        ));
        bytes[0] = GSP_INTERRUPT_QUEUE_CAPACITY as u8;
        assert!(matches!(
            GspInterruptRelayQueue::parse(&bytes),
            Err(GspInterruptQueueError::InvalidIndex { .. })
        ));
        bytes[0] = 0;
        bytes[1] = GSP_INTERRUPT_QUEUE_CAPACITY as u8 + 1;
        assert!(matches!(
            GspInterruptRelayQueue::parse(&bytes),
            Err(GspInterruptQueueError::InvalidCount { .. })
        ));
        bytes[1] = 0;
        let queue = GspInterruptRelayQueue::parse(&bytes).unwrap();
        assert_eq!(queue.encode()[12 + 5], 0xA5);
    }
}
