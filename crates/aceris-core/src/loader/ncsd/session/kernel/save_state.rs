//! Versioned, bounded serialization for the process-local ARM11 kernel table.
//!
//! Shared-memory blocks store references into the machine-wide backing catalog.
//! Their bytes are emitted once by that catalog, preserving aliasing between
//! address-space mappings, kernel handles and HLE services.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use super::*;
use crate::cpu::state::{CpuStateError, StateReader, StateWriter};
use crate::loader::ncsd::process::{SharedMemoryBackingCatalog, SharedMemoryStateError};
use crate::save_state::{section_id, SaveStateSection};

const MAGIC: &[u8; 8] = b"ACKERNEL";
const FORMAT_VERSION: u16 = 1;
const MAX_KERNEL_OBJECTS: usize = 65_536;
const MAX_KERNEL_HANDLES: usize = 131_072;
const MAX_KERNEL_WAITERS: usize = 65_536;
const MAX_WAIT_ALL_HANDLES: usize = 4_096;
const MAX_KERNEL_SECTION_BYTES: usize = 64 * 1024 * 1024;

const OBJECT_THREAD: u8 = 1;
const OBJECT_EVENT: u8 = 2;
const OBJECT_MUTEX: u8 = 3;
const OBJECT_SEMAPHORE: u8 = 4;
const OBJECT_TIMER: u8 = 5;
const OBJECT_MEMORY_BLOCK: u8 = 6;
const OBJECT_RESOURCE_LIMIT: u8 = 7;
const OBJECT_PORT: u8 = 8;
const OBJECT_SESSION_ENDPOINT: u8 = 9;
const OBJECT_SERVICE_SESSION: u8 = 10;
const OBJECT_ADDRESS_ARBITER: u8 = 11;

/// Failures produced while encoding or restoring the ARM11 kernel table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KernelStateError {
    Codec(CpuStateError),
    SharedMemory(SharedMemoryStateError),
    InvalidSection,
    InvalidData(&'static str),
    TooManyEntries { kind: &'static str, count: u32 },
    PayloadTooLarge(u64),
}

impl fmt::Display for KernelStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => write!(formatter, "kernel state codec failed: {error}"),
            Self::SharedMemory(error) => {
                write!(formatter, "kernel shared-memory reference failed: {error}")
            }
            Self::InvalidSection => {
                formatter.write_str("ARM11 kernel save-state section ID or version is invalid")
            }
            Self::InvalidData(reason) => {
                write!(formatter, "ARM11 kernel save-state is invalid: {reason}")
            }
            Self::TooManyEntries { kind, count } => {
                write!(formatter, "ARM11 kernel save-state contains too many {kind}: {count}")
            }
            Self::PayloadTooLarge(size) => {
                write!(formatter, "ARM11 kernel save-state payload is too large: {size}")
            }
        }
    }
}

impl std::error::Error for KernelStateError {}

impl From<CpuStateError> for KernelStateError {
    fn from(error: CpuStateError) -> Self {
        Self::Codec(error)
    }
}

impl From<SharedMemoryStateError> for KernelStateError {
    fn from(error: SharedMemoryStateError) -> Self {
        Self::SharedMemory(error)
    }
}

impl Arm11KernelObjectTable {
    /// Version of the ARM11 kernel-object section.
    pub(crate) const SAVE_STATE_VERSION: u16 = FORMAT_VERSION;

