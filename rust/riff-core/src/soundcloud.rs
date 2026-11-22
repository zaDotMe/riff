use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::{info, warn};

const FALLBACK_CLIENT_ID: &str = "glXhJxvYKPM87la70dG2G1bJ03AdA3hN";
const CLIENT_ID_CACHE_TTL: Duration = Duration::from_secs(30 * 60); // 30 minutes
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[derive(Debug, Clone)]
pub struct SoundCloudResolvedStream {
    pub url: String,
    pub is_hls: bool,
    pub title: String,
    pub artist: String,
    pub duration: f64,
}

pub struct SoundCloudClient {
    http: reqwest::Client,
    cached_client_id: RwLock<(String, Instant)>,
    client_id_regex: Regex,
    script_src_regex: Regex,
    clean_title_regex: Regex,
}

impl SoundCloudClient {
    pub fn new(http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            http,
            cached_client_id: RwLock::new((String::new(), Instant::now())),
            client_id_regex: Regex::new(r#"client_id\s*[:=]\s*["']([a-zA-Z0-9]{32})["']"#).unwrap(),
            script_src_regex: Regex::new(r#"src="(https://a-v2\.sndcdn\.com/assets/[^"]+\.js)""#).unwrap(),
            clean_title_regex: Regex::new(
                r#"(?i)\s*[\(\[](?:official\s*(?:music\s*|lyric\s*)?video|official\s*audio|official|lyrics?|lyric\s*video|audio|hd|4k|visualizer|mv)[\)\]]"#,
            )
            .unwrap(),
        })
    }

    pub fn clean_title(&self, title: &str) -> String {
        let cleaned = self.clean_title_regex.replace_all(title, " ");
        cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    pub async fn get_client_id(&self, force_refresh: bool) -> String {
        if !force_refresh {
            let cache = self.cached_client_id.read().await;
            if cache.1.elapsed() < CLIENT_ID_CACHE_TTL && !cache.0.is_empty() {
                return cache.0.clone();
            }
        }

        match self.scrape_client_id().await {
            Ok(new_id) => {
                info!("Obtained fresh SoundCloud client_id: {}", new_id);
                let mut cache = self.cached_client_id.write().await;
                *cache = (new_id.clone(), Instant::now());
                new_id
            }
            Err(err) => {
                warn!("Failed to scrape SoundCloud client_id: {}. Using fallback.", err);
                let cache = self.cached_client_id.read().await;
                if !cache.0.is_empty() {
                    cache.0.clone()
                } else {
                    FALLBACK_CLIENT_ID.to_string()
                }
            }
        }
    }

    async fn scrape_client_id(&self) -> Result<String> {
        let html = self
            .http
            .get("https://soundcloud.com")
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .context("fetch soundcloud.com homepage")?
            .text()
            .await?;

        let script_urls: Vec<String> = self
            .script_src_regex
            .captures_iter(&html)
            .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
            .collect();

        // Check recent scripts from the end of the page
        for url in script_urls.into_iter().rev().take(6) {
            if let Ok(resp) = self
                .http
                .get(&url)
                .header("User-Agent", USER_AGENT)
                .send()
                .await
            {
                if let Ok(js) = resp.text().await {
                    if let Some(caps) = self.client_id_regex.captures(&js) {
                        if let Some(matched) = caps.get(1) {
                            return Ok(matched.as_str().to_string());
                        }
                    }
                }
            }
        }

        Err(anyhow!("client_id not found in SoundCloud script assets"))
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Value>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let mut cid = self.get_client_id(false).await;

        for attempt in 0..2 {
            let url = format!(
                "https://api-v2.soundcloud.com/search/tracks?q={}&client_id={}&limit={}",
                url_encode(query),
                cid,
                limit.clamp(1, 50)
            );

            let res = self
                .http
                .get(&url)
                .header("User-Agent", USER_AGENT)
                .send()
                .await;

            match res {
                Ok(resp) => {
                    let status = resp.status();
                    if (status == 401 || status == 403) && attempt == 0 {
                        cid = self.get_client_id(true).await;
                        continue;
                    }

                    if resp.status().is_success() {
                        let json: Value = resp.json().await.context("parse soundcloud search response")?;
                        let collection = json
                            .get("collection")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();

                        let tracks: Vec<Value> = collection
                            .into_iter()
                            .filter(|t| t.get("id").is_some() && t.get("title").is_some())
                            .map(|t| {
                                let id = t
                                    .get("id")
                                    .map(|v| {
                                        if let Some(s) = v.as_str() {
                                            s.trim().to_string()
                                        } else if let Some(n) = v.as_u64() {
                                            n.to_string()
                                        } else {
                                            v.to_string().trim().to_string()
                                        }
                                    })
                                    .unwrap_or_default();
                                let title = t.get("title").and_then(|v| v.as_str()).unwrap_or("untitled");

                                let artist = t
                                    .get("user")
                                    .and_then(|u| u.get("username").or_else(|| u.get("name")))
                                    .and_then(|v| v.as_str())
                                    .or_else(|| {
                                        t.get("publisher_metadata")
                                            .and_then(|p| p.get("artist"))
                                            .and_then(|v| v.as_str())
                                    })
                                    .unwrap_or("unknown artist");

                                let raw_thumb = t
                                    .get("artwork_url")
                                    .or_else(|| t.get("user").and_then(|u| u.get("avatar_url")))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");

                                let thumbnail = if !raw_thumb.is_empty() {
                                    raw_thumb.replace("-large", "-t500x500")
                                } else {
                                    raw_thumb.to_string()
                                };

                                let duration_ms = t
                                    .get("duration")
                                    .or_else(|| t.get("full_duration"))
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0);

                                let permalink = t
                                    .get("permalink_url")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");

                                json!({
                                    "id": id,
                                    "title": title,
                                    "artist": artist,
                                    "duration": (duration_ms / 1000.0).round(),
                                    "thumbnail": thumbnail,
                                    "url": permalink,
                                    "platform": "soundcloud",
                                    "isCached": false,
                                })
                            })
                            .collect();

                        return Ok(tracks);
                    }
                }
                Err(err) => {
                    if attempt == 1 {
                        return Err(err.into());
                    }
                }
            }
        }

        Ok(Vec::new())
    }

