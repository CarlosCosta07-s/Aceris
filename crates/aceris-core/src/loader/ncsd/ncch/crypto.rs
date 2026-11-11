//! Original AES-128 and NCCH cryptographic parameter primitives.
//!
//! No retail console keys are embedded. Callers must provide the appropriate
//! 128-bit key from their own lawful system material. This module implements
//! AES-128 encryption, AES-CTR processing, the documented 3DS key scrambler,
//! and NCCH initial-counter construction.

/// AES-128 block and key size in bytes.
pub const AES_BLOCK_SIZE: usize = 16;
const AES_ROUNDS: usize = 10;
const EXPANDED_KEY_SIZE: usize = AES_BLOCK_SIZE * (AES_ROUNDS + 1);
const KEY_SCRAMBLER_CONSTANT: u128 = 0x1ff9_e9aa_c5fe_0408_0245_91dc_5d52_768a;

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NcchCtrRegion {
    ExtendedHeader,
    ExeFs,
    RomFs,
}

impl NcchCtrRegion {
    const fn marker(self) -> u8 {
        match self {
            Self::ExtendedHeader => 1,
            Self::ExeFs => 2,
            Self::RomFs => 3,
        }
    }
}

/// Failures while preparing NCCH cryptographic parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NcchCryptoError {
    UnsupportedNcchVersion(u16),
    UnsupportedCryptoMethod(u8),
    RegionNotPresent(NcchCtrRegion),
    RegionOffsetTooLarge(u64),
    CounterOffsetOverflow,
}

impl std::fmt::Display for NcchCryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedNcchVersion(version) => {
                write!(f, "unsupported NCCH counter version {version}")
            }
            Self::UnsupportedCryptoMethod(method) => {
                write!(f, "unsupported NCCH crypto method 0x{method:02x}")
            }
            Self::RegionNotPresent(region) => write!(f, "NCCH region {region:?} is not present"),
            Self::RegionOffsetTooLarge(offset) => write!(
                f,
                "NCCH region offset 0x{offset:x} does not fit in the counter"
            ),
            Self::CounterOffsetOverflow => write!(f, "AES-CTR counter offset overflows"),
        }
    }
}

impl std::error::Error for NcchCryptoError {}

/// Identifies the 3DS AES keyslot family selected by NCCH flag byte 3.
pub fn content_keyslot(crypto_method: u8) -> Result<u8, NcchCryptoError> {
    match crypto_method {
        0x00 => Ok(0x2c),
        0x01 => Ok(0x25),
        0x0a => Ok(0x18),
        0x0b => Ok(0x1b),
        value => Err(NcchCryptoError::UnsupportedCryptoMethod(value)),
    }
}

/// Derives a 3DS normal key from caller-supplied KeyX and KeyY.
///
/// Inputs and intermediate 128-bit arithmetic are interpreted as big-endian
/// values. No console key material is embedded in Aceris.
pub fn derive_normal_key(key_x: [u8; 16], key_y: [u8; 16]) -> [u8; 16] {
    let x = u128::from_be_bytes(key_x);
    let y = u128::from_be_bytes(key_y);
    let mixed = x.rotate_left(2) ^ y;
    mixed
        .wrapping_add(KEY_SCRAMBLER_CONSTANT)
        .rotate_right(41)
        .to_be_bytes()
}

/// Builds the initial AES-CTR value for an NCCH encrypted region.
///
/// NCCH versions 0 and 2 use the reversed partition ID in the high 64 bits
/// and a region marker in byte 8. Version 1 uses the partition ID in stored
/// byte order followed by the region's absolute offset as big-endian u32.
/// For versions 0 and 2, the region offset is intentionally ignored.
pub fn initial_ctr(
    partition_id: [u8; 8],
    ncch_version: u16,
    region: NcchCtrRegion,
    region_offset_bytes: u64,
) -> Result<[u8; 16], NcchCryptoError> {
    let mut counter = [0; 16];
    match ncch_version {
        0 | 2 => {
            for (destination, source) in counter[..8].iter_mut().zip(partition_id.iter().rev()) {
                *destination = *source;
            }
            counter[8] = region.marker();
        }
        1 => {
            let offset = u32::try_from(region_offset_bytes)
                .map_err(|_| NcchCryptoError::RegionOffsetTooLarge(region_offset_bytes))?;
            counter[..8].copy_from_slice(&partition_id);
            counter[12..16].copy_from_slice(&offset.to_be_bytes());
        }
        other => return Err(NcchCryptoError::UnsupportedNcchVersion(other)),
    }
    Ok(counter)
}

