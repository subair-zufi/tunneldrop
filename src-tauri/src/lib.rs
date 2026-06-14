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

/// Resolves the cloudflared binary path.
/// Prefers the bundled sidecar, which Tauri's externalBin mechanism places
/// NEXT TO the app executable (with the target-triple suffix stripped), not in
/// the resource dir. Falls back to "cloudflared" on PATH (dev / Homebrew).
fn cloudflared_path(_app: &tauri::AppHandle) -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["cloudflared", "cloudflared.exe"] {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
    }
    "cloudflared".to_string() // fall back to PATH
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let port = pick_free_port();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let cf = cloudflared_path(app.handle());
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
