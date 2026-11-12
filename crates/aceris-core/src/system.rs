//! Top-level emulated system.

use crate::cpu::Arm11Core;
use crate::gpu::{
    GSP_GX_COMMAND_PROCESS_COMMAND_LIST, GSP_GX_COMMAND_QUEUE_SIZE, GSP_INVALID_ADDRESS_RESULT,
    GpuDmaAddressSpace, GpuPhysicalMemory, GspGxCommandQueue, GspGxCommandQueueError,
    GspProcessCommandList, Pica200, VRAM_BASE, VRAM_SIZE,
};
use crate::loader::ncsd::ncch::{self, crypto::AES_BLOCK_SIZE};
use crate::loader::ncsd::session::Arm11RunStopReason;
use crate::loader::ncsd::{Arm11ProcessSession, Arm11RunReport, NcsdHeader, NcsdImageReader};
use crate::loader::{self, ImageFormat};
use crate::video::{DualScreenDisplay, LcdController};
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

/// Result of processing one GSP GX shared-memory queue snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GspGxCommandQueueReport {
    /// Number of entries consumed from the ring, including a failing entry.
    pub commands_consumed: u8,
    /// Number of ProcessCommandList entries submitted to the PICA200 parser.
    pub command_lists_submitted: u8,
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

/// Result of one completed development frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemFrameReport {
    pub frame_number: u64,
    pub instructions_executed: u64,
    pub cpu_stop_reason: Arm11RunStopReason,
    pub gpu_command_lists_submitted: u64,
    pub lcd_screens_scanned_out: u8,
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
        self.reset_gpu_linear_memory();
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

        self.gpu.execute_command_list(&words).map_err(|error| {
            Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid PICA200 command list: {error}"),
            ))
        })
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
    /// The caller supplies the base virtual address returned by the future GSP
    /// shared-memory service and the client slot (0..4). The queue slot must
    /// already be mapped read/write in the process. Only ProcessCommandList is
    /// dispatched in this first implementation; the other GX command families,
    /// GSP IPC rights/busy arbitration and interrupt delivery remain pending.
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
        let report = self.dispatch_gsp_gx_command_queue(&mut queue);

        self.ncsd_session
            .as_mut()
            .ok_or(Error::Unimplemented(
                "ARM11 process session was removed during GSP queue dispatch",
            ))?
            .write_memory_bytes(queue_address, &queue.encode_header())?;

        Ok(report)
    }

    fn dispatch_gsp_gx_command_queue(
        &mut self,
        queue: &mut GspGxCommandQueue,
    ) -> GspGxCommandQueueReport {
        let mut report = GspGxCommandQueueReport {
            commands_consumed: 0,
            command_lists_submitted: 0,
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

                if command.command_id != GSP_GX_COMMAND_PROCESS_COMMAND_LIST {
                    let error = GspGxCommandQueueError::UnsupportedCommand {
                        command_id: command.command_id,
                    };
                    queue.mark_error(None);
                    queue.halt();
                    report.error = Some(error);
                    break;
                }

                let arguments = command
                    .process_command_list()
                    .expect("command ID was checked immediately above");
                report.cache_flush_requested |= arguments.flush_buffer;
                report.gas_blend_update_requested |= arguments.update_gas_blend;

                match self.dispatch_gsp_process_command_list(arguments) {
                    Ok(()) => {
                        report.command_lists_submitted =
                            report.command_lists_submitted.saturating_add(1);
                    }
                    Err(error) => {
                        let result = matches!(
                            &error,
                            GspGxCommandQueueError::InvalidCommandListAddress { .. }
                                | GspGxCommandQueueError::InvalidCommandListSize { .. }
                        )
                        .then_some(GSP_INVALID_ADDRESS_RESULT);
                        queue.mark_error(result);
                        queue.halt();
                        report.error = Some(error);
                        break;
                    }
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

        let cpu_report = {
            let session = self.ncsd_session.as_mut().ok_or(Error::Unimplemented(
                "a prepared ARM11 process session is required for frame execution",
            ))?;
            let report = session.run_instructions(instruction_budget)?;
            session.advance_time(NOMINAL_FRAME_DURATION_NS);
            self.cpu = session.core().clone();
            report
        };

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
        Ok(SystemFrameReport {
            frame_number,
            instructions_executed: cpu_report.instructions_executed,
            cpu_stop_reason: cpu_report.stop_reason,
            gpu_command_lists_submitted,
            lcd_screens_scanned_out: lcd_report.screens_updated,
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
    use crate::gpu::{
        FCRAM_BASE, REG_COMMAND_BUFFER_ADDRESS_0, REG_COMMAND_BUFFER_JUMP_0,
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
    fn gsp_gx_queue_dispatches_process_command_list_to_pica200() {
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
        let report = system.dispatch_gsp_gx_command_queue(&mut queue);

        assert_eq!(report.commands_consumed, 1);
        assert_eq!(report.command_lists_submitted, 1);
        assert_eq!(report.pending_commands, 0);
        assert!(report.cache_flush_requested);
        assert!(report.gas_blend_update_requested);
        assert!(report.halted);
        assert!(report.error.is_none());
        assert_eq!(system.gpu().register(0x100), Some(0xDEAD_BEEF));
        assert_eq!(&queue.encode_header()[0..4], &[1, 0, 1, 1]);
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
        let report = system.dispatch_gsp_gx_command_queue(&mut queue);

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
