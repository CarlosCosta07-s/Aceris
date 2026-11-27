//! Small, host-independent intermediate representation for ARM11 fast blocks.
//!
//! The first lowering tier removes the guest A32 encoding from common ALU and
//! branch operations. Less common operations retain an explicit compatibility
//! node until their semantics are represented directly in this IR.

use super::decode_cache::{Entry, FastOp};
use super::shifter::shift_imm;
use super::{Arm11Core, Condition, LR, StepEvent};
use crate::memory::Bus;
use aceris_common::Result;

/// One conditionally executed guest instruction in the intermediate stream.
#[derive(Clone, Copy, Debug)]
pub(super) struct IrInstruction {
    pub(super) condition: Condition,
    /// Original encoding retained only for architectural fault diagnostics.
    /// Native IR execution never decodes this word again.
    pub(super) diagnostic_word: u32,
    pub(super) operation: IrOperation,
}

impl IrInstruction {
    pub(super) const EMPTY: Self = Self {
        condition: Condition::Unconditional,
        diagnostic_word: u32::MAX,
        operation: IrOperation::A32Fallback(Entry {
            word: u32::MAX,
            condition: 0xF,
            op: FastOp::Generic,
        }),
    };

    /// Lowers a decoded A32 instruction into a host-independent operation.
    ///
    /// Entry is used only during lowering. Common ALU and branch operations
    /// no longer retain the original guest instruction word in the IR.
    pub(super) fn lower(entry: Entry) -> Self {
        let operation = match entry.op {
            FastOp::DpImm {
                opcode,
                set_flags: false,
                rd,
                imm,
                ..
            } if opcode == 0xD || opcode == 0xF => IrOperation::LoadImmediate {
                rd,
                value: if opcode == 0xD { imm } else { !imm },
            },
            FastOp::DpImm {
                opcode,
                set_flags,
                rn,
                rd,
                imm,
                imm_carry,
            } => IrOperation::AluImmediate {
                opcode,
                set_flags,
                rn,
                rd,
                immediate: imm,
                shifter_carry: imm_carry,
            },
            FastOp::DpReg {
                opcode,
                set_flags,
                rn,
                rd,
                rm,
                kind,
                amount,
            } => IrOperation::AluRegisterShiftImmediate {
                opcode,
                set_flags,
                rn,
                rd,
                rm,
                shift_kind: kind,
                shift_amount: amount,
            },
            FastOp::TransferImm {
                load,
                byte,
                pre_index,
                writeback,
                rn,
                rd,
                offset,
            } => IrOperation::TransferImmediate {
                load,
                byte,
                pre_index,
                writeback,
                rn,
                rd,
                offset,
            },
            FastOp::HalfwordTransfer {
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                offset,
                transfer_kind,
            } => IrOperation::HalfwordTransfer {
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                offset,
                transfer_kind,
            },
            FastOp::HalfwordTransferReg {
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                rm,
                transfer_kind,
            } => IrOperation::HalfwordTransferRegister {
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                rm,
                transfer_kind,
            },
            FastOp::BlockTransfer {
                pre_index,
                up,
                writeback,
                load,
                s_bit,
                rn,
                list,
            } => IrOperation::BlockTransfer {
                pre_index,
                up,
                writeback,
                load,
                s_bit,
                rn,
                list,
            },
            FastOp::Multiply {
                rd,
                rn,
                rs,
                rm,
                accumulate,
                set_flags,
            } => IrOperation::Multiply {
                rd,
                rn,
                rs,
                rm,
                accumulate,
                set_flags,
            },
            FastOp::MultiplyLong {
                rd_hi,
                rd_lo,
                rs,
                rm,
                signed,
                accumulate,
                set_flags,
            } => IrOperation::MultiplyLong {
                rd_hi,
                rd_lo,
                rs,
                rm,
                signed,
                accumulate,
                set_flags,
            },
            FastOp::HalfwordMultiply {
                class,
                rd,
                rn,
                rs,
                rm,
                x,
                y,
            } => IrOperation::HalfwordMultiply {
                class,
                rd,
                rn,
                rs,
                rm,
                x,
                y,
            },
            FastOp::DspMultiply {
                class,
                rd,
                ra,
                rs,
                rm,
                bit7,
                subtract,
                exchange_or_round,
            } => IrOperation::DspMultiply {
                class,
                rd,
                ra,
                rs,
                rm,
                bit7,
                subtract,
                exchange_or_round,
            },
            FastOp::Branch => {
                let displacement = (((entry.word & 0x00FF_FFFF) << 8) as i32) >> 6;
                IrOperation::Branch {
                    displacement,
                    link: entry.word & (1 << 24) != 0,
                }
            }
            FastOp::Generic => IrOperation::A32Fallback(entry),
        };
        Self {
            condition: Condition::from_bits(entry.condition),
            diagnostic_word: entry.word,
            operation,
        }
    }
}

