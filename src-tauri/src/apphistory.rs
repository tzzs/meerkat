//! What every app used, minute by minute, for 30 days.
//!
//! The machine-wide history says the CPU was busy at 3 a.m.; this is what says who. Each tick
//! adds to a per-app accumulator, each minute those become one row per active app, and rows
//! older than a day are folded into ten-minute rows so a month fits in a few megabytes. Every
//! stored quantity is additive (CPU-seconds, bytes, seconds observed) or carries its weight
//! (average memory with the seconds behind it), so folding and ranking are sums, never an
//! average of averages.
//!
//! SQLite rather than the JSON file the machine history uses: the per-app rows run to a few
//! hundred thousand, and the questions asked of them ("heaviest apps this week", "what was
//! running at 21:14") are queries.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::models::Snapshot;

const MINUTE: u64 = 60;
const FOLDED: u64 = 600;
const FINE_KEEP: u64 = 24 * 3600;
const TOTAL_KEEP: u64 = 30 * 24 * 3600;
/// Apps below every one of these in a minute are idle and not worth a row.
const MIN_CPU_SECONDS: f64 = 0.05;
const MIN_BYTES: f64 = 64.0 * 1024.0;
/// The heaviest apps by memory are kept every minute even when idle, so "what was taking the
/// memory" has an answer at any moment.
const KEEP_BY_MEMORY: usize = 15;

#[derive(Default, Clone)]
struct Accumulator {
    name: String,
    exe: String,
    kind: String,
    seconds: f64,
    cpu_seconds: f64,
    memory_byte_seconds: f64,
    memory_max: f64,
    read: f64,
    write: f64,
    gpu_seconds: f64,
}

/// One app over a span of time.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppUsage {
    pub key: String,
    pub name: String,
    pub exe_path: String,
    pub kind: String,
    /// How long Meerkat saw it running in the span.
    pub seconds: f64,
    /// CPU time it used, in seconds of one core.
    pub cpu_seconds: f64,
    pub memory_average: f64,
    pub memory_peak: f64,
    pub read_bytes: f64,
    pub write_bytes: f64,
    /// GPU time, in seconds of one whole GPU.
    pub gpu_seconds: f64,
}

/// One app's figures in one bucket of its own history.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppPoint {
    pub at_ms: u64,
    pub span_seconds: u64,
    /// Average over the bucket, in percent of one core.
    pub cpu_percent: f64,
    pub memory_bytes: f64,
    pub read_bytes: f64,
    pub write_bytes: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RankBy {
    Cpu,
    Memory,
    Disk,
    Write,
    Gpu,
}

pub struct AppHistory {
    db: Connection,
    current_minute: u64,
    pending: HashMap<String, Accumulator>,
    last_fold: u64,
}

