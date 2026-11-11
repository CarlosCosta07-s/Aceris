//! Program status register.

/// ARM program status register (CPSR/SPSR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Psr(pub u32);

impl Psr {
    pub const N: u32 = 1 << 31;
    pub const Z: u32 = 1 << 30;
    pub const C: u32 = 1 << 29;
    pub const V: u32 = 1 << 28;
    /// Thumb state bit.
    pub const T: u32 = 1 << 5;

    pub const fn from_flags(n: bool, z: bool, c: bool, v: bool) -> Self {
        let mut bits = 0;
        if n {
            bits |= Self::N;
        }
        if z {
            bits |= Self::Z;
        }
        if c {
            bits |= Self::C;
        }
        if v {
            bits |= Self::V;
        }
        Self(bits)
    }

    pub const fn n(self) -> bool {
        self.0 & Self::N != 0
    }

    pub const fn z(self) -> bool {
        self.0 & Self::Z != 0
    }

    pub const fn c(self) -> bool {
        self.0 & Self::C != 0
    }

    pub const fn v(self) -> bool {
        self.0 & Self::V != 0
    }

    pub const fn thumb(self) -> bool {
        self.0 & Self::T != 0
    }

    pub fn set_thumb(&mut self, thumb: bool) {
        if thumb {
            self.0 |= Self::T;
        } else {
            self.0 &= !Self::T;
        }
    }

    /// Replaces the four condition flags, leaving every other bit untouched.
    pub fn set_flags(&mut self, n: bool, z: bool, c: bool, v: bool) {
        let mask = Self::N | Self::Z | Self::C | Self::V;
        self.0 = (self.0 & !mask) | Self::from_flags(n, z, c, v).0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_flags_preserves_other_bits() {
        let mut psr = Psr(0x0000_001F | Psr::T);
        psr.set_flags(true, false, true, false);
        assert!(psr.n() && !psr.z() && psr.c() && !psr.v());
        assert!(psr.thumb());
        assert_eq!(psr.0 & 0x1F, 0x1F);
    }

    #[test]
    fn thumb_bit_toggles() {
        let mut psr = Psr::default();
        psr.set_thumb(true);
        assert!(psr.thumb());
        psr.set_thumb(false);
        assert_eq!(psr, Psr::default());
    }
}
