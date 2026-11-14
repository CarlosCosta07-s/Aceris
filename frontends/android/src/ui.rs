//! Aceris start-up and library UI for the Android host shell.
//!
//! The host only blits RGBA frames and forwards input: the intro animation and
//! the game library are drawn by the same shared Rust code as on the desktop,
//! so both versions keep the identity in `docs/IDENTIDADE_VISUAL.md`.
//! Frames are `aceris_ui_canvas_width() x aceris_ui_canvas_height()` RGBA8.

use std::ffi::{CStr, c_char};
use std::path::Path;
use std::slice;

use aceris_core::video::frontend::{
    self, ScreenLayout, ORIENTATION_AUTO, ORIENTATION_LANDSCAPE, ORIENTATION_PORTRAIT,
};
use aceris_core::video::intro;
use aceris_core::video::library::{self, ClickResult, Library};

use crate::{ACERIS_STATUS_INVALID_ARGUMENT, ACERIS_STATUS_INVALID_HANDLE, ACERIS_STATUS_OK};

/// Opaque game library owned by the host.
pub struct AcerisLibraryHandle {
    library: Library,
    /// Requested orientation; the Android host applies it to its Activity.
    orientation: u32,
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
        orientation: ORIENTATION_AUTO,
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
    write_frame(&h.library.render_animated(0, None), output, capacity)
}


/// Requests automatic, portrait or landscape presentation for this library.
///
/// Values: 0 = automatic, 1 = portrait, 2 = landscape. The Android host
/// should read this value and apply the matching Activity orientation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_set_orientation(
    handle: *mut AcerisLibraryHandle,
    orientation: u32,
) -> i32 {
    // SAFETY: the caller guarantees that the handle is live.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if !matches!(orientation, ORIENTATION_AUTO | ORIENTATION_PORTRAIT | ORIENTATION_LANDSCAPE) {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    handle.orientation = orientation;
    ACERIS_STATUS_OK
}

/// Returns the requested orientation, or a negative status for an invalid handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_get_orientation(
    handle: *const AcerisLibraryHandle,
) -> i32 {
    // SAFETY: the caller guarantees that the handle is live.
    match unsafe { handle.as_ref() } {
        Some(handle) => handle.orientation as i32,
        None => ACERIS_STATUS_INVALID_HANDLE,
    }
}

/// Computes screen rectangles using this library's requested orientation.
///
/// # Safety
/// handle must be live; output must point to writable ScreenLayout storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_get_screen_layout(
    handle: *const AcerisLibraryHandle,
    view_width: u32,
    view_height: u32,
    output: *mut ScreenLayout,
) -> i32 {
    // SAFETY: the caller guarantees that the handle is live.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    // SAFETY: the caller guarantees writable ScreenLayout storage.
    let Some(output) = (unsafe { output.as_mut() }) else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    *output = frontend::compute_screen_layout(view_width, view_height, handle.orientation);
    ACERIS_STATUS_OK
}

/// Renders an animated library frame with an optional host-decoded RGBA8
/// background frame. Background frames must be exactly 400x480 RGBA8 bytes.
///
/// # Safety
/// handle must be live; output must have capacity writable bytes. When a
/// background pointer is non-null, it must reference FRAME_BYTES readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_render_animated(
    handle: *const AcerisLibraryHandle,
    animation_frame: u64,
    background_pixels: *const u8,
    background_len: usize,
    output: *mut u8,
    capacity: usize,
) -> i32 {
    // SAFETY: the caller guarantees that the handle is live.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    let background = if background_pixels.is_null() {
        if background_len != 0 {
            return ACERIS_STATUS_INVALID_ARGUMENT;
        }
        None
    } else {
        if background_len != FRAME_BYTES {
            return ACERIS_STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: the caller guarantees FRAME_BYTES readable bytes.
        Some(unsafe { slice::from_raw_parts(background_pixels, background_len) })
    };
    let frame = handle.library.render_animated(animation_frame, background);
    write_frame(&frame, output, capacity)
}

