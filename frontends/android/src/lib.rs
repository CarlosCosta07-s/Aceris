//! Aceris Android front-end.
//!
//! Rust-only C ABI for an Android host shell. Pixel buffers are borrowed from
//! the owning system handle and remain valid until that handle is destroyed.

#[cfg(any(target_os = "android", aceris_typecheck_native, test))]
mod app;
#[cfg(any(target_os = "android", aceris_typecheck_native))]
mod native_activity;
mod ui;

use std::ffi::{CStr, c_char};
use std::path::Path;
use std::slice;

use aceris_core::input::{pad_from_frontend_buttons, touch_from_surface_point};
use aceris_core::loader::cia::{COMMON_KEY_COUNT, TicketCommonKeyStore};
use aceris_core::loader::ncsd::ncch::crypto::AES_BLOCK_SIZE;
use aceris_core::video::frontend::compute_screen_layout;
use aceris_core::video::{BOTTOM_SCREEN_HEIGHT, BOTTOM_SCREEN_WIDTH, ScreenSurface};
use aceris_core::{DEFAULT_FRAME_INSTRUCTION_BUDGET, System};

pub const ACERIS_STATUS_OK: i32 = 0;
pub const ACERIS_STATUS_INVALID_HANDLE: i32 = -1;
pub const ACERIS_STATUS_INVALID_ARGUMENT: i32 = -2;
pub const ACERIS_STATUS_LOAD_FAILED: i32 = -3;
pub const ACERIS_STATUS_FRAME_FAILED: i32 = -4;
pub const ACERIS_STATUS_INVALID_SCREEN: i32 = -5;
pub const ACERIS_STATUS_LAST_ERROR_INVALID_ARGUMENT: isize = -2;

/// `common_key_kind` value for a caller-supplied ticket KeyY (requires KeyX).
pub const ACERIS_CIA_KEY_MATERIAL_KEY_Y: u32 = 0;
/// `common_key_kind` value for a caller-supplied, already-derived normal key.
pub const ACERIS_CIA_KEY_MATERIAL_NORMAL: u32 = 1;

/// Opaque emulator instance owned by the C/Android host.
#[repr(C)]
pub struct AcerisSystemHandle {
    system: System,
    last_error: Option<String>,
}

/// CPU state exposed for deterministic boot diagnostics on every frontend.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AcerisCpuState {
    pub program_counter: u32,
    pub cpsr: u32,
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
        last_error: None,
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

/// Copies the most recent load/frame error into a caller-owned UTF-8 buffer.
///
/// Pass a null buffer with capacity 0 to query the required byte length.
/// The returned length excludes the terminating NUL. A non-null buffer is
/// always NUL-terminated when capacity is greater than zero; truncated
/// messages retain the returned full length so the caller can retry.
///
/// # Safety
/// handle must be live. When capacity is greater than zero, output must point
/// to at least capacity writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_copy_last_error(
    handle: *const AcerisSystemHandle,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE as isize;
    };

    let Some(error) = handle.last_error.as_deref() else {
        if capacity == 0 {
            return 0;
        }
        if output.is_null() {
            return ACERIS_STATUS_LAST_ERROR_INVALID_ARGUMENT;
        }
        // SAFETY: The caller guarantees writable storage of capacity bytes.
        unsafe { *output = 0 };
        return 0;
    };

    let required = isize::try_from(error.len()).unwrap_or(isize::MAX);
    if capacity == 0 {
        return required;
    }
    if output.is_null() {
        return ACERIS_STATUS_LAST_ERROR_INVALID_ARGUMENT;
    }

    let copied = error.len().min(capacity - 1);
    // SAFETY: The caller guarantees writable storage of capacity bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(error.as_ptr(), output.cast::<u8>(), copied);
        *output.add(copied) = 0;
    }
    required
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
        Ok(_) => {
            handle.last_error = None;
            ACERIS_STATUS_OK
        }
        Err(error) => {
            handle.last_error = Some(error.to_string());
            ACERIS_STATUS_LOAD_FAILED
        }
    }
}

