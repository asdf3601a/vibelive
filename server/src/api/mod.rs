pub mod recordings;
pub mod streams;

use axum::{Router, routing::get, response::Response};
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};
use crate::AppState;

async fn hls_content_type_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let uri = request.uri().path().to_string();
    let mut response = next.run(request).await;
    if uri.ends_with(".m4s") {
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("video/mp4"),
        );
    } else if uri.ends_with(".m3u8") {
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/vnd.apple.mpegurl"),
        );
    }
    response
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let hls_dir = format!("{}/hls", state.config.media_dir);
    let recordings_dir = format!("{}/recordings", state.config.media_dir);
    let frontend_dir = "/home/kilo/vibe-livestream/frontend/dist";
    let index_file = format!("{}/index.html", frontend_dir);

    let api_routes = Router::new()
        .route("/api/health", get(|| async { "OK" }))
        .route("/api/streams", get(streams::list))
        .route("/api/streams/{key}", get(streams::get))
        .route("/api/streams/{key}/thumbnail", get(streams::thumbnail))
        .route("/api/recordings", get(recordings::list))
        .route("/api/recordings/{filename}/thumbnail", get(recordings::thumbnail));

    let hls_service = ServeDir::new(hls_dir);

    Router::new()
        .nest_service("/hls", hls_service)
        .nest_service("/recordings", ServeDir::new(recordings_dir))
        .layer(axum::middleware::from_fn(hls_content_type_middleware))
        .merge(api_routes)
        .fallback_service(ServeDir::new(frontend_dir).not_found_service(ServeFile::new(index_file)))
        .with_state(state)
}
