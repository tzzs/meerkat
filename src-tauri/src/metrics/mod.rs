//! The sampler: one thread that owns every handle and produces a `Snapshot` per tick.
//!
//! Anything Windows reports through a plain API is read directly; PDH is only used for
//! the figures with no API behind them (memory pools, cache, page file usage, processor
//! queue length, GPU engine utilisation).

pub mod battery;
pub mod gpu;
pub mod grouping;
pub mod netinfo;
pub mod ntproc;
pub mod pdh;
pub mod services;
pub mod wins;

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use sysinfo::{Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};
use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    REG_VALUE_TYPE,
};
use windows::Win32::System::SystemInformation::GlobalMemoryStatusEx;
use windows::Win32::System::SystemInformation::MEMORYSTATUSEX;
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::System::SystemServices::{TIME_ZONE_ID_DAYLIGHT, TIME_ZONE_ID_STANDARD};
use windows::Win32::System::Time::{GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION};

use crate::models::*;
use gpu::EngineProbe;
use grouping::NameCache;
use pdh::PdhQuery;

const LIVE_WINDOW: usize = 240;

const PDH_SET: [(&str, &str); 5] = [
    ("cache", r"\Memory\Cache Bytes"),
    ("paged_pool", r"\Memory\Pool Paged Resident Bytes"),
    ("nonpaged_pool", r"\Memory\Pool Nonpaged Bytes"),
    ("pagefile", r"\Paging File(_ Total)\Private Bytes"),
    ("queue", r"\System\Processor Queue Length"),
];

#[derive(Default)]
struct Trace {
    samples: VecDeque<f64>,
}

