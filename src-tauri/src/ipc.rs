//! IPC surface. Everything the UI can ask for, and the shared state the sampler thread
//! publishes into.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::alerts::Watcher;
use crate::history::History;
use crate::menu::{self, PanelAnchor};
use crate::models::{Insight, QuitResult, Snapshot};
use crate::settings::{Settings, SettingsStore};
use crate::tray::TrayManager;

pub struct AppState {
    pub latest: RwLock<Option<Snapshot>>,
    pub settings: SettingsStore,
    /// Held for the whole of a `settings_set`, so two saves cannot interleave their "was it
    /// changed" comparison, the tray rebuild and the event. Taken on worker threads only: it is
    /// held while waiting for the main thread, which therefore must never ask for it.
    pub settings_saving: Mutex<()>,
    pub history: Mutex<History>,
    pub watcher: Mutex<Watcher>,
    pub tray: Mutex<TrayManager>,
    /// Whether anyone can look at the window: shown, and not minimised. Kept per window so the
    /// sampler sends a tick only where it can be seen; `interface_visible` is "either".
    pub main_visible: AtomicBool,
    pub panel_visible: AtomicBool,
    pub interface_visible: AtomicBool,
    pub panel_pinned: AtomicBool,
    /// What the panel was last opened against (the icon and the work area around it), so a
    /// resize while it is open can keep the edge beside the icon attached.
    pub panel_anchor: Mutex<Option<PanelAnchor>>,
    /// The last size, in logical pixels, the panel's webview asked for.
    pub panel_size: Mutex<Option<(u32, u32)>>,
    /// `menu::now_ms()` when the panel was last hidden by losing focus; 0 = never.
    pub panel_blurred_at: AtomicU64,
    pub data_dir: PathBuf,
    pub app: OnceLock<AppHandle>,
}

impl AppState {
    /// Something is on screen: the sampler ticks at its normal pace and sends full snapshots.
    pub fn visible(&self) -> bool {
        self.interface_visible.load(Ordering::Relaxed)
    }

    pub fn on_screen(&self) -> OnScreen {
        OnScreen {
            main: self.main_visible.load(Ordering::Relaxed),
            panel: self.panel_visible.load(Ordering::Relaxed),
        }
    }
}

/// Which of Pulse's two windows somebody can look at right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OnScreen {
    pub main: bool,
    pub panel: bool,
}

impl OnScreen {
    pub fn any(self) -> bool {
        self.main || self.panel
    }

    /// The labels of the windows that should receive this tick's metrics.
    pub fn labels(self) -> impl Iterator<Item = &'static str> {
        [(self.main, "main"), (self.panel, "panel")]
            .into_iter()
            .filter_map(|(shown, label)| shown.then_some(label))
    }
}

/// A window somebody can see: shown, and not minimised. A minimised window is still "visible" as
/// far as Windows is concerned, and used to be sent a full snapshot a second for nobody.
pub fn is_on_screen(shown: bool, minimised: bool) -> bool {
    shown && !minimised
}

/// The sampler only pushes full snapshots while something is on screen, so every path that shows,
/// hides, minimises or restores a window has to run this. It also says "window-shown" to the main
/// window the moment it comes back, wherever that came from (the tray, a second launch, the
/// taskbar button): the frontend refetches the snapshot and settings, which it could not have been
/// kept current on while nothing was sent.
pub fn refresh_visibility(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let look = |label: &str| {
        app.get_webview_window(label).is_some_and(|window| {
            is_on_screen(window.is_visible().unwrap_or(false), window.is_minimized().unwrap_or(false))
        })
    };
    let (main, panel) = (look("main"), look("panel"));
    let main_was = state.main_visible.swap(main, Ordering::Relaxed);
    state.panel_visible.store(panel, Ordering::Relaxed);
    state.interface_visible.store(main || panel, Ordering::Relaxed);
    if main && !main_was {
        let _ = app.emit_to("main", "window-shown", ());
    }
}

/// One tick's snapshot to the windows that are on screen, serialised once however many there
/// are. `emit_to` with a label reaches the listeners registered for that window; a listener
/// registered without a target hears every window's events (see `listenEvent` in the frontend).
pub fn publish_metrics(app: &AppHandle, state: &AppState, snapshot: &Snapshot) {
    let on_screen = state.on_screen();
    if !on_screen.any() {
        return;
    }
    let Ok(json) = serde_json::to_string(snapshot) else {
        return;
    };
    for label in on_screen.labels() {
        let _ = app.emit_str_to(label, "metrics", json.clone());
    }
}

