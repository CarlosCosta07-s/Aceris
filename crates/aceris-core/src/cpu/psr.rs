//! Program status register.

/// ARM program status register (CPSR/SPSR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Psr(pub u32);

impl Psr {
    pub const N: u32 = 1 << 31;
    pub const Z: u32 = 1 << 30;
    pub const C: u32 = 1 << 29;
    pub const V: u32 = 1 << 28;

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
}
