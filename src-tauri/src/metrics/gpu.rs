//! GPU: adapter inventory through DXGI, utilisation through PDH "GPU Engine".
//!
//! Windows has no single "GPU used %" number. It reports one counter per process per engine, and
//! the counters have to be put together in the right order:
//! - an engine's utilisation is the SUM over every process using it — two processes at 5.0% and
//!   3.8% on one engine are an engine at 8.8%, not 5.0%;
//! - an adapter's is its busiest engine, and the headline is the busiest adapter;
//! - a process's is the sum over its engines, at most 100.
//!
//! Engines are told apart, and attributed to an adapter, by the LUID in the instance name
//! (`pid_1234_luid_0x00000000_0x0000AB12_phys_0_eng_3_engtype_3D`), which is DXGI's
//! `AdapterLuid`. Putting the headline on the first adapter instead is wrong on every hybrid
//! laptop, where the first one is the integrated chip that is not the busy one.

use super::pdh::PdhQuery;
use crate::models::{Availability, GpuAdapter, GpuProcess};
use std::collections::HashMap;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
    DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIFactory6,
};
use windows::core::Interface;

/// One instance per process per engine; see `parse_instance` for the instance-name shape.
pub(crate) const ENGINE_COUNTER: &str = r"\GPU Engine(*)\Utilization Percentage";

pub struct EngineProbe {
    query: PdhQuery,
    index: usize,
    adapters: Vec<GpuAdapter>,
    /// Every LUID each adapter is known by, index-aligned with `adapters`. Internal on purpose:
    /// the `GpuAdapter` the UI gets has no use for it.
    luids: Vec<Vec<u64>>,
    last_adapter_refresh: std::time::Instant,
    last_sensor_read: std::time::Instant,
}

/// The adapters worth a row, without the identifiers they are known by.
#[allow(dead_code)]
pub fn enumerate_adapters() -> Vec<GpuAdapter> {
    enumerate_with_luids().into_iter().map(|(adapter, _)| adapter).collect()
}

/// The adapters worth a row, each with every LUID it was found under (more than one when DXGI
/// lists the same chip twice).
fn enumerate_with_luids() -> Vec<(GpuAdapter, Vec<u64>)> {
    let mut out: Vec<(GpuAdapter, Vec<u64>)> = Vec::new();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory6>() }) else {
        return out;
    };
    for i in 0..8u32 {
        let Ok(adapter1) = (unsafe { factory.EnumAdapters1(i) }) else {
            break;
        };
        let Some(desc) = adapter_desc(&adapter1) else {
            continue;
        };
        let name = wide_to_string(&desc.Description);
        if is_hidden_renderer(&name, desc.VendorId, desc.Flags) {
            continue;
        }
        let luid = pack_luid(desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart);
        let kept: Vec<GpuAdapter> = out.iter().map(|(adapter, _)| adapter.clone()).collect();
        if let Some(first) = duplicate_of(&kept, &name, desc.DedicatedVideoMemory as u64) {
            // One chip under a second identifier: one row, and the engines that name either
            // identifier belong to it.
            out[first].1.push(luid);
            continue;
        }
        let mut dedicated_total = Some(desc.DedicatedVideoMemory as u64);
        let mut dedicated_used = None;
        let mut shared_used = Some(desc.SharedSystemMemory as u64);

        if let Ok(a3) = adapter1.cast::<windows::Win32::Graphics::Dxgi::IDXGIAdapter3>() {
            dedicated_used = memory_info(&a3, DXGI_MEMORY_SEGMENT_GROUP_LOCAL).map(|m| m.CurrentUsage);
            if let Some(remote) = memory_info(&a3, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL) {
                shared_used = Some(remote.CurrentUsage);
            }
            if desc.DedicatedVideoMemory == 0 {
                dedicated_total = memory_info(&a3, DXGI_MEMORY_SEGMENT_GROUP_LOCAL).map(|m| m.Budget);
            }
        }

        out.push((
            GpuAdapter {
                index: out.len() as u32,
                name,
                dedicated_total_bytes: dedicated_total,
                dedicated_used_bytes: dedicated_used,
                shared_used_bytes: shared_used,
                temperature_c: None,
                fan_percent: None,
                power_watts: None,
                utilization_percent: None,
            },
            vec![luid],
        ));
    }
    let _ = factory;
    out
}

