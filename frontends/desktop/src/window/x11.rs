//! X11 backend using Xlib through raw FFI.
//!
//! `libX11.so.6` is opened with `dlopen` at run time, so building the emulator
//! needs no X11 development files and a headless machine just gets an error
//! from [`Window::open`]. Only 24/32-bit TrueColor visuals with the usual
//! 0xFF0000/0x00FF00/0x0000FF masks are supported.

use super::{Key, WindowError, WindowEvent};
use std::ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::{self, NonNull};

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn malloc(size: usize) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
const ZPIXMAP: c_int = 2;
const KEY_PRESS_MASK: c_long = 1 << 0;
const BUTTON_PRESS_MASK: c_long = 1 << 2;
const EXPOSURE_MASK: c_long = 1 << 15;
const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
const EVENT_KEY_PRESS: c_int = 2;
const EVENT_BUTTON_PRESS: c_int = 4;
const EVENT_EXPOSE: c_int = 12;
const EVENT_CLIENT_MESSAGE: c_int = 33;

// Offsets into the 64-bit Xlib structures used below.
const VISUAL_RED_MASK: usize = 24;
const VISUAL_GREEN_MASK: usize = 32;
const VISUAL_BLUE_MASK: usize = 40;
const CLIENT_MESSAGE_DATA0: usize = 56;

type Display = c_void;

struct Xlib {
    library: *mut c_void,
    open_display: unsafe extern "C" fn(*const c_char) -> *mut Display,
    close_display: unsafe extern "C" fn(*mut Display) -> c_int,
    default_screen: unsafe extern "C" fn(*mut Display) -> c_int,
    root_window: unsafe extern "C" fn(*mut Display, c_int) -> c_ulong,
    default_visual: unsafe extern "C" fn(*mut Display, c_int) -> *mut c_void,
    default_depth: unsafe extern "C" fn(*mut Display, c_int) -> c_int,
    create_simple_window: unsafe extern "C" fn(
        *mut Display,
        c_ulong,
        c_int,
        c_int,
        c_uint,
        c_uint,
        c_uint,
        c_ulong,
        c_ulong,
    ) -> c_ulong,
    destroy_window: unsafe extern "C" fn(*mut Display, c_ulong) -> c_int,
    store_name: unsafe extern "C" fn(*mut Display, c_ulong, *const c_char) -> c_int,
    select_input: unsafe extern "C" fn(*mut Display, c_ulong, c_long) -> c_int,
    intern_atom: unsafe extern "C" fn(*mut Display, *const c_char, c_int) -> c_ulong,
    set_wm_protocols: unsafe extern "C" fn(*mut Display, c_ulong, *mut c_ulong, c_int) -> c_int,
    map_window: unsafe extern "C" fn(*mut Display, c_ulong) -> c_int,
    resize_window: unsafe extern "C" fn(*mut Display, c_ulong, c_uint, c_uint) -> c_int,
    create_gc: unsafe extern "C" fn(*mut Display, c_ulong, c_ulong, *mut c_void) -> *mut c_void,
    free_gc: unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int,
    create_image: unsafe extern "C" fn(
        *mut Display,
        *mut c_void,
        c_uint,
        c_int,
        c_int,
        *mut c_char,
        c_uint,
        c_uint,
        c_int,
        c_int,
    ) -> *mut c_void,
    destroy_image: unsafe extern "C" fn(*mut c_void) -> c_int,
    put_image: unsafe extern "C" fn(
        *mut Display,
        c_ulong,
        *mut c_void,
        *mut c_void,
        c_int,
        c_int,
        c_int,
        c_int,
        c_uint,
        c_uint,
    ) -> c_int,
    pending: unsafe extern "C" fn(*mut Display) -> c_int,
    next_event: unsafe extern "C" fn(*mut Display, *mut u64) -> c_int,
    flush: unsafe extern "C" fn(*mut Display) -> c_int,
    keycode_to_keysym: unsafe extern "C" fn(*mut Display, u8, c_int) -> c_ulong,
}

