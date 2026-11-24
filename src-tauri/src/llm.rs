use futures_util::StreamExt;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::config::{
    find_provider, get_api_key, model_supports_vision, parse_model_spec,
    resolve_provider_base_url, AppConfig, ProviderProtocol,
};
use crate::glossary::Glossary;
use crate::image_proc::Bubble;
use crate::prompts::{
    build_manga_system_prompt, build_user_prompt, can_skip_bubble, extract_json_object,
    parse_translation_items, PromptBubbleItem, TranslationItem,
};

pub fn ollama_base_url() -> String {
    std::env::var("OLLAMA_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string())
}

#[derive(Debug, Clone)]
pub enum TranslateError {
    RateLimit {
        message: String,
        retry_after: u64,
        is_daily: bool,
    },
    BadRequest(String),
    Runtime(String),
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimit { message, .. } => write!(f, "{}", message),
            Self::BadRequest(m) | Self::Runtime(m) => write!(f, "{}", m),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullProgressEvent {
    pub tool: String,
    pub model: String,
    pub status: String,
    pub completed: u64,
    pub total: u64,
    pub percent: i32,
}

fn http_client(timeout_secs: u64) -> Client {
    Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .unwrap_or_else(|_| Client::new())
}

pub async fn check_ollama_status_at(ollama_url: &str) -> (bool, Vec<String>) {
    let client = Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .unwrap_or_else(|_| Client::new());
    let url = format!("{}/api/tags", ollama_url.trim_end_matches('/'));
    let Ok(resp) = client.get(&url).send().await else {
        return (false, Vec::new());
    };
    if !resp.status().is_success() {
        return (false, Vec::new());
    }
    let Ok(val) = resp.json::<Value>().await else {
        return (false, Vec::new());
    };
    let names = val
        .get("models")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(Value::as_str))
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    (true, names)
}

pub async fn check_ollama_status() -> (bool, Vec<String>) {
    check_ollama_status_at(&ollama_base_url()).await
}

pub async fn warmup_ollama_model_at(ollama_url: &str, model: &str) {
    let Some(raw) = model.strip_prefix("ollama:") else {
        return;
    };
    let ollama_model = if raw.trim().is_empty() { "gemma4:12b" } else { raw.trim() };
    let client = http_client(60);
    let _ = client
        .post(format!("{}/api/generate", ollama_url.trim_end_matches('/')))
        .json(&json!({
            "model": ollama_model,
            "prompt": "",
            "keep_alive": "10m",
            "options": { "num_ctx": 4096 }
        }))
        .send()
        .await;
}

pub async fn unload_ollama_models_at(ollama_url: &str) -> Vec<String> {
    let mut unloaded = Vec::new();
    let client = http_client(3);
    let base = ollama_url.trim_end_matches('/');
    let Ok(resp) = client.get(format!("{}/api/ps", base)).send().await else {
        return unloaded;
    };
    let Ok(data) = resp.json::<Value>().await else {
        return unloaded;
    };
    let names: Vec<String> = data
        .get("models")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(Value::as_str))
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    for name in names {
        let res = http_client(5)
            .post(format!("{}/api/generate", base))
            .json(&json!({ "model": name, "prompt": "", "keep_alive": 0 }))
            .send()
            .await;
        if res.is_ok() {
            unloaded.push(name);
        }
    }
    unloaded
}

pub async fn pull_ollama_model(
    ollama_url: &str,
    model_name: &str,
    cancel: CancellationToken,
    mut on_progress: impl FnMut(PullProgressEvent),
) -> Result<(), String> {
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = tokio::select! {
        _ = cancel.cancelled() => return Err("Téléchargement annulé.".to_string()),
        res = client
            .post(format!("{}/api/pull", ollama_url.trim_end_matches('/')))
            .json(&json!({ "name": model_name, "stream": true }))
            .send() => res.map_err(|e| format!("Erreur réseau Ollama : {}", e))?,
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Ollama HTTP {} : {}", status.as_u16(), body));
    }

    let mut stream = resp.bytes_stream();
    let mut buffer = Vec::<u8>::new();
    loop {
        let next_chunk = tokio::select! {
            _ = cancel.cancelled() => return Err("Téléchargement annulé.".to_string()),
            c = stream.next() => c,
        };
        let Some(chunk_res) = next_chunk else {
            break;
        };
        let chunk = chunk_res.map_err(|e| format!("Erreur flux Ollama : {}", e))?;
        buffer.extend_from_slice(&chunk);

        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
            let line_str = String::from_utf8_lossy(&line_bytes).trim().to_string();
            if line_str.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<Value>(&line_str) {
                if let Some(err) = parsed.get("error").and_then(Value::as_str) {
                    return Err(err.to_string());
                }
                let status = parsed
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("Téléchargement...")
                    .to_string();
                let done = parsed.get("completed").and_then(Value::as_u64).unwrap_or(0);
                let total = parsed.get("total").and_then(Value::as_u64).unwrap_or(0);
                let pct = if total > 0 {
                    ((done as f64 / total as f64) * 100.0).round().min(100.0) as i32
                } else {
                    -1
                };
                on_progress(PullProgressEvent {
                    tool: "pull".to_string(),
                    model: model_name.to_string(),
                    status,
                    completed: done,
                    total,
                    percent: pct,
                });
            }
        }
    }
    Ok(())
}

