use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tokio::process::Command;
use tokio::time::timeout;
use tokio_util::io::ReaderStream;

pub async fn serve_file(path: PathBuf, range: Option<String>) -> Result<Response> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("stat {}", path.display()))?;
    let len = metadata.len();
    let content_type = mime_for_path(&path);

    if let Some(range) = range {
        let (start, end) = match parse_range(&range, len) {
            Some(value) => value,
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{len}"))
                    .body(Body::empty())
                    .map_err(Into::into);
            }
        };

        let mut file = tokio::fs::File::open(&path)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        file.seek(SeekFrom::Start(start)).await?;
        let stream = ReaderStream::new(file.take(end - start + 1));

        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, (end - start + 1).to_string())
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{len}"),
            )
            .body(Body::from_stream(stream))
            .map_err(Into::into);
    }

    let file = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let stream = ReaderStream::new(file);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len.to_string())
        .body(Body::from_stream(stream))
        .map_err(Into::into)
}

pub async fn resolve_with_ytdlp(
    binary: Option<&Path>,
    track_url: &str,
    timeout_secs: u64,
) -> Result<String> {
    let binary = binary
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" }));

    let mut command = Command::new(&binary);
    command
        .args([
            "-g",
            "-f",
            "bestaudio/best",
            "--no-playlist",
            "--no-warnings",
            track_url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: don't flash a console window for every resolve.
        command.creation_flags(0x0800_0000);
    }

    let output = timeout(Duration::from_secs(timeout_secs), command.output())
        .await
        .map_err(|_| anyhow!("yt-dlp timed out after {timeout_secs}s"))?
        .with_context(|| format!("run {}", binary.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        return Err(anyhow!(
            "yt-dlp exited with {}: {}",
            output.status,
            if message.is_empty() {
                "no stderr"
            } else {
                message
            }
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stream_url = stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("http://") || line.starts_with("https://"))
        .ok_or_else(|| anyhow!("yt-dlp returned no stream URL"))?;

    Ok(stream_url.to_string())
}

pub async fn proxy_audio(
    client: &reqwest::Client,
    raw_url: &str,
    range: Option<&str>,
) -> Result<Response> {
    let parsed = url::Url::parse(raw_url).context("invalid stream URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(anyhow!("unsupported stream URL scheme"));
    }

    let is_googlevideo = parsed.host_str().map(|h| h.ends_with("googlevideo.com")).unwrap_or(false);

    let effective_range = if is_googlevideo {
        // GoogleVideo strictly enforces a maximum chunk limit of 1MB (1048576 bytes) for iOS/mobile clients.
        // If range is missing or exceeds 1MB, cap it to 1MB.
        const CHUNK_SIZE: u64 = 1_048_576;
        match range {
            None => Some(format!("bytes=0-{}", CHUNK_SIZE - 1)),
            Some(r) => {
                let trimmed = r.trim();
                if let Some(spec) = trimmed.strip_prefix("bytes=") {
                    if let Some((start_str, end_str)) = spec.split_once('-') {
                        let start = start_str.parse::<u64>().unwrap_or(0);
                        if end_str.is_empty() {
                            Some(format!("bytes={start}-{}", start + CHUNK_SIZE - 1))
                        } else {
                            let end = end_str.parse::<u64>().unwrap_or(start + CHUNK_SIZE - 1);
                            if end >= start + CHUNK_SIZE {
                                Some(format!("bytes={start}-{}", start + CHUNK_SIZE - 1))
                            } else {
                                Some(trimmed.to_string())
                            }
                        }
                    } else {
                        Some(format!("bytes=0-{}", CHUNK_SIZE - 1))
                    }
                } else {
                    Some(format!("bytes=0-{}", CHUNK_SIZE - 1))
                }
            }
        }
    } else {
        range.map(|r| r.to_string())
    };

    let mut request = client.get(raw_url);
    if is_googlevideo {
        let mut ua = "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
        for (k, v) in parsed.query_pairs() {
            if k == "c" {
                if v == "ANDROID_MUSIC" {
                    ua = "com.google.android.apps.youtube.music/7.27.52 (Linux; U; Android 14; en_US) gzip";
                } else if v == "IOS" {
                    ua = "com.google.ios.youtube/20.10.4 (iPhone16,2; U; CPU iOS 18_3_2 like Mac OS X;)";
                }
            }
        }
        request = request.header(header::USER_AGENT, ua);
    }

    if let Some(ref r) = effective_range {
        request = request.header(header::RANGE, r);
    }

    let upstream = request.send().await.context("request upstream audio")?;
    let status = StatusCode::from_u16(upstream.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);

    let mut builder = Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "*");

    for name in [
        header::CONTENT_TYPE,
        header::CONTENT_LENGTH,
        header::CONTENT_RANGE,
        header::CACHE_CONTROL,
    ] {
        if let Some(value) = upstream.headers().get(&name) {
            builder = builder.header(&name, value);
        }
    }

    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .map_err(Into::into)
}

pub fn mime_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg".to_string(),
        "wav" => "audio/wav".to_string(),
        "flac" => "audio/flac".to_string(),
        "opus" => "audio/ogg; codecs=opus".to_string(),
        "ogg" | "oga" => "audio/ogg".to_string(),
        "m4a" => "audio/mp4".to_string(),
        "aac" => "audio/aac".to_string(),
        "webm" | "weba" => "audio/webm".to_string(),
        _ => mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string(),
    }
}

fn parse_range(value: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;

    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some((len.saturating_sub(suffix), len - 1));
    }

    let start: u64 = start.parse().ok()?;
    if start >= len {
        return None;
    }
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().ok()?.min(len - 1)
    };
    if end < start {
        return None;
    }

    Some((start, end))
}