impl AppHistory {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("apps.db");
        let db = Connection::open(&path)
            .or_else(|error| {
                // A database that cannot be opened (a disk full when it was last written, a
                // file from a future version) is moved aside rather than blocking the app.
                eprintln!("apps.db: {error}; starting a new one");
                let _ = std::fs::rename(&path, path.with_extension("db.bad"));
                Connection::open(&path)
            })
            .or_else(|_| Connection::open_in_memory())
            .expect("an in-memory database always opens");
        Self::with_connection(db)
    }

    fn with_connection(db: Connection) -> Self {
        let _ = db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS apps (
               id INTEGER PRIMARY KEY,
               key TEXT NOT NULL UNIQUE,
               name TEXT NOT NULL,
               exe TEXT NOT NULL,
               kind TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS usage (
               span INTEGER NOT NULL,
               bucket INTEGER NOT NULL,
               app INTEGER NOT NULL,
               seconds REAL NOT NULL,
               cpu_s REAL NOT NULL,
               mem_avg REAL NOT NULL,
               mem_max REAL NOT NULL,
               read REAL NOT NULL,
               write REAL NOT NULL,
               gpu_s REAL NOT NULL,
               PRIMARY KEY (span, bucket, app)
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS usage_by_app ON usage (app, bucket);",
        );
        Self { db, current_minute: 0, pending: HashMap::new(), last_fold: 0 }
    }

    /// Adds one sample, `elapsed` seconds after the previous one, and writes the minute out once
    /// the clock has moved past it.
    pub fn record(&mut self, snapshot: &Snapshot, elapsed: f64) {
        let second = snapshot.taken_at_ms / 1000;
        let minute = second - second % MINUTE;
        if self.current_minute != 0 && minute != self.current_minute {
            self.flush();
        }
        self.current_minute = minute;
        let elapsed = elapsed.clamp(0.0, 10.0);

        let gpu_by_pid: HashMap<u32, f64> = snapshot.gpu.per_process.iter().map(|p| (p.pid, p.percent)).collect();
        for app in &snapshot.apps {
            if app.key == crate::metrics::grouping::UNREADABLE_KEY {
                continue;
            }
            let gpu: f64 = app.pids.iter().filter_map(|pid| gpu_by_pid.get(pid)).sum();
            let entry = self.pending.entry(app.key.clone()).or_default();
            entry.name.clone_from(&app.name);
            entry.exe.clone_from(&app.exe_path);
            entry.kind = format!("{:?}", app.kind).to_lowercase();
            entry.seconds += elapsed;
            entry.cpu_seconds += app.cpu_percent / 100.0 * elapsed;
            entry.memory_byte_seconds += app.memory_bytes as f64 * elapsed;
            entry.memory_max = entry.memory_max.max(app.memory_bytes as f64);
            entry.read += app.read_bytes_per_second * elapsed;
            entry.write += app.write_bytes_per_second * elapsed;
            entry.gpu_seconds += gpu.min(100.0) / 100.0 * elapsed;
        }

        if second.saturating_sub(self.last_fold) >= FOLDED {
            self.fold(second);
        }
    }

    /// Writes the pending minute: every app that did something, plus the heaviest by memory.
    pub fn flush(&mut self) {
        let minute = self.current_minute;
        let pending = std::mem::take(&mut self.pending);
        if minute == 0 || pending.is_empty() {
            return;
        }
        let mut by_memory: Vec<(&String, f64)> = pending
            .iter()
            .map(|(key, a)| (key, if a.seconds > 0.0 { a.memory_byte_seconds / a.seconds } else { 0.0 }))
            .collect();
        by_memory.sort_by(|a, b| b.1.total_cmp(&a.1));
        let heavy: Vec<&String> = by_memory.iter().take(KEEP_BY_MEMORY).map(|(key, _)| *key).collect();

        let Ok(tx) = self.db.transaction() else { return };
        for (key, a) in &pending {
            let active = a.cpu_seconds >= MIN_CPU_SECONDS || a.read + a.write >= MIN_BYTES || a.gpu_seconds > 0.0;
            if (!active && !heavy.contains(&key)) || a.seconds <= 0.0 {
                continue;
            }
            let Some(id) = upsert_app(&tx, key, a) else { continue };
            let _ = tx.execute(
                "INSERT INTO usage (span, bucket, app, seconds, cpu_s, mem_avg, mem_max, read, write, gpu_s)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (span, bucket, app) DO UPDATE SET
                   mem_avg = (mem_avg * seconds + excluded.mem_avg * excluded.seconds) / (seconds + excluded.seconds),
                   seconds = seconds + excluded.seconds,
                   cpu_s = cpu_s + excluded.cpu_s,
                   mem_max = max(mem_max, excluded.mem_max),
                   read = read + excluded.read,
                   write = write + excluded.write,
                   gpu_s = gpu_s + excluded.gpu_s",
                params![
                    MINUTE as i64,
                    minute as i64,
                    id,
                    a.seconds,
                    a.cpu_seconds,
                    a.memory_byte_seconds / a.seconds,
                    a.memory_max,
                    a.read,
                    a.write,
                    a.gpu_seconds
                ],
            );
        }
        let _ = tx.commit();
    }

    /// Minute rows older than a day become ten-minute rows; anything older than 30 days goes.
    /// Only whole ten-minute buckets are folded, so a bucket is never half fine and half coarse.
    pub fn fold(&mut self, now: u64) {
        self.last_fold = now;
        let horizon = now.saturating_sub(FINE_KEEP);
        let cutoff = (horizon - horizon % FOLDED) as i64;
        let Ok(tx) = self.db.transaction() else { return };
        let _ = tx.execute(
            "INSERT INTO usage (span, bucket, app, seconds, cpu_s, mem_avg, mem_max, read, write, gpu_s)
             SELECT ?1, bucket - bucket % ?1, app, SUM(seconds), SUM(cpu_s),
                    SUM(mem_avg * seconds) / SUM(seconds), MAX(mem_max), SUM(read), SUM(write), SUM(gpu_s)
             FROM usage WHERE span = ?2 AND bucket < ?3 AND seconds > 0
             GROUP BY bucket - bucket % ?1, app
             ON CONFLICT (span, bucket, app) DO UPDATE SET
               mem_avg = (mem_avg * seconds + excluded.mem_avg * excluded.seconds) / (seconds + excluded.seconds),
               seconds = seconds + excluded.seconds,
               cpu_s = cpu_s + excluded.cpu_s,
               mem_max = max(mem_max, excluded.mem_max),
               read = read + excluded.read,
               write = write + excluded.write,
               gpu_s = gpu_s + excluded.gpu_s",
            params![FOLDED as i64, MINUTE as i64, cutoff],
        );
        let _ = tx.execute("DELETE FROM usage WHERE span = ?1 AND bucket < ?2", params![MINUTE as i64, cutoff]);
        let oldest = now.saturating_sub(TOTAL_KEEP) as i64;
        let _ = tx.execute("DELETE FROM usage WHERE bucket < ?1", params![oldest]);
        let _ = tx.execute("DELETE FROM apps WHERE id NOT IN (SELECT DISTINCT app FROM usage)", []);
        let _ = tx.commit();
    }

    /// The heaviest apps between `from` and `to` (Unix seconds) by one measure.
    pub fn ranking(&self, from: u64, to: u64, by: RankBy, limit: usize) -> Vec<AppUsage> {
        let order = match by {
            RankBy::Cpu => "cpu_s",
            RankBy::Memory => "mem_avg",
            RankBy::Disk => "read + write",
            RankBy::Write => "write",
            RankBy::Gpu => "gpu_s",
        };
        let sql = format!(
            "SELECT a.key, a.name, a.exe, a.kind, seconds, cpu_s, mem_avg, mem_max, read, write, gpu_s FROM (
               SELECT app, SUM(seconds) AS seconds, SUM(cpu_s) AS cpu_s,
                      SUM(mem_avg * seconds) / SUM(seconds) AS mem_avg, MAX(mem_max) AS mem_max,
                      SUM(read) AS read, SUM(write) AS write, SUM(gpu_s) AS gpu_s
               FROM usage WHERE bucket >= ?1 AND bucket < ?2 AND seconds > 0 GROUP BY app
             ) JOIN apps a ON a.id = app
             ORDER BY {order} DESC LIMIT ?3"
        );
        self.query_usage(&sql, params![from as i64, to as i64, limit as i64])
    }

    /// What was running in the bucket that holds `at` (Unix seconds), busiest first.
    pub fn at(&self, at: u64, limit: usize) -> Vec<AppUsage> {
        // The finest bucket that still exists for that moment.
        let span: Option<i64> = self
            .db
            .query_row(
                "SELECT span FROM usage WHERE bucket <= ?1 AND bucket + span > ?1 ORDER BY span LIMIT 1",
                params![at as i64],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let Some(span) = span else { return Vec::new() };
        let bucket = at as i64 - at as i64 % span;
        let sql = "SELECT a.key, a.name, a.exe, a.kind, u.seconds, u.cpu_s, u.mem_avg, u.mem_max, u.read, u.write, u.gpu_s
                   FROM usage u JOIN apps a ON a.id = u.app
                   WHERE u.span = ?1 AND u.bucket = ?2
                   ORDER BY u.cpu_s DESC, u.mem_avg DESC LIMIT ?3";
        self.query_usage(sql, params![span, bucket, limit as i64])
    }

    /// One app's history between `from` and `to`, grouped into `step`-second points.
    pub fn series(&self, key: &str, from: u64, to: u64, step: u64) -> Vec<AppPoint> {
        let step = step.max(MINUTE) as i64;
        let Ok(mut statement) = self.db.prepare(
            "SELECT u.bucket - u.bucket % ?4 AS at, SUM(u.cpu_s), SUM(u.mem_avg * u.seconds) / SUM(u.seconds),
                    SUM(u.read), SUM(u.write)
             FROM usage u JOIN apps a ON a.id = u.app
             WHERE a.key = ?1 AND u.bucket >= ?2 AND u.bucket < ?3 AND u.seconds > 0
             GROUP BY at ORDER BY at",
        ) else {
            return Vec::new();
        };
        let rows = statement.query_map(params![key, from as i64, to as i64, step], |row| {
            let at: i64 = row.get(0)?;
            let cpu_seconds: f64 = row.get(1)?;
            Ok(AppPoint {
                at_ms: at.max(0) as u64 * 1000,
                span_seconds: step as u64,
                cpu_percent: cpu_seconds / step as f64 * 100.0,
                memory_bytes: row.get(2)?,
                read_bytes: row.get(3)?,
                write_bytes: row.get(4)?,
            })
        });
        rows.map(|mapped| mapped.filter_map(Result::ok).collect()).unwrap_or_default()
    }

    fn query_usage(&self, sql: &str, args: impl rusqlite::Params) -> Vec<AppUsage> {
        let Ok(mut statement) = self.db.prepare(sql) else { return Vec::new() };
        let rows = statement.query_map(args, |row| {
            Ok(AppUsage {
                key: row.get(0)?,
                name: row.get(1)?,
                exe_path: row.get(2)?,
                kind: row.get(3)?,
                seconds: row.get(4)?,
                cpu_seconds: row.get(5)?,
                memory_average: row.get(6)?,
                memory_peak: row.get(7)?,
                read_bytes: row.get(8)?,
                write_bytes: row.get(9)?,
                gpu_seconds: row.get(10)?,
            })
        });
        rows.map(|mapped| mapped.filter_map(Result::ok).collect()).unwrap_or_default()
    }
}

