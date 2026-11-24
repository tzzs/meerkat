//! Data model crossing the IPC boundary. Mirrors `src/lib/types.ts` exactly;
//! both sides use camelCase on the wire.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Availability {
    #[default]
    Ok,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineInfo {
    pub hostname: String,
    pub os_version: String,
    pub chip_name: String,
    pub physical_cores: Option<usize>,
    pub logical_cores: usize,
    pub total_memory_bytes: u64,
    pub uptime_seconds: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuMetric {
    pub total_percent: f64,
    pub user_percent: f64,
    pub privileged_percent: f64,
    pub core_percents: Vec<f64>,
    pub processor_queue_length: Option<f64>,
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetric {
    pub total_bytes: u64,
    pub in_use_bytes: u64,
    pub available_bytes: u64,
    pub cached_bytes: u64,
    pub committed_bytes: u64,
    pub commit_limit_bytes: u64,
    pub pagefile_total_bytes: u64,
    pub pagefile_used_bytes: u64,
    pub paged_pool_bytes: u64,
    pub nonpaged_pool_bytes: u64,
    pub pressure_percent: f64,
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub id: String,
    pub name: String,
    pub mount_point: String,
    pub file_system: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub read_bytes_per_second: f64,
    pub write_bytes_per_second: f64,
    pub is_removable: bool,
    pub is_network: bool,
    pub is_solid_state: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetric {
    pub read_bytes_per_second: f64,
    pub write_bytes_per_second: f64,
    pub read_bytes_today: u64,
    pub write_bytes_today: u64,
    pub volumes: Vec<VolumeInfo>,
    pub history_read: Vec<f64>,
    pub history_write: Vec<f64>,
    pub availability: Availability,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub name: String,
    pub is_up: bool,
    pub link_speed_bits_per_second: Option<u64>,
    pub download_bytes_per_second: f64,
    pub upload_bytes_per_second: f64,
    pub is_loopback: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMetric {
    pub download_bytes_per_second: f64,
    pub upload_bytes_per_second: f64,
    pub download_bytes_today: u64,
    pub upload_bytes_today: u64,
    pub adapters: Vec<AdapterInfo>,
    pub connections: usize,
    pub history_download: Vec<f64>,
    pub history_upload: Vec<f64>,
    /// Every listening TCP socket, with the app that holds it.
    #[serde(default)]
    pub listening: Vec<ListeningPort>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    pub port: u16,
    /// What it is bound to: `0.0.0.0` or `::` takes connections from the network, `127.0.0.1`
    /// or `::1` only from this machine.
    pub address: String,
    pub pid: u32,
    pub app_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuAdapter {
    pub index: u32,
    pub name: String,
    pub dedicated_total_bytes: Option<u64>,
    pub dedicated_used_bytes: Option<u64>,
    pub shared_used_bytes: Option<u64>,
    pub temperature_c: Option<f64>,
    pub fan_percent: Option<f64>,
    pub power_watts: Option<f64>,
    pub utilization_percent: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuProcess {
    pub pid: u32,
    pub percent: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuMetric {
    pub total_percent: Option<f64>,
    pub adapters: Vec<GpuAdapter>,
    pub per_process: Vec<GpuProcess>,
    pub availability: Availability,
    pub history: Vec<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryMetric {
    pub has_battery: bool,
    pub on_ac_power: bool,
    pub charge_percent: Option<f64>,
    pub is_charging: bool,
    pub is_critical: bool,
    pub remaining_seconds: Option<u64>,
    pub power_watts: Option<f64>,
    pub design_capacity_wh: Option<f64>,
    pub full_charge_capacity_wh: Option<f64>,
    pub health_percent: Option<f64>,
    pub cycle_count: Option<u32>,
    pub source: Availability,
    pub history: Vec<f64>,
    /// One point per week from Windows' own battery report, back to when it started keeping
    /// one: how much the battery holds when full, against what it held new.
    pub capacity_history: Vec<CapacityPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapacityPoint {
    pub at_ms: u64,
    pub design_wh: f64,
    pub full_charge_wh: f64,
    pub cycle_count: Option<u32>,
}

/// One process of an app, for the app's detail view.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub exe_path: String,
    pub command_line: Vec<String>,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub private_bytes: u64,
    pub read_bytes_total: u64,
    pub write_bytes_total: u64,
    pub threads: u32,
    pub handles: u32,
    pub started_at_ms: Option<u64>,
    pub can_quit: bool,
}

/// Bytes moved on one local calendar day while Meerkat was running.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DailyTotal {
    /// Local midnight that starts the day, as Unix milliseconds.
    pub day_start_ms: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub download_bytes: u64,
    pub upload_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorReading {
    pub id: String,
    pub label: String,
    pub celsius: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemperatureMetric {
    pub cpu_celsius: Option<f64>,
    pub gpu_celsius: Option<f64>,
    pub hottest: Option<f64>,
    pub sensors: Vec<SensorReading>,
    pub availability: Availability,
    pub history: Vec<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppKind {
    #[default]
    App,
    Background,
    Service,
    System,
    Packaged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    pub title: String,
    pub pid: u32,
    pub visible: bool,
    /// HWND as a number; the UI never reads it, quit uses it to post WM_CLOSE.
    pub hwnd: isize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub key: String,
    pub name: String,
    pub exe_path: String,
    pub kind: AppKind,
    pub main_pid: u32,
    pub process_count: usize,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub read_bytes_per_second: f64,
    pub write_bytes_per_second: f64,
    pub read_bytes_total: u64,
    pub write_bytes_total: u64,
    pub connections: usize,
    pub listening_ports: Vec<u16>,
    pub oldest_started_at_ms: Option<u64>,
    pub windows: Vec<WindowInfo>,
    pub pids: Vec<u32>,
    pub unmeasured_processes: usize,
    /// The Windows services this app's processes run, by the name Windows shows for them.
    pub services: Vec<String>,
    /// Whether this user may end at least one of its processes. The system's own answer no.
    pub can_quit: bool,
    /// The rules Meerkat applies to it and whether it is paused, filled in after grouping.
    #[serde(default)]
    pub control: AppControl,
    /// For a runtime grouped by project (node, python…): the project's folder.
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppControl {
    pub efficiency: bool,
    pub priority: Option<crate::control::Priority>,
    pub mute_alerts: bool,
    pub paused: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub taken_at_ms: u64,
    pub machine: MachineInfo,
    pub cpu: CpuMetric,
    pub memory: MemoryMetric,
    pub disk: DiskMetric,
    pub network: NetworkMetric,
    pub gpu: GpuMetric,
    pub battery: BatteryMetric,
    pub temperature: TemperatureMetric,
    pub apps: Vec<AppInfo>,
    pub process_count: usize,
    pub sampler_cost_microseconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub at_ms: u64,
    pub value: f64,
    pub secondary: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySeries {
    pub metric: String,
    pub bucket_seconds: u64,
    pub range_seconds: u64,
    pub points: Vec<HistoryPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedApp {
    pub key: String,
    pub name: String,
    pub value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankingRow {
    pub hour_start_ms: u64,
    pub metric: String,
    pub apps: Vec<RankedApp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsightKind {
    HighCpu,
    GrowingMemory,
    HeavyDisk,
    HeavyNetwork,
    Heat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Insight {
    pub id: String,
    pub at_ms: u64,
    pub kind: InsightKind,
    pub app_key: String,
    pub app_name: String,
    pub value: f64,
    pub sustained_seconds: u64,
    pub repeat_count: u32,
    pub day_key: String,
    pub resolved: bool,
}

/// Why the watchdog called an app runaway. Every variant carries the reading that justified it,
/// so a verdict cannot be constructed without its evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum RunawayReason {
    /// A runtime pinned a whole core this long.
    #[serde(rename_all = "camelCase")]
    CpuSustained { sustained_seconds: u64 },
    /// A runtime still holds a listening port whose project folder no longer exists.
    ProjectGone { project: String },
    /// A runtime has grown past the memory ceiling and is still growing.
    #[serde(rename_all = "camelCase")]
    MemoryCeiling { memory_bytes: u64, ceiling_bytes: u64 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunawayVerdict {
    /// `runaway:{app_key}:{day}` — dismissal scoped to one app for the rest of the day.
    pub id: String,
    pub app_key: String,
    pub app_name: String,
    pub reason: RunawayReason,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuitResult {
    pub asked: usize,
    pub force_used: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetricKind {
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    Battery,
    Temperature,
}

impl MetricKind {
    pub fn key(self) -> &'static str {
        match self {
            MetricKind::Cpu => "cpu",
            MetricKind::Memory => "memory",
            MetricKind::Disk => "disk",
            MetricKind::Network => "network",
            MetricKind::Gpu => "gpu",
            MetricKind::Battery => "battery",
            MetricKind::Temperature => "temperature",
        }
    }
}

/// One box of a storage map: a directory's share of the volume, or the free region, or the part
/// of used space the walk could not account for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMapBox {
    /// The last path component, for the label; empty for the free and unknown regions.
    pub name: String,
    pub path: String,
    pub bytes: u64,
    /// Share of the whole volume, `bytes / totalBytes`.
    pub share: f64,
    /// Space the walk could not read; drawn apart from the named directories.
    pub unknown: bool,
    /// The volume's free space; drawn unfilled.
    pub free: bool,
}

/// A whole volume drawn as boxes: what is using the space, what is free, and what could not be
/// read.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMap {
    pub volume_id: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    /// The bytes the walk actually measured, before the `boxes` were cut to the top N.
    pub measured_bytes: u64,
    /// Directories that could not be opened; their sizes are part of the unknown region.
    pub unreadable: u64,
    pub scanned_files: u64,
    /// A newer scan or a cancel took over before this one finished.
    pub cancelled: bool,
    pub boxes: Vec<StorageMapBox>,
}

// ---- Game performance sessions (src/perf.rs) -------------------------------------------------
// One session is one bounded monitoring run. The state is a machine: idle -> recording ->
// reported. A stopped session keeps its samples so the report can be reopened later.

/// Which application a session measures.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PerfTargetMode {
    /// Follow whichever application owns the foreground window at each tick.
    #[default]
    Foreground,
    /// Watch one application chosen by its stable key for the whole session.
    App,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfTarget {
    pub mode: PerfTargetMode,
    /// The key chosen in `App` mode; absent in `Foreground` mode.
    pub app_key: Option<String>,
    /// The name shown in the session list and the report.
    pub name: String,
    /// The application actually measured at the last tick; changes while following.
    pub current_app_key: Option<String>,
}

/// Where a frame rate came from. `None` means no measurement, never a guessed number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FpsSource {
    #[default]
    None,
    /// The last `MsBetweenPresents` of a PresentMon CSV Meerkat started for this process.
    PresentMon,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfSample {
    pub at_ms: u64,
    pub fps: Option<f64>,
    pub fps_source: FpsSource,
    /// Whole machine, so the number matches the Overview tab.
    pub cpu_percent: f64,
    /// The measured application's share of all logical processors.
    pub app_cpu_percent: Option<f64>,
    /// Windows exposes no CPU package temperature to an unprivileged process; None is honest.
    pub cpu_temp_c: Option<f64>,
    /// Whole machine; `app_gpu_percent` is the target's own engine share when it can be attributed.
    pub gpu_percent: Option<f64>,
    pub app_gpu_percent: Option<f64>,
    pub gpu_temp_c: Option<f64>,
    pub memory_percent: f64,
    pub app_memory_bytes: Option<u64>,
    pub vram_percent: Option<f64>,
    pub vram_used_bytes: Option<u64>,
    pub vram_total_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PerfVerdict {
    Healthy,
    Warning,
    Problem,
    #[default]
    Inconclusive,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PerfIssueKind {
    FpsLow,
    FpsUnstable,
    CpuBound,
    GpuBound,
    VramPressure,
    GpuHot,
    MemoryPressure,
    FpsUnavailable,
}

/// A finding with its evidence. The backend never sends prose: `message_key` is translated in the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfIssue {
    pub kind: PerfIssueKind,
    pub verdict: PerfVerdict,
    pub message_key: String,
    /// Which metric this finding is about, so the UI can place the number beside it.
    pub metric: String,
    pub value: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfStat {
    pub key: String,
    pub available: bool,
    pub min: Option<f64>,
    pub avg: Option<f64>,
    pub max: Option<f64>,
    pub p95: Option<f64>,
    pub last: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfReport {
    pub session_id: String,
    pub verdict: PerfVerdict,
    pub headline_key: String,
    pub issues: Vec<PerfIssue>,
    pub stats: Vec<PerfStat>,
    pub duration_seconds: u64,
    pub sample_count: usize,
}

/// One metric's shape across a session, thinned to what a chart can draw.
///
/// The sheet's chart is 240 units wide and a long session holds tens of thousands of readings,
/// so the report carries this envelope instead of the readings: shipping every sample moved
/// 17 MB over IPC for one twenty-hour session, and the sheet took half a minute to show.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfSeries {
    pub key: String,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfSession {
    pub id: String,
    pub target: PerfTarget,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    /// How many readings the session took, including the ones the series only summarises.
    #[serde(default)]
    pub sample_count: usize,
    pub report: Option<PerfReport>,
    /// What the report's charts draw (see `PerfSeries`).
    #[serde(default)]
    pub series: Vec<PerfSeries>,
    /// The readings themselves, held only while a session records: they are appended to a
    /// journal so a crash loses nothing, and a finished session keeps its report and series
    /// instead of them. Never written to the sessions file, which is why the file no longer
    /// grows with the length of the sessions it lists. Files from the older format still carry
    /// them, and `perf::Store::open` converts those.
    #[serde(default, skip_serializing)]
    pub samples: Vec<PerfSample>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfSessionSummary {
    pub id: String,
    pub target_name: String,
    pub mode: PerfTargetMode,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub sample_count: usize,
    pub verdict: PerfVerdict,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfStatus {
    pub active: bool,
    pub session_id: Option<String>,
    pub target_name: String,
    pub mode: PerfTargetMode,
    pub current_app_key: Option<String>,
    pub sample_count: usize,
    pub started_at_ms: Option<u64>,
    pub latest: Option<PerfSample>,
}
