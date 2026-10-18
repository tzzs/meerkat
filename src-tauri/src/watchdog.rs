//! The runaway dev-process watchdog: it names a runtime that has been stuck long enough to be
//! worth the user's attention, and says why, without ever ending anything. Stopping a process
//! already has exactly one path — the detail view and the confirmation in quit.rs — and a
//! watchdog that could end one on its own would be a second, weaker version of that.
//!
//! Every verdict carries the reading that produced it (see `RunawayReason`), so a row that
//! appears can always be explained from the sample it came from. A process the sampler could not
//! read, a project folder it could not resolve, or a sample it missed produces no row: the whole
//! point is to never accuse a process of being stuck on a guess.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::metrics::projects;
use crate::models::{RunawayReason, RunawayVerdict};

/// One runtime process in one tick, reduced to what the watchdog judges. The sampler builds these
/// from the process table and the app list; the watchdog never touches either itself, so `judge`
/// runs on a list it can be handed whole in a test.
#[derive(Clone, Debug)]
pub struct Observation {
    /// The app row this process is folded into (a project row for a runtime).
    pub app_key: String,
    pub app_name: String,
    /// The process image name, e.g. "node.exe": the watchdog only ever names a dev runtime.
    pub image: String,
    /// The process identity. The pid alone is not enough: Windows hands pids out again.
    pub pid: u32,
    pub started_at_ms: u64,
    /// Percent of one core, summed across threads.
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    /// The project folder the runtime is working for, when the sampler could tell.
    pub project: Option<String>,
    /// Whether that folder still exists on disk at sample time.
    pub project_exists: bool,
    /// Whether the process owns a listening port.
    pub listening: bool,
    /// Whether its app is paused, or muted by a rule: such apps are never reported.
    pub paused: bool,
    pub muted: bool,
}

/// The limits a process has to cross before it is called runaway. Deliberately not settings: the
/// watchdog is a small, conservative set of rules, not a page of knobs.
#[derive(Clone, Debug)]
pub struct Thresholds {
    /// One whole core, fully busy. sysinfo reports a process's CPU summed across its threads, so
    /// a single thread stuck in a loop reads 100.
    pub pinned_core_percent: f64,
    /// How long that core has to stay busy before it is stuck rather than compiling. A build or
    /// a test run holds a core for minutes and then stops; a runaway does not.
    pub cpu_sustained_seconds: u64,
    /// Memory at or above this, and still rising, is a leak rather than a large working set.
    pub memory_ceiling_bytes: u64,
    /// The "still rising" span: growth has to have kept going over this window to count.
    pub memory_trend_seconds: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            pinned_core_percent: 100.0,
            cpu_sustained_seconds: 15 * 60,
            memory_ceiling_bytes: 8 * 1024 * 1024 * 1024,
            memory_trend_seconds: 5 * 60,
        }
    }
}

/// The state the sustained rules need, keyed by process instance so a busy process that exits and
/// a fresh one that starts can never add up to one long "stuck".
#[derive(Default)]
struct WatchState {
    /// (pid, started_at_ms) → when the process first crossed the CPU threshold.
    cpu_since: HashMap<(u32, u64), u64>,
    /// (pid, started_at_ms) → recent memory samples, oldest first, for "still growing".
    memory: HashMap<(u32, u64), VecDeque<(u64, u64)>>,
}

/// How often a process's memory is sampled once it has crossed the ceiling.
const MEMORY_SAMPLE_EVERY: u64 = 15;

