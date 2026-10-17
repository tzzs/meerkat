//! Battery: live state from GetSystemPowerStatus, the charge rate from WinRT's aggregate
//! report, and health figures from the report `powercfg` can already generate.

use crate::models::{Availability, CapacityPoint};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub design_capacity_wh: Option<f64>,
    pub full_charge_capacity_wh: Option<f64>,
    pub cycle_count: Option<u32>,
    /// One point per week from Windows' own report, back to when it started keeping one: how
    /// much the battery holds when full, against what it held new.
    pub capacity_history: Vec<CapacityPoint>,
    /// Full charge over design capacity. Kept apart from the watt-hours because a battery that
    /// reports relative capacities still has an honest ratio but no watt-hours at all.
    pub health_percent: Option<f64>,
}

/// Watts going in (charging) or out (on battery), from the same aggregate report Settings uses.
/// WinRT answers this without opening the battery device; the calling thread must have COM
/// initialised, which the sampler thread does when it starts.
pub fn power_watts() -> Option<f64> {
    let report = windows::Devices::Power::Battery::AggregateBattery().ok()?.GetReport().ok()?;
    let milliwatts = report.ChargeRateInMilliwatts().ok()?.Value().ok()?;
    (milliwatts != 0).then(|| (milliwatts as f64).abs() / 1000.0)
}

/// The figures move over weeks, and every read runs `powercfg` for one to three seconds.
const HEALTH_EVERY: Duration = Duration::from_secs(30 * 60);
/// `powercfg` that has not written its report by then is not going to.
const POWERCFG_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Slot {
    latest: Health,
    in_flight: bool,
    finished_at: Option<Instant>,
}

/// Runs the battery report off the sampler thread. `powercfg` used to run inline on the first
/// tick and every 30 minutes, freezing every figure for the one to three seconds it takes; now
/// one worker thread at a time does it and the sampler only reads what the last one found.
#[derive(Clone, Default)]
pub struct HealthReader {
    slot: Arc<Mutex<Slot>>,
}

impl HealthReader {
    /// The last figures read, asking for a fresh read first when one is due. Never waits for
    /// `powercfg`: until the first read lands, every figure is None rather than a guess.
    pub fn poll(&self, has_battery: bool) -> Health {
        let mut slot = lock(&self.slot);
        if read_is_due(has_battery, slot.in_flight, slot.finished_at.map(|at| at.elapsed())) {
            slot.in_flight = true;
            let shared = Arc::clone(&self.slot);
            let spawned = std::thread::Builder::new()
                .name("meerkat-battery-report".into())
                .spawn(move || {
                    let (health, availability) = read_health();
                    let mut slot = lock(&shared);
                    // A report that could not be produced says nothing about the battery, so
                    // the last figures that were read stand until one can.
                    if availability != Availability::Unavailable {
                        slot.latest = health;
                    }
                    slot.in_flight = false;
                    slot.finished_at = Some(Instant::now());
                });
            if let Err(error) = spawned {
                crate::diag::warn(format!("battery report: no worker thread: {error}"));
                slot.in_flight = false;
                slot.finished_at = Some(Instant::now());
            }
        }
        slot.latest.clone()
    }
}

/// A plain flag and two timestamps; a worker that panicked leaves nothing half-written behind.
fn lock(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A desktop never runs `powercfg` at all, and at most one read is ever in flight.
fn read_is_due(has_battery: bool, in_flight: bool, since_last: Option<Duration>) -> bool {
    has_battery && !in_flight && since_last.map_or(true, |elapsed| elapsed >= HEALTH_EVERY)
}

/// `powercfg /batteryreport /xml` is the only user-mode window Windows gives us on
/// design capacity, full charge capacity and cycle count. Blocking: one to three seconds,
/// so it belongs on a worker thread ([`HealthReader`]).
///
/// `Unavailable` means no report could be produced at all; `Partial` that the report was
/// read but holds nothing usable (a desktop, or a battery that reports no capacities).
pub fn read_health() -> (Health, Availability) {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "meerkat-battery-{}-{}.xml",
        std::process::id(),
        chrono_free_stamp()
    ));
    let mut command = Command::new("powercfg");
    command.args(["/batteryreport", "/xml", "/output"]);
    command.arg(&path);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let finished = match command.spawn() {
        Ok(child) => wait_or_kill(child, POWERCFG_TIMEOUT),
        Err(_) => false,
    };
    let xml = if finished {
        std::fs::read_to_string(&path).ok()
    } else {
        None
    };
    // Also after a kill, which can leave a half-written report behind.
    let _ = std::fs::remove_file(&path);
    let Some(xml) = xml else {
        return (Health::default(), Availability::Unavailable);
    };

    let health = parse_report(&xml);
    let availability = if health.design_capacity_wh.is_some()
        || health.full_charge_capacity_wh.is_some()
        || health.health_percent.is_some()
        || health.cycle_count.is_some()
    {
        Availability::Ok
    } else {
        Availability::Partial
    };
    (health, availability)
}

