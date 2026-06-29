use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

const DISK_WRITER_CHANNEL_BOUND: usize = 10_000;

/// Error returned when the DiskWriter channel is closed (worker task exited).
#[derive(Debug, Clone)]
pub struct DiskWriterError {
    reason: String,
}

impl std::fmt::Display for DiskWriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DiskWriter channel closed: {}", self.reason)
    }
}

impl std::error::Error for DiskWriterError {}

pub enum DiskCommand {
    CreateDirAll {
        path: PathBuf,
    },
    WriteAndRename {
        tmp_path: PathBuf,
        final_path: PathBuf,
        data: Vec<u8>,
    },
    WriteSegment {
        tmp_path: PathBuf,
        final_path: PathBuf,
        data: Vec<u8>,
    },
    WritePlaylist {
        lock_path: PathBuf,
        target_path: PathBuf,
        content: String,
    },
    RemoveFile {
        path: PathBuf,
    },
    RemoveDirAll {
        path: PathBuf,
    },
    WriteRecordingData {
        stream_key: String,
        data: Vec<u8>,
    },
    CreateRecording {
        stream_key: String,
        path: PathBuf,
        init_data: Vec<u8>,
    },
    CloseRecording {
        stream_key: String,
    },
    Flush {
        reply: tokio::sync::oneshot::Sender<()>,
    },
}

pub struct DiskWriter {
    tx: mpsc::Sender<DiskCommand>,
}

impl Clone for DiskWriter {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl Default for DiskWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskWriter {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(DISK_WRITER_CHANNEL_BOUND);
        tokio::spawn(Self::run(rx));
        Self { tx }
    }

    /// Send a command to the DiskWriter, awaiting channel capacity.
    ///
    /// Returns an error only if the DiskWriter worker task has exited
    /// (channel closed). Under normal operation this provides backpressure
    /// rather than silently dropping writes.
    pub async fn send_cmd(&self, cmd: DiskCommand) -> Result<(), DiskWriterError> {
        self.tx.send(cmd).await.map_err(|e| DiskWriterError {
            reason: e.to_string(),
        })
    }

    /// Non-blocking send for non-critical cleanup commands.
    ///
    /// Drops the command if the channel is full or closed. Use only for
    /// best-effort operations like removing stale files where failure is
    /// acceptable.
    pub fn try_send_cmd(&self, cmd: DiskCommand) {
        if let Err(e) = self.tx.try_send(cmd) {
            tracing::debug!(
                "DiskWriter: try_send_cmd skipped (channel full or closed): {}",
                e
            );
        }
    }

    pub fn sender(&self) -> mpsc::Sender<DiskCommand> {
        self.tx.clone()
    }

    pub async fn flush(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.tx.send(DiskCommand::Flush { reply: tx }).await;
        let _ = rx.await;
    }

    async fn fsync_file(file: &tokio::fs::File, label: &str) {
        if let Err(e) = file.sync_data().await {
            tracing::warn!("DiskWriter: fsync {} failed: {}", label, e);
        }
    }

    async fn write_and_fsync(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
        let mut file = tokio::fs::File::create(path).await?;
        file.write_all(data).await?;
        Self::fsync_file(&file, &path.display().to_string()).await;
        Ok(())
    }

    async fn run(mut rx: mpsc::Receiver<DiskCommand>) {
        let mut open_recordings: HashMap<String, tokio::fs::File> = HashMap::new();

        while let Some(cmd) = rx.recv().await {
            match cmd {
                DiskCommand::CreateDirAll { path } => {
                    if let Err(e) = tokio::fs::create_dir_all(&path).await {
                        tracing::error!("DiskWriter: create_dir_all {:?} failed: {}", path, e);
                    }
                }
                DiskCommand::WriteAndRename {
                    tmp_path,
                    final_path,
                    data,
                } => {
                    if let Err(e) = Self::write_and_fsync(&tmp_path, &data).await {
                        tracing::error!("DiskWriter: write {:?} failed: {}", tmp_path, e);
                        continue;
                    }
                    if let Err(e) = tokio::fs::rename(&tmp_path, &final_path).await {
                        tracing::error!(
                            "DiskWriter: rename {:?} -> {:?} failed: {}",
                            tmp_path,
                            final_path,
                            e
                        );
                    }
                }
                DiskCommand::WriteSegment {
                    tmp_path,
                    final_path,
                    data,
                } => {
                    if let Err(e) = Self::write_and_fsync(&tmp_path, &data).await {
                        tracing::error!("DiskWriter: write segment {:?} failed: {}", tmp_path, e);
                        continue;
                    }
                    if let Err(e) = tokio::fs::rename(&tmp_path, &final_path).await {
                        tracing::error!(
                            "DiskWriter: rename segment {:?} -> {:?} failed: {}",
                            tmp_path,
                            final_path,
                            e
                        );
                    }
                }
                DiskCommand::WritePlaylist {
                    lock_path,
                    target_path,
                    content,
                } => {
                    if let Err(e) = Self::write_and_fsync(&lock_path, content.as_bytes()).await {
                        tracing::error!("DiskWriter: write playlist {:?} failed: {}", lock_path, e);
                        continue;
                    }
                    if let Err(e) = tokio::fs::rename(&lock_path, &target_path).await {
                        tracing::error!(
                            "DiskWriter: rename playlist {:?} -> {:?} failed: {}",
                            lock_path,
                            target_path,
                            e
                        );
                    }
                }
                DiskCommand::RemoveFile { path } => {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                DiskCommand::RemoveDirAll { path } => {
                    let _ = tokio::fs::remove_dir_all(&path).await;
                }
                DiskCommand::CreateRecording {
                    stream_key,
                    path,
                    init_data,
                } => match tokio::fs::File::create(&path).await {
                    Ok(mut file) => {
                        if let Err(e) = file.write_all(&init_data).await {
                            tracing::error!(
                                "DiskWriter: write init for recording {} failed: {}",
                                stream_key,
                                e
                            );
                        } else {
                            open_recordings.insert(stream_key, file);
                        }
                    }
                    Err(e) => {
                        tracing::error!("DiskWriter: create recording {:?} failed: {}", path, e);
                    }
                },
                DiskCommand::WriteRecordingData { stream_key, data } => {
                    if let Some(ref mut file) = open_recordings.get_mut(&stream_key) {
                        if let Err(e) = file.write_all(&data).await {
                            tracing::error!(
                                "DiskWriter: write recording data for {} failed: {}",
                                stream_key,
                                e
                            );
                        }
                    } else {
                        tracing::warn!(
                            "DiskWriter: WriteRecordingData for {} but no open recording",
                            stream_key
                        );
                    }
                }
                DiskCommand::CloseRecording { stream_key } => {
                    if let Some(file) = open_recordings.remove(&stream_key) {
                        if let Err(e) = file.sync_all().await {
                            tracing::error!(
                                "DiskWriter: sync recording {} failed: {}",
                                stream_key,
                                e
                            );
                        }
                        drop(file);
                        tracing::info!("DiskWriter: closed recording for {}", stream_key);
                    }
                }
                DiskCommand::Flush { reply } => {
                    let _ = reply.send(());
                }
            }
        }

        // Channel closed: flush and close all open recording files
        for (key, file) in open_recordings {
            let _ = file.sync_all().await;
            drop(file);
            tracing::info!("DiskWriter: closed recording for {} (shutdown)", key);
        }
    }
}
