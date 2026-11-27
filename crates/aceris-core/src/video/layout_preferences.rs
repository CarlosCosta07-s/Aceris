//! Persistent layout preferences, with global defaults and per-title overrides.
//!
//! Portrait and landscape profiles are independent. A known title can override
//! either orientation; unidentified homebrew continues to inherit the global
//! profile rather than receiving a fabricated title identity.

use super::frontend::ScreenLayoutMode;
use crate::game_identity::GameIdentity;
use std::{collections::BTreeMap, fs};
use std::path::Path;

/// One layout profile for a particular surface orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenLayoutProfile {
    pub mode: ScreenLayoutMode,
    pub swap_screens: bool,
}

impl ScreenLayoutProfile {
    pub const fn portrait_default() -> Self {
        Self {
            mode: ScreenLayoutMode::Vertical,
            swap_screens: false,
        }
    }

    pub const fn landscape_default() -> Self {
        Self {
            mode: ScreenLayoutMode::SideBySide,
            swap_screens: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GameLayoutOverride {
    portrait: Option<ScreenLayoutProfile>,
    landscape: Option<ScreenLayoutProfile>,
}

/// Global orientation defaults and optional per-title overrides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenLayoutPreferences {
    pub portrait: ScreenLayoutProfile,
    pub landscape: ScreenLayoutProfile,
    game_overrides: BTreeMap<GameIdentity, GameLayoutOverride>,
}

impl Default for ScreenLayoutPreferences {
    fn default() -> Self {
        Self {
            portrait: ScreenLayoutProfile::portrait_default(),
            landscape: ScreenLayoutProfile::landscape_default(),
            game_overrides: BTreeMap::new(),
        }
    }
}

const FILE_NAME: &str = "screen_layout_preferences_v2.txt";
const LEGACY_FILE_NAME: &str = "screen_layout_preferences_v1.txt";
const MAGIC_V1: &str = "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1";
const MAGIC_V2: &str = "ACERIS_SCREEN_LAYOUT_PREFERENCES_V2";

impl ScreenLayoutPreferences {
    /// Loads v2 settings, or migrates the legacy global-only v1 file in memory.
    /// A malformed v2 file falls back to safe defaults rather than reviving stale v1 data.
    pub fn load(settings_directory: &Path) -> Self {
        let current_path = settings_directory.join(FILE_NAME);
        match fs::read_to_string(&current_path) {
            Ok(contents) => return Self::parse(&contents).unwrap_or_default(),
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Self::default();
            }
            Err(_) => {}
        }

        let Ok(legacy_contents) = fs::read_to_string(settings_directory.join(LEGACY_FILE_NAME)) else {
            return Self::default();
        };
        Self::parse(&legacy_contents).unwrap_or_default()
    }

    /// Parses both the legacy global-only v1 format and the v2 override format.
    /// Duplicate, malformed and unknown fields are rejected.
    pub fn parse(contents: &str) -> Option<Self> {
        let mut lines = contents.lines();
        let version = lines.next()?;
        let supports_games = version == MAGIC_V2;
        if !supports_games && version != MAGIC_V1 {
            return None;
        }

        let mut preferences = Self::default();
        let mut portrait_mode = None;
        let mut portrait_swap = None;
        let mut landscape_mode = None;
        let mut landscape_swap = None;

        for line in lines {
            let (key, value) = line.split_once('=')?;
            match (key, value) {
                ("portrait_mode", value) if portrait_mode.is_none() => {
                    portrait_mode = Some(parse_mode(value)?);
                }
                ("portrait_swap", "0") if portrait_swap.is_none() => {
                    portrait_swap = Some(false);
                }
                ("portrait_swap", "1") if portrait_swap.is_none() => {
                    portrait_swap = Some(true);
                }
                ("landscape_mode", value) if landscape_mode.is_none() => {
                    landscape_mode = Some(parse_mode(value)?);
                }
                ("landscape_swap", "0") if landscape_swap.is_none() => {
                    landscape_swap = Some(false);
                }
                ("landscape_swap", "1") if landscape_swap.is_none() => {
                    landscape_swap = Some(true);
                }
                ("game", value) if supports_games => {
                    let mut fields = value.split(',');
                    let identity = GameIdentity::parse_stable_key(fields.next()?)?;
                    let orientation = fields.next()?;
                    let profile = ScreenLayoutProfile {
                        mode: parse_mode(fields.next()?)?,
                        swap_screens: match fields.next()? {
                            "0" => false,
                            "1" => true,
                            _ => return None,
                        },
                    };
                    if fields.next().is_some() {
                        return None;
                    }
                    let entry = preferences.game_overrides.entry(identity).or_default();
                    let destination = match orientation {
                        "portrait" => &mut entry.portrait,
                        "landscape" => &mut entry.landscape,
                        _ => return None,
                    };
                    if destination.replace(profile).is_some() {
                        return None;
                    }
                }
                _ => return None,
            }
        }

        preferences.portrait = ScreenLayoutProfile {
            mode: portrait_mode?,
            swap_screens: portrait_swap?,
        };
        preferences.landscape = ScreenLayoutProfile {
            mode: landscape_mode?,
            swap_screens: landscape_swap?,
        };
        Some(preferences)
    }

    /// Selects the global profile matching the current surface orientation.
    pub fn profile_for_surface(&self, width: u32, height: u32) -> ScreenLayoutProfile {
        if width >= height {
            self.landscape
        } else {
            self.portrait
        }
    }

    /// Selects a title override when present, otherwise inheriting the global profile.
    pub fn profile_for_game(
        &self,
        identity: Option<GameIdentity>,
        width: u32,
        height: u32,
    ) -> ScreenLayoutProfile {
        let global = self.profile_for_surface(width, height);
        let Some(identity) = identity else {
            return global;
        };
        let Some(overrides) = self.game_overrides.get(&identity) else {
            return global;
        };
        if width >= height {
            overrides.landscape.unwrap_or(global)
        } else {
            overrides.portrait.unwrap_or(global)
        }
    }

    /// Replaces only the global profile matching the current surface orientation.
    pub fn set_profile_for_surface(
        &mut self,
        width: u32,
        height: u32,
        profile: ScreenLayoutProfile,
    ) {
        if width >= height {
            self.landscape = profile;
        } else {
            self.portrait = profile;
        }
    }

    /// Sets an override for a known title; no identity updates the global profile.
    pub fn set_profile_for_game(
        &mut self,
        identity: Option<GameIdentity>,
        width: u32,
        height: u32,
        profile: ScreenLayoutProfile,
    ) {
        let Some(identity) = identity else {
            self.set_profile_for_surface(width, height, profile);
            return;
        };
        let entry = self.game_overrides.entry(identity).or_default();
        if width >= height {
            entry.landscape = Some(profile);
        } else {
            entry.portrait = Some(profile);
        }
    }

    /// Returns true when a title has at least one saved orientation override.
    pub fn game_has_overrides(&self, identity: GameIdentity) -> bool {
        self.game_overrides.contains_key(&identity)
    }

    /// Removes all per-title layout overrides, leaving global defaults unchanged.
    pub fn reset_game_overrides(&mut self, identity: GameIdentity) -> bool {
        self.game_overrides.remove(&identity).is_some()
    }

    /// Persists globals and overrides deterministically in the versioned v2 format.
    pub fn save(&self, settings_directory: &Path) -> std::io::Result<()> {
        fs::create_dir_all(settings_directory)?;
        let mut contents = format!(
            "{MAGIC_V2}\nportrait_mode={}\nportrait_swap={}\nlandscape_mode={}\nlandscape_swap={}\n",
            mode_name(self.portrait.mode),
            u8::from(self.portrait.swap_screens),
            mode_name(self.landscape.mode),
            u8::from(self.landscape.swap_screens),
        );
        for (identity, overrides) in &self.game_overrides {
            let key = identity.stable_key();
            if let Some(profile) = overrides.portrait {
                contents.push_str(&format!(
                    "game={key},portrait,{},{}\n",
                    mode_name(profile.mode),
                    u8::from(profile.swap_screens),
                ));
            }
            if let Some(profile) = overrides.landscape {
                contents.push_str(&format!(
                    "game={key},landscape,{},{}\n",
                    mode_name(profile.mode),
                    u8::from(profile.swap_screens),
                ));
            }
        }
        fs::write(settings_directory.join(FILE_NAME), contents)
    }
}

fn mode_name(mode: ScreenLayoutMode) -> &'static str {
    match mode {
        ScreenLayoutMode::Auto => "auto",
        ScreenLayoutMode::SideBySide => "side_by_side",
        ScreenLayoutMode::Vertical => "vertical",
        ScreenLayoutMode::TopEnlarged => "top_enlarged",
        ScreenLayoutMode::BottomEnlarged => "bottom_enlarged",
        ScreenLayoutMode::TopOnly => "top_only",
        ScreenLayoutMode::BottomOnly => "bottom_only",
    }
}

