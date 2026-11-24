#!/usr/bin/env python3
"""
Yomi — Serveur local macOS Apple Silicon
Apple Vision (OCR natif) + Gemma 4 / Gemini API
Zéro dépendance externe (Python standard library uniquement).
"""

import base64
import glob
import hashlib
import ipaddress
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from collections import OrderedDict
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from socketserver import TCPServer
from pathlib import Path


class RateLimitError(RuntimeError):
    def __init__(self, message: str, retry_after: int = 15, is_daily: bool = False):
        super().__init__(message)
        self.retry_after = retry_after
        self.is_daily = is_daily

def _resolve_data_dir() -> Path:
    env_dir = os.environ.get("SMT_DATA_DIR")
    if env_dir:
        return Path(env_dir).expanduser()
    app_support = Path.home() / "Library" / "Application Support"
    yomi_dir = app_support / "Yomi"
    legacy_dir = app_support / "MangaTranslator"
    if not yomi_dir.exists() and legacy_dir.exists():
        try:
            legacy_dir.rename(yomi_dir)
        except OSError:
            pass
    return yomi_dir


BASE_DIR = Path(__file__).resolve().parent
DATA_DIR = _resolve_data_dir()
CONFIG_PATH = DATA_DIR / "config.json"
CACHE_DIR = DATA_DIR / "cache"
GLOSSARIES_DIR = DATA_DIR / "glossaries"
LEGACY_GLOSSARIES_DIR = BASE_DIR / "glossaries"

OCR_DIR = Path(os.environ.get("SMT_OCR_DIR") or (BASE_DIR / "ocr")).expanduser()
OCR_BINARY = OCR_DIR / "vision_ocr"
OCR_SOURCE = OCR_DIR / "vision_ocr.swift"
ANNOTATE_BINARY = OCR_DIR / "annotate_som"
ANNOTATE_SOURCE = OCR_DIR / "annotate_som.swift"
USERSCRIPT_PATH = Path(
    os.environ.get("SMT_USERSCRIPT_PATH")
    or (BASE_DIR / "userscript" / "yomi.user.js")
).expanduser()

TARGET_LANGUAGES = {
    "fr": "Français",
    "en": "English",
    "es": "Español",
    "de": "Deutsch",
    "it": "Italiano",
    "pt": "Português",
}

DEFAULT_CONFIG = {
    "onboarding_done": False,
    "ui_lang": "fr",
    "theme": "system",
    "mascot": "mochi",
    "engine_mode": "local",
    "default_model": "ollama:gemma4:12b",
    "api_provider": "gemini",
    "api_key": "",
    "api_base_url": "",
    "api_model": "gemini-3.8-flash",
    "source_lang": "en",
    "target_lang": "fr",
    "vision_mode": "auto",
    "gemini_api_key": "",
    "userscripts_dir": "",
}

API_PROVIDERS: dict[str, dict] = {
    "gemini": {
        "label": "Google Gemini",
        "protocol": "gemini",
        "base_url": "https://generativelanguage.googleapis.com/v1beta",
        "env_vars": ["GEMINI_API_KEY", "YOMI_API_KEY"],
        "default_model": "gemini-3.8-flash",
        "models": [
            {"id": "gemini-3.8-flash", "label": "Gemini 3.8 Flash"},
            {"id": "gemini-3.5-flash-lite", "label": "Gemini 3.5 Flash-Lite"},
            {"id": "gemini-3.6-flash", "label": "Gemini 3.6 Flash"},
        ],
    },
    "openai": {
        "label": "OpenAI",
        "protocol": "openai_compat",
        "base_url": "https://api.openai.com/v1",
        "env_vars": ["OPENAI_API_KEY", "YOMI_API_KEY"],
        "default_model": "gpt-4.1-mini",
        "models": [
            {"id": "gpt-4.1-mini", "label": "GPT-4.1 Mini"},
            {"id": "gpt-4.1", "label": "GPT-4.1"},
            {"id": "gpt-4o-mini", "label": "GPT-4o Mini"},
            {"id": "gpt-4o", "label": "GPT-4o"},
        ],
    },
    "anthropic": {
        "label": "Anthropic Claude",
        "protocol": "anthropic",
        "base_url": "https://api.anthropic.com/v1",
        "env_vars": ["ANTHROPIC_API_KEY", "YOMI_API_KEY"],
        "default_model": "claude-sonnet-4-5",
        "models": [
            {"id": "claude-sonnet-4-5", "label": "Claude Sonnet 4.5"},
            {"id": "claude-haiku-4-5", "label": "Claude Haiku 4.5"},
            {"id": "claude-opus-4-1", "label": "Claude Opus 4.1"},
        ],
    },
    "openrouter": {
        "label": "OpenRouter",
        "protocol": "openai_compat",
        "base_url": "https://openrouter.ai/api/v1",
        "env_vars": ["OPENROUTER_API_KEY", "YOMI_API_KEY"],
        "default_model": "google/gemini-2.5-flash",
        "models": [
            {"id": "google/gemini-2.5-flash", "label": "Gemini 2.5 Flash"},
            {"id": "openai/gpt-4.1-mini", "label": "GPT-4.1 Mini"},
            {"id": "anthropic/claude-sonnet-4", "label": "Claude Sonnet 4"},
            {"id": "qwen/qwen2.5-vl-72b-instruct", "label": "Qwen 2.5 VL 72B"},
        ],
    },
    "mistral": {
        "label": "Mistral AI",
        "protocol": "openai_compat",
        "base_url": "https://api.mistral.ai/v1",
        "env_vars": ["MISTRAL_API_KEY", "YOMI_API_KEY"],
        "default_model": "mistral-medium-latest",
        "models": [
            {"id": "mistral-medium-latest", "label": "Mistral Medium"},
            {"id": "mistral-large-latest", "label": "Mistral Large"},
            {"id": "pixtral-large-latest", "label": "Pixtral Large (Vision)"},
            {"id": "mistral-small-latest", "label": "Mistral Small"},
        ],
    },
    "deepseek": {
        "label": "DeepSeek",
        "protocol": "openai_compat",
        "base_url": "https://api.deepseek.com/v1",
        "env_vars": ["DEEPSEEK_API_KEY", "YOMI_API_KEY"],
        "default_model": "deepseek-chat",
        "models": [
            {"id": "deepseek-chat", "label": "DeepSeek Chat (V3)"},
            {"id": "deepseek-reasoner", "label": "DeepSeek Reasoner (R1)"},
        ],
    },
    "groq": {
        "label": "Groq",
        "protocol": "openai_compat",
        "base_url": "https://api.groq.com/openai/v1",
        "env_vars": ["GROQ_API_KEY", "YOMI_API_KEY"],
        "default_model": "meta-llama/llama-4-scout-17b-16e-instruct",
        "models": [
            {"id": "meta-llama/llama-4-scout-17b-16e-instruct", "label": "Llama 4 Scout 17B (Vision)"},
            {"id": "meta-llama/llama-4-maverick-17b-128e-instruct", "label": "Llama 4 Maverick 17B"},
            {"id": "llama-3.3-70b-versatile", "label": "Llama 3.3 70B"},
        ],
    },
    "xai": {
        "label": "xAI (Grok)",
        "protocol": "openai_compat",
        "base_url": "https://api.x.ai/v1",
        "env_vars": ["XAI_API_KEY", "YOMI_API_KEY"],
        "default_model": "grok-2-vision-latest",
        "models": [
            {"id": "grok-2-vision-latest", "label": "Grok 2 Vision"},
            {"id": "grok-3-mini-fast", "label": "Grok 3 Mini Fast"},
        ],
    },
    "custom": {
        "label": "Custom (OpenAI-compatible)",
        "protocol": "openai_compat",
        "base_url": "",
        "env_vars": ["YOMI_API_KEY", "OPENAI_API_KEY"],
        "default_model": "",
        "models": [],
    },
}

CONFIG_LOCK = threading.Lock()
GLOSSARIES_LOCK = threading.Lock()

DEFAULT_KOMI_GLOSSARY = {
    "slug": "komi-cant-communicate",
    "title": "Komi cherche ses mots (Komi Can't Communicate)",
    "match_keywords": [
        "komi",
        "komi-san",
        "komi-can-t-communicate",
        "komi-cherche-ses-mots",
    ],
    "characters": [
        {
            "name": "Shôko Komi",
            "gender": "F",
            "notes": "Héroïne très timide/muette (communique par carnet ou murmures). Tutoie Tadano, Najimi et sa famille ; vouvoie les inconnus.",
        },
        {
            "name": "Hitohito Tadano",
            "gender": "M",
            "notes": "Lycéen gentil et attentif. Masculin (« pensé-t-il » quand narré au masculin). Tutoie Komi, Najimi et Katai.",
        },
        {
            "name": "Najimi Osana",
            "gender": "NB",
            "notes": "Ami(e) d'enfance extraverti(e) et taquin(e). Ton familier, tutoie tout le monde.",
        },
        {
            "name": "Shôsuke Komi",
            "gender": "M",
            "notes": "Petit frère de Shôko, laconique et stoïque. Masculin.",
        },
        {
            "name": "Shûko Komi",
            "gender": "F",
            "notes": "Mère de Shôko, très joyeuse et bavarde. Féminin, tutoie sa famille (« ma chérie »).",
        },
        {
            "name": "Masayoshi Komi",
            "gender": "M",
            "notes": "Père de Shôko, calme et silencieux. Masculin.",
        },
        {
            "name": "Makoto Katai",
            "gender": "M",
            "notes": "Grand garçon musclé mais très timide. Masculin, poli avec ses camarades.",
        },
        {
            "name": "Ai Katai",
            "gender": "F",
            "notes": "Petite sœur de Makoto, look de délinquante, protectrice avec son frère (« cheffe »). Féminin.",
        },
        {
            "name": "Rumiko Manbagi",
            "gender": "F",
            "notes": "Gyaru expressive et chaleureuse, amie proche de Komi. Féminin, ton oral vif.",
        },
        {
            "name": "Ren Yamai",
            "gender": "F",
            "notes": "Admiratrice excessive de Komi. Féminin.",
        },
        {
            "name": "Himiko Agari",
            "gender": "F",
            "notes": "Anxieuse et timide. Féminin.",
        },
        {
            "name": "Shisuto Naruse",
            "gender": "M",
            "notes": "Narcissique théâtral. Masculin.",
        },
        {
            "name": "Chûsaku Kometani",
            "gender": "M",
            "notes": "Commentateur pince-sans-rire. Masculin.",
        },
    ],
    "terms": {
        "Golden Week": "Golden Week",
        "Comm.": "Chap.",
        "Hard liner": "Puriste",
    },
    "style_notes": (
        "Tutoiement naturel entre lycéens/amis et en famille ; vouvoiement envers les adultes/inconnus. "
        "Respecter strictement les pronoms HE (il) / SHE (elle) dans la narration (ex. « SHE THOUGHT » -> « pensa-t-elle »). "
        "Supprimer -san/-kun/-chan quand c'est plus fluide en français."
    ),
}


def sanitize_glossary_slug(raw: str) -> str:
    ascii_txt = (
        unicodedata.normalize("NFKD", str(raw or ""))
        .encode("ascii", "ignore")
        .decode("ascii")
        .lower()
    )
    slug = re.sub(r"[^a-z0-9]+", "-", ascii_txt).strip("-")
    if slug in ("auto", "none"):
        slug = f"{slug}-series"
    return slug[:80]


def _normalize_glossary_dict(data: dict) -> dict:
    slug = sanitize_glossary_slug(data.get("slug") or data.get("title") or "")
    title = str(data.get("title") or slug).strip()
    keywords = [str(k).strip().lower() for k in (data.get("match_keywords") or []) if str(k).strip()]
    chars: list[dict] = []
    for c in data.get("characters") or []:
        if not isinstance(c, dict):
            continue
        name = str(c.get("name") or "").strip()
        if not name:
            continue
        gender = str(c.get("gender") or "").strip().upper()
        if gender not in ("F", "M", "NB"):
            gender = ""
        chars.append({"name": name, "gender": gender, "notes": str(c.get("notes") or "").strip()})

    terms: dict[str, str] = {}
    raw_terms = data.get("terms") or {}
    if isinstance(raw_terms, dict):
        for k, v in raw_terms.items():
            ks, vs = str(k).strip(), str(v).strip()
            if ks and vs:
                terms[ks] = vs

    return {
        "slug": slug,
        "title": title,
        "match_keywords": keywords,
        "characters": chars,
        "terms": terms,
        "style_notes": str(data.get("style_notes") or "").strip(),
    }


def ensure_data_dirs() -> None:
    """Initialise les dossiers utilisateur (~Application Support) et migre une seule fois les glossaires existants."""
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    with GLOSSARIES_LOCK:
        GLOSSARIES_DIR.mkdir(parents=True, exist_ok=True)
        marker = DATA_DIR / ".migrated_glossaries"
        if (
            LEGACY_GLOSSARIES_DIR != GLOSSARIES_DIR
            and LEGACY_GLOSSARIES_DIR.exists()
            and not marker.exists()
        ):
            for src in LEGACY_GLOSSARIES_DIR.glob("*.json"):
                dst = GLOSSARIES_DIR / src.name
                if not dst.exists():
                    try:
                        shutil.copy2(src, dst)
                    except OSError:
                        pass
            try:
                marker.write_text("1", encoding="utf-8")
            except OSError:
                pass


def ensure_default_glossaries() -> None:
    ensure_data_dirs()


def _normalize_config(raw: dict) -> dict:
    cfg = dict(DEFAULT_CONFIG)
    if isinstance(raw, dict):
        for key in DEFAULT_CONFIG:
            if key in raw and raw[key] is not None:
                cfg[key] = raw[key]
    cfg["onboarding_done"] = bool(cfg.get("onboarding_done", False))
    cfg["ui_lang"] = "en" if str(cfg.get("ui_lang", "fr")).lower() == "en" else "fr"
    thm = str(cfg.get("theme", "system")).strip().lower()
    cfg["theme"] = thm if thm in ("light", "dark", "system") else "system"
    mascot = str(cfg.get("mascot", "mochi")).strip().lower()
    cfg["mascot"] = mascot if mascot in ("mochi", "kuro", "sakura") else "mochi"
    eng = str(cfg.get("engine_mode", "local")).strip().lower()
    cfg["engine_mode"] = eng if eng in ("local", "api", "both") else "local"
    cfg["default_model"] = str(cfg.get("default_model") or "ollama:gemma4:12b").strip()
    src = str(cfg.get("source_lang") or "en").strip().lower()
    cfg["source_lang"] = src if src in ("en", "ja", "auto") else "en"
    tgt = str(cfg.get("target_lang") or "fr").strip().lower()
    cfg["target_lang"] = tgt if tgt in TARGET_LANGUAGES else "fr"
    cfg["vision_mode"] = normalize_vision_mode(cfg.get("vision_mode", "auto"))
    prov = str(cfg.get("api_provider") or "gemini").strip().lower()
    cfg["api_provider"] = prov if prov in API_PROVIDERS else "gemini"
    cfg["api_key"] = str(cfg.get("api_key") or "").strip()
    cfg["gemini_api_key"] = str(cfg.get("gemini_api_key") or "").strip()
    if not cfg["api_key"] and cfg["api_provider"] == "gemini" and cfg["gemini_api_key"]:
        cfg["api_key"] = cfg["gemini_api_key"]
    cfg["api_base_url"] = str(cfg.get("api_base_url") or "").strip().rstrip("/")
    cfg["api_model"] = str(cfg.get("api_model") or API_PROVIDERS[cfg["api_provider"]]["default_model"]).strip()
    cfg["userscripts_dir"] = str(cfg.get("userscripts_dir") or "").strip()
    return cfg


def load_config() -> dict:
    with CONFIG_LOCK:
        if CONFIG_PATH.exists():
            try:
                raw = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
                return _normalize_config(raw)
            except Exception:
                pass
    return dict(DEFAULT_CONFIG)


def save_config(updates: dict) -> dict:
    if not isinstance(updates, dict):
        raise ValueError("Configuration JSON invalide.")
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    with CONFIG_LOCK:
        current = dict(DEFAULT_CONFIG)
        if CONFIG_PATH.exists():
            try:
                current = _normalize_config(json.loads(CONFIG_PATH.read_text(encoding="utf-8")))
            except Exception:
                current = dict(DEFAULT_CONFIG)
        old_provider = str(current.get("api_provider") or "gemini").strip().lower()
        for key, val in updates.items():
            if key in DEFAULT_CONFIG and val is not None:
                if key in ("gemini_api_key", "api_key") and isinstance(val, str) and not val.strip() and updates.get("keep_existing_key"):
                    continue
                current[key] = val
        new_provider = str(current.get("api_provider") or "gemini").strip().lower()
        if new_provider not in API_PROVIDERS:
            new_provider = "gemini"
        current["api_provider"] = new_provider
        new_key_str = str(updates.get("api_key") or updates.get("gemini_api_key") or "").strip()
        if updates.get("clear_api_key"):
            current["api_key"] = ""
            current["gemini_api_key"] = ""
        elif new_key_str:
            current["api_key"] = new_key_str
            if new_provider == "gemini":
                current["gemini_api_key"] = new_key_str
        elif new_provider != old_provider:
            current["api_key"] = str(current.get("gemini_api_key") or "") if new_provider == "gemini" else ""
        normalized = _normalize_config(current)
        CONFIG_PATH.write_text(json.dumps(normalized, indent=2, ensure_ascii=False), encoding="utf-8")
        try:
            os.chmod(CONFIG_PATH, 0o600)
        except OSError:
            pass
        return normalized


