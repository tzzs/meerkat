#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod audio;
mod history;
mod ipc;
mod menu;
mod metrics;
mod models;
mod nvml;
mod settings;
mod tray;
mod tray_render;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use crate::alerts::Watcher;
use crate::history::History;
use crate::ipc::AppState;
use crate::metrics::Sampler;
use crate::settings::SettingsStore;
use crate::tray::TrayManager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            ipc::snapshot,
            ipc::settings_get,
            ipc::settings_set,
            ipc::history_series,
            ipc::history_ranking,
            ipc::insights,
            ipc::insight_dismiss,
            ipc::quit_app,
            ipc::set_tray_bitmap,
            ipc::tray_ids,
            ipc::hide_panel,
            ipc::show_window,
            ipc::set_panel_size,
            ipc::set_panel_pinned,
            ipc::copy_report,
            ipc::app_version,
            ipc::audio_state,
            ipc::audio_set_volume,
            ipc::audio_set_endpoint
        ])
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                // Closing the window keeps Pulse in the tray, like the Mac version.
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
                ipc::refresh_visibility(window.app_handle());
            }
            WindowEvent::Focused(false) => {
                if window.label() == "panel" && !panel_pinned(window) {
                    let _ = window.hide();
                }
                ipc::refresh_visibility(window.app_handle());
            }
            _ => {}
        })
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data).ok();
            let settings = SettingsStore::open(&app_data);
            let initial = settings.get();
            app.manage(AppState {
                latest: RwLock::new(None),
                settings,
                history: Mutex::new(History::open(&app_data)),
                watcher: Mutex::new(Watcher::open(&app_data)),
                tray: Mutex::new(TrayManager::new()),
                interface_visible: AtomicBool::new(false),
                panel_pinned: AtomicBool::new(false),
                data_dir: app_data,
                app: OnceLock::new(),
            });

            let handle = app.handle().clone();
            handle
                .state::<AppState>()
                .tray
                .lock()
                .unwrap()
                .rebuild(&handle, &initial);

            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.set_skip_taskbar(!initial.show_in_taskbar);
            }

            spawn_sampler(handle.clone());

            // Restore remembered app volumes before anything starts playing sound.
            if audio::init_com().is_ok() {
                if let Ok(dir) = handle.path().app_data_dir() {
                    match audio::apply_saved_volumes(&dir.join("audio-volumes.json")) {
                        Ok(0) => {}
                        Ok(count) => println!("restored {count} saved app volumes"),
                        Err(error) => eprintln!("restore saved volumes: {error}"),
                    }
                }
            }

            // Opened by hand it shows its window; started at sign-in it stays in the tray.
            let at_startup = std::env::args().any(|a| a == "--autostart");
            if at_startup {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            } else {
                menu::show_main(&handle, None);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Pulse failed to start");
}

fn panel_pinned(window: &tauri::Window) -> bool {
    window
        .try_state::<AppState>()
        .map(|state| state.panel_pinned.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn spawn_sampler(app: AppHandle) {
    std::thread::Builder::new()
        .name("pulse-sampler".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            let _ = state.app.set(app.clone());
            let mut sampler = Sampler::new();
            loop {
                let settings = state.settings.get();
                let snapshot = sampler.sample();

                *state.latest.write().unwrap_or_else(|p| p.into_inner()) = Some(snapshot.clone());
                state.history.lock().unwrap_or_else(|p| p.into_inner()).record(&snapshot);

                for insight in state.watcher.lock().unwrap_or_else(|p| p.into_inner()).evaluate(&snapshot, &settings) {
                    let _ = app.emit("insight", &insight);
                    if settings.alerts.enabled {
                        let _ = app
                            .notification()
                            .builder()
                            .title(format!("Pulse: {}", insight.app_name))
                            .body(humanize(&insight))
                            .show();
                    }
                }

                state
                    .tray
                    .lock()
                    .unwrap()
                    .update(&app, &snapshot, &settings);

                if state.visible() {
                    let _ = app.emit("metrics", &snapshot);
                }
                #[cfg(debug_assertions)]
                dump_for_dev_browser(&state, &snapshot);

                std::thread::sleep(std::time::Duration::from_millis(tick_ms(
                    state.visible(),
                    snapshot.battery.on_ac_power,
                )));
            }
        })
        .expect("the sampler thread must start");
}

/// Nothing open means nothing to refresh at 1 Hz; on battery the Mac version halves its
/// own work, so do the same here.
fn tick_ms(visible: bool, on_ac: bool) -> u64 {
    let base = if visible { 1000 } else { 2000 };
    if on_ac {
        base
    } else {
        base * 2
    }
}

/// Dev-only: mirror the state to disk so `pnpm dev` in a plain browser shows the same
/// real figures as the app window. Debug builds only.
#[cfg(debug_assertions)]
fn dump_for_dev_browser(state: &AppState, snapshot: &models::Snapshot) {
    use std::sync::atomic::AtomicU64;
    static TICK: AtomicU64 = AtomicU64::new(0);
    let dir = state.data_dir.join("dev");
    let _ = std::fs::create_dir_all(&dir);
    let tick = TICK.fetch_add(1, Ordering::Relaxed);
    if let Ok(raw) = serde_json::to_string(snapshot) {
        let _ = std::fs::write(dir.join("latest.json"), raw);
    }
    if tick % 20 != 0 {
        return;
    }
    if let Ok(raw) = serde_json::to_string(&state.watcher.lock().unwrap_or_else(|p| p.into_inner()).list()) {
        let _ = std::fs::write(dir.join("insights.json"), raw);
    }
    // Runs on the sampler thread on purpose: Core Audio needs COM, and this is the only
    // place that proves it initialises correctly away from the main thread.
    match ipc::snapshot_audio() {
        Ok(snapshot) => {
            if let Ok(raw) = serde_json::to_string(&snapshot) {
                let _ = std::fs::write(dir.join("audio.json"), raw);
            }
        }
        Err(error) => eprintln!("audio snapshot: {error}"),
    }
    let now = history::now_seconds();
    let store = state.history.lock().unwrap_or_else(|p| p.into_inner());
    for range in [3600u64, 43200, 86400, 604800, 2592000] {
        for metric in ["cpu", "memory", "diskWrite", "download", "gpu", "battery", "temp"] {
            if let Ok(raw) = serde_json::to_string(&store.series(metric, range, now)) {
                let _ = std::fs::write(dir.join(format!("series-{metric}-{range}.json")), raw);
            }
        }
    }
}

fn humanize(insight: &models::Insight) -> String {
    match insight.kind {
        models::InsightKind::HighCpu => format!(
            "has used {}% of the CPU for {}s",
            insight.value.round(),
            insight.sustained_seconds
        ),
        models::InsightKind::GrowingMemory => format!(
            "grew by {:.0} MB and is still climbing",
            insight.value / 1_048_576.0
        ),
        models::InsightKind::HeavyDisk => format!(
            "has moved {:.1} MB/s to disk for {}s",
            insight.value / 1_048_576.0,
            insight.sustained_seconds
        ),
        models::InsightKind::HeavyNetwork => format!(
            "this machine has been transferring {:.1} MB/s for {}s, and this app holds the most connections",
            insight.value / 1_048_576.0,
            insight.sustained_seconds
        ),
        models::InsightKind::Heat => format!(
            "this machine reached {:.0}°C, and this app is using the most CPU",
            insight.value
        ),
    }
}
