//! Performance sessions as one state machine: idle -> recording -> reported. One store owns the
//! sessions file, so the sampler and IPC commands cannot interleave writes to it.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

use crate::ipc::AppState;
use crate::models::{
    AppInfo, GpuAdapter, PerfIssue, PerfIssueKind, PerfReport, PerfSample, PerfSession,
    PerfSessionSummary, PerfStat, PerfStatus, PerfTarget, PerfTargetMode, PerfVerdict, Snapshot,
};

const FILE_NAME: &str = "perf-sessions.json";
const MAX_SESSIONS: usize = 30;
const FLUSH_EVERY: Duration = Duration::from_secs(10);

const FPS_LOW_AVG: f64 = 50.0;
const FPS_LOW_PERCENT: f64 = 45.0;
const FPS_LOW_SHARE: f64 = 0.20;
const FPS_UNSTABLE_SPREAD: f64 = 30.0;
const CPU_BOUND_APP: f64 = 80.0;
const CPU_BOUND_GPU: f64 = 60.0;
const GPU_BOUND_GPU: f64 = 85.0;
const GPU_BOUND_APP: f64 = 50.0;
const VRAM_PRESSURE: f64 = 90.0;
const GPU_HOT: f64 = 83.0;
const MEMORY_PRESSURE: f64 = 90.0;
const MIN_SAMPLES_FOR_VERDICT: usize = 10;

/// The report always carries every series, including empty ones the UI still has to draw.
const STAT_KEYS: [&str; 10] = [
    "fps",
    "cpuPercent",
    "appCpuPercent",
    "cpuTempC",
    "gpuPercent",
    "appGpuPercent",
    "gpuTempC",
    "memoryPercent",
    "vramPercent",
    "vramUsedBytes",
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Recording { id: String },
    Reported { id: String },
}

pub struct Store {
    data_dir: PathBuf,
    phase: Phase,
    sessions: Vec<PerfSession>,
    last_flush: Instant,
}

impl Store {
    pub fn open(data_dir: &Path) -> Self {
        let sessions: Vec<PerfSession> = fs::read(data_dir.join(FILE_NAME))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut store = Self {
            data_dir: data_dir.to_path_buf(),
            phase: Phase::Idle,
            sessions,
            last_flush: Instant::now(),
        };
        let original_len = store.sessions.len();
        store.trim();
        let trimmed = store.sessions.len() != original_len;
        if store.finish_interrupted() || trimmed {
            store.flush();
        }
        store
    }

    /// A session left open by a crash is closed at its last sample, not left active forever.
    fn finish_interrupted(&mut self) -> bool {
        let mut last = None;
        for session in &mut self.sessions {
            if session.ended_at_ms.is_none() {
                let ended = session.samples.last().map(|s| s.at_ms).unwrap_or(session.started_at_ms);
                session.ended_at_ms = Some(ended);
                session.report = Some(analyze(&session.samples, session.id.clone()));
                last = Some(session.id.clone());
            }
        }
        if let Some(id) = last {
            self.phase = Phase::Reported { id };
            true
        } else {
            false
        }
    }

    pub fn start(
        &mut self,
        mode: PerfTargetMode,
        app_key: Option<String>,
        snapshot: &Snapshot,
    ) -> Result<String, String> {
        if matches!(self.phase, Phase::Recording { .. }) {
            return Err("a performance session is already recording".into());
        }
        let target = match mode {
            PerfTargetMode::Foreground => PerfTarget {
                mode,
                app_key: None,
                // The UI supplies the translated mode label when no app could be named.
                name: String::new(),
                current_app_key: None,
            },
            PerfTargetMode::App => {
                let key = app_key.filter(|key| !key.is_empty()).ok_or("choose an application")?;
                let name = snapshot
                    .apps
                    .iter()
                    .find(|app| app.key == key)
                    .ok_or("that application is not running")?
                    .name
                    .clone();
                PerfTarget {
                    mode,
                    app_key: Some(key.clone()),
                    name,
                    current_app_key: Some(key),
                }
            }
        };
        let id = new_id();
        self.sessions.push(PerfSession {
            id: id.clone(),
            target,
            started_at_ms: now_ms(),
            ended_at_ms: None,
            samples: Vec::new(),
            report: None,
        });
        self.trim();
        self.phase = Phase::Recording { id: id.clone() };
        self.flush();
        Ok(id)
    }

