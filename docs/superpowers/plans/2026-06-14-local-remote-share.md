# LocalRemoteShare Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a cross-platform (macOS + Windows) Tauri tray app that shares a single local file as a temporary public HTTPS link via a Cloudflare quick tunnel, with optional per-share password and multiple concurrent shares.

**Architecture:** A Tauri app with a Rust core holding all state. The core runs an in-process `axum` HTTP server on `127.0.0.1:<random port>` that serves a landing page and file downloads keyed by `/d/<token>`. A tunnel manager supervises one `cloudflared` child process that exposes the local server publicly. The web frontend talks to the core via Tauri commands. All state is in-memory and cleared on quit.

**Tech Stack:** Tauri v2, Rust, `axum` 0.7, Tokio, `rand`, `argon2`, `hex`, `tokio-util`, `serde`; `cloudflared` bundled as a Tauri sidecar; vanilla HTML/CSS/JS frontend.

---

## File Structure

```
local_remote_share/
  src-tauri/
    Cargo.toml                # Rust deps + sidecar config
    tauri.conf.json           # Tauri config, tray, sidecar bundling
    build.rs                  # Tauri build script
    binaries/                 # cloudflared sidecar binaries (platform-suffixed)
    src/
      main.rs                 # binary entry -> calls lib::run()
      lib.rs                  # Tauri builder, state setup, server+tray launch
      token.rs                # random token generation
      password.rs             # argon2 hash/verify
      share.rs                # Share struct + ShareRegistry
      tunnel.rs               # parse_tunnel_url + TunnelManager
      server.rs               # axum server: landing page + download routes
      commands.rs             # Tauri commands: create/revoke/list + ShareInfo DTO
      state.rs                # AppState (registry, tunnel, port)
  src/
    index.html                # minimal window markup
    main.js                   # drop/select, calls commands, renders share list
    styles.css                # minimal styling
  tests/
    fake_cloudflared.sh       # mock cloudflared emitting a fixture URL line
```

Each Rust module has one responsibility and is unit-tested in its own `#[cfg(test)]` block. The tunnel manager is tested against a fake `cloudflared` script so tests never hit the network.

---

## Task 1: Scaffold Tauri v2 project and dependencies

**Files:**
- Create: `src-tauri/Cargo.toml`
- Create: `src-tauri/tauri.conf.json`
- Create: `src-tauri/build.rs`
- Create: `src-tauri/src/main.rs`
- Create: `src-tauri/src/lib.rs`
- Create: `src/index.html`

- [ ] **Step 1: Create `src-tauri/Cargo.toml`**

```toml
[package]
name = "local-remote-share"
version = "0.1.0"
edition = "2021"

[lib]
name = "local_remote_share_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-shell = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
axum = "0.7"
tokio-util = { version = "0.7", features = ["io"] }
rand = "0.8"
hex = "0.4"
argon2 = "0.5"
anyhow = "1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `src-tauri/build.rs`**

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 3: Create `src-tauri/tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "LocalRemoteShare",
  "version": "0.1.0",
  "identifier": "com.localremoteshare.app",
  "build": {
    "frontendDist": "../src"
  },
  "app": {
    "windows": [
      {
        "title": "LocalRemoteShare",
        "width": 420,
        "height": 540,
        "resizable": true,
        "visible": true
      }
    ],
    "security": { "csp": null }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/icon.png"]
  }
}
```

- [ ] **Step 4: Create `src-tauri/src/main.rs`**

```rust
// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    local_remote_share_lib::run();
}
```

- [ ] **Step 5: Create a minimal `src-tauri/src/lib.rs`**

```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 6: Create a placeholder `src/index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <title>LocalRemoteShare</title>
  </head>
  <body>
    <h1>LocalRemoteShare</h1>
  </body>
</html>
```

- [ ] **Step 7: Verify it builds**

Run: `cd src-tauri && cargo build`
Expected: compiles successfully (downloads deps on first run).

- [ ] **Step 8: Commit**

```bash
git add src-tauri src/index.html
git commit -m "chore: scaffold Tauri v2 project with dependencies"
```

---

## Task 2: Random token generation

**Files:**
- Create: `src-tauri/src/token.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod token;`)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/token.rs`:

```rust
use rand::RngCore;

/// Generates a URL-safe random token with ~128 bits of entropy (32 hex chars).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_32_hex_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn tokens_are_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 2: Register the module**

In `src-tauri/src/lib.rs`, add at the top (above `run`):

```rust
mod token;
```

- [ ] **Step 3: Run the tests**

Run: `cd src-tauri && cargo test token::`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/token.rs src-tauri/src/lib.rs
git commit -m "feat: add random token generation"
```

---

## Task 3: Password hashing

**Files:**
- Create: `src-tauri/src/password.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod password;`)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/password.rs`:

```rust
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

/// Hashes a plaintext password using argon2. Returns the encoded hash string.
pub fn hash_password(plain: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .expect("hashing should not fail")
        .to_string()
}

/// Verifies a plaintext password against an encoded argon2 hash.
pub fn verify_password(plain: &str, encoded: &str) -> bool {
    match PasswordHash::new(encoded) {
        Ok(parsed) => Argon2::default()
            .verify_password(plain.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_password_verifies() {
        let hash = hash_password("hunter2");
        assert!(verify_password("hunter2", &hash));
    }

    #[test]
    fn wrong_password_fails() {
        let hash = hash_password("hunter2");
        assert!(!verify_password("nope", &hash));
    }

    #[test]
    fn hash_is_not_plaintext() {
        let hash = hash_password("hunter2");
        assert!(!hash.contains("hunter2"));
    }
}
```

