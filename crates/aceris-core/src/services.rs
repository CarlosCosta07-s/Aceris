//! Static data for stateless HLE system services (`cfg:u`, `apt:U`).
//!
//! Values are neutral development defaults (USA region, original 3DS model,
//! English, stereo sound). Command layouts come from public 3dbrew/libctru
//! documentation and have not been verified on hardware or against a real
//! console's configuration savegame.

/// Config block: system language (1 byte, 1 = English).
pub const CFG_BLOCK_LANGUAGE: u32 = 0x000A_0002;
/// Config block: sound output mode (1 byte, 1 = stereo).
pub const CFG_BLOCK_SOUND_MODE: u32 = 0x0007_0001;

/// Secure-info region value reported by `SecureInfoGetRegion` (1 = USA).
pub const CFG_REGION_USA: u8 = 1;
/// System model reported by `GetSystemModel` (0 = original Nintendo 3DS).
pub const CFG_MODEL_3DS: u8 = 0;

/// Returns the bytes of a supported config block.
pub fn cfg_block(id: u32) -> Option<&'static [u8]> {
    match id {
        CFG_BLOCK_LANGUAGE => Some(&[1]),
        CFG_BLOCK_SOUND_MODE => Some(&[1]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_blocks_have_documented_sizes_and_unknown_blocks_are_absent() {
        assert_eq!(cfg_block(CFG_BLOCK_LANGUAGE), Some(&[1u8][..]));
        assert_eq!(cfg_block(CFG_BLOCK_SOUND_MODE).map(<[u8]>::len), Some(1));
        assert_eq!(cfg_block(0xDEAD_BEEF), None);
    }
}
