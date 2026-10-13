//! The grouping engine: turn the wall of processes into the handful of apps the user
//! actually opened.
//!
//! Rule: a process belongs to the nearest ancestor that lives in the same installation
//! directory, so `chrome.exe --type=renderer`, `Code Helper (Plugin).exe` and
//! `msedge.exe --type=utility` fold into the app the user can see. The Windows directory is the
//! exception: everything in it shares one folder without being one program. Where Windows itself
//! splits one executable into many roles (`svchost.exe`), the services it runs are part of the
//! key instead, and name the row.

use std::collections::HashMap;
use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};

use super::netinfo::TcpOwnership;
use super::ntproc::{Processes, Sampled};
use super::services::Service;
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

pub fn collect(processes: &mut Processes) -> Vec<ProcSample> {
    let sampled = processes.sample();
    let created: HashMap<u32, u64> = sampled
        .iter()
        .map(|s| (s.process.pid, s.process.create_time))
        .collect();
    sampled.into_iter().map(|s| sample(s, &created)).collect()
}

fn sample(s: Sampled, created: &HashMap<u32, u64>) -> ProcSample {
    let Sampled { process, identity, cpu_percent } = s;
    let dir = identity
        .exe
        .rsplit_once(['\\', '/'])
        .map(|(head, _)| head.to_lowercase())
        .unwrap_or_default();
    // Windows records the pid of whoever started a process and never updates it, so once that
    // parent exits the number can be handed to something unrelated. A real parent is older than
    // its child; anything younger wearing the parent's pid is a stranger.
    let ppid = (process.parent_pid != 0 && process.parent_pid != process.pid)
        .then_some(process.parent_pid)
        .filter(|ppid| created.get(ppid).is_some_and(|born| *born <= process.create_time));
    ProcSample {
        pid: process.pid,
        ppid,
        started_at_ms: process.started_at_ms(),
        session: Some(process.session_id),
        memory_bytes: process.working_set,
        cpu_percent,
        read_total: process.read_bytes,
        write_total: process.write_bytes,
        private_bytes: process.private_bytes,
        threads: process.threads,
        handles: process.handles,
        project: identity.project,
        name: process.name,
        exe: identity.exe,
        dir,
        args: identity.args,
        // The kernel's table measures every process, readable or not.
        measurable: true,
        can_quit: identity.can_terminate,
    }
}

/// `C:\Windows`, lowercased like `ProcSample::dir`.
fn windows_dir() -> &'static str {
    static DIR: OnceLock<String> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::var("SystemRoot")
            .or_else(|_| std::env::var("windir"))
            .unwrap_or_else(|_| "C:\\Windows".to_string())
            .trim_end_matches('\\')
            .to_lowercase()
    })
}

fn in_windows_dir(dir: &str) -> bool {
    let root = windows_dir();
    dir == root || (dir.starts_with(root) && dir[root.len()..].starts_with('\\'))
}

/// Per-process byte rates derived from the kernel's cumulative transfer counters.
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
    services: &HashMap<u32, Vec<Service>>,
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
        services: Vec<String>,
        /// The processes the others were folded into: the app as the user started it.
        roots: Vec<u32>,
        project: Option<String>,
    }

    let mut groups: HashMap<String, Agg> = HashMap::new();
    let windowed: std::collections::HashSet<u32> = windows.iter().map(|w| w.pid).collect();
    let mut hidden: Vec<u32> = Vec::new();

    for p in procs {
        // A process with no figures at all would be a row of nothing. The kernel's table
        // measures every process, so this no longer happens in practice, but a sample built any
        // other way still folds such processes into one row that says how many were set aside.
        // Anything showing evidence of life (a window, a socket, a listening port) stays listed.
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
        let (anchor_key, anchor_exe, anchor_name, root) = resolve_anchor(p, &by_pid, names, services);
        let g = groups.entry(anchor_key.clone()).or_insert_with(|| Agg {
            key: anchor_key,
            exe: anchor_exe,
            name: anchor_name,
            ..Default::default()
        });
        g.pids.push(p.pid);
        if !g.roots.contains(&root) {
            g.roots.push(root);
        }
        if g.project.is_none() {
            g.project = by_pid.get(&root).and_then(|r| r.project.clone());
        }
        for service in services.get(&p.pid).into_iter().flatten() {
            if !g.services.contains(&service.display_name) {
                g.services.push(service.display_name.clone());
            }
        }
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

        // What the app is, and whether it can be ended, is decided by the processes it was
        // started as, not by its helpers: Defender's session helper is the user's own process,
        // but ending it would not end Defender, and it does not make Defender a background app.
        g.roots.sort_unstable();
        let main = g.roots.first().copied().unwrap_or(0);
        let can_quit = g.roots.iter().any(|root| by_pid.get(root).is_some_and(|p| p.can_quit));
        let kind = classify(&g.exe, main, &by_pid, any_visible);
        out.push(AppInfo {
            key: g.key,
            name: g.name,
            exe_path: g.exe,
            kind,
            main_pid: main,
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
            pids: g.pids,
            unmeasured_processes: g.unmeasured,
            services: g.services,
            can_quit,
            control: Default::default(),
            project: g.project,
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
            control: Default::default(),
            project: None,
        });
    }

    out
}

