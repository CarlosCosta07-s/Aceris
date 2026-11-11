//! An original, streaming SHA-256 implementation for Aceris.
//!
//! The algorithm follows NIST FIPS 180-4. It has no external dependencies and
//! accepts input incrementally so large RomFS levels need not be buffered whole.

use std::fmt;

const BLOCK_SIZE: usize = 64;
const DIGEST_SIZE: usize = 32;
const MAX_MESSAGE_BYTES: u64 = u64::MAX / 8;

const INITIAL_STATE: [u32; 8] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];

const ROUND_CONSTANTS: [u32; 64] = [
    0x428A_2F98,
    0x7137_4491,
    0xB5C0_FBCF,
    0xE9B5_DBA5,
    0x3956_C25B,
    0x59F1_11F1,
    0x923F_82A4,
    0xAB1C_5ED5,
    0xD807_AA98,
    0x1283_5B01,
    0x2431_85BE,
    0x550C_7DC3,
    0x72BE_5D74,
    0x80DE_B1FE,
    0x9BDC_06A7,
    0xC19B_F174,
    0xE49B_69C1,
    0xEFBE_4786,
    0x0FC1_9DC6,
    0x240C_A1CC,
    0x2DE9_2C6F,
    0x4A74_84AA,
    0x5CB0_A9DC,
    0x76F9_88DA,
    0x983E_5152,
    0xA831_C66D,
    0xB003_27C8,
    0xBF59_7FC7,
    0xC6E0_0BF3,
    0xD5A7_9147,
    0x06CA_6351,
    0x1429_2967,
    0x27B7_0A85,
    0x2E1B_2138,
    0x4D2C_6DFC,
    0x5338_0D13,
    0x650A_7354,
    0x766A_0ABB,
    0x81C2_C92E,
    0x9272_2C85,
    0xA2BF_E8A1,
    0xA81A_664B,
    0xC24B_8B70,
    0xC76C_51A3,
    0xD192_E819,
    0xD699_0624,
    0xF40E_3585,
    0x106A_A070,
    0x19A4_C116,
    0x1E37_6C08,
    0x2748_774C,
    0x34B0_BCB5,
    0x391C_0CB3,
    0x4ED8_AA4A,
    0x5B9C_CA4F,
    0x682E_6FF3,
    0x748F_82EE,
    0x78A5_636F,
    0x84C8_7814,
    0x8CC7_0208,
    0x90BE_FFFA,
    0xA450_6CEB,
    0xBEF9_A3F7,
    0xC671_78F2,
];

/// Errors that can occur while feeding a SHA-256 message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sha256Error {
    /// FIPS 180-4 limits SHA-256 messages to fewer than 2^64 bits.
    MessageTooLong,
}

impl fmt::Display for Sha256Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MessageTooLong => {
                formatter.write_str("SHA-256 message exceeds the 2^64-bit limit")
            }
        }
    }
}

impl std::error::Error for Sha256Error {}

