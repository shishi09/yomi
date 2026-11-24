use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub static CONFIG_LOCK: Mutex<()> = Mutex::new(());

pub const TARGET_LANGUAGES: &[(&str, &str)] = &[
    ("fr", "Français"),
    ("en", "English"),
    ("es", "Español"),
    ("de", "Deutsch"),
    ("it", "Italiano"),
    ("pt", "Português"),
];

pub const DEFAULT_OLLAMA_ORDER: &[&str] = &[
    "gemma4:12b",
    "gemma4:e4b",
    "gemma4:26b",
    "gemma4:e2b",
    "gemma4:31b",
    "gemma3:12b",
    "gemma3:4b",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProtocol {
    Gemini,
    Anthropic,
    OpenAiCompat,
}

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub protocol: ProviderProtocol,
    pub base_url: &'static str,
    pub env_vars: &'static [&'static str],
    pub default_model: &'static str,
    pub models: &'static [(&'static str, &'static str)],
}

pub const API_PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: "gemini",
        label: "Google Gemini",
        protocol: ProviderProtocol::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        env_vars: &["GEMINI_API_KEY", "YOMI_API_KEY"],
        default_model: "gemini-3.8-flash",
        models: &[
            ("gemini-3.8-flash", "Gemini 3.8 Flash"),
            ("gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite"),
            ("gemini-3.6-flash", "Gemini 3.6 Flash"),
        ],
    },
    ProviderInfo {
        id: "openai",
        label: "OpenAI",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "https://api.openai.com/v1",
        env_vars: &["OPENAI_API_KEY", "YOMI_API_KEY"],
        default_model: "gpt-4.1-mini",
        models: &[
            ("gpt-4.1-mini", "GPT-4.1 Mini"),
            ("gpt-4.1", "GPT-4.1"),
            ("gpt-4o-mini", "GPT-4o Mini"),
            ("gpt-4o", "GPT-4o"),
        ],
    },
    ProviderInfo {
        id: "anthropic",
        label: "Anthropic Claude",
        protocol: ProviderProtocol::Anthropic,
        base_url: "https://api.anthropic.com/v1",
        env_vars: &["ANTHROPIC_API_KEY", "YOMI_API_KEY"],
        default_model: "claude-sonnet-4-5",
        models: &[
            ("claude-sonnet-4-5", "Claude Sonnet 4.5"),
            ("claude-haiku-4-5", "Claude Haiku 4.5"),
            ("claude-opus-4-1", "Claude Opus 4.1"),
        ],
    },
    ProviderInfo {
        id: "openrouter",
        label: "OpenRouter",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "https://openrouter.ai/api/v1",
        env_vars: &["OPENROUTER_API_KEY", "YOMI_API_KEY"],
        default_model: "google/gemini-2.5-flash",
        models: &[
            ("google/gemini-2.5-flash", "Gemini 2.5 Flash"),
            ("openai/gpt-4.1-mini", "GPT-4.1 Mini"),
            ("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
            ("qwen/qwen2.5-vl-72b-instruct", "Qwen 2.5 VL 72B"),
        ],
    },
    ProviderInfo {
        id: "mistral",
        label: "Mistral AI",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "https://api.mistral.ai/v1",
        env_vars: &["MISTRAL_API_KEY", "YOMI_API_KEY"],
        default_model: "mistral-medium-latest",
        models: &[
            ("mistral-medium-latest", "Mistral Medium"),
            ("mistral-large-latest", "Mistral Large"),
            ("pixtral-large-latest", "Pixtral Large (Vision)"),
            ("mistral-small-latest", "Mistral Small"),
        ],
    },
    ProviderInfo {
        id: "deepseek",
        label: "DeepSeek",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "https://api.deepseek.com/v1",
        env_vars: &["DEEPSEEK_API_KEY", "YOMI_API_KEY"],
        default_model: "deepseek-chat",
        models: &[
            ("deepseek-chat", "DeepSeek Chat (V3)"),
            ("deepseek-reasoner", "DeepSeek Reasoner (R1)"),
        ],
    },
    ProviderInfo {
        id: "groq",
        label: "Groq",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "https://api.groq.com/openai/v1",
        env_vars: &["GROQ_API_KEY", "YOMI_API_KEY"],
        default_model: "meta-llama/llama-4-scout-17b-16e-instruct",
        models: &[
            ("meta-llama/llama-4-scout-17b-16e-instruct", "Llama 4 Scout 17B (Vision)"),
            ("meta-llama/llama-4-maverick-17b-128e-instruct", "Llama 4 Maverick 17B"),
            ("llama-3.3-70b-versatile", "Llama 3.3 70B"),
        ],
    },
    ProviderInfo {
        id: "xai",
        label: "xAI (Grok)",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "https://api.x.ai/v1",
        env_vars: &["XAI_API_KEY", "YOMI_API_KEY"],
        default_model: "grok-2-vision-latest",
        models: &[
            ("grok-2-vision-latest", "Grok 2 Vision"),
            ("grok-3-mini-fast", "Grok 3 Mini Fast"),
        ],
    },
    ProviderInfo {
        id: "custom",
        label: "Custom (OpenAI-compatible)",
        protocol: ProviderProtocol::OpenAiCompat,
        base_url: "",
        env_vars: &["YOMI_API_KEY", "OPENAI_API_KEY"],
        default_model: "",
        models: &[],
    },
];

