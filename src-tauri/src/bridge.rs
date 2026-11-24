use base64::Engine;
use futures_util::StreamExt;
use image::{ImageBuffer, ImageFormat, Rgba};
use regex::Regex;
use reqwest::Client;
use serde_json::{json, Value};
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::System;
use tauri::{AppHandle, Emitter, Manager, Theme};
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;

use crate::cache::clear_disk_cache;
use crate::config::{
    get_api_key, load_config, public_config, save_config, AppConfig,
};
use crate::llm::{
    check_ollama_status_at, pull_ollama_model, test_cloud_api_key, unload_ollama_models_at,
};
use crate::ocr::active_ocr_engine;
use crate::server::{
    build_health_payload, execute_single_translation, ServerController, ServerState,
    TranslateSingleRequest,
};
use crate::tray::{emit_navigate, refresh_tray_status};

pub struct AppRuntime {
    pub server_state: Arc<ServerState>,
    pub server: Arc<ServerController>,
    pub ollama_child: Mutex<Option<Child>>,
    pub active_pull_cancel: Mutex<Option<CancellationToken>>,
}

impl AppRuntime {
    pub fn new(server_state: Arc<ServerState>, port: u16) -> Self {
        Self {
            server_state,
            server: Arc::new(ServerController::new(port)),
            ollama_child: Mutex::new(None),
            active_pull_cancel: Mutex::new(None),
        }
    }

    pub async fn shutdown(&self) {
        let _ = unload_ollama_models_at(&self.server_state.ollama_url).await;
        self.server.stop().await;
        let mut child_guard = self.ollama_child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut ch) = *child_guard {
            let _ = ch.kill();
        }
        *child_guard = None;
    }
}

pub fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

pub fn host_os_label() -> String {
    let os_ver = System::long_os_version().unwrap_or_else(|| {
        match host_platform() {
            "macos" => "macOS".to_string(),
            "windows" => "Windows".to_string(),
            _ => "Linux".to_string(),
        }
    });
    os_ver
}

pub fn detect_chip_and_ram() -> (String, u64) {
    let mut sys = System::new_all();
    sys.refresh_cpu_all();
    sys.refresh_memory();
    let ram_gb = ((sys.total_memory() as f64) / 1_073_741_824.0).round().max(1.0) as u64;
    let brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| match host_platform() {
            "macos" => "Apple Silicon".to_string(),
            _ => std::env::consts::ARCH.to_string(),
        });
    (brand, ram_gb)
}

pub fn recommend_local_model(chip: &str, ram_gb: u64) -> &'static str {
    let mut gen = 1u32;
    let re = Regex::new(r"(?i)\bM(\d+)\b").unwrap();
    if let Some(caps) = re.captures(chip) {
        if let Ok(parsed) = caps[1].parse::<u32>() {
            gen = parsed;
        }
    }
    let low = chip.to_ascii_lowercase();
    let is_pro_plus = low.contains("pro") || low.contains("max") || low.contains("ultra");

    if ram_gb < 16 {
        return "gemma4:e4b";
    }
    if ram_gb < 24 {
        return if gen >= 3 || is_pro_plus {
            "gemma4:12b"
        } else {
            "gemma4:e4b"
        };
    }
    if ram_gb >= 32 || gen >= 3 || is_pro_plus {
        "gemma4:26b"
    } else {
        "gemma4:12b"
    }
}

pub fn find_ollama_binary() -> (String, Option<String>) {
    let home = dirs::home_dir().unwrap_or_default();
    let mut bin_candidates: Vec<PathBuf> = Vec::new();
    let mut app_candidates: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        bin_candidates.extend([
            PathBuf::from("/opt/homebrew/bin/ollama"),
            PathBuf::from("/usr/local/bin/ollama"),
            PathBuf::from("/Applications/Ollama.app/Contents/Resources/ollama"),
            home.join("Applications/Ollama.app/Contents/Resources/ollama"),
            PathBuf::from("/Applications/Ollama.app/Contents/MacOS/ollama"),
            home.join("Applications/Ollama.app/Contents/MacOS/ollama"),
        ]);
        app_candidates.extend([
            PathBuf::from("/Applications/Ollama.app"),
            home.join("Applications/Ollama.app"),
        ]);
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app) = dirs::data_local_dir() {
            bin_candidates.push(local_app.join("Programs/Ollama/ollama.exe"));
            app_candidates.push(local_app.join("Programs/Ollama/ollama app.exe"));
        }
        bin_candidates.push(PathBuf::from(r"C:\Program Files\Ollama\ollama.exe"));
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        bin_candidates.extend([
            PathBuf::from("/usr/local/bin/ollama"),
            PathBuf::from("/usr/bin/ollama"),
            home.join(".local/bin/ollama"),
        ]);
    }

    if let Ok(path_var) = std::env::var("PATH") {
        let exe_name = if cfg!(target_os = "windows") { "ollama.exe" } else { "ollama" };
        for dir in std::env::split_paths(&path_var) {
            bin_candidates.push(dir.join(exe_name));
        }
    }

    let found_bin = bin_candidates
        .into_iter()
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let found_app = app_candidates
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string());

    (found_bin, found_app)
}

