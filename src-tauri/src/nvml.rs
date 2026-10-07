//! NVIDIA GPUs through `nvml.dll`: temperature, fan speed and power draw.
//!
//! The library is opened by name when asked and closed again on the way out, so a machine
//! without the NVIDIA driver reads as "no NVML here" instead of failing at start-up with a
//! missing-DLL error.

use std::ffi::c_char;
use std::mem::size_of;
use std::sync::Mutex;

use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

#[derive(Clone, Debug)]
pub struct NvmlDevice {
    pub name: String,
    pub temperature_c: Option<f32>,
    pub fan_percent: Option<f32>,
    pub power_watts: Option<f32>,
    /// Dedicated video memory in use. DXGI's own counter stays at zero on NVIDIA
    /// drivers, so this is the only real figure for it.
    pub memory_used_bytes: Option<u64>,
}

type NvmlReturn = u32;
type DeviceHandle = *mut std::ffi::c_void;

const NVML_SUCCESS: NvmlReturn = 0;
/// `NVML_TEMPERATURE_GPU`, the only sensor worth asking a monitoring app about.
const NVML_TEMPERATURE_GPU: u32 = 0;
const NVML_DEVICE_NAME_BUFFER_SIZE: usize = 96;
const MAX_DEVICES: u32 = 32;

type InitFn = unsafe extern "system" fn() -> NvmlReturn;
type ShutdownFn = unsafe extern "system" fn() -> NvmlReturn;
type DeviceCountFn = unsafe extern "system" fn(*mut u32) -> NvmlReturn;
type HandleByIndexFn = unsafe extern "system" fn(u32, *mut DeviceHandle) -> NvmlReturn;
type DeviceNameFn = unsafe extern "system" fn(DeviceHandle, *mut c_char, u32) -> NvmlReturn;
type TemperatureFn = unsafe extern "system" fn(DeviceHandle, u32, *mut u32) -> NvmlReturn;
type FanSpeedFn = unsafe extern "system" fn(DeviceHandle, *mut u32) -> NvmlReturn;
type PowerUsageFn = unsafe extern "system" fn(DeviceHandle, *mut u32) -> NvmlReturn;
type MemoryFn = unsafe extern "system" fn(DeviceHandle, *mut NvmlMemory) -> NvmlReturn;

/// `nvmlMemory_t` (v1): total, free and used bytes of device memory.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

const _: () = assert!(size_of::<NvmlMemory>() == 24);
/// NVML's own init and shutdown are process-wide, so a second poller must not tear the
/// library down while the first one is still reading through it.
static NVML_LOCK: Mutex<()> = Mutex::new(());

/// `None` when this machine has no usable NVML: no NVIDIA card, `nvml.dll` missing, or the
/// library refusing to initialise.
pub fn devices() -> Option<Vec<NvmlDevice>> {
    let _guard = NVML_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let path = to_wide("nvml.dll");
    let module = unsafe { LoadLibraryW(PCWSTR(path.as_ptr())) }.ok()?;
    let result = unsafe { Api::resolve(module) }.and_then(|api| unsafe { api.poll() });
    let _ = unsafe { FreeLibrary(module) };

    result
}

struct Api {
    init: InitFn,
    shutdown: ShutdownFn,
    device_count: DeviceCountFn,
    handle_by_index: HandleByIndexFn,
    device_name: Option<DeviceNameFn>,
    temperature: Option<TemperatureFn>,
    fan_speed: Option<FanSpeedFn>,
    power_usage: Option<PowerUsageFn>,
    memory: Option<MemoryFn>,
}

impl Api {
    unsafe fn resolve(module: HMODULE) -> Option<Self> {
        // The suffixed exports are the ABIs a current driver implements; the bare names are
        // only reachable on a driver older than the suffix, where they mean the same thing.
        Some(Self {
            init: lookup(module, &[b"nvmlInit_v2\0", b"nvmlInit\0"])?,
            shutdown: lookup(module, &[b"nvmlShutdown\0"])?,
            device_count: lookup(
                module,
                &[b"nvmlDeviceGetCount_v2\0", b"nvmlDeviceGetCount\0"],
            )?,
            handle_by_index: lookup(
                module,
                &[
                    b"nvmlDeviceGetHandleByIndex_v2\0",
                    b"nvmlDeviceGetHandleByIndex\0",
                ],
            )?,
            device_name: lookup(module, &[b"nvmlDeviceGetName\0"]),
            temperature: lookup(module, &[b"nvmlDeviceGetTemperature\0"]),
            fan_speed: lookup(module, &[b"nvmlDeviceGetFanSpeed\0"]),
            power_usage: lookup(module, &[b"nvmlDeviceGetPowerUsage\0"]),
            memory: lookup(module, &[b"nvmlDeviceGetMemoryInfo\0"]),
        })
    }

