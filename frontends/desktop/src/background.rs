//! Desktop loader for user-provided Aceris animated library backgrounds.
//!
//! A sequence is a directory of tightly-packed 400x480 RGBA8 frames named
//! frame_000.rgba, frame_001.rgba, ... . Every frame is validated before the
//! library opens, preventing partial animations and invalid reads.

use std::{
    fs,
    path::{Path, PathBuf},
};

use aceris_core::video::library::{HEIGHT, WIDTH};

const FRAME_BYTES: usize = WIDTH * HEIGHT * 4;
const MAX_FRAMES: usize = 120;

/// Validated loopable background sequence, sampled at a fixed source FPS.
pub struct Sequence {
    frames: Vec<Vec<u8>>,
    fps: u32,
}

impl Sequence {
    pub fn load(directory: &Path, fps: u32) -> Result<Self, String> {
        if !(1..=60).contains(&fps) {
            return Err(format!(
                "background FPS must be between 1 and 60, got {fps}"
            ));
        }
        let read = fs::read_dir(directory).map_err(|error| {
            format!(
                "could not read background directory {}: {error}",
                directory.display()
            )
        })?;
        let mut paths: Vec<PathBuf> = read
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("rgba"))
            })
            .collect();
        paths.sort_by_key(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
        if paths.is_empty() {
            return Err(format!(
                "no .rgba frames found in {}; expected 400x480 RGBA8 frames",
                directory.display()
            ));
        }
        if paths.len() > MAX_FRAMES {
            return Err(format!(
                "background has {} frames; the limit is {MAX_FRAMES}",
                paths.len()
            ));
        }

        let mut frames = Vec::with_capacity(paths.len());
        for path in paths {
            let frame = fs::read(&path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))?;
            if frame.len() != FRAME_BYTES {
                return Err(format!(
                    "{} has {} bytes; a 400x480 RGBA8 frame must have {FRAME_BYTES}",
                    path.display(),
                    frame.len()
                ));
            }
            frames.push(frame);
        }
        Ok(Self { frames, fps })
    }

    /// Selects the frame corresponding to a 60 Hz host animation counter.
    pub fn frame_at(&self, host_frame: u64) -> Option<&[u8]> {
        if self.frames.is_empty() {
            return None;
        }
        let source_frame = host_frame.saturating_mul(u64::from(self.fps)) / 60;
        let index = (source_frame % self.frames.len() as u64) as usize;
        self.frames.get(index).map(Vec::as_slice)
    }

    #[cfg(test)]
    fn frame_count(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_validates_frames_and_loops_at_requested_rate() {
        let directory = std::env::temp_dir().join(format!("aceris-bg-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("frame_001.rgba"), vec![2u8; FRAME_BYTES]).unwrap();
        fs::write(directory.join("frame_000.rgba"), vec![1u8; FRAME_BYTES]).unwrap();
        fs::write(directory.join("ignore.txt"), b"ignored").unwrap();

        let sequence = Sequence::load(&directory, 30).unwrap();
        assert_eq!(sequence.frame_count(), 2);
        assert_eq!(sequence.frame_at(0).unwrap()[0], 1);
        assert_eq!(sequence.frame_at(2).unwrap()[0], 2);
        assert_eq!(sequence.frame_at(4).unwrap()[0], 1);

        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn sequence_rejects_wrong_sized_frames_and_invalid_rates() {
        let directory = std::env::temp_dir().join(format!("aceris-bg-bad-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("broken.rgba"), b"not a frame").unwrap();
        assert!(Sequence::load(&directory, 24).is_err());
        assert!(Sequence::load(&directory, 0).is_err());
        fs::remove_dir_all(&directory).unwrap();
    }
}
