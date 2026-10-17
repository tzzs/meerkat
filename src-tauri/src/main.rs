#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod apphistory;
mod audio;
mod control;
mod daily;
#[cfg(debug_assertions)]
mod devmirror;
mod diag;
mod events;
mod history;
mod i18n;
mod icons;
mod ipc;
mod mcp;
mod menu;
mod metrics;
mod models;
mod nvml;
mod quit;
mod rules;
mod settings;
mod toast;
mod tray;
mod tray_render;
mod wsl;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use crate::alerts::Watcher;
use crate::daily::Daily;
use crate::history::History;
use crate::ipc::AppState;
use crate::metrics::Sampler;
use crate::settings::SettingsStore;
use crate::tray::TrayManager;

fn main() -> tauri::Result<()> {
    // Before anything that can fail: this is a build with no console, and a Meerkat that does not
    // start (no WebView2 runtime is the usual reason) has nowhere else to say why.
    diag::init_early(toast::AUMID);
    // Started by an AI assistant as its MCP server: answer on stdio by asking the running
    // Meerkat, and never open a window or a tray icon of its own. Before the builder, because
    // that path has no app to attach to.
    if std::env::args().any(|arg| arg == "--mcp") {
        mcp::run_stdio();
        return Ok(());
    }
    let result = run();
    if let Err(error) = &result {
        diag::error(format!("Meerkat stopped: {error}"));
    }
    result
}

fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        // First on purpose: plugins set up in registration order, and a second launch exits
        // inside this one's setup, before any other plugin, window, tray icon or sampler of its
        // own exists. The running instance gets the arguments instead.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A login item firing while Meerkat already runs has nothing to show.
            if !args.iter().any(|a| a == autostart::FLAG) {
                diag::info("a second launch: showing the window of the running instance");
                menu::show_main(app, None);
            }
        }))
        // The flag is what keeps a sign-in start in the tray; without it the login item opened
        // the window like a manual launch.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![autostart::FLAG]),
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
            ipc::quit_meerkat,
            ipc::log_frontend,
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
        // One listener for every tray icon, however often the icons are rebuilt: a handler set
        // on each icon builder was registered again with every rebuild.
        .on_menu_event(menu::on_menu_event)
        .on_tray_icon_event(menu::on_tray_event)
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                // Closing the window keeps Meerkat in the tray, like the Mac version.
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
                ipc::refresh_visibility(window.app_handle());
            }
            WindowEvent::Focused(false) => {
                if window.label() == "panel" && !panel_pinned(window) {
                    // A press on the tray icon lands here first and its release afterwards;
                    // the stamp is how that release knows it was a click to close.
                    if window.is_visible().unwrap_or(false) {
                        menu::note_panel_blur(window.app_handle());
                    }
                    let _ = window.hide();
                }
                ipc::refresh_visibility(window.app_handle());
            }
            // Minimising and restoring arrive as a resize (to nothing and back), and a window
            // that is minimised is still "visible" to Windows; this is how the sampler learns
            // that nobody is looking, and that somebody is again.
            WindowEvent::Resized(_) | WindowEvent::Focused(true) => {
                ipc::refresh_visibility(window.app_handle());
            }
            _ => {}
        })
        .setup(|app| {
            let app_data = data_dir(app)?;
            std::fs::create_dir_all(&app_data).ok();
            // Before the settings and the history are read: what they have to say about a damaged
            // file is exactly what a report should carry.
            diag::init(&app_data);
            diag::info(format!(
                "Meerkat {} starting ({}, {})",
                app.package_info().version,
                if cfg!(debug_assertions) { "debug build" } else { "release build" },
                if unsafe { windows::Win32::UI::Shell::IsUserAnAdmin() }.as_bool() {
                    "elevated"
                } else {
                    "not elevated"
                },
            ));
            toast::register(&app_data);
            let settings = SettingsStore::open(&app_data);
            let initial = settings.get();
            app.manage(AppState {
                latest: RwLock::new(None),
                settings,
                settings_saving: Mutex::new(()),
                history: Mutex::new(History::open(&app_data)),
                daily: Mutex::new(Daily::open(&app_data)),
                processes: RwLock::new(Vec::new()),
                watcher: Mutex::new(Watcher::open(&app_data)),
                app_history: Mutex::new(apphistory::AppHistory::open(&app_data)),
                rules: Mutex::new(rules::Rules::open(&app_data)),
                events: Mutex::new(None),
                tray: Mutex::new(TrayManager::new()),
                main_visible: AtomicBool::new(false),
                panel_visible: AtomicBool::new(false),
                interface_visible: AtomicBool::new(false),
                mcp_enabled: AtomicBool::new(initial.mcp_enabled),
                panel_pinned: AtomicBool::new(false),
                panel_anchor: Mutex::new(None),
                panel_size: Mutex::new(None),
                panel_blurred_at: AtomicU64::new(0),
                data_dir: app_data,
                app: OnceLock::new(),
            });

            let handle = app.handle().clone();
            tray::rebuild(&handle);

            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.set_skip_taskbar(!initial.show_in_taskbar);
            }

            autostart::reconcile(&handle, initial.start_at_login);

            spawn_sampler(handle.clone());
            if initial.mcp_enabled {
                mcp::start(handle.clone());
            }

            // Restore remembered app volumes before anything starts playing sound.
            if audio::init_com().is_ok() {
                let dir = handle.state::<AppState>().data_dir.clone();
                match audio::apply_saved_volumes(&dir.join("audio-volumes.json")) {
                    Ok(0) => {}
                    Ok(count) => crate::diag::info(format!("restored {count} saved app volumes")),
                    Err(error) => crate::diag::warn(format!("restore saved volumes: {error}")),
                }
            }

            // Opened by hand it shows its window; started at sign-in it stays in the tray.
            let at_startup = std::env::args().any(|a| a == autostart::FLAG);
            if at_startup {
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.hide();
                }
            } else {
                menu::show_main(&handle, None);
            }
            Ok(())
        })
        .build(tauri::generate_context!())?
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                on_exit(app);
            }
        });
    Ok(())
}

