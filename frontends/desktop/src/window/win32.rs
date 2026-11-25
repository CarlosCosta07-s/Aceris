//! Win32 backend using user32/gdi32 through raw FFI (no external crates).
//!
//! STATUS: written without access to a Windows toolchain. It is type-checked on
//! Linux (`RUSTFLAGS="--cfg aceris_typecheck_native" cargo check`) but has never
//! been linked, run or visually checked on Windows. The pure key/coordinate
//! decoding lives in `native_input` and is unit-tested everywhere.
//!
//! The window has a fixed client size (no resize border). `present` copies a
//! top-down 32-bit BGRX DIB to the window with `SetDIBitsToDevice`.

use super::native_input::{
    game_key_from_virtual_key, is_key_repeat, key_from_virtual_key, point_from_lparam,
    scroll_rows_from_wheel,
};
use super::{WindowError, WindowEvent};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr;
use std::num::NonZeroIsize;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, Win32DisplayHandle, Win32WindowHandle};

type Hwnd = *mut c_void;
type Hinstance = *mut c_void;
type Hdc = *mut c_void;
type Wparam = usize;
type Lparam = isize;
type Lresult = isize;

type WndProc = unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    wnd_proc: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Hinstance,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
    time: u32,
    point: Point,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    colors_used: u32,
    colors_important: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 1],
}

#[cfg_attr(target_os = "windows", link(name = "user32"))]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    fn DefWindowProcW(hwnd: Hwnd, message: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> Lresult;
    fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex_style: u32) -> i32;
    fn SetWindowPos(
        hwnd: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn GetDC(hwnd: Hwnd) -> Hdc;
    fn ReleaseDC(hwnd: Hwnd, hdc: Hdc) -> i32;
    fn LoadCursorW(instance: Hinstance, name: *const u16) -> *mut c_void;
    fn ValidateRect(hwnd: Hwnd, rect: *const Rect) -> i32;
    fn SetCapture(hwnd: Hwnd) -> Hwnd;
    fn ReleaseCapture() -> i32;
}

#[cfg_attr(target_os = "windows", link(name = "gdi32"))]
unsafe extern "system" {
    fn SetDIBitsToDevice(
        hdc: Hdc,
        dest_x: i32,
        dest_y: i32,
        width: u32,
        height: u32,
        source_x: i32,
        source_y: i32,
        start_scan: u32,
        scan_lines: u32,
        bits: *const c_void,
        info: *const BitmapInfo,
        usage: u32,
    ) -> i32;
}

#[cfg_attr(target_os = "windows", link(name = "kernel32"))]
unsafe extern "system" {
    fn GetModuleHandleW(module_name: *const u16) -> Hinstance;
}

const WM_DESTROY: u32 = 0x0002;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_PAINT: u32 = 0x000F;
const WM_CLOSE: u32 = 0x0010;
const WM_QUIT: u32 = 0x0012;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MOUSEWHEEL: u32 = 0x020A;
const MK_LBUTTON: usize = 0x0001;
const PM_REMOVE: u32 = 0x0001;
const SW_SHOW: i32 = 5;
const CS_HREDRAW_VREDRAW: u32 = 0x0003;
/// WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX: a fixed-size window.
const WINDOW_STYLE: u32 = 0x00C0_0000 | 0x0008_0000 | 0x0002_0000;
const SWP_NOMOVE_NOZORDER_NOACTIVATE: u32 = 0x0002 | 0x0004 | 0x0010;
const CW_USEDEFAULT: i32 = i32::MIN;
const IDC_ARROW: usize = 32512;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;

thread_local! {
    /// Events produced by the window procedure, drained by `poll_events`.
    static EVENTS: RefCell<Vec<WindowEvent>> = const { RefCell::new(Vec::new()) };
    static LEFT_BUTTON_DOWN: Cell<bool> = const { Cell::new(false) };
}