- [ ] **Step 2: Add the argon2 std feature for SaltString generation**

In `src-tauri/Cargo.toml`, change the `argon2` dependency line to:

```toml
argon2 = { version = "0.5", features = ["std"] }
```

- [ ] **Step 3: Register the module**

In `src-tauri/src/lib.rs`, add near the other `mod` lines:

```rust
mod password;
```

- [ ] **Step 4: Run the tests**

Run: `cd src-tauri && cargo test password::`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/password.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: add argon2 password hashing"
```

---

## Task 4: Share struct and registry

**Files:**
- Create: `src-tauri/src/share.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod share;`)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/share.rs`:

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Share {
    pub token: String,
    pub file_path: PathBuf,
    pub name: String,
    pub size: u64,
    pub password_hash: Option<String>,
    pub download_count: u64,
    pub created_at: SystemTime,
}

impl Share {
    /// Creates a new share with download_count 0 and created_at now.
    pub fn new(
        token: String,
        file_path: PathBuf,
        name: String,
        size: u64,
        password_hash: Option<String>,
    ) -> Self {
        Share {
            token,
            file_path,
            name,
            size,
            password_hash,
            download_count: 0,
            created_at: SystemTime::now(),
        }
    }
}

#[derive(Default)]
pub struct ShareRegistry {
    shares: HashMap<String, Share>,
}

impl ShareRegistry {
    pub fn new() -> Self {
        ShareRegistry { shares: HashMap::new() }
    }

    pub fn insert(&mut self, share: Share) {
        self.shares.insert(share.token.clone(), share);
    }

    pub fn get(&self, token: &str) -> Option<&Share> {
        self.shares.get(token)
    }

    pub fn get_mut(&mut self, token: &str) -> Option<&mut Share> {
        self.shares.get_mut(token)
    }

    pub fn remove(&mut self, token: &str) -> Option<Share> {
        self.shares.remove(token)
    }

    pub fn list(&self) -> Vec<Share> {
        self.shares.values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.shares.is_empty()
    }

