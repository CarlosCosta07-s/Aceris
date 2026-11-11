//! A dependency-free PNG encoder.
//!
//! Pixels are filtered row by row, then compressed with a compact DEFLATE
//! implementation (LZ77 matching plus the fixed Huffman code).

/// Encodes 8-bit RGBA pixels as a PNG file.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * 4,
        "pixel buffer does not match the image size"
    );

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, no interlace
    write_chunk(&mut png, *b"IHDR", &header);

    let filtered = filter_rows(width as usize * 4, rgba);
    let mut zlib = vec![0x78, 0x01];
    zlib.extend_from_slice(&deflate(&filtered));
    zlib.extend_from_slice(&adler32(&filtered).to_be_bytes());
    write_chunk(&mut png, *b"IDAT", &zlib);

    write_chunk(&mut png, *b"IEND", &[]);
    png
}

fn write_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let checksum = crc32(&out[start..]);
    out.extend_from_slice(&checksum.to_be_bytes());
}

/// Prefixes each row with the filter (none, sub or up) that leaves the
/// smallest residual, which helps DEFLATE on smooth gradients.
fn filter_rows(stride: usize, pixels: &[u8]) -> Vec<u8> {
    const BYTES_PER_PIXEL: usize = 4;

    let mut out = Vec::with_capacity(pixels.len() + pixels.len() / stride);
    let zero_row = vec![0u8; stride];

    for (index, row) in pixels.chunks_exact(stride).enumerate() {
        let above = if index == 0 {
            &zero_row[..]
        } else {
            &pixels[(index - 1) * stride..index * stride]
        };

        let sub: Vec<u8> = (0..stride)
            .map(|i| {
                let left = if i >= BYTES_PER_PIXEL {
                    row[i - BYTES_PER_PIXEL]
                } else {
                    0
                };
                row[i].wrapping_sub(left)
            })
            .collect();
        let up: Vec<u8> = row
            .iter()
            .zip(above)
            .map(|(a, b)| a.wrapping_sub(*b))
            .collect();

        let cost = |bytes: &[u8]| -> u64 {
            bytes
                .iter()
                .map(|&b| u64::from((b as i8).unsigned_abs()))
                .sum()
        };
        let (kind, filtered): (u8, &[u8]) = [(0, row), (1, &sub[..]), (2, &up[..])]
            .into_iter()
            .min_by_key(|(_, bytes)| cost(bytes))
            .unwrap_or((0, row));

        out.push(kind);
        out.extend_from_slice(filtered);
    }
    out
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

pub fn adler32(data: &[u8]) -> u32 {
    const MODULUS: u32 = 65_521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % MODULUS;
        b = (b + a) % MODULUS;
    }
    (b << 16) | a
}

// ----- DEFLATE (RFC 1951), fixed Huffman codes ---------------------------------

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

const WINDOW: usize = 32_768;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MAX_CHAIN: usize = 48;
const HASH_BITS: usize = 15;

/// Packs bits starting from the least significant bit of each byte.
struct BitWriter {
    bytes: Vec<u8>,
    accumulator: u32,
    count: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            accumulator: 0,
            count: 0,
        }
    }

    /// Writes `count` bits of `value`, least significant bit first.
    fn bits(&mut self, value: u32, count: u32) {
        self.accumulator |= value << self.count;
        self.count += count;
        while self.count >= 8 {
            self.bytes.push(self.accumulator as u8);
            self.accumulator >>= 8;
            self.count -= 8;
        }
    }

    /// Writes a Huffman code, which is packed most significant bit first.
    fn code(&mut self, code: u32, length: u32) {
        self.bits(code.reverse_bits() >> (32 - length), length);
    }

    /// Writes a literal/length symbol using the fixed code.
    fn symbol(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0x30 + symbol, 8),
            144..=255 => self.code(0x190 + symbol - 144, 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0xC0 + symbol - 280, 8),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.count > 0 {
            self.bytes.push(self.accumulator as u8);
        }
        self.bytes
    }
}

fn hash(data: &[u8], at: usize) -> usize {
    let value = (usize::from(data[at]) << 10)
        ^ (usize::from(data[at + 1]) << 5)
        ^ usize::from(data[at + 2]);
    value & ((1 << HASH_BITS) - 1)
}

