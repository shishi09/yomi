use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderName, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::watch;
use tower_http::cors::CorsLayer;

use crate::cache::{
    clear_disk_cache, decode_image_payload, find_disk_cache_hit, get_cache_path,
    schedule_prefetch_next, CachedPageResult, ImagePayloadInput, PrefetchCache, MAX_IMAGE_BYTES,
};
use crate::config::{
    build_available_models, get_api_key, get_default_model, load_config, public_config,
    save_config, target_languages_map, AppConfig, find_provider,
};
use crate::glossary::{
    compute_glossary_hash, delete_glossary, list_glossaries, resolve_glossary, save_glossary,
    Glossary,
};
use crate::image_proc::{
    group_lines_into_bubbles, is_punctuation_only, normalize_vision_mode, should_use_vision,
    Bubble, OcrResult,
};
use crate::llm::{
    check_ollama_status_at, test_cloud_api_key, translate_bubbles, unload_ollama_models_at,
    TranslateError,
};
use crate::ocr::active_ocr_engine;
use crate::prompts::{can_skip_bubble, TranslationItem};
use crate::som::create_som_image_base64;

pub const EMBEDDED_USERSCRIPT: &str = include_str!("../../userscript/yomi.user.js");

pub struct ServerState {
    pub data_dir: PathBuf,
    pub resource_dir: Option<PathBuf>,
    pub prefetch: Arc<PrefetchCache>,
    pub ollama_url: String,
}

impl ServerState {
    pub fn resolve_userscript_text(&self) -> String {
        if let Ok(env_p) = std::env::var("SMT_USERSCRIPT_PATH") {
            let p = PathBuf::from(env_p.trim());
            if let Ok(txt) = fs::read_to_string(&p) {
                return txt;
            }
        }
        if let Some(ref res) = self.resource_dir {
            for rel in ["userscript/yomi.user.js", "yomi.user.js"] {
                if let Ok(txt) = fs::read_to_string(res.join(rel)) {
                    return txt;
                }
            }
        }
        EMBEDDED_USERSCRIPT.to_string()
    }
}

pub struct ServerController {
    shutdown_tx: Mutex<Option<watch::Sender<bool>>>,
    running: Arc<AtomicBool>,
    pub port: u16,
}

impl ServerController {
    pub fn new(port: u16) -> Self {
        Self {
            shutdown_tx: Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
            port,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub async fn start(&self, state: Arc<ServerState>) -> Result<(), String> {
        if self.is_running() {
            return Ok(());
        }
        let addr = format!("127.0.0.1:{}", self.port);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Impossible d'écouter sur {} : {}", addr, e))?;

        let (tx, mut rx) = watch::channel(false);
        *self.shutdown_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        self.running.store(true, Ordering::SeqCst);

        let running_flag = self.running.clone();
        let router = build_router(state);

        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    while !*rx.borrow() {
                        if rx.changed().await.is_err() {
                            break;
                        }
                    }
                })
                .await;
            running_flag.store(false, Ordering::SeqCst);
        });

        Ok(())
    }

    pub async fn stop(&self) {
        let tx_opt = self.shutdown_tx.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(tx) = tx_opt {
            let _ = tx.send(true);
        }
        for _ in 0..25 {
            if !self.is_running() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        self.running.store(false, Ordering::SeqCst);
    }

    pub async fn restart(&self, state: Arc<ServerState>) -> Result<(), String> {
        self.stop().await;
        self.start(state).await
    }
}

async fn private_network_middleware(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        HeaderName::from_static("access-control-allow-private-network"),
        HeaderValue::from_static("true"),
    );
    resp
}

pub fn build_router(state: Arc<ServerState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(tower_http::cors::Any);

    Router::new()
        .route("/api/glossaries", get(get_glossaries).post(post_glossary))
        .route("/api/glossaries/delete", post(post_delete_glossary))
        .route("/api/config", get(get_config).post(post_config))
        .route("/api/health", get(get_health))
        .route("/yomi.user.js", get(get_userscript))
        .route("/userscript", get(get_userscript))
        .route("/manga-translator.user.js", get(get_userscript))
        .route("/install.user.js", get(get_userscript))
        .route("/api/test-gemini-key", post(post_test_api_key))
        .route("/api/test-api-key", post(post_test_api_key))
        .route("/api/unload", post(post_unload))
        .route("/api/clear-cache", post(post_clear_cache))
        .route("/api/translate", post(post_translate))
        .route("/api/translate-batch", post(post_translate_batch))
        .route("/", get(get_index))
        .layer(DefaultBodyLimit::max(MAX_IMAGE_BYTES * 4))
        .layer(middleware::from_fn(private_network_middleware))
        .layer(cors)
        .with_state(state)
}

