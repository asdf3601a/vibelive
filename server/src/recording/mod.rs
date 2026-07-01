use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::Command;

use crate::disk_writer::{DiskCommand, DiskWriter};

/// Scan an fMP4 segment for tfdt box and rewrite base_media_decode_time.
/// Returns the new base_media_decode_time if found.
fn rewrite_segment_tfdt(segment: &mut [u8], offset: u64) -> Option<u64> {
    use crate::util::{find_box, find_box_in_range, read_u32, read_u64, write_u32, write_u64};
    let moof_pos = find_box(segment, b"moof")?;
    let moof_size = read_u32(&segment[moof_pos..moof_pos + 4]) as usize;
    let moof_end = moof_pos + moof_size;
    if moof_end > segment.len() {
        return None;
    }

    let traf_pos = find_box_in_range(&segment[moof_pos + 8..moof_end], b"traf")?;
    let traf_pos = moof_pos + 8 + traf_pos;
    let traf_size = read_u32(&segment[traf_pos..traf_pos + 4]) as usize;
    let traf_end = traf_pos + traf_size;
    if traf_end > segment.len() {
        return None;
    }

    let tfdt_pos = find_box_in_range(&segment[traf_pos + 8..traf_end], b"tfdt")?;
    let tfdt_pos = traf_pos + 8 + tfdt_pos;
    let tfdt_size = read_u32(&segment[tfdt_pos..tfdt_pos + 4]) as usize;
    if tfdt_pos + tfdt_size > segment.len() || tfdt_size < 20 {
        return None;
    }

    let version = segment[tfdt_pos + 8];
    let data_start = tfdt_pos + 12;

    if version == 1 && data_start + 8 <= segment.len() {
        let base_media_decode_time = read_u64(&segment[data_start..data_start + 8]);
        let new_time = base_media_decode_time + offset;
        write_u64(&mut segment[data_start..data_start + 8], new_time);
        Some(new_time)
    } else if version == 0 && data_start + 4 <= segment.len() {
        let base_media_decode_time = read_u32(&segment[data_start..data_start + 4]) as u64;
        let new_time = base_media_decode_time + offset;
        write_u32(&mut segment[data_start..data_start + 4], new_time as u32);
        Some(new_time)
    } else {
        None
    }
}

pub struct Fmp4Recorder {
    dir: PathBuf,
    stream_key: String,
    disk_writer: DiskWriter,
    closed: bool,
    saved_path: Option<PathBuf>,
    last_init_hash: Option<u64>,
    init_written: bool,
    recording_open: bool,
    recording_time_offset: u64,
    last_tfdt: Option<u64>,
}

impl Fmp4Recorder {
    pub fn new(media_dir: &str, stream_key: &str, disk_writer: DiskWriter) -> Self {
        let dir = PathBuf::from(media_dir).join("recordings");
        Self {
            dir,
            stream_key: stream_key.to_string(),
            disk_writer,
            closed: false,
            saved_path: None,
            last_init_hash: None,
            init_written: false,
            recording_open: false,
            recording_time_offset: 0,
            last_tfdt: None,
        }
    }

    pub async fn write_init(&mut self, init: &[u8]) -> std::io::Result<()> {
        let new_hash = crate::util::hash_bytes(init);
        if let Some(last_hash) = self.last_init_hash
            && last_hash != new_hash
            && self.recording_open
        {
            // Init changed while recording is open: close current and start new recording
            if let Err(e) = self.close().await {
                tracing::warn!(
                    "Recording: close on init change failed for {}: {}",
                    self.stream_key,
                    e
                );
            }
            self.closed = false;
        }

        if self.init_written && self.recording_open {
            // Init already written to this recording; refuse to write another
            return Ok(());
        }

        if !self.recording_open {
            // Ensure the recordings directory exists
            self.disk_writer
                .send_cmd(DiskCommand::CreateDirAll {
                    path: self.dir.clone(),
                })
                .await
                .map_err(|e| std::io::Error::other(format!("disk create dir: {}", e)))?;
            let path = self.dir.join(format!(
                "{}_{}.mp4",
                self.stream_key,
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            ));
            self.saved_path = Some(path.clone());
            self.recording_open = true;
            self.last_tfdt = None;

            self.disk_writer
                .send_cmd(DiskCommand::CreateRecording {
                    stream_key: self.stream_key.clone(),
                    path,
                    init_data: init.to_vec(),
                })
                .await
                .map_err(|e| std::io::Error::other(format!("disk create recording: {}", e)))?;
        }
        self.init_written = true;
        self.last_init_hash = Some(new_hash);
        Ok(())
    }

