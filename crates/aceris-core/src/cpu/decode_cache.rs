//! Pre-decoded execution of the most frequent A32 instruction forms for the
//! fast dispatch loop.
//!
//! The cache is direct-mapped and keyed by the instruction word itself, not by
//! its address. Decoding depends only on the 32 encoding bits, so the cache
//! never needs invalidation when code is written, remapped or unmapped: a
//! changed instruction simply has a different key. This mirrors the "decode
//! once, dispatch many times" idea of PPSSPP's IR interpreter (studied
//! read-only, GPLv2+) without its block structure; the design and code are
//! Aceris's own.
//!
//! Only forms whose architectural behaviour is fully reproduced here are
//! decoded. Everything else, including every encoding the generic path would
//! reject as UNDEFINED/UNPREDICTABLE and every use of R15, falls back to
//! [`Arm11Core::execute_arm`], so results are identical to the precise
//! interpreter by construction. The differential test at the end of this file
//! checks that claim instruction by instruction.

use super::shifter::{ShiftKind, add_with_carry, shift_imm};
use super::{Arm11Core, Condition, PC, StepEvent};
use crate::memory::Bus;
use aceris_common::Result;

/// Number of cache entries (power of two).
const ENTRIES: usize = 4096;

/// A decoded instruction body (the condition is stored beside it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FastOp {
    /// Not decoded: run through `execute_arm`.
    Generic,
    /// Data processing with a rotated immediate operand. `imm_carry` is the
    /// shifter carry-out, or `None` when the rotation is zero (carry kept).
    DpImm {
        opcode: u8,
        set_flags: bool,
        rn: u8,
        rd: u8,
        imm: u32,
        imm_carry: Option<bool>,
    },
    /// Data processing with an immediate-shifted register operand.
    DpReg {
        opcode: u8,
        set_flags: bool,
        rn: u8,
        rd: u8,
        rm: u8,
        kind: ShiftKind,
        amount: u8,
    },
    /// LDR/STR/LDRB/STRB with a 12-bit immediate offset.
    TransferImm {
        load: bool,
        byte: bool,
        pre_index: bool,
        writeback: bool,
        rn: u8,
        rd: u8,
        /// Signed byte offset (already negated for U=0).
        offset: i32,
    },
    /// B / BL.
    Branch,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    word: u32,
    condition: u8,
    op: FastOp,
}

const EMPTY: Entry = Entry {
    // 0xFFFF_FFFF is never decoded (condition 0xF stays generic), so an
    // empty slot can never be mistaken for a hit.
    word: u32::MAX,
    condition: 0xF,
    op: FastOp::Generic,
};

/// Direct-mapped cache of decoded A32 instructions.
#[derive(Clone)]
pub(super) struct DecodeCache {
    entries: Box<[Entry]>,
}

impl Default for DecodeCache {
    fn default() -> Self {
        Self {
            entries: vec![EMPTY; ENTRIES].into_boxed_slice(),
        }
    }
}

impl std::fmt::Debug for DecodeCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeCache").finish_non_exhaustive()
    }
}

impl DecodeCache {
    #[inline]
    fn slot(word: u32) -> usize {
        (word.wrapping_mul(0x9E37_79B1) >> 20) as usize & (ENTRIES - 1)
    }

    #[inline]
    fn lookup(&mut self, word: u32) -> Entry {
        let slot = Self::slot(word);
        let entry = self.entries[slot];
        if entry.word == word {
            return entry;
        }
        let decoded = Entry {
            word,
            condition: (word >> 28) as u8,
            op: decode(word),
        };
        self.entries[slot] = decoded;
        decoded
    }
}

#[inline]
fn bit(word: u32, n: u32) -> bool {
    word >> n & 1 != 0
}

#[inline]
fn field(word: u32, high: u32, low: u32) -> u32 {
    (word >> low) & ((1 << (high - low + 1)) - 1)
}

