//! IPC surface. Everything the UI can ask for, and the shared state the sampler thread
//! publishes into.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::alerts::Watcher;
use crate::history::History;
use crate::menu;
use crate::models::{Insight, QuitResult, Snapshot};
use crate::settings::{Settings, SettingsStore};
use crate::tray::TrayManager;

pub struct AppState {
    pub latest: RwLock<Option<Snapshot>>,
    pub settings: SettingsStore,
    pub history: Mutex<History>,
    pub watcher: Mutex<Watcher>,
    pub tray: Mutex<TrayManager>,
    pub interface_visible: AtomicBool,
    pub panel_pinned: AtomicBool,
    pub data_dir: PathBuf,
    pub app: OnceLock<AppHandle>,
}

impl AppState {
    pub fn visible(&self) -> bool {
        self.interface_visible.load(Ordering::Relaxed)
    }
}

/// The sampler only pushes full snapshots while something is on screen, so every path
/// that shows or hides a window has to update this.
pub fn refresh_visibility(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let visible = ["main", "panel"]
        .into_iter()
        .filter_map(|label| app.get_webview_window(label))
        .any(|w| w.is_visible().unwrap_or(false));
    state
        .interface_visible
        .store(visible, std::sync::atomic::Ordering::Relaxed);
}

#[tauri::command]
pub fn snapshot(state: State<'_, AppState>) -> Option<Snapshot> {
    state.latest.read().unwrap().clone()
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Settings {
    state.settings.get()
}

#[tauri::command]
pub fn settings_set(state: State<'_, AppState>, app: AppHandle, next: Settings) -> Result<Settings, String> {
    let saved = state.settings.set(next.clone())?;
    {
        let mut tray = state.tray.lock().unwrap();
        tray.rebuild(&app, &saved);
    }
    if let Some(snapshot) = state.latest.read().unwrap().as_ref() {
        state.tray.lock().unwrap().update(&app, snapshot, &saved);
    }
    let autostart = app.autolaunch();
    if saved.start_at_login {
        let _ = autostart.enable();
    } else {
        let _ = autostart.disable();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_skip_taskbar(!saved.show_in_taskbar);
    }
    let _ = app.emit("settings-changed", ());
    Ok(saved)
}

#[tauri::command]
pub fn history_series(
    state: State<'_, AppState>,
    metric: String,
    range_seconds: u64,
) -> crate::models::HistorySeries {
    let now = crate::history::now_seconds();
    let history = state.history.lock().unwrap();
    history.series(&metric, range_seconds, now)
}

#[tauri::command]
pub fn history_ranking(state: State<'_, AppState>, hours: u64) -> Vec<crate::models::RankingRow> {
    let now = crate::history::now_seconds();
    let history = state.history.lock().unwrap();
    history.ranking(hours, now)
}

#[tauri::command]
pub fn insights(state: State<'_, AppState>) -> Vec<Insight> {
    state.watcher.lock().unwrap().list()
}

#[tauri::command]
pub fn insight_dismiss(state: State<'_, AppState>, id: String) {
    state.watcher.lock().unwrap().dismiss(&id);
}

#[tauri::command]
pub fn quit_app(state: State<'_, AppState>, key: String, force: bool) -> QuitResult {
    let Some(snapshot) = state.latest.read().unwrap().clone() else {
        return QuitResult { asked: 0, force_used: force, error: Some("no reading yet".into()) };
    };
    let Some(app) = snapshot.apps.into_iter().find(|a| a.key == key) else {
        return QuitResult { asked: 0, force_used: force, error: Some("that app is gone".into()) };
    };

    if force {
        let pids: Vec<sysinfo::Pid> = app.pids.iter().map(|p| sysinfo::Pid::from(*p as usize)).collect();
        let mut system = sysinfo::System::new();
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), false);
        let mut killed = 0usize;
        for pid in &pids {
            if let Some(process) = system.process(*pid) {
                if process.kill() {
                    killed += 1;
                }
            }
        }
        return QuitResult { asked: killed, force_used: true, error: None };
    }

    let mut asked = 0usize;
    for window in app.windows.iter().filter(|w| w.visible) {
        let hwnd = windows::Win32::Foundation::HWND(window.hwnd as *mut core::ffi::c_void);
        let posted = unsafe {
            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(hwnd),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            )
        };
        if posted.is_ok() {
            asked += 1;
        }
    }
    QuitResult {
        asked,
        force_used: false,
        error: if asked == 0 {
            Some("no window to close; use force quit".into())
        } else {
            None
        },
    }
}

#[tauri::command]
pub fn set_tray_bitmap(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    png: Vec<u8>,
) -> Result<(), String> {
    let tray = state.tray.lock().unwrap();
    tray.set_bitmap(&app, &id, &png)
}

#[tauri::command]
pub fn tray_ids(state: State<'_, AppState>) -> Vec<String> {
    state.tray.lock().unwrap().ids()
}

#[tauri::command]
pub fn hide_panel(app: AppHandle) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.hide();
    }
    refresh_visibility(&app);
}

/// "Keep open" stops the panel from closing when it loses focus.
#[tauri::command]
pub fn set_panel_pinned(state: State<'_, AppState>, pinned: bool) {
    state.panel_pinned.store(pinned, Ordering::Relaxed);
}

#[tauri::command]
pub fn show_window(app: AppHandle, destination: Option<String>) {
    menu::show_main(&app, destination.as_deref());
}

#[tauri::command]
pub fn set_panel_size(app: AppHandle, width: u32, height: u32) -> Result<(), String> {
    let panel = app.get_webview_window("panel").ok_or("no panel")?;
    panel
        .set_size(tauri::LogicalSize::new(width as f64, height as f64))
        .map_err(|e| e.to_string())
}

/// The report Settings → About copies: machine, settings, recent log. No app names.
#[tauri::command]
pub fn copy_report(state: State<'_, AppState>, app: AppHandle) -> String {
    let snapshot = state.latest.read().unwrap().clone();
    let settings = state.settings.get();
    let mut lines = Vec::new();
    lines.push(format!("Pulse {}", app.package_info().version));
    if let Some(snapshot) = snapshot.as_ref() {
        lines.push(format!(
            "{} | {} | {} logical cores | {} bytes RAM",
            snapshot.machine.hostname,
            snapshot.machine.os_version,
            snapshot.machine.logical_cores,
            snapshot.machine.total_memory_bytes
        ));
        lines.push(format!(
            "cpu {}% | memory {}% | disk r/w {:.0}/{:.0} | net d/u {:.0}/{:.0} | apps {}",
            snapshot.cpu.total_percent.round(),
            snapshot.memory.pressure_percent.round(),
            snapshot.disk.read_bytes_per_second,
            snapshot.disk.write_bytes_per_second,
            snapshot.network.download_bytes_per_second,
            snapshot.network.upload_bytes_per_second,
            snapshot.apps.len()
        ));
        lines.push(format!("sampler cost {} µs", snapshot.sampler_cost_microseconds));
    }
    lines.push(serde_json::to_string(&settings).unwrap_or_default());
    lines.join("\n")
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}
