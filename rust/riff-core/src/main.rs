mod config;
mod db;
mod library;
mod models;
mod soundcloud;
mod stream;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::models::{
    FileQuery, LibraryAddRequest, LibraryItem, LibraryRemoveRequest, LibraryUpdateRequest,
    ProxyQuery, ScanRequest, SearchQuery, StreamQuery, Track,
};

#[derive(Clone)]
struct AppState {
    db: db::Db,
    config: Arc<config::Config>,
    token: Arc<String>,
    http: reqwest::Client,
    soundcloud: Arc<soundcloud::SoundCloudClient>,
    app_state: Arc<RwLock<Value>>,
    port: Arc<AtomicU16>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "riff_core=info,tower_http=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Arc::new(config::Config::from_args()?);
    let db = db::open(&config.db_path)?;

    let http = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .build()?;
    let soundcloud = soundcloud::SoundCloudClient::new(http.clone());

    let state = AppState {
        db,
        token: Arc::new(config.token.clone()),
        http,
        soundcloud,
        app_state: Arc::new(RwLock::new(Value::Null)),
        port: Arc::new(AtomicU16::new(0)),
        config: config.clone(),
    };

    let protected = Router::new()
        .route("/api/library", get(list_library))
        .route("/api/library-add", post(library_add))
        .route("/api/library-update", post(library_update))
        .route("/api/library-remove", post(library_remove))
        .route("/api/library-file", get(library_file))
        .route("/api/local-track", get(local_track))
        .route("/api/scan", post(scan_library))
        .route("/api/tracks", get(list_tracks))
        .route("/api/search", get(search))
        .route("/api/stream-info", get(stream_info))
        .route("/api/audio-proxy", get(audio_proxy))
        .route("/api/state", get(get_state).post(set_state))
        .route("/api/tools-status", get(tools_status))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));

    let app = Router::new()
        .route("/api/health", get(health))
        .merge(protected)
        .with_state(state.clone())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", config.port)).await?;
    let addr = listener.local_addr()?;
    state.port.store(addr.port(), Ordering::SeqCst);

    // The Electron main process (and the benchmark script) reads this line.
    println!(
        "{}",
        serde_json::to_string(&json!({
            "event": "ready",
            "name": "riff-core",
            "version": env!("CARGO_PKG_VERSION"),
            "port": addr.port(),
            "token": config.token,
            "pid": std::process::id(),
            "dataDir": config.data_dir,
            "libraryDir": config.library_dir,
        }))?
    );
    info!("riff-core listening on http://{}", addr);

    axum::serve(listener, app).await?;
    Ok(())
}

async fn auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let provided = request
        .headers()
        .get("x-riff-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    if !token_matches(provided.as_bytes(), state.token.as_bytes()) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(request).await)
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let count = tokio::task::spawn_blocking({
        let db = state.db.clone();
        move || {
            let conn = db.lock().expect("sqlite mutex poisoned");
            db::count(&conn).unwrap_or(0)
        }
    })
    .await
    .unwrap_or(0);

    Json(json!({
        "success": true,
        "name": "riff-core",
        "version": env!("CARGO_PKG_VERSION"),
        "tracks": count,
        "port": state.port.load(Ordering::SeqCst),
    }))
}

async fn list_library(
    State(state): State<AppState>,
) -> Result<Json<Vec<LibraryItem>>, ApiError> {
    let tracks = run_db(&state.db, |conn| db::list_tracks(conn)).await?;
    Ok(Json(tracks.into_iter().map(LibraryItem::from).collect()))
}

