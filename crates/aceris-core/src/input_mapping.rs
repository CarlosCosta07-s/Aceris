//! Versioned physical-gamepad button mappings shared by desktop and Android.
//!
//! Host adapters translate native events into these stable controls. This
//! module owns logical mappings and profile persistence, not platform input.

use crate::game_identity::GameIdentity;
use crate::input::pad;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const CONFIG_MAGIC: &str = "ACERIS_GAMEPAD_MAPPING_V1";
const GLOBAL_FILE: &str = "gamepad_mapping_v1.txt";
const DEVICE_DIRECTORY: &str = "gamepad_devices";
const GAME_DIRECTORY: &str = "gamepad_games";

/// A standardized physical gamepad button before it is mapped to 3DS input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum GamepadControl {
    South = 0,
    East,
    North,
    West,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    Start,
    Select,
}

impl GamepadControl {
    pub const COUNT: usize = 14;
    pub const ALL: [Self; Self::COUNT] = [
        Self::South,
        Self::East,
        Self::North,
        Self::West,
        Self::DPadUp,
        Self::DPadDown,
        Self::DPadLeft,
        Self::DPadRight,
        Self::LeftShoulder,
        Self::RightShoulder,
        Self::LeftTrigger,
        Self::RightTrigger,
        Self::Start,
        Self::Select,
    ];

    pub const fn bit(self) -> u16 {
        1u16 << (self as u8)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::South => "SOUTH",
            Self::East => "EAST",
            Self::North => "NORTH",
            Self::West => "WEST",
            Self::DPadUp => "DPAD UP",
            Self::DPadDown => "DPAD DOWN",
            Self::DPadLeft => "DPAD LEFT",
            Self::DPadRight => "DPAD RIGHT",
            Self::LeftShoulder => "LEFT SHOULDER",
            Self::RightShoulder => "RIGHT SHOULDER",
            Self::LeftTrigger => "LEFT TRIGGER",
            Self::RightTrigger => "RIGHT TRIGGER",
            Self::Start => "START",
            Self::Select => "SELECT",
        }
    }

    pub const fn config_key(self) -> &'static str {
        match self {
            Self::South => "south",
            Self::East => "east",
            Self::North => "north",
            Self::West => "west",
            Self::DPadUp => "dpad_up",
            Self::DPadDown => "dpad_down",
            Self::DPadLeft => "dpad_left",
            Self::DPadRight => "dpad_right",
            Self::LeftShoulder => "left_shoulder",
            Self::RightShoulder => "right_shoulder",
            Self::LeftTrigger => "left_trigger",
            Self::RightTrigger => "right_trigger",
            Self::Start => "start",
            Self::Select => "select",
        }
    }
}

/// A hardware-order button that a physical control can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadButton {
    A,
    B,
    X,
    Y,
    Select,
    Start,
    Right,
    Left,
    Up,
    Down,
    R,
    L,
    ZL,
    ZR,
}

impl PadButton {
    pub const ALL: [Self; 14] = [
        Self::A, Self::B, Self::X, Self::Y, Self::Select, Self::Start,
        Self::Right, Self::Left, Self::Up, Self::Down, Self::R, Self::L,
        Self::ZL, Self::ZR,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::X => "X",
            Self::Y => "Y",
            Self::Select => "SELECT",
            Self::Start => "START",
            Self::Right => "RIGHT",
            Self::Left => "LEFT",
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::R => "R",
            Self::L => "L",
            Self::ZL => "ZL",
            Self::ZR => "ZR",
        }
    }

    pub const fn bit(self) -> u32 {
        match self {
            Self::A => pad::A,
            Self::B => pad::B,
            Self::X => pad::X,
            Self::Y => pad::Y,
            Self::Select => pad::SELECT,
            Self::Start => pad::START,
            Self::Right => pad::RIGHT,
            Self::Left => pad::LEFT,
            Self::Up => pad::UP,
            Self::Down => pad::DOWN,
            Self::R => pad::R,
            Self::L => pad::L,
            Self::ZL => pad::ZL,
            Self::ZR => pad::ZR,
        }
    }

    pub const fn config_value(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::X => "x",
            Self::Y => "y",
            Self::Select => "select",
            Self::Start => "start",
            Self::Right => "right",
            Self::Left => "left",
            Self::Up => "up",
            Self::Down => "down",
            Self::R => "r",
            Self::L => "l",
            Self::ZL => "zl",
            Self::ZR => "zr",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        Some(match value {
            "a" => Self::A,
            "b" => Self::B,
            "x" => Self::X,
            "y" => Self::Y,
            "select" => Self::Select,
            "start" => Self::Start,
            "right" => Self::Right,
            "left" => Self::Left,
            "up" => Self::Up,
            "down" => Self::Down,
            "r" => Self::R,
            "l" => Self::L,
            "zl" => Self::ZL,
            "zr" => Self::ZR,
            _ => return None,
        })
    }
}

