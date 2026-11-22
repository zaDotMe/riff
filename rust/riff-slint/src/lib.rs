use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

slint::include_modules!();

fn format_duration(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    let m = s / 60;
    let rem = s % 60;
    format!("{m}:{rem:02}")
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

    // 1. Initial Library Load
    let api_clone = api.clone();
    let w_init = window_weak.clone();
    rt.spawn(async move {
        if let Ok(tracks) = api_clone.list_library().await {
            let model_items: Vec<TrackData> = tracks
                .into_iter()
                .map(|t| TrackData {
                    id: SharedString::from(&t.id),
                    title: SharedString::from(&t.title),
                    artist: SharedString::from(&t.artist),
                    duration_str: SharedString::from(format_duration(t.duration)),
                    duration_sec: t.duration as f32,
                    platform: SharedString::from("local"),
                })
                .collect();

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_init.upgrade() {
                    let rc: ModelRc<TrackData> = ModelRc::new(VecModel::from(model_items));
                    w.set_library_tracks(rc);
                }
            });
        }
    });

    // 2. Play Track Callback
    let api_play = api.clone();
    main_window.on_play_track(move |track_data| {
        let path = PathBuf::from(track_data.id.as_str());
        // If local track or file path exists
        if path.is_file() {
            api_play.play_file(path, None);
        } else {
            // Online stream resolution via YouTube / SoundCloud
            let api_res = api_play.clone();
            let id = track_data.id.to_string();
            let title = track_data.title.to_string();
            let artist = track_data.artist.to_string();
            let platform = track_data.platform.to_string();
            let duration = track_data.duration_sec as f64;

            std::thread::spawn(move || {
                let rt_stream = tokio::runtime::Runtime::new().unwrap();
                rt_stream.block_on(async move {
                    let q = riff_core::models::StreamQuery {
                        id: Some(id),
                        title: Some(title),
                        artist: Some(artist),
                        platform: Some(platform),
                        duration: Some(duration),
                        url: None,
                        thumbnail: None,
                        timeout: Some(15),
                        quality: Some("medium".to_string()),
                        q: None,
                    };
                    if let Ok(resolved) = api_res.resolve_stream(&q, 0).await {
                        if let Some(stream_url) = resolved.get("streamUrl").and_then(|v| v.as_str()) {
                            tracing::info!("Slint playing stream URL: {}", stream_url);
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

    // 4. Seek Callback
    let api_seek = api.clone();
    main_window.on_seek_to(move |pos| {
        api_seek.seek(pos as f64);
    });

    // 5. Volume Callback
    let api_vol = api.clone();
    main_window.on_set_volume(move |vol| {
        api_vol.set_volume(vol);
    });

    // 6. Search Callback
    let api_search = api.clone();
    let w_search = window_weak.clone();
    main_window.on_trigger_search(move |query_str| {
        let query = query_str.to_string();
        let api_s = api_search.clone();
        let w_s = w_search.clone();

        std::thread::spawn(move || {
            let rt_s = tokio::runtime::Runtime::new().unwrap();
            rt_s.block_on(async move {
                if let Ok((tracks, _)) = api_s.search(&query, "ytmusic", 20, 0).await {
                    let results: Vec<TrackData> = tracks
                        .into_iter()
                        .map(|v| {
                            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("Untitled").to_string();
                            let artist = v.get("artist").and_then(|x| x.as_str()).unwrap_or("Unknown").to_string();
                            let dur_f = v.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0);
                            let plat = v.get("platform").and_then(|x| x.as_str()).unwrap_or("ytmusic").to_string();

                            TrackData {
                                id: SharedString::from(&id),
                                title: SharedString::from(&title),
                                artist: SharedString::from(&artist),
                                duration_str: SharedString::from(format_duration(dur_f)),
                                duration_sec: dur_f as f32,
                                platform: SharedString::from(&plat),
                            }
                        })
                        .collect();

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_s.upgrade() {
                            let rc: ModelRc<TrackData> = ModelRc::new(VecModel::from(results));
                            w.set_search_results(rc);
                        }
                    });
                }
            });
        });
    });

    // 7. Scan Local Music Folder Callback
    let api_scan = api.clone();
    let w_scan = window_weak.clone();
    main_window.on_scan_local_folder(move || {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(dir) = folder {
            let api_sc = api_scan.clone();
            let w_sc = w_scan.clone();

            std::thread::spawn(move || {
                let rt_sc = tokio::runtime::Runtime::new().unwrap();
                rt_sc.block_on(async move {
                    let _ = api_sc.scan_library(Some(&dir)).await;
                    if let Ok(tracks) = api_sc.list_library().await {
                        let items: Vec<TrackData> = tracks
                            .into_iter()
                            .map(|t| TrackData {
                                id: SharedString::from(&t.path),
                                title: SharedString::from(&t.title),
                                artist: SharedString::from(&t.artist),
                                duration_str: SharedString::from(format_duration(t.duration)),
                                duration_sec: t.duration as f32,
                                platform: SharedString::from("local"),
                            })
                            .collect();

                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = w_sc.upgrade() {
                                let rc: ModelRc<TrackData> = ModelRc::new(VecModel::from(items));
                                w.set_library_tracks(rc);
                                w.set_status_message(SharedString::from("Scan complete!"));
                            }
                        });
                    }
                });
            });
        }
    });

    // 8. Switch UI Mode Callback
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

        // Inform user to restart or auto restart
        let _ = slint::invoke_from_event_loop(|| {
            // Set message and prepare switch
        });
    });

    // 9. Subscribe to Riff Core EventBus
    let mut rx = events.subscribe();
    let w_events = window_weak.clone();
    std::thread::spawn(move || {
        while let Ok(event) = rx.blocking_recv() {
            let w_opt = w_events.clone();
            match event {
                riff_core::events::RiffEvent::PlaybackState {
                    is_playing,
                    position_sec,
                    duration_sec,
                } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_opt.upgrade() {
                            w.set_is_playing(is_playing);
                            w.set_current_pos(position_sec as f32);
                            if duration_sec > 0.0 {
                                w.set_current_duration(duration_sec as f32);
                                w.set_dur_text(SharedString::from(format_duration(duration_sec)));
                            }
                            w.set_pos_text(SharedString::from(format_duration(position_sec)));
                        }
                    });
                }
                riff_core::events::RiffEvent::Spectrum { bands } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_opt.upgrade() {
                            let rc: ModelRc<f32> = ModelRc::new(VecModel::from(bands));
                            w.set_spectrum_bands(rc);
                        }
                    });
                }
                riff_core::events::RiffEvent::TrackChanged { track } => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_opt.upgrade() {
                            if let Some(t) = track {
                                w.set_current_title(SharedString::from(&t.title));
                                w.set_current_artist(SharedString::from(&t.artist));
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
