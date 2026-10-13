//! Worth a look: the rules that turn raw figures into something worth telling the user
//! about, with a repeat count instead of a pile of duplicates.
//!
//! Per-app network bytes are not available without an elevated ETW session, so a heavy
//! network insight names the app holding the most connections and says so in its detail.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::history::now_seconds;
use crate::models::{Insight, InsightKind, Snapshot};
use crate::settings::Settings;

const KEEP: usize = 400;
const RE_NOTIFY_AFTER: u64 = 30 * 60;
const HEAT_CELSIUS: f64 = 85.0;

#[derive(Clone, Default)]
struct Watch {
    cpu_since: Option<u64>,
    disk_since: Option<u64>,
    baseline_memory: u64,
    started_at: u64,
}

struct Hit {
    kind: InsightKind,
    key: String,
    name: String,
    value: f64,
    sustained: u64,
}

pub struct Watcher {
    path: PathBuf,
    states: HashMap<String, Watch>,
    notified_at: HashMap<String, u64>,
    insights: Vec<Insight>,
    heat_since: Option<u64>,
    network_since: Option<u64>,
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
        }
    }

    /// Insights worth a notification since the last tick. Repeats update the stored row
    /// and stay quiet until RE_NOTIFY_AFTER has passed.
    pub fn evaluate(&mut self, snapshot: &Snapshot, settings: &Settings) -> Vec<Insight> {
        let now = now_seconds();
        if !settings.alerts.enabled {
            self.states.clear();
            return Vec::new();
        }

        let mut hits: Vec<Hit> = Vec::new();
        for app in &snapshot.apps {
            let mut state = self.states.get(&app.key).cloned().unwrap_or_default();
            if state.baseline_memory == 0 {
                state.baseline_memory = app.memory_bytes;
                state.started_at = now;
            }

            if app.cpu_percent >= settings.alerts.cpu_percent {
                let since = *state.cpu_since.get_or_insert(now);
                if now.saturating_sub(since) >= settings.alerts.sustained_seconds {
                    hits.push(Hit {
                        kind: InsightKind::HighCpu,
                        key: app.key.clone(),
                        name: app.name.clone(),
                        value: app.cpu_percent,
                        sustained: now.saturating_sub(since),
                    });
                    state.cpu_since = None;
                }
            } else {
                state.cpu_since = None;
            }

            let rate = app.read_bytes_per_second + app.write_bytes_per_second;
            if rate >= settings.alerts.disk_bytes_per_second {
                let since = *state.disk_since.get_or_insert(now);
                if now.saturating_sub(since) >= settings.alerts.sustained_seconds {
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

            let growth = app.memory_bytes.saturating_sub(state.baseline_memory);
            if growth >= settings.alerts.memory_grow_bytes
                && now.saturating_sub(state.started_at) >= settings.alerts.sustained_seconds
            {
                hits.push(Hit {
                    kind: InsightKind::GrowingMemory,
                    key: app.key.clone(),
                    name: app.name.clone(),
                    value: growth as f64,
                    sustained: now.saturating_sub(state.started_at),
                });
                state.baseline_memory = app.memory_bytes;
                state.started_at = now;
            }

            self.states.insert(app.key.clone(), state);
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
        if traffic >= settings.alerts.network_bytes_per_second {
            let since = *self.network_since.get_or_insert(now);
            if now.saturating_sub(since) >= settings.alerts.sustained_seconds {
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

        self.states
            .retain(|key, _| snapshot.apps.iter().any(|a| &a.key == key));

        let mut announced = Vec::new();
        let mut dirty = false;
        for hit in hits {
            if let Some(insight) = self.record(hit, now) {
                announced.push(insight);
                dirty = true;
            } else {
                dirty = true;
            }
        }
        if dirty {
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

fn kind_key(kind: &InsightKind) -> &'static str {
    match kind {
        InsightKind::HighCpu => "cpu",
        InsightKind::GrowingMemory => "memory",
        InsightKind::HeavyDisk => "disk",
        InsightKind::HeavyNetwork => "network",
        InsightKind::Heat => "heat",
    }
}

fn day_key(now_seconds: u64) -> String {
    (now_seconds / 86_400).to_string()
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
        std::env::temp_dir().join(format!("pulse-alerts-{name}-{}", std::process::id()))
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
        }
    }

    fn snapshot(one: AppInfo) -> Snapshot {
        Snapshot {
            taken_at_ms: 0,
            machine: MachineInfo {
                hostname: "test-host".into(),
                os_version: "test".into(),
                chip_name: "test".into(),
                physical_cores: Some(2),
                logical_cores: 2,
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
            apps: vec![one],
            process_count: 1,
            sampler_cost_microseconds: 0,
        }
    }

    /// Alerts on with no settle time and unreachable disk/memory/network thresholds, so the CPU
    /// rule is the only one that can fire — and it fires on the first tick whatever the clock
    /// currently reads.
    fn alert_settings() -> Settings {
        Settings {
            alerts: AlertThresholds {
                enabled: true,
                cpu_percent: 80.0,
                memory_grow_bytes: u64::MAX,
                disk_bytes_per_second: f64::INFINITY,
                network_bytes_per_second: f64::INFINITY,
                sustained_seconds: 0,
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

    /// A fixed second inside some UTC day, for tests that drive `record` directly so they never
    /// depend on the wall clock.
    const DAY_ONE: u64 = 1_700_000_000;
    const DAY_TWO: u64 = DAY_ONE + 86_400;

    #[test]
    fn crossing_the_threshold_records_one_insight() {
        let dir = temp_dir("records");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);

        let announced = watcher.evaluate(&snapshot(app("chrome", 95.0)), &alert_settings());

        assert_eq!(announced.len(), 1, "one sustained app yields one row");
        let insight = &announced[0];
        assert_eq!(insight.kind, InsightKind::HighCpu);
        assert_eq!(insight.app_key, "chrome");
        assert_eq!(insight.repeat_count, 1);
        assert!(!insight.resolved);
        assert_eq!(insight.day_key, day_key(now_seconds()));
        assert_eq!(insight.id, format!("cpu:chrome:{}", insight.day_key));
        assert_eq!(watcher.list().len(), 1);

        // Quiet thresholds yield nothing at all.
        assert!(
            watcher
                .evaluate(&snapshot(app("chrome", 10.0)), &alert_settings())
                .is_empty()
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dismissed_row_stays_dismissed_while_the_condition_holds() {
        let dir = temp_dir("dismiss");
        let _ = fs::remove_dir_all(&dir);
        let mut watcher = Watcher::open(&dir);
        let settings = alert_settings();
        let hot = || snapshot(app("chrome", 95.0));

        let first = watcher.evaluate(&hot(), &settings);
        assert_eq!(first.len(), 1);
        let id = first[0].id.clone();
        let day = day_key(now_seconds());

        watcher.dismiss(&id);
        assert!(row(&watcher, &id).resolved, "dismiss resolves the row");
        assert!(
            Watcher::open(&dir).list().iter().any(|i| i.id == id && i.resolved),
            "dismiss persists, so a restart does not bring the row back"
        );

        // The condition still holds one second later — that must not resurrect the row.
        if day_key(now_seconds()) != day {
            // Crossed UTC midnight between the two evaluations: the ids would legitimately
            // differ, so give up rather than flake.
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let again = watcher.evaluate(&hot(), &settings);
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
        let _ = fs::remove_dir_all(&dir);
    }

    fn row(watcher: &Watcher, id: &str) -> Insight {
        watcher
            .list()
            .into_iter()
            .find(|i| i.id == id)
            .unwrap_or_else(|| panic!("{id} is not stored"))
    }
}