pub async fn detect_ollama_info(ollama_url: &str) -> Value {
    let (bin_path, app_path) = find_ollama_binary();
    let (running, models) = check_ollama_status_at(ollama_url).await;
    let installed = running || !bin_path.is_empty() || app_path.is_some();
    json!({
        "installed": installed,
        "path": bin_path,
        "app_path": app_path.unwrap_or_default(),
        "running": running,
        "models": models,
    })
}

pub async fn ensure_ollama_running(rt: &AppRuntime) -> bool {
    let (ok, _) = check_ollama_status_at(&rt.server_state.ollama_url).await;
    if ok {
        return true;
    }

    let (bin_path, app_path) = find_ollama_binary();
    if !bin_path.is_empty() {
        let log_path = rt.server_state.data_dir.join("ollama.log");
        let log_file = fs::File::create(&log_path).ok();
        let mut cmd = Command::new(&bin_path);
        cmd.arg("serve")
            .env("OLLAMA_FLASH_ATTENTION", "1")
            .env("OLLAMA_KV_CACHE_TYPE", "q8_0");
        if let Some(f) = log_file {
            if let Ok(f2) = f.try_clone() {
                cmd.stdout(Stdio::from(f)).stderr(Stdio::from(f2));
            }
        }
        if let Ok(child) = cmd.spawn() {
            *rt.ollama_child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        }
    } else if let Some(app_p) = app_path {
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("/usr/bin/open")
                .args(["-g", "-a", &app_p])
                .env("OLLAMA_FLASH_ATTENTION", "1")
                .env("OLLAMA_KV_CACHE_TYPE", "q8_0")
                .spawn();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = Command::new(&app_p).spawn();
        }
    }

    for _ in 0..24 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (poll, _) = check_ollama_status_at(&rt.server_state.ollama_url).await;
        if poll {
            return true;
        }
    }
    false
}

pub fn detect_userscripts_info(cfg: &AppConfig) -> Value {
    let home = dirs::home_dir().unwrap_or_default();

    #[cfg(target_os = "macos")]
    let app_installed = Path::new("/Applications/Userscripts.app").exists()
        || home.join("Applications/Userscripts.app").exists();

    #[cfg(not(target_os = "macos"))]
    let app_installed = false;

    let mut candidates: Vec<PathBuf> = Vec::new();
    let saved = cfg.userscripts_dir.trim();
    if !saved.is_empty() {
        candidates.push(PathBuf::from(saved));
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(home.join("Library/Containers/com.userscripts.macos.Userscripts-Extension/Data/Documents/scripts"));
        candidates.push(home.join("Library/Containers/com.userscripts.macos/Data/Documents/scripts"));
        candidates.push(home.join("Documents/Userscripts"));
        candidates.push(home.join("Userscripts"));
    }

    let detected_dir = candidates
        .into_iter()
        .find(|d| d.is_dir())
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();

    let script_path = if detected_dir.is_empty() {
        String::new()
    } else {
        Path::new(&detected_dir)
            .join("yomi.user.js")
            .to_string_lossy()
            .to_string()
    };
    let script_installed = !script_path.is_empty() && Path::new(&script_path).is_file();

    json!({
        "app_installed": app_installed,
        "detected_dir": detected_dir,
        "script_installed": script_installed,
        "script_path": script_path,
    })
}

