//! Top-level emulated system.

use crate::cpu::Arm11Core;
use crate::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use crate::loader::ncsd::session::{Arm11ProcessSession, Arm11RunReport, Arm11RunStopReason};
use crate::loader::{self, ImageFormat};
use aceris_common::{Error, Result};
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;

/// The emulated console. Front-ends create one of these, load a game and then
/// drive it one frame at a time.
#[derive(Debug, Default)]
pub struct System {
    cpu: Arm11Core,
    image: Option<ImageFormat>,
    ncsd_session: Option<Arm11ProcessSession>,
}

impl System {
    pub fn new() -> Self {
        Self::default()
    }

    /// Identifies a game image at `path` and records its validated container format.
    ///
    /// This does not prepare a CPU process. To prepare an
    /// encrypted NCSD cartridge for CPU execution, use
    /// [load_ncsd_file_with_keys] with caller-supplied NCCH keys.
    pub fn load(&mut self, path: &Path) -> Result<ImageFormat> {
        let format = loader::identify_file(path)?;
        self.image = Some(format);
        self.ncsd_session = None;
        self.cpu = Arm11Core::new();
        Ok(format)
    }

    /// Opens an NCSD cartridge image and initializes its ARM11 process session.
    ///
    /// This is the execution-loading path for NCSD cartridge images, including
    /// files conventionally named `.3ds` and `.cci`. The format is validated
    /// from its NCSD header rather than trusting the filename extension.
    ///
    /// The caller supplies menu-info and content keys. For NoCrypto NCCH
    /// partitions these values are unused; encrypted partitions require the
    /// correct keys. Aceris does not discover, embed, or persist them.
    pub fn load_ncsd_with_keys<R: Read + Seek>(
        &mut self,
        reader: R,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> Result<ImageFormat> {
        let session = Arm11ProcessSession::load_primary_ncsd(reader, menu_info_key, content_key)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not prepare the primary NCSD program: {error}"),
                ))
            })?;

        self.cpu = session.core().clone();
        self.image = Some(ImageFormat::Ncsd);
        self.ncsd_session = Some(session);
        Ok(ImageFormat::Ncsd)
    }

    /// Opens an `.3ds` or `.cci` file and prepares its primary ARM11 program.
    ///
    /// The extension is descriptive only: identify_file checks the NCSD magic
    /// and validates the declared partition ranges before loading.
    pub fn load_ncsd_file_with_keys(
        &mut self,
        path: &Path,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> Result<ImageFormat> {
        if loader::identify_file(path)? != ImageFormat::Ncsd {
            return Err(Error::UnsupportedFormat);
        }

        self.load_ncsd_with_keys(File::open(path)?, menu_info_key, content_key)
    }

    pub fn image(&self) -> Option<ImageFormat> {
        self.image
    }

    /// Returns the active ARM11 core, or the empty core before a process loads.
    pub fn cpu(&self) -> &Arm11Core {
        self.ncsd_session
            .as_ref()
            .map_or(&self.cpu, Arm11ProcessSession::core)
    }

    /// Returns the currently loaded NCSD process session, when available.
    pub const fn ncsd_session(&self) -> Option<&Arm11ProcessSession> {
        self.ncsd_session.as_ref()
    }

    /// Returns mutable access to the active NCSD process session.
    pub fn ncsd_session_mut(&mut self) -> Option<&mut Arm11ProcessSession> {
        self.ncsd_session.as_mut()
    }

    /// Executes up to `instruction_budget` instructions in the loaded ARM11 session.
    ///
    /// The report identifies budget exhaustion, unimplemented SVCs, process exit,
    /// and scheduler states without claiming that one instruction budget equals
    /// one video frame.
    pub fn run_instructions(&mut self, instruction_budget: usize) -> Result<Arm11RunReport> {
        let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
            "an NCSD ARM11 process session has not been prepared",
        ))?;
        let report = session.run_instructions(instruction_budget)?;
        self.cpu = session.core().clone();
        Ok(report)
    }

    /// Runs the system for one video frame.
    ///
    /// The ARM11 process can currently be stepped with run_instructions. A
    /// frame loop still requires the GPU, display, audio and timing pipeline.
    pub fn run_frame(&mut self) -> Result<()> {
        if self.image.is_none() {
            return Err(Error::NoImageLoaded);
        }
        Err(Error::Unimplemented(
            "frame scheduling and hardware presentation",
        ))
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

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn sha256_digest(data: &[u8]) -> [u8; 32] {
        let mut hasher = crate::loader::ncsd::ncch::romfs::sha256::Sha256::new();
        hasher.update(data).unwrap();
        hasher.finalize()
    }

    fn clear_ncsd_cartridge_fixture() -> Vec<u8> {
        const PARTITION_OFFSET: usize = 0x4000;
        const PARTITION_SIZE: usize = 0x8000;
        const EXEFS_OFFSET: usize = 0x2000;
        const EXEFS_SIZE: usize = 0x4000;
        const SEGMENT_SIZE: usize = 0x1000;

        let mut image = vec![0u8; PARTITION_OFFSET + PARTITION_SIZE];
        image[0x100..0x104].copy_from_slice(b"NCSD");
        let media_size_units = (image.len() / 0x200) as u32;
        write_u32(&mut image, 0x104, media_size_units);
        write_u32(&mut image, 0x120, 0x20);
        write_u32(&mut image, 0x124, 0x40);

        let ncch = PARTITION_OFFSET;
        image[ncch + 0x100..ncch + 0x104].copy_from_slice(b"NCCH");
        write_u32(&mut image, ncch + 0x104, 0x40);
        image[ncch + 0x18F] = 0x04; // NCCH NoCrypto.
        write_u32(&mut image, ncch + 0x180, 0x400);
        write_u32(&mut image, ncch + 0x1A0, 0x10);
        write_u32(&mut image, ncch + 0x1A4, 0x20);
        write_u32(&mut image, ncch + 0x1A8, 1);

        let mut extended_header = vec![0u8; 0x400];
        extended_header[..8].copy_from_slice(b"ACERIS!!");
        write_u32(&mut extended_header, 0x10, 0x0010_0000);
        write_u32(&mut extended_header, 0x14, 1);
        write_u32(&mut extended_header, 0x18, SEGMENT_SIZE as u32);
        write_u32(&mut extended_header, 0x1C, 0x4000);
        write_u32(&mut extended_header, 0x20, 0x0010_1000);
        write_u32(&mut extended_header, 0x24, 1);
        write_u32(&mut extended_header, 0x28, SEGMENT_SIZE as u32);
        write_u32(&mut extended_header, 0x30, 0x0010_2000);
        write_u32(&mut extended_header, 0x34, 2);
        write_u32(&mut extended_header, 0x38, SEGMENT_SIZE as u32);
        write_u32(&mut extended_header, 0x3C, SEGMENT_SIZE as u32);
        image[ncch + 0x160..ncch + 0x180].copy_from_slice(&sha256_digest(&extended_header));
        image[ncch + 0x200..ncch + 0x600].copy_from_slice(&extended_header);

        let mut code = vec![0x11; SEGMENT_SIZE];
        code[..4].copy_from_slice(&0xE3A0_0007u32.to_le_bytes()); // MOV r0, #7
        code[4..8].copy_from_slice(&0xE3A0_1005u32.to_le_bytes()); // MOV r1, #5
        let code_size = code.len();
        code.resize(code_size + SEGMENT_SIZE, 0x22);
        let code_size = code.len();
        code.resize(code_size + SEGMENT_SIZE, 0x33);

        let mut exefs = vec![0u8; EXEFS_SIZE];
        exefs[..5].copy_from_slice(b".code");
        write_u32(&mut exefs, 12, code.len() as u32);
        exefs[0x1E0..0x200].copy_from_slice(&sha256_digest(&code));
        exefs[0x200..0x200 + code.len()].copy_from_slice(&code);
        image[ncch + 0x1C0..ncch + 0x1E0].copy_from_slice(&sha256_digest(&exefs[..0x200]));
        let exefs_start = ncch + EXEFS_OFFSET;
        image[exefs_start..exefs_start + EXEFS_SIZE].copy_from_slice(&exefs);

        image
    }

    #[test]
    fn loads_and_steps_ncsd_cartridges_from_both_3ds_and_cci_paths() {
        for extension in ["3ds", "cci"] {
            let path = std::env::temp_dir()
                .join(format!("aceris-system-{}.{extension}", std::process::id()));
            std::fs::write(&path, clear_ncsd_cartridge_fixture()).unwrap();

            let mut system = System::new();
            let result = system.load_ncsd_file_with_keys(&path, [0; 16], [0; 16]);
            let _ = std::fs::remove_file(&path);
            assert_eq!(result.unwrap(), ImageFormat::Ncsd);
            assert_eq!(system.cpu().pc(), 0x0010_0000);
            assert!(system.ncsd_session().is_some());

            let report = system.run_instructions(2).unwrap();
            assert_eq!(report.instructions_executed, 2);
            assert_eq!(
                report.stop_reason,
                Arm11RunStopReason::InstructionBudgetExhausted
            );
            assert_eq!(system.cpu().regs[0], 7);
            assert_eq!(system.cpu().regs[1], 5);
        }
    }

    #[test]
    fn failed_ncsd_loading_does_not_replace_the_current_image() {
        let mut system = System::new();
        assert!(
            system
                .load_ncsd_with_keys(std::io::Cursor::new(vec![0; 0x200]), [0; 16], [0; 16])
                .is_err()
        );
        assert_eq!(system.image(), None);
        assert!(system.ncsd_session().is_none());
    }

    #[test]
    fn loading_a_missing_file_leaves_the_system_empty() {
        let mut system = System::new();
        assert!(system.load(Path::new("no/such/game.3ds")).is_err());
        assert_eq!(system.image(), None);
    }
}