/// Renders an animated library frame with distinct portrait and landscape backgrounds.
///
/// The host decodes both user-selected background animations and supplies the
/// current RGBA8 frame for each orientation. Only the frame matching the
/// current display orientation is composited. A null pointer with length zero
/// disables that orientation's custom background.
///
/// # Safety
/// handle must be live; output must have capacity writable bytes. Every
/// non-null background pointer must reference exactly FRAME_BYTES readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_library_render_animated_with_backgrounds(
    handle: *const AcerisLibraryHandle,
    animation_frame: u64,
    display_orientation: u32,
    portrait_pixels: *const u8,
    portrait_len: usize,
    landscape_pixels: *const u8,
    landscape_len: usize,
    output: *mut u8,
    capacity: usize,
) -> i32 {
    // SAFETY: the caller guarantees that the handle is live.
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return ACERIS_STATUS_INVALID_HANDLE;
    };
    if !matches!(display_orientation, ORIENTATION_PORTRAIT | ORIENTATION_LANDSCAPE) {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }

    let selected = if display_orientation == ORIENTATION_PORTRAIT {
        (portrait_pixels, portrait_len)
    } else {
        (landscape_pixels, landscape_len)
    };
    let (pixels, len) = selected;
    let background = if pixels.is_null() {
        if len != 0 {
            return ACERIS_STATUS_INVALID_ARGUMENT;
        }
        None
    } else {
        if len != FRAME_BYTES {
            return ACERIS_STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: the caller guarantees FRAME_BYTES readable bytes.
        Some(unsafe { slice::from_raw_parts(pixels, len) })
    };

    let frame = handle.library.render_animated(animation_frame, background);
    write_frame(&frame, output, capacity)
}

/// Returns the virtual button bit at a touch point, or zero when no button is hit.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_ui_hit_test_virtual_control(
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    orientation: u32,
) -> u32 {
    frontend::hit_test_virtual_control(width, height, x, y, orientation)
}

