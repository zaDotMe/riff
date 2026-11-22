// Prevents console window in release mode on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), slint::PlatformError> {
    riff_slint::run()
}
