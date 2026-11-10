//! Aceris Android front-end.
//!
//! Built as a `cdylib` and loaded by the Android app shell. Only a version
//! probe is exported for now; surface, input and lifecycle glue come later.

use std::ffi::c_char;

/// Returns the Aceris version as a static, NUL-terminated C string.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn version_is_a_valid_c_string() {
        // SAFETY: `aceris_version` returns a pointer to a static NUL-terminated string.
        let version = unsafe { CStr::from_ptr(aceris_version()) };
        assert_eq!(version.to_str().unwrap(), aceris_common::VERSION);
    }
}
