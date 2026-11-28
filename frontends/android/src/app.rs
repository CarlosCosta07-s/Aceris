//! Platform-neutral application logic for the Android host (and any other
//! touch host): start-up animation, game library, running a game, virtual
//! gamepad and touch-screen mapping. It owns no Android API, so it is unit
//! tested on every platform; `native_activity` feeds it events and blits the
//! RGBA frames it produces.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aceris_core::input::{pad, pad_from_frontend_buttons, touch_from_surface_point};
use aceris_core::input_mapping::{GamepadAxisSettings, GamepadControl, GamepadMapping};
use aceris_core::video::frontend::{
    self, ORIENTATION_AUTO, ScreenLayout, ScreenLayoutMode, ScreenRect, compute_screen_layout,
    compute_screen_layout_for_mode,
};
use aceris_core::video::layout_preferences::{ScreenLayoutPreferences, ScreenLayoutProfile};
use aceris_core::video::library::{self, ClickResult, Library, VirtualControlEditScope};
use aceris_core::video::{
    DualScreenDisplay, ScreenSurface, intro,
    quick_menu::{self, PixelFormat as MenuPixelFormat},
};
use aceris_core::{
    System,
    loaded_image::{LoadedImageIdentity, load_executable_file_with_identity},
};

const GAMEPAD_HOME_NAV_UP: u8 = 1 << 0;
const GAMEPAD_HOME_NAV_DOWN: u8 = 1 << 1;
const GAMEPAD_HOME_NAV_LEFT: u8 = 1 << 2;
const GAMEPAD_HOME_NAV_RIGHT: u8 = 1 << 3;

/// Converts a physical stick position to one console-style navigation direction.
fn gamepad_home_navigation_mask(horizontal: f32, vertical: f32) -> u8 {
    const THRESHOLD: f32 = 0.60;

    let horizontal = if horizontal.is_finite() {
        horizontal.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let vertical = if vertical.is_finite() {
        vertical.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let horizontal_magnitude = horizontal.abs();
    let vertical_magnitude = vertical.abs();

    if horizontal_magnitude < THRESHOLD && vertical_magnitude < THRESHOLD {
        return 0;
    }

    if horizontal_magnitude > vertical_magnitude {
        if horizontal < 0.0 { GAMEPAD_HOME_NAV_LEFT } else { GAMEPAD_HOME_NAV_RIGHT }
    } else if vertical_magnitude >= THRESHOLD {
        if vertical > 0.0 { GAMEPAD_HOME_NAV_UP } else { GAMEPAD_HOME_NAV_DOWN }
    } else if horizontal < 0.0 {
        GAMEPAD_HOME_NAV_LEFT
    } else {
        GAMEPAD_HOME_NAV_RIGHT
    }
}

/// Which part of the application is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Intro {
        frame: u32,
    },
    Library,
    Game,
    GameMenu,
    GameMenuConfirm {
        action: quick_menu::ConfirmationAction,
    },
}

/// One active touch pointer in surface pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub id: i32,
    pub x: f32,
    pub y: f32,
}

/// The whole application state machine.
pub struct App {
    /// Folder scanned for games; save data lives in `<games_dir>/saves/<game file name>`.
    games_dir: PathBuf,
    screen: Screen,
    library: Library,
    system: Option<System>,
    current_game_path: Option<PathBuf>,
    active_image_identity: Option<LoadedImageIdentity>,
    quick_menu_selected: usize,
    confirmation_selected: usize,
    controls_editor: bool,
    gamepad_mapping_editor: bool,
    gamepad_axis_editor: bool,
    gamepad_mapping_capture: bool,
    gamepad_mapping_selected: usize,
    gamepad_mapping_game_scope: bool,
    diagnostics_visible: bool,
    performance_summary: Option<String>,
    quick_menu_notice: Option<&'static str>,
    control_drag: Option<(i32, u32)>,
    control_resize: Option<(i32, i32, u32, f32, u8, u32)>,
    layout_preferences: ScreenLayoutPreferences,
    menu_tap: Option<(i32, i32)>,
    menu_button_pointer: Option<i32>,
    width: u32,
    height: u32,
    pointers: BTreeMap<i32, (f32, f32)>,
    /// Buttons held through keyboard keys and the virtual touchscreen overlay.
    key_mask: u32,
    /// Digital physical gamepad controls currently held.
    gamepad_button_mask: u16,
    /// L2/R2 state derived from Android's analog trigger axes.
    gamepad_trigger_axis_mask: u16,
    /// Last analog direction used for edge-triggered Home navigation.
    gamepad_home_navigation: u8,
    /// Effective global/per-title physical gamepad mapping.
    gamepad_mapping: GamepadMapping,
    /// Effective global/per-title analog-stick settings.
    gamepad_axis_settings: GamepadAxisSettings,
    /// Library tap that was pressed and awaits release (taps act on release).
    library_tap: Option<(i32, i32)>,
    /// Interleaved stereo samples (32728 Hz) mixed since the last [`App::take_audio`].
    audio: Vec<i16>,
    /// Reused half-resolution intro frame to avoid one allocation per animation tick.
    intro_buffer: Vec<u8>,
    /// When the first opening frame was drawn; keeps the animation in real time.
    intro_started: Option<Instant>,
}

impl App {
    /// Creates the app showing the intro, with games listed from `games_dir`.
    pub fn new(games_dir: &Path) -> Self {
        let games_dir_owned = games_dir.to_path_buf();
        let settings_directory = games_dir.join(".aceris");
        let layout_preferences = ScreenLayoutPreferences::load(&settings_directory);
        let mut library = Library::from_scan_settings(&settings_directory, games_dir);
        library.set_touch_keyboard_enabled(true);
        let gamepad_axis_settings =
            GamepadAxisSettings::load_for_context(&settings_directory, None, None);
        Self {
            games_dir: games_dir_owned,
            screen: Screen::Intro { frame: 0 },
            library,
            system: None,
            current_game_path: None,
            active_image_identity: None,
            quick_menu_selected: 0,
            confirmation_selected: 0,
            controls_editor: false,
            gamepad_mapping_editor: false,
            gamepad_axis_editor: false,
            gamepad_mapping_capture: false,
            gamepad_mapping_selected: 0,
            gamepad_mapping_game_scope: false,
            diagnostics_visible: false,
            performance_summary: None,
            quick_menu_notice: None,
            control_drag: None,
            control_resize: None,
            layout_preferences,
            menu_tap: None,
            menu_button_pointer: None,
            width: 0,
            height: 0,
            pointers: BTreeMap::new(),
            key_mask: 0,
            gamepad_button_mask: 0,
            gamepad_trigger_axis_mask: 0,
            gamepad_home_navigation: 0,
            gamepad_mapping: GamepadMapping::default(),
            gamepad_axis_settings,
            library_tap: None,
            audio: Vec::new(),
            intro_buffer: Vec::new(),
            intro_started: None,
        }
    }

