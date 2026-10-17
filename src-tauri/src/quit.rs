//! What Quit and Force quit are allowed to touch, and how they touch it.
//!
//! The decisions are plain functions of what the snapshot said and what Windows says now
//! (`quit_policy`, `same_application`, `family_of`), so they are tested without a process to kill.
//! What opens a handle is in the `os` section at the bottom.
//!
//! Three things are guarded, each of which used to be possible from the app list:
//!
//! * Ending something Windows depends on, or Meerkat itself (and with it the tray and the sampler),
//!   or the WebView2 processes that draw Meerkat's windows.
//! * Ending the wrong process. The ids come from a snapshot up to two seconds old, and Windows
//!   reuses an id as soon as its process is gone, so every id is checked against the live process
//!   (when it was created, and where its image is) *through the handle that then ends it*.
//! * Reporting a refusal in a sentence. The result carries a code the frontend words itself.

use std::collections::{HashMap, HashSet};

use windows::core::{HRESULT, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, HWND, LPARAM, WPARAM,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, PostMessageW, WM_CLOSE,
};

use crate::metrics::grouping::UNREADABLE_KEY;
use crate::models::{AppInfo, AppKind, QuitResult, WindowInfo};

/// Why a quit did not happen. On the wire it is the code; the frontend owns the words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitError {
    /// The sampler has not produced a snapshot yet.
    NoReading,
    /// The application is not in the latest snapshot (it ended, or its row was regrouped).
    Gone,
    /// The row is a count of processes this user cannot open, not a program.
    NotAnApplication,
    /// Ending it would take Windows, or Meerkat, down with it.
    Protected,
    /// A graceful quit found no visible window to ask.
    NoWindow,
    /// Windows refused: the process is more privileged than Meerkat.
    Denied,
}

impl QuitError {
    pub const fn code(self) -> &'static str {
        match self {
            QuitError::NoReading => "no-reading",
            QuitError::Gone => "gone",
            QuitError::NotAnApplication => "not-an-application",
            QuitError::Protected => "protected",
            QuitError::NoWindow => "no-window",
            QuitError::Denied => "denied",
        }
    }

    /// A quit that did nothing, as the command answers it.
    pub fn refusal(self, force: bool) -> QuitResult {
        QuitResult { asked: 0, force_used: force, error: Some(self.code().to_string()) }
    }
}

/// What may be done to an application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Closing its windows and ending its processes are both fine.
    Allow,
    /// Its windows may be asked to close, its processes may not be ended: a service host, which
    /// carries unrelated services and has no window to ask anyway.
    CloseOnly,
    /// Neither, for this reason.
    Refuse(QuitError),
}

impl Policy {
    /// `Ok` when the requested kind of quit is allowed, otherwise what to report.
    pub fn permits(self, force: bool) -> Result<(), QuitError> {
        match self {
            Policy::Allow => Ok(()),
            Policy::CloseOnly if !force => Ok(()),
            Policy::CloseOnly => Err(QuitError::Protected),
            Policy::Refuse(why) => Err(why),
        }
    }
}

/// Images Windows runs itself on. Ending one is a blue screen, a logoff or a black desktop, never
/// "quit an app". Compared by file stem, in lower case.
const DENIED_IMAGES: &[&str] = &[
    "csrss",
    "wininit",
    "winlogon",
    "services",
    "lsass",
    "lsaiso",
    "lsm",
    "smss",
    "system",
    "system idle process",
    "secure system",
    "registry",
    "memcompression",
    "memory compression",
    "dwm",
    "fontdrvhost",
];

/// The file name of a path (or a bare image name) without its extension, in lower case.
fn image_stem(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    match file.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => file,
    }
}

/// Whether `path` names one of the images Windows depends on, whatever its letter case.
pub fn is_denied_image(path: &str) -> bool {
    DENIED_IMAGES.contains(&image_stem(path).as_str())
}