/// Waits for `child`, killing it once `limit` has passed. True when it exited on its own.
fn wait_or_kill(mut child: Child, limit: Duration) -> bool {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// report parsing
// ---------------------------------------------------------------------------

/// What one `<Battery>` element of the report says, in the report's own units.
#[derive(Debug, Default)]
struct BatteryEntry {
    design: Option<u64>,
    full: Option<u64>,
    cycles: Option<u64>,
    /// `<RelativeCapacity>1</RelativeCapacity>`: the capacities are relative units, not mWh.
    relative: bool,
}

/// Reads the health figures out of a `powercfg /batteryreport /xml` report.
///
/// The report (namespace `http://schemas.microsoft.com/battery/2012`) carries everything as
/// PascalCase elements — `<Battery><DesignCapacity>50000</DesignCapacity>…` in mWh — and the
/// old reader looked for camelCase attributes that never appear, so no machine ever showed a
/// health figure. Only `<Battery>` blocks are read: `<RuntimeEstimates>` repeats the very same
/// tag names (`<DesignCapacity><Capacity>0</Capacity>…`) for a desktop, and `<History>` entries
/// carry `DesignCapacity="0"` as attributes. Several batteries add up their capacities and
/// report the highest cycle count; a figure any battery leaves absent or at zero is None, never
/// a number.
fn parse_report(xml: &str) -> Health {
    let entries: Vec<BatteryEntry> = element_bodies(xml, "Battery")
        .into_iter()
        .map(|block| BatteryEntry {
            design: child_number(block, "DesignCapacity"),
            full: child_number(block, "FullChargeCapacity"),
            cycles: child_number(block, "CycleCount"),
            relative: child_number(block, "RelativeCapacity") == Some(1),
        })
        .collect();
    if entries.is_empty() {
        return Health::default();
    }

    let sum = |pick: fn(&BatteryEntry) -> Option<u64>| -> Option<u64> {
        entries
            .iter()
            .map(|entry| pick(entry).filter(|value| *value > 0))
            .sum::<Option<u64>>()
    };
    let design = sum(|entry| entry.design);
    let full = sum(|entry| entry.full);
    let cycles = entries
        .iter()
        .map(|entry| entry.cycles.filter(|count| *count > 0))
        .collect::<Option<Vec<u64>>>()
        .and_then(|counts| counts.into_iter().max())
        .map(|count| count.min(u32::MAX as u64) as u32);

    let all_relative = entries.iter().all(|entry| entry.relative);
    let any_relative = entries.iter().any(|entry| entry.relative);
    // A ratio is only a ratio when both sides are counted in the same unit.
    let health_percent = match (design, full) {
        (Some(design), Some(full)) if all_relative || !any_relative => {
            Some(full as f64 / design as f64 * 100.0)
        }
        _ => None,
    };
    let watt_hours = |milliwatt_hours: Option<u64>| {
        milliwatt_hours
            .filter(|_| !any_relative)
            .map(|value| value as f64 / 1000.0)
    };
    Health {
        design_capacity_wh: watt_hours(design),
        full_charge_capacity_wh: watt_hours(full),
        cycle_count: cycles,
        health_percent,
        capacity_history: parse_capacity_history(xml),
    }
}

/// `<History>` rows carry their figures as attributes (`DesignCapacity="57000"`), one row per
/// week, which is the only place a battery's decline over time is written down. Rows with no
/// reading (a week the machine was off) are skipped rather than plotted as zero.
fn parse_capacity_history(xml: &str) -> Vec<CapacityPoint> {
    let mut out = Vec::new();
    // HistoryEntry elements are self-closing, so their figures sit in the tag's attributes
    // rather than in a body.
    let mut at = 0usize;
    while let Some(offset) = xml[at..].find("<HistoryEntry") {
        let start = at + offset;
        let Some(end) = xml[start..].find('>') else { break };
        let tag = &xml[start..start + end];
        at = start + end + 1;
        let Some(design_mwh) = attr_number(tag, "DesignCapacity") else { continue };
        let Some(full_mwh) = attr_number(tag, "FullChargeCapacity") else { continue };
        let Some(stamp) = attr_text(tag, "StartDate") else { continue };
        let Some(at_ms) = parse_report_time(&stamp) else { continue };
        if design_mwh == 0 || full_mwh == 0 {
            continue;
        }
        out.push(CapacityPoint {
            at_ms,
            design_wh: design_mwh as f64 / 1000.0,
            full_charge_wh: full_mwh as f64 / 1000.0,
            cycle_count: attr_number(tag, "CycleCount").filter(|count| *count > 0).map(|count| count.min(u32::MAX as u64) as u32),
        });
    }
    out.sort_by_key(|point| point.at_ms);
    out
}

/// One attribute's text: `Name="value"` or `Name='value'`.
fn attr_text(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!("{name}="))? + name.len() + 1;
    let quote = tag[start..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &tag[start + 1..];
    let end = body.find(quote)?;
    Some(body[..end].to_string())
}

fn attr_number(tag: &str, name: &str) -> Option<u64> {
    attr_text(tag, name)?.trim().parse().ok()
}

/// `2012-01-01T00:00:00Z`, the report's own stamp, to Unix milliseconds.
fn parse_report_time(stamp: &str) -> Option<u64> {
    let bytes = stamp.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let number = |from: usize, to: usize| stamp.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since the epoch, by the civil-from-days algorithm, so no date library is needed.
    let (y, m) = if month <= 2 { (year - 1, month + 12) } else { (year, month) };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(seconds).ok().map(|s| s * 1000)
}

/// The text of the first `local` element directly or indirectly inside `block`, as a number.
fn child_number(block: &str, local: &str) -> Option<u64> {
    element_bodies(block, local)
        .first()
        .and_then(|body| body.trim().parse().ok())
}

/// One markup tag: its local name (any `prefix:` stripped) and where it sits in the text.
struct Tag<'a> {
    local: &'a str,
    closing: bool,
    self_closing: bool,
    start: usize,
    end: usize,
}

