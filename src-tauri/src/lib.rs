mod token;
mod password;
mod share;
pub mod tunnel;
pub mod state;
pub mod server;
mod commands;

use state::AppState;
use std::net::SocketAddr;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
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

/// Shared flag: was the tray icon successfully created?
/// Used by the window-close handler to decide whether to hide or quit.
struct TrayAvailable(Arc<AtomicBool>);

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

            let tray_created = Arc::new(AtomicBool::new(false));

            // On Linux the system tray requires libayatana-appindicator3 and a
            // compatible desktop environment (GNOME needs the AppIndicator Shell
            // extension). Build failure is non-fatal: the app continues without a
            // tray icon and window-close will quit instead of hide.
            // Dedicated monochrome tray glyph (not the colorful app icon, which
            // is illegible at menu-bar size). On macOS it is flagged as a
            // template image so the system tints it for light/dark menu bars,
            // and we ship the @2x (44px) asset there; Windows/Linux trays use
            // the smaller 32px glyph.
            #[cfg(target_os = "macos")]
            let tray_icon_bytes: &[u8] = include_bytes!("../icons/MenuIcon44.png");
            #[cfg(not(target_os = "macos"))]
            let tray_icon_bytes: &[u8] = include_bytes!("../icons/MenuIcon32.png");
            let icon = tauri::image::Image::from_bytes(tray_icon_bytes)
                .expect("embedded tray icon must decode");

            // On Linux, libloading opens libayatana-appindicator3.so.1 lazily
            // inside tray-icon. If the library is absent the Lazy initialiser
            // panics rather than returning an error, so we need catch_unwind.
            // On macOS and Windows build() returns Err (never panics).
            let tray_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let builder = TrayIconBuilder::new()
                    .icon(icon)
                    .menu(&menu)
                    // Right-click shows the menu (Quit); left-click is reserved
                    // for opening the window. Without this, a left-click would
                    // pop the menu on some platforms.
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| {
                        if event.id() == "quit" {
                            // Tear down the tunnel before exiting.
                            if let Some(state) = app.try_state::<AppState>() {
                                state.tunnel.lock().unwrap().stop();
                            }
                            app.exit(0);
                        }
                    })
                    .on_tray_icon_event(|tray, event| {
                        // Open the window only on a completed left click, not on
                        // hover/move/enter events (which previously triggered it).
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            if let Some(win) = tray.app_handle().get_webview_window("main") {
                                let _ = win.show();
                                let _ = win.set_focus();
                            }
                        }
                    });
                #[cfg(target_os = "macos")]
                let builder = builder.icon_as_template(true);
                builder.build(app)
            }));

            match tray_result {
                Ok(Ok(_)) => tray_created.store(true, Ordering::SeqCst),
                Ok(Err(e)) => eprintln!(
                    "Tunneldrop: tray icon unavailable ({e}). \
                     Closing the window will quit the app."
                ),
                Err(_) => eprintln!(
                    "Tunneldrop: tray icon unavailable \
                     (libayatana-appindicator3 not found — install \
                     libayatana-appindicator3-1 and, on GNOME, the \
                     AppIndicator Shell extension). \
                     Closing the window will quit the app."
                ),
            }

            app.manage(TrayAvailable(tray_created));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_share,
            commands::revoke_share,
            commands::list_shares
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let tray_ok = window
                    .app_handle()
                    .try_state::<TrayAvailable>()
                    .map(|s| s.0.load(Ordering::SeqCst))
                    .unwrap_or(false);

                if tray_ok {
                    // Tray is available: hide to tray instead of quitting.
                    let _ = window.hide();
                    api.prevent_close();
                } else {
                    // No tray (or it failed to build). Let the window close and
                    // stop the tunnel so no cloudflared process is left behind.
                    if let Some(state) = window.app_handle().try_state::<AppState>() {
                        state.tunnel.lock().unwrap().stop();
                    }
                    // Allow the close to proceed — Tauri exits when the last window closes.
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