    /// Serializes live and retired objects, aliases, wait queues and service registrations.
    ///
    /// Shared-memory block bytes are stored by the machine backing catalog, not
    /// duplicated in the kernel section.
    pub(crate) fn encode_save_state_section(
        &self,
        catalog: &SharedMemoryBackingCatalog,
    ) -> Result<SaveStateSection, KernelStateError> {
        validate_table(self)?;

        let mut writer = StateWriter::with_capacity(4096)?;
        writer.write_bytes(MAGIC)?;
        writer.write_u16(FORMAT_VERSION)?;

        write_object_map(&mut writer, &self.objects, catalog)?;
        write_object_map(&mut writer, &self.retired_objects, catalog)?;

        write_count(&mut writer, self.handle_aliases.len(), MAX_KERNEL_HANDLES, "handles")?;
        for (handle, object_key) in &self.handle_aliases {
            writer.write_u32(*handle)?;
            writer.write_u32(*object_key)?;
        }

        write_count(
            &mut writer,
            self.service_registry.len(),
            MAX_KERNEL_OBJECTS,
            "service registrations",
        )?;
        for (name, handle) in &self.service_registry {
            writer.write_bytes(name)?;
            writer.write_u32(*handle)?;
        }

        write_count(
            &mut writer,
            self.waiter_handles.len(),
            MAX_KERNEL_WAITERS,
            "waiter references",
        )?;
        for ((thread, object_key), handle) in &self.waiter_handles {
            write_thread_id(&mut writer, *thread)?;
            writer.write_u32(*object_key)?;
            writer.write_u32(*handle)?;
        }

        write_count(
            &mut writer,
            self.wait_all_refs.len(),
            MAX_KERNEL_WAITERS,
            "wait-all owners",
        )?;
        for (thread, handles) in &self.wait_all_refs {
            write_thread_id(&mut writer, *thread)?;
            write_count(
                &mut writer,
                handles.len(),
                MAX_WAIT_ALL_HANDLES,
                "wait-all handles",
            )?;
            for (handle, object_key) in handles {
                writer.write_u32(*handle)?;
                writer.write_u32(*object_key)?;
            }
        }

        writer.write_u32(to_u32(self.high_water_handle_count, "handle high-water mark")?)?;
        writer.write_u32(self.next_handle)?;

        write_count(
            &mut writer,
            self.handoffs.len(),
            MAX_KERNEL_WAITERS,
            "thread handoffs",
        )?;
        for (thread, handle) in &self.handoffs {
            write_thread_id(&mut writer, *thread)?;
            writer.write_u32(*handle)?;
        }

        let encoded = writer.into_bytes();
        if encoded.len() > MAX_KERNEL_SECTION_BYTES {
            return Err(KernelStateError::PayloadTooLarge(encoded.len() as u64));
        }
        SaveStateSection::new(section_id::KERNEL, FORMAT_VERSION, encoded)
            .map_err(|_| KernelStateError::InvalidSection)
    }

    /// Decodes a complete replacement table before replacing any live kernel state.
    ///
    /// A malformed section, unresolved backing or invalid cross-reference leaves
    /// the current table untouched.
    pub(crate) fn restore_save_state_section(
        &mut self,
        section: &SaveStateSection,
        catalog: &SharedMemoryBackingCatalog,
    ) -> Result<(), KernelStateError> {
        if section.id() != section_id::KERNEL || section.version() != FORMAT_VERSION {
            return Err(KernelStateError::InvalidSection);
        }
        let restored = decode_table(section.payload(), catalog)?;
        *self = restored;
        Ok(())
    }
}

fn write_object_map(
    writer: &mut StateWriter,
    objects: &BTreeMap<u32, KernelObject>,
    catalog: &SharedMemoryBackingCatalog,
) -> Result<(), KernelStateError> {
    write_count(writer, objects.len(), MAX_KERNEL_OBJECTS, "kernel objects")?;
    for (key, object) in objects {
        writer.write_u32(*key)?;
        write_object(writer, object, catalog)?;
    }
    Ok(())
}

