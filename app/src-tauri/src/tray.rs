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

/// Tray icons: the app's route glyph with the exit dot in the state color.
/// Sources are `icons/tray/*.svg`.
const OFF: &[u8] = include_bytes!("../icons/tray/off.png");
const BUSY: &[u8] = include_bytes!("../icons/tray/busy.png");
const ON: &[u8] = include_bytes!("../icons/tray/on.png");

fn icon(png: &'static [u8]) -> Image<'static> {
    Image::from_bytes(png).expect("embedded tray icon")
}

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
        .icon(icon(OFF))
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
    let (png, status) = match st.phase {
        Phase::Connected => {
            let city = st
                .connection
                .as_ref()
                .map(|c| crate::city(app, &c.server.region))
                .unwrap_or_default();
            (ON, format!("Connected – {city}"))
        }
        Phase::Connecting => (BUSY, "Connecting…".to_string()),
        Phase::Disconnecting => (BUSY, "Disconnecting…".to_string()),
        Phase::Starting => (OFF, "Starting…".to_string()),
        Phase::Disconnected if !st.leftover.is_empty() => {
            (BUSY, "A server is still running".to_string())
        }
        Phase::Disconnected => (OFF, "Not connected".to_string()),
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(icon(png)));
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

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_icons_decode() {
        for png in [super::OFF, super::BUSY, super::ON] {
            let img = super::icon(png);
            assert_eq!((img.width(), img.height()), (32, 32));
        }
    }
}