/// Operations in the first executable tier of the Aceris ARM11 IR.
#[derive(Clone, Copy, Debug)]
pub(super) enum IrOperation {
    /// Direct register load after folding a MOV/MVN immediate with no flag update.
    LoadImmediate { rd: u8, value: u32 },
    /// ALU operation with a fully decoded immediate operand.
    AluImmediate {
        opcode: u8,
        set_flags: bool,
        rn: u8,
        rd: u8,
        immediate: u32,
        /// None means preserve the incoming carry flag.
        shifter_carry: Option<bool>,
    },
    /// ALU operation with an immediate-shifted register operand.
    AluRegisterShiftImmediate {
        opcode: u8,
        set_flags: bool,
        rn: u8,
        rd: u8,
        rm: u8,
        shift_kind: super::shifter::ShiftKind,
        shift_amount: u8,
    },
    /// LDR/STR/LDRB/STRB with an already decoded immediate offset.
    TransferImmediate {
        load: bool,
        byte: bool,
        pre_index: bool,
        writeback: bool,
        rn: u8,
        rd: u8,
        offset: i32,
    },
    /// LDRH/STRH/LDRSB/LDRSH with an immediate offset.
    HalfwordTransfer {
        pre_index: bool,
        up: bool,
        writeback: bool,
        load: bool,
        rn: u8,
        rd: u8,
        offset: u8,
        transfer_kind: u8,
    },
    /// LDRH/STRH/LDRSB/LDRSH with the offset read from Rm at execution.
    HalfwordTransferRegister {
        pre_index: bool,
        up: bool,
        writeback: bool,
        load: bool,
        rn: u8,
        rd: u8,
        rm: u8,
        transfer_kind: u8,
    },
    /// LDM/STM with a predecoded register mask.
    BlockTransfer {
        pre_index: bool,
        up: bool,
        writeback: bool,
        load: bool,
        s_bit: bool,
        rn: u8,
        list: u16,
    },
    /// MUL/MLA.
    Multiply {
        rd: u8,
        rn: u8,
        rs: u8,
        rm: u8,
        accumulate: bool,
        set_flags: bool,
    },
    /// UMULL/UMLAL/SMULL/SMLAL.
    MultiplyLong {
        rd_hi: u8,
        rd_lo: u8,
        rs: u8,
        rm: u8,
        signed: bool,
        accumulate: bool,
        set_flags: bool,
    },
    /// Signed halfword and word-by-halfword multiply forms.
    HalfwordMultiply {
        class: u32,
        rd: u8,
        rn: u8,
        rs: u8,
        rm: u8,
        x: bool,
        y: bool,
    },
    /// Dual-halfword and most-significant-word DSP multiplies.
    DspMultiply {
        class: u32,
        rd: u8,
        ra: u8,
        rs: u8,
        rm: u8,
        bit7: bool,
        subtract: bool,
        exchange_or_round: bool,
    },
    /// Branch displacement and link behavior decoded ahead of execution.
    Branch { displacement: i32, link: bool },
    /// Compatibility node used only for encodings not recognized by the decoder.
    A32Fallback(Entry),
}