fn write_object(
    writer: &mut StateWriter,
    object: &KernelObject,
    catalog: &SharedMemoryBackingCatalog,
) -> Result<(), KernelStateError> {
    match object {
        KernelObject::Thread(thread) => {
            writer.write_u8(OBJECT_THREAD)?;
            write_thread_id(writer, thread.id)?;
            writer.write_bool(thread.terminated)?;
            write_thread_queue(writer, &thread.waiters)?;
        }
        KernelObject::Event(event) => {
            writer.write_u8(OBJECT_EVENT)?;
            write_reset_type(writer, event.reset_type)?;
            writer.write_bool(event.signaled)?;
            write_thread_queue(writer, &event.waiters)?;
        }
        KernelObject::Mutex(mutex) => {
            writer.write_u8(OBJECT_MUTEX)?;
            write_optional_thread_id(writer, mutex.owner)?;
            write_thread_queue(writer, &mutex.waiters)?;
        }
        KernelObject::Semaphore(semaphore) => {
            writer.write_u8(OBJECT_SEMAPHORE)?;
            writer.write_u32(semaphore.count)?;
            writer.write_u32(semaphore.maximum)?;
            write_thread_queue(writer, &semaphore.waiters)?;
        }
        KernelObject::Timer(timer) => {
            writer.write_u8(OBJECT_TIMER)?;
            write_reset_type(writer, timer.reset_type)?;
            writer.write_bool(timer.signaled)?;
            write_optional_u64(writer, timer.deadline_ns)?;
            writer.write_u64(timer.interval_ns)?;
            write_thread_queue(writer, &timer.waiters)?;
        }
        KernelObject::MemoryBlock(block) => {
            writer.write_u8(OBJECT_MEMORY_BLOCK)?;
            let length = block
                .backing
                .len()
                .ok_or(SharedMemoryStateError::LockPoisoned)?;
            let slot = catalog.slot_for(&block.backing)?;
            let offset = u64::try_from(block.backing.root_base_offset())
                .map_err(|_| KernelStateError::InvalidData("memory-block offset overflows"))?;
            let length = u64::try_from(length)
                .map_err(|_| KernelStateError::InvalidData("memory-block length overflows"))?;
            writer.write_u32(slot)?;
            writer.write_u64(offset)?;
            writer.write_u64(length)?;
            writer.write_u32(block.owner_permissions)?;
            writer.write_u32(block.other_permissions)?;
        }
        KernelObject::ResourceLimit => writer.write_u8(OBJECT_RESOURCE_LIMIT)?,
        KernelObject::Port(port) => {
            writer.write_u8(OBJECT_PORT)?;
            writer.write_u16(port.max_sessions)?;
            writer.write_u16(port.active_sessions)?;
            write_u32_queue(writer, &port.pending_sessions)?;
        }
        KernelObject::SessionEndpoint(endpoint) => {
            writer.write_u8(OBJECT_SESSION_ENDPOINT)?;
            writer.write_u32(endpoint.peer_key)?;
            writer.write_u32(endpoint.port_key)?;
            writer.write_u8(match endpoint.role {
                SessionRole::Client => 0,
                SessionRole::Server => 1,
            })?;
            writer.write_bool(endpoint.remote_closed)?;
        }
        KernelObject::ServiceSession(kind) => {
            writer.write_u8(OBJECT_SERVICE_SESSION)?;
            write_service_session(writer, *kind)?;
        }
        KernelObject::AddressArbiter => writer.write_u8(OBJECT_ADDRESS_ARBITER)?,
    }
    Ok(())
}

fn write_service_session(
    writer: &mut StateWriter,
    kind: ServiceSessionKind,
) -> Result<(), KernelStateError> {
    match kind {
        ServiceSessionKind::ServiceManager { client_registered } => {
            writer.write_u8(0)?;
            writer.write_bool(client_registered)?;
        }
        ServiceSessionKind::GspGpu {
            client_id,
            shared_memory_handle,
            event_handle,
            flags,
        } => {
            writer.write_u8(1)?;
            write_optional_u8(writer, client_id)?;
            write_optional_u32(writer, shared_memory_handle)?;
            write_optional_u32(writer, event_handle)?;
            writer.write_u32(flags)?;
        }
        ServiceSessionKind::Hid => writer.write_u8(2)?,
        ServiceSessionKind::Cfg => writer.write_u8(3)?,
        ServiceSessionKind::Apt => writer.write_u8(4)?,
        ServiceSessionKind::Fs => writer.write_u8(5)?,
        ServiceSessionKind::Ptm => writer.write_u8(6)?,
        ServiceSessionKind::Dsp => writer.write_u8(7)?,
        ServiceSessionKind::Csnd => writer.write_u8(8)?,
        ServiceSessionKind::FsRomFsFile => writer.write_u8(9)?,
        ServiceSessionKind::FsHostFile => writer.write_u8(10)?,
        ServiceSessionKind::FsDirectory => writer.write_u8(11)?,
        ServiceSessionKind::Stub(index) => {
            writer.write_u8(12)?;
            writer.write_u8(index)?;
        }
    }
    Ok(())
}

