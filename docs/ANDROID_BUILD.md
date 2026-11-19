# Android: build and install (not yet verified on a device)

The Android app is a pure-native `NativeActivity` (`frontends/android/src/native_activity.rs`)
over raw FFI, with the app logic in `frontends/android/src/app.rs`. There is no Java/Kotlin code
(`android:hasCode="false"`).

## Status

- The CI job builds `libaceris_android.so` for `arm64-v8a` with `cargo ndk` (compile and link only).
- Nobody has run it on a device or emulator. Touch, hardware keys, the gamepad overlay, frame
  pacing and lifecycle handling are untested on Android.
- Packaging below is a recipe that was **not** executed: this environment has no Android SDK.

## Build the library

```
cargo ndk -t arm64-v8a build -p aceris-android --release
```

## Package an APK (recipe, unverified)

Requires Android SDK build-tools (`aapt2`, `zipalign`, `apksigner`) and a platform `android.jar`.

```
aapt2 compile --dir frontends/android/res -o build/res.zip
aapt2 link -o build/unsigned.apk -I $ANDROID_JAR \
    --manifest frontends/android/AndroidManifest.xml build/res.zip
mkdir -p build/lib/arm64-v8a
cp target/aarch64-linux-android/release/libaceris_android.so build/lib/arm64-v8a/
(cd build && zip -r unsigned.apk lib)
zipalign -f -p 4 build/unsigned.apk build/aligned.apk
apksigner sign --ks <keystore> --out build/aceris.apk build/aligned.apk
adb install -r build/aceris.apk
```

## Games

Copy `.3dsx`, `.3ds`/`.cci` or `.cia` files to the app's external files directory,
`Android/data/org.aceris.emulator/files/games/` (on Android 11+ use `adb push`, because file
managers cannot open `Android/data`). Without an external directory the app uses its internal data
directory.

## Known gaps

- No audio output, no gamepad axes (circle pad from a physical stick), no key remapping.
- No pause/resume of emulation state beyond not stepping frames while paused.
- The application id `org.aceris.emulator` is a placeholder.
