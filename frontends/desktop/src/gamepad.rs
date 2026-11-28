//! Cross-platform physical gamepad input for the desktop host.
//!
//! GilRs supplies OS-specific device enumeration and hotplug events. Aceris
//! owns the mapping from the standardized controller layout to 3DS controls.

use aceris_core::input::{CIRCLE_PAD_LIMIT, InputState, pad};
use gilrs::{Axis, Button, GamepadId, Gilrs};

use crate::window::KeyboardInput;

pub(crate) struct GamepadManager {
    gilrs: Option<Gilrs>,
    active_gamepad: Option<GamepadId>,
}

impl GamepadManager {
    pub(crate) fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(gilrs) => Some(gilrs),
            Err(error) => {
                eprintln!("Aceris: gamepad backend unavailable: {error:?}");
                None
            }
        };
        Self {
            gilrs,
            active_gamepad: None,
        }
    }

    /// Combines the keyboard and the active physical controller without one
    /// device releasing a button that is still held by the other.
    pub(crate) fn apply(
        &mut self,
        keyboard: &KeyboardInput,
        input: &mut InputState,
        deadzone_percent: u8,
    ) {
        let (gamepad_buttons, gamepad_axes) = self.read_active_gamepad(deadzone_percent);
        input.set_buttons(keyboard.button_mask() | gamepad_buttons);

        let keyboard_axes = keyboard.circle_pad();
        let x = if gamepad_axes.0 != 0 {
            gamepad_axes.0
        } else {
            keyboard_axes.0
        };
        let y = if gamepad_axes.1 != 0 {
            gamepad_axes.1
        } else {
            keyboard_axes.1
        };
        input.set_circle_pad(i32::from(x), i32::from(y));
    }

    fn read_active_gamepad(&mut self, deadzone_percent: u8) -> (u32, (i16, i16)) {
        let Some(gilrs) = self.gilrs.as_mut() else {
            self.active_gamepad = None;
            return (0, (0, 0));
        };

        // Drain the OS event queue so GilRs updates its cached button/axis state.
        while gilrs.next_event().is_some() {}

        let active = self
            .active_gamepad
            .filter(|id| gilrs.gamepad(*id).is_connected())
            .or_else(|| {
                gilrs
                    .gamepads()
                    .find(|(_, gamepad)| gamepad.is_connected())
                    .map(|(id, _)| id)
            });
        self.active_gamepad = active;

        let Some(id) = active else {
            return (0, (0, 0));
        };
        let gamepad = gilrs.gamepad(id);
        let buttons = button_mask(&gamepad);
        let axes = (
            axis_to_circle_pad(gamepad.value(Axis::LeftStickX), deadzone_percent),
            axis_to_circle_pad(-gamepad.value(Axis::LeftStickY), deadzone_percent),
        );
        (buttons, axes)
    }
}

fn button_bit(button: Button) -> u32 {
    match button {
        Button::South => pad::A,
        Button::East => pad::B,
        Button::North => pad::X,
        Button::West => pad::Y,
        Button::DPadUp => pad::UP,
        Button::DPadDown => pad::DOWN,
        Button::DPadLeft => pad::LEFT,
        Button::DPadRight => pad::RIGHT,
        Button::LeftTrigger => pad::L,
        Button::RightTrigger => pad::R,
        Button::Start => pad::START,
        Button::Select => pad::SELECT,
        _ => 0,
    }
}

fn button_mask(gamepad: &gilrs::Gamepad<'_>) -> u32 {
    const BUTTONS: [Button; 12] = [
        Button::South,
        Button::East,
        Button::North,
        Button::West,
        Button::DPadUp,
        Button::DPadDown,
        Button::DPadLeft,
        Button::DPadRight,
        Button::LeftTrigger,
        Button::RightTrigger,
        Button::Start,
        Button::Select,
    ];

    BUTTONS.iter().fold(0, |mask, button| {
        if gamepad.is_pressed(*button) {
            mask | button_bit(*button)
        } else {
            mask
        }
    })
}

fn axis_to_circle_pad(value: f32, deadzone_percent: u8) -> i16 {
    if !value.is_finite() {
        return 0;
    }
    let deadzone = f32::from(deadzone_percent.min(99)) / 100.0;
    let clamped = value.clamp(-1.0, 1.0);
    let magnitude = clamped.abs();
    if magnitude <= deadzone {
        return 0;
    }
    let normalized = (magnitude - deadzone) / (1.0 - deadzone);
    let sign = if clamped.is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    (normalized * sign * f32::from(CIRCLE_PAD_LIMIT)).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standardized_gamepad_buttons_map_to_3ds_controls() {
        assert_eq!(button_bit(Button::South), pad::A);
        assert_eq!(button_bit(Button::East), pad::B);
        assert_eq!(button_bit(Button::North), pad::X);
        assert_eq!(button_bit(Button::West), pad::Y);
        assert_eq!(button_bit(Button::LeftTrigger), pad::L);
        assert_eq!(button_bit(Button::RightTrigger), pad::R);
        assert_eq!(button_bit(Button::Start), pad::START);
        assert_eq!(button_bit(Button::Select), pad::SELECT);
        assert_eq!(button_bit(Button::Mode), 0);
    }

    #[test]
    fn gamepad_stick_uses_deadzone_and_clamps_to_circle_pad_range() {
        assert_eq!(axis_to_circle_pad(0.0, 16), 0);
        assert_eq!(axis_to_circle_pad(0.15, 16), 0);
        assert_eq!(axis_to_circle_pad(-0.16, 16), 0);
        assert_eq!(axis_to_circle_pad(1.0, 16), CIRCLE_PAD_LIMIT);
        assert_eq!(axis_to_circle_pad(-1.0, 16), -CIRCLE_PAD_LIMIT);
        assert_eq!(axis_to_circle_pad(f32::NAN, 16), 0);
        assert_eq!(axis_to_circle_pad(f32::INFINITY, 16), 0);
        assert_eq!(axis_to_circle_pad(0.07, 8), 0);
        assert!(axis_to_circle_pad(0.10, 8) > 0);
    }
}
