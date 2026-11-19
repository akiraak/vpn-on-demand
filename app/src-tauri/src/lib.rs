//! VPN on Demand: a Vultr server as a Tailscale exit node, only while you need it.
//! All logic lives here in Rust; the web UI only displays state and sends commands.

// `pub` so that development tools (`examples/measure.rs`) can drive the same flows.
pub mod cloud_init;
pub mod error;
mod secrets;
mod settings;
pub mod tailscale;
mod tray;
pub mod vpn;
pub mod vultr;

use error::VpnError;
use serde::Serialize;
use settings::Settings;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use vpn::{Connection, IpInfoProbe, Server, Step, Vpn};

// ---------- state ----------

#[derive(Serialize, Clone, Copy, Default, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Starting,
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

/// Everything the UI shows. Sent to the UI as the `state` event on every change.
#[derive(Serialize, Clone, Default, Debug)]
pub struct UiState {
    pub phase: Phase,
    pub step: Option<Step>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub connection: Option<Connection>,
    /// Servers that exist (and cost money) but aren't in use.
    pub leftover: Vec<Server>,
    /// Unix time in ms.
    pub auto_disconnect_at: Option<u64>,
    pub keys_ready: bool,
    pub tailscale_error: Option<String>,
    /// Region being connected to (while connecting).
    pub target_region: Option<String>,
}

pub struct App {
    state: Mutex<UiState>,
    busy: AtomicBool,
    cancel: Arc<AtomicBool>,
    settings: Mutex<Settings>,
    config_dir: PathBuf,
    deadline: Mutex<Option<Instant>>,
    warned: AtomicBool,
    quitting: AtomicBool,
    told_about_tray: AtomicBool,
    regions: Mutex<Vec<vultr::Region>>,
}

fn app_state(app: &AppHandle) -> tauri::State<'_, App> {
    app.state::<App>()
}

fn update(app: &AppHandle, f: impl FnOnce(&mut UiState)) {
    let snapshot = {
        let a = app_state(app);
        let mut st = a.state.lock().unwrap();
        f(&mut st);
        st.clone()
    };
    let _ = app.emit("state", &snapshot);
    tray::refresh(app, &snapshot);
}

fn snapshot(app: &AppHandle) -> UiState {
    app_state(app).state.lock().unwrap().clone()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// City name for a region id, for menus and notifications.
fn city(app: &AppHandle, region: &str) -> String {
    app_state(app)
        .regions
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.id == region)
        .map(|r| r.city.clone())
        .unwrap_or_else(|| region.to_string())
}

fn country_of(app: &AppHandle, region: &str) -> Option<String> {
    app_state(app)
        .regions
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.id == region)
        .map(|r| r.country.clone())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("Internal error: {e}"))
}

async fn key(name: &'static str) -> Result<Option<String>, String> {
    blocking(move || secrets::get(name)).await?
}

async fn keys_ready() -> bool {
    matches!(key(secrets::VULTR_API_KEY).await, Ok(Some(_)))
        && matches!(key(secrets::TAILSCALE_AUTH_KEY).await, Ok(Some(_)))
}

async fn make_vpn() -> Result<Vpn<vultr::Vultr, tailscale::Tailscale, IpInfoProbe>, VpnError> {
    let k = key(secrets::VULTR_API_KEY)
        .await
        .map_err(VpnError::Setup)?
        .unwrap_or_default();
    Ok(Vpn {
        cloud: vultr::Vultr::new(k)?,
        mesh: tailscale::Tailscale,
        probe: IpInfoProbe,
        timing: vpn::Timing::default(),
    })
}