/// What may be done to `app`. `own_family` is Meerkat's own process and everything that descends
/// from it (the WebView2 host and its renderers), as `family_of` computes it.
pub fn quit_policy(app: &AppInfo, own_family: &HashSet<u32>) -> Policy {
    if app.key == UNREADABLE_KEY {
        return Policy::Refuse(QuitError::NotAnApplication);
    }
    let protected = app.kind == AppKind::System
        // Idle (0) and System (4) have no image to speak of, and the ids around them are the
        // kernel's own.
        || app.main_pid <= 4
        || app.pids.iter().any(|pid| *pid <= 4 || own_family.contains(pid))
        || is_denied_image(&app.exe_path)
        // Without a path there is nothing to check a process against later, and a process this
        // user cannot even name is not one it can end.
        || app.exe_path.is_empty();
    if protected {
        return Policy::Refuse(QuitError::Protected);
    }
    if image_stem(&app.exe_path) == "svchost" {
        return Policy::CloseOnly;
    }
    Policy::Allow
}

/// `root` and every process that descends from it, given each process's parent id. The table is
/// walked to a fixed point, so a parent id that was reused (a cycle, or a parent listed after its
/// child) ends the walk instead of looping.
pub fn family_of(root: u32, parent_of: &HashMap<u32, u32>) -> HashSet<u32> {
    let mut family = HashSet::from([root]);
    loop {
        let before = family.len();
        for (pid, parent) in parent_of {
            if *pid != 0 && family.contains(parent) {
                family.insert(*pid);
            }
        }
        if family.len() == before {
            return family;
        }
    }
}

/// Meerkat's own process and all of its descendants, as they are right now.
pub fn own_family() -> HashSet<u32> {
    family_of(std::process::id(), &os::parent_table())
}

/// A path in the form two paths are compared in: lower case, backslashes, no `\\?\` prefix.
fn normal_path(path: &str) -> String {
    let trimmed = path.trim();
    let trimmed = trimmed.strip_prefix(r"\\?\").unwrap_or(trimmed);
    trimmed.replace('/', "\\").to_lowercase()
}

fn directory_of(normal: &str) -> &str {
    normal.rsplit_once('\\').map(|(head, _)| head).unwrap_or("")
}

/// Whether a live process is still the one the snapshot listed under `app_exe`.
///
/// A process that was created after the snapshot was taken cannot be in it: its id was reused.
/// That is the real test, since a creation time is exact. The image has to be the application's
/// own, or sit next to it, which is how the application's processes were grouped in the first
/// place (a helper with another name in the same folder belongs to the same row).
pub fn same_application(
    app_exe: &str,
    process_exe: Option<&str>,
    created_ms: Option<u64>,
    taken_at_ms: u64,
) -> bool {
    if matches!(created_ms, Some(created) if created > taken_at_ms) {
        return false;
    }
    let Some(process_exe) = process_exe else {
        return false;
    };
    let (theirs, ours) = (normal_path(process_exe), normal_path(app_exe));
    if theirs.is_empty() || ours.is_empty() {
        return false;
    }
    theirs == ours || (directory_of(&theirs) == directory_of(&ours) && !directory_of(&ours).is_empty())
}

/// The answer to a force quit that ended `ended` processes and was refused by `denied`; the rest
/// were already gone, or were no longer this application's.
pub fn force_result(ended: usize, denied: usize) -> QuitResult {
    let error = match (ended, denied) {
        (0, 0) => Some(QuitError::Gone),
        (0, _) => Some(QuitError::Denied),
        _ => None,
    };
    QuitResult { asked: ended, force_used: true, error: error.map(|e| e.code().to_string()) }
}

/// The answer to a graceful quit that asked `asked` windows to close; `refused` windows were
/// there and Windows would not take the message.
pub fn close_result(asked: usize, refused: usize) -> QuitResult {
    let error = match (asked, refused) {
        (0, 0) => Some(QuitError::NoWindow),
        (0, _) => Some(QuitError::Denied),
        _ => None,
    };
    QuitResult { asked, force_used: false, error: error.map(|e| e.code().to_string()) }
}

/// What happened to one process of a force quit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    Yes,
    /// Not there any more, or no longer this application's, or one that is never ended.
    Skipped,
    Denied,
}

/// Ends every process of `app` that is still the one the snapshot listed. `taken_at_ms` is when
/// that snapshot was taken.
pub fn force_quit(app: &AppInfo, taken_at_ms: u64, family: &HashSet<u32>) -> QuitResult {
    let (mut ended, mut denied) = (0usize, 0usize);
    for pid in &app.pids {
        match end_process(app, *pid, taken_at_ms, family) {
            Ended::Yes => ended += 1,
            Ended::Denied => denied += 1,
            Ended::Skipped => {}
        }
    }
    force_result(ended, denied)
}

