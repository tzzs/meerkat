//! Per-app rules that outlive the app: efficiency mode, a priority, alerts muted. Windows forgets
//! all of these the moment the process exits; Pulse already keeps each app's volume across
//! restarts the same way (audio.rs), and this does it for how the app runs.
//!
//! Also the list of processes Pulse has paused, kept on disk so that if Pulse itself goes away
//! while an app is paused, the next start finds it and lets it run again rather than leaving it
//! frozen with nothing on screen to explain why.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::control::{self, Priority};
use crate::metrics::grouping::ProcSample;
use crate::models::{AppControl, AppInfo};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRule {
    pub key: String,
    /// The name it had when the rule was made, for the settings list when it is not running.
    pub name: String,
    #[serde(default)]
    pub efficiency: bool,
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default)]
    pub mute_alerts: bool,
}

impl AppRule {
    fn is_empty(&self) -> bool {
        !self.efficiency && self.priority.is_none() && !self.mute_alerts
    }
}

/// A process instance: its pid and when it started, because pids are handed out again.
type Instance = (u32, u64);

pub struct Rules {
    path: PathBuf,
    paused_path: PathBuf,
    rules: Vec<AppRule>,
    /// Instances a rule has already been applied to; each gets it once, when first seen.
    applied: HashSet<Instance>,
    /// Paused instances, by app key.
    paused: HashMap<String, Vec<Instance>>,
}

