#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod audio;
mod history;
mod i18n;
mod icons;
mod ipc;
mod menu;
mod metrics;
mod models;
mod nvml;
mod settings;
mod toast;
mod tray;
mod tray_render;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter, Manager, WindowEvent};

use crate::alerts::Watcher;
use crate::history::History;
use crate::ipc::AppState;
use crate::metrics::Sampler;
use crate::settings::SettingsStore;
use crate::tray::TrayManager;

fn main() -> tauri::Result<()> {
    tauri::Builder::default()
        // First on purpose: plugins set up in registration order, and a second launch exits
        // inside this one's setup, before any other plugin, window, tray icon or sampler of its
        // own exists. The running instance gets the arguments instead.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A login item firing while Pulse already runs has nothing to show.
            if !args.iter().any(|a| a == autostart::FLAG) {
                menu::show_main(app, None);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
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
            ipc::history_ranking,
            ipc::insights,
            ipc::insight_dismiss,
            ipc::quit_app,
            ipc::quit_pulse,
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
            ipc::relaunch_elevated
        ])
        // One listener for every tray icon, however often the icons are rebuilt: a handler set
        // on each icon builder was registered again with every rebuild.
        .on_menu_event(menu::on_menu_event)
        .on_tray_icon_event(menu::on_tray_event)
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
                    // A press on the tray icon lands here first and its release afterwards;
                    // the stamp is how that release knows it was a click to close.
                    if window.is_visible().unwrap_or(false) {
                        menu::note_panel_blur(window.app_handle());
                    }
                    let _ = window.hide();
                }
                ipc::refresh_visibility(window.app_handle());
            }
            _ => {}
        })
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data).ok();
            toast::register(&app_data);
            let settings = SettingsStore::open(&app_data);
            let initial = settings.get();
            app.manage(AppState {
                latest: RwLock::new(None),
                settings,
                settings_saving: Mutex::new(()),
                history: Mutex::new(History::open(&app_data)),
                watcher: Mutex::new(Watcher::open(&app_data)),
                tray: Mutex::new(TrayManager::new()),
                interface_visible: AtomicBool::new(false),
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
/// clean way out (the tray menu, `quit_pulse`, a relaunch) passes through here.
fn on_exit(_app: &AppHandle) {
    eprintln!("exit");
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
            let toaster = toast::Toaster::start(app.clone());
            let mut sampler = Sampler::new();
            loop {
                let snapshot = sampler.sample();
                // Read after the sampling, which takes a while, so a setting that was saved in
                // the meantime already counts for this tick.
                let settings = state.settings.get();

                *state.latest.write().unwrap_or_else(|p| p.into_inner()) = Some(snapshot.clone());
                state.history.lock().unwrap_or_else(|p| p.into_inner()).record(&snapshot);

                // The watcher's lock is released before anything is emitted or shown.
                let insights = state
                    .watcher
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .evaluate(&snapshot, &settings);
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

/// Start at login. The registration itself is the `Run` value the autostart plugin writes
/// (`HKCU\…\Run\Pulse = <exe> --autostart`); this module reads it back so Pulse can tell whose
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
            Ok(()) => eprintln!("removed the login item a debug build had registered: {command}"),
            Err(error) => eprintln!("could not remove the debug build's login item: {error}"),
        }
    }

    /// Startup repair: the setting is the source of truth, but only a registration that is
    /// missing or launches something else is rewritten. One the user switched off in Task
    /// Manager stays off; re-approving it on every start would overrule them.
    #[cfg(not(debug_assertions))]
    pub fn reconcile(app: &AppHandle, wanted: bool) {
        let name = &app.package_info().name;
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
        // The plugin writes HKLM whenever it is allowed to, and an elevated Pulse (the thermal
        // sensor relaunch) is: that would start Pulse at every user's sign-in. The next normal
        // start writes the per-user value instead.
        if unsafe { windows::Win32::UI::Shell::IsUserAnAdmin() }.as_bool() {
            eprintln!("running elevated; the login item is left to the next normal start");
            return;
        }
        match app.autolaunch().enable() {
            Ok(()) => eprintln!("registered the login item"),
            Err(error) => eprintln!("could not register the login item: {error}"),
        }
    }

    #[cfg(not(debug_assertions))]
    fn disable(app: &AppHandle) {
        match app.autolaunch().disable() {
            Ok(()) => eprintln!("removed the login item"),
            Err(error) => eprintln!("could not remove the login item: {error}"),
        }
    }

    fn current_exe() -> Option<String> {
        std::env::current_exe().ok().map(|path| path.display().to_string())
    }

    /// The arguments of a `Run` command line when it launches exactly `exe`, `None` for any
    /// other program. Paths compare without regard to letter case, as Windows does. The plugin
    /// writes the path unquoted, so one with spaces is matched as a whole path followed by a
    /// space or the end, never as a bare prefix (`pulse.exe.old` is somebody else).
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

    #[cfg(debug_assertions)]
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

        const DEV: &str = r"E:\workspaces\Pulse\src-tauri\target\debug\pulse.exe";

        // The value this machine actually had: a debug build, no arguments.
        #[test]
        fn the_registration_an_old_debug_build_left_is_recognised() {
            assert_eq!(launches(DEV, DEV), Some(""));
            assert_eq!(launches(r"e:\WORKSPACES\pulse\src-tauri\target\debug\PULSE.EXE", DEV), Some(""));
            assert_eq!(launches(&format!("{DEV} --autostart"), DEV), Some("--autostart"));
            assert_eq!(launches(&format!("\"{DEV}\" --autostart"), DEV), Some("--autostart"));
            assert_eq!(launches(&format!("  \"{DEV}\"  "), DEV), Some(""));
        }

        #[test]
        fn a_registration_for_any_other_program_is_not_ours() {
            let release = r"C:\Users\me\AppData\Local\Pulse\pulse.exe";
            assert_eq!(launches(release, DEV), None);
            assert_eq!(launches(&format!("{DEV}.old"), DEV), None, "a longer file name is another file");
            assert_eq!(launches(&format!("{DEV}x --autostart"), DEV), None);
            assert_eq!(launches(&format!("\"{DEV}.old\""), DEV), None);
            assert_eq!(launches(&format!("\"{DEV}"), DEV), None, "an unterminated quote is not a match");
            assert_eq!(launches(r"E:\workspaces\Pulse\src-tauri\target\debug", DEV), None);
            assert_eq!(launches("", DEV), None);
            assert_eq!(launches(DEV, ""), None);
        }

        // The plugin writes `<path> <args>` without quotes, so a path with spaces has to match
        // as a whole before the arguments start.
        #[test]
        fn an_unquoted_path_with_spaces_matches_as_a_whole() {
            let installed = r"C:\Program Files\Pulse\pulse.exe";
            assert_eq!(launches(&format!("{installed} --autostart"), installed), Some("--autostart"));
            assert_eq!(launches(r"C:\Program Files\Pulse\pulse.exe.bak --autostart", installed), None);
            assert_eq!(launches(r"C:\Program", installed), None);
        }
    }
}
