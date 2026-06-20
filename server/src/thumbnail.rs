use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

async fn find_latest_segment(dir: &PathBuf) -> anyhow::Result<PathBuf> {
    let mut latest: Option<(u32, PathBuf)> = None;
    let mut rd = tokio::fs::read_dir(dir).await?;
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("segment") && name_str.ends_with(".m4s") {
            let idx_str = &name_str[7..name_str.len() - 4];
            if let Ok(idx) = idx_str.parse::<u32>() {
                let meta = entry.metadata().await?;
                if meta.len() > 0 && (latest.is_none() || idx > latest.as_ref().unwrap().0) {
                    latest = Some((idx, entry.path()));
                }
            }
        }
    }
    latest
        .map(|(_, p)| p)
        .ok_or_else(|| anyhow::anyhow!("no finalized segments found"))
}

pub async fn generate_thumbnails_for_stream(
    media_dir: &str,
    stream_key: &str,
    sizes: &[u32],
    interval_seconds: u32,
) -> anyhow::Result<Vec<PathBuf>> {
    let dir = PathBuf::from(media_dir).join("thumbnails").join("streams");
    tokio::fs::create_dir_all(&dir).await?;

    // Build a temporary MP4 from init + latest finalized segment so ffmpeg can decode it
    let hls_dir = PathBuf::from(media_dir).join("hls").join(stream_key);
    let init_path = hls_dir.join("init.mp4");

    if !tokio::fs::try_exists(&init_path).await.unwrap_or(false) {
        return Err(anyhow::anyhow!("init.mp4 not ready"));
    }

    let seg_path = match find_latest_segment(&hls_dir).await {
        Ok(p) => p,
        Err(e) => return Err(anyhow::anyhow!("no valid segment for thumbnail: {}", e)),
    };

    let init = tokio::fs::read(&init_path).await?;
    let seg = tokio::fs::read(&seg_path).await?;

    let tmp_path = dir.join(format!("{}_tmp.mp4", stream_key));
    let mut tmp_data = Vec::with_capacity(init.len() + seg.len());
    tmp_data.extend_from_slice(&init);
    tmp_data.extend_from_slice(&seg);
    tokio::fs::write(&tmp_path, &tmp_data).await?;

    let mut results = Vec::new();
    for &width in sizes {
        let thumb_path = dir.join(format!("{}_w{}.webp", stream_key, width));

        // Remove stale 0-byte thumbnail files before deciding whether to regenerate
        if let Ok(meta) = tokio::fs::metadata(&thumb_path).await
            && meta.len() == 0
        {
            let _ = tokio::fs::remove_file(&thumb_path).await;
        }

        // Check if existing thumbnail is fresh enough
        let should_generate = if let Ok(meta) = tokio::fs::metadata(&thumb_path).await {
            if let Ok(modified) = meta.modified() {
                modified.elapsed().unwrap_or(Duration::MAX)
                    >= Duration::from_secs(interval_seconds as u64)
            } else {
                true
            }
        } else {
            true
        };

        if should_generate {
            if let Err(e) = run_ffmpeg_thumbnail(&tmp_path, &thumb_path, Some(width)).await {
                tracing::warn!(
                    "Thumbnail generation failed for {} w={}: {}",
                    stream_key,
                    width,
                    e
                );
            } else {
                results.push(thumb_path);
            }
        } else {
            results.push(thumb_path);
        }
    }

    // Clean up temp file
    let _ = tokio::fs::remove_file(&tmp_path).await;

    Ok(results)
}

pub async fn generate_thumbnails_for_file(
    video_path: &std::path::Path,
    output_dir: &std::path::Path,
    sizes: &[u32],
) -> anyhow::Result<Vec<PathBuf>> {
    tokio::fs::create_dir_all(output_dir).await?;

    let filename = video_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid video path"))?;

    let mut results = Vec::new();
    for &width in sizes {
        let thumb_path = output_dir.join(format!("{}_w{}.webp", filename, width));
        run_ffmpeg_thumbnail(video_path, &thumb_path, Some(width)).await?;
        results.push(thumb_path);
    }

    Ok(results)
}

async fn run_ffmpeg_thumbnail(
    input: &std::path::Path,
    output: &std::path::Path,
    width: Option<u32>,
) -> anyhow::Result<PathBuf> {
    // Atomic write: write to a temp file, then rename to final path.
    // This ensures nginx never serves a partially-written thumbnail.
    let tmp_output = output.with_extension("webp.tmp");

    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-ss".to_string(),
        "00:00:00.5".to_string(),
        "-i".to_string(),
        input.to_str().unwrap().to_string(),
        "-vframes".to_string(),
        "1".to_string(),
        "-q:v".to_string(),
        "75".to_string(),
    ];

    if let Some(w) = width {
        args.push("-vf".to_string());
        args.push(format!("scale={}:-1", w));
    }

    args.push("-f".to_string());
    args.push("webp".to_string());
    args.push(tmp_output.to_str().unwrap().to_string());

    let cmd_output = Command::new("ffmpeg").args(&args).output().await?;

    if !cmd_output.status.success() {
        let stderr = String::from_utf8_lossy(&cmd_output.stderr);
        let _ = tokio::fs::remove_file(&tmp_output).await;
        return Err(anyhow::anyhow!("ffmpeg failed: {}", stderr));
    }

    let meta = tokio::fs::metadata(&tmp_output).await?;
    if meta.len() == 0 {
        let _ = tokio::fs::remove_file(&tmp_output).await;
        return Err(anyhow::anyhow!("ffmpeg produced empty output"));
    }

    // Atomic rename: readers see either the old file or the complete new file
    tokio::fs::rename(&tmp_output, output).await?;

    Ok(output.to_path_buf())
}
