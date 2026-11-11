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
    CounterRangeOverflow,
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
            Self::CounterRangeOverflow => {
                write!(f, "AES-CTR request exceeds the 128-bit counter range")
            }
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
    counter_exhausted: bool,
}

impl Aes128Ctr {
    pub fn new(key: [u8; AES_BLOCK_SIZE], initial_counter: [u8; AES_BLOCK_SIZE]) -> Self {
        Self {
            cipher: Aes128::new(key),
            counter: initial_counter,
            keystream_block: [0; AES_BLOCK_SIZE],
            keystream_offset: AES_BLOCK_SIZE,
            counter_exhausted: false,
        }
    }

    /// XORs the next bytes of the CTR keystream into data.
    ///
    /// Repeated calls continue at the exact byte position reached previously,
    /// including calls which end in the middle of a block. The complete request
    /// is checked before the buffer or stream state changes. The final counter
    /// block can be consumed once; attempting to generate another block returns
    /// `CounterRangeOverflow` instead of reusing an earlier keystream block.
    pub fn apply_keystream(&mut self, data: &mut [u8]) -> Result<(), NcchCryptoError> {
        let available = AES_BLOCK_SIZE - self.keystream_offset;
        let bytes_needing_blocks = data.len().saturating_sub(available);
        let required_blocks = bytes_needing_blocks.div_ceil(AES_BLOCK_SIZE);

        if required_blocks != 0 {
            if self.counter_exhausted {
                return Err(NcchCryptoError::CounterRangeOverflow);
            }
            let final_block_offset = (required_blocks - 1) as u128;
            u128::from_be_bytes(self.counter)
                .checked_add(final_block_offset)
                .ok_or(NcchCryptoError::CounterRangeOverflow)?;
        }

        for byte in data {
            if self.keystream_offset == AES_BLOCK_SIZE {
                self.keystream_block = self.cipher.encrypt_block(self.counter);
                if self.counter == [u8::MAX; AES_BLOCK_SIZE] {
                    self.counter_exhausted = true;
                } else {
                    increment_counter(&mut self.counter);
                }
                self.keystream_offset = 0;
            }
            *byte ^= self.keystream_block[self.keystream_offset];
            self.keystream_offset += 1;
        }
        Ok(())
    }

    /// Applies CTR at an arbitrary byte offset without processing prior data.
    ///
    /// The requested byte range is rejected if its final block would wrap the
    /// 128-bit counter and reuse an earlier keystream block.
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
        if !data.is_empty() {
            let final_block = (remainder as u128 + data.len() as u128 - 1) / AES_BLOCK_SIZE as u128;
            counter_value
                .checked_add(final_block)
                .ok_or(NcchCryptoError::CounterRangeOverflow)?;
        } else {
            return Ok(());
        }
        let mut stream = Self::new(key, counter_value.to_be_bytes());

        if remainder != 0 {
            let mut discard = [0; AES_BLOCK_SIZE];
            stream.apply_keystream(&mut discard[..remainder])?;
        }
        stream.apply_keystream(data)?;
        Ok(())
    }
}

/// Errors while opening a bounded NCCH AES-CTR region reader.
#[derive(Debug)]
pub enum NcchCtrReaderError {
    ContentAlreadyClear,
    EmptyRegion,
    RegionRangeOverflow,
    RegionOutOfBounds {
        end_bytes: u64,
        available_bytes: u64,
    },
    Crypto(NcchCryptoError),
    ReaderFailure(std::io::Error),
}

impl std::fmt::Display for NcchCtrReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContentAlreadyClear => f.write_str("NCCH content is marked as already clear"),
            Self::EmptyRegion => f.write_str("NCCH encrypted region has zero length"),
            Self::RegionRangeOverflow => f.write_str("NCCH encrypted region range overflows"),
            Self::RegionOutOfBounds {
                end_bytes,
                available_bytes,
            } => write!(
                f,
                "NCCH encrypted region ends at 0x{end_bytes:x}, beyond reader length 0x{available_bytes:x}"
            ),
            Self::Crypto(error) => write!(f, "NCCH AES-CTR setup failed: {error}"),
            Self::ReaderFailure(error) => write!(f, "NCCH encrypted region reader failed: {error}"),
        }
    }
}

