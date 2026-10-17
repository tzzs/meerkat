//! Worth a look: the rules that turn raw figures into something worth telling the user
//! about, with a repeat count instead of a pile of duplicates.
//!
//! Per-app network bytes are not available without an elevated ETW session, so a heavy
//! network insight names the app holding the most connections and says so in its detail.
//!
//! `Insight.value` is in the unit of its rule: percent of the whole machine for HighCpu, bytes
//! of growth for GrowingMemory, bytes per second for HeavyDisk and HeavyNetwork, °C for Heat.
//! `sustained_seconds` is how long the condition held; for GrowingMemory it is the span the
//! growth was measured over.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;

use crate::history::now_seconds;
use crate::models::{AppInfo, Insight, InsightKind, Snapshot};
use crate::settings::Settings;

const KEEP: usize = 400;
const RE_NOTIFY_AFTER: u64 = 30 * 60;
const HEAT_CELSIUS: f64 = 85.0;
/// Notifications one evaluation may raise. The rest are recorded in the list just the same,
/// and count as notified so they do not all arrive on the next tick instead.
const ANNOUNCE_AT_MOST: usize = 3;
/// Everything starts at sign-in at once; memory is not judged until that has settled.
const SETTLE_SECONDS: u64 = 180;

/// The growing-memory rule watches each app over this window, one sample every 15 s.
const MEMORY_WINDOW_SECONDS: u64 = 30 * 60;
const MEMORY_SAMPLE_EVERY: u64 = 15;
/// An app this young is still starting up: loading is not leaking.
const MEMORY_MIN_AGE_MS: u64 = 10 * 60 * 1000;
/// Growth has to have been going on for this long. A game loading a level, a compile or an
/// editor indexing a project climbs for a minute or three and then sits on a plateau.
const MEMORY_MIN_SPAN_SECONDS: u64 = 10 * 60;
/// The growth has to still be going on: the last five minutes must add at least 3% of it.
const MEMORY_TREND_SECONDS: u64 = 5 * 60;
const MEMORY_TREND_SHARE: f64 = 0.03;
/// And memory has to be at (about) its high, not on the way back down.
const MEMORY_NEAR_PEAK: f64 = 0.97;
/// Once announced, an app stays quiet this long unless its growth has doubled since.
const MEMORY_COOLDOWN_SECONDS: u64 = 6 * 3600;

#[derive(Default)]
struct Watch {
    cpu_since: Option<u64>,
    disk_since: Option<u64>,
    /// (Unix seconds, resident bytes), oldest first, at most 30 minutes of it.
    memory: VecDeque<(u64, u64)>,
}

struct Hit {
    kind: InsightKind,
    key: String,
    name: String,
    value: f64,
    sustained: u64,
}

/// What the growing-memory rule measured for one app.
#[derive(Debug, PartialEq)]
struct Growth {
    bytes: u64,
    /// From the last sample still at the low to now.
    over_seconds: u64,
}

pub struct Watcher {
    path: PathBuf,
    states: HashMap<String, Watch>,
    notified_at: HashMap<String, u64>,
    insights: Vec<Insight>,
    heat_since: Option<u64>,
    network_since: Option<u64>,
    opened_at: u64,
    /// App key → (when, growth in bytes) of its last growing-memory announcement.
    memory_announced: HashMap<String, (u64, u64)>,
    /// A row changed since the file was last written. A dismissed row whose condition still
    /// holds changes nothing, and must not rewrite the file on every tick.
    unsaved: bool,
}

