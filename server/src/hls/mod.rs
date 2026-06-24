pub mod fmp4;

use std::borrow::Cow;
use std::path::PathBuf;
use tokio::io::{AsyncWriteExt, BufWriter};

pub struct HlsStreamState {
    stream_dir: PathBuf,
    _track_id: u32,
    is_audio_only: bool,
    segment_duration: u32,
    current_segment_start: u64,
    segment_index: u32,
    current_file: Option<BufWriter<tokio::fs::File>>,
    fmp4_muxer: fmp4::Fmp4Muxer,
    has_video: bool,
    has_audio: bool,
    first_ts: Option<u32>,
    init_written: bool,
    init_data: Option<Vec<u8>>,
    segment_data: Vec<Vec<u8>>,
    audio_codec: Option<fmp4::AudioCodec>,
    timestamp_offset: u64,
    last_video_pts: u64,
    last_audio_pts: u64,
    // Sliding window & RFC compliance
    hls_segments_keep: u32,
    first_segment_index: u32,
    discontinuity_sequence: u32,
    segment_durations: Vec<f64>,
    // Total across ALL segments (never drained), used for recording duration
    total_duration_secs: f64,
    segment_init_versions: Vec<u32>,
    discontinuity_before: Vec<bool>,
    // Keyframe-aligned rotation
    pending_rotation: bool,
    pending_rotation_pts: u64,
    // Versioned init files
    init_version: u32,
    last_init_hash: Option<u64>,
    // Wall-clock time mapping
    stream_started_at: Option<chrono::DateTime<chrono::Utc>>,
    segment_start_offsets: Vec<u64>,
    // RFC 6381 codec string cache
    codec_string: Option<String>,
    // Actual max segment duration observed (ms), used for EXT-X-TARGETDURATION
    max_segment_duration_ms: u64,
}

impl HlsStreamState {
    pub fn new(
        media_dir: &str,
        stream_key: &str,
        track_id: u32,
        is_audio_only: bool,
        segment_duration: u32,
        hls_segments_keep: u32,
    ) -> Self {
        let mut dir = PathBuf::from(media_dir).join("hls").join(stream_key);
        if track_id > 0 {
            dir = dir.join(format!("track_{}", track_id));
        }
        Self {
            stream_dir: dir,
            _track_id: track_id,
            is_audio_only,
            segment_duration,
            current_segment_start: 0,
            segment_index: 0,
            current_file: None,
            fmp4_muxer: fmp4::Fmp4Muxer::new(),
            has_video: false,
            has_audio: false,
            first_ts: None,
            init_written: false,
            init_data: None,
            segment_data: Vec::new(),
            audio_codec: None,
            timestamp_offset: 0,
            last_video_pts: 0,
            last_audio_pts: 0,
            hls_segments_keep,
            first_segment_index: 0,
            discontinuity_sequence: 0,
            segment_durations: Vec::new(),
            segment_init_versions: Vec::new(),
            discontinuity_before: Vec::new(),
            pending_rotation: false,
            pending_rotation_pts: 0,
            init_version: 0,
            last_init_hash: None,
            stream_started_at: None,
            segment_start_offsets: Vec::new(),
            codec_string: None,
            max_segment_duration_ms: 0,
            total_duration_secs: 0.0,
        }
    }

    pub fn stream_dir(&self) -> &PathBuf {
        &self.stream_dir
    }

    pub fn current_init_path(&self) -> PathBuf {
        if self.init_version == 0 {
            self.stream_dir.join("init.mp4")
        } else {
            self.stream_dir
                .join(format!("init_v{}.mp4", self.init_version))
        }
    }

    pub fn audio_codec(&self) -> Option<fmp4::AudioCodec> {
        self.audio_codec
    }

    pub fn total_duration_secs(&self) -> f64 {
        self.total_duration_secs
    }

    /// Clear the audio-only flag when video data arrives for this track.
    pub fn set_not_audio_only(&mut self) {
        self.is_audio_only = false;
    }

    fn segment_path(&self, index: u32) -> PathBuf {
        self.stream_dir.join(format!("segment{:05}.m4s", index))
    }

