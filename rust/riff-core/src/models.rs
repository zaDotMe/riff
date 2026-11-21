use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub path: String,
    pub file: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: f64,
    pub thumbnail: String,
    pub added_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub id: String,
    pub platform: String,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub thumbnail: String,
}

impl From<Track> for LibraryItem {
    fn from(track: Track) -> Self {
        Self {
            id: track.id,
            platform: "local".to_string(),
            title: track.title,
            artist: track.artist,
            duration: track.duration,
            thumbnail: track.thumbnail,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAddRequest {
    #[serde(default)]
    pub items: Vec<LibraryAddItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAddItem {
    pub id: Option<String>,
    pub file: String,
    pub name: Option<String>,
    #[allow(dead_code)]
    pub ext: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryUpdateRequest {
    pub id: String,
    pub duration: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRemoveRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    pub dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamQuery {
    pub url: Option<String>,
    pub id: Option<String>,
    pub platform: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub timeout: Option<u64>,
    #[allow(dead_code)]
    pub quality: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProxyQuery {
    pub url: Option<String>,
}
