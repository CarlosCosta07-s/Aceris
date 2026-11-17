//! Standalone Android ARM64 performance runner.
//!
//! The same benchmark implementation is shared with the desktop example.
//! Deploy the resulting executable with ADB and run it against a title stored
//! locally on the device. No Android Activity or UI is required for throughput
//! measurements.

#[path = "../../../../crates/aceris-core/examples/benchmark.rs"]
#[allow(dead_code)]
mod benchmark_cli;

fn main() -> std::process::ExitCode {
    benchmark_cli::run_cli()
}
