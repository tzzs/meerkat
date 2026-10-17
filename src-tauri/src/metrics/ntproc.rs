//! Every process on the machine, read in one call that needs no handle and no rights.
//!
//! sysinfo reads a process's figures through a handle it opens on that process, and an ordinary
//! user cannot open most of Session 0. Windows Update, Defender, the search indexer and every
//! service host came back as zeros and were folded into "processes Pulse cannot open", which hid
//! exactly the programs that make a machine slow for no visible reason. Measured on the machine
//! this was written on: 173 Session 0 processes, none readable through a handle, while Defender
//! alone held 936 MB and had burned 79 minutes of CPU since boot.
//!
//! `NtQuerySystemInformation(SystemProcessInformation)` is what Task Manager reads. It returns
//! CPU time, memory and I/O for every process from the kernel's own table, with no access check
//! against any of them. `SystemProcessIdInformation` gives the executable's path the same way, so
//! a service can be named, given its icon, and grouped like any other program.
//!
//! What still needs a handle is asked once per process and remembered: the command line (only
//! the user's own processes answer) and whether this user is allowed to end it.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation, SYSTEM_INFORMATION_CLASS,
};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation, ProcessCommandLineInformation};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HLOCAL};
use windows::Win32::Storage::FileSystem::{GetLogicalDrives, QueryDosDeviceW};
use windows::Win32::System::Threading::{
    IsWow64Process, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ,
};
use windows::Win32::UI::Shell::CommandLineToArgvW;

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

/// Not in the SDK headers, stable since Vista, and what every process viewer uses to name a
/// process it cannot open.
const SYSTEM_PROCESS_ID_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(88);

/// FILETIME counts 100 ns ticks from 1601; Unix time starts 11 644 473 600 seconds later.
const FILETIME_UNIX_OFFSET_MS: u64 = 11_644_473_600_000;

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    /// In bytes, not characters, and without a terminating null.
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

/// `SYSTEM_PROCESS_INFORMATION` as the kernel writes it. The SDK publishes this with most of
/// the fields hidden as `Reserved`; the names here are the ones Process Hacker and the WDK use,
/// and the layout below was checked field by field against a live table on this machine (the
/// working set at 0x90, private bytes at 0xC8, the transfer counts at 0xE8 and 0xF0).
#[repr(C)]
#[derive(Clone, Copy)]
struct SystemProcessRecord {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<SystemProcessRecord>() == 0x100);

#[repr(C)]
struct SystemProcessIdRecord {
    process_id: usize,
    image_name: UnicodeString,
}

/// One row of the kernel's process table.
#[derive(Clone, Debug, PartialEq)]
pub struct NtProcess {
    pub pid: u32,
    pub parent_pid: u32,
    /// The image name, `chrome.exe`; the pseudo processes have bare names such as `Registry`.
    pub name: String,
    pub session_id: u32,
    /// FILETIME ticks. Together with the pid this is what tells a process from a later one that
    /// was handed the same number.
    pub create_time: u64,
    /// User plus kernel time, in 100 ns ticks.
    pub cpu_time: u64,
    pub working_set: u64,
    pub private_bytes: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub threads: u32,
    pub handles: u32,
}

impl NtProcess {
    pub fn started_at_ms(&self) -> Option<u64> {
        filetime_to_unix_ms(self.create_time)
    }
}

pub fn filetime_to_unix_ms(ticks: u64) -> Option<u64> {
    (ticks / 10_000).checked_sub(FILETIME_UNIX_OFFSET_MS).filter(|ms| *ms > 0)
}

