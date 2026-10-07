//! Battery: live state from GetSystemPowerStatus, health figures from the
//! report `powercfg` can already generate.

use crate::models::Availability;
use std::path::PathBuf;
use std::process::Command;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

pub struct LivePower {
    pub has_battery: bool,
    pub on_ac_power: bool,
    pub charge_percent: Option<f64>,
    pub is_charging: bool,
    pub is_critical: bool,
    pub remaining_seconds: Option<u64>,
}

pub fn live() -> LivePower {
    let mut status = SYSTEM_POWER_STATUS::default();
    let ok = unsafe { GetSystemPowerStatus(&mut status) }.is_ok();
    if !ok {
        return LivePower {
            has_battery: false,
            on_ac_power: true,
            charge_percent: None,
            is_charging: false,
            is_critical: false,
            remaining_seconds: None,
        };
    }
    let no_battery = status.BatteryFlag & 8 != 0;
    let charge = if status.BatteryLifePercent == 255 {
        None
    } else {
        Some(status.BatteryLifePercent as f64)
    };
    let remaining = if status.BatteryLifeTime == u32::MAX {
        None
    } else {
        Some(status.BatteryLifeTime as u64)
    };
    LivePower {
        has_battery: !no_battery && status.BatteryLifePercent != 255,
        on_ac_power: status.ACLineStatus != 0,
        charge_percent: charge,
        is_charging: status.BatteryFlag & 4 != 0,
        is_critical: status.BatteryFlag & 2 != 0,
        remaining_seconds: remaining,
    }
}

#[derive(Clone, Default)]
pub struct Health {
    pub design_capacity_wh: Option<f64>,
    pub full_charge_capacity_wh: Option<f64>,
    pub cycle_count: Option<u32>,
}

/// `powercfg /batteryreport /xml` is the only user-mode window Windows gives us on
/// design capacity, full charge capacity and cycle count.
pub fn read_health() -> (Health, Availability) {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "pulse-battery-{}-{}.xml",
        std::process::id(),
        chrono_free_stamp()
    ));
    let mut command = Command::new("powercfg");
    command.args(["/batteryreport", "/xml", "/output"]);
    command.arg(&path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let ran = command.output();
    let Ok(_) = ran else {
        return (Health::default(), Availability::Unavailable);
    };
    let xml = match std::fs::read_to_string(&path) {
        Ok(v) => {
            let _ = std::fs::remove_file(&path);
            v
        }
        Err(_) => return (Health::default(), Availability::Unavailable),
    };

    let design = attr_mwh(&xml, "designCapacity");
    let full = attr_mwh(&xml, "fullChargeCapacity");
    let cycles = attr_u32(&xml, "cycleCount");
    let availability = if design.is_some() || full.is_some() {
        Availability::Ok
    } else {
        Availability::Unavailable
    };
    (
        Health {
            design_capacity_wh: design,
            full_charge_capacity_wh: full,
            cycle_count: cycles,
        },
        availability,
    )
}

fn attr_mwh(haystack: &str, name: &str) -> Option<f64> {
    attr_u64(haystack, name).map(|v| v as f64 / 1000.0)
}

fn attr_u32(haystack: &str, name: &str) -> Option<u32> {
    attr_u64(haystack, name).map(|v| v as u32)
}

fn attr_u64(haystack: &str, name: &str) -> Option<u64> {
    let at = haystack.find(&format!("{name}=\""))? + name.len() + 2;
    let rest = &haystack[at..];
    let end = rest.find('"')?;
    rest[..end].parse().ok()
}

fn chrono_free_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u128 + d.as_secs() as u128 * 1000)
        .unwrap_or(0)
}
