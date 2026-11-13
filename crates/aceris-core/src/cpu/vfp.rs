//! VFPv2 floating-point coprocessor (cp10 single precision, cp11 double precision).
//!
//! Supported: the register bank, FPSCR/FPEXC/FPSID/MVFR0/MVFR1,
//! core<->VFP transfers, VLDR/VSTR/VLDM/VSTM/VPUSH/VPOP, scalar arithmetic
//! (VMLA family, VMUL, VADD, VSUB, VDIV, VSQRT), VABS/VNEG/VMOV, VCMP/VCMPE
//! and VCVT (float<->float and float<->integer).
//!
//! A verified subset of VFP11 short-vector arithmetic and unary operations is
//! supported. FPEXC's defined context fields and privileged FPINST/FPINST2
//! storage are modeled (FPINST has its defined reset value; FPINST2 uses a
//! deterministic zero for its architecturally UNPREDICTABLE reset). The legacy
//! FLDMX/FSTMX double context-transfer forms with odd word counts are also
//! implemented. Remaining limits include hardware-generated exception bounce,
//! scalar-only comparisons/conversions, and extreme precision/subnormal cases.
//! Scalar operations honor FPSCR.RMode and
//! accumulate IXC/UFC for inexact and tiny results. Single precision is
//! computed through `f64`; double precision residuals use TwoSum/FMA where
//! available.

use super::arm::{bit, bits};
use super::{Arm11Core, StepEvent};
use crate::memory::Bus;
use aceris_common::{Error, Result};

/// VFP system ID reported by the core (VFP11).
pub const FPSID: u32 = 0x4101_20B5;
/// VFP11 feature register 0: full VFP11 feature set described by ARM DDI 0274H.
pub const MVFR0: u32 = 0x1111_1111;
/// VFP11 feature register 1: no additional optional features in this profile.
pub const MVFR1: u32 = 0x0000_0000;

const FPSCR_WRITABLE: u32 = 0xF3F7_9F9F;
const FPSCR_IOE: u32 = 1 << 8;
const FPSCR_DZE: u32 = 1 << 9;
const FPSCR_OFE: u32 = 1 << 10;
const FPSCR_UFE: u32 = 1 << 11;
const FPSCR_IXE: u32 = 1 << 12;
const FPSCR_IDE: u32 = 1 << 15;
const FPSCR_SUPPORTED_TRAP_ENABLES: u32 = FPSCR_IOE | FPSCR_DZE | FPSCR_OFE | FPSCR_UFE;
const FPSCR_UNSUPPORTED_TRAP_ENABLES: u32 = FPSCR_IXE | FPSCR_IDE;
const FPSCR_EXCEPTION_STATUS: u32 = FPSCR_IOC | FPSCR_DZC | FPSCR_OFC | FPSCR_UFC | FPSCR_IXC;
const FPEXC_EXCEPTION_STATUS: u32 = FPEXC_INV | FPEXC_UFC | FPEXC_OFC | FPEXC_IOC;
const FPSCR_DN: u32 = 1 << 25;
const FPSCR_FZ: u32 = 1 << 24;
const FPSCR_IOC: u32 = 1 << 0;
const FPSCR_DZC: u32 = 1 << 1;
const FPSCR_OFC: u32 = 1 << 2;
const FPSCR_UFC: u32 = 1 << 3;
const FPSCR_IXC: u32 = 1 << 4;
const FPSCR_IDC: u32 = 1 << 7;
const FPEXC_EX: u32 = 1 << 31;
const FPEXC_EN: u32 = 1 << 30;
const FPEXC_FP2V: u32 = 1 << 28;
const FPEXC_VECITR: u32 = 0x7 << 8;
const FPEXC_INV: u32 = 1 << 7;
const FPEXC_UFC: u32 = 1 << 3;
const FPEXC_OFC: u32 = 1 << 2;
const FPEXC_IOC: u32 = 1 << 0;
const FPEXC_WRITABLE: u32 =
    FPEXC_EX | FPEXC_EN | FPEXC_FP2V | FPEXC_VECITR | FPEXC_INV | FPEXC_UFC | FPEXC_OFC | FPEXC_IOC;
const FPINST_RESET: u32 = 0xEE00_0A00;

/// VFP register state of one core.
#[derive(Clone, Debug)]
pub struct Vfp {
    /// S0-S31; Dn is the pair (S(2n) low, S(2n+1) high).
    s: [u32; 32],
    fpscr: u32,
    fpexc: u32,
    fpinst: u32,
    /// FPINST2 has an architecturally UNPREDICTABLE reset value. The emulator
    /// uses zero as its deterministic initial value until software writes it.
    fpinst2: u32,
}

impl Default for Vfp {
    fn default() -> Self {
        Self {
            s: [0; 32],
            fpscr: 0,
            fpexc: 0,
            fpinst: FPINST_RESET,
            fpinst2: 0,
        }
    }
}

impl Vfp {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn s(&self, index: usize) -> u32 {
        self.s[index]
    }

    pub fn set_s(&mut self, index: usize, value: u32) {
        self.s[index] = value;
    }

    pub fn d(&self, index: usize) -> u64 {
        u64::from(self.s[2 * index]) | (u64::from(self.s[2 * index + 1]) << 32)
    }

    pub fn set_d(&mut self, index: usize, value: u64) {
        self.s[2 * index] = value as u32;
        self.s[2 * index + 1] = (value >> 32) as u32;
    }

    pub fn fpscr(&self) -> u32 {
        self.fpscr
    }

    pub fn fpexc(&self) -> u32 {
        self.fpexc
    }

    /// Writes the architecturally defined FPEXC fields; SBZ bits remain zero.
    pub fn set_fpexc(&mut self, value: u32) {
        self.fpexc = value & FPEXC_WRITABLE;
    }

    pub fn fpinst(&self) -> u32 {
        self.fpinst
    }

    pub fn set_fpinst(&mut self, value: u32) {
        self.fpinst = value;
    }

    pub fn fpinst2(&self) -> u32 {
        self.fpinst2
    }

    pub fn set_fpinst2(&mut self, value: u32) {
        self.fpinst2 = value;
    }

    /// FPEXC.EN: when clear, VFP data instructions are undefined.
    pub fn enabled(&self) -> bool {
        self.fpexc & FPEXC_EN != 0
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled {
            self.fpexc |= FPEXC_EN;
        } else {
            self.fpexc &= !FPEXC_EN;
        }
    }

    fn set_flags_nzcv(&mut self, nzcv: u32) {
        self.fpscr = (self.fpscr & 0x0FFF_FFFF) | (nzcv << 28);
    }

    fn raise(&mut self, flags: u32) {
        self.fpscr |= flags;
    }

    fn has_enabled_traps(&self) -> bool {
        let enabled = self.fpscr & FPSCR_SUPPORTED_TRAP_ENABLES;
        enabled & (FPSCR_IOE | FPSCR_DZE | FPSCR_OFE) != 0
            || (enabled & FPSCR_UFE != 0 && self.fpscr & FPSCR_FZ == 0)
    }

    fn clear_exception_status(&mut self) {
        self.fpscr &= !FPSCR_EXCEPTION_STATUS;
    }

    fn exception_status(&self) -> u32 {
        self.fpscr & FPSCR_EXCEPTION_STATUS
    }

    fn trapped_exception_status(&self, raised: u32) -> u32 {
        let mut enabled = 0;
        if self.fpscr & FPSCR_IOE != 0 {
            enabled |= FPSCR_IOC;
        }
        if self.fpscr & FPSCR_DZE != 0 {
            enabled |= FPSCR_DZC;
        }
        if self.fpscr & FPSCR_OFE != 0 {
            enabled |= FPSCR_OFC;
        }
        // VFP11 ignores UFE in flush-to-zero mode: tiny results are
        // flushed and UFC is recorded, but no exception bounce is taken.
        if self.fpscr & FPSCR_UFE != 0 && self.fpscr & FPSCR_FZ == 0 {
            enabled |= FPSCR_UFC;
        }
        if self.fpscr & FPSCR_IXE != 0 {
            enabled |= FPSCR_IXC;
        }
        raised & enabled
    }

    fn merge_exception_status(&mut self, previous_fpscr: u32, raised: u32) {
        self.fpscr = (self.fpscr & !FPSCR_EXCEPTION_STATUS)
            | (previous_fpscr & FPSCR_EXCEPTION_STATUS)
            | raised;
    }

    fn record_exception_bounce(&mut self, instruction: u32, status: u32) {
        // VFP11 forces the bounced instruction condition to AL.
        self.fpinst = (instruction & 0x0FFF_FFFF) | 0xE000_0000;
        self.fpexc &= !(FPEXC_EXCEPTION_STATUS | FPEXC_FP2V | FPEXC_VECITR);
        self.fpexc |= FPEXC_EX;
        if status & FPSCR_IOC != 0 {
            self.fpexc |= FPEXC_IOC;
        }
        if status & FPSCR_OFC != 0 {
            self.fpexc |= FPEXC_OFC;
        }
        if status & FPSCR_UFC != 0 {
            self.fpexc |= FPEXC_UFC;
        }
    }

    fn rounding_mode(&self) -> u32 {
        (self.fpscr >> 22) & 3
    }

    pub(super) fn vector_len_field(&self) -> usize {
        ((self.fpscr >> 16) & 7) as usize
    }

