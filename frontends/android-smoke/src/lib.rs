//! Minimal NativeActivity smoke test. This crate deliberately has no Aceris
//! core, graphics, audio, JNI, or emulator dependencies.

#[cfg(target_os = "android")]
mod android {
    use std::ffi::{c_int, c_void};
    use std::ptr;

    const WINDOW_FORMAT_RGBA_8888: c_int = 1;
    const ANDROID_LOG_INFO: c_int = 4;
    const ANDROID_LOG_ERROR: c_int = 6;
    const COLOR_RGBA: u32 = 0xFF_24_9B_66;

    type ActivityCallback = unsafe extern "C" fn(*mut c_void);
    type WindowCreatedCallback = unsafe extern "C" fn(*mut c_void, *mut c_void);

    #[repr(C)]
    struct NativeActivityCallbacks {
        on_start: Option<ActivityCallback>,
        on_resume: Option<ActivityCallback>,
        on_save_instance_state:
            Option<unsafe extern "C" fn(*mut c_void, *mut usize) -> *mut c_void>,
        on_pause: Option<ActivityCallback>,
        on_stop: Option<ActivityCallback>,
        on_destroy: Option<ActivityCallback>,
        on_window_focus_changed: Option<unsafe extern "C" fn(*mut c_void, c_int)>,
        on_native_window_created: Option<WindowCreatedCallback>,
    }

    #[repr(C)]
    struct NativeActivity {
        callbacks: *mut NativeActivityCallbacks,
    }

    #[repr(C)]
    struct NativeWindowBuffer {
        width: c_int,
        height: c_int,
        stride: c_int,
        format: c_int,
        bits: *mut c_void,
        reserved: [u32; 6],
    }

    #[link(name = "android")]
    unsafe extern "C" {
        fn ANativeWindow_setBuffersGeometry(
            window: *mut c_void,
            width: c_int,
            height: c_int,
            format: c_int,
        ) -> c_int;
        fn ANativeWindow_lock(
            window: *mut c_void,
            out_buffer: *mut NativeWindowBuffer,
            in_out_dirty_bounds: *mut c_void,
        ) -> c_int;
        fn ANativeWindow_unlockAndPost(window: *mut c_void) -> c_int;
    }

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: c_int, tag: *const i8, text: *const i8) -> c_int;
    }

    fn log(priority: c_int, message: &'static [u8]) {
        // SAFETY: both byte strings are static, NUL-terminated C strings.
        unsafe {
            __android_log_write(
                priority,
                c"AcerisSmoke".as_ptr().cast(),
                message.as_ptr().cast(),
            );
        }
    }

    unsafe extern "C" fn on_native_window_created(_activity: *mut c_void, window: *mut c_void) {
        if window.is_null() {
            log(
                ANDROID_LOG_ERROR,
                b"Native window callback received null window\0",
            );
            return;
        }

        // SAFETY: Android guarantees the window remains valid during this callback.
        let geometry_result =
            unsafe { ANativeWindow_setBuffersGeometry(window, 0, 0, WINDOW_FORMAT_RGBA_8888) };
        if geometry_result != 0 {
            log(
                ANDROID_LOG_ERROR,
                b"ANativeWindow_setBuffersGeometry failed\0",
            );
            return;
        }

        let mut buffer = NativeWindowBuffer {
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            bits: ptr::null_mut(),
            reserved: [0; 6],
        };

        // SAFETY: window is valid for this callback and buffer is writable.
        if unsafe { ANativeWindow_lock(window, &mut buffer, ptr::null_mut()) } != 0 {
            log(ANDROID_LOG_ERROR, b"ANativeWindow_lock failed\0");
            return;
        }

        let width = buffer.width.max(0) as usize;
        let height = buffer.height.max(0) as usize;
        let stride = buffer.stride.max(0) as usize;
        let can_paint = !buffer.bits.is_null()
            && width > 0
            && height > 0
            && stride >= width
            && height.checked_mul(stride).is_some();

        if can_paint {
            for y in 0..height {
                for x in 0..width {
                    let offset = (y * stride + x) * 4;
                    // SAFETY: validated dimensions and stride keep each pixel inside the buffer.
                    unsafe {
                        ptr::write_unaligned(
                            buffer.bits.cast::<u8>().add(offset).cast::<u32>(),
                            COLOR_RGBA,
                        );
                    }
                }
            }
        }

        // SAFETY: balances the successful ANativeWindow_lock above.
        let post_result = unsafe { ANativeWindow_unlockAndPost(window) };
        if !can_paint || post_result != 0 {
            log(ANDROID_LOG_ERROR, b"Native window smoke paint failed\0");
        } else {
            log(
                ANDROID_LOG_INFO,
                b"SMOKE_OK: NativeActivity and native window are working\0",
            );
        }
    }

    /// Android NativeActivity entry point for the isolated smoke-test APK.
    ///
    /// # Safety
    /// Called by Android with a live ANativeActivity and callback table.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn ANativeActivity_onCreate(
        activity: *mut NativeActivity,
        _saved_state: *mut c_void,
        _saved_state_size: usize,
    ) {
        log(
            ANDROID_LOG_INFO,
            b"SMOKE_ENTERED: native library loaded and entry point called\0",
        );
        if activity.is_null() {
            log(ANDROID_LOG_ERROR, b"NativeActivity pointer was null\0");
            return;
        }

        // SAFETY: Android owns the activity and its callback table for its lifetime.
        let callbacks = unsafe { (*activity).callbacks };
        if callbacks.is_null() {
            log(
                ANDROID_LOG_ERROR,
                b"NativeActivity callback table was null\0",
            );
            return;
        }

        // SAFETY: the callback field position and C ABI match ANativeActivityCallbacks.
        unsafe {
            (*callbacks).on_native_window_created = Some(on_native_window_created);
        }
        log(
            ANDROID_LOG_INFO,
            b"SMOKE_READY: native window callback registered\0",
        );
    }
}