pub fn find_provider(id: &str) -> Option<&'static ProviderInfo> {
    let low = id.trim().to_ascii_lowercase();
    API_PROVIDERS.iter().find(|p| p.id == low)
}

pub fn ollama_model_label(tag: &str) -> Option<&'static str> {
    match tag {
        "gemma4:12b" => Some("Local · Gemma 4 12B (Vision + Top Qualité)"),
        "gemma4:e4b" => Some("Local · Gemma 4 E4B (Vision · Mac 8 Go)"),
        "gemma4:e2b" => Some("Local · Gemma 4 E2B (Vision · Ultra-léger)"),
        "gemma4:26b" => Some("Local · Gemma 4 26B MoE (Vision · Mac 24 Go+)"),
        "gemma4:31b" => Some("Local · Gemma 4 31B (Vision · Mac 32 Go+)"),
        "gemma3:12b" => Some("Local · Gemma 3 12B (Vision)"),
        "gemma3:4b" => Some("Local · Gemma 3 4B (Vision · Rapide)"),
        _ => None,
    }
}

pub fn target_language_name(code: &str) -> String {
    let low = code.trim().to_ascii_lowercase();
    TARGET_LANGUAGES
        .iter()
        .find(|(k, _)| *k == low)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| low.to_uppercase())
}

pub fn target_languages_map() -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    for (k, v) in TARGET_LANGUAGES {
        m.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    m
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub onboarding_done: bool,
    #[serde(default = "default_ui_lang")]
    pub ui_lang: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_mascot")]
    pub mascot: String,
    #[serde(default = "default_engine_mode")]
    pub engine_mode: String,
    #[serde(default = "default_model_str")]
    pub default_model: String,
    #[serde(default = "default_provider")]
    pub api_provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_base_url: String,
    #[serde(default = "default_api_model")]
    pub api_model: String,
    #[serde(default = "default_source_lang")]
    pub source_lang: String,
    #[serde(default = "default_target_lang")]
    pub target_lang: String,
    #[serde(default = "default_vision_mode")]
    pub vision_mode: String,
    #[serde(default)]
    pub gemini_api_key: String,
    #[serde(default)]
    pub userscripts_dir: String,
}

fn default_ui_lang() -> String { "fr".to_string() }
fn default_theme() -> String { "system".to_string() }
fn default_mascot() -> String { "mochi".to_string() }
fn default_engine_mode() -> String { "local".to_string() }
fn default_model_str() -> String { "ollama:gemma4:12b".to_string() }
fn default_provider() -> String { "gemini".to_string() }
fn default_api_model() -> String { "gemini-3.8-flash".to_string() }
fn default_source_lang() -> String { "en".to_string() }
fn default_target_lang() -> String { "fr".to_string() }
fn default_vision_mode() -> String { "auto".to_string() }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            onboarding_done: false,
            ui_lang: default_ui_lang(),
            theme: default_theme(),
            mascot: default_mascot(),
            engine_mode: default_engine_mode(),
            default_model: default_model_str(),
            api_provider: default_provider(),
            api_key: String::new(),
            api_base_url: String::new(),
            api_model: default_api_model(),
            source_lang: default_source_lang(),
            target_lang: default_target_lang(),
            vision_mode: default_vision_mode(),
            gemini_api_key: String::new(),
            userscripts_dir: String::new(),
        }
    }
}

