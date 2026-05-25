use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

use crate::AppState;

#[derive(Serialize)]
pub struct RecordingResponse {
    filename: String,
    stream_key: String,
    created_at: String,
    size_bytes: u64,
    duration_seconds: Option<u64>,
    url: String,
    thumbnail_url: String,
}

pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dir = PathBuf::from(&state.config.media_dir).join("recordings");

    let mut recordings = Vec::new();

    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
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

            // Derive stream_key from filename: {key}_{timestamp}.mp4
            let stream_key = name
                .rsplitn(2, '_')
                .last()
                .and_then(|p| p.strip_suffix(".mp4"))
                .unwrap_or("")
                .to_string();

            let duration = get_video_duration(&path).await.ok();

            recordings.push(RecordingResponse {
                filename: name.clone(),
                stream_key,
                created_at: modified,
                size_bytes: size,
                duration_seconds: duration,
                url: format!("/recordings/{}", name),
                thumbnail_url: format!("/api/recordings/{}/thumbnail", name),
            });
        }
    }

    // Sort by created_at descending
    recordings.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    (StatusCode::OK, Json(serde_json::json!(recordings)))
}

#[derive(serde::Deserialize)]
pub struct ThumbnailQuery {
    width: Option<u32>,
}

pub async fn thumbnail(
    State(state): State<Arc<AppState>>,
    Path(filename): Path<String>,
    Query(query): Query<ThumbnailQuery>,
) -> impl IntoResponse {
    let width = query.width;
    let media_dir = &state.config.media_dir;

    let video_path = PathBuf::from(media_dir).join("recordings").join(&filename);
    if !video_path.exists() {
        return (StatusCode::NOT_FOUND, "Recording not found").into_response();
    }

    let thumb_dir = PathBuf::from(media_dir).join("thumbnails").join("recordings");
    let _ = tokio::fs::create_dir_all(&thumb_dir).await;

    let suffix = width.map(|w| format!("_w{}", w)).unwrap_or_default();
    let thumb_path = thumb_dir.join(format!("{}{}.jpg", filename, suffix));

    // Generate thumbnail if not cached
    if !thumb_path.exists() {
        match crate::thumbnail::generate_from_file(&video_path, &thumb_path, width).await {
            Ok(_) => {},
            Err(e) => {
                tracing::warn!("Thumbnail generation failed for {}: {}", filename, e);
                return (StatusCode::NOT_FOUND, "Thumbnail not available").into_response();
            }
        }
    }

    match tokio::fs::read(&thumb_path).await {
        Ok(data) => {
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "image/jpeg")],
                data,
            ).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read thumbnail").into_response(),
    }
}

async fn get_video_duration(path: &std::path::Path) -> anyhow::Result<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path.to_str().unwrap(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow::anyhow!("ffprobe failed"));
    }

    let duration_str = String::from_utf8_lossy(&output.stdout);
    let duration_sec: f64 = duration_str.trim().parse()?;
    Ok(duration_sec.ceil() as u64)
}
