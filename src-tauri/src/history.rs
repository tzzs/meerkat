//! Rolling history. One minute resolution for the last 2 hours, 5 minutes for the last
//! day, 1 hour for 30 days. Per-app detail is kept hourly for 48 hours, which is all
//! "which app was busy at 3am" needs.
//!
//! Every stored point is a mean together with the number of samples behind it (`n`) and the
//! highest sample of each metric (`max`). Folding a tier into a coarser one and charting at any
//! bucket width are therefore exact weighted averages, and a one-minute spike survives as the
//! bucket's max instead of being averaged away. A metric that could not be measured at the
//! time is simply absent from the point: a gap, never a zero.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::models::{HistoryPoint, HistorySeries, RankedApp, RankingRow, Snapshot};

const FINE_BUCKET: u64 = 60;
const COARSE_BUCKET: u64 = 300;
const DAY_BUCKET: u64 = 3600;
const FINE_KEEP: u64 = 2 * 3600;
const COARSE_KEEP: u64 = 24 * 3600;
const TOTAL_KEEP: u64 = 30 * 24 * 3600;
const APP_KEEP: u64 = 48 * 3600;

/// Aged points are folded into the coarser tiers this often (wall clock).
const FOLD_EVERY: u64 = 10 * 60;
/// Changes are written at most this often (wall clock), plus after every fold and on `flush`.
const SAVE_EVERY: u64 = 5 * 60;
/// A gap between two samples longer than this (sleep, a stalled sampler) is not credited to
/// whichever apps happened to be running just before it.
const MAX_SAMPLE_GAP_SECONDS: f64 = 10.0;
const MEMORY_RANKED: usize = 40;
const CPU_RANKED: usize = 10;

