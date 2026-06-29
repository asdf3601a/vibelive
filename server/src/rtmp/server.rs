use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::AppState;

pub async fn start_rtmp_server(
    addr: SocketAddr,
    app_state: Arc<AppState>,
    shutdown_token: CancellationToken,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!("RTMP server listening on {}", addr);

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, peer_addr) = match accept_result {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::error!("RTMP accept error: {}", e);
                        continue;
                    }
                };
                let state = app_state.clone();

                tokio::spawn(async move {
                    tracing::debug!("RTMP connection from {}", peer_addr);
                    if let Err(e) = super::session::handle_rtmp_session(stream, peer_addr, state).await {
                        tracing::error!("RTMP session error from {}: {}", peer_addr, e);
                    }
                });
            }
            _ = shutdown_token.cancelled() => {
                tracing::info!("RTMP server: shutdown signal received, stopping accept loop.");
                return Ok(());
            }
        }
    }
}