fn set_deadline(app: &AppHandle) {
    let a = app_state(app);
    let hours = a.settings.lock().unwrap().auto_disconnect_hours;
    let secs = hours.map(|h| h as u64 * 3600).map(|s| {
        // For testing the timer without waiting hours.
        std::env::var("VOD_TEST_AUTO_DISCONNECT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(s)
    });
    let mut d = a.deadline.lock().unwrap();
    *d = secs.map(|s| Instant::now() + Duration::from_secs(s));
    a.warned.store(false, Ordering::SeqCst);
    drop(d);
    let at = secs.map(|s| now_ms() + s * 1000);
    update(app, |st| st.auto_disconnect_at = at);
}

fn clear_deadline(app: &AppHandle) {
    *app_state(app).deadline.lock().unwrap() = None;
    update(app, |st| st.auto_disconnect_at = None);
}

/// Runs `f` as the only operation in progress.
fn try_begin(app: &AppHandle) -> Result<(), String> {
    if app_state(app).busy.swap(true, Ordering::SeqCst) {
        return Err("Another operation is in progress.".into());
    }
    Ok(())
}

fn end(app: &AppHandle) {
    app_state(app).busy.store(false, Ordering::SeqCst);
    let s = snapshot(app);
    tray::refresh(app, &s);
}

// ---------- operations ----------

async fn load_regions(app: &AppHandle) {
    if !app_state(app).regions.lock().unwrap().is_empty() {
        return;
    }
    if let Ok(r) = vultr::regions().await {
        *app_state(app).regions.lock().unwrap() = r;
    }
}

/// Looks at Vultr and Tailscale and works out the current state. Used at startup, after
/// failures, and when something changed outside the app.
async fn inspect(app: &AppHandle) {
    load_regions(app).await;
    let ready = keys_ready().await;
    let mesh = vpn::Mesh::status(&tailscale::Tailscale).await;
    let tailscale_error = match &mesh {
        Ok(st) if st.running() => None,
        Ok(st) => Some(format!(
            "Tailscale is not connected (state: {}). Open Tailscale and sign in.",
            st.backend_state
        )),
        Err(e) => Some(e.to_string()),
    };

    let servers = match make_vpn().await {
        Ok(v) => match vpn::Cloud::list_servers(&v.cloud).await {
            Ok(s) => Ok(Some((v, s))),
            Err(e) => Err(e.to_string()),
        },
        Err(VpnError::Setup(_)) => Ok(None), // no Vultr key yet
        Err(e) => Err(e.to_string()),
    };

    let previous = snapshot(app);
    let mut next = UiState {
        phase: Phase::Disconnected,
        keys_ready: ready,
        tailscale_error,
        auto_disconnect_at: previous.auto_disconnect_at,
        ..Default::default()
    };

    match servers {
        Err(e) => next.error = Some(e),
        Ok(None) => {}
        Ok(Some((v, servers))) => {
            let ours = mesh.as_ref().ok().and_then(|m| m.our_exit_node().cloned());
            let in_use = ours.as_ref().and_then(|p| {
                servers
                    .iter()
                    .find(|s| vpn::hostname_for(&s.region) == p.host_name)
            });
            if let Some(server) = in_use {
                let hourly_cost =
                    vpn::Cloud::hourly_cost(&v.cloud, &server.plan, &server.region).await;
                let public_ip = previous
                    .connection
                    .as_ref()
                    .filter(|c| c.server.id == server.id)
                    .and_then(|c| c.public_ip.clone());
                let public_ip = match public_ip {
                    Some(ip) => Some(ip),
                    None => v.public_ip_with_retry().await,
                };
                next.phase = Phase::Connected;
                next.connection = Some(Connection {
                    host_name: vpn::hostname_for(&server.region),
                    expected_country: country_of(app, &server.region).unwrap_or_default(),
                    server: server.clone(),
                    hourly_cost,
                    public_ip,
                });
                next.leftover = servers
                    .iter()
                    .filter(|s| s.id != server.id)
                    .cloned()
                    .collect();
            } else {
                next.leftover = servers;
                if previous.phase == Phase::Connected && !next.leftover.is_empty() {
                    next.notice = Some(
                        "The exit node was turned off outside this app. The server is still \
                         running and costs money until you disconnect."
                            .into(),
                    );
                }
            }
        }
    }

    let connected = next.phase == Phase::Connected;
    let had_deadline = app_state(app).deadline.lock().unwrap().is_some();
    update(app, |st| *st = next);
    if connected && !had_deadline {
        set_deadline(app);
    } else if !connected {
        clear_deadline(app);
    }
}

async fn run_connect(app: AppHandle, region: String, country: String) {
    let a = app_state(&app);
    a.cancel.store(false, Ordering::SeqCst);
    {
        let mut s = a.settings.lock().unwrap();
        s.last_region = region.clone();
        s.last_country = country.clone();
        let _ = s.save(&a.config_dir);
    }
    update(&app, |st| {
        st.phase = Phase::Connecting;
        st.step = None;
        st.error = None;
        st.notice = None;
        st.target_region = Some(region.clone());
    });

    let result = async {
        let auth_key = key(secrets::TAILSCALE_AUTH_KEY)
            .await
            .map_err(VpnError::Setup)?
            .unwrap_or_default();
        let v = make_vpn().await?;
        let report_app = app.clone();
        let report = move |step: Step| update(&report_app, |st| st.step = Some(step));
        let cancel = app_state(&app).cancel.clone();
        v.connect(
            &vpn::ConnectRequest {
                region: &region,
                country: &country,
                auth_key: &auth_key,
            },
            &report,
            &cancel,
        )
        .await
    }
    .await;

    match result {
        Ok(c) => {
            let where_ = city(&app, &c.server.region);
            let mismatch = c.country_matches() == Some(false);
            let ip = c.public_ip.clone();
            update(&app, |st| {
                st.phase = Phase::Connected;
                st.step = None;
                st.connection = Some(c);
                st.leftover.clear();
                st.target_region = None;
                if mismatch {
                    st.notice = Some(format!(
                        "Connected, but your public IP address looks like it is in {}. Some \
                         sites may still treat you as outside the country.",
                        ip.map(|i| i.country).unwrap_or_default()
                    ));
                }
            });
            set_deadline(&app);
            notify(
                &app,
                "Connected",
                &format!("Your traffic now goes out through {where_}."),
            );
        }
        Err(e) => {
            let cancelled = e == VpnError::Cancelled;
            update(&app, |st| {
                st.phase = Phase::Disconnected;
                st.step = None;
                st.connection = None;
                st.target_region = None;
                st.error = if cancelled { None } else { Some(e.to_string()) };
            });
            inspect(&app).await;
            if !cancelled {
                update(&app, |st| st.error = Some(e.to_string()));
                if !app
                    .get_webview_window("main")
                    .is_some_and(|w| w.is_visible().unwrap_or(false))
                {
                    notify(&app, "Couldn't connect", &e.to_string());
                }
            }
        }
    }
    end(&app);
}

/// Returns true when everything was deleted.
async fn run_disconnect(app: AppHandle, notice: Option<String>) -> bool {
    update(&app, |st| {
        st.phase = Phase::Disconnecting;
        st.step = None;
        st.error = None;
        st.notice = None;
    });
    let result = async {
        let v = make_vpn().await?;
        let report_app = app.clone();
        v.disconnect(&move |step: Step| update(&report_app, |st| st.step = Some(step)))
            .await
    }
    .await;
    let ok = result.is_ok();
    match result {
        Ok(()) => {
            update(&app, |st| {
                st.phase = Phase::Disconnected;
                st.step = None;
                st.connection = None;
                st.leftover.clear();
                st.notice = notice;
            });
            clear_deadline(&app);
        }
        Err(e) => {
            inspect(&app).await;
            update(&app, |st| st.error = Some(e.to_string()));
            show_main(&app);
            notify(
                &app,
                "Couldn't delete the server",
                "Open VPN on Demand and press Disconnect again.",
            );
        }
    }
    end(&app);
    ok
}

/// Connect from the tray, to the last region.
fn connect_last(app: &AppHandle) {
    let (region, country) = {
        let a = app_state(app);
        let s = a.settings.lock().unwrap();
        (s.last_region.clone(), s.last_country.clone())
    };
    if try_begin(app).is_ok() {
        tauri::async_runtime::spawn(run_connect(app.clone(), region, country));
    }
}

/// Disconnect from the tray or UI. While connecting, this cancels (the flow cleans up).
fn disconnect_or_cancel(app: &AppHandle) -> Result<(), String> {
    let st = snapshot(app);
    if st.phase == Phase::Connecting {
        app_state(app).cancel.store(true, Ordering::SeqCst);
        return Ok(());
    }
    try_begin(app)?;
    tauri::async_runtime::spawn(run_disconnect(app.clone(), None));
    Ok(())
}

fn has_servers(st: &UiState) -> bool {
    st.connection.is_some()
        || !st.leftover.is_empty()
        || matches!(st.phase, Phase::Connecting | Phase::Disconnecting)
}

/// Quit, but ask first if a server is still running.
fn request_quit(app: &AppHandle) {
    if has_servers(&snapshot(app)) {
        show_main(app);
        let _ = app.emit("confirm-quit", ());
    } else {
        quit_now(app);
    }
}

fn quit_now(app: &AppHandle) {
    app_state(app).quitting.store(true, Ordering::SeqCst);
    app.exit(0);
}

/// Auto-disconnect and watching for changes made outside the app.
async fn background(app: AppHandle) {
    let mut tick: u64 = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        tick += 1;
        let a = app_state(&app);
        if a.busy.load(Ordering::SeqCst) || snapshot(&app).phase != Phase::Connected {
            continue;
        }
        let deadline = *a.deadline.lock().unwrap();
        if let Some(d) = deadline {
            let now = Instant::now();
            if now >= d {
                if try_begin(&app).is_ok() {
                    let hours = a
                        .settings
                        .lock()
                        .unwrap()
                        .auto_disconnect_hours
                        .unwrap_or(0);
                    let msg = format!(
                        "Disconnected automatically after {hours} h, and deleted the server."
                    );
                    if run_disconnect(app.clone(), Some(msg.clone())).await {
                        notify(&app, "Disconnected", &msg);
                    }
                }
                continue;
            }
            if d - now <= Duration::from_secs(10 * 60) && !a.warned.swap(true, Ordering::SeqCst) {
                notify(
                    &app,
                    "Disconnecting in 10 minutes",
                    "VPN on Demand will disconnect and delete the server soon. Open the app to keep it longer.",
                );
            }
        }
        // Every minute, check that our exit node is still in use.
        if tick.is_multiple_of(2)
            && let Ok(st) = vpn::Mesh::status(&tailscale::Tailscale).await
            && st.our_exit_node().is_none()
            && !a.busy.load(Ordering::SeqCst)
        {
            inspect(&app).await;
        }
    }
}

