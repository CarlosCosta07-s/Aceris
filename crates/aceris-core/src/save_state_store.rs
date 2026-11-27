//! Durable, identity-scoped save-state slots.
//!
//! Generations are immutable and are never overwritten. A save publishes a new
//! generation only after its bytes have been written and synchronized. Loading
//! scans generations from newest to oldest and falls back when a newer file is
//! incomplete or corrupt, so an interrupted save cannot destroy the previous
//! valid state. This is storage infrastructure only; it does not capture System.

use std::{
    cmp::Reverse,
    ffi::OsStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::save_state::{
    MAX_SAVE_STATE_PAYLOAD_BYTES, SAVE_STATE_HEADER_SIZE, SaveStateError, SaveStateFile,
    SaveStateIdentity,
};

/// Number of independently addressable save-state slots for each title.
pub const SAVE_STATE_SLOT_COUNT: u8 = 10;
/// Number of recent generations retained after a successful save.
pub const SAVE_STATE_RETAINED_GENERATIONS: usize = 3;
const SAVE_STATE_EXTENSION: &str = "acrs";

/// Filesystem-backed save-state storage rooted in the frontend's Aceris data directory.
#[derive(Clone, Debug)]
pub struct SaveStateStore {
    root: PathBuf,
}

impl SaveStateStore {
    /// Creates a store. The directory is created lazily on the first save.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Writes a new immutable generation and returns its generation number.
    ///
    /// Existing generations are never opened for writing. A partially written
    /// generation is ignored by the loader, while older generations remain
    /// available as recovery points.
    pub fn save(&self, slot: u8, state: &SaveStateFile) -> Result<u64, SaveStateStoreError> {
        validate_slot(slot)?;
        let encoded = state
            .encode()
            .map_err(SaveStateStoreError::InvalidSaveState)?;
        let directory = self.identity_directory(state.metadata().identity);
        fs::create_dir_all(&directory).map_err(map_io_error)?;

        let existing = slot_entries(&directory, slot)?;
        let mut generation = match existing.first() {
            Some(entry) => entry
                .generation
                .checked_add(1)
                .ok_or(SaveStateStoreError::GenerationExhausted)?,
            None => 1,
        };

        loop {
            let path = generation_path(&directory, slot, generation);
            let mut output = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    generation = generation
                        .checked_add(1)
                        .ok_or(SaveStateStoreError::GenerationExhausted)?;
                    continue;
                }
                Err(error) => return Err(map_io_error(error)),
            };

            let write_result = output.write_all(&encoded).and_then(|()| output.sync_all());
            drop(output);
            if let Err(error) = write_result {
                let _ = fs::remove_file(&path);
                return Err(map_io_error(error));
            }

            // Directory synchronization is best-effort because some supported
            // filesystems/platforms do not allow opening directories as files.
            sync_directory(&directory);
            prune_old_generations(&directory, slot);
            return Ok(generation);
        }
    }

    /// Loads the newest valid generation matching the exact title identity.
    ///
    /// Corrupt or interrupted newer generations are skipped. The first valid
    /// older generation is returned instead of failing the entire slot.
    pub fn load(
        &self,
        slot: u8,
        identity: SaveStateIdentity,
    ) -> Result<SaveStateFile, SaveStateStoreError> {
        validate_slot(slot)?;
        let directory = self.identity_directory(identity);
        let entries = slot_entries(&directory, slot)?;
        if entries.is_empty() {
            return Err(SaveStateStoreError::EmptySlot(slot));
        }

        let mut newest_error = None;
        for entry in entries {
            match read_generation(&entry.path, identity) {
                Ok(state) => return Ok(state),
                Err(error) => {
                    if newest_error.is_none() {
                        newest_error = Some(error);
                    }
                }
            }
        }

        Err(newest_error.unwrap_or(SaveStateStoreError::EmptySlot(slot)))
    }

    fn identity_directory(&self, identity: SaveStateIdentity) -> PathBuf {
        let mut digest = String::with_capacity(64);
        for byte in identity.content_sha256 {
            use fmt::Write as _;
            let _ = write!(&mut digest, "{byte:02x}");
        }

        let title = match identity.title_id {
            Some(title_id) => format!("title-{title_id:016x}"),
            None => "homebrew".to_owned(),
        };
        self.root.join(format!("{title}-{digest}"))
    }
}

