use std::path::PathBuf;
use axum::response::Json;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub struct LyricsQuery {
    pub title: Option<String>,
    pub artist: Option<String>,
    #[allow(dead_code)]
    pub source: Option<String>,
    #[allow(dead_code)]
    pub duration: Option<f64>,
    pub online: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(rename_all = "camelCase")]
struct LrcLibItem {
    id: Option<i64>,
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: Option<bool>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[derive(Clone)]
pub struct LyricsService {
    client: Client,
    custom_lyrics_dir: PathBuf,
}

impl LyricsService {
    pub fn new(client: Client, data_dir: PathBuf) -> Self {
        let dir = data_dir.join("custom_lyrics");
        let _ = std::fs::create_dir_all(&dir);
        Self {
            client,
            custom_lyrics_dir: dir,
        }
    }

    pub async fn fetch_lyrics(&self, query: LyricsQuery) -> Json<Value> {
        let title = query.title.unwrap_or_default().trim().to_string();
        let artist = query.artist.unwrap_or_default().trim().to_string();
        let allow_online = query.online.as_deref() != Some("0");

        if title.is_empty() && artist.is_empty() {
            return Json(json!({
                "success": false,
                "lyrics": "",
                "isCustom": false,
                "instrumental": false
            }));
        }

        // 1. Check local custom lyrics
        let file_name = format!("{artist} - {title}.lrc");
        let custom_path = self.custom_lyrics_dir.join(&file_name);
        if custom_path.is_file() {
            if let Ok(content) = tokio::fs::read_to_string(&custom_path).await {
                if !content.trim().is_empty() {
                    return Json(json!({
                        "success": true,
                        "lyrics": content,
                        "isCustom": true,
                        "instrumental": false,
                        "isExplicit": false
                    }));
                }
            }
        }

        if !allow_online {
            return Json(json!({
                "success": false,
                "lyrics": "",
                "isCustom": false,
                "instrumental": false
            }));
        }

        // 2. Fetch from LRCLIB API
        let mut clean_title = title.clone();
        if let Some(pos) = clean_title.find('(') {
            clean_title = clean_title[..pos].trim().to_string();
        }
        if let Some(pos) = clean_title.find('[') {
            clean_title = clean_title[..pos].trim().to_string();
        }

        let direct_url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}",
            url_encode(&clean_title),
            url_encode(&artist)
        );

        if let Ok(resp) = self.client.get(&direct_url).timeout(std::time::Duration::from_secs(5)).send().await {
            if resp.status().is_success() {
                if let Ok(item) = resp.json::<LrcLibItem>().await {
                    if item.instrumental == Some(true) {
                        return Json(json!({
                            "success": true,
                            "lyrics": "",
                            "isCustom": false,
                            "instrumental": true,
                            "plainLyrics": "",
                            "syncedLyrics": ""
                        }));
                    }

                    let text = item.synced_lyrics.filter(|s| !s.trim().is_empty())
                        .or_else(|| item.plain_lyrics.filter(|s| !s.trim().is_empty()))
                        .unwrap_or_default();

                    if !text.is_empty() {
                        return Json(json!({
                            "success": true,
                            "lyrics": text,
                            "isCustom": false,
                            "instrumental": false,
                            "isExplicit": false
                        }));
                    }
                }
            }
        }

        // 3. Fallback: Search on LRCLIB
        let search_url = format!(
            "https://lrclib.net/api/search?q={}",
            url_encode(&format!("{artist} {clean_title}"))
        );

        if let Ok(resp) = self.client.get(&search_url).timeout(std::time::Duration::from_secs(5)).send().await {
            if resp.status().is_success() {
                if let Ok(items) = resp.json::<Vec<LrcLibItem>>().await {
                    if let Some(first) = items.into_iter().next() {
                        if first.instrumental == Some(true) {
                            return Json(json!({
                                "success": true,
                                "lyrics": "",
                                "isCustom": false,
                                "instrumental": true,
                                "plainLyrics": "",
                                "syncedLyrics": ""
                            }));
                        }

                        let text = first.synced_lyrics.filter(|s| !s.trim().is_empty())
                            .or_else(|| first.plain_lyrics.filter(|s| !s.trim().is_empty()))
                            .unwrap_or_default();

                        if !text.is_empty() {
                            return Json(json!({
                                "success": true,
                                "lyrics": text,
                                "isCustom": false,
                                "instrumental": false,
                                "isExplicit": false
                            }));
                        }
                    }
                }
            }
        }

        Json(json!({
            "success": false,
            "lyrics": "",
            "isCustom": false,
            "instrumental": false
        }))
    }
}
