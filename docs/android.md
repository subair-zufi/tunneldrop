# Android port — spike notes

Status: **groundwork only.** There is no Android app yet. This is steps 1–2 of
the port: the Rust core compiles for Android, and we have a cloudflared binary
that an Android device can actually execute. Nothing here has run on a device.

Everything below was done without an Android SDK or NDK installed, which is why
it stops where it does — `tauri android init` and any real build need both.

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

## What is not verified

- **That the tunnel actually comes up on a device.** No SDK, no emulator, no
  device here. The binary is the right shape; that it runs and reaches the
  Cloudflare edge from inside an app sandbox is the next thing to prove.
- **That the app links.** `cargo check` stops before the linker.
- **Anything about the UI.** The window is still sized for a 420×540 desktop
  window and drag-and-drop is the only way in besides the file picker.

## Next steps

1. **Generate the Gradle project.** With `ANDROID_HOME` and `NDK_HOME` set:
   `cargo install tauri-cli --version "^2"` then `cargo tauri android init`.
   Then patch two things or the cloudflared binary will be packaged but not
   executable:

   ```kotlin
   // gen/android/app/build.gradle.kts
   android { packaging { jniLibs { useLegacyPackaging = true } } }
   ```
   ```xml
   <!-- gen/android/app/src/main/AndroidManifest.xml -->
   <application android:extractNativeLibs="true" …>
   ```

   Since AGP 4.2 the default is to leave `.so` files compressed in the APK and
   map them straight out of it, which is fine for real libraries but leaves no
   file on disk to exec. Also add `<uses-permission android:name="android.permission.INTERNET" />`.

   Then `scripts/build-cloudflared-android.sh && cargo tauri android dev`.

2. **Content URIs.** `create_share` takes a path and `AppState::add_share`
   calls `std::fs::metadata` on it. The Android file picker returns a
   `content://` URI, so that call fails. The fix is to stop storing a path:
   take a `ParcelFileDescriptor` from `ContentResolver.openFileDescriptor`, pass
   the raw fd to Rust, and read name and size from `OpenableColumns`.
   `Share.file_path` becomes a handle — path on desktop, owned fd on Android —
   and each download in `server.rs` must `dup` the fd and seek to 0, since
   concurrent requests would otherwise share one file offset. Copying into app
   cache instead is far simpler but duplicates the whole file on disk, which
   defeats the point for a 4 GB video.

3. **Foreground service.** The moment the user leaves the app, the process is
   frozen and the transfer dies. Serving needs a foreground service
   (`dataSync` type — Android 14 wants a declared type and a justification), a
   persistent notification, a partial wake lock, and realistically a prompt to
   exempt the app from battery optimisation. This is the part with no Rust
   equivalent and the most risk.

4. **Mobile UI**, and an `ACTION_SEND` intent filter so Tunneldrop appears in
   the system share sheet — which is the natural way to start a share on a
   phone, far more so than a file picker.

5. **Distribution.** Play Store review can be prickly about an app that serves
   arbitrary user files over a public URL and ships a bundled network binary;
   the APK next to the desktop builds on GitHub Releases may be the smoother
   path. Note there is no in-app updater on mobile.

iOS is a different matter: no process spawning at all, so cloudflared would have
to become a linked library, and background sockets get minutes at best. Not
worth attempting on the back of this.