/// Every element tag in `xml`, in order. Comments, CDATA, declarations and processing
/// instructions are stepped over, and a `>` inside a quoted attribute value does not end a tag.
/// Tolerant rather than validating: the report only has to yield a handful of numbers.
fn tags(xml: &str) -> Vec<Tag<'_>> {
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(offset) = xml[at..].find('<') {
        let start = at + offset;
        let rest = &xml[start..];
        let skip_to = |terminator: &str| {
            rest.find(terminator)
                .map_or(xml.len(), |end| start + end + terminator.len())
        };
        if rest.starts_with("<!--") {
            at = skip_to("-->");
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            at = skip_to("]]>");
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            at = skip_to(">");
            continue;
        }
        let mut quote: Option<u8> = None;
        let mut end = None;
        for (index, byte) in bytes.iter().enumerate().skip(start + 1) {
            match quote {
                Some(open) if *byte == open => quote = None,
                Some(_) => {}
                None if *byte == b'"' || *byte == b'\'' => quote = Some(*byte),
                None if *byte == b'>' => {
                    end = Some(index);
                    break;
                }
                None => {}
            }
        }
        let Some(end) = end else { break };
        let inner = &xml[start + 1..end];
        let closing = inner.starts_with('/');
        let self_closing = !closing && inner.ends_with('/');
        let name = inner
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        out.push(Tag {
            local,
            closing,
            self_closing,
            start,
            end: end + 1,
        });
        at = end + 1;
    }
    out
}

