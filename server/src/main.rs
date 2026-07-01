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
use tokio_util::sync::CancellationToken;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
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

fn build_cors_layer(origins: &str) -> CorsLayer {
    if origins == "*" {
        CorsLayer::permissive()
    } else {
        let allowed: Vec<_> = origins
            .split(',')
            .map(|o| o.trim().parse::<axum::http::HeaderValue>().unwrap())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(allowed))
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

/// Periodically generate thumbnails for all active streams.
async fn thumbnail_loop(app_state: Arc<AppState>, shutdown_token: CancellationToken) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
        app_state.config.thumbnail_interval_seconds as u64,
    ));
    loop {
        tokio::select! {
            _ = shutdown_token.cancelled() => {
                tracing::info!("Thumbnail loop: shutdown signal received, stopping.");
                return;
            }
            _ = interval.tick() => {}
        }

        let sm = app_state.stream_manager.read().await;
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
        let media_dir = app_state.config.media_dir.clone();
        let sizes = app_state.config.thumbnail_sizes.clone();
        let iv = app_state.config.thumbnail_interval_seconds;
        let rl = app_state.config.thumbnail_rate_limit_seconds;
        let lu = app_state.config.thumbnail_live_update;
        let sem = app_state.thumbnail_semaphore.clone();
        drop(sm);

        for (key, ended_flag, last_attempt) in tasks {
            let md = media_dir.clone();
            let sz = sizes.clone();
            let sem = sem.clone();
            let shutdown = shutdown_token.clone();
            tokio::spawn(async move {
                // Do not start thumbnail work if we're shutting down
                if shutdown.is_cancelled() {
                    return;
                }
                match crate::thumbnail::generate_thumbnails_for_stream(
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
                .await
                {
                    Ok(_paths) => {}
                    Err(e) => {
                        tracing::warn!("Thumbnail generation failed for stream '{}': {}", key, e);
                    }
                }
            });
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    dotenvy::dotenv().ok();
    let cfg = config::Config::from_env()?;
    cfg.validate()?;

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

    // Shared cancellation token for coordinated shutdown
    let shutdown_token = CancellationToken::new();

    // --- RTMP server ---
    let rtmp_addr: SocketAddr = format!("{}:{}", cfg.rtmp_host, cfg.rtmp_port).parse()?;
    let rtmp_state = app_state.clone();
    let rtmp_shutdown = shutdown_token.clone();
    tokio::spawn(async move {
        if let Err(e) = rtmp::start_rtmp_server(rtmp_addr, rtmp_state, rtmp_shutdown).await {
            tracing::error!("RTMP server error: {}", e);
        }
    });

    // --- Thumbnail generation loop ---
    let thumb_state = app_state.clone();
    let thumb_shutdown = shutdown_token.clone();
    tokio::spawn(async move {
        thumbnail_loop(thumb_state, thumb_shutdown).await;
    });

    // --- API server ---
    let api_router = api::create_router(app_state.clone());
    let api_addr: SocketAddr = format!("{}:{}", cfg.api_host, cfg.api_port).parse()?;

    let cors_layer = build_cors_layer(&cfg.cors_allowed_origins);
    let app = api_router
        .layer(cors_layer)
        .layer(TraceLayer::new_for_http());

    tracing::info!(
        "Server starting: API={}:{}, RTMP={}:{}",
        cfg.api_host,
        cfg.api_port,
        cfg.rtmp_host,
        cfg.rtmp_port,
    );

    let listener = tokio::net::TcpListener::bind(api_addr).await?;
    let disk_writer_ref = app_state.disk_writer.clone();

    tokio::select! {
        result = axum::serve(listener, app) => {
            if let Err(e) = result {
                tracing::error!("API server error: {}", e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C, shutting down gracefully...");
        }
    }

    // Signal all background tasks to stop
    shutdown_token.cancel();

    tracing::info!("Draining DiskWriter...");
    // Give a moment for in-flight DiskWriter commands to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    disk_writer_ref.flush().await;
    tracing::info!("Shutdown complete.");

    Ok(())
}
