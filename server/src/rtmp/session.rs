use crate::AppState;
use crate::hls::HlsStreamState;
use crate::recording::Fmp4Recorder;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn handle_rtmp_session(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    app_state: Arc<AppState>,
) -> anyhow::Result<()> {
    let mut handshake = Handshake::new(PeerType::Server);
    let mut handshake_done = false;

    let config = ServerSessionConfig {
        chunk_size: 4096,
        ..ServerSessionConfig::new()
    };
    let (mut session, init_results) =
        ServerSession::new(config).map_err(|e| anyhow::anyhow!("session create: {}", e))?;
    let mut init_results = Some(init_results);

    let mut buf = vec![0u8; 1024 * 64];
    let mut session_ctx = SessionContext::new();

    let session_result: anyhow::Result<()> = async {
        loop {
            let n = match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("RTMP read error for {}: {}", peer_addr, e);
                    break;
                }
            };
            tracing::debug!("Read {} bytes from socket", n);
            let data = &buf[..n];
            tracing::trace!("HEXDUMP: {:?}", &data[..n.min(64)]);

            if !handshake_done {
                tracing::debug!("Handshake: processing {} bytes", data.len());
                match handshake.process_bytes(data) {
                    Ok(HandshakeProcessResult::InProgress { response_bytes }) => {
                        if !response_bytes.is_empty() {
                            stream.write_all(&response_bytes).await?;
                            stream.flush().await?;
                        }
                        continue;
                    }
                    Ok(HandshakeProcessResult::Completed {
                        response_bytes,
                        remaining_bytes,
                    }) => {
                        if !response_bytes.is_empty() {
                            stream.write_all(&response_bytes).await?;
                            stream.flush().await?;
                        }
                        handshake_done = true;
                        if let Some(results) = init_results.take() {
                            handle_outbound(&mut stream, results).await?;
                        }
                        if !remaining_bytes.is_empty() {
                            match session.handle_input(&remaining_bytes) {
                                Ok(results) => {
                                    if let Err(e) = process_results(
                                        results,
                                        &mut session,
                                        &mut stream,
                                        &app_state,
                                        &mut session_ctx,
                                    )
                                    .await
                                    {
                                        tracing::error!("process_results after handshake: {:?}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("session input after handshake: {:?}", e);
                                    break;
                                }
                            };
                        }
                        continue;
                    }
                    Err(e) => return Err(anyhow::anyhow!("handshake: {}", e)),
                }
            }

            match session.handle_input(data) {
                Ok(results) => {
                    if let Err(e) = process_results(
                        results,
                        &mut session,
                        &mut stream,
                        &app_state,
                        &mut session_ctx,
                    )
                    .await
                    {
                        tracing::error!("process_results error {}: {:?}", peer_addr, e);
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!("session input error {}: {:?}", peer_addr, e);
                    break;
                }
            }
        }
        Ok(())
    }
    .await;

    // If not a graceful stop, enter grace period for reconnection
    if !session_ctx.graceful_stop {
        enter_grace_period(&app_state, &mut session_ctx).await;
    }

    tracing::debug!("RTMP session ended: {}", peer_addr);
    session_result
}

struct SessionContext {
    current_stream_key: Option<String>,
    current_app: Option<String>,
    hls_state: Option<HlsStreamState>,
    track_states: HashMap<u32, HlsStreamState>,
    recorder: Option<Fmp4Recorder>,
    graceful_stop: bool,
    video_width: u16,
    video_height: u16,
    media_dir: Option<String>,
    hls_segment_duration: u32,
    hls_segments_keep: u32,
    // Per-track codec caches
    track_video_codecs: HashMap<u32, crate::hls::fmp4::VideoCodec>,
    track_audio_codecs: HashMap<u32, crate::hls::fmp4::AudioCodec>,
    // Tracks that have received SequenceEnd and should be ignored
    closed_video_tracks: HashSet<u32>,
    closed_audio_tracks: HashSet<u32>,
    // Track IDs discovered for this stream (used to update PublisherInfo)
    discovered_tracks: HashSet<u32>,
    video_fps_num: u64,
    video_fps_den: u64,
}

impl SessionContext {
    fn new() -> Self {
        Self {
            current_stream_key: None,
            current_app: None,
            hls_state: None,
            track_states: HashMap::new(),
            recorder: None,
            graceful_stop: false,
            video_width: 1920,
            video_height: 1080,
            media_dir: None,
            hls_segment_duration: 0,
            hls_segments_keep: 0,
            track_video_codecs: HashMap::new(),
            track_audio_codecs: HashMap::new(),
            closed_video_tracks: HashSet::new(),
            closed_audio_tracks: HashSet::new(),
            discovered_tracks: HashSet::new(),
            video_fps_num: 30,
            video_fps_den: 1,
        }
    }

    fn get_or_create_track_state(
        &mut self,
        track_id: u32,
        is_audio_only: bool,
    ) -> &mut HlsStreamState {
        let media_dir = self.media_dir.as_ref().unwrap().clone();
        let stream_key = self.current_stream_key.as_ref().unwrap().clone();
        let segment_duration = self.hls_segment_duration;
        let segments_keep = self.hls_segments_keep;
        let fps_num = self.video_fps_num;
        let fps_den = self.video_fps_den;
        let state = self.track_states.entry(track_id).or_insert_with(|| {
            let mut s = HlsStreamState::new(
                &media_dir,
                &stream_key,
                track_id,
                is_audio_only,
                segment_duration,
                segments_keep,
            );
            s.set_video_framerate(fps_num, fps_den);
            s
        });
        // If video data arrives for a track previously created as audio-only,
        // clear the flag so write_video doesn't drop video samples.
        if !is_audio_only {
            state.set_not_audio_only();
        }
        state
    }
}

async fn notify_track_discovered(
    app_state: &Arc<AppState>,
    stream_key: &str,
    track_id: u32,
    video_codec: Option<crate::hls::fmp4::VideoCodec>,
    audio_codec: Option<crate::hls::fmp4::AudioCodec>,
) {
    let mut sm = app_state.stream_manager.write().await;
    if let Some(info) = sm.publishers_mut().get_mut(stream_key) {
        if let Some(existing) = info.tracks.iter_mut().find(|t| t.track_id == track_id) {
            if let Some(vc) = video_codec {
                existing.video_codec = Some(format!("{:?}", vc));
            }
            if let Some(ac) = audio_codec {
                existing.audio_codec = Some(format!("{:?}", ac));
            }
            return;
        }
        let hls_url = if track_id == 0 {
            format!("/hls/{}/index.m3u8", stream_key)
        } else {
            format!("/hls/{}/track_{}/index.m3u8", stream_key, track_id)
        };
        info.tracks.push(crate::rtmp::TrackInfo {
            track_id,
            hls_url,
            video_codec: video_codec.map(|c| format!("{:?}", c)),
            audio_codec: audio_codec.map(|c| format!("{:?}", c)),
        });
    }
}

async fn write_master_playlist(
    media_dir: &str,
    stream_key: &str,
    track_ids: &[u32],
) -> anyhow::Result<()> {
    let stream_dir = PathBuf::from(media_dir).join("hls").join(stream_key);
    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:6\n");

    // Default playlist (index.m3u8) always included if it exists
    let default_playlist = stream_dir.join("index.m3u8");
    if tokio::fs::try_exists(&default_playlist)
        .await
        .unwrap_or(false)
    {
        playlist.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2500000\n");
        playlist.push_str("index.m3u8\n");
    }

    // Include each track playlist
    for track_id in track_ids {
        let track_playlist = stream_dir
            .join(format!("track_{}", track_id))
            .join("index.m3u8");
        if tokio::fs::try_exists(&track_playlist)
            .await
            .unwrap_or(false)
        {
            playlist.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2500000\n");
            playlist.push_str(&format!("track_{}/index.m3u8\n", track_id));
        }
    }

    let master_path = stream_dir.join("master.m3u8");
    tokio::fs::write(&master_path, playlist).await?;
    Ok(())
}

async fn drain_hls_to_recorder(hls: &mut HlsStreamState, recorder: &mut Fmp4Recorder) {
    if let Some(init) = hls.drain_init_data() {
        let _ = recorder.write_init(&init).await;
    }
    for seg in hls.drain_segment_data() {
        let _ = recorder.write_segment(seg).await;
    }
}

async fn finalize_stream(
    app_state: &Arc<AppState>,
    stream_key: &str,
    mut hls: HlsStreamState,
    mut track_states: HashMap<u32, HlsStreamState>,
    mut recorder: Option<Fmp4Recorder>,
    remove_publisher: bool,
) {
    let _ = hls.close().await;
    let mut track_ids: Vec<u32> = Vec::new();
    for (track_id, mut track_state) in track_states.drain() {
        let _ = track_state.close().await;
        track_ids.push(track_id);
    }

    // Generate master playlist
    let media_dir = app_state.config.media_dir.clone();
    if let Err(e) = write_master_playlist(&media_dir, stream_key, &track_ids).await {
        tracing::warn!("Failed to write master playlist for {}: {}", stream_key, e);
    }

    if let Some(ref mut r) = recorder {
        drain_hls_to_recorder(&mut hls, r).await;
        let total_duration_secs: f64 = hls.total_duration_secs();
        let total_duration = if total_duration_secs > 0.0 {
            Some(total_duration_secs.round() as u64)
        } else {
            None
        };
        if let Ok(mp4_path) = r.close().await {
            let sizes = app_state.config.thumbnail_sizes.clone();
            let base_url = app_state.config.recordings_base_url.clone();
            let key = stream_key.to_string();
            let app_state = Arc::clone(app_state);
            let remux_path = mp4_path.clone();
            let filename = mp4_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
                .unwrap_or_default();
            tokio::spawn(async move {
                let thumb_dir = PathBuf::from(&media_dir)
                    .join("thumbnails")
                    .join("recordings");
                if let Err(e) =
                    crate::thumbnail::generate_thumbnails_for_file(&mp4_path, &thumb_dir, &sizes)
                        .await
                {
                    tracing::warn!(
                        "Post-recording thumbnail generation failed for {}: {}",
                        key,
                        e
                    );
                }
                if let Err(e) = crate::recording::update_index_json(
                    &media_dir,
                    &filename,
                    &key,
                    total_duration,
                    &base_url,
                    &sizes,
                )
                .await
                {
                    tracing::warn!("update_index_json failed for {}: {}", key, e);
                }
                // Background remux (non-blocking, concurrency-limited)
                app_state.remux_queue.enqueue(remux_path);
                // Only clean up HLS files if no new publisher has taken over
                let sm = app_state.stream_manager.read().await;
                let still_in_use = sm.is_live_or_pending(&key);
                drop(sm);
                if still_in_use {
                    tracing::debug!(
                        "Skipping HLS cleanup for {} — stream is still live or in grace period",
                        key
                    );
                } else {
                    let hls_dir = PathBuf::from(&media_dir).join("hls").join(&key);
                    let _ = tokio::fs::remove_dir_all(&hls_dir).await;
                    let stream_thumb_dir =
                        PathBuf::from(&media_dir).join("thumbnails").join("streams");
                    for &w in &sizes {
                        for ext in &["jxl", "avif", "png"] {
                            let _ = tokio::fs::remove_file(
                                stream_thumb_dir.join(format!("{}_w{}.{}", key, w, ext)),
                            ).await;
                        }
                    }
                }
            });
        }
    }
    if remove_publisher {
        let mut sm = app_state.stream_manager.write().await;
        sm.remove_publisher(stream_key);
    }
}

async fn finalize_session(app_state: &Arc<AppState>, ctx: &mut SessionContext) {
    if let Some(ref key) = ctx.current_stream_key.take() {
        let track_states = std::mem::take(&mut ctx.track_states);
        if let Some(hls) = ctx.hls_state.take() {
            let recorder = ctx.recorder.take();
            finalize_stream(app_state, key, hls, track_states, recorder, true).await;
        } else if let Some(mut r) = ctx.recorder.take() {
            let _ = r.close().await;
            let mut sm = app_state.stream_manager.write().await;
            sm.remove_publisher(key);
        } else {
            let mut sm = app_state.stream_manager.write().await;
            sm.remove_publisher(key);
        }
    } else if let Some(mut r) = ctx.recorder.take() {
        let _ = r.close().await;
    }
}

async fn enter_grace_period(app_state: &Arc<AppState>, ctx: &mut SessionContext) {
    // Close all track states immediately (HLS only; not needed for recording grace period)
    let mut track_ids: Vec<u32> = Vec::new();
    for (track_id, mut track_state) in ctx.track_states.drain() {
        let _ = track_state.close().await;
        track_ids.push(track_id);
    }
    // Generate master playlist with whatever we have so far
    if let Some(ref key) = ctx.current_stream_key {
        let media_dir = app_state.config.media_dir.clone();
        if let Err(e) = write_master_playlist(&media_dir, key, &track_ids).await {
            tracing::warn!(
                "Failed to write master playlist during grace period for {}: {}",
                key,
                e
            );
        }
    }

    if let Some(ref key) = ctx.current_stream_key.clone() {
        if let Some(mut hls) = ctx.hls_state.take() {
            let _ = hls.prepare_for_grace_period().await;
            if let Some(ref mut recorder) = ctx.recorder {
                drain_hls_to_recorder(&mut hls, recorder).await;
            }
            let recorder = ctx.recorder.take();
            {
                let mut sm = app_state.stream_manager.write().await;
                sm.mark_disconnected(key, hls, recorder);
            }

            let app_state_clone = app_state.clone();
            let key_clone = key.clone();
            let grace_period = app_state.config.stream_grace_period_seconds;
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(grace_period)).await;

                let pending = {
                    let mut sm = app_state_clone.stream_manager.write().await;
                    sm.remove_pending_stream(&key_clone)
                };

                if let Some(pending) = pending {
                    finalize_stream(
                        &app_state_clone,
                        &key_clone,
                        pending.hls_state,
                        HashMap::new(),
                        pending.recorder,
                        true,
                    )
                    .await;
                }
            });
        } else if let Some(mut r) = ctx.recorder.take() {
            let _ = r.close().await;
            let mut sm = app_state.stream_manager.write().await;
            sm.remove_publisher(key);
        } else {
            let mut sm = app_state.stream_manager.write().await;
            sm.remove_publisher(key);
        }
    } else if let Some(mut r) = ctx.recorder.take() {
        let _ = r.close().await;
    }
}

