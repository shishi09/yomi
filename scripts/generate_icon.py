#!/usr/bin/env python3
"""Generate the macOS AppIcon.icns with a crisp 16x14 pixel-art cat on a squircle."""

from __future__ import annotations

import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

CAT_GRID = [
    "..kk........kk..",
    ".kpk.kkkkkk.kok.",
    ".kwkkwoooowkkok.",
    ".kwwwoooooowwwk.",
    ".kwwgewwwwgewwk.",
    ".kwweewwwweewwk.",
    ".kwppwwppwwppwk.",
    ".kwwwwwwwwwwwwk.",
    "..kkkkkkkkkkkk..",
    "..ksswwwwwwssk..",
    "..kswwwwwwwwskk.",
    "..ksswwwwwwskok.",
    "..kwwksssskwwk..",
    "...kk......kk...",
]

PALETTE = {
    "k": (31, 35, 48, 255),     # #1f2330 outline
    "w": (251, 248, 241, 255),  # #fbf8f1 cream fur
    "s": (231, 223, 210, 255),  # #e7dfd2 fur shadow
    "o": (242, 155, 75, 255),   # #f29b4b ginger patch
    "p": (244, 143, 177, 255),  # #f48fb1 pink blush/ear
    "e": (27, 30, 40, 255),     # #1b1e28 eyes
    "g": (255, 255, 255, 255),  # white sparkle
}


def write_png(path: str, width: int, height: int, rgba: bytearray) -> None:
    """Write a bare RGBA PNG using stdlib zlib + struct."""
    stride = width * 4
    raw = bytearray((stride + 1) * height)
    for y in range(height):
        src_off = y * stride
        dst_off = y * (stride + 1)
        raw[dst_off] = 0
        raw[dst_off + 1 : dst_off + 1 + stride] = rgba[src_off : src_off + stride]

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw), level=6)
    with open(path, "wb") as fh:
        fh.write(b"\x89PNG\r\n\x1a\n")
        fh.write(chunk(b"IHDR", ihdr))
        fh.write(chunk(b"IDAT", idat))
        fh.write(chunk(b"IEND", b""))


