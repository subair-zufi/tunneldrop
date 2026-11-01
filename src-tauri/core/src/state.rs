use crate::share::{ShareKind, ShareRegistry};
use crate::tunnel::{TunnelManager, TunnelProvider};
use std::sync::{Arc, Mutex};

/// Shared across the axum server and Tauri commands.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Mutex<ShareRegistry>>,
    pub tunnel: Arc<dyn TunnelProvider>,
    pub port: u16,
    /// Held while starting the tunnel, so concurrent callers start it once.
    tunnel_start: Arc<tokio::sync::Mutex<()>>,
}

/// Starts the tunnel if it is not running.
pub async fn ensure_tunnel(state: &AppState) -> anyhow::Result<()> {
    let _guard = state.tunnel_start.lock().await;
    if !state.tunnel.is_running() {
        state.tunnel.start(state.port).await?;
    }
    Ok(())
}

/// Stops the tunnel if nothing is being shared any more.
pub fn stop_tunnel_if_idle(state: &AppState) {
    if state.active_count() == 0 {
        state.tunnel.stop();
    }
}

impl AppState {
    pub fn new(port: u16, cloudflared_path: String) -> Self {
        Self::with_provider(port, Arc::new(TunnelManager::new(cloudflared_path, vec![])))
    }

    pub fn with_provider(port: u16, provider: Arc<dyn TunnelProvider>) -> Self {
        AppState {
            registry: Arc::new(Mutex::new(ShareRegistry::new())),
            tunnel: provider,
            port,
            tunnel_start: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

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

    /// Registers a share over an already-open descriptor, for sources that have
    /// no path — the Android picker's `content://` URIs. Name and size come
    /// from the provider rather than the filesystem.
    #[cfg(target_os = "android")]
    pub fn add_share_fd(
        &self,
        fd: std::os::fd::OwnedFd,
        name: String,
        size: u64,
        password: Option<String>,
    ) -> anyhow::Result<String> {
        let token = crate::token::generate_token();
        let password_hash = password.map(|p| crate::password::hash_password(&p));
        let share = crate::share::Share::new(token.clone(), fd, name, size, password_hash);
        self.registry.lock().unwrap().insert(share);
        Ok(token)
    }

    /// Registers whatever the platform's file picker returned. On Android that
    /// can be a content:// URI, which has no filesystem path behind it — the
    /// provider opens the file for us instead.
    pub fn add_from_picker(
        &self,
        path: String,
        password: Option<String>,
        kind: ShareKind,
    ) -> anyhow::Result<String> {
        #[cfg(target_os = "android")]
        let token = if path.starts_with("content://") {
            let picked = crate::android_fs::open_content_uri(&path)?;
            self.add_share_fd(picked.fd, picked.name, picked.size, password)?
        } else {
            self.add_share(std::path::PathBuf::from(path), password)?
        };

        #[cfg(not(target_os = "android"))]
        let token = self.add_share(std::path::PathBuf::from(path), password)?;

        if let Some(share) = self.registry.lock().unwrap().get_mut(&token) {
            share.kind = kind;
        }
        Ok(token)
    }

    /// Removes a share. Returns true if it existed.
    pub fn revoke_share(&self, token: &str) -> bool {
        self.registry.lock().unwrap().remove(token).is_some()
    }

    pub fn active_count(&self) -> usize {
        self.registry.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FakeProvider {
        url: String,
        start_calls: AtomicUsize,
        running: AtomicBool,
        base: Mutex<Option<String>>,
    }

    impl FakeProvider {
        fn new(url: &str) -> Arc<Self> {
            Arc::new(FakeProvider {
                url: url.to_string(),
                start_calls: AtomicUsize::new(0),
                running: AtomicBool::new(false),
                base: Mutex::new(None),
            })
        }

        fn start_calls(&self) -> usize {
            self.start_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TunnelProvider for FakeProvider {
        async fn start(&self, _port: u16) -> anyhow::Result<String> {
            self.start_calls.fetch_add(1, Ordering::SeqCst);
            // Long enough for a second caller to arrive while this one starts.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.running.store(true, Ordering::SeqCst);
            *self.base.lock().unwrap() = Some(self.url.clone());
            Ok(self.url.clone())
        }
        fn base_url(&self) -> Option<String> {
            self.base.lock().unwrap().clone()
        }
        fn stop(&self) {
            self.running.store(false, Ordering::SeqCst);
            *self.base.lock().unwrap() = None;
        }
        fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn ensure_tunnel_starts_once() {
        let fake = FakeProvider::new("https://fake.example");
        let st = AppState::with_provider(0, fake.clone());
        ensure_tunnel(&st).await.unwrap();
        ensure_tunnel(&st).await.unwrap();
        assert_eq!(fake.start_calls(), 1);
        assert_eq!(st.tunnel.base_url().as_deref(), Some("https://fake.example"));
    }

    /// Two transfers can need the tunnel at the same moment. A second start
    /// would overwrite the first child process and leave it running forever.
    #[tokio::test]
    async fn concurrent_ensure_tunnel_starts_once() {
        let fake = FakeProvider::new("https://fake.example");
        let st = AppState::with_provider(0, fake.clone());
        let (a, b) = tokio::join!(ensure_tunnel(&st), ensure_tunnel(&st));
        a.unwrap();
        b.unwrap();
        assert_eq!(fake.start_calls(), 1);
    }

    #[tokio::test]
    async fn stop_tunnel_if_idle_keeps_running_while_a_share_exists() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"data").unwrap();
        let fake = FakeProvider::new("https://fake.example");
        let st = AppState::with_provider(0, fake.clone());
        let token = st.add_share(f.path().to_path_buf(), None).unwrap();
        ensure_tunnel(&st).await.unwrap();

        stop_tunnel_if_idle(&st);
        assert!(st.tunnel.is_running());

        st.revoke_share(&token);
        stop_tunnel_if_idle(&st);
        assert!(!st.tunnel.is_running());
    }

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
