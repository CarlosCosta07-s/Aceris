//! Aceris desktop front-end.
//!
//! Provides command-line diagnostics and an experimental X11 window frontend.
//! The library uses a dedicated home canvas; gameplay always places the two
//! LCDs side by side and resizes the window when entering or leaving a game.

mod background;
mod window;

use aceris_common::{NAME, VERSION};
use aceris_core::{
    DEFAULT_FRAME_INSTRUCTION_BUDGET, System,
    video::{
        DualScreenDisplay, intro,
        library::{ClickResult, Library, VISIBLE_ROWS, scan_games},
    },
};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

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
    println!("        --window     Show both LCDs in a window and run frames until it is");
    println!("                     closed (Linux/X11 only for now)");
    println!("        --scale <N>  Integer window scale factor, 1-4 (default 2)");
    println!("        --games <DIR>");
    println!("        --background-dir <DIR>");
    println!("                     Folder of 400x480 .rgba frames for an animated home background");
    println!("        --background-fps <N>");
    println!("                     Source rate for background frames, 1-60 (default 24)");
    println!("                     Folder with .3ds/.cci games for the library (default:");
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

/// Loads the same NoCrypto NCSD execution path exposed by the Android frontend.
fn load_game(system: &mut System, game: &Path) -> Result<(), String> {
    let format = system
        .load_ncsd_no_crypto_file(game)
        .map_err(|err| format!("could not prepare {}: {err}", game.display()))?;

    println!(
        "Cartridge prepared: {format:?}; ARM11 entry PC={:#010X}, CPSR={:#010X}",
        system.cpu().pc(),
        system.cpu().cpsr.0
    );
    Ok(())
}

