# LiveStream Platform — Architecture & Operations Guide

## 1. Overview

LiveStream Platform is a self-hosted live streaming server that ingests RTMP streams, transmuxes them into HLS (fMP4/CMAF) for browser playback, records sessions as MP4 files, and generates WebP thumbnails. It consists of:

- **Rust backend** — RTMP ingestion, HLS generation, recording, REST API
- **Vue 3 frontend** — Stream dashboard, live player, recordings library
- **nginx** — Reverse proxy, static file serving, HLS caching

## 2. System Architecture

```
┌─────────────────┐     RTMP      ┌─────────────────────────────┐
│  OBS / FFmpeg   │──────────────▶│  Rust Server (port 1935)    │
│  Stream Source  │               │  - RTMP session handler     │
└─────────────────┘               │  - HLS fMP4 muxer           │
                                  │  - MP4 recorder             │
                                  │  - Thumbnail generator      │
                                  └──────────────┬──────────────┘
                                                 │
┌─────────────────┐     HTTP      ┌──────────────▼──────────────┐
│  Vue 3 SPA      │◀──────────────│  nginx (port 8080)          │
│  (HLS.js player)│               │  - /api/   → localhost:8081 │
│                 │               │  - /hls/   → localhost:8081 │
│                 │               │  - /       → frontend/dist  │
└─────────────────┘               │  - /recordings/ → static    │
                                  └─────────────────────────────┘
```

### Port Allocation

| Service | Port | Purpose |
|---------|------|---------|
| nginx | 8080 | Public HTTP entry (SPA + API proxy + HLS proxy) |
| Rust API / HLS | 8081 | Internal API and HLS file serving (proxied by nginx) |
| RTMP | 1935 | Stream ingestion |

In local dev, `vite dev server` runs on port 3000 and proxies `/api`, `/hls`, `/recordings` to `localhost:8080` (nginx).

## 3. Backend Architecture (`server/`)

### 3.1 RTMP Ingestion (`rtmp/`)

- **`rtmp/server.rs`** — `TcpListener` accepting RTMP connections, spawning a `tokio` task per session.
- **`rtmp/session.rs`** — Full RTMP handshake and session lifecycle:
  - Handshake (`rml_rtmp::handshake::Handshake`)
  - Session event loop (`ServerSession::handle_input`)
  - Event dispatch: `ConnectionRequested`, `PublishStreamRequested`, `VideoDataReceived`, `AudioDataReceived`, `StreamMetadataChanged`, `PublishStreamFinished`
  - **Grace period**: On disconnect, the stream enters a grace period (default 30s). If the publisher reconnects within the grace period, the existing `HlsStreamState` and `Fmp4Recorder` are resumed. Otherwise, the stream is finalized.
- **`rtmp/enhanced.rs`** — Parses Enhanced RTMP headers (ExVideo/ExAudio, FFmpeg veovera format) to detect AV1, H.264, H.265, Opus, AAC codecs.
- **`rtmp/mod.rs`** — `StreamManager` holds two `HashMap`s:
  - `publishers`: active publishers with metadata
  - `pending_streams`: disconnected but grace-period-active streams

### 3.2 HLS Generation (`hls/`)

- **`hls/mod.rs`** — `HlsStreamState` manages per-stream HLS output:
  - Writes `init.mp4` (ftyp + moov with codec config)
  - Writes `.m4s` segments (moof + mdat)
  - Maintains `index.m3u8` playlist
  - **init.mp4 timing**: `write_init_segment()` is called inside `rotate_segment()`, ensuring init.mp4 is written when the first segment is created (after both video and audio configs have typically arrived).
  - **Resolution**: `set_video_config()` receives actual width/height from RTMP metadata (defaults 1920×1080). If metadata arrives later, `update_video_resolution()` rewrites init.mp4.
- **`hls/fmp4.rs`** — Low-level fMP4 (CMAF) muxer:
  - Supports H.264, H.265, AV1 video + AAC, Opus audio
  - Generates `init_segment()` (ftyp + moov), `flush_combined_fragment()` (moof + mdat)
  - Uses Sample tables with duration computation, composition time offsets, and proper `trex` defaults

### 3.3 Recording (`recording/`)

- **`recording/mod.rs`** — `Fmp4Recorder`:
  - Collects init segment + all flushed segments
  - On `close()`, concatenates into a single `.mp4` file (`{stream_key}_{YYYYMMDD}_{HHMMSS}.mp4`)
  - `write_index_json()` scans the recordings directory and writes `recordings/index.json` with metadata and thumbnail URLs
- **`thumbnail.rs`** — Thumbnail generation using ffmpeg:
  - **Live thumbnails**: Concatenates `init.mp4` + `segment00000.m4s` into a temp MP4, then runs ffmpeg
  - **Recording thumbnails**: Directly from the finalized MP4
  - Uses `scale={width}:-1` maintaining aspect ratio
  - Output format: **WebP** (`-quality 75 -compression_level 4`)

