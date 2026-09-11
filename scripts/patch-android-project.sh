#!/usr/bin/env bash
# Applies the edits that `cargo tauri android init` cannot know to make.
# Run it after every init — the generated project is not committed, so this is
# what makes the build reproducible.
#
# 1. Package .so files uncompressed and extract them on install. Since AGP 4.2
#    the default is to leave them compressed inside the APK and have the linker
#    map them straight out of it. That is fine for real libraries, but
#    libcloudflared.so is an executable we spawn as a child process, and there
#    is nothing to spawn if no file was ever written to nativeLibraryDir.
# 2. Make sure the app can reach the network at all.
#
# Every edit verifies itself: a silently-failed patch here produces an APK that
# installs fine and then cannot start a tunnel, which is a miserable thing to
# debug on a phone.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_DIR="${ANDROID_DIR:-$REPO_ROOT/src-tauri/gen/android}"
GRADLE_FILE="$ANDROID_DIR/app/build.gradle.kts"
MANIFEST="$ANDROID_DIR/app/src/main/AndroidManifest.xml"

[ -d "$ANDROID_DIR" ] || {
  echo "error: $ANDROID_DIR not found — run 'cargo tauri android init' first" >&2
  exit 1
}
[ -f "$GRADLE_FILE" ] || { echo "error: $GRADLE_FILE not found" >&2; exit 1; }
[ -f "$MANIFEST" ] || { echo "error: $MANIFEST not found" >&2; exit 1; }

# ── 1a. useLegacyPackaging ───────────────────────────────────────────────────
if grep -q "useLegacyPackaging" "$GRADLE_FILE"; then
  echo "==> useLegacyPackaging already set"
else
  grep -q "^android {" "$GRADLE_FILE" || {
    echo "error: no 'android {' block in $GRADLE_FILE — the template changed" >&2
    exit 1
  }
  # Insert as the first thing inside the android { } block.
  sed -i '0,/^android {/s//android {\n    \/\/ libcloudflared.so is an executable we spawn, not a library we link:\n    \/\/ it has to exist as a real file in nativeLibraryDir.\n    packaging { jniLibs { useLegacyPackaging = true } }/' "$GRADLE_FILE"
  grep -q "useLegacyPackaging" "$GRADLE_FILE" || {
    echo "error: failed to patch $GRADLE_FILE" >&2
    exit 1
  }
  echo "==> added useLegacyPackaging to $GRADLE_FILE"
fi

# ── 1b. extractNativeLibs ────────────────────────────────────────────────────
if grep -q "extractNativeLibs" "$MANIFEST"; then
  echo "==> extractNativeLibs already set"
else
  sed -i '0,/<application/s//<application android:extractNativeLibs="true"/' "$MANIFEST"
  grep -q 'extractNativeLibs="true"' "$MANIFEST" || {
    echo "error: failed to patch $MANIFEST" >&2
    exit 1
  }
  echo "==> added extractNativeLibs to $MANIFEST"
fi

# ── 2. INTERNET permission ───────────────────────────────────────────────────
if grep -q "android.permission.INTERNET" "$MANIFEST"; then
  echo "==> INTERNET permission already declared"
else
  sed -i '0,/<manifest[^>]*>/s//&\n    <uses-permission android:name="android.permission.INTERNET" \/>/' "$MANIFEST"
  grep -q "android.permission.INTERNET" "$MANIFEST" || {
    echo "error: failed to add INTERNET permission to $MANIFEST" >&2
    exit 1
  }
  echo "==> added INTERNET permission"
fi

echo
echo "Patched. Next: scripts/build-cloudflared-android.sh, then"
echo "  cargo tauri android build --apk --debug --target aarch64"
