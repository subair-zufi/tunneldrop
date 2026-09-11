# Android port — notes

Status: **an APK builds; nothing has been run on a device.** CI produces a
debug-signed arm64 APK you can sideload (see "Getting an APK" below). Whether it
works once installed is exactly the open question — none of the Android-specific
code has ever executed.

Do not expect a finished app. In particular there is no foreground service yet,
so a transfer only survives while Tunneldrop is open on screen.

## Getting an APK

Every CI run builds one. Open the run on the Actions tab, and download the
`tunneldrop-android-arm64-debug` artifact from the summary page — it unzips to
an APK. It is debug-signed, so Android will ask you to allow installing from
your browser or file manager. arm64 only; it will not install on an emulator
image built for x86_64.

Locally, with `ANDROID_HOME` and `NDK_HOME` set:

```bash
cargo install tauri-cli --version "^2"   # or: npm i -g @tauri-apps/cli@^2
cargo tauri android init
scripts/patch-android-project.sh         # must run after every init
scripts/build-cloudflared-android.sh
cargo tauri android build --apk --debug --target aarch64
```

The generated project under `src-tauri/gen/android` is not committed: CI
regenerates and re-patches it on every build, so the patch script is the source
of truth for anything the template cannot know (see below).

## What is verified

**The Rust core type-checks for `aarch64-linux-android`.**

```bash
rustup target add aarch64-linux-android
cd src-tauri && cargo check --target aarch64-linux-android
```

This is `cargo check`, so it type-checks without linking — no NDK required. It
proves the desktop-only APIs are all behind cfg gates and the capability set is
satisfiable on mobile. It does *not* prove the app links, packages, or runs. CI
runs this on every push (the `android-check` job) so the mobile target does not
quietly rot while the rest of the port is built.

What had to change to get there:

| Thing | Why it broke | Fix |
|---|---|---|
| `tauri-plugin-updater`, `tauri-plugin-process` | Pull in `ring`, whose build script wants an NDK C toolchain. Neither plugin has a mobile implementation anyway. | Moved to a desktop-only `[target.'cfg(...)']` section in `Cargo.toml`. Cargo target sections are resolved before build scripts run, so Tauri's `desktop` cfg is not usable there — the three desktop OSes are listed by name. |
| `capabilities/default.json` | Asked for `updater:default` / `process:default`, which do not exist in a build without those plugins. | Split into `capabilities/desktop.json` with `"platforms": ["linux", "macOS", "windows"]`. |
| `externalBin` in `tauri.conf.json` | Tauri looked for `binaries/cloudflared-aarch64-linux-android`. The sidecar mechanism is desktop-only. | `tauri.android.conf.json` clears it; cloudflared reaches Android through `jniLibs` instead (below). |
| Tray icon and close-to-tray | `tauri::tray` and `tauri::menu` do not exist on mobile. | Extracted to `init_tray()` behind `#[cfg(desktop)]`; the `CloseRequested` handler is gated too. |
| "Check for updates" button | Would sit there offering an action that can only report itself unavailable. | Hidden when `window.__TAURI__.updater` is absent. |

**cloudflared builds for Android and is loadable there.**

```bash
scripts/build-cloudflared-android.sh          # arm64-v8a + x86_64
```

Produces a 28 MB stripped PIE ELF per ABI:

```
ELF 64-bit LSB pie executable, ARM aarch64, interpreter /system/bin/linker64
```

Two things make this necessary rather than just downloading a release:

* Cloudflare's published `cloudflared-linux-arm64` is a **non-PIE** static
  binary. Android has required position-independent executables since API 21,
  so it will not load. `GOOS=android` gives a PIE binary linked against bionic.
* Android will not execute a file the app can write — since API 29, W^X rules
  out the data and cache directories. The one app-private directory that stays
  executable is `nativeLibraryDir`, and the only way into it is to ship the file
  under `jniLibs/` named `lib*.so`. So cloudflared travels as
  `libcloudflared.so`. It is an executable, not a library; the name is purely
  what gets it installed with the exec bit set. (Syncthing-Android has shipped
  its Go daemon this way for years.)

`cloudflared_path()` in `src-tauri/src/lib.rs` resolves that location on Android
by asking the JVM for `ApplicationInfo.nativeLibraryDir` over JNI. The rest of
`tunnel.rs` — spawn, scrape the `trycloudflare.com` URL out of stderr, kill on
revoke — should work unchanged, since it is plain `tokio::process`.

**Picked files are read through their descriptor, not a path.**

The Android picker returns a `content://` URI. There is no filesystem path
behind it — the app is granted access to a descriptor the provider opens on its
behalf — so `std::fs::metadata` on it fails, which is what would have made the
first APK useless.

`android_fs::open_content_uri` asks the ContentResolver for a
`ParcelFileDescriptor`, takes ownership of the fd with `detachFd`, and reads the
size from `getStatSize` and the name from `OpenableColumns`. `Share` now holds a
`ShareSource` — a path on desktop, an owned fd on Android — and each download
re-opens it through `/proc/self/fd/N` so two people pulling the same share do
not share a read offset. Nothing is copied: a 4 GB video is served where it
lies, which is the whole point of the app.

## What is not verified

Everything above is compile-time evidence. **No Android code in this repository
has ever run.** Specifically unknown:

- **Whether the app starts at all.** It links now, which `cargo check` never
  proved.
- **Whether cloudflared spawns from nativeLibraryDir**, and whether a quick
  tunnel establishes from inside an app sandbox.
- **Whether the JNI is right.** A wrong method signature compiles perfectly and
  fails at runtime. `open_content_uri` and `native_library_dir` are the two
  places this bites.
- **The UI.** Still laid out for a 420×540 desktop window; drag-and-drop is
  meaningless on a phone and the only way in is the file picker.

## Next steps

1. **Foreground service.** The moment the user leaves the app, the process is
   frozen and the transfer dies — today a share only works while Tunneldrop is
   on screen. Serving needs a foreground service (`dataSync` type — Android 14
   wants a declared type and a justification), a persistent notification, a
   partial wake lock, and realistically a prompt to exempt the app from battery
   optimisation. This is the part with no Rust equivalent and the most risk.

2. **Mobile UI**, and an `ACTION_SEND` intent filter so Tunneldrop appears in
   the system share sheet — which is the natural way to start a share on a
   phone, far more so than a file picker.

3. **Distribution.** Play Store review can be prickly about an app that serves
   arbitrary user files over a public URL and ships a bundled network binary;
   the APK next to the desktop builds on GitHub Releases may be the smoother
   path. Note there is no in-app updater on mobile.

iOS is a different matter: no process spawning at all, so cloudflared would have
to become a linked library, and background sockets get minutes at best. Not
worth attempting on the back of this.
