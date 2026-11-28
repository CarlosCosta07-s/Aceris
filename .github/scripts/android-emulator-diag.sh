#!/usr/bin/env bash
# Installs the diagnostic APK, launches it and prints logcat plus a screenshot.
set -u
adb install -r build/diag.apk
adb logcat -c
adb shell am start -W -n org.aceris.emulator/android.app.NativeActivity
for t in 2 5 10 20; do
  sleep $t
  echo "=== after +${t}s: pid=$(adb shell pidof org.aceris.emulator || echo DEAD)"
  adb shell dumpsys activity activities | grep -E "mResumedActivity|topResumedActivity" | head -2
done
adb shell input tap 300 600
sleep 5
echo "=== after tap: pid=$(adb shell pidof org.aceris.emulator || echo DEAD)"
adb shell screencap -p /sdcard/s.png
adb pull /sdcard/s.png s.png >/dev/null 2>&1
python3 - <<'PY' || true
import zlib,struct,base64
data=open('s.png','rb').read()
print('SCREENSHOT_BYTES', len(data))
PY
echo "=== logcat (Aceris / crash related)"
adb logcat -d | grep -E "Aceris|aceris|DEBUG|libc|AndroidRuntime|FATAL|NativeActivity|linker|ActivityManager.*(org.aceris|died|Process)|tombstone|backtrace|#[0-9]{2} pc" | head -400
echo "=== full logcat tail"
adb logcat -d | tail -150
echo "=== screenshot base64 (downscaled)"
python3 -c "print(1)" >/dev/null
if command -v convert >/dev/null; then convert s.png -resize 25% s_small.png; else cp s.png s_small.png; fi
base64 -w0 s_small.png | head -c 400000
echo