/// AES-128 encryption primitive.
#[derive(Clone)]
pub struct Aes128 {
    round_keys: [[u8; AES_BLOCK_SIZE]; AES_ROUNDS + 1],
}

impl Aes128 {
    pub fn new(key: [u8; AES_BLOCK_SIZE]) -> Self {
        let mut expanded = [0; EXPANDED_KEY_SIZE];
        expanded[..AES_BLOCK_SIZE].copy_from_slice(&key);

        let mut generated = AES_BLOCK_SIZE;
        let mut round = 1usize;
        let mut temp = [0; 4];

        while generated < EXPANDED_KEY_SIZE {
            temp.copy_from_slice(&expanded[generated - 4..generated]);
            if generated % AES_BLOCK_SIZE == 0 {
                temp.rotate_left(1);
                for byte in &mut temp {
                    *byte = SBOX[*byte as usize];
                }
                temp[0] ^= round_constant(round);
                round += 1;
            }

            for byte in temp {
                expanded[generated] = expanded[generated - AES_BLOCK_SIZE] ^ byte;
                generated += 1;
            }
        }

        let mut round_keys = [[0; AES_BLOCK_SIZE]; AES_ROUNDS + 1];
        for (round, key) in round_keys.iter_mut().enumerate() {
            let start = round * AES_BLOCK_SIZE;
            key.copy_from_slice(&expanded[start..start + AES_BLOCK_SIZE]);
        }
        Self { round_keys }
    }

    /// Encrypts exactly one 128-bit block.
    pub fn encrypt_block(&self, block: [u8; AES_BLOCK_SIZE]) -> [u8; AES_BLOCK_SIZE] {
        let mut state = block;
        add_round_key(&mut state, &self.round_keys[0]);
        for round in 1..=AES_ROUNDS {
            sub_bytes(&mut state);
            shift_rows(&mut state);
            if round != AES_ROUNDS {
                mix_columns(&mut state);
            }
            add_round_key(&mut state, &self.round_keys[round]);
        }
        state
    }
}

fn round_constant(round: usize) -> u8 {
    let mut value = 1u8;
    for _ in 1..round {
        value = xtime(value);
    }
    value
}

fn xtime(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0 }
}

fn add_round_key(state: &mut [u8; AES_BLOCK_SIZE], key: &[u8; AES_BLOCK_SIZE]) {
    for (state_byte, key_byte) in state.iter_mut().zip(key) {
        *state_byte ^= key_byte;
    }
}

fn sub_bytes(state: &mut [u8; AES_BLOCK_SIZE]) {
    for byte in state {
        *byte = SBOX[*byte as usize];
    }
}

fn shift_rows(state: &mut [u8; AES_BLOCK_SIZE]) {
    let previous = *state;
    for row in 0..4 {
        for column in 0..4 {
            state[column * 4 + row] = previous[((column + row) % 4) * 4 + row];
        }
    }
}

fn mix_columns(state: &mut [u8; AES_BLOCK_SIZE]) {
    for column in 0..4 {
        let start = column * 4;
        let a = state[start];
        let b = state[start + 1];
        let c = state[start + 2];
        let d = state[start + 3];
        let total = a ^ b ^ c ^ d;
        state[start] = a ^ total ^ xtime(a ^ b);
        state[start + 1] = b ^ total ^ xtime(b ^ c);
        state[start + 2] = c ^ total ^ xtime(c ^ d);
        state[start + 3] = d ^ total ^ xtime(d ^ a);
    }
}

/// Stateful AES-CTR processor. Counters are 128-bit big-endian values.
#[derive(Clone)]
pub struct Aes128Ctr {
    cipher: Aes128,
    counter: [u8; AES_BLOCK_SIZE],
    keystream_block: [u8; AES_BLOCK_SIZE],
    keystream_offset: usize,
}

