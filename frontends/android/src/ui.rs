//! Aceris start-up and library UI for the Android host shell.
//!
//! The host only blits RGBA frames and forwards input: the intro animation and
//! the game library are drawn by the same shared Rust code as on the desktop,
//! so both versions keep the identity in `docs/IDENTIDADE_VISUAL.md`.
//! Frames are `aceris_ui_canvas_width() x aceris_ui_canvas_height()` RGBA8.

use std::ffi::{CStr, c_char};
use std::path::Path;
use std::slice;

use aceris_core::video::intro;
use aceris_core::video::library::{self, ClickResult, Library};

use crate::{ACERIS_STATUS_INVALID_ARGUMENT, ACERIS_STATUS_INVALID_HANDLE, ACERIS_STATUS_OK};

/// Opaque game library owned by the host.
pub struct AcerisLibraryHandle {
    library: Library,
}

const FRAME_BYTES: usize = library::WIDTH * library::HEIGHT * 4;

#[unsafe(no_mangle)]
pub extern "C" fn aceris_ui_canvas_width() -> u32 {
    library::WIDTH as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn aceris_ui_canvas_height() -> u32 {
    library::HEIGHT as u32
}

/// Number of frames in the start-up animation.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_intro_frame_count() -> u32 {
    intro::FRAME_COUNT
}

/// Frames per second the animation is designed for.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_intro_fps() -> u32 {
    intro::FPS
}

/// Writes intro frame `frame` (clamped to the last) into `output` as RGBA8.
///
/// # Safety
/// output must point to at least `capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_intro_render_frame(
    frame: u32,
    output: *mut u8,
    capacity: usize,
) -> i32 {
    let rgba = intro::render_frame(frame, library::WIDTH, library::HEIGHT);
    write_frame(&rgba, output, capacity)
}

fn write_frame(rgba: &[u8], output: *mut u8, capacity: usize) -> i32 {
    if output.is_null() || capacity < FRAME_BYTES || rgba.len() != FRAME_BYTES {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: output is non-null and the caller guarantees `capacity` bytes.
    unsafe { slice::from_raw_parts_mut(output, FRAME_BYTES) }.copy_from_slice(rgba);
    ACERIS_STATUS_OK
}

/// Scans `directory` (UTF-8, NUL-terminated) for `.3ds`/`.cci` games.
/// Returns null if the argument is invalid.
///
/// # Safety
/// directory must be a readable NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_create(
    directory: *const c_char,
) -> *mut AcerisLibraryHandle {
    if directory.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: checked non-null; the caller guarantees NUL termination.
    let Ok(text) = unsafe { CStr::from_ptr(directory) }.to_str() else {
        return std::ptr::null_mut();
    };
    let entries = library::scan_games(Path::new(text));
    Box::into_raw(Box::new(AcerisLibraryHandle {
        library: Library::new(entries, text.to_string()),
    }))
}

/// # Safety
/// handle must come from `aceris_library_create` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_destroy(handle: *mut AcerisLibraryHandle) {
    if !handle.is_null() {
        // SAFETY: the handle was created by Box::into_raw above.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Number of games, or a negative status for a bad handle.
///
/// # Safety
/// handle must be live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_count(handle: *const AcerisLibraryHandle) -> isize {
    // SAFETY: the caller guarantees the handle's lifetime.
    match unsafe { handle.as_ref() } {
        Some(h) => h.library.entries().len() as isize,
        None => ACERIS_STATUS_INVALID_HANDLE as isize,
    }
}

/// Index of the selected game (0 when the library is empty).
///
/// # Safety
/// handle must be live or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_selected_index(
    handle: *const AcerisLibraryHandle,
) -> isize {
    // SAFETY: the caller guarantees the handle's lifetime.
    match unsafe { handle.as_ref() } {
        Some(h) => h.library.selected_index() as isize,
        None => ACERIS_STATUS_INVALID_HANDLE as isize,
    }
}

/// Copies the selected game's path as a NUL-terminated string. Follows the
/// same contract as `aceris_system_copy_last_error`: pass capacity 0 to get
/// the required length; returns -1 for a bad handle and -2 when nothing is
/// selected or the buffer is null with a non-zero capacity.
///
/// # Safety
/// handle must be live; output must have `capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_copy_selected_path(
    handle: *const AcerisLibraryHandle,
    output: *mut c_char,
    capacity: usize,
) -> isize {
    // SAFETY: the caller guarantees the handle's lifetime.
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE as isize;
    };
    let Some(entry) = h.library.selected_entry() else {
        return ACERIS_STATUS_INVALID_ARGUMENT as isize;
    };
    let path = entry.path.to_string_lossy();
    let required = path.len() as isize;
    if capacity == 0 {
        return required;
    }
    if output.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT as isize;
    }
    let copied = path.len().min(capacity - 1);
    // SAFETY: the caller guarantees `capacity` writable bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(path.as_ptr(), output.cast::<u8>(), copied);
        *output.add(copied) = 0;
    }
    required
}

/// Moves the selection by `delta` rows (for swipes, d-pad or wheel).
///
/// # Safety
/// handle must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_move_selection(
    handle: *mut AcerisLibraryHandle,
    delta: i32,
) -> i32 {
    // SAFETY: the caller guarantees the handle's lifetime.
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    h.library.move_selection(delta);
    ACERIS_STATUS_OK
}

