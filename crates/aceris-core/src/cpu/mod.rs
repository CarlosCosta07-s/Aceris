//! ARM11 CPU core.
//!
//! Current scope: an interpreter for ARM (A32) and classic Thumb instruction
//! sets with banked ARMv6 exception-mode registers and per-mode SPSR storage.
//! CP15, VFPv2, and ARMv6 short-descriptor MMU state live in dedicated modules.

mod arm;
mod block_cache;
mod condition;
mod coprocessor;
mod cp15;
mod decode_cache;
mod faults;
mod ir;
mod mmu;
mod mpcore_gic;
mod mpcore_scu;
mod mpcore_timer;
mod psr;
mod shifter;
pub(crate) mod state;
mod thumb;
mod vfp;

pub use block_cache::BlockCacheStats;
pub use condition::Condition;
pub use cp15::Cp15;
pub use mpcore_gic::{
    MPCORE_CPU_INTERFACE_CPU0_OFFSET, MPCORE_CPU_INTERFACE_CURRENT_OFFSET,
    MPCORE_GIC_CPU_INTERFACE_SIZE, MPCORE_GIC_DISTRIBUTOR_OFFSET, MPCORE_GIC_DISTRIBUTOR_SIZE,
    MPCORE_GIC_INTERRUPT_COUNT, MPCORE_GIC_SPURIOUS_INTERRUPT_ID,
    MPCORE_PRIVATE_TIMER_INTERRUPT_ID, MpcoreInterruptController,
};
pub use mpcore_scu::{
    MPCORE_PRIVATE_MEMORY_BASE, MPCORE_PRIVATE_MEMORY_SIZE, MpcoreScu, SCU_REGISTER_WINDOW_SIZE,
};
pub use mpcore_timer::{
    MPCORE_PRIVATE_TIMER_CPU0_ALIAS_OFFSET, MPCORE_PRIVATE_TIMER_OFFSET,
    MPCORE_PRIVATE_TIMER_REGISTER_SIZE, MPCORE_TIMER_DEFAULT_CLOCK_HZ, MpcorePrivateTimer,
};
pub use psr::Psr;
pub use state::CpuStateError;
pub use vfp::Vfp;

use state::{StateReader, StateWriter};

use crate::memory::Bus;
use aceris_common::{Error, Result};

/// Register indices with a special role.
pub const SP: usize = 13;
pub const LR: usize = 14;
pub const PC: usize = 15;

const ARM_INSTRUCTION_SIZE: u32 = 4;
const THUMB_INSTRUCTION_SIZE: u32 = 2;

/// Why [`Arm11Core::run_fast`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FastRunExit {
    /// The instruction budget was used up.
    Budget,
    /// Interrupt or execution state changed; call `run_fast` again (it will
    /// decline if the next instruction needs the precise step).
    Yield,
    /// The next instruction must run through [`Arm11Core::step`].
    NeedsPreciseStep,
    /// The last executed instruction produced this event. It is not counted
    /// in the returned number of executed instructions.
    Event(StepEvent),
}

/// What happened when the CPU executed one instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepEvent {
    /// A CPU step completed, either by executing an instruction or accepting an interrupt.
    Executed,
    /// A supervisor-call instruction was executed; the payload is its encoded
    /// comment field. Hardware exception entry is optional and disabled by default
    /// so HLE service dispatch can intercept this event.
    Svc(u32),
    /// A32 WFI or CP15 WFI retired and put the core into the wait state.
    WaitForInterrupt,
    /// The core remains in the WFI state; no guest instruction was executed.
    WaitingForInterrupt,
    /// An A32 WFE instruction retired and suspended the core waiting for an event.
    WaitForEvent,
    /// The core remains suspended in WFE; no guest instruction was executed.
    WaitingForEvent,
    /// A core executed SEV; the system scheduler must signal the other CPU cores.
    SendEvent,
    /// An exception return completed; the system scheduler must signal other MP11 CPUs.
    ExceptionReturn,
    /// A BKPT instruction was encountered. The immediate is retained for a
    /// debugger or the process exception layer.
    Breakpoint(u16),
}

/// One outstanding exclusive-access reservation for this processor context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExclusiveReservation {
    address: u32,
    size: u8,
    version: u64,
}

/// Register banks shared by ARMv6 User/System and exception modes.
#[derive(Clone, Debug, Default)]
struct BankedRegisters {
    user_r8_r12: [u32; 5],
    user_sp_lr: [u32; 2],
    fiq_r8_r14: [u32; 7],
    irq_sp_lr: [u32; 2],
    svc_sp_lr: [u32; 2],
    abort_sp_lr: [u32; 2],
    undefined_sp_lr: [u32; 2],
}

/// Saved program status registers belonging to exception modes.
#[derive(Clone, Debug, Default)]
struct SavedStatusRegisters {
    fiq: Option<Psr>,
    irq: Option<Psr>,
    svc: Option<Psr>,
    abort: Option<Psr>,
    undefined: Option<Psr>,
}

impl SavedStatusRegisters {
    fn get(&self, mode: u32) -> Option<Psr> {
        match mode {
            Psr::FIQ_MODE => self.fiq,
            Psr::IRQ_MODE => self.irq,
            Psr::SUPERVISOR_MODE => self.svc,
            Psr::ABORT_MODE => self.abort,
            Psr::UNDEFINED_MODE => self.undefined,
            _ => None,
        }
    }

