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

/// Runs the shared core's CPU self-test.
///
/// Returns the computed value (55) on success, or `-1` on failure. This is the
/// same test the desktop front-end runs with `aceris --selftest`.
#[unsafe(no_mangle)]
pub extern "C" fn aceris_selftest() -> i32 {
    aceris_core::selftest::run()
        .ok()
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(-1)
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

    #[test]
    fn selftest_runs_the_shared_core() {
        assert_eq!(aceris_selftest(), 55);
    }
}
