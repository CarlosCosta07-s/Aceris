//! Original PICA200 command processor foundation.
//!
//! This module models the documented command-list register-write protocol.
//! Vertex rasterization, shader execution and undocumented register side
//! effects are deliberately not represented as implemented.

use std::fmt;

/// Number of 32-bit internal PICA200 registers addressable by command ID.
pub const REGISTER_COUNT: usize = 0x400;
/// First documented framebuffer-state register.
pub const REG_COLOR_OPERATION: u16 = 0x100;
/// First documented geometry-state register.
pub const REG_ATTRIBUTE_BUFFERS: u16 = 0x200;
/// First command-buffer size register.
pub const REG_COMMAND_BUFFER_SIZE_0: u16 = 0x238;
/// First command-buffer address register.
pub const REG_COMMAND_BUFFER_ADDRESS_0: u16 = 0x23A;
/// First command-buffer trigger register.
pub const REG_COMMAND_BUFFER_JUMP_0: u16 = 0x23C;

/// PICA200 command-list parsing or register-write failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaCommandError {
    IncompleteCommandHeader {
        word_offset: usize,
    },
    TruncatedParameters {
        word_offset: usize,
        required_words: usize,
        remaining_words: usize,
    },
    InvalidRegister {
        register: u32,
        parameter_index: usize,
    },
}

impl fmt::Display for PicaCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteCommandHeader { word_offset } => {
                write!(
                    formatter,
                    "PICA200 command at word {word_offset} has no header"
                )
            }
            Self::TruncatedParameters {
                word_offset,
                required_words,
                remaining_words,
            } => write!(
                formatter,
                "PICA200 command at word {word_offset} needs {required_words} words, only {remaining_words} remain"
            ),
            Self::InvalidRegister {
                register,
                parameter_index,
            } => write!(
                formatter,
                "PICA200 command parameter {parameter_index} targets invalid register {register:#x}"
            ),
        }
    }
}

impl std::error::Error for PicaCommandError {}

/// Counters returned after parsing one command list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicaCommandListReport {
    pub commands: u64,
    pub register_writes: u64,
}

/// PICA200 internal register file and command-list interpreter.
#[derive(Clone, Debug)]
pub struct Pica200 {
    registers: [u32; REGISTER_COUNT],
    command_lists_submitted: u64,
    register_writes: u64,
}

impl Default for Pica200 {
    fn default() -> Self {
        Self {
            registers: [0; REGISTER_COUNT],
            command_lists_submitted: 0,
            register_writes: 0,
        }
    }
}

impl Pica200 {
    /// Creates a reset PICA200 command processor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads an internal register, or returns None for an invalid ID.
    pub fn register(&self, id: u16) -> Option<u32> {
        self.registers.get(usize::from(id)).copied()
    }

    /// Writes one complete internal register.
    pub fn write_register(&mut self, id: u16, value: u32) -> Result<(), PicaCommandError> {
        let register =
            self.registers
                .get_mut(usize::from(id))
                .ok_or(PicaCommandError::InvalidRegister {
                    register: u32::from(id),
                    parameter_index: 0,
                })?;
        *register = value;
        self.register_writes = self.register_writes.saturating_add(1);
        Ok(())
    }

    /// Parses and executes a PICA200 command list from little-endian words.
    ///
    /// A command is an initial parameter, a header, then the extra parameters
    /// counted in header bits 20..27. Bits 0..15 select the register,
    /// bits 16..19 select byte lanes, and bit 31 enables consecutive writes.
    pub fn execute_command_list(
        &mut self,
        words: &[u32],
    ) -> Result<PicaCommandListReport, PicaCommandError> {
        let mut report = PicaCommandListReport::default();
        let mut cursor = 0usize;

        while cursor < words.len() {
            let command_offset = cursor;
            if words.len() - cursor < 2 {
                return Err(PicaCommandError::IncompleteCommandHeader {
                    word_offset: command_offset,
                });
            }

            let first_parameter = words[cursor];
            let header = words[cursor + 1];
            let register_id = header & 0xFFFF;
            let byte_mask = ((header >> 16) & 0xF) as u8;
            let extra_parameters = ((header >> 20) & 0xFF) as usize;
            let consecutive = header & (1 << 31) != 0;

            let parameter_count = extra_parameters + 1;
            let command_words = 2usize.saturating_add(extra_parameters);
            let remaining_words = words.len() - cursor;
            if remaining_words < command_words {
                return Err(PicaCommandError::TruncatedParameters {
                    word_offset: command_offset,
                    required_words: command_words,
                    remaining_words,
                });
            }

            let final_register = if consecutive {
                register_id.saturating_add(extra_parameters as u32)
            } else {
                register_id
            };
            if register_id >= REGISTER_COUNT as u32 || final_register >= REGISTER_COUNT as u32 {
                return Err(PicaCommandError::InvalidRegister {
                    register: final_register,
                    parameter_index: extra_parameters,
                });
            }

            for parameter_index in 0..parameter_count {
                let register = if consecutive {
                    register_id + parameter_index as u32
                } else {
                    register_id
                };
                let value = if parameter_index == 0 {
                    first_parameter
                } else {
                    words[cursor + 1 + parameter_index]
                };
                apply_byte_mask(&mut self.registers[register as usize], value, byte_mask);
                report.register_writes = report.register_writes.saturating_add(1);
            }

            report.commands = report.commands.saturating_add(1);
            cursor += command_words;
        }

        self.command_lists_submitted = self.command_lists_submitted.saturating_add(1);
        self.register_writes = self.register_writes.saturating_add(report.register_writes);
        Ok(report)
    }

