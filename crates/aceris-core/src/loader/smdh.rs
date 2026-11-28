//! Parser and icon decoder for Nintendo 3DS SMDH application metadata.
//!
//! SMDH is found as the optional metadata section of 3DSX homebrew and as the
//! ExeFS "icon" file in CXI content. This module validates the fixed metadata
//! block and decodes the standard RGB565 8x8-tiled small and large icons.

use std::fmt;

/// Required size of a complete SMDH metadata block.
pub const SMDH_FILE_SIZE: usize = 0x36C0;
/// Number of language records stored in an SMDH block.
pub const TITLE_RECORD_COUNT: usize = 16;
/// Number of pixels per side in the small icon.
pub const SMALL_ICON_SIZE: usize = 24;
/// Number of pixels per side in the large icon.
pub const LARGE_ICON_SIZE: usize = 48;

const MAGIC: &[u8; 4] = b"SMDH";
const TITLE_RECORDS_OFFSET: usize = 0x08;
const TITLE_RECORD_SIZE: usize = 0x200;
const SHORT_DESCRIPTION_OFFSET: usize = 0x00;
const SHORT_DESCRIPTION_SIZE: usize = 0x80;
const LONG_DESCRIPTION_OFFSET: usize = 0x80;
const LONG_DESCRIPTION_SIZE: usize = 0x100;
const PUBLISHER_OFFSET: usize = 0x180;
const PUBLISHER_SIZE: usize = 0x80;
const REGION_LOCKOUT_OFFSET: usize = 0x2018;
const FLAGS_OFFSET: usize = 0x2028;
const SMALL_ICON_OFFSET: usize = 0x2040;
const SMALL_ICON_BYTES: usize = 0x480;
const LARGE_ICON_OFFSET: usize = 0x24C0;
const LARGE_ICON_BYTES: usize = 0x1200;

/// Failure while parsing an SMDH metadata block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmdhError {
    /// The supplied bytes do not contain the complete fixed-size structure.
    DataTooShort {
        actual_bytes: usize,
        required_bytes: usize,
    },
    /// The first four bytes are not the SMDH magic.
    InvalidMagic,
}

impl fmt::Display for SmdhError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DataTooShort {
                actual_bytes,
                required_bytes,
            } => write!(
                formatter,
                "SMDH data has {actual_bytes} bytes, expected at least {required_bytes}"
            ),
            Self::InvalidMagic => formatter.write_str("SMDH magic is missing"),
        }
    }
}

impl std::error::Error for SmdhError {}

/// Localized title text from one of the sixteen SMDH language slots.
///
/// The numeric slot is preserved instead of assigning names to undocumented
/// or inconsistently documented language positions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmdhApplicationTitle {
    pub short_description: String,
    pub long_description: String,
    pub publisher: String,
}

/// A decoded SMDH icon in row-major RGBA8 format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmdhIcon {
    pub width: u16,
    pub height: u16,
    /// Row-major pixels in RGBA byte order, with alpha set to 255.
    pub pixels_rgba8: Vec<u8>,
}

/// Validated title metadata and decoded standard SMDH icons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Smdh {
    pub version: u16,
    /// Sixteen title records in their original file order.
    pub titles: Vec<SmdhApplicationTitle>,
    /// Region-lockout bit mask from the SMDH settings block.
    pub region_lockout_mask: u32,
    /// Raw application flags from the SMDH settings block.
    pub flags: u32,
    pub small_icon: SmdhIcon,
    pub large_icon: SmdhIcon,
}

impl Smdh {
    /// Parses the fixed SMDH structure and decodes its standard RGB565 icons.
    ///
    /// Trailing bytes are ignored. Non-standard icon encodings are not guessed:
    /// the SMDH format permits other encodings, but this decoder supports only
    /// the observed RGB565 representation.
    pub fn parse(data: &[u8]) -> Result<Self, SmdhError> {
        if data.len() < SMDH_FILE_SIZE {
            return Err(SmdhError::DataTooShort {
                actual_bytes: data.len(),
                required_bytes: SMDH_FILE_SIZE,
            });
        }
        if data.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return Err(SmdhError::InvalidMagic);
        }