impl Watcher {
    pub fn open(dir: &PathBuf) -> Self {
        let path = dir.join("insights.json");
        let insights = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Insight>>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            states: HashMap::new(),
            notified_at: HashMap::new(),
            insights,
            heat_since: None,
            network_since: None,
            opened_at: now_seconds(),
            memory_announced: HashMap::new(),
            unsaved: false,
        }
    }

    /// Insights worth a notification since the last tick — at most three. Repeats update the
    /// stored row and stay quiet until RE_NOTIFY_AFTER has passed.
    pub fn evaluate(&mut self, snapshot: &Snapshot, settings: &Settings) -> Vec<Insight> {
        self.evaluate_at(snapshot, settings, &Default::default(), now_seconds())
    }

    /// `muted` holds the apps whose rule says not to watch them: OBS and a compiler are meant to
    /// keep the CPU busy, and the only answer to that used to be dismissing the row every day.
    pub fn evaluate_muted(
        &mut self,
        snapshot: &Snapshot,
        settings: &Settings,
        muted: &std::collections::HashSet<String>,
    ) -> Vec<Insight> {
        self.evaluate_at(snapshot, settings, muted, now_seconds())
    }

    fn evaluate_at(
        &mut self,
        snapshot: &Snapshot,
        settings: &Settings,
        muted: &std::collections::HashSet<String>,
        now: u64,
    ) -> Vec<Insight> {
        if !settings.alerts.enabled {
            self.states.clear();
            return Vec::new();
        }
        let limits = settings.alerts.sanitized();
        // sysinfo reports a process's CPU per core, summed, so one busy core of twenty reads
        // 100. The threshold and the insight are both a share of the whole machine.
        let cores = snapshot.machine.logical_cores.max(1) as f64;
        let settled = now >= self.opened_at.saturating_add(SETTLE_SECONDS);

        let mut hits: Vec<Hit> = Vec::new();
        for app in snapshot.apps.iter().filter(|app| !muted.contains(&app.key)) {
            if !self.states.contains_key(&app.key) {
                self.states.insert(app.key.clone(), Watch::default());
            }
            let Some(state) = self.states.get_mut(&app.key) else {
                continue;
            };

            let cpu = app.cpu_percent / cores;
            if cpu >= limits.cpu_percent {
                let since = *state.cpu_since.get_or_insert(now);
                if now.saturating_sub(since) >= limits.sustained_seconds {
                    hits.push(Hit {
                        kind: InsightKind::HighCpu,
                        key: app.key.clone(),
                        name: app.name.clone(),
                        value: cpu,
                        sustained: now.saturating_sub(since),
                    });
                    state.cpu_since = None;
                }
            } else {
                state.cpu_since = None;
            }

            let rate = app.read_bytes_per_second + app.write_bytes_per_second;
            if rate >= limits.disk_bytes_per_second {
                let since = *state.disk_since.get_or_insert(now);
                if now.saturating_sub(since) >= limits.sustained_seconds {
                    hits.push(Hit {
                        kind: InsightKind::HeavyDisk,
                        key: app.key.clone(),
                        name: app.name.clone(),
                        value: rate,
                        sustained: now.saturating_sub(since),
                    });
                    state.disk_since = None;
                }
            } else {
                state.disk_since = None;
            }

            if !settled || !old_enough(app, now) {
                state.memory.clear();
            } else if sample_memory(&mut state.memory, now, app.memory_bytes) {
                if let Some(growth) = memory_growth(&state.memory, limits.memory_grow_bytes) {
                    let quiet = self.memory_announced.get(&app.key).is_some_and(|(at, last)| {
                        now < at.saturating_add(MEMORY_COOLDOWN_SECONDS)
                            && growth.bytes < last.saturating_mul(2)
                    });
                    if !quiet {
                        hits.push(Hit {
                            kind: InsightKind::GrowingMemory,
                            key: app.key.clone(),
                            name: app.name.clone(),
                            value: growth.bytes as f64,
                            sustained: growth.over_seconds,
                        });
                    }
                }
            }
        }

        if let Some(hottest) = snapshot.temperature.hottest {
            if hottest >= HEAT_CELSIUS {
                let since = *self.heat_since.get_or_insert(now);
                if now.saturating_sub(since) >= 60 {
                    if let Some(busiest) = snapshot
                        .apps
                        .iter()
                        .filter(|a| a.cpu_percent > 1.0)
                        .max_by(|a, b| a.cpu_percent.total_cmp(&b.cpu_percent))
                    {
                        hits.push(Hit {
                            kind: InsightKind::Heat,
                            key: busiest.key.clone(),
                            name: busiest.name.clone(),
                            value: hottest,
                            sustained: now.saturating_sub(since),
                        });
                    }
                    self.heat_since = None;
                }
            } else {
                self.heat_since = None;
            }
        }

        let traffic =
            snapshot.network.download_bytes_per_second + snapshot.network.upload_bytes_per_second;
        if traffic >= limits.network_bytes_per_second {
            let since = *self.network_since.get_or_insert(now);
            if now.saturating_sub(since) >= limits.sustained_seconds {
                if let Some(top) = snapshot
                    .apps
                    .iter()
                    .filter(|a| a.connections > 0)
                    .max_by_key(|a| a.connections)
                {
                    hits.push(Hit {
                        kind: InsightKind::HeavyNetwork,
                        key: top.key.clone(),
                        name: top.name.clone(),
                        value: traffic,
                        sustained: now.saturating_sub(since),
                    });
                }
                self.network_since = None;
            }
        } else {
            self.network_since = None;
        }

        let present: HashSet<&str> = snapshot.apps.iter().map(|a| a.key.as_str()).collect();
        self.states.retain(|key, _| present.contains(key.as_str()));
        self.memory_announced
            .retain(|_, (at, _)| now < at.saturating_add(MEMORY_COOLDOWN_SECONDS));

        let mut announced = Vec::new();
        for hit in hits {
            // Every hit is recorded, and `record` marks each announced one as notified now —
            // so the ones over the cap stay quiet for the cooldown instead of piling up.
            if let Some(insight) = self.record(hit, now) {
                if insight.kind == InsightKind::GrowingMemory {
                    // The six-hour cooldown runs from what was announced, not from every
                    // hit: a doubling the 30-minute re-notify rule held back has to stay
                    // able to announce once that rule lets it.
                    self.memory_announced
                        .insert(insight.app_key.clone(), (now, insight.value as u64));
                }
                announced.push(insight);
            }
        }
        announced.truncate(ANNOUNCE_AT_MOST);
        if std::mem::take(&mut self.unsaved) {
            self.save();
        }
        announced
    }

    /// Fold a repeat into the existing row. Returns the insight only when it deserves a
    /// fresh notification.
    ///
    /// A row the user dismissed stays dismissed for the rest of the day: the id carries the day
    /// key, so the same rule/app cannot reappear until the next calendar day. The row is left
    /// untouched (no re-resolving, no repeat bump, no notification) for exactly that reason —
    /// otherwise a still-over-threshold app would resurrect it one second later.
    fn record(&mut self, hit: Hit, now: u64) -> Option<Insight> {
        let id = format!("{}:{}:{}", kind_key(&hit.kind), hit.key, day_key(now));
        if let Some(existing) = self.insights.iter_mut().find(|i| i.id == id) {
            if existing.resolved {
                return None;
            }
            self.unsaved = true;
            existing.repeat_count += 1;
            existing.at_ms = now * 1000;
            existing.value = hit.value;
            existing.sustained_seconds = hit.sustained;
            let last = self.notified_at.get(&id).copied().unwrap_or(0);
            if now.saturating_sub(last) >= RE_NOTIFY_AFTER {
                self.notified_at.insert(id.clone(), now);
                return Some(existing.clone());
            }
            return None;
        }
        let insight = Insight {
            id: id.clone(),
            at_ms: now * 1000,
            kind: hit.kind,
            app_key: hit.key,
            app_name: hit.name,
            value: hit.value,
            sustained_seconds: hit.sustained,
            repeat_count: 1,
            day_key: day_key(now),
            resolved: false,
        };
        self.notified_at.insert(id, now);
        self.unsaved = true;
        self.insights.push(insight.clone());
        if self.insights.len() > KEEP {
            let drop = self.insights.len() - KEEP;
            self.insights.drain(0..drop);
        }
        Some(insight)
    }

    /// Marks a row as handled by the user and persists it. The dismissal stands for the rest of
    /// the day: a still-firing condition refreshes neither the row nor the notification (see
    /// [`Watcher::record`]); only the next calendar day's id can bring the rule back.
    pub fn dismiss(&mut self, id: &str) {
        if let Some(insight) = self.insights.iter_mut().find(|i| i.id == id) {
            insight.resolved = true;
            self.save();
        }
    }

    pub fn list(&self) -> Vec<Insight> {
        let mut sorted = self.insights.clone();
        sorted.sort_by_key(|i| std::cmp::Reverse(i.at_ms));
        sorted
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string(&self.insights) {
            let _ = fs::write(&self.path, raw);
        }
    }
}

