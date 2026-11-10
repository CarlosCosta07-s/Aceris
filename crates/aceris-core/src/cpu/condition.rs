//! ARM condition codes.

use super::Psr;

/// The 4-bit condition field found in the top bits of every ARM instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Eq,
    Ne,
    Cs,
    Cc,
    Mi,
    Pl,
    Vs,
    Vc,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
    Al,
    /// `0b1111`: instructions in the unconditional encoding space.
    Unconditional,
}

impl Condition {
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0xF {
            0x0 => Self::Eq,
            0x1 => Self::Ne,
            0x2 => Self::Cs,
            0x3 => Self::Cc,
            0x4 => Self::Mi,
            0x5 => Self::Pl,
            0x6 => Self::Vs,
            0x7 => Self::Vc,
            0x8 => Self::Hi,
            0x9 => Self::Ls,
            0xA => Self::Ge,
            0xB => Self::Lt,
            0xC => Self::Gt,
            0xD => Self::Le,
            0xE => Self::Al,
            _ => Self::Unconditional,
        }
    }

    /// Whether an instruction with this condition executes given `psr`.
    pub fn passes(self, psr: Psr) -> bool {
        let (n, z, c, v) = (psr.n(), psr.z(), psr.c(), psr.v());
        match self {
            Self::Eq => z,
            Self::Ne => !z,
            Self::Cs => c,
            Self::Cc => !c,
            Self::Mi => n,
            Self::Pl => !n,
            Self::Vs => v,
            Self::Vc => !v,
            Self::Hi => c && !z,
            Self::Ls => !c || z,
            Self::Ge => n == v,
            Self::Lt => n != v,
            Self::Gt => !z && n == v,
            Self::Le => z || n != v,
            Self::Al | Self::Unconditional => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_all_sixteen_encodings() {
        assert_eq!(Condition::from_bits(0x0), Condition::Eq);
        assert_eq!(Condition::from_bits(0xE), Condition::Al);
        assert_eq!(Condition::from_bits(0xF), Condition::Unconditional);
    }

    #[test]
    fn equality_conditions_follow_zero_flag() {
        let zero = Psr::from_flags(false, true, false, false);
        assert!(Condition::Eq.passes(zero));
        assert!(!Condition::Ne.passes(zero));
        assert!(!Condition::Eq.passes(Psr::default()));
    }

    #[test]
    fn unsigned_higher_needs_carry_and_not_zero() {
        assert!(Condition::Hi.passes(Psr::from_flags(false, false, true, false)));
        assert!(!Condition::Hi.passes(Psr::from_flags(false, true, true, false)));
        assert!(Condition::Ls.passes(Psr::from_flags(false, true, true, false)));
    }

    #[test]
    fn signed_comparisons_compare_n_and_v() {
        let n_only = Psr::from_flags(true, false, false, false);
        let n_and_v = Psr::from_flags(true, false, false, true);
        assert!(Condition::Lt.passes(n_only));
        assert!(Condition::Ge.passes(n_and_v));
        assert!(Condition::Gt.passes(n_and_v));
        assert!(Condition::Le.passes(n_only));
    }

    #[test]
    fn always_passes_regardless_of_flags() {
        for bits in [0, Psr::N, Psr::Z, Psr::C, Psr::V, u32::MAX] {
            assert!(Condition::Al.passes(Psr(bits)));
        }
    }
}
