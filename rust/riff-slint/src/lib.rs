use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

slint::include_modules!();

#[derive(Clone)]
struct TrackItem {
    id: String,
    title: String,
    artist: String,
    album: String,
    duration_sec: f64,
    platform: String,
    thumbnail: String,
    is_favorite: bool,
    cover_rgba: Option<(u32, u32, Vec<u8>)>,
}

impl TrackItem {
    fn from_core(t: &riff_core::models::Track, platform: &str, is_favorite: bool) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone().unwrap_or_default(),
            duration_sec: t.duration,
            platform: platform.to_string(),
            thumbnail: t.thumbnail.clone(),
            is_favorite,
            cover_rgba: None,
        }
    }

    fn into_slint(self) -> TrackData {
        let (cover, has_cover) = match self.cover_rgba {
            Some((w, h, raw)) => (rgba_to_slint_image(w, h, raw), true),
            None => (Image::default(), false),
        };
        TrackData {
            id: SharedString::from(self.id),
            title: SharedString::from(self.title),
            artist: SharedString::from(self.artist),
            album: SharedString::from(self.album),
            duration_str: SharedString::from(format_duration(self.duration_sec)),
            duration_sec: self.duration_sec as f32,
            platform: SharedString::from(self.platform),
            thumbnail: SharedString::from(self.thumbnail),
            is_favorite: self.is_favorite,
            cover,
            has_cover,
        }
    }
}

fn format_duration(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    let m = s / 60;
    let rem = s % 60;
    format!("{m}:{rem:02}")
}

async fn fetch_cover_rgba(http: &reqwest::Client, url: &str) -> Option<(u32, u32, Vec<u8>)> {
    if url.is_empty() || !url.starts_with("http") {
        return None;
    }
    let resp = http.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let thumb = img.resize_to_fill(240, 240, image::imageops::FilterType::Triangle);
    let rgba = thumb.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w, h, rgba.into_raw()))
}

fn rgba_to_slint_image(w: u32, h: u32, raw: Vec<u8>) -> Image {
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&raw, w, h);
    Image::from_rgba8(buf)
}

fn compute_ambient_blur_and_accent(w: u32, h: u32, raw: &[u8]) -> Option<(u32, u32, Vec<u8>, (u8, u8, u8))> {
    let img = image::RgbaImage::from_raw(w, h, raw.to_vec())?;
    let small = image::imageops::resize(&img, 96, 96, image::imageops::FilterType::Triangle);

    // Extract vibrant dominant accent color
    let mut sum_r = 0.0f64;
    let mut sum_g = 0.0f64;
    let mut sum_b = 0.0f64;
    let mut total_w = 0.0f64;

    for px in small.pixels() {
        let r = px[0] as f64;
        let g = px[1] as f64;
        let b = px[2] as f64;
        let max_c = r.max(g).max(b);
        let min_c = r.min(g).min(b);
        let sat = if max_c > 1.0 { (max_c - min_c) / max_c } else { 0.0 };
        let lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        // Prefer saturated, non-black, non-blown-out pixels
        let weight = if lum > 0.08 && lum < 0.92 {
            0.15 + sat * sat * 3.0
        } else {
            0.02
        };
        sum_r += r * weight;
        sum_g += g * weight;
        sum_b += b * weight;
        total_w += weight;
    }

    let accent = if total_w > 0.0 {
        let mut r = (sum_r / total_w).clamp(0.0, 255.0);
        let mut g = (sum_g / total_w).clamp(0.0, 255.0);
        let mut b = (sum_b / total_w).clamp(0.0, 255.0);
        // Ensure accent isn't too dark for the subtle top glow
        let max_c = r.max(g).max(b);
        if max_c > 1.0 && max_c < 140.0 {
            let boost = 140.0 / max_c;
            r = (r * boost).min(255.0);
            g = (g * boost).min(255.0);
            b = (b * boost).min(255.0);
        } else if max_c <= 1.0 {
            r = 159.0;
            g = 153.0;
            b = 230.0;
        }
        (r as u8, g as u8, b as u8)
    } else {
        (0x9f, 0x99, 0xe6)
    };

    let blurred = image::imageops::blur(&small, 14.0);
    let (bw, bh) = blurred.dimensions();
    Some((bw, bh, blurred.into_raw(), accent))
}

fn to_slint_model(items: Vec<TrackItem>) -> ModelRc<TrackData> {
    let data: Vec<TrackData> = items.into_iter().map(TrackItem::into_slint).collect();
    ModelRc::new(VecModel::from(data))
}

fn format_lrc_tag(sec: f32) -> String {
    let s = sec.max(0.0);
    let m = (s / 60.0).floor() as u32;
    let rem = s - (m as f32) * 60.0;
    format!("[{m:02}:{rem:05.2}]")
}

fn is_gap_text(text: &str) -> bool {
    let t = text.trim();
    t.eq_ignore_ascii_case("[Instrumental]")
        || t.eq_ignore_ascii_case("Instrumental")
        || t.eq_ignore_ascii_case("[Music]")
}

/// Parse LRC formatted synchronized lyrics or fallback to line-by-line text
fn parse_lrc(text: &str, duration_sec: f64) -> Vec<LyricLineData> {
    let mut lines = Vec::new();
    let mut has_timestamps = false;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') {
            if let Some(close_bracket) = trimmed.find(']') {
                let time_part = &trimmed[1..close_bracket];
                let text_part = trimmed[close_bracket + 1..].trim();

                if let Some(colon) = time_part.find(':') {
                    let min_str = &time_part[..colon];
                    let sec_str = &time_part[colon + 1..];
                    if let (Ok(min), Ok(sec)) = (min_str.parse::<f64>(), sec_str.parse::<f64>()) {
                        let total = min * 60.0 + sec;
                        has_timestamps = true;
                        lines.push(LyricLineData {
                            time_sec: total as f32,
                            time_str: SharedString::from(format_duration(total)),
                            text: SharedString::from(text_part),
                            is_active: false,
                            is_past: false,
                            is_gap: is_gap_text(text_part),
                        });
                        continue;
                    }
                }
            }
        }
    }

    if has_timestamps && !lines.is_empty() {
        lines.sort_by(|a, b| a.time_sec.partial_cmp(&b.time_sec).unwrap_or(std::cmp::Ordering::Equal));
        return lines;
    }

    let raw_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && (!l.starts_with('[') || is_gap_text(l)) && !l.starts_with('{'))
        .collect();

    if raw_lines.is_empty() {
        return Vec::new();
    }

    let interval = if duration_sec > 5.0 {
        (duration_sec / raw_lines.len() as f64) as f32
    } else {
        4.0f32
    };

    raw_lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            let t = (i as f32) * interval;
            LyricLineData {
                time_sec: t,
                time_str: SharedString::from(format_duration(t as f64)),
                text: SharedString::from(l),
                is_active: false,
                is_past: false,
                is_gap: is_gap_text(l),
            }
        })
        .collect()
}

fn playlists_to_slint(list: &[riff_core::models::Playlist]) -> ModelRc<PlaylistData> {
    let data: Vec<PlaylistData> = list
        .iter()
        .map(|p| PlaylistData {
            id: SharedString::from(&p.id),
            name: SharedString::from(&p.name),
            track_count: p.tracks.len() as i32,
        })
        .collect();
    ModelRc::new(VecModel::from(data))
}

fn track_data_to_core(t: &TrackData) -> riff_core::models::Track {
    riff_core::models::Track {
        id: t.id.to_string(),
        path: t.id.to_string(),
        file: t.title.to_string(),
        title: t.title.to_string(),
        artist: t.artist.to_string(),
        album: if t.album.is_empty() { None } else { Some(t.album.to_string()) },
        duration: t.duration_sec as f64,
        thumbnail: t.thumbnail.to_string(),
        added_at: 0,
        updated_at: 0,
    }
}

fn track_data_to_item(t: &TrackData) -> TrackItem {
    TrackItem {
        id: t.id.to_string(),
        title: t.title.to_string(),
        artist: t.artist.to_string(),
        album: t.album.to_string(),
        duration_sec: t.duration_sec as f64,
        platform: t.platform.to_string(),
        thumbnail: t.thumbnail.to_string(),
        is_favorite: t.is_favorite,
        cover_rgba: t.cover.to_rgba8().map(|b| (b.width(), b.height(), b.as_bytes().to_vec())),
    }
}

