use std::io::BufRead;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Mutex;
use lofty::file::TaggedFileExt;
use tauri::Window;
use tauri_plugin_dialog::DialogExt;

#[cfg(target_os = "windows")]
use std::os::windows::io::AsRawHandle;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x08000000;
const DEFAULT_PORT: u16 = 38472;
const DEFAULT_TOKEN: &str = "riff-tauri-core";

static SERVER_PORT: AtomicU16 = AtomicU16::new(0);
static CHILD_PROCESS: Mutex<Option<std::process::Child>> = Mutex::new(None);

#[cfg(target_os = "windows")]
fn assign_to_kill_on_close_job(child: &std::process::Child) {
    use windows_sys::Win32::System::JobObjects::*;

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job != 0 {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            AssignProcessToJobObject(job, child.as_raw_handle() as _);
        }
    }
}

fn kill_backend() {
    if let Ok(mut lock) = CHILD_PROCESS.lock() {
        if let Some(mut child) = lock.take() {
            println!("[tauri] Terminating backend child process (pid: {})...", child.id());
            let _ = child.kill();
        }
    }
}

fn find_server_script() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("src/server/server.js"),
        PathBuf::from("../src/server/server.js"),
        PathBuf::from("../../src/server/server.js"),
    ];

    for c in &candidates {
        if c.exists() {
            if let Ok(abs) = std::fs::canonicalize(c) {
                return Some(abs);
            }
            return Some(c.clone());
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.as_path();
        for _ in 0..5 {
            if let Some(parent) = cur.parent() {
                let candidate = parent.join("src").join("server").join("server.js");
                if candidate.exists() {
                    return Some(candidate);
                }
                cur = parent;
            }
        }
    }

    None
}

fn create_node_cmd() -> std::process::Command {
    let script_path = match find_server_script() {
        Some(p) => p,
        None => {
            eprintln!("[tauri] Warning: Could not locate src/server/server.js. Assuming default port {}", DEFAULT_PORT);
            SERVER_PORT.store(DEFAULT_PORT, Ordering::SeqCst);
            PathBuf::from("src/server/server.js")
        }
    };

    println!("[tauri] Spawning Node background server from: {}", script_path.display());

    let mut cmd = std::process::Command::new("node");
    cmd.arg(&script_path);
    cmd.env("RIFF_TOKEN", DEFAULT_TOKEN);

    if let Some(parent) = script_path.parent().and_then(|p| p.parent()) {
        cmd.current_dir(parent);
    }
    cmd
}

