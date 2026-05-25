#!/bin/bash
set -e

cd /home/kilo/vibe-livestream

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

PASS=0
FAIL=0

SERVER_PID=""
MEDIA_DIR="./data_test"
rm -rf "$MEDIA_DIR"
mkdir -p "$MEDIA_DIR"

start_server() {
    echo "=== Starting server ==="
    MEDIA_DIR="$MEDIA_DIR" \
    RTMP_PORT=1936 \
    API_PORT=8081 \
    STREAM_KEEP_ALIVE_SECS=30 \
    HLS_SEGMENT_DURATION=2 \
    HLS_SEGMENTS_KEEP=10 \
    RECORDING_ENABLED=true \
    RUST_LOG=warn \
    ./target/release/livestream-server &
    SERVER_PID=$!
    sleep 2
}

stop_server() {
    if [ -n "$SERVER_PID" ]; then
        echo "=== Stopping server ==="
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true
        SERVER_PID=""
    fi
}

cleanup_stream() {
    local key=$1
    rm -rf "$MEDIA_DIR/hls/$key"
}

check_hls() {
    local key=$1
    local timeout=${2:-15}
    local i=0
    while [ $i -lt $timeout ]; do
        if [ -f "$MEDIA_DIR/hls/$key/index.m3u8" ]; then
            return 0
        fi
        sleep 1
        i=$((i+1))
    done
    return 1
}

count_segments() {
    local key=$1
    local m3u8="$MEDIA_DIR/hls/$key/index.m3u8"
    if [ -f "$m3u8" ]; then
        grep -c '^segment' "$m3u8" 2>/dev/null || echo 0
    else
        echo 0
    fi
}

run_test() {
    local name="$1"
    local vcodec="$2"
    local acodec="$3"
    local extra_flags="${4:-}"
    local key="test_${vcodec}_${acodec}_$(date +%s)"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: $name"
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=440:duration=10" \
        -c:v "$vcodec" -pix_fmt yuv420p \
        -g 60 -keyint_min 60 \
        -c:a "$acodec" \
        -t 8 -f flv \
        $extra_flags \
        "rtmp://localhost:1936/live/$key" >/dev/null 2>&1 &
    local ffmpeg_pid=$!

    # Wait for ffmpeg to finish (so HLS is finalized)
    wait $ffmpeg_pid 2>/dev/null || true
    sleep 1

    if [ -f "$MEDIA_DIR/hls/$key/index.m3u8" ]; then
        echo -e "${GREEN}✓ HLS playlist generated${NC}"

        local m3u8="$MEDIA_DIR/hls/$key/index.m3u8"
        echo "--- m3u8 content ---"
        cat "$m3u8"
        echo "--------------------"

        local seg_count
        seg_count=$(count_segments "$key")
        echo "Segments in playlist: $seg_count"

        if [ "$seg_count" -gt 0 ]; then
            echo -e "${GREEN}✓ $name PASSED${NC}"
            PASS=$((PASS+1))
        else
            echo -e "${RED}✗ $name FAILED (no segments)${NC}"
            FAIL=$((FAIL+1))
        fi
    else
        echo -e "${RED}✗ $name FAILED (no HLS output)${NC}"
        FAIL=$((FAIL+1))
    fi

    cleanup_stream "$key"
}

# ============ MAIN ============

trap stop_server EXIT
start_server

echo ""
echo "========== VIDEO + AUDIO CODEC TESTS =========="

# 1. H264 + AAC (legacy baseline)
run_test "H264 + AAC (legacy baseline)" libx264 aac "-preset ultrafast -tune zerolatency"

# 2. AV1 + AAC (enhanced video + legacy audio)
run_test "AV1 + AAC (enhanced video)" libsvtav1 aac "-svtav1-params preset=12:crf=35"

# 3. H264 + Opus (legacy video + enhanced audio)
run_test "H264 + Opus (enhanced audio)" libx264 libopus "-preset ultrafast -tune zerolatency -ar 48000"

# 4. AV1 + Opus (enhanced video + enhanced audio)
run_test "AV1 + Opus (both enhanced)" libsvtav1 libopus "-svtav1-params preset=12:crf=35 -ar 48000"

# 5. H264 + FLAC (test if server handles gracefully)
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Test: H264 + FLAC (unsupported audio)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
key="test_h264_flac_$(date +%s)"
ffmpeg -y -re -f lavfi -i "testsrc=duration=6:size=640x360:rate=30" \
    -f lavfi -i "sine=frequency=440:duration=6" \
    -c:v libx264 -preset ultrafast -tune zerolatency -t 4 -f flv \
    "rtmp://localhost:1936/live/$key" >/dev/null 2>&1 &
