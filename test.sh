#!/bin/bash
set -e

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

MEDIA_DIR="./data"
REPORT_DIR="$MEDIA_DIR/test_reports"
API_BASE="http://localhost:8080"
RTMP_BASE="rtmp://localhost:1935/live"

# Temp file for collecting results
RESULTS_FILE=$(mktemp)
trap 'rm -f "$RESULTS_FILE"' EXIT

# ── Environment ────────────────────────────────────────────────────
if [ -f .env ]; then
    set -a
    source .env
    set +a
fi

# ── Defaults ───────────────────────────────────────────────────────
VIDEO_CODECS="h264 hevc av1"
AUDIO_CODECS="aac opus flac"
RESOLUTIONS="240p 480p 720p 1080p 2k 4k 8k"
TESTS="codec res color graceful reconnect hls multitrack"
FULL_MATRIX=0
DEFAULT_RES="480p 720p"
DEFAULT_VCODEC="h264"
DEFAULT_ACODEC="aac"
STREAM_DURATION=6
ASPECTS="16:9"
GRACE_WAIT=""
# FPS test rates (--tests fps); not part of --tests all
FPS_VALUES="24000/1001 24 25 30000/1001 30 50 60000/1001 60"

# ── Help ───────────────────────────────────────────────────────────
show_help() {
    cat <<'EOF'
Usage: ./test.sh [OPTIONS]

Note: The 'fps' test suite is NOT included by default (--tests all).

End-to-end test suite for livestream server.  Covers codec
compatibility, resolution/aspect-ratio coverage, color-space validation,
graceful-stop, reconnect, HLS streaming, and multitrack tests.

Options:
  --video LIST    Comma-separated video codecs: h264,hevc,av1
                  (default: h264,hevc,av1)
  --audio LIST    Comma-separated audio codecs: aac,opus,flac
                  (default: aac,opus,flac)
  --res LIST      Comma-separated resolutions: 240p,480p,720p,1080p,2k,4k,8k
                  (default: 240p,480p,720p,1080p,2k,4k,8k)
  --aspect LIST   Comma-separated aspect ratios: 16:9,4:3,1:1,21:9,9:16,3:4
                  (default: 16:9)
  --tests LIST    Comma-separated test suites:
                    codec      – video+audio codec matrix
                    res        – resolution matrix (full list with --full, else DEFAULT_RES × all aspects)
                    color      – color-space / HDR compatibility
                    graceful   – graceful stop & HLS cleanup
                    reconnect  – abnormal disconnect + reconnect
                    hls        – live HLS segment verification
                    multitrack – Enhanced RTMP multitrack (2 video + 2 audio)
                    fps        – NTSC/PAL frame rate consistency (NOT included in 'all')
                    all        – run every suite EXCEPT fps
  --full          Run full Cartesian product for codec/res matrices.
                  Without this flag the suite runs in quick mode:
                    • codec matrix uses only 480p and 720p
                    • res matrix skips if codec already covers it
  --duration N    Stream duration in seconds for each test (default: 6)
  --fps LIST      Comma-separated frame rates for fps test: 24000/1001,30,60000/1001,...
                  (default: 24000/1001 24 25 30000/1001 30 50 60000/1001 60)
  --grace-wait N  Max seconds to wait for HLS cleanup in graceful-stop test
                  (default: STREAM_GRACE_PERIOD_SECONDS from .env + 5, fallback 35)
  -h, --help      Show this help message

Quick-start Examples:
  # Run everything in quick mode (recommended for CI)
  ./test.sh

  # Full matrix: every codec × every audio × every resolution
  ./test.sh --full

  # Test only AV1 and H.264 with AAC and Opus
  ./test.sh --video h264,av1 --audio aac,opus

  # Validate non-16:9 aspect ratios across all resolutions
  ./test.sh --aspect 16:9,4:3,9:16 --tests res

  # Run only the color-space suite with 4-second streams
  ./test.sh --tests color --duration 4

  # Quick codec check for HEVC at 720p only
  ./test.sh --video hevc --res 720p --tests codec --duration 3
EOF
}

# ── Arg parse ──────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case $1 in
        --video)
            shift
            VIDEO_CODECS=$(echo "$1" | tr ',' ' ')
            shift
            ;;
        --audio)
            shift
            AUDIO_CODECS=$(echo "$1" | tr ',' ' ')
            shift
            ;;
        --res)
            shift
            RESOLUTIONS=$(echo "$1" | tr ',' ' ')
            shift
            ;;
        --aspect)
            shift
            ASPECTS=$(echo "$1" | tr ',' ' ')
            shift
            ;;
        --tests)
            shift
            TESTS=$(echo "$1" | tr ',' ' ')
            shift
            ;;
        --full)
            FULL_MATRIX=1
            shift
            ;;
        --duration)
            shift
            STREAM_DURATION="$1"
            shift
            ;;
        --grace-wait)
            shift
            GRACE_WAIT="$1"
            shift
            ;;
        --fps)
            shift
            FPS_VALUES=$(echo "$1" | tr ',' ' ')
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            show_help
            exit 1
            ;;
    esac
done

# ── Resolution mapping ─────────────────────────────────────────────
res_and_aspect_to_size() {
    local res="$1"
    local aspect="${2:-16:9}"
    local height

    case "$res" in
        240p)  height=240 ;;
        480p)  height=480 ;;
        720p)  height=720 ;;
        1080p) height=1080 ;;
        2k)    height=1440 ;;
        4k)    height=2160 ;;
        8k)    height=4320 ;;
        *)     height=360 ;;
    esac

    local aspect_w aspect_h
    aspect_w=$(echo "$aspect" | cut -d':' -f1)
    aspect_h=$(echo "$aspect" | cut -d':' -f2)

    local width
    width=$(awk "BEGIN { w = int($height * $aspect_w / $aspect_h); if (w % 2 == 1) w += 1; print w }")
    echo "${width}x${height}"
}

