//! Aceris desktop front-end.
//!
//! Currently a command-line shell around the core; the window, renderer and
//! input handling come later.

use aceris_common::{NAME, VERSION};
use aceris_core::System;
use std::{env, path::PathBuf, process::ExitCode};

fn print_usage() {
    println!("{NAME} {VERSION}");
    println!();
    println!("USAGE:");
    println!("    aceris <GAME>");
    println!("    aceris --selftest");
    println!();
    println!("OPTIONS:");
    println!("    -h, --help       Print this help");
    println!("    -V, --version    Print the version");
    println!("        --selftest   Run the built-in CPU self-test");
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

fn run(path: PathBuf) -> ExitCode {
    let mut system = System::new();

    if let Err(err) = system.load(&path) {
        eprintln!("error: could not load {}: {err}", path.display());
        return ExitCode::FAILURE;
    }

    if let Err(err) = system.run_frame() {
        eprintln!("error: emulation stopped: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some("-h" | "--help") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("{NAME} {VERSION}");
            ExitCode::SUCCESS
        }
        Some("--selftest") => selftest(),
        Some(path) => run(PathBuf::from(path)),
        None => {
            print_usage();
            ExitCode::FAILURE
        }
    }
}
