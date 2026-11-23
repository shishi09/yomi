#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
APP_DIR="$DIST_DIR/Yomi.app"
PLIST_PATH="$APP_DIR/Contents/Info.plist"

ARCH="$(uname -m)"
if [[ "$ARCH" != "arm64" ]]; then
  echo "Error: Apple Silicon (arm64) Mac required (detected: $ARCH)." >&2
  exit 1
fi

if [[ "${1:-}" != "--skip-build" || ! -d "$APP_DIR" ]]; then
  "$ROOT_DIR/scripts/build_app.sh"
fi

VERSION="2.1.0"
if [[ -f "$PLIST_PATH" ]] && command -v /usr/libexec/PlistBuddy >/dev/null 2>&1; then
  VERSION="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$PLIST_PATH" 2>/dev/null || echo "2.1.0")"
fi

VOL_NAME="Yomi"
OUT_DMG="$DIST_DIR/Yomi-${VERSION}-macOS-arm64.dmg"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/yomi_dmg.XXXXXX")"
STAGE_DIR="$WORK_DIR/stage"
RW_DMG="$WORK_DIR/Yomi-rw.dmg"
ATTACH_DEV=""
MOUNT_DIR=""

cleanup() {
  set +e
  if [[ -n "$ATTACH_DEV" ]]; then
    hdiutil detach "$ATTACH_DEV" -quiet 2>/dev/null || hdiutil detach "$ATTACH_DEV" -force -quiet 2>/dev/null || true
  elif [[ -n "$MOUNT_DIR" && -d "$MOUNT_DIR" ]]; then
    hdiutil detach "$MOUNT_DIR" -quiet 2>/dev/null || hdiutil detach "$MOUNT_DIR" -force -quiet 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR" 2>/dev/null || true
}
trap cleanup EXIT

if [[ -d "/Volumes/$VOL_NAME" ]]; then
  echo "==> Detaching stale /Volumes/$VOL_NAME mount..."
  hdiutil detach "/Volumes/$VOL_NAME" -quiet 2>/dev/null || hdiutil detach "/Volumes/$VOL_NAME" -force -quiet 2>/dev/null || true
fi

mkdir -p "$STAGE_DIR/.background"

echo "==> Staging Yomi.app and Applications link..."
ditto "$APP_DIR" "$STAGE_DIR/Yomi.app"
ln -s /Applications "$STAGE_DIR/Applications"

if [[ -f "$APP_DIR/Contents/Resources/AppIcon.icns" ]]; then
  cp "$APP_DIR/Contents/Resources/AppIcon.icns" "$STAGE_DIR/.VolumeIcon.icns"
fi

echo "==> Generating Washi installer background (620x400)..."
python3 - "$STAGE_DIR/.background/background.png" <<'PY'
import math
import struct
import sys
import zlib

out_path = sys.argv[1]
W, H = 620, 400
buf = bytearray(W * H * 4)

def blend(x: int, y: int, r: int, g: int, b: int, a: float) -> None:
    if not (0 <= x < W and 0 <= y < H) or a <= 0.0:
        return
    idx = (y * W + x) * 4
    inv = 1.0 - a
    buf[idx] = max(0, min(255, int(buf[idx] * inv + r * a)))
    buf[idx + 1] = max(0, min(255, int(buf[idx + 1] * inv + g * a)))
    buf[idx + 2] = max(0, min(255, int(buf[idx + 2] * inv + b * a)))
    buf[idx + 3] = 255

# 1. Crème base (#FAF5EA) + soft Beige sable bottom wave (#F4EBDA)
for y in range(H):
    row_off = y * W * 4
    for x in range(W):
        wave_y = 284.0 + 18.0 * math.sin(x / 95.0) + 6.0 * math.cos(x / 48.0)
        if y >= wave_y + 1.5:
            r, g, b = 244, 235, 218
        elif y >= wave_y - 1.0:
            t = max(0.0, min(1.0, (y - (wave_y - 1.0)) / 2.5))
            r = int(250 + (244 - 250) * t)
            g = int(245 + (235 - 245) * t)
            b = int(234 + (218 - 234) * t)
        else:
            r, g, b = 250, 245, 234
        idx = row_off + x * 4
        buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3] = r, g, b, 255

# 2. Subtle floating butter petals
petals = [(56, 48, 0.35), (94, 72, -0.4), (534, 52, -0.3), (566, 86, 0.45), (312, 58, 0.2)]
for px, py, ang in petals:
    ca, sa = math.cos(ang), math.sin(ang)
    for dy in range(-6, 7):
        for dx in range(-6, 7):
            rx = dx * ca - dy * sa
            ry = dx * sa + dy * ca
            d = (rx / 5.0) ** 2 + (ry / 2.4) ** 2
            if d <= 1.2:
                alpha = max(0.0, min(1.0, (1.2 - d) * 1.4)) * 0.72
                blend(px + dx, py + dy, 239, 195, 106, alpha)

