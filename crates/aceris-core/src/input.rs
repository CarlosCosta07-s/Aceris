//! Platform-independent controller and touchscreen state.
//!
//! Hosts (desktop windows, Android surfaces) translate their own events into
//! this model; the emulated system reads it through [`InputState::snapshot`].
//! The model contains no window-system code. Guest-visible delivery (the HID
//! shared memory and `hid:USER` service) lives in `crate::hid` and `System`.
//!
//! The button bit order, the circle-pad range and the touch resolution follow
//! public 3DS documentation and have not been verified on hardware.

use crate::video::frontend::{ScreenLayout, buttons as frontend_buttons};

/// Hardware-order pad button bits.
pub mod pad {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const SELECT: u32 = 1 << 2;
    pub const START: u32 = 1 << 3;
    pub const RIGHT: u32 = 1 << 4;
    pub const LEFT: u32 = 1 << 5;
    pub const UP: u32 = 1 << 6;
    pub const DOWN: u32 = 1 << 7;
    pub const R: u32 = 1 << 8;
    pub const L: u32 = 1 << 9;
    pub const X: u32 = 1 << 10;
    pub const Y: u32 = 1 << 11;
    pub const ZL: u32 = 1 << 14;
    pub const ZR: u32 = 1 << 15;
    /// Every bit this model can report.
    pub const ALL: u32 = ((1 << 12) - 1) | ZL | ZR;
}

/// Largest circle-pad deflection per axis.
pub const CIRCLE_PAD_LIMIT: i16 = 156;

