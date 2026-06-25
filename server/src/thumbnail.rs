use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;

const FFMPEG_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy)]
struct CodecInfo {
    jxl_available: bool,
    avif_available: bool,
}

static CODEC_INFO: OnceLock<CodecInfo> = OnceLock::new();

pub fn init_codec_info(jxl: bool, avif: bool) {
    let _ = CODEC_INFO.set(CodecInfo {
        jxl_available: jxl,
        avif_available: avif,
    });
}

fn available_formats() -> Vec<&'static str> {
    let info = CODEC_INFO.get();
    let (jxl, avif) = match info {
        Some(c) => (c.jxl_available, c.avif_available),
        None => {
            tracing::warn!("Codec info not initialized; probing lazily is not supported. Skipping JXL/AVIF.");
            (false, false)
        }
    };
    let mut fmts = vec!["jxl", "avif", "png"];
    if !jxl {
        fmts.retain(|&f| f != "jxl");
    }
    if !avif {
        fmts.retain(|&f| f != "avif");
    }
    fmts
}

/// Probe ffmpeg for available image codecs. Returns `(jxl_available, avif_available)`.
pub async fn probe_codecs() -> (bool, bool) {
    let output = match Command::new("ffmpeg")
        .args(["-hide_banner", "-codecs"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("Failed to probe ffmpeg codecs: {}", e);
            return (false, false);
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let jxl = stdout.contains("libjxl");
    let avif = stdout.contains("libaom-av1");
    (jxl, avif)
}

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

// ── Phase 1: single-pass PNG ref extraction ─────────────────────────
//
// Uses ffmpeg filter_complex with split+scale to decode the source once
// and produce one PNG ref per width — zero redundant mp4 reads.

async fn extract_png_refs(
    video_path: &std::path::Path,
    tmp_dir: &std::path::Path,
    widths: &[u32],
) -> anyhow::Result<Vec<(u32, PathBuf)>> {
    let n = widths.len();
    if n == 0 {
        return Ok(vec![]);
    }

    let mut filter_parts: Vec<String> = Vec::with_capacity(1 + n);

    // Split into N label streams
    let split_outs: String = (0..n).map(|i| format!("[v{}]", i)).collect();
    filter_parts.push(format!("[0:v]split={}{}", n, split_outs));

    // Scale each stream
    let mut map_args: Vec<String> = Vec::with_capacity(n * 6);
    for (i, &w) in widths.iter().enumerate() {
        filter_parts.push(format!("[v{}]scale={}:-1[s{}]", i, w, i));
        let ref_path = tmp_dir.join(format!("ref_w{}.png", w));
        map_args.extend_from_slice(&[
            "-map".into(),
            format!("[s{}]", i),
            "-f".into(),
            "apng".into(),
            "-frames:v".into(),
            "1".into(),
            ref_path.to_str().unwrap().into(),
        ]);
    }

    let filter_graph = filter_parts.join(";");

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-ss".into(),
        "00:00:00.5".into(),
        "-i".into(),
        video_path.to_str().unwrap().into(),
        "-filter_complex".into(),
        filter_graph,
    ];
    args.extend(map_args);

    let output = match timeout(
        Duration::from_secs(FFMPEG_TIMEOUT_SECS),
        Command::new("ffmpeg")
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(anyhow::anyhow!("Phase 1 ffmpeg spawn failed: {}", e)),
        Err(_) => return Err(anyhow::anyhow!("Phase 1 ffmpeg timed out")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Phase 1 ffmpeg failed: {}", stderr));
    }

    let mut results = Vec::new();
    for &w in widths {
        let ref_path = tmp_dir.join(format!("ref_w{}.png", w));
        if let Ok(true) = tokio::fs::try_exists(&ref_path).await
            && let Ok(meta) = tokio::fs::metadata(&ref_path).await
            && meta.len() > 0
        {
            results.push((w, ref_path));
        }
    }

    Ok(results)
}

// ── Phase 2: encode a format from a PNG ref ─────────────────────────

async fn encode_fmt_from_png(
    png_path: &std::path::Path,
    output_path: &std::path::Path,
    fmt: &str,
) -> anyhow::Result<()> {
    let tmp_output = output_path.with_extension(format!("{}.tmp", fmt));

    match fmt {
        "png" => {
            // No re-encode — copy the PNG ref directly
            tokio::fs::copy(png_path, output_path).await?;
            return Ok(());
        }
        "jxl" | "avif" => {}
        _ => return Err(anyhow::anyhow!("unknown format: {}", fmt)),
    };

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-i".into(),
        png_path.to_str().unwrap().into(),
    ];

    match fmt {
        "jxl" => {
            args.extend_from_slice(&[
                "-c:v".into(),
                "libjxl".into(),
                "-q:v".into(),
                "90".into(),
                "-f".into(),
                "image2".into(),
            ]);
        }
        "avif" => {
            args.extend_from_slice(&[
                "-c:v".into(),
                "libaom-av1".into(),
                "-crf".into(),
                "30".into(),
                "-still-picture".into(),
                "1".into(),
                "-f".into(),
                "avif".into(),
            ]);
        }
        _ => unreachable!(),
    }

    args.push(tmp_output.to_str().unwrap().into());

    let child = Command::new("ffmpeg")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("ffmpeg spawn failed for {}: {}", fmt, e))?;

    let child_id = child.id();

    match timeout(
        Duration::from_secs(FFMPEG_TIMEOUT_SECS),
        child.wait_with_output(),
    )
    .await
    {
        Ok(Ok(cmd_output)) => {
            if !cmd_output.status.success() {
                let stderr = String::from_utf8_lossy(&cmd_output.stderr);
                let _ = tokio::fs::remove_file(&tmp_output).await;
                return Err(anyhow::anyhow!("ffmpeg failed for {}: {}", fmt, stderr));
            }

            let meta = tokio::fs::metadata(&tmp_output).await?;
            if meta.len() == 0 {
                let _ = tokio::fs::remove_file(&tmp_output).await;
                return Err(anyhow::anyhow!(
                    "ffmpeg produced empty output for {}",
                    fmt
                ));
            }

            tokio::fs::rename(&tmp_output, output_path).await?;
            Ok(())
        }
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&tmp_output).await;
            Err(anyhow::anyhow!("ffmpeg process error for {}: {}", fmt, e))
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

// ── Public API ──────────────────────────────────────────────────────

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

    let formats = available_formats();

    // Phase 0: classify widths into needs-generation vs already-fresh.
    // Already-fresh PNGs can be reused as refs for JXL/AVIF encoding.
    let mut needs_gen: Vec<u32> = Vec::new();
    let mut already_ok: Vec<(u32, PathBuf)> = Vec::new(); // (width, png_path)

    for &width in sizes {
        let png_path = dir.join(format!("{}_w{}.png", stream_key, width));

        // Remove stale 0-byte thumbnail files before deciding whether to regenerate
        for fmt in &formats {
            let p = dir.join(format!("{}_w{}.{}", stream_key, width, fmt));
            if let Ok(meta) = tokio::fs::metadata(&p).await
                && meta.len() == 0
            {
                let _ = tokio::fs::remove_file(&p).await;
            }
        }

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
            needs_gen.push(width);
        } else {
            already_ok.push((width, png_path));
        }
    }

    let mut results = Vec::new();

    if !needs_gen.is_empty() {
        // Phase 1: extract PNG refs for all widths that need generation in one pass
        let tmp_dir = dir.join(format!(".thumbnail_tmp_{}", stream_key));
        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
        tokio::fs::create_dir_all(&tmp_dir).await?;

        let refs = match extract_png_refs(&tmp_path, &tmp_dir, &needs_gen).await {
            Ok(r) => r,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(e);
            }
        };

        // Phase 2: encode JXL/AVIF/PNG from each ref
        for (w, ref_path) in &refs {
            let mut all_ok = true;
            for fmt in &formats {
                let out_path = dir.join(format!("{}_w{}.{}", stream_key, w, fmt));
                if let Err(e) = encode_fmt_from_png(ref_path, &out_path, fmt).await {
                    tracing::warn!(
                        "Thumbnail generation failed for {} w={} fmt={}: {}",
                        stream_key,
                        w,
                        fmt,
                        e
                    );
                    all_ok = false;
                }
            }
            if all_ok {
                let png_path = dir.join(format!("{}_w{}.png", stream_key, w));
                results.push(png_path);
            }
        }

        let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    }

    // Add already-fresh widths to results. Also ensure JXL/AVIF exist
    // by re-encoding from the existing PNG (matches script reuse logic).
    for (w, png_path) in &already_ok {
        for fmt in &formats {
            let out_path = dir.join(format!("{}_w{}.{}", stream_key, w, fmt));
            if tokio::fs::try_exists(&out_path).await.unwrap_or(false) {
                continue; // already exists, skip
            }
            if let Err(e) = encode_fmt_from_png(png_path, &out_path, fmt).await {
                tracing::warn!(
                    "Thumbnail format fill failed for {} w={} fmt={}: {}",
                    stream_key,
                    w,
                    fmt,
                    e
                );
            }
        }
        results.push(png_path.clone());
    }

    // Clean up temp file
    let _ = tokio::fs::remove_file(&tmp_path).await;

    if results.is_empty() && !sizes.is_empty() {
        return Err(anyhow::anyhow!(
            "all thumbnail formats failed for stream {}",
            stream_key
        ));
    }

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

    let formats = available_formats();

    // Clean stale 0-byte thumbnails before generating
    for &w in sizes {
        for fmt in &formats {
            let p = output_dir.join(format!("{}_w{}.{}", filename, w, fmt));
            if let Ok(meta) = tokio::fs::metadata(&p).await
                && meta.len() == 0
            {
                let _ = tokio::fs::remove_file(&p).await;
            }
        }
    }

    // Phase 1: extract PNG refs for all widths in a single pass
    let tmp_dir = output_dir.join(format!(".thumbnail_tmp_{}", filename));
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    tokio::fs::create_dir_all(&tmp_dir).await?;

    let refs = extract_png_refs(video_path, &tmp_dir, sizes).await?;

    // Phase 2: encode JXL/AVIF/PNG from each ref
    let mut results = Vec::new();
    for (w, ref_path) in &refs {
        let mut all_ok = true;
        for fmt in &formats {
            let out_path = output_dir.join(format!("{}_w{}.{}", filename, w, fmt));
            if let Err(e) = encode_fmt_from_png(ref_path, &out_path, fmt).await {
                tracing::warn!(
                    "Thumbnail generation failed for {} w={} fmt={}: {}",
                    filename,
                    w,
                    fmt,
                    e
                );
                all_ok = false;
            }
        }
        if all_ok {
            let png_path = output_dir.join(format!("{}_w{}.png", filename, w));
            results.push(png_path);
        }
    }

    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;

    if results.is_empty() && !sizes.is_empty() {
        return Err(anyhow::anyhow!(
            "all thumbnail formats failed for {}",
            filename
        ));
    }

    Ok(results)
}
