<div align="center">

<img src="assets/icon-512x512.png" width="112" alt="Riff icon" />

# Riff — Native Rust & Slint

**Expressive, fast, native music player for YouTube Music, SoundCloud, and local audio.**
Built purely with **Rust + Slint** for **Windows**, **Linux**, and **Google TV / Android TV**.

<a href="https://github.com/zaDotMe/riff-Rust/releases"><img src="assets/badges/release.svg" alt="Release v0.7.81" /></a>
<a href="https://github.com/zaDotMe/riff-Rust/releases"><img src="assets/badges/platforms.svg" alt="Platforms: Windows, Linux, Google TV" /></a>
<a href="https://slint.dev"><img src="assets/badges/ui.svg" alt="UI: Pure Slint Expressive" /></a>
<a href="./LICENSE"><img src="assets/badges/license.svg" alt="License: MIT" /></a>

<br />

[Expressive UI](#-expressive-ui) · [Features](#-features) · [Shortcuts & TV Remote](#-shortcuts--google-tv-remote) · [Downloads](#-downloads) · [Architecture](#-architecture) · [Building](#-building)

</div>

---

## ✨ Expressive UI

Riff combines a **native Rust audio engine** with an **expressive Material-inspired Slint interface** — designed to feel fluid on both desktop and 10-foot Google TV screens:

- **Dynamic Cover Accent & Ambient Blur** — panel surfaces, active states, and backdrops adapt smoothly to the current album art palette.
- **TV / Fullscreen Karaoke Stage** — ambient animated blobs, floating artwork, audio output badge, and 30 FPS word-by-word karaoke with configurable easing, speed, and **adjustable active-line position (`Top` / `Center` / `Bottom`)**.
- **Shimmer Loading Skeletons** — responsive animated skeletons while fetching search results, playlists, artists, or recommendations.
- **Collapsible Sidebar & Compact Mini-Player** — expand for full library & playlists, collapse to a clean icon rail, or switch to a 420×520 mini-player.
- **16-Band Live FFT Visualizer** — real-time spectrum bars integrated cleanly into the playbar.

---

## 🎵 Features

| Area | Capabilities |
| --- | --- |
| **Streaming & Local** | YouTube Music (`WEB_REMIX` / `VISIONOS` / `ANDROID_MUSIC` / `IOS` + `yt-dlp` & Smart Mirror fallbacks), YouTube, SoundCloud, and local `FLAC` / `WAV` / `MP3` / `OGG` / `M4A` / `AAC` |
| **Discovery & Search** | Unified search across **All / Songs / Playlists / Artists**, shimmer loading skeleton, retry on error, **Artist Profile** (Deezer avatar/fans + top tracks), and **Save to Library** for remote playlists |
| **Autoplay & Queue** | YouTube Music radio mix (`RDAMVM`) + Smart recommendations with **Similar Vibe**, **Artist Mix**, and **Discovery** modes, history deduplication, per-track **Add to Queue**, and **YouTube Music-style Repeat** (`Off` → `Repeat All` → `Repeat One`) |
| **Search & Play History** | Persistent search history for queries, **played tracks**, and **opened artists** with instant quick-play, plus personalized **For You** & **Jump Back In** shelves on Home |
| **Native Audio Engine** | Symphonia decoding + CPAL output, 3-band EQ + presets, speed/pitch, reverb, distortion, sleep timer |
| **Multi-Source Lyrics** | Auto, LRCLIB, Musixmatch, Kugou, NetEase (YRC), Genius, and custom LRC with **Tap-to-Sync**, ±5000 ms offset, and centered/adjustable auto-scroll |
| **Library Management** | Favorites sync across every view, custom playlists (create, add, remove track, delete), local folder scanning & track removal |

No account, no subscription, no telemetry.

---

## ⌨️ Shortcuts & Google TV Remote

| Key / Remote | Action |
| --- | --- |
| `Space` / `Pause` | Play / Pause (or mark next line when **Tap to Sync** is open) |
| `Stop` | Pause playback |
| `/` or `Ctrl + F` | Focus search bar & open search history |
| `←` / `→` | Seek −5s / +5s |
| `↑` / `↓` | Volume +5% / −5% |
| `M` | Mute / Unmute |
| `Esc` / `Back` | Close active modal/overlay or navigate back |
| `F11` | Toggle Fullscreen Karaoke Lyrics |

---

## 📦 Downloads

Release artifacts are published on the [Releases page](https://github.com/zaDotMe/riff-Rust/releases).

| Platform | Artifact |
| --- | --- |
| **Windows** (Installer) | `Riff-Setup-windows-x64.exe` |
| **Windows** (Standalone) | `Riff-Slint-windows-x64.exe` |
| **Windows** (Portable) | `Riff-windows-x64-portable.zip` |
| **Linux** (Portable) | `Riff-linux-x86_64.tar.gz` |
| **Linux** (Standalone) | `riff-slint-linux-x86_64` |
| **Linux** (Debian/Ubuntu) | `Riff-linux-amd64.deb` |
| **Google TV / Android TV** (`arm64-v8a`) | `Riff-google-tv-arm64-v8a.apk` |

---

## 🧱 Architecture

```text
riff-rust/
├─ rust/
│  ├─ riff-core/          # Native engine: Symphonia/CPAL audio, YouTube/SoundCloud/local resolver, SQLite, lyrics
│  └─ riff-slint/         # Pure Slint UI (desktop + Google TV android_main), karaoke renderer, state & shortcuts
├─ packaging/
│  ├─ windows/            # NSIS installer script
│  └─ linux/              # .desktop entry and install script
├─ scripts/               # Release packaging helpers
├─ docs/                  # Release notes
└─ assets/                # Shared icons and artwork
```

---

## 🛠️ Building

### Prerequisites

- **Rust stable** (1.80+)
- **Windows**: MSVC or `clang-cl` + Windows SDK
- **Linux**: `libasound2-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libfontconfig1-dev`, `libssl-dev`, `pkg-config`
- **Google TV / Android TV**: Android NDK + `cargo-apk` (`cargo apk build --lib`)

### Build & Run

```bash
cargo build --release -p riff-slint
./target/release/riff-slint
```

### Tests

```bash
cargo test -p riff-core -p riff-slint --lib
```

### Package Windows Artifacts

```powershell
powershell -ExecutionPolicy Bypass -File scripts/package-release.ps1
```

---

## 🙏 Credits

Riff was originally created by [rootscripts](https://github.com/rootscripts/riff).
This repository is the **Pure Rust + Slint** rewrite with a native audio engine, expressive desktop & Google TV UI, multi-source word-by-word lyrics, autoplay recommendations, and cross-platform CI/CD releases.

## 📄 License

MIT — see [LICENSE](./LICENSE).