async fn handle_outbound(
    stream: &mut TcpStream,
    results: Vec<ServerSessionResult>,
) -> anyhow::Result<()> {
    for r in results {
        if let ServerSessionResult::OutboundResponse(pkt) = r {
            stream.write_all(&pkt.bytes).await?;
            stream.flush().await?;
        }
    }
    Ok(())
}

async fn process_results(
    results: Vec<ServerSessionResult>,
    session: &mut ServerSession,
    stream: &mut TcpStream,
    app_state: &Arc<AppState>,
    ctx: &mut SessionContext,
) -> anyhow::Result<()> {
    for result in results {
        match result {
            ServerSessionResult::OutboundResponse(pkt) => {
                stream.write_all(&pkt.bytes).await?;
                stream.flush().await?;
            }
            ServerSessionResult::RaisedEvent(event) => {
                handle_event(session, stream, app_state, ctx, event).await?;
            }
            ServerSessionResult::UnhandleableMessageReceived(msg) => {
                tracing::warn!("Unhandled msg: type={}", msg.type_id);
            }
        }
    }
    Ok(())
}

async fn handle_event(
    session: &mut ServerSession,
    stream: &mut TcpStream,
    app_state: &Arc<AppState>,
    ctx: &mut SessionContext,
    event: ServerSessionEvent,
) -> anyhow::Result<()> {
    match event {
        ServerSessionEvent::ConnectionRequested {
            request_id,
            app_name,
        } => {
            tracing::debug!("RTMP connect: app={}", app_name);
            ctx.current_app = Some(app_name);
            let responses = session
                .accept_request(request_id)
                .map_err(|e| anyhow::anyhow!("accept: {}", e))?;
            handle_outbound(stream, responses).await?;
        }

        ServerSessionEvent::ReleaseStreamRequested { request_id, .. } => {
            handle_outbound(
                stream,
                session
                    .accept_request(request_id)
                    .map_err(|e| anyhow::anyhow!("release: {}", e))?,
            )
            .await?;
        }

        ServerSessionEvent::PublishStreamRequested {
            request_id,
            stream_key,
            ..
        } => {
            tracing::debug!("Publish request: stream_key={}", stream_key);
            let responses = session
                .accept_request(request_id)
                .map_err(|e| anyhow::anyhow!("publish accept: {}", e))?;
            tracing::debug!(
                "Sending {} outbound responses for publish accept",
                responses.len()
            );
            handle_outbound(stream, responses).await?;

            let mut sm = app_state.stream_manager.write().await;
            if let Some(pending) = sm.reconnect(&stream_key) {
                // Reconnect within grace period
                drop(sm);
                ctx.hls_state = Some(pending.hls_state);
                ctx.recorder = pending.recorder;
                ctx.current_stream_key = Some(stream_key.clone());
                tracing::info!("Stream {} reconnected within grace period", stream_key);
            } else {
                sm.add_publisher(
                    &stream_key,
                    crate::rtmp::PublisherInfo {
                        stream_key: stream_key.clone(),
                        app_name: ctx.current_app.clone().unwrap_or_default(),
                        started_at: chrono::Utc::now(),
                        metadata: None,
                        tracks: Vec::new(),
                        disconnected_at: None,
                    },
                );
                drop(sm);

                let media_dir = app_state.config.media_dir.clone();
                let hls_dir = std::path::PathBuf::from(&media_dir)
                    .join("hls")
                    .join(&stream_key);
                let _ = tokio::fs::create_dir_all(&hls_dir).await;
                ctx.media_dir = Some(media_dir.clone());
                ctx.hls_segment_duration = app_state.config.hls_segment_duration;
                ctx.hls_segments_keep = app_state.config.hls_segments_keep;
                ctx.hls_state = Some(HlsStreamState::new(
                    &media_dir,
                    &stream_key,
                    0,
                    false,
                    app_state.config.hls_segment_duration,
                    app_state.config.hls_segments_keep,
                ));
                ctx.track_states.clear();
                if app_state.config.recording_enabled {
                    let recordings_dir = std::path::PathBuf::from(&media_dir).join("recordings");
                    let _ = tokio::fs::create_dir_all(&recordings_dir).await;
                    ctx.recorder = Some(Fmp4Recorder::new(&media_dir, &stream_key));
                }
                ctx.current_stream_key = Some(stream_key.clone());

                tracing::info!("Stream {} is now live", stream_key);
            }
        }

        ServerSessionEvent::PublishStreamFinished { stream_key, .. } => {
            tracing::info!("Publish finished: {}", stream_key);
            ctx.graceful_stop = true;
            finalize_session(app_state, ctx).await;
        }

        ServerSessionEvent::VideoDataReceived {
            stream_key,
            data,
            timestamp,
            ..
        } => {
            if let Some(ref key) = ctx.current_stream_key
                && *key == stream_key
            {
                let ts = timestamp.value;
                handle_video_data(&data, ts, ctx, app_state, &stream_key).await;
            }
        }

        ServerSessionEvent::AudioDataReceived {
            stream_key,
            data,
            timestamp,
            ..
        } => {
            if let Some(ref key) = ctx.current_stream_key
                && *key == stream_key
            {
                let ts = timestamp.value;
                handle_audio_data(&data, ts, ctx, app_state, &stream_key).await;
            }
        }

        ServerSessionEvent::StreamMetadataChanged { metadata, .. } => {
            let width = metadata.video_width.unwrap_or(1920) as u16;
            let height = metadata.video_height.unwrap_or(1080) as u16;
            ctx.video_width = width;
            ctx.video_height = height;
            if let Some(ref mut hls) = ctx.hls_state {
                let _ = hls.update_video_resolution(width, height).await;
            }
            // Use codec names discovered from packet data rather than raw codec IDs
            let video_codec_name = ctx
                .track_video_codecs
                .get(&0)
                .map(|c| match c {
                    crate::hls::fmp4::VideoCodec::H264 => "H264".to_string(),
                    crate::hls::fmp4::VideoCodec::H265 => "HEVC".to_string(),
                    crate::hls::fmp4::VideoCodec::AV1 => "AV1".to_string(),
                })
                .unwrap_or_else(|| match metadata.video_codec_id {
                    Some(7) => "H264".to_string(),
                    Some(12) => "HEVC".to_string(),
                    Some(13) => "AV1".to_string(),
                    Some(0x61766331) => "H264".to_string(), // "avc1" FourCC
                    Some(0x68657631) => "HEVC".to_string(), // "hev1" FourCC
                    Some(0x68766331) => "HEVC".to_string(), // "hvc1" FourCC
                    Some(0x61763031) => "AV1".to_string(),  // "av01" FourCC
                    Some(0x76703039) => "VP9".to_string(),  // "vp09" FourCC
                    Some(0x76766331) => "VVC".to_string(),  // "vvc1" FourCC
                    Some(id) => format!("{}", id),
                    None => String::new(),
                });
            let audio_codec_name = ctx
                .track_audio_codecs
                .get(&0)
                .map(|c| match c {
                    crate::hls::fmp4::AudioCodec::Aac => "AAC".to_string(),
                    crate::hls::fmp4::AudioCodec::Opus => "Opus".to_string(),
                    crate::hls::fmp4::AudioCodec::Flac => "FLAC".to_string(),
                })
                .unwrap_or_else(|| match metadata.audio_codec_id {
                    Some(0) => "Linear PCM".to_string(),
                    Some(2) => "MP3".to_string(),
                    Some(10) => "AAC".to_string(),
                    Some(11) => "Speex".to_string(),
                    Some(0x4F707573) => "Opus".to_string(), // "Opus" FourCC
                    Some(0x664C6143) => "FLAC".to_string(), // "fLaC" FourCC
                    Some(0x61632D33) => "AC-3".to_string(), // "ac-3" FourCC
                    Some(0x65632D33) => "E-AC-3".to_string(), // "ec-3" FourCC
                    Some(id) => format!("{}", id),
                    None => String::new(),
                });
            let fps_float = metadata.video_frame_rate.unwrap_or(0.0) as f64;
            let (fps_num, fps_den) = fps_to_rational(fps_float);
            ctx.video_fps_num = fps_num;
            ctx.video_fps_den = fps_den;
            if let Some(ref mut hls) = ctx.hls_state {
                hls.set_video_framerate(fps_num, fps_den);
            }
            for track_state in ctx.track_states.values_mut() {
                track_state.set_video_framerate(fps_num, fps_den);
            }
            let meta = crate::rtmp::StreamMeta {
                width: metadata.video_width.unwrap_or(0),
                height: metadata.video_height.unwrap_or(0),
                video_codec: video_codec_name,
                audio_codec: audio_codec_name,
                video_bitrate: metadata.video_bitrate_kbps.unwrap_or(0),
                audio_bitrate: metadata.audio_bitrate_kbps.unwrap_or(0),
                framerate: fps_float,
            };
            if let Some(ref key) = ctx.current_stream_key {
                let mut sm = app_state.stream_manager.write().await;
                if let Some(ref mut pi) = sm.publishers_mut().get_mut(key) {
                    pi.metadata = Some(meta);
                }
            }
        }

        ServerSessionEvent::PlayStreamRequested { request_id, .. } => {
            handle_outbound(
                stream,
                session
                    .accept_request(request_id)
                    .map_err(|e| anyhow::anyhow!("play accept: {}", e))?,
            )
            .await?;
        }

        _ => {}
    }
    Ok(())
}

