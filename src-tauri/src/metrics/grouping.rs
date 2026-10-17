//! The grouping engine: turn the wall of processes into the handful of apps the user
//! actually opened.
//!
//! Rule: a process belongs to the nearest ancestor that lives in the same installation
//! directory, so `chrome.exe --type=renderer`, `Code Helper (Plugin).exe` and
//! `msedge.exe --type=utility` fold into the app the user can see. Where Windows itself
//! splits one executable into many roles (`svchost.exe -k NetworkService`), the role is
//! part of the key instead.
//!
//! Same directory is evidence of one application only where the directory is the application's
//! own. The Windows directory is shared by hundreds of unrelated programs, and the processes
//! that start them (the session manager, the service controller, the shell) are never the
//! application: see [`is_host`].

use std::collections::HashMap;

use sysinfo::{Pid, Process, System};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

use super::netinfo::TcpOwnership;
use super::wins::TopWindow;
use crate::models::{AppInfo, AppKind, WindowInfo};

#[derive(Clone, Debug)]
pub struct ProcSample {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub exe: String,
    pub dir: String,
    pub session: Option<u32>,
    pub memory_bytes: u64,
    pub cpu_percent: f64,
    pub read_total: u64,
    pub write_total: u64,
    /// When the process started, in milliseconds since the Unix epoch (a resolution of one
    /// second). `None` when Windows would not say.
    pub started_at_ms: Option<u64>,
    pub args: Vec<String>,
    pub measurable: bool,
    /// Whether this user may end the process. Service hosts and the rest of the system answer
    /// no, and the list must not offer what Windows will refuse.
    pub can_quit: bool,
    pub private_bytes: u64,
    pub threads: u32,
    pub handles: u32,
    /// For a runtime (node, python…): the project folder it is working for.
    pub project: Option<String>,
}

impl ProcSample {
    pub fn row(&self) -> crate::models::ProcessRow {
        crate::models::ProcessRow {
            pid: self.pid,
            parent_pid: self.ppid,
            name: self.name.clone(),
            exe_path: self.exe.clone(),
            command_line: self.args.clone(),
            cpu_percent: self.cpu_percent,
            memory_bytes: self.memory_bytes,
            private_bytes: self.private_bytes,
            read_bytes_total: self.read_total,
            write_bytes_total: self.write_total,
            threads: self.threads,
            handles: self.handles,
            started_at_ms: self.started_at_ms,
            can_quit: self.can_quit,
        }
    }
}

pub fn collect(sys: &System) -> Vec<ProcSample> {
    let snapshot = snapshot_names();
    let mut out = Vec::with_capacity(sys.processes().len());
    for (pid, p) in sys.processes() {
        out.push(sample(pid, p, &snapshot));
    }
    out
}

/// The image name of every process on the machine.
///
/// sysinfo can only describe a process it can open, and without administrative rights most
/// of Session 0 cannot be opened — which left 181 of 285 rows in the app list with an empty
/// name and an empty path. A Toolhelp snapshot needs no handle rights at all, so it supplies
/// the name sysinfo has to leave blank.
fn snapshot_names() -> HashMap<u32, String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = HashMap::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let used = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                out.insert(
                    entry.th32ProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..used]),
                );
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    out
}

fn sample(pid: &Pid, p: &Process, snapshot: &HashMap<u32, String>) -> ProcSample {
    let exe = p
        .exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = exe
        .rsplit_once(['\\', '/'])
        .map(|(head, _)| head.to_lowercase())
        .unwrap_or_default();
    let usage = p.disk_usage();
    // sysinfo's start time is whole seconds since the Unix epoch.
    let started_at_ms = if p.start_time() == 0 {
        None
    } else {
        Some(p.start_time().saturating_mul(1000))
    };
    let measurable = !exe.is_empty();
    let reported = p.name().to_string_lossy().to_string();
    let name = if reported.is_empty() {
        snapshot.get(&pid.as_u32()).cloned().unwrap_or_default()
    } else {
        reported
    };
    let can_quit = !exe.is_empty();
    ProcSample {
        pid: pid.as_u32(),
        ppid: p.parent().map(|pp| pp.as_u32()),
        name,
        exe,
        dir,
        session: p.session_id().map(|s| s.as_u32()),
        memory_bytes: p.memory(),
        cpu_percent: p.cpu_usage() as f64,
        read_total: usage.total_read_bytes,
        write_total: usage.total_written_bytes,
        started_at_ms,
        args: p.cmd().iter().map(|s| s.to_string_lossy().to_string()).collect(),
        measurable,
        // A row may only offer to close what this user is allowed to end. With sysinfo's
        // handle-based view that is anything with a path, which excludes the Session 0 rows.
        can_quit,
        private_bytes: p.memory(),
        threads: 0,
        handles: 0,
        project: None,
    }
}

