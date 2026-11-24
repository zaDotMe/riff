<div align="center">

<img src="assets/icon-512x512.png" width="112" alt="Riff icon" />

# Riff — Native Rust Edition

**A fast, native desktop music player for YouTube Music, SoundCloud, and local audio.**
Built with **Rust + Slint**, with a Tauri dual launcher for the legacy Web UI.

[![Release](https://img.shields.io/github/v/release/zaDotMe/riff-Rust?style=flat-square&label=release&labelColor=0d0d10&color=6750A4)](https://github.com/zaDotMe/riff-Rust/releases)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20Android%20(in%20progress)-2ec4b6?style=flat-square&labelColor=0d0d10)](https://github.com/zaDotMe/riff-Rust/releases)
[![License](https://img.shields.io/badge/license-MIT-534dff?style=flat-square&labelColor=0d0d10)](./LICENSE)

[Features](#features) · [Downloads](#downloads) · [Architecture](#architecture) · [Building](#building) · [Legacy Web UI](#legacy-web-ui) · [Credits](#credits)

</div>

---

## Overview

This repository is the **native Rust rewrite** of [Riff](https://github.com/rootscripts/riff).
The original Electron/Web implementation is kept under `src/` for reference and for the
dual-launcher mode, but the primary application is now:

- **`riff-core`** — native audio engine, streaming resolver, SQLite library, lyrics providers.
- **`riff-slint`** — the native Slint desktop UI (1:1 with the Web UI, including motion).
- **`riff-tauri`** — dual launcher that can start either the native Slint UI or the WebView2 UI.

No account, no subscription, no telemetry.

---

## Features

- **Streaming & local playback** — YouTube Music, YouTube, SoundCloud, plus local FLAC/WAV/MP3/OGG/M4A/AAC.
- **Native audio engine** — Symphonia decoding, CPAL output, 3-band EQ, speed/pitch, reverb, distortion, sleep timer, 16-band FFT visualizer.
- **Multi-source lyrics** — Auto, LRCLIB, Musixmatch, Kugou, NetEase, Genius, and custom LRC.
- **Word-by-word karaoke** — NetEase YRC, enhanced LRC, and weighted estimation, rendered at 30 FPS with Gradient and Words styles.
- **Sync tools** — per-track sync offset (±5000 ms), tap-to-sync editor, source picker, sync/source badges.
- **1:1 desktop UI** — glass blur, dynamic cover accent, ambient banner, animated EQ bars, smooth tab/modal transitions.
- **Full settings** — interface, themes, banners, audio, integrations, storage, advanced, plugins, account.
- **Library** — favorites, history, playlists, queue, local folder scanning, playlist import.
- **Dual launcher** — native Slint UI or the original WebView2 UI.

---

## Downloads

Release artifacts are published on the [Releases page](https://github.com/zaDotMe/riff-Rust/releases).

| Platform | Artifact |
| --- | --- |
| Windows (native UI) | `Riff-Slint-windows-x64.exe` |
| Windows (dual launcher) | `Riff-Tauri-windows-x64.exe` |
| Windows (portable) | `Riff-windows-x64-portable.zip` |
| Linux (portable) | `Riff-linux-x86_64.tar.gz` |
| Linux (standalone) | `riff-slint-linux-x86_64` |
| Linux (installer) | `Riff-linux-amd64.deb`, `Riff-linux-x86_64.AppImage` |
| Android | APK — in progress |

> Linux artifacts are produced by the GitHub Actions release workflow.
> Android packaging is the remaining migration milestone.

---

## Architecture

```
riff/
├─ rust/
│  ├─ riff-core/          # Native engine: audio, streaming, SQLite, lyrics
│  └─ riff-slint/         # Slint desktop UI + karaoke renderer
├─ src-tauri/             # Tauri dual launcher + Android scaffolding
├─ packaging/linux/       # .desktop entry and install script
├─ scripts/               # Release packaging helpers
├─ src/                   # Legacy Electron/Web UI (reference + dual mode)
└─ assets/                # Shared icons and artwork
```

### Playback pipeline

1. Resolve the exact track (YouTube ID first, then SoundCloud with strict title/artist/duration matching).
2. Decode with Symphonia and feed CPAL.
3. Report the **audible** position (`decoded position − sink latency`) every 100 ms so lyrics stay in sync.
4. Render lyrics from the best matching provider, with word-level timing when available.

---

## Building

### Prerequisites

- Rust stable (1.80+)
- Windows: MSVC toolchain + WebView2 (for the Tauri launcher)
- Linux: `libasound2-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev`, `libssl-dev`, `pkg-config`, `libgtk-3-dev`, `libwebkit2gtk-4.1-dev`
- Node.js 20+ only if you build the Tauri/Web launcher

### Native Slint app

```bash
cargo build --release -p riff-slint
./target/release/riff-slint
```

### Dual launcher (Tauri)

```bash
npm install
cargo build --release -p riff-tauri
# or: npx tauri build
```

### Tests

```bash
cargo test --release -p riff-core -p riff-slint
```

### Package Windows artifacts

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package-release.ps1
```

Artifacts are written to `dist/release/`.

---

## Legacy Web UI

The original Electron/Web implementation remains available and is still used by the
Tauri launcher's WebView2 mode. It lives in `src/` and is intentionally excluded from
GitHub's language statistics (see `.gitattributes`) so the repository is reported as a
**Rust + Slint** project.

Run it with:

```bash
npm install
npm start
```

---

## Credits

Riff was originally created by [rootscripts](https://github.com/rootscripts/riff).
This fork adds the native Rust + Slint rewrite, multi-source lyrics, native audio engine,
and the cross-platform release pipeline.

## License

MIT — see [LICENSE](./LICENSE).