fn decode_table(
    encoded: &[u8],
    catalog: &SharedMemoryBackingCatalog,
) -> Result<Arm11KernelObjectTable, KernelStateError> {
    if encoded.len() > MAX_KERNEL_SECTION_BYTES {
        return Err(KernelStateError::PayloadTooLarge(encoded.len() as u64));
    }
    let mut reader = StateReader::new(encoded);
    if reader.read_bytes(8)? != MAGIC {
        return Err(KernelStateError::InvalidData("signature does not match"));
    }
    let version = reader.read_u16()?;
    if version != FORMAT_VERSION {
        return Err(KernelStateError::InvalidData("unsupported kernel-state version"));
    }

    let objects = read_object_map(&mut reader, catalog)?;
    let retired_objects = read_object_map(&mut reader, catalog)?;

    let alias_count = read_count(&mut reader, MAX_KERNEL_HANDLES, "handles")?;
    let mut handle_aliases = BTreeMap::new();
    for _ in 0..alias_count {
        let handle = reader.read_u32()?;
        let object_key = reader.read_u32()?;
        if handle_aliases.insert(handle, object_key).is_some() {
            return Err(KernelStateError::InvalidData("duplicate alias handle"));
        }
    }

    let registry_count = read_count(&mut reader, MAX_KERNEL_OBJECTS, "service registrations")?;
    let mut service_registry = BTreeMap::new();
    for _ in 0..registry_count {
        let mut name = [0u8; 8];
        name.copy_from_slice(reader.read_bytes(8)?);
        let handle = reader.read_u32()?;
        if service_registry.insert(name, handle).is_some() {
            return Err(KernelStateError::InvalidData("duplicate service name"));
        }
    }

    let waiter_count = read_count(&mut reader, MAX_KERNEL_WAITERS, "waiter references")?;
    let mut waiter_handles = BTreeMap::new();
    for _ in 0..waiter_count {
        let thread = read_thread_id(&mut reader)?;
        let object_key = reader.read_u32()?;
        let handle = reader.read_u32()?;
        if waiter_handles.insert((thread, object_key), handle).is_some() {
            return Err(KernelStateError::InvalidData("duplicate waiter reference"));
        }
    }

    let wait_all_count = read_count(&mut reader, MAX_KERNEL_WAITERS, "wait-all owners")?;
    let mut wait_all_refs = BTreeMap::new();
    for _ in 0..wait_all_count {
        let thread = read_thread_id(&mut reader)?;
        let handle_count = read_count(&mut reader, MAX_WAIT_ALL_HANDLES, "wait-all handles")?;
        let mut handles = Vec::new();
        handles
            .try_reserve_exact(handle_count)
            .map_err(|_| KernelStateError::InvalidData("could not allocate wait-all references"))?;
        for _ in 0..handle_count {
            handles.push((reader.read_u32()?, reader.read_u32()?));
        }
        if wait_all_refs.insert(thread, handles).is_some() {
            return Err(KernelStateError::InvalidData("duplicate wait-all owner"));
        }
    }

    let high_water_handle_count = reader.read_u32()? as usize;
    let next_handle = reader.read_u32()?;

    let handoff_count = read_count(&mut reader, MAX_KERNEL_WAITERS, "thread handoffs")?;
    let mut handoffs = BTreeMap::new();
    for _ in 0..handoff_count {
        let thread = read_thread_id(&mut reader)?;
        let handle = reader.read_u32()?;
        if handoffs.insert(thread, handle).is_some() {
            return Err(KernelStateError::InvalidData("duplicate thread handoff"));
        }
    }

    reader.finish()?;
    let restored = Arm11KernelObjectTable {
        objects,
        retired_objects,
        handle_aliases,
        service_registry,
        waiter_handles,
        wait_all_refs,
        high_water_handle_count,
        next_handle,
        handoffs,
    };
    validate_table(&restored)?;
    Ok(restored)
}

fn read_object_map(
    reader: &mut StateReader<'_>,
    catalog: &SharedMemoryBackingCatalog,
) -> Result<BTreeMap<u32, KernelObject>, KernelStateError> {
    let count = read_count(reader, MAX_KERNEL_OBJECTS, "kernel objects")?;
    let mut objects = BTreeMap::new();
    for _ in 0..count {
        let key = reader.read_u32()?;
        if key < FIRST_HANDLE || key == CURRENT_THREAD_HANDLE || key == CURRENT_PROCESS_HANDLE {
            return Err(KernelStateError::InvalidData("invalid kernel object key"));
        }
        let object = read_object(reader, catalog)?;
        if objects.insert(key, object).is_some() {
            return Err(KernelStateError::InvalidData("duplicate kernel object key"));
        }
    }
    Ok(objects)
}

