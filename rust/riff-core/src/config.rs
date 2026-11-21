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
    pub fn from_args() -> Result<Self> {
        let args = Args::parse();

        let data_dir = match args.data_dir {
            Some(path) => path,
            None => default_data_dir()?,
        };
        let library_dir = args
            .library_dir
            .unwrap_or_else(|| data_dir.join("library"));

        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data directory {}", data_dir.display()))?;
        std::fs::create_dir_all(&library_dir)
            .with_context(|| format!("create library directory {}", library_dir.display()))?;

        let data_dir = std::path::absolute(&data_dir)
            .with_context(|| format!("resolve data directory {}", data_dir.display()))?;
        let library_dir = std::path::absolute(&library_dir)
            .with_context(|| format!("resolve library directory {}", library_dir.display()))?;

        let db_path = data_dir.join("riff-core.sqlite3");
        let token = args.token.unwrap_or_else(generate_token);
        let ytdlp = args
            .ytdlp
            .or_else(|| resolve_binary(&data_dir, "yt-dlp"));
        let ffmpeg = args
            .ffmpeg
            .or_else(|| resolve_binary(&data_dir, "ffmpeg"));

        Ok(Self {
            port: args.port,
            data_dir,
            library_dir,
            db_path,
            token,
            ytdlp,
            ffmpeg,
        })
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
