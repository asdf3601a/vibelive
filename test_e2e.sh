#!/bin/bash
set -e

cd /home/kilo/vibe-livestream

API_BASE="http://localhost:8080"
RTMP_BASE="rtmp://localhost:1935/live"

# Check health
echo "=== Health check ==="
curl -s "${API_BASE}/api/health" && echo ""

STREAM_KEY="e2e_test_$(date +%s)"
echo "Stream key: $STREAM_KEY"

# Push RTMP stream
echo "=== Pushing RTMP stream ==="
ffmpeg -re -f lavfi -i testsrc=duration=10:size=640x360:rate=30 \
  -f lavfi -i sine=frequency=440:duration=10 \
  -c:v libx264 -preset ultrafast -t 8 \
  -c:a aac -f flv "${RTMP_BASE}/${STREAM_KEY}" &
FFMPEG_PID=$!
echo "FFmpeg PID: $FFMPEG_PID"

sleep 10

# Check HLS output
echo "=== HLS output ==="
ls -la data/hls/$STREAM_KEY/ 2>/dev/null || echo "No HLS output found"
cat data/hls/$STREAM_KEY/index.m3u8 2>/dev/null || echo "No m3u8 found"

# Check recording
echo "=== Recording ==="
ls -la data/recordings/ 2>/dev/null | tail -5

kill $FFMPEG_PID 2>/dev/null || true
sleep 2

# List streams
echo "=== List streams ==="
curl -s "${API_BASE}/api/streams" | python3 -m json.tool 2>/dev/null || echo ""

# List recordings
echo "=== List recordings ==="
curl -s "${API_BASE}/api/recordings" | python3 -m json.tool 2>/dev/null || echo ""

echo "=== Done ==="