/// The last thing that runs, on the main thread, once the event loop has stopped: every
/// clean way out (the tray menu, `quit_meerkat`, a relaunch) passes through here, because each of
/// them is `AppHandle::exit`, which ends the event loop and then reports `RunEvent::Exit`.
///
/// The history is written out here. It is otherwise saved on a timer, so without this the minute
/// and the hour in progress, and everything since the last save, went with the process.
fn on_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        // A poisoned lock is still the data: losing the history because a thread panicked
        // while it held it would be the worse outcome.
        let mut history = state.history.lock().unwrap_or_else(|p| p.into_inner());
        history.flush();
        drop(history);
        state.app_history.lock().unwrap_or_else(|p| p.into_inner()).flush();
        // A paused app must not outlive the program that paused it. A crash skips this; the
        // list on disk catches that case at the next start (rules.rs).
        state.rules.lock().unwrap_or_else(|p| p.into_inner()).resume_all();
    }
    diag::info("exit");
}

/// Where settings, history and alerts live. A debug build can be pointed elsewhere with
/// `MEERKAT_DATA_DIR`, so a test instance can run beside the one in daily use without writing
/// over its settings or history. Nothing else can redirect it: Windows answers the known-folder
/// lookup from the user's token, not from APPDATA.
///
/// The first launch after the old build (which was called Pulse, and kept its files under
/// `com.tzz.pulse`) adopts what that one left behind. The folder is part of the app's identity,
/// so a rename alone would have started a user with default settings, an empty history and no
/// remembered volumes, and nothing on screen to say where they went.
fn data_dir(app: &tauri::App) -> tauri::Result<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("MEERKAT_DATA_DIR") {
        return Ok(std::path::PathBuf::from(dir));
    }
    let dir = app.path().app_data_dir()?;
    adopt_former_data(&dir);
    Ok(dir)
}

/// The identifier the app had before it was renamed.
const FORMER_IDENTIFIER: &str = "com.tzz.pulse";