async fn json_tracks_to_items(
    raw: Vec<serde_json::Value>,
    http: &reqwest::Client,
    fav_ids: &HashSet<String>,
    default_platform: &str,
) -> Vec<TrackItem> {
    let mut items: Vec<TrackItem> = raw
        .into_iter()
        .map(|v| {
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("Untitled").to_string();
            let artist = v.get("artist").and_then(|x| x.as_str()).unwrap_or("Unknown").to_string();
            let album = v.get("album").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let duration_sec = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let platform = v
                .get("platform")
                .and_then(|x| x.as_str())
                .unwrap_or(default_platform)
                .to_string();
            let thumbnail = v.get("thumbnail").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let is_favorite = fav_ids.contains(&id);
            TrackItem {
                id,
                title,
                artist,
                album,
                duration_sec,
                platform,
                thumbnail,
                is_favorite,
                cover_rgba: None,
            }
        })
        .collect();

    let mut set = tokio::task::JoinSet::new();
    for (idx, item) in items.iter().enumerate() {
        if !item.thumbnail.is_empty() {
            let client = http.clone();
            let url = item.thumbnail.clone();
            set.spawn(async move { (idx, fetch_cover_rgba(&client, &url).await) });
        }
    }
    while let Some(res) = set.join_next().await {
        if let Ok((idx, rgba)) = res {
            if let Some(slot) = items.get_mut(idx) {
                slot.cover_rgba = rgba;
            }
        }
    }

    items
}

fn detect_audio_hint(content_type: Option<&str>, url: &str) -> Option<String> {
    if let Some(ct) = content_type {
        let ct_low = ct.to_ascii_lowercase();
        if ct_low.contains("audio/mp4") || ct_low.contains("video/mp4") || ct_low.contains("mp4a") {
            return Some("m4a".to_string());
        }
        if ct_low.contains("audio/mpeg") || ct_low.contains("audio/mp3") {
            return Some("mp3".to_string());
        }
        if ct_low.contains("audio/aac") {
            return Some("aac".to_string());
        }
        if ct_low.contains("audio/ogg") || ct_low.contains("application/ogg") {
            return Some("ogg".to_string());
        }
        if ct_low.contains("audio/flac") {
            return Some("flac".to_string());
        }
        if ct_low.contains("audio/wav") || ct_low.contains("audio/x-wav") {
            return Some("wav".to_string());
        }
    }
    let url_low = url.to_ascii_lowercase();
    if url_low.contains("mime=audio%2fmp4") || url_low.contains("mime=audio/mp4") || url_low.contains(".m4a") || url_low.contains(".mp4") {
        return Some("m4a".to_string());
    }
    if url_low.contains(".mp3") {
        return Some("mp3".to_string());
    }
    if url_low.contains(".aac") {
        return Some("aac".to_string());
    }
    if url_low.contains(".ogg") {
        return Some("ogg".to_string());
    }
    if url_low.contains(".flac") {
        return Some("flac".to_string());
    }
    if url_low.contains(".wav") {
        return Some("wav".to_string());
    }
    None
}

fn googlevideo_user_agent(raw_url: &str) -> &'static str {
    if let Ok(parsed) = reqwest::Url::parse(raw_url) {
        for (k, v) in parsed.query_pairs() {
            if k == "c" {
                if v == "ANDROID_MUSIC" {
                    return "com.google.android.apps.youtube.music/7.27.52 (Linux; U; Android 14; en_US) gzip";
                } else if v == "IOS" {
                    return "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
                } else if v == "VISIONOS" || v == "WEB" || v == "WEB_REMIX" || v == "MWEB" {
                    return "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
                }
            }
        }
    }
    "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)"
}

async fn download_hls_bytes(http: &reqwest::Client, playlist_url: &str) -> Result<(Vec<u8>, Option<String>), String> {
    let base_url = reqwest::Url::parse(playlist_url).map_err(|e| format!("Invalid HLS URL: {e}"))?;
    let resp = http
        .get(playlist_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("HLS fetch failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HLS HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| format!("HLS read failed: {e}"))?;

    // If master playlist, pick first variant playlist
    if text.contains("#EXT-X-STREAM-INF") {
        for line in text.lines() {
            let t = line.trim();
            if !t.is_empty() && !t.starts_with('#') {
                if let Ok(sub_url) = base_url.join(t) {
                    return Box::pin(download_hls_bytes(http, sub_url.as_str())).await;
                }
            }
        }
    }

    let mut segment_urls: Vec<String> = Vec::new();
    let mut is_fmp4 = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("#EXT-X-MAP:") {
            if let Some(uri_pos) = t.find("URI=\"") {
                let rest = &t[uri_pos + 5..];
                if let Some(end_q) = rest.find('"') {
                    let map_uri = &rest[..end_q];
                    if let Ok(joined) = base_url.join(map_uri) {
                        segment_urls.push(joined.to_string());
                        is_fmp4 = true;
                    }
                }
            }
        } else if !t.is_empty() && !t.starts_with('#') {
            if let Ok(joined) = base_url.join(t) {
                segment_urls.push(joined.to_string());
            }
        }
    }

    if segment_urls.is_empty() {
        return Err("HLS playlist contained no segments".to_string());
    }

    let mut out = Vec::new();
    for chunk in segment_urls.chunks(6) {
        let mut futs = Vec::with_capacity(chunk.len());
        for seg_url in chunk {
            let client = http.clone();
            let u = seg_url.clone();
            futs.push(tokio::spawn(async move {
                let r = client
                    .get(&u)
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !r.status().is_success() {
                    return Err(format!("Segment HTTP {}", r.status()));
                }
                r.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
            }));
        }
        for f in futs {
            let bytes = f.await.map_err(|e| e.to_string())??;
            out.extend_from_slice(&bytes);
        }
    }

    let hint = if is_fmp4 || segment_urls.iter().any(|u| u.contains(".m4s") || u.contains(".mp4")) {
        Some("m4a".to_string())
    } else if segment_urls.iter().any(|u| u.contains(".aac")) {
        Some("aac".to_string())
    } else if segment_urls.iter().any(|u| u.contains(".mp3")) {
        Some("mp3".to_string())
    } else {
        None
    };

    Ok((out, hint))
}

async fn download_stream_bytes(
    http: &reqwest::Client,
    raw_url: &str,
    is_hls: bool,
) -> Result<(Vec<u8>, Option<String>), String> {
    if is_hls || raw_url.contains(".m3u8") {
        return download_hls_bytes(http, raw_url).await;
    }

    let is_googlevideo = reqwest::Url::parse(raw_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with("googlevideo.com")))
        .unwrap_or(false);

    if is_googlevideo {
        const CHUNK_SIZE: u64 = 1_048_576;
        let ua = googlevideo_user_agent(raw_url);
        let mut out = Vec::new();
        let mut start = 0u64;
        let mut detected_hint = detect_audio_hint(None, raw_url);

        loop {
            let end = start + CHUNK_SIZE - 1;
            let resp = http
                .get(raw_url)
                .header("User-Agent", ua)
                .header("Range", format!("bytes={start}-{end}"))
                .timeout(Duration::from_secs(20))
                .send()
                .await
                .map_err(|e| format!("Stream chunk failed: {e}"))?;

            let status = resp.status();
            if !status.is_success() && status.as_u16() != 206 {
                return Err(format!("Stream HTTP {status}"));
            }

            if detected_hint.is_none() {
                let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
                detected_hint = detect_audio_hint(ct, raw_url);
            }

            let total_len = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|cr| cr.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok());

            let chunk = resp.bytes().await.map_err(|e| format!("Chunk read failed: {e}"))?;
            if chunk.is_empty() {
                break;
            }
            let chunk_len = chunk.len() as u64;
            out.extend_from_slice(&chunk);
            start += chunk_len;

            if let Some(total) = total_len {
                if start >= total {
                    break;
                }
            } else if status.as_u16() == 200 || chunk_len < CHUNK_SIZE {
                break;
            }
        }

        if out.is_empty() {
            return Err("Empty audio stream".to_string());
        }
        return Ok((out, detected_hint));
    }

    let resp = http
        .get(raw_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Stream request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Stream HTTP {}", resp.status()));
    }

    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let hint = detect_audio_hint(ct.as_deref(), raw_url);
    let bytes = resp.bytes().await.map_err(|e| format!("Stream read failed: {e}"))?;
    if bytes.is_empty() {
        return Err("Empty audio stream".to_string());
    }
    Ok((bytes.to_vec(), hint))
}

fn show_toast(w_weak: &slint::Weak<MainWindow>, rt: &tokio::runtime::Handle, msg: impl Into<String>) {
    let text = msg.into();
    let w_show = w_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = w_show.upgrade() {
            w.set_toast_text(SharedString::from(&text));
            w.set_toast_open(true);
            w.set_status_message(SharedString::from(&text));
        }
    });
    let w_hide = w_weak.clone();
    rt.spawn(async move {
        tokio::time::sleep(Duration::from_millis(2600)).await;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_hide.upgrade() {
                w.set_toast_open(false);
            }
        });
    });
}

