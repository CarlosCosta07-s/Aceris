//! Aceris desktop front-end.
//!
//! Currently a command-line shell around the core; the window, renderer and
//! input handling come later.

use aceris_common::{NAME, VERSION};
use aceris_core::{System, video::DualScreenDisplay};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn print_usage() {
    println!("{NAME} {VERSION}");
    println!();
    println!("USAGE:");
    println!("    aceris <GAME> [--dump-lcd <DIR>]");
    println!("    aceris --selftest");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help       Print this help");
    println!("    -V, --version    Print the version");
    println!("        --selftest   Run the built-in CPU self-test");
    println!("        --dump-lcd <DIR>");
    println!("                     After the frame, write top_left.png, top_right.png and");
    println!("                     bottom.png into DIR (created if missing)");
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

fn run(path: PathBuf, dump_dir: Option<PathBuf>) -> ExitCode {
    let mut system = System::new();

    if let Err(err) = system.load(&path) {
        eprintln!("error: could not load {}: {err}", path.display());
        return ExitCode::FAILURE;
    }

    if let Err(err) = system.run_frame() {
        eprintln!("error: emulation stopped: {err}");
        return ExitCode::FAILURE;
    }

    if let Some(dir) = dump_dir {
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

/// Parses `<GAME> [--dump-lcd <DIR>]` (the option may come first).
fn parse_run_args(args: &[String]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let mut game = None;
    let mut dump = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--dump-lcd" {
            let dir = iter
                .next()
                .ok_or_else(|| "--dump-lcd requires a directory".to_string())?;
            dump = Some(PathBuf::from(dir));
        } else if game.is_none() {
            game = Some(PathBuf::from(arg));
        } else {
            return Err(format!("unexpected argument: {arg}"));
        }
    }
    game.map(|game| (game, dump))
        .ok_or_else(|| "missing game path".to_string())
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
            Ok((game, dump)) => run(game, dump),
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

    #[test]
    fn run_arguments_accept_the_dump_option_in_either_position() {
        let expected = (PathBuf::from("game.3ds"), Some(PathBuf::from("out")));
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
            (PathBuf::from("game.3ds"), None)
        );
    }

    #[test]
    fn run_arguments_reject_missing_values_and_extras() {
        assert!(parse_run_args(&strings(&["game.3ds", "--dump-lcd"])).is_err());
        assert!(parse_run_args(&strings(&["a.3ds", "b.3ds"])).is_err());
        assert!(parse_run_args(&strings(&["--dump-lcd", "out"])).is_err());
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
