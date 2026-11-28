//! Cross-platform physical gamepad input for the desktop host.
//!
//! GilRs supplies OS-specific device enumeration and hotplug events. Aceris
//! owns the mapping from the standardized controller layout to 3DS controls.

use aceris_core::{
    GameIdentity,
    input::InputState,
    input_mapping::{GamepadAxisSettings, GamepadControl, GamepadMapping},
};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use std::path::{Path, PathBuf};

use crate::window::KeyboardInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GamepadAxisProfileScope {
    Global,
    Device,
    Game,
}

pub(crate) struct GamepadManager {
    gilrs: Option<Gilrs>,
    active_gamepad: Option<GamepadId>,
    active_device_uuid: Option<[u8; 16]>,
    settings_directory: PathBuf,
    game_identity: Option<GameIdentity>,
    mapping: GamepadMapping,
    axis_settings: GamepadAxisSettings,
    mapping_capture_active: bool,
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
            axis_settings: GamepadAxisSettings::load_for_context(
                settings_directory,
                None,
                game_identity,
            ),
            mapping_capture_active: false,
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
        let _ = self.poll_mapping_capture();
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

    pub(crate) fn start_mapping_capture(&mut self) {
        self.mapping_capture_active = true;
    }

    pub(crate) fn cancel_mapping_capture(&mut self) {
        self.mapping_capture_active = false;
    }

    pub(crate) fn mapping_capture_active(&self) -> bool {
        self.mapping_capture_active
    }