// ---------- commands ----------

#[tauri::command]
fn get_state(app: AppHandle) -> UiState {
    snapshot(&app)
}

#[tauri::command]
async fn refresh(app: AppHandle) -> Result<(), String> {
    try_begin(&app)?;
    inspect(&app).await;
    end(&app);
    Ok(())
}

#[tauri::command]
async fn list_countries(app: AppHandle) -> Result<Vec<vultr::Country>, String> {
    let regions = vultr::regions().await.map_err(|e| e.to_string())?;
    *app_state(&app).regions.lock().unwrap() = regions.clone();
    Ok(vultr::group_by_country(regions))
}

#[tauri::command]
fn connect(app: AppHandle, region: String, country: String) -> Result<(), String> {
    try_begin(&app)?;
    tauri::async_runtime::spawn(run_connect(app, region, country));
    Ok(())
}

#[tauri::command]
fn disconnect(app: AppHandle) -> Result<(), String> {
    disconnect_or_cancel(&app)
}

#[tauri::command]
fn extend_auto_disconnect(app: AppHandle) {
    if snapshot(&app).phase == Phase::Connected {
        set_deadline(&app);
    }
}

#[tauri::command]
async fn get_keys() -> Result<Vec<secrets::Saved>, String> {
    blocking(|| {
        secrets::NAMES
            .iter()
            .map(|n| secrets::load(n))
            .collect::<Result<Vec<_>, _>>()
    })
    .await?
}

