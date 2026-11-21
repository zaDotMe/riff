use std::path::PathBuf;

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

#[derive(Debug, Deserialize)]
pub struct ImageProxyQuery {
    pub url: Option<String>,
    #[allow(dead_code)]
    pub artist: Option<String>,
    #[allow(dead_code)]
    pub title: Option<String>,
}

#[derive(Clone)]
pub struct ImageProxyService {
    client: Client,
    cache_dir: PathBuf,
}

impl ImageProxyService {
    pub fn new(client: Client, cache_dir: PathBuf) -> Self {
        let thumbs_dir = cache_dir.join("thumbnails");
        let _ = std::fs::create_dir_all(&thumbs_dir);
        Self {
            client,
            cache_dir: thumbs_dir,
        }
    }

    pub async fn handle_proxy(&self, query: ImageProxyQuery) -> Response {
        let raw_url = match query.url.as_deref().map(str::trim) {
            Some(u) if u.starts_with("http://") || u.starts_with("https://") => u,
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };

        // 1. Hash target URL for disk caching
        let mut hasher = Sha256::new();
        hasher.update(raw_url.as_bytes());
        let hash = hex::encode(hasher.finalize());
        let cache_path = self.cache_dir.join(format!("proxy-{hash}.img"));

        // 2. Check if already cached on disk
        if let Ok(bytes) = tokio::fs::read(&cache_path).await {
            if bytes.len() >= 100 {
                let content_type = detect_image_mime(&bytes);
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, content_type),
                        (header::CACHE_CONTROL, "public, max-age=31536000"),
                    ],
                    bytes,
                )
                    .into_response();
            }
        }

        // 3. Fetch from remote upstream
        match self
            .client
            .get(raw_url)
            .timeout(std::time::Duration::from_secs(6))
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    let content_type = resp
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string)
                        .unwrap_or_else(|| "image/jpeg".to_string());

                    if let Ok(bytes) = resp.bytes().await {
                        if bytes.len() >= 100 {
                            let bytes_vec = bytes.to_vec();
                            let path_clone = cache_path.clone();
                            let data_clone = bytes_vec.clone();
                            tokio::spawn(async move {
                                let _ = tokio::fs::write(path_clone, data_clone).await;
                            });

                            return (
                                StatusCode::OK,
                                [
                                    (header::CONTENT_TYPE, content_type.as_str()),
                                    (header::CACHE_CONTROL, "public, max-age=31536000"),
                                ],
                                bytes_vec,
                            )
                                .into_response();
                        }
                    }
                }

                // If upstream failed, redirect browser to original image URL
                debug!("Image proxy failed for {}, redirecting to origin", raw_url);
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, raw_url)],
                    (),
                )
                    .into_response()
            }
            Err(err) => {
                warn!("Image proxy fetch error: {}", err);
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, raw_url)],
                    (),
                )
                    .into_response()
            }
        }
    }
}

fn detect_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 3 && &bytes[0..3] == b"\xFF\xD8\xFF" {
        "image/jpeg"
    } else if bytes.len() >= 8 && &bytes[0..8] == b"\x89PNG\r\n\x1a\n" {
        "image/png"
    } else if bytes.len() >= 4 && &bytes[0..4] == b"RIFF" && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.len() >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"<?xml") || bytes.starts_with(b"<svg") {
        "image/svg+xml"
    } else {
        "image/jpeg"
    }
}
