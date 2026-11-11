//! AES-128-CBC primitives for 3DS ticket title keys and CIA content streams.
//!
//! Ticket title keys are exactly one 16-byte block; CIA content readers process
//! complete blocks in a bounded stream. Callers supply all AES key material.
//! This software implementation is not hardened against local side-channel attacks.

use std::fmt;
use std::io::{self, Read};

const BLOCK_SIZE: usize = 16;
const EXPANDED_KEY_SIZE: usize = 176;
const ROUND_COUNT: usize = 10;

const fn gf_mul(mut left: u8, mut right: u8) -> u8 {
    let mut product = 0u8;
    let mut bit = 0;
    while bit < 8 {
        if right & 1 != 0 {
            product ^= left;
        }
        let high_bit = left & 0x80;
        left <<= 1;
        if high_bit != 0 {
            left ^= 0x1B;
        }
        right >>= 1;
        bit += 1;
    }
    product
}

const fn gf_pow(mut value: u8, mut exponent: u8) -> u8 {
    let mut result = 1u8;
    while exponent != 0 {
        if exponent & 1 != 0 {
            result = gf_mul(result, value);
        }
        value = gf_mul(value, value);
        exponent >>= 1;
    }
    result
}

const fn make_sbox() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut value = 0usize;
    while value < 256 {
        let inverse = if value == 0 {
            0
        } else {
            gf_pow(value as u8, 254)
        };
        table[value] = inverse
            ^ inverse.rotate_left(1)
            ^ inverse.rotate_left(2)
            ^ inverse.rotate_left(3)
            ^ inverse.rotate_left(4)
            ^ 0x63;
        value += 1;
    }
    table
}

const fn make_inverse_sbox(sbox: [u8; 256]) -> [u8; 256] {
    let mut inverse = [0u8; 256];
    let mut value = 0usize;
    while value < 256 {
        inverse[sbox[value] as usize] = value as u8;
        value += 1;
    }
    inverse
}

const SBOX: [u8; 256] = make_sbox();
const INVERSE_SBOX: [u8; 256] = make_inverse_sbox(SBOX);

/// An AES-128 title key after CBC decryption.
///
/// The bytes are intentionally not included in the Debug representation.
/// Call as_bytes or into_bytes only at the point where the decrypted key must
/// be passed to the next cryptographic stage.
#[derive(PartialEq, Eq)]
pub struct TitleKey([u8; BLOCK_SIZE]);

impl TitleKey {
    pub(crate) const fn new(bytes: [u8; BLOCK_SIZE]) -> Self {
        Self(bytes)
    }

    /// Borrows the decrypted 128-bit title key.
    pub const fn as_bytes(&self) -> &[u8; BLOCK_SIZE] {
        &self.0
    }

    /// Consumes the wrapper and returns the decrypted 128-bit title key.
    pub fn into_bytes(self) -> [u8; BLOCK_SIZE] {
        self.0
    }
}

impl fmt::Debug for TitleKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TitleKey([REDACTED])")
    }
}

/// Decrypts one AES-128-CBC block without padding.
///
/// CBC for a single block is D(key, ciphertext) XOR IV. The title-key format
/// uses one complete 16-byte block, so no PKCS#7 or other padding is applied.
pub fn decrypt_aes128_cbc_block(
    key: [u8; BLOCK_SIZE],
    iv: [u8; BLOCK_SIZE],
    ciphertext: [u8; BLOCK_SIZE],
) -> [u8; BLOCK_SIZE] {
    let cipher = Aes128Decryptor::new(key);
    let mut plaintext = cipher.decrypt_block(ciphertext);
    for (byte, previous) in plaintext.iter_mut().zip(iv) {
        *byte ^= previous;
    }
    plaintext
}

struct Aes128Decryptor {
    round_keys: [[u8; BLOCK_SIZE]; ROUND_COUNT + 1],
}

impl Aes128Decryptor {
    fn new(key: [u8; BLOCK_SIZE]) -> Self {
        let mut expanded = [0u8; EXPANDED_KEY_SIZE];
        expanded[..BLOCK_SIZE].copy_from_slice(&key);

        let mut round_constant = 1u8;
        let mut word = 4usize;
        while word < 44 {
            let previous = (word - 1) * 4;
            let mut temporary = [
                expanded[previous],
                expanded[previous + 1],
                expanded[previous + 2],
                expanded[previous + 3],
            ];

            if word % 4 == 0 {
                temporary.rotate_left(1);
                for byte in &mut temporary {
                    *byte = SBOX[*byte as usize];
                }
                temporary[0] ^= round_constant;
                round_constant = gf_mul(round_constant, 2);
            }

            let destination = word * 4;
            let prior_round = (word - 4) * 4;
            let mut byte = 0usize;
            while byte < 4 {
                expanded[destination + byte] = expanded[prior_round + byte] ^ temporary[byte];
                byte += 1;
            }
            word += 1;
        }

        let mut round_keys = [[0u8; BLOCK_SIZE]; ROUND_COUNT + 1];
        let mut round = 0usize;
        while round <= ROUND_COUNT {
            let start = round * BLOCK_SIZE;
            round_keys[round].copy_from_slice(&expanded[start..start + BLOCK_SIZE]);
            round += 1;
        }

        Self { round_keys }
    }