def mask_api_key(key: str) -> str:
    k = str(key or "").strip()
    if not k:
        return ""
    if len(k) <= 8:
        return "*" * len(k)
    return f"{k[:4]}...{k[-4:]}"


def public_config(cfg: dict) -> dict:
    prov = str(cfg.get("api_provider") or "gemini").strip().lower()
    eff_key = get_api_key(provider=prov, cfg=cfg)
    out = {k: v for k, v in cfg.items() if k not in ("gemini_api_key", "api_key")}
    out["has_api_key"] = bool(eff_key)
    out["api_key_masked"] = mask_api_key(eff_key)
    out["target_languages"] = TARGET_LANGUAGES
    return out


def list_glossaries() -> list[dict]:
    if not GLOSSARIES_DIR.exists():
        ensure_data_dirs()
    items: list[dict] = []
    with GLOSSARIES_LOCK:
        if not GLOSSARIES_DIR.exists():
            return []
        for path in sorted(GLOSSARIES_DIR.glob("*.json")):
            try:
                raw = json.loads(path.read_text(encoding="utf-8"))
                if isinstance(raw, dict):
                    raw.setdefault("slug", path.stem)
                    items.append(_normalize_glossary_dict(raw))
            except Exception:
                continue
    items.sort(key=lambda g: g.get("title", "").lower())
    return items


def load_glossary(slug: str) -> dict | None:
    safe_slug = sanitize_glossary_slug(slug)
    if not safe_slug:
        return None
    path = GLOSSARIES_DIR / f"{safe_slug}.json"
    with GLOSSARIES_LOCK:
        if not path.exists():
            return None
        try:
            raw = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(raw, dict):
                raw["slug"] = safe_slug
                return _normalize_glossary_dict(raw)
        except Exception:
            return None
    return None


def save_glossary(payload: dict) -> dict:
    if not isinstance(payload, dict):
        raise ValueError("Corps JSON de glossaire invalide.")
    raw_slug = str(payload.get("slug") or payload.get("title") or "").strip()
    safe_slug = sanitize_glossary_slug(raw_slug)
    if not safe_slug:
        raise ValueError("Titre ou identifiant de série requis.")
    normalized = _normalize_glossary_dict({**payload, "slug": safe_slug})
    if not normalized["title"]:
        raise ValueError("Titre de série requis.")
    with GLOSSARIES_LOCK:
        GLOSSARIES_DIR.mkdir(parents=True, exist_ok=True)
        path = GLOSSARIES_DIR / f"{safe_slug}.json"
        path.write_text(json.dumps(normalized, indent=2, ensure_ascii=False), encoding="utf-8")
    return normalized


def delete_glossary(slug: str) -> bool:
    safe_slug = sanitize_glossary_slug(slug)
    if not safe_slug:
        return False
    path = GLOSSARIES_DIR / f"{safe_slug}.json"
    with GLOSSARIES_LOCK:
        if not path.exists():
            return False
        path.unlink()
        return True


def resolve_glossary(
    glossary_slug: str = "auto", page_url: str = "", page_title: str = ""
) -> dict | None:
    slug = str(glossary_slug or "auto").strip().lower()
    if slug == "none":
        return None
    if slug and slug != "auto":
        return load_glossary(slug)

    hay_raw = f"{page_url} {page_title}".lower()
    if not hay_raw.strip():
        return None
    hay_norm = re.sub(r"[^a-z0-9]+", " ", hay_raw)
    for gl in list_glossaries():
        candidates = list(gl.get("match_keywords") or [])
        candidates.extend([gl.get("slug", ""), gl.get("title", "")])
        for kw in candidates:
            kw_low = str(kw or "").strip().lower()
            if not kw_low:
                continue
            kw_norm = re.sub(r"[^a-z0-9]+", " ", kw_low).strip()
            if kw_low in hay_raw or (kw_norm and re.search(rf"(?<![a-z0-9]){re.escape(kw_norm)}(?![a-z0-9])", hay_norm)):
                return gl
    return None


def compute_glossary_hash(glossary: dict | None) -> str:
    if not glossary:
        return "none"
    payload = json.dumps(glossary, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:8]


def format_glossary_for_prompt(glossary: dict | None) -> str:
    if not glossary:
        return ""
    lines = [f"GLOSSAIRE SÉRIE ACTIVE ({glossary['title']}) :"]
    chars = glossary.get("characters") or []
    if chars:
        lines.append("Personnages (respecter genre et registre) :")
        for c in chars:
            g_tag = f" ({c['gender']})" if c.get("gender") else ""
            notes = f" : {c['notes']}" if c.get("notes") else ""
            lines.append(f"- {c['name']}{g_tag}{notes}")
    terms = glossary.get("terms") or {}
    if terms:
        lines.append("Termes spécifiques :")
        for k, v in terms.items():
            lines.append(f'- "{k}" -> "{v}"')
    if glossary.get("style_notes"):
        lines.append(f"Style série : {glossary['style_notes']}")
    lines.append("")
    return "\n".join(lines) + "\n"

GEMINI_API_MODELS = [
    {"id": "gemini-3.8-flash", "label": "API Gemini · 3.8 Flash"},
    {"id": "gemini-3.5-flash-lite", "label": "API Gemini · 3.5 Flash-Lite"},
    {"id": "gemini-3.6-flash", "label": "API Gemini · 3.6 Flash"},
]

OLLAMA_MODEL_LABELS = {
    "gemma4:12b": "Local · Gemma 4 12B (Vision + Top Qualité)",
    "gemma4:e4b": "Local · Gemma 4 E4B (Vision · Mac 8 Go)",
    "gemma4:e2b": "Local · Gemma 4 E2B (Vision · Ultra-léger)",
    "gemma4:26b": "Local · Gemma 4 26B MoE (Vision · Mac 24 Go+)",
    "gemma4:31b": "Local · Gemma 4 31B (Vision · Mac 32 Go+)",
    "gemma3:12b": "Local · Gemma 3 12B (Vision)",
    "gemma3:4b": "Local · Gemma 3 4B (Vision · Rapide)",
}

AVAILABLE_MODELS = [
    {"id": "ollama:gemma4:12b", "label": OLLAMA_MODEL_LABELS["gemma4:12b"]},
    *GEMINI_API_MODELS,
]

DEFAULT_MODEL = "ollama:gemma4:12b"
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://127.0.0.1:11434").rstrip("/")
MAX_IMAGE_BYTES = 20 * 1024 * 1024  # 20 MB

PUNCT_ONLY_RE = re.compile(r"^[\s!?.,…\-~•·:;()\"'「」『』！？。、・*]+$")

PREFETCH_LOCK = threading.Lock()
PREFETCH_CACHE: OrderedDict[tuple[str, str], dict] = OrderedDict()
PREFETCH_INFLIGHT: set[tuple[str, str]] = set()
MAX_PREFETCH_ENTRIES = 4


def is_punctuation_only(text: str) -> bool:
    """Retourne True pour les bulles contenant uniquement de la ponctuation (!, ?!, ...)."""
    cleaned = text.strip()
    return not cleaned or bool(PUNCT_ONLY_RE.fullmatch(cleaned))


def normalize_vision_mode(raw_val) -> str:
    if isinstance(raw_val, bool):
        return "on" if raw_val else "off"
    s = str(raw_val or "").strip().lower()
    if s in ("on", "true", "1"):
        return "on"
    if s in ("off", "false", "0"):
        return "off"
    return "auto"


def should_use_vision(vision_mode: str, model: str, bubbles: list[dict]) -> bool:
    """Mode Vision adaptatif : réserve Vision aux pages complexes ou à faible confiance OCR."""
    if not model_supports_vision(model):
        return False
    if vision_mode == "off":
        return False
    translatable = [b for b in bubbles if not is_punctuation_only(b.get("text", ""))]
    if not translatable:
        return False
    if vision_mode == "on":
        return True
    if len(translatable) >= 3:
        return True
    if any(b.get("style") == "floating" for b in translatable):
        return True
    if any(float(b.get("min_conf", 1.0)) < 0.85 for b in translatable):
        return True
    return False


def format_cloud_model_id(provider: str, raw_model: str) -> str:
    prov = (provider or "gemini").strip().lower()
    raw = (raw_model or "").strip()
    if not raw:
        raw = str(API_PROVIDERS.get(prov, {}).get("default_model") or "")
    if not raw:
        return prov
    if prov == "gemini":
        return raw.removeprefix("gemini:")
    if raw.startswith(f"{prov}:"):
        return raw
    return f"{prov}:{raw}"


def parse_model_spec(model: str, cfg: dict | None = None) -> tuple[str, str]:
    """Détermine (fournisseur, identifiant_modèle_brut) depuis un ID modèle."""
    spec = str(model or "").strip()
    if spec.startswith("ollama:"):
        return "ollama", spec.removeprefix("ollama:").strip()
    if ":" in spec:
        prefix, rest = spec.split(":", 1)
        low = prefix.strip().lower()
        if low in API_PROVIDERS:
            return low, rest.strip()
    if spec.lower().startswith("gemini-"):
        return "gemini", spec
    eff_cfg = cfg if cfg is not None else load_config()
    prov = str(eff_cfg.get("api_provider") or "gemini").strip().lower()
    if prov not in API_PROVIDERS:
        prov = "gemini"
    raw = spec or str(eff_cfg.get("api_model") or API_PROVIDERS[prov]["default_model"] or "").strip()
    return prov, raw


def model_supports_vision(model: str) -> bool:
    """Retourne True si le modèle accepte les images (multimodal)."""
    if model.startswith("ollama:"):
        low = model.lower()
        return any(
            tag in low
            for tag in ("gemma4", "gemma3", "llava", "qwen2.5vl", "qwen3-vl", "minicpm-v", "llama3.2-vision", "vision")
        )
    prov, raw_model = parse_model_spec(model)
    low_m = raw_model.lower()
    if prov == "deepseek":
        return False
    if prov == "groq":
        return ("llama-4" in low_m) or ("vision" in low_m)
    if any(t in low_m for t in ("o1-mini", "deepseek-r1", "deepseek-chat", "deepseek-reasoner")):
        return False
    return True


def build_available_models(ollama_models: list[str] | None = None) -> list[dict]:
    """Construit la liste dynamique des modèles installés localement + fournisseur API actif."""
    ordered_tags = ["gemma4:12b", "gemma4:e4b", "gemma4:26b", "gemma4:e2b", "gemma4:31b", "gemma3:12b", "gemma3:4b"]
    installed = [str(m).strip() for m in (ollama_models or []) if str(m).strip()]
    seen_ids: set[str] = set()
    local_entries: list[dict] = []

    def _add_local(tag: str) -> None:
        mid = f"ollama:{tag}"
        if mid in seen_ids:
            return
        seen_ids.add(mid)
        base_tag = tag[:-7] if tag.endswith(":latest") else tag
        label = OLLAMA_MODEL_LABELS.get(tag) or OLLAMA_MODEL_LABELS.get(base_tag)
        if not label:
            vis = "Vision" if model_supports_vision(mid) else "Texte"
            label = f"Local · {tag} ({vis})"
        local_entries.append({"id": mid, "label": label})

    for preferred in ordered_tags:
        for inst in installed:
            if inst == preferred or inst == f"{preferred}:latest":
                _add_local(preferred if inst == f"{preferred}:latest" else inst)
    for inst in installed:
        _add_local(inst)

    cfg = load_config()
    if not local_entries:
        cfg_default = cfg.get("default_model", "")
        if cfg_default.startswith("ollama:"):
            _add_local(cfg_default.removeprefix("ollama:"))

    prov = str(cfg.get("api_provider") or "gemini").strip().lower()
    pinfo = API_PROVIDERS.get(prov, API_PROVIDERS["gemini"])
    cloud_entries: list[dict] = []
    cloud_seen: set[str] = set()

    def _add_cloud(raw_id: str, nice_label: str | None = None) -> None:
        if not raw_id:
            return
        full_id = format_cloud_model_id(prov, raw_id)
        if full_id in cloud_seen:
            return
        cloud_seen.add(full_id)
        prefix_lbl = "API Gemini" if prov == "gemini" else f"API {pinfo['label']}"
        cloud_entries.append({"id": full_id, "label": f"{prefix_lbl} · {nice_label or raw_id}"})

    cfg_api_model = str(cfg.get("api_model") or "").strip()
    if cfg_api_model:
        preset_match = next((m["label"] for m in pinfo["models"] if m["id"] == cfg_api_model), None)
        _add_cloud(cfg_api_model, preset_match)
    for m in pinfo["models"]:
        _add_cloud(m["id"], m["label"])

    if not cloud_entries:
        cloud_entries = list(GEMINI_API_MODELS)

    return [*local_entries, *cloud_entries]


def check_ollama_status() -> tuple[bool, list[str]]:
    """Vérifie si Ollama tourne localement et liste les modèles installés."""
    try:
        req = urllib.request.Request(f"{OLLAMA_URL}/api/tags")
        with urllib.request.urlopen(req, timeout=1.5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            names = [m.get("name", "") for m in data.get("models", []) if m.get("name")]
            return True, names
    except Exception:
        return False, []


def warmup_ollama_model(model: str) -> None:
    """Précharge silencieusement le modèle local en RAM au lancement du serveur."""
    if not model.startswith("ollama:"):
        return
    ollama_model = model.removeprefix("ollama:").strip() or "gemma4:12b"
    try:
        body = {
            "model": ollama_model,
            "prompt": "",
            "keep_alive": "10m",
            "options": {"num_ctx": 4096},
        }
        req = urllib.request.Request(
            f"{OLLAMA_URL}/api/generate",
            data=json.dumps(body).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=60):
            pass
        print(f"[Ollama] Préchauffage {ollama_model} terminé.", flush=True)
    except Exception:
        pass


def unload_ollama_models() -> list[str]:
    """Décharge immédiatement les modèles Ollama de la RAM (keep_alive=0)."""
    unloaded: list[str] = []
    try:
        with urllib.request.urlopen(f"{OLLAMA_URL}/api/ps", timeout=2) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            models = [m.get("name", "") for m in data.get("models", []) if m.get("name")]
    except Exception:
        return unloaded

    for name in models:
        try:
            req = urllib.request.Request(
                f"{OLLAMA_URL}/api/generate",
                data=json.dumps({"model": name, "prompt": "", "keep_alive": 0}).encode("utf-8"),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=5):
                pass
            unloaded.append(name)
        except Exception:
            pass
    if unloaded:
        print(f"[Ollama] RAM libérée pour : {', '.join(unloaded)}", flush=True)
    return unloaded


def prune_disk_cache(max_files: int = 1500, max_age_days: int = 30) -> int:
    """Supprime les fichiers de cache trop anciens ou excédentaires."""
    if not CACHE_DIR.exists():
        return 0
    now = time.time()
    max_age_sec = max_age_days * 86400
    entries: list[tuple[float, Path]] = []
    deleted = 0

    for p in CACHE_DIR.glob("*.json"):
        try:
            mtime = p.stat().st_mtime
        except OSError:
            continue
        if (now - mtime) > max_age_sec:
            try:
                p.unlink()
                deleted += 1
            except OSError:
                pass
        else:
            entries.append((mtime, p))

    if len(entries) > max_files:
        entries.sort(key=lambda item: item[0])
        for _mtime, p in entries[: len(entries) - max_files]:
            try:
                p.unlink()
                deleted += 1
            except OSError:
                pass

    if deleted:
        print(f"[Cache] Rotation : {deleted} ancien(s) fichier(s) supprimé(s).", flush=True)
    return deleted


def load_dotenv(path: Path) -> None:
    """Charge un fichier .env simple sans dépendance externe."""
    if not path.exists():
        return
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]
        if key and key not in os.environ:
            os.environ[key] = value