# ── Codec args mapping ─────────────────────────────────────────────
get_vcodec_args() {
    case "$1" in
        h264) echo "libx264 -preset ultrafast -tune zerolatency" ;;
        hevc) echo "libx265 -preset ultrafast" ;;
        av1)  echo "libsvtav1 -svtav1-params preset=12:crf=35" ;;
        *)    echo "libx264" ;;
    esac
}

get_acodec_args() {
    case "$1" in
        aac)  echo "aac" ;;
        opus) echo "libopus -ar 48000" ;;
        flac) echo "flac" ;;
        *)    echo "aac" ;;
    esac
}

get_vcodec_display() {
    case "$1" in
        h264) echo "H.264" ;;
        hevc) echo "HEVC" ;;
        av1)  echo "AV1" ;;
        *)    echo "$1" ;;
    esac
}

get_acodec_display() {
    case "$1" in
        aac)  echo "AAC" ;;
        opus) echo "Opus" ;;
        flac) echo "FLAC" ;;
        *)    echo "$1" ;;
    esac
}

# ── Helpers ────────────────────────────────────────────────────────
api_call() {
    curl -s "${API_BASE}$1"
}

count_recordings() {
    local key=$1
    local count
    count=$(ls "$MEDIA_DIR/recordings/" 2>/dev/null | grep -c "^${key}_" 2>/dev/null) || count=0
    echo "$count"
}

check_mp4_integrity() {
    local mp4_path="$1"
    local key="$2"
    local max_expected_duration="${3:-30}"
    local corrupt=0

    local duration
    duration=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$mp4_path" 2>&1) || {
        echo -e "${RED}  ✗ ffprobe failed to read $key${NC}"
        corrupt=1
    }

    if [ "$corrupt" -eq 0 ]; then
        if [ -z "$duration" ] || [ "$duration" = "N/A" ]; then
            echo -e "${RED}  ✗ Duration is N/A for $key${NC}"
            corrupt=1
        else
            local too_long
            too_long=$(awk "BEGIN { print ($duration > $max_expected_duration) ? 1 : 0 }")
            local too_short
            too_short=$(awk "BEGIN { print ($duration < 0.5) ? 1 : 0 }")
            if [ "$too_long" -eq 1 ]; then
                echo -e "${RED}  ✗ Duration overflow: ${duration}s for $key${NC}"
                corrupt=1
            fi
            if [ "$too_short" -eq 1 ]; then
                echo -e "${RED}  ✗ Duration too short: ${duration}s for $key${NC}"
                corrupt=1
            fi
        fi
    fi

    if [ "$corrupt" -eq 0 ]; then
        local ffmpeg_err
        ffmpeg_err=$(ffmpeg -v error -i "$mp4_path" -c copy -f null - 2>&1) || true
        if [ -n "$ffmpeg_err" ]; then
            echo -e "${RED}  ✗ ffmpeg remux errors for $key:${NC}"
            echo "$ffmpeg_err" | head -5 | sed 's/^/    /'
            corrupt=1
        fi
    fi

    if [ "$corrupt" -eq 0 ]; then
        local frame_count
        frame_count=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=noprint_wrappers=1:nokey=1 "$mp4_path" 2>/dev/null || echo 0)
        if [ "$frame_count" -lt 10 ]; then
            echo -e "${RED}  ✗ Too few frames ($frame_count) for $key${NC}"
            corrupt=1
        fi
    fi

    if [ "$corrupt" -eq 0 ]; then
        echo -e "${GREEN}  ✓ MP4 integrity OK (duration=${duration}s, frames=${frame_count})${NC}"
        return 0
    else
        return 1
    fi
}

extract_color_info() {
    local mp4_path="$1"
    ffprobe -v error -select_streams v:0 \
        -show_entries stream=pix_fmt,color_space,color_primaries,color_transfer,profile \
        -of default=noprint_wrappers=1 "$mp4_path" 2>/dev/null
}

