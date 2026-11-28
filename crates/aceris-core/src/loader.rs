//! Game image identification.
//!
//! This module recognises supported containers and performs structural
//! validation for NCSD cartridge images, CIA installable archives and standard
//! 3DSX homebrew executables before returning them to the system.

use aceris_common::{Error, Result};
use std::{fs::File, io::Read, path::Path};

pub mod cia;
pub mod ncsd;
/// Parser and RGB565 icon decoder for 3DS SMDH title metadata.
pub mod smdh;
/// Structural parser for 3DSX homebrew executables.
pub mod threedsx;

/// Bytes read from the start of a file. The CIA header is larger than NCSD.
const PROBE_LEN: usize = cia::HEADER_SIZE;

/// Offset of the NCSD/NCCH magic in cartridge images and content files.
const CONTAINER_MAGIC_OFFSET: usize = 0x100;

/// Value of the little-endian header-size field at the start of a CIA file.
const CIA_HEADER_SIZE: u32 = cia::HEADER_SIZE as u32;

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
/// NCSD, CIA and 3DSX images receive structural validation of their public
/// headers and declared byte ranges. Signature verification, decryption,
/// relocation and executable execution are not performed.
pub fn identify_file(path: &Path) -> Result<ImageFormat> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut header = Vec::with_capacity(PROBE_LEN);
    (&mut file)
        .take(PROBE_LEN as u64)
        .read_to_end(&mut header)?;

    let format = detect_format(&header).ok_or(Error::UnsupportedFormat)?;
    match format {
        ImageFormat::Ncsd => {
            ncsd::NcsdHeader::parse(&header, file_len)?;
        }
        ImageFormat::Cia => {
            cia::CiaHeader::parse(&header, file_len)?;
        }
        ImageFormat::ThreeDsx => {
            threedsx::ThreeDsxImage::parse_reader(&mut file)?;
        }
        _ => {}
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

    fn minimal_3dsx() -> Vec<u8> {
        let mut image = vec![0u8; 32];
        image[..4].copy_from_slice(b"3DSX");
        image[4..6].copy_from_slice(&32u16.to_le_bytes());
        image[6..8].copy_from_slice(&8u16.to_le_bytes());
        image[16..20].copy_from_slice(&4u32.to_le_bytes());
        image.extend_from_slice(&[0; 24]);
        image.extend_from_slice(&[0x00, 0x00, 0xA0, 0xE1]);
        image
    }

    #[test]
    fn identify_file_validates_a_standard_3dsx_container() {
        let path =
            std::env::temp_dir().join(format!("aceris-valid-homebrew-{}.3dsx", std::process::id()));
        std::fs::write(&path, minimal_3dsx()).unwrap();

        let result = identify_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.unwrap(), ImageFormat::ThreeDsx);
    }

    #[test]
    fn identify_file_rejects_truncated_3dsx_segments() {
        let path = std::env::temp_dir().join(format!(
            "aceris-truncated-homebrew-{}.3dsx",
            std::process::id()
        ));
        let mut image = minimal_3dsx();
        image.truncate(image.len() - 1);
        std::fs::write(&path, image).unwrap();

        let result = identify_file(&path);
        let _ = std::fs::remove_file(&path);

        assert!(matches!(result, Err(Error::Io(_))));
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let result = identify_file(Path::new("definitely/not/a/real/file.3ds"));
        assert!(matches!(result, Err(Error::Io(_))));
    }
}