/// Turns one tick's observations into verdicts. Pure over its inputs: no wall clock, no process
/// table, no disk — `now` is passed in and `project_exists` was checked by the caller, so a
/// verdict can never be a guess.
fn judge(
    observations: &[Observation],
    state: &mut WatchState,
    thresholds: &Thresholds,
    now: u64,
) -> Vec<RunawayVerdict> {
    let day = crate::metrics::day_key(now.saturating_mul(1000));
    // One row per app, the first rule to fire naming it. A row is never added twice however many
    // of the rules hold for the same process at once.
    let mut hits: HashMap<String, RunawayVerdict> = HashMap::new();
    let present: HashSet<(u32, u64)> = observations.iter().map(|o| (o.pid, o.started_at_ms)).collect();

    for obs in observations {
        if obs.paused || obs.muted || !projects::is_runtime(&obs.image) {
            continue;
        }
        let instance = (obs.pid, obs.started_at_ms);

        // A listening port whose project folder is gone is the clearest case — the server is
        // still bound to a port, but the worktree it served no longer exists — and it holds even
        // when the process is otherwise idle, so it is judged first and without a sustained span.
        if obs.listening {
            if let Some(project) = &obs.project {
                if !obs.project_exists {
                    push(
                        &mut hits,
                        verdict(obs, &day, RunawayReason::ProjectGone { project: project.clone() }),
                    );
                }
            }
        }

        if obs.cpu_percent >= thresholds.pinned_core_percent {
            let since = *state.cpu_since.entry(instance).or_insert(now);
            let sustained = now.saturating_sub(since);
            if sustained >= thresholds.cpu_sustained_seconds {
                push(
                    &mut hits,
                    verdict(obs, &day, RunawayReason::CpuSustained { sustained_seconds: sustained }),
                );
            }
        } else {
            state.cpu_since.remove(&instance);
        }

        if obs.memory_bytes >= thresholds.memory_ceiling_bytes {
            let window = state.memory.entry(instance).or_default();
            let keep = thresholds.memory_trend_seconds + MEMORY_SAMPLE_EVERY;
            if sample_memory(window, now, obs.memory_bytes, keep)
                && still_growing(window, now, thresholds.memory_trend_seconds)
            {
                push(
                    &mut hits,
                    verdict(
                        obs,
                        &day,
                        RunawayReason::MemoryCeiling {
                            memory_bytes: obs.memory_bytes,
                            ceiling_bytes: thresholds.memory_ceiling_bytes,
                        },
                    ),
                );
            }
        } else {
            state.memory.remove(&instance);
        }
    }

    // A process that left the table takes its state with it, so it can neither panic nor leave a
    // stale "since" for the next process to inherit that pid.
    state.cpu_since.retain(|instance, _| present.contains(instance));
    state.memory.retain(|instance, _| present.contains(instance));

    let mut out: Vec<RunawayVerdict> = hits.into_values().collect();
    out.sort_by(|a, b| a.app_key.cmp(&b.app_key));
    out
}

fn push(hits: &mut HashMap<String, RunawayVerdict>, verdict: RunawayVerdict) {
    hits.entry(verdict.app_key.clone()).or_insert(verdict);
}

fn verdict(obs: &Observation, day: &str, reason: RunawayReason) -> RunawayVerdict {
    RunawayVerdict {
        id: format!("runaway:{}:{day}", obs.app_key),
        app_key: obs.app_key.clone(),
        app_name: obs.app_name.clone(),
        reason,
    }
}

/// Adds a sample once `MEMORY_SAMPLE_EVERY` seconds have passed since the last, and drops what
/// left the window. True when a sample was added, which is when the memory rule is worth another
/// look.
fn sample_memory(window: &mut VecDeque<(u64, u64)>, now: u64, bytes: u64, keep_seconds: u64) -> bool {
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
        .is_some_and(|(at, _)| at.saturating_add(keep_seconds) < now)
    {
        window.pop_front();
    }
    true
}

/// Whether the process is still growing: it has grown over the whole trend window and is still at
/// its high — a leak keeps climbing, a cache reaches a plateau.
fn still_growing(window: &VecDeque<(u64, u64)>, now: u64, trend_seconds: u64) -> bool {
    let Some(&(_, current)) = window.back() else {
        return false;
    };
    let Some(older) = window
        .iter()
        .find(|(at, _)| at.saturating_add(trend_seconds) <= now)
        .map(|(_, bytes)| *bytes)
    else {
        return false;
    };
    if current <= older {
        return false;
    }
    let Some(peak) = window.iter().map(|(_, bytes)| *bytes).max() else {
        return false;
    };
    (current as f64) >= peak as f64 * 0.97
}

/// The watchdog's working memory: the sustained-rule state, the day's dismissals, and the list it
/// last reported, so the frontend can ask for it after a window has been hidden.
pub struct Watchdog {
    state: WatchState,
    dismissed: HashSet<String>,
    current: Vec<RunawayVerdict>,
    day: String,
}

impl Watchdog {
    pub fn new() -> Self {
        Self {
            state: WatchState::default(),
            dismissed: HashSet::new(),
            current: Vec::new(),
            day: String::new(),
        }
    }

    /// Judges one tick and remembers the verdicts. Returns the new list so the caller can emit it.
    pub fn observe(
        &mut self,
        observations: &[Observation],
        thresholds: &Thresholds,
        now: u64,
    ) -> Vec<RunawayVerdict> {
        let day = crate::metrics::day_key(now.saturating_mul(1000));
        if day != self.day {
            // A new day: yesterday's dismissals no longer name today's rows.
            self.dismissed.clear();
            self.day = day;
        }
        let mut verdicts = judge(observations, &mut self.state, thresholds, now);
        verdicts.retain(|v| !self.dismissed.contains(&v.id));
        self.current = verdicts;
        self.current.clone()
    }