/// Records position `at` in the hash chains so later bytes can match it.
fn insert(data: &[u8], head: &mut [usize], previous: &mut [usize], at: usize) {
    if at + MIN_MATCH <= data.len() {
        let slot = hash(data, at);
        previous[at] = head[slot];
        head[slot] = at;
    }
}

/// Compresses `data` into a single fixed-Huffman DEFLATE block.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.bits(1, 1); // final block
    writer.bits(1, 2); // fixed Huffman codes

    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut previous = vec![usize::MAX; data.len()];

    let mut at = 0;
    while at < data.len() {
        let (mut best_length, mut best_distance) = (0usize, 0usize);
        if at + MIN_MATCH <= data.len() {
            let longest = (data.len() - at).min(MAX_MATCH);
            let mut candidate = head[hash(data, at)];
            let mut chain = 0;
            while candidate != usize::MAX && at - candidate <= WINDOW && chain < MAX_CHAIN {
                let mut length = 0;
                while length < longest && data[candidate + length] == data[at + length] {
                    length += 1;
                }
                if length > best_length {
                    best_length = length;
                    best_distance = at - candidate;
                    if length == longest {
                        break;
                    }
                }
                candidate = previous[candidate];
                chain += 1;
            }
        }

        if best_length >= MIN_MATCH {
            let length_index = LENGTH_BASE
                .iter()
                .rposition(|&base| usize::from(base) <= best_length)
                .unwrap_or(0);
            writer.symbol(257 + length_index as u32);
            writer.bits(
                (best_length - usize::from(LENGTH_BASE[length_index])) as u32,
                u32::from(LENGTH_EXTRA[length_index]),
            );

            let distance_index = DISTANCE_BASE
                .iter()
                .rposition(|&base| usize::from(base) <= best_distance)
                .unwrap_or(0);
            writer.code(distance_index as u32, 5);
            writer.bits(
                (best_distance - usize::from(DISTANCE_BASE[distance_index])) as u32,
                u32::from(DISTANCE_EXTRA[distance_index]),
            );

            for position in at..at + best_length {
                insert(data, &mut head, &mut previous, position);
            }
            at += best_length;
        } else {
            writer.symbol(u32::from(data[at]));
            insert(data, &mut head, &mut previous, at);
            at += 1;
        }
    }

    writer.symbol(256); // end of block
    writer.finish()
}

#[cfg(test)]
pub(crate) mod inflate {
    //! A minimal decoder for the blocks `deflate` produces, used to check it.

