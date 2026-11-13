//! Types and utilities shared by every Aceris crate.

pub mod brand;
pub mod error;

pub use error::{Error, Result};

/// Project name.
pub const NAME: &str = "Aceris";

/// Project version, taken from the workspace `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
