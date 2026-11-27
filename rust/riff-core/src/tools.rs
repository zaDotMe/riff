use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct ToolsStatus {
    pub ytdlp_installed: bool,
    pub ffmpeg_installed: bool,
}

pub fn bin_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

pub fn resolve_binary_path(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let exe_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    let local = bin_dir(data_dir).join(&exe_name);
    if local.is_file() {
        return Some(local);
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(&exe_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }

    None
}

pub fn check_binary_exists(data_dir: &Path, name: &str) -> bool {
    resolve_binary_path(data_dir, name).is_some()
}

pub fn check_tools_status(data_dir: &Path) -> ToolsStatus {
    ToolsStatus {
        ytdlp_installed: check_binary_exists(data_dir, "yt-dlp"),
        ffmpeg_installed: check_binary_exists(data_dir, "ffmpeg"),
    }
}

async fn download_with_progress<F>(
    http: &reqwest::Client,
    url: &str,
    dest: &Path,
    mut on_progress: F,
) -> Result<()>
where
    F: FnMut(f32) + Send,
{
    let mut resp = http
        .get(url)
        .header("User-Agent", "Riff-Setup/1.0")
        .timeout(Duration::from_secs(240))
        .send()
        .await
        .with_context(|| format!("Failed to download {url}"))?;

    if !resp.status().is_success() {
        return Err(anyhow!("Download failed with HTTP {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("Failed to create {}", dest.display()))?;
    let mut downloaded: u64 = 0;

    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = resp.chunk().await.context("Download stream interrupted")? {
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        if total > 0 {
            on_progress((downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32);
        }
    }
    file.flush().await?;
    on_progress(1.0);
    Ok(())
}

pub async fn install_ytdlp<F>(
    http: &reqwest::Client,
    data_dir: &Path,
    on_progress: F,
) -> Result<PathBuf>
where
    F: FnMut(f32) + Send,
{
    let dir = bin_dir(data_dir);
    tokio::fs::create_dir_all(&dir).await?;

    let (url, filename) = if cfg!(windows) {
        (
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
            "yt-dlp.exe",
        )
    } else {
        (
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp",
            "yt-dlp",
        )
    };

    let dest = dir.join(filename);
    download_with_progress(http, url, &dest, on_progress).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&dest) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&dest, perms);
        }
    }

    Ok(dest)
}

pub async fn install_ffmpeg<F>(
    http: &reqwest::Client,
    data_dir: &Path,
    on_progress: F,
) -> Result<PathBuf>
where
    F: FnMut(f32) + Send,
{
    let dir = bin_dir(data_dir);
    tokio::fs::create_dir_all(&dir).await?;

    let (url, archive_name, target_bin) = if cfg!(windows) {
        (
            "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip",
            "ffmpeg-download.zip",
            "ffmpeg.exe",
        )
    } else {
        (
            "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-linux64-gpl.tar.xz",
            "ffmpeg-download.tar.xz",
            "ffmpeg",
        )
    };

    let tmp_dir = dir.join("ffmpeg-tmp");
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    tokio::fs::create_dir_all(&tmp_dir).await?;
    let archive_path = tmp_dir.join(archive_name);

    download_with_progress(http, url, &archive_path, on_progress).await?;

    let arch_clone = archive_path.clone();
    let tmp_clone = tmp_dir.clone();
    let target_name = target_bin.to_string();
    let extracted = tokio::task::spawn_blocking(move || -> Result<Option<PathBuf>> {
        if target_name.ends_with(".exe") {
            let file = std::fs::File::open(&arch_clone)?;
            let mut archive = zip::ZipArchive::new(file)?;
            for i in 0..archive.len() {
                let mut entry = archive.by_index(i)?;
                let is_target = entry
                    .enclosed_name()
                    .and_then(|p| p.file_name().map(|s| s == std::ffi::OsStr::new(&target_name)))
                    .unwrap_or(false);
                if is_target {
                    let out_path = tmp_clone.join(&target_name);
                    let mut out_file = std::fs::File::create(&out_path)?;
                    std::io::copy(&mut entry, &mut out_file)?;
                    return Ok(Some(out_path));
                }
            }
        } else {
            let file = std::fs::File::open(&arch_clone)?;
            let mut reader = std::io::BufReader::new(file);
            let mut decompressed = Vec::new();
            lzma_rs::xz_decompress(&mut reader, &mut decompressed)
                .map_err(|e| anyhow!("xz decompress failed: {e}"))?;
            let mut archive = tar::Archive::new(std::io::Cursor::new(decompressed));
            for entry in archive.entries()? {
                let mut entry = entry?;
                let is_target = entry
                    .path()
                    .ok()
                    .and_then(|p| p.file_name().map(|s| s == std::ffi::OsStr::new(&target_name)))
                    .unwrap_or(false);
                if is_target {
                    let out_path = tmp_clone.join(&target_name);
                    entry.unpack(&out_path)?;
                    return Ok(Some(out_path));
                }
            }
        }
        Ok(None)
    })
    .await
    .map_err(|e| anyhow!("extract task failed: {e}"))??;

    let dest = dir.join(target_bin);
    if let Some(src) = extracted {
        tokio::fs::copy(&src, &dest).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&dest) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(&dest, perms);
            }
        }
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        Ok(dest)
    } else {
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        Err(anyhow!("ffmpeg binary not found inside downloaded archive"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_local_bin_tools() {
        let dir = std::env::temp_dir().join(format!("riff_tools_test_{}", uuid::Uuid::new_v4()));
        let bin = bin_dir(&dir);
        std::fs::create_dir_all(&bin).unwrap();
        let yt_name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
        std::fs::write(bin.join(yt_name), b"dummy").unwrap();
        assert!(check_binary_exists(&dir, "yt-dlp"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