impl Aes128Ctr {
    pub fn new(key: [u8; AES_BLOCK_SIZE], initial_counter: [u8; AES_BLOCK_SIZE]) -> Self {
        Self {
            cipher: Aes128::new(key),
            counter: initial_counter,
            keystream_block: [0; AES_BLOCK_SIZE],
            keystream_offset: AES_BLOCK_SIZE,
        }
    }

    /// XORs the next bytes of the CTR keystream into data.
    ///
    /// Repeated calls continue at the exact byte position reached previously,
    /// including calls which end in the middle of a block.
    pub fn apply_keystream(&mut self, data: &mut [u8]) {
        for byte in data {
            if self.keystream_offset == AES_BLOCK_SIZE {
                self.keystream_block = self.cipher.encrypt_block(self.counter);
                increment_counter(&mut self.counter);
                self.keystream_offset = 0;
            }
            *byte ^= self.keystream_block[self.keystream_offset];
            self.keystream_offset += 1;
        }
    }

    /// Applies CTR at an arbitrary byte offset without processing prior data.
    pub fn apply_keystream_at_offset(
        key: [u8; AES_BLOCK_SIZE],
        initial_counter: [u8; AES_BLOCK_SIZE],
        byte_offset: u64,
        data: &mut [u8],
    ) -> Result<(), NcchCryptoError> {
        let blocks = u128::from(byte_offset / AES_BLOCK_SIZE as u64);
        let remainder = (byte_offset % AES_BLOCK_SIZE as u64) as usize;
        let counter_value = u128::from_be_bytes(initial_counter)
            .checked_add(blocks)
            .ok_or(NcchCryptoError::CounterOffsetOverflow)?;
        let mut stream = Self::new(key, counter_value.to_be_bytes());

        if remainder != 0 {
            let mut discard = [0; AES_BLOCK_SIZE];
            stream.apply_keystream(&mut discard[..remainder]);
        }
        stream.apply_keystream(data);
        Ok(())
    }
}