fn read_object(
    reader: &mut StateReader<'_>,
    catalog: &SharedMemoryBackingCatalog,
) -> Result<KernelObject, KernelStateError> {
    match reader.read_u8()? {
        OBJECT_THREAD => Ok(KernelObject::Thread(KernelThread {
            id: read_thread_id(reader)?,
            terminated: reader.read_bool()?,
            waiters: read_thread_queue(reader)?,
        })),
        OBJECT_EVENT => Ok(KernelObject::Event(KernelEvent {
            reset_type: read_reset_type(reader)?,
            signaled: reader.read_bool()?,
            waiters: read_thread_queue(reader)?,
        })),
        OBJECT_MUTEX => Ok(KernelObject::Mutex(KernelMutex {
            owner: read_optional_thread_id(reader)?,
            waiters: read_thread_queue(reader)?,
        })),
        OBJECT_SEMAPHORE => {
            let count = reader.read_u32()?;
            let maximum = reader.read_u32()?;
            if maximum == 0 || count > maximum {
                return Err(KernelStateError::InvalidData("invalid semaphore count"));
            }
            Ok(KernelObject::Semaphore(KernelSemaphore {
                count,
                maximum,
                waiters: read_thread_queue(reader)?,
            }))
        }
        OBJECT_TIMER => Ok(KernelObject::Timer(KernelTimer {
            reset_type: read_reset_type(reader)?,
            signaled: reader.read_bool()?,
            deadline_ns: read_optional_u64(reader)?,
            interval_ns: reader.read_u64()?,
            waiters: read_thread_queue(reader)?,
        })),
        OBJECT_MEMORY_BLOCK => {
            let slot = reader.read_u32()?;
            let offset = usize::try_from(reader.read_u64()?)
                .map_err(|_| KernelStateError::InvalidData("memory-block offset overflows"))?;
            let length = usize::try_from(reader.read_u64()?)
                .map_err(|_| KernelStateError::InvalidData("memory-block length overflows"))?;
            let owner_permissions = reader.read_u32()?;
            let other_permissions = reader.read_u32()?;
            if !valid_memory_permissions(owner_permissions)
                || !valid_memory_permissions(other_permissions)
                || length == 0
                || length & 0xFFF != 0
                || offset & 0xFFF != 0
            {
                return Err(KernelStateError::InvalidData("invalid memory-block metadata"));
            }
            let backing = catalog.view(slot, offset, length)?;
            Ok(KernelObject::MemoryBlock(KernelMemoryBlock {
                backing,
                owner_permissions,
                other_permissions,
            }))
        }
        OBJECT_RESOURCE_LIMIT => Ok(KernelObject::ResourceLimit),
        OBJECT_PORT => {
            let max_sessions = reader.read_u16()?;
            let active_sessions = reader.read_u16()?;
            if max_sessions == 0 || active_sessions > max_sessions {
                return Err(KernelStateError::InvalidData("invalid service-port session count"));
            }
            Ok(KernelObject::Port(KernelPort {
                max_sessions,
                active_sessions,
                pending_sessions: read_u32_queue(reader)?,
            }))
        }
        OBJECT_SESSION_ENDPOINT => {
            let peer_key = reader.read_u32()?;
            let port_key = reader.read_u32()?;
            let role = match reader.read_u8()? {
                0 => SessionRole::Client,
                1 => SessionRole::Server,
                _ => return Err(KernelStateError::InvalidData("invalid session endpoint role")),
            };
            Ok(KernelObject::SessionEndpoint(KernelSessionEndpoint {
                peer_key,
                port_key,
                role,
                remote_closed: reader.read_bool()?,
            }))
        }
        OBJECT_SERVICE_SESSION => Ok(KernelObject::ServiceSession(read_service_session(reader)?)),
        OBJECT_ADDRESS_ARBITER => Ok(KernelObject::AddressArbiter),
        _ => Err(KernelStateError::InvalidData("unknown kernel object type")),
    }
}

fn read_service_session(
    reader: &mut StateReader<'_>,
) -> Result<ServiceSessionKind, KernelStateError> {
    match reader.read_u8()? {
        0 => Ok(ServiceSessionKind::ServiceManager {
            client_registered: reader.read_bool()?,
        }),
        1 => Ok(ServiceSessionKind::GspGpu {
            client_id: read_optional_u8(reader)?,
            shared_memory_handle: read_optional_u32(reader)?,
            event_handle: read_optional_u32(reader)?,
            flags: reader.read_u32()?,
        }),
        2 => Ok(ServiceSessionKind::Hid),
        3 => Ok(ServiceSessionKind::Cfg),
        4 => Ok(ServiceSessionKind::Apt),
        5 => Ok(ServiceSessionKind::Fs),
        6 => Ok(ServiceSessionKind::Ptm),
        7 => Ok(ServiceSessionKind::Dsp),
        8 => Ok(ServiceSessionKind::Csnd),
        9 => Ok(ServiceSessionKind::FsRomFsFile),
        10 => Ok(ServiceSessionKind::FsHostFile),
        11 => Ok(ServiceSessionKind::FsDirectory),
        12 => Ok(ServiceSessionKind::Stub(reader.read_u8()?)),
        _ => Err(KernelStateError::InvalidData("unknown service-session kind")),
    }
}

