# Android startup reliability and backend recovery

## Reference study

This work uses PPSSPP as an architectural reference only. No PPSSPP source code, assets, or implementation were copied into Aceris.

Relevant upstream reference points:

- `UI/NativeApp.cpp`: `CheckFailedGPUBackends()` records graphics backends that did not survive startup and avoids repeating a failed backend on the next process launch.
- `Core/Config.cpp`: `DefaultGPUBackend()` selects a platform-appropriate Android default; `NextValidBackend()` advances to another available backend after a recorded failure.
- `Core/EmuThread.cpp`: graphics initialization is tied to a valid graphics context/surface and an Android JNI thread context.
- `android/jni/OboeContext.cpp`: Oboe selects AAudio when recommended and otherwise retains OpenSL ES as its audio API. Audio stream opening and errors are handled as a separate subsystem.

References:
- https://github.com/hrydgard/ppsspp/blob/master/UI/NativeApp.cpp
- https://github.com/hrydgard/ppsspp/blob/master/Core/Config.cpp
- https://github.com/hrydgard/ppsspp/blob/master/Core/EmuThread.cpp
- https://github.com/hrydgard/ppsspp/blob/master/android/jni/OboeContext.cpp

## Aceris implementation

### Graphics

The Android frontend now resolves `auto` to a single backend rather than passing both Vulkan and GL to wgpu:

- ARM64 Android API 27 and newer: Vulkan first.
- Older Android / non-ARM64: OpenGL ES first.
- A failed Vulkan startup/presentation is recorded in `.aceris/android_gpu_backend_recovery_v1.txt`; the next launch tries OpenGL ES.
- A failed OpenGL ES startup/presentation advances the next launch to software presentation through the existing `ANativeWindow` CPU path.
- The recovery marker is removed only after a GPU frame is successfully presented. This also covers a process termination during surface, adapter, device, or first-present initialization.

Explicit `ACERIS_GRAPHICS_BACKEND` values remain supported. An explicit `software` setting bypasses host GPU presentation.

### Audio

AAudio is no longer opened while the launcher is starting. The output is attempted only after the first successfully presented frame of a running game. If AAudio cannot be opened, the game and launcher remain usable without sound.

This is a startup-safety change, not a complete audio compatibility solution. Aceris does not yet implement an OpenSL ES fallback equivalent to Oboe; that remains separate work.

### JNI

Optional JNI helpers are no longer invoked as part of the initial Activity setup. JNI attachment and volume-control routing are delayed until audio is opened for gameplay. The native thread supplies explicit `JavaVMAttachArgs`, matching the thread-attachment discipline used by PPSSPP.

External-media detection remains conditional on the Android audio stream being classified as game usage. On older Android versions, the volume helper can still be attached without querying newer playback-session APIs.

## Validation status and device test

CI can verify Rust type-checking, tests, Android compilation, and APK generation; it cannot prove that a specific device's Vulkan/OpenGL/audio drivers will remain stable at runtime.

After installing the APK produced by the latest successful **Build Android APK** workflow, validate in this order:

1. Launch Aceris and confirm the intro and library appear.
2. Close and reopen Aceris to confirm the graphics recovery marker does not cause a loop.
3. Launch only a test homebrew, then check whether audio initialization is non-fatal.
4. Open and close the app repeatedly, including after rotating the device.

If a GPU driver terminates the process, reopen Aceris once: the persisted marker should select the next backend. If it still terminates, the following launch should reach the software presenter.

A successful APK build is not a substitute for this physical-device test.
