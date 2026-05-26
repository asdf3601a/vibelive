use std::sync::Arc;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::collections::HashMap;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{ServerSession, ServerSessionConfig, ServerSessionResult, ServerSessionEvent};

use crate::AppState;
use crate::hls::HlsStreamState;
use crate::recording::Fmp4Recorder;

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
    let (mut session, init_results) = ServerSession::new(config)
        .map_err(|e| anyhow::anyhow!("session create: {}", e))?;
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
                    Ok(HandshakeProcessResult::Completed { response_bytes, remaining_bytes }) => {
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
                                    if let Err(e) = process_results(results, &mut session, &mut stream, &app_state, &mut session_ctx).await {
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
                    if let Err(e) = process_results(results, &mut session, &mut stream, &app_state, &mut session_ctx).await {
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
    }.await;

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
        }
    }

    fn get_or_create_track_state(&mut self, track_id: u32, is_audio_only: bool) -> &mut HlsStreamState {
        let media_dir = self.media_dir.as_ref().unwrap().clone();
        let stream_key = self.current_stream_key.as_ref().unwrap().clone();
        let segment_duration = self.hls_segment_duration;
        let segments_keep = self.hls_segments_keep;
        self.track_states.entry(track_id).or_insert_with(|| {
            HlsStreamState::new(&media_dir, &stream_key, track_id, is_audio_only, segment_duration, segments_keep)
        })
    }
}

