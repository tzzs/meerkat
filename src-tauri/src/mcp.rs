//! Meerkat for AI assistants, over the Model Context Protocol.
//!
//! `meerkat.exe --mcp` is the program an assistant such as Claude Code starts. It speaks MCP's
//! JSON-RPC on stdin and stdout, and answers each question by asking the Meerkat that is already
//! running, through a named pipe that only this Windows user can open. It never samples anything
//! itself, so what an assistant reads is what the window shows.
//!
//! It is read-only. The one action, quitting an app, is a request: Meerkat brings up its own
//! confirmation, and nothing closes unless the user says so there. And it is off until the user
//! turns it on, because command lines and window titles can carry things worth keeping private,
//! and an assistant may send what it reads to a model elsewhere.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::ipc::AppState;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const MAX_REQUEST: usize = 64 * 1024;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The signed-in user's SID as text (`S-1-5-21-…`). It names the pipe, so each user's Meerkat has
/// its own, and it is the only account the pipe's access list lets in.
fn user_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(8)];
        let fetched = GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), (buffer.len() * 8) as u32, &mut needed);
        let _ = CloseHandle(token);
        fetched.ok()?;
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let sid = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid
    }
}

fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\meerkat-mcp-{sid}")
}

// ---------------------------------------------------------------------------
// The pipe server, inside the running Meerkat
// ---------------------------------------------------------------------------

/// Starts answering on the pipe, once. Called when the setting is on at startup or is turned on;
/// turning it off afterwards makes the server refuse every question rather than tearing it down.
pub fn start(app: AppHandle) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("meerkat-mcp".into()).spawn(move || serve(app));
}

fn serve(app: AppHandle) {
    let Some(sid) = user_sid() else { return };
    let name = wide(&pipe_name(&sid));
    // Full access for this user and nobody else; the default would let every account read.
    let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut descriptor, None) }
        .is_err()
    {
        return;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                4,
                64 * 1024,
                64 * 1024,
                0,
                Some(&attributes),
            )
        };
        if pipe.is_invalid() {
            // Another Meerkat of this user already serves the name; try again later.
            std::thread::sleep(std::time::Duration::from_secs(10));
            continue;
        }
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok() || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            answer_client(&app, pipe);
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
    }
}

fn answer_client(app: &AppHandle, pipe: HANDLE) {
    let Some(request) = read_line(pipe) else { return };
    let response = match serde_json::from_slice::<Value>(&request) {
        Ok(request) => dispatch(app, &request),
        Err(error) => json!({ "ok": false, "error": error.to_string() }),
    };
    let mut bytes = serde_json::to_vec(&response).unwrap_or_default();
    bytes.push(b'\n');
    write_all(pipe, &bytes);
    let _ = unsafe { FlushFileBuffers(pipe) };
}

fn read_line(pipe: HANDLE) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    let mut chunk = [0u8; 4096];
    while line.len() < MAX_REQUEST {
        let mut read = 0u32;
        if unsafe { ReadFile(pipe, Some(&mut chunk), Some(&mut read), None) }.is_err() || read == 0 {
            break;
        }
        line.extend_from_slice(&chunk[..read as usize]);
        if let Some(end) = line.iter().position(|b| *b == b'\n') {
            line.truncate(end);
            return Some(line);
        }
    }
    (!line.is_empty()).then_some(line)
}

fn write_all(pipe: HANDLE, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let mut written = 0u32;
        if unsafe { WriteFile(pipe, Some(bytes), Some(&mut written), None) }.is_err() || written == 0 {
            return;
        }
        bytes = &bytes[written as usize..];
    }
}

