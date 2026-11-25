//! macOS backend using AppKit through the Objective-C runtime (raw FFI).
//!
//! STATUS: written without access to a Mac. It is type-checked on Linux
//! (`RUSTFLAGS="--cfg aceris_typecheck_native" cargo check`) but has never been
//! linked, run or visually checked on macOS. Pure key/scroll decoding lives in
//! `native_input` and is unit-tested everywhere.
//!
//! No subclasses or delegates are created: events are pulled with
//! `nextEventMatchingMask:` and the frame is shown by setting a `CGImage` as
//! the content view's layer contents. Closing the window hides it, which is
//! reported as `CloseRequested`; there is no menu bar, so Cmd-Q does nothing.

use super::native_input::{
    game_key_from_mac_keycode, key_from_mac_keycode, scroll_rows_from_mac_delta,
};
use super::{WindowError, WindowEvent};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{self, NonNull};

type Id = *mut c_void;
type Sel = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CgPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CgRect {
    origin: CgPoint,
    size: CgSize,
}

#[cfg_attr(target_os = "macos", link(name = "objc"))]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

// One `link` attribute per block: repeating the attribute is a lint error.
#[cfg_attr(target_os = "macos", link(name = "AppKit", kind = "framework"))]
unsafe extern "C" {}

#[cfg_attr(target_os = "macos", link(name = "Foundation", kind = "framework"))]
unsafe extern "C" {}

#[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *mut c_void;
    fn CFRelease(object: *const c_void);
}

#[cfg_attr(target_os = "macos", link(name = "CoreGraphics", kind = "framework"))]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    fn CGDataProviderCreateWithCFData(data: *mut c_void) -> *mut c_void;
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        color_space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        should_interpolate: u8,
        intent: i32,
    ) -> *mut c_void;
}

// NSEventType values.
const EVENT_LEFT_MOUSE_DOWN: u64 = 1;
const EVENT_LEFT_MOUSE_UP: u64 = 2;
const EVENT_LEFT_MOUSE_DRAGGED: u64 = 6;
const EVENT_KEY_DOWN: u64 = 10;
const EVENT_KEY_UP: u64 = 11;
const EVENT_SCROLL_WHEEL: u64 = 22;
/// NSWindowStyleMaskTitled | Closable | Miniaturizable.
const STYLE_MASK: u64 = 1 | 2 | 4;
const BACKING_STORE_BUFFERED: u64 = 2;
/// kCGBitmapByteOrder32Little | kCGImageAlphaNoneSkipFirst: memory bytes B,G,R,X.
const BITMAP_INFO_BGRX: u32 = 0x2000 | 6;
const ACTIVATION_POLICY_REGULAR: i64 = 0;

fn sel(name: &CStr) -> Sel {
    // SAFETY: `name` is a valid NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

fn class(name: &CStr) -> Id {
    // SAFETY: `name` is a valid NUL-terminated class name.
    unsafe { objc_getClass(name.as_ptr()) }
}

/// Reinterprets `objc_msgSend` as the function type of one concrete message.
///
/// # Safety
/// `F` must be an `unsafe extern "C" fn` whose signature matches the message.
unsafe fn msg_send_fn<F: Copy>() -> F {
    const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<usize>()) };
    // SAFETY: caller guarantees the signature; function pointers share a size.
    unsafe { std::mem::transmute_copy(&(objc_msgSend as unsafe extern "C" fn())) }
}

macro_rules! send {
    ($ret:ty; $obj:expr, $sel:expr $(, $arg:ident : $ty:ty = $value:expr)*) => {{
        let function: unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $ret = msg_send_fn();
        function($obj, $sel $(, $value)*)
    }};
}

/// A top-level AppKit window that shows a BGRX frame buffer.
pub struct Window {
    app: Id,
    window: Id,
    content_view: Id,
    color_space: *mut c_void,
    width: usize,
    height: usize,
    was_key: bool,
    close_reported: bool,
    mouse_down: bool,
}

impl Window {
    /// Returns the AppKit content-view handle while this Window is alive.
    pub fn raw_handles(&self) -> Result<(Option<RawDisplayHandle>, RawWindowHandle), WindowError> {
        let view = NonNull::new(self.content_view)
            .ok_or_else(|| WindowError::Backend("AppKit content view is null".to_owned()))?;
        Ok((
            Some(RawDisplayHandle::AppKit(AppKitDisplayHandle::new())),
            RawWindowHandle::AppKit(AppKitWindowHandle::new(view)),
        ))
    }

