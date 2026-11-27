use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/zaDotMe/riff-Rust/releases/latest";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub version: String,
    pub tag_name: String,
    pub notes: String,
    pub html_url: String,
    pub asset_name: String,
    pub download_url: String,
    pub asset_size: u64,
}

pub fn parse_version(raw: &str) -> Vec<u64> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next().unwrap_or(trimmed);
    core.split('.')
        .filter_map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                None
            } else {
                digits.parse::<u64>().ok()
            }
        })
        .collect()
}

pub fn is_newer_version(current: &str, latest: &str) -> bool {
    let cur = parse_version(current);
    let lat = parse_version(latest);
    let len = cur.len().max(lat.len());
    for i in 0..len {
        let a = *cur.get(i).unwrap_or(&0);
        let b = *lat.get(i).unwrap_or(&0);
        if b > a {
            return true;
        }
        if b < a {
            return false;
        }
    }
    false
}

pub fn select_asset_for_platform(assets: &[Value], target_os: &str) -> Option<(String, String, u64)> {
    let preferences: &[&str] = match target_os {
        "windows" => &["Riff-Setup-windows-x64.exe", "Riff-Slint-windows-x64.exe"],
        "linux" => &[
            "riff-slint-linux-x86_64",
            "Riff-linux-x86_64.tar.gz",
            "Riff-linux-amd64.deb",
        ],
        "android" => &["Riff-google-tv-arm64-v8a.apk"],
        _ => &[],
    };

    for wanted in preferences {
        if let Some(asset) = assets
            .iter()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(*wanted))
        {
            if let (Some(name), Some(url)) = (
                asset.get("name").and_then(|v| v.as_str()),
                asset.get("browser_download_url").and_then(|v| v.as_str()),
            ) {
                let size = asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                return Some((name.to_string(), url.to_string(), size));
            }
        }
    }

    if target_os == "android" {
        if let Some(asset) = assets.iter().find(|a| {
            a.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.ends_with(".apk"))
                .unwrap_or(false)
        }) {
            if let (Some(name), Some(url)) = (
                asset.get("name").and_then(|v| v.as_str()),
                asset.get("browser_download_url").and_then(|v| v.as_str()),
            ) {
                let size = asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                return Some((name.to_string(), url.to_string(), size));
            }
        }
    }

    None
}

pub fn parse_release_json(json: &Value, current_version: &str, target_os: &str) -> Option<ReleaseInfo> {
    let tag_name = json.get("tag_name").and_then(|v| v.as_str())?.trim().to_string();
    if !is_newer_version(current_version, &tag_name) {
        return None;
    }

    let version = tag_name.trim_start_matches(['v', 'V']).to_string();
    let notes = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let html_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/zaDotMe/riff-Rust/releases/latest")
        .to_string();
    let assets = json
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[]);

    let (asset_name, download_url, asset_size) =
        select_asset_for_platform(assets, target_os).unwrap_or_else(|| {
            (String::new(), html_url.clone(), 0)
        });

    Some(ReleaseInfo {
        version,
        tag_name,
        notes,
        html_url,
        asset_name,
        download_url,
        asset_size,
    })
}

pub async fn check_latest_release(
    http: &reqwest::Client,
    current_version: &str,
) -> Result<Option<ReleaseInfo>> {
    let resp = http
        .get(GITHUB_LATEST_RELEASE_URL)
        .header("User-Agent", format!("Riff-Updater/{current_version}"))
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("Failed to reach GitHub Releases API")?;

    if !resp.status().is_success() {
        return Err(anyhow!("GitHub Releases API returned HTTP {}", resp.status()));
    }

    let json: Value = resp.json().await.context("Invalid GitHub release JSON")?;
    Ok(parse_release_json(&json, current_version, std::env::consts::OS))
}

pub async fn download_release_asset<F>(
    http: &reqwest::Client,
    info: &ReleaseInfo,
    mut on_progress: F,
) -> Result<PathBuf>
where
    F: FnMut(u64, u64) + Send,
{
    if info.download_url.is_empty() || info.asset_name.is_empty() {
        return Err(anyhow!("No downloadable binary asset found for this platform"));
    }

    let dir = std::env::temp_dir().join(format!("riff-update-{}", info.version));
    tokio::fs::create_dir_all(&dir)
        .await
        .context("Failed to create temp update directory")?;
    let dest = dir.join(&info.asset_name);

    let mut resp = http
        .get(&info.download_url)
        .header("User-Agent", format!("Riff-Updater/{}", info.version))
        .timeout(Duration::from_secs(180))
        .send()
        .await
        .context("Failed to start update download")?;

    if !resp.status().is_success() {
        return Err(anyhow!("Update download failed with HTTP {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(info.asset_size);
    let mut file = tokio::fs::File::create(&dest)
        .await
        .context("Failed to create update installer file")?;
    let mut downloaded: u64 = 0;

    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = resp.chunk().await.context("Download stream interrupted")? {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    file.flush().await?;

    if info.asset_size > 0 && downloaded != info.asset_size {
        return Err(anyhow!(
            "Downloaded size mismatch ({downloaded} != {} bytes)",
            info.asset_size
        ));
    }

    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn version_comparison_handles_multi_digit_patch() {
        assert!(is_newer_version("0.7.81", "v0.7.82"));
        assert!(is_newer_version("0.7.82", "v0.7.83"));
        assert!(is_newer_version("0.7.8", "v0.7.81"));
        assert!(is_newer_version("0.7.99", "v0.7.100"));
        assert!(is_newer_version("0.7.81", "v0.8.0"));
        assert!(!is_newer_version("0.7.81", "v0.7.81"));
        assert!(!is_newer_version("0.7.82", "v0.7.81"));
    }

    #[test]
    fn selects_correct_asset_for_each_platform() {
        let assets = vec![
            json!({
                "name": "Riff-Setup-windows-x64.exe",
                "browser_download_url": "https://example.com/win-setup.exe",
                "size": 12345
            }),
            json!({
                "name": "riff-slint-linux-x86_64",
                "browser_download_url": "https://example.com/linux-bin",
                "size": 23456
            }),
            json!({
                "name": "Riff-google-tv-arm64-v8a.apk",
                "browser_download_url": "https://example.com/tv.apk",
                "size": 34567
            }),
        ];

        assert_eq!(
            select_asset_for_platform(&assets, "windows").unwrap().0,
            "Riff-Setup-windows-x64.exe"
        );
        assert_eq!(
            select_asset_for_platform(&assets, "linux").unwrap().0,
            "riff-slint-linux-x86_64"
        );
        assert_eq!(
            select_asset_for_platform(&assets, "android").unwrap().0,
            "Riff-google-tv-arm64-v8a.apk"
        );
    }

    #[test]
    fn parses_github_release_json() {
        let payload = json!({
            "tag_name": "v0.7.82",
            "body": "- Auto-update\n- Cover layout",
            "html_url": "https://github.com/zaDotMe/riff-Rust/releases/tag/v0.7.82",
            "assets": [
                {
                    "name": "Riff-Setup-windows-x64.exe",
                    "browser_download_url": "https://example.com/Riff-Setup-windows-x64.exe",
                    "size": 999
                }
            ]
        });

        let info = parse_release_json(&payload, "0.7.81", "windows").expect("should find update");
        assert_eq!(info.version, "0.7.82");
        assert_eq!(info.asset_name, "Riff-Setup-windows-x64.exe");
        assert_eq!(info.asset_size, 999);
        assert!(parse_release_json(&payload, "0.7.82", "windows").is_none());
    }
}