impl Arm11Core {
    /// Executes one lowered IR operation while preserving the regular step
    /// event, memory error, condition, branch and exception-return contracts.
    #[inline]
    pub(super) fn execute_ir<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        instruction: IrInstruction,
    ) -> Result<StepEvent> {
        if !instruction.condition.passes(self.cpsr) {
            return Ok(StepEvent::Executed);
        }

        match instruction.operation {
            IrOperation::LoadImmediate { rd, value } => {
                self.regs[usize::from(rd)] = value;
                Ok(StepEvent::Executed)
            }
            IrOperation::AluImmediate {
                opcode,
                set_flags,
                rn,
                rd,
                immediate,
                shifter_carry,
            } => {
                self.execute_alu(
                    opcode,
                    set_flags,
                    rn,
                    rd,
                    immediate,
                    shifter_carry.unwrap_or_else(|| self.cpsr.c()),
                );
                Ok(StepEvent::Executed)
            }
            IrOperation::AluRegisterShiftImmediate {
                opcode,
                set_flags,
                rn,
                rd,
                rm,
                shift_kind,
                shift_amount,
            } => {
                let (operand, carry) = shift_imm(
                    shift_kind,
                    self.regs[usize::from(rm)],
                    u32::from(shift_amount),
                    self.cpsr.c(),
                );
                self.execute_alu(opcode, set_flags, rn, rd, operand, carry);
                Ok(StepEvent::Executed)
            }
            IrOperation::TransferImmediate {
                load,
                byte,
                pre_index,
                writeback,
                rn,
                rd,
                offset,
            } => {
                let base = self.regs[usize::from(rn)];
                let offset_address = base.wrapping_add_signed(offset);
                let address = if pre_index { offset_address } else { base };
                if load {
                    let value = if byte {
                        u32::from(bus.read8(address)?)
                    } else {
                        self.read_armv6_single_word(
                            bus,
                            address,
                            false,
                            instruction.diagnostic_word,
                        )?
                    };
                    if writeback {
                        self.regs[usize::from(rn)] = offset_address;
                    }
                    self.regs[usize::from(rd)] = value;
                } else {
                    let value = self.regs[usize::from(rd)];
                    if byte {
                        bus.write8(address, value as u8)?;
                    } else {
                        self.write_armv6_single_word(
                            bus,
                            address,
                            value,
                            false,
                            instruction.diagnostic_word,
                        )?;
                    }
                    if writeback {
                        self.regs[usize::from(rn)] = offset_address;
                    }
                }
                Ok(StepEvent::Executed)
            }
            IrOperation::HalfwordTransfer {
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                offset,
                transfer_kind,
            } => self.execute_cached_halfword_transfer(
                bus,
                instruction.diagnostic_word,
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                u32::from(offset),
                transfer_kind,
            ),
            IrOperation::HalfwordTransferRegister {
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                rm,
                transfer_kind,
            } => self.execute_cached_halfword_transfer(
                bus,
                instruction.diagnostic_word,
                pre_index,
                up,
                writeback,
                load,
                rn,
                rd,
                self.regs[usize::from(rm)],
                transfer_kind,
            ),
            IrOperation::BlockTransfer {
                pre_index,
                up,
                writeback,
                load,
                s_bit,
                rn,
                list,
            } => self.execute_block_transfer_decoded(
                bus,
                instruction.diagnostic_word,
                pre_index,
                up,
                writeback,
                load,
                s_bit,
                u32::from(rn),
                u32::from(list),
            ),
            IrOperation::Multiply {
                rd,
                rn,
                rs,
                rm,
                accumulate,
                set_flags,
            } => {
                self.execute_cached_multiply(
                    instruction.diagnostic_word,
                    rd,
                    rn,
                    rs,
                    rm,
                    accumulate,
                    set_flags,
                )?;
                Ok(StepEvent::Executed)
            }
            IrOperation::MultiplyLong {
                rd_hi,
                rd_lo,
                rs,
                rm,
                signed,
                accumulate,
                set_flags,
            } => {
                self.execute_cached_multiply_long(
                    instruction.diagnostic_word,
                    signed,
                    accumulate,
                    set_flags,
                    rd_hi,
                    rd_lo,
                    rs,
                    rm,
                )?;
                Ok(StepEvent::Executed)
            }
            IrOperation::HalfwordMultiply {
                class,
                rd,
                rn,
                rs,
                rm,
                x,
                y,
            } => self.execute_cached_halfword_multiply(
                instruction.diagnostic_word,
                class,
                rd,
                rn,
                rs,
                rm,
                x,
                y,
            ),
            IrOperation::DspMultiply {
                class,
                rd,
                ra,
                rs,
                rm,
                bit7,
                subtract,
                exchange_or_round,
            } => self.execute_cached_signed_dsp_multiply_instruction(
                instruction.diagnostic_word,
                class,
                rd,
                ra,
                rs,
                rm,
                bit7,
                subtract,
                exchange_or_round,
            ),
            IrOperation::Branch { displacement, link } => {
                let pc = self.pc();
                if link {
                    self.regs[LR] = pc.wrapping_add(4);
                }
                self.branch(pc.wrapping_add(8).wrapping_add(displacement as u32));
                Ok(StepEvent::Executed)
            }
            IrOperation::A32Fallback(entry) => self.execute_arm_decoded(bus, entry.word, entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_common_alu_instructions_without_retaining_the_guest_word() {
        let entry = Entry {
            word: 0xE3A0_0007, // MOV r0, #7
            condition: 0xE,
            op: FastOp::DpImm {
                opcode: 0xD,
                set_flags: false,
                rn: 0,
                rd: 0,
                imm: 7,
                imm_carry: None,
            },
        };

        let lowered = IrInstruction::lower(entry);
        assert_eq!(lowered.condition, Condition::Al);
        assert!(matches!(
            lowered.operation,
            IrOperation::LoadImmediate { value: 7, .. }
        ));
    }

    #[test]
    fn lowers_mvn_immediate_without_changing_flags() {
        let entry = Entry {
            word: 0xE3E0_0007, // MVN r0, #7
            condition: 0xE,
            op: FastOp::DpImm {
                opcode: 0xF,
                set_flags: false,
                rn: 0,
                rd: 0,
                imm: 7,
                imm_carry: Some(false),
            },
        };
        let lowered = IrInstruction::lower(entry);
        assert!(matches!(
            lowered.operation,
            IrOperation::LoadImmediate {
                rd: 0,
                value: 0xFFFF_FFF8
            }
        ));
    }

    #[test]
    fn lowers_branch_displacement_and_link_without_retaining_the_guest_word() {
        let entry = Entry {
            word: 0xEBFF_FFFE, // BL to PC - 8
            condition: 0xE,
            op: FastOp::Branch,
        };

        let lowered = IrInstruction::lower(entry);
        assert!(matches!(
            lowered.operation,
            IrOperation::Branch {
                displacement: -8,
                link: true
            }
        ));
    }

    #[test]
    fn lowers_all_remaining_cached_instruction_families_to_native_ir() {
        let transfer = FastOp::TransferImm {
            load: true,
            byte: false,
            pre_index: true,
            writeback: false,
            rn: 0,
            rd: 1,
            offset: 4,
        };
        let halfword_immediate = FastOp::HalfwordTransfer {
            pre_index: true,
            up: true,
            writeback: false,
            load: true,
            rn: 0,
            rd: 1,
            offset: 4,
            transfer_kind: 1,
        };
        let halfword_register = FastOp::HalfwordTransferReg {
            pre_index: true,
            up: true,
            writeback: false,
            load: true,
            rn: 0,
            rd: 1,
            rm: 2,
            transfer_kind: 1,
        };
        let block = FastOp::BlockTransfer {
            pre_index: false,
            up: true,
            writeback: true,
            load: true,
            s_bit: false,
            rn: 0,
            list: 6,
        };
        let multiply = FastOp::Multiply {
            rd: 0,
            rn: 0,
            rs: 2,
            rm: 1,
            accumulate: false,
            set_flags: false,
        };
        let multiply_long = FastOp::MultiplyLong {
            rd_hi: 2,
            rd_lo: 3,
            rs: 1,
            rm: 4,
            signed: false,
            accumulate: false,
            set_flags: false,
        };
        let halfword_multiply = FastOp::HalfwordMultiply {
            class: 0x0100_0080,
            rd: 3,
            rn: 0,
            rs: 2,
            rm: 1,
            x: false,
            y: false,
        };
        let dsp_multiply = FastOp::DspMultiply {
            class: 0x0700_0010,
            rd: 4,
            ra: 5,
            rs: 2,
            rm: 1,
            bit7: false,
            subtract: false,
            exchange_or_round: false,
        };
        let entries = [
            Entry {
                word: 0xE590_1004,
                condition: 0xE,
                op: transfer,
            },
            Entry {
                word: 0xE1D0_10B4,
                condition: 0xE,
                op: halfword_immediate,
            },
            Entry {
                word: 0xE190_10B2,
                condition: 0xE,
                op: halfword_register,
            },
            Entry {
                word: 0xE8B0_0006,
                condition: 0xE,
                op: block,
            },
            Entry {
                word: 0xE000_0291,
                condition: 0xE,
                op: multiply,
            },
            Entry {
                word: 0xE081_2394,
                condition: 0xE,
                op: multiply_long,
            },
            Entry {
                word: 0xE100_3281,
                condition: 0xE,
                op: halfword_multiply,
            },
            Entry {
                word: 0xE704_5201,
                condition: 0xE,
                op: dsp_multiply,
            },
        ];
        for entry in entries {
            assert!(!matches!(
                IrInstruction::lower(entry).operation,
                IrOperation::A32Fallback(_)
            ));
        }
    }

    #[test]
    fn keeps_unknown_instructions_in_the_explicit_compatibility_node() {
        let entry = Entry {
            word: 0xE000_0291,
            condition: 0xE,
            op: FastOp::Generic,
        };

        assert!(matches!(
            IrInstruction::lower(entry).operation,
            IrOperation::A32Fallback(fallback) if fallback.word == entry.word
        ));
    }
}
