//! Aceris Android front-end.
//!
//! Rust-only C ABI for an Android host shell. Pixel buffers are borrowed from
//! the owning system handle and remain valid until that handle is destroyed.

use std::ffi::{CStr, c_char};
use std::path::Path;
use std::slice;

use aceris_core::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use aceris_core::video::{BOTTOM_SCREEN_HEIGHT, BOTTOM_SCREEN_WIDTH, ScreenSurface};
use aceris_core::{System, DEFAULT_FRAME_INSTRUCTION_BUDGET};

pub const ACERIS_STATUS_OK: i32 = 0;
pub const ACERIS_STATUS_INVALID_HANDLE: i32 = -1;
pub const ACERIS_STATUS_INVALID_ARGUMENT: i32 = -2;
pub const ACERIS_STATUS_LOAD_FAILED: i32 = -3;
pub const ACERIS_STATUS_FRAME_FAILED: i32 = -4;
pub const ACERIS_STATUS_INVALID_SCREEN: i32 = -5;

/// Opaque emulator instance owned by the C/Android host.
#[repr(C)]
pub struct AcerisSystemHandle {
    system: System,
}

/// Read-only description of one RGBA8 LCD pixel buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AcerisScreenInfo {
    pub width: u32,
    pub height: u32,
    pub stride_bytes: usize,
    pub data_len: usize,
    /// Borrowed pointer; valid until its system handle is destroyed.
    pub pixels: *const u8,
}

impl AcerisScreenInfo {
    const fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            stride_bytes: 0,
            data_len: 0,
            pixels: std::ptr::null(),
        }
    }
}

/// Returns the Aceris version as a static NUL-terminated C string.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Runs the shared core's CPU self-test. Returns -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_selftest() -> i32 {
    aceris_core::selftest::run()
        .ok()
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(-1)
}

/// Creates a new emulator instance. Returns null if allocation fails.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_system_create() -> *mut AcerisSystemHandle {
    Box::into_raw(Box::new(AcerisSystemHandle {
        system: System::new(),
    }))
}

/// Destroys an instance created by aceris_system_create.
///
/// # Safety
/// The pointer must be null or a live, uniquely owned handle returned by
/// aceris_system_create. It must not have been destroyed previously.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_destroy(handle: *mut AcerisSystemHandle) {
    if !handle.is_null() {
        // SAFETY: Guaranteed by this function's caller contract.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Loads a NoCrypto NCSD image (.3ds or .cci) from a UTF-8 path.
///
/// # Safety
/// handle must be a live system instance. path must be a readable,
/// NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_load_no_crypto_file(
    handle: *mut AcerisSystemHandle,
    path: *const c_char,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if path.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: The caller guarantees a readable NUL-terminated string.
    let path = unsafe { CStr::from_ptr(path) };
    let Ok(path) = path.to_str() else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    match handle.system.load_ncsd_no_crypto_file(Path::new(path)) {
        Ok(_) => ACERIS_STATUS_OK,
        Err(_) => ACERIS_STATUS_LOAD_FAILED,
    }
}

/// Loads an NCSD image using 16-byte menu-info and content keys from the caller.
///
/// Aceris copies the keys for the operation and does not persist or discover them.
///
/// # Safety
/// handle must be live; path must be a readable NUL-terminated C string; each
/// key pointer must reference at least 16 readable bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_load_file_with_keys(
    handle: *mut AcerisSystemHandle,
    path: *const c_char,
    menu_info_key: *const u8,
    content_key: *const u8,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if path.is_null() || menu_info_key.is_null() || content_key.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: The caller guarantees a readable NUL-terminated string.
    let path = unsafe { CStr::from_ptr(path) };
    let Ok(path) = path.to_str() else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    // SAFETY: The caller guarantees both key pointers reference AES_BLOCK_SIZE bytes.
    let menu_key: [u8; AES_BLOCK_SIZE] =
        unsafe { slice::from_raw_parts(menu_info_key, AES_BLOCK_SIZE) }
            .try_into()
            .expect("the AES key slice has exactly AES_BLOCK_SIZE bytes");
    // SAFETY: The caller guarantees both key pointers reference AES_BLOCK_SIZE bytes.
    let content_key: [u8; AES_BLOCK_SIZE] =
        unsafe { slice::from_raw_parts(content_key, AES_BLOCK_SIZE) }
            .try_into()
            .expect("the AES key slice has exactly AES_BLOCK_SIZE bytes");

    match handle
        .system
        .load_ncsd_file_with_keys(Path::new(path), menu_key, content_key)
    {
        Ok(_) => ACERIS_STATUS_OK,
        Err(_) => ACERIS_STATUS_LOAD_FAILED,
    }
}

/// Advances one frame with an explicit ARM11 instruction budget.
///
/// # Safety
/// handle must be a live system instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_run_frame(
    handle: *mut AcerisSystemHandle,
    instruction_budget: u64,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    let Ok(budget) = usize::try_from(instruction_budget) else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    match handle.system.run_frame_with_budget(budget) {
        Ok(_) => ACERIS_STATUS_OK,
        Err(_) => ACERIS_STATUS_FRAME_FAILED,
    }
}

