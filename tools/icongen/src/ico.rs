//! Windows `.ico` writer. Each entry embeds a complete PNG file, which every
//! Windows version since Vista understands.

/// One icon size: its side in pixels and the encoded PNG.
pub struct Entry {
    pub size: u32,
    pub png: Vec<u8>,
}

/// Serialises `entries` as an `.ico` file.
///
/// Sizes must be between 1 and 256 pixels; 256 is stored as `0`, as the
/// format requires.
pub fn encode(entries: &[Entry]) -> Vec<u8> {
    const HEADER_LEN: usize = 6;
    const ENTRY_LEN: usize = 16;

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());

    let mut offset = HEADER_LEN + ENTRY_LEN * entries.len();
    for entry in entries {
        assert!(
            (1..=256).contains(&entry.size),
            "ICO entries must be 1 to 256 pixels"
        );
        let side = if entry.size == 256 {
            0
        } else {
            entry.size as u8
        };
        out.extend_from_slice(&[side, side, 0, 0]); // width, height, palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(entry.png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += entry.png.len();
    }
    for entry in entries {
        out.extend_from_slice(&entry.png);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_directory_and_payloads_are_laid_out_in_order() {
        let ico = encode(&[
            Entry {
                size: 16,
                png: vec![1, 2, 3],
            },
            Entry {
                size: 256,
                png: vec![4, 5],
            },
        ]);

        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        // First entry: 16x16, 32 bpp, 3 bytes at offset 6 + 2 * 16.
        assert_eq!(&ico[6..8], &[16, 16]);
        assert_eq!(u16::from_le_bytes([ico[12], ico[13]]), 32);
        assert_eq!(u32::from_le_bytes(ico[14..18].try_into().unwrap()), 3);
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 38);
        // Second entry stores 256 as zero and follows the first payload.
        assert_eq!(&ico[22..24], &[0, 0]);
        assert_eq!(u32::from_le_bytes(ico[30..34].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(ico[34..38].try_into().unwrap()), 41);
        assert_eq!(&ico[38..], &[1, 2, 3, 4, 5]);
    }

    #[test]
    #[should_panic(expected = "1 to 256")]
    fn oversized_entries_are_rejected() {
        encode(&[Entry {
            size: 512,
            png: Vec::new(),
        }]);
    }
}
