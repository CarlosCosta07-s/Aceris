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

impl Arm11Core {
    pub(super) fn execute_arm<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        let condition = (word >> 28) as u8;
        if condition == 0xF {
            return self.execute_unconditional(word);
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
            0b110 => Err(Error::Unimplemented("coprocessor load/store")),
            _ if bit(word, 24) => Ok(StepEvent::Svc(bits(word, 23, 0))),
            _ => Err(Error::Unimplemented("coprocessor instructions")),
        }
    }

    /// Executes the ARMv6 scalar media instructions implemented by this core.
    fn execute_media_instruction(&mut self, word: u32) -> Result<StepEvent> {
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
        } else if word & 0x0FFF_03F0 == 0x06AF_0070
            || word & 0x0FFF_03F0 == 0x06BF_0070
            || word & 0x0FFF_03F0 == 0x06EF_0070
            || word & 0x0FFF_03F0 == 0x06FF_0070
        {
            // SXTB, SXTH, UXTB and UXTH allow a byte rotation before extraction.
            let rd = bits(word, 15, 12) as usize;
            let rm = bits(word, 3, 0) as usize;
            if rd == PC || rm == PC {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word,
                });
            }
            let rotated = self.regs[rm].rotate_right(bits(word, 11, 10) * 8);
            let value = match word & 0x0FFF_03F0 {
                0x06AF_0070 => i32::from(rotated as u8 as i8) as u32,
                0x06BF_0070 => i32::from(rotated as u16 as i16) as u32,
                0x06EF_0070 => u32::from(rotated as u8),
                0x06FF_0070 => u32::from(rotated as u16),
                _ => unreachable!("the media extension encodings were checked above"),
            };
            self.regs[rd] = value;
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
            return Err(Error::Unimplemented("ARMv6 media instruction"));
        }

        Ok(StepEvent::Executed)
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

    fn execute_unconditional(&mut self, word: u32) -> Result<StepEvent> {
        if bits(word, 27, 25) != 0b101 {
            return Err(Error::Unimplemented("unconditional-space instruction"));
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
        self.data_processing(word);
        Ok(StepEvent::Executed)
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

    fn data_processing(&mut self, word: u32) {
        let opcode = bits(word, 24, 21);
        let set_flags = bit(word, 20);
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);
        let carry_in = self.cpsr.c();

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
        let is_compare = (0x8..=0xB).contains(&opcode);

        // `S` with `rd == pc` would restore the CPSR from the SPSR, which only
        // exists in privileged modes; in user mode the flags are left alone.
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
    }

    fn multiply_or_swap<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        if bits(word, 27, 22) == 0 {
            self.multiply(word);
        } else if bits(word, 27, 23) == 0b00001 {
            self.multiply_long(word);
        } else if bits(word, 27, 23) == 0b00010 && bits(word, 21, 20) == 0 {
            self.swap(bus, word)?;
        } else {
            return Err(Error::Unimplemented("exclusive load/store"));
        }
        Ok(StepEvent::Executed)
    }

    fn multiply(&mut self, word: u32) {
        let rd = bits(word, 19, 16);
        let rn = bits(word, 15, 12);
        let rs = bits(word, 11, 8);
        let rm = bits(word, 3, 0);

        let mut result = self.reg(rm, 8).wrapping_mul(self.reg(rs, 8));
        if bit(word, 21) {
            result = result.wrapping_add(self.reg(rn, 8));
        }
        self.write_reg(rd, result);
        if bit(word, 20) {
            self.set_nz(result);
        }
    }

    fn multiply_long(&mut self, word: u32) {
        let signed = bit(word, 22);
        let accumulate = bit(word, 21);
        let rd_hi = bits(word, 19, 16);
        let rd_lo = bits(word, 15, 12);
        let rs = bits(word, 11, 8);
        let rm = bits(word, 3, 0);

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
    }

    fn swap<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<()> {
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);
        let rm = bits(word, 3, 0);
        let addr = self.reg(rn, 8);
        let new_value = self.reg(rm, 8);

        let old = if bit(word, 22) {
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

        if word & 0x0FF0_00F0 == 0x0120_0070 {
            // BKPT #imm16; exception-vector entry is not modeled yet.
            let immediate = ((bits(word, 19, 8) << 4) | bits(word, 3, 0)) as u16;
            return Ok(StepEvent::Breakpoint(immediate));
        } else if word & 0x0FFF_FFF0 == 0x012F_FF10 {
            // BX Rm
            let target = self.reg(rm, 8);
            self.branch_exchange(target);
        } else if word & 0x0FFF_FFF0 == 0x012F_FF30 {
            // BLX Rm
            let target = self.reg(rm, 8);
            self.regs[LR] = self.pc().wrapping_add(4);
            self.branch_exchange(target);
        } else if word & 0x0FFF_0FF0 == 0x016F_0F10 {
            // CLZ Rd, Rm
            let value = self.reg(rm, 8).leading_zeros();
            self.write_reg(rd, value);
        } else if word & 0x0FBF_0FFF == 0x010F_0000 {
            // MRS Rd, CPSR|SPSR
            if bit(word, 22) {
                return Err(Error::Unimplemented("SPSR access"));
            }
            self.write_reg(rd, self.cpsr.0);
        } else if word & 0x0FB0_FFF0 == 0x0120_F000 {
            // MSR CPSR|SPSR_<fields>, Rm
            let value = self.reg(rm, 8);
            self.write_status(word, value)?;
        } else if word & 0x0FB0_F000 == 0x0320_F000 {
            // MSR CPSR|SPSR_<fields>, #imm
            let value = bits(word, 7, 0).rotate_right(bits(word, 11, 8) * 2);
            self.write_status(word, value)?;
        } else {
            return Err(Error::Unimplemented("miscellaneous instruction"));
        }
        Ok(StepEvent::Executed)
    }

    /// Applies an MSR write. In user mode only the condition flags are writable.
    fn write_status(&mut self, word: u32, value: u32) -> Result<()> {
        if bit(word, 22) {
            return Err(Error::Unimplemented("SPSR access"));
        }
        if bit(word, 19) {
            self.cpsr.set_flags(
                bit(value, 31),
                bit(value, 30),
                bit(value, 29),
                bit(value, 28),
            );
        }
        Ok(())
    }

    // ----- loads and stores -------------------------------------------------

    fn single_transfer<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        let pre_index = bit(word, 24);
        let up = bit(word, 23);
        let byte = bit(word, 22);
        let writeback = !pre_index || bit(word, 21);
        let load = bit(word, 20);
        let rn = bits(word, 19, 16);
        let rd = bits(word, 15, 12);

        let offset = if bit(word, 25) {
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

        if load {
            let value = if byte {
                u32::from(bus.read8(addr)?)
            } else {
                // Classic behaviour: unaligned word loads rotate the aligned word.
                self.read_data_u32_unaligned(bus, addr)?
            };
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
                bus.write8(addr, value as u8)?;
            } else {
                self.write_data_u32(bus, addr & !3, value)?;
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

        match (load, bits(word, 6, 5)) {
            (true, 0b01) => {
                let value = u32::from(self.read_data_u16(bus, addr)?);
                self.finish_load(rn, rd, writeback, offset_addr, value);
            }
            (true, 0b10) => {
                let value = i32::from(bus.read8(addr)? as i8) as u32;
                self.finish_load(rn, rd, writeback, offset_addr, value);
            }
            (true, 0b11) => {
                let value = i32::from(self.read_data_u16(bus, addr)? as i16) as u32;
                self.finish_load(rn, rd, writeback, offset_addr, value);
            }
            (false, 0b01) => {
                let value = self.reg(rd, 8) as u16;
                self.write_data_u16(bus, addr, value)?;
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

    fn block_transfer<B: Bus + ?Sized>(&mut self, bus: &mut B, word: u32) -> Result<StepEvent> {
        let pre_index = bit(word, 24);
        let up = bit(word, 23);
        let writeback = bit(word, 21);
        let load = bit(word, 20);
        let rn = bits(word, 19, 16);
        let list = bits(word, 15, 0);

        if bit(word, 22) {
            return Err(Error::Unimplemented("LDM/STM with the S bit"));
        }
        if list == 0 {
            return Err(Error::Unimplemented("LDM/STM with an empty register list"));
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

        let mut addr = start;
        if load {
            let mut values = [0u32; 16];
            for (r, value) in values.iter_mut().enumerate() {
                if list & (1 << r) != 0 {
                    *value = self.read_data_u32(bus, addr)?;
                    addr = addr.wrapping_add(4);
                }
            }
            if writeback {
                self.set_base(rn, new_base);
            }
            for (r, value) in values.iter().enumerate().take(PC) {
                if list & (1 << r) != 0 {
                    self.regs[r] = *value;
                }
            }
            if list & (1 << PC) != 0 {
                self.branch_exchange(values[PC]);
            }
        } else {
            for r in 0..16u32 {
                if list & (1 << r) != 0 {
                    self.write_data_u32(bus, addr, self.reg(r, 8))?;
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
    use crate::cpu::Psr;
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
    fn ldrd_with_an_odd_register_is_undefined() {
        // LDRD r1,[r2]
        let mut m = Machine::new(&[0xE1C2_10D0]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
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
            0xE6EF_0C71, // UXTB r3,r1,ror #24
            0xE6FF_4471, // UXTH r4,r1,ror #8
            0xE6AF_5871, // SXTB r5,r1,ror #16 (selects 0xFE)
        ]);
        m.cpu.regs[1] = 0x80FE_1234;
        m.run(5);

        assert_eq!(m.cpu.regs[0], 0x0000_0012);
        assert_eq!(m.cpu.regs[2], 0x0000_80FE);
        assert_eq!(m.cpu.regs[3], 0x0000_0080);
        assert_eq!(m.cpu.regs[4], 0x0000_FE12);
        assert_eq!(m.cpu.regs[5], 0xFFFF_FFFE);
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
    fn swi_reports_its_comment_field() {
        let mut m = Machine::new(&[0xEF12_3456]);
        assert_eq!(m.step(), StepEvent::Svc(0x12_3456));
        assert_eq!(m.cpu.pc(), 4);
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
