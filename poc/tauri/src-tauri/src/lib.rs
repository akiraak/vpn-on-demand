//! VPN on Demand の試作（技術選定の Step 2）。
//! トレイ / メニューバー、Tailscale の CLI、資格情報ストア、Vultr の API を確かめる。VPS は作らない。

mod secrets;
mod tailscale;
mod vultr;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("内部エラー: {e}"))?
}

#[tauri::command]
async fn tailscale_status() -> Result<tailscale::Status, String> {
    blocking(tailscale::status).await
}

#[tauri::command]
async fn vultr_countries() -> Result<Vec<vultr::Country>, String> {
    vultr::countries().await
}

#[tauri::command]
async fn secret_save(name: String, value: String) -> Result<(), String> {
    blocking(move || secrets::save(&name, &value)).await
}

#[tauri::command]
async fn secret_load(name: String) -> Result<secrets::Saved, String> {
    blocking(move || secrets::load(&name)).await
}

#[tauri::command]
async fn secret_delete(name: String) -> Result<(), String> {
    blocking(move || secrets::delete(&name)).await
}

/// 窓を出して、指定したタブを開かせる。
fn show_tab(app: &AppHandle, tab: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = w.emit("show-tab", tab);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // macOS: ドックに出さず、メニューバーだけのアプリにする
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let status = MenuItem::with_id(app, "status", "状態を見る", true, None::<&str>)?;
            let regions = MenuItem::with_id(app, "regions", "国とリージョン", true, None::<&str>)?;
            let settings = MenuItem::with_id(app, "settings", "設定", true, None::<&str>)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&status, &regions, &settings, &sep, &quit])?;

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().expect("アイコンが無い").clone())
                .tooltip("VPN on Demand（試作）")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    tab => show_tab(app, tab),
                })
                .build(app)?;
            Ok(())
        })
        // 窓を閉じても終了せず、トレイに残る
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            tailscale_status,
            vultr_countries,
            secret_save,
            secret_load,
            secret_delete
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
