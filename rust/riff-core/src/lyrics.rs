use std::path::PathBuf;
use axum::response::Json;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
pub struct LyricsQuery {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub source: Option<String>,
    pub duration: Option<f64>,
    pub online: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordTiming {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordLine {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<WordTiming>,
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

fn clean_track_title(title: &str) -> String {
    let mut s = title.trim().to_string();
    if let Some(pos) = s.find('(') {
        s = s[..pos].trim().to_string();
    }
    if let Some(pos) = s.find('[') {
        s = s[..pos].trim().to_string();
    }
    s
}

fn has_synced_timestamps(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        t.starts_with('[') && t.find(']').map(|i| t[1..i].contains(':')).unwrap_or(false)
    })
}

fn norm_sim_str(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn is_loose_match(target_title: &str, cand_title: &str) -> bool {
    let a = norm_sim_str(&clean_track_title(target_title));
    let b = norm_sim_str(&clean_track_title(cand_title));
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a == b || a.contains(&b) || b.contains(&a)
}

/// Parse NetEase YRC format: `[lineStartMs,lineDurMs](wordStartMs,wordDurMs,0)word...`
pub fn parse_yrc(yrc_text: &str) -> Vec<WordLine> {
    let mut lines = Vec::new();
    for raw in yrc_text.lines() {
        let trimmed = raw.trim();
        if !trimmed.starts_with('[') {
            continue;
        }
        let Some(close_b) = trimmed.find(']') else { continue };
        let header = &trimmed[1..close_b];
        let Some((start_s, dur_s)) = header.split_once(',') else { continue };
        let (Ok(line_start_ms), Ok(line_dur_ms)) = (start_s.parse::<f64>(), dur_s.parse::<f64>()) else {
            continue;
        };
        let line_start = line_start_ms / 1000.0;
        let line_end = (line_start_ms + line_dur_ms) / 1000.0;
        let rest = &trimmed[close_b + 1..];

        let mut words = Vec::new();
        let mut cursor = 0usize;
        let bytes = rest.as_bytes();
        while cursor < bytes.len() {
            if bytes[cursor] == b'(' {
                if let Some(rel_close) = rest[cursor + 1..].find(')') {
                    let meta = &rest[cursor + 1..cursor + 1 + rel_close];
                    let parts: Vec<&str> = meta.split(',').collect();
                    let after_paren = cursor + 1 + rel_close + 1;
                    let next_open = rest[after_paren..].find('(').map(|p| after_paren + p).unwrap_or(rest.len());
                    let w_text = rest[after_paren..next_open].trim();
                    if parts.len() >= 2 && !w_text.is_empty() {
                        if let (Ok(ws_ms), Ok(wd_ms)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                            let ws = ws_ms / 1000.0;
                            let we = (ws_ms + wd_ms.max(50.0)) / 1000.0;
                            words.push(WordTiming {
                                text: w_text.to_string(),
                                start: ws,
                                end: we,
                            });
                        }
                    }
                    cursor = next_open;
                    continue;
                }
            }
            cursor += 1;
        }

        if !words.is_empty() {
            let full_text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
            if !full_text.starts_with("作词") && !full_text.starts_with("作曲") && !full_text.starts_with("制作人") {
                lines.push(WordLine {
                    start: line_start,
                    end: line_end,
                    text: full_text,
                    words,
                });
            }
        }
    }
    lines
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

    pub async fn save_custom_lyrics(&self, title: &str, artist: &str, content: &str) -> anyhow::Result<()> {
        let safe_title = title.trim().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let safe_artist = artist.trim().replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let file_name = format!("{safe_artist} - {safe_title}.lrc");
        let custom_path = self.custom_lyrics_dir.join(&file_name);
        if content.trim().is_empty() {
            if custom_path.exists() {
                let _ = tokio::fs::remove_file(&custom_path).await;
            }
        } else {
            tokio::fs::write(&custom_path, content).await?;
        }
        Ok(())
    }

    async fn fetch_lrclib(&self, title: &str, artist: &str) -> Option<(String, bool)> {
        let clean_title = clean_track_title(title);
        let direct_url = format!(
            "https://lrclib.net/api/get?track_name={}&artist_name={}",
            url_encode(&clean_title),
            url_encode(artist)
        );

        if let Ok(resp) = self.client.get(&direct_url).timeout(std::time::Duration::from_secs(5)).send().await {
            if resp.status().is_success() {
                if let Ok(item) = resp.json::<LrcLibItem>().await {
                    if item.instrumental == Some(true) {
                        return Some((String::new(), true));
                    }
                    let text = item
                        .synced_lyrics
                        .filter(|s| !s.trim().is_empty())
                        .or_else(|| item.plain_lyrics.filter(|s| !s.trim().is_empty()))
                        .unwrap_or_default();
                    if !text.is_empty() {
                        return Some((text, false));
                    }
                }
            }
        }

        let search_url = format!(
            "https://lrclib.net/api/search?q={}",
            url_encode(&format!("{artist} {clean_title}"))
        );
        if let Ok(resp) = self.client.get(&search_url).timeout(std::time::Duration::from_secs(5)).send().await {
            if resp.status().is_success() {
                if let Ok(items) = resp.json::<Vec<LrcLibItem>>().await {
                    if let Some(first) = items.into_iter().next() {
                        if first.instrumental == Some(true) {
                            return Some((String::new(), true));
                        }
                        let text = first
                            .synced_lyrics
                            .filter(|s| !s.trim().is_empty())
                            .or_else(|| first.plain_lyrics.filter(|s| !s.trim().is_empty()))
                            .unwrap_or_default();
                        if !text.is_empty() {
                            return Some((text, false));
                        }
                    }
                }
            }
        }
        None
    }

    async fn fetch_kugou(&self, title: &str, artist: &str, duration_sec: f64) -> Option<String> {
        let clean_title = clean_track_title(title);
        let dur_ms = (duration_sec.max(0.0) * 1000.0).round() as u64;
        let search_url = format!(
            "http://lyrics.kugou.com/search?ver=1&man=yes&client=pc&keyword={}&duration={}&hash=",
            url_encode(&format!("{artist} - {clean_title}")),
            dur_ms
        );
        let resp = self
            .client
            .get(&search_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: Value = resp.json().await.ok()?;
        let candidates = json.get("candidates")?.as_array()?;
        let cand = candidates.iter().find(|c| {
            let sname = c.get("songname").and_then(|v| v.as_str()).unwrap_or("");
            is_loose_match(title, sname)
        })?;
        let id = cand.get("id")?.as_str().map(String::from).or_else(|| cand.get("id")?.as_i64().map(|n| n.to_string()))?;
        let accesskey = cand.get("accesskey")?.as_str()?;
        let dl_url = format!(
            "http://lyrics.kugou.com/download?ver=1&client=pc&id={id}&accesskey={accesskey}&fmt=lrc&charset=utf8"
        );
        let dl_resp = self
            .client
            .get(&dl_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        let dl_json: Value = dl_resp.json().await.ok()?;
        let b64 = dl_json.get("content")?.as_str()?;
        let decoded = base64_decode(b64)?;
        let text = String::from_utf8(decoded).ok()?;
        let cleaned: Vec<&str> = text
            .lines()
            .filter(|l| {
                let low = l.trim().to_ascii_lowercase();
                !(low.starts_with("[id:")
                    || low.starts_with("[ti:")
                    || low.starts_with("[ar:")
                    || low.starts_with("[al:")
                    || low.starts_with("[by:")
                    || low.starts_with("[hash:")
                    || low.starts_with("[sign:")
                    || low.starts_with("[qq:")
                    || low.starts_with("[total:")
                    || low.starts_with("[offset:"))
            })
            .collect();
        let out = cleaned.join("\n").trim().to_string();
        if out.is_empty() { None } else { Some(out) }
    }

    async fn search_netease_song_id(&self, title: &str, artist: &str, duration_sec: f64) -> Option<i64> {
        let clean_title = clean_track_title(title);
        let q = format!("{clean_title} {artist}").trim().to_string();
        let search_url = format!(
            "http://music.163.com/api/search/get?s={}&type=1&limit=5",
            url_encode(&q)
        );
        let resp = self
            .client
            .get(&search_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .header("Referer", "https://music.163.com")
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: Value = resp.json().await.ok()?;
        let songs = json.pointer("/result/songs")?.as_array()?;
        for s in songs {
            let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if !is_loose_match(title, name) {
                continue;
            }
            if duration_sec > 0.0 {
                let dur_ms = s.get("duration").or_else(|| s.get("dt")).and_then(|v| v.as_f64()).unwrap_or(0.0);
                if dur_ms > 0.0 && ((dur_ms / 1000.0) - duration_sec).abs() > 5.0 {
                    continue;
                }
            }
            if let Some(id) = s.get("id").and_then(|v| v.as_i64()) {
                return Some(id);
            }
        }
        None
    }

    async fn fetch_netease_lrc(&self, title: &str, artist: &str, duration_sec: f64) -> Option<String> {
        let song_id = self.search_netease_song_id(title, artist, duration_sec).await?;
        let lyric_url = format!("http://music.163.com/api/song/lyric?id={song_id}&lv=1&kv=1&tv=-1");
        let resp = self
            .client
            .get(&lyric_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .header("Referer", "https://music.163.com")
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
            .ok()?;
        let json: Value = resp.json().await.ok()?;
        let lrc = json.pointer("/lrc/lyric").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if lrc.is_empty() { None } else { Some(lrc) }
    }

    pub async fn fetch_netease_words(&self, title: &str, artist: &str, duration_sec: f64) -> Vec<WordLine> {
        let Some(song_id) = self.search_netease_song_id(title, artist, duration_sec).await else {
            return Vec::new();
        };
        let lyric_url = format!("http://music.163.com/api/song/lyric?id={song_id}&lv=1&kv=1&tv=-1&yv=1");
        let Ok(resp) = self
            .client
            .get(&lyric_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .header("Referer", "https://music.163.com")
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
        else {
            return Vec::new();
        };
        let Ok(json) = resp.json::<Value>().await else {
            return Vec::new();
        };
        if let Some(yrc) = json.pointer("/yrc/lyric").and_then(|v| v.as_str()) {
            return parse_yrc(yrc);
        }
        Vec::new()
    }

    async fn fetch_musixmatch(&self, title: &str, artist: &str) -> Option<String> {
        let tok_resp = self
            .client
            .get("https://apic-desktop.musixmatch.com/ws/1.1/token.get?app_id=web-desktop-app-v1.0")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .ok()?;
        let tok_json: Value = tok_resp.json().await.ok()?;
        let token = tok_json.pointer("/message/body/user_token").and_then(|v| v.as_str()).unwrap_or("");
        let clean_title = clean_track_title(title);
        let macro_url = format!(
            "https://apic-desktop.musixmatch.com/ws/1.1/macro.subtitles.get?app_id=web-desktop-app-v1.0&usertoken={}&q_track={}&q_artist={}",
            url_encode(token),
            url_encode(&clean_title),
            url_encode(artist)
        );
        let resp = self
            .client
            .get(&macro_url)
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
            .ok()?;
        let json: Value = resp.json().await.ok()?;
        let macro_calls = json.pointer("/message/body/macro_calls")?;
        let sub = macro_calls
            .pointer("/track.subtitles.get/message/body/subtitle_list/0/subtitle/subtitle_body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if !sub.is_empty() {
            return Some(sub);
        }
        let plain = macro_calls
            .pointer("/track.lyrics.get/message/body/lyrics/lyrics_body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if plain.is_empty() { None } else { Some(plain) }
    }

    pub async fn fetch_lyrics(&self, query: LyricsQuery) -> Json<Value> {
        let title = query.title.unwrap_or_default().trim().to_string();
        let artist = query.artist.unwrap_or_default().trim().to_string();
        let allow_online = query.online.as_deref() != Some("0");
        let source_pref = query.source.unwrap_or_else(|| "auto".to_string()).to_ascii_lowercase();
        let duration_sec = query.duration.unwrap_or(0.0);

        if title.is_empty() && artist.is_empty() {
            return Json(json!({
                "success": false,
                "lyrics": "",
                "source": "none",
                "isSynced": false,
                "isCustom": false,
                "instrumental": false
            }));
        }

        // 1. Check local custom lyrics (unless user explicitly requested a specific online provider)
        if source_pref == "auto" || source_pref == "custom" {
            let safe_title = title.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            let safe_artist = artist.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            let safe_path = self.custom_lyrics_dir.join(format!("{safe_artist} - {safe_title}.lrc"));
            let raw_path = self.custom_lyrics_dir.join(format!("{artist} - {title}.lrc"));
            let custom_path = if safe_path.is_file() { safe_path } else { raw_path };
            if custom_path.is_file() {
                if let Ok(content) = tokio::fs::read_to_string(&custom_path).await {
                    if !content.trim().is_empty() {
                        let synced = has_synced_timestamps(&content);
                        return Json(json!({
                            "success": true,
                            "lyrics": content,
                            "source": "custom",
                            "isSynced": synced,
                            "isCustom": true,
                            "instrumental": false,
                            "isExplicit": false
                        }));
                    }
                }
            }
        }

        if !allow_online {
            return Json(json!({
                "success": false,
                "lyrics": "",
                "source": "none",
                "isSynced": false,
                "isCustom": false,
                "instrumental": false
            }));
        }

        let mut chosen_text = String::new();
        let mut chosen_source = String::new();
        let mut plain_fallback: Option<(String, String)> = None;

        match source_pref.as_str() {
            "lrclib" => {
                if let Some((text, inst)) = self.fetch_lrclib(&title, &artist).await {
                    if inst {
                        return Json(json!({ "success": true, "lyrics": "", "source": "lrclib", "isSynced": false, "isCustom": false, "instrumental": true }));
                    }
                    chosen_text = text;
                    chosen_source = "lrclib".to_string();
                }
            }
            "kugou" => {
                if let Some(text) = self.fetch_kugou(&title, &artist, duration_sec).await {
                    chosen_text = text;
                    chosen_source = "kugou".to_string();
                }
            }
            "netease" => {
                if let Some(text) = self.fetch_netease_lrc(&title, &artist, duration_sec).await {
                    chosen_text = text;
                    chosen_source = "netease".to_string();
                }
            }
            "musixmatch" => {
                if let Some(text) = self.fetch_musixmatch(&title, &artist).await {
                    chosen_text = text;
                    chosen_source = "musixmatch".to_string();
                }
            }
            _ => {
                // Auto cascade: LRCLIB -> Musixmatch -> Kugou -> NetEase (preferring synced)
                if let Some((text, inst)) = self.fetch_lrclib(&title, &artist).await {
                    if inst {
                        return Json(json!({ "success": true, "lyrics": "", "source": "lrclib", "isSynced": false, "isCustom": false, "instrumental": true }));
                    }
                    if has_synced_timestamps(&text) {
                        chosen_text = text;
                        chosen_source = "lrclib".to_string();
                    } else {
                        plain_fallback = Some((text, "lrclib".to_string()));
                    }
                }
                if chosen_text.is_empty() {
                    if let Some(text) = self.fetch_musixmatch(&title, &artist).await {
                        if has_synced_timestamps(&text) {
                            chosen_text = text;
                            chosen_source = "musixmatch".to_string();
                        } else if plain_fallback.is_none() {
                            plain_fallback = Some((text, "musixmatch".to_string()));
                        }
                    }
                }
                if chosen_text.is_empty() {
                    if let Some(text) = self.fetch_kugou(&title, &artist, duration_sec).await {
                        if has_synced_timestamps(&text) {
                            chosen_text = text;
                            chosen_source = "kugou".to_string();
                        } else if plain_fallback.is_none() {
                            plain_fallback = Some((text, "kugou".to_string()));
                        }
                    }
                }
                if chosen_text.is_empty() {
                    if let Some(text) = self.fetch_netease_lrc(&title, &artist, duration_sec).await {
                        if has_synced_timestamps(&text) {
                            chosen_text = text;
                            chosen_source = "netease".to_string();
                        } else if plain_fallback.is_none() {
                            plain_fallback = Some((text, "netease".to_string()));
                        }
                    }
                }
                if chosen_text.is_empty() {
                    if let Some((text, src)) = plain_fallback {
                        chosen_text = text;
                        chosen_source = src;
                    }
                }
            }
        }

        if !chosen_text.is_empty() {
            let synced = has_synced_timestamps(&chosen_text);
            return Json(json!({
                "success": true,
                "lyrics": chosen_text,
                "source": chosen_source,
                "isSynced": synced,
                "isCustom": false,
                "instrumental": false,
                "isExplicit": false
            }));
        }

        Json(json!({
            "success": false,
            "lyrics": "",
            "source": "none",
            "isSynced": false,
            "isCustom": false,
            "instrumental": false
        }))
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in input.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}
