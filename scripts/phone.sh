#!/usr/bin/env bash
# Build a release APK and install+launch it on the connected phone.
# One command: `scripts/phone.sh`
#
# Requires: a connected, USB-debugging-authorized device (`adb devices`
# should show it as `device`, not `unauthorized`), the Android NDK/SDK env
# (ANDROID_NDK_ROOT / ANDROID_HOME), and cargo-apk. The 16 KB page alignment
# and the baked signaling room come from build.rs / the env below.
set -euo pipefail
cd "$(dirname "$0")/.."

ROOM="${TWOTOP_ROOM:-wss://two-top-matchbox-production.up.railway.app/two-top?next=2}"

CARGO_APK_RELEASE_KEYSTORE="${CARGO_APK_RELEASE_KEYSTORE:-$HOME/.android/debug.keystore}" \
CARGO_APK_RELEASE_KEYSTORE_PASSWORD="${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}" \
TWOTOP_ROOM="$ROOM" \
TWOTOP_ICE_URL="${TWOTOP_ICE_URL:-}" \
  cargo apk run -p app --lib --target aarch64-linux-android --release
