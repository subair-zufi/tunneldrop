/// Headless demo: starts the axum server + cloudflared, adds a share from
/// a temp file, prints the public link, then waits for Ctrl-C.
///
/// Run: cargo run --example demo
use local_remote_share_lib::{server, state::AppState};
use std::io::Write;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0")?;
        l.local_addr()?.port()
    };

    let cf = std::env::var("CLOUDFLARED_PATH")
        .unwrap_or_else(|_| "cloudflared".to_string());

    let state = AppState::new(port, cf);

    let mut tmpfile = tempfile::NamedTempFile::new()?;
    tmpfile.write_all(
        b"Hello from LocalRemoteShare! This file was shared via Cloudflare tunnel.\n",
    )?;
    let tmppath = tmpfile.path().to_path_buf();
    let token = state.add_share(tmppath, None)?;

    let router = server::build_router(state.clone());
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        axum::serve(listener, router).await.unwrap();
    });

    println!("Local server running on http://127.0.0.1:{port}");
    println!("Starting cloudflared tunnel (takes ~10 s)…");

    let tunnel_url = state.tunnel.lock().unwrap().clone_handle().start(port).await?;

    let link = format!("{tunnel_url}/d/{token}");
    println!();
    println!("=== Share link ===");
    println!("{link}");
    println!("==================");
    println!();
    println!("Landing page:  GET {link}");
    println!("Direct DL:     GET {link}/download");
    println!();
    println!("Press Ctrl-C to stop the tunnel and delete the temp file.");

    tokio::signal::ctrl_c().await?;
    drop(tmpfile);
    Ok(())
}
