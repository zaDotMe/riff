# Riff Rust Migration Plan & Architecture

Dokumentasi arsitektur, hasil benchmark, dan implementasi migrasi Riff ke Rust di branch `rust`.

---

## 1. Ringkasan Eksekutif & Keputusan Arsitektur

### Pertanyaan Kunci: Axum vs Slint
- **Axum** adalah HTTP framework / server (pengganti Express 68 endpoint).
- **Slint** adalah GUI framework native (pengganti Electron + Chromium 65k LOC web UI).
- Mengganti Express ke Axum saja hanya menghemat ~50 MB RAM dan ~100 ms startup karena Express **bukan** bottleneck utama (page load Chromium 1.243 ms mendominasi total 1.750 ms).
- Pindah total ke Slint native memang memangkas RAM ke ~60–150 MB dan startup ke ~300 ms, tetapi menuntut rewrite 65k LOC UI, merusak kompatibilitas plugin JS eksternal, web visualizer (custom shader Slint masih belum stabil), dan menuntut lisensi GPL-3.0 (proyek Riff berlisensi MIT).

### Rekomendasi Terpilih (Strangler Pattern)
1. **Fase 1 (Selesai):** Rust core sidecar (`riff-core`) berbasis Axum + SQLite WAL + Lofty + Tokio. Electron tetap menjadi shell GUI, tetapi server lokal dipindahkan ke binary Rust mandiri.
2. **Fase 2 (Target Berikutnya):** Migrasi shell desktop dari Electron ke **Tauri 2 (WebView2)**. UI HTML/CSS/JS 65k LOC dipakai ulang tanpa rewrite, installer turun dari 180 MB ke ~15 MB, dan CVE Electron hilang.
3. **Stream Resolution:** Tetap menggunakan binary `yt-dlp` (Unlicense) via child process async terisolasi. Alternatif pure-Rust seperti `rusty_ytdl` masih stale dan `rustypipe` menuntut lisensi GPL-3.0 + PO token botguard.

---

## 2. Benchmark Origin (Electron + Node) vs Tauri 2 Lean Stack (Fork Ini)

| Metrik | Origin (Node.js + Electron) | Tauri 2 Lean Stack (Fork Ini) | Peningkatan / Catatan |
|---|---|---|---|
| **Binary Size (Release Exe)** | Bundled Electron + Node runtime (~150+ MB) | **10.32 MB** (`riff-tauri.exe` stripped) | **-93% ukuran file binary** |
| **Backend Core RAM (Private)** | ~66 MB s/d 85 MB | **~9.8 MB - 10.1 MB** (`riff-tauri.exe`) | **-85% Private Memory** |
| **Backend Core RAM (Working Set)** | ~110 MB | **~34 MB - 37 MB** (Physical RAM) | **-68% Working Set** |
| **Frontend RAM (WebView2)** | Bundled Chromium (~500 - 800 MB) | **~110 - 320 MB aktif / ~20 - 35 MB idle** | Dioptimalkan via V8 160MB heap + K32EmptyWorkingSet trimmer |
| **Backend In-Process Startup** | ~1.100 ms (Node boot + Express) | **< 6 ms** (Axum Tokio in-process) | **Instan (< 10 ms)** |
| **Search Latency (YT Music)** | ~800 - 1.500 ms | **~380 - 450 ms** (`WEB_REMIX` native Rust) | Metadata artis & judul bersih |
| **Stream-Info Latency** | ~1.500 - 3.500 ms | **~750 - 1.100 ms** (Smart Mirror HLS) | Seekable HLS / unthrottled proxy |
| **Database / Storage** | `metadata.json` (writeFileSync full array) | SQLite WAL mode + index B-tree | Zero UI freeze saat save library |
| **Audio Tag Parser** | ID3 manual parser JS (di main thread) | **Lofty 0.25 (Rust native)** | Zero UI blocking |

---

## 3. Komponen `riff-core` (`rust/riff-core`)

- **`src/main.rs`**: Entry point Axum router, token authentication middleware (`x-riff-token` matching timing-safe), dan endpoint JSON.
- **`src/db.rs`**: SQLite database layer dengan WAL mode, auto migration tabel `tracks`, dan index pencarian.
- **`src/library.rs`**: Local folder scanner dengan Lofty metadata extraction (ID3v2, Vorbis, FLAC, MP4 tags).
- **`src/stream.rs`**: Range streaming untuk file lokal dan audio proxy untuk URL eksternal, plus yt-dlp executor dengan flags `CREATE_NO_WINDOW`.
- **`src/config.rs`**: CLI parser (clap) & environment variables (`RIFF_PORT`, `RIFF_DATA_DIR`, `RIFF_TOKEN`).

---

## 4. Bridge di Electron (`src/main/server-manager.js`)

