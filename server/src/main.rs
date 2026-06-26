mod api;
mod config;
pub mod disk_writer;
mod hls;
mod recording;
mod rtmp;
mod thumbnail;
pub mod util;

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub stream_manager: RwLock<rtmp::StreamManager>,
    pub config: config::Config,
    pub remux_queue: Arc<recording::RemuxQueue>,
    pub thumbnail_semaphore: Arc<Semaphore>,
    pub recording_thumbnail_semaphore: Arc<Semaphore>,
    pub disk_writer: disk_writer::DiskWriter,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    dotenvy::dotenv().ok();
    let cfg = config::Config::from_env()?;

    // Probe ffmpeg codecs once at startup so thumbnail generation skips
    // unavailable formats (libjxl, libaom-av1) instead of wasting retries.
    let (jxl_ok, avif_ok) = thumbnail::probe_codecs().await;
    thumbnail::init_codec_info(jxl_ok, avif_ok);
    tracing::info!(
        "Thumbnail codecs — jxl: {}, avif: {}, png: always available",
        if jxl_ok { "available" } else { "unavailable" },
        if avif_ok { "available" } else { "unavailable" },
    );

    let disk_writer = disk_writer::DiskWriter::new();

    let app_state = Arc::new(AppState {
        stream_manager: RwLock::new(rtmp::StreamManager::new()),
        config: cfg.clone(),
        remux_queue: Arc::new(recording::RemuxQueue::new(
            cfg.recording_remux_enabled,
            cfg.recording_remux_concurrency as usize,
        )),
        thumbnail_semaphore: Arc::new(Semaphore::new(cfg.thumbnail_ffmpeg_concurrency as usize)),
        recording_thumbnail_semaphore: Arc::new(Semaphore::new(
            cfg.thumbnail_ffmpeg_concurrency as usize,
        )),
        disk_writer,
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
            thumb_state.config.thumbnail_interval_seconds as u64,
        ));
        loop {
            interval.tick().await;
            let sm = thumb_state.stream_manager.read().await;
            let tasks: Vec<(
                String,
                Arc<std::sync::atomic::AtomicBool>,
                Arc<std::sync::atomic::AtomicU64>,
            )> = sm
                .publishers()
                .iter()
                .map(|(key, info)| {
                    (
                        key.clone(),
                        info.ended.clone(),
                        info.last_thumbnail_attempt_secs.clone(),
                    )
                })
                .collect();
            let media_dir = thumb_state.config.media_dir.clone();
            let sizes = thumb_state.config.thumbnail_sizes.clone();
            let iv = thumb_state.config.thumbnail_interval_seconds;
            let rl = thumb_state.config.thumbnail_rate_limit_seconds;
            let lu = thumb_state.config.thumbnail_live_update;
            let sem = thumb_state.thumbnail_semaphore.clone();
            drop(sm);
            for (key, ended_flag, last_attempt) in tasks {
                let md = media_dir.clone();
                let sz = sizes.clone();
                let sem = sem.clone();
                tokio::spawn(async move {
                    let _ = crate::thumbnail::generate_thumbnails_for_stream(
                        crate::thumbnail::StreamThumbnailRequest {
                            media_dir: &md,
                            stream_key: &key,
                            sizes: &sz,
                            interval_seconds: iv,
                            rate_limit_seconds: rl,
                            live_update: lu,
                            ended_flag: Some(ended_flag),
                            last_attempt: Some(last_attempt),
                            semaphore: sem,
                        },
                    )
                    .await;
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
