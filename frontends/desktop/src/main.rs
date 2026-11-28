//! Aceris desktop front-end.
//!
//! Provides command-line diagnostics and an experimental native window frontend.
//! The library uses a dedicated home canvas; gameplay supports side-by-side,
//! vertical, enlarged and single-screen LCD layouts.

mod audio;
mod background;
mod gamepad;
mod performance;
mod window;

use aceris_common::{NAME, VERSION};
use aceris_core::{
    DEFAULT_FRAME_INSTRUCTION_BUDGET, System,
    loaded_image::{LoadedImageIdentity, load_executable_file_with_identity},
    video::{
        DualScreenDisplay,
        frontend::{ScreenLayoutMode, screen_layout_canvas_size},
        intro,
        layout_preferences::ScreenLayoutPreferences,
        library::{ClickResult, Library, VISIBLE_ROWS},
        quick_menu::{self, PixelFormat as MenuPixelFormat},
    },
};
use aceris_graphics::{GraphicsBackend, GraphicsPresenter};
use gamepad::{
    GamepadAxisProfileScope, GamepadManager, LIBRARY_NAV_CONFIRM, LIBRARY_NAV_DOWN,
    LIBRARY_NAV_LEFT, LIBRARY_NAV_RIGHT, LIBRARY_NAV_UP,
};
use performance::{
    DesktopFrameKind, DesktopFrameTimings, DesktopPerformanceMonitor, performance_logging_enabled,
};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
use std::process::Command;

fn print_usage() {
    println!("{NAME} {VERSION}");
    println!();
    println!("USAGE:");
    println!(
        "    aceris [GAME] [--dump-lcd <DIR>] [--window [--scale <N>] [--skip-intro] [--games <DIR>]]"
    );
    println!("    aceris --dump-intro <DIR>");
    println!("    aceris --selftest");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help       Print this help");
    println!("    -V, --version    Print the version");
    println!("        --selftest   Run the built-in CPU self-test");
    println!("        --dump-lcd <DIR>");
    println!("                     After the frame, write top_left.png, top_right.png and");
    println!("                     bottom.png into DIR (created if missing)");
    println!("        --frames <N> Headless: run N extra frames and report unimplemented requests");
    println!("        --tap <X,Y@FRAME[+HOLD]>");
    println!(
        "                     Headless: touch the bottom screen (320x240) at FRAME for HOLD frames (repeatable)"
    );
    println!("        --window     Show both LCDs in a window and run frames until it is");
    println!("                     closed (Linux/X11 only for now)");
    println!("        --scale <N>  Integer window scale factor, 1-4 (default 1)");
    println!("        --games <DIR>");
    println!("        --background-dir <DIR>");
    println!("                     Folder of 400x480 .rgba frames for an animated home background");
    println!("        --background-fps <N>");
    println!("                     Source rate for background frames, 1-60 (default 24)");
    println!("                     Folder with .3ds/.cci/.cia/.3dsx images (default:");
    println!("                     $ACERIS_GAMES or ~/Aceris/Games)");
    println!("        --skip-intro Do not play the start-up animation when the emulator opens");
    println!("        --dump-intro <DIR>");
    println!("                     Write key frames of the start-up animation as PNGs");
}

fn selftest() -> ExitCode {
    match aceris_core::selftest::run() {
        Ok(value) => {
            println!("CPU self-test passed (result: {value})");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Writes the three LCD surfaces as PNG files into `dir`.
fn dump_lcd(display: &DualScreenDisplay, dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for (name, surface) in [
        ("top_left.png", display.top_left()),
        ("top_right.png", display.top_right()),
        ("bottom.png", display.bottom()),
    ] {
        fs::write(dir.join(name), surface.to_png())?;
    }
    Ok(())
}

/// Converts a composed desktop BGRX frame to a PNG in the local screenshot directory.
fn save_screenshot_bgrx(
    bgrx: &[u8],
    width: usize,
    height: usize,
    directory: &Path,
) -> io::Result<PathBuf> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(bgrx.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "desktop screenshot buffer does not match its dimensions",
        ));
    }

    let rgba = bgrx_to_rgba(bgrx).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "desktop screenshot buffer is not a packed BGRX image",
        )
    })?;
    let encoded = aceris_core::video::png::encode_rgba8(width, height, &rgba).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "could not encode desktop screenshot dimensions",
        )
    })?;

    fs::create_dir_all(directory)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = directory.join(format!("aceris-{timestamp}.png"));
    fs::write(&path, encoded)?;
    Ok(path)
}

/// Converts host BGRX pixels to tightly packed RGBA8 without trusting X as alpha.
fn bgrx_to_rgba(bgrx: &[u8]) -> Option<Vec<u8>> {
    if bgrx.len() % 4 != 0 {
        return None;
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(bgrx.len()).ok()?;
    for pixel in bgrx.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    Some(rgba)
}

#[cfg(test)]
mod screenshot_tests {
    use super::*;

    #[test]
    fn desktop_bgrx_screenshot_conversion_preserves_rgb_and_sets_opaque_alpha() {
        assert_eq!(bgrx_to_rgba(&[30, 20, 10, 0]), Some(vec![10, 20, 30, 255]));
        assert_eq!(bgrx_to_rgba(&[0, 1, 2]), None);
    }

    #[test]
    fn desktop_screenshot_writer_creates_png() {
        let directory =
            env::temp_dir().join(format!("aceris-desktop-screenshot-{}", std::process::id()));
        let path = save_screenshot_bgrx(&[30, 20, 10, 0], 1, 1, &directory).unwrap();
        let encoded = fs::read(&path).unwrap();
        assert_eq!(
            &encoded[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        assert_eq!(&encoded[16..24], &[0, 0, 0, 1, 0, 0, 0, 1]);
        fs::remove_dir_all(directory).unwrap();
    }
}

/// Host directory holding a game's save data, extdata and SD card: `<game>.aceris-data`.
fn storage_dir_for(game: &Path) -> std::path::PathBuf {
    let mut name = game.file_name().unwrap_or_default().to_os_string();
    name.push(".aceris-data");
    game.with_file_name(name)
}

/// Loads the same NoCrypto NCSD execution path exposed by the Android frontend.
/// Opt-in "Desempenho" mode: the library setting or `ACERIS_FAST_MEMORY=1`.
/// It bypasses the precise D-cache model (see docs/PERFORMANCE_LOW_END_PLAN.md)
/// and is never a default.
fn env_fast_memory() -> bool {
    env::var("ACERIS_FAST_MEMORY").is_ok_and(|value| value == "1")
}

fn load_game(
    system: &mut System,
    game: &Path,
    fast_memory: bool,
) -> Result<LoadedImageIdentity, String> {
    system.set_storage_root(storage_dir_for(game));
    system
        .set_fast_memory(fast_memory)
        .map_err(|err| format!("could not select the memory mode: {err}"))?;
    if fast_memory {
        println!("Performance mode: fast memory enabled");
    }
    let loaded = load_executable_file_with_identity(system, game)
        .map_err(|err| format!("could not prepare {}: {err}", game.display()))?;

    println!(
        "Executable prepared: {:?}; ARM11 entry PC={:#010X}, CPSR={:#010X}",
        loaded.format,
        system.cpu().pc(),
        system.cpu().cpsr.0
    );
    Ok(loaded)
}

/// Instruction budget per frame; `ACERIS_FRAME_BUDGET` overrides the default.
fn frame_budget() -> usize {
    static BUDGET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        env::var("ACERIS_FRAME_BUDGET")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|budget| *budget > 0)
            .unwrap_or(DEFAULT_FRAME_INSTRUCTION_BUDGET)
    })
}

/// `ACERIS_STEP_AFTER=<frame>`: from that frame on, run one instruction per
/// frame and print the PC (debugging aid for guests that fail silently).
fn step_after() -> Option<u64> {
    static AFTER: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *AFTER.get_or_init(|| {
        env::var("ACERIS_STEP_AFTER")
            .ok()
            .and_then(|value| value.parse().ok())
    })
}

/// Runs one bounded frame and prints the first frame's CPU/GPU/LCD diagnostics.
fn run_frame_with_diagnostics(system: &mut System) -> Result<(), String> {
    let stepping = step_after().is_some_and(|after| system.display().frame_number() >= after);
    let report = system
        .run_frame_with_budget(if stepping { 1 } else { frame_budget() })
        .map_err(|err| {
            let cpu = system.cpu();
            format!(
                "emulation stopped: {err} (pc={:#010X} lr={:#010X} r0-r7={:x?})",
                cpu.pc(),
                cpu.regs[14],
                &cpu.regs[0..8]
            )
        })?;

    if stepping {
        let cpu = system.cpu();
        println!(
            "step pc={:#010X} lr={:#010X} r0={:#x} r1={:#x} r2={:#x}",
            cpu.pc(),
            cpu.regs[14],
            cpu.regs[0],
            cpu.regs[1],
            cpu.regs[2]
        );
    }

    if report.frame_number == 1 {
        println!(
            "First frame: instructions={}, stop={:?}, PC={:#010X}, CPSR={:#010X}, GPU lists={}, LCD screens={}",
            report.instructions_executed,
            report.cpu_stop_reason,
            system.cpu().pc(),
            system.cpu().cpsr.0,
            report.gpu_command_lists_submitted,
            report.lcd_screens_scanned_out
        );
    }

    Ok(())
}

/// Frames of the start-up animation written by `--dump-intro`.
const INTRO_DUMP_FRAMES: [u32; 8] = [0, 10, 20, 28, 39, 50, 64, 83];

/// Writes key frames of the start-up animation as PNG files into `dir`.
fn dump_intro(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let (width, height) = (window::CANVAS_WIDTH, window::CANVAS_HEIGHT);
    for frame in INTRO_DUMP_FRAMES {
        let rgba = intro::render_frame(frame, width, height);
        let png = aceris_core::video::png::encode_rgba8(width, height, &rgba)
            .ok_or_else(|| io::Error::other("could not encode PNG"))?;
        fs::write(dir.join(format!("intro_{frame:03}.png")), png)?;
    }
    Ok(())
}