def get_api_key(provider: str | None = None, cfg: dict | None = None) -> str:
    eff_cfg = cfg if cfg is not None else load_config()
    active_prov = str(eff_cfg.get("api_provider") or "gemini").strip().lower()
    prov = (provider or active_prov).strip().lower()
    if prov not in API_PROVIDERS:
        prov = active_prov if active_prov in API_PROVIDERS else "gemini"

    if prov == active_prov:
        cfg_key = str(eff_cfg.get("api_key") or "").strip()
        if cfg_key:
            return cfg_key
    if prov == "gemini":
        gem_key = str(eff_cfg.get("gemini_api_key") or "").strip()
        if gem_key:
            return gem_key

    env_vars = API_PROVIDERS.get(prov, {}).get("env_vars", [])
    for env_name in env_vars:
        if env_name == "YOMI_API_KEY" and prov != active_prov:
            continue
        val = os.environ.get(env_name, "").strip()
        if val:
            return val
    return ""


def _resolve_provider_base_url(provider: str, base_url: str = "", cfg: dict | None = None) -> str:
    eff_cfg = cfg if cfg is not None else load_config()
    raw = (base_url or "").strip()
    if not raw and provider == str(eff_cfg.get("api_provider") or "gemini").strip().lower():
        raw = str(eff_cfg.get("api_base_url") or "").strip()
    if not raw:
        raw = str(API_PROVIDERS.get(provider, {}).get("base_url") or "").strip()
    raw = raw.rstrip("/")
    parsed = urllib.parse.urlparse(raw)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError(f"URL de base invalide ou manquante pour le fournisseur '{provider}'.")
    return raw


def test_cloud_api_key(
    provider: str,
    api_key: str,
    base_url: str = "",
    model: str = "",
) -> tuple[bool, str]:
    prov = (provider or "gemini").strip().lower()
    if prov not in API_PROVIDERS:
        return False, f"Fournisseur inconnu : {prov}"
    key = str(api_key or "").strip()
    if not key:
        return False, "Clé API vide."
    try:
        resolved_base = _resolve_provider_base_url(prov, base_url=base_url)
    except ValueError as err:
        return False, str(err)

    pinfo = API_PROVIDERS[prov]
    proto = pinfo["protocol"]
    label = pinfo["label"]

    if proto == "gemini":
        req = urllib.request.Request(
            f"{resolved_base}/models?pageSize=1",
            headers={"x-goog-api-key": key},
            method="GET",
        )
        try:
            with urllib.request.urlopen(req, timeout=12) as resp:
                if 200 <= resp.status < 300:
                    return True, f"Clé {label} valide."
                return False, f"Réponse inattendue ({resp.status})."
        except urllib.error.HTTPError as e:
            return False, f"Clé refusée ({e.code})."
        except urllib.error.URLError as e:
            return False, f"Erreur réseau : {e.reason}"

    if proto == "anthropic":
        headers = {
            "x-api-key": key,
            "anthropic-version": "2023-06-01",
        }
        req = urllib.request.Request(f"{resolved_base}/models?limit=1", headers=headers, method="GET")
        try:
            with urllib.request.urlopen(req, timeout=12) as resp:
                if 200 <= resp.status < 300:
                    return True, f"Clé {label} valide."
                return False, f"Réponse inattendue ({resp.status})."
        except urllib.error.HTTPError as e:
            if e.code in (404, 405):
                probe_body = {
                    "model": (model or pinfo["default_model"] or "claude-haiku-4-5").strip(),
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "hi"}],
                }
                probe_req = urllib.request.Request(
                    f"{resolved_base}/messages",
                    data=json.dumps(probe_body).encode("utf-8"),
                    headers={**headers, "Content-Type": "application/json"},
                    method="POST",
                )
                try:
                    with urllib.request.urlopen(probe_req, timeout=12):
                        return True, f"Clé {label} valide."
                except urllib.error.HTTPError as pe:
                    return False, f"Clé ou modèle refusé ({pe.code})."
                except urllib.error.URLError as pe:
                    return False, f"Erreur réseau : {pe.reason}"
            return False, f"Clé refusée ({e.code})."
        except urllib.error.URLError as e:
            return False, f"Erreur réseau : {e.reason}"

    # OpenAI-compatible
    headers = {"Authorization": f"Bearer {key}"}
    check_url = f"{resolved_base}/models"
    if prov == "openrouter":
        headers["HTTP-Referer"] = "https://github.com/yomi"
        headers["X-Title"] = "Yomi"
        check_url = f"{resolved_base}/auth/key"
    req = urllib.request.Request(check_url, headers=headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=12) as resp:
            if 200 <= resp.status < 300:
                return True, f"Clé {label} valide."
            return False, f"Réponse inattendue ({resp.status})."
    except urllib.error.HTTPError as e:
        if e.code in (404, 405):
            probe_body = {
                "model": (model or pinfo["default_model"] or "gpt-4o-mini").strip(),
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "hi"}],
            }
            probe_req = urllib.request.Request(
                f"{resolved_base}/chat/completions",
                data=json.dumps(probe_body).encode("utf-8"),
                headers={**headers, "Content-Type": "application/json"},
                method="POST",
            )
            try:
                with urllib.request.urlopen(probe_req, timeout=12):
                    return True, f"Clé {label} valide."
            except urllib.error.HTTPError as pe:
                return False, f"Clé ou modèle refusé ({pe.code})."
            except urllib.error.URLError as pe:
                return False, f"Erreur réseau : {pe.reason}"
        return False, f"Clé refusée ({e.code})."
    except urllib.error.URLError as e:
        return False, f"Erreur réseau : {e.reason}"


def test_gemini_api_key(api_key: str) -> tuple[bool, str]:
    return test_cloud_api_key("gemini", api_key)


def _ollama_has_model(installed: list[str], model_tag: str) -> bool:
    norm = model_tag[:-7] if model_tag.endswith(":latest") else model_tag
    return any(m == model_tag or m == norm or m == f"{norm}:latest" for m in installed)


def get_default_model(
    ollama_ok: bool | None = None,
    ollama_models: list[str] | None = None,
) -> str:
    env_model = os.environ.get("TRANSLATOR_MODEL", "").strip()
    if env_model:
        return env_model
    cfg = load_config()
    cfg_model = str(cfg.get("default_model") or "").strip()
    if ollama_ok is None or ollama_models is None:
        ollama_ok, ollama_models = check_ollama_status()

    prov = str(cfg.get("api_provider") or "gemini").strip().lower()
    api_model = str(cfg.get("api_model") or API_PROVIDERS.get(prov, {}).get("default_model") or "").strip()
    has_key = bool(get_api_key(provider=prov, cfg=cfg))

    if cfg.get("engine_mode") == "api" and has_key:
        if cfg_model and not cfg_model.startswith("ollama:"):
            return cfg_model
        return format_cloud_model_id(prov, api_model)

    if cfg_model:
        if not cfg_model.startswith("ollama:") and not ollama_ok and has_key:
            return cfg_model
        elif cfg_model.startswith("ollama:") and ollama_ok:
            tag = cfg_model.removeprefix("ollama:")
            if _ollama_has_model(ollama_models, tag):
                return cfg_model

    if ollama_ok:
        for candidate in ("gemma4:12b", "gemma4:26b", "gemma4:e4b", "gemma4:e2b", "gemma4:31b", "gemma3:12b", "gemma3:4b"):
            if _ollama_has_model(ollama_models, candidate):
                return f"ollama:{candidate}"
        if ollama_models:
            return f"ollama:{ollama_models[0]}"
    if has_key:
        return format_cloud_model_id(prov, api_model)
    return cfg_model or "ollama:gemma4:12b"


def _ensure_swift_binary(
    source_path: Path, binary_path: Path, frameworks: list[str], label: str
) -> None:
    """Compile un utilitaire Swift natif si absent ou obsolète."""
    needs_build = not binary_path.exists()
    if not needs_build and source_path.exists():
        if source_path.stat().st_mtime > binary_path.stat().st_mtime:
            needs_build = True
    if not needs_build:
        return
    if not source_path.exists():
        raise RuntimeError(f"Source {label} introuvable : {source_path}")
    print(f"[{label}] Compilation du module natif Swift...", flush=True)
    cmd = ["swiftc", "-O"]
    for fw in frameworks:
        cmd.extend(["-framework", fw])
    cmd.extend([str(source_path), "-o", str(binary_path)])
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"Échec compilation {binary_path.name}:\n{proc.stderr}")
    print(f"[{label}] Compilation réussie.", flush=True)


def ensure_ocr_binary() -> None:
    _ensure_swift_binary(
        OCR_SOURCE, OCR_BINARY, ["Vision", "ImageIO", "CoreGraphics"], "OCR"
    )


def ensure_annotate_binary() -> None:
    _ensure_swift_binary(
        ANNOTATE_SOURCE, ANNOTATE_BINARY, ["AppKit", "ImageIO", "CoreGraphics"], "SoM"
    )


def _image_suffix(image_bytes: bytes) -> str:
    if image_bytes.startswith(b"\x89PNG"):
        return ".png"
    if image_bytes.startswith(b"\xff\xd8"):
        return ".jpg"
    if image_bytes[:4] == b"RIFF" and image_bytes[8:12] == b"WEBP":
        return ".webp"
    return ".img"


def create_som_image_base64(image_bytes: bytes, bubbles: list[dict], max_dim: int = 1120) -> str:
    """Dessine les boîtes numérotées (Set-of-Mark) sur l'image et retourne le JPEG base64."""
    ensure_annotate_binary()
    payload = [
        {"id": int(b["id"]), "x": float(b["x"]), "y": float(b["y"]), "w": float(b["w"]), "h": float(b["h"])}
        for b in bubbles
    ]
    tmp_files: list[str] = []
    try:
        with tempfile.NamedTemporaryFile(suffix=_image_suffix(image_bytes), delete=False) as f_img:
            f_img.write(image_bytes)
            img_path = f_img.name
            tmp_files.append(img_path)

        with tempfile.NamedTemporaryFile(suffix=".json", mode="w", encoding="utf-8", delete=False) as f_json:
            json.dump(payload, f_json)
            json_path = f_json.name
            tmp_files.append(json_path)

        with tempfile.NamedTemporaryFile(suffix=".jpg", delete=False) as f_out:
            out_path = f_out.name
            tmp_files.append(out_path)

        proc = subprocess.run(
            [str(ANNOTATE_BINARY), img_path, json_path, out_path, str(max_dim)],
            capture_output=True,
            text=True,
            timeout=20,
        )
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or "Erreur annotation Set-of-Mark")
        jpg_bytes = Path(out_path).read_bytes()
        return base64.b64encode(jpg_bytes).decode("ascii")
    finally:
        for p in tmp_files:
            try:
                os.unlink(p)
            except OSError:
                pass


def run_apple_vision_ocr(image_bytes: bytes, source_lang: str = "en") -> dict:
    """Exécute Apple Vision OCR sur une image et retourne les lignes détectées."""
    ensure_ocr_binary()
    suffix = _image_suffix(image_bytes)

    with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as tmp:
        tmp.write(image_bytes)
        tmp_path = tmp.name

    try:
        proc = subprocess.run(
            [str(OCR_BINARY), tmp_path, source_lang],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or "Erreur Apple Vision OCR")
        return json.loads(proc.stdout)
    finally:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass


def is_noise_line(line: dict, source_lang: str = "en") -> bool:
    text = line.get("text", "").strip()
    if not text:
        return True
    conf = float(line.get("confidence", 1.0))
    if conf < 0.22:
        return True
    in_closed_bubble = int(line.get("bubble_region_id", -1)) > 0
    white_ratio = float(line.get("white_ratio", 0.9))
    dark_ratio = float(line.get("dark_ratio", 0.0))
    # Garde les bulles courtes expressives (!, ?, ?!, ...)
    if len(text) == 1 and not re.match(r"[A-Za-z0-9!?…]|[\u3040-\u30ff\u4e00-\u9fff]", text):
        return True
    # En mode EN, ignore les traits purement non-latins, faux positifs OCR sur dessins et bandeaux verticaux
    if source_lang == "en":
        if len(text) == 1 and text.isalpha() and text not in ("I", "A"):
            return True
        if not re.search(r"[A-Za-z0-9!?]", text):
            return True
        if not in_closed_bubble and not re.search(r"[A-Za-z]", text) and not is_punctuation_only(text):
            return True
        if len(text) <= 6 and re.search(r"[÷±×=^|\\<>~`]", text) and len(re.findall(r"[A-Za-z]", text)) <= 2:
            return True
        if not in_closed_bubble and conf <= 0.35 and white_ratio < 0.65 and dark_ratio < 0.55:
            return True
        w = float(line.get("w", 0.0))
        h = float(line.get("h", 0.0))
        if h > 0.10 and (h / max(w, 1e-5)) > 2.4:
            return True
    # Filtre les minuscules numéros de page tout en bas de la planche
    if re.fullmatch(r"\d{1,3}", text) and line.get("y", 0) > 0.92 and line.get("h", 0) < 0.03:
        return True
    return False


def join_bubble_lines(lines: list[dict]) -> str:
    """Assemble proprement les lignes d'une même bulle en une phrase cohérente."""
    if not lines:
        return ""
    avg_h = sum(l["h"] for l in lines) / len(lines)
    # Trie d'abord par ligne visuelle (y), puis gauche -> droite sur la même ligne
    sorted_lines = sorted(lines, key=lambda l: (l["y"], l["x"]))
    rows: list[list[dict]] = []
    for ln in sorted_lines:
        cy = ln["y"] + ln["h"] / 2.0
        placed = False
        for row in rows:
            row_cy = sum(r["y"] + r["h"] / 2.0 for r in row) / len(row)
            if abs(cy - row_cy) <= avg_h * 0.5:
                row.append(ln)
                placed = True
                break
        if not placed:
            rows.append([ln])

    ordered_texts: list[str] = []
    for row in rows:
        row.sort(key=lambda l: l["x"])
        ordered_texts.append(" ".join(l["text"].strip() for l in row))

    result = ordered_texts[0]
    for nxt in ordered_texts[1:]:
        # Recolle les mots coupés par un tiret en fin de ligne (ex: "communi-" + "cate")
        if result.endswith("-") and not result.endswith("--") and nxt and nxt[0].isalpha():
            result = result[:-1] + nxt
        else:
            result = result + " " + nxt
    return re.sub(r"\s+", " ", result).strip()


def classify_bubble_style(lines: list[dict], bw: float, bh: float) -> str:
    """Détermine le style visuel naturel de la bulle à partir des pixels réels."""
    if not lines:
        return "bubble_light"
    avg_white = sum(float(l.get("white_ratio", 0.9)) for l in lines) / len(lines)
    avg_dark = sum(float(l.get("dark_ratio", 0.0)) for l in lines) / len(lines)
    max_white = max(float(l.get("white_ratio", 0.9)) for l in lines)

    if avg_dark >= 0.56 and avg_white < 0.25:
        return "bubble_dark"
    # Grand titre / texte isolé posé directement sur le dessin
    if len(lines) == 1 and bh >= 0.045 and avg_white < 0.68:
        return "floating"
    if avg_white >= 0.54 or (len(lines) >= 2 and max_white >= 0.72 and avg_white >= 0.48):
        return "bubble_light"
    return "floating"


def resolve_bubble_overlaps(bubbles: list[dict], gap: float = 0.004) -> None:
    """Résout toute collision entre boîtes de bulles pour qu'elles ne se chevauchent jamais."""
    n = len(bubbles)
    if n < 2:
        return

    min_size = 0.022
    for _ in range(3):
        changed = False
        for i in range(n):
            a = bubbles[i]
            for j in range(i + 1, n):
                b = bubbles[j]
                ax1, ay1 = a["x"], a["y"]
                ax2, ay2 = ax1 + a["w"], ay1 + a["h"]
                bx1, by1 = b["x"], b["y"]
                bx2, by2 = bx1 + b["w"], by1 + b["h"]

                if not (ax1 < bx2 and bx1 < ax2 and ay1 < by2 and by1 < ay2):
                    continue

                ov_x = min(ax2, bx2) - max(ax1, bx1)
                ov_y = min(ay2, by2) - max(ay1, by1)
                norm_x = ov_x / max(min(a["w"], b["w"]), 1e-5)
                norm_y = ov_y / max(min(a["h"], b["h"]), 1e-5)

                if norm_x <= norm_y:
                    left_b, right_b = (a, b) if (ax1 + ax2) <= (bx1 + bx2) else (b, a)
                    lx1, lx2 = left_b["x"], left_b["x"] + left_b["w"]
                    rx1, rx2 = right_b["x"], right_b["x"] + right_b["w"]
                    mid = (lx2 + rx1) * 0.5
                    # Garde les deux boîtes de part et d'autre de mid avec une marge gap
                    new_lw = max(min_size, mid - gap * 0.5 - lx1)
                    if lx1 + new_lw + gap * 0.5 > rx2 - min_size:
                        # Trop serré pour tenir avec gap : split minimal garanti sans chevauchement
                        split = max(lx1 + min_size * 0.5, min(rx2 - min_size * 0.5, mid))
                        left_b["w"] = max(0.01, split - lx1)
                        right_b["x"] = split
                        right_b["w"] = max(0.01, rx2 - split)
                    else:
                        left_b["w"] = new_lw
                        new_rx = lx1 + new_lw + gap
                        right_b["x"] = new_rx
                        right_b["w"] = max(min_size, rx2 - new_rx)
                else:
                    top_b, bot_b = (a, b) if (ay1 + ay2) <= (by1 + by2) else (b, a)
                    ty1, ty2 = top_b["y"], top_b["y"] + top_b["h"]
                    by1_c, by2_c = bot_b["y"], bot_b["y"] + bot_b["h"]
                    mid = (ty2 + by1_c) * 0.5
                    new_th = max(min_size, mid - gap * 0.5 - ty1)
                    if ty1 + new_th + gap * 0.5 > by2_c - min_size:
                        split = max(ty1 + min_size * 0.5, min(by2_c - min_size * 0.5, mid))
                        top_b["h"] = max(0.01, split - ty1)
                        bot_b["y"] = split
                        bot_b["h"] = max(0.01, by2_c - split)
                    else:
                        top_b["h"] = new_th
                        new_by = ty1 + new_th + gap
                        bot_b["y"] = new_by
                        bot_b["h"] = max(min_size, by2_c - new_by)
                changed = True
        if not changed:
            break

    for b in bubbles:
        b["x"] = round(max(0.0, min(0.99, b["x"])), 5)
        b["y"] = round(max(0.0, min(0.99, b["y"])), 5)
        b["w"] = round(max(0.01, min(1.0 - b["x"], b["w"])), 5)
        b["h"] = round(max(0.01, min(1.0 - b["y"], b["h"])), 5)


