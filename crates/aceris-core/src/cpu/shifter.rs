//! Barrel shifter and add/subtract helpers shared by ARM data processing.

/// The four ARM shift types (bits 6:5 of a shifted-register operand).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftKind {
    Lsl,
    Lsr,
    Asr,
    Ror,
}

impl ShiftKind {
    pub fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Lsl,
            1 => Self::Lsr,
            2 => Self::Asr,
            _ => Self::Ror,
        }
    }
}

/// Shifts by a 5-bit immediate, applying the special encodings:
/// `LSR #0`/`ASR #0` mean a shift by 32 and `ROR #0` means RRX.
///
/// Returns the shifted value and the shifter carry-out.
pub fn shift_imm(kind: ShiftKind, value: u32, imm5: u32, carry_in: bool) -> (u32, bool) {
    match (kind, imm5) {
        (ShiftKind::Lsl, 0) => (value, carry_in),
        (ShiftKind::Lsr | ShiftKind::Asr, 0) => shift_reg(kind, value, 32, carry_in),
        (ShiftKind::Ror, 0) => ((u32::from(carry_in) << 31) | (value >> 1), value & 1 != 0),
        _ => shift_reg(kind, value, imm5, carry_in),
    }
}

/// Shifts by an arbitrary amount (the bottom byte of a register).
///
/// An amount of zero leaves both value and carry unchanged.
pub fn shift_reg(kind: ShiftKind, value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 {
        return (value, carry_in);
    }
    match kind {
        ShiftKind::Lsl => match amount {
            1..=31 => (value << amount, (value >> (32 - amount)) & 1 != 0),
            32 => (0, value & 1 != 0),
            _ => (0, false),
        },
        ShiftKind::Lsr => match amount {
            1..=31 => (value >> amount, (value >> (amount - 1)) & 1 != 0),
            32 => (0, value >> 31 != 0),
            _ => (0, false),
        },
        ShiftKind::Asr => {
            if amount < 32 {
                (
                    ((value as i32) >> amount) as u32,
                    (value >> (amount - 1)) & 1 != 0,
                )
            } else {
                (((value as i32) >> 31) as u32, value >> 31 != 0)
            }
        }
        ShiftKind::Ror => {
            let rotation = amount & 31;
            if rotation == 0 {
                (value, value >> 31 != 0)
            } else {
                let result = value.rotate_right(rotation);
                (result, result >> 31 != 0)
            }
        }
    }
}

/// `a + b + carry_in`, returning `(result, carry_out, signed_overflow)`.
///
/// Subtraction is expressed as `a + !b + 1` by the callers.
pub fn add_with_carry(a: u32, b: u32, carry_in: bool) -> (u32, bool, bool) {
    let wide = u64::from(a) + u64::from(b) + u64::from(carry_in);
    let result = wide as u32;
    let carry = wide >> 32 != 0;
    let overflow = ((a ^ result) & (b ^ result)) >> 31 != 0;
    (result, carry, overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShiftKind::*;

    #[test]
    fn zero_amount_register_shift_is_identity() {
        assert_eq!(shift_reg(Lsl, 0xF0, 0, true), (0xF0, true));
        assert_eq!(shift_reg(Ror, 0xF0, 0, false), (0xF0, false));
    }

    #[test]
    fn lsl_carry_is_last_bit_shifted_out() {
        assert_eq!(shift_reg(Lsl, 0x8000_0001, 1, false), (2, true));
        assert_eq!(shift_reg(Lsl, 0x0000_0001, 32, false), (0, true));
        assert_eq!(shift_reg(Lsl, 0xFFFF_FFFF, 33, true), (0, false));
    }

    #[test]
    fn lsr_by_32_moves_bit_31_into_carry() {
        assert_eq!(shift_imm(Lsr, 0x8000_0000, 0, false), (0, true));
        assert_eq!(shift_reg(Lsr, 0x0000_0003, 1, false), (1, true));
        assert_eq!(shift_reg(Lsr, u32::MAX, 40, true), (0, false));
    }

    #[test]
    fn asr_fills_with_the_sign_bit() {
        assert_eq!(shift_reg(Asr, 0x8000_0000, 4, false), (0xF800_0000, false));
        assert_eq!(shift_imm(Asr, 0x8000_0000, 0, false), (u32::MAX, true));
        assert_eq!(shift_imm(Asr, 0x4000_0000, 0, true), (0, false));
    }

    #[test]
    fn ror_zero_immediate_is_rrx() {
        assert_eq!(shift_imm(Ror, 0b11, 0, true), (0x8000_0001, true));
        assert_eq!(shift_imm(Ror, 0b10, 0, false), (1, false));
    }

    #[test]
    fn ror_by_multiple_of_32_keeps_value() {
        assert_eq!(shift_reg(Ror, 0x8000_0000, 64, false), (0x8000_0000, true));
        assert_eq!(shift_reg(Ror, 0x0000_0001, 1, false), (0x8000_0000, true));
    }

    #[test]
    fn add_with_carry_reports_carry_and_overflow() {
        assert_eq!(add_with_carry(u32::MAX, 1, false), (0, true, false));
        assert_eq!(
            add_with_carry(0x7FFF_FFFF, 1, false),
            (0x8000_0000, false, true)
        );
        // 0 - 1 as 0 + !1 + 1 borrows (carry clear).
        assert_eq!(add_with_carry(0, !1, true), (u32::MAX, false, false));
        // 5 - 5 does not borrow.
        assert_eq!(add_with_carry(5, !5, true), (0, true, false));
    }
}
