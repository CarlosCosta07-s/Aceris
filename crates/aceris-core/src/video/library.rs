//! Game library screen: the list shown after the intro, where a game is chosen.
//!
//! This module only holds state and draws RGBA frames; frontends feed it
//! keyboard, mouse or touch input and decide what "launch" means.

use super::font::{self, draw_text};
use super::frontend::{VirtualControlPositions, VirtualControlPreset};
use crate::game_identity::GameIdentity;
use aceris_common::brand;
use std::fs;
use std::path::{Path, PathBuf};

/// Canvas size the layout is designed for.
pub const WIDTH: usize = 400;
/// Canvas height the layout is designed for.
pub const HEIGHT: usize = 480;

const HEADER_HEIGHT: i32 = 56;
const ROW_HEIGHT: i32 = 28;
const LIST_TOP: i32 = HEADER_HEIGHT + 8;
const FOOTER_TOP: i32 = 452;
/// Rows visible at once.
pub const VISIBLE_ROWS: usize = ((FOOTER_TOP - 22 - LIST_TOP) / ROW_HEIGHT) as usize;
const TEXT_SCALE: usize = 2;
const MARGIN: i32 = 20;

/// One game in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub path: PathBuf,
}

/// Which launcher page is currently visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibraryPage {
    #[default]
    Home,
    Games,
    Favorites,
    Recent,
    Homebrew,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LibraryViewMode {
    #[default]
    Grid,
    List,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LibraryPreferences {
    sort_descending: bool,
    view_mode: LibraryViewMode,
}

/// File-name extension filter for the game collection.
///
/// This describes the container extension only; it does not imply that every
/// file can be installed or that its contents are supported by the loader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibraryFormatFilter {
    #[default]
    All,
    ThreeDs,
    Cci,
    Cia,
    ThreeDsX,
}

impl LibraryFormatFilter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::ThreeDs,
            Self::ThreeDs => Self::Cci,
            Self::Cci => Self::Cia,
            Self::Cia => Self::ThreeDsX,
            Self::ThreeDsX => Self::All,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::All => "TODOS",
            Self::ThreeDs => ".3DS",
            Self::Cci => ".CCI",
            Self::Cia => ".CIA",
            Self::ThreeDsX => ".3DSX",
        }
    }

    fn matches_path(self, path: &Path) -> bool {
        let extension = path.extension().and_then(|value| value.to_str());
        match self {
            Self::All => true,
            Self::ThreeDs => extension.is_some_and(|value| value.eq_ignore_ascii_case("3ds")),
            Self::Cci => extension.is_some_and(|value| value.eq_ignore_ascii_case("cci")),
            Self::Cia => extension.is_some_and(|value| value.eq_ignore_ascii_case("cia")),
            Self::ThreeDsX => extension.is_some_and(|value| value.eq_ignore_ascii_case("3dsx")),
        }
    }
}

/// What a click did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickResult {
    /// The click hit nothing.
    Ignored,
    /// A row was selected.
    Selected,
    /// The already selected row was clicked again: launch it.
    Activated,
}

/// The profile scope used when the Android virtual-control editor changes a layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VirtualControlEditScope {
    /// Changes the shared layout inherited by games without a title override.
    #[default]
    Global,
    /// Changes only the currently loaded, validated title ID.
    Game,
}

/// Library state.
#[derive(Clone, Debug)]
pub struct Library {
    entries: Vec<Entry>,
    selected: usize,
    first_visible: usize,
    directory_label: String,
    scan_config: LibraryScanConfig,
    scan_settings_directory: PathBuf,
    status: Option<String>,
    page: LibraryPage,
    favorites: std::collections::HashSet<String>,
    recent: Vec<String>,
    sort_descending: bool,
    view_mode: LibraryViewMode,
    format_filter: LibraryFormatFilter,
    search_query: String,
    search_focused: bool,
    scan_path_input_focused: bool,
    touch_keyboard_enabled: bool,
    touch_keyboard_open: bool,
    virtual_controls_visible: bool,
    virtual_controls_opacity: u8,
    virtual_controls_size_percent: u8,
    virtual_controls_preset: VirtualControlPreset,
    /// Effective layout displayed by the currently loaded title.
    virtual_control_positions: VirtualControlPositions,
    /// Shared fallback layout inherited by titles without an override.
    global_virtual_control_positions: VirtualControlPositions,
    active_game_identity: Option<GameIdentity>,
    active_game_control_override: bool,
    virtual_control_edit_scope: VirtualControlEditScope,
    external_media_playing: bool,
    external_media_detection_available: bool,
    performance_mode: bool,
    gamepad_deadzone_percent: u8,
}

/// File in the settings folder that persists the opt-in performance mode.
const PERFORMANCE_MODE_FILE: &str = "performance_mode.txt";
const GAMEPAD_SETTINGS_FILE: &str = "gamepad_settings_v1.txt";
const GAMEPAD_SETTINGS_MAGIC: &str = "ACERIS_GAMEPAD_SETTINGS_V1";
const GAMEPAD_DEADZONE_STEPS: [u8; 5] = [8, 12, 16, 20, 24];
const VIRTUAL_CONTROLS_FILE: &str = "virtual_controls_v1.txt";
const VIRTUAL_CONTROLS_MAGIC: &str = "ACERIS_VIRTUAL_CONTROLS_V1";
const VIRTUAL_CONTROLS_OPACITY_FILE: &str = "virtual_controls_opacity_v1.txt";
const VIRTUAL_CONTROLS_OPACITY_MAGIC: &str = "ACERIS_VIRTUAL_CONTROLS_OPACITY_V1";
const VIRTUAL_CONTROLS_OPACITY_STEPS: [u8; 4] = [255, 204, 153, 102];
const VIRTUAL_CONTROLS_SIZE_FILE: &str = "virtual_controls_size_v1.txt";
const VIRTUAL_CONTROLS_SIZE_MAGIC: &str = "ACERIS_VIRTUAL_CONTROLS_SIZE_V1";
const VIRTUAL_CONTROLS_SIZE_STEPS: [u8; 3] = [80, 100, 120];
const VIRTUAL_CONTROLS_PRESET_FILE: &str = "virtual_controls_preset_v1.txt";
const VIRTUAL_CONTROLS_PRESET_MAGIC: &str = "ACERIS_VIRTUAL_CONTROLS_PRESET_V1";
const VIRTUAL_CONTROL_LAYOUT_FILE: &str = "virtual_control_layout_v2.txt";
const LEGACY_VIRTUAL_CONTROL_POSITIONS_FILE: &str = "virtual_control_positions_v1.txt";

/// Missing or malformed settings preserve the default visible-controls state.
fn load_virtual_controls_visible(settings_directory: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(settings_directory.join(VIRTUAL_CONTROLS_FILE)) else {
        return true;
    };
    let mut lines = contents.lines();
    if lines.next() != Some(VIRTUAL_CONTROLS_MAGIC) {
        return true;
    }
    match (lines.next(), lines.next()) {
        (Some("visible=1"), None) => true,
        (Some("visible=0"), None) => false,
        _ => true,
    }
}

fn load_virtual_controls_opacity(settings_directory: &Path) -> u8 {
    let Ok(contents) = fs::read_to_string(settings_directory.join(VIRTUAL_CONTROLS_OPACITY_FILE))
    else {
        return 255;
    };
    let mut lines = contents.lines();
    if lines.next() != Some(VIRTUAL_CONTROLS_OPACITY_MAGIC) {
        return 255;
    }
    let Some(value) = lines.next().and_then(|line| line.strip_prefix("opacity=")) else {
        return 255;
    };
    if lines.next().is_some() {
        return 255;
    }
    value.parse::<u8>().unwrap_or(255)
}

fn load_virtual_controls_size_percent(settings_directory: &Path) -> u8 {
    let Ok(contents) = fs::read_to_string(settings_directory.join(VIRTUAL_CONTROLS_SIZE_FILE))
    else {
        return 100;
    };
    let mut lines = contents.lines();
    if lines.next() != Some(VIRTUAL_CONTROLS_SIZE_MAGIC) {
        return 100;
    }
    let Some(value) = lines
        .next()
        .and_then(|line| line.strip_prefix("size_percent="))
    else {
        return 100;
    };
    if lines.next().is_some() {
        return 100;
    }
    match value.parse::<u8>() {
        Ok(size) if VIRTUAL_CONTROLS_SIZE_STEPS.contains(&size) => size,
        _ => 100,
    }
}

fn load_virtual_controls_preset(settings_directory: &Path) -> VirtualControlPreset {
    let Ok(contents) = fs::read_to_string(settings_directory.join(VIRTUAL_CONTROLS_PRESET_FILE))
    else {
        return VirtualControlPreset::Standard;
    };
    let mut lines = contents.lines();
    if lines.next() != Some(VIRTUAL_CONTROLS_PRESET_MAGIC) {
        return VirtualControlPreset::Standard;
    }
    let Some(value) = lines.next().and_then(|line| line.strip_prefix("preset=")) else {
        return VirtualControlPreset::Standard;
    };
    if lines.next().is_some() {
        return VirtualControlPreset::Standard;
    }
    VirtualControlPreset::from_config_value(value).unwrap_or_default()
}

fn load_virtual_control_positions(settings_directory: &Path) -> VirtualControlPositions {
    let current = fs::read_to_string(settings_directory.join(VIRTUAL_CONTROL_LAYOUT_FILE))
        .ok()
        .and_then(|contents| VirtualControlPositions::from_config_str(&contents));
    if let Some(positions) = current {
        return positions;
    }

    // Read the previous position-only file so existing custom layouts survive
    // the upgrade. Its V1 encoding defaults each individual button to 100%.
    fs::read_to_string(settings_directory.join(LEGACY_VIRTUAL_CONTROL_POSITIONS_FILE))
        .ok()
        .and_then(|contents| VirtualControlPositions::from_config_str(&contents))
        .unwrap_or_default()
}

/// Per-title layouts use only a validated 64-bit Title ID. Hexadecimal filenames
/// are line-safe and portable across Windows, Android, and desktop filesystems.
fn game_virtual_control_layout_path(settings_directory: &Path, identity: GameIdentity) -> PathBuf {
    settings_directory
        .join("virtual_control_profiles")
        .join(format!("{:016x}.v2", identity.title_id()))
}

fn load_game_virtual_control_positions(
    settings_directory: &Path,
    identity: GameIdentity,
) -> Option<VirtualControlPositions> {
    let contents = fs::read_to_string(game_virtual_control_layout_path(
        settings_directory,
        identity,
    ))
    .ok()?;
    VirtualControlPositions::from_config_str(&contents)
}

fn load_performance_mode(settings_directory: &Path) -> bool {
    fs::read_to_string(settings_directory.join(PERFORMANCE_MODE_FILE))
        .is_ok_and(|contents| contents.trim() == "1")
}

fn load_gamepad_deadzone_percent(settings_directory: &Path) -> u8 {
    let Ok(contents) = fs::read_to_string(settings_directory.join(GAMEPAD_SETTINGS_FILE)) else {
        return 16;
    };
    let mut lines = contents.lines();
    if lines.next() != Some(GAMEPAD_SETTINGS_MAGIC) {
        return 16;
    }
    let Some(value) = lines
        .next()
        .and_then(|line| line.strip_prefix("deadzone_percent="))
    else {
        return 16;
    };
    if lines.next().is_some() {
        return 16;
    }
    match value.parse::<u8>() {
        Ok(percent) if GAMEPAD_DEADZONE_STEPS.contains(&percent) => percent,
        _ => 16,
    }
}

/// Versioned prefix for full-path identifiers stored in Favorites and Recent.
const LIBRARY_PATH_ID_PREFIX: &str = "path-v1:";

/// Produces a line-safe identifier from the UTF-8 path used by a library entry.
/// Hex encoding preserves path bytes without introducing separators or newlines.
fn library_path_id(path: &Path) -> Option<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = String::from(LIBRARY_PATH_ID_PREFIX);
    for byte in path.to_str()?.as_bytes() {
        id.push(HEX[(*byte >> 4) as usize] as char);
        id.push(HEX[(*byte & 0x0f) as usize] as char);
    }
    Some(id)
}

fn is_valid_library_id(value: &str) -> bool {
    if let Some(encoded) = value.strip_prefix(LIBRARY_PATH_ID_PREFIX) {
        return !encoded.is_empty()
            && encoded.len().is_multiple_of(2)
            && encoded.bytes().all(|byte| byte.is_ascii_hexdigit());
    }

    // Older versions stored only a filename. Continue accepting that format
    // long enough to migrate it relative to the original single game folder.
    Path::new(value).file_name().and_then(|name| name.to_str()) == Some(value)
}

/// Reads path identifiers and legacy file names from the Aceris settings folder.
fn load_library_ids(path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .map(str::trim)
        .filter(|value| !value.is_empty() && is_valid_library_id(value))
        .map(str::to_string)
        .collect()
}

/// Converts old filename-only entries into paths under the original game folder.
/// Existing path identifiers are retained and duplicate identifiers are removed.
fn migrate_library_ids(ids: Vec<String>, legacy_directory: &Path) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .filter_map(|value| {
            let id = if value.starts_with(LIBRARY_PATH_ID_PREFIX) {
                value
            } else {
                library_path_id(&legacy_directory.join(value))?
            };
            seen.insert(id.clone()).then_some(id)
        })
        .collect()
}

fn write_library_ids(
    settings_directory: &Path,
    file_name: &str,
    values: impl IntoIterator<Item = String>,
) {
    if fs::create_dir_all(settings_directory).is_err() {
        return;
    }
    let content = values.into_iter().collect::<Vec<_>>().join("\n");
    let _ = fs::write(settings_directory.join(file_name), content);
}

/// Options controlling how the library scanner traverses user-selected folders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanOptions {
    /// Recursively inspect child directories. Disabled by default.
    pub recursive: bool,
}

/// Persistent configuration for the library scanner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryScanConfig {
    /// User-selected filesystem roots, kept in their configured order.
    pub directories: Vec<PathBuf>,
    /// Scanner traversal options.
    pub options: ScanOptions,
}

impl LibraryScanConfig {
    /// Creates a configuration containing one folder and the safe defaults.
    pub fn single_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directories: vec![normalize_scan_directory(&directory.into())],
            options: ScanOptions::default(),
        }
    }

    /// Adds a folder unless the same directory is already configured.
    ///
    /// Paths are normalized to absolute paths; existing folders are
    /// canonicalized so aliases do not create duplicate roots.
    pub fn add_directory(&mut self, directory: impl Into<PathBuf>) -> bool {
        let directory = directory.into();
        if directory.as_os_str().is_empty() {
            return false;
        }
        let directory = normalize_scan_directory(&directory);
        let identity = scan_path_identity(&directory);
        if self
            .directories
            .iter()
            .any(|existing| scan_path_identity(existing) == identity)
        {
            return false;
        }
        self.directories.push(directory);
        true
    }

    /// Removes a configured folder, but never permits an empty root list.
    pub fn remove_directory(&mut self, directory: &Path) -> bool {
        if self.directories.len() <= 1 {
            return false;
        }
        let identity = scan_path_identity(&normalize_scan_directory(directory));
        let Some(index) = self
            .directories
            .iter()
            .position(|existing| scan_path_identity(existing) == identity)
        else {
            return false;
        };
        self.directories.remove(index);
        true
    }
}

const LIBRARY_SCAN_CONFIG_FILE: &str = "library_scan_v1.txt";
const LIBRARY_SCAN_CONFIG_MAGIC: &str = "ACERIS_LIBRARY_SCAN_V1";
const LIBRARY_PREFERENCES_FILE: &str = "library_preferences_v1.txt";
const LIBRARY_PREFERENCES_MAGIC: &str = "ACERIS_LIBRARY_PREFERENCES_V1";

fn load_library_preferences(settings_directory: &Path) -> LibraryPreferences {
    let Ok(contents) = fs::read_to_string(settings_directory.join(LIBRARY_PREFERENCES_FILE)) else {
        return LibraryPreferences::default();
    };
    let mut lines = contents.lines();
    if lines.next() != Some(LIBRARY_PREFERENCES_MAGIC) {
        return LibraryPreferences::default();
    }

    let mut view_mode = None;
    let mut sort_descending = None;
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            return LibraryPreferences::default();
        };
        match (key, value) {
            ("view", "grid") if view_mode.is_none() => {
                view_mode = Some(LibraryViewMode::Grid);
            }
            ("view", "list") if view_mode.is_none() => {
                view_mode = Some(LibraryViewMode::List);
            }
            ("sort", "az") if sort_descending.is_none() => {
                sort_descending = Some(false);
            }
            ("sort", "za") if sort_descending.is_none() => {
                sort_descending = Some(true);
            }
            _ => return LibraryPreferences::default(),
        }
    }

    match (view_mode, sort_descending) {
        (Some(view_mode), Some(sort_descending)) => LibraryPreferences {
            sort_descending,
            view_mode,
        },
        _ => LibraryPreferences::default(),
    }
}

fn save_library_preferences(
    settings_directory: &Path,
    preferences: LibraryPreferences,
) -> std::io::Result<()> {
    fs::create_dir_all(settings_directory)?;
    let view_mode = match preferences.view_mode {
        LibraryViewMode::Grid => "grid",
        LibraryViewMode::List => "list",
    };
    let sort_order = if preferences.sort_descending {
        "za"
    } else {
        "az"
    };
    let contents = format!("{LIBRARY_PREFERENCES_MAGIC}\nview={view_mode}\nsort={sort_order}\n");
    fs::write(settings_directory.join(LIBRARY_PREFERENCES_FILE), contents)
}

fn normalize_scan_directory(directory: &Path) -> PathBuf {
    let absolute = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(directory))
            .unwrap_or_else(|_| directory.to_path_buf())
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}

fn deduplicate_scan_directories(directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut unique = Vec::new();
    for directory in directories {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let directory = normalize_scan_directory(directory);
        let identity = scan_path_identity(&directory);
        if seen.insert(identity) {
            unique.push(directory);
        }
    }
    unique
}

fn encode_scan_directory(directory: &Path) -> std::io::Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let directory = normalize_scan_directory(directory);
    let value = directory.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "library folder path is not valid UTF-8",
        )
    })?;
    if !directory.is_absolute() || value.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "library folder path must be absolute",
        ));
    }
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        encoded.push(HEX[(*byte >> 4) as usize] as char);
        encoded.push(HEX[(*byte & 0x0f) as usize] as char);
    }
    Ok(encoded)
}

fn decode_scan_directory(encoded: &str) -> Option<PathBuf> {
    if encoded.is_empty() || !encoded.len().is_multiple_of(2) {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        decoded.push(((high << 4) | low) as u8);
    }
    let value = String::from_utf8(decoded).ok()?;
    let directory = PathBuf::from(value);
    (directory.is_absolute() && !directory.as_os_str().is_empty()).then_some(directory)
}

/// Loads a saved multi-folder configuration.
///
/// Invalid or missing configuration falls back to the original game folder
/// with recursion disabled. Callers choose the settings directory so desktop
/// and mobile hosts can keep this file in their own writable app-data area.
pub fn load_library_scan_config(
    settings_directory: &Path,
    fallback_directory: &Path,
) -> LibraryScanConfig {
    let fallback = LibraryScanConfig::single_directory(fallback_directory.to_path_buf());
    let Ok(contents) = fs::read_to_string(settings_directory.join(LIBRARY_SCAN_CONFIG_FILE)) else {
        return fallback;
    };
    let mut lines = contents.lines();
    if lines.next() != Some(LIBRARY_SCAN_CONFIG_MAGIC) {
        return fallback;
    }

    let mut recursive = None;
    let mut directories = Vec::new();
    for line in lines {
        if let Some(value) = line.strip_prefix("recursive=") {
            if recursive.is_some() {
                return fallback;
            }
            recursive = match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => return fallback,
            };
        } else if let Some(value) = line.strip_prefix("folder=") {
            let Some(directory) = decode_scan_directory(value) else {
                return fallback;
            };
            directories.push(directory);
        } else {
            return fallback;
        }
    }

    let Some(recursive) = recursive else {
        return fallback;
    };
    let directories = deduplicate_scan_directories(&directories);
    if directories.is_empty() {
        return fallback;
    }

    LibraryScanConfig {
        directories,
        options: ScanOptions { recursive },
    }
}

/// Persists a scanner configuration using a versioned, line-safe text format.
///
/// Paths are stored as UTF-8 bytes encoded in hexadecimal. A non-UTF-8 path
/// returns InvalidInput instead of being truncated or stored ambiguously.
pub fn save_library_scan_config(
    settings_directory: &Path,
    config: &LibraryScanConfig,
) -> std::io::Result<()> {
    let directories = deduplicate_scan_directories(&config.directories);
    if directories.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "at least one library folder must be configured",
        ));
    }

    let mut contents = String::from(LIBRARY_SCAN_CONFIG_MAGIC);
    contents.push_str("\nrecursive=");
    contents.push_str(if config.options.recursive {
        "true"
    } else {
        "false"
    });
    contents.push('\n');
    for directory in directories {
        contents.push_str("folder=");
        contents.push_str(&encode_scan_directory(&directory)?);
        contents.push('\n');
    }

    fs::create_dir_all(settings_directory)?;
    fs::write(settings_directory.join(LIBRARY_SCAN_CONFIG_FILE), contents)
}

/// Scans every configured folder using the saved traversal options.
pub fn scan_games_from_config(config: &LibraryScanConfig) -> Vec<Entry> {
    scan_games_from_directories(&config.directories, config.options)
}

const MAX_SCAN_DEPTH: usize = 64;

/// Lists candidate executable images (`.3ds`, `.cci`, `.cia`, `.3dsx`) directly
/// inside `directory`, preserving the original non-recursive behavior.
pub fn scan_games(directory: &Path) -> Vec<Entry> {
    scan_games_from_directories(&[directory.to_path_buf()], ScanOptions::default())
}

/// Scans one or more user-selected directories.
///
/// Overlapping roots and repeated paths do not duplicate entries. When
/// `options.recursive` is enabled, child directories are traversed up to
/// `MAX_SCAN_DEPTH`; symbolic links encountered below a root are not followed.
/// Missing, non-directory, or unreadable roots are skipped independently so
/// one unavailable folder does not hide games in the remaining folders.
pub fn scan_games_from_directories(directories: &[PathBuf], options: ScanOptions) -> Vec<Entry> {
    let mut pending = Vec::new();
    let mut visited_directories = std::collections::HashSet::new();
    let mut seen_files = std::collections::HashSet::new();
    let mut entries = Vec::new();

    for directory in directories {
        let identity = scan_path_identity(directory);
        if visited_directories.insert(identity) {
            pending.push((directory.clone(), 0usize));
        }
    }

    while let Some((directory, depth)) = pending.pop() {
        let Ok(read) = fs::read_dir(&directory) else {
            continue;
        };

        for item in read {
            let Ok(item) = item else {
                continue;
            };
            let Ok(file_type) = item.file_type() else {
                continue;
            };
            let path = item.path();

            if file_type.is_dir() {
                if options.recursive && depth < MAX_SCAN_DEPTH {
                    let identity = scan_path_identity(&path);
                    if visited_directories.insert(identity) {
                        pending.push((path, depth + 1));
                    }
                }
                continue;
            }

            if !file_type.is_file() || !is_supported_game_image(&path) {
                continue;
            }

            let identity = scan_path_identity(&path);
            if !seen_files.insert(identity) {
                continue;
            }

            entries.push(Entry {
                title: path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path,
            });
        }
    }

    entries.sort_by(|left, right| {
        left.title
            .to_lowercase()
            .cmp(&right.title.to_lowercase())
            .then_with(|| {
                left.path
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(&right.path.to_string_lossy().to_lowercase())
            })
    });
    entries
}