        let version = read_u16(data, 0x04);
        let mut titles = Vec::with_capacity(TITLE_RECORD_COUNT);
        for slot in 0..TITLE_RECORD_COUNT {
            let start = TITLE_RECORDS_OFFSET + slot * TITLE_RECORD_SIZE;
            titles.push(SmdhApplicationTitle {
                short_description: decode_utf16le_field(
                    &data[start + SHORT_DESCRIPTION_OFFSET
                        ..start + SHORT_DESCRIPTION_OFFSET + SHORT_DESCRIPTION_SIZE],
                ),
                long_description: decode_utf16le_field(
                    &data[start + LONG_DESCRIPTION_OFFSET
                        ..start + LONG_DESCRIPTION_OFFSET + LONG_DESCRIPTION_SIZE],
                ),
                publisher: decode_utf16le_field(
                    &data[start + PUBLISHER_OFFSET..start + PUBLISHER_OFFSET + PUBLISHER_SIZE],
                ),
            });
        }

        let small_icon = decode_rgb565_icon(
            &data[SMALL_ICON_OFFSET..SMALL_ICON_OFFSET + SMALL_ICON_BYTES],
            SMALL_ICON_SIZE,
        );
        let large_icon = decode_rgb565_icon(
            &data[LARGE_ICON_OFFSET..LARGE_ICON_OFFSET + LARGE_ICON_BYTES],
            LARGE_ICON_SIZE,
        );

        Ok(Self {
            version,
            titles,
            region_lockout_mask: read_u32(data, REGION_LOCKOUT_OFFSET),
            flags: read_u32(data, FLAGS_OFFSET),
            small_icon,
            large_icon,
        })
    }

    /// Looks up a title record by its original zero-based language slot.
    pub fn title(&self, language_slot: usize) -> Option<&SmdhApplicationTitle> {
        self.titles.get(language_slot)
    }

    /// Returns the first populated short-description record, if one exists.
    ///
    /// This is a language-neutral fallback and deliberately does not claim that
    /// a particular slot matches the host-operating-system language.
    pub fn first_available_title(&self) -> Option<&SmdhApplicationTitle> {
        self.titles
            .iter()
            .find(|title| !title.short_description.is_empty())
    }
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn decode_utf16le_field(data: &[u8]) -> String {
    let units = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16_lossy(&units).trim_end().to_owned()
}

fn decode_rgb565_icon(data: &[u8], size: usize) -> SmdhIcon {
    let mut pixels_rgba8 = vec![0; size * size * 4];
    let tiles_per_row = size / 8;

    for y in 0..size {
        for x in 0..size {
            let tile_x = x / 8;
            let tile_y = y / 8;
            let tile_index = tile_y * tiles_per_row + tile_x;
            let pixel_index = tile_index * 64 + morton_index(x & 7, y & 7);
            let source_offset = pixel_index * 2;
            let packed = u16::from_le_bytes([data[source_offset], data[source_offset + 1]]);

            let red = u32::from((packed >> 11) & 0x1F);
            let green = u32::from((packed >> 5) & 0x3F);
            let blue = u32::from(packed & 0x1F);
            let target_offset = (y * size + x) * 4;
            pixels_rgba8[target_offset] = expand_channel(red, 31);
            pixels_rgba8[target_offset + 1] = expand_channel(green, 63);
            pixels_rgba8[target_offset + 2] = expand_channel(blue, 31);
            pixels_rgba8[target_offset + 3] = 255;
        }
    }

    SmdhIcon {
        width: size as u16,
        height: size as u16,
        pixels_rgba8,
    }
}

fn expand_channel(value: u32, maximum: u32) -> u8 {
    ((value * 255 + maximum / 2) / maximum) as u8
}

