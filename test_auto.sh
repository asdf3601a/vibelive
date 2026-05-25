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

MEDIA_DIR="./data"
API_BASE="http://localhost:8080"
RTMP_BASE="rtmp://localhost:1935/live"

api_call() {
    curl -s "${API_BASE}$1"
}

count_recordings() {
    local key=$1
    ls "$MEDIA_DIR/recordings/" 2>/dev/null | grep -c "^${key}_" || echo 0
}

check_mp4_integrity() {
    local mp4_path="$1"
    local key="$2"
    local max_expected_duration="${3:-30}"
    local corrupt=0

    # Check 1: ffprobe can read duration without errors
    local duration
    duration=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$mp4_path" 2>&1) || {
        echo -e "${RED}✗ ffprobe failed to read $key${NC}"
        corrupt=1
    }

    # Check 2: duration is reasonable (not overflow / underflow)
    if [ "$corrupt" -eq 0 ]; then
        if [ -z "$duration" ] || [ "$duration" = "N/A" ]; then
            echo -e "${RED}✗ Duration is N/A for $key${NC}"
            corrupt=1
        else
            # Use awk for float comparison
            local too_long
            too_long=$(awk "BEGIN { print ($duration > $max_expected_duration) ? 1 : 0 }")
            local too_short
            too_short=$(awk "BEGIN { print ($duration < 0.5) ? 1 : 0 }")
            if [ "$too_long" -eq 1 ]; then
                echo -e "${RED}✗ Duration overflow: ${duration}s (expected < ${max_expected_duration}s) for $key${NC}"
                corrupt=1
            fi
            if [ "$too_short" -eq 1 ]; then
                echo -e "${RED}✗ Duration too short: ${duration}s for $key${NC}"
                corrupt=1
            fi
        fi
    fi

    # Check 3: ffmpeg can remux without decode errors
    if [ "$corrupt" -eq 0 ]; then
        local ffmpeg_err
        ffmpeg_err=$(ffmpeg -v error -i "$mp4_path" -c copy -f null - 2>&1) || true
        if [ -n "$ffmpeg_err" ]; then
            echo -e "${RED}✗ ffmpeg remux errors for $key:${NC}"
            echo "$ffmpeg_err" | head -5
            corrupt=1
        fi
    fi

    # Check 4: at least some video frames exist
    if [ "$corrupt" -eq 0 ]; then
        local frame_count
        frame_count=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=noprint_wrappers=1:nokey=1 "$mp4_path" 2>/dev/null || echo 0)
        if [ "$frame_count" -lt 10 ]; then
            echo -e "${RED}✗ Too few frames ($frame_count) for $key${NC}"
            corrupt=1
        fi
    fi

    if [ "$corrupt" -eq 0 ]; then
        echo -e "${GREEN}✓ MP4 integrity OK (duration=${duration}s, frames=${frame_count})${NC}"
        return 0
    else
        return 1
    fi
}

run_codec_test() {
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
        -t 6 -f flv \
        $extra_flags \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ffmpeg_pid=$!

    wait $ffmpeg_pid 2>/dev/null || true
    sleep 3  # Wait for thumbnail generation and index.json

    local mp4_count
    mp4_count=$(count_recordings "$key")

    if [ "$mp4_count" -gt 0 ]; then
        echo -e "${GREEN}✓ Recording generated ($mp4_count file(s))${NC}"

        local thumb_count
        thumb_count=$(find "$MEDIA_DIR/thumbnails/recordings" -name "${key}_*.mp4_w*.jpg" 2>/dev/null | wc -l)
        if [ "$thumb_count" -gt 0 ]; then
            echo -e "${GREEN}✓ Thumbnails generated ($thumb_count)${NC}"
        else
            echo -e "${YELLOW}⚠ No thumbnails found${NC}"
        fi

        if [ -f "$MEDIA_DIR/recordings/index.json" ]; then
            echo -e "${GREEN}✓ index.json exists${NC}"
        else
            echo -e "${YELLOW}⚠ index.json not found${NC}"
        fi

        # Integrity check on the recording
        local integrity_ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                if ! check_mp4_integrity "$f" "$key" 15; then
                    integrity_ok=0
                fi
            fi
        done

        if [ "$integrity_ok" -eq 1 ]; then
            echo -e "${GREEN}✓ $name PASSED${NC}"
            PASS=$((PASS+1))
        else
            echo -e "${RED}✗ $name FAILED (file corrupt)${NC}"
            FAIL=$((FAIL+1))
        fi
    else
        echo -e "${RED}✗ $name FAILED (no recording)${NC}"
        FAIL=$((FAIL+1))
    fi
}