fn resolve_anchor(
    p: &ProcSample,
    by_pid: &HashMap<u32, &ProcSample>,
    names: &mut NameCache,
    services: &HashMap<u32, Vec<Service>>,
) -> (String, String, String, u32) {
    let mut current: &ProcSample = p;
    for _ in 0..16 {
        let Some(ppid) = current.ppid else { break };
        let Some(parent) = by_pid.get(&ppid) else { break };
        if current.dir.is_empty() || parent.dir.is_empty() || parent.dir != current.dir {
            break;
        }
        // System32 is shared by hundreds of programs that start one another: services.exe
        // starts every service host, a service host starts brokers and COM surrogates, and
        // wininit starts services.exe. Climbing through them would fold half of Windows into
        // wininit, so in the Windows directory each executable stands on its own.
        if in_windows_dir(&current.dir) {
            break;
        }
        current = parent;
    }

    let exe = current.exe.clone();
    // Without a path there is no product name to read either, so the image name from the
    // kernel's table is the best label available.
    let base = if exe.is_empty() {
        current.name.as_str()
    } else {
        exe.rsplit(['\\', '/']).next().unwrap_or(current.name.as_str())
    };
    let stem = base.trim_end_matches(".exe").to_string();

    if exe.is_empty() {
        // The pseudo processes (System, Registry, Memory Compression) have no file but do have
        // a fixed name, which keeps their key the same from one boot to the next; history and
        // dismissed alerts are filed under it.
        let key = if current.name.is_empty() {
            format!("pid:{}", current.pid)
        } else {
            format!("image:{}", current.name.to_lowercase())
        };
        return (key, exe, stem, current.pid);
    }

    let lower = exe.to_lowercase();
    // One `node.exe` serves every project on the machine; the project is what the row is.
    if super::projects::is_runtime(base) {
        if let Some(project) = &current.project {
            let folder = project.rsplit(['\\', '/']).next().unwrap_or(project);
            return (format!("{lower}|project:{}", project.to_lowercase()), exe.clone(), format!("{folder} · {stem}"), current.pid);
        }
    }
    if base.eq_ignore_ascii_case("svchost.exe") {
        // One executable, a hundred unrelated services. The services a host runs are what it
        // is, so they make the key and the first of them names the row.
        if let Some(hosted) = services.get(&current.pid).filter(|list| !list.is_empty()) {
            let mut ids: Vec<String> = hosted.iter().map(|s| s.name.to_lowercase()).collect();
            ids.sort_unstable();
            return (format!("{lower}|svc:{}", ids.join("+")), exe, hosted[0].display_name.clone(), current.pid);
        }
        // Not in the service list (it started a moment ago): the group from its command line,
        // when its owner lets us read that, and otherwise the process itself.
        let key = match role_suffix(current) {
            Some(role) => format!("{lower}|{role}"),
            None => format!("{lower}|pid:{}", current.pid),
        };
        return (key, exe, stem, current.pid);
    }

    let display = names.describe(&exe).unwrap_or(stem);
    (lower, exe, display, current.pid)
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

fn classify(exe: &str, main: u32, by_pid: &HashMap<u32, &ProcSample>, has_visible_window: bool) -> AppKind {
    let lower = exe.to_lowercase();
    let file = lower.rsplit(['\\', '/']).next().unwrap_or("");
    if main <= 8 || file.is_empty() {
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
    let root = by_pid.get(&main);
    if root.is_some_and(|p| p.session == Some(0)) {
        return AppKind::Service;
    }
    // A Windows component running in the user's session under an account of its own: the
    // desktop compositor is the one everybody sees, and calling it a background app the user
    // could close would be wrong on both counts.
    let dir = lower.rsplit_once('\\').map(|(head, _)| head).unwrap_or("");
    if in_windows_dir(dir) && root.is_some_and(|p| !p.can_quit) {
        return AppKind::System;
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
            can_quit: false,
            private_bytes: 0,
            threads: 1,
            handles: 1,
            project: None,
        }
    }

    fn child(pid: u32, ppid: u32, exe: &str) -> ProcSample {
        let image = exe.rsplit('\\').next().unwrap_or(exe);
        ProcSample { ppid: Some(ppid), memory_bytes: 1 << 20, ..proc(pid, exe, image, true) }
    }

    fn empty_tcp() -> TcpOwnership {
        TcpOwnership::default()
    }

    fn group_of(procs: &[ProcSample], tcp: TcpOwnership, windows: &[TopWindow]) -> Vec<AppInfo> {
        group(procs, windows, &tcp, &HashMap::new(), &mut NameCache::default(), &HashMap::new())
    }

    fn group_with_services(procs: &[ProcSample], services: &HashMap<u32, Vec<Service>>) -> Vec<AppInfo> {
        group(procs, &[], &empty_tcp(), &HashMap::new(), &mut NameCache::default(), services)
    }

    fn service(name: &str, display: &str) -> Service {
        Service { name: name.to_string(), display_name: display.to_string() }
    }

    const SVCHOST: &str = r"C:\Windows\System32\svchost.exe";

    /// Every service host is the same executable; the services in it are what tell them apart,
    /// and the name Windows gives the service is what the row says.
    #[test]
    fn service_hosts_are_keyed_and_named_by_their_services() {
        let procs = vec![child(1000, 900, SVCHOST), child(1001, 900, SVCHOST), child(1002, 900, SVCHOST)];
        let mut services = HashMap::new();
        services.insert(1000, vec![service("wuauserv", "Windows 更新")]);
        services.insert(1001, vec![service("WSearch", "Windows Search")]);
        services.insert(1002, vec![service("mpssvc", "Windows Defender Firewall"), service("BFE", "Base Filtering Engine")]);

        let apps = group_with_services(&procs, &services);

        assert_eq!(apps.len(), 3, "three hosts, three rows");
        let update = apps.iter().find(|a| a.pids == vec![1000]).unwrap();
        assert_eq!(update.name, "Windows 更新");
        assert_eq!(update.key, format!("{}|svc:wuauserv", SVCHOST.to_lowercase()));
        assert_eq!(update.kind, AppKind::Service);
        let shared = apps.iter().find(|a| a.pids == vec![1002]).unwrap();
        assert_eq!(shared.key, format!("{}|svc:bfe+mpssvc", SVCHOST.to_lowercase()), "sorted, so the key is stable");
        assert_eq!(shared.services, vec!["Windows Defender Firewall", "Base Filtering Engine"]);
    }

    /// services.exe starts every service host from the same folder. Climbing the tree there
    /// folded every service on the machine into one row named after wininit.
    #[test]
    fn programs_in_the_windows_directory_do_not_fold_into_their_parent() {
        let procs = vec![
            child(600, 500, r"C:\Windows\System32\wininit.exe"),
            child(700, 600, r"C:\Windows\System32\services.exe"),
            child(800, 700, SVCHOST),
            child(810, 800, r"C:\Windows\System32\dllhost.exe"),
        ];
        let apps = group_of(&procs, empty_tcp(), &[]);
        assert_eq!(apps.len(), 4, "each executable is its own row");
    }

    /// The rule the whole engine exists for still holds outside the Windows directory.
    #[test]
    fn helpers_in_an_install_directory_fold_into_the_app() {
        let chrome = r"C:\Program Files\Google\Chrome\Application\chrome.exe";
        let root = ProcSample { can_quit: true, ..child(2000, 1, chrome) };
        let procs = vec![root, child(2001, 2000, chrome), child(2002, 2001, chrome)];
        let apps = group_of(&procs, empty_tcp(), &[]);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].process_count, 3);
        assert_eq!(apps[0].main_pid, 2000, "the process the others fold into");
        assert!(apps[0].can_quit);
    }

    /// Measured on Defender: the engine runs in Session 0 and starts a helper in the user's
    /// session. The helper has the lower pid and is the user's to end, and judging the row by
    /// it called Defender a background app with a working ×.
    #[test]
    fn an_app_is_judged_by_its_root_not_by_a_helper() {
        let platform = r"C:\ProgramData\Microsoft\Windows Defender\Platform\4.18\";
        let engine = ProcSample { session: Some(0), ..child(6180, 2004, &format!("{platform}MsMpEng.exe")) };
        let helper = ProcSample { can_quit: true, ..child(3356, 6180, &format!("{platform}DefenderSessionHelper.exe")) };
        let apps = group_of(&[engine, helper], empty_tcp(), &[]);
        assert_eq!(apps.len(), 1, "the helper folds into the engine");
        assert_eq!(apps[0].kind, AppKind::Service);
        assert!(!apps[0].can_quit, "ending the helper would not end Defender");
    }

    /// The desktop compositor runs in the user's session under an account of its own.
    #[test]
    fn a_windows_component_the_user_cannot_end_is_system() {
        let dwm = child(2396, 1100, r"C:\Windows\System32\dwm.exe");
        assert_eq!(group_of(&[dwm.clone()], empty_tcp(), &[])[0].kind, AppKind::System);
        let mine = ProcSample { can_quit: true, ..child(4000, 1100, r"C:\Windows\System32\ctfmon.exe") };
        assert_eq!(group_of(&[mine], empty_tcp(), &[])[0].kind, AppKind::Background);
    }

    /// Three worktrees each running a dev server were one "Node.js" row with three ports.
    #[test]
    fn runtime_processes_split_by_project() {
        let node = r"C:\Program Files\nodejs\node.exe";
        let in_project = |pid: u32, project: &str| ProcSample { project: Some(project.to_string()), ..child(pid, 1, node) };
        let procs = vec![
            in_project(100, r"E:\work\site"),
            ProcSample { ppid: Some(100), ..in_project(101, r"E:\work\site") },
            in_project(200, r"E:\work\api"),
            child(300, 1, node),
        ];
        let apps = group_of(&procs, empty_tcp(), &[]);
        assert_eq!(apps.len(), 3, "two projects and one node with no project");
        let site = apps.iter().find(|a| a.name == "site · node").expect("named after the project folder");
        assert_eq!(site.process_count, 2);
        assert_eq!(site.project.as_deref(), Some(r"E:\work\site"));
        assert!(site.key.ends_with(r"|project:e:\work\site"));
    }

    /// A row Windows will not let this user end must not offer to end it.
    #[test]
    fn a_group_nobody_may_end_says_so() {
        let apps = group_of(&[child(1000, 900, SVCHOST)], empty_tcp(), &[]);
        assert!(!apps[0].can_quit);
    }

    /// The pseudo processes have no file but a fixed name, and that name keys them, so the
    /// same row comes back after a reboot gave them different pids.
    #[test]
    fn a_process_without_a_file_is_keyed_by_its_name() {
        let apps = group_of(&[child(4, 0, ""), child(3112, 4, "")], empty_tcp(), &[]);
        assert_eq!(apps.len(), 2, "with no name either, each falls back to its pid");
        assert!(apps.iter().any(|a| a.key == "pid:4"));
        // The kernel's table calls it "Memory Compression"; the image-path query says
        // "MemCompression". The table is where the name comes from.
        let named = group_of(&[ProcSample { name: "Memory Compression".into(), ..child(3112, 4, "") }], empty_tcp(), &[]);
        assert_eq!(named[0].key, "image:memory compression");
        assert_eq!(named[0].name, "Memory Compression");
        assert_eq!(named[0].kind, AppKind::System);
    }

    /// Windows never updates a process's parent pid, so a dead parent's number can be handed to
    /// something younger; only an older process can really be the parent.
    #[test]
    fn a_younger_process_wearing_the_parents_pid_is_not_the_parent() {
        use super::super::ntproc::{Identity, NtProcess, Sampled};
        let process = |pid: u32, parent_pid: u32, create_time: u64| NtProcess {
            pid,
            parent_pid,
            name: "x.exe".into(),
            session_id: 1,
            create_time,
            cpu_time: 0,
            working_set: 1,
            private_bytes: 1,
            read_bytes: 0,
            write_bytes: 0,
            threads: 1,
            handles: 1,
        };
        let sampled = |p: NtProcess| Sampled { process: p, identity: Identity::default(), cpu_percent: 0.0 };
        let created: HashMap<u32, u64> = [(10, 500), (20, 100)].into_iter().collect();
        assert_eq!(sample(sampled(process(30, 10, 400)), &created).ppid, None, "pid 10 started after it");
        assert_eq!(sample(sampled(process(31, 20, 400)), &created).ppid, Some(20));
        assert_eq!(sample(sampled(process(32, 99, 400)), &created).ppid, None, "a parent that is gone");
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
        let tcp = TcpOwnership { listening, total_connections: 1, ..Default::default() };

        let apps = group_of(&procs, tcp, &[]);

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].key, "image:svchost.exe");
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