/// Attempts to create a hardware presenter; the CPU path remains the fallback.
fn create_gpu_presenter(
    window: &window::Window,
    width: usize,
    height: usize,
    cache_directory: &Path,
) -> Option<GraphicsPresenter> {
    let backend = match GraphicsBackend::from_env() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("GPU presentation configuration ignored: {error}");
            return None;
        }
    };
    if backend == GraphicsBackend::Software {
        return None;
    }
    let (display, raw_window) = match window.raw_handles() {
        Ok(handles) => handles,
        Err(error) => {
            eprintln!("GPU presentation unavailable; using CPU window path: {error}");
            return None;
        }
    };
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        eprintln!("GPU presentation unavailable: requested surface size is too large");
        return None;
    };

    // SAFETY: Window is declared before the presenter and lives for the whole
    // run_windowed call. Rust drops the presenter before the native Window.
    match unsafe {
        GraphicsPresenter::new_with_cache_directory(
            display,
            raw_window,
            width,
            height,
            backend,
            Some(cache_directory),
        )
    } {
        Ok(presenter) => {
            let info = presenter.adapter_info();
            println!(
                "GPU presentation enabled: {} ({}, {})",
                info.name, info.backend, info.device_type
            );
            Some(presenter)
        }
        Err(error) => {
            eprintln!("GPU presentation unavailable; using CPU fallback: {error}");
            None
        }
    }
}

/// Presents through GPU when available; permanently falls back to the native
/// CPU window path if the surface or device is lost.
fn present_desktop_frame(
    window: &mut window::Window,
    presenter: &mut Option<GraphicsPresenter>,
    bgrx: &[u8],
    width: usize,
    height: usize,
) -> Result<(), String> {
    let gpu_result = presenter.as_mut().map(|gpu| {
        gpu.present_bgrx(
            bgrx,
            u32::try_from(width).map_err(|_| "GPU frame width is too large".to_string())?,
            u32::try_from(height).map_err(|_| "GPU frame height is too large".to_string())?,
        )
    });
    match gpu_result {
        Some(Ok(())) => return Ok(()),
        Some(Err(error)) => {
            eprintln!("GPU presenter lost; switching to CPU presentation: {error}");
            presenter.take();
        }
        None => {}
    }
    window
        .present(bgrx)
        .map_err(|error| format!("could not draw the frame: {error}"))
}

/// Plays the start-up animation; returns `Ok(false)` if the window was closed.
fn play_intro(
    window: &mut window::Window,
    presenter: &mut Option<GraphicsPresenter>,
    scale: usize,
    performance: &mut Option<DesktopPerformanceMonitor>,
) -> Result<bool, String> {
    let frame_time = Duration::from_micros(1_000_000 / u64::from(intro::FPS));
    let mut rgba = Vec::new();
    let mut bgrx = Vec::new();
    for frame in 0..intro::FRAME_COUNT {
        let started = Instant::now();
        for event in window.poll_events() {
            if event == window::WindowEvent::CloseRequested {
                return Ok(false);
            }
        }
        // Keep the procedural intro inexpensive at large desktop sizes.
        // A 2x nearest-neighbour presentation preserves the full window layout.
        let composition_started = performance.as_ref().map(|_| Instant::now());
        let intro_width = window::CANVAS_WIDTH / 2;
        let intro_height = window::CANVAS_HEIGHT / 2;
        if !intro::render_frame_into(frame, intro_width, intro_height, &mut rgba)
            || !intro::rgba_to_scaled_bgrx_into(
                &rgba,
                intro_width,
                scale.saturating_mul(2),
                &mut bgrx,
            )
        {
            return Err("could not allocate the startup animation frame".to_string());
        }
        let composition_elapsed = composition_started
            .map(|composition_started| composition_started.elapsed())
            .unwrap_or_default();
        let had_gpu_presenter = presenter.is_some();
        let presentation_started = performance.as_ref().map(|_| Instant::now());
        present_desktop_frame(
            window,
            presenter,
            &bgrx,
            window::CANVAS_WIDTH * scale,
            window::CANVAS_HEIGHT * scale,
        )?;
        let presentation_elapsed = presentation_started
            .map(|presentation_started| presentation_started.elapsed())
            .unwrap_or_default();
        if let Some(performance) = performance.as_mut() {
            performance.record(
                DesktopFrameKind::Intro,
                DesktopFrameTimings {
                    frame_time: started.elapsed(),
                    emulation_time: Duration::ZERO,
                    composition_time: composition_elapsed,
                    audio_time: Duration::ZERO,
                    presentation_time: presentation_elapsed,
                    gpu_presented: had_gpu_presenter && presenter.is_some(),
                },
            );
        }
        if let Some(rest) = frame_time.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }
    Ok(true)
}

/// How a stretch of emulation or browsing ended.
enum Flow {
    Quit,
    Restart,
    BackToLibrary,
}