pub async fn test_cloud_api_key(
    provider: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    cfg: &AppConfig,
) -> (bool, String) {
    let prov = {
        let p = provider.trim().to_ascii_lowercase();
        if p.is_empty() { "gemini".to_string() } else { p }
    };
    let Some(pinfo) = find_provider(&prov) else {
        return (false, format!("Fournisseur inconnu : {}", prov));
    };
    let key = api_key.trim();
    if key.is_empty() {
        return (false, "Clé API vide.".to_string());
    }
    let resolved_base = match resolve_provider_base_url(&prov, base_url, cfg) {
        Ok(b) => b,
        Err(e) => return (false, e),
    };
    let label = pinfo.label;
    let client = http_client(12);

    match pinfo.protocol {
        ProviderProtocol::Gemini => {
            let url = format!("{}/models?pageSize=1", resolved_base);
            match client.get(url).header("x-goog-api-key", key).send().await {
                Ok(resp) if resp.status().is_success() => (true, format!("Clé {} valide.", label)),
                Ok(resp) => (false, format!("Clé refusée ({}).", resp.status().as_u16())),
                Err(e) => (false, format!("Erreur réseau : {}", e)),
            }
        }
        ProviderProtocol::Anthropic => {
            let url = format!("{}/models?limit=1", resolved_base);
            let res = client
                .get(&url)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .await;
            match res {
                Ok(resp) if resp.status().is_success() => (true, format!("Clé {} valide.", label)),
                Ok(resp) if matches!(resp.status().as_u16(), 404 | 405) => {
                    let probe_model = if model.trim().is_empty() {
                        pinfo.default_model
                    } else {
                        model.trim()
                    };
                    let probe = client
                        .post(format!("{}/messages", resolved_base))
                        .header("x-api-key", key)
                        .header("anthropic-version", "2023-06-01")
                        .json(&json!({
                            "model": probe_model,
                            "max_tokens": 1,
                            "messages": [{ "role": "user", "content": "hi" }]
                        }))
                        .send()
                        .await;
                    match probe {
                        Ok(r2) if r2.status().is_success() => (true, format!("Clé {} valide.", label)),
                        Ok(r2) => (false, format!("Clé ou modèle refusé ({}).", r2.status().as_u16())),
                        Err(e) => (false, format!("Erreur réseau : {}", e)),
                    }
                }
                Ok(resp) => (false, format!("Clé refusée ({}).", resp.status().as_u16())),
                Err(e) => (false, format!("Erreur réseau : {}", e)),
            }
        }
        ProviderProtocol::OpenAiCompat => {
            let mut check_url = format!("{}/models", resolved_base);
            let mut req = client.get(&check_url).bearer_auth(key);
            if prov == "openrouter" {
                check_url = format!("{}/auth/key", resolved_base);
                req = client
                    .get(&check_url)
                    .bearer_auth(key)
                    .header("HTTP-Referer", "https://github.com/shishi09/yomi")
                    .header("X-Title", "Yomi");
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => (true, format!("Clé {} valide.", label)),
                Ok(resp) if matches!(resp.status().as_u16(), 404 | 405) => {
                    let probe_model = if !model.trim().is_empty() {
                        model.trim()
                    } else if !pinfo.default_model.is_empty() {
                        pinfo.default_model
                    } else {
                        "gpt-4o-mini"
                    };
                    let mut preq = client
                        .post(format!("{}/chat/completions", resolved_base))
                        .bearer_auth(key);
                    if prov == "openrouter" {
                        preq = preq
                            .header("HTTP-Referer", "https://github.com/shishi09/yomi")
                            .header("X-Title", "Yomi");
                    }
                    let probe = preq
                        .json(&json!({
                            "model": probe_model,
                            "max_tokens": 1,
                            "messages": [{ "role": "user", "content": "hi" }]
                        }))
                        .send()
                        .await;
                    match probe {
                        Ok(r2) if r2.status().is_success() => (true, format!("Clé {} valide.", label)),
                        Ok(r2) => (false, format!("Clé ou modèle refusé ({}).", r2.status().as_u16())),
                        Err(e) => (false, format!("Erreur réseau : {}", e)),
                    }
                }
                Ok(resp) => (false, format!("Clé refusée ({}).", resp.status().as_u16())),
                Err(e) => (false, format!("Erreur réseau : {}", e)),
            }
        }
    }
}

