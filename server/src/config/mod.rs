use std::env;

#[derive(Clone)]
pub struct Config {
    pub rtmp_port: u16,
    pub api_port: u16,
    pub media_dir: String,
    pub hls_segment_duration: u32,
    pub hls_segments_keep: u32,
    pub recording_enabled: bool,
    pub thumbnail_ttl_seconds: u32,
    pub thumbnail_default_width: u32,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            rtmp_port: env::var("RTMP_PORT")
                .unwrap_or_else(|_| "1935".into())
                .parse()?,
            api_port: env::var("API_PORT")
                .unwrap_or_else(|_| "8080".into())
                .parse()?,
            media_dir: env::var("MEDIA_DIR")
                .unwrap_or_else(|_| "./data".into()),
            hls_segment_duration: env::var("HLS_SEGMENT_DURATION")
                .unwrap_or_else(|_| "2".into())
                .parse()?,
            hls_segments_keep: env::var("HLS_SEGMENTS_KEEP")
                .unwrap_or_else(|_| "10".into())
                .parse()?,
            recording_enabled: env::var("RECORDING_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()?,
            thumbnail_ttl_seconds: env::var("THUMBNAIL_TTL_SECONDS")
                .unwrap_or_else(|_| "5".into())
                .parse()?,
            thumbnail_default_width: env::var("THUMBNAIL_DEFAULT_WIDTH")
                .unwrap_or_else(|_| "480".into())
                .parse()?,
        })
    }
}