impl Trace {
    fn push(&mut self, value: f64) {
        if self.samples.len() >= LIVE_WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    fn values(&self) -> Vec<f64> {
        self.samples.iter().cloned().collect()
    }
}

const TRACE_KEYS: [&str; 9] = [
    "cpu", "memory", "diskRead", "diskWrite", "download", "upload", "gpu", "battery", "temp",
];

pub struct Sampler {
    system: System,
    networks: Networks,
    disks: Disks,
    components: Components,
    pdh: Option<PdhQuery>,
    pdh_index: HashMap<String, usize>,
    gpu_probe: Option<EngineProbe>,
    processes: ntproc::Processes,
    services: services::ServiceDirectory,
    io: grouping::IoRates,
    names: NameCache,
    previous_network: HashMap<String, (u64, u64)>,
    previous_disk: HashMap<String, (u64, u64)>,
    previous_times: Option<(u64, u64, u64)>,
    last_tick: Instant,
    traces: HashMap<&'static str, Trace>,
    battery_health: battery::Health,
    health_read_at: Option<Instant>,
    day_key: String,
    day_read: u64,
    day_write: u64,
    day_down: u64,
    day_up: u64,
    machine: MachineInfo,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Sampler {
    pub fn new() -> Self {
        let mut traces: HashMap<&'static str, Trace> = HashMap::new();
        for key in TRACE_KEYS {
            traces.insert(key, Trace::default());
        }

        let mut pdh_index = HashMap::new();
        let pdh = match PdhQuery::new() {
            Some(mut query) => {
                for (name, path) in PDH_SET {
                    if let Some(index) = query.add(path) {
                        pdh_index.insert(name.to_string(), index);
                    }
                }
                query.collect();
                Some(query)
            }
            None => None,
        };

        // Processes come from the kernel's table (see ntproc), so sysinfo only has to keep the
        // processor and memory figures; asking it for processes too would open a handle on each
        // of them every tick and still come back empty for most of the system.
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let machine = machine_info(&system);

        Self {
            system,
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            pdh,
            pdh_index,
            gpu_probe: EngineProbe::new(),
            processes: ntproc::Processes::default(),
            services: services::ServiceDirectory::default(),
            io: grouping::IoRates::default(),
            names: NameCache::default(),
            previous_network: HashMap::new(),
            previous_disk: HashMap::new(),
            previous_times: None,
            last_tick: Instant::now(),
            traces,
            battery_health: battery::Health::default(),
            health_read_at: None,
            day_key: String::new(),
            day_read: 0,
            day_write: 0,
            day_down: 0,
            day_up: 0,
            machine,
        }
    }

    fn trace(&self, key: &str) -> Vec<f64> {
        self.traces.get(key).map(|t| t.values()).unwrap_or_default()
    }

    fn pdh_value(&mut self, name: &str) -> Option<f64> {
        let index = *self.pdh_index.get(name)?;
        self.pdh.as_mut()?.value(index)
    }

    pub fn sample(&mut self) -> Snapshot {
        let started = Instant::now();
        let elapsed = self.last_tick.elapsed().as_secs_f64().clamp(0.05, 60.0);
        self.last_tick = Instant::now();

        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        // One PDH collection per tick: the values read below are whatever this collected.
        if let Some(query) = self.pdh.as_mut() {
            query.collect();
        }

        let processes = grouping::collect(&mut self.processes);
        let io = self.io.advance(&processes).clone();
        let windows_list = wins::toplevel_windows();
        let tcp = netinfo::collect();
        let hosts: Vec<u32> = processes
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case("svchost.exe"))
            .map(|p| p.pid)
            .collect();
        self.services.refresh(&hosts);
        let apps = grouping::group(
            &processes,
            &windows_list,
            &tcp,
            &io,
            &mut self.names,
            self.services.by_pid(),
        );

        let mut cpu = self.cpu();
        let mut memory = self.memory();
        let mut disk = self.disk(elapsed);
        let mut network = self.network(elapsed, &tcp);
        let mut gpu = self.gpu();
        let mut battery_metric = self.battery();
        let mut temperature = self.temperature();
        // NVML is usually the only real temperature an unprivileged process can read,
        // so it fills the gap the ACPI thermal zone leaves rather than staying unused.
        if temperature.gpu_celsius.is_none() {
            temperature.gpu_celsius = gpu
                .adapters
                .iter()
                .filter_map(|adapter| adapter.temperature_c)
                .max_by(|a, b| a.total_cmp(b));
        }
        if let Some(gpu_temp) = temperature.gpu_celsius {
            if temperature.hottest.map_or(true, |h| gpu_temp > h) {
                temperature.hottest = Some(gpu_temp);
            }
            if temperature.availability == Availability::NeedsElevation {
                temperature.availability = Availability::Partial;
            }
        }

        let temp_headline = temperature.hottest.unwrap_or(0.0);
        for (key, value) in [
            ("cpu", cpu.total_percent),
            ("memory", memory.pressure_percent),
            ("diskRead", disk.read_bytes_per_second),
            ("diskWrite", disk.write_bytes_per_second),
            ("download", network.download_bytes_per_second),
            ("upload", network.upload_bytes_per_second),
            ("gpu", gpu.total_percent.unwrap_or(0.0)),
            ("battery", battery_metric.charge_percent.unwrap_or(0.0)),
            ("temp", temp_headline),
        ] {
            if let Some(trace) = self.traces.get_mut(key) {
                trace.push(value);
            }
        }

        cpu.history = self.trace("cpu");
        memory.history = self.trace("memory");
        disk.history_read = self.trace("diskRead");
        disk.history_write = self.trace("diskWrite");
        network.history_download = self.trace("download");
        network.history_upload = self.trace("upload");
        gpu.history = self.trace("gpu");
        battery_metric.history = self.trace("battery");
        temperature.history = self.trace("temp");

        let taken_at_ms = now_ms();
        self.roll_day(taken_at_ms, elapsed, &disk, &network);

        Snapshot {
            taken_at_ms,
            machine: self.machine.clone(),
            process_count: processes.len(),
            sampler_cost_microseconds: started.elapsed().as_micros() as u64,
            apps,
            cpu,
            memory,
            disk,
            network,
            gpu,
            battery: battery_metric,
            temperature,
        }
    }

    /// Today's totals are Pulse's own integral of the rate, because Windows has no
    /// "bytes since midnight" counter.
    fn roll_day(&mut self, taken_at_ms: u64, elapsed: f64, disk: &DiskMetric, network: &NetworkMetric) {
        let key = day_key(taken_at_ms);
        if key != self.day_key {
            self.day_key = key;
            self.day_read = 0;
            self.day_write = 0;
            self.day_down = 0;
            self.day_up = 0;
        }
        self.day_read += (disk.read_bytes_per_second * elapsed) as u64;
        self.day_write += (disk.write_bytes_per_second * elapsed) as u64;
        self.day_down += (network.download_bytes_per_second * elapsed) as u64;
        self.day_up += (network.upload_bytes_per_second * elapsed) as u64;
    }