pub fn install_userscript_file(st: &ServerState, folder: &Path) -> Result<PathBuf, String> {
    let content = st.resolve_userscript_text();
    let legacy = folder.join("safari-manga-translator.user.js");
    if legacy.exists() {
        let _ = fs::remove_file(&legacy);
    }
    let dst = folder.join("yomi.user.js");
    if let Ok(existing) = fs::read_to_string(&dst) {
        if existing == content {
            return Ok(dst);
        }
    }
    fs::write(&dst, content.as_bytes())
        .map_err(|e| format!("Impossible d'écrire {} : {}", dst.display(), e))?;
    Ok(dst)
}

pub fn sync_userscript_if_configured(st: &ServerState) {
    let cfg = load_config(&st.data_dir);
    let info = detect_userscripts_info(&cfg);
    let dir = info["detected_dir"].as_str().unwrap_or("");
    let saved_dir = cfg.userscripts_dir.trim();
    let already_installed = info["script_installed"].as_bool().unwrap_or(false);
    let legacy_exists = !dir.is_empty()
        && Path::new(dir)
            .join("safari-manga-translator.user.js")
            .is_file();
    if !dir.is_empty() && (already_installed || legacy_exists || !saved_dir.is_empty()) {
        let _ = install_userscript_file(st, Path::new(dir));
    }
}

pub async fn inspect_system_state(rt: &AppRuntime) -> Value {
    let cfg = load_config(&rt.server_state.data_dir);
    let (chip, ram_gb) = detect_chip_and_ram();
    let rec_model = recommend_local_model(&chip, ram_gb);
    let ollama = detect_ollama_info(&rt.server_state.ollama_url).await;
    let ocr = active_ocr_engine();
    let ocr_ready = ocr.is_ready(rt.server_state.resource_dir.as_ref());
    let userscripts = detect_userscripts_info(&cfg);
    let srv_running = rt.server.is_running();
    let health = if srv_running {
        build_health_payload(&rt.server_state).await
    } else {
        json!({})
    };

    json!({
        "platform": host_platform(),
        "os_name": host_os_label(),
        "macos": host_os_label(),
        "arch": std::env::consts::ARCH,
        "apple_silicon": cfg!(target_os = "macos") && std::env::consts::ARCH == "aarch64",
        "chip": chip,
        "ram_gb": ram_gb,
        "recommended_model": rec_model,
        "native_core": true,
        "ocr_engine": ocr.name(),
        "ocr_ready": ocr_ready,
        "python": {
            "installed": true,
            "path": "built-in",
            "version": "Rust Native",
            "source": "builtin",
        },
        "ollama": ollama,
        "userscripts": userscripts,
        "server_running": srv_running,
        "server_health": health,
        "config": public_config(&cfg),
        "data_dir": rt.server_state.data_dir.to_string_lossy().to_string(),
    })
}

fn emit_bridge_event(app: &AppHandle, event_name: &str, payload: Value) {
    let _ = app.emit(
        "yomi-bridge",
        json!({
            "event": event_name,
            "payload": payload,
        }),
    );
}

fn apply_window_theme(app: &AppHandle, raw_theme: &str) {
    if let Some(win) = app.get_webview_window("main") {
        let t = match raw_theme.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            _ => None,
        };
        let _ = win.set_theme(t);
    }
}

fn read_image_file_as_data_url(path: &Path) -> Result<(String, String), String> {
    let meta = fs::metadata(path).map_err(|e| format!("Fichier inaccessible : {}", e))?;
    if meta.len() > 30 * 1024 * 1024 {
        return Err("Image trop volumineuse (max 30 Mo).".to_string());
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("manga-page.png")
        .to_string();
    let mime = match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    };
    let raw = fs::read(path).map_err(|e| format!("Erreur lecture image : {}", e))?;
    if let Some(m) = mime {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        return Ok((format!("data:{};base64,{}", m, b64), filename));
    }
    // Decode and re-encode as PNG for bmp/tiff/etc.
    let dyn_img = image::load_from_memory(&raw)
        .map_err(|_| "Format d'image non pris en charge.".to_string())?;
    let mut png_bytes = Vec::new();
    dyn_img
        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
        .map_err(|e| format!("Erreur encodage PNG : {}", e))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok((format!("data:image/png;base64,{}", b64), filename))
}

