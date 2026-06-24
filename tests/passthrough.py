#!/usr/bin/env python3
"""Audio passthrough verification: compare raw audio frames between FLV and MP4 recording."""
import struct, subprocess, os, sys, tempfile

flv_path = os.environ['PT_FLV']
rec_path = os.environ['PT_REC']
codec = os.environ.get('PT_FOURCC', 'AAC_LEGACY')

# Extract audio frames from FLV
with open(flv_path, 'rb') as f:
    d = f.read()
flv_frames = []
pos = 13
while pos + 15 <= len(d):
    t = d[pos]
    sz = struct.unpack('>I', b'\x00' + d[pos + 1:pos + 4])[0]
    if t == 8:
        sf = (d[pos + 11] >> 4) & 0xF
        if sf == 9 and codec in ('Opus', 'fLaC') and d[pos + 12:pos + 16] == codec.encode():
            flv_frames.append(d[pos + 16:pos + 11 + sz])
        elif sf == 10 and codec == 'AAC_LEGACY':
            pkt = d[pos + 12]
            if pkt == 1:
                flv_frames.append(d[pos + 13:pos + 11 + sz])
    pos += 11 + sz + 4

print(f'  FLV frames: {len(flv_frames)} ({sum(len(f) for f in flv_frames)} bytes)')

# Extract audio frames from recording
ext = {'AAC_LEGACY': 'adts', 'Opus': 'ogg', 'fLaC': 'flac'}.get(codec, 'flac')
tmp = tempfile.mktemp(suffix=f'.{ext}')
r = subprocess.run(
    ['ffmpeg', '-y', '-hide_banner', '-loglevel', 'error',
     '-i', rec_path, '-vn', '-acodec', 'copy', '-f', ext, tmp],
    capture_output=True, text=True)
if r.returncode != 0:
    print(f'✗ ffmpeg extract failed: {r.stderr[:200]}')
    sys.exit(1)

with open(tmp, 'rb') as f:
    raw = f.read()
os.unlink(tmp)

rec_frames = []
if ext == 'adts':
    # AAC: strip 7-byte ADTS headers
    pos = 0
    while pos + 7 <= len(raw):
        if raw[pos] != 0xFF or (raw[pos + 1] & 0xF0) != 0xF0:
            pos += 1
            continue
        fl = ((raw[pos + 3] & 3) << 11) | (raw[pos + 4] << 3) | ((raw[pos + 5] >> 5) & 7)
        if fl < 7 or pos + fl > len(raw):
            pos += 1
            continue
        rec_frames.append(raw[pos + 7:pos + fl])
        pos += fl
elif ext == 'ogg':
    # Opus: skip first 2 Ogg pages (OpusHead, OpusTags), one page per frame
    pos = 0
    pages_skipped = 0
    while pos + 27 <= len(raw):
        if raw[pos:pos + 4] != b'OggS':
            pos += 1
            continue
        nsegs = raw[pos + 26]
        seg_table = raw[pos + 27:pos + 27 + nsegs]
        data_start = pos + 27 + nsegs
        if pages_skipped >= 2:
            frame = raw[data_start:data_start + sum(seg_table)]
            rec_frames.append(frame)
        pages_skipped += 1
        pos = data_start + sum(seg_table)
elif ext == 'flac':
    # FLAC: split by frame sync code 0xFF 0xF8
    pos = 0
    while pos + 2 <= len(raw):
        if raw[pos] == 0xFF and (raw[pos + 1] & 0xFC) == 0xF8:
            end = pos + 2
            while end < len(raw):
                if end + 2 <= len(raw) and raw[end] == 0xFF and (raw[end + 1] & 0xFC) == 0xF8:
                    break
                end += 1
            rec_frames.append(raw[pos:end])
            pos = end
        else:
            pos += 1

print(f'  Rec frames: {len(rec_frames)} ({sum(len(f) for f in rec_frames)} bytes)')

if not rec_frames:
    print('✗ No frames extracted from recording')
    sys.exit(1)

# Find best alignment (account for metadata frames)
best_skip, best_match = 0, 0
for skip in range(min(8, len(flv_frames))):
    end = min(len(flv_frames) - skip, len(rec_frames))
    if end <= 0:
        continue
    m = sum(1 for i in range(end) if flv_frames[skip + i] == rec_frames[i])
    if m > best_match:
        best_match, best_skip = m, skip

end = min(len(flv_frames) - best_skip, len(rec_frames))
ratio = best_match / end * 100 if end > 0 else 0
print(f'  Byte match (align={best_skip}): {best_match}/{end} ({ratio:.1f}%)')

if best_match == end and end > 0:
    ref_total = sum(len(f) for f in flv_frames[best_skip:])
    rec_total = sum(len(f) for f in rec_frames)
    if abs(ref_total - rec_total) < 200:
        print('PASSTHROUGH_OK')
    else:
        print(f'PASSTHROUGH_FAIL (size mismatch: {ref_total} vs {rec_total})')
else:
    print('PASSTHROUGH_FAIL')