/// Default folder scanned for games: `$ACERIS_GAMES`, else `~/Aceris/Games`.
fn default_games_dir() -> PathBuf {
    if let Some(dir) = env::var_os("ACERIS_GAMES") {
        return PathBuf::from(dir);
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Aceris").join("Games")
}

/// Window-mode session: the window opens and the animation plays as soon as the
/// emulator launches, then the game library is shown. Picking a game loads and
/// runs it; Esc returns to the library, and closing the window quits. A game
/// named on the command line skips the library for its first launch.
fn run_windowed(
    system: &mut System,
    game: Option<&Path>,
    games_dir: &Path,
    scale: usize,
    skip_intro: bool,
    background_dir: Option<&Path>,
    background_fps: u32,
) -> Result<(), String> {
    let settings_directory = games_dir.join(".aceris");
    let gpu_pipeline_cache_directory = settings_directory.join("gpu_pipeline_cache");
    let mut layout_preferences = ScreenLayoutPreferences::load(&settings_directory);
    let mut performance =
        performance_logging_enabled(&settings_directory).then(DesktopPerformanceMonitor::new);
    if performance.is_some() {
        eprintln!("Aceris desktop performance logging enabled (5-second reports)");
    }
    let mut window = window::Window::open(
        &format!("{NAME} {VERSION}"),
        window::CANVAS_WIDTH * scale,
        window::CANVAS_HEIGHT * scale,
    )
    .map_err(|err| format!("could not open a window: {err}"))?;
    let mut presenter = create_gpu_presenter(
        &window,
        window::CANVAS_WIDTH * scale,
        window::CANVAS_HEIGHT * scale,
        &gpu_pipeline_cache_directory,
    );
    if !skip_intro && !play_intro(&mut window, &mut presenter, scale, &mut performance)? {
        return Ok(());
    }
    let mut library = Library::from_scan_settings(&settings_directory, games_dir);
    let background = background_dir
        .map(|directory| background::Sequence::load(directory, background_fps))
        .transpose()?;
    let mut next = game.map(Path::to_path_buf);
    loop {
        let path = match next.take() {
            Some(path) => path,
            None => match browse_library(
                &mut window,
                &mut presenter,
                &mut library,
                scale,
                background.as_ref(),
                &mut performance,
                &settings_directory,
            )? {
                Some(path) => path,
                None => return Ok(()),
            },
        };

        // Start from the natural landscape canvas; gameplay may resize this
        // window to the persisted layout profile before the first frame.
        window
            .resize(
                window::GAME_CANVAS_WIDTH * scale,
                window::GAME_CANVAS_HEIGHT * scale,
            )
            .map_err(|err| format!("could not resize the gameplay window: {err}"))?;

        let _active_image_identity = match load_game(
            system,
            &path,
            library.performance_mode() || env_fast_memory(),
        ) {
            Ok(identity) => identity,
            Err(message) => {
                window
                    .resize(window::CANVAS_WIDTH * scale, window::CANVAS_HEIGHT * scale)
                    .map_err(|err| format!("could not restore the library window: {err}"))?;
                if game == Some(path.as_path()) && library.entries().is_empty() {
                    return Err(message);
                }
                library.set_status(message);
                continue;
            }
        };
        library.mark_launched(&path);
        let outcome = emulate(
            system,
            &mut window,
            &mut presenter,
            scale,
            &settings_directory,
            &mut layout_preferences,
            &mut performance,
            library.gamepad_deadzone_percent(),
        );
        report_unhandled(system);
        match outcome {
            Ok(Flow::Quit) => return Ok(()),
            Ok(Flow::Restart) => {
                next = Some(path.clone());
            }
            Ok(Flow::BackToLibrary) => {
                window
                    .resize(window::CANVAS_WIDTH * scale, window::CANVAS_HEIGHT * scale)
                    .map_err(|err| format!("could not restore the library window: {err}"))?;
            }
            // A failing game returns to the library instead of closing the emulator.
            Err(message) => {
                window
                    .resize(window::CANVAS_WIDTH * scale, window::CANVAS_HEIGHT * scale)
                    .map_err(|err| format!("could not restore the library window: {err}"))?;
                library.set_status(message);
            }
        }
    }
}

/// Shows the library until a game is chosen; `None` means the window closed.
fn browse_library(
    window: &mut window::Window,
    presenter: &mut Option<GraphicsPresenter>,
    library: &mut Library,
    scale: usize,
    background: Option<&background::Sequence>,
    performance: &mut Option<DesktopPerformanceMonitor>,
    settings_directory: &Path,
) -> Result<Option<PathBuf>, String> {
    let frame_time = Duration::from_micros(16_667);
    let mut gamepad = GamepadManager::new(settings_directory, None);
    let mut animation_frame = 0u64;
    let mut rgba = Vec::new();
    let mut bgrx = Vec::new();
    loop {
        for event in window.poll_events() {
            let mut activate = false;
            match event {
                window::WindowEvent::CloseRequested => return Ok(None),
                window::WindowEvent::Redraw => {}
                window::WindowEvent::Key(window::Key::Escape) => {
                    if library.search_is_focused() {
                        library.cancel_text_entry();
                    } else {
                        return Ok(None);
                    }
                }
                window::WindowEvent::Key(window::Key::Backspace) => {
                    library.backspace_search();
                }
                window::WindowEvent::Text(character) => {
                    library.input_search_char(character);
                }
                window::WindowEvent::Key(window::Key::Up) => library.move_selection_responsive(
                    -1,
                    window::CANVAS_WIDTH as u32,
                    window::CANVAS_HEIGHT as u32,
                ),
                window::WindowEvent::Key(window::Key::Down) => library.move_selection_responsive(
                    1,
                    window::CANVAS_WIDTH as u32,
                    window::CANVAS_HEIGHT as u32,
                ),
                window::WindowEvent::Key(window::Key::Left) => library
                    .move_selection_horizontal_responsive(
                        -1,
                        window::CANVAS_WIDTH as u32,
                        window::CANVAS_HEIGHT as u32,
                    ),
                window::WindowEvent::Key(window::Key::Right) => library
                    .move_selection_horizontal_responsive(
                        1,
                        window::CANVAS_WIDTH as u32,
                        window::CANVAS_HEIGHT as u32,
                    ),
                window::WindowEvent::Key(window::Key::PageUp) => library.move_selection_responsive(
                    -(VISIBLE_ROWS as i32),
                    window::CANVAS_WIDTH as u32,
                    window::CANVAS_HEIGHT as u32,
                ),
                window::WindowEvent::Key(window::Key::PageDown) => library
                    .move_selection_responsive(
                        VISIBLE_ROWS as i32,
                        window::CANVAS_WIDTH as u32,
                        window::CANVAS_HEIGHT as u32,
                    ),
                window::WindowEvent::Key(window::Key::Enter) => {
                    if library.page() == aceris_core::video::library::LibraryPage::Settings {
                        if library.search_is_focused() {
                            library.finish_search();
                        } else {
                            library.reload();
                        }
                    } else {
                        library.finish_search();
                        activate =
                            if library.page() == aceris_core::video::library::LibraryPage::Home {
                                library.activate_home_selection(
                                    window::CANVAS_WIDTH as u32,
                                    window::CANVAS_HEIGHT as u32,
                                ) == ClickResult::Activated
                            } else {
                                true
                            };
                    }
                }
                window::WindowEvent::Scroll(rows) => library.move_selection_responsive(
                    rows * 3,
                    window::CANVAS_WIDTH as u32,
                    window::CANVAS_HEIGHT as u32,
                ),
                window::WindowEvent::Click { x, y } => {
                    let (x, y) = (x / scale as i32, y / scale as i32);
                    activate = library.click_responsive(
                        x,
                        y,
                        window::CANVAS_WIDTH as u32,
                        window::CANVAS_HEIGHT as u32,
                    ) == ClickResult::Activated;
                }
                window::WindowEvent::FocusLost => library.finish_search(),
                window::WindowEvent::Game { .. } | window::WindowEvent::Pointer { .. } => {}
            }
            service_folder_picker_request(library);
            if activate {
                if let Some(path) = library.selected_entry().map(|entry| entry.path.clone()) {
                    return Ok(Some(path));
                }
            }
        }

        let navigation = gamepad.poll_library_navigation();
        if navigation & LIBRARY_NAV_UP != 0 {
            library.move_selection_responsive(
                -1,
                window::CANVAS_WIDTH as u32,
                window::CANVAS_HEIGHT as u32,
            );
        }
        if navigation & LIBRARY_NAV_DOWN != 0 {
            library.move_selection_responsive(
                1,
                window::CANVAS_WIDTH as u32,
                window::CANVAS_HEIGHT as u32,
            );
        }
        if navigation & LIBRARY_NAV_LEFT != 0 {
            library.move_selection_horizontal_responsive(
                -1,
                window::CANVAS_WIDTH as u32,
                window::CANVAS_HEIGHT as u32,
            );
        }
        if navigation & LIBRARY_NAV_RIGHT != 0 {
            library.move_selection_horizontal_responsive(
                1,
                window::CANVAS_WIDTH as u32,
                window::CANVAS_HEIGHT as u32,
            );
        }
        if navigation & LIBRARY_NAV_CONFIRM != 0 {
            let activate = match library.page() {
                aceris_core::video::library::LibraryPage::Home => {
                    library.activate_home_selection(
                        window::CANVAS_WIDTH as u32,
                        window::CANVAS_HEIGHT as u32,
                    ) == ClickResult::Activated
                }
                aceris_core::video::library::LibraryPage::Settings => {
                    library.reload();
                    false
                }
                _ => true,
            };
            service_folder_picker_request(library);
            if activate {
                if let Some(path) = library.selected_entry().map(|entry| entry.path.clone()) {
                    return Ok(Some(path));
                }
            }
        } else {
            service_folder_picker_request(library);
        }

        let started = Instant::now();
        let composition_started = performance.as_ref().map(|_| Instant::now());
        let background_frame = background.and_then(|sequence| sequence.frame_at(animation_frame));
        if !library.render_responsive_into_with_background(
            window::CANVAS_WIDTH as u32,
            window::CANVAS_HEIGHT as u32,
            &mut rgba,
            background_frame,
        ) || !intro::rgba_to_scaled_bgrx_into(&rgba, window::CANVAS_WIDTH, scale, &mut bgrx)
        {
            return Err("could not allocate the launcher frame".to_string());
        }
        let composition_elapsed = composition_started
            .map(|composition_started| composition_started.elapsed())
            .unwrap_or_default();
        let had_gpu_presenter = presenter.is_some();
        let presentation_started = performance.as_ref().map(|_| Instant::now());
        present_desktop_frame(
            window,
            presenter,
            &bgrx,
            window::CANVAS_WIDTH * scale,
            window::CANVAS_HEIGHT * scale,
        )?;
        let presentation_elapsed = presentation_started
            .map(|presentation_started| presentation_started.elapsed())
            .unwrap_or_default();
        if let Some(performance) = performance.as_mut() {
            performance.record(
                DesktopFrameKind::Library,
                DesktopFrameTimings {
                    frame_time: started.elapsed(),
                    emulation_time: Duration::ZERO,
                    composition_time: composition_elapsed,
                    audio_time: Duration::ZERO,
                    presentation_time: presentation_elapsed,
                    gpu_presented: had_gpu_presenter && presenter.is_some(),
                },
            );
        }
        animation_frame = animation_frame.wrapping_add(1);
        if let Some(rest) = frame_time.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }
}

/// Resolves a queued library-folder request using the desktop platform's native dialog.
fn service_folder_picker_request(library: &mut Library) {
    if !library.take_folder_picker_request() {
        return;
    }

    match choose_library_folder() {
        Ok(Some(directory)) if directory.is_dir() => match library.add_scan_directory(directory) {
            Ok(true) => {}
            Ok(false) => library.set_status("ESSA PASTA JÁ ESTÁ NA BIBLIOTECA"),
            Err(error) => library.set_status(format!("NÃO FOI POSSÍVEL SALVAR A PASTA: {error}")),
        },
        Ok(Some(_)) => library.set_status("A PASTA SELECIONADA NÃO ESTÁ DISPONÍVEL"),
        Ok(None) => library.set_status("IMPORTAÇÃO DE PASTA CANCELADA"),
        Err(error) => {
            library.begin_add_folder_path();
            library.set_status(format!("{error}. Informe o caminho da pasta manualmente."));
        }
    }
}

#[cfg(target_os = "linux")]
/// Runs a Linux desktop folder dialog without embedding a non-Rust UI toolkit.
fn run_directory_picker(program: &str, arguments: &[&str]) -> io::Result<Option<PathBuf>> {
    use std::os::unix::ffi::OsStringExt;

    let output = Command::new(program).args(arguments).output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let mut selected = output.stdout;
    while selected
        .last()
        .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
    {
        selected.pop();
    }
    if selected.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(std::ffi::OsString::from_vec(selected));
    if path.is_dir() {
        Ok(Some(path))
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the selected folder is no longer available",
        ))
    }
}