fn parse_retry_after_seconds(headers: &reqwest::header::HeaderMap, err_body: &str) -> u64 {
    if let Some(val) = headers.get("retry-after").and_then(|v| v.to_str().ok()) {
        if let Ok(sec) = val.trim().parse::<f64>() {
            return ((sec as u64) + 1).clamp(5, 90);
        }
    }
    let re1 = Regex::new(r"(?i)retry\s+(?:in|after)\s+(\d+(?:\.\d+)?)\s*s").unwrap();
    if let Some(caps) = re1.captures(err_body) {
        if let Ok(sec) = caps[1].parse::<f64>() {
            return ((sec as u64) + 2).clamp(5, 90);
        }
    }
    let re2 = Regex::new(r#""retryDelay"\s*:\s*"(\d+(?:\.\d+)?)s""#).unwrap();
    if let Some(caps) = re2.captures(err_body) {
        if let Ok(sec) = caps[1].parse::<f64>() {
            return ((sec as u64) + 2).clamp(5, 90);
        }
    }
    15
}

async fn send_ollama_chat(
    client: &Client,
    url: &str,
    ollama_model: &str,
    payload_val: &mut Value,
) -> Result<Value, TranslateError> {
    let resp = client.post(url).json(&*payload_val).send().await.map_err(|_| {
        TranslateError::Runtime(
            "Ollama local ne répond pas (port 11434). Lance 'ollama serve' ou vérifie les Réglages."
                .to_string(),
        )
    })?;
    let status = resp.status();
    if status.is_success() {
        return resp
            .json::<Value>()
            .await
            .map_err(|e| TranslateError::Runtime(format!("Réponse Ollama illisible : {}", e)));
    }
    let err_body = resp.text().await.unwrap_or_default();
    if status.as_u16() == 501
        && payload_val.get("format").is_some()
        && err_body.to_ascii_lowercase().contains("structured output")
    {
        if let Some(map) = payload_val.as_object_mut() {
            map.remove("format");
        }
        let retry_resp = client.post(url).json(&*payload_val).send().await.map_err(|e| {
            TranslateError::Runtime(format!("Erreur Ollama : {}", e))
        })?;
        let r_status = retry_resp.status();
        if r_status.is_success() {
            return retry_resp
                .json::<Value>()
                .await
                .map_err(|e| TranslateError::Runtime(format!("Réponse Ollama illisible : {}", e)));
        }
        let r_body = retry_resp.text().await.unwrap_or_default();
        return Err(TranslateError::Runtime(format!(
            "Erreur Ollama ({}) : {}",
            r_status.as_u16(),
            r_body.chars().take(240).collect::<String>()
        )));
    }
    if status.as_u16() == 404 || err_body.to_ascii_lowercase().contains("not found") {
        return Err(TranslateError::Runtime(format!(
            "Modèle local '{}' non téléchargé. Lance : ollama pull {}",
            ollama_model, ollama_model
        )));
    }
    Err(TranslateError::Runtime(format!(
        "Erreur Ollama ({}) : {}",
        status.as_u16(),
        err_body.chars().take(240).collect::<String>()
    )))
}

pub async fn translate_with_ollama_at(
    ollama_url: &str,
    bubbles: &[Bubble],
    model: &str,
    source_lang: &str,
    target_lang: &str,
    context_history: &[String],
    som_image_b64: Option<&str>,
    glossary: Option<&Glossary>,
    cfg: &AppConfig,
) -> Result<BTreeMap<usize, TranslationItem>, TranslateError> {
    if bubbles.is_empty() {
        return Ok(BTreeMap::new());
    }
    let raw_tag = model.strip_prefix("ollama:").unwrap_or(model).trim();
    let ollama_model = if raw_tag.is_empty() { "gemma4:12b" } else { raw_tag };
    let use_vision = som_image_b64.is_some() && model_supports_vision(model, cfg);
    let system_prompt = build_manga_system_prompt(source_lang, use_vision, glossary, target_lang);
    let payload_items: Vec<PromptBubbleItem<'_>> = bubbles
        .iter()
        .map(|b| PromptBubbleItem { id: b.id, text: &b.text })
        .collect();
    let user_text = build_user_prompt(&payload_items, context_history, use_vision, target_lang);

    let mut user_msg = json!({ "role": "user", "content": user_text });
    if use_vision {
        if let Some(b64) = som_image_b64 {
            user_msg["images"] = json!([b64]);
        }
    }

    let mut body = json!({
        "model": ollama_model,
        "stream": false,
        "keep_alive": "10m",
        "options": { "temperature": 0.45, "top_p": 0.9, "num_ctx": 4096 },
        "messages": [
            { "role": "system", "content": system_prompt },
            user_msg
        ]
    });
    if ollama_model.starts_with("gemma4") {
        body["think"] = json!(false);
    } else {
        body["format"] = json!({
            "type": "object",
            "properties": {
                "translations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "who": { "type": "string" },
                            "fr": { "type": "string" },
                            "skip": { "type": "boolean" }
                        },
                        "required": ["id", "who", "fr"]
                    }
                }
            },
            "required": ["translations"]
        });
    }

    let client = http_client(180);
    let url = format!("{}/api/chat", ollama_url.trim_end_matches('/'));

    let mut translations = BTreeMap::new();
    let mut last_err: Option<String> = None;

    for attempt in 0..2 {
        let mut req_body = body.clone();
        if attempt == 1 {
            let strict_prompt = format!(
                "{}\nIMPORTANT : Retourne UNIQUEMENT un JSON strictement valide \
                 (guillemets doubles obligatoires pour toutes les clés, aucune virgule en trop avant }} ou ]).",
                system_prompt
            );
            req_body["options"]["temperature"] = json!(0.15);
            req_body["messages"][0]["content"] = json!(strict_prompt);
        }
        let raw = send_ollama_chat(&client, &url, ollama_model, &mut req_body).await?;
        let content = raw
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(TranslateError::Runtime("Ollama n'a renvoyé aucun texte.".to_string()));
        }
        match extract_json_object(content) {
            Ok(parsed) => {
                translations = parse_translation_items(parsed.get("translations"));
                if !translations.is_empty() {
                    break;
                }
                last_err = Some("Ollama n'a renvoyé aucune traduction exploitable.".to_string());
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    if translations.is_empty() {
        return Err(TranslateError::Runtime(
            last_err.unwrap_or_else(|| "Ollama n'a renvoyé aucune traduction exploitable.".to_string()),
        ));
    }

    // Passe de rattrapage texte seul si certaines bulles manquent
    let missing: Vec<&Bubble> = bubbles
        .iter()
        .filter(|b| {
            let Some(tr) = translations.get(&b.id) else {
                return true;
            };
            if tr.skip && can_skip_bubble(b) {
                return false;
            }
            tr.fr.trim().is_empty()
        })
        .collect();

    if !missing.is_empty() {
        let mut page_ctx: Vec<String> = context_history.to_vec();
        for b in bubbles {
            if let Some(tr) = translations.get(&b.id) {
                if !tr.skip && !tr.fr.trim().is_empty() {
                    page_ctx.push(tr.fr.trim().to_string());
                }
            }
        }
        let rec_items: Vec<PromptBubbleItem<'_>> = missing
            .iter()
            .map(|b| PromptBubbleItem { id: b.id, text: &b.text })
            .collect();
        let rec_sys = build_manga_system_prompt(source_lang, false, glossary, target_lang);
        let rec_user = build_user_prompt(&rec_items, &page_ctx, false, target_lang);
        let mut rec_body = body.clone();
        rec_body["options"]["temperature"] = json!(0.2);
        rec_body["messages"] = json!([
            { "role": "system", "content": rec_sys },
            { "role": "user", "content": rec_user }
        ]);
        if let Ok(rec_raw) = send_ollama_chat(&client, &url, ollama_model, &mut rec_body).await {
            let rec_txt = rec_raw
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if !rec_txt.is_empty() {
                if let Ok(rec_parsed) = extract_json_object(rec_txt) {
                    for (bid, item) in parse_translation_items(rec_parsed.get("translations")) {
                        if !item.fr.is_empty() || item.skip {
                            translations.insert(bid, item);
                        }
                    }
                }
            }
        }
    }

    Ok(translations)
}

pub async fn translate_with_openai_compat(
    bubbles: &[Bubble],
    provider: &str,
    model_id: &str,
    api_key: &str,
    base_url: &str,
    source_lang: &str,
    target_lang: &str,
    context_history: &[String],
    som_image_b64: Option<&str>,
    glossary: Option<&Glossary>,
    cfg: &AppConfig,
) -> Result<BTreeMap<usize, TranslationItem>, TranslateError> {
    if bubbles.is_empty() {
        return Ok(BTreeMap::new());
    }
    let label = find_provider(provider)
        .map(|p| p.label)
        .unwrap_or("Custom API");
    let full_spec = format!("{}:{}", provider, model_id);
    let mut use_vision = som_image_b64.is_some() && model_supports_vision(&full_spec, cfg);
    let mut include_rf = true;

    let build_body = |with_rf: bool, with_img: bool| {
        let sys_prompt = build_manga_system_prompt(source_lang, with_img, glossary, target_lang);
        let items: Vec<PromptBubbleItem<'_>> = bubbles
            .iter()
            .map(|b| PromptBubbleItem { id: b.id, text: &b.text })
            .collect();
        let u_text = build_user_prompt(&items, context_history, with_img, target_lang);
        let user_content = if with_img {
            if let Some(b64) = som_image_b64 {
                json!([
                    { "type": "text", "text": u_text },
                    { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", b64) } }
                ])
            } else {
                Value::String(u_text)
            }
        } else {
            Value::String(u_text)
        };
        let mut body = json!({
            "model": model_id,
            "temperature": 0.45,
            "messages": [
                { "role": "system", "content": sys_prompt },
                { "role": "user", "content": user_content }
            ]
        });
        if with_rf {
            body["response_format"] = json!({ "type": "json_object" });
        }
        body
    };

    let client = http_client(60);
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut raw_opt: Option<Value> = None;

    for _attempt in 0..3 {
        let body = build_body(include_rf, use_vision);
        let mut req = client.post(&url).bearer_auth(api_key).json(&body);
        if provider == "openrouter" {
            req = req
                .header("HTTP-Referer", "https://github.com/shishi09/yomi")
                .header("X-Title", "Yomi");
        }
        let resp = req
            .send()
            .await
            .map_err(|e| TranslateError::Runtime(format!("Impossible de joindre l'API {} : {}", label, e)))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if status.is_success() {
            raw_opt = Some(
                resp.json::<Value>()
                    .await
                    .map_err(|e| TranslateError::Runtime(format!("Réponse {} invalide : {}", label, e)))?,
            );
            break;
        }
        let err_body = resp.text().await.unwrap_or_default();
        let low = err_body.to_ascii_lowercase();
        let code = status.as_u16();
        if matches!(code, 400 | 422) {
            if include_rf
                && ["response_format", "json_object", "structured", "json mode"]
                    .iter()
                    .any(|k| low.contains(k))
            {
                include_rf = false;
                continue;
            }
            if use_vision
                && ["image", "vision", "multimodal", "image_url", "content[1]"]
                    .iter()
                    .any(|k| low.contains(k))
            {
                use_vision = false;
                continue;
            }
        }
        if code == 429 {
            let retry_after = parse_retry_after_seconds(&headers, &err_body);
            return Err(TranslateError::RateLimit {
                message: format!(
                    "Limite de requêtes (429) sur {} ({}, reprise dans {}s).",
                    label, model_id, retry_after
                ),
                retry_after,
                is_daily: low.contains("perday") || low.contains("daily"),
            });
        }
        if matches!(code, 401 | 403) {
            return Err(TranslateError::Runtime(format!(
                "Clé API {} invalide ou non autorisée ({}) : {}",
                label,
                code,
                err_body.chars().take(240).collect::<String>()
            )));
        }
        if code == 404 {
            return Err(TranslateError::Runtime(format!(
                "Modèle '{}' ou endpoint introuvable sur {} (404).",
                model_id, label
            )));
        }
        return Err(TranslateError::Runtime(format!(
            "Erreur API {} ({}) : {}",
            label,
            code,
            err_body.chars().take(260).collect::<String>()
        )));
    }

    let raw = raw_opt.ok_or_else(|| TranslateError::Runtime(format!("Réponse invalide de {}.", label)))?;
    let choice0 = raw
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .ok_or_else(|| TranslateError::Runtime(format!("Réponse {} vide (aucun choix retourné).", label)))?;
    let msg_content = choice0.pointer("/message/content").unwrap_or(&Value::Null);
    let text_out = match msg_content {
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string(),
        Value::String(s) => s.trim().to_string(),
        _ => String::new(),
    };
    if text_out.is_empty() {
        return Err(TranslateError::Runtime(format!("{} n'a renvoyé aucun texte.", label)));
    }
    let parsed = extract_json_object(&text_out).map_err(TranslateError::Runtime)?;
    Ok(parse_translation_items(parsed.get("translations")))
}

pub async fn translate_with_anthropic(
    bubbles: &[Bubble],
    model_id: &str,
    api_key: &str,
    base_url: &str,
    source_lang: &str,
    target_lang: &str,
    context_history: &[String],
    som_image_b64: Option<&str>,
    glossary: Option<&Glossary>,
    cfg: &AppConfig,
) -> Result<BTreeMap<usize, TranslationItem>, TranslateError> {
    if bubbles.is_empty() {
        return Ok(BTreeMap::new());
    }
    let full_spec = format!("anthropic:{}", model_id);
    let mut use_vision = som_image_b64.is_some() && model_supports_vision(&full_spec, cfg);
    let client = http_client(60);
    let url = format!("{}/messages", base_url.trim_end_matches('/'));
    let mut raw_opt: Option<Value> = None;

    for _attempt in 0..2 {
        let sys_prompt = build_manga_system_prompt(source_lang, use_vision, glossary, target_lang);
        let items: Vec<PromptBubbleItem<'_>> = bubbles
            .iter()
            .map(|b| PromptBubbleItem { id: b.id, text: &b.text })
            .collect();
        let u_text = build_user_prompt(&items, context_history, use_vision, target_lang);
        let mut blocks = Vec::new();
        if use_vision {
            if let Some(b64) = som_image_b64 {
                blocks.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": b64
                    }
                }));
            }
        }
        blocks.push(json!({ "type": "text", "text": u_text }));
        let body = json!({
            "model": model_id,
            "max_tokens": 4096,
            "temperature": 0.45,
            "system": sys_prompt,
            "messages": [{ "role": "user", "content": blocks }]
        });

        let resp = client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| TranslateError::Runtime(format!("Impossible de joindre l'API Anthropic : {}", e)))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if status.is_success() {
            raw_opt = Some(
                resp.json::<Value>()
                    .await
                    .map_err(|e| TranslateError::Runtime(format!("Réponse Anthropic invalide : {}", e)))?,
            );
            break;
        }
        let err_body = resp.text().await.unwrap_or_default();
        let low = err_body.to_ascii_lowercase();
        let code = status.as_u16();
        if code == 400
            && use_vision
            && ["image", "vision", "media_type"].iter().any(|k| low.contains(k))
        {
            use_vision = false;
            continue;
        }
        if code == 429 {
            let retry_after = parse_retry_after_seconds(&headers, &err_body);
            return Err(TranslateError::RateLimit {
                message: format!(
                    "Limite de requêtes (429) sur Anthropic ({}, reprise dans {}s).",
                    model_id, retry_after
                ),
                retry_after,
                is_daily: false,
            });
        }
        if matches!(code, 401 | 403) {
            return Err(TranslateError::Runtime(format!(
                "Clé API Anthropic invalide ou non autorisée ({}) : {}",
                code,
                err_body.chars().take(240).collect::<String>()
            )));
        }
        if code == 404 {
            return Err(TranslateError::Runtime(format!(
                "Modèle '{}' introuvable sur Anthropic (404).",
                model_id
            )));
        }
        return Err(TranslateError::Runtime(format!(
            "Erreur API Anthropic ({}) : {}",
            code,
            err_body.chars().take(260).collect::<String>()
        )));
    }

    let raw = raw_opt.ok_or_else(|| TranslateError::Runtime("Réponse Anthropic invalide.".to_string()))?;
    let text_out = raw
        .get("content")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
        .trim()
        .to_string();
    if text_out.is_empty() {
        return Err(TranslateError::Runtime("Anthropic n'a renvoyé aucun texte.".to_string()));
    }
    let parsed = extract_json_object(&text_out).map_err(TranslateError::Runtime)?;
    Ok(parse_translation_items(parsed.get("translations")))
}