fn dispatch(app: &AppHandle, request: &Value) -> Value {
    let state = app.state::<AppState>();
    if !state.mcp_enabled.load(Ordering::Relaxed) {
        return json!({ "ok": false, "error": "AI assistant access is turned off in Meerkat (Settings → AI assistants)." });
    }
    let args = &request["args"];
    let result = match request["tool"].as_str().unwrap_or("") {
        "summary" => summary(&state),
        "apps" => apps(&state, args),
        "port" => port(&state, args),
        "app" => app_detail(&state, args),
        "history" => history(&state, args),
        "events" => events(args),
        "request_quit" => request_quit(app, &state, args),
        other => Err(format!("unknown tool {other}")),
    };
    match result {
        Ok(value) => json!({ "ok": true, "result": value }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

fn gb(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{value:.0} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

/// Process CPU is kept per core inside Meerkat (two busy cores read 200%); an assistant is told
/// the share of the whole machine, which is the number people mean.
fn machine_share(cpu_percent: f64, cores: usize) -> f64 {
    (cpu_percent / cores.max(1) as f64 * 10.0).round() / 10.0
}

fn app_json(app: &crate::models::AppInfo, cores: usize) -> Value {
    json!({
        "name": app.name,
        "key": app.key,
        "kind": format!("{:?}", app.kind).to_lowercase(),
        "processes": app.process_count,
        "cpuPercentOfMachine": machine_share(app.cpu_percent, cores),
        "memory": gb(app.memory_bytes as f64),
        "memoryBytes": app.memory_bytes,
        "diskPerSecond": gb(app.read_bytes_per_second + app.write_bytes_per_second) + "/s",
        "connections": app.connections,
        "listeningPorts": app.listening_ports,
        "project": app.project,
        "services": app.services,
        "path": app.exe_path,
        "paused": app.control.paused,
        "efficiencyMode": app.control.efficiency,
    })
}

fn latest(state: &AppState) -> Result<crate::models::Snapshot, String> {
    state.latest.read().unwrap_or_else(|p| p.into_inner()).clone().ok_or_else(|| "Meerkat has no reading yet".to_string())
}

fn summary(state: &AppState) -> Result<Value, String> {
    let s = latest(state)?;
    let cores = s.machine.logical_cores;
    let mut by_cpu: Vec<&crate::models::AppInfo> = s.apps.iter().collect();
    by_cpu.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
    let mut by_memory: Vec<&crate::models::AppInfo> = s.apps.iter().collect();
    by_memory.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes));
    let insights: Vec<Value> = state
        .watcher
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .list()
        .into_iter()
        .filter(|i| !i.resolved)
        .take(10)
        .map(|i| json!({ "app": i.app_name, "kind": format!("{:?}", i.kind), "sustainedSeconds": i.sustained_seconds }))
        .collect();
    Ok(json!({
        "machine": {
            "cpu": s.machine.chip_name,
            "logicalCores": cores,
            "memory": gb(s.machine.total_memory_bytes as f64),
            "os": s.machine.os_version,
            "uptimeHours": (s.machine.uptime_seconds as f64 / 3600.0 * 10.0).round() / 10.0,
        },
        "now": {
            "cpuPercent": (s.cpu.total_percent * 10.0).round() / 10.0,
            "memoryInUse": gb(s.memory.in_use_bytes as f64),
            "memoryPressurePercent": s.memory.pressure_percent,
            "commitPercent": (s.memory.committed_bytes as f64 / s.memory.commit_limit_bytes.max(1) as f64 * 100.0).round(),
            "diskReadPerSecond": gb(s.disk.read_bytes_per_second) + "/s",
            "diskWritePerSecond": gb(s.disk.write_bytes_per_second) + "/s",
            "downloadPerSecond": gb(s.network.download_bytes_per_second) + "/s",
            "uploadPerSecond": gb(s.network.upload_bytes_per_second) + "/s",
            "gpuPercent": s.gpu.total_percent,
            "hottestCelsius": s.temperature.hottest,
        },
        "busiestByCpu": by_cpu.iter().take(5).map(|a| app_json(a, cores)).collect::<Vec<_>>(),
        "busiestByMemory": by_memory.iter().take(5).map(|a| app_json(a, cores)).collect::<Vec<_>>(),
        "alertsNotDismissed": insights,
    }))
}

fn apps(state: &AppState, args: &Value) -> Result<Value, String> {
    let s = latest(state)?;
    let cores = s.machine.logical_cores;
    let limit = args["limit"].as_u64().unwrap_or(15).clamp(1, 100) as usize;
    let include_system = args["include_system"].as_bool().unwrap_or(true);
    let mut list: Vec<&crate::models::AppInfo> = s
        .apps
        .iter()
        .filter(|a| include_system || !matches!(a.kind, crate::models::AppKind::System | crate::models::AppKind::Service))
        .collect();
    match args["sort_by"].as_str().unwrap_or("cpu") {
        "memory" => list.sort_by(|a, b| b.memory_bytes.cmp(&a.memory_bytes)),
        "disk" => list.sort_by(|a, b| {
            (b.read_bytes_per_second + b.write_bytes_per_second).total_cmp(&(a.read_bytes_per_second + a.write_bytes_per_second))
        }),
        _ => list.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent)),
    }
    Ok(json!({ "apps": list.iter().take(limit).map(|a| app_json(a, cores)).collect::<Vec<_>>(), "totalApps": s.apps.len() }))
}