#[derive(Deserialize)]
struct GlossariesQuery {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
}

async fn get_glossaries(
    State(state): State<Arc<ServerState>>,
    Query(q): Query<GlossariesQuery>,
) -> Json<Value> {
    let glossaries = list_glossaries(&state.data_dir);
    let detected = resolve_glossary(&state.data_dir, "auto", &q.url, &q.title);
    Json(json!({
        "glossaries": glossaries,
        "detected_slug": detected.map(|g| g.slug),
    }))
}

async fn get_config(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let cfg = load_config(&state.data_dir);
    Json(public_config(&cfg))
}

async fn post_config(
    State(state): State<Arc<ServerState>>,
    Json(updates): Json<serde_json::Map<String, Value>>,
) -> Json<Value> {
    let saved = save_config(&state.data_dir, &updates);
    Json(json!({
        "ok": true,
        "config": public_config(&saved),
    }))
}

pub async fn build_health_payload(state: &ServerState) -> Value {
    let cfg = load_config(&state.data_dir);
    let prov = cfg.api_provider.trim().to_ascii_lowercase();
    let pinfo = find_provider(&prov).unwrap_or(&crate::config::API_PROVIDERS[0]);
    let (ollama_ok, ollama_models) = check_ollama_status_at(&state.ollama_url).await;
    let models_list = build_available_models(&ollama_models, &cfg);
    let default_model = get_default_model(ollama_ok, &ollama_models, &cfg);
    let has_key = !get_api_key(Some(&prov), &cfg).is_empty();
    let ocr = active_ocr_engine();

    json!({
        "ok": true,
        "api_key_configured": ollama_ok || has_key,
        "has_api_key": has_key,
        "api_provider": prov,
        "api_provider_label": pinfo.label,
        "ollama_ready": ollama_ok,
        "ollama_models": ollama_models,
        "default_model": default_model,
        "models": models_list,
        "source_lang": cfg.source_lang,
        "target_lang": cfg.target_lang,
        "vision_mode": cfg.vision_mode,
        "theme": cfg.theme,
        "mascot": cfg.mascot,
        "target_languages": Value::Object(target_languages_map()),
        "ocr_ready": ocr.is_ready(state.resource_dir.as_ref()),
        "ocr_engine": ocr.name(),
        "native_core": true,
    })
}

async fn get_health(State(state): State<Arc<ServerState>>) -> Json<Value> {
    Json(build_health_payload(&state).await)
}

async fn get_userscript(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let txt = state.resolve_userscript_text();
    (
        StatusCode::OK,
        [
            ("Content-Type", "text/javascript; charset=utf-8"),
            ("Cache-Control", "no-store"),
        ],
        txt,
    )
}

async fn get_index() -> Html<&'static str> {
    Html(
        "<!DOCTYPE html><html lang='fr'><head><meta charset='UTF-8'><title>Yomi — Serveur natif</title></head>\
         <body style='font-family:-apple-system,sans-serif;background:#FAF5EA;color:#231E1A;padding:36px;max-width:640px;margin:auto'>\
         <h1>Yomi (Moteur natif Rust)</h1>\
         <p>Le serveur de traduction local est actif sur le port 8765.</p>\
         <p><a href='/yomi.user.js'>Installer le userscript navigateur (/yomi.user.js)</a> · \
         <a href='/api/health'>Vérifier /api/health</a></p>\
         </body></html>",
    )
}

#[derive(Deserialize)]
struct TestApiKeyBody {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    gemini_api_key: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
}

