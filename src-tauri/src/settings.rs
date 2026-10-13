//! Settings persistence: one JSON file in the app config dir, versioned for migration.

use crate::models::MetricKind;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrayStyle {
    Value,
    Graph,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayReadout {
    pub id: String,
    pub metric: MetricKind,
    pub style: TrayStyle,
    pub show_icon: bool,
    pub show_caption: bool,
    /// Memory only: show what is free instead of what is used.
    pub invert: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionConfig {
    pub id: String,
    pub visible: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TabConfig {
    pub id: String,
    pub visible: bool,
    pub sections: Vec<SectionConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelConfig {
    pub layout: PanelLayout,
    pub tiles: Vec<SectionConfig>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PanelLayout {
    Tiles,
    List,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowConfig {
    pub tabs: Vec<TabConfig>,
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
#[serde(rename_all = "camelCase")]
pub struct AlertThresholds {
    pub enabled: bool,
    pub cpu_percent: f64,
    pub memory_grow_bytes: u64,
    pub disk_bytes_per_second: f64,
    pub network_bytes_per_second: f64,
    pub sustained_seconds: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
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
    /// Whether AI assistants may read Pulse over MCP (mcp.rs). Off until the user turns it on.
    #[serde(default)]
    pub mcp_enabled: bool,
}

pub const TAB_IDS: [&str; 9] = ["overview", "history", "cpu", "memory", "disk", "network", "gpu", "battery", "sound"];

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
        "history" => vec!["timeline", "ranking", "events"],
        "cpu" => vec!["load", "perCore", "userSystem", "history", "apps"],
        "memory" => vec!["breakdown", "pressure", "commit", "history", "apps"],
        "disk" => vec!["rate", "volumes", "today", "history", "apps"],
        "network" => vec!["rate", "adapters", "totals", "history", "ports", "connections"],
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
            panel: PanelConfig {
                layout: PanelLayout::Tiles,
                tiles: PANEL_METRICS
                    .iter()
                    .map(|m| SectionConfig { id: m.key().to_string(), visible: true })
                    .collect(),
            },
            window: WindowConfig {
                tabs: TAB_IDS
                    .iter()
                    .map(|id| TabConfig {
                        id: id.to_string(),
                        visible: true,
                        sections: default_sections(id),
                    })
                    .collect(),
            },
            alerts: AlertThresholds {
                enabled: true,
                cpu_percent: 80.0,
                memory_grow_bytes: 1_073_741_824,
                disk_bytes_per_second: 40_000_000.0,
                network_bytes_per_second: 20_000_000.0,
                sustained_seconds: 30,
            },
            mcp_enabled: false,
        }
    }
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

    fn read(path: &PathBuf) -> Settings {
        let mut settings = match fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str::<Settings>(&raw).unwrap_or_else(|e| {
                eprintln!("settings.json invalid ({e}); keeping a backup and using defaults");
                let _ = fs::write(path.with_extension("json.bad"), &raw);
                Settings::default()
            }),
            Err(_) => Settings::default(),
        };
        add_missing_tabs(&mut settings);
        settings
    }

    pub fn get(&self) -> Settings {
        self.current.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set(&self, next: Settings) -> Result<Settings, String> {
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        let serialized = serde_json::to_string_pretty(&next).map_err(|e| e.to_string())?;
        fs::create_dir_all(self.path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&tmp, serialized).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        *self.current.write().unwrap_or_else(|p| p.into_inner()) = next.clone();
        Ok(next)
    }
}

/// A settings file keeps the tab list it was written with, so a tab added in a later version
/// would never appear for anyone who had already saved a setting. Missing tabs are put back where
/// the default order has them, visible, and every tab gets any section it is missing; nothing the
/// user hid, reordered or removed is touched.
fn add_missing_tabs(settings: &mut Settings) {
    let tabs = &mut settings.window.tabs;
    for (default_index, id) in TAB_IDS.iter().enumerate() {
        if tabs.iter().any(|tab| tab.id == *id) {
            continue;
        }
        // After whichever tab precedes it by default, or first if none of those are present.
        let after = TAB_IDS[..default_index]
            .iter()
            .rev()
            .find_map(|previous| tabs.iter().position(|tab| tab.id == *previous));
        let at = after.map_or(0, |index| index + 1);
        tabs.insert(at, TabConfig { id: id.to_string(), visible: true, sections: default_sections(id) });
    }
    for tab in tabs.iter_mut() {
        for section in default_sections(&tab.id) {
            if !tab.sections.iter().any(|existing| existing.id == section.id) {
                tab.sections.push(section);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_added_later_appears_for_people_who_saved_settings_before() {
        let mut old = Settings::default();
        old.window.tabs.retain(|tab| tab.id != "history");
        old.window.tabs.swap(0, 1); // the user had put CPU first
        old.window.tabs.iter_mut().find(|tab| tab.id == "sound").unwrap().visible = false;
        old.window.tabs.iter_mut().find(|tab| tab.id == "disk").unwrap().sections.retain(|s| s.id != "today");

        add_missing_tabs(&mut old);

        let ids: Vec<&str> = old.window.tabs.iter().map(|tab| tab.id.as_str()).collect();
        assert_eq!(&ids[..3], &["cpu", "overview", "history"], "after overview, wherever the user put it");
        assert!(!old.window.tabs.iter().find(|tab| tab.id == "sound").unwrap().visible, "a hidden tab stays hidden");
        assert!(old.window.tabs.iter().find(|tab| tab.id == "disk").unwrap().sections.iter().any(|s| s.id == "today"));
        assert_eq!(old.window.tabs.len(), TAB_IDS.len());
    }

    // Settings has no PartialEq on purpose (it is a wire shape), so equality is compared on the
    // bytes that would actually be written to disk.
    fn raw(s: &Settings) -> String {
        serde_json::to_string(s).expect("settings always serialize")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pulse-settings-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temp dir to test against");
        dir
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
}
