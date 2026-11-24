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


def _write_png_from_rgba(png_path: str, width: int, height: int, rgba: bytearray | bytes) -> None:
    write_png(png_path, width, height, rgba)


def _downsample_half(rgba: bytes | bytearray, size: int) -> bytearray:
    half = size // 2
    out = bytearray(half * half * 4)
    for y in range(half):
        y0 = y * 2
        row0 = y0 * size * 4
        row1 = (y0 + 1) * size * 4
        dst_row = y * half * 4
        for x in range(half):
            x0 = x * 8
            i0 = row0 + x0
            i1 = i0 + 4
            i2 = row1 + x0
            i3 = i2 + 4
            d = dst_row + x * 4
            out[d] = (rgba[i0] + rgba[i1] + rgba[i2] + rgba[i3]) >> 2
            out[d + 1] = (rgba[i0 + 1] + rgba[i1 + 1] + rgba[i2 + 1] + rgba[i3 + 1]) >> 2
            out[d + 2] = (rgba[i0 + 2] + rgba[i1 + 2] + rgba[i2 + 2] + rgba[i3 + 2]) >> 2
            out[d + 3] = (rgba[i0 + 3] + rgba[i1 + 3] + rgba[i2 + 3] + rgba[i3 + 3]) >> 2
    return out


def render_icon_rgba(size: int = 1024) -> bytearray:
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

    return buf


def render_icon_png(png_path: str, size: int = 1024) -> None:
    buf = render_icon_rgba(size)
    write_png(png_path, size, size, buf)


def write_tray_png(png_path: str) -> None:
    """Render a 36x36 crisp pixel-art cat silhouette for menu bar / system tray."""
    grid = [
        "..##........##..",
        ".####.####.####.",
        ".##############.",
        ".##############.",
        ".###..####..###.",
        ".###..####..###.",
        ".##############.",
        ".##############.",
        "..############..",
        "..###########.#.",
        "..#############.",
        "..############..",
        "..####....####..",
        "...##......##...",
    ]
    w, h, scale = 36, 36, 2
    ox = (w - 16 * scale) // 2
    oy = (h - 14 * scale) // 2
    buf = bytearray(w * h * 4)
    col = (245, 206, 117, 255)  # Jaune beurre; alpha mask used for macOS template mode
    for gy, row in enumerate(grid):
        for gx, ch in enumerate(row):
            if ch != "#":
                continue
            for dy in range(scale):
                for dx in range(scale):
                    px = ox + gx * scale + dx
                    py = oy + gy * scale + dy
                    idx = (py * w + px) * 4
                    buf[idx] = col[0]
                    buf[idx + 1] = col[1]
                    buf[idx + 2] = col[2]
                    buf[idx + 3] = col[3]
    write_png(png_path, w, h, buf)


def write_ico(ico_path: str, png_entries: list[tuple[int, bytes]]) -> None:
    """Write a multi-resolution Windows .ico wrapping PNG images."""
    header = struct.pack("<HHH", 0, 1, len(png_entries))
    offset = 6 + 16 * len(png_entries)
    dir_bytes = bytearray()
    payload = bytearray()
    for sz, raw in png_entries:
        dim = 0 if sz >= 256 else sz
        dir_bytes.extend(struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(raw), offset))
        payload.extend(raw)
        offset += len(raw)
    with open(ico_path, "wb") as fh:
        fh.write(header)
        fh.write(dir_bytes)
        fh.write(payload)


def build_icns(out_icns: str, master_buf: bytearray | None = None) -> None:
    os.makedirs(os.path.dirname(os.path.abspath(out_icns)), exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="smt_icon_") as tmpdir:
        master_png = os.path.join(tmpdir, "icon_1024.png")
        iconset_dir = os.path.join(tmpdir, "AppIcon.iconset")
        os.makedirs(iconset_dir, exist_ok=True)

        if master_buf is None:
            master_buf = render_icon_rgba(1024)
        write_png(master_png, 1024, 1024, master_buf)

        b512 = _downsample_half(master_buf, 1024)
        b256 = _downsample_half(b512, 512)
        b128 = _downsample_half(b256, 256)
        b64 = _downsample_half(b128, 128)
        b32 = _downsample_half(b64, 64)
        b16 = _downsample_half(b32, 32)
        by_size = {16: b16, 32: b32, 64: b64, 128: b128, 256: b256, 512: b512, 1024: master_buf}

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
            write_png(os.path.join(iconset_dir, fname), px, px, by_size[px])

        if shutil.which("iconutil"):
            subprocess.run(["iconutil", "-c", "icns", iconset_dir, "-o", out_icns], check=True)


def build_tauri_icons(out_dir: str) -> None:
    os.makedirs(out_dir, exist_ok=True)
    b1024 = render_icon_rgba(1024)
    b512 = _downsample_half(b1024, 1024)
    b256 = _downsample_half(b512, 512)
    b128 = _downsample_half(b256, 256)
    b64 = _downsample_half(b128, 128)
    b32 = _downsample_half(b64, 64)

    write_png(os.path.join(out_dir, "32x32.png"), 32, 32, b32)
    write_png(os.path.join(out_dir, "128x128.png"), 128, 128, b128)
    write_png(os.path.join(out_dir, "128x128@2x.png"), 256, 256, b256)
    write_png(os.path.join(out_dir, "icon.png"), 512, 512, b512)
    write_tray_png(os.path.join(out_dir, "tray.png"))

    ico_entries: list[tuple[int, bytes]] = []
    for sz, buf in ((32, b32), (64, b64), (128, b128), (256, b256)):
        tmp_png = os.path.join(out_dir, f".tmp_{sz}.png")
        write_png(tmp_png, sz, sz, buf)
        with open(tmp_png, "rb") as fh:
            ico_entries.append((sz, fh.read()))
        os.unlink(tmp_png)
    write_ico(os.path.join(out_dir, "icon.ico"), ico_entries)

    if shutil.which("iconutil"):
        build_icns(os.path.join(out_dir, "icon.icns"), master_buf=b1024)


def main() -> int:
    if len(sys.argv) == 3 and sys.argv[1] == "--tauri-dir":
        build_tauri_icons(sys.argv[2])
        return 0
    if len(sys.argv) != 2:
        print("Usage: generate_icon.py <output.icns> | --tauri-dir <dir>", file=sys.stderr)
        return 2
    build_icns(sys.argv[1])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
