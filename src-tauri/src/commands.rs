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
    let token = state
        .add_share(std::path::PathBuf::from(path), pw)
        .map_err(|e| e.to_string())?;

    // Lazily start the tunnel if it is not running. Clone a cheap handle so the
    // Mutex guard is never held across the await. If the tunnel fails to start,
    // roll back the share we just added so it doesn't linger un-revokable.
    if !state.tunnel.lock().unwrap().is_running() {
        let port = state.port;
        let mgr = { state.tunnel.lock().unwrap().clone_handle() };
        if let Err(e) = mgr.start(port).await {
            state.revoke_share(&token);
            return Err(e.to_string());
        }
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