async fn post_test_api_key(
    State(state): State<Arc<ServerState>>,
    Json(data): Json<TestApiKeyBody>,
) -> Json<Value> {
    let cfg = load_config(&state.data_dir);
    let prov = if data.provider.trim().is_empty() {
        cfg.api_provider.clone()
    } else {
        data.provider.trim().to_ascii_lowercase()
    };
    let explicit_key = if !data.api_key.trim().is_empty() {
        data.api_key.trim().to_string()
    } else {
        data.gemini_api_key.trim().to_string()
    };
    let effective_key = if !explicit_key.is_empty() {
        explicit_key
    } else {
        get_api_key(Some(&prov), &cfg)
    };
    let base_url = if data.base_url.trim().is_empty() {
        cfg.api_base_url.clone()
    } else {
        data.base_url.trim().to_string()
    };
    let test_model = if data.model.trim().is_empty() {
        cfg.api_model.clone()
    } else {
        data.model.trim().to_string()
    };
    let (ok, msg) = test_cloud_api_key(&prov, &effective_key, &base_url, &test_model, &cfg).await;
    Json(json!({
        "ok": ok,
        "message": msg,
        "error": if ok { "" } else { &msg },
    }))
}

async fn post_glossary(
    State(state): State<Arc<ServerState>>,
    Json(input): Json<Glossary>,
) -> Response {
    match save_glossary(&state.data_dir, input) {
        Ok(saved) => (StatusCode::OK, Json(json!({ "ok": true, "glossary": saved }))).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
struct DeleteGlossaryBody {
    #[serde(default)]
    slug: String,
}

async fn post_delete_glossary(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<DeleteGlossaryBody>,
) -> Json<Value> {
    let deleted = delete_glossary(&state.data_dir, &body.slug);
    Json(json!({ "ok": true, "deleted": deleted }))
}

async fn post_unload(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let unloaded = unload_ollama_models_at(&state.ollama_url).await;
    Json(json!({ "ok": true, "unloaded": unloaded }))
}

async fn post_clear_cache(State(state): State<Arc<ServerState>>) -> Json<Value> {
    let deleted = clear_disk_cache(&state.data_dir.join("cache"));
    state.prefetch.clear();
    Json(json!({ "ok": true, "deleted": deleted }))
}

fn translate_error_response(err: TranslateError) -> Response {
    match err {
        TranslateError::RateLimit { message, retry_after, is_daily } => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": message,
                "retry_after": retry_after,
                "is_daily": is_daily,
            })),
        )
            .into_response(),
        TranslateError::BadRequest(msg) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
        }
        TranslateError::Runtime(msg) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": msg }))).into_response()
        }
    }
}

async fn translate_single_prepared(
    state: &ServerState,
    cfg: &AppConfig,
    image_bytes: &[u8],
    cache_file: &Path,
    ocr_out: &OcrResult,
    ocr_ms: u64,
    mut bubbles: Vec<Bubble>,
    model: &str,
    source_lang: &str,
    target_lang: &str,
    effective_vision: bool,
    context_history: &[String],
    index: Option<usize>,
    glossary: Option<&Glossary>,
) -> Result<CachedPageResult, TranslateError> {
    let mut to_translate = Vec::new();
    for b in &mut bubbles {
        if is_punctuation_only(&b.text) {
            b.fr = Some(b.text.clone());
        } else {
            to_translate.push(b.clone());
        }
    }

    let mut translate_ms = 0u64;
    let used_vision = effective_vision && !to_translate.is_empty();
    let mut all_translated = true;

    if !to_translate.is_empty() {
        let som_b64 = if used_vision {
            let bytes_owned = image_bytes.to_vec();
            let b_clone = to_translate.clone();
            let res = tokio::task::spawn_blocking(move || {
                create_som_image_base64(&bytes_owned, &b_clone, 1120)
            })
            .await
            .map_err(|e| TranslateError::Runtime(e.to_string()))?
            .map_err(TranslateError::Runtime)?;
            Some(res)
        } else {
            None
        };

        let t1 = std::time::Instant::now();
        let translations = translate_bubbles(
            &to_translate,
            model,
            source_lang,
            target_lang,
            context_history,
            som_b64.as_deref(),
            glossary,
            cfg,
            &state.ollama_url,
        )
        .await?;
        translate_ms = t1.elapsed().as_millis() as u64;

        let mut skipped_ids = HashSet::new();
        for b in &mut bubbles {
            if is_punctuation_only(&b.text) {
                continue;
            }
            if let Some(tr) = translations.get(&b.id) {
                if tr.skip && can_skip_bubble(b) {
                    skipped_ids.insert(b.id);
                    continue;
                }
                let fr_val = tr.fr.trim();
                if !fr_val.is_empty() {
                    b.fr = Some(fr_val.to_string());
                } else {
                    b.fr = Some(b.text.clone());
                    all_translated = false;
                }
                if !tr.who.is_empty() {
                    b.who = Some(tr.who.clone());
                }
            } else {
                b.fr = Some(b.text.clone());
                all_translated = false;
            }
        }
        if !skipped_ids.is_empty() {
            bubbles.retain(|b| !skipped_ids.contains(&b.id));
        }
    }

    let page_result = CachedPageResult {
        index,
        bubbles,
        image_width: ocr_out.width,
        image_height: ocr_out.height,
        model: model.to_string(),
        source_lang: source_lang.to_string(),
        target_lang: target_lang.to_string(),
        vision_used: used_vision,
        glossary_used: glossary.map(|g| g.title.clone()),
        cached: false,
        ocr_ms,
        translate_ms,
    };

    if all_translated {
        if let Ok(json_str) = serde_json::to_string(&page_result) {
            let _ = fs::write(cache_file, json_str);
        }
    }

    Ok(page_result)
}

