//! GPU: adapter inventory through DXGI, utilisation through PDH "GPU Engine".
//!
//! Windows has no single "GPU used %" number. It reports utilisation per engine per
//! process, so the total is the busiest engine and the per-app figure is the sum of
//! that process's engines — the same reading Task Manager takes.

use super::pdh::PdhQuery;
use crate::models::{Availability, GpuAdapter, GpuProcess};
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
    last_adapter_refresh: std::time::Instant,
    last_sensor_read: std::time::Instant,
}

pub fn enumerate_adapters() -> Vec<GpuAdapter> {
    let mut out: Vec<GpuAdapter> = Vec::new();
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
        if is_duplicate_adapter(&out, &name, desc.DedicatedVideoMemory as u64) {
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

        out.push(GpuAdapter {
            index: out.len() as u32,
            name,
            dedicated_total_bytes: dedicated_total,
            dedicated_used_bytes: dedicated_used,
            shared_used_bytes: shared_used,
            temperature_c: None,
            fan_percent: None,
            power_watts: None,
            utilization_percent: None,
        });
    }
    let _ = factory;
    out
}

/// The WARP software rasteriser and the Microsoft Basic Render Driver show up as adapters
/// too; neither is a chip anyone wants a row for.
const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;

fn is_hidden_renderer(name: &str, vendor_id: u32, flags: u32) -> bool {
    flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 || vendor_id == 0x1414 || name.contains("Basic Render")
}

/// DXGI can hand back the same physical chip twice under different LUIDs, so match on what
/// the row would show rather than on the identifier.
fn is_duplicate_adapter(seen: &[GpuAdapter], name: &str, dedicated_total_bytes: u64) -> bool {
    seen
        .iter()
        .any(|a| a.name == name && a.dedicated_total_bytes == Some(dedicated_total_bytes))
}

/// The caller folds the headline utilisation onto the first adapter only, so this is how it
/// tells whether every row still has at least one measured number to show.
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
        Some(Self {
            query,
            index,
            adapters: enumerate_adapters(),
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

    /// Reports availability as what actually arrived: `Unavailable` when the counter query
    /// failed, `Partial` when it succeeded but yielded no engine rows, `Ok` when it did. The
    /// caller can still downgrade with `all_adapters_measured`.
    pub fn refresh(&mut self) -> (Option<f64>, Vec<GpuProcess>, Availability) {
        self.read_sensors();
        if self.last_adapter_refresh.elapsed().as_secs() > 300 {
            self.adapters = enumerate_adapters();
            self.last_adapter_refresh = std::time::Instant::now();
        }
        if !self.query.collect() {
            return (None, Vec::new(), Availability::Unavailable);
        }
        let rows = self.query.array(self.index);
        if rows.is_empty() {
            // No engine instances came back, so the 0.0 this branch reports is a guess
            // rather than a measurement; the reading is only partial.
            return (
                Some(0.0),
                Vec::new(),
                Availability::Partial,
            );
        }

        let mut per_pid: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
        let mut busiest = 0f64;
        for (name, value) in rows {
            if value > busiest {
                busiest = value;
            }
            if let Some(pid) = parse_pid(&name) {
                *per_pid.entry(pid).or_insert(0.0) += value;
            }
        }
        let mut per_process: Vec<GpuProcess> = per_pid
            .into_iter()
            .filter(|(_, v)| *v >= 0.5)
            .map(|(pid, percent)| GpuProcess { pid, percent })
            .collect();
        per_process.sort_by(|a, b| b.percent.total_cmp(&a.percent));
        per_process.truncate(200);

        // Windows reports one number per engine per process; the headline figure is the
        // busiest single engine, which is the reading Task Manager uses too.
        (Some(busiest), per_process, Availability::Ok)
    }

    pub fn adapters(&self) -> &[GpuAdapter] {
        &self.adapters
    }
}

/// Instance names look like `pid_1234,luid_0x…,engtype_3D`.
fn parse_pid(instance: &str) -> Option<u32> {
    let after = instance.strip_prefix("pid_")?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
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
        let seen = vec![adapter("NVIDIA GeForce RTX 4070", Some(12_884_901_888), None)];
        assert!(is_duplicate_adapter(&seen, "NVIDIA GeForce RTX 4070", 12_884_901_888));
        assert!(!is_duplicate_adapter(&seen, "NVIDIA GeForce RTX 4070", 4_294_967_296));
        assert!(!is_duplicate_adapter(&seen, "Intel UHD Graphics 770", 0));
        assert!(!is_duplicate_adapter(&[], "NVIDIA GeForce RTX 4070", 12_884_901_888));
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

    #[test]
    fn parse_pid_reads_the_engine_instance_name() {
        assert_eq!(
            parse_pid("pid_1234,luid_0x0000000000008698,engtype_3d"),
            Some(1234)
        );
        assert_eq!(parse_pid("pid_0,luid_0x1,engtype_copy"), Some(0));
        assert_eq!(parse_pid("luid_0x1,engtype_3d"), None);
        assert_eq!(parse_pid("pid_abc,luid_0x1"), None);
    }
}
