//! Host-directory storage behind the writable `fs:USER` archives.
//!
//! Save data, extdata, system save data and the SD card are each a directory
//! under one root chosen by the frontend. Guest paths are sanitised: only
//! plain relative components are accepted, so a guest cannot escape its archive
//! directory. This models *behaviour*, not the on-disk formats of the real
//! console (no DISA/DIFF images, no fixed-size save files, no quotas); the
//! result codes returned to the guest come from public tables and are not
//! verified on hardware.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Writable archive families (archive IDs from public 3dbrew tables).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArchiveKind {
    SaveData,
    ExtSaveData,
    SystemSaveData,
    Sdmc,
}

impl ArchiveKind {
    /// Maps an `OpenArchive` archive ID to a kind (`None` for unsupported IDs).
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            4 => Some(Self::SaveData),
            6 | 0x8000_0006 => Some(Self::ExtSaveData),
            8 | 0x8000_0008 => Some(Self::SystemSaveData),
            9 | 0x1234_567B | 0x1234_567C => Some(Self::Sdmc),
            _ => None,
        }
    }

    fn directory_name(self) -> &'static str {
        match self {
            Self::SaveData => "savedata",
            Self::ExtSaveData => "extdata",
            Self::SystemSaveData => "systemsavedata",
            Self::Sdmc => "sdmc",
        }
    }
}

/// Storage failures, mapped to 3DS result codes by the service layer.
#[derive(Debug)]
pub enum StorageError {
    NotFound,
    AlreadyExists,
    InvalidPath,
    NotEmpty,
    Io(io::Error),
}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::AlreadyExists => Self::AlreadyExists,
            _ => Self::Io(error),
        }
    }
}

/// One directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_directory: bool,
    pub size: u64,
}