fn port(state: &AppState, args: &Value) -> Result<Value, String> {
    let wanted = args["port"].as_u64().ok_or("port is required")? as u16;
    let s = latest(state)?;
    let processes = state.processes.read().unwrap_or_else(|p| p.into_inner());
    let holders: Vec<Value> = s
        .network
        .listening
        .iter()
        .filter(|l| l.port == wanted)
        .map(|l| {
            let app = s.apps.iter().find(|a| a.key == l.app_key);
            let process = processes.iter().find(|p| p.pid == l.pid);
            json!({
                "address": l.address,
                "reachableFromNetwork": l.address == "0.0.0.0" || l.address == "::",
                "pid": l.pid,
                "app": app.map(|a| a.name.clone()),
                "project": app.and_then(|a| a.project.clone()),
                "process": process.map(|p| p.name.clone()),
                "commandLine": process.map(|p| redact(&p.args)),
                "canQuit": app.is_some_and(|a| a.can_quit),
            })
        })
        .collect();
    if holders.is_empty() {
        return Ok(json!({ "port": wanted, "listening": false }));
    }
    Ok(json!({ "port": wanted, "listening": true, "holders": holders }))
}

/// The app a free-text query means: its key exactly, else a name, file or project containing it,
/// the busiest first.
fn find_app<'a>(snapshot: &'a crate::models::Snapshot, query: &str) -> Option<(&'a crate::models::AppInfo, Vec<String>)> {
    if let Some(app) = snapshot.apps.iter().find(|a| a.key == query) {
        return Some((app, Vec::new()));
    }
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let mut matches: Vec<&crate::models::AppInfo> = snapshot
        .apps
        .iter()
        .filter(|a| {
            a.name.to_lowercase().contains(&needle)
                || a.exe_path.to_lowercase().rsplit('\\').next().is_some_and(|file| file.contains(&needle))
                || a.project.as_ref().is_some_and(|p| p.to_lowercase().contains(&needle))
        })
        .collect();
    matches.sort_by(|a, b| {
        (b.name.to_lowercase() == needle).cmp(&(a.name.to_lowercase() == needle)).then(b.memory_bytes.cmp(&a.memory_bytes))
    });
    let first = *matches.first()?;
    Some((first, matches.iter().skip(1).take(5).map(|a| a.name.clone()).collect()))
}

fn app_detail(state: &AppState, args: &Value) -> Result<Value, String> {
    let query = args["query"].as_str().ok_or("query is required")?;
    let s = latest(state)?;
    let (app, others) = find_app(&s, query).ok_or_else(|| format!("no running app matches {query:?}"))?;
    let pids: HashSet<u32> = app.pids.iter().copied().collect();
    let processes: Vec<Value> = state
        .processes
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|p| pids.contains(&p.pid))
        .take(40)
        .map(|p| {
            json!({
                "pid": p.pid,
                "name": p.name,
                "commandLine": redact(&p.args),
                "cpuPercentOfMachine": machine_share(p.cpu_percent, s.machine.logical_cores),
                "memory": gb(p.memory_bytes as f64),
            })
        })
        .collect();
    let now = crate::history::now_seconds();
    let day = state
        .app_history
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .ranking(now.saturating_sub(86_400), now + 60, crate::apphistory::RankBy::Cpu, 400)
        .into_iter()
        .find(|u| u.key == app.key);
    let mut out = app_json(app, s.machine.logical_cores);
    out["windows"] = json!(app.windows.iter().filter(|w| w.visible).map(|w| w.title.clone()).collect::<Vec<_>>());
    out["processList"] = json!(processes);
    out["last24Hours"] = match day {
        Some(u) => json!({
            "cpuTimeMinutes": (u.cpu_seconds / 60.0 * 10.0).round() / 10.0,
            "averageMemory": gb(u.memory_average),
            "peakMemory": gb(u.memory_peak),
            "written": gb(u.write_bytes),
            "observedHours": (u.seconds / 3600.0 * 10.0).round() / 10.0,
        }),
        None => Value::Null,
    };
    if !others.is_empty() {
        out["otherMatches"] = json!(others);
    }
    Ok(out)
}