#[cfg(target_os = "windows")]
/// Uses the Windows common item dialog with folder-only selection.
fn choose_library_folder() -> Result<Option<PathBuf>, String> {
    use std::{ffi::c_void, os::windows::ffi::OsStringExt, ptr};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const CLSID_FILE_OPEN_DIALOG: Guid = Guid {
        data1: 0xDC1C5A9C,
        data2: 0xE88A,
        data3: 0x4DDE,
        data4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
    };
    const IID_FILE_OPEN_DIALOG: Guid = Guid {
        data1: 0xD57C7288,
        data2: 0xD4AD,
        data3: 0x4768,
        data4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
    };
    const CLSCTX_INPROC_SERVER: u32 = 1;
    const COINIT_APARTMENTTHREADED: u32 = 2;
    const FOS_PICKFOLDERS: u32 = 0x20;
    const FOS_FORCEFILESYSTEM: u32 = 0x40;
    const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
    const HRESULT_CANCELLED: i32 = 0x8007_04C7u32 as i32;

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, co_init: u32) -> i32;
        fn CoUninitialize();
        fn CoCreateInstance(
            class_id: *const Guid,
            outer: *mut c_void,
            context: u32,
            interface_id: *const Guid,
            object: *mut *mut c_void,
        ) -> i32;
        fn CoTaskMemFree(memory: *mut c_void);
    }

    unsafe fn release_interface(object: *mut c_void) {
        if object.is_null() {
            return;
        }
        let vtable = unsafe { *(object as *mut *const *const c_void) };
        let release = unsafe {
            std::mem::transmute::<*const c_void, unsafe extern "system" fn(*mut c_void) -> u32>(
                *vtable.add(2),
            )
        };
        unsafe {
            release(object);
        }
    }

    fn format_hresult(action: &str, result: i32) -> String {
        format!("{action} falhou (HRESULT 0x{:08X})", result as u32)
    }

    let init_result = unsafe { CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    if init_result < 0 {
        return Err(format_hresult(
            "Inicialização do seletor de pastas",
            init_result,
        ));
    }

    let mut dialog = ptr::null_mut();
    let result = unsafe {
        let created = CoCreateInstance(
            &CLSID_FILE_OPEN_DIALOG,
            ptr::null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_FILE_OPEN_DIALOG,
            &mut dialog,
        );
        if created < 0 {
            Err(format_hresult("Criação do seletor de pastas", created))
        } else {
            let vtable = *(dialog as *mut *const *const c_void);
            let get_options = std::mem::transmute::<
                *const c_void,
                unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
            >(*vtable.add(10));
            let set_options = std::mem::transmute::<
                *const c_void,
                unsafe extern "system" fn(*mut c_void, u32) -> i32,
            >(*vtable.add(9));
            let show = std::mem::transmute::<
                *const c_void,
                unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
            >(*vtable.add(3));
            let get_result = std::mem::transmute::<
                *const c_void,
                unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32,
            >(*vtable.add(20));

            let mut options = 0;
            let options_result = get_options(dialog, &mut options);
            if options_result < 0 {
                Err(format_hresult(
                    "Leitura das opções do seletor",
                    options_result,
                ))
            } else {
                let configured =
                    set_options(dialog, options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
                if configured < 0 {
                    Err(format_hresult("Configuração do seletor", configured))
                } else {
                    let shown = show(dialog, ptr::null_mut());
                    if shown == HRESULT_CANCELLED {
                        Ok(None)
                    } else if shown < 0 {
                        Err(format_hresult("Abertura do seletor", shown))
                    } else {
                        let mut item = ptr::null_mut();
                        let fetched = get_result(dialog, &mut item);
                        if fetched < 0 {
                            Err(format_hresult("Leitura da pasta selecionada", fetched))
                        } else if item.is_null() {
                            Err("O seletor não retornou uma pasta.".to_string())
                        } else {
                            let item_vtable = *(item as *mut *const *const c_void);
                            let get_display_name = std::mem::transmute::<
                                *const c_void,
                                unsafe extern "system" fn(*mut c_void, u32, *mut *mut u16) -> i32,
                            >(
                                *item_vtable.add(5)
                            );
                            let mut display_name = ptr::null_mut();
                            let named =
                                get_display_name(item, SIGDN_FILESYSPATH, &mut display_name);
                            let path_result = if named < 0 || display_name.is_null() {
                                Err(format_hresult("Leitura do caminho selecionado", named))
                            } else {
                                let mut length = 0usize;
                                while *display_name.add(length) != 0 {
                                    length += 1;
                                }
                                let wide = std::slice::from_raw_parts(display_name, length);
                                let path = PathBuf::from(std::ffi::OsString::from_wide(wide));
                                if path.is_dir() {
                                    Ok(Some(path))
                                } else {
                                    Err("A pasta selecionada não está mais disponível.".to_string())
                                }
                            };
                            if !display_name.is_null() {
                                CoTaskMemFree(display_name.cast());
                            }
                            release_interface(item);
                            path_result
                        }
                    }
                }
            }
        }
    };
    unsafe {
        release_interface(dialog);
        CoUninitialize();
    }
    result
}

#[cfg(target_os = "macos")]
/// Uses AppKit's native directory panel without a JavaScript/AppleScript helper.
fn choose_library_folder() -> Result<Option<PathBuf>, String> {
    use std::ffi::{CStr, c_char, c_void};

    type Id = *mut c_void;
    type Selector = *mut c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Selector;
        fn objc_msgSend();
    }

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSApplicationLoad() -> i8;
    }

    unsafe {
        if NSApplicationLoad() == 0 {
            return Err("Não foi possível inicializar o AppKit.".to_string());
        }
        let application_class = objc_getClass(b"NSApplication\0".as_ptr().cast());
        if application_class.is_null() {
            return Err("O aplicativo do macOS não está disponível.".to_string());
        }
        let selector = |name: &'static [u8]| sel_registerName(name.as_ptr().cast());
        let message_send = objc_msgSend as *const ();
        let send_id: unsafe extern "C" fn(Id, Selector) -> Id = std::mem::transmute(message_send);
        let _application = send_id(application_class, selector(b"sharedApplication\0"));
        let class = objc_getClass(b"NSOpenPanel\0".as_ptr().cast());
        if class.is_null() {
            return Err("O painel de pastas do macOS não está disponível.".to_string());
        }
        let send_bool: unsafe extern "C" fn(Id, Selector, i8) = std::mem::transmute(message_send);
        let send_integer: unsafe extern "C" fn(Id, Selector) -> isize =
            std::mem::transmute(message_send);
        let send_count: unsafe extern "C" fn(Id, Selector) -> usize =
            std::mem::transmute(message_send);
        let send_index: unsafe extern "C" fn(Id, Selector, usize) -> Id =
            std::mem::transmute(message_send);
        let send_pointer: unsafe extern "C" fn(Id, Selector) -> *const c_char =
            std::mem::transmute(message_send);

        let panel = send_id(class, selector(b"openPanel\0"));
        if panel.is_null() {
            return Err("Não foi possível criar o painel de pastas do macOS.".to_string());
        }
        send_bool(panel, selector(b"setCanChooseFiles:\0"), 0);
        send_bool(panel, selector(b"setCanChooseDirectories:\0"), 1);
        send_bool(panel, selector(b"setAllowsMultipleSelection:\0"), 0);

        if send_integer(panel, selector(b"runModal\0")) != 1 {
            return Ok(None);
        }
        let urls = send_id(panel, selector(b"URLs\0"));
        if urls.is_null() || send_count(urls, selector(b"count\0")) == 0 {
            return Err("O painel não retornou uma pasta.".to_string());
        }
        let url = send_index(urls, selector(b"objectAtIndex:\0"), 0);
        if url.is_null() {
            return Err("O painel não retornou uma pasta.".to_string());
        }
        let path_object = send_id(url, selector(b"path\0"));
        if path_object.is_null() {
            return Err("Não foi possível obter o caminho da pasta selecionada.".to_string());
        }
        let path_pointer = send_pointer(path_object, selector(b"UTF8String\0"));
        if path_pointer.is_null() {
            return Err("Não foi possível ler o caminho selecionado.".to_string());
        }
        let path = PathBuf::from(CStr::from_ptr(path_pointer).to_string_lossy().into_owned());
        if path.is_dir() {
            Ok(Some(path))
        } else {
            Err("A pasta selecionada não está mais disponível.".to_string())
        }
    }
}