fn push_event(event: WindowEvent) {
    EVENTS.with(|events| events.borrow_mut().push(event));
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A top-level Win32 window that shows a BGRX frame buffer.
pub struct Window {
    hwnd: Hwnd,
    width: usize,
    height: usize,
}

impl Window {
    /// Returns HWND and the Win32 display handle while this Window is alive.
    pub fn raw_handles(&self) -> Result<(Option<RawDisplayHandle>, RawWindowHandle), WindowError> {
        let hwnd = NonZeroIsize::new(self.hwnd as isize)
            .ok_or_else(|| WindowError::Backend("Win32 window handle is null".to_owned()))?;
        Ok((
            Some(RawDisplayHandle::Win32(Win32DisplayHandle::new())),
            RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)),
        ))
    }

    pub fn open(title: &str, width: usize, height: usize) -> Result<Self, WindowError> {
        let class_name = wide("AcerisWindow");
        let title = wide(title);
        // SAFETY: plain Win32 calls with valid, NUL-terminated UTF-16 strings
        // that outlive the calls; the class and window are created on this thread.
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let class = WndClassExW {
                size: std::mem::size_of::<WndClassExW>() as u32,
                style: CS_HREDRAW_VREDRAW,
                wnd_proc: Some(window_procedure),
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: ptr::null_mut(),
                cursor: LoadCursorW(ptr::null_mut(), IDC_ARROW as *const u16),
                background: ptr::null_mut(),
                menu_name: ptr::null(),
                class_name: class_name.as_ptr(),
                icon_small: ptr::null_mut(),
            };
            // Registering twice (a second window) fails harmlessly: the class exists.
            RegisterClassExW(&class);

            let (outer_width, outer_height) = outer_size(width, height);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WINDOW_STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                outer_width,
                outer_height,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null_mut(),
            );
            if hwnd.is_null() {
                return Err(WindowError::Backend(
                    "CreateWindowExW failed to create the window".to_owned(),
                ));
            }
            ShowWindow(hwnd, SW_SHOW);
            Ok(Self {
                hwnd,
                width,
                height,
            })
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) -> Result<(), WindowError> {
        let (outer_width, outer_height) = outer_size(width, height);
        // SAFETY: `hwnd` is the live window created in `open`.
        let ok = unsafe {
            SetWindowPos(
                self.hwnd,
                ptr::null_mut(),
                0,
                0,
                outer_width,
                outer_height,
                SWP_NOMOVE_NOZORDER_NOACTIVATE,
            )
        };
        if ok == 0 {
            return Err(WindowError::Backend("SetWindowPos failed".to_owned()));
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Draws a top-down BGRX frame of exactly `width * height * 4` bytes.
    pub fn present(&mut self, bgrx: &[u8]) -> Result<(), WindowError> {
        if bgrx.len() != self.width * self.height * 4 {
            return Err(WindowError::Backend(format!(
                "frame has {} bytes, expected {}",
                bgrx.len(),
                self.width * self.height * 4
            )));
        }
        let info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: self.width as i32,
                // Negative height selects a top-down bitmap.
                height: -(self.height as i32),
                planes: 1,
                bit_count: 32,
                compression: BI_RGB,
                size_image: 0,
                x_pels_per_meter: 0,
                y_pels_per_meter: 0,
                colors_used: 0,
                colors_important: 0,
            },
            colors: [0],
        };
        // SAFETY: `hwnd` is live; `bgrx` covers every scan line described by
        // `info`, and the DC is released before returning.
        let copied = unsafe {
            let hdc = GetDC(self.hwnd);
            if hdc.is_null() {
                return Err(WindowError::Backend("GetDC failed".to_owned()));
            }
            let lines = SetDIBitsToDevice(
                hdc,
                0,
                0,
                self.width as u32,
                self.height as u32,
                0,
                0,
                0,
                self.height as u32,
                bgrx.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
            );
            ReleaseDC(self.hwnd, hdc);
            lines
        };
        if copied == 0 {
            return Err(WindowError::Backend("SetDIBitsToDevice failed".to_owned()));
        }
        Ok(())
    }

    pub fn poll_events(&mut self) -> Vec<WindowEvent> {
        let mut msg = Msg {
            hwnd: ptr::null_mut(),
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            point: Point::default(),
        };
        // SAFETY: standard message pump on the thread that owns the window.
        unsafe {
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    push_event(WindowEvent::CloseRequested);
                    continue;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()))
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: `hwnd` was created in `open` and is destroyed exactly once.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

/// Outer window size for a client area of `width` x `height`.
fn outer_size(width: usize, height: usize) -> (i32, i32) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    // SAFETY: `rect` is a valid RECT; on failure it keeps the client size.
    unsafe {
        AdjustWindowRectEx(&mut rect, WINDOW_STYLE, 0, 0);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Window procedure: translates Win32 messages into [`WindowEvent`]s.
///
/// # Safety
/// Called by the system with a valid window handle and message arguments.
unsafe extern "system" fn window_procedure(
    hwnd: Hwnd,
    message: u32,
    wparam: Wparam,
    lparam: Lparam,
) -> Lresult {
    match message {
        WM_CLOSE => {
            // Let the application decide; the window is destroyed on drop.
            push_event(WindowEvent::CloseRequested);
            0
        }
        WM_DESTROY => 0,
        WM_PAINT => {
            // SAFETY: validating the whole client area stops WM_PAINT repeating.
            unsafe { ValidateRect(hwnd, ptr::null()) };
            push_event(WindowEvent::Redraw);
            0
        }
        WM_KEYDOWN => {
            let vk = wparam as u32;
            if let Some(key) = key_from_virtual_key(vk) {
                push_event(WindowEvent::Key(key));
            }
            if !is_key_repeat(lparam)
                && let Some(key) = game_key_from_virtual_key(vk)
            {
                push_event(WindowEvent::Game { key, pressed: true });
            }
            0
        }
        WM_KEYUP => {
            if let Some(key) = game_key_from_virtual_key(wparam as u32) {
                push_event(WindowEvent::Game {
                    key,
                    pressed: false,
                });
            }
            0
        }
        WM_KILLFOCUS => {
            push_event(WindowEvent::FocusLost);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = point_from_lparam(lparam);
            LEFT_BUTTON_DOWN.with(|down| down.set(true));
            // SAFETY: capturing keeps button-up delivered outside the client area.
            unsafe { SetCapture(hwnd) };
            push_event(WindowEvent::Click { x, y });
            0
        }
        WM_MOUSEMOVE => {
            if wparam & MK_LBUTTON != 0 && LEFT_BUTTON_DOWN.with(Cell::get) {
                let (x, y) = point_from_lparam(lparam);
                push_event(WindowEvent::Pointer { x, y, down: true });
            }
            0
        }
        WM_LBUTTONUP => {
            if LEFT_BUTTON_DOWN.with(|down| down.replace(false)) {
                let (x, y) = point_from_lparam(lparam);
                // SAFETY: releases the capture taken on button-down.
                unsafe { ReleaseCapture() };
                push_event(WindowEvent::Pointer { x, y, down: false });
            }
            0
        }
        WM_MOUSEWHEEL => {
            let rows = scroll_rows_from_wheel(wparam);
            if rows != 0 {
                push_event(WindowEvent::Scroll(rows));
            }
            0
        }
        // SAFETY: forwards unhandled messages with the original arguments.
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}
