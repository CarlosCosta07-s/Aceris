#!/usr/bin/env bash
# Installs the diagnostic APK, launches it and checks startup plus the
# shared-storage permission flow, printing logcat for diagnosis.
set -u
PACKAGE=org.aceris.emulator
fail=0

adb install -r build/diag.apk
adb logcat -c
adb shell am start -W -n $PACKAGE/android.app.NativeActivity
for t in 2 5 5; do
  sleep $t
  echo "=== after +${t}s: pid=$(adb shell pidof $PACKAGE || echo DEAD)"
done

aceris_log() { adb logcat -d -s Aceris:I; }

echo "=== storage prompt"
if aceris_log | grep -q "Shared storage access: missing"; then
  echo "Access reported missing; choosing PERMITIR with gamepad A"
  adb shell input keyevent KEYCODE_BUTTON_A
  sleep 4
  if aceris_log | grep -q "Opened the storage permission screen"; then
    echo "System permission screen opened:"
    adb shell dumpsys activity activities | grep -E "topResumedActivity|mResumedActivity" | head -2
  else
    echo "::error::The storage permission screen was not opened"
    fail=1
  fi
  # Grant as the user would, then return to Aceris.
  API=$(adb shell getprop ro.build.version.sdk | tr -d '\r')
  if [ "$API" -ge 30 ]; then
    adb shell appops set --uid $PACKAGE MANAGE_EXTERNAL_STORAGE allow
  else
    adb shell pm grant $PACKAGE android.permission.READ_EXTERNAL_STORAGE
    adb shell pm grant $PACKAGE android.permission.WRITE_EXTERNAL_STORAGE
  fi
  # Android may restart the app when this permission changes; let that
  # settle before relaunching so the start intent is not lost.
  sleep 3
  adb shell am start -W -n $PACKAGE/android.app.NativeActivity
  sleep 5
  if aceris_log | grep -q "Shared storage access: granted"; then
    echo "Access granted and detected after resuming"
  else
    echo "::error::Aceris did not detect the granted access"
    fail=1
  fi
  if adb shell ls /sdcard/Aceris/games >/dev/null 2>&1; then
    echo "Public folder /sdcard/Aceris/games exists"
  else
    echo "::error::/sdcard/Aceris/games was not created"
    fail=1
  fi
else
  echo "::error::No 'missing' storage report from Aceris"
  fail=1
fi

adb shell input tap 300 600
sleep 3
echo "=== after tap: pid=$(adb shell pidof $PACKAGE || echo DEAD)"
echo "=== logcat (Aceris / crash related)"
adb logcat -d | grep -E "Aceris|aceris|AndroidRuntime|FATAL|linker|ActivityManager.*(org.aceris|died)|#[0-9]{2} pc" | head -300
if [ -z "$(adb shell pidof $PACKAGE)" ]; then
  echo "::error::Aceris is not running at the end of the check"
  adb logcat -d | tail -150
  fail=1
fi
[ $fail -eq 0 ] && echo "Aceris startup and storage access checks passed."
exit $fail
