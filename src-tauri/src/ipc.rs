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
    /// Bytes per local day, which is what "today", the last 7 and the last 30 add up.
    pub daily: Mutex<crate::daily::Daily>,
    /// Every process of the last tick; the snapshot only carries them grouped into apps.
    pub processes: RwLock<Vec<crate::metrics::grouping::ProcSample>>,
    /// What each app did, minute by minute (apphistory.rs).
    pub app_history: Mutex<crate::apphistory::AppHistory>,
    /// How each app should run, and which of them are paused.
    pub rules: Mutex<crate::rules::Rules>,
    /// The last event-log reading, when it was taken and how far back it reached.
    pub events: Mutex<Option<(std::time::Instant, u64, Vec<crate::events::SystemEvent>)>>,
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
    /// Whether AI assistants may read Meerkat over MCP (mcp.rs). Off until the user turns it on.
    pub mcp_enabled: std::sync::atomic::AtomicBool,
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

/// Which of Meerkat's two windows somebody can look at right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OnScreen {
    pub main: bool,
    pub panel: bool,
}

/// Who a tick of metrics is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audience {
    Nobody,
    /// One window, by label.
    Only(&'static str),
    /// Both windows. One broadcast, not one send each: a listener registered without a target
    /// hears every `emit_to`, so two sends would deliver the tick twice to it.
    Everyone,
}

impl OnScreen {
    pub fn audience(self) -> Audience {
        match (self.main, self.panel) {
            (false, false) => Audience::Nobody,
            (true, false) => Audience::Only("main"),
            (false, true) => Audience::Only("panel"),
            (true, true) => Audience::Everyone,
        }
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

/// One tick's snapshot to the windows that are on screen, serialised once. `emit_to` with a label
/// reaches the listeners registered for that window; a listener registered without a target
/// hears every window's events, so until the frontend registers with its window's label the
/// hidden window still gets what is sent to the visible one.
pub fn publish_metrics(app: &AppHandle, state: &AppState, snapshot: &Snapshot) {
    let audience = state.on_screen().audience();
    if audience == Audience::Nobody {
        return;
    }
    let Ok(json) = serde_json::to_string(snapshot) else {
        return;
    };
    let _ = match audience {
        Audience::Nobody => return,
        Audience::Only(label) => app.emit_str_to(label, "metrics", json),
        Audience::Everyone => app.emit_str("metrics", json),
    };
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
/// Meerkat whose tray icons are hidden by Windows could not be ended from where the user is.
#[tauri::command]
pub fn quit_meerkat(app: AppHandle) {
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
/// the settings, and the tail of Meerkat's own log. No host name, no user name (paths in the log
/// are scrubbed), no app names.
#[tauri::command(async)]
pub fn copy_report(state: State<'_, AppState>, app: AppHandle) -> String {
    let snapshot = state.latest.read().unwrap_or_else(|p| p.into_inner()).clone();
    let settings = state.settings.get();
    let build = if cfg!(debug_assertions) { "debug build" } else { "release build" };
    let mut lines = Vec::new();
    lines.push(format!("Meerkat {} ({build})", app.package_info().version));
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

/// Restarts Meerkat with administrative rights, which is the only way to reach the ACPI thermal
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

/// The mixer's state. The Sound tab polls this four times a second.
// `_state` is only read by the dev mirror, which a release build does not have.
#[tauri::command(async)]
pub fn audio_state(_state: State<'_, AppState>) -> Result<AudioState, String> {
    let snapshot = snapshot_audio()?;
    #[cfg(debug_assertions)]
    crate::devmirror::audio(&_state);
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

    fn audience(main: bool, panel: bool) -> Audience {
        OnScreen { main, panel }.audience()
    }

    // The old code sent the same 125 KB to both webviews whenever either was shown, and counted a
    // minimised window as shown.
    #[test]
    fn a_tick_goes_only_to_the_windows_somebody_can_see() {
        assert_eq!(audience(false, false), Audience::Nobody);
        assert_eq!(audience(true, false), Audience::Only("main"));
        assert_eq!(audience(false, true), Audience::Only("panel"));
        assert_eq!(audience(true, true), Audience::Everyone, "one broadcast, not two sends");
        assert_eq!(OnScreen::default().audience(), Audience::Nobody);
    }

    #[test]
    fn a_minimised_window_is_not_on_screen() {
        assert!(is_on_screen(true, false));
        assert!(!is_on_screen(true, true), "visible to Windows, but nobody is looking at it");
        assert!(!is_on_screen(false, false));
        assert!(!is_on_screen(false, true));
    }
}
#[tauri::command]
pub async fn wsl_memory() -> Option<crate::wsl::WslMemory> {
    tauri::async_runtime::spawn_blocking(crate::wsl::memory).await.ok().flatten()
}

/// The line that adds this Meerkat to Claude Code, with this copy of Meerkat's real path in it.
#[tauri::command]
pub fn mcp_command() -> String {
    let exe =
        std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "meerkat.exe".into());
    format!("claude mcp add meerkat -- \"{exe}\" --mcp")
}

use std::collections::HashSet;

/// The processes of one app as the last sample saw them.
fn running_of(state: &AppState, key: &str) -> Vec<crate::metrics::grouping::ProcSample> {
    let pids: HashSet<u32> = state
        .latest
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|snapshot| snapshot.apps.iter().find(|a| a.key == key).map(|a| a.pids.iter().copied().collect()))
        .unwrap_or_default();
    state
        .processes
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|p| pids.contains(&p.pid))
        .cloned()
        .collect()
}

/// Meerkat and everything it started: its own WebView2 processes are grouped with every other
/// program's WebView2, and pausing that group must not freeze the window doing the pausing.
fn own_tree(state: &AppState) -> HashSet<u32> {
    let processes = state.processes.read().unwrap_or_else(|p| p.into_inner());
    let mut tree: HashSet<u32> = [std::process::id()].into_iter().collect();
    loop {
        let before = tree.len();
        for process in processes.iter() {
            if process.ppid.is_some_and(|parent| tree.contains(&parent)) {
                tree.insert(process.pid);
            }
        }
        if tree.len() == before {
            return tree;
        }
    }
}

/// The processes of one app, one row each, heaviest first. The snapshot carries apps already
/// grouped, which is what every list needs; the detail view is the one place that opens a group.
#[tauri::command]
pub fn app_processes(state: State<'_, AppState>, key: String) -> Vec<crate::models::ProcessRow> {
    let pids: HashSet<u32> = match state
        .latest
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|snapshot| snapshot.apps.iter().find(|a| a.key == key).map(|a| a.pids.iter().copied().collect()))
    {
        Some(pids) => pids,
        None => return Vec::new(),
    };
    let processes = state.processes.read().unwrap_or_else(|p| p.into_inner());
    let mut rows: Vec<crate::models::ProcessRow> =
        processes.iter().filter(|p| pids.contains(&p.pid)).map(|p| p.row()).collect();
    rows.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
    rows
}

/// Brings one of an app's windows to the front, restoring it if it was minimised. Only windows
/// the last sample listed are accepted, so the page cannot be used to poke at arbitrary handles.
#[tauri::command]
pub fn focus_window(state: State<'_, AppState>, hwnd: isize) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE};
    let known = state
        .latest
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .is_some_and(|snapshot| snapshot.apps.iter().any(|a| a.windows.iter().any(|w| w.hwnd == hwnd)));
    if !known {
        return Err("not a window Meerkat listed".into());
    }
    let handle = windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        if !IsWindow(Some(handle)).as_bool() {
            return Err("that window is gone".into());
        }
        if IsIconic(handle).as_bool() {
            let _ = ShowWindow(handle, SW_RESTORE);
        }
        // Meerkat is the foreground app at the moment of the click, which is what lets Windows
        // honour this rather than flashing the taskbar button instead.
        if !SetForegroundWindow(handle).as_bool() {
            return Err("Windows kept the current window in front".into());
        }
    }
    Ok(())
}

