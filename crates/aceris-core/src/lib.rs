//! Emulation core of Aceris.
//!
//! This crate is front-end agnostic: it contains no windowing, audio output or
//! UI code, so it can be shared by the desktop and Android front-ends.

pub mod cpu;
pub mod gpu;
pub mod loader;
pub mod memory;
pub mod selftest;
pub mod system;
pub mod video;

pub use aceris_common::{Error, Result};
pub use system::{DEFAULT_FRAME_INSTRUCTION_BUDGET, System};