pub async fn translate_with_gemini(
    bubbles: &[Bubble],
    model: &str,
    api_key: &str,
    base_url: &str,
    source_lang: &str,
    target_lang: &str,
    context_history: &[String],
    som_image_b64: Option<&str>,
    glossary: Option<&Glossary>,
    cfg: &AppConfig,
) -> Result<BTreeMap<usize, TranslationItem>, TranslateError> {
    let use_vision = som_image_b64.is_some() && model_supports_vision(model, cfg);
    let system_prompt = build_manga_system_prompt(source_lang, use_vision, glossary, target_lang);
    let payload_items: Vec<PromptBubbleItem<'_>> = bubbles
        .iter()
        .map(|b| PromptBubbleItem { id: b.id, text: &b.text })
        .collect();
    let user_text = build_user_prompt(&payload_items, context_history, use_vision, target_lang);

    let mut user_parts = vec![json!({ "text": user_text })];
    if use_vision {
        if let Some(b64) = som_image_b64 {
            user_parts.push(json!({
                "inlineData": { "mimeType": "image/jpeg", "data": b64 }
            }));
        }
    }

    let body = json!({
        "systemInstruction": { "parts": [{ "text": system_prompt }] },
        "contents": [{ "role": "user", "parts": user_parts }],
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
                                "id": { "type": "INTEGER" },
                                "who": { "type": "STRING" },
                                "fr": { "type": "STRING" },
                                "skip": { "type": "BOOLEAN" }
                            },
                            "required": ["id", "who", "fr"]
                        }
                    }
                },
                "required": ["translations"]
            }
        }
    });

    let encoded_model: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect();
    let url = format!("{}/models/{}:generateContent", base_url.trim_end_matches('/'), encoded_model);
    let client = http_client(45);

    let resp = client
        .post(&url)
        .header("x-goog-api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| TranslateError::Runtime(format!("Impossible de joindre l'API Gemini : {}", e)))?;
    let status = resp.status();
    let headers = resp.headers().clone();
    if !status.is_success() {
        let err_body = resp.text().await.unwrap_or_default();
        let code = status.as_u16();
        if code == 429 {
            let retry_after = parse_retry_after_seconds(&headers, &err_body);
            let low = err_body.to_ascii_lowercase();
            let is_daily = low.contains("perday")
                || low.contains("per day")
                || low.contains("daily")
                || low.contains("generaterequestsperday");
            let msg = if is_daily {
                format!(
                    "Quota journalier gratuit atteint sur {}. Bascule sur gemini-3.5-flash-lite ou gemini-3.6-flash dans le menu.",
                    model
                )
            } else {
                format!(
                    "Limite par minute (429) atteinte sur {} (reprise dans {}s).",
                    model, retry_after
                )
            };
            return Err(TranslateError::RateLimit { message: msg, retry_after, is_daily });
        }
        if matches!(code, 400 | 401 | 403) {
            return Err(TranslateError::Runtime(format!(
                "Clé API Gemini invalide ou non autorisée ({}). Détail : {}",
                code,
                err_body.chars().take(240).collect::<String>()
            )));
        }
        if code == 404 {
            return Err(TranslateError::Runtime(format!(
                "Modèle '{}' introuvable (404). Choisis un autre modèle dans le menu.",
                model
            )));
        }
        return Err(TranslateError::Runtime(format!(
            "Erreur API Gemini ({}) : {}",
            code,
            err_body.chars().take(260).collect::<String>()
        )));
    }

    let raw = resp
        .json::<Value>()
        .await
        .map_err(|e| TranslateError::Runtime(format!("Réponse Gemini invalide : {}", e)))?;
    let text_out = raw
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
        .trim()
        .to_string();

    if text_out.is_empty() {
        return Err(TranslateError::Runtime(
            "Gemini n'a renvoyé aucun texte (peut-être filtré par sécurité).".to_string(),
        ));
    }
    let parsed = extract_json_object(&text_out).map_err(TranslateError::Runtime)?;
    Ok(parse_translation_items(parsed.get("translations")))
}

