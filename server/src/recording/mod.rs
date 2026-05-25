use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

pub struct Fmp4Recorder {
    dir: PathBuf,
    stream_key: String,
    init_data: Option<Vec<u8>>,
    segments: Vec<Vec<u8>>,
    closed: bool,
    saved_path: Option<PathBuf>,
}

impl Fmp4Recorder {
    pub fn new(media_dir: &str, stream_key: &str) -> Self {
        let dir = PathBuf::from(media_dir).join("recordings");
        std::fs::create_dir_all(&dir).ok();
        Self {
            dir,
            stream_key: stream_key.to_string(),
            init_data: None,
            segments: Vec::new(),
            closed: false,
            saved_path: None,
        }
    }

    pub async fn set_init(&mut self, init: Vec<u8>) {
        self.init_data = Some(init);
    }

    pub async fn write_segment(&mut self, segment: Vec<u8>) {
        if !segment.is_empty() {
            self.segments.push(segment);
        }
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
                std::io::Error::new(std::io::ErrorKind::Other, "already closed with no path")
            });
        }
        self.closed = true;

        let path = self.dir.join(format!(
            "{}_{}.mp4",
            self.stream_key,
            chrono::Utc::now().format("%Y%m%d_%H%M%S")
        ));
        let mut file = tokio::fs::File::create(&path).await?;

        if let Some(ref init) = self.init_data {
            file.write_all(init).await?;
        }
        for seg in &self.segments {
            file.write_all(seg).await?;
        }
        file.flush().await?;
        file.sync_all().await?;

        self.saved_path = Some(path.clone());
        Ok(path)
    }

    pub fn saved_path(&self) -> Option<&PathBuf> {
        self.saved_path.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fmp4_recorder_create_and_close() {
        let test_dir = "/tmp/recording_fmp4_test";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut recorder = Fmp4Recorder::new(test_dir, "teststream");

        recorder.set_init(vec![0x66, 0x74, 0x79, 0x70]).await;
        recorder.write_segment(vec![0x6d, 0x6f, 0x6f, 0x66]).await;
        let path = recorder.close().await.unwrap();
        assert!(path.exists());

        let recordings_dir = std::path::PathBuf::from(test_dir).join("recordings");
        let entries: Vec<_> = std::fs::read_dir(&recordings_dir).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("teststream_")
                    && e.file_name().to_string_lossy().ends_with(".mp4")
            })
            .collect();
        assert_eq!(entries.len(), 1);

        let contents = std::fs::read(entries[0].path()).unwrap();
        assert_eq!(&contents[0..4], &[0x66, 0x74, 0x79, 0x70]);
        assert_eq!(&contents[4..8], &[0x6d, 0x6f, 0x6f, 0x66]);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_fmp4_recorder_multiple_segments() {
        let test_dir = "/tmp/recording_fmp4_multi";
        let _ = std::fs::remove_dir_all(test_dir);
        let mut recorder = Fmp4Recorder::new(test_dir, "multistream");

        recorder.set_init(vec![0x01, 0x02]).await;
        recorder.write_segment(vec![0x03, 0x04]).await;
        recorder.write_segment(vec![0x05, 0x06]).await;
        recorder.write_segment(vec![0x07, 0x08]).await;
        let path = recorder.close().await.unwrap();
        assert!(path.exists());

        let recordings_dir = std::path::PathBuf::from(test_dir).join("recordings");
        let entries: Vec<_> = std::fs::read_dir(&recordings_dir).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with("multistream_")
                    && e.file_name().to_string_lossy().ends_with(".mp4")
            })
            .collect();
        assert_eq!(entries.len(), 1);

        let contents = std::fs::read(entries[0].path()).unwrap();
        assert_eq!(contents, vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);

        let _ = std::fs::remove_dir_all(test_dir);
    }
}
