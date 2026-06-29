use std::env;

#[derive(Clone)]
pub struct Config {
    pub rtmp_host: String,
    pub rtmp_port: u16,
    pub api_host: String,
    pub api_port: u16,
    pub media_dir: String,
    pub hls_segment_duration: u32,
    pub hls_segments_keep: u32,
    pub recording_enabled: bool,
    pub thumbnail_sizes: Vec<u32>,
    pub thumbnail_interval_seconds: u32,
    pub thumbnail_live_update: bool,
    pub recordings_base_url: String,
    pub stream_grace_period_seconds: u64,
    pub recording_remux_enabled: bool,
    pub recording_remux_concurrency: u32,
    pub thumbnail_ffmpeg_concurrency: u32,
    pub thumbnail_rate_limit_seconds: u32,
    pub cors_allowed_origins: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let thumbnail_sizes_str = env::var("THUMBNAIL_SIZES").unwrap_or_else(|_| "320,480".into());
        let thumbnail_sizes: Vec<u32> = thumbnail_sizes_str
            .split(',')
            .map(|s| s.trim().parse())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            rtmp_host: env::var("RTMP_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            rtmp_port: env::var("RTMP_PORT")
                .unwrap_or_else(|_| "1935".into())
                .parse()?,
            api_host: env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            api_port: env::var("API_PORT")
                .unwrap_or_else(|_| "8080".into())
                .parse()?,
            media_dir: env::var("MEDIA_DIR").unwrap_or_else(|_| "./data".into()),
            hls_segment_duration: env::var("HLS_SEGMENT_DURATION")
                .unwrap_or_else(|_| "2".into())
                .parse()?,
            hls_segments_keep: env::var("HLS_SEGMENTS_KEEP")
                .unwrap_or_else(|_| "10".into())
                .parse()?,
            recording_enabled: env::var("RECORDING_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()?,
            thumbnail_sizes,
            thumbnail_interval_seconds: env::var("THUMBNAIL_INTERVAL_SECONDS")
                .unwrap_or_else(|_| "300".into())
                .parse()?,
            thumbnail_live_update: env::var("THUMBNAIL_LIVE_UPDATE")
                .unwrap_or_else(|_| "true".into())
                .parse()?,
            recordings_base_url: env::var("RECORDINGS_BASE_URL")
                .unwrap_or_else(|_| "/recordings".into()),
            stream_grace_period_seconds: env::var("STREAM_GRACE_PERIOD_SECONDS")
                .unwrap_or_else(|_| "30".into())
                .parse()?,
            recording_remux_enabled: env::var("RECORDING_REMUX_ENABLED")
                .unwrap_or_else(|_| "true".into())
                .parse()?,
            recording_remux_concurrency: env::var("RECORDING_REMUX_CONCURRENCY")
                .unwrap_or_else(|_| "4".into())
                .parse()?,
            thumbnail_ffmpeg_concurrency: env::var("THUMBNAIL_FFMPEG_CONCURRENCY")
                .unwrap_or_else(|_| "4".into())
                .parse()?,
            thumbnail_rate_limit_seconds: env::var("THUMBNAIL_RATE_LIMIT_SECONDS")
                .unwrap_or_else(|_| "5".into())
                .parse()?,
            cors_allowed_origins: env::var("CORS_ALLOWED_ORIGINS").unwrap_or_else(|_| "*".into()),
        })
    }

    /// Validate that all configuration values are within acceptable ranges.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.thumbnail_sizes.is_empty() {
            return Err(anyhow::anyhow!(
                "THUMBNAIL_SIZES must contain at least one width (e.g. \"320\")"
            ));
        }
        for &size in &self.thumbnail_sizes {
            if size == 0 {
                return Err(anyhow::anyhow!(
                    "THUMBNAIL_SIZES contains zero-width entry — all sizes must be > 0"
                ));
            }
            if size > 7680 {
                return Err(anyhow::anyhow!(
                    "THUMBNAIL_SIZES contains width {} which exceeds maximum 7680",
                    size
                ));
            }
        }
        if self.hls_segment_duration == 0 {
            return Err(anyhow::anyhow!(
                "HLS_SEGMENT_DURATION must be > 0 (seconds)"
            ));
        }
        if self.hls_segments_keep < 3 {
            return Err(anyhow::anyhow!(
                "HLS_SEGMENTS_KEEP must be >= 3 to maintain a valid sliding window"
            ));
        }
        if self.thumbnail_interval_seconds == 0 {
            return Err(anyhow::anyhow!(
                "THUMBNAIL_INTERVAL_SECONDS must be > 0"
            ));
        }
        if self.thumbnail_ffmpeg_concurrency == 0 {
            return Err(anyhow::anyhow!(
                "THUMBNAIL_FFMPEG_CONCURRENCY must be > 0"
            ));
        }
        if self.recording_remux_concurrency == 0 {
            return Err(anyhow::anyhow!(
                "RECORDING_REMUX_CONCURRENCY must be > 0"
            ));
        }
        Ok(())
    }
}
