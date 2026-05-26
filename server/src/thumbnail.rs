use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

pub async fn generate_thumbnails_for_stream(
    media_dir: &str,
    stream_key: &str,
    sizes: &[u32],
    interval_seconds: u32,
) -> anyhow::Result<Vec<PathBuf>> {
    let dir = PathBuf::from(media_dir).join("thumbnails");
    tokio::fs::create_dir_all(&dir).await?;

    // Build a temporary MP4 from init + first segment so ffmpeg can decode it
    let hls_dir = PathBuf::from(media_dir).join("hls").join(stream_key);
    let init_path = hls_dir.join("init.mp4");
    let seg0_path = hls_dir.join("segment00000.m4s");

    if !tokio::fs::try_exists(&init_path).await.unwrap_or(false) {
        return Err(anyhow::anyhow!("init.mp4 not ready"));
    }
    if !tokio::fs::try_exists(&seg0_path).await.unwrap_or(false) {
        return Err(anyhow::anyhow!("first segment not ready"));
    }

    let init = tokio::fs::read(&init_path).await?;
    let seg0 = tokio::fs::read(&seg0_path).await?;

    let tmp_path = dir.join(format!("{}_tmp.mp4", stream_key));
    let mut tmp_data = init;
    tmp_data.extend_from_slice(&seg0);
    tokio::fs::write(&tmp_path, &tmp_data).await?;

    let mut results = Vec::new();
    for &width in sizes {
        let thumb_path = dir.join(format!("{}_w{}.webp", stream_key, width));

        // Check if existing thumbnail is fresh enough
        let should_generate = if let Ok(meta) = tokio::fs::metadata(&thumb_path).await {
            if let Ok(modified) = meta.modified() {
                modified.elapsed().unwrap_or(Duration::MAX) >= Duration::from_secs(interval_seconds as u64)
            } else {
                true
            }
        } else {
            true
        };

        if should_generate {
            if let Err(e) = run_ffmpeg_thumbnail(&tmp_path, &thumb_path, Some(width)).await {
                tracing::warn!("Thumbnail generation failed for {} w={}: {}", stream_key, width, e);
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
    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(), "error".to_string(),
        "-i".to_string(), input.to_str().unwrap().to_string(),
        "-ss".to_string(), "00:00:00.5".to_string(),
        "-vframes".to_string(), "1".to_string(),
        "-quality".to_string(), "75".to_string(),
        "-compression_level".to_string(), "4".to_string(),
    ];

    if let Some(w) = width {
        args.push("-vf".to_string());
        args.push(format!("scale={}:-1", w));
    }

    args.push(output.to_str().unwrap().to_string());

    let cmd_output = Command::new("ffmpeg")
        .args(&args)
        .output()
        .await?;

    if !cmd_output.status.success() {
        let stderr = String::from_utf8_lossy(&cmd_output.stderr);
        return Err(anyhow::anyhow!("ffmpeg failed: {}", stderr));
    }

    Ok(output.to_path_buf())
}