/// Per-process byte rates derived from sysinfo's cumulative counters.
pub struct IoRates {
    previous: HashMap<u32, (u64, u64)>,
    elapsed: std::time::Instant,
    current: HashMap<u32, (f64, f64)>,
}

impl Default for IoRates {
    fn default() -> Self {
        Self {
            previous: HashMap::new(),
            elapsed: std::time::Instant::now(),
            current: HashMap::new(),
        }
    }
}

impl IoRates {
    pub fn advance(&mut self, procs: &[ProcSample]) -> &HashMap<u32, (f64, f64)> {
        let seconds = self.elapsed.elapsed().as_secs_f64().max(0.2);
        self.elapsed = std::time::Instant::now();
        let mut next = HashMap::with_capacity(procs.len());
        let mut fresh = HashMap::with_capacity(procs.len());
        for p in procs {
            let rate = match self.previous.get(&p.pid) {
                Some((pr, pw)) => (
                    p.read_total.saturating_sub(*pr) as f64 / seconds,
                    p.write_total.saturating_sub(*pw) as f64 / seconds,
                ),
                None => (0.0, 0.0),
            };
            next.insert(p.pid, rate);
            fresh.insert(p.pid, (p.read_total, p.write_total));
        }
        self.previous = fresh;
        self.current = next;
        &self.current
    }
}

#[derive(Default)]
pub struct NameCache {
    descriptions: HashMap<String, Option<String>>,
}

impl NameCache {
    pub fn describe(&mut self, exe: &str) -> Option<String> {
        if let Some(known) = self.descriptions.get(exe) {
            return known.clone();
        }
        let found = file_description(exe);
        self.descriptions.insert(exe.to_string(), found.clone());
        found
    }
}

/// The product name Explorer and Task Manager show for an executable.
fn file_description(exe: &str) -> Option<String> {
    if exe.is_empty() || !std::path::Path::new(exe).exists() {
        return None;
    }
    let wide: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
    let path = PCWSTR(wide.as_ptr());
    unsafe {
        let size = GetFileVersionInfoSizeW(path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(path, None, size, data.as_mut_ptr().cast()).is_err() {
            return None;
        }
        let mut translation: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut tlen = 0u32;
        let sub = utf16("\\VarFileInfo\\Translation");
        if !VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(sub.as_ptr()),
            &mut translation,
            &mut tlen,
        )
        .as_bool()
        {
            return None;
        }
        let words = std::slice::from_raw_parts(translation.cast::<u16>(), 4);
        let query = utf16(&format!(
            "\\StringFileInfo\\{:04x}{:04x}\\FileDescription",
            words[0], words[1]
        ));
        let mut buffer: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(query.as_ptr()),
            &mut buffer,
            &mut len,
        )
        .as_bool()
        {
            return None;
        }
        // Measured here rather than taken from the documentation, because resources disagree:
        // a well-built block reports the character count *including* the terminating null
        // (notepad's "记事本" comes back as 4), while a hand-built one can report a byte count
        // for the same string, which is twice as long. Trusting the larger number reads past the
        // value into the keys that follow, and past the buffer at worst — that is how
        // "NVIDIA App\0\08\x18\x01FileVe" reached the UI.
        let offset = (buffer as usize).saturating_sub(data.as_ptr() as usize);
        let available = std::slice::from_raw_parts(
            buffer.cast::<u16>(),
            data.len().saturating_sub(offset) / std::mem::size_of::<u16>(),
        );
        let trimmed = version_string(available, len);
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
}

/// Reads one string out of a version resource.
///
/// The length Windows hands back is only reliable as an upper bound: some executables count
/// characters, others count bytes for the same text. Cutting at the first null and refusing to
/// read past what the caller can prove is in the buffer makes the name correct either way.
fn version_string(available: &[u16], claimed: u32) -> String {
    let units = (claimed as usize)
        .saturating_sub(1)
        .min(available.len());
    let chars: Vec<u16> = available[..units].iter().copied().take_while(|unit| *unit != 0).collect();
    let text: String = String::from_utf16_lossy(&chars)
        .chars()
        // FileDescription is free-form text left by whoever built the executable, and some of
        // it carries control bytes that would break the row layout.
        .filter(|c| !c.is_control())
        .collect();
    text.trim().to_string()
}

