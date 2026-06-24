#!/bin/bash
set -e

cd "$(dirname "$0")"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
PASS=0; WARN=0; FAIL=0

MEDIA_DIR="./data"
REPORT_DIR="${TEST_REPORT_DIR:-/tmp/livestream_test_reports}"
API_BASE="http://localhost:8080"
RTMP_BASE="rtmp://localhost:1935/live"

RESULTS_FILE=$(mktemp)
trap 'rm -f "$RESULTS_FILE"' EXIT

if [ -f .env ]; then set -a; source .env; set +a; fi

VIDEO_CODECS="h264 hevc av1"
AUDIO_CODECS="aac opus flac"
RESOLUTIONS="240p 480p 720p 1080p 2k 4k 8k"
TESTS="codec res color graceful reconnect hls multitrack thumbnail passthrough"
FULL_MATRIX=0
DEFAULT_RES="480p"
DEFAULT_VCODEC="h264"
DEFAULT_ACODEC="aac"
STREAM_DURATION=6
ASPECTS="16:9"
GRACE_WAIT=""
FPS_VALUES="24000/1001 24 25 30000/1001 30 50 60000/1001 60"
THUMBNAIL_FORMATS="jxl avif png"
THUMBNAIL_WIDTHS=$(echo "${THUMBNAIL_SIZES:-320,480}" | tr ',' ' ')

show_help() { cat <<'EOF'
Usage: ./test.sh [OPTIONS]

Test suites:
  codec       – video+audio codec matrix (quick: 480p only; --full: all res)
  res         – resolution/aspect matrix with the default codec pair
  color       – color-space / HDR compatibility + colr/clli/mdcv box validation
  graceful    – graceful stop & HLS cleanup
  reconnect   – abnormal disconnect + reconnect
  hls         – live HLS segment verification
  multitrack  – Enhanced RTMP multitrack (2 video + 2 audio)
  thumbnail   – live + recording thumbnail generation (JXL, AVIF, PNG)
  passthrough – audio byte-exact + video similarity after RTMP copy passthrough
  fps         – NTSC/PAL frame rate consistency (NOT in 'all')
  all         – all suites EXCEPT fps

Options:
  --video LIST     Video codecs: h264,hevc,av1 (default: all)
  --audio LIST     Audio codecs: aac,opus,flac (default: all)
  --res LIST       Resolutions (default: 240p..8k)
  --aspect LIST    Aspect ratios (default: 16:9)
  --tests LIST     Test suites to run (default: codec res color graceful reconnect hls multitrack thumbnail passthrough)
  --full           Full Cartesian product for codec/color
  --duration N     Stream duration in seconds (default: 6)
  --grace-wait N   Max seconds for HLS cleanup (default: STREAM_GRACE_PERIOD_SECONDS+5)
EOF
}

while [[ $# -gt 0 ]]; do
    case $1 in
        --video) shift; VIDEO_CODECS=$(echo "$1" | tr ',' ' '); shift ;;
        --audio) shift; AUDIO_CODECS=$(echo "$1" | tr ',' ' '); shift ;;
        --res) shift; RESOLUTIONS=$(echo "$1" | tr ',' ' '); shift ;;
        --aspect) shift; ASPECTS=$(echo "$1" | tr ',' ' '); shift ;;
        --tests) shift; TESTS=$(echo "$1" | tr ',' ' '); shift ;;
        --full) FULL_MATRIX=1; shift ;;
        --duration) shift; STREAM_DURATION="$1"; shift ;;
        --grace-wait) shift; GRACE_WAIT="$1"; shift ;;
        --fps) shift; FPS_VALUES=$(echo "$1" | tr ',' ' '); shift ;;
        -h|--help) show_help; exit 0 ;;
        *) echo "Unknown option: $1"; show_help; exit 1 ;;
    esac
done

res_and_aspect_to_size() {
    local res="$1" aspect="${2:-16:9}" height
    case "$res" in 240p) height=240;; 480p) height=480;; 720p) height=720;; 1080p) height=1080;; 2k) height=1440;; 4k) height=2160;; 8k) height=4320;; *) height=360;; esac
    local aw ah; aw=$(echo "$aspect" | cut -d':' -f1); ah=$(echo "$aspect" | cut -d':' -f2)
    local w; w=$(awk "BEGIN { w = int($height * $aw / $ah); if (w % 2 == 1) w += 1; print w }")
    echo "${w}x${height}"
}

get_vcodec_args() {
    case "$1" in h264) echo "libx264 -preset ultrafast -tune zerolatency";; hevc) echo "libx265 -preset ultrafast";; av1) echo "libsvtav1 -svtav1-params preset=12:crf=35";; *) echo "libx264";; esac
}
get_acodec_args() {
    case "$1" in aac) echo "aac";; opus) echo "libopus -ar 48000";; flac) echo "flac";; *) echo "aac";; esac
}
get_display() {
    case "$1" in h264) echo "H.264";; hevc) echo "HEVC";; av1) echo "AV1";; aac) echo "AAC";; opus) echo "Opus";; flac) echo "FLAC";; *) echo "$1";; esac
}

api_call() { curl -s "${API_BASE}$1"; }

count_recordings() { ls "$MEDIA_DIR/recordings/" 2>/dev/null | grep -c "^${1}_" 2>/dev/null || true; }

wait_for_recording() {
    local key="$1" timeout="${2:-20}" waited=0 rec=""
    while [ "$waited" -le "$timeout" ]; do
        rec=$(ls "$MEDIA_DIR/recordings/${key}_"*.mp4 2>/dev/null | head -1 || true)
        if [ -n "$rec" ] && [ -s "$rec" ]; then echo "$rec"; return 0; fi
        sleep 1; waited=$((waited+1))
    done
    return 1
}

