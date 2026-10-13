//! Battery: live state from GetSystemPowerStatus, the charge rate from WinRT's aggregate
//! report, and health figures from the report `powercfg` can already generate.

use crate::models::{Availability, CapacityPoint};
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

/// Watts going in (charging) or out (on battery), from the same aggregate report Settings uses.
/// WinRT answers this without opening the battery device; the calling thread must have COM
/// initialised, which the sampler thread does when it starts.
pub fn power_watts() -> Option<f64> {
    let report = windows::Devices::Power::Battery::AggregateBattery().ok()?.GetReport().ok()?;
    let milliwatts = report.ChargeRateInMilliwatts().ok()?.Value().ok()?;
    (milliwatts != 0).then(|| (milliwatts as f64).abs() / 1000.0)
}

#[derive(Clone, Default)]
pub struct Health {
    pub design_capacity_wh: Option<f64>,
    pub full_charge_capacity_wh: Option<f64>,
    pub cycle_count: Option<u32>,
    pub capacity_history: Vec<CapacityPoint>,
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
    let health = parse_report(&xml);
    let availability = if health.design_capacity_wh.is_some() || health.full_charge_capacity_wh.is_some() {
        Availability::Ok
    } else {
        Availability::Unavailable
    };
    (health, availability)
}

/// Reads the report Windows writes. Its names are PascalCase and come in two shapes: each
/// battery's figures are child elements (`<DesignCapacity>57000</DesignCapacity>`), each week of
/// history is a row of attributes (`DesignCapacity="57000"`). The parser this replaces looked for
/// `designCapacity="` and matched neither, so no laptop ever showed its health.
///
/// Capacities are milliwatt-hours. A machine with several batteries is reported as their sum,
/// which is what "how long will this run" depends on.
pub fn parse_report(xml: &str) -> Health {
    let mut design = 0u64;
    let mut full = 0u64;
    let mut cycles: Option<u32> = None;
    let batteries = between(xml, "<Batteries>", "</Batteries>").unwrap_or("");
    for block in batteries.split("<Battery>").skip(1) {
        let block = block.split("</Battery>").next().unwrap_or("");
        design += element_u64(block, "DesignCapacity").unwrap_or(0);
        full += element_u64(block, "FullChargeCapacity").unwrap_or(0);
        // Many batteries do not count cycles and report 0; a dash says that more honestly than
        // a zero on a battery that is years old.
        if let Some(count) = element_u64(block, "CycleCount").filter(|c| *c > 0) {
            cycles = Some(cycles.unwrap_or(0).max(count as u32));
        }
    }

    let mut history = Vec::new();
    let rows = between(xml, "<History>", "</History>").unwrap_or("");
    for row in rows.split("<HistoryEntry").skip(1) {
        let row = row.split("/>").next().unwrap_or("");
        let design_mwh = attr_u64(row, "DesignCapacity").unwrap_or(0);
        let full_mwh = attr_u64(row, "FullChargeCapacity").unwrap_or(0);
        let Some(at_ms) = attr(row, "StartDate").and_then(parse_utc_ms) else { continue };
        if design_mwh == 0 || full_mwh == 0 {
            continue;
        }
        history.push(CapacityPoint {
            at_ms,
            design_wh: design_mwh as f64 / 1000.0,
            full_charge_wh: full_mwh as f64 / 1000.0,
            cycle_count: attr_u64(row, "CycleCount").filter(|c| *c > 0).map(|c| c as u32),
        });
    }

    Health {
        design_capacity_wh: (design > 0).then(|| design as f64 / 1000.0),
        full_charge_capacity_wh: (full > 0).then(|| full as f64 / 1000.0),
        cycle_count: cycles,
        capacity_history: history,
    }
}

fn between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)? + start;
    Some(&haystack[start..end])
}

fn element_u64(haystack: &str, name: &str) -> Option<u64> {
    between(haystack, &format!("<{name}>"), &format!("</{name}>"))?.trim().parse().ok()
}

/// An attribute's value. The name must start the attribute, so `StartDate` does not also match
/// the tail of `LocalStartDate`.
fn attr<'a>(haystack: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let mut from = 0;
    while let Some(found) = haystack[from..].find(&needle) {
        let at = from + found;
        let starts_attribute = haystack[..at].chars().next_back().is_none_or(char::is_whitespace);
        if starts_attribute {
            let start = at + needle.len();
            let end = haystack[start..].find('"')? + start;
            return Some(&haystack[start..end]);
        }
        from = at + needle.len();
    }
    None
}

fn attr_u64(haystack: &str, name: &str) -> Option<u64> {
    attr(haystack, name)?.trim().parse().ok()
}