// A plain `fn` command runs on the main thread, which is also the one that paints and takes input:
// anything that locks, reads a file or talks to COM goes through `command(async)`, which hands it
// to the runtime's workers instead. Only the light window controls below stay on the main thread.
#[tauri::command(async)]
pub fn snapshot(state: State<'_, AppState>) -> Option<Snapshot> {
    state.latest.read().unwrap_or_else(|p| p.into_inner()).clone()
}

#[tauri::command(async)]
pub fn settings_get(state: State<'_, AppState>) -> Settings {
    state.settings.get()
}

/// Saves, applies, announces, in that order: the file first, then the tray (which is rebuilt
/// from what the store holds and has repainted when this goes on), and only then the event, so a
/// window that reacts to it by reading the settings or the tray finds the new state.
#[tauri::command(async)]
pub fn settings_set(state: State<'_, AppState>, app: AppHandle, next: Settings) -> Result<Settings, String> {
    let _one_at_a_time = state.settings_saving.lock().unwrap_or_else(|p| p.into_inner());
    let previous = state.settings.get();
    let saved = state.settings.set(next).inspect_err(|error| {
        crate::diag::error(format!("settings: could not be saved: {error}"));
    })?;
    // Waits for the main thread, which does the icons. The store's own lock is long released;
    // `settings_saving` is held, and the main thread never takes it.
    crate::tray::rebuild(&app);
    // Only a change is acted on: every other save used to re-register the login item, which
    // is how a debug build ended up starting at sign-in.
    if previous.start_at_login != saved.start_at_login {
        crate::autostart::apply(&app, saved.start_at_login);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_skip_taskbar(!saved.show_in_taskbar);
    }
    let _ = app.emit("settings-changed", ());
    Ok(saved)
}

#[tauri::command(async)]
pub fn history_series(
    state: State<'_, AppState>,
    metric: String,
    range_seconds: u64,
) -> crate::models::HistorySeries {
    let now = crate::history::now_seconds();
    let history = state.history.lock().unwrap_or_else(|p| p.into_inner());
    history.series(&metric, range_seconds, now)
}

#[tauri::command(async)]
pub fn history_ranking(state: State<'_, AppState>, hours: u64) -> Vec<crate::models::RankingRow> {
    let now = crate::history::now_seconds();
    let history = state.history.lock().unwrap_or_else(|p| p.into_inner());
    history.ranking(hours, now)
}

#[tauri::command(async)]
pub fn insights(state: State<'_, AppState>) -> Vec<Insight> {
    state.watcher.lock().unwrap_or_else(|p| p.into_inner()).list()
}

#[tauri::command(async)]
pub fn insight_dismiss(state: State<'_, AppState>, id: String) {
    state.watcher.lock().unwrap_or_else(|p| p.into_inner()).dismiss(&id);
}

/// Closes an application's windows, or ends its processes. What may be touched, and how a process
/// is recognised again before it is ended, is `quit`'s business; this only looks the row up and
/// answers with a code (`QuitError::code`) the frontend turns into words.
#[tauri::command(async)]
pub fn quit_app(state: State<'_, AppState>, key: String, force: bool) -> QuitResult {
    use crate::quit::{self, QuitError};

    // One row is copied out of the snapshot, not the snapshot.
    let (taken_at_ms, app) = {
        let latest = state.latest.read().unwrap_or_else(|p| p.into_inner());
        let Some(snapshot) = latest.as_ref() else {
            return QuitError::NoReading.refusal(force);
        };
        (snapshot.taken_at_ms, snapshot.apps.iter().find(|a| a.key == key).cloned())
    };
    let Some(app) = app else {
        return QuitError::Gone.refusal(force);
    };

    let family = quit::own_family();
    let result = match quit::quit_policy(&app, &family).permits(force) {
        Err(why) => why.refusal(force),
        Ok(()) if force => quit::force_quit(&app, taken_at_ms, &family),
        Ok(()) => quit::ask_to_close(&app, taken_at_ms),
    };
    // Counts and a code, never the application's name: this line ends up in a report.
    crate::diag::info(format!(
        "quit: {} asked {} of {} processes, {}",
        if force { "force" } else { "graceful" },
        result.asked,
        app.process_count,
        result.error.as_deref().unwrap_or("ok"),
    ));
    result
}