pub fn normalize_config(mut cfg: AppConfig) -> AppConfig {
    let prov_low = cfg.api_provider.trim().to_ascii_lowercase();
    cfg.api_provider = if find_provider(&prov_low).is_some() {
        prov_low
    } else {
        "gemini".to_string()
    };
    cfg.api_base_url = cfg.api_base_url.trim().trim_end_matches('/').to_string();
    cfg.api_model = cfg.api_model.trim().to_string();
    if cfg.api_model.is_empty() {
        cfg.api_model = find_provider(&cfg.api_provider)
            .map(|p| p.default_model.to_string())
            .unwrap_or_default();
    }
    if cfg.api_key.trim().is_empty() && cfg.api_provider == "gemini" && !cfg.gemini_api_key.trim().is_empty() {
        cfg.api_key = cfg.gemini_api_key.trim().to_string();
    }
    let src = cfg.source_lang.trim().to_ascii_lowercase();
    cfg.source_lang = if ["en", "ja", "auto"].contains(&src.as_str()) { src } else { "en".to_string() };
    let tgt = cfg.target_lang.trim().to_ascii_lowercase();
    cfg.target_lang = if TARGET_LANGUAGES.iter().any(|(k, _)| *k == tgt) { tgt } else { "fr".to_string() };
    let vis = cfg.vision_mode.trim().to_ascii_lowercase();
    cfg.vision_mode = if ["auto", "on", "off"].contains(&vis.as_str()) { vis } else { "auto".to_string() };
    cfg
}

pub fn resolve_data_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var("SMT_DATA_DIR") {
        let trimmed = env_dir.trim();
        if !trimmed.is_empty() {
            if let Some(rest) = trimmed.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    return home.join(rest);
                }
            }
            return PathBuf::from(trimmed);
        }
    }

    let base = dirs::data_dir().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share")
    });

    #[cfg(target_os = "macos")]
    {
        let yomi_dir = base.join("Yomi");
        let legacy_dir = base.join("MangaTranslator");
        if !yomi_dir.exists() && legacy_dir.exists() {
            let _ = fs::rename(&legacy_dir, &yomi_dir);
        }
        return yomi_dir;
    }

    #[cfg(target_os = "windows")]
    {
        return base.join("Yomi");
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        base.join("yomi")
    }
}

pub fn ensure_data_dirs(data_dir: &Path) {
    let _ = fs::create_dir_all(data_dir);
    let _ = fs::create_dir_all(data_dir.join("cache"));
    let _ = fs::create_dir_all(data_dir.join("glossaries"));
}

pub fn load_config(data_dir: &Path) -> AppConfig {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = data_dir.join("config.json");
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Ok(parsed) = serde_json::from_str::<AppConfig>(&raw) {
            return normalize_config(parsed);
        }
    }
    normalize_config(AppConfig::default())
}

