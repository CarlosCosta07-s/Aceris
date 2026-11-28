//! Original Aceris proportional UI typeface, rasterized from compact Rust glyph data.
//!
//! Covers ASCII letters (with dedicated lowercase), digits, Portuguese and
//! common Western European accents, ligatures, and launcher separators.
//! Regular and semibold weights share one glyph source;
//! exposed corners are softened during rendering. No OS font or external asset
//! is required, so desktop and Android produce the same text.

/// Name of the original, embedded Aceris interface typeface.
pub const FAMILY_NAME: &str = "Aceris UI";

/// Weight used by the shared launcher and in-game interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextWeight {
    #[default]
    Regular,
    Semibold,
}

/// Glyph cell width in font pixels, without spacing.
pub const GLYPH_WIDTH: usize = 5;
/// Glyph height in font pixels.
pub const GLYPH_HEIGHT: usize = 7;
/// Horizontal advance per character in font pixels.
pub const ADVANCE: usize = 6;

const UNKNOWN: [u8; 7] = [
    0b01110, 0b10001, 0b00001, 0b00110, 0b00100, 0b00000, 0b00100,
];

fn lowercase_glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch {
        'a' => [0, 0, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111],
        'b' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110,
        ],
        'c' => [0, 0, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110],
        'd' => [
            0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111,
        ],
        'e' => [0, 0, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111],
        'f' => [
            0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000,
        ],
        // Descender letters keep their bowl on the x-height rows so they do not
        // read as capitals in a 7-row cell.
        'g' => [0, 0, 0b01111, 0b10001, 0b01111, 0b00001, 0b01110],
        'h' => [
            0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'i' => [0b00100, 0, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
        'j' => [0b00010, 0, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100],
        'k' => [
            0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010,
        ],
        'l' => [
            0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'm' => [0, 0, 0b11010, 0b10101, 0b10101, 0b10101, 0b10101],
        'n' => [0, 0, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001],
        'o' => [0, 0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
        'p' => [0, 0, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000],
        'q' => [0, 0, 0b01111, 0b10001, 0b01111, 0b00001, 0b00001],
        'r' => [0, 0, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000],
        's' => [0, 0, 0b01111, 0b10000, 0b01110, 0b00001, 0b11110],
        't' => [
            0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110,
        ],
        'u' => [0, 0, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101],
        'v' => [0, 0, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'w' => [0, 0, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010],
        'x' => [0, 0, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001],
        'y' => [0, 0, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
        'z' => [0, 0, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111],
        'á' => [
            0b00010, 0b00100, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111,
        ],
        'à' => [
            0b01000, 0b00100, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111,
        ],
        'â' => [
            0b00100, 0b01010, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111,
        ],
        'ã' => [
            0b01010, 0b10101, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111,
        ],
        'é' => [
            0b00010, 0b00100, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111,
        ],
        'ê' => [
            0b00100, 0b01010, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111,
        ],
        'í' => [
            0b00010, 0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'ó' => [
            0b00010, 0b00100, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'ô' => [
            0b00100, 0b01010, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'õ' => [
            0b01010, 0b10101, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'ú' => [
            0b00010, 0b00100, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101,
        ],
        'ä' => [0b01010, 0, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111],
        'å' => [
            0b00100, 0b01010, 0b01110, 0b10001, 0b11111, 0b10001, 0b10001,
        ],
        'æ' => [0, 0, 0b01101, 0b10110, 0b11111, 0b10100, 0b01111],
        'è' => [
            0b01000, 0b00100, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111,
        ],
        'ë' => [0b01010, 0, 0b01110, 0b10001, 0b11111, 0b10000, 0b01111],
        'ì' => [0b01000, 0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
        'ï' => [0b01010, 0, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110],
        'ñ' => [
            0b01010, 0b10101, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001,
        ],
        'ò' => [
            0b01000, 0b00100, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'ö' => [0b01010, 0, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110],
        'ø' => [
            0, 0, 0b01110, 0b10011, 0b10101, 0b11001, 0b01110,
        ],
        'ù' => [
            0b01000, 0b00100, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101,
        ],
        'ü' => [0b01010, 0, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101],
        'ÿ' => [0b01010, 0, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110],
        'œ' => [
            0, 0, 0b01101, 0b10010, 0b10010, 0b10010, 0b01101,
        ],
        'ß' => [0, 0, 0b01110, 0b10000, 0b10110, 0b10001, 0b11110],
        'ç' => [0, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110, 0b00100],
        _ => return None,
    })
}

fn glyph(ch: char) -> [u8; 7] {
    let normalized = match ch {
        'Á' => 'á',
        'À' => 'à',
        'Â' => 'â',
        'Ã' => 'ã',
        'É' => 'é',
        'Ê' => 'ê',
        'Í' => 'í',
        'Ó' => 'ó',
        'Ô' => 'ô',
        'Õ' => 'õ',
        'Ú' => 'ú',
        'Ä' => 'ä',
        'Å' => 'å',
        'Æ' => 'æ',
        'È' => 'è',
        'Ë' => 'ë',
        'Ì' => 'ì',
        'Ï' => 'ï',
        'Ñ' => 'ñ',
        'Ò' => 'ò',
        'Ö' => 'ö',
        'Ø' => 'ø',
        'Ù' => 'ù',
        'Ü' => 'ü',
        'Ÿ' => 'ÿ',
        'Œ' => 'œ',
        'ẞ' => 'ß',
        'Ç' => 'ç',
        _ => ch,
    };
    if let Some(rows) = lowercase_glyph(normalized) {
        return rows;
    }
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
        '|' => [
            0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101,
        ],
        '–' | '—' => [0, 0, 0, 0b11111, 0, 0, 0],
        '·' | '•' => [0, 0, 0b01110, 0b01110, 0b01110, 0, 0],
        '→' | '>' => [0, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0],
        '←' | '<' => [0, 0b00010, 0b00100, 0b01000, 0b00100, 0b00010, 0],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        ';' => [0, 0b01100, 0b01100, 0, 0b01100, 0b00100, 0b01000],
        '"' => [0b01010, 0b01010, 0, 0, 0, 0, 0],
        '%' => [
            0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011,
        ],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        '*' => [0, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0],
        '¿' => [0, 0b01110, 0b00001, 0b00110, 0b00100, 0, 0b00100],
        '¡' => [0, 0b00100, 0, 0b00100, 0b00100, 0b00100, 0b00100],
        '[' => [
            0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110,
        ],
        ']' => [
            0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110,
        ],
        '×' => [0, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0],
        _ => UNKNOWN,
    }
}

/// Width in pixels of `chars` monospaced cells drawn at `scale`.
///
/// Kept for callers that know only a character count. Prefer [measure_text]
/// whenever the actual string is available.
pub fn text_width(chars: usize, scale: usize) -> usize {
    (chars * ADVANCE).saturating_sub(1) * scale
}

/// Measures the proportional width of a string at the supplied integer scale.
pub fn measure_text(text: &str, scale: usize) -> usize {
    let scale = scale.max(1);
    let advances = text.chars().map(character_advance).sum::<usize>();
    let tracking = if text.is_empty() { 0 } else { 1 };
    advances.saturating_sub(tracking) * scale
}

fn glyph_bounds(rows: &[u8; GLYPH_HEIGHT]) -> Option<(usize, usize)> {
    let mut minimum = GLYPH_WIDTH;
    let mut maximum = 0usize;
    for &bits in rows {
        for column in 0..GLYPH_WIDTH {
            if bits >> (GLYPH_WIDTH - 1 - column) & 1 != 0 {
                minimum = minimum.min(column);
                maximum = maximum.max(column);
            }
        }
    }
    (minimum < GLYPH_WIDTH).then_some((minimum, maximum))
}

fn character_advance(ch: char) -> usize {
    if ch == ' ' {
        return 4;
    }
    glyph_bounds(&glyph(ch))
        .map(|(_, maximum)| maximum + 2)
        .unwrap_or(ADVANCE)
}

fn pixel_is_set(rows: &[u8; GLYPH_HEIGHT], column: i32, row: i32) -> bool {
    if column < 0 || row < 0 || column >= GLYPH_WIDTH as i32 || row >= GLYPH_HEIGHT as i32 {
        return false;
    }
    rows[row as usize] >> (GLYPH_WIDTH - 1 - column as usize) & 1 != 0
}

/// Rasterizes one bitmap cell as a softly rounded shape. Only exposed corners
/// are rounded, so adjacent strokes remain connected. Four-by-four coverage
/// sampling gives the built-in face smoother edges without an external asset.
#[allow(clippy::too_many_arguments)]
fn draw_rounded_cell(
    canvas: &mut [u8],
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    scale: usize,
    color: [u8; 3],
    round_top_left: bool,
    round_top_right: bool,
    round_bottom_left: bool,
    round_bottom_right: bool,
) {
    const SAMPLES: usize = 4;
    let radius = scale as f32 * 0.48;
    let radius_squared = radius * radius;

    for dy in 0..scale {
        let py = top.saturating_add(dy as i32);
        if py < 0 || py as usize >= height {
            continue;
        }
        for dx in 0..scale {
            let px = left.saturating_add(dx as i32);
            if px < 0 || px as usize >= width {
                continue;
            }
            let mut coverage = 0u32;
            for sample_y in 0..SAMPLES {
                let local_y = dy as f32 + (sample_y as f32 + 0.5) / SAMPLES as f32;
                for sample_x in 0..SAMPLES {
                    let local_x = dx as f32 + (sample_x as f32 + 0.5) / SAMPLES as f32;
                    let outside_top_left = round_top_left
                        && local_x < radius
                        && local_y < radius
                        && (local_x - radius).powi(2) + (local_y - radius).powi(2) > radius_squared;
                    let outside_top_right = round_top_right
                        && local_x > scale as f32 - radius
                        && local_y < radius
                        && (local_x - (scale as f32 - radius)).powi(2) + (local_y - radius).powi(2)
                            > radius_squared;
                    let outside_bottom_left = round_bottom_left
                        && local_x < radius
                        && local_y > scale as f32 - radius
                        && (local_x - radius).powi(2) + (local_y - (scale as f32 - radius)).powi(2)
                            > radius_squared;
                    let outside_bottom_right = round_bottom_right
                        && local_x > scale as f32 - radius
                        && local_y > scale as f32 - radius
                        && (local_x - (scale as f32 - radius)).powi(2)
                            + (local_y - (scale as f32 - radius)).powi(2)
                            > radius_squared;
                    if !outside_top_left
                        && !outside_top_right
                        && !outside_bottom_left
                        && !outside_bottom_right
                    {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = (coverage * 255 + 8) / 16;
            let at = (py as usize * width + px as usize) * 4;
            if alpha == 255 {
                canvas[at..at + 3].copy_from_slice(&color);
            } else {
                let inverse = 255 - alpha;
                for channel in 0..3 {
                    canvas[at + channel] = ((u32::from(color[channel]) * alpha
                        + u32::from(canvas[at + channel]) * inverse
                        + 127)
                        / 255) as u8;
                }
            }
            canvas[at + 3] = 255;
        }
    }
}

/// Measures the proportional width of a string using a selected type weight.
pub fn measure_text_with_weight(text: &str, scale: usize, weight: TextWeight) -> usize {
    let scale = scale.max(1);
    let mut advances = 0usize;
    for ch in text.chars() {
        advances = advances.saturating_add(character_advance_with_weight(ch, weight));
    }
    let tracking = if text.is_empty() { 0 } else { 1 };
    advances.saturating_sub(tracking).saturating_mul(scale)
}

/// Draws text with the chosen weight into an RGBA canvas.
///
/// Pixels outside the canvas are clipped. Regular is the compact reading face;
/// semibold is reserved for headings, navigation and high-priority controls.
#[allow(clippy::too_many_arguments)]
pub fn draw_text_with_weight(
    canvas: &mut [u8],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    scale: usize,
    text: &str,
    color: [u8; 3],
    weight: TextWeight,
) {
    if scale == 0
        || width == 0
        || height == 0
        || canvas.len() != width.saturating_mul(height).saturating_mul(4)
    {
        return;
    }

    let mut cursor = x;
    for ch in text.chars() {
        if ch == ' ' {
            cursor =
                cursor.saturating_add((character_advance_with_weight(ch, weight) * scale) as i32);
            continue;
        }
        let rows = glyph_with_weight(ch, weight);
        let Some((minimum, maximum)) = glyph_bounds(&rows) else {
            cursor = cursor.saturating_add((ADVANCE * scale) as i32);
            continue;
        };
        for (row, bits) in rows.iter().enumerate() {
            for column in minimum..=maximum {
                if bits >> (GLYPH_WIDTH - 1 - column) & 1 == 0 {
                    continue;
                }
                let col = column as i32;
                let row_index = row as i32;
                draw_rounded_cell(
                    canvas,
                    width,
                    height,
                    cursor.saturating_add((column * scale) as i32),
                    y.saturating_add((row * scale) as i32),
                    scale,
                    color,
                    !pixel_is_set(&rows, col - 1, row_index)
                        && !pixel_is_set(&rows, col, row_index - 1),
                    !pixel_is_set(&rows, col + 1, row_index)
                        && !pixel_is_set(&rows, col, row_index - 1),
                    !pixel_is_set(&rows, col - 1, row_index)
                        && !pixel_is_set(&rows, col, row_index + 1),
                    !pixel_is_set(&rows, col + 1, row_index)
                        && !pixel_is_set(&rows, col, row_index + 1),
                );
            }
        }
        cursor = cursor.saturating_add((character_advance_with_weight(ch, weight) * scale) as i32);
    }
}

/// Draws the regular Aceris UI weight for compatibility with existing callers.
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
    draw_text_with_weight(
        canvas,
        width,
        height,
        x,
        y,
        scale,
        text,
        color,
        TextWeight::Regular,
    );
}

fn glyph_with_weight(ch: char, weight: TextWeight) -> [u8; GLYPH_HEIGHT] {
    let rows = glyph(ch);
    match weight {
        TextWeight::Regular => rows,
        // Extend each stroke one pixel to the right while keeping its original
        // outline. This gives headings more presence without another font asset.
        TextWeight::Semibold => rows.map(|bits| bits | (bits >> 1)),
    }
}

fn character_advance_with_weight(ch: char, weight: TextWeight) -> usize {
    if ch == ' ' {
        return 4;
    }
    glyph_bounds(&glyph_with_weight(ch, weight))
        .map(|(_, maximum)| maximum + 2)
        .unwrap_or(ADVANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawing_sets_pixels_and_clips_at_the_edges() {
        let mut canvas = vec![0u8; 8 * 8 * 4];
        draw_text(&mut canvas, 8, 8, 0, 0, 1, "I", [255, 255, 255]);
        // The rounded typeface keeps I's side bearings and softens exposed corners.
        let softened = (0usize * 8 + 1) * 4;
        assert!(canvas[softened] > 180 && canvas[softened] < 255);
        assert_eq!(canvas[softened], canvas[softened + 1]);
        assert_eq!(canvas[softened + 1], canvas[softened + 2]);
        assert_eq!(canvas[softened + 3], 255);
        let solid = (0usize * 8 + 2) * 4;
        assert_eq!(&canvas[solid..solid + 4], &[255, 255, 255, 255]);
        assert_eq!(&canvas[0..3], &[0, 0, 0]);
        // Far outside and negative origins must not panic.
        draw_text(&mut canvas, 8, 8, -100, 100, 2, "ABC", [1, 1, 1]);
        draw_text(&mut canvas, 8, 8, 6, 6, 3, "Z", [1, 1, 1]);
    }

    #[test]
    fn lowercase_and_portuguese_accents_have_dedicated_glyphs() {
        assert_ne!(glyph('a'), glyph('A'));
        assert_ne!(glyph('é'), glyph('?'));
        assert_ne!(glyph('ç'), glyph('?'));
        assert_ne!(glyph('É'), glyph('?'));
        assert_eq!(text_width(3, 2), (3 * ADVANCE - 1) * 2);
    }

    #[test]
    fn western_latin_accents_and_ligatures_have_dedicated_glyphs() {
        for (lower, upper) in [
            ('ä', 'Ä'),
            ('å', 'Å'),
            ('æ', 'Æ'),
            ('è', 'È'),
            ('ë', 'Ë'),
            ('ì', 'Ì'),
            ('ï', 'Ï'),
            ('ñ', 'Ñ'),
            ('ò', 'Ò'),
            ('ö', 'Ö'),
            ('ø', 'Ø'),
            ('ù', 'Ù'),
            ('ü', 'Ü'),
            ('ÿ', 'Ÿ'),
            ('œ', 'Œ'),
            ('ß', 'ẞ'),
        ] {
            assert_ne!(glyph(lower), UNKNOWN, "missing lowercase glyph {lower:?}");
            assert_eq!(glyph(upper), glyph(lower), "uppercase mismatch for {upper:?}");
        }
    }

    #[test]
    fn interface_separators_and_symbols_have_glyphs() {
        for ch in ['|', '&', '–', '—', '·', '→', '¿', '¡', '%', '#', '[', ']'] {
            assert_ne!(glyph(ch), UNKNOWN, "missing glyph for {ch:?}");
        }
    }

    #[test]
    fn proportional_measurement_gives_narrow_glyphs_less_advance() {
        assert!(measure_text("WWW", 2) > measure_text("III", 2));
        assert!(measure_text("Aceris UI", 1) > 0);
        assert_eq!(measure_text("", 2), 0);
        assert!(measure_text("III", 1) < text_width(3, 1));
    }

    #[test]
    fn rounded_glyph_edges_use_partial_pixel_coverage() {
        let mut canvas = vec![0u8; 8 * 8 * 4];
        draw_text(&mut canvas, 8, 8, 0, 0, 1, "I", [255, 255, 255]);
        let at = (0usize * 8 + 1) * 4;
        assert!(canvas[at] > 180 && canvas[at] < 255);
        assert_eq!(canvas[at + 3], 255);
    }

    #[test]
    fn semibold_weight_adds_stroke_presence_without_losing_portuguese_glyphs() {
        assert!(
            measure_text_with_weight("Aceris UI", 2, TextWeight::Semibold)
                >= measure_text("Aceris UI", 2)
        );
        assert_ne!(
            glyph_with_weight('á', TextWeight::Semibold),
            glyph_with_weight('á', TextWeight::Regular)
        );

        let mut regular = vec![0u8; 24 * 16 * 4];
        let mut semibold = vec![0u8; 24 * 16 * 4];
        draw_text_with_weight(
            &mut regular,
            24,
            16,
            0,
            0,
            2,
            "A",
            [255, 255, 255],
            TextWeight::Regular,
        );
        draw_text_with_weight(
            &mut semibold,
            24,
            16,
            0,
            0,
            2,
            "A",
            [255, 255, 255],
            TextWeight::Semibold,
        );
        let regular_pixels = regular.chunks_exact(4).filter(|pixel| pixel[0] > 0).count();
        let semibold_pixels = semibold
            .chunks_exact(4)
            .filter(|pixel| pixel[0] > 0)
            .count();
        assert!(semibold_pixels > regular_pixels);
    }

    #[test]
    fn descender_letters_do_not_reach_the_cap_height_row() {
        for ch in ['g', 'p', 'q', 'y'] {
            assert_eq!(glyph(ch)[1], 0, "{ch:?} would read as a capital");
            assert_ne!(glyph(ch), glyph(ch.to_ascii_uppercase()));
        }
    }
}