async fn ensure_backend_started(app: &AppHandle, rt: &AppRuntime, force_restart: bool) -> bool {
    if !force_restart && rt.server.is_running() {
        refresh_tray_status(app, true);
        return true;
    }
    let cfg = load_config(&rt.server_state.data_dir);
    if cfg.engine_mode != "api" {
        let _ = ensure_ollama_running(rt).await;
    }
    let res = if force_restart {
        rt.server.restart(rt.server_state.clone()).await
    } else {
        rt.server.start(rt.server_state.clone()).await
    };
    let ok = res.is_ok();
    refresh_tray_status(app, ok);
    ok
}

async fn handle_install_ollama(app: &AppHandle, rt: &AppRuntime) -> Result<Value, String> {
    if cfg!(all(not(target_os = "macos"), not(target_os = "windows"))) {
        let _ = open::that("https://ollama.com/download/linux");
        return Err(
            "Sur Linux, installe Ollama via https://ollama.com/download/linux (`curl -fsSL https://ollama.com/install.sh | sh`)."
                .to_string(),
        );
    }

    let dl_url = if cfg!(target_os = "macos") {
        "https://ollama.com/download/Ollama-darwin.zip"
    } else {
        "https://ollama.com/download/OllamaSetup.exe"
    };
    let dest_name = if cfg!(target_os = "macos") {
        "Ollama-darwin.zip"
    } else {
        "OllamaSetup.exe"
    };
    let dest_path = rt.server_state.data_dir.join(dest_name);

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(dl_url)
        .send()
        .await
        .map_err(|e| format!("Téléchargement Ollama échoué : {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} lors du téléchargement Ollama", resp.status().as_u16()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = fs::File::create(&dest_path).map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut done = 0u64;
    let mut last_emit = Instant::now();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(250) || (total > 0 && done >= total) {
            last_emit = Instant::now();
            let pct = if total > 0 {
                ((done as f64 / total as f64) * 100.0).round().min(100.0) as i32
            } else {
                -1
            };
            emit_bridge_event(
                app,
                "progress",
                json!({
                    "tool": "ollama",
                    "phase": "downloading",
                    "completed": done,
                    "total": total,
                    "percent": pct,
                }),
            );
        }
    }
    drop(file);

    emit_bridge_event(
        app,
        "progress",
        json!({ "tool": "ollama", "phase": "extracting", "percent": 100 }),
    );

    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().unwrap_or_default();
        let user_apps = home.join("Applications");
        let _ = fs::create_dir_all(&user_apps);
        let mut target_dir = PathBuf::from("/Applications");
        let sys_ollama = target_dir.join("Ollama.app");
        if sys_ollama.exists() && fs::remove_dir_all(&sys_ollama).is_err() {
            target_dir = user_apps.clone();
        }
        let mut out = Command::new("/usr/bin/ditto")
            .arg("-xk")
            .arg(&dest_path)
            .arg(&target_dir)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() && target_dir == Path::new("/Applications") {
            let user_ollama = user_apps.join("Ollama.app");
            if user_ollama.exists() {
                let _ = fs::remove_dir_all(&user_ollama);
            }
            out = Command::new("/usr/bin/ditto")
                .arg("-xk")
                .arg(&dest_path)
                .arg(&user_apps)
                .output()
                .map_err(|e| e.to_string())?;
        }
        let _ = fs::remove_file(&dest_path);
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).to_string());
        }
    }

    #[cfg(target_os = "windows")]
    {
        let status = Command::new(&dest_path)
            .arg("/SILENT")
            .status()
            .map_err(|e| format!("Erreur installation Ollama : {}", e))?;
        let _ = fs::remove_file(&dest_path);
        if !status.success() {
            return Err("L'installeur Ollama a été annulé ou a échoué.".to_string());
        }
    }

    let _ = ensure_ollama_running(rt).await;
    Ok(inspect_system_state(rt).await)
}