impl std::error::Error for NcchCtrReaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Crypto(error) => Some(error),
            Self::ReaderFailure(error) => Some(error),
            _ => None,
        }
    }
}

/// A seekable, decrypting view of one NCCH AES-CTR region.
///
/// The reader exposes only the selected region, with byte zero corresponding
/// to the first byte of that region. The caller supplies the AES-128 content
/// key and the region's initial counter; no console keys are embedded here.
/// Random seeks reset the CTR stream to the matching byte position.
pub struct NcchCtrReader<R> {
    inner: R,
    start: u64,
    size: u64,
    position: u64,
    key: [u8; AES_BLOCK_SIZE],
    initial_counter: [u8; AES_BLOCK_SIZE],
    stream: Option<Aes128Ctr>,
}

impl<R: std::io::Read + std::io::Seek> NcchCtrReader<R> {
    /// Creates a decrypting reader for `[region_offset, region_offset + region_size)`.
    ///
    /// The complete region is checked against both the underlying reader and
    /// the 128-bit CTR counter range before the reader is returned.
    pub fn new(
        mut inner: R,
        region_offset: u64,
        region_size: u64,
        key: [u8; AES_BLOCK_SIZE],
        initial_counter: [u8; AES_BLOCK_SIZE],
    ) -> Result<Self, NcchCtrReaderError> {
        if region_size == 0 {
            return Err(NcchCtrReaderError::EmptyRegion);
        }

        let end = region_offset
            .checked_add(region_size)
            .ok_or(NcchCtrReaderError::RegionRangeOverflow)?;
        let available = inner
            .seek(std::io::SeekFrom::End(0))
            .map_err(NcchCtrReaderError::ReaderFailure)?;
        if end > available {
            return Err(NcchCtrReaderError::RegionOutOfBounds {
                end_bytes: end,
                available_bytes: available,
            });
        }

        let final_block = u128::from((region_size - 1) / AES_BLOCK_SIZE as u64);
        u128::from_be_bytes(initial_counter)
            .checked_add(final_block)
            .ok_or(NcchCtrReaderError::Crypto(
                NcchCryptoError::CounterRangeOverflow,
            ))?;

        let actual = inner
            .seek(std::io::SeekFrom::Start(region_offset))
            .map_err(NcchCtrReaderError::ReaderFailure)?;
        if actual != region_offset {
            return Err(NcchCtrReaderError::ReaderFailure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "underlying reader did not seek to the NCCH region",
            )));
        }

        Ok(Self {
            inner,
            start: region_offset,
            size: region_size,
            position: 0,
            key,
            initial_counter,
            stream: Some(Aes128Ctr::new(key, initial_counter)),
        })
    }

    /// Returns the visible length of the decrypted region.
    pub const fn len(&self) -> u64 {
        self.size
    }

    /// Returns true when the region has no bytes.
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Returns the current position relative to the region start.
    pub const fn position(&self) -> u64 {
        self.position
    }
}

impl<R: std::io::Read + std::io::Seek> std::io::Read for NcchCtrReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() || self.position == self.size {
            return Ok(0);
        }

        let remaining = self.size - self.position;
        let readable = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let bytes_read = self.inner.read(&mut buffer[..readable])?;
        if bytes_read == 0 {
            return Ok(0);
        }

        let stream = self.stream.as_mut().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "CTR stream is unavailable before the end of the region",
            )
        })?;
        if let Err(error) = stream.apply_keystream(&mut buffer[..bytes_read]) {
            let absolute = self.start.checked_add(self.position).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "NCCH reader position overflows",
                )
            })?;
            let _ = self.inner.seek(std::io::SeekFrom::Start(absolute));
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }

        self.position += bytes_read as u64;
        Ok(bytes_read)
    }
}

impl<R: std::io::Read + std::io::Seek> std::io::Seek for NcchCtrReader<R> {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        let target = match from {
            std::io::SeekFrom::Start(offset) => i128::from(offset),
            std::io::SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            std::io::SeekFrom::End(offset) => i128::from(self.size) + i128::from(offset),
        };
        if !(0..=i128::from(self.size)).contains(&target) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek is outside the NCCH encrypted region",
            ));
        }

        let relative = u64::try_from(target).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "relative NCCH region position is invalid",
            )
        })?;
        let absolute = self.start.checked_add(relative).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "absolute NCCH region position overflows",
            )
        })?;

        let next_stream = if relative == self.size {
            None
        } else {
            Some(
                stream_at_offset(self.key, self.initial_counter, relative).map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
                })?,
            )
        };

        let actual = self.inner.seek(std::io::SeekFrom::Start(absolute))?;
        if actual != absolute {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "underlying reader did not seek to the requested NCCH position",
            ));
        }

        self.position = relative;
        self.stream = next_stream;
        Ok(relative)
    }
}

