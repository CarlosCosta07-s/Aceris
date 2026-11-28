# Android startup isolation test

## Purpose

The Aceris application currently closes before its first visible frame on at least one physical Android device. A successful CI build and a correctly packaged ARM64 shared library do not establish that the app can launch on-device.

The Android APK workflow therefore produces a second, separate diagnostic application:

- Main app ID: `org.aceris.emulator`
- Smoke-test app ID: `org.aceris.emulator.smoke`
- Smoke-test launcher label: `Aceris Startup Test`

The smoke-test library is built from `frontends/android-smoke`. It is an original, minimal Rust `NativeActivity` implementation with no Aceris core, game library, audio, JNI, WGPU, or emulator dependencies. It registers only `onNativeWindowCreated`, locks the native window, paints a solid teal/green frame, and posts it.

## Device test

Install the artifact named `aceris-android-startup-smoke-arm64` from the same successful **Build Android APK** workflow run. It can be installed alongside the main Aceris app because its application ID is different.

Launch **Aceris Startup Test**.

Expected result: a full-screen solid teal/green color. It does not emulate games and has no menus; that is intentional.

## Interpretation

| Result | What it tells us |
|---|---|
| Solid color appears and remains open | Android can launch a NativeActivity, load an ARM64 Rust shared library, invoke `ANativeActivity_onCreate`, deliver a native-window callback, and present a CPU-written buffer. The remaining crash is in the full Aceris startup path, not the basic NativeActivity/package path. |
| It closes immediately too | The failure is below the Aceris application logic: native library loading, NativeActivity entry/ABI, linker/API compatibility, or device/runtime configuration. Capture Logcat before changing the emulator code. |
| It opens but stays black | The activity/library entry succeeded, but the native-window callback or CPU buffer presentation did not complete. Check the `AcerisSmoke` Logcat tag. |

The smoke test emits these log messages:

- `SMOKE_ENTERED`: the native shared library loaded and Android called its entry point.
- `SMOKE_READY`: the activity callback table was updated.
- `SMOKE_OK`: the native window callback painted and posted a frame.

This diagnostic app is not a replacement frontend and must not be used for normal emulator testing. It exists to divide the failure into a platform/packaging layer and the full Aceris startup layer.