    fn cpu(&mut self) -> CpuMetric {
        let core_percents: Vec<f64> = self.system.cpus().iter().map(|c| c.cpu_usage() as f64).collect();
        let total_percent = self.system.global_cpu_usage() as f64;

        let (mut user_percent, mut privileged_percent) = (0f64, 0f64);
        if let Some(current) = system_times() {
            if let Some(previous) = self.previous_times {
                let idle = current.0.saturating_sub(previous.0) as f64;
                let kernel = current.1.saturating_sub(previous.1) as f64;
                let user = current.2.saturating_sub(previous.2) as f64;
                // GetSystemTimes folds idle into kernel, so kernel + user is the whole
                // elapsed span across all cores. Dividing by that keeps user + system +
                // idle at 100% of the machine, not a share of busy time.
                let total = kernel + user;
                if total > 0.0 {
                    user_percent = user / total * 100.0;
                    privileged_percent = (kernel - idle) / total * 100.0;
                }
            }
            self.previous_times = Some(current);
        }

        CpuMetric {
            total_percent,
            user_percent,
            privileged_percent,
            core_percents,
            processor_queue_length: self.pdh_value("queue"),
            history: Vec::new(),
        }
    }

    fn memory(&mut self) -> MemoryMetric {
        let mut status = MEMORYSTATUSEX::default();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok();

        let (total, available, commit_limit, commit_free) = if ok {
            (
                status.ullTotalPhys,
                status.ullAvailPhys,
                status.ullTotalPageFile,
                status.ullAvailPageFile,
            )
        } else {
            (
                self.system.total_memory(),
                self.system.available_memory(),
                self.system.total_memory(),
                self.system.total_memory(),
            )
        };

        let committed = commit_limit.saturating_sub(commit_free);
        let pagefile_used = self.pdh_value("pagefile").unwrap_or(0.0).max(0.0) as u64;
        let paged_pool = self
            .pdh_value("paged_pool")
            .map(|v| v.max(0.0) as u64)
            .unwrap_or_else(|| committed.saturating_sub(total));

        MemoryMetric {
            total_bytes: total,
            in_use_bytes: total.saturating_sub(available),
            available_bytes: available,
            cached_bytes: self.pdh_value("cache").unwrap_or(0.0).max(0.0) as u64,
            committed_bytes: committed,
            commit_limit_bytes: commit_limit,
            pagefile_total_bytes: commit_limit.saturating_sub(total),
            pagefile_used_bytes: pagefile_used,
            paged_pool_bytes: paged_pool,
            nonpaged_pool_bytes: self.pdh_value("nonpaged_pool").unwrap_or(0.0).max(0.0) as u64,
            pressure_percent: if ok { status.dwMemoryLoad as f64 } else { 0.0 },
            history: Vec::new(),
        }
    }

    fn disk(&mut self, elapsed: f64) -> DiskMetric {
        self.disks.refresh(true);
        let mut volumes = Vec::new();
        let mut seen: HashMap<String, (u64, u64)> = HashMap::new();
        let (mut total_read, mut total_write) = (0f64, 0f64);

        for disk in self.disks.iter() {
            let name = disk.name().to_string_lossy().to_string();
            let mount = disk.mount_point().to_string_lossy().to_string();
            let usage = disk.usage();
            let (read_rate, write_rate) = match self.previous_disk.get(&name) {
                Some((previous_read, previous_write)) => (
                    usage.total_read_bytes.saturating_sub(*previous_read) as f64 / elapsed,
                    usage.total_written_bytes.saturating_sub(*previous_write) as f64 / elapsed,
                ),
                None => (0.0, 0.0),
            };
            total_read += read_rate;
            total_write += write_rate;
            seen.insert(name.clone(), (usage.total_read_bytes, usage.total_written_bytes));

            let total = disk.total_space();
            let free = disk.available_space();
            volumes.push(VolumeInfo {
                id: mount.clone(),
                name,
                mount_point: mount.clone(),
                file_system: disk.file_system().to_string_lossy().to_string(),
                total_bytes: total,
                used_bytes: total.saturating_sub(free),
                free_bytes: free,
                read_bytes_per_second: read_rate,
                write_bytes_per_second: write_rate,
                is_removable: disk.is_removable(),
                is_network: mount.starts_with("\\\\"),
                is_solid_state: matches!(disk.kind(), sysinfo::DiskKind::SSD),
            });
        }
        self.previous_disk = seen;
        volumes.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));

