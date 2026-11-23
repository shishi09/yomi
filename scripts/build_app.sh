#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
APP_NAME="Yomi.app"
APP_DIR="$DIST_DIR/$APP_NAME"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RES_DIR="$CONTENTS_DIR/Resources"

ARCH="$(uname -m)"
if [[ "$ARCH" != "arm64" ]]; then
  echo "Error: Apple Silicon (arm64) Mac required (detected: $ARCH)." >&2
  exit 1
fi

echo "==> Compiling Apple Vision OCR & Set-of-Mark helpers (arm64)..."
mkdir -p "$ROOT_DIR/ocr"
swiftc -O -target arm64-apple-macosx14.0 \
  -framework Vision -framework ImageIO -framework CoreGraphics \
  "$ROOT_DIR/ocr/vision_ocr.swift" -o "$ROOT_DIR/ocr/vision_ocr"

swiftc -O -target arm64-apple-macosx14.0 \
  -framework AppKit -framework ImageIO -framework CoreGraphics \
  "$ROOT_DIR/ocr/annotate_som.swift" -o "$ROOT_DIR/ocr/annotate_som"

echo "==> Packaging $APP_NAME..."
rm -rf "$APP_DIR" "$DIST_DIR/Manga Translator.app"
mkdir -p "$MACOS_DIR" "$RES_DIR/ocr" "$RES_DIR/userscript" "$RES_DIR/ui" "$RES_DIR/glossaries/examples"

echo "==> Compiling native macOS app binary (arm64)..."
swiftc -O -target arm64-apple-macosx14.0 \
  -framework AppKit -framework WebKit \
  "$ROOT_DIR/app/MangaTranslatorApp.swift" -o "$MACOS_DIR/Yomi"

cat > "$CONTENTS_DIR/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>Yomi</string>
  <key>CFBundleIdentifier</key>
  <string>com.yomi.app</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Yomi</string>
  <key>CFBundleDisplayName</key>
  <string>Yomi</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleShortVersionString</key>
  <string>2.1.0</string>
  <key>CFBundleVersion</key>
  <string>2</string>
  <key>LSArchitecturePriority</key>
  <array>
    <string>arm64</string>
  </array>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

echo "==> Generating pixel-art cat AppIcon.icns..."
python3 "$ROOT_DIR/scripts/generate_icon.py" "$RES_DIR/AppIcon.icns"

cp "$ROOT_DIR/server.py" "$RES_DIR/server.py"
cp "$ROOT_DIR/ocr/vision_ocr" "$RES_DIR/ocr/vision_ocr"
cp "$ROOT_DIR/ocr/annotate_som" "$RES_DIR/ocr/annotate_som"
chmod +x "$RES_DIR/ocr/vision_ocr" "$RES_DIR/ocr/annotate_som" "$MACOS_DIR/Yomi"
cp "$ROOT_DIR/userscript/yomi.user.js" "$RES_DIR/userscript/yomi.user.js"
cp "$ROOT_DIR/app/ui/app.html" "$RES_DIR/ui/app.html"
if [[ -d "$ROOT_DIR/glossaries/examples" ]]; then
  cp -R "$ROOT_DIR/glossaries/examples/"* "$RES_DIR/glossaries/examples/" 2>/dev/null || true
fi

echo "==> Ad-hoc code signing..."
codesign --force --deep --sign - "$APP_DIR"
codesign --verify --deep --strict "$APP_DIR"
echo "Built: $APP_DIR"

if [[ "${1:-}" == "--install" ]]; then
  DEST="/Applications/$APP_NAME"
  echo "==> Installing to $DEST..."
  rm -rf "/Applications/Manga Translator.app" "$DEST"
  ditto "$APP_DIR" "$DEST"
  codesign --verify --deep --strict "$DEST"
  echo "Installed: $DEST"
fi
