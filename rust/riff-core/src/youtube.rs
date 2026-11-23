use reqwest::Client;
use serde_json::{json, Value};
use tracing::debug;

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

#[derive(Clone)]
pub struct YouTubeClient {
    client: Client,
}

impl YouTubeClient {
    pub fn new(client: Client) -> Self {
        Self { client }
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

    /// Resolves direct audio stream URL via InnerTube ANDROID_MUSIC or IOS player
    pub async fn resolve_stream(&self, video_id: &str, quality: &str) -> anyhow::Result<String> {
        let vid = extract_video_id(video_id);
        if vid.is_empty() {
            anyhow::bail!("Invalid YouTube video id: {video_id}");
        }

        // 1. Try ANDROID_MUSIC client
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

        match self.query_player("https://music.youtube.com/youtubei/v1/player", &android_body, quality).await {
            Ok(url) => return Ok(url),
            Err(e) => debug!("ANDROID_MUSIC player failed for {vid}: {e}"),
        }

        // 2. Try IOS client as fallback
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

        match self.query_player("https://www.youtube.com/youtubei/v1/player", &ios_body, quality).await {
            Ok(url) => return Ok(url),
            Err(e) => debug!("IOS player failed for {vid}: {e}"),
        }

        anyhow::bail!("Failed to resolve direct audio stream for YouTube ID: {vid}")
    }

    async fn query_player(&self, endpoint: &str, body: &Value, quality: &str) -> anyhow::Result<String> {
        let resp = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
            )
            .json(body)
            .timeout(std::time::Duration::from_secs(8))
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
                return Ok(url.to_string());
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
