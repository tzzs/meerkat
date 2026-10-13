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
    NeedsElevation,
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

/// Bytes moved on one local calendar day while Pulse was running.
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