/// Opens Explorer on the folder holding an executable, with the file selected. The path must be
/// one Meerkat itself reported: it ends up on a command line, and only paths that came from the
/// process table are known to be plain file paths.
#[tauri::command]
pub fn reveal_in_explorer(state: State<'_, AppState>, path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let known = state
        .processes
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .any(|p| !p.exe.is_empty() && p.exe.eq_ignore_ascii_case(&path));
    if !known || path.contains('"') || !std::path::Path::new(&path).is_file() {
        return Err("not a file Meerkat listed".into());
    }
    std::process::Command::new("explorer.exe")
        // Explorer reads `/select,"C:\a b\c.exe"` itself; Rust's own quoting of the whole
        // argument would hand it a string it does not parse.
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens an app's project folder in Explorer. Looked up by app key, so the only folders this can
/// open are ones Meerkat itself worked out from a running process.
#[tauri::command]
pub fn open_project(state: State<'_, AppState>, key: String) -> Result<(), String> {
    let folder = state
        .latest
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|snapshot| snapshot.apps.iter().find(|a| a.key == key).and_then(|a| a.project.clone()))
        .ok_or("no project for that app")?;
    if !std::path::Path::new(&folder).is_dir() {
        return Err("the folder is gone".into());
    }
    std::process::Command::new("explorer.exe").arg(&folder).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Bytes per local day for the last `days` days, oldest first, zero for days Meerkat did not run.
#[tauri::command]
pub fn daily_totals(state: State<'_, AppState>, days: u32) -> Vec<crate::models::DailyTotal> {
    let today = crate::metrics::local_day(crate::history::now_seconds() * 1000);
    state
        .daily
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .recent(today, days.clamp(1, 120), crate::metrics::local_day_start_ms)
}

/// The heaviest apps over the last `range_seconds`, by one measure.
#[tauri::command]
pub fn app_ranking(state: State<'_, AppState>, range_seconds: u64, by: crate::apphistory::RankBy) -> Vec<crate::apphistory::AppUsage> {
    let now = crate::history::now_seconds();
    let history = state.app_history.lock().unwrap_or_else(|p| p.into_inner());
    history.ranking(now.saturating_sub(range_seconds), now + 60, by, 25)
}

/// The heaviest apps since local midnight by one measure: "who wrote the most today".
#[tauri::command]
pub fn app_ranking_today(state: State<'_, AppState>, by: crate::apphistory::RankBy) -> Vec<crate::apphistory::AppUsage> {
    let now = crate::history::now_seconds();
    let today = crate::metrics::local_day(now * 1000);
    let midnight = crate::metrics::local_day_start_ms(today) / 1000;
    let history = state.app_history.lock().unwrap_or_else(|p| p.into_inner());
    history.ranking(midnight, now + 60, by, 8)
}

/// What was running at one moment (Unix milliseconds), busiest first.
#[tauri::command]
pub fn apps_at(state: State<'_, AppState>, at_ms: u64) -> Vec<crate::apphistory::AppUsage> {
    let history = state.app_history.lock().unwrap_or_else(|p| p.into_inner());
    history.at(at_ms / 1000, 12)
}

/// One app's history over the last `range_seconds`, in steps that keep a chart to a few hundred
/// points whatever the range.
#[tauri::command]
pub fn app_series(state: State<'_, AppState>, key: String, range_seconds: u64) -> Vec<crate::apphistory::AppPoint> {
    let now = crate::history::now_seconds();
    let step = match range_seconds {
        0..=21_600 => 60,
        21_601..=86_400 => 300,
        86_401..=604_800 => 3_600,
        _ => 10_800,
    };
    let history = state.app_history.lock().unwrap_or_else(|p| p.into_inner());
    history.series(&key, now.saturating_sub(range_seconds), now + 60, step)
}

/// The event-log entries that explain the timeline, newest first. Reading the log takes a
/// moment, so it happens off the main thread and is kept for a minute.
#[tauri::command]
pub async fn system_events(app: AppHandle, range_seconds: u64) -> Vec<crate::events::SystemEvent> {
    let state = app.state::<AppState>();
    if let Some((at, range, events)) = state.events.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        if *range >= range_seconds && at.elapsed().as_secs() < 60 {
            let from = crate::history::now_seconds().saturating_sub(range_seconds) * 1000;
            return events.iter().filter(|e| e.at_ms >= from).cloned().collect();
        }
    }
    let read = tauri::async_runtime::spawn_blocking(move || crate::events::recent(range_seconds * 1000, 2_000))
        .await
        .unwrap_or_default();
    *state.events.lock().unwrap_or_else(|p| p.into_inner()) = Some((std::time::Instant::now(), range_seconds, read.clone()));
    read
}