    pub fn playlist_path(&self) -> PathBuf {
        self.stream_dir.join("index.m3u8")
    }

    pub async fn set_video_config(
        &mut self,
        config: &[u8],
        codec: fmp4::VideoCodec,
        width: u16,
        height: u16,
    ) -> anyhow::Result<()> {
        self.fmp4_muxer.set_video_codec(codec, width, height);
        self.fmp4_muxer.set_video_config(config.to_vec());
        // For AV1, extract color info from the sequence header OBU
        if codec == fmp4::VideoCodec::AV1
            && let Some(color_cfg) = fmp4::codec::av1_color_config_from_config(config)
        {
            self.fmp4_muxer.set_video_color_config(color_cfg);
        }
        self.init_written = false;
        Ok(())
    }

    pub fn set_video_framerate(&mut self, num: u64, den: u64) {
        self.fmp4_muxer.set_video_framerate(num, den);
    }

    pub fn set_video_color_config(&mut self, cfg: fmp4::ColorConfig) {
        self.fmp4_muxer.set_video_color_config(cfg);
        self.init_written = false;
    }

    pub fn set_hdr_metadata(&mut self, hdr: fmp4::HdrMetadata) {
        self.fmp4_muxer.set_hdr_metadata(hdr);
        self.init_written = false;
    }

    pub async fn set_audio_config(
        &mut self,
        codec: fmp4::AudioCodec,
        config: &[u8],
    ) -> anyhow::Result<()> {
        self.audio_codec = Some(codec);
        self.fmp4_muxer.set_audio_codec(codec);
        self.fmp4_muxer.set_audio_config(config.to_vec());
        self.init_written = false;
        Ok(())
    }

    pub async fn update_video_resolution(&mut self, width: u16, height: u16) -> anyhow::Result<()> {
        self.fmp4_muxer.set_video_codec(
            self.fmp4_muxer
                .video_codec()
                .unwrap_or(fmp4::VideoCodec::H264),
            width,
            height,
        );
        // Resolution-only changes do NOT trigger init rewrite per plan
        Ok(())
    }

    pub async fn write_video(
        &mut self,
        data: &[u8],
        timestamp: u32,
        is_keyframe: bool,
        composition_time_offset: i32,
    ) -> anyhow::Result<()> {
        use std::borrow::Cow;
        if self.is_audio_only {
            return Ok(());
        }
        if self.first_ts.is_none() {
            self.first_ts = Some(timestamp);
        }
        let base_ts = self.first_ts.unwrap_or(0);
        let dts = (timestamp.saturating_sub(base_ts)) as u64 + self.timestamp_offset;

        if !self.has_video {
            if !is_keyframe {
                tracing::debug!(
                    "write_video: discarding non-keyframe before stream start, ts={}",
                    timestamp
                );
                return Ok(());
            }
            tracing::debug!(
                "write_video: first keyframe, ts={}, starting segment",
                timestamp
            );
            self.rotate_segment().await.map_err(|e| {
                tracing::error!(
                    "write_video: failed to create first segment, has_video=false, \
                         first_ts={:?}, segment_index={}, error={}",
                    self.first_ts,
                    self.segment_index,
                    e
                );
                e
            })?;
            self.has_video = true;
            self.stream_started_at = Some(chrono::Utc::now());
            self.current_segment_start = dts;
            tracing::debug!(
                "write_video: rotate_segment done, current_file={}",
                self.current_file.is_some()
            );
        }

        // Handle case where current_file was closed externally (e.g., explicit finalize_segment)
        if self.current_file.is_none() {
            if !is_keyframe {
                return Ok(());
            }
            self.segment_index += 1;
            self.current_segment_start = dts;
            self.rotate_segment().await.map_err(|e| {
                tracing::error!(
                    "write_video: failed to recover segment, has_video={}, \
                         segment_index={}, current_segment_start={}, error={}",
                    self.has_video,
                    self.segment_index,
                    self.current_segment_start,
                    e
                );
                e
            })?;
        }

        let elapsed_since_start = dts.saturating_sub(self.current_segment_start);
        let threshold = self.segment_duration as u64 * 1000;

        if elapsed_since_start >= threshold && self.current_file.is_some() && !self.pending_rotation
        {
            self.pending_rotation = true;
            self.pending_rotation_pts = dts;
        }

        if self.pending_rotation && is_keyframe {
            self.finalize_segment().await?;
            self.current_segment_start = dts;
            self.segment_index += 1;
            self.pending_rotation = false;
            self.rotate_segment().await?;
        }

        let sample_data: Cow<'_, [u8]> = match self.fmp4_muxer.video_codec() {
            Some(fmp4::VideoCodec::AV1) => Cow::Owned(fmp4::ensure_av1_obu_size_fields(data)),
            _ => Cow::Borrowed(data),
        };
        let pts = (dts as i64 + composition_time_offset as i64) as u64;
        self.fmp4_muxer
            .add_video_sample(sample_data, dts, pts, is_keyframe);
        self.last_video_pts = self.last_video_pts.max(pts);

