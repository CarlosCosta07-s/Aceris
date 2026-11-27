//! Platform-neutral application logic for the Android host (and any other
//! touch host): start-up animation, game library, running a game, virtual
//! gamepad and touch-screen mapping. It owns no Android API, so it is unit
//! tested on every platform; `native_activity` feeds it events and blits the
//! RGBA frames it produces.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aceris_core::System;
use aceris_core::input::{pad, touch_from_surface_point};
use aceris_core::video::frontend::{
    self, ORIENTATION_AUTO, ScreenLayout, ScreenRect, compute_screen_layout,
};
use aceris_core::video::library::{self, ClickResult, Library};
use aceris_core::video::{
    DualScreenDisplay, ScreenSurface, intro,
    quick_menu::{self, PixelFormat as MenuPixelFormat},
};

/// Which part of the application is on screen.
#[derive(Debug)]
pub enum Screen {
    Intro { frame: u32 },
    Library,
    Game,
    GameMenu,
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
    quick_menu_selected: usize,
    menu_tap: Option<(i32, i32)>,
    menu_button_pointer: Option<i32>,
    width: u32,
    height: u32,
    pointers: BTreeMap<i32, (f32, f32)>,
    /// Buttons held through hardware keys (gamepad, D-pad, volume keys).
    key_mask: u32,
    /// Library tap that was pressed and awaits release (taps act on release).
    library_tap: Option<(i32, i32)>,
    /// Interleaved stereo samples (32728 Hz) mixed since the last [`App::take_audio`].
    audio: Vec<i16>,
    /// Reused half-resolution intro frame to avoid one allocation per animation tick.
    intro_buffer: Vec<u8>,
}

impl App {
    /// Creates the app showing the intro, with games listed from `games_dir`.
    pub fn new(games_dir: &Path) -> Self {
        let games_dir_owned = games_dir.to_path_buf();
        let settings_directory = games_dir.join(".aceris");
        let mut library = Library::from_scan_settings(&settings_directory, games_dir);
        library.set_touch_keyboard_enabled(true);
        Self {
            games_dir: games_dir_owned,
            screen: Screen::Intro { frame: 0 },
            library,
            system: None,
            current_game_path: None,
            quick_menu_selected: 0,
            menu_tap: None,
            menu_button_pointer: None,
            width: 0,
            height: 0,
            pointers: BTreeMap::new(),
            key_mask: 0,
            library_tap: None,
            audio: Vec::new(),
            intro_buffer: Vec::new(),
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
        matches!(self.screen(), Screen::GameMenu)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Sets the surface size in pixels; call on creation and resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
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
                    Screen::Game => self.open_game_menu(),
                    Screen::GameMenu => self.resume_game(),
                    _ => {}
                }
            }
            return matches!(self.screen, Screen::Game | Screen::GameMenu | Screen::Library);
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
        match self.screen {
            Screen::Game => {
                if pressed {
                    self.key_mask |= bit;
                } else {
                    self.key_mask &= !bit;
                }
                self.apply_game_input();
            }
            Screen::GameMenu if pressed => match bit {
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
            Screen::GameMenu => {}
            Screen::Library if pressed => match bit {
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
                pad::A | pad::START => self.launch_selected(),
                _ => {}
            },
            Screen::Intro { .. } if pressed => self.screen = Screen::Library,
            _ => {}
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
                let next = frame + 1;
                self.screen = if next >= intro::FRAME_COUNT {
                    Screen::Library
                } else {
                    Screen::Intro { frame: next }
                };
                // Render the procedural intro at half resolution, then scale it
                // across the whole surface. The image stays full-screen while
                // avoiding a fourfold increase in per-pixel CPU work on phones.
                // Cap the procedural pixel workload on large/high-DPI phones.
                // The image remains full-screen; only the internal sample grid is smaller.
                let intro_scale = (self.width as f32 / 600.0)
                    .max(self.height as f32 / 360.0)
                    .max(1.0);
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
        }
    }

    fn step_game_menu_into(&mut self, output: &mut Vec<u8>) {
        let Some(system) = self.system.as_ref() else {
            self.screen = Screen::Library;
            let _ = self.library.render_responsive_into(self.width, self.height, output);
            return;
        };
        if !compose_game_into(system.display(), self.width, self.height, output) {
            return;
        }
        let _ = quick_menu::render_overlay(
            output,
            self.width,
            self.height,
            MenuPixelFormat::Rgba8,
            self.quick_menu_selected,
        );
    }

