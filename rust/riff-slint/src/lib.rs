use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

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

    let blurred = image::imageops::blur(&small, 22.0);
    let (bw, bh) = blurred.dimensions();
    Some((bw, bh, blurred.into_raw(), accent))
}

fn to_slint_model(items: Vec<TrackItem>) -> ModelRc<TrackData> {
    let data: Vec<TrackData> = items.into_iter().map(TrackItem::into_slint).collect();
    ModelRc::new(VecModel::from(data))
}

fn infer_platform_for_track(t: &riff_core::models::Track) -> &'static str {
    if t.id.starts_with("local_") || t.path.contains('\\') || t.path.contains('/') {
        "local"
    } else if t.id.len() >= 6 && t.id.chars().all(|c| c.is_ascii_digit()) {
        "soundcloud"
    } else {
        "ytmusic"
    }
}

async fn hydrate_track_covers(items: &mut [TrackItem], http: &reqwest::Client) {
    let mut set = tokio::task::JoinSet::new();
    for (idx, item) in items.iter().enumerate().take(30) {
        if item.cover_rgba.is_none() && !item.thumbnail.is_empty() {
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
}

fn sync_favorite_flags(w: &MainWindow, fav_ids: &HashSet<String>) {
    let update_model = |m: ModelRc<TrackData>| {
        let mut vec: Vec<TrackData> = m.iter().collect();
        for item in &mut vec {
            item.is_favorite = fav_ids.contains(item.id.as_str());
        }
        ModelRc::new(VecModel::from(vec))
    };
    w.set_library_tracks(update_model(w.get_library_tracks()));
    w.set_search_results(update_model(w.get_search_results()));
    w.set_history_tracks(update_model(w.get_history_tracks()));
    w.set_queue_tracks(update_model(w.get_queue_tracks()));
    w.set_home_for_you(update_model(w.get_home_for_you()));
    w.set_home_trending(update_model(w.get_home_trending()));
    w.set_active_playlist_tracks(update_model(w.get_active_playlist_tracks()));
}

#[derive(Clone)]
struct NavHistory {
    stack: Vec<i32>,
    idx: usize,
}

impl NavHistory {
    fn new() -> Self {
        Self {
            stack: vec![0],
            idx: 0,
        }
    }

    fn push(&mut self, tab: i32) {
        if self.stack.get(self.idx).copied() == Some(tab) {
            return;
        }
        self.stack.truncate(self.idx + 1);
        self.stack.push(tab);
        if self.stack.len() > 40 {
            self.stack.remove(0);
        }
        self.idx = self.stack.len().saturating_sub(1);
    }

    fn back(&mut self) -> Option<i32> {
        if self.idx > 0 {
            self.idx -= 1;
            self.stack.get(self.idx).copied()
        } else {
            None
        }
    }

    fn forward(&mut self) -> Option<i32> {
        if self.idx + 1 < self.stack.len() {
            self.idx += 1;
            self.stack.get(self.idx).copied()
        } else {
            None
        }
    }

    fn can_back(&self) -> bool {
        self.idx > 0
    }

    fn can_forward(&self) -> bool {
        self.idx + 1 < self.stack.len()
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SavedSearchEntry {
    key: String,
    kind: String,
    title: String,
    subtitle: String,
    track_id: String,
    platform: String,
    thumbnail: String,
    duration_sec: f64,
}

#[derive(Clone)]
struct SearchHistorySlot {
    entry: SavedSearchEntry,
    cover_rgba: Option<(u32, u32, Vec<u8>)>,
}

async fn hydrate_search_history(
    entries: &[SavedSearchEntry],
    http: &reqwest::Client,
) -> Vec<SearchHistorySlot> {
    let mut slots: Vec<SearchHistorySlot> = entries
        .iter()
        .cloned()
        .map(|entry| SearchHistorySlot {
            entry,
            cover_rgba: None,
        })
        .collect();
    let mut set = tokio::task::JoinSet::new();
    for (idx, s) in slots.iter().enumerate().take(12) {
        if !s.entry.thumbnail.is_empty() {
            let client = http.clone();
            let url = s.entry.thumbnail.clone();
            set.spawn(async move { (idx, fetch_cover_rgba(&client, &url).await) });
        }
    }
    while let Some(res) = set.join_next().await {
        if let Ok((idx, rgba)) = res {
            if let Some(slot) = slots.get_mut(idx) {
                slot.cover_rgba = rgba;
            }
        }
    }
    slots
}

fn to_slint_search_history(slots: Vec<SearchHistorySlot>) -> ModelRc<SearchHistoryItem> {
    let data: Vec<SearchHistoryItem> = slots
        .into_iter()
        .map(|s| {
            let (cover, has_cover) = match s.cover_rgba {
                Some((w, h, raw)) => (rgba_to_slint_image(w, h, raw), true),
                None => (Image::default(), false),
            };
            SearchHistoryItem {
                key: SharedString::from(s.entry.key),
                kind: SharedString::from(s.entry.kind),
                title: SharedString::from(s.entry.title),
                subtitle: SharedString::from(s.entry.subtitle),
                track_id: SharedString::from(s.entry.track_id),
                platform: SharedString::from(s.entry.platform),
                thumbnail: SharedString::from(s.entry.thumbnail),
                duration_sec: s.entry.duration_sec as f32,
                cover,
                has_cover,
            }
        })
        .collect();
    ModelRc::new(VecModel::from(data))
}

fn apply_sidebar_playlists(
    list: &[riff_core::models::Playlist],
    filter_q: &str,
    sort_mode: &str,
) -> Vec<riff_core::models::Playlist> {
    let q = filter_q.trim().to_ascii_lowercase();
    let mut out: Vec<riff_core::models::Playlist> = list
        .iter()
        .filter(|p| q.is_empty() || p.name.to_ascii_lowercase().contains(&q))
        .cloned()
        .collect();
    match sort_mode {
        "A-Z" => out.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase())),
        "Tracks" => out.sort_by(|a, b| b.tracks.len().cmp(&a.tracks.len())),
        _ => {}
    }
    out
}

fn apply_track_filter_sort(items: &[TrackItem], filter_q: &str, sort_mode: &str) -> Vec<TrackItem> {
    let q = filter_q.trim().to_ascii_lowercase();
    let mut out: Vec<TrackItem> = items
        .iter()
        .filter(|t| {
            q.is_empty()
                || t.title.to_ascii_lowercase().contains(&q)
                || t.artist.to_ascii_lowercase().contains(&q)
                || t.album.to_ascii_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    match sort_mode {
        "Title A-Z" => out.sort_by(|a, b| a.title.to_ascii_lowercase().cmp(&b.title.to_ascii_lowercase())),
        "Artist A-Z" => out.sort_by(|a, b| a.artist.to_ascii_lowercase().cmp(&b.artist.to_ascii_lowercase())),
        "Duration" => out.sort_by(|a, b| {
            b.duration_sec
                .partial_cmp(&a.duration_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => {}
    }
    out
}

fn format_lrc_tag(sec: f32) -> String {
    let s = sec.max(0.0);
    let m = (s / 60.0).floor() as u32;
    let rem = s - (m as f32) * 60.0;
    format!("[{m:02}:{rem:05.2}]")
}

fn is_gap_text(text: &str) -> bool {
    let t = text.trim();
    t.eq_ignore_ascii_case("[Instrumental]")
        || t.eq_ignore_ascii_case("Instrumental")
        || t.eq_ignore_ascii_case("[Music]")
}

fn format_source_label(src: &str) -> &'static str {
    match src.to_ascii_lowercase().as_str() {
        "custom" => "Custom",
        "lrclib" => "LRCLIB",
        "musixmatch" => "Musixmatch",
        "kugou" => "Kugou",
        "netease" => "NetEase",
        "genius" => "Genius",
        "auto" => "Auto",
        _ => "None",
    }
}

#[derive(Clone)]
struct LyricWordItem {
    text: String,
    start_sec: f32,
    end_sec: f32,
    is_past: bool,
    is_active: bool,
    progress: f32,
}

#[derive(Clone)]
struct LyricLineItem {
    time_sec: f32,
    time_str: String,
    text: String,
    is_active: bool,
    is_past: bool,
    is_gap: bool,
    is_synced: bool,
    words: Vec<LyricWordItem>,
}

fn build_word_rows(words: &[LyricWordItem], max_chars: usize) -> ModelRc<LyricRowData> {
    if words.is_empty() {
        return ModelRc::new(VecModel::from(Vec::new()));
    }
    let mut rows: Vec<LyricRowData> = Vec::new();
    let mut cur_row: Vec<LyricWordData> = Vec::new();
    let mut cur_len = 0usize;

    for w in words {
        let w_len = w.text.chars().count();
        if !cur_row.is_empty() && cur_len + 1 + w_len > max_chars {
            rows.push(LyricRowData {
                words: ModelRc::new(VecModel::from(std::mem::take(&mut cur_row))),
            });
            cur_len = 0;
        }
        if !cur_row.is_empty() {
            cur_len += 1;
        }
        cur_len += w_len;
        cur_row.push(LyricWordData {
            text: SharedString::from(&w.text),
            start_sec: w.start_sec,
            end_sec: w.end_sec,
            is_past: w.is_past,
            is_active: w.is_active,
            progress: w.progress,
        });
    }
    if !cur_row.is_empty() {
        rows.push(LyricRowData {
            words: ModelRc::new(VecModel::from(cur_row)),
        });
    }
    ModelRc::new(VecModel::from(rows))
}

fn to_slint_lyrics(items: &[LyricLineItem]) -> ModelRc<LyricLineData> {
    let data: Vec<LyricLineData> = items
        .iter()
        .map(|l| LyricLineData {
            time_sec: l.time_sec,
            time_str: SharedString::from(&l.time_str),
            text: SharedString::from(&l.text),
            is_active: l.is_active,
            is_past: l.is_past,
            is_gap: l.is_gap,
            is_synced: l.is_synced,
            rows: build_word_rows(&l.words, 28),
        })
        .collect();
    ModelRc::new(VecModel::from(data))
}

fn parse_enhanced_lrc_words(raw: &str, _line_start: f32, line_end: f32) -> (String, Vec<LyricWordItem>) {
    let mut tags: Vec<(f32, usize, usize)> = Vec::new();
    let mut idx = 0usize;
    while let Some(rel_open) = raw[idx..].find('<') {
        let open = idx + rel_open;
        if let Some(rel_close) = raw[open + 1..].find('>') {
            let close = open + 1 + rel_close;
            let inner = &raw[open + 1..close];
            if let Some((m_s, s_s)) = inner.split_once(':') {
                if let (Ok(m), Ok(s)) = (m_s.parse::<f32>(), s_s.parse::<f32>()) {
                    tags.push((m * 60.0 + s, open, close + 1));
                    idx = close + 1;
                    continue;
                }
            }
        }
        idx = open + 1;
    }

    if tags.is_empty() {
        return (raw.trim().to_string(), Vec::new());
    }

    let mut words = Vec::new();
    let mut clean_parts = Vec::new();
    for i in 0..tags.len() {
        let (w_start, _, end_tag) = tags[i];
        let next_start_idx = tags.get(i + 1).map(|t| t.1).unwrap_or(raw.len());
        let w_text = raw[end_tag..next_start_idx].trim();
        if !w_text.is_empty() {
            let w_end = tags
                .get(i + 1)
                .map(|t| t.0)
                .unwrap_or_else(|| if line_end > w_start { line_end } else { w_start + 0.5 });
            clean_parts.push(w_text.to_string());
            words.push(LyricWordItem {
                text: w_text.to_string(),
                start_sec: w_start,
                end_sec: w_end.max(w_start + 0.08),
                is_past: false,
                is_active: false,
                progress: 0.0,
            });
        }
    }
    (clean_parts.join(" "), words)
}

fn estimate_words_for_line(text: &str, line_start: f32, line_end: f32) -> Vec<LyricWordItem> {
    let tokens: Vec<&str> = text.split_whitespace().filter(|s| !s.is_empty()).collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let line_dur = (line_end - line_start).max(0.4);
    let mut weights = Vec::with_capacity(tokens.len());
    let mut sum_w = 0.0f32;
    for tok in &tokens {
        let clean_len = tok.chars().filter(|c| c.is_alphanumeric()).count() as f32;
        let mut w = (clean_len * 0.55).max(1.4);
        if tok.ends_with('.') || tok.ends_with('!') || tok.ends_with('?') || tok.ends_with('…') {
            w += 2.5;
        } else if tok.ends_with(',') || tok.ends_with(';') || tok.ends_with(':') {
            w += 1.4;
        }
        sum_w += w;
        weights.push(w);
    }

    let mut out = Vec::with_capacity(tokens.len());
    let mut cur_t = line_start;
    for (i, tok) in tokens.into_iter().enumerate() {
        let dur = ((weights[i] / sum_w.max(0.1)) * line_dur).max(0.08);
        let w_start = cur_t;
        let w_end = cur_t + dur;
        cur_t = w_end;
        out.push(LyricWordItem {
            text: tok.to_string(),
            start_sec: w_start,
            end_sec: w_end,
            is_past: false,
            is_active: false,
            progress: 0.0,
        });
    }
    out
}

fn parse_lrc_timestamp(tag: &str) -> Option<f32> {
    let parts: Vec<&str> = tag.split(':').collect();
    match parts.len() {
        2 => {
            let min = parts[0].trim().parse::<f64>().ok()?;
            let sec = parts[1].trim().parse::<f64>().ok()?;
            Some((min * 60.0 + sec) as f32)
        }
        3 => {
            // [mm:ss:xx] format (centiseconds after second colon)
            let min = parts[0].trim().parse::<f64>().ok()?;
            let sec = parts[1].trim().parse::<f64>().ok()?;
            let frac_str = parts[2].trim();
            let frac = frac_str.parse::<f64>().ok()?;
            let div = if frac_str.len() >= 3 { 1000.0 } else { 100.0 };
            Some((min * 60.0 + sec + frac / div) as f32)
        }
        _ => None,
    }
}

/// Parse LRC formatted synchronized lyrics (supports multi-timestamps, [mm:ss:xx], [offset:ms], and auto gaps)
fn parse_lrc(text: &str, duration_sec: f64) -> Vec<LyricLineItem> {
    let mut raw_entries: Vec<(f32, String)> = Vec::new();
    let mut has_timestamps = false;
    let mut offset_sec = 0.0f32;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Check [offset:+/-ms] header tag
        if let Some(rest) = trimmed.strip_prefix("[offset:") {
            if let Some(end) = rest.find(']') {
                if let Ok(ms) = rest[..end].trim().parse::<f32>() {
                    offset_sec = ms / 1000.0;
                }
            }
            continue;
        }

        // Extract all leading [mm:ss.xx] timestamps on this line
        let mut cursor = trimmed;
        let mut line_times: Vec<f32> = Vec::new();
        while cursor.starts_with('[') {
            if let Some(close_bracket) = cursor.find(']') {
                let inner = &cursor[1..close_bracket];
                if let Some(t) = parse_lrc_timestamp(inner) {
                    line_times.push(t);
                    cursor = cursor[close_bracket + 1..].trim_start();
                    continue;
                }
            }
            break;
        }

        if !line_times.is_empty() {
            has_timestamps = true;
            let text_part = cursor.trim().to_string();
            for t in line_times {
                raw_entries.push(((t - offset_sec).max(0.0), text_part.clone()));
            }
        }
    }

    if has_timestamps && !raw_entries.is_empty() {
        raw_entries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut lines = Vec::with_capacity(raw_entries.len() + 4);

        // Insert leading instrumental gap if first lyric starts >= 6s (matches origin lyrics.js)
        if raw_entries[0].0 >= 6.0 {
            lines.push(LyricLineItem {
                time_sec: 0.0,
                time_str: format_duration(0.0),
                text: String::new(),
                is_active: false,
                is_past: false,
                is_gap: true,
                is_synced: true,
                words: Vec::new(),
            });
        }

        for i in 0..raw_entries.len() {
            let (start, ref raw_txt) = raw_entries[i];
            let next_start = raw_entries
                .get(i + 1)
                .map(|e| e.0)
                .unwrap_or_else(|| {
                    if duration_sec as f32 > start + 0.5 {
                        (duration_sec as f32).min(start + 6.0)
                    } else {
                        start + 4.0
                    }
                });
            let line_end = next_start.max(start + 0.5);
            let (clean_txt, enh_words) = parse_enhanced_lrc_words(raw_txt, start, line_end);
            let gap = clean_txt.trim().is_empty() || is_gap_text(&clean_txt);
            let words = if gap {
                Vec::new()
            } else if !enh_words.is_empty() {
                enh_words
            } else {
                estimate_words_for_line(&clean_txt, start, line_end.min(start + 5.0))
            };
            lines.push(LyricLineItem {
                time_sec: start,
                time_str: format_duration(start as f64),
                text: if gap { String::new() } else { clean_txt },
                is_active: false,
                is_past: false,
                is_gap: gap,
                is_synced: true,
                words,
            });

            // Insert instrumental gap if pause between lines >= 6s
            if !gap {
                if let Some((nxt_t, nxt_txt)) = raw_entries.get(i + 1) {
                    let est_end = (start + 4.0).min(*nxt_t);
                    if !nxt_txt.trim().is_empty() && !is_gap_text(nxt_txt) && (*nxt_t - est_end) >= 6.0 {
                        lines.push(LyricLineItem {
                            time_sec: est_end,
                            time_str: format_duration(est_end as f64),
                            text: String::new(),
                            is_active: false,
                            is_past: false,
                            is_gap: true,
                            is_synced: true,
                            words: Vec::new(),
                        });
                    }
                }
            }
        }
        return lines;
    }

    let raw_lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && (!l.starts_with('[') || is_gap_text(l)) && !l.starts_with('{'))
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
            let gap = is_gap_text(l);
            let words = if gap {
                Vec::new()
            } else {
                estimate_words_for_line(l, t, t + interval)
            };
            LyricLineItem {
                time_sec: t,
                time_str: format_duration(t as f64),
                text: l.to_string(),
                is_active: false,
                is_past: false,
                is_gap: gap,
                is_synced: false,
                words,
            }
        })
        .collect()
}

fn norm_lyric_sim(a: &str, b: &str) -> f32 {
    let na: String = a.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect();
    let nb: String = b.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect();
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }
    if na.contains(&nb) || nb.contains(&na) {
        return (na.len().min(nb.len()) as f32) / (na.len().max(nb.len()) as f32);
    }
    0.0
}

fn merge_netease_words(lines: &mut [LyricLineItem], yrc_lines: &[riff_core::lyrics::WordLine]) -> bool {
    if lines.is_empty() || yrc_lines.is_empty() {
        return false;
    }
    let mut matched = 0usize;
    let mut y_idx = 0usize;
    let mut mapped: Vec<Option<Vec<LyricWordItem>>> = vec![None; lines.len()];

    for (i, line) in lines.iter().enumerate() {
        let start_j = y_idx.saturating_sub(3);
        let end_j = (y_idx + 4).min(yrc_lines.len());
        let mut best_sim = 0.0f32;
        let mut best_j = None;
        for j in start_j..end_j {
            let sim = norm_lyric_sim(&line.text, &yrc_lines[j].text);
            if sim > best_sim {
                best_sim = sim;
                best_j = Some(j);
            }
        }
        if let Some(bj) = best_j {
            if best_sim >= 0.6 {
                matched += 1;
                y_idx = bj + 1;
                let words: Vec<LyricWordItem> = yrc_lines[bj]
                    .words
                    .iter()
                    .map(|w| LyricWordItem {
                        text: w.text.clone(),
                        start_sec: w.start as f32,
                        end_sec: w.end as f32,
                        is_past: false,
                        is_active: false,
                        progress: 0.0,
                    })
                    .collect();
                mapped[i] = Some(words);
            }
        }
    }

    if (matched as f32) / (lines.len() as f32) < 0.55 {
        return false;
    }

    for (i, words_opt) in mapped.into_iter().enumerate() {
        if let Some(words) = words_opt {
            lines[i].words = words;
        }
    }
    true
}

fn update_lyrics_progress(lines: &mut [LyricLineItem], pos_f: f32) -> (i32, bool) {
    if lines.is_empty() {
        return (-1, false);
    }
    let match_t = pos_f + 0.08;
    let active_opt: Option<usize> = if match_t < lines[0].time_sec {
        None
    } else {
        let mut idx = 0usize;
        for (i, line) in lines.iter().enumerate() {
            if line.time_sec <= match_t {
                idx = i;
            } else {
                break;
            }
        }
        Some(idx)
    };

    let mut changed = false;
    for (i, line) in lines.iter_mut().enumerate() {
        let should_be_active = active_opt == Some(i);
        let should_be_past = active_opt.map(|a| i < a).unwrap_or(false);
        if line.is_active != should_be_active || line.is_past != should_be_past {
            line.is_active = should_be_active;
            line.is_past = should_be_past;
            changed = true;
        }

        if should_be_active && !line.is_gap {
            for w in &mut line.words {
                let dur = (w.end_sec - w.start_sec).max(0.05);
                let prog = ((match_t - w.start_sec) / dur).clamp(0.0, 1.0);
                let w_past = match_t >= w.end_sec;
                let w_active = match_t >= w.start_sec && match_t < w.end_sec;
                if (w.progress - prog).abs() > 0.02 || w.is_past != w_past || w.is_active != w_active {
                    w.progress = prog;
                    w.is_past = w_past;
                    w.is_active = w_active;
                    changed = true;
                }
            }
        }
    }

    (active_opt.map(|i| i as i32).unwrap_or(-1), changed)
}

fn spawn_fetch_lyrics(
    api: Arc<riff_core::api::RiffApi>,
    lyr_store: Arc<Mutex<Vec<LyricLineItem>>>,
    w_weak: slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    title: String,
    artist: String,
    duration_sec: f64,
    source_pref: String,
    word_source: String,
) {
    rt.spawn(async move {
        let val = api
            .fetch_lyrics(riff_core::lyrics::LyricsQuery {
                title: Some(title.clone()),
                artist: Some(artist.clone()),
                source: Some(source_pref.clone()),
                duration: Some(duration_sec),
                online: Some("1".to_string()),
            })
            .await;

        let raw_text = val.get("lyrics").and_then(|x| x.as_str()).unwrap_or("");
        let actual_src = val.get("source").and_then(|x| x.as_str()).unwrap_or("none").to_string();
        let mut parsed = parse_lrc(raw_text, duration_sec);
        let is_synced = val.get("isSynced").and_then(|x| x.as_bool()).unwrap_or_else(|| {
            parsed.iter().any(|l| l.is_synced)
        });

        if let Ok(mut store) = lyr_store.lock() {
            *store = parsed.clone();
        }

        let src_label = format_source_label(&actual_src).to_string();
        let w_first = w_weak.clone();
        let parsed_first = parsed.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_first.upgrade() {
                w.set_lyrics_lines(to_slint_lyrics(&parsed_first));
                w.set_lyrics_source_label(SharedString::from(&src_label));
                w.set_lyrics_is_synced(is_synced);
            }
        });

        // Upgrade with NetEase YRC word-by-word timings if enabled and synced
        if word_source != "estimate" && !parsed.is_empty() && is_synced {
            let yrc_lines = api.fetch_lyrics_words(&title, &artist, duration_sec).await;
            if !yrc_lines.is_empty() && merge_netease_words(&mut parsed, &yrc_lines) {
                if let Ok(mut store) = lyr_store.lock() {
                    *store = parsed.clone();
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_weak.upgrade() {
                        w.set_lyrics_lines(to_slint_lyrics(&parsed));
                        w.set_lyrics_source_label(SharedString::from("NetEase YRC"));
                    }
                });
            }
        }
    });
}

