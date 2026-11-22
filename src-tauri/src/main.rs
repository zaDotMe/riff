// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn check_ui_mode() -> String {
    let config = riff_core::config::Config::default();
    if let Ok(db) = riff_core::db::open(&config.db_path) {
        if let Ok(conn) = db.lock() {
            if let Ok(Some(mode)) = riff_core::db::get_kv(&conn, "ui_mode") {
                return mode;
            }
        }
    }
    "web".to_string()
}

fn main() {
    let mode = check_ui_mode();
    if mode == "native" || mode == "slint" {
        if let Err(e) = riff_slint::run() {
            eprintln!("[riff] Slint UI error: {:?}, falling back to Web UI", e);
            riff_tauri_lib::run();
        }
    } else {
        riff_tauri_lib::run();
    }
}