        DiskMetric {
            read_bytes_per_second: total_read,
            write_bytes_per_second: total_write,
            read_bytes_today: self.day_read,
            write_bytes_today: self.day_write,
            volumes,
            history_read: Vec::new(),
            history_write: Vec::new(),
            availability: if self.previous_disk.is_empty() {
                Availability::Unavailable
            } else {
                Availability::Ok
            },
        }
    }

    fn network(&mut self, elapsed: f64, tcp: &netinfo::TcpOwnership) -> NetworkMetric {
        self.networks.refresh(true);
        // One walk of the interface table per refresh, shared by every adapter row. An empty
        // map is a normal outcome: the speeds stay unset and the traffic numbers survive.
        let speeds = link_speed_index();
        let mut seen: HashMap<String, (u64, u64)> = HashMap::new();
        let mut rows: Vec<AdapterRow> = Vec::with_capacity(self.networks.len());

        // Rate every interface sysinfo reported first; the totals are summed after the dedupe
        // below, never here, so a driver binding cannot contribute its card's bytes twice.
        for (name, data) in self.networks.iter() {
            let (received, transmitted) = (data.total_received(), data.total_transmitted());
            let (read_rate, write_rate) = match self.previous_network.get(name) {
                Some((previous_rx, previous_tx)) => (
                    received.saturating_sub(*previous_rx) as f64 / elapsed,
                    transmitted.saturating_sub(*previous_tx) as f64 / elapsed,
                ),
                None => (0.0, 0.0),
            };
            seen.insert(name.clone(), (received, transmitted));
            rows.push(AdapterRow {
                name: name.clone(),
                received,
                transmitted,
                download_bytes_per_second: read_rate,
                upload_bytes_per_second: write_rate,
                is_up: matches!(data.operational_state(), sysinfo::InterfaceOperationalState::Up),
                is_loopback: is_loopback(name),
            });
        }
        self.previous_network = seen;

        let mut adapters = Vec::new();
        let (mut download, mut upload) = (0f64, 0f64);
        for row in drop_binding_rows(rows) {
            if !row.is_loopback {
                download += row.download_bytes_per_second;
                upload += row.upload_bytes_per_second;
            }
            adapters.push(AdapterInfo {
                name: row.name.clone(),
                is_up: row.is_up,
                link_speed_bits_per_second: match_link_speed(&row.name, &speeds),
                download_bytes_per_second: row.download_bytes_per_second,
                upload_bytes_per_second: row.upload_bytes_per_second,
                is_loopback: row.is_loopback,
            });
        }
        adapters.sort_by(|a, b| {
            (b.download_bytes_per_second + b.upload_bytes_per_second)
                .total_cmp(&(a.download_bytes_per_second + a.upload_bytes_per_second))
        });

        NetworkMetric {
            download_bytes_per_second: download,
            upload_bytes_per_second: upload,
            download_bytes_today: self.day_down,
            upload_bytes_today: self.day_up,
            adapters,
            connections: tcp.total_connections,
            history_download: Vec::new(),
            history_upload: Vec::new(),
        }
    }

    fn gpu(&mut self) -> GpuMetric {
        let Some(probe) = self.gpu_probe.as_mut() else {
            return GpuMetric {
                total_percent: None,
                adapters: Vec::new(),
                per_process: Vec::new(),
                availability: Availability::Unavailable,
                history: Vec::new(),
            };
        };
        let (total_percent, per_process, availability) = probe.refresh();
        let mut adapters = probe.adapters().to_vec();
        if let Some(percent) = total_percent {
            if let Some(first) = adapters.first_mut() {
                first.utilization_percent = Some(percent);
            }
        }
        // The probe reports Ok as soon as the engine counters answer; an adapter with nothing
        // measured at all is the case that has to demote it, because then the headline
        // describes only part of what the machine has. An adapter that reports memory but no
        // sensors is not a broken reading — it is a card whose driver exposes no sensors — and
        // saying so per adapter beats a global warning that would be permanently on for
        // everyone with an integrated GPU.
        let availability = if gpu::all_adapters_measured(&adapters) {
            availability
        } else {
            Availability::Partial
        };
        GpuMetric {
            total_percent,
            adapters,
            per_process,
            availability,
            history: Vec::new(),
        }
    }

    fn battery(&mut self) -> BatteryMetric {
        let live = battery::live();
        let due = self
            .health_read_at
            .map(|at| at.elapsed().as_secs() > 1800)
            .unwrap_or(true);
        if due && live.has_battery {
            let (health, _) = battery::read_health();
            self.battery_health = health;
            self.health_read_at = Some(Instant::now());
        }
        let health = &self.battery_health;
        let health_percent = match (health.design_capacity_wh, health.full_charge_capacity_wh) {
            (Some(design), Some(full)) if design > 0.0 => Some(full / design * 100.0),
            _ => None,
        };

        BatteryMetric {
            has_battery: live.has_battery,
            on_ac_power: live.on_ac_power,
            charge_percent: live.charge_percent,
            is_charging: live.is_charging,
            is_critical: live.is_critical,
            remaining_seconds: live.remaining_seconds,
            power_watts: None,
            design_capacity_wh: health.design_capacity_wh,
            full_charge_capacity_wh: health.full_charge_capacity_wh,
            health_percent,
            cycle_count: health.cycle_count,
            source: if !live.has_battery {
                Availability::Unavailable
            } else if health.design_capacity_wh.is_some() {
                Availability::Ok
            } else {
                Availability::Partial
            },
            history: Vec::new(),
        }
    }

    fn temperature(&mut self) -> TemperatureMetric {
        self.components.refresh(true);
        let mut sensors: Vec<SensorReading> = Vec::new();
        for component in self.components.iter() {
            let Some(reading) = component.temperature() else {
                continue;
            };
            if reading <= 0.0 {
                continue;
            }
            sensors.push(SensorReading {
                id: component.id().unwrap_or("sensor").to_string(),
                label: component.label().to_string(),
                celsius: reading as f64,
            });
        }
        let find = |needle: &str| {
            sensors
                .iter()
                .find(|s| s.label.to_lowercase().contains(needle) || s.id.to_lowercase().contains(needle))
                .map(|s| s.celsius)
        };
        let hottest = sensors.iter().map(|s| s.celsius).fold(0f64, f64::max);

        TemperatureMetric {
            cpu_celsius: find("cpu").or_else(|| (hottest > 0.0).then_some(hottest)),
            gpu_celsius: find("gpu"),
            hottest: (hottest > 0.0).then_some(hottest),
            availability: if sensors.is_empty() {
                Availability::NeedsElevation
            } else {
                Availability::Partial
            },
            sensors,
            history: Vec::new(),
        }
    }
}