fn is_supported_game_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("3ds")
                || ext.eq_ignore_ascii_case("cci")
                || ext.eq_ignore_ascii_case("3dsx")
                || ext.eq_ignore_ascii_case("cia")
        })
}

fn scan_path_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl Library {
    pub fn new(entries: Vec<Entry>, directory_label: String) -> Self {
        let directory = PathBuf::from(&directory_label);
        let settings_directory = directory.join(".aceris");
        let scan_config = LibraryScanConfig::single_directory(directory.clone());
        let legacy_favorites = load_library_ids(&settings_directory.join("favorites.txt"));
        let legacy_recent = load_library_ids(&settings_directory.join("recent.txt"));
        let favorite_ids = migrate_library_ids(legacy_favorites.clone(), &directory);
        let recent = migrate_library_ids(legacy_recent.clone(), &directory);

        // Rewrite legacy filename-only records once so later multi-folder scans
        // cannot accidentally attach them to a different file with the same name.
        if favorite_ids != legacy_favorites {
            write_library_ids(&settings_directory, "favorites.txt", favorite_ids.clone());
        }
        if recent != legacy_recent {
            write_library_ids(&settings_directory, "recent.txt", recent.clone());
        }

        let favorites = favorite_ids.into_iter().collect();
        let performance_mode = load_performance_mode(&settings_directory);
        let gamepad_deadzone_percent = load_gamepad_deadzone_percent(&settings_directory);
        let virtual_controls_visible = load_virtual_controls_visible(&settings_directory);
        let virtual_controls_opacity = load_virtual_controls_opacity(&settings_directory);
        let virtual_controls_size_percent = load_virtual_controls_size_percent(&settings_directory);
        let virtual_controls_preset = load_virtual_controls_preset(&settings_directory);
        let global_virtual_control_positions = load_virtual_control_positions(&settings_directory);
        let virtual_control_positions = global_virtual_control_positions;
        let preferences = load_library_preferences(&settings_directory);
        Self {
            entries,
            selected: 0,
            first_visible: 0,
            directory_label,
            scan_config,
            scan_settings_directory: settings_directory,
            status: None,
            page: LibraryPage::Home,
            favorites,
            recent,
            sort_descending: preferences.sort_descending,
            view_mode: preferences.view_mode,
            format_filter: LibraryFormatFilter::All,
            search_query: String::new(),
            search_focused: false,
            scan_path_input_focused: false,
            touch_keyboard_enabled: false,
            touch_keyboard_open: false,
            virtual_controls_visible,
            virtual_controls_opacity,
            virtual_controls_size_percent,
            virtual_controls_preset,
            virtual_control_positions,
            global_virtual_control_positions,
            active_game_identity: None,
            active_game_control_override: false,
            virtual_control_edit_scope: VirtualControlEditScope::Global,
            external_media_playing: false,
            external_media_detection_available: false,
            performance_mode,
            gamepad_deadzone_percent,
        }
    }

    /// Loads persisted folder configuration and scans it before creating the library.
    ///
    /// `settings_directory` must be writable by the host for later changes.
    /// `fallback_directory` preserves compatibility when no valid config exists.
    pub fn from_scan_settings(settings_directory: &Path, fallback_directory: &Path) -> Self {
        let scan_config = load_library_scan_config(settings_directory, fallback_directory);
        let entries = scan_games_from_config(&scan_config);
        let mut library = Self::new(entries, fallback_directory.display().to_string());
        library.scan_config = scan_config;
        library.scan_settings_directory = settings_directory.to_path_buf();
        library.performance_mode = load_performance_mode(settings_directory);
        library.gamepad_deadzone_percent = load_gamepad_deadzone_percent(settings_directory);
        library.virtual_controls_visible = load_virtual_controls_visible(settings_directory);
        library.virtual_controls_opacity = load_virtual_controls_opacity(settings_directory);
        library.virtual_controls_size_percent =
            load_virtual_controls_size_percent(settings_directory);
        library.virtual_controls_preset = load_virtual_controls_preset(settings_directory);
        let global_virtual_control_positions = load_virtual_control_positions(settings_directory);
        library.global_virtual_control_positions = global_virtual_control_positions;
        library.virtual_control_positions = global_virtual_control_positions;
        let preferences = load_library_preferences(settings_directory);
        library.sort_descending = preferences.sort_descending;
        library.view_mode = preferences.view_mode;
        library
    }

    /// Whether the opt-in "Desempenho" mode (fast memory without the precise
    /// D-cache model) is selected. Hosts apply it with `System::set_fast_memory`
    /// before loading a game. It is never enabled by default.
    pub fn performance_mode(&self) -> bool {
        self.performance_mode
    }

    /// Selects the performance mode and persists it in the settings folder.
    pub fn set_performance_mode(&mut self, enabled: bool) -> std::io::Result<()> {
        fs::create_dir_all(&self.scan_settings_directory)?;
        fs::write(
            self.scan_settings_directory.join(PERFORMANCE_MODE_FILE),
            if enabled { "1\n" } else { "0\n" },
        )?;
        self.performance_mode = enabled;
        self.status = Some(if enabled {
            "MODO DESEMPENHO ATIVADO (VALE NO PRÓXIMO JOGO)".to_string()
        } else {
            "MODO DESEMPENHO DESATIVADO".to_string()
        });
        Ok(())
    }

    fn toggle_performance_mode_from_settings(&mut self) {
        if let Err(error) = self.set_performance_mode(!self.performance_mode) {
            self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"));
        }
    }

    /// Current deadzone for physical controller sticks, as an integer percentage.
    pub fn gamepad_deadzone_percent(&self) -> u8 {
        self.gamepad_deadzone_percent
    }

    /// Persists a supported deadzone percentage for physical controller sticks.
    pub fn set_gamepad_deadzone_percent(&mut self, percent: u8) -> std::io::Result<()> {
        if !GAMEPAD_DEADZONE_STEPS.contains(&percent) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsupported gamepad deadzone percentage",
            ));
        }
        fs::create_dir_all(&self.scan_settings_directory)?;
        fs::write(
            self.scan_settings_directory.join(GAMEPAD_SETTINGS_FILE),
            format!("{GAMEPAD_SETTINGS_MAGIC}\ndeadzone_percent={percent}\n"),
        )?;
        self.gamepad_deadzone_percent = percent;
        self.status = Some(format!("ZONA MORTA DO ANALÓGICO: {percent}%"));
        Ok(())
    }

    fn cycle_gamepad_deadzone_from_settings(&mut self) {
        let index = GAMEPAD_DEADZONE_STEPS
            .iter()
            .position(|value| *value == self.gamepad_deadzone_percent)
            .unwrap_or(2);
        let next = GAMEPAD_DEADZONE_STEPS[(index + 1) % GAMEPAD_DEADZONE_STEPS.len()];
        if let Err(error) = self.set_gamepad_deadzone_percent(next) {
            self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"));
        }
    }

    /// Whether the Android virtual gamepad should be rendered and receive touch input.
    pub fn virtual_controls_visible(&self) -> bool {
        self.virtual_controls_visible
    }

    /// Persists whether the Android virtual gamepad is visible.
    pub fn set_virtual_controls_visible(&mut self, visible: bool) -> std::io::Result<()> {
        fs::create_dir_all(&self.scan_settings_directory)?;
        fs::write(
            self.scan_settings_directory.join(VIRTUAL_CONTROLS_FILE),
            format!("{VIRTUAL_CONTROLS_MAGIC}\nvisible={}\n", u8::from(visible)),
        )?;
        self.virtual_controls_visible = visible;
        self.status = Some(if visible {
            "CONTROLES DE TOQUE ATIVADOS".to_string()
        } else {
            "CONTROLES DE TOQUE OCULTOS".to_string()
        });
        Ok(())
    }

    fn toggle_virtual_controls_from_settings(&mut self) {
        if let Err(error) = self.set_virtual_controls_visible(!self.virtual_controls_visible) {
            self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"));
        }
    }

    /// Current overall opacity of the Android virtual gamepad.
    pub fn virtual_controls_opacity(&self) -> u8 {
        self.virtual_controls_opacity
    }

    /// Opacity formatted as an integer percentage for the settings UI.
    pub fn virtual_controls_opacity_percent(&self) -> u8 {
        ((u32::from(self.virtual_controls_opacity) * 100 + 127) / 255) as u8
    }

    /// Persists the overall virtual-control opacity without changing hit geometry.
    pub fn set_virtual_controls_opacity(&mut self, opacity: u8) -> std::io::Result<()> {
        fs::create_dir_all(&self.scan_settings_directory)?;
        fs::write(
            self.scan_settings_directory
                .join(VIRTUAL_CONTROLS_OPACITY_FILE),
            format!("{VIRTUAL_CONTROLS_OPACITY_MAGIC}\nopacity={opacity}\n"),
        )?;
        self.virtual_controls_opacity = opacity;
        self.status = Some(format!(
            "OPACIDADE DOS CONTROLES: {}%",
            self.virtual_controls_opacity_percent()
        ));
        Ok(())
    }

    fn cycle_virtual_controls_opacity_from_settings(&mut self) {
        let current = self.virtual_controls_opacity;
        let index = VIRTUAL_CONTROLS_OPACITY_STEPS
            .iter()
            .position(|value| *value == current)
            .unwrap_or(0);
        let next =
            VIRTUAL_CONTROLS_OPACITY_STEPS[(index + 1) % VIRTUAL_CONTROLS_OPACITY_STEPS.len()];
        if let Err(error) = self.set_virtual_controls_opacity(next) {
            self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"));
        }
    }

    /// Current overall size of the Android virtual gamepad, as a percentage.
    pub fn virtual_controls_size_percent(&self) -> u8 {
        self.virtual_controls_size_percent
    }

    /// Persists a supported virtual-control size percentage.
    pub fn set_virtual_controls_size_percent(&mut self, size_percent: u8) -> std::io::Result<()> {
        if !VIRTUAL_CONTROLS_SIZE_STEPS.contains(&size_percent) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unsupported virtual-control size",
            ));
        }
        fs::create_dir_all(&self.scan_settings_directory)?;
        fs::write(
            self.scan_settings_directory
                .join(VIRTUAL_CONTROLS_SIZE_FILE),
            format!("{VIRTUAL_CONTROLS_SIZE_MAGIC}\nsize_percent={size_percent}\n"),
        )?;
        self.virtual_controls_size_percent = size_percent;
        self.status = Some(format!("TAMANHO DOS CONTROLES: {size_percent}%"));
        Ok(())
    }

    fn cycle_virtual_controls_size_from_settings(&mut self) {
        let current = self.virtual_controls_size_percent;
        let index = VIRTUAL_CONTROLS_SIZE_STEPS
            .iter()
            .position(|value| *value == current)
            .unwrap_or(1);
        let next = VIRTUAL_CONTROLS_SIZE_STEPS[(index + 1) % VIRTUAL_CONTROLS_SIZE_STEPS.len()];
        if let Err(error) = self.set_virtual_controls_size_percent(next) {
            self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"));
        }
    }

    /// Selected global layout preset for the Android virtual gamepad.
    pub fn virtual_controls_preset(&self) -> VirtualControlPreset {
        self.virtual_controls_preset
    }

    /// Persists a global virtual-control layout preset.
    pub fn set_virtual_controls_preset(
        &mut self,
        preset: VirtualControlPreset,
    ) -> std::io::Result<()> {
        fs::create_dir_all(&self.scan_settings_directory)?;
        fs::write(
            self.scan_settings_directory
                .join(VIRTUAL_CONTROLS_PRESET_FILE),
            format!(
                "{VIRTUAL_CONTROLS_PRESET_MAGIC}\npreset={}\n",
                preset.config_value()
            ),
        )?;
        self.virtual_controls_preset = preset;
        self.status = Some(format!("PRESET DE CONTROLES: {}", preset.label()));
        Ok(())
    }

    fn cycle_virtual_controls_preset_from_settings(&mut self) {
        let index = VirtualControlPreset::ALL
            .iter()
            .position(|preset| *preset == self.virtual_controls_preset)
            .unwrap_or(0);
        let next = VirtualControlPreset::ALL[(index + 1) % VirtualControlPreset::ALL.len()];
        if let Err(error) = self.set_virtual_controls_preset(next) {
            self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"));
        }
    }

    /// Selects the active title and loads its saved control override, if one exists.
    ///
    /// Titles without a validated identity always inherit the global layout. Changing
    /// the active title does not write a profile; the first edit materializes an override.
    pub fn set_active_game_identity(&mut self, identity: Option<GameIdentity>) {
        if self.active_game_identity == identity {
            return;
        }
        self.active_game_identity = identity;
        self.virtual_control_edit_scope = if identity.is_some() {
            VirtualControlEditScope::Game
        } else {
            VirtualControlEditScope::Global
        };

        if let Some(identity) = identity {
            if let Some(positions) =
                load_game_virtual_control_positions(&self.scan_settings_directory, identity)
            {
                self.virtual_control_positions = positions;
                self.active_game_control_override = true;
            } else {
                self.virtual_control_positions = self.global_virtual_control_positions;
                self.active_game_control_override = false;
            }
        } else {
            self.virtual_control_positions = self.global_virtual_control_positions;
            self.active_game_control_override = false;
        }
    }

    /// Returns the validated identity of the currently active title, if available.
    pub fn active_game_identity(&self) -> Option<GameIdentity> {
        self.active_game_identity
    }

    /// Returns whether the active title has a persisted or pending control override.
    pub fn game_has_control_override(&self) -> bool {
        self.active_game_control_override
    }

    /// Current scope chosen for edits made by the virtual-control editor.
    pub fn virtual_control_edit_scope(&self) -> VirtualControlEditScope {
        self.virtual_control_edit_scope
    }

    /// Switches between global and per-title editing. Games without a validated
    /// title ID cannot enter per-title mode and remain on the global profile.
    pub fn toggle_virtual_control_edit_scope(&mut self) -> bool {
        if self.active_game_identity.is_none() {
            self.virtual_control_edit_scope = VirtualControlEditScope::Global;
            self.status =
                Some("ESTE JOGO NÃO TEM TITLE ID VALIDADO; PERFIL GLOBAL EM USO".to_string());
            return false;
        }
        self.virtual_control_edit_scope = match self.virtual_control_edit_scope {
            VirtualControlEditScope::Global => VirtualControlEditScope::Game,
            VirtualControlEditScope::Game => VirtualControlEditScope::Global,
        };
        self.status = Some(match self.virtual_control_edit_scope {
            VirtualControlEditScope::Global => "EDIÇÃO DE CONTROLES: PERFIL GLOBAL".to_string(),
            VirtualControlEditScope::Game => "EDIÇÃO DE CONTROLES: PERFIL DESTE JOGO".to_string(),
        });
        true
    }

    /// Returns the effective layout used while the game is running.
    pub fn virtual_control_positions(&self) -> &VirtualControlPositions {
        &self.virtual_control_positions
    }

    /// Returns the layout currently shown by the editor. This may be the global
    /// layout even while the running game has its own independent override.
    pub fn virtual_control_editor_positions(&self) -> &VirtualControlPositions {
        match self.virtual_control_edit_scope {
            VirtualControlEditScope::Global => &self.global_virtual_control_positions,
            VirtualControlEditScope::Game => &self.virtual_control_positions,
        }
    }

    /// Applies a transient drag delta in hundredths of a percent.
    ///
    /// The first per-title change copies the global profile, preserving inheritance
    /// until the user actually edits that game. Persist once after drag release.
    pub fn move_virtual_control(
        &mut self,
        orientation: u32,
        button: u32,
        delta_x: i16,
        delta_y: i16,
    ) -> bool {
        match self.virtual_control_edit_scope {
            VirtualControlEditScope::Game if self.active_game_identity.is_some() => {
                if self.active_game_control_override {
                    self.virtual_control_positions
                        .adjust(orientation, button, delta_x, delta_y)
                } else {
                    let mut positions = self.global_virtual_control_positions;
                    let previous = positions.offset(orientation, button);
                    let accepted = positions.adjust(orientation, button, delta_x, delta_y);
                    if accepted && positions.offset(orientation, button) != previous {
                        self.virtual_control_positions = positions;
                        self.active_game_control_override = true;
                    }
                    accepted
                }
            }
            _ => {
                let changed = self.global_virtual_control_positions.adjust(
                    orientation,
                    button,
                    delta_x,
                    delta_y,
                );
                if !self.active_game_control_override {
                    self.virtual_control_positions = self.global_virtual_control_positions;
                }
                changed
            }
        }
    }

    /// Updates one button's scale during the editor gesture; the host saves on release.
    pub fn resize_virtual_control(
        &mut self,
        orientation: u32,
        button: u32,
        size_percent: u8,
    ) -> bool {
        match self.virtual_control_edit_scope {
            VirtualControlEditScope::Game if self.active_game_identity.is_some() => {
                if self.active_game_control_override {
                    self.virtual_control_positions.set_size_percent(
                        orientation,
                        button,
                        size_percent,
                    )
                } else {
                    let mut positions = self.global_virtual_control_positions;
                    let previous = positions.size_percent(orientation, button);
                    let accepted = positions.set_size_percent(orientation, button, size_percent);
                    if accepted && positions.size_percent(orientation, button) != previous {
                        self.virtual_control_positions = positions;
                        self.active_game_control_override = true;
                    }
                    accepted
                }
            }
            _ => {
                let changed = self.global_virtual_control_positions.set_size_percent(
                    orientation,
                    button,
                    size_percent,
                );
                if !self.active_game_control_override {
                    self.virtual_control_positions = self.global_virtual_control_positions;
                }
                changed
            }
        }
    }

    /// Saves the selected scope without overwriting the other profile.
    pub fn save_virtual_control_positions(&mut self) -> std::io::Result<()> {
        if self.virtual_control_edit_scope == VirtualControlEditScope::Game
            && self.active_game_identity.is_some()
            && !self.active_game_control_override
        {
            self.status = Some("PERFIL DO JOGO CONTINUA HERDANDO O PADRÃO GLOBAL".to_string());
            return Ok(());
        }

        let destination = match (self.virtual_control_edit_scope, self.active_game_identity) {
            (VirtualControlEditScope::Game, Some(identity)) => {
                game_virtual_control_layout_path(&self.scan_settings_directory, identity)
            }
            _ => self
                .scan_settings_directory
                .join(VIRTUAL_CONTROL_LAYOUT_FILE),
        };
        fs::create_dir_all(
            destination
                .parent()
                .expect("control profile paths always have a parent directory"),
        )?;
        let positions = match self.virtual_control_edit_scope {
            VirtualControlEditScope::Game if self.active_game_identity.is_some() => {
                &self.virtual_control_positions
            }
            _ => &self.global_virtual_control_positions,
        };
        fs::write(destination, positions.to_config_string())?;
        if self.virtual_control_edit_scope == VirtualControlEditScope::Game
            && self.active_game_identity.is_some()
        {
            self.active_game_control_override = true;
        } else if !self.active_game_control_override {
            self.virtual_control_positions = self.global_virtual_control_positions;
        }
        self.status = Some("LAYOUT DOS CONTROLES SALVO".to_string());
        Ok(())
    }

    /// Restores the selected orientation to the default layout in the chosen scope.
    pub fn reset_virtual_control_positions(&mut self, orientation: u32) -> std::io::Result<()> {
        match self.virtual_control_edit_scope {
            VirtualControlEditScope::Game if self.active_game_identity.is_some() => {
                if self.active_game_control_override {
                    self.virtual_control_positions
                        .reset_orientation(orientation);
                } else {
                    let mut positions = self.global_virtual_control_positions;
                    positions.reset_orientation(orientation);
                    if positions != self.global_virtual_control_positions {
                        self.virtual_control_positions = positions;
                        self.active_game_control_override = true;
                    }
                }
            }
            _ => {
                self.global_virtual_control_positions
                    .reset_orientation(orientation);
                if !self.active_game_control_override {
                    self.virtual_control_positions = self.global_virtual_control_positions;
                }
            }
        }
        self.save_virtual_control_positions()
    }

    /// Removes the active title's entire override and returns it to global inheritance.
    pub fn reset_game_virtual_control_profile(&mut self) -> std::io::Result<bool> {
        let Some(identity) = self.active_game_identity else {
            return Ok(false);
        };
        let path = game_virtual_control_layout_path(&self.scan_settings_directory, identity);
        let removed_file = match fs::remove_file(path) {
            Ok(()) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        let had_override = self.active_game_control_override;
        self.active_game_control_override = false;
        self.virtual_control_positions = self.global_virtual_control_positions;
        self.status = Some(if had_override || removed_file {
            "PERFIL DE CONTROLES DO JOGO RESTAURADO".to_string()
        } else {
            "O JOGO JÁ USA O PADRÃO GLOBAL".to_string()
        });
        Ok(had_override || removed_file)
    }

    /// Current persisted roots and scanner options.
    pub fn scan_config(&self) -> &LibraryScanConfig {
        &self.scan_config
    }

    /// Adds a game folder and immediately persists and rescans the library.
    ///
    /// Returns `Ok(false)` when the folder is already configured.
    pub fn add_scan_directory(&mut self, directory: impl Into<PathBuf>) -> std::io::Result<bool> {
        let mut updated = self.scan_config.clone();
        if !updated.add_directory(directory) {
            return Ok(false);
        }
        save_library_scan_config(&self.scan_settings_directory, &updated)?;
        self.scan_config = updated;
        self.reload();
        self.status = Some("PASTA ADICIONADA À BIBLIOTECA".to_string());
        Ok(true)
    }

    /// Removes a configured folder and immediately persists and rescans.
    ///
    /// Returns `Ok(false)` if the folder is unknown or it is the last root.
    pub fn remove_scan_directory(&mut self, directory: &Path) -> std::io::Result<bool> {
        let mut updated = self.scan_config.clone();
        if !updated.remove_directory(directory) {
            return Ok(false);
        }
        save_library_scan_config(&self.scan_settings_directory, &updated)?;
        self.scan_config = updated;
        self.reload();
        self.status = Some("PASTA REMOVIDA DA BIBLIOTECA".to_string());
        Ok(true)
    }

    /// Enables or disables recursive scanning and immediately persists and rescans.
    ///
    /// Returns `Ok(false)` when the requested value is already active.
    pub fn set_recursive_scan(&mut self, recursive: bool) -> std::io::Result<bool> {
        if self.scan_config.options.recursive == recursive {
            return Ok(false);
        }
        let mut updated = self.scan_config.clone();
        updated.options.recursive = recursive;
        save_library_scan_config(&self.scan_settings_directory, &updated)?;
        self.scan_config = updated;
        self.reload();
        self.status = Some(if recursive {
            "BUSCA RECURSIVA ATIVADA".to_string()
        } else {
            "BUSCA RECURSIVA DESATIVADA".to_string()
        });
        Ok(true)
    }

    fn toggle_recursive_from_settings(&mut self) {
        match self.set_recursive_scan(!self.scan_config.options.recursive) {
            Ok(true) | Ok(false) => {}
            Err(error) => {
                self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A CONFIGURAÇÃO: {error}"))
            }
        }
    }

    fn remove_scan_directory_from_settings(&mut self, index: usize) {
        let Some(directory) = self.scan_config.directories.get(index).cloned() else {
            return;
        };
        match self.remove_scan_directory(&directory) {
            Ok(true) => {}
            Ok(false) => {
                self.status = Some("MANTENHA AO MENOS UMA PASTA NA BIBLIOTECA".to_string())
            }
            Err(error) => self.status = Some(format!("NÃO FOI POSSÍVEL REMOVER A PASTA: {error}")),
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        if self.page == LibraryPage::Home {
            return self.recent.iter().find_map(|id| {
                self.entries
                    .iter()
                    .find(|entry| library_path_id(&entry.path).as_deref() == Some(id.as_str()))
            });
        }
        self.filtered_entries().get(self.selected).copied()
    }

    /// Number of entries shown by the current library section.
    pub fn visible_entry_count(&self) -> usize {
        self.filtered_entries().len()
    }

    /// True when an entry is in the user's persistent favorites collection.
    pub fn is_favorite(&self, entry: &Entry) -> bool {
        library_path_id(&entry.path).is_some_and(|id| self.favorites.contains(&id))
    }

    /// Adds or removes the currently selected item from Favorites.
    pub fn toggle_selected_favorite(&mut self) -> bool {
        let Some(entry) = self.selected_entry() else {
            return false;
        };
        let Some(id) = library_path_id(&entry.path) else {
            return false;
        };
        if !self.favorites.remove(&id) {
            self.favorites.insert(id);
        }
        self.persist_library_ids("favorites.txt", self.favorites.iter().cloned());
        self.status = Some("Favoritos atualizados".to_string());
        true
    }

    /// Records a successfully launched game at the front of the persistent Recent list.
    pub fn mark_launched(&mut self, path: &Path) {
        let Some(id) = library_path_id(path) else {
            return;
        };
        self.recent.retain(|item| item != &id);
        self.recent.insert(0, id);
        self.recent.truncate(24);
        self.persist_library_ids("recent.txt", self.recent.iter().cloned());
        self.status = None;
    }

    /// Number of existing game entries currently saved in Favorites.
    pub fn favorite_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| self.is_favorite(entry))
            .count()
    }

    /// Number of valid game entries kept in Recent.
    pub fn recent_count(&self) -> usize {
        self.recent
            .iter()
            .filter(|id| {
                self.entries
                    .iter()
                    .any(|entry| library_path_id(&entry.path).as_deref() == Some(id.as_str()))
            })
            .count()
    }

    fn persist_view_preferences(&mut self) {
        let preferences = LibraryPreferences {
            sort_descending: self.sort_descending,
            view_mode: self.view_mode,
        };
        if let Err(error) = save_library_preferences(&self.scan_settings_directory, preferences) {
            self.status = Some(format!(
                "NÃO FOI POSSÍVEL SALVAR AS PREFERÊNCIAS DA BIBLIOTECA: {error}"
            ));
        }
    }

    /// Switches between A–Z and Z–A; the Recent section keeps its time order.
    pub fn toggle_sort_order(&mut self) {
        self.sort_descending = !self.sort_descending;
        self.selected = 0;
        self.first_visible = 0;
        self.status = Some(
            if self.sort_descending {
                "Ordem: Z–A"
            } else {
                "Ordem: A–Z"
            }
            .to_string(),
        );
        self.persist_view_preferences();
    }

    /// Advances the file-extension filter and resets the visible selection.
    pub fn cycle_format_filter(&mut self) {
        self.format_filter = self.format_filter.next();
        self.selected = 0;
        self.first_visible = 0;
        self.status = Some(format!("Filtro de formato: {}", self.format_filter.label()));
    }

    /// Current extension filter applied to collection pages.
    pub fn format_filter(&self) -> LibraryFormatFilter {
        self.format_filter
    }

    /// Switches the landscape collection between a compact grid and a readable list.
    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            LibraryViewMode::Grid => LibraryViewMode::List,
            LibraryViewMode::List => LibraryViewMode::Grid,
        };
        self.selected = 0;
        self.first_visible = 0;
        self.status = Some(
            match self.view_mode {
                LibraryViewMode::Grid => "Visualização: grade",
                LibraryViewMode::List => "Visualização: lista",
            }
            .to_string(),
        );
        self.persist_view_preferences();
    }

    /// True when the responsive landscape collection is displayed as a list.
    pub fn is_list_view(&self) -> bool {
        self.view_mode == LibraryViewMode::List
    }

    /// Enables a touch-operated on-screen keyboard for hosts without a system text field.
    pub fn set_touch_keyboard_enabled(&mut self, enabled: bool) {
        self.touch_keyboard_enabled = enabled;
        if !enabled {
            self.touch_keyboard_open = false;
        }
    }

    /// Current search text, shared by the desktop and Android library.
    pub fn search_query(&self) -> &str {
        &self.search_query
    }

    /// True while keyboard text should be sent to the search or folder-path field.
    pub fn search_is_focused(&self) -> bool {
        self.search_focused || self.scan_path_input_focused
    }

    /// True while the built-in touch keyboard is visible.
    pub fn touch_keyboard_is_open(&self) -> bool {
        self.touch_keyboard_open
    }

    /// Gives the search field focus; touch hosts also open the built-in keyboard.
    pub fn focus_search(&mut self) {
        self.scan_path_input_focused = false;
        self.search_focused = true;
        self.touch_keyboard_open = self.touch_keyboard_enabled;
        self.status = None;
    }

    /// Opens the folder-path field in Settings. Confirming adds the directory if it exists.
    pub fn focus_scan_path(&mut self) {
        if self.page != LibraryPage::Settings {
            return;
        }
        self.search_focused = false;
        self.scan_path_input_focused = true;
        self.search_query.clear();
        self.touch_keyboard_open = self.touch_keyboard_enabled;
        self.status = Some("DIGITE O CAMINHO DA PASTA E CONFIRME".to_string());
    }

    /// Opens the folder settings and focuses the existing Rust path-entry flow.
    ///
    /// This is not the Android SAF picker: the current NativeActivity callback
    /// table cannot receive an activity result. The picker is deliberately not
    /// launched until a result bridge exists, so a selection is never discarded.
    pub fn begin_add_folder_path(&mut self) {
        self.select_page(LibraryPage::Settings);
        self.focus_scan_path();
    }

    /// Appends printable text from a physical/system keyboard or folder-path field.
    pub fn input_search_char(&mut self, character: char) -> bool {
        let limit = if self.scan_path_input_focused {
            1024
        } else {
            96
        };
        if (!self.search_focused && !self.scan_path_input_focused)
            || character.is_control()
            || self.search_query.chars().count() >= limit
        {
            return false;
        }
        self.search_query.push(character);
        if !self.scan_path_input_focused {
            self.reset_search_selection();
        }
        true
    }

    /// Removes one Unicode scalar from the current search.
    pub fn backspace_search(&mut self) -> bool {
        if (!self.search_focused && !self.scan_path_input_focused)
            || self.search_query.pop().is_none()
        {
            return false;
        }
        if !self.scan_path_input_focused {
            self.reset_search_selection();
        }
        true
    }

    /// Clears the active search query or folder path without closing text entry.
    pub fn clear_search(&mut self) {
        if !self.search_query.is_empty() {
            self.search_query.clear();
            if !self.scan_path_input_focused {
                self.reset_search_selection();
            }
        }
    }

    /// Cancels the active text field without adding a pending folder path.
    pub fn cancel_text_entry(&mut self) {
        if self.scan_path_input_focused {
            self.scan_path_input_focused = false;
            self.search_query.clear();
            self.status = Some("ADIÇÃO DE PASTA CANCELADA".to_string());
        } else {
            self.search_focused = false;
        }
        self.touch_keyboard_open = false;
    }

    /// Closes text entry. A confirmed folder path is added only when it names an
    /// existing directory; an invalid path leaves the active library intact.
    pub fn finish_search(&mut self) {
        if self.scan_path_input_focused {
            self.scan_path_input_focused = false;
            self.touch_keyboard_open = false;
            let value = std::mem::take(&mut self.search_query);
            let value = value.trim();
            if value.is_empty() {
                self.status = Some("NENHUM CAMINHO INFORMADO".to_string());
                return;
            }
            let directory = PathBuf::from(value);
            if !directory.is_dir() {
                self.status = Some("PASTA NÃO ENCONTRADA OU SEM ACESSO".to_string());
                return;
            }
            match self.add_scan_directory(directory) {
                Ok(true) => {}
                Ok(false) => self.status = Some("ESSA PASTA JÁ ESTÁ NA BIBLIOTECA".to_string()),
                Err(error) => {
                    self.status = Some(format!("NÃO FOI POSSÍVEL SALVAR A PASTA: {error}"))
                }
            }
            return;
        }
        self.search_focused = false;
        self.touch_keyboard_open = false;
    }

    fn reset_search_selection(&mut self) {
        self.selected = 0;
        self.first_visible = 0;
        self.status = None;
    }

    /// Updates the external-media indicator shown on the home screen.
    pub fn set_external_media_playing(&mut self, playing: bool) {
        self.external_media_playing = playing;
    }

    pub fn external_media_playing(&self) -> bool {
        self.external_media_playing
    }

    /// Indicates whether the Android host can reliably detect external media.
    pub fn set_external_media_detection_available(&mut self, available: bool) {
        self.external_media_detection_available = available;
        if !available {
            self.external_media_playing = false;
        }
    }

    pub fn external_media_detection_available(&self) -> bool {
        self.external_media_detection_available
    }

    fn persist_library_ids(&self, file_name: &str, values: impl IntoIterator<Item = String>) {
        let directory = Path::new(&self.directory_label).join(".aceris");
        write_library_ids(&directory, file_name, values);
    }

    /// Changes the library section and resets its independent selection.
    pub fn select_page(&mut self, page: LibraryPage) {
        if self.page != page {
            if self.scan_path_input_focused {
                self.cancel_text_entry();
            }
            self.page = page;
            self.selected = 0;
            self.first_visible = 0;
            self.status = None;
        }
    }

    fn entry_matches_search(&self, entry: &Entry) -> bool {
        let query = fold_search_text(&self.search_query);
        query.is_empty()
            || fold_search_text(&entry.title).contains(&query)
            || fold_search_text(&entry.path.to_string_lossy()).contains(&query)
    }

    fn entry_matches_page(&self, entry: &Entry) -> bool {
        let is_homebrew = entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("3dsx"));
        match self.page {
            LibraryPage::Home => false,
            LibraryPage::Games => !is_homebrew,
            LibraryPage::Favorites => self.is_favorite(entry),
            LibraryPage::Recent => self
                .recent
                .iter()
                .any(|id| library_path_id(&entry.path).as_deref() == Some(id.as_str())),
            LibraryPage::Homebrew => is_homebrew,
            LibraryPage::Settings => false,
        }
    }

    fn filtered_entries(&self) -> Vec<&Entry> {
        if self.page == LibraryPage::Recent {
            let entries: Vec<&Entry> = self
                .recent
                .iter()
                .filter_map(|id| {
                    self.entries
                        .iter()
                        .find(|entry| library_path_id(&entry.path).as_deref() == Some(id.as_str()))
                })
                .filter(|entry| {
                    self.entry_matches_search(entry) && self.format_filter.matches_path(&entry.path)
                })
                .collect();
            return entries;
        }
        let mut entries: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| {
                self.entry_matches_page(entry)
                    && self.entry_matches_search(entry)
                    && self.format_filter.matches_path(&entry.path)
            })
            .collect();
        if self.sort_descending {
            entries.reverse();
        }
        entries
    }

    /// Shows a message (for example a load error) above the footer.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Current user-visible library status or loading diagnostic, if any.
    pub fn status_message(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Moves the selection by `delta` rows, clamped to the list.
    pub fn move_selection(&mut self, delta: i32) {
        let count = self.visible_entry_count();
        if count == 0 {
            return;
        }
        let last = count as i64 - 1;
        self.selected = (self.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
        self.status = None;
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + VISIBLE_ROWS {
            self.first_visible = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    /// Handles a click or tap at canvas coordinates.
    pub fn click(&mut self, x: i32, y: i32) -> ClickResult {
        if x < MARGIN - 8 || x >= WIDTH as i32 - MARGIN + 8 || y < LIST_TOP {
            return ClickResult::Ignored;
        }
        let row = ((y - LIST_TOP) / ROW_HEIGHT) as usize;
        let index = self.first_visible + row;
        if row >= VISIBLE_ROWS || index >= self.entries.len() {
            return ClickResult::Ignored;
        }
        if index == self.selected {
            ClickResult::Activated
        } else {
            self.move_selection(index as i32 - self.selected as i32);
            ClickResult::Selected
        }
    }

    /// Current launcher page.
    pub fn page(&self) -> LibraryPage {
        self.page
    }

    /// Re-scans every configured game folder without restarting the application.
    pub fn reload(&mut self) {
        self.entries = scan_games_from_config(&self.scan_config);
        self.selected = 0;
        self.first_visible = 0;
        self.status = Some(format!(
            "BIBLIOTECA ATUALIZADA: {} JOGO(S)",
            self.entries.len()
        ));
    }

    /// Moves selection using the number of rows actually visible on this surface.
    pub fn move_selection_responsive(&mut self, delta: i32, width: u32, height: u32) {
        let Some((portrait, _scale, _, logical_height)) = responsive_metrics(width, height) else {
            return;
        };
        if portrait {
            let visible = ((logical_height - 350.0).max(58.0) / 58.0).floor().max(1.0) as usize;
            self.move_selection_with_visible(delta, visible);
        } else if self.view_mode == LibraryViewMode::List {
            let visible_rows = ((logical_height - 174.0) / 54.0).floor().max(1.0) as usize;
            self.move_selection_with_visible(delta, visible_rows);
        } else {
            let visible_rows = landscape_grid_visible_rows(logical_height);
            let visible_items = visible_rows * 4;
            let delta = if delta.abs() == 1 { delta * 4 } else { delta };
            self.move_grid_selection(delta, visible_items);
        }
    }

    fn move_grid_selection(&mut self, delta: i32, visible_items: usize) {
        let count = self.visible_entry_count();
        if count == 0 {
            return;
        }
        self.selected =
            (self.selected as i64 + i64::from(delta)).clamp(0, count as i64 - 1) as usize;
        self.status = None;
        let visible_items = visible_items.max(4);
        if self.selected < self.first_visible {
            self.first_visible = (self.selected / 4) * 4;
        } else if self.selected >= self.first_visible + visible_items {
            let needed = self.selected + 1 - visible_items;
            self.first_visible = needed.div_ceil(4) * 4;
        }
    }

    fn move_selection_with_visible(&mut self, delta: i32, visible_rows: usize) {
        let count = self.visible_entry_count();
        if count == 0 {
            return;
        }
        let last = count as i64 - 1;
        self.selected = (self.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
        self.status = None;
        let visible_rows = visible_rows.max(1);
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + visible_rows {
            self.first_visible = self.selected + 1 - visible_rows;
        }
    }

    /// Moves one card horizontally in the landscape collection grid.
    pub fn move_selection_horizontal_responsive(&mut self, delta: i32, width: u32, height: u32) {
        let Some((portrait, _, _, logical_height)) = responsive_metrics(width, height) else {
            return;
        };
        if portrait {
            let visible = ((logical_height - 350.0).max(58.0) / 58.0).floor().max(1.0) as usize;
            self.move_selection_with_visible(delta, visible);
        } else if self.view_mode == LibraryViewMode::Grid {
            let visible_rows = landscape_grid_visible_rows(logical_height);
            self.move_grid_selection(delta, visible_rows * 4);
        }
    }

    /// Handles navigation, refresh and game selection in the responsive launcher.
    /// Coordinates are supplied in physical surface pixels.
    pub fn click_responsive(&mut self, x: i32, y: i32, width: u32, height: u32) -> ClickResult {
        let Some((portrait, scale, _, logical_height)) = responsive_metrics(width, height) else {
            return ClickResult::Ignored;
        };
        let (x, y) = (x as f32 / scale, y as f32 / scale);

        if self.touch_keyboard_open {
            if self.handle_touch_keyboard_tap(x, y, logical_height) {
                return ClickResult::Ignored;
            }
            self.finish_search();
        }

        if !portrait
            && matches!(
                self.page,
                LibraryPage::Games
                    | LibraryPage::Favorites
                    | LibraryPage::Recent
                    | LibraryPage::Homebrew
            )
            && (88.0..124.0).contains(&y)
            && (128.0..970.0).contains(&x)
        {
            if x >= 760.0 {
                self.cycle_format_filter();
            } else if x >= 710.0 {
                self.clear_search();
                self.focus_search();
            } else {
                self.focus_search();
            }
            return ClickResult::Ignored;
        }

        if portrait {
            // Portrait rendering remains a fallback for non-Android hosts.
            if (100.0..150.0).contains(&y) {
                let page = if x < 134.0 {
                    LibraryPage::Games
                } else if x < 266.0 {
                    LibraryPage::Homebrew
                } else {
                    LibraryPage::Settings
                };
                self.select_page(page);
                return ClickResult::Ignored;
            }
            if self.page == LibraryPage::Settings {
                let settings_offset = if self.touch_keyboard_enabled {
                    48.0
                } else {
                    0.0
                };
                for index in 0..self.scan_config.directories.len().min(3) {
                    let row_top = 300.0 + index as f32 * 32.0;
                    if (row_top..row_top + 28.0).contains(&y) && (334.0..374.0).contains(&x) {
                        self.remove_scan_directory_from_settings(index);
                        return ClickResult::Ignored;
                    }
                }
                if (430.0..474.0).contains(&y) && (38.0..362.0).contains(&x) {
                    self.toggle_recursive_from_settings();
                } else if (478.0..522.0).contains(&y) && (38.0..362.0).contains(&x) {
                    self.focus_scan_path();
                } else if (546.0..590.0).contains(&y) && (38.0..362.0).contains(&x) {
                    if self.touch_keyboard_enabled {
                        self.toggle_virtual_controls_from_settings();
                    } else {
                        self.reload();
                    }
                } else if self.touch_keyboard_enabled
                    && (594.0..638.0).contains(&y)
                    && (38.0..142.0).contains(&x)
                {
                    self.cycle_virtual_controls_opacity_from_settings();
                } else if self.touch_keyboard_enabled
                    && (594.0..638.0).contains(&y)
                    && (148.0..252.0).contains(&x)
                {
                    self.cycle_virtual_controls_size_from_settings();
                } else if self.touch_keyboard_enabled
                    && (594.0..638.0).contains(&y)
                    && (258.0..362.0).contains(&x)
                {
                    self.cycle_virtual_controls_preset_from_settings();
                } else if (594.0 + settings_offset..638.0 + settings_offset).contains(&y)
                    && (38.0..198.0).contains(&x)
                {
                    self.toggle_performance_mode_from_settings();
                } else if (594.0 + settings_offset..638.0 + settings_offset).contains(&y)
                    && (202.0..362.0).contains(&x)
                {
                    self.cycle_gamepad_deadzone_from_settings();
                } else if logical_height >= 740.0 + settings_offset
                    && (648.0 + settings_offset..690.0 + settings_offset).contains(&y)
                    && (38.0..362.0).contains(&x)
                {
                    self.toggle_view_mode();
                } else if logical_height >= 740.0 + settings_offset
                    && (692.0 + settings_offset..734.0 + settings_offset).contains(&y)
                    && (38.0..362.0).contains(&x)
                {
                    self.toggle_sort_order();
                }
                return ClickResult::Ignored;
            }
            // The large "Atualizar biblioteca" button exists only on the empty
            // card; with games listed the same band holds game rows, which
            // must open the game rather than rescan the folder.
            let (refresh_top, refresh_bottom) = empty_library_refresh_band(logical_height);
            if ((168.0..222.0).contains(&y) && x >= 304.0)
                || (self.visible_entry_count() == 0
                    && (refresh_top..refresh_bottom).contains(&y)
                    && (38.0..362.0).contains(&x))
            {
                self.reload();
                return ClickResult::Ignored;
            }
            if (222.0..248.0).contains(&y)
                && (20.0..380.0).contains(&x)
                && matches!(
                    self.page,
                    LibraryPage::Games
                        | LibraryPage::Favorites
                        | LibraryPage::Recent
                        | LibraryPage::Homebrew
                )
            {
                self.cycle_format_filter();
                return ClickResult::Ignored;
            }
            if y < 250.0 {
                return ClickResult::Ignored;
            }
            let row_height = 58.0;
            let visible = ((logical_height - 350.0).max(row_height) / row_height)
                .floor()
                .max(1.0) as usize;
            let index = self.first_visible + ((y - 250.0) / row_height).floor().max(0.0) as usize;
            if index >= self.first_visible + visible || index >= self.visible_entry_count() {
                return ClickResult::Ignored;
            }
            return self.select_or_activate(index);
        }

        // Console-style horizontal navigation shared by desktop and landscape Android.
        if y < 68.0 {
            let page = if (18.0..112.0).contains(&x) || (120.0..204.0).contains(&x) {
                Some(LibraryPage::Home)
            } else if (210.0..310.0).contains(&x) {
                Some(LibraryPage::Recent)
            } else if (316.0..410.0).contains(&x) {
                Some(LibraryPage::Games)
            } else if (416.0..566.0).contains(&x) {
                Some(LibraryPage::Homebrew)
            } else if (572.0..692.0).contains(&x) {
                Some(LibraryPage::Favorites)
            } else if (875.0..982.0).contains(&x) {
                Some(LibraryPage::Settings)
            } else {
                None
            };
            if let Some(page) = page {
                self.select_page(page);
            }
            return ClickResult::Ignored;
        }

        // The selected-game actions remain available regardless of the current tab.
        if (760.0..980.0).contains(&x) {
            if (238.0..282.0).contains(&y) {
                if self.selected_entry().is_some() {
                    return ClickResult::Activated;
                }
            } else if (286.0..328.0).contains(&y) {
                if self.selected_entry().is_some() {
                    self.toggle_selected_favorite();
                }
                return ClickResult::Ignored;
            } else if (336.0..378.0).contains(&y) {
                self.begin_add_folder_path();
                return ClickResult::Ignored;
            } else if (384.0..426.0).contains(&y) {
                self.reload();
                return ClickResult::Ignored;
            }
        }

        match self.page {
            LibraryPage::Home => {
                if (132.0..298.0).contains(&y) && (28.0..730.0).contains(&x) {
                    if self.recent_count() > 0 {
                        self.selected = 0;
                        return ClickResult::Activated;
                    }
                    if self.entries.is_empty() {
                        self.begin_add_folder_path();
                    } else {
                        self.select_page(LibraryPage::Games);
                    }
                } else if (340.0..430.0).contains(&y) {
                    if (28.0..248.0).contains(&x) {
                        self.select_page(LibraryPage::Games);
                    } else if (258.0..478.0).contains(&x) {
                        self.select_page(LibraryPage::Favorites);
                    } else if (488.0..730.0).contains(&x) {
                        self.select_page(LibraryPage::Homebrew);
                    }
                }
                ClickResult::Ignored
            }
            LibraryPage::Settings => {
                for index in 0..self.scan_config.directories.len().min(3) {
                    let row_top = 178.0 + index as f32 * 28.0;
                    if (row_top..row_top + 25.0).contains(&y) && (670.0..724.0).contains(&x) {
                        self.remove_scan_directory_from_settings(index);
                        return ClickResult::Ignored;
                    }
                }

                if (280.0..322.0).contains(&y) {
                    if (28.0..245.0).contains(&x) {
                        self.focus_scan_path();
                    } else if (255.0..475.0).contains(&x) {
                        self.toggle_recursive_from_settings();
                    } else if (485.0..595.0).contains(&x) {
                        self.toggle_performance_mode_from_settings();
                    } else if (605.0..730.0).contains(&x) {
                        self.cycle_gamepad_deadzone_from_settings();
                    }
                } else if (396.0..428.0).contains(&y) {
                    if (28.0..245.0).contains(&x) {
                        self.toggle_view_mode();
                    } else if (255.0..475.0).contains(&x) {
                        self.toggle_sort_order();
                    } else if self.touch_keyboard_enabled && (485.0..730.0).contains(&x) {
                        self.toggle_virtual_controls_from_settings();
                    }
                } else if self.touch_keyboard_enabled && (438.0..480.0).contains(&y) {
                    if (485.0..564.0).contains(&x) {
                        self.cycle_virtual_controls_opacity_from_settings();
                    } else if (568.0..647.0).contains(&x) {
                        self.cycle_virtual_controls_size_from_settings();
                    } else if (651.0..730.0).contains(&x) {
                        self.cycle_virtual_controls_preset_from_settings();
                    }
                }
                ClickResult::Ignored
            }
            LibraryPage::Games
            | LibraryPage::Favorites
            | LibraryPage::Recent
            | LibraryPage::Homebrew => {
                if self.visible_entry_count() == 0
                    && (500.0..692.0).contains(&x)
                    && (315.0..357.0).contains(&y)
                {
                    self.begin_add_folder_path();
                    return ClickResult::Ignored;
                }

                if (126.0..158.0).contains(&y) {
                    if (36.0..365.0).contains(&x) {
                        self.focus_search();
                    } else if (368.0..402.0).contains(&x) {
                        self.clear_search();
                    } else if (410.0..518.0).contains(&x) {
                        self.cycle_format_filter();
                    } else if (526.0..622.0).contains(&x) {
                        self.toggle_view_mode();
                    } else if (630.0..722.0).contains(&x) {
                        self.toggle_sort_order();
                    }
                    return ClickResult::Ignored;
                }

                if self.view_mode == LibraryViewMode::List {
                    let list_top = 170.0;
                    let row_step = 54.0;
                    let row_height = 48.0;
                    let visible_rows = ((logical_height - 18.0 - list_top - row_height) / row_step)
                        .floor()
                        .max(0.0) as usize
                        + 1;
                    if !(28.0..730.0).contains(&x) || y < list_top {
                        return ClickResult::Ignored;
                    }
                    let row = ((y - list_top) / row_step).floor().max(0.0) as usize;
                    let row_y = list_top + row as f32 * row_step;
                    let index = self.first_visible + row;
                    if row >= visible_rows
                        || y >= row_y + row_height
                        || index >= self.visible_entry_count()
                    {
                        return ClickResult::Ignored;
                    }
                    if (680.0..722.0).contains(&x) && (row_y + 8.0..row_y + 40.0).contains(&y) {
                        self.selected = index;
                        self.toggle_selected_favorite();
                        return ClickResult::Ignored;
                    }
                    return self.select_or_activate(index);
                }

                const COLUMNS: usize = 4;
                let card_top = 170.0;
                let card_height = 122.0;
                let row_step = 132.0;
                let card_width = 168.0;
                let card_gap = 10.0;
                if y < card_top || !(28.0..730.0).contains(&x) {
                    return ClickResult::Ignored;
                }
                let row = ((y - card_top) / row_step).floor().max(0.0) as usize;
                let column = ((x - 28.0) / (card_width + card_gap)).floor().max(0.0) as usize;
                if column >= COLUMNS {
                    return ClickResult::Ignored;
                }
                let card_x = 28.0 + column as f32 * (card_width + card_gap);
                let card_y = card_top + row as f32 * row_step;
                if x < card_x || x >= card_x + card_width || y >= card_y + card_height {
                    return ClickResult::Ignored;
                }
                let visible_rows = landscape_grid_visible_rows(logical_height);
                let index = self.first_visible + row * COLUMNS + column;
                if row >= visible_rows || index >= self.visible_entry_count() {
                    return ClickResult::Ignored;
                }
                self.selected = index;
                if x >= card_x + card_width - 30.0 && y <= card_y + 28.0 {
                    self.toggle_selected_favorite();
                    return ClickResult::Ignored;
                }
                self.select_or_activate(index)
            }
        }
    }

    fn select_or_activate(&mut self, index: usize) -> ClickResult {
        if index == self.selected {
            ClickResult::Activated
        } else {
            self.selected = index;
            self.status = None;
            ClickResult::Selected
        }
    }

    /// Renders the launcher directly at the physical surface size.
    pub fn render_responsive(&self, width: u32, height: u32) -> Vec<u8> {
        self.render_responsive_with_background(width, height, None)
    }

    /// Renders the responsive launcher with an optional user-provided background frame.
    pub fn render_responsive_with_background(
        &self,
        width: u32,
        height: u32,
        background: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut frame = Vec::new();
        self.render_responsive_into_with_background(width, height, &mut frame, background);
        frame
    }

    /// Renders into a caller-owned RGBA buffer, retaining its allocation.
    pub fn render_responsive_into(&self, width: u32, height: u32, frame: &mut Vec<u8>) -> bool {
        self.render_responsive_into_with_background(width, height, frame, None)
    }

    /// Renders into a reusable frame and optionally composites a 400x480 user background.
    pub fn render_responsive_into_with_background(
        &self,
        width: u32,
        height: u32,
        frame: &mut Vec<u8>,
        background: Option<&[u8]>,
    ) -> bool {
        let Some((portrait, scale, logical_width, logical_height)) =
            responsive_metrics(width, height)
        else {
            frame.clear();
            return false;
        };
        let Some(byte_len) = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
        else {
            frame.clear();
            return false;
        };
        if frame.capacity() < byte_len {
            let additional = byte_len.saturating_sub(frame.len());
            if frame.try_reserve_exact(additional).is_err() {
                frame.clear();
                return false;
            }
        }
        frame.resize(byte_len, 255);
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            0.0,
            logical_width,
            logical_height,
            scale,
            brand::PURPLE_DARK,
        );
        if let Some(background) = background {
            responsive_blend_background(frame, width, height, background);
        }
        if portrait {
            self.render_portrait(frame, width, height, scale, logical_height);
        } else {
            self.render_landscape(frame, width, height, scale, logical_height);
        }
        true
    }

    fn render_portrait(
        &self,
        frame: &mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        logical_height: f32,
    ) {
        let font_scale = (scale * 1.7).round().max(2.0) as usize;
        let small_scale = (scale * 1.15).round().max(1.0) as usize;

        // Brand header.
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            0.0,
            400.0,
            84.0,
            scale,
            0x160D22,
        );
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            82.0,
            400.0,
            3.0,
            scale,
            brand::ACCENT_RED,
        );
        responsive_text(
            frame,
            width,
            height,
            22.0,
            14.0,
            font_scale,
            "ACERIS",
            brand::TEXT,
        );
        responsive_text(
            frame,
            width,
            height,
            24.0,
            54.0,
            small_scale,
            "Emulador de Nintendo 3DS",
            0xBBAFC7,
        );

        // Three clear sections: installed games, homebrew, and settings.
        let tabs = [
            (16.0, 116.0, LibraryPage::Games, "Jogos", 56.0),
            (142.0, 116.0, LibraryPage::Homebrew, "Homebrew", 152.0),
            (268.0, 116.0, LibraryPage::Settings, "Config.", 300.0),
        ];
        for (x, tab_width, page, label, text_x) in tabs {
            responsive_panel(
                frame,
                width,
                height,
                x,
                101.0,
                tab_width,
                44.0,
                scale,
                if self.page == page {
                    brand::ACCENT_RED
                } else {
                    0x251731
                },
            );
            responsive_text(
                frame,
                width,
                height,
                text_x,
                116.0,
                small_scale,
                label,
                brand::TEXT,
            );
        }

        let page_title = match self.page {
            LibraryPage::Home => "Início",
            LibraryPage::Games => "Biblioteca",
            LibraryPage::Favorites => "Favoritos",
            LibraryPage::Recent => "Recentes",
            LibraryPage::Homebrew => "Homebrew e demos",
            LibraryPage::Settings => "Configurações",
        };
        responsive_text(
            frame,
            width,
            height,
            22.0,
            174.0,
            font_scale,
            page_title,
            brand::TEXT,
        );

        if self.page == LibraryPage::Settings {
            responsive_text(
                frame,
                width,
                height,
                22.0,
                212.0,
                small_scale,
                "Gerencie as pastas da sua biblioteca",
                0xBBAFC7,
            );
            responsive_panel(
                frame,
                width,
                height,
                20.0,
                250.0,
                360.0,
                if self.touch_keyboard_enabled {
                    540.0
                } else {
                    360.0
                },
                scale,
                0x21152F,
            );
            responsive_text(
                frame,
                width,
                height,
                38.0,
                278.0,
                small_scale,
                &format!(
                    "Pastas monitoradas ({})",
                    self.scan_config.directories.len()
                ),
                brand::ACCENT_GREEN_LIGHT,
            );
            for (index, directory) in self.scan_config.directories.iter().take(3).enumerate() {
                let row_y = 300.0 + index as f32 * 32.0;
                responsive_panel(
                    frame, width, height, 34.0, row_y, 340.0, 28.0, scale, 0x33223F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    42.0,
                    row_y + 8.0,
                    small_scale,
                    &tail(&directory.to_string_lossy(), 34),
                    brand::TEXT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    334.0,
                    row_y + 2.0,
                    36.0,
                    24.0,
                    scale,
                    0x422038,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    352.0,
                    row_y + 7.0,
                    small_scale,
                    "X",
                    brand::TEXT,
                );
            }
            if self.scan_config.directories.len() > 3 {
                responsive_text(
                    frame,
                    width,
                    height,
                    42.0,
                    400.0,
                    small_scale,
                    &format!(
                        "+ {} outra(s) pasta(s)",
                        self.scan_config.directories.len() - 3
                    ),
                    0xBBAFC7,
                );
            }
            responsive_panel(
                frame,
                width,
                height,
                38.0,
                430.0,
                324.0,
                44.0,
                scale,
                if self.scan_config.options.recursive {
                    0x28543F
                } else {
                    0x2D1A3B
                },
            );
            responsive_text_centered(
                frame,
                width,
                height,
                200.0,
                444.0,
                small_scale,
                if self.scan_config.options.recursive {
                    "BUSCA EM SUBPASTAS: ATIVADA"
                } else {
                    "BUSCA EM SUBPASTAS: DESATIVADA"
                },
                brand::TEXT,
            );
            responsive_panel(
                frame,
                width,
                height,
                38.0,
                478.0,
                324.0,
                44.0,
                scale,
                if self.scan_path_input_focused {
                    brand::ACCENT_RED
                } else {
                    0x2D1A3B
                },
            );
            responsive_text_centered(
                frame,
                width,
                height,
                200.0,
                492.0,
                small_scale,
                "ADICIONAR PASTA POR CAMINHO",
                brand::TEXT,
            );
            if self.scan_path_input_focused {
                responsive_text(
                    frame,
                    width,
                    height,
                    38.0,
                    532.0,
                    small_scale,
                    &truncate(&self.search_query, 42),
                    brand::ACCENT_GREEN_LIGHT,
                );
            }
            responsive_panel(
                frame,
                width,
                height,
                38.0,
                546.0,
                324.0,
                40.0,
                scale,
                if self.touch_keyboard_enabled && self.virtual_controls_visible {
                    0x28543F
                } else {
                    brand::ACCENT_RED
                },
            );
            responsive_text_centered(
                frame,
                width,
                height,
                200.0,
                559.0,
                small_scale,
                if self.touch_keyboard_enabled {
                    if self.virtual_controls_visible {
                        "CONTROLES DE TOQUE: ATIVOS"
                    } else {
                        "CONTROLES DE TOQUE: OCULTOS"
                    }
                } else {
                    "ATUALIZAR BIBLIOTECA"
                },
                brand::TEXT,
            );
            let settings_offset = if self.touch_keyboard_enabled {
                48.0
            } else {
                0.0
            };
            if logical_height >= 640.0 + settings_offset {
                responsive_panel(
                    frame,
                    width,
                    height,
                    38.0,
                    594.0 + settings_offset,
                    158.0,
                    44.0,
                    scale,
                    if self.performance_mode {
                        0x28543F
                    } else {
                        0x2D1A3B
                    },
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    117.0,
                    608.0 + settings_offset,
                    small_scale,
                    if self.performance_mode {
                        "DESEMPENHO: SIM"
                    } else {
                        "DESEMPENHO: NÃO"
                    },
                    brand::TEXT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    204.0,
                    594.0 + settings_offset,
                    158.0,
                    44.0,
                    scale,
                    0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    283.0,
                    608.0 + settings_offset,
                    small_scale,
                    &format!("ZONA MORTA: {}%", self.gamepad_deadzone_percent),
                    brand::TEXT,
                );
            }
            if logical_height >= 740.0 + settings_offset {
                responsive_text(
                    frame,
                    width,
                    height,
                    38.0,
                    642.0 + settings_offset,
                    small_scale,
                    "PREFERÊNCIAS DA BIBLIOTECA",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    38.0,
                    650.0 + settings_offset,
                    324.0,
                    38.0,
                    scale,
                    0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    663.0 + settings_offset,
                    small_scale,
                    if self.view_mode == LibraryViewMode::Grid {
                        "GRADE (PAISAGEM)"
                    } else {
                        "LISTA (PAISAGEM)"
                    },
                    brand::TEXT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    38.0,
                    694.0 + settings_offset,
                    324.0,
                    38.0,
                    scale,
                    0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    707.0 + settings_offset,
                    small_scale,
                    if self.sort_descending {
                        "ORDEM: Z–A"
                    } else {
                        "ORDEM: A–Z"
                    },
                    brand::TEXT,
                );
            }
            if self.touch_keyboard_enabled {
                responsive_panel(
                    frame, width, height, 38.0, 594.0, 104.0, 40.0, scale, 0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    90.0,
                    607.0,
                    small_scale,
                    &format!("OP: {}%", self.virtual_controls_opacity_percent()),
                    brand::TEXT,
                );
                responsive_panel(
                    frame, width, height, 148.0, 594.0, 104.0, 40.0, scale, 0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    607.0,
                    small_scale,
                    &format!("TAM: {}%", self.virtual_controls_size_percent()),
                    brand::TEXT,
                );
                responsive_panel(
                    frame, width, height, 258.0, 594.0, 104.0, 40.0, scale, 0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    310.0,
                    607.0,
                    small_scale,
                    self.virtual_controls_preset.label(),
                    brand::TEXT,
                );
            }
        } else {
            let count = self.visible_entry_count();
            let count_text = match (self.page, count) {
                (LibraryPage::Homebrew, 1) => "1 homebrew disponível".to_string(),
                (LibraryPage::Homebrew, _) => format!("{count} homebrews disponíveis"),
                (_, 1) => "1 jogo disponível".to_string(),
                _ => format!("{count} jogos disponíveis"),
            };
            responsive_text(
                frame,
                width,
                height,
                22.0,
                212.0,
                small_scale,
                &count_text,
                0xBBAFC7,
            );
            if matches!(
                self.page,
                LibraryPage::Games
                    | LibraryPage::Favorites
                    | LibraryPage::Recent
                    | LibraryPage::Homebrew
            ) {
                responsive_panel(
                    frame, width, height, 20.0, 222.0, 360.0, 26.0, scale, 0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    230.0,
                    small_scale,
                    &format!("FORMATO: {}", self.format_filter.label()),
                    brand::ACCENT_GREEN_LIGHT,
                );
            }
            responsive_panel(
                frame, width, height, 306.0, 170.0, 78.0, 44.0, scale, 0x2D1A3B,
            );
            responsive_text(
                frame,
                width,
                height,
                311.0,
                188.0,
                small_scale,
                "Atualizar",
                brand::ACCENT_GREEN_LIGHT,
            );

            if count == 0 {
                let compact = logical_height < 720.0;
                let card_top = 250.0;
                let card_height = (logical_height - 298.0).clamp(290.0, 430.0);
                responsive_panel(
                    frame,
                    width,
                    height,
                    20.0,
                    card_top,
                    360.0,
                    card_height,
                    scale,
                    0x21152F,
                );
                // Rows keep at least one text line of clearance: the "Pasta de
                // jogos" label sits 22 units above `path_y`, and it used to
                // overlap the second description line. Compact cards must
                // also end inside their minimum 290-unit height.
                let (icon_y, title_y, line_one_y, line_two_y, path_y, button_y, formats_y) =
                    if compact {
                        (
                            card_top + 38.0,
                            card_top + 90.0,
                            card_top + 114.0,
                            card_top + 132.0,
                            card_top + 170.0,
                            card_top + 226.0,
                            card_top + 278.0,
                        )
                    } else {
                        (
                            card_top + 58.0,
                            card_top + 130.0,
                            card_top + 166.0,
                            card_top + 190.0,
                            card_top + 246.0,
                            card_top + 308.0,
                            card_top + 374.0,
                        )
                    };
                let button_height = if compact { 44.0 } else { 52.0 };
                debug_assert_eq!(
                    empty_library_refresh_band(logical_height),
                    (button_y, button_y + button_height)
                );
                responsive_folder_icon(frame, width, height, 200.0, icon_y, scale);
                let (empty_title, first_line, second_line, formats) =
                    if !self.search_query.is_empty() {
                        (
                            "Nenhum resultado encontrado",
                            "Tente outro termo ou limpe a busca.",
                            "",
                            "Pesquisa sem resultados",
                        )
                    } else if self.format_filter != LibraryFormatFilter::All {
                        (
                            "Nenhum arquivo neste formato",
                            "Não há arquivos com a extensão escolhida.",
                            "Troque o filtro para ver outros formatos.",
                            "Filtro de formato ativo",
                        )
                    } else if self.page == LibraryPage::Homebrew {
                        (
                            "Nenhum homebrew encontrado",
                            "Adicione arquivos .3dsx à pasta",
                            "de jogos configurada no Aceris.",
                            "Formato: .3dsx",
                        )
                    } else {
                        (
                            "Sua biblioteca está vazia",
                            "Adicione seus arquivos de jogos à pasta",
                            "do Aceris para começar a jogar.",
                            "Formatos: .3ds / .cci / .cia / .3dsx",
                        )
                    };
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    title_y,
                    small_scale,
                    empty_title,
                    brand::TEXT,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    line_one_y,
                    small_scale,
                    first_line,
                    0xBBAFC7,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    line_two_y,
                    small_scale,
                    second_line,
                    0xBBAFC7,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    38.0,
                    path_y - 22.0,
                    small_scale,
                    "Pasta de jogos",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_panel(
                    frame, width, height, 38.0, path_y, 324.0, 48.0, scale, 0x2D1A3B,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    48.0,
                    path_y + 16.0,
                    small_scale,
                    &tail(&self.directory_label, 42),
                    brand::TEXT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    38.0,
                    button_y,
                    324.0,
                    button_height,
                    scale,
                    brand::ACCENT_RED,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    button_y + (button_height - 16.0) / 2.0,
                    small_scale,
                    "Atualizar biblioteca",
                    brand::TEXT,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    formats_y,
                    small_scale,
                    formats,
                    0xBBAFC7,
                );
            } else {
                let row_height = 58.0;
                let visible = ((logical_height - 350.0).max(row_height) / row_height)
                    .floor()
                    .max(1.0) as usize;
                for (row, entry) in self
                    .filtered_entries()
                    .into_iter()
                    .enumerate()
                    .skip(self.first_visible)
                    .take(visible)
                {
                    let y = 250.0 + (row - self.first_visible) as f32 * row_height;
                    responsive_panel(
                        frame,
                        width,
                        height,
                        20.0,
                        y,
                        360.0,
                        50.0,
                        scale,
                        if row == self.selected {
                            0x422038
                        } else {
                            0x21152F
                        },
                    );
                    responsive_panel(
                        frame,
                        width,
                        height,
                        30.0,
                        y + 7.0,
                        46.0,
                        36.0,
                        scale,
                        0x2D1A3B,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        36.0,
                        y + 19.0,
                        small_scale,
                        "3DS",
                        brand::ACCENT_GREEN_LIGHT,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        88.0,
                        y + 17.0,
                        small_scale,
                        &truncate(&entry.title, 30),
                        brand::TEXT,
                    );
                    let extension = entry
                        .path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("3ds");
                    responsive_text(
                        frame,
                        width,
                        height,
                        330.0,
                        y + 18.0,
                        small_scale,
                        &extension.to_ascii_lowercase(),
                        brand::ACCENT_GREEN_LIGHT,
                    );
                }
            }
        }

        if let Some(status) = &self.status {
            responsive_text(
                frame,
                width,
                height,
                22.0,
                (logical_height - 62.0).max(0.0),
                small_scale,
                &truncate(status, 52),
                brand::ACCENT_RED_LIGHT,
            );
        }
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            (logical_height - 36.0).max(0.0),
            400.0,
            36.0,
            scale,
            0x160D22,
        );
        responsive_text(
            frame,
            width,
            height,
            22.0,
            (logical_height - 22.0).max(0.0),
            small_scale,
            "Aceris  |  Rust  |  Seus arquivos, suas regras",
            0xBBAFC7,
        );
    }

    fn handle_touch_keyboard_tap(&mut self, x: f32, y: f32, logical_height: f32) -> bool {
        let top = search_keyboard_top(logical_height);
        if !(128.0..970.0).contains(&x) || y < top || y >= top + 174.0 {
            return false;
        }

        let rows = [
            ("qwertyuiop", 222.0),
            ("asdfghjkl", 255.0),
            (
                if self.scan_path_input_focused {
                    "zxcvbnm/."
                } else {
                    "zxcvbnm"
                },
                324.0,
            ),
        ];
        for (row, (keys, start_x)) in rows.iter().enumerate() {
            let row_top = top + 8.0 + row as f32 * 40.0;
            if (row_top..row_top + 34.0).contains(&y) && x >= *start_x {
                let offset = x - *start_x;
                let column = (offset / 66.0).floor() as usize;
                if offset - column as f32 * 66.0 < 58.0 {
                    if let Some(character) = keys.chars().nth(column) {
                        self.input_search_char(character);
                    }
                    return true;
                }
            }
        }

        let action_top = top + 128.0;
        if (action_top..action_top + 34.0).contains(&y) {
            if (152.0..332.0).contains(&x) {
                self.input_search_char(' ');
            } else if (340.0..480.0).contains(&x) {
                self.backspace_search();
            } else if (488.0..628.0).contains(&x) {
                self.clear_search();
            } else if (636.0..776.0).contains(&x) {
                self.finish_search();
            }
        }
        true
    }

    fn render_touch_keyboard(
        &self,
        frame: &mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        logical_height: f32,
        text_scale: usize,
    ) {
        let top = search_keyboard_top(logical_height);
        responsive_panel(
            frame, width, height, 128.0, top, 842.0, 174.0, scale, 0x21152F,
        );
        if self.scan_path_input_focused {
            responsive_text(
                frame,
                width,
                height,
                145.0,
                top - 22.0,
                text_scale,
                &truncate(&self.search_query, 90),
                brand::ACCENT_GREEN_LIGHT,
            );
        }
        let rows = [
            ("qwertyuiop", 222.0),
            ("asdfghjkl", 255.0),
            (
                if self.scan_path_input_focused {
                    "zxcvbnm/."
                } else {
                    "zxcvbnm"
                },
                324.0,
            ),
        ];
        for (row, (keys, start_x)) in rows.iter().enumerate() {
            let y = top + 8.0 + row as f32 * 40.0;
            for (column, character) in keys.chars().enumerate() {
                let x = *start_x + column as f32 * 66.0;
                responsive_panel(frame, width, height, x, y, 58.0, 34.0, scale, 0x33223F);
                let label = character.to_ascii_uppercase().to_string();
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    x + 29.0,
                    y + 9.0,
                    text_scale,
                    &label,
                    brand::TEXT,
                );
            }
        }
        for (x, key_width, label) in [
            (152.0, 180.0, "ESPAÇO"),
            (340.0, 140.0, "APAGAR"),
            (488.0, 140.0, "LIMPAR"),
            (636.0, 140.0, "OK"),
        ] {
            responsive_panel(
                frame,
                width,
                height,
                x,
                top + 128.0,
                key_width,
                34.0,
                scale,
                0x422038,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                x + key_width * 0.5,
                top + 137.0,
                text_scale,
                label,
                brand::TEXT,
            );
        }
    }

    fn render_landscape(
        &self,
        frame: &mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        logical_height: f32,
    ) {
        let title_scale = (scale * 1.8).round().max(2.0) as usize;
        let text_scale = (scale * 1.12).round().max(1.0) as usize;
        let small_scale = (scale * 0.92).round().max(1.0) as usize;
        let bottom = (logical_height - 18.0).max(400.0);
        let homebrew_count = self
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("3dsx"))
            })
            .count();
        let game_count = self.entries.len().saturating_sub(homebrew_count);
        let selected_entry = self.selected_entry();

        // Full-width top navigation inspired by modern console dashboards.
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            0.0,
            1000.0,
            70.0,
            scale,
            0x160D22,
        );
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            68.0,
            1000.0,
            2.0,
            scale,
            0x563B67,
        );
        responsive_panel(
            frame,
            width,
            height,
            18.0,
            10.0,
            94.0,
            46.0,
            scale,
            brand::ACCENT_RED,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            65.0,
            25.0,
            title_scale,
            "ACERIS",
            brand::TEXT,
        );

        let tabs = [
            (120.0, 84.0, LibraryPage::Home, "INÍCIO"),
            (210.0, 100.0, LibraryPage::Recent, "RECENTES"),
            (316.0, 94.0, LibraryPage::Games, "JOGOS"),
            (416.0, 150.0, LibraryPage::Homebrew, "HOMEBREW"),
            (572.0, 120.0, LibraryPage::Favorites, "FAVORITOS"),
        ];
        for (x, tab_width, page, label) in tabs {
            let selected = self.page == page;
            responsive_panel(
                frame,
                width,
                height,
                x,
                12.0,
                tab_width,
                42.0,
                scale,
                if selected { 0x422038 } else { 0x21152F },
            );
            if selected {
                responsive_fill(
                    frame,
                    width as usize,
                    height as usize,
                    x + 4.0,
                    52.0,
                    tab_width - 8.0,
                    3.0,
                    scale,
                    brand::ACCENT_GREEN_LIGHT,
                );
            }
            responsive_text_centered(
                frame,
                width,
                height,
                x + tab_width * 0.5,
                27.0,
                text_scale,
                label,
                if selected { brand::TEXT } else { 0xBBAFC7 },
            );
        }
        responsive_panel(
            frame,
            width,
            height,
            875.0,
            12.0,
            107.0,
            42.0,
            scale,
            if self.page == LibraryPage::Settings {
                brand::ACCENT_RED
            } else {
                0x2D1A3B
            },
        );
        responsive_text_centered(
            frame,
            width,
            height,
            928.0,
            27.0,
            text_scale,
            "AJUSTES",
            brand::TEXT,
        );

        // A distinct right-side action panel leaves the main area for the collection.
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            70.0,
            1000.0,
            logical_height - 70.0,
            scale,
            0x1A1026,
        );
        responsive_panel(
            frame,
            width,
            height,
            758.0,
            82.0,
            224.0,
            (bottom - 82.0).max(280.0),
            scale,
            0x21152F,
        );
        responsive_text(
            frame,
            width,
            height,
            780.0,
            103.0,
            title_scale,
            "ACERIS",
            brand::TEXT,
        );
        responsive_text(
            frame,
            width,
            height,
            782.0,
            132.0,
            small_scale,
            "BIBLIOTECA DE JOGOS",
            brand::ACCENT_GREEN_LIGHT,
        );
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            780.0,
            153.0,
            180.0,
            1.0,
            scale,
            0x563B67,
        );

        if self.page == LibraryPage::Home {
            responsive_text(
                frame,
                width,
                height,
                28.0,
                84.0,
                title_scale,
                "SEU ESPAÇO DE JOGOS",
                brand::TEXT,
            );
            responsive_text(
                frame,
                width,
                height,
                30.0,
                109.0,
                text_scale,
                "Continue de onde parou ou explore sua coleção",
                0xBBAFC7,
            );
            responsive_panel(
                frame, width, height, 28.0, 132.0, 702.0, 166.0, scale, 0x251731,
            );
            responsive_text(
                frame,
                width,
                height,
                52.0,
                151.0,
                text_scale,
                "CONTINUAR JOGANDO",
                brand::ACCENT_GREEN_LIGHT,
            );
            if let Some(entry) = selected_entry {
                render_cover_art(
                    frame,
                    width,
                    height,
                    scale,
                    58.0,
                    178.0,
                    112.0,
                    98.0,
                    entry,
                    title_scale,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    194.0,
                    187.0,
                    title_scale,
                    &truncate(&entry.title, 42),
                    brand::TEXT,
                );
                let extension = entry
                    .path
                    .extension()
                    .and_then(|value| value.to_str())
                    .unwrap_or("3ds");
                responsive_text(
                    frame,
                    width,
                    height,
                    196.0,
                    219.0,
                    text_scale,
                    &format!("FORMATO .{}", extension.to_ascii_uppercase()),
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    196.0,
                    245.0,
                    small_scale,
                    "Seu último jogo aberto",
                    0xBBAFC7,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    520.0,
                    221.0,
                    180.0,
                    48.0,
                    scale,
                    brand::ACCENT_RED,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    610.0,
                    238.0,
                    text_scale,
                    "CONTINUAR",
                    brand::TEXT,
                );
            } else {
                responsive_folder_icon(frame, width, height, 116.0, 225.0, scale);
                responsive_text(
                    frame,
                    width,
                    height,
                    202.0,
                    190.0,
                    title_scale,
                    if self.entries.is_empty() {
                        "Sua coleção começa aqui"
                    } else {
                        "Sua biblioteca está pronta"
                    },
                    brand::TEXT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    204.0,
                    223.0,
                    text_scale,
                    if self.entries.is_empty() {
                        "Adicione uma pasta para encontrar seus jogos."
                    } else {
                        "Explore os jogos disponíveis na sua biblioteca."
                    },
                    0xBBAFC7,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    520.0,
                    221.0,
                    180.0,
                    48.0,
                    scale,
                    brand::ACCENT_RED,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    610.0,
                    238.0,
                    text_scale,
                    if self.entries.is_empty() {
                        "ADICIONAR PASTA"
                    } else {
                        "VER JOGOS"
                    },
                    brand::TEXT,
                );
            }

            responsive_text(
                frame,
                width,
                height,
                28.0,
                318.0,
                text_scale,
                "EXPLORE SUA BIBLIOTECA",
                brand::ACCENT_GREEN_LIGHT,
            );
            let cards = [
                (28.0, 340.0, 220.0, "JOGOS", game_count),
                (258.0, 340.0, 220.0, "FAVORITOS", self.favorite_count()),
                (488.0, 340.0, 242.0, "HOMEBREW & DEMOS", homebrew_count),
            ];
            for (x, y, card_width, label, count) in cards {
                responsive_panel(
                    frame, width, height, x, y, card_width, 82.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    x + 16.0,
                    y + 17.0,
                    small_scale,
                    label,
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    x + 16.0,
                    y + 43.0,
                    title_scale,
                    &count.to_string(),
                    brand::TEXT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    x + card_width - 74.0,
                    y + 51.0,
                    small_scale,
                    "EXPLORAR",
                    0xBBAFC7,
                );
            }
        } else if self.page == LibraryPage::Settings {
            responsive_text(
                frame,
                width,
                height,
                28.0,
                84.0,
                title_scale,
                "CONFIGURAÇÕES",
                brand::TEXT,
            );
            responsive_text(
                frame,
                width,
                height,
                30.0,
                109.0,
                text_scale,
                "Pastas, desempenho e preferências da biblioteca",
                0xBBAFC7,
            );
            responsive_panel(
                frame,
                width,
                height,
                28.0,
                132.0,
                702.0,
                (bottom - 132.0).max(270.0),
                scale,
                0x21152F,
            );
            responsive_text(
                frame,
                width,
                height,
                48.0,
                151.0,
                text_scale,
                &format!(
                    "PASTAS MONITORADAS ({})",
                    self.scan_config.directories.len()
                ),
                brand::ACCENT_GREEN_LIGHT,
            );
            for (index, directory) in self.scan_config.directories.iter().take(3).enumerate() {
                let row_y = 178.0 + index as f32 * 28.0;
                responsive_panel(
                    frame, width, height, 44.0, row_y, 620.0, 24.0, scale, 0x33223F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    54.0,
                    row_y + 7.0,
                    small_scale,
                    &tail(&directory.to_string_lossy(), 76),
                    brand::TEXT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    670.0,
                    row_y + 1.0,
                    48.0,
                    22.0,
                    scale,
                    0x422038,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    694.0,
                    row_y + 6.0,
                    small_scale,
                    "REM",
                    brand::TEXT,
                );
            }
            if self.scan_config.directories.len() > 3 {
                responsive_text(
                    frame,
                    width,
                    height,
                    54.0,
                    266.0,
                    small_scale,
                    &format!(
                        "+ {} outra(s) pasta(s)",
                        self.scan_config.directories.len() - 3
                    ),
                    0xBBAFC7,
                );
            }
            let settings_actions = [
                (
                    28.0,
                    280.0,
                    217.0,
                    "ADICIONAR PASTA",
                    if self.scan_path_input_focused {
                        brand::ACCENT_RED
                    } else {
                        0x2D1A3B
                    },
                ),
                (
                    255.0,
                    280.0,
                    220.0,
                    if self.scan_config.options.recursive {
                        "SUBPASTAS: SIM"
                    } else {
                        "SUBPASTAS: NÃO"
                    },
                    if self.scan_config.options.recursive {
                        0x28543F
                    } else {
                        0x2D1A3B
                    },
                ),
                (
                    485.0,
                    280.0,
                    110.0,
                    if self.performance_mode {
                        "RÁPIDO: SIM"
                    } else {
                        "RÁPIDO: NÃO"
                    },
                    if self.performance_mode {
                        0x28543F
                    } else {
                        0x2D1A3B
                    },
                ),
            ];
            for (x, y, card_width, label, color) in settings_actions {
                responsive_panel(frame, width, height, x, y, card_width, 42.0, scale, color);
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    x + card_width * 0.5,
                    y + 14.0,
                    small_scale,
                    label,
                    brand::TEXT,
                );
            }
            responsive_panel(
                frame, width, height, 605.0, 280.0, 125.0, 42.0, scale, 0x2D1A3B,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                667.0,
                289.0,
                small_scale,
                "ZONA MORTA",
                brand::TEXT,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                667.0,
                306.0,
                small_scale,
                &format!("{}%", self.gamepad_deadzone_percent),
                brand::ACCENT_GREEN_LIGHT,
            );
            if self.scan_path_input_focused {
                responsive_text(
                    frame,
                    width,
                    height,
                    48.0,
                    340.0,
                    small_scale,
                    &truncate(&self.search_query, 100),
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    48.0,
                    360.0,
                    small_scale,
                    "Digite o caminho completo da pasta e pressione Enter/OK.",
                    0xBBAFC7,
                );
            }
            responsive_text(
                frame,
                width,
                height,
                48.0,
                382.0,
                small_scale,
                "PREFERÊNCIAS DA BIBLIOTECA",
                brand::ACCENT_GREEN_LIGHT,
            );
            let view_label = if self.view_mode == LibraryViewMode::Grid {
                "VISTA: GRADE"
            } else {
                "VISTA: LISTA"
            };
            let sort_label = if self.sort_descending {
                "ORDEM: Z–A"
            } else {
                "ORDEM: A–Z"
            };
            let settings_footer = [
                (28.0, 396.0, 217.0, view_label),
                (255.0, 396.0, 220.0, sort_label),
            ];
            for (x, y, card_width, label) in settings_footer {
                responsive_panel(
                    frame, width, height, x, y, card_width, 32.0, scale, 0x2D1A3B,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    x + card_width * 0.5,
                    y + 10.0,
                    small_scale,
                    label,
                    brand::TEXT,
                );
            }
            if self.touch_keyboard_enabled {
                responsive_panel(
                    frame,
                    width,
                    height,
                    485.0,
                    396.0,
                    245.0,
                    32.0,
                    scale,
                    if self.virtual_controls_visible {
                        0x28543F
                    } else {
                        0x2D1A3B
                    },
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    607.0,
                    406.0,
                    small_scale,
                    if self.virtual_controls_visible {
                        "CONTROLES: ATIVOS"
                    } else {
                        "CONTROLES: OCULTOS"
                    },
                    brand::TEXT,
                );
                if logical_height >= 540.0 {
                    let control_options =
                        [(485.0, "OPACIDADE"), (568.0, "TAMANHO"), (651.0, "LAYOUT")];
                    for (x, label) in control_options {
                        responsive_panel(
                            frame, width, height, x, 438.0, 79.0, 38.0, scale, 0x2D1A3B,
                        );
                        responsive_text_centered(
                            frame,
                            width,
                            height,
                            x + 39.5,
                            451.0,
                            small_scale,
                            label,
                            brand::TEXT,
                        );
                    }
                }
            }
        } else {
            let page_title = match self.page {
                LibraryPage::Games => "JOGOS",
                LibraryPage::Favorites => "FAVORITOS",
                LibraryPage::Recent => "RECENTES",
                LibraryPage::Homebrew => "HOMEBREW & DEMOS",
                LibraryPage::Home | LibraryPage::Settings => "BIBLIOTECA",
            };
            responsive_text(
                frame,
                width,
                height,
                28.0,
                84.0,
                title_scale,
                page_title,
                brand::TEXT,
            );
            responsive_text(
                frame,
                width,
                height,
                30.0,
                109.0,
                text_scale,
                &format!("{} item(ns) disponíveis", self.visible_entry_count()),
                0xBBAFC7,
            );
            responsive_panel(
                frame, width, height, 28.0, 126.0, 702.0, 32.0, scale, 0x21152F,
            );
            responsive_panel(
                frame,
                width,
                height,
                36.0,
                130.0,
                329.0,
                24.0,
                scale,
                if self.search_focused {
                    0x422038
                } else {
                    0x33223F
                },
            );
            let search_label = if self.search_query.is_empty() {
                "BUSCAR POR NOME OU CAMINHO".to_string()
            } else {
                format!("BUSCAR: {}", truncate(&self.search_query, 42))
            };
            responsive_text(
                frame,
                width,
                height,
                46.0,
                138.0,
                small_scale,
                &search_label,
                if self.search_focused {
                    brand::ACCENT_GREEN_LIGHT
                } else {
                    0xBBAFC7
                },
            );
            responsive_panel(
                frame, width, height, 368.0, 130.0, 34.0, 24.0, scale, 0x422038,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                385.0,
                137.0,
                small_scale,
                "X",
                brand::TEXT,
            );
            responsive_panel(
                frame, width, height, 410.0, 130.0, 108.0, 24.0, scale, 0x2D1A3B,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                464.0,
                137.0,
                small_scale,
                self.format_filter.label(),
                brand::ACCENT_GREEN_LIGHT,
            );
            responsive_panel(
                frame,
                width,
                height,
                526.0,
                130.0,
                96.0,
                24.0,
                scale,
                if self.view_mode == LibraryViewMode::List {
                    brand::ACCENT_RED
                } else {
                    0x2D1A3B
                },
            );
            responsive_text_centered(
                frame,
                width,
                height,
                574.0,
                137.0,
                small_scale,
                if self.view_mode == LibraryViewMode::List {
                    "LISTA"
                } else {
                    "GRADE"
                },
                brand::TEXT,
            );
            responsive_panel(
                frame, width, height, 630.0, 130.0, 92.0, 24.0, scale, 0x2D1A3B,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                676.0,
                137.0,
                small_scale,
                if self.sort_descending {
                    "Z–A"
                } else {
                    "A–Z"
                },
                brand::TEXT,
            );

            let entries = self.filtered_entries();
            if entries.is_empty() {
                responsive_panel(
                    frame,
                    width,
                    height,
                    28.0,
                    170.0,
                    702.0,
                    (bottom - 190.0).max(190.0),
                    scale,
                    0x21152F,
                );
                responsive_folder_icon(frame, width, height, 140.0, 270.0, scale);
                let (empty_title, first_line) = if self.page == LibraryPage::Homebrew {
                    (
                        "Nenhum homebrew encontrado",
                        "Adicione arquivos .3dsx à biblioteca.",
                    )
                } else if !self.search_query.is_empty() {
                    (
                        "Nenhum resultado encontrado",
                        "Tente outro termo ou limpe a busca.",
                    )
                } else {
                    (
                        "Sua biblioteca está vazia",
                        "Adicione uma pasta com seus jogos para começar.",
                    )
                };
                responsive_text(
                    frame,
                    width,
                    height,
                    230.0,
                    235.0,
                    title_scale,
                    empty_title,
                    brand::TEXT,
                );
                responsive_text(
                    frame, width, height, 232.0, 270.0, text_scale, first_line, 0xBBAFC7,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    500.0,
                    315.0,
                    192.0,
                    42.0,
                    scale,
                    brand::ACCENT_RED,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    596.0,
                    329.0,
                    small_scale,
                    "ADICIONAR PASTA",
                    brand::TEXT,
                );
            } else if self.view_mode == LibraryViewMode::List {
                let list_top = 170.0;
                let row_step = 54.0;
                let row_height = 48.0;
                let visible_rows = ((bottom - list_top - row_height) / row_step)
                    .floor()
                    .max(0.0) as usize
                    + 1;
                for (row, entry) in entries
                    .iter()
                    .enumerate()
                    .skip(self.first_visible)
                    .take(visible_rows)
                {
                    let y = list_top + (row - self.first_visible) as f32 * row_step;
                    responsive_panel(
                        frame,
                        width,
                        height,
                        28.0,
                        y,
                        702.0,
                        row_height,
                        scale,
                        if row == self.selected {
                            0x422038
                        } else {
                            0x21152F
                        },
                    );
                    render_cover_art(
                        frame,
                        width,
                        height,
                        scale,
                        38.0,
                        y + 5.0,
                        54.0,
                        38.0,
                        entry,
                        small_scale,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        108.0,
                        y + 17.0,
                        text_scale,
                        &truncate(&entry.title, 64),
                        brand::TEXT,
                    );
                    let extension = entry
                        .path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("3ds");
                    responsive_text(
                        frame,
                        width,
                        height,
                        650.0,
                        y + 17.0,
                        small_scale,
                        &extension.to_ascii_uppercase(),
                        brand::ACCENT_GREEN_LIGHT,
                    );
                    if self.is_favorite(entry) {
                        responsive_text(
                            frame,
                            width,
                            height,
                            694.0,
                            y + 17.0,
                            small_scale,
                            "F",
                            brand::ACCENT_GREEN_LIGHT,
                        );
                    }
                }
            } else {
                const COLUMNS: usize = 4;
                let card_top = 170.0;
                let card_width = 168.0;
                let card_height = 122.0;
                let card_gap = 10.0;
                let row_step = 132.0;
                let visible_rows = landscape_grid_visible_rows(logical_height);
                for (index, entry) in entries
                    .iter()
                    .enumerate()
                    .skip(self.first_visible)
                    .take(visible_rows * COLUMNS)
                {
                    let local = index - self.first_visible;
                    let column = local % COLUMNS;
                    let row = local / COLUMNS;
                    let x = 28.0 + column as f32 * (card_width + card_gap);
                    let y = card_top + row as f32 * row_step;
                    render_library_tile(
                        frame,
                        width,
                        height,
                        scale,
                        x,
                        y,
                        card_width,
                        card_height,
                        entry,
                        index == self.selected,
                        self.is_favorite(entry),
                        text_scale,
                        small_scale,
                    );
                }
            }
        }

        // Persistent right-hand actions mirror a console's selected-game panel.
        responsive_text(
            frame,
            width,
            height,
            780.0,
            174.0,
            small_scale,
            if self.page == LibraryPage::Home {
                "ULTIMO JOGO"
            } else if self.page == LibraryPage::Settings {
                "BIBLIOTECA"
            } else {
                "SELECIONADO"
            },
            brand::ACCENT_GREEN_LIGHT,
        );
        if let Some(entry) = selected_entry {
            responsive_text(
                frame,
                width,
                height,
                780.0,
                197.0,
                text_scale,
                &truncate(&entry.title, 26),
                brand::TEXT,
            );
            let extension = entry
                .path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("3ds");
            responsive_text(
                frame,
                width,
                height,
                782.0,
                220.0,
                small_scale,
                &format!(".{}", extension.to_ascii_uppercase()),
                0xBBAFC7,
            );
        } else {
            responsive_text(
                frame,
                width,
                height,
                780.0,
                199.0,
                small_scale,
                &format!("{} jogo(s)", game_count),
                brand::TEXT,
            );
            responsive_text(
                frame,
                width,
                height,
                782.0,
                220.0,
                small_scale,
                &format!("{} homebrew(s)", homebrew_count),
                0xBBAFC7,
            );
        }
        responsive_panel(
            frame,
            width,
            height,
            776.0,
            238.0,
            188.0,
            42.0,
            scale,
            if selected_entry.is_some() {
                brand::ACCENT_RED
            } else {
                0x2D1A3B
            },
        );
        responsive_text_centered(
            frame,
            width,
            height,
            870.0,
            252.0,
            small_scale,
            "ABRIR JOGO",
            brand::TEXT,
        );
        responsive_panel(
            frame, width, height, 776.0, 286.0, 188.0, 42.0, scale, 0x2D1A3B,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            870.0,
            300.0,
            small_scale,
            if selected_entry.is_some_and(|entry| self.is_favorite(entry)) {
                "REMOVER FAVORITO"
            } else {
                "ADICIONAR FAVORITO"
            },
            brand::TEXT,
        );
        responsive_panel(
            frame,
            width,
            height,
            776.0,
            336.0,
            188.0,
            42.0,
            scale,
            brand::ACCENT_RED,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            870.0,
            350.0,
            small_scale,
            "ADICIONAR PASTA",
            brand::TEXT,
        );
        responsive_panel(
            frame, width, height, 776.0, 384.0, 188.0, 42.0, scale, 0x2D1A3B,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            870.0,
            398.0,
            small_scale,
            "ATUALIZAR",
            brand::TEXT,
        );

        responsive_text(
            frame,
            width,
            height,
            780.0,
            (bottom - 34.0).max(0.0),
            small_scale,
            &format!(
                "{} PASTA(S) MONITORADA(S)",
                self.scan_config.directories.len()
            ),
            0xBBAFC7,
        );
        if let Some(status) = &self.status {
            responsive_text(
                frame,
                width,
                height,
                28.0,
                (bottom - 32.0).max(0.0),
                small_scale,
                &truncate(status, 86),
                brand::ACCENT_RED_LIGHT,
            );
        }
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            (logical_height - 18.0).max(0.0),
            1000.0,
            18.0,
            scale,
            0x160D22,
        );
        responsive_text(
            frame,
            width,
            height,
            22.0,
            (logical_height - 13.0).max(0.0),
            small_scale,
            "ACERIS  |  RUST  |  SEUS ARQUIVOS, SUAS REGRAS",
            0xBBAFC7,
        );
        if self.touch_keyboard_open {
            self.render_touch_keyboard(frame, width, height, scale, logical_height, text_scale);
        }
    }

    /// Draws the library as a 400x480 RGBA frame.
    pub fn render(&self) -> Vec<u8> {
        self.render_animated(0, None)
    }

    pub fn render_animated(&self, frame_index: u64, background_frame: Option<&[u8]>) -> Vec<u8> {
        let (w, h) = (WIDTH, HEIGHT);
        let mut out = vec![255u8; w * h * 4];
        let top = rgb(brand::PURPLE_LIGHT);
        let bottom = rgb(brand::PURPLE_DARK);
        for y in 0..h {
            let t = y as f32 / h as f32;
            let color = [0, 1, 2].map(|i| (top[i] as f32 * (1.0 - t) + bottom[i] as f32 * t) as u8);
            for x in 0..w {
                let at = (y * w + x) * 4;
                out[at..at + 3].copy_from_slice(&color);
            }
        }
        if let Some(background) = background_frame.filter(|frame| frame.len() == w * h * 4) {
            for (destination, source) in out.chunks_exact_mut(4).zip(background.chunks_exact(4)) {
                let alpha = (source[3] as f32 / 255.0 * 0.38).clamp(0.0, 0.38);
                for channel in 0..3 {
                    destination[channel] = (source[channel] as f32 * alpha
                        + destination[channel] as f32 * (1.0 - alpha))
                        as u8;
                }
            }
        }

        // Slow, low-contrast glints give the home screen movement without
        // changing the Aceris palette or competing with selected game rows.
        let animation = (frame_index % 1_000_000) as usize;
        let sweep = animation as i32 % 520 - 80;
        for y in (HEADER_HEIGHT + 4)..FOOTER_TOP {
            let x = (sweep + y / 5).rem_euclid(w as i32);
            blend_pixel(&mut out, x, y, rgb(brand::ACCENT_RED_LIGHT), 0.055);
        }
        let ambient_height = (FOOTER_TOP - HEADER_HEIGHT - 16) as usize;
        for particle in 0..7usize {
            let x = ((animation * (particle + 1) * 3 + particle * 47) % w) as i32;
            let y = HEADER_HEIGHT
                + 8
                + ((particle * 53 + animation * (particle + 1) / 9) % ambient_height) as i32;
            blend_pixel(&mut out, x, y, rgb(brand::ACCENT_GREEN_LIGHT), 0.11);
            blend_pixel(&mut out, x + 1, y, rgb(brand::ACCENT_RED_LIGHT), 0.07);
        }

        let text = rgb(brand::TEXT);
        let red = rgb(brand::ACCENT_RED);

        fill(
            &mut out,
            0,
            0,
            w as i32,
            HEADER_HEIGHT,
            rgb(brand::PURPLE_DARK),
        );
        fill(&mut out, 0, HEADER_HEIGHT - 3, w as i32, 3, red);
        draw_text(&mut out, w, h, MARGIN, 14, 4, "ACERIS", text);
        let count = format!("{} JOGOS", self.entries.len());
        let count_x = w as i32 - MARGIN - font::text_width(count.chars().count(), 1) as i32;
        draw_text(
            &mut out,
            w,
            h,
            count_x,
            22,
            1,
            &count,
            rgb(brand::ACCENT_RED_LIGHT),
        );

        if self.entries.is_empty() {
            self.draw_empty(&mut out);
        }
        let max_chars = (w as i32 - 2 * MARGIN - 16) as usize / (font::ADVANCE * TEXT_SCALE);
        for (row, entry) in self
            .entries
            .iter()
            .enumerate()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
        {
            let y = LIST_TOP + (row - self.first_visible) as i32 * ROW_HEIGHT;
            let selected = row == self.selected;
            if selected {
                fill(
                    &mut out,
                    MARGIN - 8,
                    y,
                    w as i32 - 2 * (MARGIN - 8),
                    ROW_HEIGHT - 4,
                    red,
                );
            }
            let title = truncate(&entry.title, max_chars);
            let color = if selected { [255, 255, 255] } else { text };
            draw_text(&mut out, w, h, MARGIN, y + 5, TEXT_SCALE, &title, color);
        }
        if self.entries.len() > VISIBLE_ROWS {
            let track = (FOOTER_TOP - 22 - LIST_TOP) as f32;
            let thumb = (track * VISIBLE_ROWS as f32 / self.entries.len() as f32).max(12.0);
            let offset = (track - thumb) * self.first_visible as f32
                / (self.entries.len() - VISIBLE_ROWS) as f32;
            fill(
                &mut out,
                w as i32 - 6,
                LIST_TOP + offset as i32,
                3,
                thumb as i32,
                rgb(brand::ACCENT_GREEN),
            );
        }

        if let Some(status) = &self.status {
            let message = truncate(status, (w - 2 * MARGIN as usize) / font::ADVANCE);
            draw_text(
                &mut out,
                w,
                h,
                MARGIN,
                FOOTER_TOP - 16,
                1,
                &message,
                rgb(brand::ACCENT_RED_LIGHT),
            );
        }
        fill(
            &mut out,
            0,
            FOOTER_TOP,
            w as i32,
            h as i32 - FOOTER_TOP,
            rgb(brand::PURPLE_DARK),
        );
        draw_text(
            &mut out,
            w,
            h,
            MARGIN,
            FOOTER_TOP + 14,
            1,
            "SETAS: ESCOLHER   ENTER: JOGAR   ESC: SAIR",
            text,
        );
        out
    }

    fn draw_empty(&self, out: &mut [u8]) {
        let text = rgb(brand::TEXT);
        let lines = [
            "NENHUM JOGO ENCONTRADO".to_string(),
            "COLOQUE .3DS, .CCI OU .3DSX EM:".to_string(),
        ];
        for (i, line) in lines.iter().enumerate() {
            draw_text(
                out,
                WIDTH,
                HEIGHT,
                MARGIN,
                LIST_TOP + 20 + i as i32 * 24,
                1,
                line,
                text,
            );
        }
        let path = tail(
            &self.directory_label,
            (WIDTH - 2 * MARGIN as usize) / font::ADVANCE,
        );
        draw_text(
            out,
            WIDTH,
            HEIGHT,
            MARGIN,
            LIST_TOP + 68,
            1,
            &path,
            rgb(brand::ACCENT_GREEN_LIGHT),
        );
    }
}

