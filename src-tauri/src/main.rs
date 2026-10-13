#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod apphistory;
mod audio;
mod control;
mod daily;
mod events;
mod history;
mod icons;
mod ipc;
mod mcp;
mod menu;
mod metrics;
mod models;
mod nvml;
mod rules;
mod settings;
mod tray;
mod tray_render;
mod wsl;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use crate::alerts::Watcher;
use crate::daily::Daily;
use crate::history::History;
use crate::ipc::AppState;
use crate::metrics::Sampler;
use crate::settings::SettingsStore;
use crate::tray::TrayManager;

fn main() {
    // Started by an AI assistant as its MCP server: answer on stdio by asking the running Pulse,
    // and never open a window or a tray icon of its own.
    if std::env::args().any(|arg| arg == "--mcp") {
        mcp::run_stdio();
        return;
    }
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
            ipc::app_ranking,
            ipc::app_ranking_today,
            ipc::apps_at,
            ipc::app_series,
            ipc::system_events,
            ipc::rules_list,
            ipc::rule_set,
            ipc::app_pause,
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
            ipc::audio_set_endpoint,
            ipc::app_icon,
            ipc::relaunch_elevated,
            ipc::app_processes,
            ipc::focus_window,
            ipc::reveal_in_explorer,
            ipc::open_project,
            ipc::mcp_command,
            ipc::wsl_memory,
            ipc::daily_totals
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
            let app_data = data_dir(app)?;
            std::fs::create_dir_all(&app_data).ok();
            let settings = SettingsStore::open(&app_data);
            let initial = settings.get();
            app.manage(AppState {
                latest: RwLock::new(None),
                settings,
                history: Mutex::new(History::open(&app_data)),
                daily: Mutex::new(Daily::open(&app_data)),
                processes: RwLock::new(Vec::new()),
                watcher: Mutex::new(Watcher::open(&app_data)),
                app_history: Mutex::new(apphistory::AppHistory::open(&app_data)),
                rules: Mutex::new(rules::Rules::open(&app_data)),
                events: Mutex::new(None),
                tray: Mutex::new(TrayManager::new()),
                interface_visible: AtomicBool::new(false),
                mcp_enabled: AtomicBool::new(initial.mcp_enabled),
                panel_pinned: AtomicBool::new(false),
                data_dir: app_data,
                app: OnceLock::new(),
            });

            let handle = app.handle().clone();
            handle
                .state::<AppState>()
                .tray
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .rebuild(&handle, &initial);

            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.set_skip_taskbar(!initial.show_in_taskbar);
            }

            spawn_sampler(handle.clone());
            if initial.mcp_enabled {
                mcp::start(handle.clone());
            }

            // Restore remembered app volumes before anything starts playing sound.
            if audio::init_com().is_ok() {
                let dir = handle.state::<AppState>().data_dir.clone();
                match audio::apply_saved_volumes(&dir.join("audio-volumes.json")) {
                    Ok(0) => {}
                    Ok(count) => println!("restored {count} saved app volumes"),
                    Err(error) => eprintln!("restore saved volumes: {error}"),
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
        .build(tauri::generate_context!())
        .expect("Pulse failed to start")
        .run(|app, event| {
            // A paused app must not outlive the program that paused it. A crash skips this; the
            // list on disk catches that case at the next start (rules.rs).
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.rules.lock().unwrap_or_else(|p| p.into_inner()).resume_all();
                }
            }
        });
}