/// The contents of every outermost `local` element, matched case-insensitively and with or
/// without a namespace prefix. `<Batteries>` is not `<Battery>`: names are compared whole.
fn element_bodies<'a>(xml: &'a str, local: &str) -> Vec<&'a str> {
    let mut bodies = Vec::new();
    let mut depth = 0usize;
    let mut body_start = 0usize;
    for tag in tags(xml) {
        if !tag.local.eq_ignore_ascii_case(local) {
            continue;
        }
        if tag.closing {
            if depth > 0 {
                depth -= 1;
                if depth == 0 {
                    bodies.push(&xml[body_start..tag.start]);
                }
            }
        } else if tag.self_closing {
            if depth == 0 {
                bodies.push("");
            }
        } else {
            if depth == 0 {
                body_start = tag.end;
            }
            depth += 1;
        }
    }
    bodies
}

fn chrono_free_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() as u128 + d.as_secs() as u128 * 1000)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// tests (report parsing only — no powercfg, so they run anywhere)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The framing of a real report from a desktop (TZZS-HOMEPC): no `<Battery>` at all, but
    /// the same tag names under `<RuntimeEstimates>`, and zeroes as attributes in `<History>`.
    const DESKTOP: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<?xml-stylesheet type='text/xsl' href='C:\battery-stylesheet.xsl'?>
<BatteryReport xmlns="http://schemas.microsoft.com/battery/2012">
  <ReportInformation>
    <ReportVersion>1.1</ReportVersion>
  </ReportInformation>
  <SystemInformation>
    <PlatformRole>Desktop</PlatformRole>
  </SystemInformation>
  <Batteries>
  </Batteries>
  <RuntimeEstimates>
    <DesignCapacity>
      <Capacity>0</Capacity>
      <ActiveRuntime>PT0S</ActiveRuntime>
    </DesignCapacity>
    <FullChargeCapacity>
      <Capacity>0</Capacity>
    </FullChargeCapacity>
  </RuntimeEstimates>
  <History>
    <HistoryEntry
      StartDate="2026-08-11T19:04:00Z"
      DesignCapacity="0"
      FullChargeCapacity="0"
      CycleCount="0"
      />
  </History>
