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

[ -d "$ANDROID_DIR" ] || {
  echo "error: $ANDROID_DIR not found — run 'cargo tauri android init' first" >&2
  exit 1
}

# Tauri has generated the app module both at gen/android/app and one level
# deeper at gen/android/<project>/app depending on version, so find it rather
# than guessing.
GRADLE_FILE="$(find "$ANDROID_DIR" -maxdepth 3 -path '*/app/build.gradle.kts' | head -1)"
MANIFEST="$(find "$ANDROID_DIR" -maxdepth 6 -path '*/app/src/main/AndroidManifest.xml' | head -1)"

[ -n "$GRADLE_FILE" ] || {
  echo "error: no app/build.gradle.kts under $ANDROID_DIR" >&2
  find "$ANDROID_DIR" -maxdepth 3 -type d >&2
  exit 1
}
[ -n "$MANIFEST" ] || {
  echo "error: no app/src/main/AndroidManifest.xml under $ANDROID_DIR" >&2
  exit 1
}
echo "==> app module: $(dirname "$GRADLE_FILE")"

# GNU sed's `-i` with no suffix and its `0,/re/` "first match only" addressing
# are both unavailable in the BSD sed macOS ships, and this script has to run on
# a maintainer's laptop as readily as on the Linux runner. Perl is on both.
# $1 = file, $2 = pattern, $3 = replacement; both are Perl-flavoured, the
# replacement may use $1..$n, and only the first match is replaced.
replace_first() {
  PAT="$2" REPL="$3" perl -0777 -i -pe '
    my ($pat, $repl) = ($ENV{PAT}, $ENV{REPL});
    # /m so ^ anchors to a line, not just the slurped file; /ee expands
    # the \n and $1 backreferences in REPL.
    s/$pat/"\"$repl\""/eem;
  ' "$1"
}

# ── 1a. useLegacyPackaging ───────────────────────────────────────────────────
if grep -q "useLegacyPackaging" "$GRADLE_FILE"; then
  echo "==> useLegacyPackaging already set"
else
  grep -q "^android {" "$GRADLE_FILE" || {
    echo "error: no 'android {' block in $GRADLE_FILE — the template changed" >&2
    exit 1
  }
  # Insert as the first thing inside the android { } block.
  replace_first "$GRADLE_FILE" '^android \{' 'android {\n    \/\/ libcloudflared.so is an executable we spawn, not a library we link:\n    \/\/ it has to exist as a real file in nativeLibraryDir.\n    packaging { jniLibs { useLegacyPackaging = true } }'
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
  replace_first "$MANIFEST" '<application' '<application android:extractNativeLibs=\"true\"'
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
  replace_first "$MANIFEST" '(<manifest[^>]*>)' '$1\n    <uses-permission android:name=\"android.permission.INTERNET\" \/>'
  grep -q "android.permission.INTERNET" "$MANIFEST" || {
    echo "error: failed to add INTERNET permission to $MANIFEST" >&2
    exit 1
  }
  echo "==> added INTERNET permission"
fi

echo
echo "Patched. Next: scripts/build-cloudflared-android.sh, then"
echo "  cargo tauri android build --apk --debug --target aarch64"