fn history(state: &AppState, args: &Value) -> Result<Value, String> {
    let hours = args["hours"].as_f64().unwrap_or(24.0).clamp(0.1, 720.0);
    let by = match args["by"].as_str().unwrap_or("cpu") {
        "memory" => crate::apphistory::RankBy::Memory,
        "disk_writes" => crate::apphistory::RankBy::Write,
        "gpu" => crate::apphistory::RankBy::Gpu,
        _ => crate::apphistory::RankBy::Cpu,
    };
    let now = crate::history::now_seconds();
    let ranking = state
        .app_history
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .ranking(now.saturating_sub((hours * 3600.0) as u64), now + 60, by, 15);
    Ok(json!({
        "hours": hours,
        "apps": ranking.iter().map(|u| json!({
            "name": u.name,
            "kind": u.kind,
            "cpuTimeMinutes": (u.cpu_seconds / 60.0 * 10.0).round() / 10.0,
            "averageMemory": gb(u.memory_average),
            "peakMemory": gb(u.memory_peak),
            "written": gb(u.write_bytes),
            "gpuTimeMinutes": (u.gpu_seconds / 60.0 * 10.0).round() / 10.0,
            "observedHours": (u.seconds / 3600.0 * 10.0).round() / 10.0,
        })).collect::<Vec<_>>(),
        "note": "Only time Meerkat was running is counted.",
    }))
}

fn events(args: &Value) -> Result<Value, String> {
    let hours = args["hours"].as_f64().unwrap_or(24.0).clamp(0.1, 24.0 * 90.0);
    let problems_only = args["problems_only"].as_bool().unwrap_or(true);
    let routine = |kind: &crate::events::SystemEventKind| {
        use crate::events::SystemEventKind::*;
        matches!(kind, Sleep | Wake | Boot | Shutdown | UpdateInstalled)
    };
    let list: Vec<Value> = crate::events::recent((hours * 3_600_000.0) as u64, 500)
        .into_iter()
        .filter(|e| !problems_only || !routine(&e.kind))
        .take(100)
        .map(|e| json!({ "atUnixMs": e.at_ms, "kind": e.kind, "subject": e.subject, "detail": e.detail }))
        .collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for e in &list {
        *counts.entry(e["kind"].as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    Ok(json!({ "hours": hours, "counts": counts, "events": list }))
}

fn request_quit(app: &AppHandle, state: &AppState, args: &Value) -> Result<Value, String> {
    let query = args["query"].as_str().ok_or("query is required")?;
    let s = latest(state)?;
    let (target, _) = find_app(&s, query).ok_or_else(|| format!("no running app matches {query:?}"))?;
    if !target.can_quit {
        return Err(format!("{} belongs to Windows or another account; Meerkat cannot end it.", target.name));
    }
    // The window comes up with its own confirmation; the user decides there.
    crate::menu::show_main(app, Some(&format!("quit:{}", target.key)));
    Ok(json!({
        "asked": true,
        "app": target.name,
        "note": "Meerkat is asking the user in its own window. Nothing is closed unless they confirm there.",
    }))
}

/// Command lines are where tokens and passwords end up (`--token=…`, `https://user:pass@…`).
/// Values that look like secrets are replaced before anything is handed to an assistant.
pub fn redact(args: &[String]) -> String {
    const SECRET_WORDS: [&str; 8] = ["token", "password", "passwd", "secret", "apikey", "api-key", "api_key", "auth"];
    let secretish = |name: &str| {
        let lower = name.trim_start_matches('-').to_lowercase();
        SECRET_WORDS.iter().any(|word| lower.contains(word))
    };
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for arg in args {
        if hide_next {
            out.push("***".to_string());
            hide_next = false;
            continue;
        }
        let mut text = arg.clone();
        if let Some((name, _)) = arg.split_once('=') {
            if secretish(name) {
                text = format!("{name}=***");
            }
        } else if arg.starts_with('-') && secretish(arg) {
            hide_next = true;
        }
        // user:password@host in a URL.
        if let Some(scheme) = text.find("://") {
            let rest = &text[scheme + 3..];
            if let (Some(at), Some(colon)) = (rest.find('@'), rest.find(':')) {
                if colon < at {
                    text = format!("{}{}:***{}", &text[..scheme + 3], &rest[..colon], &rest[at..]);
                }
            }
        }
        // ?token=… and &key=… in a query string.
        for word in ["token=", "key=", "secret=", "password="] {
            // ASCII lowering keeps byte offsets where they are, which a full lowering of
            // non-Latin text would not.
            if let Some(at) = text.to_ascii_lowercase().find(word) {
                let start = at + word.len();
                let end = text[start..].find(['&', ' ', '"']).map_or(text.len(), |e| start + e);
                text.replace_range(start..end, "***");
            }
        }
        out.push(text);
    }
    out.join(" ")
}

// ---------------------------------------------------------------------------
// The stdio side: what the assistant starts
// ---------------------------------------------------------------------------

/// Asks the running Meerkat one question over the pipe.
fn ask(tool: &str, args: &Value) -> Result<Value, String> {
    let sid = user_sid().ok_or("cannot read this user's identity")?;
    let name = wide(&pipe_name(&sid));
    let mut pipe = HANDLE::default();
    for _ in 0..5 {
        match unsafe {
            CreateFileW(PCWSTR(name.as_ptr()), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None)
        } {
            Ok(handle) => {
                pipe = handle;
                break;
            }
            Err(error) if error.code() == ERROR_PIPE_BUSY.to_hresult() => {
                let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 2_000) };
            }
            Err(_) => {
                return Err("Meerkat is not running, or 'Let AI assistants read Meerkat' is off in its Settings. \
                            (Meerkat 没有运行，或者设置里没有打开「允许 AI 助手读取」。)"
                    .into())
            }
        }
    }
    if pipe.is_invalid() {
        return Err("Meerkat is busy; try again.".into());
    }
    let mut request = serde_json::to_vec(&json!({ "tool": tool, "args": args })).unwrap_or_default();
    request.push(b'\n');
    write_all(pipe, &request);
    let mut response = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let mut read = 0u32;
        if unsafe { ReadFile(pipe, Some(&mut chunk), Some(&mut read), None) }.is_err() || read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read as usize]);
        if response.ends_with(b"\n") {
            break;
        }
    }
    let _ = unsafe { CloseHandle(pipe) };
    let value: Value = serde_json::from_slice(response.trim_ascii()).map_err(|e| format!("bad answer from Meerkat: {e}"))?;
    if value["ok"].as_bool() == Some(true) {
        Ok(value["result"].clone())
    } else {
        Err(value["error"].as_str().unwrap_or("Meerkat refused").to_string())
    }
}

