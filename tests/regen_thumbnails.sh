#!/bin/bash
set -euo pipefail

# Thumbnail regeneration script for vibe-livestream.
#
# Two-phase design (extract PNG refs → encode from refs).  Phase 1
# uses a single ffmpeg call with filter_complex to decode the source
# once and produce one PNG ref per width — zero redundant mp4 reads.
# Phase 2 encodes JXL/AVIF from the tiny cached PNGs in parallel.
#
# IO profile (1.5 GB source, warm page cache):
#   Per file: 1 mp4 read + JXL/AVIF encode from cached PNG refs
#   Total:    ~12 min, CPU-bound (JXL encoding dominates)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

MEDIA_DIR="./data"
RECORDINGS_BASE_URL="/recordings"
MAX_PARALLEL=1

cd "$PROJECT_DIR"
if [ -f .env ]; then
    set -a
    # shellcheck source=/dev/null
    source .env
    set +a
fi

while [[ $# -gt 0 ]]; do
    case $1 in
        --data|-d)
            MEDIA_DIR="$2"
            shift 2
            ;;
        --parallel|-p)
            MAX_PARALLEL="$2"
            [[ "$MAX_PARALLEL" =~ ^[0-9]+$ ]] || { echo "Invalid --parallel value: $MAX_PARALLEL"; exit 1; }
            [ "$MAX_PARALLEL" -ge 1 ] || MAX_PARALLEL=1
            shift 2
            ;;
        --help|-h)
            cat <<'EOF'
Usage: regen_thumbnails.sh [OPTIONS]

Regenerate thumbnails for all .mp4 recordings and rebuild index.json.
Reads .env for MEDIA_DIR, THUMBNAIL_SIZES, RECORDINGS_BASE_URL defaults.

Options:
  --data, -d <path>       Path to media directory (default: ./data)
  --parallel, -p <n>      Files to process concurrently (default: 1)
  --help, -h              Show this help

Formats generated: JPEG XL (.jxl), AVIF (.avif), PNG (.png)
Each format is generated at each size from THUMBNAIL_SIZES.

Unavailable codecs are detected at startup and skipped silently.
EOF
            exit 0
            ;;
        *)
            shift
            ;;
    esac
done

REC_DIR="$MEDIA_DIR/recordings"
THUMB_DIR="$MEDIA_DIR/thumbnails/recordings"
SIZES=$(echo "${THUMBNAIL_SIZES:-320,480}" | tr ',' ' ')
FFMPEG_TIMEOUT="${FFMPEG_TIMEOUT:-30}"
MAX_ATTEMPTS="${RECORDING_THUMBNAIL_MAX_ATTEMPTS:-3}"
RETRY_DELAY="${RECORDING_THUMBNAIL_RETRY_DELAY_SECS:-5}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# ── Codec probing ──────────────────────────────────────────────────
# Detect which ffmpeg codecs are available so we don't waste 3 retries
# per file on formats that can never succeed.

probe_codec() (
    set +o pipefail  # grep -q causes SIGPIPE on ffmpeg; pipefail would report 141
    ffmpeg -hide_banner -codecs 2>/dev/null | grep -qE "\b${1}\b"
)

JXL_AVAILABLE=0
AVIF_AVAILABLE=0
PNG_AVAILABLE=0

probe_codec "libjxl"    && JXL_AVAILABLE=1
probe_codec "libaom-av1" && AVIF_AVAILABLE=1
PNG_AVAILABLE=1  # built-in

# ── Faststart check / remux ────────────────────────────────────────
# ffmpeg fast-seek (-ss before -i) requires the moov atom at the
# beginning of the file.  Without it the moov sits after mdat (at the
# end), forcing a full-file seek on every thumbnail extraction.
# Remuxing with -movflags +faststart is a one-time, no-recode pass.

check_faststart() {
    python3 - "$1" <<'PYEOF'
import sys, struct
with open(sys.argv[1], 'rb') as f:
    data = f.read(8)
    if len(data) < 8:
        print("NO")
        sys.exit(0)
    size, tag = struct.unpack('>I4s', data)
    if tag != b'ftyp':
        print("NO")
        sys.exit(0)
    f.seek(size - 8, 1)
    data = f.read(8)
    if len(data) < 8:
        print("NO")
        sys.exit(0)
    _, tag = struct.unpack('>I4s', data)
    print("YES" if tag == b'moov' else "NO")
PYEOF
}

