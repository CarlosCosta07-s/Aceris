//! Memory bus abstraction.

use aceris_common::{Error, Result};

/// A little-endian byte-addressable bus.
///
/// Implementors only need to provide byte access; wider accesses have
/// (slow but correct) default implementations that can be overridden.
/// Granularity used by the local exclusive monitor in the built-in memory buses.
pub const EXCLUSIVE_RESERVATION_GRANULE: u32 = 8;

pub trait Bus {
    fn read8(&mut self, addr: u32) -> Result<u8>;
    fn write8(&mut self, addr: u32, value: u8) -> Result<()>;

    /// Reads an exclusive value and returns an opaque monitor-generation token.
    fn exclusive_read(&mut self, addr: u32, size: usize) -> Result<(u64, u64)> {
        let _ = (addr, size);
        Err(Error::Unimplemented(
            "exclusive reads are not supported by this bus",
        ))
    }

    /// Attempts an exclusive write; None means the CPU has no matching reservation.
    fn exclusive_write(
        &mut self,
        addr: u32,
        size: usize,
        value: u64,
        reservation: Option<u64>,
    ) -> Result<bool> {
        let _ = (addr, size, value, reservation);
        Err(Error::Unimplemented(
            "exclusive writes are not supported by this bus",
        ))
    }
    fn read16(&mut self, addr: u32) -> Result<u16> {
        let lo = self.read8(addr)?;
        let hi = self.read8(addr.wrapping_add(1))?;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    fn read32(&mut self, addr: u32) -> Result<u32> {
        let mut bytes = [0u8; 4];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = self.read8(addr.wrapping_add(i as u32))?;
        }
        Ok(u32::from_le_bytes(bytes))
    }


    /// Reads a byte while applying User-mode memory permissions.
    /// Buses without privilege-aware translation may use the ordinary access.
    fn read8_unprivileged(&mut self, addr: u32) -> Result<u8> {
        self.read8(addr)
    }

    /// Writes a byte while applying User-mode memory permissions.
    fn write8_unprivileged(&mut self, addr: u32, value: u8) -> Result<()> {
        self.write8(addr, value)
    }

    /// Reads a word while applying User-mode memory permissions.
    fn read32_unprivileged(&mut self, addr: u32) -> Result<u32> {
        self.read32(addr)
    }

    /// Writes a word while applying User-mode memory permissions.
    fn write32_unprivileged(&mut self, addr: u32, value: u32) -> Result<()> {
        self.write32(addr, value)
    }

    fn write16(&mut self, addr: u32, value: u16) -> Result<()> {
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(addr.wrapping_add(i as u32), byte)?;
        }
        Ok(())
    }

    fn write32(&mut self, addr: u32, value: u32) -> Result<()> {
        for (i, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.write8(addr.wrapping_add(i as u32), byte)?;
        }
        Ok(())
    }
}

/// A single contiguous block of RAM mapped at `base`.
///
/// Mostly useful for tests and early bring-up, before a real memory map exists.
#[derive(Clone, Debug)]
pub struct FlatMemory {
    base: u32,
    data: Vec<u8>,
    exclusive_versions: Vec<u64>,
}

impl FlatMemory {
    pub fn new(base: u32, size: usize) -> Self {
        let first_granule = u64::from(base) / u64::from(EXCLUSIVE_RESERVATION_GRANULE);
        let granule_count = if size == 0 {
            0
        } else {
            let last_address = u64::from(base) + size as u64 - 1;
            usize::try_from(
                last_address / u64::from(EXCLUSIVE_RESERVATION_GRANULE) - first_granule + 1,
            )
            .expect("exclusive monitor table fits host address space")
        };
        Self {
            base,
            data: vec![0; size],
            exclusive_versions: vec![0; granule_count],
        }
    }

    fn index(&self, addr: u32) -> Result<usize> {
        let offset = addr
            .checked_sub(self.base)
            .ok_or(Error::InvalidAccess { addr })? as usize;
        if offset < self.data.len() {
            Ok(offset)
        } else {
            Err(Error::InvalidAccess { addr })
        }
    }

    fn exclusive_version_index(&self, addr: u32) -> Result<usize> {
        self.index(addr)?;
        let base_granule = u64::from(self.base) / u64::from(EXCLUSIVE_RESERVATION_GRANULE);
        let address_granule = u64::from(addr) / u64::from(EXCLUSIVE_RESERVATION_GRANULE);
        usize::try_from(address_granule - base_granule).map_err(|_| Error::InvalidAccess { addr })
    }

    fn bump_exclusive_versions(&mut self, addr: u32, size: usize) -> Result<()> {
        if size == 0 {
            return Ok(());
        }
        let end = u64::from(addr) + size as u64;
        if end > u64::from(u32::MAX) + 1 {
            return Err(Error::InvalidAccess { addr });
        }
        let last_address = u32::try_from(end - 1).map_err(|_| Error::InvalidAccess { addr })?;
        let first = self.exclusive_version_index(addr)?;
        let last = self.exclusive_version_index(last_address)?;
        for index in first..=last {
            let version = self
                .exclusive_versions
                .get_mut(index)
                .ok_or(Error::InvalidAccess { addr })?;
            *version = version.wrapping_add(1);
        }
        Ok(())
    }

