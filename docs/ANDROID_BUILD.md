# Android: build, empacotamento e validação

> **Estado verificado em 22/11/2026 — HEAD 0dc68f9a2f0b17253cc491a1d3dd363fdffa1a3c.** CI aprovada em Linux, Windows, macOS e Android arm64-v8a. O homebrew Lua Player Plus Cookie Clicker em CIA NoCrypto carregou, executou código ARM11, leu RomFS e desenhou sua própria tela de erro. csndInit passou após HLE inicial de csnd:SND, mas Sound.init ainda falha. Nenhum jogo jogável foi validado.

## Estado atual

- O frontend Android é uma NativeActivity em Rust/FFI (`native_activity.rs`), com lógica em `app.rs`, UI em `ui.rs` e manifesto sem Java/Kotlin.
- CI compila e linka `libaceris_android.so` para `arm64-v8a`; isso não comprova instalação, ciclo de vida, toque, áudio ou gameplay em aparelho físico.
- Existe saída AAudio (API 26+), mas nenhum áudio foi confirmado em dispositivo.
- O empacotamento APK abaixo é uma receita manual ainda não validada. O ID `org.aceris.emulator` é placeholder.

## Compilar a biblioteca

```sh
rustup target add aarch64-linux-android
cargo install cargo-ndk
cargo ndk -t arm64-v8a build -p aceris-android --release
```

Configure `ANDROID_NDK_HOME`.

## Empacotar APK (receita não validada)

Requer Android SDK Build Tools e `android.jar` compatível.

```sh
aapt2 compile --dir frontends/android/res -o build/res.zip
aapt2 link -o build/unsigned.apk -I "$ANDROID_JAR" --manifest frontends/android/AndroidManifest.xml build/res.zip
mkdir -p build/lib/arm64-v8a
cp target/aarch64-linux-android/release/libaceris_android.so build/lib/arm64-v8a/
(cd build && zip -r unsigned.apk lib)
zipalign -f -p 4 build/unsigned.apk build/aligned.apk
apksigner sign --ks <keystore> --out build/aceris.apk build/aligned.apk
adb install -r build/aceris.apk
```

Após instalar, validar ciclo de vida, recriação de superfície, pausa/retomada, rotação, toque multitoque, teclas, gamepad, áudio e homebrew. Registrar aparelho, Android, ABI, comandos e logs.

## Dados e limitações

Use apenas jogos obtidos legalmente pelo usuário. Nunca inclua CIA, 3DS, firmware, chaves ou saves no repositório/CI/issues/releases. Ainda não há teste documentado de APK em aparelho físico; eixos analógicos, remapeamento, HID, pause/resume e áudio real precisam de validação.