/// Maps an 8x8 tile pixel to its three-level Morton/Z-order storage position.
fn morton_index(x: usize, y: usize) -> usize {
    (x & 1)
        | ((y & 1) << 1)
        | ((x & 2) << 1)
        | ((y & 2) << 2)
        | ((x & 4) << 2)
        | ((y & 4) << 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_utf16le(data: &mut [u8], offset: usize, capacity: usize, value: &str) {
        let mut cursor = offset;
        for unit in value.encode_utf16().take(capacity / 2) {
            write_u16(data, cursor, unit);
            cursor += 2;
        }
    }

    fn write_icon_pixel(
        data: &mut [u8],
        icon_offset: usize,
        size: usize,
        x: usize,
        y: usize,
        color: u16,
    ) {
        let tile_index = (y / 8) * (size / 8) + x / 8;
        let pixel_index = tile_index * 64 + morton_index(x & 7, y & 7);
        write_u16(data, icon_offset + pixel_index * 2, color);
    }

    fn minimal_smdh() -> Vec<u8> {
        let mut data = vec![0u8; SMDH_FILE_SIZE];
        data[..4].copy_from_slice(MAGIC);
        write_u16(&mut data, 0x04, 1);
        write_utf16le(
            &mut data,
            TITLE_RECORDS_OFFSET + TITLE_RECORD_SIZE,
            SHORT_DESCRIPTION_SIZE,
            "Aceris demo",
        );
        write_utf16le(
            &mut data,
            TITLE_RECORDS_OFFSET + TITLE_RECORD_SIZE + LONG_DESCRIPTION_OFFSET,
            LONG_DESCRIPTION_SIZE,
            "A homebrew sample",
        );
        write_utf16le(
            &mut data,
            TITLE_RECORDS_OFFSET + TITLE_RECORD_SIZE + PUBLISHER_OFFSET,
            PUBLISHER_SIZE,
            "Aceris",
        );
        write_u32(&mut data, REGION_LOCKOUT_OFFSET, 0xFFFF_FFFF);
        write_u32(&mut data, FLAGS_OFFSET, 0x101);
        data
    }

    #[test]
    fn parses_version_titles_publisher_and_settings() {
        let data = minimal_smdh();
        let parsed = Smdh::parse(&data).unwrap();

        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.titles.len(), TITLE_RECORD_COUNT);
        let title = parsed.title(1).unwrap();
        assert_eq!(title.short_description, "Aceris demo");
        assert_eq!(title.long_description, "A homebrew sample");
        assert_eq!(title.publisher, "Aceris");
        assert_eq!(parsed.first_available_title(), Some(title));
        assert_eq!(parsed.region_lockout_mask, 0xFFFF_FFFF);
        assert_eq!(parsed.flags, 0x101);
    }

    #[test]
    fn decodes_rgb565_pixels_and_row_major_tiles_for_both_icon_sizes() {
        let mut data = minimal_smdh();
        write_icon_pixel(&mut data, SMALL_ICON_OFFSET, SMALL_ICON_SIZE, 1, 0, 0xFFFF);
        write_icon_pixel(&mut data, LARGE_ICON_OFFSET, LARGE_ICON_SIZE, 0, 0, 0xFFFF);
        write_icon_pixel(&mut data, LARGE_ICON_OFFSET, LARGE_ICON_SIZE, 1, 0, 0xF800);
        write_icon_pixel(&mut data, LARGE_ICON_OFFSET, LARGE_ICON_SIZE, 0, 1, 0x07E0);
        write_icon_pixel(&mut data, LARGE_ICON_OFFSET, LARGE_ICON_SIZE, 8, 0, 0x001F);

        let parsed = Smdh::parse(&data).unwrap();
        assert_eq!((parsed.small_icon.width, parsed.small_icon.height), (24, 24));
        assert_eq!((parsed.large_icon.width, parsed.large_icon.height), (48, 48));
        assert_eq!(&parsed.small_icon.pixels_rgba8[4..8], &[255, 255, 255, 255]);
        assert_eq!(&parsed.large_icon.pixels_rgba8[0..4], &[255, 255, 255, 255]);
        assert_eq!(&parsed.large_icon.pixels_rgba8[4..8], &[255, 0, 0, 255]);
        assert_eq!(
            &parsed.large_icon.pixels_rgba8[48 * 4..48 * 4 + 4],
            &[0, 255, 0, 255]
        );
        assert_eq!(&parsed.large_icon.pixels_rgba8[8 * 4..8 * 4 + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn rejects_truncated_data_and_invalid_magic() {
        assert_eq!(
            Smdh::parse(&[0; 16]),
            Err(SmdhError::DataTooShort {
                actual_bytes: 16,
                required_bytes: SMDH_FILE_SIZE,
            })
        );

        let mut data = minimal_smdh();
        data[..4].copy_from_slice(b"NOPE");
        assert_eq!(Smdh::parse(&data), Err(SmdhError::InvalidMagic));
    }

    #[test]
    fn accepts_trailing_container_bytes_without_exposing_them_as_icon_data() {
        let mut data = minimal_smdh();
        data.extend_from_slice(b"trailer");
        let parsed = Smdh::parse(&data).unwrap();
        assert_eq!(parsed.large_icon.pixels_rgba8.len(), 48 * 48 * 4);
    }
}
