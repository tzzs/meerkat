//! Changing how an app runs rather than only whether it runs: Windows' efficiency mode, its
//! scheduling priority, and pausing it outright. Task Manager offers the first two and forgets
//! them the moment the app restarts; Pulse keeps them as rules (rules.rs) and applies them to
//! every process the app starts.
//!
//! Every call opens the process with only the right it needs, so it works for the user's own
//! programs without elevation and is refused, as it should be, for the system's.

use serde::{Deserialize, Serialize};
use windows::core::s;
use windows::Win32::Foundation::{CloseHandle, HANDLE, NTSTATUS};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Threading::{
    OpenProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation, ABOVE_NORMAL_PRIORITY_CLASS,
    BELOW_NORMAL_PRIORITY_CLASS, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION,
    PROCESS_SUSPEND_RESUME,
};

/// The scheduling classes worth offering. Realtime is left out on purpose: it needs elevation
/// and can starve the input stack, which is not a choice a monitor should put one click away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Low,
    BelowNormal,
    Normal,
    AboveNormal,
    High,
}

impl Priority {
    fn class(self) -> PROCESS_CREATION_FLAGS {
        match self {
            Priority::Low => IDLE_PRIORITY_CLASS,
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Normal => NORMAL_PRIORITY_CLASS,
            Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
            Priority::High => HIGH_PRIORITY_CLASS,
        }
    }
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32, rights: PROCESS_ACCESS_RIGHTS) -> Result<Handle, String> {
    unsafe { OpenProcess(rights, false, pid) }.map(Handle).map_err(|e| e.message())
}

/// Windows' efficiency mode, the same pair of calls Task Manager makes: EcoQoS, which lets the
/// scheduler put the process on efficient cores at lower clocks, plus the idle priority class.
/// Turning it off takes EcoQoS off and puts `restore` back.
pub fn set_efficiency(pid: u32, on: bool, restore: Priority) -> Result<(), String> {
    let process = open(pid, PROCESS_SET_INFORMATION)?;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    unsafe {
        SetProcessInformation(
            process.0,
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    }
    .map_err(|e| e.message())?;
    let class = if on { IDLE_PRIORITY_CLASS } else { restore.class() };
    unsafe { SetPriorityClass(process.0, class) }.map_err(|e| e.message())
}

pub fn set_priority(pid: u32, priority: Priority) -> Result<(), String> {
    let process = open(pid, PROCESS_SET_INFORMATION)?;
    unsafe { SetPriorityClass(process.0, priority.class()) }.map_err(|e| e.message())
}

type SuspendFn = unsafe extern "system" fn(HANDLE) -> NTSTATUS;

/// Pauses or resumes every thread of a process. `NtSuspendProcess` is what Resource Monitor's
/// "Suspend process" and Process Explorer use; it is not in the SDK headers, so it is looked up
/// in ntdll by name.
pub fn set_suspended(pid: u32, suspend: bool) -> Result<(), String> {
    let process = open(pid, PROCESS_SUSPEND_RESUME)?;
    let name = if suspend { s!("NtSuspendProcess") } else { s!("NtResumeProcess") };
    let ntdll = unsafe { GetModuleHandleA(s!("ntdll.dll")) }.map_err(|e| e.message())?;
    let address = unsafe { GetProcAddress(ntdll, name) }.ok_or("ntdll does not export it")?;
    // SAFETY: both functions take one process handle and return an NTSTATUS.
    let call: SuspendFn = unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, SuspendFn>(address) };
    let status = unsafe { call(process.0) };
    if status.0 < 0 {
        return Err(format!("NTSTATUS 0x{:08X}", status.0));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calls are tried on a child this test starts and owns, which is the case the app
    /// relies on: a program of the user's own, no elevation.
    #[test]
    fn efficiency_priority_and_suspension_work_on_a_process_we_own() {
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "ping", "-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("cmd.exe starts");
        let pid = child.id();
        set_efficiency(pid, true, Priority::Normal).expect("efficiency on");
        set_efficiency(pid, false, Priority::Normal).expect("efficiency off");
        set_priority(pid, Priority::BelowNormal).expect("priority");
        set_suspended(pid, true).expect("suspend");
        set_suspended(pid, false).expect("resume");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_process_that_is_not_there_is_an_error_not_a_panic() {
        assert!(set_priority(0xFFFF_FFF0, Priority::Normal).is_err());
        assert!(set_suspended(0xFFFF_FFF0, true).is_err());
    }
}