fn map_enhanced_video_codec(
    codec: crate::rtmp::enhanced::EnhancedVideoCodec,
) -> Option<crate::hls::fmp4::VideoCodec> {
    match codec {
        crate::rtmp::enhanced::EnhancedVideoCodec::Av1 => Some(crate::hls::fmp4::VideoCodec::AV1),
        crate::rtmp::enhanced::EnhancedVideoCodec::Avc => Some(crate::hls::fmp4::VideoCodec::H264),
        crate::rtmp::enhanced::EnhancedVideoCodec::Hevc => Some(crate::hls::fmp4::VideoCodec::H265),
        _ => None,
    }
}

fn map_enhanced_audio_codec(
    codec: crate::rtmp::enhanced::EnhancedAudioCodec,
) -> Option<crate::hls::fmp4::AudioCodec> {
    match codec {
        crate::rtmp::enhanced::EnhancedAudioCodec::Opus => Some(crate::hls::fmp4::AudioCodec::Opus),
        crate::rtmp::enhanced::EnhancedAudioCodec::Flac => Some(crate::hls::fmp4::AudioCodec::Flac),
        crate::rtmp::enhanced::EnhancedAudioCodec::Aac => Some(crate::hls::fmp4::AudioCodec::Aac),
        _ => None,
    }
}

