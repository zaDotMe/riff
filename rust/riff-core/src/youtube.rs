use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::debug;

const VISITOR_CACHE_TTL: Duration = Duration::from_secs(25 * 60);
const WEB_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const VISIONOS_PLAYER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const ANDROID_MUSIC_UA: &str = "com.google.android.apps.youtube.music/7.27.52 (Linux; U; Android 14; en_US) gzip";
const IOS_UA: &str = "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct YouTubeTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub thumbnail: String,
    pub url: String,
    pub platform: String,
    #[serde(rename = "isCached")]
    pub is_cached: bool,
    #[serde(rename = "isExplicit")]
    pub is_explicit: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct YouTubePlaylist {
    pub id: String,
    pub title: String,
    pub author: String,
    #[serde(rename = "trackCountText")]
    pub track_count_text: String,
    pub thumbnail: String,
    pub platform: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct YouTubeArtist {
    pub id: String,
    pub name: String,
    pub subtitle: String,
    pub thumbnail: String,
    pub platform: String,
}

#[derive(Clone)]
pub struct YouTubeClient {
    client: Client,
    visitor_cache: Arc<RwLock<(String, Instant)>>,
}

impl YouTubeClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            visitor_cache: Arc::new(RwLock::new((String::new(), Instant::now()))),
        }
    }

    /// Searches YouTube for tracks using InnerTube WEB client
    pub async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<YouTubeTrack>> {
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB",
                    "clientVersion": "2.20240101.00.00",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "query": query
        });

        let resp = self
            .client
            .post("https://www.youtube.com/youtubei/v1/search?prettyPrint=false")
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .json(&body)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("InnerTube search failed with HTTP status {}", resp.status());
        }

        let json_val: Value = resp.json().await?;
        let mut tracks = Vec::new();

        // Extract primary contents sections
        let sections = json_val
            .pointer("/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents")
            .or_else(|| json_val.pointer("/contents/sectionListRenderer/contents"))
            .and_then(|v| v.as_array());

        if let Some(sections) = sections {
            'search_loop: for section in sections {
                let items = section
                    .pointer("/itemSectionRenderer/contents")
                    .and_then(|v| v.as_array());

                if let Some(items) = items {
                    for item in items {
                        if tracks.len() >= limit {
                            break 'search_loop;
                        }

                        if let Some(v) = item.get("videoRenderer") {
                            if let Some(video_id) = v.get("videoId").and_then(|s| s.as_str()) {
                                let title = v
                                    .pointer("/title/runs/0/text")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("untitled")
                                    .to_string();

                                let artist = v
                                    .pointer("/ownerText/runs/0/text")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("Unknown Artist")
                                    .replace(" - Topic", "");

                                let dur_str = v
                                    .pointer("/lengthText/simpleText")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("0:00");
                                let duration = parse_duration_seconds(dur_str);

                                let thumbnail = v
                                    .pointer("/thumbnail/thumbnails")
                                    .and_then(|t| t.as_array())
                                    .and_then(|arr| arr.last())
                                    .and_then(|t| t.get("url"))
                                    .and_then(|u| u.as_str())
                                    .unwrap_or_default()
                                    .to_string();

                                tracks.push(YouTubeTrack {
                                    id: video_id.to_string(),
                                    title,
                                    artist,
                                    duration,
                                    thumbnail,
                                    url: format!("https://www.youtube.com/watch?v={video_id}"),
                                    platform: "youtube".to_string(),
                                    is_cached: false,
                                    is_explicit: false,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(tracks)
    }

    /// Searches YouTube Music songs via InnerTube WEB_REMIX client (clean title/artist metadata)
    pub async fn search_music(&self, query: &str, limit: usize) -> anyhow::Result<Vec<YouTubeTrack>> {
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": "1.20250310.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "query": query,
            "params": "EgWKAQIIAWoKEAkQBRAKEAMQBA=="
        });

        let resp = self
            .client
            .post("https://music.youtube.com/youtubei/v1/search?prettyPrint=false")
            .header("Content-Type", "application/json")
            .header("Origin", "https://music.youtube.com")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
            )
            .json(&body)
            .timeout(std::time::Duration::from_secs(8))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("YouTube Music search failed with HTTP status {}", resp.status());
        }

        let json_val: Value = resp.json().await?;
        let mut tracks = Vec::new();
        collect_music_items(&json_val, limit, &mut tracks);
        Ok(tracks)
    }

    async fn post_ytmusic(&self, endpoint: &str, body: Value) -> anyhow::Result<Value> {
        let resp = self
            .client
            .post(format!("https://music.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false"))
            .header("Content-Type", "application/json")
            .header("Origin", "https://music.youtube.com")
            .header("User-Agent", WEB_UA)
            .json(&body)
            .timeout(Duration::from_secs(9))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("YouTube Music {endpoint} failed with HTTP {}", resp.status());
        }
        Ok(resp.json().await?)
    }

    /// Searches YouTube Music playlists via InnerTube WEB_REMIX
    pub async fn search_music_playlists(&self, query: &str, limit: usize) -> anyhow::Result<Vec<YouTubePlaylist>> {
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": "1.20250310.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "query": query,
            "params": "EgWKAQIoAWoMEA4QChADEAQQCRAF"
        });
        let json_val = self.post_ytmusic("search", body).await?;
        let mut playlists = Vec::new();
        collect_music_playlists(&json_val, limit, &mut playlists);
        Ok(playlists)
    }

    /// Searches YouTube Music artists via InnerTube WEB_REMIX
    pub async fn search_music_artists(&self, query: &str, limit: usize) -> anyhow::Result<Vec<YouTubeArtist>> {
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": "1.20250310.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "query": query,
            "params": "EgWKAQIgAWoMEA4QChADEAQQCRAF"
        });
        let json_val = self.post_ytmusic("search", body).await?;
        let mut artists = Vec::new();
        collect_music_artists(&json_val, limit, &mut artists);
        Ok(artists)
    }

    /// Fetches a YouTube Music playlist (title, cover, tracks) via InnerTube browse
    pub async fn browse_playlist(&self, browse_id: &str, limit: usize) -> anyhow::Result<(String, String, Vec<YouTubeTrack>)> {
        let normalized_id = if browse_id.starts_with("VL") {
            browse_id.to_string()
        } else {
            format!("VL{browse_id}")
        };
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": "1.20250310.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "browseId": normalized_id
        });
        let json_val = self.post_ytmusic("browse", body).await?;
        let title = json_val
            .pointer("/header/musicDetailHeaderRenderer/title/runs/0/text")
            .or_else(|| json_val.pointer("/header/musicResponsiveHeaderRenderer/title/runs/0/text"))
            .and_then(|v| v.as_str())
            .unwrap_or("Playlist")
            .to_string();
        let cover = json_val
            .pointer("/header/musicDetailHeaderRenderer/thumbnail/croppedSquareThumbnailRenderer/thumbnail/thumbnails")
            .or_else(|| json_val.pointer("/header/musicResponsiveHeaderRenderer/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.last())
            .and_then(|t| t.get("url"))
            .and_then(|u| u.as_str())
            .unwrap_or_default()
            .to_string();

        let mut tracks = Vec::new();
        collect_music_items(&json_val, limit, &mut tracks);
        Ok((title, cover, tracks))
    }

    /// Fetches YouTube Music Up Next / Radio recommendations for a seed video ID
    pub async fn get_radio_recommendations(&self, video_id: &str, limit: usize) -> anyhow::Result<Vec<YouTubeTrack>> {
        let vid = extract_video_id(video_id);
        if vid.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({
            "context": {
                "client": {
                    "clientName": "WEB_REMIX",
                    "clientVersion": "1.20250310.01.00",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "videoId": vid,
            "playlistId": format!("RDAMVM{vid}"),
            "isAudioOnly": true
        });
        let json_val = self.post_ytmusic("next", body).await?;
        let mut tracks = Vec::new();
        collect_panel_videos(&json_val, limit, &mut tracks);
        tracks.retain(|t| t.id != vid && (t.duration == 0.0 || t.duration <= 900.0));
        Ok(tracks)
    }

    async fn get_visitor_data(&self, video_id: &str, force_refresh: bool) -> Option<String> {
        if !force_refresh {
            let cache = self.visitor_cache.read().await;
            if !cache.0.is_empty() && cache.1.elapsed() < VISITOR_CACHE_TTL {
                return Some(cache.0.clone());
            }
        }

        let watch_url = format!("https://www.youtube.com/watch?v={video_id}");
        let resp = self
            .client
            .get(&watch_url)
            .header("User-Agent", WEB_UA)
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            return None;
        }
        let html = resp.text().await.ok()?;
        let vd = extract_visitor_data_from_html(&html)?;

        let mut cache = self.visitor_cache.write().await;
        *cache = (vd.clone(), Instant::now());
        Some(vd)
    }

    async fn verify_stream_url(&self, stream_url: &str, dl_ua: &str, content_len: Option<u64>) -> bool {
        // GoogleVideo can serve chunk 0 (bytes 0..1048575) with HTTP 206 and then reject
        // subsequent chunks (bytes >= 1048576) with HTTP 403 when poToken is missing.
        let probe_range = if content_len.map(|len| len > 1_048_576).unwrap_or(true) {
            "bytes=1048576-1048576"
        } else {
            "bytes=0-0"
        };

        let resp = self
            .client
            .get(stream_url)
            .header("User-Agent", dl_ua)
            .header("Range", probe_range)
            .timeout(Duration::from_secs(8))
            .send()
            .await;

        match resp {
            Ok(r) => {
                let status = r.status().as_u16();
                status == 200 || status == 206
            }
            Err(_) => false,
        }
    }

    /// Resolves direct audio stream URL via InnerTube (VISIONOS with visitorData, then ANDROID_MUSIC / IOS fallback)
    pub async fn resolve_stream(&self, video_id: &str, quality: &str) -> anyhow::Result<String> {
        let vid = extract_video_id(video_id);
        if vid.is_empty() {
            anyhow::bail!("Invalid YouTube video id: {video_id}");
        }

        // 1. Try VISIONOS client with watch-page visitorData (reliable full-track streaming without poToken)
        for force_refresh in [false, true] {
            if let Some(visitor_data) = self.get_visitor_data(vid, force_refresh).await {
                let visionos_body = json!({
                    "context": {
                        "client": {
                            "clientName": "VISIONOS",
                            "clientVersion": "0.1",
                            "deviceMake": "Apple",
                            "deviceModel": "RealityDevice14,1",
                            "osName": "visionOS",
                            "osVersion": "2.0",
                            "hl": "en",
                            "gl": "US",
                            "visitorData": visitor_data
                        }
                    },
                    "videoId": vid
                });

                match self
                    .query_player(
                        "https://www.youtube.com/youtubei/v1/player",
                        &visionos_body,
                        quality,
                        VISIONOS_PLAYER_UA,
                        Some(("101", "0.1", Some(visitor_data.as_str()))),
                    )
                    .await
                {
                    Ok((url, content_len)) => {
                        if self.verify_stream_url(&url, WEB_UA, content_len).await {
                            return Ok(url);
                        }
                        debug!("VISIONOS stream URL failed chunk probe for {vid}");
                    }
                    Err(e) => debug!("VISIONOS player failed for {vid} (refresh={force_refresh}): {e}"),
                }
            } else if force_refresh {
                break;
            }
        }

        // 2. Try ANDROID_MUSIC client
        let android_body = json!({
            "context": {
                "client": {
                    "clientName": "ANDROID_MUSIC",
                    "clientVersion": "7.27.52",
                    "androidSdkVersion": 34,
                    "hl": "en",
                    "gl": "US"
                }
            },
            "videoId": vid
        });

        match self
            .query_player(
                "https://music.youtube.com/youtubei/v1/player",
                &android_body,
                quality,
                ANDROID_MUSIC_UA,
                Some(("21", "7.27.52", None)),
            )
            .await
        {
            Ok((url, content_len)) => {
                if self.verify_stream_url(&url, ANDROID_MUSIC_UA, content_len).await {
                    return Ok(url);
                }
                debug!("ANDROID_MUSIC stream URL failed chunk probe (403 cutoff) for {vid}");
            }
            Err(e) => debug!("ANDROID_MUSIC player failed for {vid}: {e}"),
        }

        // 3. Try IOS client as fallback (verify past 1MB cutoff so 403-poisoned URLs fall through to yt-dlp / Smart Mirror)
        let ios_body = json!({
            "context": {
                "client": {
                    "clientName": "IOS",
                    "clientVersion": "20.10.4",
                    "deviceMake": "Apple",
                    "deviceModel": "iPhone16,2",
                    "osName": "iOS",
                    "osVersion": "18.3.2.22D60",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "videoId": vid
        });

        match self
            .query_player(
                "https://www.youtube.com/youtubei/v1/player",
                &ios_body,
                quality,
                IOS_UA,
                Some(("5", "20.10.4", None)),
            )
            .await
        {
            Ok((url, content_len)) => {
                if self.verify_stream_url(&url, IOS_UA, content_len).await {
                    return Ok(url);
                }
                debug!("IOS stream URL failed chunk probe (403 cutoff) for {vid}");
            }
            Err(e) => debug!("IOS player failed for {vid}: {e}"),
        }

        anyhow::bail!("Failed to resolve playable audio stream for YouTube ID: {vid}")
    }

    async fn query_player(
        &self,
        endpoint: &str,
        body: &Value,
        quality: &str,
        user_agent: &str,
        client_headers: Option<(&str, &str, Option<&str>)>,
    ) -> anyhow::Result<(String, Option<u64>)> {
        let mut req = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("User-Agent", user_agent);

        if let Some((client_name, client_version, visitor_id)) = client_headers {
            req = req
                .header("X-YouTube-Client-Name", client_name)
                .header("X-YouTube-Client-Version", client_version);
            if let Some(vd) = visitor_id {
                if !vd.is_empty() {
                    req = req.header("X-Goog-Visitor-Id", vd);
                }
            }
        }

        let resp = req
            .json(body)
            .timeout(Duration::from_secs(8))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("InnerTube player returned HTTP {}", resp.status());
        }

        let json_val: Value = resp.json().await?;

        if let Some(status) = json_val.pointer("/playabilityStatus/status").and_then(|s| s.as_str()) {
            if status != "OK" {
                anyhow::bail!("Video unplayable: {status}");
            }
        }

        let formats = json_val
            .pointer("/streamingData/adaptiveFormats")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut audio_formats: Vec<&Value> = formats
            .iter()
            .filter(|f| {
                let mime = f.get("mimeType").and_then(|s| s.as_str()).unwrap_or_default();
                let has_url = f.get("url").and_then(|s| s.as_str()).is_some();
                let has_cipher = f.get("signatureCipher").is_some() || f.get("cipher").is_some();
                mime.starts_with("audio/") && has_url && !has_cipher
            })
            .collect();

        if audio_formats.is_empty() {
            anyhow::bail!("No direct unenciphered audio formats found in player response");
        }

        // Sort formats by bitrate depending on requested quality, preferring audio/mp4 (AAC)
        // so both WebView2 and native Symphonia decoder can play without Opus dependency.
        if quality == "low" {
            audio_formats.sort_by_key(|f| {
                let mime = f.get("mimeType").and_then(|s| s.as_str()).unwrap_or_default();
                let is_mp4 = if mime.starts_with("audio/mp4") { 0i64 } else { 1i64 };
                let br = f.get("bitrate").and_then(|b| b.as_i64()).unwrap_or(0);
                (is_mp4, br)
            });
        } else {
            audio_formats.sort_by_key(|f| {
                let mime = f.get("mimeType").and_then(|s| s.as_str()).unwrap_or_default();
                let is_mp4 = if mime.starts_with("audio/mp4") { 0i64 } else { 1i64 };
                let br = f.get("bitrate").and_then(|b| b.as_i64()).unwrap_or(0);
                (is_mp4, std::cmp::Reverse(br))
            });
        }

        if let Some(best) = audio_formats.first() {
            if let Some(url) = best.get("url").and_then(|u| u.as_str()) {
                let content_len = best
                    .get("contentLength")
                    .and_then(|v| {
                        v.as_str()
                            .and_then(|s| s.parse::<u64>().ok())
                            .or_else(|| v.as_u64())
                    });
                return Ok((url.to_string(), content_len));
            }
        }

        anyhow::bail!("No valid audio URL found in streaming formats")
    }
}

fn collect_music_items(node: &Value, limit: usize, out: &mut Vec<YouTubeTrack>) {
    if out.len() >= limit {
        return;
    }
    match node {
        Value::Object(map) => {
            if let Some(item) = map.get("musicResponsiveListItemRenderer") {
                if let Some(track) = parse_music_item(item) {
                    if !out.iter().any(|t| t.id == track.id) {
                        out.push(track);
                    }
                }
                return;
            }
            for v in map.values() {
                collect_music_items(v, limit, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_music_items(v, limit, out);
            }
        }
        _ => {}
    }
}

fn parse_music_item(item: &Value) -> Option<YouTubeTrack> {
    let video_id = item
        .pointer("/playlistItemData/videoId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            item.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId")
                .and_then(|v| v.as_str())
        })?
        .to_string();

    let columns = item.get("flexColumns")?.as_array()?;
    let runs_of = |idx: usize| -> Vec<String> {
        columns
            .get(idx)
            .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs"))
            .and_then(|r| r.as_array())
            .map(|runs| {
                runs.iter()
                    .filter_map(|r| r.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };

    let title = runs_of(0).join("");
    if title.trim().is_empty() {
        return None;
    }

    let meta: Vec<String> = runs_of(1);
    let segments: Vec<&String> = meta.iter().filter(|s| s.trim() != "•").collect();
    let duration = segments
        .last()
        .map(|s| s.trim())
        .filter(|s| s.contains(':') && s.chars().all(|c| c.is_ascii_digit() || c == ':'))
        .map(parse_duration_seconds)
        .unwrap_or(0.0);

    let artist = segments
        .first()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !(s.contains(':') && s.chars().all(|c| c.is_ascii_digit() || c == ':')))
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let thumbnail = item
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    let is_explicit = item
        .get("badges")
        .and_then(|b| b.as_array())
        .map(|badges| {
            badges.iter().any(|b| {
                b.pointer("/musicInlineBadgeRenderer/icon/iconType")
                    .and_then(|i| i.as_str())
                    == Some("MUSIC_EXPLICIT_BADGE")
            })
        })
        .unwrap_or(false);

    Some(YouTubeTrack {
        url: format!("https://music.youtube.com/watch?v={video_id}"),
        id: video_id,
        title: title.trim().to_string(),
        artist,
        duration,
        thumbnail,
        platform: "ytmusic".to_string(),
        is_cached: false,
        is_explicit,
    })
}

fn collect_music_playlists(val: &Value, limit: usize, out: &mut Vec<YouTubePlaylist>) {
    if out.len() >= limit {
        return;
    }
    match val {
        Value::Object(map) => {
            if let Some(renderer) = map.get("musicResponsiveListItemRenderer") {
                if let Some(pl) = parse_music_playlist_item(renderer) {
                    if !out.iter().any(|p| p.id == pl.id) {
                        out.push(pl);
                        if out.len() >= limit {
                            return;
                        }
                    }
                }
            }
            for (_, v) in map {
                collect_music_playlists(v, limit, out);
                if out.len() >= limit {
                    return;
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_music_playlists(item, limit, out);
                if out.len() >= limit {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn parse_music_playlist_item(item: &Value) -> Option<YouTubePlaylist> {
    let browse_id = item
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .or_else(|| item.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchPlaylistEndpoint/playlistId"))
        .and_then(|v| v.as_str())?
        .to_string();
    if !browse_id.starts_with("VL") && !browse_id.starts_with("PL") && !browse_id.starts_with("RD") {
        return None;
    }

    let columns = item.get("flexColumns")?.as_array()?;
    let runs_of = |idx: usize| -> Vec<String> {
        columns
            .get(idx)
            .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs"))
            .and_then(|r| r.as_array())
            .map(|runs| {
                runs.iter()
                    .filter_map(|r| r.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };

    let title = runs_of(0).join("");
    if title.trim().is_empty() {
        return None;
    }

    let meta = runs_of(1);
    let segments: Vec<String> = meta
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "•" && !s.eq_ignore_ascii_case("Playlist"))
        .collect();
    let author = segments.first().cloned().unwrap_or_else(|| "YouTube Music".to_string());
    let track_count_text = segments
        .iter()
        .find(|s| s.to_ascii_lowercase().contains("song") || s.to_ascii_lowercase().contains("track") || s.to_ascii_lowercase().contains("view"))
        .cloned()
        .unwrap_or_default();

    let thumbnail = item
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    Some(YouTubePlaylist {
        id: browse_id,
        title: title.trim().to_string(),
        author,
        track_count_text,
        thumbnail,
        platform: "ytmusic".to_string(),
    })
}

fn collect_music_artists(val: &Value, limit: usize, out: &mut Vec<YouTubeArtist>) {
    if out.len() >= limit {
        return;
    }
    match val {
        Value::Object(map) => {
            if let Some(renderer) = map.get("musicResponsiveListItemRenderer") {
                if let Some(art) = parse_music_artist_item(renderer) {
                    if !out.iter().any(|a| a.id == art.id) {
                        out.push(art);
                        if out.len() >= limit {
                            return;
                        }
                    }
                }
            }
            for (_, v) in map {
                collect_music_artists(v, limit, out);
                if out.len() >= limit {
                    return;
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_music_artists(item, limit, out);
                if out.len() >= limit {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn parse_music_artist_item(item: &Value) -> Option<YouTubeArtist> {
    let browse_id = item
        .pointer("/navigationEndpoint/browseEndpoint/browseId")
        .and_then(|v| v.as_str())?
        .to_string();
    if !browse_id.starts_with("UC") {
        return None;
    }

    let columns = item.get("flexColumns")?.as_array()?;
    let runs_of = |idx: usize| -> Vec<String> {
        columns
            .get(idx)
            .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs"))
            .and_then(|r| r.as_array())
            .map(|runs| {
                runs.iter()
                    .filter_map(|r| r.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };

    let name = runs_of(0).join("");
    if name.trim().is_empty() {
        return None;
    }

    let subtitle = runs_of(1)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "•")
        .collect::<Vec<_>>()
        .join(" • ");

    let thumbnail = item
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    Some(YouTubeArtist {
        id: browse_id,
        name: name.trim().to_string(),
        subtitle: if subtitle.is_empty() { "Artist".to_string() } else { subtitle },
        thumbnail,
        platform: "ytmusic".to_string(),
    })
}

fn collect_panel_videos(val: &Value, limit: usize, out: &mut Vec<YouTubeTrack>) {
    if out.len() >= limit {
        return;
    }
    match val {
        Value::Object(map) => {
            if let Some(renderer) = map.get("playlistPanelVideoRenderer") {
                if let Some(track) = parse_panel_video_item(renderer) {
                    if !out.iter().any(|t| t.id == track.id) {
                        out.push(track);
                        if out.len() >= limit {
                            return;
                        }
                    }
                }
            }
            for (_, v) in map {
                collect_panel_videos(v, limit, out);
                if out.len() >= limit {
                    return;
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_panel_videos(item, limit, out);
                if out.len() >= limit {
                    return;
                }
            }
        }
        _ => {}
    }
}

fn parse_panel_video_item(item: &Value) -> Option<YouTubeTrack> {
    let video_id = item.get("videoId").and_then(|v| v.as_str())?.to_string();
    let title = item
        .pointer("/title/runs")
        .and_then(|r| r.as_array())
        .map(|runs| runs.iter().filter_map(|r| r.get("text").and_then(|t| t.as_str())).collect::<String>())
        .unwrap_or_default();
    if title.trim().is_empty() {
        return None;
    }
    let artist = item
        .pointer("/longBylineText/runs/0/text")
        .or_else(|| item.pointer("/shortBylineText/runs/0/text"))
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown Artist")
        .trim()
        .to_string();
    let dur_text = item
        .pointer("/lengthText/runs/0/text")
        .and_then(|v| v.as_str())
        .unwrap_or("0:00");
    let duration = parse_duration_seconds(dur_text);
    let thumbnail = item
        .pointer("/thumbnail/thumbnails")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or_default()
        .to_string();

    Some(YouTubeTrack {
        url: format!("https://music.youtube.com/watch?v={video_id}"),
        id: video_id,
        title: title.trim().to_string(),
        artist,
        duration,
        thumbnail,
        platform: "ytmusic".to_string(),
        is_cached: false,
        is_explicit: false,
    })
}

fn extract_video_id(input: &str) -> &str {
    let s = input.trim();
    if s.len() == 11 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return s;
    }
    if let Some(pos) = s.find("v=") {
        let sub = &s[pos + 2..];
        let end = sub.find('&').unwrap_or(sub.len());
        let cand = &sub[..end];
        if cand.len() == 11 {
            return cand;
        }
    }
    if let Some(pos) = s.find("youtu.be/") {
        let sub = &s[pos + 9..];
        let end = sub.find('?').or_else(|| sub.find('&')).unwrap_or(sub.len());
        let cand = &sub[..end];
        if cand.len() == 11 {
            return cand;
        }
    }
    s
}

fn parse_duration_seconds(input: &str) -> f64 {
    let parts: Vec<&str> = input.split(':').collect();
    let mut total = 0.0;
    for part in parts {
        if let Ok(num) = part.trim().parse::<f64>() {
            total = total * 60.0 + num;
        }
    }
    total
}

fn extract_visitor_data_from_html(html: &str) -> Option<String> {
    for marker in ["\"VISITOR_DATA\":\"", "\"visitorData\":\""] {
        if let Some(pos) = html.find(marker) {
            let rest = &html[pos + marker.len()..];
            if let Some(end) = rest.find('"') {
                let raw = &rest[..end];
                if !raw.is_empty() {
                    return Some(percent_decode_ascii(raw));
                }
            }
        }
    }
    None
}

fn percent_decode_ascii(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_decodes_visitor_data() {
        let html = r#"{"VISITOR_DATA":"CgtUcGNCRENJOElfUSj%3D%3D","other":1}"#;
        assert_eq!(
            extract_visitor_data_from_html(html).as_deref(),
            Some("CgtUcGNCRENJOElfUSj==")
        );

        let html2 = r#"{"visitorData":"Cgt1UXJWVFgwRFBLSSg%3D"}"#;
        assert_eq!(
            extract_visitor_data_from_html(html2).as_deref(),
            Some("Cgt1UXJWVFgwRFBLSSg=")
        );
    }
}