/// An app whose oldest process started under ten minutes ago is still loading. An unknown
/// start time cannot prove that, so such an app is judged like any other.
fn old_enough(app: &AppInfo, now: u64) -> bool {
    app.oldest_started_at_ms
        .map_or(true, |started| now.saturating_mul(1000).saturating_sub(started) >= MEMORY_MIN_AGE_MS)
}

/// Adds a sample when 15 s have passed since the last one and drops what left the window.
/// True when a sample was added, which is when the rule is worth evaluating again.
fn sample_memory(window: &mut VecDeque<(u64, u64)>, now: u64, bytes: u64) -> bool {
    if window.back().is_some_and(|(at, _)| *at > now) {
        // The clock went back: these samples now lie in the future and prove nothing.
        window.clear();
    }
    if window
        .back()
        .is_some_and(|(at, _)| now < at.saturating_add(MEMORY_SAMPLE_EVERY))
    {
        return false;
    }
    window.push_back((now, bytes));
    while window
        .front()
        .is_some_and(|(at, _)| at.saturating_add(MEMORY_WINDOW_SECONDS) < now)
    {
        window.pop_front();
    }
    true
}

/// The growing-memory rule over one app's window (oldest first, the newest sample being
/// "now"). It fires only when every one of these holds:
/// - growth, the newest sample minus the window's minimum, is at least `threshold`;
/// - the growth has been going on for ten minutes or more: the last sample still within 5%
///   of the growth above the minimum is at least ten minutes old (so the window itself spans
///   ten minutes too). A load — a level, a compile, an indexer — climbs in a minute or three;
/// - memory is still at 97% or more of the window's maximum, not on its way back down;
/// - the last five minutes added at least 3% of the growth: a trend, not a plateau.
fn memory_growth(window: &VecDeque<(u64, u64)>, threshold: u64) -> Option<Growth> {
    let &(now, current) = window.back()?;
    let low = window.iter().map(|(_, bytes)| *bytes).min()?;
    let high = window.iter().map(|(_, bytes)| *bytes).max()?;
    let growth = current.saturating_sub(low);
    if growth < threshold.max(1) {
        return None;
    }
    let floor = low.saturating_add(growth / 20);
    let started = window.iter().rev().find(|(_, bytes)| *bytes <= floor)?.0;
    let over_seconds = now.saturating_sub(started);
    if over_seconds < MEMORY_MIN_SPAN_SECONDS {
        return None;
    }
    if (current as f64) < high as f64 * MEMORY_NEAR_PEAK {
        return None;
    }
    let earlier = window
        .iter()
        .rev()
        .find(|(at, _)| *at <= now.saturating_sub(MEMORY_TREND_SECONDS))?
        .1;
    if (current.saturating_sub(earlier) as f64) < growth as f64 * MEMORY_TREND_SHARE {
        return None;
    }
    Some(Growth { bytes: growth, over_seconds })
}

