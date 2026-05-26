use axum::{Json, extract::{State, Path, Query}, response::IntoResponse, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct StreamResponse {
    pub stream_key: String,
    pub status: String,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub metadata: Option<crate::rtmp::StreamMeta>,
    pub hls_url: Option<String>,
    pub player_url: Option<String>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let sm = state.stream_manager.read().await;
    let streams: Vec<StreamResponse> = sm.publishers.values().map(|info| {
        let hls_url = Some(format!("/hls/{}/index.m3u8", info.stream_key));
        let player_url = Some(format!("/live/{}", info.stream_key));
        StreamResponse {
            stream_key: info.stream_key.clone(),
            status: "live".to_string(),
            started_at: Some(info.started_at),
            metadata: info.metadata.clone(),
            hls_url,
            player_url,
        }
    }).collect();

    (StatusCode::OK, Json(serde_json::json!(streams)))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let sm = state.stream_manager.read().await;
    match sm.publishers.get(&key) {
        Some(info) => {
            let hls_url = Some(format!("/hls/{}/index.m3u8", info.stream_key));
            let player_url = Some(format!("/live/{}", info.stream_key));
            (StatusCode::OK, Json(serde_json::json!(StreamResponse {
                stream_key: info.stream_key.clone(),
                status: "live".to_string(),
                started_at: Some(info.started_at),
                metadata: info.metadata.clone(),
                hls_url,
                player_url,
            })))
        }
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Stream not found"}))),
    }
}

#[derive(Deserialize)]
pub struct ThumbnailQuery {
    pub width: Option<u32>,
}

pub async fn thumbnail(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Query(query): Query<ThumbnailQuery>,
) -> impl IntoResponse {
    let width = crate::api::closest_thumbnail_width(query.width, &state.config.thumbnail_sizes);
    let ttl = state.config.thumbnail_interval_seconds as u64;
    let cache_header = format!("max-age={}, stale-while-revalidate={}", ttl, ttl * 2);

    let thumb_dir = std::path::PathBuf::from(&state.config.media_dir).join("thumbnails").join("streams");
    let thumb_path = thumb_dir.join(format!("{}_w{}.webp", key, width));

    match tokio::fs::read(&thumb_path).await {
        Ok(data) => {
            (
                StatusCode::OK,
                [
                    (axum::http::header::CONTENT_TYPE, "image/webp"),
                    (axum::http::header::CACHE_CONTROL, cache_header.as_str()),
                ],
                data,
            ).into_response()
        }
        Err(_) => {
            // Try generating on the fly if the pre-generated thumbnail doesn't exist
            match crate::thumbnail::generate_thumbnails_for_stream(
                &state.config.media_dir,
                &key,
                &[width],
                0,
            ).await {
                Ok(paths) if !paths.is_empty() => {
                    match tokio::fs::read(&paths[0]).await {
                        Ok(data) => {
                            (
                                StatusCode::OK,
                                [
                                    (axum::http::header::CONTENT_TYPE, "image/webp"),
                                    (axum::http::header::CACHE_CONTROL, cache_header.as_str()),
                                ],
                                data,
                            ).into_response()
                        }
                        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read thumbnail").into_response(),
                    }
                }
                _ => (StatusCode::NOT_FOUND, "Thumbnail not available").into_response(),
            }
        }
    }
}