fn tools() -> Value {
    json!([
        {
            "name": "meerkat_summary",
            "description": "What this Windows PC is doing right now: CPU, memory, disk, network, GPU and temperature, the busiest apps by CPU and by memory, and alerts Meerkat has raised. Processes are grouped by app.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "meerkat_apps",
            "description": "List running apps (processes grouped by app, Windows services named), sorted by CPU, memory or disk. CPU is a percent of the whole machine.",
            "inputSchema": { "type": "object", "properties": {
                "sort_by": { "type": "string", "enum": ["cpu", "memory", "disk"] },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                "include_system": { "type": "boolean", "description": "Include Windows services and system processes (default true)." }
            } }
        },
        {
            "name": "meerkat_port",
            "description": "Which app and process is listening on a TCP port, whether it is reachable from the network, the project folder of a dev server, and its command line (secrets redacted).",
            "inputSchema": { "type": "object", "properties": { "port": { "type": "integer", "minimum": 1, "maximum": 65535 } }, "required": ["port"] }
        },
        {
            "name": "meerkat_app",
            "description": "Details of one running app: its processes and their command lines (secrets redacted), windows, ports, project folder, and what it used over the last 24 hours.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "App name, part of its file name, or its project folder." } }, "required": ["query"] }
        },
        {
            "name": "meerkat_history",
            "description": "The heaviest apps over the last N hours (up to 30 days) by CPU time, average memory, disk writes or GPU time, from Meerkat's own per-minute history.",
            "inputSchema": { "type": "object", "properties": {
                "hours": { "type": "number", "minimum": 0.1, "maximum": 720 },
                "by": { "type": "string", "enum": ["cpu", "memory", "disk_writes", "gpu"] }
            } }
        },
        {
            "name": "meerkat_events",
            "description": "System events from Windows' logs: app crashes and hangs, blue screens, unexpected shutdowns, graphics driver resets, hardware errors, failed updates, service crashes; optionally sleep, wake and boot too.",
            "inputSchema": { "type": "object", "properties": {
                "hours": { "type": "number", "minimum": 0.1, "maximum": 2160 },
                "problems_only": { "type": "boolean", "description": "Leave out sleep, wake, boot, shutdown and installed updates (default true)." }
            } }
        },
        {
            "name": "meerkat_request_quit",
            "description": "Ask the user, in Meerkat's own window, whether to quit an app. Nothing is closed unless the user confirms there; this returns as soon as the question is shown.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        }
    ])
}

fn call_tool(params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let tool = match name {
        "meerkat_summary" => "summary",
        "meerkat_apps" => "apps",
        "meerkat_port" => "port",
        "meerkat_app" => "app",
        "meerkat_history" => "history",
        "meerkat_events" => "events",
        "meerkat_request_quit" => "request_quit",
        _ => return json!({ "content": [{ "type": "text", "text": format!("unknown tool {name}") }], "isError": true }),
    };
    match ask(tool, &args) {
        Ok(value) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }] }),
        Err(error) => json!({ "content": [{ "type": "text", "text": error }], "isError": true }),
    }
}

