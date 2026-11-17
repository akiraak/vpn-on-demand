//! Tray (Windows) / menu bar (macOS) icon and menu.

use crate::{App, Phase, UiState};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

const TRAY_ID: &str = "main";

pub struct TrayItems {
    connect: MenuItem<Wry>,
    disconnect: MenuItem<Wry>,
}

/// A round status icon drawn at runtime: gray = off, amber = working, green = connected.
fn icon(rgb: [u8; 3]) -> Image<'static> {
    const N: u32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let i = ((y * N + x) * 4) as usize;
            // Filled disc with a soft edge, and a white ring inside it.
            let outer = (14.5 - d).clamp(0.0, 1.0);
            let ring = (1.0 - ((d - 8.0).abs() - 1.2).clamp(0.0, 1.0)) * (d < 10.0) as u8 as f32;
            let (r, g, b) = (
                rgb[0] as f32 * (1.0 - ring) + 255.0 * ring,
                rgb[1] as f32 * (1.0 - ring) + 255.0 * ring,
                rgb[2] as f32 * (1.0 - ring) + 255.0 * ring,
            );
            px[i] = r as u8;
            px[i + 1] = g as u8;
            px[i + 2] = b as u8;
            px[i + 3] = (outer * 255.0) as u8;
        }
    }
    Image::new_owned(px, N, N)
}

const GRAY: [u8; 3] = [0x8b, 0x94, 0x9e];
const AMBER: [u8; 3] = [0xd2, 0x99, 0x22];
const GREEN: [u8; 3] = [0x2d, 0xa4, 0x4e];

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open VPN on Demand", true, None::<&str>)?;
    let connect = MenuItem::with_id(app, "connect", "Connect", false, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, "disconnect", "Disconnect", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &connect,
            &disconnect,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    app.manage(TrayItems {
        connect,
        disconnect,
    });

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(GRAY))
        .tooltip("VPN on Demand")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => crate::show_main(app),
            "connect" => crate::connect_last(app),
            "disconnect" => {
                let _ = crate::disconnect_or_cancel(app);
            }
            "quit" => crate::request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
                && !cfg!(target_os = "macos")
            {
                crate::show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Updates the icon, tooltip and menu for the state.
pub fn refresh(app: &AppHandle, st: &UiState) {
    let busy = app
        .state::<App>()
        .busy
        .load(std::sync::atomic::Ordering::SeqCst);
    let (rgb, status) = match st.phase {
        Phase::Connected => {
            let city = st
                .connection
                .as_ref()
                .map(|c| crate::city(app, &c.server.region))
                .unwrap_or_default();
            (GREEN, format!("Connected – {city}"))
        }
        Phase::Connecting => (AMBER, "Connecting…".to_string()),
        Phase::Disconnecting => (AMBER, "Disconnecting…".to_string()),
        Phase::Starting => (GRAY, "Starting…".to_string()),
        Phase::Disconnected if !st.leftover.is_empty() => {
            (AMBER, "A server is still running".to_string())
        }
        Phase::Disconnected => (GRAY, "Not connected".to_string()),
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(icon(rgb)));
        let _ = tray.set_tooltip(Some(format!("VPN on Demand – {status}")));
    }
    let Some(items) = app.try_state::<TrayItems>() else {
        return;
    };
    let last = app
        .state::<App>()
        .settings
        .lock()
        .unwrap()
        .last_region
        .clone();
    let _ = items
        .connect
        .set_text(format!("Connect – {}", crate::city(app, &last)));
    let _ = items.connect.set_enabled(
        !busy && st.keys_ready && st.phase == Phase::Disconnected && st.leftover.is_empty(),
    );
    let (text, enabled) = match st.phase {
        Phase::Connecting => ("Cancel", true),
        Phase::Disconnecting => ("Disconnecting…", false),
        _ => (
            "Disconnect",
            !busy && (st.connection.is_some() || !st.leftover.is_empty()),
        ),
    };
    let _ = items.disconnect.set_text(text);
    let _ = items.disconnect.set_enabled(enabled);
}
