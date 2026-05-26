mod api;
mod config;
mod hls;
mod recording;
mod rtmp;
mod thumbnail;

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub stream_manager: RwLock<rtmp::StreamManager>,
    pub config: config::Config,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    dotenvy::dotenv().ok();
    let cfg = config::Config::from_env()?;

    let app_state = Arc::new(AppState {
        stream_manager: RwLock::new(rtmp::StreamManager::new()),
        config: cfg.clone(),
    });

    let rtmp_addr: SocketAddr = format!("{}:{}", cfg.rtmp_host, cfg.rtmp_port).parse()?;
    let rtmp_state = app_state.clone();
    tokio::spawn(async move {
        if let Err(e) = rtmp::start_rtmp_server(rtmp_addr, rtmp_state).await {
            tracing::error!("RTMP server error: {}", e);
        }
    });

    // Background task: periodically generate thumbnails for all active streams
    let thumb_state = app_state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
            thumb_state.config.thumbnail_interval_seconds.max(5) as u64,
        ));
        loop {
            interval.tick().await;
            let sm = thumb_state.stream_manager.read().await;
            let keys: Vec<String> = sm.publishers.keys().cloned().collect();
            let media_dir = thumb_state.config.media_dir.clone();
            let sizes = thumb_state.config.thumbnail_sizes.clone();
            let iv = thumb_state.config.thumbnail_interval_seconds;
            drop(sm);
            for key in keys {
                let md = media_dir.clone();
                let sz = sizes.clone();
                tokio::spawn(async move {
                    let _ = crate::thumbnail::generate_thumbnails_for_stream(&md, &key, &sz, iv).await;
                });
            }
        }
    });

    let api_router = api::create_router(app_state.clone());
    let api_addr: SocketAddr = format!("{}:{}", cfg.api_host, cfg.api_port).parse()?;

    let app = api_router
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    tracing::info!(
        "Server starting: API={}:{}, RTMP={}:{}",
        cfg.api_host,
        cfg.api_port,
        cfg.rtmp_host,
        cfg.rtmp_port,
    );

    let listener = tokio::net::TcpListener::bind(api_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
