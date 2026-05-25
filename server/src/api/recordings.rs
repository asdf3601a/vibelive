use axum::{extract::State, response::IntoResponse, Json, http::StatusCode};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct RecordingResponse {
    filename: String,
    stream_key: String,
    created_at: String,
    size_bytes: u64,
    url: String,
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

                recordings.push(RecordingResponse {
                    filename: name.clone(),
                    stream_key,
                    created_at: modified,
                    size_bytes: size,
                    url: format!("/recordings/{}", name),
                });
            }
        }

    // Sort by created_at descending
    recordings.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    (StatusCode::OK, Json(serde_json::json!(recordings)))
}