        // Update codec string cache when available
        if self.codec_string.is_none() {
            self.codec_string = self.fmp4_muxer.codec_string();
        }

        Ok(())
    }

    pub async fn write_audio(&mut self, data: &[u8], timestamp: u32) -> anyhow::Result<()> {
        if self.first_ts.is_none() {
            self.first_ts = Some(timestamp);
        }
        if !self.has_audio {
            if let Some(codec) = self.audio_codec {
                self.fmp4_muxer.set_audio_codec(codec);
                self.has_audio = true;
            } else {
                return Ok(());
            }
        }
        if self.current_file.is_none() {
            if self.is_audio_only {
                let base_ts = self.first_ts.unwrap_or(0);
                let pts = (timestamp.saturating_sub(base_ts)) as u64 + self.timestamp_offset;
                self.rotate_segment().await.map_err(|e| {
                    tracing::error!(
                        "write_audio: failed to create first segment, has_audio={}, \
                             segment_index={}, error={}",
                        self.has_audio,
                        self.segment_index,
                        e
                    );
                    e
                })?;
                self.stream_started_at = Some(chrono::Utc::now());
                self.current_segment_start = pts;
            } else {
                return Ok(());
            }
        }
        let base_ts = self.first_ts.unwrap_or(0);
        let pts = (timestamp.saturating_sub(base_ts)) as u64 + self.timestamp_offset;
        self.fmp4_muxer.add_audio_sample(Cow::Borrowed(data), pts);
        self.last_audio_pts = pts;

        // Audio-only segment rotation (no keyframe concept)
        if self.is_audio_only {
            let elapsed_since_start = pts.saturating_sub(self.current_segment_start);
            let threshold = self.segment_duration as u64 * 1000;
            if elapsed_since_start >= threshold
                && self.current_file.is_some()
                && !self.pending_rotation
            {
                self.pending_rotation = true;
                self.pending_rotation_pts = pts;
            }
            if self.pending_rotation {
                self.finalize_segment().await?;
                self.current_segment_start = pts;
                self.segment_index += 1;
                self.pending_rotation = false;
                self.rotate_segment().await?;
            }
        }

        Ok(())
    }

    async fn write_init_segment(&mut self) -> anyhow::Result<()> {
        if self.init_written {
            // Verify the file actually exists on disk.
            // Stale cleanup from a previous stream instance may have deleted it
            // while we were between segment rotations.
            let path = self.current_init_path();
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return Ok(());
            }
            self.init_written = false;
            self.last_init_hash = None;
        }
        let init = self.fmp4_muxer.init_segment();
        let new_hash = crate::util::hash_bytes(&init);

        if self.last_init_hash == Some(new_hash) {
            self.init_written = true;
            return Ok(());
        }

        // Allow overwriting init.mp4 if no segment has been finalized yet.
        // This handles the common case where audio config arrives after video config
        // but before the first segment is closed, avoiding a mismatched init.
        let can_overwrite = self.last_init_hash.is_some()
            && self.init_version == 0
            && self.segment_init_versions.is_empty();

        if self.last_init_hash.is_some() && !can_overwrite {
            self.init_version += 1;
        }
        let path = self.current_init_path();

        // Atomic write: tmp + rename so clients never see a partially-written init
        let tmp_path = path.with_extension("mp4.tmp");
        tokio::fs::write(&tmp_path, &init).await?;
        tokio::fs::rename(&tmp_path, &path).await?;
        self.init_data = Some(init);
        self.init_written = true;
        self.last_init_hash = Some(new_hash);
        Ok(())
    }

    async fn rotate_segment(&mut self) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.stream_dir).await?;
        self.write_init_segment().await?;
        let path = self.segment_path(self.segment_index);
        let tmp_path = path.with_extension("m4s.tmp");
        tracing::debug!("rotate_segment: creating tmp file at {:?}", tmp_path);
        let file = tokio::fs::File::create(&tmp_path).await?;
        self.current_file = Some(BufWriter::with_capacity(64 * 1024, file));
        self.update_playlist().await?;
        tracing::debug!("rotate_segment: done, segment_index={}", self.segment_index);
        Ok(())
    }

    pub async fn finalize_segment(&mut self) -> anyhow::Result<()> {
        tracing::debug!(
            "finalize_segment: current_file={}",
            self.current_file.is_some()
        );
        if let Some(mut writer) = self.current_file.take() {
            // Capture last sample duration before flush clears samples
            let last_sample_duration = if self.last_video_pts >= self.last_audio_pts {
                self.fmp4_muxer.last_video_sample_duration()
            } else {
                self.fmp4_muxer.last_audio_sample_duration()
            };
            let has_fragment = if let Some(fragment) = self.fmp4_muxer.flush_combined_fragment() {
                tracing::debug!(
                    "finalize_segment: writing fragment of {} bytes",
                    fragment.len()
                );
                writer.write_all(&fragment).await?;
                self.segment_data.push(fragment);

                let last_pts = self.last_video_pts.max(self.last_audio_pts);
                let duration_ms =
                    (last_pts + last_sample_duration).saturating_sub(self.current_segment_start);
                self.max_segment_duration_ms = self.max_segment_duration_ms.max(duration_ms);
                let duration = duration_ms as f64 / 1000.0;
                self.segment_durations.push(duration.max(0.001));
                self.total_duration_secs += duration.max(0.001);
                self.segment_start_offsets.push(self.current_segment_start);
                let prev_init = self.segment_init_versions.last().copied();
                let is_discontinuity = prev_init.is_some() && prev_init != Some(self.init_version);
                self.discontinuity_before.push(is_discontinuity);
                self.segment_init_versions.push(self.init_version);
                true
            } else {
                false
            };
            writer.flush().await?;
            writer.get_ref().sync_all().await?;
            drop(writer);

            // Atomic rename: tmp -> final segment file
            let path = self.segment_path(self.segment_index);
            let tmp_path = path.with_extension("m4s.tmp");
            if has_fragment {
                let _ = tokio::fs::rename(&tmp_path, &path).await;
            } else {
                // No data written: remove the empty temp file
                let _ = tokio::fs::remove_file(&tmp_path).await;
            }
        }
        self.update_playlist().await?;
        Ok(())
    }

    /// Finalize the current segment and prepare a fresh one for potential reconnect.
    pub async fn prepare_for_grace_period(&mut self) -> anyhow::Result<()> {
        self.finalize_segment().await?;
        self.timestamp_offset = self
            .last_video_pts
            .max(self.last_audio_pts)
            .saturating_add(33);
        self.first_ts = None;
        self.segment_index += 1;
        self.rotate_segment().await?;
        self.current_segment_start = self.timestamp_offset;
        Ok(())
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.finalize_segment().await?;
        self.write_playlist(true).await?;
        Ok(())
    }

    pub fn drain_init_data(&mut self) -> Option<Vec<u8>> {
        self.init_data.take()
    }

    pub fn drain_segment_data(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.segment_data)
    }

    async fn update_playlist(&mut self) -> anyhow::Result<()> {
        self.write_playlist(false).await
    }

    async fn write_playlist(&mut self, include_endlist: bool) -> anyhow::Result<()> {
        let finalized_count = self.segment_durations.len();
        let effective_keep = self.hls_segments_keep.max(3) as usize;

        let mut to_remove = 0usize;
        if finalized_count > effective_keep {
            to_remove = finalized_count - effective_keep;
        }
        if finalized_count >= 3 && (finalized_count - to_remove) < 3 {
            to_remove = finalized_count - 3;
        }

        for i in 0..to_remove {
            if self.discontinuity_before[i] {
                self.discontinuity_sequence += 1;
            }
            let seg_idx = self.first_segment_index + i as u32;
            let path = self.segment_path(seg_idx);
            let _ = tokio::fs::remove_file(&path).await;
        }

        if to_remove > 0 {
            self.segment_durations.drain(0..to_remove);
            self.segment_init_versions.drain(0..to_remove);
            self.discontinuity_before.drain(0..to_remove);
            self.segment_start_offsets.drain(0..to_remove);
            self.first_segment_index += to_remove as u32;
        }

        let mut playlist = String::new();
        playlist.push_str("#EXTM3U\n");
        playlist.push_str("#EXT-X-VERSION:7\n");
        if let Some(ref codecs) = self.codec_string {
            playlist.push_str(&format!("#EXT-X-CODECS:{}\n", codecs));
        }
        let target_duration = self
            .segment_duration
            .max(self.max_segment_duration_ms.div_ceil(1000) as u32);
        playlist.push_str(&format!("#EXT-X-TARGETDURATION:{}\n", target_duration));
        playlist.push_str(&format!(
            "#EXT-X-MEDIA-SEQUENCE:{}\n",
            self.first_segment_index
        ));
        playlist.push_str(&format!(
            "#EXT-X-DISCONTINUITY-SEQUENCE:{}\n",
            self.discontinuity_sequence
        ));

        if !self.segment_durations.is_empty() {
            if !self.discontinuity_before[0] {
                let init_name = if self.segment_init_versions[0] == 0 {
                    "init.mp4".to_string()
                } else {
                    format!("init_v{}.mp4", self.segment_init_versions[0])
                };
                playlist.push_str(&format!("#EXT-X-MAP:URI=\"{}\"\n", init_name));
            }
        } else {
            let init_name = if self.init_version == 0 {
                "init.mp4".to_string()
            } else {
                format!("init_v{}.mp4", self.init_version)
            };
            playlist.push_str(&format!("#EXT-X-MAP:URI=\"{}\"\n", init_name));
        }

        for i in 0..self.segment_durations.len() {
            let abs_seg_idx = self.first_segment_index + i as u32;
            if self.discontinuity_before[i] {
                playlist.push_str("#EXT-X-DISCONTINUITY\n");
                let init_name = if self.segment_init_versions[i] == 0 {
                    "init.mp4".to_string()
                } else {
                    format!("init_v{}.mp4", self.segment_init_versions[i])
                };
                playlist.push_str(&format!("#EXT-X-MAP:URI=\"{}\"\n", init_name));
            }
            playlist.push_str(&format!("#EXTINF:{:.3},\n", self.segment_durations[i]));
            if let Some(started_at) = self.stream_started_at {
                let seg_time = started_at
                    + chrono::Duration::milliseconds(self.segment_start_offsets[i] as i64);
                playlist.push_str(&format!(
                    "#EXT-X-PROGRAM-DATE-TIME:{}\n",
                    seg_time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                ));
            }
            playlist.push_str(&format!("segment{:05}.m4s\n", abs_seg_idx));
        }

        if include_endlist {
            playlist.push_str("#EXT-X-ENDLIST\n");
        }

        let lock_path = self.stream_dir.join("index.m3u8.lock");
        let target_path = self.stream_dir.join("index.m3u8");
        tokio::fs::write(&lock_path, playlist).await?;
        tokio::fs::rename(&lock_path, &target_path).await?;
        Ok(())
    }
}