def group_lines_into_bubbles(ocr_lines: list[dict], source_lang: str = "en") -> list[dict]:
    """Regroupe les lignes OCR (hybride : contours de bulle fermés + proximité)."""
    clean = [l for l in ocr_lines if not is_noise_line(l, source_lang=source_lang)]
    n = len(clean)
    if n == 0:
        return []

    parent = list(range(n))

    def find(i: int) -> int:
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    def union(i: int, j: int) -> None:
        ri, rj = find(i), find(j)
        if ri != rj:
            parent[ri] = rj

    for i in range(n):
        a = clean[i]
        ax1, ay1 = a["x"], a["y"]
        ax2, ay2 = ax1 + a["w"], ay1 + a["h"]
        a_rid = int(a.get("bubble_region_id", -1))
        for j in range(i + 1, n):
            b = clean[j]
            bx1, by1 = b["x"], b["y"]
            bx2, by2 = bx1 + b["w"], by1 + b["h"]
            b_rid = int(b.get("bubble_region_id", -1))

            lh = min(a["h"], b["h"])
            max_h = max(a["h"], b["h"])
            if lh <= 0:
                continue
            size_ratio = max_h / lh

            # Barrière dure : deux bulles fermées distinctes séparées par de l'encre noire
            if a_rid > 0 and b_rid > 0 and a_rid != b_rid:
                continue

            # Évite de fusionner deux bulles de styles opposés (ex: bulle noire et bulle blanche)
            a_dark = float(a.get("dark_ratio", 0.0)) >= 0.58 and float(a.get("white_ratio", 1.0)) < 0.25
            b_dark = float(b.get("dark_ratio", 0.0)) >= 0.58 and float(b.get("white_ratio", 1.0)) < 0.25
            if a_dark != b_dark:
                continue

            v_gap = max(0.0, max(ay1, by1) - min(ay2, by2))
            h_gap = max(0.0, max(ax1, bx1) - min(ax2, bx2))
            h_overlap = min(ax2, bx2) - max(ax1, bx1)
            v_overlap = min(ay2, by2) - max(ay1, by1)
            min_w = min(a["w"], b["w"])
            avg_h = (a["h"] + b["h"]) / 2.0

            if a_rid > 0 and a_rid == b_rid:
                # Même bulle fermée : seuils généreux en vertical, tout en gardant
                # les double-bulles collées côte à côte ("cacahuète") séparées.
                vertically_stacked = (
                    size_ratio <= 2.8
                    and v_gap <= avg_h * 2.0
                    and v_overlap <= lh * 0.45
                    and h_overlap >= min_w * 0.12
                )
                same_row = (
                    size_ratio <= 1.7
                    and v_overlap >= lh * 0.52
                    and h_gap <= lh * 0.26
                )
            else:
                # Secours pour bulles ouvertes ou textes flottants
                vertically_stacked = (
                    size_ratio <= 2.6
                    and v_gap <= avg_h * 1.35
                    and v_overlap <= lh * 0.42
                    and h_overlap >= min_w * 0.28
                )
                same_row = (
                    size_ratio <= 1.6
                    and v_overlap >= lh * 0.58
                    and h_gap <= lh * 0.28
                )

            if vertically_stacked or same_row:
                union(i, j)

    clusters: dict[int, list[dict]] = {}
    for idx, line in enumerate(clean):
        clusters.setdefault(find(idx), []).append(line)

    provisional: list[dict] = []
    region_cluster_counts: dict[int, int] = {}

    for lines in clusters.values():
        x1 = min(l["x"] for l in lines)
        y1 = min(l["y"] for l in lines)
        x2 = max(l["x"] + l["w"] for l in lines)
        y2 = max(l["y"] + l["h"] for l in lines)

        pad_x = 0.006
        pad_y = 0.005
        bx = max(0.0, x1 - pad_x)
        by = max(0.0, y1 - pad_y)
        bw = min(1.0 - bx, (x2 - x1) + 2 * pad_x)
        bh = min(1.0 - by, (y2 - y1) + 2 * pad_y)

        text = join_bubble_lines(lines)
        if not text:
            continue

        style = classify_bubble_style(lines, bw, bh)
        rids = {int(l.get("bubble_region_id", -1)) for l in lines}
        unique_rid = next(iter(rids)) if len(rids) == 1 and next(iter(rids)) > 0 else -1
        if unique_rid > 0:
            region_cluster_counts[unique_rid] = region_cluster_counts.get(unique_rid, 0) + 1

        provisional.append(
            {
                "lines": lines,
                "text": text,
                "bx": bx,
                "by": by,
                "bw": bw,
                "bh": bh,
                "style": style,
                "unique_rid": unique_rid,
            }
        )

    raw_boxes = [
        (item["bx"], item["by"], item["bx"] + item["bw"], item["by"] + item["bh"])
        for item in provisional
    ]

    bubbles: list[dict] = []
    for idx, item in enumerate(provisional):
        bx, by, bw, bh = item["bx"], item["by"], item["bw"], item["bh"]
        rid = item["unique_rid"]
        style = item["style"]

        # Étend doucement la boîte vers l'intérieur détecté de la bulle fermée
        # uniquement si la bulle n'est pas partagée et sans empiéter sur le voisin
        if rid > 0 and region_cluster_counts.get(rid, 0) == 1 and style == "bubble_light":
            ref = item["lines"][0]
            rx = ref.get("region_x")
            ry = ref.get("region_y")
            rw = ref.get("region_w")
            rh = ref.get("region_h")
            if rx is not None and ry is not None and rw is not None and rh is not None:
                tx1, ty1 = bx, by
                tx2, ty2 = bx + bw, by + bh
                text_w = max(0.01, tx2 - tx1)
                text_h = max(0.01, ty2 - ty1)

                ux1 = min(tx1, float(rx))
                uy1 = min(ty1, float(ry))
                ux2 = max(tx2, float(rx) + float(rw))
                uy2 = max(ty2, float(ry) + float(rh))

                max_w = text_w * 1.55
                max_h = text_h * 1.45

                if ux2 - ux1 > max_w:
                    extra_w = (max_w - text_w) * 0.5
                    ux1 = max(ux1, tx1 - extra_w)
                    ux2 = min(ux2, tx2 + extra_w)
                if uy2 - uy1 > max_h:
                    extra_h = (max_h - text_h) * 0.5
                    uy1 = max(uy1, ty1 - extra_h)
                    uy2 = min(uy2, ty2 + extra_h)

                # Clamp par rapport aux boîtes voisines pour ne jamais les couvrir
                for j, (ox1, oy1, ox2, oy2) in enumerate(raw_boxes):
                    if j == idx:
                        continue
                    if min(uy2, oy2) > max(uy1, oy1):
                        if ox1 >= tx2 - 0.005:
                            ux2 = min(ux2, ox1 - 0.004)
                        if ox2 <= tx1 + 0.005:
                            ux1 = max(ux1, ox2 + 0.004)
                    if min(ux2, ox2) > max(ux1, ox1):
                        if oy1 >= ty2 - 0.005:
                            uy2 = min(uy2, oy1 - 0.004)
                        if oy2 <= ty1 + 0.005:
                            uy1 = max(uy1, oy2 + 0.004)

                ux1 = min(ux1, tx1)
                ux2 = max(ux2, tx2)
                uy1 = min(uy1, ty1)
                uy2 = max(uy2, ty2)

                bx = max(0.0, ux1)
                by = max(0.0, uy1)
                bw = min(1.0 - bx, ux2 - ux1)
                bh = min(1.0 - by, uy2 - uy1)

        min_conf = min(float(l.get("confidence", 1.0)) for l in item["lines"])
        bubbles.append(
            {
                "text": item["text"],
                "x": round(bx, 5),
                "y": round(by, 5),
                "w": round(bw, 5),
                "h": round(bh, 5),
                "line_count": len(item["lines"]),
                "min_conf": round(min_conf, 3),
                "style": style,
            }
        )

    resolve_bubble_overlaps(bubbles, gap=0.004)

    # Ordre de lecture manga approximatif (bandes haut->bas, droite->gauche)
    bubbles.sort(key=lambda b: (int(b["y"] / 0.22), -(b["x"] + b["w"] / 2.0)))
    for idx, b in enumerate(bubbles):
        b["id"] = idx
    return bubbles


def _repair_json_text(text: str) -> str:
    """Corrige les fautes JSON courantes des LLM (virgules en trop, clés connues non quotées)."""
    repaired = re.sub(r",\s*([}\]])", r"\1", text)
    repaired = re.sub(r'([{\[,]\s*)(translations|id|who|fr)(\s*:)', r'\1"\2"\3', repaired)
    return repaired


def _unescape_json_str(raw: str) -> str:
    try:
        return json.loads(f'"{raw}"', strict=False)
    except Exception:
        return raw.replace('\\"', '"').replace("\\n", "\n").replace("\\t", "\t").strip()


_SKIP_WHO_TOKENS = frozenset({"ignore", "ignorer", "skip", "dessin", "art", "bruit", "ocr noise"})


def _is_skip_item(skip_raw, fr_text: str, who_text: str) -> bool:
    if isinstance(skip_raw, bool):
        if skip_raw:
            return True
    elif str(skip_raw or "").strip().lower() in ("true", "1", "yes", "oui"):
        return True
    return not fr_text and who_text.strip().lower() in _SKIP_WHO_TOKENS


def can_skip_bubble(bubble: dict) -> bool:
    """Empêche d'ignorer par erreur une vraie bulle de dialogue anglaise bien lisible."""
    style = str(bubble.get("style", ""))
    min_conf = float(bubble.get("min_conf", 1.0))
    real_words = re.findall(r"[A-Za-z]{2,}", str(bubble.get("text", "")))
    if style in ("bubble_light", "bubble_dark") and min_conf >= 0.85 and len(real_words) >= 4:
        return False
    return True


def _parse_translation_items(raw_list) -> dict[int, dict]:
    out: dict[int, dict] = {}
    if not isinstance(raw_list, list):
        return out
    for item in raw_list:
        if not isinstance(item, dict) or "id" not in item:
            continue
        try:
            bid = int(item["id"])
        except (TypeError, ValueError):
            continue
        fr_text = str(item.get("fr") or item.get("tr") or "").strip()
        who_text = str(item.get("who", "")).strip()
        is_skip = _is_skip_item(item.get("skip"), fr_text, who_text)
        entry: dict = {"fr": "" if is_skip else fr_text, "who": who_text}
        if is_skip:
            entry["skip"] = True
        out[bid] = entry
    return out


def _extract_quoted_field(frag: str, field_name: str) -> str | None:
    boundary = r'(?=\s*(?:[,}]|"?(?:id|who|fr|skip)"?\s*:))'
    strict_m = re.search(
        rf'(?:"{field_name}"|\b{field_name}\b)\s*:\s*"((?:\\.|[^"\\])*)"{boundary}',
        frag,
        flags=re.DOTALL,
    )
    if strict_m:
        return _unescape_json_str(strict_m.group(1)).strip()
    loose_m = re.search(
        rf'(?:"{field_name}"|\b{field_name}\b)\s*:\s*"((?:\\.|[^\\])*?)"{boundary}',
        frag,
        flags=re.DOTALL,
    )
    return _unescape_json_str(loose_m.group(1)).strip() if loose_m else None


def _recover_translation_items(raw_text: str) -> list[dict]:
    """Récupère les objets {id, who, fr[, skip]} valides même si le JSON global est cassé ou tronqué."""
    recovered: list[dict] = []
    for match in re.finditer(r"\{[^{}]*\}", raw_text):
        frag = match.group(0)
        repaired_frag = _repair_json_text(frag)
        try:
            obj = json.loads(repaired_frag, strict=False)
            if isinstance(obj, dict) and "id" in obj:
                fr_s = str(obj.get("fr", "")).strip()
                who_s = str(obj.get("who", "")).strip()
                skip_b = _is_skip_item(obj.get("skip"), fr_s, who_s)
                if "fr" in obj or skip_b:
                    entry: dict = {"id": int(obj["id"]), "who": who_s, "fr": "" if skip_b else fr_s}
                    if skip_b:
                        entry["skip"] = True
                    recovered.append(entry)
                    continue
        except (ValueError, TypeError, json.JSONDecodeError):
            pass

        id_m = re.search(r'(?:"id"|\bid\b)\s*:\s*(\d+)', frag)
        fr_val = _extract_quoted_field(frag, "fr")
        who_val = _extract_quoted_field(frag, "who") or ""
        skip_m = re.search(r'(?:"skip"|\bskip\b)\s*:\s*(true|false)', frag, flags=re.IGNORECASE)
        skip_raw = skip_m.group(1) if skip_m else None
        skip_b = _is_skip_item(skip_raw, fr_val or "", who_val)
        if not id_m or (fr_val is None and not skip_b):
            continue
        entry = {
            "id": int(id_m.group(1)),
            "who": who_val,
            "fr": "" if skip_b else (fr_val or ""),
        }
        if skip_b:
            entry["skip"] = True
        recovered.append(entry)
    return recovered


def extract_json_object(raw_text: str) -> dict:
    """Extrait proprement le premier objet JSON valide d'une réponse texte/markdown, avec réparation tolérante."""
    cleaned = re.sub(r"^```(?:json)?\s*|\s*```$", "", raw_text.strip(), flags=re.DOTALL).strip()
    try:
        parsed = json.loads(cleaned, strict=False)
        if isinstance(parsed, dict):
            return parsed
    except json.JSONDecodeError as first_err:
        initial_err = first_err
    else:
        initial_err = json.JSONDecodeError("Objet JSON attendu", cleaned, 0)

    start = cleaned.find("{")
    end = cleaned.rfind("}")
    candidate = cleaned if start == -1 else (cleaned[start : end + 1] if end > start else cleaned[start:])

    for attempt_text in (candidate, _repair_json_text(candidate)):
        try:
            parsed = json.loads(attempt_text, strict=False)
            if isinstance(parsed, dict):
                return parsed
        except json.JSONDecodeError:
            pass

    recovered = _recover_translation_items(cleaned)
    if recovered:
        return {"translations": recovered}
    raise initial_err


