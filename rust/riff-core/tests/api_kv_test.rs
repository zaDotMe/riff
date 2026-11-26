use std::sync::Arc;
use riff_core::api::RiffApi;
use riff_core::config::Config;
use riff_core::db;
use riff_core::events::EventBus;
use riff_core::lyrics::LyricsService;
use riff_core::models::Track;
use riff_core::player::AudioPlayer;
use riff_core::soundcloud::SoundCloudClient;
use riff_core::youtube::YouTubeClient;

#[tokio::test]
async fn test_favorites_and_history() {
    let tmp_path = std::env::temp_dir().join(format!("riff_test_{}.sqlite3", uuid::Uuid::new_v4()));
    let db = db::open(&tmp_path).unwrap();

    let config = Arc::new(Config::default());
    let http = reqwest::Client::new();
    let sc = SoundCloudClient::new(http.clone());
    let yt = Arc::new(YouTubeClient::new(http.clone()));
    let lyr = Arc::new(LyricsService::new(http.clone(), std::env::temp_dir()));
    let ev = Arc::new(EventBus::default());
    let pl = Arc::new(AudioPlayer::new(ev.clone()));

    let api = RiffApi::new(db, config, sc, yt, lyr, ev, pl);

    let sample_track = Track {
        id: "test_track_1".to_string(),
        path: "/path/to/song.mp3".to_string(),
        file: "song.mp3".to_string(),
        title: "Test Song".to_string(),
        artist: "Test Artist".to_string(),
        album: Some("Test Album".to_string()),
        duration: 180.0,
        thumbnail: "".to_string(),
        added_at: 1000,
        updated_at: 1000,
    };

    // Test history
    api.push_history(&sample_track).await.unwrap();
    let hist = api.get_history().await.unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].title, "Test Song");

    // Test favorite toggle
    let is_fav = api.toggle_favorite(&sample_track).await.unwrap();
    assert!(is_fav, "First toggle should add to favorites");

    let favs = api.get_favorites().await.unwrap();
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0].id, "test_track_1");

    let is_fav_2 = api.toggle_favorite(&sample_track).await.unwrap();
    assert!(!is_fav_2, "Second toggle should remove from favorites");

    let favs_after = api.get_favorites().await.unwrap();
    assert!(favs_after.is_empty());

    // Test clear history
    api.clear_history().await.unwrap();
    let hist_after = api.get_history().await.unwrap();
    assert!(hist_after.is_empty());
}

#[tokio::test]
async fn test_scan_subfolder_and_serve_track() {
    let base = std::env::temp_dir().join(format!("riff_scan_test_{}", uuid::Uuid::new_v4()));
    let sub = base.join("nested").join("album");
    std::fs::create_dir_all(&sub).unwrap();
    let song_path = sub.join("Artist Name - Nested Song.mp3");
    let dummy_bytes = b"ID3\x03\x00\x00\x00\x00\x00\x00\xff\xfb\x90\x00TESTAUDIO".to_vec();
    std::fs::write(&song_path, &dummy_bytes).unwrap();

    let cfg = Config::new(
        0,
        Some("test-token".to_string()),
        Some(base.join("data")),
        Some(base.join("library")),
        None,
        None,
    )
    .unwrap();

    let (port, handle) = riff_core::start_server_in_process(cfg).await.unwrap();
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}");

    // 1. Scan external directory containing nested subfolder
    let scan_resp = client
        .post(format!("{url}/api/scan"))
        .header("x-riff-token", "test-token")
        .json(&serde_json::json!({ "dir": base.to_string_lossy().to_string() }))
        .send()
        .await
        .unwrap();
    assert_eq!(scan_resp.status(), 200);

    // 2. Fetch library list
    let lib_json: serde_json::Value = client
        .get(format!("{url}/api/library"))
        .header("x-riff-token", "test-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = lib_json[0]["id"].as_str().unwrap().to_string();

    // 3. Serve nested subfolder track via /api/library-file (previously failed with HTTP 500)
    let file_resp = client
        .get(format!("{url}/api/library-file?id={id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(file_resp.status(), 200);
    let served = file_resp.bytes().await.unwrap();
    assert_eq!(served.as_ref(), dummy_bytes.as_slice());

    // 4. Batch removal via { ids: [...] } (previously failed with HTTP 422)
    let rm_resp = client
        .post(format!("{url}/api/library-remove"))
        .header("x-riff-token", "test-token")
        .json(&serde_json::json!({ "ids": [id] }))
        .send()
        .await
        .unwrap();
    assert_eq!(rm_resp.status(), 200);

    handle.abort();
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
#[ignore]
async fn test_live_ux_search_playlist_artist_and_autoplay() {
    let base = std::env::temp_dir().join(format!("riff_live_ux_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base).unwrap();
    let db = db::open(&base.join("test.sqlite3")).unwrap();
    let cfg = Arc::new(
        Config::new(0, None, Some(base.clone()), Some(base.join("lib")), None, None).unwrap(),
    );
    let http = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Riff/0.7.8")
        .build()
        .unwrap();
    let sc = SoundCloudClient::new(http.clone());
    let yt = Arc::new(YouTubeClient::new(http.clone()));
    let lyr = Arc::new(LyricsService::new(http.clone(), base.clone()));
    let ev = Arc::new(EventBus::default());
    let pl = Arc::new(AudioPlayer::new(ev.clone()));
    let api = RiffApi::new(db, cfg, sc, yt, lyr, ev, pl);

    let pls = api.search_playlists("chill lofi", "ytmusic", 5).await.unwrap();
    assert!(!pls.is_empty(), "search_playlists returned empty");
    let pl_id = pls[0]["id"].as_str().unwrap();
    let (pl_name, _cov, pl_tracks) = api.fetch_remote_playlist(pl_id, "ytmusic").await.unwrap();
    assert!(!pl_name.is_empty() && !pl_tracks.is_empty(), "remote playlist empty");

    let arts = api.search_artists("The Weeknd", "ytmusic", 5).await.unwrap();
    assert!(!arts.is_empty(), "search_artists returned empty");
    let (aname, _asub, _aava, atracks) = api
        .fetch_artist_profile("The Weeknd", arts[0]["id"].as_str(), "ytmusic")
        .await
        .unwrap();
    assert_eq!(aname, "The Weeknd");
    assert!(!atracks.is_empty(), "artist top tracks empty");

    let recs = api
        .get_recommendations("J7p4bzqLvCw", "Blinding Lights", "The Weeknd", "ytmusic", "similar", &[], 6)
        .await
        .unwrap();
    assert!(!recs.is_empty(), "autoplay recommendations empty");

    let _ = std::fs::remove_dir_all(&base);
}