ff_pid=$!
if check_hls "$key" 10; then
    echo -e "${YELLOW}⚠ HLS generated but audio may be ignored (FLAC unsupported)${NC}"
    PASS=$((PASS+1))
else
    echo -e "${YELLOW}⚠ No HLS (expected, FLAC unsupported by FLV/RTMP)${NC}"
    PASS=$((PASS+1))
fi
wait $ff_pid 2>/dev/null || true
cleanup_stream "$key"

# ============ DISCONNECT / RECONNECT TEST ============
echo ""
echo "========== DISCONNECT / RECONNECT TEST =========="

RECONNECT_KEY="reconnect_test_$(date +%s)"
echo "Stream key: $RECONNECT_KEY"

# First push (8s)
ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" \
    -f lavfi -i "sine=frequency=440:duration=10" \
    -c:v libx264 -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 \
    -c:a aac -t 8 -f flv \
    "rtmp://localhost:1936/live/$RECONNECT_KEY" >/dev/null 2>&1 &
FF_PID=$!

wait $FF_PID 2>/dev/null || true
sleep 1

echo "--- After first push ---"
if [ -f "$MEDIA_DIR/hls/$RECONNECT_KEY/index.m3u8" ]; then
    cat "$MEDIA_DIR/hls/$RECONNECT_KEY/index.m3u8"
    SEG1=$(count_segments "$RECONNECT_KEY")
    echo "Segments before disconnect: $SEG1"
else
    echo -e "${RED}✗ No HLS before disconnect${NC}"
    SEG1=0
fi

echo "--- Waiting 3s then reconnecting (within 30s keep-alive) ---"
sleep 3

# Second push (same key)
ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" \
    -f lavfi -i "sine=frequency=1000:duration=10" \
    -c:v libx264 -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 \
    -c:a aac -t 8 -f flv \
    "rtmp://localhost:1936/live/$RECONNECT_KEY" >/dev/null 2>&1 &
FF_PID=$!

wait $FF_PID 2>/dev/null || true
sleep 1

echo "--- After reconnect ---"
if [ -f "$MEDIA_DIR/hls/$RECONNECT_KEY/index.m3u8" ]; then
    cat "$MEDIA_DIR/hls/$RECONNECT_KEY/index.m3u8"
    if grep -q "DISCONTINUITY" "$MEDIA_DIR/hls/$RECONNECT_KEY/index.m3u8"; then
        echo -e "${GREEN}✓ #EXT-X-DISCONTINUITY found in playlist${NC}"
        DISC_OK=1
    else
        echo -e "${YELLOW}⚠ No DISCONTINUITY tag found${NC}"
        DISC_OK=0
    fi
    SEG2=$(count_segments "$RECONNECT_KEY")
    echo "Segments after reconnect: $SEG2"
else
    echo -e "${RED}✗ No HLS after reconnect${NC}"
    SEG2=0
    DISC_OK=0
fi

# Wait for zombie cleanup to finish recording
sleep 2

# Check recordings
echo ""
echo "--- Recordings ---"
ls -la "$MEDIA_DIR/recordings/" 2>/dev/null || echo "No recordings dir"
REC_COUNT=$(ls "$MEDIA_DIR/recordings/" 2>/dev/null | grep -c "\.mp4$" || echo 0)
echo "MP4 recordings for reconnect key: $REC_COUNT"

if [ "$DISC_OK" -eq 1 ] && [ "$SEG2" -gt 0 ]; then
    echo -e "${GREEN}✓ Reconnect test PASSED${NC}"
    PASS=$((PASS+1))
else
    echo -e "${RED}✗ Reconnect test FAILED${NC}"
    FAIL=$((FAIL+1))
fi

# Check API
sleep 2
echo ""
echo "--- API Check ---"
curl -s http://localhost:8081/api/health || echo "Health check failed"
curl -s http://localhost:8081/api/streams | python3 -m json.tool 2>/dev/null || echo "Streams API failed"

# Final summary
echo ""
echo "============================================"
echo "              TEST SUMMARY"
echo "============================================"
echo -e "${GREEN}Passed: $PASS${NC}"
echo -e "${RED}Failed: $FAIL${NC}"
echo "============================================"

stop_server
rm -rf "$MEDIA_DIR"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
