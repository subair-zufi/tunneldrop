#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tunneldrop_core::run_with(tauri::generate_context!(), Default::default());
}
