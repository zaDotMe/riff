use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tracing::info;

use crate::config::Config;
use crate::db::{self, Db};
use crate::events::{EventBus, RiffEvent};
use crate::library::{self, ScanResult};
use crate::lyrics::{LyricsQuery, LyricsService};
use crate::models::{LibraryAddItem, StreamQuery, Track};
use crate::soundcloud::SoundCloudClient;
use crate::youtube::YouTubeClient;

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

pub struct RiffApi {
    pub db: Db,
    pub config: Arc<Config>,
    pub soundcloud: Arc<SoundCloudClient>,
    pub youtube: Arc<YouTubeClient>,
    pub lyrics: Arc<LyricsService>,
    pub events: Arc<EventBus>,
    pub player: Arc<crate::player::AudioPlayer>,
}

impl RiffApi {
    pub fn new(
        db: Db,
        config: Arc<Config>,
        soundcloud: Arc<SoundCloudClient>,
        youtube: Arc<YouTubeClient>,
        lyrics: Arc<LyricsService>,
        events: Arc<EventBus>,
        player: Arc<crate::player::AudioPlayer>,
    ) -> Self {
        Self {
            db,
            config,
            soundcloud,
            youtube,
            lyrics,
            events,
            player,
        }
    }

    pub fn play_file(&self, path: std::path::PathBuf, track: Option<Track>) {
        self.player.play_file(path, track);
    }

    pub fn play_bytes(&self, data: Vec<u8>, hint: Option<String>, track: Option<Track>) {
        self.player.play_bytes(data, hint, track);
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.resume();
    }

    pub fn seek(&self, pos_sec: f64) {
        self.player.seek(pos_sec);
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn set_volume(&self, vol: f32) {
        self.player.set_volume(vol);
    }

    pub fn set_eq(&self, bass: f32, mid: f32, treble: f32) {
        self.player.set_eq(bass, mid, treble);
    }

    pub fn set_speed(&self, speed: f32) {
        self.player.set_speed(speed);
    }

    pub fn set_effects(&self, reverb: f32, distortion: f32) {
        self.player.set_effects(reverb, distortion);
    }

    pub fn get_playback_state(&self) -> (bool, f64, f64, f32) {
        (
            self.player.is_playing(),
            self.player.get_position(),
            self.player.get_duration(),
            self.player.get_volume(),
        )
    }

    /// Unified search across YouTube, YouTube Music, SoundCloud, and local SQLite library
    pub async fn search(
        &self,
        query: &str,
        platform: &str,
        limit: usize,
        offset: usize,
    ) -> Result<(Vec<Value>, bool)> {
        let q = query.trim();
        if q.is_empty() {
            return Ok((Vec::new(), false));
        }

        // 1. Direct SoundCloud search if requested
        if platform == "soundcloud" {
            if let Ok(tracks) = self.soundcloud.search(q, limit).await {
                let has_more = tracks.len() >= limit;
                return Ok((tracks, has_more));
            }
        }

        // 2. Query YouTube via native InnerTube API if requested
        if platform == "ytmusic" || platform == "rifflibrary" {
            if let Ok(tracks) = self.youtube.search_music(q, limit).await {
                if !tracks.is_empty() {
                    let has_more = tracks.len() >= limit;
                    let values: Vec<Value> = tracks
                        .into_iter()
                        .map(|t| serde_json::to_value(t).unwrap_or_default())
                        .collect();
                    return Ok((values, has_more));
                }
            }
        }

        if platform == "ytmusic" || platform == "youtube" || platform == "rifflibrary" {
            if let Ok(tracks) = self.youtube.search(q, limit).await {
                if !tracks.is_empty() {
                    let has_more = tracks.len() >= limit;
                    let values: Vec<Value> = tracks
                        .into_iter()
                        .map(|t| serde_json::to_value(t).unwrap_or_default())
                        .collect();
                    return Ok((values, has_more));
                }
            }
        }

        // 3. Query local SQLite B-Tree database
        let db = self.db.clone();
        let q_owned = q.to_string();
        let local_tracks = tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::search_tracks(&conn, &q_owned, limit + 1, offset)
        })
        .await
        .context("spawn_blocking failed")??;

