#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
APP_NAME="Yomi.app"
APP_DIR="$DIST_DIR/$APP_NAME"
TAURI_APP="$ROOT_DIR/src-tauri/target/release/bundle/macos/Yomi.app"

ARCH="$(uname -m)"
if [[ "$ARCH" != "arm64" ]]; then
  echo "Error: Apple Silicon (arm64) Mac required (detected: $ARCH)." >&2
  exit 1
fi

echo "==> Generating cross-platform icons..."
python3 "$ROOT_DIR/scripts/generate_icon.py" --tauri-dir "$ROOT_DIR/src-tauri/icons"

echo "==> Building compiled Rust + Tauri v2 bundle ($APP_NAME)..."
cd "$ROOT_DIR"
npx tauri build --bundles app

if [[ ! -d "$TAURI_APP" ]]; then
  echo "Error: Expected Tauri bundle not found at $TAURI_APP" >&2
  exit 1
fi

echo "==> Staging $APP_DIR..."
mkdir -p "$DIST_DIR"
rm -rf "$APP_DIR" "$DIST_DIR/Manga Translator.app"
ditto "$TAURI_APP" "$APP_DIR"

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
