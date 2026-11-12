//! Minimal dependency-free PNG encoder for LCD surfaces.
//!
//! Output is a valid 8-bit RGBA PNG whose zlib stream uses stored (uncompressed)
//! deflate blocks. Files are larger than a compressed PNG but need no
//! compression code, which keeps frame dumps simple and verifiable.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// Largest payload of one stored deflate block.
const STORED_BLOCK_MAX: usize = 0xFFFF;

fn crc32(chunks: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for chunk in chunks {
        for &byte in *chunk {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// Encodes tightly packed RGBA8 pixels (`width * height * 4` bytes) as a PNG.
///
/// Returns `None` when the buffer length does not match the dimensions.
pub fn encode_rgba8(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || width > u32::MAX as usize || height > u32::MAX as usize {
        return None;
    }
    let row_bytes = width.checked_mul(4)?;
    if rgba.len() != row_bytes.checked_mul(height)? {
        return None;
    }

    // Each scanline is prefixed with filter type 0 (none).
    let mut raw = Vec::with_capacity((row_bytes + 1) * height);
    for row in rgba.chunks_exact(row_bytes) {
        raw.push(0);
        raw.extend_from_slice(row);
    }

    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(STORED_BLOCK_MAX).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        zlib.extend_from_slice(&(block.len() as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, no filter, no interlace

    let mut out = Vec::with_capacity(zlib.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    push_chunk(&mut out, b"IHDR", &header);
    push_chunk(&mut out, b"IDAT", &zlib);
    push_chunk(&mut out, b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_and_adler32_match_reference_values() {
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[b"1234", b"56789"]), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    /// Reads chunks back and reassembles the stored-deflate payload.
    fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        assert_eq!(&png[..8], &SIGNATURE);
        let (mut at, mut size, mut zlib) = (8, (0, 0), Vec::new());
        let mut ended = false;
        while at < png.len() {
            let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = png[at + 4..at + 8].try_into().unwrap();
            let data = &png[at + 8..at + 8 + length];
            let stored_crc =
                u32::from_be_bytes(png[at + 8 + length..at + 12 + length].try_into().unwrap());
            assert_eq!(stored_crc, crc32(&[&kind, data]), "{kind:?} crc");
            match &kind {
                b"IHDR" => {
                    size = (
                        u32::from_be_bytes(data[0..4].try_into().unwrap()),
                        u32::from_be_bytes(data[4..8].try_into().unwrap()),
                    );
                    assert_eq!(&data[8..], &[8, 6, 0, 0, 0]);
                }
                b"IDAT" => zlib.extend_from_slice(data),
                b"IEND" => ended = true,
                other => panic!("unexpected chunk {other:?}"),
            }
            at += 12 + length;
        }
        assert!(ended);

        assert_eq!(&zlib[..2], &[0x78, 0x01]);
        let mut raw = Vec::new();
        let mut pos = 2;
        loop {
            let last = zlib[pos] == 1;
            let len = u16::from_le_bytes([zlib[pos + 1], zlib[pos + 2]]);
            let nlen = u16::from_le_bytes([zlib[pos + 3], zlib[pos + 4]]);
            assert_eq!(len, !nlen);
            raw.extend_from_slice(&zlib[pos + 5..pos + 5 + len as usize]);
            pos += 5 + len as usize;
            if last {
                break;
            }
        }
        assert_eq!(
            u32::from_be_bytes(zlib[pos..pos + 4].try_into().unwrap()),
            adler32(&raw)
        );
        assert_eq!(pos + 4, zlib.len());
        (size.0, size.1, raw)
    }

    #[test]
    fn small_image_round_trips_through_the_container() {
        let rgba = [255, 0, 0, 255, 0, 255, 0, 128];
        let png = encode_rgba8(2, 1, &rgba).unwrap();
        let (w, h, raw) = decode(&png);
        assert_eq!((w, h), (2, 1));
        assert_eq!(raw, [0, 255, 0, 0, 255, 0, 255, 0, 128]);
    }

    #[test]
    fn large_images_are_split_into_multiple_stored_blocks() {
        let (width, height) = (400, 240);
        let rgba: Vec<u8> = (0..width * height * 4).map(|i| (i % 251) as u8).collect();
        let png = encode_rgba8(width, height, &rgba).unwrap();
        let (w, h, raw) = decode(&png);
        assert_eq!((w as usize, h as usize), (width, height));
        assert!(raw.len() > STORED_BLOCK_MAX);
        for (y, row) in raw.chunks_exact(width * 4 + 1).enumerate() {
            assert_eq!(row[0], 0);
            assert_eq!(&row[1..], &rgba[y * width * 4..(y + 1) * width * 4]);
        }
    }

    #[test]
    fn mismatched_buffers_and_empty_dimensions_are_rejected() {
        assert!(encode_rgba8(2, 2, &[0; 15]).is_none());
        assert!(encode_rgba8(0, 1, &[]).is_none());
    }
}
