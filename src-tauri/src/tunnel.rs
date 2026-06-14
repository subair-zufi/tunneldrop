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
        let mut mgr = TunnelManager::new(script, vec![script.to_string()]);
        let url = mgr.start(12345).await.expect("should capture url");
        assert_eq!(url, "https://fake-test-tunnel.trycloudflare.com");
        assert!(mgr.is_running());
        mgr.stop();
        assert!(!mgr.is_running());
    }
}
