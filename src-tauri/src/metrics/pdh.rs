//! Minimal PerfData Helper wrapper: one query, several counters, wildcard instances.
//! Lives entirely on the sampler thread, so handles never cross a thread boundary.

use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HQUERY,
    // The status PdhGetFormattedCounterArrayW answers with when it is only reporting the
    // buffer size it needs.
    PDH_MORE_DATA,
};
use windows::core::PCWSTR;

const PDH_CSTATUS_VALID_DATA: u32 = 0x0;
const PDH_CSTATUS_NEW_DATA: u32 = 0x1;

pub struct Counter {
    handle: windows::Win32::System::Performance::PDH_HCOUNTER,
}

pub struct PdhQuery {
    handle: PDH_HQUERY,
    counters: Vec<Counter>,
}

impl PdhQuery {
    pub fn new() -> Option<Self> {
        let mut handle = PDH_HQUERY::default();
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &mut handle) };
        if status != 0 || handle.0.is_null() {
            return None;
        }
        Some(Self { handle, counters: Vec::new() })
    }

    pub fn add(&mut self, path: &str) -> Option<usize> {
        let mut handle = windows::Win32::System::Performance::PDH_HCOUNTER::default();
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let status = unsafe {
            PdhAddEnglishCounterW(self.handle, PCWSTR(wide.as_ptr()), 0, &mut handle)
        };
        if status != 0 {
            return None;
        }
        self.counters.push(Counter { handle });
        Some(self.counters.len() - 1)
    }

    /// Rate counters need two collections before the first value is meaningful.
    pub fn collect(&mut self) -> bool {
        (unsafe { PdhCollectQueryData(self.handle) }) == 0
    }
    pub fn value(&self, index: usize) -> Option<f64> {
        let counter = self.counters.get(index)?;
        let mut value = windows::Win32::System::Performance::PDH_FMT_COUNTERVALUE::default();
        let status =
            unsafe { PdhGetFormattedCounterValue(counter.handle, PDH_FMT_DOUBLE, None, &mut value) };
        if status != 0 {
            return None;
        }
        if value.CStatus != PDH_CSTATUS_VALID_DATA && value.CStatus != PDH_CSTATUS_NEW_DATA {
            return None;
        }
        let v = unsafe { value.Anonymous.doubleValue };
        if v.is_finite() {
            Some(v)
        } else {
            None
        }
    }

    /// Every instance of a wildcard counter, as (instance name, value).
    pub fn array(&self, index: usize) -> Vec<(String, f64)> {
        let Some(counter) = self.counters.get(index) else {
            return Vec::new();
        };
        let mut size = 0u32;
        let mut items = 0u32;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter.handle, PDH_FMT_DOUBLE, &mut size, &mut items, None)
        };
        if status != PDH_MORE_DATA && status != 0 {
            return Vec::new();
        }
        if size == 0 {
            return Vec::new();
        }
        // The instance names live after the item array in the same block, and each item
        // holds an f64, so the buffer has to keep 8-byte alignment.
        let words = (size as usize + std::mem::size_of::<u64>() - 1) / std::mem::size_of::<u64>();
        let mut buffer = vec![0u64; words];
        let ptr = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let status = unsafe {
            PdhGetFormattedCounterArrayW(counter.handle, PDH_FMT_DOUBLE, &mut size, &mut items, Some(ptr))
        };
        if status != 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(items as usize);
        unsafe {
            for i in 0..items as usize {
                let item = &*ptr.add(i);
                let name = pcwstr_to_string(item.szName.0);
                if item.FmtValue.CStatus != PDH_CSTATUS_VALID_DATA
                    && item.FmtValue.CStatus != PDH_CSTATUS_NEW_DATA
                {
                    continue;
                }
                let v = item.FmtValue.Anonymous.doubleValue;
                if v.is_finite() {
                    out.push((name, v));
                }
            }
        }
        out
    }
}

impl Drop for PdhQuery {
    fn drop(&mut self) {
        if !self.handle.0.is_null() {
            unsafe {
                PdhCloseQuery(self.handle);
            }
        }
    }
}

fn pcwstr_to_string(raw: *mut u16) -> String {
    if raw.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *raw.add(len) != 0 {
            len += 1;
            if len > 4096 {
                break;
            }
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(raw, len))
    }
}
