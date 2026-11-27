//! Persistent global preferences for the two-screen presentation layout.
//!
//! Portrait and landscape profiles are stored independently so rotating a
//! device does not overwrite the user's arrangement for the other orientation.

use super::frontend::ScreenLayoutMode;
use std::fs;
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

/// Global portrait and landscape preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenLayoutPreferences {
    pub portrait: ScreenLayoutProfile,
    pub landscape: ScreenLayoutProfile,
}

impl Default for ScreenLayoutPreferences {
    fn default() -> Self {
        Self {
            portrait: ScreenLayoutProfile::portrait_default(),
            landscape: ScreenLayoutProfile::landscape_default(),
        }
    }
}

const FILE_NAME: &str = "screen_layout_preferences_v1.txt";
const MAGIC: &str = "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1";

impl ScreenLayoutPreferences {
    /// Loads global settings; absent or malformed files use safe defaults.
    pub fn load(settings_directory: &Path) -> Self {
        let Ok(contents) = fs::read_to_string(settings_directory.join(FILE_NAME)) else {
            return Self::default();
        };
        Self::parse(&contents).unwrap_or_default()
    }

    /// Parses the complete versioned format, rejecting duplicate/unknown fields.
    pub fn parse(contents: &str) -> Option<Self> {
        let mut lines = contents.lines();
        if lines.next()? != MAGIC {
            return None;
        }

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
                _ => return None,
            }
        }

        Some(Self {
            portrait: ScreenLayoutProfile {
                mode: portrait_mode?,
                swap_screens: portrait_swap?,
            },
            landscape: ScreenLayoutProfile {
                mode: landscape_mode?,
                swap_screens: landscape_swap?,
            },
        })
    }

    /// Selects the profile matching the current surface orientation.
    pub fn profile_for_surface(&self, width: u32, height: u32) -> ScreenLayoutProfile {
        if width >= height {
            self.landscape
        } else {
            self.portrait
        }
    }

    /// Replaces only the profile matching the current surface orientation.
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

    /// Persists both orientation profiles under the supplied settings directory.
    pub fn save(&self, settings_directory: &Path) -> std::io::Result<()> {
        fs::create_dir_all(settings_directory)?;
        let contents = format!(
            "{MAGIC}\nportrait_mode={}\nportrait_swap={}\nlandscape_mode={}\nlandscape_swap={}\n",
            mode_name(self.portrait.mode),
            u8::from(self.portrait.swap_screens),
            mode_name(self.landscape.mode),
            u8::from(self.landscape.swap_screens),
        );
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
    fn profiles_round_trip_and_persist_independently() {
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

        preferences.save(&directory).unwrap();
        let reopened = ScreenLayoutPreferences::load(&directory);
        assert_eq!(reopened, preferences);

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn invalid_or_incomplete_files_fall_back_to_safe_defaults() {
        let invalid = [
            "",
            "wrong-header\nportrait_mode=vertical",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=vertical",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=vertical\nportrait_mode=top_only\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=unknown\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0",
            "ACERIS_SCREEN_LAYOUT_PREFERENCES_V1\nportrait_mode=vertical\nportrait_swap=0\nlandscape_mode=side_by_side\nlandscape_swap=0\nextra=1",
        ];
        for contents in invalid {
            assert_eq!(
                ScreenLayoutPreferences::parse(contents),
                None,
                "accepted malformed preferences: {contents:?}"
            );
        }
        assert_eq!(ScreenLayoutPreferences::parse(""), None);
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
}
