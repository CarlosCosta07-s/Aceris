//! Error type shared across the emulator.

/// Convenient result alias used throughout Aceris.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur while loading or running a game.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unrecognised or unsupported game image format")]
    UnsupportedFormat,

    #[error("no game image has been loaded")]
    NoImageLoaded,

    #[error("invalid memory access at {addr:#010x}")]
    InvalidAccess { addr: u32 },

    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
}