/// A LUID as one number, high part first — the order the counter's instance names print it in.
fn pack_luid(high: i32, low: u32) -> u64 {
    ((high as u32 as u64) << 32) | low as u64
}

/// The WARP software rasteriser and the Microsoft Basic Render Driver show up as adapters
/// too; neither is a chip anyone wants a row for.
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

fn is_hidden_renderer(name: &str, vendor_id: u32, flags: u32) -> bool {
    flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 || vendor_id == 0x1414 || name.contains("Basic Render")
}

/// DXGI can hand back the same physical chip twice under different LUIDs, so match on what
/// the row would show rather than on the identifier. The index of the row it duplicates.
fn duplicate_of(seen: &[GpuAdapter], name: &str, dedicated_total_bytes: u64) -> Option<usize> {
    seen.iter()
        .position(|a| a.name == name && a.dedicated_total_bytes == Some(dedicated_total_bytes))
}

/// Whether every row has at least one measured number to show: its utilisation, which the
/// engine counters give, or its memory, which DXGI gives. The caller demotes the whole reading
/// to partial when one is blank, because then the headline describes only part of the machine.
pub fn all_adapters_measured(adapters: &[GpuAdapter]) -> bool {
    adapters
        .iter()
        .all(|a| a.utilization_percent.is_some() || a.dedicated_total_bytes.is_some())
}

fn adapter_desc(adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1) -> Option<DXGI_ADAPTER_DESC1> {
    unsafe { adapter.GetDesc1().ok() }
}

fn memory_info(
    adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter3,
    group: windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP,
) -> Option<DXGI_QUERY_VIDEO_MEMORY_INFO> {
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    (unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info) })
        .ok()
        .map(|_| info)
}

