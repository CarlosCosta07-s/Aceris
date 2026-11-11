//! Game image identification.
//!
//! This module recognises supported containers and performs structural
//! validation for NCSD cartridge images before returning them to the system.

use aceris_common::{Error, Result};
use std::{fs::File, io::Read, path::Path};

pub mod ncsd;

/// Bytes read from the start of a file. This covers the complete NCSD header.
const PROBE_LEN: usize = ncsd::HEADER_SIZE;

/// Offset of the NCSD/NCCH magic in cartridge images and content files.
const CONTAINER_MAGIC_OFFSET: usize = 0x100;

/// Value of the little-endian header-size field at the start of a CIA file.
const CIA_HEADER_SIZE: u32 = 0x2020;

/// Container formats Aceris can recognise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Cartridge image (.3ds, .cci).
    Ncsd,
    /// Executable content archive (.cxi, .app).
    Ncch,
    /// Installable title (.cia).
    Cia,
    /// Homebrew executable (.3dsx).
    ThreeDsx,
    /// ELF executable (.elf, .axf).
    Elf,
}

impl ImageFormat {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ncsd => "NCSD",
            Self::Ncch => "NCCH",
            Self::Cia => "CIA",
            Self::ThreeDsx => "3DSX",
            Self::Elf => "ELF",
        }
    }
}

/// Identifies the format of an image from its header bytes.
pub fn detect_format(data: &[u8]) -> Option<ImageFormat> {
    if data.starts_with(b"\x7fELF") {
        return Some(ImageFormat::Elf);
    }
    if data.starts_with(b"3DSX") {
        return Some(ImageFormat::ThreeDsx);
    }

    match data.get(CONTAINER_MAGIC_OFFSET..CONTAINER_MAGIC_OFFSET + 4) {
        Some(b"NCSD") => return Some(ImageFormat::Ncsd),
        Some(b"NCCH") => return Some(ImageFormat::Ncch),
        _ => {}
    }

    let header_size = data.first_chunk::<4>().map(|b| u32::from_le_bytes(*b));
    (header_size == Some(CIA_HEADER_SIZE)).then_some(ImageFormat::Cia)
}

/// Reads the start of the file at the given path and identifies its format.
///
/// NCSD images are additionally checked for a complete header and valid
/// partition ranges. Signature verification and decryption are not performed.
pub fn identify_file(path: &Path) -> Result<ImageFormat> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut header = Vec::with_capacity(PROBE_LEN);
    file.take(PROBE_LEN as u64).read_to_end(&mut header)?;

    let format = detect_format(&header).ok_or(Error::UnsupportedFormat)?;
    if format == ImageFormat::Ncsd {
        ncsd::NcsdHeader::parse(&header, file_len)?;
    }
    Ok(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_with(offset: usize, bytes: &[u8]) -> Vec<u8> {
        let mut data = vec![0u8; 0x200];
        data[offset..offset + bytes.len()].copy_from_slice(bytes);
        data
    }

    #[test]
    fn detects_magic_at_start_of_file() {
        assert_eq!(
            detect_format(&image_with(0, b"\x7fELF")),
            Some(ImageFormat::Elf)
        );
        assert_eq!(
            detect_format(&image_with(0, b"3DSX")),
            Some(ImageFormat::ThreeDsx)
        );
    }

    #[test]
    fn detects_container_magic_at_offset_0x100() {
        assert_eq!(
            detect_format(&image_with(0x100, b"NCSD")),
            Some(ImageFormat::Ncsd)
        );
        assert_eq!(
            detect_format(&image_with(0x100, b"NCCH")),
            Some(ImageFormat::Ncch)
        );
    }

    #[test]
    fn detects_cia_by_header_size() {
        let data = image_with(0, &CIA_HEADER_SIZE.to_le_bytes());
        assert_eq!(detect_format(&data), Some(ImageFormat::Cia));
    }

    #[test]
    fn rejects_unknown_and_truncated_data() {
        assert_eq!(detect_format(&[0u8; 0x200]), None);
        assert_eq!(detect_format(&[]), None);
        assert_eq!(detect_format(b"NC"), None);
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let result = identify_file(Path::new("definitely/not/a/real/file.3ds"));
        assert!(matches!(result, Err(Error::Io(_))));
    }
}
