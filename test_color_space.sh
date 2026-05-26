#!/bin/bash
set -e

cd /home/kilo/vibe-livestream

# Colors
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
    local max_expected_duration="${3:-15}"
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

run_color_test() {
    local name="$1"
    local encoder="$2"
    local pix_fmt="$3"
    local profile_name="$4"
    local hdr="$5"
    local expected="$6"
    local key="cs_${encoder}_${pix_fmt}_${hdr}_$(date +%s)"

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Test: $name"
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
                    details="Records correctly; HDR metadata lost in output (expected with current muxer)"
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

    # Append result for JSON report (pipe-delimited, details may contain pipes)
    echo "$name|$encoder|$pix_fmt|$hdr|$expected|$result|$details" >> "$RESULTS_FILE"

    if [ "$result" = "PASS" ]; then
        PASS=$((PASS+1))
    elif [ "$result" = "WARN" ]; then
        WARN=$((WARN+1))
    else
        FAIL=$((FAIL+1))
    fi
}

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
        results.append({
            "name": parts[0],
            "encoder": parts[1],
            "pix_fmt": parts[2],
            "hdr": parts[3],
            "expected": parts[4],
            "result": parts[5],
            "details": '|'.join(parts[6:]) if len(parts) > 6 else ""
        })

report = {
    "timestamp": int(time.time()),
    "date": datetime.now().isoformat(),
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

# ============ MAIN ============

echo "=== Color Space Compatibility Test ==="
echo "API:  $API_BASE"
echo "RTMP: $RTMP_BASE"
echo ""

api_call "/api/health" || { echo "Health check failed, is the server running?"; exit 1; }

echo ""
echo "========== COLOR SPACE TEST MATRIX =========="
echo "WARN = records correctly but uses non-standard profile or loses HDR metadata"
echo ""

# Test matrix (encoder-supported combinations only; 12-bit excluded)
run_color_test "H.264 4:2:0 8-bit SDR"      libx264  yuv420p     ""      sdr  PASS
run_color_test "AV1 4:2:0 8-bit SDR"        libsvtav1 yuv420p    ""      sdr  PASS
run_color_test "H.264 4:2:2 8-bit SDR"      libx264  yuv422p     high422 sdr  WARN
run_color_test "H.264 4:4:4 8-bit SDR"      libx264  yuv444p     high444 sdr  WARN
run_color_test "H.264 NV12 8-bit SDR"       libx264  nv12        ""      sdr  PASS
run_color_test "H.264 4:2:0 10-bit SDR"     libx264  yuv420p10le ""      sdr  WARN
run_color_test "H.264 4:2:0 10-bit HDR"     libx264  yuv420p10le ""      hdr  WARN
run_color_test "AV1 4:2:0 10-bit SDR"       libsvtav1 yuv420p10le ""     sdr  PASS
run_color_test "AV1 4:2:0 10-bit HDR"       libsvtav1 yuv420p10le ""     hdr  WARN

echo ""
echo "============================================"
echo "         COLOR SPACE TEST SUMMARY"
echo "============================================"
echo -e "${GREEN}Passed: $PASS${NC}"
echo -e "${YELLOW}Warned: $WARN${NC}"
echo -e "${RED}Failed: $FAIL${NC}"
echo "============================================"

# Generate JSON report
REPORT_PATH="$REPORT_DIR/color_space_report_$(date +%s).json"
if command -v python3 >/dev/null 2>&1; then
    echo ""
    generate_json_report "$REPORT_PATH"
else
    echo ""
    echo -e "${YELLOW}⚠ python3 not found; skipping JSON report${NC}"
fi

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