/// Converts a physical analog-stick vector into the 3DS Circle Pad range.
///
/// A radial deadzone preserves the stick's direction near diagonals, unlike
/// applying the same threshold independently to X and Y. Inputs are sanitized
/// and clamped to the unit circle before the deadzone is normalized.
pub fn gamepad_axes_to_circle_pad(x: f32, y: f32, deadzone_percent: u8) -> (i16, i16) {
    let x = if x.is_finite() {
        x.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let y = if y.is_finite() {
        y.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let magnitude = x.hypot(y);
    let deadzone = f32::from(deadzone_percent.min(99)) / 100.0;

    if magnitude <= deadzone || magnitude == 0.0 {
        return (0, 0);
    }

    let normalized_magnitude = (magnitude.min(1.0) - deadzone) / (1.0 - deadzone);
    let scale = normalized_magnitude / magnitude;
    let limit = f32::from(CIRCLE_PAD_LIMIT);
    (
        (x * scale * limit).round() as i16,
        (y * scale * limit).round() as i16,
    )
}
/// Bottom-screen touch resolution.
pub const TOUCH_WIDTH: u16 = 320;
pub const TOUCH_HEIGHT: u16 = 240;

/// Converts the frontend's virtual-control mask to hardware pad bits.
pub fn pad_from_frontend_buttons(mask: u32) -> u32 {
    const MAP: [(u32, u32); 12] = [
        (frontend_buttons::UP, pad::UP),
        (frontend_buttons::DOWN, pad::DOWN),
        (frontend_buttons::LEFT, pad::LEFT),
        (frontend_buttons::RIGHT, pad::RIGHT),
        (frontend_buttons::A, pad::A),
        (frontend_buttons::B, pad::B),
        (frontend_buttons::X, pad::X),
        (frontend_buttons::Y, pad::Y),
        (frontend_buttons::L, pad::L),
        (frontend_buttons::R, pad::R),
        (frontend_buttons::START, pad::START),
        (frontend_buttons::SELECT, pad::SELECT),
    ];
    MAP.iter()
        .filter(|(source, _)| mask & source != 0)
        .fold(0, |bits, (_, target)| bits | target)
}

/// Maps a host-surface point to bottom-screen touch coordinates.
///
/// Returns `None` outside the bottom screen rectangle.
pub fn touch_from_surface_point(layout: &ScreenLayout, x: i32, y: i32) -> Option<(u16, u16)> {
    let rect = layout.bottom;
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let dx = i64::from(x) - i64::from(rect.x);
    let dy = i64::from(y) - i64::from(rect.y);
    if dx < 0 || dy < 0 || dx >= i64::from(rect.width) || dy >= i64::from(rect.height) {
        return None;
    }
    let tx = dx * i64::from(TOUCH_WIDTH) / i64::from(rect.width);
    let ty = dy * i64::from(TOUCH_HEIGHT) / i64::from(rect.height);
    Some((tx as u16, ty as u16))
}

/// Input as seen at one sampling instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputSnapshot {
    pub held: u32,
    /// Buttons that went down since the previous snapshot.
    pub pressed: u32,
    /// Buttons that went up since the previous snapshot.
    pub released: u32,
    pub circle_pad: (i16, i16),
    pub touch: Option<(u16, u16)>,
}

/// Mutable host-fed input state.
#[derive(Clone, Copy, Debug, Default)]
pub struct InputState {
    held: u32,
    last_sampled: u32,
    circle_pad: (i16, i16),
    touch: Option<(u16, u16)>,
}

impl InputState {
    /// Replaces the full set of held buttons (unknown bits are dropped).
    pub fn set_buttons(&mut self, mask: u32) {
        self.held = mask & pad::ALL;
    }

    pub fn press(&mut self, mask: u32) {
        self.held |= mask & pad::ALL;
    }

    pub fn release(&mut self, mask: u32) {
        self.held &= !mask;
    }

    /// Sets the circle pad, clamping each axis to the documented range.
    pub fn set_circle_pad(&mut self, x: i32, y: i32) {
        let clamp = |value: i32| {
            value.clamp(-i32::from(CIRCLE_PAD_LIMIT), i32::from(CIRCLE_PAD_LIMIT)) as i16
        };
        self.circle_pad = (clamp(x), clamp(y));
    }

    /// Starts or moves a touch; coordinates beyond the screen are clamped.
    pub fn touch(&mut self, x: i32, y: i32) {
        self.touch = Some((
            x.clamp(0, i32::from(TOUCH_WIDTH) - 1) as u16,
            y.clamp(0, i32::from(TOUCH_HEIGHT) - 1) as u16,
        ));
    }

    pub fn touch_end(&mut self) {
        self.touch = None;
    }

    /// Releases everything, for example when the host window loses focus.
    pub fn reset(&mut self) {
        *self = Self {
            last_sampled: self.last_sampled,
            ..Self::default()
        };
    }

    /// Currently held buttons, without sampling edges.
    pub fn snapshot_held(&self) -> u32 {
        self.held
    }

    /// Samples the state and computes button edges since the last sample.
    pub fn snapshot(&mut self) -> InputSnapshot {
        let changed = self.held ^ self.last_sampled;
        let snapshot = InputSnapshot {
            held: self.held,
            pressed: changed & self.held,
            released: changed & self.last_sampled,
            circle_pad: self.circle_pad,
            touch: self.touch,
        };
        self.last_sampled = self.held;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::frontend::{
        ORIENTATION_LANDSCAPE, ORIENTATION_PORTRAIT, compute_screen_layout,
    };

    #[test]
    fn snapshot_reports_edges_once() {
        let mut input = InputState::default();
        input.press(pad::A | pad::UP);
        let first = input.snapshot();
        assert_eq!(first.held, pad::A | pad::UP);
        assert_eq!(first.pressed, pad::A | pad::UP);
        assert_eq!(first.released, 0);

        input.release(pad::A);
        let second = input.snapshot();
        assert_eq!(second.held, pad::UP);
        assert_eq!(second.pressed, 0);
        assert_eq!(second.released, pad::A);
        assert_eq!(input.snapshot().released, 0);
    }

    #[test]
    fn a_press_and_release_between_samples_is_not_reported() {
        let mut input = InputState::default();
        input.press(pad::B);
        input.release(pad::B);
        assert_eq!(input.snapshot(), InputSnapshot::default());
    }

    #[test]
    fn physical_stick_uses_a_radial_deadzone_and_preserves_direction() {
        assert_eq!(gamepad_axes_to_circle_pad(0.0, 0.0, 16), (0, 0));
        assert_eq!(gamepad_axes_to_circle_pad(0.15, 0.0, 16), (0, 0));
        assert_eq!(gamepad_axes_to_circle_pad(0.10, 0.10, 16), (0, 0));
        assert_ne!(gamepad_axes_to_circle_pad(0.12, 0.12, 16), (0, 0));
        assert_eq!(gamepad_axes_to_circle_pad(1.0, 0.0, 16), (156, 0));
        assert_eq!(gamepad_axes_to_circle_pad(-1.0, 0.0, 16), (-156, 0));
        assert_eq!(gamepad_axes_to_circle_pad(0.0, 1.0, 16), (0, 156));
        assert_eq!(gamepad_axes_to_circle_pad(0.0, -1.0, 16), (0, -156));
    }

    #[test]
    fn physical_stick_is_clamped_to_the_unit_circle_and_rejects_non_finite_axes() {
        let diagonal = gamepad_axes_to_circle_pad(1.0, 1.0, 0);
        assert_eq!(diagonal.0, diagonal.1);
        assert!(diagonal.0 < CIRCLE_PAD_LIMIT);
        assert!(diagonal.0 > 100);
        assert_eq!(gamepad_axes_to_circle_pad(f32::NAN, 1.0, 16), (0, 156));
        assert_eq!(gamepad_axes_to_circle_pad(1.0, f32::INFINITY, 16), (156, 0));
        assert_eq!(gamepad_axes_to_circle_pad(1.0, 1.0, 100), diagonal);
    }

    #[test]
    fn unknown_bits_are_dropped_and_circle_pad_is_clamped() {
        let mut input = InputState::default();
        input.set_buttons(u32::MAX);
        assert_eq!(input.snapshot().held, pad::ALL);
        input.set_circle_pad(1000, -1000);
        assert_eq!(input.snapshot().circle_pad, (156, -156));
    }

    #[test]
    fn touch_is_clamped_and_cleared() {
        let mut input = InputState::default();
        input.touch(-5, 9999);
        assert_eq!(input.snapshot().touch, Some((0, 239)));
        input.touch_end();
        assert_eq!(input.snapshot().touch, None);
    }

    #[test]
    fn reset_releases_held_input_and_reports_the_release() {
        let mut input = InputState::default();
        input.press(pad::L);
        input.touch(10, 10);
        input.snapshot();
        input.reset();
        let snapshot = input.snapshot();
        assert_eq!(snapshot.held, 0);
        assert_eq!(snapshot.released, pad::L);
        assert_eq!(snapshot.touch, None);
    }

    #[test]
    fn frontend_virtual_buttons_convert_to_hardware_bits() {
        assert_eq!(pad_from_frontend_buttons(frontend_buttons::A), pad::A);
        assert_eq!(
            pad_from_frontend_buttons(frontend_buttons::START | frontend_buttons::UP),
            pad::START | pad::UP
        );
        assert_eq!(
            pad_from_frontend_buttons(frontend_buttons::ALL).count_ones(),
            12
        );
        assert_eq!(pad_from_frontend_buttons(0), 0);
    }

    #[test]
    fn surface_points_map_onto_the_bottom_screen_in_both_orientations() {
        for orientation in [ORIENTATION_PORTRAIT, ORIENTATION_LANDSCAPE] {
            let layout = compute_screen_layout(1000, 1000, orientation);
            let rect = layout.bottom;
            assert_eq!(
                touch_from_surface_point(&layout, rect.x, rect.y),
                Some((0, 0))
            );
            let last = touch_from_surface_point(
                &layout,
                rect.x + rect.width as i32 - 1,
                rect.y + rect.height as i32 - 1,
            )
            .unwrap();
            assert!(
                last.0 >= TOUCH_WIDTH - 2 && last.1 >= TOUCH_HEIGHT - 2,
                "{last:?}"
            );
            assert_eq!(touch_from_surface_point(&layout, rect.x - 1, rect.y), None);
            assert_eq!(
                touch_from_surface_point(&layout, rect.x, rect.y + rect.height as i32),
                None
            );
            // The top screen is not touch sensitive.
            let top = layout.top;
            assert_eq!(touch_from_surface_point(&layout, top.x, top.y), None);
        }
    }
}