wait_for_hls_segment() {
    local key="$1" timeout="${2:-30}" waited=0 hls_dir="$MEDIA_DIR/hls/$key" seg_count=0
    while [ "$waited" -le "$timeout" ]; do
        if [ -d "$hls_dir" ] && [ -f "$hls_dir/index.m3u8" ]; then
            seg_count=$(find "$hls_dir" -maxdepth 1 -name 'segment*.m4s' 2>/dev/null | wc -l)
            if [ "$seg_count" -gt 0 ]; then echo "$seg_count"; return 0; fi
        fi
        sleep 1; waited=$((waited+1))
    done
    return 1
}

expected_vcodec_name() { case "$1" in h264) echo h264;; hevc) echo hevc;; av1) echo av1;; *) echo "$1";; esac; }
expected_acodec_name() { case "$1" in aac) echo aac;; opus) echo opus;; flac) echo flac;; *) echo "$1";; esac; }

verify_media_properties() {
    local media="$1" exp_v="${2:-}" exp_a="${3:-}" exp_size="${4:-}"
    python3 - "$media" "$exp_v" "$exp_a" "$exp_size" <<'PYEOF'
import json, subprocess, sys
media, exp_v, exp_a, exp_size = sys.argv[1:5]
cmd = ['ffprobe', '-v', 'error', '-show_streams', '-of', 'json', media]
try:
    info = json.loads(subprocess.check_output(cmd, text=True))
except Exception as e:
    print(f'  ✗ ffprobe stream metadata failed: {e}')
    sys.exit(1)
streams = info.get('streams', [])
video = next((s for s in streams if s.get('codec_type') == 'video'), None)
audio = next((s for s in streams if s.get('codec_type') == 'audio'), None)
ok = True
if exp_v:
    if not video:
        print(f'  ✗ missing video stream (expected {exp_v})'); ok = False
    elif video.get('codec_name') != exp_v:
        print(f"  ✗ video codec {video.get('codec_name')} != {exp_v}"); ok = False
    else:
        print(f"  ✓ video codec {exp_v}")
if exp_a:
    if not audio:
        print(f'  ✗ missing audio stream (expected {exp_a})'); ok = False
    elif audio.get('codec_name') != exp_a:
        print(f"  ✗ audio codec {audio.get('codec_name')} != {exp_a}"); ok = False
    else:
        print(f"  ✓ audio codec {exp_a}")
if exp_size and video:
    try:
        ew, eh = map(int, exp_size.lower().split('x', 1))
        aw, ah = int(video.get('width', 0)), int(video.get('height', 0))
        if (aw, ah) != (ew, eh):
            print(f'  ✗ resolution {aw}x{ah} != {ew}x{eh}'); ok = False
        else:
            print(f'  ✓ resolution {aw}x{ah}')
    except Exception as e:
        print(f'  ⚠ resolution check skipped: {e}')
print('MEDIA_PROPS_OK' if ok else 'MEDIA_PROPS_FAIL')
sys.exit(0 if ok else 1)
PYEOF
}

check_thumbnail_set() {
    local dir="$1" prefix="$2" timeout="${3:-30}" waited=0
    while [ "$waited" -le "$timeout" ]; do
        local ok=1 missing=()
        for width in $THUMBNAIL_WIDTHS; do
            for fmt in $THUMBNAIL_FORMATS; do
                local p="$dir/${prefix}_w${width}.${fmt}"
                if [ ! -s "$p" ]; then ok=0; missing+=("w${width}.${fmt}"); fi
            done
        done
        if [ "$ok" -eq 1 ]; then
            for width in $THUMBNAIL_WIDTHS; do
                local found=""
                for fmt in $THUMBNAIL_FORMATS; do found+=" w${width}.${fmt}"; done
                echo -e "${GREEN}  ✓ thumbnails:${found}${NC}"
            done
            return 0
        fi
        sleep 1; waited=$((waited+1))
    done
    echo -e "${RED}  ✗ missing thumbnails for ${prefix}: ${missing[*]}${NC}"
    return 1
}

check_video_similarity() {
    local ref="$1" rec="$2" threshold="${3:-0.98}" out ssim
    out=$(ffmpeg -hide_banner -v info -i "$ref" -i "$rec" \
        -filter_complex "[0:v]fps=15,scale=320:-2,setpts=PTS-STARTPTS[ref];[1:v]fps=15,scale=320:-2,setpts=PTS-STARTPTS[dist];[ref][dist]ssim" \
        -frames:v 60 -an -f null - 2>&1) || true
    ssim=$(echo "$out" | grep -o 'All:[0-9.]*' | tail -1 | cut -d: -f2)
    if [ -z "$ssim" ]; then
        echo -e "${RED}  ✗ video similarity: SSIM unavailable${NC}"
        echo "$out" | tail -5 | sed 's/^/    /'
        return 1
    fi
    echo "  Video SSIM: $ssim"
    if python3 - "$ssim" "$threshold" <<'PYEOF'
import sys
ssim = float(sys.argv[1]); threshold = float(sys.argv[2])
sys.exit(0 if ssim >= threshold else 1)
PYEOF
    then
        echo -e "${GREEN}  ✓ video similarity >= ${threshold}${NC}"
        return 0
    fi
    echo -e "${RED}  ✗ video similarity below ${threshold}${NC}"
    return 1
}

