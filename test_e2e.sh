#!/bin/bash
set -e

cd /home/kilo/vibe-livestream

# Start server
DATABASE_URL="postgres://postgres:postgres@localhost:5432/livestream" \
REDIS_URL="redis://localhost:6379" \
JWT_SECRET="test-secret-12345" \
MEDIA_DIR="./data" \
./target/release/livestream-server &
SERVER_PID=$!
echo "Server PID: $SERVER_PID"
sleep 2

# Check health
echo "=== Health check ==="
curl -s http://localhost:8080/api/health && echo ""

# Register user
echo "=== Register ==="
REG=$(curl -s -X POST http://localhost:8080/api/auth/register \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","email":"test@test.com","password":"password123"}')
echo "$REG"
TOKEN=$(echo "$REG" | python3 -c "import sys,json; print(json.load(sys.stdin).get('token',''))" 2>/dev/null || echo "")
echo "Token: ${TOKEN:0:20}..."

# Create stream
echo "=== Create stream ==="
STREAM=$(curl -s -X POST http://localhost:8080/api/streams \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"title":"Test Stream"}')
echo "$STREAM"
STREAM_KEY=$(echo "$STREAM" | python3 -c "import sys,json; print(json.load(sys.stdin).get('stream_key',''))" 2>/dev/null || echo "")
echo "Stream key: $STREAM_KEY"

if [ -n "$STREAM_KEY" ]; then
  # Push RTMP stream
  echo "=== Pushing RTMP stream ==="
  ffmpeg -re -f lavfi -i testsrc=duration=10:size=640x360:rate=30 \
    -f lavfi -i sine=frequency=440:duration=10 \
    -c:v libx264 -preset ultrafast -t 8 \
    -c:a aac -f flv "rtmp://localhost:1935/live/$STREAM_KEY" &
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
fi

# List streams
echo "=== List streams ==="
curl -s http://localhost:8080/api/streams \
  -H "Authorization: Bearer $TOKEN" | python3 -m json.tool 2>/dev/null || echo ""

# List recordings
echo "=== List recordings ==="
curl -s http://localhost:8080/api/recordings \
  -H "Authorization: Bearer $TOKEN" | python3 -m json.tool 2>/dev/null || echo ""

echo "=== Done ==="
kill $SERVER_PID 2>/dev/null || true