fn fps_to_rational(fps: f64) -> (u64, u64) {
    // Match common NTSC/PAL frame rates as exact rationals.
    // The RTMP metadata often gives 29.97, 59.94, 23.976 as floats;
    // converting these directly to fractions keeps frame durations exact.
    let known: &[(f64, u64, u64)] = &[
        (23.976, 24000, 1001),
        (23.98, 24000, 1001),
        (24.0, 24, 1),
        (25.0, 25, 1),
        (29.97, 30000, 1001),
        (30.0, 30, 1),
        (48.0, 48, 1),
        (50.0, 50, 1),
        (59.94, 60000, 1001),
        (60.0, 60, 1),
        (120.0, 120, 1),
    ];
    for (ref_fps, num, den) in known {
        if (fps - ref_fps).abs() < 0.01 {
            return (*num, *den);
        }
    }
    // Default: round to nearest integer
    let rounded = (fps + 0.5).floor() as u64;
    if rounded > 0 { (rounded, 1) } else { (30, 1) }
}

/// Parse color config from Enhanced RTMP video metadata remainder.
/// Supports two wire formats:
///
/// **Binary** (FFmpeg): after fourcc → color_primaries(u8)
///   transfer_characteristics(u8) matrix_coefficients(u8) full_range(u8)
///
/// **AMF0** (OBS/veovera spec): after fourcc → AMF0 `colorInfo` object
///   with nested `colorConfig` / `hdrCll` / `hdrMdcv` objects.
fn parse_enhanced_color_config(data: &[u8]) -> crate::hls::fmp4::ColorConfig {
    // Try AMF0 parsing first
    if let Some(cfg) = try_parse_amf_color_config(data) {
        return cfg;
    }
    // Fall back to binary (FFmpeg) format
    let cp = data.first().copied().unwrap_or(1) as u16;
    let tc = data.get(1).copied().unwrap_or(1) as u16;
    let mc = data.get(2).copied().unwrap_or(1) as u16;
    let full_range = data.get(3).copied().unwrap_or(0) != 0;
    crate::hls::fmp4::ColorConfig {
        color_primaries: cp,
        transfer_characteristics: tc,
        matrix_coefficients: mc,
        full_range,
    }
}

/// Try to parse HDR metadata from Enhanced RTMP video metadata remainder.
pub fn parse_enhanced_hdr_metadata(data: &[u8]) -> Option<crate::hls::fmp4::HdrMetadata> {
    // Try AMF0 first
    if let Some(hdr) = try_parse_amf_hdr_metadata(data) {
        return Some(hdr);
    }
    // Fall back to binary format: u32 size prefix + CLLI(4) + MDCV(24)
    if data.len() < 36 { return None; }
    let hdr_data = &data[4..];
    if hdr_data.len() < 4 { return None; }
    let hdr_len = u32::from_be_bytes([hdr_data[0], hdr_data[1], hdr_data[2], hdr_data[3]]) as usize;
    let payload = hdr_data.get(4..)?;
    if hdr_len < 28 || payload.len() < hdr_len.min(28) { return None; }
    let p = payload;
    let maxcll = u16::from_be_bytes([p[0], p[1]]) as u32;
    let maxfall = u16::from_be_bytes([p[2], p[3]]) as u32;
    let mdcv_data = &p[4..];
    if mdcv_data.len() < 24 { return None; }
    Some(crate::hls::fmp4::HdrMetadata {
        max_content_light_level: maxcll,
        max_frame_average_light_level: maxfall,
        display_primaries_x: [
            u16::from_be_bytes([mdcv_data[0], mdcv_data[1]]),
            u16::from_be_bytes([mdcv_data[2], mdcv_data[3]]),
            u16::from_be_bytes([mdcv_data[4], mdcv_data[5]]),
        ],
        display_primaries_y: [
            u16::from_be_bytes([mdcv_data[6], mdcv_data[7]]),
            u16::from_be_bytes([mdcv_data[8], mdcv_data[9]]),
            u16::from_be_bytes([mdcv_data[10], mdcv_data[11]]),
        ],
        white_point_x: u16::from_be_bytes([mdcv_data[12], mdcv_data[13]]),
        white_point_y: u16::from_be_bytes([mdcv_data[14], mdcv_data[15]]),
        max_luminance: u32::from_be_bytes([mdcv_data[16], mdcv_data[17], mdcv_data[18], mdcv_data[19]]),
        min_luminance: u32::from_be_bytes([mdcv_data[20], mdcv_data[21], mdcv_data[22], mdcv_data[23]]),
    })
}

// ── AMF0 helpers for Enhanced RTMP colorInfo ──────────────────────

/// Minimal AMF0 number reader: returns Some(value, bytes_consumed).
fn amf_read_number(data: &[u8]) -> Option<(f64, usize)> {
    if data.len() < 9 || data[0] != 0x00 { return None; }
    let bits = u64::from_be_bytes(data[1..9].try_into().ok()?);
    Some((f64::from_bits(bits), 9))
}