/// Walks a buffer `NtQuerySystemInformation` filled. Every read is bounds checked against the
/// buffer, including the image name, whose pointer the kernel aims back into the same buffer:
/// a table that is malformed in any way ends the walk instead of reading someone else's memory.
fn parse_table(buffer: &[u8]) -> Vec<NtProcess> {
    let record_size = std::mem::size_of::<SystemProcessRecord>();
    let start = buffer.as_ptr() as usize;
    let end = start + buffer.len();
    let mut out = Vec::with_capacity(512);
    let mut offset = 0usize;
    loop {
        let Some(limit) = offset.checked_add(record_size) else { break };
        if limit > buffer.len() {
            break;
        }
        // SAFETY: `offset + record_size` is inside the buffer, checked just above.
        let record: SystemProcessRecord =
            unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset).cast()) };
        let pid = record.unique_process_id as u32;
        // Pid 0 is the idle loop. Its "CPU time" is the time nothing ran, and listing it would
        // put the quietest moment of the machine at the top of the busiest list.
        if pid != 0 {
            out.push(NtProcess {
                pid,
                parent_pid: record.inherited_from_unique_process_id as u32,
                name: read_name(&record.image_name, start, end),
                session_id: record.session_id,
                create_time: record.create_time.max(0) as u64,
                cpu_time: (record.user_time.max(0) as u64).saturating_add(record.kernel_time.max(0) as u64),
                working_set: record.working_set_size as u64,
                private_bytes: record.private_page_count as u64,
                read_bytes: record.read_transfer_count.max(0) as u64,
                write_bytes: record.write_transfer_count.max(0) as u64,
                threads: record.number_of_threads,
                handles: record.handle_count,
            });
        }
        if record.next_entry_offset == 0 {
            break;
        }
        offset += record.next_entry_offset as usize;
    }
    out
}

fn read_name(name: &UnicodeString, start: usize, end: usize) -> String {
    let address = name.buffer as usize;
    let bytes = name.length as usize;
    if name.buffer.is_null() || bytes == 0 || address < start || address + bytes > end || address % 2 != 0 {
        return String::new();
    }
    // SAFETY: the whole span lies inside the caller's buffer and is u16 aligned.
    let units = unsafe { std::slice::from_raw_parts(name.buffer as *const u16, bytes / 2) };
    String::from_utf16_lossy(units)
}

/// The reusable buffer the table is read into. A machine has a few hundred processes; the
/// buffer grows to fit once and is then reused every tick.
struct ProcessTable {
    buffer: Vec<u64>,
}

impl ProcessTable {
    fn new() -> Self {
        Self { buffer: vec![0u64; (512 * 1024) / 8] }
    }

    fn read(&mut self) -> Option<Vec<NtProcess>> {
        for _ in 0..4 {
            let capacity = self.buffer.len() * 8;
            let mut needed = 0u32;
            let status = unsafe {
                NtQuerySystemInformation(
                    SystemProcessInformation,
                    self.buffer.as_mut_ptr().cast(),
                    capacity as u32,
                    &mut needed,
                )
            };
            if status.0 == STATUS_INFO_LENGTH_MISMATCH {
                // Processes start between the size query and the read, so leave room for them.
                let next = (needed as usize).max(capacity) + 256 * 1024;
                self.buffer = vec![0u64; next.div_ceil(8)];
                continue;
            }
            if status.0 < 0 {
                return None;
            }
            // SAFETY: a u64 buffer viewed as its bytes; the length is the allocation's own.
            let bytes = unsafe { std::slice::from_raw_parts(self.buffer.as_ptr().cast::<u8>(), capacity) };
            return Some(parse_table(bytes));
        }
        None
    }
}

/// The NT path of a process's executable, `\Device\HarddiskVolume3\Windows\System32\svchost.exe`,
/// for any process at all. `None` for the pseudo processes (System, Registry, Memory
/// Compression), which have no file behind them.
fn nt_image_path(pid: u32) -> Option<String> {
    let mut capacity_bytes = 520usize;
    for _ in 0..3 {
        let mut name = vec![0u16; capacity_bytes / 2];
        let mut record = SystemProcessIdRecord {
            process_id: pid as usize,
            image_name: UnicodeString {
                length: 0,
                maximum_length: (name.len() * 2).min(u16::MAX as usize - 1) as u16,
                buffer: name.as_mut_ptr(),
            },
        };
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESS_ID_INFORMATION,
                (&mut record as *mut SystemProcessIdRecord).cast(),
                std::mem::size_of::<SystemProcessIdRecord>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status.0 == STATUS_INFO_LENGTH_MISMATCH {
            // The kernel says how long the name is by way of MaximumLength.
            let wanted = record.image_name.maximum_length as usize;
            if wanted <= capacity_bytes || wanted >= u16::MAX as usize {
                return None;
            }
            capacity_bytes = wanted + 2;
            continue;
        }
        // For the System process the call succeeds and hands back a null buffer.
        if status.0 < 0 || record.image_name.buffer.is_null() || record.image_name.length == 0 {
            return None;
        }
        let units = (record.image_name.length as usize / 2).min(name.len());
        let path = String::from_utf16_lossy(&name[..units]);
        return path.starts_with('\\').then_some(path);
    }
    None
}