async fn list_tracks(State(state): State<AppState>) -> Result<Json<Vec<Track>>, ApiError> {
    let tracks = run_db(&state.db, |conn| db::list_tracks(conn)).await?;
    Ok(Json(tracks))
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let q = query.q.unwrap_or_default().trim().to_string();
    let limit = query.limit.unwrap_or(12).clamp(1, 50);
    let offset = query.offset.unwrap_or(0);
    let platform = query.platform.as_deref().unwrap_or("ytmusic");

    if q.is_empty() {
        return Ok(Json(json!({
            "success": true,
            "tracks": [],
            "hasMore": false,
        })));
    }

    // 1. If platform is explicitly SoundCloud, use native high-performance Rust scraper
    if platform == "soundcloud" {
        match state.soundcloud.search(&q, limit).await {
            Ok(tracks) => {
                let has_more = tracks.len() >= limit;
                return Ok(Json(json!({
                    "success": true,
                    "tracks": tracks,
                    "hasMore": has_more,
                })));
            }
            Err(err) => {
                warn!("SoundCloud native search error: {}", err);
            }
        }
    }

    // 2. Query local SQLite B-Tree database
    let tracks = run_db(&state.db, {
        let q = q.clone();
        move |conn| db::search_tracks(conn, &q, limit + 1, offset)
    })
    .await?;

    if !tracks.is_empty() || platform == "local" {
        let has_more = tracks.len() > limit;
        let sliced: Vec<Value> = tracks
            .into_iter()
            .take(limit)
            .map(|t| {
                json!({
                    "id": t.id,
                    "title": t.title,
                    "artist": t.artist,
                    "album": t.album.unwrap_or_default(),
                    "duration": t.duration,
                    "thumbnail": t.thumbnail,
                    "platform": "local",
                    "isLocal": true,
                    "isCached": true,
                })
            })
            .collect();

        return Ok(Json(json!({
            "success": true,
            "tracks": sliced,
            "hasMore": has_more,
        })));
    }

    // 3. Fallback: query SoundCloud if no local matches
    if let Ok(tracks) = state.soundcloud.search(&q, limit).await {
        if !tracks.is_empty() {
            let has_more = tracks.len() >= limit;
            return Ok(Json(json!({
                "success": true,
                "tracks": tracks,
                "hasMore": has_more,
            })));
        }
    }

    Ok(Json(json!({
        "success": true,
        "tracks": [],
        "hasMore": false,
    })))
}

async fn library_add(
    State(state): State<AppState>,
    Json(payload): Json<LibraryAddRequest>,
) -> Result<Json<Value>, ApiError> {
    let library_dir = state.config.library_dir.clone();
    let now = db::now_millis();

    let count = run_db(&state.db, move |conn| {
        let mut count = 0usize;

        for item in payload.items {
            let file_name = match Path::new(&item.file).file_name().and_then(|value| value.to_str())
            {
                Some(name) if name == item.file => name.to_string(),
                _ => continue,
            };
            let path = library_dir.join(&file_name);
            if !path.is_file() {
                continue;
            }

            let id = item
                .id
                .filter(|id| id.starts_with("local_"))
                .unwrap_or_else(|| library::local_id(&path));
            let (title, artist) = match item.name {
                Some(name) if !name.trim().is_empty() => library::parse_display_name(&name),
                _ => library::parse_display_name(&file_name),
            };

            let track = Track {
                id,
                path: path.to_string_lossy().to_string(),
                file: file_name,
                title,
                artist,
                album: None,
                duration: 0.0,
                thumbnail: String::new(),
                added_at: now,
                updated_at: now,
            };

            db::upsert_track(conn, &track)?;
            count += 1;
        }

        Ok(count)
    })
    .await?;

    Ok(Json(json!({ "success": true, "count": count })))
}

async fn library_update(
    State(state): State<AppState>,
    Json(payload): Json<LibraryUpdateRequest>,
) -> Result<Json<Value>, ApiError> {
    let duration = payload.duration.unwrap_or(0.0);
    let updated = run_db(&state.db, move |conn| {
        db::update_duration(conn, &payload.id, duration)
    })
    .await?;

    Ok(Json(json!({ "success": true, "updated": updated })))
}