    pub fn list(&self) -> Vec<RunawayVerdict> {
        self.current.clone()
    }

    /// Marks an app as handled for the rest of the day.
    pub fn dismiss(&mut self, id: &str) {
        self.dismissed.insert(id.to_string());
        self.current.retain(|v| v.id != id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(key: &str, cpu: f64, memory: u64) -> Observation {
        Observation {
            app_key: key.into(),
            app_name: key.into(),
            image: "node.exe".into(),
            pid: 42,
            started_at_ms: 1000,
            cpu_percent: cpu,
            memory_bytes: memory,
            project: None,
            project_exists: true,
            listening: false,
            paused: false,
            muted: false,
        }
    }

    fn thresholds() -> Thresholds {
        Thresholds::default()
    }

    /// A fixed second inside some day, so the rules never depend on the wall clock.
    const T0: u64 = 1_700_000_000;

    #[test]
    fn a_core_pinned_for_thirty_seconds_is_not_runaway() {
        let mut state = WatchState::default();
        let t = thresholds();
        for second in 0..30 {
            let verdicts = judge(&[obs("node", 100.0, 0)], &mut state, &t, T0 + second);
            assert!(verdicts.is_empty(), "a verdict at {second}s");
        }
    }

    #[test]
    fn a_core_pinned_for_twenty_minutes_is_runaway() {
        let mut state = WatchState::default();
        let t = thresholds();
        let mut last = Vec::new();
        for second in 0..(20 * 60) {
            last = judge(&[obs("node", 100.0, 0)], &mut state, &t, T0 + second);
        }
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].app_key, "node");
        assert!(matches!(
            last[0].reason,
            RunawayReason::CpuSustained { sustained_seconds } if sustained_seconds >= t.cpu_sustained_seconds
        ));
    }

    #[test]
    fn busy_then_idle_then_busy_does_not_accumulate() {
        let mut state = WatchState::default();
        let t = thresholds();
        let busy = obs("node", 100.0, 0);
        let idle = obs("node", 0.0, 0);
        // Busy for a stretch well under the threshold, then idle, then busy again: each stretch
        // starts over, so the total can never reach a verdict.
        for second in 0..(t.cpu_sustained_seconds - 60) {
            assert!(judge(&[busy.clone()], &mut state, &t, T0 + second).is_empty());
        }
        for second in 0..60 {
            assert!(judge(&[idle.clone()], &mut state, &t, T0 + t.cpu_sustained_seconds + second).is_empty());
        }
        for second in 0..(t.cpu_sustained_seconds - 60) {
            assert!(
                judge(&[busy.clone()], &mut state, &t, T0 + t.cpu_sustained_seconds + 60 + second).is_empty(),
                "the second stretch starts from zero"
            );
        }
    }

    #[test]
    fn a_listening_port_whose_project_is_gone_is_runaway_even_when_idle() {
        let mut state = WatchState::default();
        let t = thresholds();
        let mut one = obs("node", 0.0, 0);
        one.project = Some(r"E:\work\deleted".into());
        one.project_exists = false;
        one.listening = true;
        let verdicts = judge(&[one], &mut state, &t, T0);
        assert_eq!(verdicts.len(), 1);
        assert!(matches!(verdicts[0].reason, RunawayReason::ProjectGone { .. }));
    }

    #[test]
    fn memory_under_the_ceiling_is_not_reported_however_long() {
        let mut state = WatchState::default();
        let t = thresholds();
        let under = t.memory_ceiling_bytes - 1;
        for minute in 0..120 {
            let verdicts = judge(&[obs("python", 0.0, under)], &mut state, &t, T0 + minute * 60);
            assert!(verdicts.is_empty(), "a verdict after {minute} minutes");
        }
    }

    #[test]
    fn memory_past_the_ceiling_but_flat_is_not_runaway() {
        let mut state = WatchState::default();
        let t = thresholds();
        let flat = t.memory_ceiling_bytes + 512 * 1024 * 1024;
        for sample in 0..=(t.memory_trend_seconds / MEMORY_SAMPLE_EVERY + 1) {
            let verdicts =
                judge(&[obs("python", 0.0, flat)], &mut state, &t, T0 + sample * MEMORY_SAMPLE_EVERY);
            assert!(verdicts.is_empty(), "{verdicts:?}");
        }
    }