fn validate_table(table: &Arm11KernelObjectTable) -> Result<(), KernelStateError> {
    if table.objects.len() > MAX_KERNEL_OBJECTS || table.retired_objects.len() > MAX_KERNEL_OBJECTS {
        return Err(KernelStateError::TooManyEntries {
            kind: "kernel objects",
            count: u32::try_from(table.objects.len() + table.retired_objects.len())
                .unwrap_or(u32::MAX),
        });
    }
    if table.handle_aliases.len() > MAX_KERNEL_HANDLES
        || table.waiter_handles.len() > MAX_KERNEL_WAITERS
        || table.wait_all_refs.len() > MAX_KERNEL_WAITERS
        || table.handoffs.len() > MAX_KERNEL_WAITERS
    {
        return Err(KernelStateError::InvalidData("kernel table entry limit exceeded"));
    }

    let mut all_keys = BTreeSet::new();
    for key in table.objects.keys().chain(table.retired_objects.keys()) {
        if *key < FIRST_HANDLE
            || *key == CURRENT_THREAD_HANDLE
            || *key == CURRENT_PROCESS_HANDLE
            || !all_keys.insert(*key)
        {
            return Err(KernelStateError::InvalidData("kernel object keys overlap or are reserved"));
        }
    }

    for (handle, object_key) in &table.handle_aliases {
        if *handle < FIRST_HANDLE
            || *handle == CURRENT_THREAD_HANDLE
            || *handle == CURRENT_PROCESS_HANDLE
            || all_keys.contains(handle)
            || !all_keys.contains(object_key)
        {
            return Err(KernelStateError::InvalidData("duplicate or dangling handle alias"));
        }
    }

    let open_handle_count = table.objects.len() + table.handle_aliases.len();
    if table.high_water_handle_count < open_handle_count
        || table.high_water_handle_count > MAX_KERNEL_HANDLES
    {
        return Err(KernelStateError::InvalidData("invalid handle high-water mark"));
    }
    let largest_handle = all_keys
        .iter()
        .chain(table.handle_aliases.keys())
        .copied()
        .max()
        .unwrap_or(FIRST_HANDLE - 1);
    if table.next_handle < FIRST_HANDLE
        || table.next_handle <= largest_handle
        || table.next_handle == CURRENT_THREAD_HANDLE
        || table.next_handle == CURRENT_PROCESS_HANDLE
    {
        return Err(KernelStateError::InvalidData("next handle would recycle an existing handle"));
    }

    for handle in table.service_registry.values() {
        if !matches!(
            object_by_key(table, *handle),
            Some(KernelObject::Port(_))
        ) {
            return Err(KernelStateError::InvalidData("service registry points to a non-port"));
        }
    }

    for ((thread, object_key), _) in &table.waiter_handles {
        validate_thread_id(*thread)?;
        if object_by_key(table, *object_key).is_none() {
            return Err(KernelStateError::InvalidData("waiter references a missing object"));
        }
    }

    for (thread, handles) in &table.wait_all_refs {
        validate_thread_id(*thread)?;
        if handles.len() > MAX_WAIT_ALL_HANDLES {
            return Err(KernelStateError::InvalidData("wait-all handle list is too large"));
        }
        for (_, object_key) in handles {
            if object_by_key(table, *object_key).is_none() {
                return Err(KernelStateError::InvalidData("wait-all references a missing object"));
            }
        }
    }

    for (thread, handle) in &table.handoffs {
        validate_thread_id(*thread)?;
        if *handle < FIRST_HANDLE || *handle == CURRENT_THREAD_HANDLE || *handle == CURRENT_PROCESS_HANDLE {
            return Err(KernelStateError::InvalidData("invalid handoff handle"));
        }
    }

    for (object_key, object) in table.objects.iter().chain(table.retired_objects.iter()) {
        match object {
            KernelObject::Thread(thread) => {
                validate_thread_id(thread.id)?;
                validate_queue(&thread.waiters)?;
            }
            KernelObject::Event(event) => validate_queue(&event.waiters)?,
            KernelObject::Mutex(mutex) => {
                if let Some(owner) = mutex.owner {
                    validate_thread_id(owner)?;
                }
                validate_queue(&mutex.waiters)?;
            }
            KernelObject::Semaphore(semaphore) => {
                if semaphore.maximum == 0 || semaphore.count > semaphore.maximum {
                    return Err(KernelStateError::InvalidData("invalid semaphore state"));
                }
                validate_queue(&semaphore.waiters)?;
            }
            KernelObject::Timer(timer) => validate_queue(&timer.waiters)?,
            KernelObject::MemoryBlock(block) => {
                if !valid_memory_permissions(block.owner_permissions)
                    || !valid_memory_permissions(block.other_permissions)
                    || block.backing.len().is_none_or(|length| {
                        length == 0
                            || length & 0xFFF != 0
                            || block.backing.root_base_offset() & 0xFFF != 0
                    })
                {
                    return Err(KernelStateError::InvalidData("invalid memory-block state"));
                }
            }
            KernelObject::Port(port) => {
                if port.max_sessions == 0
                    || port.active_sessions > port.max_sessions
                    || port.pending_sessions.len() > MAX_KERNEL_WAITERS
                {
                    return Err(KernelStateError::InvalidData("invalid port state"));
                }
            }
            KernelObject::SessionEndpoint(endpoint) => {
                if !matches!(object_by_key(table, endpoint.port_key), Some(KernelObject::Port(_))) {
                    return Err(KernelStateError::InvalidData("session endpoint refers to a missing port"));
                }
                if let Some(KernelObject::SessionEndpoint(peer)) = object_by_key(table, endpoint.peer_key) {
                    if peer.peer_key != *object_key
                        || peer.port_key != endpoint.port_key
                        || peer.role == endpoint.role
                    {
                        return Err(KernelStateError::InvalidData("session endpoint peer linkage is invalid"));
                    }
                } else {
                    return Err(KernelStateError::InvalidData("session endpoint peer is missing"));
                }
            }
            KernelObject::ResourceLimit
            | KernelObject::ServiceSession(_)
            | KernelObject::AddressArbiter => {}
        }
    }

    validate_port_links(table)?;
    Ok(())
}

