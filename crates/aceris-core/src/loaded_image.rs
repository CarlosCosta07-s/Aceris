//! Loaded-image metadata shared by the desktop and Android frontends.
//!
//! A title ID and a content fingerprint serve different purposes: the former
//! groups per-title preferences, while the latter isolates save states to the
//! exact bytes of the image that was loaded.

use std::{
    fmt,
    path::Path,
};

use aceris_common::Error as CoreError;

use crate::{
    game_identity::GameIdentity,
    loader::ImageFormat,
    save_state::SaveStateIdentity,
    save_state_identity::{SaveStateIdentityError, identity_from_image_path},
    system::System,
};

/// Identity metadata produced after a supported executable has loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedImageIdentity {
    /// Container format recognized by the loader.
    pub format: ImageFormat,
    /// Validated title identity, when the container provides one.
    pub game_identity: Option<GameIdentity>,
    /// Exact full-file identity used to isolate save-state slots.
    pub save_state_identity: SaveStateIdentity,
}

/// Loads an executable and calculates the identity of the selected image.
///
/// The fingerprint is computed only after the existing loader has accepted
/// the image. It covers the complete file, not only the boot partition, and
/// uses the validated Title ID when available. 3DSX homebrew therefore gets a
/// stable content identity even though it has no validated Title ID.
///
/// The caller must keep the selected file unchanged while it is being loaded.
pub fn load_executable_file_with_identity(
    system: &mut System,
    path: &Path,
) -> Result<LoadedImageIdentity, LoadIdentifiedImageError> {
    let format = system
        .load_executable_file(path)
        .map_err(LoadIdentifiedImageError::Load)?;
    let game_identity = system.game_identity();
    let save_state_identity = identity_from_image_path(
        path,
        game_identity.map(GameIdentity::title_id),
    )
    .map_err(LoadIdentifiedImageError::Identity)?;

    Ok(LoadedImageIdentity {
        format,
        game_identity,
        save_state_identity,
    })
}

/// Failure while loading an executable and associating its content identity.
#[derive(Debug)]
pub enum LoadIdentifiedImageError {
    Load(CoreError),
    Identity(SaveStateIdentityError),
}

impl fmt::Display for LoadIdentifiedImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(error) => write!(formatter, "executable load failed: {error}"),
            Self::Identity(error) => {
                write!(formatter, "could not identify loaded image for save states: {error}")
            }
        }
    }
}

impl std::error::Error for LoadIdentifiedImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(error) => Some(error),
            Self::Identity(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn temporary_image_path() -> std::path::PathBuf {
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "aceris-loaded-image-{}-{sequence}.3dsx",
            std::process::id()
        ))
    }

    #[test]
    fn homebrew_identity_uses_exact_image_bytes_without_a_title_id() {
        let path = temporary_image_path();
        let bytes = b"synthetic homebrew image";
        fs::write(&path, bytes).expect("synthetic image should be written");

        let loaded = LoadedImageIdentity {
            format: ImageFormat::ThreeDsx,
            game_identity: None,
            save_state_identity: identity_from_image_path(&path, None)
                .expect("image identity should be calculated"),
        };

        assert_eq!(loaded.format, ImageFormat::ThreeDsx);
        assert_eq!(loaded.game_identity, None);
        assert_eq!(
            loaded.save_state_identity.content_sha256,
            identity_from_image_path(&path, None)
                .expect("same image should retain its fingerprint")
                .content_sha256
        );
        assert_eq!(loaded.save_state_identity.title_id, None);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn retail_identity_keeps_title_id_separate_from_content_hash() {
        let path = temporary_image_path();
        fs::write(&path, b"synthetic retail image").expect("synthetic image should be written");
        let game_identity = GameIdentity::from_title_id(0x0004_0000_1234_5600);
        let identity = identity_from_image_path(
            &path,
            game_identity.map(GameIdentity::title_id),
        )
        .expect("retail image identity should be calculated");

        assert_eq!(identity.title_id, Some(0x0004_0000_1234_5600));
        assert_ne!(identity.content_sha256, [0; 32]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn changing_image_bytes_changes_save_state_identity_even_when_title_id_matches() {
        let path = temporary_image_path();
        fs::write(&path, b"first revision").expect("first image should be written");
        let title_id = Some(0x0004_0000_1234_5600);
        let first = identity_from_image_path(&path, title_id)
            .expect("first identity should be calculated");

        fs::write(&path, b"second revision").expect("second image should be written");
        let second = identity_from_image_path(&path, title_id)
            .expect("second identity should be calculated");

        assert_eq!(first.title_id, second.title_id);
        assert_ne!(first.content_sha256, second.content_sha256);
        let _ = fs::remove_file(path);
    }
}
