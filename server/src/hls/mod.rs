pub mod fmp4;
pub mod mpegts;
mod playlist;

use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

pub struct HlsManager {
    pub base_dir: PathBuf,
    pub streams: Vec<String>,
}

impl HlsManager {
    pub fn new(media_dir: &str) -> Self {
        Self {
            base_dir: PathBuf::from(media_dir).join("hls"),
            streams: Vec::new(),
        }
    }
}

pub struct HlsStreamState {
    stream_dir: PathBuf,
    segment_duration: u32,
    current_segment_start: u64,
    segment_index: u32,
    current_file: Option<tokio::fs::File>,
    fmp4_muxer: fmp4::Fmp4Muxer,
    has_video: bool,
    has_audio: bool,
    first_video_ts: Option<u32>,
    first_audio_ts: Option<u32>,
    init_written: bool,
    init_data: Option<Vec<u8>>,
    segment_data: Vec<Vec<u8>>,
    audio_codec: Option<fmp4::AudioCodec>,
    timestamp_offset: u64,
    last_video_pts: u64,
    last_audio_pts: u64,
}

impl HlsStreamState {
    pub fn new(media_dir: &str, stream_key: &str, segment_duration: u32) -> Self {
        let dir = PathBuf::from(media_dir).join("hls").join(stream_key);
        std::fs::create_dir_all(&dir).ok();
        Self {
            stream_dir: dir,
            segment_duration,
            current_segment_start: 0,
            segment_index: 0,
            current_file: None,
            fmp4_muxer: fmp4::Fmp4Muxer::new(),
            has_video: false,
            has_audio: false,
            first_video_ts: None,
            first_audio_ts: None,
            init_written: false,
            init_data: None,
            segment_data: Vec::new(),
            audio_codec: None,
            timestamp_offset: 0,
            last_video_pts: 0,
            last_audio_pts: 0,
        }
    }

    pub fn stream_dir(&self) -> &PathBuf {
        &self.stream_dir
    }

    fn segment_path(&self, index: u32) -> PathBuf {
        self.stream_dir.join(format!("segment{:05}.m4s", index))
    }

    fn playlist_path(&self) -> PathBuf {
        self.stream_dir.join("index.m3u8")
    }

    pub async fn set_video_config(&mut self, config: &[u8], codec: fmp4::VideoCodec) -> anyhow::Result<()> {
        self.fmp4_muxer.set_video_codec(codec, 1920, 1080);
        self.fmp4_muxer.set_video_config(config.to_vec());
        self.init_written = false;
        self.write_init_segment().await?;
        Ok(())
    }

    pub async fn set_audio_config(&mut self, codec: fmp4::AudioCodec, config: &[u8]) -> anyhow::Result<()> {
        self.audio_codec = Some(codec);
        self.fmp4_muxer.set_audio_codec(codec);
        self.fmp4_muxer.set_audio_config(config.to_vec());
        self.init_written = false;
        self.write_init_segment().await?;
        Ok(())
    }

    pub async fn write_video(&mut self, data: &[u8], timestamp: u32, is_keyframe: bool) -> anyhow::Result<()> {
        if self.first_video_ts.is_none() {
            self.first_video_ts = Some(timestamp);
        }
        let base_ts = self.first_video_ts.unwrap_or(0);
        let pts = (timestamp - base_ts) as u64 + self.timestamp_offset;

        if !self.has_video {
            self.has_video = true;
            self.current_segment_start = pts;
            self.rotate_segment().await?;
        }

        let elapsed_since_start = pts.saturating_sub(self.current_segment_start);
        if elapsed_since_start > (self.segment_duration as u64 * 1000) && self.current_file.is_some() {
            self.finalize_segment().await?;
            self.current_segment_start = pts;
            self.segment_index += 1;
            self.rotate_segment().await?;
        }

        self.fmp4_muxer.add_video_sample(data.to_vec(), pts, pts, is_keyframe);
        self.last_video_pts = pts;
        Ok(())
    }

    pub async fn write_audio(&mut self, data: &[u8], timestamp: u32) -> anyhow::Result<()> {
        if self.first_audio_ts.is_none() {
            self.first_audio_ts = Some(timestamp);
        }
        if !self.has_audio {
            self.has_audio = true;
            if let Some(codec) = self.audio_codec {
                self.fmp4_muxer.set_audio_codec(codec);
            } else {
                self.fmp4_muxer.set_audio_codec(fmp4::AudioCodec::AAC);
            }
        }
        if self.current_file.is_none() {
            return Ok(());
        }
        let base_ts = self.first_audio_ts.unwrap_or(0);
        let pts = (timestamp - base_ts) as u64 + self.timestamp_offset;
        self.fmp4_muxer.add_audio_sample(data.to_vec(), pts);
        self.last_audio_pts = pts;
        Ok(())
    }

    async fn write_init_segment(&mut self) -> anyhow::Result<()> {
        if self.init_written {
            return Ok(());
        }
        let init = self.fmp4_muxer.init_segment();
        let path = self.stream_dir.join("init.mp4");
        tokio::fs::write(&path, &init).await?;
        self.init_data = Some(init);
        self.init_written = true;
        Ok(())
    }

    async fn rotate_segment(&mut self) -> anyhow::Result<()> {
        let path = self.segment_path(self.segment_index);
        let file = tokio::fs::File::create(&path).await?;
        self.current_file = Some(file);
        self.update_playlist().await?;
        Ok(())
    }

