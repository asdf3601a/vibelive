use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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
                thumbnail_url: entry
                    .thumbnails
                    .values()
                    .next()
                    .cloned()
                    .unwrap_or_default(),
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
                duration_seconds: None,
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