fn validate_port_links(table: &Arm11KernelObjectTable) -> Result<(), KernelStateError> {
    for (port_key, object) in table.objects.iter().chain(table.retired_objects.iter()) {
        let KernelObject::Port(port) = object else {
            continue;
        };
        let mut pending = BTreeSet::new();
        for endpoint_key in &port.pending_sessions {
            if !pending.insert(*endpoint_key)
                || !matches!(
                    object_by_key(table, *endpoint_key),
                    Some(KernelObject::SessionEndpoint(KernelSessionEndpoint {
                        port_key: endpoint_port,
                        role: SessionRole::Server,
                        remote_closed: false,
                        ..
                    })) if endpoint_port == port_key
                )
            {
                return Err(KernelStateError::InvalidData("pending endpoint is not queued on its port"));
            }
        }
        let endpoint_count = table
            .objects
            .iter()
            .chain(table.retired_objects.iter())
            .filter(|(_, candidate)| {
                matches!(
                    candidate,
                    KernelObject::SessionEndpoint(endpoint)
                        if endpoint.port_key == *port_key && !endpoint.remote_closed
                )
            })
            .count();
        if endpoint_count / 2 != usize::from(port.active_sessions) {
            return Err(KernelStateError::InvalidData("port active-session count does not match endpoints"));
        }
    }
    Ok(())
}

fn object_by_key(table: &Arm11KernelObjectTable, key: u32) -> Option<&KernelObject> {
    table.objects.get(&key).or_else(|| table.retired_objects.get(&key))
}

fn validate_queue(queue: &VecDeque<ThreadId>) -> Result<(), KernelStateError> {
    if queue.len() > MAX_KERNEL_WAITERS {
        return Err(KernelStateError::InvalidData("kernel wait queue is too large"));
    }
    let mut seen = BTreeSet::new();
    for thread in queue {
        validate_thread_id(*thread)?;
        if !seen.insert(*thread) {
            return Err(KernelStateError::InvalidData("kernel wait queue contains a duplicate thread"));
        }
    }
    Ok(())
}

fn valid_memory_permissions(value: u32) -> bool {
    matches!(value, 0 | 1 | 3)
}

