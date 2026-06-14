# cloudflared sidecar

LocalRemoteShare ships cloudflared as a Tauri `externalBin` sidecar so the
packaged app does not depend on cloudflared being installed on the user's
system.

## How the resolver works

`cloudflared_path()` in `src-tauri/src/lib.rs`:
1. Checks for a file named `cloudflared` (or `cloudflared.exe` on Windows)
   next to the app executable (Tauri's standard sidecar placement).
2. Falls back to the string `"cloudflared"` so the OS `PATH` is searched —
   this keeps `cargo run` / dev mode working when cloudflared is installed
   via Homebrew or another package manager.

## Binary naming convention

Tauri requires the sidecar file to be named with a Rust target-triple suffix
at build time, placed in `src-tauri/binaries/`:

| Platform          | Required filename                                          |
|-------------------|------------------------------------------------------------|
| macOS Apple Si    | `cloudflared-aarch64-apple-darwin`                         |
| macOS Intel       | `cloudflared-x86_64-apple-darwin`                         |
| Windows x64       | `cloudflared-x86_64-pc-windows-msvc.exe`                  |
| Linux x64         | `cloudflared-x86_64-unknown-linux-gnu`                     |

## Obtaining binaries

### macOS (build host — copy from Homebrew)

```bash
cp "$(which cloudflared)" src-tauri/binaries/cloudflared-aarch64-apple-darwin
chmod +x src-tauri/binaries/cloudflared-aarch64-apple-darwin
```

### Other platforms

Download from the official release page:
  https://github.com/cloudflare/cloudflared/releases

Rename the asset using the correct triple suffix and place it in
`src-tauri/binaries/`.

## Important

- `src-tauri/binaries/` is **gitignored** (large binaries should not be
  committed). Binaries must be present before running `cargo tauri build`.
- The `externalBin` entry is declared in `src-tauri/tauri.conf.json` and the
  sidecar execute permission is declared in
  `src-tauri/capabilities/default.json`.