async fn library_remove(
    State(state): State<AppState>,
    Json(payload): Json<LibraryRemoveRequest>,
) -> Result<Json<Value>, ApiError> {
    let removed = run_db(&state.db, move |conn| db::remove_track(conn, &payload.id)).await?;
    Ok(Json(json!({ "success": true, "removed": removed })))
}

async fn scan_library(
    State(state): State<AppState>,
    payload: Option<Json<ScanRequest>>,
) -> Result<Json<Value>, ApiError> {
    let dir = match payload.and_then(|Json(payload)| payload.dir) {
        Some(dir) => {
            let path = PathBuf::from(dir);
            if !path.is_dir() {
                return Err(ApiError::bad_request("scan directory not found"));
            }
            path
        }
        None => state.config.library_dir.clone(),
    };

    let db = state.db.clone();
    let result = tokio::task::spawn_blocking(move || library::scan_library(&db, &dir))
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?
        .map_err(|error| ApiError::internal(error.to_string()))?;

    Ok(Json(json!({
        "success": true,
        "scanned": result.scanned,
        "skipped": result.skipped,
        "total": result.total,
    })))
}

async fn library_file(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = query
        .id
        .ok_or_else(|| ApiError::bad_request("missing track id"))?;
    serve_track(&state, &id, &headers).await
}

async fn local_track(
    State(state): State<AppState>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = query
        .id
        .ok_or_else(|| ApiError::bad_request("missing track id"))?;
    serve_track(&state, &id, &headers).await
}

async fn serve_track(
    state: &AppState,
    id: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let track = run_db(&state.db, {
        let id = id.to_string();
        move |conn| db::get_track(conn, &id)
    })
    .await?
    .ok_or_else(|| ApiError::not_found("track not found"))?;

    let path = state.config.library_dir.join(&track.file);
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    stream::serve_file(path, range)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))
}

async fn stream_info(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
) -> Result<Json<Value>, ApiError> {
    let port = state.port.load(Ordering::SeqCst);
    let id = query.id.clone().unwrap_or_default().trim().to_string();
    let platform = query.platform.clone().unwrap_or_default().trim().to_string();
    let track_url = query.url.clone().unwrap_or_default().trim().to_string();

    if id.starts_with("local_") {
        let track = run_db(&state.db, {
            let id = id.clone();
            move |conn| db::get_track(conn, &id)
        })
        .await?
        .ok_or_else(|| ApiError::not_found("local track not found"))?;

        return Ok(Json(json!({
            "success": true,
            "isLocal": true,
            "streamUrl": format!(
                "http://127.0.0.1:{port}/api/library-file?id={}",
                url_encode(&track.id)
            ),
            "sourceMeta": {
                "id": track.id,
                "title": track.title,
                "artist": track.artist,
                "platform": "local",
            },
        })));
    }

    // 1. Resolve SoundCloud stream natively in Rust
    let is_soundcloud = platform == "soundcloud"
        || track_url.contains("soundcloud.com")
        || (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && id.len() >= 6);

    if is_soundcloud {
        let identifier = if !track_url.is_empty() {
            &track_url
        } else {
            &id
        };
        let query_fallback = query.title.as_deref();

        match state.soundcloud.resolve_stream(identifier, query_fallback).await {
            Ok(direct_url) => {
                let proxy_url = format!(
                    "http://127.0.0.1:{port}/api/audio-proxy?url={}",
                    url_encode(&direct_url)
                );

                return Ok(Json(json!({
                    "success": true,
                    "isLocal": false,
                    "streamUrl": proxy_url,
                    "rawStreamUrl": direct_url,
                    "proxyUrl": proxy_url,
                    "sourceMeta": {
                        "id": id,
                        "title": query.title.unwrap_or_default(),
                        "artist": query.artist.unwrap_or_default(),
                        "duration": query.duration.unwrap_or(0.0),
                        "thumbnail": query.thumbnail.unwrap_or_default(),
                        "platform": "soundcloud",
                        "url": track_url,
                    },
                })));
            }
            Err(err) => {
                warn!("SoundCloud native stream resolution failed: {}. Falling back to yt-dlp.", err);
            }
        }
    }

    // 2. Resolve YouTube / other platforms via yt-dlp
    let target_url = if !track_url.is_empty() {
        track_url
    } else if id.len() == 11
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        format!("https://www.youtube.com/watch?v={id}")
    } else if is_soundcloud {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "failed to resolve soundcloud stream",
        ));
    } else {
        return Err(ApiError::bad_request("missing url or id"));
    };

    let timeout_secs = query.timeout.unwrap_or(20).clamp(5, 30);
    let raw_stream = stream::resolve_with_ytdlp(
        state.config.ytdlp.as_deref(),
        &target_url,
        timeout_secs,
    )
    .await
    .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, error.to_string()))?;

    let proxy_url = format!(
        "http://127.0.0.1:{port}/api/audio-proxy?url={}",
        url_encode(&raw_stream)
    );

    Ok(Json(json!({
        "success": true,
        "isLocal": false,
        "streamUrl": proxy_url,
        "rawStreamUrl": raw_stream,
        "proxyUrl": proxy_url,
        "sourceMeta": {
            "id": id,
            "title": query.title.unwrap_or_default(),
            "artist": query.artist.unwrap_or_default(),
            "duration": query.duration.unwrap_or(0.0),
            "thumbnail": query.thumbnail.unwrap_or_default(),
            "platform": query.platform.unwrap_or_else(|| "ytmusic".to_string()),
            "url": target_url,
        },
    })))
}