    fn set(&mut self, mode: u32, value: Psr) -> bool {
        let slot = match mode {
            Psr::FIQ_MODE => &mut self.fiq,
            Psr::IRQ_MODE => &mut self.irq,
            Psr::SUPERVISOR_MODE => &mut self.svc,
            Psr::ABORT_MODE => &mut self.abort,
            Psr::UNDEFINED_MODE => &mut self.undefined,
            _ => return false,
        };
        *slot = Some(value);
        true
    }
}

/// One ARM11 processor core with banked exception-mode registers.
#[derive(Clone, Debug, Default)]
pub struct Arm11Core {
    /// Decoded A32 forms for the fast dispatch loop (keyed by encoding).
    decode_cache: decode_cache::DecodeCache,
    /// Straight-line A32 blocks from bus-certified immutable executable text.
    block_cache: block_cache::BlockCache,
    /// `r0`-`r15`. While an instruction executes, `r15` holds the address of
    /// that instruction; operand reads of `pc` add the pipeline offset.
    pub regs: [u32; 16],
    pub cpsr: Psr,
    branch_taken: bool,
    exclusive_reservation: Option<ExclusiveReservation>,
    banked: BankedRegisters,
    saved_status: SavedStatusRegisters,
    high_vectors_enabled: bool,
    exception_big_endian: bool,
    svc_exception_entry_enabled: bool,
    irq_line_asserted: bool,
    fiq_line_asserted: bool,
    waiting_for_interrupt: bool,
    waiting_for_event: bool,
    event_registered: bool,
    /// Set by exception-return instructions until the host scheduler consumes it.
    exception_return_wakeup: bool,
    cp15: Cp15,
    tlb: mmu::TranslationLookasideBuffer,
    vfp: Vfp,
    fault_exception_entry_enabled: bool,
    /// Set while one step runs: the fault being reported came from the fetch.
    prefetch_fault: bool,
    /// Set while one step runs: the MMU already latched FSR/FAR for this fault.
    fault_latched: bool,
}

impl Arm11Core {
    pub fn new() -> Self {
        Self::default()
    }

    /// Version of the complete architectural ARM11 core state.
    pub const SAVE_STATE_VERSION: u16 = 1;

    /// Serializes architectural CPU state. Host decode/block caches are omitted
    /// and rebuilt lazily; architectural TLB and CP15 state are preserved.
    pub fn encode_save_state(&self) -> std::result::Result<Vec<u8>, CpuStateError> {
        let mut writer = StateWriter::with_capacity(4096)?;
        writer.write_bytes(b"ACARM11\0")?;
        writer.write_u16(Self::SAVE_STATE_VERSION)?;

        for register in self.regs {
            writer.write_u32(register)?;
        }
        writer.write_u32(self.cpsr.0)?;
        writer.write_bool(self.branch_taken)?;

        writer.write_bool(self.exclusive_reservation.is_some())?;
        if let Some(reservation) = self.exclusive_reservation {
            writer.write_u32(reservation.address)?;
            writer.write_u8(reservation.size)?;
            writer.write_u64(reservation.version)?;
        }

        for value in self.banked.user_r8_r12 {
            writer.write_u32(value)?;
        }
        for value in self.banked.user_sp_lr {
            writer.write_u32(value)?;
        }
        for value in self.banked.fiq_r8_r14 {
            writer.write_u32(value)?;
        }
        for value in self.banked.irq_sp_lr {
            writer.write_u32(value)?;
        }
        for value in self.banked.svc_sp_lr {
            writer.write_u32(value)?;
        }
        for value in self.banked.abort_sp_lr {
            writer.write_u32(value)?;
        }
        for value in self.banked.undefined_sp_lr {
            writer.write_u32(value)?;
        }

        for saved in [
            self.saved_status.fiq,
            self.saved_status.irq,
            self.saved_status.svc,
            self.saved_status.abort,
            self.saved_status.undefined,
        ] {
            writer.write_bool(saved.is_some())?;
            if let Some(value) = saved {
                writer.write_u32(value.0)?;
            }
        }

        for value in [
            self.high_vectors_enabled,
            self.exception_big_endian,
            self.svc_exception_entry_enabled,
            self.irq_line_asserted,
            self.fiq_line_asserted,
            self.waiting_for_interrupt,
            self.waiting_for_event,
            self.event_registered,
            self.exception_return_wakeup,
            self.fault_exception_entry_enabled,
            self.prefetch_fault,
            self.fault_latched,
        ] {
            writer.write_bool(value)?;
        }

        self.cp15.write_save_state(&mut writer)?;
        self.tlb.write_save_state(&mut writer)?;
        self.vfp.write_save_state(&mut writer)?;
        Ok(writer.into_bytes())
    }