const SYSTEM_PAGE_FILE_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(18);

/// `SYSTEM_PAGEFILE_INFORMATION`: one record per page file, sizes in pages.
#[repr(C)]
#[derive(Clone, Copy)]
struct PageFileRecord {
    next_entry_offset: u32,
    total_size: u32,
    total_in_use: u32,
    peak_usage: u32,
    page_file_name: UnicodeString,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<PageFileRecord>() == 32);

/// The page files' total size and how much of it is in use, in bytes, summed over every page
/// file. The Memory tab used to ask PDH for `\Paging File(_ Total)\Private Bytes`, a counter that
/// does not exist, and so reported 0 B in use on a machine with 3.7 GB paged out.
pub fn page_files() -> Option<(u64, u64)> {
    const PAGE: u64 = 4096;
    let mut buffer = vec![0u64; 512];
    let mut needed = 0u32;
    let status = unsafe {
        NtQuerySystemInformation(SYSTEM_PAGE_FILE_INFORMATION, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed)
    };
    if status.0 < 0 {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
    let (mut total, mut used) = (0u64, 0u64);
    let mut offset = 0usize;
    // An empty answer (no page file at all) is a zeroed first record.
    while offset + std::mem::size_of::<PageFileRecord>() <= bytes.len() {
        let record: PageFileRecord = unsafe { std::ptr::read_unaligned(bytes.as_ptr().add(offset).cast()) };
        total += u64::from(record.total_size) * PAGE;
        used += u64::from(record.total_in_use) * PAGE;
        if record.next_entry_offset == 0 {
            break;
        }
        offset += record.next_entry_offset as usize;
    }
    Some((total, used))
}

/// How the kernel's device names map onto drive letters: `\Device\HarddiskVolume3` → `C:`.
#[derive(Default)]
struct DeviceMap {
    pairs: Vec<(String, String)>,
    read_at: Option<Instant>,
}

impl DeviceMap {
    /// Drives come and go (a USB stick, a mounted image), so the map is re-read now and then
    /// and immediately whenever a path names a device it does not know.
    fn dos_path(&mut self, nt: &str) -> String {
        let stale = self.read_at.is_none_or(|at| at.elapsed().as_secs() > 120);
        if stale {
            self.reload();
        }
        if let Some(path) = to_dos_path(nt, &self.pairs) {
            return path;
        }
        if !stale && self.read_at.is_some_and(|at| at.elapsed().as_secs() > 5) {
            self.reload();
            if let Some(path) = to_dos_path(nt, &self.pairs) {
                return path;
            }
        }
        nt.to_string()
    }

    fn reload(&mut self) {
        self.pairs = read_device_map();
        self.read_at = Some(Instant::now());
    }
}

fn read_device_map() -> Vec<(String, String)> {
    let drives = unsafe { GetLogicalDrives() };
    let mut pairs = Vec::new();
    for index in 0..26u8 {
        if drives & (1 << index) == 0 {
            continue;
        }
        let letter = format!("{}:", (b'A' + index) as char);
        let wide: Vec<u16> = letter.encode_utf16().chain(std::iter::once(0)).collect();
        let mut target = [0u16; 1024];
        let written = unsafe { QueryDosDeviceW(PCWSTR(wide.as_ptr()), Some(&mut target)) } as usize;
        if written == 0 {
            continue;
        }
        let end = target.iter().position(|c| *c == 0).unwrap_or(written.min(target.len()));
        let device = String::from_utf16_lossy(&target[..end]);
        // A SUBST drive points at another drive's folder rather than at a device; its paths
        // already come back under the real drive.
        if device.starts_with(r"\Device\") {
            pairs.push((device, letter));
        }
    }
    pairs
}

/// `\Device\HarddiskVolume3\Windows\notepad.exe` → `C:\Windows\notepad.exe`. `None` when the
/// device is not one the map knows.
fn to_dos_path(nt: &str, pairs: &[(String, String)]) -> Option<String> {
    for (device, letter) in pairs {
        if nt.len() > device.len()
            && nt.is_char_boundary(device.len())
            && nt[..device.len()].eq_ignore_ascii_case(device)
            && nt[device.len()..].starts_with('\\')
        {
            return Some(format!("{letter}{}", &nt[device.len()..]));
        }
    }
    if let Some(rest) = nt.strip_prefix(r"\Device\Mup\") {
        return Some(format!(r"\\{rest}"));
    }
    nt.strip_prefix(r"\??\").map(str::to_string)
}

/// The command line of a process this user owns. Others refuse the handle, and that is fine:
/// nothing in Pulse depends on reading a service's arguments.
fn command_line(pid: u32) -> Option<Vec<String>> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut needed = 0u32;
    let _ = unsafe {
        NtQueryInformationProcess(handle, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut needed)
    };
    let result = if needed == 0 || needed > 1 << 20 {
        None
    } else {
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        let capacity = buffer.len() * 8;
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                ProcessCommandLineInformation,
                buffer.as_mut_ptr().cast(),
                capacity as u32,
                &mut needed,
            )
        };
        if status.0 < 0 {
            None
        } else {
            // SAFETY: a u64 buffer viewed as its bytes.
            let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), capacity) };
            let header: UnicodeString = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
            let start = bytes.as_ptr() as usize;
            let text = read_name(&header, start, start + bytes.len());
            Some(split_command_line(&text))
        }
    };
    let _ = unsafe { CloseHandle(handle) };
    result
}