fn system_times() -> Option<(u64, u64, u64)> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    Some((combine(idle), combine(kernel), combine(user)))
}

fn combine(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

fn is_loopback(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("loopback") || lower.starts_with("lo ") || name == "Software Simple"
}

/// One interface as `Sampler::network` sees it, before bindings are filtered out. A plain
/// struct so the dedupe can be unit tested without a live interface table.
#[derive(Clone, Debug, PartialEq)]
struct AdapterRow {
    name: String,
    /// Cumulative counters, not rates: the binding filter compares totals.
    received: u64,
    transmitted: u64,
    download_bytes_per_second: f64,
    upload_bytes_per_second: f64,
    is_up: bool,
    is_loopback: bool,
}

/// Windows keeps a driver binding — the packet driver Npcap attaches to a card, VMware's
/// VMnet1 adapter, and friends — as a separate row in the interface table, with its own
/// `Alias` of `<card>-<binding>` but the very same counters as the card it is bound to.
/// sysinfo therefore hands back `以太网 2` *and*
/// `以太网 2-Npcap Packet Driver (NPCAP)-0000`, and the UI shows one network card twice with
/// identical numbers. `Some(parent)` marks a row as such a shadow of an already kept adapter.
///
/// Both halves of the test are load-bearing: matching the name shape alone would merge two
/// genuinely distinct idle cards, which are both sitting at zero counters.
fn binding_parent<'a>(
    row: &AdapterRow,
    mut kept: impl Iterator<Item = &'a AdapterRow>,
) -> Option<&'a AdapterRow> {
    kept.find(|parent| {
        parent.name != row.name
            && parent.received == row.received
            && parent.transmitted == row.transmitted
            && is_binding_of(&row.name, &parent.name)
    })
}

/// The `<card>-<binding>` name shape: `name` is `parent` followed by `-` and then the binding.
fn is_binding_of(name: &str, parent: &str) -> bool {
    name.strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('-') && rest.len() > 1)
}

/// Drops binding shadows, keeping the shortest name — the physical card always comes first.
fn drop_binding_rows(rows: Vec<AdapterRow>) -> Vec<AdapterRow> {
    let mut rows = rows;
    rows.sort_by(|a, b| {
        a.name
            .chars()
            .count()
            .cmp(&b.name.chars().count())
            .then_with(|| a.name.cmp(&b.name))
    });
    rows.into_iter().fold(Vec::<AdapterRow>::new(), |mut kept, row| {
        if binding_parent(&row, kept.iter()).is_none() {
            kept.push(row);
        }
        kept
    })
}

/// Bound on how many interface rows are read, purely defensive: `NumEntries` comes from memory
/// the system allocated, and a nonsensical value must not turn into an unbounded loop. Real
/// machines are two orders of magnitude below this.
const MAX_IF_ROWS: usize = 4096;

/// Index keys say which `MIB_IF_ROW2` field they came from, because the match order matters:
/// exact alias, then exact description, then case-insensitive alias.
const FROM_ALIAS: &str = "alias=";
const FROM_DESCRIPTION: &str = "description=";
const FROM_FOLDED_ALIAS: &str = "folded-alias=";

/// An interface row reduced to the fields name matching needs.
#[derive(Clone, Debug)]
struct InterfaceSpeed {
    alias: String,
    description: String,
    /// Bits per second. `0` means "this interface reports no speed" and never reaches the index.
    link_speed: u64,
}

