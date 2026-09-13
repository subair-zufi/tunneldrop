#!/usr/bin/env bash
# Builds cloudflared for Android and drops it into the Gradle project's jniLibs
# so it ends up inside the APK.
#
# Why this exists instead of downloading a release binary:
#
#   * Tauri's `externalBin` sidecar mechanism is desktop-only. On Android the
#     only app-private directory a process may execute from is nativeLibraryDir
#     (since API 29 the platform refuses to exec anything the app can write),
#     and the only way to get a file in there is to ship it under `jniLibs/`
#     named `lib*.so`. cloudflared is a normal ELF executable, not a library —
#     the name is purely what makes the packager install it with the exec bit.
#
#   * Cloudflare's published `cloudflared-linux-arm64` is a non-PIE static
#     binary. Android has required position-independent executables since
#     API 21, so it will not load. `GOOS=android` produces a PIE binary linked
#     against bionic (`/system/bin/linker64`), which is what a device wants.
#
# Why CGO is on (and why that drags in the NDK):
#
#   * Android has no /etc/resolv.conf. Go's pure-Go resolver reads that file to
#     find a nameserver, finds nothing, and falls back to localhost:53 — where
#     nothing is listening. Every lookup then fails with
#         lookup api.trycloudflare.com on [::1]:53: connection refused
#     and no tunnel is ever created. With cgo, Go resolves through bionic's
#     getaddrinfo, which talks to the platform resolver and works.
#
#   * That means a C cross-compiler, so NDK_HOME (or ANDROID_NDK_ROOT) must
#     point at an NDK. Building without it would produce a binary that starts
#     up fine and can never reach the network — the worst kind of broken.
#
# Usage:
#   scripts/build-cloudflared-android.sh              # arm64-v8a + x86_64
#   ABIS="arm64-v8a" scripts/build-cloudflared-android.sh
#   CLOUDFLARED_REF=2026.9.1 scripts/build-cloudflared-android.sh
#
# Requires Go (the cloudflared tree pins a toolchain in go.mod; a recent Go will
# fetch the right one automatically) and an Android NDK, for the reason above.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Pinned so a build is reproducible; bump deliberately.
CLOUDFLARED_REF="${CLOUDFLARED_REF:-2026.9.1}"
# arm64 covers every current phone; x86_64 is for the emulator. Add
# "armeabi-v7a" if you care about 32-bit devices.
ABIS="${ABIS:-arm64-v8a x86_64}"

SRC_DIR="${SRC_DIR:-$REPO_ROOT/target/cloudflared-src}"

# Drop the binary next to the generated app module's manifest. Tauri has put
# that module both at gen/android/app and one level deeper depending on
# version, so find it; before `tauri android init` has ever run there is
# nothing to find and the conventional path is created.
ANDROID_DIR="${ANDROID_DIR:-$REPO_ROOT/src-tauri/gen/android}"
if [ -z "${JNILIBS_DIR:-}" ]; then
  APP_MANIFEST="$(find "$ANDROID_DIR" -maxdepth 6 -path '*/app/src/main/AndroidManifest.xml' 2>/dev/null | head -1)"
  if [ -n "$APP_MANIFEST" ]; then
    JNILIBS_DIR="$(dirname "$APP_MANIFEST")/jniLibs"
  else
    echo "note: no generated Android project yet; using the conventional path"
    JNILIBS_DIR="$ANDROID_DIR/app/src/main/jniLibs"
  fi
fi

command -v go >/dev/null || { echo "error: go is not installed" >&2; exit 1; }

# The NDK supplies the C cross-compiler cgo needs. NDK_HOME is what Tauri's
# mobile tooling sets; ANDROID_NDK_ROOT is the other common spelling.
NDK="${NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
[ -n "$NDK" ] && [ -d "$NDK" ] || {
  echo "error: set NDK_HOME (or ANDROID_NDK_ROOT) to an Android NDK — cgo needs its C compiler" >&2
  exit 1
}

