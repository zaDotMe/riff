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

## 2. Benchmark Origin (Electron + Node) vs Rust Core

| Metrik | Origin (Node.js internal) | Rust Core (`riff-core`) | Peningkatan |
|---|---|---|---|
| Server RSS Memory | ~66 MB | **~25 MB working set / 3 MB private** | **-75% s/d -95%** |
| Database / Library storage | `metadata.json` (writeFileSync full array) | SQLite WAL mode + index B-tree | Tidak ada UI freeze saat save |
| Audio tag parser | ID3 manual parser JS (di main process) | **Lofty 0.25 (Rust native)** | Zero main thread blocking |
| Binary size (Release) | Node runtime bundled | **10.4 MB** (stripped single binary) | Sangat ringkas |
| Stream proxy | Node `http`/`https` | Tokio `ReaderStream` + Axum byte range | Range request efisien |

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

## 5. Roadmap Menuju Tauri 2 (Fase 2)

1. Pasang `@tauri-apps/cli` v2.
2. Scaffold `src-tauri` yang langsung meng-embed `riff-core` sebagai internal module atau sidecar.
3. Sambungkan IPC window management (minimize, maximize, close, custom titlebar) ke Tauri commands.
4. Hubungkan SMTC / Media Key via crate `souvlaki` / `tray-icon`.