/// AMF0 ECMA Array reader: returns fields as (key, value_bytes).
/// ECMA Array marker (0x08) + u32 count + key-value pairs + 0x000009 terminator.
fn amf_next_value(data: &[u8]) -> Option<(u8, &[u8], usize)> {
    if data.is_empty() { return None; }
    let marker = data[0];
    let _consumed = 1;
    match marker {
        0x00 => { // Number
            let (_, n) = amf_read_number(data)?;
            Some((marker, &data[1..9], n))
        }
        0x01 => { // Boolean
            Some((marker, &data[1..2], 2))
        }
        0x02 => { // String
            if data.len() < 3 { return None; }
            let len = u16::from_be_bytes([data[1], data[2]]) as usize;
            if data.len() < 3 + len { return None; }
            Some((marker, &data[3..3+len], 3 + len))
        }
        0x05 | 0x06 => { // Null / Undefined
            Some((marker, &[], 1))
        }
        0x08 => { // ECMA Array
            if data.len() < 5 { return None; }
            let _count = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            // Consume key-value pairs until empty string + 0x09 terminator
            let mut off = 5;
            loop {
                if off + 3 > data.len() { return None; }
                let klen = u16::from_be_bytes([data[off], data[off+1]]) as usize;
                if klen == 0 && off + 2 < data.len() && data[off+2] == 0x09 {
                    off += 3; break; // 0x0009 terminator
                }
                if off + 2 + klen >= data.len() { return None; }
                off += 2 + klen;
                // value
                let val_marker = data[off];
                if val_marker == 0x00 { off += 9; }
                else if val_marker == 0x01 { off += 2; }
                else if val_marker == 0x02 { let slen = u16::from_be_bytes([data[off+1], data[off+2]]) as usize; off += 3 + slen; }
                else if val_marker == 0x03 || val_marker == 0x08 { off = skip_amf_object(data, off)?; }
                else if val_marker == 0x0A { off = skip_amf_strict_array(data, off)?; }
                else if val_marker == 0x05 || val_marker == 0x06 { off += 1; }
                else { return None; }
            }
            Some((marker, &data[5..off], off))
        }
        0x03 => { // Object
            let mut off = 1;
            off = skip_amf_object(data, off)?;
            Some((marker, &data[1..off], off))
        }
        0x0A => { // Strict Array
            if data.len() < 5 { return None; }
            let _count = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            let mut off = 5;
            let mut remaining = _count as usize;
            while remaining > 0 && off < data.len() {
                let (_, _, n) = amf_next_value(&data[off..])?;
                off += n;
                remaining -= 1;
            }
            Some((marker, &data[5..off], off))
        }
        _ => None,
    }
}

/// Skip past an AMF0 Object (0x03): key-value pairs terminated by 0x0009.
fn skip_amf_object(data: &[u8], start: usize) -> Option<usize> {
    let mut off = start;
    loop {
        if off + 3 > data.len() { return None; }
        let klen = u16::from_be_bytes([data[off], data[off+1]]) as usize;
        if klen == 0 && off + 2 < data.len() && data[off+2] == 0x09 {
            off += 3; break;
        }
        if off + 2 + klen >= data.len() { return None; }
        off += 2 + klen;
        let (_, _, n) = amf_next_value(&data[off..])?;
        off += n;
    }
    Some(off)
}

/// Skip past an AMF0 Strict Array (0x0A).
fn skip_amf_strict_array(data: &[u8], start: usize) -> Option<usize> {
    if start + 4 >= data.len() { return None; }
    let count = u32::from_be_bytes([data[start], data[start+1], data[start+2], data[start+3]]) as usize;
    let mut off = start + 4;
    for _ in 0..count {
        if off >= data.len() { return None; }
        let (_, _, n) = amf_next_value(&data[off..])?;
        off += n;
    }
    Some(off)
}

/// Look up a field by name inside an AMF0 ECMA Array or Object at the given offset.
fn amf_lookup<'a>(data: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let (_, _, total) = amf_next_value(data)?;
    let marker = data[0];
    let body = &data[1..total]; // skip the marker byte
    if marker != 0x03 && marker != 0x08 { return None; }
    if marker == 0x08 {
        // ECMA Array: skip 4-byte count at start of body
        if body.len() < 4 { return None; }
        let mut off = 4;
        loop {
            if off + 2 > body.len() { return None; }
            let klen = u16::from_be_bytes([body[off], body[off+1]]) as usize;
            if klen == 0 && off + 2 < body.len() && body[off+2] == 0x09 { break; }
            if off + 2 + klen > body.len() { return None; }
            let key = std::str::from_utf8(&body[off+2..off+2+klen]).ok();
            off += 2 + klen;
            if off >= body.len() { return None; }
            let _val_marker = body[off];
            if key == Some(name) {
                // Return value including its type marker
                let (_, _, vlen) = amf_next_value(&body[off..])?;
                return Some(&body[off..off + vlen]);
            }
            // Skip value
            let (_, _, vlen) = amf_next_value(&body[off..])?;
            off += vlen;
        }
        None
    } else {
        // Object: no count prefix
        let mut off = 0;
        loop {
            if off + 2 > body.len() { return None; }
            let klen = u16::from_be_bytes([body[off], body[off+1]]) as usize;
            if klen == 0 && off + 2 < body.len() && body[off+2] == 0x09 { break; }
            if off + 2 + klen > body.len() { return None; }
            let key = std::str::from_utf8(&body[off+2..off+2+klen]).ok();
            off += 2 + klen;
            if off >= body.len() { return None; }
            if key == Some(name) {
                // Return value including its type marker
                let (_, _, vlen) = amf_next_value(&body[off..])?;
                return Some(&body[off..off + vlen]);
            }
            // Skip value
            let (_, _, vlen) = amf_next_value(&body[off..])?;
            off += vlen;
        }
        None
    }
}

fn try_parse_amf_color_config(data: &[u8]) -> Option<crate::hls::fmp4::ColorConfig> {
    let (first_marker, _, consumed) = amf_next_value(data)?;
    if first_marker != 0x02 { return None; }
    let obj = &data[consumed..];
    let (omarker, _, _) = amf_next_value(obj)?;
    if omarker != 0x03 && omarker != 0x08 { return None; }
    let cc = amf_lookup(obj, "colorConfig")?;
    let read_num_field = |name: &str| -> Option<f64> {
        let v = amf_lookup(cc, name)?;
        amf_read_number(v).map(|r| r.0)
    };
    // FFmpeg sends only matrixCoefficients; colorPrimaries and transferCharacteristics
    // may be absent — make them optional.
    let cp = read_num_field("colorPrimaries")
        .or_else(|| read_num_field("ColorPrimaries"))
        .or_else(|| read_num_field("color_primaries"))
        .unwrap_or(0.0) as u16;
    let tc = read_num_field("transferCharacteristics")
        .or_else(|| read_num_field("TransferCharacteristics"))
        .or_else(|| read_num_field("transfer_characteristics"))
        .unwrap_or(0.0) as u16;
    let mc = read_num_field("matrixCoefficients")
        .or_else(|| read_num_field("MatrixCoefficients"))
        .or_else(|| read_num_field("matrix_coefficients"))? as u16;
    let fr = read_num_field("fullRange")
        .or_else(|| read_num_field("FullRange"))
        .or_else(|| read_num_field("full_range"))
        .unwrap_or(0.0) as u8 != 0;
    Some(crate::hls::fmp4::ColorConfig {
        color_primaries: cp,
        transfer_characteristics: tc,
        matrix_coefficients: mc,
        full_range: fr,
    })
}