fn find_rust_core_bin() -> Option<PathBuf> {
    let bin_name = if cfg!(windows) { "riff-core.exe" } else { "riff-core" };

    #[cfg(debug_assertions)]
    let candidates = [
        PathBuf::from("target").join("debug").join(bin_name),
        PathBuf::from("target").join("release").join(bin_name),
        PathBuf::from("bin").join(bin_name),
    ];

    #[cfg(not(debug_assertions))]
    let candidates = [
        PathBuf::from("target").join("release").join(bin_name),
        PathBuf::from("target").join("debug").join(bin_name),
        PathBuf::from("bin").join(bin_name),
    ];

    for c in &candidates {
        if c.exists() {
            if let Ok(abs) = std::fs::canonicalize(c) {
                return Some(abs);
            }
            return Some(c.clone());
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join(bin_name);
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn start_backend_server() {
    let explicit_node = std::env::var("RIFF_BACKEND")
        .map(|v| v.to_lowercase() == "node")
        .unwrap_or(false);
    let rust_bin_opt = find_rust_core_bin();
    let prefer_rust = !explicit_node && rust_bin_opt.is_some();

    let mut cmd = if prefer_rust {
        if let Some(rust_bin) = rust_bin_opt {
            println!("[tauri] Spawning native Rust core backend: {}", rust_bin.display());
            let mut c = std::process::Command::new(&rust_bin);
            c.arg("--port").arg(DEFAULT_PORT.to_string());
            c.arg("--token").arg(DEFAULT_TOKEN);
            if let Some(proj_dir) = find_server_script().and_then(|p| p.parent().and_then(|p| p.parent().map(|p| p.to_path_buf()))) {
                c.current_dir(proj_dir);
            }
            c
        } else {
            eprintln!("[tauri] Warning: riff-core binary was not found. Falling back to Node server.");
            create_node_cmd()
        }
    } else {
        create_node_cmd()
    };

    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    match cmd.spawn() {
        Ok(mut child) => {
            #[cfg(target_os = "windows")]
            assign_to_kill_on_close_job(&child);

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            if let Ok(mut lock) = CHILD_PROCESS.lock() {
                *lock = Some(child);
            }

            // Monitor stdout for readiness event
            if let Some(stdout) = stdout {
                std::thread::spawn(move || {
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines().flatten() {
                        if line.contains("\"event\":\"ready\"") || line.contains("\"event\": \"ready\"") {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                                if let Some(p) = val.get("port").and_then(|v| v.as_u64()) {
                                    SERVER_PORT.store(p as u16, Ordering::SeqCst);
                                    println!("[tauri] Backend server ready on port {}", p);
                                }
                            }
                        } else if let Some(idx) = line.find("http://127.0.0.1:") {
                            let rest = &line[idx + "http://127.0.0.1:".len()..];
                            if let Some(port_str) = rest.split(|c: char| !c.is_numeric()).next() {
                                if let Ok(p) = port_str.parse::<u16>() {
                                    SERVER_PORT.store(p, Ordering::SeqCst);
                                    println!("[tauri] Backend server ready on port {}", p);
                                }
                            }
                        } else if let Some(idx) = line.find("listening on ") {
                            let rest = &line[idx + "listening on ".len()..];
                            if let Some(port_str) = rest.split(|c: char| !c.is_numeric()).next() {
                                if let Ok(p) = port_str.parse::<u16>() {
                                    SERVER_PORT.store(p, Ordering::SeqCst);
                                    println!("[tauri] Backend server listening on port {}", p);
                                }
                            }
                        }
                    }
                });
            }

            // Monitor stderr for errors in debug mode
            if let Some(stderr) = stderr {
                std::thread::spawn(move || {
                    let reader = std::io::BufReader::new(stderr);
                    for _line in reader.lines().flatten() {
                        #[cfg(debug_assertions)]
                        eprintln!("[server-stderr] {}", _line);
                    }
                });
            }
        }
        Err(err) => {
            eprintln!("[tauri] Failed to spawn Node backend server: {}. Falling back to default port {}", err, DEFAULT_PORT);
            SERVER_PORT.store(DEFAULT_PORT, Ordering::SeqCst);
        }
    }
}

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
    kill_backend();
    let _ = window.close();
}

#[tauri::command]
async fn get_server_port() -> u16 {
    for _ in 0..100 {
        let port = SERVER_PORT.load(Ordering::SeqCst);
        if port != 0 {
            return port;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    DEFAULT_PORT
}

#[tauri::command]
fn get_server_token() -> String {
    DEFAULT_TOKEN.to_string()
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn log_error(
    level: String,
    message: String,
    stack: Option<String>,
    filename: Option<String>,
    line: Option<u32>,
) {
    eprintln!(
        "[webview-{}] {} (at {:?}:{:?}) (stack: {:?})",
        level, message, filename, line, stack
    );
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

#[tauri::command]
fn get_storage_all() -> serde_json::Value {
    let candidates = [
        PathBuf::from("src/storage.json"),
        PathBuf::from("../src/storage.json"),
    ];

    for c in &candidates {
        if c.exists() {
            if let Ok(content) = std::fs::read_to_string(c) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    return json;
                }
            }
        }
    }

    serde_json::json!({})
}

#[tauri::command]
fn start_drag(window: Window) {
    let _ = window.start_dragging();
}

fn read_audio_metadata(path: &std::path::Path) -> serde_json::Value {
    let file_name = path.file_name().and_then(|v| v.to_str()).unwrap_or("track");
    let base = path.file_stem().and_then(|v| v.to_str()).unwrap_or(file_name);

    let (mut title, mut artist) = if let Some((a, t)) = base.split_once(" - ") {
        (t.trim().to_string(), a.trim().to_string())
    } else {
        (base.trim().to_string(), "Unknown Artist".to_string())
    };

    let mut album = String::new();
    let mut duration = 0.0;

    if let Ok(tagged) = lofty::read_from_path(path) {
        use lofty::file::AudioFile;
        use lofty::tag::Accessor;
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(t) = tag.title() {
                let s = t.trim();
                if !s.is_empty() {
                    title = s.to_string();
                }
            }
            if let Some(a) = tag.artist() {
                let s = a.trim();
                if !s.is_empty() {
                    artist = s.to_string();
                }
            }
            if let Some(al) = tag.album() {
                album = al.trim().to_string();
            }
        }
        duration = tagged.properties().duration().as_secs_f64();
    }

    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let id = format!("local_{:012x}", hash & 0x0000_ffff_ffff_ffff);
    let path_str = path.to_string_lossy().to_string();
    let file_url = format!("file:///{}", path_str.replace('\\', "/"));

    serde_json::json!({
        "id": id,
        "title": title,
        "artist": artist,
        "album": album,
        "duration": duration.round() as u64,
        "path": path_str,
        "url": file_url,
        "platform": "local"
    })
}

#[tauri::command]
async fn pick_media(app: tauri::AppHandle) -> Option<String> {
    let file = app.dialog().file()
        .add_filter("Media", &["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm"])
        .blocking_pick_file();
    file.and_then(|fp| fp.into_path().ok()).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn pick_local_files(app: tauri::AppHandle) -> Vec<serde_json::Value> {
    let files = app.dialog().file()
        .add_filter("Audio", &["mp3", "flac", "wav", "ogg", "oga", "m4a", "aac", "webm", "weba", "opus", "wma"])
        .blocking_pick_files();

    let mut result = Vec::new();
    if let Some(files) = files {
        for file in files {
            if let Ok(path) = file.into_path() {
                if path.is_file() {
                    result.push(read_audio_metadata(&path));
                }
            }
        }
    }
    result
}

#[tauri::command]
async fn pick_local_folder(app: tauri::AppHandle) -> Vec<serde_json::Value> {
    let folder = app.dialog().file().blocking_pick_folder();
    let mut result = Vec::new();
    let audio_exts = ["mp3", "flac", "wav", "ogg", "oga", "m4a", "aac", "webm", "weba", "opus", "wma"];

    if let Some(folder) = folder {
        if let Ok(dir) = folder.into_path() {
            for entry in walkdir::WalkDir::new(&dir).follow_links(false).into_iter().flatten() {
                if entry.file_type().is_file() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                    if audio_exts.contains(&ext.as_str()) {
                        result.push(read_audio_metadata(path));
                    }
                }
            }
        }
    }
    result
}

#[tauri::command]
async fn import_audio_files(app: tauri::AppHandle) -> Vec<serde_json::Value> {
    let files = app.dialog().file()
        .add_filter("Audio", &["mp3", "flac", "wav", "ogg", "oga", "m4a", "aac", "webm", "weba", "opus"])
        .blocking_pick_files();

    let mut out = Vec::new();
    let library_dir = std::path::PathBuf::from(get_data_dir()).join("library");
    let _ = std::fs::create_dir_all(&library_dir);

    if let Some(files) = files {
        for file in files {
            let path = match file.into_path() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if !path.is_file() { continue; }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let base_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("track").to_string();

            let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in format!("{base_name}{file_size}").as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            let id = format!("local_{:012x}", hash & 0x0000_ffff_ffff_ffff);
            let target_name = format!("{id}.{ext}");
            let target_path = library_dir.join(&target_name);

            if !target_path.exists() {
                let _ = std::fs::copy(&path, &target_path);
            }

            out.push(serde_json::json!({
                "id": id,
                "file": target_name,
                "ext": ext,
                "name": base_name
            }));
        }
    }
    out
}

#[tauri::command]
async fn pick_data_dir(app: tauri::AppHandle) -> Option<String> {
    let folder = app.dialog().file().blocking_pick_folder();
    folder.and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn get_download_dir() -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join("Downloads").to_string_lossy().to_string()
}

#[tauri::command]
async fn pick_download_dir(app: tauri::AppHandle) -> Option<String> {
    let folder = app.dialog().file().blocking_pick_folder();
    folder.and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn open_logs_folder() {
    let data_dir = get_data_dir();
    let logs_dir = std::path::PathBuf::from(data_dir).join("logs");
    let _ = std::fs::create_dir_all(&logs_dir);
    let _ = open::that(&logs_dir);
}

#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    kill_backend();
    app.restart();
}

#[tauri::command]
fn save_storage_batch(data: serde_json::Value) {
    let path = PathBuf::from("src/storage.json");
    if let Ok(str_val) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(path, str_val);
    }
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
            start_drag,
            get_server_port,
            get_server_token,
            get_app_version,
            open_external,
            get_data_dir,
            get_storage_all,
            save_storage_batch,
            log_error,
            pick_media,
            pick_local_files,
            pick_local_folder,
            import_audio_files,
            pick_data_dir,
            get_download_dir,
            pick_download_dir,
            open_logs_folder,
            restart_app,
        ])
        .setup(|_app| {
            start_backend_server();
            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                kill_backend();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