fn landscape_grid_visible_rows(logical_height: f32) -> usize {
    const TOP: f32 = 170.0;
    const CARD_HEIGHT: f32 = 122.0;
    const ROW_STEP: f32 = 132.0;
    let last_content_y = (logical_height - 18.0).max(400.0);
    (((last_content_y - TOP - CARD_HEIGHT) / ROW_STEP).floor() as i32 + 1).max(1) as usize
}

fn cover_palette(title: &str) -> (u32, u32, u32) {
    const PALETTES: [(u32, u32, u32); 6] = [
        (0x51213A, 0xE5173F, 0xF4A6B7),
        (0x183A54, 0x3E9AB5, 0xA7DDE8),
        (0x31442C, 0x80D45A, 0xC8ECA9),
        (0x4B2A68, 0x9A65C7, 0xD9B7F0),
        (0x514020, 0xD39C39, 0xF0D59A),
        (0x26334E, 0x667DCC, 0xB8C5F0),
    ];
    let mut hash = 2_166_136_261u32;
    for byte in title.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(16_777_619);
    }
    PALETTES[(hash as usize) % PALETTES.len()]
}

#[allow(clippy::too_many_arguments)]
fn render_cover_art(
    frame: &mut [u8],
    width: u32,
    height: u32,
    scale: f32,
    x: f32,
    y: f32,
    cover_width: f32,
    cover_height: f32,
    entry: &Entry,
    text_scale: usize,
) {
    let (base, accent, highlight) = cover_palette(&entry.title);
    responsive_panel(
        frame,
        width,
        height,
        x,
        y,
        cover_width,
        cover_height,
        scale,
        base,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 4.0,
        y + 4.0,
        cover_width - 8.0,
        cover_height - 8.0,
        scale,
        base,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 10.0,
        y + cover_height * 0.24,
        cover_width - 20.0,
        5.0,
        scale,
        accent,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + cover_width * 0.18,
        y + cover_height * 0.40,
        cover_width * 0.64,
        4.0,
        scale,
        highlight,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + cover_width * 0.34,
        y + cover_height * 0.57,
        cover_width * 0.32,
        4.0,
        scale,
        accent,
    );
    responsive_text_centered(
        frame,
        width,
        height,
        x + cover_width * 0.5,
        y + cover_height * 0.68,
        text_scale,
        "3DS",
        brand::TEXT,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_library_tile(
    frame: &mut [u8],
    width: u32,
    height: u32,
    scale: f32,
    x: f32,
    y: f32,
    card_width: f32,
    card_height: f32,
    entry: &Entry,
    selected: bool,
    favorite: bool,
    text_scale: usize,
    small_scale: usize,
) {
    responsive_panel(
        frame,
        width,
        height,
        x,
        y,
        card_width,
        card_height,
        scale,
        if selected { 0x422038 } else { 0x21152F },
    );
    let (base, accent, highlight) = cover_palette(&entry.title);
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 6.0,
        y + 6.0,
        card_width - 12.0,
        70.0,
        scale,
        base,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 14.0,
        y + 20.0,
        card_width - 28.0,
        4.0,
        scale,
        accent,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 28.0,
        y + 37.0,
        card_width - 56.0,
        4.0,
        scale,
        highlight,
    );
    responsive_text_centered(
        frame,
        width,
        height,
        x + card_width * 0.5,
        y + 53.0,
        text_scale,
        "3DS",
        brand::TEXT,
    );
    if favorite {
        responsive_panel(
            frame,
            width,
            height,
            x + card_width - 28.0,
            y + 8.0,
            20.0,
            20.0,
            scale,
            0x422038,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            x + card_width - 18.0,
            y + 13.0,
            small_scale,
            "F",
            brand::ACCENT_GREEN_LIGHT,
        );
    }
    responsive_text(
        frame,
        width,
        height,
        x + 10.0,
        y + 87.0,
        small_scale,
        &truncate(&entry.title, 25),
        brand::TEXT,
    );
    let extension = entry
        .path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("3ds");
    responsive_text(
        frame,
        width,
        height,
        x + 10.0,
        y + 108.0,
        small_scale,
        &extension.to_ascii_uppercase(),
        brand::ACCENT_GREEN_LIGHT,
    );
}

fn search_keyboard_top(logical_height: f32) -> f32 {
    (logical_height - 210.0).max(150.0)
}

fn fold_search_text(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            other => other,
        })
        .collect()
}

