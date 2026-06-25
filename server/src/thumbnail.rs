use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;

const FFMPEG_TIMEOUT_SECS: u64 = 30;

/// Parameters for generating live-stream thumbnails. Grouping these into a
/// struct keeps call sites self-documenting and makes future additions
/// non-breaking instead of growing an 8-argument function signature.
pub struct StreamThumbnailRequest<'a> {
    pub media_dir: &'a str,
    pub stream_key: &'a str,
    pub sizes: &'a [u32],
    pub interval_seconds: u32,
    pub rate_limit_seconds: u32,
    pub live_update: bool,
    pub ended_flag: Option<Arc<AtomicBool>>,
    pub last_attempt: Option<Arc<AtomicU64>>,
    pub semaphore: Arc<Semaphore>,
}

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
    req: StreamThumbnailRequest<'_>,
) -> anyhow::Result<Vec<PathBuf>> {
    let StreamThumbnailRequest {
        media_dir,
        stream_key,
        sizes,
        interval_seconds,
        rate_limit_seconds,
        live_update,
        ended_flag,
        last_attempt,
        semaphore,
    } = req;

    // Rate-limit check first (cheapest, no I/O)
    if let Some(ref attempt_ts) = last_attempt {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let last = attempt_ts.load(Ordering::Relaxed);
        if now.saturating_sub(last) < rate_limit_seconds as u64 {
            return Err(anyhow::anyhow!("rate limited"));
        }
        attempt_ts.store(now, Ordering::Relaxed);
    }

    // Early ended check (atomic read, no I/O)
    if let Some(ref flag) = ended_flag
        && flag.load(Ordering::Relaxed)
    {
        return Err(anyhow::anyhow!("stream has ended"));
    }

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

    // Acquire concurrency permit (may wait for other ffmpeg to drain)
    let _permit = semaphore.acquire().await;

    // Re-check after acquiring permit
    if let Some(ref flag) = ended_flag
        && flag.load(Ordering::Relaxed)
    {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(anyhow::anyhow!("stream has ended"));
    }

    let mut results = Vec::new();
    for &width in sizes {
        let png_path = dir.join(format!("{}_w{}.png", stream_key, width));

        // Remove stale 0-byte thumbnail files before deciding whether to regenerate
        for fmt in THUMBNAIL_FORMATS {
            let p = dir.join(format!("{}_w{}.{}", stream_key, width, fmt));
            if let Ok(meta) = tokio::fs::metadata(&p).await
                && meta.len() == 0
            {
                let _ = tokio::fs::remove_file(&p).await;
            }
        }

        // Decide whether to (re)generate. In once-mode (live_update=false) we
        // only generate when no thumbnail exists yet, then never refresh.
        // In live-update mode we regenerate once the PNG is older than the
        // configured interval. PNG is the canonical existence indicator.
        let should_generate = if !live_update {
            !tokio::fs::try_exists(&png_path).await.unwrap_or(false)
        } else if let Ok(meta) = tokio::fs::metadata(&png_path).await {
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
            let mut all_ok = true;
            for &fmt in THUMBNAIL_FORMATS {
                let thumb_path = dir.join(format!("{}_w{}.{}", stream_key, width, fmt));
                if let Err(e) = run_ffmpeg_thumbnail_fmt(&tmp_path, &thumb_path, Some(width), fmt).await {
                    tracing::warn!(
                        "Thumbnail generation failed for {} w={} fmt={}: {}",
                        stream_key,
                        width,
                        fmt,
                        e
                    );
                    all_ok = false;
                }
            }
            if all_ok {
                results.push(png_path);
            }
        } else {
            results.push(png_path);
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
    semaphore: Arc<Semaphore>,
) -> anyhow::Result<Vec<PathBuf>> {
    tokio::fs::create_dir_all(output_dir).await?;

    let _permit = semaphore.acquire().await;

    let filename = video_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid video path"))?;

    let mut results = Vec::new();
    for &width in sizes {
        for &fmt in THUMBNAIL_FORMATS {
            let thumb_path = output_dir.join(format!("{}_w{}.{}", filename, width, fmt));
            run_ffmpeg_thumbnail_fmt(video_path, &thumb_path, Some(width), fmt).await?;
            if fmt == "png" {
                results.push(thumb_path);
            }
        }
    }

    Ok(results)
}

async fn run_ffmpeg_thumbnail_fmt(
    input: &std::path::Path,
    output: &std::path::Path,
    width: Option<u32>,
    fmt: &str,
) -> anyhow::Result<PathBuf> {
    let tmp_output = output.with_extension(format!("{}.tmp", fmt));

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
    ];

    match fmt {
        "jxl" => {
            args.push("-c:v".to_string());
            args.push("libjxl".to_string());
            args.push("-q:v".to_string());
            args.push("90".to_string());
        }
        "avif" => {
            args.push("-c:v".to_string());
            args.push("libaom-av1".to_string());
            args.push("-crf".to_string());
            args.push("30".to_string());
            args.push("-still-picture".to_string());
            args.push("1".to_string());
        }
        _ => {} // PNG: no extra codec flags needed
    }

    if let Some(w) = width {
        args.push("-vf".to_string());
        args.push(format!("scale={}:-1", w));
    }

    args.push("-f".to_string());
    args.push(match fmt {
        "jxl" => "image2",
        "avif" => "avif",
        _ => "apng",
    }.to_string());
    args.push(tmp_output.to_str().unwrap().to_string());

    let child = Command::new("ffmpeg")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("ffmpeg spawn failed: {}", e))?;

    let child_id = child.id();

    match timeout(Duration::from_secs(FFMPEG_TIMEOUT_SECS), child.wait_with_output()).await {
        Ok(Ok(cmd_output)) => {
            if !cmd_output.status.success() {
                let stderr = String::from_utf8_lossy(&cmd_output.stderr);
                let _ = tokio::fs::remove_file(&tmp_output).await;
                return Err(anyhow::anyhow!("ffmpeg failed for {}: {}", fmt, stderr));
            }

            let meta = tokio::fs::metadata(&tmp_output).await?;
            if meta.len() == 0 {
                let _ = tokio::fs::remove_file(&tmp_output).await;
                return Err(anyhow::anyhow!("ffmpeg produced empty output for {}", fmt));
            }

            tokio::fs::rename(&tmp_output, output).await?;
            Ok(output.to_path_buf())
        }
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&tmp_output).await;
            Err(anyhow::anyhow!("ffmpeg process error: {}", e))
        }
        Err(_) => {
            if let Some(id) = child_id {
                let _ = Command::new("kill")
                    .arg("-9")
                    .arg(id.to_string())
                    .output()
                    .await;
            }
            let _ = tokio::fs::remove_file(&tmp_output).await;
            Err(anyhow::anyhow!(
                "ffmpeg timed out after {}s for {}",
                FFMPEG_TIMEOUT_SECS,
                fmt
            ))
        }
    }
}

const THUMBNAIL_FORMATS: &[&str] = &["jxl", "avif", "png"];