    /// Appends one sample per tick while recording. Returns the changed status for live updates.
    pub fn record(&mut self, snapshot: &Snapshot) -> Option<PerfStatus> {
        let id = match &self.phase {
            Phase::Recording { id } => id.clone(),
            _ => return None,
        };
        let index = self.sessions.iter().position(|session| session.id == id)?;
        let (mode, key) = {
            let target = &self.sessions[index].target;
            (target.mode, target.app_key.clone())
        };
        let (app, current_key) = match mode {
            PerfTargetMode::App => (
                key.as_deref().and_then(|key| snapshot.apps.iter().find(|a| &a.key == key)),
                key,
            ),
            PerfTargetMode::Foreground => {
                let foreground = foreground::Window::current();
                let app = foreground.as_ref().and_then(|window| window.app_in(snapshot));
                (app, app.map(|app| app.key.clone()))
            }
        };
        let session = &mut self.sessions[index];
        session.target.current_app_key = current_key;
        session.samples.push(build_sample(snapshot, app, &self.data_dir));
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            self.flush();
        }
        Some(self.status())
    }

    pub fn stop(&mut self) -> Result<PerfReport, String> {
        let id = match &self.phase {
            Phase::Recording { id } => id.clone(),
            _ => return Err("no performance session is recording".into()),
        };
        let index = self.sessions.iter().position(|session| session.id == id).ok_or("session is gone")?;
        self.sessions[index].ended_at_ms = Some(now_ms());
        let report = analyze(&self.sessions[index].samples, id.clone());
        self.sessions[index].report = Some(report.clone());
        self.phase = Phase::Reported { id };
        self.flush();
        Ok(report)
    }

    pub fn status(&self) -> PerfStatus {
        let mut status = PerfStatus::default();
        if let Some(session) = self.current_session() {
            status.active = matches!(self.phase, Phase::Recording { .. });
            status.session_id = Some(session.id.clone());
            status.target_name = session.target.name.clone();
            status.mode = session.target.mode;
            status.current_app_key = session.target.current_app_key.clone();
            status.sample_count = session.samples.len();
            status.started_at_ms = Some(session.started_at_ms);
            status.latest = session.samples.last().cloned();
        }
        status
    }

    pub fn sessions(&self) -> Vec<PerfSessionSummary> {
        self.sessions
            .iter()
            .rev()
            .map(|session| PerfSessionSummary {
                id: session.id.clone(),
                target_name: session.target.name.clone(),
                mode: session.target.mode,
                started_at_ms: session.started_at_ms,
                ended_at_ms: session.ended_at_ms,
                sample_count: session.samples.len(),
                verdict: session.report.as_ref().map(|report| report.verdict).unwrap_or(PerfVerdict::Inconclusive),
            })
            .collect()
    }

    pub fn session(&self, id: &str) -> Option<PerfSession> {
        self.sessions.iter().find(|session| session.id == id).cloned()
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|session| session.id != id);
        if self.phase_id() == Some(id) {
            self.phase = Phase::Idle;
        }
        let removed = self.sessions.len() != before;
        if removed {
            self.flush();
        }
        removed
    }

    fn current_session(&self) -> Option<&PerfSession> {
        let id = self.phase_id()?;
        self.sessions.iter().find(|session| session.id == id)
    }

    fn phase_id(&self) -> Option<&str> {
        match &self.phase {
            Phase::Idle => None,
            Phase::Recording { id } | Phase::Reported { id } => Some(id),
        }
    }

    fn trim(&mut self) {
        while self.sessions.len() > MAX_SESSIONS {
            let id = self.sessions.remove(0).id;
            if self.phase_id() == Some(&id) {
                self.phase = Phase::Idle;
            }
        }
    }

    pub fn flush(&mut self) {
        let path = self.data_dir.join(FILE_NAME);
        if let Err(error) = write_atomically(&path, &self.sessions) {
            crate::diag::warn(format!("perf sessions: could not be saved: {error}"));
            return;
        }
        self.last_flush = Instant::now();
    }
}