def build_manga_system_prompt(
    source_lang: str = "en",
    use_vision: bool = False,
    glossary: dict | None = None,
    target_lang: str = "fr",
) -> str:
    src_label = {
        "en": "anglais",
        "ja": "japonais",
        "auto": "anglais ou japonais",
    }.get(source_lang, source_lang)
    tgt_code = (target_lang or "fr").strip().lower()
    tgt_name = TARGET_LANGUAGES.get(tgt_code, tgt_code.upper())

    vision_block = ""
    if use_vision:
        vision_block = (
            "\nVISION (IMAGE ANNOTÉE FOURNIE) :\n"
            "- Regarde attentivement la planche de manga jointe : chaque bulle est encadrée en rouge avec son numéro d'id (0, 1, 2...).\n"
            "- Identifie qui parle à qui (fille/garçon -> accords grammaticaux exacts ; ado/ami -> tutoiement ; adulte/inconnu -> vouvoiement) et observe l'expression des visages (gêne, ironie, panique, cri).\n"
            "- Relis directement le texte dans chaque cadre rouge sur l'image pour corriger toute erreur d'OCR !\n"
            "- Faux positifs OCR : si un cadre rouge recouvre en réalité du DESSIN pur ou une ONOMATOPÉE japonaise (katakana/hiragana) mal lue par l'OCR en chiffres/lettres parasites (ex. フン lu '7%', ポカポカ lu 'TitT', D2: Drn), renvoie {\"id\": ..., \"who\": \"ignore\", \"fr\": \"\", \"skip\": true} afin de ne rien afficher par-dessus le dessin.\n"
            "- Ne mets JAMAIS \"skip\": true sur une vraie bulle de dialogue, pensée, encadré narratif ou vraie note/onomatopée en anglais (ex. '*BUTTER KNIFE.', 'MAMA!!', 'PHEW', 'Gloom').\n"
        )

    glossary_block = format_glossary_for_prompt(glossary)

    if tgt_code != "fr":
        src_en = {"en": "English", "ja": "Japanese", "auto": "English or Japanese"}.get(source_lang, source_lang)
        vis_en = (
            "\nVISION (ANNOTATED PAGE PROVIDED):\n"
            "- Inspect the attached manga page: each bubble is boxed in red with its id number.\n"
            "- Infer speaker gender, age, register, and facial expression, and fix minor OCR glitches by reading the red boxes directly.\n"
            "- OCR false positives: if a red box covers pure artwork or misread Japanese SFX, return {\"id\": ..., \"who\": \"ignore\", \"fr\": \"\", \"skip\": true}.\n"
            "- Never skip real dialogue, narration, or English SFX/notes.\n"
            if use_vision
            else ""
        )
        return (
            f"You are a professional manga localizer ({src_en} -> {tgt_name}).\n"
            f"Read ALL bubbles first to understand the scene before translating them into natural, idiomatic spoken {tgt_name}.\n"
            f"{vis_en}\n"
            "RULES:\n"
            f"1. Write natural spoken {tgt_name} dialogue (never literal word-for-word). Match character tone and pronouns (HE/SHE).\n"
            "2. Join split sentences smoothly across consecutive ids.\n"
            "3. Keep proper nouns and Japanese terms like Golden Week, senpai, bento, ramen, sensei when appropriate.\n"
            "4. Fill `who` (2-4 words) first, then write the translated dialogue inside the JSON field named `fr`.\n"
            "5. Translate ALL received ids (unless visually skipped in Vision mode). Return ONLY valid JSON: "
            '{"translations": [{"id": 0, "who": "...", "fr": "..."}]}\n\n'
            f"{glossary_block}"
        )

    return (
        f"Tu es un adaptateur professionnel de manga ({src_label} -> français), dans le style des éditions françaises (Pika, Kurokawa, Ki-oon).\n"
        "Tu reçois les bulles d'un manga dans l'ordre de lecture (droite à gauche). Lis d'abord TOUTES les bulles pour saisir la scène et qui parle avant de traduire.\n"
        f"{vision_block}\n"
        "RÈGLES D'ADAPTATION :\n"
        "1. Écris un français ORAL, NATUREL et IDIOMATIQUE (comme parlent vraiment des lycéens ou une famille en France). Ne traduis JAMAIS mot à mot et évite les calques de l'anglais.\n"
        "2. Phrases coupées sur 2 bulles : si deux id qui se suivent forment clairement une seule phrase coupée sur deux bulles, traduis l'ensemble de manière fluide en répartissant naturellement la phrase française entre les deux id.\n"
        "3. Casse normale : l'OCR est souvent en MAJUSCULES ; écris en minuscules françaises normales (majuscule en début de phrase uniquement, sauf cris/titres).\n"
        "4. Concision : reste court et percutant pour que le texte tienne dans une bulle de manga.\n"
        "5. Correction OCR : si un mot source est légèrement abîmé par l'OCR (ex. 'KOM' -> 'Komi', 'DEMONS' -> 'démoniaque'), corrige-le grâce au contexte.\n"
        "6. Analyse qui parle et le ton : pour chaque bulle, remplis d'abord `who` en 2 à 4 mots (ex. \"Komi, gênée\", \"Mère de Komi, polie\", \"Narrateur, ironique\", \"Katai, sérieux\") afin de choisir le bon genre (masculin/féminin) et le bon registre (tu/vous), puis écris la réplique française naturelle dans `fr`.\n"
        "7. Traduis absolument TOUS les id reçus sans en sauter aucun (y compris les notes *... et petits textes ; sauf faux positif visuel marqué `\"skip\": true` en mode Vision). "
        "Dans le texte français de `fr`, utilise des guillemets français `« ... »` (jamais de guillemets droits `\"` non échappés). "
        'Réponds UNIQUEMENT avec un objet JSON valide (sans texte avant ni après) de la forme exacte : '
        '{"translations": [{"id": 0, "who": "...", "fr": "..."}]}\n\n'
        "CONVENTIONS MANGA VF :\n"
        "- Termes japonais conservés tels quels : Golden Week (JAMAIS 'Semaine d'or'), senpai, kôhai, bento, ramen, onsen, konbini, matsuri, sensei.\n"
        "- Tutoiement par défaut : en manga, tutoie TOUJOURS par défaut (`tu`, `t'`, `te`, `s'il te plaît`, `arrête`, `regarde`). N'utilise « vous » que si un personnage parle clairement à un professeur, un inconnu âgé ou un groupe (pluriel) ; en cas de doute, utilise « tu » ! Supprime -san/-kun/-chan quand c'est plus fluide en français.\n"
        "- Pronoms narratifs : respecte STRICTEMENT `HE` = il / `SHE` = elle (ex. `\"WHAT A CUTIE,\" SHE THOUGHT.` -> `« Qu'elle est mignonne... » pensa-t-elle.`, JAMAIS `pensa-t-il`).\n\n"
        f"{glossary_block}"
        "EXEMPLES DE BONNE ADAPTATION (à suivre) :\n"
        '- "WAH, YOU SCARED ME!!" -> "Ouah, tu m\'as fait flipper !!" (PAS "Vous m\'avez fait sursauter")\n'
        '- "STOP! PLEASE, STOP!" -> "Arrête ! S\'il te plaît, arrête !" (PAS "Arrêtez ! S\'il vous plaît")\n'
        '- \'"WHAT A CUTIE," SHE THOUGHT.\' -> \'« Qu\\\'elle est mignonne... » pensa-t-elle.\' (respect strict de SHE -> elle)\n'
        '- \'"THAT WAS HER OWN DAMN FAULT FOR BLASTING "KITANO MIKI" THROUGH MY HOOD AT NIGHT."\' -> "C\'est entièrement sa faute si elle balançait « Kitano Miki » à fond dans mon quartier en pleine nuit." (PAS "dans mon capot")\n'
        '- "OUR BOSS IS STILL BAWLING OVER HOW YOU BROKE HER BELOVED BIKE!!" -> "Notre cheffe pleurniche encore parce que t\'as bousillé sa précieuse moto !!"\n'
        '- "*BUTTER KNIFE." -> "*Couteau à beurre."\n'
        '- "OH DEARIE ME, YOU\'RE SO NERVOUS." -> "Oh ma chérie, tu es toute nerveuse."\n'
        '- "GOLDEN WEEK!" -> "La Golden Week !" (PAS "Semaine d\'or !")\n'
        '- "WAIT, I DON\'T WANT TO BE SEEN GETTING ALL HYPED WITH MY FAMILY!" -> "Hors de question qu\'on me voie tout excité avec ma famille !" (PAS "Attends, je ne veux pas être vu devenir tout excité...")\n'
        '- "SO THEY BECOME LOW-SPIRITED!" -> "Du coup, ils font la tête !" (PAS "Donc ils deviennent bas d\'esprit !")\n'
        '- "ARE YOU FOR REAL?!" -> "Tu rigoles ?!" (PAS "Es-tu pour vrai ?!")\n'
        '- "I\'M GONNA PASS OUT..." -> "Je vais tourner de l\'œil..." (PAS "Je vais passer dehors...")'
    )


def build_user_prompt(
    payload_items: list[dict],
    context_history: list[str] | None = None,
    use_vision: bool = False,
    target_lang: str = "fr",
) -> str:
    tgt_code = (target_lang or "fr").strip().lower()
    tgt_name = TARGET_LANGUAGES.get(tgt_code, tgt_code.upper())
    clean_ctx: list[str] = []
    if context_history:
        for line in context_history:
            txt = str(line or "").strip()
            if txt:
                clean_ctx.append(txt[:160])
        clean_ctx = clean_ctx[-8:]

    parts: list[str] = []
    if clean_ctx:
        ctx_lines = "\n".join(f"- {c}" for c in clean_ctx)
        parts.append(
            "CONTEXTE DES PAGES PRÉCÉDENTES (pour garder le même ton, genre et tu/vous — NE PAS retraduire) :\n"
            + ctx_lines
            + "\n"
        )

    if tgt_code == "fr":
        if use_vision:
            parts.append(
                "Traduis en français les bulles numérotées dans l'image jointe (texte OCR indicatif ci-dessous) :\n"
            )
        else:
            parts.append("Traduis ces bulles de manga en français :\n")
    else:
        if use_vision:
            parts.append(
                f"Translate into {tgt_name} the numbered bubbles in the attached image (OCR text below for reference):\n"
            )
        else:
            parts.append(f"Translate these manga bubbles into {tgt_name}:\n")

    parts.append(json.dumps(payload_items, ensure_ascii=False))
    return "".join(parts)


def translate_bubbles_with_ollama(
    bubbles: list[dict],
    model: str,
    source_lang: str = "en",
    target_lang: str = "fr",
    context_history: list[str] | None = None,
    som_image_b64: str | None = None,
    glossary: dict | None = None,
) -> dict[int, dict]:
    """Traduit les bulles 100 % en local via Ollama (ex: gemma4:12b) en JSON structuré."""
    if not bubbles:
        return {}

    ollama_model = model.removeprefix("ollama:").strip() or "gemma4:12b"
    use_vision = bool(som_image_b64 and model_supports_vision(model))
    system_prompt = build_manga_system_prompt(
        source_lang, use_vision=use_vision, glossary=glossary, target_lang=target_lang
    )
    payload_items = [{"id": b["id"], "text": b["text"]} for b in bubbles]
    user_text = build_user_prompt(
        payload_items, context_history=context_history, use_vision=use_vision, target_lang=target_lang
    )

    user_msg: dict = {"role": "user", "content": user_text}
    if use_vision and som_image_b64:
        user_msg["images"] = [som_image_b64]

    body: dict = {
        "model": ollama_model,
        "stream": False,
        "keep_alive": "10m",
        "options": {"temperature": 0.45, "top_p": 0.9, "num_ctx": 4096},
        "messages": [
            {"role": "system", "content": system_prompt},
            user_msg,
        ],
    }
    # Désactive le mode réflexion caché sur Gemma 4 et évite le paramètre format
    # qui déclenche HTTP 501 ("structured output is unavailable") sur Ollama 0.40.16
    if ollama_model.startswith("gemma4"):
        body["think"] = False
    else:
        body["format"] = {
            "type": "object",
            "properties": {
                "translations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "integer"},
                            "who": {"type": "string"},
                            "fr": {"type": "string"},
                            "skip": {"type": "boolean"},
                        },
                        "required": ["id", "who", "fr"],
                    },
                }
            },
            "required": ["translations"],
        }

    def _post_ollama_chat(payload_dict: dict) -> dict:
        req = urllib.request.Request(
            f"{OLLAMA_URL}/api/chat",
            data=json.dumps(payload_dict).encode("utf-8"),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req, timeout=180) as resp:
            return json.loads(resp.read().decode("utf-8"))

    def _read_http_error(err: urllib.error.HTTPError) -> str:
        try:
            return err.read().decode("utf-8", errors="replace")
        except Exception:
            return ""

    def _send_ollama_request(payload_dict: dict) -> dict:
        try:
            return _post_ollama_chat(payload_dict)
        except urllib.error.HTTPError as e:
            err_body = _read_http_error(e)
            if e.code == 501 and "format" in payload_dict and "structured output" in err_body.lower():
                payload_dict.pop("format", None)
                body.pop("format", None)
                try:
                    return _post_ollama_chat(payload_dict)
                except urllib.error.HTTPError as retry_err:
                    retry_body = _read_http_error(retry_err)
                    raise RuntimeError(f"Erreur Ollama ({retry_err.code}) : {retry_body[:240]}")
            if e.code == 404 or "not found" in err_body.lower():
                raise RuntimeError(
                    f"Modèle local '{ollama_model}' non téléchargé. "
                    f"Lance dans le Terminal : ollama pull {ollama_model}"
                )
            raise RuntimeError(f"Erreur Ollama ({e.code}) : {err_body[:240]}")
        except urllib.error.URLError:
            raise RuntimeError(
                "Ollama local ne répond pas (port 11434). "
                "Relance start.command ou lance 'ollama serve' dans le Terminal."
            )

    translations: dict[int, dict] = {}
    last_json_err: json.JSONDecodeError | None = None
    for attempt in range(2):
        if attempt == 0:
            req_body = body
        else:
            strict_prompt = (
                system_prompt
                + "\nIMPORTANT : Retourne UNIQUEMENT un JSON strictement valide "
                "(guillemets doubles obligatoires pour toutes les clés, aucune virgule en trop avant } ou ])."
            )
            req_body = {
                **body,
                "options": {**body.get("options", {}), "temperature": 0.15},
                "messages": [
                    {"role": "system", "content": strict_prompt},
                    user_msg,
                ],
            }

        raw = _send_ollama_request(req_body)
        content = raw.get("message", {}).get("content", "").strip()
        if not content:
            raise RuntimeError("Ollama n'a renvoyé aucun texte.")

        try:
            parsed = extract_json_object(content)
        except json.JSONDecodeError as e:
            last_json_err = e
            if attempt == 0:
                continue
            raise

        translations = _parse_translation_items(
            parsed.get("translations") if isinstance(parsed, dict) else None
        )
        if translations:
            break
        if attempt == 0:
            continue
        raise RuntimeError("Ollama n'a renvoyé aucune traduction exploitable.")

    if not translations:
        if last_json_err is not None:
            raise last_json_err
        raise RuntimeError("Ollama n'a renvoyé aucune traduction exploitable.")

    # Passe de rattrapage rapide (texte seul) si certaines bulles ont été sautées
    def _is_handled(b: dict, tr: dict | None) -> bool:
        if not tr:
            return False
        if tr.get("skip") and can_skip_bubble(b):
            return True
        return bool(str(tr.get("fr", "")).strip())

    missing_bubbles = [
        b for b in bubbles if not _is_handled(b, translations.get(b["id"]))
    ]
    if missing_bubbles:
        try:
            page_ctx = list(context_history or [])
            for b in bubbles:
                tr = translations.get(b["id"])
                if tr and not tr.get("skip") and str(tr.get("fr", "")).strip():
                    page_ctx.append(str(tr["fr"]).strip())
            recovery_items = [{"id": b["id"], "text": b["text"]} for b in missing_bubbles]
            recovery_sys = build_manga_system_prompt(
                source_lang, use_vision=False, glossary=glossary, target_lang=target_lang
            )
            recovery_user = build_user_prompt(
                recovery_items, context_history=page_ctx[-8:], use_vision=False, target_lang=target_lang
            )
            recovery_body = {
                **body,
                "options": {**body.get("options", {}), "temperature": 0.2},
                "messages": [
                    {"role": "system", "content": recovery_sys},
                    {"role": "user", "content": recovery_user},
                ],
            }
            rec_raw = _send_ollama_request(recovery_body)
            rec_content = rec_raw.get("message", {}).get("content", "").strip()
            if rec_content:
                rec_parsed = extract_json_object(rec_content)
                for bid, item in _parse_translation_items(
                    rec_parsed.get("translations") if isinstance(rec_parsed, dict) else None
                ).items():
                    if item["fr"] or item.get("skip"):
                        translations[bid] = item
        except Exception as rec_err:
            print(f"[Ollama rattrapage] Échec partiel ignoré : {rec_err}", flush=True)

    return translations


def _parse_retry_after_seconds(headers, err_body: str) -> int:
    if headers is not None:
        raw_hdr = str(headers.get("Retry-After") or "").strip()
        if raw_hdr:
            try:
                return max(5, min(90, int(float(raw_hdr)) + 1))
            except ValueError:
                pass
    m = re.search(r"retry\s+(?:in|after)\s+(\d+(?:\.\d+)?)\s*s", err_body, re.IGNORECASE)
    if not m:
        m = re.search(r'"retryDelay"\s*:\s*"(\d+(?:\.\d+)?)s"', err_body)
    if m:
        return max(5, min(90, int(float(m.group(1))) + 2))
    return 15