#[cfg(target_os = "linux")]
/// Uses an installed desktop portal helper for folder selection.
fn choose_library_folder() -> Result<Option<PathBuf>, String> {
    for (program, arguments) in [
        (
            "zenity",
            vec![
                "--file-selection",
                "--directory",
                "--title=Selecione a pasta de jogos do Aceris",
            ],
        ),
        (
            "kdialog",
            vec![
                "--getexistingdirectory",
                ".",
                "Selecione a pasta de jogos do Aceris",
            ],
        ),
    ] {
        match run_directory_picker(program, &arguments) {
            Ok(selection) => return Ok(selection),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "NÃO FOI POSSÍVEL ABRIR O SELETOR DE PASTAS: {error}"
                ));
            }
        }
    }
    Err(
        "Nenhum seletor de pastas foi encontrado. Instale zenity ou kdialog para importar uma pasta."
            .to_string(),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn choose_library_folder() -> Result<Option<PathBuf>, String> {
    Err("A seleção nativa de pastas não está disponível nesta plataforma.".to_string())
}

/// Starts or moves a touch using the active layout's bottom-screen rectangle.
fn touch_at(
    system: &mut System,
    x: i32,
    y: i32,
    scale: usize,
    mode: ScreenLayoutMode,
    swap_screens: bool,
) {
    match window::touch_from_window_point_with_layout(x, y, scale, mode, swap_screens) {
        Some((tx, ty)) => system.input_mut().touch(i32::from(tx), i32::from(ty)),
        None => system.input_mut().touch_end(),
    }
}

/// Prints the services and commands the game asked for that are not implemented yet.
fn report_unhandled(system: &System) {
    let unhandled = system.unhandled_requests();
    if !unhandled.is_empty() {
        eprintln!("Not implemented (requested by the game):");
        for entry in unhandled {
            eprintln!("  {entry}");
        }
    }
}

/// Runs frames at roughly 60 Hz until the window closes or Esc is pressed.
fn emulate(
    system: &mut System,
    window: &mut window::Window,
    presenter: &mut Option<GraphicsPresenter>,
    scale: usize,
    settings_directory: &Path,
    layout_preferences: &mut ScreenLayoutPreferences,
    performance: &mut Option<DesktopPerformanceMonitor>,
    gamepad_deadzone_percent: u8,
) -> Result<Flow, String> {
    let frame_time = Duration::from_micros(16_667);
    let mut keyboard = window::KeyboardInput::default();
    let mut gamepad = GamepadManager::new(settings_directory, system.game_identity());
    let mut audio = audio::AudioOutput::open();
    let mut frame = Vec::new();
    let mut menu_open = false;
    let mut menu_selected = 0usize;
    let mut diagnostics_visible = false;
    let mut quick_menu_notice: Option<&'static str> = None;
    let mut gamepad_mapping_editor_open = false;
    let mut gamepad_axis_editor_open = false;
    let mut gamepad_axis_profile_scope = GamepadAxisProfileScope::Global;
    let mut gamepad_mapping_selected = 0usize;
    let mut gamepad_mapping_game_scope = false;
    let mut layout_profile = layout_preferences.profile_for_game(
        system.game_identity(),
        window::GAME_CANVAS_WIDTH as u32,
        window::GAME_CANVAS_HEIGHT as u32,
    );
    let (initial_width, initial_height) = screen_layout_canvas_size(layout_profile.mode);
    window
        .resize(
            initial_width as usize * scale,
            initial_height as usize * scale,
        )
        .map_err(|error| format!("could not apply the saved gameplay layout: {error}"))?;
    let mut pending_confirmation: Option<quick_menu::ConfirmationAction> = None;
    let mut confirmation_selected = 0usize;
    loop {
        let started = Instant::now();
        let mut capture_screenshot = false;
        for event in window.poll_events() {
            match event {
                window::WindowEvent::CloseRequested => return Ok(Flow::Quit),
                window::WindowEvent::Key(window::Key::Escape) => {
                    if gamepad_mapping_editor_open {
                        if gamepad.mapping_capture_active() {
                            gamepad.cancel_mapping_capture();
                        } else if gamepad_axis_editor_open {
                            gamepad_axis_editor_open = false;
                        } else {
                            gamepad_mapping_editor_open = false;
                            gamepad.reload_effective_mapping();
                            gamepad.reload_effective_axis_settings();
                        }
                    } else if pending_confirmation.take().is_some() {
                        confirmation_selected = 0;
                    } else if menu_open {
                        menu_open = false;
                        keyboard.reset(system.input_mut());
                    } else {
                        menu_open = true;
                        menu_selected = 0;
                        quick_menu_notice = None;
                        keyboard.reset(system.input_mut());
                        if let Some(audio) = audio.as_mut() {
                            if let Err(message) = audio.flush() {
                                eprintln!("desktop audio queue flush failed: {message}");
                            }
                        }
                    }
                    if let Some(performance) = performance.as_mut() {
                        performance.reset_after_pause();
                    }
                }
                window::WindowEvent::Key(window::Key::Up)
                    if gamepad_mapping_editor_open
                        && gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    let mut settings = *gamepad.axis_settings();
                    settings.set_invert_x(!settings.invert_x());
                    if let Err(error) = gamepad
                        .save_axis_settings_for_editor_scope(settings, gamepad_axis_profile_scope)
                    {
                        eprintln!("could not save gamepad axis settings: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Down)
                    if gamepad_mapping_editor_open
                        && gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    let mut settings = *gamepad.axis_settings();
                    settings.set_invert_y(!settings.invert_y());
                    if let Err(error) = gamepad
                        .save_axis_settings_for_editor_scope(settings, gamepad_axis_profile_scope)
                    {
                        eprintln!("could not save gamepad axis settings: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Left)
                    if gamepad_mapping_editor_open
                        && gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    let mut settings = *gamepad.axis_settings();
                    settings
                        .set_sensitivity_percent(settings.sensitivity_percent().saturating_sub(10));
                    if let Err(error) = gamepad
                        .save_axis_settings_for_editor_scope(settings, gamepad_axis_profile_scope)
                    {
                        eprintln!("could not save gamepad axis settings: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Right)
                    if gamepad_mapping_editor_open
                        && gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    let mut settings = *gamepad.axis_settings();
                    settings
                        .set_sensitivity_percent(settings.sensitivity_percent().saturating_add(10));
                    if let Err(error) = gamepad
                        .save_axis_settings_for_editor_scope(settings, gamepad_axis_profile_scope)
                    {
                        eprintln!("could not save gamepad axis settings: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Enter)
                    if gamepad_mapping_editor_open
                        && gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    gamepad_axis_profile_scope =
                        gamepad.next_axis_profile_scope(gamepad_axis_profile_scope);
                    let settings =
                        gamepad.load_axis_settings_for_editor_scope(gamepad_axis_profile_scope);
                    gamepad.set_axis_settings_for_editor(settings);
                }
                window::WindowEvent::Key(window::Key::Backspace)
                    if gamepad_mapping_editor_open
                        && gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                        aceris_core::input_mapping::GamepadAxisSettings::default(),
                        gamepad_axis_profile_scope,
                    ) {
                        eprintln!("could not reset gamepad axis settings: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::PageDown)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    gamepad_axis_editor_open = true;
                    gamepad_axis_profile_scope = gamepad.default_axis_profile_scope();
                    let settings =
                        gamepad.load_axis_settings_for_editor_scope(gamepad_axis_profile_scope);
                    gamepad.set_axis_settings_for_editor(settings);
                }
                window::WindowEvent::Key(window::Key::Up)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    gamepad_mapping_selected = gamepad_mapping_selected.saturating_sub(1);
                }
                window::WindowEvent::Key(window::Key::Down)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    gamepad_mapping_selected = (gamepad_mapping_selected + 1)
                        .min(aceris_core::input_mapping::GamepadControl::COUNT - 1);
                }
                window::WindowEvent::Key(window::Key::Left)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    let mut mapping = *gamepad.mapping();
                    let control =
                        aceris_core::input_mapping::GamepadControl::ALL[gamepad_mapping_selected];
                    mapping.cycle_target(control, -1);
                    if let Err(error) =
                        gamepad.save_mapping_for_editor(mapping, gamepad_mapping_game_scope)
                    {
                        eprintln!("could not save gamepad mapping: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Right)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    let mut mapping = *gamepad.mapping();
                    let control =
                        aceris_core::input_mapping::GamepadControl::ALL[gamepad_mapping_selected];
                    mapping.cycle_target(control, 1);
                    if let Err(error) =
                        gamepad.save_mapping_for_editor(mapping, gamepad_mapping_game_scope)
                    {
                        eprintln!("could not save gamepad mapping: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Enter)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    gamepad_mapping_game_scope =
                        !gamepad_mapping_game_scope && system.game_identity().is_some();
                    let mapping = if gamepad_mapping_game_scope {
                        gamepad.load_game_mapping_for_editor()
                    } else {
                        gamepad.load_global_mapping_for_editor()
                    };
                    gamepad.set_mapping_for_editor(mapping);
                }
                window::WindowEvent::Key(window::Key::Backspace)
                    if gamepad_mapping_editor_open
                        && !gamepad_axis_editor_open
                        && !gamepad.mapping_capture_active() =>
                {
                    if let Err(error) = gamepad.save_mapping_for_editor(
                        aceris_core::input_mapping::GamepadMapping::default(),
                        gamepad_mapping_game_scope,
                    ) {
                        eprintln!("could not reset gamepad mapping: {error}");
                    }
                }
                window::WindowEvent::Key(window::Key::Up) if pending_confirmation.is_some() => {
                    confirmation_selected = confirmation_selected.saturating_sub(1);
                }
                window::WindowEvent::Key(window::Key::Down) if pending_confirmation.is_some() => {
                    confirmation_selected = (confirmation_selected + 1).min(1);
                }
                window::WindowEvent::Key(window::Key::Enter) if pending_confirmation.is_some() => {
                    if confirmation_selected == 0 {
                        pending_confirmation = None;
                    } else if let Some(action) = pending_confirmation {
                        return Ok(match action {
                            quick_menu::ConfirmationAction::Restart => Flow::Restart,
                            quick_menu::ConfirmationAction::BackToLibrary => Flow::BackToLibrary,
                        });
                    }
                }
                window::WindowEvent::Key(window::Key::Up) if menu_open => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    menu_selected = quick_menu::move_selection(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        menu_selected,
                        0,
                        -1,
                        quick_menu::DESKTOP_ACTION_COUNT,
                    );
                }
                window::WindowEvent::Key(window::Key::Down) if menu_open => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    menu_selected = quick_menu::move_selection(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        menu_selected,
                        0,
                        1,
                        quick_menu::DESKTOP_ACTION_COUNT,
                    );
                }
                window::WindowEvent::Key(window::Key::Left) if menu_open => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    menu_selected = quick_menu::move_selection(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        menu_selected,
                        -1,
                        0,
                        quick_menu::DESKTOP_ACTION_COUNT,
                    );
                }
                window::WindowEvent::Key(window::Key::Right) if menu_open => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    menu_selected = quick_menu::move_selection(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        menu_selected,
                        1,
                        0,
                        quick_menu::DESKTOP_ACTION_COUNT,
                    );
                }
                window::WindowEvent::Key(window::Key::Enter)
                    if menu_open && !gamepad_mapping_editor_open =>
                {
                    match menu_selected {
                        0 => {
                            menu_open = false;
                            keyboard.reset(system.input_mut());
                            if let Some(performance) = performance.as_mut() {
                                performance.reset_after_pause();
                            }
                        }
                        1 => {
                            pending_confirmation = Some(quick_menu::ConfirmationAction::Restart);
                            confirmation_selected = 0;
                        }
                        2 => {
                            pending_confirmation =
                                Some(quick_menu::ConfirmationAction::BackToLibrary);
                            confirmation_selected = 0;
                        }
                        3 => {
                            layout_profile.mode = layout_profile.mode.next();
                            let (width, height) = screen_layout_canvas_size(layout_profile.mode);
                            window
                                .resize(width as usize * scale, height as usize * scale)
                                .map_err(|error| {
                                    format!("could not resize the gameplay layout: {error}")
                                })?;
                            layout_preferences.set_profile_for_game(
                                system.game_identity(),
                                window::GAME_CANVAS_WIDTH as u32,
                                window::GAME_CANVAS_HEIGHT as u32,
                                layout_profile,
                            );
                            if let Err(error) = layout_preferences.save(settings_directory) {
                                eprintln!("could not save screen layout preferences: {error}");
                            }
                        }
                        4 => {
                            layout_profile.swap_screens = !layout_profile.swap_screens;
                            layout_preferences.set_profile_for_game(
                                system.game_identity(),
                                window::GAME_CANVAS_WIDTH as u32,
                                window::GAME_CANVAS_HEIGHT as u32,
                                layout_profile,
                            );
                            if let Err(error) = layout_preferences.save(settings_directory) {
                                eprintln!("could not save screen layout preferences: {error}");
                            }
                        }
                        5 => {
                            if let Some(identity) = system.game_identity() {
                                if layout_preferences.reset_game_overrides(identity) {
                                    layout_profile = layout_preferences.profile_for_surface(
                                        window::GAME_CANVAS_WIDTH as u32,
                                        window::GAME_CANVAS_HEIGHT as u32,
                                    );
                                    let (width, height) =
                                        screen_layout_canvas_size(layout_profile.mode);
                                    window
                                        .resize(width as usize * scale, height as usize * scale)
                                        .map_err(|error| {
                                            format!(
                                                "could not restore the global gameplay layout: {error}"
                                            )
                                        })?;
                                    if let Err(error) = layout_preferences.save(settings_directory)
                                    {
                                        eprintln!(
                                            "could not save screen layout preferences: {error}"
                                        );
                                    }
                                }
                            }
                        }
                        6 => capture_screenshot = true,
                        7 => {
                            diagnostics_visible = !diagnostics_visible;
                            menu_open = false;
                            keyboard.reset(system.input_mut());
                            if let Some(performance) = performance.as_mut() {
                                performance.reset_after_pause();
                            }
                        }
                        8 => {
                            gamepad_mapping_editor_open = true;
                            gamepad.cancel_mapping_capture();
                            gamepad_mapping_selected = 0;
                            gamepad_mapping_game_scope = system.game_identity().is_some();
                            let mapping = if gamepad_mapping_game_scope {
                                gamepad.load_game_mapping_for_editor()
                            } else {
                                gamepad.load_global_mapping_for_editor()
                            };
                            gamepad.set_mapping_for_editor(mapping);
                        }
                        _ => {}
                    }
                }
                window::WindowEvent::Click { x, y } if pending_confirmation.is_some() => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    if let Some(choice) = quick_menu::hit_test_confirmation(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        x,
                        y,
                    ) {
                        confirmation_selected = choice;
                        if choice == 0 {
                            pending_confirmation = None;
                        } else if let Some(action) = pending_confirmation {
                            return Ok(match action {
                                quick_menu::ConfirmationAction::Restart => Flow::Restart,
                                quick_menu::ConfirmationAction::BackToLibrary => {
                                    Flow::BackToLibrary
                                }
                            });
                        }
                    }
                }
                window::WindowEvent::Click { x, y }
                    if menu_open && gamepad_mapping_editor_open && gamepad_axis_editor_open =>
                {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    match quick_menu::hit_test_gamepad_axis_editor(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        x,
                        y,
                    ) {
                        Some(quick_menu::GamepadAxisEditorHit::Scope) => {
                            gamepad_axis_profile_scope =
                                gamepad.next_axis_profile_scope(gamepad_axis_profile_scope);
                            let settings = gamepad
                                .load_axis_settings_for_editor_scope(gamepad_axis_profile_scope);
                            gamepad.set_axis_settings_for_editor(settings);
                        }
                        Some(quick_menu::GamepadAxisEditorHit::InvertX) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.set_invert_x(!settings.invert_x());
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not save gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::InvertY) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.set_invert_y(!settings.invert_y());
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not save gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::SensitivityDown) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.set_sensitivity_percent(
                                settings.sensitivity_percent().saturating_sub(10),
                            );
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not save gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::SensitivityUp) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.set_sensitivity_percent(
                                settings.sensitivity_percent().saturating_add(10),
                            );
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not save gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::DeadzoneDown) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.adjust_deadzone_override(gamepad_deadzone_percent, -1);
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not save gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::DeadzoneUp) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.adjust_deadzone_override(gamepad_deadzone_percent, 1);
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not save gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::DeadzoneInherit) => {
                            let mut settings = *gamepad.axis_settings();
                            settings.set_deadzone_override_percent(None);
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                settings,
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not inherit global deadzone: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::Reset) => {
                            if let Err(error) = gamepad.save_axis_settings_for_editor_scope(
                                aceris_core::input_mapping::GamepadAxisSettings::default(),
                                gamepad_axis_profile_scope,
                            ) {
                                eprintln!("could not reset gamepad axis settings: {error}");
                            }
                        }
                        Some(quick_menu::GamepadAxisEditorHit::Back) => {
                            gamepad_axis_editor_open = false;
                        }
                        None => {}
                    }
                }
                window::WindowEvent::Click { x, y } if menu_open && gamepad_mapping_editor_open => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    match quick_menu::hit_test_gamepad_mapping_editor(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        x,
                        y,
                    ) {
                        Some(quick_menu::GamepadMappingEditorHit::Scope) => {
                            gamepad.cancel_mapping_capture();
                            gamepad_mapping_game_scope =
                                !gamepad_mapping_game_scope && system.game_identity().is_some();
                            let mapping = if gamepad_mapping_game_scope {
                                gamepad.load_game_mapping_for_editor()
                            } else {
                                gamepad.load_global_mapping_for_editor()
                            };
                            gamepad.set_mapping_for_editor(mapping);
                        }
                        Some(quick_menu::GamepadMappingEditorHit::Control(index)) => {
                            gamepad_mapping_selected = index;
                        }
                        Some(quick_menu::GamepadMappingEditorHit::Target(index)) => {
                            gamepad_mapping_selected = index;
                            let mut mapping = *gamepad.mapping();
                            let control = aceris_core::input_mapping::GamepadControl::ALL[index];
                            mapping.cycle_target(control, 1);
                            if let Err(error) =
                                gamepad.save_mapping_for_editor(mapping, gamepad_mapping_game_scope)
                            {
                                eprintln!("could not save gamepad mapping: {error}");
                            }
                        }
                        Some(quick_menu::GamepadMappingEditorHit::Axes) => {
                            gamepad.cancel_mapping_capture();
                            gamepad_axis_editor_open = true;
                            gamepad_axis_profile_scope = gamepad.default_axis_profile_scope();
                            let settings = gamepad
                                .load_axis_settings_for_editor_scope(gamepad_axis_profile_scope);
                            gamepad.set_axis_settings_for_editor(settings);
                        }
                        Some(quick_menu::GamepadMappingEditorHit::Capture) => {
                            if gamepad.mapping_capture_active() {
                                gamepad.cancel_mapping_capture();
                            } else {
                                let selected_control =
                                    aceris_core::input_mapping::GamepadControl::ALL
                                        [gamepad_mapping_selected];
                                if gamepad.mapping().target(selected_control).is_some() {
                                    gamepad.start_mapping_capture();
                                } else {
                                    quick_menu_notice = Some("SELECIONE UM BOTAO 3DS");
                                }
                            }
                        }
                        Some(quick_menu::GamepadMappingEditorHit::Reset) => {
                            gamepad.cancel_mapping_capture();
                            if let Err(error) = gamepad.save_mapping_for_editor(
                                aceris_core::input_mapping::GamepadMapping::default(),
                                gamepad_mapping_game_scope,
                            ) {
                                eprintln!("could not reset gamepad mapping: {error}");
                            }
                        }
                        Some(quick_menu::GamepadMappingEditorHit::Close) => {
                            gamepad.cancel_mapping_capture();
                            gamepad_mapping_editor_open = false;
                            gamepad.reload_effective_mapping();
                            gamepad.reload_effective_axis_settings();
                        }
                        None => {}
                    }
                }
                window::WindowEvent::Click { x, y } if menu_open => {
                    let (canvas_width, canvas_height) =
                        screen_layout_canvas_size(layout_profile.mode);
                    if let Some(action) = quick_menu::hit_test_desktop(
                        canvas_width * scale as u32,
                        canvas_height * scale as u32,
                        x,
                        y,
                    ) {
                        menu_selected = action;
                        match action {
                            0 => {
                                menu_open = false;
                                keyboard.reset(system.input_mut());
                                if let Some(performance) = performance.as_mut() {
                                    performance.reset_after_pause();
                                }
                            }
                            1 => {
                                pending_confirmation =
                                    Some(quick_menu::ConfirmationAction::Restart);
                                confirmation_selected = 0;
                            }
                            2 => {
                                pending_confirmation =
                                    Some(quick_menu::ConfirmationAction::BackToLibrary);
                                confirmation_selected = 0;
                            }
                            3 => {
                                layout_profile.mode = layout_profile.mode.next();
                                let (width, height) =
                                    screen_layout_canvas_size(layout_profile.mode);
                                window
                                    .resize(width as usize * scale, height as usize * scale)
                                    .map_err(|error| {
                                        format!("could not resize the gameplay layout: {error}")
                                    })?;
                                layout_preferences.set_profile_for_game(
                                    system.game_identity(),
                                    window::GAME_CANVAS_WIDTH as u32,
                                    window::GAME_CANVAS_HEIGHT as u32,
                                    layout_profile,
                                );
                                if let Err(error) = layout_preferences.save(settings_directory) {
                                    eprintln!("could not save screen layout preferences: {error}");
                                }
                            }
                            4 => {
                                layout_profile.swap_screens = !layout_profile.swap_screens;
                                layout_preferences.set_profile_for_game(
                                    system.game_identity(),
                                    window::GAME_CANVAS_WIDTH as u32,
                                    window::GAME_CANVAS_HEIGHT as u32,
                                    layout_profile,
                                );
                                if let Err(error) = layout_preferences.save(settings_directory) {
                                    eprintln!("could not save screen layout preferences: {error}");
                                }
                            }
                            5 => {
                                if let Some(identity) = system.game_identity() {
                                    if layout_preferences.reset_game_overrides(identity) {
                                        layout_profile = layout_preferences.profile_for_surface(
                                            window::GAME_CANVAS_WIDTH as u32,
                                            window::GAME_CANVAS_HEIGHT as u32,
                                        );
                                        let (width, height) =
                                            screen_layout_canvas_size(layout_profile.mode);
                                        window
                                            .resize(width as usize * scale, height as usize * scale)
                                            .map_err(|error| {
                                                format!("could not restore the global gameplay layout: {error}")
                                            })?;
                                        if let Err(error) =
                                            layout_preferences.save(settings_directory)
                                        {
                                            eprintln!(
                                                "could not save screen layout preferences: {error}"
                                            );
                                        }
                                    }
                                }
                            }
                            6 => capture_screenshot = true,
                            7 => {
                                diagnostics_visible = !diagnostics_visible;
                                menu_open = false;
                                keyboard.reset(system.input_mut());
                                if let Some(performance) = performance.as_mut() {
                                    performance.reset_after_pause();
                                }
                            }
                            _ => {}
                        }
                    }
                }
                window::WindowEvent::Game { key, pressed }
                    if !menu_open && pending_confirmation.is_none() =>
                {
                    keyboard.apply(key, pressed, system.input_mut());
                }
                window::WindowEvent::Click { x, y } if !menu_open => touch_at(
                    system,
                    x,
                    y,
                    scale,
                    layout_profile.mode,
                    layout_profile.swap_screens,
                ),
                window::WindowEvent::Pointer { x, y, down: true } if !menu_open => touch_at(
                    system,
                    x,
                    y,
                    scale,
                    layout_profile.mode,
                    layout_profile.swap_screens,
                ),
                window::WindowEvent::Pointer { down: false, .. } if !menu_open => {
                    system.input_mut().touch_end()
                }
                window::WindowEvent::FocusLost => keyboard.reset(system.input_mut()),
                _ => {}
            }
        }
        if gamepad_mapping_editor_open {
            if let Some(captured_control) = gamepad.poll_mapping_capture() {
                let selected_control =
                    aceris_core::input_mapping::GamepadControl::ALL[gamepad_mapping_selected];
                if let Some(target) = gamepad.mapping().target(selected_control) {
                    let mut mapping = *gamepad.mapping();
                    mapping.assign_target_uniquely(captured_control, Some(target));
                    match gamepad.save_mapping_for_editor(mapping, gamepad_mapping_game_scope) {
                        Ok(()) => {
                            gamepad_mapping_selected = captured_control as usize;
                            quick_menu_notice = Some("BOTAO CAPTURADO");
                        }
                        Err(error) => eprintln!("could not save captured gamepad mapping: {error}"),
                    }
                } else {
                    quick_menu_notice = Some("SELECIONE UM BOTAO 3DS");
                }
            }
        } else {
            let _ = gamepad.poll_mapping_capture();
        }
        let mut emulation_elapsed = Duration::ZERO;
        let mut audio_elapsed = Duration::ZERO;
        if !menu_open {
            gamepad.apply(&keyboard, system.input_mut(), gamepad_deadzone_percent);
            let emulation_started = performance.as_ref().map(|_| Instant::now());
            run_frame_with_diagnostics(system)?;
            emulation_elapsed = emulation_started
                .map(|emulation_started| emulation_started.elapsed())
                .unwrap_or_default();

            let audio_started = performance.as_ref().map(|_| Instant::now());
            if let Some(audio) = audio.as_mut() {
                audio.feed(system);
            }
            audio_elapsed = audio_started
                .map(|audio_started| audio_started.elapsed())
                .unwrap_or_default();
        }

        let composition_started = performance.as_ref().map(|_| Instant::now());
        if !window::compose_bgrx_layout_into(
            system.display(),
            scale,
            layout_profile.mode,
            layout_profile.swap_screens,
            &mut frame,
        ) {
            return Err("could not allocate the desktop presentation buffer".to_string());
        }
        let (canvas_width, canvas_height) = screen_layout_canvas_size(layout_profile.mode);
        let frame_width = canvas_width as usize * scale;
        let frame_height = canvas_height as usize * scale;
        if capture_screenshot {
            match save_screenshot_bgrx(
                &frame,
                frame_width,
                frame_height,
                &settings_directory.join("screenshots"),
            ) {
                Ok(path) => {
                    quick_menu_notice = Some("CAPTURA SALVA");
                    println!("Screenshot salvo em {}", path.display());
                }
                Err(error) => {
                    quick_menu_notice = Some("ERRO AO CAPTURAR");
                    eprintln!("Não foi possível salvar o screenshot: {error}");
                }
            }
        }
        if diagnostics_visible && !menu_open {
            let unhandled = system.unhandled_requests();
            let host_performance = performance
                .as_ref()
                .and_then(DesktopPerformanceMonitor::latest_report);
            if !quick_menu::render_diagnostics_overlay(
                &mut frame,
                frame_width as u32,
                frame_height as u32,
                MenuPixelFormat::Bgrx8,
                system.cpu().pc(),
                unhandled.len(),
                unhandled.first().map(String::as_str),
                host_performance,
            ) {
                return Err("could not render the diagnostics overlay".to_string());
            }
        }
        if menu_open {
            let inherited_profile_label = if layout_preferences.game_has_override_for_surface(
                system.game_identity(),
                frame_width as u32,
                frame_height as u32,
            ) {
                "PERFIL DESTE JOGO"
            } else {
                "PADRÃO GLOBAL"
            };
            let profile_label = quick_menu_notice.unwrap_or(inherited_profile_label);
            let rendered = match pending_confirmation {
                Some(action) => quick_menu::render_confirmation_overlay(
                    &mut frame,
                    frame_width as u32,
                    frame_height as u32,
                    MenuPixelFormat::Bgrx8,
                    action,
                    confirmation_selected,
                ),
                None if gamepad_mapping_editor_open && gamepad_axis_editor_open => {
                    quick_menu::render_gamepad_axis_editor(
                        &mut frame,
                        frame_width as u32,
                        frame_height as u32,
                        MenuPixelFormat::Bgrx8,
                        gamepad.axis_settings(),
                        gamepad_deadzone_percent,
                        match gamepad_axis_profile_scope {
                            GamepadAxisProfileScope::Global => "ESCOPO: GLOBAL",
                            GamepadAxisProfileScope::Device => "ESCOPO: MODELO",
                            GamepadAxisProfileScope::Game => "ESCOPO: JOGO",
                        },
                    )
                }
                None if gamepad_mapping_editor_open => quick_menu::render_gamepad_mapping_editor(
                    &mut frame,
                    frame_width as u32,
                    frame_height as u32,
                    MenuPixelFormat::Bgrx8,
                    gamepad.mapping(),
                    gamepad_mapping_selected,
                    if gamepad_mapping_game_scope {
                        "ESCOPO: JOGO"
                    } else {
                        "ESCOPO: GLOBAL"
                    },
                    gamepad.mapping_capture_active(),
                ),
                None => quick_menu::render_desktop_overlay_with_profile(
                    &mut frame,
                    frame_width as u32,
                    frame_height as u32,
                    MenuPixelFormat::Bgrx8,
                    menu_selected,
                    profile_label,
                ),
            };
            if !rendered {
                return Err("could not render the quick menu overlay".to_string());
            }
        }
        let composition_elapsed = composition_started
            .map(|composition_started| composition_started.elapsed())
            .unwrap_or_default();

        let had_gpu_presenter = presenter.is_some();
        let presentation_started = performance.as_ref().map(|_| Instant::now());
        present_desktop_frame(window, presenter, &frame, frame_width, frame_height)?;
        let presentation_elapsed = presentation_started
            .map(|presentation_started| presentation_started.elapsed())
            .unwrap_or_default();
        if !menu_open {
            if let Some(performance) = performance.as_mut() {
                performance.record(
                    DesktopFrameKind::Game,
                    DesktopFrameTimings {
                        frame_time: started.elapsed(),
                        emulation_time: emulation_elapsed,
                        composition_time: composition_elapsed,
                        audio_time: audio_elapsed,
                        presentation_time: presentation_elapsed,
                        gpu_presented: had_gpu_presenter && presenter.is_some(),
                    },
                );
            }
        }
        if let Some(rest) = frame_time.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }
}