/// Handles a tap at canvas coordinates. Returns 0 if ignored, 1 if a row was
/// selected, 2 if the selected row was tapped again (launch it), or a
/// negative status.
///
/// # Safety
/// handle must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_tap(
    handle: *mut AcerisLibraryHandle,
    x: i32,
    y: i32,
) -> i32 {
    // SAFETY: the caller guarantees the handle's lifetime.
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    match h.library.click(x, y) {
        ClickResult::Ignored => 0,
        ClickResult::Selected => 1,
        ClickResult::Activated => 2,
    }
}

/// Shows `message` (UTF-8, NUL-terminated) above the footer, e.g. a load error.
///
/// # Safety
/// handle must be live; message must be a NUL-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_set_status(
    handle: *mut AcerisLibraryHandle,
    message: *const c_char,
) -> i32 {
    // SAFETY: the caller guarantees the handle's lifetime.
    let Some(h) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if message.is_null() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: checked non-null; the caller guarantees NUL termination.
    h.library.set_status(
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned(),
    );
    ACERIS_STATUS_OK
}

/// Draws the library into `output` as RGBA8 (`canvas width * height * 4` bytes).
///
/// # Safety
/// handle must be live; output must have `capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_render(
    handle: *const AcerisLibraryHandle,
    output: *mut u8,
    capacity: usize,
) -> i32 {
    // SAFETY: the caller guarantees the handle's lifetime.
    let Some(h) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    write_frame(&h.library.render(), output, capacity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;

    #[test]
    fn intro_frames_render_into_host_buffers() {
        assert_eq!(
            aceris_ui_canvas_width() * aceris_ui_canvas_height() * 4,
            FRAME_BYTES as u32
        );
        assert_eq!(aceris_intro_frame_count(), intro::FRAME_COUNT);
        let mut buffer = vec![0u8; FRAME_BYTES];
        // SAFETY: buffer has FRAME_BYTES writable bytes.
        unsafe {
            assert_eq!(
                aceris_intro_render_frame(110, buffer.as_mut_ptr(), buffer.len()),
                0
            );
            assert_eq!(
                aceris_intro_render_frame(0, buffer.as_mut_ptr(), buffer.len() - 1),
                ACERIS_STATUS_INVALID_ARGUMENT
            );
            assert_eq!(
                aceris_intro_render_frame(0, std::ptr::null_mut(), FRAME_BYTES),
                ACERIS_STATUS_INVALID_ARGUMENT
            );
        }
        assert!(buffer.iter().any(|&b| b != 0));
    }

    #[test]
    fn library_scans_selects_taps_and_renders() {
        let dir = std::env::temp_dir().join(format!("aceris-android-lib-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["one.3ds", "two.cci", "skip.txt"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let c_dir = CString::new(dir.to_str().unwrap()).unwrap();
        // SAFETY: all pointers are valid for the duration of each call and the
        // handle is destroyed exactly once at the end.
        unsafe {
            let handle = aceris_library_create(c_dir.as_ptr());
            assert!(!handle.is_null());
            assert_eq!(aceris_library_count(handle), 2);
            assert_eq!(aceris_library_selected_index(handle), 0);
            assert_eq!(aceris_library_move_selection(handle, 1), 0);
            assert_eq!(aceris_library_selected_index(handle), 1);

            let needed = aceris_library_copy_selected_path(handle, std::ptr::null_mut(), 0);
            let mut path = vec![0 as c_char; needed as usize + 1];
            aceris_library_copy_selected_path(handle, path.as_mut_ptr(), path.len());
            let path = CStr::from_ptr(path.as_ptr()).to_str().unwrap().to_string();
            assert!(path.ends_with("two.cci"), "{path}");

            // Row 0 is at y ~ 64..92 on the canvas: select it, then tap again.
            assert_eq!(aceris_library_tap(handle, 100, 70), 1);
            assert_eq!(aceris_library_tap(handle, 100, 70), 2);
            assert_eq!(aceris_library_tap(handle, 100, 5), 0);

            let message = CString::new("COULD NOT LOAD").unwrap();
            assert_eq!(aceris_library_set_status(handle, message.as_ptr()), 0);
            let mut frame = vec![0u8; FRAME_BYTES];
            assert_eq!(
                aceris_library_render(handle, frame.as_mut_ptr(), frame.len()),
                0
            );
            assert!(frame.chunks_exact(4).all(|p| p[3] == 255));

            aceris_library_destroy(handle);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_handles_and_arguments_are_rejected() {
        // SAFETY: null handles are explicitly allowed by these functions.
        unsafe {
            assert!(aceris_library_create(std::ptr::null()).is_null());
            assert_eq!(
                aceris_library_count(std::ptr::null()),
                ACERIS_STATUS_INVALID_HANDLE as isize
            );
            assert_eq!(
                aceris_library_tap(std::ptr::null_mut(), 0, 0),
                ACERIS_STATUS_INVALID_HANDLE
            );
            assert_eq!(
                aceris_library_render(std::ptr::null(), std::ptr::null_mut(), 0),
                ACERIS_STATUS_INVALID_HANDLE
            );
            aceris_library_destroy(std::ptr::null_mut());
        }
    }
}
