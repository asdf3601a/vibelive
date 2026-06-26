#!/usr/bin/env python3
"""DTS / CTS / PTS timing consistency check for MP4/fMP4 files.

Walks every moof → traf in the file and, for each track:
  * reads tfdt  → base_media_decode_time
  * reads tfhd  → default_sample_duration (if present)
  * reads trun  → per-sample size, duration, composition_time_offset
  * reconstructs DTS = base + cumulative durations
  * computes PTS = DTS + composition_time_offset

Flags checked:
  1. First sample's composition_time_offset == 0  (CTS normalization)
  2. DTS monotonically non-decreasing
  3. PTS monotonically non-decreasing

Prints one diagnostic line per traf plus a final ``TIMING_OK`` or
``TIMING_FAIL`` and exits 0/1 accordingly.  Invoked by test.sh's check_mp4().
"""
import struct
import sys


# ── Box walking helpers ──────────────────────────────────────────

def iter_boxes(data, start, end):
    """Yield (offset, size, type_bytes) for all top-level boxes in [start, end)."""
    o = start
    while o + 8 <= end:
        sz = struct.unpack('>I', data[o:o + 4])[0]
        tp = data[o + 4:o + 8]
        if sz < 8 or o + sz > end:
            break
        yield (o, sz, tp)
        o += sz


def find_child(data, target, start, end):
    """Find first child box of *target* type in [start, end)."""
    for off, sz, tp in iter_boxes(data, start, end):
        if tp == target:
            return off, sz
    return None, None


def walk_moofs(data):
    """Yield (moof_offset, moof_size) for every moof box at the top level."""
    for off, sz, tp in iter_boxes(data, 0, len(data)):
        if tp == b'moof':
            yield off, sz


# ── tfhd / tfdt / trun parsers ───────────────────────────────────

def parse_tfhd(data, off, size):
    """Return (track_id, default_sample_duration or None)."""
    end = off + size
    pos = off + 8  # skip size + type
    if pos + 4 > end:
        return 0, None
    version = data[pos]
    pos += 1
    flags = (data[pos] << 16) | (data[pos + 1] << 8) | data[pos + 2]
    pos += 3
    if pos + 4 > end:
        return 0, None
    track_id = struct.unpack('>I', data[pos:pos + 4])[0]
    pos += 4
    default_duration = None
    if flags & 0x000008 and pos + 4 <= end:
        default_duration = struct.unpack('>I', data[pos:pos + 4])[0]
        pos += 4
    return track_id, default_duration


def parse_tfdt(data, off, size):
    """Return base_media_decode_time (u64)."""
    end = off + size
    pos = off + 8
    version = data[pos] if pos < end else 0
    pos = off + 12  # skip version(1) + flags(3)
    if version == 1 and pos + 8 <= end:
        return struct.unpack('>Q', data[pos:pos + 8])[0]
    elif pos + 4 <= end:
        return struct.unpack('>I', data[pos:pos + 4])[0]
    return 0


def parse_trun(data, off, size):
    """Return (version, flags, sample_count, entries) where entries is a list of
    dicts with keys: size, duration (if present), cto (if present).
    """
    end = off + size
    pos = off + 8
    version = data[pos] if pos < end else 0
    pos += 1
    flags = (data[pos] << 16) | (data[pos + 1] << 8) | data[pos + 2]
    pos += 3

    if pos + 4 > end:
        return version, flags, 0, []
    sample_count = struct.unpack('>I', data[pos:pos + 4])[0]
    pos += 4

    if flags & 0x000001:  # data-offset-present
        pos += 4
    if flags & 0x000004:  # first-sample-flags-present
        pos += 4

    has_duration = bool(flags & 0x000100)
    has_size = bool(flags & 0x000200)
    has_flags = bool(flags & 0x000400)
    has_cto = bool(flags & 0x000800)

    entries = []
    for _ in range(sample_count):
        e = {}
        if has_duration:
            if pos + 4 > end:
                break
            e['duration'] = struct.unpack('>I', data[pos:pos + 4])[0]
            pos += 4
        if has_size:
            if pos + 4 > end:
                break
            e['size'] = struct.unpack('>I', data[pos:pos + 4])[0]
            pos += 4
        if has_flags:
            pos += 4
        if has_cto:
            if pos + 4 > end:
                break
            e['cto'] = struct.unpack('>i', data[pos:pos + 4])[0]  # signed
            pos += 4
        entries.append(e)

    return version, flags, sample_count, entries