# The NDK's toolchain is published under a host tag. Apple silicon runs the
# x86_64 build under Rosetta, so darwin-x86_64 is right on every Mac.
case "$(uname -s)" in
  Darwin) HOST_TAG="darwin-x86_64" ;;
  Linux)  HOST_TAG="linux-x86_64" ;;
  *) echo "error: unsupported build host $(uname -s)" >&2; exit 1 ;;
esac
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin"
[ -d "$TOOLCHAIN" ] || {
  echo "error: no NDK toolchain at $TOOLCHAIN" >&2
  exit 1
}

# Must match bundle.android.minSdkVersion in tauri.conf.json.
API_LEVEL="${API_LEVEL:-24}"

# ABI -> the NDK clang that targets it.
clang_for() {
  case "$1" in
    arm64-v8a)   echo "$TOOLCHAIN/aarch64-linux-android$API_LEVEL-clang" ;;
    armeabi-v7a) echo "$TOOLCHAIN/armv7a-linux-androideabi$API_LEVEL-clang" ;;
    x86_64)      echo "$TOOLCHAIN/x86_64-linux-android$API_LEVEL-clang" ;;
    x86)         echo "$TOOLCHAIN/i686-linux-android$API_LEVEL-clang" ;;
    *) echo "error: unknown ABI '$1'" >&2; exit 1 ;;
  esac
}

# ABI -> GOARCH (plus GOARM for 32-bit).
goarch_for() {
  case "$1" in
    arm64-v8a)   echo "arm64" ;;
    armeabi-v7a) echo "arm" ;;
    x86_64)      echo "amd64" ;;
    x86)         echo "386" ;;
    *) echo "error: unknown ABI '$1'" >&2; exit 1 ;;
  esac
}

if [ ! -d "$SRC_DIR/.git" ]; then
  echo "==> cloning cloudflared @ $CLOUDFLARED_REF"
  git clone --depth 1 --branch "$CLOUDFLARED_REF" \
    https://github.com/cloudflare/cloudflared.git "$SRC_DIR"
else
  echo "==> reusing $SRC_DIR"
  git -C "$SRC_DIR" fetch --depth 1 origin "$CLOUDFLARED_REF"
  git -C "$SRC_DIR" checkout -q FETCH_HEAD
fi

for abi in $ABIS; do
  goarch="$(goarch_for "$abi")"
  out="$JNILIBS_DIR/$abi/libcloudflared.so"
  mkdir -p "$(dirname "$out")"

  cc="$(clang_for "$abi")"
  [ -x "$cc" ] || { echo "error: no compiler at $cc" >&2; exit 1; }

  echo "==> building $abi (GOARCH=$goarch, CC=$(basename "$cc"))"
  (
    cd "$SRC_DIR"
    # CGO_ENABLED=1 is what makes DNS work on a device; see the header.
    export GOOS=android GOARCH="$goarch" CGO_ENABLED=1 CC="$cc"
    [ "$goarch" = "arm" ] && export GOARM=7
    # -s -w strips the symbol table and DWARF: ~29 MB instead of ~45 MB per ABI,
    # and this binary is never debugged on-device.
    go build -trimpath -ldflags="-s -w" -o "$out" ./cmd/cloudflared
  )

  if command -v file >/dev/null; then
    file "$out" | sed 's/^/    /'
  fi
  echo "    $(du -h "$out" | cut -f1)  ->  ${out#"$REPO_ROOT"/}"
done

cat <<'EOF'

Done. Two things the Gradle project must do or the binary will not be
executable at runtime:

  1. app/build.gradle.kts:

       android {
           packaging { jniLibs { useLegacyPackaging = true } }
       }

  2. app/src/main/AndroidManifest.xml, on <application>:

       android:extractNativeLibs="true"

Since AGP 4.2 the default is to leave .so files compressed inside the APK and
have the linker map them straight out of it. That works for real libraries but
not for us: there is no extracted file on disk to exec. Both switches together
restore the old behaviour of unpacking into nativeLibraryDir.
EOF