async fn load_home_sections(
    api: &riff_core::api::RiffApi,
    http: &reqwest::Client,
    fav_ids: &HashSet<String>,
) -> (Vec<TrackItem>, Vec<TrackItem>) {
    let (fy_res, tr_res) = tokio::join!(
        api.search("top hits mix", "ytmusic", 8, 0),
        api.search("trending music 2026", "ytmusic", 8, 0),
    );
    let for_you_raw = fy_res.map(|(t, _)| t).unwrap_or_default();
    let trending_raw = tr_res.map(|(t, _)| t).unwrap_or_default();

    tokio::join!(
        json_tracks_to_items(for_you_raw, http, fav_ids, "ytmusic"),
        json_tracks_to_items(trending_raw, http, fav_ids, "ytmusic"),
    )
}

pub fn run() -> Result<(), slint::PlatformError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");
    let rt_handle = rt.handle().clone();

    let config = Arc::new(riff_core::config::Config::default());
    let db = riff_core::db::open(&config.db_path).expect("Failed to open SQLite database");
    let http = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Riff/0.7.6")
        .build()
        .expect("Failed to create HTTP client");

    let soundcloud = riff_core::soundcloud::SoundCloudClient::new(http.clone());
    let youtube = Arc::new(riff_core::youtube::YouTubeClient::new(http.clone()));
    let lyrics = Arc::new(riff_core::lyrics::LyricsService::new(http.clone(), config.data_dir.clone()));
    let events = Arc::new(riff_core::events::EventBus::default());
    let player = Arc::new(riff_core::player::AudioPlayer::new(events.clone()));

    let api = Arc::new(riff_core::api::RiffApi::new(
        db,
        config.clone(),
        soundcloud,
        youtube,
        lyrics,
        events.clone(),
        player.clone(),
    ));

    let main_window = MainWindow::new()?;
    let window_weak = main_window.as_weak();

    let active_lyrics_cache: Arc<Mutex<Vec<LyricLineData>>> = Arc::new(Mutex::new(Vec::new()));
    let playlist_cache: Arc<Mutex<Vec<TrackItem>>> = Arc::new(Mutex::new(Vec::new()));
    let current_index: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let shuffle_state = Arc::new(AtomicBool::new(false));
    let repeat_state = Arc::new(AtomicU8::new(0)); // 0: off, 1: all, 2: one
    let sync_lines_cache: Arc<Mutex<Vec<SyncLineData>>> = Arc::new(Mutex::new(Vec::new()));
    let pending_playlist_track: Arc<Mutex<Option<TrackItem>>> = Arc::new(Mutex::new(None));
    let sleep_deadline: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let sleep_track_end = Arc::new(AtomicBool::new(false));
    let eof_handled = Arc::new(AtomicBool::new(false));
    let play_generation = Arc::new(AtomicU64::new(0));

    // 1. Initial Data Load (Library, Favorites, History, Playlists, Home Feed)
    let api_init = api.clone();
    let http_init = http.clone();
    let w_init = window_weak.clone();
    let pl_init = playlist_cache.clone();

    rt_handle.spawn(async move {
        let (lib_res, fav_res, hist_res, pls_res) = tokio::join!(
            api_init.list_library(),
            api_init.get_favorites(),
            api_init.get_history(),
            api_init.get_playlists(),
        );
        let lib_tracks = lib_res.unwrap_or_default();
        let fav_tracks = fav_res.unwrap_or_default();
        let hist_tracks = hist_res.unwrap_or_default();
        let user_playlists = pls_res.unwrap_or_default();

        let fav_ids: HashSet<String> = fav_tracks.iter().map(|t| t.id.clone()).collect();

        let lib_items: Vec<TrackItem> = lib_tracks
            .iter()
            .map(|t| {
                let mut item = TrackItem::from_core(t, "local", fav_ids.contains(&t.id));
                if !t.path.is_empty() {
                    item.id = t.path.clone();
                }
                item
            })
            .collect();
        let fav_items: Vec<TrackItem> = fav_tracks
            .iter()
            .map(|t| TrackItem::from_core(t, "favorite", true))
            .collect();
        let hist_items: Vec<TrackItem> = hist_tracks
            .iter()
            .map(|t| TrackItem::from_core(t, "history", fav_ids.contains(&t.id)))
            .collect();

        if let Ok(mut pl) = pl_init.lock() {
            *pl = lib_items.clone();
        }

        let queue_init = lib_items.clone();
        let w_first = w_init.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_first.upgrade() {
                w.set_library_tracks(to_slint_model(lib_items));
                w.set_favorite_tracks(to_slint_model(fav_items));
                w.set_history_tracks(to_slint_model(hist_items));
                w.set_queue_tracks(to_slint_model(queue_init));
                w.set_playlists(playlists_to_slint(&user_playlists));
            }
        });

        // Load Home sections ("For you" & "Trending now") with cover art
        let (fy_items, tr_items) = load_home_sections(&api_init, &http_init, &fav_ids).await;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_init.upgrade() {
                w.set_home_for_you(to_slint_model(fy_items));
                w.set_home_trending(to_slint_model(tr_items));
            }
        });
    });

    // Refresh Home Callback
    let api_ref = api.clone();
    let http_ref = http.clone();
    let w_ref = window_weak.clone();
    let rt_ref = rt_handle.clone();
    main_window.on_refresh_home(move || {
        let api_r = api_ref.clone();
        let http_r = http_ref.clone();
        let w_r = w_ref.clone();
        rt_ref.spawn(async move {
            let favs = api_r.get_favorites().await.unwrap_or_default();
            let fav_ids: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
            let (fy_items, tr_items) = load_home_sections(&api_r, &http_r, &fav_ids).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_r.upgrade() {
                    w.set_home_for_you(to_slint_model(fy_items));
                    w.set_home_trending(to_slint_model(tr_items));
                }
            });
        });
    });

    // 2. Play Track Callback
    let api_play = api.clone();
    let http_play = http.clone();
    let w_play = window_weak.clone();
    let lyr_play = active_lyrics_cache.clone();
    let pl_play = playlist_cache.clone();
    let cur_idx_play = current_index.clone();
    let eof_play = eof_handled.clone();
    let gen_play = play_generation.clone();
    let rt_play = rt_handle.clone();

    main_window.on_play_track(move |track| {
        eof_play.store(false, Ordering::SeqCst);
        let my_gen = gen_play.fetch_add(1, Ordering::SeqCst) + 1;
        let t_thumb = track.thumbnail.to_string();
        let has_cov = track.has_cover;

        if let Some(w) = w_play.upgrade() {
            w.set_current_id(track.id.clone());
            w.set_current_title(track.title.clone());
            w.set_current_artist(track.artist.clone());
            w.set_current_album(track.album.clone());
            w.set_current_platform(track.platform.clone());
            w.set_current_is_favorite(track.is_favorite);
            w.set_active_lyric_index(-1);
            w.set_current_pos(0.0);
            w.set_pos_text(SharedString::from("0:00"));
            if track.duration_sec > 0.0 {
                w.set_current_duration(track.duration_sec);
                w.set_dur_text(SharedString::from(format_duration(track.duration_sec as f64)));
            }
            w.set_status_message(SharedString::from(format!("Loading: {}...", track.title)));
            w.set_current_has_cover(has_cov);
            if has_cov {
                w.set_current_cover(track.cover.clone());
                if let Some(buf) = track.cover.to_rgba8() {
                    if let Some((bw, bh, braw, (ar, ag, ab))) =
                        compute_ambient_blur_and_accent(buf.width(), buf.height(), buf.as_bytes())
                    {
                        w.set_current_cover_blur(rgba_to_slint_image(bw, bh, braw));
                        w.set_current_has_blur(true);
                        w.set_current_accent(slint::Color::from_rgb_u8(ar, ag, ab));
                    }
                }
            } else {
                w.set_current_has_blur(false);
                w.set_current_accent(slint::Color::from_rgb_u8(0x9f, 0x99, 0xe6));
            }
        }

        if !has_cov && !t_thumb.is_empty() {
            let http_c = http_play.clone();
            let w_c = w_play.clone();
            let thumb_url = t_thumb.clone();
            rt_play.spawn(async move {
                if let Some((cw, ch, raw)) = fetch_cover_rgba(&http_c, &thumb_url).await {
                    let blur_data = compute_ambient_blur_and_accent(cw, ch, &raw);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_c.upgrade() {
                            w.set_current_cover(rgba_to_slint_image(cw, ch, raw));
                            w.set_current_has_cover(true);
                            if let Some((bw, bh, braw, (ar, ag, ab))) = blur_data {
                                w.set_current_cover_blur(rgba_to_slint_image(bw, bh, braw));
                                w.set_current_has_blur(true);
                                w.set_current_accent(slint::Color::from_rgb_u8(ar, ag, ab));
                            }
                        }
                    });
                }
            });
        }

        if let (Ok(mut pl), Ok(mut idx)) = (pl_play.lock(), cur_idx_play.lock()) {
            if let Some(pos) = pl.iter().position(|t| t.id == track.id.as_str()) {
                *idx = pos;
            } else {
                pl.insert(0, track_data_to_item(&track));
                *idx = 0;
            }
            let q_clone = pl.clone();
            if let Some(w) = w_play.upgrade() {
                w.set_queue_tracks(to_slint_model(q_clone));
            }
        }

        let core_track = riff_core::models::Track {
            id: track.id.to_string(),
            path: track.id.to_string(),
            file: track.title.to_string(),
            title: track.title.to_string(),
            artist: track.artist.to_string(),
            album: if track.album.is_empty() { None } else { Some(track.album.to_string()) },
            duration: track.duration_sec as f64,
            thumbnail: track.thumbnail.to_string(),
            added_at: 0,
            updated_at: 0,
        };

        let api_hist = api_play.clone();
        let track_for_hist = core_track.clone();
        let w_hist = w_play.clone();

        rt_play.spawn(async move {
            let _ = api_hist.push_history(&track_for_hist).await;
            if let Ok(updated_hist) = api_hist.get_history().await {
                let favs = api_hist.get_favorites().await.unwrap_or_default();
                let fav_set: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                let h_items: Vec<TrackItem> = updated_hist
                    .iter()
                    .map(|t| TrackItem::from_core(t, "history", fav_set.contains(&t.id)))
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_hist.upgrade() {
                        w.set_history_tracks(to_slint_model(h_items));
                    }
                });
            }
        });

        // Fetch Synced Lyrics
        let api_lyr = api_play.clone();
        let lyr_store = lyr_play.clone();
        let w_lyr = w_play.clone();
        let q_title = track.title.to_string();
        let q_artist = track.artist.to_string();
        let q_dur = track.duration_sec as f64;

        rt_play.spawn(async move {
            let val = api_lyr
                .fetch_lyrics(riff_core::lyrics::LyricsQuery {
                    title: Some(q_title),
                    artist: Some(q_artist),
                    source: None,
                    duration: Some(q_dur),
                    online: Some("1".to_string()),
                })
                .await;

            let raw_text = val.get("lyrics").and_then(|x| x.as_str()).unwrap_or("");
            let parsed = parse_lrc(raw_text, q_dur);

            if let Ok(mut store) = lyr_store.lock() {
                *store = parsed.clone();
            }

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_lyr.upgrade() {
                    w.set_lyrics_lines(ModelRc::new(VecModel::from(parsed)));
                }
            });
        });

        // Play audio (local file or stream)
        let path = PathBuf::from(track.id.as_str());
        if path.is_file() {
            api_play.play_file(path, Some(core_track));
            show_toast(&w_play, &rt_play, format!("Playing: {}", track.title));
        } else {
            let api_str = api_play.clone();
            let http_str = http_play.clone();
            let w_str = w_play.clone();
            let rt_str = rt_play.clone();
            let gen_str = gen_play.clone();
            let s_id = track.id.to_string();
            let s_title = track.title.to_string();
            let s_artist = track.artist.to_string();
            let s_platform = track.platform.to_string();
            let s_dur = track.duration_sec as f64;
            let ct_clone = core_track.clone();

            show_toast(&w_play, &rt_play, format!("Loading: {}...", track.title));

            rt_play.spawn(async move {
                // Handle local_* IDs stored in favorites/history/playlists
                if s_id.starts_with("local_") {
                    if let Ok(lib) = api_str.list_library().await {
                        if let Some(found) = lib.into_iter().find(|t| t.id == s_id) {
                            let p = PathBuf::from(&found.path);
                            if p.is_file() {
                                if gen_str.load(Ordering::SeqCst) == my_gen {
                                    api_str.play_file(p, Some(ct_clone));
                                    show_toast(&w_str, &rt_str, format!("Playing: {s_title}"));
                                }
                                return;
                            }
                        }
                    }
                }

                let q = riff_core::models::StreamQuery {
                    id: Some(s_id),
                    title: Some(s_title.clone()),
                    artist: Some(s_artist),
                    platform: Some(s_platform),
                    duration: Some(s_dur),
                    url: None,
                    thumbnail: None,
                    timeout: Some(20),
                    quality: Some("medium".to_string()),
                    q: None,
                };

                match api_str.resolve_stream(&q, 0).await {
                    Ok(resolved) => {
                        if gen_str.load(Ordering::SeqCst) != my_gen {
                            return;
                        }
                        let is_hls = resolved.get("isHls").and_then(|v| v.as_bool()).unwrap_or(false);
                        let raw_url = resolved
                            .get("rawStreamUrl")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                resolved
                                    .get("streamUrl")
                                    .and_then(|v| v.as_str())
                                    .filter(|u| !u.starts_with("http://127.0.0.1:0"))
                            });

                        if let Some(url) = raw_url {
                            match download_stream_bytes(&http_str, url, is_hls).await {
                                Ok((bytes, hint)) => {
                                    if gen_str.load(Ordering::SeqCst) == my_gen {
                                        api_str.play_bytes(bytes, hint, Some(ct_clone));
                                        show_toast(&w_str, &rt_str, format!("Playing: {s_title}"));
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!("Stream download failed for '{}': {}", s_title, err);
                                    show_toast(&w_str, &rt_str, format!("Playback error: {err}"));
                                }
                            }
                        } else {
                            tracing::warn!("No playable stream URL resolved for '{}'", s_title);
                            show_toast(&w_str, &rt_str, "Playback error: No stream URL found");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("resolve_stream failed for '{}': {}", s_title, err);
                        show_toast(&w_str, &rt_str, format!("Stream error: {err}"));
                    }
                }
            });
        }
    });

    // 3. Play / Pause Toggle Callback
    let api_toggle = api.clone();
    let w_toggle = window_weak.clone();
    main_window.on_toggle_play_pause(move || {
        let (playing, _, _, _) = api_toggle.get_playback_state();
        if playing {
            api_toggle.pause();
        } else if let Some(w) = w_toggle.upgrade() {
            if w.get_current_id().is_empty() {
                let first = w
                    .get_queue_tracks()
                    .iter()
                    .next()
                    .or_else(|| w.get_home_trending().iter().next())
                    .or_else(|| w.get_home_for_you().iter().next())
                    .or_else(|| w.get_history_tracks().iter().next())
                    .or_else(|| w.get_library_tracks().iter().next());
                if let Some(t) = first {
                    w.invoke_play_track(t);
                }
            } else {
                api_toggle.resume();
            }
        } else {
            api_toggle.resume();
        }
    });

    // 4. Play Previous & Next Callbacks
    let pl_prev = playlist_cache.clone();
    let cur_idx_prev = current_index.clone();
    let w_prev = window_weak.clone();

    main_window.on_play_prev(move || {
        let target = if let (Ok(pl), Ok(mut idx)) = (pl_prev.lock(), cur_idx_prev.lock()) {
            if pl.is_empty() {
                None
            } else {
                if *idx == 0 {
                    *idx = pl.len() - 1;
                } else {
                    *idx -= 1;
                }
                pl.get(*idx).cloned()
            }
        } else {
            None
        };
        if let Some(item) = target {
            if let Some(w) = w_prev.upgrade() {
                w.invoke_play_track(item.into_slint());
            }
        }
    });

    let pl_next = playlist_cache.clone();
    let cur_idx_next = current_index.clone();
    let shuf_next = shuffle_state.clone();
    let w_next = window_weak.clone();

    main_window.on_play_next(move || {
        let target = if let (Ok(pl), Ok(mut idx)) = (pl_next.lock(), cur_idx_next.lock()) {
            if pl.is_empty() {
                None
            } else {
                if shuf_next.load(Ordering::SeqCst) && pl.len() > 1 {
                    let nanos = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as usize)
                        .unwrap_or(1);
                    let mut next_i = nanos % pl.len();
                    if next_i == *idx {
                        next_i = (*idx + 1) % pl.len();
                    }
                    *idx = next_i;
                } else {
                    *idx = (*idx + 1) % pl.len();
                }
                pl.get(*idx).cloned()
            }
        } else {
            None
        };
        if let Some(item) = target {
            if let Some(w) = w_next.upgrade() {
                w.invoke_play_track(item.into_slint());
            }
        }
    });

    // 4b. Shuffle & Repeat Callbacks
    let shuf_cb = shuffle_state.clone();
    let w_shuf = window_weak.clone();
    main_window.on_toggle_shuffle(move || {
        let next = !shuf_cb.load(Ordering::SeqCst);
        shuf_cb.store(next, Ordering::SeqCst);
        if let Some(w) = w_shuf.upgrade() {
            w.set_shuffle_on(next);
        }
    });

    let rep_cb = repeat_state.clone();
    let w_rep = window_weak.clone();
    main_window.on_cycle_repeat(move || {
        let cur = rep_cb.load(Ordering::SeqCst);
        let next = (cur + 1) % 3;
        rep_cb.store(next, Ordering::SeqCst);
        if let Some(w) = w_rep.upgrade() {
            w.set_repeat_mode(next as i32);
        }
    });

    // 5. Seek Callback
    let api_seek = api.clone();
    main_window.on_seek_to(move |pos| {
        api_seek.seek(pos as f64);
    });

    // 6. Volume Callback
    let api_vol = api.clone();
    main_window.on_set_volume(move |vol| {
        api_vol.set_volume(vol);
    });

    // 7. Favorite Toggle for Current Track
    let api_fav = api.clone();
    let w_fav = window_weak.clone();
    let rt_fav = rt_handle.clone();

    main_window.on_toggle_favorite_current(move || {
        if let Some(w) = w_fav.upgrade() {
            let id = w.get_current_id().to_string();
            if id.is_empty() {
                return;
            }
            let track = riff_core::models::Track {
                id: id.clone(),
                path: id,
                file: w.get_current_title().to_string(),
                title: w.get_current_title().to_string(),
                artist: w.get_current_artist().to_string(),
                album: Some(w.get_current_album().to_string()),
                duration: w.get_current_duration() as f64,
                thumbnail: String::new(),
                added_at: 0,
                updated_at: 0,
            };

            let api_f = api_fav.clone();
            let w_f = w_fav.clone();

            rt_fav.spawn(async move {
                if let Ok(is_fav) = api_f.toggle_favorite(&track).await {
                    let favs = api_f.get_favorites().await.unwrap_or_default();
                    let fav_items: Vec<TrackItem> = favs
                        .iter()
                        .map(|t| TrackItem::from_core(t, "favorite", true))
                        .collect();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_f.upgrade() {
                            w.set_current_is_favorite(is_fav);
                            w.set_favorite_tracks(to_slint_model(fav_items));
                        }
                    });
                }
            });
        }
    });

    // 7b. Favorite Toggle for Any Track Row
    let api_fav_row = api.clone();
    let w_fav_row = window_weak.clone();
    let rt_fav_row = rt_handle.clone();
    main_window.on_toggle_favorite_track(move |td| {
        let core_t = track_data_to_core(&td);
        let tid = td.id.to_string();
        let api_f = api_fav_row.clone();
        let w_f = w_fav_row.clone();
        rt_fav_row.spawn(async move {
            if let Ok(is_fav) = api_f.toggle_favorite(&core_t).await {
                let favs = api_f.get_favorites().await.unwrap_or_default();
                let fav_ids: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                let fav_items: Vec<TrackItem> = favs
                    .iter()
                    .map(|t| TrackItem::from_core(t, "favorite", true))
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_f.upgrade() {
                        if w.get_current_id().as_str() == tid {
                            w.set_current_is_favorite(is_fav);
                        }
                        w.set_favorite_tracks(to_slint_model(fav_items));
                        let update_model = |m: ModelRc<TrackData>| {
                            let mut vec: Vec<TrackData> = m.iter().collect();
                            for item in &mut vec {
                                item.is_favorite = fav_ids.contains(item.id.as_str());
                            }
                            ModelRc::new(VecModel::from(vec))
                        };
                        w.set_library_tracks(update_model(w.get_library_tracks()));
                        w.set_search_results(update_model(w.get_search_results()));
                        w.set_history_tracks(update_model(w.get_history_tracks()));
                        w.set_queue_tracks(update_model(w.get_queue_tracks()));
                        w.set_active_playlist_tracks(update_model(w.get_active_playlist_tracks()));
                    }
                });
            }
        });
    });

    // 8. Search Callback
    let api_search = api.clone();
    let http_search = http.clone();
    let w_search = window_weak.clone();
    let pl_search = playlist_cache.clone();
    let rt_search = rt_handle.clone();

    main_window.on_trigger_search(move |query_str, plat_str| {
        let query = query_str.to_string();
        let plat = plat_str.to_string();
        let api_s = api_search.clone();
        let http_s = http_search.clone();
        let w_s = w_search.clone();
        let pl_s = pl_search.clone();

        rt_search.spawn(async move {
            let search_platform = if plat == "all" { "ytmusic" } else { &plat };
            if let Ok((tracks, _)) = api_s.search(&query, search_platform, 25, 0).await {
                let favs = api_s.get_favorites().await.unwrap_or_default();
                let fav_set: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();

                let items = json_tracks_to_items(tracks, &http_s, &fav_set, "ytmusic").await;

                if let Ok(mut pl) = pl_s.lock() {
                    *pl = items.clone();
                }

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_s.upgrade() {
                        w.set_search_results(to_slint_model(items));
                    }
                });
            }
        });
    });

    // 9. Scan Local Music Folder Callback
    let api_scan = api.clone();
    let w_scan = window_weak.clone();
    let pl_scan = playlist_cache.clone();
    let rt_scan = rt_handle.clone();

    main_window.on_scan_local_folder(move || {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(dir) = folder {
            let api_sc = api_scan.clone();
            let w_sc = w_scan.clone();
            let pl_sc = pl_scan.clone();

            rt_scan.spawn(async move {
                let _ = api_sc.scan_library(Some(&dir)).await;
                if let Ok(tracks) = api_sc.list_library().await {
                    let favs = api_sc.get_favorites().await.unwrap_or_default();
                    let fav_set: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();

                    let items: Vec<TrackItem> = tracks
                        .iter()
                        .map(|t| {
                            let mut item = TrackItem::from_core(t, "local", fav_set.contains(&t.id));
                            item.id = t.path.clone();
                            item
                        })
                        .collect();

                    if let Ok(mut pl) = pl_sc.lock() {
                        *pl = items.clone();
                    }

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_sc.upgrade() {
                            w.set_library_tracks(to_slint_model(items));
                            w.set_status_message(SharedString::from("Scan complete!"));
                        }
                    });
                }
            });
        }
    });

    // 9b. Add Local Audio Files Callback
    let api_addf = api.clone();
    let w_addf = window_weak.clone();
    let pl_addf = playlist_cache.clone();
    let rt_addf = rt_handle.clone();

    main_window.on_add_local_files(move || {
        let files = rfd::FileDialog::new()
            .add_filter("Audio", &["mp3", "flac", "wav", "ogg", "m4a", "aac", "opus"])
            .pick_files();
        if let Some(paths) = files {
            let api_af = api_addf.clone();
            let w_af = w_addf.clone();
            let pl_af = pl_addf.clone();

            rt_addf.spawn(async move {
                let add_items: Vec<riff_core::models::LibraryAddItem> = paths
                    .iter()
                    .map(|p| riff_core::models::LibraryAddItem {
                        id: None,
                        file: p.to_string_lossy().to_string(),
                        name: p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()),
                        ext: p.extension().and_then(|s| s.to_str()).map(|s| s.to_string()),
                    })
                    .collect();
                let _ = api_af.add_library_items(add_items).await;
                if let Ok(tracks) = api_af.list_library().await {
                    let favs = api_af.get_favorites().await.unwrap_or_default();
                    let fav_set: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();

                    let items: Vec<TrackItem> = tracks
                        .iter()
                        .map(|t| {
                            let mut item = TrackItem::from_core(t, "local", fav_set.contains(&t.id));
                            if !t.path.is_empty() {
                                item.id = t.path.clone();
                            }
                            item
                        })
                        .collect();

                    if let Ok(mut pl) = pl_af.lock() {
                        *pl = items.clone();
                    }

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_af.upgrade() {
                            w.set_library_tracks(to_slint_model(items));
                            w.set_status_message(SharedString::from("Files added!"));
                        }
                    });
                }
            });
        }
    });

    // 10. Equalizer, Speed & Audio FX Callbacks
    let api_eq = api.clone();
    main_window.on_set_equalizer(move |bass, mid, treble| {
        api_eq.set_eq(bass, mid, treble);
    });

    let api_spd = api.clone();
    main_window.on_set_audio_speed(move |spd| {
        api_spd.set_speed(spd);
    });

    let api_fx = api.clone();
    main_window.on_set_audio_effects(move |reverb, distortion| {
        api_fx.set_effects(reverb, distortion);
    });

    let api_rst = api.clone();
    let w_rst = window_weak.clone();
    main_window.on_reset_audio_settings(move || {
        api_rst.set_eq(0.0, 0.0, 0.0);
        api_rst.set_speed(1.0);
        api_rst.set_effects(0.0, 0.0);
        api_rst.set_volume(1.0);
        if let Some(w) = w_rst.upgrade() {
            w.set_eq_bass(0.0);
            w.set_eq_mid(0.0);
            w.set_eq_treble(0.0);
            w.set_audio_speed(1.0);
            w.set_audio_reverb(0.0);
            w.set_audio_distortion(0.0);
            w.set_eq_preset_name(SharedString::from("Default"));
        }
    });

    // 11. Audio Presets Callback (matches defaultPresets in state.js)
    let api_preset = api.clone();
    let w_preset = window_weak.clone();

    main_window.on_apply_eq_preset(move |preset_name| {
        let (bass, mid, treble, speed, reverb, distortion, vol) = match preset_name.as_str() {
            "p_default" | "flat" => (0.0f32, 0.0f32, 0.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "p_speedup" => (0.0f32, 0.0f32, 0.0f32, 1.35f32, 0.0f32, 0.0f32, 1.0f32),
            "p_speedup_pitch" => (0.0f32, 0.0f32, 0.0f32, 1.30f32, 0.0f32, 0.0f32, 1.0f32),
            "p_slowed" => (2.5f32, -1.0f32, -3.0f32, 0.82f32, 65.0f32, 0.0f32, 1.0f32),
            "p_nightcore" => (1.0f32, 1.0f32, 3.0f32, 1.25f32, 20.0f32, 0.0f32, 1.0f32),
            "p_distortion" => (5.0f32, 2.0f32, 4.0f32, 1.0f32, 0.0f32, 75.0f32, 1.0f32),
            "bass_boost" => (6.0f32, 0.0f32, -1.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "vocal" => (-2.0f32, 4.0f32, 2.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "rock" => (4.0f32, -1.0f32, 3.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "treble_boost" => (-1.0f32, 1.0f32, 5.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            _ => (0.0f32, 0.0f32, 0.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
        };
        api_preset.set_eq(bass, mid, treble);
        api_preset.set_speed(speed);
        api_preset.set_effects(reverb, distortion);
        let _ = vol;

        if let Some(w) = w_preset.upgrade() {
            w.set_eq_bass(bass);
            w.set_eq_mid(mid);
            w.set_eq_treble(treble);
            w.set_audio_speed(speed);
            w.set_audio_reverb(reverb);
            w.set_audio_distortion(distortion);
        }
    });

    // 11b. Sleep Timer Callbacks
    let sd_set = sleep_deadline.clone();
    let ste_set = sleep_track_end.clone();
    let w_sleep = window_weak.clone();
    main_window.on_set_sleep_minutes(move |mins| {
        if mins == 0 {
            if let Ok(mut d) = sd_set.lock() { *d = None; }
            ste_set.store(false, Ordering::SeqCst);
            if let Some(w) = w_sleep.upgrade() {
                w.set_sleep_active(false);
                w.set_sleep_status(SharedString::from("00:00"));
            }
        } else if mins < 0 {
            if let Ok(mut d) = sd_set.lock() { *d = None; }
            ste_set.store(true, Ordering::SeqCst);
            if let Some(w) = w_sleep.upgrade() {
                w.set_sleep_active(true);
                w.set_sleep_status(SharedString::from("Track end"));
            }
        } else {
            ste_set.store(false, Ordering::SeqCst);
            if let Ok(mut d) = sd_set.lock() {
                *d = Some(Instant::now() + Duration::from_secs((mins as u64) * 60));
            }
            if let Some(w) = w_sleep.upgrade() {
                w.set_sleep_active(true);
                w.set_sleep_status(SharedString::from(format!("{mins:02}:00")));
            }
        }
    });

    let sd_add = sleep_deadline.clone();
    let w_sleep_add = window_weak.clone();
    main_window.on_add_sleep_5m(move || {
        if let Ok(mut d) = sd_add.lock() {
            let base = d.unwrap_or_else(Instant::now);
            let next = base + Duration::from_secs(300);
            *d = Some(next);
            let rem = next.saturating_duration_since(Instant::now()).as_secs();
            if let Some(w) = w_sleep_add.upgrade() {
                w.set_sleep_active(true);
                w.set_sleep_status(SharedString::from(format!("{:02}:{:02}", rem / 60, rem % 60)));
            }
        }
    });

    // 11c. Custom Lyrics Editor Callbacks
    let lyr_open_ed = active_lyrics_cache.clone();
    let w_open_ed = window_weak.clone();
    main_window.on_open_custom_lyrics(move || {
        if let Some(w) = w_open_ed.upgrade() {
            let text = if let Ok(lyr) = lyr_open_ed.lock() {
                lyr.iter()
                    .map(|l| if l.is_gap { "[Instrumental]".to_string() } else { l.text.to_string() })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };
            w.set_custom_lyrics_text(SharedString::from(text));
            w.set_custom_lyrics_open(true);
        }
    });

    let api_save_ed = api.clone();
    let lyr_save_ed = active_lyrics_cache.clone();
    let w_save_ed = window_weak.clone();
    let rt_save_ed = rt_handle.clone();
    main_window.on_save_custom_lyrics(move |raw_text| {
        if let Some(w) = w_save_ed.upgrade() {
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            if title.is_empty() || w.get_current_id().is_empty() {
                w.set_custom_lyrics_open(false);
                return;
            }

            let new_lines: Vec<String> = raw_text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();

            let text_to_save = if let Ok(old) = lyr_save_ed.lock() {
                let has_sync = old.iter().any(|l| l.time_sec >= 0.0);
                if has_sync && !old.is_empty() && old.len() == new_lines.len() {
                    new_lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| format!("{}{}", format_lrc_tag(old[i].time_sec), line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    raw_text.trim().to_string()
                }
            } else {
                raw_text.trim().to_string()
            };

            let parsed = parse_lrc(&text_to_save, dur);
            if let Ok(mut store) = lyr_save_ed.lock() {
                *store = parsed.clone();
            }
            w.set_lyrics_lines(ModelRc::new(VecModel::from(parsed)));
            w.set_active_lyric_index(-1);
            w.set_custom_lyrics_open(false);

            let api_s = api_save_ed.clone();
            rt_save_ed.spawn(async move {
                let _ = api_s.save_custom_lyrics(&title, &artist, &text_to_save).await;
            });
        }
    });

    // 11d. Tap-to-Sync Lyrics Callbacks
    let lyr_sync_open = active_lyrics_cache.clone();
    let sync_cache_open = sync_lines_cache.clone();
    let api_sync_open = api.clone();
    let w_sync_open = window_weak.clone();
    main_window.on_open_sync_lyrics(move || {
        if let Some(w) = w_sync_open.upgrade() {
            if w.get_current_id().is_empty() {
                return;
            }
            let lines: Vec<SyncLineData> = if let Ok(lyr) = lyr_sync_open.lock() {
                lyr.iter()
                    .enumerate()
                    .map(|(i, l)| SyncLineData {
                        text: l.text.clone(),
                        time_sec: -1.0,
                        time_str: SharedString::from(""),
                        is_synced: false,
                        is_next: i == 0,
                        is_gap: l.is_gap,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if lines.is_empty() {
                w.invoke_open_custom_lyrics();
                return;
            }
            if let Ok(mut sc) = sync_cache_open.lock() {
                *sc = lines.clone();
            }
            w.set_sync_lines(ModelRc::new(VecModel::from(lines)));
            w.set_sync_synced_count(0);
            w.set_sync_lyrics_open(true);
            api_sync_open.seek(0.0);
            api_sync_open.resume();
        }
    });

    let sync_cache_tap = sync_lines_cache.clone();
    let api_sync_tap = api.clone();
    let w_sync_tap = window_weak.clone();
    main_window.on_tap_sync_line(move |idx| {
        let (_, pos_sec, _, _) = api_sync_tap.get_playback_state();
        let t = pos_sec as f32;
        if let Ok(mut sc) = sync_cache_tap.lock() {
            let i = idx as usize;
            if i < sc.len() {
                sc[i].time_sec = t;
                sc[i].time_str = SharedString::from(format_lrc_tag(t));
                sc[i].is_synced = true;
                let next_idx = ((i + 1)..sc.len()).find(|&j| !sc[j].is_synced);
                for (j, item) in sc.iter_mut().enumerate() {
                    item.is_next = Some(j) == next_idx;
                }
                let synced_cnt = sc.iter().filter(|l| l.is_synced).count() as i32;
                let cloned = sc.clone();
                if let Some(w) = w_sync_tap.upgrade() {
                    w.set_sync_lines(ModelRc::new(VecModel::from(cloned)));
                    w.set_sync_synced_count(synced_cnt);
                }
            }
        }
    });

    let sync_cache_next = sync_lines_cache.clone();
    let w_sync_next = window_weak.clone();
    main_window.on_tap_sync_next(move || {
        let next_idx = if let Ok(sc) = sync_cache_next.lock() {
            sc.iter().position(|l| l.is_next).or_else(|| sc.iter().position(|l| !l.is_synced))
        } else {
            None
        };
        if let Some(i) = next_idx {
            if let Some(w) = w_sync_next.upgrade() {
                w.invoke_tap_sync_line(i as i32);
            }
        }
    });

    let sync_cache_rst = sync_lines_cache.clone();
    let api_sync_rst = api.clone();
    let w_sync_rst = window_weak.clone();
    main_window.on_restart_sync_lyrics(move || {
        api_sync_rst.seek(0.0);
        if let Ok(mut sc) = sync_cache_rst.lock() {
            for (i, item) in sc.iter_mut().enumerate() {
                item.time_sec = -1.0;
                item.time_str = SharedString::from("");
                item.is_synced = false;
                item.is_next = i == 0;
            }
            let cloned = sc.clone();
            if let Some(w) = w_sync_rst.upgrade() {
                w.set_sync_lines(ModelRc::new(VecModel::from(cloned)));
                w.set_sync_synced_count(0);
            }
        }
    });

    let sync_cache_save = sync_lines_cache.clone();
    let lyr_sync_save = active_lyrics_cache.clone();
    let api_sync_save = api.clone();
    let w_sync_save = window_weak.clone();
    let rt_sync_save = rt_handle.clone();
    main_window.on_save_sync_lyrics(move || {
        if let Some(w) = w_sync_save.upgrade() {
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            let lrc_content = if let Ok(sc) = sync_cache_save.lock() {
                sc.iter()
                    .map(|l| {
                        let txt = if l.is_gap { "[Instrumental]" } else { l.text.as_str() };
                        if l.is_synced && l.time_sec >= 0.0 {
                            format!("{}{}", format_lrc_tag(l.time_sec), txt)
                        } else {
                            txt.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };

            let parsed = parse_lrc(&lrc_content, dur);
            if let Ok(mut store) = lyr_sync_save.lock() {
                *store = parsed.clone();
            }
            w.set_lyrics_lines(ModelRc::new(VecModel::from(parsed)));
            w.set_active_lyric_index(-1);
            w.set_sync_lyrics_open(false);

            let api_s = api_sync_save.clone();
            rt_sync_save.spawn(async move {
                let _ = api_s.save_custom_lyrics(&title, &artist, &lrc_content).await;
            });
        }
    });

    // 11e. Playlists & Queue Callbacks
    let api_cpl = api.clone();
    let w_cpl = window_weak.clone();
    let rt_cpl = rt_handle.clone();
    main_window.on_create_playlist(move |name| {
        let api_c = api_cpl.clone();
        let w_c = w_cpl.clone();
        let n = name.to_string();
        rt_cpl.spawn(async move {
            if let Ok(created) = api_c.create_playlist(&n).await {
                let all = api_c.get_playlists().await.unwrap_or_default();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_c.upgrade() {
                        w.set_playlists(playlists_to_slint(&all));
                        w.set_active_playlist_id(SharedString::from(&created.id));
                        w.set_active_playlist_name(SharedString::from(&created.name));
                        w.set_active_playlist_tracks(ModelRc::new(VecModel::from(Vec::new())));
                        w.set_active_tab(6);
                    }
                });
            }
        });
    });

    let api_opl = api.clone();
    let http_opl = http.clone();
    let w_opl = window_weak.clone();
    let rt_opl = rt_handle.clone();
    main_window.on_open_playlist(move |pid| {
        let api_o = api_opl.clone();
        let http_o = http_opl.clone();
        let w_o = w_opl.clone();
        let id = pid.to_string();
        rt_opl.spawn(async move {
            let all = api_o.get_playlists().await.unwrap_or_default();
            let favs = api_o.get_favorites().await.unwrap_or_default();
            let fav_ids: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
            if let Some(pl) = all.into_iter().find(|p| p.id == id) {
                let mut items: Vec<TrackItem> = pl
                    .tracks
                    .iter()
                    .map(|t| TrackItem::from_core(t, "playlist", fav_ids.contains(&t.id)))
                    .collect();
                for item in &mut items {
                    if !item.thumbnail.is_empty() {
                        item.cover_rgba = fetch_cover_rgba(&http_o, &item.thumbnail).await;
                    }
                }
                let pl_id = pl.id;
                let pl_name = pl.name;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_o.upgrade() {
                        w.set_active_playlist_id(SharedString::from(&pl_id));
                        w.set_active_playlist_name(SharedString::from(&pl_name));
                        w.set_active_playlist_tracks(to_slint_model(items));
                        w.set_active_tab(6);
                    }
                });
            }
        });
    });

    let api_dpl = api.clone();
    let w_dpl = window_weak.clone();
    let rt_dpl = rt_handle.clone();
    main_window.on_delete_playlist(move |pid| {
        let api_d = api_dpl.clone();
        let w_d = w_dpl.clone();
        let id = pid.to_string();
        rt_dpl.spawn(async move {
            let _ = api_d.delete_playlist(&id).await;
            let all = api_d.get_playlists().await.unwrap_or_default();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_d.upgrade() {
                    w.set_playlists(playlists_to_slint(&all));
                    w.set_active_tab(0);
                }
            });
        });
    });

    let pl_play_all = playlist_cache.clone();
    let cur_idx_all = current_index.clone();
    let shuf_all = shuffle_state.clone();
    let w_play_all = window_weak.clone();
    main_window.on_play_playlist_all(move |shuffle| {
        if let Some(w) = w_play_all.upgrade() {
            let tracks: Vec<TrackData> = w.get_active_playlist_tracks().iter().collect();
            if tracks.is_empty() {
                return;
            }
            shuf_all.store(shuffle, Ordering::SeqCst);
            w.set_shuffle_on(shuffle);
            let items: Vec<TrackItem> = tracks.iter().map(track_data_to_item).collect();
            if let Ok(mut pl) = pl_play_all.lock() {
                *pl = items.clone();
            }
            if let Ok(mut idx) = cur_idx_all.lock() {
                *idx = 0;
            }
            w.set_queue_tracks(to_slint_model(items));
            w.invoke_play_track(tracks[0].clone());
        }
    });

    let pend_pl = pending_playlist_track.clone();
    let w_req_pl = window_weak.clone();
    main_window.on_request_add_to_playlist(move |td| {
        if let Ok(mut p) = pend_pl.lock() {
            *p = Some(track_data_to_item(&td));
        }
        if let Some(w) = w_req_pl.upgrade() {
            w.set_pending_playlist_track_title(td.title);
            w.set_add_to_playlist_open(true);
        }
    });

    let pend_conf = pending_playlist_track.clone();
    let api_conf = api.clone();
    let w_conf = window_weak.clone();
    let rt_conf = rt_handle.clone();
    main_window.on_confirm_add_to_playlist(move |pid| {
        let item_opt = pend_conf.lock().ok().and_then(|mut g| g.take());
        if let Some(item) = item_opt {
            let core_t = riff_core::models::Track {
                id: item.id.clone(),
                path: item.id,
                file: item.title.clone(),
                title: item.title,
                artist: item.artist,
                album: if item.album.is_empty() { None } else { Some(item.album) },
                duration: item.duration_sec,
                thumbnail: item.thumbnail,
                added_at: 0,
                updated_at: 0,
            };
            let api_c = api_conf.clone();
            let w_c = w_conf.clone();
            let id = pid.to_string();
            rt_conf.spawn(async move {
                let _ = api_c.add_track_to_playlist(&id, &core_t).await;
                let all = api_c.get_playlists().await.unwrap_or_default();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_c.upgrade() {
                        w.set_playlists(playlists_to_slint(&all));
                    }
                });
            });
        }
    });

    let pl_clrq = playlist_cache.clone();
    let w_clrq = window_weak.clone();
    main_window.on_clear_queue(move || {
        if let Ok(mut pl) = pl_clrq.lock() {
            pl.clear();
        }
        if let Some(w) = w_clrq.upgrade() {
            w.set_queue_tracks(ModelRc::new(VecModel::from(Vec::new())));
        }
    });

    // 11f. Compact Mini Player Toggle
    let w_mini = window_weak.clone();
    main_window.on_toggle_mini_mode(move || {
        if let Some(w) = w_mini.upgrade() {
            let next = !w.get_mini_mode();
            w.set_mini_mode(next);
            if next {
                w.window().set_size(slint::LogicalSize::new(420.0, 520.0));
            } else {
                w.window().set_size(slint::LogicalSize::new(1100.0, 720.0));
            }
        }
    });

    // 12. Clear Play History Callback
    let api_clrhist = api.clone();
    let w_clrhist = window_weak.clone();
    let rt_clrhist = rt_handle.clone();

    main_window.on_clear_play_history(move || {
        let api_c = api_clrhist.clone();
        let w_c = w_clrhist.clone();
        rt_clrhist.spawn(async move {
            let _ = api_c.clear_history().await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_c.upgrade() {
                    w.set_history_tracks(ModelRc::new(VecModel::from(Vec::new())));
                }
            });
        });
    });

    // 13. Switch UI Mode Callback
    let api_mode = api.clone();
    let rt_mode = rt_handle.clone();
    main_window.on_switch_ui_mode(move |target_mode| {
        let api_m = api_mode.clone();
        let mode_str = target_mode.to_string();
        rt_mode.spawn(async move {
            let _ = api_m.set_ui_mode(&mode_str).await;
        });
    });

    // 13b. Frameless Window Controls
    let w_min = window_weak.clone();
    main_window.on_window_minimize(move || {
        if let Some(w) = w_min.upgrade() {
            w.window().set_minimized(true);
        }
    });

    let w_max = window_weak.clone();
    main_window.on_window_maximize(move || {
        if let Some(w) = w_max.upgrade() {
            let next = !w.window().is_maximized();
            w.window().set_maximized(next);
            w.set_is_maximized(next);
        }
    });

    main_window.on_window_close(move || {
        let _ = slint::quit_event_loop();
    });

    // 14. Sleep Timer Background Ticker (1s)
    let sd_tick = sleep_deadline.clone();
    let api_sleep_tick = api.clone();
    let w_sleep_tick = window_weak.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut expired = false;
        let mut rem_secs = None;
        if let Ok(mut d) = sd_tick.lock() {
            if let Some(deadline) = *d {
                let now = Instant::now();
                if now >= deadline {
                    *d = None;
                    expired = true;
                } else {
                    rem_secs = Some(deadline.saturating_duration_since(now).as_secs());
                }
            }
        }
        if expired {
            api_sleep_tick.pause();
            let w_c = w_sleep_tick.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_c.upgrade() {
                    w.set_sleep_active(false);
                    w.set_sleep_status(SharedString::from("00:00"));
                }
            });
        } else if let Some(rem) = rem_secs {
            let w_c = w_sleep_tick.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_c.upgrade() {
                    w.set_sleep_status(SharedString::from(format!("{:02}:{:02}", rem / 60, rem % 60)));
                }
            });
        }
    });

    // 15. Subscribe to EventBus
    let mut rx = events.subscribe();
    let w_events = window_weak.clone();
    let lyrics_cache_ev = active_lyrics_cache.clone();
    let rep_ev = repeat_state.clone();
    let ste_ev = sleep_track_end.clone();
    let eof_ev = eof_handled.clone();
    let api_ev = api.clone();

    std::thread::spawn(move || {
        while let Ok(event) = rx.blocking_recv() {
            let w_opt = w_events.clone();
            match event {
                riff_core::events::RiffEvent::PlaybackState {
                    is_playing,
                    position_sec,
                    duration_sec,
                } => {
                    let lyr_cache = lyrics_cache_ev.clone();
                    let rep_mode = rep_ev.load(Ordering::SeqCst);
                    let is_eof = !is_playing && duration_sec > 1.0 && position_sec >= duration_sec - 0.35;
                    let should_handle_eof = is_eof && !eof_ev.swap(true, Ordering::SeqCst);
                    let sleep_end = should_handle_eof && ste_ev.swap(false, Ordering::SeqCst);
                    let api_e = api_ev.clone();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_opt.upgrade() {
                            w.set_is_playing(is_playing);
                            w.set_current_pos(position_sec as f32);
                            if duration_sec > 0.0 {
                                w.set_current_duration(duration_sec as f32);
                                w.set_dur_text(SharedString::from(format_duration(duration_sec)));
                            }
                            w.set_pos_text(SharedString::from(format_duration(position_sec)));

                            if let Ok(mut lyr) = lyr_cache.lock() {
                                if !lyr.is_empty() {
                                    let mut active_idx = 0;
                                    let pos_f = position_sec as f32;
                                    for (i, line) in lyr.iter().enumerate() {
                                        if line.time_sec <= pos_f {
                                            active_idx = i;
                                        } else {
                                            break;
                                        }
                                    }
                                    let mut changed = false;
                                    for (i, line) in lyr.iter_mut().enumerate() {
                                        let should_be_active = i == active_idx;
                                        let should_be_past = i < active_idx;
                                        if line.is_active != should_be_active || line.is_past != should_be_past {
                                            line.is_active = should_be_active;
                                            line.is_past = should_be_past;
                                            changed = true;
                                        }
                                    }
                                    if changed {
                                        w.set_active_lyric_index(active_idx as i32);
                                        w.set_lyrics_lines(ModelRc::new(VecModel::from(lyr.clone())));
                                    }
                                }
                            }

                            if sleep_end {
                                w.set_sleep_active(false);
                                w.set_sleep_status(SharedString::from("00:00"));
                            } else if should_handle_eof {
                                if rep_mode == 2 {
                                    api_e.seek(0.0);
                                    api_e.resume();
                                } else if rep_mode == 1 {
                                    w.invoke_play_next();
                                }
                            }
                        }
                    });
                }
                riff_core::events::RiffEvent::Spectrum { bands } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_opt.upgrade() {
                            w.set_spectrum_bands(ModelRc::new(VecModel::from(bands)));
                        }
                    });
                }
                riff_core::events::RiffEvent::TrackChanged { track } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_opt.upgrade() {
                            if let Some(t) = track {
                                w.set_current_id(SharedString::from(&t.id));
                                w.set_current_title(SharedString::from(&t.title));
                                w.set_current_artist(SharedString::from(&t.artist));
                                if let Some(alb) = t.album {
                                    w.set_current_album(SharedString::from(&alb));
                                }
                            }
                        }
                    });
                }
                _ => {}
            }
        }
    });

    main_window.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lrc_synced() {
        let lrc_data = r#"
[00:04.50] Is this the real life?
[00:08.20] Is this just fantasy?
[00:12.80] Caught in a landslide
[00:15.10] No escape from reality
"#;
        let lines = parse_lrc(lrc_data, 180.0);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Is this the real life?");
        assert!((lines[0].time_sec - 4.50).abs() < 0.01);
        assert_eq!(lines[1].text, "Is this just fantasy?");
        assert!((lines[1].time_sec - 8.20).abs() < 0.01);
    }

    #[test]
    fn test_parse_lrc_plain_fallback() {
        let plain_text = "Line 1\nLine 2\nLine 3\nLine 4";
        let lines = parse_lrc(plain_text, 40.0);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Line 1");
        assert_eq!(lines[3].text, "Line 4");
    }

    #[test]
    fn test_detect_audio_hint() {
        assert_eq!(detect_audio_hint(Some("audio/mp4; codecs=\"mp4a.40.2\""), "https://example.com/x"), Some("m4a".to_string()));
        assert_eq!(detect_audio_hint(Some("audio/mpeg"), "https://example.com/x"), Some("mp3".to_string()));
        assert_eq!(detect_audio_hint(None, "https://rr1.googlevideo.com/videoplayback?mime=audio%2Fmp4"), Some("m4a".to_string()));
        assert_eq!(detect_audio_hint(None, "https://cf-media.sndcdn.com/track.mp3"), Some("mp3".to_string()));
    }

    #[test]
    #[ignore]
    fn test_live_stream_resolve_and_decode() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = Arc::new(riff_core::config::Config::default());
            let db = riff_core::db::open(&config.db_path).unwrap();
            let http = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Riff/0.7.6")
                .build()
                .unwrap();
            let soundcloud = riff_core::soundcloud::SoundCloudClient::new(http.clone());
            let youtube = Arc::new(riff_core::youtube::YouTubeClient::new(http.clone()));
            let lyrics = Arc::new(riff_core::lyrics::LyricsService::new(http.clone(), config.data_dir.clone()));
            let events = Arc::new(riff_core::events::EventBus::default());
            let player = Arc::new(riff_core::player::AudioPlayer::new(events.clone()));
            let api = riff_core::api::RiffApi::new(db, config, soundcloud, youtube, lyrics, events, player);

            let (tracks, _) = api.search("lofi hip hop", "ytmusic", 3, 0).await.unwrap();
            assert!(!tracks.is_empty(), "search returned no tracks");
            let first = &tracks[0];
            let q = riff_core::models::StreamQuery {
                id: first.get("id").and_then(|v| v.as_str()).map(String::from),
                title: first.get("title").and_then(|v| v.as_str()).map(String::from),
                artist: first.get("artist").and_then(|v| v.as_str()).map(String::from),
                platform: Some("ytmusic".to_string()),
                duration: first.get("duration").and_then(|v| v.as_f64()),
                url: None,
                thumbnail: None,
                timeout: Some(20),
                quality: Some("medium".to_string()),
                q: None,
            };
            let resolved = api.resolve_stream(&q, 0).await.expect("resolve_stream failed");
            let is_hls = resolved.get("isHls").and_then(|v| v.as_bool()).unwrap_or(false);
            let raw_url = resolved.get("rawStreamUrl").and_then(|v| v.as_str()).expect("missing rawStreamUrl");
            let (bytes, hint) = download_stream_bytes(&http, raw_url, is_hls).await.expect("download_stream_bytes failed");
            assert!(bytes.len() > 10_000, "downloaded audio too small: {}", bytes.len());
            let dec = riff_core::player::decoder::AudioDecoder::new(std::io::Cursor::new(bytes), hint.as_deref())
                .expect("AudioDecoder failed to decode stream");
            assert!(dec.sample_rate() > 0);
        });
    }
}