/// Runs one bounded frame and prints the first frame's CPU/GPU/LCD diagnostics.
fn run_frame_with_diagnostics(system: &mut System) -> Result<(), String> {
    let report = system
        .run_frame_with_budget(DEFAULT_FRAME_INSTRUCTION_BUDGET)
        .map_err(|err| format!("emulation stopped: {err}"))?;

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
const INTRO_DUMP_FRAMES: [u32; 8] = [0, 20, 36, 50, 70, 90, 115, 149];

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

/// Plays the start-up animation; returns `Ok(false)` if the window was closed.
fn play_intro(window: &mut window::Window, scale: usize) -> Result<bool, String> {
    let frame_time = Duration::from_micros(1_000_000 / u64::from(intro::FPS));
    for frame in 0..intro::FRAME_COUNT {
        let started = Instant::now();
        for event in window.poll_events() {
            if event == window::WindowEvent::CloseRequested {
                return Ok(false);
            }
        }
        let rgba = intro::render_frame(frame, window::CANVAS_WIDTH, window::CANVAS_HEIGHT);
        let bgrx = intro::rgba_to_scaled_bgrx(&rgba, window::CANVAS_WIDTH, scale);
        window
            .present(&bgrx)
            .map_err(|err| format!("could not draw the frame: {err}"))?;
        if let Some(rest) = frame_time.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }
    Ok(true)
}

/// How a stretch of emulation or browsing ended.
enum Flow {
    Quit,
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
    let mut window = window::Window::open(
        &format!("{NAME} {VERSION}"),
        window::CANVAS_WIDTH * scale,
        window::CANVAS_HEIGHT * scale,
    )
    .map_err(|err| format!("could not open a window: {err}"))?;
    if !skip_intro && !play_intro(&mut window, scale)? {
        return Ok(());
    }
    let mut library = Library::new(scan_games(games_dir), games_dir.display().to_string());
    let background = background_dir
        .map(|directory| background::Sequence::load(directory, background_fps))
        .transpose()?;
    let mut next = game.map(Path::to_path_buf);
    loop {
        let path = match next.take() {
            Some(path) => path,
            None => match browse_library(&mut window, &mut library, scale, background.as_ref())? {
                Some(path) => path,
                None => return Ok(()),
            },
        };

        // Desktop gameplay always presents the two LCDs horizontally:
        // top LCD on the left, bottom LCD on the right.
        window
            .resize(
                window::GAME_CANVAS_WIDTH * scale,
                window::GAME_CANVAS_HEIGHT * scale,
            )
            .map_err(|err| format!("could not resize the gameplay window: {err}"))?;

        if let Err(message) = load_game(system, &path) {
            window
                .resize(window::CANVAS_WIDTH * scale, window::CANVAS_HEIGHT * scale)
                .map_err(|err| format!("could not restore the library window: {err}"))?;
            if game == Some(path.as_path()) && library.entries().is_empty() {
                return Err(message);
            }
            library.set_status(message);
            continue;
        }
        match emulate(system, &mut window, scale) {
            Ok(Flow::Quit) => return Ok(()),
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
    library: &mut Library,
    scale: usize,
    background: Option<&background::Sequence>,
) -> Result<Option<PathBuf>, String> {
    let frame_time = Duration::from_micros(16_667);
    let mut animation_frame = 0u64;
    loop {
        for event in window.poll_events() {
            let mut activate = false;
            match event {
                window::WindowEvent::CloseRequested => return Ok(None),
                window::WindowEvent::Redraw => {}
                window::WindowEvent::Key(window::Key::Escape) => return Ok(None),
                window::WindowEvent::Key(window::Key::Up) => library.move_selection(-1),
                window::WindowEvent::Key(window::Key::Down) => library.move_selection(1),
                window::WindowEvent::Key(window::Key::PageUp) => {
                    library.move_selection(-(VISIBLE_ROWS as i32))
                }
                window::WindowEvent::Key(window::Key::PageDown) => {
                    library.move_selection(VISIBLE_ROWS as i32)
                }
                window::WindowEvent::Key(window::Key::Enter) => activate = true,
                window::WindowEvent::Scroll(rows) => library.move_selection(rows * 3),
                window::WindowEvent::Click { x, y } => {
                    let (x, y) = (x / scale as i32, y / scale as i32);
                    activate = library.click(x, y) == ClickResult::Activated;
                }
            }
            if activate {
                if let Some(entry) = library.selected_entry() {
                    return Ok(Some(entry.path.clone()));
                }
            }
        }
        let started = Instant::now();
        let background_frame = background.and_then(|sequence| sequence.frame_at(animation_frame));
        let rgba = library.render_animated(animation_frame, background_frame);
        let bgrx = intro::rgba_to_scaled_bgrx(&rgba, window::CANVAS_WIDTH, scale);
        window
            .present(&bgrx)
            .map_err(|err| format!("could not draw the frame: {err}"))?;
        animation_frame = animation_frame.wrapping_add(1);
        if let Some(rest) = frame_time.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }
}

/// Runs frames at roughly 60 Hz until the window closes or Esc is pressed.
fn emulate(system: &mut System, window: &mut window::Window, scale: usize) -> Result<Flow, String> {
    let frame_time = Duration::from_micros(16_667);
    loop {
        let started = Instant::now();
        for event in window.poll_events() {
            match event {
                window::WindowEvent::CloseRequested => return Ok(Flow::Quit),
                window::WindowEvent::Key(window::Key::Escape) => return Ok(Flow::BackToLibrary),
                _ => {}
            }
        }
        run_frame_with_diagnostics(system)?;
        window
            .present(&window::compose_bgrx(system.display(), scale))
            .map_err(|err| format!("could not draw the frame: {err}"))?;
        if let Some(rest) = frame_time.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }
}

fn run(options: RunOptions) -> ExitCode {
    let mut system = System::new();

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
        if let Err(message) = load_game(&mut system, game) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
        if let Err(message) = run_frame_with_diagnostics(&mut system) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
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
}

/// Parses `<GAME> [--dump-lcd <DIR>] [--window] [--scale <N>]` in any order.
fn parse_run_args(args: &[String]) -> Result<RunOptions, String> {
    let mut game = None;
    let mut dump_lcd = None;
    let mut window = false;
    let mut scale = 2;
    let mut skip_intro = false;
    let mut games_dir = None;
    let mut background_dir = None;
    let mut background_fps = 24;
    let mut background_fps_configured = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dump-lcd" => {
                let dir = iter
                    .next()
                    .ok_or_else(|| "--dump-lcd requires a directory".to_string())?;
                dump_lcd = Some(PathBuf::from(dir));
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
        }
    }

    #[test]
    fn run_arguments_accept_options_in_any_order() {
        let expected = options("game.3ds", Some("out"), false, 2);
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
            options("game.3ds", None, false, 2)
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