pub async fn translate_bubbles(
    bubbles: &[Bubble],
    model: &str,
    source_lang: &str,
    target_lang: &str,
    context_history: &[String],
    som_image_b64: Option<&str>,
    glossary: Option<&Glossary>,
    cfg: &AppConfig,
    ollama_url: &str,
) -> Result<BTreeMap<usize, TranslationItem>, TranslateError> {
    let (provider, raw_model) = parse_model_spec(model, cfg);
    if provider == "ollama" {
        return translate_with_ollama_at(
            ollama_url,
            bubbles,
            &format!("ollama:{}", raw_model),
            source_lang,
            target_lang,
            context_history,
            som_image_b64,
            glossary,
            cfg,
        )
        .await;
    }
    if bubbles.is_empty() {
        return Ok(BTreeMap::new());
    }
    let pinfo = find_provider(&provider).unwrap_or(&crate::config::API_PROVIDERS[0]);
    let api_key = get_api_key(Some(&provider), cfg);
    if api_key.is_empty() {
        return Err(TranslateError::BadRequest(format!(
            "Clé API manquante pour {} : configure-la dans Yomi (Paramètres → Moteur & Modèles) ou choisis Ollama local.",
            pinfo.label
        )));
    }
    let base_url = resolve_provider_base_url(&provider, "", cfg).map_err(TranslateError::BadRequest)?;
    match pinfo.protocol {
        ProviderProtocol::Anthropic => {
            translate_with_anthropic(
                bubbles,
                &raw_model,
                &api_key,
                &base_url,
                source_lang,
                target_lang,
                context_history,
                som_image_b64,
                glossary,
                cfg,
            )
            .await
        }
        ProviderProtocol::OpenAiCompat => {
            translate_with_openai_compat(
                bubbles,
                &provider,
                &raw_model,
                &api_key,
                &base_url,
                source_lang,
                target_lang,
                context_history,
                som_image_b64,
                glossary,
                cfg,
            )
            .await
        }
        ProviderProtocol::Gemini => {
            translate_with_gemini(
                bubbles,
                &raw_model,
                &api_key,
                &base_url,
                source_lang,
                target_lang,
                context_history,
                som_image_b64,
                glossary,
                cfg,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockStats {
        calls: usize,
        saw_openrouter_referer: bool,
    }

    async fn start_mock_server() -> (String, Arc<Mutex<MockStats>>) {
        let stats = Arc::new(Mutex::new(MockStats::default()));
        let app = Router::new()
            .route(
                "/openai/chat/completions",
                post(|State(st): State<Arc<Mutex<MockStats>>>, Json(body): Json<Value>| async move {
                    let mut guard = st.lock().unwrap();
                    guard.calls += 1;
                    if body.get("response_format").is_some() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": { "message": "response_format is not supported" } })),
                        );
                    }
                    (
                        StatusCode::OK,
                        Json(json!({
                            "choices": [{
                                "message": {
                                    "content": "{\"translations\":[{\"id\":0,\"who\":\"Komi\",\"fr\":\"Salut !\"}]}"
                                }
                            }]
                        })),
                    )
                }),
            )
            .route(
                "/openrouter/auth/key",
                get(|State(st): State<Arc<Mutex<MockStats>>>, headers: HeaderMap| async move {
                    let ok = headers
                        .get("http-referer")
                        .and_then(|v| v.to_str().ok())
                        == Some("https://github.com/shishi09/yomi");
                    st.lock().unwrap().saw_openrouter_referer = ok;
                    Json(json!({ "data": { "label": "ok" } }))
                }),
            )
            .route(
                "/anthropic/messages",
                post(|Json(_body): Json<Value>| async move {
                    Json(json!({
                        "content": [{
                            "type": "text",
                            "text": "{\"translations\":[{\"id\":0,\"who\":\"Tadano\",\"fr\":\"Bonjour !\"}]}"
                        }]
                    }))
                }),
            )
            .route(
                "/gemini/models/gemini-3.8-flash:generateContent",
                post(|Json(_body): Json<Value>| async move {
                    Json(json!({
                        "candidates": [{
                            "content": {
                                "parts": [{ "text": "{\"translations\":[{\"id\":0,\"who\":\"Komi\",\"fr\":\"Coucou !\"}]}" }]
                            }
                        }]
                    }))
                }),
            )
            .route(
                "/ollama/api/chat",
                post(|Json(body): Json<Value>| async move {
                    if body.get("format").is_some() {
                        return (
                            StatusCode::NOT_IMPLEMENTED,
                            Json(json!({ "error": "structured output is unavailable" })),
                        );
                    }
                    (
                        StatusCode::OK,
                        Json(json!({
                            "message": {
                                "content": "{\"translations\":[{\"id\":0,\"who\":\"Komi\",\"fr\":\"Salut local !\"}]}"
                            }
                        })),
                    )
                }),
            )
            .with_state(stats.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let _ = Body::empty();
        (format!("http://{}", addr), stats)
    }

    fn sample_bubble() -> Vec<Bubble> {
        vec![Bubble {
            id: 0,
            text: "HELLO!".into(),
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.1,
            line_count: 1,
            min_conf: 0.99,
            style: "bubble_light".into(),
            fr: None,
            who: None,
        }]
    }

    #[tokio::test]
    async fn openai_compat_400_fallback_anthropic_gemini_and_openrouter_key_test() {
        let (base, stats) = start_mock_server().await;
        let cfg = AppConfig::default();

        // 1. OpenAI-compat strips response_format on 400 and retries
        let oai = translate_with_openai_compat(
            &sample_bubble(),
            "openai",
            "gpt-4.1-mini",
            "sk-test",
            &format!("{}/openai", base),
            "en",
            "fr",
            &[],
            None,
            None,
            &cfg,
        )
        .await
        .unwrap();
        assert_eq!(oai[&0].fr, "Salut !");
        assert_eq!(stats.lock().unwrap().calls, 2);

        // 2. Anthropic native /messages
        let ant = translate_with_anthropic(
            &sample_bubble(),
            "claude-sonnet-4-5",
            "sk-ant-test",
            &format!("{}/anthropic", base),
            "en",
            "fr",
            &[],
            None,
            None,
            &cfg,
        )
        .await
        .unwrap();
        assert_eq!(ant[&0].fr, "Bonjour !");

        // 3. Gemini native :generateContent
        let gem = translate_with_gemini(
            &sample_bubble(),
            "gemini-3.8-flash",
            "AIzaTest",
            &format!("{}/gemini", base),
            "en",
            "fr",
            &[],
            None,
            None,
            &cfg,
        )
        .await
        .unwrap();
        assert_eq!(gem[&0].fr, "Coucou !");

        // 4. OpenRouter key check uses /auth/key + shishi09/yomi HTTP-Referer
        let (ok, _) = test_cloud_api_key(
            "openrouter",
            "sk-or-test",
            &format!("{}/openrouter", base),
            "",
            &cfg,
        )
        .await;
        assert!(ok);
        assert!(stats.lock().unwrap().saw_openrouter_referer);

        // 5. Local Ollama with HTTP 501 format fallback (non-gemma4 tag exercises format stripping)
        let oll = translate_with_ollama_at(
            &format!("{}/ollama", base),
            &sample_bubble(),
            "ollama:llama3.2-vision",
            "en",
            "fr",
            &[],
            None,
            None,
            &cfg,
        )
        .await
        .unwrap();
        assert_eq!(oll[&0].fr, "Salut local !");
    }
}
