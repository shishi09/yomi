# Yomi — Cross-Platform Manga Translator & Studio (Rust + Tauri)

**Yomi** (from Japanese *yomu* — « to read ») is a compiled, zero-Python **Rust + Tauri v2** desktop app (**macOS, Windows, Linux**) that translates manga pages live inside your browser (**Safari, Chrome, Firefox, Brave, Arc, Edge**) or inside **Yomi Studio** — **100% locally and free with Ollama (Gemma 4) + OS-native OCR**, or via **any Cloud API provider** (**Google Gemini, OpenAI, Anthropic Claude, OpenRouter, Mistral, DeepSeek, Groq, xAI**, or any OpenAI-compatible endpoint).

> **Français (résumé rapide) :** Application compilée **Rust + Tauri v2** (zéro Python requis) multi-plateformes (**macOS Apple Silicon, Windows, Linux**) gardant **Apple Vision** natif sur Mac, **Windows.Media.Ocr** sur Windows et **Tesseract** sur Linux. Comprend un assistant guidé en 5 étapes, **Yomi Studio**, la barre de menus / zone de notification, les glossaires par série et 9 fournisseurs Cloud API. Voir la section française ci-dessous.

---

## Architecture Highlights

- **Compiled Rust Core (`src-tauri/`)**:
  - Embedded **`axum` + `tokio`** HTTP server (`127.0.0.1:8765`) — zero Python installation or virtualenvs required.
  - Pure-Rust image flood-fill (`image_proc.rs`) and JPEG **Set-of-Mark** renderer (`som.rs`) — zero temp-file subprocess overhead for bubble annotation.
  - SSRF-safe remote image fetcher, disk + prefetch cache, and series glossaries.
- **OS-Native OCR (`src-tauri/src/ocr/`)**:
  - **macOS**: Native **Apple Vision** (`ocr/vision_ocr.swift` compiled & bundled directly in `Yomi.app`).
  - **Windows**: Native **`Windows.Media.Ocr`** built into Windows 10/11.
  - **Linux**: **Tesseract OCR** (`tesseract-ocr`).
- **Tauri v2 Native Shell & Tray**:
  - Same Washi stationery UI (`FR / EN`, `Light / Dark / System` with warm Sumi-e dark mode), native fullscreen layout, OS dialogs (`tauri-plugin-dialog`), clipboard paste (`arboard`), and menu bar / system tray companion cat.

---

## Features

- **5-step first-launch assistant**:
  1. **Appearance**: Pick your mascot (**Mochi**, **Kuro**, or **Sakura**) and theme (**Light / Dark / System**).
  2. **Engine**: **100% Local (Ollama)**, **Cloud API (Any Provider)** — Google Gemini, OpenAI, Anthropic Claude, OpenRouter, Mistral, DeepSeek, Groq, xAI, or Custom OpenAI-compatible Base URL — or **Both**.
  3. **Setup & Models**: Detects CPU/chip + RAM + Ollama + native OS OCR, recommends the right Gemma 4 model for your hardware, and offers 1-click Ollama setup (skips Ollama completely in API-only mode).
  4. **Languages & Vision**: Configure source (`English`, `Japanese`, `Auto`), target (`Français`, `English`, `Español`, `Deutsch`, `Italiano`, `Português`), and Set-of-Mark Vision precision.
  5. **Browser**: Auto-install into Safari *Userscripts* (macOS), or 1-click install URL (`/yomi.user.js`) for Chrome/Edge/Arc/Brave/Firefox (*Tampermonkey* / *Violentmonkey*).
- **Yomi Studio (Built-In Page Translator & Bubble Editor)**:
  - Import any image (`PNG / JPG / WebP / GIF / BMP`), drag & drop anywhere, paste with `⌘V` / `Ctrl+V`, or click **Sample Page**.
  - **Crop tool**, **Zoom (`50%–300%`)**, live text editor with original OCR reference, drag/resize bubbles, font & style controls (**White / Dark / Floating**), Undo/Redo, hold `Space` to peek original, and full-res **Export PNG**.

---

## Requirements & Hardware Recommendations

- **OS**: **macOS 14+ (Apple Silicon)**, **Windows 10/11**, or **Linux** (`webkit2gtk-4.1` + `tesseract-ocr` for local OCR)
- Local Gemma 4 model recommendations (RAM aware):
  - **8 GB RAM** (or **16 GB M1 / M2 base**): `gemma4:e4b` (fast & light) or Cloud API
  - **16 GB RAM (M3+ / Pro / desktop)** or **24 GB M1/M2 base**: `gemma4:12b`
  - **24–32 GB+ RAM**: `gemma4:26b`
- Browser userscript extension:
  - **Safari (macOS)**: *Userscripts* (free on Mac App Store, allowed on all websites)
  - **Chrome / Edge / Firefox / Brave / Arc**: *Tampermonkey* or *Violentmonkey*

---

## Download & Install (Recommended)

