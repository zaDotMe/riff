use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::Track;

pub type Db = Arc<Mutex<Connection>>;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
    id          TEXT PRIMARY KEY,
    path        TEXT NOT NULL UNIQUE,
    file        TEXT NOT NULL,
    title       TEXT NOT NULL,
    artist      TEXT NOT NULL,
    album       TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    thumbnail   TEXT NOT NULL DEFAULT '',
    added_at    INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_title  ON tracks(title);
CREATE INDEX IF NOT EXISTS idx_tracks_added  ON tracks(added_at DESC);
"#;

pub fn open(path: &Path) -> Result<Db> {
    let conn = Connection::open(path)
        .with_context(|| format!("open sqlite database {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -2000)?;
    conn.pragma_update(None, "mmap_size", 16777216)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.execute_batch(SCHEMA)?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub fn upsert_track(conn: &Connection, track: &Track) -> Result<()> {
    conn.execute(
        r#"
        INSERT OR REPLACE INTO tracks
            (id, path, file, title, artist, album, duration_ms, thumbnail, added_at, updated_at)
        VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        "#,
        params![
            track.id,
            track.path,
            track.file,
            track.title,
            track.artist,
            track.album,
            (track.duration * 1000.0).round() as i64,
            track.thumbnail,
            track.added_at as i64,
            track.updated_at as i64,
        ],
    )?;
    Ok(())
}

pub fn list_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, path, file, title, artist, album, duration_ms, thumbnail, added_at, updated_at
        FROM tracks
        ORDER BY added_at DESC, title COLLATE NOCASE ASC
        "#,
    )?;
    let rows = stmt.query_map([], row_to_track)?;
    let mut tracks = Vec::new();
    for row in rows {
        tracks.push(row?);
    }
    Ok(tracks)
}

pub fn get_track(conn: &Connection, id: &str) -> Result<Option<Track>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, path, file, title, artist, album, duration_ms, thumbnail, added_at, updated_at
        FROM tracks
        WHERE id = ?1
        LIMIT 1
        "#,
    )?;
    let track = stmt.query_row([id], row_to_track).optional()?;
    Ok(track)
}

pub fn remove_track(conn: &Connection, id: &str) -> Result<bool> {
    let changed = conn.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    Ok(changed > 0)
}

pub fn update_duration(conn: &Connection, id: &str, duration: f64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE tracks SET duration_ms = ?1, updated_at = ?2 WHERE id = ?3",
        params![
            (duration * 1000.0).round() as i64,
            now_millis() as i64,
            id
        ],
    )?;
    Ok(changed > 0)
}

pub fn count(conn: &Connection) -> Result<i64> {
    let count = conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
    Ok(count)
}

pub fn search_tracks(conn: &Connection, query: &str, limit: usize, offset: usize) -> Result<Vec<Track>> {
    let pattern = format!("%{query}%");
    let mut stmt = conn.prepare(
        r#"
        SELECT id, path, file, title, artist, album, duration_ms, thumbnail, added_at, updated_at
        FROM tracks
        WHERE title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1 OR file LIKE ?1
        ORDER BY added_at DESC
        LIMIT ?2 OFFSET ?3
        "#,
    )?;
    let rows = stmt.query_map(params![pattern, limit as i64, offset as i64], row_to_track)?;
    let mut tracks = Vec::new();
    for row in rows {
        tracks.push(row?);
    }
    Ok(tracks)
}

fn row_to_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<Track> {
    let duration_ms: i64 = row.get("duration_ms")?;
    let added_at: i64 = row.get("added_at")?;
    let updated_at: i64 = row.get("updated_at")?;

    Ok(Track {
        id: row.get("id")?,
        path: row.get("path")?,
        file: row.get("file")?,
        title: row.get("title")?,
        artist: row.get("artist")?,
        album: row.get("album")?,
        duration: duration_ms as f64 / 1000.0,
        thumbnail: row.get("thumbnail")?,
        added_at: added_at as u64,
        updated_at: updated_at as u64,
    })
}

pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
