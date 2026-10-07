//! Rolling history. One minute resolution for the last 2 hours, 5 minutes for the last
//! day, 1 hour for 30 days. Per-app detail is kept hourly for 48 hours, which is all
//! "which app was busy at 3am" needs.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{HistoryPoint, HistorySeries, RankedApp, RankingRow, Snapshot};

const FINE_BUCKET: u64 = 60;
const COARSE_BUCKET: u64 = 300;
const DAY_BUCKET: u64 = 3600;
const FINE_KEEP: u64 = 2 * 3600;
const COARSE_KEEP: u64 = 24 * 3600;
const TOTAL_KEEP: u64 = 30 * 24 * 3600;
const APP_KEEP: u64 = 48 * 3600;

const METRICS: [&str; 12] = [
    "cpu",
    "memory",
    "memoryUsed",
    "memoryTotal",
    "available",
    "committed",
    "diskRead",
    "diskWrite",
    "download",
    "upload",
    "gpu",
    "temp",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredPoint {
    pub at: u64,
    pub values: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredHour {
    pub at: u64,
    pub apps: Vec<RankedApp>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub points: Vec<StoredPoint>,
    pub app_hours: Vec<StoredHour>,
}

#[derive(Default)]
struct Accumulator {
    bucket: u64,
    sums: BTreeMap<String, f64>,
    count: u32,
}

pub struct History {
    path: PathBuf,
    store: Store,
    current: Accumulator,
    ticks: u32,
    last_hour_recorded: u64,
}

impl History {
    pub fn open(dir: &PathBuf) -> Self {
        let path = dir.join("history.json");
        let store = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Store>(&raw).ok())
            .unwrap_or_default();
        let mut this = Self {
            path,
            store,
            current: Accumulator::default(),
            ticks: 0,
            last_hour_recorded: 0,
        };
        let now = now_seconds();
        this.fold(now);
        this
    }

    pub fn record(&mut self, snapshot: &Snapshot) {
        let second = snapshot.taken_at_ms / 1000;
        let bucket = second - second % FINE_BUCKET;

        if self.current.bucket != bucket && self.current.count > 0 {
            self.flush();
        }
        self.current.bucket = bucket;
        for metric in METRICS {
            let value = metric_value(snapshot, metric);
            *self.current.sums.entry(metric.to_string()).or_insert(0.0) += value;
        }
        self.current.count += 1;

        self.ticks += 1;
        if self.ticks >= 60 {
            self.ticks = 0;
            self.flush();
            self.record_apps(snapshot);
            self.fold(second);
            self.save();
        }
    }

    fn flush(&mut self) {
        if self.current.count == 0 {
            return;
        }
        let divisor = self.current.count as f64;
        let values: BTreeMap<String, f64> = self
            .current
            .sums
            .iter()
            .map(|(key, sum)| (key.clone(), sum / divisor))
            .collect();
        let at = self.current.bucket;
        self.store.points.push(StoredPoint { at, values });
        self.store.points.sort_by_key(|p| p.at);
        self.store.points.dedup_by_key(|p| p.at);
        self.current = Accumulator { bucket: at, ..Default::default() };
    }

    /// Fold what aged out of the fine tier into coarser buckets, then drop the rest.
    fn fold(&mut self, now: u64) {
        self.store.points = fold_older(std::mem::take(&mut self.store.points), now.saturating_sub(FINE_KEEP), COARSE_BUCKET);
        self.store.points = fold_older(std::mem::take(&mut self.store.points), now.saturating_sub(COARSE_KEEP), DAY_BUCKET);
        let oldest = now.saturating_sub(TOTAL_KEEP);
        self.store.points.retain(|p| p.at >= oldest);
        self.store.app_hours.retain(|h| h.at >= now.saturating_sub(APP_KEEP));
    }

    fn record_apps(&mut self, snapshot: &Snapshot) {
        let second = snapshot.taken_at_ms / 1000;
        let hour = second - second % DAY_BUCKET;
        if hour == self.last_hour_recorded {
            return;
        }
        self.last_hour_recorded = hour;
        let mut apps: Vec<RankedApp> = snapshot
            .apps
            .iter()
            .take(40)
            .map(|app| RankedApp {
                key: app.key.clone(),
                name: app.name.clone(),
                value: app.memory_bytes as f64,
            })
            .collect();
        apps.sort_by(|a, b| b.value.total_cmp(&a.value));
        self.store.app_hours.push(StoredHour { at: hour, apps });
        self.store.app_hours.sort_by_key(|h| h.at);
        self.store.app_hours.dedup_by_key(|h| h.at);
    }

    pub fn series(&self, metric: &str, range_seconds: u64, now_seconds: u64) -> HistorySeries {
        let bucket = if range_seconds <= FINE_KEEP {
            FINE_BUCKET
        } else if range_seconds <= COARSE_KEEP {
            COARSE_BUCKET
        } else {
            DAY_BUCKET
        };
        let from = now_seconds.saturating_sub(range_seconds);
        let points = self
            .store
            .points
            .iter()
            .filter(|p| p.at >= from && p.at % bucket == 0)
            .filter_map(|p| {
                p.values.get(metric).map(|value| HistoryPoint {
                    at_ms: p.at * 1000,
                    value: *value,
                    secondary: None,
                })
            })
            .collect();
        HistorySeries {
            metric: metric.to_string(),
            bucket_seconds: bucket,
            range_seconds,
            points,
        }
    }

    pub fn ranking(&self, hours: u64, now_seconds: u64) -> Vec<RankingRow> {
        let from = now_seconds.saturating_sub(hours * 3600);
        self.store
            .app_hours
            .iter()
            .filter(|h| h.at >= from)
            .map(|h| RankingRow {
                hour_start_ms: h.at * 1000,
                metric: "memory".to_string(),
                apps: h.apps.clone(),
            })
            .collect()
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(raw) = serde_json::to_string(&self.store) else {
            return;
        };
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        if fs::write(&tmp, raw).is_ok() {
            let _ = fs::rename(&tmp, &self.path);
        }
    }
}

fn fold_older(points: Vec<StoredPoint>, horizon: u64, bucket: u64) -> Vec<StoredPoint> {
    let mut recent: Vec<StoredPoint> = Vec::with_capacity(points.len());
    let mut groups: BTreeMap<u64, Vec<StoredPoint>> = BTreeMap::new();
    for point in points {
        if point.at >= horizon {
            recent.push(point);
        } else {
            let key = point.at - point.at % bucket;
            groups.entry(key).or_default().push(point);
        }
    }
    let mut folded: Vec<StoredPoint> = groups
        .into_iter()
        .map(|(at, group)| {
            let mut sums: BTreeMap<String, f64> = BTreeMap::new();
            for point in &group {
                for (key, value) in &point.values {
                    *sums.entry(key.clone()).or_insert(0.0) += *value;
                }
            }
            let values = sums
                .into_iter()
                .map(|(key, sum)| (key, sum / group.len() as f64))
                .collect();
            StoredPoint { at, values }
        })
        .collect();
    folded.extend(recent);
    folded.sort_by_key(|p| p.at);
    folded.dedup_by_key(|p| p.at);
    folded
}

fn metric_value(snapshot: &Snapshot, metric: &str) -> f64 {
    match metric {
        "cpu" => snapshot.cpu.total_percent,
        "memory" => snapshot.memory.pressure_percent,
        "memoryUsed" => snapshot.memory.in_use_bytes as f64,
        "memoryTotal" => snapshot.memory.total_bytes as f64,
        "available" => snapshot.memory.available_bytes as f64,
        "committed" => snapshot.memory.committed_bytes as f64,
        "diskRead" => snapshot.disk.read_bytes_per_second,
        "diskWrite" => snapshot.disk.write_bytes_per_second,
        "download" => snapshot.network.download_bytes_per_second,
        "upload" => snapshot.network.upload_bytes_per_second,
        "gpu" => snapshot.gpu.total_percent.unwrap_or(0.0),
        "temp" => snapshot.temperature.hottest.unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
