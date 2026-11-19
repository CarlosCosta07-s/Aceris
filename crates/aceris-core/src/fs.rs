//! Backing store for the HLE `fs:USER` service.
//!
//! Only the application's own RomFS (archive ID 3) is served, as the raw NCCH
//! RomFS region including its IVFC header, which is what libctru's `romfsInit`
//! reads. SD card, save data, extdata and system archives are not implemented.
//! Retail NCCH decryption is not performed: the RomFS must be cleartext.

use std::fmt;
use std::io::{self, Read, Seek, SeekFrom};

/// Object-safe combination of `Read + Seek`.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// The application RomFS region as an on-demand readable byte range.
pub struct RomFsRegion {
    reader: Box<dyn ReadSeek>,
    size: u64,
}

impl fmt::Debug for RomFsRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RomFsRegion")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl RomFsRegion {
    /// Wraps a reader whose byte zero is the start of the RomFS region.
    pub fn new(reader: Box<dyn ReadSeek>, size: u64) -> Self {
        Self { reader, size }
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Reads up to `len` bytes at `offset`; reads past the end are truncated.
    pub fn read_at(&mut self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if offset >= self.size {
            return Ok(Vec::new());
        }
        let available = usize::try_from(self.size - offset).unwrap_or(usize::MAX);
        let len = len.min(available);
        self.reader.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; len];
        self.reader.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_are_bounded_and_truncated_at_the_region_end() {
        let data: Vec<u8> = (0..16).collect();
        let mut region = RomFsRegion::new(Box::new(Cursor::new(data)), 10);
        assert_eq!(region.size(), 10);
        assert_eq!(region.read_at(2, 3).unwrap(), vec![2, 3, 4]);
        assert_eq!(region.read_at(8, 100).unwrap(), vec![8, 9]);
        assert!(region.read_at(10, 4).unwrap().is_empty());
    }
}