/// Complete mapping from standardized physical controls to 3DS pad buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GamepadMapping {
    targets: [Option<PadButton>; GamepadControl::COUNT],
}

impl Default for GamepadMapping {
    fn default() -> Self {
        let mut mapping = Self {
            targets: [None; GamepadControl::COUNT],
        };
        mapping.set_target(GamepadControl::South, Some(PadButton::A));
        mapping.set_target(GamepadControl::East, Some(PadButton::B));
        mapping.set_target(GamepadControl::North, Some(PadButton::X));
        mapping.set_target(GamepadControl::West, Some(PadButton::Y));
        mapping.set_target(GamepadControl::DPadUp, Some(PadButton::Up));
        mapping.set_target(GamepadControl::DPadDown, Some(PadButton::Down));
        mapping.set_target(GamepadControl::DPadLeft, Some(PadButton::Left));
        mapping.set_target(GamepadControl::DPadRight, Some(PadButton::Right));
        mapping.set_target(GamepadControl::LeftShoulder, Some(PadButton::L));
        mapping.set_target(GamepadControl::RightShoulder, Some(PadButton::R));
        mapping.set_target(GamepadControl::LeftTrigger, Some(PadButton::ZL));
        mapping.set_target(GamepadControl::RightTrigger, Some(PadButton::ZR));
        mapping.set_target(GamepadControl::Start, Some(PadButton::Start));
        mapping.set_target(GamepadControl::Select, Some(PadButton::Select));
        mapping
    }
}

impl GamepadMapping {
    pub fn target(&self, control: GamepadControl) -> Option<PadButton> {
        self.targets[control as usize]
    }

    pub fn set_target(&mut self, control: GamepadControl, target: Option<PadButton>) {
        self.targets[control as usize] = target;
    }

    /// Cycles a control's destination through disabled and all supported 3DS buttons.
    pub fn cycle_target(&mut self, control: GamepadControl, direction: i8) {
        if direction == 0 {
            return;
        }
        let current = self
            .target(control)
            .and_then(|target| PadButton::ALL.iter().position(|candidate| *candidate == target))
            .map(|index| index + 1)
            .unwrap_or(0);
        let count = PadButton::ALL.len() + 1;
        let next = (current as isize + isize::from(direction.signum()))
            .rem_euclid(count as isize) as usize;
        self.set_target(control, next.checked_sub(1).map(|index| PadButton::ALL[index]));
    }

    pub fn mask_for_held_controls(&self, held_controls: u16) -> u32 {
        GamepadControl::ALL
            .into_iter()
            .filter(|control| held_controls & control.bit() != 0)
            .filter_map(|control| self.target(control))
            .fold(0, |mask, target| mask | target.bit())
    }

    pub fn mask_from_pressed(&self, mut is_pressed: impl FnMut(GamepadControl) -> bool) -> u32 {
        GamepadControl::ALL
            .into_iter()
            .filter(|control| is_pressed(*control))
            .filter_map(|control| self.target(control))
            .fold(0, |mask, target| mask | target.bit())
    }

    pub fn to_config_string(&self) -> String {
        let mut contents = format!("{CONFIG_MAGIC}\n");
        for control in GamepadControl::ALL {
            let value = self
                .target(control)
                .map(PadButton::config_value)
                .unwrap_or("none");
            contents.push_str(control.config_key());
            contents.push('=');
            contents.push_str(value);
            contents.push('\n');
        }
        contents
    }

