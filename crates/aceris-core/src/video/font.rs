//! Tiny built-in 5x7 bitmap font for frontend screens (uppercase only).
//!
//! Lowercase letters are drawn as uppercase; characters without a glyph
//! (accents, symbols) are drawn as `?`.

/// Glyph cell width in font pixels, without spacing.
pub const GLYPH_WIDTH: usize = 5;
/// Glyph height in font pixels.
pub const GLYPH_HEIGHT: usize = 7;
/// Horizontal advance per character in font pixels.
pub const ADVANCE: usize = 6;

const UNKNOWN: [u8; 7] = [
    0b01110, 0b10001, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100,
];

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        ' ' => [0; 7],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ',' => [0, 0, 0, 0, 0b01100, 0b00100, 0b01000],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '/' => [
            0b00001, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        _ => UNKNOWN,
    }
}

/// Width in pixels of `chars` characters drawn at `scale`.
pub fn text_width(chars: usize, scale: usize) -> usize {
    (chars * ADVANCE).saturating_sub(1) * scale
}

/// Draws `text` into an RGBA canvas with its top-left corner at (`x`, `y`).
/// Pixels outside the canvas are clipped.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut [u8],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    scale: usize,
    text: &str,
    color: [u8; 3],
) {
    for (index, ch) in text.chars().enumerate() {
        let rows = glyph(ch);
        let left = x + (index * ADVANCE * scale) as i32;
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_WIDTH {
                if bits >> (GLYPH_WIDTH - 1 - col) & 1 == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = left + (col * scale + dx) as i32;
                        let py = y + (row * scale + dy) as i32;
                        if px < 0 || py < 0 || px as usize >= width || py as usize >= height {
                            continue;
                        }
                        let at = (py as usize * width + px as usize) * 4;
                        canvas[at..at + 3].copy_from_slice(&color);
                        canvas[at + 3] = 255;
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
    fn drawing_sets_pixels_and_clips_at_the_edges() {
        let mut canvas = vec![0u8; 8 * 8 * 4];
        draw_text(&mut canvas, 8, 8, 0, 0, 1, "I", [9, 8, 7]);
        // Top row of "I" is 0b01110: columns 1..=3.
        assert_eq!(&canvas[4..7], &[9, 8, 7]);
        assert_eq!(&canvas[0..3], &[0, 0, 0]);
        // Far outside and negative origins must not panic.
        draw_text(&mut canvas, 8, 8, -100, 100, 2, "ABC", [1, 1, 1]);
        draw_text(&mut canvas, 8, 8, 6, 6, 3, "Z", [1, 1, 1]);
    }

    #[test]
    fn lowercase_matches_uppercase_and_unknown_is_question_mark() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('é'), glyph('?'));
        assert_eq!(text_width(3, 2), (3 * ADVANCE - 1) * 2);
    }
}