impl Rules {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("rules.json");
        let paused_path = dir.join("paused.json");
        let rules = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<AppRule>>(&raw).ok())
            .unwrap_or_default();
        let mut this = Self { path, paused_path, rules, applied: HashSet::new(), paused: HashMap::new() };
        this.resume_left_paused();
        this
    }

    pub fn list(&self) -> Vec<AppRule> {
        self.rules.clone()
    }

    pub fn rule(&self, key: &str) -> Option<&AppRule> {
        self.rules.iter().find(|rule| rule.key == key)
    }

    pub fn muted(&self) -> HashSet<String> {
        self.rules.iter().filter(|rule| rule.mute_alerts).map(|rule| rule.key.clone()).collect()
    }

    /// Stores `rule` (or drops it when it no longer asks for anything) and applies the change
    /// to the app's running processes at once, including undoing what was turned off.
    pub fn set(&mut self, rule: AppRule, running: &[&ProcSample]) -> Result<(), String> {
        let before = self.rule(&rule.key).cloned().unwrap_or_default();
        self.rules.retain(|existing| existing.key != rule.key);
        if !rule.is_empty() {
            self.rules.push(rule.clone());
        }
        self.save()?;
        let mut errors = Vec::new();
        for process in running.iter().filter(|p| p.can_quit) {
            let result = if rule.efficiency != before.efficiency {
                control::set_efficiency(process.pid, rule.efficiency, rule.priority.unwrap_or(Priority::Normal))
            } else if rule.priority != before.priority && !rule.efficiency {
                control::set_priority(process.pid, rule.priority.unwrap_or(Priority::Normal))
            } else {
                Ok(())
            };
            if let Err(error) = result {
                errors.push(error);
            }
            self.applied.insert(instance(process));
        }
        // Some helpers of an app refuse (a sandboxed renderer, say); the rule still stands for
        // the rest, so only a total failure is reported.
        if !errors.is_empty() && errors.len() == running.len() {
            return Err(errors.remove(0));
        }
        Ok(())
    }

    /// Applies the rules to processes seen for the first time: an app restarted, or a new
    /// helper of one already running.
    pub fn enforce(&mut self, apps: &[AppInfo], processes: &HashMap<u32, &ProcSample>) {
        if self.rules.iter().all(|rule| !rule.efficiency && rule.priority.is_none()) {
            return;
        }
        let mut live: HashSet<Instance> = HashSet::new();
        for app in apps {
            let Some(rule) = self.rules.iter().find(|rule| rule.key == app.key) else { continue };
            if !rule.efficiency && rule.priority.is_none() {
                continue;
            }
            for pid in &app.pids {
                let Some(process) = processes.get(pid) else { continue };
                let id = instance(process);
                live.insert(id);
                if self.applied.contains(&id) || !process.can_quit {
                    continue;
                }
                let _ = if rule.efficiency {
                    control::set_efficiency(process.pid, true, Priority::Normal)
                } else {
                    control::set_priority(process.pid, rule.priority.unwrap_or(Priority::Normal))
                };
                self.applied.insert(id);
            }
        }
        self.applied.retain(|id| live.contains(id));
    }

    /// Pauses or resumes an app's processes. Never Pulse itself or anything it started (its
    /// WebView2 processes share a group with every other app's), and never Explorer, which is the
    /// taskbar and desktop the user would need to undo it.
    pub fn pause(&mut self, key: &str, pause: bool, running: &[&ProcSample], protected: &HashSet<u32>) -> Result<usize, String> {
        let mut changed = 0;
        let mut last_error = None;
        if pause {
            let mut list = Vec::new();
            for process in running.iter().filter(|p| p.can_quit && !protected.contains(&p.pid)) {
                if process.name.eq_ignore_ascii_case("explorer.exe") {
                    continue;
                }
                match control::set_suspended(process.pid, true) {
                    Ok(()) => {
                        changed += 1;
                        list.push(instance(process));
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            if !list.is_empty() {
                self.paused.entry(key.to_string()).or_default().extend(list);
            }
        } else {
            let paused = self.paused.remove(key).unwrap_or_default();
            let alive: HashSet<Instance> = running.iter().map(|p| instance(p)).collect();
            for (pid, started) in paused {
                if !alive.contains(&(pid, started)) {
                    continue;
                }
                match control::set_suspended(pid, false) {
                    Ok(()) => changed += 1,
                    Err(error) => last_error = Some(error),
                }
            }
        }
        self.save_paused();
        match (changed, last_error) {
            (0, Some(error)) => Err(error),
            _ => Ok(changed),
        }
    }

    /// Lets every paused process run again; called on the way out.
    pub fn resume_all(&mut self) {
        for (pid, _) in self.paused.values().flatten() {
            let _ = control::set_suspended(*pid, false);
        }
        self.paused.clear();
        self.save_paused();
    }

    /// Writes what Pulse knows about an app's control state into its row.
    pub fn annotate(&self, apps: &mut [AppInfo]) {
        for app in apps {
            let rule = self.rule(&app.key);
            app.control = AppControl {
                efficiency: rule.is_some_and(|r| r.efficiency),
                priority: rule.and_then(|r| r.priority),
                mute_alerts: rule.is_some_and(|r| r.mute_alerts),
                paused: self.paused.contains_key(&app.key),
            };
        }
    }

    /// A previous run that ended while something was paused left the list behind. Those
    /// processes, if they still exist, are resumed now: nothing would otherwise ever do it.
    fn resume_left_paused(&mut self) {
        let left = fs::read_to_string(&self.paused_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<HashMap<String, Vec<Instance>>>(&raw).ok())
            .unwrap_or_default();
        for (pid, _) in left.values().flatten() {
            let _ = control::set_suspended(*pid, false);
        }
        let _ = fs::remove_file(&self.paused_path);
    }

    fn save(&self) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(&self.rules).map_err(|e| e.to_string())?;
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        fs::write(&tmp, raw).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    fn save_paused(&self) {
        if self.paused.is_empty() {
            let _ = fs::remove_file(&self.paused_path);
        } else if let Ok(raw) = serde_json::to_string(&self.paused) {
            let _ = fs::write(&self.paused_path, raw);
        }
    }
}

fn instance(process: &ProcSample) -> Instance {
    (process.pid, process.started_at_ms.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pulse-rules-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_rule_survives_a_restart_and_an_empty_one_is_dropped() {
        let dir = temp_dir("persist");
        let mut rules = Rules::open(&dir);
        rules.set(AppRule { key: "c:\\a.exe".into(), name: "A".into(), mute_alerts: true, ..Default::default() }, &[]).unwrap();
        rules.set(AppRule { key: "c:\\b.exe".into(), name: "B".into(), efficiency: true, ..Default::default() }, &[]).unwrap();
        let back = Rules::open(&dir);
        assert_eq!(back.list().len(), 2);
        assert!(back.muted().contains("c:\\a.exe"));

        let mut back = back;
        back.set(AppRule { key: "c:\\a.exe".into(), name: "A".into(), ..Default::default() }, &[]).unwrap();
        assert_eq!(Rules::open(&dir).list().len(), 1, "a rule that asks for nothing is not kept");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rows_say_what_rules_and_pauses_apply_to_them() {
        let dir = temp_dir("annotate");
        let mut rules = Rules::open(&dir);
        rules
            .set(AppRule { key: "k".into(), name: "K".into(), priority: Some(Priority::BelowNormal), ..Default::default() }, &[])
            .unwrap();
        let mut apps = vec![crate::models::AppInfo { key: "k".into(), ..test_app() }, test_app()];
        rules.annotate(&mut apps);
        assert_eq!(apps[0].control.priority, Some(Priority::BelowNormal));
        assert_eq!(apps[1].control, AppControl::default());
        let _ = fs::remove_dir_all(dir);
    }

    fn test_app() -> AppInfo {
        AppInfo {
            key: "other".into(),
            name: "Other".into(),
            exe_path: String::new(),
            kind: crate::models::AppKind::App,
            main_pid: 1,
            process_count: 1,
            cpu_percent: 0.0,
            memory_bytes: 0,
            read_bytes_per_second: 0.0,
            write_bytes_per_second: 0.0,
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
            control: AppControl::default(),
        }
    }
}
