//! Aceris desktop front-end.
//!
//! Currently a command-line shell around the core; the window, renderer and
//! input handling come later.

mod window;

use aceris_common::{NAME, VERSION};
use aceris_core::{System, video::DualScreenDisplay};
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
    println!("    aceris <GAME> [--dump-lcd <DIR>] [--window [--scale <N>]]");
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

/// Runs frames at roughly 60 Hz in a window until it is closed.
fn run_windowed(system: &mut System, scale: usize) -> Result<(), String> {
    let mut window = window::Window::open(
        &format!("{NAME} {VERSION}"),
        window::CANVAS_WIDTH * scale,
        window::CANVAS_HEIGHT * scale,
    )
    .map_err(|err| format!("could not open a window: {err}"))?;
    let frame_time = Duration::from_micros(16_667);
    loop {
        let started = Instant::now();
        for event in window.poll_events() {
            if event == window::WindowEvent::CloseRequested {
                return Ok(());
            }
        }
        system
            .run_frame()
            .map_err(|err| format!("emulation stopped: {err}"))?;
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

    if let Err(err) = system.load(&options.game) {
        eprintln!("error: could not load {}: {err}", options.game.display());
        return ExitCode::FAILURE;
    }

    if options.window {
        if let Err(message) = run_windowed(&mut system, options.scale) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    } else if let Err(err) = system.run_frame() {
        eprintln!("error: emulation stopped: {err}");
        return ExitCode::FAILURE;
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
    game: PathBuf,
    dump_lcd: Option<PathBuf>,
    window: bool,
    scale: usize,
}

/// Parses `<GAME> [--dump-lcd <DIR>] [--window] [--scale <N>]` in any order.
fn parse_run_args(args: &[String]) -> Result<RunOptions, String> {
    let mut game = None;
    let mut dump_lcd = None;
    let mut window = false;
    let mut scale = 2;
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
    Ok(RunOptions {
        game: game.ok_or_else(|| "missing game path".to_string())?,
        dump_lcd,
        window,
        scale,
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
        Some(_) => match parse_run_args(&args) {
            Ok(options) => run(options),
            Err(message) => {
                eprintln!("error: {message}");
                ExitCode::FAILURE
            }
        },
        None => {
            print_usage();
            ExitCode::FAILURE
        }
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
            game: PathBuf::from(game),
            dump_lcd: dump.map(PathBuf::from),
            window,
            scale,
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