async fn write_master_playlist(media_dir: &str, stream_key: &str, track_ids: &[u32]) -> anyhow::Result<()> {
    let stream_dir = PathBuf::from(media_dir).join("hls").join(stream_key);
    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:6\n");

    // Default playlist (index.m3u8) always included if it exists
    let default_playlist = stream_dir.join("index.m3u8");
    if tokio::fs::try_exists(&default_playlist).await.unwrap_or(false) {
        playlist.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2500000\n");
        playlist.push_str("index.m3u8\n");
    }

    // Include each track playlist
    for track_id in track_ids {
        let track_playlist = stream_dir.join(format!("track_{}", track_id)).join("index.m3u8");
        if tokio::fs::try_exists(&track_playlist).await.unwrap_or(false) {
            playlist.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2500000\n");
            playlist.push_str(&format!("track_{}/index.m3u8\n", track_id));
        }
    }

    let master_path = stream_dir.join("master.m3u8");
    tokio::fs::write(&master_path, playlist).await?;
    Ok(())
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
        if let Some(init) = hls.drain_init_data() {
            let _ = r.write_init(&init).await;
        }
        for seg in hls.drain_segment_data() {
            let _ = r.write_segment(&seg).await;
        }
        if let Ok(mp4_path) = r.close().await {
            let sizes = app_state.config.thumbnail_sizes.clone();
            let base_url = app_state.config.recordings_base_url.clone();
            let key = stream_key.to_string();
            tokio::spawn(async move {
                let thumb_dir = PathBuf::from(&media_dir).join("thumbnails").join("recordings");
                if let Err(e) = crate::thumbnail::generate_thumbnails_for_file(&mp4_path, &thumb_dir, &sizes).await {
                    tracing::warn!("Post-recording thumbnail generation failed for {}: {}", key, e);
                }
                if let Err(e) = crate::recording::write_index_json(&media_dir, &base_url, &sizes).await {
                    tracing::warn!("write_index_json failed for {}: {}", key, e);
                }
                // Clean up HLS files and stream thumbnails after recording is saved
                let hls_dir = PathBuf::from(&media_dir).join("hls").join(&key);
                let _ = tokio::fs::remove_dir_all(&hls_dir).await;
                let stream_thumb_dir = PathBuf::from(&media_dir).join("thumbnails").join("streams");
                for &w in &sizes {
                    let _ = tokio::fs::remove_file(stream_thumb_dir.join(format!("{}_w{}.webp", key, w))).await;
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
            tracing::warn!("Failed to write master playlist during grace period for {}: {}", key, e);
        }
    }

    if let Some(ref key) = ctx.current_stream_key.clone() {
        if let Some(mut hls) = ctx.hls_state.take() {
            let _ = hls.prepare_for_grace_period().await;
            if let Some(ref mut recorder) = ctx.recorder {
                if let Some(init) = hls.drain_init_data() {
                    let _ = recorder.write_init(&init).await;
                }
                for seg in hls.drain_segment_data() {
                    let _ = recorder.write_segment(&seg).await;
                }
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
                    sm.pending_streams.remove(&key_clone)
                };

                if let Some(pending) = pending {
                    finalize_stream(&app_state_clone, &key_clone, pending.hls_state, HashMap::new(), pending.recorder, true).await;
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
        ServerSessionEvent::ConnectionRequested { request_id, app_name } => {
            tracing::debug!("RTMP connect: app={}", app_name);
            ctx.current_app = Some(app_name);
            let responses = session.accept_request(request_id)
                .map_err(|e| anyhow::anyhow!("accept: {}", e))?;
            handle_outbound(stream, responses).await?;
        }

        ServerSessionEvent::ReleaseStreamRequested { request_id, .. } => {
            handle_outbound(stream, session.accept_request(request_id)
                .map_err(|e| anyhow::anyhow!("release: {}", e))?).await?;
        }

        ServerSessionEvent::PublishStreamRequested { request_id, stream_key, .. } => {
            tracing::debug!("Publish request: stream_key={}", stream_key);
            let responses = session.accept_request(request_id)
                .map_err(|e| anyhow::anyhow!("publish accept: {}", e))?;
            tracing::debug!("Sending {} outbound responses for publish accept", responses.len());
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
                sm.add_publisher(&stream_key, crate::rtmp::PublisherInfo {
                    stream_key: stream_key.clone(),
                    app_name: ctx.current_app.clone().unwrap_or_default(),
                    started_at: chrono::Utc::now(),
                    metadata: None,
                    disconnected_at: None,
                });
                drop(sm);

                let media_dir = app_state.config.media_dir.clone();
                let hls_dir = std::path::PathBuf::from(&media_dir).join("hls").join(&stream_key);
                let _ = tokio::fs::create_dir_all(&hls_dir).await;
                ctx.media_dir = Some(media_dir.clone());
                ctx.hls_segment_duration = app_state.config.hls_segment_duration;
                ctx.hls_segments_keep = app_state.config.hls_segments_keep;
                ctx.hls_state = Some(HlsStreamState::new(&media_dir, &stream_key, 0, false, app_state.config.hls_segment_duration, app_state.config.hls_segments_keep));
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

        ServerSessionEvent::VideoDataReceived { stream_key, data, timestamp, .. } => {
            if let Some(ref key) = ctx.current_stream_key
                && *key == stream_key
            {
                let ts = timestamp.value;
                handle_video_data(&data, ts, ctx, app_state, &stream_key).await;
            }
        }

        ServerSessionEvent::AudioDataReceived { stream_key, data, timestamp, .. } => {
            if let Some(ref key) = ctx.current_stream_key
                && *key == stream_key
            {
                let ts = timestamp.value;
                handle_audio_data(&data, ts, ctx).await;
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
            let meta = crate::rtmp::StreamMeta {
                width: metadata.video_width.unwrap_or(0),
                height: metadata.video_height.unwrap_or(0),
                video_codec: metadata.video_codec_id.map(|c| format!("{}", c)).unwrap_or_default(),
                audio_codec: metadata.audio_codec_id.map(|c| format!("{}", c)).unwrap_or_default(),
                video_bitrate: metadata.video_bitrate_kbps.unwrap_or(0),
                audio_bitrate: metadata.audio_bitrate_kbps.unwrap_or(0),
                framerate: metadata.video_frame_rate.unwrap_or(0.0) as f64,
            };
            if let Some(ref key) = ctx.current_stream_key {
                let mut sm = app_state.stream_manager.write().await;
                if let Some(ref mut pi) = sm.publishers.get_mut(key) {
                    pi.metadata = Some(meta);
                }
            }
        }

        ServerSessionEvent::PlayStreamRequested { request_id, .. } => {
            handle_outbound(stream, session.accept_request(request_id)
                .map_err(|e| anyhow::anyhow!("play accept: {}", e))?).await?;
        }

        _ => {}
    }
    Ok(())
}

async fn handle_video_data(data: &[u8], ts: u32, ctx: &mut SessionContext, _app_state: &Arc<AppState>, _stream_key: &str) {
    tracing::debug!("handle_video_data: ts={}, len={}, first_bytes={:02x?}", ts, data.len(), &data[..data.len().min(8)]);
    if crate::rtmp::enhanced::is_enhanced_video(data) {
        if let Ok((header, remainder)) = crate::rtmp::enhanced::parse_enhanced_video_header(data) {
            let is_keyframe = header.frame_type == crate::rtmp::enhanced::VideoFrameType::KeyFrame;
            match header.packet_type {
                crate::rtmp::enhanced::VideoPacketType::Multitrack => {
                    if let Ok((_multitrack_type, inner_pt, tracks)) = crate::rtmp::enhanced::parse_enhanced_video_multitrack(remainder) {
                        for (i, track) in tracks.iter().enumerate() {
                            let codec = match track.codec {
                                crate::rtmp::enhanced::EnhancedVideoCodec::Av1 => crate::hls::fmp4::VideoCodec::AV1,
                                crate::rtmp::enhanced::EnhancedVideoCodec::Avc => crate::hls::fmp4::VideoCodec::H264,
                                crate::rtmp::enhanced::EnhancedVideoCodec::Hevc => crate::hls::fmp4::VideoCodec::H265,
                                _ => crate::hls::fmp4::VideoCodec::H264,
                            };
                            let video_width = ctx.video_width;
                            let video_height = ctx.video_height;
                            // Default hls_state gets only the first video track
                            if i == 0 {
                                if let Some(ref mut hls) = ctx.hls_state {
                                    match inner_pt {
                                        Some(crate::rtmp::enhanced::VideoPacketType::SequenceStart) => {
                                            let _ = hls.set_video_config(track.payload, codec, video_width, video_height).await;
                                        }
                                        _ => {
                                            let _ = hls.write_video(track.payload, ts, is_keyframe).await;
                                        }
                                    }
                                }
                            }
                            // Every track gets its own track state
                            let track_state = ctx.get_or_create_track_state(track.track_id, false);
                            match inner_pt {
                                Some(crate::rtmp::enhanced::VideoPacketType::SequenceStart) => {
                                    let _ = track_state.set_video_config(track.payload, codec, video_width, video_height).await;
                                }
                                _ => {
                                    let _ = track_state.write_video(track.payload, ts, is_keyframe).await;
                                }
                            }
                        }
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::SequenceStart => {
                    let codec = match header.codec {
                        crate::rtmp::enhanced::EnhancedVideoCodec::Av1 => crate::hls::fmp4::VideoCodec::AV1,
                        crate::rtmp::enhanced::EnhancedVideoCodec::Avc => crate::hls::fmp4::VideoCodec::H264,
                        crate::rtmp::enhanced::EnhancedVideoCodec::Hevc => crate::hls::fmp4::VideoCodec::H265,
                        _ => crate::hls::fmp4::VideoCodec::H264,
                    };
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.set_video_config(remainder, codec, ctx.video_width, ctx.video_height).await;
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::CodedFrames |
                crate::rtmp::enhanced::VideoPacketType::CodedFramesX => {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.write_video(remainder, ts, is_keyframe).await;
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
        tracing::debug!("legacy video: frame_type={}, codec_id={}, len={}", frame_type, codec_id, data.len());

        if codec_id == 7 && data.len() >= 2 {
            let avc_packet_type = data[1];
            let remainder = if data.len() >= 5 { &data[5..] } else { &[] };
            tracing::debug!("avc_packet_type={}, remainder_len={}", avc_packet_type, remainder.len());

            if avc_packet_type == 0 {
                // AVC sequence header -> avcC config
                if let Some(ref mut hls) = ctx.hls_state {
                    let _ = hls.set_video_config(remainder, crate::hls::fmp4::VideoCodec::H264, ctx.video_width, ctx.video_height).await;
                }
            } else if avc_packet_type == 1 {
                // AVC NALU -> raw AVCC sample data
                if let Some(ref mut hls) = ctx.hls_state {
                    let _ = hls.write_video(remainder, ts, is_keyframe).await;
                }
            }
        } else {
            // Non-AVC legacy video (Sorenson, VP6, etc.)
            let remainder = &data[1..];
            if let Some(ref mut hls) = ctx.hls_state {
                let _ = hls.write_video(remainder, ts, is_keyframe).await;
            }
        }
    }

    // Drain default hls_state to recorder
    if let (Some(ref mut hls), Some(ref mut recorder)) = (ctx.hls_state.as_mut(), ctx.recorder.as_mut()) {
        if let Some(init) = hls.drain_init_data() {
            let _ = recorder.write_init(&init).await;
        }
        for seg in hls.drain_segment_data() {
            let _ = recorder.write_segment(&seg).await;
        }
    }

    // Drain track states to recorder (not needed for default track; recording uses default hls_state only)
    // But we still need to drain segment data from track states to free memory
    for track_state in ctx.track_states.values_mut() {
        let _ = track_state.drain_init_data();
        let _ = track_state.drain_segment_data();
    }

    if let Some(ref mut r) = ctx.recorder {
        let _ = r.write_video(data, ts).await;
    }
}

async fn handle_audio_data(data: &[u8], ts: u32, ctx: &mut SessionContext) {
    if crate::rtmp::enhanced::is_enhanced_audio(data) {
        if let Ok((header, remainder)) = crate::rtmp::enhanced::parse_enhanced_audio_header(data) {
            match header.packet_type {
                crate::rtmp::enhanced::AudioPacketType::Multitrack => {
                    if let Ok((_multitrack_type, inner_pt, tracks)) = crate::rtmp::enhanced::parse_enhanced_audio_multitrack(remainder) {
                        for (i, track) in tracks.iter().enumerate() {
                            let codec = match track.codec {
                                crate::rtmp::enhanced::EnhancedAudioCodec::Opus => crate::hls::fmp4::AudioCodec::Opus,
                                _ => crate::hls::fmp4::AudioCodec::Aac,
                            };
                            // Default hls_state gets only the first audio track
                            if i == 0 {
                                if let Some(ref mut hls) = ctx.hls_state {
                                    match inner_pt {
                                        Some(crate::rtmp::enhanced::AudioPacketType::SequenceStart) => {
                                            let _ = hls.set_audio_config(codec, track.payload).await;
                                        }
                                        _ => {
                                            let _ = hls.write_audio(track.payload, ts).await;
                                        }
                                    }
                                }
                            }
                            // Every track gets its own track state
                            let track_state = ctx.get_or_create_track_state(track.track_id, true);
                            match inner_pt {
                                Some(crate::rtmp::enhanced::AudioPacketType::SequenceStart) => {
                                    let _ = track_state.set_audio_config(codec, track.payload).await;
                                }
                                _ => {
                                    let _ = track_state.write_audio(track.payload, ts).await;
                                }
                            }
                        }
                    }
                }
                crate::rtmp::enhanced::AudioPacketType::SequenceStart => {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let codec = match header.codec {
                            crate::rtmp::enhanced::EnhancedAudioCodec::Opus => crate::hls::fmp4::AudioCodec::Opus,
                            _ => crate::hls::fmp4::AudioCodec::Aac,
                        };
                        let _ = hls.set_audio_config(codec, remainder).await;
                    }
                }
                crate::rtmp::enhanced::AudioPacketType::CodedFrames => {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.write_audio(remainder, ts).await;
                    }
                }
                _ => {}
            }
        }
    } else if !data.is_empty() {
        let sound_format = (data[0] & 0xF0) >> 4;
        if sound_format == 10 && data.len() >= 2 {
            let aac_packet_type = data[1];
            let remainder = &data[2..];
            if aac_packet_type == 0 {
                if let Some(ref mut hls) = ctx.hls_state {
                    let _ = hls.set_audio_config(crate::hls::fmp4::AudioCodec::Aac, remainder).await;
                }
            } else if aac_packet_type == 1
                && let Some(ref mut hls) = ctx.hls_state
            {
                let _ = hls.write_audio(remainder, ts).await;
            }
        } else if sound_format == 9 && data.len() > 5 {
            // FFmpeg uses FLV audio format 9 for Opus, prefixing data with "Opus"
            let remainder = &data[1..];
            if remainder.starts_with(b"Opus") {
                let opus_data = &remainder[4..];
                if opus_data.starts_with(b"OpusHead") {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.set_audio_config(crate::hls::fmp4::AudioCodec::Opus, opus_data).await;
                    }
                } else {
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.write_audio(opus_data, ts).await;
                    }
                }
            } else {
                let remainder = &data[1..];
                if let Some(ref mut hls) = ctx.hls_state {
                    let _ = hls.write_audio(remainder, ts).await;
                }
            }
        } else {
            let remainder = &data[1..];
            if let Some(ref mut hls) = ctx.hls_state {
                let _ = hls.write_audio(remainder, ts).await;
            }
        }
    }

    // Drain default hls_state to recorder
    if let (Some(ref mut hls), Some(ref mut recorder)) = (ctx.hls_state.as_mut(), ctx.recorder.as_mut()) {
        if let Some(init) = hls.drain_init_data() {
            let _ = recorder.write_init(&init).await;
        }
        for seg in hls.drain_segment_data() {
            let _ = recorder.write_segment(&seg).await;
        }
    }

    // Drain track states to free memory
    for track_state in ctx.track_states.values_mut() {
        let _ = track_state.drain_init_data();
        let _ = track_state.drain_segment_data();
    }

    if let Some(ref mut r) = ctx.recorder {
        let _ = r.write_audio(data, ts).await;
    }
}
