use crate::AppState;
use crate::disk_writer::{DiskCommand, DiskWriter};
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn handle_rtmp_session(
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    app_state: Arc<AppState>,
) -> anyhow::Result<()> {
    let span = tracing::info_span!("rtmp_session", %peer_addr);
    let _guard = span.enter();

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
    let mut session_ctx = SessionContext::new(app_state.disk_writer.clone());

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
    disk_writer: DiskWriter,
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
    fn new(disk_writer: DiskWriter) -> Self {
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
            disk_writer,
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
        let disk_writer = self.disk_writer.clone();
        let state = self.track_states.entry(track_id).or_insert_with(|| {
            let mut s = HlsStreamState::new(
                &media_dir,
                &stream_key,
                track_id,
                is_audio_only,
                segment_duration,
                segments_keep,
                disk_writer,
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

    /// Lazily announce a track to the stream manager on first sighting.
    /// Uses whatever codec info is currently cached. No-op once discovered.
    async fn ensure_track_discovered(
        &mut self,
        app_state: &Arc<AppState>,
        stream_key: &str,
        track_id: u32,
    ) {
        if self.discovered_tracks.contains(&track_id) {
            return;
        }
        self.discovered_tracks.insert(track_id);
        let video_codec = self.track_video_codecs.get(&track_id).copied();
        let audio_codec = self.track_audio_codecs.get(&track_id).copied();
        notify_track_discovered(app_state, stream_key, track_id, video_codec, audio_codec).await;
    }

    /// Record a track's video codec and announce the track on discovery or
    /// codec change (so the published track listing stays in sync).
    async fn register_video_codec(
        &mut self,
        app_state: &Arc<AppState>,
        stream_key: &str,
        track_id: u32,
        codec: crate::hls::fmp4::VideoCodec,
    ) {
        let old = self.track_video_codecs.insert(track_id, codec);
        let is_new_track = !self.discovered_tracks.contains(&track_id);
        let is_codec_change = old.is_none() || old != Some(codec);
        if !is_new_track && !is_codec_change {
            return;
        }
        if is_new_track {
            self.discovered_tracks.insert(track_id);
        }
        let audio_codec = self.track_audio_codecs.get(&track_id).copied();
        notify_track_discovered(app_state, stream_key, track_id, Some(codec), audio_codec).await;
    }

    /// Record a track's audio codec and announce the track on discovery or
    /// codec change (so the published track listing stays in sync).
    async fn register_audio_codec(
        &mut self,
        app_state: &Arc<AppState>,
        stream_key: &str,
        track_id: u32,
        codec: crate::hls::fmp4::AudioCodec,
    ) {
        let old = self.track_audio_codecs.insert(track_id, codec);
        let is_new_track = !self.discovered_tracks.contains(&track_id);
        let is_codec_change = old.is_none() || old != Some(codec);
        if !is_new_track && !is_codec_change {
            return;
        }
        if is_new_track {
            self.discovered_tracks.insert(track_id);
        }
        let video_codec = self.track_video_codecs.get(&track_id).copied();
        notify_track_discovered(app_state, stream_key, track_id, video_codec, Some(codec)).await;
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
                existing.video_codec = Some(vc.display_name().to_string());
            }
            if let Some(ac) = audio_codec {
                existing.audio_codec = Some(ac.display_name().to_string());
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
            video_codec: video_codec.map(|c| c.display_name().to_string()),
            audio_codec: audio_codec.map(|c| c.display_name().to_string()),
        });
    }
}

fn write_master_playlist(
    media_dir: &str,
    stream_key: &str,
    track_ids: &[u32],
    disk_writer: &DiskWriter,
) {
    let stream_dir = PathBuf::from(media_dir).join("hls").join(stream_key);
    let mut playlist = String::new();
    playlist.push_str("#EXTM3U\n");
    playlist.push_str("#EXT-X-VERSION:6\n");

    // Default playlist (index.m3u8) always included — highest bandwidth entry
    playlist.push_str("#EXT-X-STREAM-INF:BANDWIDTH=2500000\n");
    playlist.push_str("index.m3u8\n");

    // Alternate tracks get progressively lower estimated bandwidth
    for (i, track_id) in track_ids.iter().enumerate() {
        let bandwidth = 1500000u32.saturating_sub(i as u32 * 500_000).max(500_000);
        playlist.push_str(&format!("#EXT-X-STREAM-INF:BANDWIDTH={}\n", bandwidth));
        playlist.push_str(&format!("track_{}/index.m3u8\n", track_id));
    }

    let master_path = stream_dir.join("master.m3u8");
    disk_writer.send(DiskCommand::WriteAndRename {
        tmp_path: stream_dir.join("master.m3u8.tmp"),
        final_path: master_path,
        data: playlist.into_bytes(),
    });
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
    write_master_playlist(&media_dir, stream_key, &track_ids, &app_state.disk_writer);

    // Set ended flag and delete live thumbnails BEFORE spawning any tasks
    {
        let sm = app_state.stream_manager.read().await;
        if let Some(info) = sm.get_publisher(stream_key) {
            info.ended.store(true, Ordering::SeqCst);
        }
    }
    let stream_thumb_dir = PathBuf::from(&media_dir).join("thumbnails").join("streams");
    let sizes = app_state.config.thumbnail_sizes.clone();
    for &w in &sizes {
        for ext in &["jxl", "avif", "png"] {
            app_state.disk_writer.send(DiskCommand::RemoveFile {
                path: stream_thumb_dir.join(format!("{}_w{}.{}", stream_key, w, ext)),
            });
        }
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
            let base_url = app_state.config.recordings_base_url.clone();
            let key = stream_key.to_string();
            let app_state = Arc::clone(app_state);
            let remux_path = mp4_path.clone();
            let filename = mp4_path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
                .unwrap_or_default();
            let recording_thumbnail_semaphore = app_state.recording_thumbnail_semaphore.clone();
            let disk_writer = app_state.disk_writer.clone();
            tokio::spawn(async move {
                // Wait for all pending disk writes to complete before
                // generating thumbnails (which read files from disk)
                disk_writer.flush().await;

                // Remux to faststart BEFORE thumbnail generation so we get
                // efficient single-pass extraction. If remux fails, proceed
                // anyway — thumbnails still work, just marginally slower.
                if let Err(e) = app_state.remux_queue.remux_now(&remux_path).await {
                    tracing::warn!(
                        "Recording remux failed for {}: {} — proceeding with thumbnails anyway",
                        key,
                        e
                    );
                }

                // Generate recording thumbnails (acquires the recording-dedicated
                // semaphore so live-thumbnail peaks cannot starve post-recording
                // generation). Unlike live thumbnails, failures are retried since a
                // recording is a one-shot artifact that won't be regenerated later.
                let thumb_dir = PathBuf::from(&media_dir)
                    .join("thumbnails")
                    .join("recordings");
                for attempt in 1..=RECORDING_THUMBNAIL_MAX_ATTEMPTS {
                    match crate::thumbnail::generate_thumbnails_for_file(
                        &mp4_path,
                        &thumb_dir,
                        &sizes,
                        recording_thumbnail_semaphore.clone(),
                    )
                    .await
                    {
                        Ok(_) => break,
                        Err(e) => {
                            if attempt < RECORDING_THUMBNAIL_MAX_ATTEMPTS {
                                tracing::warn!(
                                    "Recording thumbnail attempt {}/{} failed for {}: {}; retrying in {}s",
                                    attempt,
                                    RECORDING_THUMBNAIL_MAX_ATTEMPTS,
                                    key,
                                    e,
                                    RECORDING_THUMBNAIL_RETRY_DELAY_SECS
                                );
                                tokio::time::sleep(tokio::time::Duration::from_secs(
                                    RECORDING_THUMBNAIL_RETRY_DELAY_SECS,
                                ))
                                .await;
                            } else {
                                tracing::warn!(
                                    "Recording thumbnail generation failed after {} attempts for {}: {}",
                                    RECORDING_THUMBNAIL_MAX_ATTEMPTS,
                                    key,
                                    e
                                );
                            }
                        }
                    }
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
                    app_state.disk_writer.send(DiskCommand::RemoveDirAll { path: hls_dir });
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
        write_master_playlist(&media_dir, key, &track_ids, &ctx.disk_writer);
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
                        ended: Arc::new(AtomicBool::new(false)),
                        last_thumbnail_attempt_secs: Arc::new(AtomicU64::new(0)),
                    },
                );
                drop(sm);

                let media_dir = app_state.config.media_dir.clone();
                // Directory creation is handled by DiskWriter via HlsStreamState::rotate_segment()
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
                    ctx.disk_writer.clone(),
                ));
                ctx.track_states.clear();
                if app_state.config.recording_enabled {
                    // Recordings directory creation is handled by DiskWriter via Fmp4Recorder
                    ctx.recorder = Some(Fmp4Recorder::new(&media_dir, &stream_key, ctx.disk_writer.clone()));
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
                .map(|c| c.display_name().to_string())
                .or_else(|| metadata.video_codec_id.map(video_codec_id_name))
                .unwrap_or_default();
            let audio_codec_name = ctx
                .track_audio_codecs
                .get(&0)
                .map(|c| c.display_name().to_string())
                .or_else(|| metadata.audio_codec_id.map(audio_codec_id_name))
                .unwrap_or_default();
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

/// Map a legacy FLV/RTMP video codec id (or FourCC) to a display name.
/// Falls back to the raw id when unknown.
fn video_codec_id_name(id: u32) -> String {
    match id {
        7 => "H264".to_string(),
        12 => "HEVC".to_string(),
        13 => "AV1".to_string(),
        0x61766331 => "H264".to_string(), // "avc1"
        0x68657631 => "HEVC".to_string(), // "hev1"
        0x68766331 => "HEVC".to_string(), // "hvc1"
        0x61763031 => "AV1".to_string(),  // "av01"
        0x76703039 => "VP9".to_string(),  // "vp09"
        0x76766331 => "VVC".to_string(),  // "vvc1"
        other => other.to_string(),
    }
}

/// Map a legacy FLV/RTMP audio codec id (or FourCC) to a display name.
/// Falls back to the raw id when unknown.
fn audio_codec_id_name(id: u32) -> String {
    match id {
        0 => "Linear PCM".to_string(),
        2 => "MP3".to_string(),
        10 => "AAC".to_string(),
        11 => "Speex".to_string(),
        0x4F707573 => "Opus".to_string(),   // "Opus"
        0x664C6143 => "FLAC".to_string(),   // "fLaC"
        0x61632D33 => "AC-3".to_string(),   // "ac-3"
        0x65632D33 => "E-AC-3".to_string(), // "ec-3"
        other => other.to_string(),
    }
}

const RECORDING_THUMBNAIL_MAX_ATTEMPTS: u32 = 3;
const RECORDING_THUMBNAIL_RETRY_DELAY_SECS: u64 = 5;

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
                                ctx.register_video_codec(
                                    app_state,
                                    stream_key,
                                    track.track_id,
                                    codec,
                                )
                                .await;
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
                                        Some(crate::rtmp::enhanced::VideoPacketType::Metadata) => {
                                            let _ = hls
                                                .set_video_color_config(
                                                    super::color::parse_enhanced_color_config(track.payload),
                                                )
                                                .await;
                                            if let Some(hdr) =
                                                super::color::parse_enhanced_hdr_metadata(track.payload)
                                            {
                                                let _ = hls.set_hdr_metadata(hdr).await;
                                            }
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
                                // Every track gets its own track state (skip track 0 — already handled by ctx.hls_state)
                                if track.track_id != 0 {
                                    let track_state =
                                        ctx.get_or_create_track_state(track.track_id, false);
                                    match inner_pt {
                                        Some(
                                            crate::rtmp::enhanced::VideoPacketType::SequenceStart,
                                        ) => {
                                            let _ = track_state
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
                                            let _ = track_state.finalize_segment().await;
                                            ctx.closed_video_tracks.insert(track.track_id);
                                        }
                                        Some(crate::rtmp::enhanced::VideoPacketType::Metadata) => {
                                            let _ = track_state
                                                .set_video_color_config(
                                                    super::color::parse_enhanced_color_config(track.payload),
                                                )
                                                .await;
                                            if let Some(hdr) =
                                                super::color::parse_enhanced_hdr_metadata(track.payload)
                                            {
                                                let _ = track_state.set_hdr_metadata(hdr).await;
                                            }
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
                    ctx.register_video_codec(app_state, stream_key, 0, codec)
                        .await;
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls
                            .set_video_config(remainder, codec, ctx.video_width, ctx.video_height)
                            .await;
                    }
                }
                crate::rtmp::enhanced::VideoPacketType::CodedFrames
                | crate::rtmp::enhanced::VideoPacketType::CodedFramesX => {
                    ctx.ensure_track_discovered(app_state, stream_key, 0).await;
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
                        let _ = hls
                            .set_video_color_config(super::color::parse_enhanced_color_config(remainder))
                            .await;
                        if let Some(hdr) = super::color::parse_enhanced_hdr_metadata(remainder) {
                            let _ = hls.set_hdr_metadata(hdr).await;
                        }
                    }
                    for track_state in ctx.track_states.values_mut() {
                        let _ = track_state
                            .set_video_color_config(super::color::parse_enhanced_color_config(remainder))
                            .await;
                        if let Some(hdr) = super::color::parse_enhanced_hdr_metadata(remainder) {
                            let _ = track_state.set_hdr_metadata(hdr).await;
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
                ctx.register_video_codec(
                    app_state,
                    stream_key,
                    0,
                    crate::hls::fmp4::VideoCodec::H264,
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
                ctx.ensure_track_discovered(app_state, stream_key, 0).await;
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
            ctx.ensure_track_discovered(app_state, stream_key, 0).await;
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
                                ctx.register_audio_codec(
                                    app_state,
                                    stream_key,
                                    track.track_id,
                                    codec,
                                )
                                .await;
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
                                // Every track gets its own track state (skip track 0 — already handled by ctx.hls_state)
                                if track.track_id != 0 {
                                    let track_state =
                                        ctx.get_or_create_track_state(track.track_id, true);
                                    match inner_pt {
                                        Some(
                                            crate::rtmp::enhanced::AudioPacketType::SequenceStart,
                                        ) => {
                                            let _ = track_state
                                                .set_audio_config(codec, track.payload)
                                                .await;
                                        }
                                        Some(
                                            crate::rtmp::enhanced::AudioPacketType::SequenceEnd,
                                        ) => {
                                            let _ = track_state.finalize_segment().await;
                                            ctx.closed_audio_tracks.insert(track.track_id);
                                        }
                                        _ => {
                                            let _ =
                                                track_state.write_audio(track.payload, ts).await;
                                        }
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
                    ctx.register_audio_codec(app_state, stream_key, 0, codec)
                        .await;
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.set_audio_config(codec, remainder).await;
                    }
                }
                crate::rtmp::enhanced::AudioPacketType::CodedFrames => {
                    ctx.ensure_track_discovered(app_state, stream_key, 0).await;
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
                ctx.register_audio_codec(
                    app_state,
                    stream_key,
                    0,
                    crate::hls::fmp4::AudioCodec::Aac,
                )
                .await;
                if let Some(ref mut hls) = ctx.hls_state
                    && let Err(e) = hls
                        .set_audio_config(crate::hls::fmp4::AudioCodec::Aac, remainder)
                        .await
                {
                    tracing::warn!("HLS set_audio_config failed: {}", e);
                }
            } else if aac_packet_type == 1 && ctx.hls_state.is_some() {
                ctx.ensure_track_discovered(app_state, stream_key, 0).await;
                if let Some(ref mut hls) = ctx.hls_state
                    && let Err(e) = hls.write_audio(remainder, ts).await
                {
                    tracing::warn!("HLS write_audio failed: {}", e);
                }
            }
        } else if sound_format == 9 && data.len() > 5 {
            // FFmpeg uses FLV audio format 9 for Opus ("Opus" prefix) and FLAC ("fLaC" prefix)
            let remainder = &data[1..];
            if remainder.starts_with(b"Opus") {
                let opus_data = &remainder[4..];
                if opus_data.starts_with(b"OpusHead") {
                    ctx.register_audio_codec(
                        app_state,
                        stream_key,
                        0,
                        crate::hls::fmp4::AudioCodec::Opus,
                    )
                    .await;
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls
                            .set_audio_config(crate::hls::fmp4::AudioCodec::Opus, opus_data)
                            .await;
                    }
                } else {
                    ctx.ensure_track_discovered(app_state, stream_key, 0).await;
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls.write_audio(opus_data, ts).await;
                    }
                }
            } else if remainder.starts_with(b"fLaC") {
                // Decide the FLAC sub-path up front so the hls borrow is not
                // held across the (mutable) ctx codec-registration calls.
                let flac_set = ctx
                    .hls_state
                    .as_ref()
                    .map(|h| h.audio_codec() == Some(crate::hls::fmp4::AudioCodec::Flac))
                    .unwrap_or(false);
                if !flac_set && remainder.len() >= 38 {
                    // First packet: store config (includes "fLaC" prefix)
                    ctx.register_audio_codec(
                        app_state,
                        stream_key,
                        0,
                        crate::hls::fmp4::AudioCodec::Flac,
                    )
                    .await;
                    if let Some(ref mut hls) = ctx.hls_state {
                        let _ = hls
                            .set_audio_config(crate::hls::fmp4::AudioCodec::Flac, remainder)
                            .await;
                    }
                } else if flac_set && remainder.len() > 4 {
                    // Subsequent packets: strip "fLaC" prefix before writing as audio sample.
                    // FFmpeg prepends "fLaC" to every FLAC packet in FLV.
                    ctx.ensure_track_discovered(app_state, stream_key, 0).await;
                    let frame_data = &remainder[4..];
                    // Valid FLAC frames start with 0xFF (frame sync); skip non-frame packets.
                    if frame_data[0] == 0xFF
                        && let Some(ref mut hls) = ctx.hls_state
                    {
                        let _ = hls.write_audio(frame_data, ts).await;
                    }
                }
            } else {
                ctx.ensure_track_discovered(app_state, stream_key, 0).await;
                if let Some(ref mut hls) = ctx.hls_state {
                    let _ = hls.write_audio(remainder, ts).await;
                }
            }
        } else {
            ctx.ensure_track_discovered(app_state, stream_key, 0).await;
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

#[cfg(test)]
mod tests {
    #[test]
    fn test_amf0_colorinfo_hdr_parse_and_verify() {
        let mut data = Vec::new();
        // Outer: AMF string "colorInfo"
        data.push(0x02u8);
        data.extend_from_slice(&9u16.to_be_bytes());
        data.extend_from_slice(b"colorInfo");
        // colorInfo value: AMF0 Object (0x03)
        data.push(0x03u8);
        // colorConfig sub-object
        data.extend_from_slice(&11u16.to_be_bytes());
        data.extend_from_slice(b"colorConfig");
        data.push(0x03u8);
        data.extend_from_slice(&18u16.to_be_bytes());
        data.extend_from_slice(b"matrixCoefficients");
        data.push(0x00u8);
        data.extend_from_slice(&f64::to_be_bytes(9.0));
        data.extend_from_slice(&[0x00, 0x00, 0x09]);
        // hdrCll sub-object
        data.extend_from_slice(&6u16.to_be_bytes());
        data.extend_from_slice(b"hdrCll");
        data.push(0x03u8);
        for (name, val) in [("maxFall", 1000.0), ("maxCLL", 1000.0)] {
            data.extend_from_slice(&(name.len() as u16).to_be_bytes());
            data.extend_from_slice(name.as_bytes());
            data.push(0x00u8);
            data.extend_from_slice(&f64::to_be_bytes(val));
        }
        data.extend_from_slice(&[0x00, 0x00, 0x09]);
        // hdrMdcv sub-object
        data.extend_from_slice(&7u16.to_be_bytes());
        data.extend_from_slice(b"hdrMdcv");
        data.push(0x03u8);
        for (name, val) in [
            ("redX", 0.68),
            ("redY", 0.32),
            ("greenX", 0.265),
            ("greenY", 0.69),
            ("blueX", 0.15),
            ("blueY", 0.06),
            ("whitePointX", 0.3127),
            ("whitePointY", 0.329),
            ("maxLuminance", 1000.0),
            ("minLuminance", 0.005),
        ] {
            data.extend_from_slice(&(name.len() as u16).to_be_bytes());
            data.extend_from_slice(name.as_bytes());
            data.push(0x00u8);
            data.extend_from_slice(&f64::to_be_bytes(val));
        }
        data.extend_from_slice(&[0x00, 0x00, 0x09]);
        data.extend_from_slice(&[0x00, 0x00, 0x09]);

        let hdr = crate::rtmp::color::parse_enhanced_hdr_metadata(&data)
            .expect("Should parse AMF0 colorInfo with hdrCll/hdrMdcv");

        assert_eq!(hdr.max_content_light_level, 1000, "MaxCLL");
        assert_eq!(hdr.max_frame_average_light_level, 1000, "MaxFALL");
        assert_eq!(hdr.display_primaries_x[0], 34000, "RX");
        assert_eq!(hdr.display_primaries_y[0], 16000, "RY");
        assert_eq!(hdr.display_primaries_x[1], 13250, "GX");
        assert_eq!(hdr.display_primaries_y[1], 34500, "GY");
        assert_eq!(hdr.display_primaries_x[2], 7500, "BX");
        assert_eq!(hdr.display_primaries_y[2], 3000, "BY");
        assert_eq!(hdr.white_point_x, 15635, "WX");
        assert_eq!(hdr.white_point_y, 16450, "WY");
        assert_eq!(hdr.max_luminance, 10000000, "maxLum");
        assert_eq!(hdr.min_luminance, 50, "minLum");
    }
}
