//! Memory bus abstraction.

use aceris_common::{Error, Result};

/// A little-endian byte-addressable bus.
///
/// Implementors only need to provide byte access; wider accesses have
/// (slow but correct) default implementations that can be overridden.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> Result<u8>;
    fn write8(&mut self, addr: u32, value: u8) -> Result<()>;

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
}

impl FlatMemory {
    pub fn new(base: u32, size: usize) -> Self {
        Self {
            base,
            data: vec![0; size],
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
}

impl Bus for FlatMemory {
    fn read8(&mut self, addr: u32) -> Result<u8> {
        Ok(self.data[self.index(addr)?])
    }

    fn write8(&mut self, addr: u32, value: u8) -> Result<()> {
        let index = self.index(addr)?;
        self.data[index] = value;
        Ok(())
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
