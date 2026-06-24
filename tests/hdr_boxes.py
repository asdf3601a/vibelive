#!/usr/bin/env python3
"""Validate that an fMP4 init segment carries the HDR metadata boxes
(colr, clli, mdcv) required for correct PQ/HLG playback.

Walks the ISOBMFF box hierarchy, descending into the visual sample entry,
and reports each expected box as present/missing. Prints ``HDR_OK`` or
``HDR_FAIL`` and exits 0/1 accordingly. Invoked by test.sh's color suite.
"""
import struct
import sys

CONTAINERS = {b'moov', b'trak', b'mdia', b'minf', b'stbl'}
SAMPLE_ENTRIES = {b'avc1', b'hvc1', b'hev1', b'av01'}


def child_range(off, size, typ):
    """Range of the payload to recurse into for a given container box."""
    if typ == b'stsd':
        return off + 16, off + size  # skip FullBox header + entry_count
    if typ in SAMPLE_ENTRIES:
        return off + 86, off + size  # visual sample entry header
    if typ in CONTAINERS:
        return off + 8, off + size
    return None


def find_box(data, target, start=0, end=None):
    if end is None:
        end = len(data)
    off = start
    while off + 8 <= end:
        sz = struct.unpack('>I', data[off:off + 4])[0]
        typ = data[off + 4:off + 8]
        if sz < 8 or off + sz > end:
            break
        if typ == target:
            return (sz, data[off + 8:off + sz])
        cr = child_range(off, sz, typ)
        if cr:
            r = find_box(data, target, *cr)
            if r:
                return r
        off += sz
    return None


def main(path):
    with open(path, 'rb') as f:
        data = f.read()

    ok = True
    for box_name in (b'colr', b'clli', b'mdcv'):
        r = find_box(data, box_name)
        if r:
            sz, _pl = r
            print(f'  {box_name.decode()}: present ({sz}b)')
        else:
            print(f'  {box_name.decode()}: MISSING')
            ok = False

    print('HDR_OK' if ok else 'HDR_FAIL')
    return 0 if ok else 1


if __name__ == '__main__':
    if len(sys.argv) != 2:
        print('usage: hdr_boxes.py <init.mp4>', file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