fn utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

const PIDDEN_EXES: [&str; 8] = [
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "smss.exe",
    "svchost.exe",
    "fontdrvhost.exe",
];

/// The synthetic group holding the processes this user cannot open. Its `AppInfo.name` is
/// empty on purpose: the front end translates the key, so the row reads the same in every
/// language and never looks like a program with a blank label.
pub const UNREADABLE_KEY: &str = "unreadable-processes";

pub fn group(
    procs: &[ProcSample],
    windows: &[TopWindow],
    tcp: &TcpOwnership,
    io: &HashMap<u32, (f64, f64)>,
    names: &mut NameCache,
) -> Vec<AppInfo> {
    let by_pid: HashMap<u32, &ProcSample> = procs.iter().map(|p| (p.pid, p)).collect();

    #[derive(Default)]
    struct Agg {
        key: String,
        pids: Vec<u32>,
        cpu: f64,
        memory: u64,
        read_rate: f64,
        write_rate: f64,
        read_total: u64,
        write_total: u64,
        oldest: Option<u64>,
        exe: String,
        name: String,
        connections: usize,
        ports: Vec<u16>,
        unmeasured: usize,
    }

    let mut groups: HashMap<String, Agg> = HashMap::new();
    let windowed: std::collections::HashSet<u32> = windows.iter().map(|w| w.pid).collect();
    let mut hidden: Vec<u32> = Vec::new();

    for p in procs {
        // A process we cannot open has no path, no memory and no cpu, so on a machine without
        // administrative rights it becomes a row of nothing — 181 of them here, most of them
        // service hosts. They fold into one row that says how many were set aside rather than
        // burying the applications. Anything still showing evidence of life (a window, a
        // socket, a listening port) stays listed under the name the snapshot gives us.
        if !p.measurable
            && p.cpu_percent == 0.0
            && p.memory_bytes == 0
            && !windowed.contains(&p.pid)
            && !tcp.connections.contains_key(&p.pid)
            && !tcp.listening.contains_key(&p.pid)
        {
            hidden.push(p.pid);
            continue;
        }
        let (anchor_key, anchor_exe, anchor_name) = resolve_anchor(p, &by_pid, names);
        let g = groups.entry(anchor_key.clone()).or_insert_with(|| Agg {
            key: anchor_key,
            exe: anchor_exe,
            name: anchor_name,
            ..Default::default()
        });
        g.pids.push(p.pid);
        g.cpu += p.cpu_percent;
        g.memory += p.memory_bytes;
        if let Some((r, w)) = io.get(&p.pid) {
            g.read_rate += *r;
            g.write_rate += *w;
        }
        g.read_total += p.read_total;
        g.write_total += p.write_total;
        if let Some(started) = p.started_at_ms {
            g.oldest = Some(g.oldest.map_or(started, |o| o.min(started)));
        }
        if !p.measurable {
            g.unmeasured += 1;
        }
        if let Some(count) = tcp.connections.get(&p.pid) {
            g.connections += *count;
        }
        if let Some(ports) = tcp.listening.get(&p.pid) {
            for port in ports {
                if !g.ports.contains(port) {
                    g.ports.push(*port);
                }
            }
        }
    }

    let visible_by_pid: HashMap<u32, Vec<&TopWindow>> = {
        let mut m: HashMap<u32, Vec<&TopWindow>> = HashMap::new();
        for w in windows {
            m.entry(w.pid).or_default().push(w);
        }
        m
    };

    let mut out: Vec<AppInfo> = Vec::with_capacity(groups.len());
    for (_, mut g) in groups {
        let mut titled: Vec<WindowInfo> = Vec::new();
        let mut any_visible = false;
        for pid in &g.pids {
            if let Some(list) = visible_by_pid.get(pid) {
                for w in list {
                    any_visible |= w.visible;
                    titled.push(WindowInfo {
                        title: w.title.clone(),
                        pid: *pid,
                        visible: w.visible,
                        hwnd: w.hwnd,
                    });
                }
            }
        }
        titled.sort_by(|a, b| b.visible.cmp(&a.visible));
        titled.truncate(24);
        g.ports.sort_unstable();
        let pids: &[u32] = &g.pids;

        let kind = classify(&g.exe, &g.name, g.pids.iter().min().copied().unwrap_or(0), &by_pid, any_visible);
        out.push(AppInfo {
            key: g.key,
            name: g.name,
            exe_path: g.exe,
            kind,
            main_pid: g.pids.iter().copied().min().unwrap_or(0),
            process_count: g.pids.len(),
            cpu_percent: g.cpu,
            memory_bytes: g.memory,
            read_bytes_per_second: g.read_rate,
            write_bytes_per_second: g.write_rate,
            read_bytes_total: g.read_total,
            write_bytes_total: g.write_total,
            connections: g.connections,
            listening_ports: g.ports,
            oldest_started_at_ms: g.oldest,
            windows: titled,
            pids: pids.to_vec(),
            unmeasured_processes: g.unmeasured,
            services: Vec::new(),
            can_quit: pids.iter().any(|pid| by_pid.get(pid).is_some_and(|p| p.can_quit)),
            control: crate::models::AppControl::default(),
            project: None,
        });
    }

    out.sort_by(|a, b| {
        b.memory_bytes
            .cmp(&a.memory_bytes)
            .then(b.cpu_percent.total_cmp(&a.cpu_percent))
    });

    if !hidden.is_empty() {
        hidden.sort_unstable();
        let count = hidden.len();
        // The name is deliberately empty: the front end translates this key, so the row reads
        // the same in every language and never looks like a program with a blank label.
        out.push(AppInfo {
            key: UNREADABLE_KEY.to_string(),
            name: String::new(),
            exe_path: String::new(),
            kind: AppKind::System,
            main_pid: hidden[0],
            process_count: count,
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
            pids: hidden,
            unmeasured_processes: count,
            services: Vec::new(),
            can_quit: false,
            control: crate::models::AppControl::default(),
            project: None,
        });
    }

    out
}