# ── Core stream test ───────────────────────────────────────────────
run_stream_test() {
    local name="$1"
    local vcodec_raw="$2"
    local acodec_raw="$3"
    local size="$4"
    local key="$5"
    local check_hls="${6:-0}"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: $name"
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    # Build ffmpeg args array safely
    local ffmpeg_cmd=(ffmpeg -y -re -f lavfi -i "testsrc=duration=${STREAM_DURATION}:size=${size}:rate=30")
    ffmpeg_cmd+=(-f lavfi -i "sine=frequency=440:duration=${STREAM_DURATION}")

    # Parse video codec args
    read -r venc vrest <<< "$vcodec_raw"
    ffmpeg_cmd+=(-c:v "$venc")
    if [ -n "$vrest" ]; then
        ffmpeg_cmd+=($vrest)
    fi
    ffmpeg_cmd+=(-pix_fmt yuv420p -g 60 -keyint_min 60)

    # Parse audio codec args
    read -r aenc arest <<< "$acodec_raw"
    ffmpeg_cmd+=(-c:a "$aenc")
    if [ -n "$arest" ]; then
        ffmpeg_cmd+=($arest)
    fi

    ffmpeg_cmd+=(-t "$STREAM_DURATION" -f flv "${RTMP_BASE}/${key}")

    # Start ffmpeg in background
    "${ffmpeg_cmd[@]}" >/dev/null 2>&1 &
    local ffmpeg_pid=$!

    # Optionally check HLS while streaming
    if [ "$check_hls" -eq 1 ]; then
        sleep 4
        local hls_dir="$MEDIA_DIR/hls/$key"
        if [ -d "$hls_dir" ] && [ -f "$hls_dir/index.m3u8" ]; then
            local seg_count
            seg_count=$(ls -1 "$hls_dir"/*.ts 2>/dev/null | wc -l)
            echo -e "${GREEN}  ✓ HLS output active ($seg_count segment(s))${NC}"
        else
            echo -e "${YELLOW}  ⚠ HLS output not yet available${NC}"
        fi
    fi

    wait $ffmpeg_pid 2>/dev/null || true
    sleep 3

    local mp4_count
    mp4_count=$(count_recordings "$key")

    local result="FAIL"
    local result_color="$RED"

    if [ "$mp4_count" -gt 0 ]; then
        echo -e "${GREEN}  ✓ Recording generated ($mp4_count file(s))${NC}"

        local thumb_count
        thumb_count=$(find "$MEDIA_DIR/thumbnails/recordings" -name "${key}_*.mp4_w*.webp" 2>/dev/null | wc -l)
        if [ "$thumb_count" -gt 0 ]; then
            echo -e "${GREEN}  ✓ Thumbnails generated ($thumb_count)${NC}"
        else
            echo -e "${YELLOW}  ⚠ No thumbnails found${NC}"
        fi

        if [ -f "$MEDIA_DIR/recordings/index.json" ]; then
            echo -e "${GREEN}  ✓ index.json exists${NC}"
        else
            echo -e "${YELLOW}  ⚠ index.json not found${NC}"
        fi

        local integrity_ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                if ! check_mp4_integrity "$f" "$key" 15; then
                    integrity_ok=0
                fi
            fi
        done

        if [ "$integrity_ok" -eq 1 ]; then
            result="PASS"
            result_color="$GREEN"
        fi
    fi

    echo -e "${result_color}  ● Result: $result${NC}"
    echo "$name|$result" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    else
        FAIL=$((FAIL+1))
    fi
}

# ── Codec matrix ───────────────────────────────────────────────────
run_codec_matrix() {
    echo ""
    echo "========== VIDEO + AUDIO CODEC MATRIX =========="

    local res_list
    if [ "$FULL_MATRIX" -eq 1 ]; then
        res_list="$RESOLUTIONS"
    else
        res_list="$DEFAULT_RES"
    fi

    # Use first aspect for codec matrix to keep test count manageable
    local default_aspect
    default_aspect=$(echo "$ASPECTS" | awk '{print $1}')

    for v in $VIDEO_CODECS; do
        for a in $AUDIO_CODECS; do
            for r in $res_list; do
                local size
                size=$(res_and_aspect_to_size "$r" "$default_aspect")
                local vargs
                vargs=$(get_vcodec_args "$v")
                local aargs
                aargs=$(get_acodec_args "$a")
                local vdisp
                vdisp=$(get_vcodec_display "$v")
                local adisp
                adisp=$(get_acodec_display "$a")
                local key="codec_${v}_${a}_${r}_$(date +%s)"
                run_stream_test "${vdisp} + ${adisp} @ ${r} (${default_aspect})" "$vargs" "$aargs" "$size" "$key" 0
            done
        done
    done
}

# ── Resolution matrix ──────────────────────────────────────────────
run_res_matrix() {
    if [ "$FULL_MATRIX" -eq 1 ]; then
        echo ""
        echo "========== RESOLUTION MATRIX (covered by --full codec matrix) =========="
        return
    fi

    echo ""
    echo "========== RESOLUTION MATRIX =========="

    local vargs
    vargs=$(get_vcodec_args "$DEFAULT_VCODEC")
    local aargs
    aargs=$(get_acodec_args "$DEFAULT_ACODEC")
    local vdisp
    vdisp=$(get_vcodec_display "$DEFAULT_VCODEC")
    local adisp
    adisp=$(get_acodec_display "$DEFAULT_ACODEC")

    local res_list
    res_list="$DEFAULT_RES"
    for r in $res_list; do
        for aspect in $ASPECTS; do
            local size
            size=$(res_and_aspect_to_size "$r" "$aspect")
            local key="res_${DEFAULT_VCODEC}_${DEFAULT_ACODEC}_${r}_${aspect}_$(date +%s)"
            run_stream_test "Resolution ${r} @ ${aspect} (${vdisp}+${adisp})" "$vargs" "$aargs" "$size" "$key" 0
        done
    done
}

# ── Color space tests ──────────────────────────────────────────────
run_color_test() {
    local name="$1"
    local encoder="$2"
    local pix_fmt="$3"
    local profile_name="$4"
    local hdr="$5"
    local expected="$6"
    local key="color_${encoder}_${pix_fmt}_${hdr}_$(date +%s)"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Color Test: $name"
    echo "Encoder: $encoder | Pixel Format: $pix_fmt | HDR: $hdr"
    if [ -n "$profile_name" ]; then
        echo "Profile: $profile_name"
    fi
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local ffmpeg_args=(
        -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30"
        -f lavfi -i "sine=frequency=440:duration=10"
        -c:v "$encoder"
        -pix_fmt "$pix_fmt"
        -g 60 -keyint_min 60
        -c:a aac
        -t 6 -f flv
    )

    if [ -n "$profile_name" ]; then
        ffmpeg_args+=(-profile:v "$profile_name")
    fi

    if [ "$hdr" = "hdr" ]; then
        ffmpeg_args+=(-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc)
    fi

    if [ "$encoder" = "libsvtav1" ]; then
        ffmpeg_args+=(-svtav1-params preset=12:crf=35)
    fi

    ffmpeg "${ffmpeg_args[@]}" "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ffmpeg_pid=$!

    wait $ffmpeg_pid 2>/dev/null || true
    sleep 3

    local mp4_count
    mp4_count=$(count_recordings "$key")

    local result="FAIL"
    local result_color="$RED"
    local details=""

    if [ "$mp4_count" -gt 0 ]; then
        echo -e "${GREEN}  ✓ Recording generated ($mp4_count file(s))${NC}"

        local integrity_ok=1
        local mp4_path=""
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                mp4_path="$f"
                if ! check_mp4_integrity "$f" "$key" 15; then
                    integrity_ok=0
                fi
            fi
        done

        if [ "$integrity_ok" -eq 1 ] && [ -n "$mp4_path" ]; then
            local color_info
            color_info=$(extract_color_info "$mp4_path")
            echo "  Color info:"
            echo "$color_info" | sed 's/^/    /'

            if [ "$expected" = "WARN" ]; then
                result="WARN"
                result_color="$YELLOW"
                if [ "$hdr" = "hdr" ]; then
                    details="Records correctly; test encoder sends Enhanced RTMP Metadata (colr/clli/mdcv via colorInfo AMF object)"
                else
                    details="Records correctly; may not play in browser (expected)"
                fi
            else
                result="PASS"
                result_color="$GREEN"
                details="Baseline compatible"
            fi
        else
            result="FAIL"
            result_color="$RED"
            details="MP4 integrity check failed"
        fi
    else
        result="FAIL"
        result_color="$RED"
        details="No recording generated"
    fi

    echo -e "${result_color}  ● Result: $result${NC} — $details"
    echo "$name|$encoder|$pix_fmt|$hdr|$expected|$result|$details" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    elif [ "$result" = "WARN" ]; then
        WARN=$((WARN+1))
    else
        FAIL=$((FAIL+1))
    fi
}

run_color_matrix() {
    echo ""
    echo "========== COLOR SPACE TEST MATRIX =========="

    run_color_test "H.264 4:2:0 8-bit SDR"      libx264  yuv420p     ""      sdr  PASS
    run_color_test "AV1 4:2:0 8-bit SDR"        libsvtav1 yuv420p    ""      sdr  PASS
    run_color_test "H.264 4:2:2 8-bit SDR"      libx264  yuv422p     high422 sdr  WARN
    run_color_test "H.264 4:4:4 8-bit SDR"      libx264  yuv444p     high444 sdr  WARN
    run_color_test "H.264 NV12 8-bit SDR"       libx264  nv12        ""      sdr  PASS
    run_color_test "H.264 4:2:0 10-bit SDR"     libx264  yuv420p10le ""      sdr  WARN
    run_color_test "H.264 4:2:0 10-bit HDR"     libx264  yuv420p10le ""      hdr  WARN
    run_color_test "AV1 4:2:0 10-bit SDR"       libsvtav1 yuv420p10le ""     sdr  PASS
    run_color_test "AV1 4:2:0 10-bit HDR"       libsvtav1 yuv420p10le ""     hdr  WARN
}

# ── Graceful stop test ─────────────────────────────────────────────
run_graceful_stop_test() {
    echo ""
    echo "========== GRACEFUL STOP + HLS CLEANUP =========="

    local key="graceful_$(date +%s)"
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
    local ff_pid=$!

    wait $ff_pid 2>/dev/null || true
    sleep 3

    local hls_dir="$MEDIA_DIR/hls/$key"
    local mp4_count
    mp4_count=$(count_recordings "$key")

    # HLS cleanup happens after STREAM_GRACE_PERIOD_SECONDS in a background task.
    # Poll up to GRACE_WAIT seconds to allow it to complete.
    local grace_wait="${GRACE_WAIT:-$(( ${STREAM_GRACE_PERIOD_SECONDS:-30} + 5 ))}"
    echo "  Grace period: ${STREAM_GRACE_PERIOD_SECONDS:-30}s, polling up to ${grace_wait}s"
    local hls_wait=0
    while [ -d "$hls_dir" ] && [ "$hls_wait" -lt "$grace_wait" ]; do
        sleep 1
        hls_wait=$((hls_wait + 1))
    done

    local result="FAIL"
    local result_color="$RED"

    if [ "$mp4_count" -gt 0 ] && [ ! -d "$hls_dir" ]; then
        echo -e "${GREEN}  ✓ Recording exists and HLS directory cleaned up${NC}"

        local integrity_ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                if ! check_mp4_integrity "$f" "$key" 10; then
                    integrity_ok=0
                fi
            fi
        done

        if [ "$integrity_ok" -eq 1 ]; then
            result="PASS"
            result_color="$GREEN"
        else
            result="FAIL"
            result_color="$RED"
        fi
    elif [ "$mp4_count" -eq 0 ]; then
        echo -e "${RED}  ✗ No recording generated${NC}"
    else
        echo -e "${RED}  ✗ HLS directory still exists${NC}"
    fi

    echo -e "${result_color}  ● Result: $result${NC}"
    echo "graceful_stop|$result" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    else
        FAIL=$((FAIL+1))
    fi
}

# ── Reconnect test ─────────────────────────────────────────────────
run_reconnect_test() {
    echo ""
    echo "========== ABNORMAL DISCONNECT / RECONNECT =========="

    local key="reconnect_$(date +%s)"
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
    local ff_pid=$!

    sleep 4

    # Abruptly kill ffmpeg
    kill -9 $ff_pid 2>/dev/null || true
    sleep 1

    local stream_json
    stream_json=$(api_call "/api/streams")
    if echo "$stream_json" | grep -q "$key"; then
        echo -e "${GREEN}  ✓ Stream still visible in API during grace period${NC}"
    else
        echo -e "${YELLOW}  ⚠ Stream not visible in API during grace period${NC}"
    fi

    echo "  --- Reconnecting within grace period ---"
    ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=1000:duration=10" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 \
        -c:a aac -t 4 -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    ff_pid=$!

    wait $ff_pid 2>/dev/null || true
    sleep 3

    local mp4_count
    mp4_count=$(count_recordings "$key")

    local result="FAIL"
    local result_color="$RED"

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
            result="PASS"
            result_color="$GREEN"
            echo -e "${GREEN}  ✓ Reconnect test PASSED (1 merged recording)${NC}"
        else
            echo -e "${RED}  ✗ Reconnect test FAILED (file corrupt)${NC}"
        fi
    else
        echo -e "${RED}  ✗ Reconnect test FAILED (expected 1 recording, got $mp4_count)${NC}"
    fi

    echo -e "${result_color}  ● Result: $result${NC}"
    echo "reconnect|$result" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    else
        FAIL=$((FAIL+1))
    fi
}

# ── HLS / E2E test ─────────────────────────────────────────────────
run_hls_test() {
    echo ""
    echo "========== HLS / E2E STREAMING TEST =========="

    local key="hls_$(date +%s)"
    local size="640x360"
    local vargs
    vargs=$(get_vcodec_args "h264")
    local aargs
    aargs=$(get_acodec_args "aac")

    run_stream_test "HLS E2E (H.264 + AAC @ 360p)" "$vargs" "$aargs" "$size" "$key" 1
}

# ── Multitrack test ────────────────────────────────────────────────
run_multitrack_test() {
    echo ""
    echo "========== MULTITRACK ENHANCED RTMP TEST =========="

    local key="multitrack_$(date +%s)"
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: Multitrack stream (2 video + 2 audio)"
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re \
        -f lavfi -i "testsrc=duration=15:size=1280x720:rate=30" \
        -f lavfi -i "testsrc=duration=15:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=440:duration=15" \
        -f lavfi -i "sine=frequency=880:duration=15" \
        -map 0:v -c:v:0 libsvtav1 -preset:v:0 12 -pix_fmt:v:0 yuv420p -b:v:0 1500k -g:v:0 60 -keyint_min:v:0 60 \
        -map 1:v -c:v:1 libx264 -preset:v:1 ultrafast -pix_fmt:v:1 yuv420p -b:v:1 500k -g:v:1 60 -keyint_min:v:1 60 \
        -map 2:a -c:a:0 libopus -ar:a:0 48000 -b:a:0 128k \
        -map 3:a -c:a:1 aac -ar:a:1 44100 -b:a:1 128k \
        -f flv "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ffmpeg_pid=$!

    sleep 1
    if ! kill -0 "$ffmpeg_pid" 2>/dev/null; then
        echo -e "${RED}  ✗ ffmpeg failed to start${NC}"
        echo "multitrack|FAIL" >> "$RESULTS_FILE"
        FAIL=$((FAIL+1))
        return
    fi

    # Wait for segments while stream is live (HLS cleaned on finalize when recording=true)
    sleep 8

    local result="FAIL"
    local result_color="$RED"
    local hls_dir="$MEDIA_DIR/hls/$key"
    local hls_ok=1

    if [ -f "$hls_dir/index.m3u8" ]; then
        echo -e "${GREEN}  ✓ index.m3u8 exists${NC}"
    else
        echo -e "${RED}  ✗ index.m3u8 not found${NC}"
        hls_ok=0
    fi

    if [ -f "$hls_dir/track_1/index.m3u8" ]; then
        echo -e "${GREEN}  ✓ track_1/index.m3u8 exists${NC}"
    else
        echo -e "${RED}  ✗ track_1/index.m3u8 not found${NC}"
        hls_ok=0
    fi

    local default_segs track1_segs
    default_segs=($(find "$hls_dir" -maxdepth 1 -name 'segment*.m4s' | sort))
    track1_segs=($(find "$hls_dir/track_1" -maxdepth 1 -name 'segment*.m4s' | sort))

    if [ ${#default_segs[@]} -ge 1 ]; then
        echo -e "${GREEN}  ✓ default track has ${#default_segs[@]} segment(s)${NC}"
    else
        echo -e "${RED}  ✗ default track has no segments${NC}"
        hls_ok=0
    fi

    if [ ${#track1_segs[@]} -ge 1 ]; then
        echo -e "${GREEN}  ✓ track_1 has ${#track1_segs[@]} segment(s)${NC}"
    else
        echo -e "${RED}  ✗ track_1 has no segments${NC}"
        hls_ok=0
    fi

    if [ -f "$hls_dir/init.mp4" ]; then
        echo -e "${GREEN}  ✓ default init.mp4 exists${NC}"
    else
        echo -e "${RED}  ✗ default init.mp4 not found${NC}"
        hls_ok=0
    fi

    if [ -f "$hls_dir/track_1/init.mp4" ]; then
        echo -e "${GREEN}  ✓ track_1/init.mp4 exists${NC}"
    else
        echo -e "${RED}  ✗ track_1/init.mp4 not found${NC}"
        hls_ok=0
    fi

    local tmp_combined
    tmp_combined=$(mktemp)
    local fmp4_ok=1

    if [ ${#default_segs[@]} -gt 0 ]; then
        cat "$hls_dir/init.mp4" "${default_segs[0]}" > "$tmp_combined"
        if ffprobe -hide_banner -loglevel error -show_format -show_streams "$tmp_combined" >/dev/null 2>&1; then
            echo -e "${GREEN}  ✓ default fMP4 valid${NC}"
        else
            echo -e "${RED}  ✗ default fMP4 invalid${NC}"
            fmp4_ok=0
        fi
    else
        echo -e "${RED}  ✗ default fMP4 skipped (no segments)${NC}"
        fmp4_ok=0
    fi

    if [ ${#track1_segs[@]} -gt 0 ]; then
        cat "$hls_dir/track_1/init.mp4" "${track1_segs[0]}" > "$tmp_combined"
        if ffprobe -hide_banner -loglevel error -show_format -show_streams "$tmp_combined" >/dev/null 2>&1; then
            echo -e "${GREEN}  ✓ track_1 fMP4 valid${NC}"
        else
            echo -e "${RED}  ✗ track_1 fMP4 invalid${NC}"
            fmp4_ok=0
        fi
    else
        echo -e "${RED}  ✗ track_1 fMP4 skipped (no segments)${NC}"
        fmp4_ok=0
    fi
    rm -f "$tmp_combined"

    # API checks (while live)
    local streams_json
    streams_json=$(api_call "/api/streams" 2>/dev/null || echo "")
    if echo "$streams_json" | grep -q "$key"; then
        echo -e "${GREEN}  ✓ Stream visible in /api/streams${NC}"
    else
        echo -e "${RED}  ✗ Stream not visible in /api/streams${NC}"
        hls_ok=0
    fi

    local tracks_json track_count
    tracks_json=$(echo "$streams_json" | python3 -c "
import sys, json
data = json.load(sys.stdin)
for s in data:
    if s['stream_key'] == '$key':
        print(json.dumps(s.get('tracks', [])))
        break
else:
    print('[]')
" 2>/dev/null || echo "[]")

    track_count=$(echo "$tracks_json" | python3 -c "import sys, json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)

    if [ "$track_count" -ge 2 ]; then
        echo -e "${GREEN}  ✓ API reports $track_count tracks${NC}"
    else
        echo -e "${RED}  ✗ Expected >=2 tracks in API, found $track_count${NC}"
        hls_ok=0
    fi

    if echo "$tracks_json" | grep -q "track_1/index.m3u8"; then
        echo -e "${GREEN}  ✓ API tracks include track_1 HLS URL${NC}"
    else
        echo -e "${RED}  ✗ API tracks missing track_1 HLS URL${NC}"
        hls_ok=0
    fi

    local codec_ok=1
    for tid in 0 1; do
        local vc ac
        vc=$(echo "$tracks_json" | python3 -c "
import sys, json
tracks = json.load(sys.stdin)
for t in tracks:
    if t['track_id'] == $tid:
        print(t.get('video_codec') or '')
        break
" 2>/dev/null || echo "")
        ac=$(echo "$tracks_json" | python3 -c "
import sys, json
tracks = json.load(sys.stdin)
for t in tracks:
    if t['track_id'] == $tid:
        print(t.get('audio_codec') or '')
        break
" 2>/dev/null || echo "")
        if [ -n "$vc" ]; then
            echo -e "${GREEN}  ✓ track $tid video_codec='$vc'${NC}"
        else
            echo -e "${RED}  ✗ track $tid video_codec is null/empty${NC}"
            codec_ok=0
        fi
        if [ -n "$ac" ]; then
            echo -e "${GREEN}  ✓ track $tid audio_codec='$ac'${NC}"
        else
            echo -e "${RED}  ✗ track $tid audio_codec is null/empty${NC}"
            codec_ok=0
        fi
    done

    # Stop ffmpeg and wait for finalize
    if kill -0 "$ffmpeg_pid" 2>/dev/null; then
        kill "$ffmpeg_pid" 2>/dev/null || true
        wait "$ffmpeg_pid" 2>/dev/null || true
    fi
    sleep 3

    # Recording checks
    local rec_count rec_ok=1
    rec_count=$(find "$MEDIA_DIR/recordings" -name "${key}_*.mp4" 2>/dev/null | wc -l) || rec_count=0

    if [ "$rec_count" -eq 0 ]; then
        echo -e "${RED}  ✗ No recording generated${NC}"
        rec_ok=0
    else
        echo -e "${GREEN}  ✓ Recording generated ($rec_count file(s))${NC}"
        if [ "$rec_count" -ne 1 ]; then
            echo -e "${RED}  ✗ Expected exactly 1 recording file (default track only), found $rec_count${NC}"
            rec_ok=0
        else
            echo -e "${GREEN}  ✓ Exactly 1 recording file (default track only)${NC}"
        fi

        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                local dur corrupt=0 too_short ffmpeg_err frame_count stream_count
                dur=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$f" 2>&1) || corrupt=1
                if [ "$corrupt" -eq 1 ] || [ -z "$dur" ] || [ "$dur" = "N/A" ]; then
                    echo -e "${RED}  ✗ Recording ffprobe failed: $(basename "$f")${NC}"
                    rec_ok=0
                    continue
                fi
                too_short=$(awk "BEGIN { print ($dur < 0.5) ? 1 : 0 }")
                if [ "$too_short" -eq 1 ]; then
                    echo -e "${RED}  ✗ Recording too short (${dur}s): $(basename "$f")${NC}"
                    rec_ok=0
                    continue
                fi
                ffmpeg_err=$(ffmpeg -v error -i "$f" -c copy -f null - 2>&1) || true
                if [ -n "$ffmpeg_err" ]; then
                    echo -e "${RED}  ✗ Recording remux errors: $(basename "$f")${NC}"
                    echo "$ffmpeg_err" | head -3 | sed 's/^/    /'
                    rec_ok=0
                    continue
                fi
                frame_count=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=noprint_wrappers=1:nokey=1 "$f" 2>/dev/null || echo 0)
                if [ "$frame_count" -lt 10 ]; then
                    echo -e "${RED}  ✗ Recording too few frames ($frame_count): $(basename "$f")${NC}"
                    rec_ok=0
                    continue
                fi
                echo -e "${GREEN}  ✓ Recording integrity OK (${dur}s, ${frame_count} frames)${NC}"

                stream_count=$(ffprobe -v error -show_entries format=nb_streams -of default=noprint_wrappers=1:nokey=1 "$f" 2>/dev/null || echo 0)
                if [ "$stream_count" -lt 2 ]; then
                    echo -e "${RED}  ✗ Recording missing audio/video streams (found $stream_count): $(basename "$f")${NC}"
                    rec_ok=0
                    continue
                fi
                echo -e "${GREEN}  ✓ Recording contains $stream_count streams (video + audio)${NC}"
            fi
        done
    fi

    if [ -f "$MEDIA_DIR/recordings/index.json" ]; then
        echo -e "${GREEN}  ✓ recordings/index.json exists${NC}"
    else
        echo -e "${YELLOW}  ⚠ recordings/index.json not found${NC}"
    fi

    if [ "$hls_ok" -eq 1 ] && [ "$fmp4_ok" -eq 1 ] && [ "$codec_ok" -eq 1 ] && [ "$rec_ok" -eq 1 ]; then
        result="PASS"
        result_color="$GREEN"
    fi

    echo -e "${result_color}  ● Result: $result${NC}"
    echo "multitrack|$result" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    else
        FAIL=$((FAIL+1))
    fi
}

# ── FPS frame rate test ────────────────────────────────────────────
run_fps_test() {
    local fps_frac="$1"
    local key="fps_$(echo "$fps_frac" | tr '/' '_')_$(date +%s)"
    local duration="${STREAM_DURATION}"

    # Parse fps numerator/denominator
    local fps_num=$fps_frac
    local fps_den=1
    if echo "$fps_frac" | grep -q '/'; then
        fps_num=$(echo "$fps_frac" | cut -d/ -f1)
        fps_den=$(echo "$fps_frac" | cut -d/ -f2)
    fi

    # GOP ≈ fps * 2 (approx 2-second keyframe interval)
    local gop=$(( fps_num * 2 / fps_den ))
    if [ "$gop" -lt 1 ]; then gop=1; fi

    local size="640x360"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "FPS Test: ${fps_num}/${fps_den}"
    echo "Key:  $key"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=${duration}:size=${size}:rate=${fps_frac}" \
        -f lavfi -i "sine=frequency=440:duration=${duration}" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \
        -g "$gop" -keyint_min "$gop" \
        -c:a aac -t "$duration" -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ff_pid=$!

    wait $ff_pid 2>/dev/null || true
    sleep 3

    local mp4_count
    mp4_count=$(count_recordings "$key")

    local result="FAIL"
    local result_color="$RED"

    if [ "$mp4_count" -gt 0 ]; then
        local rec_file
        rec_file=$(ls "$MEDIA_DIR/recordings/${key}_"*.mp4 2>/dev/null | head -1)
        if [ -n "$rec_file" ]; then
            # Check consistent frame duration via ffprobe
            # Use -show_packets instead of -show_entries frame, since the
            # recording may still be in fragmented MP4 format (remux runs async).
            local frame_durations
            frame_durations=$(ffprobe -v quiet -select_streams v:0 \
                -show_packets "$rec_file" 2>/dev/null \
                | grep '^duration=' | grep -v 'duration=0$' | sed 's/duration=//')

            if [ -z "$frame_durations" ]; then
                echo -e "${RED}  ✗ ffprobe returned no frame data${NC}"
            else
                local total unique
                total=$(echo "$frame_durations" | wc -l)
                unique=$(echo "$frame_durations" | sort -u | wc -l)

                # Get the dominant (most common) duration
                local dom_dur
                dom_dur=$(echo "$frame_durations" | sort -n | uniq -c | sort -rn | head -1 | awk '{print $2}')
                local dom_count
                dom_count=$(echo "$frame_durations" | sort -n | uniq -c | sort -rn | head -1 | awk '{print $1}')
                local dom_sec
                local tb_den
                tb_den=$(ffprobe -v error -select_streams v:0 -show_entries stream=time_base -of default=noprint_wrappers=1:nokey=1 "$rec_file" 2>/dev/null | tail -1)
                tb_den="${tb_den#*/}"
                dom_sec=$(python3 -c "print($dom_dur / ${tb_den:-90000})")

                echo -e "  frames=$total  unique=$unique  dominant=${dom_dur}ticks (${dom_sec}s)  count=$dom_count/$total"

                # Frame durations are uniform (from framerate rational).
                # Verify dominant covers ≥85% of frames (one outlier at segment boundary OK).
                local ok
                ok=$(python3 <<PY
total = $total
dom_count = $dom_count
ratio = dom_count / total
print("YES" if ratio >= 0.85 else f"NO (dominant covers {dom_count}/{total} = {ratio*100:.0f}%, need ≥85%)")
PY
)
                if [ "$ok" = "YES" ] && [ "$dom_count" -gt $(( total * 85 / 100 )) ]; then
                    echo -e "${GREEN}  ✓ Frame duration consistent ($dom_dur, ${dom_count}/${total} frames)${NC}"
                    result="PASS"
                    result_color="$GREEN"
                else
                    echo -e "${RED}  ✗ $ok${NC}"
                    if [ "$dom_count" -le $(( total * 85 / 100 )) ]; then
                        echo -e "${RED}  ✗ Dominant duration covers only ${dom_count}/${total} frames${NC}"
                        echo "  All distinct durations:"
                        echo "$frame_durations" | sort | uniq -c | sort -rn | head -10 | while read c d; do
                            echo "    ${c}× ${d}s"
                        done
                    fi
                fi
            fi
        else
            echo -e "${RED}  ✗ No recording file found${NC}"
        fi
    else
        echo -e "${RED}  ✗ No recording generated${NC}"
    fi

    echo -e "${result_color}  ● Result: $result${NC}"
    echo "fps_${fps_frac}|$result" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    else
        FAIL=$((FAIL+1))
    fi
}

