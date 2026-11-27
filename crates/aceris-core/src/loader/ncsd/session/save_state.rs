//! Versioned snapshot of ARM11 process-local wait and interrupt state.
//!
//! This section complements the kernel object table and the per-thread CPU
//! scheduler. It stores process-side references and blocking metadata that do
//! not belong to either component.

use std::collections::BTreeMap;

use crate::cpu::state::{StateReader, StateWriter};
use crate::loader::ncsd::session::kernel::save_state::KernelStateError;
use crate::save_state::{SaveStateSection, section_id};

use super::{
    Arm11InterruptController, Arm11ProcessSession, InterruptBinding, InterruptTargetKind, ThreadId,
};

const MAGIC: &[u8; 8] = b"ACPRWAIT";
const FORMAT_VERSION: u16 = 1;
const MAX_PENDING_THREADS: usize = 512;
const MAX_WAIT_HANDLES: usize = 1024;
const MAX_ARBITER_WAITERS: usize = 4096;
const MAX_INTERRUPT_BINDINGS: usize = 97;
const MAX_SECTION_BYTES: usize = 4 * 1024 * 1024;

impl Arm11ProcessSession {
    /// Encodes wait deadlines, address-arbiter queues, multi-object waits and
    /// user-bound interrupt registrations as a distinct ACRPAYLD section.
    pub(crate) fn encode_runtime_state_section(
        &self,
    ) -> Result<SaveStateSection, KernelStateError> {
        let mut writer = StateWriter::with_capacity(1024)?;
        writer.write_bytes(MAGIC)?;
        writer.write_u16(FORMAT_VERSION)?;
        writer.write_u32(self.process_id)?;
        writer.write_bool(self.external_irq_asserted)?;

        write_count(
            &mut writer,
            self.wait_deadlines.len(),
            MAX_PENDING_THREADS,
            "wait deadlines",
        )?;
        for (thread, (handle, deadline)) in &self.wait_deadlines {
            write_thread_id(&mut writer, *thread)?;
            if *handle == 0 {
                return Err(KernelStateError::InvalidData(
                    "wait deadline has a null handle",
                ));
            }
            writer.write_u32(*handle)?;
            writer.write_u64(*deadline)?;
        }

        write_count(
            &mut writer,
            self.arbiter_waiters.len(),
            MAX_ARBITER_WAITERS,
            "address-arbiter waiters",
        )?;
        for (address, thread) in &self.arbiter_waiters {
            if address & 3 != 0 {
                return Err(KernelStateError::InvalidData(
                    "address-arbiter waiter address is unaligned",
                ));
            }
            writer.write_u32(*address)?;
            write_thread_id(&mut writer, *thread)?;
        }

        write_wait_map(&mut writer, &self.multi_waits, "wait-any entries")?;
        write_wait_map(&mut writer, &self.wait_all_waits, "wait-all entries")?;

        let bindings = self.interrupts.save_state_bindings();
        write_count(
            &mut writer,
            bindings.len(),
            MAX_INTERRUPT_BINDINGS,
            "interrupt bindings",
        )?;
        for binding in bindings {
            writer.write_u8(binding.interrupt_id)?;
            if binding.handle == 0 {
                return Err(KernelStateError::InvalidData(
                    "interrupt binding has a null handle",
                ));
            }
            writer.write_u32(binding.handle)?;
            writer.write_u8(binding.priority)?;
            writer.write_bool(binding.level_high_active)?;
            writer.write_u8(match binding.target {
                InterruptTargetKind::Event => 0,
                InterruptTargetKind::Semaphore => 1,
            })?;
        }

        let payload = writer.into_bytes();
        if payload.len() > MAX_SECTION_BYTES {
            return Err(KernelStateError::PayloadTooLarge(payload.len() as u64));
        }
        SaveStateSection::new(section_id::ARM11_PROCESS_RUNTIME, FORMAT_VERSION, payload)
            .map_err(|_| KernelStateError::InvalidSection)
    }