/// The shell's own splitting rules, quotes and backslashes included.
fn split_command_line(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        // CommandLineToArgvW answers an empty string with the path of the calling program.
        return Vec::new();
    }
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut count = 0i32;
    let argv = unsafe { CommandLineToArgvW(PCWSTR(wide.as_ptr()), &mut count) };
    if argv.is_null() {
        return vec![text.to_string()];
    }
    let mut out = Vec::with_capacity(count.max(0) as usize);
    for index in 0..count.max(0) as usize {
        let arg = unsafe { *argv.add(index) };
        out.push(unsafe { arg.to_string() }.unwrap_or_default());
    }
    let _ = unsafe { LocalFree(Some(HLOCAL(argv.cast()))) };
    out
}

/// `PROCESS_BASIC_INFORMATION`, declared here because the SDK's version drags in the whole PEB
/// type for the one pointer this reads.
#[repr(C)]
#[derive(Default)]
struct BasicInformation {
    exit_status: i32,
    peb: usize,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

/// Where these live in a 64-bit process: `PEB.ProcessParameters`, and in the parameters,
/// `CurrentDirectory.DosPath`. A 32-bit process keeps a 32-bit PEB and is skipped.
const PEB_PROCESS_PARAMETERS: usize = 0x20;
const PARAMETERS_CURRENT_DIRECTORY: usize = 0x38;

/// The working directory of a process this user owns, read out of its own memory the way
/// Process Explorer does. Only asked of runtimes (node, python…), whose working directory is
/// often the only clue to which project they are running.
fn current_directory(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, false, pid) }.ok()?;
    let read = || -> Option<String> {
        let mut wow64 = windows::core::BOOL(0);
        unsafe { IsWow64Process(handle, &mut wow64) }.ok()?;
        if wow64.as_bool() {
            return None;
        }
        let mut basic = BasicInformation::default();
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                ProcessBasicInformation,
                (&mut basic as *mut BasicInformation).cast(),
                std::mem::size_of::<BasicInformation>() as u32,
                std::ptr::null_mut(),
            )
        };
        if status.0 < 0 || basic.peb == 0 {
            return None;
        }
        let mut parameters = 0usize;
        unsafe {
            ReadProcessMemory(
                handle,
                (basic.peb + PEB_PROCESS_PARAMETERS) as *const core::ffi::c_void,
                (&mut parameters as *mut usize).cast(),
                std::mem::size_of::<usize>(),
                None,
            )
        }
        .ok()?;
        let mut path = UnicodeString { length: 0, maximum_length: 0, buffer: std::ptr::null_mut() };
        unsafe {
            ReadProcessMemory(
                handle,
                (parameters + PARAMETERS_CURRENT_DIRECTORY) as *const core::ffi::c_void,
                (&mut path as *mut UnicodeString).cast(),
                std::mem::size_of::<UnicodeString>(),
                None,
            )
        }
        .ok()?;
        if path.length == 0 || path.length % 2 != 0 || path.buffer.is_null() {
            return None;
        }
        let mut units = vec![0u16; path.length as usize / 2];
        unsafe {
            ReadProcessMemory(
                handle,
                path.buffer as *const core::ffi::c_void,
                units.as_mut_ptr().cast(),
                path.length as usize,
                None,
            )
        }
        .ok()?;
        Some(String::from_utf16_lossy(&units))
    };
    let result = read();
    let _ = unsafe { CloseHandle(handle) };
    result
}