async fn audio_proxy(
    State(state): State<AppState>,
    Query(query): Query<ProxyQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let raw_url = query
        .url
        .ok_or_else(|| ApiError::bad_request("missing url"))?;
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());

    stream::proxy_audio(&state.http, &raw_url, range)
        .await
        .map_err(|error| ApiError::new(StatusCode::BAD_GATEWAY, error.to_string()))
}

async fn get_state(State(state): State<AppState>) -> Json<Value> {
    Json(state.app_state.read().await.clone())
}

async fn set_state(
    State(state): State<AppState>,
    Json(value): Json<Value>,
) -> Json<Value> {
    *state.app_state.write().await = value;
    Json(json!({ "success": true }))
}

async fn tools_status(State(state): State<AppState>) -> Json<Value> {
    let ytdlp = binary_status(
        state.config.ytdlp.as_deref(),
        if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" },
    )
    .await;
    let ffmpeg = binary_status(
        state.config.ffmpeg.as_deref(),
        if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" },
    )
    .await;

    Json(json!({
        "success": true,
        "ytdlp": ytdlp,
        "ffmpeg": ffmpeg,
    }))
}

async fn binary_status(configured: Option<&Path>, fallback: &str) -> Value {
    let binary = configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(fallback));

    match tokio::process::Command::new(&binary)
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            json!({
                "found": true,
                "path": binary.to_string_lossy(),
                "version": version,
            })
        }
        _ => json!({
            "found": false,
            "path": binary.to_string_lossy(),
        }),
    }
}

async fn run_db<T, F>(db: &db::Db, operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.lock().expect("sqlite mutex poisoned");
        operation(&conn)
    })
    .await
    .map_err(|error| ApiError::internal(format!("database task failed: {error}")))?
    .map_err(|error| ApiError::internal(error.to_string()))
}

fn token_matches(provided: &[u8], expected: &[u8]) -> bool {
    if provided.len() != expected.len() {
        return false;
    }

    let mut difference = 0u8;
    for (left, right) in provided.iter().zip(expected.iter()) {
        difference |= left ^ right;
    }
    difference == 0
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "success": false,
                "error": self.message,
            })),
        )
            .into_response()
    }
}