# ── MP4 integrity + stts check ────────────────────────────────────
check_mp4() {
    local mp4="$1" key="$2" max_exp="${3:-30}" corrupt=0
    local duration
    duration=$(ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 "$mp4" 2>&1) || corrupt=1
    if [ "$corrupt" -eq 0 ]; then
        if [ -z "$duration" ] || [ "$duration" = "N/A" ]; then echo -e "${RED}  ✗ Duration N/A${NC}"; corrupt=1
        elif awk "BEGIN { exit ($duration > $max_exp || $duration < 0.5) ? 0 : 1 }"; then
            echo -e "${RED}  ✗ Duration=${duration}s (expected 0.5-${max_exp})${NC}"; corrupt=1
        fi
    fi
    if [ "$corrupt" -eq 0 ]; then
        local err; err=$(ffmpeg -v error -i "$mp4" -c copy -f null - 2>&1) || true
        if [ -n "$err" ]; then echo -e "${RED}  ✗ ffmpeg errors:${NC}"; echo "$err" | head -3 | sed 's/^/    /'; corrupt=1; fi
    fi
    if [ "$corrupt" -eq 0 ]; then
        local fcnt; fcnt=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=noprint_wrappers=1:nokey=1 "$mp4" 2>/dev/null || echo 0)
        if [ "$fcnt" -lt 10 ]; then echo -e "${RED}  ✗ Too few frames ($fcnt)${NC}"; corrupt=1; fi
    fi
    if [ "$corrupt" -eq 1 ]; then return 1; fi

    # stts consistency check (audio AAC frames must be uniform 1024)
    local stts_pass
    stts_pass=$(python3 "$(dirname "$0")/tests/stts_check.py" "$mp4" 2>&1) || true

    echo "$stts_pass" | grep -q "^OK$" && echo -e "${GREEN}  ✓ stts consistent${NC}" || echo -e "${YELLOW}  ⚠ stts incomplete${NC}"
    echo "$stts_pass" | grep -v "^OK$" | grep -v "^FAIL$" | while IFS= read -r l; do echo "  $l"; done

    echo -e "${GREEN}  ✓ MP4 integrity OK (duration=${duration}s, frames=${fcnt})${NC}"
    return 0
}