/// Incremental SHA-256 state.
///
/// Call `update` with any number of byte slices, then call `finalize` once.
/// The final digest is returned in the standard big-endian byte order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; BLOCK_SIZE],
    buffer_len: usize,
    total_len_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// Creates a new SHA-256 computation.
    pub const fn new() -> Self {
        Self {
            state: INITIAL_STATE,
            buffer: [0; BLOCK_SIZE],
            buffer_len: 0,
            total_len_bytes: 0,
        }
    }

    /// Adds bytes to the message without buffering previously processed blocks.
    pub fn update(&mut self, mut input: &[u8]) -> Result<(), Sha256Error> {
        if input.is_empty() {
            return Ok(());
        }

        let input_len = u64::try_from(input.len()).map_err(|_| Sha256Error::MessageTooLong)?;
        let new_total = self
            .total_len_bytes
            .checked_add(input_len)
            .filter(|length| *length <= MAX_MESSAGE_BYTES)
            .ok_or(Sha256Error::MessageTooLong)?;
        self.total_len_bytes = new_total;

        if self.buffer_len != 0 {
            let needed = BLOCK_SIZE - self.buffer_len;
            if input.len() < needed {
                self.buffer[self.buffer_len..self.buffer_len + input.len()].copy_from_slice(input);
                self.buffer_len += input.len();
                return Ok(());
            }

            self.buffer[self.buffer_len..].copy_from_slice(&input[..needed]);
            let block = self.buffer;
            self.compress(&block);
            self.buffer_len = 0;
            self.buffer.fill(0);
            input = &input[needed..];
        }

        let mut blocks = input.chunks_exact(BLOCK_SIZE);
        for chunk in &mut blocks {
            let block: &[u8; BLOCK_SIZE] = chunk
                .try_into()
                .expect("chunks_exact always yields complete SHA-256 blocks");
            self.compress(block);
        }

        let remainder = blocks.remainder();
        if !remainder.is_empty() {
            self.buffer[..remainder.len()].copy_from_slice(remainder);
            self.buffer_len = remainder.len();
        }

        Ok(())
    }

    /// Completes the message and returns the 32-byte digest.
    pub fn finalize(mut self) -> [u8; DIGEST_SIZE] {
        let message_bit_len = self.total_len_bytes * 8;

        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buffer = [0; BLOCK_SIZE];
        } else {
            self.buffer[self.buffer_len..56].fill(0);
        }

        self.buffer[56..64].copy_from_slice(&message_bit_len.to_be_bytes());
        let final_block = self.buffer;
        self.compress(&final_block);

        let mut digest = [0; DIGEST_SIZE];
        for (index, word) in self.state.iter().enumerate() {
            let start = index * 4;
            digest[start..start + 4].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    /// Hashes a complete in-memory message.
    pub fn digest(input: &[u8]) -> Result<[u8; DIGEST_SIZE], Sha256Error> {
        let mut hasher = Self::new();
        hasher.update(input)?;
        Ok(hasher.finalize())
    }

    fn compress(&mut self, block: &[u8; BLOCK_SIZE]) {
        let mut schedule = [0u32; 64];

        for (index, word_bytes) in block.chunks_exact(4).enumerate() {
            schedule[index] =
                u32::from_be_bytes([word_bytes[0], word_bytes[1], word_bytes[2], word_bytes[3]]);
        }

        for index in 16..64 {
            let x = schedule[index - 15];
            let y = schedule[index - 2];
            let small_sigma_0 = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let small_sigma_1 = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(small_sigma_0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(small_sigma_1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;

        for index in 0..64 {
            let big_sigma_1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp_1 = h
                .wrapping_add(big_sigma_1)
                .wrapping_add(choose)
                .wrapping_add(ROUND_CONSTANTS[index])
                .wrapping_add(schedule[index]);

            let big_sigma_0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp_2 = big_sigma_0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp_1);
            d = c;
            c = b;
            b = a;
            a = temp_1.wrapping_add(temp_2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_hex(bytes: &[u8]) -> String {
        let mut result = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use fmt::Write as _;
            write!(&mut result, "{byte:02x}").unwrap();
        }
        result
    }

    #[test]
    fn matches_empty_message_known_answer() {
        let digest = Sha256::digest(b"").unwrap();
        assert_eq!(
            to_hex(&digest),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn matches_abc_known_answer() {
        let digest = Sha256::digest(b"abc").unwrap();
        assert_eq!(
            to_hex(&digest),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn matches_multi_block_known_answer() {
        let message = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        let digest = Sha256::digest(message).unwrap();
        assert_eq!(
            to_hex(&digest),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn matches_million_a_known_answer_with_incremental_updates() {
        let mut hasher = Sha256::new();
        let chunk = [b'a'; 1_000];
        for _ in 0..1_000 {
            hasher.update(&chunk).unwrap();
        }
        assert_eq!(
            to_hex(&hasher.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn incremental_chunk_boundaries_match_single_update() {
        let message = (0..=255).cycle().take(1_025).collect::<Vec<_>>();
        let expected = Sha256::digest(&message).unwrap();

        for chunk_size in 1..=130 {
            let mut hasher = Sha256::new();
            for chunk in message.chunks(chunk_size) {
                hasher.update(chunk).unwrap();
            }
            assert_eq!(hasher.finalize(), expected, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn rejects_message_lengths_outside_the_standard_limit_without_mutating_state() {
        let mut hasher = Sha256::new();
        hasher.total_len_bytes = MAX_MESSAGE_BYTES;
        let before = hasher.clone();

        assert_eq!(hasher.update(b"x"), Err(Sha256Error::MessageTooLong));
        assert_eq!(hasher, before);
    }

    #[test]
    fn accepts_empty_updates_without_changing_state() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc").unwrap();
        let before = hasher.clone();

        hasher.update(b"").unwrap();
        assert_eq!(hasher, before);
    }
}
