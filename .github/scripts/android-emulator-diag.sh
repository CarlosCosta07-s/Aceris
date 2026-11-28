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
import struct, zlib
data = open('s.png', 'rb').read()
w, h = struct.unpack('>II', data[16:24])
print('SCREENSHOT', w, 'x', h, len(data), 'bytes')
PY
echo "=== logcat (Aceris / crash related)"
adb logcat -d | grep -E "Aceris|aceris|DEBUG|libc|AndroidRuntime|FATAL|NativeActivity|linker|ActivityManager.*(org.aceris|died|Process)|tombstone|backtrace|#[0-9]{2} pc" | head -400
echo "=== full logcat tail"
adb logcat -d | tail -150
if [ -z "$(adb shell pidof org.aceris.emulator)" ]; then
  echo "::error::Aceris is not running 20 s after launch"
  exit 1
fi
echo "Aceris is still running after launch and a touch."