fn can_terminate(pid: u32) -> bool {
    match unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) } {
        Ok(handle) => {
            let _ = unsafe { CloseHandle(handle) };
            true
        }
        Err(_) => false,
    }
}

/// What does not change over a process's life, asked for once.
#[derive(Clone, Debug, Default)]
pub struct Identity {
    /// The executable as a DOS path, or empty for the pseudo processes.
    pub exe: String,
    pub args: Vec<String>,
    /// Whether this user may end it. Asked of Windows rather than guessed from the session.
    pub can_terminate: bool,
    /// For a runtime (node, python…): the project folder it is working for.
    pub project: Option<String>,
}

/// A process as one tick sees it.
#[derive(Clone, Debug)]
pub struct Sampled {
    pub process: NtProcess,
    pub identity: Identity,
    /// Percent of one core, so a process keeping two cores busy reads 200%; the settings decide
    /// later whether that is shown as is or divided by the core count.
    pub cpu_percent: f64,
}

/// A process instance: the pid alone is not enough, because Windows hands pids out again.
type InstanceKey = (u32, u64);

pub struct Processes {
    table: ProcessTable,
    devices: DeviceMap,
    known: HashMap<InstanceKey, Identity>,
    previous_cpu: HashMap<InstanceKey, u64>,
    previous_at: Option<Instant>,
}

impl Default for Processes {
    fn default() -> Self {
        Self {
            table: ProcessTable::new(),
            devices: DeviceMap::default(),
            known: HashMap::new(),
            previous_cpu: HashMap::new(),
            previous_at: None,
        }
    }
}

impl Processes {
    /// One reading of every process. Empty only if the kernel refused the table outright,
    /// which no supported Windows does.
    pub fn sample(&mut self) -> Vec<Sampled> {
        let Some(table) = self.table.read() else {
            return Vec::new();
        };
        let now = Instant::now();
        let elapsed_ticks = self
            .previous_at
            .map(|at| now.duration_since(at).as_nanos() as f64 / 100.0)
            .unwrap_or(0.0);
        self.previous_at = Some(now);

        let mut live: HashSet<InstanceKey> = HashSet::with_capacity(table.len());
        let mut next_cpu: HashMap<InstanceKey, u64> = HashMap::with_capacity(table.len());
        let mut out = Vec::with_capacity(table.len());
        for process in table {
            let key = (process.pid, process.create_time);
            live.insert(key);
            next_cpu.insert(key, process.cpu_time);
            let cpu_percent = cpu_percent(self.previous_cpu.get(&key).copied(), process.cpu_time, elapsed_ticks);
            let identity = match self.known.get(&key) {
                Some(known) => known.clone(),
                None => {
                    let args = command_line(process.pid).unwrap_or_default();
                    let project = if super::projects::is_runtime(&process.name) {
                        let cwd = current_directory(process.pid);
                        let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
                        super::projects::project_root(
                            &args,
                            cwd.as_deref(),
                            &super::projects::has_project_file,
                            home.as_deref(),
                        )
                        .map(|root| root.to_string_lossy().into_owned())
                    } else {
                        None
                    };
                    let identity = Identity {
                        exe: nt_image_path(process.pid)
                            .map(|nt| self.devices.dos_path(&nt))
                            .unwrap_or_default(),
                        args,
                        can_terminate: can_terminate(process.pid),
                        project,
                    };
                    self.known.insert(key, identity.clone());
                    identity
                }
            };
            out.push(Sampled { process, identity, cpu_percent });
        }
        self.known.retain(|key, _| live.contains(key));
        self.previous_cpu = next_cpu;
        out
    }
}

