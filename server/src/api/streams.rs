use axum::{Json, extract::{State, Path}, response::IntoResponse, http::StatusCode};
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

fn build_stream_response(info: &crate::rtmp::PublisherInfo, thumbnail_sizes: &[u32]) -> StreamResponse {
    let hls_url = Some(format!("/hls/{}/index.m3u8", info.stream_key));
    let player_url = Some(format!("/live/{}", info.stream_key));

    let mut thumbnails = BTreeMap::new();
    for &width in thumbnail_sizes {
        thumbnails.insert(
            width.to_string(),
            format!("/thumbnails/streams/{}_w{}.webp", info.stream_key, width),
        );
    }
    let thumbnail_url = thumbnail_sizes.first()
        .map(|w| format!("/thumbnails/streams/{}_w{}.webp", info.stream_key, w))
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

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let sm = state.stream_manager.read().await;
    let sizes = state.config.thumbnail_sizes.clone();
    let media_dir = state.config.media_dir.clone();
    let interval = state.config.thumbnail_interval_seconds;

    let mut streams: Vec<StreamResponse> = sm.publishers().values().map(|info| {
        // Fire-and-forget thumbnail generation so static files exist for nginx
        let key = info.stream_key.clone();
        let md = media_dir.clone();
        let sz = sizes.clone();
        let iv = interval;
        tokio::spawn(async move {
            let _ = crate::thumbnail::generate_thumbnails_for_stream(&md, &key, &sz, iv).await;
        });
        build_stream_response(info, &sizes)
    }).collect();

    streams.sort_by(|a, b| a.stream_key.cmp(&b.stream_key));

    (StatusCode::OK, Json(serde_json::json!(streams)))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let sm = state.stream_manager.read().await;
    match sm.get_publisher(&key) {
        Some(info) => {
            // Fire-and-forget thumbnail generation
            let md = state.config.media_dir.clone();
            let sz = state.config.thumbnail_sizes.clone();
            let iv = state.config.thumbnail_interval_seconds;
            let stream_key = key.clone();
            tokio::spawn(async move {
                let _ = crate::thumbnail::generate_thumbnails_for_stream(&md, &stream_key, &sz, iv).await;
            });

            (StatusCode::OK, Json(serde_json::json!(build_stream_response(info, &state.config.thumbnail_sizes))))
        }
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "Stream not found"}))),
    }
}