pub fn save_config(data_dir: &Path, updates: &serde_json::Map<String, Value>) -> AppConfig {
    let _guard = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ensure_data_dirs(data_dir);
    let path = data_dir.join("config.json");
    let mut current = if let Ok(raw) = fs::read_to_string(&path) {
        serde_json::from_str::<AppConfig>(&raw).unwrap_or_default()
    } else {
        AppConfig::default()
    };
    let old_provider = current.api_provider.trim().to_ascii_lowercase();

    if let Some(v) = updates.get("onboarding_done").and_then(Value::as_bool) {
        current.onboarding_done = v;
    }
    if let Some(v) = updates.get("ui_lang").and_then(Value::as_str) {
        current.ui_lang = v.to_string();
    }
    if let Some(v) = updates.get("theme").and_then(Value::as_str) {
        current.theme = v.to_string();
    }
    if let Some(v) = updates.get("mascot").and_then(Value::as_str) {
        current.mascot = v.to_string();
    }
    if let Some(v) = updates.get("engine_mode").and_then(Value::as_str) {
        current.engine_mode = v.to_string();
    }
    if let Some(v) = updates.get("default_model").and_then(Value::as_str) {
        current.default_model = v.trim().to_string();
    }
    if let Some(v) = updates.get("api_provider").and_then(Value::as_str) {
        current.api_provider = v.to_string();
    }
    if let Some(v) = updates.get("api_base_url").and_then(Value::as_str) {
        current.api_base_url = v.to_string();
    }
    if let Some(v) = updates.get("api_model").and_then(Value::as_str) {
        current.api_model = v.to_string();
    }
    if let Some(v) = updates.get("source_lang").and_then(Value::as_str) {
        current.source_lang = v.to_string();
    }
    if let Some(v) = updates.get("target_lang").and_then(Value::as_str) {
        current.target_lang = v.to_string();
    }
    if let Some(v) = updates.get("vision_mode").and_then(Value::as_str) {
        current.vision_mode = v.to_string();
    }
    if let Some(v) = updates.get("userscripts_dir").and_then(Value::as_str) {
        current.userscripts_dir = v.trim().to_string();
    }

    let resolved_prov = {
        let p = current.api_provider.trim().to_ascii_lowercase();
        if find_provider(&p).is_some() { p } else { "gemini".to_string() }
    };
    current.api_provider = resolved_prov.clone();

    let new_api_key = updates
        .get("api_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let new_gem_key = updates
        .get("gemini_api_key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let effective_new_key = if !new_api_key.is_empty() { new_api_key } else { new_gem_key };

    let clear_key = updates.get("clear_api_key").and_then(Value::as_bool).unwrap_or(false);
    if clear_key {
        current.api_key.clear();
        current.gemini_api_key.clear();
    } else if !effective_new_key.is_empty() {
        current.api_key = effective_new_key.clone();
        if resolved_prov == "gemini" {
            current.gemini_api_key = effective_new_key;
        }
    } else if resolved_prov != old_provider {
        current.api_key = if resolved_prov == "gemini" {
            current.gemini_api_key.clone()
        } else {
            String::new()
        };
    }

    let normalized = normalize_config(current);
    if let Ok(json_str) = serde_json::to_string_pretty(&normalized) {
        let _ = fs::write(&path, json_str);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
    normalized
}

pub fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 8 {
        return "*".repeat(chars.len());
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{}...{}", prefix, suffix)
}

pub fn get_api_key(provider_override: Option<&str>, cfg: &AppConfig) -> String {
    let active_prov = {
        let p = cfg.api_provider.trim().to_ascii_lowercase();
        if find_provider(&p).is_some() { p } else { "gemini".to_string() }
    };
    let prov = match provider_override {
        Some(p) => {
            let low = p.trim().to_ascii_lowercase();
            if find_provider(&low).is_some() { low } else { active_prov.clone() }
        }
        None => active_prov.clone(),
    };

    if prov == active_prov {
        let k = cfg.api_key.trim();
        if !k.is_empty() {
            return k.to_string();
        }
    }
    if prov == "gemini" {
        let gk = cfg.gemini_api_key.trim();
        if !gk.is_empty() {
            return gk.to_string();
        }
    }

    if let Some(pinfo) = find_provider(&prov) {
        for &env_name in pinfo.env_vars {
            if env_name == "YOMI_API_KEY" && prov != active_prov {
                continue;
            }
            if let Ok(val) = std::env::var(env_name) {
                let t = val.trim();
                if !t.is_empty() {
                    return t.to_string();
                }
            }
        }
    }
    String::new()
}

pub fn public_config(cfg: &AppConfig) -> Value {
    let effective_key = get_api_key(None, cfg);
    let mut val = serde_json::to_value(cfg).unwrap_or_else(|_| Value::Object(Default::default()));
    if let Value::Object(ref mut map) = val {
        map.remove("gemini_api_key");
        map.remove("api_key");
        map.insert("has_api_key".to_string(), Value::Bool(!effective_key.is_empty()));
        map.insert("api_key_masked".to_string(), Value::String(mask_api_key(&effective_key)));
    }
    val
}

pub fn format_cloud_model_id(provider: &str, raw_model: &str) -> String {
    let prov = provider.trim().to_ascii_lowercase();
    let prov = if prov.is_empty() { "gemini".to_string() } else { prov };
    let mut raw = raw_model.trim().to_string();
    if raw.is_empty() {
        raw = find_provider(&prov)
            .map(|p| p.default_model.to_string())
            .unwrap_or_default();
    }
    if raw.is_empty() {
        return prov;
    }
    if prov == "gemini" {
        return raw.strip_prefix("gemini:").unwrap_or(&raw).to_string();
    }
    let prefix = format!("{}:", prov);
    if raw.starts_with(&prefix) {
        raw
    } else {
        format!("{}:{}", prov, raw)
    }
}

pub fn parse_model_spec(model: &str, cfg: &AppConfig) -> (String, String) {
    let spec = model.trim();
    if let Some(rest) = spec.strip_prefix("ollama:") {
        return ("ollama".to_string(), rest.trim().to_string());
    }
    if let Some((prefix, rest)) = spec.split_once(':') {
        let low = prefix.trim().to_ascii_lowercase();
        if find_provider(&low).is_some() {
            return (low, rest.trim().to_string());
        }
    }
    if spec.to_ascii_lowercase().starts_with("gemini-") {
        return ("gemini".to_string(), spec.to_string());
    }
    let prov = {
        let p = cfg.api_provider.trim().to_ascii_lowercase();
        if find_provider(&p).is_some() { p } else { "gemini".to_string() }
    };
    let raw = if spec.is_empty() {
        if !cfg.api_model.trim().is_empty() {
            cfg.api_model.trim().to_string()
        } else {
            find_provider(&prov)
                .map(|p| p.default_model.to_string())
                .unwrap_or_default()
        }
    } else {
        spec.to_string()
    };
    (prov, raw)
}

pub fn model_supports_vision(model: &str, cfg: &AppConfig) -> bool {
    if model.starts_with("ollama:") {
        let low = model.to_ascii_lowercase();
        return [
            "gemma4",
            "gemma3",
            "llava",
            "qwen2.5vl",
            "qwen3-vl",
            "minicpm-v",
            "llama3.2-vision",
            "vision",
        ]
        .iter()
        .any(|tag| low.contains(tag));
    }
    let (prov, raw_model) = parse_model_spec(model, cfg);
    let low_m = raw_model.to_ascii_lowercase();
    if prov == "deepseek" {
        return false;
    }
    if prov == "groq" {
        return low_m.contains("llama-4") || low_m.contains("vision");
    }
    if ["o1-mini", "deepseek-r1", "deepseek-chat", "deepseek-reasoner"]
        .iter()
        .any(|t| low_m.contains(t))
    {
        return false;
    }
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
}

fn push_local_model(
    seen_ids: &mut HashSet<String>,
    local_entries: &mut Vec<ModelEntry>,
    tag: &str,
    cfg: &AppConfig,
) {
    let mid = format!("ollama:{}", tag);
    if !seen_ids.insert(mid.clone()) {
        return;
    }
    let base_tag = tag.strip_suffix(":latest").unwrap_or(tag);
    let label = ollama_model_label(tag)
        .or_else(|| ollama_model_label(base_tag))
        .map(str::to_string)
        .unwrap_or_else(|| {
            let vis = if model_supports_vision(&mid, cfg) { "Vision" } else { "Texte" };
            format!("Local · {} ({})", tag, vis)
        });
    local_entries.push(ModelEntry { id: mid, label });
}

pub fn build_available_models(ollama_models: &[String], cfg: &AppConfig) -> Vec<ModelEntry> {
    let installed: Vec<String> = ollama_models
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut seen_ids = HashSet::new();
    let mut local_entries = Vec::new();

    for &preferred in DEFAULT_OLLAMA_ORDER {
        for inst in &installed {
            if inst == preferred || inst == &format!("{}:latest", preferred) {
                let chosen = if inst == &format!("{}:latest", preferred) {
                    preferred
                } else {
                    inst.as_str()
                };
                push_local_model(&mut seen_ids, &mut local_entries, chosen, cfg);
            }
        }
    }
    for inst in &installed {
        push_local_model(&mut seen_ids, &mut local_entries, inst, cfg);
    }

    if local_entries.is_empty() && cfg.default_model.starts_with("ollama:") {
        let tag = cfg.default_model.trim_start_matches("ollama:");
        push_local_model(&mut seen_ids, &mut local_entries, tag, cfg);
    }

    let prov = cfg.api_provider.trim().to_ascii_lowercase();
    let pinfo = find_provider(&prov).unwrap_or(&API_PROVIDERS[0]);
    let mut cloud_entries = Vec::new();
    let mut cloud_seen = HashSet::new();

    let mut add_cloud = |raw_id: &str, nice_label: Option<&str>| {
        if raw_id.is_empty() {
            return;
        }
        let full_id = format_cloud_model_id(&prov, raw_id);
        if !cloud_seen.insert(full_id.clone()) {
            return;
        }
        let prefix_lbl = if prov == "gemini" {
            "API Gemini".to_string()
        } else {
            format!("API {}", pinfo.label)
        };
        cloud_entries.push(ModelEntry {
            id: full_id,
            label: format!("{} · {}", prefix_lbl, nice_label.unwrap_or(raw_id)),
        });
    };

    let cfg_api_model = cfg.api_model.trim();
    if !cfg_api_model.is_empty() {
        let preset_match = pinfo
            .models
            .iter()
            .find(|(id, _)| *id == cfg_api_model)
            .map(|(_, lbl)| *lbl);
        add_cloud(cfg_api_model, preset_match);
    }
    for (id, lbl) in pinfo.models {
        add_cloud(id, Some(lbl));
    }

    if cloud_entries.is_empty() {
        for (id, lbl) in API_PROVIDERS[0].models {
            cloud_entries.push(ModelEntry {
                id: (*id).to_string(),
                label: format!("API Gemini · {}", lbl),
            });
        }
    }

    let mut out = local_entries;
    out.extend(cloud_entries);
    out
}

pub fn ollama_has_model(installed: &[String], model_tag: &str) -> bool {
    let norm = model_tag.strip_suffix(":latest").unwrap_or(model_tag);
    installed
        .iter()
        .any(|m| m == model_tag || m == norm || m == &format!("{}:latest", norm))
}

pub fn get_default_model(ollama_ok: bool, ollama_models: &[String], cfg: &AppConfig) -> String {
    if let Ok(env_model) = std::env::var("TRANSLATOR_MODEL") {
        let trimmed = env_model.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let cfg_model = cfg.default_model.trim();
    let prov = cfg.api_provider.trim().to_ascii_lowercase();
    let api_model = if !cfg.api_model.trim().is_empty() {
        cfg.api_model.trim().to_string()
    } else {
        find_provider(&prov)
            .map(|p| p.default_model.to_string())
            .unwrap_or_default()
    };
    let has_key = !get_api_key(Some(&prov), cfg).is_empty();

    if cfg.engine_mode == "api" && has_key {
        if !cfg_model.is_empty() && !cfg_model.starts_with("ollama:") {
            return cfg_model.to_string();
        }
        return format_cloud_model_id(&prov, &api_model);
    }

    if !cfg_model.is_empty() {
        if !cfg_model.starts_with("ollama:") && !ollama_ok && has_key {
            return cfg_model.to_string();
        } else if cfg_model.starts_with("ollama:") && ollama_ok {
            let tag = cfg_model.trim_start_matches("ollama:");
            if ollama_has_model(ollama_models, tag) {
                return cfg_model.to_string();
            }
        }
    }

    if ollama_ok {
        for &candidate in DEFAULT_OLLAMA_ORDER {
            if ollama_has_model(ollama_models, candidate) {
                return format!("ollama:{}", candidate);
            }
        }
        if let Some(first) = ollama_models.first() {
            return format!("ollama:{}", first);
        }
    }

    if has_key {
        return format_cloud_model_id(&prov, &api_model);
    }
    if !cfg_model.is_empty() {
        cfg_model.to_string()
    } else {
        "ollama:gemma4:12b".to_string()
    }
}

pub fn resolve_provider_base_url(
    provider: &str,
    base_url: &str,
    cfg: &AppConfig,
) -> Result<String, String> {
    let raw_in = base_url.trim();
    let mut raw = if !raw_in.is_empty() {
        raw_in.to_string()
    } else if provider == cfg.api_provider.trim().to_ascii_lowercase()
        && !cfg.api_base_url.trim().is_empty()
    {
        cfg.api_base_url.trim().to_string()
    } else {
        find_provider(provider)
            .map(|p| p.base_url.to_string())
            .unwrap_or_default()
    };
    raw = raw.trim_end_matches('/').to_string();
    let parsed = url::Url::parse(&raw)
        .map_err(|_| format!("URL de base invalide ou manquante pour le fournisseur '{}'.", provider))?;
    if !["http", "https"].contains(&parsed.scheme()) || parsed.host_str().is_none() {
        return Err(format!(
            "URL de base invalide ou manquante pour le fournisseur '{}'.",
            provider
        ));
    }
    Ok(raw)
}

pub fn load_dotenv(path: &Path) -> HashMap<String, String> {
    let mut loaded = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return loaded;
    };
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let mut val = v.trim();
        if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            val = &val[1..val.len() - 1];
        }
        if !key.is_empty() && std::env::var_os(key).is_none() {
            std::env::set_var(key, val);
            loaded.insert(key.to_string(), val.to_string());
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mask_api_key_redacts_cleanly() {
        assert_eq!(mask_api_key(""), "");
        assert_eq!(mask_api_key("123456"), "******");
        assert_eq!(mask_api_key("sk-proj-abcdefgh1234"), "sk-p...1234");
    }

    #[test]
    fn save_config_preserves_gemini_key_and_redacts_public() {
        let tmp = tempfile::tempdir().unwrap();
        let mut u1 = serde_json::Map::new();
        u1.insert("api_provider".into(), json!("gemini"));
        u1.insert("api_key".into(), json!("gem-test-key-123456"));
        let c1 = save_config(tmp.path(), &u1);
        assert_eq!(c1.api_key, "gem-test-key-123456");
        assert_eq!(c1.gemini_api_key, "gem-test-key-123456");

        let pub1 = public_config(&c1);
        assert!(pub1.get("api_key").is_none());
        assert!(pub1.get("gemini_api_key").is_none());
        assert_eq!(pub1["has_api_key"], json!(true));
        assert_eq!(pub1["api_key_masked"], json!("gem-...3456"));

        // Switching to openai clears active api_key so Gemini key doesn't bleed over
        let mut u2 = serde_json::Map::new();
        u2.insert("api_provider".into(), json!("openai"));
        u2.insert("api_model".into(), json!("gpt-4.1-mini"));
        let c2 = save_config(tmp.path(), &u2);
        assert_eq!(c2.api_provider, "openai");
        assert_eq!(c2.api_key, "");
        assert_eq!(c2.gemini_api_key, "gem-test-key-123456");
    }

    #[test]
    fn model_spec_parsing_and_vision_support() {
        let cfg = AppConfig::default();
        assert_eq!(parse_model_spec("ollama:gemma4:12b", &cfg), ("ollama".into(), "gemma4:12b".into()));
        assert_eq!(parse_model_spec("openai:gpt-4.1-mini", &cfg), ("openai".into(), "gpt-4.1-mini".into()));
        assert_eq!(parse_model_spec("gemini-3.8-flash", &cfg), ("gemini".into(), "gemini-3.8-flash".into()));
        assert!(model_supports_vision("ollama:gemma4:12b", &cfg));
        assert!(!model_supports_vision("deepseek:deepseek-chat", &cfg));
        assert!(model_supports_vision("groq:meta-llama/llama-4-scout-17b-16e-instruct", &cfg));
        assert!(!model_supports_vision("groq:llama-3.3-70b-versatile", &cfg));
    }
}