/// Called by the sampler right after the snapshot is published.
pub fn record(state: &AppState, snapshot: &Snapshot) {
    let status = {
        let mut store = state.perf.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        store.record(snapshot)
    };
    if let (Some(status), Some(app)) = (status, state.app.get()) {
        let _ = app.emit("perf-status", &status);
    }
}

/// Toggle foreground monitoring from the global hotkey.
pub fn toggle_foreground(app: &AppHandle, state: &AppState) {
    let snapshot = state.latest.read().unwrap_or_else(|p| p.into_inner()).clone();
    let status = {
        let mut store = state.perf.lock().unwrap_or_else(|p| p.into_inner());
        if store.status().active {
            if let Err(error) = store.stop() {
                crate::diag::warn(format!("hotkey stop: {error}"));
                return;
            }
        } else if let Some(snapshot) = snapshot.as_ref() {
            if let Err(error) = store.start(PerfTargetMode::Foreground, None, snapshot) {
                crate::diag::warn(format!("hotkey start: {error}"));
                return;
            }
        } else {
            return;
        }
        store.status()
    };
    let _ = app.emit("perf-status", &status);
}

mod foreground {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    use crate::models::{AppInfo, Snapshot};

    pub struct Window {
        pid: u32,
        hwnd: isize,
    }

    impl Window {
        pub fn current() -> Option<Self> {
            let hwnd = unsafe { GetForegroundWindow() };
            if hwnd.0.is_null() {
                return None;
            }
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            (pid != 0).then_some(Self { pid, hwnd: hwnd.0 as isize })
        }

        pub fn app_in<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a AppInfo> {
            snapshot
                .apps
                .iter()
                .find(|app| app.windows.iter().any(|window| window.hwnd == self.hwnd))
                .or_else(|| {
                    snapshot
                        .apps
                        .iter()
                        .find(|app| app.main_pid == self.pid || app.pids.contains(&self.pid))
                })
        }
    }
}

fn build_sample(snapshot: &Snapshot, app: Option<&AppInfo>, data_dir: &Path) -> PerfSample {
    let (fps, fps_source) = crate::fps::last_fps(data_dir);
    let vram = snapshot
        .gpu
        .adapters
        .iter()
        .filter(|adapter| adapter.dedicated_total_bytes.is_some_and(|bytes| bytes > 0))
        .max_by_key(|adapter| adapter.dedicated_total_bytes.unwrap_or(0));
    let gpu_temp_c = snapshot.temperature.gpu_celsius.or_else(|| vram.and_then(|adapter| adapter.temperature_c));
    let pids: HashSet<u32> = app
        .map(|app| app.pids.iter().copied().chain(std::iter::once(app.main_pid)).collect())
        .unwrap_or_default();
    let app_gpu_percent = app.map(|_| {
        snapshot
            .gpu
            .per_process
            .iter()
            .filter(|process| pids.contains(&process.pid))
            .map(|process| process.percent)
            .sum::<f64>()
            // PDH engine rows can carry a tiny negative remainder after a reset.
            .max(0.0)
    });
    let vram_percent = vram.and_then(vram_percent);
    PerfSample {
        at_ms: snapshot.taken_at_ms,
        fps,
        fps_source,
        cpu_percent: snapshot.cpu.total_percent,
        app_cpu_percent: app.map(|app| app.cpu_percent),
        cpu_temp_c: snapshot.temperature.cpu_celsius,
        gpu_percent: snapshot.gpu.total_percent,
        app_gpu_percent,
        gpu_temp_c,
        memory_percent: snapshot.memory.pressure_percent,
        app_memory_bytes: app.map(|app| app.memory_bytes),
        vram_percent,
        vram_used_bytes: vram.and_then(|adapter| adapter.dedicated_used_bytes),
        vram_total_bytes: vram.and_then(|adapter| adapter.dedicated_total_bytes),
    }
}

fn vram_percent(adapter: &GpuAdapter) -> Option<f64> {
    let used = adapter.dedicated_used_bytes? as f64;
    let total = adapter.dedicated_total_bytes? as f64;
    (total > 0.0).then_some(used / total * 100.0)
}