/// Loads an executable supported by the shared core from a UTF-8 path.
///
/// Currently accepts standard 3DSX homebrew, unencrypted NCSD cartridge
/// images (.3ds/.cci), and CIA archives whose selected boot content is clear
/// and uses NCCH NoCrypto. Encrypted NCSD still uses the explicit-key entry
/// point below; encrypted CIA content is not yet supported by this path.
///
/// # Safety
/// handle must be a live system instance. path must be a readable,
/// NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_load_executable_file(
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
    match handle.system.load_executable_file(Path::new(path)) {
        Ok(_) => {
            handle.last_error = None;
            ACERIS_STATUS_OK
        }
        Err(error) => {
            handle.last_error = Some(error.to_string());
            ACERIS_STATUS_LOAD_FAILED
        }
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
        Ok(_) => {
            handle.last_error = None;
            ACERIS_STATUS_OK
        }
        Err(error) => {
            handle.last_error = Some(error.to_string());
            ACERIS_STATUS_LOAD_FAILED
        }
    }
}

/// Loads a CIA using caller-supplied ticket common-key and NCCH key material.
///
/// The common-key index must be in 0..COMMON_KEY_COUNT. For
/// ACERIS_CIA_KEY_MATERIAL_KEY_Y, common_key contains KeyY and key_x is
/// required. For ACERIS_CIA_KEY_MATERIAL_NORMAL, common_key is the already
/// derived normal key and key_x must be null. All keys are 16 bytes.
/// The temporary key store is created for this call only; keys are not
/// retained by the emulator handle or written to disk.
///
/// # Safety
/// handle must be live; path must be a readable NUL-terminated UTF-8 string;
/// common_key, menu_info_key and content_key must each reference 16 readable
/// bytes. key_x must reference 16 readable bytes in KeyY mode and must be
/// null in normal-key mode. Key buffers must not overlap the mutable handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_load_cia_file_with_keys(
    handle: *mut AcerisSystemHandle,
    path: *const c_char,
    ticket_common_key_index: u32,
    common_key_kind: u32,
    common_key: *const u8,
    key_x: *const u8,
    menu_info_key: *const u8,
    content_key: *const u8,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if path.is_null() || common_key.is_null() || menu_info_key.is_null() || content_key.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }

    let Ok(ticket_common_key_index) = u8::try_from(ticket_common_key_index) else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    if usize::from(ticket_common_key_index) >= COMMON_KEY_COUNT {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    if common_key_kind != ACERIS_CIA_KEY_MATERIAL_KEY_Y
        && common_key_kind != ACERIS_CIA_KEY_MATERIAL_NORMAL
    {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    if common_key_kind == ACERIS_CIA_KEY_MATERIAL_KEY_Y && key_x.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    if common_key_kind == ACERIS_CIA_KEY_MATERIAL_NORMAL && !key_x.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }

    // SAFETY: The caller guarantees every non-null key pointer references 16 readable bytes.
    let common_key_bytes: [u8; AES_BLOCK_SIZE] =
        unsafe { slice::from_raw_parts(common_key, AES_BLOCK_SIZE) }
            .try_into()
            .expect("the AES key slice has exactly AES_BLOCK_SIZE bytes");
    // SAFETY: The caller guarantees these pointers reference AES_BLOCK_SIZE readable bytes.
    let menu_key: [u8; AES_BLOCK_SIZE] =
        unsafe { slice::from_raw_parts(menu_info_key, AES_BLOCK_SIZE) }
            .try_into()
            .expect("the AES key slice has exactly AES_BLOCK_SIZE bytes");
    // SAFETY: The caller guarantees these pointers reference AES_BLOCK_SIZE readable bytes.
    let content_key: [u8; AES_BLOCK_SIZE] =
        unsafe { slice::from_raw_parts(content_key, AES_BLOCK_SIZE) }
            .try_into()
            .expect("the AES key slice has exactly AES_BLOCK_SIZE bytes");

    let mut key_store = TicketCommonKeyStore::default();
    let key_result = if common_key_kind == ACERIS_CIA_KEY_MATERIAL_KEY_Y {
        // SAFETY: The caller guarantees key_x references AES_BLOCK_SIZE readable bytes.
        let key_x_bytes: [u8; AES_BLOCK_SIZE] =
            unsafe { slice::from_raw_parts(key_x, AES_BLOCK_SIZE) }
                .try_into()
                .expect("the AES key slice has exactly AES_BLOCK_SIZE bytes");
        key_store.set_key_x(key_x_bytes);
        key_store.set_key_y(ticket_common_key_index, common_key_bytes)
    } else {
        key_store.set_normal_key(ticket_common_key_index, common_key_bytes)
    };
    if let Err(error) = key_result {
        handle.last_error = Some(error.to_string());
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }

    // SAFETY: The caller guarantees path is readable and NUL-terminated.
    let path = unsafe { CStr::from_ptr(path) };
    let Ok(path) = path.to_str() else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };

    match handle
        .system
        .load_cia_file_with_keys(Path::new(path), &key_store, menu_key, content_key)
    {
        Ok(_) => {
            handle.last_error = None;
            ACERIS_STATUS_OK
        }
        Err(error) => {
            handle.last_error = Some(error.to_string());
            ACERIS_STATUS_LOAD_FAILED
        }
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
        Ok(_) => {
            handle.last_error = None;
            ACERIS_STATUS_OK
        }
        Err(error) => {
            handle.last_error = Some(error.to_string());
            ACERIS_STATUS_FRAME_FAILED
        }
    }
}

