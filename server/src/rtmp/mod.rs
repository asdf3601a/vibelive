pub mod enhanced;
pub mod server;
pub mod session;
pub mod stream;

pub use server::start_rtmp_server;

use std::collections::HashMap;

pub struct StreamManager {
    pub publishers: HashMap<String, PublisherInfo>,
}

#[derive(Clone, serde::Serialize)]
pub struct PublisherInfo {
    pub stream_key: String,
    pub app_name: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub metadata: Option<StreamMeta>,
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

impl StreamManager {
    pub fn new() -> Self {
        Self {
            publishers: HashMap::new(),
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
}
