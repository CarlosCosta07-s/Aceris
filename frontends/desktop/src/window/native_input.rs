//! Platform-neutral decoding helpers for the Windows and macOS backends.
//!
//! The helpers contain no FFI so they build and are unit-tested on every
//! platform even though only the matching backend calls them at run time.

// Each backend uses only its own half; the rest is exercised by the tests.
#![allow(dead_code)]

use super::{GameKey, Key};
use aceris_core::input::pad;

/// Maps a Windows virtual-key code to a front-end navigation key.
pub fn key_from_virtual_key(vk: u32) -> Option<Key> {
    match vk {
        0x26 => Some(Key::Up),
        0x28 => Some(Key::Down),
        0x25 => Some(Key::Left),
        0x27 => Some(Key::Right),
        0x21 => Some(Key::PageUp),
        0x22 => Some(Key::PageDown),
        0x0D => Some(Key::Enter),
        0x1B => Some(Key::Escape),
        _ => None,
    }
}

/// Maps a Windows virtual-key code to a controller input.
///
/// Same layout as the X11 backend: arrows are the D-pad; I/J/K/L the circle
/// pad; X/Z/S/A are A/B/X/Y; Q/W are L/R; 1/2 are ZL/ZR; Return is Start and
/// Backspace is Select.
pub fn game_key_from_virtual_key(vk: u32) -> Option<GameKey> {
    let button = |bit| Some(GameKey::Button(bit));
    match vk {
        0x25 => button(pad::LEFT),
        0x26 => button(pad::UP),
        0x27 => button(pad::RIGHT),
        0x28 => button(pad::DOWN),
        0x0D => button(pad::START),
        0x08 => button(pad::SELECT),
        0x58 => button(pad::A),
        0x5A => button(pad::B),
        0x53 => button(pad::X),
        0x41 => button(pad::Y),
        0x51 => button(pad::L),
        0x57 => button(pad::R),
        0x31 => button(pad::ZL),
        0x32 => button(pad::ZR),
        0x49 => Some(GameKey::Stick { x: 0, y: 1 }),
        0x4B => Some(GameKey::Stick { x: 0, y: -1 }),
        0x4A => Some(GameKey::Stick { x: -1, y: 0 }),
        0x4C => Some(GameKey::Stick { x: 1, y: 0 }),
        _ => None,
    }
}

/// Splits a mouse-message `lParam` into signed client coordinates
/// (`GET_X_LPARAM` / `GET_Y_LPARAM`).
pub fn point_from_lparam(lparam: isize) -> (i32, i32) {
    let bits = lparam as usize;
    (
        i32::from((bits & 0xFFFF) as u16 as i16),
        i32::from(((bits >> 16) & 0xFFFF) as u16 as i16),
    )
}

/// Converts a `WM_MOUSEWHEEL` `wParam` into list rows: negative scrolls up.
pub fn scroll_rows_from_wheel(wparam: usize) -> i32 {
    let delta = i32::from(((wparam >> 16) & 0xFFFF) as u16 as i16);
    -(delta / 120)
}

/// Converts an `NSEvent` scroll delta into list rows: negative scrolls up.
///
/// Wheel mice report lines; trackpads (`precise`) report points, about ten per row.
pub fn scroll_rows_from_mac_delta(delta_y: f64, precise: bool) -> i32 {
    let rows = if precise { delta_y / 10.0 } else { delta_y };
    // Positive deltaY means content moves down, which selects earlier rows.
    -(rows.round() as i32)
}

/// True when a `WM_KEYDOWN` `lParam` marks an auto-repeat (bit 30: key was down).
pub fn is_key_repeat(lparam: isize) -> bool {
    (lparam as usize) & (1 << 30) != 0
}

/// Maps a macOS virtual key code (`kVK_*`) to a front-end navigation key.
pub fn key_from_mac_keycode(code: u16) -> Option<Key> {
    match code {
        0x7E => Some(Key::Up),
        0x7D => Some(Key::Down),
        0x7B => Some(Key::Left),
        0x7C => Some(Key::Right),
        0x74 => Some(Key::PageUp),
        0x79 => Some(Key::PageDown),
        0x24 | 0x4C => Some(Key::Enter),
        0x35 => Some(Key::Escape),
        _ => None,
    }
}

