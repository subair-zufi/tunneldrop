use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Supervises a single cloudflared child process and captures its public URL.
pub struct TunnelManager {
    program: String,
    extra_args: Vec<String>,
    base_url: Arc<Mutex<Option<String>>>,
    child: Arc<Mutex<Option<tokio::process::Child>>>,
}

impl TunnelManager {
    /// `program` is the path to the cloudflared binary (or a fake for tests).
    /// When `extra_args` is non-empty, it REPLACES the standard tunnel args
    /// entirely (used for testing with a fake binary). When empty, the standard
    /// `tunnel --url http://127.0.0.1:{port}` args are used.
    pub fn new(program: impl Into<String>, extra_args: Vec<String>) -> Self {
        TunnelManager {
            program: program.into(),
            extra_args,
            base_url: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts the tunnel pointed at the given local port and waits (up to ~15s)
    /// for the public URL to appear in stdout. Returns the URL.
    pub async fn start(&self, port: u16) -> anyhow::Result<String> {
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
        *self.child.lock().unwrap() = Some(child);

        let base_url = self.base_url.clone();
        // cloudflared prints the URL to stderr in real life and our fake uses
        // stdout; scan both.
        let found = scan_for_url(stdout, stderr).await;
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
    pub fn stop(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.start_kill();
        }
        *self.child.lock().unwrap() = None;
        *self.base_url.lock().unwrap() = None;
    }

    /// Returns whether the tunnel process is alive. Reaps a child that has
    /// exited on its own (cloudflared crash) so the next `create_share` will
    /// transparently restart the tunnel rather than trusting a stale handle.
    pub fn is_running(&self) -> bool {
        let mut guard = self.child.lock().unwrap();
        let exited = match guard.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(Some(_))),
            None => return false,
        };
        if exited {
            *guard = None;
            drop(guard);
            *self.base_url.lock().unwrap() = None;
            return false;
        }
        true
    }

    /// Returns a handle sharing the same child/base_url state (cheap Arc clone).
    pub fn clone_handle(&self) -> TunnelManager {
        TunnelManager {
            program: self.program.clone(),
            extra_args: self.extra_args.clone(),
            base_url: self.base_url.clone(),
            child: self.child.clone(),
        }
    }
}

async fn scan_for_url(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
) -> Option<String> {
    let mut out = BufReader::new(stdout).lines();
    let mut err = BufReader::new(stderr).lines();
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(15));
    tokio::pin!(deadline);
    // Track per-stream EOF so a closed pipe (e.g. the child exited before
    // printing a URL) doesn't busy-spin the select loop until the deadline.
    let (mut out_done, mut err_done) = (false, false);
    loop {
        if out_done && err_done {
            return None; // process exited without printing a URL
        }
        tokio::select! {
            () = &mut deadline => return None,
            line = out.next_line(), if !out_done => {
                match line {
                    Ok(Some(l)) => if let Some(u) = parse_tunnel_url(&l) { return Some(u); },
                    Ok(None) => out_done = true,
                    Err(_) => return None,
                }
            }
            line = err.next_line(), if !err_done => {
                match line {
                    Ok(Some(l)) => if let Some(u) = parse_tunnel_url(&l) { return Some(u); },
                    Ok(None) => err_done = true,
                    Err(_) => return None,
                }
            }
        }
    }
}

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

    #[tokio::test]
    async fn start_errors_quickly_when_process_exits_without_url() {
        // The fake exits immediately without printing a URL. scan_for_url must
        // return on double-EOF rather than busy-spinning until the 15s deadline.
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/fake_cloudflared_nourl.sh");
        let mgr = TunnelManager::new(script, vec![script.to_string()]);
        let started = std::time::Instant::now();
        let result = mgr.start(12345).await;
        assert!(result.is_err(), "expected error when no URL is printed");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "should fail fast on EOF, not wait for the 15s deadline (took {:?})",
            started.elapsed()
        );
        // The process exited on its own; is_running must reap it and report false
        // (so a later create_share will restart the tunnel rather than trust a
        // stale handle).
        assert!(!mgr.is_running(), "a crashed/exited process must read as not running");
        mgr.stop();
    }
}
