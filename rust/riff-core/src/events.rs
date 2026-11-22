use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::models::Track;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum RiffEvent {
    TrackChanged {
        track: Option<Track>,
    },
    PlaybackState {
        is_playing: bool,
        position_sec: f64,
        duration_sec: f64,
    },
    VolumeChanged {
        volume: f32,
        muted: bool,
    },
    Spectrum {
        bands: Vec<f32>,
    },
    QueueUpdated {
        tracks: Vec<Track>,
        current_index: usize,
    },
    UiModeChanged {
        mode: String,
    },
    LibraryUpdated {
        count: i64,
    },
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<RiffEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn emit(&self, event: RiffEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RiffEvent> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}
