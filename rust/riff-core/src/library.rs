use std::path::Path;

use anyhow::Result;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use rusqlite::OptionalExtension;
use serde::Serialize;
use walkdir::WalkDir;

use crate::db::{self, Db};
use crate::models::Track;

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "opus", "ogg", "oga", "m4a", "aac", "webm", "weba",
];

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub scanned: usize,
    pub skipped: usize,
    pub total: i64,
}

pub fn scan_library(db: &Db, dir: &Path) -> Result<ScanResult> {
    let conn = db.lock().expect("sqlite mutex poisoned");
    let now = db::now_millis();
    let mut result = ScanResult::default();

    for entry in WalkDir::new(dir).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result.skipped += 1;
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        if file_name.is_empty() {
            result.skipped += 1;
            continue;
        }

        let path_string = path.to_string_lossy().to_string();
        let existing: Option<(String, i64)> = conn
            .query_row(
                "SELECT id, added_at FROM tracks WHERE path = ?1 LIMIT 1",
                [&path_string],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let (id, added_at) = match existing {
            Some((id, added_at)) => (id, added_at as u64),
            None => (local_id(path), now),
        };

        let (title, artist, album, duration) = read_metadata(path, &file_name);
        let track = Track {
            id,
            path: path_string,
            file: file_name,
            title,
            artist,
            album,
            duration,
            thumbnail: String::new(),
            added_at,
            updated_at: now,
        };

        match db::upsert_track(&conn, &track) {
            Ok(()) => result.scanned += 1,
            Err(_) => result.skipped += 1,
        }
    }

    result.total = db::count(&conn)?;
    Ok(result)
}

pub fn local_id(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("local_{:012x}", hash & 0x0000_ffff_ffff_ffff)
}

fn read_metadata(path: &Path, file_name: &str) -> (String, String, Option<String>, f64) {
    let fallback = parse_display_name(file_name);

    let tagged = match lofty::read_from_path(path) {
        Ok(tagged) => tagged,
        Err(_) => return (fallback.0, fallback.1, None, 0.0),
    };

    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let title = tag
        .and_then(|tag| tag.title())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback.0);
    let artist = tag
        .and_then(|tag| tag.artist())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback.1);
    let album = tag
        .and_then(|tag| tag.album())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let duration = tagged.properties().duration().as_secs_f64();

    (title, artist, album, duration)
}

pub fn parse_display_name(name: &str) -> (String, String) {
    let clean_name = match Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
    {
        Some(extension) if AUDIO_EXTENSIONS.contains(&extension.as_str()) => Path::new(name)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(name)
            .to_string(),
        _ => name.to_string(),
    };

    parse_clean_name(clean_name.trim())
}

fn parse_clean_name(stem: &str) -> (String, String) {
    // Strip leading track numbers such as "01 - ", "01. ", or "01 ".
    let bytes = stem.as_bytes();
    let mut index = 0;
    while index < bytes.len()
        && (bytes[index].is_ascii_digit()
            || bytes[index] == b'.'
            || bytes[index] == b'-'
            || bytes[index] == b' ')
    {
        index += 1;
    }
    let clean = if index > 0 && index < stem.len() {
        stem[index..].trim()
    } else {
        stem
    };

    if let Some((artist, title)) = clean.split_once(" - ") {
        let artist = artist.trim();
        let title = title.trim();
        if !artist.is_empty() && !title.is_empty() {
            return (title.to_string(), artist.to_string());
        }
    }

    (clean.to_string(), "Unknown artist".to_string())
}
