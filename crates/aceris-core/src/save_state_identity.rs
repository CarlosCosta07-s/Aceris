//! Exact boot-image identity for save-state isolation.
//!
//! The digest covers the complete byte stream of the selected image. It does
//! not depend on its filename or path. This intentionally treats repacked or
//! otherwise byte-different images as different identities, even when their
//! guest program happens to be equivalent.

use std::{
    fmt,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

use crate::save_state::SaveStateIdentity;

const SHA256_INITIAL_STATE: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

const SHA256_ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// Errors that prevent a stable save-state image identity from being created.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveStateIdentityError {
    InvalidTitleId,
    EmptyImage,
    ImageLengthOverflow,
    Read(io::ErrorKind),
    PositionRestore(io::ErrorKind),
}

impl fmt::Display for SaveStateIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTitleId => formatter.write_str("Title ID must be non-zero when present"),
            Self::EmptyImage => formatter.write_str("boot image is empty"),
            Self::ImageLengthOverflow => formatter.write_str("boot image is too large to hash"),
            Self::Read(kind) => write!(formatter, "could not read boot image: {kind:?}"),
            Self::PositionRestore(kind) => {
                write!(
                    formatter,
                    "could not restore image reader position: {kind:?}"
                )
            }
        }
    }
}

impl std::error::Error for SaveStateIdentityError {}

/// Computes a save-state identity from a complete image file.
///
/// The entire file is hashed in bounded memory. Its path and display name are
/// never incorporated. The optional Title ID is metadata in addition to the
/// digest; a zero Title ID is rejected instead of silently treated as absent.
pub fn identity_from_image_path(
    path: &Path,
    title_id: Option<u64>,
) -> Result<SaveStateIdentity, SaveStateIdentityError> {
    let mut file = File::open(path).map_err(|error| SaveStateIdentityError::Read(error.kind()))?;
    identity_from_image_reader(&mut file, title_id)
}

/// Computes an identity from byte zero to EOF and restores the reader's cursor.
///
/// The caller can therefore inspect the file first, calculate its identity,
/// and continue loading it without reopening or repositioning it manually.
pub fn identity_from_image_reader<R: Read + Seek>(
    reader: &mut R,
    title_id: Option<u64>,
) -> Result<SaveStateIdentity, SaveStateIdentityError> {
    if title_id == Some(0) {
        return Err(SaveStateIdentityError::InvalidTitleId);
    }

    let original_position = reader
        .stream_position()
        .map_err(|error| SaveStateIdentityError::Read(error.kind()))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| SaveStateIdentityError::Read(error.kind()))?;

    let digest_result = sha256_reader(reader);
    let restore_result = reader.seek(SeekFrom::Start(original_position));
    let digest = match (digest_result, restore_result) {
        (Err(error), _) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(SaveStateIdentityError::PositionRestore(error.kind()));
        }
        (Ok(digest), Ok(_)) => digest,
    };

    Ok(SaveStateIdentity {
        title_id,
        content_sha256: digest,
    })
}

/// Streams a SHA-256 digest without loading the entire image into memory.
fn sha256_reader<R: Read>(reader: &mut R) -> Result<[u8; 32], SaveStateIdentityError> {
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read_any = false;

    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(SaveStateIdentityError::Read(error.kind())),
        };
        read_any = true;
        hasher.update(&buffer[..count])?;
    }

    if !read_any {
        return Err(SaveStateIdentityError::EmptyImage);
    }
    hasher.finish()
}

struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    block_len: usize,
    total_bytes: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: SHA256_INITIAL_STATE,
            block: [0; 64],
            block_len: 0,
            total_bytes: 0,
        }
    }

    fn update(&mut self, mut bytes: &[u8]) -> Result<(), SaveStateIdentityError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(SaveStateIdentityError::ImageLengthOverflow)?;

        if self.block_len != 0 {
            let count = (64 - self.block_len).min(bytes.len());
            self.block[self.block_len..self.block_len + count].copy_from_slice(&bytes[..count]);
            self.block_len += count;
            bytes = &bytes[count..];

            if self.block_len == 64 {
                let block = self.block;
                self.compress(&block);
                self.block_len = 0;
            }
        }

        while bytes.len() >= 64 {
            let block: &[u8; 64] = bytes[..64]
                .try_into()
                .expect("a full SHA-256 block has exactly 64 bytes");
            self.compress(block);
            bytes = &bytes[64..];
        }

        if !bytes.is_empty() {
            self.block[..bytes.len()].copy_from_slice(bytes);
            self.block_len = bytes.len();
        }
        Ok(())
    }

    fn finish(mut self) -> Result<[u8; 32], SaveStateIdentityError> {
        let bit_length = self
            .total_bytes
            .checked_mul(8)
            .ok_or(SaveStateIdentityError::ImageLengthOverflow)?;

        self.block[self.block_len] = 0x80;
        self.block_len += 1;

        if self.block_len > 56 {
            self.block[self.block_len..].fill(0);
            let block = self.block;
            self.compress(&block);
            self.block = [0; 64];
            self.block_len = 0;
        } else {
            self.block[self.block_len..56].fill(0);
        }

        self.block[56..64].copy_from_slice(&bit_length.to_be_bytes());
        let block = self.block;
        self.compress(&block);

        let mut digest = [0u8; 32];
        for (chunk, word) in digest.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        Ok(digest)
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut schedule = [0u32; 64];
        for (index, word) in schedule[..16].iter_mut().enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                block[offset],
                block[offset + 1],
                block[offset + 2],
                block[offset + 3],
            ]);
        }

        for index in 16..64 {
            let x = schedule[index - 15];
            let y = schedule[index - 2];
            let small_sigma_zero = x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3);
            let small_sigma_one = y.rotate_right(17) ^ y.rotate_right(19) ^ (y >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(small_sigma_zero)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(small_sigma_one);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let big_sigma_one = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let temp_one = h
                .wrapping_add(big_sigma_one)
                .wrapping_add(choose)
                .wrapping_add(SHA256_ROUND_CONSTANTS[index])
                .wrapping_add(schedule[index]);
            let big_sigma_zero = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp_two = big_sigma_zero.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp_one);
            d = c;
            c = b;
            b = a;
            a = temp_one.wrapping_add(temp_two);
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
    use std::io::Cursor;

    fn decode_hex(value: &str) -> [u8; 32] {
        let mut output = [0u8; 32];
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .expect("test digest should be hexadecimal");
        }
        output
    }

    fn digest(bytes: &[u8]) -> Result<[u8; 32], SaveStateIdentityError> {
        sha256_reader(&mut Cursor::new(bytes))
    }

    #[test]
    fn sha256_matches_standard_test_vectors() {
        assert_eq!(
            digest(b"abc").unwrap(),
            decode_hex("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert_eq!(
            digest(b"The quick brown fox jumps over the lazy dog").unwrap(),
            decode_hex("d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592")
        );
    }

    #[test]
    fn sha256_handles_padding_boundaries() {
        let vectors = [
            (
                55,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            (
                63,
                "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34",
            ),
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                65,
                "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
            ),
        ];

        for (length, expected) in vectors {
            let image = vec![b'a'; length];
            assert_eq!(
                digest(&image).unwrap(),
                decode_hex(expected),
                "incorrect SHA-256 digest for {length} bytes"
            );
        }
    }

    #[test]
    fn sha256_streams_large_images_across_many_blocks() {
        let image = vec![b'a'; 1_000_000];
        assert_eq!(
            digest(&image).unwrap(),
            decode_hex("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
        );
    }

    #[test]
    fn identity_hashes_the_whole_image_and_restores_the_reader_position() {
        let mut reader = Cursor::new(b"synthetic boot image".to_vec());
        reader.set_position(7);
        let identity = identity_from_image_reader(&mut reader, Some(0x0004_0000_0012_3400))
            .expect("image identity should be calculated");
        assert_eq!(reader.position(), 7);
        assert_eq!(
            identity,
            SaveStateIdentity {
                title_id: Some(0x0004_0000_0012_3400),
                content_sha256: decode_hex(
                    "3d6f4b4cf86576178d6557e77f87d809a9dd4ee96df7d61297e42c8dfc3c21f3"
                )
            }
        );
    }

    #[test]
    fn identical_images_share_identity_independent_of_path() {
        let bytes = b"same synthetic image bytes";
        let first = identity_from_image_reader(&mut Cursor::new(bytes), None).unwrap();
        let second = identity_from_image_reader(&mut Cursor::new(bytes), None).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn different_images_have_different_identities() {
        let first = identity_from_image_reader(&mut Cursor::new(b"image one"), None).unwrap();
        let second = identity_from_image_reader(&mut Cursor::new(b"image two"), None).unwrap();
        assert_ne!(first.content_sha256, second.content_sha256);
    }

    #[test]
    fn invalid_title_id_and_empty_image_are_rejected() {
        assert_eq!(
            identity_from_image_reader(&mut Cursor::new(b"image"), Some(0)),
            Err(SaveStateIdentityError::InvalidTitleId)
        );
        assert_eq!(
            identity_from_image_reader(&mut Cursor::new(b""), None),
            Err(SaveStateIdentityError::EmptyImage)
        );
    }
}
