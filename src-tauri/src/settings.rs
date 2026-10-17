//! Settings persistence: one JSON file in the app config dir, versioned for migration.
//!
//! Reading is forgiving on purpose. A field the file does not have takes its default, a field
//! this build does not know is ignored, and a field with the wrong shape falls back alone
//! instead of taking every other setting down with it. Whatever is read, or handed to
//! [`SettingsStore::set`], then goes through [`Settings::sanitize`], so the rest of the app only
//! ever sees ids it knows and thresholds it can use.

use crate::models::MetricKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// 2: `alerts.cpu_percent` became a share of the whole machine (it was per core, summed).
pub const SETTINGS_VERSION: u32 = 2;

/// What a file without a version field is taken to be: everything before version 2 had one,
/// so a file without it is old or hand-written, and the v1 migration is harmless for both.
fn legacy_version() -> u32 {
    1
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrayStyle {
    Value,
    Graph,
    Both,
}

fn default_tray_style() -> TrayStyle {
    TrayStyle::Value
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayReadout {
    pub id: String,
    pub metric: MetricKind,
    #[serde(default = "default_tray_style")]
    pub style: TrayStyle,
    #[serde(default)]
    pub show_icon: bool,
    #[serde(default)]
    pub show_caption: bool,
    /// Memory only: show what is free instead of what is used.
    #[serde(default)]
    pub invert: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionConfig {
    pub id: String,
    #[serde(default = "yes")]
    pub visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabConfig {
    pub id: String,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub sections: Vec<SectionConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PanelConfig {
    pub layout: PanelLayout,
    pub tiles: Vec<SectionConfig>,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            layout: PanelLayout::Tiles,
            tiles: PANEL_METRICS
                .iter()
                .map(|m| SectionConfig { id: m.key().to_string(), visible: true })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PanelLayout {
    Tiles,
    List,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowConfig {
    pub tabs: Vec<TabConfig>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            tabs: TAB_IDS
                .iter()
                .map(|id| TabConfig {
                    id: id.to_string(),
                    visible: true,
                    sections: default_sections(id),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TemperatureUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkUnit {
    Bytes,
    Bits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StorageUnit {
    Binary,
    Decimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CpuScale {
    PerCore,
    PerMachine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    Auto,
    En,
    ZhHans,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertThresholds {
    pub enabled: bool,
    /// Percent of the whole machine (all logical cores), 1–100.
    pub cpu_percent: f64,
    /// Growth of one app's memory within the last 30 minutes, in bytes; at least 100 MB.
    pub memory_grow_bytes: u64,
    pub disk_bytes_per_second: f64,
    pub network_bytes_per_second: f64,
    /// How long CPU, disk or network has to stay over its threshold, 5–3600 s.
    pub sustained_seconds: u64,
}

/// The smallest growth the memory rule accepts: below it, ordinary churn would qualify.
pub const MEMORY_GROW_FLOOR_BYTES: u64 = 100 * 1_048_576;

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            enabled: true,
            cpu_percent: 25.0,
            memory_grow_bytes: 1_073_741_824,
            disk_bytes_per_second: 40_000_000.0,
            network_bytes_per_second: 20_000_000.0,
            sustained_seconds: 30,
        }
    }
}

impl AlertThresholds {
    /// The thresholds the rules can actually use. A threshold of 0 used to make every app hit
    /// at once (137 notifications in one tick), so each one is clamped: CPU to 1–100 % of the
    /// machine, the sustain to 5–3600 s, memory growth to at least 100 MB and the rates to at
    /// least 1 B/s. A value that is not a number at all (NaN, ±∞) falls back to its default.
    pub fn sanitized(&self) -> AlertThresholds {
        let defaults = AlertThresholds::default();
        let rate = |value: f64, default: f64| if value.is_finite() { value.max(1.0) } else { default };
        AlertThresholds {
            enabled: self.enabled,
            cpu_percent: if self.cpu_percent.is_finite() {
                self.cpu_percent.clamp(1.0, 100.0)
            } else {
                defaults.cpu_percent
            },
            memory_grow_bytes: self.memory_grow_bytes.max(MEMORY_GROW_FLOOR_BYTES),
            disk_bytes_per_second: rate(self.disk_bytes_per_second, defaults.disk_bytes_per_second),
            network_bytes_per_second: rate(self.network_bytes_per_second, defaults.network_bytes_per_second),
            sustained_seconds: self.sustained_seconds.clamp(5, 3600),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    #[serde(default = "legacy_version")]
    pub version: u32,
    pub language: Language,
    pub temperature_unit: TemperatureUnit,
    pub network_unit: NetworkUnit,
    pub storage_unit: StorageUnit,
    pub cpu_scale: CpuScale,
    pub theme: Theme,
    pub show_system_apps_in_lists: bool,
    pub open_window_on: String,
    pub start_at_login: bool,
    pub show_in_taskbar: bool,
    pub tray: Vec<TrayReadout>,
    pub panel: PanelConfig,
    pub window: WindowConfig,
    pub alerts: AlertThresholds,
}

pub const TAB_IDS: [&str; 8] = ["overview", "cpu", "memory", "disk", "network", "gpu", "battery", "sound"];

/// What the dropdown panel can draw. These are metrics, not tabs: the default used to be built
/// from `TAB_IDS`, which handed the panel "sound" — a tab with no series behind it — and one
/// tile that cannot be drawn takes the whole panel down.
pub const PANEL_METRICS: [MetricKind; 6] = [
    MetricKind::Cpu,
    MetricKind::Memory,
    MetricKind::Disk,
    MetricKind::Network,
    MetricKind::Gpu,
    MetricKind::Battery,
];

pub fn default_sections(tab: &str) -> Vec<SectionConfig> {
    let ids: Vec<&str> = match tab {
        "overview" => vec!["summary", "charts", "topApps", "worthALook", "storage", "insights"],
        "cpu" => vec!["load", "perCore", "userSystem", "history", "apps"],
        "memory" => vec!["breakdown", "pressure", "commit", "history", "apps"],
        "disk" => vec!["rate", "volumes", "today", "history", "apps"],
        "network" => vec!["rate", "adapters", "totals", "history", "connections"],
        "gpu" => vec!["utilisation", "adapters", "memory", "history", "apps"],
        "battery" => vec!["charge", "health", "power", "history"],
        "sound" => vec!["output", "apps", "input"],
        _ => vec![],
    };
    ids.into_iter()
        .map(|id| SectionConfig { id: id.to_string(), visible: true })
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            language: Language::Auto,
            temperature_unit: TemperatureUnit::Celsius,
            network_unit: NetworkUnit::Bytes,
            storage_unit: StorageUnit::Decimal,
            cpu_scale: CpuScale::PerMachine,
            theme: Theme::Auto,
            show_system_apps_in_lists: true,
            open_window_on: "overview".into(),
            start_at_login: true,
            show_in_taskbar: false,
            tray: vec![
                TrayReadout { id: "cpu".into(), metric: MetricKind::Cpu, style: TrayStyle::Both, show_icon: false, show_caption: false, invert: false, enabled: true },
                TrayReadout { id: "memory".into(), metric: MetricKind::Memory, style: TrayStyle::Value, show_icon: false, show_caption: false, invert: false, enabled: true },
                TrayReadout { id: "network".into(), metric: MetricKind::Network, style: TrayStyle::Graph, show_icon: false, show_caption: false, invert: false, enabled: false },
            ],
            panel: PanelConfig::default(),
            window: WindowConfig::default(),
            alerts: AlertThresholds::default(),
        }
    }
}

impl Settings {
    /// Brings any settings — read from disk or sent by the UI — into a shape the app can use:
    /// - tabs keep the user's order and visibility; unknown and repeated ids are dropped, and
    ///   tabs this build has that the list lacks are appended in default order;
    /// - each tab's sections get the same treatment against [`default_sections`];
    /// - panel tiles keep only ids in [`PANEL_METRICS`], deduplicated, missing ones appended
    ///   (a file written by an older default still carried a `sound` tile);
    /// - tray readouts with a repeated id are dropped (each id is one tray icon);
    /// - alert thresholds are clamped ([`AlertThresholds::sanitized`]);
    /// - `open_window_on` is "overview", "last" or a tab id, otherwise "overview".
    pub fn sanitize(&mut self) {
        self.version = SETTINGS_VERSION;

        let defaults: Vec<&str> = TAB_IDS.to_vec();
        let mut tabs = keep_known(std::mem::take(&mut self.window.tabs), &defaults, |tab| &tab.id);
        for id in TAB_IDS {
            if !tabs.iter().any(|tab| tab.id == id) {
                tabs.push(TabConfig { id: id.to_string(), visible: true, sections: Vec::new() });
            }
        }
        for tab in &mut tabs {
            tab.sections = with_defaults(std::mem::take(&mut tab.sections), &default_sections(&tab.id));
        }
        self.window.tabs = tabs;

        let tiles: Vec<SectionConfig> = PANEL_METRICS
            .iter()
            .map(|metric| SectionConfig { id: metric.key().to_string(), visible: true })
            .collect();
        self.panel.tiles = with_defaults(std::mem::take(&mut self.panel.tiles), &tiles);

        let mut seen: Vec<String> = Vec::new();
        self.tray.retain(|readout| {
            if seen.contains(&readout.id) {
                return false;
            }
            seen.push(readout.id.clone());
            true
        });

        self.alerts = self.alerts.sanitized();

        if self.open_window_on != "last" && !TAB_IDS.contains(&self.open_window_on.as_str()) {
            self.open_window_on = "overview".into();
        }
    }
}

/// `items` without the ones whose id is unknown or already taken, in the user's order.
fn keep_known<T>(items: Vec<T>, known: &[&str], id: impl Fn(&T) -> &String) -> Vec<T> {
    let mut kept: Vec<T> = Vec::with_capacity(items.len());
    for item in items {
        let key = id(&item);
        if known.contains(&key.as_str()) && !kept.iter().any(|other| id(other) == key) {
            kept.push(item);
        }
    }
    kept
}

/// The user's sections (known ids only, each once, in their order and visibility) followed by
/// whatever `defaults` has that they lack, in default order.
fn with_defaults(sections: Vec<SectionConfig>, defaults: &[SectionConfig]) -> Vec<SectionConfig> {
    let known: Vec<&str> = defaults.iter().map(|section| section.id.as_str()).collect();
    let mut merged = keep_known(sections, &known, |section| &section.id);
    for section in defaults {
        if !merged.iter().any(|kept| kept.id == section.id) {
            merged.push(section.clone());
        }
    }
    merged
}

/// v1 → v2: the CPU alert threshold was a per-core sum and is now a share of the machine. Only
/// the old default moves to the new one; a value the user picked is theirs to revisit, since
/// no conversion of it could know what they meant.
fn migrate(mut settings: Settings) -> Settings {
    if settings.version < 2 && settings.alerts.cpu_percent == 80.0 {
        settings.alerts.cpu_percent = AlertThresholds::default().cpu_percent;
    }
    settings
}

/// Reads a settings file without letting one bad field cost the user every other setting.
/// The flag is true when some part of the file had to be left out. None when the text is not a
/// JSON object at all.
fn decode(raw: &str) -> Option<(Settings, bool)> {
    // Notepad, and Windows PowerShell's `Set-Content -Encoding UTF8`, put a byte-order mark in
    // front of the text. It is not JSON, so without this a hand-edited file was judged "not a
    // settings file" and every setting in it went back to its default.
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let value: Value = serde_json::from_str(raw).ok()?;
    if !value.is_object() {
        return None;
    }
    if let Ok(settings) = serde_json::from_value::<Settings>(value.clone()) {
        return Some((settings, false));
    }
    // Something in it has the wrong shape — an enum value from a newer build, a hand edit.
    // Start from the defaults and take over every part of the file that still reads, so only
    // the broken piece falls back.
    let mut merged = serde_json::to_value(Settings::default()).ok()?;
    if let Some(object) = merged.as_object_mut() {
        object.insert("version".into(), Value::from(legacy_version()));
    }
    adopt(&mut merged, &mut Vec::new(), &value);
    let settings = serde_json::from_value::<Settings>(merged).ok()?;
    Some((settings, true))
}

/// Copies `incoming`'s fields into `root` at `path` one at a time, keeping each only if the
/// whole document still reads. Objects that do not fit as a whole are tried field by field,
/// arrays item by item.
fn adopt(root: &mut Value, path: &mut Vec<String>, incoming: &Value) {
    let Value::Object(fields) = incoming else {
        return;
    };
    for (key, value) in fields {
        path.push(key.clone());
        if !try_set(root, path, value.clone()) {
            match (value_at(root, path), value) {
                (Some(Value::Object(_)), Value::Object(_)) => adopt(root, path, value),
                (Some(Value::Array(_)), Value::Array(items)) => {
                    let kept: Vec<Value> = items
                        .iter()
                        .filter(|item| accepts(root, path, Value::Array(vec![(*item).clone()])))
                        .cloned()
                        .collect();
                    try_set(root, path, Value::Array(kept));
                }
                _ => crate::diag::warn(format!("settings: ignoring an unreadable {}", path.join("."))),
            }
        }
        path.pop();
    }
}

fn value_at<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(root, |node, key| node.get(key))
}

fn with_value_at(root: &Value, path: &[String], value: Value) -> Option<Value> {
    let (last, parents) = path.split_last()?;
    let mut candidate = root.clone();
    let mut node = &mut candidate;
    for key in parents {
        node = node.get_mut(key)?;
    }
    node.as_object_mut()?.insert(last.clone(), value);
    Some(candidate)
}

fn accepts(root: &Value, path: &[String], value: Value) -> bool {
    with_value_at(root, path, value)
        .is_some_and(|candidate| serde_json::from_value::<Settings>(candidate).is_ok())
}

fn try_set(root: &mut Value, path: &[String], value: Value) -> bool {
    let Some(candidate) = with_value_at(root, path, value) else {
        return false;
    };
    if serde_json::from_value::<Settings>(candidate.clone()).is_err() {
        return false;
    }
    *root = candidate;
    true
}

/// Temp file, then rename: a power cut leaves the old file or the new one, never half of one.
fn write_atomically(path: &Path, settings: &Settings) -> Result<(), String> {
    let serialized = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut tmp = path.to_path_buf();
    tmp.set_extension("json.tmp");
    fs::write(&tmp, serialized).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
    current: std::sync::Arc<std::sync::RwLock<Settings>>,
}

impl SettingsStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let current = Self::read(&path);
        Self {
            path,
            current: std::sync::Arc::new(std::sync::RwLock::new(current)),
        }
    }

    /// The file's settings, brought up to date.
    ///
    /// - No file (first run) or one that cannot be read right now: defaults, and the file is left
    ///   exactly as it is — something that could not be read is never overwritten.
    /// - Not a JSON object: kept as `settings.json.bad`, defaults take its place.
    /// - Older or newer than this build, or with parts that had to be left out or repaired: the
    ///   repaired file is written back, after the original was kept as `settings.v<N>.json.bak`
    ///   (once — an existing backup is never replaced) or, for a same-version file with an
    ///   unreadable part, as `settings.json.bad`. If that copy cannot be made the file is not
    ///   rewritten; this run still uses the repaired settings.
    fn read(path: &Path) -> Settings {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    crate::diag::warn(format!("settings: {} could not be read ({e}); defaults for this run, file left alone", path.display()));
                }
                return Settings::default();
            }
        };
        let Some((decoded, lossy)) = decode(&raw) else {
            crate::diag::warn("settings.json is not a settings file; keeping a backup and using defaults");
            if fs::write(path.with_extension("json.bad"), &raw).is_ok() {
                if let Err(e) = write_atomically(path, &Settings::default()) {
                    crate::diag::error(format!("settings: could not write the defaults back: {e}"));
                }
            }
            return Settings::default();
        };

        let found = decoded.version;
        let before = as_value(&decoded);
        let mut settings = migrate(decoded);
        settings.sanitize();
        if !lossy && before == as_value(&settings) {
            return settings;
        }

        let kept = if found != SETTINGS_VERSION {
            let backup = path.with_file_name(format!("settings.v{found}.json.bak"));
            backup.exists() || fs::write(&backup, &raw).is_ok()
        } else if lossy {
            fs::write(path.with_extension("json.bad"), &raw).is_ok()
        } else {
            true
        };
        if kept {
            match write_atomically(path, &settings) {
                Ok(()) => crate::diag::info(format!("settings: {} brought up to date (was version {found})", path.display())),
                Err(e) => crate::diag::error(format!("settings: could not write the migrated file: {e}")),
            }
        } else {
            crate::diag::warn("settings: could not keep a copy of the old file, so it is left as it is");
        }
        settings
    }

    pub fn get(&self) -> Settings {
        self.current.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Saves `next` after [`Settings::sanitize`] and returns what was saved; the UI adopts the
    /// returned value, so an id or threshold it should not have sent comes straight back fixed.
    pub fn set(&self, mut next: Settings) -> Result<Settings, String> {
        next.sanitize();
        // The lock is held across the write so that two saves cannot interleave on the temp file.
        let mut current = self.current.write().unwrap_or_else(|p| p.into_inner());
        write_atomically(&self.path, &next)?;
        *current = next.clone();
        Ok(next)
    }
}

fn as_value(settings: &Settings) -> Value {
    serde_json::to_value(settings).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Settings has no PartialEq on purpose (it is a wire shape), so equality is compared on the
    // bytes that would actually be written to disk.
    fn raw(s: &Settings) -> String {
        serde_json::to_string(s).expect("settings always serialize")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("meerkat-settings-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir to test against");
        dir
    }

    // Found by running the app against a settings file written by Windows PowerShell 5.1, which
    // starts it with a UTF-8 byte-order mark: the file was judged "not a settings file", kept as
    // .json.bad and replaced by defaults, taking the user's choices with it.
    #[test]
    fn a_byte_order_mark_does_not_make_the_file_unreadable() {
        let plain = serde_json::to_string(&Settings::default()).expect("defaults serialize");
        let with_mark = format!("\u{feff}{plain}");
        let (decoded, lossy) = decode(&with_mark).expect("a BOM is not part of the JSON text");
        assert!(!lossy, "nothing in the file had to be left out");
        assert_eq!(raw(&decoded), raw(&Settings::default()));
        // And a file that merely starts with something else is still refused.
        assert!(decode(&format!("x{plain}")).is_none());
    }

    // The panel's default tiles were once built from TAB_IDS, which put "sound" among the
    // metrics. The panel then asked for a series that does not exist, threw during render, and
    // came up blank with nothing on screen to explain itself.
    #[test]
    fn every_default_panel_tile_is_a_metric() {
        let metrics: Vec<String> = [
            MetricKind::Cpu,
            MetricKind::Memory,
            MetricKind::Disk,
            MetricKind::Network,
            MetricKind::Gpu,
            MetricKind::Battery,
            MetricKind::Temperature,
        ]
        .iter()
        .map(|m| m.key().to_string())
        .collect();
        let tiles = Settings::default().panel.tiles;
        assert!(!tiles.is_empty());
        for tile in &tiles {
            assert!(metrics.contains(&tile.id), "{} is a tab, not a metric", tile.id);
        }
        assert!(!tiles.iter().any(|t| t.id == "sound" || t.id == "overview"));
    }

    #[test]
    fn every_default_tab_exists_and_has_sections() {
        let window = Settings::default().window;
        let ids: Vec<&str> = window.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, TAB_IDS.to_vec(), "the tab list and its defaults may not drift apart");
        for tab in &window.tabs {
            assert!(!tab.sections.is_empty(), "{} opens with nothing to show", tab.id);
        }
    }

    #[test]
    fn an_unknown_tab_has_no_sections_rather_than_someones_else() {
        assert!(default_sections("nope").is_empty());
        let sound: Vec<String> = default_sections("sound").iter().map(|s| s.id.clone()).collect();
        assert_eq!(sound, vec!["output", "apps", "input"]);
    }

    #[test]
    fn a_settings_file_that_cannot_be_read_falls_back_without_losing_it() {
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.json");
        fs::write(&path, b"{ \"theme\": broken mid-write").unwrap();
        let loaded = SettingsStore::read(&path);
        assert_eq!(raw(&loaded), raw(&Settings::default()), "defaults win");
        assert!(dir.join("settings.json.bad").exists(), "the unreadable file is kept for a look");
        assert_eq!(fs::read_to_string(dir.join("settings.json.bad")).unwrap(), "{ \"theme\": broken mid-write");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn what_is_saved_is_what_is_read_back() {
        let dir = temp_dir("roundtrip");
        let store = SettingsStore::open(&dir);
        let mut next = store.get();
        next.theme = Theme::Dark;
        next.panel.layout = PanelLayout::List;
        next.tray.retain(|r| r.metric != MetricKind::Network);
        let saved = store.set(next).expect("saving to a directory that exists");

        let back = SettingsStore::read(&dir.join("settings.json"));
        assert_eq!(raw(&back), raw(&saved), "what comes off the disk is what set() reported");
        assert!(!dir.join("settings.json.tmp").exists(), "the write is a rename, not a truncate");
        assert!(dir.join("settings.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    // What a version-1 install has on disk after the user has customised a few things: the old
    // CPU default, the bogus "sound" panel tile, no "sound" tab, an overview that predates its
    // last section, and a reordered, partly hidden tab list.
    fn v1_file() -> Value {
        let mut file = serde_json::to_value(Settings::default()).unwrap();
        file["version"] = Value::from(1);
        file["alerts"]["cpuPercent"] = Value::from(80.0);
        file["panel"]["tiles"] = serde_json::json!([
            { "id": "cpu", "visible": true },
            { "id": "memory", "visible": false },
            { "id": "disk", "visible": true },
            { "id": "network", "visible": true },
            { "id": "gpu", "visible": true },
            { "id": "battery", "visible": true },
            { "id": "sound", "visible": true },
        ]);
        file["window"]["tabs"] = serde_json::json!([
            { "id": "cpu", "visible": true, "sections": [
                { "id": "history", "visible": true }, { "id": "load", "visible": false },
                { "id": "perCore", "visible": true }, { "id": "userSystem", "visible": true },
                { "id": "apps", "visible": true } ] },
            { "id": "overview", "visible": true, "sections": [
                { "id": "summary", "visible": true }, { "id": "charts", "visible": true },
                { "id": "topApps", "visible": true }, { "id": "worthALook", "visible": true },
                { "id": "storage", "visible": true } ] },
            { "id": "gpu", "visible": false, "sections": [] },
            { "id": "memory", "visible": true, "sections": [] },
            { "id": "disk", "visible": true, "sections": [] },
            { "id": "network", "visible": true, "sections": [] },
            { "id": "battery", "visible": true, "sections": [] },
        ]);
        file
    }

    fn ids(sections: &[SectionConfig]) -> Vec<&str> {
        sections.iter().map(|s| s.id.as_str()).collect()
    }

    fn tab<'a>(settings: &'a Settings, id: &str) -> &'a TabConfig {
        settings.window.tabs.iter().find(|t| t.id == id).expect("every tab is present after sanitising")
    }

    // Every field added to Settings used to make every existing file fail to deserialise, and a
    // failed file meant defaults — the user's whole configuration, gone, by an update.
    #[test]
    fn a_v1_file_is_brought_up_to_date_without_losing_what_the_user_set() {
        let dir = temp_dir("migrate");
        let original = serde_json::to_string_pretty(&v1_file()).unwrap();
        fs::write(dir.join("settings.json"), &original).unwrap();

        let store = SettingsStore::open(&dir);
        let settings = store.get();

        assert_eq!(settings.version, SETTINGS_VERSION);
        assert_eq!(settings.alerts.cpu_percent, 25.0, "the old default moves to the new default");

        let tiles = ids(&settings.panel.tiles);
        assert_eq!(tiles, vec!["cpu", "memory", "disk", "network", "gpu", "battery"], "no sound tile");
        assert!(!settings.panel.tiles[1].visible, "a hidden tile stays hidden");

        let tabs: Vec<&str> = settings.window.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            tabs,
            vec!["cpu", "overview", "gpu", "memory", "disk", "network", "battery", "sound"],
            "the user's order, then what is new"
        );
        assert!(!tab(&settings, "gpu").visible, "a hidden tab stays hidden");
        assert_eq!(
            ids(&tab(&settings, "sound").sections),
            vec!["output", "apps", "input"],
            "a tab the file never had gets its default sections"
        );
        assert_eq!(
            ids(&tab(&settings, "overview").sections),
            vec!["summary", "charts", "topApps", "worthALook", "storage", "insights"],
            "a section added since is appended"
        );
        let cpu = tab(&settings, "cpu");
        assert_eq!(ids(&cpu.sections), vec!["history", "load", "perCore", "userSystem", "apps"]);
        assert!(!cpu.sections[1].visible, "a hidden section stays hidden");
        assert_eq!(ids(&tab(&settings, "memory").sections), vec!["breakdown", "pressure", "commit", "history", "apps"]);

        // The file itself was rewritten, and the old one kept.
        let on_disk: Value = serde_json::from_str(&fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(on_disk["version"], 2);
        assert_eq!(on_disk["alerts"]["cpuPercent"], 25.0);
        assert_eq!(fs::read_to_string(dir.join("settings.v1.json.bak")).unwrap(), original);
        assert_eq!(raw(&SettingsStore::read(&dir.join("settings.json"))), raw(&settings));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_backup_of_the_old_file_is_made_once() {
        let dir = temp_dir("backup-once");
        let backup = dir.join("settings.v1.json.bak");
        fs::write(dir.join("settings.json"), serde_json::to_string(&v1_file()).unwrap()).unwrap();
        SettingsStore::open(&dir);
        assert!(backup.exists());

        // Something older wrote a version-1 file again; the first backup is not replaced.
        fs::write(&backup, "the first backup").unwrap();
        fs::write(dir.join("settings.json"), serde_json::to_string(&v1_file()).unwrap()).unwrap();
        SettingsStore::open(&dir);
        assert_eq!(fs::read_to_string(&backup).unwrap(), "the first backup");

        // A current file is read without a backup or a rewrite.
        let current = fs::read_to_string(dir.join("settings.json")).unwrap();
        let _ = fs::remove_file(&backup);
        SettingsStore::open(&dir);
        assert!(!backup.exists());
        assert_eq!(fs::read_to_string(dir.join("settings.json")).unwrap(), current);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cpu_threshold_the_user_chose_is_not_replaced() {
        let dir = temp_dir("keep-threshold");
        let mut file = v1_file();
        file["alerts"]["cpuPercent"] = Value::from(60.0);
        fs::write(dir.join("settings.json"), serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(SettingsStore::open(&dir).get().alerts.cpu_percent, 60.0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fields_this_build_has_never_heard_of_are_ignored_not_fatal() {
        let dir = temp_dir("unknown-fields");
        let mut file = serde_json::to_value(Settings::default()).unwrap();
        file["theme"] = Value::from("dark");
        file["somethingFromTheFuture"] = serde_json::json!({ "a": [1, 2, 3] });
        file["alerts"]["cpuPercent"] = Value::from(40.0);
        file["alerts"]["tomorrowsKnob"] = Value::from(true);
        file["window"]["tabs"][0]["pinned"] = Value::from(true);
        fs::write(dir.join("settings.json"), serde_json::to_string(&file).unwrap()).unwrap();

        let settings = SettingsStore::open(&dir).get();
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.alerts.cpu_percent, 40.0);
        assert_eq!(settings.window.tabs.len(), TAB_IDS.len());
        assert!(!dir.join("settings.json.bad").exists(), "nothing was unreadable");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_missing_most_fields_takes_defaults_for_them() {
        let dir = temp_dir("sparse");
        fs::write(dir.join("settings.json"), r#"{ "language": "zhHans", "alerts": { "enabled": false } }"#).unwrap();
        let settings = SettingsStore::open(&dir).get();
        assert_eq!(settings.language, Language::ZhHans);
        assert!(!settings.alerts.enabled);
        assert_eq!(settings.alerts.sustained_seconds, 30);
        assert_eq!(settings.tray.len(), Settings::default().tray.len());
        assert_eq!(settings.window.tabs.len(), TAB_IDS.len());
        assert_eq!(settings.version, SETTINGS_VERSION);
        let _ = fs::remove_dir_all(dir);
    }

    // One bad value used to reset the whole file. Now only that value falls back.
    #[test]
    fn one_unreadable_value_costs_only_itself() {
        let dir = temp_dir("one-bad-value");
        let text = r#"{
            "version": 2,
            "language": "zhHans",
            "theme": "sepia",
            "temperatureUnit": "fahrenheit",
            "tray": [
                { "id": "cpu", "metric": "cpu", "style": "graph" },
                { "id": "mystery", "metric": "quantum" },
                { "id": "memory", "metric": "memory", "enabled": false }
            ],
            "alerts": { "cpuPercent": "lots", "sustainedSeconds": 90 }
        }"#;
        fs::write(dir.join("settings.json"), text).unwrap();

        let settings = SettingsStore::open(&dir).get();
        assert_eq!(settings.language, Language::ZhHans, "readable fields are kept");
        assert_eq!(settings.temperature_unit, TemperatureUnit::Fahrenheit);
        assert_eq!(settings.theme, Theme::Auto, "the broken one alone falls back");
        assert_eq!(settings.alerts.cpu_percent, 25.0);
        assert_eq!(settings.alerts.sustained_seconds, 90);
        let tray: Vec<&str> = settings.tray.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(tray, vec!["cpu", "memory"], "the unreadable readout is left out, the rest stay");
        assert_eq!(settings.tray[0].style, TrayStyle::Graph);
        assert!(!settings.tray[1].enabled);
        assert_eq!(fs::read_to_string(dir.join("settings.json.bad")).unwrap(), text, "the original is kept");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn text_that_is_not_a_settings_object_is_kept_and_replaced() {
        for (tag, text) in [("array", "[1, 2, 3]"), ("empty", ""), ("truncated", "{ \"version\": 2, \"them")] {
            let dir = temp_dir(tag);
            fs::write(dir.join("settings.json"), text).unwrap();
            let settings = SettingsStore::open(&dir).get();
            assert_eq!(raw(&settings), raw(&Settings::default()), "{tag}: defaults");
            assert_eq!(fs::read_to_string(dir.join("settings.json.bad")).unwrap(), text, "{tag}: kept");
            // What replaced it reads cleanly, so the next start does not repeat this.
            let again = SettingsStore::read(&dir.join("settings.json"));
            assert_eq!(raw(&again), raw(&Settings::default()), "{tag}: the replacement is valid");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_never_overwritten() {
        let dir = temp_dir("unreadable");
        // A directory where the file should be: reading it fails with something other than
        // "not found", and whatever is there must be left alone.
        fs::create_dir_all(dir.join("settings.json")).unwrap();
        let settings = SettingsStore::read(&dir.join("settings.json"));
        assert_eq!(raw(&settings), raw(&Settings::default()));
        assert!(dir.join("settings.json").is_dir());
        assert!(!dir.join("settings.json.bad").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_is_a_first_run_and_writes_nothing() {
        let dir = temp_dir("first-run");
        let settings = SettingsStore::open(&dir).get();
        assert_eq!(raw(&settings), raw(&Settings::default()));
        assert!(!dir.join("settings.json").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_from_a_newer_build_is_kept_before_it_is_rewritten() {
        let dir = temp_dir("newer");
        let mut file = serde_json::to_value(Settings::default()).unwrap();
        file["version"] = Value::from(3);
        file["theme"] = Value::from("dark");
        let text = serde_json::to_string(&file).unwrap();
        fs::write(dir.join("settings.json"), &text).unwrap();
        let settings = SettingsStore::open(&dir).get();
        assert_eq!(settings.theme, Theme::Dark);
        assert_eq!(settings.version, SETTINGS_VERSION);
        assert_eq!(fs::read_to_string(dir.join("settings.v3.json.bak")).unwrap(), text);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn thresholds_are_clamped_to_what_the_rules_can_use() {
        let mut settings = Settings::default();
        settings.alerts = AlertThresholds {
            enabled: true,
            cpu_percent: 0.0,
            memory_grow_bytes: 5,
            disk_bytes_per_second: -3.0,
            network_bytes_per_second: 0.0,
            sustained_seconds: 0,
        };
        settings.sanitize();
        assert_eq!(settings.alerts.cpu_percent, 1.0);
        assert_eq!(settings.alerts.memory_grow_bytes, MEMORY_GROW_FLOOR_BYTES);
        assert_eq!(settings.alerts.disk_bytes_per_second, 1.0);
        assert_eq!(settings.alerts.network_bytes_per_second, 1.0);
        assert_eq!(settings.alerts.sustained_seconds, 5);

        settings.alerts.cpu_percent = 640.0;
        settings.alerts.sustained_seconds = 1_000_000;
        settings.sanitize();
        assert_eq!(settings.alerts.cpu_percent, 100.0);
        assert_eq!(settings.alerts.sustained_seconds, 3600);

        for not_a_number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            settings.alerts.cpu_percent = not_a_number;
            settings.alerts.disk_bytes_per_second = not_a_number;
            settings.alerts.network_bytes_per_second = not_a_number;
            settings.sanitize();
            let defaults = AlertThresholds::default();
            assert_eq!(settings.alerts.cpu_percent, defaults.cpu_percent);
            assert_eq!(settings.alerts.disk_bytes_per_second, defaults.disk_bytes_per_second);
            assert_eq!(settings.alerts.network_bytes_per_second, defaults.network_bytes_per_second);
        }
    }

    #[test]
    fn open_window_on_is_overview_last_or_a_tab() {
        let mut settings = Settings::default();
        for ok in ["overview", "last", "gpu", "sound"] {
            settings.open_window_on = ok.into();
            settings.sanitize();
            assert_eq!(settings.open_window_on, ok);
        }
        for bad in ["", "Last", "nowhere", "settings"] {
            settings.open_window_on = bad.into();
            settings.sanitize();
            assert_eq!(settings.open_window_on, "overview", "{bad:?}");
        }
    }

    #[test]
    fn sanitising_drops_unknown_and_repeated_ids_and_appends_what_is_missing() {
        let mut settings = Settings::default();
        let section = |id: &str, visible: bool| SectionConfig { id: id.into(), visible };
        settings.window.tabs = vec![
            TabConfig { id: "network".into(), visible: true, sections: vec![section("totals", false), section("nonsense", true), section("totals", true)] },
            TabConfig { id: "wat".into(), visible: true, sections: vec![section("a", true)] },
            TabConfig { id: "network".into(), visible: false, sections: vec![] },
            TabConfig { id: "cpu".into(), visible: false, sections: vec![] },
        ];
        settings.panel.tiles = vec![section("gpu", false), section("sound", true), section("gpu", true), section("overview", true)];
        let readout = |id: &str, metric| TrayReadout {
            id: id.into(), metric, style: TrayStyle::Value, show_icon: false, show_caption: false, invert: false, enabled: true,
        };
        settings.tray = vec![readout("a", MetricKind::Cpu), readout("b", MetricKind::Gpu), readout("a", MetricKind::Memory)];
        settings.sanitize();

        let tabs: Vec<&str> = settings.window.tabs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(tabs, vec!["network", "cpu", "overview", "memory", "disk", "gpu", "battery", "sound"]);
        assert!(settings.window.tabs[0].visible, "the first of two repeats wins");
        assert!(!settings.window.tabs[1].visible);
        assert_eq!(ids(&settings.window.tabs[0].sections), vec!["totals", "rate", "adapters", "history", "connections"]);
        assert!(!settings.window.tabs[0].sections[0].visible);
        assert_eq!(ids(&settings.panel.tiles), vec!["gpu", "cpu", "memory", "disk", "network", "battery"]);
        assert!(!settings.panel.tiles[0].visible);
        assert_eq!(settings.tray.len(), 2);
        assert_eq!(settings.tray[0].metric, MetricKind::Cpu);
        assert_eq!(settings.tray[1].id, "b");
    }

    #[test]
    fn sanitising_a_clean_settings_changes_nothing() {
        let mut settings = Settings::default();
        let before = raw(&settings);
        settings.sanitize();
        assert_eq!(raw(&settings), before);
    }

    // The UI sends whatever it holds; what comes back, and what is on disk, is the sanitised
    // value, so the UI cannot persist an id or threshold the rest of the app would trip over.
    #[test]
    fn set_stores_and_returns_the_sanitised_value() {
        let dir = temp_dir("set-sanitised");
        let store = SettingsStore::open(&dir);
        let mut next = store.get();
        next.alerts.cpu_percent = 5000.0;
        next.alerts.sustained_seconds = 1;
        next.open_window_on = "elsewhere".into();
        next.panel.tiles.push(SectionConfig { id: "sound".into(), visible: true });
        next.version = 1;

        let saved = store.set(next).unwrap();
        assert_eq!(saved.alerts.cpu_percent, 100.0);
        assert_eq!(saved.alerts.sustained_seconds, 5);
        assert_eq!(saved.open_window_on, "overview");
        assert_eq!(saved.version, SETTINGS_VERSION);
        assert!(!saved.panel.tiles.iter().any(|t| t.id == "sound"));
        assert_eq!(raw(&store.get()), raw(&saved));
        assert_eq!(raw(&SettingsStore::read(&dir.join("settings.json"))), raw(&saved));
        let _ = fs::remove_dir_all(dir);
    }
}