        if !local_tracks.is_empty() || platform == "local" {
            let has_more = local_tracks.len() > limit;
            let sliced: Vec<Value> = local_tracks
                .into_iter()
                .take(limit)
                .map(|t| {
                    json!({
                        "id": t.id,
                        "title": t.title,
                        "artist": t.artist,
                        "album": t.album.unwrap_or_default(),
                        "duration": t.duration,
                        "thumbnail": t.thumbnail,
                        "platform": "local",
                        "isLocal": true,
                        "isCached": true,
                    })
                })
                .collect();
            return Ok((sliced, has_more));
        }

        // 4. Fallback: SoundCloud search
        if let Ok(tracks) = self.soundcloud.search(q, limit).await {
            if !tracks.is_empty() {
                let has_more = tracks.len() >= limit;
                return Ok((tracks, has_more));
            }
        }

        Ok((Vec::new(), false))
    }

    /// Resolve stream URL: Local file, SoundCloud unencrypted HLS/MP3, or YouTube Smart Mirror / yt-dlp
    pub async fn resolve_stream(&self, query: &StreamQuery, local_port: u16) -> Result<Value> {
        let id = query.id.clone().unwrap_or_default().trim().to_string();
        let platform = query.platform.clone().unwrap_or_default().trim().to_string();
        let track_url = query.url.clone().unwrap_or_default().trim().to_string();

        if id.starts_with("local_") {
            let db = self.db.clone();
            let id_clone = id.clone();
            let track = tokio::task::spawn_blocking(move || {
                let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
                db::get_track(&conn, &id_clone)
            })
            .await
            .context("spawn_blocking failed")??
            .ok_or_else(|| anyhow::anyhow!("local track not found"))?;

            return Ok(json!({
                "success": true,
                "isLocal": true,
                "streamUrl": format!(
                    "http://127.0.0.1:{local_port}/api/library-file?id={}",
                    url_encode(&track.id)
                ),
                "sourceMeta": {
                    "id": track.id,
                    "title": track.title,
                    "artist": track.artist,
                    "platform": "local",
                },
            }));
        }

        // 1. Resolve SoundCloud stream natively
        let is_soundcloud = platform == "soundcloud"
            || track_url.contains("soundcloud.com")
            || (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 6);

        if is_soundcloud {
            let identifier = if !track_url.is_empty() { &track_url } else { &id };
            let query_fallback = query.title.as_deref();

            if let Ok(resolved) = self.soundcloud.resolve_stream(identifier, query_fallback).await {
                let is_hls = resolved.is_hls;
                let stream_url = if is_hls {
                    resolved.url.clone()
                } else {
                    format!(
                        "http://127.0.0.1:{local_port}/api/audio-proxy?url={}",
                        url_encode(&resolved.url)
                    )
                };

                return Ok(json!({
                    "success": true,
                    "isLocal": false,
                    "streamUrl": stream_url,
                    "rawStreamUrl": resolved.url,
                    "proxyUrl": format!("http://127.0.0.1:{local_port}/api/audio-proxy?url={}", url_encode(&resolved.url)),
                    "isHls": is_hls,
                    "sourceMeta": {
                        "id": id,
                        "title": if !resolved.title.is_empty() { resolved.title } else { query.title.clone().unwrap_or_default() },
                        "artist": if !resolved.artist.is_empty() { resolved.artist } else { query.artist.clone().unwrap_or_default() },
                        "duration": if resolved.duration > 0.0 { resolved.duration } else { query.duration.unwrap_or(0.0) },
                        "thumbnail": query.thumbnail.clone().unwrap_or_default(),
                        "platform": "soundcloud",
                        "url": track_url,
                    },
                }));
            }
        }

        // 2. Resolve YouTube / Smart Mirror
        let target_url = if !track_url.is_empty() {
            track_url
        } else if id.len() == 11 && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            format!("https://www.youtube.com/watch?v={id}")
        } else if is_soundcloud {
            bail!("failed to resolve soundcloud stream");
        } else {
            bail!("missing url or id");
        };

        let title_val = query.title.clone().unwrap_or_default();
        let artist_val = query.artist.clone().unwrap_or_default();
        let q_val = query.q.clone().unwrap_or_default();
        let duration_val = query.duration.unwrap_or(0.0);
        let effective_title = if !title_val.is_empty() { title_val.clone() } else { q_val };

        let is_yt = platform == "youtube"
            || platform == "ytmusic"
            || target_url.contains("youtube.com")
            || target_url.contains("youtu.be");

        if is_yt && (!effective_title.is_empty() || !artist_val.is_empty()) {
            if let Ok(resolved) = self.soundcloud.resolve_smart_mirror(&effective_title, &artist_val, duration_val).await {
                info!(
                    "Smart Mirror resolved full track '{}' by '{}' (dur: {}s, is_hls: {})",
                    resolved.title, resolved.artist, resolved.duration, resolved.is_hls
                );
                let is_hls = resolved.is_hls;
                let stream_url = if is_hls {
                    resolved.url.clone()
                } else {
                    format!(
                        "http://127.0.0.1:{local_port}/api/audio-proxy?url={}",
                        url_encode(&resolved.url)
                    )
                };

                return Ok(json!({
                    "success": true,
                    "isLocal": false,
                    "streamUrl": stream_url,
                    "rawStreamUrl": resolved.url,
                    "proxyUrl": format!("http://127.0.0.1:{local_port}/api/audio-proxy?url={}", url_encode(&resolved.url)),
                    "isHls": is_hls,
                    "resolvedId": id,
                    "sourceMeta": {
                        "id": id,
                        "title": title_val,
                        "artist": artist_val,
                        "duration": duration_val,
                        "thumbnail": query.thumbnail.clone().unwrap_or_default(),
                        "platform": if platform.is_empty() { "ytmusic".to_string() } else { platform },
                        "url": target_url,
                    },
                }));
            }
        }

        let quality = query.quality.as_deref().unwrap_or("medium");
        let timeout_secs = query.timeout.unwrap_or(20).clamp(5, 30);

        // 2b. Attempt yt-dlp resolution first
        if let Ok(raw_stream) = crate::stream::resolve_with_ytdlp(
            self.config.ytdlp.as_deref(),
            &target_url,
            timeout_secs,
        )
        .await
        {
            let is_hls = raw_stream.contains(".m3u8");
            let proxy_url = format!(
                "http://127.0.0.1:{local_port}/api/audio-proxy?url={}",
                url_encode(&raw_stream)
            );
            let stream_url = if is_hls { raw_stream.clone() } else { proxy_url.clone() };

            return Ok(json!({
                "success": true,
                "isLocal": false,
                "streamUrl": stream_url,
                "rawStreamUrl": raw_stream,
                "proxyUrl": proxy_url,
                "isHls": is_hls,
                "resolvedId": id,
                "sourceMeta": {
                    "id": id,
                    "title": title_val,
                    "artist": artist_val,
                    "duration": duration_val,
                    "thumbnail": query.thumbnail.clone().unwrap_or_default(),
                    "platform": if platform.is_empty() { "ytmusic".to_string() } else { platform },
                    "url": target_url,
                },
            }));
        }

        // 2c. Fallback: native InnerTube player resolution for YouTube
        if is_yt {
            let cand_id = if !id.is_empty() { &id } else { &target_url };
            if let Ok(direct_url) = self.youtube.resolve_stream(cand_id, quality).await {
                let proxy_url = format!(
                    "http://127.0.0.1:{local_port}/api/audio-proxy?url={}",
                    url_encode(&direct_url)
                );

                return Ok(json!({
                    "success": true,
                    "isLocal": false,
                    "streamUrl": proxy_url,
                    "rawStreamUrl": direct_url,
                    "proxyUrl": proxy_url,
                    "isHls": false,
                    "resolvedId": id,
                    "sourceMeta": {
                        "id": id,
                        "title": title_val,
                        "artist": artist_val,
                        "duration": duration_val,
                        "thumbnail": query.thumbnail.clone().unwrap_or_default(),
                        "platform": "youtube",
                        "url": target_url,
                    },
                }));
            }
        }

        bail!("failed to resolve audio stream via smart mirror, yt-dlp, or innertube");
    }

    /// List all tracks in SQLite library
    pub async fn list_library(&self) -> Result<Vec<Track>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::list_tracks(&conn)
        })
        .await
        .context("spawn_blocking failed")?
    }

    /// Add items to SQLite library
    pub async fn add_library_items(&self, items: Vec<LibraryAddItem>) -> Result<usize> {
        let db = self.db.clone();
        let library_dir = self.config.library_dir.clone();

        let inserted = tokio::task::spawn_blocking(move || -> Result<usize> {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            let now = db::now_millis();
            let mut count = 0;

            for item in items {
                let file_name = item.file.trim();
                if file_name.is_empty() {
                    continue;
                }
                let path = library_dir.join(file_name);
                let path_string = path.to_string_lossy().to_string();

                let id = item.id.unwrap_or_else(|| library::local_id(&path));
                let display = item.name.unwrap_or_else(|| file_name.to_string());
                let (title, artist) = library::parse_display_name(&display);

                let track = Track {
                    id,
                    path: path_string,
                    file: file_name.to_string(),
                    title,
                    artist,
                    album: None,
                    duration: 0.0,
                    thumbnail: String::new(),
                    added_at: now,
                    updated_at: now,
                };

                if db::upsert_track(&conn, &track).is_ok() {
                    count += 1;
                }
            }

            Ok(count)
        })
        .await
        .context("spawn_blocking failed")??;

        if inserted > 0 {
            if let Ok(total) = self.count_tracks().await {
                self.events.emit(RiffEvent::LibraryUpdated { count: total });
            }
        }

        Ok(inserted)
    }

    /// Remove a track from library
    pub async fn remove_from_library(&self, id: &str) -> Result<bool> {
        let db = self.db.clone();
        let id_owned = id.to_string();
        let removed = tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::remove_track(&conn, &id_owned)
        })
        .await
        .context("spawn_blocking failed")??;

        if removed {
            if let Ok(total) = self.count_tracks().await {
                self.events.emit(RiffEvent::LibraryUpdated { count: total });
            }
        }

        Ok(removed)
    }

    /// Scan directory for audio files and add them to library
    pub async fn scan_library(&self, dir: Option<&Path>) -> Result<ScanResult> {
        let db = self.db.clone();
        let target_dir = match dir {
            Some(d) => d.to_path_buf(),
            None => self.config.library_dir.clone(),
        };

        let result = tokio::task::spawn_blocking(move || library::scan_library(&db, &target_dir))
            .await
            .context("spawn_blocking failed")??;

        self.events.emit(RiffEvent::LibraryUpdated { count: result.total });

        Ok(result)
    }

    /// Total count of tracks
    pub async fn count_tracks(&self) -> Result<i64> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::count(&conn)
        })
        .await
        .context("spawn_blocking failed")?
    }

    /// Get setting value from SQLite kv_store
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let db = self.db.clone();
        let key_owned = key.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::get_kv(&conn, &key_owned)
        })
        .await
        .context("spawn_blocking failed")?
    }

    /// Set setting value into SQLite kv_store
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let db = self.db.clone();
        let key_owned = key.to_string();
        let value_owned = value.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::set_kv(&conn, &key_owned, &value_owned)
        })
        .await
        .context("spawn_blocking failed")??;

        if key == "ui_mode" {
            self.events.emit(RiffEvent::UiModeChanged { mode: value.to_string() });
        }

        Ok(())
    }

    /// Get all settings
    pub async fn get_all_settings(&self) -> Result<HashMap<String, String>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|_| anyhow::anyhow!("db mutex poisoned"))?;
            db::get_all_kv(&conn)
        })
        .await
        .context("spawn_blocking failed")?
    }

    /// Get UI mode ("web" or "native")
    pub async fn get_ui_mode(&self) -> String {
        self.get_setting("ui_mode")
            .await
            .unwrap_or(None)
            .unwrap_or_else(|| "web".to_string())
    }

    /// Set UI mode ("web" or "native")
    pub async fn set_ui_mode(&self, mode: &str) -> Result<()> {
        let sanitized = match mode.to_ascii_lowercase().as_str() {
            "native" | "slint" => "native",
            _ => "web",
        };
        self.set_setting("ui_mode", sanitized).await
    }

    /// Fetch lyrics for a track
    pub async fn fetch_lyrics(&self, query: LyricsQuery) -> Value {
        self.lyrics.fetch_lyrics(query).await.0
    }

    /// Save custom or synced LRC lyrics for a track
    pub async fn save_custom_lyrics(&self, title: &str, artist: &str, content: &str) -> Result<()> {
        self.lyrics.save_custom_lyrics(title, artist, content).await
    }

    /// Get favorite tracks from SQLite kv_store
    pub async fn get_favorites(&self) -> Result<Vec<Track>> {
        if let Some(json_str) = self.get_setting("riff_favorites").await? {
            if let Ok(tracks) = serde_json::from_str::<Vec<Track>>(&json_str) {
                return Ok(tracks);
            }
        }
        Ok(Vec::new())
    }

    /// Save favorite tracks into SQLite kv_store
    pub async fn save_favorites(&self, tracks: &[Track]) -> Result<()> {
        let json_str = serde_json::to_string(tracks)?;
        self.set_setting("riff_favorites", &json_str).await
    }

    /// Toggle favorite status of a track
    pub async fn toggle_favorite(&self, track: &Track) -> Result<bool> {
        let mut list = self.get_favorites().await?;
        let exists_idx = list.iter().position(|t| t.id == track.id || t.path == track.path);
        let is_now_fav = if let Some(idx) = exists_idx {
            list.remove(idx);
            false
        } else {
            list.insert(0, track.clone());
            true
        };
        self.save_favorites(&list).await?;
        Ok(is_now_fav)
    }

    /// Get play history from SQLite kv_store
    pub async fn get_history(&self) -> Result<Vec<Track>> {
        if let Some(json_str) = self.get_setting("riff_history").await? {
            if let Ok(tracks) = serde_json::from_str::<Vec<Track>>(&json_str) {
                return Ok(tracks);
            }
        }
        Ok(Vec::new())
    }

    /// Record a track to play history
    pub async fn push_history(&self, track: &Track) -> Result<()> {
        let mut list = self.get_history().await?;
        list.retain(|t| t.id != track.id && t.path != track.path);
        list.insert(0, track.clone());
        if list.len() > 100 {
            list.truncate(100);
        }
        let json_str = serde_json::to_string(&list)?;
        self.set_setting("riff_history", &json_str).await
    }

    /// Clear play history
    pub async fn clear_history(&self) -> Result<()> {
        self.set_setting("riff_history", "[]").await
    }

    /// Get user playlists from SQLite kv_store
    pub async fn get_playlists(&self) -> Result<Vec<crate::models::Playlist>> {
        if let Some(json_str) = self.get_setting("riff_playlists").await? {
            if let Ok(list) = serde_json::from_str::<Vec<crate::models::Playlist>>(&json_str) {
                return Ok(list);
            }
        }
        Ok(Vec::new())
    }

    /// Save user playlists into SQLite kv_store
    pub async fn save_playlists(&self, playlists: &[crate::models::Playlist]) -> Result<()> {
        let json_str = serde_json::to_string(playlists)?;
        self.set_setting("riff_playlists", &json_str).await
    }

    /// Create a new playlist
    pub async fn create_playlist(&self, name: &str) -> Result<crate::models::Playlist> {
        let mut list = self.get_playlists().await?;
        let id = format!("pl_{}", db::now_millis());
        let pl = crate::models::Playlist {
            id,
            name: name.trim().to_string(),
            tracks: Vec::new(),
        };
        list.push(pl.clone());
        self.save_playlists(&list).await?;
        Ok(pl)
    }

    /// Rename a playlist
    pub async fn rename_playlist(&self, id: &str, new_name: &str) -> Result<()> {
        let mut list = self.get_playlists().await?;
        if let Some(pl) = list.iter_mut().find(|p| p.id == id) {
            pl.name = new_name.trim().to_string();
            self.save_playlists(&list).await?;
        }
        Ok(())
    }

    /// Delete a playlist
    pub async fn delete_playlist(&self, id: &str) -> Result<()> {
        let mut list = self.get_playlists().await?;
        list.retain(|p| p.id != id);
        self.save_playlists(&list).await
    }

    /// Add a track to a playlist
    pub async fn add_track_to_playlist(&self, playlist_id: &str, track: &Track) -> Result<()> {
        let mut list = self.get_playlists().await?;
        if let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) {
            if !pl.tracks.iter().any(|t| t.id == track.id) {
                pl.tracks.push(track.clone());
                self.save_playlists(&list).await?;
            }
        }
        Ok(())
    }

    /// Remove a track from a playlist
    pub async fn remove_track_from_playlist(&self, playlist_id: &str, track_id: &str) -> Result<()> {
        let mut list = self.get_playlists().await?;
        if let Some(pl) = list.iter_mut().find(|p| p.id == playlist_id) {
            pl.tracks.retain(|t| t.id != track_id);
            self.save_playlists(&list).await?;
        }
        Ok(())
    }
}
