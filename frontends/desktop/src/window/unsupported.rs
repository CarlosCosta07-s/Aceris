//! Placeholder backend for platforms without a window implementation.

use super::{WindowError, WindowEvent};

/// A window that can never be opened on this platform.
pub struct Window {
    _private: (),
}

impl Window {
    pub fn open(_title: &str, _width: usize, _height: usize) -> Result<Self, WindowError> {
        Err(WindowError::Unsupported)
    }

    pub fn present(&mut self, _bgrx: &[u8]) -> Result<(), WindowError> {
        Err(WindowError::Unsupported)
    }

    pub fn poll_events(&mut self) -> Vec<WindowEvent> {
        Vec::new()
    }
}
