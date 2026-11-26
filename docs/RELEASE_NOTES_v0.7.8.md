# Riff v0.7.8 - Space Playback Shortcut + Google TV

Riff v0.7.8 builds on the pure Slint release with a keyboard shortcut and native Android TV / Google TV packaging.

## Playback shortcut

- **Space** pauses and resumes the current track.
- **Space** still advances synchronized lyrics while the **Tap to Sync** editor is open.
- The shortcut is ignored while editing text in a search or text field.
- Google TV remote-friendly **landscape**, fullscreen, no-touchscreen-required layout.

## Google TV / Android TV

- Native Slint + Riff Core **arm64-v8a APK**: `Riff-google-tv-arm64-v8a.apk`.
- Android TV **`LEANBACK_LAUNCHER`** plus regular Android **`LAUNCHER`** entries.
- `android.software.leanback` not required; `android.hardware.touchscreen` not required.
- Minimum Android API **26**, target API **34**.
- Android `INTERNET` permission and fullscreen landscape mode.
- App-private writable data directory for library, history, favorites, and settings.
- Streaming-first Android TV experience; local file picker callbacks gracefully no-op on Android.

## Desktop

- Existing pure Slint Windows NSIS setup, portable, standalone, Linux `.deb`, and portable artifacts remain available.
- **No Tauri packaging**.

## Verification

- Shared Slint sources compile for desktop and Android.
- Desktop `cargo test -p riff-core -p riff-slint`.
- Release CI runs `cargo test --release -p riff-core -p riff-slint`.
- Android TV CI builds an arm64-v8a APK with Android SDK + NDK and `cargo-apk`.
- Production release signing uses `ANDROID_RELEASE_KEYSTORE_BASE64` + `ANDROID_RELEASE_KEYSTORE_PASSWORD` repository secrets; without them, a debug-signed APK is produced.