# ── Main ─────────────────────────────────────────────────────────

def main(path):
    with open(path, 'rb') as f:
        data = f.read()

    ok = True
    traf_count = 0

    for moof_off, moof_sz in walk_moofs(data):
        moof_end = moof_off + moof_sz

        # Walk traf boxes inside moof
        for traf_off, traf_sz, tp in iter_boxes(data, moof_off + 8, moof_end):
            if tp != b'traf':
                continue
            traf_count += 1
            traf_end = traf_off + traf_sz

            # Parse tfhd
            tfhd_off, tfhd_sz = find_child(data, b'tfhd', traf_off + 8, traf_end)
            track_id, default_dur = (0, None)
            if tfhd_off is not None:
                track_id, default_dur = parse_tfhd(data, tfhd_off, tfhd_sz)

            # Parse tfdt
            tfdt_off, tfdt_sz = find_child(data, b'tfdt', traf_off + 8, traf_end)
            base_dts = 0
            if tfdt_off is not None:
                base_dts = parse_tfdt(data, tfdt_off, tfdt_sz)

            # Parse trun
            trun_off, trun_sz = find_child(data, b'trun', traf_off + 8, traf_end)
            if trun_off is None:
                continue
            version, flags, sample_count, entries = parse_trun(data, trun_off, trun_sz)

            is_video = bool(flags & 0x000004)
            track_label = f"track {track_id} ({'video' if is_video else 'audio'})"

            if sample_count == 0 or not entries:
                print(f'  {track_label}: empty trun')
                continue

            # ── Reconstruct DTS and PTS ────────────────────────
            dts = base_dts
            prev_dts = None
            prev_pts = None
            first_cto_ok = True
            dts_mono_ok = True
            pts_mono_ok = True
            cto_details = []

            for i, e in enumerate(entries):
                cto = e.get('cto', 0)
                dur = e.get('duration', default_dur if default_dur is not None else 0)
                pts = dts + cto

                if i == 0 and cto != 0:
                    first_cto_ok = False

                if prev_dts is not None and dts < prev_dts:
                    dts_mono_ok = False
                if prev_pts is not None and pts < prev_pts:
                    pts_mono_ok = False

                if is_video:
                    cto_details.append(cto)

                prev_dts = dts
                prev_pts = pts
                dts += dur

            # ── Report ─────────────────────────────────────────
            n = len(entries)
            cto_str = ''
            if is_video:
                unique_ctos = sorted(set(cto_details))
                if len(unique_ctos) <= 4:
                    cto_str = f', CTOs={unique_ctos}'
                else:
                    cto_str = f', CTO range=[{min(unique_ctos)},{max(unique_ctos)}]'

            dur_str = f'dur={default_dur}' if default_dur is not None else 'dur=?'
            print(f'  {track_label}: {n} samples, base_dts={base_dts}, {dur_str}{cto_str}')

            if not first_cto_ok:
                first_val = entries[0].get('cto', 0)
                print(f'    ✗ first CTO != 0 (got {first_val})')
                ok = False
            if not dts_mono_ok:
                print(f'    ✗ DTS not monotonically increasing')
                ok = False
            if not pts_mono_ok:
                print(f'    ✗ PTS not monotonically increasing')
                ok = False

    if traf_count == 0:
        # Fragmented MP4 with no moof (e.g. pure init.mp4) — nothing to check
        print('  no moof/traf found (init segment or empty file)')

    print('TIMING_OK' if ok else 'TIMING_FAIL')
    return 0 if ok else 1


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print('usage: timing_check.py <mp4>', file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