#[tauri::command]
async fn save_key(app: AppHandle, name: String, value: String) -> Result<(), String> {
    blocking(move || secrets::save(&name, &value)).await??;
    let ready = keys_ready().await;
    update(&app, |st| st.keys_ready = ready);
    Ok(())
}

#[tauri::command]
async fn delete_key(app: AppHandle, name: String) -> Result<(), String> {
    blocking(move || secrets::delete(&name)).await??;
    update(&app, |st| st.keys_ready = false);
    Ok(())
}

#[derive(Serialize)]
struct Check {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
struct SetupReport {
    tailscale: Check,
    vultr: Check,
    auth_key: Check,
}

#[tauri::command]
async fn test_setup() -> SetupReport {
    let tailscale = match vpn::Mesh::status(&tailscale::Tailscale).await {
        Ok(st) if st.running() => Check {
            ok: true,
            message: format!(
                "Running. This PC is \"{}\" in {}.",
                st.self_host_name, st.tailnet
            ),
        },
        Ok(st) => Check {
            ok: false,
            message: format!(
                "Not connected (state: {}). Open Tailscale and sign in.",
                st.backend_state
            ),
        },
        Err(e) => Check {
            ok: false,
            message: e.to_string(),
        },
    };
    let vultr = match key(secrets::VULTR_API_KEY).await {
        Ok(Some(k)) => match vultr::Vultr::new(k) {
            Ok(v) => match v.check().await {
                Ok(m) => Check {
                    ok: true,
                    message: m,
                },
                Err(e) => Check {
                    ok: false,
                    message: e.to_string(),
                },
            },
            Err(e) => Check {
                ok: false,
                message: e.to_string(),
            },
        },
        Ok(None) => Check {
            ok: false,
            message: "Not set.".into(),
        },
        Err(e) => Check {
            ok: false,
            message: e,
        },
    };
    let auth_key = match key(secrets::TAILSCALE_AUTH_KEY).await {
        Ok(Some(k)) => match cloud_init::validate_auth_key(&k) {
            Ok(()) => Check {
                ok: true,
                message: "Looks valid. Whether it works is only known when a server joins.".into(),
            },
            Err(e) => Check {
                ok: false,
                message: e.to_string(),
            },
        },
        Ok(None) => Check {
            ok: false,
            message: "Not set.".into(),
        },
        Err(e) => Check {
            ok: false,
            message: e,
        },
    };
    SetupReport {
        tailscale,
        vultr,
        auth_key,
    }
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    app_state(&app).settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let a = app_state(&app);
    let changed_hours = {
        let mut s = a.settings.lock().unwrap();
        let new = settings.normalized();
        let changed = s.auto_disconnect_hours != new.auto_disconnect_hours;
        *s = new;
        s.save(&a.config_dir)?;
        changed
    };
    if changed_hours && snapshot(&app).phase == Phase::Connected {
        set_deadline(&app);
    }
    Ok(())
}

/// Answer to the quit dialog.
#[tauri::command]
async fn quit_app(app: AppHandle, delete: bool) -> Result<(), String> {
    if !delete {
        quit_now(&app);
        return Ok(());
    }
    if snapshot(&app).phase == Phase::Connecting {
        app_state(&app).cancel.store(true, Ordering::SeqCst);
        // Wait for the connect flow to clean up.
        for _ in 0..600 {
            if !app_state(&app).busy.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    try_begin(&app)?;
    if run_disconnect(app.clone(), None).await {
        quit_now(&app);
        Ok(())
    } else {
        Err("Couldn't delete the server, so the app is still running. Try again.".into())
    }
}

// ---------- app ----------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let config_dir = app.path().app_config_dir()?;
            app.manage(App {
                state: Mutex::new(UiState::default()),
                busy: AtomicBool::new(false),
                cancel: Arc::new(AtomicBool::new(false)),
                settings: Mutex::new(Settings::load(&config_dir)),
                config_dir,
                deadline: Mutex::new(None),
                warned: AtomicBool::new(false),
                quitting: AtomicBool::new(false),
                told_about_tray: AtomicBool::new(false),
                regions: Mutex::new(Vec::new()),
            });
            tray::build(app.handle())?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let _ = try_begin(&handle);
                inspect(&handle).await;
                end(&handle);
                let st = snapshot(&handle);
                if !st.leftover.is_empty() {
                    show_main(&handle);
                    notify(
                        &handle,
                        "A server is still running",
                        "A VPN on Demand server from before is still running and costs money. Open the app to delete or use it.",
                    );
                }
                background(handle).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the app in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
                let app = window.app_handle();
                if !app_state(app).told_about_tray.swap(true, Ordering::SeqCst) {
                    notify(app, "Still running", "VPN on Demand keeps running in the tray. Use Quit in the tray menu to exit.");
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            refresh,
            list_countries,
            connect,
            disconnect,
            extend_auto_disconnect,
            get_keys,
            save_key,
            delete_key,
            test_setup,
            get_settings,
            save_settings,
            quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the app");

    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, .. } = event
            && !app_state(app).quitting.load(Ordering::SeqCst)
        {
            api.prevent_exit();
            request_quit(app);
        }
    });
}