const METRICS: [&str; 13] = [
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
    "battery",
    "temp",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredPoint {
    /// Bucket start, Unix seconds.
    pub at: u64,
    /// Mean of each metric over the bucket.
    pub values: BTreeMap<String, f64>,
    /// Highest single sample of each metric in the bucket. Files from before it existed load
    /// with an empty map, and the mean then stands in for the max.
    #[serde(default)]
    pub max: BTreeMap<String, f64>,
    /// Samples behind the means: this point's weight whenever it is folded or charted.
    #[serde(default = "one")]
    pub n: u32,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredHour {
    /// Hour start, Unix seconds.
    pub at: u64,
    /// Memory ranking: mean resident bytes over the part of the hour Pulse saw, top 40.
    pub apps: Vec<RankedApp>,
    /// CPU ranking: mean percent of the whole machine over the same span, top 10.
    #[serde(default)]
    pub cpu: Vec<RankedApp>,
    /// Seconds of the hour that were observed — the weight when two runs' halves of one hour
    /// are merged. Zero in files from before it existed.
    #[serde(default)]
    pub seconds: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub points: Vec<StoredPoint>,
    pub app_hours: Vec<StoredHour>,
}

#[derive(Clone, Copy)]
struct Stat {
    sum: f64,
    count: u32,
    max: f64,
}

/// The minute being collected.
#[derive(Default)]
struct Minute {
    bucket: u64,
    samples: u32,
    stats: BTreeMap<&'static str, Stat>,
}

#[derive(Default)]
struct AppSeconds {
    name: String,
    /// Percent of the machine × seconds.
    cpu: f64,
    /// Bytes × seconds.
    memory: f64,
}

/// The hour being collected for the rankings: time-weighted, because the sampler runs at
/// 1, 2 or 4 second intervals depending on what is on screen and on the power source.
#[derive(Default)]
struct Hour {
    at: u64,
    seconds: f64,
    apps: HashMap<String, AppSeconds>,
}

pub struct History {
    store: Store,
    minute: Minute,
    hour: Hour,
    last_sample_ms: Option<u64>,
    last_fold: u64,
    last_save: u64,
    dirty: bool,
    generation: u64,
    disk: Arc<Disk>,
}

impl History {
    pub fn open(dir: &PathBuf) -> Self {
        Self::open_at(dir, now_seconds())
    }

    fn open_at(dir: &PathBuf, now: u64) -> Self {
        let path = dir.join("history.json");
        let mut store = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Store>(&raw).ok())
            .unwrap_or_default();
        // The chart reads points by position, so a file that came off disk out of order or
        // with two points for one bucket (an older build, a hand edit) is straightened first.
        store.points = normalize(std::mem::take(&mut store.points));
        store.app_hours.sort_by_key(|h| h.at);
        let hours = std::mem::take(&mut store.app_hours);
        for hour in hours {
            merge_hour(&mut store.app_hours, hour);
        }
        let mut this = Self {
            store,
            minute: Minute::default(),
            hour: Hour::default(),
            last_sample_ms: None,
            last_fold: now,
            last_save: now,
            dirty: false,
            generation: 0,
            disk: Arc::new(Disk::new(path)),
        };
        this.fold(now);
        this
    }

    /// Takes one sample. Everything here is in memory: a minute is stored when the next one
    /// starts, folds and saves run on wall-clock intervals, and the file itself is written
    /// from a short-lived thread so the caller's lock is never held across disk IO.
    pub fn record(&mut self, snapshot: &Snapshot) {
        let second = snapshot.taken_at_ms / 1000;
        let bucket = second - second % FINE_BUCKET;
        if self.minute.samples > 0 && self.minute.bucket != bucket {
            self.close_minute();
        }
        self.minute.bucket = bucket;
        self.minute.samples += 1;
        for metric in METRICS {
            let Some(value) = metric_value(snapshot, metric) else {
                continue;
            };
            let stat = self.minute.stats.entry(metric).or_insert(Stat {
                sum: 0.0,
                count: 0,
                max: value,
            });
            stat.sum += value;
            stat.count += 1;
            stat.max = stat.max.max(value);
        }

        self.record_apps(snapshot);

        if interval_elapsed(&mut self.last_fold, second, FOLD_EVERY) {
            self.last_fold = second;
            self.fold(second);
            self.save(second);
        } else if self.dirty && interval_elapsed(&mut self.last_save, second, SAVE_EVERY) {
            self.save(second);
        }
    }

    /// Stores the minute and the hour collected so far and writes the file before returning.
    /// For app exit: whatever was recorded since the last save would otherwise be lost. A run
    /// that starts again inside the same minute or hour merges into what this stored.
    // Called from the app's exit path in main.rs; until that hook lands nothing else calls it.
    #[allow(dead_code)]
    pub fn flush(&mut self) {
        self.close_minute();
        self.close_hour();
        match serde_json::to_vec(&self.store) {
            Ok(bytes) => {
                self.generation += 1;
                self.dirty = false;
                // Anything still queued for the background writer is older and is dropped.
                self.disk.write(self.generation, &bytes);
            }
            Err(error) => crate::diag::error(format!("history: could not serialise for flush: {error}")),
        }
    }

    fn close_minute(&mut self) {
        let minute = std::mem::take(&mut self.minute);
        if minute.samples == 0 {
            return;
        }
        let mut values = BTreeMap::new();
        let mut max = BTreeMap::new();
        for (metric, stat) in minute.stats {
            if stat.count == 0 {
                continue;
            }
            values.insert(metric.to_string(), stat.sum / stat.count as f64);
            max.insert(metric.to_string(), stat.max);
        }
        if values.is_empty() {
            return;
        }
        merge_point(
            &mut self.store.points,
            StoredPoint {
                at: minute.bucket,
                values,
                max,
                n: minute.samples,
            },
        );
        self.dirty = true;
    }

    /// Fold what aged out of the fine tier into coarser buckets, then drop the rest.
    fn fold(&mut self, now: u64) {
        let before = (self.store.points.len(), self.store.app_hours.len());
        self.store.points = fold_older(std::mem::take(&mut self.store.points), now.saturating_sub(FINE_KEEP), COARSE_BUCKET);
        self.store.points = fold_older(std::mem::take(&mut self.store.points), now.saturating_sub(COARSE_KEEP), DAY_BUCKET);
        let oldest = now.saturating_sub(TOTAL_KEEP);
        self.store.points.retain(|p| p.at >= oldest);
        self.store.app_hours.retain(|h| h.at >= now.saturating_sub(APP_KEEP));
        if (self.store.points.len(), self.store.app_hours.len()) != before {
            self.dirty = true;
        }
    }

    fn record_apps(&mut self, snapshot: &Snapshot) {
        let at_ms = snapshot.taken_at_ms;
        let second = at_ms / 1000;
        let hour = second - second % DAY_BUCKET;
        if self.hour.seconds > 0.0 && self.hour.at != hour {
            self.close_hour();
        }
        self.hour.at = hour;

        // Each sample stands for the time since the previous one.
        let seconds = match self.last_sample_ms {
            Some(previous) if at_ms > previous => {
                ((at_ms - previous) as f64 / 1000.0).min(MAX_SAMPLE_GAP_SECONDS)
            }
            Some(_) => 0.0,
            None => 1.0,
        };
        self.last_sample_ms = Some(at_ms);
        if seconds <= 0.0 {
            return;
        }
        self.hour.seconds += seconds;
        // sysinfo reports a process's CPU per core, summed: 100 is one core, not the machine.
        let cores = snapshot.machine.logical_cores.max(1) as f64;
        for app in &snapshot.apps {
            if app.cpu_percent <= 0.0 && app.memory_bytes == 0 {
                continue;
            }
            let entry = match self.hour.apps.get_mut(&app.key) {
                Some(entry) => entry,
                None => self.hour.apps.entry(app.key.clone()).or_insert_with(|| AppSeconds {
                    name: app.name.clone(),
                    ..Default::default()
                }),
            };
            if entry.name != app.name {
                entry.name = app.name.clone();
            }
            if app.cpu_percent.is_finite() {
                entry.cpu += (app.cpu_percent / cores).clamp(0.0, 100.0) * seconds;
            }
            entry.memory += app.memory_bytes as f64 * seconds;
        }
    }

    fn close_hour(&mut self) {
        let hour = std::mem::take(&mut self.hour);
        if let Some(stored) = summarize(&hour) {
            merge_hour(&mut self.store.app_hours, stored);
            self.dirty = true;
        }
    }

    /// `metric` aggregated for a chart covering `[now − range, now]`.
    ///
    /// Stored points are combined into buckets of `bucket_seconds` (60 for up to an hour, 300
    /// up to 12 hours, 600 up to a day, 3600 up to a week, 14400 beyond): `value` is the mean
    /// weighted by sample count, `secondary` the highest sample. Buckets are aligned to the
    /// epoch and start inside the window; a bucket with no data is left out, so a gap in the
    /// result is a time nothing was recorded, not a zero.
    pub fn series(&self, metric: &str, range_seconds: u64, now_seconds: u64) -> HistorySeries {
        let step = series_bucket(range_seconds);
        let from = now_seconds.saturating_sub(range_seconds);
        let first = from.div_ceil(step).saturating_mul(step);
        let start = self.store.points.partition_point(|p| p.at < first);

        let mut points: Vec<HistoryPoint> = Vec::new();
        // (bucket, Σ value × weight, Σ weight, max)
        let mut open: Option<(u64, f64, f64, f64)> = None;
        for point in &self.store.points[start..] {
            if point.at > now_seconds {
                break;
            }
            let Some(&value) = point.values.get(metric) else {
                continue;
            };
            if !value.is_finite() {
                continue;
            }
            let peak = point
                .max
                .get(metric)
                .copied()
                .filter(|peak| peak.is_finite())
                .unwrap_or(value)
                .max(value);
            let weight = point.n.max(1) as f64;
            let bucket = point.at - point.at % step;
            match open.as_mut() {
                Some(current) if current.0 == bucket => {
                    current.1 += value * weight;
                    current.2 += weight;
                    current.3 = current.3.max(peak);
                }
                _ => {
                    if let Some(done) = open.take() {
                        points.push(history_point(done));
                    }
                    open = Some((bucket, value * weight, weight, peak));
                }
            }
        }
        if let Some(done) = open {
            points.push(history_point(done));
        }
        HistorySeries {
            metric: metric.to_string(),
            bucket_seconds: step,
            range_seconds,
            points,
        }
    }

    /// Two rows per hour, oldest first: "memory" (mean bytes, top 40) and "cpu" (mean percent
    /// of the whole machine, top 10). The hour in progress is included with what it has so far.
    pub fn ranking(&self, hours: u64, now_seconds: u64) -> Vec<RankingRow> {
        let from = now_seconds.saturating_sub(hours.saturating_mul(3600));
        let mut stored: Vec<StoredHour> = self.store.app_hours.clone();
        if let Some(live) = summarize(&self.hour) {
            merge_hour(&mut stored, live);
        }
        let mut rows = Vec::with_capacity(stored.len() * 2);
        for hour in stored.into_iter().filter(|h| h.at >= from && h.at <= now_seconds) {
            rows.push(RankingRow {
                hour_start_ms: hour.at * 1000,
                metric: "memory".to_string(),
                apps: hour.apps,
            });
            rows.push(RankingRow {
                hour_start_ms: hour.at * 1000,
                metric: "cpu".to_string(),
                apps: hour.cpu,
            });
        }
        rows
    }

    /// Hands the store to the background writer. Serialising is the only cost paid here.
    fn save(&mut self, now: u64) {
        self.last_save = now;
        if !self.dirty {
            return;
        }
        match serde_json::to_vec(&self.store) {
            Ok(bytes) => {
                self.generation += 1;
                self.dirty = false;
                save_in_background(&self.disk, self.generation, bytes);
            }
            Err(error) => crate::diag::error(format!("history: could not serialise: {error}")),
        }
    }

    /// Waits until the background writer has nothing left to do.
    #[cfg(test)]
    fn settle(&self) {
        for _ in 0..500 {
            if !lock(&self.disk.queue).running {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the history writer never finished");
    }
}

/// True when `every` seconds have passed since `last`. A clock that went backwards starts the
/// interval over instead of stalling it until the clock catches up.
fn interval_elapsed(last: &mut u64, now: u64, every: u64) -> bool {
    if now < *last {
        *last = now;
    }
    now - *last >= every
}

fn series_bucket(range_seconds: u64) -> u64 {
    if range_seconds <= 3600 {
        60
    } else if range_seconds <= 12 * 3600 {
        300
    } else if range_seconds <= 24 * 3600 {
        600
    } else if range_seconds <= 7 * 24 * 3600 {
        3600
    } else {
        14_400
    }
}

fn history_point((bucket, weighted, weight, peak): (u64, f64, f64, f64)) -> HistoryPoint {
    HistoryPoint {
        at_ms: bucket * 1000,
        value: weighted / weight,
        secondary: Some(peak),
    }
}

/// The weighted merge every tier uses: each metric's mean is weighted by the samples behind
/// it and divided by the samples of the points that actually carry that metric — a metric
/// missing from part of the group is averaged over where it exists, not diluted toward zero.
fn combine(at: u64, group: &[StoredPoint]) -> StoredPoint {
    let mut sums: BTreeMap<&str, (f64, f64, f64)> = BTreeMap::new();
    let mut samples: u64 = 0;
    for point in group {
        let weight = point.n.max(1);
        samples += weight as u64;
        for (metric, value) in &point.values {
            if !value.is_finite() {
                continue;
            }
            let peak = point
                .max
                .get(metric)
                .copied()
                .filter(|peak| peak.is_finite())
                .unwrap_or(*value)
                .max(*value);
            let entry = sums.entry(metric.as_str()).or_insert((0.0, 0.0, peak));
            entry.0 += value * weight as f64;
            entry.1 += weight as f64;
            entry.2 = entry.2.max(peak);
        }
    }
    let mut values = BTreeMap::new();
    let mut max = BTreeMap::new();
    for (metric, (weighted, weight, peak)) in sums {
        values.insert(metric.to_string(), weighted / weight);
        max.insert(metric.to_string(), peak);
    }
    StoredPoint {
        at,
        values,
        max,
        n: samples.min(u32::MAX as u64) as u32,
    }
}

/// Inserts `point` in order, merging it into a point already stored for the same bucket.
fn merge_point(points: &mut Vec<StoredPoint>, point: StoredPoint) {
    match points.binary_search_by_key(&point.at, |p| p.at) {
        Ok(index) => {
            let existing = std::mem::replace(&mut points[index], combine(point.at, &[]));
            points[index] = combine(point.at, &[existing, point]);
        }
        Err(index) => points.insert(index, point),
    }
}

/// Ascending, one point per bucket.
fn normalize(mut points: Vec<StoredPoint>) -> Vec<StoredPoint> {
    points.sort_by_key(|p| p.at);
    let mut out: Vec<StoredPoint> = Vec::with_capacity(points.len());
    for point in points {
        match out.last_mut() {
            Some(last) if last.at == point.at => {
                let earlier = std::mem::replace(last, combine(point.at, &[]));
                *last = combine(point.at, &[earlier, point]);
            }
            _ => out.push(point),
        }
    }
    out
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
    // A bucket folded earlier sits in its group next to the minutes that aged into it since,
    // and the weights make the second fold exact rather than a pull toward the newest minute.
    let mut folded: Vec<StoredPoint> = groups
        .into_iter()
        .map(|(at, group)| combine(at, &group))
        .collect();
    folded.extend(recent);
    normalize(folded)
}

/// What the collected hour ranks as, or None when nothing was observed.
fn summarize(hour: &Hour) -> Option<StoredHour> {
    if hour.seconds <= 0.0 {
        return None;
    }
    let ranked = |value: fn(&AppSeconds) -> f64, keep: usize| -> Vec<RankedApp> {
        let mut apps: Vec<RankedApp> = hour
            .apps
            .iter()
            .map(|(key, app)| RankedApp {
                key: key.clone(),
                name: app.name.clone(),
                value: value(app) / hour.seconds,
            })
            .filter(|app| app.value > 0.0)
            .collect();
        apps.sort_by(|a, b| b.value.total_cmp(&a.value).then_with(|| a.key.cmp(&b.key)));
        apps.truncate(keep);
        apps
    };
    Some(StoredHour {
        at: hour.at,
        apps: ranked(|app| app.memory, MEMORY_RANKED),
        cpu: ranked(|app| app.cpu, CPU_RANKED),
        seconds: hour.seconds,
    })
}

/// Stores `hour`, merging it with an entry for the same hour weighted by observed seconds — a
/// restart inside an hour must not erase the half recorded before it.
fn merge_hour(hours: &mut Vec<StoredHour>, hour: StoredHour) {
    let index = match hours.binary_search_by_key(&hour.at, |h| h.at) {
        Ok(index) => index,
        Err(index) => {
            hours.insert(index, hour);
            return;
        }
    };
    let earlier = &hours[index];
    let (a, b) = (earlier.seconds.max(0.0), hour.seconds.max(0.0));
    if a + b <= 0.0 {
        hours[index] = hour;
        return;
    }
    let blend = |old: &[RankedApp], new: &[RankedApp], keep: usize| -> Vec<RankedApp> {
        let mut merged: HashMap<&str, (String, f64)> = HashMap::new();
        for app in old {
            merged.insert(app.key.as_str(), (app.name.clone(), app.value * a));
        }
        for app in new {
            let entry = merged.entry(app.key.as_str()).or_insert((app.name.clone(), 0.0));
            entry.0 = app.name.clone();
            entry.1 += app.value * b;
        }
        let mut apps: Vec<RankedApp> = merged
            .into_iter()
            .map(|(key, (name, weighted))| RankedApp {
                key: key.to_string(),
                name,
                value: weighted / (a + b),
            })
            .filter(|app| app.value > 0.0)
            .collect();
        apps.sort_by(|x, y| y.value.total_cmp(&x.value).then_with(|| x.key.cmp(&y.key)));
        apps.truncate(keep);
        apps
    };
    let merged = StoredHour {
        at: hour.at,
        apps: blend(&earlier.apps, &hour.apps, MEMORY_RANKED),
        cpu: blend(&earlier.cpu, &hour.cpu, CPU_RANKED),
        seconds: a + b,
    };
    hours[index] = merged;
}

/// The figure stored for `metric`, or None when it was not measured — a GPU without engine
/// counters, a desktop's battery, a machine with no temperature sensor. Never a zero for those.
fn metric_value(snapshot: &Snapshot, metric: &str) -> Option<f64> {
    let value = match metric {
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
        "gpu" => snapshot.gpu.total_percent?,
        "battery" if snapshot.battery.has_battery => snapshot.battery.charge_percent?,
        "temp" => snapshot.temperature.hottest?,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

// ---------------------------------------------------------------------------
// the file
// ---------------------------------------------------------------------------

/// `history.json` and the single writer thread that may be touching it.
struct Disk {
    path: PathBuf,
    queue: Mutex<Queue>,
    /// Generation last written; held for the whole write so two writers never interleave.
    written: Mutex<u64>,
}

#[derive(Default)]
struct Queue {
    pending: Option<(u64, Vec<u8>)>,
    running: bool,
}

impl Disk {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            queue: Mutex::new(Queue::default()),
            written: Mutex::new(0),
        }
    }

    /// Writes atomically (temp file, then rename). A generation older than the one already on
    /// disk is dropped, so a slow background write can never undo a newer flush.
    fn write(&self, generation: u64, bytes: &[u8]) {
        let mut written = self.written.lock().unwrap_or_else(|p| p.into_inner());
        if generation <= *written {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        match fs::write(&tmp, bytes).and_then(|_| fs::rename(&tmp, &self.path)) {
            Ok(()) => *written = generation,
            Err(error) => crate::diag::error(format!("history: could not write {}: {error}", self.path.display())),
        }
    }
}

fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    queue.lock().unwrap_or_else(|p| p.into_inner())
}

/// Queues `bytes` and makes sure one writer thread is on it. Only the newest queued
/// generation is ever written; a thread exits as soon as the queue is empty.
fn save_in_background(disk: &Arc<Disk>, generation: u64, bytes: Vec<u8>) {
    let mut queue = lock(&disk.queue);
    queue.pending = Some((generation, bytes));
    if queue.running {
        return;
    }
    queue.running = true;
    drop(queue);

    let worker = Arc::clone(disk);
    let spawned = std::thread::Builder::new()
        .name("pulse-history-save".into())
        .spawn(move || loop {
            let job = {
                let mut queue = lock(&worker.queue);
                match queue.pending.take() {
                    Some(job) => job,
                    None => {
                        queue.running = false;
                        return;
                    }
                }
            };
            worker.write(job.0, &job.1);
        });
    if let Err(error) = spawned {
        crate::diag::warn(format!("history: no writer thread ({error}); writing inline"));
        let job = {
            let mut queue = lock(&disk.queue);
            queue.running = false;
            queue.pending.take()
        };
        if let Some((generation, bytes)) = job {
            disk.write(generation, &bytes);
        }
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
    use crate::models::{AppInfo, AppKind, CpuMetric, MachineInfo};

    fn point(at: u64, values: &[(&str, f64)]) -> StoredPoint {
        StoredPoint {
            at,
            values: values.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            max: BTreeMap::new(),
            n: 1,
        }
    }

    fn weighted(at: u64, n: u32, values: &[(&str, f64)], max: &[(&str, f64)]) -> StoredPoint {
        StoredPoint {
            n,
            max: max.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            ..point(at, values)
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pulse-history-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir to test against");
        dir
    }

    /// An hour boundary, so minutes and hours in these tests start where they look like they do.
    const HOUR0: u64 = 1_999_998_000;

    fn snapshot_at(second: u64, cpu: f64) -> Snapshot {
        Snapshot {
            taken_at_ms: second * 1000,
            machine: MachineInfo {
                hostname: "test-host".into(),
                os_version: "test".into(),
                chip_name: "test".into(),
                physical_cores: Some(2),
                logical_cores: 2,
                total_memory_bytes: 8 << 30,
                uptime_seconds: 0,
            },
            cpu: CpuMetric { total_percent: cpu, ..Default::default() },
            memory: Default::default(),
            disk: Default::default(),
            network: Default::default(),
            gpu: Default::default(),
            battery: Default::default(),
            temperature: Default::default(),
            apps: Vec::new(),
            process_count: 0,
            sampler_cost_microseconds: 0,
        }
    }

    fn app(key: &str, cpu_percent: f64, memory_bytes: u64) -> AppInfo {
        AppInfo {
            key: key.into(),
            name: key.into(),
            exe_path: format!("C:\\test\\{key}.exe"),
            kind: AppKind::App,
            main_pid: 4242,
            process_count: 1,
            cpu_percent,
            memory_bytes,
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
        }
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
        assert_eq!(out[0].max["cpu"], 30.0, "the busiest minute survives as the max");
        assert_eq!(out[0].n, 3);
    }

    #[test]
    fn a_metric_missing_from_part_of_a_bucket_is_averaged_where_it_exists() {
        // Changed on purpose: the divisor used to be the size of the bucket, so a GPU that was
        // only measured for half of it charted at half its real load. Each metric is now divided
        // by the samples that actually carry it.
        let out = fold_older(
            vec![point(1_000, &[("cpu", 10.0)]), point(1_060, &[("gpu", 50.0)])],
            5_000,
            300,
        );
        assert_eq!(out[0].values["cpu"], 10.0);
        assert_eq!(out[0].values["gpu"], 50.0);
        assert_eq!(out[0].n, 2);
    }

    #[test]
    fn folding_weighs_every_point_by_the_samples_behind_it() {
        // A five-minute bucket folded earlier (four minutes, 240 samples) meets the fifth minute
        // when it ages out. Averaging the two as equals would read (10 + 60) / 2 = 35.
        let out = fold_older(
            vec![
                weighted(900, 240, &[("cpu", 10.0)], &[("cpu", 15.0)]),
                weighted(1_140, 60, &[("cpu", 60.0)], &[("cpu", 95.0)]),
            ],
            5_000,
            300,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].values["cpu"], 20.0, "(10 × 240 + 60 × 60) / 300");
        assert_eq!(out[0].max["cpu"], 95.0);
        assert_eq!(out[0].n, 300);
        // And folding the result again changes nothing.
        let again = fold_older(out.clone(), 5_000, 300);
        assert_eq!(again[0].values["cpu"], 20.0);
        assert_eq!(again[0].n, 300);
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
        // `flush` rather than the old synchronous `save`: saving now happens on a background
        // thread, and `flush` is the write that has finished when it returns.
        let now = now_seconds();
        let dir = temp_dir("roundtrip");
        {
            let mut h = History::open(&dir);
            h.store.points = vec![point(now - 180, &[("cpu", 7.5)]), point(now - 120, &[("cpu", 8.5)])];
            h.flush();
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
            app_hours: vec![],
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
        // Changed on purpose: the query used to keep only points whose start was a multiple of
        // the bucket width, so a day's chart showed one minute in five and dropped the rest
        // along with their spikes. Every point inside the window is now aggregated into its
        // bucket instead, and buckets start inside the window.
        let now = HOUR0 + 7_200;
        let dir = temp_dir("series");
        let mut h = History::open_at(&dir, now);
        h.store.points = vec![
            point(now - 7_200, &[("cpu", 4.0)]),
            point(now - 1_830, &[("cpu", 5.0)]), // a restart mid-minute: still real data
            point(now - 1_800, &[("cpu", 3.0)]),
            point(now - 600, &[("cpu", 2.0)]),
            point(now - 300, &[("cpu", 1.0)]),
        ];

        let hour = h.series("cpu", 3_600, now);
        assert_eq!(hour.bucket_seconds, 60, "an hour of history is minute detail");
        let seen: Vec<u64> = hour.points.iter().map(|p| p.at_ms / 1000).collect();
        assert_eq!(seen, vec![now - 1_860, now - 1_800, now - 600, now - 300], "ascending, inside the window");

        let day = h.series("cpu", 86_400, now);
        assert_eq!(day.bucket_seconds, 600, "a day of history is ten-minute detail");
        let seen: Vec<(u64, f64)> = day.points.iter().map(|p| (p.at_ms / 1000, p.value)).collect();
        assert_eq!(
            seen,
            vec![(now - 7_200, 4.0), (now - 2_400, 5.0), (now - 1_800, 3.0), (now - 600, 1.5)],
            "the window widens to reach the two-hour-old point, and neighbours share a bucket"
        );
        assert_eq!(day.points[3].secondary, Some(2.0));

        assert!(h.series("nope", 3_600, now).points.is_empty(), "a metric nobody stored is empty, not an error");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bucket_widths_follow_the_range() {
        assert_eq!(series_bucket(3_600), 60);
        assert_eq!(series_bucket(12 * 3_600), 300);
        assert_eq!(series_bucket(24 * 3_600), 600);
        assert_eq!(series_bucket(7 * 86_400), 3_600);
        assert_eq!(series_bucket(30 * 86_400), 14_400);
    }

    #[test]
    fn a_series_keeps_its_gaps_and_its_peaks() {
        let now = HOUR0 + 3_600;
        let dir = temp_dir("gaps");
        let mut h = History::open_at(&dir, now);
        h.store.points = vec![
            weighted(HOUR0 + 600, 60, &[("cpu", 10.0)], &[("cpu", 30.0)]),
            weighted(HOUR0 + 660, 60, &[("cpu", 20.0)], &[("cpu", 25.0)]),
            // Ten minutes in which Pulse was not running.
            weighted(HOUR0 + 1_320, 30, &[("cpu", 40.0)], &[("cpu", 90.0)]),
        ];

        let hour = h.series("cpu", 3_600, now);
        let seen: Vec<(u64, f64, Option<f64>)> =
            hour.points.iter().map(|p| (p.at_ms / 1000, p.value, p.secondary)).collect();
        assert_eq!(
            seen,
            vec![
                (HOUR0 + 600, 10.0, Some(30.0)),
                (HOUR0 + 660, 20.0, Some(25.0)),
                (HOUR0 + 1_320, 40.0, Some(90.0)),
            ],
            "no zeros are invented for the minutes nobody recorded"
        );

        let half_day = h.series("cpu", 12 * 3_600, now);
        let seen: Vec<(u64, f64, Option<f64>)> =
            half_day.points.iter().map(|p| (p.at_ms / 1000, p.value, p.secondary)).collect();
        assert_eq!(seen, vec![(HOUR0 + 600, 15.0, Some(30.0)), (HOUR0 + 1_200, 40.0, Some(90.0))]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_minute_is_stored_once_when_the_next_one_begins() {
        let dir = temp_dir("minute");
        let mut h = History::open_at(&dir, HOUR0);
        h.record(&snapshot_at(HOUR0 + 5, 10.0));
        h.record(&snapshot_at(HOUR0 + 25, 30.0));
        h.record(&snapshot_at(HOUR0 + 59, 20.0));
        assert!(h.store.points.is_empty(), "the minute is still being collected");
        h.record(&snapshot_at(HOUR0 + 61, 50.0));
        assert_eq!(h.store.points.len(), 1);
        let first = &h.store.points[0];
        assert_eq!((first.at, first.n), (HOUR0, 3));
        assert_eq!(first.values["cpu"], 20.0);
        assert_eq!(first.max["cpu"], 30.0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unmeasured_metrics_are_left_out_not_zeroed() {
        let dir = temp_dir("absent");
        let mut h = History::open_at(&dir, HOUR0);
        // A desktop with no GPU counters and no temperature sensor; a stray charge figure
        // without a battery is not a battery either.
        let mut desktop = snapshot_at(HOUR0 + 1, 10.0);
        desktop.battery.charge_percent = Some(100.0);
        h.record(&desktop);
        // A laptop that measures everything.
        let mut laptop = snapshot_at(HOUR0 + 61, 10.0);
        laptop.battery.has_battery = true;
        laptop.battery.charge_percent = Some(80.0);
        laptop.gpu.total_percent = Some(12.5);
        laptop.temperature.hottest = Some(55.0);
        h.record(&laptop);
        h.flush();

        let desktop_point = &h.store.points[0];
        assert!(desktop_point.values.contains_key("cpu"));
        for absent in ["gpu", "battery", "temp"] {
            assert!(!desktop_point.values.contains_key(absent), "{absent} was stored as a number");
        }
        let laptop_point = &h.store.points[1];
        assert_eq!(laptop_point.values["battery"], 80.0);
        assert_eq!(laptop_point.values["gpu"], 12.5);
        assert_eq!(laptop_point.values["temp"], 55.0);

        let battery = h.series("battery", 3_600, HOUR0 + 120);
        assert_eq!(battery.points.len(), 1, "the minute without a battery is a gap");
        assert_eq!(battery.points[0].at_ms, (HOUR0 + 60) * 1000);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_from_before_weights_and_maxima_still_opens() {
        let dir = temp_dir("legacy");
        let legacy = format!(
            r#"{{"points":[{{"at":{at},"values":{{"cpu":5.0}}}}],
                "app_hours":[{{"at":{hour},"apps":[{{"key":"c:\\chrome.exe","name":"Chrome","value":1000.0}}]}}]}}"#,
            at = HOUR0 + 60,
            hour = HOUR0
        );
        fs::write(dir.join("history.json"), legacy).unwrap();

        let h = History::open_at(&dir, HOUR0 + 600);
        assert_eq!(h.store.points.len(), 1);
        assert_eq!(h.store.points[0].n, 1);
        assert!(h.store.points[0].max.is_empty());
        let series = h.series("cpu", 3_600, HOUR0 + 600);
        assert_eq!(series.points[0].secondary, Some(5.0), "the mean stands in for a max it never had");

        let rows = h.ranking(48, HOUR0 + 600);
        assert_eq!(rows.len(), 2, "both rows for the hour, the CPU one simply empty");
        assert_eq!(rows[0].metric, "memory");
        assert_eq!(rows[0].apps[0].name, "Chrome");
        assert_eq!(rows[1].metric, "cpu");
        assert!(rows[1].apps.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn flush_writes_what_has_not_been_saved() {
        let dir = temp_dir("flush");
        let mut h = History::open_at(&dir, HOUR0);
        let mut busy = snapshot_at(HOUR0 + 10, 42.0);
        busy.apps = vec![app("chrome", 100.0, 1_000_000)];
        h.record(&busy);
        h.settle();
        assert!(!dir.join("history.json").exists(), "nothing is due yet: the minute is still open");

        h.flush();
        assert!(dir.join("history.json").exists(), "flush has written by the time it returns");
        let back = History::open_at(&dir, HOUR0 + 30);
        assert_eq!(back.store.points.len(), 1);
        assert_eq!(back.store.points[0].values["cpu"], 42.0);
        let rows = back.ranking(48, HOUR0 + 30);
        assert_eq!(rows[1].apps[0].key, "chrome", "the unfinished hour was stored too");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_restart_inside_a_minute_merges_instead_of_duplicating() {
        let dir = temp_dir("restart");
        {
            let mut h = History::open_at(&dir, HOUR0);
            h.record(&snapshot_at(HOUR0 + 10, 10.0));
            h.record(&snapshot_at(HOUR0 + 20, 20.0));
            h.flush();
        }
        let mut h = History::open_at(&dir, HOUR0 + 30);
        h.record(&snapshot_at(HOUR0 + 40, 60.0));
        h.flush();
        assert_eq!(h.store.points.len(), 1, "one point per minute, however many runs fed it");
        let only = &h.store.points[0];
        assert_eq!(only.n, 3);
        assert_eq!(only.values["cpu"], 30.0);
        assert_eq!(only.max["cpu"], 60.0);
        assert_eq!(h.store.app_hours.len(), 1, "and one entry for the hour");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_follows_the_wall_clock_and_happens_off_the_callers_thread() {
        let dir = temp_dir("cadence");
        let mut h = History::open_at(&dir, HOUR0);
        for tick in 0..=24 {
            h.record(&snapshot_at(HOUR0 + tick * 10, 5.0));
        }
        h.settle();
        assert!(!dir.join("history.json").exists(), "four minutes in, nothing is written yet");

        for tick in 25..=31 {
            h.record(&snapshot_at(HOUR0 + tick * 10, 5.0));
        }
        h.settle();
        assert!(dir.join("history.json").exists(), "five minutes in, the background writer saved");
        let back = History::open_at(&dir, HOUR0 + 310);
        assert_eq!(back.store.points.len(), 5, "the five finished minutes");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_older_write_never_replaces_a_newer_one() {
        let dir = temp_dir("generation");
        let disk = Disk::new(dir.join("history.json"));
        disk.write(2, b"newer");
        disk.write(1, b"older");
        assert_eq!(fs::read_to_string(dir.join("history.json")).unwrap(), "newer");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_clock_that_goes_back_restarts_the_interval() {
        let mut last = 1_000;
        assert!(!interval_elapsed(&mut last, 1_200, 300));
        assert!(interval_elapsed(&mut last, 1_300, 300));
        let mut last = 1_000;
        assert!(!interval_elapsed(&mut last, 400, 300));
        assert_eq!(last, 400, "measured from the new reading");
        assert!(interval_elapsed(&mut last, 700, 300));
    }

    #[test]
    fn the_ranking_has_a_memory_row_and_a_cpu_row_per_hour() {
        let dir = temp_dir("ranking");
        let mut h = History::open_at(&dir, HOUR0);
        // Four logical cores: sysinfo's 200 is two cores, half the machine.
        let tick = |second: u64, apps: Vec<AppInfo>| {
            let mut s = snapshot_at(second, 0.0);
            s.machine.logical_cores = 4;
            s.apps = apps;
            s
        };
        h.record(&tick(HOUR0, vec![app("chrome", 200.0, 2_000_000_000)])); // counts 1 s
        h.record(&tick(HOUR0 + 1, vec![app("chrome", 200.0, 2_000_000_000)])); // 1 s
        h.record(&tick(HOUR0 + 3, vec![app("cargo", 400.0, 1_000_000_000)])); // 2 s
        h.record(&tick(HOUR0 + 3_605, vec![app("cargo", 9_000.0, 1)])); // next hour; gap capped

        let rows = h.ranking(48, HOUR0 + 3_610);
        let shape: Vec<(u64, &str)> = rows.iter().map(|r| (r.hour_start_ms / 1000, r.metric.as_str())).collect();
        assert_eq!(
            shape,
            vec![(HOUR0, "memory"), (HOUR0, "cpu"), (HOUR0 + 3_600, "memory"), (HOUR0 + 3_600, "cpu")],
            "two rows per hour, oldest first, the hour in progress included"
        );
        let values = |row: &RankingRow| -> Vec<(String, f64)> {
            row.apps.iter().map(|a| (a.key.clone(), a.value)).collect()
        };
        // Averaged over the four observed seconds, not sampled at the top of the hour.
        assert_eq!(values(&rows[0]), vec![("chrome".into(), 1e9), ("cargo".into(), 5e8)]);
        assert_eq!(values(&rows[1]), vec![("cargo".into(), 50.0), ("chrome".into(), 25.0)]);
        assert_eq!(values(&rows[3]), vec![("cargo".into(), 100.0)], "a share of the machine never passes 100");
        let _ = fs::remove_dir_all(dir);
    }
}