impl Drop for HlsStreamState {
    fn drop(&mut self) {
        let _ = self.current_file.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hls_stream_state_new() {
        let test_dir = std::env::temp_dir().join("hls_test_new");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 4, 10);
        assert_eq!(state.segment_index, 0);
        assert!(state.current_file.is_none());
        assert!(!state.has_video);
        assert!(!state.has_audio);
        assert!(
            tokio::fs::try_exists(&state.stream_dir)
                .await
                .unwrap_or(false)
        );
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_first_video_triggers_segment_creation() {
        let test_dir = std::env::temp_dir().join("hls_test_first_video");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 4, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();

        assert!(state.has_video);
        assert!(state.current_file.is_some());
        // Segment file is a temp file until finalize_segment renames it
        assert!(
            tokio::fs::try_exists(state.segment_path(0).with_extension("m4s.tmp"))
                .await
                .unwrap_or(false)
        );
        assert!(
            tokio::fs::try_exists(state.playlist_path())
                .await
                .unwrap_or(false)
        );

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_playlist_content() {
        let test_dir = std::env::temp_dir().join("hls_test_playlist");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.starts_with("#EXTM3U"));
        assert!(playlist.contains("#EXT-X-VERSION:7"));
        assert!(playlist.contains("#EXT-X-TARGETDURATION:2"));
        assert!(playlist.contains("segment00000.m4s"));
        assert!(playlist.contains("init.mp4"));
        assert!(!playlist.contains("#EXT-X-DISCONTINUITY\n"));
        assert!(playlist.contains("#EXT-X-PROGRAM-DATE-TIME:"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_audio_before_video_is_noop() {
        let test_dir = std::env::temp_dir().join("hls_test_audio_first");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 4, 10);

        let aac = vec![0x01, 0x02, 0x03];
        state.write_audio(&aac, 0).await.unwrap();

        assert!(!state.has_video);
        assert!(state.current_file.is_none());
        assert!(
            !tokio::fs::try_exists(state.segment_path(0))
                .await
                .unwrap_or(false)
        );

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_segment_rotation() {
        let test_dir = std::env::temp_dir().join("hls_test_rotation");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10); // 2s segment duration

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        // First frame at ts=0
        state.write_video(&nal, 0, true, 0).await.unwrap();
        assert_eq!(state.segment_index, 0);

        // Frame at ts=2500 (> 2s duration) triggers finalize + new segment
        state.write_video(&nal, 2500, true, 0).await.unwrap();
        assert_eq!(state.segment_index, 1);
        assert!(
            tokio::fs::try_exists(state.segment_path(0))
                .await
                .unwrap_or(false)
        );
        // New segment is still a temp file until it gets finalized
        assert!(
            tokio::fs::try_exists(state.segment_path(1).with_extension("m4s.tmp"))
                .await
                .unwrap_or(false)
        );

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.contains("segment00000.m4s"));
        assert!(!playlist.contains("segment00001.m4s"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_keyframe_aligned_rotation() {
        let test_dir = std::env::temp_dir().join("hls_test_keyframe_rotation");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal_key = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        let nal_nonkey = vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x88, 0x84, 0x00];

        state.write_video(&nal_key, 0, true, 0).await.unwrap();
        assert_eq!(state.segment_index, 0);

        // Time threshold exceeded but non-keyframe should NOT rotate yet
        state
            .write_video(&nal_nonkey, 2500, false, 0)
            .await
            .unwrap();
        assert_eq!(state.segment_index, 0);
        assert!(state.pending_rotation);

        // Keyframe should trigger rotation
        state.write_video(&nal_key, 2600, true, 0).await.unwrap();
        assert_eq!(state.segment_index, 1);

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_nonkeyframe_never_force_rotates() {
        let test_dir = std::env::temp_dir().join("hls_test_nonkeyframe_no_force");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal_key = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        let nal_nonkey = vec![0x00, 0x00, 0x00, 0x04, 0x41, 0x88, 0x84, 0x00];

        state.write_video(&nal_key, 0, true, 0).await.unwrap();
        assert_eq!(state.segment_index, 0);

        // Write many non-keyframes well beyond target duration + max_extra
        for i in 1..=10 {
            state
                .write_video(&nal_nonkey, i * 1000, false, 0)
                .await
                .unwrap();
        }

        // Segment should still not have rotated
        assert_eq!(state.segment_index, 0);
        assert!(state.pending_rotation);

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_sliding_window() {
        let test_dir = std::env::temp_dir().join("hls_test_sliding");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 3); // keep 3

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];

        // Create 5 segments
        for i in 0..5 {
            let ts = i * 2500;
            state.write_video(&nal, ts, true, 0).await.unwrap();
        }
        state.finalize_segment().await.unwrap();

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        // Should keep at most 3 finalized segments
        assert!(playlist.contains("segment00002.m4s"));
        assert!(playlist.contains("segment00003.m4s"));
        assert!(playlist.contains("segment00004.m4s"));
        assert!(!playlist.contains("segment00000.m4s"));
        assert!(!playlist.contains("segment00001.m4s"));

        // MEDIA-SEQUENCE should be 2
        assert!(playlist.contains("#EXT-X-MEDIA-SEQUENCE:2"));

        // Old segment files should be deleted
        assert!(
            !tokio::fs::try_exists(state.segment_path(0))
                .await
                .unwrap_or(false)
        );
        assert!(
            !tokio::fs::try_exists(state.segment_path(1))
                .await
                .unwrap_or(false)
        );
        assert!(
            tokio::fs::try_exists(state.segment_path(2))
                .await
                .unwrap_or(false)
        );

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_atomic_write() {
        let test_dir = std::env::temp_dir().join("hls_test_atomic");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        // .lock file should not exist after rename
        let lock_path = state.stream_dir.join("index.m3u8.lock");
        assert!(!tokio::fs::try_exists(&lock_path).await.unwrap_or(false));

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.starts_with("#EXTM3U"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_versioned_init() {
        let test_dir = std::env::temp_dir().join("hls_test_versioned_init");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config1 = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config1, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        // init.mp4 should exist
        assert!(
            tokio::fs::try_exists(state.stream_dir.join("init.mp4"))
                .await
                .unwrap_or(false)
        );

        // Change video config
        let avcc_config2 = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x01];
        state
            .set_video_config(&avcc_config2, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();
        state.write_video(&nal, 2500, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        // init_v1.mp4 should exist
        assert!(
            tokio::fs::try_exists(state.stream_dir.join("init_v1.mp4"))
                .await
                .unwrap_or(false)
        );

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.contains("init_v1.mp4"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_discontinuity_tag() {
        let test_dir = std::env::temp_dir().join("hls_test_discontinuity");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config1 = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config1, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        // Change video config to trigger discontinuity
        let avcc_config2 = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x01];
        state
            .set_video_config(&avcc_config2, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();
        state.write_video(&nal, 2500, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.contains("#EXT-X-DISCONTINUITY\n"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_grace_period() {
        let test_dir = std::env::temp_dir().join("hls_test_grace");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        let old_offset = state.timestamp_offset;
        state.prepare_for_grace_period().await.unwrap();
        assert!(state.timestamp_offset > old_offset);
        assert_eq!(state.segment_index, 1);
        assert!(state.current_file.is_some());

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_close_adds_endlist() {
        let test_dir = std::env::temp_dir().join("hls_test_close");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        state.close().await.unwrap();

        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.ends_with("#EXT-X-ENDLIST\n"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_audio_video_combined() {
        let test_dir = std::env::temp_dir().join("hls_test_av_combined");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("hls").join("testkey"))
            .await
            .unwrap();
        let mut state = HlsStreamState::new(test_dir.to_str().unwrap(), "testkey", 0, false, 2, 10);

        let avcc_config = vec![0x01, 0x42, 0xC0, 0x1E, 0xFF, 0xE1, 0x00, 0x00];
        state
            .set_video_config(&avcc_config, fmp4::VideoCodec::H264, 1920, 1080)
            .await
            .unwrap();
        let aac_config = vec![0x12, 0x10];
        state
            .set_audio_config(fmp4::AudioCodec::Aac, &aac_config)
            .await
            .unwrap();

        let nal = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
        state.write_video(&nal, 0, true, 0).await.unwrap();
        let aac = vec![0xAF, 0x01];
        state.write_audio(&aac, 0).await.unwrap();
        state.finalize_segment().await.unwrap();

        assert!(
            tokio::fs::try_exists(state.segment_path(0))
                .await
                .unwrap_or(false)
        );
        let playlist = tokio::fs::read_to_string(state.playlist_path())
            .await
            .unwrap();
        assert!(playlist.contains("segment00000.m4s"));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }
}