fn upsert_app(tx: &rusqlite::Transaction<'_>, key: &str, a: &Accumulator) -> Option<i64> {
    // The name can change (a program updated, the language switched); the latest one wins.
    tx.query_row(
        "INSERT INTO apps (key, name, exe, kind) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (key) DO UPDATE SET name = excluded.name, exe = excluded.exe, kind = excluded.kind
         RETURNING id",
        params![key, a.name, a.exe, a.kind],
        |row| row.get(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppInfo, AppKind, MachineInfo};

    fn app(key: &str, cpu_percent: f64, memory_mb: u64, write_per_second: f64) -> AppInfo {
        AppInfo {
            key: key.into(),
            name: key.to_uppercase(),
            exe_path: format!("C:\\{key}.exe"),
            kind: AppKind::App,
            main_pid: 1,
            process_count: 1,
            cpu_percent,
            memory_bytes: memory_mb << 20,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: write_per_second,
            read_bytes_total: 0,
            write_bytes_total: 0,
            connections: 0,
            listening_ports: Vec::new(),
            oldest_started_at_ms: None,
            windows: Vec::new(),
            pids: vec![1],
            unmeasured_processes: 0,
            services: Vec::new(),
            can_quit: true,
            control: Default::default(),
            project: None,
        }
    }

    fn snapshot(at_seconds: u64, apps: Vec<AppInfo>) -> Snapshot {
        Snapshot {
            taken_at_ms: at_seconds * 1000,
            machine: MachineInfo {
                hostname: "test".into(),
                os_version: "test".into(),
                chip_name: "test".into(),
                physical_cores: Some(2),
                logical_cores: 4,
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

    fn memory_db() -> AppHistory {
        AppHistory::with_connection(Connection::open_in_memory().unwrap())
    }

    /// A day boundary-aligned clock far from the epoch, so the 30-day window never underflows.
    const T0: u64 = 1_800_000_000 - 1_800_000_000 % 600;

    #[test]
    fn a_minute_of_samples_becomes_one_row_per_busy_app() {
        let mut h = memory_db();
        for second in (0..60).step_by(2) {
            h.record(&snapshot(T0 + second, vec![app("busy", 50.0, 100, 0.0), app("idle", 0.0, 10, 0.0)]), 2.0);
        }
        h.flush();
        let top = h.ranking(T0, T0 + 60, RankBy::Cpu, 10);
        assert_eq!(top[0].key, "busy");
        assert!((top[0].cpu_seconds - 30.0).abs() < 1e-9, "half a core for a minute is 30 CPU-seconds");
        assert!((top[0].memory_average - (100 << 20) as f64).abs() < 1.0);
        assert!(top.iter().any(|a| a.key == "idle"), "kept: it is among the heaviest by memory");
    }

    #[test]
    fn folding_keeps_the_totals_and_drops_the_minutes() {
        let mut h = memory_db();
        for minute in 0..20u64 {
            let at = T0 + minute * 60;
            h.record(&snapshot(at, vec![app("busy", 100.0, 200 + minute, 1_000_000.0)]), 60.0_f64.min(10.0));
            h.flush();
        }
        let before = h.ranking(T0, T0 + 1200, RankBy::Write, 5)[0].clone();
        h.fold(T0 + FINE_KEEP + 1200);
        let after = h.ranking(T0, T0 + 1200, RankBy::Write, 5)[0].clone();
        assert!((before.cpu_seconds - after.cpu_seconds).abs() < 1e-6, "CPU time survives the fold");
        assert!((before.write_bytes - after.write_bytes).abs() < 1e-3);
        assert!((before.memory_average - after.memory_average).abs() < 1.0, "weighted, not averaged twice");
        assert_eq!(before.memory_peak, after.memory_peak);
        let fine: i64 = h.db.query_row("SELECT COUNT(*) FROM usage WHERE span = 60", [], |r| r.get(0)).unwrap();
        let coarse: i64 = h.db.query_row("SELECT COUNT(*) FROM usage WHERE span = 600", [], |r| r.get(0)).unwrap();
        assert_eq!((fine, coarse), (0, 2), "twenty minutes are two ten-minute rows");
    }

    #[test]
    fn the_moment_query_finds_the_finest_bucket() {
        let mut h = memory_db();
        h.record(&snapshot(T0 + 5, vec![app("a", 80.0, 50, 0.0), app("b", 10.0, 50, 0.0)]), 5.0);
        h.flush();
        let at = h.at(T0 + 30, 5);
        assert_eq!(at.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), vec!["a", "b"], "busiest first");
        assert!(h.at(T0 + 3600, 5).is_empty(), "nothing recorded then");
    }

    #[test]
    fn a_series_groups_by_step_and_reports_percent_of_a_core() {
        let mut h = memory_db();
        for minute in 0..4u64 {
            for second in (0..60).step_by(10) {
                h.record(&snapshot(T0 + minute * 60 + second, vec![app("x", 25.0, 64, 0.0)]), 10.0);
            }
        }
        h.flush();
        let points = h.series("x", T0, T0 + 240, 120);
        assert_eq!(points.len(), 2, "four minutes in two-minute steps");
        assert!((points[0].cpu_percent - 25.0).abs() < 1e-6);
        assert_eq!(points[1].at_ms, (T0 + 120) * 1000);
    }

    #[test]
    fn thirty_days_is_the_limit() {
        let mut h = memory_db();
        h.record(&snapshot(T0, vec![app("old", 90.0, 10, 0.0)]), 5.0);
        h.flush();
        h.fold(T0 + TOTAL_KEEP + 3600);
        assert!(h.ranking(0, u64::MAX / 2, RankBy::Cpu, 5).is_empty());
        let apps: i64 = h.db.query_row("SELECT COUNT(*) FROM apps", [], |r| r.get(0)).unwrap();
        assert_eq!(apps, 0, "an app with no rows left is forgotten too");
    }
}
