#!/usr/bin/env python3
"""Edit list (edts/elst) consistency check for MP4/fMP4 files.

Validates the multi-encoder edit-list policy:
  * video trak (avc1/hvc1/av01): NO edts  (timing via CTS normalization)
  * AAC audio trak (mp4a):       edts with elst media_time == 1024  (pre-roll skip)
  * Opus audio trak (Opus):      NO edts  (pre-roll via sgpd/sbgp + dOps pre_skip)
  * FLAC audio trak (fLaC):      NO edts  (no encoder delay)

For fragmented movies (moov with mvex, i.e. our muxer's init segment / fMP4)
the AAC elst is REQUIRED and must carry media_time == 1024. For progressive
MP4s (no mvex, e.g. a recording remuxed by ffmpeg) the elst is advisory: the
mov demuxer applies the edit list during remux, dropping the priming, so the
output may legitimately lack an elst — only its absence is noted, not failed.

Walks moov -> trak, reads each trak's handler type (hdlr) and sample entry
(stsd) to classify it, then enforces the rules above on any edts/elst present.

Prints one diagnostic line per trak plus a final ``EDTS_OK`` or ``EDTS_FAIL``
and exits 0/1 accordingly.  Invoked by test.sh's check_mp4().
"""
import struct
import sys


# AAC-LC encoder pre-roll skipped via the audio edit list (one AAC frame).
# Must match server/src/hls/fmp4/mod.rs AAC_PRIMING_SAMPLES.
AAC_PRIMING_SAMPLES = 1024


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
    """Find first child box of *target* type in [start, end). Returns (off, sz)."""
    for off, sz, tp in iter_boxes(data, start, end):
        if tp == target:
            return off, sz
    return None, None


def walk_traks(data):
    """Yield (trak_off, trak_sz) for every trak box under the first moov."""
    moof_off = None
    for off, sz, tp in iter_boxes(data, 0, len(data)):
        if tp == b'moov':
            moof_off, moof_sz = off, sz
            break
    if moof_off is None:
        return
    for off, sz, tp in iter_boxes(data, moof_off + 8, moof_off + moof_sz):
        if tp == b'trak':
            yield off, sz


def is_fragmented_movie(data):
    """True if the moov carries an mvex box (fragmented movie / init segment).

    Distinguishes our muxer's init segment (mvex present → the AAC pre-roll
    elst MUST be present and correct) from a progressive MP4 produced by ffmpeg
    remux (no mvex → the demuxer already applied the edit list during remux, so
    an elst is advisory rather than required).
    """
    for off, sz, tp in iter_boxes(data, 0, len(data)):
        if tp == b'moov':
            mvex_off, _ = find_child(data, b'mvex', off + 8, off + sz)
            return mvex_off is not None
    return False


def trak_handler_type(data, trak_off, trak_sz):
    """Return the 4-byte handler_type (b'vide'/'soun'/...) of a trak, or None."""
    mdia_off, mdia_sz = find_child(data, b'mdia', trak_off + 8, trak_off + trak_sz)
    if mdia_off is None:
        return None
    hdlr_off, hdlr_sz = find_child(data, b'hdlr', mdia_off + 8, mdia_off + mdia_sz)
    if hdlr_off is None:
        return None
    # hdlr fullbox: size(4)+type(4)+version(1)+flags(3)+pre_defined(4)+handler_type(4)
    pos = hdlr_off + 8 + 4 + 4
    if pos + 4 > hdlr_off + hdlr_sz:
        return None
    return data[pos:pos + 4]


def trak_codec_fourcc(data, trak_off, trak_sz):
    """Return the sample-entry FourCC (b'mp4a'/b'Opus'/b'fLaC'/b'avc1'/...) or None."""
    mdia_off, mdia_sz = find_child(data, b'mdia', trak_off + 8, trak_off + trak_sz)
    if mdia_off is None:
        return None
    minf_off, minf_sz = find_child(data, b'minf', mdia_off + 8, mdia_off + mdia_sz)
    if minf_off is None:
        return None
    stbl_off, stbl_sz = find_child(data, b'stbl', minf_off + 8, minf_off + minf_sz)
    if stbl_off is None:
        return None
    stsd_off, stsd_sz = find_child(data, b'stsd', stbl_off + 8, stbl_off + stbl_sz)
    if stsd_off is None:
        return None
    # stsd fullbox: size(4)+type(4)+version(1)+flags(3)+entry_count(4)+entries
    pos = stsd_off + 8 + 4 + 4
    if pos + 8 > stsd_off + stsd_sz:
        return None
    # First sample entry: size(4)+type(4)
    return data[pos + 4:pos + 8]


