# Riff v0.7.6 — Native Rust + Slint & Tauri Release (Windows & Linux)

## Highlights
- **Native Slint Desktop UI (`riff-slint`)** dengan tampilan 1:1 Web UI, glassmorphism blur, dynamic cover accent, dan animasi halus.
- **Multi-Source Lyrics & Word-by-Word Karaoke**:
  - Provider: Auto, LRCLIB, Musixmatch, Kugou LRC, NetEase, dan Custom LRC.
  - Word-by-word karaoke 30 FPS (NetEase YRC + enhanced LRC + estimasi berbobot) dengan mode **Gradient** & **Words**.
  - Status sync & source switcher langsung di panel Now Playing & Fullscreen Karaoke.
- **Native Audio Engine (`riff-core`)**:
  - Playback lokal (MP3, FLAC, WAV, OGG, M4A, AAC) & streaming (YouTube Music, SoundCloud, HLS `.m3u8`, GoogleVideo chunked).
  - 3-Band Equalizer, Speed & Pitch resampling, Reverb, Distortion, Sleep Timer, dan 16-band FFT visualizer.
- **Dual Launcher (`riff-tauri`)**: mendukung mode Native Slint maupun WebView2 Classic UI.

## Assets
- **Windows**:
  - `Riff-Slint-windows-x64.exe` — Standalone Native Slint UI executable (dengan icon Riff).
  - `Riff-Tauri-windows-x64.exe` — Dual launcher (WebView2 + Native Slint).
  - `Riff-windows-x64-portable.zip` — Paket portable lengkap (`riff-slint.exe`, `riff-tauri.exe`, `riff-core.exe`).
  - `Riff-Setup-windows-x64.exe` / `.msi` — Installer Windows (via GitHub Actions).
- **Linux**:
  - `Riff-linux-x86_64.tar.gz` — Paket portable Linux (`riff-slint`, `riff-tauri`, `riff-core`, `riff.desktop`, `riff.png`, `install.sh`).
  - `riff-slint-linux-x86_64` — Standalone binary Linux.
  - `Riff-linux-amd64.deb` / `Riff-linux-x86_64.AppImage` — Paket installer Linux (via GitHub Actions).
