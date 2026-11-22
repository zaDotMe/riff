use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

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
        .filter(|l| !l.is_empty() && !l.starts_with('[') && !l.starts_with('{'))
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
            }
        })
        .collect()
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

    // 1. Initial Data Load (Library, Favorites, History, Home Feed)
    let api_init = api.clone();
    let http_init = http.clone();
    let w_init = window_weak.clone();
    let pl_init = playlist_cache.clone();

    rt_handle.spawn(async move {
        let (lib_res, fav_res, hist_res) = tokio::join!(
            api_init.list_library(),
            api_init.get_favorites(),
            api_init.get_history(),
        );
        let lib_tracks = lib_res.unwrap_or_default();
        let fav_tracks = fav_res.unwrap_or_default();
        let hist_tracks = hist_res.unwrap_or_default();

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

        let w_first = w_init.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_first.upgrade() {
                w.set_library_tracks(to_slint_model(lib_items));
                w.set_favorite_tracks(to_slint_model(fav_items));
                w.set_history_tracks(to_slint_model(hist_items));
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
    let rt_play = rt_handle.clone();

    main_window.on_play_track(move |track| {
        let t_thumb = track.thumbnail.to_string();
        let has_cov = track.has_cover;

        if let Some(w) = w_play.upgrade() {
            w.set_current_id(track.id.clone());
            w.set_current_title(track.title.clone());
            w.set_current_artist(track.artist.clone());
            w.set_current_album(track.album.clone());
            w.set_current_platform(track.platform.clone());
            w.set_current_is_favorite(track.is_favorite);
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

        if let (Ok(pl), Ok(mut idx)) = (pl_play.lock(), cur_idx_play.lock()) {
            if let Some(pos) = pl.iter().position(|t| t.id == track.id.as_str()) {
                *idx = pos;
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
        } else {
            let api_str = api_play.clone();
            let http_str = http_play.clone();
            let s_id = track.id.to_string();
            let s_title = track.title.to_string();
            let s_artist = track.artist.to_string();
            let s_platform = track.platform.to_string();
            let s_dur = track.duration_sec as f64;
            let ct_clone = core_track.clone();

            rt_play.spawn(async move {
                let q = riff_core::models::StreamQuery {
                    id: Some(s_id),
                    title: Some(s_title),
                    artist: Some(s_artist),
                    platform: Some(s_platform),
                    duration: Some(s_dur),
                    url: None,
                    thumbnail: None,
                    timeout: Some(15),
                    quality: Some("medium".to_string()),
                    q: None,
                };

                if let Ok(resolved) = api_str.resolve_stream(&q, 0).await {
                    if let Some(stream_url) = resolved.get("streamUrl").and_then(|v| v.as_str()) {
                        if let Ok(resp) = http_str.get(stream_url).send().await {
                            if let Ok(bytes) = resp.bytes().await {
                                api_str.play_bytes(bytes.to_vec(), Some("mp3".to_string()), Some(ct_clone));
                            }
                        }
                    }
                }
            });
        }
    });

    // 3. Play / Pause Toggle Callback
    let api_toggle = api.clone();
    main_window.on_toggle_play_pause(move || {
        let (playing, _, _, _) = api_toggle.get_playback_state();
        if playing {
            api_toggle.pause();
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
    let w_next = window_weak.clone();

    main_window.on_play_next(move || {
        let target = if let (Ok(pl), Ok(mut idx)) = (pl_next.lock(), cur_idx_next.lock()) {
            if pl.is_empty() {
                None
            } else {
                *idx = (*idx + 1) % pl.len();
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

    // 10. Equalizer Control Callback
    let api_eq = api.clone();
    main_window.on_set_equalizer(move |bass, mid, treble| {
        api_eq.set_eq(bass, mid, treble);
    });

    // 11. Equalizer Presets Callback
    let api_preset = api.clone();
    let w_preset = window_weak.clone();

    main_window.on_apply_eq_preset(move |preset_name| {
        let (bass, mid, treble) = match preset_name.as_str() {
            "p_default" | "flat" => (0.0f32, 0.0f32, 0.0f32),
            "p_speedup" | "p_speedup_pitch" => (0.0f32, 0.0f32, 0.0f32),
            "p_slowed" => (2.5f32, -1.0f32, -3.0f32),
            "p_nightcore" => (1.0f32, 1.0f32, 3.0f32),
            "p_distortion" => (5.0f32, 2.0f32, 4.0f32),
            "bass_boost" => (6.0f32, 0.0f32, -1.0f32),
            "vocal" => (-2.0f32, 4.0f32, 2.0f32),
            "rock" => (4.0f32, -1.0f32, 3.0f32),
            "treble_boost" => (-1.0f32, 1.0f32, 5.0f32),
            _ => (0.0f32, 0.0f32, 0.0f32),
        };
        api_preset.set_eq(bass, mid, treble);

        if let Some(w) = w_preset.upgrade() {
            w.set_eq_bass(bass);
            w.set_eq_mid(mid);
            w.set_eq_treble(treble);
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

    // 14. Subscribe to EventBus
    let mut rx = events.subscribe();
    let w_events = window_weak.clone();
    let lyrics_cache_ev = active_lyrics_cache.clone();

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
                                        if line.is_active != should_be_active {
                                            line.is_active = should_be_active;
                                            changed = true;
                                        }
                                    }
                                    if changed {
                                        w.set_lyrics_lines(ModelRc::new(VecModel::from(lyr.clone())));
                                    }
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
}
