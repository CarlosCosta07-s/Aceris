//! Top-level emulated system.

use crate::cpu::Arm11Core;
use crate::loader::{self, ImageFormat};
use aceris_common::{Error, Result};
use std::path::Path;
use tracing::info;

/// The emulated console. Front-ends create one of these, load a game and then
/// drive it one frame at a time.
#[derive(Debug, Default)]
pub struct System {
    cpu: Arm11Core,
    image: Option<ImageFormat>,
}

impl System {
    pub fn new() -> Self {
        Self::default()
    }

    /// Identifies and loads the game image at `path`.
    pub fn load(&mut self, path: &Path) -> Result<ImageFormat> {
        let format = loader::identify_file(path)?;
        info!(path = %path.display(), format = format.name(), "identified game image");
        self.image = Some(format);
        Ok(format)
    }

    pub fn image(&self) -> Option<ImageFormat> {
        self.image
    }

    pub fn cpu(&self) -> &Arm11Core {
        &self.cpu
    }

    /// Runs the system for one video frame.
    pub fn run_frame(&mut self) -> Result<()> {
        if self.image.is_none() {
            return Err(Error::NoImageLoaded);
        }
        Err(Error::Unimplemented("emulation loop"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_without_an_image_fails() {
        let mut system = System::new();
        assert!(matches!(system.run_frame(), Err(Error::NoImageLoaded)));
    }

    #[test]
    fn loading_a_missing_file_leaves_the_system_empty() {
        let mut system = System::new();
        assert!(system.load(Path::new("no/such/game.3ds")).is_err());
        assert_eq!(system.image(), None);
    }
}
