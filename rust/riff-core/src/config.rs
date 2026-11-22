use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use directories::BaseDirs;
use uuid::Uuid;

/// Rust core sidecar for Riff.
#[derive(Debug, Clone, Parser)]
#[command(name = "riff-core", version, about)]
pub struct Args {
    /// Port to listen on. 0 picks a free port.
    #[arg(long, env = "RIFF_PORT", default_value_t = 0)]
    pub port: u16,

    /// Data directory. Defaults to %LOCALAPPDATA%\\Riff on Windows.
    #[arg(long, env = "RIFF_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Music library directory. Defaults to <data-dir>/library.
    #[arg(long, env = "RIFF_LIBRARY_DIR")]
    pub library_dir: Option<PathBuf>,

    /// Shared token sent by the renderer in the x-riff-token header.
    #[arg(long, env = "RIFF_TOKEN")]
    pub token: Option<String>,

    /// Path to the yt-dlp binary. Falls back to <data-dir>/bin and PATH.
    #[arg(long, env = "RIFF_YTDLP")]
    pub ytdlp: Option<PathBuf>,

    /// Path to the ffmpeg binary. Falls back to <data-dir>/bin and PATH.
    #[arg(long, env = "RIFF_FFMPEG")]
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub data_dir: PathBuf,
    pub library_dir: PathBuf,
    pub db_path: PathBuf,
    pub token: String,
    pub ytdlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

impl Config {
    pub fn new(
        port: u16,
        token: Option<String>,
        data_dir: Option<PathBuf>,
        library_dir: Option<PathBuf>,
        ytdlp: Option<PathBuf>,
        ffmpeg: Option<PathBuf>,
    ) -> Result<Self> {
        let data_dir = match data_dir {
            Some(path) => path,
            None => default_data_dir()?,
        };
        let library_dir = library_dir.unwrap_or_else(|| data_dir.join("library"));

        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data directory {}", data_dir.display()))?;
        std::fs::create_dir_all(&library_dir)
            .with_context(|| format!("create library directory {}", library_dir.display()))?;

        let data_dir = std::path::absolute(&data_dir)
            .with_context(|| format!("resolve data directory {}", data_dir.display()))?;
        let library_dir = std::path::absolute(&library_dir)
            .with_context(|| format!("resolve library directory {}", library_dir.display()))?;

        let db_path = data_dir.join("riff-core.sqlite3");
        let token = token.unwrap_or_else(generate_token);
        let ytdlp = ytdlp.or_else(|| resolve_binary(&data_dir, "yt-dlp"));
        let ffmpeg = ffmpeg.or_else(|| resolve_binary(&data_dir, "ffmpeg"));

        Ok(Self {
            port,
            data_dir,
            library_dir,
            db_path,
            token,
            ytdlp,
            ffmpeg,
        })
    }

    pub fn default_or_fallback() -> Self {
        Self::new(0, None, None, None, None, None).unwrap_or_else(|_| Self {
            port: 0,
            data_dir: PathBuf::from("."),
            library_dir: PathBuf::from("./library"),
            db_path: PathBuf::from("./riff-core.sqlite3"),
            token: "riff-token".to_string(),
            ytdlp: None,
            ffmpeg: None,
        })
    }

    pub fn from_args() -> Result<Self> {
        let args = Args::parse();
        Self::new(
            args.port,
            args.token,
            args.data_dir,
            args.library_dir,
            args.ytdlp,
            args.ffmpeg,
        )
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::default_or_fallback()
    }
}

fn default_data_dir() -> Result<PathBuf> {
    let base = BaseDirs::new().context("could not determine the user data directory")?;
    Ok(base.data_local_dir().join("Riff"))
}

fn generate_token() -> String {
    // 64 hex characters. The renderer only needs an opaque value.
    format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn resolve_binary(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(windows) {
        candidates.push(data_dir.join("bin").join(format!("{name}.exe")));
        candidates.push(PathBuf::from("bin").join(format!("{name}.exe")));
    }
    candidates.push(data_dir.join("bin").join(name));
    candidates.push(PathBuf::from("bin").join(name));

    candidates.into_iter().find(|path| path.is_file())
}