fn parse_mode(value: &str) -> Option<ScreenLayoutMode> {
    Some(match value {
        "auto" => ScreenLayoutMode::Auto,
        "side_by_side" => ScreenLayoutMode::SideBySide,
        "vertical" => ScreenLayoutMode::Vertical,
        "top_enlarged" => ScreenLayoutMode::TopEnlarged,
        "bottom_enlarged" => ScreenLayoutMode::BottomEnlarged,
        "top_only" => ScreenLayoutMode::TopOnly,
        "bottom_only" => ScreenLayoutMode::BottomOnly,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aceris-layout-preferences-{}-{nonce}",
            std::process::id()
        ))
    }

    fn identity(title_id: u64) -> GameIdentity {
        GameIdentity::from_title_id(title_id).unwrap()
    }

    #[test]
    fn defaults_keep_portrait_and_landscape_profiles_separate() {
        let preferences = ScreenLayoutPreferences::default();
        assert_eq!(
            preferences.profile_for_surface(1080, 2400),
            ScreenLayoutProfile::portrait_default()
        );
        assert_eq!(
            preferences.profile_for_surface(2400, 1080),
            ScreenLayoutProfile::landscape_default()
        );
    }

    #[test]
    fn globals_and_game_overrides_round_trip_and_persist_independently() {
        let directory = temporary_directory();
        let mut preferences = ScreenLayoutPreferences::default();
        preferences.portrait = ScreenLayoutProfile {
            mode: ScreenLayoutMode::BottomEnlarged,
            swap_screens: true,
        };
        preferences.landscape = ScreenLayoutProfile {
            mode: ScreenLayoutMode::TopOnly,
            swap_screens: false,
        };
        let first = identity(0x0004_0000_1234_5600);
        let second = identity(0x0004_0000_1234_5601);
        preferences.set_profile_for_game(
            Some(first),
            1080,
            2400,
            ScreenLayoutProfile {
                mode: ScreenLayoutMode::TopEnlarged,
                swap_screens: false,
            },
        );
        preferences.set_profile_for_game(
            Some(first),
            2400,
            1080,
            ScreenLayoutProfile {
                mode: ScreenLayoutMode::BottomOnly,
                swap_screens: true,
            },
        );
        preferences.set_profile_for_game(
            Some(second),
            1080,
            2400,
            ScreenLayoutProfile {
                mode: ScreenLayoutMode::TopOnly,
                swap_screens: true,
            },
        );

        preferences.save(&directory).unwrap();
        let reopened = ScreenLayoutPreferences::load(&directory);
        assert_eq!(reopened, preferences);
        assert_eq!(
            reopened.profile_for_game(Some(first), 1080, 2400).mode,
            ScreenLayoutMode::TopEnlarged
        );
        assert_eq!(
            reopened.profile_for_game(Some(first), 2400, 1080).mode,
            ScreenLayoutMode::BottomOnly
        );
        assert_eq!(
            reopened.profile_for_game(Some(second), 1080, 2400).mode,
            ScreenLayoutMode::TopOnly
        );
        assert_eq!(reopened.profile_for_game(None, 1080, 2400), reopened.portrait);
        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn unknown_titles_inherit_globals_and_reset_removes_only_that_title() {
        let mut preferences = ScreenLayoutPreferences::default();
        let first = identity(0x0004_0000_1234_5600);
        let second = identity(0x0004_0000_1234_5601);
        let custom = ScreenLayoutProfile {
            mode: ScreenLayoutMode::TopOnly,
            swap_screens: true,
        };
        preferences.set_profile_for_game(Some(first), 1080, 2400, custom);
        assert_eq!(
            preferences.profile_for_game(None, 1080, 2400),
            ScreenLayoutProfile::portrait_default()
        );
        assert_eq!(
            preferences.profile_for_game(Some(second), 1080, 2400),
            ScreenLayoutProfile::portrait_default()
        );
        assert!(preferences.reset_game_overrides(first));
        assert!(!preferences.reset_game_overrides(first));
        assert!(!preferences.game_has_overrides(first));
        assert_eq!(
            preferences.profile_for_game(Some(first), 1080, 2400),
            ScreenLayoutProfile::portrait_default()
        );
    }

    #[test]
    fn legacy_v1_preferences_migrate_to_global_only_v2_state() {
        let legacy = "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=top_enlarged\nportrait_swap=1\nlandscape_mode=side_by_side\nlandscape_swap=0\n";
        let parsed = ScreenLayoutPreferences::parse(legacy).unwrap();
        assert_eq!(
            parsed.portrait,
            ScreenLayoutProfile {
                mode: ScreenLayoutMode::TopEnlarged,
                swap_screens: true,
            }
        );
        assert_eq!(parsed.landscape, ScreenLayoutProfile::landscape_default());
        assert!(parsed.game_overrides.is_empty());
    }

    #[test]
    fn invalid_or_duplicate_fields_fall_back_to_safe_defaults() {
        let invalid = [
            "",
            "wrong-header\nportrait_mode=vertical",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=vertical",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=vertical\nportrait_mode=top_only\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=unknown\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=vertical\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0\nextra=1",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V2\nportrait_mode=vertical\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0\ngame=title-v1:0004000012345600,portrait,top_only,0\ngame=title-v1:0004000012345600,portrait,vertical,0",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V2\nportrait_mode=vertical\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0\ngame=title-v1:00040000123456xz,portrait,top_only,0",
        ];
        for contents in invalid {
            assert_eq!(
                ScreenLayoutPreferences::parse(contents),
                None,
                "accepted malformed preferences: {contents:?}"
            );
        }
    }

    #[test]
    fn setting_one_orientation_does_not_mutate_the_other() {
        let mut preferences = ScreenLayoutPreferences::default();
        preferences.set_profile_for_surface(
            1080,
            2400,
            ScreenLayoutProfile {
                mode: ScreenLayoutMode::TopEnlarged,
                swap_screens: true,
            },
        );
        assert_eq!(
            preferences.profile_for_surface(1080, 2400).mode,
            ScreenLayoutMode::TopEnlarged
        );
        assert_eq!(
            preferences.profile_for_surface(2400, 1080),
            ScreenLayoutProfile::landscape_default()
        );
    }

    #[test]
    fn legacy_file_loads_before_first_v2_save_and_is_preserved() {
        let directory = temporary_directory();
        fs::create_dir_all(&directory).unwrap();
        let legacy = "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=top_enlarged\nportrait_swap=1\nlandscape_mode=side_by_side\nlandscape_swap=0\n";
        fs::write(directory.join(LEGACY_FILE_NAME), legacy).unwrap();
        let preferences = ScreenLayoutPreferences::load(&directory);
        assert_eq!(preferences.portrait.mode, ScreenLayoutMode::TopEnlarged);
        preferences.save(&directory).unwrap();
        assert!(directory.join(LEGACY_FILE_NAME).exists());
        assert_eq!(ScreenLayoutPreferences::load(&directory), preferences);
        fs::remove_dir_all(directory).ok();
    }
}