    /// Restores process-side wait metadata atomically. Every collection and the
    /// complete interrupt table are decoded into temporary values first.
    pub(crate) fn restore_runtime_state_section(
        &mut self,
        section: &SaveStateSection,
    ) -> Result<(), KernelStateError> {
        if section.id() != section_id::ARM11_PROCESS_RUNTIME || section.version() != FORMAT_VERSION
        {
            return Err(KernelStateError::InvalidSection);
        }
        if section.payload().len() > MAX_SECTION_BYTES {
            return Err(KernelStateError::PayloadTooLarge(
                section.payload().len() as u64
            ));
        }

        let mut reader = StateReader::new(section.payload());
        if reader.read_bytes(8)? != MAGIC.as_slice() {
            return Err(KernelStateError::InvalidData(
                "process-runtime signature does not match",
            ));
        }
        if reader.read_u16()? != FORMAT_VERSION {
            return Err(KernelStateError::InvalidData(
                "process-runtime section version is unsupported",
            ));
        }

        let process_id = reader.read_u32()?;
        if process_id == 0 {
            return Err(KernelStateError::InvalidData("process ID is zero"));
        }
        let external_irq_asserted = reader.read_bool()?;

        let deadline_count = read_count(&mut reader, MAX_PENDING_THREADS, "wait deadlines")?;
        let mut wait_deadlines = BTreeMap::new();
        for _ in 0..deadline_count {
            let thread = read_thread_id(&mut reader)?;
            let handle = reader.read_u32()?;
            let deadline = reader.read_u64()?;
            if handle == 0 || wait_deadlines.insert(thread, (handle, deadline)).is_some() {
                return Err(KernelStateError::InvalidData(
                    "wait deadline entry is invalid or duplicated",
                ));
            }
        }

        let arbiter_count =
            read_count(&mut reader, MAX_ARBITER_WAITERS, "address-arbiter waiters")?;
        let mut arbiter_waiters = Vec::new();
        arbiter_waiters
            .try_reserve_exact(arbiter_count)
            .map_err(|_| KernelStateError::InvalidData("could not allocate arbiter waiters"))?;
        for _ in 0..arbiter_count {
            let address = reader.read_u32()?;
            let thread = read_thread_id(&mut reader)?;
            if address & 3 != 0 {
                return Err(KernelStateError::InvalidData(
                    "address-arbiter waiter address is unaligned",
                ));
            }
            arbiter_waiters.push((address, thread));
        }

        let multi_waits = read_wait_map(&mut reader, "wait-any entries")?;
        let wait_all_waits = read_wait_map(&mut reader, "wait-all entries")?;

        let binding_count = read_count(&mut reader, MAX_INTERRUPT_BINDINGS, "interrupt bindings")?;
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(binding_count)
            .map_err(|_| KernelStateError::InvalidData("could not allocate interrupt bindings"))?;
        for _ in 0..binding_count {
            let interrupt_id = reader.read_u8()?;
            let handle = reader.read_u32()?;
            let priority = reader.read_u8()?;
            let level_high_active = reader.read_bool()?;
            let target = match reader.read_u8()? {
                0 => InterruptTargetKind::Event,
                1 => InterruptTargetKind::Semaphore,
                _ => {
                    return Err(KernelStateError::InvalidData(
                        "interrupt target kind is invalid",
                    ));
                }
            };
            if handle == 0 {
                return Err(KernelStateError::InvalidData(
                    "interrupt binding has a null handle",
                ));
            }
            bindings.push(InterruptBinding {
                interrupt_id,
                handle,
                priority,
                level_high_active,
                target,
            });
        }
        reader.finish()?;

        let mut interrupts = Arm11InterruptController::default();
        interrupts
            .restore_save_state_bindings(&bindings)
            .map_err(|_| KernelStateError::InvalidData("interrupt binding table is invalid"))?;

        self.process_id = process_id;
        self.external_irq_asserted = external_irq_asserted;
        self.wait_deadlines = wait_deadlines;
        self.arbiter_waiters = arbiter_waiters;
        self.multi_waits = multi_waits;
        self.wait_all_waits = wait_all_waits;
        self.interrupts = interrupts;
        Ok(())
    }
}

fn write_wait_map(
    writer: &mut StateWriter,
    waits: &BTreeMap<ThreadId, Vec<u32>>,
    kind: &'static str,
) -> Result<(), KernelStateError> {
    write_count(writer, waits.len(), MAX_PENDING_THREADS, kind)?;
    for (thread, handles) in waits {
        write_thread_id(writer, *thread)?;
        write_count(writer, handles.len(), MAX_WAIT_HANDLES, "wait handles")?;
        for handle in handles {
            if *handle == 0 {
                return Err(KernelStateError::InvalidData(
                    "wait list contains a null handle",
                ));
            }
            writer.write_u32(*handle)?;
        }
    }
    Ok(())
}

fn read_wait_map(
    reader: &mut StateReader<'_>,
    kind: &'static str,
) -> Result<BTreeMap<ThreadId, Vec<u32>>, KernelStateError> {
    let count = read_count(reader, MAX_PENDING_THREADS, kind)?;
    let mut waits = BTreeMap::new();
    for _ in 0..count {
        let thread = read_thread_id(reader)?;
        let handle_count = read_count(reader, MAX_WAIT_HANDLES, "wait handles")?;
        let mut handles = Vec::new();
        handles
            .try_reserve_exact(handle_count)
            .map_err(|_| KernelStateError::InvalidData("could not allocate wait handles"))?;
        for _ in 0..handle_count {
            let handle = reader.read_u32()?;
            if handle == 0 {
                return Err(KernelStateError::InvalidData(
                    "wait list contains a null handle",
                ));
            }
            handles.push(handle);
        }
        if waits.insert(thread, handles).is_some() {
            return Err(KernelStateError::InvalidData(
                "process wait map contains a duplicate thread",
            ));
        }
    }
    Ok(waits)
}

fn write_thread_id(writer: &mut StateWriter, thread: ThreadId) -> Result<(), KernelStateError> {
    if thread.raw() == 0 {
        return Err(KernelStateError::InvalidData("thread ID zero is reserved"));
    }
    writer.write_u32(thread.raw())?;
    Ok(())
}

fn read_thread_id(reader: &mut StateReader<'_>) -> Result<ThreadId, KernelStateError> {
    let thread = ThreadId::from_raw(reader.read_u32()?);
    if thread.raw() == 0 {
        return Err(KernelStateError::InvalidData("thread ID zero is reserved"));
    }
    Ok(thread)
}

fn write_count(
    writer: &mut StateWriter,
    count: usize,
    maximum: usize,
    kind: &'static str,
) -> Result<(), KernelStateError> {
    if count > maximum {
        return Err(KernelStateError::TooManyEntries {
            kind,
            count: u32::try_from(count).unwrap_or(u32::MAX),
        });
    }
    writer.write_u32(u32::try_from(count).map_err(|_| KernelStateError::InvalidData(kind))?)?;
    Ok(())
}

fn read_count(
    reader: &mut StateReader<'_>,
    maximum: usize,
    kind: &'static str,
) -> Result<usize, KernelStateError> {
    let count = reader.read_u32()? as usize;
    if count > maximum {
        return Err(KernelStateError::TooManyEntries {
            kind,
            count: u32::try_from(count).unwrap_or(u32::MAX),
        });
    }
    Ok(count)
}