run_fps_matrix() {
    echo ""
    echo "========== NTSC/PAL FRAME RATE TEST =========="
    for fps in $FPS_VALUES; do
        run_fps_test "$fps"
    done
}

# ── JSON report ────────────────────────────────────────────────────
generate_json_report() {
    local output_path="$1"
    mkdir -p "$(dirname "$output_path")"

    python3 - "$RESULTS_FILE" "$PASS" "$WARN" "$FAIL" "$output_path" <<'PYEOF'
import json, sys, time
from datetime import datetime

results_file = sys.argv[1]
pass_count = int(sys.argv[2])
warn_count = int(sys.argv[3])
fail_count = int(sys.argv[4])
output_path = sys.argv[5]

results = []
with open(results_file) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        parts = line.split('|')
        entry = {"name": parts[0], "result": parts[1]}
        if len(parts) > 2:
            entry["encoder"] = parts[1]
            entry["pix_fmt"] = parts[2]
            entry["hdr"] = parts[3]
            entry["expected"] = parts[4]
            entry["result"] = parts[5]
            entry["details"] = '|'.join(parts[6:]) if len(parts) > 6 else ""
            entry["name"] = parts[0]
        results.append(entry)

report = {
    "timestamp": int(time.time()),
    "date": datetime.now().isoformat(),
    "config": {
        "video_codecs": " ".join(sys.argv[6].split()) if len(sys.argv) > 6 else "",
        "audio_codecs": " ".join(sys.argv[7].split()) if len(sys.argv) > 7 else "",
        "resolutions": " ".join(sys.argv[8].split()) if len(sys.argv) > 8 else "",
        "aspects": " ".join(sys.argv[9].split()) if len(sys.argv) > 9 else "",
        "full_matrix": sys.argv[10] if len(sys.argv) > 10 else "0"
    },
    "summary": {
        "pass": pass_count,
        "warn": warn_count,
        "fail": fail_count,
        "total": pass_count + warn_count + fail_count
    },
    "results": results
}

with open(output_path, 'w') as f:
    json.dump(report, f, indent=2)

print(f"JSON report written to: {output_path}")
PYEOF
}