    /// Updates the system-level indicator for external media playback.
    ///
    /// The Android native activity calls this when media-session state changes.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn set_external_media_playing(&mut self, playing: bool) {
        self.library.set_external_media_playing(playing);
    }

    /// Updates the Android host capability shown in the media settings.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn set_external_media_detection_available(&mut self, available: bool) {
        self.library
            .set_external_media_detection_available(available);
    }

    /// Updates the latest host-side timing summary displayed by the optional diagnostics overlay.
    ///
    /// This is not guest FPS or emulated-clock time.
    pub fn set_performance_summary(&mut self, summary: String) {
        self.performance_summary = Some(summary);
    }

    /// Takes the audio the DSP mixed since the previous call (interleaved stereo
    /// `i16` at `System::audio_sample_rate()`).
    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.audio)
    }

    /// Reclaims the audio buffer after the host output has consumed its samples.
    ///
    /// Clearing retains the allocation so the next frame can mix into the same
    /// storage instead of allocating a new sample vector.
    pub fn recycle_audio_buffer(&mut self, mut samples: Vec<i16>) {
        samples.clear();
        self.audio = samples;
    }

    /// True while the quick menu is open and guest audio must not reach the host.
    pub fn is_game_menu_open(&self) -> bool {
        self.gamepad_mapping_editor
            || matches!(
                self.screen(),
                Screen::GameMenu | Screen::GameMenuConfirm { .. }
            )
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Identity of the exact executable currently loaded, when available.
    ///
    /// This is metadata for future save-state operations; it does not imply
    /// that full machine snapshots are currently supported.
    pub fn active_image_identity(&self) -> Option<LoadedImageIdentity> {
        self.active_image_identity
    }

    /// Sets the surface size in pixels; call on creation and resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Applies the primary physical gamepad stick to the emulated Circle Pad.
    ///
    /// Android reports positive Y when the physical stick moves down; the 3DS
    /// host input convention uses positive Y for up.
    pub fn set_gamepad_axes(&mut self, x: f32, y: f32) -> bool {
        if self.gamepad_mapping_editor || self.controls_editor {
            self.gamepad_home_navigation = 0;
            return false;
        }

        if matches!(self.screen, Screen::Library)
            && self.library.page() == library::LibraryPage::Home
        {
            let navigation = gamepad_home_navigation_mask(x, -y);
            let newly_pressed = navigation & !self.gamepad_home_navigation;
            self.gamepad_home_navigation = navigation;
            if newly_pressed & GAMEPAD_HOME_NAV_UP != 0 {
                self.library.move_selection_responsive(-1, self.width, self.height);
            }
            if newly_pressed & GAMEPAD_HOME_NAV_DOWN != 0 {
                self.library.move_selection_responsive(1, self.width, self.height);
            }
            if newly_pressed & GAMEPAD_HOME_NAV_LEFT != 0 {
                self.library.move_selection_horizontal_responsive(-1, self.width, self.height);
            }
            if newly_pressed & GAMEPAD_HOME_NAV_RIGHT != 0 {
                self.library.move_selection_horizontal_responsive(1, self.width, self.height);
            }
            return true;
        }

        self.gamepad_home_navigation = 0;
        let deadzone_percent = self.library.gamepad_deadzone_percent();
        let Some(system) = self.system.as_mut() else {
            return false;
        };
        let (circle_x, circle_y) =
            self.gamepad_axis_settings
                .apply_to_circle_pad(x, -y, deadzone_percent);
        system
            .input_mut()
            .set_circle_pad(i32::from(circle_x), i32::from(circle_y));
        true
    }

    /// Applies analog triggers through the currently selected physical-button mapping.
    pub fn set_gamepad_triggers(&mut self, left: f32, right: f32) -> bool {
        if matches!(self.screen, Screen::Game)
            && self.gamepad_mapping_editor
            && self.gamepad_mapping_capture
        {
            let left_pressed = gamepad_trigger_control_mask(left, 0.0) != 0;
            let right_pressed = gamepad_trigger_control_mask(0.0, right) != 0;
            if left_pressed || right_pressed {
                let control = if left_pressed && right_pressed && right > left {
                    GamepadControl::RightTrigger
                } else if left_pressed {
                    GamepadControl::LeftTrigger
                } else {
                    GamepadControl::RightTrigger
                };
                self.capture_gamepad_mapping_control(control);
            }
            return true;
        }
        if !matches!(self.screen, Screen::Game)
            || self.controls_editor
            || self.gamepad_mapping_editor
        {
            return false;
        }
        self.gamepad_trigger_axis_mask = gamepad_trigger_control_mask(left, right);
        self.apply_game_input();
        true
    }

    /// Handles a key event whose Android input source is a physical gamepad.
    /// Menu navigation retains standard Android button meanings; gameplay uses profiles.
    pub fn gamepad_key(&mut self, android_keycode: i32, pressed: bool) -> bool {
        if matches!(self.screen, Screen::Game)
            && self.gamepad_mapping_editor
            && self.gamepad_mapping_capture
        {
            if pressed {
                if let Some(control) = gamepad_control_from_android_keycode(android_keycode) {
                    self.capture_gamepad_mapping_control(control);
                }
            }
            return true;
        }
        if !matches!(self.screen, Screen::Game)
            || self.controls_editor
            || self.gamepad_mapping_editor
        {
            return self.key(android_keycode, pressed);
        }
        let Some(control) = gamepad_control_from_android_keycode(android_keycode) else {
            return false;
        };
        if pressed {
            self.gamepad_button_mask |= control.bit();
        } else {
            self.gamepad_button_mask &= !control.bit();
        }
        self.apply_game_input();
        true
    }

    /// Replaces the set of touching pointers (multi-touch aware).
    pub fn set_pointers(&mut self, pointers: &[Pointer]) {
        let previous = std::mem::take(&mut self.pointers);
        self.pointers = pointers.iter().map(|p| (p.id, (p.x, p.y))).collect();
        match self.screen {
            Screen::Intro { .. } => {
                // Any touch skips the intro.
                if !self.pointers.is_empty() && previous.is_empty() {
                    self.screen = Screen::Library;
                }
            }
            Screen::Library => self.update_library_touch(&previous),
            Screen::Game => self.update_game_touch(&previous),
            Screen::GameMenu => self.update_game_menu_touch(&previous),
            Screen::GameMenuConfirm { action } => {
                self.update_game_menu_confirmation_touch(&previous, action)
            }
        }
    }

    /// Handles a hardware key; returns true when the key was consumed.
    pub fn key(&mut self, android_keycode: i32, pressed: bool) -> bool {
        if android_keycode == AKEYCODE_BACK {
            if pressed {
                match self.screen {
                    Screen::Library if self.library.search_is_focused() => {
                        self.library.cancel_text_entry();
                    }
                    Screen::Game if self.gamepad_mapping_editor && self.gamepad_mapping_capture => {
                        self.cancel_gamepad_mapping_capture();
                    }
                    Screen::Game if self.gamepad_mapping_editor && self.gamepad_axis_editor => {
                        self.gamepad_axis_editor = false;
                    }
                    Screen::Game if self.gamepad_mapping_editor => {
                        self.finish_gamepad_mapping_editor()
                    }
                    Screen::Game if self.controls_editor => self.finish_control_editor(),
                    Screen::Game => self.open_game_menu(),
                    Screen::GameMenu => self.resume_game(),
                    Screen::GameMenuConfirm { .. } => self.cancel_game_menu_confirmation(),
                    // Back during the opening skips to the library instead of
                    // letting NativeActivity finish (close) the app.
                    Screen::Intro { .. } => self.screen = Screen::Library,
                    _ => {}
                }
            }
            return true;
        }
        if pressed && matches!(self.screen, Screen::Library) && self.library.search_is_focused() {
            match android_keycode {
                AKEYCODE_DEL => {
                    self.library.backspace_search();
                    return true;
                }
                AKEYCODE_ENTER => {
                    self.library.finish_search();
                    return true;
                }
                _ => {}
            }
        }
        let Some(bit) = pad_from_android_keycode(android_keycode) else {
            return false;
        };
        // The host key map returns frontend button bits for key_mask and the
        // on-screen overlay. Menu navigation uses the guest's hardware-order
        // bits, so convert before comparing against input::pad constants.
        let pad_bit = pad_from_frontend_buttons(bit);
        match self.screen {
            Screen::Game if self.gamepad_mapping_editor && self.gamepad_axis_editor && pressed => {
                match pad_bit {
                    pad::UP => self.toggle_gamepad_axis_invert_x(),
                    pad::DOWN => self.toggle_gamepad_axis_invert_y(),
                    pad::LEFT => self.adjust_gamepad_axis_sensitivity(-10),
                    pad::RIGHT | pad::A | pad::START => self.adjust_gamepad_axis_sensitivity(10),
                    pad::L => self.adjust_gamepad_axis_deadzone(-1),
                    pad::R => self.adjust_gamepad_axis_deadzone(1),
                    pad::X => {
                        self.toggle_gamepad_mapping_scope();
                        self.library.set_status("ESCOPO DO PERFIL ALTERADO");
                    }
                    pad::Y => self.reset_gamepad_axis_settings_editor(),
                    pad::B => self.gamepad_axis_editor = false,
                    _ => {}
                }
            }
            Screen::Game if self.gamepad_mapping_editor && pressed => match pad_bit {
                pad::UP => {
                    self.gamepad_mapping_selected = self.gamepad_mapping_selected.saturating_sub(1);
                }
                pad::DOWN => {
                    self.gamepad_mapping_selected =
                        (self.gamepad_mapping_selected + 1).min(GamepadControl::COUNT - 1);
                }
                pad::LEFT => self.cycle_gamepad_mapping_target(-1),
                pad::RIGHT | pad::A | pad::START => self.cycle_gamepad_mapping_target(1),
                pad::X => self.toggle_gamepad_mapping_scope(),
                pad::Y => self.start_gamepad_mapping_capture(),
                pad::B => self.finish_gamepad_mapping_editor(),
                _ => {}
            },
            Screen::Game if self.gamepad_mapping_editor => {}
            Screen::Game if self.controls_editor && pressed && pad_bit == pad::B => {
                self.finish_control_editor();
            }
            Screen::Game => {
                if pressed {
                    self.key_mask |= bit;
                } else {
                    self.key_mask &= !bit;
                }
                self.apply_game_input();
            }
            Screen::GameMenu if pressed => match pad_bit {
                pad::UP => {
                    self.quick_menu_selected = self.quick_menu_selected.saturating_sub(1);
                }
                pad::DOWN => {
                    self.quick_menu_selected =
                        (self.quick_menu_selected + 1).min(quick_menu::ACTION_COUNT - 1);
                }
                pad::A | pad::START => self.activate_game_menu_action(),
                pad::B => self.resume_game(),
                _ => {}
            },
            Screen::GameMenuConfirm { action } if pressed => match pad_bit {
                pad::UP => {
                    self.confirmation_selected = self.confirmation_selected.saturating_sub(1);
                }
                pad::DOWN => {
                    self.confirmation_selected = (self.confirmation_selected + 1).min(1);
                }
                pad::A | pad::START => self.activate_game_menu_confirmation(action),
                pad::B => self.cancel_game_menu_confirmation(),
                _ => {}
            },
            Screen::GameMenuConfirm { .. } => {}
            Screen::GameMenu => {}
            Screen::Library if pressed => match pad_bit {
                pad::UP => self
                    .library
                    .move_selection_responsive(-1, self.width, self.height),
                pad::DOWN => self
                    .library
                    .move_selection_responsive(1, self.width, self.height),
                pad::LEFT => {
                    self.library
                        .move_selection_horizontal_responsive(-1, self.width, self.height)
                }
                pad::RIGHT => {
                    self.library
                        .move_selection_horizontal_responsive(1, self.width, self.height)
                }
                pad::A | pad::START => {
                    if self.library.page() == library::LibraryPage::Home {
                        if self
                            .library
                            .activate_home_selection(self.width, self.height)
                            == ClickResult::Activated
                        {
                            self.launch_selected();
                        }
                    } else {
                        self.launch_selected();
                    }
                }
                _ => {}
            },
            Screen::Intro { .. } if pressed => self.screen = Screen::Library,
            _ => {}
        }
        if matches!(self.screen, Screen::Library) && self.library.take_folder_picker_request() {
            // SAF does not have a result callback in this Rust-only NativeActivity,
            // so keep the path-entry fallback available for the import action.
            self.library.begin_add_folder_path();
        }
        true
    }

    /// Sends a printable character from a physical/system keyboard to the focused search.
    pub fn text_input(&mut self, character: char) -> bool {
        matches!(self.screen, Screen::Library) && self.library.input_search_char(character)
    }

    /// Advances one 60 Hz tick and returns an owned RGBA8 frame.
    ///
    /// Hosts that present every frame should use step_into to retain the
    /// output allocation between ticks.
    pub fn step(&mut self) -> Vec<u8> {
        let mut frame = Vec::new();
        self.step_into(&mut frame);
        frame
    }

    /// Advances one tick and writes into a caller-owned RGBA8 frame buffer.
    ///
    /// The buffer keeps its capacity between calls and is cleared when the
    /// surface has no usable dimensions.
    pub fn step_into(&mut self, output: &mut Vec<u8>) {
        if self.width == 0 || self.height == 0 {
            output.clear();
            return;
        }
        match self.screen {
            Screen::Intro { frame } => {
                // Advance by at least one frame per tick, and further when the
                // device renders slower than 60 Hz, so the opening keeps its
                // real duration instead of crawling on slow phones.
                let started = *self.intro_started.get_or_insert_with(Instant::now);
                let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                let next = (frame + 1).max(intro::frame_for_elapsed_micros(elapsed));
                self.screen = if next >= intro::FRAME_COUNT {
                    Screen::Library
                } else {
                    Screen::Intro { frame: next }
                };
                // The procedural opening is rendered on a smaller sample grid
                // with a fixed pixel budget, then scaled to the whole surface.
                // The budget is split by area (not by a landscape-only box), so
                // a portrait phone no longer gets a blurry ~160x360 image.
                let pixels = self.width as f32 * self.height as f32;
                let intro_scale = (pixels / INTRO_PIXEL_BUDGET).sqrt().max(1.0);
                let intro_width = ((self.width as f32 / intro_scale).round() as u32).max(1);
                let intro_height = ((self.height as f32 / intro_scale).round() as u32).max(1);
                let rendered = intro::render_frame_into(
                    frame,
                    intro_width as usize,
                    intro_height as usize,
                    &mut self.intro_buffer,
                );
                if rendered && reset_black_frame(output, self.width, self.height) {
                    blit_scaled(
                        output,
                        self.width,
                        &self.intro_buffer,
                        intro_width,
                        intro_height,
                        ScreenRect {
                            x: 0,
                            y: 0,
                            width: self.width,
                            height: self.height,
                        },
                    );
                }
            }
            Screen::Library => {
                let _ = self
                    .library
                    .render_responsive_into(self.width, self.height, output);
            }
            Screen::Game => self.step_game_into(output),
            Screen::GameMenu => self.step_game_menu_into(output),
            Screen::GameMenuConfirm { action } => {
                self.step_game_menu_confirmation_into(output, action)
            }
        }
    }

    fn step_game_menu_into(&mut self, output: &mut Vec<u8>) {
        let Some(system) = self.system.as_ref() else {
            self.screen = Screen::Library;
            let _ = self
                .library
                .render_responsive_into(self.width, self.height, output);
            return;
        };
        let layout = self.game_layout();
        if !compose_game_into(system.display(), &layout, self.width, self.height, output) {
            return;
        }
        let screen_override = self.layout_preferences.game_has_override_for_surface(
            system.game_identity(),
            self.width,
            self.height,
        );
        let control_override = self.library.game_has_control_override();
        let inherited_profile_label = match (screen_override, control_override) {
            (true, true) => "TELAS: JOGO | CONTROLES: JOGO",
            (true, false) => "TELAS: JOGO | CONTROLES: GLOBAL",
            (false, true) => "TELAS: GLOBAL | CONTROLES: JOGO",
            (false, false) => "TELAS: GLOBAL | CONTROLES: GLOBAL",
        };
        let profile_label = self.quick_menu_notice.unwrap_or(inherited_profile_label);
        let restore_label =
            if self.library.virtual_control_edit_scope() == VirtualControlEditScope::Game {
                "RESTAURAR PERFIL"
            } else {
                "RESTAURAR CONTROLES"
            };
        let scope_label = match (
            self.library.active_game_identity().is_some(),
            self.library.virtual_control_edit_scope(),
        ) {
            (true, VirtualControlEditScope::Global) => "PERFIL DESTE JOGO",
            (true, VirtualControlEditScope::Game) => "PADRÃO GLOBAL",
            (false, _) => "GLOBAL (SEM ID)",
        };
        let _ = quick_menu::render_overlay_with_control_profiles(
            output,
            self.width,
            self.height,
            MenuPixelFormat::Rgba8,
            self.quick_menu_selected,
            profile_label,
            restore_label,
            scope_label,
        );
    }

    fn step_game_menu_confirmation_into(
        &mut self,
        output: &mut Vec<u8>,
        action: quick_menu::ConfirmationAction,
    ) {
        let Some(system) = self.system.as_ref() else {
            self.screen = Screen::Library;
            let _ = self
                .library
                .render_responsive_into(self.width, self.height, output);
            return;
        };
        let layout = self.game_layout();
        if !compose_game_into(system.display(), &layout, self.width, self.height, output) {
            return;
        }
        let _ = quick_menu::render_confirmation_overlay(
            output,
            self.width,
            self.height,
            MenuPixelFormat::Rgba8,
            action,
            self.confirmation_selected,
        );
    }

    fn step_game_into(&mut self, output: &mut Vec<u8>) {
        if self.gamepad_mapping_editor {
            let Some(system) = self.system.as_ref() else {
                self.screen = Screen::Library;
                let _ = self
                    .library
                    .render_responsive_into(self.width, self.height, output);
                return;
            };
            if compose_game_into(
                system.display(),
                &self.game_layout(),
                self.width,
                self.height,
                output,
            ) {
                let scope_label = if self.gamepad_mapping_game_scope {
                    "ESCOPO: JOGO"
                } else {
                    "ESCOPO: GLOBAL"
                };
                if self.gamepad_axis_editor {
                    let _ = quick_menu::render_gamepad_axis_editor(
                        output,
                        self.width,
                        self.height,
                        MenuPixelFormat::Rgba8,
                        &self.gamepad_axis_settings,
                        self.library.gamepad_deadzone_percent(),
                        scope_label,
                    );
                } else {
                    let _ = quick_menu::render_gamepad_mapping_editor(
                        output,
                        self.width,
                        self.height,
                        MenuPixelFormat::Rgba8,
                        &self.gamepad_mapping,
                        self.gamepad_mapping_selected,
                        scope_label,
                        self.gamepad_mapping_capture,
                    );
                }
            }
            return;
        }
        if self.controls_editor {
            let Some(system) = self.system.as_ref() else {
                self.screen = Screen::Library;
                let _ = self
                    .library
                    .render_responsive_into(self.width, self.height, output);
                return;
            };
            if compose_game_into(
                system.display(),
                &self.game_layout(),
                self.width,
                self.height,
                output,
            ) {
                self.render_virtual_controls_overlay(output, 0);
                if self.control_resize.is_none() {
                    if let Some((_, button)) = self.control_drag {
                        if let Some(center) = frontend::virtual_control_center_with_positions(
                            self.width,
                            self.height,
                            ORIENTATION_AUTO,
                            self.library.virtual_controls_size_percent(),
                            self.library.virtual_controls_preset(),
                            self.displayed_virtual_control_positions(),
                            button,
                        ) {
                            let _ = quick_menu::render_control_editor_guides(
                                output,
                                self.width,
                                self.height,
                                MenuPixelFormat::Rgba8,
                                center,
                            );
                        }
                    }
                }
                let _ = quick_menu::render_control_editor_hint(
                    output,
                    self.width,
                    self.height,
                    MenuPixelFormat::Rgba8,
                );
            }
            return;
        }
        let Some(system) = self.system.as_mut() else {
            self.screen = Screen::Library;
            let _ = self
                .library
                .render_responsive_into(self.width, self.height, output);
            return;
        };
        if let Err(error) = system.run_frame() {
            let message = format!("Execução interrompida: {error}");
            let diagnostic = self.game_diagnostic();
            self.leave_game();
            let message = diagnostic
                .map(|detail| format!("{message} — HLE: {detail}"))
                .unwrap_or(message);
            self.library.set_status(message);
            let _ = self
                .library
                .render_responsive_into(self.width, self.height, output);
            return;
        }
        system.take_audio_samples(&mut self.audio);
        let system = self.system.as_ref().expect("system exists while in game");
        let layout = self.game_layout();
        if !compose_game_into(system.display(), &layout, self.width, self.height, output) {
            return;
        }
        let pressed_mask = self.held_mask();
        self.render_virtual_controls_overlay(output, pressed_mask);
        if self.diagnostics_visible {
            if let Some(system) = self.system.as_ref() {
                let unhandled = system.unhandled_requests();
                let _ = quick_menu::render_diagnostics_overlay(
                    output,
                    self.width,
                    self.height,
                    MenuPixelFormat::Rgba8,
                    system.cpu().pc(),
                    unhandled.len(),
                    unhandled.first().map(String::as_str),
                    self.performance_summary.as_deref(),
                );
            }
        }
        let _ =
            quick_menu::render_menu_button(output, self.width, self.height, MenuPixelFormat::Rgba8);
    }

    fn held_mask(&self) -> u32 {
        let touch_mask = self
            .pointers
            .values()
            .fold(0, |mask, &(x, y)| mask | self.hit_virtual_button(x, y));
        self.key_mask | touch_mask
    }

    fn displayed_virtual_control_positions(&self) -> &frontend::VirtualControlPositions {
        if self.controls_editor {
            self.library.virtual_control_editor_positions()
        } else {
            self.library.virtual_control_positions()
        }
    }

    fn hit_virtual_button(&self, x: f32, y: f32) -> u32 {
        if !self.library.virtual_controls_visible() && !self.controls_editor {
            return 0;
        }
        frontend::hit_test_virtual_control_with_positions(
            self.width,
            self.height,
            x as i32,
            y as i32,
            ORIENTATION_AUTO,
            self.library.virtual_controls_size_percent(),
            self.library.virtual_controls_preset(),
            self.displayed_virtual_control_positions(),
        )
    }

    fn render_virtual_controls_overlay(&self, frame: &mut [u8], pressed_mask: u32) {
        if self.library.virtual_controls_visible() || self.controls_editor {
            frontend::blend_virtual_pad_overlay_into_with_positions(
                frame,
                self.width,
                self.height,
                ORIENTATION_AUTO,
                pressed_mask,
                self.library.virtual_controls_opacity(),
                self.library.virtual_controls_size_percent(),
                self.library.virtual_controls_preset(),
                self.displayed_virtual_control_positions(),
            );
        }
    }

    fn active_layout_profile(&self) -> ScreenLayoutProfile {
        let identity = self.system.as_ref().and_then(System::game_identity);
        self.layout_preferences
            .profile_for_game(identity, self.width, self.height)
    }

    fn game_layout(&self) -> ScreenLayout {
        let profile = self.active_layout_profile();
        compute_screen_layout_for_mode(self.width, self.height, profile.mode, profile.swap_screens)
    }

    fn save_layout_preferences(&self) {
        let settings_directory = self.games_dir.join(".aceris");
        if let Err(error) = self.layout_preferences.save(&settings_directory) {
            eprintln!("could not save screen layout preferences: {error}");
        }
    }

    fn cycle_screen_layout(&mut self) {
        let mut profile = self.active_layout_profile();
        profile.mode = match profile.mode {
            ScreenLayoutMode::Auto if self.width >= self.height => ScreenLayoutMode::Vertical,
            ScreenLayoutMode::Auto => ScreenLayoutMode::SideBySide,
            mode => mode.next(),
        };
        let identity = self.system.as_ref().and_then(System::game_identity);
        self.layout_preferences
            .set_profile_for_game(identity, self.width, self.height, profile);
        self.save_layout_preferences();
    }

    fn toggle_screen_swap(&mut self) {
        let mut profile = self.active_layout_profile();
        profile.swap_screens = !profile.swap_screens;
        let identity = self.system.as_ref().and_then(System::game_identity);
        self.layout_preferences
            .set_profile_for_game(identity, self.width, self.height, profile);
        self.save_layout_preferences();
    }

    fn reset_game_layout(&mut self) {
        if let Some(identity) = self.system.as_ref().and_then(System::game_identity) {
            self.layout_preferences.reset_game_overrides(identity);
            self.save_layout_preferences();
        }
    }

    fn reset_virtual_controls_layout(&mut self) {
        if self.library.virtual_control_edit_scope() == VirtualControlEditScope::Game {
            match self.library.reset_game_virtual_control_profile() {
                Ok(_) => {}
                Err(error) => self.library.set_status(format!(
                    "NÃO FOI POSSÍVEL RESTAURAR O PERFIL DO JOGO: {error}"
                )),
            }
            return;
        }

        let orientation = self.virtual_control_orientation();
        match self.library.reset_virtual_control_positions(orientation) {
            Ok(()) => self.library.set_status(match orientation {
                frontend::ORIENTATION_LANDSCAPE => "CONTROLES RESTAURADOS PARA PAISAGEM",
                _ => "CONTROLES RESTAURADOS PARA RETRATO",
            }),
            Err(error) => self
                .library
                .set_status(format!("NÃO FOI POSSÍVEL RESTAURAR OS CONTROLES: {error}")),
        }
    }

    fn toggle_virtual_control_edit_scope(&mut self) {
        self.library.toggle_virtual_control_edit_scope();
    }

    fn apply_game_input(&mut self) {
        let layout = self.game_layout();
        let mask = self.held_mask();
        let touch = self.pointers.iter().find_map(|(id, &(x, y))| {
            // Menu-shortcut and virtual-control touches must not reach the guest stylus.
            if self.menu_button_pointer == Some(*id) || self.hit_virtual_button(x, y) != 0 {
                return None;
            }
            touch_from_surface_point(&layout, x as i32, y as i32)
        });
        let Some(system) = self.system.as_mut() else {
            return;
        };
        let physical_mask = self
            .gamepad_mapping
            .mask_for_held_controls(self.gamepad_button_mask | self.gamepad_trigger_axis_mask);
        let input = system.input_mut();
        input.set_buttons(aceris_core::input::pad_from_frontend_buttons(mask) | physical_mask);
        match touch {
            Some((x, y)) => input.touch(i32::from(x), i32::from(y)),
            None => input.touch_end(),
        }
    }

    fn update_game_touch(&mut self, previous: &BTreeMap<i32, (f32, f32)>) {
        if self.gamepad_mapping_editor {
            self.update_gamepad_mapping_editor_touch(previous);
            return;
        }
        if self.controls_editor {
            self.update_control_editor_touch(previous);
            return;
        }
        if self.menu_button_pointer.is_none() {
            for (&id, &(x, y)) in &self.pointers {
                if !previous.contains_key(&id)
                    && quick_menu::hit_test_menu_button(
                        self.width,
                        self.height,
                        x.round() as i32,
                        y.round() as i32,
                    )
                {
                    self.menu_button_pointer = Some(id);
                    break;
                }
            }
        }
        if let Some(pointer_id) = self.menu_button_pointer {
            if !self.pointers.contains_key(&pointer_id) {
                let opened = previous.get(&pointer_id).is_some_and(|&(x, y)| {
                    quick_menu::hit_test_menu_button(
                        self.width,
                        self.height,
                        x.round() as i32,
                        y.round() as i32,
                    )
                });
                self.menu_button_pointer = None;
                if opened {
                    self.open_game_menu();
                    return;
                }
            }
        }
        self.apply_game_input();
    }

    fn update_control_editor_touch(&mut self, previous: &BTreeMap<i32, (f32, f32)>) {
        if self.control_drag.is_none() {
            for (&id, &(x, y)) in &self.pointers {
                if previous.contains_key(&id) {
                    continue;
                }
                let button = self.hit_virtual_button(x, y);
                if button != 0 {
                    self.control_drag = Some((id, button));
                    break;
                }
            }
        }

        let Some((pointer_id, button)) = self.control_drag else {
            return;
        };
        let orientation = self.virtual_control_orientation();

        // A second finger on the selected button switches from moving to pinch-resize.
        if self.control_resize.is_none() {
            if let (Some(&(first_x, first_y)), Some((&second_id, &(second_x, second_y)))) = (
                self.pointers.get(&pointer_id),
                self.pointers.iter().find(|(id, _)| **id != pointer_id),
            ) {
                if self.hit_virtual_button(second_x, second_y) == button {
                    let start_distance = (second_x - first_x).hypot(second_y - first_y);
                    if start_distance >= 6.0 {
                        let initial_size = self
                            .displayed_virtual_control_positions()
                            .size_percent(orientation, button);
                        self.control_resize = Some((
                            pointer_id,
                            second_id,
                            button,
                            start_distance,
                            initial_size,
                            orientation,
                        ));
                    }
                }
            }
        }

        if let Some((
            first_id,
            second_id,
            resize_button,
            start_distance,
            initial_size,
            resize_orientation,
        )) = self.control_resize
        {
            match (self.pointers.get(&first_id), self.pointers.get(&second_id)) {
                (Some(&(first_x, first_y)), Some(&(second_x, second_y))) => {
                    let current_distance = (second_x - first_x).hypot(second_y - first_y);
                    let scaled =
                        (f32::from(initial_size) * current_distance / start_distance).round();
                    let size_percent = (scaled as i16).clamp(60, 150) as u8;
                    self.library.resize_virtual_control(
                        resize_orientation,
                        resize_button,
                        size_percent,
                    );
                    return;
                }
                _ => {
                    self.control_resize = None;
                    self.control_drag = None;
                    if let Err(error) = self.library.save_virtual_control_positions() {
                        self.library
                            .set_status(format!("NÃO FOI POSSÍVEL SALVAR O LAYOUT: {error}"));
                    }
                    return;
                }
            }
        }

        match (previous.get(&pointer_id), self.pointers.get(&pointer_id)) {
            (Some(&(old_x, old_y)), Some(&(new_x, new_y))) => {
                let delta_x = new_x - old_x;
                let delta_y = new_y - old_y;
                if delta_x != 0.0 || delta_y != 0.0 {
                    let width = self.width.max(1) as f32;
                    let height = self.height.max(1) as f32;
                    let normalized_x = (delta_x * 10_000.0 / width).round() as i16;
                    let normalized_y = (delta_y * 10_000.0 / height).round() as i16;
                    self.library.move_virtual_control(
                        orientation,
                        button,
                        normalized_x,
                        normalized_y,
                    );
                }
            }
            (Some(_), None) => {
                self.control_drag = None;
                if let Err(error) = self.library.save_virtual_control_positions() {
                    self.library
                        .set_status(format!("NÃO FOI POSSÍVEL SALVAR AS POSIÇÕES: {error}"));
                }
            }
            _ => {}
        }
    }

    fn virtual_control_orientation(&self) -> u32 {
        if self.width >= self.height {
            frontend::ORIENTATION_LANDSCAPE
        } else {
            frontend::ORIENTATION_PORTRAIT
        }
    }

    fn begin_control_editor(&mut self) {
        self.gamepad_mapping_editor = false;
        self.controls_editor = true;
        self.control_drag = None;
        self.control_resize = None;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
        self.screen = Screen::Game;
    }

    fn finish_control_editor(&mut self) {
        if self.control_drag.take().is_some() || self.control_resize.take().is_some() {
            if let Err(error) = self.library.save_virtual_control_positions() {
                self.library
                    .set_status(format!("NÃO FOI POSSÍVEL SALVAR O LAYOUT: {error}"));
            }
        }
        self.control_resize = None;
        self.controls_editor = false;
        self.pointers.clear();
        self.key_mask = 0;
        self.gamepad_button_mask = 0;
        self.gamepad_trigger_axis_mask = 0;
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
    }

    fn begin_gamepad_mapping_editor(&mut self) {
        self.controls_editor = false;
        self.control_drag = None;
        self.control_resize = None;
        self.gamepad_mapping_capture = false;
        self.gamepad_axis_editor = false;
        self.gamepad_mapping_selected = 0;
        self.gamepad_mapping_game_scope = self
            .system
            .as_ref()
            .and_then(System::game_identity)
            .is_some();
        self.load_gamepad_mapping_editor_scope();
        self.gamepad_mapping_editor = true;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        self.key_mask = 0;
        self.gamepad_button_mask = 0;
        self.gamepad_trigger_axis_mask = 0;
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
        self.screen = Screen::Game;
    }

    fn start_gamepad_mapping_capture(&mut self) {
        let selected_control = GamepadControl::ALL[self.gamepad_mapping_selected];
        if self.gamepad_mapping.target(selected_control).is_none() {
            self.library
                .set_status("SELECIONE UM DESTINO 3DS ANTES DE CAPTURAR");
            return;
        }
        self.gamepad_mapping_capture = true;
        self.library.set_status("PRESSIONE UM BOTAO NO CONTROLE");
    }

    fn cancel_gamepad_mapping_capture(&mut self) {
        self.gamepad_mapping_capture = false;
        self.library.set_status("CAPTURA CANCELADA");
    }

    fn capture_gamepad_mapping_control(&mut self, captured_control: GamepadControl) {
        let selected_control = GamepadControl::ALL[self.gamepad_mapping_selected];
        let Some(target) = self.gamepad_mapping.target(selected_control) else {
            self.cancel_gamepad_mapping_capture();
            self.library
                .set_status("SELECIONE UM DESTINO 3DS ANTES DE CAPTURAR");
            return;
        };
        self.gamepad_mapping
            .assign_target_uniquely(captured_control, Some(target));
        self.gamepad_mapping_selected = captured_control as usize;
        self.gamepad_mapping_capture = false;
        self.save_gamepad_mapping_editor();
    }

    fn load_gamepad_mapping_editor_scope(&mut self) {
        let settings_directory = self.games_dir.join(".aceris");
        self.gamepad_mapping = if self.gamepad_mapping_game_scope {
            self.system
                .as_ref()
                .and_then(System::game_identity)
                .map(|identity| {
                    GamepadMapping::load_for_game_or_global(&settings_directory, identity)
                })
                .unwrap_or_else(|| GamepadMapping::load_global(&settings_directory))
        } else {
            GamepadMapping::load_global(&settings_directory)
        };
        self.load_gamepad_axis_settings_editor_scope();
    }

    fn load_gamepad_axis_settings_editor_scope(&mut self) {
        let settings_directory = self.games_dir.join(".aceris");
        self.gamepad_axis_settings = if self.gamepad_mapping_game_scope {
            self.system
                .as_ref()
                .and_then(System::game_identity)
                .map(|identity| {
                    GamepadAxisSettings::load_for_game_or_global(&settings_directory, identity)
                })
                .unwrap_or_else(|| GamepadAxisSettings::load_global(&settings_directory))
        } else {
            GamepadAxisSettings::load_global(&settings_directory)
        };
    }

    fn save_gamepad_axis_settings_editor(&mut self, settings: GamepadAxisSettings) {
        let settings_directory = self.games_dir.join(".aceris");
        let result = if self.gamepad_mapping_game_scope {
            self.system
                .as_ref()
                .and_then(System::game_identity)
                .map(|identity| settings.save_for_game(&settings_directory, identity))
                .unwrap_or_else(|| settings.save_global(&settings_directory))
        } else {
            settings.save_global(&settings_directory)
        };
        if let Err(error) = result {
            self.library
                .set_status(format!("NÃO FOI POSSÍVEL SALVAR AJUSTES: {error}"));
        } else {
            self.gamepad_axis_settings = settings;
            self.library.set_status(if self.gamepad_mapping_game_scope {
                "AJUSTES DO ANALÓGICO SALVOS PARA O JOGO"
            } else {
                "AJUSTES GLOBAIS DO ANALÓGICO SALVOS"
            });
        }
    }

    fn toggle_gamepad_axis_invert_x(&mut self) {
        let mut settings = self.gamepad_axis_settings;
        settings.set_invert_x(!settings.invert_x());
        self.save_gamepad_axis_settings_editor(settings);
    }

    fn toggle_gamepad_axis_invert_y(&mut self) {
        let mut settings = self.gamepad_axis_settings;
        settings.set_invert_y(!settings.invert_y());
        self.save_gamepad_axis_settings_editor(settings);
    }

    fn adjust_gamepad_axis_sensitivity(&mut self, delta: i8) {
        let mut settings = self.gamepad_axis_settings;
        let next = if delta < 0 {
            settings
                .sensitivity_percent()
                .saturating_sub(delta.unsigned_abs())
        } else {
            settings.sensitivity_percent().saturating_add(delta as u8)
        };
        settings.set_sensitivity_percent(next);
        self.save_gamepad_axis_settings_editor(settings);
    }

    fn reset_gamepad_axis_settings_editor(&mut self) {
        self.save_gamepad_axis_settings_editor(GamepadAxisSettings::default());
    }

    fn adjust_gamepad_axis_deadzone(&mut self, direction: i8) {
        let mut settings = self.gamepad_axis_settings;
        settings.adjust_deadzone_override(self.library.gamepad_deadzone_percent(), direction);
        self.save_gamepad_axis_settings_editor(settings);
    }

    fn inherit_gamepad_axis_deadzone(&mut self) {
        let mut settings = self.gamepad_axis_settings;
        settings.set_deadzone_override_percent(None);
        self.save_gamepad_axis_settings_editor(settings);
    }

    fn save_gamepad_mapping_editor(&mut self) {
        let settings_directory = self.games_dir.join(".aceris");
        let result = if self.gamepad_mapping_game_scope {
            self.system
                .as_ref()
                .and_then(System::game_identity)
                .map(|identity| {
                    self.gamepad_mapping
                        .save_for_game(&settings_directory, identity)
                })
                .unwrap_or_else(|| self.gamepad_mapping.save_global(&settings_directory))
        } else {
            self.gamepad_mapping.save_global(&settings_directory)
        };
        if let Err(error) = result {
            self.library
                .set_status(format!("NÃO FOI POSSÍVEL SALVAR O MAPEAMENTO: {error}"));
        } else {
            self.library.set_status(if self.gamepad_mapping_game_scope {
                "MAPEAMENTO DO JOGO SALVO"
            } else {
                "MAPEAMENTO GLOBAL SALVO"
            });
        }
    }

    fn cycle_gamepad_mapping_target(&mut self, direction: i8) {
        let Some(control) = GamepadControl::ALL
            .get(self.gamepad_mapping_selected)
            .copied()
        else {
            return;
        };
        self.gamepad_mapping.cycle_target(control, direction);
        self.save_gamepad_mapping_editor();
    }

    fn toggle_gamepad_mapping_scope(&mut self) {
        let has_game_identity = self
            .system
            .as_ref()
            .and_then(System::game_identity)
            .is_some();
        if !has_game_identity {
            self.gamepad_mapping_game_scope = false;
            self.load_gamepad_mapping_editor_scope();
            return;
        }
        self.gamepad_mapping_game_scope = !self.gamepad_mapping_game_scope;
        self.load_gamepad_mapping_editor_scope();
    }

    fn reset_gamepad_mapping_editor(&mut self) {
        self.gamepad_mapping = GamepadMapping::default();
        self.save_gamepad_mapping_editor();
    }

    fn finish_gamepad_mapping_editor(&mut self) {
        self.gamepad_mapping_editor = false;
        self.gamepad_axis_editor = false;
        self.gamepad_mapping_capture = false;
        self.gamepad_mapping = GamepadMapping::load_for_context(
            &self.games_dir.join(".aceris"),
            None,
            self.system.as_ref().and_then(System::game_identity),
        );
        self.gamepad_axis_settings = GamepadAxisSettings::load_for_context(
            &self.games_dir.join(".aceris"),
            None,
            self.system.as_ref().and_then(System::game_identity),
        );
        self.pointers.clear();
        self.menu_tap = None;
        self.key_mask = 0;
        self.gamepad_button_mask = 0;
        self.gamepad_trigger_axis_mask = 0;
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
    }

    fn update_gamepad_mapping_editor_touch(&mut self, previous: &BTreeMap<i32, (f32, f32)>) {
        if previous.is_empty() {
            if let Some((&_id, &(x, y))) = self.pointers.iter().next() {
                self.menu_tap = Some((x.round() as i32, y.round() as i32));
            }
            return;
        }
        if !self.pointers.is_empty() {
            return;
        }
        let Some((_, (x, y))) = previous.iter().next() else {
            self.menu_tap = None;
            return;
        };
        if self.menu_tap.take().is_none() {
            return;
        }
        if self.gamepad_axis_editor {
            match quick_menu::hit_test_gamepad_axis_editor(
                self.width,
                self.height,
                x.round() as i32,
                y.round() as i32,
            ) {
                Some(quick_menu::GamepadAxisEditorHit::Scope) => {
                    self.toggle_gamepad_mapping_scope();
                }
                Some(quick_menu::GamepadAxisEditorHit::InvertX) => {
                    self.toggle_gamepad_axis_invert_x();
                }
                Some(quick_menu::GamepadAxisEditorHit::InvertY) => {
                    self.toggle_gamepad_axis_invert_y();
                }
                Some(quick_menu::GamepadAxisEditorHit::SensitivityDown) => {
                    self.adjust_gamepad_axis_sensitivity(-10);
                }
                Some(quick_menu::GamepadAxisEditorHit::SensitivityUp) => {
                    self.adjust_gamepad_axis_sensitivity(10);
                }
                Some(quick_menu::GamepadAxisEditorHit::DeadzoneDown) => {
                    self.adjust_gamepad_axis_deadzone(-1);
                }
                Some(quick_menu::GamepadAxisEditorHit::DeadzoneUp) => {
                    self.adjust_gamepad_axis_deadzone(1);
                }
                Some(quick_menu::GamepadAxisEditorHit::DeadzoneInherit) => {
                    self.inherit_gamepad_axis_deadzone();
                }
                Some(quick_menu::GamepadAxisEditorHit::Reset) => {
                    self.reset_gamepad_axis_settings_editor();
                }
                Some(quick_menu::GamepadAxisEditorHit::Back) => {
                    self.gamepad_axis_editor = false;
                }
                None => {}
            }
            return;
        }
        match quick_menu::hit_test_gamepad_mapping_editor(
            self.width,
            self.height,
            x.round() as i32,
            y.round() as i32,
        ) {
            Some(quick_menu::GamepadMappingEditorHit::Scope) => {
                self.cancel_gamepad_mapping_capture();
                self.toggle_gamepad_mapping_scope();
            }
            Some(quick_menu::GamepadMappingEditorHit::Control(index)) => {
                self.gamepad_mapping_selected = index;
            }
            Some(quick_menu::GamepadMappingEditorHit::Target(index)) => {
                self.gamepad_mapping_selected = index;
                self.cycle_gamepad_mapping_target(1);
            }
            Some(quick_menu::GamepadMappingEditorHit::Reset) => {
                self.cancel_gamepad_mapping_capture();
                self.reset_gamepad_mapping_editor();
            }
            Some(quick_menu::GamepadMappingEditorHit::Axes) => {
                self.cancel_gamepad_mapping_capture();
                self.gamepad_axis_editor = true;
                self.load_gamepad_axis_settings_editor_scope();
            }
            Some(quick_menu::GamepadMappingEditorHit::Capture) => {
                if self.gamepad_mapping_capture {
                    self.cancel_gamepad_mapping_capture();
                } else {
                    self.start_gamepad_mapping_capture();
                }
            }
            Some(quick_menu::GamepadMappingEditorHit::Close) => {
                self.finish_gamepad_mapping_editor();
            }
            None => {}
        }
    }

    fn update_game_menu_touch(&mut self, previous: &BTreeMap<i32, (f32, f32)>) {
        if previous.is_empty() {
            if let Some((&_id, &(x, y))) = self.pointers.iter().next() {
                self.menu_tap = Some((x.round() as i32, y.round() as i32));
            }
            return;
        }
        if self.pointers.is_empty() {
            let last = previous.values().next().copied();
            if let (Some(_start), Some((x, y))) = (self.menu_tap.take(), last) {
                if let Some(index) = quick_menu::hit_test(
                    self.width,
                    self.height,
                    x.round() as i32,
                    y.round() as i32,
                ) {
                    self.quick_menu_selected = index;
                    self.activate_game_menu_action();
                }
            }
        }
    }

    fn update_game_menu_confirmation_touch(
        &mut self,
        previous: &BTreeMap<i32, (f32, f32)>,
        action: quick_menu::ConfirmationAction,
    ) {
        if previous.is_empty() {
            if let Some((&_id, &(x, y))) = self.pointers.iter().next() {
                self.menu_tap = Some((x.round() as i32, y.round() as i32));
            }
            return;
        }
        if self.pointers.is_empty() {
            let last = previous.values().next().copied();
            if let (Some(_start), Some((x, y))) = (self.menu_tap.take(), last) {
                if let Some(index) = quick_menu::hit_test_confirmation(
                    self.width,
                    self.height,
                    x.round() as i32,
                    y.round() as i32,
                ) {
                    self.confirmation_selected = index;
                    self.activate_game_menu_confirmation(action);
                }
            }
        }
    }

    /// Opens the menu at a frame boundary. The app thread keeps processing input
    /// and drawing the last LCD scanout while no guest frame is executed.
    fn open_game_menu(&mut self) {
        if self.system.is_none() {
            return;
        }
        self.key_mask = 0;
        self.gamepad_button_mask = 0;
        self.gamepad_trigger_axis_mask = 0;
        self.controls_editor = false;
        self.gamepad_mapping_editor = false;
        self.control_drag = None;
        self.control_resize = None;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        self.audio.clear();
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
        self.quick_menu_selected = 0;
        self.confirmation_selected = 0;
        self.quick_menu_notice = None;
        self.screen = Screen::GameMenu;
    }

    fn resume_game(&mut self) {
        if self.system.is_none() {
            self.leave_game();
            return;
        }
        self.key_mask = 0;
        self.gamepad_button_mask = 0;
        self.gamepad_trigger_axis_mask = 0;
        self.controls_editor = false;
        self.gamepad_mapping_editor = false;
        self.control_drag = None;
        self.control_resize = None;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
        self.screen = Screen::Game;
    }

    fn activate_game_menu_action(&mut self) {
        match self.quick_menu_selected {
            0 => self.resume_game(),
            1 => self.request_game_menu_confirmation(quick_menu::ConfirmationAction::Restart),
            2 => self.request_game_menu_confirmation(quick_menu::ConfirmationAction::BackToLibrary),
            3 => self.cycle_screen_layout(),
            4 => self.toggle_screen_swap(),
            5 => self.reset_game_layout(),
            6 => self.reset_virtual_controls_layout(),
            7 => self.toggle_virtual_control_edit_scope(),
            quick_menu::ACTION_EDIT_CONTROLS => self.begin_control_editor(),
            quick_menu::ACTION_CAPTURE_SCREENSHOT => self.capture_screenshot(),
            quick_menu::ACTION_EDIT_GAMEPAD_MAPPING => self.begin_gamepad_mapping_editor(),
            quick_menu::ACTION_DIAGNOSTICS => {
                self.diagnostics_visible = !self.diagnostics_visible;
                self.resume_game();
            }
            _ => {}
        }
    }

    fn capture_screenshot(&mut self) {
        let frame = {
            let Some(system) = self.system.as_ref() else {
                self.library.set_status("NÃO HÁ JOGO ATIVO PARA CAPTURAR.");
                return;
            };
            let layout = self.game_layout();
            let mut rgba = Vec::new();
            if !compose_game_into(
                system.display(),
                &layout,
                self.width,
                self.height,
                &mut rgba,
            ) {
                self.library
                    .set_status("NÃO FOI POSSÍVEL COMPOR A CAPTURA DE TELA.");
                return;
            }
            rgba
        };

        let directory = self.games_dir.join(".aceris").join("screenshots");
        match write_screenshot_rgba8(&directory, self.width, self.height, &frame) {
            Ok(path) => {
                self.quick_menu_notice = Some("CAPTURA SALVA");
                self.library
                    .set_status(format!("CAPTURA SALVA: {}", path.display()));
            }
            Err(error) => {
                self.quick_menu_notice = Some("ERRO AO CAPTURAR");
                self.library
                    .set_status(format!("NÃO FOI POSSÍVEL SALVAR A CAPTURA: {error}"));
            }
        }
    }

    fn request_game_menu_confirmation(&mut self, action: quick_menu::ConfirmationAction) {
        self.confirmation_selected = 0;
        self.menu_tap = None;
        self.screen = Screen::GameMenuConfirm { action };
    }

    fn cancel_game_menu_confirmation(&mut self) {
        self.confirmation_selected = 0;
        self.menu_tap = None;
        self.pointers.clear();
        self.screen = Screen::GameMenu;
    }

    fn activate_game_menu_confirmation(&mut self, action: quick_menu::ConfirmationAction) {
        if self.confirmation_selected == 0 {
            self.cancel_game_menu_confirmation();
            return;
        }
        match action {
            quick_menu::ConfirmationAction::Restart => self.restart_current_game(),
            quick_menu::ConfirmationAction::BackToLibrary => self.leave_game(),
        }
    }

    fn restart_current_game(&mut self) {
        let Some(path) = self.current_game_path.clone() else {
            self.leave_game();
            return;
        };
        let mut system = System::new();
        system.enable_guest_gpu_rendering();
        if self.library.performance_mode() {
            if let Err(error) = system.set_fast_memory(true) {
                self.library
                    .set_status(format!("Modo desempenho indisponível: {error}"));
            }
        }
        let save_name = path.file_name().unwrap_or_default().to_os_string();
        system.set_storage_root(self.games_dir.join("saves").join(save_name));
        match system.load_executable_file(&path) {
            Ok(_) => {
                let game_identity = system.game_identity();
                let settings_directory = self.games_dir.join(".aceris");
                self.gamepad_mapping =
                    GamepadMapping::load_for_context(&settings_directory, None, game_identity);
                self.gamepad_axis_settings =
                    GamepadAxisSettings::load_for_context(&settings_directory, None, game_identity);
                self.library.set_active_game_identity(game_identity);
                self.system = Some(system);
                self.key_mask = 0;
                self.gamepad_button_mask = 0;
                self.gamepad_trigger_axis_mask = 0;
                self.pointers.clear();
                self.menu_tap = None;
                self.menu_button_pointer = None;
                self.quick_menu_selected = 0;
                self.screen = Screen::Game;
            }
            Err(error) => {
                self.leave_game();
                self.library
                    .set_status(format!("Não foi possível reiniciar: {error}"));
            }
        }
    }

    /// Library taps act on release so a scroll gesture does not launch a game.
    fn update_library_touch(&mut self, previous: &BTreeMap<i32, (f32, f32)>) {
        if previous.is_empty() {
            if let Some((&_id, &(x, y))) = self.pointers.iter().next() {
                self.library_tap = Some((x.round() as i32, y.round() as i32));
            }
            return;
        }
        if self.pointers.is_empty() {
            let (_, &(last_x, last_y)) = previous.iter().next().expect("previous not empty");
            if let Some((_, down_y)) = self.library_tap.take() {
                let up_x = last_x.round() as i32;
                let up_y = last_y.round() as i32;
                let dragged = up_y - down_y;
                let portrait = self.height >= self.width;
                let logical_row_height = if portrait { 58.0 } else { 94.0 };
                let logical_width = if portrait { 400.0 } else { 1000.0 };
                let row_height = (self.width as f32 / logical_width * logical_row_height)
                    .round()
                    .max(12.0) as i32;
                if dragged.abs() >= (self.height as i32 / 80).max(12) {
                    let mut rows = -(dragged as f32 / row_height as f32).round() as i32;
                    if rows == 0 {
                        rows = -dragged.signum();
                    }
                    self.library
                        .move_selection_responsive(rows, self.width, self.height);
                } else {
                    match self
                        .library
                        .click_responsive(up_x, up_y, self.width, self.height)
                    {
                        // On touch devices, one deliberate tap should launch a game.
                        // Desktop keeps the familiar select-then-Enter/double-click flow.
                        ClickResult::Activated | ClickResult::Selected => self.launch_selected(),
                        ClickResult::Ignored => {}
                    }
                }
            }
        }
        if self.library.take_folder_picker_request() {
            // The pure-Rust NativeActivity host has no Activity.onActivityResult
            // bridge for SAF. Keep path entry usable instead of opening a picker
            // whose selected document URI the host would be unable to retain.
            self.library.begin_add_folder_path();
        }
    }

    fn launch_selected(&mut self) {
        if self.library.page() == library::LibraryPage::Settings {
            self.library.reload();
            return;
        }
        let Some(entry) = self.library.selected_entry() else {
            self.library.set_status("Nenhum jogo na pasta.");
            return;
        };
        let path = entry.path.clone();
        let mut system = System::new();
        system.enable_guest_gpu_rendering();
        if self.library.performance_mode() {
            // Opt-in "Desempenho" mode chosen in Settings; never a default.
            if let Err(error) = system.set_fast_memory(true) {
                self.library
                    .set_status(format!("Modo desempenho indisponível: {error}"));
            }
        }
        let save_name = path.file_name().unwrap_or_default().to_os_string();
        system.set_storage_root(self.games_dir.join("saves").join(save_name));
        self.active_image_identity = None;
        match load_executable_file_with_identity(&mut system, &path) {
            Ok(loaded) => {
                let settings_directory = self.games_dir.join(".aceris");
                self.gamepad_mapping = GamepadMapping::load_for_context(
                    &settings_directory,
                    None,
                    loaded.game_identity,
                );
                self.gamepad_axis_settings = GamepadAxisSettings::load_for_context(
                    &settings_directory,
                    None,
                    loaded.game_identity,
                );
                self.library.set_active_game_identity(loaded.game_identity);
                self.library.mark_launched(&path);
                self.active_image_identity = Some(loaded);
                self.system = Some(system);
                self.current_game_path = Some(path);
                self.key_mask = 0;
                self.gamepad_button_mask = 0;
                self.gamepad_trigger_axis_mask = 0;
                self.quick_menu_selected = 0;
                self.screen = Screen::Game;
                self.apply_game_input();
            }
            Err(error) => {
                self.library
                    .set_status(format!("Não foi possível carregar: {error}"));
            }
        }
    }

    fn game_diagnostic(&self) -> Option<String> {
        let system = self.system.as_ref()?;
        let requests = system.unhandled_requests();
        requests
            .iter()
            .find(|request| request.starts_with("srv:GetServiceHandle"))
            .or_else(|| requests.first())
            .cloned()
    }

    fn leave_game(&mut self) {
        let diagnostic = self.game_diagnostic();
        self.system = None;
        self.active_image_identity = None;
        self.library.set_active_game_identity(None);
        self.current_game_path = None;
        self.audio.clear();
        self.key_mask = 0;
        self.gamepad_button_mask = 0;
        self.gamepad_trigger_axis_mask = 0;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        self.quick_menu_selected = 0;
        self.confirmation_selected = 0;
        self.screen = Screen::Library;
        if let Some(diagnostic) = diagnostic {
            self.library
                .set_status(format!("Diagnóstico HLE: {diagnostic}"));
        }
    }
}

