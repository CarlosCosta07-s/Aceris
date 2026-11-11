//! Aceris desktop front-end.
//!
//! Currently a command-line shell around the core; the window, renderer and
//! input handling come later.

use aceris_common::{NAME, VERSION};
use aceris_core::System;
use std::{env, path::PathBuf, process::ExitCode};
use tracing::error;
use tracing_subscriber::EnvFilter;

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
    println!();
    println!("Set RUST_LOG (for example RUST_LOG=debug) to change the log level.");
}

fn selftest() -> ExitCode {
    match aceris_core::selftest::run() {
        Ok(value) => {
            println!("CPU self-test passed (result: {value})");
            ExitCode::SUCCESS
        }
        Err(err) => {
            error!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run(path: PathBuf) -> ExitCode {
    let mut system = System::new();

    if let Err(err) = system.load(&path) {
        error!("could not load {}: {err}", path.display());
        return ExitCode::FAILURE;
    }

    if let Err(err) = system.run_frame() {
        error!("emulation stopped: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

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