fn playlists_to_slint(list: &[riff_core::models::Playlist]) -> ModelRc<PlaylistData> {
    let data: Vec<PlaylistData> = list
        .iter()
        .map(|p| PlaylistData {
            id: SharedString::from(&p.id),
            name: SharedString::from(&p.name),
            track_count: p.tracks.len() as i32,
        })
        .collect();
    ModelRc::new(VecModel::from(data))
}

fn track_data_to_core(t: &TrackData) -> riff_core::models::Track {
    riff_core::models::Track {
        id: t.id.to_string(),
        path: t.id.to_string(),
        file: t.title.to_string(),
        title: t.title.to_string(),
        artist: t.artist.to_string(),
        album: if t.album.is_empty() { None } else { Some(t.album.to_string()) },
        duration: t.duration_sec as f64,
        thumbnail: t.thumbnail.to_string(),
        added_at: 0,
        updated_at: 0,
    }
}

fn track_data_to_item(t: &TrackData) -> TrackItem {
    TrackItem {
        id: t.id.to_string(),
        title: t.title.to_string(),
        artist: t.artist.to_string(),
        album: t.album.to_string(),
        duration_sec: t.duration_sec as f64,
        platform: t.platform.to_string(),
        thumbnail: t.thumbnail.to_string(),
        is_favorite: t.is_favorite,
        cover_rgba: t.cover.to_rgba8().map(|b| (b.width(), b.height(), b.as_bytes().to_vec())),
    }
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

fn detect_audio_hint(content_type: Option<&str>, url: &str) -> Option<String> {
    if let Some(ct) = content_type {
        let ct_low = ct.to_ascii_lowercase();
        if ct_low.contains("audio/mp4") || ct_low.contains("video/mp4") || ct_low.contains("mp4a") {
            return Some("m4a".to_string());
        }
        if ct_low.contains("audio/mpeg") || ct_low.contains("audio/mp3") {
            return Some("mp3".to_string());
        }
        if ct_low.contains("audio/aac") {
            return Some("aac".to_string());
        }
        if ct_low.contains("audio/ogg") || ct_low.contains("application/ogg") {
            return Some("ogg".to_string());
        }
        if ct_low.contains("audio/flac") {
            return Some("flac".to_string());
        }
        if ct_low.contains("audio/wav") || ct_low.contains("audio/x-wav") {
            return Some("wav".to_string());
        }
    }
    let url_low = url.to_ascii_lowercase();
    if url_low.contains("mime=audio%2fmp4") || url_low.contains("mime=audio/mp4") || url_low.contains(".m4a") || url_low.contains(".mp4") {
        return Some("m4a".to_string());
    }
    if url_low.contains(".mp3") {
        return Some("mp3".to_string());
    }
    if url_low.contains(".aac") {
        return Some("aac".to_string());
    }
    if url_low.contains(".ogg") {
        return Some("ogg".to_string());
    }
    if url_low.contains(".flac") {
        return Some("flac".to_string());
    }
    if url_low.contains(".wav") {
        return Some("wav".to_string());
    }
    None
}

fn googlevideo_user_agent(raw_url: &str) -> &'static str {
    if let Ok(parsed) = reqwest::Url::parse(raw_url) {
        for (k, v) in parsed.query_pairs() {
            if k == "c" {
                if v == "ANDROID_MUSIC" {
                    return "com.google.android.apps.youtube.music/7.27.52 (Linux; U; Android 14; en_US) gzip";
                } else if v == "ANDROID" {
                    return "com.google.android.youtube/20.10.38 (Linux; U; Android 14) gzip";
                } else if v == "IOS" {
                    return "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
                } else if v == "VISIONOS" || v == "WEB" || v == "WEB_REMIX" || v == "MWEB" {
                    return "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
                }
            }
        }
    }
    "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)"
}