fn run(options: RunOptions) -> ExitCode {
    let mut system = System::new();
    if env::var_os("ACERIS_NO_GPU").is_none() {
        system.enable_guest_gpu_rendering();
    }

    if options.window {
        if let Err(message) = run_windowed(
            &mut system,
            options.game.as_deref(),
            &options.games_dir.clone().unwrap_or_else(default_games_dir),
            options.scale,
            options.skip_intro,
            options.background_dir.as_deref(),
            options.background_fps,
        ) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    } else if let Some(game) = &options.game {
        let _loaded_image_identity = match load_game(&mut system, game, env_fast_memory()) {
            Ok(identity) => identity,
            Err(message) => {
                eprintln!("error: {message}");
                return ExitCode::FAILURE;
            }
        };
        for frame in 0..=options.frames {
            apply_taps(&mut system, &options.taps, frame);
            if let Err(message) = run_frame_with_diagnostics(&mut system) {
                eprintln!("error: {message}");
                report_unhandled(&system);
                return ExitCode::FAILURE;
            }
        }
        println!(
            "After {} frame(s): PC={:#010X}, CPSR={:#010X}",
            options.frames + 1,
            system.cpu().pc(),
            system.cpu().cpsr.0
        );
        report_unhandled(&system);
    }

    if let Some(dir) = options.dump_lcd {
        if let Err(err) = dump_lcd(system.display(), &dir) {
            eprintln!(
                "error: could not write LCD dump to {}: {err}",
                dir.display()
            );
            return ExitCode::FAILURE;
        }
        println!("LCD surfaces written to {}", dir.display());
    }

    ExitCode::SUCCESS
}

