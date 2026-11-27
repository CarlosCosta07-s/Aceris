//! Small checked binary codec used by CPU save-state sections.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuStateError {
    AllocationFailed,
    Truncated,
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidBoolean(u8),
    InvalidProcessorMode(u32),
    InvalidReservationSize(u8),
    InvalidTlbEntry,
    InvalidTlbVictim(u8),
    InvalidCp15CoreId(u32),
    InvalidTtbcr(u32),
    InvalidFpscr(u32),
    InvalidFpexc(u32),
    InvalidThreadMetadata,
    InvalidThreadState(u8),
    TooManyThreads(u32),
    TrailingData,
}

impl fmt::Display for CpuStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationFailed => f.write_str("could not allocate ARM11 save-state buffer"),
            Self::Truncated => f.write_str("ARM11 save-state data is truncated"),
            Self::InvalidMagic => f.write_str("ARM11 save-state signature is invalid"),
            Self::UnsupportedVersion(version) => {
                write!(f, "ARM11 save-state version {version} is unsupported")
            }
            Self::InvalidBoolean(value) => {
                write!(f, "ARM11 save-state boolean value {value} is invalid")
            }
            Self::InvalidProcessorMode(mode) => {
                write!(f, "ARM11 save-state processor mode {mode:#04X} is invalid")
            }
            Self::InvalidReservationSize(size) => {
                write!(f, "ARM11 exclusive reservation size {size} is invalid")
            }
            Self::InvalidTlbEntry => f.write_str("ARM11 save-state contains an invalid TLB entry"),
            Self::InvalidTlbVictim(way) => {
                write!(f, "ARM11 save-state TLB victim way {way} is invalid")
            }
            Self::InvalidCp15CoreId(id) => {
                write!(f, "ARM11 CP15 core ID {id} is invalid")
            }
            Self::InvalidTtbcr(value) => {
                write!(f, "ARM11 CP15 TTBCR value {value:#010X} is invalid")
            }
            Self::InvalidFpscr(value) => {
                write!(
                    f,
                    "ARM11 VFP FPSCR value {value:#010X} has reserved bits set"
                )
            }
            Self::InvalidFpexc(value) => {
                write!(
                    f,
                    "ARM11 VFP FPEXC value {value:#010X} has reserved bits set"
                )
            }
            Self::InvalidThreadMetadata => {
                f.write_str("ARM11 save-state thread scheduler metadata is inconsistent")
            }
            Self::InvalidThreadState(state) => {
                write!(f, "ARM11 save-state thread state {state} is invalid")
            }
            Self::TooManyThreads(count) => {
                write!(f, "ARM11 save-state contains too many threads: {count}")
            }
            Self::TrailingData => f.write_str("ARM11 save-state contains trailing bytes"),
        }
    }
}

impl std::error::Error for CpuStateError {}

pub(crate) struct StateWriter {
    bytes: Vec<u8>,
}

impl StateWriter {
    pub(crate) fn with_capacity(capacity: usize) -> Result<Self, CpuStateError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| CpuStateError::AllocationFailed)?;
        Ok(Self { bytes })
    }

    pub(crate) fn write_bytes(&mut self, value: &[u8]) -> Result<(), CpuStateError> {
        self.bytes
            .try_reserve(value.len())
            .map_err(|_| CpuStateError::AllocationFailed)?;
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    pub(crate) fn write_u8(&mut self, value: u8) -> Result<(), CpuStateError> {
        self.write_bytes(&[value])
    }

    pub(crate) fn write_bool(&mut self, value: bool) -> Result<(), CpuStateError> {
        self.write_u8(u8::from(value))
    }

    pub(crate) fn write_u16(&mut self, value: u16) -> Result<(), CpuStateError> {
        self.write_bytes(&value.to_le_bytes())
    }

    pub(crate) fn write_u32(&mut self, value: u32) -> Result<(), CpuStateError> {
        self.write_bytes(&value.to_le_bytes())
    }

    pub(crate) fn write_u64(&mut self, value: u64) -> Result<(), CpuStateError> {
        self.write_bytes(&value.to_le_bytes())
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

pub(crate) struct StateReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> StateReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub(crate) fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], CpuStateError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(CpuStateError::Truncated)?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or(CpuStateError::Truncated)?;
        self.cursor = end;
        Ok(bytes)
    }

    pub(crate) fn read_u8(&mut self) -> Result<u8, CpuStateError> {
        Ok(self.read_bytes(1)?[0])
    }

    pub(crate) fn read_bool(&mut self) -> Result<bool, CpuStateError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(CpuStateError::InvalidBoolean(value)),
        }
    }

    pub(crate) fn read_u16(&mut self) -> Result<u16, CpuStateError> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn read_u32(&mut self) -> Result<u32, CpuStateError> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn read_u64(&mut self) -> Result<u64, CpuStateError> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub(crate) fn finish(self) -> Result<(), CpuStateError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(CpuStateError::TrailingData)
        }
    }
}
