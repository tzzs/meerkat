//! Dev-only (debug builds): mirror Meerkat's live state to `<app data>/dev`, so `pnpm dev` in a plain
//! browser shows the same real figures as the app window (`vite.config.ts` serves the folder as
//! `/fixtures`).
//!
//! **Opt-in.** Nothing is written unless the app was started with `MEERKAT_DEV_MIRROR=1` in its
//! environment (`MEERKAT_DEV_MIRROR=1 pnpm tauri dev`). The mirror used to be on in every debug
//! build, and the debug build is the one this machine runs all day: it rewrote ~125 KB of JSON
//! every second, about 10 GB a day.
//!
//! Even when on, a file is written at most once per `LATEST_EVERY` (the snapshot) or
//! `OTHERS_EVERY` (everything else), only when its content differs from what is already there,
//! and atomically (a temp file renamed over it), so the dev server never reads half a file.
//!
//! The bytes are produced by a closure that only runs when a write is due, so a build without the
//! variable never serialises anything for the mirror.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::ipc::AppState;
use crate::models::Snapshot;

/// The latest snapshot: the browser view polls it once a second, and 2 s is plenty for a mirror.
pub const LATEST_EVERY: Duration = Duration::from_secs(2);
/// Series, insights, audio and ranking change on the scale of a minute.
pub const OTHERS_EVERY: Duration = Duration::from_secs(20);

/// The ranges the browser view can ask the history for, and the metrics it draws.
const RANGES: [u64; 5] = [3600, 43_200, 86_400, 604_800, 2_592_000];
const METRICS: [&str; 7] = ["cpu", "memory", "diskWrite", "download", "gpu", "battery", "temp"];

/// Whether the mirror was asked for. Read once: the environment of a running process does not change.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let on = std::env::var("MEERKAT_DEV_MIRROR").map(|value| is_on(&value)).unwrap_or(false);
        if on {
            crate::diag::info("dev mirror: on (MEERKAT_DEV_MIRROR=1), writing to the app data folder's dev directory");
        }
        on
    })
}

fn is_on(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "TRUE" | "True")
}

/// What was last seen of one file: when it was last considered, and a fingerprint of its content.
struct Seen {
    at: Instant,
    fingerprint: u64,
}

fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Whether a file is due: never seen, or last considered `every` ago.
fn is_due(seen: Option<&Seen>, now: Instant, every: Duration) -> bool {
    seen.map_or(true, |s| now.duration_since(s.at) >= every)
}

/// Writes `<dir>/<name>` with what `produce` returns, if the mirror is on, the file is due and
/// its content changed. Safe to call from any thread, as often as is convenient.
pub fn publish_bytes(dir: &Path, name: &str, every: Duration, produce: impl FnOnce() -> Option<Vec<u8>>) {
    if !enabled() {
        return;
    }
    static SEEN: Mutex<Option<HashMap<String, Seen>>> = Mutex::new(None);
    let now = Instant::now();
    let key = dir.join(name).display().to_string();
    {
        let guard = SEEN.lock().unwrap_or_else(|p| p.into_inner());
        if !is_due(guard.as_ref().and_then(|map| map.get(&key)), now, every) {
            return;
        }
    }
    // Produced outside the lock: it may serialise 125 KB or read the history.
    let Some(bytes) = produce() else {
        return;
    };
    let print = fingerprint(&bytes);
    let unchanged = {
        let mut guard = SEEN.lock().unwrap_or_else(|p| p.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        let unchanged = map.get(&key).is_some_and(|seen| seen.fingerprint == print);
        map.insert(key, Seen { at: now, fingerprint: print });
        unchanged
    };
    if unchanged {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let temp = dir.join(format!("{name}.tmp"));
    if std::fs::write(&temp, &bytes).is_ok() {
        let _ = std::fs::rename(&temp, dir.join(name));
    }
}

/// `publish_bytes` for JSON text.
pub fn publish(dir: &Path, name: &str, every: Duration, produce: impl FnOnce() -> Option<String>) {
    publish_bytes(dir, name, every, || produce().map(String::into_bytes));
}

/// The sampler's call, once per tick. Runs on the sampler thread on purpose: Core Audio needs COM,
/// and this is the one place that proves it initialises correctly away from the main thread.
pub fn tick(state: &AppState, snapshot: &Snapshot) {
    if !enabled() {
        return;
    }
    let dir = state.data_dir.join("dev");
    publish(&dir, "latest.json", LATEST_EVERY, || serde_json::to_string(snapshot).ok());
    publish(&dir, "insights.json", OTHERS_EVERY, || {
        serde_json::to_string(&state.watcher.lock().unwrap_or_else(|p| p.into_inner()).list()).ok()
    });
    audio(state);

    let now = crate::history::now_seconds();
    let store = state.history.lock().unwrap_or_else(|p| p.into_inner());
    publish(&dir, "ranking.json", OTHERS_EVERY, || serde_json::to_string(&store.ranking(24, now)).ok());
    for range in RANGES {
        for metric in METRICS {
            publish(&dir, &format!("series-{metric}-{range}.json"), OTHERS_EVERY, || {
                serde_json::to_string(&store.series(metric, range, now)).ok()
            });
        }
    }
}

/// The mixer's state, as `audio_state` answers it. Called by the sampler and by the command, which
/// the Sound tab polls four times a second; the throttle is what keeps that from being a write each.
pub fn audio(state: &AppState) {
    if !enabled() {
        return;
    }
    let dir = state.data_dir.join("dev");
    publish(&dir, "audio.json", LATEST_EVERY, || match crate::ipc::snapshot_audio() {
        Ok(snapshot) => serde_json::to_string(&snapshot).ok(),
        Err(error) => {
            crate::diag::warn(format!("dev mirror: audio snapshot: {error}"));
            None
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_one_turns_the_mirror_on() {
        assert!(is_on("1"));
        assert!(is_on(" 1 "));
        assert!(is_on("true"));
        for off in ["", "0", "false", "yes", "2", "on"] {
            assert!(!is_on(off), "{off:?}");
        }
    }

    #[test]
    fn a_file_is_due_the_first_time_and_then_after_its_interval() {
        let start = Instant::now();
        let seen = Seen { at: start, fingerprint: 1 };
        assert!(is_due(None, start, LATEST_EVERY));
        assert!(!is_due(Some(&seen), start, LATEST_EVERY));
        assert!(!is_due(Some(&seen), start + Duration::from_millis(1_999), LATEST_EVERY));
        assert!(is_due(Some(&seen), start + LATEST_EVERY, LATEST_EVERY));
        assert!(!is_due(Some(&seen), start + Duration::from_secs(19), OTHERS_EVERY));
    }

    #[test]
    fn equal_content_has_equal_fingerprints_and_a_change_has_another() {
        assert_eq!(fingerprint(b"{\"a\":1}"), fingerprint(b"{\"a\":1}"));
        assert_ne!(fingerprint(b"{\"a\":1}"), fingerprint(b"{\"a\":2}"));
    }
}