/// Options for running a game.
#[derive(Debug, PartialEq, Eq)]
struct RunOptions {
    /// Optional only in window mode, where the emulator waits on the logo.
    game: Option<PathBuf>,
    dump_lcd: Option<PathBuf>,
    window: bool,
    scale: usize,
    skip_intro: bool,
    games_dir: Option<PathBuf>,
    background_dir: Option<PathBuf>,
    background_fps: u32,
    /// Extra frames to run in headless mode (0 = just the first frame).
    frames: u32,
    /// Scripted touches for headless runs (`--tap X,Y@FRAME[+HOLD]`).
    taps: Vec<Tap>,
}

/// One scripted touch on the bottom screen (320x240 coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tap {
    x: i32,
    y: i32,
    frame: u32,
    hold_frames: u32,
}

/// Parses `X,Y@FRAME` or `X,Y@FRAME+HOLD` (hold defaults to 3 frames).
fn parse_tap(text: &str) -> Result<Tap, String> {
    let bad = || format!("--tap expects X,Y@FRAME[+HOLD], got {text}");
    let (point, when) = text.split_once('@').ok_or_else(bad)?;
    let (x, y) = point.split_once(',').ok_or_else(bad)?;
    let (frame, hold) = match when.split_once('+') {
        Some((frame, hold)) => (frame, hold.parse::<u32>().map_err(|_| bad())?),
        None => (when, 3),
    };
    Ok(Tap {
        x: x.trim().parse().map_err(|_| bad())?,
        y: y.trim().parse().map_err(|_| bad())?,
        frame: frame.trim().parse().map_err(|_| bad())?,
        hold_frames: hold.max(1),
    })
}

