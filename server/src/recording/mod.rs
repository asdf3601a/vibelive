use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

fn hash_bytes(data: &[u8]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// Scan an fMP4 segment for tfdt box and rewrite base_media_decode_time.
/// Returns the new base_media_decode_time if found.
fn rewrite_segment_tfdt(segment: &mut [u8], offset: u64) -> Option<u64> {
    // Find moof box
    let moof_pos = find_box(segment, b"moof")?;
    let moof_size = read_u32(&segment[moof_pos..moof_pos + 4]) as usize;
    let moof_end = moof_pos + moof_size;
    if moof_end > segment.len() {
        return None;
    }

    // Find traf inside moof
    let traf_pos = find_box_in_range(&segment[moof_pos + 8..moof_end], b"traf")?;
    let traf_pos = moof_pos + 8 + traf_pos;
    let traf_size = read_u32(&segment[traf_pos..traf_pos + 4]) as usize;
    let traf_end = traf_pos + traf_size;
    if traf_end > segment.len() {
        return None;
    }

    // Find tfdt inside traf
    let tfdt_pos = find_box_in_range(&segment[traf_pos + 8..traf_end], b"tfdt")?;
    let tfdt_pos = traf_pos + 8 + tfdt_pos;
    let tfdt_size = read_u32(&segment[tfdt_pos..tfdt_pos + 4]) as usize;
    if tfdt_pos + tfdt_size > segment.len() || tfdt_size < 20 {
        return None;
    }

    // tfdt version at offset 8
    let version = segment[tfdt_pos + 8];
    let data_start = tfdt_pos + 12; // after size(4) + type(4) + version/flags(4)

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

fn find_box(data: &[u8], box_type: &[u8; 4]) -> Option<usize> {
    find_box_in_range(data, box_type)
}

fn find_box_in_range(data: &[u8], box_type: &[u8; 4]) -> Option<usize> {
    let mut offset = 0;
    while offset + 8 <= data.len() {
        let size = read_u32(&data[offset..offset + 4]) as usize;
        if size == 0 {
            // Box extends to end of parent
            break;
        }
        if size == 1 {
            // Extended size (64-bit) - skip for simplicity
            offset += 16;
            continue;
        }
        if &data[offset + 4..offset + 8] == box_type {
            return Some(offset);
        }
        offset += size;
    }
    None
}

fn read_u32(data: &[u8]) -> u32 {
    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
}

fn write_u32(data: &mut [u8], value: u32) {
    data[..4].copy_from_slice(&value.to_be_bytes());
}

fn read_u64(data: &[u8]) -> u64 {
    u64::from_be_bytes([
        data[0], data[1], data[2], data[3],
        data[4], data[5], data[6], data[7],
    ])
}

fn write_u64(data: &mut [u8], value: u64) {
    data[..8].copy_from_slice(&value.to_be_bytes());
}

pub struct Fmp4Recorder {
    dir: PathBuf,
    stream_key: String,
    file: Option<tokio::fs::File>,
    closed: bool,
    saved_path: Option<PathBuf>,
    last_init_hash: Option<u64>,
    recording_time_offset: u64,
    last_tfdt: Option<u64>,
}

impl Fmp4Recorder {
    pub fn new(media_dir: &str, stream_key: &str) -> Self {
        let dir = PathBuf::from(media_dir).join("recordings");
        Self {
            dir,
            stream_key: stream_key.to_string(),
            file: None,
            closed: false,
            saved_path: None,
            last_init_hash: None,
            recording_time_offset: 0,
            last_tfdt: None,
        }
    }

    pub async fn write_init(&mut self, init: &[u8]) -> std::io::Result<()> {
        let new_hash = hash_bytes(init);
        if let Some(last_hash) = self.last_init_hash {
            if last_hash != new_hash && self.file.is_some() {
                // Init changed while file is open: close current and start new recording
                let _ = self.close().await;
                self.closed = false;
            }
        }

        if self.file.is_none() {
            let path = self.dir.join(format!(
                "{}_{}.mp4",
                self.stream_key,
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            ));
            let mut file = tokio::fs::File::create(&path).await?;
            file.write_all(init).await?;
            file.flush().await?;
            file.sync_all().await?;
            self.saved_path = Some(path);
            self.file = Some(file);
            self.last_tfdt = None;
        }
        self.last_init_hash = Some(new_hash);
        Ok(())
    }

    pub async fn write_segment(&mut self, segment: &[u8]) -> std::io::Result<()> {
        if let Some(ref mut file) = self.file {
            let mut segment = segment.to_vec();

            if let Some(rewritten_tfdt) = rewrite_segment_tfdt(&mut segment, self.recording_time_offset) {
                if let Some(last_tfdt) = self.last_tfdt {
                    if rewritten_tfdt < last_tfdt {
                        // Gap or backward jump detected: bridge it
                        let gap = last_tfdt.saturating_sub(rewritten_tfdt) + 1;
                        self.recording_time_offset += gap;
                        // Re-rewrite with adjusted offset
                        if let Some(new_tfdt) = rewrite_segment_tfdt(&mut segment, self.recording_time_offset) {
                            self.last_tfdt = Some(new_tfdt);
                        }
                    } else {
                        self.last_tfdt = Some(rewritten_tfdt);
                    }
                } else {
                    self.last_tfdt = Some(rewritten_tfdt);
                }
            }

            file.write_all(&segment).await?;
            file.flush().await?;
            file.sync_all().await?;
        }
        Ok(())
    }

    pub async fn write_video(&mut self, _data: &[u8], _ts: u32) -> std::io::Result<()> {
        Ok(())
    }

    pub async fn write_audio(&mut self, _data: &[u8], _ts: u32) -> std::io::Result<()> {
        Ok(())
    }

    pub async fn close(&mut self) -> std::io::Result<PathBuf> {
        if self.closed {
            return self.saved_path.clone().ok_or_else(|| {
                std::io::Error::other("already closed with no path")
            });
        }
        self.closed = true;

        if let Some(mut file) = self.file.take() {
            file.flush().await?;
            file.sync_all().await?;
        }

        self.saved_path.clone().ok_or_else(|| {
            std::io::Error::other("no file was written")
        })
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

pub async fn write_index_json(
    media_dir: &str,
    recordings_base_url: &str,
    thumbnail_sizes: &[u32],
) -> anyhow::Result<()> {
    let recordings_dir = PathBuf::from(media_dir).join("recordings");
    let mut recordings = Vec::new();

    if let Ok(mut rd) = tokio::fs::read_dir(&recordings_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if !name.ends_with(".mp4") {
                continue;
            }

            let meta = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size = meta.len();
            let modified = match meta.modified() {
                Ok(t) => {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339()
                }
                Err(_) => chrono::Utc::now().to_rfc3339(),
            };

            let stream_key = parse_stream_key_from_filename(&name);

            let mut thumbnails = HashMap::new();
            for width in thumbnail_sizes {
                let thumb_filename = format!("{}_w{}.webp", name, width);
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
                filename: name.clone(),
                stream_key,
                created_at: modified,
                size_bytes: size,
                duration_seconds: None, // Could be populated with ffprobe if needed
                url: format!("{}/{}", recordings_base_url, name),
                thumbnails,
            });
        }
    }

    recordings.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let index = RecordingsIndex { recordings };
    let index_path = recordings_dir.join("index.json");
    let tmp_path = recordings_dir.join("index.json.tmp");
    let json = serde_json::to_string_pretty(&index)?;
    tokio::fs::write(&tmp_path, json).await?;
    tokio::fs::rename(&tmp_path, &index_path).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fmp4_recorder_create_and_close() {
        let test_dir = std::env::temp_dir().join("recording_fmp4_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings")).await.unwrap();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "teststream");

        recorder.write_init(&[0x66, 0x74, 0x79, 0x70]).await.unwrap();
        recorder.write_segment(&[0x6d, 0x6f, 0x6f, 0x66]).await.unwrap();
        let path = recorder.close().await.unwrap();
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
        tokio::fs::create_dir_all(&test_dir.join("recordings")).await.unwrap();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "multistream");

        recorder.write_init(&[0x01, 0x02]).await.unwrap();
        recorder.write_segment(&[0x03, 0x04]).await.unwrap();
        recorder.write_segment(&[0x05, 0x06]).await.unwrap();
        recorder.write_segment(&[0x07, 0x08]).await.unwrap();
        let path = recorder.close().await.unwrap();
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
        assert_eq!(contents, vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_write_index_json() {
        let test_dir = std::env::temp_dir().join("recording_index_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings")).await.unwrap();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "indexstream");

        recorder.write_init(&[0x66, 0x74, 0x79, 0x70]).await.unwrap();
        recorder.write_segment(&[0x6d, 0x6f, 0x6f, 0x66]).await.unwrap();
        let _path = recorder.close().await.unwrap();

        crate::recording::write_index_json(
            test_dir.to_str().unwrap(),
            "/recordings",
            &[320, 480],
        ).await.unwrap();

        let index_path = test_dir.join("recordings").join("index.json");
        assert!(tokio::fs::try_exists(&index_path).await.unwrap_or(false));

        let index_data = tokio::fs::read_to_string(&index_path).await.unwrap();
        let index: crate::recording::RecordingsIndex = serde_json::from_str(&index_data).unwrap();
        assert_eq!(index.recordings.len(), 1);
        assert_eq!(index.recordings[0].stream_key, "indexstream");

        let _ = tokio::fs::remove_dir_all(&test_dir).await;
    }

    #[tokio::test]
    async fn test_double_close() {
        let test_dir = std::env::temp_dir().join("recording_double_close_test");
        let _ = tokio::fs::remove_dir_all(&test_dir).await;
        tokio::fs::create_dir_all(&test_dir.join("recordings")).await.unwrap();
        let mut recorder = Fmp4Recorder::new(test_dir.to_str().unwrap(), "doublestream");

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
        assert_eq!(
            parse_stream_key_from_filename("invalid.mp4"),
            "invalid"
        );
    }
}
