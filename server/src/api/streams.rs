use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use std::collections::BTreeMap;
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
    pub thumbnail_url: String,
    pub thumbnails: BTreeMap<String, String>,
    pub tracks: Vec<crate::rtmp::TrackInfo>,
}

fn build_stream_response(
    info: &crate::rtmp::PublisherInfo,
    thumbnail_sizes: &[u32],
) -> StreamResponse {
    let hls_url = Some(format!("/hls/{}/index.m3u8", info.stream_key));
    let player_url = Some(format!("/live/{}", info.stream_key));

    let mut thumbnails = BTreeMap::new();
    for &width in thumbnail_sizes {
        thumbnails.insert(
            width.to_string(),
            format!("/thumbnails/streams/{}_w{}.png", info.stream_key, width),
        );
    }
    let thumbnail_url = thumbnail_sizes
        .first()
        .map(|w| format!("/thumbnails/streams/{}_w{}.png", info.stream_key, w))
        .unwrap_or_default();

    StreamResponse {
        stream_key: info.stream_key.clone(),
        status: "live".to_string(),
        started_at: Some(info.started_at),
        metadata: info.metadata.clone(),
        hls_url,
        player_url,
        thumbnail_url,
        thumbnails,
        tracks: info.tracks.clone(),
    }
}

/// Fire-and-forget live-stream thumbnail generation so the static files exist
/// for nginx to serve. Shared by the list and get endpoints.
fn spawn_stream_thumbnails(state: &Arc<AppState>, info: &crate::rtmp::PublisherInfo) {
    let md = state.config.media_dir.clone();
    let sz = state.config.thumbnail_sizes.clone();
    let iv = state.config.thumbnail_interval_seconds;
    let rl = state.config.thumbnail_rate_limit_seconds;
    let ended = info.ended.clone();
    let last_attempt = info.last_thumbnail_attempt_secs.clone();
    let sem = state.thumbnail_semaphore.clone();
    let key = info.stream_key.clone();
    tokio::spawn(async move {
        let _ = crate::thumbnail::generate_thumbnails_for_stream(
            crate::thumbnail::StreamThumbnailRequest {
                media_dir: &md,
                stream_key: &key,
                sizes: &sz,
                interval_seconds: iv,
                rate_limit_seconds: rl,
                ended_flag: Some(ended),
                last_attempt: Some(last_attempt),
                semaphore: sem,
            },
        )
        .await;
    });
}

pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let sizes = state.config.thumbnail_sizes.clone();
    let sm = state.stream_manager.read().await;
    let mut streams: Vec<StreamResponse> = sm
        .publishers()
        .values()
        .map(|info| {
            spawn_stream_thumbnails(&state, info);
            build_stream_response(info, &sizes)
        })
        .collect();
    drop(sm);

    streams.sort_by(|a, b| a.stream_key.cmp(&b.stream_key));

    (StatusCode::OK, Json(serde_json::json!(streams)))
}

pub async fn get(State(state): State<Arc<AppState>>, Path(key): Path<String>) -> impl IntoResponse {
    let sm = state.stream_manager.read().await;
    match sm.get_publisher(&key) {
        Some(info) => {
            spawn_stream_thumbnails(&state, info);

            (
                StatusCode::OK,
                Json(serde_json::json!(build_stream_response(
                    info,
                    &state.config.thumbnail_sizes
                ))),
            )
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Stream not found"})),
        ),
    }
}