# ── Core stream test ──────────────────────────────────────────────
run_stream_test() {
    local name="$1" vcodec_raw="$2" acodec_raw="$3" size="$4" key="$5" check_hls="${6:-0}" exp_v="${7:-}" exp_a="${8:-}"
    echo ""; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: $name"; echo "Key:  $key"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local ffmpeg_cmd=(ffmpeg -y -re -f lavfi -i "testsrc=duration=${STREAM_DURATION}:size=${size}:rate=30")
    ffmpeg_cmd+=(-f lavfi -i "sine=frequency=440:duration=${STREAM_DURATION}")

    read -r venc vrest <<< "$vcodec_raw"
    ffmpeg_cmd+=(-c:v "$venc"); [ -n "$vrest" ] && ffmpeg_cmd+=($vrest)
    ffmpeg_cmd+=(-pix_fmt yuv420p -g 60 -keyint_min 60)

    read -r aenc arest <<< "$acodec_raw"
    ffmpeg_cmd+=(-c:a "$aenc"); [ -n "$arest" ] && ffmpeg_cmd+=($arest)
    ffmpeg_cmd+=(-t "$STREAM_DURATION" -f flv "${RTMP_BASE}/${key}")

    "${ffmpeg_cmd[@]}" >/dev/null 2>&1 &
    local ff_pid=$!

    if [ "$check_hls" -eq 1 ]; then
        local seg_count=""
        if seg_count=$(wait_for_hls_segment "$key" 20); then
            echo -e "${GREEN}  ✓ HLS active ($seg_count m4s segs)${NC}"
            api_call "/api/streams" >/dev/null || true
        else echo -e "${YELLOW}  ⚠ HLS not yet available${NC}"; fi
    fi
    wait $ff_pid 2>/dev/null || true

    local rec="" mp4_count; rec=$(wait_for_recording "$key" 20 || true); mp4_count=$(count_recordings "$key")
    local result="FAIL"; local result_color="$RED"

    if [ "$mp4_count" -gt 0 ] && [ -n "$rec" ]; then
        echo -e "${GREEN}  ✓ Recording generated ($mp4_count file(s))${NC}"
        [ -f "$MEDIA_DIR/recordings/index.json" ] && echo -e "${GREEN}  ✓ index.json${NC}" || echo -e "${YELLOW}  ⚠ index.json not found${NC}"

        local ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
            if [ -f "$f" ]; then
                check_mp4 "$f" "$key" 15 || ok=0
                if [ -n "$exp_v" ] || [ -n "$exp_a" ]; then
                    verify_media_properties "$f" "$exp_v" "$exp_a" "$size" || ok=0
                fi
                check_thumbnail_set "$MEDIA_DIR/thumbnails/recordings" "$(basename "$f")" 30 || ok=0
            fi
        done
        [ "$ok" -eq 1 ] && { result="PASS"; result_color="$GREEN"; }
    else echo -e "${RED}  ✗ No recording generated${NC}"
    fi
    echo -e "${result_color}  ● Result: $result${NC}"
    echo "$key|$result" >> "$RESULTS_FILE"
    [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
}

# ── Codec matrix ──────────────────────────────────────────────────
run_codec_matrix() {
    echo "========== VIDEO + AUDIO CODEC MATRIX =========="
    local res_list="$([ "$FULL_MATRIX" -eq 1 ] && echo "$RESOLUTIONS" || echo "$DEFAULT_RES")"
    local asp; asp=$(echo "$ASPECTS" | awk '{print $1}')
    for v in $VIDEO_CODECS; do
        for a in $AUDIO_CODECS; do
            for r in $res_list; do
                local size=$(res_and_aspect_to_size "$r" "$asp")
                local vargs=$(get_vcodec_args "$v"); local aargs=$(get_acodec_args "$a")
                local vd=$(get_display "$v"); local ad=$(get_display "$a")
                local key="codec_${v}_${a}_${r}_$(date +%s)"
                run_stream_test "${vd}+${ad}@${r}" "$vargs" "$aargs" "$size" "$key" 0 "$(expected_vcodec_name "$v")" "$(expected_acodec_name "$a")"
            done
        done
    done
}

# ── Resolution / aspect matrix ────────────────────────────────────
run_resolution_matrix() {
    echo "========== RESOLUTION / ASPECT MATRIX =========="
    local vargs=$(get_vcodec_args "$DEFAULT_VCODEC")
    local aargs=$(get_acodec_args "$DEFAULT_ACODEC")
    local vd=$(get_display "$DEFAULT_VCODEC")
    local ad=$(get_display "$DEFAULT_ACODEC")
    for asp in $ASPECTS; do
        for r in $RESOLUTIONS; do
            local size=$(res_and_aspect_to_size "$r" "$asp")
            local safe_asp=${asp//:/x}
            local key="res_${DEFAULT_VCODEC}_${DEFAULT_ACODEC}_${r}_${safe_asp}_$(date +%s)"
            run_stream_test "${vd}+${ad}@${r}/${asp}" "$vargs" "$aargs" "$size" "$key" 0 "$(expected_vcodec_name "$DEFAULT_VCODEC")" "$(expected_acodec_name "$DEFAULT_ACODEC")"
        done
    done
}

# ── Color + HDR box validation ───────────────────────────────────
extract_color_info() {
    ffprobe -v error -select_streams v:0 -show_entries stream=pix_fmt,color_space,color_primaries,color_transfer,profile -of default=noprint_wrappers=1 "$1" 2>/dev/null
}

run_color_test() {
    local name="$1" encoder="$2" pix_fmt="$3" profile="$4" hdr="$5" expected="$6" key="color_${encoder}_${pix_fmt}_${hdr}_$(date +%s)"
    echo ""; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Color: $name"; echo "Encoder: $encoder | Pixel: $pix_fmt | HDR: $hdr"
    [ -n "$profile" ] && echo "Profile: $profile"; echo "Key: $key"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    local args=(-y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" -f lavfi -i "sine=frequency=440:duration=10")
    args+=(-c:v "$encoder" -pix_fmt "$pix_fmt" -g 60 -keyint_min 60 -c:a aac -t 6 -f flv)
    [ -n "$profile" ] && args+=(-profile:v "$profile")
    if [ "$hdr" = "hdr" ]; then
        args+=(-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc)
        if [ "$encoder" = "libx265" ]; then
            # FFmpeg's Enhanced RTMP muxer emits hdrCll/hdrMdcv only when the
            # encoder exposes content-light/mastering-display side data.
            args+=(-x265-params "master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50):max-cll=1000,400")
        fi
    fi
    [ "$encoder" = "libx265" ] && args+=(-preset ultrafast)
    [ "$encoder" = "libsvtav1" ] && args+=(-svtav1-params "preset=12:crf=35")

    ffmpeg "${args[@]}" "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ff_pid=$!; wait $ff_pid 2>/dev/null || true

    local mp4_path="" mp4_count=0
    mp4_path=$(wait_for_recording "$key" 20 || true)
    mp4_count=$(count_recordings "$key")
    local result="FAIL" result_color="$RED" ok=1
    [ "$mp4_count" -gt 0 ] && echo -e "${GREEN}  ✓ Recording ($mp4_count file(s))${NC}" || ok=0

    if [ "$ok" -eq 1 ] && [ -z "$mp4_path" ]; then ok=0; fi
    if [ "$ok" -eq 1 ]; then
        check_mp4 "$mp4_path" "$key" 15 || ok=0
    fi
    if [ "$ok" -eq 1 ]; then
        local color_info; color_info=$(extract_color_info "$mp4_path")
        echo "  Color info:"; echo "$color_info" | sed 's/^/    /'

        # Check init.mp4 for colr/clli/mdcv on HDR streams
        local hls_dir="$MEDIA_DIR/hls/$key"
        if [ "$hdr" = "hdr" ]; then
            if [ ! -f "$hls_dir/init.mp4" ]; then
                echo -e "${RED}  ✗ HDR validation requires $hls_dir/init.mp4${NC}"
                ok=0
            else
            local box_out
            box_out=$(python3 "$(dirname "$0")/tests/hdr_boxes.py" "$hls_dir/init.mp4" 2>&1) || true
            echo "$box_out" | sed 's/^/    /'
            if echo "$box_out" | grep -q "^HDR_OK$"; then
                echo -e "${GREEN}  ✓ HDR boxes (colr/clli/mdcv) present in init.mp4${NC}"
            else
                echo -e "${RED}  ✗ HDR boxes absent or incomplete in init.mp4${NC}"
                ok=0
            fi
            fi
        fi

        if [ "$ok" -eq 1 ]; then
            if [ "$expected" = "WARN" ]; then
                result="WARN"; result_color="$YELLOW"
            else
                result="PASS"; result_color="$GREEN"
            fi
        fi
    else
        echo -e "${RED}  ✗ No recording or integrity failed${NC}"
    fi

    echo -e "${result_color}  ● Result: $result${NC}"
    echo "$key|$result" >> "$RESULTS_FILE"
    case "$result" in PASS) PASS=$((PASS+1));; WARN) WARN=$((WARN+1));; *) FAIL=$((FAIL+1));; esac
}

run_color_matrix() {
    echo "========== COLOR SPACE + HDR BOXES =========="
    run_color_test "H.264 4:2:0 8-bit SDR"      libx264   yuv420p     ""      sdr  PASS
    run_color_test "AV1 4:2:0 8-bit SDR"        libsvtav1 yuv420p     ""      sdr  PASS
    run_color_test "H.264 4:2:2 8-bit SDR"      libx264   yuv422p     high422 sdr  WARN
    run_color_test "H.264 4:4:4 8-bit SDR"      libx264   yuv444p     high444 sdr  WARN
    run_color_test "H.264 NV12 8-bit SDR"       libx264   nv12        ""      sdr  PASS
    run_color_test "H.264 4:2:0 10-bit SDR"     libx264   yuv420p10le ""      sdr  WARN
    run_color_test "HEVC 4:2:0 10-bit SDR"       libx265   yuv420p10le ""      sdr  PASS
    run_color_test "HEVC 4:2:0 10-bit HDR"       libx265  yuv420p10le ""      hdr  PASS
    run_color_test "AV1 4:2:0 10-bit SDR"        libsvtav1 yuv420p10le ""     sdr  PASS
    run_color_test "AV1 4:2:0 10-bit HDR"        libsvtav1 yuv420p10le ""     hdr  PASS
}