    #[test]
    fn memory_past_the_ceiling_and_still_growing_is_runaway() {
        let mut state = WatchState::default();
        let t = thresholds();
        let ceiling = t.memory_ceiling_bytes;
        let mut last = Vec::new();
        for sample in 0..=(t.memory_trend_seconds / MEMORY_SAMPLE_EVERY + 1) {
            let bytes = ceiling + sample * 1024 * 1024;
            last = judge(&[obs("python", 0.0, bytes)], &mut state, &t, T0 + sample * MEMORY_SAMPLE_EVERY);
        }
        assert_eq!(last.len(), 1, "{last:?}");
        assert!(matches!(last[0].reason, RunawayReason::MemoryCeiling { .. }));
    }

    #[test]
    fn a_paused_or_muted_app_is_never_reported() {
        let mut state = WatchState::default();
        let t = thresholds();
        let mut gone = obs("node", 0.0, 0);
        gone.project = Some(r"E:\gone".into());
        gone.project_exists = false;
        gone.listening = true;

        let mut paused = gone.clone();
        paused.paused = true;
        assert!(judge(&[paused], &mut state, &t, T0).is_empty());

        let mut muted = gone;
        muted.muted = true;
        assert!(judge(&[muted], &mut state, &t, T0).is_empty());
    }

    #[test]
    fn an_app_is_reported_once_however_many_rules_fire() {
        let mut state = WatchState::default();
        let t = thresholds();
        let mut one = obs("node", 100.0, 0);
        one.project = Some(r"E:\gone".into());
        one.project_exists = false;
        one.listening = true;
        // The dead-project rule fires from the first tick; the CPU rule joins once the core has
        // been pinned long enough. Neither may add a second row.
        let mut last = Vec::new();
        for second in 0..=(t.cpu_sustained_seconds + 1) {
            last = judge(&[one.clone()], &mut state, &t, T0 + second);
            assert!(last.len() <= 1, "one row, not one per rule");
        }
        assert_eq!(last.len(), 1);
        assert!(matches!(last[0].reason, RunawayReason::ProjectGone { .. }), "the first rule to fire names the row");
    }

    #[test]
    fn a_process_that_disappears_yields_no_verdict_and_no_panic() {
        let mut state = WatchState::default();
        let t = thresholds();
        let _ = judge(&[obs("node", 100.0, 0)], &mut state, &t, T0);
        assert!(!state.cpu_since.is_empty(), "the CPU rule is armed");
        let verdicts = judge(&[], &mut state, &t, T0 + 1);
        assert!(verdicts.is_empty());
        assert!(state.cpu_since.is_empty(), "its state goes with it");
    }

    #[test]
    fn a_non_runtime_is_never_reported() {
        let mut state = WatchState::default();
        let t = thresholds();
        let mut chrome = obs("chrome", 100.0, 0);
        chrome.image = "chrome.exe".into();
        chrome.project = Some(r"E:\gone".into());
        chrome.project_exists = false;
        chrome.listening = true;
        let verdicts = judge(&[chrome], &mut state, &t, T0 + t.cpu_sustained_seconds + 10);
        assert!(verdicts.is_empty());
    }

    #[test]
    fn a_verdict_serialises_to_the_shape_the_frontend_reads() {
        let reason = RunawayReason::CpuSustained { sustained_seconds: 900 };
        assert_eq!(
            serde_json::to_value(&reason).unwrap(),
            serde_json::json!({ "reason": "cpuSustained", "sustainedSeconds": 900 })
        );
        let gone = RunawayReason::ProjectGone { project: r"E:\x".into() };
        assert_eq!(
            serde_json::to_value(&gone).unwrap(),
            serde_json::json!({ "reason": "projectGone", "project": r"E:\x" })
        );
        let mem = RunawayReason::MemoryCeiling { memory_bytes: 8, ceiling_bytes: 9 };
        assert_eq!(
            serde_json::to_value(&mem).unwrap(),
            serde_json::json!({ "reason": "memoryCeiling", "memoryBytes": 8, "ceilingBytes": 9 })
        );
    }

    #[test]
    fn a_dismissed_app_stays_quiet_for_the_rest_of_the_day() {
        let mut watchdog = Watchdog::new();
        let t = thresholds();
        let mut gone = obs("node", 0.0, 0);
        gone.project = Some(r"E:\gone".into());
        gone.project_exists = false;
        gone.listening = true;

        let list = watchdog.observe(&[gone.clone()], &t, T0);
        assert_eq!(list.len(), 1);
        watchdog.dismiss(&list[0].id);
        assert!(watchdog.list().is_empty(), "the dismiss takes the row off the live list");

        let again = watchdog.observe(&[gone], &t, T0 + 60);
        assert!(again.is_empty(), "dismissed for the rest of the day");
    }
}
