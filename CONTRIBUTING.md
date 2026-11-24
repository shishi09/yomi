# Contributing to Yomi

Thank you for helping improve **Yomi**!

## Prerequisites

- **Rust** (`1.80+`) & **Node.js 20+** (`npm install` installs `@tauri-apps/cli`)
- **Platform tools**:
  - **macOS (`arm64`)**: macOS 14+ & Xcode Command Line Tools (`xcode-select --install` for `swiftc` + Apple Vision)
  - **Windows 10/11**: MSVC Build Tools & WebView2
  - **Linux**: `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, and `tesseract-ocr`

## Building & Testing Locally

1. Generate app & tray icons and run the Rust test suite:
   ```bash
   python3 scripts/generate_icon.py --tauri-dir src-tauri/icons
   cargo test --manifest-path src-tauri/Cargo.toml
   ```

2. Build and install `Yomi.app` on macOS:
   ```bash
   ./scripts/build_app.sh --install
   codesign --verify --deep --strict "/Applications/Yomi.app"
   ```

3. Run the native diagnostic self-test:
   ```bash
   "/Applications/Yomi.app/Contents/MacOS/yomi" --self-test
   ```

4. Check Python icon generator syntax & userscript:
   ```bash
   python3 -m py_compile scripts/generate_icon.py
   ```

## Repository Hygiene

- **Never commit** `.env`, API keys, personal file paths (`/Users/...`), `node_modules/`, `src-tauri/target/`, `src-tauri/resources/vision_ocr`, `cache/`, `dist/`, `*.log`, personal `glossaries/*.json`, or compiled binaries.
- Shipped glossary examples belong in `glossaries/examples/`.
- Preserve existing bilingual `FR` / `EN` support and both **Light (Washi Crème)** and **Dark (Sumi-e)** themes across `app/ui/app.html` and `userscript/yomi.user.js`.

## Pull Requests

1. Fork the repository and create a topic branch from `main`.
2. Keep changes focused and verify `cargo test --manifest-path src-tauri/Cargo.toml` passes cleanly after your final edit.
3. Open a Pull Request describing what changed and how it was verified.