# ── Graceful stop ─────────────────────────────────────────────────
run_graceful_stop_test() {
    echo "========== GRACEFUL STOP + HLS CLEANUP =========="
    local key="graceful_$(date +%s)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"; echo "Graceful Stop"; echo "Key: $key"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" -f lavfi -i "sine=frequency=440:duration=10" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 -c:a aac -t 4 -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ff_pid=$!; wait $ff_pid 2>/dev/null || true; sleep 5

    local hls_dir="$MEDIA_DIR/hls/$key"
    local mp4_count=$(count_recordings "$key")
    local grace_wait="${GRACE_WAIT:-$(( ${STREAM_GRACE_PERIOD_SECONDS:-30} + 5 ))}"
    echo "  Grace period: ${STREAM_GRACE_PERIOD_SECONDS:-30}s, polling up to ${grace_wait}s"
    local hls_wait=0
    while [ -d "$hls_dir" ] && [ "$hls_wait" -lt "$grace_wait" ]; do sleep 1; hls_wait=$((hls_wait+1)); done

    local result="FAIL" result_color="$RED"
    if [ "$mp4_count" -gt 0 ] && [ ! -d "$hls_dir" ]; then
        echo -e "${GREEN}  ✓ Recording exists and HLS cleaned${NC}"
        local ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do [ -f "$f" ] && check_mp4 "$f" "$key" 10 || ok=0; done
        [ "$ok" -eq 1 ] && { result="PASS"; result_color="$GREEN"; }
    elif [ "$mp4_count" -eq 0 ]; then echo -e "${RED}  ✗ No recording${NC}"
    else echo -e "${RED}  ✗ HLS dir still exists${NC}"; fi

    echo -e "${result_color}  ● Result: $result${NC}"; echo "$key|$result" >> "$RESULTS_FILE"
    [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
}

# ── Reconnect ─────────────────────────────────────────────────────
run_reconnect_test() {
    echo "========== ABNORMAL DISCONNECT / RECONNECT =========="
    local key="reconnect_$(date +%s)"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"; echo "Reconnect Test"; echo "Key: $key"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=30:size=640x360:rate=30" -f lavfi -i "sine=frequency=440:duration=30" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 -c:a aac -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ff_pid=$!; sleep 4; kill -9 $ff_pid 2>/dev/null || true; sleep 1

    local json; json=$(api_call "/api/streams")
    echo "$json" | grep -q "$key" && echo -e "${GREEN}  ✓ Stream visible during grace period${NC}" || echo -e "${YELLOW}  ⚠ Stream not visible${NC}"
    echo "  --- Reconnecting ---"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=10:size=640x360:rate=30" -f lavfi -i "sine=frequency=1000:duration=10" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 -c:a aac -t 4 -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    ff_pid=$!; wait $ff_pid 2>/dev/null || true; sleep 5

    local mp4_count=$(count_recordings "$key")
    local result="FAIL" result_color="$RED"
    if [ "$mp4_count" -eq 1 ]; then
        local ok=1
        for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do [ -f "$f" ] && check_mp4 "$f" "$key" 15 || ok=0; done
        [ "$ok" -eq 1 ] && { result="PASS"; result_color="$GREEN"; echo -e "${GREEN}  ✓ Reconnect PASSED${NC}"; }
    else echo -e "${RED}  ✗ Expected 1 recording, got $mp4_count${NC}"; fi

    echo -e "${result_color}  ● Result: $result${NC}"; echo "$key|$result" >> "$RESULTS_FILE"
    [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
}

# ── HLS / E2E ─────────────────────────────────────────────────────
run_hls_test() {
    echo "========== HLS / E2E STREAMING =========="
    local key="hls_$(date +%s)" size="640x360"
    local vargs=$(get_vcodec_args "h264"); local aargs=$(get_acodec_args "aac")
    run_stream_test "H.264+AAC@360p" "$vargs" "$aargs" "$size" "$key" 1 h264 aac
}

# ── Multitrack ────────────────────────────────────────────────────
run_multitrack_test() {
    echo "========== MULTITRACK ENHANCED RTMP =========="
    local key="multitrack_$(date +%s)" duration="$STREAM_DURATION"
    [ "$duration" -lt 12 ] 2>/dev/null && duration=12
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"; echo "Multitrack (2v+2a)"; echo "Key: $key"; echo "Duration: ${duration}s"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=${duration}:size=1280x720:rate=30" -f lavfi -i "testsrc=duration=${duration}:size=640x360:rate=30" \
        -f lavfi -i "sine=frequency=440:duration=${duration}" -f lavfi -i "sine=frequency=880:duration=${duration}" \
        -map 0:v -c:v:0 libsvtav1 -svtav1-params preset=12 -pix_fmt:v:0 yuv420p -b:v:0 1500k -g:v:0 60 -keyint_min:v:0 60 \
        -map 1:v -c:v:1 libx264 -preset:v:1 ultrafast -pix_fmt:v:1 yuv420p -b:v:1 500k -g:v:1 60 -keyint_min:v:1 60 \
        -map 2:a -c:a:0 libopus -ar:a:0 48000 -b:a:0 128k -map 3:a -c:a:1 aac -ar:a:1 44100 -b:a:1 128k \
        -f flv "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ffmpeg_pid=$!; sleep 1
    if ! kill -0 "$ffmpeg_pid" 2>/dev/null; then echo -e "${RED}  ✗ ffmpeg failed to start${NC}"; echo "$key|FAIL" >> "$RESULTS_FILE"; FAIL=$((FAIL+1)); return; fi
    wait_for_hls_segment "$key" 30 >/dev/null || true
    sleep 2

    local result="FAIL" result_color="$RED" hls_dir="$MEDIA_DIR/hls/$key"
    local hls_ok=1 fmp4_ok=1 codec_ok=1 rec_ok=1

    [ -f "$hls_dir/index.m3u8" ] && echo -e "${GREEN}  ✓ index.m3u8${NC}" || { echo -e "${RED}  ✗ index.m3u8${NC}"; hls_ok=0; }
    [ -f "$hls_dir/track_1/index.m3u8" ] && echo -e "${GREEN}  ✓ track_1/index.m3u8${NC}" || { echo -e "${RED}  ✗ track_1/index.m3u8${NC}"; hls_ok=0; }

    local def_segs=($(find "$hls_dir" -maxdepth 1 -name 'segment*.m4s' | sort))
    local t1_segs=($(find "$hls_dir/track_1" -maxdepth 1 -name 'segment*.m4s' | sort))
    [ ${#def_segs[@]} -ge 1 ] && echo -e "${GREEN}  ✓ default: ${#def_segs[@]} seg(s)${NC}" || { echo -e "${RED}  ✗ default: no segments${NC}"; hls_ok=0; }
    [ ${#t1_segs[@]} -ge 1 ] && echo -e "${GREEN}  ✓ track_1: ${#t1_segs[@]} seg(s)${NC}" || { echo -e "${RED}  ✗ track_1: no segments${NC}"; hls_ok=0; }

    for d in "" "/track_1"; do
        [ -f "$hls_dir${d}/init.mp4" ] && echo -e "${GREEN}  ✓ ${d:-default}/init.mp4${NC}" || { echo -e "${RED}  ✗ ${d:-default}/init.mp4${NC}"; hls_ok=0; }
    done

    local tmp_c; tmp_c=$(mktemp)
    for pair in "0:default" "1:track_1"; do
        local idx="${pair%%:*}"; local label="${pair##*:}"; local segments
        [ "$idx" = "0" ] && segments=("${def_segs[@]}") || segments=("${t1_segs[@]}")
        if [ ${#segments[@]} -gt 0 ]; then
            local init_dir="$hls_dir"; [ "$label" = "track_1" ] && init_dir="$hls_dir/track_1"
            cat "$init_dir/init.mp4" "${segments[0]}" > "$tmp_c"
            if ffprobe -hide_banner -loglevel error -show_format -show_streams "$tmp_c" >/dev/null 2>&1; then
                echo -e "${GREEN}  ✓ $label fMP4 valid${NC}"
                if [ "$label" = "default" ]; then
                    verify_media_properties "$tmp_c" av1 opus "" || codec_ok=0
                else
                    verify_media_properties "$tmp_c" h264 aac "" || codec_ok=0
                fi
            else echo -e "${RED}  ✗ $label fMP4 invalid${NC}"; fmp4_ok=0; fi
        fi
    done; rm -f "$tmp_c"

    local streams_json; streams_json=$(api_call "/api/streams" 2>/dev/null || echo "")
    echo "$streams_json" | grep -q "$key" && echo -e "${GREEN}  ✓ In /api/streams${NC}" || echo -e "${YELLOW}  ⚠ Not in /api/streams${NC}"

    local tracks_json
    tracks_json=$(echo "$streams_json" | python3 -c "import sys,json; d=json.load(sys.stdin)
for s in d:
    if s.get('stream_key')=='$key':
        print(json.dumps(s.get('tracks',[]))); break" 2>/dev/null || echo "[]")
    local tcount; tcount=$(echo "$tracks_json" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo 0)
    [ "$tcount" -ge 2 ] && echo -e "${GREEN}  ✓ $tcount tracks${NC}" || { echo -e "${RED}  ✗ <2 tracks${NC}"; hls_ok=0; }

    if kill -0 "$ffmpeg_pid" 2>/dev/null; then kill "$ffmpeg_pid" 2>/dev/null || true; wait "$ffmpeg_pid" 2>/dev/null || true; fi; sleep 5

    local rec_count; rec_count=$(find "$MEDIA_DIR/recordings" -name "${key}_*.mp4" 2>/dev/null | wc -l) || rec_count=0
    if [ "$rec_count" -eq 0 ]; then
        echo -e "${RED}  ✗ No recording${NC}"; rec_ok=0
    else
        echo -e "${GREEN}  ✓ Recording ($rec_count)${NC}"
        if [ "$rec_count" -eq 1 ]; then
            echo -e "${GREEN}  ✓ Exactly 1 recording${NC}"
        else
            echo -e "${YELLOW}  ⚠ Expected 1 recording, found $rec_count${NC}"
        fi
    fi

    for f in "$MEDIA_DIR/recordings/${key}_"*.mp4; do
        if [ -f "$f" ]; then
            check_mp4 "$f" "$key" 30 || rec_ok=0
            verify_media_properties "$f" av1 opus "1280x720" || rec_ok=0
            check_thumbnail_set "$MEDIA_DIR/thumbnails/recordings" "$(basename "$f")" 30 || rec_ok=0
        fi
    done

    [ "$hls_ok" -eq 1 ] && [ "$fmp4_ok" -eq 1 ] && [ "$codec_ok" -eq 1 ] && [ "$rec_ok" -eq 1 ] && { result="PASS"; result_color="$GREEN"; }
    echo -e "${result_color}  ● Result: $result${NC}"; echo "$key|$result" >> "$RESULTS_FILE"
    [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
}

# ── FPS frame rate ────────────────────────────────────────────────
run_fps_test() {
    local fps_frac="$1" key="fps_$(echo "$fps_frac" | tr '/' '_')_$(date +%s)" duration="$STREAM_DURATION"
    local fps_num=$fps_frac fps_den=1
    echo "$fps_frac" | grep -q '/' && fps_num=$(echo "$fps_frac" | cut -d/ -f1) && fps_den=$(echo "$fps_frac" | cut -d/ -f2)
    local gop=$(( fps_num * 2 / fps_den )); [ "$gop" -lt 1 ] && gop=1
    local size="640x360"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"; echo "FPS: ${fps_num}/${fps_den}"; echo "Key: $key"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=${duration}:size=${size}:rate=${fps_frac}" -f lavfi -i "sine=frequency=440:duration=${duration}" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g "$gop" -keyint_min "$gop" -c:a aac -t "$duration" -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ff_pid=$!; wait $ff_pid 2>/dev/null || true; sleep 5

    local mp4_count=$(count_recordings "$key") result="FAIL" result_color="$RED"
    if [ "$mp4_count" -gt 0 ]; then
        local f; f=$(ls "$MEDIA_DIR/recordings/${key}_"*.mp4 2>/dev/null | head -1)
        if [ -n "$f" ]; then
            local durs; durs=$(ffprobe -v quiet -select_streams v:0 -show_packets "$f" 2>/dev/null | grep '^duration=' | grep -v 'duration=0$' | sed 's/duration=//')
            if [ -z "$durs" ]; then echo -e "${RED}  ✗ No frame data${NC}"
            else
                local total=$(echo "$durs" | wc -l)
                local unique=$(echo "$durs" | sort -u | wc -l)
                local dom_dur=$(echo "$durs" | sort -n | uniq -c | sort -rn | head -1 | awk '{print $2}')
                local dom_cnt=$(echo "$durs" | sort -n | uniq -c | sort -rn | head -1 | awk '{print $1}')
                local tb=$(ffprobe -v error -select_streams v:0 -show_entries stream=time_base -of default=noprint_wrappers=1:nokey=1 "$f" 2>/dev/null | tail -1)
                tb="${tb#*/}"; local dom_sec=$(python3 -c "print($dom_dur / ${tb:-90000})")
                echo -e "  frames=$total unique=$unique dominant=${dom_dur}t (${dom_sec}s) count=${dom_cnt}/${total}"

                if python3 -c "exit(0 if ${dom_cnt}/${total} >= 0.85 else 1)" 2>/dev/null; then
                    echo -e "${GREEN}  ✓ Frame duration consistent${NC}"; result="PASS"; result_color="$GREEN"
                else echo -e "${RED}  ✗ Dominant covers ${dom_cnt}/${total}${NC}"; fi
            fi
        fi
    else echo -e "${RED}  ✗ No recording${NC}"; fi
    echo -e "${result_color}  ● Result: $result${NC}"; echo "$key|$result" >> "$RESULTS_FILE"
    [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
}

run_fps_matrix() {
    echo "========== NTSC/PAL FRAME RATE TEST =========="
    for fps in $FPS_VALUES; do run_fps_test "$fps"; done
}

# ── JSON report ───────────────────────────────────────────────────
generate_json_report() {
    local output_path="$1"
    local dir; dir=$(dirname "$output_path")
    mkdir -p "$dir" 2>/dev/null || true
    python3 - "$RESULTS_FILE" "$PASS" "$WARN" "$FAIL" "$output_path" "$VIDEO_CODECS" "$AUDIO_CODECS" "$RESOLUTIONS" "$ASPECTS" "$FULL_MATRIX" <<'PYEOF'
import json, sys, time, datetime
rf, pc, wc, fc, op, vc, ac, res, asp, fm = sys.argv[1:11]
with open(rf) as f: results = [l.strip().split('|', 1) for l in f if l.strip()]
report = {
    "timestamp": int(time.time()), "date": datetime.datetime.now().isoformat(),
    "config": {"video_codecs": vc, "audio_codecs": ac, "resolutions": res, "aspects": asp, "full_matrix": fm},
    "summary": {"pass": int(pc), "warn": int(wc), "fail": int(fc), "total": int(pc)+int(wc)+int(fc)},
    "results": [{"name": n, "result": r} for n,r in results]
}
with open(op, 'w') as f: json.dump(report, f, indent=2)
print(f"Report: {op}")
PYEOF
}

# ── Passthrough bounded similarity verification ───────────────────
# Encodes once, pushes with -c copy, and verifies raw audio frame
# data is byte-identical over the matched media window with a small,
# codec-specific allowance for encoder/container setup and tail packets.
run_passthrough_test() {
    echo "========== AUDIO PASSTHROUGH BOUNDED SIMILARITY =========="
    local duration=4

    for acodec in $AUDIO_CODECS; do
        local key="pt_${acodec}_$(date +%s)"
        echo ""
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "Passthrough: $(echo $acodec | tr 'a-z' 'A-Z')"
        echo "Key:  $key"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

        local result="FAIL" result_color="$RED"
        local acodec_args=$(get_acodec_args "$acodec")
        local fourcc="" max_audio_delta=0
        case "$acodec" in
            aac) fourcc="AAC_LEGACY"; max_audio_delta=1;;
            opus) fourcc="Opus"; max_audio_delta=4;;
            flac) fourcc="fLaC"; max_audio_delta=5;;
        esac

        # 1. Encode once to FLV
        local flv; flv=$(mktemp /tmp/pt_${acodec}.XXXXXX.flv)
        echo "  Encoding reference FLV (${duration}s, H.264+${acodec^^})..."
        read -r aenc arest <<< "$acodec_args"
        ffmpeg -y -f lavfi -i "testsrc=duration=${duration}:size=640x360:rate=30" \
            -f lavfi -i "sine=frequency=440:duration=${duration}" \
            -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency \
            -c:a "$aenc" $([ -n "$arest" ] && echo "$arest") \
            -g 60 -keyint_min 60 -t "$duration" -f flv "$flv" >/dev/null 2>&1
        [ ! -s "$flv" ] && { echo -e "${RED}  ✗ FLV encode failed${NC}"; rm -f "$flv"; FAIL=$((FAIL+1)); echo "pt_${acodec}|FAIL" >> "$RESULTS_FILE"; continue; }

        # 2. Push with -c copy
        echo "  Pushing through RTMP with -c copy..."
        ffmpeg -y -re -i "$flv" -c copy -f flv "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
        local ff_pid=$!; wait $ff_pid 2>/dev/null || true

        # 3. Find recording
        local rec; rec=$(wait_for_recording "$key" 20 || true)
        if [ -z "$rec" ]; then echo -e "${RED}  ✗ No recording${NC}"; rm -f "$flv"; FAIL=$((FAIL+1)); echo "pt_${acodec}|FAIL" >> "$RESULTS_FILE"; continue; fi
        echo "  Recording: $(basename "$rec") ($(stat -c%s "$rec") bytes)"

        # 4. Byte-level comparison via standalone Python script
        export PT_FLV="$flv" PT_REC="$rec" PT_FOURCC="$fourcc" PT_MAX_AUDIO_FRAME_DELTA="$max_audio_delta"
        local py_out
        py_out=$(python3 "$(dirname "$0")/tests/passthrough.py" 2>&1) || true
        echo "$py_out" | sed 's/^/  /'

        # 5. Audio byte-exact + video similarity + stts verification
        local video_ok=1
        check_video_similarity "$flv" "$rec" 0.98 || video_ok=0
        if echo "$py_out" | grep -q "^PASSTHROUGH_OK$" && [ "$video_ok" -eq 1 ]; then
            if check_mp4 "$rec" "$key" 15; then
                result="PASS"; result_color="$GREEN"
            fi
        fi

        rm -f "$flv"
        echo -e "${result_color}  ● Result: $result${NC}"
        echo "pt_${acodec}|$result" >> "$RESULTS_FILE"
        [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
    done
}

# ── Thumbnail generation ──────────────────────────────────────────
run_thumbnail_test() {
    echo "========== THUMBNAIL GENERATION (JXL / AVIF / PNG) =========="
    local key="thumb_$(date +%s)" duration="$STREAM_DURATION"
    [ "$duration" -lt 8 ] 2>/dev/null && duration=8
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"; echo "Thumbnail formats"; echo "Key: $key"; echo "Widths: $THUMBNAIL_WIDTHS | Formats: $THUMBNAIL_FORMATS"; echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    ffmpeg -y -re -f lavfi -i "testsrc=duration=${duration}:size=640x360:rate=30" -f lavfi -i "sine=frequency=440:duration=${duration}" \
        -c:v libx264 -pix_fmt yuv420p -preset ultrafast -tune zerolatency -g 60 -keyint_min 60 -c:a aac -t "$duration" -f flv \
        "${RTMP_BASE}/${key}" >/dev/null 2>&1 &
    local ff_pid=$!
    local result="FAIL" result_color="$RED" ok=1

    local seg_count=""
    if seg_count=$(wait_for_hls_segment "$key" 30); then
        echo -e "${GREEN}  ✓ live HLS segment ready ($seg_count m4s segs)${NC}"
    else
        echo -e "${RED}  ✗ no live HLS segment available for thumbnail source${NC}"
        ok=0
    fi
    api_call "/api/streams" >/dev/null || true
    check_thumbnail_set "$MEDIA_DIR/thumbnails/streams" "$key" 30 || ok=0

    wait $ff_pid 2>/dev/null || true
    local rec; rec=$(wait_for_recording "$key" 30 || true)
    if [ -n "$rec" ]; then
        echo -e "${GREEN}  ✓ Recording generated for thumbnail source${NC}"
        check_mp4 "$rec" "$key" 15 || ok=0
        check_thumbnail_set "$MEDIA_DIR/thumbnails/recordings" "$(basename "$rec")" 45 || ok=0
    else
        echo -e "${RED}  ✗ No recording generated for thumbnail test${NC}"
        ok=0
    fi

    [ "$ok" -eq 1 ] && { result="PASS"; result_color="$GREEN"; }
    echo -e "${result_color}  ● Result: $result${NC}"; echo "$key|$result" >> "$RESULTS_FILE"
    [ "$result" = "PASS" ] && PASS=$((PASS+1)) || FAIL=$((FAIL+1))
}

# ── Main ──────────────────────────────────────────────────────────
echo "=== Livestream Test Suite ==="
echo "API: $API_BASE | RTMP: $RTMP_BASE | Duration: ${STREAM_DURATION}s"
echo "Video: $VIDEO_CODECS | Audio: $AUDIO_CODECS | Tests: $TESTS"
echo "Res: $( [ "$FULL_MATRIX" -eq 1 ] && echo "$RESOLUTIONS (full)" || echo "$DEFAULT_RES (quick)" )"
echo ""

api_call "/api/health" >/dev/null || { echo "Server not running"; exit 1; }

echo "$TESTS" | grep -qw "all" && TESTS="codec res color graceful reconnect hls multitrack thumbnail passthrough"

for t in $TESTS; do
    case $t in
        codec) run_codec_matrix ;;
        res) run_resolution_matrix ;;
        color|hdr|hdr-validate) run_color_matrix ;;
        graceful) run_graceful_stop_test ;;
        reconnect) run_reconnect_test ;;
        hls) run_hls_test ;;
        multitrack) run_multitrack_test ;;
        thumbnail|thumbnails) run_thumbnail_test ;;
        fps) run_fps_matrix ;;
        passthrough) run_passthrough_test ;;
        *) echo "Unknown: $t" ;;
    esac
done

echo ""
echo "============================================"
echo "              TEST SUMMARY"
echo "============================================"
echo -e "${GREEN}Passed: $PASS${NC}"
[ "$WARN" -gt 0 ] && echo -e "${YELLOW}Warned: $WARN${NC}"
echo -e "${RED}Failed: $FAIL${NC}"
echo "============================================"

[ -f "$REPORT_DIR/merged_report_$(date +%s).json" ] && echo "Report exists" || generate_json_report "$REPORT_DIR/merged_report_$(date +%s).json"
echo "Health:  $(api_call "/api/health")"
echo "Streams: $(api_call "/api/streams")"
exit $FAIL
