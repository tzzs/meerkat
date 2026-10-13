//! Which Windows services live in which process, so a service host reads as the service it is.
//!
//! On any machine with more than 3.5 GB of memory Windows gives most services a service host of
//! their own, and every one of them is `svchost.exe`. The service control manager says which
//! service each of those processes is running, and the display names it returns are already in
//! the language Windows is shown in, so "Windows 更新" needs no translation table of ours.

use std::collections::HashMap;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_MORE_DATA;
use windows::Win32::System::Services::{
    CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, ENUM_SERVICE_STATUS_PROCESSW,
    ENUM_SERVICE_TYPE, SC_ENUM_PROCESS_INFO, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_ACTIVE,
    SERVICE_WIN32,
};

/// Per-user services (Clipboard, Connected Devices Platform…) run one instance per signed-in
/// user, named after the template with a session suffix: `cbdhsvc_4a1c2`.
const SERVICE_USER_SERVICE: u32 = 0x40;
const SERVICE_USERSERVICE_INSTANCE: u32 = 0x80;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// The short name, `wuauserv`, with any per-user suffix removed so it stays the same across
    /// sign-ins. This is what keys an app row, so history and alerts survive a reboot.
    pub name: String,
    /// What Windows calls it in its own language.
    pub display_name: String,
}

#[derive(Default)]
pub struct ServiceDirectory {
    by_pid: HashMap<u32, Vec<Service>>,
    read_at: Option<Instant>,
}

impl ServiceDirectory {
    pub fn by_pid(&self) -> &HashMap<u32, Vec<Service>> {
        &self.by_pid
    }

    /// Services start and stop on their own schedule, but a new service host is the moment that
    /// matters: it appears with no name. So the list is re-read every half minute, and sooner
    /// when a service host shows up that the last reading did not know about.
    pub fn refresh(&mut self, service_hosts: &[u32]) {
        let age = self.read_at.map(|at| at.elapsed().as_secs());
        let unknown = service_hosts.iter().any(|pid| !self.by_pid.contains_key(pid));
        let due = match age {
            None => true,
            Some(seconds) => seconds >= 30 || (unknown && seconds >= 3),
        };
        if !due {
            return;
        }
        if let Some(map) = read_services() {
            self.by_pid = map;
        }
        self.read_at = Some(Instant::now());
    }
}

fn read_services() -> Option<HashMap<u32, Vec<Service>>> {
    let manager = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) }.ok()?;
    // Asking for the per-user instances as well; an SCM that does not know those types refuses
    // the whole call, so the plain Win32 set is the fallback.
    let with_user = ENUM_SERVICE_TYPE(SERVICE_WIN32.0 | SERVICE_USER_SERVICE | SERVICE_USERSERVICE_INSTANCE);
    let rows = enumerate(manager, with_user).or_else(|| enumerate(manager, SERVICE_WIN32));
    let _ = unsafe { CloseServiceHandle(manager) };

    let mut map: HashMap<u32, Vec<Service>> = HashMap::new();
    for (pid, kind, name, display_name) in rows? {
        if pid == 0 {
            continue;
        }
        let instance = kind & SERVICE_USERSERVICE_INSTANCE != 0;
        let service = Service {
            name: if instance { strip_instance_suffix(&name) } else { name.clone() },
            display_name: if instance { strip_instance_suffix(&display_name) } else { display_name },
        };
        map.entry(pid).or_default().push(service);
    }
    for services in map.values_mut() {
        services.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        services.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    }
    Some(map)
}

/// (pid, service type, name, display name) for every running service of the given types.
fn enumerate(
    manager: windows::Win32::System::Services::SC_HANDLE,
    kinds: ENUM_SERVICE_TYPE,
) -> Option<Vec<(u32, u32, String, String)>> {
    let mut out = Vec::new();
    let mut resume = 0u32;
    // The entries hold pointers, so the buffer is u64-backed to keep them aligned.
    let mut buffer = vec![0u64; 64 * 1024 / 8];
    for _ in 0..64 {
        let mut needed = 0u32;
        let mut returned = 0u32;
        // SAFETY: a byte view of the u64 buffer, the allocation's own length.
        let bytes = unsafe { std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 8) };
        let result = unsafe {
            EnumServicesStatusExW(
                manager,
                SC_ENUM_PROCESS_INFO,
                kinds,
                SERVICE_ACTIVE,
                Some(bytes),
                &mut needed,
                &mut returned,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        let more = match &result {
            Ok(()) => false,
            Err(error) if error.code() == ERROR_MORE_DATA.to_hresult() => true,
            Err(_) => return None,
        };
        let entries = buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for index in 0..returned as usize {
            // SAFETY: the SCM wrote `returned` entries at the start of the buffer.
            let entry = unsafe { &*entries.add(index) };
            let name = unsafe { entry.lpServiceName.to_string() }.unwrap_or_default();
            let display = unsafe { entry.lpDisplayName.to_string() }.unwrap_or_default();
            out.push((
                entry.ServiceStatusProcess.dwProcessId,
                entry.ServiceStatusProcess.dwServiceType.0,
                name,
                display,
            ));
        }
        if !more {
            return Some(out);
        }
        if returned == 0 {
            // Not even one entry fitted: grow to what the SCM asked for.
            let wanted = (needed as usize).max(buffer.len() * 8) + 4096;
            buffer = vec![0u64; wanted.div_ceil(8)];
        }
    }
    Some(out)
}

/// `cbdhsvc_4a1c2` → `cbdhsvc`; `剪贴板用户服务_4a1c2` → `剪贴板用户服务`. Only a short hex tail
/// is a session suffix; anything else is part of the name.
pub fn strip_instance_suffix(name: &str) -> String {
    match name.rsplit_once('_') {
        Some((head, tail))
            if !head.is_empty()
                && (4..=8).contains(&tail.len())
                && tail.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            head.to_string()
        }
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_user_suffixes_come_off_and_real_underscores_stay() {
        assert_eq!(strip_instance_suffix("cbdhsvc_4a1c2"), "cbdhsvc");
        assert_eq!(strip_instance_suffix("剪贴板用户服务_4a1c2"), "剪贴板用户服务");
        assert_eq!(strip_instance_suffix("CDPUserSvc_2b1c4f0"), "CDPUserSvc");
        assert_eq!(strip_instance_suffix("NVDisplay.ContainerLocalSystem"), "NVDisplay.ContainerLocalSystem");
        assert_eq!(strip_instance_suffix("my_service"), "my_service", "not hex");
        assert_eq!(strip_instance_suffix("svc_abc"), "svc_abc", "too short to be a suffix");
        assert_eq!(strip_instance_suffix("_4a1c2"), "_4a1c2");
    }

    /// Any user may list the running services; the RPC endpoint mapper runs in every Windows.
    #[test]
    fn the_live_directory_names_service_hosts() {
        let map = read_services().expect("the SCM answers an ordinary user");
        let all: Vec<&Service> = map.values().flatten().collect();
        assert!(all.len() > 20, "a running Windows has dozens of services");
        assert!(all.iter().any(|s| s.name.eq_ignore_ascii_case("RpcSs")), "RpcSs always runs");
        assert!(all.iter().all(|s| !s.display_name.is_empty()));
    }
}
