//! HID shared-memory image published to the guest through `hid:USER`.
//!
//! The 0x2B0-byte layout (PAD, touch, accelerometer, gyroscope, debug pad
//! regions, 8-entry rings with a last-updated index) follows public 3dbrew
//! documentation and libctru's reading convention. It has NOT been verified on
//! hardware. Accelerometer, gyroscope and debug pad regions stay zeroed; the
//! 3D slider, GPIO bits 12-13 and raw circle-pad/touch fields are not modelled.

use crate::input::InputSnapshot;

/// Size of the documented HID shared-memory layout.
pub const HID_SHARED_MEMORY_LAYOUT_SIZE: usize = 0x2B0;
/// Size of the memory block created for the guest (one page).
pub const HID_SHARED_MEMORY_BLOCK_SIZE: u32 = 0x1000;

const PAD_BASE: usize = 0x000;
const PAD_ENTRIES: usize = 0x028;
const TOUCH_BASE: usize = 0x0A8;
const TOUCH_ENTRIES: usize = 0x0C8;
const RING_LEN: u32 = 8;

/// Circle-pad deflection that sets the direction bits 28-31.
pub const CIRCLE_PAD_DIRECTION_THRESHOLD: i16 = 41;
const CIRCLE_RIGHT: u32 = 1 << 28;
const CIRCLE_LEFT: u32 = 1 << 29;
const CIRCLE_UP: u32 = 1 << 30;
const CIRCLE_DOWN: u32 = 1 << 31;

/// Event handles returned by `GetIPCHandles`, in documented order.
pub const HID_EVENT_COUNT: usize = 5;
/// Indices into the event-handle array.
pub mod event {
    pub const PAD0: usize = 0;
    pub const PAD1: usize = 1;
    pub const ACCELEROMETER: usize = 2;
    pub const GYROSCOPE: usize = 3;
    pub const DEBUG_PAD: usize = 4;
}

/// In-memory copy of the HID shared memory plus ring bookkeeping.
#[derive(Clone, Debug)]
pub struct HidSharedMemory {
    image: [u8; HID_SHARED_MEMORY_LAYOUT_SIZE],
    pad_index: u32,
    touch_index: u32,
    pad_tick: u64,
    touch_tick: u64,
    updated_once: bool,
}

impl Default for HidSharedMemory {
    fn default() -> Self {
        Self::new()
    }
}

impl HidSharedMemory {
    pub fn new() -> Self {
        Self {
            image: [0; HID_SHARED_MEMORY_LAYOUT_SIZE],
            pad_index: 0,
            touch_index: 0,
            pad_tick: 0,
            touch_tick: 0,
            updated_once: false,
        }
    }

    /// The current image, ready to copy into the shared block.
    pub fn image(&self) -> &[u8] {
        &self.image
    }

    /// Advances the PAD and touch rings by one entry from a sampled snapshot.
    pub fn update(&mut self, snapshot: &InputSnapshot, tick: u64) {
        // The first update writes entry 0; later updates advance the ring.
        let next_pad = if self.updated_once {
            (self.pad_index + 1) % RING_LEN
        } else {
            0
        };
        let next_touch = if self.updated_once {
            (self.touch_index + 1) % RING_LEN
        } else {
            0
        };
        self.updated_once = true;

        let mut held = snapshot.held;
        let mut pressed = snapshot.pressed;
        let mut released = snapshot.released;
        let (cx, cy) = snapshot.circle_pad;
        let mut direction = 0;
        if cx >= CIRCLE_PAD_DIRECTION_THRESHOLD {
            direction |= CIRCLE_RIGHT;
        }
        if cx <= -CIRCLE_PAD_DIRECTION_THRESHOLD {
            direction |= CIRCLE_LEFT;
        }
        if cy >= CIRCLE_PAD_DIRECTION_THRESHOLD {
            direction |= CIRCLE_UP;
        }
        if cy <= -CIRCLE_PAD_DIRECTION_THRESHOLD {
            direction |= CIRCLE_DOWN;
        }
        // ZL/ZR (bits 14-15) exist only on New 3DS and are not in this layout.
        let mask = !((1u32 << 14) | (1 << 15));
        held &= mask;
        pressed &= mask;
        released &= mask;
        // Direction bits appear in held/pressed according to the previous entry.
        let previous_direction = self.read_u32(PAD_ENTRIES + self.pad_index as usize * 0x10)
            & (CIRCLE_RIGHT | CIRCLE_LEFT | CIRCLE_UP | CIRCLE_DOWN);
        let held = held | direction;
        pressed |= direction & !previous_direction;
        released |= previous_direction & !direction;

        // Header: previous tick, latest tick, index, current state.
        let previous_tick = self.pad_tick;
        self.write_u64(PAD_BASE + 0x08, previous_tick);
        self.write_u64(PAD_BASE, tick);
        self.write_u32(PAD_BASE + 0x10, next_pad);
        self.write_u32(PAD_BASE + 0x1C, held & 0x0FFF_FFFF);
        let entry = PAD_ENTRIES + next_pad as usize * 0x10;
        self.write_u32(entry, held);
        self.write_u32(entry + 4, pressed);
        self.write_u32(entry + 8, released);
        self.write_u16(entry + 0xC, cx as u16);
        self.write_u16(entry + 0xE, cy as u16);
        self.pad_index = next_pad;
        self.pad_tick = tick;

        let previous_touch_tick = self.touch_tick;
        self.write_u64(TOUCH_BASE + 0x08, previous_touch_tick);
        self.write_u64(TOUCH_BASE, tick);
        self.write_u32(TOUCH_BASE + 0x10, next_touch);
        let touch_entry = TOUCH_ENTRIES + next_touch as usize * 8;
        match snapshot.touch {
            Some((x, y)) => {
                self.write_u16(touch_entry, x);
                self.write_u16(touch_entry + 2, y);
                self.image[touch_entry + 4] = 1;
            }
            None => {
                self.write_u16(touch_entry, 0);
                self.write_u16(touch_entry + 2, 0);
                self.image[touch_entry + 4] = 0;
            }
        }
        self.touch_index = next_touch;
        self.touch_tick = tick;
    }

