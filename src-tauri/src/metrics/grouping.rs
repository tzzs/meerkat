//! The grouping engine: turn the wall of processes into the handful of apps the user
//! actually opened.
//!
//! Rule: a process belongs to the nearest ancestor that lives in the same installation
//! directory, so `chrome.exe --type=renderer`, `Code Helper (Plugin).exe` and
//! `msedge.exe --type=utility` fold into the app the user can see. Where Windows itself
//! splits one executable into many roles (`svchost.exe -k NetworkService`), the role is
//! part of the key instead.

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
    pub started_at_ms: Option<u64>,
    pub args: Vec<String>,
    pub measurable: bool,
}

pub fn collect(sys: &System) -> Vec<ProcSample> {
    let mut out = Vec::with_capacity(sys.processes().len());
    for (pid, p) in sys.processes() {
        out.push(sample(pid, p));
    }
    out
}

fn sample(pid: &Pid, p: &Process) -> ProcSample {
    let exe = p
        .exe()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = exe
        .rsplit_once(['\\', '/'])
        .map(|(head, _)| head.to_lowercase())
        .unwrap_or_default();
    let usage = p.disk_usage();
    let started_at_ms = if p.start_time() == 0 {
        None
    } else {
        Some(boot_anchored(p.start_time()))
    };
    let measurable = !exe.is_empty();
    ProcSample {
        pid: pid.as_u32(),
        ppid: p.parent().map(|pp| pp.as_u32()),
        name: p.name().to_string_lossy().to_string(),
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
    }
}

/// sysinfo gives whole seconds since boot; convert once per process list.
fn boot_anchored(seconds_since_boot: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.saturating_sub(seconds_since_boot * 1000)
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
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(
            buffer.cast::<u16>(),
            len.saturating_sub(1) as usize,
        ));
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
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

    for p in procs {
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
            pids: g.pids,
            unmeasured_processes: g.unmeasured,
        });
    }

    out.sort_by(|a, b| {
        b.memory_bytes
            .cmp(&a.memory_bytes)
            .then(b.cpu_percent.total_cmp(&a.cpu_percent))
    });
    out
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
        if current.dir.is_empty() || parent.dir.is_empty() || parent.dir != current.dir {
            break;
        }
        current = parent;
    }

    let exe = current.exe.clone();
    let stem = exe
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(current.name.as_str())
        .trim_end_matches(".exe")
        .to_string();

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