fn wide_to_string(raw: &[u16; 128]) -> String {
    let end = raw.iter().position(|c| *c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

impl EngineProbe {
    pub fn new() -> Option<Self> {
        let mut query = PdhQuery::new()?;
        let index = query.add(ENGINE_COUNTER)?;
        query.collect();
        let (adapters, luids) = enumerate_with_luids().into_iter().unzip();
        Some(Self {
            query,
            index,
            adapters,
            luids,
            last_adapter_refresh: std::time::Instant::now(),
            last_sensor_read: std::time::Instant::now() - std::time::Duration::from_secs(60),
        })
    }

    /// NVIDIA reports temperature, fan and live power draw through NVML without any
    /// elevation, which is the only real sensor source on a machine whose ACPI thermal
    /// zone is a static motherboard reading rather than the CPU die.
    fn read_sensors(&mut self) {
        if self.last_sensor_read.elapsed().as_secs() < 5 {
            return;
        }
        self.last_sensor_read = std::time::Instant::now();
        let Some(devices) = crate::nvml::devices() else {
            return;
        };
        for adapter in self.adapters.iter_mut() {
            let name = adapter.name.to_lowercase();
            let Some(device) = devices.iter().find(|d| d.name.to_lowercase() == name) else {
                continue;
            };
            adapter.temperature_c = device.temperature_c.map(|v| v as f64);
            adapter.fan_percent = device.fan_percent.map(|v| v as f64);
            adapter.power_watts = device.power_watts.map(|v| v as f64);
            // DXGI reports zero used video memory on NVIDIA drivers, which is an absence
            // rather than a measurement; NVML has the real figure.
            if adapter.dedicated_used_bytes.unwrap_or(0) == 0 {
                adapter.dedicated_used_bytes = device.memory_used_bytes;
            }
        }
    }

    /// The headline utilisation, the per-process figures and what actually arrived:
    /// `Unavailable` when the counter query failed, `Partial` when it succeeded but yielded no
    /// engine rows (the headline is then absent rather than a guessed 0), `Ok` when it did.
    /// Each adapter's own `utilization_percent` is set as a side effect. The caller can still
    /// downgrade the availability with `all_adapters_measured`.
    pub fn refresh(&mut self) -> (Option<f64>, Vec<GpuProcess>, Availability) {
        self.read_sensors();
        if self.last_adapter_refresh.elapsed().as_secs() > 300 {
            let (adapters, luids) = enumerate_with_luids().into_iter().unzip();
            self.adapters = adapters;
            self.luids = luids;
            self.last_adapter_refresh = std::time::Instant::now();
        }
        if !self.query.collect() {
            self.clear_utilisation();
            return (None, Vec::new(), Availability::Unavailable);
        }
        let rows = self.query.array(self.index);
        if rows.is_empty() {
            // No engine instances came back, which says nothing about how busy the chips are:
            // an absent figure, not a 0.
            self.clear_utilisation();
            return (None, Vec::new(), Availability::Partial);
        }

        let loads = EngineLoads::from_rows(&rows);
        let (per_adapter, headline) = loads.per_adapter(&self.luids);
        for (adapter, percent) in self.adapters.iter_mut().zip(per_adapter) {
            adapter.utilization_percent = percent;
        }
        (headline, loads.processes(), Availability::Ok)
    }

    fn clear_utilisation(&mut self) {
        for adapter in self.adapters.iter_mut() {
            adapter.utilization_percent = None;
        }
    }

    pub fn adapters(&self) -> &[GpuAdapter] {
        &self.adapters
    }
}

/// A row of the "GPU Engine" counter set, taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EngineInstance {
    pid: Option<u32>,
    /// The adapter, in the packing of [`pack_luid`].
    luid: Option<u64>,
    /// Which physical engine of that adapter: `phys.eng`. When the name does not carry both,
    /// the whole instance name, so that a format this code does not know degrades to "each row
    /// is an engine of its own" (the busiest row) instead of to a wrong merge.
    engine: String,
}

/// Instance names look like `pid_1234_luid_0x00000000_0x0000AB12_phys_0_eng_3_engtype_3D`.
/// Everything before the engine type is a number, so the name is read as words separated by
/// `_` (and `,`, which an older note of the format used).
fn parse_instance(name: &str) -> EngineInstance {
    let words: Vec<&str> = name.split(['_', ',']).collect();
    let after = |key: &str| {
        words
            .iter()
            .position(|word| word.eq_ignore_ascii_case(key))
            .map(|at| &words[at + 1..])
    };
    let number = |key: &str| -> Option<u32> { after(key)?.first()?.parse().ok() };

    let luid = after("luid").and_then(|rest| {
        let hex = |word: &&str| {
            word.strip_prefix("0x")
                .or_else(|| word.strip_prefix("0X"))
                .and_then(|digits| u64::from_str_radix(digits, 16).ok())
        };
        match (rest.first().and_then(hex), rest.get(1).and_then(hex)) {
            (Some(high), Some(low)) if high <= u32::MAX as u64 && low <= u32::MAX as u64 => {
                Some((high << 32) | low)
            }
            // One 16-digit number: the two halves already side by side.
            (Some(whole), _) => Some(whole),
            _ => None,
        }
    });
    let engine = match (number("phys"), number("eng")) {
        (Some(phys), Some(eng)) => format!("{phys}.{eng}"),
        _ => name.to_string(),
    };
    EngineInstance { pid: number("pid"), luid, engine }
}

/// What one tick's engine rows add up to.
struct EngineLoads {
    /// Per physical engine (adapter, engine), the sum over the processes using it, at most 100.
    engines: HashMap<(Option<u64>, String), f64>,
    /// Per process, the sum over its engines, at most 100.
    processes: HashMap<u32, f64>,
}

impl EngineLoads {
    fn from_rows(rows: &[(String, f64)]) -> Self {
        let mut engines: HashMap<(Option<u64>, String), f64> = HashMap::new();
        let mut processes: HashMap<u32, f64> = HashMap::new();
        for (name, value) in rows {
            // A utilisation is a share of one engine's time; a negative one is noise.
            let value = value.max(0.0);
            let instance = parse_instance(name);
            *engines.entry((instance.luid, instance.engine)).or_insert(0.0) += value;
            if let Some(pid) = instance.pid {
                *processes.entry(pid).or_insert(0.0) += value;
            }
        }
        // The rows are sampled a hair apart, so a sum can pass 100 by a little.
        for load in engines.values_mut().chain(processes.values_mut()) {
            *load = load.min(100.0);
        }
        Self { engines, processes }
    }

    /// Each adapter's busiest engine (index-aligned with `adapters`, which lists the LUIDs each
    /// is known by) and the headline, the busiest adapter.
    ///
    /// An adapter no engine row names is idle: the counters list the engines of the processes
    /// that use it, so none means none. It reads 0 as long as the rows could be attributed at
    /// all. When none can (no row names a LUID that DXGI listed, or there is no adapter to match
    /// against), the busiest engine of all is the headline, and goes to the first adapter,
    /// which is the least wrong thing left to say.
    fn per_adapter(&self, adapters: &[Vec<u64>]) -> (Vec<Option<f64>>, Option<f64>) {
        fn busiest(loads: impl Iterator<Item = f64>) -> Option<f64> {
            loads.fold(None, |best: Option<f64>, load| Some(best.map_or(load, |b| b.max(load))))
        }
        let measured: Vec<Option<f64>> = adapters
            .iter()
            .map(|luids| {
                busiest(
                    self.engines
                        .iter()
                        .filter(|((luid, _), _)| luid.is_some_and(|luid| luids.contains(&luid)))
                        .map(|(_, load)| *load),
                )
            })
            .collect();

        if measured.iter().all(Option::is_none) {
            let all = busiest(self.engines.values().cloned());
            let mut per_adapter = vec![None; adapters.len()];
            if let Some(first) = per_adapter.first_mut() {
                *first = all;
            }
            return (per_adapter, all);
        }
        let per_adapter: Vec<Option<f64>> =
            measured.into_iter().map(|load| Some(load.unwrap_or(0.0))).collect();
        let headline = busiest(per_adapter.iter().flatten().cloned());
        (per_adapter, headline)
    }

    /// The processes using the GPU, busiest first. Those under half a percent are noise.
    fn processes(&self) -> Vec<GpuProcess> {
        let mut out: Vec<GpuProcess> = self
            .processes
            .iter()
            .filter(|(_, percent)| **percent >= 0.5)
            .map(|(pid, percent)| GpuProcess { pid: *pid, percent: *percent })
            .collect();
        out.sort_by(|a, b| b.percent.total_cmp(&a.percent).then(a.pid.cmp(&b.pid)));
        out.truncate(200);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(
        name: &str,
        dedicated_total_bytes: Option<u64>,
        utilization_percent: Option<f64>,
    ) -> GpuAdapter {
        GpuAdapter {
            index: 0,
            name: name.to_string(),
            dedicated_total_bytes,
            dedicated_used_bytes: None,
            shared_used_bytes: None,
            temperature_c: None,
            fan_percent: None,
            power_watts: None,
            utilization_percent,
        }
    }

    /// Neither the WARP rasteriser nor the Basic Render Driver is a chip worth a row.
    #[test]
    fn software_and_basic_renderers_are_filtered_out() {
        assert!(is_hidden_renderer("Microsoft Basic Render Driver", 0x1414, 0));
        assert!(is_hidden_renderer("Intel UHD Graphics", 0x8086, DXGI_ADAPTER_FLAG_SOFTWARE));
        assert!(is_hidden_renderer("Some Accelerator", 0x1414, 0));
        assert!(!is_hidden_renderer("NVIDIA GeForce RTX 4070", 0x10de, 0));
    }

    /// The same chip behind a second LUID stays one row; a second chip that merely shares
    /// its name does not get folded away with it.
    #[test]
    fn adapters_dedupe_by_name_and_capacity() {
        let seen = vec![
            adapter("Intel UHD Graphics 770", Some(134_217_728), None),
            adapter("NVIDIA GeForce RTX 4070", Some(12_884_901_888), None),
        ];
        assert_eq!(duplicate_of(&seen, "NVIDIA GeForce RTX 4070", 12_884_901_888), Some(1));
        assert_eq!(duplicate_of(&seen, "NVIDIA GeForce RTX 4070", 4_294_967_296), None);
        assert_eq!(duplicate_of(&seen, "Intel UHD Graphics 770", 134_217_728), Some(0));
        assert_eq!(duplicate_of(&seen, "Intel UHD Graphics 770", 0), None);
        assert_eq!(duplicate_of(&[], "NVIDIA GeForce RTX 4070", 12_884_901_888), None);
    }

    /// A row is honest as soon as it carries one measured number, and the reading goes
    /// partial the moment any row would have to be blank.
    #[test]
    fn a_row_counts_as_measured_once_it_has_one_real_number() {
        let utilised = adapter("NVIDIA GeForce RTX 4070", Some(12_884_901_888), Some(18.7));
        assert!(all_adapters_measured(&[utilised.clone()]));

        // No engine attributed to this chip, but DXGI did measure its memory.
        assert!(all_adapters_measured(&[adapter(
            "Intel UHD Graphics 770",
            Some(134_217_728),
            None
        )]));

        let blank = adapter("Virtual Display Adapter", None, None);
        assert!(!all_adapters_measured(&[blank.clone()]));
        assert!(
            !all_adapters_measured(&[utilised, blank]),
            "one blank row makes the whole reading partial"
        );

        // Nothing enumerated is nothing unmeasured; the rows carry no claim at all.
        assert!(all_adapters_measured(&[]));
    }

    // The instance names are Windows's: pid, the adapter's LUID as two 8-digit halves (high
    // first), the physical engine and its index, and the engine's type last.
    fn name(pid: u32, luid_low: u32, phys: u32, eng: u32, kind: &str) -> String {
        format!("pid_{pid}_luid_0x00000000_0x{luid_low:08X}_phys_{phys}_eng_{eng}_engtype_{kind}")
    }

    fn rows(list: &[(String, f64)]) -> EngineLoads {
        EngineLoads::from_rows(list)
    }

    const IGPU: u64 = 0x1111;
    const DGPU: u64 = 0xAB12;

    #[test]
    fn an_instance_name_is_taken_apart_into_process_adapter_and_engine() {
        let parsed = parse_instance("pid_1234_luid_0x00000000_0x0000AB12_phys_0_eng_3_engtype_3D");
        assert_eq!(parsed.pid, Some(1234));
        assert_eq!(parsed.luid, Some(DGPU));
        assert_eq!(parsed.engine, "0.3");

        // The high half is the first number, so a LUID with one is not mistaken for another.
        let high = parse_instance("pid_7_luid_0x00000001_0x00000002_phys_1_eng_0_engtype_Copy");
        assert_eq!(high.luid, Some(0x1_0000_0002));
        assert_eq!(high.engine, "1.0");

        // An engine type with a space in it does not disturb the rest.
        let kind = parse_instance("pid_9_luid_0x00000000_0x0000AB12_phys_0_eng_12_engtype_High Priority 3D");
        assert_eq!((kind.pid, kind.luid, kind.engine.as_str()), (Some(9), Some(DGPU), "0.12"));
        assert_eq!(parse_instance("pid_0_luid_0x00000000_0x0000AB12_phys_0_eng_0_engtype_3D").pid, Some(0));
    }

    #[test]
    fn a_name_without_a_process_or_in_a_format_nobody_knows_still_parses() {
        assert_eq!(parse_instance("luid_0x1,engtype_3d").pid, None);
        assert_eq!(parse_instance("pid_abc,luid_0x1").pid, None);

        let parsed = parse_instance("something_else_entirely");
        assert_eq!((parsed.pid, parsed.luid), (None, None));
        assert_eq!(parsed.engine, "something_else_entirely");

        // The older, comma-separated notation still gives the process and the adapter, and
        // with no engine index in it, each row is an engine of its own.
        let old = parse_instance("pid_1234,luid_0x0000000000008698,engtype_3d");
        assert_eq!((old.pid, old.luid), (Some(1234), Some(0x8698)));
        assert_eq!(old.engine, "pid_1234,luid_0x0000000000008698,engtype_3d");
    }

    /// The parser is only worth what its idea of the instance names is worth, so every name this
    /// machine's counter actually has is taken apart here. A machine with no engine rows (a VM
    /// without a GPU) has nothing to say, and passes.
    #[test]
    fn every_instance_name_the_live_counter_has_parses() {
        let mut query = PdhQuery::new().expect("PDH opens a query");
        let index = query.add(ENGINE_COUNTER).expect("the engine counter resolves");
        query.collect();
        std::thread::sleep(std::time::Duration::from_millis(250));
        query.collect();
        for (instance, _) in query.array(index) {
            let parsed = parse_instance(&instance);
            assert!(parsed.pid.is_some(), "no process in {instance}");
            assert!(parsed.luid.is_some(), "no adapter in {instance}");
            assert_ne!(parsed.engine, instance, "no physical engine in {instance}");
        }
    }

    #[test]
    fn a_dxgi_luid_packs_the_way_the_counter_prints_it() {
        assert_eq!(pack_luid(0, 0xAB12), DGPU);
        assert_eq!(pack_luid(1, 2), 0x1_0000_0002);
        assert_eq!(pack_luid(-1, 0), 0xFFFF_FFFF_0000_0000, "the high part is unsigned in the name");
    }

    // Live evidence that drove this: a total of 4.95% while the two busiest processes read 5.0%
    // and 3.8%. The total was the highest single row; the engine they share is at 8.8%.
    #[test]
    fn processes_sharing_an_engine_add_up_to_the_engine() {
        let loads = rows(&[
            (name(100, 0xAB12, 0, 0, "3D"), 5.0),
            (name(200, 0xAB12, 0, 0, "3D"), 3.8),
            (name(300, 0xAB12, 0, 1, "Copy"), 2.0),
        ]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![DGPU]]);
        assert_eq!(per_adapter.len(), 1);
        assert!((per_adapter[0].unwrap() - 8.8).abs() < 1e-9, "{per_adapter:?}");
        assert!((headline.unwrap() - 8.8).abs() < 1e-9, "{headline:?}");

        let pids: Vec<(u32, f64)> = loads.processes().iter().map(|p| (p.pid, p.percent)).collect();
        assert_eq!(pids, vec![(100, 5.0), (200, 3.8), (300, 2.0)]);
    }

    #[test]
    fn a_process_is_the_sum_of_its_engines_and_never_over_a_hundred() {
        let loads = rows(&[
            (name(100, 0xAB12, 0, 0, "3D"), 40.0),
            (name(100, 0xAB12, 0, 1, "Copy"), 15.0),
            (name(100, 0xAB12, 0, 2, "VideoDecode"), 10.0),
            (name(200, 0xAB12, 0, 0, "3D"), 70.0),
            (name(200, 0xAB12, 0, 3, "Compute_0"), 60.0),
        ]);
        let by_pid: HashMap<u32, f64> = loads.processes().iter().map(|p| (p.pid, p.percent)).collect();
        assert_eq!(by_pid[&100], 65.0);
        assert_eq!(by_pid[&200], 100.0, "130 of summed engines is capped");
        // And an engine shared by two processes cannot pass 100 either: 40 + 70 on engine 0.
        let (per_adapter, _) = loads.per_adapter(&[vec![DGPU]]);
        assert_eq!(per_adapter, vec![Some(100.0)]);
    }

    #[test]
    fn the_headline_is_the_busiest_adapter_not_the_first_one() {
        // A hybrid laptop: the integrated chip is listed first and is nearly idle, the
        // discrete one is doing the work.
        let loads = rows(&[
            (name(100, 0x1111, 0, 0, "3D"), 6.0),
            (name(200, 0xAB12, 0, 0, "3D"), 55.0),
            (name(300, 0xAB12, 0, 0, "3D"), 25.0),
            (name(300, 0xAB12, 0, 1, "Copy"), 9.0),
        ]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![IGPU], vec![DGPU]]);
        assert_eq!(per_adapter, vec![Some(6.0), Some(80.0)]);
        assert_eq!(headline, Some(80.0));
    }

    #[test]
    fn engines_with_the_same_index_on_different_adapters_are_different_engines() {
        let loads = rows(&[
            (name(100, 0x1111, 0, 0, "3D"), 30.0),
            (name(200, 0xAB12, 0, 0, "3D"), 30.0),
        ]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![IGPU], vec![DGPU]]);
        assert_eq!(per_adapter, vec![Some(30.0), Some(30.0)], "not one engine at 60");
        assert_eq!(headline, Some(30.0));
    }

    #[test]
    fn one_chip_listed_under_two_luids_is_one_adapter() {
        let loads = rows(&[
            (name(100, 0x1111, 0, 0, "3D"), 20.0),
            (name(200, 0x2222, 0, 0, "3D"), 35.0),
        ]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![0x1111, 0x2222]]);
        assert_eq!(per_adapter, vec![Some(35.0)]);
        assert_eq!(headline, Some(35.0));
    }

    #[test]
    fn an_adapter_nothing_runs_on_is_idle_while_the_others_are_measured() {
        let loads = rows(&[(name(100, 0x1111, 0, 0, "3D"), 12.0)]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![IGPU], vec![DGPU]]);
        assert_eq!(per_adapter, vec![Some(12.0), Some(0.0)]);
        assert_eq!(headline, Some(12.0));
    }

    #[test]
    fn engines_of_an_adapter_nobody_lists_are_not_the_headline() {
        // WARP and the Basic Render Driver are not rows; their engines are not the GPU's load.
        let loads = rows(&[
            (name(100, 0xAB12, 0, 0, "3D"), 10.0),
            (name(200, 0x9999, 0, 0, "3D"), 90.0),
        ]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![DGPU]]);
        assert_eq!(per_adapter, vec![Some(10.0)]);
        assert_eq!(headline, Some(10.0));
        // The process using it is still a process using a GPU engine.
        assert!(loads.processes().iter().any(|p| p.pid == 200));
    }

    #[test]
    fn when_no_row_can_be_attributed_the_busiest_engine_is_the_headline() {
        // LUIDs DXGI never listed, or names that carry none: the previous reading, put on the
        // first adapter only because it is the one place there is to put it.
        let loads = rows(&[
            (name(1, 0x5555, 0, 0, "3D"), 15.0),
            (name(2, 0x5555, 0, 0, "3D"), 10.0),
            (name(2, 0x5555, 0, 1, "Copy"), 3.0),
        ]);
        let (per_adapter, headline) = loads.per_adapter(&[vec![IGPU], vec![DGPU]]);
        assert_eq!(per_adapter, vec![Some(25.0), None]);
        assert_eq!(headline, Some(25.0));

        let (per_adapter, headline) = loads.per_adapter(&[]);
        assert!(per_adapter.is_empty());
        assert_eq!(headline, Some(25.0), "no adapter listed is not no reading");

        let unknown = rows(&[("who_knows_1".to_string(), 7.0), ("who_knows_2".to_string(), 9.0)]);
        assert_eq!(
            unknown.per_adapter(&[vec![DGPU]]).1,
            Some(9.0),
            "rows of an unknown shape are engines of their own"
        );
    }

    #[test]
    fn noise_below_zero_and_processes_under_half_a_percent_are_dropped() {
        let loads = rows(&[
            (name(100, 0xAB12, 0, 0, "3D"), -3.0),
            (name(200, 0xAB12, 0, 1, "3D"), 0.4),
            (name(300, 0xAB12, 0, 2, "3D"), 0.5),
        ]);
        let pids: Vec<u32> = loads.processes().iter().map(|p| p.pid).collect();
        assert_eq!(pids, vec![300]);
        assert_eq!(loads.per_adapter(&[vec![DGPU]]).0, vec![Some(0.5)]);
    }
}