    pub async fn write_segment(&mut self, mut segment: Vec<u8>) -> std::io::Result<()> {
        if !self.recording_open {
            return Ok(());
        }
        if let Some(rewritten_tfdt) = rewrite_segment_tfdt(&mut segment, self.recording_time_offset)
        {
            if let Some(last_tfdt) = self.last_tfdt {
                if rewritten_tfdt < last_tfdt {
                    // Gap or backward jump detected: bridge it
                    let gap = last_tfdt.saturating_sub(rewritten_tfdt) + 1;
                    self.recording_time_offset += gap;
                    // Re-rewrite with adjusted offset
                    if let Some(new_tfdt) =
                        rewrite_segment_tfdt(&mut segment, self.recording_time_offset)
                    {
                        self.last_tfdt = Some(new_tfdt);
                    }
                } else {
                    self.last_tfdt = Some(rewritten_tfdt);
                }
            } else {
                self.last_tfdt = Some(rewritten_tfdt);
            }
        }

        self.disk_writer
            .send_cmd(DiskCommand::WriteRecordingData {
                stream_key: self.stream_key.clone(),
                data: segment,
            })
            .await
            .map_err(|e| std::io::Error::other(format!("disk write recording: {}", e)))?;
        Ok(())
    }

    pub async fn close(&mut self) -> std::io::Result<PathBuf> {
        if self.closed {
            return self
                .saved_path
                .clone()
                .ok_or_else(|| std::io::Error::other("already closed with no path"));
        }
        self.closed = true;
        self.last_init_hash = None;
        self.init_written = false;

        if self.recording_open {
            self.recording_open = false;
            self.disk_writer
                .send_cmd(DiskCommand::CloseRecording {
                    stream_key: self.stream_key.clone(),
                })
                .await
                .map_err(|e| std::io::Error::other(format!("disk close recording: {}", e)))?;
        }

        self.saved_path
            .clone()
            .ok_or_else(|| std::io::Error::other("no file was written"))
    }

    pub fn saved_path(&self) -> Option<&PathBuf> {
        self.saved_path.as_ref()
    }
}