fn validate_thread_id(thread: ThreadId) -> Result<(), KernelStateError> {
    if thread.raw() == 0 {
        Err(KernelStateError::InvalidData("thread ID zero is reserved"))
    } else {
        Ok(())
    }
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
    writer.write_u32(to_u32(count, kind)?).map_err(Into::into)
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

fn to_u32(count: usize, kind: &'static str) -> Result<u32, KernelStateError> {
    u32::try_from(count).map_err(|_| KernelStateError::InvalidData(kind))
}

fn write_thread_queue(
    writer: &mut StateWriter,
    queue: &VecDeque<ThreadId>,
) -> Result<(), KernelStateError> {
    write_count(writer, queue.len(), MAX_KERNEL_WAITERS, "wait queue")?;
    for thread in queue {
        write_thread_id(writer, *thread)?;
    }
    Ok(())
}

fn read_thread_queue(reader: &mut StateReader<'_>) -> Result<VecDeque<ThreadId>, KernelStateError> {
    let count = read_count(reader, MAX_KERNEL_WAITERS, "wait queue")?;
    let mut queue = VecDeque::new();
    queue.try_reserve(count).map_err(|_| KernelStateError::InvalidData("could not allocate wait queue"))?;
    for _ in 0..count {
        queue.push_back(read_thread_id(reader)?);
    }
    validate_queue(&queue)?;
    Ok(queue)
}

fn write_u32_queue(
    writer: &mut StateWriter,
    queue: &VecDeque<u32>,
) -> Result<(), KernelStateError> {
    write_count(writer, queue.len(), MAX_KERNEL_WAITERS, "handle queue")?;
    for handle in queue {
        writer.write_u32(*handle)?;
    }
    Ok(())
}

fn read_u32_queue(reader: &mut StateReader<'_>) -> Result<VecDeque<u32>, KernelStateError> {
    let count = read_count(reader, MAX_KERNEL_WAITERS, "handle queue")?;
    let mut queue = VecDeque::new();
    queue.try_reserve(count).map_err(|_| KernelStateError::InvalidData("could not allocate handle queue"))?;
    for _ in 0..count {
        queue.push_back(reader.read_u32()?);
    }
    Ok(queue)
}

fn write_thread_id(writer: &mut StateWriter, thread: ThreadId) -> Result<(), KernelStateError> {
    validate_thread_id(thread)?;
    writer.write_u32(thread.raw())?;
    Ok(())
}

fn read_thread_id(reader: &mut StateReader<'_>) -> Result<ThreadId, KernelStateError> {
    let thread = ThreadId::from_raw(reader.read_u32()?);
    validate_thread_id(thread)?;
    Ok(thread)
}

fn write_optional_thread_id(
    writer: &mut StateWriter,
    thread: Option<ThreadId>,
) -> Result<(), KernelStateError> {
    writer.write_bool(thread.is_some())?;
    if let Some(thread) = thread {
        write_thread_id(writer, thread)?;
    }
    Ok(())
}

fn read_optional_thread_id(
    reader: &mut StateReader<'_>,
) -> Result<Option<ThreadId>, KernelStateError> {
    if reader.read_bool()? {
        Ok(Some(read_thread_id(reader)?))
    } else {
        Ok(None)
    }
}

fn write_reset_type(
    writer: &mut StateWriter,
    reset_type: EventResetType,
) -> Result<(), KernelStateError> {
    writer.write_u8(match reset_type {
        EventResetType::OneShot => 0,
        EventResetType::Sticky => 1,
    })?;
    Ok(())
}

fn read_reset_type(reader: &mut StateReader<'_>) -> Result<EventResetType, KernelStateError> {
    match reader.read_u8()? {
        0 => Ok(EventResetType::OneShot),
        1 => Ok(EventResetType::Sticky),
        _ => Err(KernelStateError::InvalidData("invalid event reset type")),
    }
}

fn write_optional_u64(
    writer: &mut StateWriter,
    value: Option<u64>,
) -> Result<(), KernelStateError> {
    writer.write_bool(value.is_some())?;
    if let Some(value) = value {
        writer.write_u64(value)?;
    }
    Ok(())
}

fn read_optional_u64(reader: &mut StateReader<'_>) -> Result<Option<u64>, KernelStateError> {
    if reader.read_bool()? {
        Ok(Some(reader.read_u64()?))
    } else {
        Ok(None)
    }
}

fn write_optional_u32(
    writer: &mut StateWriter,
    value: Option<u32>,
) -> Result<(), KernelStateError> {
    writer.write_bool(value.is_some())?;
    if let Some(value) = value {
        writer.write_u32(value)?;
    }
    Ok(())
}

fn read_optional_u32(reader: &mut StateReader<'_>) -> Result<Option<u32>, KernelStateError> {
    if reader.read_bool()? {
        Ok(Some(reader.read_u32()?))
    } else {
        Ok(None)
    }
}

fn write_optional_u8(
    writer: &mut StateWriter,
    value: Option<u8>,
) -> Result<(), KernelStateError> {
    writer.write_bool(value.is_some())?;
    if let Some(value) = value {
        writer.write_u8(value)?;
    }
    Ok(())
}

fn read_optional_u8(reader: &mut StateReader<'_>) -> Result<Option<u8>, KernelStateError> {
    if reader.read_bool()? {
        Ok(Some(reader.read_u8()?))
    } else {
        Ok(None)
    }
}