async fn download_hls_bytes(http: &reqwest::Client, playlist_url: &str) -> Result<(Vec<u8>, Option<String>), String> {
    let base_url = reqwest::Url::parse(playlist_url).map_err(|e| format!("Invalid HLS URL: {e}"))?;
    let resp = http
        .get(playlist_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("HLS fetch failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HLS HTTP {}", resp.status()));
    }
    let text = resp.text().await.map_err(|e| format!("HLS read failed: {e}"))?;

    // If master playlist, pick first variant playlist
    if text.contains("#EXT-X-STREAM-INF") {
        for line in text.lines() {
            let t = line.trim();
            if !t.is_empty() && !t.starts_with('#') {
                if let Ok(sub_url) = base_url.join(t) {
                    return Box::pin(download_hls_bytes(http, sub_url.as_str())).await;
                }
            }
        }
    }

    let mut segment_urls: Vec<String> = Vec::new();
    let mut is_fmp4 = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("#EXT-X-MAP:") {
            if let Some(uri_pos) = t.find("URI=\"") {
                let rest = &t[uri_pos + 5..];
                if let Some(end_q) = rest.find('"') {
                    let map_uri = &rest[..end_q];
                    if let Ok(joined) = base_url.join(map_uri) {
                        segment_urls.push(joined.to_string());
                        is_fmp4 = true;
                    }
                }
            }
        } else if !t.is_empty() && !t.starts_with('#') {
            if let Ok(joined) = base_url.join(t) {
                segment_urls.push(joined.to_string());
            }
        }
    }

    if segment_urls.is_empty() {
        return Err("HLS playlist contained no segments".to_string());
    }

    let mut out = Vec::new();
    for chunk in segment_urls.chunks(6) {
        let mut futs = Vec::with_capacity(chunk.len());
        for seg_url in chunk {
            let client = http.clone();
            let u = seg_url.clone();
            futs.push(tokio::spawn(async move {
                let r = client
                    .get(&u)
                    .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !r.status().is_success() {
                    return Err(format!("Segment HTTP {}", r.status()));
                }
                r.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
            }));
        }
        for f in futs {
            let bytes = f.await.map_err(|e| e.to_string())??;
            out.extend_from_slice(&bytes);
        }
    }

    let hint = if is_fmp4 || segment_urls.iter().any(|u| u.contains(".m4s") || u.contains(".mp4")) {
        Some("m4a".to_string())
    } else if segment_urls.iter().any(|u| u.contains(".aac")) {
        Some("aac".to_string())
    } else if segment_urls.iter().any(|u| u.contains(".mp3")) {
        Some("mp3".to_string())
    } else {
        None
    };

    Ok((out, hint))
}

async fn download_stream_bytes(
    http: &reqwest::Client,
    raw_url: &str,
    is_hls: bool,
) -> Result<(Vec<u8>, Option<String>), String> {
    if is_hls || raw_url.contains(".m3u8") {
        return download_hls_bytes(http, raw_url).await;
    }

    let is_googlevideo = reqwest::Url::parse(raw_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.ends_with("googlevideo.com")))
        .unwrap_or(false);

    if is_googlevideo {
        const CHUNK_SIZE: u64 = 1_048_576;
        let ua = googlevideo_user_agent(raw_url);
        let is_web_client = reqwest::Url::parse(raw_url)
            .ok()
            .map(|u| {
                u.query_pairs()
                    .any(|(k, v)| k == "c" && matches!(v.as_ref(), "WEB" | "WEB_REMIX" | "MWEB"))
            })
            .unwrap_or(false);
        let mut out = Vec::new();
        let mut start = 0u64;
        let mut detected_hint = detect_audio_hint(None, raw_url);

        loop {
            let end = start + CHUNK_SIZE - 1;
            let mut req = http
                .get(raw_url)
                .header("User-Agent", ua)
                .header("Range", format!("bytes={start}-{end}"))
                .timeout(Duration::from_secs(20));
            if is_web_client {
                req = req
                    .header("Origin", "https://music.youtube.com")
                    .header("Referer", "https://music.youtube.com/");
            }
            let mut resp = req
                .send()
                .await
                .map_err(|e| format!("Stream chunk failed: {e}"))?;

            if resp.status().as_u16() == 403 && start == 0 {
                let mut fallback_req = http
                    .get(raw_url)
                    .header("User-Agent", ua)
                    .timeout(Duration::from_secs(30));
                if is_web_client {
                    fallback_req = fallback_req
                        .header("Origin", "https://music.youtube.com")
                        .header("Referer", "https://music.youtube.com/");
                }
                resp = fallback_req
                    .send()
                    .await
                    .map_err(|e| format!("Stream fallback failed: {e}"))?;
            }

            let status = resp.status();
            if !status.is_success() && status.as_u16() != 206 {
                return Err(format!("Stream HTTP {status}"));
            }

            if detected_hint.is_none() {
                let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok());
                detected_hint = detect_audio_hint(ct, raw_url);
            }

            let total_len = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|cr| cr.rsplit('/').next())
                .and_then(|s| s.parse::<u64>().ok());

            let chunk = resp.bytes().await.map_err(|e| format!("Chunk read failed: {e}"))?;
            if chunk.is_empty() {
                break;
            }
            let chunk_len = chunk.len() as u64;
            out.extend_from_slice(&chunk);
            start += chunk_len;

            if let Some(total) = total_len {
                if start >= total {
                    break;
                }
            } else if status.as_u16() == 200 || chunk_len < CHUNK_SIZE {
                break;
            }
        }

        if out.is_empty() {
            return Err("Empty audio stream".to_string());
        }
        return Ok((out, detected_hint));
    }

    let resp = http
        .get(raw_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Stream request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Stream HTTP {}", resp.status()));
    }

    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let hint = detect_audio_hint(ct.as_deref(), raw_url);
    let bytes = resp.bytes().await.map_err(|e| format!("Stream read failed: {e}"))?;
    if bytes.is_empty() {
        return Err("Empty audio stream".to_string());
    }
    Ok((bytes.to_vec(), hint))
}

fn show_toast(w_weak: &slint::Weak<MainWindow>, rt: &tokio::runtime::Handle, msg: impl Into<String>) {
    let text = msg.into();
    let w_show = w_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = w_show.upgrade() {
            w.set_toast_text(SharedString::from(&text));
            w.set_toast_open(true);
            w.set_status_message(SharedString::from(&text));
        }
    });
    let w_hide = w_weak.clone();
    rt.spawn(async move {
        tokio::time::sleep(Duration::from_millis(2600)).await;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_hide.upgrade() {
                w.set_toast_open(false);
            }
        });
    });
}

