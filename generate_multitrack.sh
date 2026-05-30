#!/bin/bash
# Continuous multitrack (AV1 + H.264) stream generator
# Launches a new stream every INTERVAL seconds, each running for DURATION seconds
set -e

RTMP_URL="${RTMP_URL:-rtmp://5.home.oktw.one:1935/live}"
STREAM_PREFIX="${STREAM_PREFIX:-aislop_generator}"
DURATION=60
INTERVAL=65

cleanup() {
    echo ""
    echo "Killing background ffmpeg processes..."
    jobs -p | xargs -r kill 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

echo "Multitrack generator: new stream every ${INTERVAL}s, each ${DURATION}s"
echo "  RTMP: ${RTMP_URL}/${STREAM_PREFIX}"
echo "  Tracks: track_0=AV1+Opus  track_1=H.264+AAC"
echo "Press Ctrl+C to stop"
echo ""

while true; do
    KEY="${STREAM_PREFIX}"

    echo "[$(date '+%H:%M:%S')] ${KEY}"

    ffmpeg -y -re -loglevel warning \
        -f lavfi -i "testsrc=duration=${DURATION}:size=854x480:rate=30" \
        -f lavfi -i "testsrc=duration=${DURATION}:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=440:duration=${DURATION}" \
        -f lavfi -i "sine=frequency=880:duration=${DURATION}" \
        -map 0:v -c:v:1 libsvtav1 -preset:v:0 12 -pix_fmt:v:0 yuv420p -b:v:0 800k -g:v:0 60 \
        -map 1:v -c:v:0 libx264 -preset:v:1 ultrafast -pix_fmt:v:1 yuv420p -b:v:1 300k -g:v:1 30 \
        -map 2:a -c:a:1 libopus -ar:a:0 48000 -b:a:0 64k \
        -map 3:a -c:a:0 aac -ar:a:1 44100 -b:a:1 64k \
        -f flv "${RTMP_URL}/${KEY}" >/dev/null 2>&1 &

    sleep "${INTERVAL}"
done
