#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

# ── Colors ─────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
WARN=0
FAIL=0

# ── Config ─────────────────────────────────────────────────────────
STREAM_KEY="multitrack_test_$(date +%s)"
MEDIA_DIR=$(mktemp -d /tmp/livestream_multitrack_test.XXXXXX)
API_PORT=18081
RTMP_PORT=11935
API_BASE="http://localhost:${API_PORT}"
RTMP_BASE="rtmp://localhost:${RTMP_PORT}/live"
SERVER_PID=""
FFMPEG_PID=""

_cleanup() {
    if [[ -n "${FFMPEG_PID:-}" ]]; then
        kill "$FFMPEG_PID" 2>/dev/null || true
        wait "$FFMPEG_PID" 2>/dev/null || true
    fi
    if [[ -n "${SERVER_PID:-}" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$MEDIA_DIR"
}
trap _cleanup EXIT

pass() { echo -e "${GREEN}  ✓ $1${NC}"; PASS=$((PASS+1)); }
warn() { echo -e "${YELLOW}  ⚠ $1${NC}"; WARN=$((WARN+1)); }
fail() { echo -e "${RED}  ✗ $1${NC}"; FAIL=$((FAIL+1)); }

# ── Build ──────────────────────────────────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Multitrack Enhanced RTMP Integration Test"
echo "Key:  $STREAM_KEY"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

echo ""
echo "[1/7] Building server..."
if ! cargo build --release --bin livestream-server >/dev/null 2>&1; then
    fail "Server build failed"
    exit 1
fi
pass "Server build OK"

# ── Start Server ───────────────────────────────────────────────────
echo ""
echo "[2/7] Starting server (RTMP $RTMP_PORT / API $API_PORT)..."
MEDIA_DIR="$MEDIA_DIR" \
API_PORT="$API_PORT" \
API_HOST=0.0.0.0 \
RTMP_PORT="$RTMP_PORT" \
RTMP_HOST=0.0.0.0 \
HLS_SEGMENT_DURATION=2 \
RECORDING_ENABLED=true \
THUMBNAIL_INTERVAL_SECONDS=9999 \
RUST_LOG=info \
    ./target/release/livestream-server > /tmp/multitrack_server.log 2>&1 &
SERVER_PID=$!

for i in {1..30}; do
    if nc -z localhost "$RTMP_PORT" 2>/dev/null; then
        break
    fi
    sleep 0.5
done

if ! curl -s "${API_BASE}/api/health" >/dev/null 2>&1; then
    fail "Server health check failed (RTMP $RTMP_PORT not ready)"
    cat /tmp/multitrack_server.log | tail -20
    exit 1
fi
pass "Server ready on RTMP $RTMP_PORT"

# ── Push Multitrack Stream ─────────────────────────────────────────
echo ""
echo "[3/7] Pushing multitrack stream via ffmpeg..."
ffmpeg -hide_banner -loglevel warning -y -re \
    -f lavfi -i "testsrc=duration=15:size=1280x720:rate=30" \
    -f lavfi -i "testsrc=duration=15:size=640x360:rate=30" \
    -f lavfi -i "sine=frequency=440:duration=15" \
    -f lavfi -i "sine=frequency=880:duration=15" \
    -map 0:v -c:v:0 libsvtav1 -preset:v:0 12 -pix_fmt:v:0 yuv420p -b:v:0 1500k -g:v:0 60 -keyint_min:v:0 60 \
    -map 1:v -c:v:1 libx264 -preset:v:1 ultrafast -pix_fmt:v:1 yuv420p -b:v:1 500k -g:v:1 60 -keyint_min:v:1 60 \
    -map 2:a -c:a:0 libopus -ar:a:0 48000 -b:a:0 128k \
    -map 3:a -c:a:1 aac -ar:a:1 44100 -b:a:1 128k \
    -f flv "${RTMP_BASE}/${STREAM_KEY}" > /tmp/multitrack_ffmpeg.log 2>&1 &
FFMPEG_PID=$!

sleep 1
if ! kill -0 "$FFMPEG_PID" 2>/dev/null; then
    fail "ffmpeg failed to start"
    cat /tmp/multitrack_ffmpeg.log | tail -10
    exit 1
fi

# Wait for segments to start appearing while ffmpeg is still running
sleep 8

# ── HLS + fMP4 Checks (must be done while stream is live; recording=true cleans HLS on finalize) ──
echo ""
echo "[4/7] Checking HLS output (while stream is live)..."
HLS_DIR="$MEDIA_DIR/hls/$STREAM_KEY"

if [[ ! -f "$HLS_DIR/index.m3u8" ]]; then
    fail "index.m3u8 (default track) not found"
else
    pass "index.m3u8 exists"
fi

if [[ ! -f "$HLS_DIR/track_1/index.m3u8" ]]; then
    fail "track_1/index.m3u8 not found"
else
    pass "track_1/index.m3u8 exists"
fi

DEFAULT_SEGS=($(find "$HLS_DIR" -maxdepth 1 -name 'segment*.m4s' | sort))
TRACK1_SEGS=($(find "$HLS_DIR/track_1" -maxdepth 1 -name 'segment*.m4s' | sort))

if [[ ${#DEFAULT_SEGS[@]} -lt 1 ]]; then
    fail "default track has no segments"
else
    pass "default track has ${#DEFAULT_SEGS[@]} segment(s)"
fi

if [[ ${#TRACK1_SEGS[@]} -lt 1 ]]; then
    fail "track_1 has no segments"
else
    pass "track_1 has ${#TRACK1_SEGS[@]} segment(s)"
fi

if [[ ! -f "$HLS_DIR/init.mp4" ]]; then
    fail "default init.mp4 not found"
else
    pass "default init.mp4 exists"
fi

if [[ ! -f "$HLS_DIR/track_1/init.mp4" ]]; then
    fail "track_1/init.mp4 not found"
else
    pass "track_1/init.mp4 exists"
fi

# Validate fMP4 while files are still present
echo ""
echo "[5/7] Validating fMP4 structures..."
TMP_COMBINED=$(mktemp)

if [[ ${#DEFAULT_SEGS[@]} -gt 0 ]]; then
    cat "$HLS_DIR/init.mp4" "${DEFAULT_SEGS[0]}" > "$TMP_COMBINED"
    if ! ffprobe -hide_banner -loglevel error -show_format -show_streams "$TMP_COMBINED" >/dev/null 2>&1; then
        fail "ffprobe rejected default init + segment"
    else
        pass "default fMP4 valid"
    fi
else
    fail "default fMP4 skipped (no segments)"
fi

if [[ ${#TRACK1_SEGS[@]} -gt 0 ]]; then
    cat "$HLS_DIR/track_1/init.mp4" "${TRACK1_SEGS[0]}" > "$TMP_COMBINED"
    if ! ffprobe -hide_banner -loglevel error -show_format -show_streams "$TMP_COMBINED" >/dev/null 2>&1; then
        fail "ffprobe rejected track_1 init + segment"
    else
        pass "track_1 fMP4 valid"
    fi
else
    fail "track_1 fMP4 skipped (no segments)"
fi

rm -f "$TMP_COMBINED"

# Check API tracks while stream is still live
echo ""
echo "[6/7] Checking API tracks (while stream is live)..."
STREAMS_JSON=$(curl -s "${API_BASE}/api/streams" 2>/dev/null || echo "")
if ! echo "$STREAMS_JSON" | grep -q "$STREAM_KEY"; then
    fail "Stream not visible in /api/streams"
    exit 1
fi
pass "Stream visible in /api/streams"

# Extract tracks JSON for this stream via python
TRACKS_JSON=$(echo "$STREAMS_JSON" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for s in data:
    if s['stream_key'] == '$STREAM_KEY':
        print(json.dumps(s.get('tracks', [])))
        break
else:
    print('[]')
" 2>/dev/null || echo "[]")

TRACK_COUNT=$(echo "$TRACKS_JSON" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)

if [[ "$TRACK_COUNT" -lt 2 ]]; then
    fail "Expected >=2 tracks in API response, found $TRACK_COUNT"
    exit 1
fi
pass "API reports $TRACK_COUNT tracks"

if echo "$TRACKS_JSON" | grep -q "track_1/index.m3u8"; then
    pass "API tracks include track_1 HLS URL"
else
    fail "API tracks missing track_1 HLS URL"
fi

# Verify each track has non-null video_codec and audio_codec
for tid in 0 1; do
    VC=$(echo "$TRACKS_JSON" | python3 -c "
import sys, json
tracks = json.load(sys.stdin)
for t in tracks:
    if t['track_id'] == $tid:
        print(t.get('video_codec') or '')
        break
" 2>/dev/null || echo "")
    AC=$(echo "$TRACKS_JSON" | python3 -c "
import sys, json
tracks = json.load(sys.stdin)
for t in tracks:
    if t['track_id'] == $tid:
        print(t.get('audio_codec') or '')
        break
" 2>/dev/null || echo "")
    if [[ -z "$VC" ]]; then
        fail "track $tid video_codec is null/empty"
    else
        pass "track $tid video_codec='$VC'"
    fi
    if [[ -z "$AC" ]]; then
        fail "track $tid audio_codec is null/empty"
    else
        pass "track $tid audio_codec='$AC'"
    fi
done

# Stop ffmpeg gracefully and wait for finalize
if kill -0 "$FFMPEG_PID" 2>/dev/null; then
    kill "$FFMPEG_PID" 2>/dev/null || true
    wait "$FFMPEG_PID" 2>/dev/null || true
fi
sleep 3
pass "ffmpeg stream finished"

# ── Recording Checks ───────────────────────────────────────────────
echo ""
echo "[7/7] Checking recording output..."
REC_COUNT=$(find "$MEDIA_DIR/recordings" -name "${STREAM_KEY}_*.mp4" 2>/dev/null | wc -l) || REC_COUNT=0
if [[ "$REC_COUNT" -eq 0 ]]; then
    fail "No recording generated"
else
    pass "Recording generated ($REC_COUNT file(s))"

    # Recording must contain ONLY the default track, so exactly 1 file
    if [[ "$REC_COUNT" -ne 1 ]]; then
        fail "Expected exactly 1 recording file (default track only), found $REC_COUNT"
    else
        pass "Exactly 1 recording file (default track only)"
    fi

    for f in "$MEDIA_DIR/recordings/${STREAM_KEY}_"*.mp4; do
        if [[ -f "$f" ]]; then
            dur=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$f" 2>&1) || corrupt=1
            if [[ "${corrupt:-0}" -eq 1 ]] || [[ -z "$dur" ]] || [[ "$dur" = "N/A" ]]; then
                fail "Recording ffprobe failed: $(basename "$f")"
                continue
            fi
            too_short=$(awk "BEGIN { print ($dur < 0.5) ? 1 : 0 }")
            if [[ "$too_short" -eq 1 ]]; then
                fail "Recording too short (${dur}s): $(basename "$f")"
                continue
            fi
            ffmpeg_err=$(ffmpeg -v error -i "$f" -c copy -f null - 2>&1) || true
            if [[ -n "$ffmpeg_err" ]]; then
                fail "Recording remux errors: $(basename "$f")"
                echo "$ffmpeg_err" | head -3 | sed 's/^/    /'
                continue
            fi
            frame_count=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=noprint_wrappers=1:nokey=1 "$f" 2>/dev/null || echo 0)
            if [[ "$frame_count" -lt 10 ]]; then
                fail "Recording too few frames ($frame_count): $(basename "$f")"
                continue
            fi
            pass "Recording integrity OK (${dur}s, ${frame_count} frames)"

            # Verify recording contains both video and audio (default track has both)
            stream_count=$(ffprobe -v error -show_entries format=nb_streams -of default=noprint_wrappers=1:nokey=1 "$f" 2>/dev/null || echo 0)
            if [[ "$stream_count" -lt 2 ]]; then
                fail "Recording missing audio/video streams (found $stream_count): $(basename "$f")"
                continue
            fi
            pass "Recording contains $stream_count streams (video + audio)"
        fi
    done
fi

if [[ -f "$MEDIA_DIR/recordings/index.json" ]]; then
    pass "recordings/index.json exists"
else
    fail "recordings/index.json not found"
fi

# ── Summary ────────────────────────────────────────────────────────
echo ""
echo "============================================"
echo "           MULTITRACK TEST SUMMARY"
echo "============================================"
echo -e "${GREEN}Passed: $PASS${NC}"
if [[ "$WARN" -gt 0 ]]; then
    echo -e "${YELLOW}Warned: $WARN${NC}"
fi
if [[ "$FAIL" -gt 0 ]]; then
    echo -e "${RED}Failed: $FAIL${NC}"
fi
echo "============================================"

if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
exit 0