    fn step_game_into(&mut self, output: &mut Vec<u8>) {
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
        if !compose_game_into(system.display(), self.width, self.height, output) {
            return;
        }
        let pressed_mask = self.held_mask();
        overlay_virtual_pad(output, self.width, self.height, pressed_mask);
        let _ = quick_menu::render_menu_button(
            output,
            self.width,
            self.height,
            MenuPixelFormat::Rgba8,
        );
    }

    fn held_mask(&self) -> u32 {
        let touch_mask = self
            .pointers
            .values()
            .fold(0, |mask, &(x, y)| mask | self.hit_virtual_button(x, y));
        self.key_mask | touch_mask
    }

    fn hit_virtual_button(&self, x: f32, y: f32) -> u32 {
        frontend::hit_test_virtual_control(
            self.width,
            self.height,
            x as i32,
            y as i32,
            ORIENTATION_AUTO,
        )
    }

    fn game_layout(&self) -> ScreenLayout {
        compute_screen_layout(self.width, self.height, ORIENTATION_AUTO)
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
        let input = system.input_mut();
        input.set_buttons(aceris_core::input::pad_from_frontend_buttons(mask));
        match touch {
            Some((x, y)) => input.touch(i32::from(x), i32::from(y)),
            None => input.touch_end(),
        }
    }

    fn update_game_touch(&mut self, previous: &BTreeMap<i32, (f32, f32)>) {
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

    /// Opens the menu at a frame boundary. The app thread keeps processing input
    /// and drawing the last LCD scanout while no guest frame is executed.
    fn open_game_menu(&mut self) {
        if self.system.is_none() {
            return;
        }
        self.key_mask = 0;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        self.audio.clear();
        if let Some(system) = self.system.as_mut() {
            system.input_mut().reset();
        }
        self.quick_menu_selected = 0;
        self.screen = Screen::GameMenu;
    }

    fn resume_game(&mut self) {
        if self.system.is_none() {
            self.leave_game();
            return;
        }
        self.key_mask = 0;
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
            1 => self.restart_current_game(),
            _ => self.leave_game(),
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
                self.system = Some(system);
                self.key_mask = 0;
                self.pointers.clear();
                self.menu_tap = None;
                self.menu_button_pointer = None;
                self.quick_menu_selected = 0;
                self.screen = Screen::Game;
            }
            Err(error) => {
                self.leave_game();
                self.library.set_status(format!("Não foi possível reiniciar: {error}"));
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
        match system.load_executable_file(&path) {
            Ok(_) => {
                self.library.mark_launched(&path);
                self.system = Some(system);
                self.current_game_path = Some(path);
                self.key_mask = 0;
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
        self.current_game_path = None;
        self.audio.clear();
        self.key_mask = 0;
        self.pointers.clear();
        self.menu_tap = None;
        self.menu_button_pointer = None;
        self.quick_menu_selected = 0;
        self.screen = Screen::Library;
        if let Some(diagnostic) = diagnostic {
            self.library
                .set_status(format!("Diagnóstico HLE: {diagnostic}"));
        }
    }
}

pub const AKEYCODE_BACK: i32 = 4;
const AKEYCODE_ENTER: i32 = 66;
const AKEYCODE_DEL: i32 = 67;

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
    compose_game_into(display, width, height, &mut frame);
    frame
}

/// Composes both LCDs into a reusable RGBA frame buffer.
fn compose_game_into(
    display: &DualScreenDisplay,
    width: u32,
    height: u32,
    frame: &mut Vec<u8>,
) -> bool {
    let layout = compute_screen_layout(width, height, ORIENTATION_AUTO);
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
        assert!(quick_menu::hit_test_menu_button(app.width, app.height, x, y));
        touch(&mut app, x as f32, y as f32);
        assert!(app.system.as_mut().unwrap().input_mut().snapshot().touch.is_none());
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

            assert!(compose_game_into(&display, width, height, &mut actual));
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
    fn android_keycodes_map_dpad_and_gamepad_buttons() {
        assert_eq!(pad_from_android_keycode(19), Some(frontend::buttons::UP));
        assert_eq!(
            pad_from_android_keycode(109),
            Some(frontend::buttons::SELECT)
        );
        assert_eq!(pad_from_android_keycode(29), None);
    }
}
