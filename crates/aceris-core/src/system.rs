//! Top-level emulated system.

use crate::cpu::Arm11Core;
use crate::gpu::{
    DisplayTransferRequest, GSP_GX_COMMAND_DISPLAY_TRANSFER, GSP_GX_COMMAND_FLUSH_CACHE_REGIONS,
    GSP_GX_COMMAND_MEMORY_FILL, GSP_GX_COMMAND_PROCESS_COMMAND_LIST, GSP_GX_COMMAND_QUEUE_SIZE,
    GSP_GX_COMMAND_REQUEST_DMA, GSP_GX_COMMAND_TEXTURE_COPY, GSP_INTERRUPT_QUEUE_SIZE,
    GSP_INVALID_ADDRESS_RESULT, GpuDmaAddressSpace, GpuPhysicalMemory, GspDisplayTransfer,
    GspDmaRequest, GspFlushCacheRegions, GspGxCommandQueue, GspGxCommandQueueError, GspInterruptId,
    GspInterruptQueuePushResult, GspInterruptRelayQueue, GspMemoryFill, GspMemoryFillBuffer,
    GspProcessCommandList, GspTextureCopy, MAX_GSP_DMA_BYTES, Pica200, PicaRenderReport,
    PicaRenderSession, PicaRenderSettings, REG_ATTRIBBUFFERS_LOC, TextureCopyError,
    TextureCopyRequest, VRAM_BASE, VRAM_SIZE, display_transfer_buffer_sizes,
    execute_display_transfer, execute_texture_copy,
};
use crate::loader::cia::{CiaArchiveReader, TicketCommonKeyStore, tmd::CONTENT_TYPE_ENCRYPTED};
use crate::loader::ncsd::ncch::{self, crypto::AES_BLOCK_SIZE};
use crate::loader::ncsd::session::gsp_service::{
    GSP_SHARED_MEMORY_BASE, GSP_SHARED_MEMORY_SIZE, GspClientUnregistration,
    GspInterruptDispatchReport, GspIpcRequest, GspIpcResultResponse,
    GspRegisterInterruptRelayQueueResponse, GspRenderingRightAcquireOutcome,
    GspRenderingRightHandoff, GspServiceError, GspServiceRegistry, decode_gsp_ipc_request,
};
use crate::loader::ncsd::session::ipc::{IpcCommandHeader, IpcMessage};
use crate::loader::ncsd::session::kernel::ServiceSessionKind;
use crate::loader::ncsd::session::{Arm11RunStopReason, ThreadId};
use crate::loader::ncsd::session::{
    RESULT_INVALID_ADDRESS, RESULT_INVALID_ENUM, RESULT_INVALID_HANDLE, RESULT_OUT_OF_MEMORY,
};
use crate::loader::ncsd::{Arm11ProcessSession, Arm11RunReport, NcsdHeader, NcsdImageReader};
use crate::loader::threedsx::ThreeDsxImage;
use crate::loader::{self, ImageFormat};
use crate::video::{
    DualScreenDisplay, LcdController, LcdPanel, PDC_FRAMEBUFFER_A_FIRST, PDC_FRAMEBUFFER_A_SECOND,
    PDC_FRAMEBUFFER_B_FIRST, PDC_FRAMEBUFFER_B_SECOND, PDC_FRAMEBUFFER_FORMAT,
    PDC_FRAMEBUFFER_SELECT, PDC_FRAMEBUFFER_STRIDE,
};
use aceris_common::{Error, Result};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingGspAcquire {
    client_id: u8,
    thread_id: ThreadId,
}

const GSP_TOP_FRAMEBUFFER_INFO_OFFSET: u32 = 0x200;
const GSP_BOTTOM_FRAMEBUFFER_INFO_OFFSET: u32 = 0x240;
const GSP_FRAMEBUFFER_INFO_CLIENT_STRIDE: u32 = 0x80;
const GSP_FRAMEBUFFER_INFO_HEADER_SIZE: u32 = 4;
const GSP_FRAMEBUFFER_INFO_ENTRY_SIZE: u32 = 0x1C;
const GSP_FRAMEBUFFER_INFO_NEW_DATA: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingGspFramebufferInfo {
    panel: LcdPanel,
    flags_address: u32,
    cleared_flags: u8,
    active_buffer: u32,
    left_address: u32,
    right_address: u32,
    stride: u32,
    format: u32,
    status: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedGspFramebufferInfo {
    info: PendingGspFramebufferInfo,
    left_physical_address: u32,
    right_physical_address: u32,
}

/// The emulated console. Front-ends create one of these, load a game and then
/// drive it one frame at a time.
#[derive(Debug, Default)]
pub struct System {
    cpu: Arm11Core,
    image: Option<ImageFormat>,
    ncsd_session: Option<Arm11ProcessSession>,
    gsp_service: GspServiceRegistry,
    pending_gsp_acquires: Vec<PendingGspAcquire>,
    gpu: Pica200,
    /// When set, submitted command lists are rendered, not only parsed.
    pica_render_settings: Option<PicaRenderSettings>,
    /// Persistent state for the enabled software PICA200 rendering path.
    pica_render_session: Option<PicaRenderSession>,
    last_pica_render_report: Option<PicaRenderReport>,
    physical_memory: GpuPhysicalMemory,
    gpu_address_space: GpuDmaAddressSpace,
    lcd: LcdController,
    display: DualScreenDisplay,
}

/// Nominal 60 Hz development-frame duration in nanoseconds.
pub const NOMINAL_FRAME_DURATION_NS: u64 = 16_666_667;

/// Deterministic instruction cap used by System::run_frame.
pub const DEFAULT_FRAME_INSTRUCTION_BUDGET: usize = 100_000;

/// Maximum guest PICA200 command-list size accepted by one submission.
pub const MAX_PICA_COMMAND_WORDS: usize = 0x1_0000;

/// Result returned by GSP when another client currently owns rendering rights.
const RESULT_GSP_RENDERING_RIGHT_BUSY: u32 = 0xC840_2BF0;
/// Result returned by GX commands that require rendering rights.
const RESULT_GSP_NO_RIGHT: u32 = 0xD820_2A06;

/// Maximum number of 32-bit words in the ARM11 TLS IPC command buffer.
const ARM11_IPC_BUFFER_WORDS: usize = 0x40;
/// Offset of the IPC command buffer within each ARM11 TLS page.
const ARM11_TLS_IPC_BUFFER_OFFSET: u32 = 0x80;

/// Result of processing one GSP GX shared-memory queue snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GspGxCommandQueueReport {
    /// Number of entries consumed from the ring, including a failing entry.
    pub commands_consumed: u8,
    /// Number of ProcessCommandList entries submitted to the PICA200 parser.
    pub command_lists_submitted: u8,
    /// Number of MemoryFill buffer operations completed.
    pub memory_fills_completed: u8,
    /// Number of RequestDMA commands completed by the synchronous software backend.
    pub dma_requests_completed: u8,
    /// Whether a guest RequestDMA requested source writeback before DMA reads.
    pub flush_source_requested: bool,
    /// Number of DisplayTransfer commands completed by the synchronous software backend.
    pub display_transfers_completed: u8,
    /// Number of TextureCopy commands completed by the synchronous backend.
    pub texture_copies_completed: u8,
    /// Non-empty FlushCacheRegions spans validated and processed in order.
    pub cache_regions_validated: u8,
    /// Number of entries still pending after this dispatch.
    pub pending_commands: u8,
    /// Requests observed but not independently emulated by this GSP layer.
    pub cache_flush_requested: bool,
    pub gas_blend_update_requested: bool,
    pub busy_check_requested: bool,
    pub halted: bool,
    pub status: u8,
    pub last_result: u32,
    pub error: Option<GspGxCommandQueueError>,
}

/// Result of enqueueing one GSP interrupt into a client's relay queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GspInterruptRelayReport {
    pub client_id: u8,
    pub interrupt: GspInterruptId,
    pub push_result: GspInterruptQueuePushResult,
    /// The service layer must signal the client's registered event when true.
    pub event_signal_required: bool,
    pub pending_interrupts: u8,
    pub missed_pdc0: u32,
    pub missed_pdc1: u32,
    pub error_code: u8,
}

fn message_words(message: IpcMessage<'_>) -> Vec<u32> {
    let word_count = message.header().expected_word_count();
    let mut words = Vec::with_capacity(word_count);
    words.push(message.header().raw());
    words.extend_from_slice(message.normal_parameters());
    words.extend_from_slice(message.translation_words());
    words
}

fn gsp_service_result_code(error: GspServiceError) -> u32 {
    match error {
        GspServiceError::NoFreeClientSlots => RESULT_OUT_OF_MEMORY,
        GspServiceError::MisalignedSharedMemoryBase { .. }
        | GspServiceError::SharedMemoryUnavailable { .. }
        | GspServiceError::AddressOverflow { .. } => RESULT_INVALID_ADDRESS,
        GspServiceError::InvalidEventHandle { .. }
        | GspServiceError::InvalidClientId { .. }
        | GspServiceError::ClientNotRegistered { .. }
        | GspServiceError::ClientAlreadyRegistered { .. }
        | GspServiceError::RenderingRightRequired { .. }
        | GspServiceError::RenderingRightNotOwned { .. }
        | GspServiceError::DuplicateClientSession { .. }
        | GspServiceError::ClientSessionUnavailable { .. }
        | GspServiceError::ProcessSessionUnavailable
        | GspServiceError::InvalidRelayQueue { .. }
        | GspServiceError::EventSignalFailed { .. } => RESULT_INVALID_HANDLE,
    }
}

