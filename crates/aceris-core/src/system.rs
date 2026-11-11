//! Top-level emulated system.

use crate::cpu::Arm11Core;
use crate::gpu::Pica200;
use crate::video::DualScreenDisplay;
use crate::loader::ncsd::session::Arm11RunStopReason;
use crate::loader::ncsd::ncch::{self, crypto::AES_BLOCK_SIZE};
use crate::loader::ncsd::{Arm11ProcessSession, Arm11RunReport, NcsdHeader, NcsdImageReader};
use crate::loader::{self, ImageFormat};
use aceris_common::{Error, Result};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// The emulated console. Front-ends create one of these, load a game and then
/// drive it one frame at a time.
#[derive(Debug, Default)]
pub struct System {
    cpu: Arm11Core,
    image: Option<ImageFormat>,
    ncsd_session: Option<Arm11ProcessSession>,
    gpu: Pica200,
    display: DualScreenDisplay,
}

/// Nominal 60 Hz development-frame duration in nanoseconds.
pub const NOMINAL_FRAME_DURATION_NS: u64 = 16_666_667;

/// Deterministic instruction cap used by System::run_frame.
pub const DEFAULT_FRAME_INSTRUCTION_BUDGET: usize = 100_000;

/// Result of one completed development frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemFrameReport {
    pub frame_number: u64,
    pub instructions_executed: u64,
    pub cpu_stop_reason: Arm11RunStopReason,
    pub elapsed_virtual_time_ns: u64,
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

    /// Inspects and returns the validated public header of an NCSD cartridge.
    ///
    /// This accepts cartridge files commonly named `.3ds` and `.cci`; the
    /// NCSD magic and partition ranges are validated from file contents, not
    /// inferred from the extension. Inspection does not verify signatures,
    /// decrypt partitions, or prepare an executable process.
    pub fn inspect_ncsd_file(path: &Path) -> Result<NcsdHeader> {
        let image = NcsdImageReader::open(File::open(path)?).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not inspect NCSD cartridge: {error}"),
            ))
        })?;

        Ok(image.header().clone())
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

    /// Loads a NoCrypto NCSD cartridge from one seekable reader.
    ///
    /// NCSD and primary NCCH headers are structurally validated on this same
    /// reader before the reader is rewound and passed to the ARM11 loader.
    /// Encrypted cartridges are rejected without changing the current system;
    /// use [load_ncsd_with_keys] with caller-supplied keys for those images.
    pub fn load_ncsd_no_crypto_reader<R: Read + Seek>(
        &mut self,
        mut reader: R,
    ) -> Result<ImageFormat> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let start = reader.seek(SeekFrom::Start(0))?;
        if start != 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "reader did not seek to the beginning of the NCSD image",
            )));
        }

        let mut ncsd_header_bytes = [0; crate::loader::ncsd::HEADER_SIZE];
        reader.read_exact(&mut ncsd_header_bytes)?;
        let ncsd_header = NcsdHeader::parse(&ncsd_header_bytes, file_len)?;
        let primary_partition =
            *ncsd_header
                .primary_partition()
                .ok_or(Error::InvalidNcsdPartition {
                    index: 0,
                    reason: "primary partition is missing",
                })?;

        let partition_position = reader.seek(SeekFrom::Start(primary_partition.offset_bytes))?;
        if partition_position != primary_partition.offset_bytes {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "reader did not seek to the primary NCSD partition",
            )));
        }

        let mut ncch_header_bytes = [0; ncch::HEADER_SIZE];
        reader.read_exact(&mut ncch_header_bytes)?;
        let ncch_header = ncch::NcchHeader::parse(&ncch_header_bytes, primary_partition.size_bytes)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid primary NCCH header: {error}"),
                ))
            })?;

        if !ncch_header.declares_no_crypto() {
            return Err(Error::Unimplemented(
                "encrypted NCCH content requires caller-supplied keys; use load_ncsd_with_keys",
            ));
        }

        let start = reader.seek(SeekFrom::Start(0))?;
        if start != 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "reader did not rewind before ARM11 loading",
            )));
        }

        self.load_ncsd_with_keys(reader, [0; AES_BLOCK_SIZE], [0; AES_BLOCK_SIZE])
    }

    /// Opens a NoCrypto `.3ds` or `.cci` cartridge without asking for AES keys.
    ///
    /// Validation and process loading use the same open file handle.
    pub fn load_ncsd_no_crypto_file(&mut self, path: &Path) -> Result<ImageFormat> {
        self.load_ncsd_no_crypto_reader(File::open(path)?)
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

    /// Returns the PICA200 command processor.
    pub const fn gpu(&self) -> &Pica200 {
        &self.gpu
    }

    /// Returns mutable access to the PICA200 command processor.
    pub fn gpu_mut(&mut self) -> &mut Pica200 {
        &mut self.gpu
    }

    /// Returns the current dual-screen output surfaces.
    pub const fn display(&self) -> &DualScreenDisplay {
        &self.display
    }

    /// Returns mutable access to the dual-screen output surfaces.
    pub fn display_mut(&mut self) -> &mut DualScreenDisplay {
        &mut self.display
    }

    /// Advances one frame using a caller-supplied ARM11 instruction budget.
    ///
    /// The budget is deterministic but not cycle-accurate. The development
    /// display currently provides RGBA8 output surfaces; rendering commands
    /// must be connected to those surfaces by later PICA200 pipeline stages.
    pub fn run_frame_with_budget(
        &mut self,
        instruction_budget: usize,
    ) -> Result<SystemFrameReport> {
        if self.image.is_none() {
            return Err(Error::NoImageLoaded);
        }

        let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
            "a prepared ARM11 process session is required for frame execution",
        ))?;
        let cpu_report = session.run_instructions(instruction_budget)?;
        session.advance_time(NOMINAL_FRAME_DURATION_NS);
        self.cpu = session.core().clone();

        let frame_number = self.display.present();
        Ok(SystemFrameReport {
            frame_number,
            instructions_executed: cpu_report.instructions_executed,
            cpu_stop_reason: cpu_report.stop_reason,
            elapsed_virtual_time_ns: NOMINAL_FRAME_DURATION_NS,
        })
    }

    /// Runs one development frame using the default instruction budget.
    ///
    /// This is not a claim of hardware-cycle-accurate frame timing.
    pub fn run_frame(&mut self) -> Result<()> {
        self.run_frame_with_budget(DEFAULT_FRAME_INSTRUCTION_BUDGET)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::ncsd::session::Arm11RunStopReason;

    #[test]
    fn running_without_an_image_fails() {
        let mut system = System::new();
        assert!(matches!(system.run_frame(), Err(Error::NoImageLoaded)));
    }

    #[test]
    fn frame_requires_a_prepared_arm11_process() {
        let mut system = System::new();
        system.image = Some(ImageFormat::Ncsd);
        assert!(matches!(
            system.run_frame_with_budget(0),
            Err(Error::Unimplemented(_))
        ));
        assert_eq!(system.display().frame_number(), 0);
    }

    #[test]
    fn frame_executes_cpu_advances_virtual_time_and_presents_dual_screens() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let report = system.run_frame_with_budget(2).unwrap();
        assert_eq!(report.frame_number, 1);
        assert_eq!(report.instructions_executed, 2);
        assert_eq!(
            report.cpu_stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
        assert_eq!(report.elapsed_virtual_time_ns, NOMINAL_FRAME_DURATION_NS);
        assert_eq!(system.display().frame_number(), 1);
        assert_eq!(system.display().top_left().pixels().len(), 400 * 240 * 4);
        assert_eq!(system.display().top_right().pixels().len(), 400 * 240 * 4);
        assert_eq!(system.display().bottom().pixels().len(), 320 * 240 * 4);
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
    fn inspects_ncsd_partition_metadata_from_both_3ds_and_cci_paths() {
        for extension in ["3ds", "cci"] {
            let path = std::env::temp_dir()
                .join(format!("aceris-inspect-{}.{extension}", std::process::id()));
            let image = clear_ncsd_cartridge_fixture();
            std::fs::write(&path, &image).unwrap();

            let header = System::inspect_ncsd_file(&path).unwrap();
            let _ = std::fs::remove_file(&path);

            assert_eq!(header.image_size_bytes, image.len() as u64);
            assert_eq!(header.partitions.len(), 1);
            let primary = header.primary_partition().unwrap();
            assert_eq!(primary.index, 0);
            assert_eq!(primary.offset_bytes, 0x4000);
            assert_eq!(primary.size_bytes, 0x8000);
        }
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
    fn loads_no_crypto_ncsd_without_caller_keys_from_both_3ds_and_cci_paths() {
        for extension in ["3ds", "cci"] {
            let path = std::env::temp_dir().join(format!(
                "aceris-no-crypto-{}.{extension}",
                std::process::id()
            ));
            std::fs::write(&path, clear_ncsd_cartridge_fixture()).unwrap();

            let mut system = System::new();
            let result = system.load_ncsd_no_crypto_file(&path);
            let _ = std::fs::remove_file(&path);

            assert_eq!(result.unwrap(), ImageFormat::Ncsd);
            assert_eq!(system.cpu().pc(), 0x0010_0000);
            assert!(system.ncsd_session().is_some());

            let report = system.run_instructions(2).unwrap();
            assert_eq!(report.instructions_executed, 2);
            assert_eq!(system.cpu().regs[0], 7);
            assert_eq!(system.cpu().regs[1], 5);
        }
    }

    #[test]
    fn loads_no_crypto_ncsd_from_a_seekable_reader() {
        let image = clear_ncsd_cartridge_fixture();
        let mut system = System::new();

        assert_eq!(
            system
                .load_ncsd_no_crypto_reader(std::io::Cursor::new(image))
                .unwrap(),
            ImageFormat::Ncsd
        );
        assert_eq!(system.cpu().pc(), 0x0010_0000);
        assert!(system.ncsd_session().is_some());

        let report = system.run_instructions(2).unwrap();
        assert_eq!(report.instructions_executed, 2);
        assert_eq!(system.cpu().regs[0], 7);
        assert_eq!(system.cpu().regs[1], 5);
    }

    #[test]
    fn no_crypto_loader_rejects_encrypted_primary_partition_without_replacing_session() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; 16],
                [0; 16],
            )
            .unwrap();
        let original_pc = system.cpu().pc();

        let path =
            std::env::temp_dir().join(format!("aceris-encrypted-{}.3ds", std::process::id()));
        let mut encrypted = clear_ncsd_cartridge_fixture();
        encrypted[0x4000 + 0x18F] &= !0x04;
        std::fs::write(&path, encrypted).unwrap();

        let error = system.load_ncsd_no_crypto_file(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);

        assert!(matches!(error, Error::Unimplemented(_)));
        assert_eq!(system.image(), Some(ImageFormat::Ncsd));
        assert_eq!(system.cpu().pc(), original_pc);
        assert!(system.ncsd_session().is_some());
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