def translate_bubbles_with_openai_compat(
    bubbles: list[dict],
    provider: str,
    model_id: str,
    api_key: str,
    base_url: str,
    source_lang: str = "en",
    target_lang: str = "fr",
    context_history: list[str] | None = None,
    som_image_b64: str | None = None,
    glossary: dict | None = None,
) -> dict[int, dict]:
    """Traduit les bulles via n'importe quel fournisseur compatible OpenAI (/chat/completions)."""
    if not bubbles:
        return {}

    pinfo = API_PROVIDERS.get(provider, API_PROVIDERS["custom"])
    label = pinfo["label"]
    use_vision = bool(som_image_b64 and model_supports_vision(f"{provider}:{model_id}"))
    include_rf = True

    def _build_body(with_rf: bool, with_img: bool) -> dict:
        sys_prompt = build_manga_system_prompt(
            source_lang, use_vision=with_img, glossary=glossary, target_lang=target_lang
        )
        items = [{"id": b["id"], "text": b["text"]} for b in bubbles]
        u_text = build_user_prompt(
            items, context_history=context_history, use_vision=with_img, target_lang=target_lang
        )
        if with_img and som_image_b64:
            user_content: object = [
                {"type": "text", "text": u_text},
                {"type": "image_url", "image_url": {"url": f"data:image/jpeg;base64,{som_image_b64}"}},
            ]
        else:
            user_content = u_text
        body: dict = {
            "model": model_id,
            "temperature": 0.45,
            "messages": [
                {"role": "system", "content": sys_prompt},
                {"role": "user", "content": user_content},
            ],
        }
        if with_rf:
            body["response_format"] = {"type": "json_object"}
        return body

    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {api_key}",
    }
    if provider == "openrouter":
        headers["HTTP-Referer"] = "https://github.com/yomi"
        headers["X-Title"] = "Yomi"

    url = f"{base_url.rstrip('/')}/chat/completions"
    raw = None
    for _attempt in range(3):
        body = _build_body(include_rf, use_vision)
        req = urllib.request.Request(
            url,
            data=json.dumps(body).encode("utf-8"),
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                raw = json.loads(resp.read().decode("utf-8"))
            break
        except urllib.error.HTTPError as e:
            err_body = ""
            try:
                err_body = e.read().decode("utf-8", errors="replace")
            except Exception:
                pass
            low_err = err_body.lower()
            if e.code in (400, 422):
                if include_rf and any(k in low_err for k in ("response_format", "json_object", "structured", "json mode")):
                    include_rf = False
                    continue
                if use_vision and any(k in low_err for k in ("image", "vision", "multimodal", "image_url", "content[1]")):
                    use_vision = False
                    continue
            if e.code == 429:
                retry_after = _parse_retry_after_seconds(e.headers, err_body)
                raise RateLimitError(
                    f"Limite de requêtes (429) sur {label} ({model_id}, reprise dans {retry_after}s).",
                    retry_after=retry_after,
                    is_daily="perday" in low_err or "daily" in low_err,
                )
            if e.code in (401, 403):
                raise RuntimeError(f"Clé API {label} invalide ou non autorisée ({e.code}) : {err_body[:240]}")
            if e.code == 404:
                raise RuntimeError(f"Modèle '{model_id}' ou endpoint introuvable sur {label} (404).")
            raise RuntimeError(f"Erreur API {label} ({e.code}) : {err_body[:260]}")
        except urllib.error.URLError as e:
            raise RuntimeError(f"Impossible de joindre l'API {label} : {e.reason}")

    if not isinstance(raw, dict):
        raise RuntimeError(f"Réponse invalide de {label}.")

    choices = raw.get("choices") or []
    if not choices:
        raise RuntimeError(f"Réponse {label} vide (aucun choix retourné).")

    msg_content = (choices[0].get("message") or {}).get("content", "")
    if isinstance(msg_content, list):
        text_out = "".join(
            str(part.get("text", "")) for part in msg_content if isinstance(part, dict)
        ).strip()
    else:
        text_out = str(msg_content or "").strip()

    if not text_out:
        raise RuntimeError(f"{label} n'a renvoyé aucun texte.")

    text_out = re.sub(r"^```(?:json)?\s*|\s*```$", "", text_out, flags=re.DOTALL).strip()
    parsed = extract_json_object(text_out)
    return _parse_translation_items(
        parsed.get("translations") if isinstance(parsed, dict) else None
    )


def translate_bubbles_with_anthropic(
    bubbles: list[dict],
    model_id: str,
    api_key: str,
    base_url: str,
    source_lang: str = "en",
    target_lang: str = "fr",
    context_history: list[str] | None = None,
    som_image_b64: str | None = None,
    glossary: dict | None = None,
) -> dict[int, dict]:
    """Traduit les bulles via l'API native Anthropic Claude (/v1/messages)."""
    if not bubbles:
        return {}

    use_vision = bool(som_image_b64 and model_supports_vision(f"anthropic:{model_id}"))

    def _build_body(with_img: bool) -> dict:
        sys_prompt = build_manga_system_prompt(
            source_lang, use_vision=with_img, glossary=glossary, target_lang=target_lang
        )
        items = [{"id": b["id"], "text": b["text"]} for b in bubbles]
        u_text = build_user_prompt(
            items, context_history=context_history, use_vision=with_img, target_lang=target_lang
        )
        blocks: list[dict] = []
        if with_img and som_image_b64:
            blocks.append(
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": som_image_b64,
                    },
                }
            )
        blocks.append({"type": "text", "text": u_text})
        return {
            "model": model_id,
            "max_tokens": 4096,
            "temperature": 0.45,
            "system": sys_prompt,
            "messages": [{"role": "user", "content": blocks}],
        }

    url = f"{base_url.rstrip('/')}/messages"
    headers = {
        "Content-Type": "application/json",
        "x-api-key": api_key,
        "anthropic-version": "2023-06-01",
    }

    raw = None
    for _attempt in range(2):
        body = _build_body(use_vision)
        req = urllib.request.Request(
            url,
            data=json.dumps(body).encode("utf-8"),
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                raw = json.loads(resp.read().decode("utf-8"))
            break
        except urllib.error.HTTPError as e:
            err_body = ""
            try:
                err_body = e.read().decode("utf-8", errors="replace")
            except Exception:
                pass
            low_err = err_body.lower()
            if e.code == 400 and use_vision and any(k in low_err for k in ("image", "vision", "media_type")):
                use_vision = False
                continue
            if e.code == 429:
                retry_after = _parse_retry_after_seconds(e.headers, err_body)
                raise RateLimitError(
                    f"Limite de requêtes (429) sur Anthropic ({model_id}, reprise dans {retry_after}s).",
                    retry_after=retry_after,
                    is_daily=False,
                )
            if e.code in (401, 403):
                raise RuntimeError(f"Clé API Anthropic invalide ou non autorisée ({e.code}) : {err_body[:240]}")
            if e.code == 404:
                raise RuntimeError(f"Modèle '{model_id}' introuvable sur Anthropic (404).")
            raise RuntimeError(f"Erreur API Anthropic ({e.code}) : {err_body[:260]}")
        except urllib.error.URLError as e:
            raise RuntimeError(f"Impossible de joindre l'API Anthropic : {e.reason}")

    if not isinstance(raw, dict):
        raise RuntimeError("Réponse Anthropic invalide.")

    blocks = raw.get("content") or []
    text_out = "".join(
        str(b.get("text", "")) for b in blocks if isinstance(b, dict) and b.get("type") == "text"
    ).strip()
    if not text_out:
        raise RuntimeError("Anthropic n'a renvoyé aucun texte.")

    text_out = re.sub(r"^```(?:json)?\s*|\s*```$", "", text_out, flags=re.DOTALL).strip()
    parsed = extract_json_object(text_out)
    return _parse_translation_items(
        parsed.get("translations") if isinstance(parsed, dict) else None
    )


def translate_bubbles_with_gemini(
    bubbles: list[dict],
    model: str,
    source_lang: str = "en",
    target_lang: str = "fr",
    context_history: list[str] | None = None,
    som_image_b64: str | None = None,
    glossary: dict | None = None,
) -> dict[int, dict]:
    """Traduit toutes les bulles d'une planche (Ollama local ou n'importe quelle API Cloud)."""
    cfg = load_config()
    provider, raw_model = parse_model_spec(model, cfg=cfg)

    if provider == "ollama":
        return translate_bubbles_with_ollama(
            bubbles,
            model=f"ollama:{raw_model}",
            source_lang=source_lang,
            target_lang=target_lang,
            context_history=context_history,
            som_image_b64=som_image_b64,
            glossary=glossary,
        )

    if not bubbles:
        return {}

    api_key = get_api_key(provider=provider, cfg=cfg)
    pinfo = API_PROVIDERS.get(provider, API_PROVIDERS["gemini"])
    if not api_key:
        raise ValueError(
            f"Clé API manquante pour {pinfo['label']} : configure-la dans Yomi (Paramètres → Moteur & Modèles) ou choisis Ollama local."
        )

    base_url = _resolve_provider_base_url(provider, cfg=cfg)
    proto = pinfo["protocol"]

    if proto == "anthropic":
        return translate_bubbles_with_anthropic(
            bubbles,
            model_id=raw_model,
            api_key=api_key,
            base_url=base_url,
            source_lang=source_lang,
            target_lang=target_lang,
            context_history=context_history,
            som_image_b64=som_image_b64,
            glossary=glossary,
        )

    if proto == "openai_compat":
        return translate_bubbles_with_openai_compat(
            bubbles,
            provider=provider,
            model_id=raw_model,
            api_key=api_key,
            base_url=base_url,
            source_lang=source_lang,
            target_lang=target_lang,
            context_history=context_history,
            som_image_b64=som_image_b64,
            glossary=glossary,
        )

    model = raw_model
    use_vision = bool(som_image_b64 and model_supports_vision(model))
    system_prompt = build_manga_system_prompt(
        source_lang, use_vision=use_vision, glossary=glossary, target_lang=target_lang
    )
    payload_items = [{"id": b["id"], "text": b["text"]} for b in bubbles]
    user_text = build_user_prompt(
        payload_items, context_history=context_history, use_vision=use_vision, target_lang=target_lang
    )

    user_parts: list[dict] = [{"text": user_text}]
    if use_vision and som_image_b64:
        user_parts.append({"inlineData": {"mimeType": "image/jpeg", "data": som_image_b64}})

    body = {
        "systemInstruction": {"parts": [{"text": system_prompt}]},
        "contents": [
            {
                "role": "user",
                "parts": user_parts,
            }
        ],
        "generationConfig": {
            "temperature": 0.45,
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "OBJECT",
                "properties": {
                    "translations": {
                        "type": "ARRAY",
                        "items": {
                            "type": "OBJECT",
                            "properties": {
                                "id": {"type": "INTEGER"},
                                "who": {"type": "STRING"},
                                "fr": {"type": "STRING"},
                                "skip": {"type": "BOOLEAN"},
                            },
                            "required": ["id", "who", "fr"],
                        },
                    }
                },
                "required": ["translations"],
            },
        },
    }

    safe_model = urllib.parse.quote(model, safe="-._")
    url = f"{base_url.rstrip('/')}/models/{safe_model}:generateContent"
    req = urllib.request.Request(
        url,
        data=json.dumps(body).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "x-goog-api-key": api_key,
        },
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=45) as resp:
            raw = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        err_body = ""
        try:
            err_body = e.read().decode("utf-8", errors="replace")
        except Exception:
            pass
        if e.code == 429:
            retry_after = 15
            m = re.search(r"retry\s+in\s+(\d+(?:\.\d+)?)s", err_body, re.IGNORECASE)
            if not m:
                m = re.search(r'"retryDelay"\s*:\s*"(\d+(?:\.\d+)?)s"', err_body)
            if m:
                retry_after = max(5, min(90, int(float(m.group(1))) + 2))
            is_daily = bool(
                re.search(r"PerDay|per\s*day|daily|GenerateRequestsPerDay", err_body, re.IGNORECASE)
            )
            if is_daily:
                msg = (
                    f"Quota journalier gratuit atteint sur {model}. "
                    "Bascule sur gemini-3.5-flash-lite ou gemini-3.6-flash dans le menu."
                )
            else:
                msg = (
                    f"Limite par minute (429) atteinte sur {model} "
                    f"(reprise dans {retry_after}s)."
                )
            print(f"[GEMINI 429] model={model} daily={is_daily} retry={retry_after}s | {err_body[:260]}", flush=True)
            raise RateLimitError(msg, retry_after=retry_after, is_daily=is_daily)
        if e.code in (400, 401, 403):
            raise RuntimeError(
                f"Clé API Gemini invalide ou non autorisée ({e.code}). "
                f"Vérifie GEMINI_API_KEY dans .env. Détail : {err_body[:240]}"
            )
        if e.code == 404:
            raise RuntimeError(
                f"Modèle '{model}' introuvable (404). Choisis un autre modèle dans le menu."
            )
        raise RuntimeError(f"Erreur API Gemini ({e.code}) : {err_body[:260]}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Impossible de joindre l'API Gemini : {e.reason}")

    candidates = raw.get("candidates") or []
    if not candidates:
        raise RuntimeError("Réponse Gemini vide (peut-être filtrée par sécurité).")

    parts = candidates[0].get("content", {}).get("parts", [])
    text_out = "".join(p.get("text", "") for p in parts).strip()
    if not text_out:
        raise RuntimeError("Gemini n'a renvoyé aucun texte.")

    # Retire d'éventuelles balises markdown ```json ... ```
    text_out = re.sub(r"^```(?:json)?\s*|\s*```$", "", text_out, flags=re.DOTALL).strip()
    parsed = extract_json_object(text_out)
    return _parse_translation_items(
        parsed.get("translations") if isinstance(parsed, dict) else None
    )


def validate_public_image_url(url: str) -> None:
    """Bloque toute URL pointant vers localhost, une IP privée ou non routable (anti-SSRF)."""
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("URL d'image invalide.")
    host = parsed.hostname.strip().lower()
    if host in ("localhost", "localhost.localdomain") or host.endswith(".local"):
        raise ValueError("Hôte local interdit pour image_url.")
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    try:
        infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except socket.gaierror as e:
        raise ValueError(f"Impossible de résoudre l'hôte image ({host}) : {e}")
    if not infos:
        raise ValueError("Aucune adresse IP résolue pour image_url.")
    for info in infos:
        ip_str = info[4][0].split("%", 1)[0]
        ip_obj = ipaddress.ip_address(ip_str)
        if not ip_obj.is_global:
            raise ValueError("Adresse IP non publique interdite pour image_url.")