fn load(library: *mut c_void, name: &str) -> Result<*mut c_void, WindowError> {
    let symbol = CString::new(name).expect("symbol names contain no NUL");
    // SAFETY: `library` is a live handle returned by dlopen and `symbol` is a
    // valid NUL-terminated string.
    let address = unsafe { dlsym(library, symbol.as_ptr()) };
    if address.is_null() {
        Err(WindowError::Backend(format!("libX11 lacks {name}")))
    } else {
        Ok(address)
    }
}

/// Resolves `name` from `library` as a function pointer of type `F`.
///
/// `F` must be the `unsafe extern "C" fn` type matching the symbol's Xlib prototype.
fn load_function<F: Copy>(library: *mut c_void, name: &str) -> Result<F, WindowError> {
    assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    let address = load(library, name)?;
    // SAFETY: `F` is a function-pointer type of the same size as `address`, and
    // the address was resolved from libX11 for exactly this prototype.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&address) })
}

impl Xlib {
    fn open() -> Result<Self, WindowError> {
        let name = CString::new("libX11.so.6").expect("no NUL");
        // SAFETY: plain dlopen of a system library by name.
        let library = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
        if library.is_null() {
            return Err(WindowError::Backend(
                "libX11.so.6 is not available (is an X11 session installed?)".to_string(),
            ));
        }
        macro_rules! symbol {
            ($name:literal) => {
                load_function(library, $name)?
            };
        }
        Ok(Self {
            library,
            open_display: symbol!("XOpenDisplay"),
            close_display: symbol!("XCloseDisplay"),
            default_screen: symbol!("XDefaultScreen"),
            root_window: symbol!("XRootWindow"),
            default_visual: symbol!("XDefaultVisual"),
            default_depth: symbol!("XDefaultDepth"),
            create_simple_window: symbol!("XCreateSimpleWindow"),
            destroy_window: symbol!("XDestroyWindow"),
            store_name: symbol!("XStoreName"),
            select_input: symbol!("XSelectInput"),
            intern_atom: symbol!("XInternAtom"),
            set_wm_protocols: symbol!("XSetWMProtocols"),
            map_window: symbol!("XMapWindow"),
            resize_window: symbol!("XResizeWindow"),
            create_gc: symbol!("XCreateGC"),
            free_gc: symbol!("XFreeGC"),
            create_image: symbol!("XCreateImage"),
            destroy_image: symbol!("XDestroyImage"),
            put_image: symbol!("XPutImage"),
            pending: symbol!("XPending"),
            next_event: symbol!("XNextEvent"),
            flush: symbol!("XFlush"),
            keycode_to_keysym: symbol!("XKeycodeToKeysym"),
        })
    }
}

/// An X11 window showing a BGRX image of a fixed size.
pub struct Window {
    xlib: Xlib,
    display: NonNull<Display>,
    window: c_ulong,
    gc: NonNull<c_void>,
    image: NonNull<c_void>,
    pixels: NonNull<u8>,
    width: usize,
    height: usize,
    delete_atom: c_ulong,
    visual: *mut c_void,
    depth: c_int,
}

fn backend_error(message: &str) -> WindowError {
    WindowError::Backend(message.to_string())
}

impl Window {
    /// Opens and maps a window whose client area is `width` x `height` pixels.
    pub fn open(title: &str, width: usize, height: usize) -> Result<Self, WindowError> {
        let (w, h) = (
            c_uint::try_from(width).map_err(|_| backend_error("window width is too large"))?,
            c_uint::try_from(height).map_err(|_| backend_error("window height is too large"))?,
        );
        if w == 0 || h == 0 {
            return Err(backend_error("window dimensions must be non-zero"));
        }
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| backend_error("window is too large"))?;
        let title = CString::new(title).map_err(|_| backend_error("title contains NUL"))?;
        let xlib = Xlib::open()?;

