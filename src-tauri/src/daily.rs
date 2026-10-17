//! Bytes per local day, kept for 120 days.
//!
//! Two things depended on this not existing. "Today" was a counter in the sampler, so it went
//! back to zero every time Pulse restarted while still saying "since midnight". And the 7- and
//! 30-day totals the network tab has always had strings for could only have been guessed from
//! the averaged history, which counts an hour Pulse ran for ten minutes as a whole hour.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::DailyTotal;

const KEEP_DAYS: i64 = 120;
const SAVE_EVERY: u32 = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayTotals {
    pub read: u64,
    pub write: u64,
    pub download: u64,
    pub upload: u64,
}

pub struct Daily {
    path: PathBuf,
    /// Keyed by the local day number, days since 1970-01-01 in local time.
    days: BTreeMap<i64, DayTotals>,
    unsaved: u32,
}

impl Daily {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("daily.json");
        let days = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<BTreeMap<i64, DayTotals>>(&raw).ok())
            .unwrap_or_default();
        Self { path, days, unsaved: 0 }
    }

    pub fn day(&self, day: i64) -> Option<DayTotals> {
        self.days.get(&day).copied()
    }

    /// The sampler's running totals for `day`. They only grow within a day, and they are set
    /// rather than added because the sampler already carries the day's whole count.
    pub fn record(&mut self, day: i64, totals: DayTotals) {
        let first_of_day = !self.days.contains_key(&day);
        self.days.insert(day, totals);
        self.unsaved += 1;
        if first_of_day {
            self.days.retain(|kept, _| *kept > day - KEEP_DAYS);
            // A new day is a good moment to make yesterday's final count durable.
            self.save();
        } else if self.unsaved >= SAVE_EVERY {
            self.save();
        }
    }

    /// The last `count` days up to and including `today`, oldest first, with a zero row for any
    /// day Pulse did not run at all, so a chart's bars line up with the calendar.
    pub fn recent(&self, today: i64, count: u32, day_start_ms: impl Fn(i64) -> u64) -> Vec<DailyTotal> {
        let first = today - i64::from(count.max(1)) + 1;
        (first..=today)
            .map(|day| {
                let totals = self.days.get(&day).copied().unwrap_or_default();
                DailyTotal {
                    day_start_ms: day_start_ms(day),
                    read_bytes: totals.read,
                    write_bytes: totals.write,
                    download_bytes: totals.download,
                    upload_bytes: totals.upload,
                }
            })
            .collect()
    }

    pub fn save(&mut self) {
        self.unsaved = 0;
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(raw) = serde_json::to_string(&self.days) else {
            return;
        };
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        if fs::write(&tmp, raw).is_ok() {
            let _ = fs::rename(&tmp, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulse-daily-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir to test against");
        dir
    }

    fn totals(n: u64) -> DayTotals {
        DayTotals { read: n, write: n * 2, download: n * 3, upload: n * 4 }
    }

    #[test]
    fn a_restart_finds_today_where_it_was_left() {
        let dir = temp_dir("restart");
        let mut daily = Daily::open(&dir);
        daily.record(20_000, totals(5));
        daily.record(20_000, totals(9));
        daily.save();
        let back = Daily::open(&dir);
        assert_eq!(back.day(20_000), Some(totals(9)), "the latest count, not a sum of counts");
        assert_eq!(back.day(19_999), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_days_are_calendar_aligned_with_gaps_as_zero() {
        let dir = temp_dir("recent");
        let mut daily = Daily::open(&dir);
        daily.record(100, totals(1));
        daily.record(102, totals(3));
        let rows = daily.recent(102, 4, |day| day as u64 * 10);
        assert_eq!(rows.iter().map(|r| r.day_start_ms).collect::<Vec<_>>(), vec![990, 1000, 1010, 1020]);
        assert_eq!(rows.iter().map(|r| r.read_bytes).collect::<Vec<_>>(), vec![0, 1, 0, 3]);
        assert_eq!(rows[3].upload_bytes, 12);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn days_older_than_the_window_are_dropped_when_a_new_day_starts() {
        let dir = temp_dir("window");
        let mut daily = Daily::open(&dir);
        daily.record(1, totals(1));
        daily.record(1 + KEEP_DAYS, totals(2));
        assert_eq!(daily.day(1), None);
        assert!(daily.day(1 + KEEP_DAYS).is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_file_starts_empty() {
        let dir = temp_dir("broken");
        fs::write(dir.join("daily.json"), b"{ \"3\": trunc").unwrap();
        assert_eq!(Daily::open(&dir).day(3), None);
        let _ = fs::remove_dir_all(dir);
    }
}
