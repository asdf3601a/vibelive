use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    http::StatusCode,
    Json,
};
use serde::Serialize;
use std::collections::HashMap;
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
    thumbnails: HashMap<String, String>,
}

pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let dir = PathBuf::from(&state.config.media_dir).join("recordings");
    let index_path = dir.join("index.json");

    let mut recordings = Vec::new();

    // Try reading from index.json first
    if let Ok(index_data) = tokio::fs::read_to_string(&index_path).await
        && let Ok(index) = serde_json::from_str::<crate::recording::RecordingsIndex>(&index_data)
    {
        for entry in index.recordings {
            recordings.push(RecordingResponse {
                filename: entry.filename,
                stream_key: entry.stream_key,
                created_at: entry.created_at,
                size_bytes: entry.size_bytes,
                duration_seconds: entry.duration_seconds,
                url: entry.url,
                thumbnail_url: entry.thumbnails.values().next().cloned().unwrap_or_default(),
                thumbnails: entry.thumbnails,
            });
        }
        return (StatusCode::OK, Json(serde_json::json!(recordings)));
    }

    // Fallback: scan directory
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

            let stream_key = crate::recording::parse_stream_key_from_filename(&name);

            let duration = get_video_duration(&path).await.ok();

            let mut thumbnails = HashMap::new();
            for width in &state.config.thumbnail_sizes {
                let thumb_filename = format!("{}_w{}.webp", name, width);
                let thumb_path = PathBuf::from(&state.config.media_dir)
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

            recordings.push(RecordingResponse {
                filename: name.clone(),
                stream_key,
                created_at: modified,
                size_bytes: size,
                duration_seconds: duration,
                url: format!("{}/{}", state.config.recordings_base_url, name),
                thumbnail_url: thumbnails.values().next().cloned().unwrap_or_default(),
                thumbnails,
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
    let width = crate::api::closest_thumbnail_width(query.width, &state.config.thumbnail_sizes);

    let thumb_dir = PathBuf::from(&state.config.media_dir).join("thumbnails").join("recordings");
    let thumb_path = thumb_dir.join(format!("{}_w{}.webp", filename, width));

    match tokio::fs::read(&thumb_path).await {
        Ok(data) => {
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "image/webp")],
                data,
            ).into_response()
        }
        Err(_) => {
            // Fallback: generate on-the-fly if pre-generated doesn't exist
            let video_path = PathBuf::from(&state.config.media_dir).join("recordings").join(&filename);
            if !tokio::fs::try_exists(&video_path).await.unwrap_or(false) {
                return (StatusCode::NOT_FOUND, "Recording not found").into_response();
            }

            let _ = tokio::fs::create_dir_all(&thumb_dir).await;
            match crate::thumbnail::generate_thumbnails_for_file(&video_path, &thumb_dir, &[width]).await {
                Ok(paths) if !paths.is_empty() => {
                    match tokio::fs::read(&paths[0]).await {
                        Ok(data) => {
                            (
                                StatusCode::OK,
                                [(axum::http::header::CONTENT_TYPE, "image/webp")],
                                data,
                            ).into_response()
                        }
                        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read thumbnail").into_response(),
                    }
                }
                Ok(_) => {
                    (StatusCode::NOT_FOUND, "Thumbnail not available").into_response()
                }
                Err(e) => {
                    tracing::warn!("Thumbnail generation failed for {}: {}", filename, e);
                    (StatusCode::NOT_FOUND, "Thumbnail not available").into_response()
                }
            }
        }
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