class _SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        validate_public_image_url(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


_SAFE_URL_OPENER = urllib.request.build_opener(_SafeRedirectHandler())


def decode_image_payload(data: dict) -> bytes:
    """Extrait les octets d'image depuis base64 data URL ou URL distante publique."""
    img_b64 = data.get("image_base64", "")
    if img_b64:
        if "," in img_b64:
            img_b64 = img_b64.split(",", 1)[1]
        raw = base64.b64decode(img_b64)
        if len(raw) > MAX_IMAGE_BYTES:
            raise ValueError("Image trop volumineuse (> 20 Mo).")
        return raw

    img_url = data.get("image_url", "").strip()
    if img_url:
        validate_public_image_url(img_url)
        headers = {
            "User-Agent": (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
                "AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15"
            ),
            "Accept": "image/avif,image/webp,image/apng,image/*,*/*;q=0.8",
            "Accept-Language": "fr-FR,fr;q=0.9,en;q=0.8",
        }
        referer = data.get("referer", "").strip()
        if referer:
            headers["Referer"] = referer
            ref_parsed = urllib.parse.urlparse(referer)
            if ref_parsed.scheme and ref_parsed.netloc:
                headers["Origin"] = f"{ref_parsed.scheme}://{ref_parsed.netloc}"
        req = urllib.request.Request(img_url, headers=headers)
        try:
            with _SAFE_URL_OPENER.open(req, timeout=20) as resp:
                ctype = str(resp.headers.get("Content-Type", "")).split(";")[0].strip().lower()
                if ctype and not (
                    ctype.startswith("image/")
                    or ctype in ("application/octet-stream", "binary/octet-stream")
                ):
                    raise ValueError(f"Type MIME non-image refusé ({ctype}).")
                raw = resp.read(MAX_IMAGE_BYTES + 1)
                if len(raw) > MAX_IMAGE_BYTES:
                    raise ValueError("Image distante trop volumineuse (> 20 Mo).")
                return raw
        except urllib.error.HTTPError as e:
            raise ValueError(f"IMAGE_FETCH_FAILED ({e.code}) : impossible de télécharger l'image côté serveur.")
        except urllib.error.URLError as e:
            raise ValueError(f"IMAGE_FETCH_FAILED : {e.reason}")

    raise ValueError("Aucune image fournie (image_base64 ou image_url requis).")


def schedule_prefetch_next(prefetch_spec: dict | None, source_lang: str) -> None:
    """Lance le téléchargement + OCR de la page suivante pendant que le LLM traduit."""
    if not isinstance(prefetch_spec, dict):
        return
    img_url = str(prefetch_spec.get("image_url") or "").strip()
    parsed = urllib.parse.urlparse(img_url)
    if not img_url or parsed.scheme not in ("http", "https"):
        return

    key = (img_url, source_lang)
    with PREFETCH_LOCK:
        if key in PREFETCH_CACHE or key in PREFETCH_INFLIGHT:
            return
        PREFETCH_INFLIGHT.add(key)

    def _worker() -> None:
        try:
            img_bytes = decode_image_payload(prefetch_spec)
            t0 = time.perf_counter()
            ocr_out = run_apple_vision_ocr(img_bytes, source_lang=source_lang)
            ocr_ms = int((time.perf_counter() - t0) * 1000)
            bubbles = group_lines_into_bubbles(
                ocr_out.get("lines", []), source_lang=source_lang
            )
            with PREFETCH_LOCK:
                PREFETCH_CACHE[key] = {
                    "img_bytes": img_bytes,
                    "ocr_out": ocr_out,
                    "ocr_ms": ocr_ms,
                    "bubbles": bubbles,
                }
                PREFETCH_CACHE.move_to_end(key)
                while len(PREFETCH_CACHE) > MAX_PREFETCH_ENTRIES:
                    PREFETCH_CACHE.popitem(last=False)
        except Exception:
            pass
        finally:
            with PREFETCH_LOCK:
                PREFETCH_INFLIGHT.discard(key)

    threading.Thread(target=_worker, daemon=True).start()


def find_disk_cache_hit(
    image_bytes: bytes,
    model: str,
    source_lang: str,
    target_lang: str,
    vision_mode: str,
    glossary_hash: str = "none",
) -> dict | None:
    if not model_supports_vision(model) or vision_mode == "off":
        p = get_cache_path(
            image_bytes, model, source_lang, target_lang, vision=False, glossary_hash=glossary_hash
        )
        if p.exists():
            return json.loads(p.read_text(encoding="utf-8"))
        return None
    if vision_mode == "on":
        p = get_cache_path(
            image_bytes, model, source_lang, target_lang, vision=True, glossary_hash=glossary_hash
        )
        if p.exists():
            return json.loads(p.read_text(encoding="utf-8"))
        return None
    # Mode adaptatif "auto"
    p_vis = get_cache_path(
        image_bytes, model, source_lang, target_lang, vision=True, glossary_hash=glossary_hash
    )
    if p_vis.exists():
        return json.loads(p_vis.read_text(encoding="utf-8"))
    p_txt = get_cache_path(
        image_bytes, model, source_lang, target_lang, vision=False, glossary_hash=glossary_hash
    )
    if p_txt.exists():
        cached_txt = json.loads(p_txt.read_text(encoding="utf-8"))
        if not should_use_vision("auto", model, cached_txt.get("bubbles", [])):
            return cached_txt
    return None


def get_cache_path(
    image_bytes: bytes,
    model: str,
    source_lang: str,
    target_lang: str,
    vision: bool = False,
    glossary_hash: str = "none",
) -> Path:
    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    h = hashlib.sha256()
    h.update(image_bytes)
    h.update(
        f"|{model}|{source_lang}|{target_lang}|v6|vis={1 if vision else 0}|gl={glossary_hash}".encode("utf-8")
    )
    return CACHE_DIR / f"{h.hexdigest()}.json"


TEST_PAGE_HTML = r"""<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<title>Yomi — Test local</title>
<style>
  :root {
    --bg: #0f1117; --card: #181c25; --border: #2b3242;
    --text: #eef1f7; --muted: #98a2b3; --accent: #4f8cff;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", sans-serif;
    background: var(--bg); color: var(--text); padding: 24px;
  }
  .wrap { max-width: 980px; margin: 0 auto; }
  h1 { margin: 0 0 6px; font-size: 22px; }
  .sub { color: var(--muted); font-size: 14px; margin-bottom: 18px; }
  .bar {
    display: flex; flex-wrap: wrap; gap: 10px; align-items: center;
    background: var(--card); border: 1px solid var(--border);
    padding: 12px 14px; border-radius: 12px; margin-bottom: 18px;
  }
  select, button {
    background: #222836; color: var(--text); border: 1px solid var(--border);
    padding: 8px 12px; border-radius: 8px; font-size: 13px; cursor: pointer;
  }
  button.primary { background: var(--accent); border-color: var(--accent); font-weight: 600; }
  button:disabled { opacity: 0.5; cursor: wait; }
  .badge { font-size: 12px; padding: 4px 10px; border-radius: 999px; margin-left: auto; }
  .badge.ok { background: #123824; color: #5ce09b; }
  .badge.warn { background: #3c2612; color: #f6b26b; }
  .dropzone {
    border: 2px dashed var(--border); border-radius: 14px; padding: 28px;
    text-align: center; color: var(--muted); cursor: pointer;
    background: rgba(255,255,255,0.02); margin-bottom: 18px;
  }
  .dropzone.drag { border-color: var(--accent); color: var(--text); }
  #status { font-size: 13px; color: var(--muted); min-height: 20px; margin-bottom: 12px; }
  .stage-wrap { display: flex; justify-content: center; }
  .stage { position: relative; display: inline-block; max-width: 100%; }
  .stage img { display: block; max-width: 100%; height: auto; border-radius: 8px; }
  .bubble {
    position: absolute;
    display: flex; align-items: center; justify-content: center; text-align: center;
    padding: 3px 5px; font-weight: 700; line-height: 1.13; cursor: pointer;
    overflow: hidden; transition: opacity 0.15s ease;
    font-family: "CC Wild Words", "Anime Ace 2.0 BB", "Avenir Next Condensed", "Chalkboard SE", -apple-system, sans-serif;
    letter-spacing: -0.01em;
    text-wrap: balance;
    overflow-wrap: break-word;
    hyphens: auto;
  }
  .bubble.style-bubble_light {
    background: #ffffff;
    color: #111111;
    border-radius: 26% / 20%;
    box-shadow: 0 0 4px 2px #ffffff;
  }
  .bubble.style-bubble_dark {
    background: #141414;
    color: #f8f8f8;
    border-radius: 26% / 20%;
    box-shadow: 0 0 4px 2px #141414;
  }
  .bubble.style-floating {
    background: rgba(255, 255, 255, 0.78);
    backdrop-filter: blur(4px);
    -webkit-backdrop-filter: blur(4px);
    color: #111111;
    border-radius: 10px;
    box-shadow: 0 0 5px 2px rgba(255, 255, 255, 0.70);
    text-shadow: -1.5px -1.5px 0 #fff, 1.5px -1.5px 0 #fff, -1.5px 1.5px 0 #fff, 1.5px 1.5px 0 #fff, 0 0 6px #fff;
    text-transform: uppercase;
  }
  .bubble.peek { opacity: 0.08; }
  .stage.hide-bubbles .bubble { display: none; }
</style>
</head>
<body>
<div class="wrap">
  <h1>Yomi — Page de test</h1>
  <div class="sub">Glisse une planche ici ou colle une capture d'écran (<strong>Cmd + V</strong>) pour tester Apple Vision + Gemini.</div>

  <div class="bar">
    <select id="model"></select>
    <select id="lang">
      <option value="en" selected>Anglais → Français</option>
      <option value="ja">Japonais → Français</option>
      <option value="auto">Auto → Français</option>
    </select>
    <select id="visionMode">
      <option value="auto" selected>Vision : Auto (Recommandé)</option>
      <option value="on">Vision : ON (Toujours)</option>
      <option value="off">Vision : OFF (Texte rapide)</option>
    </select>
    <button id="toggleBtn" disabled>Afficher l'original</button>
    <button id="retranslateBtn" class="primary" disabled>Retraduire</button>
    <span id="keyBadge" class="badge warn">Vérification...</span>
  </div>

  <div id="dropzone" class="dropzone">
    Clique pour choisir une image, glisse-dépose une page de manga, ou appuie sur <strong>Cmd + V</strong>
    <input id="fileInput" type="file" accept="image/*" hidden />
  </div>

  <div id="status"></div>
  <div class="stage-wrap">
    <div id="stage" class="stage"></div>
  </div>
</div>

<script>
let currentDataUrl = null;
let bubblesVisible = true;

async function init() {
  const res = await fetch('/api/health');
  const data = await res.json();
  const sel = document.getElementById('model');
  sel.innerHTML = '';
  for (const m of data.models) {
    const opt = document.createElement('option');
    opt.value = m.id;
    opt.textContent = m.label;
    if (m.id === data.default_model) opt.selected = true;
    sel.appendChild(opt);
  }
  const badge = document.getElementById('keyBadge');
  if (data.api_key_configured) {
    badge.textContent = 'Clé Gemini OK';
    badge.className = 'badge ok';
  } else {
    badge.textContent = 'Clé GEMINI_API_KEY manquante (.env)';
    badge.className = 'badge warn';
  }
}

function fitBubbleFont(el, text) {
  const w = el.clientWidth || 80;
  const h = el.clientHeight || 40;
  let size = Math.max(9, Math.min(20, Math.sqrt((w * h) / Math.max(text.length, 4)) * 1.15));
  el.style.fontSize = size + 'px';
  while (size > 7.5 && (el.scrollHeight > el.clientHeight + 2 || el.scrollWidth > el.clientWidth + 2)) {
    size -= 0.7;
    el.style.fontSize = size.toFixed(1) + 'px';
  }
}

async function translateImage(dataUrl, skipCache = false) {
  currentDataUrl = dataUrl;
  const status = document.getElementById('status');
  const stage = document.getElementById('stage');
  stage.innerHTML = '';
  const img = document.createElement('img');
  img.src = dataUrl;
  stage.appendChild(img);

  status.textContent = 'OCR Apple Vision + traduction Gemini en cours...';
  document.getElementById('retranslateBtn').disabled = true;

  try {
    const res = await fetch('/api/translate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        image_base64: dataUrl,
        model: document.getElementById('model').value,
        source_lang: document.getElementById('lang').value,
        target_lang: 'fr',
        vision: document.getElementById('visionMode').value,
        skip_cache: skipCache
      })
    });
    const out = await res.json();
    if (!res.ok) throw new Error(out.error || 'Erreur serveur');

    if (!img.complete) await new Promise(r => img.onload = r);

    for (const b of out.bubbles) {
      const div = document.createElement('div');
      div.className = 'bubble style-' + (b.style || 'bubble_light');
      div.style.left = (b.x * 100) + '%';
      div.style.top = (b.y * 100) + '%';
      div.style.width = (b.w * 100) + '%';
      div.style.height = (b.h * 100) + '%';
      div.textContent = b.fr || b.text;
      div.title = (b.who ? b.who + ' · ' : '') + 'Original : ' + b.text + ' (cliquer pour voir l\'original)';
      div.addEventListener('click', () => div.classList.toggle('peek'));
      stage.appendChild(div);
      fitBubbleFont(div, div.textContent);
    }

    const visTag = out.vision_used ? ' · Vision' : '';
    const cacheTxt = out.cached ? ' (depuis le cache)' : ` (OCR ${out.ocr_ms} ms, Traduction ${out.translate_ms} ms)`;
    status.textContent = `${out.bubbles.length} bulles traduites avec ${out.model}${visTag}${cacheTxt}. Clique sur une bulle pour voir l'original.`;
    document.getElementById('toggleBtn').disabled = false;
    document.getElementById('retranslateBtn').disabled = false;
  } catch (err) {
    status.textContent = 'Erreur : ' + err.message;
    document.getElementById('retranslateBtn').disabled = false;
  }
}

function handleFile(file) {
  if (!file || !file.type.startsWith('image/')) return;
  const reader = new FileReader();
  reader.onload = () => translateImage(reader.result);
  reader.readAsDataURL(file);
}

const dz = document.getElementById('dropzone');
const fi = document.getElementById('fileInput');
dz.addEventListener('click', () => fi.click());
fi.addEventListener('change', () => handleFile(fi.files[0]));
dz.addEventListener('dragover', e => { e.preventDefault(); dz.classList.add('drag'); });
dz.addEventListener('dragleave', () => dz.classList.remove('drag'));
dz.addEventListener('drop', e => {
  e.preventDefault(); dz.classList.remove('drag');
  handleFile(e.dataTransfer.files[0]);
});
window.addEventListener('paste', e => {
  for (const item of (e.clipboardData?.items || [])) {
    if (item.type.startsWith('image/')) {
      handleFile(item.getAsFile());
      break;
    }
  }
});
document.getElementById('toggleBtn').addEventListener('click', () => {
  bubblesVisible = !bubblesVisible;
  document.getElementById('stage').classList.toggle('hide-bubbles', !bubblesVisible);
  document.getElementById('toggleBtn').textContent = bubblesVisible ? "Afficher l'original" : "Afficher la VF";
});
document.getElementById('retranslateBtn').addEventListener('click', () => {
  if (currentDataUrl) translateImage(currentDataUrl, true);
});

init();
</script>
</body>
</html>
"""


class MangaTranslatorHandler(BaseHTTPRequestHandler):
    def _send_cors(self) -> None:
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.send_header("Access-Control-Allow-Private-Network", "true")

    def _send_json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self._send_cors()
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self) -> None:
        self.send_response(204)
        self._send_cors()
        self.end_headers()

    def do_GET(self) -> None:
        parsed_req = urllib.parse.urlparse(self.path)
        path = parsed_req.path
        if path == "/api/glossaries":
            qs = urllib.parse.parse_qs(parsed_req.query)
            page_url = (qs.get("url") or [""])[0]
            page_title = (qs.get("title") or [""])[0]
            glossaries = list_glossaries()
            detected = resolve_glossary("auto", page_url=page_url, page_title=page_title)
            self._send_json(
                200,
                {
                    "glossaries": glossaries,
                    "detected_slug": detected["slug"] if detected else None,
                },
            )
            return

        if path == "/api/config":
            self._send_json(200, public_config(load_config()))
            return

        if path == "/api/health":
            cfg = load_config()
            prov = str(cfg.get("api_provider") or "gemini").strip().lower()
            pinfo = API_PROVIDERS.get(prov, API_PROVIDERS["gemini"])
            ollama_ok, ollama_models = check_ollama_status()
            models_list = build_available_models(ollama_models)
            default_model = get_default_model(ollama_ok=ollama_ok, ollama_models=ollama_models)
            has_key = bool(get_api_key(provider=prov, cfg=cfg))
            self._send_json(
                200,
                {
                    "ok": True,
                    "api_key_configured": ollama_ok or has_key,
                    "has_api_key": has_key,
                    "api_provider": prov,
                    "api_provider_label": pinfo["label"],
                    "ollama_ready": ollama_ok,
                    "ollama_models": ollama_models,
                    "default_model": default_model,
                    "models": models_list,
                    "source_lang": cfg["source_lang"],
                    "target_lang": cfg["target_lang"],
                    "vision_mode": cfg["vision_mode"],
                    "theme": cfg["theme"],
                    "mascot": cfg.get("mascot", "mochi"),
                    "target_languages": TARGET_LANGUAGES,
                    "ocr_ready": OCR_BINARY.exists(),
                },
            )
            return

        if path in ("/userscript", "/yomi.user.js", "/manga-translator.user.js", "/install.user.js"):
            if not USERSCRIPT_PATH.exists():
                self._send_json(404, {"error": "Userscript introuvable"})
                return
            content = USERSCRIPT_PATH.read_bytes()
            self.send_response(200)
            self._send_cors()
            self.send_header("Content-Type", "text/javascript; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)
            return

        if path == "/":
            html = TEST_PAGE_HTML.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(html)))
            self.end_headers()
            self.wfile.write(html)
            return

        self._send_json(404, {"error": "Endpoint inconnu"})

    def _translate_single_prepared(
        self,
        image_bytes: bytes,
        cache_file: Path,
        ocr_out: dict,
        ocr_ms: int,
        bubbles: list[dict],
        model: str,
        source_lang: str,
        target_lang: str,
        effective_vision: bool,
        context_history: list[str] | None = None,
        index: int | None = None,
        glossary: dict | None = None,
    ) -> dict:
        to_translate = []
        for b in bubbles:
            if is_punctuation_only(b.get("text", "")):
                b["fr"] = b["text"]
            else:
                to_translate.append(b)

        translate_ms = 0
        used_vision = bool(effective_vision and to_translate)
        all_translated = True

        if to_translate:
            som_b64 = None
            if used_vision:
                som_b64 = create_som_image_base64(image_bytes, to_translate)

            t1 = time.perf_counter()
            translations = translate_bubbles_with_gemini(
                to_translate,
                model=model,
                source_lang=source_lang,
                target_lang=target_lang,
                context_history=context_history,
                som_image_b64=som_b64,
                glossary=glossary,
            )
            translate_ms = int((time.perf_counter() - t1) * 1000)

            skipped_ids: set[int] = set()
            for b in to_translate:
                tr = translations.get(b["id"])
                if tr and tr.get("skip") and can_skip_bubble(b):
                    skipped_ids.add(b["id"])
                    continue
                fr_val = str(tr.get("fr", "")).strip() if tr else ""
                if fr_val:
                    b["fr"] = fr_val
                else:
                    b["fr"] = b["text"]
                    all_translated = False
                if tr and tr.get("who"):
                    b["who"] = tr["who"]

            if skipped_ids:
                bubbles = [b for b in bubbles if b["id"] not in skipped_ids]

        page_result: dict = {
            "bubbles": bubbles,
            "image_width": ocr_out.get("width", 0),
            "image_height": ocr_out.get("height", 0),
            "model": model,
            "source_lang": source_lang,
            "target_lang": target_lang,
            "vision_used": used_vision,
            "glossary_used": glossary["title"] if glossary else None,
            "cached": False,
            "ocr_ms": ocr_ms,
            "translate_ms": translate_ms,
        }
        if index is not None:
            page_result["index"] = index

        if all_translated:
            try:
                cache_file.write_text(
                    json.dumps(page_result, ensure_ascii=False), encoding="utf-8"
                )
            except OSError:
                pass
        return page_result

    def _handle_translate_batch(self, data: dict) -> None:
        """Traduit un lot de planches avec Vision adaptatif, mémoire roulante et pré-OCR."""
        pages_in = data.get("pages")
        if not isinstance(pages_in, list) or not pages_in:
            self._send_json(400, {"error": "Champ 'pages' requis (liste non vide)."})
            return

        model = str(data.get("model") or get_default_model()).strip()
        source_lang = str(data.get("source_lang") or "en").strip().lower()
        target_lang = str(data.get("target_lang") or "fr").strip().lower()
        skip_cache = bool(data.get("skip_cache", False))
        vision_mode = normalize_vision_mode(data.get("vision", "auto"))
        glossary_slug = str(data.get("glossary_slug") or "auto").strip()
        page_url = str(data.get("page_url") or data.get("referer") or "").strip()
        if not page_url and isinstance(pages_in[0], dict):
            page_url = str(pages_in[0].get("referer") or "").strip()
        page_title = str(data.get("page_title") or "").strip()
        glossary = resolve_glossary(glossary_slug, page_url=page_url, page_title=page_title)
        glossary_hash = compute_glossary_hash(glossary)
        context_history = data.get("context_history") or []
        if not isinstance(context_history, list):
            context_history = []

        # Déclenche dès maintenant le pré-téléchargement + OCR de la page suivante (N+1)
        schedule_prefetch_next(data.get("prefetch_next"), source_lang)

        results: dict[int, dict] = {}
        pending_ocr: list[dict] = []

        def prep_page(item: dict) -> tuple[int, dict | None, dict | None]:
            idx = int(item.get("index", 0))
            try:
                img_url = str(item.get("image_url") or "").strip()
                prefetched = None
                if img_url and not item.get("image_base64"):
                    with PREFETCH_LOCK:
                        prefetched = PREFETCH_CACHE.pop((img_url, source_lang), None)

                if prefetched is not None:
                    img_bytes = prefetched["img_bytes"]
                    ocr_out = prefetched["ocr_out"]
                    ocr_ms = prefetched["ocr_ms"]
                    bubbles = [dict(b) for b in prefetched["bubbles"]]
                else:
                    img_bytes = decode_image_payload(item)
                    if not skip_cache:
                        hit = find_disk_cache_hit(
                            img_bytes,
                            model,
                            source_lang,
                            target_lang,
                            vision_mode,
                            glossary_hash=glossary_hash,
                        )
                        if hit is not None:
                            hit["cached"] = True
                            hit["index"] = idx
                            return (idx, hit, None)
                    t0 = time.perf_counter()
                    ocr_out = run_apple_vision_ocr(img_bytes, source_lang=source_lang)
                    ocr_ms = int((time.perf_counter() - t0) * 1000)
                    bubbles = group_lines_into_bubbles(
                        ocr_out.get("lines", []), source_lang=source_lang
                    )

                page_vision = should_use_vision(vision_mode, model, bubbles)
                cache_file = get_cache_path(
                    img_bytes,
                    model,
                    source_lang,
                    target_lang,
                    vision=page_vision,
                    glossary_hash=glossary_hash,
                )
                if not skip_cache and cache_file.exists():
                    cached_resp = json.loads(cache_file.read_text(encoding="utf-8"))
                    cached_resp["cached"] = True
                    cached_resp["index"] = idx
                    return (idx, cached_resp, None)

                return (
                    idx,
                    None,
                    {
                        "index": idx,
                        "img_bytes": img_bytes,
                        "cache_file": cache_file,
                        "ocr_out": ocr_out,
                        "ocr_ms": ocr_ms,
                        "bubbles": bubbles,
                        "page_vision": page_vision,
                    },
                )
            except Exception as e:
                return (idx, {"index": idx, "error": str(e)}, None)

        with ThreadPoolExecutor(max_workers=min(4, len(pages_in))) as pool:
            for idx, immediate, pending in pool.map(prep_page, pages_in):
                if immediate is not None:
                    results[idx] = immediate
                elif pending is not None:
                    pending_ocr.append(pending)

        pending_sorted = sorted(pending_ocr, key=lambda x: x["index"])
        translate_ms = 0
        any_vision = any(p["page_vision"] for p in pending_sorted)

        if any_vision:
            running_ctx = [str(c) for c in context_history if c]
            for p in pending_sorted:
                idx = p["index"]
                page_result = self._translate_single_prepared(
                    image_bytes=p["img_bytes"],
                    cache_file=p["cache_file"],
                    ocr_out=p["ocr_out"],
                    ocr_ms=p["ocr_ms"],
                    bubbles=p["bubbles"],
                    model=model,
                    source_lang=source_lang,
                    target_lang=target_lang,
                    effective_vision=p["page_vision"],
                    context_history=running_ctx,
                    index=idx,
                    glossary=glossary,
                )
                translate_ms += page_result.get("translate_ms", 0)
                for b in page_result.get("bubbles", []):
                    fr = str(b.get("fr") or "").strip()
                    if fr and not is_punctuation_only(fr):
                        running_ctx.append(fr)
                running_ctx = running_ctx[-8:]
                results[idx] = page_result
        else:
            page_complete: dict[int, bool] = {p["index"]: True for p in pending_sorted}
            combined_bubbles: list[dict] = []
            mapping: list[tuple[int, dict]] = []
            next_id = 0
            for p in pending_sorted:
                for b in p["bubbles"]:
                    if is_punctuation_only(b.get("text", "")):
                        b["fr"] = b["text"]
                    else:
                        combined_bubbles.append({"id": next_id, "text": b["text"]})
                        mapping.append((p["index"], b))
                        next_id += 1

            skipped_obj_ids: set[int] = set()
            if combined_bubbles:
                t1 = time.perf_counter()
                translations = translate_bubbles_with_gemini(
                    combined_bubbles,
                    model=model,
                    source_lang=source_lang,
                    target_lang=target_lang,
                    context_history=context_history,
                    som_image_b64=None,
                    glossary=glossary,
                )
                translate_ms = int((time.perf_counter() - t1) * 1000)
                for gid, (page_idx, b) in enumerate(mapping):
                    tr = translations.get(gid)
                    if tr and tr.get("skip") and can_skip_bubble(b):
                        skipped_obj_ids.add(id(b))
                        continue
                    fr_val = str(tr.get("fr", "")).strip() if tr else ""
                    if fr_val:
                        b["fr"] = fr_val
                    else:
                        b["fr"] = b["text"]
                        page_complete[page_idx] = False
                    if tr and tr.get("who"):
                        b["who"] = tr["who"]

            for p in pending_sorted:
                idx = p["index"]
                kept_bubbles = (
                    [b for b in p["bubbles"] if id(b) not in skipped_obj_ids]
                    if skipped_obj_ids
                    else p["bubbles"]
                )
                page_result = {
                    "index": idx,
                    "bubbles": kept_bubbles,
                    "image_width": p["ocr_out"].get("width", 0),
                    "image_height": p["ocr_out"].get("height", 0),
                    "model": model,
                    "source_lang": source_lang,
                    "target_lang": target_lang,
                    "vision_used": False,
                    "glossary_used": glossary["title"] if glossary else None,
                    "cached": False,
                    "ocr_ms": p["ocr_ms"],
                    "translate_ms": translate_ms,
                }
                if page_complete.get(idx, True):
                    try:
                        p["cache_file"].write_text(
                            json.dumps(page_result, ensure_ascii=False), encoding="utf-8"
                        )
                    except OSError:
                        pass
                results[idx] = page_result

        ordered = [results[int(item.get("index", 0))] for item in pages_in]
        vis_count = sum(1 for p in pending_sorted if p["page_vision"])
        mode_tag = f"Vision {vis_count}/{len(pending_sorted)}" if any_vision else "Texte"
        print(
            f"[BATCH {mode_tag}] {len(pages_in)} pages ({len(pending_ocr)} traduites, "
            f"{len(pages_in) - len(pending_ocr)} cache) via {model} ({translate_ms} ms)",
            flush=True,
        )
        self._send_json(
            200,
            {
                "results": ordered,
                "model": model,
                "vision_used": any_vision,
                "translate_ms": translate_ms,
            },
        )

    def do_POST(self) -> None:
        path = urllib.parse.urlparse(self.path).path
        if path == "/api/unload":
            unloaded = unload_ollama_models()
            self._send_json(200, {"ok": True, "unloaded": unloaded})
            return

        if path == "/api/clear-cache":
            deleted = 0
            if CACHE_DIR.exists():
                for p in CACHE_DIR.glob("*.json"):
                    try:
                        p.unlink()
                        deleted += 1
                    except OSError:
                        pass
            with PREFETCH_LOCK:
                PREFETCH_CACHE.clear()
            self._send_json(200, {"ok": True, "deleted": deleted})
            return

        if path not in (
            "/api/translate",
            "/api/translate-batch",
            "/api/glossaries",
            "/api/glossaries/delete",
            "/api/config",
            "/api/test-gemini-key",
            "/api/test-api-key",
        ):
            self._send_json(404, {"error": "Endpoint inconnu"})
            return

        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > MAX_IMAGE_BYTES * 4:
                self._send_json(400, {"error": "Taille de requête invalide."})
                return
            raw_body = self.rfile.read(length)
            data = json.loads(raw_body.decode("utf-8"))

            if path == "/api/config":
                saved_cfg = save_config(data)
                self._send_json(200, {"ok": True, "config": public_config(saved_cfg)})
                return

            if path in ("/api/test-gemini-key", "/api/test-api-key"):
                cfg = load_config()
                prov = str(data.get("provider") or cfg.get("api_provider") or "gemini").strip().lower()
                test_key = str(data.get("api_key") or data.get("gemini_api_key") or "").strip() or get_api_key(provider=prov, cfg=cfg)
                base_url = str(data.get("base_url") or cfg.get("api_base_url") or "").strip()
                test_model = str(data.get("model") or cfg.get("api_model") or "").strip()
                ok, msg = test_cloud_api_key(prov, test_key, base_url=base_url, model=test_model)
                self._send_json(200, {"ok": ok, "message": msg, "error": "" if ok else msg})
                return

            if path == "/api/glossaries":
                saved = save_glossary(data)
                self._send_json(200, {"ok": True, "glossary": saved})
                return

            if path == "/api/glossaries/delete":
                slug = str(data.get("slug") or "").strip()
                deleted = delete_glossary(slug)
                self._send_json(200, {"ok": True, "deleted": deleted})
                return

            if path == "/api/translate-batch":
                self._handle_translate_batch(data)
                return

            cfg_defaults = load_config()
            model = str(data.get("model") or get_default_model()).strip()
            source_lang = str(data.get("source_lang") or cfg_defaults["source_lang"]).strip().lower()
            target_lang = str(data.get("target_lang") or cfg_defaults["target_lang"]).strip().lower()
            skip_cache = bool(data.get("skip_cache", False))
            vision_mode = normalize_vision_mode(data.get("vision") or cfg_defaults["vision_mode"])
            glossary_slug = str(data.get("glossary_slug") or "auto").strip()
            page_url = str(data.get("page_url") or data.get("referer") or "").strip()
            page_title = str(data.get("page_title") or "").strip()
            glossary = resolve_glossary(glossary_slug, page_url=page_url, page_title=page_title)
            glossary_hash = compute_glossary_hash(glossary)
            context_history = data.get("context_history") or []
            if not isinstance(context_history, list):
                context_history = []

            schedule_prefetch_next(data.get("prefetch_next"), source_lang)

            img_url = str(data.get("image_url") or "").strip()
            prefetched = None
            if img_url and not data.get("image_base64"):
                with PREFETCH_LOCK:
                    prefetched = PREFETCH_CACHE.pop((img_url, source_lang), None)

            if prefetched is not None:
                image_bytes = prefetched["img_bytes"]
                ocr_out = prefetched["ocr_out"]
                ocr_ms = prefetched["ocr_ms"]
                bubbles = [dict(b) for b in prefetched["bubbles"]]
            else:
                image_bytes = decode_image_payload(data)
                if not skip_cache:
                    hit = find_disk_cache_hit(
                        image_bytes,
                        model,
                        source_lang,
                        target_lang,
                        vision_mode,
                        glossary_hash=glossary_hash,
                    )
                    if hit is not None:
                        hit["cached"] = True
                        self._send_json(200, hit)
                        return

                t0 = time.perf_counter()
                ocr_out = run_apple_vision_ocr(image_bytes, source_lang=source_lang)
                ocr_ms = int((time.perf_counter() - t0) * 1000)
                bubbles = group_lines_into_bubbles(
                    ocr_out.get("lines", []), source_lang=source_lang
                )

            effective_vision = should_use_vision(vision_mode, model, bubbles)
            cache_file = get_cache_path(
                image_bytes,
                model,
                source_lang,
                target_lang,
                vision=effective_vision,
                glossary_hash=glossary_hash,
            )

            if not skip_cache and cache_file.exists():
                cached_resp = json.loads(cache_file.read_text(encoding="utf-8"))
                cached_resp["cached"] = True
                self._send_json(200, cached_resp)
                return

            result = self._translate_single_prepared(
                image_bytes=image_bytes,
                cache_file=cache_file,
                ocr_out=ocr_out,
                ocr_ms=ocr_ms,
                bubbles=bubbles,
                model=model,
                source_lang=source_lang,
                target_lang=target_lang,
                effective_vision=effective_vision,
                context_history=context_history,
                glossary=glossary,
            )

            self._send_json(200, result)

        except RateLimitError as e:
            self._send_json(
                429,
                {
                    "error": str(e),
                    "retry_after": e.retry_after,
                    "is_daily": e.is_daily,
                },
            )
        except ValueError as e:
            print(f"[ERR 400] {e}", flush=True)
            self._send_json(400, {"error": str(e)})
        except Exception as e:
            print(f"[ERR 500] {e}", flush=True)
            self._send_json(500, {"error": str(e)})

    def log_message(self, fmt: str, *args) -> None:
        print(f"[{self.log_date_time_string()}] {fmt % args}", flush=True)