fn responsive_metrics(width: u32, height: u32) -> Option<(bool, f32, f32, f32)> {
    if width < 160 || height < 160 || width > 4096 || height > 4096 {
        return None;
    }
    let portrait = height >= width;
    let logical_width = if portrait { 400.0 } else { 1000.0 };
    let scale = width as f32 / logical_width;
    let logical_height = height as f32 / scale;
    (scale.is_finite() && scale > 0.0 && logical_height.is_finite()).then_some((
        portrait,
        scale,
        logical_width,
        logical_height,
    ))
}

/// Cover-crops a user 400x480 RGBA background and blends it below launcher controls.
fn responsive_blend_background(frame: &mut [u8], width: u32, height: u32, background: &[u8]) {
    const SOURCE_WIDTH: u32 = WIDTH as u32;
    const SOURCE_HEIGHT: u32 = HEIGHT as u32;
    if width == 0
        || height == 0
        || background.len() != SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 4
        || frame.len() != width as usize * height as usize * 4
    {
        return;
    }

    let target_w = width as f64;
    let target_h = height as f64;
    let source_w = SOURCE_WIDTH as f64;
    let source_h = SOURCE_HEIGHT as f64;
    let scale = (target_w / source_w).max(target_h / source_h);
    let scaled_w = source_w * scale;
    let scaled_h = source_h * scale;
    let crop_x = (scaled_w - target_w) * 0.5;
    let crop_y = (scaled_h - target_h) * 0.5;

    for y in 0..height {
        let source_y =
            (((y as f64 + 0.5 + crop_y) / scale).floor() as u32).min(SOURCE_HEIGHT - 1) as usize;
        for x in 0..width {
            let source_x =
                (((x as f64 + 0.5 + crop_x) / scale).floor() as u32).min(SOURCE_WIDTH - 1) as usize;
            let source_at = (source_y * SOURCE_WIDTH as usize + source_x) * 4;
            let target_at = (y as usize * width as usize + x as usize) * 4;
            let alpha = u32::from(background[source_at + 3]) * 38 / 100;
            let inverse = 255 - alpha;
            for channel in 0..3 {
                frame[target_at + channel] = ((u32::from(background[source_at + channel]) * alpha
                    + u32::from(frame[target_at + channel]) * inverse)
                    / 255) as u8;
            }
        }
    }
}