    /// Parses only the exact V1 schema; malformed or partial profiles are rejected.
    pub fn from_config_str(contents: &str) -> Option<Self> {
        let mut lines = contents.lines();
        if lines.next()? != CONFIG_MAGIC {
            return None;
        }
        let mut mapping = Self {
            targets: [None; GamepadControl::COUNT],
        };
        for control in GamepadControl::ALL {
            let (key, value) = lines.next()?.split_once('=')?;
            if key != control.config_key() {
                return None;
            }
            let target = if value == "none" {
                None
            } else {
                Some(PadButton::from_config_value(value)?)
            };
            mapping.set_target(control, target);
        }
        if lines.next().is_some() {
            return None;
        }
        Some(mapping)
    }

    /// Loads the effective mapping with game > controller-model > global precedence.
    ///
    /// The device key is GilRs' 16-byte model UUID. A zero UUID is not persisted.
    pub fn load_for_context(
        settings_directory: &Path,
        device_uuid: Option<[u8; 16]>,
        game_identity: Option<GameIdentity>,
    ) -> Self {
        let global = load_profile(&settings_directory.join(GLOBAL_FILE)).unwrap_or_default();
        let device = device_uuid
            .filter(|uuid| !is_zero_uuid(uuid))
            .and_then(|uuid| load_profile(&device_profile_path(settings_directory, uuid)));
        let game = game_identity
            .and_then(|identity| load_profile(&game_profile_path(settings_directory, identity)));
        game.or(device).unwrap_or(global)
    }

    /// Loads only the global profile, falling back to the standard controller layout.
    pub fn load_global(settings_directory: &Path) -> Self {
        load_profile(&settings_directory.join(GLOBAL_FILE)).unwrap_or_default()
    }

    /// Loads a title-specific profile, falling back to the global profile.
    pub fn load_for_game_or_global(
        settings_directory: &Path,
        identity: GameIdentity,
    ) -> Self {
        load_profile(&game_profile_path(settings_directory, identity))
            .unwrap_or_else(|| Self::load_global(settings_directory))
    }

    pub fn save_global(&self, settings_directory: &Path) -> io::Result<()> {
        save_profile(&settings_directory.join(GLOBAL_FILE), self)
    }

    pub fn save_for_device(
        &self,
        settings_directory: &Path,
        device_uuid: [u8; 16],
    ) -> io::Result<()> {
        if is_zero_uuid(&device_uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a zero controller UUID cannot identify a model profile",
            ));
        }
        save_profile(&device_profile_path(settings_directory, device_uuid), self)
    }

    pub fn save_for_game(
        &self,
        settings_directory: &Path,
        identity: GameIdentity,
    ) -> io::Result<()> {
        save_profile(&game_profile_path(settings_directory, identity), self)
    }
}

fn is_zero_uuid(uuid: &[u8; 16]) -> bool {
    uuid.iter().all(|byte| *byte == 0)
}