/// Result of one completed development frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemFrameReport {
    pub frame_number: u64,
    pub instructions_executed: u64,
    pub cpu_stop_reason: Arm11RunStopReason,
    pub gpu_command_lists_submitted: u64,
    pub lcd_screens_scanned_out: u8,
    pub elapsed_virtual_time_ns: u64,
    /// Number of client relay-queue deliveries across this frame's PDC0/PDC1 events.
    pub gsp_interrupt_deliveries: u8,
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
        self.reset_gpu_linear_memory();
        self.image = Some(format);
        self.ncsd_session = None;
        self.gsp_service = GspServiceRegistry::new();
        self.pending_gsp_acquires.clear();
        self.cpu = Arm11Core::new();
        Ok(format)
    }

    /// Loads a 3DSX homebrew executable into the shared ARM11 process runtime.
    ///
    /// The 3DSX parser chooses the conservative built-in memory profile,
    /// applies relocations, and prepares all segments before the current
    /// process is replaced. A malformed image therefore leaves the old
    /// session intact.
    pub fn load_3dsx_reader<R: Read + Seek>(&mut self, mut reader: R) -> Result<ImageFormat> {
        let image = ThreeDsxImage::parse_reader(&mut reader)?;
        let request = image.conventional_process_request()?;
        let prepared = image.load_and_prepare_process(&mut reader, request)?;
        let mut session = Arm11ProcessSession::from_3dsx_prepared(prepared).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not map 3DSX image into ARM11 memory: {error}"),
            ))
        })?;

        self.reset_gpu_linear_memory();
        session.attach_gpu_services(self.physical_memory.clone(), self.gpu_address_space.clone());
        self.cpu = session.core().clone();
        self.image = Some(ImageFormat::ThreeDsx);
        self.ncsd_session = Some(session);
        self.gsp_service = GspServiceRegistry::new();
        self.pending_gsp_acquires.clear();
        Ok(ImageFormat::ThreeDsx)
    }

    /// Opens and prepares a 3DSX homebrew executable.
    pub fn load_3dsx_file(&mut self, path: &Path) -> Result<ImageFormat> {
        self.load_3dsx_reader(File::open(path)?)
    }

    /// Loads an executable format supported by the current launch path.
    ///
    /// NoCrypto NCSD cartridges, clear CIA boot contents and standard 3DSX
    /// homebrew files share the same ARM11 session and frame execution path.
    pub fn load_executable_file(&mut self, path: &Path) -> Result<ImageFormat> {
        match loader::identify_file(path)? {
            ImageFormat::Ncsd => self.load_ncsd_no_crypto_file(path),
            ImageFormat::Cia => self.load_cia_no_crypto_file(path),
            ImageFormat::ThreeDsx => self.load_3dsx_file(path),
            _ => Err(Error::Unimplemented(
                "execution loading is not implemented for this image format",
            )),
        }
    }

    /// Loads the boot NCCH from a CIA when both CIA and NCCH content are clear.
    ///
    /// The selected TMD content hash, NCCH extended-header hash, ExeFS
    /// superblock and .code digest are verified before replacing the active
    /// process. Ticket/TMD signatures and encrypted CIA/NCCH content are not
    /// supported by this bootstrap path; no title is persistently installed.
    pub fn load_cia_no_crypto_file(&mut self, path: &Path) -> Result<ImageFormat> {
        let mut archive = CiaArchiveReader::open(File::open(path)?).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not open CIA archive: {error}"),
            ))
        })?;
        let title_id = archive.title_metadata().title_id;
        let boot_content_index = archive.title_metadata().boot_content_index;

        let ncch = archive
            .open_verified_ncch_content_reader(boot_content_index)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not verify CIA boot content: {error}"),
                ))
            })?
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "CIA boot content is not selected by its content-index bitmap",
                ))
            })?;

        if ncch.header().program_id != title_id {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "CIA TMD title ID {title_id:#018x} does not match NCCH program ID {:#018x}",
                    ncch.header().program_id
                ),
            )));
        }
        if !ncch.header().declares_no_crypto() {
            return Err(Error::Unimplemented(
                "CIA boot content uses NCCH encryption; encrypted CIA/NCCH launch is not supported by this path",
            ));
        }

        let address_space = ncch
            .load_arm11_address_space([0; AES_BLOCK_SIZE], [0; AES_BLOCK_SIZE])
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not prepare ARM11 process from CIA: {error}"),
                ))
            })?;
        let mut session = Arm11ProcessSession::from_address_space(address_space);

        self.reset_gpu_linear_memory();
        session.attach_gpu_services(self.physical_memory.clone(), self.gpu_address_space.clone());
        self.cpu = session.core().clone();
        self.image = Some(ImageFormat::Cia);
        self.ncsd_session = Some(session);
        self.gsp_service = GspServiceRegistry::new();
        self.pending_gsp_acquires.clear();
        Ok(ImageFormat::Cia)
    }

    /// Loads a CIA whose outer content is clear but whose NCCH regions may be encrypted.
    ///
    /// The CIA content is verified against its TMD before the NCCH is opened.
    /// The caller supplies the NCCH menu-info and content keys; no ticket
    /// common key or title-key decryption is needed when the TMD marks the
    /// content as clear. Encrypted outer CIA content must use
    /// `load_cia_with_keys` instead.
    ///
    /// The active process is replaced only after all content and executable
    /// checks succeed.
    pub fn load_cia_with_ncch_keys<R: Read + Seek>(
        &mut self,
        reader: R,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> Result<ImageFormat> {
        let mut archive = CiaArchiveReader::open(reader).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not open CIA archive: {error}"),
            ))
        })?;
        let title_id = archive.title_metadata().title_id;
        let boot_content_index = archive.title_metadata().boot_content_index;
        let location = archive
            .content_location(boot_content_index)
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "CIA boot content is not selected by its content-index bitmap",
                ))
            })?;

        if location.content_type & CONTENT_TYPE_ENCRYPTED != 0 {
            return Err(Error::Unimplemented(
                "CIA boot content uses outer AES-CBC encryption; use load_cia_with_keys with caller-supplied ticket keys",
            ));
        }

        let ncch = archive
            .open_verified_ncch_content_reader(boot_content_index)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not verify CIA boot content: {error}"),
                ))
            })?
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "CIA boot content is not selected by its content-index bitmap",
                ))
            })?;

        if ncch.header().program_id != title_id {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "CIA TMD title ID {title_id:#018x} does not match NCCH program ID {:#018x}",
                    ncch.header().program_id
                ),
            )));
        }

        let address_space = ncch
            .load_arm11_address_space(menu_info_key, content_key)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not prepare ARM11 process from CIA NCCH: {error}"),
                ))
            })?;
        let mut session = Arm11ProcessSession::from_address_space(address_space);

        self.reset_gpu_linear_memory();
        session.attach_gpu_services(self.physical_memory.clone(), self.gpu_address_space.clone());
        self.cpu = session.core().clone();
        self.image = Some(ImageFormat::Cia);
        self.ncsd_session = Some(session);
        self.gsp_service = GspServiceRegistry::new();
        self.pending_gsp_acquires.clear();
        Ok(ImageFormat::Cia)
    }

    /// Opens and launches a clear-content CIA using caller-supplied NCCH keys.
    pub fn load_cia_file_with_ncch_keys(
        &mut self,
        path: &Path,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> Result<ImageFormat> {
        if loader::identify_file(path)? != ImageFormat::Cia {
            return Err(Error::UnsupportedFormat);
        }

        self.load_cia_with_ncch_keys(File::open(path)?, menu_info_key, content_key)
    }

    /// Loads the boot NCCH from an encrypted CIA using caller-supplied keys.
    ///
    /// The ticket title key is decrypted through `key_store`, then the CIA
    /// AES-CBC content hash is verified before the NCCH is opened. The shared
    /// NCCH preparation path validates the executable and uses the supplied
    /// menu-info/content keys for encrypted NCCH regions. Ticket/TMD signatures
    /// are not verified; Aceris never discovers or persists key material.
    ///
    /// The active process is replaced only after all content and executable
    /// checks succeed.
    pub fn load_cia_with_keys<R: Read + Seek>(
        &mut self,
        reader: R,
        key_store: &TicketCommonKeyStore,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> Result<ImageFormat> {
        let mut archive = CiaArchiveReader::open(reader).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("could not open CIA archive: {error}"),
            ))
        })?;
        let title_id = archive.title_metadata().title_id;
        let boot_content_index = archive.title_metadata().boot_content_index;

        let ticket = archive.ticket().ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "encrypted CIA launch requires a ticket",
            ))
        })?;
        let title_key = key_store
            .decrypt_ticket_title_key(ticket)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("could not resolve caller-supplied CIA ticket keys: {error}"),
                ))
            })?;

        let ncch = archive
            .open_verified_decrypted_ncch_content_reader(boot_content_index, &title_key)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not decrypt and verify CIA boot content: {error}"),
                ))
            })?
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "CIA boot content is not selected by its content-index bitmap",
                ))
            })?;

        if ncch.header().program_id != title_id {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "CIA TMD title ID {title_id:#018x} does not match NCCH program ID {:#018x}",
                    ncch.header().program_id
                ),
            )));
        }

        let address_space = ncch
            .load_arm11_address_space(menu_info_key, content_key)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("could not prepare ARM11 process from encrypted CIA: {error}"),
                ))
            })?;
        let mut session = Arm11ProcessSession::from_address_space(address_space);

        self.reset_gpu_linear_memory();
        session.attach_gpu_services(self.physical_memory.clone(), self.gpu_address_space.clone());
        self.cpu = session.core().clone();
        self.image = Some(ImageFormat::Cia);
        self.ncsd_session = Some(session);
        self.gsp_service = GspServiceRegistry::new();
        self.pending_gsp_acquires.clear();
        Ok(ImageFormat::Cia)
    }

    /// Opens and launches an encrypted CIA from a file with caller-supplied keys.
    pub fn load_cia_file_with_keys(
        &mut self,
        path: &Path,
        key_store: &TicketCommonKeyStore,
        menu_info_key: [u8; AES_BLOCK_SIZE],
        content_key: [u8; AES_BLOCK_SIZE],
    ) -> Result<ImageFormat> {
        if loader::identify_file(path)? != ImageFormat::Cia {
            return Err(Error::UnsupportedFormat);
        }

        self.load_cia_with_keys(File::open(path)?, key_store, menu_info_key, content_key)
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
        let mut session =
            Arm11ProcessSession::load_primary_ncsd(reader, menu_info_key, content_key).map_err(
                |error| {
                    Error::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("could not prepare the primary NCSD program: {error}"),
                    ))
                },
            )?;

        self.reset_gpu_linear_memory();
        session.attach_gpu_services(self.physical_memory.clone(), self.gpu_address_space.clone());
        self.cpu = session.core().clone();
        self.image = Some(ImageFormat::Ncsd);
        self.ncsd_session = Some(session);
        self.gsp_service = GspServiceRegistry::new();
        self.pending_gsp_acquires.clear();
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

    /// Returns read-only access to the GSP service registry.
    pub const fn gsp_service_registry(&self) -> &GspServiceRegistry {
        &self.gsp_service
    }

    /// Registers a GSP client using the flags supplied by RegisterInterruptRelayQueue.
    pub fn register_gsp_client_with_flags(
        &mut self,
        client_id: u8,
        shared_memory_base: u32,
        event_handle: u32,
        flags: u32,
    ) -> std::result::Result<(), GspServiceError> {
        let session = self
            .ncsd_session
            .as_mut()
            .ok_or(GspServiceError::ProcessSessionUnavailable)?;
        self.gsp_service.register_client_with_flags(
            session,
            client_id,
            shared_memory_base,
            event_handle,
            flags,
        )
    }

    /// Registers a GSP client with default flags.
    pub fn register_gsp_client(
        &mut self,
        client_id: u8,
        shared_memory_base: u32,
        event_handle: u32,
    ) -> std::result::Result<(), GspServiceError> {
        self.register_gsp_client_with_flags(client_id, shared_memory_base, event_handle, 0)
    }

    /// Registers the process in the first free GSP client slot.
    pub fn register_next_gsp_client(
        &mut self,
        shared_memory_base: u32,
        event_handle: u32,
    ) -> std::result::Result<u8, GspServiceError> {
        self.register_next_gsp_client_with_flags(shared_memory_base, event_handle, 0)
    }

    /// Registers the process in the first free GSP client slot.
    pub fn register_next_gsp_client_with_flags(
        &mut self,
        shared_memory_base: u32,
        event_handle: u32,
        flags: u32,
    ) -> std::result::Result<u8, GspServiceError> {
        let session = self
            .ncsd_session
            .as_mut()
            .ok_or(GspServiceError::ProcessSessionUnavailable)?;
        self.gsp_service.register_next_available_client_with_flags(
            session,
            shared_memory_base,
            event_handle,
            flags,
        )
    }

    /// Removes a GSP client and reports any pending rendering-right handoff.
    pub fn unregister_gsp_client(
        &mut self,
        client_id: u8,
    ) -> std::result::Result<GspClientUnregistration, GspServiceError> {
        let unregistration = self.gsp_service.unregister_client_with_handoff(client_id)?;
        if unregistration.rendering_right_wait_cancelled {
            self.wake_pending_gsp_acquire(client_id, RESULT_INVALID_HANDLE)
                .map_err(|_| GspServiceError::ClientSessionUnavailable { client_id })?;
        }
        if let Some(next_owner) = unregistration
            .rendering_right_handoff
            .and_then(|handoff| handoff.next_owner)
        {
            self.wake_pending_gsp_acquire(next_owner, 0).map_err(|_| {
                GspServiceError::ClientSessionUnavailable {
                    client_id: next_owner,
                }
            })?;
        }
        Ok(unregistration)
    }

    /// Attempts to acquire rendering rights without joining the wait queue.
    pub fn try_acquire_gsp_rendering_right(
        &mut self,
        client_id: u8,
    ) -> std::result::Result<bool, GspServiceError> {
        self.gsp_service.try_acquire_rendering_right(client_id)
    }

    /// Queues an AcquireRight request in FIFO order.
    ///
    /// Guest IPC calls that return Waiting suspend their ARM11 thread and are
    /// resumed by the system dispatcher after a rendering-right handoff.
    pub fn acquire_gsp_rendering_right(
        &mut self,
        client_id: u8,
    ) -> std::result::Result<GspRenderingRightAcquireOutcome, GspServiceError> {
        self.gsp_service.acquire_rendering_right(client_id)
    }

    /// Releases rendering rights and reports the next FIFO owner to wake.
    pub fn release_gsp_rendering_right(
        &mut self,
        client_id: u8,
    ) -> std::result::Result<GspRenderingRightHandoff, GspServiceError> {
        let handoff = self
            .gsp_service
            .release_rendering_right_with_handoff(client_id)?;
        if let Some(next_owner) = handoff.next_owner {
            self.wake_pending_gsp_acquire(next_owner, 0).map_err(|_| {
                GspServiceError::ClientSessionUnavailable {
                    client_id: next_owner,
                }
            })?;
        }
        Ok(handoff)
    }

    /// Routes one GPU interrupt through registered relay queues and event handles.
    pub fn dispatch_gsp_interrupt(
        &mut self,
        interrupt: GspInterruptId,
    ) -> std::result::Result<GspInterruptDispatchReport, GspServiceError> {
        let session = self
            .ncsd_session
            .as_mut()
            .ok_or(GspServiceError::ProcessSessionUnavailable)?;
        self.gsp_service
            .deliver_interrupt_to_session(session, interrupt)
    }

    /// Executes up to `instruction_budget` instructions in the loaded ARM11 session.
    ///
    /// The report identifies budget exhaustion, unimplemented SVCs, process exit,
    /// and scheduler states without claiming that one instruction budget equals
    /// one video frame.
    pub fn run_instructions(&mut self, instruction_budget: usize) -> Result<Arm11RunReport> {
        let mut instructions_executed = 0u64;
        let mut stop_reason = Arm11RunStopReason::InstructionBudgetExhausted;

        loop {
            let already_executed = usize::try_from(instructions_executed).unwrap_or(usize::MAX);
            let remaining = instruction_budget.saturating_sub(already_executed);
            if remaining == 0 {
                break;
            }

            let report = self
                .ncsd_session
                .as_mut()
                .ok_or(Error::Unimplemented(
                    "an ARM11 process session has not been prepared",
                ))?
                .run_instructions(remaining)?;
            instructions_executed =
                instructions_executed.saturating_add(report.instructions_executed);
            stop_reason = report.stop_reason;

            if stop_reason != Arm11RunStopReason::IpcRequest {
                break;
            }

            if self.dispatch_pending_ipc_request()? {
                stop_reason = Arm11RunStopReason::InstructionBudgetExhausted;
                continue;
            }
            stop_reason = Arm11RunStopReason::InstructionBudgetExhausted;
        }

        let session = self.ncsd_session.as_ref().ok_or(Error::Unimplemented(
            "an NCSD ARM11 process session has not been prepared",
        ))?;
        self.cpu = session.core().clone();
        Ok(Arm11RunReport {
            instructions_executed,
            stop_reason,
        })
    }

    /// Dispatches one guest SendSyncRequest against the HLE service registry.
    ///
    /// Requests and replies use the current thread's TLS command buffer. A
    /// service call is handled synchronously before ARM11 execution resumes.
    fn dispatch_pending_ipc_request(&mut self) -> Result<bool> {
        let (request_thread, service_handle, tls_address, request_words, service_kind) = {
            let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
                "an NCSD ARM11 process session has not been prepared",
            ))?;
            let request_thread = session.current_thread_id().ok_or(Error::Unimplemented(
                "IPC request has no selected ARM11 thread",
            ))?;
            let registers = session.current_registers().ok_or(Error::Unimplemented(
                "IPC request has no running ARM11 thread",
            ))?;
            let tls_address = session.current_tls_address().ok_or(Error::Unimplemented(
                "IPC request thread has no TLS mapping",
            ))?;
            let request_address = tls_address
                .checked_add(ARM11_TLS_IPC_BUFFER_OFFSET)
                .ok_or(Error::InvalidAccess { addr: tls_address })?;
            let header = session.read_memory_u32(request_address)?;
            let expected_words = IpcCommandHeader::decode(header).expected_word_count();
            if expected_words > ARM11_IPC_BUFFER_WORDS {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "ARM11 IPC message declares {expected_words} words; TLS buffer holds {ARM11_IPC_BUFFER_WORDS}"
                    ),
                )));
            }

            let mut request_words = Vec::with_capacity(expected_words);
            for index in 0..expected_words {
                let offset =
                    u32::try_from(index.saturating_mul(4)).map_err(|_| Error::InvalidAccess {
                        addr: request_address,
                    })?;
                let address = request_address
                    .checked_add(offset)
                    .ok_or(Error::InvalidAccess {
                        addr: request_address,
                    })?;
                request_words.push(session.read_memory_u32(address)?);
            }
            let service_handle = registers[0];
            let service_kind = session
                .service_session_kind(service_handle)
                .ok_or(Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("SVC SendSyncRequest used non-service handle 0x{service_handle:08X}"),
                )))?;
            (
                request_thread,
                service_handle,
                tls_address,
                request_words,
                service_kind,
            )
        };

        let message = IpcMessage::decode(&request_words).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid ARM11 IPC request: {error:?}"),
            ))
        })?;
        let pending_before = self.pending_gsp_acquires.len();
        let response_words = match service_kind {
            ServiceSessionKind::ServiceManager { .. } => {
                self.dispatch_service_manager_request(service_handle, message)?
            }
            ServiceSessionKind::GspGpu {
                client_id,
                shared_memory_handle,
                event_handle,
                flags,
            } => self.dispatch_gsp_request(
                service_handle,
                client_id,
                shared_memory_handle,
                event_handle,
                flags,
                message,
            )?,
        };
        if self.pending_gsp_acquires.len() > pending_before {
            return Ok(true);
        }

        if response_words.len() > ARM11_IPC_BUFFER_WORDS {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "ARM11 IPC response exceeds the TLS command buffer",
            )));
        }
        let request_address = tls_address
            .checked_add(ARM11_TLS_IPC_BUFFER_OFFSET)
            .ok_or(Error::InvalidAccess { addr: tls_address })?;
        let mut response_bytes = Vec::with_capacity(response_words.len() * 4);
        for word in response_words {
            response_bytes.extend_from_slice(&word.to_le_bytes());
        }
        let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
            "ARM11 process session was removed during IPC dispatch",
        ))?;
        session.write_memory_bytes(request_address, &response_bytes)?;
        if !session.set_thread_register(request_thread, 0, 0) {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "ARM11 IPC response thread disappeared during service dispatch",
            )));
        }
        Ok(false)
    }

    /// Completes every queued IPC request belonging to a GSP rendering-right owner.
    ///
    /// The GSP wait queue is per registered client, while more than one ARM11
    /// thread in that client may be waiting on the same AcquireRight operation.
    fn wake_pending_gsp_acquire(&mut self, client_id: u8, result_code: u32) -> Result<bool> {
        let response = GspIpcResultResponse::for_request(
            GspIpcRequest::AcquireRight {
                flags: 0,
                process_handle: 0,
            },
            result_code,
        )
        .expect("AcquireRight has a simple reply")
        .encode();

        let mut woke_any = false;
        while let Some(index) = self
            .pending_gsp_acquires
            .iter()
            .position(|pending| pending.client_id == client_id)
        {
            let pending = self.pending_gsp_acquires[index];
            let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
                "ARM11 process session is unavailable while completing GSP AcquireRight",
            ))?;
            session.write_ipc_response_to_thread(pending.thread_id, &response)?;
            session
                .wake_thread_for_ipc(pending.thread_id)
                .map_err(|error| Error::Io(io::Error::other(error.to_string())))?;
            self.pending_gsp_acquires.remove(index);
            woke_any = true;
        }
        Ok(woke_any)
    }

    fn dispatch_service_manager_request(
        &mut self,
        service_handle: u32,
        message: IpcMessage<'_>,
    ) -> Result<Vec<u32>> {
        const SRV_REGISTER_CLIENT_HEADER: u32 = 0x0001_0002;
        const SRV_GET_SERVICE_HANDLE_HEADER: u32 = 0x0005_0100;

        let raw_header = message.header().raw();
        match raw_header {
            SRV_REGISTER_CLIENT_HEADER => {
                let translation = message.translation_words();
                if translation.len() != 2 || translation[0] != 0x20 {
                    return Ok(vec![0x0001_0040, RESULT_INVALID_ENUM]);
                }
                // The kernel-supplied process ID is not yet modelled; retain
                // the registration state on this process-local srv: session.
                let result = self
                    .ncsd_session
                    .as_mut()
                    .ok_or(Error::Unimplemented("ARM11 process session is unavailable"))?
                    .register_service_manager_client(service_handle)
                    .map(|()| 0)
                    .unwrap_or(RESULT_INVALID_HANDLE);
                Ok(vec![0x0001_0040, result])
            }
            SRV_GET_SERVICE_HANDLE_HEADER => {
                let registered = self
                    .ncsd_session
                    .as_ref()
                    .and_then(|session| session.service_manager_client_registered(service_handle))
                    .unwrap_or(false);
                if !registered {
                    return Ok(vec![0x0005_0040, RESULT_INVALID_HANDLE]);
                }
                let parameters = message.normal_parameters();
                if parameters.len() != 4 || !message.translation_words().is_empty() {
                    return Ok(vec![0x0005_0040, RESULT_INVALID_ENUM]);
                }
                let mut requested_name = [0u8; 8];
                requested_name[0..4].copy_from_slice(&parameters[0].to_le_bytes());
                requested_name[4..8].copy_from_slice(&parameters[1].to_le_bytes());
                let name_length = usize::try_from(parameters[2]).unwrap_or(usize::MAX);
                if name_length != 8 || &requested_name != b"gsp::Gpu" {
                    return Ok(vec![0x0005_0040, RESULT_INVALID_ENUM]);
                }

                let service_handle = match self
                    .ncsd_session
                    .as_mut()
                    .ok_or(Error::Unimplemented("ARM11 process session is unavailable"))?
                    .create_gsp_gpu_session()
                {
                    Ok(handle) => handle,
                    Err(_) => return Ok(vec![0x0005_0040, RESULT_OUT_OF_MEMORY]),
                };
                // GetServiceHandle returns a COPY_HANDLE translation containing
                // the newly created client-side gsp::Gpu session.
                Ok(vec![0x0005_0042, 0, 0, service_handle])
            }
            _ => Ok(vec![(raw_header & 0xFFFF_0000) | 0x40, RESULT_INVALID_ENUM]),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_gsp_request(
        &mut self,
        service_handle: u32,
        client_id: Option<u8>,
        shared_memory_handle: Option<u32>,
        _event_handle: Option<u32>,
        flags: u32,
        message: IpcMessage<'_>,
    ) -> Result<Vec<u32>> {
        let request = match decode_gsp_ipc_request(&message_words(message)) {
            Ok(request) => request,
            Err(_) => {
                return Ok(vec![
                    (message.header().raw() & 0xFFFF_0000) | 0x40,
                    RESULT_INVALID_ENUM,
                ]);
            }
        };

        match request {
            GspIpcRequest::RegisterInterruptRelayQueue {
                flags: requested_flags,
                event_handle: requested_event,
            } => {
                if client_id.is_some() {
                    return Ok(GspRegisterInterruptRelayQueueResponse::error(
                        RESULT_INVALID_HANDLE,
                    )
                    .encode()
                    .to_vec());
                }

                let session = self
                    .ncsd_session
                    .as_mut()
                    .ok_or(Error::Unimplemented("ARM11 process session is unavailable"))?;
                let memory_handle =
                    match session.create_memory_block(GSP_SHARED_MEMORY_SIZE as u32, 3, 3) {
                        Ok(handle) => handle,
                        Err(_) => {
                            return Ok(GspRegisterInterruptRelayQueueResponse::error(
                                RESULT_OUT_OF_MEMORY,
                            )
                            .encode()
                            .to_vec());
                        }
                    };
                if session
                    .map_memory_block(memory_handle, GSP_SHARED_MEMORY_BASE, 3, 0x1000_0000)
                    .is_err()
                {
                    let _ = session.close_handle(memory_handle);
                    return Ok(GspRegisterInterruptRelayQueueResponse::error(
                        RESULT_INVALID_ADDRESS,
                    )
                    .encode()
                    .to_vec());
                }

                let registration = self.gsp_service.register_next_available_client_with_flags(
                    session,
                    GSP_SHARED_MEMORY_BASE,
                    requested_event,
                    requested_flags,
                );
                let assigned_client_id = match registration {
                    Ok(client_id) => client_id,
                    Err(error) => {
                        let _ = session.unmap_memory_block(memory_handle, GSP_SHARED_MEMORY_BASE);
                        let _ = session.close_handle(memory_handle);
                        return Ok(GspRegisterInterruptRelayQueueResponse::error(
                            gsp_service_result_code(error),
                        )
                        .encode()
                        .to_vec());
                    }
                };
                if session
                    .set_gsp_session_registration(
                        service_handle,
                        Some(assigned_client_id),
                        Some(memory_handle),
                        Some(requested_event),
                        requested_flags,
                    )
                    .is_err()
                {
                    let _ = self.gsp_service.unregister_client(assigned_client_id);
                    let _ = session.unmap_memory_block(memory_handle, GSP_SHARED_MEMORY_BASE);
                    let _ = session.close_handle(memory_handle);
                    return Ok(GspRegisterInterruptRelayQueueResponse::error(
                        RESULT_INVALID_HANDLE,
                    )
                    .encode()
                    .to_vec());
                }

                Ok(GspRegisterInterruptRelayQueueResponse::new(
                    0,
                    assigned_client_id,
                    memory_handle,
                )
                .encode()
                .to_vec())
            }
            GspIpcRequest::TriggerCmdReqQueue => {
                let result = match client_id {
                    Some(client_id) => self
                        .trigger_gsp_cmd_req_queue(client_id)
                        .map(|_| 0)
                        .unwrap_or(RESULT_INVALID_ADDRESS),
                    None => RESULT_INVALID_HANDLE,
                };
                Ok(GspIpcResultResponse::for_request(request, result)
                    .expect("TriggerCmdReqQueue has a simple reply")
                    .encode()
                    .to_vec())
            }
            GspIpcRequest::UnregisterInterruptRelayQueue => {
                let result = match (client_id, shared_memory_handle) {
                    (Some(client_id), Some(memory_handle)) => {
                        match self.gsp_service.unregister_client_with_handoff(client_id) {
                            Ok(unregistration) => {
                                if unregistration.rendering_right_wait_cancelled {
                                    self.wake_pending_gsp_acquire(
                                        client_id,
                                        RESULT_INVALID_HANDLE,
                                    )?;
                                }
                                if let Some(next_owner) = unregistration
                                    .rendering_right_handoff
                                    .and_then(|handoff| handoff.next_owner)
                                {
                                    self.wake_pending_gsp_acquire(next_owner, 0)?;
                                }
                                if let Some(session) = self.ncsd_session.as_mut() {
                                    let _ = session
                                        .unmap_memory_block(memory_handle, GSP_SHARED_MEMORY_BASE);
                                    let _ = session.close_handle(memory_handle);
                                    let _ = session.set_gsp_session_registration(
                                        service_handle,
                                        None,
                                        None,
                                        None,
                                        flags,
                                    );
                                }
                                0
                            }
                            Err(error) => gsp_service_result_code(error),
                        }
                    }
                    _ => RESULT_INVALID_HANDLE,
                };
                Ok(GspIpcResultResponse::for_request(request, result)
                    .expect("UnregisterInterruptRelayQueue has a simple reply")
                    .encode()
                    .to_vec())
            }
            GspIpcRequest::TryAcquireRight { process_handle: _ } => {
                let result = match client_id {
                    Some(client_id) => {
                        match self.gsp_service.try_acquire_rendering_right(client_id) {
                            Ok(true) => 0,
                            Ok(false) => RESULT_GSP_RENDERING_RIGHT_BUSY,
                            Err(error) => gsp_service_result_code(error),
                        }
                    }
                    None => RESULT_INVALID_HANDLE,
                };
                Ok(GspIpcResultResponse::for_request(request, result)
                    .expect("TryAcquireRight has a simple reply")
                    .encode()
                    .to_vec())
            }
            GspIpcRequest::AcquireRight {
                flags: _,
                process_handle: _,
            } => {
                let result = match client_id {
                    Some(client_id) => {
                        let acquire = self.gsp_service.acquire_rendering_right(client_id);
                        match acquire {
                            Ok(GspRenderingRightAcquireOutcome::Acquired) => 0,
                            Ok(
                                GspRenderingRightAcquireOutcome::Waiting { .. }
                                | GspRenderingRightAcquireOutcome::AlreadyWaiting { .. },
                            ) => {
                                let session = self.ncsd_session.as_mut().ok_or(
                                    Error::Unimplemented("ARM11 process session is unavailable"),
                                )?;
                                let Some(thread_id) = session.current_thread_id() else {
                                    if matches!(
                                        acquire,
                                        Ok(GspRenderingRightAcquireOutcome::Waiting { .. })
                                    ) {
                                        let _ =
                                            self.gsp_service.cancel_rendering_right_wait(client_id);
                                    }
                                    return Ok(GspIpcResultResponse::for_request(
                                        request,
                                        RESULT_INVALID_HANDLE,
                                    )
                                    .expect("AcquireRight has a simple reply")
                                    .encode()
                                    .to_vec());
                                };
                                match session.wait_current_thread_for_ipc() {
                                    Ok(_) => {
                                        self.pending_gsp_acquires.push(PendingGspAcquire {
                                            client_id,
                                            thread_id,
                                        });
                                        return Ok(Vec::new());
                                    }
                                    Err(_) => {
                                        if matches!(
                                            acquire,
                                            Ok(GspRenderingRightAcquireOutcome::Waiting { .. })
                                        ) && !self
                                            .pending_gsp_acquires
                                            .iter()
                                            .any(|pending| pending.client_id == client_id)
                                        {
                                            let _ = self
                                                .gsp_service
                                                .cancel_rendering_right_wait(client_id);
                                        }
                                        RESULT_INVALID_HANDLE
                                    }
                                }
                            }
                            Err(error) => gsp_service_result_code(error),
                        }
                    }
                    None => RESULT_INVALID_HANDLE,
                };
                Ok(GspIpcResultResponse::for_request(request, result)
                    .expect("AcquireRight has a simple reply")
                    .encode()
                    .to_vec())
            }
            GspIpcRequest::ReleaseRight => {
                let result = match client_id {
                    Some(client_id) => match self
                        .gsp_service
                        .release_rendering_right_with_handoff(client_id)
                    {
                        Ok(handoff) => {
                            if let Some(next_owner) = handoff.next_owner {
                                self.wake_pending_gsp_acquire(next_owner, 0)?;
                            }
                            0
                        }
                        Err(error) => gsp_service_result_code(error),
                    },
                    None => RESULT_INVALID_HANDLE,
                };
                Ok(GspIpcResultResponse::for_request(request, result)
                    .expect("ReleaseRight has a simple reply")
                    .encode()
                    .to_vec())
            }
        }
    }

    /// Returns the PICA200 command processor.
    /// Returns read-only access to the sparse ARM11 physical-memory backing.
    pub const fn physical_memory(&self) -> &GpuPhysicalMemory {
        &self.physical_memory
    }

    /// Returns mutable access to physical memory for loader, DMA and display setup.
    pub fn physical_memory_mut(&mut self) -> &mut GpuPhysicalMemory {
        &mut self.physical_memory
    }

    /// Releases all linear FCRAM allocations and clears the DMA alias table.
    ///
    /// This is used when replacing the active process so stale process mappings
    /// cannot be reused by the next title.
    fn reset_gpu_linear_memory(&mut self) {
        // A process replacement must not inherit the previous title's active
        // color/depth target or the last draw's diagnostics.
        self.pica_render_session = self.pica_render_settings.map(PicaRenderSession::new);
        self.last_pica_render_report = None;

        let mappings = self.gpu_address_space.mappings();
        for mapping in mappings {
            let _ = self
                .gpu_address_space
                .unmap_linear_region(mapping.virtual_base(), mapping.size());
            if self
                .physical_memory
                .has_linear_fcram_allocation(mapping.physical_base(), mapping.size())
            {
                let _ = self
                    .physical_memory
                    .free_linear_fcram(mapping.physical_base(), mapping.size());
            }
        }
    }

    /// Signals an external ARMv6K CPU event to the active process core.
    ///
    /// Multi-core orchestration can call this for each target core after a guest SEV.
    pub fn signal_cpu_event(&mut self) -> Result<()> {
        let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
            "an NCSD ARM11 process session has not been prepared",
        ))?;
        session.signal_cpu_event()?;
        self.cpu = session.core().clone();
        Ok(())
    }

    /// Returns the explicitly registered process-linear to GPU-physical mappings.
    pub const fn gpu_address_space(&self) -> &GpuDmaAddressSpace {
        &self.gpu_address_space
    }

    /// Returns mutable access to the GPU DMA mapping table.
    ///
    /// Register only ranges whose guest linear-memory allocation has already
    /// succeeded. This API does not allocate guest memory on its own.
    pub fn gpu_address_space_mut(&mut self) -> &mut GpuDmaAddressSpace {
        &mut self.gpu_address_space
    }

    /// Registers a confirmed original-model 3DS linear-memory allocation.
    pub fn map_gpu_linear_region(
        &mut self,
        virtual_base: u32,
        size: u32,
    ) -> Result<crate::gpu::GpuLinearMapping> {
        self.gpu_address_space
            .map_linear_region(virtual_base, size)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    error.to_string(),
                ))
            })
    }

    /// Removes a previously registered linear-memory DMA mapping.
    pub fn unmap_gpu_linear_region(&mut self, virtual_base: u32, size: u32) -> Result<()> {
        self.gpu_address_space
            .unmap_linear_region(virtual_base, size)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    error.to_string(),
                ))
            })
    }

    pub const fn gpu(&self) -> &Pica200 {
        &self.gpu
    }

    /// Returns mutable access to the PICA200 command processor.
    pub fn gpu_mut(&mut self) -> &mut Pica200 {
        &mut self.gpu
    }

    /// Returns read-only access to the physical LCD controller registers.
    pub const fn lcd_controller(&self) -> &LcdController {
        &self.lcd
    }

    /// Returns mutable access to the physical LCD controller registers.
    pub fn lcd_controller_mut(&mut self) -> &mut LcdController {
        &mut self.lcd
    }

    /// Returns the current dual-screen output surfaces.
    pub const fn display(&self) -> &DualScreenDisplay {
        &self.display
    }

    /// Returns mutable access to the dual-screen output surfaces.
    pub fn display_mut(&mut self) -> &mut DualScreenDisplay {
        &mut self.display
    }

    /// Reads and submits a PICA200 command list from physical FCRAM/VRAM.
    ///
    /// The guest address must be four-byte aligned and the requested size is
    /// bounded to prevent an untrusted process from forcing an unbounded host
    /// allocation. All words are read before the GPU parser is invoked, so an
    /// invalid guest range cannot partially modify PICA200 registers.
    /// Compatibility wrapper for the original physical-memory submission API.
    ///
    /// Despite the legacy name, `address` is a physical byte address, not a
    /// process virtual address. Use `submit_pica_command_list_from_linear_memory`
    /// for a registered linear-memory virtual range.
    pub fn submit_pica_command_list_from_memory(
        &mut self,
        address: u32,
        word_count: usize,
    ) -> Result<crate::gpu::PicaCommandListReport> {
        self.submit_pica_command_list_from_physical_memory(address, word_count)
    }

    /// Reads a PICA200 command list from an already translated physical address.
    pub fn submit_pica_command_list_from_physical_memory(
        &mut self,
        address: u32,
        word_count: usize,
    ) -> Result<crate::gpu::PicaCommandListReport> {
        if address & 3 != 0 || word_count > MAX_PICA_COMMAND_WORDS {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PICA200 command list has an unaligned address or exceeds the word limit",
            )));
        }

        let byte_count = word_count.checked_mul(4).ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PICA200 command-list byte count overflowed",
            ))
        })?;
        let end_address = u64::from(address) + byte_count as u64;
        if end_address > 0x1_0000_0000 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PICA200 command list crosses the 32-bit address-space boundary",
            )));
        }

        if self.ncsd_session.is_none() {
            return Err(Error::Unimplemented(
                "a prepared ARM11 process session is required for guest GPU commands",
            ));
        }
        self.physical_memory
            .validate_range(address, byte_count)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    error.to_string(),
                ))
            })?;

        let mut words = Vec::new();
        words.try_reserve_exact(word_count).map_err(|error| {
            Error::Io(io::Error::other(format!(
                "could not allocate PICA200 command buffer: {error}"
            )))
        })?;
        for index in 0..word_count {
            let offset = u32::try_from(index.checked_mul(4).ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "PICA200 command offset overflowed",
                ))
            })?)
            .map_err(|_| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "PICA200 command offset exceeds 32-bit addressing",
                ))
            })?;
            words.push(
                self.physical_memory
                    .read_u32(address + offset)
                    .map_err(|error| {
                        Error::Io(io::Error::new(
                            io::ErrorKind::InvalidData,
                            error.to_string(),
                        ))
                    })?,
            );
        }

        if let Some(settings) = self.pica_render_settings {
            // GPUREG_ATTRIBBUFFERS_LOC holds the physical base address >> 3
            // (28 bits); this is the documented hardware encoding, applied here
            // without independent validation against hardware. Keep one render
            // session across GSP submissions so color/depth state survives.
            let (result, flushed) = {
                let session = self
                    .pica_render_session
                    .get_or_insert_with(|| PicaRenderSession::new(settings));
                let result = session.execute_command_list(
                    &mut self.gpu,
                    &words,
                    &mut self.physical_memory,
                    |pica, _| {
                        pica.register(REG_ATTRIBBUFFERS_LOC)
                            .map(|raw| (raw & 0x0FFF_FFFF) << 3)
                    },
                );
                // GSP may submit DisplayTransfer immediately after this
                // command. Make the render target visible in physical memory
                // before returning, while retaining the in-memory depth state.
                let flushed = session.flush(&mut self.physical_memory);
                (result, flushed)
            };

            let (report, render) = result.map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("PICA200 command list rendering failed: {error}"),
                ))
            })?;
            flushed.map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("PICA200 render-target flush failed: {error}"),
                ))
            })?;
            self.last_pica_render_report = Some(render);
            return Ok(report);
        }

        self.gpu.execute_command_list(&words).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid PICA200 command list: {error}"),
            ))
        })
    }

    /// Enables (or disables with `None`) rendering of submitted command lists.
    ///
    /// Disabled by default, which keeps the parser-only behavior. When enabled,
    /// each draw is rendered into the live tiled render target at trigger time.
    pub fn set_pica_rendering(&mut self, settings: Option<PicaRenderSettings>) {
        self.pica_render_settings = settings;
        self.pica_render_session = settings.map(PicaRenderSession::new);
        self.last_pica_render_report = None;
    }

    /// Totals of the most recent rendered command list, if rendering is enabled.
    pub const fn last_pica_render_report(&self) -> Option<PicaRenderReport> {
        self.last_pica_render_report
    }

    /// Resolves a process-linear command-list address through the registered DMA map.
    ///
    /// The complete byte range must be inside one mapping. The translated
    /// physical address is then submitted through the same bounded parser path.
    pub fn submit_pica_command_list_from_linear_memory(
        &mut self,
        linear_virtual_address: u32,
        word_count: usize,
    ) -> Result<crate::gpu::PicaCommandListReport> {
        let byte_count = word_count.checked_mul(4).ok_or_else(|| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "PICA200 command-list byte count overflowed",
            ))
        })?;
        let resolved = self
            .gpu_address_space
            .resolve_physical_range(&self.physical_memory, linear_virtual_address, byte_count)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    error.to_string(),
                ))
            })?;
        self.submit_pica_command_list_from_physical_memory(resolved.physical_address, word_count)
    }

    /// Processes PICA200 command buffers triggered through internal registers.
    ///
    /// The descriptor contains a physical address; command words are fetched
    /// from the sparse FCRAM/VRAM backing. Process virtual-to-physical mapping
    /// for linear memory remains a separate responsibility.
    pub fn process_pending_pica_command_buffers(&mut self) -> Result<u64> {
        let mut pending = Vec::with_capacity(2);
        for channel in 0..=1 {
            let descriptor = self
                .gpu
                .command_buffer_descriptor(channel)
                .map_err(|error| {
                    Error::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        error.to_string(),
                    ))
                })?;
            if let Some(descriptor) = descriptor {
                if descriptor.word_count() > MAX_PICA_COMMAND_WORDS {
                    return Err(Error::Io(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "PICA200 command buffer exceeds the configured word limit",
                    )));
                }
                pending.push(descriptor);
            }
        }

        let mut submitted = 0u64;
        for descriptor in pending {
            self.submit_pica_command_list_from_physical_memory(
                descriptor.address,
                descriptor.word_count(),
            )?;
            self.gpu
                .acknowledge_command_buffer(descriptor)
                .map_err(|error| {
                    Error::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        error.to_string(),
                    ))
                })?;
            submitted = submitted.saturating_add(1);
        }
        Ok(submitted)
    }

    /// Reads, dispatches and writes back one client's GSP GX shared-memory queue.
    ///
    /// The caller supplies the registered 0x1000-byte shared-memory base and
    /// the client slot (0..4). Each trigger clears the queue halt latch before
    /// processing. The supported synchronous subset includes RequestDMA,
    /// ProcessCommandList, MemoryFill, DisplayTransfer, TextureCopy and
    /// FlushCacheRegions. FlushCacheRegions currently validates guest ranges only;
    /// the process D-cache performs writeback/invalidation for mapped FCRAM
    /// ranges. Busy arbitration and asynchronous device timing remain pending.
    pub fn process_gsp_gx_command_queue(
        &mut self,
        shared_memory_base: u32,
        client_id: u8,
    ) -> Result<GspGxCommandQueueReport> {
        let queue_offset = GspGxCommandQueue::client_queue_offset(client_id).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                error.to_string(),
            ))
        })?;
        let queue_address = shared_memory_base
            .checked_add(queue_offset)
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "GSP GX queue address overflowed the process address space",
                ))
            })?;

        let mut bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        self.ncsd_session
            .as_mut()
            .ok_or(Error::Unimplemented(
                "a prepared ARM11 process session is required for GSP shared memory",
            ))?
            .read_memory_bytes(queue_address, &mut bytes)?;

        let mut queue = GspGxCommandQueue::parse(&bytes).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid GSP GX command queue: {error}"),
            ))
        })?;
        queue.reset_halt_for_trigger();
        let client_has_rendering_right = self.gsp_service.owns_rendering_right(client_id);
        let report = self.dispatch_gsp_gx_command_queue_with_client(
            &mut queue,
            client_has_rendering_right,
            Some((shared_memory_base, client_id)),
        );

        self.ncsd_session
            .as_mut()
            .ok_or(Error::Unimplemented(
                "ARM11 process session was removed during GSP queue dispatch",
            ))?
            .write_memory_bytes(queue_address, &queue.encode_header())?;

        Ok(report)
    }

    /// Handles TriggerCmdReqQueue for a previously registered GSP client.
    ///
    /// TriggerCmdReqQueue itself does not require rendering rights. Individual
    /// queue commands apply their own rights requirements during dispatch.
    pub fn trigger_gsp_cmd_req_queue(&mut self, client_id: u8) -> Result<GspGxCommandQueueReport> {
        let registration = self
            .gsp_service
            .client_registration(client_id)
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("GSP client {client_id} is not registered"),
                ))
            })?;
        self.process_gsp_gx_command_queue(registration.shared_memory_base(), client_id)
    }

    /// Legacy low-level helper that updates one relay queue without signaling
    /// the registered event handle. Prefer dispatch_gsp_interrupt for normal
    /// service delivery, which routes recipients and signals their process-local
    /// event objects after successfully queuing the interrupt.
    ///
    /// shared_memory_base is the guest virtual address of the 0x1000-byte GSP
    /// shared-memory block. The queue is located at client_id * 0x40.
    pub fn enqueue_gsp_interrupt_for_client(
        &mut self,
        shared_memory_base: u32,
        client_id: u8,
        interrupt: GspInterruptId,
    ) -> Result<GspInterruptRelayReport> {
        let queue_offset =
            GspInterruptRelayQueue::client_queue_offset(client_id).map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    error.to_string(),
                ))
            })?;
        let queue_address = shared_memory_base
            .checked_add(queue_offset)
            .ok_or_else(|| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "GSP interrupt relay queue address overflowed the process address space",
                ))
            })?;

        let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
            "a prepared ARM11 process session is required for GSP interrupt delivery",
        ))?;
        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        session.read_memory_bytes(queue_address, &mut bytes)?;
        let mut queue = GspInterruptRelayQueue::parse(&bytes).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid GSP interrupt relay queue: {error}"),
            ))
        })?;

        let push_result = queue.push(interrupt);
        let event_signal_required =
            matches!(push_result, GspInterruptQueuePushResult::Queued { .. });
        let report = GspInterruptRelayReport {
            client_id,
            interrupt,
            push_result,
            event_signal_required,
            pending_interrupts: queue.number_interrupts(),
            missed_pdc0: queue.missed_pdc0(),
            missed_pdc1: queue.missed_pdc1(),
            error_code: queue.error_code(),
        };

        session.write_memory_bytes(queue_address, &queue.encode())?;
        Ok(report)
    }

    #[cfg(test)]
    fn dispatch_gsp_gx_command_queue_with_right(
        &mut self,
        queue: &mut GspGxCommandQueue,
        client_has_rendering_right: bool,
    ) -> GspGxCommandQueueReport {
        self.dispatch_gsp_gx_command_queue_with_client(queue, client_has_rendering_right, None)
    }

    fn dispatch_gsp_gx_command_queue_with_client(
        &mut self,
        queue: &mut GspGxCommandQueue,
        client_has_rendering_right: bool,
        client_context: Option<(u32, u8)>,
    ) -> GspGxCommandQueueReport {
        let mut report = GspGxCommandQueueReport {
            commands_consumed: 0,
            command_lists_submitted: 0,
            memory_fills_completed: 0,
            dma_requests_completed: 0,
            flush_source_requested: false,
            display_transfers_completed: 0,
            texture_copies_completed: 0,
            cache_regions_validated: 0,
            pending_commands: queue.command_count(),
            cache_flush_requested: false,
            gas_blend_update_requested: false,
            busy_check_requested: false,
            halted: queue.is_halted(),
            status: queue.status(),
            last_result: queue.last_result(),
            error: None,
        };

        if !queue.is_halted() {
            while let Some(command) = queue.consume_next() {
                report.commands_consumed = report.commands_consumed.saturating_add(1);
                report.busy_check_requested |= command.fail_if_busy;

                let dispatch_result = match command.command_id {
                    GSP_GX_COMMAND_REQUEST_DMA => {
                        let arguments = command
                            .request_dma()
                            .expect("command ID was checked immediately above");
                        report.flush_source_requested |= arguments.flush_source;
                        if client_has_rendering_right {
                            self.dispatch_gsp_dma_request(arguments).and_then(|()| {
                                report.dma_requests_completed =
                                    report.dma_requests_completed.saturating_add(1);
                                self.dispatch_gsp_interrupt(GspInterruptId::Dma)
                                    .map(|_| ())
                                    .map_err(|error| {
                                        GspGxCommandQueueError::ProcessMemory(error.to_string())
                                    })
                            })
                        } else {
                            Ok(())
                        }
                    }
                    GSP_GX_COMMAND_PROCESS_COMMAND_LIST => {
                        let arguments = command
                            .process_command_list()
                            .expect("command ID was checked immediately above");
                        report.cache_flush_requested |= arguments.flush_buffer;
                        report.gas_blend_update_requested |= arguments.update_gas_blend;
                        if client_has_rendering_right {
                            self.dispatch_gsp_process_command_list(arguments).map(|()| {
                                report.command_lists_submitted =
                                    report.command_lists_submitted.saturating_add(1);
                            })
                        } else {
                            Ok(())
                        }
                    }
                    GSP_GX_COMMAND_MEMORY_FILL => {
                        let arguments = command
                            .memory_fill()
                            .expect("command ID was checked immediately above");
                        if client_has_rendering_right {
                            self.dispatch_gsp_memory_fill(arguments)
                                .and_then(|completed_mask| {
                                    report.memory_fills_completed = report
                                        .memory_fills_completed
                                        .saturating_add(completed_mask.count_ones() as u8);
                                    let completion_interrupt = if completed_mask & 1 != 0 {
                                        Some(GspInterruptId::Psc0)
                                    } else if completed_mask & 2 != 0 {
                                        Some(GspInterruptId::Psc1)
                                    } else {
                                        None
                                    };
                                    if let Some(interrupt) = completion_interrupt {
                                        self.dispatch_gsp_interrupt(interrupt).map(|_| ()).map_err(
                                            |error| {
                                                GspGxCommandQueueError::ProcessMemory(
                                                    error.to_string(),
                                                )
                                            },
                                        )
                                    } else {
                                        Ok(())
                                    }
                                })
                        } else {
                            Ok(())
                        }
                    }
                    GSP_GX_COMMAND_DISPLAY_TRANSFER => {
                        let arguments = command
                            .display_transfer()
                            .expect("command ID was checked immediately above");
                        if client_has_rendering_right {
                            self.dispatch_gsp_display_transfer(arguments)
                                .and_then(|()| {
                                    if let Some((shared_memory_base, client_id)) = client_context {
                                        self.update_lcd_from_gsp_shared_memory(
                                            shared_memory_base,
                                            client_id,
                                        )?;
                                    }
                                    report.display_transfers_completed =
                                        report.display_transfers_completed.saturating_add(1);
                                    self.dispatch_gsp_interrupt(GspInterruptId::Ppf)
                                        .map(|_| ())
                                        .map_err(|error| {
                                            GspGxCommandQueueError::DisplayTransfer(
                                                error.to_string(),
                                            )
                                        })
                                })
                        } else {
                            Ok(())
                        }
                    }
                    GSP_GX_COMMAND_FLUSH_CACHE_REGIONS => {
                        let arguments = command
                            .flush_cache_regions()
                            .expect("command ID was checked immediately above");
                        report.cache_flush_requested = true;
                        if client_has_rendering_right {
                            self.validate_gsp_cache_regions(arguments, &mut report)
                        } else {
                            Err(GspGxCommandQueueError::RenderingRightRequired)
                        }
                    }
                    GSP_GX_COMMAND_TEXTURE_COPY => {
                        let arguments = command
                            .texture_copy()
                            .expect("command ID was checked immediately above");
                        if client_has_rendering_right {
                            self.dispatch_gsp_texture_copy(arguments).and_then(|()| {
                                report.texture_copies_completed =
                                    report.texture_copies_completed.saturating_add(1);
                                self.dispatch_gsp_interrupt(GspInterruptId::Ppf)
                                    .map(|_| ())
                                    .map_err(|error| {
                                        GspGxCommandQueueError::TextureCopy(error.to_string())
                                    })
                            })
                        } else {
                            Ok(())
                        }
                    }
                    command_id => Err(GspGxCommandQueueError::UnsupportedCommand { command_id }),
                };

                if let Err(error) = dispatch_result {
                    let result = match &error {
                        GspGxCommandQueueError::RenderingRightRequired => Some(RESULT_GSP_NO_RIGHT),
                        GspGxCommandQueueError::InvalidCacheRegionRange { .. }
                        | GspGxCommandQueueError::InvalidCommandListAddress { .. }
                        | GspGxCommandQueueError::InvalidCommandListSize { .. }
                        | GspGxCommandQueueError::InvalidMemoryFillRange { .. }
                        | GspGxCommandQueueError::InvalidDisplayTransferAddress { .. }
                        | GspGxCommandQueueError::InvalidDmaRange { .. }
                        | GspGxCommandQueueError::InvalidTextureCopyAddress { .. } => {
                            Some(GSP_INVALID_ADDRESS_RESULT)
                        }
                        _ => None,
                    };
                    queue.mark_error(result);
                    queue.halt();
                    report.error = Some(error);
                    break;
                }

                if command.stop_after {
                    queue.halt();
                    break;
                }
            }
        }

        report.pending_commands = queue.command_count();
        report.halted = queue.is_halted();
        report.status = queue.status();
        report.last_result = queue.last_result();
        report
    }

    fn framebuffer_info_error(reason: impl Into<String>) -> GspGxCommandQueueError {
        GspGxCommandQueueError::DisplayTransfer(format!("GSP framebuffer info: {}", reason.into()))
    }

    fn read_gsp_framebuffer_info(
        &mut self,
        shared_memory_base: u32,
        client_id: u8,
        panel: LcdPanel,
    ) -> std::result::Result<Option<PendingGspFramebufferInfo>, GspGxCommandQueueError> {
        if client_id >= crate::gpu::GSP_INTERRUPT_QUEUE_CLIENT_COUNT {
            return Err(Self::framebuffer_info_error(format!(
                "invalid client slot {client_id}"
            )));
        }
        let panel_offset = match panel {
            LcdPanel::Top => GSP_TOP_FRAMEBUFFER_INFO_OFFSET,
            LcdPanel::Bottom => GSP_BOTTOM_FRAMEBUFFER_INFO_OFFSET,
        };
        let client_offset = u32::from(client_id)
            .checked_mul(GSP_FRAMEBUFFER_INFO_CLIENT_STRIDE)
            .ok_or_else(|| Self::framebuffer_info_error("client offset overflowed"))?;
        let info_address = shared_memory_base
            .checked_add(panel_offset)
            .and_then(|address| address.checked_add(client_offset))
            .ok_or_else(|| Self::framebuffer_info_error("header address overflowed"))?;

        let mut header = [0u8; GSP_FRAMEBUFFER_INFO_HEADER_SIZE as usize];
        self.ncsd_session
            .as_mut()
            .ok_or_else(|| Self::framebuffer_info_error("ARM11 process session is unavailable"))?
            .read_memory_bytes(info_address, &mut header)
            .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;
        if header[1] & GSP_FRAMEBUFFER_INFO_NEW_DATA == 0 {
            return Ok(None);
        }

        let index = header[0];
        if index > 1 {
            return Err(Self::framebuffer_info_error(format!(
                "entry index {index} is outside the two-entry ring"
            )));
        }
        let entry_offset = GSP_FRAMEBUFFER_INFO_HEADER_SIZE
            .checked_add(u32::from(index) * GSP_FRAMEBUFFER_INFO_ENTRY_SIZE)
            .ok_or_else(|| Self::framebuffer_info_error("entry offset overflowed"))?;
        let entry_address = info_address
            .checked_add(entry_offset)
            .ok_or_else(|| Self::framebuffer_info_error("entry address overflowed"))?;
        let mut entry = [0u8; GSP_FRAMEBUFFER_INFO_ENTRY_SIZE as usize];
        self.ncsd_session
            .as_mut()
            .ok_or_else(|| Self::framebuffer_info_error("ARM11 process session is unavailable"))?
            .read_memory_bytes(entry_address, &mut entry)
            .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;

        let word = |index: usize| {
            u32::from_le_bytes(
                entry[index * 4..index * 4 + 4]
                    .try_into()
                    .expect("framebuffer-info entry contains seven complete words"),
            )
        };
        let active_buffer = word(0);
        if active_buffer > 1 {
            return Err(Self::framebuffer_info_error(format!(
                "active framebuffer index {active_buffer} is invalid"
            )));
        }
        let format = word(4);
        let format_id = format & 0x7;
        if format_id > 4 {
            return Err(Self::framebuffer_info_error(format!(
                "pixel format {format_id} is unsupported"
            )));
        }
        let interlace = (format >> 4) & 0x3;
        if interlace != 0 {
            return Err(Self::framebuffer_info_error(format!(
                "interlace mode {interlace} is not implemented"
            )));
        }

        Ok(Some(PendingGspFramebufferInfo {
            panel,
            flags_address: info_address + 1,
            cleared_flags: header[1] & !GSP_FRAMEBUFFER_INFO_NEW_DATA,
            active_buffer,
            left_address: word(1),
            right_address: word(2),
            stride: word(3),
            format,
            status: word(5),
        }))
    }

    fn resolve_gsp_framebuffer_info(
        &self,
        info: PendingGspFramebufferInfo,
    ) -> std::result::Result<ResolvedGspFramebufferInfo, GspGxCommandQueueError> {
        let (width, height) = match info.panel {
            LcdPanel::Top => (
                crate::video::TOP_SCREEN_WIDTH,
                crate::video::TOP_SCREEN_HEIGHT,
            ),
            LcdPanel::Bottom => (
                crate::video::BOTTOM_SCREEN_WIDTH,
                crate::video::BOTTOM_SCREEN_HEIGHT,
            ),
        };
        let bytes_per_pixel = match info.format & 0x7 {
            0 => 4usize,
            1 => 3,
            2..=4 => 2,
            _ => unreachable!("pixel format was validated while parsing"),
        };
        let row_bytes = width
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| Self::framebuffer_info_error("framebuffer row size overflowed"))?;
        if info.stride & 0x8000_0000 != 0
            || (info.stride != 0 && (info.stride & 0x7 != 0 || (info.stride as usize) < row_bytes))
        {
            return Err(Self::framebuffer_info_error(format!(
                "invalid {:?} stride {:#010x} for a {row_bytes}-byte row",
                info.panel, info.stride
            )));
        }
        let source_length = if info.stride == 0 {
            row_bytes
        } else {
            (height - 1)
                .checked_mul(info.stride as usize)
                .and_then(|offset| offset.checked_add(row_bytes))
                .ok_or_else(|| Self::framebuffer_info_error("framebuffer range overflowed"))?
        };
        let left_physical_address =
            self.resolve_gsp_physical_range(info.left_address, source_length)?;
        let right_physical_address = if info.panel == LcdPanel::Top {
            if info.right_address == 0 {
                left_physical_address
            } else {
                self.resolve_gsp_physical_range(info.right_address, source_length)?
            }
        } else {
            0
        };
        Ok(ResolvedGspFramebufferInfo {
            info,
            left_physical_address,
            right_physical_address,
        })
    }

    fn apply_gsp_framebuffer_info(
        &mut self,
        resolved: ResolvedGspFramebufferInfo,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        let panel = resolved.info.panel;
        let (left_register, right_register) = match resolved.info.active_buffer {
            0 => (PDC_FRAMEBUFFER_A_FIRST, PDC_FRAMEBUFFER_B_FIRST),
            1 => (PDC_FRAMEBUFFER_A_SECOND, PDC_FRAMEBUFFER_B_SECOND),
            _ => unreachable!("active framebuffer was validated while parsing"),
        };
        self.lcd
            .write_register(panel, left_register, resolved.left_physical_address)
            .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;
        if panel == LcdPanel::Top {
            self.lcd
                .write_register(panel, right_register, resolved.right_physical_address)
                .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;
        }
        for (register, value) in [
            (PDC_FRAMEBUFFER_FORMAT, resolved.info.format),
            (PDC_FRAMEBUFFER_STRIDE, resolved.info.stride),
            (PDC_FRAMEBUFFER_SELECT, resolved.info.status),
        ] {
            self.lcd
                .write_register(panel, register, value)
                .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;
        }
        Ok(())
    }

    fn toggle_lcd_framebuffer(
        &mut self,
        panel: LcdPanel,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        let status = self
            .lcd
            .read_register(panel, PDC_FRAMEBUFFER_SELECT)
            .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;
        self.lcd
            .write_register(panel, PDC_FRAMEBUFFER_SELECT, status ^ 1)
            .map_err(|error| Self::framebuffer_info_error(error.to_string()))
    }

    /// Applies newly published GSP framebuffer metadata after DisplayTransfer.
    ///
    /// The guest publishes two alternating metadata entries per panel. All
    /// selected framebuffer spans are translated and validated before either
    /// LCD controller is modified. A panel without new metadata toggles its
    /// next-buffer selector, matching the GSP's automatic buffer-swap behavior.
    fn update_lcd_from_gsp_shared_memory(
        &mut self,
        shared_memory_base: u32,
        client_id: u8,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        let top = self.read_gsp_framebuffer_info(shared_memory_base, client_id, LcdPanel::Top)?;
        let bottom =
            self.read_gsp_framebuffer_info(shared_memory_base, client_id, LcdPanel::Bottom)?;
        let resolved_top = top
            .map(|info| self.resolve_gsp_framebuffer_info(info))
            .transpose()?;
        let resolved_bottom = bottom
            .map(|info| self.resolve_gsp_framebuffer_info(info))
            .transpose()?;

        if let Some(info) = resolved_top {
            self.apply_gsp_framebuffer_info(info)?;
        } else {
            self.toggle_lcd_framebuffer(LcdPanel::Top)?;
        }
        if let Some(info) = resolved_bottom {
            self.apply_gsp_framebuffer_info(info)?;
        } else {
            self.toggle_lcd_framebuffer(LcdPanel::Bottom)?;
        }

        for info in [top, bottom].into_iter().flatten() {
            self.ncsd_session
                .as_mut()
                .ok_or_else(|| {
                    Self::framebuffer_info_error("ARM11 process session is unavailable")
                })?
                .write_memory_bytes(info.flags_address, &[info.cleared_flags])
                .map_err(|error| Self::framebuffer_info_error(error.to_string()))?;
        }
        Ok(())
    }

    /// Resolves one GSP-visible virtual address to the physical memory backing.
    ///
    /// VRAM is available at its physical address and at the legacy application
    /// alias. All other process addresses go through the DMA map populated by
    /// ControlMemory.
    fn resolve_gsp_physical_range(
        &self,
        address: u32,
        length: usize,
    ) -> std::result::Result<u32, GspGxCommandQueueError> {
        const GSP_VRAM_ALIAS: u32 = 0x1F00_0000;
        let end = u64::from(address)
            .checked_add(length as u64)
            .ok_or(GspGxCommandQueueError::InvalidDisplayTransferAddress { address })?;
        let start = u64::from(address);
        let physical_vram_end = u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
        let alias_vram_end = u64::from(GSP_VRAM_ALIAS) + u64::from(VRAM_SIZE);

        let physical_address = if start >= u64::from(VRAM_BASE) && end <= physical_vram_end {
            address
        } else if start >= u64::from(GSP_VRAM_ALIAS) && end <= alias_vram_end {
            VRAM_BASE + (address - GSP_VRAM_ALIAS)
        } else {
            self.gpu_address_space
                .resolve_physical_range(&self.physical_memory, address, length)
                .map_err(|_| GspGxCommandQueueError::InvalidDisplayTransferAddress { address })?
                .physical_address
        };
        self.physical_memory
            .validate_range(physical_address, length)
            .map_err(|_| GspGxCommandQueueError::InvalidDisplayTransferAddress { address })?;
        Ok(physical_address)
    }

    /// Executes one bounded GX RequestDMA operation.
    ///
    /// Both complete ranges are translated and validated before any memory is
    /// changed. Staging the source gives overlapping copies deterministic
    /// memmove semantics. When requested, the ARM11 source range is cleaned and
    /// invalidated before the DMA backend reads shared physical memory.
    fn dispatch_gsp_dma_request(
        &mut self,
        command: GspDmaRequest,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        if command.size_bytes == 0 {
            return Err(GspGxCommandQueueError::InvalidDmaRange {
                source_address: command.source_address,
                destination_address: command.destination_address,
                size_bytes: command.size_bytes,
            });
        }
        let size_bytes = command.size_bytes as usize;
        if size_bytes > MAX_GSP_DMA_BYTES {
            return Err(GspGxCommandQueueError::DmaTooLarge {
                size_bytes: command.size_bytes,
                maximum_bytes: MAX_GSP_DMA_BYTES,
            });
        }

        let source_physical = self
            .resolve_gsp_physical_range(command.source_address, size_bytes)
            .map_err(|_| GspGxCommandQueueError::InvalidDmaRange {
                source_address: command.source_address,
                destination_address: command.destination_address,
                size_bytes: command.size_bytes,
            })?;
        let destination_physical = self
            .resolve_gsp_physical_range(command.destination_address, size_bytes)
            .map_err(|_| GspGxCommandQueueError::InvalidDmaRange {
                source_address: command.source_address,
                destination_address: command.destination_address,
                size_bytes: command.size_bytes,
            })?;

        let source_is_vram = source_physical >= VRAM_BASE
            && u64::from(source_physical) < u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
        if command.flush_source && !source_is_vram {
            if let Some(session) = self.ncsd_session.as_mut() {
                session
                    .flush_process_data_cache(command.source_address, command.size_bytes)
                    .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))?;
                // DMA is an external observer: finish the CPU writeback queue
                // before staging the source bytes from physical memory.
                session
                    .drain_write_buffer()
                    .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))?;
            }
        }

        let mut staged = Vec::new();
        staged
            .try_reserve_exact(size_bytes)
            .map_err(|_| GspGxCommandQueueError::DmaAllocationFailed { size_bytes })?;
        staged.resize(size_bytes, 0);
        self.physical_memory
            .read(source_physical, &mut staged)
            .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))?;
        self.physical_memory
            .write(destination_physical, &staged)
            .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))
    }

    /// Executes one basic DisplayTransfer operation.
    ///
    /// The software backend completes the memory copy synchronously. Its caller
    /// queues the PPF event only after the destination bytes are committed.
    fn dispatch_gsp_display_transfer(
        &mut self,
        command: GspDisplayTransfer,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        if command.input_address & 7 != 0 {
            return Err(GspGxCommandQueueError::InvalidDisplayTransferAddress {
                address: command.input_address,
            });
        }
        if command.output_address & 7 != 0 {
            return Err(GspGxCommandQueueError::InvalidDisplayTransferAddress {
                address: command.output_address,
            });
        }

        let mut request = DisplayTransferRequest {
            input_address: command.input_address,
            output_address: command.output_address,
            input_width: command.input_width(),
            input_height: command.input_height(),
            output_width: command.output_width(),
            output_height: command.output_height(),
            flags: command.flags,
        };
        let (source_length, destination_length) = display_transfer_buffer_sizes(request)
            .map_err(|error| GspGxCommandQueueError::DisplayTransfer(error.to_string()))?;
        request.input_address =
            self.resolve_gsp_physical_range(command.input_address, source_length)?;
        request.output_address =
            self.resolve_gsp_physical_range(command.output_address, destination_length)?;

        execute_display_transfer(&mut self.physical_memory, request)
            .map(|_| ())
            .map_err(|error| GspGxCommandQueueError::DisplayTransfer(error.to_string()))
    }

    /// Validates the non-empty address ranges supplied to GX FlushCacheRegions.
    ///
    /// FCRAM spans are cleaned and invalidated through the owning ARM11 process.
    /// VRAM spans are validated only because this model does not cache device memory.
    fn validate_gsp_cache_regions(
        &mut self,
        command: GspFlushCacheRegions,
        report: &mut GspGxCommandQueueReport,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        for region in command.regions {
            if region.size_bytes == 0 {
                break;
            }
            let physical = self
                .resolve_gsp_physical_range(region.address, region.size_bytes as usize)
                .map_err(|_| GspGxCommandQueueError::InvalidCacheRegionRange {
                    address: region.address,
                    size_bytes: region.size_bytes,
                })?;
            let is_vram = physical >= VRAM_BASE
                && u64::from(physical) < u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
            if !is_vram {
                if let Some(session) = self.ncsd_session.as_mut() {
                    session
                        .flush_process_data_cache(region.address, region.size_bytes)
                        .map_err(|error| {
                            GspGxCommandQueueError::ProcessMemory(error.to_string())
                        })?;
                    session
                        .drain_write_buffer()
                        .map_err(|error| {
                            GspGxCommandQueueError::ProcessMemory(error.to_string())
                        })?;
                }
            }
            report.cache_regions_validated = report.cache_regions_validated.saturating_add(1);
        }
        Ok(())
    }

    /// Executes one raw GSP TextureCopy operation.
    ///
    /// The transfer engine copies bytes without format conversion. Gap mode
    /// is limited to equal per-row copy widths; unsupported or unmapped spans
    /// fail before destination memory is modified.
    fn dispatch_gsp_texture_copy(
        &mut self,
        command: GspTextureCopy,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        let request = TextureCopyRequest {
            source_address: command.source_address,
            destination_address: command.destination_address,
            size_bytes: command.size_bytes,
            source_line_width_and_gap: command.source_line_width_and_gap,
            destination_line_width_and_gap: command.destination_line_width_and_gap,
            flags: command.flags,
        };
        execute_texture_copy(&mut self.physical_memory, &self.gpu_address_space, request)
            .map(|_| ())
            .map_err(|error| match error {
                TextureCopyError::InvalidAddress { address }
                | TextureCopyError::MisalignedAddress { address } => {
                    GspGxCommandQueueError::InvalidTextureCopyAddress { address }
                }
                TextureCopyError::AddressOverflow { base, .. } => {
                    GspGxCommandQueueError::InvalidTextureCopyAddress { address: base }
                }
                other => GspGxCommandQueueError::TextureCopy(other.to_string()),
            })
    }

    /// Executes the two optional buffers of one GX MemoryFill command.
    ///
    /// The original 3DS exposes the fill units over VRAM. Both ranges are
    /// validated and prepared before either buffer is modified, preventing a
    /// malformed second buffer from leaving a partially completed command.
    fn dispatch_gsp_memory_fill(
        &mut self,
        fill: GspMemoryFill,
    ) -> std::result::Result<u8, GspGxCommandQueueError> {
        let first = self.prepare_gsp_memory_fill_buffer(fill.buffers[0])?;
        let second = self.prepare_gsp_memory_fill_buffer(fill.buffers[1])?;

        let mut completed_mask = 0u8;
        for (index, prepared) in [first, second].into_iter().enumerate() {
            if let Some((physical_address, bytes)) = prepared {
                self.physical_memory
                    .write(physical_address, &bytes)
                    .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))?;
                completed_mask |= 1 << index;
            }
        }
        Ok(completed_mask)
    }

    fn prepare_gsp_memory_fill_buffer(
        &self,
        buffer: GspMemoryFillBuffer,
    ) -> std::result::Result<Option<(u32, Vec<u8>)>, GspGxCommandQueueError> {
        if buffer.start_address == 0 || !buffer.is_triggered() {
            return Ok(None);
        }
        if buffer.start_address & 7 != 0
            || buffer.end_address & 7 != 0
            || buffer.start_address >= buffer.end_address
        {
            return Err(GspGxCommandQueueError::InvalidMemoryFillRange {
                start_address: buffer.start_address,
                end_address: buffer.end_address,
            });
        }

        let guest_vram_base = 0x1F00_0000u32;
        let guest_vram_end = u64::from(guest_vram_base) + u64::from(VRAM_SIZE);
        let physical_vram_end = u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
        let start = u64::from(buffer.start_address);
        let end = u64::from(buffer.end_address);
        let physical_address = if start >= u64::from(VRAM_BASE) && end <= physical_vram_end {
            buffer.start_address
        } else if start >= u64::from(guest_vram_base) && end <= guest_vram_end {
            VRAM_BASE + (buffer.start_address - guest_vram_base)
        } else {
            return Err(GspGxCommandQueueError::InvalidMemoryFillRange {
                start_address: buffer.start_address,
                end_address: buffer.end_address,
            });
        };

        let length = usize::try_from(end - start).map_err(|_| {
            GspGxCommandQueueError::InvalidMemoryFillRange {
                start_address: buffer.start_address,
                end_address: buffer.end_address,
            }
        })?;
        self.physical_memory
            .validate_range(physical_address, length)
            .map_err(|_| GspGxCommandQueueError::InvalidMemoryFillRange {
                start_address: buffer.start_address,
                end_address: buffer.end_address,
            })?;

        let element_width = buffer.element_width_bytes().ok_or(
            GspGxCommandQueueError::UnsupportedMemoryFillWidth {
                control: buffer.control,
            },
        )?;
        let pattern = buffer.fill_value.to_le_bytes();
        let mut bytes = vec![0u8; length];
        for chunk in bytes.chunks_mut(element_width) {
            chunk.copy_from_slice(&pattern[..chunk.len()]);
        }
        Ok(Some((physical_address, bytes)))
    }

    fn dispatch_gsp_process_command_list(
        &mut self,
        command: GspProcessCommandList,
    ) -> std::result::Result<(), GspGxCommandQueueError> {
        let address = command.buffer_address;
        let size_bytes = command.buffer_size_bytes;
        if address & 7 != 0 {
            return Err(GspGxCommandQueueError::InvalidCommandListAddress {
                address,
                size_bytes,
            });
        }
        if size_bytes == 0 || size_bytes & 7 != 0 {
            return Err(GspGxCommandQueueError::InvalidCommandListSize { size_bytes });
        }
        let maximum_bytes = MAX_PICA_COMMAND_WORDS * 4;
        if u64::from(size_bytes) > maximum_bytes as u64 {
            return Err(GspGxCommandQueueError::CommandListTooLarge {
                size_bytes,
                maximum_bytes,
            });
        }
        let end = u64::from(address) + u64::from(size_bytes);
        if end > 0x1_0000_0000 {
            return Err(GspGxCommandQueueError::AddressOverflow {
                base: address,
                offset: u64::from(size_bytes),
            });
        }

        let word_count = usize::try_from(size_bytes / 4).map_err(|_| {
            GspGxCommandQueueError::CommandListTooLarge {
                size_bytes,
                maximum_bytes,
            }
        })?;

        let physical_address =
            if address >= VRAM_BASE && end <= u64::from(VRAM_BASE) + u64::from(VRAM_SIZE) {
                self.physical_memory
                    .validate_range(address, size_bytes as usize)
                    .map_err(|_| GspGxCommandQueueError::InvalidCommandListAddress {
                        address,
                        size_bytes,
                    })?;
                address
            } else {
                self.gpu_address_space
                    .resolve_physical_range(&self.physical_memory, address, size_bytes as usize)
                    .map_err(|_| GspGxCommandQueueError::InvalidCommandListAddress {
                        address,
                        size_bytes,
                    })?
                    .physical_address
            };

        let command_list_is_vram = physical_address >= VRAM_BASE
            && u64::from(physical_address) < u64::from(VRAM_BASE) + u64::from(VRAM_SIZE);
        if command.flush_buffer && !command_list_is_vram {
            if let Some(session) = self.ncsd_session.as_mut() {
                session
                    .flush_process_data_cache(address, size_bytes)
                    .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))?;
                session
                    .drain_write_buffer()
                    .map_err(|error| GspGxCommandQueueError::ProcessMemory(error.to_string()))?;
            }
        }

        self.submit_pica_command_list_from_physical_memory(physical_address, word_count)
            .map_err(|error| GspGxCommandQueueError::CommandListSubmission {
                reason: error.to_string(),
            })?;
        Ok(())
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

        let cpu_report = self.run_instructions(instruction_budget)?;
        {
            let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
                "a prepared ARM11 process session is required for frame execution",
            ))?;
            session.advance_time(NOMINAL_FRAME_DURATION_NS);
            self.cpu = session.core().clone();
        }

        let gpu_command_lists_submitted = self.process_pending_pica_command_buffers()?;
        let lcd_report = self
            .lcd
            .scanout(&self.physical_memory, &mut self.display)
            .map_err(|error| {
                Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    error.to_string(),
                ))
            })?;
        let frame_number = self.display.present();
        self.lcd.advance_vblank();
        let gsp_interrupt_deliveries = self.dispatch_frame_vblank_interrupts()?;
        Ok(SystemFrameReport {
            frame_number,
            instructions_executed: cpu_report.instructions_executed,
            cpu_stop_reason: cpu_report.stop_reason,
            gpu_command_lists_submitted,
            lcd_screens_scanned_out: lcd_report.screens_updated,
            elapsed_virtual_time_ns: NOMINAL_FRAME_DURATION_NS,
            gsp_interrupt_deliveries,
        })
    }

    fn dispatch_frame_vblank_interrupts(&mut self) -> Result<u8> {
        let top = self
            .dispatch_gsp_interrupt(GspInterruptId::Pdc0)
            .map_err(|error| {
                Error::Io(io::Error::other(format!(
                    "GSP PDC0 interrupt delivery failed: {error}"
                )))
            })?;
        let bottom = self
            .dispatch_gsp_interrupt(GspInterruptId::Pdc1)
            .map_err(|error| {
                Error::Io(io::Error::other(format!(
                    "GSP PDC1 interrupt delivery failed: {error}"
                )))
            })?;

        let deliveries = top.deliveries.len().saturating_add(bottom.deliveries.len());
        Ok(u8::try_from(deliveries).unwrap_or(u8::MAX))
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
    use crate::gpu::{
        FCRAM_BASE, GSP_GX_COMMAND_QUEUE_HEADER_SIZE, GSP_INTERRUPT_QUEUE_CLIENT_STRIDE,
        GSP_INTERRUPT_QUEUE_IGNORE_PDC, REG_COMMAND_BUFFER_ADDRESS_0, REG_COMMAND_BUFFER_JUMP_0,
        REG_COMMAND_BUFFER_SIZE_0,
    };
    use crate::loader::ncsd::session::Arm11RunStopReason;
    use crate::video::{
        BOTTOM_SCREEN_WIDTH, LcdPanel, LcdPixelFormat, PDC_CONTROL, PDC_FRAMEBUFFER_A_FIRST,
        PDC_FRAMEBUFFER_A_SECOND, PDC_FRAMEBUFFER_B_FIRST, PDC_FRAMEBUFFER_B_SECOND,
        PDC_FRAMEBUFFER_FORMAT, PDC_FRAMEBUFFER_SELECT, PDC_FRAMEBUFFER_STRIDE, TOP_SCREEN_WIDTH,
    };

    #[test]
    fn running_without_an_image_fails() {
        let mut system = System::new();
        assert!(matches!(system.run_frame(), Err(Error::NoImageLoaded)));
    }

    fn ipc_program_ncsd_cartridge_fixture(instructions: &[u32]) -> Vec<u8> {
        const NCSD_PARTITION_OFFSET: usize = 0x4000;
        const EXEFS_OFFSET: usize = 0x2000;
        const EXEFS_HEADER_SIZE: usize = 0x200;
        const CODE_SIZE: usize = 0x3000;
        let mut image = clear_ncsd_cartridge_fixture();
        let exefs_start = NCSD_PARTITION_OFFSET + EXEFS_OFFSET;
        let code_start = exefs_start + EXEFS_HEADER_SIZE;
        for (index, instruction) in instructions.iter().copied().enumerate() {
            let offset = code_start + index * 4;
            image[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
        }

        // Rebuild the .code digest and the NCCH digest of the ExeFS header
        // after modifying the synthetic executable instructions.
        let code_hash = sha256_digest(&image[code_start..code_start + CODE_SIZE]);
        image[exefs_start + 0x1E0..exefs_start + 0x200].copy_from_slice(&code_hash);
        let exefs_header_hash = sha256_digest(&image[exefs_start..exefs_start + EXEFS_HEADER_SIZE]);
        let ncch = NCSD_PARTITION_OFFSET;
        image[ncch + 0x1C0..ncch + 0x1E0].copy_from_slice(&exefs_header_hash);
        image
    }

    fn ipc_svc_ncsd_cartridge_fixture(svc_count: usize) -> Vec<u8> {
        ipc_program_ncsd_cartridge_fixture(&vec![0xEF00_0032; svc_count])
    }

    fn run_one_ipc_svc(system: &mut System) {
        let report = system.run_instructions(1).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
    }

    fn write_tls_ipc_request(system: &mut System, session_handle: u32, words: &[u32]) -> u32 {
        let session = system
            .ncsd_session
            .as_mut()
            .expect("test process is loaded");
        let tls = session.current_tls_address().expect("main thread has TLS");
        let buffer = tls + ARM11_TLS_IPC_BUFFER_OFFSET;
        for (index, word) in words.iter().copied().enumerate() {
            session
                .write_memory_u32(buffer + u32::try_from(index * 4).unwrap(), word)
                .unwrap();
        }
        assert!(session.set_current_register(0, session_handle));
        buffer
    }

    fn read_tls_ipc_response(system: &mut System, buffer: u32, word_count: usize) -> Vec<u32> {
        let session = system
            .ncsd_session
            .as_mut()
            .expect("test process is loaded");
        (0..word_count)
            .map(|index| {
                session
                    .read_memory_u32(buffer + u32::try_from(index * 4).unwrap())
                    .unwrap()
            })
            .collect()
    }

    /// Reserves real FCRAM backing and maps the matching CPU and GSP aliases.
    fn map_test_linear_fcram(system: &mut System, size: u32) -> (u32, u32) {
        let physical_base = system
            .physical_memory_mut()
            .allocate_linear_fcram(size)
            .unwrap();
        let virtual_base = 0x1400_0000 + (physical_base - FCRAM_BASE);
        system
            .gpu_address_space_mut()
            .map_linear_region(virtual_base, size)
            .unwrap();
        system
            .ncsd_session_mut()
            .unwrap()
            .map_linear_memory_region(virtual_base, physical_base, size)
            .unwrap();
        (virtual_base, physical_base)
    }

    #[test]
    fn acquire_right_blocks_and_resumes_after_rendering_handoff() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(ipc_svc_ncsd_cartridge_fixture(1)),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (memory_handle, event_zero, event_one, gsp_handle) = {
            let session = system.ncsd_session.as_mut().unwrap();
            let memory_handle = session.create_memory_block(0x1000, 3, 3).unwrap();
            session
                .map_memory_block(memory_handle, GSP_SHARED_MEMORY_BASE, 3, 0x1000_0000)
                .unwrap();
            let event_zero = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            let event_one = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            let gsp_handle = session.create_gsp_gpu_session().unwrap();
            (memory_handle, event_zero, event_one, gsp_handle)
        };

        // Two synthetic clients share the same process-mapped GSP block.
        system
            .register_gsp_client(0, GSP_SHARED_MEMORY_BASE, event_zero)
            .unwrap();
        system
            .register_gsp_client(1, GSP_SHARED_MEMORY_BASE, event_one)
            .unwrap();
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .set_gsp_session_registration(
                gsp_handle,
                Some(1),
                Some(memory_handle),
                Some(event_one),
                0,
            )
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        // Client 1 issues AcquireRight while client 0 owns the rendering token.
        let request_buffer =
            write_tls_ipc_request(&mut system, gsp_handle, &[0x0016_0042, 0, 0, 0]);
        run_one_ipc_svc(&mut system);
        assert_eq!(system.ncsd_session().unwrap().current_thread_id(), None);
        assert_eq!(
            system.pending_gsp_acquires,
            vec![PendingGspAcquire {
                client_id: 1,
                thread_id: crate::loader::ncsd::session::MAIN_THREAD_ID,
            }]
        );

        // Releasing client 0 transfers ownership and completes the saved TLS reply.
        let handoff = system.release_gsp_rendering_right(0).unwrap();
        assert_eq!(handoff.next_owner, Some(1));
        let session = system.ncsd_session.as_ref().unwrap();
        assert_eq!(
            session.current_thread_id(),
            Some(crate::loader::ncsd::session::MAIN_THREAD_ID)
        );
        assert_eq!(session.core().regs[0], 0);
        assert!(system.gsp_service_registry().owns_rendering_right(1));
        assert_eq!(
            read_tls_ipc_response(&mut system, request_buffer, 2),
            vec![0x0016_0040, 0]
        );
        assert!(system.pending_gsp_acquires.is_empty());

        system.release_gsp_rendering_right(1).unwrap();
    }
    #[test]
    fn service_manager_rejects_lookup_before_client_registration() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(ipc_svc_ncsd_cartridge_fixture(1)),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        let service_manager = system
            .ncsd_session
            .as_mut()
            .unwrap()
            .create_service_manager_session()
            .unwrap();

        let response_buffer = write_tls_ipc_request(
            &mut system,
            service_manager,
            &[0x0005_0100, 0x3A70_7367, 0x7570_473A, 8, 0],
        );
        run_one_ipc_svc(&mut system);

        assert_eq!(
            read_tls_ipc_response(&mut system, response_buffer, 2),
            vec![0x0005_0040, RESULT_INVALID_HANDLE]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .service_manager_client_registered(service_manager),
            Some(false)
        );
    }

    #[test]
    fn service_manager_connects_gsp_and_registers_shared_relay_memory() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(ipc_program_ncsd_cartridge_fixture(&[
                    0xEF00_002D, // SVC ConnectToPort
                    0xEF00_0032, // srv:RegisterClient
                    0xEF00_0032, // srv:GetServiceHandle
                    0xEF00_0032, // GSP RegisterInterruptRelayQueue
                    0xEF00_0032, // GSP TryAcquireRight
                    0xEF00_0032, // GSP TriggerCmdReqQueue (first batch)
                    0xEF00_0032, // GSP TriggerCmdReqQueue (resume halted queue)
                    0xEF00_0032, // GSP TriggerCmdReqQueue (MemoryFill, both buffers)
                    0xEF00_0032, // GSP TriggerCmdReqQueue (buffer 1 only / PSC1)
                    0xEF00_0032, // GSP ReleaseRight
                    0xEF00_0032, // GSP UnregisterInterruptRelayQueue
                ])),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let event_handle = system
            .ncsd_session
            .as_mut()
            .unwrap()
            .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
            .unwrap();

        // ConnectToPort("srv:") creates the service-manager client handle.
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .write_memory_bytes(0x0010_2000, b"srv:")
            .unwrap();
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .set_current_register(0, 0x0010_2010);
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .set_current_register(1, 0x0010_2000);
        run_one_ipc_svc(&mut system);
        let service_manager = system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_u32(0x0010_2010)
            .unwrap();
        assert!(matches!(
            system
                .ncsd_session()
                .unwrap()
                .service_session_kind(service_manager),
            Some(ServiceSessionKind::ServiceManager {
                client_registered: false,
            })
        ));

        // srv:RegisterClient supplies the process-ID translation descriptor.
        let register_client_buffer =
            write_tls_ipc_request(&mut system, service_manager, &[0x0001_0002, 0x20, 0]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, register_client_buffer, 2),
            vec![0x0001_0040, 0]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .service_manager_client_registered(service_manager),
            Some(true)
        );

        // srv:GetServiceHandle("gsp::Gpu", 8, 0)
        let srv_buffer = write_tls_ipc_request(
            &mut system,
            service_manager,
            &[0x0005_0100, 0x3A70_7367, 0x7570_473A, 8, 0],
        );
        run_one_ipc_svc(&mut system);
        let srv_response = read_tls_ipc_response(&mut system, srv_buffer, 4);
        assert_eq!(srv_response[0], 0x0005_0042);
        assert_eq!(srv_response[1], 0);
        assert_eq!(srv_response[2], 0); // COPY_HANDLE descriptor
        let gsp_handle = srv_response[3];
        assert_ne!(gsp_handle, 0);
        assert!(matches!(
            system
                .ncsd_session()
                .unwrap()
                .service_session_kind(gsp_handle),
            Some(ServiceSessionKind::GspGpu {
                client_id: None,
                ..
            })
        ));

        // gsp::Gpu:RegisterInterruptRelayQueue(flags=0, event_handle)
        let register_buffer =
            write_tls_ipc_request(&mut system, gsp_handle, &[0x0013_0042, 0, 0, event_handle]);
        run_one_ipc_svc(&mut system);
        let register_response = read_tls_ipc_response(&mut system, register_buffer, 5);
        assert_eq!(register_response[0], 0x0013_0082);
        assert_eq!(register_response[1], 0);
        assert_eq!(register_response[2], 0); // First free GSP client slot
        assert_eq!(register_response[3], 0); // COPY_HANDLE descriptor
        let shared_memory_handle = register_response[4];
        assert_ne!(shared_memory_handle, 0);
        assert_eq!(system.gsp_service_registry().registered_client_count(), 1);

        // The guest's MapMemoryBlock call after the IPC response is idempotent
        // for this exact block/address, but does not accept another mapping.
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .map_memory_block(shared_memory_handle, GSP_SHARED_MEMORY_BASE, 3, 0x1000_0000)
            .unwrap();

        // Acquire rendering rights through the guest GSP IPC request.
        let acquire_buffer = write_tls_ipc_request(&mut system, gsp_handle, &[0x0015_0002, 0, 0]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, acquire_buffer, 2),
            vec![0x0015_0040, 0]
        );
        assert!(system.gsp_service_registry().owns_rendering_right(0));

        // Two commands are queued. The first halts after execution; the second
        // must remain pending until a second TriggerCmdReqQueue IPC arrives.
        let first_command_address = VRAM_BASE + 0x1000;
        let second_command_address = VRAM_BASE + 0x1010;
        system
            .physical_memory_mut()
            .write_u32(first_command_address, 0xDEAD_BEEF)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(first_command_address + 4, 0x000F_0100)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(second_command_address, 0xCAFE_BABE)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(second_command_address + 4, 0x000F_0101)
            .unwrap();

        let queue_address =
            GSP_SHARED_MEMORY_BASE + GspGxCommandQueue::client_queue_offset(0).unwrap();
        let mut gx_queue = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        gx_queue[1] = 2;
        for (slot, command_address, stop_after) in [
            (0usize, first_command_address, true),
            (1usize, second_command_address, false),
        ] {
            let entry = GSP_GX_COMMAND_QUEUE_HEADER_SIZE + slot * 0x20;
            gx_queue[entry] = GSP_GX_COMMAND_PROCESS_COMMAND_LIST;
            gx_queue[entry + 2] = u8::from(stop_after);
            gx_queue[entry + 4..entry + 8].copy_from_slice(&command_address.to_le_bytes());
            gx_queue[entry + 8..entry + 12].copy_from_slice(&8u32.to_le_bytes());
        }
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .write_memory_bytes(queue_address, &gx_queue)
            .unwrap();

        // First trigger submits register 0x100 and leaves one command halted.
        let trigger_buffer = write_tls_ipc_request(&mut system, gsp_handle, &[0x000C_0000]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, trigger_buffer, 2),
            vec![0x000C_0040, 0]
        );
        assert_eq!(system.gpu().register(0x100), Some(0xDEAD_BEEF));
        let mut queue_header = [0u8; GSP_GX_COMMAND_QUEUE_HEADER_SIZE];
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(queue_address, &mut queue_header)
            .unwrap();
        assert_eq!(queue_header[0], 1);
        assert_eq!(queue_header[1], 1);
        assert_ne!(queue_header[2] & 0x01, 0);
        assert_ne!(queue_header[3] & 0x01, 0);

        // Second trigger clears the halt latch and submits the remaining command.
        let trigger_buffer = write_tls_ipc_request(&mut system, gsp_handle, &[0x000C_0000]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, trigger_buffer, 2),
            vec![0x000C_0040, 0]
        );
        assert_eq!(system.gpu().register(0x101), Some(0xCAFE_BABE));
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(queue_address, &mut queue_header)
            .unwrap();
        assert_eq!(queue_header[0], 2);
        assert_eq!(queue_header[1], 0);
        assert_eq!(queue_header[2] & 0x01, 0);
        assert_eq!(queue_header[3] & 0x01, 0);

        // GX MemoryFill writes both VRAM buffers through TriggerCmdReqQueue.
        let fill_start_0 = VRAM_BASE + 0x2000;
        let fill_end_0 = fill_start_0 + 0x10;
        let fill_start_1 = 0x1F00_2020;
        let fill_end_1 = fill_start_1 + 0x18;
        let mut fill_queue = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        fill_queue[0] = 2;
        fill_queue[1] = 1;
        let fill_entry = GSP_GX_COMMAND_QUEUE_HEADER_SIZE + 2 * 0x20;
        fill_queue[fill_entry] = GSP_GX_COMMAND_MEMORY_FILL;
        for (index, value) in [
            fill_start_0,
            0x1122_3344,
            fill_end_0,
            fill_start_1,
            0x00A1_B2C3,
            fill_end_1,
            0x0101_0201,
        ]
        .into_iter()
        .enumerate()
        {
            let offset = fill_entry + 4 + index * 4;
            fill_queue[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .write_memory_bytes(queue_address, &fill_queue)
            .unwrap();

        let fill_trigger_buffer = write_tls_ipc_request(&mut system, gsp_handle, &[0x000C_0000]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, fill_trigger_buffer, 2),
            vec![0x000C_0040, 0]
        );
        let mut fill_0_bytes = [0u8; 16];
        system
            .physical_memory()
            .read(fill_start_0, &mut fill_0_bytes)
            .unwrap();
        assert_eq!(
            fill_0_bytes,
            [
                0x44, 0x33, 0x22, 0x11, 0x44, 0x33, 0x22, 0x11, 0x44, 0x33, 0x22, 0x11, 0x44, 0x33,
                0x22, 0x11,
            ]
        );
        let mut fill_1_bytes = [0u8; 24];
        system
            .physical_memory()
            .read(VRAM_BASE + 0x2020, &mut fill_1_bytes)
            .unwrap();
        assert_eq!(
            fill_1_bytes,
            [
                0xC3, 0xB2, 0xA1, 0xC3, 0xB2, 0xA1, 0xC3, 0xB2, 0xA1, 0xC3, 0xB2, 0xA1, 0xC3, 0xB2,
                0xA1, 0xC3, 0xB2, 0xA1, 0xC3, 0xB2, 0xA1, 0xC3, 0xB2, 0xA1,
            ]
        );
        let mut relay_queue_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(GSP_SHARED_MEMORY_BASE, &mut relay_queue_bytes)
            .unwrap();
        let relay_queue = GspInterruptRelayQueue::parse(&relay_queue_bytes).unwrap();
        assert_eq!(
            relay_queue.pending_interrupts().unwrap(),
            vec![GspInterruptId::Psc0]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .event_handle_is_signaled(event_handle),
            Some(true)
        );

        // Buffer 1 alone uses the PSC1 fill unit and the 16-bit fill mode.
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .clear_event_handle(event_handle)
            .unwrap();
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .write_memory_bytes(
                GSP_SHARED_MEMORY_BASE,
                &GspInterruptRelayQueue::new().encode(),
            )
            .unwrap();
        let fill_start_1_only = 0x1F00_2040;
        let fill_end_1_only = fill_start_1_only + 0x10;
        let mut fill_queue_1_only = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        fill_queue_1_only[0] = 3;
        fill_queue_1_only[1] = 1;
        let fill_entry_1_only = GSP_GX_COMMAND_QUEUE_HEADER_SIZE + 3 * 0x20;
        fill_queue_1_only[fill_entry_1_only] = GSP_GX_COMMAND_MEMORY_FILL;
        for (index, value) in [
            0u32,
            0,
            0,
            fill_start_1_only,
            0x0000_ABCD,
            fill_end_1_only,
            0x0001_0000,
        ]
        .into_iter()
        .enumerate()
        {
            let offset = fill_entry_1_only + 4 + index * 4;
            fill_queue_1_only[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .write_memory_bytes(queue_address, &fill_queue_1_only)
            .unwrap();

        let fill_1_only_trigger = write_tls_ipc_request(&mut system, gsp_handle, &[0x000C_0000]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, fill_1_only_trigger, 2),
            vec![0x000C_0040, 0]
        );
        let mut fill_1_only_bytes = [0u8; 16];
        system
            .physical_memory()
            .read(VRAM_BASE + 0x2040, &mut fill_1_only_bytes)
            .unwrap();
        assert_eq!(
            fill_1_only_bytes,
            [
                0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB, 0xCD, 0xAB,
                0xCD, 0xAB,
            ]
        );
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(GSP_SHARED_MEMORY_BASE, &mut relay_queue_bytes)
            .unwrap();
        let relay_queue_1_only = GspInterruptRelayQueue::parse(&relay_queue_bytes).unwrap();
        assert_eq!(
            relay_queue_1_only.pending_interrupts().unwrap(),
            vec![GspInterruptId::Psc1]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .event_handle_is_signaled(event_handle),
            Some(true)
        );

        // ReleaseRight must travel through the same guest IPC path.
        let release_buffer = write_tls_ipc_request(&mut system, gsp_handle, &[0x0017_0000]);
        run_one_ipc_svc(&mut system);
        assert_eq!(
            read_tls_ipc_response(&mut system, release_buffer, 2),
            vec![0x0017_0040, 0]
        );
        assert_eq!(system.gsp_service_registry().rendering_right_owner(), None);

        // Unregister releases the queue mapping and memory handle.
        let unregister_buffer = write_tls_ipc_request(&mut system, gsp_handle, &[0x0014_0000]);
        run_one_ipc_svc(&mut system);
        let unregister_response = read_tls_ipc_response(&mut system, unregister_buffer, 2);
        assert_eq!(unregister_response, vec![0x0014_0040, 0]);
        assert_eq!(system.gsp_service_registry().registered_client_count(), 0);
        assert!(matches!(
            system
                .ncsd_session()
                .unwrap()
                .service_session_kind(gsp_handle),
            Some(ServiceSessionKind::GspGpu {
                client_id: None,
                shared_memory_handle: None,
                event_handle: None,
                ..
            })
        ));
    }

    #[test]
    fn submits_pica_commands_from_guest_memory_into_register_file() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        system
            .physical_memory_mut()
            .write_u32(FCRAM_BASE + 0x2000, 0xDEAD_BEEF)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(FCRAM_BASE + 0x2004, 0x000F_0100)
            .unwrap();

        let report = system
            .submit_pica_command_list_from_memory(FCRAM_BASE + 0x2000, 2)
            .unwrap();
        assert_eq!(report.commands, 1);
        assert_eq!(report.register_writes, 1);
        assert_eq!(system.gpu().register(0x100), Some(0xDEAD_BEEF));
    }

    #[test]
    fn submits_pica_commands_from_registered_linear_memory() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        system.map_gpu_linear_region(0x1400_2000, 0x1000).unwrap();
        system
            .physical_memory_mut()
            .write_u32(FCRAM_BASE + 0x2000, 0xCAFE_BABE)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(FCRAM_BASE + 0x2004, 0x000F_0100)
            .unwrap();

        let report = system
            .submit_pica_command_list_from_linear_memory(0x1400_2000, 2)
            .unwrap();
        assert_eq!(report.register_writes, 1);
        assert_eq!(system.gpu().register(0x100), Some(0xCAFE_BABE));
    }

    #[test]
    fn linear_command_submission_rejects_unregistered_virtual_addresses() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        assert!(
            system
                .submit_pica_command_list_from_linear_memory(0x1400_2000, 2)
                .is_err()
        );
        assert_eq!(system.gpu().register(0x100), Some(0));
    }

    #[test]
    fn invalid_guest_gpu_command_range_does_not_change_registers() {
        let mut system = System::new();
        assert!(matches!(
            system.submit_pica_command_list_from_memory(0x0010_2000, 2),
            Err(Error::Unimplemented(_))
        ));
    }

    #[test]
    fn gsp_interrupt_delivery_updates_client_shared_memory_and_requests_event_signal() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (shared_memory_base, queue_address) = {
            let session = system.ncsd_session.as_mut().unwrap();
            let stack = session.address_space().layout().stack;
            let shared_block = [0u8; GSP_INTERRUPT_QUEUE_CLIENT_STRIDE as usize
                * crate::gpu::GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize];
            assert!(stack.mapped_size_bytes as usize >= shared_block.len());
            session
                .write_memory_bytes(stack.address, &shared_block)
                .unwrap();
            (
                stack.address,
                stack.address + GSP_INTERRUPT_QUEUE_CLIENT_STRIDE * 2,
            )
        };

        let report = system
            .enqueue_gsp_interrupt_for_client(shared_memory_base, 2, GspInterruptId::P3d)
            .unwrap();
        assert_eq!(report.client_id, 2);
        assert_eq!(report.interrupt, GspInterruptId::P3d);
        assert_eq!(
            report.push_result,
            GspInterruptQueuePushResult::Queued { slot: 0 }
        );
        assert!(report.event_signal_required);
        assert_eq!(report.pending_interrupts, 1);
        assert_eq!(report.error_code, 0);

        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(queue_address, &mut bytes)
            .unwrap();
        let queue = GspInterruptRelayQueue::parse(&bytes).unwrap();
        assert_eq!(queue.pending_interrupts().unwrap(), [GspInterruptId::P3d]);
    }

    #[test]
    fn gsp_interrupt_delivery_respects_guest_ignore_pdc_flag() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (shared_memory_base, queue_address) = {
            let session = system.ncsd_session.as_mut().unwrap();
            let stack = session.address_space().layout().stack;
            let mut shared_block = [0u8; GSP_INTERRUPT_QUEUE_CLIENT_STRIDE as usize
                * crate::gpu::GSP_INTERRUPT_QUEUE_CLIENT_COUNT as usize];
            shared_block[GSP_INTERRUPT_QUEUE_CLIENT_STRIDE as usize + 3] =
                GSP_INTERRUPT_QUEUE_IGNORE_PDC;
            session
                .write_memory_bytes(stack.address, &shared_block)
                .unwrap();
            (
                stack.address,
                stack.address + GSP_INTERRUPT_QUEUE_CLIENT_STRIDE,
            )
        };

        let report = system
            .enqueue_gsp_interrupt_for_client(shared_memory_base, 1, GspInterruptId::Pdc0)
            .unwrap();
        assert_eq!(report.push_result, GspInterruptQueuePushResult::IgnoredPdc);
        assert!(!report.event_signal_required);
        assert_eq!(report.pending_interrupts, 0);
        assert_eq!(report.missed_pdc0, 0);

        let mut bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(queue_address, &mut bytes)
            .unwrap();
        let queue = GspInterruptRelayQueue::parse(&bytes).unwrap();
        assert!(queue.ignores_pdc());
        assert!(queue.pending_interrupts().unwrap().is_empty());
    }

    #[test]
    fn gsp_gx_request_dma_copies_linear_memory_to_vram_and_signals_dma() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        // RequestDMA accepts byte-granular spans. The legacy linear alias maps
        // to the same FCRAM backing used by the ARM11 process and GPU.
        let (linear_virtual_base, _linear_physical_base) =
            map_test_linear_fcram(&mut system, 0x1000);
        let source_virtual = linear_virtual_base + 0x11;
        let destination_virtual: u32 = VRAM_BASE + 0x20_003;
        let expected = [
            0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6, 0x17, 0x28, 0x39, 0x4A, 0x5B, 0x6C, 0x7D,
        ];
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_bytes(source_virtual, &expected)
            .unwrap();
        // The source bytes are dirty in the ARM11 cache; RequestDMA's flush flag
        // must write them back before the GPU/DMA engine reads physical memory.

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session_mut().unwrap();
            let stack = session.address_space().layout().stack;
            assert_eq!(stack.address & 0xFFF, 0);
            assert!(stack.mapped_size_bytes as usize >= GSP_SHARED_MEMORY_SIZE);
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_REQUEST_DMA;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&source_virtual.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&destination_virtual.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&(expected.len() as u32).to_le_bytes());
        queue_bytes[entry + 28..entry + 32].copy_from_slice(&1u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.dma_requests_completed, 1);
        assert!(report.flush_source_requested);
        assert_eq!(report.pending_commands, 0);
        assert!(report.halted);
        assert!(report.error.is_none());

        let mut actual = [0u8; 13];
        system
            .physical_memory()
            .read(destination_virtual, &mut actual)
            .unwrap();
        assert_eq!(actual, expected);

        // DMA is signaled only after the staged destination write completes.
        let interrupt_queue_address =
            shared_memory_base + GspInterruptRelayQueue::client_queue_offset(0).unwrap();
        let mut interrupt_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(interrupt_queue_address, &mut interrupt_bytes)
            .unwrap();
        let interrupt_queue = GspInterruptRelayQueue::parse(&interrupt_bytes).unwrap();
        assert_eq!(
            interrupt_queue.pending_interrupts().unwrap(),
            [GspInterruptId::Dma]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .event_handle_is_signaled(event_handle),
            Some(true)
        );
    }

    #[test]
    fn gsp_gx_request_dma_requires_cpu_invalidation_after_device_write() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (source_virtual, source_physical) = map_test_linear_fcram(&mut system, 0x1000);
        let (destination_virtual, destination_physical) =
            map_test_linear_fcram(&mut system, 0x1000);
        let source_address = source_virtual + 0x100;
        let destination_address = destination_virtual + 0x100;
        let expected = 0xA1B2_C3D4u32;

        {
            let session = system.ncsd_session_mut().unwrap();
            session.write_memory_u32(source_address, expected).unwrap();
            // Prime a clean CPU cache line for the DMA destination before the
            // device writes it. The line must remain stale until invalidated.
            assert_eq!(session.read_memory_u32(destination_address).unwrap(), 0);
        }

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session_mut().unwrap();
            let stack = session.address_space().layout().stack;
            assert_eq!(stack.address & 0xFFF, 0);
            assert!(stack.mapped_size_bytes as usize >= GSP_SHARED_MEMORY_SIZE);
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_REQUEST_DMA;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&source_address.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&destination_address.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&4u32.to_le_bytes());
        queue_bytes[entry + 28..entry + 32].copy_from_slice(&1u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.dma_requests_completed, 1);
        assert!(report.flush_source_requested);
        assert!(report.error.is_none());

        // The source flush makes the CPU's dirty bytes visible to DMA.
        assert_eq!(
            system
                .physical_memory()
                .read_u32(source_physical + 0x100)
                .unwrap(),
            expected
        );
        // DMA commits the destination to shared physical memory.
        assert_eq!(
            system
                .physical_memory()
                .read_u32(destination_physical + 0x100)
                .unwrap(),
            expected
        );

        let session = system.ncsd_session_mut().unwrap();
        // Device writes do not snoop or silently invalidate the CPU's clean line.
        assert_eq!(session.read_memory_u32(destination_address).unwrap(), 0);
        assert_eq!(
            session
                .invalidate_process_data_cache(destination_address, 4)
                .unwrap(),
            1
        );
        assert_eq!(
            session.read_memory_u32(destination_address).unwrap(),
            expected
        );
    }

    #[test]
    fn gsp_gx_request_dma_cache_coherence_uses_physical_identity_across_aliases() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (old_virtual_base, physical_base) = map_test_linear_fcram(&mut system, 0x1000);
        let new_virtual_base = 0x3000_0000 + (physical_base - FCRAM_BASE);
        system
            .ncsd_session_mut()
            .unwrap()
            .map_linear_memory_region(new_virtual_base, physical_base, 0x1000)
            .unwrap();

        let source_address = old_virtual_base + 0x100;
        let destination_old_alias = old_virtual_base + 0x200;
        let destination_new_alias = new_virtual_base + 0x200;
        let expected = 0x5A6B_7C8Du32;

        {
            let session = system.ncsd_session_mut().unwrap();
            session.write_memory_u32(source_address, expected).unwrap();
            // Prime the destination through the new alias. Both aliases must
            // resolve to one physical cache line in the ARM11 data cache.
            assert_eq!(session.read_memory_u32(destination_new_alias).unwrap(), 0);
            assert_eq!(session.address_space().data_cache_line_count(), 2);
        }

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session_mut().unwrap();
            let stack = session.address_space().layout().stack;
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_REQUEST_DMA;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&source_address.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&destination_old_alias.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&4u32.to_le_bytes());
        queue_bytes[entry + 28..entry + 32].copy_from_slice(&1u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.dma_requests_completed, 1);
        assert!(report.flush_source_requested);
        assert!(report.error.is_none());

        assert_eq!(
            system
                .physical_memory()
                .read_u32(physical_base + 0x100)
                .unwrap(),
            expected
        );
        assert_eq!(
            system
                .physical_memory()
                .read_u32(physical_base + 0x200)
                .unwrap(),
            expected
        );

        let session = system.ncsd_session_mut().unwrap();
        // RequestDMA's flush_source writes back and removes the dirty source
        // line. The unrelated destination line remains resident and clean.
        assert_eq!(session.address_space().data_cache_line_count(), 1);
        // The DMA write is visible through physical backing, but the clean
        // destination line remains stale regardless of which alias reads it.
        assert_eq!(session.read_memory_u32(destination_old_alias).unwrap(), 0);
        assert_eq!(session.read_memory_u32(destination_new_alias).unwrap(), 0);
        assert_eq!(
            session
                .invalidate_process_data_cache(destination_new_alias, 4)
                .unwrap(),
            1
        );
        assert_eq!(session.address_space().data_cache_line_count(), 0);
        // Invalidating through one alias exposes the DMA write through both.
        assert_eq!(
            session.read_memory_u32(destination_old_alias).unwrap(),
            expected
        );
        assert_eq!(
            session.read_memory_u32(destination_new_alias).unwrap(),
            expected
        );
        assert_eq!(session.address_space().data_cache_line_count(), 1);
    }

    #[test]
    fn gsp_gx_request_dma_is_consumed_without_rendering_rights() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        let destination = VRAM_BASE + 0x20_000;
        let before = [0xCC; 8];
        system
            .physical_memory_mut()
            .write(destination, &before)
            .unwrap();

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_REQUEST_DMA;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&0x1400_0000u32.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&destination.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&8u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, false);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.dma_requests_completed, 0);
        assert!(report.error.is_none());

        let mut after = [0u8; 8];
        system
            .physical_memory()
            .read(destination, &mut after)
            .unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn gsp_gx_display_transfer_executes_tiled_to_linear_and_reports_completion() {
        use crate::gpu::{DisplayTransferRequest, execute_display_transfer};

        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session.as_mut().unwrap();
            let stack = session.address_space().layout().stack;
            assert_eq!(stack.address & 0xFFF, 0);
            assert!(stack.mapped_size_bytes as usize >= GSP_SHARED_MEMORY_SIZE);
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let top_left_framebuffer = VRAM_BASE + 0x100_000;
        let top_right_framebuffer = VRAM_BASE + 0x180_000;
        let bottom_framebuffer = VRAM_BASE + 0x200_000;
        fn encode_framebuffer_info(
            active_buffer: u32,
            left: u32,
            right: u32,
            stride: u32,
            format: u32,
            status: u32,
        ) -> [u8; GSP_FRAMEBUFFER_INFO_ENTRY_SIZE as usize] {
            let words = [active_buffer, left, right, stride, format, status, 0];
            let mut bytes = [0u8; GSP_FRAMEBUFFER_INFO_ENTRY_SIZE as usize];
            for (index, word) in words.into_iter().enumerate() {
                bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
            bytes
        }
        {
            let session = system.ncsd_session.as_mut().unwrap();
            let top_info = shared_memory_base + GSP_TOP_FRAMEBUFFER_INFO_OFFSET;
            let top_entry =
                top_info + GSP_FRAMEBUFFER_INFO_HEADER_SIZE + GSP_FRAMEBUFFER_INFO_ENTRY_SIZE;
            session
                .write_memory_bytes(
                    top_entry,
                    &encode_framebuffer_info(
                        1,
                        top_left_framebuffer,
                        top_right_framebuffer,
                        (TOP_SCREEN_WIDTH * 4) as u32,
                        0,
                        1,
                    ),
                )
                .unwrap();
            session.write_memory_bytes(top_info, &[1, 1, 0, 0]).unwrap();

            let bottom_info = shared_memory_base + GSP_BOTTOM_FRAMEBUFFER_INFO_OFFSET;
            let bottom_entry = bottom_info + GSP_FRAMEBUFFER_INFO_HEADER_SIZE;
            session
                .write_memory_bytes(
                    bottom_entry,
                    &encode_framebuffer_info(
                        0,
                        bottom_framebuffer,
                        0,
                        (BOTTOM_SCREEN_WIDTH * 4) as u32,
                        0,
                        0,
                    ),
                )
                .unwrap();
            session
                .write_memory_bytes(bottom_info, &[0, 1, 0, 0])
                .unwrap();
        }

        let linear_source = VRAM_BASE + 0x20_000;
        let tiled_source = VRAM_BASE + 0x40_000;
        let linear_output = VRAM_BASE + 0x60_000;
        let mut expected = vec![0u8; 64 * 64 * 4];
        for y in 0..64usize {
            for x in 0..64usize {
                let offset = (y * 64 + x) * 4;
                expected[offset..offset + 4].copy_from_slice(&[
                    x as u8,
                    y as u8,
                    (x ^ y) as u8,
                    255,
                ]);
            }
        }
        system
            .physical_memory_mut()
            .write(linear_source, &expected)
            .unwrap();
        execute_display_transfer(
            system.physical_memory_mut(),
            DisplayTransferRequest {
                input_address: linear_source,
                output_address: tiled_source,
                input_width: 64,
                input_height: 64,
                output_width: 64,
                output_height: 64,
                flags: crate::gpu::DISPLAY_TRANSFER_LINEAR_TO_TILED,
            },
        )
        .unwrap();

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_DISPLAY_TRANSFER;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&tiled_source.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&linear_output.to_le_bytes());
        let dimensions = (64u32 << 16) | 64;
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&dimensions.to_le_bytes());
        queue_bytes[entry + 16..entry + 20].copy_from_slice(&dimensions.to_le_bytes());

        let queue_address = shared_memory_base + GspGxCommandQueue::client_queue_offset(0).unwrap();
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .write_memory_bytes(queue_address, &queue_bytes)
            .unwrap();
        let report = system
            .process_gsp_gx_command_queue(shared_memory_base, 0)
            .unwrap();

        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.display_transfers_completed, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.halted);
        assert!(report.error.is_none());

        // The selected GSP framebuffers are physically resolved and applied
        // to the two LCD PDCs before the completion interrupt is delivered.
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Top, PDC_FRAMEBUFFER_A_SECOND)
                .unwrap(),
            top_left_framebuffer
        );
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Top, PDC_FRAMEBUFFER_B_SECOND)
                .unwrap(),
            top_right_framebuffer
        );
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Top, PDC_FRAMEBUFFER_STRIDE)
                .unwrap(),
            (TOP_SCREEN_WIDTH * 4) as u32
        );
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Top, PDC_FRAMEBUFFER_SELECT)
                .unwrap(),
            1
        );
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Bottom, PDC_FRAMEBUFFER_A_FIRST)
                .unwrap(),
            bottom_framebuffer
        );
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Bottom, PDC_FRAMEBUFFER_SELECT)
                .unwrap(),
            0
        );
        let mut top_header = [0u8; 4];
        let mut bottom_header = [0u8; 4];
        {
            let session = system.ncsd_session.as_mut().unwrap();
            session
                .read_memory_bytes(
                    shared_memory_base + GSP_TOP_FRAMEBUFFER_INFO_OFFSET,
                    &mut top_header,
                )
                .unwrap();
            session
                .read_memory_bytes(
                    shared_memory_base + GSP_BOTTOM_FRAMEBUFFER_INFO_OFFSET,
                    &mut bottom_header,
                )
                .unwrap();
        }
        assert_eq!(top_header[1] & GSP_FRAMEBUFFER_INFO_NEW_DATA, 0);
        assert_eq!(bottom_header[1] & GSP_FRAMEBUFFER_INFO_NEW_DATA, 0);

        // With no new metadata, GSP toggles each panel's next-buffer selector.
        system
            .update_lcd_from_gsp_shared_memory(shared_memory_base, 0)
            .unwrap();
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Top, PDC_FRAMEBUFFER_SELECT)
                .unwrap(),
            0
        );
        assert_eq!(
            system
                .lcd
                .read_register(LcdPanel::Bottom, PDC_FRAMEBUFFER_SELECT)
                .unwrap(),
            1
        );

        // DisplayTransfer completion must enqueue PPF for the rights owner
        // and signal that client's registered process-local event.
        let interrupt_queue_address =
            shared_memory_base + GspInterruptRelayQueue::client_queue_offset(0).unwrap();
        let mut interrupt_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session
            .as_mut()
            .unwrap()
            .read_memory_bytes(interrupt_queue_address, &mut interrupt_bytes)
            .unwrap();
        let interrupt_queue = GspInterruptRelayQueue::parse(&interrupt_bytes).unwrap();
        assert_eq!(
            interrupt_queue.pending_interrupts().unwrap(),
            [GspInterruptId::Ppf]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .event_handle_is_signaled(event_handle),
            Some(true)
        );

        let mut actual = vec![0u8; expected.len()];
        system
            .physical_memory()
            .read(linear_output, &mut actual)
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn gsp_gx_display_transfer_to_fcram_requires_cpu_invalidation() {
        use crate::gpu::{DISPLAY_TRANSFER_LINEAR_TO_TILED, execute_display_transfer};

        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        const WIDTH: u16 = 64;
        const HEIGHT: u16 = 64;
        const BUFFER_SIZE: usize = WIDTH as usize * HEIGHT as usize * 4;
        let (output_virtual, output_physical) =
            map_test_linear_fcram(&mut system, BUFFER_SIZE as u32);

        // Prime a clean ARM11 cache copy of the destination before the
        // DisplayTransfer engine writes the shared physical backing.
        let zeros = vec![0u8; BUFFER_SIZE];
        system
            .physical_memory_mut()
            .write(output_physical, &zeros)
            .unwrap();
        {
            let session = system.ncsd_session_mut().unwrap();
            let mut cached = vec![0u8; BUFFER_SIZE];
            session
                .read_memory_bytes(output_virtual, &mut cached)
                .unwrap();
            assert_eq!(cached, zeros);
        }

        let mut expected = vec![0u8; BUFFER_SIZE];
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let offset = (y * WIDTH as usize + x) * 4;
                expected[offset..offset + 4].copy_from_slice(&[
                    x as u8,
                    y as u8,
                    (x ^ y) as u8,
                    255,
                ]);
            }
        }

        let linear_source = VRAM_BASE + 0x20_000;
        let tiled_source = VRAM_BASE + 0x40_000;
        system
            .physical_memory_mut()
            .write(linear_source, &expected)
            .unwrap();
        execute_display_transfer(
            system.physical_memory_mut(),
            DisplayTransferRequest {
                input_address: linear_source,
                output_address: tiled_source,
                input_width: WIDTH,
                input_height: HEIGHT,
                output_width: WIDTH,
                output_height: HEIGHT,
                flags: DISPLAY_TRANSFER_LINEAR_TO_TILED,
            },
        )
        .unwrap();

        let mut queue_bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_DISPLAY_TRANSFER;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&tiled_source.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&output_virtual.to_le_bytes());
        let dimensions = (u32::from(HEIGHT) << 16) | u32::from(WIDTH);
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&dimensions.to_le_bytes());
        queue_bytes[entry + 16..entry + 20].copy_from_slice(&dimensions.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.display_transfers_completed, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.error.is_none());

        // The transfer engine writes the shared backing, but it does not
        // snoop or silently evict the CPU's clean cache copy.
        let mut physical = vec![0u8; BUFFER_SIZE];
        system
            .physical_memory()
            .read(output_physical, &mut physical)
            .unwrap();
        assert_eq!(physical, expected);

        let session = system.ncsd_session_mut().unwrap();
        let mut stale = vec![0u8; BUFFER_SIZE];
        session
            .read_memory_bytes(output_virtual, &mut stale)
            .unwrap();
        assert_eq!(stale, zeros);

        assert!(
            session
                .invalidate_process_data_cache(output_virtual, BUFFER_SIZE as u32)
                .unwrap()
                > 0
        );
        let mut visible = vec![0u8; BUFFER_SIZE];
        session
            .read_memory_bytes(output_virtual, &mut visible)
            .unwrap();
        assert_eq!(visible, expected);
    }

    #[test]
    fn gsp_gx_display_transfer_is_consumed_without_rendering_rights() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_DISPLAY_TRANSFER;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&(VRAM_BASE + 0x20_000).to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&(VRAM_BASE + 0x40_000).to_le_bytes());
        let dimensions = (64u32 << 16) | 64;
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&dimensions.to_le_bytes());
        queue_bytes[entry + 16..entry + 20].copy_from_slice(&dimensions.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, false);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.display_transfers_completed, 0);
        assert!(report.error.is_none());
    }

    #[test]
    fn gsp_gx_process_command_list_flushes_cache_before_pica_submission() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (command_list_address, command_list_physical_base) =
            map_test_linear_fcram(&mut system, 0x1000);
        let baseline_cache_lines = system
            .ncsd_session()
            .unwrap()
            .address_space()
            .data_cache_line_count();
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_u32(command_list_address, 0xDEAD_BEEF)
            .unwrap();
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_u32(command_list_address + 4, 0x000F_0100)
            .unwrap();
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines + 1
        );

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[0] = 0;
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = crate::gpu::GSP_GX_COMMAND_PROCESS_COMMAND_LIST;
        queue_bytes[entry + 2] = 1; // halt after this entry
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&command_list_address.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&8u32.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&1u32.to_le_bytes());
        queue_bytes[entry + 28..entry + 32].copy_from_slice(&1u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);

        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.command_lists_submitted, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.cache_flush_requested);
        assert!(report.gas_blend_update_requested);
        assert!(report.halted);
        assert!(report.error.is_none());
        assert_eq!(system.gpu().register(0x100), Some(0xDEAD_BEEF));
        assert_eq!(&queue.encode_header()[0..4], &[1, 0, 1, 1]);
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines
        );

        // The command-list flush invalidated the CPU line after writing it
        // back. A later device-side update must be visible on the next load.
        system
            .physical_memory_mut()
            .write(command_list_physical_base, &0xCAFE_BABEu32.to_le_bytes())
            .unwrap();
        assert_eq!(
            system
                .ncsd_session_mut()
                .unwrap()
                .read_memory_u32(command_list_address)
                .unwrap(),
            0xCAFE_BABE
        );
    }

    /// Guest-mapped buffers for a one-triangle draw through the GSP queue.
    struct RenderFixture {
        command_list_address: u32,
        vertex_address: u32,
        vertex_physical: u32,
        target_address: u32,
        target_physical: u32,
    }

    const RENDER_TARGET_REGISTERS: (u32, u32) = (2, (7 << 12) | 8);

    /// Loads a process, maps linear FCRAM buffers and programs the GPU for a
    /// triangle covering the lower-left half of an 8x8 RGBA8 target.
    fn render_fixture(system: &mut System) -> RenderFixture {
        use crate::gpu::{
            ClipDepthRange, DepthFunction, GeometryPipeline, Matrix4, PicaRenderSettings,
            REG_ATTRIBBUFFER0_OFFSET, REG_ATTRIBBUFFERS_FORMAT_HIGH, REG_ATTRIBBUFFERS_FORMAT_LOW,
            REG_COLORBUFFER_FORMAT, REG_COLORBUFFER_LOC, REG_FRAMEBUFFER_BLOCK32,
            REG_FRAMEBUFFER_DIM, REG_NUMVERTICES, REG_SH_OUTMAP_O0,
            REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, REG_VSH_ATTRIBUTES_PERMUTATION_LOW,
            REG_VSH_CODETRANSFER_DATA0, REG_VSH_CODETRANSFER_INDEX, REG_VSH_ENTRYPOINT,
            REG_VSH_INPUTBUFFER_CONFIG, REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX, Viewport,
        };

        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        let (command_list_address, _) = map_test_linear_fcram(system, 0x1000);
        let (vertex_address, vertex_physical) = map_test_linear_fcram(system, 0x1000);
        let (target_address, target_physical) = map_test_linear_fcram(system, 0x1000);

        let gpu = system.gpu_mut();
        for (register, value) in [
            (REG_ATTRIBBUFFERS_LOC, vertex_physical >> 3),
            (REG_ATTRIBBUFFERS_FORMAT_LOW, 0x1),
            (REG_ATTRIBBUFFERS_FORMAT_HIGH, 0),
            (REG_ATTRIBBUFFER0_OFFSET, 0),
            (REG_ATTRIBBUFFER0_OFFSET + 1, 0),
            (REG_ATTRIBBUFFER0_OFFSET + 2, (2 << 16) | (2 << 28)),
            (REG_VSH_INPUTBUFFER_CONFIG, 0),
            (REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 0),
            (REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0),
            (REG_VSH_OPDESCS_INDEX, 0),
            (
                REG_VSH_OPDESCS_DATA0,
                0b1111 | (0x1B << 5) | (0x1B << 14) | (0x1B << 23),
            ),
            (REG_VSH_CODETRANSFER_INDEX, 0),
            (REG_VSH_CODETRANSFER_DATA0, 0x13 << 26),
            (REG_VSH_CODETRANSFER_DATA0, 0x22 << 26),
            (REG_VSH_ENTRYPOINT, 0),
            (REG_NUMVERTICES, 3),
            (REG_COLORBUFFER_LOC, target_physical >> 3),
            (REG_COLORBUFFER_FORMAT, RENDER_TARGET_REGISTERS.0),
            (REG_FRAMEBUFFER_BLOCK32, 0),
            (REG_FRAMEBUFFER_DIM, RENDER_TARGET_REGISTERS.1),
        ] {
            gpu.write_register(register, value).unwrap();
        }
        for index in 0..7u16 {
            gpu.write_register(REG_SH_OUTMAP_O0 + index, 0x1F1F_1F1F)
                .unwrap();
        }
        gpu.write_register(REG_SH_OUTMAP_O0, 0x0302_0100).unwrap();

        system.set_pica_rendering(Some(PicaRenderSettings {
            pipeline: GeometryPipeline::new(
                Matrix4::from_column_major([
                    0.25, 0.0, 0.0, 0.0, //
                    0.0, 0.25, 0.0, 0.0, //
                    0.0, 0.0, 1.0, 0.0, //
                    -1.0, -1.0, 0.0, 1.0,
                ])
                .unwrap(),
                Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
                ClipDepthRange::NegativeOneToOne,
            ),
            default_color: [200, 100, 50, 255],
            depth_function: DepthFunction::Less,
        }));
        RenderFixture {
            command_list_address,
            vertex_address,
            vertex_physical,
            target_address,
            target_physical,
        }
    }

    /// Writes one DRAWARRAYS trigger from the CPU and runs it through the GX
    /// queue with a command-list flush. Returns the queue report.
    fn run_draw_through_gsp(
        system: &mut System,
        fixture: &RenderFixture,
    ) -> GspGxCommandQueueReport {
        let list = fixture.command_list_address;
        let session = system.ncsd_session_mut().unwrap();
        session.write_memory_u32(list, 1).unwrap();
        session.write_memory_u32(list + 4, 0x000F_022E).unwrap();
        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = crate::gpu::GSP_GX_COMMAND_PROCESS_COMMAND_LIST;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&list.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&8u32.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&1u32.to_le_bytes());
        queue_bytes[entry + 28..entry + 32].copy_from_slice(&1u32.to_le_bytes());
        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true)
    }

    fn rendered_pixel(system: &System, target_physical: u32, x: usize, y: usize) -> [u8; 4] {
        let config = crate::gpu::PicaRenderTargetConfig::from_registers(
            target_physical >> 3,
            RENDER_TARGET_REGISTERS.0,
            0,
            RENDER_TARGET_REGISTERS.1,
        )
        .unwrap();
        let pixels = config.read_rgba8(system.physical_memory()).unwrap();
        let offset = (y * 8 + x) * 4;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    }

    #[test]
    fn gsp_render_transfer_and_lcd_scanout_form_one_complete_frame() {
        use crate::gpu::{
            ClipDepthRange, DepthFunction, GSP_GX_COMMAND_DISPLAY_TRANSFER,
            GSP_GX_COMMAND_ENTRY_SIZE, GSP_GX_COMMAND_PROCESS_COMMAND_LIST,
            GSP_GX_COMMAND_QUEUE_HEADER_SIZE, GSP_GX_COMMAND_QUEUE_SIZE, GeometryPipeline, Matrix4,
            PicaRenderSettings, REG_ATTRIBBUFFER0_OFFSET, REG_ATTRIBBUFFERS_FORMAT_HIGH,
            REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC, REG_COLORBUFFER_FORMAT,
            REG_COLORBUFFER_LOC, REG_FRAMEBUFFER_BLOCK32, REG_FRAMEBUFFER_DIM, REG_NUMVERTICES,
            REG_SH_OUTMAP_O0, REG_VSH_ATTRIBUTES_PERMUTATION_HIGH,
            REG_VSH_ATTRIBUTES_PERMUTATION_LOW, REG_VSH_CODETRANSFER_DATA0,
            REG_VSH_CODETRANSFER_INDEX, REG_VSH_ENTRYPOINT, REG_VSH_INPUTBUFFER_CONFIG,
            REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX, VRAM_BASE, Viewport,
        };
        use crate::video::{
            LcdPanel, PDC_CONTROL, PDC_FRAMEBUFFER_A_FIRST, PDC_FRAMEBUFFER_A_SECOND,
            PDC_FRAMEBUFFER_B_FIRST, PDC_FRAMEBUFFER_B_SECOND, PDC_FRAMEBUFFER_FORMAT,
            PDC_FRAMEBUFFER_STRIDE, TOP_SCREEN_WIDTH,
        };

        const WIDTH: usize = 400;
        const HEIGHT: usize = 240;
        const COMMAND_LIST: u32 = VRAM_BASE + 0x10_000;
        const VERTICES: u32 = VRAM_BASE + 0x20_000;
        const TILED_TARGET: u32 = VRAM_BASE + 0x40_000;
        const LINEAR_FRAMEBUFFER: u32 = VRAM_BASE + 0xA0_000;

        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let gpu = system.gpu_mut();
        for (register, value) in [
            (REG_ATTRIBBUFFERS_LOC, VERTICES >> 3),
            (REG_ATTRIBBUFFERS_FORMAT_LOW, 0x1),
            (REG_ATTRIBBUFFERS_FORMAT_HIGH, 0),
            (REG_ATTRIBBUFFER0_OFFSET, 0),
            (REG_ATTRIBBUFFER0_OFFSET + 1, 0),
            (REG_ATTRIBBUFFER0_OFFSET + 2, (2 << 16) | (2 << 28)),
            (REG_VSH_INPUTBUFFER_CONFIG, 0),
            (REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 0),
            (REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0),
            (REG_VSH_OPDESCS_INDEX, 0),
            (
                REG_VSH_OPDESCS_DATA0,
                0b1111 | (0x1B << 5) | (0x1B << 14) | (0x1B << 23),
            ),
            (REG_VSH_CODETRANSFER_INDEX, 0),
            (REG_VSH_CODETRANSFER_DATA0, 0x13 << 26),
            (REG_VSH_CODETRANSFER_DATA0, 0x22 << 26),
            (REG_VSH_ENTRYPOINT, 0),
            (REG_NUMVERTICES, 3),
            (REG_COLORBUFFER_LOC, TILED_TARGET >> 3),
            (REG_COLORBUFFER_FORMAT, 2),
            (REG_FRAMEBUFFER_BLOCK32, 0),
            (
                REG_FRAMEBUFFER_DIM,
                (((HEIGHT as u32) - 1) << 12) | WIDTH as u32,
            ),
        ] {
            gpu.write_register(register, value).unwrap();
        }
        for index in 0..7u16 {
            gpu.write_register(REG_SH_OUTMAP_O0 + index, 0x1F1F_1F1F)
                .unwrap();
        }
        gpu.write_register(REG_SH_OUTMAP_O0, 0x0302_0100).unwrap();

        system.set_pica_rendering(Some(PicaRenderSettings {
            pipeline: GeometryPipeline::new(
                Matrix4::from_column_major([
                    0.25, 0.0, 0.0, 0.0, //
                    0.0, 0.25, 0.0, 0.0, //
                    0.0, 0.0, 1.0, 0.0, //
                    -1.0, -1.0, 0.0, 1.0,
                ])
                .unwrap(),
                Viewport::new(0.0, 0.0, WIDTH as f32, HEIGHT as f32).unwrap(),
                ClipDepthRange::NegativeOneToOne,
            ),
            default_color: [200, 100, 50, 255],
            depth_function: DepthFunction::Less,
        }));

        system
            .physical_memory_mut()
            .write(VERTICES, &[0, 0, 8, 0, 0, 8])
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(COMMAND_LIST, 1)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(COMMAND_LIST + 4, 0x000F_022E)
            .unwrap();

        let mut queue_bytes = [0u8; GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 2;

        let command_entry = GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[command_entry] = GSP_GX_COMMAND_PROCESS_COMMAND_LIST;
        queue_bytes[command_entry + 4..command_entry + 8]
            .copy_from_slice(&COMMAND_LIST.to_le_bytes());
        queue_bytes[command_entry + 8..command_entry + 12].copy_from_slice(&8u32.to_le_bytes());

        let transfer_entry = command_entry + GSP_GX_COMMAND_ENTRY_SIZE;
        queue_bytes[transfer_entry] = GSP_GX_COMMAND_DISPLAY_TRANSFER;
        queue_bytes[transfer_entry + 4..transfer_entry + 8]
            .copy_from_slice(&TILED_TARGET.to_le_bytes());
        queue_bytes[transfer_entry + 8..transfer_entry + 12]
            .copy_from_slice(&LINEAR_FRAMEBUFFER.to_le_bytes());
        let dimensions = ((HEIGHT as u32) << 16) | WIDTH as u32;
        queue_bytes[transfer_entry + 12..transfer_entry + 16]
            .copy_from_slice(&dimensions.to_le_bytes());
        queue_bytes[transfer_entry + 16..transfer_entry + 20]
            .copy_from_slice(&dimensions.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.commands_consumed, 2);
        assert_eq!(report.command_lists_submitted, 1);
        assert_eq!(report.display_transfers_completed, 1);
        let render = system.last_pica_render_report().unwrap();
        assert_eq!(render.triangles_submitted, 1);
        assert!(render.pixels_written > 0);

        let lcd = system.lcd_controller_mut();
        for (register, value) in [
            (PDC_FRAMEBUFFER_A_FIRST, LINEAR_FRAMEBUFFER),
            (PDC_FRAMEBUFFER_A_SECOND, LINEAR_FRAMEBUFFER),
            (PDC_FRAMEBUFFER_B_FIRST, LINEAR_FRAMEBUFFER),
            (PDC_FRAMEBUFFER_B_SECOND, LINEAR_FRAMEBUFFER),
            (PDC_FRAMEBUFFER_FORMAT, 0),
            (PDC_FRAMEBUFFER_STRIDE, (TOP_SCREEN_WIDTH * 4) as u32),
            (PDC_CONTROL, 0x1_0001),
        ] {
            lcd.write_register(LcdPanel::Top, register, value).unwrap();
        }

        let frame = system.run_frame_with_budget(0).unwrap();
        assert_eq!(frame.lcd_screens_scanned_out, 2);
        let sample = (180 * WIDTH + 40) * 4;
        assert_eq!(
            &system.display().top_left().pixels()[sample..sample + 4],
            &[200, 100, 50, 255]
        );
        assert_eq!(
            &system.display().top_right().pixels()[sample..sample + 4],
            &[200, 100, 50, 255]
        );
    }

    #[test]
    fn gsp_process_command_list_renders_a_draw_into_the_tiled_target_when_enabled() {
        let mut system = System::new();
        let fixture = render_fixture(&mut system);
        system
            .physical_memory_mut()
            .write(fixture.vertex_physical, &[0, 0, 8, 0, 0, 8])
            .unwrap();

        let report = run_draw_through_gsp(&mut system, &fixture);

        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(report.command_lists_submitted, 1);
        let render = system.last_pica_render_report().unwrap();
        assert_eq!(render.triangles_submitted, 1);
        assert!(render.pixels_written >= 28);
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [200, 100, 50, 255]
        );
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 6, 1),
            [0; 4]
        );
    }

    #[test]
    fn consecutive_gsp_command_lists_share_the_pica_depth_buffer() {
        let mut system = System::new();
        let fixture = render_fixture(&mut system);
        system
            .physical_memory_mut()
            .write(fixture.vertex_physical, &[0, 0, 8, 0, 0, 8])
            .unwrap();

        let first_queue = run_draw_through_gsp(&mut system, &fixture);
        assert!(first_queue.error.is_none(), "{:?}", first_queue.error);
        let first_render = system.last_pica_render_report().unwrap();
        assert_eq!(first_render.triangles_submitted, 1);
        assert!(first_render.pixels_written > 0);

        // A second identical draw at the same depth must be rejected. This
        // proves System retained the render session between independent GSP
        // ProcessCommandList submissions rather than resetting depth to 1.0.
        let second_queue = run_draw_through_gsp(&mut system, &fixture);
        assert!(second_queue.error.is_none(), "{:?}", second_queue.error);
        let second_render = system.last_pica_render_report().unwrap();
        assert_eq!(second_render.triangles_submitted, 1);
        assert_eq!(second_render.pixels_written, 0);
        assert!(second_render.depth_rejected_pixels > 0);
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [200, 100, 50, 255]
        );
    }

    /// Reads every word of the 256-byte tiled target through the CPU alias.
    fn cpu_target_words(system: &mut System, fixture: &RenderFixture) -> Vec<u32> {
        let session = system.ncsd_session_mut().unwrap();
        (0..64)
            .map(|word| {
                session
                    .read_memory_u32(fixture.target_address + word * 4)
                    .unwrap()
            })
            .collect()
    }

    fn physical_target_words(system: &System, fixture: &RenderFixture) -> Vec<u32> {
        (0..64)
            .map(|word| {
                system
                    .physical_memory()
                    .read_u32(fixture.target_physical + word * 4)
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn gpu_render_into_fcram_is_not_snooped_by_a_clean_cpu_line() {
        let mut system = System::new();
        let fixture = render_fixture(&mut system);
        system
            .physical_memory_mut()
            .write(fixture.vertex_physical, &[0, 0, 8, 0, 0, 8])
            .unwrap();
        // Prime clean CPU lines over the whole (zeroed) target.
        assert!(
            cpu_target_words(&mut system, &fixture)
                .iter()
                .all(|w| *w == 0)
        );

        let report = run_draw_through_gsp(&mut system, &fixture);
        assert!(report.error.is_none(), "{:?}", report.error);

        let physical = physical_target_words(&system, &fixture);
        assert!(
            physical.iter().any(|word| *word != 0),
            "the draw must have written pixels"
        );
        // The CPU keeps seeing its stale lines until it invalidates them.
        assert!(
            cpu_target_words(&mut system, &fixture)
                .iter()
                .all(|w| *w == 0)
        );
        system
            .ncsd_session_mut()
            .unwrap()
            .invalidate_process_data_cache(fixture.target_address, 256)
            .unwrap();
        assert_eq!(cpu_target_words(&mut system, &fixture), physical);
    }

    #[test]
    fn a_dirty_cpu_line_flushed_after_the_draw_overwrites_the_gpu_pixels() {
        let mut system = System::new();
        let fixture = render_fixture(&mut system);
        system
            .physical_memory_mut()
            .write(fixture.vertex_physical, &[0, 0, 8, 0, 0, 8])
            .unwrap();
        // The CPU clears the target; the zeros stay dirty in the D-cache.
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_bytes(fixture.target_address, &[0u8; 256])
            .unwrap();

        let report = run_draw_through_gsp(&mut system, &fixture);
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [200, 100, 50, 255]
        );

        // Writing the stale dirty lines back destroys the GPU output...
        system
            .ncsd_session_mut()
            .unwrap()
            .store_process_data_cache(fixture.target_address, 256)
            .unwrap();
        system
            .ncsd_session_mut()
            .unwrap()
            .drain_write_buffer()
            .unwrap();
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [0; 4]
        );
    }

    #[test]
    fn invalidating_a_dirty_cpu_line_before_the_draw_preserves_the_gpu_pixels() {
        let mut system = System::new();
        let fixture = render_fixture(&mut system);
        system
            .physical_memory_mut()
            .write(fixture.vertex_physical, &[0, 0, 8, 0, 0, 8])
            .unwrap();
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_bytes(fixture.target_address, &[0u8; 256])
            .unwrap();
        // ...while discarding them first keeps it.
        system
            .ncsd_session_mut()
            .unwrap()
            .invalidate_process_data_cache(fixture.target_address, 256)
            .unwrap();

        let report = run_draw_through_gsp(&mut system, &fixture);
        assert!(report.error.is_none(), "{:?}", report.error);
        system
            .ncsd_session_mut()
            .unwrap()
            .store_process_data_cache(fixture.target_address, 256)
            .unwrap();
        system
            .ncsd_session_mut()
            .unwrap()
            .drain_write_buffer()
            .unwrap();
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [200, 100, 50, 255]
        );
    }

    #[test]
    fn unflushed_cpu_vertex_data_is_not_seen_by_the_gpu_but_flushed_data_is() {
        let mut system = System::new();
        let fixture = render_fixture(&mut system);
        // The CPU writes the vertices; they sit dirty in the D-cache.
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_bytes(fixture.vertex_address, &[0, 0, 8, 0, 0, 8])
            .unwrap();
        assert_eq!(
            system
                .physical_memory()
                .read_u32(fixture.vertex_physical)
                .unwrap(),
            0
        );

        // Without a flush the GPU reads zeros: a degenerate triangle.
        let report = run_draw_through_gsp(&mut system, &fixture);
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [0; 4]
        );

        // After an explicit flush the same draw sees the CPU's data.
        system
            .ncsd_session_mut()
            .unwrap()
            .flush_process_data_cache(fixture.vertex_address, 8)
            .unwrap();
        system
            .ncsd_session_mut()
            .unwrap()
            .drain_write_buffer()
            .unwrap();
        let report = run_draw_through_gsp(&mut system, &fixture);
        assert!(report.error.is_none(), "{:?}", report.error);
        assert_eq!(
            rendered_pixel(&system, fixture.target_physical, 1, 6),
            [200, 100, 50, 255]
        );
    }

    #[test]
    fn command_lists_are_only_parsed_when_rendering_is_not_enabled() {
        let system = System::new();
        assert!(system.last_pica_render_report().is_none());
    }

    #[test]
    fn gsp_gx_queue_consumes_process_command_list_without_rendering_rights() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let command_list_address = VRAM_BASE + 0x1000;
        system
            .physical_memory_mut()
            .write_u32(command_list_address, 0xDEAD_BEEF)
            .unwrap();
        system
            .physical_memory_mut()
            .write_u32(command_list_address + 4, 0x000F_0100)
            .unwrap();

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = crate::gpu::GSP_GX_COMMAND_PROCESS_COMMAND_LIST;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&command_list_address.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&8u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, false);

        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.command_lists_submitted, 0);
        assert_eq!(report.pending_commands, 0);
        assert!(report.error.is_none());
        assert_eq!(system.gpu().register(0x100), Some(0));
        assert_eq!(&queue.encode_header()[0..2], &[1, 0]);
    }

    #[test]
    fn gsp_gx_queue_rejects_unmapped_process_command_list_address() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = crate::gpu::GSP_GX_COMMAND_PROCESS_COMMAND_LIST;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&0x1400_4000u32.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&8u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);

        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.command_lists_submitted, 0);
        assert_eq!(report.pending_commands, 0);
        assert_eq!(report.status, 0x81);
        assert_eq!(report.last_result, GSP_INVALID_ADDRESS_RESULT);
        assert!(matches!(
            report.error,
            Some(GspGxCommandQueueError::InvalidCommandListAddress { .. })
        ));
        assert_eq!(system.gpu().register(0x100), Some(0));
    }

    #[test]
    fn frame_processes_pica_command_buffer_channels_before_display_presentation() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let command_list = [0xDEAD_BEEF, 0x000F_0100, 0x1234_5678, 0x000F_0101];
        for (index, word) in command_list.iter().copied().enumerate() {
            system
                .physical_memory_mut()
                .write_u32(FCRAM_BASE + 0x2000 + (index as u32 * 4), word)
                .unwrap();
        }

        system
            .gpu_mut()
            .write_register(REG_COMMAND_BUFFER_SIZE_0, 2)
            .unwrap();
        system
            .gpu_mut()
            .write_register(REG_COMMAND_BUFFER_ADDRESS_0, (FCRAM_BASE + 0x2000) >> 3)
            .unwrap();
        system
            .gpu_mut()
            .write_register(REG_COMMAND_BUFFER_JUMP_0, 1)
            .unwrap();

        let report = system.run_frame_with_budget(0).unwrap();

        assert_eq!(report.gpu_command_lists_submitted, 1);
        assert_eq!(report.gsp_interrupt_deliveries, 0);
        assert_eq!(system.gpu().register(0x100), Some(0xDEAD_BEEF));
        assert_eq!(system.gpu().register(0x101), Some(0x1234_5678));
        assert_eq!(system.gpu().register(REG_COMMAND_BUFFER_JUMP_0), Some(0));
        assert_eq!(system.display().frame_number(), 1);
    }

    #[test]
    fn invalid_pica_command_buffer_stays_pending_for_diagnostics() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        system
            .gpu_mut()
            .write_register(REG_COMMAND_BUFFER_SIZE_0, 1)
            .unwrap();
        system
            .gpu_mut()
            .write_register(REG_COMMAND_BUFFER_ADDRESS_0, (FCRAM_BASE + 0x2000) >> 3)
            .unwrap();
        system
            .gpu_mut()
            .write_register(REG_COMMAND_BUFFER_JUMP_0, 1)
            .unwrap();

        assert!(system.run_frame_with_budget(0).is_err());
        assert_eq!(system.gpu().register(REG_COMMAND_BUFFER_JUMP_0), Some(1));
        assert_eq!(system.display().frame_number(), 0);
    }

    #[test]
    fn frame_scans_out_physical_top_stereo_and_bottom_framebuffers() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let top_left = FCRAM_BASE + 0x8000;
        let top_right = FCRAM_BASE + 0x9000;
        let bottom = FCRAM_BASE + 0xA000;
        system
            .physical_memory_mut()
            .write(top_left, &[0x44, 0x33, 0x22, 0x11])
            .unwrap();
        system
            .physical_memory_mut()
            .write(top_right, &[0x88, 0x77, 0x66, 0x55])
            .unwrap();
        system
            .physical_memory_mut()
            .write(bottom, &[0xCC, 0xBB, 0xAA, 0x99])
            .unwrap();

        for (panel, address_a, address_b, stride) in [
            (
                LcdPanel::Top,
                top_left,
                top_right,
                (TOP_SCREEN_WIDTH * 4) as u32,
            ),
            (
                LcdPanel::Bottom,
                bottom,
                0,
                (BOTTOM_SCREEN_WIDTH * 4) as u32,
            ),
        ] {
            for (offset, value) in [
                (PDC_FRAMEBUFFER_A_FIRST, address_a),
                (PDC_FRAMEBUFFER_A_SECOND, address_a),
                (PDC_FRAMEBUFFER_B_FIRST, address_b),
                (PDC_FRAMEBUFFER_B_SECOND, address_b),
                (PDC_FRAMEBUFFER_FORMAT, LcdPixelFormat::Rgba8 as u32),
                (PDC_FRAMEBUFFER_STRIDE, stride),
                (PDC_CONTROL, 0x0001_0001),
            ] {
                system
                    .lcd_controller_mut()
                    .write_register(panel, offset, value)
                    .unwrap();
            }
        }

        let report = system.run_frame_with_budget(0).unwrap();
        assert_eq!(report.lcd_screens_scanned_out, 3);
        assert_eq!(
            &system.display().top_left().pixels()[..4],
            &[0x11, 0x22, 0x33, 0x44]
        );
        assert_eq!(
            &system.display().top_right().pixels()[..4],
            &[0x55, 0x66, 0x77, 0x88]
        );
        assert_eq!(
            &system.display().bottom().pixels()[..4],
            &[0x99, 0xAA, 0xBB, 0xCC]
        );
        assert_eq!(
            system
                .lcd_controller()
                .read_register(LcdPanel::Top, PDC_FRAMEBUFFER_SELECT),
            Ok(0)
        );
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
        assert_eq!(report.lcd_screens_scanned_out, 0);
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

    fn clear_cia_archive_fixture() -> Vec<u8> {
        const CIA_HEADER_SIZE: usize = 0x2020;
        const TICKET_SIZE: usize = 0x350;
        const TMD_SIZE: usize = 0xB34;
        const CONTENT_SIZE: usize = 0x8000;
        const TITLE_ID: u64 = 0x0004_0000_0000_0001;

        let cartridge = clear_ncsd_cartridge_fixture();
        let mut content = cartridge[0x4000..0x4000 + CONTENT_SIZE].to_vec();
        content[0x118..0x120].copy_from_slice(&TITLE_ID.to_le_bytes());

        let ticket_offset = 0x2040usize;
        let tmd_offset = (ticket_offset + TICKET_SIZE + 0x3F) & !0x3F;
        let content_offset = (tmd_offset + TMD_SIZE + 0x3F) & !0x3F;
        let mut cia = vec![0u8; content_offset + content.len()];

        write_u32(&mut cia, 0x00, CIA_HEADER_SIZE as u32);
        cia[0x04..0x06].copy_from_slice(&0u16.to_le_bytes());
        cia[0x06..0x08].copy_from_slice(&0u16.to_le_bytes());
        write_u32(&mut cia, 0x08, 0);
        write_u32(&mut cia, 0x0C, TICKET_SIZE as u32);
        write_u32(&mut cia, 0x10, TMD_SIZE as u32);
        write_u32(&mut cia, 0x14, 0);
        cia[0x18..0x20].copy_from_slice(&(CONTENT_SIZE as u64).to_le_bytes());
        cia[0x20] = 0x80;

        cia[ticket_offset..ticket_offset + 4].copy_from_slice(&0x0001_0004u32.to_be_bytes());
        let ticket_body = ticket_offset + 0x140;
        cia[ticket_body + 0x9C..ticket_body + 0xA4].copy_from_slice(&TITLE_ID.to_be_bytes());

        cia[tmd_offset..tmd_offset + 4].copy_from_slice(&0x0001_0004u32.to_be_bytes());
        let tmd_body = tmd_offset + 0x140;
        cia[tmd_body + 0x4C..tmd_body + 0x54].copy_from_slice(&TITLE_ID.to_be_bytes());
        cia[tmd_body + 0x9E..tmd_body + 0xA0].copy_from_slice(&1u16.to_be_bytes());
        cia[tmd_body + 0xA0..tmd_body + 0xA2].copy_from_slice(&0u16.to_be_bytes());

        let info_table_start = tmd_body + 0xC4;
        let content_records_start = info_table_start + 64 * 0x24;
        let mut content_record = [0u8; 0x30];
        content_record[0x00..0x04].copy_from_slice(&1u32.to_be_bytes());
        content_record[0x04..0x06].copy_from_slice(&0u16.to_be_bytes());
        content_record[0x06..0x08].copy_from_slice(&0u16.to_be_bytes());
        content_record[0x08..0x10].copy_from_slice(&(CONTENT_SIZE as u64).to_be_bytes());
        content_record[0x10..0x30].copy_from_slice(&sha256_digest(&content));
        cia[content_records_start..content_records_start + content_record.len()]
            .copy_from_slice(&content_record);

        let mut info_record = [0u8; 0x24];
        info_record[0x00..0x02].copy_from_slice(&0u16.to_be_bytes());
        info_record[0x02..0x04].copy_from_slice(&1u16.to_be_bytes());
        info_record[0x04..0x24].copy_from_slice(&sha256_digest(&content_record));
        cia[info_table_start..info_table_start + info_record.len()].copy_from_slice(&info_record);
        let info_table = &cia[info_table_start..content_records_start];
        let info_table_hash = sha256_digest(info_table);
        cia[tmd_body + 0xA4..tmd_body + 0xC4].copy_from_slice(&info_table_hash);

        cia[content_offset..content_offset + content.len()].copy_from_slice(&content);
        cia
    }

    #[test]
    fn loads_clear_no_crypto_cia_boot_content_into_arm11_session() {
        let path =
            std::env::temp_dir().join(format!("aceris-clear-cia-{}.cia", std::process::id()));
        std::fs::write(&path, clear_cia_archive_fixture()).unwrap();

        let mut system = System::new();
        let result = system.load_executable_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.unwrap(), ImageFormat::Cia);
        assert_eq!(system.image(), Some(ImageFormat::Cia));
        assert_eq!(system.cpu().pc(), 0x0010_0000);

        let report = system.run_instructions(2).unwrap();
        assert_eq!(report.instructions_executed, 2);
        assert_eq!(system.cpu().regs[0], 7);
        assert_eq!(system.cpu().regs[1], 5);
    }

    fn encrypted_cia_archive_fixture(
        encrypt_ncch_regions: bool,
    ) -> (Vec<u8>, [u8; 16], [u8; 16], [u8; 16]) {
        const TICKET_OFFSET: usize = 0x2040;
        const TICKET_SIZE: usize = 0x350;
        const TMD_SIZE: usize = 0xB34;
        const CONTENT_SIZE: usize = 0x8000;
        const TITLE_ID: u64 = 0x0004_0000_0000_0001;
        const TITLE_KEY: [u8; 16] = [0x82; 16];
        const NORMAL_KEY: [u8; 16] = [0x71; 16];
        const MENU_INFO_KEY: [u8; 16] = [0x63; 16];
        const CONTENT_KEY: [u8; 16] = [0x64; 16];

        let mut cia = clear_cia_archive_fixture();
        let tmd_offset = (TICKET_OFFSET + TICKET_SIZE + 0x3F) & !0x3F;
        let content_offset = (tmd_offset + TMD_SIZE + 0x3F) & !0x3F;
        let tmd_body = tmd_offset + 0x140;
        let info_table_start = tmd_body + 0xC4;
        let content_records_start = info_table_start + 64 * 0x24;
        let mut clear_content = cia[content_offset..content_offset + CONTENT_SIZE].to_vec();

        if encrypt_ncch_regions {
            // Remove NoCrypto and encrypt the extended header and .code with
            // their distinct NCCH AES-CTR key groups before wrapping the CIA.
            clear_content[0x18F] &= !0x04;
            let ncch_header = ncch::NcchHeader::parse(
                &clear_content[..ncch::HEADER_SIZE],
                clear_content.len() as u64,
            )
            .unwrap();

            let extended_header_counter = ncch_header
                .initial_ctr_for_region(ncch::crypto::NcchCtrRegion::ExtendedHeader)
                .unwrap();
            let mut extended_header = clear_content[0x200..0x600].to_vec();
            ncch::crypto::Aes128Ctr::new(MENU_INFO_KEY, extended_header_counter)
                .apply_keystream(&mut extended_header)
                .unwrap();
            clear_content[0x200..0x600].copy_from_slice(&extended_header);

            let exefs_region = ncch_header.exefs.unwrap();
            let exefs_start = usize::try_from(exefs_region.offset_bytes).unwrap();
            let exefs_size = usize::try_from(exefs_region.size_bytes).unwrap();
            let mut exefs = clear_content[exefs_start..exefs_start + exefs_size].to_vec();
            let code_size =
                usize::try_from(u32::from_le_bytes(exefs[12..16].try_into().unwrap())).unwrap();
            assert!(code_size <= exefs.len() - 0x200);

            let exefs_counter = ncch_header
                .initial_ctr_for_region(ncch::crypto::NcchCtrRegion::ExeFs)
                .unwrap();
            ncch::crypto::Aes128Ctr::new(MENU_INFO_KEY, exefs_counter)
                .apply_keystream(&mut exefs[..0x200])
                .unwrap();
            ncch::crypto::Aes128Ctr::apply_keystream_at_offset(
                CONTENT_KEY,
                exefs_counter,
                0x200,
                &mut exefs[0x200..0x200 + code_size],
            )
            .unwrap();
            clear_content[exefs_start..exefs_start + exefs_size].copy_from_slice(&exefs);
        }

        // Mark the boot content as outer-CIA encrypted and hash the exact
        // NCCH bytes stored after optional inner AES-CTR encryption.
        let mut content_record = [0u8; 0x30];
        content_record.copy_from_slice(&cia[content_records_start..content_records_start + 0x30]);
        content_record[0x06..0x08].copy_from_slice(&1u16.to_be_bytes());
        content_record[0x10..0x30].copy_from_slice(&sha256_digest(&clear_content));
        cia[content_records_start..content_records_start + content_record.len()]
            .copy_from_slice(&content_record);

        let mut info_table = cia[info_table_start..content_records_start].to_vec();
        info_table[0x04..0x24].copy_from_slice(&sha256_digest(&content_record));
        cia[info_table_start..content_records_start].copy_from_slice(&info_table);
        cia[tmd_body + 0xA4..tmd_body + 0xC4].copy_from_slice(&sha256_digest(&info_table));

        // Encrypt the ticket title key with the explicitly configured normal key.
        let mut title_key_iv = [0u8; 16];
        title_key_iv[..8].copy_from_slice(&TITLE_ID.to_be_bytes());
        let mut title_key_block = TITLE_KEY;
        for (byte, previous) in title_key_block.iter_mut().zip(title_key_iv) {
            *byte ^= previous;
        }
        let encrypted_title_key = crate::loader::ncsd::ncch::crypto::Aes128::new(NORMAL_KEY)
            .encrypt_block(title_key_block);
        let ticket_body = TICKET_OFFSET + 0x140;
        cia[ticket_body + 0x7F..ticket_body + 0x8F].copy_from_slice(&encrypted_title_key);

        // Encrypt the complete CIA content with the ticket title key.
        let cipher = crate::loader::ncsd::ncch::crypto::Aes128::new(TITLE_KEY);
        let mut previous = crate::loader::cia::cia_content_iv(0);
        let mut encrypted_content = Vec::with_capacity(CONTENT_SIZE);
        for chunk in clear_content.chunks_exact(16) {
            let mut block: [u8; 16] = chunk.try_into().unwrap();
            for (byte, prior) in block.iter_mut().zip(previous) {
                *byte ^= prior;
            }
            let ciphertext = cipher.encrypt_block(block);
            encrypted_content.extend_from_slice(&ciphertext);
            previous = ciphertext;
        }
        cia[content_offset..content_offset + CONTENT_SIZE].copy_from_slice(&encrypted_content);

        (cia, NORMAL_KEY, MENU_INFO_KEY, CONTENT_KEY)
    }

    fn clear_outer_encrypted_ncch_cia_fixture() -> (Vec<u8>, [u8; 16], [u8; 16]) {
        const TICKET_SIZE: usize = 0x350;
        const TMD_SIZE: usize = 0xB34;
        const CONTENT_SIZE: usize = 0x8000;

        // Reuse the fixture's internally encrypted NCCH, then remove only its
        // outer CIA AES-CBC layer to model a clear CIA containing encrypted NCCH.
        let (mut cia, _, menu_info_key, content_key) = encrypted_cia_archive_fixture(true);
        let tmd_offset = (0x2040 + TICKET_SIZE + 0x3F) & !0x3F;
        let content_offset = (tmd_offset + TMD_SIZE + 0x3F) & !0x3F;
        let tmd_body = tmd_offset + 0x140;
        let info_table_start = tmd_body + 0xC4;
        let content_records_start = info_table_start + 64 * 0x24;

        let encrypted_bytes = cia[content_offset..content_offset + CONTENT_SIZE].to_vec();
        let mut decrypted = crate::loader::cia::Aes128CbcReader::new_seekable(
            std::io::Cursor::new(encrypted_bytes),
            [0x82; AES_BLOCK_SIZE],
            crate::loader::cia::cia_content_iv(0),
            CONTENT_SIZE as u64,
        )
        .unwrap();
        let mut clear_content = Vec::new();
        decrypted.read_to_end(&mut clear_content).unwrap();
        assert_eq!(clear_content.len(), CONTENT_SIZE);
        cia[content_offset..content_offset + CONTENT_SIZE].copy_from_slice(&clear_content);

        // The content record's SHA-256 already describes this clear outer
        // content. Change its encryption flag and refresh the TMD table hashes.
        let mut content_record = [0u8; 0x30];
        content_record.copy_from_slice(&cia[content_records_start..content_records_start + 0x30]);
        content_record[0x06..0x08].copy_from_slice(&0u16.to_be_bytes());
        cia[content_records_start..content_records_start + content_record.len()]
            .copy_from_slice(&content_record);

        let mut info_table = cia[info_table_start..content_records_start].to_vec();
        info_table[0x04..0x24].copy_from_slice(&sha256_digest(&content_record));
        cia[info_table_start..content_records_start].copy_from_slice(&info_table);
        cia[tmd_body + 0xA4..tmd_body + 0xC4].copy_from_slice(&sha256_digest(&info_table));

        (cia, menu_info_key, content_key)
    }

    #[test]
    fn loads_clear_cia_with_encrypted_ncch_boot_content_using_ncch_keys() {
        let (cia, menu_info_key, content_key) = clear_outer_encrypted_ncch_cia_fixture();
        let mut system = System::new();

        let result =
            system.load_cia_with_ncch_keys(std::io::Cursor::new(cia), menu_info_key, content_key);

        assert_eq!(result.unwrap(), ImageFormat::Cia);
        assert_eq!(system.image(), Some(ImageFormat::Cia));
        assert_eq!(system.cpu().pc(), 0x0010_0000);

        let report = system.run_instructions(2).unwrap();
        assert_eq!(report.instructions_executed, 2);
        assert_eq!(system.cpu().regs[0], 7);
        assert_eq!(system.cpu().regs[1], 5);
    }

    #[test]
    fn clear_ncch_key_path_rejects_outer_encrypted_cia_without_replacing_process() {
        let (cia, _, _, _) = encrypted_cia_archive_fixture(false);
        let mut system = System::new();

        let error = system
            .load_cia_with_ncch_keys(
                std::io::Cursor::new(cia),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap_err();

        assert!(matches!(error, Error::Unimplemented(_)));
        assert_eq!(system.image(), None);
    }

    #[test]
    fn loads_encrypted_cia_boot_content_with_caller_supplied_ticket_keys() {
        let (cia, normal_key, _, _) = encrypted_cia_archive_fixture(false);
        let mut keys = crate::loader::cia::TicketCommonKeyStore::default();
        keys.set_normal_key(0, normal_key).unwrap();

        let mut system = System::new();
        let result = system.load_cia_with_keys(
            std::io::Cursor::new(cia),
            &keys,
            [0; AES_BLOCK_SIZE],
            [0; AES_BLOCK_SIZE],
        );

        assert_eq!(result.unwrap(), ImageFormat::Cia);
        assert_eq!(system.image(), Some(ImageFormat::Cia));
        assert_eq!(system.cpu().pc(), 0x0010_0000);

        let report = system.run_instructions(2).unwrap();
        assert_eq!(report.instructions_executed, 2);
        assert_eq!(system.cpu().regs[0], 7);
        assert_eq!(system.cpu().regs[1], 5);
    }

    #[test]
    fn loads_encrypted_cia_and_encrypted_ncch_into_arm11_session() {
        let (cia, normal_key, menu_info_key, content_key) = encrypted_cia_archive_fixture(true);
        let mut keys = crate::loader::cia::TicketCommonKeyStore::default();
        keys.set_normal_key(0, normal_key).unwrap();

        let mut system = System::new();
        let result =
            system.load_cia_with_keys(std::io::Cursor::new(cia), &keys, menu_info_key, content_key);

        assert_eq!(result.unwrap(), ImageFormat::Cia);
        assert_eq!(system.image(), Some(ImageFormat::Cia));
        assert_eq!(system.cpu().pc(), 0x0010_0000);

        let report = system.run_instructions(2).unwrap();
        assert_eq!(report.instructions_executed, 2);
        assert_eq!(system.cpu().regs[0], 7);
        assert_eq!(system.cpu().regs[1], 5);
    }

    #[test]
    #[ignore = "requires ACERIS_TEST_CIA to point to a local clear CIA file"]
    fn loads_external_cia_and_executes_one_arm11_instruction() {
        let path = std::path::PathBuf::from(
            std::env::var_os("ACERIS_TEST_CIA").expect("set ACERIS_TEST_CIA to the local CIA path"),
        );
        let mut system = System::new();

        assert_eq!(
            system.load_executable_file(&path).unwrap(),
            ImageFormat::Cia
        );
        assert_eq!(system.image(), Some(ImageFormat::Cia));
        assert_eq!(system.cpu().pc(), 0x0010_0000);

        let report = system.run_instructions(1).unwrap();
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(
            report.stop_reason,
            Arm11RunStopReason::InstructionBudgetExhausted
        );
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

    #[test]
    fn gsp_gx_texture_copy_copies_gapped_rows_and_signals_ppf() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let source_virtual: u32 = 0x1400_0200;
        let source_physical = crate::gpu::FCRAM_BASE + 0x200;
        let destination_physical = VRAM_BASE + 0x2000;
        let source_bytes: Vec<u8> = (0..272).map(|value| value as u8).collect();
        system
            .gpu_address_space_mut()
            .map_linear_region(0x1400_0000, 0x1000)
            .unwrap();
        system
            .physical_memory_mut()
            .write(source_physical, &source_bytes)
            .unwrap();
        system
            .physical_memory_mut()
            .write(destination_physical, &[0xCC; 192])
            .unwrap();

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session_mut().unwrap();
            let stack = session.address_space().layout().stack;
            assert_eq!(stack.address & 0xFFF, 0);
            assert!(stack.mapped_size_bytes as usize >= GSP_SHARED_MEMORY_SIZE);
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_TEXTURE_COPY;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&source_virtual.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&destination_physical.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&192u32.to_le_bytes());
        queue_bytes[entry + 16..entry + 20].copy_from_slice(&0x0001_0002u32.to_le_bytes());
        queue_bytes[entry + 20..entry + 24].copy_from_slice(&0x0000_0002u32.to_le_bytes());
        queue_bytes[entry + 24..entry + 28].copy_from_slice(&0x0Cu32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.texture_copies_completed, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.halted);
        assert!(report.error.is_none());

        let mut actual = [0u8; 192];
        system
            .physical_memory()
            .read(destination_physical, &mut actual)
            .unwrap();
        let mut expected = Vec::new();
        for row in 0..6 {
            let start = row * 48;
            expected.extend_from_slice(&source_bytes[start..start + 32]);
        }
        assert_eq!(actual.as_slice(), expected.as_slice());

        let interrupt_queue_address =
            shared_memory_base + GspInterruptRelayQueue::client_queue_offset(0).unwrap();
        let mut interrupt_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(interrupt_queue_address, &mut interrupt_bytes)
            .unwrap();
        let interrupt_queue = GspInterruptRelayQueue::parse(&interrupt_bytes).unwrap();
        assert_eq!(
            interrupt_queue.pending_interrupts().unwrap(),
            [GspInterruptId::Ppf]
        );
    }

    #[test]
    fn gsp_gx_texture_copy_to_fcram_requires_cpu_invalidation() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (old_virtual_base, physical_base) = map_test_linear_fcram(&mut system, 0x1000);
        let new_virtual_base = 0x3000_0000 + (physical_base - FCRAM_BASE);
        system
            .ncsd_session_mut()
            .unwrap()
            .map_linear_memory_region(new_virtual_base, physical_base, 0x1000)
            .unwrap();

        let source_address = VRAM_BASE + 0x3000;
        let destination_old_alias = old_virtual_base + 0x200;
        let destination_new_alias = new_virtual_base + 0x200;
        // The GPU DMA map resolves the registered old alias; the process
        // address space independently exposes both aliases to the CPU.
        let destination_address = destination_old_alias;
        let expected: [u8; 16] = [
            0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xA9, 0xBA, 0xCB, 0xDC, 0xED,
            0xFE, 0x0F,
        ];
        system
            .physical_memory_mut()
            .write(source_address, &expected)
            .unwrap();

        let baseline_cache_lines = system
            .ncsd_session()
            .unwrap()
            .address_space()
            .data_cache_line_count();
        let mut cached_destination = [0xFF; 16];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(destination_new_alias, &mut cached_destination)
            .unwrap();
        assert_eq!(cached_destination, [0; 16]);
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines + 1
        );

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session_mut().unwrap();
            let stack = session.address_space().layout().stack;
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_TEXTURE_COPY;
        queue_bytes[entry + 2] = 1;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&source_address.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&destination_address.to_le_bytes());
        queue_bytes[entry + 12..entry + 16].copy_from_slice(&(expected.len() as u32).to_le_bytes());
        queue_bytes[entry + 24..entry + 28].copy_from_slice(&0x08u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.texture_copies_completed, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.halted);
        assert!(report.error.is_none());

        let mut physical_destination = [0u8; 16];
        system
            .physical_memory()
            .read(physical_base + 0x200, &mut physical_destination)
            .unwrap();
        assert_eq!(physical_destination, expected);

        // TextureCopy writes the physical FCRAM backing directly. The clean
        // CPU line remains stale through both virtual aliases until invalidated.
        let mut stale_destination = [0xFF; 16];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(destination_old_alias, &mut stale_destination)
            .unwrap();
        assert_eq!(stale_destination, [0; 16]);
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines + 1
        );

        assert_eq!(
            system
                .ncsd_session_mut()
                .unwrap()
                .invalidate_process_data_cache(destination_new_alias, expected.len() as u32)
                .unwrap(),
            1
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines
        );
        let mut refreshed_destination = [0u8; 16];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(destination_old_alias, &mut refreshed_destination)
            .unwrap();
        assert_eq!(refreshed_destination, expected);

        let interrupt_queue_address =
            shared_memory_base + GspInterruptRelayQueue::client_queue_offset(0).unwrap();
        let mut interrupt_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(interrupt_queue_address, &mut interrupt_bytes)
            .unwrap();
        let interrupt_queue = GspInterruptRelayQueue::parse(&interrupt_bytes).unwrap();
        assert_eq!(
            interrupt_queue.pending_interrupts().unwrap(),
            [GspInterruptId::Ppf]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .event_handle_is_signaled(event_handle),
            Some(true)
        );
    }

    #[test]
    fn gsp_gx_flush_then_texture_copy_reads_dirty_cpu_fcram_data() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();

        let (linear_virtual_base, linear_physical_base) =
            map_test_linear_fcram(&mut system, 0x1000);
        let source_address = linear_virtual_base + 0x100;
        let destination_address = VRAM_BASE + 0x4000;
        let expected: [u8; 16] = [
            0x91, 0x82, 0x73, 0x64, 0x55, 0x46, 0x37, 0x28, 0x19, 0x0A, 0xFB, 0xEC, 0xDD, 0xCE,
            0xBF, 0xA0,
        ];
        system
            .physical_memory_mut()
            .write(linear_physical_base + 0x100, &[0; 16])
            .unwrap();

        let baseline_cache_lines = system
            .ncsd_session()
            .unwrap()
            .address_space()
            .data_cache_line_count();
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_bytes(source_address, &expected)
            .unwrap();
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines + 1
        );
        let mut backing_before_flush = [0xFF; 16];
        system
            .physical_memory()
            .read(linear_physical_base + 0x100, &mut backing_before_flush)
            .unwrap();
        assert_eq!(backing_before_flush, [0; 16]);

        let (shared_memory_base, event_handle) = {
            let session = system.ncsd_session_mut().unwrap();
            let stack = session.address_space().layout().stack;
            session
                .write_memory_bytes(stack.address, &vec![0u8; GSP_SHARED_MEMORY_SIZE])
                .unwrap();
            let event_handle = session
                .create_event_handle(crate::loader::ncsd::session::kernel::EventResetType::OneShot)
                .unwrap();
            (stack.address, event_handle)
        };
        system
            .register_gsp_client(0, shared_memory_base, event_handle)
            .unwrap();
        assert!(system.try_acquire_gsp_rendering_right(0).unwrap());

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 2;
        let flush_entry = GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[flush_entry] = GSP_GX_COMMAND_FLUSH_CACHE_REGIONS;
        for (index, value) in [source_address, expected.len() as u32, 0, 0, 0, 0, 0]
            .into_iter()
            .enumerate()
        {
            let offset = flush_entry + 4 + index * 4;
            queue_bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        let copy_entry = GSP_GX_COMMAND_QUEUE_HEADER_SIZE + crate::gpu::GSP_GX_COMMAND_ENTRY_SIZE;
        queue_bytes[copy_entry] = GSP_GX_COMMAND_TEXTURE_COPY;
        queue_bytes[copy_entry + 2] = 1;
        queue_bytes[copy_entry + 4..copy_entry + 8].copy_from_slice(&source_address.to_le_bytes());
        queue_bytes[copy_entry + 8..copy_entry + 12]
            .copy_from_slice(&destination_address.to_le_bytes());
        queue_bytes[copy_entry + 12..copy_entry + 16]
            .copy_from_slice(&(expected.len() as u32).to_le_bytes());
        queue_bytes[copy_entry + 24..copy_entry + 28].copy_from_slice(&0x08u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 2);
        assert!(report.cache_flush_requested);
        assert_eq!(report.cache_regions_validated, 1);
        assert_eq!(report.texture_copies_completed, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.halted);
        assert!(report.error.is_none());
        assert_eq!(report.last_result, 0);

        let mut backing_source = [0u8; 16];
        system
            .physical_memory()
            .read(linear_physical_base + 0x100, &mut backing_source)
            .unwrap();
        assert_eq!(backing_source, expected);
        let mut copied_destination = [0u8; 16];
        system
            .physical_memory()
            .read(destination_address, &mut copied_destination)
            .unwrap();
        assert_eq!(copied_destination, expected);
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines
        );

        let interrupt_queue_address =
            shared_memory_base + GspInterruptRelayQueue::client_queue_offset(0).unwrap();
        let mut interrupt_bytes = [0u8; GSP_INTERRUPT_QUEUE_SIZE];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(interrupt_queue_address, &mut interrupt_bytes)
            .unwrap();
        let interrupt_queue = GspInterruptRelayQueue::parse(&interrupt_bytes).unwrap();
        assert_eq!(
            interrupt_queue.pending_interrupts().unwrap(),
            [GspInterruptId::Ppf]
        );
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .event_handle_is_signaled(event_handle),
            Some(true)
        );
    }

    #[test]
    fn gsp_gx_flush_cache_regions_cleans_and_invalidates_fcram_ranges() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        let (linear_virtual_base, linear_physical_base) =
            map_test_linear_fcram(&mut system, 0x2000);
        let baseline_cache_lines = system
            .ncsd_session()
            .unwrap()
            .address_space()
            .data_cache_line_count();
        system
            .ncsd_session_mut()
            .unwrap()
            .write_memory_bytes(linear_virtual_base + 0x100, &[0xCA, 0xFE, 0xBA, 0xBE])
            .unwrap();
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines + 1
        );

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_FLUSH_CACHE_REGIONS;
        queue_bytes[entry + 2] = 1;
        let arguments = [
            linear_virtual_base + 0x100,
            0x20,
            linear_virtual_base + 0x800,
            0x10,
            linear_virtual_base + 0x1800,
            0x40,
            0,
        ];
        for (index, value) in arguments.into_iter().enumerate() {
            let offset = entry + 4 + index * 4;
            queue_bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.commands_consumed, 1);
        assert!(report.cache_flush_requested);
        assert_eq!(report.cache_regions_validated, 3);
        assert_eq!(report.pending_commands, 0);
        assert!(report.error.is_none());
        assert_eq!(report.last_result, 0);
        let mut flushed = [0u8; 4];
        system
            .physical_memory()
            .read(linear_physical_base + 0x100, &mut flushed)
            .unwrap();
        assert_eq!(flushed, [0xCA, 0xFE, 0xBA, 0xBE]);

        // FlushCacheRegions must remove the clean line as well as write back
        // its dirty bytes. A later device write must therefore be visible
        // to the next CPU load without a separate invalidate operation.
        assert_eq!(
            system
                .ncsd_session()
                .unwrap()
                .address_space()
                .data_cache_line_count(),
            baseline_cache_lines
        );
        system
            .physical_memory_mut()
            .write(linear_physical_base + 0x100, &[0x11, 0x22, 0x33, 0x44])
            .unwrap();
        let mut cpu_observed = [0u8; 4];
        system
            .ncsd_session_mut()
            .unwrap()
            .read_memory_bytes(linear_virtual_base + 0x100, &mut cpu_observed)
            .unwrap();
        assert_eq!(cpu_observed, [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn gsp_gx_flush_cache_regions_zero_size_stops_later_regions() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        let (linear_virtual_base, _linear_physical_base) =
            map_test_linear_fcram(&mut system, 0x2000);

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_FLUSH_CACHE_REGIONS;
        let arguments = [
            linear_virtual_base + 0x100,
            0x20,
            0xDEAD_BEEF,
            0,
            linear_virtual_base + 0x1800,
            0x40,
            0,
        ];
        for (index, value) in arguments.into_iter().enumerate() {
            let offset = entry + 4 + index * 4;
            queue_bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert!(report.cache_flush_requested);
        assert_eq!(report.cache_regions_validated, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.error.is_none());
    }

    #[test]
    fn gsp_gx_flush_cache_regions_stops_at_first_invalid_range() {
        let mut system = System::new();
        system
            .load_ncsd_with_keys(
                std::io::Cursor::new(clear_ncsd_cartridge_fixture()),
                [0; AES_BLOCK_SIZE],
                [0; AES_BLOCK_SIZE],
            )
            .unwrap();
        let (linear_virtual_base, _linear_physical_base) =
            map_test_linear_fcram(&mut system, 0x2000);

        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_FLUSH_CACHE_REGIONS;
        let arguments = [
            linear_virtual_base + 0x100,
            0x20,
            linear_virtual_base + 0x4000,
            0x20,
            linear_virtual_base + 0x1800,
            0x40,
            0,
        ];
        for (index, value) in arguments.into_iter().enumerate() {
            let offset = entry + 4 + index * 4;
            queue_bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, true);
        assert_eq!(report.cache_regions_validated, 1);
        assert_eq!(
            report.error,
            Some(GspGxCommandQueueError::InvalidCacheRegionRange {
                address: linear_virtual_base + 0x4000,
                size_bytes: 0x20,
            })
        );
        assert_eq!(report.last_result, GSP_INVALID_ADDRESS_RESULT);
        assert!(report.halted);
        assert!(report.status & 0x80 != 0);
    }

    #[test]
    fn gsp_gx_flush_cache_regions_requires_rendering_rights() {
        let mut system = System::new();
        let mut queue_bytes = [0u8; crate::gpu::GSP_GX_COMMAND_QUEUE_SIZE];
        queue_bytes[1] = 1;
        let entry = crate::gpu::GSP_GX_COMMAND_QUEUE_HEADER_SIZE;
        queue_bytes[entry] = GSP_GX_COMMAND_FLUSH_CACHE_REGIONS;
        queue_bytes[entry + 4..entry + 8].copy_from_slice(&0x1500_0000u32.to_le_bytes());
        queue_bytes[entry + 8..entry + 12].copy_from_slice(&0x20u32.to_le_bytes());

        let mut queue = GspGxCommandQueue::parse(&queue_bytes).unwrap();
        let report = system.dispatch_gsp_gx_command_queue_with_right(&mut queue, false);
        assert!(report.cache_flush_requested);
        assert_eq!(report.cache_regions_validated, 0);
        assert_eq!(
            report.error,
            Some(GspGxCommandQueueError::RenderingRightRequired)
        );
        assert_eq!(report.last_result, RESULT_GSP_NO_RIGHT);
        assert!(report.halted);
    }
}