#[tauri::command]
pub fn rules_list(state: State<'_, AppState>) -> Vec<crate::rules::AppRule> {
    state.rules.lock().unwrap_or_else(|p| p.into_inner()).list()
}

/// Stores an app's rule and applies it to what is running now; from then on every process the
/// app starts gets it too.
#[tauri::command]
pub fn rule_set(state: State<'_, AppState>, rule: crate::rules::AppRule) -> Result<(), String> {
    let running = running_of(&state, &rule.key);
    let refs: Vec<&crate::metrics::grouping::ProcSample> = running.iter().collect();
    state.rules.lock().unwrap_or_else(|p| p.into_inner()).set(rule, &refs)
}

/// Pauses or resumes every process of an app this user may control. Returns how many changed.
#[tauri::command]
pub fn app_pause(state: State<'_, AppState>, key: String, pause: bool) -> Result<usize, String> {
    let running = running_of(&state, &key);
    let refs: Vec<&crate::metrics::grouping::ProcSample> = running.iter().collect();
    let protected = own_tree(&state);
    let changed = state.rules.lock().unwrap_or_else(|p| p.into_inner()).pause(&key, pause, &refs, &protected)?;
    // The row's "paused" mark comes from the next sample; patch the current one so the button
    // does not flip back for a second.
    if let Some(snapshot) = state.latest.write().unwrap_or_else(|p| p.into_inner()).as_mut() {
        if let Some(app) = snapshot.apps.iter_mut().find(|a| a.key == key) {
            app.control.paused = pause && changed > 0;
        }
    }
    Ok(changed)
}
