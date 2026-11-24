use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};

use crate::bridge::AppRuntime;

pub struct TrayStatusHandle<R: Runtime> {
    pub status_item: MenuItem<R>,
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

pub fn emit_navigate<R: Runtime>(app: &AppHandle<R>, view: &str) {
    show_main_window(app);
    let _ = app.emit(
        "yomi-bridge",
        serde_json::json!({
            "event": "navigate",
            "payload": { "view": view }
        }),
    );
}

pub fn refresh_tray_status<R: Runtime>(app: &AppHandle<R>, running: bool) {
    if let Some(handle) = app.try_state::<TrayStatusHandle<R>>() {
        let txt = if running {
            "Serveur local actif (port 8765)"
        } else {
            "Serveur arrêté"
        };
        let _ = handle.status_item.set_text(txt);
    }
}

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

    let status_item = MenuItem::with_id(app, "tray_status", "Serveur arrêté", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let item_home = MenuItem::with_id(app, "tray_home", "Ouvrir / Accueil", true, Some("CmdOrCtrl+O"))?;
    let item_studio = MenuItem::with_id(app, "tray_studio", "Yomi Studio", true, None::<&str>)?;
    let item_settings = MenuItem::with_id(app, "tray_settings", "Réglages…", true, Some("CmdOrCtrl+,"))?;
    let item_wizard = MenuItem::with_id(app, "tray_wizard", "Relancer l'assistant…", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let item_unload = MenuItem::with_id(app, "tray_unload", "Libérer la RAM Ollama", true, None::<&str>)?;
    let item_clear = MenuItem::with_id(app, "tray_clear", "Vider le cache de traduction", true, None::<&str>)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let item_quit = MenuItem::with_id(app, "tray_quit", "Quitter Yomi", true, Some("CmdOrCtrl+Q"))?;

    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &sep1,
            &item_home,
            &item_studio,
            &item_settings,
            &item_wizard,
            &sep2,
            &item_unload,
            &item_clear,
            &sep3,
            &item_quit,
        ],
    )?;

    app.manage(TrayStatusHandle { status_item });

    TrayIconBuilder::with_id("yomi-tray")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Yomi")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                "tray_home" => emit_navigate(app, "home"),
                "tray_studio" => emit_navigate(app, "test"),
                "tray_settings" => emit_navigate(app, "settings"),
                "tray_wizard" => emit_navigate(app, "wizard"),
                "tray_unload" => {
                    if let Some(rt) = app.try_state::<Arc<AppRuntime>>() {
                        let url = rt.server_state.ollama_url.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = crate::llm::unload_ollama_models_at(&url).await;
                        });
                    }
                }
                "tray_clear" => {
                    if let Some(rt) = app.try_state::<Arc<AppRuntime>>() {
                        let cache_dir = rt.server_state.data_dir.join("cache");
                        let _ = crate::cache::clear_disk_cache(&cache_dir);
                        rt.server_state.prefetch.clear();
                    }
                }
                "tray_quit" => {
                    if let Some(rt) = app.try_state::<Arc<AppRuntime>>() {
                        let rt_clone = rt.inner().clone();
                        let app_handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            rt_clone.shutdown().await;
                            app_handle.exit(0);
                        });
                    } else {
                        app.exit(0);
                    }
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}