/// Decodes the supported forms; anything else is `Generic`.
fn decode(word: u32) -> FastOp {
    let condition = word >> 28;
    if condition == 0xF {
        return FastOp::Generic;
    }
    let pc = PC as u32;
    match field(word, 27, 25) {
        0b000 | 0b001 => {
            let immediate = bit(word, 25);
            // Register forms with bit 4 set are register-shifted operands,
            // multiplies or extra load/stores.
            if !immediate && bit(word, 4) {
                return FastOp::Generic;
            }
            let opcode = field(word, 24, 21);
            let set_flags = bit(word, 20);
            let rn = field(word, 19, 16);
            let rd = field(word, 15, 12);
            let compare = (0x8..=0xB).contains(&opcode);
            // TST/TEQ/CMP/CMN without S are MRS/MSR/BX/CLZ/hints.
            if compare && !set_flags {
                return FastOp::Generic;
            }
            // Encodings the generic path rejects, and every R15 operand.
            if (compare && rd != 0) || (matches!(opcode, 0xD | 0xF) && rn != 0) {
                return FastOp::Generic;
            }
            if rd == pc || rn == pc {
                return FastOp::Generic;
            }
            if immediate {
                let rotation = field(word, 11, 8) * 2;
                let imm = field(word, 7, 0).rotate_right(rotation);
                FastOp::DpImm {
                    opcode: opcode as u8,
                    set_flags,
                    rn: rn as u8,
                    rd: rd as u8,
                    imm,
                    imm_carry: (rotation != 0).then_some(imm >> 31 != 0),
                }
            } else {
                let rm = field(word, 3, 0);
                if rm == pc {
                    return FastOp::Generic;
                }
                FastOp::DpReg {
                    opcode: opcode as u8,
                    set_flags,
                    rn: rn as u8,
                    rd: rd as u8,
                    rm: rm as u8,
                    kind: ShiftKind::from_bits(field(word, 6, 5)),
                    amount: field(word, 11, 7) as u8,
                }
            }
        }
        0b010 => {
            let pre_index = bit(word, 24);
            let writeback_bit = bit(word, 21);
            // P=0,W=1 are the unprivileged LDRT/STRT forms.
            if !pre_index && writeback_bit {
                return FastOp::Generic;
            }
            let writeback = !pre_index || writeback_bit;
            let rn = field(word, 19, 16);
            let rd = field(word, 15, 12);
            if rn == pc || rd == pc || (writeback && rn == rd) {
                return FastOp::Generic;
            }
            let magnitude = field(word, 11, 0) as i32;
            FastOp::TransferImm {
                load: bit(word, 20),
                byte: bit(word, 22),
                pre_index,
                writeback,
                rn: rn as u8,
                rd: rd as u8,
                offset: if bit(word, 23) { magnitude } else { -magnitude },
            }
        }
        0b101 => FastOp::Branch,
        _ => FastOp::Generic,
    }
}