    pub fn len(&self) -> usize {
        self.shares.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(token: &str) -> Share {
        Share::new(token.to_string(), PathBuf::from("/tmp/x"), "x".into(), 10, None)
    }

    #[test]
    fn insert_and_get() {
        let mut r = ShareRegistry::new();
        r.insert(sample("abc"));
        assert_eq!(r.get("abc").unwrap().name, "x");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn remove_makes_it_gone() {
        let mut r = ShareRegistry::new();
        r.insert(sample("abc"));
        assert!(r.remove("abc").is_some());
        assert!(r.get("abc").is_none());
        assert!(r.is_empty());
    }

    #[test]
    fn increment_download_count_via_get_mut() {
        let mut r = ShareRegistry::new();
        r.insert(sample("abc"));
        r.get_mut("abc").unwrap().download_count += 1;
        assert_eq!(r.get("abc").unwrap().download_count, 1);
    }

    #[test]
    fn list_returns_all() {
        let mut r = ShareRegistry::new();
        r.insert(sample("a"));
        r.insert(sample("b"));
        assert_eq!(r.list().len(), 2);
    }
}
```

- [ ] **Step 2: Register the module**

In `src-tauri/src/lib.rs`, add near the other `mod` lines:

```rust
mod share;
```

- [ ] **Step 3: Run the tests**

Run: `cd src-tauri && cargo test share::`
Expected: PASS (4 tests).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/share.rs src-tauri/src/lib.rs
git commit -m "feat: add Share struct and in-memory registry"
```

---

## Task 5: Tunnel URL parsing

**Files:**
- Create: `src-tauri/src/tunnel.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod tunnel;`)

- [ ] **Step 1: Write the failing test**

Create `src-tauri/src/tunnel.rs`:

```rust
/// Extracts a trycloudflare.com HTTPS URL from a line of cloudflared output.
/// Returns None if the line contains no such URL.
pub fn parse_tunnel_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    // The URL ends at the first whitespace or control character.
    let end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.contains("trycloudflare.com") {
        Some(url.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_from_banner_line() {
        let line = "2024-01-01 INF |  https://brave-fox-1234.trycloudflare.com  |";
        assert_eq!(
            parse_tunnel_url(line),
            Some("https://brave-fox-1234.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn ignores_non_tunnel_url() {
        let line = "Visit https://developers.cloudflare.com for docs";
        assert_eq!(parse_tunnel_url(line), None);
    }

    #[test]
    fn returns_none_when_no_url() {
        assert_eq!(parse_tunnel_url("starting tunnel..."), None);
    }
}
```

- [ ] **Step 2: Register the module**

In `src-tauri/src/lib.rs`, add near the other `mod` lines:

```rust
mod tunnel;
```

- [ ] **Step 3: Run the tests**

Run: `cd src-tauri && cargo test tunnel::parse`
Expected: PASS (3 tests).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/tunnel.rs src-tauri/src/lib.rs
git commit -m "feat: add cloudflared URL parsing"
```

---

## Task 6: Tunnel manager (process supervision)

**Files:**
- Modify: `src-tauri/src/tunnel.rs` (add `TunnelManager`)
- Create: `tests/fake_cloudflared.sh`

- [ ] **Step 1: Create the fake cloudflared script**

Create `tests/fake_cloudflared.sh`:

```bash
#!/usr/bin/env bash
# Mimics cloudflared: prints a banner then a tunnel URL, then stays alive.
echo "INF starting tunnel"
echo "INF |  https://fake-test-tunnel.trycloudflare.com  |"
# Stay alive so the manager can supervise it.
sleep 30
```

Then make it executable:

```bash
chmod +x tests/fake_cloudflared.sh
```

- [ ] **Step 2: Write the failing test (append to `src-tauri/src/tunnel.rs`)**

Add the `TunnelManager` and an async test. Place the struct above the `#[cfg(test)]` block:

```rust
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Supervises a single cloudflared child process and captures its public URL.
pub struct TunnelManager {
    program: String,
    extra_args: Vec<String>,
    base_url: Arc<Mutex<Option<String>>>,
    child: Option<tokio::process::Child>,
}

impl TunnelManager {
    /// `program` is the path to the cloudflared binary (or a fake for tests).
    /// `extra_args` are appended after the standard tunnel args.
    pub fn new(program: impl Into<String>, extra_args: Vec<String>) -> Self {
        TunnelManager {
            program: program.into(),
            extra_args,
            base_url: Arc::new(Mutex::new(None)),
            child: None,
        }
    }

    /// Starts the tunnel pointed at the given local port and waits (up to ~15s)
    /// for the public URL to appear in stdout. Returns the URL.
    pub async fn start(&mut self, port: u16) -> anyhow::Result<String> {
        let mut cmd = Command::new(&self.program);
        if self.extra_args.is_empty() {
            cmd.arg("tunnel")
                .arg("--url")
                .arg(format!("http://127.0.0.1:{port}"));
        } else {
            cmd.args(&self.extra_args);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        self.child = Some(child);

        let base_url = self.base_url.clone();
        // cloudflared prints the URL to stderr in real life and our fake uses
        // stdout; scan both.
        let found = scan_for_url(stdout, stderr, base_url.clone()).await;
        match found {
            Some(url) => {
                *base_url.lock().unwrap() = Some(url.clone());
                Ok(url)
            }
            None => anyhow::bail!("tunnel URL not found before stream closed"),
        }
    }

    pub fn base_url(&self) -> Option<String> {
        self.base_url.lock().unwrap().clone()
    }

    /// Kills the child process if running.
    pub fn stop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
        self.child = None;
        *self.base_url.lock().unwrap() = None;
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }
}

async fn scan_for_url(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    _base_url: Arc<Mutex<Option<String>>>,
) -> Option<String> {
    let mut out = BufReader::new(stdout).lines();
    let mut err = BufReader::new(stderr).lines();
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(15));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => return None,
            line = out.next_line() => {
                match line {
                    Ok(Some(l)) => if let Some(u) = parse_tunnel_url(&l) { return Some(u); },
                    Ok(None) => {}
                    Err(_) => return None,
                }
            }
            line = err.next_line() => {
                match line {
                    Ok(Some(l)) => if let Some(u) = parse_tunnel_url(&l) { return Some(u); },
                    Ok(None) => {}
                    Err(_) => return None,
                }
            }
        }
    }
}
```

Then add this test inside the existing `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn start_captures_url_from_fake_binary() {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fake_cloudflared.sh");
        let mut mgr = TunnelManager::new(script, vec![script.to_string()]);
        let url = mgr.start(12345).await.expect("should capture url");
        assert_eq!(url, "https://fake-test-tunnel.trycloudflare.com");
        assert!(mgr.is_running());
        mgr.stop();
        assert!(!mgr.is_running());
    }
```

- [ ] **Step 3: Run the test**

Run: `cd src-tauri && cargo test tunnel::tests::start_captures_url_from_fake_binary -- --nocapture`
Expected: PASS. (The `extra_args` of just the script path makes the manager exec the script directly.)

- [ ] **Step 4: Run all tunnel tests**

Run: `cd src-tauri && cargo test tunnel::`
Expected: PASS (4 tests total).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tunnel.rs tests/fake_cloudflared.sh
git commit -m "feat: add cloudflared process supervision"
```

---

## Task 7: HTTP server — landing page and unprotected download

**Files:**
- Create: `src-tauri/src/state.rs`
- Create: `src-tauri/src/server.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod state; mod server;`)

- [ ] **Step 1: Create shared app state**

Create `src-tauri/src/state.rs`:

```rust
use crate::share::ShareRegistry;
use crate::tunnel::TunnelManager;
use std::sync::{Arc, Mutex};

/// Shared across the axum server and Tauri commands.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Mutex<ShareRegistry>>,
    pub tunnel: Arc<Mutex<TunnelManager>>,
    pub port: u16,
    pub cloudflared_path: String,
}

impl AppState {
    pub fn new(port: u16, cloudflared_path: String) -> Self {
        AppState {
            registry: Arc::new(Mutex::new(ShareRegistry::new())),
            tunnel: Arc::new(Mutex::new(TunnelManager::new(cloudflared_path.clone(), vec![]))),
            port,
            cloudflared_path,
        }
    }
}
```

- [ ] **Step 2: Write the failing test**

Create `src-tauri/src/server.rs`:

```rust
use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use tokio_util::io::ReaderStream;

/// Builds the axum router for serving landing pages and downloads.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/d/:token", get(landing_page))
        .route("/d/:token/download", post(download))
        .with_state(state)
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

async fn landing_page(Path(token): Path<String>, State(state): State<AppState>) -> Response {
    let share = { state.registry.lock().unwrap().get(&token).cloned() };
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };
    let needs_pw = share.password_hash.is_some();
    let pw_field = if needs_pw {
        r#"<input type="password" name="password" placeholder="Password" required />"#
    } else {
        ""
    };
    let html = format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{name}</title>