run_graceful_stop_test() {
    local key="graceful_test_$(date +%s)"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: Graceful stop (HLS cleanup)"
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=440:duration=10" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 \
        -c:a aac -t 4 -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    FF_PID=$!

    wait $FF_PID 2>/dev/null || true
    sleep 3  # Wait for finalization and HLS cleanup

    local hls_dir="$MEDIA_DIR/hls/$key"
    local mp4_count
    mp4_count=$(count_recordings "$key")

    if [ "$mp4_count" -gt 0 ] && [ ! -d "$hls_dir" ]; then
        echo -e "${GREEN}✓ Recording exists and HLS directory cleaned up${NC}"

        local integrity_ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                if ! check_mp4_integrity "$f" "$key" 10; then
                    integrity_ok=0
                fi
            fi
        done

        if [ "$integrity_ok" -eq 1 ]; then
            echo -e "${GREEN}✓ Graceful stop test PASSED${NC}"
            PASS=$((PASS+1))
        else
            echo -e "${RED}✗ Graceful stop test FAILED (file corrupt)${NC}"
            FAIL=$((FAIL+1))
        fi
    elif [ "$mp4_count" -eq 0 ]; then
        echo -e "${RED}✗ Graceful stop test FAILED (no recording)${NC}"
        FAIL=$((FAIL+1))
    else
        echo -e "${RED}✗ Graceful stop test FAILED (HLS directory still exists)${NC}"
        FAIL=$((FAIL+1))
    fi
}

run_reconnect_test() {
    local key="reconnect_test_$(date +%s)"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: Abnormal disconnect + reconnect"
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # First push (long duration, will be killed abruptly)
    ffmpeg -y -re -f lavfi -i "testsrc=duration=30:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=440:duration=30" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 \
        -c:a aac -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    FF_PID=$!

    sleep 4  # Let it push for 4 seconds

    # Abruptly kill ffmpeg (simulates network drop / abnormal disconnect)
    kill -9 $FF_PID 2>/dev/null || true
    sleep 1

    # Check API - stream should still be "live" during grace period
    local stream_json
    stream_json=$(api_call "/api/streams")
    if echo "$stream_json" | grep -q "$key"; then
        echo -e "${GREEN}✓ Stream still visible in API during grace period${NC}"
    else
        echo -e "${YELLOW}⚠ Stream not visible in API during grace period${NC}"
    fi

    echo "--- Reconnecting within grace period ---"
    ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=1000:duration=10" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 \
        -c:a aac -t 4 -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    FF_PID=$!

    wait $FF_PID 2>/dev/null || true
    sleep 3  # Wait for finalization

    echo "--- After reconnect ---"
    local mp4_count
    mp4_count=$(count_recordings "$key")
    echo "MP4 recordings for reconnect key: $mp4_count"

    if [ "$mp4_count" -eq 1 ]; then
        local integrity_ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                if ! check_mp4_integrity "$f" "$key" 15; then
                    integrity_ok=0
                fi
            fi
        done

        if [ "$integrity_ok" -eq 1 ]; then
            echo -e "${GREEN}✓ Reconnect test PASSED (1 merged recording)${NC}"
            PASS=$((PASS+1))
        else
            echo -e "${RED}✗ Reconnect test FAILED (file corrupt)${NC}"
            FAIL=$((FAIL+1))
        fi
    else
        echo -e "${RED}✗ Reconnect test FAILED (expected 1 recording, got $mp4_count)${NC}"
        FAIL=$((FAIL+1))
    fi
}

# ============ MAIN ============

echo "=== Using existing instance ==="
echo "API:  $API_BASE"
echo "RTMP: $RTMP_BASE"
echo ""

api_call "/api/health" || { echo "Health check failed, is the server running?"; exit 1; }

echo ""
echo "========== VIDEO + AUDIO CODEC TESTS =========="

run_codec_test "H264 + AAC (legacy baseline)" libx264 aac "-preset ultrafast -tune zerolatency"
run_codec_test "AV1 + AAC (enhanced video)" libsvtav1 aac "-svtav1-params preset=12:crf=35"
run_codec_test "H264 + Opus (enhanced audio)" libx264 libopus "-preset ultrafast -tune zerolatency -ar 48000"
run_codec_test "AV1 + Opus (both enhanced)" libsvtav1 libopus "-svtav1-params preset=12:crf=35 -ar 48000"

echo ""
echo "========== GRACEFUL STOP + HLS CLEANUP =========="
run_graceful_stop_test

echo ""
echo "========== ABNORMAL DISCONNECT / RECONNECT =========="
run_reconnect_test

echo ""
echo "========== API CHECK =========="
echo "Health: $(api_call "/api/health")"
echo "Streams: $(api_call "/api/streams")"

# Final summary
echo ""
echo "============================================"
echo "              TEST SUMMARY"
echo "============================================"
echo -e "${GREEN}Passed: $PASS${NC}"
echo -e "${RED}Failed: $FAIL${NC}"
echo "============================================"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
