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
}

pub const TAB_IDS: [&str; 8] = ["overview", "cpu", "memory", "disk", "network", "gpu", "battery", "sound"];

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
            panel: PanelConfig {
                layout: PanelLayout::Tiles,
                tiles: TAB_IDS
                    .iter()
                    .filter(|t| **t != "overview")
                    .map(|id| SectionConfig { id: id.to_string(), visible: true })
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
        match fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str::<Settings>(&raw).unwrap_or_else(|e| {
                eprintln!("settings.json invalid ({e}); keeping a backup and using defaults");
                let _ = fs::write(path.with_extension("json.bad"), &raw);
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn get(&self) -> Settings {
        self.current.read().unwrap().clone()
    }

    pub fn set(&self, next: Settings) -> Result<Settings, String> {
        let mut tmp = self.path.clone();
        tmp.set_extension("json.tmp");
        let serialized = serde_json::to_string_pretty(&next).map_err(|e| e.to_string())?;
        fs::create_dir_all(self.path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&tmp, serialized).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        *self.current.write().unwrap() = next.clone();
        Ok(next)
    }
}