    /// Restores a complete core state transactionally. The active core remains
    /// untouched when the input is malformed or incompatible.
    pub fn restore_save_state(&mut self, encoded: &[u8]) -> std::result::Result<(), CpuStateError> {
        let mut reader = StateReader::new(encoded);
        if reader.read_bytes(8)? != b"ACARM11\0" {
            return Err(CpuStateError::InvalidMagic);
        }
        let version = reader.read_u16()?;
        if version != Self::SAVE_STATE_VERSION {
            return Err(CpuStateError::UnsupportedVersion(version));
        }

        let mut restored = Self::default();
        for register in &mut restored.regs {
            *register = reader.read_u32()?;
        }
        let cpsr = reader.read_u32()?;
        if !Psr::is_valid_mode(cpsr & Psr::MODE_MASK) {
            return Err(CpuStateError::InvalidProcessorMode(cpsr & Psr::MODE_MASK));
        }
        restored.cpsr = Psr(cpsr);
        restored.branch_taken = reader.read_bool()?;

        if reader.read_bool()? {
            let address = reader.read_u32()?;
            let size = reader.read_u8()?;
            if !matches!(size, 1 | 2 | 4 | 8) || address & u32::from(size - 1) != 0 {
                return Err(CpuStateError::InvalidReservationSize(size));
            }
            restored.exclusive_reservation = Some(ExclusiveReservation {
                address,
                size,
                version: reader.read_u64()?,
            });
        }

        for value in &mut restored.banked.user_r8_r12 {
            *value = reader.read_u32()?;
        }
        for value in &mut restored.banked.user_sp_lr {
            *value = reader.read_u32()?;
        }
        for value in &mut restored.banked.fiq_r8_r14 {
            *value = reader.read_u32()?;
        }
        for value in &mut restored.banked.irq_sp_lr {
            *value = reader.read_u32()?;
        }
        for value in &mut restored.banked.svc_sp_lr {
            *value = reader.read_u32()?;
        }
        for value in &mut restored.banked.abort_sp_lr {
            *value = reader.read_u32()?;
        }
        for value in &mut restored.banked.undefined_sp_lr {
            *value = reader.read_u32()?;
        }

        let mut read_saved_status = || -> std::result::Result<Option<Psr>, CpuStateError> {
            if !reader.read_bool()? {
                return Ok(None);
            }
            let value = reader.read_u32()?;
            if !Psr::is_valid_mode(value & Psr::MODE_MASK) {
                return Err(CpuStateError::InvalidProcessorMode(value & Psr::MODE_MASK));
            }
            Ok(Some(Psr(value)))
        };
        restored.saved_status = SavedStatusRegisters {
            fiq: read_saved_status()?,
            irq: read_saved_status()?,
            svc: read_saved_status()?,
            abort: read_saved_status()?,
            undefined: read_saved_status()?,
        };
        drop(read_saved_status);

        restored.high_vectors_enabled = reader.read_bool()?;
        restored.exception_big_endian = reader.read_bool()?;
        restored.svc_exception_entry_enabled = reader.read_bool()?;
        restored.irq_line_asserted = reader.read_bool()?;
        restored.fiq_line_asserted = reader.read_bool()?;
        restored.waiting_for_interrupt = reader.read_bool()?;
        restored.waiting_for_event = reader.read_bool()?;
        restored.event_registered = reader.read_bool()?;
        restored.exception_return_wakeup = reader.read_bool()?;
        restored.fault_exception_entry_enabled = reader.read_bool()?;
        restored.prefetch_fault = reader.read_bool()?;
        restored.fault_latched = reader.read_bool()?;

        restored.cp15 = Cp15::read_save_state(&mut reader)?;
        restored.tlb = mmu::TranslationLookasideBuffer::read_save_state(&mut reader)?;
        restored.vfp = Vfp::read_save_state(&mut reader)?;
        reader.finish()?;

        *self = restored;
        Ok(())
    }

    /// Returns a snapshot of immutable-text A32 block-cache counters.
    pub fn block_cache_stats(&self) -> BlockCacheStats {
        self.block_cache.stats()
    }

    /// Returns and resets the block-cache counters for one profiling interval.
    pub fn take_block_cache_stats(&mut self) -> BlockCacheStats {
        self.block_cache.take_stats()
    }