/// A clean exit for the window's own Quit: closing the window only hides it, so without this a
/// Pulse whose tray icons are hidden by Windows could not be ended from where the user is.
#[tauri::command]
pub fn quit_pulse(app: AppHandle) {
    app.exit(0);
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

/// The webview reports its content size on every update; most of those are the size it already
/// has, and those never reach the window system.
#[tauri::command]
pub fn set_panel_size(app: AppHandle, state: State<'_, AppState>, width: u32, height: u32) -> Result<(), String> {
    let wanted = (width.max(1), height.max(1));
    {
        let mut last = state.panel_size.lock().unwrap_or_else(|p| p.into_inner());
        if *last == Some(wanted) {
            return Ok(());
        }
        *last = Some(wanted);
    }
    menu::resize_panel(&app, &state, wanted.0, wanted.1)
}

/// How many of the log's last lines the report carries.
const REPORT_LOG_LINES: usize = 40;

/// The report Settings → About copies: version, the machine's OS build and CPU, a few figures,
/// the settings, and the tail of Pulse's own log. No host name, no user name (paths in the log
/// are scrubbed), no app names.
#[tauri::command(async)]
pub fn copy_report(state: State<'_, AppState>, app: AppHandle) -> String {
    let snapshot = state.latest.read().unwrap_or_else(|p| p.into_inner()).clone();
    let settings = state.settings.get();
    let build = if cfg!(debug_assertions) { "debug build" } else { "release build" };
    let mut lines = Vec::new();
    lines.push(format!("Pulse {} ({build})", app.package_info().version));
    if let Some(snapshot) = snapshot.as_ref() {
        lines.push(format!(
            "{} | {} | {} logical cores | {} bytes RAM",
            snapshot.machine.os_version,
            snapshot.machine.chip_name,
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
    lines.push(format!("--- last {REPORT_LOG_LINES} log lines"));
    let log = crate::diag::tail_for_report(REPORT_LOG_LINES);
    if log.is_empty() {
        lines.push("(the log is empty)".to_string());
    }
    lines.extend(log);
    lines.join("
")
}

/// A line from a window's own error handling, so a failure in the interface is in the same log
/// (and the same report) as the backend's. The level is "error", "warn" or "info"; the message is
/// cut at 2 KiB and at most 20 lines per 10 seconds are kept.
#[tauri::command(async)]
pub fn log_frontend(level: String, message: String) -> Result<(), String> {
    crate::diag::from_frontend(&level, &message)
}

#[tauri::command]
pub fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioState {
    output_endpoints: Vec<crate::audio::Endpoint>,
    input_endpoints: Vec<crate::audio::Endpoint>,
    render: Vec<crate::audio::AppSession>,
    capture: Vec<crate::audio::AppSession>,
}

/// Read the whole mixer in one pass. Also called from the sampler thread in debug builds,
/// which is what proves the COM initialisation works off the main thread.
pub fn snapshot_audio() -> Result<AudioState, String> {
    crate::audio::init_com()?;
    Ok(AudioState {
        output_endpoints: crate::audio::endpoints_of(crate::audio::Role::Render).unwrap_or_default(),
        input_endpoints: crate::audio::endpoints_of(crate::audio::Role::Capture).unwrap_or_default(),
        render: crate::audio::sessions(crate::audio::Role::Render).unwrap_or_default(),
        capture: crate::audio::sessions(crate::audio::Role::Capture).unwrap_or_default(),
    })
}

/// One application's real icon, as a data URL the row can drop straight into an `<img>`.
///
/// Async because extraction makes the shell load the executable's module, which costs tens
/// of milliseconds the first time each file is touched; a synchronous command would pay that
/// on the thread that paints the window. The work goes to `spawn_blocking` so even the async
/// runtime's workers stay free for the sampler.
#[tauri::command]
pub async fn app_icon(exe_path: String, size: i32) -> Option<String> {
    let image = tauri::async_runtime::spawn_blocking(move || crate::icons::icon_for(&exe_path, size))
        .await
        .ok()??;
    png_data_url(&image)
}

/// Restarts Pulse with administrative rights, which is the only way to reach the ACPI thermal
/// zone on Windows. The consent dialog is the user's decision: a declined one is reported and
/// this instance keeps running, because quitting on their "No" would be worse than the missing
/// sensor. Only after the elevated child is actually launched does this one exit, so the tray
/// is never left without a process behind it.
#[tauri::command]
pub fn relaunch_elevated(app: AppHandle) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let verb = wide("runas");
    let path = wide(&exe.to_string_lossy());
    let launched = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports failure as a value at or below 32 rather than an error handle.
    let code = launched.0 as isize;
    if code > 32 {
        app.exit(0);
        return Ok(());
    }
    Err(match code {
        // The user pressed No on the consent dialog, or it was blocked by policy.
        5 | 1223 => "cancelled".to_string(),
        other => {
            crate::diag::warn(format!("relaunch as administrator: ShellExecute failed with {other}"));
            format!("ShellExecute failed: {other}")
        }
    })
}

fn png_data_url(image: &crate::icons::IconImage) -> Option<String> {
    use base64::Engine;
    let rgba = image::RgbaImage::from_raw(image.width, image.height, image.rgba.clone())?;
    let mut cursor = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(cursor.into_inner());
    Some(format!("data:image/png;base64,{encoded}"))
}

#[tauri::command(async)]
pub fn audio_state(state: State<'_, AppState>) -> Result<AudioState, String> {
    let snapshot = snapshot_audio()?;
    #[cfg(debug_assertions)]
    if let Ok(raw) = serde_json::to_string(&snapshot) {
        let dir = state.data_dir.join("dev");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("audio.json"), raw);
    }
    Ok(snapshot)
}

/// Windows restores a same-named app's volume within a login session on its own, so the
/// saved rules mostly earn their keep across a reboot or an audio-service restart.
#[tauri::command(async)]
pub fn audio_set_volume(
    state: State<'_, AppState>,
    role: String,
    id: u32,
    pid: u32,
    volume: f32,
    muted: bool,
) -> Result<(), String> {
    crate::audio::init_com()?;
    let role = parse_role(&role)?;
    crate::audio::set_session_volume(role, id, volume)?;
    crate::audio::set_session_mute(role, id, muted)?;
    if let Some(exe) = crate::audio::exe_name(pid) {
        let endpoint = crate::audio::endpoints_of(role)
            .ok()
            .and_then(|list| list.into_iter().find(|e| e.is_default).map(|e| e.id))
            .unwrap_or_default();
        crate::audio::remember_volume(&state.data_dir.join("audio-volumes.json"), &exe, &endpoint, volume, muted)?;
    }
    Ok(())
}

#[tauri::command(async)]
pub fn audio_set_endpoint(endpoint_id: String, volume: f32, muted: bool) -> Result<(), String> {
    crate::audio::init_com()?;
    crate::audio::set_endpoint_volume(&endpoint_id, volume)?;
    crate::audio::set_endpoint_mute(&endpoint_id, muted)
}

fn parse_role(role: &str) -> Result<crate::audio::Role, String> {
    match role {
        "render" => Ok(crate::audio::Role::Render),
        "capture" => Ok(crate::audio::Role::Capture),
        other => Err(format!("unknown audio role {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(main: bool, panel: bool) -> Vec<&'static str> {
        OnScreen { main, panel }.labels().collect()
    }

    // The old code sent the same 125 KB to both webviews whenever either was shown, and counted a
    // minimised window as shown.
    #[test]
    fn a_tick_goes_only_to_the_windows_somebody_can_see() {
        assert!(labels(false, false).is_empty());
        assert_eq!(labels(true, false), ["main"]);
        assert_eq!(labels(false, true), ["panel"]);
        assert_eq!(labels(true, true), ["main", "panel"]);
        assert!(!OnScreen::default().any());
        assert!(OnScreen { main: false, panel: true }.any());
    }

    #[test]
    fn a_minimised_window_is_not_on_screen() {
        assert!(is_on_screen(true, false));
        assert!(!is_on_screen(true, true), "visible to Windows, but nobody is looking at it");
        assert!(!is_on_screen(false, false));
        assert!(!is_on_screen(false, true));
    }
}
