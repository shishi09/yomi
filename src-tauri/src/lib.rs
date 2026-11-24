pub mod bridge;
pub mod cache;
pub mod config;
pub mod glossary;
pub mod image_proc;
pub mod llm;
pub mod ocr;
pub mod prompts;
pub mod server;
pub mod som;
pub mod tray;

use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, WindowEvent};

use bridge::{inspect_system_state, sync_userscript_if_configured, AppRuntime};
use cache::{prune_disk_cache, PrefetchCache};
use config::{ensure_data_dirs, get_default_model, load_config, load_dotenv, resolve_data_dir};
use glossary::ensure_default_glossaries;
use llm::{check_ollama_status_at, ollama_base_url, warmup_ollama_model_at};
use server::ServerState;
use tray::{refresh_tray_status, setup_tray};

fn resolve_server_port() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .filter(|&p| p > 0)
        .unwrap_or(8765)
}

fn build_server_state(resource_dir: Option<PathBuf>) -> Arc<ServerState> {
    let data_dir = resolve_data_dir();
    ensure_data_dirs(&data_dir);

    let exe_parent = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if let Some(ref res) = resource_dir {
        let ex1 = res.join("glossaries/examples");
        let ex2 = res.join("_up_/glossaries/examples");
        if ex1.is_dir() {
            ensure_default_glossaries(&data_dir, Some(&ex1));
        } else if ex2.is_dir() {
            ensure_default_glossaries(&data_dir, Some(&ex2));
        } else {
            ensure_default_glossaries(&data_dir, None);
        }
    } else if let Some(ref exe_d) = exe_parent {
        let dev_ex = exe_d.join("../../../glossaries/examples");
        ensure_default_glossaries(&data_dir, if dev_ex.is_dir() { Some(&dev_ex) } else { None });
    } else {
        ensure_default_glossaries(&data_dir, None);
    }

    let _ = load_dotenv(&data_dir.join(".env"));
    if let Some(ref exe_d) = exe_parent {
        let _ = load_dotenv(&exe_d.join(".env"));
    }

    Arc::new(ServerState {
        data_dir,
        resource_dir,
        prefetch: Arc::new(PrefetchCache::new()),
        ollama_url: ollama_base_url(),
    })
}

pub fn run_self_test() -> i32 {
    let rt_tokio = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Tokio runtime error: {}", e);
            return 1;
        }
    };
    rt_tokio.block_on(async {
        let state = build_server_state(None);
        let port = resolve_server_port();
        let runtime = Arc::new(AppRuntime::new(state.clone(), port));
        let health = server::build_health_payload(&state).await;
        let mut report = inspect_system_state(&runtime).await;
        if let Some(obj) = report.as_object_mut() {
            obj.insert("server_health".to_string(), health);
        }
        match serde_json::to_string_pretty(&report) {
            Ok(txt) => println!("{}", txt),
            Err(e) => {
                eprintln!("Serialization error: {}", e);
                return 1;
            }
        }
        0
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let res_dir = app.path().resource_dir().ok();
            let server_state = build_server_state(res_dir);
            let port = resolve_server_port();
            let runtime = Arc::new(AppRuntime::new(server_state.clone(), port));
            app.manage(runtime.clone());

            let _ = setup_tray(app);

            // Sync Safari Userscript silently at launch if configured
            sync_userscript_if_configured(&server_state);

            // Background cache pruning & conditional embedded Axum startup
            let app_handle = app.handle().clone();
            let rt_bg = runtime.clone();
            tauri::async_runtime::spawn(async move {
                let cache_dir = rt_bg.server_state.data_dir.join("cache");
                let _ = prune_disk_cache(&cache_dir, 1500, 30);

                let cfg = load_config(&rt_bg.server_state.data_dir);
                if cfg.onboarding_done {
                    if cfg.engine_mode != "api" {
                        let _ = bridge::ensure_ollama_running(&rt_bg).await;
                    }
                    let started = rt_bg.server.start(rt_bg.server_state.clone()).await.is_ok();
                    refresh_tray_status(&app_handle, started);
                    if started && cfg.engine_mode != "api" {
                        let (ok, mods) = check_ollama_status_at(&rt_bg.server_state.ollama_url).await;
                        if ok {
                            let def = get_default_model(ok, &mods, &cfg);
                            if def.starts_with("ollama:") {
                                warmup_ollama_model_at(&rt_bg.server_state.ollama_url, &def).await;
                            }
                        }
                    }
                }
            });

            if let Some(win) = app.get_webview_window("main") {
                let win_clone = win.clone();
                let app_fs = app.handle().clone();
                win.on_window_event(move |ev| match ev {
                    WindowEvent::CloseRequested { api, .. } => {
                        #[cfg(target_os = "macos")]
                        {
                            api.prevent_close();
                            let _ = win_clone.hide();
                        }
                    }
                    WindowEvent::Resized(_) => {
                        if let Ok(is_fs) = win_clone.is_fullscreen() {
                            let _ = app_fs.emit(
                                "yomi-bridge",
                                json!({ "event": "fullscreen", "payload": { "active": is_fs } }),
                            );
                        }
                    }
                    _ => {}
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![bridge::bridge_call])
        .run(tauri::generate_context!())
        .expect("error while running Yomi");
}