fn write_screenshot_rgba8(
    directory: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    let encoded = aceris_core::video::png::encode_rgba8(width as usize, height as usize, rgba)
        .ok_or_else(|| "dimensões ou buffer RGBA inválidos".to_string())?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = directory.join(format!("aceris-{timestamp}.png"));
    fs::write(&path, encoded).map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod screenshot_tests {
    use super::*;

    #[test]
    fn physical_gamepad_left_stick_navigates_home_with_edge_triggering() {
        let directory = std::env::temp_dir().join(format!("aceris-home-stick-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut app = App::new(&directory);
        app.resize(1000, 600);
        app.screen = Screen::Library;
        app.library.home_focus = HomeFocus::Actions;
        app.library.home_action_selected = 0;

        assert!(app.set_gamepad_axes(0.8, 0.0));
        assert_eq!(app.library.home_action_selected, 1);
        assert!(app.set_gamepad_axes(0.8, 0.0));
        assert_eq!(app.library.home_action_selected, 1, "a held stick must not repeat a focus change every frame");

        assert!(app.set_gamepad_axes(0.0, 0.0));
        assert!(app.set_gamepad_axes(0.8, 0.0));
        assert_eq!(app.library.home_action_selected, 2);

        assert!(app.set_gamepad_axes(0.0, 0.0));
        app.library.home_focus = HomeFocus::Games;
        app.library.home_action_selected = 0;
        assert!(app.set_gamepad_axes(0.0, 0.8));
        assert_eq!(app.library.home_focus, HomeFocus::Actions);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn home_gamepad_stick_mapping_uses_deadzone_and_dominant_axis() {
        assert_eq!(gamepad_home_navigation_mask(0.59, 0.0), 0);
        assert_eq!(gamepad_home_navigation_mask(-0.60, 0.0), GAMEPAD_HOME_NAV_LEFT);
        assert_eq!(gamepad_home_navigation_mask(0.0, 0.61), GAMEPAD_HOME_NAV_UP);
        assert_eq!(gamepad_home_navigation_mask(0.0, -0.61), GAMEPAD_HOME_NAV_DOWN);
        assert_eq!(gamepad_home_navigation_mask(0.8, 0.7), GAMEPAD_HOME_NAV_RIGHT);
        assert_eq!(gamepad_home_navigation_mask(0.7, 0.8), GAMEPAD_HOME_NAV_UP);
        assert_eq!(gamepad_home_navigation_mask(f32::NAN, 0.0), 0);
    }

    #[test]
    fn physical_gamepad_capture_moves_selected_destination_to_pressed_button() {
        let directory =
            std::env::temp_dir().join(format!("aceris-gamepad-capture-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut app = App::new(&directory);
        app.system = Some(System::new());
        app.begin_gamepad_mapping_editor();
        app.start_gamepad_mapping_capture();
        assert!(app.gamepad_mapping_capture);
        assert!(app.gamepad_key(97, true));
        assert!(!app.gamepad_mapping_capture);
        assert_eq!(app.gamepad_mapping_selected, GamepadControl::East as usize);
        assert_eq!(app.gamepad_mapping.target(GamepadControl::South), None);
        assert_eq!(
            app.gamepad_mapping.target(GamepadControl::East),
            Some(aceris_core::input_mapping::PadButton::A)
        );
        let saved = GamepadMapping::load_global(&directory.join(".aceris"));
        assert_eq!(
            saved.target(GamepadControl::East),
            Some(aceris_core::input_mapping::PadButton::A)
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn physical_gamepad_editor_saves_global_mapping_and_restores_effective_mapping() {
        let directory =
            std::env::temp_dir().join(format!("aceris-gamepad-editor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut app = App::new(&directory);
        app.system = Some(System::new());
        app.begin_gamepad_mapping_editor();
        assert!(app.gamepad_mapping_editor);
        app.cycle_gamepad_mapping_target(1);
        assert_eq!(
            app.gamepad_mapping.target(GamepadControl::South),
            Some(aceris_core::input_mapping::PadButton::B)
        );
        app.finish_gamepad_mapping_editor();
        assert!(!app.gamepad_mapping_editor);
        let saved = GamepadMapping::load_global(&directory.join(".aceris"));
        assert_eq!(
            saved.target(GamepadControl::South),
            Some(aceris_core::input_mapping::PadButton::B)
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn physical_gamepad_axis_editor_persists_global_inversion_and_sensitivity() {
        let directory =
            std::env::temp_dir().join(format!("aceris-axis-editor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let mut app = App::new(&directory);
        app.system = Some(System::new());
        app.begin_gamepad_mapping_editor();
        app.gamepad_axis_editor = true;
        app.toggle_gamepad_axis_invert_x();
        app.toggle_gamepad_axis_invert_y();
        app.adjust_gamepad_axis_sensitivity(20);

        let saved = GamepadAxisSettings::load_global(&directory.join(".aceris"));
        assert!(saved.invert_x());
        assert!(saved.invert_y());
        assert_eq!(saved.sensitivity_percent(), 120);

        app.finish_gamepad_mapping_editor();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn screenshot_writer_creates_a_valid_png_in_the_requested_directory() {
        let directory =
            std::env::temp_dir().join(format!("aceris-app-screenshot-{}", std::process::id()));
        let rgba = [255, 0, 0, 255];
        let path = write_screenshot_rgba8(&directory, 1, 1, &rgba).unwrap();
        let encoded = fs::read(&path).unwrap();

        assert_eq!(
            &encoded[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        assert_eq!(&encoded[16..24], &[0, 0, 0, 1, 0, 0, 0, 1]);
        fs::remove_dir_all(directory).unwrap();
    }
}

pub const AKEYCODE_BACK: i32 = 4;
const AKEYCODE_ENTER: i32 = 66;
const AKEYCODE_DEL: i32 = 67;
const AKEYCODE_BUTTON_L2: i32 = 104;
const AKEYCODE_BUTTON_R2: i32 = 105;

fn gamepad_control_from_android_keycode(code: i32) -> Option<GamepadControl> {
    Some(match code {
        19 => GamepadControl::DPadUp,
        20 => GamepadControl::DPadDown,
        21 => GamepadControl::DPadLeft,
        22 => GamepadControl::DPadRight,
        96 => GamepadControl::South,
        97 => GamepadControl::East,
        99 => GamepadControl::North,
        100 => GamepadControl::West,
        102 => GamepadControl::LeftShoulder,
        103 => GamepadControl::RightShoulder,
        AKEYCODE_BUTTON_L2 => GamepadControl::LeftTrigger,
        AKEYCODE_BUTTON_R2 => GamepadControl::RightTrigger,
        108 => GamepadControl::Start,
        109 => GamepadControl::Select,
        _ => return None,
    })
}

/// Maps an Android key code to a front-end button bit (`frontend::buttons`).
///
/// D-pad and gamepad buttons only; letters are left to the system.
pub fn pad_from_android_keycode(code: i32) -> Option<u32> {
    use frontend::buttons as bit;
    Some(match code {
        19 => bit::UP,
        20 => bit::DOWN,
        21 => bit::LEFT,
        22 => bit::RIGHT,
        96 => bit::A,  // BUTTON_A
        97 => bit::B,  // BUTTON_B
        99 => bit::X,  // BUTTON_X
        100 => bit::Y, // BUTTON_Y
        102 => bit::L, // BUTTON_L1
        103 => bit::R, // BUTTON_R1
        108 => bit::START,
        109 => bit::SELECT,
        _ => return None,
    })
}

fn gamepad_trigger_control_mask(left: f32, right: f32) -> u16 {
    let is_pressed = |value: f32| value.is_finite() && value.clamp(0.0, 1.0) >= 0.5;
    (if is_pressed(left) {
        GamepadControl::LeftTrigger.bit()
    } else {
        0
    }) | (if is_pressed(right) {
        GamepadControl::RightTrigger.bit()
    } else {
        0
    })
}

#[cfg(test)]
mod gamepad_axis_tests {
    use super::*;
    use aceris_core::input::gamepad_axes_to_circle_pad;

    #[test]
    fn android_gamepad_keycodes_map_to_standardized_controls() {
        assert_eq!(
            gamepad_control_from_android_keycode(96),
            Some(GamepadControl::South)
        );
        assert_eq!(
            gamepad_control_from_android_keycode(19),
            Some(GamepadControl::DPadUp)
        );
        assert_eq!(
            gamepad_control_from_android_keycode(102),
            Some(GamepadControl::LeftShoulder)
        );
        assert_eq!(
            gamepad_control_from_android_keycode(AKEYCODE_BUTTON_L2),
            Some(GamepadControl::LeftTrigger)
        );
        assert_eq!(
            gamepad_control_from_android_keycode(AKEYCODE_BUTTON_R2),
            Some(GamepadControl::RightTrigger)
        );
        assert_eq!(
            gamepad_control_from_android_keycode(AKEYCODE_BUTTON_R2 + 1),
            None
        );
    }

    #[test]
    fn analog_trigger_axes_use_a_safe_digital_threshold() {
        assert_eq!(gamepad_trigger_control_mask(0.0, 0.0), 0);
        assert_eq!(gamepad_trigger_control_mask(0.49, 0.49), 0);
        assert_eq!(
            gamepad_trigger_control_mask(0.5, 0.0),
            GamepadControl::LeftTrigger.bit()
        );
        assert_eq!(
            gamepad_trigger_control_mask(0.0, 0.5),
            GamepadControl::RightTrigger.bit()
        );
        assert_eq!(
            gamepad_trigger_control_mask(1.0, 1.0),
            GamepadControl::LeftTrigger.bit() | GamepadControl::RightTrigger.bit()
        );
        assert_eq!(gamepad_trigger_control_mask(f32::NAN, f32::INFINITY), 0);
        assert_eq!(gamepad_trigger_control_mask(-1.0, -0.5), 0);
    }

    #[test]
    fn android_stick_uses_the_shared_radial_deadzone_mapping() {
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
        assert_ne!(gamepad_axes_to_circle_pad(0.12, 0.12, 16), (0, 0));
    }
}

/// Samples rendered per opening frame before scaling to the surface.
const INTRO_PIXEL_BUDGET: f32 = 400.0 * 400.0;

fn resize_rgba_frame(frame: &mut Vec<u8>, width: u32, height: u32) -> bool {
    let Some(byte_len) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixel_count| pixel_count.checked_mul(4))
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
    frame.resize(byte_len, 0);
    true
}

fn fill_opaque_black(pixels: &mut [u8]) {
    const OPAQUE_BLACK: [u8; 4] = [0, 0, 0, 0xFF];
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&OPAQUE_BLACK);
    }
}

fn reset_black_frame(frame: &mut Vec<u8>, width: u32, height: u32) -> bool {
    if !resize_rgba_frame(frame, width, height) {
        return false;
    }
    fill_opaque_black(frame);
    true
}

/// Clears only the parts of the host frame that are not overwritten by LCD blits.
///
/// The two screen rectangles are merged per row, so reused pixels cannot leak
/// through the gutter, letterboxing, or portrait/landscape margins.
fn reset_frame_background(
    frame: &mut Vec<u8>,
    width: u32,
    height: u32,
    screens: [ScreenRect; 2],
) -> bool {
    if !resize_rgba_frame(frame, width, height) {
        return false;
    }
    if width == 0 || height == 0 {
        return true;
    }

    let width_pixels = width as usize;
    let width_bound = i64::from(width);
    for row in 0..height as usize {
        let y = row as i64;
        let mut ranges = [(0usize, 0usize); 2];
        let mut range_count = 0usize;

        for screen in screens {
            let top = i64::from(screen.y);
            let bottom = top + i64::from(screen.height);
            if y < top || y >= bottom {
                continue;
            }

            let start = i64::from(screen.x).clamp(0, width_bound) as usize;
            let end =
                (i64::from(screen.x) + i64::from(screen.width)).clamp(0, width_bound) as usize;
            if start < end {
                ranges[range_count] = (start, end);
                range_count += 1;
            }
        }

        if range_count == 2 && ranges[1].0 < ranges[0].0 {
            ranges.swap(0, 1);
        }

        let row_start = row * width_pixels * 4;
        let mut covered_until = 0usize;
        for &(start, end) in &ranges[..range_count] {
            if start > covered_until {
                let clear_start = row_start + covered_until * 4;
                let clear_end = row_start + start * 4;
                fill_opaque_black(&mut frame[clear_start..clear_end]);
            }
            covered_until = covered_until.max(end);
        }
        if covered_until < width_pixels {
            let clear_start = row_start + covered_until * 4;
            fill_opaque_black(&mut frame[clear_start..row_start + width_pixels * 4]);
        }
    }
    true
}

/// Nearest-neighbour blit of an RGBA `source` into `rect` of an RGBA `target`.
fn blit_scaled(
    target: &mut [u8],
    target_width: u32,
    source: &[u8],
    source_width: u32,
    source_height: u32,
    rect: ScreenRect,
) {
    if rect.width == 0 || rect.height == 0 || source_width == 0 || source_height == 0 {
        return;
    }
    let target_height = (target.len() / 4) as u32 / target_width.max(1);
    // Compute the nearest-neighbour horizontal step once, then advance it
    // with integer error accumulation instead of dividing for every pixel.
    let source_step = source_width / rect.width;
    let source_remainder = source_width % rect.width;
    for row in 0..rect.height {
        let y = rect.y + row as i32;
        if y < 0 || y as u32 >= target_height {
            continue;
        }
        let source_y = (row * source_height / rect.height).min(source_height - 1) as usize;
        let mut source_x = 0u32;
        let mut source_error = 0u32;
        for column in 0..rect.width {
            let x = rect.x + column as i32;
            if x >= 0 && (x as u32) < target_width {
                let from = (source_y * source_width as usize
                    + source_x.min(source_width - 1) as usize)
                    * 4;
                let to = (y as usize * target_width as usize + x as usize) * 4;
                target[to..to + 4].copy_from_slice(&source[from..from + 4]);
            }

            source_x += source_step;
            source_error += source_remainder;
            if source_error >= rect.width {
                source_x += 1;
                source_error -= rect.width;
            }
        }
    }
}

/// Composes both LCDs into an RGBA frame of the surface size.
pub fn compose_game(display: &DualScreenDisplay, width: u32, height: u32) -> Vec<u8> {
    let mut frame = Vec::new();
    let layout = compute_screen_layout(width, height, ORIENTATION_AUTO);
    compose_game_into(display, &layout, width, height, &mut frame);
    frame
}

/// Composes both LCDs into a reusable RGBA frame buffer.
fn compose_game_into(
    display: &DualScreenDisplay,
    layout: &ScreenLayout,
    width: u32,
    height: u32,
    frame: &mut Vec<u8>,
) -> bool {
    if !reset_frame_background(frame, width, height, [layout.top, layout.bottom]) {
        return false;
    }
    let mut draw = |surface: &ScreenSurface, rect: ScreenRect| {
        blit_scaled(
            frame,
            width,
            surface.pixels(),
            surface.width() as u32,
            surface.height() as u32,
            rect,
        );
    };
    draw(display.top_left(), layout.top);
    draw(display.bottom(), layout.bottom);
    true
}

/// Alpha-blends the virtual gamepad overlay onto an RGBA frame.
fn overlay_virtual_pad(frame: &mut [u8], width: u32, height: u32, pressed_mask: u32) {
    frontend::blend_virtual_pad_overlay_into(frame, width, height, ORIENTATION_AUTO, pressed_mask);
}

#[cfg(test)]
mod tests {
    use super::*;
    use aceris_core::video::ScreenTarget;
    use std::fs;

    #[test]
    fn audio_buffer_starts_empty_and_is_drained_by_take_audio() {
        let (mut app, dir) = app_with_games(0);
        assert!(app.take_audio().is_empty());
        app.audio.extend([1, 2, 3, 4]);
        assert_eq!(app.take_audio(), vec![1, 2, 3, 4]);
        assert!(app.take_audio().is_empty());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn app_loads_persisted_multi_folder_recursive_scan_config() {
        let (_initial_app, root) = app_with_games(1);
        let secondary = root.join("secondary");
        let nested = secondary.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("extra.cci"), b"not a valid image").unwrap();

        let mut config = library::LibraryScanConfig::single_directory(root.clone());
        assert!(config.add_directory(secondary));
        config.options.recursive = true;
        library::save_library_scan_config(&root.join(".aceris"), &config).unwrap();

        let app = App::new(&root);
        assert_eq!(app.library.entries().len(), 2);
        assert_eq!(app.library.scan_config().directories.len(), 2);
        assert!(app.library.scan_config().options.recursive);

        fs::remove_dir_all(root).ok();
    }

    fn app_with_games(count: usize) -> (App, std::path::PathBuf) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("aceris-app-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for index in 0..count {
            // Not a valid image: loading must fail cleanly.
            fs::write(dir.join(format!("game{index}.3dsx")), b"not a 3dsx").unwrap();
        }
        let mut app = App::new(&dir);
        app.resize(800, 960);
        (app, dir)
    }

    fn touch(app: &mut App, x: f32, y: f32) {
        app.set_pointers(&[Pointer { id: 0, x, y }]);
    }

    fn release(app: &mut App) {
        app.set_pointers(&[]);
    }

    #[test]
    fn recycled_audio_buffer_keeps_capacity_between_frames() {
        let (mut app, dir) = app_with_games(0);
        app.audio.reserve_exact(64);
        let capacity = app.audio.capacity();
        app.audio.extend([1, 2, 3, 4]);

        let samples = app.take_audio();
        assert_eq!(samples, [1, 2, 3, 4]);
        app.recycle_audio_buffer(samples);

        assert!(app.audio.is_empty());
        assert_eq!(app.audio.capacity(), capacity);
        app.audio.extend([5, 6]);
        let samples = app.take_audio();
        assert_eq!(samples, [5, 6]);
        app.recycle_audio_buffer(samples);
        assert_eq!(app.audio.capacity(), capacity);
        fs::remove_dir_all(dir).ok();
    }

    fn reference_blit_scaled(
        target: &mut [u8],
        target_width: u32,
        source: &[u8],
        source_width: u32,
        source_height: u32,
        rect: ScreenRect,
    ) {
        if rect.width == 0 || rect.height == 0 || source_width == 0 || source_height == 0 {
            return;
        }
        let target_height = (target.len() / 4) as u32 / target_width.max(1);
        for row in 0..rect.height {
            let y = rect.y + row as i32;
            if y < 0 || y as u32 >= target_height {
                continue;
            }
            let source_y = (row * source_height / rect.height).min(source_height - 1) as usize;
            for column in 0..rect.width {
                let x = rect.x + column as i32;
                if x < 0 || x as u32 >= target_width {
                    continue;
                }
                let source_x = (column * source_width / rect.width).min(source_width - 1) as usize;
                let from = (source_y * source_width as usize + source_x) * 4;
                let to = (y as usize * target_width as usize + x as usize) * 4;
                target[to..to + 4].copy_from_slice(&source[from..from + 4]);
            }
        }
    }

    #[test]
    fn incremental_blit_matches_the_reference_for_scaled_and_clipped_rectangles() {
        for (source_width, source_height) in [(2, 3), (3, 2), (5, 4)] {
            let source = (0..source_width as usize * source_height as usize)
                .flat_map(|index| {
                    let value = (index as u8).wrapping_mul(17);
                    [value, value.wrapping_add(1), value.wrapping_add(2), 255]
                })
                .collect::<Vec<_>>();
            for rect_width in [1, 2, 3, 4, 5, 6, 7] {
                for rect_height in [1, 2, 3, 5, 6] {
                    let rect = ScreenRect {
                        x: -2,
                        y: 1,
                        width: rect_width,
                        height: rect_height,
                    };
                    let mut expected = vec![0xA5; 9 * 7 * 4];
                    let mut actual = expected.clone();
                    reference_blit_scaled(
                        &mut expected,
                        9,
                        &source,
                        source_width,
                        source_height,
                        rect,
                    );
                    blit_scaled(&mut actual, 9, &source, source_width, source_height, rect);
                    assert_eq!(
                        actual, expected,
                        "source={source_width}x{source_height}, rect={rect_width}x{rect_height}"
                    );
                }
            }
        }
    }

    #[test]
    fn intro_plays_to_the_end_then_shows_the_library() {
        let (mut app, dir) = app_with_games(0);
        let first = app.step();
        assert_eq!(first.len(), 800 * 960 * 4);
        assert!(matches!(app.screen(), Screen::Intro { frame: 1 }));
        for _ in 1..intro::FRAME_COUNT {
            app.step();
        }
        assert!(matches!(app.screen(), Screen::Library));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn step_into_reuses_the_surface_frame_allocation() {
        let (mut app, dir) = app_with_games(0);
        let mut frame = Vec::new();
        app.step_into(&mut frame);
        let allocation = frame.as_ptr();
        let capacity = frame.capacity();
        for _ in 0..8 {
            app.step_into(&mut frame);
        }
        assert_eq!(frame.len(), 800 * 960 * 4);
        assert_eq!(frame.as_ptr(), allocation);
        assert_eq!(frame.capacity(), capacity);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn any_touch_or_pad_key_skips_the_intro() {
        let (mut app, dir) = app_with_games(0);
        touch(&mut app, 10.0, 10.0);
        assert!(matches!(app.screen(), Screen::Library));
        let (mut app, dir2) = app_with_games(0);
        assert!(app.key(96, true));
        assert!(matches!(app.screen(), Screen::Library));
        fs::remove_dir_all(dir).ok();
        fs::remove_dir_all(dir2).ok();
    }

    #[test]
    fn surface_without_size_produces_no_frame() {
        let dir = std::env::temp_dir();
        let mut app = App::new(&dir.join("aceris-no-such-games-dir"));
        assert!(app.step().is_empty());
    }

    #[test]
    fn one_library_tap_attempts_to_launch_the_selected_homebrew() {
        let (mut app, dir) = app_with_games(2);
        app.screen = Screen::Library;
        app.library.select_page(library::LibraryPage::Homebrew);

        // The responsive portrait layout starts cards at logical y=250.
        // A single tap on the second card must attempt to load it.
        touch(&mut app, 100.0, 328.0 * 2.0);
        release(&mut app);

        assert!(matches!(app.screen(), Screen::Library));
        assert_eq!(app.library.selected_index(), 1);
        assert!(app.library.status_message().is_some());
        assert!(app.system.is_none());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loading_an_invalid_image_reports_an_error_and_stays_in_the_library() {
        let (mut app, dir) = app_with_games(1);
        app.screen = Screen::Library;
        app.library.select_page(library::LibraryPage::Homebrew);
        assert!(app.key(96, true)); // A launches the selected entry
        assert!(matches!(app.screen(), Screen::Library));
        assert!(app.system.is_none());
        assert_eq!(app.library.recent_count(), 0);
        let frame = app.step();
        assert_eq!(frame.len(), 800 * 960 * 4);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn launching_with_no_games_shows_a_status_instead_of_crashing() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::Library;
        assert!(app.key(96, true));
        assert!(matches!(app.screen(), Screen::Library));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn game_touches_feed_buttons_and_the_bottom_screen() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::Game;
        app.system = Some({
            let mut system = System::new();
            system.enable_guest_gpu_rendering();
            system
        });
        let layout = app.game_layout();
        // Bottom-screen centre → touch (160, 120) when no virtual button is there.
        let cx = layout.bottom.x as f32 + layout.bottom.width as f32 / 2.0;
        let cy = layout.bottom.y as f32 + layout.bottom.height as f32 / 2.0;
        let on_button = app.hit_virtual_button(cx, cy) != 0;
        touch(&mut app, cx, cy);
        let snapshot = app.system.as_mut().unwrap().input_mut().snapshot();
        if !on_button {
            // Pixel centres round differently by one touch unit depending on the layout.
            let (x, y) = snapshot.touch.expect("bottom-screen touch is reported");
            assert!(
                (159..=161).contains(&x) && (119..=121).contains(&y),
                "{x},{y}"
            );
        }
        release(&mut app);
        assert_eq!(
            app.system.as_mut().unwrap().input_mut().snapshot().touch,
            None
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hardware_keys_set_pad_bits_and_back_opens_and_closes_the_quick_menu() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::Game;
        app.system = Some({
            let mut system = System::new();
            system.enable_guest_gpu_rendering();
            system
        });
        assert!(app.key(96, true));
        assert_eq!(
            app.system.as_mut().unwrap().input_mut().snapshot().held,
            pad::A
        );
        assert!(app.key(96, false));
        assert_eq!(app.system.as_mut().unwrap().input_mut().snapshot().held, 0);
        assert!(app.key(AKEYCODE_BACK, true));
        assert!(matches!(app.screen(), Screen::GameMenu));
        assert!(app.is_game_menu_open());
        assert!(app.system.is_some());
        assert!(app.key(AKEYCODE_BACK, true));
        assert!(matches!(app.screen(), Screen::Game));
        assert!(!app.is_game_menu_open());
        assert!(app.system.is_some());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn quick_menu_cycles_layout_and_swaps_physical_screens() {
        let (mut app, dir) = app_with_games(0);
        app.resize(1080, 2400);
        app.screen = Screen::GameMenu;
        app.system = Some(System::new());
        app.quick_menu_selected = 3;

        app.activate_game_menu_action();
        assert_eq!(
            app.active_layout_profile().mode,
            ScreenLayoutMode::TopEnlarged
        );
        assert!(matches!(app.screen(), Screen::GameMenu));

        app.quick_menu_selected = 4;
        app.activate_game_menu_action();
        assert!(app.active_layout_profile().swap_screens);
        assert!(matches!(app.screen(), Screen::GameMenu));

        let layout = app.game_layout();
        let normal = compute_screen_layout_for_mode(
            app.width,
            app.height,
            app.active_layout_profile().mode,
            false,
        );
        assert_eq!(layout.top, normal.bottom);
        assert_eq!(layout.bottom, normal.top);

        // Recreating App reloads the separate portrait/landscape global profiles.
        let mut reopened = App::new(&dir);
        reopened.resize(1080, 2400);
        assert_eq!(
            reopened.active_layout_profile(),
            ScreenLayoutProfile {
                mode: ScreenLayoutMode::TopEnlarged,
                swap_screens: true,
            }
        );
        reopened.resize(2400, 1080);
        assert_eq!(
            reopened.active_layout_profile(),
            ScreenLayoutProfile::landscape_default()
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn restart_requires_confirmation_and_back_cancels_without_leaving_the_game() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::GameMenu;
        app.system = Some(System::new());
        app.quick_menu_selected = 1;

        app.activate_game_menu_action();
        assert!(matches!(
            app.screen(),
            Screen::GameMenuConfirm {
                action: quick_menu::ConfirmationAction::Restart
            }
        ));
        assert!(app.is_game_menu_open());
        assert!(app.system.is_some());

        assert!(app.key(AKEYCODE_BACK, true));
        assert!(matches!(app.screen(), Screen::GameMenu));
        assert!(app.is_game_menu_open());
        assert!(app.system.is_some());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn return_to_library_occurs_only_after_confirmation() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::GameMenu;
        app.system = Some(System::new());
        app.quick_menu_selected = 2;

        app.activate_game_menu_action();
        assert!(matches!(
            app.screen(),
            Screen::GameMenuConfirm {
                action: quick_menu::ConfirmationAction::BackToLibrary
            }
        ));
        assert!(app.system.is_some());

        assert!(app.key(20, true), "Android D-pad Down is consumed");
        assert_eq!(app.confirmation_selected, 1);
        assert!(app.key(96, true), "Android A confirms the selected action");
        assert!(matches!(app.screen(), Screen::Library));
        assert!(app.system.is_none());
        assert!(!app.is_game_menu_open());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn home_dpad_focus_activates_favorite_and_import_actions() {
        let (mut app, dir) = app_with_games(1);
        app.resize(1000, 600);
        app.screen = Screen::Library;

        assert!(app.key(20, true), "D-pad Down focuses the home action row");
        assert!(app.key(22, true), "D-pad Right focuses Add Favorite");
        assert!(app.key(96, true), "A activates the focused favorite action");
        assert_eq!(app.library.favorite_count(), 1);

        assert!(app.key(22, true), "D-pad Right focuses Import Folder");
        assert!(app.key(96, true), "A opens the folder path fallback");
        assert_eq!(app.library.page(), library::LibraryPage::Settings);
        assert!(app.library.search_is_focused());

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn focused_library_accepts_android_text_backspace_and_enter() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::Library;
        app.library.select_page(library::LibraryPage::Games);
        app.library.focus_search();

        assert!(app.text_input('P'));
        assert!(app.text_input('o'));
        assert_eq!(app.library.search_query(), "Po");
        assert!(app.key(AKEYCODE_DEL, true));
        assert_eq!(app.library.search_query(), "P");
        assert!(app.key(AKEYCODE_ENTER, true));
        assert!(!app.library.search_is_focused());
        assert_eq!(app.library.search_query(), "P");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tapping_the_visible_menu_shortcut_opens_the_quick_menu_without_guest_stylus_input() {
        let (mut app, dir) = app_with_games(0);
        app.resize(2400, 1080);
        app.system = Some(System::new());
        app.screen = Screen::Game;
        let (x, y) = (app.width as i32 - 60, 40);
        assert!(quick_menu::hit_test_menu_button(
            app.width, app.height, x, y
        ));
        touch(&mut app, x as f32, y as f32);
        assert!(
            app.system
                .as_mut()
                .unwrap()
                .input_mut()
                .snapshot()
                .touch
                .is_none()
        );
        release(&mut app);
        assert!(matches!(app.screen(), Screen::GameMenu));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn quick_menu_renders_without_advancing_the_guest_frame() {
        let (mut app, dir) = app_with_games(0);
        let mut system = System::new();
        system.enable_guest_gpu_rendering();
        let frame_before = system.display().frame_number();
        app.system = Some(system);
        app.current_game_path = Some(dir.join("placeholder.3dsx"));
        app.screen = Screen::Game;
        assert!(app.key(AKEYCODE_BACK, true));
        assert!(matches!(app.screen(), Screen::GameMenu));

        let mut frame = Vec::new();
        app.step_into(&mut frame);
        assert_eq!(frame.len(), app.width as usize * app.height as usize * 4);
        assert_eq!(
            app.system.as_ref().unwrap().display().frame_number(),
            frame_before,
            "opening and drawing the quick menu must not advance guest emulation"
        );

        assert!(app.key(AKEYCODE_BACK, true));
        assert!(matches!(app.screen(), Screen::Game));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unknown_keys_are_not_consumed() {
        let (mut app, dir) = app_with_games(0);
        app.screen = Screen::Game;
        assert!(!app.key(29, true)); // KEYCODE_A letter
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn game_composition_places_both_lcds_in_the_layout() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([255, 0, 0, 255]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([0, 0, 255, 255]);
        // Landscape surface: top LCD left, bottom LCD right.
        let (width, height) = (736, 240);
        let frame = compose_game(&display, width, height);
        let at = |x: usize, y: usize| {
            let i = (y * width as usize + x) * 4;
            [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
        };
        assert_eq!(at(10, 10), [255, 0, 0, 255]);
        assert_eq!(at(700, 200), [0, 0, 255, 255]);
        // The gutter between the LCDs stays black.
        assert_eq!(at(408, 100), [0, 0, 0, 255]);
    }

    #[test]
    fn optimized_game_composition_matches_full_clear_for_reused_buffers() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(ScreenTarget::TopLeft)
            .clear([17, 34, 51, 255]);
        display
            .surface_mut(ScreenTarget::Bottom)
            .clear([68, 85, 102, 255]);

        let mut actual = Vec::new();
        for (width, height) in [
            (736, 240),
            (1080, 2400),
            (2400, 1080),
            (853, 1281),
            (1280, 720),
            (0, 720),
            (1280, 0),
        ] {
            let byte_len = (width as usize) * (height as usize) * 4;
            actual.resize(byte_len, 0xA5);
            actual.fill(0xA5);

            let mut expected = vec![0xA5; byte_len];
            assert!(reset_black_frame(&mut expected, width, height));
            let layout = compute_screen_layout(width, height, ORIENTATION_AUTO);
            reference_blit_scaled(
                &mut expected,
                width,
                display.top_left().pixels(),
                display.top_left().width() as u32,
                display.top_left().height() as u32,
                layout.top,
            );
            reference_blit_scaled(
                &mut expected,
                width,
                display.bottom().pixels(),
                display.bottom().width() as u32,
                display.bottom().height() as u32,
                layout.bottom,
            );

            assert!(compose_game_into(
                &display,
                &layout,
                width,
                height,
                &mut actual
            ));
            assert_eq!(actual, expected, "composition differs at {width}x{height}");
        }
    }

    #[test]
    fn virtual_overlay_blends_without_touching_transparent_pixels() {
        let mut frame = [10u8, 20, 30, 255].repeat(400 * 800);
        overlay_virtual_pad(&mut frame, 400, 800, 0);
        // The very first pixel is far from every button and stays untouched.
        assert_eq!(&frame[..4], &[10, 20, 30, 255]);
        assert!(frame.chunks_exact(4).any(|p| p != [10, 20, 30, 255]));
        let before = frame.clone();
        overlay_virtual_pad(&mut frame, 400, 800, frontend::buttons::A);
        assert_ne!(frame, before);
    }

    #[test]
    fn hidden_virtual_controls_stop_rendering_and_touch_capture_but_keep_hardware_input() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);
        app.library.set_virtual_controls_visible(false).unwrap();
        assert!(!App::new(&dir).library.virtual_controls_visible());

        let mut frame = [10u8, 20, 30, 255].repeat(400 * 800);
        let unchanged = frame.clone();
        app.render_virtual_controls_overlay(&mut frame, 0);
        assert_eq!(frame, unchanged);

        // This point is the centre of the default on-screen A button.
        assert_eq!(app.hit_virtual_button(361.0, 608.0), 0);
        touch(&mut app, 361.0, 608.0);
        assert_eq!(app.held_mask(), 0);

        // Physical keys remain available when the touch overlay is hidden.
        app.key_mask = frontend::buttons::A;
        assert_eq!(app.held_mask(), frontend::buttons::A);

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn virtual_control_opacity_setting_changes_android_overlay_strength() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);
        let original = [10u8, 20, 30, 255].repeat(400 * 800);

        app.library.set_virtual_controls_opacity(0).unwrap();
        let mut hidden_overlay = original.clone();
        app.render_virtual_controls_overlay(&mut hidden_overlay, 0);
        assert_eq!(hidden_overlay, original);

        app.library.set_virtual_controls_opacity(255).unwrap();
        let mut full_overlay = original.clone();
        app.render_virtual_controls_overlay(&mut full_overlay, 0);
        assert_ne!(full_overlay, original);

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn virtual_control_size_setting_updates_android_hit_testing_and_overlay() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);

        // The point sits just outside the default A button but inside the 120% target.
        assert_eq!(app.hit_virtual_button(382.0, 608.0), 0);
        app.library.set_virtual_controls_size_percent(120).unwrap();
        assert_eq!(app.hit_virtual_button(382.0, 608.0), frontend::buttons::A);

        let original = [10u8, 20, 30, 255].repeat(400 * 800);
        app.library.set_virtual_controls_size_percent(80).unwrap();
        let mut compact = original.clone();
        app.render_virtual_controls_overlay(&mut compact, 0);
        app.library.set_virtual_controls_size_percent(120).unwrap();
        let mut enlarged = original;
        app.render_virtual_controls_overlay(&mut enlarged, 0);
        assert_ne!(compact, enlarged);

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn virtual_control_preset_updates_android_hit_testing_and_rendering() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);

        // The D-pad moves to the right while its directional semantics stay intact.
        assert_eq!(app.hit_virtual_button(332.0, 576.0), frontend::buttons::X);
        app.library
            .set_virtual_controls_preset(frontend::VirtualControlPreset::LeftHanded)
            .unwrap();
        assert_eq!(app.hit_virtual_button(332.0, 576.0), frontend::buttons::UP);

        let original = [10u8, 20, 30, 255].repeat(400 * 800);
        app.library
            .set_virtual_controls_preset(frontend::VirtualControlPreset::Standard)
            .unwrap();
        let mut standard = original.clone();
        app.render_virtual_controls_overlay(&mut standard, 0);
        app.library
            .set_virtual_controls_preset(frontend::VirtualControlPreset::LeftHanded)
            .unwrap();
        let mut left_handed = original;
        app.render_virtual_controls_overlay(&mut left_handed, 0);
        assert_ne!(standard, left_handed);

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn custom_virtual_control_drag_persists_independent_orientation_positions() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);
        app.system = Some(System::new());
        app.quick_menu_selected = quick_menu::ACTION_EDIT_CONTROLS;
        app.activate_game_menu_action();
        assert!(app.controls_editor);

        // Move A 20x40 pixels in portrait; settings use normalized coordinates.
        touch(&mut app, 361.0, 608.0);
        touch(&mut app, 381.0, 648.0);
        assert_eq!(app.hit_virtual_button(381.0, 648.0), frontend::buttons::A);
        app.set_pointers(&[]);
        assert_eq!(
            app.library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            frontend::VirtualControlOffset { x: 500, y: 500 }
        );

        // The landscape layout starts from its own defaults and stores its own delta.
        app.resize(800, 400);
        touch(&mut app, 745.0, 304.0);
        touch(&mut app, 740.0, 309.0);
        assert_eq!(app.hit_virtual_button(740.0, 309.0), frontend::buttons::A);
        app.set_pointers(&[]);
        assert_eq!(
            app.library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            frontend::VirtualControlOffset { x: -63, y: 125 }
        );
        assert_eq!(
            app.library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            frontend::VirtualControlOffset { x: 500, y: 500 }
        );

        app.key(AKEYCODE_BACK, true);
        assert!(!app.controls_editor);
        let reopened = App::new(&dir);
        assert_eq!(
            reopened
                .library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            frontend::VirtualControlOffset { x: 500, y: 500 }
        );
        assert_eq!(
            reopened
                .library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            frontend::VirtualControlOffset { x: -63, y: 125 }
        );

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn quick_menu_restores_only_the_active_orientation_control_layout() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);
        app.library
            .set_virtual_controls_preset(frontend::VirtualControlPreset::LeftHanded)
            .unwrap();
        app.library.set_virtual_controls_size_percent(120).unwrap();
        app.library.move_virtual_control(
            frontend::ORIENTATION_PORTRAIT,
            frontend::buttons::A,
            500,
            -250,
        );
        app.library.resize_virtual_control(
            frontend::ORIENTATION_PORTRAIT,
            frontend::buttons::A,
            140,
        );
        app.library.move_virtual_control(
            frontend::ORIENTATION_LANDSCAPE,
            frontend::buttons::A,
            -300,
            400,
        );
        app.library.resize_virtual_control(
            frontend::ORIENTATION_LANDSCAPE,
            frontend::buttons::A,
            130,
        );
        app.library.save_virtual_control_positions().unwrap();

        app.quick_menu_selected = 6;
        app.activate_game_menu_action();
        assert_eq!(
            app.library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            frontend::VirtualControlOffset::default()
        );
        assert_eq!(
            app.library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            100
        );
        assert_eq!(
            app.library
                .virtual_control_positions()
                .offset(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            frontend::VirtualControlOffset { x: -300, y: 400 }
        );
        assert_eq!(
            app.library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            130
        );
        assert_eq!(
            app.library.virtual_controls_preset(),
            frontend::VirtualControlPreset::LeftHanded
        );
        assert_eq!(app.library.virtual_controls_size_percent(), 120);
        assert_eq!(
            app.library.status_message(),
            Some("CONTROLES RESTAURADOS PARA RETRATO")
        );

        let reopened = App::new(&dir);
        assert_eq!(
            reopened
                .library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            100
        );
        assert_eq!(
            reopened
                .library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            130
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn custom_virtual_control_pinch_resizes_independently_and_persists() {
        let (mut app, dir) = app_with_games(0);
        app.resize(400, 800);
        app.system = Some(System::new());
        app.quick_menu_selected = quick_menu::ACTION_EDIT_CONTROLS;
        app.activate_game_menu_action();
        assert!(app.controls_editor);

        // Two touches on A spread apart to enlarge only the portrait button.
        app.set_pointers(&[Pointer {
            id: 0,
            x: 353.0,
            y: 608.0,
        }]);
        app.set_pointers(&[
            Pointer {
                id: 0,
                x: 353.0,
                y: 608.0,
            },
            Pointer {
                id: 1,
                x: 369.0,
                y: 608.0,
            },
        ]);
        app.set_pointers(&[
            Pointer {
                id: 0,
                x: 353.0,
                y: 608.0,
            },
            Pointer {
                id: 1,
                x: 377.0,
                y: 608.0,
            },
        ]);
        assert_eq!(
            app.library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            150
        );
        app.set_pointers(&[Pointer {
            id: 0,
            x: 353.0,
            y: 608.0,
        }]);
        app.set_pointers(&[]);

        // Pinching inward in landscape changes only that orientation's A button.
        app.resize(800, 400);
        app.set_pointers(&[Pointer {
            id: 0,
            x: 751.0,
            y: 304.0,
        }]);
        app.set_pointers(&[
            Pointer {
                id: 0,
                x: 751.0,
                y: 304.0,
            },
            Pointer {
                id: 1,
                x: 739.0,
                y: 304.0,
            },
        ]);
        app.set_pointers(&[
            Pointer {
                id: 0,
                x: 751.0,
                y: 304.0,
            },
            Pointer {
                id: 1,
                x: 745.0,
                y: 304.0,
            },
        ]);
        assert_eq!(
            app.library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            60
        );
        assert_eq!(
            app.library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            150
        );

        app.set_pointers(&[]);
        app.key(AKEYCODE_BACK, true);
        assert!(!app.controls_editor);
        let reopened = App::new(&dir);
        assert_eq!(
            reopened
                .library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_PORTRAIT, frontend::buttons::A),
            150
        );
        assert_eq!(
            reopened
                .library
                .virtual_control_positions()
                .size_percent(frontend::ORIENTATION_LANDSCAPE, frontend::buttons::A),
            60
        );

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn android_keycodes_map_dpad_and_gamepad_buttons() {
        assert_eq!(pad_from_android_keycode(19), Some(frontend::buttons::UP));
        assert_eq!(
            pad_from_android_keycode(109),
            Some(frontend::buttons::SELECT)
        );
        assert_eq!(pad_from_android_keycode(102), Some(frontend::buttons::L));
        assert_eq!(pad_from_android_keycode(103), Some(frontend::buttons::R));
        assert_eq!(pad_from_android_keycode(104), None);
        assert_eq!(pad_from_android_keycode(105), None);
        assert_eq!(pad_from_android_keycode(29), None);
    }
}