// These low-level draw primitives intentionally accept a full rectangle and target buffer.
#[allow(clippy::too_many_arguments)]
fn responsive_fill(
    frame: &mut [u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    rect_width: f32,
    rect_height: f32,
    scale: f32,
    color: u32,
) {
    let left = (x * scale).floor().max(0.0) as usize;
    let top = (y * scale).floor().max(0.0) as usize;
    let right = ((x + rect_width) * scale).ceil().min(width as f32) as usize;
    let bottom = ((y + rect_height) * scale).ceil().min(height as f32) as usize;
    let pixel = [(color >> 16) as u8, (color >> 8) as u8, color as u8, 255];
    if left >= right
        || top >= bottom
        || frame.len() != width.saturating_mul(height).saturating_mul(4)
    {
        return;
    }
    for row in top..bottom {
        let start = (row * width + left) * 4;
        let end = (row * width + right) * 4;
        for rgba in frame[start..end].chunks_exact_mut(4) {
            rgba.copy_from_slice(&pixel);
        }
    }
}

// Panels share the target geometry directly to avoid transient layout allocations.
#[allow(clippy::too_many_arguments)]
fn responsive_panel(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    rect_width: f32,
    rect_height: f32,
    scale: f32,
    color: u32,
) {
    // Offset shadow separates cards from the shared dark canvas without
    // changing their hit targets or the logical layout coordinates.
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 3.0,
        y + 3.0,
        rect_width,
        rect_height,
        scale,
        0x0C0814,
    );
    // Violet outer stroke and the existing per-control fill.
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x,
        y,
        rect_width,
        rect_height,
        scale,
        0x563B67,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 1.0,
        y + 1.0,
        rect_width - 2.0,
        rect_height - 2.0,
        scale,
        color,
    );
    // A restrained top highlight and bottom edge add depth without gradients.
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 2.0,
        y + 2.0,
        rect_width - 4.0,
        1.0,
        scale,
        0x634B7E,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 2.0,
        y + rect_height - 2.0,
        rect_width - 4.0,
        1.0,
        scale,
        0x160F24,
    );
}