    fn decrypt_block(&self, mut state: [u8; BLOCK_SIZE]) -> [u8; BLOCK_SIZE] {
        add_round_key(&mut state, &self.round_keys[ROUND_COUNT]);

        let mut round = ROUND_COUNT - 1;
        while round > 0 {
            inverse_shift_rows(&mut state);
            inverse_sub_bytes(&mut state);
            add_round_key(&mut state, &self.round_keys[round]);
            inverse_mix_columns(&mut state);
            round -= 1;
        }

        inverse_shift_rows(&mut state);
        inverse_sub_bytes(&mut state);
        add_round_key(&mut state, &self.round_keys[0]);
        state
    }
}

fn add_round_key(state: &mut [u8; BLOCK_SIZE], key: &[u8; BLOCK_SIZE]) {
    for (byte, key_byte) in state.iter_mut().zip(key) {
        *byte ^= key_byte;
    }
}

fn inverse_sub_bytes(state: &mut [u8; BLOCK_SIZE]) {
    for byte in state {
        *byte = INVERSE_SBOX[*byte as usize];
    }
}

fn inverse_shift_rows(state: &mut [u8; BLOCK_SIZE]) {
    let previous = *state;
    for row in 0..4 {
        for column in 0..4 {
            state[column * 4 + row] = previous[((column + 4 - row) % 4) * 4 + row];
        }
    }
}

fn inverse_mix_columns(state: &mut [u8; BLOCK_SIZE]) {
    for column in 0..4 {
        let offset = column * 4;
        let a = state[offset];
        let b = state[offset + 1];
        let c = state[offset + 2];
        let d = state[offset + 3];

        state[offset] = gf_mul(a, 14) ^ gf_mul(b, 11) ^ gf_mul(c, 13) ^ gf_mul(d, 9);
        state[offset + 1] = gf_mul(a, 9) ^ gf_mul(b, 14) ^ gf_mul(c, 11) ^ gf_mul(d, 13);
        state[offset + 2] = gf_mul(a, 13) ^ gf_mul(b, 9) ^ gf_mul(c, 14) ^ gf_mul(d, 11);
        state[offset + 3] = gf_mul(a, 11) ^ gf_mul(b, 13) ^ gf_mul(c, 9) ^ gf_mul(d, 14);
    }
}

/// A streaming AES-128-CBC decryptor for a bounded, block-aligned source.
///
/// The reader owns its source and retains only one decrypted block. Its caller
/// is responsible for bounding the source to the encrypted CIA content range.
pub struct Aes128CbcReader<R> {
    reader: R,
    cipher: Aes128Decryptor,
    previous_ciphertext: [u8; BLOCK_SIZE],
    plaintext_block: [u8; BLOCK_SIZE],
    plaintext_offset: usize,
    ciphertext_remaining: u64,
    failure: Option<io::ErrorKind>,
}

impl<R: Read> Aes128CbcReader<R> {
    /// Creates a CBC reader over exactly `ciphertext_size_bytes` bytes.
    ///
    /// CBC input must contain complete blocks. No padding is added or removed.
    pub fn new(
        reader: R,
        key: [u8; BLOCK_SIZE],
        iv: [u8; BLOCK_SIZE],
        ciphertext_size_bytes: u64,
    ) -> io::Result<Self> {
        if ciphertext_size_bytes % BLOCK_SIZE as u64 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "AES-CBC input size is not a multiple of 16 bytes",
            ));
        }

        Ok(Self {
            reader,
            cipher: Aes128Decryptor::new(key),
            previous_ciphertext: iv,
            plaintext_block: [0; BLOCK_SIZE],
            plaintext_offset: BLOCK_SIZE,
            ciphertext_remaining: ciphertext_size_bytes,
            failure: None,
        })
    }

    /// Returns the underlying encrypted source.
    pub fn into_inner(self) -> R {
        self.reader
    }

    fn load_next_block(&mut self) -> io::Result<bool> {
        if self.ciphertext_remaining == 0 {
            return Ok(false);
        }

        let mut ciphertext = [0u8; BLOCK_SIZE];
        if let Err(error) = self.reader.read_exact(&mut ciphertext) {
            self.failure = Some(error.kind());
            return Err(error);
        }

        self.ciphertext_remaining -= BLOCK_SIZE as u64;
        self.plaintext_block = self.cipher.decrypt_block(ciphertext);
        for (byte, previous) in self
            .plaintext_block
            .iter_mut()
            .zip(self.previous_ciphertext)
        {
            *byte ^= previous;
        }
        self.previous_ciphertext = ciphertext;
        self.plaintext_offset = 0;
        Ok(true)
    }
}