/// The lower-cased image name: the file name of the path, or the snapshot's name for a process
/// whose path could not be read.
fn image_name(p: &ProcSample) -> String {
    let from_path = p.exe.rsplit(['\\', '/']).next().unwrap_or("");
    let name = if from_path.is_empty() { p.name.as_str() } else { from_path };
    name.to_lowercase()
}

/// A process that starts programs which have nothing to do with it: the session manager and
/// the processes it starts, the service controller and its service hosts, the logon and
/// security processes, and the shell. They live in the Windows directory next to every other
/// tool, so "same directory as the parent" says nothing there. Elevated, `svchost.exe` →
/// `services.exe` → `wininit.exe` is a chain of System32 images, and folding it by directory
/// turned every service host, and every System32 helper started by one, into a single
/// "wininit" app and wiped out the per-role split of `svchost.exe`.
fn is_host(p: &ProcSample) -> bool {
    let image = image_name(p);
    PIDDEN_EXES.contains(&image.as_str()) || matches!(image.as_str(), "explorer.exe" | "lsm.exe")
}

/// Whether this user may end the process: anything with a real executable path is theirs to
/// close, and the rows with no path are Windows' own. `quit` checks again through the handle it
/// opens, so this is only what the row offers.
fn can_terminate(exe: &str) -> bool {
    !exe.is_empty()
}

/// Whether `child` is part of the application `parent` anchors: they live in the same directory,
/// neither is a host, and the parent did not start after the child — a parent id can have been
/// reused by an unrelated process once the real parent exited, and such a "parent" is younger
/// than the child it claims.
fn folds_into(child: &ProcSample, parent: &ProcSample) -> bool {
    if child.dir.is_empty() || parent.dir.is_empty() || child.dir != parent.dir {
        return false;
    }
    if is_host(child) || is_host(parent) {
        return false;
    }
    match (child.started_at_ms, parent.started_at_ms) {
        (Some(child_start), Some(parent_start)) => parent_start <= child_start,
        _ => true,
    }
}

