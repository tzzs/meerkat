//! Tray menu and the panel/window show-hide plumbing.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Wry};

use crate::i18n::{self, Lang, Text};

pub fn tray_menu(app: &AppHandle, lang: Lang) -> Result<Menu<Wry>, String> {
    let open: MenuItem<Wry> = MenuItem::with_id(app, "open", i18n::text(lang, Text::OpenPulse), true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let settings: MenuItem<Wry> =
        MenuItem::with_id(app, "settings", i18n::text(lang, Text::Settings), true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let separator: PredefinedMenuItem<Wry> =
        PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let quit: MenuItem<Wry> =
        MenuItem::with_id(app, "quit", i18n::text(lang, Text::QuitPulse), true, None::<&str>)
            .map_err(|e| e.to_string())?;
    Menu::with_items(app, &[&open, &settings, &separator, &quit]).map_err(|e| e.to_string())
}

pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        "open" => show_main(app, None),
        "settings" => show_main(app, Some("settings")),
        "quit" => app.exit(0),
        _ => {}
    }
}

/// Registered once for every tray icon (see `main`), so rebuilding the icons never stacks
/// handlers.
pub fn on_tray_event(app: &AppHandle, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        position,
        rect,
        ..
    } = event
    {
        let height = match rect.size {
            tauri::Size::Physical(size) => size.height as f64,
            tauri::Size::Logical(size) => size.height,
        };
        toggle_panel(app, position.x, position.y, height);
    }
}

/// Anchor the panel under the icon that was clicked, keeping it on screen. Tray event
/// coordinates and window sizes are both physical pixels, so nothing is scaled here.
fn toggle_panel(app: &AppHandle, x: f64, y: f64, icon_height: f64) {
    let Some(panel) = app.get_webview_window("panel") else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        crate::ipc::refresh_visibility(app);
        return;
    }
    let width = panel.outer_size().map(|s| s.width as f64).unwrap_or(400.0);
    let screen_width = panel
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| m.size().width as f64)
        .unwrap_or(width * 2.0);
    let left = (x - width / 2.0).clamp(0.0, (screen_width - width).max(0.0));
    let top = y + icon_height;
    let _ = panel.set_position(PhysicalPosition::new(left as i32, top as i32));
    let _ = panel.show();
    let _ = panel.set_focus();
    let _ = app.emit_to("panel", "panel-shown", ());
    crate::ipc::refresh_visibility(app);
}

pub fn show_main(app: &AppHandle, destination: Option<&str>) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(destination) = destination {
            let _ = window.emit("navigate", destination);
        }
    }
    crate::ipc::refresh_visibility(app);
}