fn uuid_hex(uuid: [u8; 16]) -> String {
    let mut encoded = String::with_capacity(32);
    for byte in uuid {
        use std::fmt::Write as _;
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn device_profile_path(settings_directory: &Path, uuid: [u8; 16]) -> PathBuf {
    settings_directory
        .join(DEVICE_DIRECTORY)
        .join(format!("{}.v1", uuid_hex(uuid)))
}

fn game_profile_path(settings_directory: &Path, identity: GameIdentity) -> PathBuf {
    settings_directory
        .join(GAME_DIRECTORY)
        .join(format!("{:016x}.v1", identity.title_id()))
}

fn load_profile(path: &Path) -> Option<GamepadMapping> {
    GamepadMapping::from_config_str(&fs::read_to_string(path).ok()?)
}

fn save_profile(path: &Path, mapping: &GamepadMapping) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, mapping.to_config_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mapping_matches_the_standard_3ds_controller_layout() {
        let mapping = GamepadMapping::default();
        assert_eq!(mapping.target(GamepadControl::South), Some(PadButton::A));
        assert_eq!(mapping.target(GamepadControl::East), Some(PadButton::B));
        assert_eq!(mapping.target(GamepadControl::North), Some(PadButton::X));
        assert_eq!(mapping.target(GamepadControl::West), Some(PadButton::Y));
        assert_eq!(
            mapping.target(GamepadControl::LeftShoulder),
            Some(PadButton::L)
        );
        assert_eq!(
            mapping.target(GamepadControl::RightShoulder),
            Some(PadButton::R)
        );
        assert_eq!(
            mapping.target(GamepadControl::LeftTrigger),
            Some(PadButton::ZL)
        );
        assert_eq!(
            mapping.target(GamepadControl::RightTrigger),
            Some(PadButton::ZR)
        );
    }

    #[test]
    fn mapping_config_round_trips_and_supports_explicitly_unmapped_controls() {
        let mut mapping = GamepadMapping::default();
        mapping.set_target(GamepadControl::South, Some(PadButton::X));
        mapping.set_target(GamepadControl::LeftShoulder, None);
        assert_eq!(
            GamepadMapping::from_config_str(&mapping.to_config_string()),
            Some(mapping)
        );
    }

    #[test]
    fn target_cycle_includes_disabled_and_wraps_in_both_directions() {
        let mut mapping = GamepadMapping::default();
        mapping.set_target(GamepadControl::South, None);
        mapping.cycle_target(GamepadControl::South, 1);
        assert_eq!(mapping.target(GamepadControl::South), Some(PadButton::A));
        mapping.cycle_target(GamepadControl::South, -1);
        assert_eq!(mapping.target(GamepadControl::South), None);
    }

    #[test]
    fn parser_rejects_wrong_version_missing_duplicate_or_unknown_fields() {
        let defaults = GamepadMapping::default().to_config_string();
        assert!(GamepadMapping::from_config_str("ACERIS_GAMEPAD_MAPPING_V2\n").is_none());
        assert!(GamepadMapping::from_config_str(&defaults.replace("south=a\n", "")).is_none());
        assert!(
            GamepadMapping::from_config_str(&defaults.replace("south=a\n", "east=b\n")).is_none()
        );
        assert!(
            GamepadMapping::from_config_str(&defaults.replace("south=a\n", "south=unknown\n"))
                .is_none()
        );
        assert!(GamepadMapping::from_config_str(&(defaults + "unexpected=value\n")).is_none());
    }

    #[test]
    fn duplicate_targets_are_allowed_and_held_controls_are_combined() {
        let mut mapping = GamepadMapping::default();
        mapping.set_target(GamepadControl::East, Some(PadButton::A));
        mapping.set_target(GamepadControl::West, None);
        let held =
            GamepadControl::South.bit() | GamepadControl::East.bit() | GamepadControl::West.bit();
        assert_eq!(mapping.mask_for_held_controls(held), pad::A);
    }

    #[test]
    fn global_device_and_game_profiles_follow_precedence() {
        let root =
            std::env::temp_dir().join(format!("aceris-gamepad-profiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut global = GamepadMapping::default();
        global.set_target(GamepadControl::South, Some(PadButton::B));
        global.save_global(&root).unwrap();
        let device_uuid = [0xA5; 16];
        let mut device = GamepadMapping::default();
        device.set_target(GamepadControl::South, Some(PadButton::X));
        device.save_for_device(&root, device_uuid).unwrap();
        let identity = GameIdentity::from_title_id(0x0004_0000_1234_5600).unwrap();
        let mut game = GamepadMapping::default();
        game.set_target(GamepadControl::South, Some(PadButton::Y));
        game.save_for_game(&root, identity).unwrap();
        assert_eq!(
            GamepadMapping::load_for_context(&root, None, None).target(GamepadControl::South),
            Some(PadButton::B)
        );
        assert_eq!(
            GamepadMapping::load_for_context(&root, Some(device_uuid), None)
                .target(GamepadControl::South),
            Some(PadButton::X)
        );
        assert_eq!(
            GamepadMapping::load_for_context(&root, Some(device_uuid), Some(identity))
                .target(GamepadControl::South),
            Some(PadButton::Y)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn zero_uuid_cannot_write_a_device_profile() {
        let root =
            std::env::temp_dir().join(format!("aceris-gamepad-zero-uuid-{}", std::process::id()));
        let error = GamepadMapping::default()
            .save_for_device(&root, [0; 16])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let _ = fs::remove_dir_all(root);
    }
}