fn end_process(app: &AppInfo, pid: u32, taken_at_ms: u64, family: &HashSet<u32>) -> Ended {
    if pid <= 4 || family.contains(&pid) {
        return Ended::Skipped;
    }
    let handle = match os::open(pid, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(handle) => handle,
        Err(os::OpenError::Gone) => return Ended::Skipped,
        Err(os::OpenError::Refused) => return Ended::Denied,
    };
    let image = os::image_path(&handle);
    if !same_application(&app.exe_path, image.as_deref(), os::created_ms(&handle), taken_at_ms) {
        return Ended::Skipped;
    }
    // A process of a protected image is never ended, whatever row it was found under.
    if image.as_deref().is_some_and(is_denied_image) {
        return Ended::Skipped;
    }
    if os::terminate(&handle) {
        Ended::Yes
    } else {
        Ended::Denied
    }
}

/// Asks every visible window of `app` to close, the way the close button does.
pub fn ask_to_close(app: &AppInfo, taken_at_ms: u64) -> QuitResult {
    let (mut asked, mut refused) = (0usize, 0usize);
    for window in app.windows.iter().filter(|w| w.visible) {
        if !window_is_still_theirs(app, window, taken_at_ms) {
            continue;
        }
        match os::post_close(window.hwnd) {
            os::Posted::Yes => asked += 1,
            os::Posted::Refused => refused += 1,
            os::Posted::Gone => {}
        }
    }
    close_result(asked, refused)
}

/// A window handle is a number Windows hands out again once a window is destroyed, so one from a
/// snapshot is only used when it still belongs to a process of the application.
fn window_is_still_theirs(app: &AppInfo, window: &WindowInfo, taken_at_ms: u64) -> bool {
    let Some(pid) = os::window_owner(window.hwnd) else {
        return false;
    };
    if pid != window.pid || !app.pids.contains(&pid) {
        return false;
    }
    let Ok(handle) = os::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
        return false;
    };
    same_application(&app.exe_path, os::image_path(&handle).as_deref(), os::created_ms(&handle), taken_at_ms)
}

/// Everything that holds a handle.
mod os {
    use super::*;

    /// A process handle that closes itself.
    pub struct Process(HANDLE);

    impl Drop for Process {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub enum OpenError {
        /// No process has this id any more.
        Gone,
        /// Windows would not hand over the rights asked for.
        Refused,
    }

    pub fn open(pid: u32, rights: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS) -> Result<Process, OpenError> {
        match unsafe { OpenProcess(rights, false, pid) } {
            Ok(handle) => Ok(Process(handle)),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => Err(OpenError::Gone),
            Err(_) => Err(OpenError::Refused),
        }
    }

