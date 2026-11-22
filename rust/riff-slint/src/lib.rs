use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

slint::include_modules!();

fn format_duration(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    let m = s / 60;
    let rem = s % 60;
    format!("{m}:{rem:02}")
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

    // Fallback: evenly distribute plain lyrics lines across track duration
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

pub fn run() -> Result<(), slint::PlatformError> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    // Initialize Riff Core Config & Services
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

    // Cache of active lyrics and track list for prev/next
    let active_lyrics_cache: Arc<Mutex<Vec<LyricLineData>>> = Arc::new(Mutex::new(Vec::new()));
    let playlist_cache: Arc<Mutex<Vec<TrackData>>> = Arc::new(Mutex::new(Vec::new()));
    let current_index: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    // 1. Initial Data Load (Library, Favorites, History)
    let api_init = api.clone();
    let w_init = window_weak.clone();
    let pl_init = playlist_cache.clone();

    rt.spawn(async move {
        // Load library
        let lib_tracks = api_init.list_library().await.unwrap_or_default();
        let fav_tracks = api_init.get_favorites().await.unwrap_or_default();
        let hist_tracks = api_init.get_history().await.unwrap_or_default();

        let fav_ids: std::collections::HashSet<String> = fav_tracks.iter().map(|t| t.id.clone()).collect();

        let lib_data: Vec<TrackData> = lib_tracks
            .iter()
            .map(|t| TrackData {
                id: SharedString::from(&t.id),
                title: SharedString::from(&t.title),
                artist: SharedString::from(&t.artist),
                album: SharedString::from(t.album.as_deref().unwrap_or("")),
                duration_str: SharedString::from(format_duration(t.duration)),
                duration_sec: t.duration as f32,
                platform: SharedString::from("local"),
                thumbnail: SharedString::from(&t.thumbnail),
                is_favorite: fav_ids.contains(&t.id),
            })
            .collect();

        let fav_data: Vec<TrackData> = fav_tracks
            .iter()
            .map(|t| TrackData {
                id: SharedString::from(&t.id),
                title: SharedString::from(&t.title),
                artist: SharedString::from(&t.artist),
                album: SharedString::from(t.album.as_deref().unwrap_or("")),
                duration_str: SharedString::from(format_duration(t.duration)),
                duration_sec: t.duration as f32,
                platform: SharedString::from("favorite"),
                thumbnail: SharedString::from(&t.thumbnail),
                is_favorite: true,
            })
            .collect();

        let hist_data: Vec<TrackData> = hist_tracks
            .iter()
            .map(|t| TrackData {
                id: SharedString::from(&t.id),
                title: SharedString::from(&t.title),
                artist: SharedString::from(&t.artist),
                album: SharedString::from(t.album.as_deref().unwrap_or("")),
                duration_str: SharedString::from(format_duration(t.duration)),
                duration_sec: t.duration as f32,
                platform: SharedString::from("history"),
                thumbnail: SharedString::from(&t.thumbnail),
                is_favorite: fav_ids.contains(&t.id),
            })
            .collect();

        if let Ok(mut pl) = pl_init.lock() {
            *pl = lib_data.clone();
        }

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_init.upgrade() {
                w.set_library_tracks(ModelRc::new(VecModel::from(lib_data)));
                w.set_favorite_tracks(ModelRc::new(VecModel::from(fav_data)));
                w.set_history_tracks(ModelRc::new(VecModel::from(hist_data)));
            }
        });
    });

    // 2. Play Track Callback
    let api_play = api.clone();
    let w_play = window_weak.clone();
    let lyr_play = active_lyrics_cache.clone();
    let pl_play = playlist_cache.clone();
    let cur_idx_play = current_index.clone();

    main_window.on_play_track(move |track| {
        let t_id = track.id.to_string();
        let t_title = track.title.to_string();
        let t_artist = track.artist.to_string();
        let t_album = track.album.to_string();
        let t_plat = track.platform.to_string();
        let is_fav = track.is_favorite;

        let w_opt = w_play.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_opt.upgrade() {
                w.set_current_id(SharedString::from(&t_id));
                w.set_current_title(SharedString::from(&t_title));
                w.set_current_artist(SharedString::from(&t_artist));
                w.set_current_album(SharedString::from(&t_album));
                w.set_current_platform(SharedString::from(&t_plat));
                w.set_current_is_favorite(is_fav);
            }
        });

        if let (Ok(pl), Ok(mut idx)) = (pl_play.lock(), cur_idx_play.lock()) {
            if let Some(pos) = pl.iter().position(|t| t.id == track.id) {
                *idx = pos;
            }
        }

        // Record into history
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

        std::thread::spawn(move || {
            let rt_h = tokio::runtime::Runtime::new().unwrap();
            rt_h.block_on(async move {
                let _ = api_hist.push_history(&track_for_hist).await;
                if let Ok(updated_hist) = api_hist.get_history().await {
                    let favs = api_hist.get_favorites().await.unwrap_or_default();
                    let fav_set: std::collections::HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                    let h_data: Vec<TrackData> = updated_hist
                        .into_iter()
                        .map(|t| TrackData {
                            id: SharedString::from(&t.id),
                            title: SharedString::from(&t.title),
                            artist: SharedString::from(&t.artist),
                            album: SharedString::from(t.album.as_deref().unwrap_or("")),
                            duration_str: SharedString::from(format_duration(t.duration)),
                            duration_sec: t.duration as f32,
                            platform: SharedString::from("history"),
                            thumbnail: SharedString::from(&t.thumbnail),
                            is_favorite: fav_set.contains(&t.id),
                        })
                        .collect();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_hist.upgrade() {
                            w.set_history_tracks(ModelRc::new(VecModel::from(h_data)));
                        }
                    });
                }
            });
        });

        // Fetch Synced Lyrics
        let api_lyr = api_play.clone();
        let lyr_store = lyr_play.clone();
        let w_lyr = w_play.clone();
        let q_title = track.title.to_string();
        let q_artist = track.artist.to_string();
        let q_dur = track.duration_sec as f64;

        std::thread::spawn(move || {
            let rt_l = tokio::runtime::Runtime::new().unwrap();
            rt_l.block_on(async move {
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
        });

        // Play audio (local file or stream)
        let path = PathBuf::from(track.id.as_str());
        if path.is_file() {
            api_play.play_file(path, Some(core_track));
        } else {
            // Online stream resolution
            let api_str = api_play.clone();
            let s_id = track.id.to_string();
            let s_title = track.title.to_string();
            let s_artist = track.artist.to_string();
            let s_platform = track.platform.to_string();
            let s_dur = track.duration_sec as f64;
            let ct_clone = core_track.clone();

            std::thread::spawn(move || {
                let rt_s = tokio::runtime::Runtime::new().unwrap();
                rt_s.block_on(async move {
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
                            if let Ok(resp) = reqwest::get(stream_url).await {
                                if let Ok(bytes) = resp.bytes().await {
                                    api_str.play_bytes(bytes.to_vec(), Some("mp3".to_string()), Some(ct_clone));
                                }
                            }
                        }
                    }
                });
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
        if let (Ok(pl), Ok(mut idx)) = (pl_prev.lock(), cur_idx_prev.lock()) {
            if !pl.is_empty() {
                if *idx == 0 {
                    *idx = pl.len() - 1;
                } else {
                    *idx -= 1;
                }
                if let Some(target) = pl.get(*idx) {
                    if let Some(w) = w_prev.upgrade() {
                        w.invoke_play_track(target.clone());
                    }
                }
            }
        }
    });

    let pl_next = playlist_cache.clone();
    let cur_idx_next = current_index.clone();
    let w_next = window_weak.clone();

    main_window.on_play_next(move || {
        if let (Ok(pl), Ok(mut idx)) = (pl_next.lock(), cur_idx_next.lock()) {
            if !pl.is_empty() {
                *idx = (*idx + 1) % pl.len();
                if let Some(target) = pl.get(*idx) {
                    if let Some(w) = w_next.upgrade() {
                        w.invoke_play_track(target.clone());
                    }
                }
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

            std::thread::spawn(move || {
                let rt_f = tokio::runtime::Runtime::new().unwrap();
                rt_f.block_on(async move {
                    if let Ok(is_fav) = api_f.toggle_favorite(&track).await {
                        let favs = api_f.get_favorites().await.unwrap_or_default();
                        let fav_items: Vec<TrackData> = favs
                            .into_iter()
                            .map(|t| TrackData {
                                id: SharedString::from(&t.id),
                                title: SharedString::from(&t.title),
                                artist: SharedString::from(&t.artist),
                                album: SharedString::from(t.album.as_deref().unwrap_or("")),
                                duration_str: SharedString::from(format_duration(t.duration)),
                                duration_sec: t.duration as f32,
                                platform: SharedString::from("favorite"),
                                thumbnail: SharedString::from(&t.thumbnail),
                                is_favorite: true,
                            })
                            .collect();

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_f.upgrade() {
                                w.set_current_is_favorite(is_fav);
                                w.set_favorite_tracks(ModelRc::new(VecModel::from(fav_items)));
                            }
                        });
                    }
                });
            });
        }
    });

    // 8. Search Callback
    let api_search = api.clone();
    let w_search = window_weak.clone();
    let pl_search = playlist_cache.clone();

    main_window.on_trigger_search(move |query_str, plat_str| {
        let query = query_str.to_string();
        let plat = plat_str.to_string();
        let api_s = api_search.clone();
        let w_s = w_search.clone();
        let pl_s = pl_search.clone();

        std::thread::spawn(move || {
            let rt_s = tokio::runtime::Runtime::new().unwrap();
            rt_s.block_on(async move {
                let search_platform = if plat == "all" { "ytmusic" } else { &plat };
                if let Ok((tracks, _)) = api_s.search(&query, search_platform, 25, 0).await {
                    let favs = api_s.get_favorites().await.unwrap_or_default();
                    let fav_set: std::collections::HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();

                    let results: Vec<TrackData> = tracks
                        .into_iter()
                        .map(|v| {
                            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("Untitled").to_string();
                            let artist = v.get("artist").and_then(|x| x.as_str()).unwrap_or("Unknown").to_string();
                            let album = v.get("album").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let dur_f = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
                            let p = v.get("platform").and_then(|x| x.as_str()).unwrap_or("ytmusic").to_string();
                            let thumb = v.get("thumbnail").and_then(|x| x.as_str()).unwrap_or("").to_string();

                            TrackData {
                                id: SharedString::from(&id),
                                title: SharedString::from(&title),
                                artist: SharedString::from(&artist),
                                album: SharedString::from(&album),
                                duration_str: SharedString::from(format_duration(dur_f)),
                                duration_sec: dur_f as f32,
                                platform: SharedString::from(&p),
                                thumbnail: SharedString::from(&thumb),
                                is_favorite: fav_set.contains(&id),
                            }
                        })
                        .collect();

                    if let Ok(mut pl) = pl_s.lock() {
                        *pl = results.clone();
                    }

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_s.upgrade() {
                            w.set_search_results(ModelRc::new(VecModel::from(results)));
                        }
                    });
                }
            });
        });
    });

    // 9. Scan Local Music Folder Callback
    let api_scan = api.clone();
    let w_scan = window_weak.clone();
    let pl_scan = playlist_cache.clone();

    main_window.on_scan_local_folder(move || {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(dir) = folder {
            let api_sc = api_scan.clone();
            let w_sc = w_scan.clone();
            let pl_sc = pl_scan.clone();

            std::thread::spawn(move || {
                let rt_sc = tokio::runtime::Runtime::new().unwrap();
                rt_sc.block_on(async move {
                    let _ = api_sc.scan_library(Some(&dir)).await;
                    if let Ok(tracks) = api_sc.list_library().await {
                        let favs = api_sc.get_favorites().await.unwrap_or_default();
                        let fav_set: std::collections::HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();

                        let items: Vec<TrackData> = tracks
                            .into_iter()
                            .map(|t| TrackData {
                                id: SharedString::from(&t.path),
                                title: SharedString::from(&t.title),
                                artist: SharedString::from(&t.artist),
                                album: SharedString::from(t.album.as_deref().unwrap_or("")),
                                duration_str: SharedString::from(format_duration(t.duration)),
                                duration_sec: t.duration as f32,
                                platform: SharedString::from("local"),
                                thumbnail: SharedString::from(&t.thumbnail),
                                is_favorite: fav_set.contains(&t.id),
                            })
                            .collect();

                        if let Ok(mut pl) = pl_sc.lock() {
                            *pl = items.clone();
                        }

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_sc.upgrade() {
                                w.set_library_tracks(ModelRc::new(VecModel::from(items)));
                                w.set_status_message(SharedString::from("Scan complete!"));
                            }
                        });
                    }
                });
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
            "bass_boost" => (6.0f32, 0.0f32, -1.0f32),
            "vocal" => (-2.0f32, 4.0f32, 2.0f32),
            "rock" => (4.0f32, -1.0f32, 3.0f32),
            "treble_boost" => (-1.0f32, 1.0f32, 5.0f32),
            _ => (0.0f32, 0.0f32, 0.0f32), // Flat
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

    main_window.on_clear_play_history(move || {
        let api_c = api_clrhist.clone();
        let w_c = w_clrhist.clone();
        std::thread::spawn(move || {
            let rt_c = tokio::runtime::Runtime::new().unwrap();
            rt_c.block_on(async move {
                let _ = api_c.clear_history().await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_c.upgrade() {
                        w.set_history_tracks(ModelRc::new(VecModel::from(Vec::new())));
                    }
                });
            });
        });
    });

    // 13. Switch UI Mode Callback
    let api_mode = api.clone();
    main_window.on_switch_ui_mode(move |target_mode| {
        let api_m = api_mode.clone();
        let mode_str = target_mode.to_string();
        std::thread::spawn(move || {
            let rt_m = tokio::runtime::Runtime::new().unwrap();
            rt_m.block_on(async move {
                let _ = api_m.set_ui_mode(&mode_str).await;
            });
        });
    });

    // 14. Subscribe to EventBus (Real-time PlaybackState, Spectrum FFT, Lyrics highlighting)
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

                            // Real-time Lyrics Line Highlight Sync
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
