# Riff v0.7.6 — Native Rust + Slint & Tauri Release (Windows & Linux)

## Highlights

- **Native Slint desktop UI (`riff-slint`)** with a 1:1 match for the Web UI, glassmorphism blur, dynamic cover accent, ambient banner, and smooth entrance motion.
- **Multi-source lyrics & word-by-word karaoke**:
  - Providers: Auto, LRCLIB, Musixmatch, Kugou LRC, NetEase, Genius, and custom LRC.
  - Word-by-word karaoke at 30 FPS (NetEase YRC + enhanced LRC + weighted estimation) with **Gradient** and **Words** styles.
  - Sync/source badges and a source picker directly in the Now Playing panel and Fullscreen Karaoke.
  - Per-track sync offset (±5000 ms), tap-to-sync editor with Play/Pause, and lyrics animation easing.
- **Native audio engine (`riff-core`)**:
  - Local playback (MP3, FLAC, WAV, OGG, M4A, AAC) and streaming (YouTube Music, SoundCloud, HLS `.m3u8`, GoogleVideo chunked).
  - 3-band equalizer, speed & pitch resampling, reverb, distortion, sleep timer, and a 16-band FFT visualizer.
  - Audible-position reporting (decoded position minus sink latency) every 100 ms for accurate lyric sync.
- **Window chrome**:
  - Auto-hide titlebar reveal when the pointer reaches the top edge.
  - Staggered fade/slide entrance for the window controls, with hover highlight, icon color, and border-radius motion.
- **Dual launcher (`riff-tauri`)**: start either the native Slint UI or the classic WebView2 UI.

## Assets

### Windows

- `Riff-Slint-windows-x64.exe` — standalone native Slint UI executable (with the Riff icon).
- `Riff-Tauri-windows-x64.exe` — dual launcher (WebView2 + native Slint).
- `Riff-windows-x64-portable.zip` — complete portable package (`riff-slint.exe`, `riff-tauri.exe`, `riff-core.exe`).
- `Riff-Setup-windows-x64.exe` / `.msi` — Windows installers (via GitHub Actions).

### Linux

- `Riff-linux-x86_64.tar.gz` — portable Linux package (`riff-slint`, `riff-tauri`, `riff-core`, `riff.desktop`, `riff.png`, `install.sh`).
- `riff-slint-linux-x86_64` — standalone Linux binary.
- `Riff-linux-amd64.deb` / `Riff-linux-x86_64.AppImage` — Linux installers (via GitHub Actions).

### Android

- APK packaging is the remaining migration milestone.

## Verification

- `cargo test --release -p riff-core -p riff-slint` — all tests pass.
- Windows binaries built locally with `cargo build --release` and packaged with `scripts/package-release.ps1`.
- Linux artifacts are produced by `.github/workflows/release.yml`.