fn resolve_anchor(
    p: &ProcSample,
    by_pid: &HashMap<u32, &ProcSample>,
    names: &mut NameCache,
) -> (String, String, String) {
    let mut current: &ProcSample = p;
    for _ in 0..16 {
        let Some(ppid) = current.ppid else { break };
        let Some(parent) = by_pid.get(&ppid) else { break };
        if !folds_into(current, parent) {
            break;
        }
        current = parent;
    }

    let exe = current.exe.clone();
    // Without a path there is no product name to read either, so the image name from the
    // snapshot is the best label available.
    let base = if exe.is_empty() {
        current.name.as_str()
    } else {
        exe.rsplit(['\\', '/']).next().unwrap_or(current.name.as_str())
    };
    let stem = base.trim_end_matches(".exe").to_string();

    // svchost and dllhost reuse one executable for many unrelated roles.
    let role = role_suffix(current);
    let key = if exe.is_empty() {
        format!("pid:{}", current.pid)
    } else {
        match &role {
            Some(r) => format!("{}|{}", exe.to_lowercase(), r),
            None => exe.to_lowercase(),
        }
    };

    let display = match (&role, names.describe(&exe)) {
        (Some(r), _) => format!("Service host: {r}"),
        (None, Some(description)) => description,
        (None, None) => stem,
    };
    (key, exe, display)
}

fn role_suffix(p: &ProcSample) -> Option<String> {
    let name = p
        .exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    if name != "svchost.exe" {
        return None;
    }
    let position = p.args.iter().position(|a| a.eq_ignore_ascii_case("-k"))?;
    p.args.get(position + 1).cloned()
}

