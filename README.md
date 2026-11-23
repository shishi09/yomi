# Yomi — Manga Translator for Apple Silicon (macOS)

**Yomi** (from Japanese *yomu* — « to read ») is a native macOS Apple Silicon app (`Yomi.app`) that translates manga pages directly inside your browser (**Safari, Chrome, Firefox, Brave, Arc**) or inside **Yomi Studio** — **100% locally and free with Gemma 4 + Apple Vision**, or optionally via **Gemini API**.

> **Français (résumé rapide) :** Application macOS native Apple Silicon (`Yomi.app`) avec **assistant guidé en 5 étapes** (3 chats pixel-art animés + thème Clair/Sombre/Système, recommandation puce + RAM, installation 1 clic de Python 3 / Ollama / Gemma 4, synchro navigateur automatique), **Yomi Studio** intégré pour traduire/retoucher/exporter des planches en PNG, et icône chat dans la barre de menus. Voir la section française ci-dessous.

---

## Features

- **Native macOS App (`Yomi.app`) & Menu Bar Cat**:
  - Japanese Washi stationery UI (`FR / EN`, `Light / Dark / System` with warm Sumi-e dark mode), **full-bleed native fullscreen mode**, and an animated pixel-art companion cat in the window and macOS menu bar (sleeping when idle, awake when translating, with a **floating menu bar panel that opens even over foreign fullscreen apps**).
  - Matching **Washi Safari popover** and floating reader pill (`Crème / Jaune beurre / Encre / Beige sable / Vert discret`).
  - **5-step first-launch assistant**:
    1. **Appearance**: Pick your mascot (**Mochi**, **Kuro**, or **Sakura**) and theme (**Light / Dark / System**).
    2. **Engine**: **100% Local (Ollama)**, **Gemini API (Cloud)**, or **Both**.
    3. **Setup & Models**: Detects Apple Silicon chip + RAM + Python + Ollama, recommends the right Gemma 4 model for your hardware, and offers 1-click installation (skips Ollama completely in API-only mode).
    4. **Languages & Vision**: Configure source (`English`, `Japanese`, `Auto`), target (`Français`, `English`, `Español`, `Deutsch`, `Italiano`, `Português`), and Set-of-Mark Vision precision.
    5. **Browser**: Direct auto-install & auto-sync into Safari *Userscripts*, or 1-click install URL (`/yomi.user.js`) for Chrome/Arc/Brave/Firefox (*Tampermonkey* / *Violentmonkey*).
- **Yomi Studio (Built-In Page Translator & Bubble Editor)**:
  - Import any image (`PNG / JPG / WebP / HEIC`), drag & drop anywhere, paste with `⌘V`, or click **Sample Page**.
  - **Crop tool**: Frame only the manga panel cleanly before translating screenshots.
  - **Translator editing tools**: Zoom (`50%–300%`), edit bubble text live alongside the original OCR reference, drag/resize bubbles, switch font (**Manga / Modern / Classic**), alignment (**Left / Center / Right**), style (**White / Dark / Floating**), font size (`A− / Auto / A+`), quick-delete list, `⌘Z / ⌘⇧Z` Undo/Redo, hold `Space` to peek at the original, and **Export PNG** at full original resolution.
- **Apple Vision OCR + Closed-Bubble Flood-Fill & Vision Set-of-Mark**:
  - Swift binary using Apple Vision + pixel flood-fill to keep translated text neatly inside speech bubbles, plus numbered Set-of-Mark context boxes and per-series glossaries (`~/Library/Application Support/Yomi/glossaries/`).

---

## Requirements & Hardware Recommendations

- **Mac with Apple Silicon (`arm64` — M1, M2, M3, M4, M5)** running **macOS 14.0 (Sonoma)** or later
- Local Gemma 4 model recommendations (chip + unified memory aware):
  - **8 GB Mac** (or **16 GB M1 / M2 base**): `gemma4:e4b` (fast & light) or Gemini API
  - **16 GB Mac (M3+ / Pro / Max)** or **24 GB M1/M2 base**: `gemma4:12b`
  - **24 GB+ Mac (M3+ / Pro / Max)** or **32 GB+**: `gemma4:26b`
- Browser userscript extension:
  - **Safari**: *Userscripts* (free on Mac App Store, allowed on all websites)
  - **Chrome / Firefox / Brave / Arc**: *Tampermonkey* or *Violentmonkey*