    pub async fn resolve_smart_mirror(
        &self,
        title: &str,
        artist: &str,
        target_dur_sec: f64,
    ) -> Result<SoundCloudResolvedStream> {
        let cleaned_title = self.clean_title(title);
        let query = if !cleaned_title.is_empty() && !artist.is_empty() {
            format!("{} {}", cleaned_title, artist)
        } else if !cleaned_title.is_empty() {
            cleaned_title.clone()
        } else if !artist.is_empty() {
            artist.to_string()
        } else {
            return Err(anyhow!("empty title and artist for smart mirror"));
        };

        let mut cid = self.get_client_id(false).await;

        for attempt in 0..2 {
            let ep = format!(
                "https://api-v2.soundcloud.com/search/tracks?q={}&limit=15&client_id={}",
                url_encode(&query),
                cid
            );

            let resp = match self.http.get(&ep).header("User-Agent", USER_AGENT).send().await {
                Ok(r) => r,
                Err(e) => {
                    if attempt == 1 {
                        return Err(e.into());
                    }
                    continue;
                }
            };

            let status = resp.status();
            if (status == 401 || status == 403) && attempt == 0 {
                cid = self.get_client_id(true).await;
                continue;
            }

            if !status.is_success() {
                return Err(anyhow!("SoundCloud search returned status {}", status));
            }

            let json: Value = resp.json().await.context("parse soundcloud smart mirror search response")?;
            let collection = json
                .get("collection")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Filter out snippets and extreme durations (<35s or >660s)
            let mut candidates: Vec<Value> = collection
                .into_iter()
                .filter(|t| {
                    let is_snip = t.get("policy").and_then(|p| p.as_str()) == Some("SNIP")
                        || t.get("snipped").and_then(|s| s.as_bool()).unwrap_or(false);
                    if is_snip {
                        return false;
                    }

                    let dur_ms = t
                        .get("duration")
                        .or_else(|| t.get("full_duration"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let dur_sec = dur_ms / 1000.0;
                    if dur_sec < 35.0 || dur_sec > 660.0 {
                        return false;
                    }

                    let has_transcodings = t
                        .get("media")
                        .and_then(|m| m.get("transcodings"))
                        .and_then(|tr| tr.as_array())
                        .map(|arr| !arr.is_empty())
                        .unwrap_or(false);

                    has_transcodings
                })
                .collect();

            if candidates.is_empty() {
                return Err(anyhow!("no viable candidates found on soundcloud for '{}'", query));
            }

            // Rank candidates by score descending
            candidates.sort_by(|a, b| {
                let sa = score_track(a, &cleaned_title, artist, target_dur_sec);
                let sb = score_track(b, &cleaned_title, artist, target_dur_sec);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });

            // Try the top candidates until one successfully yields a playable stream
            for cand in candidates.iter().take(5) {
                if let Ok(Some((stream_url, is_hls))) = self.resolve_transcoding(cand, &cid).await {
                    let track_title = cand
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or(title)
                        .to_string();
                    let track_artist = cand
                        .get("user")
                        .and_then(|u| u.get("username").or_else(|| u.get("name")))
                        .and_then(|v| v.as_str())
                        .unwrap_or(artist)
                        .to_string();
                    let dur_ms = cand
                        .get("duration")
                        .or_else(|| cand.get("full_duration"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(target_dur_sec * 1000.0);

                    return Ok(SoundCloudResolvedStream {
                        url: stream_url,
                        is_hls,
                        title: track_title,
                        artist: track_artist,
                        duration: dur_ms / 1000.0,
                    });
                }
            }
            break;
        }

        Err(anyhow!("could not resolve smart mirror audio stream for '{}'", query))
    }

    pub async fn resolve_stream(
        &self,
        url_or_id: &str,
        query_fallback: Option<&str>,
    ) -> Result<SoundCloudResolvedStream> {
        let mut cid = self.get_client_id(false).await;
        let norm = url_or_id.trim();

        for attempt in 0..2 {
            let track_obj = if norm.chars().all(|c| c.is_ascii_digit()) && !norm.is_empty() {
                // Numeric track ID
                let ep = format!("https://api-v2.soundcloud.com/tracks/{}?client_id={}", norm, cid);
                self.fetch_json_track(&ep).await
            } else if norm.contains("soundcloud.com/") {
                // Permalink resolve
                let ep = format!(
                    "https://api-v2.soundcloud.com/resolve?url={}&client_id={}",
                    url_encode(norm),
                    cid
                );
                self.fetch_json_track(&ep).await
            } else if let Some(q) = query_fallback {
                // Fallback search
                let ep = format!(
                    "https://api-v2.soundcloud.com/search/tracks?q={}&limit=10&client_id={}",
                    url_encode(q),
                    cid
                );
                match self.fetch_json_track(&ep).await {
                    Ok(val) => {
                        let first = val
                            .get("collection")
                            .and_then(|c| c.as_array())
                            .and_then(|arr| {
                                arr.iter().find(|t| {
                                    t.get("policy").and_then(|p| p.as_str()) != Some("SNIP")
                                        && !t.get("snipped").and_then(|s| s.as_bool()).unwrap_or(false)
                                })
                            })
                            .cloned();
                        first.ok_or_else(|| anyhow!("no valid tracks in search fallback"))
                    }
                    Err(e) => Err(e),
                }
            } else {
                Err(anyhow!("unsupported soundcloud identifier"))
            };

            match track_obj {
                Ok(obj) => {
                    if let Some((stream_url, is_hls)) = self.resolve_transcoding(&obj, &cid).await? {
                        let title = obj.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let artist = obj
                            .get("user")
                            .and_then(|u| u.get("username").or_else(|| u.get("name")))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let dur_ms = obj
                            .get("duration")
                            .or_else(|| obj.get("full_duration"))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);

                        return Ok(SoundCloudResolvedStream {
                            url: stream_url,
                            is_hls,
                            title,
                            artist,
                            duration: dur_ms / 1000.0,
                        });
                    }
                }
                Err(err) => {
                    let msg = err.to_string();
                    if (msg.contains("401") || msg.contains("403")) && attempt == 0 {
                        cid = self.get_client_id(true).await;
                        continue;
                    }
                }
            }
            break;
        }

        Err(anyhow!("could not resolve soundcloud audio stream"))
    }

    async fn fetch_json_track(&self, url: &str) -> Result<Value> {
        let resp = self
            .http
            .get(url)
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(anyhow!("SoundCloud API returned status {}", status));
        }

        let val: Value = resp.json().await?;
        Ok(val)
    }

    async fn resolve_transcoding(
        &self,
        track_obj: &Value,
        cid: &str,
    ) -> Result<Option<(String, bool)>> {
        let transcodings = track_obj
            .get("media")
            .and_then(|m| m.get("transcodings"))
            .and_then(|t| t.as_array());

        let list = match transcodings {
            Some(l) => l,
            None => return Ok(None),
        };

        // Filter out snippets and preview endpoints
        let valid_transcodings: Vec<&Value> = list
            .iter()
            .filter(|t| {
                let snipped = t.get("snipped").and_then(|s| s.as_bool()).unwrap_or(false);
                if snipped {
                    return false;
                }
                let url = t.get("url").and_then(|u| u.as_str()).unwrap_or("");
                !url.contains("/preview/")
            })
            .collect();

        // Sort: prioritize progressive MP3, then HLS
        let mut sorted = valid_transcodings;
        sorted.sort_by(|a, b| {
            let proto_a = a
                .get("format")
                .and_then(|f| f.get("protocol"))
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let proto_b = b
                .get("format")
                .and_then(|f| f.get("protocol"))
                .and_then(|p| p.as_str())
                .unwrap_or("");
            if proto_a == "progressive" && proto_b != "progressive" {
                std::cmp::Ordering::Less
            } else if proto_b == "progressive" && proto_a != "progressive" {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        for t in sorted {
            if let Some(base_url) = t.get("url").and_then(|u| u.as_str()) {
                let protocol = t
                    .get("format")
                    .and_then(|f| f.get("protocol"))
                    .and_then(|p| p.as_str())
                    .unwrap_or("");

                let sep = if base_url.contains('?') { "&" } else { "?" };
                let auth_param = track_obj
                    .get("track_authorization")
                    .and_then(|a| a.as_str())
                    .map(|a| format!("&track_authorization={}", url_encode(a)))
                    .unwrap_or_default();

                let req_url = format!("{}{}client_id={}{}", base_url, sep, cid, auth_param);
                if let Ok(resp) = self
                    .http
                    .get(&req_url)
                    .header("User-Agent", USER_AGENT)
                    .send()
                    .await
                {
                    if resp.status().is_success() {
                        if let Ok(json) = resp.json::<Value>().await {
                            if let Some(cdn_url) = json.get("url").and_then(|u| u.as_str()) {
                                if !cdn_url.is_empty()
                                    && !cdn_url.contains("cf-preview-media")
                                    && !cdn_url.contains("/preview/")
                                {
                                    let is_hls = protocol == "hls" || cdn_url.contains(".m3u8");
                                    return Ok(Some((cdn_url.to_string(), is_hls)));
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }
}

fn score_track(t: &Value, target_title: &str, target_artist: &str, target_dur_sec: f64) -> f64 {
    let dur_ms = t
        .get("duration")
        .or_else(|| t.get("full_duration"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let dur_sec = dur_ms / 1000.0;

    let mut score = 100.0;
    let title = t
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let artist = t
        .get("user")
        .and_then(|u| u.get("username").or_else(|| u.get("name")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    let tgt_title = target_title.to_lowercase();
    let tgt_artist = target_artist.to_lowercase();

    if !tgt_title.is_empty() && title.contains(&tgt_title) {
        score += 35.0;
    }
    if !tgt_artist.is_empty() && (title.contains(&tgt_artist) || artist.contains(&tgt_artist)) {
        score += 20.0;
    }

    const PENALTIES: &[&str] = &[
        "slowed",
        "reverb",
        "remix",
        "sped up",
        "speed up",
        "mashup",
        "cover",
        "tribute",
        "edit",
        "acoustic",
        "instrumental",
        "karaoke",
    ];
    for p in PENALTIES {
        if title.contains(p) && !tgt_title.contains(p) {
            score -= 30.0;
        }
    }

    if title.contains("original") || title.contains("official") {
        score += 15.0;
    }

    if target_dur_sec > 0.0 {
        let diff = (dur_sec - target_dur_sec).abs();
        if diff <= 10.0 {
            score += 40.0;
        } else if diff <= 25.0 {
            score += 20.0;
        } else if diff <= 50.0 {
            score += 5.0;
        } else {
            score -= diff.min(50.0);
        }
    }

    score
}
