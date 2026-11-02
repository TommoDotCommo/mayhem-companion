"""Draws assets/app.ico (pink rounded tile with a white "M"), standard library only."""
import struct, zlib, sys

PINK = (0xff, 0x4f, 0xa3)
M = [(0.22, 0.78), (0.22, 0.24), (0.35, 0.24), (0.50, 0.49), (0.65, 0.24), (0.78, 0.24), (0.78, 0.78),
     (0.66, 0.78), (0.66, 0.44), (0.535, 0.65), (0.465, 0.65), (0.34, 0.44), (0.34, 0.78)]

def inside(poly, x, y):
    c = False
    for (x1, y1), (x2, y2) in zip(poly, poly[1:] + poly[:1]):
        if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
            c = not c
    return c

def in_tile(x, y, r=0.22):
    cx, cy = min(max(x, r), 1 - r), min(max(y, r), 1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r

def render(size, ss=4):
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            tile = white = 0
            for sy in range(ss):
                for sx in range(ss):
                    x, y = (px + (sx + 0.5) / ss) / size, (py + (sy + 0.5) / ss) / size
                    if in_tile(x, y):
                        tile += 1
                        white += inside(M, x, y)
            n = ss * ss
            a = tile / n
            w = white / tile if tile else 0
            rgb = [round(c * (1 - w) + 255 * w) for c in PINK]
            row += bytes(rgb + [round(a * 255)])
        rows.append(bytes(row))
    raw = b"".join(b"\0" + r for r in rows)
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

sizes = [16, 24, 32, 48, 64, 256]
pngs = [render(s) for s in sizes]
out = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for s, p in zip(sizes, pngs):
    out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(p), offset)
    offset += len(p)
open(sys.argv[1], "wb").write(out + b"".join(pngs))