    /// Changes the active processor mode while preserving all banked registers.
    ///
    /// User and System share r13-r14; FIQ has private r8-r14; IRQ, Supervisor,
    /// Abort and Undefined each have private r13-r14. Registers r0-r7 are
    /// common to every mode.
    pub fn switch_mode(&mut self, mode: u32) -> Result<()> {
        if !Psr::is_valid_mode(mode) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: mode,
            });
        }
        let old_mode = self.cpsr.mode();
        if old_mode == mode {
            self.cpsr.0 = (self.cpsr.0 & !Psr::MODE_MASK) | mode;
            return Ok(());
        }

        if old_mode == Psr::FIQ_MODE {
            self.banked.fiq_r8_r14.copy_from_slice(&self.regs[8..15]);
        } else {
            self.banked.user_r8_r12.copy_from_slice(&self.regs[8..13]);
            self.save_sp_lr(old_mode);
        }

        if mode == Psr::FIQ_MODE {
            self.regs[8..15].copy_from_slice(&self.banked.fiq_r8_r14);
        } else {
            if old_mode == Psr::FIQ_MODE {
                self.regs[8..13].copy_from_slice(&self.banked.user_r8_r12);
            }
            self.load_sp_lr(mode);
        }

        self.cpsr.0 = (self.cpsr.0 & !Psr::MODE_MASK) | mode;
        Ok(())
    }

    fn save_sp_lr(&mut self, mode: u32) {
        let values = [self.regs[SP], self.regs[LR]];
        let bank = match mode {
            Psr::USER_MODE | Psr::SYSTEM_MODE => &mut self.banked.user_sp_lr,
            Psr::IRQ_MODE => &mut self.banked.irq_sp_lr,
            Psr::SUPERVISOR_MODE => &mut self.banked.svc_sp_lr,
            Psr::ABORT_MODE => &mut self.banked.abort_sp_lr,
            Psr::UNDEFINED_MODE => &mut self.banked.undefined_sp_lr,
            _ => return,
        };
        *bank = values;
    }

    fn load_sp_lr(&mut self, mode: u32) {
        let values = match mode {
            Psr::USER_MODE | Psr::SYSTEM_MODE => self.banked.user_sp_lr,
            Psr::IRQ_MODE => self.banked.irq_sp_lr,
            Psr::SUPERVISOR_MODE => self.banked.svc_sp_lr,
            Psr::ABORT_MODE => self.banked.abort_sp_lr,
            Psr::UNDEFINED_MODE => self.banked.undefined_sp_lr,
            _ => return,
        };
        self.regs[SP] = values[0];
        self.regs[LR] = values[1];
    }

    /// Configures the CP15-controlled exception vector and endianness state.
    ///
    /// Kept in sync automatically when the guest writes SCTLR through CP15; both
    /// default to the low-vector, little-endian reset profile.
    pub fn configure_exception_entry(
        &mut self,
        high_vectors_enabled: bool,
        exception_big_endian: bool,
    ) {
        self.high_vectors_enabled = high_vectors_enabled;
        self.exception_big_endian = exception_big_endian;
    }

    /// Enables or disables hardware SVC exception entry.
    ///
    /// Disabled by default so the HLE session can intercept StepEvent::Svc
    /// and dispatch OS services without requiring a mapped vector page.
    pub fn set_svc_exception_entry_enabled(&mut self, enabled: bool) {
        self.svc_exception_entry_enabled = enabled;
    }

    /// Drives the external IRQ input. The line remains asserted until the
    /// interrupt controller deasserts it; CPSR.I masks acceptance, not the line.
    pub fn set_irq_line(&mut self, asserted: bool) {
        self.irq_line_asserted = asserted;
    }

    /// Returns the current external IRQ input level for diagnostics and tests.
    pub const fn irq_line_asserted(&self) -> bool {
        self.irq_line_asserted
    }

    /// Drives the external FIQ input. FIQ has priority over IRQ when both
    /// lines are asserted and CPSR.F permits its acceptance.
    pub fn set_fiq_line(&mut self, asserted: bool) {
        self.fiq_line_asserted = asserted;
    }

    /// Enters an ARMv6 exception, banking the old CPSR and selecting its vector.
    fn enter_exception(
        &mut self,
        mode: u32,
        vector_offset: u32,
        return_address: u32,
        interrupt_masks: u32,
    ) -> Result<()> {
        let previous_cpsr = self.cpsr;
        self.switch_mode(mode)?;
        self.set_saved_program_status(previous_cpsr)?;
        self.regs[LR] = return_address;

        let mut entered_cpsr = previous_cpsr.0 & !(Psr::MODE_MASK | Psr::T | Psr::E);
        entered_cpsr |= mode | interrupt_masks;
        if self.exception_big_endian {
            entered_cpsr |= Psr::E;
        }
        self.cpsr.0 = entered_cpsr;

        let vector_base: u32 = if self.high_vectors_enabled {
            0xFFFF_0000
        } else {
            0
        };
        self.branch(vector_base.wrapping_add(vector_offset));
        Ok(())
    }

    /// Enters the ARMv6 Supervisor exception and preserves the previous CPSR.
    /// SVC uses vector offset 0x08 and masks IRQ on entry.
    fn enter_svc_exception(&mut self, return_address: u32) -> Result<()> {
        self.enter_exception(Psr::SUPERVISOR_MODE, 0x08, return_address, Psr::I)
    }

    /// Accepts a pending FIQ before IRQ, provided its CPSR mask is clear.
    /// Returns true when an exception was entered instead of fetching an
    /// instruction. The interrupt input remains level-sensitive.
    fn service_pending_interrupt(&mut self) -> Result<bool> {
        let return_address = self.pc().wrapping_add(4);
        if self.fiq_line_asserted && self.cpsr.0 & Psr::F == 0 {
            self.enter_exception(
                Psr::FIQ_MODE,
                0x1C,
                return_address,
                Psr::A | Psr::I | Psr::F,
            )?;
            return Ok(true);
        }
        if self.irq_line_asserted && self.cpsr.0 & Psr::I == 0 {
            self.enter_exception(Psr::IRQ_MODE, 0x18, return_address, Psr::A | Psr::I)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Returns the SPSR belonging to the active exception mode, if one exists.
    pub fn saved_program_status(&self) -> Option<Psr> {
        self.saved_status.get(self.cpsr.mode())
    }

    /// Stores the SPSR belonging to the active exception mode.
    /// User and System modes do not have an SPSR.
    pub fn set_saved_program_status(&mut self, value: Psr) -> Result<()> {
        if !self.saved_status.set(self.cpsr.mode(), value) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: self.cpsr.mode(),
            });
        }
        Ok(())
    }

    /// CP15 system control state.
    pub fn cp15(&self) -> &Cp15 {
        &self.cp15
    }

    /// Mutable CP15 state, e.g. for the kernel layer to set the thread ID on a context switch.
    pub fn cp15_mut(&mut self) -> &mut Cp15 {
        &mut self.cp15
    }

    pub fn pc(&self) -> u32 {
        self.regs[PC]
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.regs[PC] = pc;
    }

    /// Clears the processor's local exclusive-access reservation.
    pub fn clear_exclusive_monitor(&mut self) {
        self.exclusive_reservation = None;
    }

    /// Signals an event to this core's ARMv6K event register.
    ///
    /// The system scheduler uses this for external events, a remote core's
    /// SEV, and the cross-core return-from-exception wakeup condition.
    pub fn signal_event(&mut self) {
        self.event_registered = true;
    }

    /// Takes the pending exception-return notification for the host MPCore scheduler.
    ///
    /// A return from exception on one MP11 CPU is a WakeUpAction for the other
    /// CPUs. This latch is separate from StepEvent so direct core execution keeps
    /// its existing result semantics; process sessions translate it for scheduling.
    pub fn take_exception_return_wakeup(&mut self) -> bool {
        std::mem::take(&mut self.exception_return_wakeup)
    }

    /// Fetches and executes one instruction.
    ///
    /// With fault-exception entry enabled (see `set_fault_exception_entry_enabled`),
    /// undefined instructions and aborts become hardware exceptions instead of errors.
    pub fn step<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<StepEvent> {
        let outcome = if !self.fault_exception_entry_enabled {
            self.step_instruction(bus)
        } else {
            let snapshot = (self.regs, self.cpsr);
            self.prefetch_fault = false;
            self.fault_latched = false;
            match self.step_instruction(bus) {
                Err(error) => self.take_fault_exception(snapshot, error),
                ok => ok,
            }
        };

        // ACTLR.SMP is a core-level setting, not a cached page-table attribute.
        // Publish writes made by this instruction even when SCTLR.M is clear.
        bus.set_arm11_smp_mode(self.cp15.core_id(), self.cp15.smp_mode_enabled());
        outcome
    }

    /// True when no per-instruction check of [`Self::step`] can have an
    /// effect: no pending IRQ/FIQ line, no WFI/WFE sleep, no pending TLB
    /// maintenance and fault-exception entry off. MMU execution has its own
    /// batched path in `mmu.rs`.
    #[inline]
    fn fast_path_eligible(&self) -> bool {
        !(self.fault_exception_entry_enabled
            || self.irq_line_asserted
            || self.fiq_line_asserted
            || self.waiting_for_event
            || self.waiting_for_interrupt
            || self.cp15.has_pending_tlb_maintenance())
    }

    /// ARM instructions that may change CP15 state (and therefore what the
    /// fast loop assumes) always take the precise step. VFP (CP10/CP11)
    /// stays on the fast path.
    #[inline]
    pub(super) fn needs_precise_step(word: u32, thumb: bool) -> bool {
        if thumb {
            return false;
        }
        let coprocessor_space = (word >> 25) & 7 == 0b110 || (word >> 24) & 0xF == 0xE;
        coprocessor_space && ((word >> 8) & 0xF == 15 || word >> 28 == 0xF)
    }

    /// Executes up to `budget` instructions without repeating the
    /// per-instruction scheduler, interrupt and SMP checks of [`Self::step`].
    /// With the MMU enabled, one translated bus and TLB are retained for the
    /// batch; guest address translation and permission checks still apply to
    /// every memory access. The loop leaves for precise execution when an
    /// instruction or observable device-state change requires it.
    pub fn run_fast<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        budget: u64,
    ) -> Result<(u64, FastRunExit)> {
        if budget == 0 {
            return Ok((0, FastRunExit::Budget));
        }
        if !self.fast_path_eligible() {
            return Ok((0, FastRunExit::NeedsPreciseStep));
        }
        if self.cp15.mmu_enabled() {
            return self.run_fast_translated(bus, budget);
        }
        // ACTLR cannot change inside the loop (CP15 writes take the precise
        // step), so one publication covers the whole run.
        bus.set_arm11_smp_mode(self.cp15.core_id(), self.cp15.smp_mode_enabled());
        let mut executed = 0u64;
        while executed < budget {
            let pc = self.pc();
            let thumb = self.cpsr.thumb();
            if !thumb {
                let mode = self.cpsr.mode();
                if let Some(block) =
                    self.block_cache
                        .block_for(pc, mode, bus, &mut self.decode_cache)
                {
                    for index in 0..block.len().min((budget - executed) as usize) {
                        let Some(cached) = block.instruction(index) else {
                            break;
                        };
                        let instruction_pc = self.pc();
                        if instruction_pc != pc.wrapping_add((index as u32) * ARM_INSTRUCTION_SIZE)
                        {
                            break;
                        }
                        self.branch_taken = false;
                        let mut event = self.execute_arm_decoded(bus, cached.word, cached.entry)?;
                        if !self.branch_taken {
                            self.regs[PC] = instruction_pc.wrapping_add(ARM_INSTRUCTION_SIZE);
                        }
                        if std::mem::take(&mut self.exception_return_wakeup) {
                            event = StepEvent::ExceptionReturn;
                        }
                        if event != StepEvent::Executed {
                            return Ok((executed, FastRunExit::Event(event)));
                        }
                        executed += 1;
                        if bus.take_interrupt_state_touched() {
                            return Ok((executed, FastRunExit::Yield));
                        }
                        if cached.ends_block {
                            break;
                        }
                    }
                    continue;
                }
            }
            let fetched = if thumb {
                bus.fetch16(pc).map(u32::from)
            } else {
                bus.fetch32(pc)
            };
            let instruction = fetched.inspect_err(|_| self.prefetch_fault = true)?;
            if Self::needs_precise_step(instruction, thumb) {
                return Ok((executed, FastRunExit::NeedsPreciseStep));
            }
            self.branch_taken = false;
            let (mut event, size) = if thumb {
                (
                    self.execute_thumb(bus, instruction)?,
                    THUMB_INSTRUCTION_SIZE,
                )
            } else {
                (
                    self.execute_arm_cached(bus, instruction)?,
                    ARM_INSTRUCTION_SIZE,
                )
            };
            if !self.branch_taken {
                self.regs[PC] = pc.wrapping_add(size);
            }
            if std::mem::take(&mut self.exception_return_wakeup) {
                event = StepEvent::ExceptionReturn;
            }
            if event != StepEvent::Executed {
                return Ok((executed, FastRunExit::Event(event)));
            }
            executed += 1;
            // Eligibility cannot change here: IRQ/FIQ lines move only when the
            // bus reports interrupt state, WFI/WFE are events, CP15 (TLB
            // maintenance) takes the precise step and fault entry is host
            // configuration.
            debug_assert!(self.fast_path_eligible());
            if bus.take_interrupt_state_touched() {
                return Ok((executed, FastRunExit::Yield));
            }
        }
        Ok((executed, FastRunExit::Budget))
    }

    fn step_instruction<B: Bus + ?Sized>(&mut self, bus: &mut B) -> Result<StepEvent> {
        // An unmasked IRQ/FIQ is a WakeUpAction and sets the event register
        // even when the core is not currently asleep in WFE.
        let wakeup_action = (self.irq_line_asserted && self.cpsr.0 & Psr::I == 0)
            || (self.fiq_line_asserted && self.cpsr.0 & Psr::F == 0);
        if wakeup_action {
            self.event_registered = true;
        }

        // WFE wakes for a registered event. The event register stays set after
        // wakeup and is consumed only when a WFE instruction executes again.
        if self.waiting_for_event {
            if self.event_registered {
                self.waiting_for_event = false;
            } else {
                return Ok(StepEvent::WaitingForEvent);
            }
        }

        // WFI wakes for an IRQ or FIQ even when CPSR.I/F masks exception entry.
        if self.waiting_for_interrupt {
            if !self.irq_line_asserted && !self.fiq_line_asserted {
                return Ok(StepEvent::WaitingForInterrupt);
            }
            self.waiting_for_interrupt = false;
        }

        if let Some(operation) = self.cp15.take_tlb_maintenance() {
            self.tlb.maintain(operation);
        }

        let pc = self.pc();
        self.branch_taken = false;

        if self.service_pending_interrupt()? {
            return Ok(StepEvent::Executed);
        }

        if self.cp15.mmu_enabled() {
            return self.step_translated(bus, pc);
        }

        let thumb = self.cpsr.thumb();
        let fetched = if thumb {
            bus.fetch16(pc).map(u32::from)
        } else {
            bus.fetch32(pc)
        };
        let instruction = fetched.inspect_err(|_| self.prefetch_fault = true)?;
        let (event, size) = if thumb {
            (
                self.execute_thumb(bus, instruction)?,
                THUMB_INSTRUCTION_SIZE,
            )
        } else {
            (self.execute_arm(bus, instruction)?, ARM_INSTRUCTION_SIZE)
        };

        if !self.branch_taken {
            self.regs[PC] = pc.wrapping_add(size);
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Bus, FlatMemory};
    use aceris_common::Error;

    fn memory_with(word: u32) -> FlatMemory {
        let mut mem = FlatMemory::new(0, 16);
        mem.write32(0, word).unwrap();
        mem
    }

    #[derive(Debug)]
    struct SmpTrackingBus {
        memory: FlatMemory,
        last_smp_mode: Option<(u32, bool)>,
    }

    impl Bus for SmpTrackingBus {
        fn read8(&mut self, address: u32) -> aceris_common::Result<u8> {
            self.memory.read8(address)
        }

        fn write8(&mut self, address: u32, value: u8) -> aceris_common::Result<()> {
            self.memory.write8(address, value)
        }

        fn set_arm11_smp_mode(&mut self, core_id: u32, enabled: bool) {
            self.last_smp_mode = Some((core_id, enabled));
        }
    }

    #[test]
    fn core_step_publishes_guest_mcr_actlr_smp_even_when_mmu_is_disabled() {
        let mut bus = SmpTrackingBus {
            // MCR p15,0,r1,c1,c0,1 (ACTLR)
            memory: memory_with(0xEE01_1F30),
            last_smp_mode: None,
        };
        let mut cpu = Arm11Core::new();
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.regs[1] = 1 << 5;

        assert_eq!(cpu.step(&mut bus).unwrap(), StepEvent::Executed);
        assert!(cpu.cp15.smp_mode_enabled());
        assert_eq!(bus.last_smp_mode, Some((0, true)));
    }

    #[test]
    fn mode_switches_preserve_user_system_fiq_and_exception_register_banks() {
        let mut cpu = Arm11Core::new();
        cpu.regs[8..13].copy_from_slice(&[8, 9, 10, 11, 12]);
        cpu.regs[SP] = 0x1000;
        cpu.regs[LR] = 0x1004;

        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[0, 0, 0, 0, 0]);
        cpu.regs[8..15].copy_from_slice(&[0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xF13, 0xF14]);

        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[8, 9, 10, 11, 12]);
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0, 0));
        cpu.regs[SP] = 0x2000;
        cpu.regs[LR] = 0x2004;

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0, 0));
        cpu.regs[SP] = 0x3000;
        cpu.regs[LR] = 0x3004;

        cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[8, 9, 10, 11, 12]);
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x1000, 0x1004));

        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x2000, 0x2004));
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x3000, 0x3004));

        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        assert_eq!(
            &cpu.regs[8..15],
            &[0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xF13, 0xF14]
        );
        cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        assert_eq!(&cpu.regs[8..13], &[8, 9, 10, 11, 12]);
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x1000, 0x1004));
    }

    #[test]
    fn saved_program_status_registers_are_private_to_exception_modes() {
        let mut cpu = Arm11Core::new();
        assert_eq!(cpu.saved_program_status(), None);
        assert!(cpu.set_saved_program_status(Psr(0xA000_0010)).is_err());

        let fiq_spsr = Psr(0xF000_0011);
        let irq_spsr = Psr(0xA000_0012);
        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        cpu.set_saved_program_status(fiq_spsr).unwrap();
        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        cpu.set_saved_program_status(irq_spsr).unwrap();
        assert_eq!(cpu.saved_program_status(), Some(irq_spsr));

        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        assert_eq!(cpu.saved_program_status(), Some(fiq_spsr));
        cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        assert_eq!(cpu.saved_program_status(), None);
    }

    #[test]
    fn cpsr_mode_is_the_single_source_of_truth_for_banks_and_spsr() {
        let mut cpu = Arm11Core::new();
        let supervisor_spsr = Psr(0xA000_0010);

        // CPSR is public and may be restored by a context loader. Subsequent
        // mode-sensitive operations must follow it rather than stale metadata.
        cpu.cpsr = Psr(Psr::SUPERVISOR_MODE);
        cpu.regs[SP] = 0x3000;
        cpu.regs[LR] = 0x3004;
        cpu.set_saved_program_status(supervisor_spsr).unwrap();

        cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0, 0));
        assert_eq!(cpu.saved_program_status(), None);

        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!((cpu.regs[SP], cpu.regs[LR]), (0x3000, 0x3004));
        assert_eq!(cpu.saved_program_status(), Some(supervisor_spsr));
    }

    #[test]
    fn invalid_processor_modes_are_rejected_without_changing_the_active_bank() {
        let mut cpu = Arm11Core::new();
        cpu.regs[SP] = 0x1234;
        assert!(cpu.switch_mode(0x15).is_err());
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(cpu.regs[SP], 0x1234);
    }

    #[test]
    fn svc_is_reported_to_hle_without_hardware_exception_entry_by_default() {
        let mut memory = FlatMemory::new(0, 0x40);
        memory.write32(0, 0xEF12_3456).unwrap(); // SVC #0x123456.
        let mut cpu = Arm11Core::new();

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Svc(0x12_3456));
        assert_eq!(cpu.pc(), 4);
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(cpu.saved_program_status(), None);
    }

    #[test]
    fn svc_hardware_entry_saves_state_and_movs_pc_lr_returns_to_next_instruction() {
        let mut memory = FlatMemory::new(0, 0x40);
        memory.write32(0, 0xEF12_3456).unwrap(); // SVC #0x123456.
        memory.write32(0x08, 0xE1B0_F00E).unwrap(); // MOVS pc,lr.
        let mut cpu = Arm11Core::new();
        cpu.set_svc_exception_entry_enabled(true);
        let interrupted_status = cpu.cpsr;

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Svc(0x12_3456));
        assert_eq!(cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert_eq!(cpu.pc(), 0x08);
        assert_eq!(cpu.regs[LR], 4);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_status));
        assert_ne!(cpu.cpsr.0 & Psr::I, 0);
        assert_eq!(cpu.cpsr.0 & Psr::A, interrupted_status.0 & Psr::A);
        assert_eq!(cpu.cpsr.0 & Psr::F, interrupted_status.0 & Psr::F);

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
        assert_eq!(cpu.cpsr, interrupted_status);
    }

    #[test]
    fn irq_waits_for_unmasking_then_enters_irq_vector_and_returns() {
        let mut memory = FlatMemory::new(0, 0x40);
        memory.write32(0, 0xE1A0_0000).unwrap(); // MOV r0,r0
        memory.write32(0x18, 0xE25E_F004).unwrap(); // SUBS pc,lr,#4
        let mut cpu = Arm11Core::new();
        cpu.set_irq_line(true);
        cpu.cpsr.0 |= Psr::I;

        // A masked IRQ remains asserted while the current instruction runs.
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
        assert_eq!(cpu.cpsr.mode(), Psr::USER_MODE);

        cpu.cpsr.0 &= !Psr::I;
        let interrupted_status = cpu.cpsr;
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
        assert_eq!(cpu.pc(), 0x18);
        assert_eq!(cpu.regs[LR], 8);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_status));
        assert_ne!(cpu.cpsr.0 & Psr::I, 0);
        assert_ne!(cpu.cpsr.0 & Psr::A, 0);

        // Deassert the level before the handler returns to the interrupted PC.
        cpu.set_irq_line(false);
        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
        assert_eq!(cpu.cpsr, interrupted_status);
    }

    #[test]
    fn fiq_has_priority_and_sets_all_exception_masks() {
        let mut memory = FlatMemory::new(0, 4);
        let mut cpu = Arm11Core::new();
        cpu.set_irq_line(true);
        cpu.set_fiq_line(true);
        let interrupted_status = cpu.cpsr;

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::FIQ_MODE);
        assert_eq!(cpu.pc(), 0x1C);
        assert_eq!(cpu.regs[LR], 4);
        assert_eq!(cpu.saved_program_status(), Some(interrupted_status));
        assert_eq!(
            cpu.cpsr.0 & (Psr::A | Psr::I | Psr::F),
            Psr::A | Psr::I | Psr::F
        );
    }

    #[test]
    fn masked_fiq_does_not_block_an_unmasked_irq() {
        let mut memory = FlatMemory::new(0, 4);
        let mut cpu = Arm11Core::new();
        cpu.cpsr.0 |= Psr::F;
        cpu.set_irq_line(true);
        cpu.set_fiq_line(true);

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
        assert_eq!(cpu.pc(), 0x18);
        assert_ne!(cpu.cpsr.0 & Psr::A, 0);
        assert_ne!(cpu.cpsr.0 & Psr::I, 0);
        assert_ne!(cpu.cpsr.0 & Psr::F, 0);
    }

    #[test]
    fn interrupt_entry_uses_high_vectors_and_exception_endianness() {
        let mut memory = FlatMemory::new(0, 4);
        let mut cpu = Arm11Core::new();
        cpu.configure_exception_entry(true, true);
        cpu.set_irq_line(true);

        assert_eq!(cpu.step(&mut memory).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 0xFFFF_0018);
        assert!(cpu.cpsr.big_endian());
        assert_eq!(cpu.cpsr.mode(), Psr::IRQ_MODE);
    }

    #[test]
    fn arm11_save_state_round_trips_architectural_cpu_context() {
        let mut cpu = Arm11Core::new();
        cpu.regs = [
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
            0x1E, 0x1F,
        ];
        cpu.cpsr = Psr(Psr::SUPERVISOR_MODE | Psr::N | Psr::I | Psr::Q);
        cpu.switch_mode(Psr::FIQ_MODE).unwrap();
        cpu.regs[8..15].copy_from_slice(&[0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86]);
        cpu.cpsr = Psr(Psr::FIQ_MODE | Psr::F);
        cpu.saved_status.fiq = Some(Psr(Psr::SUPERVISOR_MODE | Psr::T | Psr::Z));
        cpu.saved_status.irq = Some(Psr(Psr::USER_MODE | Psr::I));

        cpu.branch_taken = true;
        cpu.exclusive_reservation = Some(ExclusiveReservation {
            address: 0x1000,
            size: 4,
            version: 0x1234_5678,
        });
        cpu.high_vectors_enabled = true;
        cpu.exception_big_endian = true;
        cpu.svc_exception_entry_enabled = true;
        cpu.irq_line_asserted = true;
        cpu.fiq_line_asserted = true;
        cpu.waiting_for_event = true;
        cpu.event_registered = true;
        cpu.exception_return_wakeup = true;
        cpu.fault_exception_entry_enabled = true;

        cpu.cp15.write(true, 0, 1, 0, 0, 0x00C5_0079).unwrap();
        cpu.cp15.write(true, 0, 2, 0, 2, 3).unwrap();
        cpu.cp15.write(true, 0, 8, 5, 1, 0x1234_5678).unwrap();
        cpu.vfp.set_s(7, 0x7FC0_1234);
        cpu.vfp.set_enabled(true);
        cpu.vfp.set_fpinst(0xEE00_0A10);
        cpu.vfp.set_fpinst2(0xEE00_0A20);

        let encoded = cpu.encode_save_state().unwrap();
        let mut restored = Arm11Core::new();
        restored.restore_save_state(&encoded).unwrap();

        assert_eq!(restored.regs, cpu.regs);
        assert_eq!(restored.cpsr, cpu.cpsr);
        assert_eq!(restored.banked.user_r8_r12, cpu.banked.user_r8_r12);
        assert_eq!(restored.banked.fiq_r8_r14, cpu.banked.fiq_r8_r14);
        assert_eq!(restored.saved_status.fiq, cpu.saved_status.fiq);
        assert_eq!(restored.saved_status.irq, cpu.saved_status.irq);
        assert_eq!(restored.exclusive_reservation, cpu.exclusive_reservation);
        assert_eq!(restored.cp15.sctlr(), cpu.cp15.sctlr());
        assert_eq!(restored.cp15.ttbr0(), cpu.cp15.ttbr0());
        assert_eq!(restored.vfp.s(7), cpu.vfp.s(7));
        assert_eq!(restored.vfp.fpexc(), cpu.vfp.fpexc());
        assert_eq!(restored.vfp.fpinst(), cpu.vfp.fpinst());
        assert_eq!(restored.vfp.fpinst2(), cpu.vfp.fpinst2());
        assert_eq!(restored.encode_save_state().unwrap(), encoded);
    }

    #[test]
    fn malformed_arm11_save_state_does_not_change_the_active_core() {
        let mut cpu = Arm11Core::new();
        cpu.regs[0] = 0xDEAD_BEEF;
        let before = cpu.encode_save_state().unwrap();

        assert!(cpu.restore_save_state(&before[..before.len() - 1]).is_err());
        assert_eq!(cpu.encode_save_state().unwrap(), before);

        let mut invalid_mode = before.clone();
        invalid_mode[10 + 16 * 4..10 + 17 * 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(cpu.restore_save_state(&invalid_mode).is_err());
        assert_eq!(cpu.encode_save_state().unwrap(), before);

        let mut trailing = before.clone();
        trailing.push(0);
        assert!(cpu.restore_save_state(&trailing).is_err());
        assert_eq!(cpu.encode_save_state().unwrap(), before);
    }

    #[test]
    fn instruction_with_failing_condition_is_skipped() {
        // `BNE` encoding; Z is set, so it must not execute.
        let mut mem = memory_with(0x1A00_0000);
        let mut cpu = Arm11Core::new();
        cpu.cpsr = Psr::from_flags(false, true, false, false);

        assert_eq!(cpu.step(&mut mem).unwrap(), StepEvent::Executed);
        assert_eq!(cpu.pc(), 4);
    }

    #[test]
    fn fetch_from_unmapped_memory_fails() {
        let mut mem = FlatMemory::new(0x1000, 16);
        let mut cpu = Arm11Core::new();
        assert!(matches!(
            cpu.step(&mut mem),
            Err(Error::InvalidAccess { .. })
        ));
    }

    #[test]
    fn thumb_state_fetches_halfwords() {
        let mut mem = memory_with(0x2105_2007); // MOV r0,#7 ; MOV r1,#5
        let mut cpu = Arm11Core::new();
        cpu.cpsr.set_thumb(true);
        cpu.step(&mut mem).unwrap();
        cpu.step(&mut mem).unwrap();
        assert_eq!((cpu.regs[0], cpu.regs[1], cpu.pc()), (7, 5, 4));
    }
}