/// Advances one frame using the core's default deterministic instruction cap.
///
/// # Safety
/// handle must be a live system instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_run_default_frame(handle: *mut AcerisSystemHandle) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    match handle
        .system
        .run_frame_with_budget(DEFAULT_FRAME_INSTRUCTION_BUDGET)
    {
        Ok(_) => {
            handle.last_error = None;
            ACERIS_STATUS_OK
        }
        Err(error) => {
            handle.last_error = Some(error.to_string());
            ACERIS_STATUS_FRAME_FAILED
        }
    }
}

/// Writes the current ARM11 PC and CPSR to a caller-owned structure.
///
/// # Safety
/// handle must be live and out_state must point to writable AcerisCpuState storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_get_cpu_state(
    handle: *const AcerisSystemHandle,
    out_state: *mut AcerisCpuState,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    // SAFETY: The caller guarantees writable AcerisCpuState output storage.
    let Some(output) = (unsafe { out_state.as_mut() }) else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };

    *output = AcerisCpuState {
        program_counter: handle.system.cpu().pc(),
        cpsr: handle.system.cpu().cpsr.0,
    };
    ACERIS_STATUS_OK
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

/// Replaces the held controller buttons with a mask of the frontend virtual
/// control bits (`aceris_core::video::frontend::buttons`). Unknown bits are
/// ignored.
///
/// # Safety
/// handle must be live and not used concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_set_buttons(
    handle: *mut AcerisSystemHandle,
    frontend_mask: u32,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime and exclusivity.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    handle
        .system
        .input_mut()
        .set_buttons(pad_from_frontend_buttons(frontend_mask));
    ACERIS_STATUS_OK
}

/// Sets the circle pad deflection; each axis is clamped to the documented range.
///
/// # Safety
/// handle must be live and not used concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_set_circle_pad(
    handle: *mut AcerisSystemHandle,
    x: i32,
    y: i32,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime and exclusivity.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    handle.system.input_mut().set_circle_pad(x, y);
    ACERIS_STATUS_OK
}

/// Reports a touch at a point of the host surface (pixels, origin top-left).
///
/// The surface is laid out with the same rule as the rendered composition for
/// `orientation` (0 portrait, 1 landscape). A point outside the bottom screen
/// ends the current touch, so a drag that leaves the screen releases it.
///
/// # Safety
/// handle must be live and not used concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_touch(
    handle: *mut AcerisSystemHandle,
    surface_width: u32,
    surface_height: u32,
    orientation: u32,
    x: i32,
    y: i32,
) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime and exclusivity.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if surface_width == 0 || surface_height == 0 {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    let layout = compute_screen_layout(surface_width, surface_height, orientation);
    match touch_from_surface_point(&layout, x, y) {
        Some((tx, ty)) => handle
            .system
            .input_mut()
            .touch(i32::from(tx), i32::from(ty)),
        None => handle.system.input_mut().touch_end(),
    }
    ACERIS_STATUS_OK
}