#[derive(Deserialize)]
pub struct TranslateSingleRequest {
    #[serde(default)]
    pub image_base64: String,
    #[serde(default)]
    pub image_url: String,
    #[serde(default)]
    pub referer: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub source_lang: String,
    #[serde(default)]
    pub target_lang: String,
    #[serde(default)]
    pub skip_cache: bool,
    #[serde(default)]
    pub vision: Option<Value>,
    #[serde(default)]
    pub glossary_slug: String,
    #[serde(default)]
    pub page_url: String,
    #[serde(default)]
    pub page_title: String,
    #[serde(default)]
    pub context_history: Vec<String>,
    #[serde(default)]
    pub prefetch_next: Option<ImagePayloadInput>,
}

fn vision_opt_to_mode(v: &Option<Value>) -> String {
    match v {
        Some(Value::Bool(b)) => normalize_vision_mode(Some(if *b { "on" } else { "off" })),
        Some(Value::String(s)) => normalize_vision_mode(Some(s)),
        _ => "auto".to_string(),
    }
}

pub async fn execute_single_translation(
    state: Arc<ServerState>,
    req: TranslateSingleRequest,
) -> Result<CachedPageResult, TranslateError> {
    let cfg = load_config(&state.data_dir);
    let model = if req.model.trim().is_empty() {
        let (ok, mods) = check_ollama_status_at(&state.ollama_url).await;
        get_default_model(ok, &mods, &cfg)
    } else {
        req.model.trim().to_string()
    };
    let source_lang = if req.source_lang.trim().is_empty() {
        cfg.source_lang.clone()
    } else {
        req.source_lang.trim().to_ascii_lowercase()
    };
    let target_lang = if req.target_lang.trim().is_empty() {
        cfg.target_lang.clone()
    } else {
        req.target_lang.trim().to_ascii_lowercase()
    };
    let vision_mode = if req.vision.is_some() {
        vision_opt_to_mode(&req.vision)
    } else {
        normalize_vision_mode(Some(&cfg.vision_mode))
    };
    let glossary_slug = if req.glossary_slug.trim().is_empty() {
        "auto".to_string()
    } else {
        req.glossary_slug.trim().to_string()
    };
    let page_url = if !req.page_url.trim().is_empty() {
        req.page_url.trim().to_string()
    } else {
        req.referer.trim().to_string()
    };
    let glossary = resolve_glossary(&state.data_dir, &glossary_slug, &page_url, req.page_title.trim());
    let glossary_hash = compute_glossary_hash(glossary.as_ref());
    let cache_dir = state.data_dir.join("cache");
    let _ = fs::create_dir_all(&cache_dir);

    schedule_prefetch_next(
        req.prefetch_next.as_ref(),
        &source_lang,
        state.prefetch.clone(),
        state.resource_dir.clone(),
    );

    let img_url = req.image_url.trim().to_string();
    let prefetched = if !img_url.is_empty() && req.image_base64.trim().is_empty() {
        state.prefetch.take(&img_url, &source_lang)
    } else {
        None
    };

    let (image_bytes, ocr_out, ocr_ms, bubbles) = if let Some(pf) = prefetched {
        (pf.img_bytes, pf.ocr_out, pf.ocr_ms, pf.bubbles)
    } else {
        let payload = ImagePayloadInput {
            image_base64: req.image_base64.clone(),
            image_url: req.image_url.clone(),
            referer: req.referer.clone(),
        };
        let bytes = decode_image_payload(&payload)
            .await
            .map_err(TranslateError::BadRequest)?;

        if !req.skip_cache {
            if let Some(mut hit) = find_disk_cache_hit(
                &cache_dir,
                &bytes,
                &model,
                &source_lang,
                &target_lang,
                &vision_mode,
                &glossary_hash,
                &cfg,
            ) {
                hit.cached = true;
                return Ok(hit);
            }
        }

        let bytes_for_ocr = bytes.clone();
        let src_clone = source_lang.clone();
        let res_dir = state.resource_dir.clone();
        let t0 = std::time::Instant::now();
        let ocr = tokio::task::spawn_blocking(move || {
            active_ocr_engine().recognize(&bytes_for_ocr, &src_clone, res_dir.as_ref())
        })
        .await
        .map_err(|e| TranslateError::Runtime(e.to_string()))?
        .map_err(TranslateError::Runtime)?;
        let elapsed_ocr = t0.elapsed().as_millis() as u64;
        let bubs = group_lines_into_bubbles(&ocr.lines, &source_lang);
        (bytes, ocr, elapsed_ocr, bubs)
    };

    let effective_vision = should_use_vision(&vision_mode, &model, &bubbles, &cfg);
    let cache_file = get_cache_path(
        &cache_dir,
        &image_bytes,
        &model,
        &source_lang,
        &target_lang,
        effective_vision,
        &glossary_hash,
    );

    if !req.skip_cache && cache_file.exists() {
        if let Ok(raw) = fs::read_to_string(&cache_file) {
            if let Ok(mut cached_resp) = serde_json::from_str::<CachedPageResult>(&raw) {
                cached_resp.cached = true;
                return Ok(cached_resp);
            }
        }
    }

    translate_single_prepared(
        &state,
        &cfg,
        &image_bytes,
        &cache_file,
        &ocr_out,
        ocr_ms,
        bubbles,
        &model,
        &source_lang,
        &target_lang,
        effective_vision,
        &req.context_history,
        None,
        glossary.as_ref(),
    )
    .await
}