fn try_parse_amf_hdr_metadata(data: &[u8]) -> Option<crate::hls::fmp4::HdrMetadata> {
    let (first_marker, _, consumed) = amf_next_value(data)?;
    if first_marker != 0x02 { return None; }
    let obj = &data[consumed..];
    let cll = amf_lookup(obj, "hdrCll")?;
    let mdcv = amf_lookup(obj, "hdrMdcv")?;

    let read_num = |obj: &[u8], name: &str| -> Option<f64> {
        let v = amf_lookup(obj, name)?;
        amf_read_number(v).map(|r| r.0)
    };
    // hdrCll: maxCll, maxFall
    let maxcll = read_num(cll, "maxCll")? as u32;
    let maxfall = read_num(cll, "maxFall")? as u32;

    // hdrMdcv: displayPrimariesX[3], displayPrimariesY[3], whitePointX, whitePointY, maxLuminance, minLuminance
    // Fields are AMF0 Number scalars (or Arrays in some implementations — handle both)
    let read_primaries = |obj: &[u8], name: &str| -> Option<[u16; 3]> {
        let v = amf_lookup(obj, name)?;
        // Could be Strict Array (0x0A) of 3 numbers, or comma-separated string, or just 3 separate fields
        if !v.is_empty() && v[0] == 0x0A {
            // Strict Array: 0x0A + u32 count + 3 Numbers
            let mut arr = [0u16; 3];
            arr[0] = amf_read_number(&v[5..]).map(|r| r.0 as u16)?;
            arr[1] = amf_read_number(&v[14..]).map(|r| r.0 as u16)?;
            arr[2] = amf_read_number(&v[23..]).map(|r| r.0 as u16)?;
            Some(arr)
        } else {
            // Separate fields
            let a = read_num(obj, name)? as u16;
            Some([a, 0, 0])
        }
    };

    let display_primaries_x = read_primaries(mdcv, "displayPrimariesX")?;
    let display_primaries_y = read_primaries(mdcv, "displayPrimariesY")?;
    let white_point_x = read_num(mdcv, "whitePointX")? as u16;
    let white_point_y = read_num(mdcv, "whitePointY")? as u16;
    let max_luminance = read_num(mdcv, "maxLuminance")? as u32;
    let min_luminance = read_num(mdcv, "minLuminance")? as u32;

    Some(crate::hls::fmp4::HdrMetadata {
        max_content_light_level: maxcll,
        max_frame_average_light_level: maxfall,
        display_primaries_x,
        display_primaries_y,
        white_point_x,
        white_point_y,
        max_luminance,
        min_luminance,
    })
}