/// Maps a macOS virtual key code (`kVK_*`) to a controller input.
pub fn game_key_from_mac_keycode(code: u16) -> Option<GameKey> {
    let button = |bit| Some(GameKey::Button(bit));
    match code {
        0x7B => button(pad::LEFT),
        0x7E => button(pad::UP),
        0x7C => button(pad::RIGHT),
        0x7D => button(pad::DOWN),
        0x24 | 0x4C => button(pad::START),
        0x33 => button(pad::SELECT),
        0x07 => button(pad::A),
        0x06 => button(pad::B),
        0x01 => button(pad::X),
        0x00 => button(pad::Y),
        0x0C => button(pad::L),
        0x0D => button(pad::R),
        0x12 => button(pad::ZL),
        0x13 => button(pad::ZR),
        0x22 => Some(GameKey::Stick { x: 0, y: 1 }),
        0x28 => Some(GameKey::Stick { x: 0, y: -1 }),
        0x26 => Some(GameKey::Stick { x: -1, y: 0 }),
        0x25 => Some(GameKey::Stick { x: 1, y: 0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_virtual_keys_match_the_x11_layout() {
        assert_eq!(
            game_key_from_virtual_key(0x26),
            Some(GameKey::Button(pad::UP))
        );
        assert_eq!(
            game_key_from_virtual_key(0x58),
            Some(GameKey::Button(pad::A))
        );
        assert_eq!(
            game_key_from_virtual_key(0x0D),
            Some(GameKey::Button(pad::START))
        );
        assert_eq!(
            game_key_from_virtual_key(0x49),
            Some(GameKey::Stick { x: 0, y: 1 })
        );
        assert_eq!(
            game_key_from_virtual_key(0x4C),
            Some(GameKey::Stick { x: 1, y: 0 })
        );
        assert_eq!(game_key_from_virtual_key(0x7A), None);
        assert_eq!(key_from_virtual_key(0x1B), Some(Key::Escape));
        assert_eq!(key_from_virtual_key(0x22), Some(Key::PageDown));
        assert_eq!(key_from_virtual_key(0x41), None);
    }

    #[test]
    fn every_x11_game_binding_has_a_windows_and_macos_equivalent() {
        // Same physical letters/arrows on every backend.
        let x11 = [
            (0xFF51u64, 0x25u32, 0x7Bu16),
            (0xFF52, 0x26, 0x7E),
            (0xFF53, 0x27, 0x7C),
            (0xFF54, 0x28, 0x7D),
            (0xFF0D, 0x0D, 0x24),
            (0xFF08, 0x08, 0x33),
            (0x78, 0x58, 0x07),
            (0x7A, 0x5A, 0x06),
            (0x73, 0x53, 0x01),
            (0x61, 0x41, 0x00),
            (0x71, 0x51, 0x0C),
            (0x77, 0x57, 0x0D),
            (0x31, 0x31, 0x12),
            (0x32, 0x32, 0x13),
            (0x69, 0x49, 0x22),
            (0x6B, 0x4B, 0x28),
            (0x6A, 0x4A, 0x26),
            (0x6C, 0x4C, 0x25),
        ];
        for (keysym, vk, mac) in x11 {
            let expected = super::super::game_key_from_keysym(keysym);
            assert!(expected.is_some(), "keysym {keysym:#x}");
            assert_eq!(game_key_from_virtual_key(vk), expected, "vk {vk:#x}");
            assert_eq!(game_key_from_mac_keycode(mac), expected, "mac {mac:#x}");
        }
    }

    #[test]
    fn mac_scroll_deltas_become_rows() {
        assert_eq!(scroll_rows_from_mac_delta(1.0, false), -1);
        assert_eq!(scroll_rows_from_mac_delta(-3.0, false), 3);
        assert_eq!(scroll_rows_from_mac_delta(20.0, true), -2);
        assert_eq!(scroll_rows_from_mac_delta(2.0, true), 0);
    }

    #[test]
    fn mac_navigation_keys() {
        assert_eq!(key_from_mac_keycode(0x7E), Some(Key::Up));
        assert_eq!(key_from_mac_keycode(0x4C), Some(Key::Enter));
        assert_eq!(key_from_mac_keycode(0x35), Some(Key::Escape));
        assert_eq!(key_from_mac_keycode(0x00), None);
    }

    #[test]
    fn lparam_points_are_signed_16_bit_pairs() {
        assert_eq!(point_from_lparam((20 << 16) | 10), (10, 20));
        // -1 in both halves (a click captured left of/above the client area).
        assert_eq!(point_from_lparam(0xFFFF_FFFFu32 as i32 as isize), (-1, -1));
        assert_eq!(point_from_lparam(0x0001_FFFEu32 as isize), (-2, 1));
    }

    #[test]
    fn wheel_notches_become_list_rows_and_repeats_are_detected() {
        assert_eq!(scroll_rows_from_wheel(120 << 16), -1);
        assert_eq!(scroll_rows_from_wheel(((-120i16) as u16 as usize) << 16), 1);
        assert_eq!(scroll_rows_from_wheel(240 << 16), -2);
        assert!(is_key_repeat(1 << 30));
        assert!(!is_key_repeat(1));
    }
}
