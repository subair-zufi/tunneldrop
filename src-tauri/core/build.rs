// tauri-build normally sets the `desktop` and `mobile` cfgs. This crate has no
// Tauri config of its own, so it runs no tauri-build and has to set them here.
fn main() {
    println!("cargo:rustc-check-cfg=cfg(desktop)");
    println!("cargo:rustc-check-cfg=cfg(mobile)");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if matches!(target.as_str(), "windows" | "macos" | "linux") {
        println!("cargo:rustc-cfg=desktop");
    } else {
        println!("cargo:rustc-cfg=mobile");
    }
}
