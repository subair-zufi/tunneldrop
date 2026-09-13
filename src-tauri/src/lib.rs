mod token;
mod password;
mod share;
#[cfg(target_os = "android")]
mod android_fs;
pub mod tunnel;
pub mod state;
pub mod server;
mod commands;

use state::AppState;
use std::net::SocketAddr;
use tauri::Manager;

// Tray, the menu it hangs off, and the close-to-tray flag exist on desktop
// only: Android and iOS have no system tray and no closable window.
#[cfg(desktop)]
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
#[cfg(desktop)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
#[cfg(desktop)]
use tauri::menu::{MenuBuilder, MenuItemBuilder};

fn pick_free_port() -> u16 {
    // Bind to port 0 to let the OS choose, then release it.
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .unwrap()
        .port()
}

/// Android: the app's `nativeLibraryDir`, the one app-private directory whose
/// contents the platform still lets us execute. Since API 29 a process may not
/// exec anything it can write, which rules out the data dir and the cache dir;
/// files the packager installs from `jniLibs/` land here with the exec bit set.
/// That is why cloudflared ships as `libcloudflared.so` — the name is the only
/// thing that gets it packaged and extracted, it is a normal ELF executable.
///
/// We ask the dynamic linker which file this function was loaded from, rather
/// than asking the JVM for the Activity's ApplicationInfo. The JNI route needs
/// an Android `Context`, and the usual way to reach one —
/// `ndk_context::android_context()` — *panics* ("android context was not
/// initialized") unless ndk-glue initialised it, which nothing in Tauri's stack
/// ever does: tao keeps its own activity registry and leaves ndk-context alone.
/// Since this runs inside Tauri's `setup`, that panic unwound into tao's FFI
/// boundary, which aborts rather than unwinds — the app died on every launch
/// with a bare SIGABRT and, because Android discards stderr, no message.
///
/// The linker needs no JVM, no Context and no particular thread, and its answer
/// is exactly as good: everything under `lib/<abi>/` in the APK is unpacked into
/// a single directory, so whatever holds `libtunneldrop_lib.so` also holds
/// `libcloudflared.so`.
///
/// Returns None if the linker cannot name our own mapping, which should not
/// happen for a library the platform loaded from a file.
#[cfg(target_os = "android")]
fn native_library_dir() -> Option<String> {
    use std::ffi::{c_void, CStr};

    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr only reads the address it is given and fills `info` with
    // pointers into the linker's own bookkeeping, which lives as long as the
    // library stays loaded. A zeroed Dl_info is a valid out-parameter.
    let found = unsafe { libc::dladdr(native_library_dir as *const c_void, &mut info) };
    if found == 0 || info.dli_fname.is_null() {
        return None;
    }
    // SAFETY: on success dladdr sets dli_fname to a NUL-terminated path owned
    // by the linker.
    let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_str().ok()?;
    let dir = std::path::Path::new(path).parent()?;
    Some(dir.to_string_lossy().into_owned())
}

/// Resolves the cloudflared binary path.
/// Prefers the bundled sidecar, which Tauri's externalBin mechanism places
/// NEXT TO the app executable (with the target-triple suffix stripped), not in
/// the resource dir. Falls back to "cloudflared" on PATH (dev / Homebrew).
fn cloudflared_path(_app: &tauri::AppHandle) -> String {
    // On Android there is no sidecar mechanism and current_exe() points at the
    // zygote (`/system/bin/app_process64`), so resolve the jniLibs copy instead.
    #[cfg(target_os = "android")]
    if let Some(dir) = native_library_dir() {
        return format!("{dir}/libcloudflared.so");
    }

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
#[cfg(desktop)]
struct TrayAvailable(Arc<AtomicBool>);

/// Builds the tray icon and its menu. Returns the flag the window-close handler
/// reads to decide between hiding and quitting; false means the tray could not
/// be created and closing the window should exit the app.
#[cfg(desktop)]
fn init_tray(app: &tauri::App) -> Result<Arc<AtomicBool>, Box<dyn std::error::Error>> {
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

    Ok(tray_created)
}

/// Android throws away a process's stderr, so the message from a panicking
/// Rust thread — the only thing that says *why* the app died — never reaches
/// `adb logcat`. tao catches the unwind at the FFI boundary and calls
/// `abort()`, leaving a bare SIGABRT tombstone and no explanation. Installing
/// a hook that writes the panic through liblog puts the message back in
/// logcat, under the "Tunneldrop" tag.
#[cfg(target_os = "android")]
fn install_panic_logger() {
    use std::ffi::CString;
    use std::os::raw::c_char;

    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
    }

    const ANDROID_LOG_ERROR: i32 = 6;

    std::panic::set_hook(Box::new(|info| {
        let text = format!("PANIC: {info}");
        let (Ok(tag), Ok(text)) = (CString::new("Tunneldrop"), CString::new(text)) else {
            return;
        };
        // SAFETY: both pointers are valid NUL-terminated strings that outlive
        // the call, which is all liblog requires.
        unsafe { __android_log_write(ANDROID_LOG_ERROR, tag.as_ptr(), text.as_ptr()) };
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "android")]
    install_panic_logger();

    let port = pick_free_port();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init());

    // Self-update plugins are desktop-only. Guard the registration so a future
    // mobile target still compiles; on desktop this enables the in-app updater
    // and relaunch-after-install.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    builder
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

            // Mobile has no tray: the OS owns the app's lifecycle, so there is
            // nothing to hide to and no Quit item to offer.
            #[cfg(desktop)]
            app.manage(TrayAvailable(init_tray(app)?));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::create_share,
            commands::revoke_share,
            commands::list_shares
        ])
        // Close-to-tray is desktop-only; on mobile the window is never closed by
        // the user and `TrayAvailable` is not in state at all.
        .on_window_event(|_window, _event| {
            #[cfg(desktop)]
            if let tauri::WindowEvent::CloseRequested { api, .. } = _event {
                let window = _window;
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