/// Reads `GetIfTable2` once and indexes it by name. Read-only, no elevation.
///
/// Failure is not interesting: iphlpapi can refuse, and an empty index just means every adapter
/// keeps `link_speed_bits_per_second: None`. Traffic numbers must never depend on this call.
fn link_speed_index() -> HashMap<String, u64> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) }.is_err() || table.is_null() {
        return HashMap::new();
    }
    let rows = unsafe { read_interface_speeds(table) };
    unsafe { FreeMibTable(table as *const core::ffi::c_void) };
    index_speeds(&rows)
}

/// Copies the rows out while the table is still ours to free.
unsafe fn read_interface_speeds(table: *mut MIB_IF_TABLE2) -> Vec<InterfaceSpeed> {
    let count = unsafe { (*table).NumEntries } as usize;
    let source = unsafe { (*table).Table.as_ptr() };
    let mut rows = Vec::new();
    for index in 0..count.min(MAX_IF_ROWS) {
        let row = unsafe { &*source.add(index) };
        // windows-rs 0.62 exposes MIB_IF_ROW2's speed as the transmit/receive pair rather than a
        // single `LinkSpeed` field. The two agree on Ethernet and can differ on Wi-Fi, where the
        // negotiated rate is the faster side.
        let link_speed = row.TransmitLinkSpeed.max(row.ReceiveLinkSpeed);
        rows.push(InterfaceSpeed {
            alias: from_utf16_nul(&row.Alias),
            description: from_utf16_nul(&row.Description),
            link_speed,
        });
    }
    rows
}

/// `MIB_IF_ROW2` name fields are fixed UTF-16 arrays; the string ends at the first NUL.
fn from_utf16_nul(field: &[u16]) -> String {
    let end = field.iter().position(|c| *c == 0).unwrap_or(field.len());
    String::from_utf16_lossy(&field[..end])
}

/// Builds the lookup keys for the three match tiers. Interfaces that report no speed — loopback,
/// a disconnected radio, a pseudo adapter, which all come back as 0 — are left out entirely, so
/// a match can never hand back a link of 0 bps. When several interfaces claim one key the faster
/// speed wins, because a binding mirrors its card and if it disagrees the card's rate is right.
fn index_speeds(rows: &[InterfaceSpeed]) -> HashMap<String, u64> {
    let mut index = HashMap::with_capacity(rows.len() * 3);
    for row in rows {
        if row.link_speed == 0 {
            continue;
        }
        for (prefix, key) in [
            (FROM_ALIAS, row.alias.clone()),
            (FROM_DESCRIPTION, row.description.clone()),
            (FROM_FOLDED_ALIAS, row.alias.to_lowercase()),
        ] {
            let key = format!("{prefix}{key}");
            index
                .entry(key)
                .and_modify(|speed: &mut u64| *speed = (*speed).max(row.link_speed))
                .or_insert(row.link_speed);
        }
    }
    index
}

/// The link speed of the adapter sysinfo named `name`, in bits per second: exact `Alias` first,
/// then exact `Description`, then case-insensitive `Alias`. `None` when nothing in the table
/// matches — an honest gap beats a guessed speed.
fn match_link_speed(name: &str, index: &HashMap<String, u64>) -> Option<u64> {
    let speed = index
        .get(&format!("{FROM_ALIAS}{name}"))
        .or_else(|| index.get(&format!("{FROM_DESCRIPTION}{name}")))
        .or_else(|| index.get(&format!("{FROM_FOLDED_ALIAS}{}", name.to_lowercase())))
        .copied()
        .unwrap_or(0);
    (speed > 0).then_some(speed)
}

pub fn day_key(at_ms: u64) -> String {
    ((at_ms as i64 + local_offset_ms()) / 86_400_000).to_string()
}

/// Daily totals must roll at local midnight, not UTC midnight.
fn local_offset_ms() -> i64 {
    let mut info = DYNAMIC_TIME_ZONE_INFORMATION::default();
    let zone = unsafe { GetDynamicTimeZoneInformation(&mut info) };
    let bias = if zone == TIME_ZONE_ID_DAYLIGHT {
        info.Bias + info.DaylightBias
    } else if zone == TIME_ZONE_ID_STANDARD {
        info.Bias + info.StandardBias
    } else {
        info.Bias
    };
    -(bias as i64) * 60_000
}