fn stream_at_offset(
    key: [u8; AES_BLOCK_SIZE],
    initial_counter: [u8; AES_BLOCK_SIZE],
    byte_offset: u64,
) -> Result<Aes128Ctr, NcchCryptoError> {
    let block_offset = u128::from(byte_offset / AES_BLOCK_SIZE as u64);
    let remainder = (byte_offset % AES_BLOCK_SIZE as u64) as usize;
    let counter = u128::from_be_bytes(initial_counter)
        .checked_add(block_offset)
        .ok_or(NcchCryptoError::CounterOffsetOverflow)?;
    let mut stream = Aes128Ctr::new(key, counter.to_be_bytes());

    if remainder != 0 {
        let mut discard = [0; AES_BLOCK_SIZE];
        stream.apply_keystream(&mut discard[..remainder])?;
    }
    Ok(stream)
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
        Aes128Ctr::new(key, counter)
            .apply_keystream(&mut data)
            .unwrap();
        assert_eq!(data, expected);

        Aes128Ctr::new(key, counter)
            .apply_keystream(&mut data)
            .unwrap();
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
        Aes128Ctr::new(key, counter)
            .apply_keystream(&mut expected)
            .unwrap();

        let mut actual = plaintext;
        let mut stream = Aes128Ctr::new(key, counter);
        stream.apply_keystream(&mut actual[..3]).unwrap();
        stream.apply_keystream(&mut actual[3..19]).unwrap();
        stream.apply_keystream(&mut actual[19..]).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn ctr_random_access_matches_sequential_processing() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let mut sequential = [0u8; 73];
        Aes128Ctr::new(key, counter)
            .apply_keystream(&mut sequential)
            .unwrap();
        let mut random_access = [0u8; 37];
        Aes128Ctr::apply_keystream_at_offset(key, counter, 21, &mut random_access).unwrap();
        assert_eq!(&random_access[..], &sequential[21..58]);
    }

    #[test]
    fn random_access_rejects_counter_wrap_without_mutating_input() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = [0xff; AES_BLOCK_SIZE];

        let mut final_full_block = [0; AES_BLOCK_SIZE];
        Aes128Ctr::apply_keystream_at_offset(key, counter, 0, &mut final_full_block).unwrap();
        assert_eq!(final_full_block, Aes128::new(key).encrypt_block(counter));

        let mut crossing_block_boundary = [0x5a; 17];
        assert_eq!(
            Aes128Ctr::apply_keystream_at_offset(key, counter, 0, &mut crossing_block_boundary),
            Err(NcchCryptoError::CounterRangeOverflow)
        );
        assert_eq!(crossing_block_boundary, [0x5a; 17]);

        let mut unaligned_crossing = [0xa5; 2];
        assert_eq!(
            Aes128Ctr::apply_keystream_at_offset(key, counter, 15, &mut unaligned_crossing),
            Err(NcchCryptoError::CounterRangeOverflow)
        );
        assert_eq!(unaligned_crossing, [0xa5; 2]);
    }

    #[test]
    fn ctr_stream_rejects_counter_wrap_without_mutating_input_or_state() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = [u8::MAX; AES_BLOCK_SIZE];
        let mut stream = Aes128Ctr::new(key, counter);

        let mut crossing = [0x5a; 17];
        assert_eq!(
            stream.apply_keystream(&mut crossing),
            Err(NcchCryptoError::CounterRangeOverflow)
        );
        assert_eq!(crossing, [0x5a; 17]);

        let mut final_block = [0; AES_BLOCK_SIZE];
        stream.apply_keystream(&mut final_block).unwrap();
        assert_eq!(final_block, Aes128::new(key).encrypt_block(counter));

        let mut after_exhaustion = [0xa5; 1];
        assert_eq!(
            stream.apply_keystream(&mut after_exhaustion),
            Err(NcchCryptoError::CounterRangeOverflow)
        );
        assert_eq!(after_exhaustion, [0xa5; 1]);
    }

    #[test]
    fn ctr_stream_can_finish_cached_bytes_of_the_final_counter_block() {
        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = [u8::MAX; AES_BLOCK_SIZE];
        let mut stream = Aes128Ctr::new(key, counter);
        let mut actual = [0; AES_BLOCK_SIZE];

        stream.apply_keystream(&mut actual[..5]).unwrap();
        stream.apply_keystream(&mut actual[5..]).unwrap();
        assert_eq!(actual, Aes128::new(key).encrypt_block(counter));

        let mut empty = [0u8; 0];
        stream.apply_keystream(&mut empty).unwrap();

        let mut next = [0x3c; 1];
        assert_eq!(
            stream.apply_keystream(&mut next),
            Err(NcchCryptoError::CounterRangeOverflow)
        );
        assert_eq!(next, [0x3c; 1]);
    }

    #[test]
    fn ncch_ctr_reader_decrypts_region_and_supports_unaligned_seeks() {
        use std::io::{Cursor, Read, Seek, SeekFrom};

        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = hex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff");
        let plaintext: Vec<u8> = (0..73).map(|value| (value * 11) as u8).collect();
        let mut ciphertext = plaintext.clone();
        Aes128Ctr::new(key, counter)
            .apply_keystream(&mut ciphertext)
            .unwrap();

        let mut container = vec![0xA5; 13];
        container.extend_from_slice(&ciphertext);
        container.extend_from_slice(&[0x5A; 9]);

        let mut reader = NcchCtrReader::new(
            Cursor::new(container),
            13,
            ciphertext.len() as u64,
            key,
            counter,
        )
        .unwrap();
        assert_eq!(reader.len(), plaintext.len() as u64);

        let mut decoded = Vec::new();
        reader.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, plaintext);
        assert_eq!(reader.position(), plaintext.len() as u64);

        reader.seek(SeekFrom::Start(7)).unwrap();
        let mut middle = [0; 19];
        reader.read_exact(&mut middle).unwrap();
        assert_eq!(&middle, &plaintext[7..26]);

        reader.seek(SeekFrom::End(-5)).unwrap();
        let mut tail = [0; 5];
        reader.read_exact(&mut tail).unwrap();
        assert_eq!(&tail, &plaintext[plaintext.len() - 5..]);

        assert!(
            reader
                .seek(SeekFrom::Start(plaintext.len() as u64 + 1))
                .is_err()
        );
    }

    #[test]
    fn ncch_ctr_reader_rejects_truncated_and_counter_wrapping_regions() {
        use std::io::Cursor;

        let key = hex("000102030405060708090a0b0c0d0e0f");
        let counter = hex("00000000000000000000000000000000");
        assert!(matches!(
            NcchCtrReader::new(Cursor::new(vec![0; 16]), 8, 9, key, counter),
            Err(NcchCtrReaderError::RegionOutOfBounds {
                end_bytes: 17,
                available_bytes: 16
            })
        ));

        assert!(matches!(
            NcchCtrReader::new(
                Cursor::new(vec![0; 17]),
                0,
                17,
                key,
                [u8::MAX; AES_BLOCK_SIZE]
            ),
            Err(NcchCtrReaderError::Crypto(
                NcchCryptoError::CounterRangeOverflow
            ))
        ));
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
            hex("07060504030201000100000000000000")
        );
        assert_eq!(
            initial_ctr(id, 2, NcchCtrRegion::RomFs, 0x123400).unwrap(),
            hex("07060504030201000300000000000000")
        );
        assert_eq!(
            initial_ctr(id, 2, NcchCtrRegion::ExeFs, 0).unwrap(),
            hex("07060504030201000200000000000000")
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
        Aes128Ctr::new(key, counter)
            .apply_keystream(&mut bytes)
            .unwrap();
        let first = Aes128::new(key).encrypt_block(counter);
        let second = Aes128::new(key).encrypt_block(hex("00000000000000000000000000000100"));
        let mut expected = [0; 32];
        expected[..16].copy_from_slice(&first);
        expected[16..].copy_from_slice(&second);
        assert_eq!(bytes, expected);
    }
}