/// The MCP versions this server knows. The tool surface it uses is the same in all of them.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub fn reply_to(message: &Value) -> Option<Value> {
    // A notification has no id and gets no answer.
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => {
            let asked = message["params"]["protocolVersion"].as_str().unwrap_or("");
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "meerkat", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Meerkat is the system monitor running on this Windows PC. Ask it what is using the CPU, memory, disk or a port, what an app did over the last days, and what Windows logged (crashes, blue screens, driver resets). It cannot close anything by itself: meerkat_request_quit only asks the user."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(call_tool(&message["params"])),
        _ => Err(json!({ "code": -32601, "message": format!("Method not found: {method}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// `meerkat.exe --mcp`: one JSON-RPC message per line in, one per line out, until stdin closes.
pub fn run_stdio() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => reply_to(&message),
            Err(error) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": error.to_string() } })),
        };
        if let Some(reply) = reply {
            if writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn secrets_in_command_lines_are_replaced() {
        assert_eq!(redact(&args(&["app.exe", "--token=abc123", "--port", "80"])), "app.exe --token=*** --port 80");
        assert_eq!(redact(&args(&["app.exe", "--password", "hunter2", "x"])), "app.exe --password *** x");
        assert_eq!(redact(&args(&["git", "https://me:pat_123@github.com/o/r"])), "git https://me:***@github.com/o/r");
        assert_eq!(redact(&args(&["curl", "https://api.x/y?key=SECRET&page=2"])), "curl https://api.x/y?key=***&page=2");
        assert_eq!(redact(&args(&["node.exe", "vite.js", "--port", "1420"])), "node.exe vite.js --port 1420", "nothing to hide");
    }

    #[test]
    fn the_handshake_and_the_tool_list_answer() {
        let init = reply_to(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } })).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26", "the client's version when it is one we know");
        let unknown = reply_to(&json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } })).unwrap();
        assert_eq!(unknown["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        let list = reply_to(&json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" })).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"meerkat_port") && names.contains(&"meerkat_request_quit"));
        assert!(reply_to(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none(), "notifications get no answer");
        assert_eq!(reply_to(&json!({ "jsonrpc": "2.0", "id": 4, "method": "nope" })).unwrap()["error"]["code"], -32601);
    }

    #[test]
    fn the_pipe_is_named_after_this_user() {
        let sid = user_sid().expect("the current user has a SID");
        assert!(sid.starts_with("S-1-"));
        assert!(pipe_name(&sid).starts_with(r"\\.\pipe\meerkat-mcp-S-1-"));
    }

    /// With no Meerkat running the tools answer with an explanation, not a hang or a crash.
    #[test]
    fn a_question_with_nobody_to_answer_it_says_so() {
        let reply = call_tool(&json!({ "name": "meerkat_summary", "arguments": {} }));
        // Either a real Meerkat of this user answered, or the explanation came back.
        if reply["isError"] == true {
            assert!(reply["content"][0]["text"].as_str().unwrap().contains("Meerkat"));
        }
    }
}