def parse_elst(data, edts_off, edts_sz):
    """Return list of (segment_duration, media_time, media_rate_int) for an elst,
    or None if no elst is present inside the edts."""
    elst_off, elst_sz = find_child(data, b'elst', edts_off + 8, edts_off + edts_sz)
    if elst_off is None:
        return None
    end = elst_off + elst_sz
    pos = elst_off + 8  # skip size+type
    version = data[pos]
    pos += 4  # version(1)+flags(3)
    if pos + 4 > end:
        return None
    entry_count = struct.unpack('>I', data[pos:pos + 4])[0]
    pos += 4
    entries = []
    for _ in range(entry_count):
        if version == 1:
            if pos + 20 > end:
                break
            seg_dur = struct.unpack('>Q', data[pos:pos + 8])[0]
            media_time = struct.unpack('>q', data[pos + 8:pos + 16])[0]
            rate_int = struct.unpack('>h', data[pos + 16:pos + 18])[0]
            pos += 20
        else:
            if pos + 12 > end:
                break
            seg_dur = struct.unpack('>I', data[pos:pos + 4])[0]
            media_time = struct.unpack('>i', data[pos + 4:pos + 8])[0]
            rate_int = struct.unpack('>h', data[pos + 8:pos + 10])[0]
            pos += 12
        entries.append((seg_dur, media_time, rate_int))
    return entries


def main(path):
    with open(path, 'rb') as f:
        data = f.read()

    ok = True
    trak_count = 0
    fragmented = is_fragmented_movie(data)

    for trak_off, trak_sz in walk_traks(data):
        trak_count += 1
        trak_end = trak_off + trak_sz
        handler = trak_handler_type(data, trak_off, trak_sz)
        fourcc = trak_codec_fourcc(data, trak_off, trak_sz)

        edts_off, edts_sz = find_child(data, b'edts', trak_off + 8, trak_end)
        elst_entries = parse_elst(data, edts_off, edts_sz) if edts_off is not None else None

        label = f"trak {trak_count} ({handler.decode('latin1') if handler else '?'}/{fourcc.decode('latin1') if fourcc else '?'})"

        if handler == b'vide':
            # Video must never carry an edts (CTS normalization handles timing).
            if edts_off is not None:
                print(f'  {label}: ✗ unexpected edts on video trak')
                ok = False
            else:
                print(f'  {label}: no edts (ok)')
        elif handler == b'soun' and fourcc == b'mp4a':
            # AAC pre-roll skip via elst (media_time == 1024). Required for
            # fragmented movies (our init); advisory for progressive remuxes
            # whose demuxer already consumed the priming.
            if elst_entries is None or len(elst_entries) == 0:
                if fragmented:
                    print(f'  {label}: ✗ AAC trak missing edts/elst (no pre-roll skip)')
                    ok = False
                else:
                    print(f'  {label}: no edts (progressive; priming applied during remux)')
            else:
                seg_dur, media_time, rate_int = elst_entries[0]
                detail = f'seg_dur={seg_dur}, media_time={media_time}, rate={rate_int}'
                if media_time != AAC_PRIMING_SAMPLES:
                    if fragmented:
                        print(f'  {label}: ✗ AAC elst media_time={media_time} (expected {AAC_PRIMING_SAMPLES}) [{detail}]')
                        ok = False
                    else:
                        print(f'  {label}: edts/elst present, media_time={media_time} (progressive; advisory) [{detail}]')
                else:
                    print(f'  {label}: edts/elst ok ({detail})')
        elif handler == b'soun':
            # Opus / FLAC: pre-roll is handled elsewhere (or absent); no edts.
            if edts_off is not None:
                print(f'  {label}: ✗ unexpected edts on {fourcc} audio trak')
                ok = False
            else:
                print(f'  {label}: no edts (ok)')
        else:
            # Unknown handler: just report presence.
            if edts_off is not None:
                print(f'  {label}: edts present (unhandled handler)')
            else:
                print(f'  {label}: no edts')

    if trak_count == 0:
        print('  no trak found (init segment or empty file)')

    print('EDTS_OK' if ok else 'EDTS_FAIL')
    return 0 if ok else 1


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print('usage: edts_check.py <mp4>', file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