    pub fn open(title: &str, width: usize, height: usize) -> Result<Self, WindowError> {
        // SAFETY: AppKit messages are sent with the documented selectors and
        // argument types; every receiver is checked for nil before use.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let result = Self::open_inner(title, width, height);
            objc_autoreleasePoolPop(pool);
            result
        }
    }

    /// # Safety
    /// Must run on the main thread inside an autorelease pool.
    unsafe fn open_inner(title: &str, width: usize, height: usize) -> Result<Self, WindowError> {
        // SAFETY: see `open`.
        unsafe {
            let app = send!(Id; class(c"NSApplication"), sel(c"sharedApplication"));
            if app.is_null() {
                return Err(WindowError::Backend(
                    "NSApplication is unavailable".to_owned(),
                ));
            }
            send!(bool; app, sel(c"setActivationPolicy:"), policy: i64 = ACTIVATION_POLICY_REGULAR);
            send!(Id; app, sel(c"finishLaunching"));

            let frame = CgRect {
                origin: CgPoint { x: 100.0, y: 100.0 },
                size: CgSize {
                    width: width as f64,
                    height: height as f64,
                },
            };
            let allocated = send!(Id; class(c"NSWindow"), sel(c"alloc"));
            let window = send!(
                Id; allocated,
                sel(c"initWithContentRect:styleMask:backing:defer:"),
                rect: CgRect = frame,
                style: u64 = STYLE_MASK,
                backing: u64 = BACKING_STORE_BUFFERED,
                defer: bool = false
            );
            if window.is_null() {
                return Err(WindowError::Backend("NSWindow creation failed".to_owned()));
            }
            // Keep the window object alive after the user closes it.
            send!(Id; window, sel(c"setReleasedWhenClosed:"), flag: bool = false);

            let mut utf8 = title.as_bytes().to_vec();
            utf8.push(0);
            let ns_title = send!(
                Id; class(c"NSString"), sel(c"stringWithUTF8String:"),
                text: *const c_char = utf8.as_ptr().cast()
            );
            send!(Id; window, sel(c"setTitle:"), title: Id = ns_title);

            let content_view = send!(Id; window, sel(c"contentView"));
            send!(Id; content_view, sel(c"setWantsLayer:"), flag: bool = true);
            send!(Id; window, sel(c"makeKeyAndOrderFront:"), sender: Id = ptr::null_mut());
            send!(Id; app, sel(c"activateIgnoringOtherApps:"), flag: bool = true);

            Ok(Self {
                app,
                window,
                content_view,
                color_space: CGColorSpaceCreateDeviceRGB(),
                width,
                height,
                was_key: true,
                close_reported: false,
                mouse_down: false,
            })
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) -> Result<(), WindowError> {
        // SAFETY: `window` is the live NSWindow created in `open`.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            send!(
                Id; self.window, sel(c"setContentSize:"),
                size: CgSize = CgSize { width: width as f64, height: height as f64 }
            );
            objc_autoreleasePoolPop(pool);
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Shows a top-down BGRX frame of exactly `width * height * 4` bytes.
    pub fn present(&mut self, bgrx: &[u8]) -> Result<(), WindowError> {
        let expected = self.width * self.height * 4;
        if bgrx.len() != expected {
            return Err(WindowError::Backend(format!(
                "frame has {} bytes, expected {expected}",
                bgrx.len()
            )));
        }
        // SAFETY: CoreFoundation copies the bytes in CFDataCreate; every object
        // created here is released once after the layer has retained the image.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let data = CFDataCreate(ptr::null(), bgrx.as_ptr(), bgrx.len() as isize);
            let provider = CGDataProviderCreateWithCFData(data);
            let image = CGImageCreate(
                self.width,
                self.height,
                8,
                32,
                self.width * 4,
                self.color_space,
                BITMAP_INFO_BGRX,
                provider,
                ptr::null(),
                0,
                0,
            );
            let outcome = if image.is_null() {
                Err(WindowError::Backend("CGImageCreate failed".to_owned()))
            } else {
                let layer = send!(Id; self.content_view, sel(c"layer"));
                send!(Id; layer, sel(c"setContents:"), image: Id = image);
                Ok(())
            };
            if !image.is_null() {
                CFRelease(image);
            }
            CFRelease(provider);
            CFRelease(data);
            objc_autoreleasePoolPop(pool);
            outcome
        }
    }

    pub fn poll_events(&mut self) -> Vec<WindowEvent> {
        let mut events = Vec::new();
        // SAFETY: standard AppKit event pump on the main thread; the pool owns
        // the autoreleased events for the duration of the call.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let default_mode = send!(
                Id; class(c"NSString"), sel(c"stringWithUTF8String:"),
                text: *const c_char = c"kCFRunLoopDefaultMode".as_ptr()
            );
            loop {
                let event = send!(
                    Id; self.app,
                    sel(c"nextEventMatchingMask:untilDate:inMode:dequeue:"),
                    mask: u64 = u64::MAX,
                    date: Id = ptr::null_mut(),
                    mode: Id = default_mode,
                    dequeue: bool = true
                );
                if event.is_null() {
                    break;
                }
                let kind = send!(u64; event, sel(c"type"));
                let forward = self.translate(event, kind, &mut events);
                if forward {
                    send!(Id; self.app, sel(c"sendEvent:"), event: Id = event);
                }
            }
            send!(Id; self.app, sel(c"updateWindows"));

            let visible = send!(bool; self.window, sel(c"isVisible"));
            if !visible && !self.close_reported {
                self.close_reported = true;
                events.push(WindowEvent::CloseRequested);
            }
            let is_key = send!(bool; self.window, sel(c"isKeyWindow"));
            if self.was_key && !is_key {
                events.push(WindowEvent::FocusLost);
            }
            self.was_key = is_key;
            objc_autoreleasePoolPop(pool);
        }
        events
    }

    /// Translates one NSEvent; returns whether it should still go to `sendEvent:`.
    ///
    /// # Safety
    /// `event` must be a live NSEvent of type `kind`.
    unsafe fn translate(&mut self, event: Id, kind: u64, events: &mut Vec<WindowEvent>) -> bool {
        // SAFETY: selectors below are valid for the event types they are sent to.
        unsafe {
            match kind {
                EVENT_KEY_DOWN | EVENT_KEY_UP => {
                    let code = send!(u16; event, sel(c"keyCode"));
                    let pressed = kind == EVENT_KEY_DOWN;
                    let repeat = pressed && send!(bool; event, sel(c"isARepeat"));
                    if pressed && let Some(key) = key_from_mac_keycode(code) {
                        events.push(WindowEvent::Key(key));
                    }
                    if !repeat && let Some(key) = game_key_from_mac_keycode(code) {
                        events.push(WindowEvent::Game { key, pressed });
                    }
                    // Swallowed: forwarding unhandled keys makes AppKit beep.
                    false
                }
                EVENT_LEFT_MOUSE_DOWN | EVENT_LEFT_MOUSE_DRAGGED | EVENT_LEFT_MOUSE_UP => {
                    let location = send!(CgPoint; event, sel(c"locationInWindow"));
                    let (x, y) = (location.x as i32, self.height as i32 - location.y as i32);
                    let inside =
                        x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32;
                    match kind {
                        EVENT_LEFT_MOUSE_DOWN if inside => {
                            self.mouse_down = true;
                            events.push(WindowEvent::Click { x, y });
                        }
                        EVENT_LEFT_MOUSE_DRAGGED if self.mouse_down => {
                            events.push(WindowEvent::Pointer { x, y, down: true });
                        }
                        EVENT_LEFT_MOUSE_UP if self.mouse_down => {
                            self.mouse_down = false;
                            events.push(WindowEvent::Pointer { x, y, down: false });
                        }
                        _ => {}
                    }
                    // Title-bar clicks and drags must still reach the window.
                    true
                }
                EVENT_SCROLL_WHEEL => {
                    let delta = send!(f64; event, sel(c"scrollingDeltaY"));
                    let precise = send!(bool; event, sel(c"hasPreciseScrollingDeltas"));
                    let rows = scroll_rows_from_mac_delta(delta, precise);
                    if rows != 0 {
                        events.push(WindowEvent::Scroll(rows));
                    }
                    true
                }
                _ => true,
            }
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: the window and color space were created in `open` and are
        // released exactly once.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            send!(Id; self.window, sel(c"close"));
            objc_autoreleasePoolPop(pool);
            CFRelease(self.color_space);
        }
    }
}