    /// The full path of the process's image, as the Win32 name.
    pub fn image_path(process: &Process) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut size).ok()?;
        }
        String::from_utf16(&buffer[..size as usize]).ok()
    }

    /// When the process was created, in milliseconds since the Unix epoch.
    pub fn created_ms(process: &Process) -> Option<u64> {
        let (mut created, mut exited, mut kernel, mut user) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        // 100 ns units since 1601; 11 644 473 600 s lie between that and 1970.
        (ticks / 10_000).checked_sub(11_644_473_600_000)
    }

    pub fn terminate(process: &Process) -> bool {
        unsafe { TerminateProcess(process.0, 1) }.is_ok()
    }

    pub fn window_owner(hwnd: isize) -> Option<u32> {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() {
                return None;
            }
            let mut pid = 0u32;
            let thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
            (thread != 0 && pid != 0).then_some(pid)
        }
    }

    pub enum Posted {
        Yes,
        Refused,
        Gone,
    }

    pub fn post_close(hwnd: isize) -> Posted {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        match unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
            Ok(()) => Posted::Yes,
            // User interface privilege isolation: Meerkat is not elevated and the window is.
            Err(error) if error.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) => Posted::Refused,
            Err(_) => Posted::Gone,
        }
    }

    /// Every process id on the machine with the id of its parent, from a Toolhelp snapshot,
    /// which needs no rights on the processes themselves.
    pub fn parent_table() -> HashMap<u32, u32> {
        let mut table = HashMap::new();
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return table;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    table.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snapshot);
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD: &str = r"C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE";

    fn app(key: &str, exe: &str, kind: AppKind, pids: &[u32]) -> AppInfo {
        AppInfo {
            key: key.to_string(),
            name: "x".to_string(),
            exe_path: exe.to_string(),
            kind,
            main_pid: pids.iter().copied().min().unwrap_or(0),
            process_count: pids.len(),
            cpu_percent: 0.0,
            memory_bytes: 0,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
            read_bytes_total: 0,
            write_bytes_total: 0,
            connections: 0,
            listening_ports: Vec::new(),
            oldest_started_at_ms: None,
            windows: Vec::new(),
            pids: pids.to_vec(),
            unmeasured_processes: 0,
        }
    }

    fn nobody() -> HashSet<u32> {
        HashSet::new()
    }

    #[test]
    fn an_ordinary_application_may_be_closed_and_ended() {
        let word = app(&WORD.to_lowercase(), WORD, AppKind::App, &[4120, 5012]);
        assert_eq!(quit_policy(&word, &nobody()), Policy::Allow);
        assert_eq!(Policy::Allow.permits(false), Ok(()));
        assert_eq!(Policy::Allow.permits(true), Ok(()));
    }

    #[test]
    fn meerkat_itself_and_everything_that_descends_from_it_is_protected() {
        let own = HashSet::from([900, 901, 902]);
        // Meerkat's own row.
        let meerkat = app(r"c:\meerkat\meerkat.exe", r"C:\Meerkat\meerkat.exe", AppKind::App, &[900]);
        assert_eq!(quit_policy(&meerkat, &own), Policy::Refuse(QuitError::Protected));
        // The WebView2 host is another executable in another folder, but it is Meerkat's.
        let webview = app(
            r"c:\program files (x86)\microsoft\edgewebview\application\msedgewebview2.exe",
            r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application\msedgewebview2.exe",
            AppKind::Background,
            &[880, 901, 902],
        );
        assert_eq!(quit_policy(&webview, &own), Policy::Refuse(QuitError::Protected));
        // The same program started by somebody else is an ordinary one.
        let theirs = app("webview", r"C:\Edge\msedgewebview2.exe", AppKind::Background, &[3000, 3001]);
        assert_eq!(quit_policy(&theirs, &own), Policy::Allow);
    }

    #[test]
    fn the_kernels_own_ids_are_protected_whatever_they_are_called() {
        for pids in [&[4u32][..], &[0], &[4, 1000], &[2000, 3]] {
            let looks_fine = app("x", r"C:\Tools\tool.exe", AppKind::App, pids);
            assert_eq!(quit_policy(&looks_fine, &nobody()), Policy::Refuse(QuitError::Protected), "{pids:?}");
        }
    }

    #[test]
    fn the_images_windows_runs_on_are_protected_in_any_letter_case() {
        for exe in [
            r"C:\Windows\System32\csrss.exe",
            r"C:\WINDOWS\SYSTEM32\CSRSS.EXE",
            r"c:\windows\system32\Wininit.exe",
            r"C:\Windows\System32\winlogon.exe",
            r"C:\Windows\System32\services.exe",
            r"C:\Windows\System32\lsass.exe",
            r"C:\Windows\System32\smss.exe",
            r"C:\Windows\System32\dwm.exe",
            r"C:\Windows\System32\fontdrvhost.exe",
            "System",
            "Registry",
            "MemCompression",
        ] {
            let app = app("x", exe, AppKind::Background, &[612]);
            assert_eq!(quit_policy(&app, &nobody()), Policy::Refuse(QuitError::Protected), "{exe}");
        }
        // A look-alike in another folder is still refused by name; a longer name is not the name.
        assert!(is_denied_image(r"D:\Games\csrss.exe"));
        assert!(!is_denied_image(r"C:\Tools\csrss-helper.exe"));
        assert!(!is_denied_image(r"C:\Tools\mydwm.exe"));
    }

    #[test]
    fn a_system_row_or_a_process_without_a_path_is_protected() {
        let system = app("x", r"C:\Windows\System32\something.exe", AppKind::System, &[700]);
        assert_eq!(quit_policy(&system, &nobody()), Policy::Refuse(QuitError::Protected));
        // No path: nothing to check a process against, and the user cannot open it anyway.
        let unnamed = app("pid:812", "", AppKind::App, &[812]);
        assert_eq!(quit_policy(&unnamed, &nobody()), Policy::Refuse(QuitError::Protected));
    }

    #[test]
    fn the_row_of_unreadable_processes_is_not_an_application() {
        let folded = app(UNREADABLE_KEY, "", AppKind::System, &[8, 12, 16]);
        assert_eq!(quit_policy(&folded, &nobody()), Policy::Refuse(QuitError::NotAnApplication));
    }

    // A service host carries services that have nothing to do with one another: it may be asked to
    // close (it has no window, so that finds nothing) but is never ended from here.
    #[test]
    fn a_service_host_can_be_asked_but_never_ended() {
        let host = app(
            r"c:\windows\system32\svchost.exe|netsvcs",
            r"C:\Windows\System32\svchost.exe",
            AppKind::Service,
            &[1500, 1504],
        );
        let policy = quit_policy(&host, &nobody());
        assert_eq!(policy, Policy::CloseOnly);
        assert_eq!(policy.permits(false), Ok(()));
        assert_eq!(policy.permits(true), Err(QuitError::Protected));
        // Upper case is the same program.
        let loud = app("x", r"C:\WINDOWS\SYSTEM32\SVCHOST.EXE", AppKind::Service, &[1600]);
        assert_eq!(quit_policy(&loud, &nobody()), Policy::CloseOnly);
    }

    #[test]
    fn a_refused_policy_refuses_both_kinds_of_quit() {
        let refuse = Policy::Refuse(QuitError::Protected);
        assert_eq!(refuse.permits(false), Err(QuitError::Protected));
        assert_eq!(refuse.permits(true), Err(QuitError::Protected));
    }

    // The frontend maps these strings; they are the contract.
    #[test]
    fn the_codes_are_the_ones_the_frontend_knows() {
        let codes: Vec<&str> = [
            QuitError::NoReading,
            QuitError::Gone,
            QuitError::NotAnApplication,
            QuitError::Protected,
            QuitError::NoWindow,
            QuitError::Denied,
        ]
        .into_iter()
        .map(QuitError::code)
        .collect();
        assert_eq!(codes, ["no-reading", "gone", "not-an-application", "protected", "no-window", "denied"]);

        let refused = QuitError::Protected.refusal(true);
        assert_eq!((refused.asked, refused.force_used, refused.error.as_deref()), (0, true, Some("protected")));
    }

    #[test]
    fn the_family_is_the_process_and_all_it_started() {
        // 100 (meerkat) -> 101 (webview host) -> 102, 103 (renderers); 200 is unrelated.
        let parents = HashMap::from([(100, 1), (101, 100), (102, 101), (103, 101), (200, 1), (201, 200), (1, 0)]);
        assert_eq!(family_of(100, &parents), HashSet::from([100, 101, 102, 103]));
        assert_eq!(family_of(200, &parents), HashSet::from([200, 201]));
    }

    #[test]
    fn a_table_with_a_cycle_ends_the_walk() {
        // Parent ids are reused, so 7 and 8 can name each other. Neither is anybody's descendant but
        // their own, and the walk has to finish.
        let parents = HashMap::from([(7, 8), (8, 7), (9, 8), (0, 0), (4, 0)]);
        assert_eq!(family_of(7, &parents), HashSet::from([7, 8, 9]));
        assert_eq!(family_of(50, &parents), HashSet::from([50]));
    }

    #[test]
    fn the_same_image_in_another_letter_case_is_the_same_application() {
        assert!(same_application(WORD, Some(&WORD.to_uppercase()), Some(1_000), 5_000));
        assert!(same_application(WORD, Some(&format!(r"\\?\{WORD}")), Some(1_000), 5_000));
        assert!(same_application(WORD, Some(&WORD.replace('\\', "/")), Some(1_000), 5_000));
    }

    #[test]
    fn a_helper_beside_the_application_belongs_to_it() {
        let helper = r"C:\Program Files\Microsoft Office\root\Office16\ai.exe";
        assert!(same_application(WORD, Some(helper), Some(1_000), 5_000));
    }

    // The case this exists for: the process in the snapshot ended, and Windows gave its id to
    // whatever started next.
    #[test]
    fn a_reused_id_is_not_the_application() {
        let stranger = r"C:\Windows\System32\notepad.exe";
        assert!(!same_application(WORD, Some(stranger), Some(1_000), 5_000), "another folder");
        // Created after the snapshot, even though it sits in the application's own folder.
        assert!(!same_application(WORD, Some(WORD), Some(5_001), 5_000), "started after the snapshot");
        assert!(same_application(WORD, Some(WORD), Some(5_000), 5_000), "at the same millisecond is not later");
    }

    #[test]
    fn a_process_that_cannot_be_identified_is_left_alone() {
        assert!(!same_application(WORD, None, Some(1_000), 5_000));
        assert!(!same_application(WORD, Some(""), Some(1_000), 5_000));
        assert!(!same_application("", Some(WORD), Some(1_000), 5_000));
        // An unknown creation time does not decide it either way; the image still has to match.
        assert!(same_application(WORD, Some(WORD), None, 5_000));
        assert!(!same_application(WORD, Some(r"C:\x\y.exe"), None, 5_000));
    }

    #[test]
    fn a_force_quit_reports_what_actually_ended() {
        let all = force_result(3, 0);
        assert_eq!((all.asked, all.force_used, all.error), (3, true, None));
        let some = force_result(2, 1);
        assert_eq!((some.asked, some.error), (2, None), "a partial quit still quit something");
        assert_eq!(force_result(0, 2).error.as_deref(), Some("denied"), "every kill failed");
        assert_eq!(force_result(0, 0).error.as_deref(), Some("gone"), "nothing left that was theirs");
    }

    #[test]
    fn a_graceful_quit_reports_windows_asked_and_windows_that_would_not_listen() {
        let asked = close_result(2, 0);
        assert_eq!((asked.asked, asked.force_used, asked.error), (2, false, None));
        assert_eq!(close_result(0, 0).error.as_deref(), Some("no-window"));
        assert_eq!(close_result(0, 1).error.as_deref(), Some("denied"), "an elevated window");
        assert_eq!(close_result(1, 1).error, None);
    }

    // The real thing, against this test process: Meerkat's own family contains it, so the policy
    // would refuse a row holding it, and an application row that lists it is never ended.
    #[test]
    fn this_process_is_in_its_own_family_and_cannot_be_ended_through_a_row() {
        let family = own_family();
        assert!(family.contains(&std::process::id()));
        let mine = app("self", &std::env::current_exe().unwrap().to_string_lossy(), AppKind::App, &[std::process::id()]);
        assert_eq!(quit_policy(&mine, &family), Policy::Refuse(QuitError::Protected));
        // Even asked directly (a row that slipped past the policy), the process is skipped.
        let result = force_quit(&mine, u64::MAX, &family);
        assert_eq!(result.asked, 0);
        assert_eq!(result.error.as_deref(), Some("gone"));
    }

    // The real kill path against a throw-away child of this test: a ping that would run for half a
    // minute. Only the id the snapshot listed under the right image, created before the snapshot,
    // is ended; the same id under another image, or "from before it existed", is left running.
    #[test]
    fn only_the_process_the_snapshot_listed_is_ended() {
        use std::process::{Command, Stdio};
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let ping = format!(r"{root}\System32\PING.EXE");
        let mut child = Command::new(&ping)
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("ping starts");
        let pid = child.id();
        let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        // The snapshot "was taken" a second after the child started, so it is a listed process.
        let taken_after = now_ms + 1_000;
        let nobody = HashSet::new();
        let running = |child: &mut std::process::Child| child.try_wait().expect("polls").is_none();

        // The id under an image that is not its own, in another folder: PID reuse, as far as the
        // check can tell.
        let stranger = app("k", r"C:\Elsewhere\other.exe", AppKind::App, &[pid]);
        let refused = force_quit(&stranger, taken_after, &nobody);
        assert_eq!((refused.asked, refused.error.as_deref()), (0, Some("gone")));
        assert!(running(&mut child), "the process was not touched");

        // The right image, but the snapshot predates the process: its id was reused after it.
        let listed = app("k", &ping, AppKind::App, &[pid]);
        let too_old = force_quit(&listed, now_ms.saturating_sub(60_000), &nobody);
        assert_eq!(too_old.asked, 0);
        assert!(running(&mut child), "a process newer than the snapshot is not the listed one");

        // The real thing.
        let ended = force_quit(&listed, taken_after, &nobody);
        assert_eq!((ended.asked, ended.error), (1, None));
        let status = child.wait().expect("the child is gone");
        assert!(!status.success(), "it was ended, not finished");

        // And a second try finds nothing left, which is "gone" and not a refusal.
        let again = force_quit(&listed, taken_after, &nobody);
        assert_eq!((again.asked, again.error.as_deref()), (0, Some("gone")));
    }
}
