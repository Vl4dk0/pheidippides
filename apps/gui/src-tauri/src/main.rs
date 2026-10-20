#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;

fn main() -> Result<(), String> {
    let platform = phi_core::Store::platform().map_err(|e| e.to_string())?;
    let store = phi_core::Store::new(platform.root.join("gui"), platform.cache_root.join("gui"));
    tauri::Builder::default()
        .manage(bridge::AppState(store))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            bridge::profiles,
            bridge::provision,
            bridge::execute,
            bridge::poll
        ])
        .run(tauri::generate_context!())
        .map_err(|e| e.to_string())
}