### 3.4 API (`api/`)

Axum router with these endpoints:

| Endpoint | Description |
|----------|-------------|
| `GET /api/health` | Health check |
| `GET /api/streams` | List active streams with metadata |
| `GET /api/streams/{key}` | Get single stream details |
| `GET /api/streams/{key}/thumbnail?width=W` | Stream thumbnail (WebP) |
| `GET /api/recordings` | List recordings (from index.json or fallback scan) |
| `GET /api/recordings/{filename}/thumbnail?width=W` | Recording thumbnail (WebP) |

Static file serving:
- `/hls/*` → `MEDIA_DIR/hls/*`
- `/recordings/*` → `MEDIA_DIR/recordings/*`

### 3.5 Configuration (`config/`)

Environment variables (all in `.env`):

| Variable | Default | Description |
|----------|---------|-------------|
| `RTMP_HOST` | `0.0.0.0` | RTMP listen address |
| `RTMP_PORT` | 1935 | RTMP ingestion port |
| `API_HOST` | `0.0.0.0` | API/HLS listen address |
| `API_PORT` | 8080 | Internal API/HLS port |
| `MEDIA_DIR` | `./data` | Root for HLS, recordings, thumbnails |
| `HLS_SEGMENT_DURATION` | 4 | Target segment duration in seconds |
| `HLS_SEGMENTS_KEEP` | 10 | Unused (legacy) |
| `RECORDING_ENABLED` | true | Enable MP4 recording |
| `THUMBNAIL_SIZES` | `320,480` | Comma-separated thumbnail widths |
| `THUMBNAIL_INTERVAL_SECONDS` | 10 | Minimum interval between thumbnail regenerations |
| `RECORDINGS_BASE_URL` | `/recordings` | Base URL for recording links |
| `STREAM_GRACE_PERIOD_SECONDS` | 30 | Reconnection grace period |

## 4. Frontend Architecture (`frontend/`)

### 4.1 Stack

- **Vue 3** (Composition API + `<script setup>`)
- **Vue Router** (history mode, SPA routing)
- **Pinia** (lightweight state management)
- **Tailwind CSS v4** (utility-first styling)
- **hls.js** (HLS playback in browsers)
- **Vite** (build tool + dev server)

### 4.2 Page Structure

| Route | View | Purpose |
|-------|------|---------|
| `/` | `Home.vue` | Active stream grid with polling |
| `/live/:key` | `LiveWatch.vue` | HLS player + stream metadata sidebar |
| `/recordings` | `Recordings.vue` | Recordings library with filters (stream key, date range) |

### 4.3 Key Patterns

- **Polling**: `usePolling()` composable fetches data every 3s, pauses when tab is hidden, and only updates reactive state when data actually changes (deep equality check).
- **Two-stage updates**: The Recordings page shows a "有新的錄影可查看 / Refresh" toast instead of abruptly re-rendering the list.
- **Player**: `Player.vue` uses hls.js with `enableWorker` and `lowLatencyMode`. Falls back to native HLS on Safari.

### 4.4 API Client

`api/client.ts` provides a thin typed fetch wrapper (`apiFetch<T>`) that throws `ApiError` on non-2xx responses and extracts `{ error: string }` bodies.

## 5. nginx Configuration

`nginx.local.conf` is the production-like config used in development:

- **Port 8080** — public entry point
- **`/`** — serves `frontend/dist` (SPA, `try_files` fallback to `index.html`)
- **`/api/`** → `localhost:8081`
- **`/hls/`** → `localhost:8081` (with `Cache-Control: no-cache`)
- **`/recordings/`** — static alias to `data/recordings/`
- **`/recordings/thumbnails/`** — static alias to `data/thumbnails/recordings/`

**Important**: When nginx runs as a non-root user, you must set `proxy_temp_path` to a writable directory (e.g., `/tmp/nginx_proxy_temp`) to avoid permission errors when proxying large HLS segments.

## 6. Data Flow

### Stream Ingestion to Playback

1. **Publish**: OBS/ffmpeg pushes RTMP to `:1935/live/{stream_key}`
2. **Handshake**: `rtmp/server.rs` accepts TCP, `session.rs` performs RTMP handshake
3. **Codec detection**: `enhanced.rs` parses video/audio headers → determines codec (H.264/AV1/Opus/AAC)
4. **HLS muxing**: `HlsStreamState` receives video/audio frames → `fmp4.rs` builds init.mp4 + segments
5. **Playlist update**: `update_playlist()` writes `index.m3u8` referencing `init.mp4` and segments
6. **Playback**: Browser loads `/hls/{key}/index.m3u8` via nginx → hls.js fetches init.mp4 + segments

