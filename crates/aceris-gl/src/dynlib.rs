//! Minimal `dlopen`/`dlsym` wrapper (libc symbols declared by hand).

use std::ffi::{CString, c_char, c_int, c_void};

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;
const RTLD_GLOBAL: c_int = 0x100;

/// An opened shared library. It is intentionally never closed: function
/// pointers resolved from it stay valid for the life of the process.
#[derive(Clone, Copy, Debug)]
pub struct Library(*mut c_void);

// SAFETY: the handle is an opaque pointer returned by dlopen; dlsym on it is
// thread-safe and the library is never unloaded.
unsafe impl Send for Library {}
// SAFETY: see above.
unsafe impl Sync for Library {}

impl Library {
    /// Opens the first library of `names` that loads.
    pub fn open_any(names: &[&str]) -> Option<Self> {
        names.iter().find_map(|name| {
            let name = CString::new(*name).ok()?;
            // SAFETY: `name` is a valid NUL-terminated string.
            let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
            (!handle.is_null()).then_some(Self(handle))
        })
    }

    /// Resolves `symbol`, returning null when it is missing.
    pub fn symbol(&self, symbol: &str) -> *mut c_void {
        let Ok(symbol) = CString::new(symbol) else {
            return std::ptr::null_mut();
        };
        // SAFETY: the handle came from dlopen and `symbol` is NUL-terminated.
        unsafe { dlsym(self.0, symbol.as_ptr()) }
    }
}