/// Presses or releases the scripted touches that start or end on `frame`.
fn apply_taps(system: &mut System, taps: &[Tap], frame: u32) {
    for tap in taps {
        if frame == tap.frame {
            system.input_mut().touch(tap.x, tap.y);
        } else if frame == tap.frame.saturating_add(tap.hold_frames) {
            system.input_mut().touch_end();
        }
    }
}

/// Parses `<GAME> [--dump-lcd <DIR>] [--window] [--scale <N>]` in any order.
fn parse_run_args(args: &[String]) -> Result<RunOptions, String> {
    let mut game = None;
    let mut dump_lcd = None;
    let mut window = false;
    let mut scale = 1;
    let mut skip_intro = false;
    let mut games_dir = None;
    let mut background_dir = None;
    let mut background_fps = 24;
    let mut background_fps_configured = false;
    let mut frames = 0u32;
    let mut taps = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dump-lcd" => {
                let dir = iter
                    .next()
                    .ok_or_else(|| "--dump-lcd requires a directory".to_string())?;
                dump_lcd = Some(PathBuf::from(dir));
            }
            "--frames" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--frames requires a number".to_string())?;
                frames = value
                    .parse::<u32>()
                    .map_err(|_| format!("--frames must be a number, got {value}"))?;
            }
            "--tap" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--tap requires X,Y@FRAME[+HOLD]".to_string())?;
                taps.push(parse_tap(value)?);
            }
            "--window" => window = true,
            "--skip-intro" => skip_intro = true,
            "--games" => {
                let dir = iter
                    .next()
                    .ok_or_else(|| "--games requires a directory".to_string())?;
                games_dir = Some(PathBuf::from(dir));
            }
            "--background-dir" => {
                let dir = iter
                    .next()
                    .ok_or_else(|| "--background-dir requires a directory".to_string())?;
                background_dir = Some(PathBuf::from(dir));
            }
            "--background-fps" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--background-fps requires a number".to_string())?;
                background_fps = value
                    .parse::<u32>()
                    .ok()
                    .filter(|fps| (1..=60).contains(fps))
                    .ok_or_else(|| format!("--background-fps must be 1-60, got {value}"))?;
                background_fps_configured = true;
            }
            "--scale" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--scale requires a number".to_string())?;
                scale = match value.parse::<usize>() {
                    Ok(n @ 1..=4) => n,
                    _ => return Err(format!("--scale must be 1-4, got {value}")),
                };
            }
            _ if game.is_none() => game = Some(PathBuf::from(arg)),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
    }
    if (background_dir.is_some() || background_fps_configured) && !window {
        return Err("--background-dir/--background-fps require --window".to_string());
    }
    Ok(RunOptions {
        game: match game {
            Some(game) => Some(game),
            None if window => None,
            None => return Err("missing game path".to_string()),
        },
        dump_lcd,
        window,
        scale,
        skip_intro,
        games_dir,
        background_dir,
        background_fps,
        frames,
        taps,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("{NAME} {VERSION}");
            ExitCode::SUCCESS
        }
        Some("--selftest") => selftest(),
        Some("--dump-intro") => match args.as_slice() {
            [_, dir] => match dump_intro(Path::new(dir)) {
                Ok(()) => {
                    println!("Intro frames written to {dir}");
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: could not write intro frames to {dir}: {err}");
                    ExitCode::FAILURE
                }
            },
            _ => {
                eprintln!("error: --dump-intro requires exactly one directory");
                ExitCode::FAILURE
            }
        },
        Some(_) => match parse_run_args(&args) {
            Ok(options) => run(options),
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        },
        // Launching without arguments opens the window and plays the intro.
        None => run(RunOptions {
            game: None,
            dump_lcd: None,
            window: true,
            scale: 2,
            skip_intro: false,
            games_dir: None,
            background_dir: None,
            background_fps: 24,
            frames: 0,
            taps: Vec::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    fn options(game: &str, dump: Option<&str>, window: bool, scale: usize) -> RunOptions {
        RunOptions {
            game: Some(PathBuf::from(game)),
            dump_lcd: dump.map(PathBuf::from),
            window,
            scale,
            skip_intro: false,
            games_dir: None,
            background_dir: None,
            background_fps: 24,
            frames: 0,
            taps: Vec::new(),
        }
    }

    #[test]
    fn tap_arguments_parse_with_default_and_explicit_hold() {
        assert_eq!(
            parse_tap("160,120@400").unwrap(),
            Tap {
                x: 160,
                y: 120,
                frame: 400,
                hold_frames: 3
            }
        );
        assert_eq!(parse_tap("1,2@3+9").unwrap().hold_frames, 9);
        assert!(parse_tap("160,120").is_err());
        assert!(parse_tap("a,b@c").is_err());
    }

    #[test]
    fn run_arguments_accept_options_in_any_order() {
        // The redesigned desktop launcher uses a 960x600 logical canvas at scale 1.
        let expected = options("game.3ds", Some("out"), false, 1);
        assert_eq!(
            parse_run_args(&strings(&["game.3ds", "--dump-lcd", "out"])).unwrap(),
            expected
        );
        assert_eq!(
            parse_run_args(&strings(&["--dump-lcd", "out", "game.3ds"])).unwrap(),
            expected
        );
        assert_eq!(
            parse_run_args(&strings(&["game.3ds"])).unwrap(),
            options("game.3ds", None, false, 1)
        );
        assert_eq!(
            parse_run_args(&strings(&["--scale", "3", "--window", "game.3ds"])).unwrap(),
            options("game.3ds", None, true, 3)
        );
    }

    #[test]
    fn window_mode_allows_launching_without_a_game() {
        let parsed = parse_run_args(&strings(&["--window"])).unwrap();
        assert!(parsed.window && parsed.game.is_none());
        assert!(parse_run_args(&strings(&["--skip-intro"])).is_err());
    }

    #[test]
    fn games_directory_flag_is_parsed() {
        let parsed = parse_run_args(&strings(&["--window", "--games", "/g"])).unwrap();
        assert_eq!(parsed.games_dir, Some(PathBuf::from("/g")));
        assert!(parse_run_args(&strings(&["--window", "--games"])).is_err());
        let themed = parse_run_args(&strings(&[
            "--window",
            "--background-dir",
            "/wallpapers",
            "--background-fps",
            "30",
        ]))
        .unwrap();
        assert_eq!(themed.background_dir, Some(PathBuf::from("/wallpapers")));
        assert_eq!(themed.background_fps, 30);
        assert!(parse_run_args(&strings(&["--background-dir", "/wallpapers"])).is_err());
        assert!(parse_run_args(&strings(&["--window", "--background-fps", "0"])).is_err());
        assert!(parse_run_args(&strings(&["game.3ds", "--background-fps", "24"])).is_err());
    }

    #[test]
    fn skip_intro_flag_is_parsed() {
        let parsed = parse_run_args(&strings(&["g.3ds", "--window", "--skip-intro"])).unwrap();
        assert!(parsed.skip_intro && parsed.window);
        assert!(!parse_run_args(&strings(&["g.3ds"])).unwrap().skip_intro);
    }

    #[test]
    fn dump_intro_writes_valid_png_frames() {
        let dir = env::temp_dir().join(format!("aceris-intro-test-{}", std::process::id()));
        dump_intro(&dir).unwrap();
        for frame in INTRO_DUMP_FRAMES {
            let png = fs::read(dir.join(format!("intro_{frame:03}.png"))).unwrap();
            assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_arguments_reject_missing_values_extras_and_bad_scales() {
        assert!(parse_run_args(&strings(&["game.3ds", "--dump-lcd"])).is_err());
        assert!(parse_run_args(&strings(&["a.3ds", "b.3ds"])).is_err());
        assert!(parse_run_args(&strings(&["--dump-lcd", "out"])).is_err());
        assert!(parse_run_args(&strings(&["g", "--scale"])).is_err());
        for bad in ["0", "5", "x", "-1"] {
            assert!(
                parse_run_args(&strings(&["g", "--scale", bad])).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn dump_lcd_writes_three_valid_png_files() {
        let mut display = DualScreenDisplay::new().unwrap();
        display
            .surface_mut(aceris_core::video::ScreenTarget::Bottom)
            .clear([10, 20, 30, 255]);
        let dir = env::temp_dir().join(format!("aceris-lcd-dump-{}", std::process::id()));
        dump_lcd(&display, &dir).unwrap();
        for name in ["top_left.png", "top_right.png", "bottom.png"] {
            let bytes = fs::read(dir.join(name)).unwrap();
            assert_eq!(
                &bytes[..8],
                &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
            );
        }
        let bottom = fs::read(dir.join("bottom.png")).unwrap();
        assert_eq!(&bottom[16..24], &[0, 0, 1, 0x40, 0, 0, 0, 0xF0]); // 320x240
        fs::remove_dir_all(&dir).unwrap();
    }
}