    pub async fn finalize_segment(&mut self) -> anyhow::Result<()> {
        if let Some(mut file) = self.current_file.take() {
            if let Some(fragment) = self.fmp4_muxer.flush_combined_fragment() {
                file.write_all(&fragment).await?;
                self.segment_data.push(fragment);
            }
            file.flush().await?;
            file.sync_all().await?;
        }
        self.update_playlist().await?;
        Ok(())
    }

    /// Finalize the current segment and prepare a fresh one for potential reconnect.
    pub async fn prepare_for_grace_period(&mut self) -> anyhow::Result<()> {
        self.finalize_segment().await?;
        self.timestamp_offset = self.last_video_pts.max(self.last_audio_pts).saturating_add(33);
        self.first_video_ts = None;
        self.first_audio_ts = None;
        self.segment_index += 1;
        self.rotate_segment().await?;
        self.current_segment_start = self.timestamp_offset;
        Ok(())
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.finalize_segment().await?;
        let path = self.playlist_path();
        let mut playlist = tokio::fs::read_to_string(&path).await?;
        if !playlist.ends_with("#EXT-X-ENDLIST\n") {
            playlist.push_str("#EXT-X-ENDLIST\n");
            tokio::fs::write(&path, playlist).await?;
        }
        Ok(())
    }

    pub fn take_recording_data(&mut self) -> (Option<Vec<u8>>, Vec<Vec<u8>>) {
        (self.init_data.take(), std::mem::take(&mut self.segment_data))
    }

    async fn update_playlist(&mut self) -> anyhow::Result<()> {
        let mut playlist = String::new();
        playlist.push_str("#EXTM3U\n");
        playlist.push_str("#EXT-X-VERSION:7\n");
        playlist.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", self.segment_duration));
        playlist.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
        playlist.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n");

        for i in 0..=self.segment_index {
            let path = self.segment_path(i);
            let include = match tokio::fs::metadata(&path).await {
                Ok(m) => m.len() > 0,
                Err(_) => false,
            };
            if include {
                playlist.push_str(&format!("#EXTINF:{}.000,\n", self.segment_duration));
                playlist.push_str(&format!("segment{:05}.m4s\n", i));
            }
        }

        let path = self.playlist_path();
        tokio::fs::write(&path, playlist).await?;
        Ok(())
    }
}

impl Drop for HlsStreamState {
    fn drop(&mut self) {
        // Synchronous drop cannot await async finalize_segment.
        // The session handler is responsible for calling finalize_segment
        // before dropping the HlsStreamState.
        let _ = self.current_file.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hls_stream_state_new() {
        let test_dir = "/tmp/hls_test_new";
        let _ = std::fs::remove_dir_all(test_dir);
        let state = HlsStreamState::new(test_dir, "testkey", 4);
        assert_eq!(state.segment_index, 0);
        assert!(state.current_file.is_none());
        assert!(!state.has_video);
        assert!(!state.has_audio);
        assert!(state.stream_dir.exists());
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_first_video_triggers_segment_creation() {
        let test_dir = "/tmp/hls_test_first_video";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut state = HlsStreamState::new(test_dir, "testkey", 4);

        // Set config first, then write video
        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state.set_video_config(&avcc_config, fmp4::VideoCodec::H264).await.unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true).await.unwrap();

        assert!(state.has_video);
        assert!(state.current_file.is_some());
        assert!(state.segment_path(0).exists());
        assert!(state.playlist_path().exists());

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_playlist_content() {
        let test_dir = "/tmp/hls_test_playlist";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut state = HlsStreamState::new(test_dir, "testkey", 2);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state.set_video_config(&avcc_config, fmp4::VideoCodec::H264).await.unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true).await.unwrap();
        // Finalize so segment appears in playlist
        state.finalize_segment().await.unwrap();

        let playlist = tokio::fs::read_to_string(state.playlist_path()).await.unwrap();
        assert!(playlist.starts_with("#EXTM3U"));
        assert!(playlist.contains("#EXT-X-VERSION:7"));
        assert!(playlist.contains("#EXT-X-TARGETDURATION:2"));
        assert!(playlist.contains("segment00000.m4s"));
        assert!(playlist.contains("init.mp4"));
        // Should not contain discontinuity tag
        assert!(!playlist.contains("#EXT-X-DISCONTINUITY"));

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_audio_before_video_is_noop() {
        let test_dir = "/tmp/hls_test_audio_first";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut state = HlsStreamState::new(test_dir, "testkey", 4);

        // Audio before video should not create a segment (no current_file)
        let aac = vec![0x01, 0x02, 0x03];
        state.write_audio(&aac, 0).await.unwrap();

        assert!(!state.has_video);
        assert!(state.current_file.is_none());
        assert!(!state.segment_path(0).exists());

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_segment_rotation() {
        let test_dir = "/tmp/hls_test_rotation";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut state = HlsStreamState::new(test_dir, "testkey", 2); // 2s segment duration

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state.set_video_config(&avcc_config, fmp4::VideoCodec::H264).await.unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        // First frame at ts=0
        state.write_video(&nal, 0, true).await.unwrap();
        assert_eq!(state.segment_index, 0);

        // Frame at ts=2500 (> 2s duration) triggers finalize + new segment
        state.write_video(&nal, 2500, true).await.unwrap();
        assert_eq!(state.segment_index, 1);
        assert!(state.segment_path(0).exists());
        assert!(state.segment_path(1).exists());

        let playlist = tokio::fs::read_to_string(state.playlist_path()).await.unwrap();
        // Only finalized segment 0 should be in playlist
        assert!(playlist.contains("segment00000.m4s"));
        // Segment 1 is not yet finalized, should not appear
        assert!(!playlist.contains("segment00001.m4s"));

        let _ = std::fs::remove_dir_all(test_dir);
    }
}
