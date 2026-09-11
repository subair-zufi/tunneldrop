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