/// Writes a transparent RGBA8 virtual gamepad overlay into a host-owned buffer.
///
/// # Safety
/// output must reference at least capacity writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aceris_ui_render_virtual_controls(
    width: u32,
    height: u32,
    orientation: u32,
    pressed_mask: u32,
    output: *mut u8,
    capacity: usize,
) -> i32 {
    let Some(frame) =
        frontend::render_virtual_pad_overlay(width, height, orientation, pressed_mask)
    else {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    };
    if output.is_null() || capacity < frame.len() {
        return ACERIS_STATUS_INVALID_ARGUMENT;
    }
    // SAFETY: output is non-null and capacity covers the complete rendered frame.
    unsafe { slice::from_raw_parts_mut(output, frame.len()) }.copy_from_slice(&frame);
    ACERIS_STATUS_OK
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
    fn library_orientation_switches_between_portrait_and_landscape() {
        let directory = std::env::temp_dir().join(format!(
            "aceris-orientation-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let c_dir = CString::new(directory.to_str().unwrap()).unwrap();
        // SAFETY: pointers are valid and the handle is destroyed once.
        unsafe {
            let handle = aceris_library_create(c_dir.as_ptr());
            assert!(!handle.is_null());
            assert_eq!(aceris_library_get_orientation(handle), ORIENTATION_AUTO as i32);
            assert_eq!(
                aceris_library_set_orientation(handle, ORIENTATION_PORTRAIT),
                ACERIS_STATUS_OK
            );
            let mut layout = ScreenLayout::default();
            assert_eq!(
                aceris_library_get_screen_layout(handle, 1080, 2400, &mut layout),
                ACERIS_STATUS_OK
            );
            assert_eq!(layout.orientation, ORIENTATION_PORTRAIT);
            assert_eq!(
                aceris_library_set_orientation(handle, ORIENTATION_LANDSCAPE),
                ACERIS_STATUS_OK
            );
            assert_eq!(
                aceris_library_get_screen_layout(handle, 2400, 1080, &mut layout),
                ACERIS_STATUS_OK
            );
            assert_eq!(layout.orientation, ORIENTATION_LANDSCAPE);
            assert_eq!(aceris_library_set_orientation(handle, 99), ACERIS_STATUS_INVALID_ARGUMENT);
            aceris_library_destroy(handle);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn virtual_controls_render_and_report_touch_buttons() {
        let width = 480;
        let height = 800;
        let mut overlay = vec![0u8; width as usize * height as usize * 4];
        // The A button is on the right-hand control cluster.
        let pressed = frontend::buttons::A;
        let rect_center_x = (width as f32 * 0.83
            + (width.min(height) as f32 * 0.048 * 1.5))
            .round() as i32;
        let rect_center_y = (height as f32 * 0.76).round() as i32;
        assert_eq!(
            aceris_ui_hit_test_virtual_control(
                width,
                height,
                rect_center_x,
                rect_center_y,
                ORIENTATION_PORTRAIT
            ),
            pressed
        );
        // SAFETY: overlay has exactly width * height * 4 writable bytes.
        assert_eq!(
            unsafe {
                aceris_ui_render_virtual_controls(
                    width,
                    height,
                    ORIENTATION_PORTRAIT,
                    pressed,
                    overlay.as_mut_ptr(),
                    overlay.len()
                )
            },
            ACERIS_STATUS_OK
        );
        assert!(overlay.chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(overlay.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn animated_library_renderer_accepts_only_full_size_background_frames() {
        let directory = std::env::temp_dir().join(format!(
            "aceris-animated-lib-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let c_dir = CString::new(directory.to_str().unwrap()).unwrap();
        // SAFETY: pointers and output buffers are valid for each call.
        unsafe {
            let handle = aceris_library_create(c_dir.as_ptr());
            let mut output = vec![0u8; FRAME_BYTES];
            let background = vec![20u8; FRAME_BYTES];
            assert_eq!(
                aceris_library_render_animated(
                    handle,
                    12,
                    background.as_ptr(),
                    background.len(),
                    output.as_mut_ptr(),
                    output.len()
                ),
                ACERIS_STATUS_OK
            );
            assert_eq!(
                aceris_library_render_animated(
                    handle,
                    12,
                    background.as_ptr(),
                    background.len() - 1,
                    output.as_mut_ptr(),
                    output.len()
                ),
                ACERIS_STATUS_INVALID_ARGUMENT
            );
            aceris_library_destroy(handle);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn animated_library_selects_a_distinct_background_for_each_orientation() {
        let directory = std::env::temp_dir().join(format!(
            "aceris-dual-background-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let c_dir = CString::new(directory.to_str().unwrap()).unwrap();
        // SAFETY: all pointers and buffers remain valid during each call.
        unsafe {
            let handle = aceris_library_create(c_dir.as_ptr());
            assert!(!handle.is_null());
            let mut portrait = Vec::with_capacity(FRAME_BYTES);
            let mut landscape = Vec::with_capacity(FRAME_BYTES);
            for _ in 0..(FRAME_BYTES / 4) {
                portrait.extend_from_slice(&[255, 0, 0, 255]);
                landscape.extend_from_slice(&[0, 0, 255, 255]);
            }
            let mut output = vec![0u8; FRAME_BYTES];
            assert_eq!(
                aceris_library_render_animated_with_backgrounds(
                    handle,
                    0,
                    ORIENTATION_PORTRAIT,
                    portrait.as_ptr(),
                    portrait.len(),
                    landscape.as_ptr(),
                    landscape.len(),
                    output.as_mut_ptr(),
                    output.len()
                ),
                ACERIS_STATUS_OK
            );
            let portrait_output = output.clone();
            assert_eq!(
                aceris_library_render_animated_with_backgrounds(
                    handle,
                    0,
                    ORIENTATION_LANDSCAPE,
                    portrait.as_ptr(),
                    portrait.len(),
                    landscape.as_ptr(),
                    landscape.len(),
                    output.as_mut_ptr(),
                    output.len()
                ),
                ACERIS_STATUS_OK
            );
            assert_ne!(portrait_output, output);
            assert_eq!(
                aceris_library_render_animated_with_backgrounds(
                    handle,
                    0,
                    99,
                    portrait.as_ptr(),
                    portrait.len(),
                    landscape.as_ptr(),
                    landscape.len(),
                    output.as_mut_ptr(),
                    output.len()
                ),
                ACERIS_STATUS_INVALID_ARGUMENT
            );
            aceris_library_destroy(handle);
        }
        fs::remove_dir_all(directory).unwrap();
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
