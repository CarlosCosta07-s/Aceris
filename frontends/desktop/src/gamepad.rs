//! Cross-platform physical gamepad input for the desktop host.
//!
//! GilRs supplies OS-specific device enumeration and hotplug events. Aceris
//! owns the mapping from the standardized controller layout to 3DS controls.

use aceris_core::{
    GameIdentity,
    input::{CIRCLE_PAD_LIMIT, InputState},
    input_mapping::{GamepadControl, GamepadMapping},
};
use gilrs::{Axis, Button, GamepadId, Gilrs};
use std::path::{Path, PathBuf};

use crate::window::KeyboardInput;

pub(crate) struct GamepadManager {
    gilrs: Option<Gilrs>,
    active_gamepad: Option<GamepadId>,
    active_device_uuid: Option<[u8; 16]>,
    settings_directory: PathBuf,
    game_identity: Option<GameIdentity>,
    mapping: GamepadMapping,
}

impl GamepadManager {
    pub(crate) fn new(settings_directory: &Path, game_identity: Option<GameIdentity>) -> Self {
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
            active_device_uuid: None,
            settings_directory: settings_directory.to_path_buf(),
            game_identity,
            mapping: GamepadMapping::load_for_context(settings_directory, None, game_identity),
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

    pub(crate) fn mapping(&self) -> &GamepadMapping {
        &self.mapping
    }

    pub(crate) fn set_mapping_for_editor(&mut self, mapping: GamepadMapping) {
        self.mapping = mapping;
    }

    pub(crate) fn load_global_mapping_for_editor(&self) -> GamepadMapping {
        GamepadMapping::load_global(&self.settings_directory)
    }

    pub(crate) fn load_game_mapping_for_editor(&self) -> GamepadMapping {
        self.game_identity
            .map(|identity| {
                GamepadMapping::load_for_game_or_global(&self.settings_directory, identity)
            })
            .unwrap_or_else(|| self.load_global_mapping_for_editor())
    }

    pub(crate) fn save_mapping_for_editor(
        &mut self,
        mapping: GamepadMapping,
        game_scope: bool,
    ) -> std::io::Result<()> {
        if game_scope {
            if let Some(identity) = self.game_identity {
                mapping.save_for_game(&self.settings_directory, identity)?;
            } else {
                mapping.save_global(&self.settings_directory)?;
            }
        } else {
            mapping.save_global(&self.settings_directory)?;
        }
        self.mapping = mapping;
        Ok(())
    }

    pub(crate) fn reload_effective_mapping(&mut self) {
        self.mapping = GamepadMapping::load_for_context(
            &self.settings_directory,
            self.active_device_uuid,
            self.game_identity,
        );
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
            self.active_device_uuid = None;
            self.mapping = GamepadMapping::load_for_context(
                &self.settings_directory,
                None,
                self.game_identity,
            );
            return (0, (0, 0));
        };
        let gamepad = gilrs.gamepad(id);
        let reported_uuid = gamepad.uuid();
        let device_uuid = (!reported_uuid.iter().all(|byte| *byte == 0)).then_some(reported_uuid);
        if device_uuid != self.active_device_uuid {
            self.active_device_uuid = device_uuid;
            self.mapping = GamepadMapping::load_for_context(
                &self.settings_directory,
                device_uuid,
                self.game_identity,
            );
        }
        let buttons = button_mask(&gamepad, &self.mapping);
        let axes = (
            axis_to_circle_pad(gamepad.value(Axis::LeftStickX), deadzone_percent),
            axis_to_circle_pad(-gamepad.value(Axis::LeftStickY), deadzone_percent),
        );
        (buttons, axes)
    }
}

fn gilrs_button(control: GamepadControl) -> Button {
    match control {
        GamepadControl::South => Button::South,
        GamepadControl::East => Button::East,
        GamepadControl::North => Button::North,
        GamepadControl::West => Button::West,
        GamepadControl::DPadUp => Button::DPadUp,
        GamepadControl::DPadDown => Button::DPadDown,
        GamepadControl::DPadLeft => Button::DPadLeft,
        GamepadControl::DPadRight => Button::DPadRight,
        GamepadControl::LeftShoulder => Button::LeftTrigger,
        GamepadControl::RightShoulder => Button::RightTrigger,
        GamepadControl::LeftTrigger => Button::LeftTrigger2,
        GamepadControl::RightTrigger => Button::RightTrigger2,
        GamepadControl::Start => Button::Start,
        GamepadControl::Select => Button::Select,
    }
}

fn button_mask(gamepad: &gilrs::Gamepad<'_>, mapping: &GamepadMapping) -> u32 {
    mapping.mask_from_pressed(|control| gamepad.is_pressed(gilrs_button(control)))
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
    use aceris_core::input::pad;
    use aceris_core::input_mapping::PadButton;

    #[test]
    fn gilrs_controls_cover_the_standard_3ds_button_sources() {
        assert_eq!(gilrs_button(GamepadControl::South), Button::South);
        assert_eq!(gilrs_button(GamepadControl::East), Button::East);
        assert_eq!(
            gilrs_button(GamepadControl::LeftShoulder),
            Button::LeftTrigger
        );
        assert_eq!(
            gilrs_button(GamepadControl::RightShoulder),
            Button::RightTrigger
        );
        assert_eq!(
            gilrs_button(GamepadControl::LeftTrigger),
            Button::LeftTrigger2
        );
        assert_eq!(
            gilrs_button(GamepadControl::RightTrigger),
            Button::RightTrigger2
        );
        assert_eq!(
            GamepadMapping::default().target(GamepadControl::LeftTrigger),
            Some(PadButton::ZL)
        );
        assert_eq!(
            GamepadMapping::default().target(GamepadControl::RightTrigger),
            Some(PadButton::ZR)
        );
    }

    #[test]
    fn remapped_controls_produce_the_selected_hardware_bits() {
        let mut mapping = GamepadMapping::default();
        mapping.set_target(GamepadControl::South, Some(PadButton::X));
        mapping.set_target(GamepadControl::East, None);
        assert_eq!(
            mapping.mask_from_pressed(|control| matches!(
                control,
                GamepadControl::South | GamepadControl::East
            )),
            pad::X
        );
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