---

## Build & Run from Source

```bash
git clone https://github.com/<your-username>/yomi.git
cd yomi
./scripts/build_app.sh --install
open "/Applications/Yomi.app"
```

*Requires Xcode Command Line Tools (`xcode-select --install`) only when building the Swift binaries from source. Once built, Python 3, Ollama, models, and browser userscripts can all be set up directly inside `Yomi.app`.*

### Packaging a Release (`.zip`)

```bash
./scripts/build_app.sh
ditto -c -k --sequesterRsrc --keepParent dist/Yomi.app dist/Yomi-macOS-arm64.zip
```

---

## Project Structure

- `app/` — Native Swift/AppKit shell (`MangaTranslatorApp.swift`) & embedded WebKit UI (`ui/app.html`)
- `ocr/` — Apple Vision OCR (`vision_ocr.swift`) & Set-of-Mark box annotation (`annotate_som.swift`)
- `userscript/` — Browser overlay & frosted popover script (`yomi.user.js`) for Safari / Chrome / Arc / Firefox / Brave
- `scripts/` — Universal Apple Silicon app bundler (`build_app.sh`) & Washi pixel-cat icon generator (`generate_icon.py`)
- `glossaries/examples/` — Built-in series glossary examples copied into `Application Support`
- `server.py` — Local translation HTTP server (Apple Vision + Ollama Gemma 4 + Gemini API fallback)

---

## Shortcuts

### In Browser (Reader Overlay)
- **`Option + P`**: Preload & translate full chapter in background
- **`Option + T`**: Translate currently visible page
- **`Option + H`**: Toggle translated bubbles on/off
- **`Option + M`**: Open/close floating control panel
- **Click any bubble**: Toggle original source text

### In Yomi Studio
- **`⌘V`**: Paste screenshot from clipboard
- **Hold `Space`**: Temporarily show original page
- **`⌘Z` / `⌘⇧Z`**: Undo / Redo bubble edits
- **`⌘+` / `⌘−` / `⌘0`**: Zoom in / out / fit (`50%–300%`)

---

## Known Limitations

- **Apple Silicon macOS only**: Speech-bubble OCR (`ocr/vision_ocr.swift`) and Set-of-Mark annotation (`ocr/annotate_som.swift`) depend on macOS `Vision`, `AppKit`, and `CoreGraphics`. Intel Macs, Windows, and Linux are not supported.
- **No AI inpainting**: Original text is covered cleanly with matched white/dark speech-bubble overlays or subtle blurred halos rather than neural image inpainting.
- **Vertical Japanese & heavy SFX**: Dense vertical Japanese or stylized hand-drawn SFX are harder for Apple Vision than horizontal English scanlation text.
- **Protected viewers**: Sites rendering pages inside cross-origin `<iframe>`s or encrypted DRM readers cannot be accessed by browser userscripts.
- **Ad-hoc signing**: Builds without a paid Apple Developer ID are ad-hoc signed; if shared via `.zip`, recipients may need **Right-click → Open** once on first launch.

---

## En français (utilisation rapide)

1. Compile et installe l'app avec `./scripts/build_app.sh --install`, puis ouvre **`Yomi.app`** dans `/Applications`.
2. Suis l'assistant en 5 étapes :
   - Choisis ton compagnon chat (**Mochi**, **Kuro** ou **Sakura**) et le thème (**Clair / Sombre / Système**).
   - Choisis ton moteur (**100 % Local Ollama**, **API Gemini**, ou **les deux**).
   - L'app détecte ta puce Apple Silicon + RAM, installe en 1 clic ce qui manque (**Python 3**, **Ollama**) et pré-sélectionne le modèle Gemma 4 adapté à ton Mac.
   - Choisis tes langues source/cible et le mode Vision.
   - Connecte ton navigateur : installation automatique dans le dossier **Userscripts** (Safari) avec mise à jour silencieuse au lancement, ou installation en 1 clic via `/yomi.user.js` dans Chrome/Arc/Brave/Firefox.
3. Ouvre n'importe quel chapitre manga et appuie sur **`Option + P`**, ou clique sur **Yomi Studio** depuis l'accueil pour traduire, recadrer, retoucher les bulles et exporter une planche en PNG.
