use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

pub async fn get_or_generate_thumbnail(
    stream_key: &str,
    media_dir: &str,
    width: Option<u32>,
    ttl_seconds: u64,
) -> anyhow::Result<PathBuf> {
    let dir = PathBuf::from(media_dir).join("thumbnails");
    tokio::fs::create_dir_all(&dir).await?;

    let suffix = width.map(|w| format!("_w{}", w)).unwrap_or_default();
    let thumb_path = dir.join(format!("{}{}.jpg", stream_key, suffix));

    // Check if existing thumbnail is fresh enough
    if let Ok(meta) = tokio::fs::metadata(&thumb_path).await {
        if let Ok(modified) = meta.modified() {
            if modified.elapsed().unwrap_or(Duration::MAX) < Duration::from_secs(ttl_seconds) {
                return Ok(thumb_path);
            }
        }
    }

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

    let result = run_ffmpeg_thumbnail(&tmp_path, &thumb_path, width).await;

    // Clean up temp file
    let _ = tokio::fs::remove_file(&tmp_path).await;

    result
}

pub async fn generate_from_file(
    video_path: &std::path::Path,
    output_path: &std::path::Path,
    width: Option<u32>,
) -> anyhow::Result<PathBuf> {
    run_ffmpeg_thumbnail(video_path, output_path, width).await
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
        "-q:v".to_string(), "2".to_string(),
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
