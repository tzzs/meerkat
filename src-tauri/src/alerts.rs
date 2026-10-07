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
                if now - since >= settings.alerts.sustained_seconds {
                    hits.push(Hit {
                        kind: InsightKind::HighCpu,
                        key: app.key.clone(),
                        name: app.name.clone(),
                        value: app.cpu_percent,
                        sustained: now - since,
                    });
                    state.cpu_since = None;
                }
            } else {
                state.cpu_since = None;
            }

            let rate = app.read_bytes_per_second + app.write_bytes_per_second;
            if rate >= settings.alerts.disk_bytes_per_second {
                let since = *state.disk_since.get_or_insert(now);
                if now - since >= settings.alerts.sustained_seconds {
                    hits.push(Hit {
                        kind: InsightKind::HeavyDisk,
                        key: app.key.clone(),
                        name: app.name.clone(),
                        value: rate,
                        sustained: now - since,
                    });
                    state.disk_since = None;
                }
            } else {
                state.disk_since = None;
            }

            let growth = app.memory_bytes.saturating_sub(state.baseline_memory);
            if growth >= settings.alerts.memory_grow_bytes
                && now - state.started_at >= settings.alerts.sustained_seconds
            {
                hits.push(Hit {
                    kind: InsightKind::GrowingMemory,
                    key: app.key.clone(),
                    name: app.name.clone(),
                    value: growth as f64,
                    sustained: now - state.started_at,
                });
                state.baseline_memory = app.memory_bytes;
                state.started_at = now;
            }

            self.states.insert(app.key.clone(), state);
        }

        if let Some(hottest) = snapshot.temperature.hottest {
            if hottest >= HEAT_CELSIUS {
                let since = *self.heat_since.get_or_insert(now);
                if now - since >= 60 {
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
                            sustained: now - since,
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
            if now - since >= settings.alerts.sustained_seconds {
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
                        sustained: now - since,
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
    fn record(&mut self, hit: Hit, now: u64) -> Option<Insight> {
        let id = format!("{}:{}:{}", kind_key(&hit.kind), hit.key, day_key(now));
        if let Some(existing) = self.insights.iter_mut().find(|i| i.id == id) {
            existing.repeat_count += 1;
            existing.at_ms = now * 1000;
            existing.value = hit.value;
            existing.sustained_seconds = hit.sustained;
            existing.resolved = false;
            let last = self.notified_at.get(&id).copied().unwrap_or(0);
            if now - last >= RE_NOTIFY_AFTER {
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