    unsafe fn poll(&self) -> Option<Vec<NvmlDevice>> {
        if (self.init)() != NVML_SUCCESS {
            return None;
        }
        let devices = self.devices();
        let _ = (self.shutdown)();
        Some(devices)
    }

    unsafe fn devices(&self) -> Vec<NvmlDevice> {
        let mut count = 0u32;
        if (self.device_count)(&mut count) != NVML_SUCCESS {
            return Vec::new();
        }
        // A count out of NVML is still just a number; cap it before it becomes an allocation.
        let mut out = Vec::new();
        for index in 0..count.min(MAX_DEVICES) {
            let mut handle: DeviceHandle = std::ptr::null_mut();
            if (self.handle_by_index)(index, &mut handle) != NVML_SUCCESS || handle.is_null() {
                continue;
            }
            out.push(NvmlDevice {
                name: self.read_name(handle),
                temperature_c: self.read_temperature(handle),
                fan_percent: self.read_fan_speed(handle),
                power_watts: self.read_power_usage(handle),
                memory_used_bytes: self.read_memory_used(handle),
            });
        }
        out
    }

    unsafe fn read_memory_used(&self, device: DeviceHandle) -> Option<u64> {
        let get = self.memory?;
        let mut memory = NvmlMemory::default();
        if get(device, &mut memory) != NVML_SUCCESS || memory.total == 0 {
            return None;
        }
        Some(memory.used)
    }

    unsafe fn read_name(&self, device: DeviceHandle) -> String {
        let Some(get) = self.device_name else {
            return String::new();
        };
        let mut buffer: [c_char; NVML_DEVICE_NAME_BUFFER_SIZE] = [0; NVML_DEVICE_NAME_BUFFER_SIZE];
        if get(device, buffer.as_mut_ptr(), buffer.len() as u32) != NVML_SUCCESS {
            return String::new();
        }
        read_ascii(&buffer).unwrap_or_default()
    }

    unsafe fn read_temperature(&self, device: DeviceHandle) -> Option<f32> {
        let get = self.temperature?;
        let mut raw = 0u32;
        if get(device, NVML_TEMPERATURE_GPU, &mut raw) != NVML_SUCCESS {
            return None;
        }
        Some(raw as f32)
    }

    /// A card with no tachometer answers `NVML_ERROR_NOT_SUPPORTED` (3) rather than a
    /// number, which is an absence and not a failure — treated here like any other non-zero
    /// return.
    unsafe fn read_fan_speed(&self, device: DeviceHandle) -> Option<f32> {
        let get = self.fan_speed?;
        let mut raw = 0u32;
        if get(device, &mut raw) != NVML_SUCCESS {
            return None;
        }
        Some(raw as f32)
    }

    unsafe fn read_power_usage(&self, device: DeviceHandle) -> Option<f32> {
        let get = self.power_usage?;
        let mut milli = 0u32;
        if get(device, &mut milli) != NVML_SUCCESS {
            return None;
        }
        // NVML reports power draw in milliwatts.
        Some(milli as f32 / 1000.0)
    }
}

/// Takes the first name the driver exports and reinterprets its address as `T`, which has to
/// be a function pointer of the same width.
unsafe fn lookup<T: Copy>(module: HMODULE, names: &[&[u8]]) -> Option<T> {
    assert!(size_of::<T>() == size_of::<usize>());
    let mut address = 0usize;
    for name in names {
        let found = unsafe { GetProcAddress(module, PCSTR(name.as_ptr())) };
        address = found.map_or(0, |proc| proc as usize);
        if address != 0 {
            break;
        }
    }
    if address == 0 {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy(&address) })
}

fn read_ascii(raw: &[c_char]) -> Option<String> {
    let end = raw.iter().position(|c| *c == 0).unwrap_or(raw.len());
    let bytes: Vec<u8> = raw[..end].iter().map(|c| *c as u8).collect();
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    String::from_utf8(bytes)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
