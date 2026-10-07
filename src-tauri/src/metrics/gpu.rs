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

pub struct EngineProbe {
    query: PdhQuery,
    index: usize,
    adapters: Vec<GpuAdapter>,
    last_adapter_refresh: std::time::Instant,
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
        // The WARP software rasteriser and the Microsoft Basic Render Driver show up as
        // adapters too; neither is a chip anyone wants a row for.
        const DXGI_ADAPTER_FLAG_SOFTWARE: u32 = 2;
        let name = wide_to_string(&desc.Description);
        let basic = desc.VendorId == 0x1414 || name.contains("Basic Render");
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE != 0 || basic {
            continue;
        }
        // DXGI can hand back the same physical chip twice under different LUIDs, so match
        // on what the row would show rather than on the identifier.
        if out
            .iter()
            .any(|a| a.name == name && a.dedicated_total_bytes == Some(desc.DedicatedVideoMemory as u64))
        {
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
            utilization_percent: None,
        });
    }
    let _ = factory;
    out
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
        let index = query.add(r"\GPU Engine(*)\Utilization Percentage")?;
        query.collect();
        Some(Self {
            query,
            index,
            adapters: enumerate_adapters(),
            last_adapter_refresh: std::time::Instant::now(),
        })
    }

    pub fn refresh(&mut self) -> (Option<f64>, Vec<GpuProcess>, Availability) {
        if self.last_adapter_refresh.elapsed().as_secs() > 300 {
            self.adapters = enumerate_adapters();
            self.last_adapter_refresh = std::time::Instant::now();
        }
        if !self.query.collect() {
            return (None, Vec::new(), Availability::Unavailable);
        }
        let rows = self.query.array(self.index);
        if rows.is_empty() {
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
        (Some(busiest), per_process, Availability::Partial)
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
