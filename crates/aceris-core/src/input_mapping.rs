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
const AXIS_GLOBAL_FILE: &str = "gamepad_axis_settings_v1.txt";
const AXIS_DEVICE_DIRECTORY: &str = "gamepad_axis_devices";
const AXIS_GAME_DIRECTORY: &str = "gamepad_axis_games";
const AXIS_CONFIG_MAGIC: &str = "ACERIS_GAMEPAD_AXIS_SETTINGS_V1";
const AXIS_DEADZONE_STEPS: [u8; 5] = [8, 12, 16, 20, 24];

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
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::Select,
        Self::Start,
        Self::Right,
        Self::Left,
        Self::Up,
        Self::Down,
        Self::R,
        Self::L,
        Self::ZL,
        Self::ZR,
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

    /// Assigns a destination to a captured physical control and frees that
    /// destination from any other source to keep mappings one-to-one.
    pub fn assign_target_uniquely(&mut self, control: GamepadControl, target: Option<PadButton>) {
        if let Some(target) = target {
            for other in GamepadControl::ALL {
                if other != control && self.target(other) == Some(target) {
                    self.set_target(other, None);
                }
            }
        }
        self.set_target(control, target);
    }

    /// Cycles a control's destination through disabled and all supported 3DS buttons.
    pub fn cycle_target(&mut self, control: GamepadControl, direction: i8) {
        if direction == 0 {
            return;
        }
        let current = self
            .target(control)
            .and_then(|target| {
                PadButton::ALL
                    .iter()
                    .position(|candidate| *candidate == target)
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        let count = PadButton::ALL.len() + 1;
        let next = (current as isize + isize::from(direction.signum())).rem_euclid(count as isize)
            as usize;
        self.set_target(
            control,
            next.checked_sub(1).map(|index| PadButton::ALL[index]),
        );
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
    pub fn load_for_game_or_global(settings_directory: &Path, identity: GameIdentity) -> Self {
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

/// Analog-stick configuration stored independently from button mappings.
///
/// Profiles use the same precedence as button mappings: game > controller
/// model > global. This keeps existing button-map files fully compatible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GamepadAxisSettings {
    invert_x: bool,
    invert_y: bool,
    sensitivity_percent: u8,
    deadzone_override_percent: Option<u8>,
}

impl Default for GamepadAxisSettings {
    fn default() -> Self {
        Self {
            invert_x: false,
            invert_y: false,
            sensitivity_percent: 100,
            deadzone_override_percent: None,
        }
    }
}

impl GamepadAxisSettings {
    pub fn invert_x(&self) -> bool {
        self.invert_x
    }

    pub fn invert_y(&self) -> bool {
        self.invert_y
    }

    pub fn sensitivity_percent(&self) -> u8 {
        self.sensitivity_percent
    }

    pub fn set_invert_x(&mut self, enabled: bool) {
        self.invert_x = enabled;
    }

    pub fn set_invert_y(&mut self, enabled: bool) {
        self.invert_y = enabled;
    }

    /// Sensitivity is clamped to the supported 50–200% range.
    pub fn set_sensitivity_percent(&mut self, percent: u8) {
        self.sensitivity_percent = percent.clamp(50, 200);
    }

    /// Current per-profile deadzone override. None inherits the existing global setting.
    pub fn deadzone_override_percent(&self) -> Option<u8> {
        self.deadzone_override_percent
    }

    /// Selects one supported per-profile deadzone, or None to inherit the global value.
    pub fn set_deadzone_override_percent(&mut self, percent: Option<u8>) -> bool {
        if percent.is_some_and(|value| !AXIS_DEADZONE_STEPS.contains(&value)) {
            return false;
        }
        self.deadzone_override_percent = percent;
        true
    }

    /// Moves the override by one supported step relative to the effective global fallback.
    pub fn adjust_deadzone_override(&mut self, inherited_percent: u8, direction: i8) {
        if direction == 0 {
            return;
        }
        let inherited = if AXIS_DEADZONE_STEPS.contains(&inherited_percent) {
            inherited_percent
        } else {
            16
        };
        let current = self.deadzone_override_percent.unwrap_or(inherited);
        let index = AXIS_DEADZONE_STEPS
            .iter()
            .position(|value| *value == current)
            .unwrap_or(2);
        let next = if direction > 0 {
            (index + 1).min(AXIS_DEADZONE_STEPS.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        self.deadzone_override_percent = Some(AXIS_DEADZONE_STEPS[next]);
    }

    /// Effective deadzone, preserving the previous global setting when no override exists.
    pub fn effective_deadzone_percent(&self, inherited_percent: u8) -> u8 {
        self.deadzone_override_percent.unwrap_or(inherited_percent)
    }

    /// Applies axis inversion and sensitivity before converting to guest input.
    pub fn apply_to_circle_pad(&self, x: f32, y: f32, deadzone_percent: u8) -> (i16, i16) {
        let x = if self.invert_x { -x } else { x };
        let y = if self.invert_y { -y } else { y };
        crate::input::gamepad_axes_to_circle_pad_with_sensitivity(
            x,
            y,
            self.effective_deadzone_percent(deadzone_percent),
            self.sensitivity_percent,
        )
    }

    pub fn to_config_string(&self) -> String {
        let deadzone = self
            .deadzone_override_percent
            .map(|value| value.to_string())
            .unwrap_or_else(|| "inherit".to_string());
        format!(
            "{AXIS_CONFIG_MAGIC}\ninvert_x={}\ninvert_y={}\nsensitivity_percent={}\ndeadzone_percent={}\n",
            u8::from(self.invert_x),
            u8::from(self.invert_y),
            self.sensitivity_percent,
            deadzone
        )
    }

    /// Parses only the exact V1 axis-settings format.
    pub fn from_config_str(contents: &str) -> Option<Self> {
        let mut lines = contents.lines();
        if lines.next()? != AXIS_CONFIG_MAGIC {
            return None;
        }
        let invert_x = match lines.next()? {
            "invert_x=0" => false,
            "invert_x=1" => true,
            _ => return None,
        };
        let invert_y = match lines.next()? {
            "invert_y=0" => false,
            "invert_y=1" => true,
            _ => return None,
        };
        let sensitivity_percent = lines
            .next()?
            .strip_prefix("sensitivity_percent=")?
            .parse::<u8>()
            .ok()
            .filter(|value| (50..=200).contains(value))?;
        let deadzone_override_percent = match lines.next() {
            None | Some("deadzone_percent=inherit") => None,
            Some(line) => {
                let percent = line
                    .strip_prefix("deadzone_percent=")?
                    .parse::<u8>()
                    .ok()?;
                if !AXIS_DEADZONE_STEPS.contains(&percent) {
                    return None;
                }
                Some(percent)
            }
        };
        if lines.next().is_some() {
            return None;
        }
        Some(Self {
            invert_x,
            invert_y,
            sensitivity_percent,
            deadzone_override_percent,
        })
    }

    /// Loads the active profile with game > controller-model > global precedence.
    pub fn load_for_context(
        settings_directory: &Path,
        device_uuid: Option<[u8; 16]>,
        game_identity: Option<GameIdentity>,
    ) -> Self {
        let global =
            load_axis_profile(&settings_directory.join(AXIS_GLOBAL_FILE)).unwrap_or_default();
        let device = device_uuid
            .filter(|uuid| !is_zero_uuid(uuid))
            .and_then(|uuid| {
                load_axis_profile(&axis_device_profile_path(settings_directory, uuid))
            });
        let game = game_identity.and_then(|identity| {
            load_axis_profile(&axis_game_profile_path(settings_directory, identity))
        });
        game.or(device).unwrap_or(global)
    }

    pub fn load_global(settings_directory: &Path) -> Self {
        load_axis_profile(&settings_directory.join(AXIS_GLOBAL_FILE)).unwrap_or_default()
    }

    pub fn load_for_game_or_global(settings_directory: &Path, identity: GameIdentity) -> Self {
        load_axis_profile(&axis_game_profile_path(settings_directory, identity))
            .unwrap_or_else(|| Self::load_global(settings_directory))
    }

    pub fn save_global(&self, settings_directory: &Path) -> io::Result<()> {
        save_axis_profile(&settings_directory.join(AXIS_GLOBAL_FILE), self)
    }

    pub fn save_for_device(
        &self,
        settings_directory: &Path,
        device_uuid: [u8; 16],
    ) -> io::Result<()> {
        if is_zero_uuid(&device_uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a zero controller UUID cannot identify an axis profile",
            ));
        }
        save_axis_profile(
            &axis_device_profile_path(settings_directory, device_uuid),
            self,
        )
    }

    pub fn save_for_game(
        &self,
        settings_directory: &Path,
        identity: GameIdentity,
    ) -> io::Result<()> {
        save_axis_profile(&axis_game_profile_path(settings_directory, identity), self)
    }
}

fn axis_device_profile_path(settings_directory: &Path, uuid: [u8; 16]) -> PathBuf {
    settings_directory
        .join(AXIS_DEVICE_DIRECTORY)
        .join(format!("{}.v1", uuid_hex(uuid)))
}

fn axis_game_profile_path(settings_directory: &Path, identity: GameIdentity) -> PathBuf {
    settings_directory
        .join(AXIS_GAME_DIRECTORY)
        .join(format!("{:016x}.v1", identity.title_id()))
}

fn load_axis_profile(path: &Path) -> Option<GamepadAxisSettings> {
    GamepadAxisSettings::from_config_str(&fs::read_to_string(path).ok()?)
}

fn save_axis_profile(path: &Path, settings: &GamepadAxisSettings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, settings.to_config_string())
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
    fn assigning_captured_control_moves_the_selected_destination_uniquely() {
        let mut mapping = GamepadMapping::default();
        mapping.assign_target_uniquely(GamepadControl::East, Some(PadButton::A));
        assert_eq!(mapping.target(GamepadControl::South), None);
        assert_eq!(mapping.target(GamepadControl::East), Some(PadButton::A));
        mapping.assign_target_uniquely(GamepadControl::North, None);
        assert_eq!(mapping.target(GamepadControl::East), Some(PadButton::A));
        assert_eq!(mapping.target(GamepadControl::North), None);
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
    fn axis_settings_round_trip_and_reject_malformed_values() {
        let mut settings = GamepadAxisSettings::default();
        settings.set_invert_x(true);
        settings.set_invert_y(true);
        settings.set_sensitivity_percent(175);
        assert_eq!(
            GamepadAxisSettings::from_config_str(&settings.to_config_string()),
            Some(settings)
        );
        assert!(
            GamepadAxisSettings::from_config_str(
                "ACERIS_GAMEPAD_AXIS_SETTINGS_V2\ninvert_x=0\ninvert_y=0\nsensitivity_percent=100\n"
            )
            .is_none()
        );
        assert!(
            GamepadAxisSettings::from_config_str(
                "ACERIS_GAMEPAD_AXIS_SETTINGS_V1\ninvert_x=1\ninvert_y=0\nsensitivity_percent=201\n"
            )
            .is_none()
        );
        assert!(GamepadAxisSettings::from_config_str(
            "ACERIS_GAMEPAD_AXIS_SETTINGS_V1\ninvert_x=0\ninvert_y=0\nsensitivity_percent=100\nextra=1\n"
        )
        .is_none());
        assert!(GamepadAxisSettings::from_config_str(
            "ACERIS_GAMEPAD_AXIS_SETTINGS_V1\ninvert_x=0\ninvert_y=0\nsensitivity_percent=100\ndeadzone_percent=9\n"
        )
        .is_none());
        assert!(GamepadAxisSettings::from_config_str(
            "ACERIS_GAMEPAD_AXIS_SETTINGS_V1\ninvert_x=0\ninvert_y=0\nsensitivity_percent=100\ndeadzone_percent=inherit\nextra=1\n"
        )
        .is_none());
    }

    #[test]
    fn axis_settings_apply_inversion_and_sensitivity() {
        let mut normal = GamepadAxisSettings::default();
        let baseline = normal.apply_to_circle_pad(0.5, 0.0, 16);
        assert!(baseline.0 > 0);

        normal.set_invert_x(true);
        assert_eq!(normal.apply_to_circle_pad(0.5, 0.0, 16).0, -baseline.0);

        normal.set_invert_x(false);
        normal.set_sensitivity_percent(50);
        let lower = normal.apply_to_circle_pad(0.5, 0.0, 16);
        normal.set_sensitivity_percent(200);
        let higher = normal.apply_to_circle_pad(0.5, 0.0, 16);
        assert!(lower.0 > 0 && lower.0 < baseline.0);
        assert!(higher.0 > baseline.0);
    }

    #[test]
    fn axis_deadzone_override_is_optional_backward_compatible_and_step_limited() {
        let legacy = "ACERIS_GAMEPAD_AXIS_SETTINGS_V1\ninvert_x=0\ninvert_y=0\nsensitivity_percent=100\n";
        let parsed = GamepadAxisSettings::from_config_str(legacy).unwrap();
        assert_eq!(parsed.deadzone_override_percent(), None);
        assert_eq!(parsed.effective_deadzone_percent(20), 20);

        let mut settings = parsed;
        assert!(settings.set_deadzone_override_percent(Some(24)));
        assert_eq!(settings.effective_deadzone_percent(8), 24);
        assert!(!settings.set_deadzone_override_percent(Some(9)));
        assert_eq!(settings.deadzone_override_percent(), Some(24));
        assert!(settings.set_deadzone_override_percent(None));
        assert_eq!(settings.deadzone_override_percent(), None);
        assert_eq!(GamepadAxisSettings::from_config_str(&settings.to_config_string()), Some(settings));
    }

    #[test]
    fn axis_deadzone_adjustment_uses_global_fallback_and_clamps_to_supported_steps() {
        let mut settings = GamepadAxisSettings::default();
        settings.adjust_deadzone_override(16, -1);
        assert_eq!(settings.deadzone_override_percent(), Some(12));
        settings.adjust_deadzone_override(16, 1);
        assert_eq!(settings.deadzone_override_percent(), Some(16));

        settings.set_deadzone_override_percent(Some(24));
        settings.adjust_deadzone_override(16, 1);
        assert_eq!(settings.deadzone_override_percent(), Some(24));
        settings.adjust_deadzone_override(16, -1);
        assert_eq!(settings.deadzone_override_percent(), Some(20));

        settings.set_deadzone_override_percent(None);
        settings.adjust_deadzone_override(99, -1);
        assert_eq!(settings.deadzone_override_percent(), Some(12));
    }

    #[test]
    fn axis_deadzone_override_changes_circle_pad_processing_only_when_set() {
        let inherited = GamepadAxisSettings::default();
        let low_global = inherited.apply_to_circle_pad(0.15, 0.0, 8);
        let high_global = inherited.apply_to_circle_pad(0.15, 0.0, 20);
        assert_ne!(low_global, high_global);

        let mut override_settings = GamepadAxisSettings::default();
        override_settings.set_deadzone_override_percent(Some(20));
        assert_eq!(override_settings.apply_to_circle_pad(0.15, 0.0, 8), high_global);
        assert_eq!(
            override_settings.apply_to_circle_pad(0.15, 0.0, 24),
            override_settings.apply_to_circle_pad(0.15, 0.0, 20)
        );
    }

    #[test]
    fn global_device_and_game_axis_profiles_follow_precedence() {
        let root =
            std::env::temp_dir().join(format!("aceris-axis-profiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut global = GamepadAxisSettings::default();
        global.set_invert_x(true);
        global.save_global(&root).unwrap();

        let device_uuid = [0x5A; 16];
        let mut device = GamepadAxisSettings::default();
        device.set_invert_y(true);
        device.save_for_device(&root, device_uuid).unwrap();

        let identity = GameIdentity::from_title_id(0x0004_0000_1234_5600).unwrap();
        let mut game = GamepadAxisSettings::default();
        game.set_sensitivity_percent(150);
        game.save_for_game(&root, identity).unwrap();

        assert!(GamepadAxisSettings::load_for_context(&root, None, None).invert_x());
        assert!(GamepadAxisSettings::load_for_context(&root, Some(device_uuid), None).invert_y());
        let active =
            GamepadAxisSettings::load_for_context(&root, Some(device_uuid), Some(identity));
        assert_eq!(active.sensitivity_percent(), 150);
        assert!(!active.invert_x());
        assert!(!active.invert_y());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn zero_uuid_cannot_write_an_axis_profile() {
        let root =
            std::env::temp_dir().join(format!("aceris-axis-zero-uuid-{}", std::process::id()));
        let error = GamepadAxisSettings::default()
            .save_for_device(&root, [0; 16])
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
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