    pub const fn command_lists_submitted(&self) -> u64 {
        self.command_lists_submitted
    }

    pub const fn register_writes(&self) -> u64 {
        self.register_writes
    }

    pub fn reset(&mut self) {
        self.registers.fill(0);
        self.command_lists_submitted = 0;
        self.register_writes = 0;
    }
}

fn apply_byte_mask(target: &mut u32, value: u32, mask: u8) {
    let mut byte_mask = 0u32;
    for lane in 0..4 {
        if mask & (1 << lane) != 0 {
            byte_mask |= 0xFF << (lane * 8);
        }
    }
    *target = (*target & !byte_mask) | (value & byte_mask);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(register: u16, mask: u8, consecutive: bool, extra: u8) -> u32 {
        u32::from(register)
            | (u32::from(mask & 0xF) << 16)
            | (u32::from(extra) << 20)
            | (u32::from(consecutive) << 31)
    }

    #[test]
    fn command_list_writes_one_register_with_a_full_mask() {
        let mut gpu = Pica200::new();
        let report = gpu
            .execute_command_list(&[0xDEAD_BEEF, command(0x100, 0xF, false, 0)])
            .unwrap();
        assert_eq!(report.commands, 1);
        assert_eq!(report.register_writes, 1);
        assert_eq!(gpu.register(0x100), Some(0xDEAD_BEEF));
        assert_eq!(gpu.command_lists_submitted(), 1);
    }

    #[test]
    fn command_list_applies_byte_mask_without_changing_other_lanes() {
        let mut gpu = Pica200::new();
        gpu.write_register(0x100, 0x1122_3344).unwrap();
        gpu.execute_command_list(&[0xAABB_CCDD, command(0x100, 0b0101, false, 0)])
            .unwrap();
        assert_eq!(gpu.register(0x100), Some(0x11BB_33DD));
    }

    #[test]
    fn command_list_can_write_consecutive_registers() {
        let mut gpu = Pica200::new();
        let report = gpu
            .execute_command_list(&[0x10, command(0x200, 0xF, true, 2), 0x20, 0x30])
            .unwrap();
        assert_eq!(report.commands, 1);
        assert_eq!(report.register_writes, 3);
        assert_eq!(gpu.register(0x200), Some(0x10));
        assert_eq!(gpu.register(0x201), Some(0x20));
        assert_eq!(gpu.register(0x202), Some(0x30));
    }

    #[test]
    fn command_list_reports_truncated_extra_parameters() {
        let mut gpu = Pica200::new();
        let error = gpu
            .execute_command_list(&[0x10, command(0x200, 0xF, true, 2), 0x20])
            .unwrap_err();
        assert_eq!(
            error,
            PicaCommandError::TruncatedParameters {
                word_offset: 0,
                required_words: 4,
                remaining_words: 3,
            }
        );
        assert_eq!(gpu.command_lists_submitted(), 0);
    }

    #[test]
    fn command_list_rejects_register_overflow_before_any_write() {
        let mut gpu = Pica200::new();
        let error = gpu
            .execute_command_list(&[0x10, command(0x3FF, 0xF, true, 1), 0x20])
            .unwrap_err();
        assert!(matches!(error, PicaCommandError::InvalidRegister { .. }));
        assert_eq!(gpu.register(0x3FF), Some(0));
        assert_eq!(gpu.command_lists_submitted(), 0);
    }

    #[test]
    fn empty_command_list_is_a_successful_noop() {
        let mut gpu = Pica200::new();
        assert_eq!(
            gpu.execute_command_list(&[]).unwrap(),
            PicaCommandListReport::default()
        );
        assert_eq!(gpu.command_lists_submitted(), 1);
    }
}
