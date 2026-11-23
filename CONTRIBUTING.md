# Contributing to Yomi

Thank you for helping improve **Yomi**!

## Prerequisites

- **Apple Silicon Mac (`arm64`)** running **macOS 14.0 (Sonoma)** or later
- **Xcode Command Line Tools** (`xcode-select --install`) for `swiftc`
- **Python 3.10+** (standard library only — `server.py` has zero third-party pip dependencies)

## Building & Testing Locally

1. Build and install `Yomi.app` into `/Applications`:
   ```bash
   ./scripts/build_app.sh --install
   codesign --verify --deep --strict "/Applications/Yomi.app"
   ```
   Ensure the Swift build completes with **zero compiler warnings**.

2. Run the native diagnostic self-test:
   ```bash
   "/Applications/Yomi.app/Contents/MacOS/Yomi" --self-test
   ```

3. Verify the local backend serves `server.py` and `userscript/yomi.user.js` cleanly and check for syntax errors:
   ```bash
   python3 -m py_compile server.py scripts/generate_icon.py
   ```

## Repository Hygiene

- **Never commit** `.env`, API keys, personal file paths (`/Users/...`), `cache/`, `dist/`, `server.log`, `server.pid`, personal `glossaries/*.json`, or compiled Swift binaries (`ocr/vision_ocr`, `ocr/annotate_som`).
- Shipped glossary examples belong in `glossaries/examples/`.
- Preserve existing bilingual `FR` / `EN` support and both **Light (Washi Crème)** and **Dark (Sumi-e)** themes across `app/ui/app.html`, `userscript/yomi.user.js`, and the macOS menu bar panel.

## Pull Requests

1. Fork the repository and create a topic branch from `main`.
2. Keep changes focused and verify `./scripts/build_app.sh --install` passes cleanly after your final edit.
3. Open a Pull Request describing what changed and how it was verified.