/// Parses the stream key from a recording filename like `{key}_{YYYYMMDD}_{HHMMSS}.mp4`.
/// Handles stream keys that contain underscores.
pub fn parse_stream_key_from_filename(name: &str) -> String {
    let stem = name.strip_suffix(".mp4").unwrap_or(name);
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 3 {
        parts[..parts.len() - 2].join("_")
    } else {
        stem.to_string()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RecordingEntry {
    pub filename: String,
    pub stream_key: String,
    pub created_at: String,
    pub size_bytes: u64,
    pub duration_seconds: Option<u64>,
    pub url: String,
    pub thumbnails: HashMap<String, String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RecordingsIndex {
    pub recordings: Vec<RecordingEntry>,
}

/// Remux a fragmented MP4 into a regular MP4 with faststart.
/// The input file is replaced atomically (tmp + rename).
async fn remux_fmp4_to_mp4(path: &std::path::Path) -> anyhow::Result<()> {
    let tmp_path = path.with_extension("mp4.tmp");

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-strict",
            "-2",
            "-i",
            path.to_str().unwrap(),
            "-c",
            "copy",
            "-movflags",
            "+faststart+negative_cts_offsets",
            "-f",
            "mp4",
            tmp_path.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(anyhow::anyhow!("ffmpeg remux failed: {}", stderr));
    }

    let meta = tokio::fs::metadata(&tmp_path).await?;
    if meta.len() == 0 {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(anyhow::anyhow!("ffmpeg produced empty output"));
    }

    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

/// A concurrency-limited background queue for FFmpeg remux jobs.
/// Acquires a semaphore permit before running ffmpeg, limiting simultaneous
/// remux operations to the configured concurrency.
pub struct RemuxQueue {
    semaphore: tokio::sync::Semaphore,
    enabled: bool,
}

impl RemuxQueue {
    pub fn new(enabled: bool, concurrency: usize) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(concurrency.max(1)),
            enabled,
        }
    }

    /// Execute remux inline, acquiring the semaphore to respect concurrency.
    /// Waits until the remux completes. Returns Ok(()) if remux is disabled.
    /// Callers should proceed even on error — thumbnail generation still
    /// works without faststart, just slightly less efficient.
    pub async fn remux_now(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let _permit = self.semaphore.acquire().await;
        tracing::info!("Remuxing recording: {}", path.display());
        remux_fmp4_to_mp4(path).await
    }
}

pub async fn update_index_json(
    media_dir: &str,
    filename: &str,
    stream_key: &str,
    duration_seconds: Option<u64>,
    recordings_base_url: &str,
    thumbnail_sizes: &[u32],
) -> anyhow::Result<()> {
    let recordings_dir = PathBuf::from(media_dir).join("recordings");
    let index_path = recordings_dir.join("index.json");

    let mut recordings = Vec::new();
    if let Ok(index_data) = tokio::fs::read_to_string(&index_path).await
        && let Ok(index) = serde_json::from_str::<RecordingsIndex>(&index_data)
    {
        recordings = index
            .recordings
            .into_iter()
            .filter(|e| e.filename != filename)
            .collect();
    }

    let path = recordings_dir.join(filename);
    let meta = tokio::fs::metadata(&path).await?;
    let modified: chrono::DateTime<chrono::Utc> = meta.modified()?.into();

    let mut thumbnails = HashMap::new();
    for width in thumbnail_sizes {
        let thumb_filename = format!("{}_w{}.png", filename, width);
        let thumb_path = PathBuf::from(media_dir)
            .join("thumbnails")
            .join("recordings")
            .join(&thumb_filename);
        if tokio::fs::try_exists(&thumb_path).await.unwrap_or(false) {
            thumbnails.insert(
                width.to_string(),
                format!("/thumbnails/recordings/{}", thumb_filename),
            );
        }
    }

    recordings.push(RecordingEntry {
        filename: filename.to_string(),
        stream_key: stream_key.to_string(),
        created_at: modified.to_rfc3339(),
        size_bytes: meta.len(),
        duration_seconds,
        url: format!("{}/{}", recordings_base_url, filename),
        thumbnails,
    });

    recordings.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let index = RecordingsIndex { recordings };
    let tmp_path = recordings_dir.join("index.json.tmp");
    let json = serde_json::to_string_pretty(&index)?;
    tokio::fs::write(&tmp_path, json).await?;
    tokio::fs::rename(&tmp_path, &index_path).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_disk_writer() -> DiskWriter {
        DiskWriter::new()
    }

    async fn flush_and_wait(dw: &DiskWriter) {
        dw.flush().await;
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_fmp4_recorder_create_and_close() {
        let test_dir = std::env::temp_dir().join("recording_fmp4_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings"))
            .await
            .unwrap();
        let dw = test_disk_writer();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "teststream", dw.clone());

        recorder
            .write_init(&[0x66, 0x74, 0x79, 0x70])
            .await
            .unwrap();
        recorder
            .write_segment(vec![0x6d, 0x6f, 0x6f, 0x66])
            .await
            .unwrap();
        let path = recorder.close().await.unwrap();

        flush_and_wait(&dw).await;

        assert!(tokio::fs::try_exists(&path).await.unwrap_or(false));

        let recordings_dir = test_dir.join("recordings");
        let mut rd = tokio::fs::read_dir(&recordings_dir).await.unwrap();
        let mut entries = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("teststream_") && name.ends_with(".mp4") {
                entries.push(entry);
            }
        }
        assert_eq!(entries.len(), 1);

        let contents = tokio::fs::read(entries[0].path()).await.unwrap();
        assert_eq!(&contents[0..4], &[0x66, 0x74, 0x79, 0x70]);
        assert_eq!(&contents[4..8], &[0x6d, 0x6f, 0x6f, 0x66]);

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_fmp4_recorder_multiple_segments() {
        let test_dir = std::env::temp_dir().join("recording_fmp4_multi");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings"))
            .await
            .unwrap();
        let dw = test_disk_writer();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "multistream", dw.clone());

        recorder.write_init(&[0x01, 0x02]).await.unwrap();
        recorder.write_segment(vec![0x03, 0x04]).await.unwrap();
        recorder.write_segment(vec![0x05, 0x06]).await.unwrap();
        recorder.write_segment(vec![0x07, 0x08]).await.unwrap();
        let path = recorder.close().await.unwrap();

        flush_and_wait(&dw).await;

        assert!(tokio::fs::try_exists(&path).await.unwrap_or(false));

        let recordings_dir = test_dir.join("recordings");
        let mut rd = tokio::fs::read_dir(&recordings_dir).await.unwrap();
        let mut entries = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("multistream_") && name.ends_with(".mp4") {
                entries.push(entry);
            }
        }
        assert_eq!(entries.len(), 1);

        let contents = tokio::fs::read(entries[0].path()).await.unwrap();
        assert_eq!(
            contents,
            vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
        );

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_update_index_json() {
        let test_dir = std::env::temp_dir().join("recording_index_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings"))
            .await
            .unwrap();
        let dw = test_disk_writer();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "indexstream", dw.clone());

        recorder
            .write_init(&[0x66, 0x74, 0x79, 0x70])
            .await
            .unwrap();
        recorder
            .write_segment(vec![0x6d, 0x6f, 0x6f, 0x66])
            .await
            .unwrap();
        let path = recorder.close().await.unwrap();

        flush_and_wait(&dw).await;

        let filename = path.file_name().unwrap().to_str().unwrap().to_string();

        crate::recording::update_index_json(
            test_dir.to_str().unwrap(),
            &filename,
            "indexstream",
            Some(42),
            "/recordings",
            &[320, 480],
        )
        .await
        .unwrap();

        let index_path = test_dir.join("recordings").join("index.json");
        assert!(tokio::fs::try_exists(&index_path).await.unwrap_or(false));

        let index_data = tokio::fs::read_to_string(&index_path).await.unwrap();
        let index: crate::recording::RecordingsIndex = serde_json::from_str(&index_data).unwrap();
        assert_eq!(index.recordings.len(), 1);
        assert_eq!(index.recordings[0].stream_key, "indexstream");
        assert_eq!(index.recordings[0].duration_seconds, Some(42));

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_double_close() {
        let test_dir = std::env::temp_dir().join("recording_double_close_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings"))
            .await
            .unwrap();
        let dw = test_disk_writer();
        let mut recorder =
            Fmp4Recorder::new(test_dir.to_str().unwrap(), "doublestream", dw.clone());

        recorder.write_init(&[0x01, 0x02]).await.unwrap();
        let path1 = recorder.close().await.unwrap();
        let path2 = recorder.close().await.unwrap();
        assert_eq!(path1, path2);

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[test]
    fn test_parse_stream_key_from_filename() {
        assert_eq!(
            parse_stream_key_from_filename("mystream_20260525_143000.mp4"),
            "mystream"
        );
        assert_eq!(
            parse_stream_key_from_filename("my_stream_key_20260525_143000.mp4"),
            "my_stream_key"
        );
        assert_eq!(
            parse_stream_key_from_filename("key_20260525_143000.mp4"),
            "key"
        );
        assert_eq!(parse_stream_key_from_filename("invalid.mp4"), "invalid");
    }

    #[tokio::test]
    async fn test_no_duplicate_moov_on_reconnect() {
        let test_dir = std::env::temp_dir().join("recording_moov_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings"))
            .await
            .unwrap();
        let dw = test_disk_writer();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "moovstream", dw.clone());

        // Build a minimal valid init segment (ftyp + moov)
        let mut init_a = Vec::new();
        init_a.extend_from_slice(&(8u32).to_be_bytes());
        init_a.extend_from_slice(b"ftyp");
        init_a.extend_from_slice(b"iso5");
        init_a.extend_from_slice(&(8u32).to_be_bytes());
        init_a.extend_from_slice(b"moov");

        // Write init A + 2 segments
        recorder.write_init(&init_a).await.unwrap();
        recorder
            .write_segment(vec![
                0x00, 0x00, 0x00, 0x14, 0x6d, 0x6f, 0x6f, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ])
            .await
            .unwrap();
        recorder
            .write_segment(vec![
                0x00, 0x00, 0x00, 0x14, 0x6d, 0x6f, 0x6f, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ])
            .await
            .unwrap();

        // Simulate close + reconnect with changed init
        recorder.close().await.unwrap();

        flush_and_wait(&dw).await;

        // Build init B (different size/content to trigger hash change)
        let mut init_b = Vec::new();
        init_b.extend_from_slice(&(12u32).to_be_bytes());
        init_b.extend_from_slice(b"ftyp");
        init_b.extend_from_slice(b"mp42");
        init_b.extend_from_slice(b"mp41");
        init_b.extend_from_slice(&(8u32).to_be_bytes());
        init_b.extend_from_slice(b"moov");

        recorder.write_init(&init_b).await.unwrap();
        recorder
            .write_segment(vec![
                0x00, 0x00, 0x00, 0x14, 0x6d, 0x6f, 0x6f, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ])
            .await
            .unwrap();
        let path = recorder.close().await.unwrap();

        flush_and_wait(&dw).await;

        // Read the recorded file and count "moov" occurrences
        let contents = tokio::fs::read(&path).await.unwrap();
        let moov_count = contents.windows(4).filter(|w| w == b"moov").count();
        assert_eq!(
            moov_count, 1,
            "Expected exactly 1 moov atom, found {}",
            moov_count
        );

        // Also verify ftyp and moov appear once each
        let ftyp_count = contents.windows(4).filter(|w| w == b"ftyp").count();
        assert_eq!(
            ftyp_count, 1,
            "Expected exactly 1 ftyp atom, found {}",
            ftyp_count
        );

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }
}
