// ==UserScript==
// @name         Yomi — Manga Translator
// @namespace    local.yomi.manga
// @version      2.1.0
// @downloadURL  http://127.0.0.1:8765/yomi.user.js
// @updateURL    http://127.0.0.1:8765/yomi.user.js
// @description  Traduit les planches de manga (EN/JA -> FR/EN/ES/DE/IT/PT) sur macOS Apple Silicon (Apple Vision + Ollama Gemma 4 ou API Gemini) dans Safari, Chrome, Firefox et Brave
// @match        *://*/*
// @grant        GM.xmlHttpRequest
// @grant        GM_xmlhttpRequest
// @connect      127.0.0.1
// @connect      localhost
// @connect      *
// @run-at       document-idle
// ==/UserScript==

(function () {
  "use strict";

  if (window.top !== window.self) return;
  if (location.hostname === "127.0.0.1" && location.port === "8765") return;

  const SERVER_URL = "http://127.0.0.1:8765";
  const STORAGE_KEY = "smt_prefs_v4";

  const DEFAULT_PREFS = {
    model: "ollama:gemma4:12b",
    sourceLang: "en",
    targetLang: "fr",
    theme: "system",
    mascot: "mochi",
    autoMode: false,
    visible: true,
    visionMode: "auto",
    glossarySlug: "auto",
  };

  const CAT_PALETTES = {
    mochi: { k: "#1f2330", w: "#fbf8f1", s: "#e7dfd2", o: "#f29b4b", p: "#f48fb1", e: "#1b1e28", g: "#ffffff", c: "#d6ccb8", b: "#f29b4b" },
    kuro: { k: "#0d0f17", w: "#252938", s: "#3d435c", o: "#1a1d29", p: "#b57592", e: "#ffd166", g: "#ffffff", c: "#ff4f7a", b: "#ffd166" },
    sakura: { k: "#3b2636", w: "#fff5f8", s: "#eedbe5", o: "#7a5368", p: "#ff7aa2", e: "#3b9eff", g: "#ffffff", c: "#ff7aa2", b: "#fff5f8" },
  };
  const CAT_FACE_MINI = [
    "..kk........kk..",
    ".kpk.kkkkkk.kok.",
    ".kwkkwoooowkkok.",
    ".kwwwoooooowwwk.",
    ".kwwgewwwwgewwk.",
    ".kwweewwwweewwk.",
    ".kwppwwppwwppwk.",
    ".kwwwwwwwwwwwwk.",
    "..kkkccbbcckkk..",
    "..ksswwwwwwssk..",
    "..kswwwwwwwwskk.",
    "..ksswwwwwwskok.",
    "..kwwksssskwwk..",
    "...kk......kk...",
  ];

  function renderMiniCatSvg(mascotId) {
    const pal = CAT_PALETTES[mascotId] || CAT_PALETTES.mochi;
    let rects = "";
    for (let y = 0; y < CAT_FACE_MINI.length; y++) {
      const row = CAT_FACE_MINI[y];
      for (let x = 0; x < row.length; x++) {
        const ch = row[x];
        if (ch !== "." && pal[ch]) {
          rects += `<rect x="${x}" y="${y}" width="1" height="1" fill="${pal[ch]}"/>`;
        }
      }
    }
    return `<svg viewBox="0 0 16 14" shape-rendering="crispEdges" aria-hidden="true">${rects}</svg>`;
  }

  function shortModelLabel(modelId, rawLabel) {
    const id = String(modelId || "");
    if (id.startsWith("ollama:")) {
      const tag = id.slice("ollama:".length);
      const m = tag.match(/^gemma4:(e?\d+b)/i);
      if (m) return `Gemma 4 ${m[1].toUpperCase()}`;
      if (rawLabel) return String(rawLabel).replace(/^Local\s*[·•-]\s*/i, "").replace(/\s*\([^)]*\)/g, "").trim() || tag;
      return tag;
    }
    if (id.startsWith("gemini-")) {
      return id
        .replace(/^gemini-/, "Gemini ")
        .replace(/-flash-lite/i, " Flash-Lite")
        .replace(/-flash/i, " Flash")
        .replace(/-pro/i, " Pro");
    }
    if (rawLabel) {
      return String(rawLabel).replace(/^API\s+/i, "").replace(/\s*\([^)]*\)/g, "").trim() || id;
    }
    const colon = id.indexOf(":");
    if (colon > 0) {
      const prov = id.slice(0, colon);
      const rest = id.slice(colon + 1).split("/").pop();
      return `${prov.charAt(0).toUpperCase() + prov.slice(1)} · ${rest}`;
    }
    return id;
  }

  function modelSupportsVision(model) {
    if (!model) return false;
    const low = String(model).toLowerCase();
    if (low.startsWith("ollama:")) {
      return ["gemma4", "gemma3", "llava", "qwen2.5vl", "qwen3-vl", "minicpm-v", "llama3.2-vision", "vision"].some(
        (tag) => low.includes(tag)
      );
    }
    if (low.startsWith("deepseek:")) return false;
    if (low.startsWith("groq:")) return low.includes("llama-4") || low.includes("vision");
    if (["o1-mini", "deepseek-r1", "deepseek-chat", "deepseek-reasoner"].some((t) => low.includes(t))) return false;
    return true;
  }

  function isVisionEffective() {
    return Boolean(prefs.visionMode !== "off" && modelSupportsVision(prefs.model));
  }

  function getBatchSize() {
    // Mode Vision local : 1 page par appel (précision max + mémoire entre pages)
    // Mode Texte local : 2 pages par lot | API cloud : 4 pages par lot
    if (prefs.model.startsWith("ollama:")) {
      return isVisionEffective() ? 1 : 2;
    }
    return 4;
  }

  let hasSavedTargetLang = false;
  let hasSavedTheme = false;

  function loadPrefs() {
    try {
      const rawV4 = localStorage.getItem(STORAGE_KEY);
      if (rawV4) {
        const saved = JSON.parse(rawV4);
        hasSavedTargetLang = Boolean(saved.targetLang);
        hasSavedTheme = Boolean(saved.theme);
        const merged = { ...DEFAULT_PREFS, ...saved };
        if (!["light", "dark", "system"].includes(merged.theme)) merged.theme = "system";
        if (!["mochi", "kuro", "sakura"].includes(merged.mascot)) merged.mascot = "mochi";
        if (merged.visionMode === true) merged.visionMode = "auto";
        else if (merged.visionMode === false) merged.visionMode = "off";
        if (!["auto", "on", "off"].includes(merged.visionMode)) merged.visionMode = "auto";
        return merged;
      }
      // Migration douce depuis v3 / v2
      const rawOld = localStorage.getItem("smt_prefs_v3") || localStorage.getItem("smt_prefs_v2");
      if (rawOld) {
        const old = JSON.parse(rawOld);
        const migrated = {
          ...DEFAULT_PREFS,
          model: old.model || DEFAULT_PREFS.model,
          sourceLang: old.sourceLang || DEFAULT_PREFS.sourceLang,
          autoMode: Boolean(old.autoMode),
          visible: old.visible !== undefined ? Boolean(old.visible) : true,
          visionMode: old.visionMode === false || old.visionMode === "off" ? "off" : (old.visionMode === "on" ? "on" : "auto"),
          glossarySlug: old.glossarySlug || "auto",
        };
        if (typeof migrated.model === "string" && migrated.model.startsWith("ollama:") && !migrated.model.startsWith("ollama:gemma4")) {
          migrated.model = DEFAULT_PREFS.model;
        }
        savePrefs(migrated);
        return migrated;
      }
      return { ...DEFAULT_PREFS };
    } catch {
      return { ...DEFAULT_PREFS };
    }
  }

  function savePrefs(prefs) {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
    } catch {}
  }

  const prefs = loadPrefs();
  const state = {
    panelOpen: false,
    serverOnline: false,
    apiKeyOk: false,
    overlays: new Map(), // Element -> { layer, bubbles, signature, translating }
    syncScheduled: false,
    forcedVisible: false,
    preloadRunning: false,
    preloadAbort: false,
    quotaBlockedModel: null,
    lastUrl: location.href,
    glossaries: [],
    detectedGlossarySlug: null,
    editorSlug: null,
  };

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  // --- Requêtes HTTP compatibles Safari Userscripts (contourne Mixed Content & CORS) ---
  function gmRequest(options) {
    const fn =
      (typeof GM !== "undefined" && typeof GM.xmlHttpRequest === "function" && GM.xmlHttpRequest) ||
      (typeof GM_xmlhttpRequest === "function" && GM_xmlhttpRequest);

    if (fn) {
      return new Promise((resolve, reject) => {
        try {
          const maybePromise = fn({
            method: options.method || "GET",
            url: options.url,
            headers: options.headers || {},
            data: options.data,
            responseType: options.responseType,
            timeout: options.timeout || 50000,
            onload: (res) => resolve(res),
            onerror: (err) => reject(new Error(err?.error || "Erreur réseau extension")),
            ontimeout: () => reject(new Error("Délai dépassé (serveur local trop lent)")),
          });
          if (maybePromise && typeof maybePromise.then === "function") {
            maybePromise.then(resolve, reject);
          }
        } catch (err) {
          reject(err);
        }
      });
    }

    return fetch(options.url, {
      method: options.method || "GET",
      headers: options.headers || {},
      body: options.data,
    }).then(async (res) => {
      if (options.responseType === "blob") {
        return { status: res.status, response: await res.blob() };
      }
      return { status: res.status, responseText: await res.text() };
    });
  }

  async function apiJson(path, payload = null, timeoutMs = 50000) {
    const res = await gmRequest({
      method: payload ? "POST" : "GET",
      url: SERVER_URL + path,
      headers: payload ? { "Content-Type": "application/json" } : {},
      data: payload ? JSON.stringify(payload) : undefined,
      timeout: timeoutMs,
    });
    const text = res.responseText || "";
    let data = {};
    try {
      data = JSON.parse(text);
    } catch {
      throw new Error(`Réponse invalide (${res.status}): ${text.slice(0, 120)}`);
    }
    if (res.status < 200 || res.status >= 300) {
      const err = new Error(data.error || `Erreur HTTP ${res.status}`);
      err.status = res.status;
      err.data = data;
      throw err;
    }
    return data;
  }

  // --- Styles injectés (intégration naturelle Niveau 1 enrichi) ---
  function injectStyles() {
    if (document.getElementById("smt-styles")) return;
    const style = document.createElement("style");
    style.id = "smt-styles";
    style.textContent = `
      #smt-overlay-root {
        position: absolute;
        top: 0; left: 0;
        width: 0; height: 0;
        pointer-events: none;
        z-index: 2147483600;
      }
      .smt-page-layer {
        position: absolute;
        pointer-events: none;
        overflow: hidden;
      }
      .smt-page-layer.smt-hidden .smt-bubble {
        display: none !important;
      }
      .smt-bubble {
        position: absolute;
        pointer-events: auto;
        display: flex;
        align-items: center;
        justify-content: center;
        text-align: center;
        padding: 3px 5px;
        font-family: "CC Wild Words", "Anime Ace 2.0 BB", "Avenir Next Condensed", "Chalkboard SE", -apple-system, BlinkMacSystemFont, sans-serif;
        font-weight: 700;
        line-height: 1.13;
        letter-spacing: -0.01em;
        text-wrap: balance;
        overflow-wrap: break-word;
        hyphens: auto;
        cursor: pointer;
        user-select: text;
        overflow: hidden;
        transition: opacity 0.14s ease;
        box-sizing: border-box;
      }
      .smt-bubble.smt-style-bubble_light {
        background: #ffffff;
        color: #111111;
        border-radius: 26% / 20%;
        box-shadow: 0 0 4px 2px #ffffff;
      }
      .smt-bubble.smt-style-bubble_dark {
        background: #141414;
        color: #f8f8f8;
        border-radius: 26% / 20%;
        box-shadow: 0 0 4px 2px #141414;
      }
      .smt-bubble.smt-style-floating {
        background: rgba(255, 255, 255, 0.78);
        backdrop-filter: blur(4px);
        -webkit-backdrop-filter: blur(4px);
        color: #111111;
        border-radius: 10px;
        box-shadow: 0 0 5px 2px rgba(255, 255, 255, 0.70);
        text-shadow: -1.5px -1.5px 0 #fff, 1.5px -1.5px 0 #fff, -1.5px 1.5px 0 #fff, 1.5px 1.5px 0 #fff, 0 0 6px #fff;
        text-transform: uppercase;
      }
      .smt-bubble:hover {
        outline: 1.5px solid rgba(196, 142, 42, 0.65);
      }
      .smt-bubble.smt-peek {
        opacity: 0.08;
      }
      .smt-loading-badge {
        position: absolute;
        top: 12px; right: 12px;
        background: rgba(15, 17, 23, 0.84);
        color: #fff;
        font-family: -apple-system, BlinkMacSystemFont, sans-serif;
        font-size: 11px;
        font-weight: 600;
        padding: 5px 9px;
        border-radius: 999px;
        box-shadow: 0 4px 14px rgba(0,0,0,0.3);
        pointer-events: none;
      }

      #smt-ui {
        --y-bg: rgba(28, 25, 21, 0.90);
        --y-surface: rgba(40, 36, 30, 0.86);
        --y-surface-hi: rgba(52, 46, 38, 0.96);
        --y-sep: rgba(246, 212, 139, 0.10);
        --y-border: rgba(246, 212, 139, 0.16);
        --y-text: #F6EFE2;
        --y-muted: #A99D8E;
        --y-accent: #F2CA74;
        --y-accent-h: #E5B758;
        --y-accent-soft: rgba(242, 202, 116, 0.20);
        --y-on-accent: #1D1814;
        --y-green: #7FB069;
        --y-green-soft: rgba(127, 176, 105, 0.18);
        --y-warn: #E8A849;
        --y-warn-soft: rgba(232, 168, 73, 0.18);
        --y-red: #E56B63;
        --y-red-soft: rgba(229, 107, 99, 0.18);
        --y-switch-off: rgba(246, 239, 226, 0.22);
        --y-kbd: rgba(246, 239, 226, 0.10);
        --y-input: rgba(24, 21, 18, 0.86);
        --y-shadow: 0 18px 48px rgba(0, 0, 0, 0.48);
        position: fixed;
        right: 16px;
        bottom: 16px;
        z-index: 2147483645;
        font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif;
        color: var(--y-text);
        display: none;
        line-height: 1.35;
        -webkit-font-smoothing: antialiased;
      }
      #smt-ui * { box-sizing: border-box; }
      #smt-ui.smt-visible { display: block; }
      #smt-ui.smt-theme-light {
        --y-bg: rgba(250, 245, 234, 0.92);
        --y-surface: rgba(244, 235, 218, 0.88);
        --y-surface-hi: rgba(252, 248, 240, 0.98);
        --y-sep: rgba(35, 30, 26, 0.08);
        --y-border: rgba(35, 30, 26, 0.13);
        --y-text: #231E1A;
        --y-muted: #786D61;
        --y-accent: #F5CE75;
        --y-accent-h: #EBB852;
        --y-accent-soft: rgba(245, 206, 117, 0.28);
        --y-on-accent: #231E1A;
        --y-green: #4F7F3B;
        --y-green-soft: rgba(79, 127, 59, 0.15);
        --y-warn: #A86416;
        --y-warn-soft: rgba(245, 179, 66, 0.22);
        --y-red: #B83A32;
        --y-red-soft: rgba(184, 58, 50, 0.14);
        --y-switch-off: rgba(35, 30, 26, 0.20);
        --y-kbd: rgba(35, 30, 26, 0.07);
        --y-input: rgba(252, 248, 240, 0.96);
        --y-shadow: 0 18px 48px rgba(74, 58, 35, 0.16);
      }
      .smt-mini-cat {
        display: inline-flex; align-items: center; justify-content: center; flex-shrink: 0;
      }
      .smt-mini-cat svg {
        width: 18px; height: 16px; display: block; image-rendering: pixelated;
      }
      #smt-pill {
        display: inline-flex;
        align-items: center;
        gap: 7px;
        background: var(--y-bg);
        backdrop-filter: blur(20px) saturate(180%);
        -webkit-backdrop-filter: blur(20px) saturate(180%);
        color: var(--y-text);
        border: 1px solid var(--y-border);
        border-radius: 999px;
        padding: 7px 13px 7px 10px;
        font-family: inherit;
        font-size: 13px;
        font-weight: 600;
        cursor: pointer;
        box-shadow: 0 8px 24px rgba(0,0,0,0.28);
        transition: border-color 0.15s, transform 0.12s;
      }
      #smt-pill:hover { border-color: var(--y-accent); transform: translateY(-1px); }
      .smt-dot {
        width: 7px; height: 7px; border-radius: 50%;
        background: var(--y-warn); display: inline-block; flex-shrink: 0;
      }
      .smt-dot.ok { background: var(--y-green); }
      .smt-dot.err { background: var(--y-red); }
      .smt-dot.busy { background: var(--y-accent); animation: smtPulse 1s infinite; }
      @keyframes smtPulse { 0%,100% { opacity: 1; } 50% { opacity: 0.35; } }

      #smt-panel {
        width: 296px;
        max-height: 82vh;
        overflow-y: auto;
        background: var(--y-bg);
        backdrop-filter: blur(20px) saturate(180%);
        -webkit-backdrop-filter: blur(20px) saturate(180%);
        border: 1px solid var(--y-border);
        border-radius: 16px;
        padding: 13px;
        box-shadow: var(--y-shadow);
        display: none;
        margin-bottom: 10px;
        color: var(--y-text);
        text-align: left;
      }
      #smt-panel.open { display: flex; flex-direction: column; gap: 9px; }
      .smt-head {
        display: flex; align-items: center; gap: 7px;
      }
      .smt-title { font-size: 14px; font-weight: 700; letter-spacing: -0.01em; }
      .smt-conn-pill {
        font-size: 10px; font-weight: 700; padding: 2px 7px; border-radius: 999px;
        margin-right: auto; white-space: nowrap;
      }
      .smt-conn-pill.ok { background: var(--y-green-soft); color: var(--y-green); }
      .smt-conn-pill.warn { background: var(--y-warn-soft); color: var(--y-warn); }
      .smt-conn-pill.err { background: var(--y-red-soft); color: var(--y-red); }
      .smt-conn-pill.busy { background: var(--y-accent-soft); color: var(--y-accent); }
      .smt-theme-seg {
        display: inline-flex; background: var(--y-kbd);
        border: 1px solid var(--y-sep); border-radius: 999px; padding: 2px; gap: 1px;
      }
      .smt-theme-seg button {
        border: none; background: transparent; color: var(--y-muted);
        font-family: inherit; font-size: 10px; font-weight: 600;
        padding: 2px 6px; border-radius: 999px; cursor: pointer;
      }
      .smt-theme-seg button.active { background: var(--y-surface-hi); color: var(--y-text); box-shadow: 0 1px 2px rgba(0,0,0,0.16); }
      .smt-close {
        width: 22px; height: 22px; border-radius: 999px;
        background: var(--y-kbd); border: none; color: var(--y-muted);
        font-size: 14px; line-height: 1; cursor: pointer; padding: 0;
        display: inline-flex; align-items: center; justify-content: center;
      }
      .smt-close:hover { color: var(--y-text); background: var(--y-surface-hi); }

      .smt-alert {
        display: none; padding: 7px 10px; border-radius: 10px;
        font-size: 11.5px; font-weight: 500;
        background: var(--y-red-soft); color: var(--y-red);
        border: 1px solid rgba(255, 69, 58, 0.25);
      }
      .smt-alert.warn {
        background: var(--y-warn-soft); color: var(--y-warn);
        border-color: rgba(255, 159, 10, 0.25);
      }
      .smt-alert.show { display: block; }

      .smt-btn-primary {
        width: 100%; border: none; border-radius: 10px;
        background: var(--y-accent); color: var(--y-on-accent);
        padding: 9px 12px; font-family: inherit; font-size: 13px; font-weight: 650;
        cursor: pointer; transition: background 0.14s, opacity 0.14s;
      }
      .smt-btn-primary:hover:not(:disabled) { background: var(--y-accent-h); }
      .smt-btn-primary.danger { background: var(--y-red); color: #ffffff; }
      .smt-btn-primary:disabled { opacity: 0.45; cursor: not-allowed; }
      .smt-progress {
        width: 100%; height: 4px; border-radius: 999px;
        background: var(--y-kbd); overflow: hidden; display: none; margin-top: -3px;
      }
      .smt-progress.show { display: block; }
      .smt-progress-bar {
        height: 100%; width: 0%; background: var(--y-accent);
        transition: width 0.25s ease;
      }
      .smt-sec-row { display: flex; gap: 6px; align-items: center; }
      .smt-btn-sec {
        flex: 1; min-width: 0; border-radius: 9px;
        border: 1px solid var(--y-border); background: var(--y-surface); color: var(--y-text);
        padding: 7px 10px; font-family: inherit; font-size: 12px; font-weight: 600;
        cursor: pointer; white-space: nowrap;
      }
      .smt-btn-sec:hover:not(:disabled) { background: var(--y-surface-hi); }
      .smt-btn-sec:disabled { opacity: 0.45; cursor: not-allowed; }
      .smt-btn-icon {
        width: 32px; height: 32px; flex-shrink: 0; border-radius: 9px;
        border: 1px solid var(--y-border); background: var(--y-surface); color: var(--y-text);
        font-size: 14px; font-weight: 600; cursor: pointer; padding: 0;
        display: inline-flex; align-items: center; justify-content: center;
      }
      .smt-btn-icon:hover:not(:disabled) { background: var(--y-surface-hi); }
      .smt-btn-icon:disabled { opacity: 0.45; cursor: not-allowed; }

      .smt-group {
        background: var(--y-surface); border: 1px solid var(--y-sep);
        border-radius: 12px; overflow: hidden;
      }
      .smt-group-row {
        display: flex; align-items: center; justify-content: space-between; gap: 10px;
        padding: 8px 11px; position: relative; font-size: 12px; font-weight: 500;
      }
      .smt-group-row + .smt-group-row::before {
        content: ""; position: absolute; top: 0; left: 11px; right: 11px;
        height: 1px; background: var(--y-sep);
      }
      .smt-switch {
        position: relative; width: 36px; height: 20px; border-radius: 999px;
        border: none; background: var(--y-switch-off); cursor: pointer; padding: 0;
        flex-shrink: 0; transition: background 0.18s ease;
      }
      .smt-switch::after {
        content: ""; position: absolute; top: 2px; left: 2px;
        width: 16px; height: 16px; border-radius: 50%; background: #ffffff;
        box-shadow: 0 1px 3px rgba(0,0,0,0.28); transition: transform 0.18s ease;
      }
      .smt-switch.on { background: var(--y-accent); }
      .smt-switch.on::after { transform: translateX(16px); }
      .smt-seg {
        display: inline-flex; padding: 2px; border-radius: 8px;
        background: var(--y-kbd); border: 1px solid var(--y-sep); gap: 1px;
      }
      .smt-seg button {
        border: none; background: transparent; color: var(--y-muted);
        font-family: inherit; font-size: 11px; font-weight: 600;
        padding: 3px 8px; border-radius: 6px; cursor: pointer;
      }
      .smt-seg button.on { background: var(--y-surface-hi); color: var(--y-text); box-shadow: 0 1px 2px rgba(0,0,0,0.16); }
      .smt-seg button:disabled { opacity: 0.4; cursor: not-allowed; }

      .smt-drawer-wrap {
        background: var(--y-surface); border: 1px solid var(--y-sep);
        border-radius: 12px; overflow: hidden;
      }
      .smt-drawer-toggle {
        width: 100%; border: none; background: transparent; color: var(--y-text);
        padding: 8px 11px; font-family: inherit; font-size: 12px; font-weight: 600;
        cursor: pointer; display: flex; align-items: center; justify-content: space-between; gap: 8px;
      }
      .smt-drawer-toggle:hover { background: var(--y-kbd); }
      .smt-drawer-sum { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
      .smt-drawer-chev {
        font-size: 11px; color: var(--y-muted); flex-shrink: 0;
        transition: transform 0.16s ease;
      }
      .smt-drawer-toggle.open .smt-drawer-chev { transform: rotate(180deg); }
      .smt-drawer-body {
        display: none; padding: 9px 11px 11px; border-top: 1px solid var(--y-sep);
        flex-direction: column; gap: 8px;
      }
      .smt-drawer-body.open { display: flex; }

      .smt-row { margin: 0; }
      .smt-label {
        display: block; font-size: 10.5px; font-weight: 600; color: var(--y-muted);
        margin-bottom: 4px;
      }
      .smt-select {
        width: 100%; background: var(--y-input); color: var(--y-text);
        border: 1px solid var(--y-border); border-radius: 8px;
        padding: 6px 8px; font-family: inherit; font-size: 12px; outline: none;
      }
      .smt-select:focus { border-color: var(--y-accent); }
      .smt-inline-row { display: flex; gap: 6px; align-items: center; }
      .smt-inline-row .smt-select { flex: 1; min-width: 0; }
      .smt-icon-btn {
        flex: 0 0 auto; padding: 6px 9px; border-radius: 8px; font-size: 12px;
        cursor: pointer; background: var(--y-surface); color: var(--y-text);
        border: 1px solid var(--y-border);
      }
      #smt-glossary-editor {
        display: none; padding-top: 8px; border-top: 1px solid var(--y-sep);
        flex-direction: column; gap: 7px;
      }
      #smt-glossary-editor.open { display: flex; }
      .smt-input, .smt-textarea {
        width: 100%; background: var(--y-input); color: var(--y-text);
        border: 1px solid var(--y-border); border-radius: 7px;
        padding: 5px 8px; font-size: 11px; font-family: inherit; outline: none;
      }
      .smt-textarea { resize: vertical; min-height: 48px; line-height: 1.3; }
      .smt-btns { display: flex; gap: 6px; }
      .smt-btn {
        flex: 1; padding: 6px 9px; border-radius: 8px; font-size: 11.5px;
        font-weight: 600; cursor: pointer; border: 1px solid var(--y-border);
        background: var(--y-surface); color: var(--y-text); font-family: inherit;
      }
      .smt-btn.primary { background: var(--y-accent); border-color: var(--y-accent); color: var(--y-on-accent); }
      .smt-btn.danger { background: var(--y-red); border-color: var(--y-red); color: #fff; }

      .smt-status {
        font-size: 11px; color: var(--y-muted); line-height: 1.35; min-height: 14px;
      }
      .smt-hints-row {
        display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
        font-size: 10.5px; color: var(--y-muted);
      }
      .smt-hint-chip { display: inline-flex; align-items: center; gap: 4px; }
      .smt-kbd {
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 10px; font-weight: 700; color: var(--y-text);
        background: var(--y-kbd); border: 1px solid var(--y-sep);
        padding: 1px 5px; border-radius: 5px;
      }
    `;
    document.head.appendChild(style);
  }

  // --- Détection des planches manga (optimisée sans reflow coûteux) ---
  function isMangaCandidate(el) {
    if (!el || !el.isConnected) return false;
    const tag = el.tagName;
    if (tag !== "IMG" && tag !== "CANVAS") return false;
    if (tag === "IMG" && (!el.complete || el.naturalWidth < 250 || el.naturalHeight < 250)) {
      return false;
    }
    // offsetWidth/offsetHeight évitent getComputedStyle() sur chaque image
    if (el.offsetWidth < 260 || el.offsetHeight < 300) return false;
    return true;
  }

  function findMangaElements() {
    const nodes = document.querySelectorAll("img, canvas");
    const list = [];
    for (const el of nodes) {
      if (isMangaCandidate(el)) list.push(el);
    }
    if (list.length > 1) {
      list.sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top);
    }
    return list;
  }

  function isInViewport(el) {
    const r = el.getBoundingClientRect();
    const vh = window.innerHeight || document.documentElement.clientHeight;
    const vw = window.innerWidth || document.documentElement.clientWidth;
    return r.bottom > 40 && r.top < vh - 40 && r.right > 0 && r.left < vw;
  }

  function getElementSignature(el) {
    const visTag = modelSupportsVision(prefs.model) ? prefs.visionMode : "off";
    if (el.tagName === "IMG") {
      return `img:${el.currentSrc || el.src}:${el.naturalWidth}x${el.naturalHeight}:${prefs.model}:${prefs.sourceLang}:${visTag}`;
    }
    return `canvas:${el.width}x${el.height}:${prefs.model}:${prefs.sourceLang}:${visTag}`;
  }

  function isAlreadyTranslated(el) {
    const entry = state.overlays.get(el);
    if (!entry || entry.translating) return false;
    return entry.signature === getElementSignature(el);
  }

  function getContextHistoryBefore(targetEl, maxLines = 8) {
    const pages = findMangaElements();
    const lines = [];
    for (const el of pages) {
      if (el === targetEl) break;
      const entry = state.overlays.get(el);
      if (!entry || !entry.bubbles || entry.bubbles.length === 0) continue;
      for (const b of entry.bubbles) {
        const fr = (b.fr || "").trim();
        if (fr) lines.push(fr);
      }
    }
    return lines.slice(-maxLines);
  }

  // --- Extraction de l'image (évite responseType:"blob" buggé sur Safari Userscripts) ---
  function arrayBufferToDataURL(buffer, mime = "image/jpeg") {
    const bytes = new Uint8Array(buffer);
    let binary = "";
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk));
    }
    return `data:${mime};base64,${btoa(binary)}`;
  }

  async function fetchImageViaExtension(src) {
    const res = await gmRequest({
      method: "GET",
      url: src,
      responseType: "arraybuffer",
      headers: { Referer: location.href },
      timeout: 12000,
    });
    if (res.status >= 200 && res.status < 300 && res.response) {
      return { image_base64: arrayBufferToDataURL(res.response) };
    }
    throw new Error(`Téléchargement image échoué (${res.status})`);
  }

  function canvasToCompactDataURL(sourceEl, srcW, srcH, maxDim = 1500) {
    const scale = Math.min(1, maxDim / Math.max(srcW, srcH));
    const w = Math.max(1, Math.round(srcW * scale));
    const h = Math.max(1, Math.round(srcH * scale));
    const c = document.createElement("canvas");
    c.width = w;
    c.height = h;
    const ctx = c.getContext("2d");
    ctx.drawImage(sourceEl, 0, 0, w, h);
    return c.toDataURL("image/jpeg", 0.82);
  }

  async function extractImagePayload(el) {
    if (el.tagName === "CANVAS") {
      try {
        return { image_base64: canvasToCompactDataURL(el, el.width, el.height) };
      } catch {
        throw new Error("Canvas protégé (lecture directe bloquée par le navigateur).");
      }
    }

    const src = el.currentSrc || el.src;
    if (!src) throw new Error("Source image introuvable.");

    // 1. Préfère envoyer l'URL légère (~150 octets) au serveur Python :
    // évite la limite de taille IPC de Safari WebExtension lors des lots de plusieurs pages !
    if (src.startsWith("http://") || src.startsWith("https://")) {
      return { image_url: src, referer: location.href };
    }

    if (src.startsWith("data:image/")) {
      return { image_base64: src };
    }

    // 2. Fallback pour blob: ou autres sources locales
    try {
      return { image_base64: canvasToCompactDataURL(el, el.naturalWidth, el.naturalHeight) };
    } catch {
      throw new Error("Impossible d'extraire cette image.");
    }
  }

  // --- Gestion des calques de traduction ---
  function getOverlayRoot() {
    let root = document.getElementById("smt-overlay-root");
    if (!root) {
      root = document.createElement("div");
      root.id = "smt-overlay-root";
      document.body.appendChild(root);
    }
    return root;
  }

  function fitBubbleFont(div, text) {
    const w = div.clientWidth || 60;
    const h = div.clientHeight || 30;
    const chars = Math.max(text.length, 4);
    let size = Math.max(9.5, Math.min(24, Math.sqrt((w * h) / chars) * 1.16));
    div.style.fontSize = size.toFixed(1) + "px";

    let guard = 0;
    while (
      guard < 22 &&
      size > 7.5 &&
      (div.scrollHeight > div.clientHeight + 2 || div.scrollWidth > div.clientWidth + 2)
    ) {
      size -= 0.7;
      div.style.fontSize = size.toFixed(1) + "px";
      guard++;
    }
  }

  function syncOverlayPositions() {
    state.syncScheduled = false;
    for (const [el, entry] of state.overlays.entries()) {
      if (!el.isConnected) {
        entry.layer.remove();
        state.overlays.delete(el);
        continue;
      }
      const r = el.getBoundingClientRect();
      if (r.width < 50 || r.height < 50) {
        entry.layer.style.display = "none";
        continue;
      }
      entry.layer.style.display = "block";
      entry.layer.style.left = r.left + window.scrollX + "px";
      entry.layer.style.top = r.top + window.scrollY + "px";
      entry.layer.style.width = r.width + "px";
      entry.layer.style.height = r.height + "px";
      entry.layer.classList.toggle("smt-hidden", !prefs.visible);

      if (Math.abs((entry.lastWidth || 0) - r.width) > 15) {
        entry.lastWidth = r.width;
        for (const div of entry.layer.querySelectorAll(".smt-bubble")) {
          fitBubbleFont(div, div.textContent || "");
        }
      }
    }
  }

  function scheduleSync() {
    if (state.syncScheduled) return;
    state.syncScheduled = true;
    requestAnimationFrame(syncOverlayPositions);
  }

  function layoutBubblesNonOverlapping(bubbles) {
    const boxes = bubbles.map((b) => ({
      b,
      x: b.x,
      y: b.y,
      w: Math.max(b.w, 0.025),
      h: Math.max(b.h, 0.024),
    }));

    const gap = 0.003;
    const minSize = 0.02;

    // Élargit prudemment les bulles très étroites uniquement s'il y a de la place libre
    for (let i = 0; i < boxes.length; i++) {
      const cur = boxes[i];
      if (cur.w < 0.068) {
        const targetW = 0.068;
        const cx = cur.x + cur.w / 2;
        let nx = Math.max(0, Math.min(1 - targetW, cx - targetW / 2));
        let nx2 = nx + targetW;
        for (let j = 0; j < boxes.length; j++) {
          if (i === j) continue;
          const o = boxes[j];
          if (Math.min(cur.y + cur.h, o.y + o.h) <= Math.max(cur.y, o.y)) continue;
          if (o.x >= cur.x + cur.w - 0.003) nx2 = Math.min(nx2, o.x - gap);
          if (o.x + o.w <= cur.x + 0.003) nx = Math.max(nx, o.x + o.w + gap);
        }
        nx = Math.min(nx, cur.x);
        nx2 = Math.max(nx2, cur.x + cur.w);
        cur.x = nx;
        cur.w = Math.max(cur.w, nx2 - nx);
      }
      if (cur.h < 0.030) {
        const targetH = 0.030;
        const cy = cur.y + cur.h / 2;
        let ny = Math.max(0, Math.min(1 - targetH, cy - targetH / 2));
        let ny2 = ny + targetH;
        for (let j = 0; j < boxes.length; j++) {
          if (i === j) continue;
          const o = boxes[j];
          if (Math.min(cur.x + cur.w, o.x + o.w) <= Math.max(cur.x, o.x)) continue;
          if (o.y >= cur.y + cur.h - 0.003) ny2 = Math.min(ny2, o.y - gap);
          if (o.y + o.h <= cur.y + 0.003) ny = Math.max(ny, o.y + o.h + gap);
        }
        ny = Math.min(ny, cur.y);
        ny2 = Math.max(ny2, cur.y + cur.h);
        cur.y = ny;
        cur.h = Math.max(cur.h, ny2 - ny);
      }
    }

    // Résolveur de collisions 2D (2 passes) : garantit zéro chevauchement à l'écran
    for (let pass = 0; pass < 2; pass++) {
      for (let i = 0; i < boxes.length; i++) {
        const a = boxes[i];
        for (let j = i + 1; j < boxes.length; j++) {
          const b = boxes[j];
          const ax2 = a.x + a.w, ay2 = a.y + a.h;
          const bx2 = b.x + b.w, by2 = b.y + b.h;
          if (!(a.x < bx2 && b.x < ax2 && a.y < by2 && b.y < ay2)) continue;

          const ovX = Math.min(ax2, bx2) - Math.max(a.x, b.x);
          const ovY = Math.min(ay2, by2) - Math.max(a.y, b.y);
          const normX = ovX / Math.max(Math.min(a.w, b.w), 1e-5);
          const normY = ovY / Math.max(Math.min(a.h, b.h), 1e-5);

          if (normX <= normY) {
            const [left, right] = (a.x + ax2 <= b.x + bx2) ? [a, b] : [b, a];
            const lx2 = left.x + left.w, rx2 = right.x + right.w;
            const mid = (lx2 + right.x) * 0.5;
            const newLW = Math.max(minSize, mid - gap * 0.5 - left.x);
            if (left.x + newLW + gap * 0.5 > rx2 - minSize) {
              const split = Math.max(left.x + minSize * 0.5, Math.min(rx2 - minSize * 0.5, mid));
              left.w = Math.max(0.01, split - left.x);
              right.x = split;
              right.w = Math.max(0.01, rx2 - split);
            } else {
              left.w = newLW;
              right.x = left.x + newLW + gap;
              right.w = Math.max(minSize, rx2 - right.x);
            }
          } else {
            const [top, bot] = (a.y + ay2 <= b.y + by2) ? [a, b] : [b, a];
            const ty2 = top.y + top.h, by2c = bot.y + bot.h;
            const mid = (ty2 + bot.y) * 0.5;
            const newTH = Math.max(minSize, mid - gap * 0.5 - top.y);
            if (top.y + newTH + gap * 0.5 > by2c - minSize) {
              const split = Math.max(top.y + minSize * 0.5, Math.min(by2c - minSize * 0.5, mid));
              top.h = Math.max(0.01, split - top.y);
              bot.y = split;
              bot.h = Math.max(0.01, by2c - split);
            } else {
              top.h = newTH;
              bot.y = top.y + newTH + gap;
              bot.h = Math.max(minSize, by2c - bot.y);
            }
          }
        }
      }
    }

    return boxes;
  }

  function renderBubbles(el, bubbles) {
    let entry = state.overlays.get(el);
    if (!entry) {
      const layer = document.createElement("div");
      layer.className = "smt-page-layer";
      getOverlayRoot().appendChild(layer);
      entry = { layer, bubbles: [], signature: "", translating: false, lastWidth: 0 };
      state.overlays.set(el, entry);
    }

    entry.layer.innerHTML = "";
    entry.bubbles = bubbles;
    entry.lastWidth = 0;

    syncOverlayPositions();

    const placedBoxes = layoutBubblesNonOverlapping(bubbles);
    for (const box of placedBoxes) {
      const b = box.b;
      const styleName = b.style || "bubble_light";
      const div = document.createElement("div");
      div.className = `smt-bubble smt-style-${styleName}`;

      div.style.left = box.x * 100 + "%";
      div.style.top = box.y * 100 + "%";
      div.style.width = box.w * 100 + "%";
      div.style.height = box.h * 100 + "%";
      div.textContent = b.fr || b.text;
      const whoPrefix = b.who ? `${b.who} · ` : "";
      div.title = `${whoPrefix}Original : ${b.text}\n(Cliquer pour voir l'original)`;
      div.addEventListener("click", (e) => {
        e.stopPropagation();
        div.classList.toggle("smt-peek");
      });
      entry.layer.appendChild(div);
      fitBubbleFont(div, div.textContent);
    }
  }

  function showLoadingOnElement(el, text) {
    let entry = state.overlays.get(el);
    if (!entry) {
      const layer = document.createElement("div");
      layer.className = "smt-page-layer";
      getOverlayRoot().appendChild(layer);
      entry = { layer, bubbles: [], signature: "", translating: false, lastWidth: 0 };
      state.overlays.set(el, entry);
    }
    syncOverlayPositions();
    let badge = entry.layer.querySelector(".smt-loading-badge");
    if (!badge) {
      badge = document.createElement("div");
      badge.className = "smt-loading-badge";
      entry.layer.appendChild(badge);
    }
    badge.textContent = text;
  }

  function clearLoadingOnElement(el) {
    const entry = state.overlays.get(el);
    if (!entry) return;
    const badge = entry.layer.querySelector(".smt-loading-badge");
    if (badge) badge.remove();
  }

  async function translateElement(el, force = false) {
    const sig = getElementSignature(el);
    const existing = state.overlays.get(el);
    if (existing?.translating) return null;
    if (!force && existing && existing.signature === sig) {
      return { skipped: true, count: existing.bubbles.length };
    }

    if (!existing) {
      const layer = document.createElement("div");
      layer.className = "smt-page-layer";
      getOverlayRoot().appendChild(layer);
      state.overlays.set(el, { layer, bubbles: [], signature: "", translating: true, lastWidth: 0 });
    } else {
      existing.translating = true;
    }

    const loadingText = isVisionEffective() ? "Vision FR..." : "Traduction FR...";
    showLoadingOnElement(el, loadingText);

    try {
      let imgPayload = await extractImagePayload(el);
      const contextHistory = getContextHistoryBefore(el, 8);
      let data;
      // Réessai automatique en cas de limite de débit (429) ou d'échec de téléchargement distant
      for (let attempt = 0; attempt < 3; attempt++) {
        try {
          data = await apiJson(
            "/api/translate",
            {
              ...imgPayload,
              model: prefs.model,
              source_lang: prefs.sourceLang,
              target_lang: prefs.targetLang || "fr",
              vision: prefs.visionMode,
              glossary_slug: prefs.glossarySlug || "auto",
              page_url: location.href,
              page_title: document.title,
              context_history: contextHistory,
              skip_cache: force,
            },
            185000
          );
          break;
        } catch (err) {
          const isFetchFail = /IMAGE_FETCH_FAILED/i.test(err.message || "") && imgPayload.image_url;
          if (isFetchFail && attempt === 0) {
            // Fallback 1 : canvas compact si même origine, sinon arraybuffer via l'extension
            try {
              imgPayload = { image_base64: canvasToCompactDataURL(el, el.naturalWidth, el.naturalHeight) };
            } catch {
              imgPayload = await fetchImageViaExtension(imgPayload.image_url);
            }
            continue;
          }
          const isRateLimit = err.status === 429 || /429|quota/i.test(err.message || "");
          if (isRateLimit && attempt < 2 && !state.preloadAbort) {
            showLoadingOnElement(el, "Pause quota (4s)...");
            await sleep(4000 * (attempt + 1));
            showLoadingOnElement(el, loadingText);
            continue;
          }
          throw err;
        }
      }

      const entry = state.overlays.get(el);
      if (entry) {
        entry.signature = sig;
        entry.translating = false;
      }
      renderBubbles(el, data.bubbles || []);
      return {
        count: (data.bubbles || []).length,
        cached: Boolean(data.cached),
        ms: (data.ocr_ms || 0) + (data.translate_ms || 0),
      };
    } catch (err) {
      const entry = state.overlays.get(el);
      if (entry) entry.translating = false;
      clearLoadingOnElement(el);
      throw err;
    }
  }

  // --- Préchargement intelligent de tout le chapitre (Mode Défilement) ---
  function getChapterStats(precomputedPages = null) {
    const pages = precomputedPages || findMangaElements();
    let done = 0;
    for (const p of pages) {
      if (isAlreadyTranslated(p)) done++;
    }
    return { total: pages.length, done, pages };
  }

  function pickNextChapterBatch(maxCount = getBatchSize()) {
    const pages = findMangaElements();
    const pending = pages.filter((el) => {
      const entry = state.overlays.get(el);
      if (entry?.translating) return false;
      return !isAlreadyTranslated(el);
    });
    if (pending.length === 0) return [];

    // Priorité 1 : planches visibles ou juste en dessous, puis celles au-dessus
    const belowOrVisible = [];
    const above = [];
    for (const el of pending) {
      const r = el.getBoundingClientRect();
      if (r.bottom > 40) belowOrVisible.push({ el, top: r.top });
      else above.push({ el, top: r.top });
    }
    belowOrVisible.sort((a, b) => a.top - b.top);
    above.sort((a, b) => a.top - b.top);
    return [...belowOrVisible, ...above].slice(0, maxCount).map((x) => x.el);
  }

  function updateProgressUI(precomputedPages = null) {
    const { total, done } = getChapterStats(precomputedPages);
    const ready = state.serverOnline && state.apiKeyOk;
    if (uiPillLabel) {
      let nextLabel = "Yomi";
      if (total > 0 && (state.preloadRunning || done > 0)) {
        const suffix = done === total ? `${done}/${total} ✓` : `${done}/${total}`;
        nextLabel = `Yomi · ${suffix}`;
      }
      if (uiPillLabel.textContent !== nextLabel) uiPillLabel.textContent = nextLabel;
    }
    if (uiDotEl) {
      let nextDot = "smt-dot err";
      if (state.preloadRunning) {
        nextDot = "smt-dot busy";
      } else if (ready) {
        nextDot = "smt-dot ok";
      } else if (state.serverOnline) {
        nextDot = "smt-dot";
      }
      if (uiDotEl.className !== nextDot) uiDotEl.className = nextDot;
    }
    if (uiConnPill) {
      let cTxt = "Hors ligne";
      let cCls = "smt-conn-pill err";
      if (state.preloadRunning) {
        cTxt = total > 0 ? `${done}/${total}` : "En cours";
        cCls = "smt-conn-pill busy";
      } else if (ready) {
        cTxt = "Prêt";
        cCls = "smt-conn-pill ok";
      } else if (state.serverOnline) {
        cTxt = "Config";
        cCls = "smt-conn-pill warn";
      }
      if (uiConnPill.textContent !== cTxt) uiConnPill.textContent = cTxt;
      if (uiConnPill.className !== cCls) uiConnPill.className = cCls;
    }
    if (uiProgressWrap && uiProgressBar) {
      const show = total > 0 && (state.preloadRunning || (done > 0 && done < total));
      uiProgressWrap.classList.toggle("show", show);
      const nextWidth = total > 0 ? Math.round((done / total) * 100) + "%" : "0%";
      if (uiProgressBar.style.width !== nextWidth) uiProgressBar.style.width = nextWidth;
    }
    if (uiChapterBtn) {
      const nextTxt = state.preloadRunning
        ? `Arrêter (${done}/${total})`
        : total > 0
          ? `Traduire le chapitre (${total}p)`
          : "Traduire le chapitre";
      const nextCls = state.preloadRunning ? "smt-btn-primary danger" : "smt-btn-primary";
      if (uiChapterBtn.textContent !== nextTxt) uiChapterBtn.textContent = nextTxt;
      if (uiChapterBtn.className !== nextCls) uiChapterBtn.className = nextCls;
      uiChapterBtn.disabled = !state.preloadRunning && !ready;
    }
    if (uiTranslateBtn) uiTranslateBtn.disabled = !ready;
    if (uiRetryBtn) uiRetryBtn.disabled = !ready;
  }

  function stopChapterPreload() {
    state.preloadAbort = true;
    state.preloadRunning = false;
    updateProgressUI();
    setStatus("Préchargement mis en pause.");
  }

  async function translateBatch(elements, prefetchNext = null) {
    const sigs = new Map();
    const pagePayloads = [];

    for (let i = 0; i < elements.length; i++) {
      const el = elements[i];
      sigs.set(i, getElementSignature(el));
      let entry = state.overlays.get(el);
      if (!entry) {
        const layer = document.createElement("div");
        layer.className = "smt-page-layer";
        getOverlayRoot().appendChild(layer);
        entry = { layer, bubbles: [], signature: "", translating: true, lastWidth: 0 };
        state.overlays.set(el, entry);
      } else {
        entry.translating = true;
      }
      const badgeLabel = isVisionEffective()
        ? (elements.length > 1 ? `Vision FR (${elements.length}p)...` : "Vision FR...")
        : `Lot FR (${elements.length}p)...`;
      showLoadingOnElement(el, badgeLabel);
      try {
        const p = await extractImagePayload(el);
        pagePayloads.push({ index: i, ...p });
      } catch (err) {
        entry.translating = false;
        clearLoadingOnElement(el);
      }
    }

    if (pagePayloads.length === 0) return { allCached: true };

    try {
      const contextHistory = getContextHistoryBefore(elements[0], 8);
      const data = await apiJson(
        "/api/translate-batch",
        {
          pages: pagePayloads,
          model: prefs.model,
          source_lang: prefs.sourceLang,
          target_lang: prefs.targetLang || "fr",
          vision: prefs.visionMode,
          glossary_slug: prefs.glossarySlug || "auto",
          page_url: location.href,
          page_title: document.title,
          context_history: contextHistory,
          prefetch_next: prefetchNext,
          skip_cache: false,
        },
        185000
      );

      let allCached = true;
      for (const item of data.results || []) {
        const el = elements[item.index];
        if (!el) continue;
        const entry = state.overlays.get(el);
        if (entry) entry.translating = false;
        if (item.error) {
          clearLoadingOnElement(el);
          // Fallback unitaire si le téléchargement serveur a échoué pour cette image
          if (/IMAGE_FETCH_FAILED/i.test(item.error)) {
            await translateElement(el, false).catch(() => {});
          }
          continue;
        }
        if (!item.cached) allCached = false;
        if (entry) entry.signature = sigs.get(item.index) || getElementSignature(el);
        renderBubbles(el, item.bubbles || []);
      }
      return { allCached };
    } catch (err) {
      for (const el of elements) {
        const entry = state.overlays.get(el);
        if (entry) entry.translating = false;
        clearLoadingOnElement(el);
      }
      throw err;
    }
  }

  async function startChapterPreload() {
    if (state.preloadRunning) {
      stopChapterPreload();
      return;
    }

    const { total } = getChapterStats();
    if (total === 0) {
      setStatus("Aucune planche détectée à précharger.");
      return;
    }

    const batchSize = getBatchSize();
    state.preloadRunning = true;
    state.preloadAbort = false;
    state.quotaBlockedModel = null;
    updateProgressUI();
    setStatus(`Préchargement par lots de ${batchSize} pages (${total}p)...`);

    let lastError = null;
    let consecutive429 = 0;

    while (!state.preloadAbort) {
      const picked = pickNextChapterBatch(batchSize + 1);
      if (picked.length === 0) break;

      const batch = picked.slice(0, batchSize);
      const nextEl = picked[batchSize];
      let prefetchNext = null;
      if (nextEl && nextEl.tagName === "IMG") {
        const nextSrc = nextEl.currentSrc || nextEl.src || "";
        if (nextSrc.startsWith("http://") || nextSrc.startsWith("https://")) {
          prefetchNext = { image_url: nextSrc, referer: location.href };
        }
      }

      try {
        const res = await translateBatch(batch, prefetchNext);
        consecutive429 = 0;
        updateProgressUI();
        const stats = getChapterStats();
        setStatus(`Chapitre : ${stats.done}/${stats.total} pages traduites...`);
        if (
          res &&
          !res.allCached &&
          !prefs.model.startsWith("ollama:") &&
          stats.done < stats.total &&
          !state.preloadAbort
        ) {
          // Pause uniquement sur API cloud (aucune limite en local Ollama !)
          await sleep(2000);
        }
      } catch (err) {
        lastError = err.message;
        const is429 = err.status === 429 || /429|quota/i.test(err.message || "");
        const isDaily = Boolean(err.data?.is_daily);
        const retryAfter = Number(err.data?.retry_after || 20);

        if (isDaily) {
          state.quotaBlockedModel = prefs.model;
          state.preloadAbort = true;
          break;
        }

        if (is429) {
          consecutive429++;
          if (consecutive429 >= 2) {
            // 2 fois 429 d'affilée malgré la pause -> quota atteint sur ce modèle, on arrête proprement
            state.quotaBlockedModel = prefs.model;
            lastError = `Quota atteint sur ${prefs.model}. Passe sur gemini-3.5-flash-lite dans le menu !`;
            state.preloadAbort = true;
            break;
          }
          // Compte à rebours propre sans spammer le serveur
          for (let s = retryAfter; s > 0 && !state.preloadAbort; s--) {
            setStatus(`Limite/min atteinte — reprise dans ${s}s...`);
            await sleep(1000);
          }
          continue;
        }

        // Autre erreur bloquante (serveur arrêté, clé invalide, etc.)
        state.preloadAbort = true;
        break;
      }
    }

    const wasAborted = state.preloadAbort;
    state.preloadRunning = false;
    updateProgressUI();

    const finalStats = getChapterStats();
    if (lastError && wasAborted) {
      setStatus(lastError);
    } else if (wasAborted) {
      setStatus(`En pause (${finalStats.done}/${finalStats.total} pages prêtes).`);
    } else {
      setStatus(`Chapitre prêt ! ${finalStats.done}/${finalStats.total} pages traduites.`);
    }
  }

  async function translateVisiblePages(force = false) {
    const all = findMangaElements();
    const targets = all.filter(isInViewport);
    const list = targets.length > 0 ? targets : all.slice(0, 2);

    if (list.length === 0) {
      setStatus("Aucune planche de manga détectée sur cette page.");
      return;
    }

    setStatus(`Traduction de ${list.length} planche(s) visible(s)...`);
    let totalBubbles = 0;
    let errors = [];

    for (const el of list) {
      try {
        const res = await translateElement(el, force);
        if (res) totalBubbles += res.count || 0;
        updateProgressUI();
      } catch (err) {
        errors.push(err.message);
      }
    }

    if (errors.length > 0) {
      setStatus(`Erreur : ${errors[0]}`);
    } else {
      setStatus(`OK : ${totalBubbles} bulle(s) sur ${list.length} planche(s).`);
    }
  }

  // --- Interface flottante ---
  let uiStatusEl = null;
  let uiDotEl = null;
  let uiConnPill = null;
  let uiAlertEl = null;
  let uiPillLabel = null;
  let uiAutoBtn = null;
  let uiVisionSeg = null;
  let uiToggleBtn = null;
  let uiChapterBtn = null;
  let uiTranslateBtn = null;
  let uiRetryBtn = null;
  let uiProgressWrap = null;
  let uiProgressBar = null;
  let uiSettingsSummary = null;
  let uiRootEl = null;
  let uiPanelEl = null;
  let uiGlossarySel = null;
  let uiGlossaryEditor = null;

  function formatCharactersText(chars) {
    return (chars || [])
      .map((c) => {
        const g = c.gender ? ` (${c.gender})` : "";
        const n = c.notes ? ` : ${c.notes}` : "";
        return `${c.name || ""}${g}${n}`.trim();
      })
      .filter(Boolean)
      .join("\n");
  }

  function parseCharactersText(txt) {
    const out = [];
    for (const rawLine of String(txt || "").split(/\r?\n/)) {
      const line = rawLine.trim();
      if (!line) continue;
      let left = line;
      let notes = "";
      const colonIdx = line.indexOf(":");
      if (colonIdx !== -1) {
        left = line.slice(0, colonIdx).trim();
        notes = line.slice(colonIdx + 1).trim();
      }
      let name = left;
      let gender = "";
      const gMatch = left.match(/^(.*?)\((F|M|NB|f|m|nb)\)\s*$/);
      if (gMatch) {
        name = gMatch[1].trim();
        gender = gMatch[2].toUpperCase();
      }
      if (name) out.push({ name, gender, notes });
    }
    return out;
  }

  function formatTermsText(terms) {
    return Object.entries(terms || {})
      .map(([k, v]) => `${k} = ${v}`)
      .join("\n");
  }

  function parseTermsText(txt) {
    const out = {};
    for (const rawLine of String(txt || "").split(/\r?\n/)) {
      const line = rawLine.trim();
      if (!line) continue;
      const m = line.match(/^(.+?)\s*(?:=|->|:)\s*(.+)$/);
      if (m && m[1].trim() && m[2].trim()) {
        out[m[1].trim()] = m[2].trim();
      }
    }
    return out;
  }

  function renderGlossarySelect() {
    if (!uiGlossarySel) return;
    const prev = prefs.glossarySlug || "auto";
    uiGlossarySel.innerHTML = "";

    const autoOpt = document.createElement("option");
    autoOpt.value = "auto";
    const detGl = state.glossaries.find((g) => g.slug === state.detectedGlossarySlug);
    autoOpt.textContent = detGl ? `Auto (${detGl.title})` : "Auto (aucun détecté)";
    uiGlossarySel.appendChild(autoOpt);

    const noneOpt = document.createElement("option");
    noneOpt.value = "none";
    noneOpt.textContent = "Aucun";
    uiGlossarySel.appendChild(noneOpt);

    for (const g of state.glossaries) {
      const opt = document.createElement("option");
      opt.value = g.slug;
      opt.textContent = g.title || g.slug;
      uiGlossarySel.appendChild(opt);
    }

    const validValues = new Set(["auto", "none", ...state.glossaries.map((g) => g.slug)]);
    if (!validValues.has(prev)) {
      prefs.glossarySlug = "auto";
      savePrefs(prefs);
    }
    uiGlossarySel.value = prefs.glossarySlug || "auto";
  }

  async function loadGlossaries() {
    try {
      const q = `url=${encodeURIComponent(location.href)}&title=${encodeURIComponent(document.title || "")}`;
      const res = await apiJson(`/api/glossaries?${q}`, null, 4000);
      state.glossaries = Array.isArray(res.glossaries) ? res.glossaries : [];
      state.detectedGlossarySlug = res.detected_slug || null;
      renderGlossarySelect();
    } catch {}
  }

  function openGlossaryEditor(slugOverride) {
    if (!uiGlossaryEditor || !uiRootEl) return;
    const chosenSlug =
      slugOverride !== undefined
        ? slugOverride
        : prefs.glossarySlug === "auto"
          ? state.detectedGlossarySlug
          : prefs.glossarySlug;
    const gl = chosenSlug && chosenSlug !== "none"
      ? state.glossaries.find((g) => g.slug === chosenSlug)
      : null;

    state.editorSlug = gl ? gl.slug : null;
    uiRootEl.querySelector("#smt-gl-title").value = gl ? gl.title || "" : "";
    uiRootEl.querySelector("#smt-gl-keywords").value = gl ? (gl.match_keywords || []).join(", ") : "";
    uiRootEl.querySelector("#smt-gl-chars").value = gl ? formatCharactersText(gl.characters) : "";
    uiRootEl.querySelector("#smt-gl-terms").value = gl ? formatTermsText(gl.terms) : "";
    uiRootEl.querySelector("#smt-gl-style").value = gl ? gl.style_notes || "" : "";
    uiRootEl.querySelector("#smt-gl-del").disabled = !state.editorSlug;
    uiGlossaryEditor.classList.add("open");
  }

  async function saveGlossaryFromEditor() {
    if (!uiRootEl) return;
    const title = uiRootEl.querySelector("#smt-gl-title").value.trim();
    if (!title) {
      setStatus("Entre un titre de série.");
      return;
    }
    const keywords = uiRootEl
      .querySelector("#smt-gl-keywords")
      .value.split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const characters = parseCharactersText(uiRootEl.querySelector("#smt-gl-chars").value);
    const terms = parseTermsText(uiRootEl.querySelector("#smt-gl-terms").value);
    const styleNotes = uiRootEl.querySelector("#smt-gl-style").value.trim();

    try {
      const res = await apiJson(
        "/api/glossaries",
        {
          slug: state.editorSlug || undefined,
          title,
          match_keywords: keywords,
          characters,
          terms,
          style_notes: styleNotes,
        },
        8000
      );
      if (res.glossary?.slug) {
        state.editorSlug = res.glossary.slug;
        prefs.glossarySlug = res.glossary.slug;
        savePrefs(prefs);
      }
      await loadGlossaries();
      if (uiGlossaryEditor) uiGlossaryEditor.classList.remove("open");
      setStatus(`Glossaire enregistré : ${res.glossary?.title || title}`);
    } catch (err) {
      setStatus(`Erreur glossaire : ${err.message}`);
    }
  }

  async function deleteCurrentGlossary() {
    if (!state.editorSlug) return;
    try {
      await apiJson("/api/glossaries/delete", { slug: state.editorSlug }, 5000);
      if (prefs.glossarySlug === state.editorSlug) {
        prefs.glossarySlug = "auto";
        savePrefs(prefs);
      }
      state.editorSlug = null;
      await loadGlossaries();
      if (uiGlossaryEditor) uiGlossaryEditor.classList.remove("open");
      setStatus("Glossaire supprimé.");
    } catch (err) {
      setStatus(`Erreur suppression : ${err.message}`);
    }
  }

  function setStatus(msg) {
    if (uiStatusEl) uiStatusEl.textContent = msg;
  }

  function applyUiTheme() {
    if (!uiRootEl) return;
    const mode = prefs.theme || "system";
    const sysLight = window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches;
    const useLight = mode === "light" || (mode === "system" && sysLight);
    uiRootEl.classList.toggle("smt-theme-light", useLight);
    for (const b of uiRootEl.querySelectorAll("[data-smt-theme]")) {
      b.classList.toggle("active", b.getAttribute("data-smt-theme") === mode);
    }
  }

  function syncMiniCatSlots() {
    if (!uiRootEl) return;
    const svg = renderMiniCatSvg(prefs.mascot);
    const pillCat = uiRootEl.querySelector("#smt-pill-cat");
    const headCat = uiRootEl.querySelector("#smt-head-cat");
    if (pillCat) pillCat.innerHTML = svg;
    if (headCat) headCat.innerHTML = svg;
  }

  function updateSettingsSummary() {
    if (!uiSettingsSummary) return;
    const modelSel = uiRootEl?.querySelector("#smt-model");
    const chosenOpt = modelSel?.selectedOptions?.[0]?.textContent;
    const mName = chosenOpt || shortModelLabel(prefs.model);
    const srcTxt = (prefs.sourceLang || "en").toUpperCase();
    const tgtTxt = (prefs.targetLang || "fr").toUpperCase();
    uiSettingsSummary.textContent = `${mName} · ${srcTxt} → ${tgtTxt}`;
  }

  function syncAlertBox() {
    if (!uiAlertEl) return;
    if (!state.serverOnline) {
      uiAlertEl.textContent = "Yomi est arrêté — lancez Yomi.app";
      uiAlertEl.className = "smt-alert show";
    } else if (!state.apiKeyOk) {
      uiAlertEl.textContent = "Configurez Ollama ou une clé API dans Yomi.app";
      uiAlertEl.className = "smt-alert warn show";
    } else {
      uiAlertEl.textContent = "";
      uiAlertEl.className = "smt-alert";
    }
  }

  async function checkServerHealth() {
    try {
      const h = await apiJson("/api/health", null, 4000);
      state.serverOnline = Boolean(h.ok);
      state.apiKeyOk = Boolean(h.api_key_configured);

      const modelSel = uiRootEl?.querySelector("#smt-model");
      if (modelSel && Array.isArray(h.models) && h.models.length > 0) {
        const current = prefs.model;
        modelSel.innerHTML = "";
        for (const m of h.models) {
          if (!m || !m.id) continue;
          const opt = document.createElement("option");
          opt.value = m.id;
          opt.textContent = shortModelLabel(m.id, m.label);
          modelSel.appendChild(opt);
        }
        const ids = new Set(h.models.map((m) => m.id));
        if (ids.has(current)) {
          modelSel.value = current;
        } else if (h.default_model && ids.has(h.default_model)) {
          prefs.model = h.default_model;
          modelSel.value = h.default_model;
          savePrefs(prefs);
        } else {
          prefs.model = h.models[0].id;
          modelSel.value = prefs.model;
          savePrefs(prefs);
        }
      }

      if (!hasSavedTargetLang && h.target_lang) {
        prefs.targetLang = h.target_lang;
        hasSavedTargetLang = true;
        const tgtSel = uiRootEl?.querySelector("#smt-target-lang");
        if (tgtSel) tgtSel.value = prefs.targetLang;
        savePrefs(prefs);
      }

      if (!hasSavedTheme && h.theme) {
        prefs.theme = h.theme;
        hasSavedTheme = true;
        savePrefs(prefs);
        applyUiTheme();
      }

      if (h.mascot && ["mochi", "kuro", "sakura"].includes(h.mascot) && prefs.mascot !== h.mascot) {
        prefs.mascot = h.mascot;
        savePrefs(prefs);
        syncMiniCatSlots();
      }

      updateSettingsSummary();
      syncAlertBox();
      updateProgressUI();
      if (!state.apiKeyOk) {
        setStatus("Lance Ollama ou configure une clé API dans Yomi.app");
      } else if (!state.preloadRunning) {
        const { total, done } = getChapterStats();
        const mShort = shortModelLabel(prefs.model);
        setStatus(total > 0 ? `Prêt · ${done}/${total} pages traduites (${mShort})` : `Prêt (${mShort})`);
      }
    } catch {
      state.serverOnline = false;
      syncAlertBox();
      updateProgressUI();
      setStatus("Serveur arrêté : lance Yomi.app");
    }
  }

  function buildUI() {
    if (document.getElementById("smt-ui")) return;
    injectStyles();

    const ui = document.createElement("div");
    ui.id = "smt-ui";
    const catSvg = renderMiniCatSvg(prefs.mascot);
    ui.innerHTML = `
      <div id="smt-panel">
        <div class="smt-head">
          <span id="smt-head-cat" class="smt-mini-cat">${catSvg}</span>
          <span class="smt-title">Yomi</span>
          <span id="smt-conn-pill" class="smt-conn-pill warn">…</span>
          <div class="smt-theme-seg" title="Thème : Clair / Sombre / Système">
            <button type="button" data-smt-theme="light" title="Clair">☀</button>
            <button type="button" data-smt-theme="dark" title="Sombre">☾</button>
            <button type="button" data-smt-theme="system" title="Système">Auto</button>
          </div>
          <button class="smt-close" id="smt-close" title="Fermer">×</button>
        </div>

        <div id="smt-alert" class="smt-alert"></div>

        <button class="smt-btn-primary" id="smt-chapter-btn" type="button">Traduire le chapitre</button>

        <div class="smt-progress" id="smt-progress">
          <div class="smt-progress-bar" id="smt-progress-bar"></div>
        </div>

        <div class="smt-sec-row">
          <button class="smt-btn-sec" id="smt-translate-btn" type="button">Page visible</button>
          <button class="smt-btn-sec" id="smt-toggle-btn" type="button">Masquer VF</button>
          <button class="smt-btn-icon" id="smt-retry-btn" type="button" title="Retraduire la page">↻</button>
        </div>

        <div class="smt-group">
          <div class="smt-group-row">
            <span>Traduction auto</span>
            <button type="button" id="smt-auto-btn" class="smt-switch" role="switch" aria-checked="false"></button>
          </div>
          <div class="smt-group-row">
            <span>Vision</span>
            <div id="smt-vision-seg" class="smt-seg">
              <button type="button" data-smt-vis="auto">Auto</button>
              <button type="button" data-smt-vis="on">ON</button>
              <button type="button" data-smt-vis="off">OFF</button>
            </div>
          </div>
        </div>

        <div class="smt-drawer-wrap">
          <button type="button" id="smt-settings-toggle" class="smt-drawer-toggle">
            <span id="smt-settings-summary" class="smt-drawer-sum">Gemma 4 12B · EN → FR</span>
            <span class="smt-drawer-chev">▾</span>
          </button>
          <div id="smt-settings-drawer" class="smt-drawer-body">
            <div class="smt-row">
              <label class="smt-label" for="smt-model">Moteur / Modèle</label>
              <select class="smt-select" id="smt-model">
                <option value="ollama:gemma4:12b">Gemma 4 12B</option>
                <option value="gemini-3.8-flash">Gemini 3.8 Flash</option>
                <option value="gemini-3.5-flash-lite">Gemini 3.5 Flash-Lite</option>
                <option value="gemini-3.6-flash">Gemini 3.6 Flash</option>
              </select>
            </div>

            <div class="smt-row">
              <label class="smt-label" for="smt-lang">Langues (source → cible)</label>
              <div class="smt-inline-row">
                <select class="smt-select" id="smt-lang">
                  <option value="en">Anglais</option>
                  <option value="ja">Japonais</option>
                  <option value="auto">Auto</option>
                </select>
                <select class="smt-select" id="smt-target-lang">
                  <option value="fr">→ Français</option>
                  <option value="en">→ English</option>
                  <option value="es">→ Español</option>
                  <option value="de">→ Deutsch</option>
                  <option value="it">→ Italiano</option>
                  <option value="pt">→ Português</option>
                </select>
              </div>
            </div>

            <div class="smt-row">
              <label class="smt-label" for="smt-glossary">Glossaire série</label>
              <div class="smt-inline-row">
                <select class="smt-select" id="smt-glossary">
                  <option value="auto">Auto (aucun détecté)</option>
                  <option value="none">Aucun</option>
                </select>
                <button class="smt-icon-btn" id="smt-glossary-edit-btn" type="button" title="Créer / éditer le glossaire">✎</button>
              </div>
            </div>

            <div id="smt-glossary-editor">
              <div class="smt-row">
                <label class="smt-label" for="smt-gl-title">Titre série</label>
                <input class="smt-input" id="smt-gl-title" type="text" placeholder="Ex: Komi cherche ses mots" />
              </div>
              <div class="smt-row">
                <label class="smt-label" for="smt-gl-keywords">Mots-clés URL / titre (séparés par virgule)</label>
                <input class="smt-input" id="smt-gl-keywords" type="text" placeholder="komi, komi-san" />
              </div>
              <div class="smt-row">
                <label class="smt-label" for="smt-gl-chars">Personnages (1/ligne : Nom (F/M) : notes)</label>
                <textarea class="smt-textarea" id="smt-gl-chars" placeholder="Shôko Komi (F) : timide, tutoie Tadano"></textarea>
              </div>
              <div class="smt-row">
                <label class="smt-label" for="smt-gl-terms">Termes (1/ligne : Anglais = Français)</label>
                <textarea class="smt-textarea" id="smt-gl-terms" placeholder="Golden Week = Golden Week"></textarea>
              </div>
              <div class="smt-row">
                <label class="smt-label" for="smt-gl-style">Notes de style</label>
                <textarea class="smt-textarea" id="smt-gl-style" placeholder="Tutoiement entre lycéens, HE=il / SHE=elle..."></textarea>
              </div>
              <div class="smt-btns">
                <button class="smt-btn primary" id="smt-gl-save" type="button">Enregistrer</button>
                <button class="smt-btn" id="smt-gl-new" type="button">+ Nouveau</button>
              </div>
              <div class="smt-btns">
                <button class="smt-btn danger" id="smt-gl-del" type="button">Suppr.</button>
                <button class="smt-btn" id="smt-gl-close" type="button">Fermer</button>
              </div>
            </div>
          </div>
        </div>

        <div class="smt-status" id="smt-status">Vérification du serveur…</div>
        <div class="smt-hints-row">
          <span class="smt-hint-chip"><span class="smt-kbd">⌥P</span> Chapitre</span>
          <span class="smt-hint-chip"><span class="smt-kbd">⌥T</span> Page</span>
          <span class="smt-hint-chip"><span class="smt-kbd">⌥H</span> Masquer</span>
        </div>
      </div>

      <button id="smt-pill" type="button" title="Yomi (⌥M)">
        <span id="smt-pill-cat" class="smt-mini-cat">${catSvg}</span>
        <span class="smt-dot" id="smt-dot"></span>
        <span id="smt-pill-label">Yomi</span>
      </button>
    `;

    document.body.appendChild(ui);
    uiRootEl = ui;
    uiPanelEl = ui.querySelector("#smt-panel");
    uiStatusEl = ui.querySelector("#smt-status");
    uiDotEl = ui.querySelector("#smt-dot");
    uiConnPill = ui.querySelector("#smt-conn-pill");
    uiAlertEl = ui.querySelector("#smt-alert");
    uiPillLabel = ui.querySelector("#smt-pill-label");
    uiAutoBtn = ui.querySelector("#smt-auto-btn");
    uiVisionSeg = ui.querySelector("#smt-vision-seg");
    uiToggleBtn = ui.querySelector("#smt-toggle-btn");
    uiChapterBtn = ui.querySelector("#smt-chapter-btn");
    uiTranslateBtn = ui.querySelector("#smt-translate-btn");
    uiRetryBtn = ui.querySelector("#smt-retry-btn");
    uiProgressWrap = ui.querySelector("#smt-progress");
    uiProgressBar = ui.querySelector("#smt-progress-bar");
    uiSettingsSummary = ui.querySelector("#smt-settings-summary");
    uiGlossarySel = ui.querySelector("#smt-glossary");
    uiGlossaryEditor = ui.querySelector("#smt-glossary-editor");

    const modelSel = ui.querySelector("#smt-model");
    const langSel = ui.querySelector("#smt-lang");
    const targetLangSel = ui.querySelector("#smt-target-lang");
    modelSel.value = prefs.model;
    langSel.value = prefs.sourceLang;
    targetLangSel.value = prefs.targetLang || "fr";
    renderGlossarySelect();
    applyUiTheme();

    if (window.matchMedia) {
      try {
        window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
          if ((prefs.theme || "system") === "system") applyUiTheme();
        });
      } catch {}
    }

    for (const b of ui.querySelectorAll("[data-smt-theme]")) {
      b.addEventListener("click", () => {
        prefs.theme = b.getAttribute("data-smt-theme") || "system";
        hasSavedTheme = true;
        savePrefs(prefs);
        applyUiTheme();
      });
    }

    const drawerToggle = ui.querySelector("#smt-settings-toggle");
    const drawerBody = ui.querySelector("#smt-settings-drawer");
    if (drawerToggle && drawerBody) {
      drawerToggle.addEventListener("click", () => {
        const nextOpen = !drawerBody.classList.contains("open");
        drawerToggle.classList.toggle("open", nextOpen);
        drawerBody.classList.toggle("open", nextOpen);
      });
    }

    function updateButtons() {
      if (uiAutoBtn) {
        uiAutoBtn.classList.toggle("on", Boolean(prefs.autoMode));
        uiAutoBtn.setAttribute("aria-checked", prefs.autoMode ? "true" : "false");
      }

      const visionSupported = modelSupportsVision(prefs.model);
      if (uiVisionSeg) {
        for (const vb of uiVisionSeg.querySelectorAll("[data-smt-vis]")) {
          const mode = vb.getAttribute("data-smt-vis");
          vb.classList.toggle("on", (prefs.visionMode || "auto") === mode);
          vb.disabled = !visionSupported && mode !== "off";
        }
        uiVisionSeg.title = visionSupported
          ? "Auto : Vision sur pages complexes · ON : toujours · OFF : texte rapide"
          : "Ce modèle ne gère pas les images";
      }

      if (uiToggleBtn) uiToggleBtn.textContent = prefs.visible ? "Masquer VF" : "Afficher VF";
      updateSettingsSummary();
      updateProgressUI();
    }
    updateButtons();

    ui.querySelector("#smt-pill").addEventListener("click", () => {
      state.panelOpen = !state.panelOpen;
      uiPanelEl.classList.toggle("open", state.panelOpen);
      if (state.panelOpen) {
        checkServerHealth();
        loadGlossaries();
      }
    });

    ui.querySelector("#smt-close").addEventListener("click", () => {
      state.panelOpen = false;
      uiPanelEl.classList.remove("open");
    });

    modelSel.addEventListener("change", () => {
      prefs.model = modelSel.value;
      savePrefs(prefs);
      state.quotaBlockedModel = null;
      updateButtons();
      const visInfo = isVisionEffective() ? " (Vision ON)" : "";
      setStatus(`Modèle : ${shortModelLabel(prefs.model)}${visInfo}`);
    });

    langSel.addEventListener("change", () => {
      prefs.sourceLang = langSel.value;
      savePrefs(prefs);
      updateSettingsSummary();
      setStatus(`Langue : ${prefs.sourceLang.toUpperCase()} → ${(prefs.targetLang || "fr").toUpperCase()}`);
    });

    targetLangSel.addEventListener("change", () => {
      prefs.targetLang = targetLangSel.value || "fr";
      hasSavedTargetLang = true;
      savePrefs(prefs);
      updateSettingsSummary();
      setStatus(`Langue : ${prefs.sourceLang.toUpperCase()} → ${prefs.targetLang.toUpperCase()}`);
    });

    uiGlossarySel.addEventListener("change", () => {
      prefs.glossarySlug = uiGlossarySel.value || "auto";
      savePrefs(prefs);
      if (prefs.glossarySlug === "none") {
        setStatus("Glossaire désactivé.");
      } else if (prefs.glossarySlug === "auto") {
        const detGl = state.glossaries.find((g) => g.slug === state.detectedGlossarySlug);
        setStatus(detGl ? `Glossaire Auto : ${detGl.title}` : "Glossaire Auto (aucun détecté)");
      } else {
        const chosen = state.glossaries.find((g) => g.slug === prefs.glossarySlug);
        setStatus(`Glossaire : ${chosen?.title || prefs.glossarySlug}`);
      }
    });

    ui.querySelector("#smt-glossary-edit-btn").addEventListener("click", () => {
      if (uiGlossaryEditor.classList.contains("open")) {
        uiGlossaryEditor.classList.remove("open");
      } else {
        openGlossaryEditor();
      }
    });

    ui.querySelector("#smt-gl-save").addEventListener("click", saveGlossaryFromEditor);
    ui.querySelector("#smt-gl-new").addEventListener("click", () => openGlossaryEditor(null));
    ui.querySelector("#smt-gl-del").addEventListener("click", deleteCurrentGlossary);
    ui.querySelector("#smt-gl-close").addEventListener("click", () => {
      uiGlossaryEditor.classList.remove("open");
    });

    uiChapterBtn.addEventListener("click", () => {
      startChapterPreload();
    });

    uiTranslateBtn.addEventListener("click", () => {
      translateVisiblePages(false);
    });

    uiRetryBtn.addEventListener("click", () => {
      translateVisiblePages(true);
    });

    if (uiVisionSeg) {
      for (const vb of uiVisionSeg.querySelectorAll("[data-smt-vis]")) {
        vb.addEventListener("click", () => {
          const nextMode = vb.getAttribute("data-smt-vis") || "auto";
          prefs.visionMode = nextMode;
          savePrefs(prefs);
          updateButtons();
          if (!modelSupportsVision(prefs.model)) {
            setStatus("Vision réservé à Gemma 4 / Gemma 3 / Gemini.");
          } else if (prefs.visionMode === "auto") {
            setStatus("Vision Auto : activé sur les pages complexes, rapide sur les pages simples.");
          } else if (prefs.visionMode === "on") {
            setStatus("Vision ON : toutes les pages passent en mode visuel.");
          } else {
            setStatus("Vision OFF : traduction texte rapide.");
          }
        });
      }
    }

    uiAutoBtn.addEventListener("click", () => {
      prefs.autoMode = !prefs.autoMode;
      savePrefs(prefs);
      updateButtons();
      if (prefs.autoMode) {
        startChapterPreload();
      } else {
        stopChapterPreload();
      }
    });

    uiToggleBtn.addEventListener("click", () => {
      prefs.visible = !prefs.visible;
      savePrefs(prefs);
      updateButtons();
      syncOverlayPositions();
    });
  }

  function updatePillVisibility() {
    if (!uiRootEl) return;
    const pages = findMangaElements();
    const hasManga = pages.length > 0;
    uiRootEl.classList.toggle("smt-visible", hasManga || state.forcedVisible || state.panelOpen);
    updateProgressUI(pages);
  }

  // --- Raccourcis clavier (Option+P, Option+T, Option+H, Option+M) ---
  window.addEventListener("keydown", (e) => {
    if (!e.altKey || e.metaKey || e.ctrlKey) return;
    const tag = (document.activeElement?.tagName || "").toLowerCase();
    if (tag === "input" || tag === "textarea" || document.activeElement?.isContentEditable) return;

    const code = e.code;
    if (code === "KeyP") {
      e.preventDefault();
      state.forcedVisible = true;
      updatePillVisibility();
      startChapterPreload();
    } else if (code === "KeyT") {
      e.preventDefault();
      state.forcedVisible = true;
      updatePillVisibility();
      translateVisiblePages(false);
    } else if (code === "KeyH") {
      e.preventDefault();
      prefs.visible = !prefs.visible;
      savePrefs(prefs);
      if (uiToggleBtn) uiToggleBtn.textContent = prefs.visible ? "Masquer VF" : "Afficher VF";
      syncOverlayPositions();
    } else if (code === "KeyM") {
      e.preventDefault();
      state.forcedVisible = true;
      state.panelOpen = !state.panelOpen;
      updatePillVisibility();
      if (uiPanelEl) uiPanelEl.classList.toggle("open", state.panelOpen);
      if (state.panelOpen) {
        checkServerHealth();
        loadGlossaries();
      }
    }
  });

  // --- Initialisation & suivi dynamique (changement de chapitre / lazy loading) ---
  buildUI();
  updatePillVisibility();
  checkServerHealth().then(() => {
    if (state.serverOnline) loadGlossaries();
    if (prefs.autoMode && state.serverOnline && state.apiKeyOk) {
      startChapterPreload();
    }
  });

  window.addEventListener("scroll", scheduleSync, { passive: true });
  window.addEventListener("resize", scheduleSync, { passive: true });
  window.addEventListener("load", () => {
    updatePillVisibility();
    scheduleSync();
  });

  // Vérification légère toutes les 2 s (changement de chapitre SPA / lazy-load) sans MutationObserver bloquant
  setInterval(() => {
    if (location.href !== state.lastUrl) {
      state.lastUrl = location.href;
      state.preloadAbort = false;
      loadGlossaries();
      syncOverlayPositions();
    }
    const pages = findMangaElements();
    if (uiRootEl) {
      uiRootEl.classList.toggle("smt-visible", pages.length > 0 || state.forcedVisible || state.panelOpen);
      updateProgressUI(pages);
    }
    if (state.overlays.size > 0) scheduleSync();

    // Si Mode Auto activé et de nouvelles planches non traduites apparaissent (lazy load ou chapitre suivant)
    if (
      prefs.autoMode &&
      !state.preloadRunning &&
      state.serverOnline &&
      state.apiKeyOk &&
      state.quotaBlockedModel !== prefs.model
    ) {
      const { total, done } = getChapterStats(pages);
      if (total > 0 && done < total) {
        startChapterPreload();
      }
    }
  }, 2000);
})();