fn validate_slot(slot: u8) -> Result<(), SaveStateStoreError> {
    if !(1..=SAVE_STATE_SLOT_COUNT).contains(&slot) {
        return Err(SaveStateStoreError::InvalidSlot(slot));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct SlotEntry {
    generation: u64,
    path: PathBuf,
}

fn slot_entries(directory: &Path, slot: u8) -> Result<Vec<SlotEntry>, SaveStateStoreError> {
    let read_directory = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(map_io_error(error)),
    };

    let mut entries = Vec::new();
    for entry in read_directory {
        let entry = entry.map_err(map_io_error)?;
        if !entry.file_type().map_err(map_io_error)?.is_file() {
            continue;
        }
        let Some(generation) = parse_generation(&entry.file_name(), slot) else {
            continue;
        };
        entries.push(SlotEntry {
            generation,
            path: entry.path(),
        });
    }

    entries.sort_unstable_by_key(|entry| Reverse(entry.generation));
    Ok(entries)
}

fn parse_generation(file_name: &OsStr, slot: u8) -> Option<u64> {
    let file_name = file_name.to_str()?;
    let prefix = format!("slot-{slot:02}-g");
    let generation = file_name.strip_prefix(&prefix)?.strip_suffix(".acrs")?;
    if generation.len() != 20 {
        return None;
    }
    generation.parse().ok()
}

fn generation_path(directory: &Path, slot: u8, generation: u64) -> PathBuf {
    directory.join(format!(
        "slot-{slot:02}-g{generation:020}.{SAVE_STATE_EXTENSION}"
    ))
}

fn read_generation(
    path: &Path,
    expected_identity: SaveStateIdentity,
) -> Result<SaveStateFile, SaveStateStoreError> {
    let file = File::open(path).map_err(map_io_error)?;
    let length = file.metadata().map_err(map_io_error)?.len();
    let max_file_size = SAVE_STATE_HEADER_SIZE as u64 + MAX_SAVE_STATE_PAYLOAD_BYTES as u64;
    if length > max_file_size {
        return Err(SaveStateStoreError::InvalidSaveState(
            SaveStateError::PayloadTooLarge(length.saturating_sub(SAVE_STATE_HEADER_SIZE as u64)),
        ));
    }

    let capacity = usize::try_from(length)
        .map_err(|_| SaveStateStoreError::InvalidSaveState(SaveStateError::InvalidLength))?;
    let mut encoded = Vec::new();
    encoded
        .try_reserve_exact(capacity)
        .map_err(|_| SaveStateStoreError::InvalidSaveState(SaveStateError::AllocationFailed))?;

    // Read at most one byte beyond the format limit. This also protects against
    // a file being extended after its metadata length was inspected.
    file.take(max_file_size + 1)
        .read_to_end(&mut encoded)
        .map_err(map_io_error)?;

    SaveStateFile::decode(&encoded, expected_identity)
        .map_err(SaveStateStoreError::InvalidSaveState)
}

fn prune_old_generations(directory: &Path, slot: u8) {
    let Ok(entries) = slot_entries(directory, slot) else {
        return;
    };
    for entry in entries.into_iter().skip(SAVE_STATE_RETAINED_GENERATIONS) {
        let _ = fs::remove_file(entry.path);
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) {
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) {}

fn map_io_error(error: io::Error) -> SaveStateStoreError {
    SaveStateStoreError::Io(error.kind())
}

/// Errors reported by the save-state slot store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStateStoreError {
    InvalidSlot(u8),
    EmptySlot(u8),
    GenerationExhausted,
    Io(io::ErrorKind),
    InvalidSaveState(SaveStateError),
}

impl fmt::Display for SaveStateStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot(slot) => write!(
                formatter,
                "save-state slot {slot} is outside the supported range 1..={SAVE_STATE_SLOT_COUNT}"
            ),
            Self::EmptySlot(slot) => write!(formatter, "save-state slot {slot} is empty"),
            Self::GenerationExhausted => {
                formatter.write_str("save-state generation counter is exhausted")
            }
            Self::Io(kind) => write!(formatter, "save-state storage I/O error: {kind:?}"),
            Self::InvalidSaveState(error) => write!(formatter, "invalid save state: {error}"),
        }
    }
}

