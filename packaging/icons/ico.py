#!/usr/bin/env python3
"""Packs square PNGs into a Windows .ico (PNG-compressed entries, Vista+).

Usage: ico.py <out.ico> <in.png>...
"""

import struct
import sys


def png_side(data):
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise SystemExit("not a PNG")
    width, height = struct.unpack(">II", data[16:24])
    if width != height or width > 256:
        raise SystemExit(f"ico entries must be square and <= 256 px, got {width}x{height}")
    return width


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    images = []
    for path in sys.argv[2:]:
        with open(path, "rb") as f:
            data = f.read()
        images.append((png_side(data), data))
    images.sort(key=lambda image: image[0])

    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, blobs = b"", b""
    for side, data in images:
        # 256 is stored as 0; 32 bpp; offset into the file.
        entries += struct.pack("<BBBBHHII", side % 256, side % 256, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    with open(sys.argv[1], "wb") as f:
        f.write(header + entries + blobs)


if __name__ == "__main__":
    main()
