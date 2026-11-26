//! Minimal platform dynamic-library loader used by the EGL/OpenGL backend.

use std::ffi::{CString, c_void};

#[cfg(unix)]
mod platform {
    use std::ffi::{c_char, c_int, c_void};

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    const RTLD_NOW: c_int = 2;
    const RTLD_GLOBAL: c_int = 0x100;

    pub(super) fn open(filename: *const c_char) -> *mut c_void {
        // SAFETY: the caller passes a valid NUL-terminated library name.
        unsafe { dlopen(filename, RTLD_NOW | RTLD_GLOBAL) }
    }

    pub(super) fn symbol(handle: *mut c_void, name: *const c_char) -> *mut c_void {
        // SAFETY: the handle comes from dlopen and name is NUL-terminated.
        unsafe { dlsym(handle, name) }
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::{c_char, c_void};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "LoadLibraryA"]
        fn load_library_a(filename: *const c_char) -> *mut c_void;
        #[link_name = "GetProcAddress"]
        fn get_proc_address(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    pub(super) fn open(filename: *const c_char) -> *mut c_void {
        // SAFETY: the caller passes a valid NUL-terminated DLL name.
        unsafe { load_library_a(filename) }
    }

    pub(super) fn symbol(module: *mut c_void, name: *const c_char) -> *mut c_void {
        // SAFETY: the module comes from LoadLibraryA and name is NUL-terminated.
        unsafe { get_proc_address(module, name) }
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    use std::ffi::{c_char, c_void};

    pub(super) fn open(_filename: *const c_char) -> *mut c_void {
        std::ptr::null_mut()
    }

    pub(super) fn symbol(_module: *mut c_void, _name: *const c_char) -> *mut c_void {
        std::ptr::null_mut()
    }
}

/// An opened shared library. It is intentionally never closed: function
/// pointers resolved from it stay valid for the life of the process.
#[derive(Clone, Copy, Debug)]
pub struct Library(*mut c_void);

// SAFETY: the handle is an opaque platform-loader pointer; symbol lookup is
// thread-safe and the library is never unloaded.
unsafe impl Send for Library {}
// SAFETY: see above.
unsafe impl Sync for Library {}

impl Library {
    /// Opens the first library of `names` that loads on this platform.
    pub fn open_any(names: &[&str]) -> Option<Self> {
        names.iter().find_map(|name| {
            let name = CString::new(*name).ok()?;
            let handle = platform::open(name.as_ptr());
            (!handle.is_null()).then_some(Self(handle))
        })
    }

    /// Resolves `symbol`, returning null when it is missing.
    pub fn symbol(&self, symbol: &str) -> *mut c_void {
        let Ok(symbol) = CString::new(symbol) else {
            return std::ptr::null_mut();
        };
        platform::symbol(self.0, symbol.as_ptr())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::Library;

    #[test]
    fn windows_loader_opens_a_system_dll_and_resolves_an_export() {
        let kernel32 = Library::open_any(&["kernel32.dll"])
            .expect("the Windows kernel32 system library must be loadable");
        assert!(!kernel32.symbol("GetCurrentProcessId").is_null());
    }
}
