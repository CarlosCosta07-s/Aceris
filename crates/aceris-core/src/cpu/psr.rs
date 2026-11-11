//! Program status register.

/// ARM program status register (CPSR/SPSR).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Psr(pub u32);

impl Default for Psr {
    fn default() -> Self {
        Self(Self::USER_MODE)
    }
}

impl Psr {
    pub const N: u32 = 1 << 31;
    pub const Z: u32 = 1 << 30;
    pub const C: u32 = 1 << 29;
    pub const V: u32 = 1 << 28;
    /// Sticky saturation flag.
    pub const Q: u32 = 1 << 27;
    /// Greater-than-or-equal flags used by ARMv6 parallel media instructions.
    pub const GE_MASK: u32 = 0xF << 16;
    /// Big-endian data access state.
    pub const E: u32 = 1 << 9;
    /// Asynchronous abort mask.
    pub const A: u32 = 1 << 8;
    /// IRQ mask.
    pub const I: u32 = 1 << 7;
    /// FIQ mask.
    pub const F: u32 = 1 << 6;
    /// Thumb state bit.
    pub const T: u32 = 1 << 5;
    /// CPSR mode field.
    pub const MODE_MASK: u32 = 0x1F;
    /// User mode: the mode initially supported by the bootstrap.
    pub const USER_MODE: u32 = 0x10;

    pub const fn from_flags(n: bool, z: bool, c: bool, v: bool) -> Self {
        let mut bits = Self::USER_MODE;
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

    /// Returns the sticky saturation flag.
    pub const fn q(self) -> bool {
        self.0 & Self::Q != 0
    }

    /// Returns the four ARMv6 GE flags as bits 3..0.
    pub const fn ge(self) -> u8 {
        ((self.0 & Self::GE_MASK) >> 16) as u8
    }

    /// Replaces only the four ARMv6 GE flags.
    pub fn set_ge(&mut self, ge: u8) {
        self.0 = (self.0 & !Self::GE_MASK) | (u32::from(ge & 0xF) << 16);
    }

    /// Sets the sticky saturation flag. It is never cleared implicitly.
    pub fn set_q(&mut self) {
        self.0 |= Self::Q;
    }

    pub const fn thumb(self) -> bool {
        self.0 & Self::T != 0
    }

    /// Returns whether data accesses use big-endian byte order.
    pub const fn big_endian(self) -> bool {
        self.0 & Self::E != 0
    }

    /// Returns the CPSR mode field.
    pub const fn mode(self) -> u32 {
        self.0 & Self::MODE_MASK
    }

    /// Returns whether the current mode has privileged CPSR control.
    pub const fn privileged(self) -> bool {
        self.mode() != Self::USER_MODE
    }

    /// Changes only the CPSR data endianness bit.
    pub fn set_big_endian(&mut self, big_endian: bool) {
        if big_endian {
            self.0 |= Self::E;
        } else {
            self.0 &= !Self::E;
        }
    }

    /// Sets or clears selected A/I/F interrupt masks (mask bit 0=A, 1=I, 2=F).
    /// The boolean is true for CPSID (masked) and false for CPSIE (unmasked).
    pub fn set_interrupt_masks(&mut self, mask: u8, masked: bool) {
        let mut bits = 0;
        if mask & 0b001 != 0 {
            bits |= Self::A;
        }
        if mask & 0b010 != 0 {
            bits |= Self::I;
        }
        if mask & 0b100 != 0 {
            bits |= Self::F;
        }
        if masked {
            self.0 |= bits;
        } else {
            self.0 &= !bits;
        }
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
        let mut flags = 0;
        if n {
            flags |= Self::N;
        }
        if z {
            flags |= Self::Z;
        }
        if c {
            flags |= Self::C;
        }
        if v {
            flags |= Self::V;
        }
        self.0 = (self.0 & !mask) | flags;
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

    #[test]
    fn ge_flags_update_independently_of_condition_and_saturation_flags() {
        let mut psr = Psr(0xE800_0010);
        psr.set_ge(0b1010);
        assert_eq!(psr.ge(), 0b1010);
        assert!(psr.q());
        assert!(psr.n());
        assert!(psr.z());
        assert_eq!(psr.mode(), Psr::USER_MODE);

        psr.set_ge(0b0101);
        assert_eq!(psr.ge(), 0b0101);
        assert!(psr.q());
    }

    #[test]
    fn reset_starts_in_user_little_endian_mode() {
        let psr = Psr::default();
        assert_eq!(psr.mode(), Psr::USER_MODE);
        assert!(!psr.privileged());
        assert!(!psr.big_endian());
    }

    #[test]
    fn endianness_and_interrupt_masks_preserve_unrelated_state() {
        let mut psr = Psr::default();
        psr.set_thumb(true);
        psr.set_big_endian(true);
        psr.set_interrupt_masks(0b011, true);
        assert!(psr.thumb());
        assert!(psr.big_endian());
        assert_ne!(psr.0 & (Psr::A | Psr::I), 0);
        psr.set_big_endian(false);
        assert!(!psr.big_endian());
        assert!(psr.thumb());
    }
}
