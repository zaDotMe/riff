# Riff v0.7.7 - Pure Slint Release (Windows & Linux)

Riff v0.7.7 packages the **native Slint application** as the primary desktop experience. The Windows installer no longer installs Tauri or WebView2 mode, and release artifacts contain only `riff-slint` + `riff-core`.

## Core playback & library fixes

- Fix streaming HTTP 403 with robust YouTube Music resolution (`VISIONOS`, cached `visitorData`, stream verification), correct GoogleVideo user agents and `Origin`/`Referer` headers, HLS support, SoundCloud fallback, and 1MB chunking.
- Surface decoder/open errors as `PlaybackError` instead of false "Playing" states.
- Correct `ffmpeg -version`, `yt-dlp --version`, Windows `CREATE_NO_WINDOW`, and tool detection.
- Serve local tracks using canonical absolute `track.path`, supporting subfolders and external scan directories.
- Support batch library removal (`ids`, `id`, `clearAll`) and prevent invalid-path HTTP 500s.
- Return individual songs in `/api/hot`, `/api/trending`, `/api/top-chart`, filtering compilations and tracks longer than 15 minutes.
- Preserve playback error handling and verify real live-stream end-to-end decode.

## Slint UI & UX

- Original-Riff-style layout with collapsible left sidebar (expanded 320px / compact 80px rail), compact tiles, persisted collapse state, bottom actions, and correct playbar bounds.
- Persistent search history dropdown, recent searches, clear/remove, history navigation, always-visible YouTube / YouTube Music / SoundCloud switcher, and responsive compact platform pills.
- Sidebar `+ Create` popover, inline library search, sort, local scan/add, My files filtering/sorting/shuffle, and Liked Songs filtering/sorting/shuffle.
- Full original-style track lists, cover hydration, favorite synchronization across every view, thumbnail preservation, correct platform inference, and SoundCloud search.
- Navigation back/forward, playlist sync, queue controls, search state, and silent-decode notifications.
- Real platform icons and Riff icon assets.

## Packaging

- **Windows installer:** `Riff-Setup-windows-x64.exe` (NSIS) — installs `riff-slint.exe`, `riff-core.exe`, start menu/desktop shortcuts, uninstaller; **no Tauri**.
- **Windows portable:** `Riff-windows-x64-portable.zip`.
- **Windows standalone:** `Riff-Slint-windows-x64.exe`.
- **Linux portable:** `Riff-linux-x86_64.tar.gz`.
- **Linux standalone:** `riff-slint-linux-x86_64`.
- **Linux installer:** `Riff-linux-amd64.deb`.
- Local packaging: `scripts/package-release.ps1`.

## Verification

- `cargo test -p riff-core -p riff-slint` passes all unit/integration tests.
- `cargo test --release -p riff-core -p riff-slint` used in CI.
- Live live-stream resolution + decode test passes.
- End-to-end checks verified tools status, streaming HTTP 206 audio chunks, scanned local subfolder playback, batch removal, feeds, and native Slint launch.
