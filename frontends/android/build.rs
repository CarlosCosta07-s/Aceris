//! Makes the Android shared library fail to link, instead of failing to load
//! on the device, when it references a symbol the NDK does not provide.
//!
//! Without this flag a call such as `AKeyEvent_getUnicodeChar` (not an NDK
//! API) linked successfully and `NativeActivity` then closed the app at
//! startup with `dlopen failed: cannot locate symbol`.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,--no-undefined");
    }
}