/// Ends the current touch, if any.
///
/// # Safety
/// handle must be live and not used concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_touch_release(handle: *mut AcerisSystemHandle) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime and exclusivity.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    handle.system.input_mut().touch_end();
    ACERIS_STATUS_OK
}

/// Releases every button, the circle pad and the touch, for example when the
/// app loses focus.
///
/// # Safety
/// handle must be live and not used concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_system_input_reset(handle: *mut AcerisSystemHandle) -> i32 {
    // SAFETY: The caller guarantees the handle's lifetime and exclusivity.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    handle.system.input_mut().reset();
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
    use std::ffi::{CStr, CString};

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
    fn android_abi_exposes_the_last_load_error() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());
        let path = CString::new("/aceris-test-file-that-does-not-exist.3ds").unwrap();

        // SAFETY: handle is live and path is a valid NUL-terminated string.
        assert_eq!(
            unsafe { aceris_system_load_no_crypto_file(handle, path.as_ptr()) },
            ACERIS_STATUS_LOAD_FAILED
        );

        // SAFETY: A zero-capacity query does not dereference the null output.
        let required = unsafe { aceris_system_copy_last_error(handle, std::ptr::null_mut(), 0) };
        assert!(required > 0);

        let mut buffer = vec![0u8; required as usize + 1];
        // SAFETY: buffer has the queried number of bytes plus a NUL terminator.
        let copied = unsafe {
            aceris_system_copy_last_error(
                handle,
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
            )
        };
        assert_eq!(copied, required);
        // SAFETY: The getter writes a NUL-terminated UTF-8 error string.
        let message = unsafe { CStr::from_ptr(buffer.as_ptr().cast::<c_char>()) }
            .to_str()
            .unwrap();
        assert!(!message.is_empty());

        // SAFETY: handle is live, unique and destroyed exactly once.
        unsafe { aceris_system_destroy(handle) };
    }

    #[test]
    fn android_abi_rejects_invalid_cia_key_parameters() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());
        let path = CString::new("not-opened.cia").unwrap();
        let common_key = [0x11u8; AES_BLOCK_SIZE];
        let menu_key = [0x22u8; AES_BLOCK_SIZE];
        let content_key = [0x33u8; AES_BLOCK_SIZE];

        // SAFETY: The handle and path are live; all non-null key arrays are 16 readable bytes.
        assert_eq!(
            unsafe {
                aceris_system_load_cia_file_with_keys(
                    handle,
                    path.as_ptr(),
                    COMMON_KEY_COUNT as u32,
                    ACERIS_CIA_KEY_MATERIAL_NORMAL,
                    common_key.as_ptr(),
                    std::ptr::null(),
                    menu_key.as_ptr(),
                    content_key.as_ptr(),
                )
            },
            ACERIS_STATUS_INVALID_ARGUMENT
        );

        // SAFETY: The handle and path are live; all key arrays are 16 readable bytes.
        assert_eq!(
            unsafe {
                aceris_system_load_cia_file_with_keys(
                    handle,
                    path.as_ptr(),
                    0,
                    2,
                    common_key.as_ptr(),
                    std::ptr::null(),
                    menu_key.as_ptr(),
                    content_key.as_ptr(),
                )
            },
            ACERIS_STATUS_INVALID_ARGUMENT
        );

        // SAFETY: The handle and path are live; non-null key arrays are 16 readable bytes.
        assert_eq!(
            unsafe {
                aceris_system_load_cia_file_with_keys(
                    handle,
                    path.as_ptr(),
                    0,
                    ACERIS_CIA_KEY_MATERIAL_KEY_Y,
                    common_key.as_ptr(),
                    std::ptr::null(),
                    menu_key.as_ptr(),
                    content_key.as_ptr(),
                )
            },
            ACERIS_STATUS_INVALID_ARGUMENT
        );

        // SAFETY: The handle and path are live; all key arrays are 16 readable bytes.
        assert_eq!(
            unsafe {
                aceris_system_load_cia_file_with_keys(
                    handle,
                    path.as_ptr(),
                    0,
                    ACERIS_CIA_KEY_MATERIAL_NORMAL,
                    common_key.as_ptr(),
                    common_key.as_ptr(),
                    menu_key.as_ptr(),
                    content_key.as_ptr(),
                )
            },
            ACERIS_STATUS_INVALID_ARGUMENT
        );

        // SAFETY: This is the unique live allocation, destroyed exactly once.
        unsafe { aceris_system_destroy(handle) };
    }

    #[test]
    fn android_abi_routes_key_y_and_normal_key_configurations_to_the_core_loader() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());
        let path = std::env::temp_dir().join(format!(
            "aceris-android-missing-cia-{}.cia",
            std::process::id()
        ));
        let path = CString::new(path.to_str().unwrap()).unwrap();
        let common_key = [0x11u8; AES_BLOCK_SIZE];
        let key_x = [0x44u8; AES_BLOCK_SIZE];
        let menu_key = [0x22u8; AES_BLOCK_SIZE];
        let content_key = [0x33u8; AES_BLOCK_SIZE];

        // SAFETY: The handle/path and all fixed-size key buffers are valid for this call.
        assert_eq!(
            unsafe {
                aceris_system_load_cia_file_with_keys(
                    handle,
                    path.as_ptr(),
                    0,
                    ACERIS_CIA_KEY_MATERIAL_KEY_Y,
                    common_key.as_ptr(),
                    key_x.as_ptr(),
                    menu_key.as_ptr(),
                    content_key.as_ptr(),
                )
            },
            ACERIS_STATUS_LOAD_FAILED
        );

        // SAFETY: The handle/path and all fixed-size key buffers are valid for this call.
        assert_eq!(
            unsafe {
                aceris_system_load_cia_file_with_keys(
                    handle,
                    path.as_ptr(),
                    0,
                    ACERIS_CIA_KEY_MATERIAL_NORMAL,
                    common_key.as_ptr(),
                    std::ptr::null(),
                    menu_key.as_ptr(),
                    content_key.as_ptr(),
                )
            },
            ACERIS_STATUS_LOAD_FAILED
        );

        // SAFETY: The handle is uniquely owned and destroyed exactly once.
        unsafe { aceris_system_destroy(handle) };
    }

    fn minimal_3dsx_fixture() -> Vec<u8> {
        let instructions = [0xE3A0_0007u32, 0xE3A0_1005, 0xE080_2001];
        let code_size = instructions.len() * 4;
        let mut image = vec![0u8; 0x20 + 3 * 8 + code_size];
        image[..4].copy_from_slice(b"3DSX");
        image[0x04..0x06].copy_from_slice(&0x20u16.to_le_bytes());
        image[0x06..0x08].copy_from_slice(&8u16.to_le_bytes());
        image[0x10..0x14].copy_from_slice(&(code_size as u32).to_le_bytes());
        for (index, instruction) in instructions.into_iter().enumerate() {
            let offset = 0x20 + 3 * 8 + index * 4;
            image[offset..offset + 4].copy_from_slice(&instruction.to_le_bytes());
        }
        image
    }

    #[test]
    fn android_abi_loads_and_executes_3dsx_homebrew() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());
        let path = std::env::temp_dir().join(format!(
            "aceris-android-homebrew-{}.3dsx",
            std::process::id()
        ));
        std::fs::write(&path, minimal_3dsx_fixture()).unwrap();
        let path = CString::new(path.to_str().unwrap()).unwrap();

        // SAFETY: handle is live and path is a readable NUL-terminated string.
        assert_eq!(
            unsafe { aceris_system_load_executable_file(handle, path.as_ptr()) },
            ACERIS_STATUS_OK
        );

        let mut before = AcerisCpuState::default();
        // SAFETY: handle is live and before is writable.
        assert_eq!(
            unsafe { aceris_system_get_cpu_state(handle, &mut before) },
            ACERIS_STATUS_OK
        );
        assert_eq!(before.program_counter, 0x0010_0000);

        let mut frame_number = u64::MAX;
        // SAFETY: handle is live and frame_number is writable.
        assert_eq!(
            unsafe { aceris_system_get_frame_number(handle, &mut frame_number) },
            ACERIS_STATUS_OK
        );
        assert_eq!(frame_number, 0);

        // SAFETY: handle is live and the instruction budget is a valid small value.
        assert_eq!(
            unsafe { aceris_system_run_frame(handle, 3) },
            ACERIS_STATUS_OK
        );

        // A successful frame must advance the presentation counter exactly once.
        // SAFETY: handle is live and frame_number is writable.
        assert_eq!(
            unsafe { aceris_system_get_frame_number(handle, &mut frame_number) },
            ACERIS_STATUS_OK
        );
        assert_eq!(frame_number, 1);

        let mut after = AcerisCpuState::default();
        // SAFETY: handle is live and after is writable.
        assert_eq!(
            unsafe { aceris_system_get_cpu_state(handle, &mut after) },
            ACERIS_STATUS_OK
        );
        assert_eq!(after.program_counter, 0x0010_000C);

        // SAFETY: handle is live, uniquely owned and destroyed exactly once.
        unsafe { aceris_system_destroy(handle) };
    }

    #[test]
    fn android_abi_exposes_the_three_fixed_lcd_surfaces() {
        let handle = aceris_system_create();
        assert!(!handle.is_null());

        let mut cpu_state = AcerisCpuState::default();
        // SAFETY: handle is live and cpu_state is writable.
        assert_eq!(
            unsafe { aceris_system_get_cpu_state(handle, &mut cpu_state) },
            ACERIS_STATUS_OK
        );

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

    #[test]
    fn input_abi_feeds_buttons_circle_pad_and_touch() {
        use aceris_core::input::pad;
        use aceris_core::video::frontend::{ORIENTATION_LANDSCAPE, buttons};
        let handle = aceris_system_create();
        // SAFETY: the handle was just created and is destroyed below.
        unsafe {
            assert_eq!(
                aceris_system_set_buttons(handle, buttons::A | buttons::START | 0xF000_0000),
                ACERIS_STATUS_OK
            );
            assert_eq!(
                aceris_system_set_circle_pad(handle, 900, -900),
                ACERIS_STATUS_OK
            );
            let state = (*handle).system.input_mut().snapshot();
            assert_eq!(state.held, pad::A | pad::START);
            assert_eq!(state.circle_pad, (156, -156));

            // A point in the bottom screen touches; one outside releases.
            let layout = compute_screen_layout(1000, 1000, ORIENTATION_LANDSCAPE);
            let inside = (layout.bottom.x + 1, layout.bottom.y + 1);
            assert_eq!(
                aceris_system_touch(
                    handle,
                    1000,
                    1000,
                    ORIENTATION_LANDSCAPE,
                    inside.0,
                    inside.1
                ),
                ACERIS_STATUS_OK
            );
            assert!((*handle).system.input_mut().snapshot().touch.is_some());
            assert_eq!(
                aceris_system_touch(
                    handle,
                    1000,
                    1000,
                    ORIENTATION_LANDSCAPE,
                    layout.top.x,
                    layout.top.y
                ),
                ACERIS_STATUS_OK
            );
            assert_eq!((*handle).system.input_mut().snapshot().touch, None);

            aceris_system_touch(
                handle,
                1000,
                1000,
                ORIENTATION_LANDSCAPE,
                inside.0,
                inside.1,
            );
            assert_eq!(aceris_system_touch_release(handle), ACERIS_STATUS_OK);
            assert_eq!((*handle).system.input_mut().snapshot().touch, None);

            assert_eq!(aceris_system_input_reset(handle), ACERIS_STATUS_OK);
            assert_eq!((*handle).system.input_mut().snapshot().held, 0);
            assert_eq!(
                aceris_system_touch(handle, 0, 1000, ORIENTATION_LANDSCAPE, 0, 0),
                ACERIS_STATUS_INVALID_ARGUMENT
            );
            aceris_system_destroy(handle);
        }
        // SAFETY: null handles are rejected without being dereferenced.
        unsafe {
            assert_eq!(
                aceris_system_set_buttons(std::ptr::null_mut(), 1),
                ACERIS_STATUS_INVALID_HANDLE
            );
            assert_eq!(
                aceris_system_touch_release(std::ptr::null_mut()),
                ACERIS_STATUS_INVALID_HANDLE
            );
        }
    }
}
