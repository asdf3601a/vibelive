#!/usr/bin/env python3
"""stts (decoding-time-to-sample) consistency check for an MP4/fMP4 file.

Walks the box hierarchy, inspects every stts box, and flags:
  * more than 6 sample-group entries (suggests jittery durations), and
  * a non-trivial fraction of near-zero-duration frames.

Prints one diagnostic line per non-empty stts box plus a final ``OK`` or
``FAIL`` line and exits 0/1 accordingly. Invoked by test.sh's check_mp4().
"""
import struct
import sys


def iter_boxes(data, target, start, end):
    o = start
    while o + 8 <= end:
        sz = struct.unpack('>I', data[o:o + 4])[0]
        tp = data[o + 4:o + 8]
        if sz == 0 or o + sz > end:
            break
        if tp == target:
            yield (o, sz)
        if tp in (b'moov', b'trak', b'mdia', b'minf', b'stbl', b'moof'):
            yield from iter_boxes(data, target, o + 8, o + sz)
        if tp == b'stsd':
            yield from iter_boxes(data, target, o + 12, o + sz)
        o += sz


STANDARD_DURATIONS = (120, 240, 480, 512, 960, 1024, 1920, 2048,
                      2880, 3840, 4096, 4608, 4800)


def main(path):
    with open(path, 'rb') as f:
        data = f.read()

    ok = True
    found_nonempty = False
    for off, _sz in iter_boxes(data, b'stts', 0, len(data)):
        entry_count = struct.unpack('>I', data[off + 12:off + 16])[0]
        if entry_count == 0:
            continue
        found_nonempty = True
        pos = off + 16
        entries = []
        for _ in range(entry_count):
            cnt = struct.unpack('>I', data[pos:pos + 4])[0]
            dur = struct.unpack('>I', data[pos + 4:pos + 8])[0]
            entries.append((cnt, dur))
            pos += 8
        total = sum(c for c, _ in entries)
        durs_str = ', '.join(f'{c}x{d}' for c, d in entries)
        if len(entries) > 6:
            print(f'  stts: {len(entries)} groups (expected ≤6): {durs_str[:80]}...')
            ok = False
        else:
            dom_cnt, dom_dur = max(entries, key=lambda e: e[0])
            if dom_dur not in STANDARD_DURATIONS:
                print(f'  stts: {total} samples, {durs_str} - ⚠ nonstandard delta={dom_dur}')
            else:
                print(f'  stts: {total} samples, {durs_str}')
            zero_frames = sum(c for c, d in entries if d <= 1)
            if zero_frames > total * 0.05:
                print(f'  ✗ stts: {zero_frames}/{total} frames with duration≤1 (essentially zero)')
                ok = False
    if not found_nonempty:
        print('  stts: fragmented MP4 (durations in moof)')

    print('OK' if ok else 'FAIL')
    return 0 if ok else 1


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print('usage: stts_check.py <mp4>', file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