    /// Drains GilRs events and returns the first supported button pressed while capture is armed.
    pub(crate) fn poll_mapping_capture(&mut self) -> Option<GamepadControl> {
        let capture_active = self.mapping_capture_active;
        let active_gamepad = self.active_gamepad;
        let gilrs = self.gilrs.as_mut()?;
        let mut captured = None;
        while let Some(event) = gilrs.next_event() {
            if !capture_active || active_gamepad.is_some_and(|active| event.id != active) {
                continue;
            }
            if let EventType::ButtonPressed(button, _) = event.event {
                if let Some(control) = gamepad_control_from_gilrs_button(button) {
                    captured = Some(control);
                    break;
                }
            }
        }
        if captured.is_some() {
            self.mapping_capture_active = false;
        }
        captured
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

    pub(crate) fn axis_settings(&self) -> &GamepadAxisSettings {
        &self.axis_settings
    }

    pub(crate) fn set_axis_settings_for_editor(&mut self, settings: GamepadAxisSettings) {
        self.axis_settings = settings;
    }

    pub(crate) fn default_axis_profile_scope(&self) -> GamepadAxisProfileScope {
        if self.game_identity.is_some() {
            GamepadAxisProfileScope::Game
        } else if self.active_device_uuid.is_some() {
            GamepadAxisProfileScope::Device
        } else {
            GamepadAxisProfileScope::Global
        }
    }

    pub(crate) fn next_axis_profile_scope(
        &self,
        scope: GamepadAxisProfileScope,
    ) -> GamepadAxisProfileScope {
        match scope {
            GamepadAxisProfileScope::Global if self.active_device_uuid.is_some() => {
                GamepadAxisProfileScope::Device
            }
            GamepadAxisProfileScope::Global if self.game_identity.is_some() => {
                GamepadAxisProfileScope::Game
            }
            GamepadAxisProfileScope::Global => GamepadAxisProfileScope::Global,
            GamepadAxisProfileScope::Device if self.game_identity.is_some() => {
                GamepadAxisProfileScope::Game
            }
            GamepadAxisProfileScope::Device => GamepadAxisProfileScope::Global,
            GamepadAxisProfileScope::Game => GamepadAxisProfileScope::Global,
        }
    }

    pub(crate) fn load_axis_settings_for_editor_scope(
        &self,
        scope: GamepadAxisProfileScope,
    ) -> GamepadAxisSettings {
        match scope {
            GamepadAxisProfileScope::Global => {
                GamepadAxisSettings::load_global(&self.settings_directory)
            }
            GamepadAxisProfileScope::Device => self
                .active_device_uuid
                .map(|uuid| {
                    GamepadAxisSettings::load_for_context(
                        &self.settings_directory,
                        Some(uuid),
                        None,
                    )
                })
                .unwrap_or_else(|| GamepadAxisSettings::load_global(&self.settings_directory)),
            GamepadAxisProfileScope::Game => self
                .game_identity
                .map(|identity| {
                    GamepadAxisSettings::load_for_context(
                        &self.settings_directory,
                        self.active_device_uuid,
                        Some(identity),
                    )
                })
                .unwrap_or_else(|| GamepadAxisSettings::load_global(&self.settings_directory)),
        }
    }

    pub(crate) fn save_axis_settings_for_editor_scope(
        &mut self,
        settings: GamepadAxisSettings,
        scope: GamepadAxisProfileScope,
    ) -> std::io::Result<()> {
        match scope {
            GamepadAxisProfileScope::Global => {
                settings.save_global(&self.settings_directory)?;
            }
            GamepadAxisProfileScope::Device => {
                let Some(uuid) = self.active_device_uuid else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "no active controller model is available",
                    ));
                };
                settings.save_for_device(&self.settings_directory, uuid)?;
            }
            GamepadAxisProfileScope::Game => {
                if let Some(identity) = self.game_identity {
                    settings.save_for_game(&self.settings_directory, identity)?;
                } else {
                    settings.save_global(&self.settings_directory)?;
                }
            }
        }
        self.axis_settings = settings;
        Ok(())
    }

    pub(crate) fn reload_effective_axis_settings(&mut self) {
        self.axis_settings = GamepadAxisSettings::load_for_context(
            &self.settings_directory,
            self.active_device_uuid,
            self.game_identity,
        );
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
            self.axis_settings = GamepadAxisSettings::load_for_context(
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
            self.axis_settings = GamepadAxisSettings::load_for_context(
                &self.settings_directory,
                device_uuid,
                self.game_identity,
            );
        }
        let buttons = button_mask(&gamepad, &self.mapping);
        let axes = self.axis_settings.apply_to_circle_pad(
            gamepad.value(Axis::LeftStickX),
            -gamepad.value(Axis::LeftStickY),
            deadzone_percent,
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

fn gamepad_control_from_gilrs_button(button: Button) -> Option<GamepadControl> {
    Some(match button {
        Button::South => GamepadControl::South,
        Button::East => GamepadControl::East,
        Button::North => GamepadControl::North,
        Button::West => GamepadControl::West,
        Button::DPadUp => GamepadControl::DPadUp,
        Button::DPadDown => GamepadControl::DPadDown,
        Button::DPadLeft => GamepadControl::DPadLeft,
        Button::DPadRight => GamepadControl::DPadRight,
        Button::LeftTrigger => GamepadControl::LeftShoulder,
        Button::RightTrigger => GamepadControl::RightShoulder,
        Button::LeftTrigger2 => GamepadControl::LeftTrigger,
        Button::RightTrigger2 => GamepadControl::RightTrigger,
        Button::Start => GamepadControl::Start,
        Button::Select => GamepadControl::Select,
        _ => return None,
    })
}

fn button_mask(gamepad: &gilrs::Gamepad<'_>, mapping: &GamepadMapping) -> u32 {
    mapping.mask_from_pressed(|control| gamepad.is_pressed(gilrs_button(control)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aceris_core::input::{gamepad_axes_to_circle_pad, pad};
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
    fn desktop_stick_uses_the_shared_radial_deadzone_mapping() {
        assert_eq!(gamepad_axes_to_circle_pad(0.0, 0.0, 16), (0, 0));
        assert_eq!(gamepad_axes_to_circle_pad(0.15, 0.0, 16), (0, 0));
        assert_eq!(
            gamepad_axes_to_circle_pad(1.0, 0.0, 16),
            (aceris_core::input::CIRCLE_PAD_LIMIT, 0)
        );
        assert_eq!(
            gamepad_axes_to_circle_pad(0.0, 1.0, 16),
            (0, aceris_core::input::CIRCLE_PAD_LIMIT)
        );
        assert_eq!(gamepad_axes_to_circle_pad(1.0, f32::NAN, 16).1, 0);
    }
}
