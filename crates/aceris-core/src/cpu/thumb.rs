//! Thumb (16-bit) instruction decoding and execution.
//!
//! Covers the ARMv6 Thumb set: the 19 classic formats, the two-halfword
//! `BL`/`BLX` pair, and the ARMv6 additions `REV*`, `SXT*` and `UXT*`.
//! Reading `pc` as an operand yields the instruction address plus 4.

use super::arm::{bit, bits};
use super::shifter::{ShiftKind, add_with_carry, shift_imm, shift_reg};
use super::{Arm11Core, Condition, LR, PC, SP, StepEvent};
use crate::memory::Bus;
use aceris_common::{Error, Result};

/// Offset applied when a Thumb instruction reads `pc`.
const PC_OFFSET: u32 = 4;

/// Word load with the classic unaligned-rotate behaviour.
fn load_word<B: Bus + ?Sized>(core: &Arm11Core, bus: &mut B, addr: u32) -> Result<u32> {
    core.read_data_u32_unaligned(bus, addr)
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
                0b001 => self.thumb_hi_register(half),
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
            _ => self.thumb_branch(half),
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

    fn thumb_hi_register(&mut self, half: u32) {
        let rd = bits(half, 2, 0) | (u32::from(bit(half, 7)) << 3);
        let rs = bits(half, 5, 3) | (u32::from(bit(half, 6)) << 3);
        let a = self.reg(rd, PC_OFFSET);
        let b = self.reg(rs, PC_OFFSET);

        match bits(half, 9, 8) {
            0 => self.thumb_write_hi(rd, a.wrapping_add(b)),
            1 => self.thumb_arith(None, a, !b, true),
            2 => self.thumb_write_hi(rd, b),
            _ => {
                // BX Rm, or BLX Rm when the H1 bit is set.
                if bit(half, 7) {
                    self.regs[LR] = self.pc().wrapping_add(2) | 1;
                }
                self.branch_exchange(b);
            }
        }
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
            0b000 => self.write_data_u32(bus, addr & !3, self.regs[rt])?,
            0b001 => self.write_data_u16(bus, addr, self.regs[rt] as u16)?,
            0b010 => bus.write8(addr, self.regs[rt] as u8)?,
            0b011 => self.regs[rt] = i32::from(bus.read8(addr)? as i8) as u32,
            0b100 => self.regs[rt] = load_word(self, bus, addr)?,
            0b101 => self.regs[rt] = u32::from(self.read_data_u16(bus, addr)?),
            0b110 => self.regs[rt] = u32::from(bus.read8(addr)?),
            _ => self.regs[rt] = i32::from(self.read_data_u16(bus, addr)? as i16) as u32,
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
                load_word(self, bus, addr)?
            };
        } else if byte {
            bus.write8(addr, self.regs[rt] as u8)?;
        } else {
            self.write_data_u32(bus, addr & !3, self.regs[rt])?;
        }
        Ok(())
    }

    fn thumb_halfword_offset<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let rn = bits(half, 5, 3) as usize;
        let rt = bits(half, 2, 0) as usize;
        let addr = self.regs[rn].wrapping_add(bits(half, 10, 6) * 2);

        if bit(half, 11) {
            self.regs[rt] = u32::from(self.read_data_u16(bus, addr)?);
        } else {
            self.write_data_u16(bus, addr, self.regs[rt] as u16)?;
        }
        Ok(())
    }

    fn thumb_sp_relative<B: Bus + ?Sized>(&mut self, bus: &mut B, half: u32) -> Result<()> {
        let rt = bits(half, 10, 8) as usize;
        let addr = self.regs[SP].wrapping_add(bits(half, 7, 0) * 4);

        if bit(half, 11) {
            self.regs[rt] = load_word(self, bus, addr)?;
        } else {
            self.write_data_u32(bus, addr & !3, self.regs[rt])?;
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

        let mut addr = self.regs[rn];
        let new_base = addr.wrapping_add(4 * list.count_ones());

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
        let new_sp = self.regs[SP].wrapping_sub(4 * list.count_ones());

        let regs = self.regs;
        let mut addr = new_sp;
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

        let mut addr = self.regs[SP];
        let mut values = [0u32; 16];
        for (r, value) in values.iter_mut().enumerate() {
            if list & (1 << r) != 0 {
                *value = self.read_data_u32(bus, addr)?;
                addr = addr.wrapping_add(4);
            }
        }

        self.regs[SP] = addr;
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
    fn thumb_branch(&mut self, half: u32) {
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
                // BLX suffix: continue in ARM state at a word-aligned target.
                let target = self.regs[LR].wrapping_add(imm11 << 1) & !3;
                self.regs[LR] = pc.wrapping_add(2) | 1;
                self.cpsr.set_thumb(false);
                self.branch(target);
            }
        }
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
    fn armv6_cpy_low_register_alias_moves_value_without_changing_flags() {
        let mut m = Machine::with(0x4608, |cpu, _| {
            cpu.regs[1] = 0xA5A5_5A5A;
            cpu.cpsr.set_flags(true, false, true, true);
        });
        let flags = m.cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V);

        m.step();

        assert_eq!(m.cpu.regs[0], 0xA5A5_5A5A);
        assert_eq!(
            m.cpu.cpsr.0 & (Psr::N | Psr::Z | Psr::C | Psr::V),
            flags
        );
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
    fn blx_pair_switches_to_arm() {
        let mut m = Machine::new(&[0xF000, 0xE800]);
        m.step();
        m.step();
        assert_eq!(m.cpu.pc(), 4);
        assert_eq!(m.cpu.regs[14], 5);
        assert!(!m.cpu.cpsr.thumb());
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
