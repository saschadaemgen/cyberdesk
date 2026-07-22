#!/usr/bin/env python3
"""Generate the CyberDesk application icon from the committed brand tokens.

The artwork is DERIVED, not invented (CD-44 Stage D): the geometry comes from
the CARVILON mark already in the repo - the open outer ring with its signature
gap (theme.toml [ring]: radius, stroke, gap_degrees) and the Energy Core's
hollow inner ring plus centre spark (src/start.html) - and every colour comes
from theme.toml [colors]. Re-run this after a token change:

    python scripts/make-icon.py

It writes:
  assets/cyberdesk.ico   multi-size icon for the .exe resource and the shell
  assets/cyberdesk.rgba  64x64 straight RGBA for the winit window icon

No third-party imaging library: the PNG payloads are written with zlib, which
is in the standard library, so the generator runs anywhere the repo does.
"""

import math
import os
import struct
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")
SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 4  # supersampling factor per axis


def tokens():
    """Read the colours and ring geometry straight from theme.toml."""
    vals = {}
    section = None
    with open(os.path.join(ROOT, "src", "theme.toml"), encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("[") and line.endswith("]"):
                section = line[1:-1]
                continue
            if "=" not in line:
                continue
            key, raw = (p.strip() for p in line.split("=", 1))
            # Values may be quoted (colours are "#RRGGBB", so a naive
            # comment strip would eat them); trailing comments only count
            # outside the quotes.
            if raw.startswith('"'):
                end = raw.find('"', 1)
                value = raw[1:end] if end > 0 else raw.strip('"')
            else:
                value = raw.split("#", 1)[0].strip()
            vals[f"{section}.{key}"] = value
    return vals


T = tokens()


def rgb(hex_str):
    h = hex_str.lstrip("#")
    return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))


BG = rgb(T["colors.background"])
BRAND = rgb(T["colors.brand"])
TEXT = rgb(T["colors.text"])

# Ring geometry as fractions of the icon edge, from the theme tokens. The
# outer ring is scaled up slightly versus the shell (0.40 of the viewport is
# right for a fullscreen mark, 0.36 reads better inside a square icon with
# its own margin), and the inner ring uses the Energy Core proportion.
R_OUTER = 0.355
W_OUTER = float(T["ring.stroke"]) * 3.1        # 0.010 -> ~0.031 of the edge
GAP_DEG = float(T["ring.gap_degrees"])
R_INNER = 0.150
W_INNER = float(T["ring.inner_stroke"]) * 2.6
R_SPARK = 0.052


def coverage(px, py, size, small):
    """Ink coverage at a supersampled point, 0..1, plus which colour it is."""
    cx = cy = size / 2.0
    x = (px - cx) / size
    y = (py - cy) / size
    d = math.hypot(x, y)
    ang = math.degrees(math.atan2(y, x)) % 360.0

    # Outer open ring: a gap centred at the top (the CARVILON signature).
    half = GAP_DEG / 2.0
    in_gap = ang > (270.0 - half) and ang < (270.0 + half)
    outer = abs(d - R_OUTER) <= W_OUTER / 2.0 and not in_gap

    # Inner hollow ring and the centre spark (the Energy Core, simplified).
    inner = abs(d - R_INNER) <= W_INNER / 2.0
    spark = d <= R_SPARK
    # At 16 and 24 px the thin inner ring turns to mush, so the core reads as
    # the spark alone: an honest simplification, not a different mark.
    if small:
        inner = False
    return outer or inner, spark


def render(size):
    """Straight-alpha RGBA bytes for one square icon size."""
    small = size <= 24
    out = bytearray()
    n = SS * SS
    for py in range(size):
        for px in range(size):
            ink = 0
            spark_hits = 0
            for sy in range(SS):
                for sx in range(SS):
                    fx = px + (sx + 0.5) / SS
                    fy = py + (sy + 0.5) / SS
                    is_ink, is_spark = coverage(fx, fy, size, small)
                    if is_ink or is_spark:
                        ink += 1
                    if is_spark:
                        spark_hits += 1
            if ink == 0:
                # Transparent margin: the shell composites the icon on the
                # taskbar's own background, so no fake plate behind the mark.
                out += bytes((0, 0, 0, 0))
                continue
            a = int(round(255 * ink / n))
            # The spark is the brightest point (text white), the rings brand.
            t = spark_hits / max(ink, 1)
            col = tuple(
                int(round(BRAND[i] + (TEXT[i] - BRAND[i]) * t)) for i in range(3)
            )
            out += bytes((col[0], col[1], col[2], a))
    return bytes(out)


def png(rgba, size):
    """Encode straight RGBA as a PNG (dependency-free)."""
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # filter: none
        raw += rgba[y * stride : (y + 1) * stride]

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(
            ">I", zlib.crc32(body) & 0xFFFFFFFF
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def main():
    os.makedirs(ASSETS, exist_ok=True)
    images = []
    rgba64 = None
    for size in SIZES:
        rgba = render(size)
        if size == 64:
            rgba64 = rgba
        images.append((size, png(rgba, size)))

    # ICO container: PNG payloads throughout (supported since Vista; the
    # target is Windows 11 only).
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack(
            "<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset
        )
        blobs += data
        offset += len(data)
    with open(os.path.join(ASSETS, "cyberdesk.ico"), "wb") as fh:
        fh.write(header + entries + blobs)

    # Raw 64x64 RGBA for winit's window icon (no PNG decoder needed at runtime).
    with open(os.path.join(ASSETS, "cyberdesk.rgba"), "wb") as fh:
        fh.write(rgba64)

    print(f"wrote assets/cyberdesk.ico ({len(SIZES)} sizes) and assets/cyberdesk.rgba")


if __name__ == "__main__":
    main()