### Recording Flow

1. **Recording**: `Fmp4Recorder` collects init + segments in memory
2. **Finalize on stop**: `close()` concatenates all data into `{key}_{timestamp}.mp4`
3. **Thumbnail**: ffmpeg generates `{filename}_w{size}.webp` thumbnails
4. **Index**: `write_index_json()` updates `recordings/index.json`

### Grace Period Flow

1. **Disconnect**: `handle_event` receives `PublishStreamFinished` → `finalize_session()`
2. **Grace period**: `StreamManager::mark_disconnected()` stores `HlsStreamState` + `Fmp4Recorder` in `pending_streams`
3. **Timer**: After `STREAM_GRACE_PERIOD_SECONDS`, `finalize_stream()` is called automatically
4. **Reconnect**: If a new `PublishStreamRequested` arrives for the same key within the grace period, `reconnect()` restores the pending state

## 7. Setup Guide

### 7.1 Prerequisites

- Rust 1.95+ (for 2024 edition with `let` chains)
- Node.js 22+ + npm
- ffmpeg (for thumbnails and test scripts)
- nginx (for reverse proxy in production-like setups)

### 7.2 Development Setup

```bash
# Clone and enter project
cd /home/kilo/vibe-livestream

# 1. Backend dependencies
cargo check

# 2. Frontend dependencies
cd frontend && npm install

# 3. Build frontend
cd frontend && npm run build

# 4. Build backend (release)
cargo build --release

# 5. Configure environment
cp .env.example .env
# Edit .env as needed (especially API_PORT if using nginx)

# 6. Start Rust server
./target/release/livestream-server

# 7. In another terminal, start nginx
nginx -c $(pwd)/nginx.local.conf

# Or for frontend dev with hot reload:
cd frontend && npm run dev   # port 3000, proxies to localhost:8080
```

### 7.3 Docker Setup

The project provides a split-container deployment via Docker Compose:

- **`livestream-backend`** — Rust server (RTMP 1935 + API/HLS 8080)
- **`livestream-nginx`** — nginx serving the Vue frontend and proxying `/api/`, `/hls/` to the backend

```bash
# Build images and start containers
docker compose up --build -d

# Check status
docker compose ps

# View logs
docker compose logs -f backend
docker compose logs -f nginx
```

**Port mapping (Docker):**

| Host | Container | Service |
|------|-----------|---------|
| `1935` | `backend:1935` | RTMP ingestion |
| `8080` | `nginx:80` | HTTP (SPA + API proxy + HLS proxy + recordings) |

**Environment variables (Docker Compose):**

Set in `docker-compose.yml` or via `.env`:

```yaml
RTMP_HOST=0.0.0.0
RTMP_PORT=1935
API_HOST=0.0.0.0
API_PORT=8080
MEDIA_DIR=/data
RUST_LOG=info
```

For a single-image build (legacy multi-stage), use the root `Dockerfile` instead.

## 8. Maintenance

### 8.1 Daily Operations

**Check server health:**
```bash
curl http://localhost:8080/api/health
```

**Check logs:**
```bash
tail -f server.log           # Rust server
tail -f nginx_error.log      # nginx errors
tail -f nginx_access.log     # HTTP access
```

**Restart services:**
```bash
# Restart Rust server
pkill -f livestream-server
./target/release/livestream-server > server.log 2>&1 &

# Reload nginx
nginx -s reload -c $(pwd)/nginx.local.conf
```

### 8.2 Disk Management

Recorded content accumulates in `MEDIA_DIR/`:
- `MEDIA_DIR/hls/{stream_key}/` — HLS segments and playlists (cleaned up after recording)
- `MEDIA_DIR/recordings/` — MP4 files + `index.json`
- `MEDIA_DIR/thumbnails/recordings/` — WebP thumbnails
- `MEDIA_DIR/thumbnails/{stream_key}_w{size}.webp` — Live stream thumbnails

Implement a retention policy (e.g., cron job) to delete old recordings:
```bash
# Example: delete recordings older than 30 days
find ./data/recordings -name "*.mp4" -mtime +30 -delete
# Then regenerate index.json by touching any stream or restarting
```

### 8.3 Thumbnail Configuration

Change thumbnail sizes via `THUMBNAIL_SIZES`:
```bash
THUMBNAIL_SIZES=320,480,640,1280
```

Existing `.jpg` thumbnails are **not** auto-migrated after the WebP switch. They will simply be ignored by the new code.

### 8.4 Monitoring

Key metrics to monitor:
- **RTMP connection count** — log lines with `RTMP connection from`
- **HLS segment generation rate** — check `segment{index}.m4s` file creation in `data/hls/`
- **Recording finalization** — check for `index.json` updates
- **nginx 5xx errors** — `grep '" 5' nginx_access.log`
- **Disk usage** — `df -h` on `MEDIA_DIR` volume