fn kind_key(kind: &InsightKind) -> &'static str {
    match kind {
        InsightKind::HighCpu => "cpu",
        InsightKind::GrowingMemory => "memory",
        InsightKind::HeavyDisk => "disk",
        InsightKind::HeavyNetwork => "network",
        InsightKind::Heat => "heat",
    }
}

/// The local calendar day, the same one the sampler's daily totals roll over on. A UTC day made
/// "dismissed for the rest of the day" end at 08:00 in UTC+8.
fn day_key(now_seconds: u64) -> String {
    crate::metrics::day_key(now_seconds.saturating_mul(1000))
}

// ---------------------------------------------------------------------------
// tests (rule folding only — no samplers, so they run anywhere)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppInfo, AppKind, MachineInfo};
    use crate::settings::AlertThresholds;

    /// Scratch config dir per test: the process id plus the test name keeps parallel tests from
    /// colliding, and nothing here ever touches the real profile directory.
    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("meerkat-alerts-{name}-{}", std::process::id()))
    }

    /// A single-app snapshot; every figure but CPU sits well below any threshold.
    fn app(key: &str, cpu_percent: f64) -> AppInfo {
        AppInfo {
            key: key.into(),
            name: key.into(),
            exe_path: format!("C:\\test\\{key}.exe"),
            kind: AppKind::App,
            main_pid: 4242,
            process_count: 1,
            cpu_percent,
            memory_bytes: 100_000_000,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
            read_bytes_total: 0,
            write_bytes_total: 0,
            connections: 0,
            listening_ports: Vec::new(),
            oldest_started_at_ms: None,
            windows: Vec::new(),
            pids: vec![4242],
            unmeasured_processes: 0,
            services: Vec::new(),
            can_quit: true,
            control: Default::default(),
            project: None,
        }
    }

    fn snapshot_of(apps: Vec<AppInfo>, logical_cores: usize) -> Snapshot {
        Snapshot {
            taken_at_ms: 0,
            machine: MachineInfo {
                hostname: "test-host".into(),
                os_version: "test".into(),
                chip_name: "test".into(),
                physical_cores: Some(logical_cores),
                logical_cores,
                total_memory_bytes: 8 << 30,
                uptime_seconds: 0,
            },
            cpu: Default::default(),
            memory: Default::default(),
            disk: Default::default(),
            network: Default::default(),
            gpu: Default::default(),
            battery: Default::default(),
            temperature: Default::default(),
            apps,
            process_count: 1,
            sampler_cost_microseconds: 0,
        }
    }

    /// A two-core machine, so sysinfo's per-core 190 is 95% of it.
    fn snapshot(one: AppInfo) -> Snapshot {
        snapshot_of(vec![one], 2)
    }

    /// Alerts on with the shortest sustain the rules accept (5 s) and unreachable disk/memory/
    /// network thresholds, so the CPU rule is the only one that can fire.
    fn alert_settings() -> Settings {
        Settings {
            alerts: AlertThresholds {
                enabled: true,
                cpu_percent: 80.0,
                memory_grow_bytes: u64::MAX,
                disk_bytes_per_second: 1e15,
                network_bytes_per_second: 1e15,
                sustained_seconds: 5,
            },
            ..Settings::default()
        }
    }

    fn hit(kind: InsightKind, key: &str, value: f64) -> Hit {
        Hit {
            kind,
            key: key.into(),
            name: key.into(),
            value,
            sustained: 30,
        }
    }

    /// A fixed second inside some day, for tests that drive the rules directly so they never
    /// depend on the wall clock.
    const DAY_ONE: u64 = 1_700_000_000;
    const DAY_TWO: u64 = DAY_ONE + 86_400;

    /// Over the threshold at `now` and again at `now + 5`: the second evaluation is the one a
    /// five-second sustain lets through.
    fn sustained(watcher: &mut Watcher, shot: &Snapshot, settings: &Settings, now: u64) -> Vec<Insight> {
        assert!(watcher.evaluate_at(shot, settings, &Default::default(), now).is_empty(), "nothing before the sustain");
        watcher.evaluate_at(shot, settings, &Default::default(), now + 5)
    }

    #[test]
    fn crossing_the_threshold_records_one_insight() {
        let dir = temp_dir("records");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);

        let announced = sustained(&mut watcher, &snapshot(app("chrome", 190.0)), &alert_settings(), DAY_ONE);

        assert_eq!(announced.len(), 1, "one sustained app yields one row");
        let insight = &announced[0];
        assert_eq!(insight.kind, InsightKind::HighCpu);
        assert_eq!(insight.app_key, "chrome");
        assert_eq!(insight.value, 95.0, "a share of the whole machine");
        assert_eq!(insight.repeat_count, 1);
        assert!(!insight.resolved);
        assert_eq!(insight.day_key, day_key(DAY_ONE));
        assert_eq!(insight.id, format!("cpu:chrome:{}", insight.day_key));
        assert_eq!(watcher.list().len(), 1);

        // Quiet thresholds yield nothing at all.
        assert!(
            watcher
                .evaluate_at(&snapshot(app("chrome", 10.0)), &alert_settings(), &Default::default(), DAY_ONE + 10)
                .is_empty()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cpu_is_judged_as_a_share_of_the_whole_machine() {
        let dir = temp_dir("cores");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        let settings = Settings {
            alerts: AlertThresholds { cpu_percent: 25.0, ..alert_settings().alerts },
            ..Settings::default()
        };
        // One core flat out on a twenty-core machine: sysinfo says 100, the machine is 5% busy.
        let one_core = snapshot_of(vec![app("cargo", 100.0)], 20);
        assert!(sustained(&mut watcher, &one_core, &settings, DAY_ONE).is_empty());
        // Six cores' worth is 30% of it.
        let six_cores = snapshot_of(vec![app("cargo", 600.0)], 20);
        let announced = sustained(&mut watcher, &six_cores, &settings, DAY_ONE + 100);
        assert_eq!(announced.len(), 1);
        assert_eq!(announced[0].value, 30.0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_threshold_cannot_make_every_app_hit() {
        let dir = temp_dir("zero");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        let settings = Settings {
            alerts: AlertThresholds { cpu_percent: 0.0, sustained_seconds: 0, ..alert_settings().alerts },
            ..Settings::default()
        };
        // 137 idle apps; a threshold of 0 used to announce every one of them at once.
        let idle: Vec<AppInfo> = (0..137).map(|i| app(&format!("app{i}"), 0.5)).collect();
        assert!(watcher.evaluate_at(&snapshot_of(idle.clone(), 2), &settings, &Default::default(), DAY_ONE).is_empty());
        assert!(watcher.evaluate_at(&snapshot_of(idle, 2), &settings, &Default::default(), DAY_ONE + 5).is_empty(), "clamped to 1%");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_evaluation_announces_at_most_three() {
        let dir = temp_dir("cap");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        let busy: Vec<AppInfo> = (0..10).map(|i| app(&format!("app{i}"), 190.0)).collect();
        let shot = snapshot_of(busy, 2);

        let announced = sustained(&mut watcher, &shot, &alert_settings(), DAY_ONE);
        assert_eq!(announced.len(), ANNOUNCE_AT_MOST);
        assert_eq!(watcher.list().len(), 10, "every hit is in the list");

        // The seven that were not announced count as notified: they stay quiet for the cooldown
        // rather than arriving as the next batch.
        let next = sustained(&mut watcher, &shot, &alert_settings(), DAY_ONE + 10);
        assert!(next.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_day_key_is_the_local_calendar_day() {
        assert_eq!(day_key(DAY_ONE), crate::metrics::day_key(DAY_ONE * 1000));
        assert_ne!(day_key(DAY_ONE), day_key(DAY_TWO));
    }

    #[test]
    fn a_dismissed_row_stays_dismissed_while_the_condition_holds() {
        let dir = temp_dir("dismiss");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        let settings = alert_settings();
        let hot = || snapshot(app("chrome", 190.0));

        let first = sustained(&mut watcher, &hot(), &settings, DAY_ONE);
        assert_eq!(first.len(), 1);
        let id = first[0].id.clone();

        watcher.dismiss(&id);
        assert!(row(&watcher, &id).resolved, "dismiss resolves the row");
        assert!(
            Watcher::open(&dir).list().iter().any(|i| i.id == id && i.resolved),
            "dismiss persists, so a restart does not bring the row back"
        );

        // The condition still holds a little later — that must not resurrect the row.
        let again = sustained(&mut watcher, &hot(), &settings, DAY_ONE + 10);
        assert!(again.is_empty(), "a dismissed row must not notify again");

        let kept = row(&watcher, &id);
        assert!(kept.resolved, "the next sample must not un-resolve the row");
        assert_eq!(kept.value, first[0].value, "a dismissed row is left alone");
        assert_eq!(kept.repeat_count, 1, "and its repeat count does not creep up");
        assert_eq!(kept.at_ms, first[0].at_ms);
        assert_eq!(watcher.list().len(), 1, "and no second row is added");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `record` takes the clock as an argument, so these id-space checks are pinned to seconds
    /// of our choosing.
    #[test]
    fn a_dismissal_is_scoped_to_its_rule_app_and_day() {
        let dir = temp_dir("scoped");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        assert_ne!(day_key(DAY_ONE), day_key(DAY_TWO));
        let id = format!("cpu:chrome:{}", day_key(DAY_ONE));

        watcher
            .record(hit(InsightKind::HighCpu, "chrome", 95.0), DAY_ONE)
            .expect("the first crossing announces");
        watcher.dismiss(&id);

        // Same rule, same app, same day: nothing, and the row keeps its dismissal.
        assert!(
            watcher
                .record(hit(InsightKind::HighCpu, "chrome", 99.0), DAY_ONE)
                .is_none()
        );
        assert!(row(&watcher, &id).resolved);

        // A different rule for the same app is a different row and still announces.
        let disk = watcher
            .record(hit(InsightKind::HeavyDisk, "chrome", 5.0e9), DAY_ONE)
            .expect("another rule is unaffected");
        assert_eq!(disk.kind, InsightKind::HeavyDisk);
        assert!(!disk.resolved);

        // A different app under the same rule is unaffected too.
        let other_app = watcher
            .record(hit(InsightKind::HighCpu, "firefox", 95.0), DAY_ONE)
            .expect("another app is unaffected");
        assert_eq!(other_app.app_key, "firefox");

        // The next calendar day gets a fresh row: the dismissal lasts a day, not forever.
        let tomorrow = watcher
            .record(hit(InsightKind::HighCpu, "chrome", 95.0), DAY_TWO)
            .expect("a new day is unaffected");
        assert_ne!(tomorrow.id, id);
        assert_eq!(tomorrow.day_key, day_key(DAY_TWO));
        assert_eq!(watcher.list().len(), 4);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_row_is_refreshed_and_re_notified_after_the_cooldown() {
        let dir = temp_dir("repeat");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);

        let announced = watcher
            .record(hit(InsightKind::HighCpu, "chrome", 95.0), DAY_ONE)
            .unwrap();
        assert_eq!(announced.repeat_count, 1);

        // A repeat inside the cooldown refreshes the stored row but stays quiet.
        let later = DAY_ONE + 10;
        assert!(
            watcher
                .record(hit(InsightKind::HighCpu, "chrome", 91.5), later)
                .is_none()
        );
        let row = &watcher.list()[0];
        assert_eq!(row.repeat_count, 2);
        assert_eq!(row.value, 91.5, "the row shows the latest sample");
        assert_eq!(row.at_ms, later * 1000);

        // Once RE_NOTIFY_AFTER has passed the same row is announced again.
        let refreshed = watcher
            .record(hit(InsightKind::HighCpu, "chrome", 93.0), DAY_ONE + RE_NOTIFY_AFTER)
            .expect("the cooldown expiring re-notifies the live row");
        assert_eq!(refreshed.repeat_count, 3);
        assert!(!refreshed.resolved);
    }

    fn row(watcher: &Watcher, id: &str) -> Insight {
        watcher
            .list()
            .into_iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("{id} is not stored"))
    }

    // -- growing memory --------------------------------------------------------------------

    const GIB: u64 = 1_073_741_824;
    const MB: u64 = 1_000_000;

    /// Only the memory rule can fire: the default 1 GiB growth, nothing else reachable.
    fn memory_settings() -> Settings {
        Settings {
            alerts: AlertThresholds {
                enabled: true,
                cpu_percent: 100.0,
                memory_grow_bytes: GIB,
                disk_bytes_per_second: 1e15,
                network_bytes_per_second: 1e15,
                sustained_seconds: 30,
            },
            ..Settings::default()
        }
    }

    /// A watcher opened long before `DAY_ONE`, so the settle time is over unless a test says so.
    fn settled_watcher(tag: &str) -> (Watcher, PathBuf) {
        let dir = temp_dir(tag);
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        watcher.opened_at = DAY_ONE - 3_600;
        (watcher, dir)
    }

    /// Drives one app through a memory curve, one tick every 5 s from `from` to `to`
    /// (seconds), and returns every growing-memory insight announced on the way.
    fn drive(
        watcher: &mut Watcher,
        started_at: u64,
        from: u64,
        to: u64,
        curve: impl Fn(u64) -> u64,
    ) -> Vec<Insight> {
        let settings = memory_settings();
        let mut announced = Vec::new();
        let mut now = from;
        while now <= to {
            let mut one = app("code", 0.0);
            one.oldest_started_at_ms = Some(started_at * 1000);
            one.memory_bytes = curve(now);
            announced.extend(
                watcher
                    .evaluate_at(&snapshot(one), &settings, &Default::default(), now)
                    .into_iter()
                    .filter(|i| i.kind == InsightKind::GrowingMemory),
            );
            now += 5;
        }
        announced
    }

    /// An app started an hour before `DAY_ONE`, so its age never stands in the way.
    const LONG_AGO: u64 = DAY_ONE - 3_600;

    #[test]
    fn two_gigabytes_right_after_start_is_loading_not_leaking() {
        let (mut watcher, dir) = settled_watcher("memory-start");
        let started = DAY_ONE;
        // +2 GB over the first three minutes, then the slow creep of a warm cache for the rest
        // of the hour. Measured from launch that is 2.5 GB "growth" over twenty-odd minutes,
        // still rising — which is exactly why nothing younger than ten minutes is sampled.
        let announced = drive(&mut watcher, started, DAY_ONE, DAY_ONE + 3_600, |now| {
            let t = now - started;
            if t < 180 {
                500 * MB + 2_000 * MB * t / 180
            } else {
                2_500 * MB + 25 * MB * (t - 180) / 60
            }
        });
        assert!(announced.is_empty(), "{announced:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_steady_climb_is_announced_once_then_cools_down() {
        let (mut watcher, dir) = settled_watcher("memory-climb");
        // +1.2 GB spread over 25 minutes and still rising, then on at the same pace for hours.
        let rate = 1_200 * MB / 1_500; // bytes per second
        let announced = drive(&mut watcher, LONG_AGO, DAY_ONE, DAY_ONE + 3 * 3_600, |now| {
            GIB + rate * (now - DAY_ONE)
        });
        assert_eq!(announced.len(), 1, "once, then the six-hour cooldown: {announced:?}");
        let insight = &announced[0];
        assert!(insight.value >= GIB as f64, "value is the growth in bytes: {}", insight.value);
        assert!(insight.value < 1_300.0 * MB as f64);
        assert!(
            (MEMORY_MIN_SPAN_SECONDS..=MEMORY_WINDOW_SECONDS).contains(&insight.sustained_seconds),
            "sustained_seconds is the span the growth was measured over: {}",
            insight.sustained_seconds
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_step_followed_by_a_plateau_is_not_a_leak() {
        let (mut watcher, dir) = settled_watcher("memory-step");
        // Steady for twenty minutes, +2 GB within a minute, then flat for ten more minutes.
        let step_at = DAY_ONE + 1_200;
        let announced = drive(&mut watcher, LONG_AGO, DAY_ONE, step_at + 660, |now| {
            if now < step_at {
                GIB
            } else {
                GIB + (2_000 * MB).min(2_000 * MB * (now - step_at) / 60)
            }
        });
        assert!(announced.is_empty(), "{announced:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rise_that_falls_back_is_not_a_leak() {
        let (mut watcher, dir) = settled_watcher("memory-fall");
        // Up 1.5 GB in three minutes, back down, then a slow climb that never regains the peak.
        let announced = drive(&mut watcher, LONG_AGO, DAY_ONE, DAY_ONE + 1_800, |now| {
            let t = now - DAY_ONE;
            match t {
                0..=179 => GIB + 1_500 * MB * t / 180,
                180..=239 => GIB + 1_500 * MB - 1_500 * MB * (t - 180) / 60,
                _ => GIB + 1_300 * MB * (t - 240) / 1_560,
            }
        });
        assert!(announced.is_empty(), "below 97% of the window's high: {announced:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_process_younger_than_ten_minutes_is_not_judged() {
        let (mut watcher, dir) = settled_watcher("memory-young");
        let started = DAY_ONE;
        // A steep, steady climb over the first ten minutes of the app's life.
        let climb = |now: u64| GIB + 3_000 * MB * (now - started) / 540;
        let announced = drive(&mut watcher, started, DAY_ONE, DAY_ONE + 595, climb);
        assert!(announced.is_empty(), "{announced:?}");
        assert!(
            watcher.states["code"].memory.is_empty(),
            "no baseline is taken while the app is still starting"
        );
        let _ = drive(&mut watcher, started, DAY_ONE + 600, DAY_ONE + 600, climb);
        assert_eq!(
            watcher.states["code"].memory.front().map(|(at, _)| *at),
            Some(DAY_ONE + 600),
            "the window opens at ten minutes of age"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_about_memory_is_judged_while_sign_in_settles() {
        let dir = temp_dir("memory-settle");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        watcher.opened_at = DAY_ONE;
        let rate = 1_200 * MB / 1_500;
        let _ = drive(&mut watcher, LONG_AGO, DAY_ONE, DAY_ONE + SETTLE_SECONDS - 5, |now| {
            GIB + rate * (now - DAY_ONE)
        });
        assert!(
            watcher.states.values().all(|w| w.memory.is_empty()),
            "no samples, so no baseline, during the first three minutes"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn growth_that_doubles_breaks_the_cooldown() {
        let (mut watcher, dir) = settled_watcher("memory-double");
        // 0.8 MB/s: announced once the 30-minute window holds 1 GiB of growth, and the window
        // can never hold twice that at this pace, so the cooldown keeps it quiet.
        let slow = 1_200 * MB / 1_500;
        let first = drive(&mut watcher, LONG_AGO, DAY_ONE, DAY_ONE + 3_000, |now| {
            GIB + slow * (now - DAY_ONE)
        });
        assert_eq!(first.len(), 1, "{first:?}");
        let announced_growth = first[0].value;

        // Then the leak speeds up to 2.5 MB/s, well inside the six-hour cooldown.
        let base = GIB + slow * 3_000;
        let fast = 2_500_000;
        let second = drive(&mut watcher, LONG_AGO, DAY_ONE + 3_005, DAY_ONE + 3_600, |now| {
            base + fast * (now - DAY_ONE - 3_000)
        });
        assert_eq!(second.len(), 1, "{second:?}");
        assert!(second[0].value >= 2.0 * announced_growth);
        assert_eq!(second[0].repeat_count, 2, "the same row for the day, refreshed");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_growth_rule_in_isolation() {
        let window = |points: &[(u64, u64)]| points.iter().copied().collect::<VecDeque<_>>();
        // Too short a span, however steep.
        assert_eq!(memory_growth(&window(&[(0, 0), (300, 2 * GIB)]), GIB), None);
        // Ten minutes of steady growth, still rising.
        assert_eq!(
            memory_growth(&window(&[(0, 0), (300, GIB), (600, 2 * GIB)]), GIB),
            Some(Growth { bytes: 2 * GIB, over_seconds: 600 })
        );
        // The same growth, but the last five minutes were flat.
        assert_eq!(memory_growth(&window(&[(0, 0), (300, 2 * GIB), (600, 2 * GIB)]), GIB), None);
        // The same growth, but well off its high.
        assert_eq!(
            memory_growth(&window(&[(0, 0), (300, 3 * GIB), (600, 2 * GIB)]), GIB),
            None
        );
        // Under the threshold.
        assert_eq!(memory_growth(&window(&[(0, 0), (300, GIB / 4), (600, GIB / 2)]), GIB), None);
    }
}