    fn validate_exclusive_access(addr: u32, size: usize) -> Result<()> {
        if !matches!(size, 1 | 2 | 4 | 8) || addr as usize % size != 0 {
            return Err(Error::InvalidAccess { addr });
        }
        Ok(())
    }
}

impl Bus for FlatMemory {
    fn read8(&mut self, addr: u32) -> Result<u8> {
        Ok(self.data[self.index(addr)?])
    }

    fn write8(&mut self, addr: u32, value: u8) -> Result<()> {
        let index = self.index(addr)?;
        self.data[index] = value;
        self.bump_exclusive_versions(addr, 1)
    }

    fn exclusive_read(&mut self, addr: u32, size: usize) -> Result<(u64, u64)> {
        Self::validate_exclusive_access(addr, size)?;
        let start = self.index(addr)?;
        let end = start
            .checked_add(size)
            .ok_or(Error::InvalidAccess { addr })?;
        if end > self.data.len() {
            return Err(Error::InvalidAccess { addr });
        }
        let mut value = 0u64;
        for (index, byte) in self.data[start..end].iter().copied().enumerate() {
            value |= u64::from(byte) << (index * 8);
        }
        let version_index = self.exclusive_version_index(addr)?;
        let version = *self
            .exclusive_versions
            .get(version_index)
            .ok_or(Error::InvalidAccess { addr })?;
        Ok((value, version))
    }

    fn exclusive_write(
        &mut self,
        addr: u32,
        size: usize,
        value: u64,
        reservation: Option<u64>,
    ) -> Result<bool> {
        Self::validate_exclusive_access(addr, size)?;
        let start = self.index(addr)?;
        let end = start
            .checked_add(size)
            .ok_or(Error::InvalidAccess { addr })?;
        if end > self.data.len() {
            return Err(Error::InvalidAccess { addr });
        }
        let version_index = self.exclusive_version_index(addr)?;
        let current_version = *self
            .exclusive_versions
            .get(version_index)
            .ok_or(Error::InvalidAccess { addr })?;
        if reservation != Some(current_version) {
            return Ok(false);
        }
        self.data[start..end].copy_from_slice(&value.to_le_bytes()[..size]);
        self.bump_exclusive_versions(addr, size)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accesses_are_little_endian() {
        let mut mem = FlatMemory::new(0x1000, 16);
        mem.write32(0x1000, 0xDEAD_BEEF).unwrap();
        assert_eq!(mem.read8(0x1000).unwrap(), 0xEF);
        assert_eq!(mem.read16(0x1002).unwrap(), 0xDEAD);
        assert_eq!(mem.read32(0x1000).unwrap(), 0xDEAD_BEEF);
    }

    #[test]
    fn exclusive_write_succeeds_once_and_consumes_its_reservation() {
        let mut memory = FlatMemory::new(0x1000, 32);
        memory.write32(0x1000, 0x1122_3344).unwrap();

        let (loaded, reservation) = memory.exclusive_read(0x1000, 4).unwrap();
        assert_eq!(loaded, 0x1122_3344);
        assert!(
            memory
                .exclusive_write(0x1000, 4, 0xAABB_CCDD, Some(reservation))
                .unwrap()
        );
        assert!(
            !memory
                .exclusive_write(0x1000, 4, 0x5566_7788, Some(reservation))
                .unwrap()
        );
        assert_eq!(memory.read32(0x1000).unwrap(), 0xAABB_CCDD);
    }

    #[test]
    fn any_store_in_the_eight_byte_reservation_granule_invalidates_the_token() {
        let mut memory = FlatMemory::new(0x1000, 32);
        let (_, reservation) = memory.exclusive_read(0x1000, 4).unwrap();
        memory.write8(0x1007, 0x55).unwrap();
        assert!(
            !memory
                .exclusive_write(0x1000, 4, 0xDEAD_BEEF, Some(reservation))
                .unwrap()
        );
    }

    #[test]
    fn exclusive_access_rejects_invalid_sizes_and_alignment() {
        let mut memory = FlatMemory::new(0x1000, 32);
        assert!(memory.exclusive_read(0x1001, 4).is_err());
        assert!(memory.exclusive_read(0x1000, 3).is_err());
        assert!(memory.exclusive_write(0x1000, 3, 0, None).is_err());
        assert!(memory.exclusive_write(0x1001, 4, 0, None).is_err());
        assert!(!memory.exclusive_write(0x1000, 8, 0, None).unwrap());
    }

    #[test]
    fn out_of_range_accesses_fail() {
        let mut mem = FlatMemory::new(0x1000, 16);
        assert!(matches!(
            mem.read8(0x0FFF),
            Err(Error::InvalidAccess { addr: 0x0FFF })
        ));
        assert!(mem.read8(0x1010).is_err());
        assert!(mem.write32(0x100E, 0).is_err());
    }
}