/// Directory-backed storage rooted at a frontend-chosen path.
#[derive(Clone, Debug)]
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Creates the archive directory if needed.
    pub fn ensure_archive(&self, kind: ArchiveKind) -> Result<(), StorageError> {
        fs::create_dir_all(self.root.join(kind.directory_name()))?;
        Ok(())
    }

    fn resolve(&self, kind: ArchiveKind, path: &str) -> Result<PathBuf, StorageError> {
        let mut resolved = self.root.join(kind.directory_name());
        // Split on '/' only: the host's own separators ('\\' and drive or stream
        // markers such as ':') are rejected rather than interpreted.
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => return Err(StorageError::InvalidPath),
                _ if part.contains(['\\', ':', '\0']) => return Err(StorageError::InvalidPath),
                _ => resolved.push(part),
            }
        }
        Ok(resolved)
    }

    /// Opens a file. `read`/`write`/`create` follow the FS open-flag bits 0/1/2.
    pub fn open_file(
        &self,
        kind: ArchiveKind,
        path: &str,
        read: bool,
        write: bool,
        create: bool,
    ) -> Result<File, StorageError> {
        let resolved = self.resolve(kind, path)?;
        if resolved.is_dir() {
            return Err(StorageError::InvalidPath);
        }
        Ok(OpenOptions::new()
            .read(read || !write)
            .write(write)
            .create(create && write)
            .truncate(false)
            .open(resolved)?)
    }

    /// Creates a file of `size` zero bytes; fails if it already exists.
    pub fn create_file(
        &self,
        kind: ArchiveKind,
        path: &str,
        size: u64,
    ) -> Result<(), StorageError> {
        let resolved = self.resolve(kind, path)?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(resolved)?;
        file.set_len(size)?;
        Ok(())
    }

    pub fn delete_file(&self, kind: ArchiveKind, path: &str) -> Result<(), StorageError> {
        let resolved = self.resolve(kind, path)?;
        if resolved.is_dir() {
            return Err(StorageError::InvalidPath);
        }
        Ok(fs::remove_file(resolved)?)
    }

    pub fn create_directory(&self, kind: ArchiveKind, path: &str) -> Result<(), StorageError> {
        Ok(fs::create_dir(self.resolve(kind, path)?)?)
    }

    pub fn delete_directory(
        &self,
        kind: ArchiveKind,
        path: &str,
        recursive: bool,
    ) -> Result<(), StorageError> {
        let resolved = self.resolve(kind, path)?;
        if recursive {
            return Ok(fs::remove_dir_all(resolved)?);
        }
        fs::remove_dir(&resolved).map_err(|error| {
            if fs::read_dir(&resolved).is_ok_and(|mut entries| entries.next().is_some()) {
                StorageError::NotEmpty
            } else {
                error.into()
            }
        })
    }

    pub fn rename(&self, kind: ArchiveKind, from: &str, to: &str) -> Result<(), StorageError> {
        let (from, to) = (self.resolve(kind, from)?, self.resolve(kind, to)?);
        if to.exists() {
            return Err(StorageError::AlreadyExists);
        }
        Ok(fs::rename(from, to)?)
    }

    /// Lists a directory, directories first then files, each sorted by name.
    pub fn read_directory(
        &self,
        kind: ArchiveKind,
        path: &str,
    ) -> Result<Vec<DirEntry>, StorageError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.resolve(kind, path)?)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            entries.push(DirEntry {
                name,
                is_directory: metadata.is_dir(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
            });
        }
        entries.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    fn temp_storage(name: &str) -> Storage {
        let root =
            std::env::temp_dir().join(format!("aceris-storage-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let storage = Storage::new(root);
        storage.ensure_archive(ArchiveKind::SaveData).unwrap();
        storage
    }

    #[test]
    fn archive_ids_map_to_kinds() {
        assert_eq!(ArchiveKind::from_id(4), Some(ArchiveKind::SaveData));
        assert_eq!(ArchiveKind::from_id(9), Some(ArchiveKind::Sdmc));
        assert_eq!(
            ArchiveKind::from_id(0x8000_0006),
            Some(ArchiveKind::ExtSaveData)
        );
        assert_eq!(ArchiveKind::from_id(3), None, "RomFS is not host storage");
    }

    #[test]
    fn files_round_trip_and_create_is_exclusive() {
        let storage = temp_storage("files");
        let kind = ArchiveKind::SaveData;
        storage.create_file(kind, "/slot0.bin", 16).unwrap();
        assert!(matches!(
            storage.create_file(kind, "/slot0.bin", 16),
            Err(StorageError::AlreadyExists)
        ));
        let mut file = storage
            .open_file(kind, "/slot0.bin", true, true, false)
            .unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(b"data").unwrap();
        drop(file);
        let mut file = storage
            .open_file(kind, "slot0.bin", true, false, false)
            .unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[4..8], b"data");
        assert!(matches!(
            storage.open_file(kind, "/missing", true, false, false),
            Err(StorageError::NotFound)
        ));
        storage.open_file(kind, "/made", true, true, true).unwrap();
        storage.delete_file(kind, "/made").unwrap();
    }

    #[test]
    fn path_traversal_is_rejected() {
        let storage = temp_storage("paths");
        for path in ["/../escape", "a/../../b", "/a\\b", "C:/x", "/a:stream"] {
            assert!(
                matches!(
                    storage.open_file(ArchiveKind::SaveData, path, true, true, true),
                    Err(StorageError::InvalidPath)
                ),
                "{path}"
            );
        }
    }

    #[test]
    fn directories_list_sorted_and_delete_rules_apply() {
        let storage = temp_storage("dirs");
        let kind = ArchiveKind::SaveData;
        storage.create_directory(kind, "/b").unwrap();
        storage.create_directory(kind, "/a").unwrap();
        storage.create_file(kind, "/z.bin", 3).unwrap();
        storage.create_file(kind, "/b/inner", 1).unwrap();
        let names: Vec<_> = storage
            .read_directory(kind, "/")
            .unwrap()
            .into_iter()
            .map(|entry| (entry.name, entry.is_directory, entry.size))
            .collect();
        assert_eq!(
            names,
            vec![
                ("a".to_string(), true, 0),
                ("b".to_string(), true, 0),
                ("z.bin".to_string(), false, 3)
            ]
        );
        assert!(matches!(
            storage.delete_directory(kind, "/b", false),
            Err(StorageError::NotEmpty)
        ));
        storage.delete_directory(kind, "/b", true).unwrap();
        storage.delete_directory(kind, "/a", false).unwrap();
        storage.rename(kind, "/z.bin", "/y.bin").unwrap();
        assert!(matches!(
            storage.rename(kind, "/missing", "/q"),
            Err(StorageError::NotFound)
        ));
    }
}