fn increment_counter(counter: &mut [u8; AES_BLOCK_SIZE]) {
    for byte in counter.iter_mut().rev() {
        let (next, overflow) = byte.overflowing_add(1);
        *byte = next;
        if !overflow {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex<const N: usize>(value: &str) -> [u8; N] {
        assert_eq!(value.len(), N * 2);
        let mut result = [0; N];
        for (index, byte) in result.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap();
        }
        result
    }

    #[test]
    fn aes_128_matches_fips_197_known_answer() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let plaintext = hex("00112233445566778899aabbccddeeff");
        let expected = hex("69c4e0d86a7b0430d8cdb78070b4c55a");
        assert_eq!(Aes128::new(key).encrypt_block(plaintext), expected);
    }

    #[test]
    fn aes_ctr_matches_nist_sp_800_38a_four_block_vector() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let counter = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut data = hex::<64>(
            "6bc1bee22e409f96e93d7e117393172a\
             ae2d8a571e03ac9c9eb76fac45af8e51\
             30c81c46a35ce411e5fbc1191a0a52ef\
             f69f2445df4f9b17ad2b417be66c3710",
        );
        let expected = hex::<64>(
            "874d6191b620e3261bef6864990db6ce\
             9806f66b7970fdff8617187bb9fffdff\
             5ae4df3edbd5d35e5b4f09020db03eab\
             1e031dda2fbe03d1792170a0f3009cee",
        );
        Aes128Ctr::new(key, counter).apply_keystream(&mut data);
        assert_eq!(data, expected);

        Aes128Ctr::new(key, counter).apply_keystream(&mut data);
        assert_eq!(
            data,
            hex::<64>(
                "6bc1bee22e409f96e93d7e117393172a\
                 ae2d8a571e03ac9c9eb76fac45af8e51\
                 30c81c46a35ce411e5fbc1191a0a52ef\
                 f69f2445df4f9b17ad2b417be66c3710"
            )
        );
    }

    #[test]
    fn ctr_streaming_calls_preserve_partial_block_position() {
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        let counter = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let plaintext = hex::<29>(
            "6bc1bee22e409f96e93d7e117393172a\
             ae2d8a571e03ac9c9eb76fac45",
        );
        let mut expected = plaintext;
        Aes128Ctr::new(key, counter).apply_keystream(&mut expected);

        let mut actual = plaintext;
        let mut stream = Aes128Ctr::new(key, counter);
        stream.apply_keystream(&mut actual[..3]);
        stream.apply_keystream(&mut actual[3..19]);
        stream.apply_keystream(&mut actual[19..]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn ctr_random_access_matches_sequential_processing() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut sequential = [0u8; 73];
        Aes128Ctr::new(key, counter).apply_keystream(&mut sequential);
        let mut random_access = [0u8; 37];
        Aes128Ctr::apply_keystream_at_offset(key, counter, 21, &mut random_access).unwrap();
        assert_eq!(&random_access[..], &sequential[21..58]);
    }

    #[test]
    fn key_scrambler_matches_independent_known_answer() {
        let key_x = hex("00112233445566778899aabbccddeeff");
        let key_y = hex("ffeeddccbbaa99887766554433221100");
        assert_eq!(
            derive_normal_key(key_x, key_y),
            hex("c3aed410c30fd21f56387e822f2ba348")
        );
    }

    #[test]
    fn content_crypto_method_selects_documented_keyslot() {
        assert_eq!(content_keyslot(0x00), Ok(0x2c));
        assert_eq!(content_keyslot(0x01), Ok(0x25));
        assert_eq!(content_keyslot(0x0a), Ok(0x18));
        assert_eq!(content_keyslot(0x0b), Ok(0x1b));
        assert_eq!(
            content_keyslot(0x02),
            Err(NcchCryptoError::UnsupportedCryptoMethod(0x02))
        );
    }

    #[test]
    fn legacy_and_current_ncch_counters_use_region_markers() {
        let id = hex("0001020304050607");
        assert_eq!(
            initial_ctr(id, 0, NcchCtrRegion::ExtendedHeader, 0x200).unwrap(),
            hex("07060504030201010100000000000000")
        );
        assert_eq!(
            initial_ctr(id, 2, NcchCtrRegion::RomFs, 0x123400).unwrap(),
            hex("07060504030201010300000000000000")
        );
        assert_eq!(
            initial_ctr(id, 2, NcchCtrRegion::ExeFs, 0).unwrap(),
            hex("07060504030201010200000000000000")
        );
    }

    #[test]
    fn version_one_counter_encodes_absolute_region_offset_big_endian() {
        let id = hex("0001020304050607");
        assert_eq!(
            initial_ctr(id, 1, NcchCtrRegion::ExtendedHeader, 0x200).unwrap(),
            hex("00010203040506070000000000000200")
        );
        assert_eq!(
            initial_ctr(id, 1, NcchCtrRegion::RomFs, 0x12345678).unwrap(),
            hex("00010203040506070000000012345678")
        );
    }

    #[test]
    fn rejects_unknown_counter_version_and_out_of_range_region_offset() {
        let id = hex("0001020304050607");
        assert_eq!(
            initial_ctr(id, 3, NcchCtrRegion::ExeFs, 0),
            Err(NcchCryptoError::UnsupportedNcchVersion(3))
        );
        assert_eq!(
            initial_ctr(id, 1, NcchCtrRegion::RomFs, u64::from(u32::MAX) + 1),
            Err(NcchCryptoError::RegionOffsetTooLarge(
                u64::from(u32::MAX) + 1
            ))
        );
    }

    #[test]
    fn counter_advances_as_a_big_endian_128_bit_integer() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = hex("000000000000000000000000000000ff");
        let mut bytes = [0u8; 32];
        Aes128Ctr::new(key, counter).apply_keystream(&mut bytes);
        let first = Aes128::new(key).encrypt_block(counter);
        let second = Aes128::new(key).encrypt_block(hex("00000000000000000000000000000100"));
        let mut expected = [0; 32];
        expected[..16].copy_from_slice(&first);
        expected[16..].copy_from_slice(&second);
        assert_eq!(bytes, expected);
    }
}