<style>body{{font-family:system-ui;max-width:32rem;margin:4rem auto;text-align:center}}
button{{padding:.6rem 1.2rem;font-size:1rem;cursor:pointer}}
input{{padding:.5rem;margin:.5rem;font-size:1rem}}</style></head>
<body><h1>{name}</h1><p>{size}</p>
<form method="post" action="/d/{token}/download">{pw_field}<br/>
<button type="submit">Download</button></form></body></html>"#,
        name = share.name,
        size = human_size(share.size),
        token = token,
        pw_field = pw_field,
    );
    Html(html).into_response()
}

#[derive(serde::Deserialize, Default)]
struct DownloadForm {
    password: Option<String>,
}

async fn download(
    Path(token): Path<String>,
    State(state): State<AppState>,
    form: Option<axum::Form<DownloadForm>>,
) -> Response {
    let share = { state.registry.lock().unwrap().get(&token).cloned() };
    let Some(share) = share else {
        return (StatusCode::NOT_FOUND, "Link not found or revoked.").into_response();
    };

    if let Some(hash) = &share.password_hash {
        let provided = form.and_then(|f| f.0.password).unwrap_or_default();
        if !crate::password::verify_password(&provided, hash) {
            return (StatusCode::UNAUTHORIZED, "Incorrect password.").into_response();
        }
    }

    let file = match tokio::fs::File::open(&share.file_path).await {
        Ok(f) => f,
        Err(_) => return (StatusCode::GONE, "File no longer available.").into_response(),
    };

    {
        let mut reg = state.registry.lock().unwrap();
        if let Some(s) = reg.get_mut(&token) {
            s.download_count += 1;
        }
    }

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", share.name),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::Share;
    use axum::body::to_bytes;
    use axum::http::Request;
    use std::io::Write;
    use std::path::PathBuf;
    use tower::ServiceExt; // for `oneshot`

    fn state_with_share(share: Share) -> AppState {
        let st = AppState::new(0, "cloudflared".into());
        st.registry.lock().unwrap().insert(share);
        st
    }

    fn temp_file(contents: &[u8]) -> (tempfile::NamedTempFile, PathBuf) {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(contents).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    #[tokio::test]
    async fn landing_page_shows_name() {
        let (_keep, path) = temp_file(b"hello");
        let share = Share::new("tok1".into(), path, "report.pdf".into(), 5, None);
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(Request::builder().uri("/d/tok1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("report.pdf"));
    }

    #[tokio::test]
    async fn unknown_token_is_404() {
        let app = build_router(AppState::new(0, "cloudflared".into()));
        let res = app
            .oneshot(Request::builder().uri("/d/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn download_streams_file_and_increments_count() {
        let (_keep, path) = temp_file(b"hello world");
        let share = Share::new("tok2".into(), path, "a.txt".into(), 11, None);
        let st = state_with_share(share);
        let app = build_router(st.clone());
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok2/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"hello world");
        assert_eq!(st.registry.lock().unwrap().get("tok2").unwrap().download_count, 1);
    }
}
```

- [ ] **Step 3: Add the `tower` dev-dependency**

In `src-tauri/Cargo.toml`, under `[dev-dependencies]` add:

```toml
tower = { version = "0.5", features = ["util"] }
```

- [ ] **Step 4: Register the modules**

In `src-tauri/src/lib.rs`, add near the other `mod` lines:

```rust
mod state;
mod server;
```

- [ ] **Step 5: Run the tests**

Run: `cd src-tauri && cargo test server::`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/server.rs src-tauri/src/state.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: add axum landing page and download streaming"
```

---

## Task 8: Password-protected download path

**Files:**
- Modify: `src-tauri/src/server.rs` (tests only — logic already present from Task 7)

- [ ] **Step 1: Write the failing tests (append to `server.rs` tests module)**

```rust
    #[tokio::test]
    async fn protected_download_rejects_wrong_password() {
        let (_keep, path) = temp_file(b"secret");
        let hash = crate::password::hash_password("letmein");
        let share = Share::new("tok3".into(), path, "s.txt".into(), 6, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok3/download")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=wrong"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_download_accepts_correct_password() {
        let (_keep, path) = temp_file(b"secret");
        let hash = crate::password::hash_password("letmein");
        let share = Share::new("tok4".into(), path, "s.txt".into(), 6, Some(hash));
        let app = build_router(state_with_share(share));
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/d/tok4/download")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("password=letmein"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"secret");
    }
```

- [ ] **Step 2: Run the tests**

Run: `cd src-tauri && cargo test server::`
Expected: PASS (5 tests total). The password logic from Task 7 already satisfies these — if either fails, fix `download` in `server.rs` accordingly.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/server.rs
git commit -m "test: cover password-protected download path"
```

---

## Task 9: AppState helpers — create / revoke / list shares

**Files:**
- Modify: `src-tauri/src/state.rs` (add share-management methods)

- [ ] **Step 1: Write the failing test (append to `state.rs`)**

Add these methods inside `impl AppState`:

```rust
    /// Reads file metadata, creates a share, and inserts it. Returns the token.
    pub fn add_share(
        &self,
        file_path: std::path::PathBuf,
        password: Option<String>,
    ) -> anyhow::Result<String> {
        let meta = std::fs::metadata(&file_path)?;
        let name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let token = crate::token::generate_token();
        let password_hash = password.map(|p| crate::password::hash_password(&p));
        let share = crate::share::Share::new(token.clone(), file_path, name, meta.len(), password_hash);
        self.registry.lock().unwrap().insert(share);
        Ok(token)
    }

    /// Removes a share. Returns true if it existed.
    pub fn revoke_share(&self, token: &str) -> bool {
        self.registry.lock().unwrap().remove(token).is_some()
    }

    pub fn active_count(&self) -> usize {
        self.registry.lock().unwrap().len()
    }
```

Then add a test module at the bottom of `state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn add_then_revoke() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"data").unwrap();
        let st = AppState::new(0, "cloudflared".into());
        let token = st.add_share(f.path().to_path_buf(), None).unwrap();
        assert_eq!(st.active_count(), 1);
        assert!(st.revoke_share(&token));
        assert_eq!(st.active_count(), 0);
        assert!(!st.revoke_share(&token));
    }

    #[test]
    fn add_share_reads_name_and_size() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"12345").unwrap();
        let st = AppState::new(0, "cloudflared".into());
        let token = st.add_share(f.path().to_path_buf(), Some("pw".into())).unwrap();
        let reg = st.registry.lock().unwrap();
        let share = reg.get(&token).unwrap();
        assert_eq!(share.size, 5);
        assert!(share.password_hash.is_some());
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cd src-tauri && cargo test state::`
Expected: PASS (2 tests).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/state.rs
git commit -m "feat: add share create/revoke/count helpers to AppState"
```

---

## Task 10: Tauri commands and DTO

**Files:**
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod commands;`)

- [ ] **Step 1: Write the command module with a unit test**

Create `src-tauri/src/commands.rs`:

```rust
use crate::state::AppState;
use serde::Serialize;
use tauri::State;

/// Serializable view of a share sent to the frontend.
#[derive(Serialize, Clone)]
pub struct ShareInfo {
    pub token: String,
    pub name: String,
    pub size: u64,
    pub has_password: bool,
    pub download_count: u64,
    pub link: Option<String>,
}

/// Builds the public link for a token, if the tunnel base URL is known.
fn build_link(base: &Option<String>, token: &str) -> Option<String> {
    base.as_ref().map(|b| format!("{b}/d/{token}"))
}

fn share_infos(state: &AppState) -> Vec<ShareInfo> {
    let base = state.tunnel.lock().unwrap().base_url();
    state
        .registry
        .lock()
        .unwrap()
        .list()
        .into_iter()
        .map(|s| ShareInfo {
            link: build_link(&base, &s.token),
            token: s.token,
            name: s.name,
            size: s.size,
            has_password: s.password_hash.is_some(),
            download_count: s.download_count,
        })
        .collect()
}

#[tauri::command]
pub async fn create_share(
    path: String,
    password: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<ShareInfo>, String> {
    let pw = password.filter(|p| !p.is_empty());
    state
        .add_share(std::path::PathBuf::from(path), pw)
        .map_err(|e| e.to_string())?;

    // Lazily start the tunnel if it is not already running.
    let needs_start = !state.tunnel.lock().unwrap().is_running();
    if needs_start {
        let port = state.port;
        let mut guard = state.tunnel.lock().unwrap();
        // start() is async; we run it to completion while holding intent.
        // Note: actual await happens outside the lock in the real impl below.
        drop(guard);
        let mut mgr = state.tunnel.lock().unwrap();
        mgr.start(port).await.map_err(|e| e.to_string())?;
    }

    Ok(share_infos(&state))
}

#[tauri::command]
pub fn revoke_share(token: String, state: State<'_, AppState>) -> Vec<ShareInfo> {
    state.revoke_share(&token);
    // Stop the tunnel if no shares remain.
    if state.active_count() == 0 {
        state.tunnel.lock().unwrap().stop();
    }
    share_infos(&state)
}

#[tauri::command]
pub fn list_shares(state: State<'_, AppState>) -> Vec<ShareInfo> {
    share_infos(&state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_link_formats_correctly() {
        let base = Some("https://x.trycloudflare.com".to_string());
        assert_eq!(
            build_link(&base, "abc"),
            Some("https://x.trycloudflare.com/d/abc".to_string())
        );
    }

    #[test]
    fn build_link_none_when_no_base() {
        assert_eq!(build_link(&None, "abc"), None);
    }
}
```

> **Implementation note for the executor:** the `create_share` tunnel-start block above must not hold the `Mutex` across the `.await`. Use this corrected body for the start section instead:
>
> ```rust
>     if !state.tunnel.lock().unwrap().is_running() {
>         let port = state.port;
>         // Take the manager out behavior is avoided: instead, make start() take &self
>         // OR clone an Arc. Simplest: hold the lock only inside start via interior state.
>     }
> ```
>
> Because `TunnelManager::start` needs `&mut self` and is async, refactor `TunnelManager` so the child handle and base_url live behind `Arc<Mutex<...>>` internally and `start` takes `&self`. Apply Task 10b before running.

- [ ] **Step 2: Run the unit tests**

Run: `cd src-tauri && cargo test commands::`
Expected: PASS (2 tests for `build_link`).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/commands.rs
git commit -m "feat: add Tauri commands and ShareInfo DTO"
```

---

## Task 10b: Refactor TunnelManager for async-safe shared start

**Files:**
- Modify: `src-tauri/src/tunnel.rs`
- Modify: `src-tauri/src/commands.rs`

This removes the "lock held across await" problem by making the child handle interior-mutable so `start` takes `&self`.

- [ ] **Step 1: Change `TunnelManager` internals**

Replace the `child: Option<...>` field and methods so the child lives behind an `Arc<Mutex<>>`:

```rust
pub struct TunnelManager {
    program: String,
    extra_args: Vec<String>,
    base_url: Arc<Mutex<Option<String>>>,
    child: Arc<Mutex<Option<tokio::process::Child>>>,
}

impl TunnelManager {
    pub fn new(program: impl Into<String>, extra_args: Vec<String>) -> Self {
        TunnelManager {
            program: program.into(),
            extra_args,
            base_url: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn start(&self, port: u16) -> anyhow::Result<String> {
        let mut cmd = Command::new(&self.program);
        if self.extra_args.is_empty() {
            cmd.arg("tunnel").arg("--url").arg(format!("http://127.0.0.1:{port}"));
        } else {
            cmd.args(&self.extra_args);
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        *self.child.lock().unwrap() = Some(child);

        match scan_for_url(stdout, stderr, self.base_url.clone()).await {
            Some(url) => {
                *self.base_url.lock().unwrap() = Some(url.clone());
                Ok(url)
            }
            None => anyhow::bail!("tunnel URL not found before stream closed"),
        }
    }

    pub fn base_url(&self) -> Option<String> {
        self.base_url.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.start_kill();
        }
        *self.child.lock().unwrap() = None;
        *self.base_url.lock().unwrap() = None;
    }

    pub fn is_running(&self) -> bool {
        self.child.lock().unwrap().is_some()
    }
}
```

- [ ] **Step 2: Update the tunnel test** to use `&` (the existing test calls `mgr.start`/`mgr.stop` — change `let mut mgr` to `let mgr`):

```rust
    #[tokio::test]
    async fn start_captures_url_from_fake_binary() {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fake_cloudflared.sh");
        let mgr = TunnelManager::new(script, vec![script.to_string()]);
        let url = mgr.start(12345).await.expect("should capture url");
        assert_eq!(url, "https://fake-test-tunnel.trycloudflare.com");
        assert!(mgr.is_running());
        mgr.stop();
        assert!(!mgr.is_running());
    }
```

- [ ] **Step 3: Simplify `create_share` start block** in `commands.rs`:

```rust
    if !state.tunnel.lock().unwrap().is_running() {
        let port = state.port;
        // Clone the manager handle (cheap: Arc fields) so we don't hold the
        // outer lock across await.
        let mgr = { state.tunnel.lock().unwrap().clone_handle() };
        mgr.start(port).await.map_err(|e| e.to_string())?;
    }
```

And add a cheap handle clone to `TunnelManager`:

```rust
    /// Returns a handle sharing the same child/base_url (cheap Arc clone).
    pub fn clone_handle(&self) -> TunnelManager {
        TunnelManager {
            program: self.program.clone(),
            extra_args: self.extra_args.clone(),
            base_url: self.base_url.clone(),
            child: self.child.clone(),
        }
    }
```

- [ ] **Step 4: Run all Rust tests**

Run: `cd src-tauri && cargo test`
Expected: PASS (all tests across modules).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tunnel.rs src-tauri/src/commands.rs
git commit -m "refactor: make TunnelManager async-safe via interior Arc handles"
```

---

## Task 11: Wire server, state, commands, and tray into the app

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Replace `lib.rs` `run()` with full wiring**

```rust
mod token;
mod password;
mod share;
mod tunnel;
mod state;
mod server;
mod commands;

use state::AppState;
use std::net::SocketAddr;
use tauri::tray::TrayIconBuilder;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::Manager;

fn pick_free_port() -> u16 {
    // Bind to port 0 to let the OS choose, then release it.
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

/// Resolves the cloudflared sidecar path; falls back to PATH lookup.
fn cloudflared_path(_app: &tauri::AppHandle) -> String {
    // The Tauri shell sidecar resolves the bundled binary at runtime;
    // for the spawned process we use the resolved path or "cloudflared".
    "cloudflared".to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let port = pick_free_port();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .setup(move |app| {
            let cf = cloudflared_path(&app.handle());
            let app_state = AppState::new(port, cf);
            app.manage(app_state.clone());

            // Launch the local axum server.
            let router = server::build_router(app_state.clone());
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
                axum::serve(listener, router).await.unwrap();
            });

            // Tray icon with a quit item.
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app).items(&[&quit]).build()?;
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
                        // Tear down the tunnel before exiting.
                        if let Some(state) = app.try_state::<AppState>() {
                            state.tunnel.lock().unwrap().stop();
                        }
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, _event| {
                    // Show the main window when the tray icon is clicked.
                    if let Some(win) = tray.app_handle().get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_share,
            commands::revoke_share,
            commands::list_shares
        ])
        .on_window_event(|window, event| {
            // Hide instead of quitting when the window is closed (tray app behavior).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 2: Ensure the main window has label "main"**

In `src-tauri/tauri.conf.json`, add `"label": "main"` to the window object in `app.windows[0]`.

- [ ] **Step 3: Build**

Run: `cd src-tauri && cargo build`
Expected: compiles. (Tray APIs require the `tray-icon` feature already in Cargo.toml.)

- [ ] **Step 4: Run all tests**

Run: `cd src-tauri && cargo test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/tauri.conf.json
git commit -m "feat: wire server, commands, and tray into Tauri app"
```

---

## Task 12: Frontend UI

**Files:**
- Modify: `src/index.html`
- Create: `src/main.js`
- Create: `src/styles.css`

- [ ] **Step 1: Write `src/index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <link rel="stylesheet" href="styles.css" />
    <title>LocalRemoteShare</title>
  </head>
  <body>
    <main>
      <div id="dropzone">Drag a file here, or <button id="pick">choose a file</button></div>
      <label id="pw-row">
        <input type="checkbox" id="use-pw" /> Password protect
        <input type="password" id="pw" placeholder="password" disabled />
      </label>
      <ul id="shares"></ul>
      <p id="status"></p>
    </main>
    <script type="module" src="main.js"></script>
  </body>
</html>
```

- [ ] **Step 2: Write `src/styles.css`**

```css
:root { font-family: system-ui, sans-serif; }
body { margin: 0; padding: 1rem; }
#dropzone {
  border: 2px dashed #888; border-radius: 12px; padding: 2rem 1rem;
  text-align: center; color: #555; margin-bottom: 1rem;
}
#dropzone.over { border-color: #2b8a3e; color: #2b8a3e; }
#pw-row { display: flex; align-items: center; gap: .5rem; margin-bottom: 1rem; font-size: .9rem; }
ul { list-style: none; padding: 0; margin: 0; }
li { border: 1px solid #ddd; border-radius: 8px; padding: .6rem; margin-bottom: .5rem; }
li .name { font-weight: 600; }
li .link { font-size: .8rem; word-break: break-all; color: #1c7ed6; }
li button { font-size: .8rem; margin-right: .4rem; cursor: pointer; }
#status { color: #888; font-size: .8rem; min-height: 1rem; }
```

- [ ] **Step 3: Write `src/main.js`**

```js
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";

const dropzone = document.getElementById("dropzone");
const sharesEl = document.getElementById("shares");
const statusEl = document.getElementById("status");
const usePw = document.getElementById("use-pw");
const pwInput = document.getElementById("pw");
const pickBtn = document.getElementById("pick");

usePw.addEventListener("change", () => { pwInput.disabled = !usePw.checked; });

function setStatus(msg) { statusEl.textContent = msg; }

async function createShare(path) {
  setStatus("Starting tunnel…");
  const password = usePw.checked ? pwInput.value : null;
  try {
    const shares = await invoke("create_share", { path, password });
    render(shares);
    setStatus("");
  } catch (e) {
    setStatus("Error: " + e);
  }
}

async function revoke(token) {
  const shares = await invoke("revoke_share", { token });
  render(shares);
}

function render(shares) {
  sharesEl.innerHTML = "";
  for (const s of shares) {
    const li = document.createElement("li");
    const link = s.link ?? "(link pending…)";
    li.innerHTML = `
      <div class="name">${s.name} <small>(${(s.size / 1024).toFixed(1)} KB)</small></div>
      <div class="link">${link}</div>
      <div>
        <button class="copy">Copy link</button>
        <button class="revoke">Revoke</button>
        <small>${s.has_password ? "🔒 " : ""}${s.download_count} downloads</small>
      </div>`;
    li.querySelector(".copy").onclick = () => navigator.clipboard.writeText(link);
    li.querySelector(".revoke").onclick = () => revoke(s.token);
    sharesEl.appendChild(li);
  }
}

pickBtn.addEventListener("click", async () => {
  const path = await open({ multiple: false });
  if (typeof path === "string") createShare(path);
});

// Tauri file drop events.
getCurrentWebview().onDragDropEvent((event) => {
  if (event.payload.type === "over") {
    dropzone.classList.add("over");
  } else if (event.payload.type === "drop") {
    dropzone.classList.remove("over");
    const paths = event.payload.paths;
    if (paths && paths.length > 0) createShare(paths[0]);
  } else {
    dropzone.classList.remove("over");
  }
});

// Initial render.
invoke("list_shares").then(render);
```

- [ ] **Step 4: Add the dialog plugin**

In `src-tauri/Cargo.toml` `[dependencies]` add:

```toml
tauri-plugin-dialog = "2"
```

In `src-tauri/src/lib.rs`, add to the builder chain (next to the shell plugin):

```rust
        .plugin(tauri_plugin_dialog::init())
```

- [ ] **Step 5: Create a frontend `package.json` for the JS API modules**

Create `package.json` at the project root:

```json
{
  "name": "local-remote-share-ui",
  "private": true,
  "dependencies": {
    "@tauri-apps/api": "^2",
    "@tauri-apps/plugin-dialog": "^2"
  }
}
```

Then install:

Run: `npm install`
Expected: `node_modules` created with the Tauri API packages.

> **Note:** `frontendDist` points at `../src` (plain files). Bare module specifiers like `@tauri-apps/api/core` require a bundler. For v1 simplicity, either (a) add a minimal Vite setup, or (b) replace the imports with the global `window.__TAURI__` API. The executor should pick (b) for the leanest path: replace `import { invoke } ...` with `const { invoke } = window.__TAURI__.core;` etc., and enable `app.withGlobalTauri: true` in `tauri.conf.json`.

- [ ] **Step 6: Apply the global-Tauri approach**

In `src-tauri/tauri.conf.json`, set `"app": { "withGlobalTauri": true, ... }`. In `src/main.js`, replace the three import lines with:

```js
const { invoke } = window.__TAURI__.core;
const { open } = window.__TAURI__.dialog;
const { getCurrentWebview } = window.__TAURI__.webview;
```

- [ ] **Step 7: Run the app**

Run: `cd src-tauri && cargo tauri dev` (or `cargo run`)
Expected: window opens with the dropzone; tray icon appears.

- [ ] **Step 8: Commit**

```bash
git add src/index.html src/main.js src/styles.css src-tauri/Cargo.toml src-tauri/src/lib.rs src-tauri/tauri.conf.json package.json
git commit -m "feat: add minimal drag-drop frontend UI"
```

---

## Task 13: Bundle cloudflared as a sidecar

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Create: `src-tauri/binaries/` (downloaded binaries)
- Modify: `src-tauri/src/lib.rs` (resolve sidecar path)

- [ ] **Step 1: Download cloudflared binaries into `src-tauri/binaries/`**

Tauri sidecars must be named `<name>-<target-triple>`. Download the right binary per platform:

```bash
mkdir -p src-tauri/binaries
# macOS (Apple Silicon example). Adjust per host/target:
#   https://github.com/cloudflare/cloudflared/releases/latest
# Place and rename, e.g.:
#   src-tauri/binaries/cloudflared-aarch64-apple-darwin
#   src-tauri/binaries/cloudflared-x86_64-apple-darwin
#   src-tauri/binaries/cloudflared-x86_64-pc-windows-msvc.exe
chmod +x src-tauri/binaries/cloudflared-*
```

To find your host triple: `rustc -Vv | grep host`.

- [ ] **Step 2: Declare the sidecar and shell permission**

In `src-tauri/tauri.conf.json`, add to `bundle`:

```json
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/icon.png"],
    "externalBin": ["binaries/cloudflared"]
  }
```

Create `src-tauri/capabilities/default.json` (Tauri v2 capabilities):

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default capability set",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "dialog:default",
    "shell:allow-execute",
    {
      "identifier": "shell:allow-execute",
      "allow": [{ "name": "binaries/cloudflared", "sidecar": true }]
    }
  ]
}
```

- [ ] **Step 3: Resolve the sidecar path at runtime**

Replace `cloudflared_path` in `src-tauri/src/lib.rs` with a resolver that prefers the bundled sidecar and falls back to PATH:

```rust
fn cloudflared_path(app: &tauri::AppHandle) -> String {
    use tauri::path::BaseDirectory;
    // Tauri places sidecars next to the executable at runtime.
    if let Ok(p) = app
        .path()
        .resolve("cloudflared", BaseDirectory::Resource)
    {
        if p.exists() {
            return p.to_string_lossy().to_string();
        }
    }
    "cloudflared".to_string() // fall back to PATH
}
```

> **Note for executor:** the exact resolved location of an `externalBin` differs between `dev` and bundled builds. If resolution fails in `dev`, keep the `"cloudflared"` PATH fallback (install cloudflared locally via `brew install cloudflared`) so development still works. Verify the bundled path with a quick log line during the first packaged build.

- [ ] **Step 4: Build**

Run: `cd src-tauri && cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/tauri.conf.json src-tauri/capabilities src-tauri/src/lib.rs
git commit -m "feat: bundle cloudflared as a sidecar with PATH fallback"
```

(Do not commit the large binaries if undesired; add `src-tauri/binaries/` to `.gitignore` and document the download step instead.)

---

## Task 14: Manual end-to-end checklist

**Files:**
- Create: `docs/superpowers/MANUAL-E2E.md`

- [ ] **Step 1: Write the checklist**

Create `docs/superpowers/MANUAL-E2E.md`:

```markdown
# LocalRemoteShare — Manual E2E Checklist

Prerequisite: `cloudflared` installed (bundled sidecar, or `brew install cloudflared`).

1. Launch the app (`cd src-tauri && cargo tauri dev`). Window + tray icon appear.
2. Drag a small file (e.g. a PDF) onto the dropzone.
   - A share row appears; within ~10s a `https://*.trycloudflare.com/d/<token>` link shows.
3. Click "Copy link", open it in a browser (or another device).
   - Landing page shows the file name and size.
   - Click Download → file downloads intact. Share row download count increments to 1.
4. Add a second file with "Password protect" checked and a password.
   - Open its link → landing page shows a password field.
   - Submit a wrong password → "Incorrect password." Submit the right one → file downloads.
5. Click "Revoke" on a share → its link now returns 404.
6. Revoke all shares → confirm the cloudflared process exits (tunnel down).
7. Quit via the tray menu → confirm no `cloudflared` process remains running
   (`pgrep cloudflared` returns nothing).
8. Move/delete a shared file on disk, then try its link → landing page or download
   reports the file is gone (410).
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/MANUAL-E2E.md
git commit -m "docs: add manual end-to-end test checklist"
```

---

## Self-Review Notes

- **Spec coverage:** tray + window (Task 11, 12), drag-drop/select (Task 12), single-file share (Task 9), Cloudflare quick tunnel (Tasks 5, 6, 13), landing page (Task 7), optional password (Tasks 3, 8), multiple concurrent shares (Task 4, lazy tunnel start/stop in Tasks 10/10b/commands), revoke + 404 (Tasks 9, 7), ephemeral state / teardown on quit (Task 11), 127.0.0.1-only binding (Task 11), file-moved 410 (Task 7), port conflict via ephemeral port pick (Task 11), bundled cloudflared with fallback (Task 13), tests incl. mock cloudflared (Task 6) and manual E2E (Task 14). All spec sections map to tasks.
- **Async-safety:** Task 10 intentionally introduces the naive version and Task 10b corrects the "lock across await" issue — execute 10b before running the app.
- **Type consistency:** `ShareInfo`, `AppState`, `TunnelManager`, `ShareRegistry`, `Share` names and method signatures (`add_share`, `revoke_share`, `active_count`, `start`, `stop`, `is_running`, `base_url`, `clone_handle`) are consistent across tasks.
- **Known executor decisions flagged inline:** global-Tauri vs bundler (Task 12), sidecar path resolution in dev vs bundled (Task 13), whether to commit binaries (Task 13).
