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
    let saved = state.settings.set(next)?;
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

#[tauri::command(async)]
pub fn quit_app(state: State<'_, AppState>, key: String, force: bool) -> QuitResult {
    let Some(snapshot) = state.latest.read().unwrap_or_else(|p| p.into_inner()).clone() else {
        return QuitResult { asked: 0, force_used: force, error: Some("no reading yet".into()) };
    };
    let Some(app) = snapshot.apps.into_iter().find(|a| a.key == key) else {
        return QuitResult { asked: 0, force_used: force, error: Some("that app is gone".into()) };
    };

    // The synthetic row is a count, not an application: its pids are the system processes this
    // user cannot even open, so refusing is better than presenting them as something to kill.
    if app.key == crate::metrics::grouping::UNREADABLE_KEY {
        return QuitResult {
            asked: 0,
            force_used: false,
            error: Some("not an application".into()),
        };
    }

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

/// The report Settings → About copies: machine, settings, recent log. No app names.
#[tauri::command(async)]
pub fn copy_report(state: State<'_, AppState>, app: AppHandle) -> String {
    let snapshot = state.latest.read().unwrap_or_else(|p| p.into_inner()).clone();
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
        other => format!("ShellExecute failed: {other}"),
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
