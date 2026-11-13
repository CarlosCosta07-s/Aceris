//! ARM (A32) instruction decoding and execution.

use super::shifter::{ShiftKind, add_with_carry, shift_imm, shift_reg};
use super::{Arm11Core, Condition, LR, PC, StepEvent};
use crate::memory::Bus;
use aceris_common::{Error, Result};

pub(super) fn bit(word: u32, n: u32) -> bool {
    (word >> n) & 1 != 0
}

/// Extracts bits `hi..=lo` of `word`.
pub(super) fn bits(word: u32, hi: u32, lo: u32) -> u32 {
    (word >> lo) & ((1 << (hi - lo + 1)) - 1)
}

/// Result of an ALU operation, keeping what the flag update needs.
enum Alu {
    Logical(u32),
    Arithmetic {
        value: u32,
        carry: bool,
        overflow: bool,
    },
}

fn arithmetic(a: u32, b: u32, carry_in: bool) -> Alu {
    let (value, carry, overflow) = add_with_carry(a, b, carry_in);
    Alu::Arithmetic {
        value,
        carry,
        overflow,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParallelOperation {
    Add,
    Subtract,
    AddSubtractExchange,
    SubtractAddExchange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParallelArithmetic {
    SignedModulo,
    SignedSaturating,
    SignedHalving,
    UnsignedModulo,
    UnsignedSaturating,
    UnsignedHalving,
}

fn sign_extend_lane(value: u32, width: u32) -> i64 {
    let shift = 32 - width;
    i64::from(((value << shift) as i32) >> shift)
}

/// Saturates a signed intermediate to the ARM 32-bit signed range.
fn signed_saturate_32(value: i64) -> (i32, bool) {
    let saturated = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    (saturated as i32, saturated != value)
}

/// Returns one signed 16-bit lane from a register as a widened integer.
fn signed_halfword(value: u32, top: bool) -> i64 {
    let halfword = if top { value >> 16 } else { value & 0xFFFF };
    i64::from(halfword as u16 as i16)
}

impl Arm11Core {
    pub(super) fn execute_arm<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        // A32 WFE/WFI/SEV share the architectural NOP32 hint encoding.
        // NOP, YIELD, and reserved hints are no-ops independent of condition;
        // WFE, WFI, and SEV retain their normal ARM condition check.
        if word & 0x0FFF_FF00 == 0x0320_F000 {
            let hint = bits(word, 7, 0);
            if (2..=4).contains(&hint) {
                let condition = (word >> 28) as u8;
                if !Condition::from_bits(condition).passes(self.cpsr) {
                    return Ok(StepEvent::Executed);
                }

                return match hint {
                    2 => {
                        if self.event_registered {
                            self.event_registered = false;
                            Ok(StepEvent::Executed)
                        } else {
                            self.waiting_for_event = true;
                            Ok(StepEvent::WaitForEvent)
                        }
                    }
                    3 => {
                        self.waiting_for_interrupt = true;
                        Ok(StepEvent::WaitForInterrupt)
                    }
                    4 => {
                        self.signal_event();
                        Ok(StepEvent::SendEvent)
                    }
                    _ => unreachable!(),
                };
            }

            return Ok(StepEvent::Executed);
        }

        let condition = (word >> 28) as u8;
        if condition == 0xF {
            return self.execute_unconditional(bus, word);
        }
        if !Condition::from_bits(condition).passes(self.cpsr) {
            return Ok(StepEvent::Executed);
        }

        match bits(word, 27, 25) {
            0b000 | 0b001 => self.execute_data_space(bus, word),
            0b010 => self.single_transfer(bus, word),
            0b011 if bit(word, 4) => self.execute_media_instruction(word),
            0b011 => self.single_transfer(bus, word),
            0b100 => self.block_transfer(bus, word),
            0b101 => {
                self.branch_with_link(word);
                Ok(StepEvent::Executed)
            }
            0b110 => self.execute_coprocessor(bus, word),
            _ if bit(word, 24) => {
                if self.svc_exception_entry_enabled {
                    self.enter_svc_exception(self.pc().wrapping_add(4))?;
                }
                Ok(StepEvent::Svc(bits(word, 23, 0)))
            }
            _ => self.execute_coprocessor(bus, word),
        }
    }

    /// Executes the ARMv6 scalar media instructions implemented by this core.
    fn execute_media_instruction(&mut self, word: u32) -> Result<StepEvent> {
        if matches!(word & 0x0FF0_0010, 0x0700_0010 | 0x0740_0010 | 0x0750_0010) {
            return self.execute_signed_dsp_multiply_instruction(word);
        }
        let rd = bits(word, 15, 12) as usize;
        let rm = bits(word, 3, 0) as usize;

        if word & 0x0FFF_0FF0 == 0x06BF_0F30 {
            // REV Rd, Rm
            if rd == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            self.regs[rd] = self.regs[rm].swap_bytes();
        } else if word & 0x0FFF_0FF0 == 0x06BF_0FB0 {
            // REV16 Rd, Rm
            if rd == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let value = self.regs[rm];
            self.regs[rd] = ((value & 0x00FF_00FF) << 8) | ((value & 0xFF00_FF00) >> 8);
        } else if word & 0x0FFF_0FF0 == 0x06FF_0FB0 {
            // REVSH Rd, Rm
            if rd == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            self.regs[rd] = i32::from((self.regs[rm] as u16).swap_bytes() as i16) as u32;
        } else if matches!(
            word & 0x0FF0_03F0,
            0x0680_0070 | 0x06A0_0070 | 0x06B0_0070 | 0x06C0_0070 | 0x06E0_0070 | 0x06F0_0070
        ) {
            // ARMv6 signed/unsigned extend, optionally accumulating into Rn.
            return self.execute_extend_instruction(word);
        } else if matches!(word & 0x0FF0_0FF0, 0x06A0_0F30 | 0x06E0_0F30) {
            // SSAT16/USAT16 saturate the two signed source halfwords independently.
            // SSAT16 encodes sat-1 (1..=16); USAT16 encodes sat (0..=15).
            let rd = bits(word, 15, 12) as usize;
            let rm = bits(word, 3, 0) as usize;
            if rd == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }

            let saturation_bits = bits(word, 19, 16);
            let signed_saturate = word & 0x0FF0_0FF0 == 0x06A0_0F30;
            let mut result = 0u32;
            let mut saturated = false;
            let source = self.regs[rm];

            for lane in 0..2 {
                let value = signed_halfword(source, lane == 1);
                let clamped = if signed_saturate {
                    let width = saturation_bits + 1;
                    let minimum = -(1i64 << (width - 1));
                    let maximum = (1i64 << (width - 1)) - 1;
                    value.clamp(minimum, maximum)
                } else {
                    let maximum = if saturation_bits == 0 {
                        0
                    } else {
                        (1i64 << saturation_bits) - 1
                    };
                    value.clamp(0, maximum)
                };
                saturated |= clamped != value;
                result |= u32::from(clamped as i16 as u16) << (lane * 16);
            }

            self.regs[rd] = result;
            if saturated {
                self.cpsr.set_q();
            }
        } else if word & 0x0FE0_0070 == 0x06A0_0010 || word & 0x0FE0_0070 == 0x06E0_0010 {
            // SSAT/USAT: shift a signed source, saturate, and set sticky CPSR.Q
            // only when the result is clamped.
            let rd = bits(word, 15, 12) as usize;
            let rm = bits(word, 3, 0) as usize;
            if rd == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }

            let shift_amount = bits(word, 11, 7);
            let shifted = if bit(word, 6) {
                // ASR #32 is encoded as an immediate of zero.
                if shift_amount == 0 {
                    (self.regs[rm] as i32 >> 31) as u32
                } else {
                    ((self.regs[rm] as i32) >> shift_amount) as u32
                }
            } else {
                self.regs[rm].wrapping_shl(shift_amount)
            };
            let signed_value = i64::from(shifted as i32);
            let saturation_bits = bits(word, 20, 16);
            let signed_saturate = word & 0x0FE0_0070 == 0x06A0_0010;

            let (result, saturated) = if signed_saturate {
                let width = saturation_bits + 1;
                let (minimum, maximum) = if width == 32 {
                    (i64::from(i32::MIN), i64::from(i32::MAX))
                } else {
                    (-(1i64 << (width - 1)), (1i64 << (width - 1)) - 1)
                };
                let clamped = signed_value.clamp(minimum, maximum);
                (clamped as u32, clamped != signed_value)
            } else {
                let maximum = if saturation_bits == 0 {
                    0
                } else {
                    (1i64 << saturation_bits) - 1
                };
                let clamped = signed_value.clamp(0, maximum);
                (clamped as u32, clamped != signed_value)
            };

            self.regs[rd] = result;
            if saturated {
                self.cpsr.set_q();
            }
        } else if word & 0x0FF0_0070 == 0x0680_0010 || word & 0x0FF0_0070 == 0x0680_0050 {
            // PKHBT / PKHTB: combine halfwords after shifting Rm.
            let rn = bits(word, 19, 16) as usize;
            if rd == PC || rn == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let shift = bits(word, 11, 7);
            let shifted = if word & 0x40 == 0 {
                self.regs[rm].wrapping_shl(shift)
            } else if shift == 0 {
                (self.regs[rm] as i32 >> 31) as u32
            } else {
                ((self.regs[rm] as i32) >> shift) as u32
            };
            self.regs[rd] = if word & 0x40 == 0 {
                (shifted & 0xFFFF_0000) | (self.regs[rn] & 0x0000_FFFF)
            } else {
                (self.regs[rn] & 0xFFFF_0000) | (shifted & 0x0000_FFFF)
            };
        } else if word & 0x0FF0_0FF0 == 0x0680_0FB0 {
            // SEL chooses each result byte using the corresponding CPSR.GE flag.
            let rn = bits(word, 19, 16) as usize;
            if rd == PC || rn == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let first = self.regs[rn];
            let second = self.regs[rm];
            let ge = self.cpsr.ge();
            let mut result = 0u32;
            for lane in 0..4 {
                let source = if ge & (1 << lane) != 0 { first } else { second };
                result |= source & (0xFF << (lane * 8));
            }
            self.regs[rd] = result;
        } else if self.execute_parallel_media_instruction(word)? {
            // The instruction updates packed lanes and, where applicable, GE.
        } else if word & 0x0FF0_00F0 == 0x0780_0010 {
            // USAD8/USADA8: Rd is bits 19:16 and Ra is bits 15:12.
            let rd = bits(word, 19, 16) as usize;
            let accumulator = bits(word, 15, 12) as usize;
            let source_a = bits(word, 11, 8) as usize;
            let source_b = rm;
            if rd == PC || source_a == PC || source_b == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }

            let a = self.regs[source_a];
            let b = self.regs[source_b];
            let mut sum = 0u32;
            for lane in 0..4 {
                let byte_a = (a >> (lane * 8)) & 0xFF;
                let byte_b = (b >> (lane * 8)) & 0xFF;
                sum = sum.wrapping_add(byte_a.abs_diff(byte_b));
            }
            if accumulator != PC {
                sum = sum.wrapping_add(self.regs[accumulator]);
            }
            self.regs[rd] = sum;
        } else {
            // Every ARMv6 media encoding is handled above; the rest of this
            // space (including the permanently-undefined UDF encoding) is undefined.
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        Ok(StepEvent::Executed)
    }

    /// Executes signed halfword and word-by-halfword multiply instructions.
    fn execute_halfword_multiply_instruction(&mut self, word: u32) -> Result<StepEvent> {
        let class = word & 0x0FF0_0090;
        let rd = bits(word, 19, 16) as usize;
        let rn = bits(word, 15, 12) as usize;
        let rs = bits(word, 11, 8) as usize;
        let rm = bits(word, 3, 0) as usize;
        let pc = self.pc();
        let undefined = || Error::UndefinedInstruction { pc, word };

        match class {
            0x0100_0080 => {
                // SMLAxy: signed halfword product plus a signed 32-bit accumulator.
                if rd == PC || rn == PC || rs == PC || rm == PC {
                    return Err(undefined());
                }
                let first = signed_halfword(self.regs[rm], bit(word, 5));
                let second = signed_halfword(self.regs[rs], bit(word, 6));
                let result = first * second + i64::from(self.regs[rn] as i32);
                if result < i64::from(i32::MIN) || result > i64::from(i32::MAX) {
                    self.cpsr.set_q();
                }
                self.regs[rd] = result as u32;
            }
            0x0120_0080 => {
                // SMLAWy and SMULWy share the same opcode class. Bit 5
                // selects the non-accumulating SMULW form.
                if rd == PC || rs == PC || rm == PC {
                    return Err(undefined());
                }
                let word_operand = i64::from(self.regs[rm] as i32);
                let halfword_operand = signed_halfword(self.regs[rs], bit(word, 6));
                let product_high = (word_operand * halfword_operand) >> 16;
                if bit(word, 5) {
                    // SMULWy reserves bits[15:12] as zero.
                    if rn != 0 {
                        return Err(undefined());
                    }
                    self.regs[rd] = product_high as u32;
                } else {
                    // SMLAWy accumulates the high product word with Rn.
                    if rn == PC {
                        return Err(undefined());
                    }
                    let result = product_high + i64::from(self.regs[rn] as i32);
                    if result < i64::from(i32::MIN) || result > i64::from(i32::MAX) {
                        self.cpsr.set_q();
                    }
                    self.regs[rd] = result as u32;
                }
            }
            0x0140_0080 => {
                // SMLALxy: sign-extended 16x16 product added to RdHi:RdLo.
                let rd_hi = rd;
                let rd_lo = rn;
                if rd_hi == PC || rd_lo == PC || rs == PC || rm == PC || rd_hi == rd_lo {
                    return Err(undefined());
                }
                let first = signed_halfword(self.regs[rm], bit(word, 5));
                let second = signed_halfword(self.regs[rs], bit(word, 6));
                let product = first * second;
                let accumulator = (u64::from(self.regs[rd_hi]) << 32) | u64::from(self.regs[rd_lo]);
                let result = accumulator.wrapping_add(product as u64);
                self.regs[rd_lo] = result as u32;
                self.regs[rd_hi] = (result >> 32) as u32;
            }
            0x0160_0080 => {
                // SMULxy: signed halfword product. Bits[15:12] are SBZ.
                if rn != 0 || rd == PC || rs == PC || rm == PC {
                    return Err(undefined());
                }
                let first = signed_halfword(self.regs[rm], bit(word, 5));
                let second = signed_halfword(self.regs[rs], bit(word, 6));
                self.regs[rd] = (first * second) as u32;
            }
            _ => return Err(undefined()),
        }
        Ok(StepEvent::Executed)
    }

    /// Executes ARMv6 dual-halfword and most-significant-word multiply instructions.
    fn execute_signed_dsp_multiply_instruction(&mut self, word: u32) -> Result<StepEvent> {
        let class = word & 0x0FF0_0010;
        let rd = bits(word, 19, 16) as usize;
        let ra = bits(word, 15, 12) as usize;
        let rs = bits(word, 11, 8) as usize;
        let rm = bits(word, 3, 0) as usize;
        let pc = self.pc();
        let undefined = || Error::UndefinedInstruction { pc, word };

        match class {
            0x0700_0010 => {
                // SMLAD{X}/SMLSD{X}; Ra=PC selects SMUAD{X}/SMUSD{X}.
                if bit(word, 7) || rd == PC || rm == PC || rs == PC {
                    return Err(undefined());
                }
                let subtract = bit(word, 6);
                let exchange = bit(word, 5);
                let first = self.regs[rm];
                let second = if exchange {
                    self.regs[rs].rotate_right(16)
                } else {
                    self.regs[rs]
                };
                let low_product = signed_halfword(first, false) * signed_halfword(second, false);
                let high_product = signed_halfword(first, true) * signed_halfword(second, true);
                let products = if subtract {
                    low_product - high_product
                } else {
                    low_product + high_product
                };
                let accumulator = if ra == PC {
                    0
                } else {
                    i64::from(self.regs[ra] as i32)
                };
                let result = accumulator + products;

                // SMUAD and accumulating dual forms set sticky Q on overflow.
                // SMUSD's unaccumulated product difference always fits in i32.
                if (ra != PC || !subtract)
                    && (result < i64::from(i32::MIN) || result > i64::from(i32::MAX))
                {
                    self.cpsr.set_q();
                }
                self.regs[rd] = result as u32;
            }
            0x0740_0010 => {
                // SMLALD{X}/SMLSLD{X}; 64-bit accumulator in RdHi:RdLo.
                let rd_hi = rd;
                let rd_lo = ra;
                if bit(word, 7)
                    || rd_hi == PC
                    || rd_lo == PC
                    || rd_hi == rd_lo
                    || rm == PC
                    || rs == PC
                {
                    return Err(undefined());
                }
                let exchange = bit(word, 5);
                let subtract = bit(word, 6);
                let first = self.regs[rm];
                let second = if exchange {
                    self.regs[rs].rotate_right(16)
                } else {
                    self.regs[rs]
                };
                let low_product = signed_halfword(first, false) * signed_halfword(second, false);
                let high_product = signed_halfword(first, true) * signed_halfword(second, true);
                let products = if subtract {
                    low_product - high_product
                } else {
                    low_product + high_product
                };
                let accumulator = (u64::from(self.regs[rd_hi]) << 32) | u64::from(self.regs[rd_lo]);
                let result = accumulator.wrapping_add(products as u64);
                self.regs[rd_lo] = result as u32;
                self.regs[rd_hi] = (result >> 32) as u32;
            }
            0x0750_0010 => {
                // SMMUL{R}/SMMLA{R}/SMMLS{R}; bit 6 selects subtraction,
                // and bit 5 requests rounding before extracting the high word.
                if bit(word, 7) != bit(word, 6) || rd == PC || rm == PC || rs == PC {
                    return Err(undefined());
                }
                let subtract = bit(word, 6);
                let round = if bit(word, 5) { 1i128 << 31 } else { 0 };
                if subtract && ra == PC {
                    return Err(undefined());
                }

                let first = i128::from(self.regs[rm] as i32);
                let second = i128::from(self.regs[rs] as i32);
                let product = first * second;
                let wide_result = if subtract {
                    (i128::from(self.regs[ra] as i32) << 32) - product + round
                } else if ra == PC {
                    product + round
                } else {
                    (i128::from(self.regs[ra] as i32) << 32) + product + round
                };
                self.regs[rd] = (wide_result >> 32) as u32;
            }
            _ => return Err(undefined()),
        }

        Ok(StepEvent::Executed)
    }

    /// Executes ARMv6 byte/halfword extension and extension-with-add forms.
    ///
    /// Rn=PC selects the non-accumulating operation. The input is rotated
    /// right by 0, 8, 16, or 24 bits before extraction; additions wrap at the
    /// destination width and do not alter CPSR flags.
    fn execute_extend_instruction(&mut self, word: u32) -> Result<StepEvent> {
        let operation = bits(word, 21, 20);
        let unsigned = bit(word, 22);
        let rn = bits(word, 19, 16) as usize;
        let rd = bits(word, 15, 12) as usize;
        let rotation = bits(word, 11, 10) * 8;
        let rm = bits(word, 3, 0) as usize;

        if rd == PC || rm == PC {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let rotated = self.regs[rm].rotate_right(rotation);
        let accumulator = if rn == PC { 0 } else { self.regs[rn] };

        match operation {
            0b00 => {
                // SXTB16/UXTB16 and SXTAB16/UXTAB16.
                let upper_byte = (rotated >> 16) as u8;
                let lower_byte = rotated as u8;
                let upper = if unsigned {
                    u16::from(upper_byte)
                } else {
                    i16::from(upper_byte as i8) as u16
                };
                let lower = if unsigned {
                    u16::from(lower_byte)
                } else {
                    i16::from(lower_byte as i8) as u16
                };
                let upper_base = (accumulator >> 16) as u16;
                let lower_base = accumulator as u16;
                let result = (upper_base.wrapping_add(upper) as u32) << 16
                    | u32::from(lower_base.wrapping_add(lower));
                self.regs[rd] = result;
            }
            0b10 => {
                // SXTB/UXTB and SXTAB/UXTAB.
                let extended = if unsigned {
                    u32::from(rotated as u8)
                } else {
                    i32::from(rotated as u8 as i8) as u32
                };
                self.regs[rd] = accumulator.wrapping_add(extended);
            }
            0b11 => {
                // SXTH/UXTH and SXTAH/UXTAH.
                let extended = if unsigned {
                    u32::from(rotated as u16)
                } else {
                    i32::from(rotated as u16 as i16) as u32
                };
                self.regs[rd] = accumulator.wrapping_add(extended);
            }
            _ => {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
        }

        Ok(StepEvent::Executed)
    }

    /// Executes the complete ARMv6 parallel add/subtract instruction family.
    ///
    /// Implements all 36 combinations of signed/unsigned modular,
    /// saturating, and halving arithmetic over byte and halfword lanes.
    /// Only the non-saturating, non-halving S/U variants update GE.
    fn execute_parallel_media_instruction(&mut self, word: u32) -> Result<bool> {
        if bits(word, 27, 24) != 0b0110 || bits(word, 11, 8) != 0xF || !bit(word, 4) {
            return Ok(false);
        }

        let arithmetic = match bits(word, 22, 20) {
            0b001 => ParallelArithmetic::SignedModulo,
            0b010 => ParallelArithmetic::SignedSaturating,
            0b011 => ParallelArithmetic::SignedHalving,
            0b101 => ParallelArithmetic::UnsignedModulo,
            0b110 => ParallelArithmetic::UnsignedSaturating,
            0b111 => ParallelArithmetic::UnsignedHalving,
            _ => return Ok(false),
        };
        let rn = bits(word, 19, 16) as usize;
        let rd = bits(word, 15, 12) as usize;
        let rm = bits(word, 3, 0) as usize;
        if rd == PC || rn == PC || rm == PC {
            // REV/REV16/REVSH and other encodings with Rn=PC are decoded
            // before this helper. Parallel arithmetic itself forbids PC.
            return Ok(false);
        }

        let width = if bit(word, 7) { 8 } else { 16 };
        let operation_code = bits(word, 6, 5);
        let operation = match (width, operation_code) {
            (8, 0) => ParallelOperation::Add,
            (8, 3) => ParallelOperation::Subtract,
            (16, 0) => ParallelOperation::Add,
            (16, 1) => ParallelOperation::AddSubtractExchange,
            (16, 2) => ParallelOperation::SubtractAddExchange,
            (16, 3) => ParallelOperation::Subtract,
            _ => return Ok(false),
        };

        let lhs = self.regs[rn];
        let rhs = self.regs[rm];
        let lane_count = 32 / width;
        let lane_mask = (1u32 << width) - 1;
        let signed_min = -(1i64 << (width - 1));
        let signed_max = (1i64 << (width - 1)) - 1;
        let mut result = 0u32;
        let mut ge = 0u8;
        let mut updates_ge = false;

        for lane in 0..lane_count {
            let rhs_lane = match operation {
                ParallelOperation::AddSubtractExchange | ParallelOperation::SubtractAddExchange => {
                    lane_count - lane - 1
                }
                _ => lane,
            };
            let shift_lhs = lane * width;
            let shift_rhs = rhs_lane * width;
            let a = (lhs >> shift_lhs) & lane_mask;
            let b = (rhs >> shift_rhs) & lane_mask;
            let is_add = match operation {
                ParallelOperation::Add => true,
                ParallelOperation::Subtract => false,
                ParallelOperation::AddSubtractExchange => lane == lane_count - 1,
                ParallelOperation::SubtractAddExchange => lane != lane_count - 1,
            };

            let (lane_result, ge_set) = match arithmetic {
                ParallelArithmetic::SignedModulo => {
                    updates_ge = true;
                    let a = sign_extend_lane(a, width);
                    let b = sign_extend_lane(b, width);
                    let value = if is_add { a + b } else { a - b };
                    (value as u32 & lane_mask, Some(value >= 0))
                }
                ParallelArithmetic::UnsignedModulo => {
                    updates_ge = true;
                    if is_add {
                        let value = u64::from(a) + u64::from(b);
                        (value as u32 & lane_mask, Some(value > u64::from(lane_mask)))
                    } else {
                        (a.wrapping_sub(b) & lane_mask, Some(a >= b))
                    }
                }
                ParallelArithmetic::SignedSaturating => {
                    let a = sign_extend_lane(a, width);
                    let b = sign_extend_lane(b, width);
                    let value = if is_add { a + b } else { a - b };
                    (value.clamp(signed_min, signed_max) as u32 & lane_mask, None)
                }
                ParallelArithmetic::UnsignedSaturating => {
                    let value = if is_add {
                        i64::from(a) + i64::from(b)
                    } else {
                        i64::from(a) - i64::from(b)
                    };
                    (value.clamp(0, i64::from(lane_mask)) as u32, None)
                }
                ParallelArithmetic::SignedHalving => {
                    let a = sign_extend_lane(a, width);
                    let b = sign_extend_lane(b, width);
                    let value = if is_add { a + b } else { a - b };
                    ((value >> 1) as u32 & lane_mask, None)
                }
                ParallelArithmetic::UnsignedHalving => {
                    // The architectural result is the middle half of a
                    // widened 17-bit add/subtract result, including borrow.
                    let value = if is_add {
                        i64::from(a) + i64::from(b)
                    } else {
                        i64::from(a) - i64::from(b)
                    };
                    ((value >> 1) as u32 & lane_mask, None)
                }
            };

            result |= lane_result << shift_lhs;
            if let Some(ge_set) = ge_set {
                if ge_set {
                    if width == 16 {
                        ge |= 0b11 << (lane * 2);
                    } else {
                        ge |= 1 << lane;
                    }
                }
            }
        }

        self.regs[rd] = result;
        if updates_ge {
            self.cpsr.set_ge(ge);
        }
        Ok(true)
    }

    // ----- register helpers -------------------------------------------------

    /// Reads register `n`. Reading `pc` yields the current instruction address
    /// plus `pc_offset` (8 for most operands, 12 for register-controlled shifts).
    pub(super) fn reg(&self, n: u32, pc_offset: u32) -> u32 {
        if n as usize == PC {
            self.regs[PC].wrapping_add(pc_offset)
        } else {
            self.regs[n as usize]
        }
    }

    /// Reads a halfword using the current CPSR data endianness.
    pub(super) fn read_data_u16<B: Bus + ?Sized>(&self, bus: &mut B, address: u32) -> Result<u16> {
        let value = bus.read16(address)?;
        Ok(if self.cpsr.big_endian() {
            value.swap_bytes()
        } else {
            value
        })
    }

    /// Reads a word using the current CPSR data endianness.
    pub(super) fn read_data_u32<B: Bus + ?Sized>(&self, bus: &mut B, address: u32) -> Result<u32> {
        let value = bus.read32(address)?;
        Ok(if self.cpsr.big_endian() {
            value.swap_bytes()
        } else {
            value
        })
    }

    /// Reads an unaligned word with the classic endian-dependent rotation.
    pub(super) fn read_data_u32_unaligned<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
    ) -> Result<u32> {
        let value = self.read_data_u32(bus, address & !3)?;
        let rotation = (address & 3) * 8;
        Ok(if self.cpsr.big_endian() {
            value.rotate_left(rotation)
        } else {
            value.rotate_right(rotation)
        })
    }

    /// Reads a word using User-mode MMU permissions without changing CPSR mode.
    pub(super) fn read_data_u32_unprivileged<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
    ) -> Result<u32> {
        let value = bus.read32_unprivileged(address)?;
        Ok(if self.cpsr.big_endian() {
            value.swap_bytes()
        } else {
            value
        })
    }

    /// Reads an unaligned word using User-mode MMU permissions.
    pub(super) fn read_data_u32_unaligned_unprivileged<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
    ) -> Result<u32> {
        let value = self.read_data_u32_unprivileged(bus, address & !3)?;
        let rotation = (address & 3) * 8;
        Ok(if self.cpsr.big_endian() {
            value.rotate_left(rotation)
        } else {
            value.rotate_right(rotation)
        })
    }

    /// Writes a word using User-mode MMU permissions without changing CPSR mode.
    pub(super) fn write_data_u32_unprivileged<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        value: u32,
    ) -> Result<()> {
        bus.write32_unprivileged(
            address,
            if self.cpsr.big_endian() {
                value.swap_bytes()
            } else {
                value
            },
        )
    }

    /// Writes a halfword using the current CPSR data endianness.
    pub(super) fn write_data_u16<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        value: u16,
    ) -> Result<()> {
        bus.write16(
            address,
            if self.cpsr.big_endian() {
                value.swap_bytes()
            } else {
                value
            },
        )
    }

    /// Writes a word using the current CPSR data endianness.
    pub(super) fn write_data_u32<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        value: u32,
    ) -> Result<()> {
        bus.write32(
            address,
            if self.cpsr.big_endian() {
                value.swap_bytes()
            } else {
                value
            },
        )
    }

    /// Writes register `n`; a write to `pc` is a branch (without interworking).
    fn write_reg(&mut self, n: u32, value: u32) {
        if n as usize == PC {
            self.branch(value & !3);
        } else {
            self.regs[n as usize] = value;
        }
    }

    /// Base-register writeback. Writing back to `pc` is unpredictable, so ignore it.
    fn set_base(&mut self, n: u32, value: u32) {
        if n as usize != PC {
            self.regs[n as usize] = value;
        }
    }

    pub(super) fn branch(&mut self, target: u32) {
        self.regs[PC] = target;
        self.branch_taken = true;
    }

    /// Branch that selects ARM or Thumb state from bit 0 of `target`.
    pub(super) fn branch_exchange(&mut self, target: u32) {
        if target & 1 != 0 {
            self.cpsr.set_thumb(true);
            self.branch(target & !1);
        } else {
            self.cpsr.set_thumb(false);
            self.branch(target & !3);
        }
    }

    fn set_nz(&mut self, value: u32) {
        let (c, v) = (self.cpsr.c(), self.cpsr.v());
        self.cpsr.set_flags(value >> 31 != 0, value == 0, c, v);
    }

    // ----- unconditional space ---------------------------------------------

    fn execute_unconditional<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        // CLREX is an unconditional A32 ARMv6K instruction.
        if word == 0xF57F_F01F {
            self.clear_exclusive_monitor();
            return Ok(StepEvent::Executed);
        }
        // SETEND modifies only CPSR.E; instruction fetch remains little-endian.
        if word & 0xFFFF_FDFF == 0xF101_0000 {
            self.cpsr.set_big_endian(bit(word, 9));
            return Ok(StepEvent::Executed);
        }
        // PLD is an architecturally optional data-prefetch hint and has no
        // visible effect on this interpreter's functional memory model.
        // Its register offset (when present) must not be PC; PC-relative
        // immediate literals remain valid.
        if word & 0xFD70_F000 == 0xF550_F000 {
            if bit(word, 25) && bits(word, 3, 0) as usize == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            return Ok(StepEvent::Executed);
        }
        // CPS switches the active mode bank and/or updates selected A/I/F masks.
        if word & 0xFFF0_FE20 == 0xF100_0000 {
            let imod = bits(word, 19, 18);
            let change_mode = bit(word, 17);
            let interrupt_mask = u8::from(bit(word, 8))
                | (u8::from(bit(word, 7)) << 1)
                | (u8::from(bit(word, 6)) << 2);
            let mode = bits(word, 4, 0);
            let valid_mode = super::Psr::is_valid_mode(mode);
            let valid_encoding = if imod == 0 {
                change_mode && interrupt_mask == 0 && valid_mode
            } else {
                matches!(imod, 0b10 | 0b11)
                    && interrupt_mask != 0
                    && (!change_mode || valid_mode)
                    && (change_mode || mode == 0)
            };
            if !valid_encoding {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            // CPS is architecturally a NOP in User mode.
            if !self.cpsr.privileged() {
                return Ok(StepEvent::Executed);
            }
            if change_mode {
                self.switch_mode(mode)?;
            }
            if imod != 0 {
                self.cpsr.set_interrupt_masks(interrupt_mask, imod == 0b11);
            }
            return Ok(StepEvent::Executed);
        }
        // SRS stores the current mode's LR/SPSR on a selected banked SP.
        // It is unconditional in A32 and was introduced by ARMv6.
        if word & 0xFE4F_FFE0 == 0xF84D_0500 {
            return self.store_return_state(bus, word);
        }
        // RFE loads the target PC and CPSR from two consecutive words. It is
        // an ARMv6 privileged instruction and exists only in A32.
        if word & 0xFE50_FFFF == 0xF810_0A00 {
            return self.return_from_exception_memory(bus, word);
        }
        let unconditional_class = bits(word, 27, 25);
        if matches!(unconditional_class, 0b110 | 0b111) {
            // ARMv6 reserves the unconditional CP15 instruction forms:
            // MCR2/MRC2, CDP2, LDC2/STC2 and MCRR2/MRRC2 are Undefined for
            // CP15. Other coprocessors may define these encodings; they are
            // kept as explicit emulator limits until that coprocessor exists.
            if bits(word, 11, 8) == 15 {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            return Err(Error::Unimplemented(
                "unconditional coprocessor instruction",
            ));
        }
        if unconditional_class != 0b101 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        // BLX (immediate): always switches to Thumb state.
        let pc = self.pc();
        let offset = ((word & 0x00FF_FFFF) << 8) as i32 >> 6;
        let half = bits(word, 24, 24) << 1;
        let target = pc
            .wrapping_add(8)
            .wrapping_add(offset as u32)
            .wrapping_add(half);
        self.regs[LR] = pc.wrapping_add(4);
        self.cpsr.set_thumb(true);
        self.branch(target & !1);
        Ok(StepEvent::Executed)
    }

    // ----- branches ---------------------------------------------------------

    fn branch_with_link(&mut self, word: u32) {
        let pc = self.pc();
        let offset = (((word & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
        if bit(word, 24) {
            self.regs[LR] = pc.wrapping_add(4);
        }
        self.branch(pc.wrapping_add(8).wrapping_add(offset));
    }

    // ----- data processing, multiplies, misc -------------------------------

    /// Bits 27:25 are `00x`: data processing, multiplies, swaps, halfword
    /// transfers and the miscellaneous instructions all share this space.
    fn execute_data_space<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        if !bit(word, 25) {
            // Signed halfword and word-by-halfword multiply forms.
            if matches!(
                word & 0x0FF0_0090,
                0x0100_0080 | 0x0120_0080 | 0x0140_0080 | 0x0160_0080
            ) {
                return self.execute_halfword_multiply_instruction(word);
            }
            if bits(word, 7, 4) == 0b1001 {
                return self.multiply_or_swap(bus, word);
            }
            if bit(word, 7) && bit(word, 4) {
                return self.halfword_transfer(bus, word);
            }
        }

        let opcode = bits(word, 24, 21);
        if (0x8..=0xB).contains(&opcode) && !bit(word, 20) {
            return self.misc_instruction(word);
        }
        self.data_processing(word)
    }

    /// Evaluates the shifter operand: `(value, carry_out, pc_offset)`, where
    /// `pc_offset` is what reading `pc` as another operand should add.
    fn shifter_operand(&self, word: u32) -> (u32, bool, u32) {
        let carry_in = self.cpsr.c();

        if bit(word, 25) {
            let rotation = bits(word, 11, 8) * 2;
            let value = bits(word, 7, 0).rotate_right(rotation);
            let carry = if rotation == 0 {
                carry_in
            } else {
                value >> 31 != 0
            };
            return (value, carry, 8);
        }

        let kind = ShiftKind::from_bits(bits(word, 6, 5));
        let rm = bits(word, 3, 0);
        if bit(word, 4) {
            let amount = self.reg(bits(word, 11, 8), 12) & 0xFF;
            let (value, carry) = shift_reg(kind, self.reg(rm, 12), amount, carry_in);
            (value, carry, 12)
        } else {
            let (value, carry) = shift_imm(kind, self.reg(rm, 8), bits(word, 11, 7), carry_in);
            (value, carry, 8)
        }
    }

    fn data_processing(&mut self, word: u32) -> Result<StepEvent> {
        let opcode = bits(word, 24, 21);
        let set_flags = bit(word, 20);
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);
        let carry_in = self.cpsr.c();
        let is_compare = (0x8..=0xB).contains(&opcode);

        // TST, TEQ, CMP and CMN encode Rd as SBZ. Reject malformed
        // encodings instead of silently executing them as comparisons.
        if is_compare && rd != 0 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        // ARMv6 makes every use of R15 in a register-controlled data
        // processing shift UNPREDICTABLE, including the otherwise-unused Rn
        // field of MOV/MVN. Reject all four fields before reading operands.
        if !bit(word, 25) && bit(word, 4) {
            let rs = bits(word, 11, 8);
            let rm = bits(word, 3, 0);
            if rn == PC as u32 || rd == PC as u32 || rs == PC as u32 || rm == PC as u32 {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
        }

        let (operand, shifter_carry, pc_offset) = self.shifter_operand(word);
        let a = self.reg(rn, pc_offset);

        let outcome = match opcode {
            0x0 | 0x8 => Alu::Logical(a & operand),
            0x1 | 0x9 => Alu::Logical(a ^ operand),
            0x2 | 0xA => arithmetic(a, !operand, true),
            0x3 => arithmetic(operand, !a, true),
            0x4 | 0xB => arithmetic(a, operand, false),
            0x5 => arithmetic(a, operand, carry_in),
            0x6 => arithmetic(a, !operand, carry_in),
            0x7 => arithmetic(operand, !a, carry_in),
            0xC => Alu::Logical(a | operand),
            0xD => Alu::Logical(operand),
            0xE => Alu::Logical(a & !operand),
            _ => Alu::Logical(!operand),
        };

        let result = match outcome {
            Alu::Logical(value) | Alu::Arithmetic { value, .. } => value,
        };
        // S + Rd == PC performs an exception return: restore CPSR from the
        // active mode's SPSR, then branch using the restored ARM/Thumb state.
        if set_flags && !is_compare && rd as usize == PC {
            self.exception_return(word, result)?;
            return Ok(StepEvent::Executed);
        }

        if set_flags && (is_compare || rd as usize != PC) {
            match outcome {
                Alu::Logical(value) => {
                    let overflow = self.cpsr.v();
                    self.cpsr
                        .set_flags(value >> 31 != 0, value == 0, shifter_carry, overflow);
                }
                Alu::Arithmetic {
                    value,
                    carry,
                    overflow,
                } => self
                    .cpsr
                    .set_flags(value >> 31 != 0, value == 0, carry, overflow),
            }
        }

        if !is_compare {
            self.write_reg(rd, result);
        }
        Ok(StepEvent::Executed)
    }

    /// Restores the active exception mode's SPSR and branches to an
    /// instruction address aligned for the restored ARM or Thumb state.
    fn exception_return(&mut self, word: u32, target: u32) -> Result<()> {
        let current_mode = self.cpsr.mode();
        if !super::Psr::mode_has_spsr(current_mode) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let restored = self
            .saved_program_status()
            .ok_or(Error::Unimplemented("SPSR has not been initialized"))?;
        self.restore_status_and_branch(word, target, restored)
    }

    /// Shared architectural state restoration used by MOVS-to-PC, LDM exception
    /// returns, and RFE. A latched notification lets the MPCore scheduler wake
    /// the other CPUs without changing the direct CPU-step result.
    fn restore_status_and_branch(
        &mut self,
        word: u32,
        target: u32,
        restored: super::Psr,
    ) -> Result<()> {
        if !super::Psr::is_valid_mode(restored.mode()) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        // Capture the target before switching banks; the old mode's SP/LR
        // are saved by switch_mode and the restored mode's registers loaded.
        self.switch_mode(restored.mode())?;
        self.cpsr = restored;
        let aligned_target = if restored.thumb() {
            target & !1
        } else {
            target & !3
        };
        self.branch(aligned_target);
        self.exception_return_wakeup = true;
        Ok(())
    }

    /// Reads the SP belonging to a processor mode without changing the active bank.
    fn banked_sp(&self, mode: u32) -> Option<u32> {
        let current = self.cpsr.mode();
        let shares_user_bank = matches!(current, super::Psr::USER_MODE | super::Psr::SYSTEM_MODE)
            && matches!(mode, super::Psr::USER_MODE | super::Psr::SYSTEM_MODE);
        if current == mode || shares_user_bank {
            return Some(self.regs[super::SP]);
        }

        match mode {
            super::Psr::USER_MODE | super::Psr::SYSTEM_MODE => Some(self.banked.user_sp_lr[0]),
            super::Psr::FIQ_MODE => Some(self.banked.fiq_r8_r14[5]),
            super::Psr::IRQ_MODE => Some(self.banked.irq_sp_lr[0]),
            super::Psr::SUPERVISOR_MODE => Some(self.banked.svc_sp_lr[0]),
            super::Psr::ABORT_MODE => Some(self.banked.abort_sp_lr[0]),
            super::Psr::UNDEFINED_MODE => Some(self.banked.undefined_sp_lr[0]),
            _ => None,
        }
    }

    /// Updates the SP belonging to a processor mode without changing the active bank.
    fn set_banked_sp(&mut self, mode: u32, value: u32) -> bool {
        let current = self.cpsr.mode();
        let shares_user_bank = matches!(current, super::Psr::USER_MODE | super::Psr::SYSTEM_MODE)
            && matches!(mode, super::Psr::USER_MODE | super::Psr::SYSTEM_MODE);
        if current == mode || shares_user_bank {
            self.regs[super::SP] = value;
            return true;
        }

        match mode {
            super::Psr::USER_MODE | super::Psr::SYSTEM_MODE => {
                self.banked.user_sp_lr[0] = value;
            }
            super::Psr::FIQ_MODE => self.banked.fiq_r8_r14[5] = value,
            super::Psr::IRQ_MODE => self.banked.irq_sp_lr[0] = value,
            super::Psr::SUPERVISOR_MODE => self.banked.svc_sp_lr[0] = value,
            super::Psr::ABORT_MODE => self.banked.abort_sp_lr[0] = value,
            super::Psr::UNDEFINED_MODE => self.banked.undefined_sp_lr[0] = value,
            _ => return false,
        }
        true
    }

    /// Executes ARMv6 SRS using the banked SP selected by the instruction.
    fn store_return_state<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        let current_mode = self.cpsr.mode();
        if !super::Psr::mode_has_spsr(current_mode) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }
        // Validate the target mode before consulting the source SPSR, so
        // architecturally undefined target encodings are never masked by
        // the emulator's uninitialized-SPSR diagnostic.
        let target_mode = bits(word, 4, 0);
        if !super::Psr::is_valid_mode(target_mode) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let saved_status = self.saved_program_status().ok_or(Error::Unimplemented(
            "SRS source SPSR has not been initialized",
        ))?;
        let raw_base = self
            .banked_sp(target_mode)
            .ok_or(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            })?;
        let unaligned = raw_base & 3 != 0;
        if unaligned && self.cp15.alignment_fault_enabled() {
            return Err(Error::InvalidAccess { addr: raw_base });
        }
        let base = if unaligned { raw_base & !3 } else { raw_base };

        let pre_index = bit(word, 24);
        let increment = bit(word, 23);
        let writeback = bit(word, 21);

        // SRS transfers exactly two words: LR_current followed by SPSR_current.
        // The start address follows the four LDM/STM addressing-mode conventions.
        let mut address = if increment {
            base
        } else {
            base.wrapping_sub(8)
        };
        if pre_index == increment {
            address = address.wrapping_add(4);
        }

        if unaligned {
            self.require_normal_unaligned_range(
                bus,
                address,
                8,
                true,
                self.cpsr.privileged(),
                word,
            )?;
        }

        let (link_value, status_value) = if self.cpsr.big_endian() {
            (self.regs[LR].swap_bytes(), saved_status.0.swap_bytes())
        } else {
            (self.regs[LR], saved_status.0)
        };
        bus.write32(address, link_value)?;
        bus.write32(address.wrapping_add(4), status_value)?;

        if writeback {
            let updated = if increment {
                raw_base.wrapping_add(8)
            } else {
                raw_base.wrapping_sub(8)
            };
            if !self.set_banked_sp(target_mode, updated) {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
        }
        Ok(StepEvent::Executed)
    }

    /// Executes the A32 Return From Exception instruction.
    fn return_from_exception_memory<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        if !self.cpsr.privileged() {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let base_register = bits(word, 19, 16) as usize;
        if base_register == PC {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let raw_base = self.regs[base_register];
        let unaligned = raw_base & 3 != 0;
        if unaligned && self.cp15.alignment_fault_enabled() {
            return Err(Error::InvalidAccess { addr: raw_base });
        }
        let base = if unaligned { raw_base & !3 } else { raw_base };

        let pre_index = bit(word, 24);
        let increment = bit(word, 23);
        let writeback = bit(word, 21);

        // RFE transfers PC first and CPSR second. The four addressing modes
        // follow the same before/after and increment/decrement convention as
        // the ARM multiple-transfer instructions.
        let mut address = if increment {
            base
        } else {
            base.wrapping_sub(8)
        };
        if pre_index == increment {
            address = address.wrapping_add(4);
        }

        if unaligned {
            self.require_normal_unaligned_range(
                bus,
                address,
                8,
                false,
                self.cpsr.privileged(),
                word,
            )?;
        }

        let mut target = bus.read32(address)?;
        let mut status = bus.read32(address.wrapping_add(4))?;
        if self.cpsr.big_endian() {
            target = target.swap_bytes();
            status = status.swap_bytes();
        }
        let status = super::Psr(status);
        if !super::Psr::is_valid_mode(status.mode()) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        // Write back while the old exception bank is still active. This is
        // essential when Rn is SP and the return restores User/System mode.
        if writeback {
            self.regs[base_register] = if increment {
                raw_base.wrapping_add(8)
            } else {
                raw_base.wrapping_sub(8)
            };
        }

        self.restore_status_and_branch(word, target, status)?;
        Ok(StepEvent::Executed)
    }

    fn multiply_or_swap<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        // All A32 LDREX/STREX size variants share bits[11:4] = 0xF9.
        if bits(word, 11, 4) == 0xF9 {
            return self.execute_exclusive_instruction(bus, word);
        }
        // UMAAL is the ARMv6 unsigned multiply-accumulate-accumulate long form.
        if bits(word, 27, 20) == 0x04 {
            return self.unsigned_multiply_accumulate_accumulate_long(word);
        }
        if bits(word, 27, 22) == 0 {
            self.multiply(word)?;
        } else if bits(word, 27, 23) == 0b00001 {
            self.multiply_long(word)?;
        } else if bits(word, 27, 23) == 0b00010 && bits(word, 21, 20) == 0 {
            self.swap(bus, word)?;
        } else {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }
        Ok(StepEvent::Executed)
    }

    fn multiply(&mut self, word: u32) -> Result<()> {
        let rd = bits(word, 19, 16);
        let rn = bits(word, 15, 12);
        let rs = bits(word, 11, 8);
        let rm = bits(word, 3, 0);
        let accumulate = bit(word, 21);

        // MUL reserves Rn as SBZ. MLA uses Rn as its accumulator.
        // Both forms prohibit R15 in every architecturally used register.
        if rd as usize == PC
            || rs as usize == PC
            || rm as usize == PC
            || (accumulate && rn as usize == PC)
            || (!accumulate && rn != 0)
        {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        // Read every source before writeback; ARMv6 permits Rd == Rm.
        let mut result = self.reg(rm, 8).wrapping_mul(self.reg(rs, 8));
        if accumulate {
            result = result.wrapping_add(self.reg(rn, 8));
        }
        self.write_reg(rd, result);
        if bit(word, 20) {
            self.set_nz(result);
        }
        Ok(())
    }

    fn multiply_long(&mut self, word: u32) -> Result<()> {
        let signed = bit(word, 22);
        let accumulate = bit(word, 21);
        let rd_hi = bits(word, 19, 16);
        let rd_lo = bits(word, 15, 12);
        let rs = bits(word, 11, 8);
        let rm = bits(word, 3, 0);

        // R15 is not valid in any long-multiply register, and the two
        // destination registers must be distinct. Source/destination
        // overlap is permitted in ARMv6 because operands are read first.
        if rd_hi as usize == PC
            || rd_lo as usize == PC
            || rs as usize == PC
            || rm as usize == PC
            || rd_hi == rd_lo
        {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let (a, b) = (self.reg(rm, 8), self.reg(rs, 8));
        let mut result = if signed {
            (i64::from(a as i32) * i64::from(b as i32)) as u64
        } else {
            u64::from(a) * u64::from(b)
        };
        if accumulate {
            let current = (u64::from(self.reg(rd_hi, 8)) << 32) | u64::from(self.reg(rd_lo, 8));
            result = result.wrapping_add(current);
        }

        self.write_reg(rd_lo, result as u32);
        self.write_reg(rd_hi, (result >> 32) as u32);
        if bit(word, 20) {
            let (c, v) = (self.cpsr.c(), self.cpsr.v());
            self.cpsr.set_flags(result >> 63 != 0, result == 0, c, v);
        }
        Ok(())
    }

    /// Executes UMAAL, which adds two independent 32-bit accumulators
    /// to an unsigned 32x32-bit product and writes the 64-bit result.
    fn unsigned_multiply_accumulate_accumulate_long(&mut self, word: u32) -> Result<StepEvent> {
        let rd_hi = bits(word, 19, 16) as usize;
        let rd_lo = bits(word, 15, 12) as usize;
        let rs = bits(word, 11, 8) as usize;
        let rm = bits(word, 3, 0) as usize;
        if rd_hi == PC || rd_lo == PC || rs == PC || rm == PC || rd_hi == rd_lo {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        // Read every source before writing either destination; ARMv6 permits
        // source/destination aliasing apart from RdHi == RdLo.
        let result = u64::from(self.regs[rm]) * u64::from(self.regs[rs])
            + u64::from(self.regs[rd_lo])
            + u64::from(self.regs[rd_hi]);
        self.regs[rd_lo] = result as u32;
        self.regs[rd_hi] = (result >> 32) as u32;
        Ok(StepEvent::Executed)
    }

    /// Executes A32 exclusive loads and stores.
    ///
    /// LDREX/STREX word are part of ARMv6. The byte, halfword and doubleword
    /// forms are ARMv6K extensions; they share the same local-monitor contract.
    fn execute_exclusive_instruction<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        let operation = bits(word, 27, 20);
        let (load, size) = match operation {
            0x19 => (true, 4),  // LDREX
            0x18 => (false, 4), // STREX
            0x1D => (true, 1),  // LDREXB
            0x1C => (false, 1), // STREXB
            0x1F => (true, 2),  // LDREXH
            0x1E => (false, 2), // STREXH
            0x1B => (true, 8),  // LDREXD
            0x1A => (false, 8), // STREXD
            _ => {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
        };

        let rn = bits(word, 19, 16) as usize;
        if rn == PC {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let address = self.regs[rn];
        if address as usize % size != 0 {
            return Err(Error::InvalidAccess { addr: address });
        }

        if load {
            let rt = bits(word, 15, 12) as usize;
            if rt == PC || (size == 8 && (rt % 2 != 0 || rt == LR || rt + 1 >= PC)) {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }

            // A new exclusive load replaces the previous local reservation.
            self.exclusive_reservation = None;
            let (raw_value, version) = bus.exclusive_read(address, size)?;
            match size {
                1 => self.regs[rt] = raw_value as u32,
                2 => {
                    let value = raw_value as u16;
                    self.regs[rt] = u32::from(if self.cpsr.big_endian() {
                        value.swap_bytes()
                    } else {
                        value
                    });
                }
                4 => {
                    let value = raw_value as u32;
                    self.regs[rt] = if self.cpsr.big_endian() {
                        value.swap_bytes()
                    } else {
                        value
                    };
                }
                8 => {
                    let first = raw_value as u32;
                    let second = (raw_value >> 32) as u32;
                    if self.cpsr.big_endian() {
                        self.regs[rt] = first.swap_bytes();
                        self.regs[rt + 1] = second.swap_bytes();
                    } else {
                        self.regs[rt] = first;
                        self.regs[rt + 1] = second;
                    }
                }
                _ => unreachable!(),
            }

            self.exclusive_reservation = Some(super::ExclusiveReservation {
                address,
                size: size as u8,
                version,
            });
        } else {
            let rd = bits(word, 15, 12) as usize;
            let rt = bits(word, 3, 0) as usize;
            let second_rt = rt + 1;

            if rd == PC
                || rt == PC
                || rd == rn
                || rd == rt
                || (size == 8 && (rt % 2 != 0 || rt == LR || second_rt >= PC || rd == second_rt))
            {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }

            let stored_value = match size {
                1 => u64::from((self.regs[rt] & 0xFF) as u8),
                2 => {
                    let value = self.regs[rt] as u16;
                    u64::from(if self.cpsr.big_endian() {
                        value.swap_bytes()
                    } else {
                        value
                    })
                }
                4 => {
                    let value = self.regs[rt];
                    u64::from(if self.cpsr.big_endian() {
                        value.swap_bytes()
                    } else {
                        value
                    })
                }
                8 => {
                    let first = self.regs[rt];
                    let second = self.regs[second_rt];
                    if self.cpsr.big_endian() {
                        u64::from(first.swap_bytes()) | (u64::from(second.swap_bytes()) << 32)
                    } else {
                        u64::from(first) | (u64::from(second) << 32)
                    }
                }
                _ => unreachable!(),
            };

            // STREX always consumes the local reservation, including on failure.
            let reservation = self.exclusive_reservation.take().and_then(|saved| {
                (saved.address == address && usize::from(saved.size) == size)
                    .then_some(saved.version)
            });
            let succeeded = bus.exclusive_write(address, size, stored_value, reservation)?;
            self.regs[rd] = u32::from(!succeeded);
        }

        Ok(StepEvent::Executed)
    }

    fn swap<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<()> {
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);
        let rm = bits(word, 3, 0);
        let byte = bit(word, 22);

        // SWP/SWPB do not permit PC in any operand. The base register
        // must also differ from both transfer registers; Rd == Rm is valid.
        // ARM11 MPCore SCTLR[10] is Reserved/SBZ (DDI 0360F), so this
        // ARMv6K profile does not use the later ARMv7 SCTLR.SW enable gate.
        if rn as usize == PC || rd as usize == PC || rm as usize == PC || rn == rd || rn == rm {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let addr = self.reg(rn, 8);
        let new_value = self.reg(rm, 8);

        // SWPB is a byte access. SWP is a WLoad+WStore in legacy mode
        // (U=0), but becomes a word-sized synchronization access when U=1.
        if !byte && addr & 3 != 0 {
            let sctlr = self.cp15.sctlr();
            if sctlr & ((1 << 22) | (1 << 1)) != 0 {
                return Err(Error::InvalidAccess { addr });
            }
            let aligned = addr & !3;
            self.require_normal_unaligned_range(
                bus,
                aligned,
                4,
                false,
                self.cpsr.privileged(),
                word,
            )?;
            self.require_normal_unaligned_range(
                bus,
                aligned,
                4,
                true,
                self.cpsr.privileged(),
                word,
            )?;
        }

        let old = if byte {
            let old = u32::from(bus.read8(addr)?);
            bus.write8(addr, new_value as u8)?;
            old
        } else {
            let old = self.read_data_u32_unaligned(bus, addr)?;
            self.write_data_u32(bus, addr & !3, new_value)?;
            old
        };
        self.write_reg(rd, old);
        Ok(())
    }

    /// Instructions encoded in the otherwise-unused compare opcodes with `S=0`.
    fn misc_instruction(&mut self, word: u32) -> Result<StepEvent> {
        let rm = bits(word, 3, 0);
        let rd = bits(word, 15, 12);

        if word & 0x0F90_0FF0 == 0x0100_0050 {
            // QADD/QSUB/QDADD/QDSUB use signed 32-bit saturation. QD* first
            // saturates the doubled Rn operand, then saturates the final sum
            // or difference. The CPSR.Q flag is sticky across instructions.
            let rd_index = rd as usize;
            let rm_index = rm as usize;
            let rn = bits(word, 19, 16) as usize;
            if rd_index == PC || rn == PC || rm_index == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }

            let first = i64::from(self.regs[rm_index] as i32);
            let second = i64::from(self.regs[rn] as i32);
            let (result, saturated) = match bits(word, 22, 21) {
                0b00 => signed_saturate_32(first + second), // QADD
                0b01 => signed_saturate_32(first - second), // QSUB
                0b10 => {
                    let (doubled, double_saturated) = signed_saturate_32(second * 2);
                    let (result, add_saturated) = signed_saturate_32(first + i64::from(doubled));
                    (result, double_saturated || add_saturated)
                }
                0b11 => {
                    let (doubled, double_saturated) = signed_saturate_32(second * 2);
                    let (result, subtract_saturated) =
                        signed_saturate_32(first - i64::from(doubled));
                    (result, double_saturated || subtract_saturated)
                }
                _ => unreachable!(),
            };

            self.regs[rd_index] = result as u32;
            if saturated {
                self.cpsr.set_q();
            }
        } else if word & 0x0FF0_00F0 == 0x0120_0070 {
            // BKPT #imm16; exception-vector entry is not modeled yet.
            let immediate = ((bits(word, 19, 8) << 4) | bits(word, 3, 0)) as u16;
            return Ok(StepEvent::Breakpoint(immediate));
        } else if word & 0x0FFF_FFF0 == 0x012F_FF10 {
            // BX Rm
            let target = self.reg(rm, 8);
            self.branch_exchange(target);
        } else if word & 0x0FFF_FFF0 == 0x012F_FF20 {
            // This profile models the Jazelle extension as permanently disabled
            // (the ARMv6 trivial implementation), so BXJ behaves exactly as BX.
            let target = self.reg(rm, 8);
            self.branch_exchange(target);
        } else if word & 0x0FFF_FFF0 == 0x012F_FF30 {
            // BLX Rm does not permit PC as its source register in ARMv6.
            if rm as usize == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let target = self.reg(rm, 8);
            self.regs[LR] = self.pc().wrapping_add(4);
            self.branch_exchange(target);
        } else if word & 0x0FFF_0FF0 == 0x016F_0F10 {
            // CLZ Rd, Rm does not permit PC as either operand.
            if rd as usize == PC || rm as usize == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let value = self.reg(rm, 8).leading_zeros();
            self.write_reg(rd, value);
        } else if word & 0x0FBF_0FFF == 0x010F_0000 {
            // MRS Rd, CPSR|SPSR
            if rd as usize == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let value = if bit(word, 22) {
                if !super::Psr::mode_has_spsr(self.cpsr.mode()) {
                    return Err(Error::UndefinedInstruction {
                        pc: self.pc(),
                        word,
                    });
                }
                self.saved_program_status()
                    .ok_or(Error::Unimplemented("SPSR has not been initialized"))?
                    .0
            } else {
                self.cpsr.0
            };
            self.write_reg(rd, value);
        } else if word & 0x0FB0_FFF0 == 0x0120_F000 {
            // MSR CPSR|SPSR_<fields>, Rm. PC is not a permitted source register.
            if rm as usize == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let value = self.reg(rm, 8);
            self.write_status(word, value)?;
        } else if word & 0x0FB0_F000 == 0x0320_F000 {
            // MSR CPSR|SPSR_<fields>, #imm
            let value = bits(word, 7, 0).rotate_right(bits(word, 11, 8) * 2);
            self.write_status(word, value)?;
        } else {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }
        Ok(StepEvent::Executed)
    }

    /// Applies an MSR write to CPSR or the current exception mode's SPSR.
    /// User mode may only write the top (flags) byte of CPSR.
    fn write_status(&mut self, word: u32, value: u32) -> Result<()> {
        let fields = bits(word, 19, 16);
        let mut write_mask = 0;
        if fields & 0b1000 != 0 {
            write_mask |= 0xFF00_0000;
        }
        if fields & 0b0100 != 0 {
            write_mask |= 0x00FF_0000;
        }
        if fields & 0b0010 != 0 {
            write_mask |= 0x0000_FF00;
        }
        if fields & 0b0001 != 0 {
            write_mask |= 0x0000_00FF;
        }
        if write_mask == 0 {
            return Ok(());
        }

        if bit(word, 22) {
            if !super::Psr::mode_has_spsr(self.cpsr.mode()) {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let previous = match self.saved_program_status() {
                Some(previous) => previous.0,
                None if write_mask == u32::MAX => 0,
                None => {
                    return Err(Error::Unimplemented(
                        "partial SPSR write requires an initialized SPSR",
                    ));
                }
            };
            self.set_saved_program_status(super::Psr(
                (previous & !write_mask) | (value & write_mask),
            ))?;
            return Ok(());
        }

        // ARM1136 permits NZCVQ, GE and E writes from every mode.
        // A/I/F and the mode field remain privileged-only. J, T and
        // reserved CPSR bits are not writable through MSR on this core.
        let mut writable_cpsr_bits = super::Psr::N
            | super::Psr::Z
            | super::Psr::C
            | super::Psr::V
            | super::Psr::Q
            | super::Psr::GE_MASK
            | super::Psr::E;
        if self.cpsr.privileged() {
            writable_cpsr_bits |=
                super::Psr::A | super::Psr::I | super::Psr::F | super::Psr::MODE_MASK;
        }
        write_mask &= writable_cpsr_bits;
        if write_mask == 0 {
            return Ok(());
        }

        if write_mask & super::Psr::MODE_MASK != 0 {
            let mode = value & super::Psr::MODE_MASK;
            if !super::Psr::is_valid_mode(mode) {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            self.switch_mode(mode)?;
        }

        self.cpsr.0 = (self.cpsr.0 & !write_mask) | (value & write_mask);
        Ok(())
    }

    // ----- loads and stores -------------------------------------------------

    /// Rejects an unaligned access whose translated bytes are not all Normal memory.
    ///
    /// ARMv6 marks an otherwise-normal unaligned access to Device or
    /// Strongly-ordered memory as UNPREDICTABLE. Query every byte so an access
    /// crossing a page boundary cannot silently mix memory types.
    fn require_normal_unaligned_range<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        size: usize,
        write: bool,
        privileged: bool,
        word: u32,
    ) -> Result<()> {
        for offset in 0..size {
            if !bus.is_normal_memory(address.wrapping_add(offset as u32), write, privileged)? {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
        }
        Ok(())
    }

    /// Reads an A32 single-word transfer using the current ARMv6 U/A alignment model.
    ///
    /// With U=0,A=0 the legacy aligned-word rotation is retained. With U=1,A=0,
    /// an unaligned normal-memory access is assembled from the exact byte addresses
    /// in the current CPSR endianness. A=1 always faults on an unaligned word.
    pub(super) fn read_armv6_single_word<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        unprivileged: bool,
        word: u32,
    ) -> Result<u32> {
        let unaligned = address & 3 != 0;
        let sctlr = self.cp15.sctlr();
        let unaligned_support = sctlr & (1 << 22) != 0;
        let alignment_check = sctlr & (1 << 1) != 0;

        if unaligned && alignment_check {
            return Err(Error::InvalidAccess { addr: address });
        }

        if unaligned {
            let attribute_address = if unaligned_support {
                address
            } else {
                address & !3
            };
            self.require_normal_unaligned_range(
                bus,
                attribute_address,
                4,
                false,
                !unprivileged,
                word,
            )?;
        }

        if unaligned && unaligned_support {
            let mut bytes = [0u8; 4];
            for (offset, byte) in bytes.iter_mut().enumerate() {
                let current = address.wrapping_add(offset as u32);
                *byte = if unprivileged {
                    bus.read8_unprivileged(current)?
                } else {
                    bus.read8(current)?
                };
            }
            return Ok(if self.cpsr.big_endian() {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            });
        }

        if unprivileged {
            self.read_data_u32_unaligned_unprivileged(bus, address)
        } else {
            self.read_data_u32_unaligned(bus, address)
        }
    }

    /// Writes an A32 single-word transfer using the current ARMv6 U/A alignment model.
    pub(super) fn write_armv6_single_word<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        value: u32,
        unprivileged: bool,
        word: u32,
    ) -> Result<()> {
        let unaligned = address & 3 != 0;
        let sctlr = self.cp15.sctlr();
        let unaligned_support = sctlr & (1 << 22) != 0;
        let alignment_check = sctlr & (1 << 1) != 0;

        if unaligned && alignment_check {
            return Err(Error::InvalidAccess { addr: address });
        }

        if unaligned {
            let attribute_address = if unaligned_support {
                address
            } else {
                address & !3
            };
            self.require_normal_unaligned_range(
                bus,
                attribute_address,
                4,
                true,
                !unprivileged,
                word,
            )?;
        }

        if unaligned && unaligned_support {
            let bytes = if self.cpsr.big_endian() {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            };
            for (offset, byte) in bytes.into_iter().enumerate() {
                let current = address.wrapping_add(offset as u32);
                if unprivileged {
                    bus.write8_unprivileged(current, byte)?;
                } else {
                    bus.write8(current, byte)?;
                }
            }
            return Ok(());
        }

        let aligned_address = if unaligned { address & !3 } else { address };
        if unprivileged {
            self.write_data_u32_unprivileged(bus, aligned_address, value)
        } else {
            self.write_data_u32(bus, aligned_address, value)
        }
    }

    /// Reads an ARMv6 halfword using SCTLR.U/A and the current memory attributes.
    ///
    /// U=1 allows the exact unaligned halfword. U=0 with A=0 is architecturally
    /// UNPREDICTABLE, so the interpreter rejects it deterministically. A=1
    /// always reports an alignment fault for an odd address.
    pub(super) fn read_armv6_halfword<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        instruction: u32,
    ) -> Result<u16> {
        if address & 1 != 0 {
            let sctlr = self.cp15.sctlr();
            if sctlr & (1 << 1) != 0 {
                return Err(Error::InvalidAccess { addr: address });
            }
            if sctlr & (1 << 22) == 0 {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word: instruction,
                });
            }
            self.require_normal_unaligned_range(
                bus,
                address,
                2,
                false,
                self.cpsr.privileged(),
                instruction,
            )?;
        }
        self.read_data_u16(bus, address)
    }

    /// Writes an ARMv6 halfword using SCTLR.U/A and the current memory attributes.
    pub(super) fn write_armv6_halfword<B: Bus + ?Sized>(
        &self,
        bus: &mut B,
        address: u32,
        value: u16,
        instruction: u32,
    ) -> Result<()> {
        if address & 1 != 0 {
            let sctlr = self.cp15.sctlr();
            if sctlr & (1 << 1) != 0 {
                return Err(Error::InvalidAccess { addr: address });
            }
            if sctlr & (1 << 22) == 0 {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word: instruction,
                });
            }
            self.require_normal_unaligned_range(
                bus,
                address,
                2,
                true,
                self.cpsr.privileged(),
                instruction,
            )?;
        }
        self.write_data_u16(bus, address, value)
    }

    fn single_transfer<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        let pre_index = bit(word, 24);
        let up = bit(word, 23);
        let byte = bit(word, 22);
        let writeback = !pre_index || bit(word, 21);
        let load = bit(word, 20);
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);
        // ARMv6 LDRT/LDRBT/STRT/STRBT use P=0,W=1. The address is
        // post-indexed; only the memory transaction uses User permissions.
        let unprivileged_access = !pre_index && bit(word, 21);

        // R15 is never a register offset (Rm). A writeback form cannot
        // use PC as its base or use the same register for base and transfer.
        // Byte transfers also cannot use PC as Rt; PC is supported only by
        // word LDR/STR forms. Keep PC-relative, no-writeback forms available.
        let register_offset = bit(word, 25);
        let rm = bits(word, 3, 0);
        let invalid_operands = (writeback && (rn as usize == PC || rn == rd))
            || (register_offset && rm as usize == PC)
            || (byte && rd as usize == PC)
            || (unprivileged_access && load && rd as usize == PC);
        if invalid_operands {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let offset = if register_offset {
            let kind = ShiftKind::from_bits(bits(word, 6, 5));
            let rm = self.reg(bits(word, 3, 0), 8);
            shift_imm(kind, rm, bits(word, 11, 7), self.cpsr.c()).0
        } else {
            bits(word, 11, 0)
        };

        let base = self.reg(rn, 8);
        let offset_addr = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let addr = if pre_index { offset_addr } else { base };

        // An unaligned word load into PC is UNPREDICTABLE when it would
        // otherwise complete normally. Alignment checking remains a fault.
        if load && !byte && rd as usize == PC && addr & 3 != 0 && self.cp15.sctlr() & (1 << 1) == 0
        {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        if load {
            let value = if byte {
                if unprivileged_access {
                    u32::from(bus.read8_unprivileged(addr)?)
                } else {
                    u32::from(bus.read8(addr)?)
                }
            } else {
                self.read_armv6_single_word(bus, addr, unprivileged_access, word)?
            };
            if rd as usize == PC && (byte || value & 3 == 2) {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            if writeback {
                self.set_base(rn, offset_addr);
            }
            // A loaded value wins over the writeback when `rd == rn`.
            if rd as usize == PC {
                self.branch_exchange(value);
            } else {
                self.regs[rd as usize] = value;
            }
        } else {
            let value = self.reg(rd, 8);
            if byte {
                if unprivileged_access {
                    bus.write8_unprivileged(addr, value as u8)?;
                } else {
                    bus.write8(addr, value as u8)?;
                }
            } else {
                self.write_armv6_single_word(bus, addr, value, unprivileged_access, word)?;
            }
            if writeback {
                self.set_base(rn, offset_addr);
            }
        }
        Ok(StepEvent::Executed)
    }

    /// LDRH/STRH/LDRSB/LDRSH and LDRD/STRD.
    fn halfword_transfer<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        let pre_index = bit(word, 24);
        let up = bit(word, 23);
        let writeback = !pre_index || bit(word, 21);
        let load = bit(word, 20);
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);

        let offset = if bit(word, 22) {
            (bits(word, 11, 8) << 4) | bits(word, 3, 0)
        } else {
            self.reg(bits(word, 3, 0), 8)
        };

        let base = self.reg(rn, 8);
        let offset_addr = if up {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let addr = if pre_index { offset_addr } else { base };

        let undefined = Error::UndefinedInstruction {
            pc: self.pc(),
            word,
        };

        // PC is not a valid Rt/Rm in halfword or signed-byte transfers.
        // PC-relative immediate forms remain available without writeback.
        let register_offset = !bit(word, 22);
        let rm = bits(word, 3, 0);
        let pair_transfer = matches!((load, bits(word, 6, 5)), (false, 0b10 | 0b11));
        let doubleword_load = pair_transfer && bits(word, 6, 5) == 0b10;
        let pair_second = rd + 1;
        // P=0,W=1 is CONSTRAINED UNPREDICTABLE for LDRD/STRD in ARMv6.
        // This core selects the deterministic UndefinedInstruction outcome.
        let invalid_pair = pair_transfer
            && (rd % 2 != 0
                || rd >= 14
                || (!pre_index && bit(word, 21))
                || (writeback && rn == pair_second)
                || (register_offset && doubleword_load && (rm == rd || rm == pair_second)));
        // PC-relative literal forms are available only for loads with
        // immediate offsets (including LDRD). PC is not a general halfword
        // transfer base and cannot be used for stores or register offsets.
        let pc_literal_allowed = !register_offset && !writeback && (load || doubleword_load);
        let invalid_operands = rd as usize == PC
            || (rn as usize == PC && !pc_literal_allowed)
            || (writeback && rn == rd)
            || (register_offset && rm as usize == PC)
            || invalid_pair;
        if invalid_operands {
            return Err(undefined);
        }

        // LDRD/STRD are not covered by U=1 unaligned single-word support.
        // They require word alignment with U=1, but doubleword alignment in
        // the legacy U=0 configuration.
        if pair_transfer {
            let sctlr = self.cp15.sctlr();
            let unaligned_support = sctlr & (1 << 22) != 0;
            let required_alignment_mask = if unaligned_support { 3 } else { 7 };
            if addr & required_alignment_mask != 0 {
                if unaligned_support || sctlr & (1 << 1) != 0 {
                    return Err(Error::InvalidAccess { addr });
                }
                return Err(undefined);
            }
        }

        // Single-halfword accesses use the common ARMv6 U/A path shared with Thumb.

        match (load, bits(word, 6, 5)) {
            (true, 0b01) => {
                let value = u32::from(self.read_armv6_halfword(bus, addr, word)?);
                self.finish_load(rn, rd, writeback, offset_addr, value);
            }
            (true, 0b10) => {
                let value = i32::from(bus.read8(addr)? as i8) as u32;
                self.finish_load(rn, rd, writeback, offset_addr, value);
            }
            (true, 0b11) => {
                let value = i32::from(self.read_armv6_halfword(bus, addr, word)? as i16) as u32;
                self.finish_load(rn, rd, writeback, offset_addr, value);
            }
            (false, 0b01) => {
                let value = self.reg(rd, 8) as u16;
                self.write_armv6_halfword(bus, addr, value, word)?;
                if writeback {
                    self.set_base(rn, offset_addr);
                }
            }
            // LDRD: bit 20 clear, S=1, H=0.
            (false, 0b10) => {
                if rd % 2 != 0 || rd >= 14 {
                    return Err(undefined);
                }
                let first = self.read_data_u32(bus, addr)?;
                let second = self.read_data_u32(bus, addr.wrapping_add(4))?;
                if writeback {
                    self.set_base(rn, offset_addr);
                }
                self.regs[rd as usize] = first;
                self.regs[rd as usize + 1] = second;
            }
            // STRD: bit 20 clear, S=1, H=1.
            (false, 0b11) => {
                if rd % 2 != 0 || rd >= 14 {
                    return Err(undefined);
                }
                let (first, second) = (self.reg(rd, 8), self.reg(rd + 1, 8));
                self.write_data_u32(bus, addr, first)?;
                self.write_data_u32(bus, addr.wrapping_add(4), second)?;
                if writeback {
                    self.set_base(rn, offset_addr);
                }
            }
            _ => return Err(undefined),
        }
        Ok(StepEvent::Executed)
    }

    /// Common tail of the single-register halfword/byte loads.
    fn finish_load(&mut self, rn: u32, rd: u32, writeback: bool, new_base: u32, value: u32) {
        if writeback {
            self.set_base(rn, new_base);
        }
        self.write_reg(rd, value);
    }

    /// Reads a User-mode register while a privileged mode is active (LDM/STM with S).
    fn user_register(&self, index: usize) -> u32 {
        match index {
            8..=12 if self.cpsr.mode() == super::Psr::FIQ_MODE => {
                self.banked.user_r8_r12[index - 8]
            }
            13 | 14 => self.banked.user_sp_lr[index - 13],
            _ => self.regs[index],
        }
    }

    fn set_user_register(&mut self, index: usize, value: u32) {
        match index {
            8..=12 if self.cpsr.mode() == super::Psr::FIQ_MODE => {
                self.banked.user_r8_r12[index - 8] = value;
            }
            13 | 14 => self.banked.user_sp_lr[index - 13] = value,
            _ => self.regs[index] = value,
        }
    }

    fn block_transfer<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        let pre_index = bit(word, 24);
        let up = bit(word, 23);
        let writeback = bit(word, 21);
        let load = bit(word, 20);
        let rn = bits(word, 19, 16);
        let list = bits(word, 15, 0);

        // ARMv6 does not permit PC as the A32 multiple-transfer base.
        // A writeback load may not include its base in the destination list.
        // For a writeback store, the base may appear only as the lowest
        // numbered register, where the original base value is well-defined.
        let base_in_list = list & (1 << rn) != 0;
        let invalid_writeback_list =
            writeback && base_in_list && (load || list & ((1 << rn) - 1) != 0);
        if rn as usize == PC || invalid_writeback_list {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let s_bit = bit(word, 22);
        let pc_in_list = list & (1 << PC) != 0;
        // S without a loaded PC transfers the User-mode registers; with a loaded
        // PC (LDM only) it is an exception return that also restores the SPSR.
        let exception_return = s_bit && load && pc_in_list;
        let user_bank = s_bit && !exception_return;
        let mode = self.cpsr.mode();
        if user_bank
            && (writeback || matches!(mode, super::Psr::USER_MODE | super::Psr::SYSTEM_MODE))
        {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }
        if exception_return && !super::Psr::mode_has_spsr(mode) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }
        if list == 0 {
            // The architecture classifies an empty list as UNPREDICTABLE.
            // Reject it deterministically rather than report a missing feature.
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word,
            });
        }

        let size = 4 * list.count_ones();
        let base = self.reg(rn, 8);
        let (start, new_base) = if up {
            let start = base.wrapping_add(if pre_index { 4 } else { 0 });
            (start, base.wrapping_add(size))
        } else {
            let new_base = base.wrapping_sub(size);
            (
                new_base.wrapping_add(if pre_index { 0 } else { 4 }),
                new_base,
            )
        };

        // LDM/STM are multi-word accesses. U=1 or A=1 requires a
        // word-aligned start address; the legacy U=0,A=0 path uses
        // Word[Align(Addr)] but still writes back from the original base.
        let unaligned = start & 3 != 0;
        if unaligned && self.cp15.alignment_fault_enabled() {
            return Err(Error::InvalidAccess { addr: start });
        }
        let transfer_start = if unaligned {
            let aligned = start & !3;
            // Device and Strongly-ordered memory cannot use the legacy
            // unaligned multi-word behavior. Preflight the complete byte
            // range before the first load/store to avoid partial state.
            self.require_normal_unaligned_range(
                bus,
                aligned,
                size as usize,
                !load,
                self.cpsr.privileged(),
                word,
            )?;
            aligned
        } else {
            start
        };

        let mut addr = transfer_start;
        if load {
            let mut values = [0u32; 16];
            for (r, value) in values.iter_mut().enumerate() {
                if list & (1 << r) != 0 {
                    *value = self.read_data_u32(bus, addr)?;
                    addr = addr.wrapping_add(4);
                }
            }
            // A normal LDM interworks when loading PC, but an A32 target
            // aligned only to a halfword (low bits 10) is UNPREDICTABLE.
            if pc_in_list && !exception_return && values[PC] & 3 == 2 {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            if writeback {
                self.set_base(rn, new_base);
            }
            for (r, value) in values.iter().enumerate().take(PC) {
                if list & (1 << r) != 0 {
                    if user_bank {
                        self.set_user_register(r, *value);
                    } else {
                        self.regs[r] = *value;
                    }
                }
            }
            if exception_return {
                self.exception_return(word, values[PC])?;
            } else if pc_in_list {
                self.branch_exchange(values[PC]);
            }
        } else {
            for r in 0..16u32 {
                if list & (1 << r) != 0 {
                    let value = if user_bank && r != PC as u32 {
                        self.user_register(r as usize)
                    } else {
                        self.reg(r, 8)
                    };
                    self.write_data_u32(bus, addr, value)?;
                    addr = addr.wrapping_add(4);
                }
            }
            if writeback {
                self.set_base(rn, new_base);
            }
        }
        Ok(StepEvent::Executed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{Psr, SP};
    use crate::memory::FlatMemory;

    /// Scratch area for load/store tests.
    const DATA: u32 = 0x800;

    struct Machine {
        cpu: Arm11Core,
        mem: FlatMemory,
    }

    impl Machine {
        fn new(program: &[u32]) -> Self {
            let mut mem = FlatMemory::new(0, 0x1000);
            for (i, word) in program.iter().enumerate() {
                mem.write32(i as u32 * 4, *word).unwrap();
            }
            Self {
                cpu: Arm11Core::new(),
                mem,
            }
        }

        fn step(&mut self) -> StepEvent {
            self.cpu.step(&mut self.mem).unwrap()
        }

        fn run(&mut self, steps: usize) {
            for _ in 0..steps {
                self.step();
            }
        }

        fn flags(&self) -> (bool, bool, bool, bool) {
            let psr = self.cpu.cpsr;
            (psr.n(), psr.z(), psr.c(), psr.v())
        }
    }

    // ----- data processing --------------------------------------------------

    #[test]
    fn armv6_srs_supports_all_four_addressing_modes_and_updates_target_bank() {
        let cases = [
            (false, true, 0x900, 0x908),  // SRSIA sp!, #irq
            (true, true, 0x904, 0x908),   // SRSIB sp!, #irq
            (false, false, 0x8FC, 0x8F8), // SRSDA sp!, #irq
            (true, false, 0x8F8, 0x8F8),  // SRSDB sp!, #irq
        ];
        let link = 0x1234_5678;
        let saved = Psr(0xA000_0030 | Psr::USER_MODE);

        for (pre, increment, transfer_address, expected_sp) in cases {
            let mut m = Machine::new(&[0]);
            m.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
            m.cpu.regs[SP] = 0x900;
            m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            m.cpu.regs[SP] = 0x700;
            m.cpu.regs[super::LR] = link;
            m.cpu.set_saved_program_status(saved).unwrap();

            let instruction = 0xF84D_0500
                | (u32::from(pre) << 24)
                | (u32::from(increment) << 23)
                | (1 << 21)
                | Psr::IRQ_MODE;
            m.mem.write32(0, instruction).unwrap();

            assert_eq!(m.step(), StepEvent::Executed);
            assert_eq!(m.mem.read32(transfer_address).unwrap(), link);
            assert_eq!(m.mem.read32(transfer_address + 4).unwrap(), saved.0);

            // SRS writes back to IRQ's banked SP, not the current SVC SP.
            m.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
            assert_eq!(m.cpu.regs[SP], expected_sp);
            m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            assert_eq!(m.cpu.regs[SP], 0x700);
        }
    }

    #[test]
    fn armv6_srs_without_writeback_and_big_endian_storage() {
        let mut m = Machine::new(&[0]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.regs[SP] = 0x800;
        m.cpu.regs[super::LR] = 0x1122_3344;
        let saved = Psr(0xA000_0010 | Psr::USER_MODE);
        m.cpu.set_saved_program_status(saved).unwrap();
        m.cpu.cpsr.set_big_endian(true);

        // SRSIA #svc, without writeback.
        let instruction = 0xF8CD_0500 | Psr::SUPERVISOR_MODE;
        m.mem.write32(0, instruction).unwrap();
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.mem.read32(0x800).unwrap(), 0x4433_2211);
        assert_eq!(m.mem.read32(0x804).unwrap(), saved.0.swap_bytes());
        assert_eq!(m.cpu.regs[SP], 0x800);
    }

    #[test]
    fn armv6_srs_alignment_obeys_sctlr_a_and_u() {
        let mut user = Machine::new(&[0xF84D_0512]);
        assert!(matches!(
            user.cpu.step(&mut user.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
        assert_eq!(user.cpu.pc(), 0);

        let mut invalid_target = Machine::new(&[0xF84D_0516]);
        invalid_target
            .cpu
            .switch_mode(Psr::SUPERVISOR_MODE)
            .unwrap();
        invalid_target
            .cpu
            .set_saved_program_status(Psr(Psr::USER_MODE))
            .unwrap();
        assert!(matches!(
            invalid_target.cpu.step(&mut invalid_target.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // With both alignment controls clear, SRS uses Align(SP) for its
        // transfer address rather than rejecting the unaligned banked SP.
        let mut unaligned = Machine::new(&[0xF84D_0512]);
        unaligned.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        unaligned.cpu.regs[super::LR] = 0x1122_3344;
        unaligned.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        unaligned.cpu.regs[SP] = 0x901;
        unaligned.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        let saved = Psr(Psr::USER_MODE);
        unaligned.cpu.set_saved_program_status(saved).unwrap();
        assert_eq!(
            unaligned.cpu.step(&mut unaligned.mem).unwrap(),
            StepEvent::Executed
        );
        assert_eq!(unaligned.mem.read32(0x8FC).unwrap(), 0x1122_3344);
        assert_eq!(unaligned.mem.read32(0x900).unwrap(), saved.0);
        assert_eq!(unaligned.cpu.pc(), 4);

        // ARMv6 SRS checks alignment whenever either SCTLR.A or SCTLR.U is set.
        for control in [1 << 1, 1 << 22] {
            let mut strict = Machine::new(&[0xF84D_0512]);
            strict.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            strict.cpu.set_saved_program_status(saved).unwrap();
            strict.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
            strict.cpu.regs[SP] = 0x901;
            strict.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            strict.cpu.cp15.write(true, 0, 1, 0, 0, control).unwrap();
            assert!(matches!(
                strict.cpu.step(&mut strict.mem),
                Err(Error::InvalidAccess { addr: 0x901 })
            ));
            assert_eq!(strict.cpu.pc(), 0);
        }
    }

    #[test]
    fn armv6_srs_legacy_unaligned_sp_aligns_transfer_and_writes_back_original_base() {
        let mut m = Machine::new(&[0]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.regs[super::LR] = 0x1122_3344;
        m.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        m.cpu.regs[SP] = 0x901;
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        let saved = Psr(Psr::USER_MODE);
        m.cpu.set_saved_program_status(saved).unwrap();

        // SRSIA sp!, #irq: data uses Align(SP), writeback uses the original SP.
        let instruction = 0xF84D_0500 | (1 << 23) | (1 << 21) | Psr::IRQ_MODE;
        m.mem.write32(0, instruction).unwrap();
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.mem.read32(0x900).unwrap(), 0x1122_3344);
        assert_eq!(m.mem.read32(0x904).unwrap(), saved.0);

        m.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        assert_eq!(m.cpu.regs[SP], 0x909);
    }

    #[test]
    fn armv6_rfe_supports_all_four_addressing_modes_and_sp_writeback() {
        let cases = [
            (0xF8BD_0A00, 0x800, 0x808), // RFEIA sp!
            (0xF9BD_0A00, 0x804, 0x808), // RFEIB sp!
            (0xF83D_0A00, 0x7FC, 0x7F8), // RFEDA sp!
            (0xF93D_0A00, 0x7F8, 0x7F8), // RFEDB sp!
        ];

        for (instruction, transfer_address, expected_writeback) in cases {
            let mut m = Machine::new(&[instruction]);
            m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            m.cpu.regs[SP] = DATA;
            let restored = Psr(0xA000_0030 | Psr::USER_MODE); // User + Thumb.
            m.mem.write32(transfer_address, 0x1237).unwrap();
            m.mem.write32(transfer_address + 4, restored.0).unwrap();

            assert_eq!(m.step(), StepEvent::Executed);
            assert_eq!(m.cpu.pc(), 0x1236);
            assert_eq!(m.cpu.cpsr, restored);

            // The writeback belongs to the old SVC bank, not the restored
            // User bank. Re-enter SVC to inspect its saved SP.
            m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            assert_eq!(m.cpu.regs[SP], expected_writeback);
        }
    }

    #[test]
    fn armv6_rfe_without_writeback_preserves_the_base_register() {
        // RFEIA r0, without the writeback suffix.
        let mut m = Machine::new(&[0xF890_0A00]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.regs[0] = DATA;
        let restored = Psr(Psr::SYSTEM_MODE);
        m.mem.write32(DATA, 0x2345).unwrap();
        m.mem.write32(DATA + 4, restored.0).unwrap();

        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.pc(), 0x2344);
        assert_eq!(m.cpu.cpsr, restored);
        assert_eq!(m.cpu.regs[0], DATA);
    }

    #[test]
    fn armv6_rfe_legacy_unaligned_base_aligns_transfer_and_writes_back_original_base() {
        // RFEIA r0!: the two memory words start at Align(r0), but writeback
        // advances the original, unaligned base by eight bytes.
        let mut m = Machine::new(&[0xF8B0_0A00]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.regs[0] = DATA + 1;
        let restored = Psr(Psr::SYSTEM_MODE);
        m.mem.write32(DATA, 0x2345).unwrap();
        m.mem.write32(DATA + 4, restored.0).unwrap();

        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.pc(), 0x2344);
        assert_eq!(m.cpu.cpsr, restored);
        assert_eq!(m.cpu.regs[0], DATA + 9);
    }

    #[test]
    fn armv6_rfe_rejects_user_mode_bad_base_and_invalid_status_atomically() {
        // RFE from User mode is undefined and does not advance the PC.
        let mut user = Machine::new(&[0xF8BD_0A00]);
        user.cpu.regs[SP] = DATA;
        assert!(matches!(
            user.cpu.step(&mut user.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
        assert_eq!(user.cpu.pc(), 0);
        assert_eq!(user.cpu.cpsr.mode(), Psr::USER_MODE);

        // Rn == PC is reserved.
        let mut pc_base = Machine::new(&[0xF8BF_0A00]);
        pc_base.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert!(matches!(
            pc_base.cpu.step(&mut pc_base.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // U or A makes an unaligned RFE a word-alignment fault.
        for alignment_bit in [1 << 22, 1 << 1] {
            let mut unaligned = Machine::new(&[0xF8B0_0A00]);
            unaligned.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            unaligned.cpu.regs[0] = DATA + 1;
            unaligned
                .cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, alignment_bit)
                .unwrap();
            assert!(matches!(
                unaligned.cpu.step(&mut unaligned.mem),
                Err(Error::InvalidAccess { addr: 0x801 })
            ));
            assert_eq!(unaligned.cpu.pc(), 0);
            assert_eq!(unaligned.cpu.regs[0], DATA + 1);
        }

        // Invalid restored mode must not partially apply writeback.
        let mut invalid_status = Machine::new(&[0xF8BD_0A00]);
        invalid_status
            .cpu
            .switch_mode(Psr::SUPERVISOR_MODE)
            .unwrap();
        invalid_status.cpu.regs[SP] = DATA;
        invalid_status.mem.write32(DATA, 0x3456).unwrap();
        invalid_status.mem.write32(DATA + 4, 0x15).unwrap();
        assert!(matches!(
            invalid_status.cpu.step(&mut invalid_status.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
        assert_eq!(invalid_status.cpu.regs[SP], DATA);
        assert_eq!(invalid_status.cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert_eq!(invalid_status.cpu.pc(), 0);
    }

    #[test]
    fn armv6_movs_pc_lr_restores_spsr_and_thumb_state() {
        // MOVS pc,lr returns to Thumb state with bit 1 of the target preserved.
        let mut m = Machine::new(&[0xE1B0_F00E]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        let restored = Psr(0xA000_003F | Psr::SYSTEM_MODE);
        m.cpu.set_saved_program_status(restored).unwrap();
        m.cpu.regs[LR] = 0x1237;

        assert_eq!(m.step(), StepEvent::Executed);

        assert_eq!(m.cpu.cpsr, restored);
        assert_eq!(m.cpu.cpsr.mode(), Psr::SYSTEM_MODE);
        assert!(m.cpu.cpsr.thumb());
        assert_eq!(m.cpu.pc(), 0x1236);
    }

    #[test]
    fn exception_return_sets_a_single_host_wakeup_notification() {
        let mut m = Machine::new(&[0xE1B0_F00E]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.set_saved_program_status(Psr(Psr::USER_MODE)).unwrap();
        m.cpu.regs[LR] = 0x1234;

        assert_eq!(m.step(), StepEvent::Executed);
        assert!(m.cpu.take_exception_return_wakeup());
        assert!(!m.cpu.take_exception_return_wakeup());
    }

    #[test]
    fn armv6_movs_pc_lr_restores_arm_state_and_word_alignment() {
        // MOVS pc,lr returns to ARM state and clears both low address bits.
        let mut m = Machine::new(&[0xE1B0_F00E]);
        m.cpu.switch_mode(Psr::IRQ_MODE).unwrap();
        let restored = Psr(0x5000_001F);
        m.cpu.set_saved_program_status(restored).unwrap();
        m.cpu.regs[LR] = 0x1237;

        assert_eq!(m.step(), StepEvent::Executed);

        assert_eq!(m.cpu.cpsr, restored);
        assert_eq!(m.cpu.cpsr.mode(), Psr::SYSTEM_MODE);
        assert!(!m.cpu.cpsr.thumb());
        assert_eq!(m.cpu.pc(), 0x1234);
    }

    #[test]
    fn armv6_movs_pc_lr_rejects_missing_or_invalid_spsr_without_switching_modes() {
        // User mode has no SPSR.
        let mut user = Machine::new(&[0xE1B0_F00E]);
        user.cpu.regs[LR] = 0x1237;
        assert!(matches!(
            user.cpu.step(&mut user.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
        assert_eq!(user.cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(user.cpu.pc(), 0);

        // An exception mode with no saved status cannot return.
        let mut missing = Machine::new(&[0xE1B0_F00E]);
        missing.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        missing.cpu.regs[LR] = 0x1237;
        assert!(matches!(
            missing.cpu.step(&mut missing.mem),
            Err(Error::Unimplemented("SPSR has not been initialized"))
        ));
        assert_eq!(missing.cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert_eq!(missing.cpu.pc(), 0);

        // SPSR mode 0x15 is reserved and must not partially switch banks.
        let mut invalid = Machine::new(&[0xE1B0_F00E]);
        invalid.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        invalid.cpu.set_saved_program_status(Psr(0x15)).unwrap();
        invalid.cpu.regs[LR] = 0x1237;
        assert!(matches!(
            invalid.cpu.step(&mut invalid.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
        assert_eq!(invalid.cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert_eq!(invalid.cpu.pc(), 0);
    }

    #[test]
    fn mov_immediate_then_add_registers() {
        // MOV r0,#1 ; MOV r1,#2 ; ADD r2,r0,r1
        let mut m = Machine::new(&[0xE3A0_0001, 0xE3A0_1002, 0xE080_2001]);
        m.run(3);
        assert_eq!(m.cpu.regs[2], 3);
        assert_eq!(m.cpu.pc(), 12);
    }

    #[test]
    fn subs_to_zero_sets_zero_and_carry() {
        // SUBS r0,r0,#1
        let mut m = Machine::new(&[0xE250_0001]);
        m.cpu.regs[0] = 1;
        m.step();
        assert_eq!(m.cpu.regs[0], 0);
        assert_eq!(m.flags(), (false, true, true, false));
    }

    #[test]
    fn subs_below_zero_sets_negative_and_clears_carry() {
        let mut m = Machine::new(&[0xE250_0001]);
        m.step();
        assert_eq!(m.cpu.regs[0], u32::MAX);
        assert_eq!(m.flags(), (true, false, false, false));
    }

    #[test]
    fn adds_signed_overflow() {
        // ADDS r0,r0,#1
        let mut m = Machine::new(&[0xE290_0001]);
        m.cpu.regs[0] = 0x7FFF_FFFF;
        m.step();
        assert_eq!(m.cpu.regs[0], 0x8000_0000);
        assert_eq!(m.flags(), (true, false, false, true));
    }

    #[test]
    fn adc_propagates_carry_for_64_bit_addition() {
        // ADDS r4,r0,r2 ; ADC r5,r1,r3
        let mut m = Machine::new(&[0xE090_4002, 0xE0A1_5003]);
        m.cpu.regs[0] = 0xFFFF_FFFF;
        m.cpu.regs[2] = 1;
        m.run(2);
        assert_eq!((m.cpu.regs[4], m.cpu.regs[5]), (0, 1));
    }

    #[test]
    fn logical_ops_take_carry_from_the_shifter() {
        // MOVS r0,r1,LSL #1
        let mut m = Machine::new(&[0xE1B0_0081]);
        m.cpu.regs[1] = 0x8000_0000;
        m.cpu.cpsr = Psr::from_flags(false, false, false, true);
        m.step();
        assert_eq!(m.cpu.regs[0], 0);
        // Z and C set, V untouched.
        assert_eq!(m.flags(), (false, true, true, true));
    }

    #[test]
    fn ror_with_zero_immediate_is_rrx() {
        // MOVS r0,r1,RRX
        let mut m = Machine::new(&[0xE1B0_0061]);
        m.cpu.regs[1] = 3;
        m.cpu.cpsr = Psr::from_flags(false, false, true, false);
        m.step();
        assert_eq!(m.cpu.regs[0], 0x8000_0001);
        assert!(m.cpu.cpsr.c());
    }

    #[test]
    fn register_controlled_shift_uses_bottom_byte_of_rs() {
        // MOV r0,r1,LSR r2
        let mut m = Machine::new(&[0xE1A0_0231, 0xE1A0_0231]);
        m.cpu.regs[1] = 0x100;
        m.cpu.regs[2] = 4;
        m.step();
        assert_eq!(m.cpu.regs[0], 0x10);

        m.cpu.regs[2] = 0xFFFF_FF28; // only the low byte (40) counts
        m.step();
        assert_eq!(m.cpu.regs[0], 0);
    }

    #[test]
    fn reading_pc_yields_instruction_address_plus_eight() {
        // MOV r1,r1 ; MOV r0,pc
        let mut m = Machine::new(&[0xE1A0_1001, 0xE1A0_000F]);
        m.run(2);
        assert_eq!(m.cpu.regs[0], 4 + 8);
    }

    #[test]
    fn conditional_execution_follows_compare() {
        // CMP r0,#5 ; MOVEQ r1,#1 ; MOVNE r2,#1
        let mut m = Machine::new(&[0xE350_0005, 0x03A0_1001, 0x13A0_2001]);
        m.cpu.regs[0] = 5;
        m.run(3);
        assert_eq!((m.cpu.regs[1], m.cpu.regs[2]), (1, 0));
    }

    #[test]
    fn armv6_a32_comparisons_reject_nonzero_rd_without_side_effects() {
        // TST, TEQ, CMP and CMN all reserve the Rd field as SBZ.
        for instruction in [
            0xE110_1002, // TST r0,r2 with invalid Rd=r1
            0xE130_1002, // TEQ r0,r2 with invalid Rd=r1
            0xE150_1002, // CMP r0,r2 with invalid Rd=r1
            0xE170_1002, // CMN r0,r2 with invalid Rd=r1
        ] {
            let mut m = Machine::new(&[instruction]);
            m.cpu.regs[0] = 0x1234_5678;
            m.cpu.regs[1] = 0xA5A5_5A5A;
            m.cpu.regs[2] = 0x8765_4321;
            m.cpu.cpsr.set_flags(true, false, true, false);
            let registers_before = m.cpu.regs;
            let status_before = m.cpu.cpsr;

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
            ));
            assert_eq!(
                m.cpu.regs, registers_before,
                "instruction {instruction:#010x}"
            );
            assert_eq!(m.cpu.cpsr, status_before, "instruction {instruction:#010x}");
            assert_eq!(m.cpu.pc(), 0, "instruction {instruction:#010x}");
        }
    }

    #[test]
    fn armv6_a32_register_shift_rejects_pc_in_every_operand_field() {
        // ADD r0,r3,r1,LSL r2 with R15 in Rn, Rd, Rs or Rm.
        for instruction in [
            0xE08F_0211, // Rn = PC
            0xE083_F211, // Rd = PC
            0xE083_0F11, // Rs = PC
            0xE083_021F, // Rm = PC
        ] {
            let mut m = Machine::new(&[instruction]);
            m.cpu.regs[1] = 0x1234_5678;
            m.cpu.regs[2] = 3;
            m.cpu.regs[3] = 0x8765_4321;
            m.cpu.cpsr.set_flags(true, false, true, false);
            let registers_before = m.cpu.regs;
            let status_before = m.cpu.cpsr;

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
            ));
            assert_eq!(
                m.cpu.regs, registers_before,
                "instruction {instruction:#010x}"
            );
            assert_eq!(m.cpu.cpsr, status_before, "instruction {instruction:#010x}");
            assert_eq!(m.cpu.pc(), 0, "instruction {instruction:#010x}");
        }
    }

    #[test]
    fn writing_pc_with_data_processing_branches() {
        // MOV pc,r0
        let mut m = Machine::new(&[0xE1A0_F000]);
        m.cpu.regs[0] = 0x40;
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
    }

    // ----- loads and stores -------------------------------------------------

    fn machine_with_data(program: &[u32]) -> Machine {
        let mut m = Machine::new(program);
        m.cpu.regs[1] = DATA;
        m.mem.write32(DATA, 0xDEAD_BEEF).unwrap();
        m.mem.write32(DATA + 4, 0x1234_5678).unwrap();
        m
    }

    #[test]
    fn ldr_and_ldrb_load_from_memory() {
        // LDR r0,[r1] ; LDRB r3,[r1]
        let mut m = machine_with_data(&[0xE591_0000, 0xE5D1_3000]);
        m.run(2);
        assert_eq!(m.cpu.regs[0], 0xDEAD_BEEF);
        assert_eq!(m.cpu.regs[3], 0xEF);
    }

    #[test]
    fn str_and_strb_store_to_memory() {
        // STR r0,[r1,#4] ; STRB r2,[r1,#1]
        let mut m = machine_with_data(&[0xE581_0004, 0xE5C1_2001]);
        m.cpu.regs[0] = 0x1122_3344;
        m.cpu.regs[2] = 0xAB;
        m.run(2);
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 0x1122_3344);
        assert_eq!(m.mem.read8(DATA + 1).unwrap(), 0xAB);
    }

    #[test]
    fn post_index_loads_then_updates_base() {
        // LDR r0,[r1],#4
        let mut m = machine_with_data(&[0xE491_0004]);
        m.step();
        assert_eq!(m.cpu.regs[0], 0xDEAD_BEEF);
        assert_eq!(m.cpu.regs[1], DATA + 4);
    }

    #[test]
    fn pre_index_with_writeback_updates_base_first() {
        // LDR r0,[r1,#4]!
        let mut m = machine_with_data(&[0xE5B1_0004]);
        m.step();
        assert_eq!(m.cpu.regs[0], 0x1234_5678);
        assert_eq!(m.cpu.regs[1], DATA + 4);
    }

    #[test]
    fn negative_immediate_offset() {
        // LDR r0,[r1,#-4]
        let mut m = machine_with_data(&[0xE511_0004]);
        m.cpu.regs[1] = DATA + 4;
        m.step();
        assert_eq!(m.cpu.regs[0], 0xDEAD_BEEF);
    }

    #[test]
    fn scaled_register_offset() {
        // LDR r0,[r1,r2,LSL #2]
        let mut m = machine_with_data(&[0xE791_0102]);
        m.cpu.regs[2] = 1;
        m.step();
        assert_eq!(m.cpu.regs[0], 0x1234_5678);
    }

    #[test]
    fn unaligned_ldr_rotates_the_aligned_word() {
        let mut m = machine_with_data(&[0xE591_0000]);
        m.mem.write32(DATA, 0xDDCC_BBAA).unwrap();
        m.cpu.regs[1] = DATA + 1;
        m.step();
        assert_eq!(m.cpu.regs[0], 0xAADD_CCBB);
    }

    #[test]
    fn armv6_unaligned_word_loads_and_stores_follow_u_a_model() {
        for big_endian in [false, true] {
            let mut load = machine_with_data(&[0xE591_0000]); // LDR r0,[r1]
            load.mem.write32(DATA, 0x3322_11AA).unwrap();
            load.mem.write32(DATA + 4, 0x8877_6644).unwrap();
            load.cpu.regs[1] = DATA + 1;
            load.cpu.cpsr.set_big_endian(big_endian);
            load.cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            load.step();
            assert_eq!(
                load.cpu.regs[0],
                if big_endian { 0x1122_3344 } else { 0x4433_2211 }
            );

            let mut store = Machine::new(&[0xE581_0000]); // STR r0,[r1]
            store.cpu.regs[0] = 0xA1B2_C3D4;
            store.cpu.regs[1] = DATA + 1;
            store.cpu.cpsr.set_big_endian(big_endian);
            store
                .cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            store.mem.write32(DATA, 0x3322_11AA).unwrap();
            store.mem.write32(DATA + 4, 0x8877_6644).unwrap();
            store.step();

            let expected = if big_endian {
                [0xA1, 0xB2, 0xC3, 0xD4]
            } else {
                [0xD4, 0xC3, 0xB2, 0xA1]
            };
            assert_eq!(store.mem.read8(DATA).unwrap(), 0xAA);
            for (offset, byte) in expected.into_iter().enumerate() {
                assert_eq!(store.mem.read8(DATA + 1 + offset as u32).unwrap(), byte);
            }
            assert_eq!(store.mem.read8(DATA + 5).unwrap(), 0x66);
        }
    }

    #[test]
    fn armv6_unaligned_ldrt_and_strt_use_user_byte_accesses_when_u_enabled() {
        for big_endian in [false, true] {
            let mut load = machine_with_data(&[0xE4B1_0000]); // LDRT r0,[r1],#0
            load.mem.write32(DATA, 0x3322_11AA).unwrap();
            load.mem.write32(DATA + 4, 0x8877_6644).unwrap();
            load.cpu.regs[1] = DATA + 1;
            load.cpu.cpsr.set_big_endian(big_endian);
            load.cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            load.step();
            assert_eq!(
                load.cpu.regs[0],
                if big_endian { 0x1122_3344 } else { 0x4433_2211 }
            );

            let mut store = Machine::new(&[0xE4A1_0000]); // STRT r0,[r1],#0
            store.cpu.regs[0] = 0xA1B2_C3D4;
            store.cpu.regs[1] = DATA + 1;
            store.cpu.cpsr.set_big_endian(big_endian);
            store
                .cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            store.mem.write32(DATA, 0x3322_11AA).unwrap();
            store.mem.write32(DATA + 4, 0x8877_6644).unwrap();
            store.step();

            let expected = if big_endian {
                [0xA1, 0xB2, 0xC3, 0xD4]
            } else {
                [0xD4, 0xC3, 0xB2, 0xA1]
            };
            for (offset, byte) in expected.into_iter().enumerate() {
                assert_eq!(store.mem.read8(DATA + 1 + offset as u32).unwrap(), byte);
            }
            assert_eq!(store.cpu.regs[1], DATA + 1);
        }
    }

    #[test]
    fn armv6_alignment_check_faults_unaligned_strt_before_writeback_or_memory() {
        let mut m = Machine::new(&[0xE4A1_0004]); // STRT r0,[r1],#4
        m.cpu.regs[0] = 0xA1B2_C3D4;
        m.cpu.regs[1] = DATA + 1;
        m.cpu
            .cp15_mut()
            .write(true, 0, 1, 0, 0, (1 << 22) | (1 << 1))
            .unwrap();
        m.mem.write32(DATA, 0x4433_2211).unwrap();
        let before = m.mem.read32(DATA).unwrap();

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::InvalidAccess { addr }) if addr == DATA + 1
        ));
        assert_eq!(m.cpu.regs[1], DATA + 1);
        assert_eq!(m.mem.read32(DATA).unwrap(), before);
    }

    #[test]
    fn armv6_unaligned_ldr_pc_is_rejected_without_alignment_check() {
        let mut m = Machine::new(&[0xE591_F000]); // LDR pc,[r1]
        m.cpu.regs[1] = DATA + 1;
        m.cpu.cp15_mut().write(true, 0, 1, 0, 0, 1 << 22).unwrap();

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xE591_F000
            })
        ));
        assert_eq!(m.cpu.pc(), 0);
    }

    #[test]
    fn ldr_into_pc_branches() {
        // LDR pc,[r1]
        let mut m = machine_with_data(&[0xE591_F000]);
        m.mem.write32(DATA, 0x20).unwrap();
        m.step();
        assert_eq!(m.cpu.pc(), 0x20);
    }

    #[test]
    fn halfword_and_signed_loads() {
        // LDRH r0,[r1] ; LDRSH r2,[r1] ; LDRSB r3,[r1]
        let mut m = machine_with_data(&[0xE1D1_00B0, 0xE1D1_20F0, 0xE1D1_30D0]);
        m.mem.write32(DATA, 0x1234_ABCD).unwrap();
        m.run(3);
        assert_eq!(m.cpu.regs[0], 0xABCD);
        assert_eq!(m.cpu.regs[2], 0xFFFF_ABCD);
        assert_eq!(m.cpu.regs[3], 0xFFFF_FFCD);
    }

    #[test]
    fn strh_writes_two_bytes_only() {
        // STRH r0,[r1]
        let mut m = machine_with_data(&[0xE1C1_00B0]);
        m.cpu.regs[0] = 0xFFFF_1234;
        m.step();
        assert_eq!(m.mem.read32(DATA).unwrap(), 0xDEAD_1234);
    }

    #[test]
    fn armv6_unaligned_halfword_loads_and_stores_follow_u_a_model() {
        for big_endian in [false, true] {
            let mut load = machine_with_data(&[0xE1D1_00B0]); // LDRH r0,[r1]
            load.mem.write32(DATA, 0x3322_11AA).unwrap();
            load.cpu.regs[1] = DATA + 1;
            load.cpu.cpsr.set_big_endian(big_endian);
            load.cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            load.step();
            assert_eq!(load.cpu.regs[0], if big_endian { 0x1122 } else { 0x2211 });

            let mut store = Machine::new(&[0xE1C1_00B0]); // STRH r0,[r1]
            store.cpu.regs[0] = 0xA1B2;
            store.cpu.regs[1] = DATA + 1;
            store.cpu.cpsr.set_big_endian(big_endian);
            store
                .cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            store.mem.write32(DATA, 0x3322_11AA).unwrap();
            store.step();

            let expected = if big_endian {
                [0xA1, 0xB2]
            } else {
                [0xB2, 0xA1]
            };
            assert_eq!(store.mem.read8(DATA).unwrap(), 0xAA);
            assert_eq!(store.mem.read8(DATA + 1).unwrap(), expected[0]);
            assert_eq!(store.mem.read8(DATA + 2).unwrap(), expected[1]);
            assert_eq!(store.mem.read8(DATA + 3).unwrap(), 0x33);
        }
    }

    #[test]
    fn legacy_unaligned_halfword_transfers_are_rejected_without_side_effects() {
        let mut load = Machine::new(&[0xE1F1_00B2]); // LDRH r0,[r1,#2]!
        load.cpu.regs[0] = 0xCAFE_BABE;
        load.cpu.regs[1] = DATA + 1;
        load.mem.write32(DATA, 0x4433_2211).unwrap();
        assert!(matches!(
            load.cpu.step(&mut load.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xE1F1_00B2
            })
        ));
        assert_eq!(load.cpu.regs[0], 0xCAFE_BABE);
        assert_eq!(load.cpu.regs[1], DATA + 1);
        assert_eq!(load.mem.read32(DATA).unwrap(), 0x4433_2211);

        let mut store = Machine::new(&[0xE1E1_00B2]); // STRH r0,[r1,#2]!
        store.cpu.regs[0] = 0xA1B2;
        store.cpu.regs[1] = DATA + 1;
        store.mem.write32(DATA, 0x4433_2211).unwrap();
        assert!(matches!(
            store.cpu.step(&mut store.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xE1E1_00B2
            })
        ));
        assert_eq!(store.cpu.regs[1], DATA + 1);
        assert_eq!(store.mem.read32(DATA).unwrap(), 0x4433_2211);
    }

    #[test]
    fn alignment_check_faults_unaligned_halfword_store_before_writeback_or_memory() {
        let mut m = Machine::new(&[0xE1E1_00B2]); // STRH r0,[r1,#2]!
        m.cpu.regs[0] = 0xA1B2;
        m.cpu.regs[1] = DATA + 1;
        m.cpu
            .cp15_mut()
            .write(true, 0, 1, 0, 0, (1 << 22) | (1 << 1))
            .unwrap();
        m.mem.write32(DATA, 0x4433_2211).unwrap();

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::InvalidAccess { addr }) if addr == DATA + 3
        ));
        assert_eq!(m.cpu.regs[1], DATA + 1);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x4433_2211);
    }

    #[test]
    fn ldrd_and_strd_move_register_pairs() {
        // LDRD r4,[r1] ; STRD r4,[r1,#8]
        let mut m = machine_with_data(&[0xE1C1_40D0, 0xE1C1_40F8]);
        m.run(1);
        assert_eq!((m.cpu.regs[4], m.cpu.regs[5]), (0xDEAD_BEEF, 0x1234_5678));
        m.run(1);
        assert_eq!(m.mem.read32(DATA + 8).unwrap(), 0xDEAD_BEEF);
        assert_eq!(m.mem.read32(DATA + 12).unwrap(), 0x1234_5678);
    }

    #[test]
    fn armv6_ldrd_strd_in_u_mode_require_word_alignment() {
        for big_endian in [false, true] {
            let mut load = Machine::new(&[0xE1C1_40D0]); // LDRD r4,[r1]
            load.cpu.regs[1] = DATA + 4;
            load.cpu.cpsr.set_big_endian(big_endian);
            load.cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            load.mem.write32(DATA + 4, 0x1122_3344).unwrap();
            load.mem.write32(DATA + 8, 0x5566_7788).unwrap();
            load.step();
            assert_eq!(
                (load.cpu.regs[4], load.cpu.regs[5]),
                if big_endian {
                    (0x4433_2211, 0x8877_6655)
                } else {
                    (0x1122_3344, 0x5566_7788)
                }
            );

            let mut store = Machine::new(&[0xE1C1_40F0]); // STRD r4,[r1]
            store.cpu.regs[1] = DATA + 4;
            store.cpu.regs[4] = 0x1122_3344;
            store.cpu.regs[5] = 0x5566_7788;
            store.cpu.cpsr.set_big_endian(big_endian);
            store
                .cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 22)
                .unwrap();
            store.step();
            assert_eq!(
                store.mem.read32(DATA + 4).unwrap(),
                if big_endian { 0x4433_2211 } else { 0x1122_3344 }
            );
            assert_eq!(
                store.mem.read32(DATA + 8).unwrap(),
                if big_endian { 0x8877_6655 } else { 0x5566_7788 }
            );
        }

        for instruction in [0xE1C1_40D0, 0xE1C1_40F0] {
            let mut m = Machine::new(&[instruction]);
            m.cpu.regs[1] = DATA + 1;
            m.cpu.regs[4] = 0xAABB_CCDD;
            m.cpu.regs[5] = 0x1122_3344;
            m.cpu.cp15_mut().write(true, 0, 1, 0, 0, 1 << 22).unwrap();
            m.mem.write32(DATA, 0xDEAD_BEEF).unwrap();
            m.mem.write32(DATA + 4, 0xCAFE_BABE).unwrap();
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::InvalidAccess { addr }) if addr == DATA + 1
            ));
            assert_eq!(m.cpu.regs[4], 0xAABB_CCDD);
            assert_eq!(m.cpu.regs[5], 0x1122_3344);
            assert_eq!(m.mem.read32(DATA).unwrap(), 0xDEAD_BEEF);
            assert_eq!(m.mem.read32(DATA + 4).unwrap(), 0xCAFE_BABE);
        }
    }

    #[test]
    fn legacy_ldrd_strd_require_doubleword_alignment() {
        for instruction in [0xE1C1_40D0, 0xE1C1_40F0] {
            let mut legacy = Machine::new(&[instruction]);
            legacy.cpu.regs[1] = DATA + 4;
            legacy.cpu.regs[4] = 0xAABB_CCDD;
            legacy.cpu.regs[5] = 0x1122_3344;
            legacy.mem.write32(DATA + 4, 0xDEAD_BEEF).unwrap();
            legacy.mem.write32(DATA + 8, 0xCAFE_BABE).unwrap();
            assert!(matches!(
                legacy.cpu.step(&mut legacy.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == instruction
            ));
            assert_eq!(legacy.cpu.regs[4], 0xAABB_CCDD);
            assert_eq!(legacy.cpu.regs[5], 0x1122_3344);
            assert_eq!(legacy.mem.read32(DATA + 4).unwrap(), 0xDEAD_BEEF);
            assert_eq!(legacy.mem.read32(DATA + 8).unwrap(), 0xCAFE_BABE);

            let mut strict = Machine::new(&[instruction]);
            strict.cpu.regs[1] = DATA + 4;
            strict
                .cpu
                .cp15_mut()
                .write(true, 0, 1, 0, 0, 1 << 1)
                .unwrap();
            assert!(matches!(
                strict.cpu.step(&mut strict.mem),
                Err(Error::InvalidAccess { addr }) if addr == DATA + 4
            ));
        }
    }

    #[test]
    fn ldrd_with_an_odd_register_is_undefined() {
        // LDRD r1,[r2]
        let mut m = Machine::new(&[0xE1C2_10D0]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
    }

    #[test]
    fn swp_word_uses_legacy_rotation_and_aligned_store_when_u_and_a_are_clear() {
        let mut m = Machine::new(&[0xE101_0092]); // SWP r0,r2,[r1]
        m.cpu.regs[1] = DATA + 1;
        m.cpu.regs[2] = 0x1122_3344;
        m.mem.write32(DATA, 0xDDCC_BBAA).unwrap();

        m.step();

        assert_eq!(m.cpu.regs[0], 0xAADD_CCBB);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x1122_3344);
    }

    #[test]
    fn swp_word_faults_when_u_or_a_requires_word_alignment() {
        for sctlr in [1 << 22, 1 << 1, (1 << 22) | (1 << 1)] {
            let mut m = Machine::new(&[0xE101_0092]); // SWP r0,r2,[r1]
            m.cpu.regs[0] = 0xCAFE_BABE;
            m.cpu.regs[1] = DATA + 1;
            m.cpu.regs[2] = 0x1122_3344;
            m.cpu.cp15_mut().write(true, 0, 1, 0, 0, sctlr).unwrap();
            m.mem.write32(DATA, 0xDEAD_BEEF).unwrap();

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::InvalidAccess { addr }) if addr == DATA + 1
            ));
            assert_eq!(m.cpu.regs[0], 0xCAFE_BABE);
            assert_eq!(m.mem.read32(DATA).unwrap(), 0xDEAD_BEEF);
        }
    }

    #[test]
    fn swpb_allows_unaligned_byte_addresses_even_when_alignment_check_is_enabled() {
        let mut m = Machine::new(&[0xE141_0092]); // SWPB r0,r2,[r1]
        m.cpu.regs[0] = 0xCAFE_BABE;
        m.cpu.regs[1] = DATA + 1;
        m.cpu.regs[2] = 0x1122_33CC;
        m.cpu.cp15_mut().write(true, 0, 1, 0, 0, 1 << 1).unwrap();
        m.mem.write8(DATA + 1, 0xA5).unwrap();

        m.step();
        assert_eq!(m.cpu.regs[0], 0xA5);
        assert_eq!(m.mem.read8(DATA + 1).unwrap(), 0xCC);
    }

    #[test]
    fn swp_exchanges_register_and_memory() {
        // SWP r0,r2,[r1]
        let mut m = machine_with_data(&[0xE101_0092]);
        m.cpu.regs[2] = 0xBBBB_BBBB;
        m.step();
        assert_eq!(m.cpu.regs[0], 0xDEAD_BEEF);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0xBBBB_BBBB);
    }

    // ----- block transfers --------------------------------------------------

    #[test]
    fn push_and_pop_round_trip_and_return() {
        // STMDB sp!,{r4,lr} ; LDMIA sp!,{r4,pc}
        let mut m = Machine::new(&[0xE92D_4010, 0xE8BD_8010]);
        m.cpu.regs[13] = 0x900;
        m.cpu.regs[4] = 0x44;
        m.cpu.regs[14] = 0x20;

        m.step();
        assert_eq!(m.cpu.regs[13], 0x8F8);
        assert_eq!(m.mem.read32(0x8F8).unwrap(), 0x44);
        assert_eq!(m.mem.read32(0x8FC).unwrap(), 0x20);

        m.cpu.regs[4] = 0;
        m.step();
        assert_eq!(m.cpu.regs[4], 0x44);
        assert_eq!(m.cpu.regs[13], 0x900);
        assert_eq!(m.cpu.pc(), 0x20);
    }

    #[test]
    fn stmia_stores_lowest_register_at_lowest_address() {
        // STMIA r0,{r1,r2,r3}
        let mut m = Machine::new(&[0xE880_000E]);
        m.cpu.regs[0] = DATA;
        m.cpu.regs[1] = 10;
        m.cpu.regs[2] = 20;
        m.cpu.regs[3] = 30;
        m.step();
        assert_eq!(m.mem.read32(DATA).unwrap(), 10);
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 20);
        assert_eq!(m.mem.read32(DATA + 8).unwrap(), 30);
        assert_eq!(m.cpu.regs[0], DATA, "no writeback requested");
    }

    #[test]
    fn armv6_legacy_unaligned_ldm_stm_align_memory_and_preserve_writeback() {
        for big_endian in [false, true] {
            let mut load = Machine::new(&[0xE8B0_0006]); // LDMIA r0!,{r1,r2}
            load.cpu.regs[0] = DATA + 1;
            load.cpu.cpsr.set_big_endian(big_endian);
            load.mem.write32(DATA, 0x1122_3344).unwrap();
            load.mem.write32(DATA + 4, 0x5566_7788).unwrap();

            load.step();

            assert_eq!(
                (load.cpu.regs[1], load.cpu.regs[2]),
                if big_endian {
                    (0x4433_2211, 0x8877_6655)
                } else {
                    (0x1122_3344, 0x5566_7788)
                }
            );
            assert_eq!(load.cpu.regs[0], DATA + 9);

            let mut store = Machine::new(&[0xE8A0_0006]); // STMIA r0!,{r1,r2}
            store.cpu.regs[0] = DATA + 1;
            store.cpu.regs[1] = 0x1122_3344;
            store.cpu.regs[2] = 0x5566_7788;
            store.cpu.cpsr.set_big_endian(big_endian);
            store.mem.write32(DATA - 4, 0xAABB_CCDD).unwrap();

            store.step();

            assert_eq!(
                store.mem.read32(DATA).unwrap(),
                if big_endian { 0x4433_2211 } else { 0x1122_3344 }
            );
            assert_eq!(
                store.mem.read32(DATA + 4).unwrap(),
                if big_endian { 0x8877_6655 } else { 0x5566_7788 }
            );
            assert_eq!(store.mem.read32(DATA - 4).unwrap(), 0xAABB_CCDD);
            assert_eq!(store.cpu.regs[0], DATA + 9);
        }
    }

    #[test]
    fn armv6_u_and_a_alignment_faults_are_atomic_for_ldm_and_stm() {
        for alignment_bit in [1 << 22, 1 << 1] {
            for instruction in [0xE8B0_0006, 0xE8A0_0006] {
                let mut m = Machine::new(&[instruction]);
                m.cpu.regs[0] = DATA + 1;
                m.cpu.regs[1] = 0x1122_3344;
                m.cpu.regs[2] = 0x5566_7788;
                m.cpu
                    .cp15_mut()
                    .write(true, 0, 1, 0, 0, alignment_bit)
                    .unwrap();
                m.mem.write32(DATA, 0xAABB_CCDD).unwrap();
                m.mem.write32(DATA + 4, 0x1234_5678).unwrap();
                let before = (m.mem.read32(DATA).unwrap(), m.mem.read32(DATA + 4).unwrap());

                assert!(matches!(
                    m.cpu.step(&mut m.mem),
                    Err(Error::InvalidAccess { addr }) if addr == DATA + 1
                ));
                assert_eq!(m.cpu.regs[0], DATA + 1);
                assert_eq!(m.cpu.regs[1], 0x1122_3344);
                assert_eq!(m.cpu.regs[2], 0x5566_7788);
                assert_eq!(
                    (m.mem.read32(DATA).unwrap(), m.mem.read32(DATA + 4).unwrap()),
                    before
                );
                assert_eq!(m.cpu.pc(), 0);
            }
        }
    }

    // ----- branches ---------------------------------------------------------

    #[test]
    fn forward_branch_skips_an_instruction() {
        // B +0 (target 8) ; MOV r0,#1 ; MOV r1,#2
        let mut m = Machine::new(&[0xEA00_0000, 0xE3A0_0001, 0xE3A0_1002]);
        m.run(2);
        assert_eq!((m.cpu.regs[0], m.cpu.regs[1]), (0, 2));
    }

    #[test]
    fn branch_to_self_stays_put() {
        let mut m = Machine::new(&[0xEAFF_FFFE]);
        m.run(3);
        assert_eq!(m.cpu.pc(), 0);
    }

    #[test]
    fn bl_saves_the_return_address() {
        let mut m = Machine::new(&[0xEB00_0000]);
        m.step();
        assert_eq!((m.cpu.regs[14], m.cpu.pc()), (4, 8));
    }

    #[test]
    fn bx_selects_arm_or_thumb_from_bit_zero() {
        // BX lr
        let mut m = Machine::new(&[0xE12F_FF1E]);
        m.cpu.regs[14] = 0x40;
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(!m.cpu.cpsr.thumb());

        let mut m = Machine::new(&[0xE12F_FF1E]);
        m.cpu.regs[14] = 0x41;
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(m.cpu.cpsr.thumb());
        // The next fetch is a 16-bit halfword: the zeroed memory decodes as
        // `MOVS r0, r0`, so pc advances by 2.
        m.step();
        assert_eq!(m.cpu.pc(), 0x42);
    }

    #[test]
    fn blx_register_links_and_branches() {
        // BLX r2
        let mut m = Machine::new(&[0xE12F_FF32]);
        m.cpu.regs[2] = 0x30;
        m.step();
        assert_eq!((m.cpu.regs[14], m.cpu.pc()), (4, 0x30));
    }

    #[test]
    fn blx_immediate_enters_thumb_with_half_word_offset() {
        let mut m = Machine::new(&[0xFA00_0000, 0xFB00_0000]);
        m.step();
        assert_eq!((m.cpu.regs[14], m.cpu.pc()), (4, 8));
        assert!(m.cpu.cpsr.thumb());

        let mut m = Machine::new(&[0xFB00_0000]);
        m.step();
        assert_eq!(m.cpu.pc(), 0xA);
    }

    #[test]
    fn armv6_cpsid_and_cpsie_update_interrupt_masks_without_touching_flags() {
        // CPSID i ; CPSIE i ; CPSID aif ; CPSIE aif.
        let mut m = Machine::new(&[0xF10C_0080, 0xF108_0080, 0xF10C_01C0, 0xF108_01C0]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.cpsr.set_flags(true, false, true, false);
        m.cpu.cpsr.set_q();
        m.cpu.cpsr.set_ge(0b1010);
        let stable = m.cpu.cpsr.0 & !((1 << 8) | (1 << 7) | (1 << 6));

        m.step();
        assert_ne!(m.cpu.cpsr.0 & (1 << 7), 0);
        m.step();
        assert_eq!(m.cpu.cpsr.0 & (1 << 7), 0);
        m.step();
        assert_eq!(m.cpu.cpsr.0 & ((1 << 8) | (1 << 7) | (1 << 6)), 0x1C0);
        m.step();
        assert_eq!(m.cpu.cpsr.0 & ((1 << 8) | (1 << 7) | (1 << 6)), 0);
        assert_eq!(m.cpu.cpsr.0 & !((1 << 8) | (1 << 7) | (1 << 6)), stable);
        assert_eq!(m.flags(), (true, false, true, false));
        assert!(m.cpu.cpsr.q());
        assert_eq!(m.cpu.cpsr.ge(), 0b1010);
    }

    #[test]
    fn armv6_cps_is_a_noop_in_user_mode_and_switches_privileged_banks() {
        // CPSID i is treated as a NOP in User mode.
        let mut user = Machine::new(&[0xF10C_0080]);
        user.step();
        assert_eq!(user.cpu.cpsr.0 & (1 << 7), 0);

        // CPS #SVC switches from System to Supervisor and selects its SP/LR bank.
        let mut privileged = Machine::new(&[0xF102_0013]);
        privileged.cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        privileged.cpu.regs[SP] = 0x2000;
        privileged.cpu.regs[LR] = 0x2004;
        privileged.step();
        assert_eq!(privileged.cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert_eq!((privileged.cpu.regs[SP], privileged.cpu.regs[LR]), (0, 0));

        privileged.cpu.regs[SP] = 0x3000;
        privileged.cpu.regs[LR] = 0x3004;
        privileged.cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        assert_eq!(
            (privileged.cpu.regs[SP], privileged.cpu.regs[LR]),
            (0x2000, 0x2004)
        );
        privileged.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert_eq!(
            (privileged.cpu.regs[SP], privileged.cpu.regs[LR]),
            (0x3000, 0x3004)
        );

        // Mode bits are SBZ when CPSID/CPSIE does not request a mode change.
        let mut malformed = Machine::new(&[0xF10C_0093]);
        malformed.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        assert!(matches!(
            malformed.cpu.step(&mut malformed.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
    }

    #[test]
    fn armv6_mrs_and_msr_read_and_write_the_active_spsr() {
        // MRS r0,SPSR ; MSR SPSR_f,r4 ; MRS r1,SPSR ;
        // MSR SPSR_c,r3 ; MRS r2,SPSR ; MRS r5,CPSR.
        let mut m = Machine::new(&[
            0xE14F_0000,
            0xE168_F004,
            0xE14F_1000,
            0xE161_F003,
            0xE14F_2000,
            0xE10F_5000,
        ]);
        m.cpu
            .switch_mode(super::super::Psr::SUPERVISOR_MODE)
            .unwrap();
        m.cpu
            .set_saved_program_status(super::super::Psr(0x1234_0010))
            .unwrap();
        m.cpu.regs[4] = 0xA500_0000;
        m.cpu.regs[3] = super::super::Psr::SYSTEM_MODE;

        m.step();
        assert_eq!(m.cpu.regs[0], 0x1234_0010);
        m.step();
        m.step();
        assert_eq!(m.cpu.regs[1], 0xA534_0010);
        m.step();
        m.step();
        assert_eq!(m.cpu.regs[2], 0xA534_001F);
        m.step();
        assert_eq!(
            m.cpu.regs[5] & super::super::Psr::MODE_MASK,
            super::super::Psr::SUPERVISOR_MODE
        );
    }

    #[test]
    fn armv6_msr_cpsr_control_field_switches_register_banks() {
        // MSR CPSR_c,r6.
        let mut m = Machine::new(&[0xE121_F006]);
        m.cpu.switch_mode(super::super::Psr::SYSTEM_MODE).unwrap();
        m.cpu.regs[SP] = 0x1000;
        m.cpu.regs[LR] = 0x1004;
        m.cpu.regs[6] = super::super::Psr::IRQ_MODE;

        m.step();
        assert_eq!(m.cpu.cpsr.mode(), super::super::Psr::IRQ_MODE);
        assert_eq!((m.cpu.regs[SP], m.cpu.regs[LR]), (0, 0));

        m.cpu.switch_mode(super::super::Psr::SYSTEM_MODE).unwrap();
        assert_eq!((m.cpu.regs[SP], m.cpu.regs[LR]), (0x1000, 0x1004));
    }

    #[test]
    fn armv6_msr_rejects_pc_as_register_source_for_cpsr_and_spsr() {
        // MSR CPSR_c,pc ; MSR SPSR_fsxc,pc.
        for instruction in [0xE121_F00F, 0xE16F_F00F] {
            let mut m = Machine::new(&[instruction]);
            m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
            let before_cpsr = m.cpu.cpsr;
            let before_spsr = Psr(0xA000_0010);
            m.cpu.set_saved_program_status(before_spsr).unwrap();

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, .. })
            ));
            assert_eq!(m.cpu.cpsr, before_cpsr);
            assert_eq!(m.cpu.saved_program_status(), Some(before_spsr));
        }
    }

    #[test]
    fn armv6_spsr_access_is_rejected_without_an_exception_mode_bank() {
        // MRS r0,SPSR and MSR SPSR_fsxc,r2 are not available in User mode.
        let mut mrs = Machine::new(&[0xE14F_0000]);
        assert!(matches!(
            mrs.cpu.step(&mut mrs.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        let mut msr = Machine::new(&[0xE16F_F002]);
        msr.cpu.regs[2] = 0x1234_5678;
        assert!(matches!(
            msr.cpu.step(&mut msr.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
    }

    #[test]
    fn armv6_user_mode_msr_can_write_nzcvq_ge_and_endianness_only() {
        // MSR CPSR_fsxc,r1 with every source bit set. ARM1136 exposes NZCVQ,
        // GE and E to User mode, but protects A/I/F/M and ignores J/T/reserved.
        let mut m = Machine::new(&[0xE12F_F001]);
        m.cpu.regs[1] = u32::MAX;

        m.step();

        let expected =
            Psr::N | Psr::Z | Psr::C | Psr::V | Psr::Q | Psr::GE_MASK | Psr::E | Psr::USER_MODE;
        assert_eq!(m.cpu.cpsr.0, expected);
    }

    #[test]
    fn armv6_privileged_msr_cannot_modify_j_or_t_state_bits() {
        // The privileged CPSR control field is writable, but J and T are not.
        let mut m = Machine::new(&[0xE12F_F001]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.regs[1] = u32::MAX;

        m.step();

        let expected = Psr::N
            | Psr::Z
            | Psr::C
            | Psr::V
            | Psr::Q
            | Psr::GE_MASK
            | Psr::E
            | Psr::A
            | Psr::I
            | Psr::F
            | Psr::SYSTEM_MODE;
        assert_eq!(m.cpu.cpsr.0, expected);
    }

    #[test]
    fn armv6_msr_cpsr_rejects_invalid_mode_without_mutating_status() {
        // MSR CPSR_c,r6 with an unallocated mode value.
        let mut m = Machine::new(&[0xE121_F006]);
        m.cpu.switch_mode(super::super::Psr::SYSTEM_MODE).unwrap();
        m.cpu.regs[6] = 0x15;
        let before = m.cpu.cpsr;
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
        assert_eq!(m.cpu.cpsr, before);
    }

    #[test]
    fn armv6_msr_cpsr_control_write_does_not_grant_privilege_from_reserved_mode() {
        // MSR CPSR_c,r6 must not treat an unallocated mode as privileged.
        let mut m = Machine::new(&[0xE121_F006]);
        m.cpu.cpsr = Psr(0x15);
        m.cpu.regs[6] = Psr::SYSTEM_MODE;
        let before = m.cpu.cpsr;

        m.step();

        assert_eq!(m.cpu.cpsr, before);
    }

    #[test]
    fn armv6_setend_changes_only_data_endianness() {
        // SETEND BE ; SETEND LE.
        let mut m = Machine::new(&[0xF101_0200, 0xF101_0000]);
        m.cpu.cpsr.set_flags(true, false, true, false);
        m.cpu.cpsr.set_q();
        m.cpu.cpsr.set_ge(0b1010);
        let preserved = m.cpu.cpsr.0 & !(1 << 9);

        m.step();
        assert!(m.cpu.cpsr.big_endian());
        assert_eq!(m.cpu.cpsr.0 & !(1 << 9), preserved);

        m.step();
        assert!(!m.cpu.cpsr.big_endian());
        assert_eq!(m.cpu.cpsr.0 & !(1 << 9), preserved);
    }

    #[test]
    fn arm1136_a32_nop_ignores_condition_and_hint_fields() {
        // EQ NOP with Z clear, AL NOP with a nonzero hint, and NV-encoded NOP.
        let mut m = Machine::new(&[0x0320_F0A5, 0xE320_F0FF, 0xF320_F055]);
        m.cpu.regs[0] = 0x1122_3344;
        m.cpu.regs[1] = 0x5566_7788;
        m.cpu.cpsr.set_flags(false, false, true, false);
        let before_cpsr = m.cpu.cpsr;

        m.run(3);

        assert_eq!(m.cpu.pc(), 12);
        assert_eq!(m.cpu.regs[0], 0x1122_3344);
        assert_eq!(m.cpu.regs[1], 0x5566_7788);
        assert_eq!(m.cpu.cpsr, before_cpsr);
    }

    #[test]
    fn armv6k_a32_wfe_consumes_pending_event_or_suspends_until_signaled() {
        let mut immediate = Machine::new(&[0xE320_F002]);
        immediate.cpu.signal_event();
        assert_eq!(immediate.step(), StepEvent::Executed);
        assert!(!immediate.cpu.event_registered);
        assert_eq!(immediate.cpu.pc(), 4);

        let mut waiting = Machine::new(&[0xE320_F002, 0xE3A0_0007, 0xE320_F002]);
        assert_eq!(waiting.step(), StepEvent::WaitForEvent);
        assert_eq!(waiting.cpu.pc(), 4);
        assert_eq!(waiting.step(), StepEvent::WaitingForEvent);
        assert_eq!(waiting.cpu.pc(), 4);

        waiting.cpu.signal_event();
        assert_eq!(waiting.step(), StepEvent::Executed);
        assert_eq!(waiting.cpu.regs[0], 7);
        assert!(waiting.cpu.event_registered);
        assert_eq!(waiting.step(), StepEvent::Executed);
        assert!(!waiting.cpu.event_registered);
        assert_eq!(waiting.cpu.pc(), 12);
    }

    #[test]
    fn armv6k_a32_wfe_wakes_on_unmasked_interrupt_and_keeps_masked_irq_asleep() {
        let mut m = Machine::new(&[0xE320_F002, 0xE3A0_0001]);
        assert_eq!(m.step(), StepEvent::WaitForEvent);

        m.cpu.cpsr.0 |= Psr::I;
        m.cpu.set_irq_line(true);
        assert_eq!(m.step(), StepEvent::WaitingForEvent);
        assert_eq!(m.cpu.pc(), 4);
        assert!(!m.cpu.event_registered);

        m.cpu.cpsr.0 &= !Psr::I;
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.cpsr.mode(), Psr::IRQ_MODE);
        assert!(m.cpu.event_registered);
        assert_eq!(m.cpu.pc(), 0x18);
    }

    #[test]
    fn armv6k_unmasked_interrupt_latches_event_even_when_core_is_awake() {
        let mut m = Machine::new(&[0xE320_F001]);
        m.cpu.set_irq_line(true);
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.cpsr.mode(), Psr::IRQ_MODE);
        assert!(m.cpu.event_registered);
    }

    #[test]
    fn armv6k_a32_wfi_hint_wakes_even_when_interrupt_is_masked() {
        let mut m = Machine::new(&[0xE320_F003, 0xE3A0_2007]);
        assert_eq!(m.step(), StepEvent::WaitForInterrupt);
        assert_eq!(m.cpu.pc(), 4);

        m.cpu.cpsr.0 |= Psr::I | Psr::F;
        m.cpu.set_irq_line(true);
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(m.cpu.regs[2], 7);
        assert_eq!(m.cpu.pc(), 8);
    }

    #[test]
    fn armv6k_a32_sev_reports_broadcast_and_sets_local_event_register() {
        let mut m = Machine::new(&[0xE320_F004, 0xE320_F002]);
        assert_eq!(m.step(), StepEvent::SendEvent);
        assert!(m.cpu.event_registered);
        assert_eq!(m.cpu.pc(), 4);

        assert_eq!(m.step(), StepEvent::Executed);
        assert!(!m.cpu.event_registered);
        assert_eq!(m.cpu.pc(), 8);
    }

    #[test]
    fn armv6k_a32_wait_hints_obey_condition_while_reserved_hints_remain_nops() {
        let mut conditional = Machine::new(&[0x0320_F002, 0x0320_F003, 0x0320_F004]);
        assert_eq!(conditional.step(), StepEvent::Executed);
        assert_eq!(conditional.step(), StepEvent::Executed);
        assert_eq!(conditional.step(), StepEvent::Executed);
        assert_eq!(conditional.cpu.pc(), 12);
        assert!(!conditional.cpu.waiting_for_event);
        assert!(!conditional.cpu.waiting_for_interrupt);
        assert!(!conditional.cpu.event_registered);

        let mut hints = Machine::new(&[0x0320_F001, 0x0320_F0A5]);
        hints.run(2);
        assert_eq!(hints.cpu.pc(), 8);
        assert!(!hints.cpu.waiting_for_event);
    }

    #[test]
    fn armv6_pld_immediate_register_and_literal_forms_are_functional_hints() {
        // PLD [r0,#16] ; PLD [r1,-r2] ; PLD [pc,#32].
        let mut m = Machine::new(&[0xF5D0_F010, 0xF751_F002, 0xF5DF_F020]);
        m.cpu.regs[0] = 0x1000;
        m.cpu.regs[1] = 0x2000;
        m.cpu.regs[2] = 0x40;
        m.cpu.regs[3] = 0xAABB_CCDD;
        m.cpu.cpsr.set_flags(true, false, true, false);
        let before = [m.cpu.regs[0], m.cpu.regs[1], m.cpu.regs[2], m.cpu.regs[3]];
        let flags = m.flags();

        m.run(3);

        assert_eq!(
            [m.cpu.regs[0], m.cpu.regs[1], m.cpu.regs[2], m.cpu.regs[3]],
            before
        );
        assert_eq!(m.flags(), flags);
    }

    #[test]
    fn armv6_reverse_media_instructions_transform_bytes_without_flags() {
        // REV r0,r1 ; REV16 r2,r3 ; REVSH r4,r5
        let mut m = Machine::new(&[0xE6BF_0F31, 0xE6BF_2FB3, 0xE6FF_4FB5]);
        m.cpu.regs[1] = 0x1234_5678;
        m.cpu.regs[3] = 0x1234_5678;
        m.cpu.regs[5] = 0x0000_80FE;
        m.cpu.cpsr.set_flags(true, false, true, false);

        m.run(3);

        assert_eq!(m.cpu.regs[0], 0x7856_3412);
        assert_eq!(m.cpu.regs[2], 0x3412_7856);
        assert_eq!(m.cpu.regs[4], 0xFFFF_FE80);
        assert_eq!(m.flags(), (true, false, true, false));
    }

    #[test]
    fn armv6_extend_instructions_apply_rotation_and_sign_extension() {
        // SXTB r0,r1,ror #8; SXTH r2,r1,ror #16;
        // UXTB r3,r1,ror #24; UXTH r4,r1,ror #8.
        let mut m = Machine::new(&[
            0xE6AF_0471, // SXTB r0,r1,ror #8
            0xE6BF_2871, // SXTH r2,r1,ror #16
            0xE6EF_3C71, // UXTB r3,r1,ror #24
            0xE6FF_4471, // UXTH r4,r1,ror #8
            0xE6AF_5871, // SXTB r5,r1,ror #16 (selects 0xFE)
        ]);
        m.cpu.regs[1] = 0x80FE_1234;
        m.run(5);

        assert_eq!(m.cpu.regs[0], 0x0000_0012);
        assert_eq!(m.cpu.regs[2], 0xFFFF_80FE);
        assert_eq!(m.cpu.regs[3], 0x0000_0080);
        assert_eq!(m.cpu.regs[4], 0x0000_FE12);
        assert_eq!(m.cpu.regs[5], 0xFFFF_FFFE);
    }

    #[test]
    fn armv6_extend_and_add_instructions_apply_rotation_and_wraparound() {
        // SXTAB r0,r1,r2,ror #16 ; SXTAH r3,r4,r2,ror #16
        // UXTAB r5,r6,r2 ; UXTAH r7,r6,r2.
        let mut m = Machine::new(&[0xE6A1_0872, 0xE6B4_3872, 0xE6E6_5072, 0xE6F6_7072]);
        m.cpu.regs[1] = 100;
        m.cpu.regs[2] = 0x80FE_1234;
        m.cpu.regs[4] = 0x0001_0000;
        m.cpu.regs[6] = 0x0001_0000;

        m.run(4);

        assert_eq!(m.cpu.regs[0], 98);
        assert_eq!(m.cpu.regs[3], 0x0000_80FE);
        assert_eq!(m.cpu.regs[5], 0x0001_0034);
        assert_eq!(m.cpu.regs[7], 0x0001_1234);
    }

    #[test]
    fn armv6_extend_byte16_instructions_extend_both_selected_lanes() {
        // SXTB16 r0,r1,ror #8 ; UXTB16 r2,r1,ror #8
        // SXTAB16 r3,r4,r1,ror #8 ; UXTAB16 r5,r4,r1,ror #8.
        let mut m = Machine::new(&[0xE68F_0471, 0xE6CF_2471, 0xE684_3471, 0xE6C4_5471]);
        m.cpu.regs[1] = 0x8010_2030;
        m.cpu.regs[4] = 0x0001_0002;

        m.run(4);

        assert_eq!(m.cpu.regs[0], 0xFF80_0020);
        assert_eq!(m.cpu.regs[2], 0x0080_0020);
        assert_eq!(m.cpu.regs[3], 0xFF81_0022);
        assert_eq!(m.cpu.regs[5], 0x0081_0022);
    }

    #[test]
    fn armv6_saturate_instructions_set_sticky_q_only_when_clamping() {
        // SSAT r0,#8,r1 ; USAT r2,#7,r3,LSL #1 ; SSAT r4,#16,r5
        let mut m = Machine::new(&[0xE6A7_0011, 0xE6E7_2193, 0xE6AF_4015]);
        m.cpu.regs[1] = 384;
        m.cpu.regs[3] = 128;
        m.cpu.regs[5] = 100;
        assert!(!m.cpu.cpsr.q());

        m.run(3);

        assert_eq!(m.cpu.regs[0], 127);
        assert_eq!(m.cpu.regs[2], 127);
        assert_eq!(m.cpu.regs[4], 100);
        assert!(m.cpu.cpsr.q());
    }

    #[test]
    fn armv6_scalar_saturating_arithmetic_implements_all_four_operations() {
        // QADD r0,r1,r2 ; QSUB r0,r1,r2 ;
        // QDADD r0,r1,r2 ; QDSUB r0,r1,r2.
        let mut m = Machine::new(&[0xE102_0051, 0xE122_0051, 0xE142_0051, 0xE162_0051]);

        m.cpu.regs[1] = i32::MAX as u32;
        m.cpu.regs[2] = 1;
        m.step();
        assert_eq!(m.cpu.regs[0], i32::MAX as u32);
        assert!(m.cpu.cpsr.q());

        m.cpu.regs[1] = i32::MIN as u32;
        m.cpu.regs[2] = 1;
        m.step();
        assert_eq!(m.cpu.regs[0], i32::MIN as u32);
        assert!(m.cpu.cpsr.q());

        // QDADD saturates the doubled second operand to INT_MAX; adding
        // INT_MIN then produces -1 without a second saturation.
        m.cpu.regs[1] = i32::MIN as u32;
        m.cpu.regs[2] = i32::MAX as u32;
        m.step();
        assert_eq!(m.cpu.regs[0], u32::MAX);
        assert!(m.cpu.cpsr.q());

        // QDSUB saturates the doubled INT_MIN operand before subtraction.
        m.cpu.regs[1] = i32::MAX as u32;
        m.cpu.regs[2] = i32::MIN as u32;
        m.step();
        assert_eq!(m.cpu.regs[0], i32::MAX as u32);
        assert!(m.cpu.cpsr.q());
    }

    #[test]
    fn armv6_ssat16_and_usat16_saturate_each_signed_halfword_independently() {
        // SSAT16 r0,#8,r1 ; USAT16 r2,#7,r3 ; SSAT16 r5,#16,r6.
        let mut m = Machine::new(&[0xE6A7_0F31, 0xE6E7_2F33, 0xE6AF_5F36]);
        m.cpu.regs[1] = 0x8000_7F01;
        m.cpu.regs[3] = 0x00FF_FF80;
        m.cpu.regs[6] = 0x8000_7FFF;
        m.cpu.cpsr.set_flags(true, false, true, false);

        m.run(3);

        assert_eq!(m.cpu.regs[0], 0xFF80_007F);
        assert_eq!(m.cpu.regs[2], 0x007F_0000);
        assert_eq!(m.cpu.regs[5], 0x8000_7FFF);
        assert!(m.cpu.cpsr.q());
        assert_eq!(m.flags(), (true, false, true, false));
    }

    #[test]
    fn armv6_ssat16_and_usat16_preserve_q_when_no_halfword_saturates() {
        // SSAT16 r0,#16,r1 ; USAT16 r2,#15,r3 — both lanes are in range.
        let mut m = Machine::new(&[0xE6AF_0F31, 0xE6EF_2F33]);
        m.cpu.regs[1] = 0x8000_7FFF;
        m.cpu.regs[3] = 0x7FFF_0000;
        m.cpu.cpsr.set_flags(false, true, true, false);

        m.run(2);

        assert_eq!(m.cpu.regs[0], 0x8000_7FFF);
        assert_eq!(m.cpu.regs[2], 0x7FFF_0000);
        assert!(!m.cpu.cpsr.q());
        assert_eq!(m.flags(), (false, true, true, false));
    }

    #[test]
    fn armv6_ssat16_and_usat16_reject_pc_registers() {
        for instruction in [0xE6A7_0F3F, 0xE6E7_FF31] {
            let mut m = Machine::new(&[instruction]);
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, .. })
            ));
        }
    }

    #[test]
    fn armv6_scalar_saturating_arithmetic_preserves_nzcv_and_sticky_q() {
        // QADD r0,r1,r2 ; QADD r0,r1,r2
        let mut m = Machine::new(&[0xE102_0051, 0xE102_0051]);
        m.cpu.regs[1] = 20;
        m.cpu.regs[2] = 22;
        m.cpu.cpsr.set_flags(true, false, true, false);

        m.step();
        assert_eq!(m.cpu.regs[0], 42);
        assert_eq!(m.flags(), (true, false, true, false));
        assert!(!m.cpu.cpsr.q());

        m.cpu.cpsr.set_q();
        m.cpu.regs[1] = 1;
        m.cpu.regs[2] = 2;
        m.step();
        assert_eq!(m.cpu.regs[0], 3);
        assert!(m.cpu.cpsr.q());
        assert_eq!(m.flags(), (true, false, true, false));
    }

    #[test]
    fn armv6_scalar_saturating_arithmetic_rejects_pc_registers() {
        for instruction in [0xE102_F051, 0xE10F_0051, 0xE102_005F] {
            let mut m = Machine::new(&[instruction]);
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, .. })
            ));
        }
    }

    #[test]
    fn armv6_parallel_signed_byte_arithmetic_sets_ge_for_sel() {
        // SADD8 r0,r1,r2 ; SEL r3,r4,r5
        let mut m = Machine::new(&[
            0xE611_0F92, // SADD8 r0,r1,r2
            0xE684_3FB5, // SEL r3,r4,r5
        ]);
        m.cpu.regs[1] = 0xFF01_807F;
        m.cpu.regs[2] = 0x01FF_FF01;
        m.cpu.regs[4] = 0x1122_3344;
        m.cpu.regs[5] = 0xAABB_CCDD;

        m.run(2);

        assert_eq!(m.cpu.regs[0], 0x0000_7F80);
        assert_eq!(m.cpu.cpsr.ge(), 0b1101);
        assert_eq!(m.cpu.regs[3], 0x1122_CC44);
    }

    #[test]
    fn armv6_parallel_signed_and_unsigned_halfword_operations_set_ge() {
        // SADD16 r0,r1,r2 ; UADD16 r3,r4,r5 ; USUB16 r6,r7,r8
        let mut m = Machine::new(&[0xE611_0F12, 0xE654_3F15, 0xE657_6F78]);
        m.cpu.regs[1] = 0x7FFF_8000;
        m.cpu.regs[2] = 0x0001_FFFF;
        m.cpu.regs[4] = 0xFFFF_0000;
        m.cpu.regs[5] = 0x0001_0001;
        m.cpu.regs[7] = 0x0001_0000;
        m.cpu.regs[8] = 0x0001_0001;

        m.run(1);
        assert_eq!(m.cpu.regs[0], 0x8000_7FFF);
        assert_eq!(m.cpu.cpsr.ge(), 0b1100);

        m.run(1);
        assert_eq!(m.cpu.regs[3], 0x0000_0001);
        assert_eq!(m.cpu.cpsr.ge(), 0b1100);

        m.run(1);
        assert_eq!(m.cpu.regs[6], 0x0000_FFFF);
        assert_eq!(m.cpu.cpsr.ge(), 0b1100);
    }

    #[test]
    fn armv6_parallel_saturating_and_halving_variants_preserve_all_flags() {
        // QADD16, UQADD8, SHADD8, UHSUB16.
        let mut m = Machine::new(&[0xE621_0F12, 0xE664_3F95, 0xE637_6F98, 0xE67A_9F7B]);
        m.cpu.regs[1] = 0x7FFF_8000;
        m.cpu.regs[2] = 0x0001_FFFF;
        m.cpu.regs[4] = 0xFF00_80FE;
        m.cpu.regs[5] = 0x0201_8080;
        m.cpu.regs[7] = 0xFF01_807F;
        m.cpu.regs[8] = 0x01FF_FF01;
        m.cpu.regs[10] = 0x0001_0000;
        m.cpu.regs[11] = 0x0001_0001;
        m.cpu.cpsr.set_ge(0b1010);
        m.cpu.cpsr.set_q();

        m.run(4);

        assert_eq!(m.cpu.regs[0], 0x7FFF_8000);
        assert_eq!(m.cpu.regs[3], 0xFF01_FFFF);
        assert_eq!(m.cpu.regs[6], 0x0000_BF40);
        assert_eq!(m.cpu.regs[9], 0x0000_FFFF);
        assert_eq!(m.cpu.cpsr.ge(), 0b1010);
        assert!(m.cpu.cpsr.q());
    }

    #[test]
    fn armv6_parallel_exchanged_saturating_and_halving_variants_swap_operands() {
        // QASX r3,r1,r2 ; UHASX r6,r1,r2.
        let mut m = Machine::new(&[0xE621_3F32, 0xE671_6F32]);
        m.cpu.regs[1] = 0x000A_0014;
        m.cpu.regs[2] = 0x0003_0005;

        m.run(2);

        assert_eq!(m.cpu.regs[3], 0x000F_0011);
        assert_eq!(m.cpu.regs[6], 0x0007_0008);
    }

    #[test]
    fn armv6_parallel_exchange_operations_swap_the_second_operand_lanes() {
        // SADDSUBX r0,r1,r2 ; SSUBADDX r3,r1,r2
        let mut m = Machine::new(&[0xE611_0F32, 0xE611_3F52]);
        m.cpu.regs[1] = 0x000A_0014;
        m.cpu.regs[2] = 0x0003_0005;

        m.run(2);

        assert_eq!(m.cpu.regs[0], 0x000F_0011);
        assert_eq!(m.cpu.regs[3], 0x0005_0017);
    }

    #[test]
    fn armv6_pack_halfword_handles_left_and_arithmetic_right_shifts() {
        // PKHBT r0,r1,r2,LSL #8 ; PKHTB r3,r4,r5,ASR #8
        let mut m = Machine::new(&[0xE681_0412, 0xE684_3455]);
        m.cpu.regs[1] = 0x1122_3344;
        m.cpu.regs[2] = 0xAABB_CCDD;
        m.cpu.regs[4] = 0x5566_7788;
        m.cpu.regs[5] = 0x8000_1234;

        m.run(2);

        assert_eq!(m.cpu.regs[0], 0xBBCC_3344);
        assert_eq!(m.cpu.regs[3], 0x5566_0012);
    }

    #[test]
    fn armv6_usad8_and_usada8_sum_absolute_byte_differences() {
        // USAD8 r0,r1,r2 ; USADA8 r4,r1,r2,r3
        let mut m = Machine::new(&[0xE780_F112, 0xE784_3112]);
        m.cpu.regs[1] = 0xFF00_8040;
        m.cpu.regs[2] = 0x0180_8020;
        m.cpu.regs[3] = 10;

        m.run(2);

        assert_eq!(m.cpu.regs[0], 414);
        assert_eq!(m.cpu.regs[4], 424);
    }

    #[test]
    fn armv6_halfword_multiply_accumulate_and_plain_multiply_cover_all_lane_pairs() {
        // SMLABB: low(Rm) * low(Rs) + Rn.
        let mut m = Machine::new(&[0xE100_3281]);
        m.cpu.regs[1] = 0x0002_0003;
        m.cpu.regs[2] = 0x0004_0005;
        m.cpu.regs[3] = 3;
        m.step();
        assert_eq!(m.cpu.regs[0], 18);

        // SMLABT: low(Rm) * high(Rs) + Rn.
        let mut m = Machine::new(&[0xE104_76C5]);
        m.cpu.regs[5] = 0x0002_0003;
        m.cpu.regs[6] = 0x0004_0005;
        m.cpu.regs[7] = 1;
        m.step();
        assert_eq!(m.cpu.regs[4], 13);

        // SMLATB: high(Rm) * low(Rs) + Rn.
        let mut m = Machine::new(&[0xE108_BAA9]);
        m.cpu.regs[9] = 0x0002_0003;
        m.cpu.regs[10] = 0x0004_0005;
        m.cpu.regs[11] = 2;
        m.step();
        assert_eq!(m.cpu.regs[8], 12);

        // SMLATT: high(Rm) * high(Rs) + Rn.
        let mut m = Machine::new(&[0xE100_32E1]);
        m.cpu.regs[1] = 0x0002_0003;
        m.cpu.regs[2] = 0x0004_0005;
        m.cpu.regs[3] = 3;
        m.step();
        assert_eq!(m.cpu.regs[0], 11);

        // SMULBT: low(Rm) * high(Rs), without accumulation.
        let mut m = Machine::new(&[0xE167_09C8]);
        m.cpu.regs[8] = 0x0002_0003;
        m.cpu.regs[9] = 0x0004_0005;
        m.step();
        assert_eq!(m.cpu.regs[7], 12);
    }

    #[test]
    fn armv6_halfword_multiply_accumulate_sets_sticky_q_on_sum_overflow() {
        // SMLABB: 3*5 plus i32::MAX overflows and wraps to 0x8000000E.
        let mut m = Machine::new(&[0xE100_3281]);
        m.cpu.regs[1] = 3;
        m.cpu.regs[2] = 5;
        m.cpu.regs[3] = i32::MAX as u32;
        m.step();
        assert_eq!(m.cpu.regs[0], 0x8000_000E);
        assert!(m.cpu.cpsr.q());
        assert_eq!(m.flags(), (false, false, false, false));
    }

    #[test]
    fn armv6_long_halfword_multiply_accumulates_into_register_pair() {
        // SMLALBB r8,r9,r10,r11; product 3*5 is added to RdHi:RdLo.
        let mut m = Machine::new(&[0xE149_8B8A]);
        m.cpu.regs[8] = 100;
        m.cpu.regs[9] = 0;
        m.cpu.regs[10] = 0x0002_0003;
        m.cpu.regs[11] = 0x0004_0005;
        m.step();
        assert_eq!((m.cpu.regs[9], m.cpu.regs[8]), (0, 115));

        // SMLALTT uses the upper halfword of each source register.
        let mut m = Machine::new(&[0xE145_47E6]);
        m.cpu.regs[4] = 0;
        m.cpu.regs[5] = 0;
        m.cpu.regs[6] = 0x0002_0003;
        m.cpu.regs[7] = 0x0004_0005;
        m.step();
        assert_eq!((m.cpu.regs[5], m.cpu.regs[4]), (0, 8));
    }

    #[test]
    fn armv6_word_by_halfword_multiply_variants_select_bottom_and_top() {
        // SMLAWT r7,r8,r9,r10: high 32 bits of 0x10000*2 plus accumulator 7.
        let mut m = Machine::new(&[0xE127_A9C8]);
        m.cpu.regs[8] = 0x0001_0000;
        m.cpu.regs[9] = 0x0002_0000;
        m.cpu.regs[10] = 7;
        m.step();
        assert_eq!(m.cpu.regs[7], 9);

        // SMULWB r11,r12,r0: high 32 bits of 0x10000*2.
        let mut m = Machine::new(&[0xE12B_00AC]);
        m.cpu.regs[12] = 0x0001_0000;
        m.cpu.regs[0] = 2;
        m.step();
        assert_eq!(m.cpu.regs[11], 2);

        // SMULWT r1,r2,r3 uses the upper halfword of Rs.
        let mut m = Machine::new(&[0xE121_03E2]);
        m.cpu.regs[2] = 0x0001_0000;
        m.cpu.regs[3] = 0x0002_0000;
        m.step();
        assert_eq!(m.cpu.regs[1], 2);
    }

    #[test]
    fn armv6_word_by_halfword_accumulate_sets_q_on_accumulator_overflow() {
        // SMLAWB: the shifted product is 2; adding i32::MAX overflows.
        let mut m = Machine::new(&[0xE123_6584]);
        m.cpu.regs[4] = 0x0001_0000;
        m.cpu.regs[5] = 2;
        m.cpu.regs[6] = i32::MAX as u32;
        m.step();
        assert_eq!(m.cpu.regs[3], 0x8000_0001);
        assert!(m.cpu.cpsr.q());
    }

    #[test]
    fn armv6_umaal_accumulates_two_independent_words_without_touching_flags() {
        // UMAAL r0,r1,r2,r3 = r2*r3 + r0 + r1 (not a concatenated accumulator).
        let mut m = Machine::new(&[0xE041_0392]);
        m.cpu.regs[0] = 0xFFFF_FFFE;
        m.cpu.regs[1] = 2;
        m.cpu.regs[2] = 3;
        m.cpu.regs[3] = 5;
        m.cpu.cpsr.set_flags(true, false, true, true);
        m.cpu.cpsr.set_q();
        m.step();
        assert_eq!((m.cpu.regs[1], m.cpu.regs[0]), (1, 15));
        assert_eq!(m.flags(), (true, false, true, true));
        assert!(m.cpu.cpsr.q());

        // RdHi and RdLo must be different registers.
        let mut m = Machine::new(&[0xE041_1392]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
    }

    #[test]
    fn armv6_dual_halfword_multiply_add_subtract_and_exchange() {
        // SMLAD r0,r1,r2,r3
        let mut m = Machine::new(&[0xE700_3211]);
        m.cpu.regs[1] = 0x0002_0003;
        m.cpu.regs[2] = 0x0004_0005;
        m.cpu.regs[3] = 7;
        m.step();
        assert_eq!(m.cpu.regs[0], 30);

        // SMLADX r4,r5,r6,r7 exchanges the two halfwords of Rs.
        let mut m = Machine::new(&[0xE704_7635]);
        m.cpu.regs[5] = 0x0002_0003;
        m.cpu.regs[6] = 0x0004_0005;
        m.cpu.regs[7] = 1;
        m.step();
        assert_eq!(m.cpu.regs[4], 23);

        // SMLSD r8,r9,r10,r11 computes low-product minus high-product.
        let mut m = Machine::new(&[0xE708_BA59]);
        m.cpu.regs[9] = 0x0002_0003;
        m.cpu.regs[10] = 0x0004_0005;
        m.cpu.regs[11] = 11;
        m.step();
        assert_eq!(m.cpu.regs[8], 18);

        // SMLSDX r0,r1,r2,r3 uses the exchanged second operand.
        let mut m = Machine::new(&[0xE700_3271]);
        m.cpu.regs[1] = 0x0002_0003;
        m.cpu.regs[2] = 0x0004_0005;
        m.cpu.regs[3] = 3;
        m.step();
        assert_eq!(m.cpu.regs[0], 5);
    }

    #[test]
    fn armv6_unaccumulated_dual_multiply_sets_q_only_on_add_overflow() {
        // SMUAD r4,r5,r6: two (-32768 * -32768) products overflow signed 32-bit.
        let mut m = Machine::new(&[0xE704_F615]);
        m.cpu.regs[5] = 0x8000_8000;
        m.cpu.regs[6] = 0x8000_8000;
        m.cpu.cpsr.set_flags(true, false, true, true);
        m.step();
        assert_eq!(m.cpu.regs[4], 0x8000_0000);
        assert!(m.cpu.cpsr.q());
        assert_eq!(m.flags(), (true, false, true, true));

        // SMUADX uses the exchanged halfwords for both products.
        let mut m = Machine::new(&[0xE707_F938]);
        m.cpu.regs[8] = 0x0002_0003;
        m.cpu.regs[9] = 0x0004_0005;
        m.step();
        assert_eq!(m.cpu.regs[7], 22);

        // SMUSD r10,r11,r12 has a bounded product difference and no Q update.
        let mut m = Machine::new(&[0xE70A_FC5B]);
        m.cpu.regs[11] = 0x0002_0003;
        m.cpu.regs[12] = 0x0004_0005;
        m.step();
        assert_eq!(m.cpu.regs[10], 7);
        assert!(!m.cpu.cpsr.q());

        // SMUSDX applies the same exchange before subtracting the products.
        let mut m = Machine::new(&[0xE700_F271]);
        m.cpu.regs[1] = 0x0002_0003;
        m.cpu.regs[2] = 0x0004_0005;
        m.step();
        assert_eq!(m.cpu.regs[0], 2);
        assert!(!m.cpu.cpsr.q());

        // SMLSD sets sticky Q if adding its product difference overflows.
        let mut m = Machine::new(&[0xE708_BA59]);
        m.cpu.regs[9] = 0x0002_0003;
        m.cpu.regs[10] = 0x0004_0005;
        m.cpu.regs[11] = i32::MAX as u32;
        m.step();
        assert_eq!(m.cpu.regs[8], 0x8000_0006);
        assert!(m.cpu.cpsr.q());
    }

    #[test]
    fn armv6_long_dual_halfword_multiply_accumulates_64_bits() {
        // SMLALD r0,r1,r2,r3, including carry from RdLo into RdHi.
        let mut m = Machine::new(&[0xE741_0312]);
        m.cpu.regs[0] = 0xFFFF_FFF0;
        m.cpu.regs[1] = 0;
        m.cpu.regs[2] = 0x0002_0003;
        m.cpu.regs[3] = 0x0004_0005;
        m.step();
        assert_eq!((m.cpu.regs[1], m.cpu.regs[0]), (1, 7));

        // SMLALDX r4,r5,r6,r7 uses exchanged halfwords.
        let mut m = Machine::new(&[0xE745_4736]);
        m.cpu.regs[6] = 0x0002_0003;
        m.cpu.regs[7] = 0x0004_0005;
        m.step();
        assert_eq!((m.cpu.regs[5], m.cpu.regs[4]), (0, 22));

        // SMLSLD r8,r9,r10,r11 adds (low product - high product).
        let mut m = Machine::new(&[0xE749_8B5A]);
        m.cpu.regs[8] = 10;
        m.cpu.regs[9] = 0;
        m.cpu.regs[10] = 0x0002_0003;
        m.cpu.regs[11] = 0x0004_0005;
        m.step();
        assert_eq!((m.cpu.regs[9], m.cpu.regs[8]), (0, 17));

        // SMLSLDX r0,r1,r2,r3 exchanges the second operand before subtraction.
        let mut m = Machine::new(&[0xE741_0372]);
        m.cpu.regs[0] = 10;
        m.cpu.regs[1] = 0;
        m.cpu.regs[2] = 0x0002_0003;
        m.cpu.regs[3] = 0x0004_0005;
        m.step();
        assert_eq!((m.cpu.regs[1], m.cpu.regs[0]), (0, 12));
    }

    #[test]
    fn armv6_most_significant_word_multiply_truncates_accumulates_and_rounds() {
        // SMMUL r4,r5,r6: high word of signed 1 * -2^31 is -1.
        let mut m = Machine::new(&[0xE754_F615]);
        m.cpu.regs[5] = 1;
        m.cpu.regs[6] = 0x8000_0000;
        m.step();
        assert_eq!(m.cpu.regs[4], u32::MAX);

        // SMMULR rounds the same product to zero.
        let mut m = Machine::new(&[0xE757_F938]);
        m.cpu.regs[8] = 1;
        m.cpu.regs[9] = 0x8000_0000;
        m.step();
        assert_eq!(m.cpu.regs[7], 0);

        // SMMLA r10,r11,r12,r0 adds the accumulator to the high product word.
        let mut m = Machine::new(&[0xE75A_0C1B]);
        m.cpu.regs[11] = 0x4000_0000;
        m.cpu.regs[12] = 0x4000_0000;
        m.cpu.regs[0] = 3;
        m.step();
        assert_eq!(m.cpu.regs[10], 0x1000_0003);

        // SMMLAR rounds before extraction, preserving the accumulator.
        let mut m = Machine::new(&[0xE751_4332]);
        m.cpu.regs[2] = 1;
        m.cpu.regs[3] = 0x8000_0000;
        m.cpu.regs[4] = 7;
        m.step();
        assert_eq!(m.cpu.regs[1], 7);

        // SMMLS includes the borrow from the discarded low half of the product.
        let mut m = Machine::new(&[0xE755_87D6]);
        m.cpu.regs[6] = 1;
        m.cpu.regs[7] = 1;
        m.cpu.regs[8] = 0;
        m.step();
        assert_eq!(m.cpu.regs[5], u32::MAX);

        // SMMLSR rounds the subtraction before extracting the high word.
        let mut m = Machine::new(&[0xE759_CBFA]);
        m.cpu.regs[10] = 1;
        m.cpu.regs[11] = 0x8000_0000;
        m.cpu.regs[12] = 0;
        m.step();
        assert_eq!(m.cpu.regs[9], 1);
    }

    #[test]
    fn armv6_signed_dsp_multiply_rejects_invalid_register_combinations() {
        // SMLAD cannot target PC.
        let mut m = Machine::new(&[0xE70F_3211]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // SMLALD cannot use the same register for the high and low result.
        let mut m = Machine::new(&[0xE741_1312]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // SMMUL cannot read or write PC.
        let mut m = Machine::new(&[0xE75F_F615]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // SMLABB cannot use PC as its accumulator.
        let mut m = Machine::new(&[0xE100_F281]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // SMULBB requires the reserved accumulator field to be zero.
        let mut m = Machine::new(&[0xE164_1685]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));

        // SMULWB also requires its reserved field to be zero.
        let mut m = Machine::new(&[0xE12B_10AC]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
    }

    #[test]
    fn armv6_ldrex_strex_word_succeeds_once() {
        // LDREX r0,[r1] ; STREX r2,r3,[r1] ; STREX r2,r3,[r1]
        let mut m = Machine::new(&[0xE191_0F9F, 0xE181_2F93, 0xE181_2F93]);
        m.cpu.regs[1] = DATA;
        m.cpu.regs[3] = 0xAABB_CCDD;
        m.mem.write32(DATA, 0x1122_3344).unwrap();

        m.step();
        assert_eq!(m.cpu.regs[0], 0x1122_3344);
        m.step();
        assert_eq!(m.cpu.regs[2], 0);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0xAABB_CCDD);

        m.cpu.regs[3] = 0x5566_7788;
        m.step();
        assert_eq!(m.cpu.regs[2], 1);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0xAABB_CCDD);
    }

    #[test]
    fn intervening_store_in_the_same_monitor_granule_breaks_strex() {
        // LDREX r0,[r1] ; STR r4,[r1,#4] ; STREX r2,r3,[r1]
        let mut m = Machine::new(&[0xE191_0F9F, 0xE581_4004, 0xE181_2F93]);
        m.cpu.regs[1] = DATA;
        m.cpu.regs[3] = 0xAABB_CCDD;
        m.cpu.regs[4] = 0x5566_7788;
        m.mem.write32(DATA, 0x1122_3344).unwrap();
        m.mem.write32(DATA + 4, 0).unwrap();

        m.run(3);

        assert_eq!(m.cpu.regs[2], 1);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x1122_3344);
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 0x5566_7788);
    }

    #[test]
    fn armv6k_byte_halfword_and_doubleword_exclusives_round_trip() {
        // LDREXB r4,[r5] ; STREXB r6,r7,[r5]
        let mut byte = Machine::new(&[0xE1D5_4F9F, 0xE1C5_6F97]);
        byte.cpu.regs[5] = DATA;
        byte.cpu.regs[7] = 0x1AB;
        byte.mem.write8(DATA, 0xFE).unwrap();
        byte.run(2);
        assert_eq!(byte.cpu.regs[4], 0xFE);
        assert_eq!(byte.cpu.regs[6], 0);
        assert_eq!(byte.mem.read8(DATA).unwrap(), 0xAB);

        // LDREXH r8,[r9] ; STREXH r10,r11,[r9]
        let mut half = Machine::new(&[0xE1F9_8F9F, 0xE1E9_AF9B]);
        half.cpu.regs[9] = DATA;
        half.cpu.regs[11] = 0x1234;
        half.mem.write16(DATA, 0xABCD).unwrap();
        half.run(2);
        assert_eq!(half.cpu.regs[8], 0xABCD);
        assert_eq!(half.cpu.regs[10], 0);
        assert_eq!(half.mem.read16(DATA).unwrap(), 0x1234);

        // LDREXD r0,r1,[r2] ; STREXD r3,r4,r5,[r2]
        let mut double = Machine::new(&[0xE1B2_0F9F, 0xE1A2_3F94]);
        double.cpu.regs[2] = DATA;
        double.cpu.regs[4] = 0xAABB_CCDD;
        double.cpu.regs[5] = 0x1122_3344;
        double.mem.write32(DATA, 0x0102_0304).unwrap();
        double.mem.write32(DATA + 4, 0x0506_0708).unwrap();
        double.run(2);
        assert_eq!(double.cpu.regs[0], 0x0102_0304);
        assert_eq!(double.cpu.regs[1], 0x0506_0708);
        assert_eq!(double.cpu.regs[3], 0);
        assert_eq!(double.mem.read32(DATA).unwrap(), 0xAABB_CCDD);
        assert_eq!(double.mem.read32(DATA + 4).unwrap(), 0x1122_3344);
    }

    #[test]
    fn clrex_clears_the_outstanding_exclusive_reservation() {
        // LDREX r0,[r1] ; CLREX ; STREX r2,r3,[r1]
        let mut m = Machine::new(&[0xE191_0F9F, 0xF57F_F01F, 0xE181_2F93]);
        m.cpu.regs[1] = DATA;
        m.cpu.regs[3] = 0xAABB_CCDD;
        m.mem.write32(DATA, 0x1122_3344).unwrap();

        m.run(3);

        assert_eq!(m.cpu.regs[2], 1);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x1122_3344);
    }

    #[test]
    fn strex_to_a_different_address_fails_without_writing() {
        // LDREX r0,[r1] ; STREX r2,r3,[r4]
        let mut m = Machine::new(&[0xE191_0F9F, 0xE184_2F93]);
        m.cpu.regs[1] = DATA;
        m.cpu.regs[4] = DATA + 8;
        m.cpu.regs[3] = 0xAABB_CCDD;
        m.mem.write32(DATA, 0x1122_3344).unwrap();
        m.mem.write32(DATA + 8, 0x5566_7788).unwrap();

        m.run(2);

        assert_eq!(m.cpu.regs[2], 1);
        assert_eq!(m.mem.read32(DATA + 8).unwrap(), 0x5566_7788);
    }

    #[test]
    fn exclusive_word_accesses_observe_big_endian_data_order() {
        // LDREX r0,[r1] ; STREX r2,r3,[r1]
        let mut m = Machine::new(&[0xE191_0F9F, 0xE181_2F93]);
        m.cpu.regs[1] = DATA;
        m.cpu.regs[3] = 0xAABB_CCDD;
        m.cpu.cpsr.set_big_endian(true);
        m.mem.write32(DATA, 0x1122_3344).unwrap();

        m.run(2);

        assert_eq!(m.cpu.regs[0], 0x4433_2211);
        assert_eq!(m.cpu.regs[2], 0);
        assert_eq!(
            [
                m.mem.read8(DATA).unwrap(),
                m.mem.read8(DATA + 1).unwrap(),
                m.mem.read8(DATA + 2).unwrap(),
                m.mem.read8(DATA + 3).unwrap(),
            ],
            [0xAA, 0xBB, 0xCC, 0xDD]
        );
    }

    #[test]
    fn exclusive_instructions_reject_invalid_register_combinations() {
        for instruction in [
            0xE1B1_1F9F, // LDREXD with odd Rt
            0xE1B1_EF9F, // LDREXD with Rt == LR
            0xE181_1F93, // STREX Rd == Rn
            0xE181_2F92, // STREX Rd == Rt
            0xE1A2_5F94, // STREXD Rd == Rt2
        ] {
            let mut m = Machine::new(&[instruction]);
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, .. })
            ));
        }
    }

    #[test]
    fn exclusive_instructions_reject_misaligned_addresses() {
        for (instruction, address) in [
            (0xE191_0F9F, DATA + 2), // LDREX needs word alignment
            (0xE1F9_8F9F, DATA + 1), // LDREXH needs halfword alignment
            (0xE1B2_0F9F, DATA + 4), // LDREXD needs doubleword alignment
        ] {
            let mut m = Machine::new(&[instruction]);
            m.cpu.regs[1] = address;
            m.cpu.regs[2] = address;
            m.cpu.regs[9] = address;
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::InvalidAccess { .. })
            ));
        }
    }

    // ----- multiply, CLZ, status, SWI ---------------------------------------

    #[test]
    fn mul_and_mla() {
        // MUL r0,r1,r2 ; MLA r4,r1,r2,r3
        let mut m = Machine::new(&[0xE000_0291, 0xE024_3291]);
        m.cpu.regs[1] = 6;
        m.cpu.regs[2] = 7;
        m.cpu.regs[3] = 100;
        m.run(2);
        assert_eq!(m.cpu.regs[0], 42);
        assert_eq!(m.cpu.regs[4], 142);
    }

    #[test]
    fn muls_sets_zero_flag() {
        // MULS r0,r1,r2
        let mut m = Machine::new(&[0xE010_0291]);
        m.cpu.regs[2] = 5;
        m.step();
        assert_eq!(m.cpu.regs[0], 0);
        assert!(m.cpu.cpsr.z());
    }

    #[test]
    fn long_multiplies_produce_a_64_bit_result() {
        // UMULL r0,r1,r2,r3 ; SMULL r4,r5,r2,r3
        let mut m = Machine::new(&[0xE081_0392, 0xE0C5_4392]);
        m.cpu.regs[2] = 0xFFFF_FFFF;
        m.cpu.regs[3] = 2;
        m.run(2);
        assert_eq!((m.cpu.regs[0], m.cpu.regs[1]), (0xFFFF_FFFE, 1));
        assert_eq!((m.cpu.regs[4], m.cpu.regs[5]), (0xFFFF_FFFE, 0xFFFF_FFFF));
    }

    #[test]
    fn multiply_and_long_multiply_reject_unpredictable_registers() {
        let encode_short = |accumulate: bool, rd: u32, rn: u32, rs: u32, rm: u32| {
            0xE000_0090
                | if accumulate { 1 << 21 } else { 0 }
                | (rd << 16)
                | (rn << 12)
                | (rs << 8)
                | rm
        };

        // MUL: Rn is SBZ; Rd, Rs and Rm cannot be PC.
        // MLA: each of Rd, Rn, Rs and Rm must be a general-purpose register.
        for (accumulate, rd, rn, rs, rm) in [
            (false, 15, 0, 2, 1),
            (false, 0, 0, 15, 1),
            (false, 0, 0, 2, 15),
            (false, 0, 1, 2, 1),
            (true, 15, 3, 2, 1),
            (true, 0, 15, 2, 1),
            (true, 0, 3, 15, 1),
            (true, 0, 3, 2, 15),
        ] {
            let word = encode_short(accumulate, rd, rn, rs, rm);
            let mut m = Machine::new(&[word]);
            let before_regs = m.cpu.regs;
            let before_cpsr = m.cpu.cpsr;
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, .. })
            ));
            assert_eq!(m.cpu.regs, before_regs);
            assert_eq!(m.cpu.cpsr, before_cpsr);
        }

        let encode_long =
            |signed: bool, accumulate: bool, rd_hi: u32, rd_lo: u32, rs: u32, rm: u32| {
                0xE080_0090
                    | if signed { 1 << 22 } else { 0 }
                    | if accumulate { 1 << 21 } else { 0 }
                    | (rd_hi << 16)
                    | (rd_lo << 12)
                    | (rs << 8)
                    | rm
            };

        // UMULL/UMLAL/SMULL/SMLAL reject PC operands and aliased destinations.
        for (signed, accumulate, rd_hi, rd_lo, rs, rm) in [
            (false, false, 0, 0, 3, 2),
            (false, false, 15, 0, 3, 2),
            (false, false, 1, 15, 3, 2),
            (false, false, 1, 0, 15, 2),
            (false, false, 1, 0, 3, 15),
            (true, true, 1, 0, 15, 2),
        ] {
            let word = encode_long(signed, accumulate, rd_hi, rd_lo, rs, rm);
            let mut m = Machine::new(&[word]);
            let before_regs = m.cpu.regs;
            let before_cpsr = m.cpu.cpsr;
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, .. })
            ));
            assert_eq!(m.cpu.regs, before_regs);
            assert_eq!(m.cpu.cpsr, before_cpsr);
        }
    }

    #[test]
    fn armv6_multiply_allows_destination_source_overlap() {
        // ARMv6 permits Rd == Rm for MUL and RdLo == Rm for UMULL.
        let mut m = Machine::new(&[0xE001_0291, 0xE082_0390]);
        m.cpu.regs[1] = 6;
        m.cpu.regs[2] = 7;
        m.cpu.regs[0] = u32::MAX;
        m.cpu.regs[3] = 2;

        m.run(2);

        assert_eq!(m.cpu.regs[1], 42);
        assert_eq!(m.cpu.regs[0], 0xFFFF_FFFE);
        assert_eq!(m.cpu.regs[2], 1);
    }

    #[test]
    fn clz_counts_leading_zeros() {
        // CLZ r0,r1
        let mut m = Machine::new(&[0xE16F_0F11, 0xE16F_0F11]);
        m.cpu.regs[1] = 0x00F0_0000;
        m.step();
        assert_eq!(m.cpu.regs[0], 8);
        m.cpu.regs[1] = 0;
        m.step();
        assert_eq!(m.cpu.regs[0], 32);
    }

    #[test]
    fn msr_and_mrs_move_the_condition_flags() {
        // MSR cpsr_f,r0 ; MRS r1,cpsr
        let mut m = Machine::new(&[0xE128_F000, 0xE10F_1000]);
        m.cpu.regs[0] = 0xF000_0000;
        m.run(2);
        assert_eq!(m.flags(), (true, true, true, true));
        assert_eq!(m.cpu.regs[1] >> 28, 0xF);
    }

    #[test]
    fn swi_is_intercepted_for_hle_service_dispatch_by_default() {
        let mut m = Machine::new(&[0xEF12_3456]);
        assert_eq!(m.step(), StepEvent::Svc(0x12_3456));
        assert_eq!(m.cpu.pc(), 0x04);
        assert_eq!(m.cpu.cpsr.mode(), Psr::USER_MODE);
        assert_eq!(m.cpu.saved_program_status(), None);
    }

    #[test]
    fn swi_enters_supervisor_mode_when_hardware_exception_entry_is_enabled() {
        let mut m = Machine::new(&[0xEF12_3456]);
        let original_cpsr = m.cpu.cpsr;
        m.cpu.set_svc_exception_entry_enabled(true);

        assert_eq!(m.step(), StepEvent::Svc(0x12_3456));
        assert_eq!(m.cpu.pc(), 0x08);
        assert_eq!(m.cpu.regs[LR], 0x04);
        assert_eq!(m.cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert!(!m.cpu.cpsr.thumb());
        assert_ne!(m.cpu.cpsr.0 & (1 << 7), 0);
        assert_eq!(m.cpu.saved_program_status(), Some(original_cpsr));
    }

    #[test]
    fn swi_exception_returns_to_the_instruction_after_the_a32_call() {
        // The low SVC vector contains MOVS pc,lr.
        let mut m = Machine::new(&[0xEF00_0001, 0xE1A0_0000, 0xE1B0_F00E]);
        let original_cpsr = m.cpu.cpsr;
        m.cpu.set_svc_exception_entry_enabled(true);

        assert_eq!(m.step(), StepEvent::Svc(1));
        assert_eq!(m.cpu.pc(), 0x08);
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.pc(), 0x04);
        assert_eq!(m.cpu.cpsr, original_cpsr);
    }

    #[test]
    fn swi_uses_configured_high_vectors_and_exception_endianness() {
        let mut m = Machine::new(&[0xEF00_0001]);
        m.cpu.set_svc_exception_entry_enabled(true);
        m.cpu.configure_exception_entry(true, true);

        assert_eq!(m.step(), StepEvent::Svc(1));
        assert_eq!(m.cpu.pc(), 0xFFFF_0008);
        assert_ne!(m.cpu.cpsr.0 & (1 << 9), 0);
    }

    #[test]
    fn ldm_stm_with_s_bit_access_the_user_bank_registers() {
        let mut m = Machine::new(&[0xE8C0_6000, 0xE8D0_6000]);
        m.cpu.regs[13] = 0x1111;
        m.cpu.regs[14] = 0x2222;
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.regs[13] = 0x5555; // SVC sp stays untouched
        m.cpu.regs[0] = DATA;
        // STMIA r0, {sp, lr}^ stores the User registers.
        m.step();
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x1111);
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 0x2222);
        // LDMIA r0, {sp, lr}^ replaces them without touching the SVC bank.
        m.mem.write32(DATA, 0xAAAA).unwrap();
        m.mem.write32(DATA + 4, 0xBBBB).unwrap();
        m.step();
        assert_eq!(m.cpu.regs[13], 0x5555);
        m.cpu.switch_mode(Psr::USER_MODE).unwrap();
        assert_eq!((m.cpu.regs[13], m.cpu.regs[14]), (0xAAAA, 0xBBBB));
    }

    #[test]
    fn ldm_with_s_bit_and_pc_returns_from_the_exception() {
        let mut m = Machine::new(&[0xE8D0_8002]);
        m.cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        m.cpu.set_saved_program_status(Psr(0x6000_0010)).unwrap();
        m.cpu.regs[0] = DATA;
        m.mem.write32(DATA, 0x77).unwrap();
        m.mem.write32(DATA + 4, 0x41).unwrap();
        m.step();
        assert_eq!(m.cpu.regs[1], 0x77);
        assert_eq!(m.cpu.pc(), 0x40);
        assert_eq!(m.cpu.cpsr, Psr(0x6000_0010));
    }

    #[test]
    fn ldm_stm_s_bit_is_undefined_where_it_has_no_meaning() {
        // STM^ in User mode, and STM^ with writeback in a privileged mode.
        for (program, mode) in [
            (0xE8C0_6000, Psr::USER_MODE),
            (0xE8E0_6000, Psr::SUPERVISOR_MODE),
        ] {
            let mut m = Machine::new(&[program]);
            m.cpu.switch_mode(mode).unwrap();
            m.cpu.regs[0] = DATA;
            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { .. })
            ));
        }
        // The exception-return form needs an SPSR, so System mode rejects it.
        let mut m = Machine::new(&[0xE8D0_8002]);
        m.cpu.switch_mode(Psr::SYSTEM_MODE).unwrap();
        m.cpu.regs[0] = DATA;
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { .. })
        ));
    }

    #[test]
    fn empty_arm_block_transfer_lists_are_undefined() {
        for instruction in [0xE880_0000, 0xE890_0000] {
            let mut machine = Machine::new(&[instruction]);
            machine.cpu.regs[0] = DATA;
            assert!(
                matches!(
                    machine.cpu.step(&mut machine.mem),
                    Err(Error::UndefinedInstruction { pc: 0, word })
                        if word == instruction
                ),
                "empty ARM LDM/STM list {instruction:#010x} must be rejected as undefined"
            );
            assert_eq!(machine.cpu.pc(), 0);
            assert_eq!(machine.cpu.regs[0], DATA);
        }
    }

    #[test]
    fn coprocessor_instructions_are_not_implemented_yet() {
        let mut m = Machine::new(&[0xEE00_0000]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::Unimplemented(_))
        ));
    }
}
