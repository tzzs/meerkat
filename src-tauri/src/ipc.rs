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
    pub daily: Mutex<crate::daily::Daily>,
    /// Every process of the last tick; the snapshot only carries them grouped into apps.
    pub processes: RwLock<Vec<crate::metrics::grouping::ProcSample>>,
    pub watcher: Mutex<Watcher>,
    pub app_history: Mutex<crate::apphistory::AppHistory>,
    pub rules: Mutex<crate::rules::Rules>,
    /// The last event-log reading, when it was taken and how far back it reached.
    pub events: Mutex<Option<(std::time::Instant, u64, Vec<crate::events::SystemEvent>)>>,
    pub tray: Mutex<TrayManager>,
    pub interface_visible: AtomicBool,
    /// Mirrors `Settings::mcp_enabled` for the pipe server, which checks it on every question.
    pub mcp_enabled: AtomicBool,
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
    state.latest.read().unwrap_or_else(|p| p.into_inner()).clone()
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Settings {
    state.settings.get()
}

#[tauri::command]
pub fn settings_set(state: State<'_, AppState>, app: AppHandle, next: Settings) -> Result<Settings, String> {
    let saved = state.settings.set(next.clone())?;
    {
        let mut tray = state.tray.lock().unwrap_or_else(|p| p.into_inner());
        tray.rebuild(&app, &saved);
    }
    if let Some(snapshot) = state.latest.read().unwrap_or_else(|p| p.into_inner()).as_ref() {
        state.tray.lock().unwrap_or_else(|p| p.into_inner()).update(&app, snapshot, &saved);
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
    state.mcp_enabled.store(saved.mcp_enabled, Ordering::Relaxed);
    if saved.mcp_enabled {
        crate::mcp::start(app.clone());
    }
    let _ = app.emit("settings-changed", ());
    Ok(saved)
}

/// What the WSL virtual machine's memory is made of, asked of Linux when the vmmemWSL row is
/// opened. Off the main thread: `wsl.exe` can take a moment.
#[tauri::command]
pub async fn wsl_memory() -> Option<crate::wsl::WslMemory> {
    tauri::async_runtime::spawn_blocking(crate::wsl::memory).await.ok().flatten()
}

/// The line that adds Pulse to Claude Code, with this copy of Pulse's real path in it.
#[tauri::command]
pub fn mcp_command() -> String {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "pulse.exe".into());
    format!("claude mcp add pulse -- \"{exe}\" --mcp")
}

#[tauri::command]
pub fn history_series(
    state: State<'_, AppState>,
    metric: String,
    range_seconds: u64,
) -> crate::models::HistorySeries {
    let now = crate::history::now_seconds();
    let history = state.history.lock().unwrap_or_else(|p| p.into_inner());
    history.series(&metric, range_seconds, now)
}

#[tauri::command]
pub fn insights(state: State<'_, AppState>) -> Vec<Insight> {
    state.watcher.lock().unwrap_or_else(|p| p.into_inner()).list()
}

#[tauri::command]
pub fn insight_dismiss(state: State<'_, AppState>, id: String) {
    state.watcher.lock().unwrap_or_else(|p| p.into_inner()).dismiss(&id);
}

#[tauri::command]
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
    // Service hosts and the rest of the system are listed now, and Windows refuses to let an
    // ordinary user end them. The row does not offer it; this is the same answer for any caller
    // that asks anyway.
    if !app.can_quit {
        return QuitResult {
            asked: 0,
            force_used: false,
            error: Some("needs administrator rights".into()),
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

/// The processes of one app, one row each, heaviest first. The snapshot carries apps already
/// grouped, which is what every list needs; the detail view is the one place that opens a group.
#[tauri::command]
pub fn app_processes(state: State<'_, AppState>, key: String) -> Vec<crate::models::ProcessRow> {
    let pids: std::collections::HashSet<u32> = match state
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
        return Err("not a window Pulse listed".into());
    }
    let handle = windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void);
    unsafe {
        if !IsWindow(Some(handle)).as_bool() {
            return Err("that window is gone".into());
        }
        if IsIconic(handle).as_bool() {
            let _ = ShowWindow(handle, SW_RESTORE);
        }
        // Pulse is the foreground app at the moment of the click, which is what lets Windows
        // honour this rather than flashing the taskbar button instead.
        if !SetForegroundWindow(handle).as_bool() {
            return Err("Windows kept the current window in front".into());
        }
    }
    Ok(())
}

/// Opens Explorer on the folder holding an executable, with the file selected. The path must be
/// one Pulse itself reported: it ends up on a command line, and only paths that came from the
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
        return Err("not a file Pulse listed".into());
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
/// open are ones Pulse itself worked out from a running process.
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

/// Bytes per local day for the last `days` days, oldest first, zero for days Pulse did not run.
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
pub fn app_ranking(
    state: State<'_, AppState>,
    range_seconds: u64,
    by: crate::apphistory::RankBy,
) -> Vec<crate::apphistory::AppUsage> {
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

/// The processes of one app as the last sample saw them.
fn running_of(state: &AppState, key: &str) -> Vec<crate::metrics::grouping::ProcSample> {
    let pids: std::collections::HashSet<u32> = state
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

/// Pulse and everything it started: its own WebView2 processes are grouped with every other
/// program's WebView2, and pausing that group must not freeze the window doing the pausing.
fn own_tree(state: &AppState) -> std::collections::HashSet<u32> {
    let processes = state.processes.read().unwrap_or_else(|p| p.into_inner());
    let mut tree: std::collections::HashSet<u32> = [std::process::id()].into_iter().collect();
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

#[tauri::command]
pub fn set_tray_bitmap(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    png: Vec<u8>,
) -> Result<(), String> {
    let tray = state.tray.lock().unwrap_or_else(|p| p.into_inner());
    tray.set_bitmap(&app, &id, &png)
}

#[tauri::command]
pub fn tray_ids(state: State<'_, AppState>) -> Vec<String> {
    state.tray.lock().unwrap_or_else(|p| p.into_inner()).ids()
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

#[tauri::command]
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
#[tauri::command]
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

#[tauri::command]
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
