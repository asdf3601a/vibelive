pub mod recordings;
pub mod streams;

pub fn closest_thumbnail_width(requested: Option<u32>, sizes: &[u32]) -> u32 {
    match requested {
        None => sizes.first().copied().unwrap_or(480),
        Some(w) => {
            let mut best = sizes.first().copied().unwrap_or(480);
            for &s in sizes {
                if s <= w && s > best {
                    best = s;
                }
            }
            best
        }
    }
}

use axum::{Router, routing::get, response::Response};
use std::sync::Arc;
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

    let api_routes = Router::new()
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .route("/api/streams", get(streams::list))
        .route("/api/streams/{key}", get(streams::get))
        .route("/api/streams/{key}/thumbnail", get(streams::thumbnail))
        .route("/api/recordings", get(recordings::list))
        .route("/api/recordings/{filename}/thumbnail", get(recordings::thumbnail));

    let hls_service = tower_http::services::ServeDir::new(hls_dir);

    Router::new()
        .nest_service("/hls", hls_service)
        .nest_service("/recordings", tower_http::services::ServeDir::new(recordings_dir))
        .layer(axum::middleware::from_fn(hls_content_type_middleware))
        .merge(api_routes)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closest_thumbnail_width_exact_match() {
        let sizes = vec![320, 480, 640];
        assert_eq!(closest_thumbnail_width(Some(480), &sizes), 480);
    }

    #[test]
    fn test_closest_thumbnail_width_lower() {
        let sizes = vec![320, 480];
        assert_eq!(closest_thumbnail_width(Some(640), &sizes), 480);
    }

    #[test]
    fn test_closest_thumbnail_width_below_all() {
        let sizes = vec![320, 480];
        assert_eq!(closest_thumbnail_width(Some(200), &sizes), 320);
    }

    #[test]
    fn test_closest_thumbnail_width_none() {
        let sizes = vec![320, 480];
        assert_eq!(closest_thumbnail_width(None, &sizes), 320);
    }

    #[test]
    fn test_closest_thumbnail_width_empty_sizes() {
        assert_eq!(closest_thumbnail_width(Some(200), &[]), 480);
        assert_eq!(closest_thumbnail_width(None, &[]), 480);
    }
}