/// `2026-08-13T16:57:00Z` as Unix milliseconds. Only this one shape, which is the shape the
/// report writes; anything else is skipped rather than guessed at.
fn parse_utc_ms(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (year, month, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(|p| p.split('.').next().unwrap_or("0").parse::<i64>());
    let (hour, minute) = (t.next()?.ok()?, t.next()?.ok()?);
    let second = t.next().unwrap_or(Ok(0)).ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok().map(|s| s * 1000)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn chrono_free_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u128 + d.as_secs() as u128 * 1000)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes `powercfg /batteryreport /xml` writes, cut down: element children per battery,
    /// attribute rows per week. Taken from a real report on this machine (a desktop, so its
    /// batteries block is empty and its history all zeros) with laptop figures filled in.
    const REPORT: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<BatteryReport xmlns="http://schemas.microsoft.com/battery/2012">
  <Batteries>
    <Battery>
      <Id>DELL 1VX1H</Id>
      <DesignCapacity>56000</DesignCapacity>
      <FullChargeCapacity>47600</FullChargeCapacity>
      <CycleCount>312</CycleCount>
    </Battery>
  </Batteries>
  <RuntimeEstimates>
    <DesignCapacity>
      <Capacity>56000</Capacity>
    </DesignCapacity>
  </RuntimeEstimates>
  <History>
    <HistoryEntry
      StartDate="2026-08-13T16:57:00Z"
      LocalStartDate="2026-08-14T00:57:00"
      DesignCapacity="56000"
      FullChargeCapacity="49000"
      CycleCount="300"
      />
    <HistoryEntry
      LocalStartDate="2026-08-21T00:57:00"
      StartDate="2026-08-20T16:57:00Z"
      DesignCapacity="0"
      FullChargeCapacity="0"
      CycleCount="0"
      />
    <HistoryEntry
      LocalStartDate="2026-08-28T00:57:00"
      StartDate="2026-08-27T16:57:00Z"
      DesignCapacity="56000"
      FullChargeCapacity="47600"
      CycleCount="0"
      />
  </History>
</BatteryReport>"#;

    #[test]
    fn a_laptop_report_gives_its_health_and_its_history() {
        let health = parse_report(REPORT);
        assert_eq!(health.design_capacity_wh, Some(56.0));
        assert_eq!(health.full_charge_capacity_wh, Some(47.6));
        assert_eq!(health.cycle_count, Some(312));
        assert_eq!(health.capacity_history.len(), 2, "the week with no reading is skipped");
        let first = &health.capacity_history[0];
        assert_eq!(first.at_ms, 1_786_640_220_000, "2026-08-13T16:57:00Z");
        assert_eq!((first.design_wh, first.full_charge_wh, first.cycle_count), (56.0, 49.0, Some(300)));
        let last = &health.capacity_history[1];
        assert_eq!(last.at_ms, 1_786_640_220_000 + 14 * 86_400_000, "UTC start, not the local one listed first");
        assert_eq!(last.cycle_count, None, "a zero count is no count");
    }

    #[test]
    fn a_desktop_report_has_no_health_at_all() {
        let start = REPORT.find("<Batteries>").unwrap();
        let end = REPORT.find("</Batteries>").unwrap();
        let desktop = format!("{}<Batteries>\n  {}", &REPORT[..start], &REPORT[end..]);
        let health = parse_report(&desktop);
        assert_eq!(health.design_capacity_wh, None, "the runtime estimate's DesignCapacity is not a battery");
        assert_eq!(health.cycle_count, None);
    }

    #[test]
    fn several_batteries_add_up() {
        let two = REPORT.replace(
            "</Batteries>",
            "<Battery><DesignCapacity>24000</DesignCapacity><FullChargeCapacity>20000</FullChargeCapacity><CycleCount>40</CycleCount></Battery></Batteries>",
        );
        let health = parse_report(&two);
        assert_eq!(health.design_capacity_wh, Some(80.0));
        assert_eq!(health.full_charge_capacity_wh, Some(67.6));
        assert_eq!(health.cycle_count, Some(312), "the more worn battery's count");
    }

    #[test]
    fn utc_timestamps_convert_and_odd_shapes_are_refused() {
        assert_eq!(parse_utc_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
        assert_eq!(parse_utc_ms("2026-08-14T00:57:00"), None, "local time has no zone to trust");
        assert_eq!(parse_utc_ms("2026-13-01T00:00:00Z"), None);
    }

    /// The real report, on whatever machine runs the tests: it must parse without a battery too.
    #[test]
    fn the_live_report_parses() {
        let (health, _) = read_health();
        if health.design_capacity_wh.is_none() {
            assert!(health.capacity_history.is_empty(), "no battery, no history worth drawing");
        }
    }
}