#[tauri::command]
pub async fn bridge_call(
    app: AppHandle,
    rt: tauri::State<'_, Arc<AppRuntime>>,
    action: String,
    payload: Option<Value>,
) -> Result<Value, String> {
    let payload_val = payload.unwrap_or_else(|| json!({}));
    let payload_obj = payload_val.as_object().cloned().unwrap_or_default();

    match action.as_str() {
        "getState" => {
            let state = inspect_system_state(rt.inner()).await;
            let running = state["server_running"].as_bool().unwrap_or(false);
            refresh_tray_status(&app, running);
            Ok(state)
        }

        "installPython" => Ok(inspect_system_state(rt.inner()).await),

        "installOllama" => handle_install_ollama(&app, rt.inner()).await,

        "startOllama" => {
            let ok = ensure_ollama_running(rt.inner()).await;
            if ok {
                Ok(inspect_system_state(rt.inner()).await)
            } else {
                Err("Ollama ne répond pas sur 127.0.0.1:11434".to_string())
            }
        }

        "pullModel" => {
            let model_name = payload_obj
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if model_name.is_empty() {
                return Err("Model required".to_string());
            }
            if !ensure_ollama_running(rt.inner()).await {
                return Err("Ollama ne répond pas sur 127.0.0.1:11434".to_string());
            }
            let cancel = CancellationToken::new();
            *rt.active_pull_cancel.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel.clone());
            let app_clone = app.clone();
            let res = pull_ollama_model(
                &rt.server_state.ollama_url,
                &model_name,
                cancel,
                move |ev| {
                    emit_bridge_event(
                        &app_clone,
                        "progress",
                        serde_json::to_value(ev).unwrap_or_else(|_| json!({})),
                    );
                },
            )
            .await;
            *rt.active_pull_cancel.lock().unwrap_or_else(|e| e.into_inner()) = None;
            res?;
            Ok(inspect_system_state(rt.inner()).await)
        }

        "cancelPull" => {
            if let Some(cancel) = rt
                .active_pull_cancel
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                cancel.cancel();
            }
            Ok(json!({ "cancelled": true }))
        }

        "testGeminiKey" | "testApiKey" => {
            let cfg = load_config(&rt.server_state.data_dir);
            let prov = payload_obj
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or(&cfg.api_provider)
                .trim()
                .to_ascii_lowercase();
            let explicit_key = payload_obj
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let key = if !explicit_key.is_empty() {
                explicit_key
            } else {
                get_api_key(Some(&prov), &cfg)
            };
            let base_url = payload_obj
                .get("base_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let eff_base = if base_url.is_empty() && prov == cfg.api_provider {
                cfg.api_base_url.clone()
            } else {
                base_url
            };
            let model = payload_obj
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(&cfg.api_model)
                .trim()
                .to_string();
            let (ok, detail) = test_cloud_api_key(&prov, &key, &eff_base, &model, &cfg).await;
            if ok {
                Ok(json!({ "ok": true }))
            } else {
                Err(detail)
            }
        }

        "autoInstallSafariUserscript" => {
            let cfg = load_config(&rt.server_state.data_dir);
            let info = detect_userscripts_info(&cfg);
            let dir = info["detected_dir"].as_str().unwrap_or("");
            if !dir.is_empty() {
                let dst = install_userscript_file(&rt.server_state, Path::new(dir))?;
                let mut u = serde_json::Map::new();
                u.insert("userscripts_dir".into(), Value::String(dir.to_string()));
                let _ = save_config(&rt.server_state.data_dir, &u);
                Ok(json!({
                    "auto": true,
                    "path": dst.to_string_lossy().to_string(),
                    "state": inspect_system_state(rt.inner()).await,
                }))
            } else {
                let picked = app
                    .dialog()
                    .file()
                    .set_title("Dossier de l'extension Userscripts")
                    .blocking_pick_folder();
                let Some(folder_fp) = picked else {
                    return Ok(json!({ "cancelled": true }));
                };
                let folder = folder_fp.into_path().map_err(|e| e.to_string())?;
                let dst = install_userscript_file(&rt.server_state, &folder)?;
                let mut u = serde_json::Map::new();
                u.insert("userscripts_dir".into(), Value::String(folder.to_string_lossy().to_string()));
                let _ = save_config(&rt.server_state.data_dir, &u);
                Ok(json!({
                    "path": dst.to_string_lossy().to_string(),
                    "state": inspect_system_state(rt.inner()).await,
                }))
            }
        }

        "installUserscriptToFolder" => {
            let cfg = load_config(&rt.server_state.data_dir);
            let info = detect_userscripts_info(&cfg);
            let mut builder = app
                .dialog()
                .file()
                .set_title("Dossier de l'extension Userscripts");
            let dir = info["detected_dir"].as_str().unwrap_or("");
            if !dir.is_empty() {
                builder = builder.set_directory(Path::new(dir));
            }
            let Some(folder_fp) = builder.blocking_pick_folder() else {
                return Ok(json!({ "cancelled": true }));
            };
            let folder = folder_fp.into_path().map_err(|e| e.to_string())?;
            let dst = install_userscript_file(&rt.server_state, &folder)?;
            let mut u = serde_json::Map::new();
            u.insert("userscripts_dir".into(), Value::String(folder.to_string_lossy().to_string()));
            let _ = save_config(&rt.server_state.data_dir, &u);
            Ok(json!({
                "path": dst.to_string_lossy().to_string(),
                "state": inspect_system_state(rt.inner()).await,
            }))
        }

        "openUserscriptInstallUrl" => {
            let ok = ensure_backend_started(&app, rt.inner(), false).await;
            if !ok {
                return Err("Serveur local indisponible.".to_string());
            }
            let url = format!("http://127.0.0.1:{}/yomi.user.js", rt.server.port);
            open::that(&url).map_err(|e| format!("Impossible d'ouvrir le navigateur : {}", e))?;
            Ok(json!({
                "opened": true,
                "url": url,
                "state": inspect_system_state(rt.inner()).await,
            }))
        }

        "openUserscriptsAppStore" => {
            let url = if cfg!(target_os = "macos") {
                "macappstore://apps.apple.com/app/id1463298887"
            } else {
                "https://www.tampermonkey.net/"
            };
            let _ = open::that(url);
            Ok(json!({}))
        }

        "openExternalUrl" => {
            let raw = payload_obj
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let parsed = url::Url::parse(raw).map_err(|_| "Invalid external URL".to_string())?;
            if parsed.scheme().eq_ignore_ascii_case("https")
                && parsed
                    .host_str()
                    .is_some_and(|h| h.eq_ignore_ascii_case("github.com"))
            {
                open::that(parsed.as_str()).map_err(|e| e.to_string())?;
                Ok(json!({ "opened": true, "url": parsed.as_str() }))
            } else {
                Err("Invalid external URL".to_string())
            }
        }

        "copyUserscript" => {
            let txt = rt.server_state.resolve_userscript_text();
            let mut cb = arboard::Clipboard::new()
                .map_err(|e| format!("Presse-papiers indisponible : {}", e))?;
            cb.set_text(txt)
                .map_err(|e| format!("Erreur copie presse-papiers : {}", e))?;
            Ok(json!({ "copied": true }))
        }

        "quitApp" => {
            let rt_clone = rt.inner().clone();
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                rt_clone.shutdown().await;
                app_clone.exit(0);
            });
            Ok(json!({}))
        }

        "setTheme" => {
            let raw_theme = payload_obj
                .get("theme")
                .and_then(Value::as_str)
                .unwrap_or("system")
                .trim()
                .to_ascii_lowercase();
            let clean_theme = if ["light", "dark", "system"].contains(&raw_theme.as_str()) {
                raw_theme
            } else {
                "system".to_string()
            };
            let mut u = serde_json::Map::new();
            u.insert("theme".into(), Value::String(clean_theme.clone()));
            let _ = save_config(&rt.server_state.data_dir, &u);
            apply_window_theme(&app, &clean_theme);
            Ok(json!({ "theme": clean_theme }))
        }

        "saveSettings" => {
            let saved = save_config(&rt.server_state.data_dir, &payload_obj);
            apply_window_theme(&app, &saved.theme);
            Ok(inspect_system_state(rt.inner()).await)
        }

        "saveConfigAndStartServer" => {
            let saved = save_config(&rt.server_state.data_dir, &payload_obj);
            apply_window_theme(&app, &saved.theme);
            let _ = ensure_backend_started(&app, rt.inner(), true).await;
            Ok(inspect_system_state(rt.inner()).await)
        }

        "restartServer" => {
            let _ = ensure_backend_started(&app, rt.inner(), true).await;
            Ok(inspect_system_state(rt.inner()).await)
        }

        "startServer" => {
            let _ = ensure_backend_started(&app, rt.inner(), false).await;
            Ok(inspect_system_state(rt.inner()).await)
        }

        "stopServer" => {
            rt.server.stop().await;
            refresh_tray_status(&app, false);
            Ok(inspect_system_state(rt.inner()).await)
        }

        "unloadRam" => {
            let unloaded = unload_ollama_models_at(&rt.server_state.ollama_url).await;
            Ok(json!({ "unloaded": unloaded }))
        }

        "clearCache" => {
            let deleted = clear_disk_cache(&rt.server_state.data_dir.join("cache"));
            rt.server_state.prefetch.clear();
            Ok(json!({ "deleted": deleted }))
        }

        "openTestPage" => {
            emit_navigate(&app, "test");
            Ok(json!({}))
        }

        "pickTestImage" => {
            let picked = app
                .dialog()
                .file()
                .set_title("Choisir une planche de manga")
                .add_filter(
                    "Images",
                    &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tiff", "heic"],
                )
                .blocking_pick_file();
            let Some(fp) = picked else {
                return Ok(json!({ "cancelled": true }));
            };
            let path = fp.into_path().map_err(|e| e.to_string())?;
            let (data_url, filename) = read_image_file_as_data_url(&path)?;
            Ok(json!({ "data_url": data_url, "filename": filename }))
        }

        "readClipboardImage" => {
            let Ok(mut cb) = arboard::Clipboard::new() else {
                return Ok(json!({ "found": false }));
            };
            if let Ok(img) = cb.get_image() {
                if let Some(rgba_buf) =
                    ImageBuffer::<Rgba<u8>, _>::from_raw(img.width as u32, img.height as u32, img.bytes.into_owned())
                {
                    let mut png_bytes = Vec::new();
                    if image::DynamicImage::ImageRgba8(rgba_buf)
                        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
                        .is_ok()
                    {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
                        return Ok(json!({
                            "found": true,
                            "data_url": format!("data:image/png;base64,{}", b64),
                            "filename": "clipboard.png",
                        }));
                    }
                }
            }
            Ok(json!({ "found": false }))
        }

        "getTestHealth" => {
            let auto_start = payload_obj
                .get("auto_start")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if !rt.server.is_running() && auto_start {
                let _ = ensure_backend_started(&app, rt.inner(), false).await;
            }
            let health = build_health_payload(&rt.server_state).await;
            let state = inspect_system_state(rt.inner()).await;
            Ok(json!({ "health": health, "state": state }))
        }

        "translateTestImage" => {
            let img_b64 = payload_obj
                .get("image_base64")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if img_b64.is_empty() {
                return Err("Image manquante".to_string());
            }
            if !rt.server.is_running() {
                let _ = ensure_backend_started(&app, rt.inner(), false).await;
            }
            let req = TranslateSingleRequest {
                image_base64: img_b64,
                image_url: String::new(),
                referer: String::new(),
                model: payload_obj
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                source_lang: payload_obj
                    .get("source_lang")
                    .and_then(Value::as_str)
                    .unwrap_or("en")
                    .to_string(),
                target_lang: payload_obj
                    .get("target_lang")
                    .and_then(Value::as_str)
                    .unwrap_or("fr")
                    .to_string(),
                skip_cache: payload_obj
                    .get("skip_cache")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                vision: payload_obj.get("vision").cloned(),
                glossary_slug: "auto".to_string(),
                page_url: String::new(),
                page_title: String::new(),
                context_history: Vec::new(),
                prefetch_next: None,
            };
            let res = execute_single_translation(rt.server_state.clone(), req)
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(res).map_err(|e| e.to_string())
        }

        "saveExportedImage" => {
            let mut b64 = payload_obj
                .get("data_url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if let Some((_, after)) = b64.split_once(";base64,") {
                b64 = after;
            }
            let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
            let png_bytes = base64::engine::general_purpose::STANDARD
                .decode(&cleaned)
                .map_err(|_| "Image PNG invalide".to_string())?;
            if png_bytes.is_empty() {
                return Err("Image PNG invalide".to_string());
            }
            let raw_name = payload_obj
                .get("suggested_name")
                .and_then(Value::as_str)
                .unwrap_or("manga-translated.png")
                .trim();
            let clean_name = if raw_name.is_empty() {
                "manga-translated.png"
            } else {
                raw_name
            };
            let picked = app
                .dialog()
                .file()
                .set_title("Exporter la planche traduite")
                .set_file_name(clean_name)
                .add_filter("PNG Image", &["png"])
                .blocking_save_file();
            let Some(fp) = picked else {
                return Ok(json!({ "cancelled": true }));
            };
            let path = fp.into_path().map_err(|e| e.to_string())?;
            fs::write(&path, &png_bytes).map_err(|e| e.to_string())?;
            Ok(json!({
                "saved": true,
                "path": path.to_string_lossy().to_string(),
            }))
        }

        other => Err(format!("Action inconnue: {}", other)),
    }
}