impl Arm11Core {
    /// Executes one A32 instruction through the decode cache. Behaviour is
    /// identical to [`Self::execute_arm`].
    #[inline]
    pub(super) fn execute_arm_cached<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        let entry = self.decode_cache.lookup(word);
        if entry.op == FastOp::Generic {
            return self.execute_arm(bus, word);
        }
        if !Condition::from_bits(entry.condition).passes(self.cpsr) {
            return Ok(StepEvent::Executed);
        }
        match entry.op {
            FastOp::DpImm {
                opcode,
                set_flags,
                rn,
                rd,
                imm,
                imm_carry,
            } => {
                let carry = imm_carry.unwrap_or_else(|| self.cpsr.c());
                self.execute_alu(opcode, set_flags, rn, rd, imm, carry);
            }
            FastOp::DpReg {
                opcode,
                set_flags,
                rn,
                rd,
                rm,
                kind,
                amount,
            } => {
                let (operand, carry) = shift_imm(
                    kind,
                    self.regs[usize::from(rm)],
                    u32::from(amount),
                    self.cpsr.c(),
                );
                self.execute_alu(opcode, set_flags, rn, rd, operand, carry);
            }
            FastOp::TransferImm {
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
                        self.read_armv6_single_word(bus, address, false, word)?
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
                        self.write_armv6_single_word(bus, address, value, false, word)?;
                    }
                    if writeback {
                        self.regs[usize::from(rn)] = offset_address;
                    }
                }
            }
            FastOp::Branch => self.branch_with_link(word),
            FastOp::Generic => unreachable!("handled above"),
        }
        Ok(StepEvent::Executed)
    }

    /// ALU core shared by the decoded data-processing forms. `rd` and `rn`
    /// are never R15 here.
    #[inline]
    fn execute_alu(
        &mut self,
        opcode: u8,
        set_flags: bool,
        rn: u8,
        rd: u8,
        operand: u32,
        shifter_carry: bool,
    ) {
        let a = self.regs[usize::from(rn)];
        let carry_in = self.cpsr.c();
        // (result, Some((carry, overflow))) for arithmetic, None for logical.
        let (result, arithmetic) = match opcode {
            0x0 | 0x8 => (a & operand, None),
            0x1 | 0x9 => (a ^ operand, None),
            0x2 | 0xA => {
                let (v, c, o) = add_with_carry(a, !operand, true);
                (v, Some((c, o)))
            }
            0x3 => {
                let (v, c, o) = add_with_carry(operand, !a, true);
                (v, Some((c, o)))
            }
            0x4 | 0xB => {
                let (v, c, o) = add_with_carry(a, operand, false);
                (v, Some((c, o)))
            }
            0x5 => {
                let (v, c, o) = add_with_carry(a, operand, carry_in);
                (v, Some((c, o)))
            }
            0x6 => {
                let (v, c, o) = add_with_carry(a, !operand, carry_in);
                (v, Some((c, o)))
            }
            0x7 => {
                let (v, c, o) = add_with_carry(operand, !a, carry_in);
                (v, Some((c, o)))
            }
            0xC => (a | operand, None),
            0xD => (operand, None),
            0xE => (a & !operand, None),
            _ => (!operand, None),
        };
        if set_flags {
            let (carry, overflow) = arithmetic.unwrap_or((shifter_carry, self.cpsr.v()));
            self.cpsr
                .set_flags(result >> 31 != 0, result == 0, carry, overflow);
        }
        if !(0x8..=0xB).contains(&opcode) {
            self.regs[usize::from(rd)] = result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    const MEMORY_SIZE: u32 = 0x2000;

    /// Small deterministic generator (xorshift) for the differential test.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 16) as u32
        }
    }

    fn fresh_core(rng: &mut Rng) -> Arm11Core {
        let mut core = Arm11Core::default();
        for register in 0..15 {
            // Keep base registers inside the 64 KiB test memory most of the time.
            core.regs[register] = if rng.next() & 1 == 0 {
                0x800 + (rng.next() & 0xFFC)
            } else {
                rng.next()
            };
        }
        core.regs[PC] = 0x100;
        let flags = rng.next() & 0xF000_0000;
        core.cpsr = super::super::Psr((core.cpsr.0 & 0x0FFF_FFFF) | flags);
        core
    }

    /// Generates words biased toward the decoded classes.
    fn random_word(rng: &mut Rng) -> u32 {
        let raw = rng.next();
        let class = match rng.next() % 4 {
            0 => 0b000,
            1 => 0b001,
            2 => 0b010,
            _ => 0b101,
        };
        let condition = if rng.next().is_multiple_of(3) {
            rng.next() >> 28
        } else {
            0xE
        };
        (condition << 28) | (class << 25) | (raw & 0x01FF_FFFF)
    }

    #[test]
    fn decoded_execution_matches_the_generic_interpreter() {
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        let mut decoded_hits = 0;
        let mut template = FlatMemory::new(0, MEMORY_SIZE as usize);
        for index in (0..MEMORY_SIZE).step_by(4) {
            template.write32(index, rng.next()).unwrap();
        }
        for _ in 0..60_000 {
            let word = random_word(&mut rng);
            let seed_core = fresh_core(&mut rng);
            let mut memory_a = template.clone();
            let mut memory_b = memory_a.clone();
            let mut generic = seed_core.clone();
            let mut cached = seed_core;
            generic.branch_taken = false;
            cached.branch_taken = false;

            let expected = generic.execute_arm(&mut memory_a, word);
            let actual = cached.execute_arm_cached(&mut memory_b, word);
            if decode(word) != FastOp::Generic {
                decoded_hits += 1;
            }
            assert_eq!(
                format!("{expected:?}"),
                format!("{actual:?}"),
                "result for {word:#010X}"
            );
            assert_eq!(generic.regs, cached.regs, "registers for {word:#010X}");
            assert_eq!(generic.cpsr, cached.cpsr, "CPSR for {word:#010X}");
            assert_eq!(
                generic.branch_taken, cached.branch_taken,
                "branch for {word:#010X}"
            );
            for index in (0..MEMORY_SIZE).step_by(4) {
                assert_eq!(
                    memory_a.read32(index).unwrap(),
                    memory_b.read32(index).unwrap(),
                    "memory at {index:#X} for {word:#010X}"
                );
            }
        }
        assert!(
            decoded_hits > 15_000,
            "decoded forms exercised: {decoded_hits}"
        );
    }

    #[test]
    fn cache_hits_reuse_the_decoded_entry_and_never_alias_empty_slots() {
        let mut cache = DecodeCache::default();
        let add = 0xE280_0001; // add r0, r0, #1
        assert_ne!(cache.lookup(add).op, FastOp::Generic);
        assert_eq!(cache.lookup(add).word, add);
        assert_eq!(cache.lookup(u32::MAX).op, FastOp::Generic);
    }
}