/// Advances one frame using the core's default deterministic instruction cap.
///
/// # Safety
/// handle must be a live system instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_run_default_frame(
    handle: *mut AcerisSystemHandle,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    match handle
        .system
        .run_frame_with_budget(DEFAULT_FRAME_INSTRUCTION_BUDGET)
    {
        Ok(_) => ACERIS_STATUS_OK,
        Err(_) => ACERIS_STATUS_FRAME_FAILED,
    }
}

/// Writes the current presentation counter to out_frame_number.
///
/// # Safety
/// handle must be live and out_frame_number must point to writable u64 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_get_frame_number(
    handle: *const AcerisSystemHandle,
    out_frame_number: *mut u64,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    // SAFETY: The caller guarantees writable u64 output storage.
    let Some(output) = (unsafe { out_frame_number.as_mut() }) else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    *output = handle.system.display().frame_number();
    ACERIS_STATUS_OK
}

/// Describes an LCD surface. IDs: 0 top-left, 1 top-right, 2 bottom.
///
/// The pixel pointer is read-only and remains valid until the owning system
/// handle is destroyed. The host must not write through it.
///
/// # Safety
/// handle must be live and out_info must point to writable AcerisScreenInfo storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_get_screen_info(
    handle: *const AcerisSystemHandle,
    screen_id: u32,
    out_info: *mut AcerisScreenInfo,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    // SAFETY: The caller guarantees writable AcerisScreenInfo output storage.
    let Some(output) = (unsafe { out_info.as_mut() }) else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };

    let surface: &ScreenSurface = match screen_id {
        0 => handle.system.display().top_left(),
        1 => handle.system.display().top_right(),
        2 => handle.system.display().bottom(),
        _ => {
            *output = AcerisScreenInfo::empty();
            return ACERIS_STATUS_INVALID_SCREEN;
        }
    };
    *output = AcerisScreenInfo {
        width: surface.width() as u32,
        height: surface.height() as u32,
        stride_bytes: surface.stride_bytes(),
        data_len: surface.pixels().len(),
        pixels: surface.pixels().as_ptr(),
    };
    ACERIS_STATUS_OK
}

/// Returns a screen's fixed logical width, or zero for an invalid ID.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_screen_width(screen_id: u32) -> u32 {
    match screen_id {
        0 | 1 => 400,
        2 => BOTTOM_SCREEN_WIDTH as u32,
        _ => 0,
    }
}

/// Returns a screen's fixed logical height, or zero for an invalid ID.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_screen_height(screen_id: u32) -> u32 {
    match screen_id {
        0 | 1 => 240,
        2 => BOTTOM_SCREEN_HEIGHT as u32,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn version_is_a_valid_c_string() {
        // SAFETY: aceris_version returns a static NUL-terminated string.
        let version = unsafe { CStr::from_ptr(aceris_version()) };
        assert_eq!(version.to_str().unwrap(), aceris_common::VERSION);
    }

    #[test]
    fn selftest_runs_the_shared_core() {
        let expected = i32::try_from(aceris_core::selftest::EXPECTED).unwrap();
        assert_eq!(aceris_selftest(), expected);
    }

    #[test]
    fn android_abi_exposes_the_three_fixed_lcd_surfaces() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());

        for (screen, expected_width, expected_height) in [
            (0, 400, 240),
            (1, 400, 240),
            (2, BOTTOM_SCREEN_WIDTH as u32, BOTTOM_SCREEN_HEIGHT as u32),
        ] {
            let mut info = AcerisScreenInfo::empty();
            // SAFETY: handle is live and info is writable for this call.
            assert_eq!(
                unsafe { aceris_system_get_screen_info(handle, screen, &mut info) },
                ACERIS_STATUS_OK
            );
            assert_eq!(info.width, expected_width);
            assert_eq!(info.height, expected_height);
            assert_eq!(info.stride_bytes, expected_width as usize * 4);
            assert_eq!(
                info.data_len,
                expected_width as usize * expected_height as usize * 4
            );
            assert!(!info.pixels.is_null());
        }

        // SAFETY: handle is live, unique and destroyed exactly once.
        unsafe { aceris_system_destroy(handle) };
    }

    #[test]
    fn invalid_screen_and_null_handles_return_stable_status_codes() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());
        let mut info = AcerisScreenInfo::empty();

        // SAFETY: handle is live and info is writable.
        assert_eq!(
            unsafe { aceris_system_get_screen_info(handle, 3, &mut info) },
            ACERIS_STATUS_INVALID_SCREEN
        );
        assert_eq!(info.width, 0);
        assert!(info.pixels.is_null());

        // SAFETY: Destroy explicitly accepts null.
        unsafe { aceris_system_destroy(std::ptr::null_mut()) };
        // SAFETY: A null handle cannot be dereferenced.
        assert_eq!(
            unsafe { aceris_system_run_frame(std::ptr::null_mut(), 1) },
            ACERIS_STATUS_INVALID_HANDLE
        );
        // SAFETY: This is the single live allocation, destroyed once.
        unsafe { aceris_system_destroy(handle) };
    }
}
