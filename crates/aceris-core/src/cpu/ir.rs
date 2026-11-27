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
    pub(super) condition: u8,
    pub(super) operation: IrOperation,
}

impl IrInstruction {
    pub(super) const EMPTY: Self = Self {
        condition: 0xF,
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
            FastOp::Branch => {
                let displacement = (((entry.word & 0x00FF_FFFF) << 8) as i32) >> 6;
                IrOperation::Branch {
                    displacement,
                    link: entry.word & (1 << 24) != 0,
                }
            }
            _ => IrOperation::A32Fallback(entry),
        };
        Self {
            condition: entry.condition,
            operation,
        }
    }
}

/// Operations in the first executable tier of the Aceris ARM11 IR.
#[derive(Clone, Copy, Debug)]
pub(super) enum IrOperation {
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
    /// Branch displacement and link behavior decoded ahead of execution.
    Branch { displacement: i32, link: bool },
    /// Transitional node for instructions not yet represented by native IR ops.
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
        if !Condition::from_bits(instruction.condition).passes(self.cpsr) {
            return Ok(StepEvent::Executed);
        }

        match instruction.operation {
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
            IrOperation::Branch {
                displacement,
                link,
            } => {
                let pc = self.pc();
                if link {
                    self.regs[LR] = pc.wrapping_add(4);
                }
                self.branch(pc.wrapping_add(8).wrapping_add(displacement as u32));
                Ok(StepEvent::Executed)
            }
            IrOperation::A32Fallback(entry) => {
                self.execute_arm_decoded(bus, entry.word, entry)
            }
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
        assert_eq!(lowered.condition, 0xE);
        assert!(matches!(
            lowered.operation,
            IrOperation::AluImmediate {
                opcode: 0xD,
                immediate: 7,
                ..
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
    fn keeps_unlowered_instructions_in_the_explicit_compatibility_node() {
        let entry = Entry {
            word: 0xE000_0291,
            condition: 0xE,
            op: FastOp::Multiply {
                rd: 0,
                rn: 0,
                rs: 2,
                rm: 1,
                accumulate: false,
                set_flags: false,
            },
        };

        assert!(matches!(
            IrInstruction::lower(entry).operation,
            IrOperation::A32Fallback(fallback) if fallback.word == entry.word
        ));
    }
}