fn classify(
    exe: &str,
    _display: &str,
    lowest_pid: u32,
    by_pid: &HashMap<u32, &ProcSample>,
    has_visible_window: bool,
) -> AppKind {
    let lower = exe.to_lowercase();
    let file = lower.rsplit(['\\', '/']).next().unwrap_or("");
    if lowest_pid <= 8 || file.is_empty() {
        return AppKind::System;
    }
    if lower.contains("\\windowsapps\\") {
        return AppKind::Packaged;
    }
    if PIDDEN_EXES.contains(&file) {
        return if file == "svchost.exe" || file == "services.exe" {
            AppKind::Service
        } else {
            AppKind::System
        };
    }
    if let Some(p) = by_pid.get(&lowest_pid) {
        if p.session == Some(0) {
            return AppKind::Service;
        }
    }
    if has_visible_window {
        AppKind::App
    } else {
        AppKind::Background
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::wins::TopWindow;

    fn proc(pid: u32, exe: &str, image: &str, measurable: bool) -> ProcSample {
        ProcSample {
            pid,
            ppid: None,
            name: image.to_string(),
            exe: exe.to_string(),
            dir: exe
                .rsplit_once(['\\', '/'])
                .map(|(head, _)| head.to_lowercase())
                .unwrap_or_default(),
            session: Some(1),
            memory_bytes: 0,
            cpu_percent: 0.0,
            read_total: 0,
            write_total: 0,
            started_at_ms: None,
            args: Vec::new(),
            measurable,
            can_quit: true,
            private_bytes: 0,
            threads: 0,
            handles: 0,
            project: None,
        }
    }

    fn empty_tcp() -> TcpOwnership {
        TcpOwnership {
            connections: HashMap::new(),
            listening: HashMap::new(),
            total_connections: 0,
            listeners: Vec::new(),
        }
    }

    fn group_of(procs: &[ProcSample], tcp: TcpOwnership, windows: &[TopWindow]) -> Vec<AppInfo> {
        group(procs, windows, &tcp, &HashMap::new(), &mut NameCache::default())
    }

    /// Ten processes nobody can open have to become one honest row, not ten blank ones.
    #[test]
    fn unreadable_and_inert_processes_fold_into_a_single_row() {
        let mut procs: Vec<ProcSample> = (100..110)
            .map(|pid| proc(pid, "", "svchost.exe", false))
            .collect();
        procs.push(proc(500, "C:\\Windows\\notepad.exe", "notepad.exe", true));

        let apps = group_of(&procs, empty_tcp(), &[]);

        assert_eq!(apps.len(), 2, "one application plus the folded group");
        let folded = apps.iter().find(|a| a.key == UNREADABLE_KEY).expect("folded row");
        assert_eq!(folded.process_count, 10);
        assert_eq!(folded.pids.len(), 10);
        assert!(folded.name.is_empty(), "the label comes from the translation");
        assert_eq!(folded.unmeasured_processes, 10);
        assert_eq!(folded.connections, 0);
    }

    /// A socket is evidence of life: such a process stays listed, and the snapshot name is
    /// the only label there is for it.
    #[test]
    fn an_unreadable_process_with_a_listening_port_stays_visible() {
        let procs = vec![proc(300, "", "svchost.exe", false)];
        let mut listening = HashMap::new();
        listening.insert(300u32, vec![445u16]);
        let tcp = TcpOwnership {
            connections: HashMap::new(),
            listening,
            total_connections: 1,
            listeners: Vec::new(),
        };

        let apps = group_of(&procs, tcp, &[]);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].key, "pid:300");
        assert_eq!(apps[0].name, "svchost");
        assert_eq!(apps[0].listening_ports, vec![445]);
    }

    /// Windows belong to a process even when its counters do not, so a visible window keeps
    /// the row on the list.
    #[test]
    fn an_unreadable_process_with_a_window_stays_visible() {
        let procs = vec![proc(400, "", "protected.exe", false)];
        let window = TopWindow {
            pid: 400,
            title: "Something".to_string(),
            visible: true,
            hwnd: 7,
        };

        let apps = group_of(&procs, empty_tcp(), &[window]);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "protected");
        assert_eq!(apps[0].windows[0].title, "Something");
    }

    const SYSTEM32: &str = "C:\\Windows\\System32\\";

    /// A process of `exe` started by `ppid`, at `started` seconds since the epoch.
    fn child(pid: u32, ppid: u32, exe: &str, started: u64) -> ProcSample {
        let image = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
        let mut p = proc(pid, exe, image, true);
        p.ppid = Some(ppid);
        p.started_at_ms = Some(started * 1000);
        p
    }

    fn with_args(mut p: ProcSample, args: &[&str]) -> ProcSample {
        p.args = args.iter().map(|a| a.to_string()).collect();
        p
    }

    fn keys(apps: &[AppInfo]) -> Vec<String> {
        let mut keys: Vec<String> = apps.iter().map(|a| a.key.clone()).collect();
        keys.sort();
        keys
    }

    /// Elevated, every System32 image can be read, and they all share one directory. This chain
    /// used to fold into a single app named after its root.
    #[test]
    fn the_windows_service_chain_stays_apart() {
        let procs = vec![
            child(300, 4, &format!("{SYSTEM32}smss.exe"), 100),
            child(400, 300, &format!("{SYSTEM32}wininit.exe"), 101),
            child(410, 400, &format!("{SYSTEM32}lsass.exe"), 102),
            child(500, 400, &format!("{SYSTEM32}services.exe"), 102),
            with_args(child(600, 500, &format!("{SYSTEM32}svchost.exe"), 103), &["-k", "netsvcs"]),
            with_args(child(610, 500, &format!("{SYSTEM32}svchost.exe"), 103), &["-k", "DcomLaunch"]),
            // Started by a service host, in the same directory: its own program.
            child(700, 610, &format!("{SYSTEM32}RuntimeBroker.exe"), 200),
        ];

        let apps = group_of(&procs, empty_tcp(), &[]);

        assert_eq!(
            keys(&apps),
            vec![
                "c:\\windows\\system32\\lsass.exe",
                "c:\\windows\\system32\\runtimebroker.exe",
                "c:\\windows\\system32\\services.exe",
                "c:\\windows\\system32\\smss.exe",
                "c:\\windows\\system32\\svchost.exe|DcomLaunch",
                "c:\\windows\\system32\\svchost.exe|netsvcs",
                "c:\\windows\\system32\\wininit.exe",
            ]
        );
        assert!(apps.iter().all(|a| a.process_count == 1), "nothing was folded into anything");
    }

    #[test]
    fn a_browser_and_its_helpers_are_one_app() {
        let chrome = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
        let procs = vec![
            child(900, 1, "C:\\Windows\\explorer.exe", 50),
            child(1000, 900, chrome, 100),
            with_args(child(1001, 1000, chrome, 101), &["--type=renderer"]),
            with_args(child(1002, 1000, chrome, 101), &["--type=renderer"]),
            with_args(child(1003, 1000, chrome, 102), &["--type=gpu-process"]),
            // A helper that has its own executable in the same directory.
            child(1004, 1000, "C:\\Program Files\\Google\\Chrome\\Application\\chrome_proxy.exe", 103),
        ];

        let apps = group_of(&procs, empty_tcp(), &[]);

        assert_eq!(apps.len(), 2, "the browser and the shell that started it");
        let browser = apps.iter().find(|a| a.exe_path == chrome).expect("the browser group");
        assert_eq!(browser.process_count, 5);
        let mut pids = browser.pids.clone();
        pids.sort_unstable();
        assert_eq!(pids, vec![1000, 1001, 1002, 1003, 1004]);
    }

    /// The shell is a host: a tool that happens to sit in the Windows directory is its own
    /// app when started from it, not part of "Windows Explorer".
    #[test]
    fn what_the_shell_starts_from_its_own_directory_is_not_the_shell() {
        let procs = vec![
            child(900, 1, "C:\\Windows\\explorer.exe", 50),
            child(901, 900, "C:\\Windows\\regedit.exe", 400),
        ];
        let apps = group_of(&procs, empty_tcp(), &[]);
        assert_eq!(keys(&apps), vec!["c:\\windows\\explorer.exe", "c:\\windows\\regedit.exe"]);
    }

    /// Once its real parent has exited, a process id can be handed to anything. A "parent"
    /// that started after the child cannot be its parent.
    #[test]
    fn a_reused_parent_id_is_not_a_parent() {
        let launcher = "C:\\Apps\\Studio\\launcher.exe";
        let worker = "C:\\Apps\\Studio\\worker.exe";

        let genuine = vec![child(50, 1, launcher, 1_000), child(60, 50, worker, 1_005)];
        assert_eq!(group_of(&genuine, empty_tcp(), &[]).len(), 1, "an older parent folds");

        let same_second = vec![child(50, 1, launcher, 1_000), child(60, 50, worker, 1_000)];
        assert_eq!(group_of(&same_second, empty_tcp(), &[]).len(), 1, "the clock has one-second steps");

        let reused = vec![child(50, 1, launcher, 2_000), child(60, 50, worker, 1_000)];
        assert_eq!(group_of(&reused, empty_tcp(), &[]).len(), 2, "this 'parent' is younger");
    }

    /// An unknown start time proves nothing either way, so it does not block the fold.
    #[test]
    fn an_unknown_start_time_does_not_stop_a_fold() {
        let mut parent = child(50, 1, "C:\\Apps\\Studio\\launcher.exe", 1_000);
        parent.started_at_ms = None;
        let worker = child(60, 50, "C:\\Apps\\Studio\\worker.exe", 1_000);
        assert_eq!(group_of(&[parent, worker], empty_tcp(), &[]).len(), 1);
    }

    /// sysinfo reports the start time as seconds since the epoch. It was once read as seconds
    /// since boot, which turned the field into the process's age, and with it the "younger than
    /// ten minutes" test of the memory alert into one nothing could fail.
    #[test]
    fn the_start_time_of_a_live_process_is_a_moment_not_an_age() {
        use sysinfo::{ProcessesToUpdate, System};
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let me = std::process::id();
        let sample = collect(&system)
            .into_iter()
            .find(|p| p.pid == me)
            .expect("this test process is in the list");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let started = sample.started_at_ms.expect("a process can read its own start time");
        assert!(started <= now, "{started} is in the future of {now}");
        assert!(now - started < 3_600_000, "a test process is not an hour old: {started} vs {now}");
    }

    /// Measured against notepad.exe here: a well-formed block reports characters, with the
    /// terminating null counted in.
    #[test]
    fn version_string_accepts_a_character_count() {
        let text = "记事本";
        let block: Vec<u16> = text.encode_utf16().chain([0]).collect();
        assert_eq!(version_string(&block, 4), text);
    }

    /// Measured against NVIDIA's overlay.exe: the same field can come back as a byte count,
    /// twice as long, and the text still has to stop at the null rather than run into the
    /// key that follows it in the resource.
    #[test]
    fn version_string_cuts_an_overlong_claim_at_the_null() {
        let mut block: Vec<u16> = "NVIDIA App".encode_utf16().collect();
        block.push(0);
        block.extend("FileVersion".encode_utf16());
        assert_eq!(version_string(&block, 22), "NVIDIA App");
    }

    /// The overlong claim is also the one that can read outside the allocation, so the
    /// buffer's own length is the hard limit.
    #[test]
    fn version_string_refuses_to_read_past_the_buffer() {
        let block: Vec<u16> = "Edge".encode_utf16().collect();
        assert_eq!(version_string(&block, 4000), "Edge");
    }

    /// Bugly's SDK ships a FileDescription with control bytes in it.
    #[test]
    fn version_string_drops_control_bytes() {
        let mut block: Vec<u16> = "Bugly".encode_utf16().collect();
        block.push(7);
        block.extend("-Windows".encode_utf16());
        block.push(0);
        assert_eq!(version_string(&block, 25), "Bugly-Windows");
    }
}