Electron kini mendukung dual-mode:
- **Default:** Menggunakan Node server asli untuk backward compatibility.
- **Rust Mode:** Aktif jika environment variable `RIFF_BACKEND=rust` atau `RIFF_USE_RUST=1` diset, atau binary `target/release/riff-core.exe` ditemukan saat dipanggil. Jika Rust binary gagal, otomatis fallback ke Node server tanpa crash.

Cara menjalankan dengan Rust backend:
```powershell
$env:RIFF_BACKEND = "rust"
npm start
```

---

## 5. Status Implementasi Tauri 2 & Lean Stack Migration (Single Binary)

Arsitektur Riff telah dimigrasikan ke **Lean Stack** (Single Binary In-Process Axum Core):
1. **Single Binary Executable (`10.32 MB`):**
   - Mengubah `rust/riff-core` menjadi dual-target crate (`lib.rs` + `main.rs`).
   - `src-tauri` mengimpor `riff-core` langsung sebagai library crate (`riff-core = { path = "../rust/riff-core" }`).
   - Server Axum dijalankan secara **in-process** di dalam runtime Tokio Tauri (`tauri::async_runtime::spawn`), mengeliminasi overhead subprocess, IPC pipe, port hunting, dan Windows Job Objects.
   - Ukuran release binary adalah **10.32 MB** (single standalone binary) dengan penggunaan Private Memory backend hanya **~9.8 - 10.1 MB**.
2. **Compiler & Memory Release Profile Optimization:**
   - Profil rilis root `Cargo.toml`: `opt-level = "s"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, dan `strip = true`.
   - SQLite PRAGMA tuning: `cache_size = -2000` (~2 MB), `mmap_size = 16777216` (16 MB), `temp_store = MEMORY`.
3. **`src/renderer/tauri-bridge.js` (Strangler Bridge):**
   - Otomatis mendeteksi environment Tauri dan memetakan `window.electronAPI` ke Tauri commands sehingga UI web (65k LOC) berjalan tanpa modifikasi masif.
   - Native file/folder dialogs: `pickMedia`, `pickLocalFiles`, `pickLocalFolder`, `importAudioFiles`, `pickDataDir`, `pickDownloadDir`.
   - Native audio metadata extraction di Rust via `lofty 0.25` dan `walkdir 2`.
   - Native window controls (minimize, maximize/restore, close, dan smooth titlebar dragging).
   - System controls: `openExternal`, `openLogsFolder`, `restartApp`, dan synchronized storage batching.
4. **Native YouTube Music & SoundCloud Scraper / Smart Mirror Engine:**
   - Search YouTube Music menggunakan InnerTube client `WEB_REMIX` native Rust, menghasilkan metadata judul lagu, nama artis, dan durasi yang bersih tanpa noise judul video ("Official Video", "MV", dsb).
   - Smart Mirror SoundCloud memetakan track YouTube/YTMusic ke stream HLS unencrypted SoundCloud, secara otomatis menolak stream terenkripsi DRM (`cbc-encrypted-hls` / `ctr-encrypted-hls` dengan Sample-AES/FairPlay) dan menyaring kandidat remix/fan-edit saat user mencari lagu original.
   - Fallback modern via `yt-dlp` menggunakan client unthrottled (`c=VISIONOS`) yang bebas dari pembatasan 1MB Google Video, diproxy secara efisien dengan chunking HTTP 206 Partial Content.
5. **Zero-Copy Streaming Audio Proxy (`GET /api/audio-proxy`):**
   - Mendukung HTTP `206 Partial Content` (Range Requests) langsung ke WebView2/HTML5 `<audio>`.
6. **Benchmark & Testing Tooling:**
   - Jalankan benchmark performa, latency endpoint, dan profil RAM kapan saja dengan:
     ```powershell
     npm run bench
     ```
7. **Chromium / WebView2 Lean Memory Optimization (Target < 350 MB):**
   - **V8 Heap Capping (`--js-flags=--max-old-space-size=160`):** Membatasi alokasi heap JavaScript agar GC V8 berjalan lebih agresif dan tidak menahan objek DOM / audio yang sudah tidak terpakai.
   - **Single Renderer Enforcement (`--renderer-process-limit=1`):** Mencegah Chromium membuat proses renderer berlebih di background.
   - **Low-End Device Mode (`--enable-low-end-device-mode`):** Memangkas ukuran cache decoded image, glyph font Skia, dan buffer backing store canvas visualizer.
   - **Automatic Win32 Working Set Trimmer:** Thread background berkala dan event `tauri::WindowEvent::Focused(false)` memanggil `K32EmptyWorkingSet` pada proses induk dan child WebView2, membebaskan physical memory cache yang idle sehingga konsumsi RAM WebView2 stabil di **~110–320 MB saat aktif** dan **~20–35 MB saat idle/background**.
8. **Cara menjalankan via Tauri 2 Single Binary:**
   ```powershell
   npm run tauri:dev
   ```
   Atau jalankan binary release langsung:
   ```powershell
   .\target\release\riff-tauri.exe
   ```