1. Download the installer for your OS from the [Releases](https://github.com/shishi09/yomi/releases) page:
   - **macOS (Apple Silicon)**: `Yomi-3.0.0-macOS-arm64.dmg` (drag **`Yomi.app`** into **`Applications`**)
   - **Windows / Linux**: `.msi` / `.exe` / `.AppImage` / `.deb`
2. Launch **Yomi** and follow the 5-step guided setup.

> **macOS first launch notice:** Builds distributed without a paid Apple Developer ID are ad-hoc signed. If macOS prevents opening on first launch, click **Open Anyway** in **System Settings → Privacy & Security**, or run `xattr -cr "/Applications/Yomi.app"` once.

---

## Build & Run from Source

```bash
git clone https://github.com/shishi09/yomi.git
cd yomi
npm install
python3 scripts/generate_icon.py --tauri-dir src-tauri/icons

# Run Rust test suite
cargo test --manifest-path src-tauri/Cargo.toml

# Build & install on macOS Apple Silicon:
./scripts/build_app.sh --install
open "/Applications/Yomi.app"

# Or build cross-platform Tauri bundle on any OS:
npx tauri build
```

### Packaging a macOS `.dmg` Release

```bash
./scripts/build_dmg.sh
```

Builds `dist/Yomi-3.0.0-macOS-arm64.dmg` with drag-to-Applications layout ready to attach to a GitHub Release.

---

## Project Structure

- `src-tauri/` — Compiled Rust + Tauri v2 backend & shell (`config`, `glossary`, `image_proc`, `som`, `ocr/{macos,windows,linux}`, `prompts`, `llm`, `cache`, `server`, `tray`, `bridge`)
- `app/ui/` — Shared Washi HTML/CSS/JS application & **Yomi Studio** (`app.html`)
- `ocr/` — Native **Apple Vision** Swift helper (`vision_ocr.swift`, compiled automatically on macOS by `build.rs`)
- `userscript/` — Cross-browser overlay & frosted popover script (`yomi.user.js`)
- `scripts/` — macOS `.app` / `.dmg` packagers & cross-platform icon generator (`generate_icon.py`)
- `glossaries/examples/` — Built-in series glossary examples
- `.github/workflows/` — Cross-platform GitHub Actions CI (`macos-latest`, `windows-latest`, `ubuntu-22.04`)

---

## Shortcuts

### In Browser (Reader Overlay)
- **`Option + P` / `Alt + P`**: Preload & translate full chapter in background
- **`Option + T` / `Alt + T`**: Translate currently visible page
- **`Option + H` / `Alt + H`**: Toggle translated bubbles on/off
- **`Option + M` / `Alt + M`**: Open/close floating control panel
- **Click any bubble**: Toggle original source text

### In Yomi Studio
- **`⌘V` / `Ctrl+V`**: Paste screenshot from clipboard
- **Hold `Space`**: Temporarily show original page
- **`⌘Z` / `⌘⇧Z`**: Undo / Redo bubble edits
- **`⌘+` / `⌘−` / `⌘0`**: Zoom in / out / fit (`50%–300%`)

---

## Known Limitations

- **No AI inpainting**: Original text is covered cleanly with matched white/dark speech-bubble overlays or subtle blurred halos rather than neural image inpainting.
- **Vertical Japanese & heavy SFX**: Dense vertical Japanese or stylized hand-drawn SFX are harder for OS OCR than horizontal English scanlation text.
- **Protected viewers**: Sites rendering pages inside cross-origin `<iframe>`s or encrypted DRM readers cannot be accessed by browser userscripts.
- **Ad-hoc macOS signing**: Builds without a paid Apple Developer ID are ad-hoc signed; recipients may need **System Settings → Privacy & Security → Open Anyway** once on first launch.

---

## Feedback, Bugs & Ideas

Have feedback, spotted a bug, or want a new feature or API provider? You can reach out directly from **Yomi → Settings → About** or use:

- **[Report a bug](https://github.com/shishi09/yomi/issues/new?template=bug_report.md)**
- **[Suggest a feature or improvement](https://github.com/shishi09/yomi/issues/new?template=feature_request.md)**
- **[GitHub Discussions](https://github.com/shishi09/yomi/discussions)** — questions, general feedback, upvoting ideas, and community manga glossaries

---

## En français (utilisation rapide)

1. Télécharge l'installeur dans **Releases** (`Yomi-3.0.0-macOS-arm64.dmg` sur Mac Apple Silicon, ou `.msi`/`.AppImage` sur Windows/Linux) — **aucun Python à installer**.
   - *Premier lancement macOS :* comme l’application est signée localement sans certificat Apple Developer payant, clique sur **Ouvrir quand même** dans *Réglages Système → Confidentialité et sécurité* (ou exécute `xattr -cr "/Applications/Yomi.app"` une seule fois) si macOS bloque l’ouverture.
2. Ouvre **Yomi** et suis l'assistant en 5 étapes :
   - Choisis ton compagnon chat (**Mochi**, **Kuro** ou **Sakura**) et le thème (**Clair / Sombre / Système**).
   - Choisis ton moteur (**100 % Local Ollama**, **API Cloud (Gemini, OpenAI, Claude, OpenRouter, Mistral, DeepSeek, Groq, xAI ou URL OpenAI-compatible)**, ou **les deux**).
   - L'app vérifie ton matériel, l'OCR natif de ton système (**Apple Vision** sur Mac, **Windows OCR** sur Windows, **Tesseract** sur Linux) et propose Ollama en 1 clic.
   - Choisis tes langues source/cible et le mode Vision.
   - Connecte ton navigateur : installation automatique dans **Userscripts** (Safari sur Mac), ou installation en 1 clic via `/yomi.user.js` dans Chrome/Edge/Arc/Brave/Firefox.
3. Ouvre n'importe quel chapitre manga et appuie sur **`Option + P`** (ou **`Alt + P`**), ou clique sur **Yomi Studio** depuis l'accueil pour traduire, recadrer, retoucher les bulles et exporter une planche en PNG.

### Retours & améliorations

Tu peux envoyer un retour directement depuis **Yomi → Paramètres → À propos** ou ici sur GitHub :

- **[Signaler un bug](https://github.com/shishi09/yomi/issues/new?template=bug_report.md)**
- **[Proposer une amélioration](https://github.com/shishi09/yomi/issues/new?template=feature_request.md)**
- **[GitHub Discussions](https://github.com/shishi09/yomi/discussions)** — avis généraux, questions d'installation, votes sur les idées et partage de glossaires manga
