use std::sync::atomic::{AtomicU16, Ordering};
use tauri::Window;

static SERVER_PORT: AtomicU16 = AtomicU16::new(0);

#[tauri::command]
fn minimize_window(window: Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn maximize_window(window: Window) {
    if let Ok(is_maximized) = window.is_maximized() {
        if is_maximized {
            let _ = window.unmaximize();
        } else {
            let _ = window.maximize();
        }
    }
}

#[tauri::command]
fn close_window(window: Window) {
    let _ = window.close();
}

#[tauri::command]
fn get_server_port() -> u16 {
    SERVER_PORT.load(Ordering::SeqCst)
}

#[tauri::command]
fn get_server_token() -> String {
    // Shared token between Tauri frontend and backend
    "riff-tauri-core".to_string()
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn open_external(url: String) {
    let _ = open::that(&url);
}

#[tauri::command]
fn get_data_dir() -> String {
    let base = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(base)
        .join("Riff")
        .to_string_lossy()
        .to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            minimize_window,
            maximize_window,
            close_window,
            get_server_port,
            get_server_token,
            get_app_version,
            open_external,
            get_data_dir,
        ])
        .setup(|app| {
            // Spawn or connect to riff-core background server
            let _handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Background core readiness placeholder
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