impl std::error::Error for SaveStateStoreError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save_state::SaveStateMetadata;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory() -> PathBuf {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "aceris-save-state-store-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temporary directory should be created");
        path
    }

    fn identity(seed: u8) -> SaveStateIdentity {
        SaveStateIdentity {
            title_id: Some(0x0004_0000_1234_0000 + u64::from(seed)),
            content_sha256: [seed; 32],
        }
    }

    fn state(identity: SaveStateIdentity, frame_number: u64) -> SaveStateFile {
        SaveStateFile::new(
            SaveStateMetadata {
                identity,
                frame_number,
                virtual_time_ns: frame_number.saturating_mul(16_666_667),
            },
            vec![frame_number as u8, 0xA5, 0x5A],
        )
        .expect("test save state should be valid")
    }

    #[test]
    fn saves_and_loads_independent_slots() {
        let root = temporary_directory();
        let store = SaveStateStore::new(&root);
        let identity = identity(1);
        let first = state(identity, 10);
        let second = state(identity, 20);

        store.save(1, &first).expect("slot one should save");
        store.save(2, &second).expect("slot two should save");

        assert_eq!(store.load(1, identity).unwrap(), first);
        assert_eq!(store.load(2, identity).unwrap(), second);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn different_title_identities_use_separate_directories() {
        let root = temporary_directory();
        let store = SaveStateStore::new(&root);
        let first_identity = identity(2);
        let second_identity = identity(3);

        store
            .save(1, &state(first_identity, 1))
            .expect("first title should save");
        assert_eq!(
            store.load(1, second_identity),
            Err(SaveStateStoreError::EmptySlot(1))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_newest_generation_falls_back_to_previous_valid_state() {
        let root = temporary_directory();
        let store = SaveStateStore::new(&root);
        let identity = identity(4);
        store
            .save(1, &state(identity, 1))
            .expect("first generation should save");
        store
            .save(1, &state(identity, 2))
            .expect("second generation should save");

        let directory = store.identity_directory(identity);
        let entries = slot_entries(&directory, 1).unwrap();
        fs::write(&entries[0].path, b"corrupted")
            .expect("newest generation should be corrupted for the test");

        let loaded = store.load(1, identity).unwrap();
        assert_eq!(loaded.metadata().frame_number, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_generation_is_ignored_and_next_save_recovers() {
        let root = temporary_directory();
        let store = SaveStateStore::new(&root);
        let identity = identity(5);
        store
            .save(1, &state(identity, 1))
            .expect("first generation should save");

        let directory = store.identity_directory(identity);
        fs::write(generation_path(&directory, 1, 2), b"partial")
            .expect("partial generation should be created");
        assert_eq!(store.load(1, identity).unwrap().metadata().frame_number, 1);

        store
            .save(1, &state(identity, 3))
            .expect("next generation should save after the partial file");
        assert_eq!(store.load(1, identity).unwrap().metadata().frame_number, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn successful_saves_retain_only_the_three_newest_generations() {
        let root = temporary_directory();
        let store = SaveStateStore::new(&root);
        let identity = identity(6);
        for frame in 0..5 {
            store
                .save(1, &state(identity, frame))
                .expect("generation should save");
        }

        let directory = store.identity_directory(identity);
        let entries = slot_entries(&directory, 1).unwrap();
        assert_eq!(entries.len(), SAVE_STATE_RETAINED_GENERATIONS);
        assert_eq!(store.load(1, identity).unwrap().metadata().frame_number, 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_slots_are_rejected_before_filesystem_access() {
        let store = SaveStateStore::new("/path/that/need/not/exist");
        let identity = identity(7);
        assert_eq!(
            store.load(0, identity),
            Err(SaveStateStoreError::InvalidSlot(0))
        );
        assert_eq!(
            store.save(11, &state(identity, 0)),
            Err(SaveStateStoreError::InvalidSlot(11))
        );
    }

    #[test]
    fn missing_slot_is_reported_as_empty() {
        let root = temporary_directory();
        let store = SaveStateStore::new(&root);
        assert_eq!(
            store.load(1, identity(8)),
            Err(SaveStateStoreError::EmptySlot(1))
        );
        let _ = fs::remove_dir_all(root);
    }
}
