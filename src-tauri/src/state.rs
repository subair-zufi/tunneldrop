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