/// Copies what a build under the former identifier left beside this one's folder, for any file
/// this one does not have yet. It never overwrites: a settings file written by this build is
/// newer and truer than one adopted, and an install that was started, stopped and restarted must
/// not have its own choices quietly replaced on the third start.
///
/// The former folder is left where it is. Deleting someone's data because a program that used to
/// own it was renamed is not this program's call to make.
fn adopt_former_data(dir: &std::path::Path) {
    let Some(parent) = dir.parent() else { return };
    let former = parent.join(FORMER_IDENTIFIER);
    if former == dir || !former.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&former) else { return };
    let mut adopted = Vec::new();
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dir.join(entry.file_name());
        if !from.is_file() || to.exists() {
            continue;
        }
        if std::fs::copy(&from, &to).is_ok() {
            adopted.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    if !adopted.is_empty() {
        crate::diag::info(format!("adopted from the previous install: {}", adopted.join(", ")));
    }
}

fn panel_pinned(window: &tauri::Window) -> bool {
    window
        .try_state::<AppState>()
        .map(|state| state.panel_pinned.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn spawn_sampler(app: AppHandle) {
    std::thread::Builder::new()
        .name("meerkat-sampler".into())
        .spawn(move || {
            let state = app.state::<AppState>();
            let _ = state.app.set(app.clone());
            let toaster = toast::Toaster::start(app.clone());
            // WinRT's battery report is read on this thread, and WinRT needs COM.
            if let Err(error) = audio::init_com() {
                diag::warn(format!("sampler COM: {error}"));
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
                let mut snapshot = sampler.sample();
                let elapsed = last_sample.elapsed().as_secs_f64();
                last_sample = std::time::Instant::now();
                // Read after the sampling, which takes a while, so a setting that was saved in
                // the meantime already counts for this tick.
                let settings = state.settings.get();
                // The rules remember how each app should run and apply it to every process the
                // app starts; `muted` is what the watcher should not raise alerts about.
                let muted = {
                    let mut rules = state.rules.lock().unwrap_or_else(|p| p.into_inner());
                    let by_pid: std::collections::HashMap<u32, &metrics::grouping::ProcSample> =
                        sampler.processes().iter().map(|p| (p.pid, p)).collect();
                    rules.enforce(&snapshot.apps, &by_pid);
                    rules.annotate(&mut snapshot.apps);
                    rules.muted()
                };

                *state.latest.write().unwrap_or_else(|p| p.into_inner()) = Some(snapshot.clone());
                *state.processes.write().unwrap_or_else(|p| p.into_inner()) = sampler.processes().to_vec();
                state.history.lock().unwrap_or_else(|p| p.into_inner()).record(&snapshot);
                {
                    let (day, totals) = sampler.day_totals();
                    state.daily.lock().unwrap_or_else(|p| p.into_inner()).record(day, totals);
                }
                state.app_history.lock().unwrap_or_else(|p| p.into_inner()).record(&snapshot, elapsed);

                // The watcher's lock is released before anything is emitted or shown.
                let insights = state
                    .watcher
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .evaluate_muted(&snapshot, &settings, &muted);
                for insight in insights {
                    let _ = app.emit("insight", &insight);
                    if settings.alerts.enabled {
                        let lang = i18n::resolve(settings.language);
                        toaster.show(
                            i18n::insight_title(&insight),
                            i18n::insight_body(&insight, lang, i18n::Units::of(&settings)),
                        );
                    }
                }

                // Posted to the main thread, never waited for: the sampler holds no lock here
                // and does not stand in the shell's queue.
                tray::update(&app, &snapshot, &settings);

                ipc::publish_metrics(&app, &state, &snapshot);
                #[cfg(debug_assertions)]
                devmirror::tick(&state, &snapshot);

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

/// Start at login. The registration itself is the `Run` value the autostart plugin writes
/// (`HKCU\…\Run\Meerkat = <exe> --autostart`); this module reads it back so Meerkat can tell whose
/// registration it is before changing it.
mod autostart {
    use tauri::AppHandle;
    #[cfg(not(debug_assertions))]
    use tauri_plugin_autostart::ManagerExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };

    /// Passed by the login item, so a sign-in start stays in the tray.
    pub const FLAG: &str = "--autostart";

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    /// A debug build is never a login item: it loads the dev server's URL, nothing serves that
    /// at sign-in, and the user gets an empty "localhost refused to connect" window. Builds
    /// before this one registered themselves on every settings save, so a debug start removes
    /// a registration that launches this very executable, and leaves any other alone.
    #[cfg(debug_assertions)]
    pub fn reconcile(app: &AppHandle, _wanted: bool) {
        let name = &app.package_info().name;
        let (Some(command), Some(exe)) = (read_run_value(name), current_exe()) else {
            return;
        };
        if launches(&command, &exe).is_none() {
            return;
        }
        match delete_run_value(name) {
            Ok(()) => crate::diag::info(format!("removed the login item a debug build had registered: {command}")),
            Err(error) => crate::diag::warn(format!("could not remove the debug build's login item: {error}")),
        }
    }

    /// Startup repair: the setting is the source of truth, but only a registration that is
    /// missing or launches something else is rewritten. One the user switched off in Task
    /// Manager stays off; re-approving it on every start would overrule them.
    #[cfg(not(debug_assertions))]
    pub fn reconcile(app: &AppHandle, wanted: bool) {
        let name = &app.package_info().name;
        // An older build registered itself under the name it had then. That entry survives the
        // rename and fires at every sign-in, so the user gets two programs, two sets of tray
        // icons, and one of them quietly working with a folder this build no longer reads.
        drop_former_registration(name);
        let registered = read_run_value(name);
        if wanted {
            let current = match (&registered, current_exe()) {
                (Some(command), Some(exe)) => launches(command, &exe) == Some(FLAG),
                _ => false,
            };
            if !current {
                enable(app);
            }
        } else if registered.is_some() || app.autolaunch().is_enabled().unwrap_or(false) {
            disable(app);
        }
    }

    /// The user flipped the setting: do exactly what it says.
    pub fn apply(app: &AppHandle, wanted: bool) {
        #[cfg(debug_assertions)]
        reconcile(app, wanted);
        #[cfg(not(debug_assertions))]
        if wanted {
            enable(app);
        } else {
            disable(app);
        }
    }

    #[cfg(not(debug_assertions))]
    fn enable(app: &AppHandle) {
        // The plugin writes HKLM whenever it is allowed to, and an elevated Meerkat (the thermal
        // sensor relaunch) is: that would start Meerkat at every user's sign-in. The next normal
        // start writes the per-user value instead.
        if unsafe { windows::Win32::UI::Shell::IsUserAnAdmin() }.as_bool() {
            crate::diag::info("running elevated; the login item is left to the next normal start");
            return;
        }
        match app.autolaunch().enable() {
            Ok(()) => crate::diag::info("registered the login item"),
            Err(error) => crate::diag::warn(format!("could not register the login item: {error}")),
        }
    }

    #[cfg(not(debug_assertions))]
    fn disable(app: &AppHandle) {
        match app.autolaunch().disable() {
            Ok(()) => crate::diag::info("removed the login item"),
            Err(error) => crate::diag::warn(format!("could not remove the login item: {error}")),
        }
    }

    fn current_exe() -> Option<String> {
        std::env::current_exe().ok().map(|path| path.display().to_string())
    }

    /// The arguments of a `Run` command line when it launches exactly `exe`, `None` for any
    /// other program. Paths compare without regard to letter case, as Windows does. The plugin
    /// writes the path unquoted, so one with spaces is matched as a whole path followed by a
    /// space or the end, never as a bare prefix (`meerkat.exe.old` is somebody else).
    pub fn launches<'a>(command: &'a str, exe: &str) -> Option<&'a str> {
        let command = command.trim();
        if exe.is_empty() {
            return None;
        }
        let rest = match command.strip_prefix('"') {
            Some(quoted) => strip_path(quoted, exe)?.strip_prefix('"')?,
            None => strip_path(command, exe)?,
        };
        (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim())
    }

    /// `text` after a leading `path`, compared character by character without case.
    fn strip_path<'a>(text: &'a str, path: &str) -> Option<&'a str> {
        let mut rest = text.chars();
        for expected in path.chars() {
            let actual = rest.next()?;
            if !actual.to_lowercase().eq(expected.to_lowercase()) {
                return None;
            }
        }
        Some(rest.as_str())
    }

    /// The name the login item was written under before the app was renamed, and the executable
    /// name that goes with it.
    const FORMER_NAME: &str = "Pulse";
    const FORMER_EXE: &str = "pulse.exe";

    /// Whether a `Run` command line points at a file called `pulse.exe`, quoted or not, with or
    /// without arguments after it.
    ///
    /// `launches` compares whole paths and so cannot tell an old registration for *this* program
    /// from one for a different program that happens to be called Pulse. This is deliberately
    /// narrower than "the key is named Pulse": the entry is only ours to remove when the file it
    /// launches is the one the former build was called.
    fn names_former_app(command: &str) -> bool {
        let command = command.trim();
        let head = match command.strip_prefix('"') {
            Some(quoted) => quoted.split_once('"').map_or(quoted, |(head, _)| head),
            None => command.split_whitespace().next().unwrap_or(command),
        };
        head
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(FORMER_EXE))
    }

    /// Removes the registration the former build left, when it is recognisably the former build's.
    #[cfg(not(debug_assertions))]
    fn drop_former_registration(current_name: &str) {
        if current_name.eq_ignore_ascii_case(FORMER_NAME) {
            return;
        }
        let Some(command) = read_run_value(FORMER_NAME) else { return };
        if !names_former_app(&command) {
            return;
        }
        match delete_run_value(FORMER_NAME) {
            Ok(()) => {
                crate::diag::info(format!("removed the login item the previous build registered: {command}"))
            }
            Err(error) => {
                crate::diag::warn(format!("could not remove the previous build's login item: {error}"))
            }
        }
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_run_value(name: &str) -> Option<String> {
        let key = wide(RUN_KEY);
        let value = wide(name);
        // Raw text: an expandable value with %VARIABLES% simply never matches this executable.
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut size = 0u32;
        let status = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr()), flags, None, None, Some(&mut size))
        };
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    fn delete_run_value(name: &str) -> Result<(), String> {
        use windows::Win32::System::Registry::RegDeleteKeyValueW;
        let key = wide(RUN_KEY);
        let value = wide(name);
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), PCWSTR(value.as_ptr())) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("error {}", status.0))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const DEV: &str = r"E:\workspaces\Meerkat\src-tauri\target\debug\meerkat.exe";

        // The value an old debug build left: the bare path, no arguments.
        #[test]
        fn the_registration_an_old_debug_build_left_is_recognised() {
            assert_eq!(launches(DEV, DEV), Some(""));
            assert_eq!(launches(r"e:\WORKSPACES\meerkat\src-tauri\target\debug\MEERKAT.EXE", DEV), Some(""));
            assert_eq!(launches(&format!("{DEV} --autostart"), DEV), Some("--autostart"));
            assert_eq!(launches(&format!("\"{DEV}\" --autostart"), DEV), Some("--autostart"));
            assert_eq!(launches(&format!("  \"{DEV}\"  "), DEV), Some(""));
        }

        #[test]
        fn a_registration_for_any_other_program_is_not_ours() {
            let release = r"C:\Users\me\AppData\Local\Meerkat\meerkat.exe";
            assert_eq!(launches(release, DEV), None);
            assert_eq!(launches(&format!("{DEV}.old"), DEV), None, "a longer file name is another file");
            assert_eq!(launches(&format!("{DEV}x --autostart"), DEV), None);
            assert_eq!(launches(&format!("\"{DEV}.old\""), DEV), None);
            assert_eq!(launches(&format!("\"{DEV}"), DEV), None, "an unterminated quote is not a match");
            assert_eq!(launches(r"E:\workspaces\Meerkat\src-tauri\target\debug", DEV), None);
            assert_eq!(launches("", DEV), None);
            assert_eq!(launches(DEV, ""), None);
        }

        // The plugin writes `<path> <args>` without quotes, so a path with spaces has to match
        // as a whole before the arguments start.
        #[test]
        fn an_unquoted_path_with_spaces_matches_as_a_whole() {
            let installed = r"C:\Program Files\Meerkat\meerkat.exe";
            assert_eq!(launches(&format!("{installed} --autostart"), installed), Some("--autostart"));
            assert_eq!(launches(r"C:\Program Files\Meerkat\meerkat.exe.bak --autostart", installed), None);
            assert_eq!(launches(r"C:\Program", installed), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tick_ms;

    #[test]
    fn the_sampler_slows_down_when_nothing_is_on_screen_and_again_on_battery() {
        assert_eq!(tick_ms(true, true), 1000);
        assert_eq!(tick_ms(false, true), 2000, "no window to feed");
        assert_eq!(tick_ms(true, false), 2000, "on battery");
        assert_eq!(tick_ms(false, false), 4000);
    }
}