/// Pure post-session analysis. Nothing here reads a snapshot or touches a file.
pub fn analyze(samples: &[PerfSample], session_id: String) -> PerfReport {
    let stats: Vec<PerfStat> = STAT_KEYS.iter().map(|key| stat(key, samples)).collect();
    let stat = |key: &str| stats.iter().find(|item| item.key == key).expect("every stat key is present");
    let fps = stat("fps");
    let app_cpu = stat("appCpuPercent");
    let gpu = stat("gpuPercent");
    let gpu_temp = stat("gpuTempC");
    let vram = stat("vramPercent");
    let memory = stat("memoryPercent");

    let mut issues = Vec::new();
    let low_share = if samples.is_empty() {
        0.0
    } else {
        samples
            .iter()
            .filter(|sample| sample.fps.is_some_and(|fps| fps.is_finite() && fps < FPS_LOW_PERCENT))
            .count() as f64
            / samples.len() as f64
    };
    if fps.avg.is_some_and(|avg| avg < FPS_LOW_AVG) {
        issues.push(issue(PerfIssueKind::FpsLow, "fps", fps.avg));
    } else if low_share > FPS_LOW_SHARE {
        issues.push(issue(PerfIssueKind::FpsLow, "fps", Some(low_share * 100.0)));
    }
    if let (Some(p95), Some(min)) = (fps.p95, fps.min) {
        if p95 - min > FPS_UNSTABLE_SPREAD {
            issues.push(issue(PerfIssueKind::FpsUnstable, "fps", Some(p95 - min)));
        }
    }
    if app_cpu.avg.is_some_and(|avg| avg >= CPU_BOUND_APP) && gpu.avg.is_some_and(|avg| avg <= CPU_BOUND_GPU) {
        issues.push(issue(PerfIssueKind::CpuBound, "appCpuPercent", app_cpu.avg));
    }
    if gpu.avg.is_some_and(|avg| avg >= GPU_BOUND_GPU) && app_cpu.avg.is_some_and(|avg| avg <= GPU_BOUND_APP) {
        issues.push(issue(PerfIssueKind::GpuBound, "gpuPercent", gpu.avg));
    }
    if vram.avg.is_some_and(|avg| avg >= VRAM_PRESSURE) {
        issues.push(issue(PerfIssueKind::VramPressure, "vramPercent", vram.avg));
    }
    if gpu_temp.max.is_some_and(|max| max >= GPU_HOT) {
        issues.push(issue(PerfIssueKind::GpuHot, "gpuTempC", gpu_temp.max));
    }
    if memory.avg.is_some_and(|avg| avg >= MEMORY_PRESSURE) {
        issues.push(issue(PerfIssueKind::MemoryPressure, "memoryPercent", memory.avg));
    }
    if !fps.available {
        issues.push(issue(PerfIssueKind::FpsUnavailable, "fps", None));
    }

    let verdict = if issues.iter().any(|finding| finding.verdict == PerfVerdict::Problem) {
        PerfVerdict::Problem
    } else if issues.iter().any(|finding| finding.verdict == PerfVerdict::Warning) {
        PerfVerdict::Warning
    } else if samples.len() < MIN_SAMPLES_FOR_VERDICT
        || issues.iter().any(|finding| finding.verdict == PerfVerdict::Inconclusive)
    {
        PerfVerdict::Inconclusive
    } else {
        PerfVerdict::Healthy
    };
    let duration_seconds = samples
        .first()
        .zip(samples.last())
        .map(|(first, last)| last.at_ms.saturating_sub(first.at_ms) / 1000)
        .unwrap_or(0);
    PerfReport {
        session_id,
        verdict,
        headline_key: match verdict {
            PerfVerdict::Healthy => "perf.headline.healthy",
            PerfVerdict::Warning => "perf.headline.warning",
            PerfVerdict::Problem => "perf.headline.problem",
            PerfVerdict::Inconclusive => "perf.headline.inconclusive",
        }
        .into(),
        issues,
        stats,
        duration_seconds,
        sample_count: samples.len(),
    }
}