class FastThreadingHTTPServer(ThreadingHTTPServer):
    """Évite le délai DNS (socket.getfqdn) au démarrage sur 127.0.0.1 sur macOS."""

    def server_bind(self) -> None:
        TCPServer.server_bind(self)
        host, port = self.server_address[:2]
        self.server_name = str(host)
        self.server_port = int(port)


def main() -> None:
    load_dotenv(BASE_DIR / ".env")
    ensure_data_dirs()
    ensure_ocr_binary()
    ensure_annotate_binary()

    host = "127.0.0.1"
    port = int(os.environ.get("PORT", "8765"))

    server = FastThreadingHTTPServer((host, port), MangaTranslatorHandler)
    print("=" * 56)
    print("  Yomi — Local Apple Silicon Manga Translator (Apple Vision + Gemma 4 / Gemini)")
    print("=" * 56)
    ollama_ok, ollama_models = check_ollama_status()
    default_model = get_default_model(ollama_ok=ollama_ok, ollama_models=ollama_models)
    print(f"  Serveur actif : http://{host}:{port}")
    print(f"  Modèle défaut : {default_model}")
    if ollama_ok:
        models_txt = ", ".join(ollama_models[:4]) or "aucun modèle téléchargé"
        print(f"  Ollama local  : OK ({models_txt})")
        if default_model.startswith("ollama:"):
            threading.Thread(
                target=warmup_ollama_model, args=(default_model,), daemon=True
            ).start()
    else:
        print("  Ollama local  : Inactif (lance 'ollama serve')")
    if get_api_key():
        print("  API Cloud     : Clé configurée", flush=True)
    threading.Thread(target=prune_disk_cache, daemon=True).start()
    print("  Ctrl+C pour arrêter.")
    print("=" * 56, flush=True)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nArrêt du serveur.")
        server.server_close()


if __name__ == "__main__":
    main()
