#!/usr/bin/env bash
set -euo pipefail

# Build (unless --skip-build), install, and run the Maestro connection smoke
# flow against a local Android emulator or connected device.
root=$(cd -- "$(dirname -- "$0")/.." && pwd)
mobile="$root/apps/mobile"
apk="$mobile/android/app/build/outputs/apk/release/app-release.apk"
skip_build=false
screenshot=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build)
      skip_build=true
      shift
      ;;
    --screenshot)
      screenshot=${2:-}
      shift 2
      ;;
    *)
      echo "Usage: bash scripts/android-emulator-smoke.sh [--skip-build] [--screenshot PATH]" >&2
      exit 2
      ;;
  esac
done

export ANDROID_HOME=${ANDROID_HOME:-"$HOME/Android/Sdk"}
export ANDROID_SDK_ROOT=${ANDROID_SDK_ROOT:-"$ANDROID_HOME"}
export PATH="$ANDROID_HOME/platform-tools:$HOME/.maestro/bin:$PATH"

command -v adb >/dev/null || { echo "adb not found; set ANDROID_HOME to your Android SDK." >&2; exit 1; }
command -v maestro >/dev/null || { echo "maestro not found; install Maestro CLI first." >&2; exit 1; }

if [[ "$skip_build" != true ]]; then
  (cd "$mobile/android" && ./gradlew assembleRelease -PreactNativeArchitectures=x86_64 --no-daemon)
fi
if [[ ! -f "$apk" ]]; then
  echo "APK not found at $apk" >&2
  exit 1
fi

adb wait-for-device
for _ in $(seq 1 60); do
  if [[ "$(adb shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == "1" ]]; then
    break
  fi
  sleep 2
done

adb install -r "$apk"
MAESTRO_CLI_NO_ANALYTICS=1 MAESTRO_CLI_ANALYSIS_NOTIFICATION_DISABLED=true \
  maestro test "$mobile/.maestro/connect.yaml"

if [[ -n "$screenshot" ]]; then
  mkdir -p "$(dirname -- "$screenshot")"
  adb exec-out screencap -p >"$screenshot"
fi