async fn handle_video_data(
    data: &[u8],
    ts: u32,
    ctx: &mut SessionContext,
    app_state: &Arc<AppState>,
    stream_key: &str,
) {
    tracing::debug!(
        "handle_video_data: ts={}, len={}, first_bytes={:02x?}, hls_state={}",
        ts,
        data.len(),
        &data[..data.len().min(8)],
        ctx.hls_state.is_some()
    );
    if crate::rtmp::enhanced::is_enhanced_video(data) {
        if let Ok((header, remainder)) = crate::rtmp::enhanced::parse_enhanced_video_header(data) {
            let is_keyframe = header.frame_type == crate::rtmp::enhanced::VideoFrameType::KeyFrame;
            match header.packet_type {
                crate::rtmp::enhanced::VideoPacketType::Multitrack => {
                    match crate::rtmp::enhanced::parse_enhanced_video_multitrack(remainder) {
                        Ok((_multitrack_type, inner_pt, tracks)) => {
                            tracing::debug!(
                                "Parsed video multitrack: {} tracks, inner_pt={:?}",
                                tracks.len(),
                                inner_pt
                            );
                            for track in tracks.iter() {
                                if ctx.closed_video_tracks.contains(&track.track_id) {
                                    continue;
                                }
                                let Some(codec) = map_enhanced_video_codec(track.codec.clone())
                                else {
                                    tracing::warn!(
                                        "Unsupported video codec {:?}, skipping track {}",
                                        track.codec,
                                        track.track_id
                                    );
                                    continue;
                                };
                                let old_video_codec =
                                    ctx.track_video_codecs.insert(track.track_id, codec);
                                let is_new_video_codec =
                                    old_video_codec.is_none() || old_video_codec != Some(codec);
                                let is_new_track = !ctx.discovered_tracks.contains(&track.track_id);
                                if is_new_track || is_new_video_codec {
                                    if is_new_track {
                                        ctx.discovered_tracks.insert(track.track_id);
                                    }
                                    let audio_codec =
                                        ctx.track_audio_codecs.get(&track.track_id).copied();
                                    notify_track_discovered(
                                        app_state,
                                        stream_key,
                                        track.track_id,
                                        Some(codec),
                                        audio_codec,
                                    )
                                    .await;
                                }
                                let video_width = ctx.video_width;
                                let video_height = ctx.video_height;
                                // Default hls_state gets track_id == 0 as primary track
                                if track.track_id == 0
                                    && let Some(ref mut hls) = ctx.hls_state
                                {
                                    match inner_pt {
                                        Some(
                                            crate::rtmp::enhanced::VideoPacketType::SequenceStart,
                                        ) => {
                                            let _ = hls
                                                .set_video_config(
                                                    track.payload,
                                                    codec,
                                                    video_width,
                                                    video_height,
                                                )
                                                .await;
                                        }
                                        Some(
                                            crate::rtmp::enhanced::VideoPacketType::SequenceEnd,
                                        ) => {
                                            let _ = hls.finalize_segment().await;
                                        }
                                        _ => {
                                            let _ = hls
                                                .write_video(
                                                    track.payload,
                                                    ts,
                                                    is_keyframe,
                                                    track.composition_time_offset,
                                                )
                                                .await;
                                        }
                                    }
                                }
                                // Every track gets its own track state
                                let track_state =
                                    ctx.get_or_create_track_state(track.track_id, false);
                                match inner_pt {
                                    Some(crate::rtmp::enhanced::VideoPacketType::SequenceStart) => {
                                        let _ = track_state
                                            .set_video_config(
                                                track.payload,
                                                codec,
                                                video_width,
                                                video_height,
                                            )
                                            .await;
                                    }
                                    Some(crate::rtmp::enhanced::VideoPacketType::SequenceEnd) => {
                                        let _ = track_state.finalize_segment().await;
                                        ctx.closed_video_tracks.insert(track.track_id);
                                    }
                                    _ => {
                                        let _ = track_state
                                            .write_video(
                                                track.payload,
                                                ts,
                                                is_keyframe,
                                                track.composition_time_offset,
                                            )
                                            .await;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse video multitrack: {}", e);
                        }
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::SequenceStart => {
                    let Some(codec) = map_enhanced_video_codec(header.codec.clone()) else {
                        tracing::warn!(
                            "Unsupported video codec {:?}, dropping SequenceStart",
                            header.codec
                        );
                        return;
                    };
                    let old_video_codec = ctx.track_video_codecs.insert(0, codec);
                    let is_new_video_codec =
                        old_video_codec.is_none() || old_video_codec != Some(codec);
                    let is_new_track = !ctx.discovered_tracks.contains(&0);
                    if is_new_track || is_new_video_codec {
                        if is_new_track {
                            ctx.discovered_tracks.insert(0);
                        }
                        let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                        notify_track_discovered(app_state, stream_key, 0, Some(codec), audio_codec)
                            .await;
                    }
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls
                            .set_video_config(remainder, codec, ctx.video_width, ctx.video_height)
                            .await;
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::CodedFrames
                | crate::rtmp::enhanced::VideoPacketType::CodedFramesX => {
                    if !ctx.discovered_tracks.contains(&0) {
                        ctx.discovered_tracks.insert(0);
                        let video_codec = ctx.track_video_codecs.get(&0).copied();
                        let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                        notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec)
                            .await;
                    }
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls
                            .write_video(remainder, ts, is_keyframe, header.composition_time_offset)
                            .await;
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::SequenceEnd => {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.finalize_segment().await;
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::Metadata => {
                    if let Some(ref mut hls) = ctx.hls_state {
                        hls.set_video_color_config(parse_enhanced_color_config(remainder));
                        if let Some(hdr) = parse_enhanced_hdr_metadata(remainder) {
                            hls.set_hdr_metadata(hdr);
                        }
                    }
                    for track_state in ctx.track_states.values_mut() {
                        track_state.set_video_color_config(parse_enhanced_color_config(remainder));
                        if let Some(hdr) = parse_enhanced_hdr_metadata(remainder) {
                            track_state.set_hdr_metadata(hdr);
                        }
                    }
                }
                _ => {}
            }
        }
    } else if !data.is_empty() {
        // Legacy FLV video tag
        let frame_type = (data[0] & 0xF0) >> 4;
        let codec_id = data[0] & 0x0F;
        let is_keyframe = frame_type == 1;
        tracing::debug!(
            "legacy video: frame_type={}, codec_id={}, len={}, hls_state={}",
            frame_type,
            codec_id,
            data.len(),
            ctx.hls_state.is_some()
        );

        if codec_id == 7 && data.len() >= 2 {
            let avc_packet_type = data[1];
            let remainder = if data.len() >= 5 { &data[5..] } else { &[] };
            tracing::debug!(
                "avc_packet_type={}, remainder_len={}",
                avc_packet_type,
                remainder.len()
            );

            if avc_packet_type == 0 {
                // AVC sequence header -> avcC config
                ctx.track_video_codecs
                    .insert(0, crate::hls::fmp4::VideoCodec::H264);
                if !ctx.discovered_tracks.contains(&0) {
                    ctx.discovered_tracks.insert(0);
                }
                let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                notify_track_discovered(
                    app_state,
                    stream_key,
                    0,
                    Some(crate::hls::fmp4::VideoCodec::H264),
                    audio_codec,
                )
                .await;
                if let Some(ref mut hls) = ctx.hls_state
                    && let Err(e) = hls
                        .set_video_config(
                            remainder,
                            crate::hls::fmp4::VideoCodec::H264,
                            ctx.video_width,
                            ctx.video_height,
                        )
                        .await
                {
                    tracing::warn!("HLS set_video_config failed: {}", e);
                }
            } else if avc_packet_type == 1 {
                // AVC NALU -> raw AVCC sample data
                let composition_time_offset = if data.len() >= 5 {
                    let ct_bytes: [u8; 3] = [data[2], data[3], data[4]];
                    i32::from_be_bytes([
                        if ct_bytes[0] & 0x80 != 0 { 0xFF } else { 0x00 },
                        ct_bytes[0],
                        ct_bytes[1],
                        ct_bytes[2],
                    ])
                } else {
                    0
                };
                if !ctx.discovered_tracks.contains(&0) {
                    ctx.discovered_tracks.insert(0);
                    let video_codec = ctx.track_video_codecs.get(&0).copied();
                    let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                    notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec)
                        .await;
                }
                if let Some(ref mut hls) = ctx.hls_state
                    && let Err(e) = hls
                        .write_video(remainder, ts, is_keyframe, composition_time_offset)
                        .await
                {
                    tracing::warn!("HLS write_video failed: {}", e);
                }
            } else if avc_packet_type == 2 {
                // AVC end of sequence
                if let Some(ref mut hls) = ctx.hls_state
                    && let Err(e) = hls.finalize_segment().await
                {
                    tracing::warn!("HLS finalize_segment failed: {}", e);
                }
            }
        } else {
            // Non-AVC legacy video (Sorenson, VP6, etc.)
            if !ctx.discovered_tracks.contains(&0) {
                ctx.discovered_tracks.insert(0);
                let video_codec = ctx.track_video_codecs.get(&0).copied();
                let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec).await;
            }
            let remainder = &data[1..];
            if let Some(ref mut hls) = ctx.hls_state {
                let _ = hls.write_video(remainder, ts, is_keyframe, 0).await;
            }
        }
    }

    // Drain default hls_state to recorder
    if let (Some(ref mut hls), Some(ref mut recorder)) =
        (ctx.hls_state.as_mut(), ctx.recorder.as_mut())
    {
        drain_hls_to_recorder(hls, recorder).await;
    }

    // Drain track states to free memory
    for track_state in ctx.track_states.values_mut() {
        let _ = track_state.drain_init_data();
        let _ = track_state.drain_segment_data();
    }
}

async fn handle_audio_data(
    data: &[u8],
    ts: u32,
    ctx: &mut SessionContext,
    app_state: &Arc<AppState>,
    stream_key: &str,
) {
    tracing::debug!(
        "handle_audio_data: ts={}, len={}, first_bytes={:02x?}, hls_state={}",
        ts,
        data.len(),
        &data[..data.len().min(8)],
        ctx.hls_state.is_some()
    );
    if crate::rtmp::enhanced::is_enhanced_audio(data) {
        if let Ok((header, remainder)) = crate::rtmp::enhanced::parse_enhanced_audio_header(data) {
            match header.packet_type {
                crate::rtmp::enhanced::AudioPacketType::Multitrack => {
                    match crate::rtmp::enhanced::parse_enhanced_audio_multitrack(remainder) {
                        Ok((_multitrack_type, inner_pt, tracks)) => {
                            tracing::debug!(
                                "Parsed audio multitrack: {} tracks, inner_pt={:?}",
                                tracks.len(),
                                inner_pt
                            );
                            for track in tracks.iter() {
                                if ctx.closed_audio_tracks.contains(&track.track_id) {
                                    continue;
                                }
                                let Some(codec) = map_enhanced_audio_codec(track.codec.clone())
                                else {
                                    tracing::warn!(
                                        "Unsupported audio codec {:?}, skipping track {}",
                                        track.codec,
                                        track.track_id
                                    );
                                    continue;
                                };
                                let old_audio_codec =
                                    ctx.track_audio_codecs.insert(track.track_id, codec);
                                let is_new_audio_codec =
                                    old_audio_codec.is_none() || old_audio_codec != Some(codec);
                                let is_new_track = !ctx.discovered_tracks.contains(&track.track_id);
                                if is_new_track || is_new_audio_codec {
                                    if is_new_track {
                                        ctx.discovered_tracks.insert(track.track_id);
                                    }
                                    let video_codec =
                                        ctx.track_video_codecs.get(&track.track_id).copied();
                                    notify_track_discovered(
                                        app_state,
                                        stream_key,
                                        track.track_id,
                                        video_codec,
                                        Some(codec),
                                    )
                                    .await;
                                }
                                // Default hls_state gets track_id == 0 as primary track
                                if track.track_id == 0
                                    && let Some(ref mut hls) = ctx.hls_state
                                {
                                    match inner_pt {
                                        Some(
                                            crate::rtmp::enhanced::AudioPacketType::SequenceStart,
                                        ) => {
                                            let _ =
                                                hls.set_audio_config(codec, track.payload).await;
                                        }
                                        Some(
                                            crate::rtmp::enhanced::AudioPacketType::SequenceEnd,
                                        ) => {
                                            let _ = hls.finalize_segment().await;
                                        }
                                        _ => {
                                            let _ = hls.write_audio(track.payload, ts).await;
                                        }
                                    }
                                }
                                // Every track gets its own track state
                                let track_state =
                                    ctx.get_or_create_track_state(track.track_id, true);
                                match inner_pt {
                                    Some(crate::rtmp::enhanced::AudioPacketType::SequenceStart) => {
                                        let _ = track_state
                                            .set_audio_config(codec, track.payload)
                                            .await;
                                    }
                                    Some(crate::rtmp::enhanced::AudioPacketType::SequenceEnd) => {
                                        let _ = track_state.finalize_segment().await;
                                        ctx.closed_audio_tracks.insert(track.track_id);
                                    }
                                    _ => {
                                        let _ = track_state.write_audio(track.payload, ts).await;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse audio multitrack: {}", e);
                        }
                    }
                }
                crate::rtmp::enhanced::AudioPacketType::SequenceStart => {
                    let Some(codec) = map_enhanced_audio_codec(header.codec.clone()) else {
                        tracing::warn!(
                            "Unsupported audio codec {:?}, dropping SequenceStart",
                            header.codec
                        );
                        return;
                    };
                    let old_audio_codec = ctx.track_audio_codecs.insert(0, codec);
                    let is_new_audio_codec =
                        old_audio_codec.is_none() || old_audio_codec != Some(codec);
                    let is_new_track = !ctx.discovered_tracks.contains(&0);
                    if is_new_track || is_new_audio_codec {
                        if is_new_track {
                            ctx.discovered_tracks.insert(0);
                        }
                        let video_codec = ctx.track_video_codecs.get(&0).copied();
                        notify_track_discovered(app_state, stream_key, 0, video_codec, Some(codec))
                            .await;
                    }
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.set_audio_config(codec, remainder).await;
                    }
                }
                crate::rtmp::enhanced::AudioPacketType::CodedFrames => {
                    if !ctx.discovered_tracks.contains(&0) {
                        ctx.discovered_tracks.insert(0);
                        let video_codec = ctx.track_video_codecs.get(&0).copied();
                        let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                        notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec)
                            .await;
                    }
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.write_audio(remainder, ts).await;
                    }
                }
                crate::rtmp::enhanced::AudioPacketType::SequenceEnd => {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.finalize_segment().await;
                    }
                }
                _ => {}
            }
        }
    } else if !data.is_empty() {
        let sound_format = (data[0] & 0xF0) >> 4;
        tracing::debug!(
            "legacy audio: sound_format={}, len={}, hls_state={}",
            sound_format,
            data.len(),
            ctx.hls_state.is_some()
        );
        if sound_format == 10 && data.len() >= 2 {
            let aac_packet_type = data[1];
            let remainder = &data[2..];
            if aac_packet_type == 0 {
                ctx.track_audio_codecs
                    .insert(0, crate::hls::fmp4::AudioCodec::Aac);
                if !ctx.discovered_tracks.contains(&0) {
                    ctx.discovered_tracks.insert(0);
                    let video_codec = ctx.track_video_codecs.get(&0).copied();
                    notify_track_discovered(
                        app_state,
                        stream_key,
                        0,
                        video_codec,
                        Some(crate::hls::fmp4::AudioCodec::Aac),
                    )
                    .await;
                }
                if let Some(ref mut hls) = ctx.hls_state
                    && let Err(e) = hls
                        .set_audio_config(crate::hls::fmp4::AudioCodec::Aac, remainder)
                        .await
                {
                    tracing::warn!("HLS set_audio_config failed: {}", e);
                }
            } else if aac_packet_type == 1
                && let Some(ref mut hls) = ctx.hls_state
            {
                if !ctx.discovered_tracks.contains(&0) {
                    ctx.discovered_tracks.insert(0);
                    let video_codec = ctx.track_video_codecs.get(&0).copied();
                    let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                    notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec)
                        .await;
                }
                if let Err(e) = hls.write_audio(remainder, ts).await {
                    tracing::warn!("HLS write_audio failed: {}", e);
                }
            }
        } else if sound_format == 9 && data.len() > 5 {
            // FFmpeg uses FLV audio format 9 for Opus ("Opus" prefix) and FLAC ("fLaC" prefix)
            let remainder = &data[1..];
            if remainder.starts_with(b"Opus") {
                let opus_data = &remainder[4..];
                if opus_data.starts_with(b"OpusHead") {
                    ctx.track_audio_codecs
                        .insert(0, crate::hls::fmp4::AudioCodec::Opus);
                    if !ctx.discovered_tracks.contains(&0) {
                        ctx.discovered_tracks.insert(0);
                        let video_codec = ctx.track_video_codecs.get(&0).copied();
                        notify_track_discovered(
                            app_state,
                            stream_key,
                            0,
                            video_codec,
                            Some(crate::hls::fmp4::AudioCodec::Opus),
                        )
                        .await;
                    }
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls
                            .set_audio_config(crate::hls::fmp4::AudioCodec::Opus, opus_data)
                            .await;
                    }
                } else {
                    if !ctx.discovered_tracks.contains(&0) {
                        ctx.discovered_tracks.insert(0);
                        let video_codec = ctx.track_video_codecs.get(&0).copied();
                        let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                        notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec)
                            .await;
                    }
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.write_audio(opus_data, ts).await;
                    }
                }
            } else if remainder.starts_with(b"fLaC") {
                if let Some(ref mut hls) = ctx.hls_state {
                    if hls.audio_codec() != Some(crate::hls::fmp4::AudioCodec::Flac)
                        && remainder.len() >= 38
                    {
                        // First packet: store config (includes "fLaC" prefix)
                        ctx.track_audio_codecs
                            .insert(0, crate::hls::fmp4::AudioCodec::Flac);
                        if !ctx.discovered_tracks.contains(&0) {
                            ctx.discovered_tracks.insert(0);
                            let video_codec = ctx.track_video_codecs.get(&0).copied();
                            notify_track_discovered(
                                app_state,
                                stream_key,
                                0,
                                video_codec,
                                Some(crate::hls::fmp4::AudioCodec::Flac),
                            )
                            .await;
                        }
                        let _ = hls
                            .set_audio_config(crate::hls::fmp4::AudioCodec::Flac, remainder)
                            .await;
                    } else if hls.audio_codec() == Some(crate::hls::fmp4::AudioCodec::Flac)
                        && remainder.len() > 4
                    {
                        // Subsequent packets: strip "fLaC" prefix before writing as audio sample.
                        // FFmpeg prepends "fLaC" to every FLAC packet in FLV.
                        if !ctx.discovered_tracks.contains(&0) {
                            ctx.discovered_tracks.insert(0);
                            let video_codec = ctx.track_video_codecs.get(&0).copied();
                            let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                            notify_track_discovered(
                                app_state,
                                stream_key,
                                0,
                                video_codec,
                                audio_codec,
                            )
                            .await;
                        }
                        let frame_data = &remainder[4..];
                        // Valid FLAC frames start with 0xFF (frame sync); skip non-frame packets.
                        if frame_data[0] == 0xFF {
                            let _ = hls.write_audio(frame_data, ts).await;
                        }
                    }
                }
            } else {
                if !ctx.discovered_tracks.contains(&0) {
                    ctx.discovered_tracks.insert(0);
                    let video_codec = ctx.track_video_codecs.get(&0).copied();
                    let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                    notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec)
                        .await;
                }
                if let Some(ref mut hls) = ctx.hls_state {
                    let _ = hls.write_audio(remainder, ts).await;
                }
            }
        } else {
            if !ctx.discovered_tracks.contains(&0) {
                ctx.discovered_tracks.insert(0);
                let video_codec = ctx.track_video_codecs.get(&0).copied();
                let audio_codec = ctx.track_audio_codecs.get(&0).copied();
                notify_track_discovered(app_state, stream_key, 0, video_codec, audio_codec).await;
            }
            let remainder = &data[1..];
            if let Some(ref mut hls) = ctx.hls_state {
                let _ = hls.write_audio(remainder, ts).await;
            }
        }
    }

    // Drain default hls_state to recorder
    if let (Some(ref mut hls), Some(ref mut recorder)) =
        (ctx.hls_state.as_mut(), ctx.recorder.as_mut())
    {
        drain_hls_to_recorder(hls, recorder).await;
    }

    // Drain track states to free memory
    for track_state in ctx.track_states.values_mut() {
        let _ = track_state.drain_init_data();
        let _ = track_state.drain_segment_data();
    }
}