// Text coordinates and scale are explicit so all frontend layouts use the same renderer.
#[allow(clippy::too_many_arguments)]
fn responsive_text(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    text_scale: usize,
    text: &str,
    color: u32,
) {
    let Some((_, ui_scale, _, _)) = responsive_metrics(width, height) else {
        return;
    };
    draw_text(
        frame,
        width as usize,
        height as usize,
        (x * ui_scale).round() as i32,
        (y * ui_scale).round() as i32,
        text_scale.max(1),
        text,
        [(color >> 16) as u8, (color >> 8) as u8, color as u8],
    );
}

/// Draws text centred on a logical X coordinate, accounting for the UI scale.
#[allow(clippy::too_many_arguments)]
fn responsive_text_centered(
    frame: &mut [u8],
    width: u32,
    height: u32,
    center_x: f32,
    y: f32,
    text_scale: usize,
    text: &str,
    color: u32,
) {
    let Some((_, ui_scale, logical_width, _)) = responsive_metrics(width, height) else {
        return;
    };
    let glyph_width = font::text_width(text.chars().count(), text_scale.max(1)) as f32;
    let logical_text_width = glyph_width / ui_scale;
    let x = (center_x - logical_text_width * 0.5).clamp(0.0, logical_width);
    responsive_text(frame, width, height, x, y, text_scale, text, color);
}

/// Vertical span (logical units) of the portrait empty-library refresh button;
/// shared by drawing and hit-testing so the two cannot drift apart again.
fn empty_library_refresh_band(logical_height: f32) -> (f32, f32) {
    const CARD_TOP: f32 = 250.0;
    if logical_height < 720.0 {
        (CARD_TOP + 226.0, CARD_TOP + 226.0 + 44.0)
    } else {
        (CARD_TOP + 308.0, CARD_TOP + 308.0 + 52.0)
    }
}

fn responsive_folder_icon(frame: &mut [u8], width: u32, height: u32, x: f32, y: f32, scale: f32) {
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x - 36.0,
        y - 12.0,
        72.0,
        48.0,
        scale,
        brand::ACCENT_GREEN_LIGHT,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x - 29.0,
        y - 21.0,
        34.0,
        14.0,
        scale,
        brand::ACCENT_GREEN_LIGHT,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x - 31.0,
        y - 7.0,
        62.0,
        38.0,
        scale,
        0x2D1A3B,
    );
}

fn blend_pixel(canvas: &mut [u8], x: i32, y: i32, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    let at = (y as usize * WIDTH + x as usize) * 4;
    for channel in 0..3 {
        canvas[at + channel] =
            (color[channel] as f32 * alpha + canvas[at + channel] as f32 * (1.0 - alpha)) as u8;
    }
}

fn rgb(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

fn fill(canvas: &mut [u8], x: i32, y: i32, width: i32, height: i32, color: [u8; 3]) {
    for py in y.max(0)..(y + height).min(HEIGHT as i32) {
        for px in x.max(0)..(x + width).min(WIDTH as i32) {
            let at = (py as usize * WIDTH + px as usize) * 4;
            canvas[at..at + 3].copy_from_slice(&color);
        }
    }
}

/// Shortens `text` to `max` characters, ending in `...` when cut.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    text.chars().take(keep).collect::<String>() + "..."
}