async fn post_translate(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<TranslateSingleRequest>,
) -> Response {
    match execute_single_translation(state, req).await {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(err) => translate_error_response(err),
    }
}

#[derive(Deserialize)]
struct BatchPageInput {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    image_base64: String,
    #[serde(default)]
    image_url: String,
    #[serde(default)]
    referer: String,
}

#[derive(Deserialize)]
struct TranslateBatchRequest {
    #[serde(default)]
    pages: Vec<BatchPageInput>,
    #[serde(default)]
    model: String,
    #[serde(default)]
    source_lang: String,
    #[serde(default)]
    target_lang: String,
    #[serde(default)]
    skip_cache: bool,
    #[serde(default)]
    vision: Option<Value>,
    #[serde(default)]
    glossary_slug: String,
    #[serde(default)]
    page_url: String,
    #[serde(default)]
    referer: String,
    #[serde(default)]
    page_title: String,
    #[serde(default)]
    context_history: Vec<String>,
    #[serde(default)]
    prefetch_next: Option<ImagePayloadInput>,
}

struct PreparedPage {
    index: usize,
    img_bytes: Vec<u8>,
    cache_file: PathBuf,
    ocr_out: OcrResult,
    ocr_ms: u64,
    bubbles: Vec<Bubble>,
    page_vision: bool,
}

