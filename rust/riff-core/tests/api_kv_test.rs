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