/// Keeps the last `max` characters, starting with `...` when cut.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    "...".to_string() + &text.chars().skip(count - keep).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::super::frontend::{self, ORIENTATION_PORTRAIT, VirtualControlOffset};
    use super::*;

    fn library(count: usize) -> Library {
        let entries = (0..count)
            .map(|i| Entry {
                title: format!("Game {i}"),
                path: PathBuf::from(format!("g{i}.3ds")),
            })
            .collect();
        let mut library = Library::new(entries, "/games".to_string());
        library.select_page(LibraryPage::Games);
        library
    }

    #[test]
    fn responsive_panels_render_shadow_outline_and_depth_highlights() {
        let (width, height) = (100, 60);
        let mut frame = vec![0u8; width * height * 4];
        responsive_panel(
            &mut frame,
            width as u32,
            height as u32,
            10.0,
            10.0,
            40.0,
            24.0,
            1.0,
            0x21152F,
        );

        let pixel = |x: usize, y: usize| {
            let offset = (y * width + x) * 4;
            [frame[offset], frame[offset + 1], frame[offset + 2]]
        };
        assert_eq!(pixel(52, 20), [0x0C, 0x08, 0x14]);
        assert_eq!(pixel(10, 10), [0x56, 0x3B, 0x67]);
        assert_eq!(pixel(11, 11), [0x21, 0x15, 0x2F]);
        assert_eq!(pixel(12, 12), [0x63, 0x4B, 0x7E]);
        assert_eq!(pixel(12, 32), [0x16, 0x0F, 0x24]);
    }

    #[test]
    fn responsive_text_coordinates_follow_the_surface_scale() {
        let (width, height) = (800, 1200);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        responsive_text(&mut frame, width, height, 10.0, 10.0, 2, "A", 0xE5173F);

        // Portrait UI scale is 2: the first lit pixel in the 5x7 A glyph
        // must appear at logical (11, 10) transformed to physical (22, 20).
        let scaled = (20usize * width as usize + 22) * 4;
        assert_eq!(&frame[scaled..scaled + 3], &[0xE5, 0x17, 0x3F]);

        let unscaled = (10usize * width as usize + 11) * 4;
        assert_eq!(&frame[unscaled..unscaled + 3], &[0, 0, 0]);
    }

    #[test]
    fn responsive_centered_text_uses_measured_bitmap_width() {
        let (width, height) = (800, 1200);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        responsive_text_centered(&mut frame, width, height, 200.0, 40.0, 2, "OK", 0xF5EEF8);
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [0xF5, 0xEE, 0xF8])
        );
    }

    #[test]
    fn responsive_background_is_cover_cropped_and_invalid_frames_are_ignored() {
        let library = Library::new(Vec::new(), "/games".to_string());
        let (width, height) = (800, 1200);
        let plain = library.render_responsive(width, height);
        let background = vec![240u8; WIDTH * HEIGHT * 4];
        let with_background =
            library.render_responsive_with_background(width, height, Some(&background));
        assert_ne!(plain, with_background);
        assert_eq!(with_background.len(), plain.len());

        let invalid = library.render_responsive_with_background(width, height, Some(&[255; 16]));
        assert_eq!(invalid, plain);
    }

    #[test]
    fn responsive_launcher_fills_phone_and_desktop_surfaces() {
        let library = Library::new(Vec::new(), "/games".to_string());
        for (width, height) in [(689, 1536), (960, 600), (1920, 1080)] {
            let frame = library.render_responsive(width, height);
            assert_eq!(frame.len(), width as usize * height as usize * 4);
            assert!(frame.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }
    }

    #[test]
    fn responsive_navigation_filters_games_and_homebrew_and_refreshes_settings() {
        let dir = std::env::temp_dir().join(format!("aceris-responsive-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sample.3dsx"), b"fixture").unwrap();
        fs::write(dir.join("retail.3ds"), b"fixture").unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Games);

        // Refresh from the library header while the Games section is selected.
        assert_eq!(
            library.click_responsive(680, 384, 800, 1200),
            ClickResult::Ignored
        );
        assert_eq!(library.entries().len(), 2);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(
            library.selected_entry().unwrap().path.extension().unwrap(),
            "3ds"
        );

        // Switch to Homebrew and verify the .3dsx-only filter.
        assert_eq!(
            library.click_responsive(400, 250, 800, 1200),
            ClickResult::Ignored
        );
        assert_eq!(library.page(), LibraryPage::Homebrew);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(
            library.selected_entry().unwrap().path.extension().unwrap(),
            "3dsx"
        );

        // Settings exposes the same real refresh action.
        assert_eq!(
            library.click_responsive(700, 250, 800, 1200),
            ClickResult::Ignored
        );
        assert_eq!(library.page(), LibraryPage::Settings);
        assert_eq!(
            library.click_responsive(400, 1_040, 800, 1200),
            ClickResult::Ignored
        );
        assert!(library.status.is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn favorites_and_recent_are_restored_after_reopening_the_library() {
        let dir =
            std::env::temp_dir().join(format!("aceris-library-persistence-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let alpha = dir.join("alpha.3ds");
        let beta = dir.join("beta.3ds");
        fs::write(&alpha, b"fixture").unwrap();
        fs::write(&beta, b"fixture").unwrap();

        {
            let mut library = Library::new(scan_games(&dir), dir.display().to_string());
            library.select_page(LibraryPage::Games);
            assert!(library.toggle_selected_favorite());
            library.mark_launched(&beta);
        }

        let reopened = Library::new(scan_games(&dir), dir.display().to_string());
        assert_eq!(reopened.favorite_count(), 1);
        assert_eq!(reopened.recent_count(), 1);
        assert_eq!(
            fs::read_to_string(dir.join(".aceris").join("favorites.txt")).unwrap(),
            library_path_id(&alpha).unwrap()
        );
        assert_eq!(
            fs::read_to_string(dir.join(".aceris").join("recent.txt")).unwrap(),
            library_path_id(&beta).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn library_view_and_sort_preferences_survive_reopening() {
        let dir =
            std::env::temp_dir().join(format!("aceris-library-preferences-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let games = vec![
            Entry {
                title: "Alpha".to_string(),
                path: dir.join("alpha.3ds"),
            },
            Entry {
                title: "Beta".to_string(),
                path: dir.join("beta.3ds"),
            },
        ];

        {
            let mut library = Library::new(games.clone(), dir.display().to_string());
            library.select_page(LibraryPage::Games);
            assert!(!library.is_list_view());
            assert!(!library.sort_descending);
            library.toggle_view_mode();
            library.toggle_sort_order();
            assert!(library.is_list_view());
            assert!(library.sort_descending);
        }

        let reopened = Library::new(games, dir.display().to_string());
        assert!(reopened.is_list_view());
        assert!(reopened.sort_descending);
        assert_eq!(
            fs::read_to_string(dir.join(".aceris").join(LIBRARY_PREFERENCES_FILE)).unwrap(),
            format!("{LIBRARY_PREFERENCES_MAGIC}\nview=list\nsort=za\n")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_library_preferences_fall_back_to_safe_defaults() {
        let dir = std::env::temp_dir().join(format!(
            "aceris-library-preferences-invalid-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(LIBRARY_PREFERENCES_FILE),
            format!("{LIBRARY_PREFERENCES_MAGIC}\nview=unknown\nsort=za\n"),
        )
        .unwrap();
        let preferences = load_library_preferences(&dir);
        assert_eq!(preferences, LibraryPreferences::default());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn favorites_and_recent_distinguish_same_filename_in_different_roots() {
        let root =
            std::env::temp_dir().join(format!("aceris-duplicate-names-{}", std::process::id()));
        let secondary = root.join("secondary");
        fs::create_dir_all(&secondary).unwrap();
        let primary_game = root.join("shared.3ds");
        let secondary_game = secondary.join("shared.3ds");
        fs::write(&primary_game, b"primary").unwrap();
        fs::write(&secondary_game, b"secondary").unwrap();

        let entries = vec![
            Entry {
                title: "Primary".into(),
                path: primary_game.clone(),
            },
            Entry {
                title: "Secondary".into(),
                path: secondary_game.clone(),
            },
        ];
        let mut library = Library::new(entries.clone(), root.display().to_string());
        library.select_page(LibraryPage::Games);
        library.move_selection(1);
        assert_eq!(library.selected_entry().unwrap().path, secondary_game);
        assert!(library.toggle_selected_favorite());
        assert!(library.is_favorite(&entries[1]));
        assert!(!library.is_favorite(&entries[0]));

        library.mark_launched(&primary_game);
        library.mark_launched(&secondary_game);
        assert_eq!(library.recent_count(), 2);
        library.select_page(LibraryPage::Recent);
        assert_eq!(library.selected_entry().unwrap().path, secondary_game);
        library.select_page(LibraryPage::Home);
        assert_eq!(library.selected_entry().unwrap().path, secondary_game);

        let reopened = Library::new(entries, root.display().to_string());
        assert!(reopened.is_favorite(&reopened.entries()[1]));
        assert!(!reopened.is_favorite(&reopened.entries()[0]));
        assert_eq!(reopened.recent_count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_filename_ids_migrate_to_the_original_games_folder() {
        let root =
            std::env::temp_dir().join(format!("aceris-legacy-library-ids-{}", std::process::id()));
        let secondary = root.join("secondary");
        let settings = root.join(".aceris");
        fs::create_dir_all(&secondary).unwrap();
        fs::create_dir_all(&settings).unwrap();
        let primary_game = root.join("legacy.3ds");
        let secondary_game = secondary.join("legacy.3ds");
        fs::write(&primary_game, b"primary").unwrap();
        fs::write(&secondary_game, b"secondary").unwrap();
        fs::write(settings.join("favorites.txt"), "legacy.3ds").unwrap();
        fs::write(settings.join("recent.txt"), "legacy.3ds").unwrap();

        let entries = vec![
            Entry {
                title: "Primary".into(),
                path: primary_game.clone(),
            },
            Entry {
                title: "Secondary".into(),
                path: secondary_game.clone(),
            },
        ];
        let mut library = Library::new(entries.clone(), root.display().to_string());
        assert!(library.is_favorite(&entries[0]));
        assert!(!library.is_favorite(&entries[1]));
        library.select_page(LibraryPage::Recent);
        assert_eq!(library.selected_entry().unwrap().path, primary_game);
        assert_eq!(
            fs::read_to_string(settings.join("favorites.txt")).unwrap(),
            library_path_id(&primary_game).unwrap()
        );
        assert_eq!(
            fs::read_to_string(settings.join("recent.txt")).unwrap(),
            library_path_id(&primary_game).unwrap()
        );

        // A newly favourited file with the same basename remains independent.
        library.select_page(LibraryPage::Games);
        library.move_selection(1);
        assert!(library.toggle_selected_favorite());
        assert!(library.is_favorite(&entries[0]));
        assert!(library.is_favorite(&entries[1]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn favorite_count_ignores_files_missing_from_the_game_folder() {
        let dir =
            std::env::temp_dir().join(format!("aceris-favorite-count-{}", std::process::id()));
        fs::create_dir_all(dir.join(".aceris")).unwrap();
        fs::write(dir.join("alpha.3ds"), b"fixture").unwrap();
        fs::write(
            dir.join(".aceris").join("favorites.txt"),
            "alpha.3ds\nremoved.3ds",
        )
        .unwrap();

        let library = Library::new(scan_games(&dir), dir.display().to_string());
        assert_eq!(library.favorite_count(), 1);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn landscape_settings_toolbar_refresh_matches_its_visible_bounds() {
        let dir =
            std::env::temp_dir().join(format!("aceris-landscape-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sample.3ds"), b"fixture").unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Settings);

        // Landscape Settings uses the shared header refresh control at the top-right.
        assert_eq!(
            library.click_responsive(870, 405, 1000, 600),
            ClickResult::Ignored
        );
        assert_eq!(library.entries().len(), 1);
        assert!(library.status_message().is_some());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn home_continue_uses_the_most_recent_game() {
        let dir = std::env::temp_dir().join(format!("aceris-home-recent-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latest.3ds");
        fs::write(&path, b"fixture").unwrap();
        let entry = Entry {
            title: "Latest game".into(),
            path: path.clone(),
        };
        let mut library = Library::new(vec![entry], dir.display().to_string());
        assert_eq!(library.page(), LibraryPage::Home);
        assert!(library.selected_entry().is_none());
        library.mark_launched(&path);
        assert_eq!(library.selected_entry().unwrap().title, "Latest game");
        assert_eq!(
            library.click_responsive(200, 200, 1000, 600),
            ClickResult::Activated
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn search_matches_titles_paths_and_ignores_case_and_accents() {
        let entries = vec![
            Entry {
                title: "Pokémon Alpha".into(),
                path: PathBuf::from("/games/Pokémon Alpha.3ds"),
            },
            Entry {
                title: "Beta Adventure".into(),
                path: PathBuf::from("/games/beta.cia"),
            },
            Entry {
                title: "Demo".into(),
                path: PathBuf::from("/games/homebrew/demo.3dsx"),
            },
        ];
        let mut library = Library::new(entries, "/games".into());
        library.select_page(LibraryPage::Games);
        library.focus_search();
        for character in "POKEMON".chars() {
            assert!(library.input_search_char(character));
        }
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Pokémon Alpha");

        library.clear_search();
        for character in "BETA.CIA".chars() {
            library.input_search_char(character);
        }
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Beta Adventure");
        library.finish_search();
        assert!(!library.input_search_char('x'));
    }

    #[test]
    fn touch_keyboard_opens_types_clears_and_finishes_search() {
        let mut library = library(12);
        library.set_touch_keyboard_enabled(true);
        assert_eq!(
            library.click_responsive(200, 142, 1000, 600),
            ClickResult::Ignored
        );
        assert!(library.search_is_focused());
        assert!(library.touch_keyboard_is_open());

        let keyboard_top = search_keyboard_top(600.0);
        assert_eq!(
            library.click_responsive(248, (keyboard_top + 25.0) as i32, 1000, 600),
            ClickResult::Ignored
        );
        assert_eq!(library.search_query(), "q");

        assert_eq!(
            library.click_responsive(550, (keyboard_top + 145.0) as i32, 1000, 600),
            ClickResult::Ignored
        );
        assert!(library.search_query().is_empty());
        assert!(library.touch_keyboard_is_open());

        assert_eq!(
            library.click_responsive(706, (keyboard_top + 145.0) as i32, 1000, 600),
            ClickResult::Ignored
        );
        assert!(!library.touch_keyboard_is_open());
        assert!(!library.search_is_focused());
    }

    #[test]
    fn landscape_library_can_switch_between_grid_and_list() {
        let mut library = library(12);
        assert!(!library.is_list_view());
        assert_eq!(
            library.click_responsive(574, 142, 1000, 600),
            ClickResult::Ignored
        );
        assert!(library.is_list_view());
        assert_eq!(
            library.click_responsive(574, 142, 1000, 600),
            ClickResult::Ignored
        );
        assert!(!library.is_list_view());
    }

    #[test]
    fn list_mode_uses_vertical_row_navigation_and_ignores_horizontal_steps() {
        let mut library = library(12);
        library.toggle_view_mode();
        library.move_selection_responsive(1, 1000, 600);
        assert_eq!(library.selected_index(), 1);
        library.move_selection_horizontal_responsive(1, 1000, 600);
        assert_eq!(library.selected_index(), 1);
        library.move_selection_responsive(1, 1000, 600);
        assert_eq!(library.selected_index(), 2);
    }

    #[test]
    fn list_mode_clicks_rows_and_favorite_control_independently() {
        let dir = std::env::temp_dir().join(format!("aceris-list-view-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let entries = (0..8)
            .map(|index| Entry {
                title: format!("Game {index}"),
                path: PathBuf::from(format!("g{index}.3ds")),
            })
            .collect();
        let mut library = Library::new(entries, dir.display().to_string());
        library.select_page(LibraryPage::Games);
        library.toggle_view_mode();

        assert_eq!(
            library.click_responsive(420, 245, 1000, 600),
            ClickResult::Selected
        );
        assert_eq!(library.selected_entry().unwrap().title, "Game 1");
        assert_eq!(
            library.click_responsive(420, 245, 1000, 600),
            ClickResult::Activated
        );

        assert_eq!(
            library.click_responsive(870, 307, 1000, 600),
            ClickResult::Ignored
        );
        assert_eq!(library.favorite_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Game 1");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn horizontal_grid_navigation_moves_by_rows_and_columns() {
        let mut library = library(18);
        library.move_selection_responsive(1, 1000, 480);
        assert_eq!(library.selected_index(), 4);
        library.move_selection_horizontal_responsive(1, 1000, 480);
        assert_eq!(library.selected_index(), 5);
        library.move_selection_responsive(-1, 1000, 480);
        assert_eq!(library.selected_index(), 1);
    }

    #[test]
    fn library_sort_order_and_external_media_indicator_are_stateful() {
        let entries = vec![
            Entry {
                title: "Alpha".into(),
                path: PathBuf::from("alpha.3ds"),
            },
            Entry {
                title: "Beta".into(),
                path: PathBuf::from("beta.3ds"),
            },
        ];
        let mut library = Library::new(entries, "/games".into());
        library.select_page(LibraryPage::Games);
        assert_eq!(library.selected_entry().unwrap().title, "Alpha");
        library.toggle_sort_order();
        assert_eq!(library.selected_entry().unwrap().title, "Beta");
        library.toggle_sort_order();
        assert_eq!(library.selected_entry().unwrap().title, "Alpha");

        assert!(!library.external_media_detection_available());
        assert!(!library.external_media_playing());
        library.set_external_media_detection_available(true);
        assert!(library.external_media_detection_available());
        library.set_external_media_playing(true);
        assert!(library.external_media_playing());
        library.set_external_media_detection_available(false);
        assert!(!library.external_media_detection_available());
        assert!(!library.external_media_playing());
    }

    #[test]
    fn homebrew_filter_does_not_mix_executable_cartridges() {
        let entries = vec![
            Entry {
                title: "Retail".into(),
                path: PathBuf::from("retail.3ds"),
            },
            Entry {
                title: "Demo".into(),
                path: PathBuf::from("demo.3dsx"),
            },
        ];
        let mut library = Library::new(entries, "/games".into());
        library.select_page(LibraryPage::Games);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Retail");
        library.select_page(LibraryPage::Homebrew);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Demo");
        library.select_page(LibraryPage::Settings);
        assert_eq!(library.visible_entry_count(), 0);
        assert!(library.selected_entry().is_none());
    }

    #[test]
    fn selection_clamps_and_scrolls_to_stay_visible() {
        let mut lib = library(30);
        lib.move_selection(-5);
        assert_eq!(lib.selected_index(), 0);
        lib.move_selection(VISIBLE_ROWS as i32 + 2);
        assert_eq!(lib.selected_index(), VISIBLE_ROWS + 2);
        assert!(lib.selected_index() < lib.first_visible + VISIBLE_ROWS);
        lib.move_selection(1000);
        assert_eq!(lib.selected_index(), 29);
        lib.move_selection(-1000);
        assert_eq!((lib.selected_index(), lib.first_visible), (0, 0));
        assert_eq!(library(0).selected_entry(), None);
    }

    #[test]
    fn click_selects_then_activates_and_ignores_empty_areas() {
        let mut lib = library(3);
        let row_y = |row: i32| LIST_TOP + row * ROW_HEIGHT + 4;
        assert_eq!(lib.click(100, row_y(2)), ClickResult::Selected);
        assert_eq!(lib.selected_index(), 2);
        assert_eq!(lib.click(100, row_y(2)), ClickResult::Activated);
        assert_eq!(lib.click(100, row_y(5)), ClickResult::Ignored);
        assert_eq!(lib.click(100, 10), ClickResult::Ignored);
    }

    #[test]
    fn render_has_the_canvas_size_and_marks_the_selected_row() {
        let mut lib = library(4);
        lib.move_selection(1);
        let frame = lib.render();
        assert_eq!(frame.len(), WIDTH * HEIGHT * 4);
        let at = ((LIST_TOP + ROW_HEIGHT + 20) as usize * WIDTH + 12) * 4;
        assert_eq!(&frame[at..at + 3], &rgb(brand::ACCENT_RED));
        // The empty and status screens must render without panicking.
        let mut empty = library(0);
        empty.set_status("COULD NOT LOAD");
        assert_eq!(empty.render().len(), WIDTH * HEIGHT * 4);
        assert_eq!(library(500).render().len(), WIDTH * HEIGHT * 4);
    }

    #[test]
    fn gamepad_deadzone_is_versioned_persisted_and_cycles_from_settings() {
        let root =
            std::env::temp_dir().join(format!("aceris-gamepad-deadzone-{}", std::process::id()));
        let settings = root.join(".aceris");
        let mut library = Library::new(Vec::new(), root.display().to_string());
        assert_eq!(library.gamepad_deadzone_percent(), 16);
        library.scan_settings_directory = settings.clone();

        library.set_gamepad_deadzone_percent(20).unwrap();
        assert_eq!(library.gamepad_deadzone_percent(), 20);
        assert_eq!(
            fs::read_to_string(settings.join(GAMEPAD_SETTINGS_FILE)).unwrap(),
            "ACERIS_GAMEPAD_SETTINGS_V1\ndeadzone_percent=20\n"
        );
        assert_eq!(
            Library::from_scan_settings(&settings, &root).gamepad_deadzone_percent(),
            20
        );

        library.cycle_gamepad_deadzone_from_settings();
        assert_eq!(library.gamepad_deadzone_percent(), 24);
        assert!(library.set_gamepad_deadzone_percent(99).is_err());

        fs::write(
            settings.join(GAMEPAD_SETTINGS_FILE),
            "ACERIS_GAMEPAD_SETTINGS_V1\ndeadzone_percent=99\n",
        )
        .unwrap();
        assert_eq!(
            Library::from_scan_settings(&settings, &root).gamepad_deadzone_percent(),
            16
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn performance_mode_is_opt_in_persisted_and_toggled_from_settings() {
        let root = std::env::temp_dir().join(format!("aceris-lib-perf-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let settings = root.join("settings");
        fs::create_dir_all(&root).unwrap();

        let mut library = Library::from_scan_settings(&settings, &root);
        assert!(!library.performance_mode());
        library.select_page(LibraryPage::Settings);

        // Landscape 1000x600: performance and deadzone have separate hit targets.
        library.click_responsive(540, 301, 1000, 600);
        assert!(library.performance_mode());
        assert!(Library::from_scan_settings(&settings, &root).performance_mode());
        library.click_responsive(667, 301, 1000, 600);
        assert_eq!(library.gamepad_deadzone_percent(), 20);
        assert_eq!(
            Library::from_scan_settings(&settings, &root).gamepad_deadzone_percent(),
            20
        );
        assert_eq!(library.render_responsive(1000, 600).len(), 1000 * 600 * 4);

        // Portrait 400x800: both controls move below "ATUALIZAR BIBLIOTECA".
        library.click_responsive(117, 616, 400, 800);
        assert!(!library.performance_mode());
        assert!(!Library::from_scan_settings(&settings, &root).performance_mode());
        library.click_responsive(283, 616, 400, 800);
        assert_eq!(library.gamepad_deadzone_percent(), 24);
        assert_eq!(
            Library::from_scan_settings(&settings, &root).gamepad_deadzone_percent(),
            24
        );
        assert_eq!(library.render_responsive(400, 800).len(), 400 * 800 * 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn library_view_and_sort_preferences_are_accessible_in_settings_on_both_orientations() {
        let root = std::env::temp_dir().join(format!(
            "aceris-library-view-settings-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let settings = root.join("settings");

        let mut library = Library::from_scan_settings(&settings, &root);
        library.select_page(LibraryPage::Settings);
        assert!(!library.is_list_view());
        assert!(!library.sort_descending);

        // Desktop / landscape settings expose both choices in the library settings card.
        library.click_responsive(136, 410, 1000, 600);
        library.click_responsive(365, 410, 1000, 600);
        assert!(library.is_list_view());
        assert!(library.sort_descending);

        let mut library = Library::from_scan_settings(&settings, &root);
        library.select_page(LibraryPage::Settings);
        // Tall Android portrait settings expose the same choices below the existing options.
        library.click_responsive(200, 670, 400, 800);
        library.click_responsive(200, 714, 400, 800);
        assert!(!library.is_list_view());
        assert!(!library.sort_descending);

        let reopened = Library::from_scan_settings(&settings, &root);
        assert!(!reopened.is_list_view());
        assert!(!reopened.sort_descending);
        assert_eq!(reopened.render_responsive(1000, 600).len(), 1000 * 600 * 4);
        assert_eq!(reopened.render_responsive(400, 800).len(), 400 * 800 * 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn truncation_helpers_respect_the_limit() {
        assert_eq!(truncate("ABCDEFGHIJ", 6), "ABC...");
        assert_eq!(truncate("ABC", 6), "ABC");
        assert_eq!(tail("/a/long/path/here", 8), ".../here");
    }

    #[test]
    fn scan_lists_supported_image_containers_sorted() {
        let dir = std::env::temp_dir().join(format!("aceris-lib-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["b.3ds", "A.CCI", "homebrew.3dsx", "notes.txt", "C.CIA"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let titles: Vec<String> = scan_games(&dir).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, ["A", "b", "C", "homebrew"]);
        fs::remove_dir_all(&dir).unwrap();
        assert!(scan_games(&dir).is_empty());
    }

    #[test]
    fn legacy_virtual_control_positions_migrate_to_v2_on_save() {
        let root = std::env::temp_dir().join(format!(
            "aceris-virtual-control-migration-{}",
            std::process::id()
        ));
        let games = root.join("games");
        let settings = root.join("settings");
        fs::create_dir_all(&games).unwrap();
        fs::create_dir_all(&settings).unwrap();

        let mut old_positions = VirtualControlPositions::default();
        assert!(old_positions.adjust(ORIENTATION_PORTRAIT, frontend::buttons::A, 450, -250));
        let mut legacy = String::from("ACERIS_VIRTUAL_CONTROL_POSITIONS_V1\n");
        for line in old_positions.to_config_string().lines().skip(1) {
            let (key, values) = line.split_once('=').unwrap();
            let (position, _size) = values.rsplit_once(',').unwrap();
            legacy.push_str(&format!("{key}={position}\n"));
        }
        fs::write(settings.join(LEGACY_VIRTUAL_CONTROL_POSITIONS_FILE), legacy).unwrap();

        let mut library = Library::from_scan_settings(&settings, &games);
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, frontend::buttons::A),
            VirtualControlOffset { x: 450, y: -250 }
        );
        assert_eq!(
            library
                .virtual_control_positions()
                .size_percent(ORIENTATION_PORTRAIT, frontend::buttons::A),
            100
        );
        assert!(library.resize_virtual_control(ORIENTATION_PORTRAIT, frontend::buttons::A, 135));
        library.save_virtual_control_positions().unwrap();

        let migrated = fs::read_to_string(settings.join(VIRTUAL_CONTROL_LAYOUT_FILE)).unwrap();
        assert!(migrated.starts_with("ACERIS_VIRTUAL_CONTROL_LAYOUT_V2\n"));
        let reopened = Library::from_scan_settings(&settings, &games);
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, frontend::buttons::A),
            VirtualControlOffset { x: 450, y: -250 }
        );
        assert_eq!(
            reopened
                .virtual_control_positions()
                .size_percent(ORIENTATION_PORTRAIT, frontend::buttons::A),
            135
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn library_model_loads_and_manages_persisted_scan_roots() {
        let root =
            std::env::temp_dir().join(format!("aceris-library-scan-model-{}", std::process::id()));
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        let third = root.join("third");
        let settings = root.join("app-settings");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(secondary.join("nested")).unwrap();
        fs::create_dir_all(&third).unwrap();
        fs::write(primary.join("base.3ds"), b"x").unwrap();
        fs::write(secondary.join("nested").join("extra.3ds"), b"x").unwrap();
        fs::write(third.join("third.cia"), b"x").unwrap();

        let mut config = LibraryScanConfig::single_directory(primary.clone());
        assert!(config.add_directory(secondary.clone()));
        config.options.recursive = true;
        save_library_scan_config(&settings, &config).unwrap();

        let mut library = Library::from_scan_settings(&settings, &primary);
        assert_eq!(library.entries().len(), 2);
        assert!(library.scan_config().options.recursive);
        assert_eq!(library.scan_config().directories.len(), 2);

        assert!(library.set_recursive_scan(false).unwrap());
        assert_eq!(library.entries().len(), 1);
        assert!(!library.set_recursive_scan(false).unwrap());

        assert!(library.add_scan_directory(third.clone()).unwrap());
        assert_eq!(library.entries().len(), 2);
        assert!(!library.add_scan_directory(third.clone()).unwrap());

        assert!(library.remove_scan_directory(&secondary).unwrap());
        assert_eq!(library.entries().len(), 2);
        assert!(!library.remove_scan_directory(&secondary).unwrap());
        assert!(library.remove_scan_directory(&primary).unwrap());
        assert_eq!(library.scan_config().directories.len(), 1);
        assert!(!library.remove_scan_directory(&third).unwrap());

        let reopened = Library::from_scan_settings(&settings, &primary);
        assert_eq!(reopened.entries().len(), 1);
        assert_eq!(reopened.scan_config().directories.len(), 1);
        assert_eq!(
            reopened
                .entries()
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["third"]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_library_scan_config_write_does_not_change_live_library_state() {
        let root = std::env::temp_dir().join(format!(
            "aceris-library-scan-write-fail-{}",
            std::process::id()
        ));
        let games = root.join("games");
        let blocked_settings = root.join("not-a-directory");
        fs::create_dir_all(&games).unwrap();
        fs::write(games.join("base.3ds"), b"x").unwrap();
        fs::write(&blocked_settings, b"block").unwrap();

        let mut library = Library::from_scan_settings(&blocked_settings, &games);
        let original_config = library.scan_config().clone();
        let additional = root.join("additional");
        fs::create_dir_all(&additional).unwrap();

        assert!(library.add_scan_directory(additional).is_err());
        assert_eq!(library.scan_config(), &original_config);
        assert_eq!(library.entries().len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn library_scan_config_round_trips_multiple_folders_and_recursion() {
        let root = std::env::temp_dir().join(format!(
            "aceris-scan-config-roundtrip-{}",
            std::process::id()
        ));
        let primary = root.join("primary");
        let secondary = root.join("external").join("games");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(secondary.join("nested")).unwrap();
        fs::write(primary.join("base.3ds"), b"x").unwrap();
        fs::write(secondary.join("nested").join("nested.CCI"), b"x").unwrap();
        let settings = root.join("settings");

        let mut config = LibraryScanConfig::single_directory(primary.clone());
        assert!(config.add_directory(secondary.clone()));
        config.options.recursive = true;
        save_library_scan_config(&settings, &config).unwrap();

        let loaded = load_library_scan_config(&settings, &primary);
        assert!(loaded.options.recursive);
        assert_eq!(
            loaded.directories,
            vec![
                fs::canonicalize(&primary).unwrap(),
                fs::canonicalize(&secondary).unwrap()
            ]
        );
        let titles = scan_games_from_config(&loaded)
            .into_iter()
            .map(|entry| entry.title)
            .collect::<Vec<_>>();
        assert_eq!(titles, ["base", "nested"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn library_scan_config_deduplicates_aliases_and_keeps_one_root() {
        let root =
            std::env::temp_dir().join(format!("aceris-scan-config-dedup-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        let mut config = LibraryScanConfig::single_directory(first.clone());
        assert!(!config.add_directory(first.clone()));
        assert!(config.add_directory(second.clone()));
        assert_eq!(config.directories.len(), 2);
        assert!(config.remove_directory(&first));
        assert_eq!(config.directories, vec![fs::canonicalize(&second).unwrap()]);
        assert!(!config.remove_directory(&second));
        assert_eq!(config.directories.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_library_scan_config_falls_back_to_the_original_folder() {
        let root =
            std::env::temp_dir().join(format!("aceris-scan-config-invalid-{}", std::process::id()));
        let settings = root.join("settings");
        let fallback = root.join("original-games");
        fs::create_dir_all(&settings).unwrap();
        fs::write(
            settings.join(LIBRARY_SCAN_CONFIG_FILE),
            "ACERIS_LIBRARY_SCAN_V1\nrecursive=not-a-boolean\nfolder=zz",
        )
        .unwrap();

        let config = load_library_scan_config(&settings, &fallback);
        assert_eq!(
            config.directories,
            vec![normalize_scan_directory(&fallback)]
        );
        assert!(!config.options.recursive);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saving_library_scan_config_rejects_an_empty_folder_list() {
        let root =
            std::env::temp_dir().join(format!("aceris-scan-config-empty-{}", std::process::id()));
        let config = LibraryScanConfig {
            directories: Vec::new(),
            options: ScanOptions::default(),
        };
        let error = save_library_scan_config(&root, &config).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!root.exists());
    }

    #[test]
    fn scan_recursion_is_opt_in() {
        let dir = std::env::temp_dir().join(format!("aceris-lib-recursive-{}", std::process::id()));
        let nested = dir.join("updates").join("demos");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("base.3ds"), b"x").unwrap();
        fs::write(nested.join("nested.CCI"), b"x").unwrap();

        let direct = scan_games(&dir);
        assert_eq!(
            direct
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["base"]
        );

        let recursive = scan_games_from_directories(
            std::slice::from_ref(&dir),
            ScanOptions { recursive: true },
        );
        assert_eq!(
            recursive
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["base", "nested"]
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_multiple_directories_deduplicates_overlapping_roots() {
        let dir = std::env::temp_dir().join(format!("aceris-lib-roots-{}", std::process::id()));
        let first = dir.join("first");
        let nested = first.join("nested");
        let second = dir.join("second");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("alpha.3ds"), b"x").unwrap();
        fs::write(nested.join("shared.3dsx"), b"x").unwrap();
        fs::write(second.join("beta.cia"), b"x").unwrap();

        let entries = scan_games_from_directories(
            &[first.clone(), second.clone(), nested.clone(), first.clone()],
            ScanOptions { recursive: true },
        );
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "shared"]
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.path == nested.join("shared.3dsx"))
                .count(),
            1
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_skips_missing_roots_without_discarding_available_folders() {
        let dir = std::env::temp_dir().join(format!("aceris-lib-missing-{}", std::process::id()));
        let available = dir.join("available");
        let missing = dir.join("missing");
        fs::create_dir_all(&available).unwrap();
        fs::write(available.join("working.3ds"), b"x").unwrap();

        let entries = scan_games_from_directories(&[missing, available], ScanOptions::default());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "working");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn settings_page_adds_removes_and_toggles_persisted_scan_roots() {
        let dir =
            std::env::temp_dir().join(format!("aceris-settings-scan-ui-{}", std::process::id()));
        let primary = dir.join("primary");
        let secondary = dir.join("secondary");
        let nested = secondary.join("nested");
        let settings = dir.join("settings");
        fs::create_dir_all(&primary).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::write(primary.join("base.3ds"), b"x").unwrap();
        fs::write(nested.join("extra.cci"), b"x").unwrap();

        let config = LibraryScanConfig::single_directory(primary.clone());
        save_library_scan_config(&settings, &config).unwrap();
        let mut library = Library::from_scan_settings(&settings, &primary);
        library.select_page(LibraryPage::Settings);
        assert_eq!(library.entries().len(), 1);

        library.click_responsive(365, 301, 1000, 600);
        assert!(library.scan_config().options.recursive);

        library.click_responsive(136, 301, 1000, 600);
        assert!(library.search_is_focused());
        for character in secondary.to_string_lossy().chars() {
            assert!(library.input_search_char(character));
        }
        library.finish_search();

        assert_eq!(library.scan_config().directories.len(), 2);
        assert_eq!(library.entries().len(), 2);
        assert!(!library.search_is_focused());

        library.click_responsive(694, 218, 1000, 600);
        assert_eq!(library.scan_config().directories.len(), 1);
        assert_eq!(library.entries().len(), 1);

        library.click_responsive(694, 185, 1000, 600);
        assert_eq!(library.scan_config().directories.len(), 1);
        assert_eq!(
            library.status_message(),
            Some("MANTENHA AO MENOS UMA PASTA NA BIBLIOTECA")
        );

        let reopened = Library::from_scan_settings(&settings, &primary);
        assert_eq!(reopened.scan_config().directories.len(), 1);
        assert!(reopened.scan_config().options.recursive);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn console_home_add_folder_action_opens_path_entry() {
        let dir =
            std::env::temp_dir().join(format!("aceris-console-add-folder-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.set_touch_keyboard_enabled(true);

        assert_eq!(
            library.click_responsive(870, 355, 1000, 600),
            ClickResult::Ignored
        );
        assert_eq!(library.page(), LibraryPage::Settings);
        assert!(library.search_is_focused());
        assert!(library.touch_keyboard_is_open());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancelling_folder_path_input_does_not_add_a_root() {
        let dir = std::env::temp_dir().join(format!(
            "aceris-settings-cancel-path-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Settings);
        library.click_responsive(136, 301, 1000, 600);
        for character in "/path/that/does/not/exist".chars() {
            library.input_search_char(character);
        }
        library.cancel_text_entry();

        assert_eq!(library.scan_config().directories.len(), 1);
        assert_eq!(library.search_query(), "");
        assert!(!library.search_is_focused());
        assert_eq!(library.status_message(), Some("ADIÇÃO DE PASTA CANCELADA"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_path_touch_keyboard_inserts_slash_and_dot() {
        let dir =
            std::env::temp_dir().join(format!("aceris-settings-touch-path-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Settings);
        library.set_touch_keyboard_enabled(true);

        library.click_responsive(136, 301, 1000, 600);
        assert!(library.touch_keyboard_is_open());

        let keyboard_top = search_keyboard_top(600.0);
        library.click_responsive(815, (keyboard_top + 105.0) as i32, 1000, 600);
        library.click_responsive(881, (keyboard_top + 105.0) as i32, 1000, 600);
        assert_eq!(library.search_query(), "/.");

        library.clear_search();
        assert_eq!(library.search_query(), "");
        library.finish_search();
        assert_eq!(library.scan_config().directories.len(), 1);
        assert_eq!(library.status_message(), Some("NENHUM CAMINHO INFORMADO"));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn file_format_filter_cycles_and_matches_extensions_case_insensitively() {
        let dir = std::env::temp_dir().join(format!("aceris-format-filter-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in [
            "alpha.3ds",
            "beta.CCI",
            "gamma.cia",
            "demo.3dsx",
            "notes.txt",
        ] {
            fs::write(dir.join(name), b"fixture").unwrap();
        }

        let mut library = Library::new(scan_games(&dir), dir.display().to_string());
        library.select_page(LibraryPage::Games);
        assert_eq!(library.visible_entry_count(), 3);

        for (filter, expected_title) in [
            (LibraryFormatFilter::ThreeDs, Some("alpha")),
            (LibraryFormatFilter::Cci, Some("beta")),
            (LibraryFormatFilter::Cia, Some("gamma")),
            (LibraryFormatFilter::ThreeDsX, None),
        ] {
            library.cycle_format_filter();
            assert_eq!(library.format_filter(), filter);
            let expected_count = if expected_title.is_some() { 1 } else { 0 };
            assert_eq!(library.visible_entry_count(), expected_count);
            if let Some(title) = expected_title {
                assert_eq!(library.selected_entry().unwrap().title, title);
            }
        }

        library.select_page(LibraryPage::Homebrew);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "demo");
        library.cycle_format_filter();
        assert_eq!(library.format_filter(), LibraryFormatFilter::All);
        assert_eq!(library.visible_entry_count(), 1);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn responsive_format_filter_control_works_in_portrait_and_landscape() {
        let dir =
            std::env::temp_dir().join(format!("aceris-format-filter-ui-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("alpha.3ds"), b"fixture").unwrap();
        fs::write(dir.join("beta.cci"), b"fixture").unwrap();

        let mut library = Library::new(scan_games(&dir), dir.display().to_string());
        library.select_page(LibraryPage::Games);

        assert_eq!(
            library.click_responsive(200, 234, 400, 800),
            ClickResult::Ignored
        );
        assert_eq!(library.format_filter(), LibraryFormatFilter::ThreeDs);
        assert_eq!(library.visible_entry_count(), 1);

        assert_eq!(
            library.click_responsive(464, 142, 1000, 600),
            ClickResult::Ignored
        );
        assert_eq!(library.format_filter(), LibraryFormatFilter::Cci);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "beta");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn virtual_touch_control_visibility_persists_and_settings_toggle_works_on_both_orientations() {
        let dir = std::env::temp_dir().join(format!(
            "aceris-touch-controls-settings-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();

        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Settings);
        library.set_touch_keyboard_enabled(true);
        assert!(library.virtual_controls_visible());

        // The portrait control replaces the duplicated refresh action on touch hosts.
        library.click_responsive(200, 580, 400, 800);
        assert!(!library.virtual_controls_visible());

        let reopened = Library::new(Vec::new(), dir.display().to_string());
        assert!(!reopened.virtual_controls_visible());

        let mut library = reopened;
        library.select_page(LibraryPage::Settings);
        library.set_touch_keyboard_enabled(true);
        library.set_virtual_controls_visible(true).unwrap();
        library.click_responsive(600, 410, 1000, 600);
        assert!(!library.virtual_controls_visible());

        // Opacity, size and preset occupy separate thirds of the landscape row.
        assert_eq!(library.virtual_controls_size_percent(), 100);
        assert_eq!(
            library.virtual_controls_preset(),
            VirtualControlPreset::Standard
        );
        library.click_responsive(525, 458, 1000, 600);
        assert_eq!(library.virtual_controls_opacity(), 204);
        library.click_responsive(607, 458, 1000, 600);
        assert_eq!(library.virtual_controls_size_percent(), 120);
        library.click_responsive(690, 458, 1000, 600);
        assert_eq!(
            library.virtual_controls_preset(),
            VirtualControlPreset::Compact
        );
        library.click_responsive(690, 458, 1000, 600);
        assert_eq!(
            library.virtual_controls_preset(),
            VirtualControlPreset::LeftHanded
        );
        let reopened = Library::new(Vec::new(), dir.display().to_string());
        assert!(!reopened.virtual_controls_visible());
        assert_eq!(reopened.virtual_controls_opacity(), 204);
        assert_eq!(reopened.virtual_controls_size_percent(), 120);
        assert_eq!(
            reopened.virtual_controls_preset(),
            VirtualControlPreset::LeftHanded
        );

        // Portrait exposes all three controls with the same independent persistence.
        let mut portrait = reopened;
        portrait.select_page(LibraryPage::Settings);
        portrait.set_touch_keyboard_enabled(true);
        portrait.click_responsive(90, 616, 400, 800);
        assert_eq!(portrait.virtual_controls_opacity(), 153);
        portrait.click_responsive(200, 616, 400, 800);
        assert_eq!(portrait.virtual_controls_size_percent(), 80);
        portrait.click_responsive(310, 616, 400, 800);
        assert_eq!(
            portrait.virtual_controls_preset(),
            VirtualControlPreset::RightHanded
        );
        let reopened = Library::new(Vec::new(), dir.display().to_string());
        assert_eq!(reopened.virtual_controls_opacity(), 153);
        assert_eq!(reopened.virtual_controls_size_percent(), 80);
        assert_eq!(
            reopened.virtual_controls_preset(),
            VirtualControlPreset::RightHanded
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recursive_scan_does_not_follow_symbolic_directory_links() {
        use std::os::unix::fs::symlink;

        let dir = std::env::temp_dir().join(format!("aceris-lib-symlink-{}", std::process::id()));
        let root = dir.join("root");
        let external = dir.join("external");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("outside.3ds"), b"x").unwrap();
        symlink(&external, root.join("linked")).unwrap();

        let entries = scan_games_from_directories(
            std::slice::from_ref(&root),
            ScanOptions { recursive: true },
        );
        assert!(entries.is_empty());

        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod virtual_control_profile_tests {
    use super::super::frontend::{
        ORIENTATION_LANDSCAPE, ORIENTATION_PORTRAIT, VirtualControlOffset, VirtualControlPositions,
        buttons,
    };
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aceris-control-profiles-{}-{nonce}",
            std::process::id()
        ))
    }

    fn identity(title_id: u64) -> GameIdentity {
        GameIdentity::from_title_id(title_id).unwrap()
    }

    #[test]
    fn title_profiles_inherit_global_then_materialize_and_persist_independently() {
        let directory = temporary_directory();
        let mut library = Library::new(Vec::new(), directory.display().to_string());
        assert!(library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 250, -150));
        assert!(library.resize_virtual_control(ORIENTATION_LANDSCAPE, buttons::A, 115));
        library.save_virtual_control_positions().unwrap();

        let first = identity(0x0004_0000_1234_5600);
        let second = identity(0x0004_0000_1234_5601);
        library.set_active_game_identity(Some(first));
        assert!(!library.game_has_control_override());
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 250, y: -150 }
        );
        assert_eq!(
            library
                .virtual_control_positions()
                .size_percent(ORIENTATION_LANDSCAPE, buttons::A),
            115
        );

        assert!(library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 500, 300));
        assert!(library.resize_virtual_control(ORIENTATION_LANDSCAPE, buttons::A, 135));
        assert!(library.game_has_control_override());
        library.save_virtual_control_positions().unwrap();

        let first_path = game_virtual_control_layout_path(&directory.join(".aceris"), first);
        assert!(first_path.is_file());
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 750, y: 150 }
        );

        library.set_active_game_identity(Some(second));
        assert!(!library.game_has_control_override());
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 250, y: -150 }
        );
        assert_eq!(
            library
                .virtual_control_positions()
                .size_percent(ORIENTATION_LANDSCAPE, buttons::A),
            115
        );

        let mut reopened = Library::new(Vec::new(), directory.display().to_string());
        reopened.set_active_game_identity(Some(first));
        assert!(reopened.game_has_control_override());
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 750, y: 150 }
        );
        assert_eq!(
            reopened
                .virtual_control_positions()
                .size_percent(ORIENTATION_LANDSCAPE, buttons::A),
            135
        );
        reopened.set_active_game_identity(Some(second));
        assert!(!reopened.game_has_control_override());
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 250, y: -150 }
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn editing_global_does_not_overwrite_an_active_game_override() {
        let directory = temporary_directory();
        let mut library = Library::new(Vec::new(), directory.display().to_string());
        let title = identity(0x0004_0000_1234_5602);
        library.set_active_game_identity(Some(title));
        library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 700, 0);
        library.save_virtual_control_positions().unwrap();
        let game_layout = *library.virtual_control_positions();

        assert!(library.toggle_virtual_control_edit_scope());
        assert_eq!(
            library.virtual_control_edit_scope(),
            VirtualControlEditScope::Global
        );
        library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 200, 100);
        library.save_virtual_control_positions().unwrap();

        assert_eq!(*library.virtual_control_positions(), game_layout);
        assert_eq!(
            library
                .virtual_control_editor_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 200, y: 100 }
        );

        let mut reopened = Library::new(Vec::new(), directory.display().to_string());
        reopened.set_active_game_identity(Some(title));
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 700, y: 0 }
        );
        reopened.set_active_game_identity(None);
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 200, y: 100 }
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn resetting_game_profile_removes_only_its_override_and_restores_inheritance() {
        let directory = temporary_directory();
        let mut library = Library::new(Vec::new(), directory.display().to_string());
        library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 300, 0);
        library.save_virtual_control_positions().unwrap();

        let first = identity(0x0004_0000_1234_5603);
        let second = identity(0x0004_0000_1234_5604);
        library.set_active_game_identity(Some(first));
        library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 700, 0);
        library.save_virtual_control_positions().unwrap();
        library.set_active_game_identity(Some(second));
        library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, -200, 0);
        library.save_virtual_control_positions().unwrap();

        library.set_active_game_identity(Some(first));
        assert!(library.reset_game_virtual_control_profile().unwrap());
        assert!(!library.game_has_control_override());
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 300, y: 0 }
        );
        assert!(!game_virtual_control_layout_path(&directory.join(".aceris"), first).exists());
        assert!(game_virtual_control_layout_path(&directory.join(".aceris"), second).exists());

        let mut reopened = Library::new(Vec::new(), directory.display().to_string());
        reopened.set_active_game_identity(Some(first));
        assert!(!reopened.game_has_control_override());
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 300, y: 0 }
        );
        reopened.set_active_game_identity(Some(second));
        assert!(reopened.game_has_control_override());
        assert_eq!(
            reopened
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            VirtualControlOffset { x: 100, y: 0 }
        );

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn unidentified_homebrew_stays_on_global_scope() {
        let directory = temporary_directory();
        let mut library = Library::new(Vec::new(), directory.display().to_string());
        assert!(!library.toggle_virtual_control_edit_scope());
        assert_eq!(
            library.virtual_control_edit_scope(),
            VirtualControlEditScope::Global
        );
        library.set_active_game_identity(None);
        assert!(library.move_virtual_control(ORIENTATION_LANDSCAPE, buttons::B, -400, 250));
        library.save_virtual_control_positions().unwrap();
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_LANDSCAPE, buttons::B),
            VirtualControlOffset { x: -400, y: 250 }
        );

        fs::remove_dir_all(directory).ok();
    }
    #[test]
    fn configured_settings_directory_supplies_global_control_profile() {
        let root = temporary_directory();
        let fallback = root.join("fallback");
        let settings = root.join("settings");
        let fallback_settings = fallback.join(".aceris");
        fs::create_dir_all(&fallback_settings).unwrap();
        fs::create_dir_all(&settings).unwrap();

        // Deliberately make the fallback and configured settings disagree. The
        // editor and newly launched titles must both use the configured profile.
        let mut fallback_layout = VirtualControlPositions::default();
        fallback_layout.adjust(ORIENTATION_PORTRAIT, buttons::A, 900, 0);
        fs::write(
            fallback_settings.join(VIRTUAL_CONTROL_LAYOUT_FILE),
            fallback_layout.to_config_string(),
        )
        .unwrap();

        let mut configured_layout = VirtualControlPositions::default();
        configured_layout.adjust(ORIENTATION_PORTRAIT, buttons::A, -250, 375);
        fs::write(
            settings.join(VIRTUAL_CONTROL_LAYOUT_FILE),
            configured_layout.to_config_string(),
        )
        .unwrap();

        let mut library = Library::from_scan_settings(&settings, &fallback);
        let expected = VirtualControlOffset { x: -250, y: 375 };
        assert_eq!(
            library
                .virtual_control_editor_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            expected
        );

        library.set_active_game_identity(Some(identity(0x0004_0000_1234_56ff)));
        assert_eq!(
            library
                .virtual_control_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            expected
        );
        assert!(library.toggle_virtual_control_edit_scope());
        assert_eq!(
            library
                .virtual_control_editor_positions()
                .offset(ORIENTATION_PORTRAIT, buttons::A),
            expected
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn untouched_game_editor_does_not_create_a_profile_override() {
        let directory = temporary_directory();
        let mut library = Library::new(Vec::new(), directory.display().to_string());
        let title = identity(0x0004_0000_1234_56fe);
        library.set_active_game_identity(Some(title));
        assert!(!library.game_has_control_override());

        // A tap/release in the editor can reach save without changing geometry.
        assert!(library.move_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 0, 0));
        assert!(library.resize_virtual_control(ORIENTATION_PORTRAIT, buttons::A, 100));
        library.save_virtual_control_positions().unwrap();

        assert!(!library.game_has_control_override());
        assert!(!game_virtual_control_layout_path(&directory.join(".aceris"), title).exists());
        fs::remove_dir_all(directory).ok();
    }
}
