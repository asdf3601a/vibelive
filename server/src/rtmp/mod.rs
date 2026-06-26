pub(crate) mod enhanced;
pub mod server;
pub mod session;

pub use server::start_rtmp_server;

use crate::hls::HlsStreamState;
use crate::recording::Fmp4Recorder;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct StreamManager {
    publishers: HashMap<String, PublisherInfo>,
    pending_streams: HashMap<String, PendingStream>,
}

#[derive(Clone, serde::Serialize)]
pub struct TrackInfo {
    pub track_id: u32,
    pub hls_url: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct PublisherInfo {
    pub stream_key: String,
    pub app_name: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub metadata: Option<StreamMeta>,
    pub tracks: Vec<TrackInfo>,
    #[serde(skip)]
    pub disconnected_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip)]
    pub ended: Arc<AtomicBool>,
    #[serde(skip)]
    pub last_thumbnail_attempt_secs: Arc<AtomicU64>,
}

#[derive(Clone, serde::Serialize)]
pub struct StreamMeta {
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub audio_codec: String,
    pub video_bitrate: u32,
    pub audio_bitrate: u32,
    pub framerate: f64,
}

pub struct PendingStream {
    pub stream_key: String,
    pub disconnected_at: chrono::DateTime<chrono::Utc>,
    pub hls_state: HlsStreamState,
    pub recorder: Option<Fmp4Recorder>,
}

impl StreamManager {
    pub fn new() -> Self {
        Self {
            publishers: HashMap::new(),
            pending_streams: HashMap::new(),
        }
    }

    pub fn add_publisher(&mut self, stream_key: &str, info: PublisherInfo) {
        self.publishers.insert(stream_key.to_string(), info);
    }

    pub fn remove_publisher(&mut self, stream_key: &str) {
        self.publishers.remove(stream_key);
    }

    pub fn is_publishing(&self, stream_key: &str) -> bool {
        self.publishers.contains_key(stream_key)
    }

    pub fn is_live_or_pending(&self, stream_key: &str) -> bool {
        self.publishers.contains_key(stream_key) || self.pending_streams.contains_key(stream_key)
    }

    pub fn mark_disconnected(
        &mut self,
        stream_key: &str,
        hls_state: HlsStreamState,
        recorder: Option<Fmp4Recorder>,
    ) {
        let now = chrono::Utc::now();
        if let Some(ref mut info) = self.publishers.get_mut(stream_key) {
            info.disconnected_at = Some(now);
        }
        self.pending_streams.insert(
            stream_key.to_string(),
            PendingStream {
                stream_key: stream_key.to_string(),
                disconnected_at: now,
                hls_state,
                recorder,
            },
        );
    }

    pub fn reconnect(&mut self, stream_key: &str) -> Option<PendingStream> {
        let pending = self.pending_streams.remove(stream_key);
        if pending.is_some()
            && let Some(ref mut info) = self.publishers.get_mut(stream_key)
        {
            info.disconnected_at = None;
            info.ended.store(false, Ordering::SeqCst);
        }
        pending
    }

    pub fn publishers(&self) -> &HashMap<String, PublisherInfo> {
        &self.publishers
    }

    pub fn publishers_mut(&mut self) -> &mut HashMap<String, PublisherInfo> {
        &mut self.publishers
    }

    pub fn pending_streams_mut(&mut self) -> &mut HashMap<String, PendingStream> {
        &mut self.pending_streams
    }

    pub fn get_publisher(&self, key: &str) -> Option<&PublisherInfo> {
        self.publishers.get(key)
    }

    pub fn remove_pending_stream(&mut self, key: &str) -> Option<PendingStream> {
        self.pending_streams.remove(key)
    }
}