    struct BitReader<'a> {
        data: &'a [u8],
        position: usize,
    }

    impl BitReader<'_> {
        fn bit(&mut self) -> u32 {
            let byte = self.data[self.position / 8];
            let bit = (byte >> (self.position % 8)) & 1;
            self.position += 1;
            u32::from(bit)
        }

        fn bits(&mut self, count: u32) -> u32 {
            (0..count).fold(0, |value, i| value | (self.bit() << i))
        }

        fn huffman(&mut self, length: u32) -> u32 {
            (0..length).fold(0, |code, _| (code << 1) | self.bit())
        }

        fn symbol(&mut self) -> u32 {
            let mut code = 0;
            for length in 1..=9 {
                code = (code << 1) | self.bit();
                match (length, code) {
                    (7, 0..=0x17) => return 256 + code,
                    (8, 0x30..=0xBF) => return code - 0x30,
                    (8, 0xC0..=0xC7) => return 280 + code - 0xC0,
                    (9, 0x190..=0x1FF) => return 144 + code - 0x190,
                    _ => {}
                }
            }
            panic!("invalid fixed Huffman code");
        }
    }

    pub fn decode(compressed: &[u8]) -> Vec<u8> {
        use super::{DISTANCE_BASE, DISTANCE_EXTRA, LENGTH_BASE, LENGTH_EXTRA};

        let mut reader = BitReader {
            data: compressed,
            position: 0,
        };
        assert_eq!(reader.bits(1), 1, "expected a final block");
        assert_eq!(reader.bits(2), 1, "expected fixed Huffman codes");

        let mut out = Vec::new();
        loop {
            match reader.symbol() {
                256 => return out,
                literal @ 0..=255 => out.push(literal as u8),
                symbol => {
                    let index = (symbol - 257) as usize;
                    let length = usize::from(LENGTH_BASE[index])
                        + reader.bits(u32::from(LENGTH_EXTRA[index])) as usize;
                    let distance_index = reader.huffman(5) as usize;
                    let distance = usize::from(DISTANCE_BASE[distance_index])
                        + reader.bits(u32::from(DISTANCE_EXTRA[distance_index])) as usize;
                    for _ in 0..length {
                        out.push(out[out.len() - distance]);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    fn round_trip(data: &[u8]) {
        assert_eq!(inflate::decode(&deflate(data)), data);
    }

    #[test]
    fn deflate_round_trips_edge_cases() {
        round_trip(b"");
        round_trip(b"a");
        round_trip(b"ab");
        round_trip(b"abcabcabcabcabcabc");
        round_trip(&[0u8; 1000]);
        round_trip(&(0..=255u8).collect::<Vec<_>>());
    }

    #[test]
    fn deflate_round_trips_long_and_noisy_data() {
        let mut state = 12345u32;
        let noisy: Vec<u8> = (0..20_000)
            .map(|i| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                if i % 7 < 4 {
                    (i % 251) as u8
                } else {
                    (state >> 24) as u8
                }
            })
            .collect();
        round_trip(&noisy);

        // A match that reaches back across the whole 32 KiB window.
        let mut far = vec![9u8; 40_000];
        far[..300]
            .iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = (i * 7) as u8);
        let start = far[..300].to_vec();
        far[32_000..32_300].copy_from_slice(&start);
        round_trip(&far);
    }

    #[test]
    fn repetitive_data_compresses_well() {
        assert!(deflate(&[7u8; 10_000]).len() < 100);
    }

    #[test]
    fn png_has_valid_structure() {
        let pixels: Vec<u8> = (0..4 * 3 * 4).map(|i| (i * 5) as u8).collect();
        let png = encode_rgba(4, 3, &pixels);

        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 3);
        assert_eq!(&png[24..29], &[8, 6, 0, 0, 0]);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");

        // Every chunk's CRC must match its type and data.
        let mut at = 8;
        while at < png.len() {
            let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let body = &png[at + 4..at + 8 + length];
            let stored =
                u32::from_be_bytes(png[at + 8 + length..at + 12 + length].try_into().unwrap());
            assert_eq!(crc32(body), stored);
            at += 12 + length;
        }
    }

    #[test]
    fn png_pixels_decode_back_to_the_input() {
        let pixels: Vec<u8> = (0..8 * 8 * 4).map(|i| ((i * i) % 256) as u8).collect();
        let png = encode_rgba(8, 8, &pixels);

        // Locate the IDAT chunk and strip the zlib header and Adler-32.
        let mut at = 8;
        let zlib = loop {
            let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            if &png[at + 4..at + 8] == b"IDAT" {
                break &png[at + 8..at + 8 + length];
            }
            at += 12 + length;
        };
        assert_eq!(&zlib[..2], &[0x78, 0x01]);
        assert_eq!((u32::from(zlib[0]) * 256 + u32::from(zlib[1])) % 31, 0);

        let filtered = inflate::decode(&zlib[2..zlib.len() - 4]);
        assert_eq!(
            u32::from_be_bytes(zlib[zlib.len() - 4..].try_into().unwrap()),
            adler32(&filtered)
        );

        // Undo the row filters.
        let stride = 8 * 4;
        let mut decoded: Vec<u8> = Vec::new();
        for (row_index, row) in filtered.chunks_exact(stride + 1).enumerate() {
            for i in 0..stride {
                let left = if i >= 4 {
                    decoded[row_index * stride + i - 4]
                } else {
                    0
                };
                let above = if row_index > 0 {
                    decoded[(row_index - 1) * stride + i]
                } else {
                    0
                };
                let predictor = match row[0] {
                    0 => 0,
                    1 => left,
                    2 => above,
                    other => panic!("unexpected filter {other}"),
                };
                decoded.push(row[1 + i].wrapping_add(predictor));
            }
        }
        assert_eq!(decoded, pixels);
    }
}