fn issue(kind: PerfIssueKind, metric: &str, value: Option<f64>) -> PerfIssue {
    use PerfIssueKind::*;
    let (verdict, message_key) = match kind {
        FpsLow => (PerfVerdict::Problem, "perf.issue.fpsLow"),
        FpsUnstable => (PerfVerdict::Warning, "perf.issue.fpsUnstable"),
        CpuBound => (PerfVerdict::Warning, "perf.issue.cpuBound"),
        GpuBound => (PerfVerdict::Warning, "perf.issue.gpuBound"),
        VramPressure => (PerfVerdict::Problem, "perf.issue.vramPressure"),
        GpuHot => (PerfVerdict::Problem, "perf.issue.gpuHot"),
        MemoryPressure => (PerfVerdict::Problem, "perf.issue.memoryPressure"),
        FpsUnavailable => (PerfVerdict::Inconclusive, "perf.issue.fpsUnavailable"),
    };
    PerfIssue { kind, verdict, message_key: message_key.into(), metric: metric.into(), value }
}

fn stat(key: &str, samples: &[PerfSample]) -> PerfStat {
    let values: Vec<f64> = samples
        .iter()
        .filter_map(|sample| value_of(sample, key))
        .filter(|value| value.is_finite())
        .collect();
    if values.is_empty() {
        return PerfStat { key: key.into(), available: false, min: None, avg: None, max: None, p95: None, last: None };
    }
    let mut sorted = values.clone();
    sorted.sort_by(f64::total_cmp);
    let p95 = sorted[(values.len() * 95).div_ceil(100) - 1];
    PerfStat {
        key: key.into(),
        available: true,
        min: Some(sorted[0]),
        avg: Some(values.iter().sum::<f64>() / values.len() as f64),
        max: Some(sorted[sorted.len() - 1]),
        p95: Some(p95),
        last: values.last().copied(),
    }
}

fn value_of(sample: &PerfSample, key: &str) -> Option<f64> {
    match key {
        "fps" => sample.fps,
        "cpuPercent" => Some(sample.cpu_percent),
        "appCpuPercent" => sample.app_cpu_percent,
        "cpuTempC" => sample.cpu_temp_c,
        "gpuPercent" => sample.gpu_percent,
        "appGpuPercent" => sample.app_gpu_percent,
        "gpuTempC" => sample.gpu_temp_c,
        "memoryPercent" => Some(sample.memory_percent),
        "vramPercent" => sample.vram_percent,
        "vramUsedBytes" => sample.vram_used_bytes.map(|bytes| bytes as f64),
        _ => None,
    }
}

fn write_atomically(path: &Path, sessions: &[PerfSession]) -> Result<(), String> {
    let bytes = serde_json::to_vec(sessions).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp, path).map_err(|error| error.to_string())
}