async fn post_translate_batch(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<TranslateBatchRequest>,
) -> Response {
    if req.pages.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Champ 'pages' requis (liste non vide)." })),
        )
            .into_response();
    }

    let cfg = load_config(&state.data_dir);
    let model = if req.model.trim().is_empty() {
        let (ok, mods) = check_ollama_status_at(&state.ollama_url).await;
        get_default_model(ok, &mods, &cfg)
    } else {
        req.model.trim().to_string()
    };
    let source_lang = if req.source_lang.trim().is_empty() {
        "en".to_string()
    } else {
        req.source_lang.trim().to_ascii_lowercase()
    };
    let target_lang = if req.target_lang.trim().is_empty() {
        "fr".to_string()
    } else {
        req.target_lang.trim().to_ascii_lowercase()
    };
    let vision_mode = vision_opt_to_mode(&req.vision);
    let glossary_slug = if req.glossary_slug.trim().is_empty() {
        "auto".to_string()
    } else {
        req.glossary_slug.trim().to_string()
    };
    let page_url = if !req.page_url.trim().is_empty() {
        req.page_url.trim().to_string()
    } else if !req.referer.trim().is_empty() {
        req.referer.trim().to_string()
    } else {
        req.pages[0].referer.trim().to_string()
    };
    let glossary = resolve_glossary(&state.data_dir, &glossary_slug, &page_url, req.page_title.trim());
    let glossary_hash = compute_glossary_hash(glossary.as_ref());
    let cache_dir = state.data_dir.join("cache");
    let _ = fs::create_dir_all(&cache_dir);

    schedule_prefetch_next(
        req.prefetch_next.as_ref(),
        &source_lang,
        state.prefetch.clone(),
        state.resource_dir.clone(),
    );

    let ordered_indices: Vec<usize> = req.pages.iter().map(|p| p.index).collect();
    let sem = Arc::new(tokio::sync::Semaphore::new(4));
    let mut handles = Vec::with_capacity(req.pages.len());

    for item in req.pages {
        let st = state.clone();
        let cfg_c = cfg.clone();
        let model_c = model.clone();
        let src_c = source_lang.clone();
        let tgt_c = target_lang.clone();
        let vis_c = vision_mode.clone();
        let ghash_c = glossary_hash.clone();
        let cache_c = cache_dir.clone();
        let skip_c = req.skip_cache;
        let sem_c = sem.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem_c.acquire().await.ok();
            let idx = item.index;
            let res: Result<(Option<Value>, Option<PreparedPage>), String> = async {
                let img_url = item.image_url.trim().to_string();
                let prefetched = if !img_url.is_empty() && item.image_base64.trim().is_empty() {
                    st.prefetch.take(&img_url, &src_c)
                } else {
                    None
                };

                let (img_bytes, ocr_out, ocr_ms, bubbles) = if let Some(pf) = prefetched {
                    (pf.img_bytes, pf.ocr_out, pf.ocr_ms, pf.bubbles)
                } else {
                    let payload = ImagePayloadInput {
                        image_base64: item.image_base64,
                        image_url: item.image_url,
                        referer: item.referer,
                    };
                    let bytes = decode_image_payload(&payload).await?;
                    if !skip_c {
                        if let Some(mut hit) = find_disk_cache_hit(
                            &cache_c, &bytes, &model_c, &src_c, &tgt_c, &vis_c, &ghash_c, &cfg_c,
                        ) {
                            hit.cached = true;
                            hit.index = Some(idx);
                            let val = serde_json::to_value(hit).map_err(|e| e.to_string())?;
                            return Ok((Some(val), None));
                        }
                    }
                    let bytes_for_ocr = bytes.clone();
                    let src_ocr = src_c.clone();
                    let res_dir = st.resource_dir.clone();
                    let t0 = std::time::Instant::now();
                    let ocr = tokio::task::spawn_blocking(move || {
                        active_ocr_engine().recognize(&bytes_for_ocr, &src_ocr, res_dir.as_ref())
                    })
                    .await
                    .map_err(|e| e.to_string())??;
                    let ms = t0.elapsed().as_millis() as u64;
                    let bubs = group_lines_into_bubbles(&ocr.lines, &src_c);
                    (bytes, ocr, ms, bubs)
                };

                let page_vision = should_use_vision(&vis_c, &model_c, &bubbles, &cfg_c);
                let cache_file = get_cache_path(
                    &cache_c, &img_bytes, &model_c, &src_c, &tgt_c, page_vision, &ghash_c,
                );
                if !skip_c && cache_file.exists() {
                    if let Ok(raw) = fs::read_to_string(&cache_file) {
                        if let Ok(mut hit) = serde_json::from_str::<CachedPageResult>(&raw) {
                            hit.cached = true;
                            hit.index = Some(idx);
                            let val = serde_json::to_value(hit).map_err(|e| e.to_string())?;
                            return Ok((Some(val), None));
                        }
                    }
                }

                Ok((
                    None,
                    Some(PreparedPage {
                        index: idx,
                        img_bytes,
                        cache_file,
                        ocr_out,
                        ocr_ms,
                        bubbles,
                        page_vision,
                    }),
                ))
            }
            .await;

            match res {
                Ok(pair) => (idx, pair.0, pair.1),
                Err(e) => (idx, Some(json!({ "index": idx, "error": e })), None),
            }
        }));
    }

    let mut results: BTreeMap<usize, Value> = BTreeMap::new();
    let mut pending_pages = Vec::new();
    for h in handles {
        if let Ok((idx, imm, pend)) = h.await {
            if let Some(v) = imm {
                results.insert(idx, v);
            } else if let Some(p) = pend {
                pending_pages.push(p);
            }
        }
    }

    pending_pages.sort_by_key(|p| p.index);
    let any_vision = pending_pages.iter().any(|p| p.page_vision);
    let mut translate_ms = 0u64;

    if any_vision {
        let mut running_ctx = req.context_history.clone();
        for p in pending_pages {
            let idx = p.index;
            let page_res = match translate_single_prepared(
                &state,
                &cfg,
                &p.img_bytes,
                &p.cache_file,
                &p.ocr_out,
                p.ocr_ms,
                p.bubbles,
                &model,
                &source_lang,
                &target_lang,
                p.page_vision,
                &running_ctx,
                Some(idx),
                glossary.as_ref(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => return translate_error_response(e),
            };
            translate_ms += page_res.translate_ms;
            for b in &page_res.bubbles {
                if let Some(fr) = &b.fr {
                    let t = fr.trim();
                    if !t.is_empty() && !is_punctuation_only(t) {
                        running_ctx.push(t.to_string());
                    }
                }
            }
            if running_ctx.len() > 8 {
                let drop_n = running_ctx.len() - 8;
                running_ctx.drain(..drop_n);
            }
            results.insert(idx, serde_json::to_value(page_res).unwrap_or(Value::Null));
        }
    } else if !pending_pages.is_empty() {
        let mut page_complete: BTreeMap<usize, bool> =
            pending_pages.iter().map(|p| (p.index, true)).collect();
        let mut combined_bubbles = Vec::new();
        let mut mapping: Vec<(usize, usize)> = Vec::new();
        let mut next_id = 0usize;

        for (pi, p) in pending_pages.iter_mut().enumerate() {
            for (bi, b) in p.bubbles.iter_mut().enumerate() {
                if is_punctuation_only(&b.text) {
                    b.fr = Some(b.text.clone());
                } else {
                    let mut synthetic = b.clone();
                    synthetic.id = next_id;
                    combined_bubbles.push(synthetic);
                    mapping.push((pi, bi));
                    next_id += 1;
                }
            }
        }

        let mut translations: BTreeMap<usize, TranslationItem> = BTreeMap::new();
        if !combined_bubbles.is_empty() {
            let t1 = std::time::Instant::now();
            translations = match translate_bubbles(
                &combined_bubbles,
                &model,
                &source_lang,
                &target_lang,
                &req.context_history,
                None,
                glossary.as_ref(),
                &cfg,
                &state.ollama_url,
            )
            .await
            {
                Ok(tr) => tr,
                Err(e) => return translate_error_response(e),
            };
            translate_ms = t1.elapsed().as_millis() as u64;
        }

        let mut skipped_by_page: BTreeMap<usize, HashSet<usize>> = BTreeMap::new();
        for (gid, (pi, bi)) in mapping.into_iter().enumerate() {
            let p_idx = pending_pages[pi].index;
            let b = &mut pending_pages[pi].bubbles[bi];
            if let Some(tr) = translations.get(&gid) {
                if tr.skip && can_skip_bubble(b) {
                    skipped_by_page.entry(pi).or_default().insert(bi);
                    continue;
                }
                let fr_val = tr.fr.trim();
                if !fr_val.is_empty() {
                    b.fr = Some(fr_val.to_string());
                } else {
                    b.fr = Some(b.text.clone());
                    page_complete.insert(p_idx, false);
                }
                if !tr.who.is_empty() {
                    b.who = Some(tr.who.clone());
                }
            } else {
                b.fr = Some(b.text.clone());
                page_complete.insert(p_idx, false);
            }
        }

        for (pi, p) in pending_pages.into_iter().enumerate() {
            let idx = p.index;
            let kept: Vec<Bubble> = if let Some(sk) = skipped_by_page.get(&pi) {
                p.bubbles
                    .into_iter()
                    .enumerate()
                    .filter(|(bi, _)| !sk.contains(bi))
                    .map(|(_, b)| b)
                    .collect()
            } else {
                p.bubbles
            };
            let page_res = CachedPageResult {
                index: Some(idx),
                bubbles: kept,
                image_width: p.ocr_out.width,
                image_height: p.ocr_out.height,
                model: model.clone(),
                source_lang: source_lang.clone(),
                target_lang: target_lang.clone(),
                vision_used: false,
                glossary_used: glossary.as_ref().map(|g| g.title.clone()),
                cached: false,
                ocr_ms: p.ocr_ms,
                translate_ms,
            };
            if page_complete.get(&idx).copied().unwrap_or(true) {
                if let Ok(json_str) = serde_json::to_string(&page_res) {
                    let _ = fs::write(&p.cache_file, json_str);
                }
            }
            results.insert(idx, serde_json::to_value(page_res).unwrap_or(Value::Null));
        }
    }

    let ordered: Vec<Value> = ordered_indices
        .into_iter()
        .map(|idx| results.remove(&idx).unwrap_or_else(|| json!({ "index": idx, "error": "Missing result" })))
        .collect();

    #[derive(Serialize)]
    struct BatchResp {
        results: Vec<Value>,
        model: String,
        vision_used: bool,
        translate_ms: u64,
    }

    (
        StatusCode::OK,
        Json(BatchResp {
            results: ordered,
            model,
            vision_used: any_vision,
            translate_ms,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn axum_router_endpoints_and_cors_headers() {
        let tmp = tempfile::tempdir().unwrap();
        crate::glossary::ensure_default_glossaries(tmp.path(), None);
        let state = Arc::new(ServerState {
            data_dir: tmp.path().to_path_buf(),
            resource_dir: None,
            prefetch: Arc::new(PrefetchCache::new()),
            ollama_url: "http://127.0.0.1:1".to_string(),
        });
        let app = build_router(state);

        // /api/health
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("access-control-allow-private-network").unwrap(),
            "true"
        );
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let health: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["ok"], json!(true));
        assert_eq!(health["native_core"], json!(true));

        // /api/config POST + GET redaction
        let post_req = Request::builder()
            .method("POST")
            .uri("/api/config")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"api_provider":"openai","api_key":"sk-secret12345678"}"#))
            .unwrap();
        let post_resp = app.clone().oneshot(post_req).await.unwrap();
        assert_eq!(post_resp.status(), StatusCode::OK);
        let post_bytes = post_resp.into_body().collect().await.unwrap().to_bytes();
        let post_json: Value = serde_json::from_slice(&post_bytes).unwrap();
        assert!(post_json["config"].get("api_key").is_none());
        assert_eq!(post_json["config"]["api_key_masked"], json!("sk-s...5678"));

        // /yomi.user.js
        let us_resp = app
            .clone()
            .oneshot(Request::builder().uri("/yomi.user.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(us_resp.status(), StatusCode::OK);
        let us_bytes = us_resp.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&us_bytes).contains("==UserScript=="));

        // /api/glossaries
        let gl_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/glossaries?url=https://site.test/komi-san/ch2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(gl_resp.status(), StatusCode::OK);
        let gl_bytes = gl_resp.into_body().collect().await.unwrap().to_bytes();
        let gl_json: Value = serde_json::from_slice(&gl_bytes).unwrap();
        assert_eq!(gl_json["detected_slug"], json!("komi-cant-communicate"));

        // /api/clear-cache
        fs::write(tmp.path().join("cache/sample.json"), "{}").unwrap();
        let clr_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/clear-cache")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(clr_resp.status(), StatusCode::OK);
        let clr_bytes = clr_resp.into_body().collect().await.unwrap().to_bytes();
        let clr_json: Value = serde_json::from_slice(&clr_bytes).unwrap();
        assert_eq!(clr_json["ok"], json!(true));
        assert_eq!(clr_json["deleted"], json!(1));
    }
}