# ── Main ───────────────────────────────────────────────────────────
echo "=== Livestream Test Suite ==="
echo "API:      $API_BASE"
echo "RTMP:     $RTMP_BASE"
echo "Video:    $VIDEO_CODECS"
echo "Audio:    $AUDIO_CODECS"
if [ "$FULL_MATRIX" -eq 1 ]; then
    echo "Res:      $RESOLUTIONS"
else
    echo "Res:      $DEFAULT_RES (quick mode; full: $RESOLUTIONS)"
fi
echo "Aspect:   $ASPECTS"
echo "Tests:    $TESTS"
echo "Duration: ${STREAM_DURATION}s"
if [ "$FULL_MATRIX" -eq 1 ]; then
    echo "Mode:     FULL MATRIX"
else
    echo "Mode:     QUICK"
fi
echo ""

api_call "/api/health" || { echo "Health check failed, is the server running?"; exit 1; }

# Expand "all" in TESTS
if echo "$TESTS" | grep -qw "all"; then
    TESTS="codec res color graceful reconnect hls multitrack"
fi

# Run selected test suites
for t in $TESTS; do
    case $t in
        codec)
            run_codec_matrix
            ;;
        res)
            run_res_matrix
            ;;
        color)
            run_color_matrix
            ;;
        graceful)
            run_graceful_stop_test
            ;;
        reconnect)
            run_reconnect_test
            ;;
        hls)
            run_hls_test
            ;;
        multitrack)
            run_multitrack_test
            ;;
        fps)
            run_fps_matrix
            ;;
        *)
            echo "Unknown test suite: $t"
            ;;
    esac
done

# Final summary
echo ""
echo "============================================"
echo "              TEST SUMMARY"
echo "============================================"
echo -e "${GREEN}Passed: $PASS${NC}"
if [ "$WARN" -gt 0 ]; then
    echo -e "${YELLOW}Warned: $WARN${NC}"
fi
echo -e "${RED}Failed: $FAIL${NC}"
echo "============================================"

# Generate JSON report
REPORT_PATH="$REPORT_DIR/merged_report_$(date +%s).json"
if command -v python3 >/dev/null 2>&1; then
    echo ""
    generate_json_report "$REPORT_PATH" "$VIDEO_CODECS" "$AUDIO_CODECS" "$RESOLUTIONS" "$ASPECTS" "$FULL_MATRIX"
else
    echo ""
    echo -e "${YELLOW}⚠ python3 not found; skipping JSON report${NC}"
fi

# List streams and recordings
echo ""
echo "========== API CHECK =========="
echo "Health:   $(api_call "/api/health")"
echo "Streams:  $(api_call "/api/streams")"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