fn new_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("perf-{nanos:x}")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CpuMetric, FpsSource, GpuMetric, MemoryMetric, TemperatureMetric};

    fn sample() -> PerfSample {
        PerfSample {
            at_ms: 0,
            fps: Some(60.0),
            fps_source: FpsSource::PresentMon,
            cpu_percent: 20.0,
            app_cpu_percent: Some(20.0),
            cpu_temp_c: Some(50.0),
            gpu_percent: Some(30.0),
            app_gpu_percent: Some(10.0),
            gpu_temp_c: Some(60.0),
            memory_percent: 40.0,
            app_memory_bytes: Some(1_000),
            vram_percent: Some(30.0),
            vram_used_bytes: Some(3_000),
            vram_total_bytes: Some(10_000),
        }
    }

    fn repeat(count: usize, change: impl Fn(&mut PerfSample)) -> Vec<PerfSample> {
        (0..count)
            .map(|index| {
                let mut result = sample();
                result.at_ms = index as u64 * 1_000;
                change(&mut result);
                result
            })
            .collect()
    }

    fn kinds(report: &PerfReport) -> Vec<PerfIssueKind> {
        report.issues.iter().map(|finding| finding.kind).collect()
    }

    #[test]
    fn a_healthy_session_has_all_ten_stats_and_no_findings() {
        let report = analyze(&repeat(10, |_| {}), "s1".into());
        assert_eq!(report.verdict, PerfVerdict::Healthy);
        assert_eq!(report.headline_key, "perf.headline.healthy");
        assert!(report.issues.is_empty());
        assert_eq!(
            report.stats.iter().map(|stat| stat.key.as_str()).collect::<Vec<_>>(),
            STAT_KEYS
        );
        assert!(report.stats.iter().all(|stat| stat.available));
        assert_eq!(report.stats[0].avg, Some(60.0));
        assert_eq!(report.stats[9].avg, Some(3_000.0));
        assert_eq!(report.duration_seconds, 9);
    }

    #[test]
    fn an_empty_session_has_unavailable_stats_and_an_inconclusive_verdict() {
        let report = analyze(&[], "s1".into());
        assert_eq!(report.verdict, PerfVerdict::Inconclusive);
        assert_eq!(report.headline_key, "perf.headline.inconclusive");
        assert_eq!(report.stats.len(), 10);
        assert!(report.stats.iter().all(|stat| {
            !stat.available && stat.min.is_none() && stat.avg.is_none()
                && stat.max.is_none() && stat.p95.is_none() && stat.last.is_none()
        }));
        assert!(kinds(&report).contains(&PerfIssueKind::FpsUnavailable));
    }

    #[test]
    fn fewer_than_ten_samples_is_inconclusive() {
        assert_eq!(analyze(&repeat(9, |_| {}), "s".into()).verdict, PerfVerdict::Inconclusive);
    }

    #[test]
    fn a_low_average_frame_rate_is_a_problem() {
        let report = analyze(&repeat(10, |sample| sample.fps = Some(49.0)), "s".into());
        assert!(kinds(&report).contains(&PerfIssueKind::FpsLow));
        assert_eq!(report.verdict, PerfVerdict::Problem);
        assert_eq!(report.stats[0].avg, Some(49.0));
    }

    #[test]
    fn more_than_a_fifth_of_frames_below_45_is_a_problem() {
        let report = analyze(
            &repeat(10, |sample| {
                if sample.at_ms < 3_000 {
                    sample.fps = Some(40.0);
                }
            }),
            "s".into(),
        );
        assert!(kinds(&report).contains(&PerfIssueKind::FpsLow));
    }

    #[test]
    fn exactly_a_fifth_of_frames_below_45_is_not_a_finding() {
        let report = analyze(
            &repeat(10, |sample| {
                if sample.at_ms < 2_000 {
                    sample.fps = Some(40.0);
                }
            }),
            "s".into(),
        );
        assert!(!kinds(&report).contains(&PerfIssueKind::FpsLow));
    }

    #[test]
    fn a_wide_frame_time_spread_is_unstable() {
        let report = analyze(
            &repeat(20, |sample| {
                sample.fps = if sample.at_ms % 2_000 == 0 { Some(60.0) } else { Some(100.0) };
            }),
            "s".into(),
        );
        assert!(kinds(&report).contains(&PerfIssueKind::FpsUnstable));
        assert_eq!(report.verdict, PerfVerdict::Warning);
    }

    #[test]
    fn a_cpu_bound_session_is_reported_when_the_gpu_is_idle() {
        let report = analyze(
            &repeat(10, |sample| {
                sample.app_cpu_percent = Some(85.0);
                sample.gpu_percent = Some(50.0);
            }),
            "s".into(),
        );
        assert!(kinds(&report).contains(&PerfIssueKind::CpuBound));
    }

    #[test]
    fn a_gpu_bound_session_is_reported_when_the_app_is_idle() {
        let report = analyze(
            &repeat(10, |sample| {
                sample.gpu_percent = Some(90.0);
                sample.app_cpu_percent = Some(40.0);
            }),
            "s".into(),
        );
        assert!(kinds(&report).contains(&PerfIssueKind::GpuBound));
    }

    #[test]
    fn sustained_vram_pressure_heat_and_memory_pressure_are_reported() {
        let report = analyze(
            &repeat(10, |sample| {
                sample.vram_percent = Some(95.0);
                sample.gpu_temp_c = Some(85.0);
                sample.memory_percent = 95.0;
                sample.fps = None;
            }),
            "s".into(),
        );
        let found = kinds(&report);
        assert!(found.contains(&PerfIssueKind::VramPressure));
        assert!(found.contains(&PerfIssueKind::GpuHot));
        assert!(found.contains(&PerfIssueKind::MemoryPressure));
        assert!(found.contains(&PerfIssueKind::FpsUnavailable));
        assert_eq!(report.verdict, PerfVerdict::Problem);
    }

    #[test]
    fn no_frame_rate_is_reported_even_when_everything_else_looks_fine() {
        let report = analyze(&repeat(10, |sample| sample.fps = None), "s".into());
        assert!(kinds(&report).contains(&PerfIssueKind::FpsUnavailable));
        assert!(!report.stats[0].available);
        assert_eq!(report.verdict, PerfVerdict::Inconclusive);
    }

    #[test]
    fn a_problem_outranks_a_warning() {
        let report = analyze(
            &repeat(10, |sample| {
                sample.fps = Some(40.0);
                sample.gpu_percent = Some(90.0);
                sample.app_cpu_percent = Some(40.0);
            }),
            "s".into(),
        );
        assert_eq!(report.verdict, PerfVerdict::Problem);
    }

    #[test]
    fn a_second_start_is_refused_and_a_stop_completes_the_state_machine() {
        let dir = std::env::temp_dir().join(format!("meerkat-perf-{}", std::process::id()));
        let mut store = Store::open(&dir);
        let snapshot = test_snapshot();
        let id = store.start(PerfTargetMode::Foreground, None, &snapshot).unwrap();
        assert!(store.start(PerfTargetMode::Foreground, None, &snapshot).is_err());
        assert!(store.record(&snapshot).is_some());
        assert_eq!(store.status().sample_count, 1);
        assert_eq!(store.stop().unwrap().session_id, id);
        assert!(!store.status().active);
        assert!(store.stop().is_err());
        assert_eq!(store.sessions().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sessions_survive_reopening_and_are_capped_at_thirty() {
        let dir = std::env::temp_dir().join(format!("meerkat-perf-reopen-{}", std::process::id()));
        let snapshot = test_snapshot();
        {
            let mut store = Store::open(&dir);
            for _ in 0..32 {
                let id = store.start(PerfTargetMode::Foreground, None, &snapshot).unwrap();
                store.record(&snapshot);
                assert_eq!(store.stop().unwrap().session_id, id);
            }
        }
        let reopened = Store::open(&dir);
        assert_eq!(reopened.sessions().len(), MAX_SESSIONS);
        assert_eq!(reopened.sessions()[0].sample_count, 1);
        assert_eq!(reopened.session(&reopened.sessions()[0].id).unwrap().samples.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unfinished_session_is_closed_when_the_app_comes_back() {
        let dir = std::env::temp_dir().join(format!("meerkat-perf-crash-{}", std::process::id()));
        let snapshot = test_snapshot();
        {
            let mut store = Store::open(&dir);
            store.start(PerfTargetMode::Foreground, None, &snapshot).unwrap();
            store.record(&snapshot);
        }
        let reopened = Store::open(&dir);
        assert!(!reopened.status().active);
        let summary = &reopened.sessions()[0];
        assert!(summary.ended_at_ms.is_some());
        assert_eq!(summary.verdict, PerfVerdict::Inconclusive);
        assert!(reopened.session(&summary.id).unwrap().report.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_app_key_is_refused() {
        let dir = std::env::temp_dir().join(format!("meerkat-perf-key-{}", std::process::id()));
        let mut store = Store::open(&dir);
        assert!(store.start(PerfTargetMode::App, Some("missing".into()), &test_snapshot()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn test_snapshot() -> Snapshot {
        Snapshot {
            taken_at_ms: 1_000,
            machine: crate::models::MachineInfo {
                hostname: "test".into(),
                os_version: "test".into(),
                chip_name: "test".into(),
                physical_cores: None,
                logical_cores: 1,
                total_memory_bytes: 8_000,
                uptime_seconds: 0,
            },
            cpu: CpuMetric::default(),
            memory: MemoryMetric::default(),
            disk: Default::default(),
            network: Default::default(),
            gpu: GpuMetric::default(),
            battery: Default::default(),
            temperature: TemperatureMetric::default(),
            apps: Vec::new(),
            process_count: 0,
            sampler_cost_microseconds: 0,
        }
    }
}