# 3. Dashed rounded drop-zone frames around (175, 195) and (445, 195)
def draw_dashed_box(cx: int, cy: int, half_w: int = 72, half_h: int = 72, rad: int = 20) -> None:
    for y in range(cy - half_h - 3, cy + half_h + 4):
        for x in range(cx - half_w - 3, cx + half_w + 4):
            qx = abs(x - cx) - (half_w - rad)
            qy = abs(y - cy) - (half_h - rad)
            dist = math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - rad
            if abs(dist) <= 1.1:
                if ((x + y) // 9) % 2 == 0:
                    alpha = max(0.0, 1.0 - abs(dist) / 1.1) * 0.85
                    blend(x, y, 214, 198, 170, alpha)

draw_dashed_box(175, 195)
draw_dashed_box(445, 195)

# 4. Crisp Butter-yellow pill & Ink arrow centered at (310, 195)
acx, acy = 310, 195
for y in range(acy - 28, acy + 29):
    for x in range(acx - 28, acx + 29):
        d = math.hypot(x - acx, y - acy)
        if d <= 24.5:
            a = min(1.0, max(0.0, 24.5 - d))
            blend(x, y, 245, 206, 117, a)
        if 23.0 <= d <= 25.0:
            blend(x, y, 224, 181, 90, min(1.0, 1.0 - abs(d - 24.0)) * 0.9)

# Ink right arrow inside circle
def dist_segment(px: float, py: float, x1: float, y1: float, x2: float, y2: float) -> float:
    vx, vy = x2 - x1, y2 - y1
    wx, wy = px - x1, py - y1
    denom = vx * vx + vy * vy
    t = max(0.0, min(1.0, (wx * vx + wy * vy) / denom)) if denom else 0.0
    return math.hypot(px - (x1 + t * vx), py - (y1 + t * vy))

segments = [
    (acx - 9.0, float(acy), acx + 9.0, float(acy)),
    (acx + 2.0, acy - 7.0, acx + 9.5, float(acy)),
    (acx + 2.0, acy + 7.0, acx + 9.5, float(acy)),
]
for y in range(acy - 12, acy + 13):
    for x in range(acx - 13, acx + 14):
        md = min(dist_segment(float(x), float(y), *seg) for seg in segments)
        if md <= 1.9:
            alpha = max(0.0, min(1.0, 1.9 - md))
            blend(x, y, 35, 30, 26, alpha)

stride = W * 4
raw = bytearray((stride + 1) * H)
for y in range(H):
    raw[y * (stride + 1)] = 0
    raw[y * (stride + 1) + 1 : y * (stride + 1) + 1 + stride] = buf[y * stride : (y + 1) * stride]

def chunk(tag: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

with open(out_path, "wb") as fh:
    fh.write(b"\x89PNG\r\n\x1a\n")
    fh.write(chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0)))
    fh.write(chunk(b"IDAT", zlib.compress(bytes(raw), level=6)))
    fh.write(chunk(b"IEND", b""))
PY

STAGE_KB="$(du -sk "$STAGE_DIR" | awk '{print $1 + 40960}')"
echo "==> Creating temporary read-write disk image..."
hdiutil create \
  -srcfolder "$STAGE_DIR" \
  -volname "$VOL_NAME" \
  -fs HFS+ \
  -fsargs "-c c=64,a=16,e=16" \
  -size "${STAGE_KB}k" \
  -format UDRW \
  "$RW_DMG" >/dev/null

echo "==> Attaching disk image to configure Finder layout..."
ATTACH_OUT="$(hdiutil attach -readwrite -noverify -noautoopen "$RW_DMG")"
ATTACH_DEV="$(printf '%s\n' "$ATTACH_OUT" | awk '/\/Volumes\// {print $1; exit}')"
MOUNT_DIR="$(printf '%s\n' "$ATTACH_OUT" | sed -n 's/.*\(\/Volumes\/.*\)$/\1/p' | head -n 1)"

if [[ -z "$MOUNT_DIR" || ! -d "$MOUNT_DIR" ]]; then
  echo "Error: Failed to mount temporary DMG." >&2
  exit 1
fi

if command -v SetFile >/dev/null 2>&1 && [[ -f "$MOUNT_DIR/.VolumeIcon.icns" ]]; then
  SetFile -a C "$MOUNT_DIR" 2>/dev/null || true
fi

DISK_NAME="$(basename "$MOUNT_DIR")"
python3 - "$DISK_NAME" <<'PY' || echo "Note: Finder layout skipped (Automation permission or headless shell)"
import subprocess
import sys

disk_name = sys.argv[1]
script = f'''
tell application "Finder"
  tell disk "{disk_name}"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {{200, 140, 820, 540}}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 112
    set background picture of opts to file ".background:background.png"
    set position of item "Yomi.app" of container window to {{175, 195}}
    set position of item "Applications" of container window to {{445, 195}}
    close
    open
    update without registering applications
    delay 1
  end tell
end tell
'''
try:
    subprocess.run(["osascript", "-e", script], check=True, timeout=8, capture_output=True, text=True)
except Exception as exc:
    raise SystemExit(1)
PY

rm -rf "$MOUNT_DIR/.fseventsd" 2>/dev/null || true
sync
sleep 1

echo "==> Detaching temporary disk image..."
hdiutil detach "$ATTACH_DEV" -quiet 2>/dev/null || hdiutil detach "$ATTACH_DEV" -force -quiet
ATTACH_DEV=""
MOUNT_DIR=""

echo "==> Compressing final DMG ($OUT_DMG)..."
rm -f "$OUT_DMG"
hdiutil convert "$RW_DMG" -format UDZO -imagekey zlib-level=9 -o "$OUT_DMG" >/dev/null

echo "==> Verifying DMG integrity..."
hdiutil verify "$OUT_DMG"

echo "Built DMG: $OUT_DMG"
du -h "$OUT_DMG" | awk '{print "Size: " $1}'
shasum -a 256 "$OUT_DMG"
