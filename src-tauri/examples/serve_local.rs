//! Headless test: start the axum server, add a share, print the local URL,
//! then download the file and confirm the bytes match — no cloudflared needed.
use local_remote_share_lib::{server, state::AppState};
use std::io::Write;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port: u16 = 18432;
    let state = AppState::new(port, "cloudflared".into());

    // Create a real temp file to share.
    let mut tmpfile = tempfile::NamedTempFile::new()?;
    writeln!(tmpfile, "Hello from Tunneldrop!")?;
    writeln!(tmpfile, "File name : sample.txt")?;
    writeln!(tmpfile, "Served by : axum 0.7 inside Tauri 2")?;
    writeln!(tmpfile, "Tunnel    : Cloudflare quick tunnel (trycloudflare.com)")?;
    let tmppath = tmpfile.path().to_path_buf();
    let size    = std::fs::metadata(&tmppath)?.len();

    let token = state.add_share(tmppath, None)?;

    // Start server.
    let router = server::build_router(state.clone());
    let addr   = SocketAddr::from(([127, 0, 0, 1], port));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, router).await.unwrap();
    });

    let base = format!("http://127.0.0.1:{port}");
    println!("=== Server running on {base} ===");
    println!("Token : {token}");
    println!("File  : {size} bytes");
    println!();

    // Give server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // ── test 1: landing page ────────────────────────────────────────────────
    let lp_url = format!("{base}/d/{token}");
    let lp = reqwest::get(&lp_url).await?;
    println!("GET  {}  →  {}", lp_url, lp.status());
    let html = lp.text().await?;
    assert!(html.contains("Download"), "landing page must contain download widget");
    assert!(html.contains(&format!(r#"href="/d/{token}/download""#)),
            "must use GET link (not POST form) for unprotected share");
    println!("     landing page OK — GET link present, no POST form ✓");

    // ── test 2: direct GET download ─────────────────────────────────────────
    let dl_url = format!("{base}/d/{token}/download");
    let dl = reqwest::get(&dl_url).await?;
    println!("GET  {}  →  {}", dl_url, dl.status());
    assert_eq!(dl.status(), 200);
    let body = dl.bytes().await?;
    println!("     downloaded {} bytes ✓", body.len());
    assert_eq!(body.len() as u64, size);

    // ── test 3: revoked token returns 404 ───────────────────────────────────
    state.revoke_share(&token);
    let miss = reqwest::get(&lp_url).await?;
    println!("GET  {} (revoked)  →  {}", lp_url, miss.status());
    assert_eq!(miss.status(), 404);
    println!("     revoked share is 404 ✓");

    println!();
    println!("All local checks passed.");
    Ok(())
}
