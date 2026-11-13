//! Thumb (16-bit) instruction decoding and execution.
//!
//! Covers the ARMv6 Thumb formats and ARMv6 additions supported by the
//! ARM11 target, including `BL`/`BLX`, `CPS`, `CPY`, `SETEND`, `BKPT`,
//! `REV*`, `SXT*` and `UXT*`. Thumb-2 and later Thumb hint encodings are
//! rejected. Reading `pc` as an operand yields the instruction address plus 4.

use super::arm::{bit, bits};
use super::shifter::{ShiftKind, add_with_carry, shift_imm, shift_reg};
use super::{Arm11Core, Condition, LR, PC, SP, StepEvent};
use crate::memory::Bus;
use aceris_common::{Error, Result};

/// Offset applied when a Thumb instruction reads `pc`.
const PC_OFFSET: u32 = 4;

/// Word load with the classic unaligned-rotate behaviour.
fn load_word<B: Bus + ?Sized>(
    core: &Arm11Core,
    bus: &mut B,
    addr: u32,
    instruction: u32,
) -> Result<u32> {
    core.read_armv6_single_word(bus, addr, false, instruction)
}

impl Arm11Core {
    pub(super) fn execute_thumb<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        half: u32,
    ) -> Result<StepEvent> {
        match bits(half, 15, 13) {
            0b000 if bits(half, 12, 11) == 0b11 => self.thumb_add_sub(half),
            0b000 => self.thumb_shift_immediate(half),
            0b001 => self.thumb_immediate(half),
            0b010 => match bits(half, 12, 10) {
                0b000 => self.thumb_alu(half),
                0b001 => self.thumb_hi_register(half)?,
                0b010 | 0b011 => self.thumb_pc_relative_load(bus, half)?,
                _ => self.thumb_register_offset(bus, half)?,
            },
            0b011 => self.thumb_immediate_offset(bus, half)?,
            0b100 if !bit(half, 12) => self.thumb_halfword_offset(bus, half)?,
            0b100 => self.thumb_sp_relative(bus, half)?,
            0b101 if !bit(half, 12) => self.thumb_load_address(half),
            0b101 => return self.thumb_misc(bus, half),
            0b110 if !bit(half, 12) => self.thumb_block_transfer(bus, half)?,
            0b110 => return self.thumb_conditional_branch(half),
            _ => self.thumb_branch(half)?,
        }
        Ok(StepEvent::Executed)
    }

    // ----- flag helpers -----------------------------------------------------

    /// Sets N and Z from `value`; C and V change only when given.
    fn update_flags(&mut self, value: u32, carry: Option<bool>, overflow: Option<bool>) {
        let c = carry.unwrap_or(self.cpsr.c());
        let v = overflow.unwrap_or(self.cpsr.v());
        self.cpsr.set_flags(value >> 31 != 0, value == 0, c, v);
    }

    /// `a + b + carry_in` with full flag update, optionally storing to `rd`.
    fn thumb_arith(&mut self, rd: Option<usize>, a: u32, b: u32, carry_in: bool) {
        let (value, carry, overflow) = add_with_carry(a, b, carry_in);
        if let Some(rd) = rd {
            self.regs[rd] = value;
        }
        self.update_flags(value, Some(carry), Some(overflow));
    }

    /// Writes a result that may target `pc` (hi-register forms); no interworking.
    fn thumb_write_hi(&mut self, rd: u32, value: u32) {
        if rd as usize == PC {
            self.branch(value & !1);
        } else {
            self.regs[rd as usize] = value;
        }
    }

    // ----- data processing --------------------------------------------------

    fn thumb_shift_immediate(&mut self, half: u32) {
        let kind = match bits(half, 12, 11) {
            0 => ShiftKind::Lsl,
            1 => ShiftKind::Lsr,
            _ => ShiftKind::Asr,
        };
        let rs = bits(half, 5, 3) as usize;
        let rd = bits(half, 2, 0) as usize;
        let (result, carry) = shift_imm(kind, self.regs[rs], bits(half, 10, 6), self.cpsr.c());
        self.regs[rd] = result;
        self.update_flags(result, Some(carry), None);
    }

    fn thumb_add_sub(&mut self, half: u32) {
        let field = bits(half, 8, 6);
        let b = if bit(half, 10) {
            field
        } else {
            self.regs[field as usize]
        };
        let a = self.regs[bits(half, 5, 3) as usize];
        let rd = bits(half, 2, 0) as usize;
        if bit(half, 9) {
            self.thumb_arith(Some(rd), a, !b, true);
        } else {
            self.thumb_arith(Some(rd), a, b, false);
        }
    }

    fn thumb_immediate(&mut self, half: u32) {
        let rd = bits(half, 10, 8) as usize;
        let imm = bits(half, 7, 0);
        let a = self.regs[rd];
        match bits(half, 12, 11) {
            0 => {
                self.regs[rd] = imm;
                self.update_flags(imm, None, None);
            }
            1 => self.thumb_arith(None, a, !imm, true),
            2 => self.thumb_arith(Some(rd), a, imm, false),
            _ => self.thumb_arith(Some(rd), a, !imm, true),
        }
    }

    fn thumb_alu(&mut self, half: u32) {
        let op = bits(half, 9, 6);
        let rs = bits(half, 5, 3) as usize;
        let rd = bits(half, 2, 0) as usize;
        let (a, b) = (self.regs[rd], self.regs[rs]);
        let carry = self.cpsr.c();

        match op {
            0x0 => self.thumb_logical(rd, a & b),
            0x1 => self.thumb_logical(rd, a ^ b),
            0x2 => self.thumb_shift(rd, ShiftKind::Lsl, a, b),
            0x3 => self.thumb_shift(rd, ShiftKind::Lsr, a, b),
            0x4 => self.thumb_shift(rd, ShiftKind::Asr, a, b),
            0x5 => self.thumb_arith(Some(rd), a, b, carry),
            0x6 => self.thumb_arith(Some(rd), a, !b, carry),
            0x7 => self.thumb_shift(rd, ShiftKind::Ror, a, b),
            // TST: flags only.
            0x8 => self.update_flags(a & b, None, None),
            // NEG (RSB with zero).
            0x9 => self.thumb_arith(Some(rd), 0, !b, true),
            0xA => self.thumb_arith(None, a, !b, true),
            0xB => self.thumb_arith(None, a, b, false),
            0xC => self.thumb_logical(rd, a | b),
            0xD => self.thumb_logical(rd, a.wrapping_mul(b)),
            0xE => self.thumb_logical(rd, a & !b),
            _ => self.thumb_logical(rd, !b),
        }
    }

    /// Stores `value` to `rd` and sets N and Z.
    fn thumb_logical(&mut self, rd: usize, value: u32) {
        self.regs[rd] = value;
        self.update_flags(value, None, None);
    }

    fn thumb_shift(&mut self, rd: usize, kind: ShiftKind, value: u32, amount: u32) {
        let (result, carry) = shift_reg(kind, value, amount & 0xFF, self.cpsr.c());
        self.regs[rd] = result;
        self.update_flags(result, Some(carry), None);
    }

    fn thumb_hi_register(&mut self, half: u32) -> Result<()> {
        let rd = bits(half, 2, 0) | (u32::from(bit(half, 7)) << 3);
        let rs = bits(half, 5, 3) | (u32::from(bit(half, 6)) << 3);
        let op = bits(half, 9, 8);

        // High-register ADD (T2) and CMP (T3) require at least one
        // high register. The low/low encodings are UNPREDICTABLE.
        // MOV (T3) is deliberately excluded: low/low is CPY in ARMv6.
        if op <= 1 && !bit(half, 7) && !bit(half, 6) {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }

        // CMP (3) permits PC as Rm, but not as its first operand Rn.
        if op == 1 && rd as usize == PC {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }

        let a = self.reg(rd, PC_OFFSET);
        let b = self.reg(rs, PC_OFFSET);

        match op {
            0 => self.thumb_write_hi(rd, a.wrapping_add(b)),
            1 => self.thumb_arith(None, a, !b, true),
            2 => self.thumb_write_hi(rd, b),
            _ => {
                // BX/BLX reserve the low three destination bits as zero.
                // BLX cannot use PC as Rm. A target with bits[1:0] == 0b10
                // would enter ARM state at a non-word-aligned address.
                let link = bit(half, 7);
                if bits(half, 2, 0) != 0 || (link && rs as usize == PC) || (b & 3 == 2) {
                    return Err(Error::UndefinedInstruction {
                        pc: self.pc(),
                        word: half,
                    });
                }

                if link {
                    self.regs[LR] = self.pc().wrapping_add(2) | 1;
                }
                self.branch_exchange(b);
            }
        }
        Ok(())
    }

    // ----- loads and stores -------------------------------------------------

    fn thumb_pc_relative_load<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let rd = bits(half, 10, 8) as usize;
        let base = self.pc().wrapping_add(PC_OFFSET) & !3;
        self.regs[rd] = self.read_data_u32(bus, base.wrapping_add(bits(half, 7, 0) * 4))?;
        Ok(())
    }

    fn thumb_register_offset<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let rm = bits(half, 8, 6) as usize;
        let rn = bits(half, 5, 3) as usize;
        let rt = bits(half, 2, 0) as usize;
        let addr = self.regs[rn].wrapping_add(self.regs[rm]);

        match bits(half, 11, 9) {
            0b000 => self.write_armv6_single_word(bus, addr, self.regs[rt], false, half)?,
            0b001 => self.write_armv6_halfword(bus, addr, self.regs[rt] as u16, half)?,
            0b010 => bus.write8(addr, self.regs[rt] as u8)?,
            0b011 => self.regs[rt] = i32::from(bus.read8(addr)? as i8) as u32,
            0b100 => self.regs[rt] = load_word(self, bus, addr, half)?,
            0b101 => {
                self.regs[rt] = u32::from(self.read_armv6_halfword(bus, addr, half)?);
            }
            0b110 => self.regs[rt] = u32::from(bus.read8(addr)?),
            _ => {
                self.regs[rt] = i32::from(self.read_armv6_halfword(bus, addr, half)? as i16) as u32;
            }
        }
        Ok(())
    }

    fn thumb_immediate_offset<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let byte = bit(half, 12);
        let load = bit(half, 11);
        let imm = bits(half, 10, 6);
        let rn = bits(half, 5, 3) as usize;
        let rt = bits(half, 2, 0) as usize;
        let addr = self.regs[rn].wrapping_add(if byte { imm } else { imm * 4 });

        if load {
            self.regs[rt] = if byte {
                u32::from(bus.read8(addr)?)
            } else {
                load_word(self, bus, addr, half)?
            };
        } else if byte {
            bus.write8(addr, self.regs[rt] as u8)?;
        } else {
            self.write_armv6_single_word(bus, addr, self.regs[rt], false, half)?;
        }
        Ok(())
    }

    fn thumb_halfword_offset<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let rn = bits(half, 5, 3) as usize;
        let rt = bits(half, 2, 0) as usize;
        let addr = self.regs[rn].wrapping_add(bits(half, 10, 6) * 2);

        if bit(half, 11) {
            self.regs[rt] = u32::from(self.read_armv6_halfword(bus, addr, half)?);
        } else {
            self.write_armv6_halfword(bus, addr, self.regs[rt] as u16, half)?;
        }
        Ok(())
    }

    fn thumb_sp_relative<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let rt = bits(half, 10, 8) as usize;
        let addr = self.regs[SP].wrapping_add(bits(half, 7, 0) * 4);

        if bit(half, 11) {
            self.regs[rt] = load_word(self, bus, addr, half)?;
        } else {
            self.write_armv6_single_word(bus, addr, self.regs[rt], false, half)?;
        }
        Ok(())
    }

    fn thumb_load_address(&mut self, half: u32) {
        let rd = bits(half, 10, 8) as usize;
        let base = if bit(half, 11) {
            self.regs[SP]
        } else {
            self.pc().wrapping_add(PC_OFFSET) & !3
        };
        self.regs[rd] = base.wrapping_add(bits(half, 7, 0) * 4);
    }

    fn thumb_block_transfer<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let load = bit(half, 11);
        let rn = bits(half, 10, 8) as usize;
        let list = bits(half, 7, 0);
        if list == 0 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }

        // With writeback, STMIA may include Rn only when Rn is the
        // lowest-numbered register in the list. Other forms have no
        // architecturally reliable stored value for the base register.
        if !load && list & (1 << rn) != 0 && list & ((1 << rn) - 1) != 0 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }

        let base = self.regs[rn];
        if base & 3 != 0 && self.cp15.alignment_fault_enabled() {
            return Err(Error::InvalidAccess { addr: base });
        }
        // Legacy ARMv6 multi-word transfers access Word[Align(Addr)].
        // Keep writeback based on the original (possibly unaligned) base.
        let mut addr = base & !3;
        let new_base = base.wrapping_add(4 * list.count_ones());

        if load {
            let mut values = [0u32; 8];
            for (r, value) in values.iter_mut().enumerate() {
                if list & (1 << r) != 0 {
                    *value = self.read_data_u32(bus, addr)?;
                    addr = addr.wrapping_add(4);
                }
            }
            // When the base is also loaded, the loaded value wins.
            if list & (1 << rn) == 0 {
                self.regs[rn] = new_base;
            }
            for (r, value) in values.iter().enumerate() {
                if list & (1 << r) != 0 {
                    self.regs[r] = *value;
                }
            }
        } else {
            let regs = self.regs;
            for (r, value) in regs.iter().enumerate().take(8) {
                if list & (1 << r) != 0 {
                    self.write_data_u32(bus, addr, *value)?;
                    addr = addr.wrapping_add(4);
                }
            }
            self.regs[rn] = new_base;
        }
        Ok(())
    }

    fn thumb_push<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let list = bits(half, 7, 0) | (u32::from(bit(half, 8)) << LR);
        if list == 0 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }
        let original_sp = self.regs[SP];
        let new_sp = original_sp.wrapping_sub(4 * list.count_ones());
        if new_sp & 3 != 0 && self.cp15.alignment_fault_enabled() {
            return Err(Error::InvalidAccess { addr: new_sp });
        }

        let regs = self.regs;
        let mut addr = new_sp & !3;
        for (r, value) in regs.iter().enumerate() {
            if list & (1 << r) != 0 {
                self.write_data_u32(bus, addr, *value)?;
                addr = addr.wrapping_add(4);
            }
        }
        self.regs[SP] = new_sp;
        Ok(())
    }

    fn thumb_pop<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let list = bits(half, 7, 0) | (u32::from(bit(half, 8)) << PC);
        if list == 0 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }

        let original_sp = self.regs[SP];
        if original_sp & 3 != 0 && self.cp15.alignment_fault_enabled() {
            return Err(Error::InvalidAccess { addr: original_sp });
        }
        let mut addr = original_sp & !3;
        let new_sp = original_sp.wrapping_add(4 * list.count_ones());
        let mut values = [0u32; 16];
        for (r, value) in values.iter_mut().enumerate() {
            if list & (1 << r) != 0 {
                *value = self.read_data_u32(bus, addr)?;
                addr = addr.wrapping_add(4);
            }
        }

        // POP {pc} interworks through BX semantics. A word-aligned ARM
        // target is required; bits[1:0] == 0b10 is architecturally
        // UNPREDICTABLE and must not be silently rounded down.
        if list & (1 << PC) != 0 && values[PC] & 3 == 2 {
            return Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            });
        }

        self.regs[SP] = new_sp;
        for (r, value) in values.iter().enumerate().take(PC) {
            if list & (1 << r) != 0 {
                self.regs[r] = *value;
            }
        }
        if list & (1 << PC) != 0 {
            // Popping into `pc` interworks, like BX.
            self.branch_exchange(values[PC]);
        }
        Ok(())
    }

    // ----- miscellaneous ----------------------------------------------------

    fn thumb_misc<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<StepEvent> {
        if half & 0xFF00 == 0xBE00 {
            // BKPT #imm8; the process layer reports a debugger stop.
            return Ok(StepEvent::Breakpoint(bits(half, 7, 0) as u16));
        }

        if half & 0xFFF7 == 0xB650 {
            // SETEND changes data byte order only; instruction fetch stays LE.
            self.cpsr.set_big_endian(bit(half, 3));
            return Ok(StepEvent::Executed);
        }

        if half & 0xFFE0 == 0xB660 {
            // CPSIE/CPSID update selected A/I/F masks. User mode treats CPS
            // as a NOP; the Thumb encoding does not include a mode change.
            let encoded_flags = (half & 0x7) as u8;
            // T16 encodes A:I:F from bit 2 down to bit 0, while the
            // shared CPSR helper expects A:I:F in bits 0 through 2.
            let interrupt_mask = ((encoded_flags & 0b100) >> 2)
                | (encoded_flags & 0b010)
                | ((encoded_flags & 0b001) << 2);
            if bit(half, 3) || interrupt_mask == 0 {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word: half,
                });
            }
            if self.cpsr.privileged() {
                self.cpsr.set_interrupt_masks(interrupt_mask, bit(half, 4));
            }
            return Ok(StepEvent::Executed);
        }

        let rm = bits(half, 5, 3) as usize;
        let rd = bits(half, 2, 0) as usize;

        // The BFxx hint/IT space is outside the ARMv6/ARMv6K Thumb baseline.
        // NOP and IT are ARMv6T2 additions; YIELD/WFE/WFI/SEV are later additions.
        // Reject these encodings instead of treating an unsupported hint as a NOP.
        match bits(half, 11, 8) {
            // ADD/SUB sp, #imm7*4
            0b0000 => {
                let amount = bits(half, 6, 0) * 4;
                self.regs[SP] = if bit(half, 7) {
                    self.regs[SP].wrapping_sub(amount)
                } else {
                    self.regs[SP].wrapping_add(amount)
                };
            }
            // SXTH, SXTB, UXTH, UXTB
            0b0010 => {
                let value = self.regs[rm];
                self.regs[rd] = match bits(half, 7, 6) {
                    0 => i32::from(value as u16 as i16) as u32,
                    1 => i32::from(value as u8 as i8) as u32,
                    2 => value & 0xFFFF,
                    _ => value & 0xFF,
                };
            }
            0b0100 | 0b0101 => self.thumb_push(bus, half)?,
            0b1100 | 0b1101 => self.thumb_pop(bus, half)?,
            // REV, REV16, REVSH
            0b1010 => {
                let value = self.regs[rm];
                self.regs[rd] = match bits(half, 7, 6) {
                    0 => value.swap_bytes(),
                    1 => ((value & 0xFF00_FF00) >> 8) | ((value & 0x00FF_00FF) << 8),
                    3 => i32::from((value as u16).swap_bytes() as i16) as u32,
                    _ => {
                        return Err(Error::UndefinedInstruction {
                            pc: self.pc(),
                            word: half,
                        });
                    }
                };
            }
            _ => {
                return Err(Error::UndefinedInstruction {
                    pc: self.pc(),
                    word: half,
                });
            }
        }
        Ok(StepEvent::Executed)
    }

    // ----- branches ---------------------------------------------------------

    fn thumb_conditional_branch(&mut self, half: u32) -> Result<StepEvent> {
        match bits(half, 11, 8) {
            0xF => {
                if self.svc_exception_entry_enabled {
                    self.enter_svc_exception(self.pc().wrapping_add(2))?;
                }
                Ok(StepEvent::Svc(bits(half, 7, 0)))
            }
            0xE => Err(Error::UndefinedInstruction {
                pc: self.pc(),
                word: half,
            }),
            cond => {
                if Condition::from_bits(cond as u8).passes(self.cpsr) {
                    let offset = (((half & 0xFF) << 24) as i32 >> 23) as u32;
                    self.branch(self.pc().wrapping_add(PC_OFFSET).wrapping_add(offset));
                }
                Ok(StepEvent::Executed)
            }
        }
    }

    /// `B`, and both halves of the `BL`/`BLX` pair.
    ///
    /// The first half only stages the upper offset bits in `lr`; the second
    /// half completes the branch and leaves the return address in `lr`.
    fn thumb_branch(&mut self, half: u32) -> Result<()> {
        let pc = self.pc();
        let imm11 = half & 0x7FF;

        match bits(half, 12, 11) {
            0b00 => {
                let offset = ((imm11 << 21) as i32 >> 20) as u32;
                self.branch(pc.wrapping_add(PC_OFFSET).wrapping_add(offset));
            }
            0b10 => {
                let offset = ((imm11 << 21) as i32 >> 9) as u32;
                self.regs[LR] = pc.wrapping_add(PC_OFFSET).wrapping_add(offset);
            }
            0b11 => {
                let target = self.regs[LR].wrapping_add(imm11 << 1);
                self.regs[LR] = pc.wrapping_add(2) | 1;
                self.branch(target & !1);
            }
            _ => {
                // BLX suffix bit[0] is fixed to zero in the ARMv6 encoding.
                if bit(half, 0) {
                    return Err(Error::UndefinedInstruction { pc, word: half });
                }
                // BLX suffix: continue in ARM state at a word-aligned target.
                let target = self.regs[LR].wrapping_add(imm11 << 1) & !3;
                self.regs[LR] = pc.wrapping_add(2) | 1;
                self.cpsr.set_thumb(false);
                self.branch(target);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Psr;
    use crate::memory::FlatMemory;

    const DATA: u32 = 0x800;

    struct Machine {
        cpu: Arm11Core,
        mem: FlatMemory,
    }

    impl Machine {
        fn new(program: &[u16]) -> Self {
            let mut mem = FlatMemory::new(0, 0x1000);
            for (i, half) in program.iter().enumerate() {
                mem.write16(i as u32 * 2, *half).unwrap();
            }
            let mut cpu = Arm11Core::new();
            cpu.cpsr.set_thumb(true);
            Self { cpu, mem }
        }

        /// A machine whose registers were prepared by `setup`, ready to run `half`.
        fn with(half: u16, setup: impl FnOnce(&mut Arm11Core, &mut FlatMemory)) -> Self {
            let mut m = Self::new(&[half]);
            setup(&mut m.cpu, &mut m.mem);
            m
        }

        fn step(&mut self) -> StepEvent {
            self.cpu.step(&mut self.mem).unwrap()
        }

        fn flags(&self) -> (bool, bool, bool, bool) {
            let psr = self.cpu.cpsr;
            (psr.n(), psr.z(), psr.c(), psr.v())
        }
    }

    // ----- shifts and arithmetic --------------------------------------------

    #[test]
    fn move_add_and_sub_immediates() {
        // MOV r0,#5 ; ADD r0,#3 ; SUB r0,#1
        let mut m = Machine::new(&[0x2005, 0x3003, 0x3801]);
        for _ in 0..3 {
            m.step();
        }
        assert_eq!(m.cpu.regs[0], 7);
        assert_eq!(m.cpu.pc(), 6);
    }

    #[test]
    fn cmp_immediate_sets_zero_and_carry() {
        // CMP r0,#8
        let mut m = Machine::with(0x2808, |cpu, _| cpu.regs[0] = 8);
        m.step();
        assert_eq!(m.flags(), (false, true, true, false));
    }

    #[test]
    fn shift_by_immediate_updates_flags() {
        // LSL r1,r0,#4
        let mut m = Machine::with(0x0101, |cpu, _| cpu.regs[0] = 3);
        m.step();
        assert_eq!(m.cpu.regs[1], 0x30);

        // LSR r1,r0,#1
        let mut m = Machine::with(0x0841, |cpu, _| cpu.regs[0] = 3);
        m.step();
        assert_eq!(m.cpu.regs[1], 1);
        assert!(m.cpu.cpsr.c());

        // ASR r1,r0,#31
        let mut m = Machine::with(0x17C1, |cpu, _| cpu.regs[0] = 0xC000_0000);
        m.step();
        assert_eq!(m.cpu.regs[1], u32::MAX);
        assert_eq!(m.flags(), (true, false, true, false));
    }

    #[test]
    fn zero_immediate_lsr_and_asr_each_shift_by_32() {
        let mut lsr = Machine::with(0x0801, |cpu, _| cpu.regs[0] = 0x8000_0001);
        lsr.step();
        assert_eq!(lsr.cpu.regs[1], 0);
        assert_eq!(lsr.flags(), (false, true, true, false));

        let mut asr = Machine::with(0x1001, |cpu, _| cpu.regs[0] = 0x8000_0001);
        asr.step();
        assert_eq!(asr.cpu.regs[1], u32::MAX);
        assert_eq!(asr.flags(), (true, false, true, false));
    }

    #[test]
    fn lsl_zero_copies_the_register_and_keeps_carry() {
        // LSL r1,r0,#0 (MOVS r1,r0)
        let mut m = Machine::with(0x0001, |cpu, _| {
            cpu.regs[0] = 9;
            cpu.cpsr.set_flags(false, false, true, false);
        });
        m.step();
        assert_eq!(m.cpu.regs[1], 9);
        assert_eq!(m.flags(), (false, false, true, false));
    }

    #[test]
    fn add_and_sub_register_and_small_immediate() {
        let setup = |cpu: &mut Arm11Core, _: &mut FlatMemory| {
            cpu.regs[0] = 10;
            cpu.regs[1] = 3;
        };

        // ADD r2,r0,r1
        let mut m = Machine::with(0x1842, setup);
        m.step();
        assert_eq!(m.cpu.regs[2], 13);

        // SUB r2,r0,r1
        let mut m = Machine::with(0x1A42, setup);
        m.step();
        assert_eq!(m.cpu.regs[2], 7);
        assert_eq!(m.flags(), (false, false, true, false));

        // ADD r1,r0,#7
        let mut m = Machine::with(0x1DC1, setup);
        m.step();
        assert_eq!(m.cpu.regs[1], 17);

        // SUB r1,r0,#1
        let mut m = Machine::with(0x1E41, setup);
        m.step();
        assert_eq!(m.cpu.regs[1], 9);
    }

    // ----- ALU operations ---------------------------------------------------

    fn alu(half: u16, r0: u32, r1: u32, flags: Psr) -> Machine {
        let mut m = Machine::with(half, |cpu, _| {
            cpu.regs[0] = r0;
            cpu.regs[1] = r1;
            cpu.cpsr
                .set_flags(flags.n(), flags.z(), flags.c(), flags.v());
        });
        m.step();
        m
    }

    #[test]
    fn logical_alu_operations() {
        let none = Psr::default();
        assert_eq!(alu(0x4008, 0xFF, 0x0F, none).cpu.regs[0], 0x0F); // AND
        assert_eq!(alu(0x4048, 0xFF, 0x0F, none).cpu.regs[0], 0xF0); // EOR
        assert_eq!(alu(0x4308, 0xF0, 0x0F, none).cpu.regs[0], 0xFF); // ORR
        assert_eq!(alu(0x4388, 0xFF, 0x0F, none).cpu.regs[0], 0xF0); // BIC
        assert_eq!(alu(0x43C8, 0, 0, none).cpu.regs[0], u32::MAX); // MVN
    }

    #[test]
    fn tst_only_sets_flags() {
        let m = alu(0x4208, 0xF0, 0x0F, Psr::default());
        assert_eq!(m.cpu.regs[0], 0xF0);
        assert!(m.cpu.cpsr.z());
    }

    #[test]
    fn neg_and_mul() {
        let m = alu(0x4248, 0, 5, Psr::default()); // NEG r0,r1
        assert_eq!(m.cpu.regs[0], 0xFFFF_FFFB);
        assert_eq!(m.flags(), (true, false, false, false));

        let m = alu(0x4348, 6, 7, Psr::default()); // MUL r0,r1
        assert_eq!(m.cpu.regs[0], 42);
    }

    #[test]
    fn adc_uses_the_carry_flag() {
        let carry = Psr::from_flags(false, false, true, false);
        assert_eq!(alu(0x4148, 1, 2, carry).cpu.regs[0], 4);
        assert_eq!(alu(0x4148, 1, 2, Psr::default()).cpu.regs[0], 3);
    }

    #[test]
    fn sbc_uses_the_inverse_borrow_carry_semantics() {
        let carry = Psr::from_flags(false, false, true, false);
        assert_eq!(alu(0x4188, 5, 2, carry).cpu.regs[0], 3);
        assert_eq!(alu(0x4188, 5, 2, Psr::default()).cpu.regs[0], 2);
    }

    #[test]
    fn register_controlled_asr_sign_extends_and_sets_carry() {
        let m = alu(0x4108, 0x8000_0000, 4, Psr::default());
        assert_eq!(m.cpu.regs[0], 0xF800_0000);
        assert_eq!(m.flags(), (true, false, false, false));

        let m = alu(0x4108, 0x8000_0001, 32, Psr::default());
        assert_eq!(m.cpu.regs[0], u32::MAX);
        assert_eq!(m.flags(), (true, false, true, false));
    }

    #[test]
    fn register_controlled_shifts() {
        assert_eq!(alu(0x4088, 1, 4, Psr::default()).cpu.regs[0], 0x10); // LSL
        assert_eq!(alu(0x40C8, 0x100, 4, Psr::default()).cpu.regs[0], 0x10); // LSR
        let m = alu(0x41C8, 1, 1, Psr::default()); // ROR
        assert_eq!(m.cpu.regs[0], 0x8000_0000);
        assert!(m.cpu.cpsr.c());
    }

    #[test]
    fn cmp_register_borrows_when_smaller() {
        let m = alu(0x4288, 3, 5, Psr::default()); // CMP r0,r1
        assert_eq!(m.cpu.regs[0], 3);
        assert_eq!(m.flags(), (true, false, false, false));
    }

    #[test]
    fn cmn_sets_addition_flags_without_writing_destination() {
        let m = alu(0x42C8, 0x7FFF_FFFF, 1, Psr::default()); // CMN r0,r1.
        assert_eq!(m.cpu.regs[0], 0x7FFF_FFFF);
        assert_eq!(m.flags(), (true, false, false, true));
    }

    #[test]
    fn cmp_supports_high_register_operands() {
        let mut m = Machine::with(0x4580, |cpu, _| {
            cpu.regs[8] = 3;
            cpu.regs[0] = 5;
        }); // CMP r8,r0.
        m.step();

        assert_eq!(m.cpu.regs[8], 3);
        assert_eq!(m.cpu.regs[0], 5);
        assert_eq!(m.flags(), (true, false, false, false));
    }

    // ----- hi registers and BX ----------------------------------------------

    #[test]
    fn mov_to_and_from_high_registers() {
        // MOV r8,r0
        let mut m = Machine::with(0x4680, |cpu, _| cpu.regs[0] = 0x1234);
        m.step();
        assert_eq!(m.cpu.regs[8], 0x1234);

        // MOV r0,r8
        let mut m = Machine::with(0x4640, |cpu, _| cpu.regs[8] = 0x55);
        m.step();
        assert_eq!(m.cpu.regs[0], 0x55);
    }

    #[test]
    fn high_register_data_processing_accepts_sp_operands() {
        let original_flags = (true, false, true, true);

        // MOV r8,sp accepts SP as the high-register source and preserves flags.
        let mut mov = Machine::with(0x46E8, |cpu, _| {
            cpu.regs[SP] = 0xA5A5_5A5A;
            cpu.cpsr.set_flags(
                original_flags.0,
                original_flags.1,
                original_flags.2,
                original_flags.3,
            );
        });
        mov.step();
        assert_eq!(mov.cpu.regs[8], 0xA5A5_5A5A);
        assert_eq!(mov.flags(), original_flags);

        // ADD r8,sp also leaves the condition flags unchanged.
        let mut add = Machine::with(0x44E8, |cpu, _| {
            cpu.regs[8] = 7;
            cpu.regs[SP] = 9;
            cpu.cpsr.set_flags(
                original_flags.0,
                original_flags.1,
                original_flags.2,
                original_flags.3,
            );
        });
        add.step();
        assert_eq!(add.cpu.regs[8], 16);
        assert_eq!(add.flags(), original_flags);

        // CMP r8,sp accepts SP as Rm and updates only the condition flags.
        let mut cmp = Machine::with(0x45E8, |cpu, _| {
            cpu.regs[8] = 9;
            cpu.regs[SP] = 9;
        });
        cmp.step();
        assert_eq!(cmp.cpu.regs[8], 9);
        assert_eq!(cmp.cpu.regs[SP], 9);
        assert_eq!(cmp.flags(), (false, true, true, false));
    }

    #[test]
    fn cmp_with_pc_as_first_operand_is_undefined_without_side_effects() {
        // CMP PC,R0 encodes the UNPREDICTABLE Rn = R15 form.
        let mut m = Machine::with(0x4587, |cpu, _| {
            cpu.regs[0] = 0x1234_5678;
            cpu.cpsr.set_flags(true, true, false, true);
        });
        let old_regs = m.cpu.regs;
        let old_cpsr = m.cpu.cpsr;

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0x4587
            })
        ));
        assert_eq!(m.cpu.regs, old_regs);
        assert_eq!(m.cpu.cpsr, old_cpsr);
        assert_eq!(m.cpu.pc(), 0);
    }

    #[test]
    fn bx_pc_at_word_aligned_instruction_branches_to_next_word() {
        // BX PC reads address + 4; at address 0 this is a valid ARM target.
        let mut m = Machine::with(0x4778, |_, _| {});
        m.step();

        assert_eq!(m.cpu.pc(), 4);
        assert!(!m.cpu.cpsr.thumb());
    }

    #[test]
    fn bx_pc_at_halfword_aligned_instruction_rejects_unpredictable_target() {
        // At address 2, BX PC reads 6 (bits[1:0] == 0b10).
        let mut m = Machine::with(0x4778, |cpu, mem| {
            cpu.set_pc(2);
            mem.write16(2, 0x4778).unwrap();
        });
        let old_cpsr = m.cpu.cpsr;
        let old_lr = m.cpu.regs[LR];

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 2,
                word: 0x4778
            })
        ));
        assert_eq!(m.cpu.pc(), 2);
        assert_eq!(m.cpu.cpsr, old_cpsr);
        assert_eq!(m.cpu.regs[LR], old_lr);
    }

    #[test]
    fn high_register_pc_operands_read_current_instruction_plus_four() {
        // At address 2, PC reads as 6 (bit 1 is retained in high-register forms).
        let mut mov = Machine::with(0x4678, |cpu, mem| {
            cpu.set_pc(2);
            mem.write16(2, 0x4678).unwrap(); // MOV r0,pc.
        });
        mov.step();
        assert_eq!(mov.cpu.regs[0], 6);
        assert_eq!(mov.cpu.pc(), 4);

        let mut add = Machine::with(0x4478, |cpu, mem| {
            cpu.set_pc(2);
            cpu.regs[0] = 10;
            mem.write16(2, 0x4478).unwrap(); // ADD r0,pc.
        });
        add.step();
        assert_eq!(add.cpu.regs[0], 16);

        let mut cmp = Machine::with(0x4578, |cpu, mem| {
            cpu.set_pc(2);
            cpu.regs[0] = 6;
            mem.write16(2, 0x4578).unwrap(); // CMP r0,pc.
        });
        cmp.step();
        assert!(cmp.cpu.cpsr.z());
        assert!(cmp.cpu.cpsr.c());
    }

    #[test]
    fn armv6_cpy_low_register_alias_moves_value_without_changing_flags() {
        let mut m = Machine::with(0x4608, |cpu, _| {
            cpu.regs[1] = 0xA5A5_5A5A;
            cpu.cpsr.set_flags(true, false, true, true);
        });
        let flags = m.cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V);

        m.step();

        assert_eq!(m.cpu.regs[0], 0xA5A5_5A5A);
        assert_eq!(m.cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V), flags);
    }

    #[test]
    fn high_register_add_and_cmp_reject_low_low_encodings_atomically() {
        for half in [0x4408, 0x4508] {
            let mut m = Machine::with(half, |cpu, _| {
                cpu.regs[0] = 0x1122_3344;
                cpu.regs[1] = 0x5566_7788;
                cpu.cpsr.set_flags(true, false, true, true);
            });
            let old_regs = m.cpu.regs;
            let old_cpsr = m.cpu.cpsr;

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == u32::from(half)
            ));
            assert_eq!(m.cpu.pc(), 0);
            assert_eq!(m.cpu.regs, old_regs);
            assert_eq!(m.cpu.cpsr, old_cpsr);
        }
    }

    #[test]
    fn add_high_register_leaves_flags_alone() {
        // ADD r0,r8
        let mut m = Machine::with(0x4440, |cpu, _| {
            cpu.regs[0] = 1;
            cpu.regs[8] = 2;
            cpu.cpsr.set_flags(true, true, true, true);
        });
        m.step();
        assert_eq!(m.cpu.regs[0], 3);
        assert_eq!(m.flags(), (true, true, true, true));
    }

    #[test]
    fn mov_pc_lr_stays_in_thumb_state() {
        // MOV pc,lr
        let mut m = Machine::with(0x46F7, |cpu, _| cpu.regs[14] = 0x41);
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(m.cpu.cpsr.thumb());
    }

    #[test]
    fn bx_switches_state_from_bit_zero() {
        // BX lr
        let mut m = Machine::with(0x4770, |cpu, _| cpu.regs[14] = 0x40);
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(!m.cpu.cpsr.thumb());

        let mut m = Machine::with(0x4770, |cpu, _| cpu.regs[14] = 0x41);
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(m.cpu.cpsr.thumb());
    }

    #[test]
    fn blx_register_sets_a_thumb_return_address() {
        // BLX r3
        let mut m = Machine::with(0x4798, |cpu, _| cpu.regs[3] = 0x31);
        m.step();
        assert_eq!((m.cpu.regs[14], m.cpu.pc()), (3, 0x30));
        assert!(m.cpu.cpsr.thumb());
    }

    #[test]
    fn blx_register_switches_to_arm_when_target_bit_zero_is_clear() {
        // BLX r3 with an even target clears Thumb state and word-aligns PC.
        let mut m = Machine::with(0x4798, |cpu, _| cpu.regs[3] = 0x40);
        m.step();

        assert_eq!(m.cpu.regs[LR], 3);
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(!m.cpu.cpsr.thumb());
    }

    #[test]
    fn bx_and_blx_accept_sp_as_target_register() {
        // BX sp uses bit zero to retain Thumb state.
        let mut bx = Machine::with(0x4768, |cpu, _| cpu.regs[SP] = 0x101);
        bx.step();
        assert_eq!(bx.cpu.pc(), 0x100);
        assert!(bx.cpu.cpsr.thumb());

        // BLX sp links the Thumb return address and enters ARM state.
        let mut blx = Machine::with(0x47E8, |cpu, _| cpu.regs[SP] = 0x200);
        blx.step();
        assert_eq!(blx.cpu.regs[LR], 3);
        assert_eq!(blx.cpu.pc(), 0x200);
        assert!(!blx.cpu.cpsr.thumb());
    }

    // ----- loads and stores -------------------------------------------------

    fn data_machine(half: u16) -> Machine {
        Machine::with(half, |cpu, mem| {
            cpu.regs[1] = DATA;
            cpu.regs[2] = 4;
            mem.write32(DATA + 4, 0x8000_80F0).unwrap();
        })
    }

    #[test]
    fn pc_relative_load_uses_word_aligned_pc_plus_four() {
        // LDR r0,[pc,#4]: (0 + 4) & !3 + 4 = 8
        let mut m = Machine::with(0x4801, |_, mem| mem.write32(8, 0xCAFE_BABE).unwrap());
        m.step();
        assert_eq!(m.cpu.regs[0], 0xCAFE_BABE);
    }

    #[test]
    fn register_offset_loads() {
        let mut m = data_machine(0x5888); // LDR r0,[r1,r2]
        m.step();
        assert_eq!(m.cpu.regs[0], 0x8000_80F0);

        let mut m = data_machine(0x5C88); // LDRB
        m.step();
        assert_eq!(m.cpu.regs[0], 0xF0);

        let mut m = data_machine(0x5688); // LDRSB
        m.step();
        assert_eq!(m.cpu.regs[0], 0xFFFF_FFF0);

        let mut m = data_machine(0x5A88); // LDRH
        m.step();
        assert_eq!(m.cpu.regs[0], 0x80F0);

        let mut m = data_machine(0x5E88); // LDRSH
        m.step();
        assert_eq!(m.cpu.regs[0], 0xFFFF_80F0);
    }

    #[test]
    fn register_offset_stores() {
        for (half, expected) in [
            (0x5088, 0xAABB_CCDD), // STR
            (0x5288, 0x0000_CCDD), // STRH
            (0x5488, 0x0000_00DD), // STRB
        ] {
            let mut m = data_machine(half);
            m.mem.write32(DATA + 4, 0).unwrap();
            m.cpu.regs[0] = 0xAABB_CCDD;
            m.step();
            assert_eq!(m.mem.read32(DATA + 4).unwrap(), expected, "{half:#06x}");
        }
    }

    #[test]
    fn immediate_offset_word_byte_and_halfword() {
        let setup = |cpu: &mut Arm11Core, _: &mut FlatMemory| {
            cpu.regs[0] = 0x1122_3344;
            cpu.regs[1] = DATA;
        };

        let mut m = Machine::with(0x6048, setup); // STR r0,[r1,#4]
        m.step();
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 0x1122_3344);

        let mut m = Machine::with(0x7048, setup); // STRB r0,[r1,#1]
        m.step();
        assert_eq!(m.mem.read8(DATA + 1).unwrap(), 0x44);

        let mut m = Machine::with(0x8048, setup); // STRH r0,[r1,#2]
        m.step();
        assert_eq!(m.mem.read16(DATA + 2).unwrap(), 0x3344);

        let load_setup = |cpu: &mut Arm11Core, mem: &mut FlatMemory| {
            cpu.regs[1] = DATA;
            mem.write32(DATA + 4, 0x1122_3344).unwrap();
        };
        let mut m = Machine::with(0x6848, load_setup); // LDR r0,[r1,#4]
        m.step();
        assert_eq!(m.cpu.regs[0], 0x1122_3344);

        let mut m = Machine::with(0x7948, load_setup); // LDRB r0,[r1,#5]
        m.step();
        assert_eq!(m.cpu.regs[0], 0x33);

        let mut m = Machine::with(0x8888, load_setup); // LDRH r0,[r1,#4]
        m.step();
        assert_eq!(m.cpu.regs[0], 0x3344);
    }

    #[test]
    fn legacy_unaligned_thumb_word_accesses_align_stores_and_rotate_loads() {
        // The legacy ARMv6 word path aligns stores and rotates unaligned loads.
        // Cover both immediate-offset and register-offset Thumb encodings.
        for (load, store) in [(0x680B, 0x6008), (0x588B, 0x5088)] {
            let mut m = Machine::new(&[load, store]);
            m.cpu.regs[0] = 0xAABB_CCDD;
            m.cpu.regs[1] = DATA + 1;
            m.cpu.regs[2] = 0;
            m.mem.write32(DATA, 0x1122_3344).unwrap();

            m.step();
            assert_eq!(m.cpu.regs[3], 0x4411_2233, "LDR {load:#06x}");

            m.step();
            assert_eq!(
                m.mem.read32(DATA).unwrap(),
                0xAABB_CCDD,
                "STR {store:#06x} must use Align(Addr)"
            );
        }
    }

    #[test]
    fn legacy_unaligned_thumb_word_accesses_preserve_big_endian_rotation() {
        // SETEND BE; the legacy load rotates left while a store writes Word[Align(Addr)].
        for (load, store) in [(0x680B, 0x6008), (0x588B, 0x5088)] {
            let mut m = Machine::new(&[0xB658, load, store]);
            m.cpu.regs[0] = 0xAABB_CCDD;
            m.cpu.regs[1] = DATA + 1;
            m.cpu.regs[2] = 0;
            // Big-endian logical word 0x11223344 in the flat little-endian backing store.
            m.mem.write32(DATA, 0x4433_2211).unwrap();

            m.step();
            assert!(m.cpu.cpsr.big_endian());
            m.step();
            assert_eq!(m.cpu.regs[3], 0x2233_4411, "LDR {load:#06x}");

            m.step();
            assert_eq!(
                m.mem.read32(DATA).unwrap(),
                0xDDCC_BBAA,
                "STR {store:#06x} must preserve big-endian Word[Align(Addr)]"
            );
        }
    }

    fn set_unaligned_word_base(cpu: &mut Arm11Core, form: u8) {
        match form {
            0 => {
                cpu.regs[1] = DATA;
                cpu.regs[2] = 1;
            }
            1 => cpu.regs[1] = DATA + 1,
            2 => cpu.regs[SP] = DATA + 1,
            _ => unreachable!("test fixture form"),
        }
    }

    #[test]
    fn thumb_single_word_transfers_honor_sctlr_u_for_all_word_addressing_forms() {
        // Register-offset, immediate-offset and SP-relative T1 word transfers
        // all use the same ARMv6 U/A behavior as the A32 single-transfer path.
        let forms = [
            (0x5888, 0x5088, 0), // LDR/STR r0,[r1,r2], with r2=1.
            (0x6808, 0x6008, 1), // LDR/STR r0,[r1,#0], with r1 unaligned.
            (0x9800, 0x9000, 2), // LDR/STR r0,[sp,#0], with SP unaligned.
        ];

        for big_endian in [false, true] {
            for &(load_half, store_half, form) in &forms {
                let mut load = Machine::with(load_half, |cpu, mem| {
                    set_unaligned_word_base(cpu, form);
                    cpu.cpsr.set_big_endian(big_endian);
                    cpu.cp15.write(true, 0, 1, 0, 0, 1 << 22).unwrap();
                    for (offset, byte) in
                        [0xAA, 0x11, 0x22, 0x33, 0x44, 0xBB].into_iter().enumerate()
                    {
                        mem.write8(DATA + offset as u32, byte).unwrap();
                    }
                });
                load.step();
                assert_eq!(
                    load.cpu.regs[0],
                    if big_endian { 0x1122_3344 } else { 0x4433_2211 },
                    "LDR {load_half:#06x}, BE={big_endian}"
                );

                let mut store = Machine::with(store_half, |cpu, mem| {
                    set_unaligned_word_base(cpu, form);
                    cpu.regs[0] = 0xA1B2_C3D4;
                    cpu.cpsr.set_big_endian(big_endian);
                    cpu.cp15.write(true, 0, 1, 0, 0, 1 << 22).unwrap();
                    for (offset, byte) in [0xAA, 0, 0, 0, 0, 0xBB].into_iter().enumerate() {
                        mem.write8(DATA + offset as u32, byte).unwrap();
                    }
                });
                store.step();

                let expected = if big_endian {
                    [0xA1, 0xB2, 0xC3, 0xD4]
                } else {
                    [0xD4, 0xC3, 0xB2, 0xA1]
                };
                assert_eq!(store.mem.read8(DATA).unwrap(), 0xAA);
                for (offset, byte) in expected.into_iter().enumerate() {
                    assert_eq!(
                        store.mem.read8(DATA + 1 + offset as u32).unwrap(),
                        byte,
                        "STR {store_half:#06x}, BE={big_endian}, byte={offset}"
                    );
                }
                assert_eq!(store.mem.read8(DATA + 5).unwrap(), 0xBB);
            }
        }
    }

    #[test]
    fn thumb_single_word_transfers_fault_atomically_when_sctlr_a_is_enabled() {
        let instructions = [
            (0x5888, 0), // LDR r0,[r1,r2].
            (0x5088, 0), // STR r0,[r1,r2].
            (0x6808, 1), // LDR r0,[r1,#0].
            (0x6008, 1), // STR r0,[r1,#0].
            (0x9800, 2), // LDR r0,[sp,#0].
            (0x9000, 2), // STR r0,[sp,#0].
        ];

        for (half, form) in instructions {
            let mut machine = Machine::with(half, |cpu, mem| {
                set_unaligned_word_base(cpu, form);
                cpu.regs[0] = 0xA1B2_C3D4;
                cpu.cp15.write(true, 0, 1, 0, 0, 1 << 1).unwrap();
                mem.write32(DATA, 0x1122_3344).unwrap();
                mem.write32(DATA + 4, 0x5566_7788).unwrap();
            });
            let old_regs = machine.cpu.regs;
            let old_cpsr = machine.cpu.cpsr;
            let old_first = machine.mem.read32(DATA).unwrap();
            let old_second = machine.mem.read32(DATA + 4).unwrap();

            assert!(matches!(
                machine.cpu.step(&mut machine.mem),
                Err(Error::InvalidAccess { addr }) if addr == DATA + 1
            ));
            assert_eq!(machine.cpu.regs, old_regs, "{half:#06x}");
            assert_eq!(machine.cpu.cpsr, old_cpsr, "{half:#06x}");
            assert_eq!(machine.mem.read32(DATA).unwrap(), old_first, "{half:#06x}");
            assert_eq!(
                machine.mem.read32(DATA + 4).unwrap(),
                old_second,
                "{half:#06x}"
            );
        }
    }

    #[test]
    fn thumb_unaligned_word_fault_enters_data_abort_and_latches_address() {
        let mut m = Machine::with(0x6808, |cpu, mem| {
            cpu.regs[0] = 0xDEAD_BEEF;
            cpu.regs[1] = DATA + 1;
            cpu.cp15.write(true, 0, 1, 0, 0, 1 << 1).unwrap();
            cpu.set_fault_exception_entry_enabled(true);
            mem.write32(DATA, 0x1122_3344).unwrap();
        });
        let interrupted_status = m.cpu.cpsr;

        assert_eq!(m.cpu.step(&mut m.mem).unwrap(), StepEvent::Executed);
        assert_eq!(m.cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert!(!m.cpu.cpsr.thumb());
        assert_eq!(m.cpu.pc(), 0x10);
        assert_eq!(m.cpu.regs[LR], 8);
        assert_eq!(m.cpu.saved_program_status(), Some(interrupted_status));
        assert_eq!(m.cpu.cp15.dfar(), DATA + 1);
        assert_eq!(m.cpu.cp15.dfsr(), 0x8);
        assert_eq!(m.cpu.regs[0], 0xDEAD_BEEF);
        assert_eq!(m.cpu.regs[1], DATA + 1);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x1122_3344);
    }

    #[test]
    fn thumb_unaligned_multiple_and_stack_transfers_enter_data_abort() {
        let cases = [
            (0xC006, DATA + 1, DATA + 1),       // STMIA r0!,{r1,r2}
            (0xC806, DATA + 1, DATA + 1),       // LDMIA r0!,{r1,r2}
            (0xB403, DATA + 0x11, DATA + 0x09), // PUSH {r0,r1}: fault at new SP.
            (0xBC03, DATA + 1, DATA + 1),       // POP {r0,r1}: fault at original SP.
        ];

        for alignment_bit in [1 << 22, 1 << 1] {
            for (half, base, fault_address) in cases {
                let mut m = Machine::with(half, |cpu, mem| {
                    cpu.regs[0] = base;
                    cpu.regs[1] = 0x1122_3344;
                    cpu.regs[2] = 0x5566_7788;
                    cpu.regs[SP] = base;
                    cpu.cp15.write(true, 0, 1, 0, 0, alignment_bit).unwrap();
                    cpu.set_fault_exception_entry_enabled(true);
                    mem.write32(DATA, 0xCAFE_BABE).unwrap();
                    mem.write32(DATA + 4, 0xDEAD_BEEF).unwrap();
                    mem.write32(DATA + 8, 0xA5A5_5A5A).unwrap();
                });
                let interrupted_status = m.cpu.cpsr;
                let original_regs = m.cpu.regs;
                let original_memory = [
                    m.mem.read32(DATA).unwrap(),
                    m.mem.read32(DATA + 4).unwrap(),
                    m.mem.read32(DATA + 8).unwrap(),
                    m.mem.read32(DATA + 12).unwrap(),
                    m.mem.read32(DATA + 16).unwrap(),
                ];

                assert_eq!(
                    m.cpu.step(&mut m.mem).unwrap(),
                    StepEvent::Executed,
                    "{half:#06x}, SCTLR bit {alignment_bit:#x}"
                );
                assert_eq!(m.cpu.cpsr.mode(), Psr::ABORT_MODE);
                assert!(!m.cpu.cpsr.thumb());
                assert_eq!(m.cpu.pc(), 0x10);
                assert_eq!(m.cpu.regs[LR], 8);
                assert_eq!(m.cpu.saved_program_status(), Some(interrupted_status));
                assert_eq!(m.cpu.cp15.dfar(), fault_address);
                assert_eq!(m.cpu.cp15.dfsr(), 0x8);
                assert_eq!(m.cpu.regs[..13], original_regs[..13]);
                m.cpu.switch_mode(Psr::USER_MODE).unwrap();
                assert_eq!(m.cpu.regs[SP], base, "{half:#06x}");
                for (offset, expected) in original_memory.into_iter().enumerate() {
                    assert_eq!(
                        m.mem.read32(DATA + offset as u32 * 4).unwrap(),
                        expected,
                        "{half:#06x}, memory word {offset}"
                    );
                }
            }
        }
    }

    #[test]
    fn sp_relative_store_and_load() {
        let mut m = Machine::with(0x9002, |cpu, _| {
            // STR r0,[sp,#8]
            cpu.regs[13] = DATA;
            cpu.regs[0] = 0xDEAD_BEEF;
        });
        m.step();
        assert_eq!(m.mem.read32(DATA + 8).unwrap(), 0xDEAD_BEEF);

        let mut m = Machine::with(0x9802, |cpu, mem| {
            // LDR r0,[sp,#8]
            cpu.regs[13] = DATA;
            mem.write32(DATA + 8, 0x77).unwrap();
        });
        m.step();
        assert_eq!(m.cpu.regs[0], 0x77);
    }

    #[test]
    fn setend_big_endian_is_used_by_thumb_word_loads_and_stores() {
        let mut m = Machine::new(&[0xB658, 0x6008, 0x680A]); // SETEND BE; STR r0,[r1]; LDR r2,[r1].
        m.cpu.regs[0] = 0x1122_3344;
        m.cpu.regs[1] = DATA;

        m.step();
        assert!(m.cpu.cpsr.big_endian());
        m.step();
        assert_eq!(m.mem.read32(DATA).unwrap(), 0x4433_2211);
        m.step();

        assert_eq!(m.cpu.regs[2], 0x1122_3344);
    }

    #[test]
    fn setend_big_endian_is_used_by_thumb_halfword_and_signed_loads() {
        let mut m = Machine::new(&[0xB658, 0x8008, 0x880A, 0x5F8B]);
        m.cpu.regs[0] = 0x1234_8001;
        m.cpu.regs[1] = DATA;
        m.cpu.regs[6] = 0;

        m.step(); // SETEND BE.
        m.step(); // STRH r0,[r1].
        assert_eq!(m.mem.read16(DATA).unwrap(), 0x0180);
        m.step(); // LDRH r2,[r1].
        assert_eq!(m.cpu.regs[2], 0x8001);
        m.step(); // LDRSH r3,[r1,r6].
        assert_eq!(m.cpu.regs[3], 0xFFFF_8001);
    }

    #[test]
    fn thumb_unaligned_halfword_loads_and_stores_honor_sctlr_u_and_endianness() {
        // T1 register-offset and immediate-offset forms share ARMv6's
        // Halfword[Addr] behavior when SCTLR.U=1.
        let load_forms = [0x5A88, 0x8808]; // LDRH register and immediate.
        let store_forms = [0x5288, 0x8008]; // STRH register and immediate.

        for big_endian in [false, true] {
            for half in load_forms {
                let mut m = Machine::with(half, |cpu, mem| {
                    cpu.regs[1] = DATA + 1;
                    cpu.regs[2] = 0;
                    cpu.cpsr.set_big_endian(big_endian);
                    cpu.cp15.write(true, 0, 1, 0, 0, 1 << 22).unwrap();
                    mem.write8(DATA, 0xAA).unwrap();
                    let bytes = if big_endian {
                        [0x81, 0x23]
                    } else {
                        [0x23, 0x81]
                    };
                    mem.write8(DATA + 1, bytes[0]).unwrap();
                    mem.write8(DATA + 2, bytes[1]).unwrap();
                    mem.write8(DATA + 3, 0xBB).unwrap();
                });
                m.step();
                assert_eq!(m.cpu.regs[0], 0x8123, "LDRH {half:#06x}, BE={big_endian}");
            }

            // LDRSH has no immediate-offset Thumb form; it uses register offset.
            let mut signed = Machine::with(0x5E88, |cpu, mem| {
                cpu.regs[1] = DATA + 1;
                cpu.regs[2] = 0;
                cpu.cpsr.set_big_endian(big_endian);
                cpu.cp15.write(true, 0, 1, 0, 0, 1 << 22).unwrap();
                let bytes = if big_endian {
                    [0x81, 0x23]
                } else {
                    [0x23, 0x81]
                };
                mem.write8(DATA + 1, bytes[0]).unwrap();
                mem.write8(DATA + 2, bytes[1]).unwrap();
            });
            signed.step();
            assert_eq!(signed.cpu.regs[0], 0xFFFF_8123, "LDRSH, BE={big_endian}");

            for half in store_forms {
                let mut m = Machine::with(half, |cpu, mem| {
                    cpu.regs[0] = 0xA1B2;
                    cpu.regs[1] = DATA + 1;
                    cpu.regs[2] = 0;
                    cpu.cpsr.set_big_endian(big_endian);
                    cpu.cp15.write(true, 0, 1, 0, 0, 1 << 22).unwrap();
                    mem.write8(DATA, 0xAA).unwrap();
                    mem.write8(DATA + 1, 0).unwrap();
                    mem.write8(DATA + 2, 0).unwrap();
                    mem.write8(DATA + 3, 0xBB).unwrap();
                });
                m.step();
                let expected = if big_endian {
                    [0xA1, 0xB2]
                } else {
                    [0xB2, 0xA1]
                };
                assert_eq!(m.mem.read8(DATA).unwrap(), 0xAA);
                assert_eq!(m.mem.read8(DATA + 1).unwrap(), expected[0]);
                assert_eq!(m.mem.read8(DATA + 2).unwrap(), expected[1]);
                assert_eq!(m.mem.read8(DATA + 3).unwrap(), 0xBB);
            }
        }
    }

    #[test]
    fn thumb_unaligned_halfword_is_rejected_without_u_and_faults_with_a() {
        for half in [0x5A88, 0x5E88, 0x5288, 0x8808, 0x8008] {
            let mut legacy = Machine::with(half, |cpu, mem| {
                cpu.regs[0] = 0xA1B2_C3D4;
                cpu.regs[1] = DATA + 1;
                cpu.regs[2] = 0;
                mem.write8(DATA + 1, 0x34).unwrap();
                mem.write8(DATA + 2, 0x12).unwrap();
            });
            let old_regs = legacy.cpu.regs;
            let old_mem = [
                legacy.mem.read8(DATA + 1).unwrap(),
                legacy.mem.read8(DATA + 2).unwrap(),
            ];
            assert!(matches!(
                legacy.cpu.step(&mut legacy.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == u32::from(half)
            ));
            assert_eq!(legacy.cpu.regs, old_regs);
            assert_eq!(legacy.mem.read8(DATA + 1).unwrap(), old_mem[0]);
            assert_eq!(legacy.mem.read8(DATA + 2).unwrap(), old_mem[1]);

            let mut alignment_fault = Machine::with(half, |cpu, mem| {
                cpu.regs[0] = 0xA1B2_C3D4;
                cpu.regs[1] = DATA + 1;
                cpu.regs[2] = 0;
                cpu.cp15.write(true, 0, 1, 0, 0, 1 << 1).unwrap();
                mem.write8(DATA + 1, 0x34).unwrap();
                mem.write8(DATA + 2, 0x12).unwrap();
            });
            let old_regs = alignment_fault.cpu.regs;
            assert!(matches!(
                alignment_fault.cpu.step(&mut alignment_fault.mem),
                Err(Error::InvalidAccess { addr }) if addr == DATA + 1
            ));
            assert_eq!(alignment_fault.cpu.regs, old_regs);
            assert_eq!(alignment_fault.mem.read8(DATA + 1).unwrap(), 0x34);
            assert_eq!(alignment_fault.mem.read8(DATA + 2).unwrap(), 0x12);
        }
    }

    #[test]
    fn thumb_unaligned_halfword_fault_enters_data_abort_and_latches_address() {
        let mut m = Machine::with(0x8808, |cpu, mem| {
            cpu.regs[0] = 0xDEAD_BEEF;
            cpu.regs[1] = DATA + 1;
            cpu.cp15.write(true, 0, 1, 0, 0, 1 << 1).unwrap();
            cpu.set_fault_exception_entry_enabled(true);
            mem.write8(DATA + 1, 0x34).unwrap();
            mem.write8(DATA + 2, 0x12).unwrap();
        });
        let interrupted_status = m.cpu.cpsr;

        assert_eq!(m.cpu.step(&mut m.mem).unwrap(), StepEvent::Executed);
        assert_eq!(m.cpu.cpsr.mode(), Psr::ABORT_MODE);
        assert!(!m.cpu.cpsr.thumb());
        assert_eq!(m.cpu.pc(), 0x10);
        assert_eq!(m.cpu.regs[LR], 8);
        assert_eq!(m.cpu.saved_program_status(), Some(interrupted_status));
        assert_eq!(m.cpu.cp15.dfar(), DATA + 1);
        assert_eq!(m.cpu.cp15.dfsr(), 0x8);
        assert_eq!(m.cpu.regs[0], 0xDEAD_BEEF);
    }

    #[test]
    fn load_address_from_sp_and_pc() {
        // ADD r0,sp,#8
        let mut m = Machine::with(0xA802, |cpu, _| cpu.regs[13] = 0x100);
        m.step();
        assert_eq!(m.cpu.regs[0], 0x108);

        // ADD r0,pc,#8: ((0 + 4) & !3) + 8
        let mut m = Machine::new(&[0xA002]);
        m.step();
        assert_eq!(m.cpu.regs[0], 12);
    }

    #[test]
    fn add_and_sub_stack_pointer() {
        let mut m = Machine::new(&[0xB004, 0xB084]);
        m.cpu.regs[13] = 0x100;
        m.step();
        assert_eq!(m.cpu.regs[13], 0x110);
        m.step();
        assert_eq!(m.cpu.regs[13], 0x100);
    }

    #[test]
    fn legacy_unaligned_thumb_push_pop_align_memory_and_preserve_sp() {
        for big_endian in [false, true] {
            let mut m = Machine::new(&[0xB403, 0xBC03]);
            m.cpu.regs[SP] = DATA + 0x81;
            m.cpu.regs[0] = 0x1122_3344;
            m.cpu.regs[1] = 0x5566_7788;
            m.cpu.cpsr.set_big_endian(big_endian);

            m.step(); // PUSH {r0,r1}
            assert_eq!(m.cpu.regs[SP], DATA + 0x79);
            assert_eq!(
                m.mem.read32(DATA + 0x78).unwrap(),
                if big_endian { 0x4433_2211 } else { 0x1122_3344 }
            );
            assert_eq!(
                m.mem.read32(DATA + 0x7C).unwrap(),
                if big_endian { 0x8877_6655 } else { 0x5566_7788 }
            );

            m.cpu.regs[0] = 0;
            m.cpu.regs[1] = 0;
            m.step(); // POP {r0,r1}
            assert_eq!(m.cpu.regs[0], 0x1122_3344);
            assert_eq!(m.cpu.regs[1], 0x5566_7788);
            assert_eq!(m.cpu.regs[SP], DATA + 0x81);
        }
    }

    #[test]
    fn thumb_push_pop_fault_on_unaligned_stack_when_sctlr_requires_alignment() {
        for control_bit in [1 << 22, 1 << 1] {
            for half in [0xB403, 0xBC03] {
                let mut m = Machine::with(half, |cpu, mem| {
                    cpu.regs[SP] = DATA + 0x81;
                    cpu.regs[0] = 0x1122_3344;
                    cpu.regs[1] = 0x5566_7788;
                    cpu.cp15.write(true, 0, 1, 0, 0, control_bit).unwrap();
                    mem.write32(DATA + 0x78, 0xCAFE_BABE).unwrap();
                    mem.write32(DATA + 0x7C, 0xDEAD_BEEF).unwrap();
                });
                let old_regs = m.cpu.regs;
                let old_first = m.mem.read32(DATA + 0x78).unwrap();
                let old_second = m.mem.read32(DATA + 0x7C).unwrap();
                let expected_address = if half == 0xB403 {
                    DATA + 0x79
                } else {
                    DATA + 0x81
                };

                assert!(matches!(
                    m.cpu.step(&mut m.mem),
                    Err(Error::InvalidAccess { addr }) if addr == expected_address
                ));
                assert_eq!(m.cpu.regs, old_regs);
                assert_eq!(m.cpu.pc(), 0);
                assert_eq!(m.mem.read32(DATA + 0x78).unwrap(), old_first);
                assert_eq!(m.mem.read32(DATA + 0x7C).unwrap(), old_second);
            }
        }
    }

    #[test]
    fn push_and_pop_round_trip() {
        // PUSH {r4,lr} ; POP {r4,pc}
        let mut m = Machine::new(&[0xB510, 0xBD10]);
        m.cpu.regs[13] = 0x900;
        m.cpu.regs[4] = 0x44;
        m.cpu.regs[14] = 0x21;

        m.step();
        assert_eq!(m.cpu.regs[13], 0x8F8);
        assert_eq!(m.mem.read32(0x8F8).unwrap(), 0x44);
        assert_eq!(m.mem.read32(0x8FC).unwrap(), 0x21);

        m.cpu.regs[4] = 0;
        m.step();
        assert_eq!(m.cpu.regs[4], 0x44);
        assert_eq!(m.cpu.regs[13], 0x900);
        assert_eq!(m.cpu.pc(), 0x20);
        assert!(m.cpu.cpsr.thumb(), "bit 0 of the popped value keeps Thumb");
    }

    #[test]
    fn empty_thumb_push_and_pop_register_lists_are_rejected_atomically() {
        for half in [0xB400, 0xBC00] {
            let mut m = Machine::with(half, |cpu, mem| {
                cpu.regs[SP] = DATA;
                mem.write32(DATA, 0xC0DE_CAFE).unwrap();
            });

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == u32::from(half)
            ));
            assert_eq!(m.cpu.pc(), 0);
            assert_eq!(m.cpu.regs[SP], DATA);
            assert_eq!(m.mem.read32(DATA).unwrap(), 0xC0DE_CAFE);
        }
    }

    #[test]
    fn pop_pc_with_even_address_switches_to_arm() {
        let mut m = Machine::new(&[0xBD00]); // POP {pc}
        m.cpu.regs[13] = DATA;
        m.mem.write32(DATA, 0x40).unwrap();
        m.step();
        assert_eq!(m.cpu.pc(), 0x40);
        assert!(!m.cpu.cpsr.thumb());
    }

    #[test]
    fn pop_pc_rejects_halfword_aligned_arm_target_without_mutating_state() {
        let mut m = Machine::new(&[0xBD00]); // POP {pc}
        m.cpu.regs[SP] = DATA;
        m.mem.write32(DATA, 0x42).unwrap();
        let old_cpsr = m.cpu.cpsr;

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xBD00
            })
        ));
        assert_eq!(m.cpu.pc(), 0);
        assert_eq!(m.cpu.regs[SP], DATA);
        assert_eq!(m.cpu.cpsr, old_cpsr);
    }

    #[test]
    fn stmia_and_ldmia_with_writeback() {
        // STMIA r0!,{r1,r2}
        let mut m = Machine::with(0xC006, |cpu, _| {
            cpu.regs[0] = DATA;
            cpu.regs[1] = 10;
            cpu.regs[2] = 20;
        });
        m.step();
        assert_eq!(m.mem.read32(DATA).unwrap(), 10);
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 20);
        assert_eq!(m.cpu.regs[0], DATA + 8);

        // LDMIA r0!,{r1,r2}
        let mut m = Machine::with(0xC806, |cpu, mem| {
            cpu.regs[0] = DATA;
            mem.write32(DATA, 7).unwrap();
            mem.write32(DATA + 4, 8).unwrap();
        });
        m.step();
        assert_eq!((m.cpu.regs[1], m.cpu.regs[2]), (7, 8));
        assert_eq!(m.cpu.regs[0], DATA + 8);
    }

    #[test]
    fn multiple_transfers_preserve_architectural_base_register_rules() {
        // STMIA r0!,{r0,r1}: when Rn is the lowest register, its old value is stored.
        let mut store = Machine::with(0xC003, |cpu, _| {
            cpu.regs[0] = DATA;
            cpu.regs[1] = 0x1122_3344;
        });
        store.step();
        assert_eq!(store.mem.read32(DATA).unwrap(), DATA);
        assert_eq!(store.mem.read32(DATA + 4).unwrap(), 0x1122_3344);
        assert_eq!(store.cpu.regs[0], DATA + 8);

        // LDMIA r0!,{r0,r1}: the loaded base value wins and writeback is suppressed.
        let mut load = Machine::with(0xC803, |cpu, mem| {
            cpu.regs[0] = DATA;
            mem.write32(DATA, DATA + 0x100).unwrap();
            mem.write32(DATA + 4, 0x5566_7788).unwrap();
        });
        load.step();
        assert_eq!(load.cpu.regs[0], DATA + 0x100);
        assert_eq!(load.cpu.regs[1], 0x5566_7788);
    }

    #[test]
    fn legacy_unaligned_thumb_multiple_transfers_align_memory_and_preserve_base_writeback() {
        for big_endian in [false, true] {
            let first: u32 = 0x1122_3344;
            let second: u32 = 0x5566_7788;
            let mut load = Machine::with(0xC806, |cpu, mem| {
                cpu.regs[0] = DATA + 1;
                cpu.cpsr.set_big_endian(big_endian);
                mem.write32(
                    DATA,
                    if big_endian {
                        first.swap_bytes()
                    } else {
                        first
                    },
                )
                .unwrap();
                mem.write32(
                    DATA + 4,
                    if big_endian {
                        second.swap_bytes()
                    } else {
                        second
                    },
                )
                .unwrap();
            });
            load.step();

            assert_eq!((load.cpu.regs[1], load.cpu.regs[2]), (first, second));
            assert_eq!(
                load.cpu.regs[0],
                DATA + 9,
                "LDMIA writeback must use original base"
            );

            let mut store = Machine::with(0xC006, |cpu, _| {
                cpu.regs[0] = DATA + 1;
                cpu.regs[1] = first;
                cpu.regs[2] = second;
                cpu.cpsr.set_big_endian(big_endian);
            });
            store.step();

            assert_eq!(
                store.mem.read32(DATA).unwrap(),
                if big_endian {
                    first.swap_bytes()
                } else {
                    first
                }
            );
            assert_eq!(
                store.mem.read32(DATA + 4).unwrap(),
                if big_endian {
                    second.swap_bytes()
                } else {
                    second
                }
            );
            assert_eq!(
                store.cpu.regs[0],
                DATA + 9,
                "STMIA writeback must use original base"
            );
        }
    }

    #[test]
    fn thumb_multiple_transfers_fault_on_unaligned_base_when_sctlr_requires_alignment() {
        for control_bit in [1 << 22, 1 << 1] {
            for half in [0xC806, 0xC006] {
                let mut m = Machine::with(half, |cpu, mem| {
                    cpu.regs[0] = DATA + 1;
                    cpu.regs[1] = 0x1122_3344;
                    cpu.regs[2] = 0x5566_7788;
                    cpu.cp15.write(true, 0, 1, 0, 0, control_bit).unwrap();
                    mem.write32(DATA, 0xCAFE_BABE).unwrap();
                    mem.write32(DATA + 4, 0xDEAD_BEEF).unwrap();
                });
                let old_regs = m.cpu.regs;
                let old_first = m.mem.read32(DATA).unwrap();
                let old_second = m.mem.read32(DATA + 4).unwrap();

                assert!(matches!(
                    m.cpu.step(&mut m.mem),
                    Err(Error::InvalidAccess { addr }) if addr == DATA + 1
                ));
                assert_eq!(m.cpu.regs, old_regs);
                assert_eq!(m.cpu.pc(), 0);
                assert_eq!(m.mem.read32(DATA).unwrap(), old_first);
                assert_eq!(m.mem.read32(DATA + 4).unwrap(), old_second);
            }
        }
    }

    #[test]
    fn stmia_rejects_writeback_base_that_is_not_lowest_in_register_list() {
        // STMIA r2!,{r1,r2} has no architecturally reliable stored value for R2.
        let mut m = Machine::with(0xC206, |cpu, mem| {
            cpu.regs[1] = 0x1122_3344;
            cpu.regs[2] = DATA;
            mem.write32(DATA, 0xCAFE_BABE).unwrap();
            mem.write32(DATA + 4, 0xDEAD_BEEF).unwrap();
        });

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xC206
            })
        ));
        assert_eq!(m.cpu.pc(), 0);
        assert_eq!(m.cpu.regs[2], DATA);
        assert_eq!(m.mem.read32(DATA).unwrap(), 0xCAFE_BABE);
        assert_eq!(m.mem.read32(DATA + 4).unwrap(), 0xDEAD_BEEF);
    }

    // ----- branches ---------------------------------------------------------

    #[test]
    fn conditional_branch_taken_and_not_taken() {
        // BNE self (offset -2 -> target = pc + 4 - 4)
        let mut m = Machine::new(&[0xD1FE]);
        m.step();
        assert_eq!(m.cpu.pc(), 0);

        let mut m = Machine::new(&[0xD1FE]);
        m.cpu.cpsr.set_flags(false, true, false, false);
        m.step();
        assert_eq!(m.cpu.pc(), 2);

        // BEQ +0 (target pc + 4)
        let mut m = Machine::new(&[0xD000]);
        m.cpu.cpsr.set_flags(false, true, false, false);
        m.step();
        assert_eq!(m.cpu.pc(), 4);
    }

    #[test]
    fn unconditional_branch() {
        let mut m = Machine::new(&[0xE000]); // B +0 -> pc + 4
        m.step();
        assert_eq!(m.cpu.pc(), 4);

        let mut m = Machine::new(&[0xE7FE]); // B .
        m.step();
        assert_eq!(m.cpu.pc(), 0);
    }

    #[test]
    fn bkpt_reports_its_immediate_and_retires_without_mutating_cpu_state() {
        let mut m = Machine::with(0xBEA5, |cpu, _| {
            cpu.regs[0] = 0x1234_5678;
            cpu.regs[LR] = 0x8765_4321;
            cpu.cpsr.set_flags(true, false, true, true);
        });
        let old_regs = m.cpu.regs;
        let old_flags = m.flags();
        let old_mode = m.cpu.cpsr.mode();

        assert_eq!(m.step(), StepEvent::Breakpoint(0xA5));
        assert_eq!(&m.cpu.regs[..PC], &old_regs[..PC]);
        assert_eq!(m.flags(), old_flags);
        assert_eq!(m.cpu.cpsr.mode(), old_mode);
        assert_eq!(m.cpu.pc(), 2);

        // The entire imm8 field is surfaced to the debugger.
        let mut max_immediate = Machine::with(0xBEFF, |_, _| {});
        assert_eq!(max_immediate.step(), StepEvent::Breakpoint(0xFF));
        assert_eq!(max_immediate.cpu.pc(), 2);
    }

    #[test]
    fn swi_is_intercepted_for_hle_dispatch_in_thumb_mode_by_default() {
        let mut m = Machine::new(&[0xDF21]);
        assert_eq!(m.step(), StepEvent::Svc(0x21));
        assert_eq!(m.cpu.pc(), 0x02);
        assert_eq!(m.cpu.cpsr.mode(), Psr::USER_MODE);
        assert!(m.cpu.cpsr.thumb());
        assert_eq!(m.cpu.saved_program_status(), None);
    }

    #[test]
    fn swi_enters_supervisor_arm_state_when_hardware_entry_is_enabled() {
        let mut m = Machine::new(&[0xDF21]);
        let original_cpsr = m.cpu.cpsr;
        m.cpu.set_svc_exception_entry_enabled(true);

        assert_eq!(m.step(), StepEvent::Svc(0x21));
        assert_eq!(m.cpu.pc(), 0x08);
        assert_eq!(m.cpu.regs[14], 0x02);
        assert_eq!(m.cpu.cpsr.mode(), Psr::SUPERVISOR_MODE);
        assert!(!m.cpu.cpsr.thumb());
        assert_ne!(m.cpu.cpsr.0 & (1 << 7), 0);
        assert_eq!(m.cpu.saved_program_status(), Some(original_cpsr));
    }

    #[test]
    fn thumb_swi_returns_to_the_instruction_after_the_call() {
        // The 32-bit ARM MOVS pc,lr handler is placed at vector 0x08.
        let mut m = Machine::new(&[0xDF21, 0x46C0, 0x46C0, 0x46C0, 0xF00E, 0xE1B0]);
        let original_cpsr = m.cpu.cpsr;
        m.cpu.set_svc_exception_entry_enabled(true);

        assert_eq!(m.step(), StepEvent::Svc(0x21));
        assert_eq!(m.cpu.pc(), 0x08);
        assert_eq!(m.step(), StepEvent::Executed);
        assert_eq!(m.cpu.pc(), 0x02);
        assert_eq!(m.cpu.cpsr, original_cpsr);
    }

    #[test]
    fn condition_1110_is_undefined() {
        let mut m = Machine::new(&[0xDE00]);
        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction { pc: 0, .. })
        ));
    }

    #[test]
    fn bl_pair_branches_and_links() {
        // BL +4: prefix stages lr, suffix (imm11 = 2) completes the branch.
        let mut m = Machine::new(&[0xF000, 0xF802]);
        m.step();
        assert_eq!((m.cpu.regs[14], m.cpu.pc()), (4, 2));
        m.step();
        assert_eq!(m.cpu.pc(), 8);
        assert_eq!(m.cpu.regs[14], 5, "return address 4 with the Thumb bit");
        assert!(m.cpu.cpsr.thumb());
    }

    #[test]
    fn bl_pair_sign_extends_negative_high_offset() {
        // A high immediate of -1 contributes -4096 to the paired branch target.
        let mut m = Machine::new(&[0xF7FF, 0xF800]);

        m.step();
        assert_eq!(m.cpu.regs[LR], 0xFFFF_F004);
        m.step();

        assert_eq!(m.cpu.pc(), 0xFFFF_F004);
        assert_eq!(m.cpu.regs[LR], 5);
        assert!(m.cpu.cpsr.thumb());
    }

    #[test]
    fn blx_pair_switches_to_arm() {
        let mut m = Machine::new(&[0xF000, 0xE800]);
        m.step();
        m.step();
        assert_eq!(m.cpu.pc(), 4);
        assert_eq!(m.cpu.regs[14], 5);
        assert!(!m.cpu.cpsr.thumb());
    }

    #[test]
    fn bx_and_blx_register_encodings_reject_nonzero_reserved_bits_atomically() {
        for half in [0x4701, 0x4781] {
            let mut m = Machine::with(half, |cpu, _| {
                cpu.regs[0] = 0x41;
                cpu.regs[LR] = 0xDEAD_BEEF;
            });
            let old_cpsr = m.cpu.cpsr;

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == u32::from(half)
            ));
            assert_eq!(m.cpu.pc(), 0);
            assert_eq!(m.cpu.regs[LR], 0xDEAD_BEEF);
            assert_eq!(m.cpu.cpsr, old_cpsr);
        }
    }

    #[test]
    fn bx_rejects_a_halfword_aligned_arm_target() {
        let mut m = Machine::with(0x4700, |cpu, _| cpu.regs[0] = 0x102);

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0x4700
            })
        ));
        assert_eq!(m.cpu.pc(), 0);
        assert!(m.cpu.cpsr.thumb());
    }

    #[test]
    fn blx_register_rejects_pc_and_halfword_aligned_arm_targets_atomically() {
        for (half, target) in [(0x47F8, 0), (0x4780, 0x102)] {
            let mut m = Machine::with(half, |cpu, _| {
                cpu.regs[0] = target;
                cpu.regs[LR] = 0xCAFE_BABE;
            });
            let old_cpsr = m.cpu.cpsr;

            assert!(matches!(
                m.cpu.step(&mut m.mem),
                Err(Error::UndefinedInstruction { pc: 0, word }) if word == u32::from(half)
            ));
            assert_eq!(m.cpu.pc(), 0);
            assert_eq!(m.cpu.regs[LR], 0xCAFE_BABE);
            assert_eq!(m.cpu.cpsr, old_cpsr);
        }
    }

    #[test]
    fn blx_immediate_suffix_rejects_nonzero_fixed_bit_atomically() {
        let mut m = Machine::new(&[0xF000, 0xE801]);
        m.step();
        let staged_lr = m.cpu.regs[LR];

        assert!(matches!(
            m.cpu.step(&mut m.mem),
            Err(Error::UndefinedInstruction {
                pc: 2,
                word: 0xE801
            })
        ));
        assert_eq!(m.cpu.pc(), 2);
        assert_eq!(m.cpu.regs[LR], staged_lr);
        assert!(m.cpu.cpsr.thumb());
    }

    // ----- ARMv6 extras and hints ---------------------------------------------

    fn misc(half: u16, value: u32) -> u32 {
        let mut m = Machine::with(half, |cpu, _| cpu.regs[1] = value);
        m.step();
        m.cpu.regs[0]
    }

    #[test]
    fn sign_and_zero_extension() {
        assert_eq!(misc(0xB208, 0x8000), 0xFFFF_8000); // SXTH
        assert_eq!(misc(0xB248, 0x80), 0xFFFF_FF80); // SXTB
        assert_eq!(misc(0xB288, 0x1234_5678), 0x5678); // UXTH
        assert_eq!(misc(0xB2C8, 0x1FF), 0xFF); // UXTB
    }

    #[test]
    fn byte_reversal() {
        assert_eq!(misc(0xBA08, 0x1122_3344), 0x4433_2211); // REV
        assert_eq!(misc(0xBA48, 0x1122_3344), 0x2211_4433); // REV16
        assert_eq!(misc(0xBAC8, 0x0000_00FF), 0xFFFF_FF00); // REVSH
    }

    #[test]
    fn thumb_cpsid_and_cpsie_update_interrupt_masks_without_touching_flags() {
        // CPSID i ; CPSIE i ; CPSID aif ; CPSIE aif.
        let mut m = Machine::new(&[0xB672, 0xB662, 0xB677, 0xB667]);
        m.cpu.cpsr.0 = (m.cpu.cpsr.0 & !0x1F) | 0x13; // Supervisor mode.
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
    fn thumb_cps_maps_async_abort_and_fiq_fields_individually() {
        // CPSID a ; CPSIE a ; CPSID f ; CPSIE f.
        let mut m = Machine::new(&[0xB674, 0xB664, 0xB671, 0xB661]);
        m.cpu.cpsr.0 = (m.cpu.cpsr.0 & !0x1F) | 0x13;

        m.step();
        assert_ne!(m.cpu.cpsr.0 & (1 << 8), 0);
        assert_eq!(m.cpu.cpsr.0 & (1 << 6), 0);
        m.step();
        assert_eq!(m.cpu.cpsr.0 & (1 << 8), 0);
        m.step();
        assert_ne!(m.cpu.cpsr.0 & (1 << 6), 0);
        assert_eq!(m.cpu.cpsr.0 & (1 << 8), 0);
        m.step();
        assert_eq!(m.cpu.cpsr.0 & (1 << 6), 0);
    }

    #[test]
    fn thumb_setend_big_and_little_endian_change_only_the_cpsr_e_bit() {
        // SETEND BE ; SETEND LE. Encodings are verified against ARMv6K.
        let mut m = Machine::new(&[0xB658, 0xB650]);
        m.cpu.cpsr.set_flags(true, false, true, false);
        m.cpu.cpsr.set_q();
        m.cpu.cpsr.set_ge(0b1010);
        let stable = m.cpu.cpsr.0 & !(1 << 9);

        m.step();
        assert!(m.cpu.cpsr.big_endian());
        assert_eq!(m.cpu.cpsr.0 & !(1 << 9), stable);

        m.step();
        assert!(!m.cpu.cpsr.big_endian());
        assert_eq!(m.cpu.cpsr.0 & !(1 << 9), stable);
        assert_eq!(m.flags(), (true, false, true, false));
        assert!(m.cpu.cpsr.q());
        assert_eq!(m.cpu.cpsr.ge(), 0b1010);
    }

    #[test]
    fn thumb_cps_is_a_noop_in_user_mode() {
        let mut m = Machine::new(&[0xB672]); // CPSID i.
        m.step();
        assert_eq!(m.cpu.cpsr.0 & (1 << 7), 0);
    }

    #[test]
    fn empty_thumb_block_transfer_lists_are_undefined() {
        for half in [0xC000, 0xC800] {
            let mut machine = Machine::new(&[half]);
            assert!(
                matches!(
                    machine.cpu.step(&mut machine.mem),
                    Err(Error::UndefinedInstruction { pc: 0, .. })
                ),
                "empty Thumb LDM/STM list {half:#06x} must be rejected as undefined"
            );
            assert_eq!(machine.cpu.pc(), 0);
        }
    }

    #[test]
    fn reserved_thumb_miscellaneous_encodings_are_undefined() {
        // BF01 is in the 16-bit hint space but is not a defined ARMv6K hint.
        let mut machine = Machine::new(&[0xBF01]);
        assert!(matches!(
            machine.cpu.step(&mut machine.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xBF01
            })
        ));
        assert_eq!(machine.cpu.pc(), 0);
    }

    #[test]
    fn armv6k_thumb_rejects_the_armv6t2_nop_hint() {
        let mut machine = Machine::with(0xBF00, |_, _| {});
        assert!(matches!(
            machine.cpu.step(&mut machine.mem),
            Err(Error::UndefinedInstruction {
                pc: 0,
                word: 0xBF00
            })
        ));
        assert_eq!(machine.cpu.pc(), 0);
    }

    #[test]
    fn armv6k_thumb_rejects_yield_wfe_wfi_and_sev_hint_encodings() {
        for half in [0xBF10, 0xBF20, 0xBF30, 0xBF40] {
            let mut machine = Machine::with(half, |_, _| {});
            assert!(
                matches!(
                    machine.cpu.step(&mut machine.mem),
                    Err(Error::UndefinedInstruction { .. })
                ),
                "expected ARMv6K Thumb hint {half:#06x} to be undefined"
            );
        }
    }

    #[test]
    fn mov_register_to_itself_has_no_visible_effect() {
        let mut m = Machine::with(0x46C0, |cpu, _| {
            cpu.regs[8] = 0x1234_5678;
            cpu.cpsr.set_flags(true, false, true, false);
        });
        let old_status = m.cpu.cpsr;

        m.step();

        assert_eq!(m.cpu.regs[8], 0x1234_5678);
        assert_eq!(m.cpu.cpsr, old_status);
        assert_eq!(m.cpu.pc(), 2);
    }

    // ----- ARM <-> Thumb ------------------------------------------------------

    #[test]
    fn arm_code_can_branch_into_thumb_code() {
        // ARM: MOV r0,#0x11 ; BX r0   ->   Thumb at 0x10: MOV r1,#7
        let mut mem = FlatMemory::new(0, 0x1000);
        mem.write32(0, 0xE3A0_0011).unwrap();
        mem.write32(4, 0xE12F_FF10).unwrap();
        mem.write16(0x10, 0x2107).unwrap();

        let mut cpu = Arm11Core::new();
        for _ in 0..3 {
            cpu.step(&mut mem).unwrap();
        }
        assert_eq!(cpu.regs[1], 7);
        assert_eq!(cpu.pc(), 0x12);
        assert!(cpu.cpsr.thumb());
    }
}