</BatteryReport>"#;

    fn laptop(batteries: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<BatteryReport xmlns="http://schemas.microsoft.com/battery/2012">
  <ReportInformation><ReportVersion>1.1</ReportVersion></ReportInformation>
  <Batteries>{batteries}</Batteries>
  <RuntimeEstimates>
    <DesignCapacity><Capacity>99999</Capacity></DesignCapacity>
    <FullChargeCapacity><Capacity>88888</Capacity></FullChargeCapacity>
  </RuntimeEstimates>
  <History>
    <HistoryEntry DesignCapacity="77777" FullChargeCapacity="66666" CycleCount="999" />
  </History>
</BatteryReport>"#
        )
    }

    const ONE_BATTERY: &str = r#"
    <Battery>
      <Id>DELL 1VX1H1</Id>
      <Manufacturer>SMP</Manufacturer>
      <Chemistry>LiP</Chemistry>
      <LongTerm>1</LongTerm>
      <RelativeCapacity>0</RelativeCapacity>
      <DesignCapacity>50000</DesignCapacity>
      <FullChargeCapacity>45000</FullChargeCapacity>
      <CycleCount>120</CycleCount>
    </Battery>"#;

    #[test]
    fn a_laptop_report_yields_capacities_cycles_and_health() {
        let health = parse_report(&laptop(ONE_BATTERY));
        assert_eq!(health.design_capacity_wh, Some(50.0), "mWh read as Wh");
        assert_eq!(health.full_charge_capacity_wh, Some(45.0));
        assert_eq!(health.cycle_count, Some(120));
        assert_eq!(health.health_percent, Some(90.0));
    }

    #[test]
    fn a_desktop_report_claims_nothing() {
        // Neither `<RuntimeEstimates>` nor the `<History>` attributes are a battery.
        assert_eq!(parse_report(DESKTOP), Health::default());
        assert_eq!(parse_report(""), Health::default());
        assert_eq!(parse_report("not xml at all <<<"), Health::default());
    }

    #[test]
    fn the_runtime_estimates_block_is_never_read_as_a_battery() {
        // A laptop whose battery has no capacities: the 99999/88888 next door must not leak in.
        let health = parse_report(&laptop("<Battery><Id>X</Id></Battery>"));
        assert_eq!(health, Health::default());
    }

    #[test]
    fn several_batteries_add_up_and_keep_the_highest_cycle_count() {
        let second = r#"<Battery><DesignCapacity>30000</DesignCapacity>
            <FullChargeCapacity>24000</FullChargeCapacity><CycleCount>300</CycleCount></Battery>"#;
        let health = parse_report(&laptop(&format!("{ONE_BATTERY}{second}")));
        assert_eq!(health.design_capacity_wh, Some(80.0));
        assert_eq!(health.full_charge_capacity_wh, Some(69.0));
        assert_eq!(health.cycle_count, Some(300));
        assert_eq!(health.health_percent, Some(69.0 / 80.0 * 100.0));
    }

    #[test]
    fn a_figure_one_battery_leaves_out_is_no_total_at_all() {
        // Summing only the battery that answered would claim a pack half its real size.
        let silent = "<Battery><DesignCapacity>30000</DesignCapacity><CycleCount>0</CycleCount></Battery>";
        let health = parse_report(&laptop(&format!("{ONE_BATTERY}{silent}")));
        assert_eq!(health.design_capacity_wh, Some(80.0));
        assert_eq!(health.full_charge_capacity_wh, None);
        assert_eq!(health.cycle_count, None, "a zero cycle count is no cycle count");
        assert_eq!(health.health_percent, None);
    }

    #[test]
    fn zero_capacities_are_absences() {
        let zero = "<Battery><DesignCapacity>0</DesignCapacity><FullChargeCapacity>0</FullChargeCapacity>\
                    <CycleCount>0</CycleCount></Battery>";
        assert_eq!(parse_report(&laptop(zero)), Health::default());
    }

    #[test]
    fn tags_are_matched_whatever_their_case_prefix_or_spacing() {
        let report = r#"<b:BatteryReport xmlns:b="http://schemas.microsoft.com/battery/2012">
          <b:batteries>
            <b:battery id="a>b">
              <!-- <DesignCapacity>1</DesignCapacity> -->
              <b:designcapacity>
                 50000
              </b:designcapacity>
              <B:FULLCHARGECAPACITY>40000</B:FULLCHARGECAPACITY>
              <b:CycleCount >7</b:CycleCount >
            </b:battery>
          </b:batteries>
        </b:BatteryReport>"#;
        let health = parse_report(report);
        assert_eq!(health.design_capacity_wh, Some(50.0));
        assert_eq!(health.full_charge_capacity_wh, Some(40.0));
        assert_eq!(health.cycle_count, Some(7));
        assert_eq!(health.health_percent, Some(80.0));
    }

    #[test]
    fn relative_capacities_give_a_ratio_but_no_watt_hours() {
        let relative = "<Battery><RelativeCapacity>1</RelativeCapacity><DesignCapacity>100</DesignCapacity>\
                        <FullChargeCapacity>87</FullChargeCapacity></Battery>";
        let health = parse_report(&laptop(relative));
        assert_eq!(health.design_capacity_wh, None);
        assert_eq!(health.full_charge_capacity_wh, None);
        assert_eq!(health.health_percent, Some(87.0));
    }

    #[test]
    fn batteries_is_not_battery() {
        assert_eq!(element_bodies("<Batteries><Battery>x</Battery></Batteries>", "Battery"), vec!["x"]);
        assert_eq!(element_bodies("<Batteries></Batteries>", "Battery"), Vec::<&str>::new());
        assert_eq!(element_bodies("<Battery/>", "battery"), vec![""]);
    }

    /// Runs the real `powercfg` (one to three seconds), so only on request:
    /// `cargo test live_report -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_report() {
        let started = Instant::now();
        let (health, availability) = read_health();
        println!("{health:?} {availability:?} in {:?}", started.elapsed());
        assert_ne!(availability, Availability::Unavailable, "powercfg produced a report");
    }

    #[test]
    fn powercfg_runs_only_for_a_battery_and_never_twice_at_once() {
        assert!(read_is_due(true, false, None), "the first read is due at once");
        assert!(!read_is_due(false, false, None), "a desktop never runs powercfg");
        assert!(!read_is_due(true, true, None), "one read in flight at a time");
        assert!(!read_is_due(true, false, Some(Duration::from_secs(60))));
        assert!(read_is_due(true, false, Some(HEALTH_EVERY)));
        // No battery: polling returns nothing and starts nothing.
        let reader = HealthReader::default();
        assert_eq!(reader.poll(false), Health::default());
        assert!(!lock(&reader.slot).in_flight);
    }
}