ensure_faststart() {
    local video="$1"
    local status
    status=$(check_faststart "$video")
    if [ "$status" = "YES" ]; then
        return 0
    fi

    echo "  remuxing for faststart..."
    local tmp="${video}.tmp"
    if timeout --foreground --kill-after=10 "$FFMPEG_TIMEOUT" ffmpeg -y -hide_banner -loglevel error \
        -i "$video" -c copy -movflags +faststart -f mp4 "$tmp" 2>/dev/null \
        && [ -f "$tmp" ] && [ -s "$tmp" ]; then
        mv "$tmp" "$video"
        echo -e "  ${GREEN}faststart remux OK${NC}"
        return 0
    else
        echo -e "  ${YELLOW}faststart remux failed — proceeding anyway${NC}"
        rm -f "$tmp" 2>/dev/null || true
        return 1
    fi
}

# ── Thumbnail generation for a single .mp4 ────────────────────────
# Phase 1: ensure a PNG reference exists for every width.  If a valid
#   PNG already sits in the output directory we use it directly (zero
#   mp4 reads).  Otherwise a single ffmpeg call extracts all widths
#   from the source in one pass.
# Phase 2: encode JXL / AVIF from the PNG refs in parallel.  The PNG
#   output is a no-op copy when the ref was just extracted; existing
#   PNGs are left untouched.
generate_thumbnails() {
    local video="$1"
    local output_dir="$2"
    local name
    name=$(basename "$video")
    local ok=1
    local tmpdir
    tmpdir=$(mktemp -d "${output_dir}/.regen_XXXXXX")

    # Clean stale 0-byte thumbnails
    for width in $SIZES; do
        for fmt in jxl avif png; do
            local stale="${output_dir}/${name}_w${width}.${fmt}"
            if [ -f "$stale" ] && [ ! -s "$stale" ]; then
                rm -f "$stale" 2>/dev/null || true
            fi
        done
    done

    local width_list=($SIZES)
    local nw=${#width_list[@]}
    local need_extract=0
    local w

    # Wire up PNG refs.  For widths whose final PNG already exists we
    # symlink / copy it into tmpdir as the ref; widths that are missing
    # trigger a full extraction pass.
    for ((i=0; i<nw; i++)); do
        w="${width_list[$i]}"
        local candidate="${output_dir}/${name}_w${w}.png"
        if [ -f "$candidate" ] && [ -s "$candidate" ]; then
            ln -sf "$candidate" "${tmpdir}/ref_w${w}.png" 2>/dev/null \
                || cp "$candidate" "${tmpdir}/ref_w${w}.png" 2>/dev/null \
                || true
        else
            need_extract=1
        fi
    done

    if [ "$need_extract" = 1 ]; then
        # Phase 1 — extract PNG refs from source (single ffmpeg call)
        local filter_parts=()
        local map_args=()

        local split_outs=""
        for ((i=0; i<nw; i++)); do
            split_outs+="[v${i}]"
        done
        filter_parts+=("[0:v]split=${nw}${split_outs}")

        for ((i=0; i<nw; i++)); do
            w="${width_list[$i]}"
            filter_parts+=("[v${i}]scale=${w}:-1[s${i}]")
            map_args+=(-map "[s${i}]" -f apng -frames:v 1 "${tmpdir}/ref_w${w}.png")
        done

        local filter_graph
        filter_graph=$(printf '%s;' "${filter_parts[@]}")
        filter_graph="${filter_graph%;}"

        if timeout --foreground --kill-after=10 "$FFMPEG_TIMEOUT" ffmpeg -y -hide_banner -loglevel error \
            -ss 00:00:00.5 -i "$video" \
            -filter_complex "$filter_graph" \
            "${map_args[@]}" \
            2>/dev/null; then
            for ((i=0; i<nw; i++)); do
                local ref="${tmpdir}/ref_w${width_list[$i]}.png"
                if [ -f "$ref" ] && [ -s "$ref" ]; then
                    echo -e "  ${GREEN}ref${NC} w=${width_list[$i]} (from source)"
                else
                    echo -e "  ${YELLOW}ref${NC} w=${width_list[$i]} failed"
                    ok=0
                fi
            done
        else
            for ((i=0; i<nw; i++)); do
                echo -e "  ${YELLOW}ref${NC} w=${width_list[$i]} failed (cannot decode source)"
            done
            ok=0
        fi
    fi

    # Phase 2 — JXL / AVIF / PNG from refs (parallel)
    local pids=()
    local markers=()
    local labels=()

    for width in $SIZES; do
        local ref="${tmpdir}/ref_w${width}.png"
        local final_png="${output_dir}/${name}_w${width}.png"

        if [ ! -f "$ref" ]; then
            continue
        fi

        # PNG: copy only if the ref is NOT already the final file
        local png_marker="${output_dir}/.tmp_${name}_w${width}_png.ok"
        if [ "$ref" -ef "$final_png" ]; then
            # Same inode — file already in place, nothing to copy.
            # Still use a background subshell so pids/markers/labels stay aligned.
            ( touch "$png_marker" ) &
            pids+=("$!")
        else
            (
                cp "$ref" "$final_png" 2>/dev/null && touch "$png_marker"
            ) &
            pids+=("$!")
        fi
        markers+=("$png_marker")
        labels+=("png w=${width}")

        # JXL from PNG ref
        if [ "$JXL_AVAILABLE" = 1 ]; then
            local jxl_marker="${output_dir}/.tmp_${name}_w${width}_jxl.ok"
            local jxl_out="${output_dir}/${name}_w${width}.jxl"
            (
                if timeout --foreground --kill-after=10 "$FFMPEG_TIMEOUT" ffmpeg -y -hide_banner -loglevel error \
                    -i "$ref" -c:v libjxl -q:v 90 -f image2 \
                    "$jxl_out" 2>/dev/null \
                    && [ -f "$jxl_out" ] && [ -s "$jxl_out" ]; then
                    touch "$jxl_marker"
                else
                    rm -f "$jxl_out" 2>/dev/null || true
                fi
            ) &
            pids+=("$!")
            markers+=("$jxl_marker")
            labels+=("jxl w=${width}")
        fi

        # AVIF from PNG ref
        if [ "$AVIF_AVAILABLE" = 1 ]; then
            local avif_marker="${output_dir}/.tmp_${name}_w${width}_avif.ok"
            local avif_out="${output_dir}/${name}_w${width}.avif"
            (
                if timeout --foreground --kill-after=10 "$FFMPEG_TIMEOUT" ffmpeg -y -hide_banner -loglevel error \
                    -i "$ref" -c:v libaom-av1 -crf 30 -still-picture 1 -f avif \
                    "$avif_out" 2>/dev/null \
                    && [ -f "$avif_out" ] && [ -s "$avif_out" ]; then
                    touch "$avif_marker"
                else
                    rm -f "$avif_out" 2>/dev/null || true
                fi
            ) &
            pids+=("$!")
            markers+=("$avif_marker")
            labels+=("avif w=${width}")
        fi
    done

    # Wait for all phase-2 jobs, then report
    for i in "${!pids[@]}"; do
        wait "${pids[$i]}" 2>/dev/null || true
        if [ -f "${markers[$i]}" ]; then
            echo -e "  ${GREEN}${labels[$i]}${NC}"
            rm -f "${markers[$i]}"
        else
            echo -e "  ${YELLOW}${labels[$i]} failed${NC}"
            ok=0
        fi
    done

    rm -rf "$tmpdir"
    return "$([ "$ok" -eq 1 ] && echo 0 || echo 1)"
}

# ── index.json management ──────────────────────────────────────────
# State is kept in-memory via a temp JSON file so per-entry updates
# don't require a full directory scan every invocation.

init_index() {
    python3 - "$REC_DIR" "$THUMB_DIR" "$RECORDINGS_BASE_URL" "$(printf '%s' "$SIZES")" <<'PYEOF'
import os, json, sys, datetime, tempfile

rec_dir = sys.argv[1]
thumb_dir = sys.argv[2]
base_url = sys.argv[3]
sizes = sys.argv[4].split()

# Load existing index to preserve server-populated fields
existing = {}
idx_path = os.path.join(rec_dir, 'index.json')
if os.path.isfile(idx_path):
    try:
        with open(idx_path) as f:
            data = json.load(f)
        for e in data.get('recordings', []):
            existing[e['filename']] = e
    except (json.JSONDecodeError, KeyError):
        pass

entries = []
for fname in sorted(os.listdir(rec_dir)):
    if not fname.endswith('.mp4'):
        continue
    path = os.path.join(rec_dir, fname)

    prev = existing.get(fname, {})

    stream_key = prev.get('stream_key', '')
    if not stream_key:
        stem = fname[:-4]
        parts = stem.split('_')
        stream_key = '_'.join(parts[:-2]) if len(parts) >= 3 else stem

    created_at = prev.get('created_at', '')
    if not created_at:
        st = os.stat(path)
        created_at = datetime.datetime.fromtimestamp(st.st_mtime, tz=datetime.timezone.utc).isoformat()

    size_bytes = prev.get('size_bytes', os.path.getsize(path))
    duration_seconds = prev.get('duration_seconds', None)

    thumbs = {}
    for w in sizes:
        png = os.path.join(thumb_dir, f'{fname}_w{w}.png')
        if os.path.isfile(png) and os.path.getsize(png) > 0:
            thumbs[str(w)] = f'/thumbnails/recordings/{fname}_w{w}.png'

    entries.append({
        'filename': fname,
        'stream_key': stream_key,
        'created_at': created_at,
        'size_bytes': size_bytes,
        'duration_seconds': duration_seconds,
        'url': prev.get('url', f'{base_url}/{fname}'),
        'thumbnails': thumbs,
    })

entries.sort(key=lambda e: e['created_at'], reverse=True)

# Write state file (pass back to shell)
state_path = os.path.join(rec_dir, '.regen_state.json')
with open(state_path, 'w') as f:
    json.dump({'recordings': entries}, f)

# Also write index.json
tmp = os.path.join(rec_dir, 'index.json.tmp')
with open(tmp, 'w') as f:
    json.dump({'recordings': entries}, f, indent=2)
os.replace(tmp, idx_path)
print(f'  index.json: {len(entries)} recording(s)')
PYEOF
}

# Update a single entry after its thumbnails were (re)generated.
# Only touches that one entry; no directory scan.
update_index_entry() {
    local fname="$1"
    python3 - "$REC_DIR" "$THUMB_DIR" "$RECORDINGS_BASE_URL" "$(printf '%s' "$SIZES")" "$fname" <<'PYEOF'
import os, json, sys, datetime

rec_dir = sys.argv[1]
thumb_dir = sys.argv[2]
base_url = sys.argv[3]
sizes = sys.argv[4].split()
fname = sys.argv[5]

state_path = os.path.join(rec_dir, '.regen_state.json')
entries = []

if os.path.isfile(state_path):
    try:
        with open(state_path) as f:
            entries = json.load(f).get('recordings', [])
    except (json.JSONDecodeError, KeyError):
        pass

# Find or create entry for this filename
entry = next((e for e in entries if e['filename'] == fname), None)
if entry is None:
    # New file not in initial scan — create entry from disk
    path = os.path.join(rec_dir, fname)
    st = os.stat(path)
    stem = fname[:-4]
    parts = stem.split('_')
    stream_key = '_'.join(parts[:-2]) if len(parts) >= 3 else stem
    entry = {
        'filename': fname,
        'stream_key': stream_key,
        'created_at': datetime.datetime.fromtimestamp(st.st_mtime, tz=datetime.timezone.utc).isoformat(),
        'size_bytes': st.st_size,
        'duration_seconds': None,
        'url': f'{base_url}/{fname}',
        'thumbnails': {},
    }
    entries.append(entry)

# Update thumbnails for this entry only
thumbs = {}
for w in sizes:
    png = os.path.join(thumb_dir, f'{fname}_w{w}.png')
    if os.path.isfile(png) and os.path.getsize(png) > 0:
        thumbs[str(w)] = f'/thumbnails/recordings/{fname}_w{w}.png'
entry['thumbnails'] = thumbs

# Keep sorted
entries.sort(key=lambda e: e['created_at'], reverse=True)

# Write state
with open(state_path, 'w') as f:
    json.dump({'recordings': entries}, f)

# Write index atomically
tmp = os.path.join(rec_dir, 'index.json.tmp')
idx = os.path.join(rec_dir, 'index.json')
with open(tmp, 'w') as f:
    json.dump({'recordings': entries}, f, indent=2)
os.replace(tmp, idx)
PYEOF
}

# ── Cleanup trap ───────────────────────────────────────────────────
# Parallel-mode background jobs are tracked in a pid file so the trap
# can kill them on SIGINT / SIGTERM even when batch_pids is out of scope.
_PID_FILE=""

cleanup() {
    if [ -n "${_PID_FILE:-}" ] && [ -f "$_PID_FILE" ]; then
        while read -r pid; do
            kill "$pid" 2>/dev/null || true
        done < "$_PID_FILE"
        rm -f "$_PID_FILE" 2>/dev/null || true
    fi
    rm -f "$REC_DIR/.regen_state.json" 2>/dev/null || true
    echo ""
    echo "Interrupted."
    exit 130
}
trap cleanup SIGINT SIGTERM

# ── Main ───────────────────────────────────────────────────────────
main() {
    echo "=== Thumbnail Regeneration ==="
    echo "Media dir:  $MEDIA_DIR"
    echo "Recordings: $REC_DIR"
    echo "Thumbnails: $THUMB_DIR"
    echo "Sizes:      $SIZES"
    echo "Parallel:   $MAX_PARALLEL file(s)"

    local fmt_list=""
    [ "$JXL_AVAILABLE"  = 1 ] && fmt_list="${fmt_list}jxl "
    [ "$AVIF_AVAILABLE" = 1 ] && fmt_list="${fmt_list}avif "
    [ "$PNG_AVAILABLE"  = 1 ] && fmt_list="${fmt_list}png "
    echo "Formats:    ${fmt_list:-none!}"

    local warn=""
    [ "$JXL_AVAILABLE"  = 0 ] && warn="${warn}  libjxl not found — skipping jxl\n"
    [ "$AVIF_AVAILABLE" = 0 ] && warn="${warn}  libaom-av1 not found — skipping avif\n"
    [ -n "$warn" ] && echo -e "${YELLOW}${warn}${NC}"

    if [ "$PNG_AVAILABLE" = 0 ]; then
        echo -e "${RED}ERROR: no available image codecs, nothing to generate${NC}"
        exit 1
    fi

    if ! command -v ffmpeg &>/dev/null; then
        echo -e "${RED}ERROR: ffmpeg not found${NC}"
        exit 1
    fi

    mkdir -p "$REC_DIR"
    mkdir -p "$THUMB_DIR"

    if [ ! -w "$THUMB_DIR" ]; then
        echo -e "${RED}ERROR: $THUMB_DIR is not writable${NC}"
        echo "Fix: chown -R \$USER $MEDIA_DIR/thumbnails  or  sudo $0 --data $MEDIA_DIR"
        exit 1
    fi
    if [ ! -w "$REC_DIR" ]; then
        echo -e "${RED}ERROR: $REC_DIR is not writable${NC}"
        exit 1
    fi

    shopt -s nullglob
    local videos=("$REC_DIR"/*.mp4)
    shopt -u nullglob

    if [ ${#videos[@]} -eq 0 ]; then
        echo "No .mp4 files found in $REC_DIR"
        init_index
        echo "Done."
        exit 0
    fi

    local total=${#videos[@]}
    local count=0 ok=0 fail=0
    local start_time
    start_time=$(date +%s)

    # Build initial index once (full directory scan, preserves existing data)
    init_index >/dev/null

    if [ "$MAX_PARALLEL" -le 1 ]; then
        # ── Sequential mode (detailed output) ────────────────────────────
        for video in "${videos[@]}"; do
            count=$((count + 1))
            local name
            name=$(basename "$video")

            local elapsed
            elapsed=$(($(date +%s) - start_time))
            if [ "$count" -gt 1 ]; then
                printf "[%d/%d] %s  (%ds elapsed)\n" "$count" "$total" "$name" "$elapsed"
            else
                printf "[%d/%d] %s\n" "$count" "$total" "$name"
            fi

            ensure_faststart "$video"

            local ok_file=0
            for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
                if [ "$attempt" -gt 1 ]; then
                    echo "  (retry $attempt/$MAX_ATTEMPTS)"
                    sleep "$RETRY_DELAY"
                fi

                if generate_thumbnails "$video" "$THUMB_DIR"; then
                    ok_file=1
                    break
                fi
            done

            if [ "$ok_file" -eq 1 ]; then
                ok=$((ok + 1))
            else
                echo -e "  ${RED}FAILED after $MAX_ATTEMPTS attempts${NC}"
                fail=$((fail + 1))
            fi

            update_index_entry "$name" >/dev/null
        done
    else
        # ── Parallel mode (batched, clean progress lines) ───────────────
        local logdir
        logdir=$(mktemp -d "${THUMB_DIR}/.regen_log_XXXXXX")
        _PID_FILE=$(mktemp)

        for ((i=0; i<total; i+=MAX_PARALLEL)); do
            local batch_pids=()
            local batch_results=()
            local batch_end=$((i + MAX_PARALLEL))
            [ "$batch_end" -gt "$total" ] && batch_end="$total"

            # Launch batch
            for ((j=i; j<batch_end; j++)); do
                local video="${videos[$j]}"
                local fname
                fname=$(basename "$video")
                local log="${logdir}/${fname}.log"
                local result="${logdir}/${fname}.result"

                (
                    echo "$BASHPID" >> "${_PID_FILE:?}"
                    ensure_faststart "$video" >>"$log" 2>&1
                    local file_ok=0
                    for attempt in $(seq 1 "$MAX_ATTEMPTS"); do
                        if generate_thumbnails "$video" "$THUMB_DIR" >>"$log" 2>&1; then
                            file_ok=1
                            break
                        fi
                        [ "$attempt" -lt "$MAX_ATTEMPTS" ] && sleep "$RETRY_DELAY"
                    done
                    echo "$file_ok" > "$result"
                ) &
                batch_pids+=("$!")
                batch_results["$j"]="$result"
            done

            # Wait for batch
            for pid in "${batch_pids[@]}"; do
                wait "$pid" 2>/dev/null || true
            done
            > "$_PID_FILE"  # clear — all jobs in this batch are done

            # Report and update index
            for ((j=i; j<batch_end; j++)); do
                count=$((count + 1))
                fname=$(basename "${videos[$j]}")
                local file_ok
                file_ok=$(cat "${batch_results[$j]}" 2>/dev/null || echo 0)
                local elapsed
                elapsed=$(($(date +%s) - start_time))

                if [ "$file_ok" = 1 ]; then
                    ok=$((ok + 1))
                    printf "[%d/%d] %s  ${GREEN}OK${NC}  (%ds)\n" "$count" "$total" "$fname" "$elapsed"
                else
                    fail=$((fail + 1))
                    printf "[%d/%d] %s  ${RED}FAIL${NC}  (%ds)\n" "$count" "$total" "$fname" "$elapsed"
                fi

                update_index_entry "$fname" >/dev/null
            done
        done

        rm -rf "$logdir"
        rm -f "$_PID_FILE"
    fi

    local total_elapsed
    total_elapsed=$(($(date +%s) - start_time))

    echo ""
    echo "=== Summary ==="
    printf "Total: %d  ${GREEN}OK: %d${NC}  ${RED}Failed: %d${NC}  (%ds)\n" "$total" "$ok" "$fail" "$total_elapsed"
    echo ""

    init_index  # final verified write
    rm -f "$REC_DIR/.regen_state.json"
    echo "Done."
}

main "$@"