impl<R: Read> Read for Aes128CbcReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }

        if let Some(kind) = self.failure {
            return Err(io::Error::new(
                kind,
                "AES-CBC source previously failed while reading a block",
            ));
        }

        let mut written = 0usize;
        while written < output.len() {
            if self.plaintext_offset == BLOCK_SIZE {
                match self.load_next_block() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        return if written == 0 {
                            Err(error)
                        } else {
                            Ok(written)
                        };
                    }
                }
            }

            let available = BLOCK_SIZE - self.plaintext_offset;
            let requested = (output.len() - written).min(available);
            let block_end = self.plaintext_offset + requested;
            output[written..written + requested]
                .copy_from_slice(&self.plaintext_block[self.plaintext_offset..block_end]);
            self.plaintext_offset = block_end;
            written += requested;
        }

        Ok(written)
    }
}

/// Builds the outer CIA AES-CBC IV for a TMD content index.
///
/// The big-endian 16-bit index occupies the first two bytes; the rest is zero.
pub const fn cia_content_iv(content_index: u16) -> [u8; BLOCK_SIZE] {
    let index = content_index.to_be_bytes();
    [index[0], index[1], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_hex<const N: usize>(hex: &str) -> [u8; N] {
        assert_eq!(hex.len(), N * 2);
        let mut bytes = [0u8; N];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
        }
        bytes
    }

    #[test]
    fn decrypts_the_fips_197_aes128_known_answer_block() {
        let key = decode_hex::<16>("000102030405060708090a0b0c0d0e0f");
        let ciphertext = decode_hex::<16>("69c4e0d86a7b0430d8cdb78070b4c55a");
        let expected = decode_hex::<16>("00112233445566778899aabbccddeeff");

        let plaintext = Aes128Decryptor::new(key).decrypt_block(ciphertext);

        assert_eq!(plaintext, expected);
    }

    #[test]
    fn decrypts_the_nist_sp800_38a_single_cbc_block() {
        let key = decode_hex::<16>("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = decode_hex::<16>("000102030405060708090a0b0c0d0e0f");
        let ciphertext = decode_hex::<16>("7649abac8119b246cee98e9b12e9197d");
        let expected = decode_hex::<16>("6bc1bee22e409f96e93d7e117393172a");

        assert_eq!(decrypt_aes128_cbc_block(key, iv, ciphertext), expected);
    }

    #[test]
    fn streams_multiple_cbc_blocks_from_short_reads() {
        struct ShortReader {
            bytes: std::io::Cursor<Vec<u8>>,
            maximum_read: usize,
        }

        impl Read for ShortReader {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                let limit = output.len().min(self.maximum_read);
                self.bytes.read(&mut output[..limit])
            }
        }

        let key = decode_hex::<16>("2b7e151628aed2a6abf7158809cf4f3c");
        let iv = decode_hex::<16>("000102030405060708090a0b0c0d0e0f");
        let ciphertext =
            decode_hex::<32>("7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2");
        let expected =
            decode_hex::<32>("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        let source = ShortReader {
            bytes: std::io::Cursor::new(ciphertext.to_vec()),
            maximum_read: 3,
        };
        let mut reader = Aes128CbcReader::new(source, key, iv, ciphertext.len() as u64).unwrap();
        let mut actual = Vec::new();

        reader.read_to_end(&mut actual).unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn rejects_unaligned_and_truncated_cbc_sources() {
        let key = [0x11; BLOCK_SIZE];
        let iv = [0x22; BLOCK_SIZE];
        assert_eq!(
            Aes128CbcReader::new(std::io::Cursor::new(vec![0; 17]), key, iv, 17)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        let mut reader =
            Aes128CbcReader::new(std::io::Cursor::new(vec![0; 15]), key, iv, 16).unwrap();
        let mut block = [0; BLOCK_SIZE];
        assert_eq!(
            reader.read(&mut block).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            reader.read(&mut block).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn creates_the_content_iv_from_the_big_endian_tmd_index() {
        assert_eq!(
            cia_content_iv(0x1234),
            [0x12, 0x34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn title_key_debug_output_is_redacted() {
        let title_key = TitleKey([0xA5; BLOCK_SIZE]);

        assert_eq!(format!("{title_key:?}"), "TitleKey([REDACTED])");
        assert_eq!(title_key.as_bytes(), &[0xA5; BLOCK_SIZE]);
    }
}