def blend_pixel(buf: bytearray, idx: int, sr: int, sg: int, sb: int, sa: int) -> None:
    if sa <= 0:
        return
    if sa >= 255:
        buf[idx] = sr
        buf[idx + 1] = sg
        buf[idx + 2] = sb
        buf[idx + 3] = 255
        return
    inv = 255 - sa
    dr, dg, db, da = buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]
    out_a = sa + (da * inv) // 255
    if out_a == 0:
        return
    buf[idx] = (sr * sa + dr * da * inv // 255) // out_a
    buf[idx + 1] = (sg * sa + dg * da * inv // 255) // out_a
    buf[idx + 2] = (sb * sa + db * da * inv // 255) // out_a
    buf[idx + 3] = out_a


def render_icon_png(png_path: str, size: int = 1024) -> None:
    """Render a 1024x1024 macOS squircle background + crisp integer-scaled pixel cat."""
    for row in CAT_GRID:
        assert len(row) == 16, f"Expected 16 cols, got {len(row)} in {row!r}"

    buf = bytearray(size * size * 4)

    # macOS-inspired squircle inside standard 1024 canvas padding
    margin = size * 0.098  # ~100px Apple HIG icon margin
    half = size / 2.0
    radius = half - margin
    p_exp = 5.0

    # Background gradient colors: Yomi Washi Crème (#FAF5EA) -> Jaune beurre (#F2CA74)
    top_rgb = (250, 245, 234)
    bot_rgb = (242, 202, 116)

    for y in range(size):
        dy = abs(y + 0.5 - half) / radius
        t_y = max(0.0, min(1.0, (y - margin) / (2.0 * radius)))
        br = int(top_rgb[0] + (bot_rgb[0] - top_rgb[0]) * t_y)
        bg = int(top_rgb[1] + (bot_rgb[1] - top_rgb[1]) * t_y)
        bb = int(top_rgb[2] + (bot_rgb[2] - top_rgb[2]) * t_y)

        # Subtle warm paper top sheen
        sheen = max(0.0, 1.0 - t_y * 1.9) * 8.0
        dy_p = dy ** p_exp

        row_off = y * size * 4
        for x in range(size):
            dx = abs(x + 0.5 - half) / radius
            d = (dx ** p_exp + dy_p) ** (1.0 / p_exp)
            if d > 1.012:
                continue

            # Anti-aliased edge
            alpha = 255
            if d > 0.996:
                alpha = int(max(0.0, min(1.0, (1.012 - d) / 0.016)) * 255)

            # Subtle warm sand border near rim for crisp separation on light/dark Docks
            rim = max(0.0, (d - 0.95) / 0.05) * 18.0 if d > 0.95 else 0.0
            r = max(0, min(255, int(br + sheen - rim * 0.7)))
            g = max(0, min(255, int(bg + sheen - rim * 0.9)))
            b = max(0, min(255, int(bb + sheen * 0.5 - rim * 1.1)))

            idx = row_off + x * 4
            buf[idx] = r
            buf[idx + 1] = g
            buf[idx + 2] = b
            buf[idx + 3] = alpha

    # Subtle elliptical shadow under the cat
    sh_cx = half
    sh_cy = size * 0.775
    sh_rx = size * 0.25
    sh_ry = size * 0.042
    for y in range(int(sh_cy - sh_ry * 2), int(sh_cy + sh_ry * 2) + 1):
        if not (0 <= y < size):
            continue
        for x in range(int(sh_cx - sh_rx * 2), int(sh_cx + sh_rx * 2) + 1):
            if not (0 <= x < size):
                continue
            dist2 = ((x - sh_cx) / sh_rx) ** 2 + ((y - sh_cy) / sh_ry) ** 2
            if dist2 < 1.0:
                sa = int((1.0 - dist2) * 58)
                blend_pixel(buf, (y * size + x) * 4, 48, 36, 22, sa)

    # Draw integer-scaled crisp 16x14 pixel cat
    px_scale = 38  # 16*38 = 608px wide, 14*38 = 532px tall
    ox = (size - 16 * px_scale) // 2
    oy = (size - 14 * px_scale) // 2 - 8

    for gy, row in enumerate(CAT_GRID):
        for gx, ch in enumerate(row):
            col = PALETTE.get(ch)
            if not col:
                continue
            x0 = ox + gx * px_scale
            y0 = oy + gy * px_scale
            for py in range(y0, y0 + px_scale):
                row_off = py * size * 4
                for px in range(x0, x0 + px_scale):
                    idx = row_off + px * 4
                    buf[idx] = col[0]
                    buf[idx + 1] = col[1]
                    buf[idx + 2] = col[2]
                    buf[idx + 3] = col[3]

    write_png(png_path, size, size, buf)


def build_icns(out_icns: str) -> None:
    os.makedirs(os.path.dirname(os.path.abspath(out_icns)), exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="smt_icon_") as tmpdir:
        master_png = os.path.join(tmpdir, "icon_1024.png")
        iconset_dir = os.path.join(tmpdir, "AppIcon.iconset")
        os.makedirs(iconset_dir, exist_ok=True)

        render_icon_png(master_png, 1024)

        specs = [
            ("icon_16x16.png", 16),
            ("icon_16x16@2x.png", 32),
            ("icon_32x32.png", 32),
            ("icon_32x32@2x.png", 64),
            ("icon_128x128.png", 128),
            ("icon_128x128@2x.png", 256),
            ("icon_256x256.png", 256),
            ("icon_256x256@2x.png", 512),
            ("icon_512x512.png", 512),
            ("icon_512x512@2x.png", 1024),
        ]
        for fname, px in specs:
            dst = os.path.join(iconset_dir, fname)
            if px == 1024:
                shutil.copyfile(master_png, dst)
            else:
                subprocess.run(
                    ["sips", "-z", str(px), str(px), master_png, "--out", dst],
                    check=True,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )

        subprocess.run(["iconutil", "-c", "icns", iconset_dir, "-o", out_icns], check=True)


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: generate_icon.py <output.icns>", file=sys.stderr)
        return 2
    build_icns(sys.argv[1])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