### 8.5 Common Issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| "Permission denied" on HLS segments in nginx | nginx `proxy_temp_path` not writable | Set `proxy_temp_path` in nginx config to `/tmp/nginx_proxy_temp` |
| Player shows "Waiting for stream" | No active RTMP publisher | Check OBS is streaming to correct RTMP URL |
| Thumbnails not generating | ffmpeg not installed or not in PATH | Install ffmpeg |
| API_PORT conflict | Port 8080 already used | Set `API_PORT=8081` in `.env` and update nginx upstream |
| Recordings not appearing in list | `index.json` stale | Restart server or trigger a new recording finalization |

## 9. Testing

### 9.1 Unit Tests

```bash
cargo test
```

Covers:
- `rtmp/enhanced` — Header parsing (AV1, AVC, Opus)
- `hls/fmp4` — fMP4 box structure, fragment generation
- `hls/mod` — Segment rotation, playlist content, grace period, close
- `recording/mod` — Recorder lifecycle, index.json generation
- `api/mod` — `closest_thumbnail_width` logic

### 9.2 Integration Tests

```bash
# Full automated test suite (requires running server + ffmpeg)
./test_auto.sh
```

Tests:
- H264 + AAC, AV1 + AAC, H264 + Opus, AV1 + Opus codec combinations
- Graceful stop + HLS cleanup
- Abnormal disconnect + reconnect grace period
- MP4 integrity (ffprobe + ffmpeg remux)
- Thumbnail generation

```bash
# Color space / HDR compatibility test
./test_color_space.sh
```

### 9.3 Manual Testing with ffmpeg

```bash
# Push a test stream
ffmpeg -re -f lavfi -i testsrc=duration=30:size=1280x720:rate=30 \
  -f lavfi -i "sine=frequency=440:duration=30" \
  -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \
  -c:a aac -ar 44100 \
  -f flv rtmp://localhost:1935/live/testkey
```

Then open `http://localhost:8080/live/testkey` in a browser.

## 10. Development Guidelines

### 10.1 Code Quality

Always run before committing:
```bash
cargo check
cargo clippy --all-targets --all-features
cargo test
```

### 10.2 Adding a New API Endpoint

1. Add handler in `server/src/api/{module}.rs`
2. Register route in `server/src/api/mod.rs` (`create_router`)
3. Add frontend API call in `frontend/src/api/streams.ts` or `recordings.ts`
4. Add types in `frontend/src/types/index.ts`

### 10.3 Adding a New Codec

1. Add variant to `hls/fmp4.rs` `VideoCodec` or `AudioCodec`
2. Update `write_stsd_video/audio` to emit the correct sample entry box
3. Map RTMP codec ID → `VideoCodec`/`AudioCodec` in `rtmp/session.rs::handle_video/audio_data`
4. Add test in `hls/fmp4.rs` tests

### 10.4 File Organization

```
vibe-livestream/
├── server/src/
│   ├── main.rs              # Entry point, AppState, axum server
│   ├── config/mod.rs        # Env var configuration
│   ├── api/                 # REST API handlers
│   ├── hls/                 # HLS generation + fMP4 muxer
│   ├── recording/           # MP4 recording + index.json
│   ├── rtmp/                # RTMP server + session + enhanced parsing
│   └── thumbnail.rs         # ffmpeg thumbnail generation
├── frontend/src/
│   ├── views/               # Page-level components
│   ├── components/          # Reusable UI components
│   ├── api/                 # Typed fetch wrappers
│   ├── composables/         # Vue composables (polling, streams)
│   ├── stores/              # Pinia stores
│   ├── router/              # Vue Router config
│   └── types/               # TypeScript interfaces
├── nginx.local.conf         # nginx reverse proxy config (local dev)
├── nginx.docker.conf        # nginx config for Docker Compose
├── Dockerfile               # Legacy single-image multi-stage build
├── Dockerfile.backend       # Rust backend image
├── Dockerfile.nginx         # Frontend + nginx image
├── docker-compose.yml       # Docker Compose setup (backend + nginx)
└── test_auto.sh             # Automated integration tests
```

## 11. Security Notes

- **No authentication** is currently implemented. Anyone who can reach `:1935` can publish, and anyone who can reach `:8080` can watch/list.
- Stream keys are arbitrary strings — there is no validation.
- The server binds to `0.0.0.0` by default.
- If deploying publicly, place nginx or a load balancer in front and add authentication/authorization at that layer.

## 12. CI/CD

GitHub Actions (`.github/workflows/ci.yml`):

1. **Check & Lint** — `cargo check`, `cargo clippy`, `cargo fmt --check`
2. **Tests** — `cargo test --lib`
3. **Frontend Build** — `npm ci && npm run build`
4. **Docker Build** — `docker build`
