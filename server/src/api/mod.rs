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

use axum::{Router, routing::get, response::Response, extract::State, Json, extract::Request};
use std::sync::Arc;
use crate::AppState;

fn extract_host(host_header: &str) -> String {
    // Handle IPv6 bracket notation like [::1]:8080
    if host_header.starts_with('[')
        && let Some(end) = host_header.find(']')
    {
        return host_header[1..end].to_string();
    }
    // IPv4 or hostname with optional port
    host_header.split(':').next().unwrap_or(host_header).to_string()
}

async fn get_config(State(state): State<Arc<AppState>>, req: Request) -> Json<serde_json::Value> {
    let host = req.headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(extract_host)
        .unwrap_or_else(|| state.config.api_host.clone());
    let rtmp_url = format!("rtmp://{}:{}/live/{{stream_key}}", host, state.config.rtmp_port);
    Json(serde_json::json!({
        "rtmp_url_template": rtmp_url,
        "multitrack_supported": true,
        "enhanced_rtmp": false,
        "supported_video_codecs": ["H264", "HEVC", "AV1"],
        "supported_audio_codecs": ["AAC", "Opus", "FLAC"],
        "example_ffmpeg_single": format!(
            "ffmpeg -re -f lavfi -i testsrc=duration=30:size=1280x720:rate=30 \\\n  -f lavfi -i \"sine=frequency=440:duration=30\" \\\n  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \\\n  -c:a aac -ar 44100 \\\n  -f flv {}",
            rtmp_url.replace("{{stream_key}}", "testkey")
        ),
        "example_ffmpeg_multitrack": format!(
            "ffmpeg -re \\\n  -f lavfi -i \"testsrc=duration=30:size=1280x720:rate=30\" \\\n  -f lavfi -i \"testsrc=duration=30:size=640x360:rate=30\" \\\n  -f lavfi -i \"sine=frequency=440:duration=30\" \\\n  -f lavfi -i \"sine=frequency=880:duration=30\" \\\n  -map 0:v -c:v:0 libsvtav1 -preset:v:0 12 -pix_fmt:v:0 yuv420p -b:v:0 1500k -g:v:0 60 \\\n  -map 1:v -c:v:1 libx264 -preset:v:1 ultrafast -pix_fmt:v:1 yuv420p -b:v:1 500k -g:v:1 60 \\\n  -map 2:a -c:a:0 libopus -ar:a:0 48000 -b:a:0 128k \\\n  -map 3:a -c:a:1 aac -ar:a:1 44100 -b:a:1 128k \\\n  -f flv {}",
            rtmp_url.replace("{{stream_key}}", "testkey")
        )
    }))
}

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
            axum::http::HeaderValue::from_static("application/vnd.apple.mpegurl; charset=utf-8"),
        );
    }
    response
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let hls_dir = format!("{}/hls", state.config.media_dir);
    let recordings_dir = format!("{}/recordings", state.config.media_dir);
    let thumbnails_dir = format!("{}/thumbnails", state.config.media_dir);

    let api_routes = Router::new()
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }))
        .route("/api/config", get(get_config))
        .route("/api/streams", get(streams::list))
        .route("/api/streams/{key}", get(streams::get))
        .route("/api/recordings", get(recordings::list))
        .route("/api/recordings/{filename}/thumbnail", get(recordings::thumbnail));

    let hls_service = tower_http::services::ServeDir::new(hls_dir);

    Router::new()
        .nest_service("/hls", hls_service)
        .nest_service("/recordings", tower_http::services::ServeDir::new(recordings_dir))
        .nest_service("/thumbnails", tower_http::services::ServeDir::new(thumbnails_dir))
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