    pub(super) fn vector_stride_field(&self) -> u32 {
        (self.fpscr >> 20) & 3
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prec {
    Single,
    Double,
}

impl Prec {
    fn exp_mask(self) -> u64 {
        match self {
            Self::Single => 0x7F80_0000,
            Self::Double => 0x7FF0_0000_0000_0000,
        }
    }

    fn frac_mask(self) -> u64 {
        match self {
            Self::Single => 0x007F_FFFF,
            Self::Double => 0x000F_FFFF_FFFF_FFFF,
        }
    }

    fn sign_bit(self) -> u64 {
        match self {
            Self::Single => 1 << 31,
            Self::Double => 1 << 63,
        }
    }

    fn quiet_bit(self) -> u64 {
        match self {
            Self::Single => 1 << 22,
            Self::Double => 1 << 51,
        }
    }

    fn is_nan(self, b: u64) -> bool {
        b & self.exp_mask() == self.exp_mask() && b & self.frac_mask() != 0
    }

    fn is_snan(self, b: u64) -> bool {
        self.is_nan(b) && b & self.quiet_bit() == 0
    }

    fn is_subnormal(self, b: u64) -> bool {
        b & self.exp_mask() == 0 && b & self.frac_mask() != 0
    }

    fn is_inf(self, b: u64) -> bool {
        b & self.exp_mask() == self.exp_mask() && b & self.frac_mask() == 0
    }

    fn default_nan(self) -> u64 {
        self.exp_mask() | self.quiet_bit()
    }

    fn decode_f64(self, b: u64) -> f64 {
        match self {
            Self::Single => f64::from(f32::from_bits(b as u32)),
            Self::Double => f64::from_bits(b),
        }
    }

    /// Rounds a computed value to this precision (round-to-nearest-even).
    fn encode_f64(self, x: f64) -> u64 {
        match self {
            Self::Single => u64::from((x as f32).to_bits()),
            Self::Double => x.to_bits(),
        }
    }

    fn max_finite(self) -> f64 {
        match self {
            Self::Single => f64::from(f32::MAX),
            Self::Double => f64::MAX,
        }
    }

    fn min_normal(self) -> f64 {
        match self {
            Self::Single => f64::from(f32::MIN_POSITIVE),
            Self::Double => f64::MIN_POSITIVE,
        }
    }

    fn max_finite_bits(self, negative: bool) -> u64 {
        match self {
            Self::Single => u64::from(0x7F7_FFFFu32 | if negative { 0x8000_0000 } else { 0 }),
            Self::Double => 0x7FEF_FFFF_FFFF_FFFF | if negative { 1 << 63 } else { 0 },
        }
    }

    fn infinity_bits(self, negative: bool) -> u64 {
        self.exp_mask() | if negative { self.sign_bit() } else { 0 }
    }

    fn next_up(self, bits: u64) -> u64 {
        match self {
            Self::Single => {
                let value = f32::from_bits(bits as u32);
                if value.is_nan() || value == f32::INFINITY {
                    bits
                } else if value == 0.0 {
                    1
                } else if value.is_sign_negative() {
                    bits - 1
                } else {
                    bits + 1
                }
            }
            Self::Double => {
                let value = f64::from_bits(bits);
                if value.is_nan() || value == f64::INFINITY {
                    bits
                } else if value == 0.0 {
                    1
                } else if value.is_sign_negative() {
                    bits - 1
                } else {
                    bits + 1
                }
            }
        }
    }

    fn next_down(self, bits: u64) -> u64 {
        match self {
            Self::Single => {
                let value = f32::from_bits(bits as u32);
                if value.is_nan() || value == f32::NEG_INFINITY {
                    bits
                } else if value == 0.0 {
                    u64::from(0x8000_0001u32)
                } else if value.is_sign_negative() {
                    bits + 1
                } else {
                    bits - 1
                }
            }
            Self::Double => {
                let value = f64::from_bits(bits);
                if value.is_nan() || value == f64::NEG_INFINITY {
                    bits
                } else if value == 0.0 {
                    (1 << 63) | 1
                } else if value.is_sign_negative() {
                    bits + 1
                } else {
                    bits - 1
                }
            }
        }
    }

    fn overflow_result(self, negative: bool, mode: u32) -> u64 {
        let infinity = self.infinity_bits(negative);
        let maximum = self.max_finite_bits(negative);
        match mode {
            0 => infinity,              // nearest
            1 if !negative => infinity, // toward +infinity
            2 if negative => infinity,  // toward -infinity
            _ => maximum,               // toward zero or finite direction
        }
    }
}

/// Where an operand register lives and how it is addressed.
fn single_index(high4: u32, low: bool) -> usize {
    ((high4 << 1) | u32::from(low)) as usize
}

impl Vfp {
    fn read(&self, prec: Prec, index: usize) -> u64 {
        match prec {
            Prec::Single => u64::from(self.s[index]),
            Prec::Double => self.d(index),
        }
    }

    fn write(&mut self, prec: Prec, index: usize, value: u64) {
        match prec {
            Prec::Single => self.s[index] = value as u32,
            Prec::Double => self.set_d(index, value),
        }
    }

    /// Applies input flush-to-zero (FPSCR.FZ) and records IDC.
    fn input(&mut self, prec: Prec, bits: u64) -> u64 {
        if self.fpscr & FPSCR_FZ != 0 && prec.is_subnormal(bits) {
            self.raise(FPSCR_IDC);
            bits & prec.sign_bit()
        } else {
            bits
        }
    }

    /// NaN handling common to all arithmetic: returns the result if any input is NaN.
    fn nan_result(&mut self, prec: Prec, operands: &[u64]) -> Option<u64> {
        let nan = operands.iter().copied().find(|b| prec.is_nan(*b))?;
        if operands.iter().any(|b| prec.is_snan(*b)) {
            self.raise(FPSCR_IOC);
        }
        if self.fpscr & FPSCR_DN != 0 {
            return Some(prec.default_nan());
        }
        // A signaling NaN takes priority over a quiet one; first operand wins.
        let chosen = operands
            .iter()
            .copied()
            .find(|b| prec.is_snan(*b))
            .unwrap_or(nan);
        Some(chosen | prec.quiet_bit())
    }

    /// Converts an unrounded host result to VFP precision and applies FPSCR.RMode.
    /// `exact_relation` is the sign of (exact operation result - host nearest result).
    fn finish(&mut self, prec: Prec, finite_inputs: bool, value: f64, exact_relation: i8) -> u64 {
        if value.is_nan() {
            self.raise(FPSCR_IOC);
            return prec.default_nan();
        }

        let mode = self.rounding_mode();
        let nearest_bits = prec.encode_f64(value);
        let nearest_value = prec.decode_f64(nearest_bits);
        // When narrowing to single precision, the gap from the f64 host result
        // to the f32 candidate dominates the much smaller f64 operation residual.
        let relation = if value > nearest_value {
            1
        } else if value < nearest_value {
            -1
        } else {
            exact_relation
        };

        let magnitude = value.abs();
        let maximum = prec.max_finite();
        let exact_magnitude_overflow = magnitude > maximum
            || (magnitude == maximum
                && if value.is_sign_negative() {
                    relation < 0
                } else {
                    relation > 0
                });
        let rounded_overflow = prec.is_inf(nearest_bits);
        let overflow = finite_inputs
            && if mode == 0 {
                rounded_overflow
            } else {
                exact_magnitude_overflow
            };

        if overflow {
            self.raise(FPSCR_OFC | FPSCR_IXC);
            return prec.overflow_result(value.is_sign_negative(), mode);
        }

        let mut result = nearest_bits;
        match mode {
            1 if relation > 0 => result = prec.next_up(result),
            2 if relation < 0 => result = prec.next_down(result),
            3 if ((!value.is_sign_negative() && relation < 0)
                || (value.is_sign_negative() && relation > 0)) =>
            {
                result = if value.is_sign_negative() {
                    prec.next_up(result)
                } else {
                    prec.next_down(result)
                };
            }
            _ => {}
        }

        let rounded = prec.decode_f64(result);
        let inexact = relation != 0;
        if inexact {
            self.raise(FPSCR_IXC);
        }

        // In full-compliance VFP11 mode, underflow is confirmed after
        // rounding and does not require an inexact result.
        let tiny_after_rounding = value != 0.0 && rounded.abs() < prec.min_normal();
        if tiny_after_rounding {
            self.raise(FPSCR_UFC);
        }

        // Flush-to-zero detects underflow before rounding. The VFP11 returns
        // positive zero and ignores UFE (no asynchronous bounce) in this mode.
        let tiny_before_rounding =
            value.is_finite() && value != 0.0 && magnitude < prec.min_normal();
        if self.fpscr & FPSCR_FZ != 0 && tiny_before_rounding {
            self.raise(FPSCR_UFC);
            return 0;
        }
        result
    }

    /// Returns the sign of the exact residual for a host operation rounded to f64.
    fn operation_residual(op: BinOp, a: f64, b: f64, rounded: f64) -> i8 {
        if !a.is_finite() || !b.is_finite() || !rounded.is_finite() {
            return 0;
        }

        let residual = match op {
            BinOp::Add | BinOp::Sub => {
                let rhs = if op == BinOp::Add { b } else { -b };
                // Error-free TwoSum transformation.
                let virtual_b = rounded - a;
                let virtual_a = rounded - virtual_b;
                (a - virtual_a) + (rhs - virtual_b)
            }
            BinOp::Mul => a.mul_add(b, -rounded),
            BinOp::Div if b != 0.0 => {
                let remainder = (-rounded).mul_add(b, a);
                return if b.is_sign_negative() {
                    -sign_of(remainder)
                } else {
                    sign_of(remainder)
                };
            }
            BinOp::Div => return 0,
        };
        sign_of(residual)
    }

    fn sqrt_residual(value: f64, rounded: f64) -> i8 {
        if !value.is_finite() || !rounded.is_finite() {
            return 0;
        }
        // A positive (rounded^2 - value) means the rounded root is too large.
        -sign_of(rounded.mul_add(rounded, -value))
    }

    fn finite(prec: Prec, operands: &[u64]) -> bool {
        operands
            .iter()
            .all(|b| b & prec.exp_mask() != prec.exp_mask())
    }

    fn binary(&mut self, prec: Prec, op: BinOp, a: u64, b: u64) -> Result<u64> {
        let (a, b) = (self.input(prec, a), self.input(prec, b));
        if let Some(nan) = self.nan_result(prec, &[a, b]) {
            return Ok(nan);
        }
        let (x, y) = (prec.decode_f64(a), prec.decode_f64(b));
        let finite = Self::finite(prec, &[a, b]);
        if op == BinOp::Div && finite && y == 0.0 && x != 0.0 {
            self.raise(FPSCR_DZC);
            let negative = (a ^ b) & prec.sign_bit() != 0;
            let inf = prec.exp_mask() | if negative { prec.sign_bit() } else { 0 };
            return Ok(inf);
        }
        let result = match op {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => x / y,
        };
        let residual = Self::operation_residual(op, x, y, result);
        Ok(self.finish(prec, finite, result, residual))
    }

    fn sqrt(&mut self, prec: Prec, a: u64) -> Result<u64> {
        let a = self.input(prec, a);
        if let Some(nan) = self.nan_result(prec, &[a]) {
            return Ok(nan);
        }
        let value = prec.decode_f64(a);
        let finite = Self::finite(prec, &[a]);
        let result = value.sqrt();
        let residual = Self::sqrt_residual(value, result);
        Ok(self.finish(prec, finite, result, residual))
    }

    fn compare(&mut self, prec: Prec, a: u64, b: u64, signal_quiet_nan: bool) {
        let (a, b) = (self.input(prec, a), self.input(prec, b));
        let nan = prec.is_nan(a) || prec.is_nan(b);
        if prec.is_snan(a) || prec.is_snan(b) || (nan && signal_quiet_nan) {
            self.raise(FPSCR_IOC);
        }
        let (x, y) = (prec.decode_f64(a), prec.decode_f64(b));
        let nzcv = if nan {
            0b0011
        } else if x == y {
            0b0110
        } else if x < y {
            0b1000
        } else {
            0b0010
        };
        self.set_flags_nzcv(nzcv);
    }

    fn float_to_float(&mut self, from: Prec, to: Prec, value: u64) -> u64 {
        let value = self.input(from, value);
        if from.is_nan(value) {
            if from.is_snan(value) {
                self.raise(FPSCR_IOC);
            }
            if self.fpscr & FPSCR_DN != 0 {
                return to.default_nan();
            }
            let converted = to.encode_f64(from.decode_f64(value));
            return converted | to.quiet_bit();
        }
        let finite = Self::finite(from, &[value]);
        self.finish(to, finite, from.decode_f64(value), 0)
    }

    fn float_to_int(&mut self, from: Prec, value: u64, signed: bool, round_to_zero: bool) -> u32 {
        let value = self.input(from, value);
        if from.is_nan(value) {
            self.raise(FPSCR_IOC);
            return 0;
        }
        let x = from.decode_f64(value);
        let rounded = if round_to_zero {
            x.trunc()
        } else {
            match self.rounding_mode() {
                0 => x.round_ties_even(),
                1 => x.ceil(),
                2 => x.floor(),
                _ => x.trunc(),
            }
        };
        let (min, max) = if signed {
            (-2147483648.0, 2147483647.0)
        } else {
            (0.0, 4294967295.0)
        };
        let result = if rounded > max {
            self.raise(FPSCR_IOC);
            max
        } else if rounded < min {
            self.raise(FPSCR_IOC);
            min
        } else {
            if rounded != x {
                self.raise(FPSCR_IXC);
            }
            rounded
        };
        if signed {
            (result as i32) as u32
        } else {
            result as u32
        }
    }

    fn int_to_float(&mut self, to: Prec, raw: u32, signed: bool) -> Result<u64> {
        let value = if signed {
            f64::from(raw as i32)
        } else {
            f64::from(raw)
        };
        Ok(self.finish(to, true, value, 0))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// Returns -1, 0, or 1 according to the sign of a finite residual.
fn sign_of(value: f64) -> i8 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// Negates a value without touching NaN payloads (sign-bit flip).
fn negate(prec: Prec, bits: u64) -> u64 {
    bits ^ prec.sign_bit()
}

impl Arm11Core {
    /// Grants the guest access to cp10/cp11 and sets FPEXC.EN, as an OS kernel
    /// does before running user code that uses the VFP.
    pub fn enable_vfp(&mut self) {
        self.cp15.grant_vfp_access();
        self.vfp.set_enabled(true);
    }

    /// VFP register state.
    pub fn vfp(&self) -> &Vfp {
        &self.vfp
    }

    /// Mutable VFP register state.
    pub fn vfp_mut(&mut self) -> &mut Vfp {
        &mut self.vfp
    }

    fn vfp_undefined(&self, word: u32) -> Error {
        Error::UndefinedInstruction {
            pc: self.pc(),
            word,
        }
    }

    /// Entry point for cp10/cp11 instructions.
    pub(super) fn execute_vfp<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
    ) -> Result<StepEvent> {
        if !self.cp15.vfp_access_enabled() {
            return Err(self.vfp_undefined(word));
        }

        let top = bits(word, 27, 24);
        let system_register =
            top == 0b1110 && bit(word, 4) && bits(word, 23, 21) == 0b111 && bits(word, 19, 16) == 8;
        // VFP11 permits the exception handler to inspect and clear FPEXC.EX.
        // Other VFP operations bounce through Undefined until support code
        // has acknowledged the pending exception.
        if self.vfp.fpexc & FPEXC_EX != 0 && !system_register {
            return Err(self.vfp_undefined(word));
        }

        let before = self.vfp.clone();
        let track_exceptions = top == 0b1110 && !bit(word, 4) && self.vfp.has_enabled_traps();
        if track_exceptions {
            self.vfp.clear_exception_status();
        }

        let prec = if bits(word, 11, 8) == 11 {
            Prec::Double
        } else {
            Prec::Single
        };
        let result = if bits(word, 27, 21) == 0b110_0010 {
            self.vfp_two_register_transfer(word, prec)
        } else if top == 0b1110 && bit(word, 4) {
            self.vfp_register_transfer(word, prec)
        } else if top == 0b1110 {
            self.vfp_data_processing(word, prec)
        } else {
            self.vfp_load_store(bus, word, prec)
        };
        if let Err(error) = result {
            self.vfp = before;
            return Err(error);
        }

        if track_exceptions {
            let raised = self.vfp.exception_status();
            let trapped = self.vfp.trapped_exception_status(raised);
            if trapped != 0 {
                self.vfp = before;
                self.vfp.record_exception_bounce(word, trapped);
            } else {
                self.vfp.merge_exception_status(before.fpscr, raised);
            }
        }
        Ok(StepEvent::Executed)
    }

    fn require_enabled(&self, word: u32) -> Result<()> {
        if self.vfp.enabled() {
            Ok(())
        } else {
            Err(self.vfp_undefined(word))
        }
    }

    /// Decodes Vd/Vn/Vm into a register index; the high-bank bit is rejected for D registers.
    fn vfp_reg(&self, word: u32, prec: Prec, high4: u32, low: bool) -> Result<usize> {
        match prec {
            Prec::Single => Ok(single_index(high4, low)),
            Prec::Double if low => Err(self.vfp_undefined(word)),
            Prec::Double => Ok(high4 as usize),
        }
    }

    fn vfp_two_register_transfer(&mut self, word: u32, prec: Prec) -> Result<()> {
        self.require_enabled(word)?;
        let (rt, rt2) = (bits(word, 15, 12), bits(word, 19, 16));
        let to_core = bit(word, 20);
        if rt == 15 || rt2 == 15 || (to_core && rt == rt2) || bits(word, 7, 6) != 0b00 {
            return Err(self.vfp_undefined(word));
        }
        let m = self.vfp_reg(word, prec, bits(word, 3, 0), bit(word, 5))?;
        let (lo, hi) = match prec {
            Prec::Single => {
                if m == 31 {
                    return Err(self.vfp_undefined(word));
                }
                (m, m + 1)
            }
            Prec::Double => (2 * m, 2 * m + 1),
        };
        if to_core {
            self.regs[rt as usize] = self.vfp.s[lo];
            self.regs[rt2 as usize] = self.vfp.s[hi];
        } else {
            self.vfp.s[lo] = self.regs[rt as usize];
            self.vfp.s[hi] = self.regs[rt2 as usize];
        }
        Ok(())
    }

    fn vfp_register_transfer(&mut self, word: u32, prec: Prec) -> Result<()> {
        let opc1 = bits(word, 23, 21);
        let to_core = bit(word, 20);
        let rt = bits(word, 15, 12);
        let fixed_zero = bits(word, 3, 0) == 0 && bits(word, 6, 5) == 0;
        match (prec, opc1) {
            (Prec::Single, 0b000) if fixed_zero && rt != 15 => {
                self.require_enabled(word)?;
                let n = single_index(bits(word, 19, 16), bit(word, 7));
                if to_core {
                    self.regs[rt as usize] = self.vfp.s[n];
                } else {
                    self.vfp.s[n] = self.regs[rt as usize];
                }
                Ok(())
            }
            (Prec::Single, 0b111) if fixed_zero && !bit(word, 7) => {
                self.vfp_system_register(word, to_core, rt)
            }
            (Prec::Double, 0b000 | 0b001) if fixed_zero && rt != 15 && !bit(word, 7) => {
                // VMOV between a core register and one half of a D register.
                self.require_enabled(word)?;
                let index = 2 * bits(word, 19, 16) as usize + usize::from(bit(word, 21));
                if to_core {
                    self.regs[rt as usize] = self.vfp.s[index];
                } else {
                    self.vfp.s[index] = self.regs[rt as usize];
                }
                Ok(())
            }
            _ => Err(self.vfp_undefined(word)),
        }
    }

    fn vfp_system_register(&mut self, word: u32, to_core: bool, rt: u32) -> Result<()> {
        let reg = bits(word, 19, 16);
        let privileged = self.cpsr.privileged();
        let enabled = self.vfp.enabled();
        if to_core {
            let value = match reg {
                0 if enabled || privileged => FPSID,
                1 => {
                    self.require_enabled(word)?;
                    self.vfp.fpscr
                }
                6 if enabled || privileged => MVFR1,
                7 if enabled || privileged => MVFR0,
                8 if privileged => self.vfp.fpexc(),
                9 if privileged => self.vfp.fpinst(),
                10 if privileged => self.vfp.fpinst2(),
                _ => return Err(self.vfp_undefined(word)),
            };
            if rt == 15 {
                if reg != 1 {
                    return Err(self.vfp_undefined(word));
                }
                self.cpsr.set_flags(
                    value & (1 << 31) != 0,
                    value & (1 << 30) != 0,
                    value & (1 << 29) != 0,
                    value & (1 << 28) != 0,
                );
            } else {
                self.regs[rt as usize] = value;
            }
            return Ok(());
        }
        if rt == 15 {
            return Err(self.vfp_undefined(word));
        }
        let value = self.regs[rt as usize];
        match reg {
            // The VFP11 exposes identification registers to User mode only
            // when FPEXC.EN is set. Writes are ignored because they are RO.
            0 | 6 | 7 if enabled || privileged => {}
            1 => {
                self.require_enabled(word)?;
                if value & FPSCR_UNSUPPORTED_TRAP_ENABLES != 0 {
                    return Err(Error::Unimplemented(
                        "VFP OFE/UFE/IDE/IXE exception support",
                    ));
                }
                self.vfp.fpscr = value & FPSCR_WRITABLE;
            }
            8 if privileged => self.vfp.set_fpexc(value),
            9 if privileged => self.vfp.set_fpinst(value),
            10 if privileged => self.vfp.set_fpinst2(value),
            _ => return Err(self.vfp_undefined(word)),
        }
        Ok(())
    }

    fn vfp_load_store<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        word: u32,
        prec: Prec,
    ) -> Result<()> {
        self.require_enabled(word)?;
        let (p, u, w, load) = (bit(word, 24), bit(word, 23), bit(word, 21), bit(word, 20));
        let rn = bits(word, 19, 16);
        let imm8 = bits(word, 7, 0);
        let first = self.vfp_reg(word, prec, bits(word, 15, 12), bit(word, 22))?;
        let words_per_reg = if prec == Prec::Single { 1 } else { 2 };
        let base = if rn == 15 {
            self.reg(rn, 8) & !3
        } else {
            self.regs[rn as usize]
        };
        // VFP double-precision odd imm8 encodings are the deprecated
        // FLDMX/FSTMX context-save forms. They transfer imm8 - 1 words,
        // while retaining the full imm8 * 4 address displacement.
        let offset = imm8 * 4;

        let (start, count, writeback) = match (p, u, w) {
            // VLDR / VSTR
            (true, _, false) => {
                let address = if u {
                    base.wrapping_add(offset)
                } else {
                    base.wrapping_sub(offset)
                };
                (address, 1, None)
            }
            // VLDM / VSTM increment-after
            (false, true, _) => (
                base,
                (imm8 / words_per_reg) as usize,
                w.then(|| base.wrapping_add(offset)),
            ),
            // VLDM / VSTM decrement-before (VPUSH / VPOP-adjacent forms)
            (true, false, true) => {
                let address = base.wrapping_sub(offset);
                (address, (imm8 / words_per_reg) as usize, Some(address))
            }
            _ => return Err(self.vfp_undefined(word)),
        };
        let registers_available = match prec {
            Prec::Single => 32,
            Prec::Double => 16,
        };
        if count == 0 || first + count > registers_available || (writeback.is_some() && rn == 15) {
            return Err(self.vfp_undefined(word));
        }

        let mut address = start;
        for reg in first..first + count {
            for half in 0..words_per_reg {
                let index = match prec {
                    Prec::Single => reg,
                    Prec::Double => 2 * reg + half as usize,
                };
                if load {
                    self.vfp.s[index] = self.read_data_u32(bus, address)?;
                } else {
                    self.write_data_u32(bus, address, self.vfp.s[index])?;
                }
                address = address.wrapping_add(4);
            }
        }
        if let Some(value) = writeback {
            self.regs[rn as usize] = value;
        }
        Ok(())
    }

    /// Resolves VFPv2's circular register-bank semantics for one CDP.
    ///
    /// A destination in bank 0 selects a scalar operation. Otherwise the
    /// destination is a vector; Fm in bank 0 is broadcast as a scalar, while
    /// Fn (for three-register instructions) is always a vector operand.
    fn vfp_vector_register_lanes(
        &self,
        word: u32,
        prec: Prec,
        destination: usize,
        sources: &[(usize, bool)],
    ) -> Result<Vec<(usize, Vec<usize>)>> {
        let bank_size = match prec {
            Prec::Single => 8,
            Prec::Double => 4,
        };
        let len_field = self.vfp.vector_len_field();
        let stride_field = self.vfp.vector_stride_field();

        // Only STRIDE encodings 00 (one register) and 11 (two registers)
        // are defined. Validate the FPSCR configuration before scalar/vector
        // destination selection: an invalid encoding remains unpredictable
        // even when Fd happens to reside in bank zero.
        if !matches!(stride_field, 0b00 | 0b11) {
            return Err(self.vfp_undefined(word));
        }

        if len_field == 0 {
            if stride_field != 0 {
                return Err(self.vfp_undefined(word));
            }
            return Ok(vec![(
                destination,
                sources.iter().map(|(source, _)| *source).collect(),
            )]);
        }

        let stride = if stride_field == 0b00 { 1 } else { 2 };

        // Any destination in bank 0 forces a scalar operation, even when
        // FPSCR.LEN is nonzero. Other destinations select short-vector mode.
        if destination < bank_size {
            return Ok(vec![(
                destination,
                sources.iter().map(|(source, _)| *source).collect(),
            )]);
        }
        let length = len_field + 1;
        // A vector may wrap within its bank, but it must never reuse a
        // register. Invalid LEN/STRIDE/precision combinations are UNPREDICTABLE.
        if (length - 1) * stride >= bank_size {
            return Err(self.vfp_undefined(word));
        }

        let bank_base = |register: usize| (register / bank_size) * bank_size;
        let bank_offset = |register: usize, iteration: usize| {
            bank_base(register) + (register % bank_size + iteration * stride) % bank_size
        };

        let mut lanes = Vec::with_capacity(length);
        for iteration in 0..length {
            let destination_register = bank_offset(destination, iteration);
            let source_registers = sources
                .iter()
                .map(|(source, scalar_if_bank_zero)| {
                    if *scalar_if_bank_zero && *source < bank_size {
                        *source
                    } else {
                        bank_offset(*source, iteration)
                    }
                })
                .collect();
            lanes.push((destination_register, source_registers));
        }
        Ok(lanes)
    }

    fn vfp_data_processing(&mut self, word: u32, prec: Prec) -> Result<()> {
        self.require_enabled(word)?;
        let (p, q, r, s) = (bit(word, 23), bit(word, 21), bit(word, 20), bit(word, 6));
        let d = self.vfp_reg(word, prec, bits(word, 15, 12), bit(word, 22));
        let n = self.vfp_reg(word, prec, bits(word, 19, 16), bit(word, 7));
        let m = self.vfp_reg(word, prec, bits(word, 3, 0), bit(word, 5));

        if p && q && r {
            return self.vfp_extension(word, prec, s);
        }
        let (d, n, m) = (d?, n?, m?);
        let lanes = self.vfp_vector_register_lanes(word, prec, d, &[(n, false), (m, true)])?;
        if lanes.len() > 1 && self.vfp.has_enabled_traps() {
            return Err(Error::Unimplemented("VFP short-vector exception bounce"));
        }

        // Snapshot every source and accumulator before committing any lane.
        // This preserves vector behavior when destination and source banks overlap.
        let inputs: Vec<_> = lanes
            .into_iter()
            .map(|(destination, sources)| {
                (
                    destination,
                    self.vfp.read(prec, destination),
                    self.vfp.read(prec, sources[0]),
                    self.vfp.read(prec, sources[1]),
                )
            })
            .collect();

        let mut results = Vec::with_capacity(inputs.len());
        for (destination, fd, fn_, fm) in inputs {
            let result = match (p, q, r) {
                (false, false, false) => {
                    let product = self.vfp.binary(prec, BinOp::Mul, fn_, fm)?;
                    let product = if s { negate(prec, product) } else { product };
                    self.vfp.binary(prec, BinOp::Add, fd, product)?
                }
                (false, false, true) => {
                    let product = self.vfp.binary(prec, BinOp::Mul, fn_, fm)?;
                    let product = if s { negate(prec, product) } else { product };
                    self.vfp
                        .binary(prec, BinOp::Add, negate(prec, fd), product)?
                }
                (false, true, false) => {
                    let product = self.vfp.binary(prec, BinOp::Mul, fn_, fm)?;
                    if s { negate(prec, product) } else { product }
                }
                (false, true, true) => {
                    let op = if s { BinOp::Sub } else { BinOp::Add };
                    self.vfp.binary(prec, op, fn_, fm)?
                }
                (true, false, false) if !s => self.vfp.binary(prec, BinOp::Div, fn_, fm)?,
                _ => return Err(self.vfp_undefined(word)),
            };
            results.push((destination, result));
        }

        for (destination, result) in results {
            self.vfp.write(prec, destination, result);
        }
        Ok(())
    }

    /// The "extension" opcode space: moves, sign changes, sqrt, compare and convert.
    fn vfp_extension(&mut self, word: u32, prec: Prec, s: bool) -> Result<()> {
        if !s {
            // VFPv3 immediate moves are not part of VFPv2.
            return Err(self.vfp_undefined(word));
        }
        let op = bits(word, 19, 16);
        let flag = bit(word, 7);
        let d_low = bit(word, 22);
        let m_low = bit(word, 5);
        let vd = bits(word, 15, 12);
        let vm = bits(word, 3, 0);

        match op {
            0b0000 | 0b0001 => {
                let d = self.vfp_reg(word, prec, vd, d_low)?;
                let m = self.vfp_reg(word, prec, vm, m_low)?;
                let lanes = self.vfp_vector_register_lanes(word, prec, d, &[(m, true)])?;
                if lanes.len() > 1 && self.vfp.has_enabled_traps() {
                    return Err(Error::Unimplemented("VFP short-vector exception bounce"));
                }
                let inputs: Vec<_> = lanes
                    .into_iter()
                    .map(|(destination, sources)| (destination, self.vfp.read(prec, sources[0])))
                    .collect();

                let mut results = Vec::with_capacity(inputs.len());
                for (destination, value) in inputs {
                    let result = match (op, flag) {
                        (0, false) => value,
                        (0, true) => value & !prec.sign_bit(),
                        (1, false) => negate(prec, value),
                        _ => self.vfp.sqrt(prec, value)?,
                    };
                    results.push((destination, result));
                }
                for (destination, result) in results {
                    self.vfp.write(prec, destination, result);
                }
            }
            0b0100 | 0b0101 => {
                let d = self.vfp_reg(word, prec, vd, d_low)?;
                let a = self.vfp.read(prec, d);
                let b = if op == 0b0101 {
                    if vm != 0 || m_low {
                        return Err(self.vfp_undefined(word));
                    }
                    0
                } else {
                    let m = self.vfp_reg(word, prec, vm, m_low)?;
                    self.vfp.read(prec, m)
                };
                self.vfp.compare(prec, a, b, flag);
            }
            0b0111 if flag => {
                // VCVT between single and double: cp10 widens, cp11 narrows.
                let (from, to) = match prec {
                    Prec::Single => (Prec::Single, Prec::Double),
                    Prec::Double => (Prec::Double, Prec::Single),
                };
                let (d, m) = match prec {
                    Prec::Single => (
                        self.vfp_reg(word, Prec::Double, vd, d_low)?,
                        self.vfp_reg(word, Prec::Single, vm, m_low)?,
                    ),
                    Prec::Double => (
                        self.vfp_reg(word, Prec::Single, vd, d_low)?,
                        self.vfp_reg(word, Prec::Double, vm, m_low)?,
                    ),
                };
                let value = self.vfp.read(from, m);
                let converted = self.vfp.float_to_float(from, to, value);
                self.vfp.write(to, d, converted);
            }
            0b1000 => {
                // Integer in Sm to float in Fd; bit 7 selects a signed source.
                let d = self.vfp_reg(word, prec, vd, d_low)?;
                let m = single_index(vm, m_low);
                let raw = self.vfp.s[m];
                let value = self.vfp.int_to_float(prec, raw, flag)?;
                self.vfp.write(prec, d, value);
            }
            0b1100 | 0b1101 => {
                // Float in Fm to integer in Sd; bit 7 selects round-toward-zero.
                let d = single_index(vd, d_low);
                let m = self.vfp_reg(word, prec, vm, m_low)?;
                let value = self.vfp.read(prec, m);
                let signed = op == 0b1101;
                self.vfp.s[d] = self.vfp.float_to_int(prec, value, signed, flag);
            }
            _ => return Err(self.vfp_undefined(word)),
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

    fn machine() -> (Arm11Core, FlatMemory) {
        let mut cpu = Arm11Core::new();
        cpu.enable_vfp();
        (cpu, FlatMemory::new(0, 0x1000))
    }

    fn run(cpu: &mut Arm11Core, mem: &mut FlatMemory, words: &[u32]) -> Result<()> {
        for (i, w) in words.iter().enumerate() {
            mem.write32(i as u32 * 4, *w).unwrap();
        }
        cpu.set_pc(0);
        for _ in words {
            cpu.step(mem)?;
        }
        Ok(())
    }

    /// Encodes a data-processing word for registers d, n, m (full indices).
    fn dp(base: u32, d: u32, n: u32, m: u32, double: bool) -> u32 {
        let (d_hi, d_lo) = if double { (d, 0) } else { (d >> 1, d & 1) };
        let (n_hi, n_lo) = if double { (n, 0) } else { (n >> 1, n & 1) };
        let (m_hi, m_lo) = if double { (m, 0) } else { (m >> 1, m & 1) };
        base | (if double { 0x100 } else { 0 })
            | (d_lo << 22)
            | (n_hi << 16)
            | (d_hi << 12)
            | (n_lo << 7)
            | (m_lo << 5)
            | m_hi
    }

    fn set_f32(cpu: &mut Arm11Core, i: usize, v: f32) {
        cpu.vfp_mut().set_s(i, v.to_bits());
    }

    fn f32_of(cpu: &Arm11Core, i: usize) -> f32 {
        f32::from_bits(cpu.vfp().s(i))
    }

    fn set_f64(cpu: &mut Arm11Core, i: usize, v: f64) {
        cpu.vfp_mut().set_d(i, v.to_bits());
    }

    fn f64_of(cpu: &Arm11Core, i: usize) -> f64 {
        f64::from_bits(cpu.vfp().d(i))
    }

    const VMLA: u32 = 0xEE00_0A00;
    const VMLS: u32 = 0xEE00_0A40;
    const VNMLA: u32 = 0xEE10_0A40;
    const VNMLS: u32 = 0xEE10_0A00;
    const VMUL: u32 = 0xEE20_0A00;
    const VNMUL: u32 = 0xEE20_0A40;
    const VADD: u32 = 0xEE30_0A00;
    const VSUB: u32 = 0xEE30_0A40;
    const VDIV: u32 = 0xEE80_0A00;
    const VMOV_REG: u32 = 0xEEB0_0A40;
    const VABS: u32 = 0xEEB0_0AC0;
    const VNEG: u32 = 0xEEB1_0A40;
    const VSQRT: u32 = 0xEEB1_0AC0;
    const VCMP: u32 = 0xEEB4_0A40;
    const VCMPE: u32 = 0xEEB4_0AC0;
    const VCMP_ZERO: u32 = 0xEEB5_0A40;
    const VCVT_F: u32 = 0xEEB7_0AC0;
    const VCVT_FROM_U32: u32 = 0xEEB8_0A40;
    const VCVT_FROM_S32: u32 = 0xEEB8_0AC0;
    const VCVT_TO_U32_Z: u32 = 0xEEBC_0AC0;
    const VCVT_TO_S32: u32 = 0xEEBD_0A40;
    const VCVT_TO_S32_Z: u32 = 0xEEBD_0AC0;

    #[test]
    fn single_precision_arithmetic() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, 1.5);
        set_f32(&mut cpu, 2, 2.25);
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 3.75);
        run(&mut cpu, &mut mem, &[dp(VSUB, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), -0.75);
        run(&mut cpu, &mut mem, &[dp(VMUL, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 3.375);
        run(&mut cpu, &mut mem, &[dp(VNMUL, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), -3.375);
        run(&mut cpu, &mut mem, &[dp(VDIV, 0, 2, 1, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 1.5);
    }

    #[test]
    fn multiply_accumulate_family_signs() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, 2.0);
        set_f32(&mut cpu, 2, 3.0);
        for (op, expected) in [
            (VMLA, 10.0 + 6.0),
            (VMLS, 10.0 - 6.0),
            (VNMLS, -10.0 + 6.0),
            (VNMLA, -10.0 - 6.0),
        ] {
            set_f32(&mut cpu, 0, 10.0);
            run(&mut cpu, &mut mem, &[dp(op, 0, 1, 2, false)]).unwrap();
            assert_eq!(f32_of(&cpu, 0), expected, "{op:#x}");
        }
    }

    #[test]
    fn double_precision_arithmetic_and_registers() {
        let (mut cpu, mut mem) = machine();
        set_f64(&mut cpu, 1, 1.0);
        set_f64(&mut cpu, 2, 3.0);
        run(&mut cpu, &mut mem, &[dp(VDIV, 0, 1, 2, true)]).unwrap();
        assert_eq!(f64_of(&cpu, 0), 1.0 / 3.0);
        // D0 aliases S0 (low word) and S1 (high word).
        let bits = (1.0f64 / 3.0).to_bits();
        assert_eq!(cpu.vfp().s(0), bits as u32);
        assert_eq!(cpu.vfp().s(1), (bits >> 32) as u32);
    }

    #[test]
    fn single_precision_rounds_like_f32() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, 16777216.0);
        set_f32(&mut cpu, 2, 1.0);
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 16777216.0);
    }

    #[test]
    fn sign_and_sqrt_operations() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, -4.0);
        run(&mut cpu, &mut mem, &[dp(VABS, 0, 0, 1, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 4.0);
        run(&mut cpu, &mut mem, &[dp(VNEG, 2, 0, 1, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 2), 4.0);
        run(&mut cpu, &mut mem, &[dp(VSQRT, 3, 0, 0, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 3), 2.0);
        run(&mut cpu, &mut mem, &[dp(VMOV_REG, 4, 0, 1, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 4), -4.0);
    }

    #[test]
    fn invalid_operations_and_divide_by_zero_set_flags() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, -1.0);
        run(&mut cpu, &mut mem, &[dp(VSQRT, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x7FC0_0000);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IOC, 0);

        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, 1.0);
        set_f32(&mut cpu, 2, 0.0);
        run(&mut cpu, &mut mem, &[dp(VDIV, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), f32::INFINITY);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_DZC, 0);

        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, f32::MAX);
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 1, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), f32::INFINITY);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_OFC, 0);
    }

    #[test]
    fn nan_propagation_prefers_signaling_operand_and_quiets_it() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().set_s(1, 0x7FC0_0001); // quiet NaN
        cpu.vfp_mut().set_s(2, 0x7F80_0002); // signaling NaN
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x7FC0_0002);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IOC, 0);

        // Default-NaN mode discards the payload.
        cpu.vfp_mut().fpscr |= FPSCR_DN;
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x7FC0_0000);
    }

    #[test]
    fn flush_to_zero_applies_to_inputs_and_outputs() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().fpscr |= FPSCR_FZ;
        cpu.vfp_mut().set_s(1, 0x0000_0001); // smallest subnormal
        set_f32(&mut cpu, 2, 1.0);
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 1.0);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IDC, 0);

        set_f32(&mut cpu, 1, f32::MIN_POSITIVE);
        set_f32(&mut cpu, 2, 0.5);
        run(&mut cpu, &mut mem, &[dp(VMUL, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_UFC, 0);
    }

    #[test]
    fn compare_sets_fpscr_flags_and_vmrs_copies_them_to_cpsr() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 0, 1.0);
        set_f32(&mut cpu, 1, 2.0);
        run(&mut cpu, &mut mem, &[dp(VCMP, 0, 0, 1, false), 0xEEF1_FA10]).unwrap();
        // less than: N set, others clear
        assert_eq!(cpu.vfp().fpscr() >> 28, 0b1000);
        assert!(cpu.cpsr.0 & (1 << 31) != 0);
        assert!(cpu.cpsr.0 & (1 << 30) == 0);

        run(&mut cpu, &mut mem, &[dp(VCMP, 1, 1, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().fpscr() >> 28, 0b0110);
        run(&mut cpu, &mut mem, &[dp(VCMP, 1, 0, 0, false)]).unwrap();
        assert_eq!(cpu.vfp().fpscr() >> 28, 0b0010);
    }

    #[test]
    fn compare_with_zero_and_nan_behaviour() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 0, 0.0);
        run(&mut cpu, &mut mem, &[dp(VCMP_ZERO, 0, 0, 0, false)]).unwrap();
        assert_eq!(cpu.vfp().fpscr() >> 28, 0b0110);

        cpu.vfp_mut().set_s(0, 0x7FC0_0000);
        cpu.vfp_mut().fpscr = 0;
        run(&mut cpu, &mut mem, &[dp(VCMP, 0, 0, 0, false)]).unwrap();
        assert_eq!(cpu.vfp().fpscr() >> 28, 0b0011);
        assert_eq!(cpu.vfp().fpscr() & FPSCR_IOC, 0);
        run(&mut cpu, &mut mem, &[dp(VCMPE, 0, 0, 0, false)]).unwrap();
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IOC, 0);
    }

    #[test]
    fn conversions_between_float_integer_and_double() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().set_s(1, (-7i32) as u32);
        run(&mut cpu, &mut mem, &[dp(VCVT_FROM_S32, 0, 0, 1, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), -7.0);
        cpu.vfp_mut().set_s(2, 0xFFFF_FFFF);
        run(&mut cpu, &mut mem, &[dp(VCVT_FROM_U32, 0, 0, 1, true)]).unwrap();
        assert_eq!(f64_of(&cpu, 0), 4294967295.0);

        set_f32(&mut cpu, 2, 2.5);
        run(&mut cpu, &mut mem, &[dp(VCVT_TO_S32, 3, 0, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(3), 2, "ties to even under RN");
        set_f32(&mut cpu, 2, -2.9);
        run(&mut cpu, &mut mem, &[dp(VCVT_TO_S32_Z, 3, 0, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(3) as i32, -2);

        set_f32(&mut cpu, 4, 1.25);
        run(&mut cpu, &mut mem, &[dp(VCVT_F, 4, 0, 4, false)]).unwrap();
        assert_eq!(f64_of(&cpu, 2), 1.25);
        set_f64(&mut cpu, 5, 0.1);
        run(&mut cpu, &mut mem, &[dp(VCVT_F, 6, 0, 5, true)]).unwrap();
        assert_eq!(f32_of(&cpu, 12), 0.1f64 as f32);
    }

    #[test]
    fn float_to_integer_saturates_and_flags_invalid() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, 3.0e9);
        run(&mut cpu, &mut mem, &[dp(VCVT_TO_S32_Z, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), i32::MAX as u32);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IOC, 0);
        set_f32(&mut cpu, 1, -1.0);
        run(&mut cpu, &mut mem, &[dp(VCVT_TO_U32_Z, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0);
        cpu.vfp_mut().set_s(1, 0x7FC0_0000);
        run(&mut cpu, &mut mem, &[dp(VCVT_TO_S32_Z, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0);
    }

    #[test]
    fn fpscr_rounding_mode_controls_single_arithmetic_and_exception_flags() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, 2.5);
        cpu.vfp_mut().fpscr = 1 << 22; // toward +infinity for float-to-int
        run(&mut cpu, &mut mem, &[dp(VCVT_TO_S32, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 3);

        set_f32(&mut cpu, 1, 1.0);
        set_f32(&mut cpu, 2, 2.0_f32.powi(-24)); // halfway between 1.0f and next f32
        cpu.vfp_mut().fpscr = 1 << 22;
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x3F80_0001);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IXC, 0);

        cpu.vfp_mut().fpscr = 2 << 22; // toward -infinity
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x3F80_0000);

        set_f32(&mut cpu, 1, -1.0);
        set_f32(&mut cpu, 2, -2.0_f32.powi(-24));
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0xBF80_0001);
    }

    #[test]
    fn directed_rounding_uses_residuals_for_multiply_divide_and_sqrt() {
        let (mut cpu, mut mem) = machine();

        let adjacent = f32::from_bits(0x3F80_0001);
        set_f32(&mut cpu, 1, adjacent);
        set_f32(&mut cpu, 2, adjacent);
        cpu.vfp_mut().fpscr = 1 << 22; // toward +infinity
        run(&mut cpu, &mut mem, &[dp(VMUL, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x3F80_0003);

        set_f32(&mut cpu, 1, 1.0);
        set_f32(&mut cpu, 2, 10.0);
        cpu.vfp_mut().fpscr = 2 << 22; // toward -infinity
        run(&mut cpu, &mut mem, &[dp(VDIV, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x3DCC_CCCC);

        set_f32(&mut cpu, 1, 2.0);
        cpu.vfp_mut().fpscr = 1 << 22; // sqrt(2) rounds upward
        run(&mut cpu, &mut mem, &[dp(VSQRT, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0x3FB5_04F4);
    }

    #[test]
    fn inexact_underflow_and_flush_to_zero_update_fpscr_flags() {
        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, f32::from_bits(1));
        set_f32(&mut cpu, 2, 0.5);
        run(&mut cpu, &mut mem, &[dp(VMUL, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IXC, 0);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_UFC, 0);

        let (mut cpu, mut mem) = machine();
        set_f32(&mut cpu, 1, f32::from_bits(0x0080_0000)); // minimum normal
        set_f32(&mut cpu, 2, 0.5); // exact subnormal result
        cpu.vfp_mut().fpscr = FPSCR_FZ;
        run(&mut cpu, &mut mem, &[dp(VMUL, 0, 1, 2, false)]).unwrap();
        assert_eq!(cpu.vfp().s(0), 0);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IXC, 0);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_UFC, 0);
    }

    #[test]
    fn fpscr_rounding_mode_controls_double_arithmetic() {
        let (mut cpu, mut mem) = machine();
        set_f64(&mut cpu, 1, 1.0);
        set_f64(&mut cpu, 2, 2.0_f64.powi(-53));
        cpu.vfp_mut().fpscr = 1 << 22; // toward +infinity
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, true)]).unwrap();
        assert_eq!(cpu.vfp().d(0), 0x3FF0_0000_0000_0001);
        assert_ne!(cpu.vfp().fpscr() & FPSCR_IXC, 0);
    }

    #[test]
    fn single_short_vector_add_wraps_register_banks_and_broadcasts_scalar_fm() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().fpscr = 3 << 16; // four elements, stride one

        // Fd starts at S14, Fn at S22. Both vectors wrap within their
        // respective eight-register banks; Fm=S2 is broadcast as a scalar.
        set_f32(&mut cpu, 22, 1.5);
        set_f32(&mut cpu, 23, 2.5);
        set_f32(&mut cpu, 16, 3.5);
        set_f32(&mut cpu, 17, 4.5);
        set_f32(&mut cpu, 2, 10.0);
        set_f32(&mut cpu, 10, -99.0);

        run(&mut cpu, &mut mem, &[dp(VADD, 14, 22, 2, false)]).unwrap();

        assert_eq!(f32_of(&cpu, 14), 11.5);
        assert_eq!(f32_of(&cpu, 15), 12.5);
        assert_eq!(f32_of(&cpu, 8), 13.5);
        assert_eq!(f32_of(&cpu, 9), 14.5);
        assert_eq!(f32_of(&cpu, 10), -99.0);
    }

    #[test]
    fn single_short_vector_add_uses_bank_zero_fn_as_a_vector() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().fpscr = 3 << 16;
        for (index, value) in [(0, 1.0), (1, 2.0), (2, 3.0), (3, 4.0)] {
            set_f32(&mut cpu, index, value);
        }
        for (index, value) in [(8, 10.0), (9, 20.0), (10, 30.0), (11, 40.0)] {
            set_f32(&mut cpu, index, value);
        }

        // With Fd outside bank 0, Fn is always a vector, even when it starts
        // in bank 0. Fm in bank 1 is a second vector.
        run(&mut cpu, &mut mem, &[dp(VADD, 16, 0, 8, false)]).unwrap();
        assert_eq!(
            [16, 17, 18, 19].map(|index| f32_of(&cpu, index)),
            [11.0, 22.0, 33.0, 44.0]
        );
    }

    #[test]
    fn single_short_vector_add_honors_stride_two() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().fpscr = (3 << 16) | (3 << 20); // four elements, stride two
        for (index, value) in [(16, 1.0), (18, 2.0), (20, 3.0), (22, 4.0)] {
            set_f32(&mut cpu, index, value);
        }
        set_f32(&mut cpu, 1, 100.0);

        run(&mut cpu, &mut mem, &[dp(VADD, 8, 16, 1, false)]).unwrap();
        assert_eq!(
            [8, 10, 12, 14].map(|index| f32_of(&cpu, index)),
            [101.0, 102.0, 103.0, 104.0]
        );
    }

    #[test]
    fn double_short_vector_add_wraps_four_register_banks() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().fpscr = 3 << 16; // four doubles, stride one
        for (index, value) in [(10, 1.0), (11, 2.0), (8, 3.0), (9, 4.0)] {
            set_f64(&mut cpu, index, value);
        }
        set_f64(&mut cpu, 2, 10.0);

        // D14..D15 wrap to D12..D13 in bank 3. Fm=D2 is scalar.
        run(&mut cpu, &mut mem, &[dp(VADD, 14, 10, 2, true)]).unwrap();
        assert_eq!(
            [14, 15, 12, 13].map(|index| f64_of(&cpu, index)),
            [11.0, 12.0, 13.0, 14.0]
        );
    }

    #[test]
    fn short_vector_sqrt_and_scalar_destination_selection() {
        let (mut cpu, mut mem) = machine();
        cpu.vfp_mut().fpscr = 1 << 16; // two vector elements
        set_f32(&mut cpu, 8, 4.0);
        set_f32(&mut cpu, 9, 9.0);
        run(&mut cpu, &mut mem, &[dp(VSQRT, 16, 0, 8, false)]).unwrap();
        assert_eq!([16, 17].map(|index| f32_of(&cpu, index)), [2.0, 3.0]);

        // Bank-0 destinations select scalar operations even when LEN is nonzero.
        set_f32(&mut cpu, 8, 7.0);
        set_f32(&mut cpu, 16, 100.0);
        set_f32(&mut cpu, 0, -1.0);
        run(&mut cpu, &mut mem, &[dp(VADD, 0, 8, 16, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 107.0);
        assert_eq!(f32_of(&cpu, 1), 0.0);
    }

    #[test]
    fn short_vector_rejects_len_stride_combinations_that_repeat_registers() {
        let (mut cpu, mut mem) = machine();
        // LEN=2 means three elements; stride two would repeat a D register.
        cpu.vfp_mut().fpscr = (2 << 16) | (3 << 20);
        assert!(matches!(
            run(&mut cpu, &mut mem, &[dp(VADD, 4, 8, 12, true)]),
            Err(Error::UndefinedInstruction { .. })
        ));

        // LEN=0 with STRIDE=11 is unpredictable even when Fd is in bank 0.
        cpu.vfp_mut().fpscr = 3 << 20;
        assert!(matches!(
            run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]),
            Err(Error::UndefinedInstruction { .. })
        ));

        // STRIDE=01 is reserved with LEN nonzero, even for scalar Fd.
        cpu.vfp_mut().fpscr = (1 << 16) | (1 << 20);
        assert!(matches!(
            run(&mut cpu, &mut mem, &[dp(VADD, 0, 1, 2, false)]),
            Err(Error::UndefinedInstruction { .. })
        ));
    }

    #[test]
    fn vfp_compare_and_convert_remain_scalar_with_short_vectors_enabled() {
        let (mut cpu, mut mem) = machine();
        // A valid vector configuration must not make VCMP or VCVT iterate.
        cpu.vfp_mut().fpscr = (3 << 16) | (3 << 20);
        set_f32(&mut cpu, 0, 3.0);
        set_f32(&mut cpu, 1, 2.0);
        run(&mut cpu, &mut mem, &[dp(VCMP, 0, 0, 1, false)]).unwrap();
        assert_eq!(cpu.vfp().fpscr() >> 28, 0b0010);

        cpu.vfp_mut().set_s(2, 42);
        run(&mut cpu, &mut mem, &[dp(VCVT_FROM_S32, 0, 0, 2, false)]).unwrap();
        assert_eq!(f32_of(&cpu, 0), 42.0);
        assert_eq!(cpu.vfp().s(8), 0);
    }

    #[test]
    fn core_register_transfers() {
        let (mut cpu, mut mem) = machine();
        cpu.regs[1] = 0x4048_F5C3;
        // vmov s5, r1 ; vmov r2, s5
        run(&mut cpu, &mut mem, &[0xEE02_1A90, 0xEE12_2A90]).unwrap();
        assert_eq!(cpu.vfp().s(5), 0x4048_F5C3);
        assert_eq!(cpu.regs[2], 0x4048_F5C3);

        // vmov d3[1], r1 ; vmov r4, d3[1]
        run(&mut cpu, &mut mem, &[0xEE23_1B10, 0xEE33_4B10]).unwrap();
        assert_eq!(cpu.vfp().s(7), 0x4048_F5C3);
        assert_eq!(cpu.regs[4], 0x4048_F5C3);

        // vmov s2, s3, r5, r6 ; vmov r7, r8, d1 reads the same pair
        cpu.regs[5] = 11;
        cpu.regs[6] = 22;
        run(&mut cpu, &mut mem, &[0xEC46_5A11, 0xEC58_7B11]).unwrap();
        assert_eq!((cpu.vfp().s(2), cpu.vfp().s(3)), (11, 22));
        assert_eq!((cpu.regs[7], cpu.regs[8]), (11, 22));
    }

    #[test]
    fn system_registers_and_access_rules() {
        let (mut cpu, mut mem) = machine();
        cpu.regs[1] = 0x0300_0000; // FZ | DN
        // vmsr fpscr, r1 ; vmrs r2, fpscr ; vmrs r3, fpsid
        run(&mut cpu, &mut mem, &[0xEEE1_1A10, 0xEEF1_2A10, 0xEEF0_3A10]).unwrap();
        assert_eq!(cpu.regs[2], 0x0300_0000);
        assert_eq!(cpu.regs[3], FPSID);

        // VFP11 feature IDs are readable in User mode when the coprocessor
        // is accessible. Writes to identification registers are ignored.
        cpu.regs[1] = 0xDEAD_BEEF;
        run(
            &mut cpu,
            &mut mem,
            &[
                0xEEF7_5A10, // vmrs r5, MVFR0
                0xEEF6_6A10, // vmrs r6, MVFR1
                0xEEE0_1A10, // vmsr FPSID, r1 (read-only)
                0xEEE7_1A10, // vmsr MVFR0, r1 (read-only)
                0xEEE6_1A10, // vmsr MVFR1, r1 (read-only)
            ],
        )
        .unwrap();
        assert_eq!(cpu.regs[5], MVFR0);
        assert_eq!(cpu.regs[6], MVFR1);

        // Scalar invalid-operation traps are recorded in FPSCR and enter
        // the VFP11 exception-bounce state when the instruction executes.
        cpu.regs[1] = 1 << 8;
        run(&mut cpu, &mut mem, &[0xEEE1_1A10]).unwrap();
        assert_eq!(cpu.vfp().fpscr() & FPSCR_IOE, FPSCR_IOE);

        // FPEXC is privileged: user mode gets an undefined instruction.
        assert!(matches!(
            run(&mut cpu, &mut mem, &[0xEEF8_4A10]),
            Err(Error::UndefinedInstruction { .. })
        ));
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        run(&mut cpu, &mut mem, &[0xEEF8_4A10]).unwrap();
        assert_eq!(cpu.regs[4], FPEXC_EN);
    }

    #[test]
    fn vfp_identification_register_access_obeys_fpexc_and_privilege() {
        let mut cpu = Arm11Core::new();
        let mut mem = FlatMemory::new(0, 0x1000);
        cpu.cp15_mut().grant_vfp_access();

        // With FPEXC.EN clear, FPSID and MVFR0/1 require a privileged mode.
        for instruction in [
            0xEEF0_3A10, // VMRS r3, FPSID
            0xEEF7_5A10, // VMRS r5, MVFR0
            0xEEF6_6A10, // VMRS r6, MVFR1
            0xEEE0_1A10, // VMSR FPSID, r1
            0xEEE7_1A10, // VMSR MVFR0, r1
            0xEEE6_1A10, // VMSR MVFR1, r1
        ] {
            assert!(
                matches!(
                    run(&mut cpu, &mut mem, &[instruction]),
                    Err(Error::UndefinedInstruction { .. })
                ),
                "User mode, FPEXC.EN=0: {instruction:#010x}"
            );
        }

        // Privileged mode may inspect identification registers while disabled.
        cpu.switch_mode(Psr::SUPERVISOR_MODE).unwrap();
        cpu.regs[1] = 0xDEAD_BEEF;
        run(
            &mut cpu,
            &mut mem,
            &[
                0xEEF0_3A10,
                0xEEF7_5A10,
                0xEEF6_6A10,
                0xEEE0_1A10,
                0xEEE7_1A10,
                0xEEE6_1A10,
                0xEEF0_7A10,
                0xEEF7_8A10,
                0xEEF6_9A10,
            ],
        )
        .unwrap();
        assert_eq!(cpu.regs[3], FPSID);
        assert_eq!(cpu.regs[5], MVFR0);
        assert_eq!(cpu.regs[6], MVFR1);
        assert_eq!(cpu.regs[7], FPSID);
        assert_eq!(cpu.regs[8], MVFR0);
        assert_eq!(cpu.regs[9], MVFR1);

        // FPSCR remains inaccessible while FPEXC.EN is clear, even in SVC.
        assert!(matches!(
            run(&mut cpu, &mut mem, &[0xEEF1_2A10]),
            Err(Error::UndefinedInstruction { .. })
        ));

        // Once enabled, the identification registers are accessible in User mode.
        cpu.switch_mode(Psr::USER_MODE).unwrap();
        cpu.vfp_mut().set_enabled(true);
        run(
            &mut cpu,
            &mut mem,
            &[
                0xEEF0_3A10,
                0xEEF7_5A10,
                0xEEF6_6A10,
                0xEEE0_1A10,
                0xEEE7_1A10,
                0xEEE6_1A10,
            ],
        )
        .unwrap();
        assert_eq!(cpu.regs[3], FPSID);
        assert_eq!(cpu.regs[5], MVFR0);
        assert_eq!(cpu.regs[6], MVFR1);
    }

    #[test]
    fn vfp_is_undefined_until_enabled_in_cpacr_and_fpexc() {
        let mut cpu = Arm11Core::new();
        let mut mem = FlatMemory::new(0, 0x1000);
        let add = dp(VADD, 0, 1, 2, false);
        assert!(matches!(
            run(&mut cpu, &mut mem, &[add]),
            Err(Error::UndefinedInstruction { .. })
        ));
        cpu.cp15_mut().grant_vfp_access();
        assert!(matches!(
            run(&mut cpu, &mut mem, &[add]),
            Err(Error::UndefinedInstruction { .. })
        ));
        cpu.vfp_mut().set_enabled(true);
        run(&mut cpu, &mut mem, &[add]).unwrap();
    }

    #[test]
    fn vldr_and_vstr_use_word_offsets() {
        let (mut cpu, mut mem) = machine();
        mem.write32(DATA + 8, 0x3FC0_0000).unwrap();
        cpu.regs[1] = DATA;
        // vldr s3, [r1, #8] ; vstr s3, [r1, #-4]
        run(&mut cpu, &mut mem, &[0xEDD1_1A02, 0xED41_1A01]).unwrap();
        assert_eq!(f32_of(&cpu, 3), 1.5);
        assert_eq!(mem.read32(DATA - 4).unwrap(), 0x3FC0_0000);

        // vldr d2, [r1] reads low word first.
        mem.write32(DATA, 0x1111_1111).unwrap();
        mem.write32(DATA + 4, 0x2222_2222).unwrap();
        run(&mut cpu, &mut mem, &[0xED91_2B00]).unwrap();
        assert_eq!(cpu.vfp().d(2), 0x2222_2222_1111_1111);
    }

    #[test]
    fn vpush_and_vpop_round_trip_with_writeback() {
        let (mut cpu, mut mem) = machine();
        cpu.regs[13] = DATA + 0x100;
        set_f64(&mut cpu, 8, 1.25);
        set_f64(&mut cpu, 9, -2.5);
        // vpush {d8-d9} ; vpop {d10-d11}
        run(&mut cpu, &mut mem, &[0xED2D_8B04, 0xECBD_AB04]).unwrap();
        assert_eq!(cpu.regs[13], DATA + 0x100);
        assert_eq!(f64_of(&cpu, 10), 1.25);
        assert_eq!(f64_of(&cpu, 11), -2.5);

        // vpush leaves SP lowered by the transferred size.
        run(&mut cpu, &mut mem, &[0xED2D_8B04]).unwrap();
        assert_eq!(cpu.regs[13], DATA + 0x100 - 16);
    }

    #[test]
    fn vstm_and_vldm_increment_after_and_reject_invalid_forms() {
        let (mut cpu, mut mem) = machine();
        for i in 0..4 {
            cpu.vfp_mut().set_s(i, 100 + i as u32);
        }
        cpu.regs[2] = DATA;
        // vstmia r2!, {s0-s3} ; vldmia r2, {s8-s11} after resetting r2
        run(&mut cpu, &mut mem, &[0xECA2_0A04]).unwrap();
        assert_eq!(cpu.regs[2], DATA + 16);
        cpu.regs[2] = DATA;
        run(&mut cpu, &mut mem, &[0xEC92_4A04]).unwrap();
        assert_eq!(cpu.vfp().s(8), 100);
        assert_eq!(cpu.vfp().s(11), 103);

        // FSTMX DB stores one D register using the legacy odd word count.
        // FLDMX IA reloads it and restores SP; the address displacement keeps
        // the extra format word even though only two data words are transferred.
        let stack_top = DATA + 0x100;
        cpu.regs[13] = stack_top;
        cpu.vfp_mut().set_s(4, 0x1122_3344);
        cpu.vfp_mut().set_s(5, 0x5566_7788);
        run(&mut cpu, &mut mem, &[0xED2D_2B03]).unwrap(); // FSTMX r13!, {d2}
        assert_eq!(cpu.regs[13], stack_top - 12);
        assert_eq!(mem.read32(stack_top - 12).unwrap(), 0x1122_3344);
        assert_eq!(mem.read32(stack_top - 8).unwrap(), 0x5566_7788);

        cpu.vfp_mut().set_s(4, 0);
        cpu.vfp_mut().set_s(5, 0);
        run(&mut cpu, &mut mem, &[0xECBD_2B03]).unwrap(); // FLDMX r13!, {d2}
        assert_eq!(cpu.regs[13], stack_top);
        assert_eq!(cpu.vfp().s(4), 0x1122_3344);
        assert_eq!(cpu.vfp().s(5), 0x5566_7788);

        // A zero-register transfer is unpredictable and rejected.
        assert!(matches!(
            run(&mut cpu, &mut mem, &[0xEC92_4A00]),
            Err(Error::UndefinedInstruction { .. })
        ));
    }

    #[test]
    fn d_register_high_bank_bit_is_undefined_on_vfpv2() {
        let (mut cpu, mut mem) = machine();
        // vadd.f64 with D bit set would address d16+.
        let word = dp(VADD, 0, 1, 2, true) | (1 << 22);
        assert!(matches!(
            run(&mut cpu, &mut mem, &[word]),
            Err(Error::UndefinedInstruction { .. })
        ));
    }
}