async fn load_home_sections(
    api: &riff_core::api::RiffApi,
    http: &reqwest::Client,
    fav_ids: &HashSet<String>,
) -> (Vec<TrackItem>, Vec<TrackItem>) {
    let (fy_res, tr_res) = tokio::join!(
        api.search("popular pop songs 2026", "ytmusic", 14, 0),
        api.search("trending viral songs 2026", "ytmusic", 14, 0),
    );
    let filter_songs = |tracks: Vec<serde_json::Value>| -> Vec<serde_json::Value> {
        let filtered: Vec<serde_json::Value> = tracks
            .iter()
            .filter(|t| {
                let dur = t.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0);
                dur == 0.0 || dur <= 900.0
            })
            .take(8)
            .cloned()
            .collect();
        if filtered.is_empty() {
            tracks.into_iter().take(8).collect()
        } else {
            filtered
        }
    };
    let for_you_raw = filter_songs(fy_res.map(|(t, _)| t).unwrap_or_default());
    let trending_raw = filter_songs(tr_res.map(|(t, _)| t).unwrap_or_default());

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

    let active_lyrics_cache: Arc<Mutex<Vec<LyricLineItem>>> = Arc::new(Mutex::new(Vec::new()));
    let playlist_cache: Arc<Mutex<Vec<TrackItem>>> = Arc::new(Mutex::new(Vec::new()));
    let current_index: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let shuffle_state = Arc::new(AtomicBool::new(false));
    let repeat_state = Arc::new(AtomicU8::new(0)); // 0: off, 1: all, 2: one
    let sync_lines_cache: Arc<Mutex<Vec<SyncLineData>>> = Arc::new(Mutex::new(Vec::new()));
    let pending_playlist_track: Arc<Mutex<Option<TrackItem>>> = Arc::new(Mutex::new(None));
    let sleep_deadline: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let sleep_track_end = Arc::new(AtomicBool::new(false));
    let eof_handled = Arc::new(AtomicBool::new(false));
    let play_generation = Arc::new(AtomicU64::new(0));
    let lyrics_offset_ms = Arc::new(AtomicI32::new(0));
    let nav_history = Arc::new(Mutex::new(NavHistory::new()));
    let all_playlists_cache: Arc<Mutex<Vec<riff_core::models::Playlist>>> = Arc::new(Mutex::new(Vec::new()));
    let sb_filter_query: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let sb_sort_mode: Arc<Mutex<String>> = Arc::new(Mutex::new("Recents".to_string()));
    let search_history_cache: Arc<Mutex<Vec<SavedSearchEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let all_library_cache: Arc<Mutex<Vec<TrackItem>>> = Arc::new(Mutex::new(Vec::new()));
    let local_filter_query: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let local_sort_mode: Arc<Mutex<String>> = Arc::new(Mutex::new("Default".to_string()));
    let all_favorites_cache: Arc<Mutex<Vec<TrackItem>>> = Arc::new(Mutex::new(Vec::new()));
    let fav_filter_query: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let fav_sort_mode: Arc<Mutex<String>> = Arc::new(Mutex::new("Default".to_string()));

    // 1. Initial Data Load (Library, Favorites, History, Playlists, Home Feed)
    let api_init = api.clone();
    let http_init = http.clone();
    let w_init = window_weak.clone();
    let pl_init = playlist_cache.clone();
    let off_init = lyrics_offset_ms.clone();
    let all_pls_init = all_playlists_cache.clone();
    let sh_init = search_history_cache.clone();
    let all_lib_init = all_library_cache.clone();
    let all_fav_init = all_favorites_cache.clone();

    rt_handle.spawn(async move {
        let (lib_res, fav_res, hist_res, pls_res, kv_res) = tokio::join!(
            api_init.list_library(),
            api_init.get_favorites(),
            api_init.get_history(),
            api_init.get_playlists(),
            api_init.get_all_settings(),
        );
        let lib_tracks = lib_res.unwrap_or_default();
        let fav_tracks = fav_res.unwrap_or_default();
        let hist_tracks = hist_res.unwrap_or_default();
        let user_playlists = pls_res.unwrap_or_default();
        let kv_map = kv_res.unwrap_or_default();

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
        let mut fav_items: Vec<TrackItem> = fav_tracks
            .iter()
            .map(|t| TrackItem::from_core(t, infer_platform_for_track(t), true))
            .collect();
        let mut hist_items: Vec<TrackItem> = hist_tracks
            .iter()
            .map(|t| TrackItem::from_core(t, infer_platform_for_track(t), fav_ids.contains(&t.id)))
            .collect();

        let saved_sh: Vec<SavedSearchEntry> = kv_map
            .get("slint_search_history")
            .and_then(|s| serde_json::from_str::<Vec<SavedSearchEntry>>(s).ok())
            .unwrap_or_default();

        let (_, _, sh_slots) = tokio::join!(
            hydrate_track_covers(&mut fav_items, &http_init),
            hydrate_track_covers(&mut hist_items, &http_init),
            hydrate_search_history(&saved_sh, &http_init),
        );

        if let Ok(mut pl) = pl_init.lock() {
            *pl = lib_items.clone();
        }
        if let Ok(mut pls) = all_pls_init.lock() {
            *pls = user_playlists.clone();
        }
        if let Ok(mut sh) = sh_init.lock() {
            *sh = saved_sh;
        }
        if let Ok(mut lc) = all_lib_init.lock() {
            *lc = lib_items.clone();
        }
        if let Ok(mut fc) = all_fav_init.lock() {
            *fc = fav_items.clone();
        }

        let queue_init = lib_items.clone();
        let w_first = w_init.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w_first.upgrade() {
                let theme = w.global::<Theme>();
                if let Some(v) = kv_map.get("slint_sidebar_collapsed") {
                    w.set_sidebar_collapsed(v == "1");
                }
                if let Some(v) = kv_map.get("slint_show_visualizer") {
                    w.set_show_visualizer(v != "0");
                }
                if let Some(v) = kv_map.get("slint_visualizer_opacity").and_then(|s| s.parse::<f32>().ok()) {
                    w.set_visualizer_opacity((v / 100.0).clamp(0.02, 0.30));
                }
                if let Some(v) = kv_map.get("slint_auto_hide_titlebar") {
                    w.set_auto_hide_titlebar(v != "0");
                }
                if let Some(v) = kv_map.get("slint_show_timestamps") {
                    w.set_show_timestamps(v != "0");
                }
                if let Some(v) = kv_map.get("slint_rp_banner") {
                    w.set_rp_banner_enabled(v != "0");
                }
                if let Some(v) = kv_map.get("slint_rp_gradient") {
                    w.set_rp_gradient_enabled(v != "0");
                }
                if let Some(v) = kv_map.get("slint_tvk_blobs") {
                    w.set_tvk_blobs_enabled(v != "0");
                }
                if let Some(v) = kv_map.get("slint_search_platform") {
                    w.set_search_platform(SharedString::from(v.as_str()));
                }
                if let Some(v) = kv_map.get("slint_lyrics_source") {
                    w.set_lyrics_source(SharedString::from(v.as_str()));
                    w.set_lyrics_source_label(SharedString::from(format_source_label(v)));
                }
                if let Some(v) = kv_map.get("slint_word_source") {
                    w.set_lyrics_word_source(SharedString::from(v.as_str()));
                }
                if let Some(v) = kv_map.get("slint_lyrics_mode") {
                    w.set_lyrics_mode(SharedString::from(v.as_str()));
                }
                if let Some(v) = kv_map.get("slint_lyrics_word_style") {
                    w.set_lyrics_word_style(SharedString::from(v.as_str()));
                }
                if let Some(v) = kv_map.get("slint_lyrics_warnings") {
                    w.set_lyrics_warnings(v != "0");
                }
                if let Some(v) = kv_map.get("slint_lyrics_autoscroll") {
                    w.set_lyrics_autoscroll(v != "0");
                }
                if let Some(v) = kv_map.get("slint_lyrics_fullscreen_only") {
                    w.set_lyrics_fullscreen_only(v == "1");
                }
                if let Some(v) = kv_map.get("slint_lyrics_speed").and_then(|s| s.parse::<f32>().ok()) {
                    w.set_lyrics_speed((v / 10.0).clamp(0.5, 2.5));
                }
                if let Some(v) = kv_map.get("slint_lyrics_tightness").and_then(|s| s.parse::<f32>().ok()) {
                    w.set_lyrics_tightness(v.clamp(2.0, 24.0));
                }
                if let Some(v) = kv_map.get("slint_lyrics_ease") {
                    w.set_lyrics_ease(SharedString::from(v.as_str()));
                }
                if let Some(v) = kv_map.get("slint_lyrics_offset_ms").and_then(|s| s.parse::<i32>().ok()) {
                    let clamped = v.clamp(-5000, 5000);
                    off_init.store(clamped, Ordering::SeqCst);
                    w.set_lyrics_offset_ms(clamped as f32);
                }
                if let Some(v) = kv_map.get("slint_ambient_blur") {
                    theme.set_ambient_blur_enabled(v != "0");
                }
                if let Some(v) = kv_map.get("slint_dynamic_accent") {
                    theme.set_dynamic_accent_enabled(v != "0");
                }
                if let Some(v) = kv_map.get("slint_reduced_motion") {
                    theme.set_reduced_motion(v == "1");
                }
                if let Some(v) = kv_map.get("slint_panel_opacity").and_then(|s| s.parse::<f32>().ok()) {
                    theme.set_panel_opacity((v / 100.0).clamp(0.50, 1.0));
                }
                if let Some(acc) = kv_map.get("slint_accent") {
                    let (col, on) = match acc.as_str() {
                        "indigo" => (slint::Color::from_rgb_u8(0x7c, 0x8c, 0xff), slint::Color::from_rgb_u8(0x12, 0x18, 0x4a)),
                        "blue" => (slint::Color::from_rgb_u8(0x64, 0xb5, 0xf6), slint::Color::from_rgb_u8(0x0d, 0x2b, 0x45)),
                        "teal" => (slint::Color::from_rgb_u8(0x4d, 0xb6, 0xac), slint::Color::from_rgb_u8(0x0b, 0x33, 0x2f)),
                        "green" => (slint::Color::from_rgb_u8(0x81, 0xc7, 0x84), slint::Color::from_rgb_u8(0x12, 0x38, 0x15)),
                        "amber" => (slint::Color::from_rgb_u8(0xff, 0xb7, 0x4d), slint::Color::from_rgb_u8(0x3d, 0x26, 0x00)),
                        "rose" => (slint::Color::from_rgb_u8(0xf0, 0x62, 0x92), slint::Color::from_rgb_u8(0x4a, 0x0d, 0x24)),
                        "mono" => (slint::Color::from_rgb_u8(0xe2, 0xe2, 0xe9), slint::Color::from_rgb_u8(0x12, 0x12, 0x14)),
                        _ => (slint::Color::from_rgb_u8(0x9f, 0x99, 0xe6), slint::Color::from_rgb_u8(0x1e, 0x1a, 0x4d)),
                    };
                    theme.set_custom_accent(col);
                    theme.set_custom_on_accent(on);
                    theme.set_primary(col);
                    theme.set_on_primary(on);
                }

                w.set_library_tracks(to_slint_model(lib_items));
                w.set_favorite_tracks(to_slint_model(fav_items));
                w.set_history_tracks(to_slint_model(hist_items));
                w.set_queue_tracks(to_slint_model(queue_init));
                w.set_playlists(playlists_to_slint(&user_playlists));
                w.set_search_history(to_slint_search_history(sh_slots));
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
    let eof_play = eof_handled.clone();
    let gen_play = play_generation.clone();
    let rt_play = rt_handle.clone();

    main_window.on_play_track(move |track| {
        eof_play.store(false, Ordering::SeqCst);
        let my_gen = gen_play.fetch_add(1, Ordering::SeqCst) + 1;
        let t_thumb = track.thumbnail.to_string();
        let has_cov = track.has_cover;

        if let Some(w) = w_play.upgrade() {
            w.set_current_id(track.id.clone());
            w.set_current_title(track.title.clone());
            w.set_current_artist(track.artist.clone());
            w.set_current_album(track.album.clone());
            w.set_current_platform(track.platform.clone());
            w.set_current_thumbnail(track.thumbnail.clone());
            w.set_current_is_favorite(track.is_favorite);
            w.set_active_lyric_index(-1);
            w.set_current_pos(0.0);
            w.set_pos_text(SharedString::from("0:00"));
            if track.duration_sec > 0.0 {
                w.set_current_duration(track.duration_sec);
                w.set_dur_text(SharedString::from(format_duration(track.duration_sec as f64)));
            }
            w.set_status_message(SharedString::from(format!("Loading: {}...", track.title)));
            w.set_current_has_cover(has_cov);
            if has_cov {
                w.set_current_cover(track.cover.clone());
                if let Some(buf) = track.cover.to_rgba8() {
                    if let Some((bw, bh, braw, (ar, ag, ab))) =
                        compute_ambient_blur_and_accent(buf.width(), buf.height(), buf.as_bytes())
                    {
                        w.set_current_cover_blur(rgba_to_slint_image(bw, bh, braw));
                        w.set_current_has_blur(true);
                        let theme = w.global::<Theme>();
                        if theme.get_dynamic_accent_enabled() {
                            let col = slint::Color::from_rgb_u8(ar, ag, ab);
                            w.set_current_accent(col);
                            theme.set_primary(col);
                        } else {
                            w.set_current_accent(theme.get_custom_accent());
                        }
                    }
                }
            } else {
                w.set_current_has_blur(false);
                let theme = w.global::<Theme>();
                w.set_current_accent(theme.get_custom_accent());
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
                                let theme = w.global::<Theme>();
                                if theme.get_dynamic_accent_enabled() {
                                    let col = slint::Color::from_rgb_u8(ar, ag, ab);
                                    w.set_current_accent(col);
                                    theme.set_primary(col);
                                } else {
                                    w.set_current_accent(theme.get_custom_accent());
                                }
                            }
                        }
                    });
                }
            });
        }

        if let (Ok(mut pl), Ok(mut idx)) = (pl_play.lock(), cur_idx_play.lock()) {
            if let Some(pos) = pl.iter().position(|t| t.id == track.id.as_str()) {
                *idx = pos;
            } else {
                pl.insert(0, track_data_to_item(&track));
                *idx = 0;
            }
            let q_clone = pl.clone();
            if let Some(w) = w_play.upgrade() {
                w.set_queue_tracks(to_slint_model(q_clone));
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
        let http_hist = http_play.clone();
        let track_for_hist = core_track.clone();
        let w_hist = w_play.clone();

        rt_play.spawn(async move {
            let _ = api_hist.push_history(&track_for_hist).await;
            if let Ok(updated_hist) = api_hist.get_history().await {
                let favs = api_hist.get_favorites().await.unwrap_or_default();
                let fav_set: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                let mut h_items: Vec<TrackItem> = updated_hist
                    .iter()
                    .map(|t| TrackItem::from_core(t, infer_platform_for_track(t), fav_set.contains(&t.id)))
                    .collect();
                hydrate_track_covers(&mut h_items, &http_hist).await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_hist.upgrade() {
                        w.set_history_tracks(to_slint_model(h_items));
                    }
                });
            }
        });

        // Fetch Synced Lyrics + Word-by-word timings
        let (src_pref, word_src) = if let Some(w) = w_play.upgrade() {
            (w.get_lyrics_source().to_string(), w.get_lyrics_word_source().to_string())
        } else {
            ("auto".to_string(), "auto".to_string())
        };
        spawn_fetch_lyrics(
            api_play.clone(),
            lyr_play.clone(),
            w_play.clone(),
            &rt_play,
            track.title.to_string(),
            track.artist.to_string(),
            track.duration_sec as f64,
            src_pref,
            word_src,
        );

        // Play audio (local file or stream)
        let path = PathBuf::from(track.id.as_str());
        if path.is_file() {
            api_play.play_file(path, Some(core_track));
            show_toast(&w_play, &rt_play, format!("Playing: {}", track.title));
        } else {
            let api_str = api_play.clone();
            let http_str = http_play.clone();
            let w_str = w_play.clone();
            let rt_str = rt_play.clone();
            let gen_str = gen_play.clone();
            let s_id = track.id.to_string();
            let s_title = track.title.to_string();
            let s_artist = track.artist.to_string();
            let s_platform = track.platform.to_string();
            let s_dur = track.duration_sec as f64;
            let ct_clone = core_track.clone();

            show_toast(&w_play, &rt_play, format!("Loading: {}...", track.title));

            rt_play.spawn(async move {
                // Handle local_* IDs stored in favorites/history/playlists
                if s_id.starts_with("local_") {
                    if let Ok(lib) = api_str.list_library().await {
                        if let Some(found) = lib.into_iter().find(|t| t.id == s_id) {
                            let p = PathBuf::from(&found.path);
                            if p.is_file() {
                                if gen_str.load(Ordering::SeqCst) == my_gen {
                                    api_str.play_file(p, Some(ct_clone));
                                    show_toast(&w_str, &rt_str, format!("Playing: {s_title}"));
                                }
                                return;
                            }
                        }
                    }
                }

                let q = riff_core::models::StreamQuery {
                    id: Some(s_id),
                    title: Some(s_title.clone()),
                    artist: Some(s_artist),
                    platform: Some(s_platform),
                    duration: Some(s_dur),
                    url: None,
                    thumbnail: None,
                    timeout: Some(20),
                    quality: Some("medium".to_string()),
                    q: None,
                };

                match api_str.resolve_stream(&q, 0).await {
                    Ok(resolved) => {
                        if gen_str.load(Ordering::SeqCst) != my_gen {
                            return;
                        }
                        let is_hls = resolved.get("isHls").and_then(|v| v.as_bool()).unwrap_or(false);
                        let raw_url = resolved
                            .get("rawStreamUrl")
                            .and_then(|v| v.as_str())
                            .or_else(|| {
                                resolved
                                    .get("streamUrl")
                                    .and_then(|v| v.as_str())
                                    .filter(|u| !u.starts_with("http://127.0.0.1:0"))
                            });

                        if let Some(url) = raw_url {
                            match download_stream_bytes(&http_str, url, is_hls).await {
                                Ok((bytes, hint)) => {
                                    if gen_str.load(Ordering::SeqCst) == my_gen {
                                        api_str.play_bytes(bytes, hint, Some(ct_clone));
                                        show_toast(&w_str, &rt_str, format!("Playing: {s_title}"));
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!("Stream download failed for '{}': {}", s_title, err);
                                    show_toast(&w_str, &rt_str, format!("Playback error: {err}"));
                                }
                            }
                        } else {
                            tracing::warn!("No playable stream URL resolved for '{}'", s_title);
                            show_toast(&w_str, &rt_str, "Playback error: No stream URL found");
                        }
                    }
                    Err(err) => {
                        tracing::warn!("resolve_stream failed for '{}': {}", s_title, err);
                        show_toast(&w_str, &rt_str, format!("Stream error: {err}"));
                    }
                }
            });
        }
    });

    // 3. Play / Pause Toggle Callback
    let api_toggle = api.clone();
    let w_toggle = window_weak.clone();
    main_window.on_toggle_play_pause(move || {
        let (playing, _, _, _) = api_toggle.get_playback_state();
        if playing {
            api_toggle.pause();
        } else if let Some(w) = w_toggle.upgrade() {
            if w.get_current_id().is_empty() {
                let first = w
                    .get_queue_tracks()
                    .iter()
                    .next()
                    .or_else(|| w.get_home_trending().iter().next())
                    .or_else(|| w.get_home_for_you().iter().next())
                    .or_else(|| w.get_history_tracks().iter().next())
                    .or_else(|| w.get_library_tracks().iter().next());
                if let Some(t) = first {
                    w.invoke_play_track(t);
                }
            } else {
                api_toggle.resume();
            }
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
    let shuf_next = shuffle_state.clone();
    let w_next = window_weak.clone();

    main_window.on_play_next(move || {
        let target = if let (Ok(pl), Ok(mut idx)) = (pl_next.lock(), cur_idx_next.lock()) {
            if pl.is_empty() {
                None
            } else {
                if shuf_next.load(Ordering::SeqCst) && pl.len() > 1 {
                    let nanos = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as usize)
                        .unwrap_or(1);
                    let mut next_i = nanos % pl.len();
                    if next_i == *idx {
                        next_i = (*idx + 1) % pl.len();
                    }
                    *idx = next_i;
                } else {
                    *idx = (*idx + 1) % pl.len();
                }
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

    // 4b. Shuffle & Repeat Callbacks
    let shuf_cb = shuffle_state.clone();
    let w_shuf = window_weak.clone();
    main_window.on_toggle_shuffle(move || {
        let next = !shuf_cb.load(Ordering::SeqCst);
        shuf_cb.store(next, Ordering::SeqCst);
        if let Some(w) = w_shuf.upgrade() {
            w.set_shuffle_on(next);
        }
    });

    let rep_cb = repeat_state.clone();
    let w_rep = window_weak.clone();
    main_window.on_cycle_repeat(move || {
        let cur = rep_cb.load(Ordering::SeqCst);
        let next = (cur + 1) % 3;
        rep_cb.store(next, Ordering::SeqCst);
        if let Some(w) = w_rep.upgrade() {
            w.set_repeat_mode(next as i32);
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
    let http_fav = http.clone();
    let w_fav = window_weak.clone();
    let rt_fav = rt_handle.clone();
    let all_fav_cur = all_favorites_cache.clone();

    main_window.on_toggle_favorite_current(move || {
        if let Some(w) = w_fav.upgrade() {
            let id = w.get_current_id().to_string();
            if id.is_empty() {
                return;
            }
            let alb = w.get_current_album().to_string();
            let track = riff_core::models::Track {
                id: id.clone(),
                path: id,
                file: w.get_current_title().to_string(),
                title: w.get_current_title().to_string(),
                artist: w.get_current_artist().to_string(),
                album: if alb.is_empty() { None } else { Some(alb) },
                duration: w.get_current_duration() as f64,
                thumbnail: w.get_current_thumbnail().to_string(),
                added_at: 0,
                updated_at: 0,
            };

            let api_f = api_fav.clone();
            let http_f = http_fav.clone();
            let w_f = w_fav.clone();
            let fc = all_fav_cur.clone();

            rt_fav.spawn(async move {
                if let Ok(is_fav) = api_f.toggle_favorite(&track).await {
                    let favs = api_f.get_favorites().await.unwrap_or_default();
                    let fav_ids: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                    let mut fav_items: Vec<TrackItem> = favs
                        .iter()
                        .map(|t| TrackItem::from_core(t, infer_platform_for_track(t), true))
                        .collect();
                    hydrate_track_covers(&mut fav_items, &http_f).await;
                    if let Ok(mut g) = fc.lock() {
                        *g = fav_items.clone();
                    }

                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_f.upgrade() {
                            w.set_current_is_favorite(is_fav);
                            w.set_favorite_tracks(to_slint_model(fav_items));
                            sync_favorite_flags(&w, &fav_ids);
                        }
                    });
                }
            });
        }
    });

    // 7b. Favorite Toggle for Any Track Row
    let api_fav_row = api.clone();
    let http_fav_row = http.clone();
    let w_fav_row = window_weak.clone();
    let rt_fav_row = rt_handle.clone();
    let all_fav_row = all_favorites_cache.clone();
    main_window.on_toggle_favorite_track(move |td| {
        let core_t = track_data_to_core(&td);
        let tid = td.id.to_string();
        let api_f = api_fav_row.clone();
        let http_f = http_fav_row.clone();
        let w_f = w_fav_row.clone();
        let fc = all_fav_row.clone();
        rt_fav_row.spawn(async move {
            if let Ok(is_fav) = api_f.toggle_favorite(&core_t).await {
                let favs = api_f.get_favorites().await.unwrap_or_default();
                let fav_ids: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                let mut fav_items: Vec<TrackItem> = favs
                    .iter()
                    .map(|t| TrackItem::from_core(t, infer_platform_for_track(t), true))
                    .collect();
                hydrate_track_covers(&mut fav_items, &http_f).await;
                if let Ok(mut g) = fc.lock() {
                    *g = fav_items.clone();
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_f.upgrade() {
                        if w.get_current_id().as_str() == tid {
                            w.set_current_is_favorite(is_fav);
                        }
                        w.set_favorite_tracks(to_slint_model(fav_items));
                        sync_favorite_flags(&w, &fav_ids);
                    }
                });
            }
        });
    });

    // 8. Search Callback (with Search History persistence + loading state)
    let api_search = api.clone();
    let http_search = http.clone();
    let w_search = window_weak.clone();
    let pl_search = playlist_cache.clone();
    let sh_search = search_history_cache.clone();
    let rt_search = rt_handle.clone();

    main_window.on_trigger_search(move |query_str, plat_str| {
        let query = query_str.trim().to_string();
        if query.is_empty() {
            return;
        }
        let plat = plat_str.to_string();
        let api_s = api_search.clone();
        let http_s = http_search.clone();
        let w_s = w_search.clone();
        let pl_s = pl_search.clone();
        let sh_s = sh_search.clone();

        if let Some(w) = w_s.upgrade() {
            w.set_is_searching(true);
            w.set_search_history_open(false);
        }

        rt_search.spawn(async move {
            let search_platform = if plat == "all" || plat.is_empty() { "ytmusic" } else { &plat };

            // Record search query in persistent Search History
            let entry = SavedSearchEntry {
                key: format!("q:{}", query.to_ascii_lowercase()),
                kind: "query".to_string(),
                title: query.clone(),
                subtitle: String::new(),
                track_id: String::new(),
                platform: search_platform.to_string(),
                thumbnail: String::new(),
                duration_sec: 0.0,
            };
            let sh_snapshot = if let Ok(mut g) = sh_s.lock() {
                g.retain(|e| e.key != entry.key);
                g.insert(0, entry);
                if g.len() > 20 {
                    g.truncate(20);
                }
                g.clone()
            } else {
                Vec::new()
            };
            if let Ok(json_str) = serde_json::to_string(&sh_snapshot) {
                let _ = api_s.set_setting("slint_search_history", &json_str).await;
            }
            let sh_slots = hydrate_search_history(&sh_snapshot, &http_s).await;

            let items = if let Ok((tracks, _)) = api_s.search(&query, search_platform, 25, 0).await {
                let favs = api_s.get_favorites().await.unwrap_or_default();
                let fav_set: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
                let items = json_tracks_to_items(tracks, &http_s, &fav_set, search_platform).await;
                if let Ok(mut pl) = pl_s.lock() {
                    *pl = items.clone();
                }
                items
            } else {
                Vec::new()
            };

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_s.upgrade() {
                    w.set_is_searching(false);
                    w.set_search_results(to_slint_model(items));
                    w.set_search_history(to_slint_search_history(sh_slots));
                }
            });
        });
    });

    // 9. Scan Local Music Folder Callback
    let api_scan = api.clone();
    let w_scan = window_weak.clone();
    let pl_scan = playlist_cache.clone();
    let all_lib_scan = all_library_cache.clone();
    let rt_scan = rt_handle.clone();

    main_window.on_scan_local_folder(move || {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(dir) = folder {
            let api_sc = api_scan.clone();
            let w_sc = w_scan.clone();
            let pl_sc = pl_scan.clone();
            let lc = all_lib_scan.clone();

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
                    if let Ok(mut g) = lc.lock() {
                        *g = items.clone();
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
    let all_lib_addf = all_library_cache.clone();
    let rt_addf = rt_handle.clone();

    main_window.on_add_local_files(move || {
        let files = rfd::FileDialog::new()
            .add_filter("Audio", &["mp3", "flac", "wav", "ogg", "m4a", "aac", "opus"])
            .pick_files();
        if let Some(paths) = files {
            let api_af = api_addf.clone();
            let w_af = w_addf.clone();
            let pl_af = pl_addf.clone();
            let lc = all_lib_addf.clone();

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
                    if let Ok(mut g) = lc.lock() {
                        *g = items.clone();
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

    // 10. Equalizer, Speed & Audio FX Callbacks
    let api_eq = api.clone();
    main_window.on_set_equalizer(move |bass, mid, treble| {
        api_eq.set_eq(bass, mid, treble);
    });

    let api_spd = api.clone();
    main_window.on_set_audio_speed(move |spd| {
        api_spd.set_speed(spd);
    });

    let api_fx = api.clone();
    main_window.on_set_audio_effects(move |reverb, distortion| {
        api_fx.set_effects(reverb, distortion);
    });

    let api_rst = api.clone();
    let w_rst = window_weak.clone();
    main_window.on_reset_audio_settings(move || {
        api_rst.set_eq(0.0, 0.0, 0.0);
        api_rst.set_speed(1.0);
        api_rst.set_effects(0.0, 0.0);
        api_rst.set_volume(1.0);
        if let Some(w) = w_rst.upgrade() {
            w.set_eq_bass(0.0);
            w.set_eq_mid(0.0);
            w.set_eq_treble(0.0);
            w.set_audio_speed(1.0);
            w.set_audio_reverb(0.0);
            w.set_audio_distortion(0.0);
            w.set_eq_preset_name(SharedString::from("Default"));
        }
    });

    // 11. Audio Presets Callback (matches defaultPresets in state.js)
    let api_preset = api.clone();
    let w_preset = window_weak.clone();

    main_window.on_apply_eq_preset(move |preset_name| {
        let (bass, mid, treble, speed, reverb, distortion, vol) = match preset_name.as_str() {
            "p_default" | "flat" => (0.0f32, 0.0f32, 0.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "p_speedup" => (0.0f32, 0.0f32, 0.0f32, 1.35f32, 0.0f32, 0.0f32, 1.0f32),
            "p_speedup_pitch" => (0.0f32, 0.0f32, 0.0f32, 1.30f32, 0.0f32, 0.0f32, 1.0f32),
            "p_slowed" => (2.5f32, -1.0f32, -3.0f32, 0.82f32, 65.0f32, 0.0f32, 1.0f32),
            "p_nightcore" => (1.0f32, 1.0f32, 3.0f32, 1.25f32, 20.0f32, 0.0f32, 1.0f32),
            "p_distortion" => (5.0f32, 2.0f32, 4.0f32, 1.0f32, 0.0f32, 75.0f32, 1.0f32),
            "bass_boost" => (6.0f32, 0.0f32, -1.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "vocal" => (-2.0f32, 4.0f32, 2.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "rock" => (4.0f32, -1.0f32, 3.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            "treble_boost" => (-1.0f32, 1.0f32, 5.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
            _ => (0.0f32, 0.0f32, 0.0f32, 1.0f32, 0.0f32, 0.0f32, 1.0f32),
        };
        api_preset.set_eq(bass, mid, treble);
        api_preset.set_speed(speed);
        api_preset.set_effects(reverb, distortion);
        let _ = vol;

        if let Some(w) = w_preset.upgrade() {
            w.set_eq_bass(bass);
            w.set_eq_mid(mid);
            w.set_eq_treble(treble);
            w.set_audio_speed(speed);
            w.set_audio_reverb(reverb);
            w.set_audio_distortion(distortion);
        }
    });

    // 11b. Sleep Timer Callbacks
    let sd_set = sleep_deadline.clone();
    let ste_set = sleep_track_end.clone();
    let w_sleep = window_weak.clone();
    main_window.on_set_sleep_minutes(move |mins| {
        if mins == 0 {
            if let Ok(mut d) = sd_set.lock() { *d = None; }
            ste_set.store(false, Ordering::SeqCst);
            if let Some(w) = w_sleep.upgrade() {
                w.set_sleep_active(false);
                w.set_sleep_status(SharedString::from("00:00"));
            }
        } else if mins < 0 {
            if let Ok(mut d) = sd_set.lock() { *d = None; }
            ste_set.store(true, Ordering::SeqCst);
            if let Some(w) = w_sleep.upgrade() {
                w.set_sleep_active(true);
                w.set_sleep_status(SharedString::from("Track end"));
            }
        } else {
            ste_set.store(false, Ordering::SeqCst);
            if let Ok(mut d) = sd_set.lock() {
                *d = Some(Instant::now() + Duration::from_secs((mins as u64) * 60));
            }
            if let Some(w) = w_sleep.upgrade() {
                w.set_sleep_active(true);
                w.set_sleep_status(SharedString::from(format!("{mins:02}:00")));
            }
        }
    });

    let sd_add = sleep_deadline.clone();
    let w_sleep_add = window_weak.clone();
    main_window.on_add_sleep_5m(move || {
        if let Ok(mut d) = sd_add.lock() {
            let base = d.unwrap_or_else(Instant::now);
            let next = base + Duration::from_secs(300);
            *d = Some(next);
            let rem = next.saturating_duration_since(Instant::now()).as_secs();
            if let Some(w) = w_sleep_add.upgrade() {
                w.set_sleep_active(true);
                w.set_sleep_status(SharedString::from(format!("{:02}:{:02}", rem / 60, rem % 60)));
            }
        }
    });

    // 11c. Custom Lyrics Editor Callbacks
    let lyr_open_ed = active_lyrics_cache.clone();
    let w_open_ed = window_weak.clone();
    main_window.on_open_custom_lyrics(move || {
        if let Some(w) = w_open_ed.upgrade() {
            let text = if let Ok(lyr) = lyr_open_ed.lock() {
                lyr.iter()
                    .map(|l| if l.is_gap { "[Instrumental]".to_string() } else { l.text.to_string() })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };
            w.set_custom_lyrics_text(SharedString::from(text));
            w.set_custom_lyrics_open(true);
        }
    });

    let api_save_ed = api.clone();
    let lyr_save_ed = active_lyrics_cache.clone();
    let w_save_ed = window_weak.clone();
    let rt_save_ed = rt_handle.clone();
    main_window.on_save_custom_lyrics(move |raw_text| {
        if let Some(w) = w_save_ed.upgrade() {
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            if title.is_empty() || w.get_current_id().is_empty() {
                w.set_custom_lyrics_open(false);
                return;
            }

            let new_lines: Vec<String> = raw_text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();

            let text_to_save = if let Ok(old) = lyr_save_ed.lock() {
                let has_sync = old.iter().any(|l| l.time_sec >= 0.0);
                if has_sync && !old.is_empty() && old.len() == new_lines.len() {
                    new_lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| format!("{}{}", format_lrc_tag(old[i].time_sec), line))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    raw_text.trim().to_string()
                }
            } else {
                raw_text.trim().to_string()
            };

            let parsed = parse_lrc(&text_to_save, dur);
            let is_synced = parsed.iter().any(|l| l.is_synced);
            if let Ok(mut store) = lyr_save_ed.lock() {
                *store = parsed.clone();
            }
            w.set_lyrics_lines(to_slint_lyrics(&parsed));
            w.set_lyrics_source_label(SharedString::from("Custom"));
            w.set_lyrics_is_synced(is_synced);
            w.set_active_lyric_index(-1);
            w.set_custom_lyrics_open(false);

            let api_s = api_save_ed.clone();
            rt_save_ed.spawn(async move {
                let _ = api_s.save_custom_lyrics(&title, &artist, &text_to_save).await;
            });
        }
    });

    // 11d. Tap-to-Sync Lyrics Callbacks
    let lyr_sync_open = active_lyrics_cache.clone();
    let sync_cache_open = sync_lines_cache.clone();
    let api_sync_open = api.clone();
    let w_sync_open = window_weak.clone();
    main_window.on_open_sync_lyrics(move || {
        if let Some(w) = w_sync_open.upgrade() {
            if w.get_current_id().is_empty() {
                return;
            }
            let lines: Vec<SyncLineData> = if let Ok(lyr) = lyr_sync_open.lock() {
                lyr.iter()
                    .enumerate()
                    .map(|(i, l)| SyncLineData {
                        text: SharedString::from(&l.text),
                        time_sec: -1.0,
                        time_str: SharedString::from(""),
                        is_synced: false,
                        is_next: i == 0,
                        is_gap: l.is_gap,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if lines.is_empty() {
                w.invoke_open_custom_lyrics();
                return;
            }
            if let Ok(mut sc) = sync_cache_open.lock() {
                *sc = lines.clone();
            }
            w.set_sync_lines(ModelRc::new(VecModel::from(lines)));
            w.set_sync_synced_count(0);
            w.set_sync_lyrics_open(true);
            api_sync_open.seek(0.0);
            api_sync_open.resume();
        }
    });

    let sync_cache_tap = sync_lines_cache.clone();
    let api_sync_tap = api.clone();
    let w_sync_tap = window_weak.clone();
    main_window.on_tap_sync_line(move |idx| {
        let (_, pos_sec, _, _) = api_sync_tap.get_playback_state();
        let t = pos_sec as f32;
        if let Ok(mut sc) = sync_cache_tap.lock() {
            let i = idx as usize;
            if i < sc.len() {
                sc[i].time_sec = t;
                sc[i].time_str = SharedString::from(format_lrc_tag(t));
                sc[i].is_synced = true;
                let next_idx = ((i + 1)..sc.len()).find(|&j| !sc[j].is_synced);
                for (j, item) in sc.iter_mut().enumerate() {
                    item.is_next = Some(j) == next_idx;
                }
                let synced_cnt = sc.iter().filter(|l| l.is_synced).count() as i32;
                let cloned = sc.clone();
                if let Some(w) = w_sync_tap.upgrade() {
                    w.set_sync_lines(ModelRc::new(VecModel::from(cloned)));
                    w.set_sync_synced_count(synced_cnt);
                }
            }
        }
    });

    let sync_cache_next = sync_lines_cache.clone();
    let w_sync_next = window_weak.clone();
    main_window.on_tap_sync_next(move || {
        let next_idx = if let Ok(sc) = sync_cache_next.lock() {
            sc.iter().position(|l| l.is_next).or_else(|| sc.iter().position(|l| !l.is_synced))
        } else {
            None
        };
        if let Some(i) = next_idx {
            if let Some(w) = w_sync_next.upgrade() {
                w.invoke_tap_sync_line(i as i32);
            }
        }
    });

    let sync_cache_rst = sync_lines_cache.clone();
    let api_sync_rst = api.clone();
    let w_sync_rst = window_weak.clone();
    main_window.on_restart_sync_lyrics(move || {
        api_sync_rst.seek(0.0);
        if let Ok(mut sc) = sync_cache_rst.lock() {
            for (i, item) in sc.iter_mut().enumerate() {
                item.time_sec = -1.0;
                item.time_str = SharedString::from("");
                item.is_synced = false;
                item.is_next = i == 0;
            }
            let cloned = sc.clone();
            if let Some(w) = w_sync_rst.upgrade() {
                w.set_sync_lines(ModelRc::new(VecModel::from(cloned)));
                w.set_sync_synced_count(0);
            }
        }
    });

    let sync_cache_save = sync_lines_cache.clone();
    let lyr_sync_save = active_lyrics_cache.clone();
    let api_sync_save = api.clone();
    let w_sync_save = window_weak.clone();
    let rt_sync_save = rt_handle.clone();
    main_window.on_save_sync_lyrics(move || {
        if let Some(w) = w_sync_save.upgrade() {
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            let lrc_content = if let Ok(sc) = sync_cache_save.lock() {
                sc.iter()
                    .map(|l| {
                        let txt = if l.is_gap { "[Instrumental]" } else { l.text.as_str() };
                        if l.is_synced && l.time_sec >= 0.0 {
                            format!("{}{}", format_lrc_tag(l.time_sec), txt)
                        } else {
                            txt.to_string()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                String::new()
            };

            let parsed = parse_lrc(&lrc_content, dur);
            let is_synced = parsed.iter().any(|l| l.is_synced);
            if let Ok(mut store) = lyr_sync_save.lock() {
                *store = parsed.clone();
            }
            w.set_lyrics_lines(to_slint_lyrics(&parsed));
            w.set_lyrics_source_label(SharedString::from("Custom"));
            w.set_lyrics_is_synced(is_synced);
            w.set_active_lyric_index(-1);
            w.set_sync_lyrics_open(false);

            let api_s = api_sync_save.clone();
            rt_sync_save.spawn(async move {
                let _ = api_s.save_custom_lyrics(&title, &artist, &lrc_content).await;
            });
        }
    });

    // 11e. Playlists & Queue Callbacks
    let api_cpl = api.clone();
    let w_cpl = window_weak.clone();
    let rt_cpl = rt_handle.clone();
    let all_pls_cpl = all_playlists_cache.clone();
    let nav_cpl = nav_history.clone();
    main_window.on_create_playlist(move |name| {
        let api_c = api_cpl.clone();
        let w_c = w_cpl.clone();
        let n = name.to_string();
        let apc = all_pls_cpl.clone();
        let nh = nav_cpl.clone();
        rt_cpl.spawn(async move {
            if let Ok(created) = api_c.create_playlist(&n).await {
                let all = api_c.get_playlists().await.unwrap_or_default();
                if let Ok(mut g) = apc.lock() {
                    *g = all.clone();
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_c.upgrade() {
                        w.set_playlists(playlists_to_slint(&all));
                        w.set_active_playlist_id(SharedString::from(&created.id));
                        w.set_active_playlist_name(SharedString::from(&created.name));
                        w.set_active_playlist_tracks(ModelRc::new(VecModel::from(Vec::new())));
                        if let Ok(mut nav) = nh.lock() {
                            nav.push(6);
                            w.set_can_go_back(nav.can_back());
                            w.set_can_go_forward(nav.can_forward());
                        }
                        w.set_active_tab(6);
                    }
                });
            }
        });
    });

    let api_opl = api.clone();
    let http_opl = http.clone();
    let w_opl = window_weak.clone();
    let rt_opl = rt_handle.clone();
    let nav_opl = nav_history.clone();
    main_window.on_open_playlist(move |pid| {
        let api_o = api_opl.clone();
        let http_o = http_opl.clone();
        let w_o = w_opl.clone();
        let id = pid.to_string();
        let nh = nav_opl.clone();
        rt_opl.spawn(async move {
            let all = api_o.get_playlists().await.unwrap_or_default();
            let favs = api_o.get_favorites().await.unwrap_or_default();
            let fav_ids: HashSet<String> = favs.iter().map(|t| t.id.clone()).collect();
            if let Some(pl) = all.into_iter().find(|p| p.id == id) {
                let mut items: Vec<TrackItem> = pl
                    .tracks
                    .iter()
                    .map(|t| TrackItem::from_core(t, infer_platform_for_track(t), fav_ids.contains(&t.id)))
                    .collect();
                hydrate_track_covers(&mut items, &http_o).await;
                let pl_id = pl.id;
                let pl_name = pl.name;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_o.upgrade() {
                        w.set_active_playlist_id(SharedString::from(&pl_id));
                        w.set_active_playlist_name(SharedString::from(&pl_name));
                        w.set_active_playlist_tracks(to_slint_model(items));
                        if let Ok(mut nav) = nh.lock() {
                            nav.push(6);
                            w.set_can_go_back(nav.can_back());
                            w.set_can_go_forward(nav.can_forward());
                        }
                        w.set_active_tab(6);
                    }
                });
            }
        });
    });

    let api_dpl = api.clone();
    let w_dpl = window_weak.clone();
    let rt_dpl = rt_handle.clone();
    let all_pls_dpl = all_playlists_cache.clone();
    let nav_dpl = nav_history.clone();
    main_window.on_delete_playlist(move |pid| {
        let api_d = api_dpl.clone();
        let w_d = w_dpl.clone();
        let id = pid.to_string();
        let apc = all_pls_dpl.clone();
        let nh = nav_dpl.clone();
        rt_dpl.spawn(async move {
            let _ = api_d.delete_playlist(&id).await;
            let all = api_d.get_playlists().await.unwrap_or_default();
            if let Ok(mut g) = apc.lock() {
                *g = all.clone();
            }
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_d.upgrade() {
                    w.set_playlists(playlists_to_slint(&all));
                    if let Ok(mut nav) = nh.lock() {
                        nav.push(0);
                        w.set_can_go_back(nav.can_back());
                        w.set_can_go_forward(nav.can_forward());
                    }
                    w.set_active_tab(0);
                }
            });
        });
    });

    let pl_play_all = playlist_cache.clone();
    let cur_idx_all = current_index.clone();
    let shuf_all = shuffle_state.clone();
    let w_play_all = window_weak.clone();
    main_window.on_play_playlist_all(move |shuffle| {
        if let Some(w) = w_play_all.upgrade() {
            let tracks: Vec<TrackData> = w.get_active_playlist_tracks().iter().collect();
            if tracks.is_empty() {
                return;
            }
            shuf_all.store(shuffle, Ordering::SeqCst);
            w.set_shuffle_on(shuffle);
            let items: Vec<TrackItem> = tracks.iter().map(track_data_to_item).collect();
            if let Ok(mut pl) = pl_play_all.lock() {
                *pl = items.clone();
            }
            if let Ok(mut idx) = cur_idx_all.lock() {
                *idx = 0;
            }
            w.set_queue_tracks(to_slint_model(items));
            w.invoke_play_track(tracks[0].clone());
        }
    });

    let pend_pl = pending_playlist_track.clone();
    let w_req_pl = window_weak.clone();
    main_window.on_request_add_to_playlist(move |td| {
        if let Ok(mut p) = pend_pl.lock() {
            *p = Some(track_data_to_item(&td));
        }
        if let Some(w) = w_req_pl.upgrade() {
            w.set_pending_playlist_track_title(td.title);
            w.set_add_to_playlist_open(true);
        }
    });

    let pend_conf = pending_playlist_track.clone();
    let api_conf = api.clone();
    let w_conf = window_weak.clone();
    let rt_conf = rt_handle.clone();
    main_window.on_confirm_add_to_playlist(move |pid| {
        let item_opt = pend_conf.lock().ok().and_then(|mut g| g.take());
        if let Some(item) = item_opt {
            let core_t = riff_core::models::Track {
                id: item.id.clone(),
                path: item.id,
                file: item.title.clone(),
                title: item.title,
                artist: item.artist,
                album: if item.album.is_empty() { None } else { Some(item.album) },
                duration: item.duration_sec,
                thumbnail: item.thumbnail,
                added_at: 0,
                updated_at: 0,
            };
            let api_c = api_conf.clone();
            let w_c = w_conf.clone();
            let id = pid.to_string();
            rt_conf.spawn(async move {
                let _ = api_c.add_track_to_playlist(&id, &core_t).await;
                let all = api_c.get_playlists().await.unwrap_or_default();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_c.upgrade() {
                        w.set_playlists(playlists_to_slint(&all));
                    }
                });
            });
        }
    });

    let pl_clrq = playlist_cache.clone();
    let w_clrq = window_weak.clone();
    main_window.on_clear_queue(move || {
        if let Ok(mut pl) = pl_clrq.lock() {
            pl.clear();
        }
        if let Some(w) = w_clrq.upgrade() {
            w.set_queue_tracks(ModelRc::new(VecModel::from(Vec::new())));
        }
    });

    // 11f. Compact Mini Player Toggle
    let w_mini = window_weak.clone();
    main_window.on_toggle_mini_mode(move || {
        if let Some(w) = w_mini.upgrade() {
            let next = !w.get_mini_mode();
            w.set_mini_mode(next);
            if next {
                w.window().set_size(slint::LogicalSize::new(420.0, 520.0));
            } else {
                w.window().set_size(slint::LogicalSize::new(1100.0, 720.0));
            }
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

    // 13. Switch UI Mode & Save Setting Callbacks
    let api_mode = api.clone();
    let rt_mode = rt_handle.clone();
    main_window.on_switch_ui_mode(move |target_mode| {
        let api_m = api_mode.clone();
        let mode_str = target_mode.to_string();
        rt_mode.spawn(async move {
            let _ = api_m.set_ui_mode(&mode_str).await;
        });
    });

    let api_set = api.clone();
    let rt_set = rt_handle.clone();
    main_window.on_save_setting(move |key, val| {
        let api_s = api_set.clone();
        let k = key.to_string();
        let v = val.to_string();
        rt_set.spawn(async move {
            let _ = api_s.set_setting(&k, &v).await;
        });
    });

    let api_lsrc = api.clone();
    let lyr_lsrc = active_lyrics_cache.clone();
    let w_lsrc = window_weak.clone();
    let rt_lsrc = rt_handle.clone();
    main_window.on_change_lyrics_source(move |src| {
        let src_str = src.to_string();
        let api_s = api_lsrc.clone();
        let src_save = src_str.clone();
        rt_lsrc.spawn(async move {
            let _ = api_s.set_setting("slint_lyrics_source", &src_save).await;
        });
        if let Some(w) = w_lsrc.upgrade() {
            w.set_lyrics_source(src.clone());
            w.set_lyrics_source_label(SharedString::from(format_source_label(&src_str)));
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            let word_src = w.get_lyrics_word_source().to_string();
            if !w.get_current_id().is_empty() {
                spawn_fetch_lyrics(
                    api_lsrc.clone(),
                    lyr_lsrc.clone(),
                    w_lsrc.clone(),
                    &rt_lsrc,
                    title,
                    artist,
                    dur,
                    src_str,
                    word_src,
                );
            }
        }
    });

    let api_wsrc = api.clone();
    let lyr_wsrc = active_lyrics_cache.clone();
    let w_wsrc = window_weak.clone();
    let rt_wsrc = rt_handle.clone();
    main_window.on_change_word_source(move |wsrc| {
        let wsrc_str = wsrc.to_string();
        let api_s = api_wsrc.clone();
        let wsrc_save = wsrc_str.clone();
        rt_wsrc.spawn(async move {
            let _ = api_s.set_setting("slint_word_source", &wsrc_save).await;
        });
        if let Some(w) = w_wsrc.upgrade() {
            w.set_lyrics_word_source(wsrc.clone());
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            let src_pref = w.get_lyrics_source().to_string();
            if !w.get_current_id().is_empty() {
                spawn_fetch_lyrics(
                    api_wsrc.clone(),
                    lyr_wsrc.clone(),
                    w_wsrc.clone(),
                    &rt_wsrc,
                    title,
                    artist,
                    dur,
                    src_pref,
                    wsrc_str,
                );
            }
        }
    });

    let off_set = lyrics_offset_ms.clone();
    let api_off = api.clone();
    let lyr_off = active_lyrics_cache.clone();
    let w_off = window_weak.clone();
    let rt_off = rt_handle.clone();
    main_window.on_set_lyrics_offset(move |ms| {
        let ms_i = (ms.round() as i32).clamp(-5000, 5000);
        off_set.store(ms_i, Ordering::SeqCst);
        let api_s = api_off.clone();
        let val_str = ms_i.to_string();
        rt_off.spawn(async move {
            let _ = api_s.set_setting("slint_lyrics_offset_ms", &val_str).await;
        });
        let (_, pos_sec, _, _) = api_off.get_playback_state();
        let eff_pos = (pos_sec as f32) + (ms_i as f32 / 1000.0);
        if let Ok(mut lyr) = lyr_off.lock() {
            if !lyr.is_empty() {
                let (active_idx, _) = update_lyrics_progress(&mut lyr, eff_pos);
                if let Some(w) = w_off.upgrade() {
                    w.set_active_lyric_index(active_idx);
                    w.set_lyrics_lines(to_slint_lyrics(&lyr));
                }
            }
        }
    });

    let api_clrcust = api.clone();
    let lyr_clrcust = active_lyrics_cache.clone();
    let w_clrcust = window_weak.clone();
    let rt_clrcust = rt_handle.clone();
    main_window.on_clear_custom_lyrics(move || {
        if let Some(w) = w_clrcust.upgrade() {
            let title = w.get_current_title().to_string();
            let artist = w.get_current_artist().to_string();
            let dur = w.get_current_duration() as f64;
            let src_pref = w.get_lyrics_source().to_string();
            let word_src = w.get_lyrics_word_source().to_string();
            if w.get_current_id().is_empty() {
                return;
            }
            let api_c = api_clrcust.clone();
            let lyr_c = lyr_clrcust.clone();
            let w_c = w_clrcust.clone();
            let rt_c = rt_clrcust.clone();
            rt_clrcust.spawn(async move {
                let _ = api_c.save_custom_lyrics(&title, &artist, "").await;
                spawn_fetch_lyrics(api_c, lyr_c, w_c, &rt_c, title, artist, dur, src_pref, word_src);
            });
        }
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

    // 13c. Navigation History, Collapsible Sidebar, Search History, and View Shuffle/Filter/Sort
    let nh_nav = nav_history.clone();
    let w_nav = window_weak.clone();
    main_window.on_navigate_to(move |tab| {
        if let Some(w) = w_nav.upgrade() {
            if let Ok(mut nav) = nh_nav.lock() {
                nav.push(tab);
                w.set_can_go_back(nav.can_back());
                w.set_can_go_forward(nav.can_forward());
            }
            w.set_active_tab(tab);
        }
    });

    let nh_back = nav_history.clone();
    let w_back = window_weak.clone();
    main_window.on_nav_back(move || {
        if let Some(w) = w_back.upgrade() {
            if let Ok(mut nav) = nh_back.lock() {
                if let Some(tab) = nav.back() {
                    w.set_active_tab(tab);
                }
                w.set_can_go_back(nav.can_back());
                w.set_can_go_forward(nav.can_forward());
            }
        }
    });

    let nh_fwd = nav_history.clone();
    let w_fwd = window_weak.clone();
    main_window.on_nav_forward(move || {
        if let Some(w) = w_fwd.upgrade() {
            if let Ok(mut nav) = nh_fwd.lock() {
                if let Some(tab) = nav.forward() {
                    w.set_active_tab(tab);
                }
                w.set_can_go_back(nav.can_back());
                w.set_can_go_forward(nav.can_forward());
            }
        }
    });

    let api_sb = api.clone();
    let w_sb = window_weak.clone();
    let rt_sb = rt_handle.clone();
    main_window.on_toggle_sidebar(move || {
        if let Some(w) = w_sb.upgrade() {
            let next = !w.get_sidebar_collapsed();
            w.set_sidebar_collapsed(next);
            let api_s = api_sb.clone();
            let val = if next { "1" } else { "0" };
            rt_sb.spawn(async move {
                let _ = api_s.set_setting("slint_sidebar_collapsed", val).await;
            });
        }
    });

    let pls_flt = all_playlists_cache.clone();
    let q_sb_flt = sb_filter_query.clone();
    let m_sb_flt = sb_sort_mode.clone();
    let w_sb_flt = window_weak.clone();
    main_window.on_filter_sidebar(move |query| {
        let q = query.to_string();
        if let Ok(mut g) = q_sb_flt.lock() {
            *g = q.clone();
        }
        let mode = m_sb_flt.lock().map(|g| g.clone()).unwrap_or_else(|_| "Recents".to_string());
        if let (Ok(all), Some(w)) = (pls_flt.lock(), w_sb_flt.upgrade()) {
            let filtered = apply_sidebar_playlists(&all, &q, &mode);
            w.set_playlists(playlists_to_slint(&filtered));
        }
    });

    let pls_srt = all_playlists_cache.clone();
    let q_sb_srt = sb_filter_query.clone();
    let m_sb_srt = sb_sort_mode.clone();
    let w_sb_srt = window_weak.clone();
    main_window.on_cycle_sidebar_sort(move || {
        let next_mode = if let Ok(mut g) = m_sb_srt.lock() {
            *g = match g.as_str() {
                "Recents" => "A-Z".to_string(),
                "A-Z" => "Tracks".to_string(),
                _ => "Recents".to_string(),
            };
            g.clone()
        } else {
            "Recents".to_string()
        };
        let q = q_sb_srt.lock().map(|g| g.clone()).unwrap_or_default();
        if let (Ok(all), Some(w)) = (pls_srt.lock(), w_sb_srt.upgrade()) {
            let sorted = apply_sidebar_playlists(&all, &q, &next_mode);
            w.set_sb_sort_label(SharedString::from(&next_mode));
            w.set_playlists(playlists_to_slint(&sorted));
        }
    });

    let w_sh_sel = window_weak.clone();
    main_window.on_select_search_history(move |entry| {
        if let Some(w) = w_sh_sel.upgrade() {
            w.set_search_history_open(false);
            if entry.kind == "track" && !entry.track_id.is_empty() {
                let td = TrackData {
                    id: entry.track_id,
                    title: entry.title,
                    artist: entry.subtitle,
                    album: SharedString::default(),
                    duration_str: SharedString::from(format_duration(entry.duration_sec as f64)),
                    duration_sec: entry.duration_sec,
                    platform: entry.platform,
                    thumbnail: entry.thumbnail,
                    is_favorite: false,
                    cover: entry.cover,
                    has_cover: entry.has_cover,
                };
                w.invoke_play_track(td);
            } else {
                let plat = if entry.platform.is_empty() {
                    w.get_search_platform()
                } else {
                    entry.platform.clone()
                };
                w.set_search_platform(plat.clone());
                w.set_search_query(entry.title.clone());
                w.invoke_navigate_to(1);
                w.invoke_trigger_search(entry.title, plat);
            }
        }
    });

    let api_sh_rm = api.clone();
    let http_sh_rm = http.clone();
    let sh_rm_cache = search_history_cache.clone();
    let w_sh_rm = window_weak.clone();
    let rt_sh_rm = rt_handle.clone();
    main_window.on_remove_search_history(move |key| {
        let k = key.to_string();
        let snapshot = if let Ok(mut g) = sh_rm_cache.lock() {
            g.retain(|e| e.key != k);
            g.clone()
        } else {
            Vec::new()
        };
        let api_s = api_sh_rm.clone();
        let http_s = http_sh_rm.clone();
        let w_s = w_sh_rm.clone();
        rt_sh_rm.spawn(async move {
            if let Ok(json_str) = serde_json::to_string(&snapshot) {
                let _ = api_s.set_setting("slint_search_history", &json_str).await;
            }
            let slots = hydrate_search_history(&snapshot, &http_s).await;
            let is_empty = slots.is_empty();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_s.upgrade() {
                    w.set_search_history(to_slint_search_history(slots));
                    if is_empty {
                        w.set_search_history_open(false);
                    }
                }
            });
        });
    });

    let api_sh_clr = api.clone();
    let sh_clr_cache = search_history_cache.clone();
    let w_sh_clr = window_weak.clone();
    let rt_sh_clr = rt_handle.clone();
    main_window.on_clear_search_history(move || {
        if let Ok(mut g) = sh_clr_cache.lock() {
            g.clear();
        }
        if let Some(w) = w_sh_clr.upgrade() {
            w.set_search_history(ModelRc::new(VecModel::from(Vec::new())));
            w.set_search_history_open(false);
        }
        let api_s = api_sh_clr.clone();
        rt_sh_clr.spawn(async move {
            let _ = api_s.set_setting("slint_search_history", "[]").await;
        });
    });

    let shuf_loc_st = shuffle_state.clone();
    let pl_loc = playlist_cache.clone();
    let idx_loc = current_index.clone();
    let w_loc_shuf = window_weak.clone();
    main_window.on_shuffle_local(move || {
        if let Some(w) = w_loc_shuf.upgrade() {
            let tracks: Vec<TrackData> = w.get_library_tracks().iter().collect();
            if tracks.is_empty() {
                return;
            }
            shuf_loc_st.store(true, Ordering::SeqCst);
            w.set_shuffle_on(true);
            let items: Vec<TrackItem> = tracks.iter().map(track_data_to_item).collect();
            let pick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0))
                % tracks.len();
            if let Ok(mut pl) = pl_loc.lock() {
                *pl = items.clone();
            }
            if let Ok(mut idx) = idx_loc.lock() {
                *idx = pick;
            }
            w.set_queue_tracks(to_slint_model(items));
            w.invoke_play_track(tracks[pick].clone());
        }
    });

    let lc_flt = all_library_cache.clone();
    let q_loc_flt = local_filter_query.clone();
    let m_loc_flt = local_sort_mode.clone();
    let w_loc_flt = window_weak.clone();
    main_window.on_filter_local(move |query| {
        let q = query.to_string();
        if let Ok(mut g) = q_loc_flt.lock() {
            *g = q.clone();
        }
        let mode = m_loc_flt.lock().map(|g| g.clone()).unwrap_or_else(|_| "Default".to_string());
        if let (Ok(all), Some(w)) = (lc_flt.lock(), w_loc_flt.upgrade()) {
            let filtered = apply_track_filter_sort(&all, &q, &mode);
            w.set_library_tracks(to_slint_model(filtered));
        }
    });

    let lc_srt = all_library_cache.clone();
    let q_loc_srt = local_filter_query.clone();
    let m_loc_srt = local_sort_mode.clone();
    let w_loc_srt = window_weak.clone();
    let rt_loc_srt = rt_handle.clone();
    main_window.on_sort_local(move || {
        let next_mode = if let Ok(mut g) = m_loc_srt.lock() {
            *g = match g.as_str() {
                "Default" => "Title A-Z".to_string(),
                "Title A-Z" => "Artist A-Z".to_string(),
                "Artist A-Z" => "Duration".to_string(),
                _ => "Default".to_string(),
            };
            g.clone()
        } else {
            "Default".to_string()
        };
        let q = q_loc_srt.lock().map(|g| g.clone()).unwrap_or_default();
        if let (Ok(all), Some(w)) = (lc_srt.lock(), w_loc_srt.upgrade()) {
            let sorted = apply_track_filter_sort(&all, &q, &next_mode);
            w.set_library_tracks(to_slint_model(sorted));
        }
        show_toast(&w_loc_srt, &rt_loc_srt, format!("Sort: {next_mode}"));
    });

    let shuf_fav_st = shuffle_state.clone();
    let pl_fav = playlist_cache.clone();
    let idx_fav = current_index.clone();
    let w_fav_shuf = window_weak.clone();
    main_window.on_shuffle_favorites(move || {
        if let Some(w) = w_fav_shuf.upgrade() {
            let tracks: Vec<TrackData> = w.get_favorite_tracks().iter().collect();
            if tracks.is_empty() {
                return;
            }
            shuf_fav_st.store(true, Ordering::SeqCst);
            w.set_shuffle_on(true);
            let items: Vec<TrackItem> = tracks.iter().map(track_data_to_item).collect();
            let pick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0))
                % tracks.len();
            if let Ok(mut pl) = pl_fav.lock() {
                *pl = items.clone();
            }
            if let Ok(mut idx) = idx_fav.lock() {
                *idx = pick;
            }
            w.set_queue_tracks(to_slint_model(items));
            w.invoke_play_track(tracks[pick].clone());
        }
    });

    let fc_flt = all_favorites_cache.clone();
    let q_fav_flt = fav_filter_query.clone();
    let m_fav_flt = fav_sort_mode.clone();
    let w_fav_flt = window_weak.clone();
    main_window.on_filter_favorites(move |query| {
        let q = query.to_string();
        if let Ok(mut g) = q_fav_flt.lock() {
            *g = q.clone();
        }
        let mode = m_fav_flt.lock().map(|g| g.clone()).unwrap_or_else(|_| "Default".to_string());
        if let (Ok(all), Some(w)) = (fc_flt.lock(), w_fav_flt.upgrade()) {
            let filtered = apply_track_filter_sort(&all, &q, &mode);
            w.set_favorite_tracks(to_slint_model(filtered));
        }
    });

    let fc_srt = all_favorites_cache.clone();
    let q_fav_srt = fav_filter_query.clone();
    let m_fav_srt = fav_sort_mode.clone();
    let w_fav_srt = window_weak.clone();
    let rt_fav_srt = rt_handle.clone();
    main_window.on_sort_favorites(move || {
        let next_mode = if let Ok(mut g) = m_fav_srt.lock() {
            *g = match g.as_str() {
                "Default" => "Title A-Z".to_string(),
                "Title A-Z" => "Artist A-Z".to_string(),
                "Artist A-Z" => "Duration".to_string(),
                _ => "Default".to_string(),
            };
            g.clone()
        } else {
            "Default".to_string()
        };
        let q = q_fav_srt.lock().map(|g| g.clone()).unwrap_or_default();
        if let (Ok(all), Some(w)) = (fc_srt.lock(), w_fav_srt.upgrade()) {
            let sorted = apply_track_filter_sort(&all, &q, &next_mode);
            w.set_favorite_tracks(to_slint_model(sorted));
        }
        show_toast(&w_fav_srt, &rt_fav_srt, format!("Sort: {next_mode}"));
    });

    // 14. Sleep Timer Background Ticker (1s)
    let sd_tick = sleep_deadline.clone();
    let api_sleep_tick = api.clone();
    let w_sleep_tick = window_weak.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let mut expired = false;
        let mut rem_secs = None;
        if let Ok(mut d) = sd_tick.lock() {
            if let Some(deadline) = *d {
                let now = Instant::now();
                if now >= deadline {
                    *d = None;
                    expired = true;
                } else {
                    rem_secs = Some(deadline.saturating_duration_since(now).as_secs());
                }
            }
        }
        if expired {
            api_sleep_tick.pause();
            let w_c = w_sleep_tick.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_c.upgrade() {
                    w.set_sleep_active(false);
                    w.set_sleep_status(SharedString::from("00:00"));
                }
            });
        } else if let Some(rem) = rem_secs {
            let w_c = w_sleep_tick.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w_c.upgrade() {
                    w.set_sleep_status(SharedString::from(format!("{:02}:{:02}", rem / 60, rem % 60)));
                }
            });
        }
    });

    // 15. Subscribe to EventBus
    let mut rx = events.subscribe();
    let w_events = window_weak.clone();
    let lyrics_cache_ev = active_lyrics_cache.clone();
    let rep_ev = repeat_state.clone();
    let ste_ev = sleep_track_end.clone();
    let eof_ev = eof_handled.clone();
    let api_ev = api.clone();
    let off_ev = lyrics_offset_ms.clone();
    let rt_ev = rt_handle.clone();

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
                    let rep_mode = rep_ev.load(Ordering::SeqCst);
                    let off_sec = off_ev.load(Ordering::SeqCst) as f32 / 1000.0;
                    let is_eof = !is_playing && duration_sec > 1.0 && position_sec >= duration_sec - 0.35;
                    let should_handle_eof = is_eof && !eof_ev.swap(true, Ordering::SeqCst);
                    let sleep_end = should_handle_eof && ste_ev.swap(false, Ordering::SeqCst);
                    let api_e = api_ev.clone();

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
                                    let (active_idx, changed) = update_lyrics_progress(&mut lyr, (position_sec as f32) + off_sec);
                                    if changed {
                                        w.set_active_lyric_index(active_idx);
                                        w.set_lyrics_lines(to_slint_lyrics(&lyr));
                                    }
                                }
                            }

                            if sleep_end {
                                w.set_sleep_active(false);
                                w.set_sleep_status(SharedString::from("00:00"));
                            } else if should_handle_eof {
                                if rep_mode == 2 {
                                    api_e.seek(0.0);
                                    api_e.resume();
                                } else if rep_mode == 1 {
                                    w.invoke_play_next();
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
                riff_core::events::RiffEvent::PlaybackError { message } => {
                    let w_err = w_opt.clone();
                    let rt_err = rt_ev.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w_err.upgrade() {
                            w.set_is_playing(false);
                        }
                    });
                    show_toast(&w_opt, &rt_err, format!("Playback error: {message}"));
                }
                _ => {}
            }
        }
    });

    // 15b. Smooth 30 FPS Word-by-Word Karaoke Ticker
    let api_karaoke = api.clone();
    let lyr_karaoke = active_lyrics_cache.clone();
    let w_karaoke = window_weak.clone();
    let off_karaoke = lyrics_offset_ms.clone();
    std::thread::spawn(move || loop {
        let (playing, pos_sec, _, _) = api_karaoke.get_playback_state();
        if playing {
            let off_sec = off_karaoke.load(Ordering::SeqCst) as f32 / 1000.0;
            let updated = if let Ok(mut lyr) = lyr_karaoke.lock() {
                if !lyr.is_empty() {
                    let (active_idx, changed) = update_lyrics_progress(&mut lyr, (pos_sec as f32) + off_sec);
                    if changed {
                        Some((active_idx, lyr.clone()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((active_idx, snapshot)) = updated {
                let w_c = w_karaoke.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = w_c.upgrade() {
                        w.set_active_lyric_index(active_idx);
                        w.set_lyrics_lines(to_slint_lyrics(&snapshot));
                    }
                });
            }
            std::thread::sleep(Duration::from_millis(33));
        } else {
            std::thread::sleep(Duration::from_millis(150));
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

    #[test]
    fn test_detect_audio_hint() {
        assert_eq!(detect_audio_hint(Some("audio/mp4; codecs=\"mp4a.40.2\""), "https://example.com/x"), Some("m4a".to_string()));
        assert_eq!(detect_audio_hint(Some("audio/mpeg"), "https://example.com/x"), Some("mp3".to_string()));
        assert_eq!(detect_audio_hint(None, "https://rr1.googlevideo.com/videoplayback?mime=audio%2Fmp4"), Some("m4a".to_string()));
        assert_eq!(detect_audio_hint(None, "https://cf-media.sndcdn.com/track.mp3"), Some("mp3".to_string()));
    }

    #[test]
    fn test_word_by_word_karaoke_and_yrc() {
        let (clean, words) = parse_enhanced_lrc_words("<00:01.00>Hello <00:01.50>world", 1.0, 2.5);
        assert_eq!(clean, "Hello world");
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "Hello");
        assert!((words[0].start_sec - 1.0).abs() < 0.01);
        assert!((words[1].start_sec - 1.5).abs() < 0.01);

        let mut lines = parse_lrc("[00:01.00]<00:01.00>Hello <00:01.50>world\n[00:03.00]Next line", 10.0);
        assert!(lines[0].is_synced);
        let (active_idx, changed) = update_lyrics_progress(&mut lines, 1.25);
        assert_eq!(active_idx, 0);
        assert!(changed);
        let first_word = &lines[0].words[0];
        assert!(first_word.is_active);
        assert!((first_word.progress - 0.66).abs() < 0.05);

        let yrc = "[1000,2000](1000,500,0)Hello (1500,500,0)world";
        let parsed_yrc = riff_core::lyrics::parse_yrc(yrc);
        assert_eq!(parsed_yrc.len(), 1);
        assert_eq!(parsed_yrc[0].words.len(), 2);
        assert_eq!(parsed_yrc[0].words[0].text, "Hello");
    }

    #[test]
    fn test_lrc_multi_timestamp_offset_and_strict_match() {
        let lrc = "[offset:500]\n[00:08.50][00:20:50]Chorus line\n[00:10.00]Verse";
        let mut lines = parse_lrc(lrc, 60.0);
        // Leading gap inserted because first lyric starts at 8.0s >= 6s
        assert!(lines[0].is_gap);
        assert_eq!(lines[1].text, "Chorus line");
        assert!((lines[1].time_sec - 8.0).abs() < 0.05);
        assert_eq!(lines[2].text, "Verse");
        assert!((lines[2].time_sec - 9.5).abs() < 0.05);
        // Middle instrumental gap inserted between 13.5s and 20.0s (>= 6s)
        assert!(lines[3].is_gap);
        assert_eq!(lines[4].text, "Chorus line");
        assert!((lines[4].time_sec - 20.0).abs() < 0.05);

        // Before first lyric (at 0.5s), active line should be 0 (leading gap), and if no gap, -1
        let (idx, _) = update_lyrics_progress(&mut lines, 0.5);
        assert_eq!(idx, 0);

        assert!(riff_core::lyrics::is_strict_lyric_match(
            "Rock That Body (Official Video)",
            "The Black Eyed Peas - Topic",
            "Rock That Body",
            "Black Eyed Peas"
        ));
        assert!(!riff_core::lyrics::is_strict_lyric_match(
            "Rock That Body",
            "The Black Eyed Peas",
            "Rock That Body",
            "Random DJ"
        ));
    }

    #[test]
    fn test_nav_history_and_sidebar_filter_sort() {
        let mut nav = NavHistory::new();
        assert!(!nav.can_back());
        assert!(!nav.can_forward());
        nav.push(1);
        nav.push(3);
        assert!(nav.can_back());
        assert!(!nav.can_forward());
        assert_eq!(nav.back(), Some(1));
        assert!(nav.can_forward());
        assert_eq!(nav.forward(), Some(3));
        assert_eq!(nav.back(), Some(1));
        nav.push(2);
        assert!(!nav.can_forward());

        let pls = vec![
            riff_core::models::Playlist {
                id: "1".to_string(),
                name: "Chill Mix".to_string(),
                tracks: vec![],
            },
            riff_core::models::Playlist {
                id: "2".to_string(),
                name: "Ambient Focus".to_string(),
                tracks: vec![riff_core::models::Track {
                    id: "a".to_string(),
                    path: "a".to_string(),
                    file: "a".to_string(),
                    title: "Track".to_string(),
                    artist: "Artist".to_string(),
                    album: None,
                    duration: 120.0,
                    thumbnail: String::new(),
                    added_at: 0,
                    updated_at: 0,
                }],
            },
        ];
        let az = apply_sidebar_playlists(&pls, "", "A-Z");
        assert_eq!(az[0].name, "Ambient Focus");
        let flt = apply_sidebar_playlists(&pls, "chill", "Recents");
        assert_eq!(flt.len(), 1);
        assert_eq!(flt[0].id, "1");
    }

    #[test]
    #[ignore]
    fn test_live_stream_resolve_and_decode() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = Arc::new(riff_core::config::Config::default());
            let db = riff_core::db::open(&config.db_path).unwrap();
            let http = reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) Riff/0.7.6")
                .build()
                .unwrap();
            let soundcloud = riff_core::soundcloud::SoundCloudClient::new(http.clone());
            let youtube = Arc::new(riff_core::youtube::YouTubeClient::new(http.clone()));
            let lyrics = Arc::new(riff_core::lyrics::LyricsService::new(http.clone(), config.data_dir.clone()));
            let events = Arc::new(riff_core::events::EventBus::default());
            let player = Arc::new(riff_core::player::AudioPlayer::new(events.clone()));
            let api = riff_core::api::RiffApi::new(db, config, soundcloud, youtube, lyrics, events, player);

            let (tracks, _) = api.search("lofi hip hop", "ytmusic", 3, 0).await.unwrap();
            assert!(!tracks.is_empty(), "search returned no tracks");
            let first = &tracks[0];
            let q = riff_core::models::StreamQuery {
                id: first.get("id").and_then(|v| v.as_str()).map(String::from),
                title: first.get("title").and_then(|v| v.as_str()).map(String::from),
                artist: first.get("artist").and_then(|v| v.as_str()).map(String::from),
                platform: Some("ytmusic".to_string()),
                duration: first.get("duration").and_then(|v| v.as_f64()),
                url: None,
                thumbnail: None,
                timeout: Some(20),
                quality: Some("medium".to_string()),
                q: None,
            };
            let resolved = api.resolve_stream(&q, 0).await.expect("resolve_stream failed");
            let is_hls = resolved.get("isHls").and_then(|v| v.as_bool()).unwrap_or(false);
            let raw_url = resolved.get("rawStreamUrl").and_then(|v| v.as_str()).expect("missing rawStreamUrl");
            let (bytes, hint) = download_stream_bytes(&http, raw_url, is_hls).await.expect("download_stream_bytes failed");
            assert!(bytes.len() > 10_000, "downloaded audio too small: {}", bytes.len());
            let dec = riff_core::player::decoder::AudioDecoder::new(std::io::Cursor::new(bytes), hint.as_deref())
                .expect("AudioDecoder failed to decode stream");
            assert!(dec.sample_rate() > 0);
        });
    }
}