/// Percent of one core over the interval. The first sight of a process has nothing to compare
/// against, and reads 0 rather than a lifetime average that would look like a spike.
fn cpu_percent(previous: Option<u64>, current: u64, elapsed_ticks: f64) -> f64 {
    match previous {
        Some(before) if elapsed_ticks > 0.0 => current.saturating_sub(before) as f64 / elapsed_ticks * 100.0,
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// tests (pure logic only, plus one read of the live table that any user may make)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pid: usize, next: u32, name: &[u16], name_at: usize) -> SystemProcessRecord {
        SystemProcessRecord {
            next_entry_offset: next,
            number_of_threads: 3,
            working_set_private_size: 0,
            hard_fault_count: 0,
            number_of_threads_high_watermark: 0,
            cycle_time: 0,
            create_time: 133_000_000_000_000_000,
            user_time: 20_000_000,
            kernel_time: 5_000_000,
            image_name: UnicodeString {
                length: (name.len() * 2) as u16,
                maximum_length: (name.len() * 2) as u16,
                buffer: name_at as *mut u16,
            },
            base_priority: 8,
            unique_process_id: pid,
            inherited_from_unique_process_id: 4,
            handle_count: 10,
            session_id: 1,
            unique_process_key: 0,
            peak_virtual_size: 0,
            virtual_size: 0,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 50 << 20,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
            private_page_count: 40 << 20,
            read_operation_count: 0,
            write_operation_count: 0,
            other_operation_count: 0,
            read_transfer_count: 7_000,
            write_transfer_count: 9_000,
            other_transfer_count: 0,
        }
    }

    /// A buffer laid out the way the kernel lays it out: records chained by offset, each image
    /// name stored after its record and pointed at by address.
    fn table(entries: &[(usize, &str)]) -> Vec<u64> {
        let size = std::mem::size_of::<SystemProcessRecord>();
        let stride = size + 64;
        let mut buffer = vec![0u64; (stride * entries.len()).div_ceil(8)];
        let base = buffer.as_mut_ptr() as usize;
        for (index, (pid, name)) in entries.iter().enumerate() {
            let at = index * stride;
            let wide: Vec<u16> = name.encode_utf16().collect();
            let next = if index + 1 == entries.len() { 0 } else { stride as u32 };
            let entry = record(*pid, next, &wide, base + at + size);
            unsafe {
                std::ptr::write_unaligned((base + at) as *mut SystemProcessRecord, entry);
                std::ptr::copy_nonoverlapping(wide.as_ptr(), (base + at + size) as *mut u16, wide.len());
            }
        }
        buffer
    }

    fn bytes(buffer: &[u64]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast(), buffer.len() * 8) }
    }

    #[test]
    fn the_walk_follows_the_offsets_and_skips_the_idle_loop() {
        let buffer = table(&[(0, ""), (4, "System"), (1234, "chrome.exe")]);
        let rows = parse_table(bytes(&buffer));
        assert_eq!(rows.len(), 2, "pid 0 is idle time, not a process");
        assert_eq!(rows[0].pid, 4);
        assert_eq!(rows[0].name, "System");
        assert_eq!(rows[1].name, "chrome.exe");
        assert_eq!(rows[1].cpu_time, 25_000_000, "user plus kernel");
        assert_eq!(rows[1].working_set, 50 << 20);
        assert_eq!(rows[1].private_bytes, 40 << 20);
        assert_eq!((rows[1].read_bytes, rows[1].write_bytes), (7_000, 9_000));
        assert_eq!(rows[1].parent_pid, 4);
    }

    #[test]
    fn a_name_pointing_outside_the_buffer_is_dropped_not_read() {
        let mut buffer = table(&[(77, "evil.exe")]);
        // Aim the name somewhere that is certainly not this buffer.
        let mut entry: SystemProcessRecord = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast()) };
        entry.image_name.buffer = 0x10 as *mut u16;
        unsafe { std::ptr::write_unaligned(buffer.as_mut_ptr().cast(), entry) };
        let rows = parse_table(bytes(&buffer));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "", "an unverifiable name is left empty");
    }

    #[test]
    fn a_truncated_buffer_ends_the_walk() {
        let buffer = table(&[(10, "a.exe"), (11, "b.exe")]);
        let size = std::mem::size_of::<SystemProcessRecord>();
        // Cut the buffer in the middle of the second record.
        let rows = parse_table(&bytes(&buffer)[..size + 64 + size / 2]);
        assert_eq!(rows.len(), 1, "the half record is not read");
    }

    #[test]
    fn device_paths_become_drive_paths() {
        let pairs = vec![
            (r"\Device\HarddiskVolume3".to_string(), "C:".to_string()),
            (r"\Device\HarddiskVolume12".to_string(), "E:".to_string()),
        ];
        assert_eq!(
            to_dos_path(r"\Device\HarddiskVolume3\Windows\System32\svchost.exe", &pairs).as_deref(),
            Some(r"C:\Windows\System32\svchost.exe")
        );
        // Volume1 is a prefix of Volume12; the path separator is what tells them apart.
        assert_eq!(
            to_dos_path(r"\Device\HarddiskVolume12\games\x.exe", &pairs).as_deref(),
            Some(r"E:\games\x.exe")
        );
        assert_eq!(to_dos_path(r"\Device\HarddiskVolume1\x.exe", &pairs), None);
        assert_eq!(
            to_dos_path(r"\device\harddiskvolume3\a.exe", &pairs).as_deref(),
            Some(r"C:\a.exe"),
            "device names compare without case"
        );
        assert_eq!(
            to_dos_path(r"\Device\Mup\server\share\tool.exe", &pairs).as_deref(),
            Some(r"\\server\share\tool.exe")
        );
        assert_eq!(to_dos_path(r"\??\C:\x.exe", &pairs).as_deref(), Some(r"C:\x.exe"));
    }

    #[test]
    fn cpu_is_a_share_of_one_core_over_the_interval() {
        // One second is 10 000 000 ticks; half a second of CPU in it is half a core.
        assert_eq!(cpu_percent(Some(0), 5_000_000, 10_000_000.0), 50.0);
        assert_eq!(cpu_percent(Some(0), 20_000_000, 10_000_000.0), 200.0, "two cores read 200%");
        assert_eq!(cpu_percent(None, 99_000_000, 10_000_000.0), 0.0, "first sight is not a spike");
        assert_eq!(cpu_percent(Some(10), 5, 10_000_000.0), 0.0, "a counter never runs backwards");
        assert_eq!(cpu_percent(Some(0), 5, 0.0), 0.0);
    }

    #[test]
    fn filetimes_convert_to_unix_milliseconds() {
        // 2024-01-01T00:00:00Z.
        assert_eq!(filetime_to_unix_ms(133_485_408_000_000_000), Some(1_704_067_200_000));
        assert_eq!(filetime_to_unix_ms(0), None, "processes started at boot report no time");
    }

    /// Checked against `Win32_PageFileUsage` on this machine (25 GB allocated, 3.7 GB in use).
    #[test]
    fn page_file_usage_is_read_and_is_never_more_than_the_size() {
        let (total, used) = page_files().expect("any user may ask");
        assert!(used <= total);
        if total > 0 {
            assert!(total >= 16 << 20, "a page file is at least a few megabytes");
        }
    }

    #[test]
    fn command_lines_split_the_way_the_shell_splits_them() {
        assert_eq!(
            split_command_line(r#""C:\Program Files\nodejs\node.exe" vite.js --port 1420"#),
            vec![r"C:\Program Files\nodejs\node.exe", "vite.js", "--port", "1420"]
        );
        assert!(split_command_line("   ").is_empty());
    }

    /// The live table is readable by any user; if this fails, nothing in the app list works.
    #[test]
    fn the_live_table_includes_this_process_with_its_own_path() {
        let mut processes = Processes::default();
        let rows = processes.sample();
        assert!(rows.len() > 10, "a running Windows has more than a handful of processes");
        let me = std::process::id();
        let mine = rows.iter().find(|r| r.process.pid == me).expect("this test process is listed");
        let exe = std::env::current_exe().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(mine.identity.exe.to_lowercase(), exe, "the path comes back as a drive path");
        assert!(mine.identity.can_terminate, "a process may end itself");
        assert!(mine.process.working_set > 0);
        let here = std::env::current_dir().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(
            current_directory(me).map(|d| d.trim_end_matches('\\').to_lowercase()),
            Some(here.trim_end_matches('\\').to_string()),
            "a process's working directory is read out of its own memory"
        );
        assert!(rows.iter().any(|r| r.process.pid == 4 && r.identity.exe.is_empty()), "System has no file");
    }
}