fn read_registry_string(hive: &HKEY, sub_key: &str, value: &str) -> Option<String> {
    let key: Vec<u16> = sub_key.encode_utf16().chain(std::iter::once(0)).collect();
    let name: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let mut opened = HKEY::default();
    if unsafe { RegOpenKeyExW(*hive, PCWSTR(key.as_ptr()), None, KEY_READ, &mut opened) }.is_err() {
        return None;
    }
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    let mut kind = REG_VALUE_TYPE(0);
    let status = unsafe {
        RegQueryValueExW(
            opened,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    let _ = unsafe { RegCloseKey(opened) };
    if status.is_err() {
        return None;
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buffer[..end]).trim().to_string())
}

pub fn machine_info(system: &System) -> MachineInfo {
    let product = read_registry_string(
        &HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "ProductName",
    );
    let build = read_registry_string(
        &HKEY_LOCAL_MACHINE,
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "CurrentBuildNumber",
    );
    let os_version = match (product, build) {
        (Some(product), Some(build)) => {
            // ProductName still says "Windows 10" on 11 because apps were told not to
            // branch on the version; the build number is what Settings actually shows.
            let marketing = if build.parse::<u32>().unwrap_or(0) >= 22000 {
                product.replace("Windows 10", "Windows 11")
            } else {
                product
            };
            format!("{marketing} (build {build})")
        }
        (Some(product), None) => product,
        _ => "Windows".to_string(),
    };
    let chip_name = read_registry_string(
        &HKEY_LOCAL_MACHINE,
        "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
        "ProcessorNameString",
    )
    .unwrap_or_else(|| {
        system
            .cpus()
            .first()
            .map(|c| c.brand().to_string())
            .unwrap_or_default()
    });

    MachineInfo {
        hostname: std::env::var("COMPUTERNAME").unwrap_or_default(),
        os_version,
        chip_name,
        physical_cores: System::physical_core_count(),
        logical_cores: system.cpus().len().max(1),
        total_memory_bytes: system.total_memory(),
        uptime_seconds: System::uptime(),
    }
}

// ---------------------------------------------------------------------------
// tests (pure logic only — no interface table and no iphlpapi, so they run anywhere)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, received: u64, transmitted: u64) -> AdapterRow {
        AdapterRow {
            name: name.to_string(),
            received,
            transmitted,
            download_bytes_per_second: 0.0,
            upload_bytes_per_second: 0.0,
            is_up: true,
            is_loopback: false,
        }
    }

    fn speed(alias: &str, description: &str, link_speed: u64) -> InterfaceSpeed {
        InterfaceSpeed {
            alias: alias.to_string(),
            description: description.to_string(),
            link_speed,
        }
    }

    #[test]
    fn binding_name_is_the_card_dashed_with_the_binding() {
        // The real shapes: Npcap's packet driver and VMware's host-only adapter.
        assert!(is_binding_of(
            "以太网 2-Npcap Packet Driver (NPCAP)-0000",
            "以太网 2"
        ));
        assert!(is_binding_of(
            "以太网 2-VMware Network Adapter VMnet1",
            "以太网 2"
        ));
        // A different card is not a binding, however similar the name looks.
        assert!(!is_binding_of("以太网 10", "以太网 2"));
        assert!(!is_binding_of("以太网 20", "以太网 2"));
        assert!(!is_binding_of("以太网 2", "以太网 2"));
        // A dash is required, and so is something after it.
        assert!(!is_binding_of("以太网 2", "以太网"));
        assert!(!is_binding_of("以太网 2-", "以太网 2"));
        assert!(!is_binding_of("WLAN 2", "以太网 2"));
    }

    #[test]
    fn binding_parent_needs_the_name_shape_and_identical_counters() {
        let card = row("以太网 2", 618_790, 52_205);
        let binding = row("以太网 2-Npcap Packet Driver (NPCAP)-0000", 618_790, 52_205);
        assert_eq!(binding_parent(&binding, [&card].into_iter()), Some(&card));

        // Two genuinely distinct idle cards: the counters agree (both zero) but neither name is
        // a binding of the other, so both must be reported.
        let idle = row("以太网 10", 0, 0);
        let other_idle = row("以太网 20", 0, 0);
        assert_eq!(binding_parent(&other_idle, [&idle].into_iter()), None);
        // Even a prefix-shaped pair with equal zero counters survives if the names are the
        // wrong way round, i.e. the shorter one is the one being tested.
        let prefixed_idle = row("以太网 2-VirtualBox Host Only", 0, 0);
        let zero_card = row("以太网 2", 0, 0);
        assert_eq!(binding_parent(&prefixed_idle, [&zero_card].into_iter()), Some(&zero_card));
        assert_eq!(binding_parent(&zero_card, [&prefixed_idle].into_iter()), None);

        // Name shape matches but the totals differ, so this is traffic of its own.
        let mut drifting = binding.clone();
        drifting.received += 1;
        assert_eq!(binding_parent(&drifting, [&card].into_iter()), None);
    }

    #[test]
    fn dedupe_keeps_the_card_and_drops_its_shadow() {
        let kept = drop_binding_rows(vec![
            row("以太网 2-Npcap Packet Driver (NPCAP)-0000", 618_790, 52_205),
            row("以太网 2", 618_790, 52_205),
            row("蓝牙网络连接 2", 0, 0),
            row("WLAN 2", 0, 0),
        ]);
        // The kept set is what matters; `drop_binding_rows` orders by name length so that the
        // card always precedes its binding, and `Sampler::network` re-sorts by traffic anyway.
        let mut names: Vec<&str> = kept.iter().map(|row| row.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["WLAN 2", "以太网 2", "蓝牙网络连接 2"]);
        assert_eq!(
            kept.iter().find(|row| row.name == "以太网 2").map(|row| row.received),
            Some(618_790),
            "the surviving row is the physical card"
        );
    }

    #[test]
    fn aggregate_totals_count_each_interface_once() {
        // `Sampler::network` sums over the deduped rows; the same arithmetic here proves a
        // binding's mirrored bytes cannot double the headline download figure.
        let mut card = row("以太网 2", 618_790, 52_205);
        card.download_bytes_per_second = 61_879.9;
        card.upload_bytes_per_second = 5_220.5;
        let mut shadow = card.clone();
        shadow.name = "以太网 2-Npcap Packet Driver (NPCAP)-0000".to_string();

        let rows = drop_binding_rows(vec![shadow, card]);
        let download: f64 = rows.iter().map(|r| r.download_bytes_per_second).sum();
        let upload: f64 = rows.iter().map(|r| r.upload_bytes_per_second).sum();
        assert_eq!(rows.len(), 1);
        assert_eq!(download, 61_879.9);
        assert_eq!(upload, 5_220.5);
    }

    #[test]
    fn link_speed_matches_alias_then_description_then_folded_alias() {
        let index = index_speeds(&[
            speed("以太网 2", "Realtek PCIe 2.5GbE Family Controller", 1_000_000_000),
            speed("WLAN 2", "MediaTek Wi-Fi 6E MT7922 Card", 866_000_000),
        ]);
        assert_eq!(match_link_speed("以太网 2", &index), Some(1_000_000_000));
        assert_eq!(
            match_link_speed("Realtek PCIe 2.5GbE Family Controller", &index),
            Some(1_000_000_000)
        );
        assert_eq!(match_link_speed("wlan 2", &index), Some(866_000_000));
        assert_eq!(match_link_speed("WLAN 2 ", &index), None, "names are not trimmed");
        assert_eq!(match_link_speed("以太网 99", &index), None);
        // GetIfTable2 failed, or the adapter simply is not in the table: an empty index is a
        // gap in the UI, not a wrong number.
        assert_eq!(match_link_speed("以太网 2", &HashMap::new()), None);
    }

    #[test]
    fn link_speed_tie_breaks_by_tier_then_by_the_faster_link() {
        // "Ethernet" is one interface's alias and another's description; the alias tier wins.
        let index = index_speeds(&[
            speed("Ethernet", "Realtek PCIe GbE", 1_000_000_000),
            speed("Card", "Ethernet", 100_000_000),
        ]);
        assert_eq!(match_link_speed("Ethernet", &index), Some(1_000_000_000));
        assert_eq!(match_link_speed("Card", &index), Some(100_000_000));

        // Two rows claim the same alias (a binding mirroring its card): the faster speed wins.
        let index = index_speeds(&[
            speed("以太网 2", "Npcap Packet Driver", 100_000_000),
            speed("以太网 2", "Realtek PCIe 2.5GbE Family Controller", 2_500_000_000),
        ]);
        assert_eq!(match_link_speed("以太网 2", &index), Some(2_500_000_000));

        // 0 bps is "no speed reported", so such an interface is never indexed at all.
        let index = index_speeds(&[speed("TAP-Windows Adapter V9", "TAP Adapter V9", 0)]);
        assert!(index.is_empty());
        assert_eq!(match_link_speed("TAP-Windows Adapter V9", &index), None);
    }

    #[test]
    fn interface_names_end_at_their_first_nul() {
        let utf16 = "以太网 2".encode_utf16().collect::<Vec<u16>>();
        let field = utf16.into_iter().chain([0u16; 4]).collect::<Vec<u16>>();
        assert_eq!(from_utf16_nul(&field), "以太网 2");
        assert_eq!(from_utf16_nul(&[0, 0, 0]), "");
    }
}
