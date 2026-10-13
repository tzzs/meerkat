//! Rolling history of the machine as a whole. One minute resolution for the last 2 hours,
//! 5 minutes for the last day, 1 hour for 30 days. What each app did is kept separately, per
//! minute, in apphistory.rs.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{HistoryPoint, HistorySeries, Snapshot};

const FINE_BUCKET: u64 = 60;
const COARSE_BUCKET: u64 = 300;
const DAY_BUCKET: u64 = 3600;
const FINE_KEEP: u64 = 2 * 3600;
const COARSE_KEEP: u64 = 24 * 3600;
const TOTAL_KEEP: u64 = 30 * 24 * 3600;

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

/// Files written before per-app history moved out still carry an `app_hours` list; serde skips
/// the field it no longer knows, so they open as they always did.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub points: Vec<StoredPoint>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn point(at: u64, values: &[(&str, f64)]) -> StoredPoint {
        StoredPoint { at, values: values.iter().map(|(k, v)| (k.to_string(), *v)).collect() }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pulse-history-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir to test against");
        dir
    }

    #[test]
    fn the_horizon_is_where_fine_detail_stops_being_fine() {
        // A point exactly at the horizon is still recent: `>=` is what keeps the newest minute
        // of every bucket from being averaged with history.
        let horizon = 10_000;
        let out = fold_older(vec![point(horizon, &[("cpu", 40.0)])], horizon, 300);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].at, horizon);
        assert_eq!(out[0].values["cpu"], 40.0);
    }

    #[test]
    fn older_points_average_onto_their_bucket_start() {
        let out = fold_older(
            vec![
                point(1_000, &[("cpu", 10.0)]),
                point(1_060, &[("cpu", 30.0)]),
                point(1_120, &[("cpu", 20.0)]),
            ],
            5_000,
            300,
        );
        assert_eq!(out.len(), 1, "three points inside one five-minute bucket");
        assert_eq!(out[0].at, 900, "the bucket start, not the first sample");
        assert_eq!(out[0].values["cpu"], 20.0, "(10 + 30 + 20) / 3");
    }

    #[test]
    fn a_metric_missing_from_part_of_a_bucket_is_diluted_not_skipped() {
        // The divisor is the size of the bucket, so a metric only some points carry reads low.
        // Pinned deliberately: changing it changes every historical chart's old edge.
        let out = fold_older(
            vec![point(1_000, &[("cpu", 10.0)]), point(1_060, &[("gpu", 50.0)])],
            5_000,
            300,
        );
        assert_eq!(out[0].values["cpu"], 5.0);
        assert_eq!(out[0].values["gpu"], 25.0);
    }

    #[test]
    fn three_tiers_survive_one_fold() {
        // A real epoch, because 30 days of retention subtracted from a made-up small `now`
        // underflows and panics before the assertion it was meant to check ever runs.
        let now = 2_000_000_000;
        let dir = temp_dir("tiers");
        let mut h = History::open(&dir);
        h.store.points = vec![
            point(now - 120, &[("cpu", 1.0)]), // fine: stays where it is
            point(now - FINE_KEEP - 300, &[("cpu", 2.0)]), // coarse: five-minute bucket
            point(now - COARSE_KEEP - 3_600, &[("cpu", 3.0)]), // hourly bucket
            point(now - TOTAL_KEEP - 86_400, &[("cpu", 4.0)]), // gone
        ];
        h.fold(now);

        let at = |p: &StoredPoint| p.at;
        let recent = h.store.points.iter().find(|p| at(p) > now - FINE_KEEP).expect("the fine point");
        assert_eq!(at(recent), now - 120, "the fine tier is never re-bucketed");

        let mid = h.store.points.iter().filter(|p| {
            let a = at(p);
            a <= now - FINE_KEEP && a > now - COARSE_KEEP
        }).collect::<Vec<_>>();
        assert!(mid.iter().all(|p| at(p) % COARSE_BUCKET == 0), "the middle tier is five-minute aligned");

        let old = h.store.points.iter().filter(|p| at(p) <= now - COARSE_KEEP).collect::<Vec<_>>();
        assert!(!old.is_empty(), "the day-old points are still there, an hour apart");
        assert!(old.iter().all(|p| at(p) % DAY_BUCKET == 0), "the old tier is hourly aligned");
        assert!(h.store.points.iter().all(|p| at(p) >= now - TOTAL_KEEP), "nothing older than 30 days survives");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_history_file_broken_by_a_power_cut_starts_over() {
        let dir = temp_dir("corrupt");
        fs::write(dir.join("history.json"), b"{ not json, truncated mid-write").unwrap();
        let h = History::open(&dir);
        assert!(h.store.points.is_empty(), "a corrupt file reads as no history, not as a panic");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn what_is_saved_is_what_opens_again() {
        let now = now_seconds();
        let dir = temp_dir("roundtrip");
        {
            let mut h = History::open(&dir);
            h.store.points = vec![point(now - 180, &[("cpu", 7.5)]), point(now - 120, &[("cpu", 8.5)])];
            h.save();
        }
        let back = History::open(&dir);
        assert_eq!(back.store.points.len(), 2);
        assert_eq!(back.store.points[0].values["cpu"], 7.5);
        assert!(!dir.join("history.json.tmp").exists(), "the temp file is renamed, not left behind");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn opening_a_file_puts_it_back_in_order() {
        // The chart indexes points by position, so a store that came off disk out of order —
        // an older build, a hand-edited file — has to be straightened before anything reads it.
        let now = now_seconds() - now_seconds() % 60;
        let dir = temp_dir("unsorted");
        let store = Store {
            points: vec![
                point(now - 300, &[("cpu", 1.0)]),
                point(now - 900, &[("cpu", 3.0)]),
                point(now - 600, &[("cpu", 2.0)]),
            ],
        };
        fs::write(dir.join("history.json"), serde_json::to_string(&store).unwrap()).unwrap();

        let h = History::open(&dir);
        let at: Vec<u64> = h.store.points.iter().map(|p| p.at).collect();
        let mut sorted = at.clone();
        sorted.sort_unstable();
        assert_eq!(at, sorted, "ascending after open()");
        assert_eq!(h.series("cpu", 3_600, now).points.len(), 3, "the whole window survives the sort");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_series_is_windowed_by_range_and_bucket() {
        // A `now` that is a multiple of both 60 and 300, so "one minute ago" really is on a
        // minute boundary; the query drops anything that is not aligned to its bucket.
        let now = 1_000_200;
        let dir = temp_dir("series");
        let mut h = History::open(&dir);
        // Ascending, which is the order record() keeps the store in; the query preserves it
        // rather than sorting, and the chart relies on that.
        h.store.points = vec![
            point(now - 7_200, &[("cpu", 4.0)]),
            point(now - 1_830, &[("cpu", 5.0)]), // not on a minute boundary: never shown
            point(now - 1_800, &[("cpu", 3.0)]),
            point(now - 600, &[("cpu", 2.0)]),
            point(now - 300, &[("cpu", 1.0)]),
        ];

        let hour = h.series("cpu", 3_600, now);
        assert_eq!(hour.bucket_seconds, FINE_BUCKET, "an hour of history is fine detail");
        let seen: Vec<u64> = hour.points.iter().map(|p| p.at_ms / 1000).collect();
        assert_eq!(seen, vec![now - 1_800, now - 600, now - 300], "ascending, inside the window");

        let day = h.series("cpu", 86_400, now);
        assert_eq!(day.bucket_seconds, COARSE_BUCKET, "a day of history is five-minute detail");
        assert_eq!(day.points.len(), 4, "the window widens to reach the hour-old point");

        assert!(h.series("nope", 3_600, now).points.is_empty(), "a metric nobody stored is empty, not an error");
        let _ = fs::remove_dir_all(dir);
    }
}