        // SAFETY: every call below follows the Xlib prototypes, uses handles
        // created in this function, and checks results before use. On failure
        // the display and library are released before returning.
        unsafe {
            let display = (xlib.open_display)(ptr::null());
            let Some(display) = NonNull::new(display) else {
                dlclose(xlib.library);
                return Err(backend_error(
                    "cannot open the X display (is DISPLAY set and an X server running?)",
                ));
            };
            let d = display.as_ptr();
            let fail = |message: &str| {
                (xlib.close_display)(d);
                dlclose(xlib.library);
                Err(backend_error(message))
            };

            let screen = (xlib.default_screen)(d);
            let visual = (xlib.default_visual)(d, screen);
            let depth = (xlib.default_depth)(d, screen);
            if visual.is_null() || !matches!(depth, 24 | 32) {
                return fail("unsupported X visual (need 24/32-bit TrueColor)");
            }
            let mask = |offset: usize| {
                ptr::read_unaligned(visual.cast::<u8>().add(offset).cast::<c_ulong>())
            };
            if mask(VISUAL_RED_MASK) != 0xFF_0000
                || mask(VISUAL_GREEN_MASK) != 0x00_FF00
                || mask(VISUAL_BLUE_MASK) != 0x00_00FF
            {
                return fail("unsupported X visual channel layout");
            }

            let root = (xlib.root_window)(d, screen);
            let window = (xlib.create_simple_window)(d, root, 0, 0, w, h, 0, 0, 0);
            (xlib.store_name)(d, window, title.as_ptr());
            (xlib.select_input)(
                d,
                window,
                KEY_PRESS_MASK | BUTTON_PRESS_MASK | EXPOSURE_MASK | STRUCTURE_NOTIFY_MASK,
            );
            let protocol = CString::new("WM_DELETE_WINDOW").expect("no NUL");
            let mut delete_atom = (xlib.intern_atom)(d, protocol.as_ptr(), 0);
            (xlib.set_wm_protocols)(d, window, &mut delete_atom, 1);
            (xlib.map_window)(d, window);

            let Some(gc) = NonNull::new((xlib.create_gc)(d, window, 0, ptr::null_mut())) else {
                (xlib.destroy_window)(d, window);
                return fail("XCreateGC failed");
            };
            let Some(pixels) = NonNull::new(malloc(byte_len).cast::<u8>()) else {
                (xlib.free_gc)(d, gc.as_ptr());
                (xlib.destroy_window)(d, window);
                return fail("out of memory for the frame buffer");
            };
            ptr::write_bytes(pixels.as_ptr(), 0, byte_len);
            let image = (xlib.create_image)(
                d,
                visual,
                depth as c_uint,
                ZPIXMAP,
                0,
                pixels.as_ptr().cast::<c_char>(),
                w,
                h,
                32,
                0,
            );
            let Some(image) = NonNull::new(image) else {
                libc_free(pixels.as_ptr().cast());
                (xlib.free_gc)(d, gc.as_ptr());
                (xlib.destroy_window)(d, window);
                return fail("XCreateImage failed");
            };
            (xlib.flush)(d);

            Ok(Self {
                xlib,
                display,
                window,
                gc,
                image,
                pixels,
                width,
                height,
                delete_atom,
                visual,
                depth,
            })
        }
    }

    /// Resizes the X11 window and replaces its client-side image buffer.
    ///
    /// The new XImage is created before the old one is released, so allocation
    /// or XCreateImage failures leave the existing window buffer usable.
    pub fn resize(&mut self, width: usize, height: usize) -> Result<(), WindowError> {
        let (w, h) = (
            c_uint::try_from(width).map_err(|_| backend_error("window width is too large"))?,
            c_uint::try_from(height).map_err(|_| backend_error("window height is too large"))?,
        );
        if w == 0 || h == 0 {
            return Err(backend_error("window dimensions must be non-zero"));
        }
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| backend_error("window is too large"))?;

        // SAFETY: the X11 handles and visual remain live for this Window.
        // The new XImage takes ownership of the new malloc buffer; destroying
        // the old XImage releases its previous buffer exactly once.
        unsafe {
            let Some(pixels) = NonNull::new(malloc(byte_len).cast::<u8>()) else {
                return Err(backend_error("out of memory for the resized frame buffer"));
            };
            ptr::write_bytes(pixels.as_ptr(), 0, byte_len);
            let image = (self.xlib.create_image)(
                self.display.as_ptr(),
                self.visual,
                self.depth as c_uint,
                ZPIXMAP,
                0,
                pixels.as_ptr().cast::<c_char>(),
                w,
                h,
                32,
                0,
            );
            let Some(image) = NonNull::new(image) else {
                libc_free(pixels.as_ptr().cast());
                return Err(backend_error("XCreateImage failed while resizing"));
            };

            (self.xlib.resize_window)(self.display.as_ptr(), self.window, w, h);
            let previous_image = self.image;
            self.image = image;
            self.pixels = pixels;
            self.width = width;
            self.height = height;
            (self.xlib.destroy_image)(previous_image.as_ptr());
            (self.xlib.flush)(self.display.as_ptr());
        }
        Ok(())
    }

    /// Draws a `width * height * 4` byte BGRX image into the window.
    pub fn present(&mut self, bgrx: &[u8]) -> Result<(), WindowError> {
        let expected = self.width * self.height * 4;
        if bgrx.len() != expected {
            return Err(WindowError::Backend(format!(
                "frame has {} bytes, expected {expected}",
                bgrx.len()
            )));
        }
        // SAFETY: `pixels` owns `expected` bytes (see `open`), the source slice
        // has the same length and cannot overlap Xlib's private buffer, and the
        // display, window, GC and image handles are alive until `drop`.
        unsafe {
            ptr::copy_nonoverlapping(bgrx.as_ptr(), self.pixels.as_ptr(), expected);
            (self.xlib.put_image)(
                self.display.as_ptr(),
                self.window,
                self.gc.as_ptr(),
                self.image.as_ptr(),
                0,
                0,
                0,
                0,
                self.width as c_uint,
                self.height as c_uint,
            );
            (self.xlib.flush)(self.display.as_ptr());
        }
        Ok(())
    }

    /// Drains pending window-system events.
    pub fn poll_events(&mut self) -> Vec<WindowEvent> {
        let mut events = Vec::new();
        let mut buffer = [0u64; 24]; // XEvent is a 192-byte union
        // SAFETY: the display is alive; `buffer` is at least as large and as
        // aligned as XEvent, which XNextEvent fills in.
        unsafe {
            while (self.xlib.pending)(self.display.as_ptr()) > 0 {
                (self.xlib.next_event)(self.display.as_ptr(), buffer.as_mut_ptr());
                let kind = buffer[0] as u32 as c_int;
                match kind {
                    EVENT_CLIENT_MESSAGE
                        if buffer[CLIENT_MESSAGE_DATA0 / 8] == self.delete_atom =>
                    {
                        events.push(WindowEvent::CloseRequested);
                    }
                    EVENT_EXPOSE => events.push(WindowEvent::Redraw),
                    EVENT_KEY_PRESS => {
                        // XKeyEvent: keycode is the high half of the word at byte 80.
                        let keycode = (buffer[10] >> 32) as u8;
                        let keysym =
                            (self.xlib.keycode_to_keysym)(self.display.as_ptr(), keycode, 0);
                        if let Some(key) = key_from_keysym(keysym) {
                            events.push(WindowEvent::Key(key));
                        }
                    }
                    EVENT_BUTTON_PRESS => {
                        // XButtonEvent: x/y at bytes 64/68, button at byte 84.
                        let x = buffer[8] as u32 as i32;
                        let y = (buffer[8] >> 32) as u32 as i32;
                        match (buffer[10] >> 32) as u32 {
                            1 => events.push(WindowEvent::Click { x, y }),
                            4 => events.push(WindowEvent::Scroll(-1)),
                            5 => events.push(WindowEvent::Scroll(1)),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        events
    }
}

/// Maps an X keysym to a front-end key (arrows, paging, Return, Escape).
fn key_from_keysym(keysym: c_ulong) -> Option<Key> {
    match keysym {
        0xFF52 => Some(Key::Up),
        0xFF54 => Some(Key::Down),
        0xFF55 => Some(Key::PageUp),
        0xFF56 => Some(Key::PageDown),
        0xFF0D | 0xFF8D => Some(Key::Enter),
        0xFF1B => Some(Key::Escape),
        _ => None,
    }
}

fn libc_free(pointer: *mut c_void) {
    unsafe extern "C" {
        fn free(pointer: *mut c_void);
    }
    // SAFETY: only called with pointers obtained from `malloc`.
    unsafe { free(pointer) };
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: each handle was created in `open` and is released exactly
        // once. XDestroyImage frees the malloc'ed pixel buffer.
        unsafe {
            (self.xlib.destroy_image)(self.image.as_ptr());
            (self.xlib.free_gc)(self.display.as_ptr(), self.gc.as_ptr());
            (self.xlib.destroy_window)(self.display.as_ptr(), self.window);
            (self.xlib.close_display)(self.display.as_ptr());
            dlclose(self.xlib.library);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_map_to_navigation_keys() {
        assert_eq!(key_from_keysym(0xFF52), Some(Key::Up));
        assert_eq!(key_from_keysym(0xFF8D), Some(Key::Enter));
        assert_eq!(key_from_keysym(0xFF1B), Some(Key::Escape));
        assert_eq!(key_from_keysym(c_ulong::from(b'a')), None);
    }

    /// Opens a window if an X server is reachable; `None` means skip (headless CI).
    fn open_or_skip(width: usize, height: usize) -> Option<Window> {
        match Window::open("aceris test", width, height) {
            Ok(window) => Some(window),
            Err(WindowError::Backend(message)) => {
                eprintln!("skipping X11 test: {message}");
                None
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    fn gradient(width: usize, height: usize) -> Vec<u8> {
        let mut frame = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                frame.extend_from_slice(&[
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    0x80,
                    0xFF,
                ]);
            }
        }
        frame
    }

    #[test]
    fn invalid_dimensions_are_rejected_before_touching_x11() {
        assert!(matches!(
            Window::open("t", 0, 10),
            Err(WindowError::Backend(_))
        ));
        assert!(matches!(
            Window::open("t", 10, 0),
            Err(WindowError::Backend(_))
        ));
        assert!(matches!(
            Window::open("t", usize::MAX, 2),
            Err(WindowError::Backend(_))
        ));
    }

    #[test]
    fn presents_frames_and_polls_events_when_a_display_exists() {
        let Some(mut window) = open_or_skip(64, 48) else {
            return;
        };
        window.present(&gradient(64, 48)).unwrap();
        assert!(window.present(&[0; 3]).is_err());
        assert!(window.resize(0, 48).is_err());
        assert!(window.resize(usize::MAX, 2).is_err());
        window.resize(736, 240).unwrap();
        window.present(&gradient(736, 240)).unwrap();
        assert!(window.present(&gradient(64, 48)).is_err());
        let _ = window.poll_events();
    }

    /// Manual check: shows the gradient for a few seconds so a screenshot can be taken.
    #[test]
    #[ignore = "needs an X server; used for manual screenshots"]
    fn shows_a_gradient_for_inspection() {
        let mut window = open_or_skip(400, 480).expect("an X server is required");
        let frame = gradient(400, 480);
        for _ in 0..30 {
            window.poll_events();
            window.present(&frame).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