    fn read_u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.image[offset..offset + 4].try_into().expect("4 bytes"))
    }

    fn write_u16(&mut self, offset: usize, value: u16) {
        self.image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(&mut self, offset: usize, value: u32) {
        self.image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(&mut self, offset: usize, value: u64) {
        self.image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputState, pad};

    fn u32_at(hid: &HidSharedMemory, offset: usize) -> u32 {
        hid.read_u32(offset)
    }

    #[test]
    fn first_update_writes_entry_zero_then_ring_advances_and_wraps() {
        let mut hid = HidSharedMemory::new();
        let mut input = InputState::default();
        input.press(pad::A);
        hid.update(&input.snapshot(), 100);
        assert_eq!(u32_at(&hid, 0x10), 0);
        assert_eq!(u32_at(&hid, 0x28), pad::A);
        assert_eq!(u32_at(&hid, 0x2C), pad::A);
        assert_eq!(u32_at(&hid, 0x1C), pad::A);
        for tick in 1..=8 {
            hid.update(&input.snapshot(), 100 + tick);
        }
        // 9 further updates after the first: index 8 % 8 == 0 again.
        assert_eq!(u32_at(&hid, 0x10), 0);
        // Entry 1 was written by the second update: held, no new edge.
        assert_eq!(u32_at(&hid, 0x38), pad::A);
        assert_eq!(u32_at(&hid, 0x3C), 0);
    }

    #[test]
    fn ticks_record_latest_and_previous() {
        let mut hid = HidSharedMemory::new();
        let snap = InputSnapshot::default();
        hid.update(&snap, 1000);
        hid.update(&snap, 2000);
        assert_eq!(hid.read_u32(0x00), 2000);
        assert_eq!(hid.read_u32(0x08), 1000);
        assert_eq!(hid.read_u32(0xA8), 2000);
        assert_eq!(hid.read_u32(0xB0), 1000);
    }

    #[test]
    fn release_edge_and_circle_pad_direction_bits() {
        let mut hid = HidSharedMemory::new();
        let mut input = InputState::default();
        input.press(pad::B);
        input.set_circle_pad(100, -50);
        hid.update(&input.snapshot(), 1);
        let held = u32_at(&hid, 0x28);
        assert_eq!(held & pad::B, pad::B);
        assert_ne!(held & CIRCLE_RIGHT, 0);
        assert_ne!(held & CIRCLE_DOWN, 0);
        assert_eq!(held & (CIRCLE_LEFT | CIRCLE_UP), 0);
        assert_eq!(u32_at(&hid, 0x2C) & CIRCLE_RIGHT, CIRCLE_RIGHT);
        let cx = i16::from_le_bytes([hid.image()[0x34], hid.image()[0x35]]);
        let cy = i16::from_le_bytes([hid.image()[0x36], hid.image()[0x37]]);
        assert_eq!((cx, cy), (100, -50));
        // Centre the stick and release B.
        input.release(pad::B);
        input.set_circle_pad(0, 0);
        hid.update(&input.snapshot(), 2);
        assert_eq!(u32_at(&hid, 0x38), 0);
        assert_eq!(u32_at(&hid, 0x40) & pad::B, pad::B);
        assert_ne!(u32_at(&hid, 0x40) & CIRCLE_RIGHT, 0);
    }

    #[test]
    fn small_circle_pad_deflection_does_not_set_direction_bits() {
        let mut hid = HidSharedMemory::new();
        let mut input = InputState::default();
        input.set_circle_pad(40, -40);
        hid.update(&input.snapshot(), 1);
        assert_eq!(u32_at(&hid, 0x28), 0);
    }

    #[test]
    fn zl_zr_are_not_published() {
        let mut hid = HidSharedMemory::new();
        let mut input = InputState::default();
        input.press(pad::ZL | pad::ZR | pad::X);
        hid.update(&input.snapshot(), 1);
        assert_eq!(u32_at(&hid, 0x28), pad::X);
    }

    #[test]
    fn touch_entries_carry_coordinates_and_valid_flag() {
        let mut hid = HidSharedMemory::new();
        let mut input = InputState::default();
        input.touch(120, 200);
        hid.update(&input.snapshot(), 1);
        let entry = 0xC8;
        assert_eq!(
            u16::from_le_bytes([hid.image()[entry], hid.image()[entry + 1]]),
            120
        );
        assert_eq!(
            u16::from_le_bytes([hid.image()[entry + 2], hid.image()[entry + 3]]),
            200
        );
        assert_eq!(hid.image()[entry + 4], 1);
        input.touch_end();
        hid.update(&input.snapshot(), 2);
        assert_eq!(u32_at(&hid, 0xB8), 1);
        assert_eq!(hid.image()[0xC8 + 8 + 4], 0);
    }
}