/// Where settings, history and alerts live. A debug build can be pointed elsewhere with
/// `PULSE_DATA_DIR`, so a test instance can run beside the one in daily use without writing
/// over its settings or history. Nothing else can redirect it: Windows answers the known-folder
/// lookup from the user's token, not from APPDATA.
fn data_dir(app: &tauri::App) -> tauri::Result<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("PULSE_DATA_DIR") {
        return Ok(std::path::PathBuf::from(dir));
    }
    app.path().app_data_dir()
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
            // WinRT's battery report is read on this thread, and WinRT needs COM.
            if let Err(error) = audio::init_com() {
                eprintln!("sampler COM: {error}");
            }
            let mut sampler = Sampler::new();
            {
                let daily = state.daily.lock().unwrap_or_else(|p| p.into_inner());
                let today = metrics::local_day(history::now_seconds() * 1000);
                if let Some(totals) = daily.day(today) {
                    sampler.restore_day(today, totals);
                }
            }
            let mut last_sample = std::time::Instant::now();
            loop {
                let settings = state.settings.get();
                let mut snapshot = sampler.sample();
                let elapsed = last_sample.elapsed().as_secs_f64();
                let muted = {
                    let mut rules = state.rules.lock().unwrap_or_else(|p| p.into_inner());
                    let by_pid: std::collections::HashMap<u32, &metrics::grouping::ProcSample> =
                        sampler.processes().iter().map(|p| (p.pid, p)).collect();
                    rules.enforce(&snapshot.apps, &by_pid);
                    rules.annotate(&mut snapshot.apps);
                    rules.muted()
                };
                last_sample = std::time::Instant::now();

                *state.latest.write().unwrap_or_else(|p| p.into_inner()) = Some(snapshot.clone());
                *state.processes.write().unwrap_or_else(|p| p.into_inner()) = sampler.processes().to_vec();
                state.history.lock().unwrap_or_else(|p| p.into_inner()).record(&snapshot);
                {
                    let (day, totals) = sampler.day_totals();
                    state.daily.lock().unwrap_or_else(|p| p.into_inner()).record(day, totals);
                }
                state.app_history.lock().unwrap_or_else(|p| p.into_inner()).record(&snapshot, elapsed);

                for insight in state.watcher.lock().unwrap_or_else(|p| p.into_inner()).evaluate(&snapshot, &settings, &muted) {
                    let _ = app.emit("insight", &insight);
                    if settings.alerts.enabled {
                        let chinese = speaks_chinese(&settings);
                        let separator = if chinese { "：" } else { ": " };
                        let _ = app
                            .notification()
                            .builder()
                            .title(format!("Pulse{separator}{}", insight.app_name))
                            .body(humanize(&insight, chinese))
                            .show();
                    }
                }

                state
                    .tray
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
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

/// Whether notifications should be in Chinese: the language setting, or Windows' own display
/// language when it is left on automatic, which is also what the window follows.
fn speaks_chinese(settings: &settings::Settings) -> bool {
    match settings.language {
        settings::Language::ZhHans => true,
        settings::Language::En => false,
        settings::Language::Auto => {
            const LANG_CHINESE: u16 = 0x04;
            let ui = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
            ui & 0x3ff == LANG_CHINESE
        }
    }
}

/// The notification body. The window already said these things in both languages; the
/// notifications were English only, so a Chinese desktop got English toasts.
fn humanize(insight: &models::Insight, chinese: bool) -> String {
    let mb = |bytes: f64| bytes / 1_048_576.0;
    let (value, seconds) = (insight.value, insight.sustained_seconds);
    match (insight.kind, chinese) {
        (models::InsightKind::HighCpu, false) => format!("has used {}% of the CPU for {seconds}s", value.round()),
        (models::InsightKind::HighCpu, true) => format!("已持续 {seconds} 秒占用 {}% CPU", value.round()),
        (models::InsightKind::GrowingMemory, false) => format!("grew by {:.0} MB and is still climbing", mb(value)),
        (models::InsightKind::GrowingMemory, true) => format!("内存增长了 {:.0} MB，还在上涨", mb(value)),
        (models::InsightKind::HeavyDisk, false) => {
            format!("has moved {:.1} MB/s to disk for {seconds}s", mb(value))
        }
        (models::InsightKind::HeavyDisk, true) => format!("已持续 {seconds} 秒读写磁盘 {:.1} MB/s", mb(value)),
        (models::InsightKind::HeavyNetwork, false) => format!(
            "this machine has been transferring {:.1} MB/s for {seconds}s, and this app holds the most connections",
            mb(value)
        ),
        (models::InsightKind::HeavyNetwork, true) => {
            format!("整机已持续 {seconds} 秒传输 {:.1} MB/s，这个应用的网络连接最多", mb(value))
        }
        (models::InsightKind::Heat, false) => {
            format!("this machine reached {value:.0}°C, and this app is using the most CPU")
        }
        (models::InsightKind::Heat, true) => format!("整机温度达到 {value:.0}°C，此刻这个应用占用 CPU 最多"),
    }
}
