//! Tray readouts. Windows has no menu bar to draw into, so each readout becomes its own
//! notification-area icon, exactly the way TrafficMonitor and Stat Tray work.
//!
//! The icon bitmap is drawn by `tray_render` with GDI on the sampler thread; no webview is
//! involved, so the readout keeps updating while every window is hidden. The tooltip carries
//! the full value text, so the readout is never empty even before the first bitmap.

use std::collections::HashMap;

use tauri::{image::Image, AppHandle, Manager, tray::TrayIconBuilder};

use crate::models::{MetricKind, Snapshot};
use crate::settings::{Settings, Theme, TrayReadout, TrayStyle};

pub struct TrayManager {
    icons: HashMap<String, String>,
}

impl TrayManager {
    pub fn new() -> Self {
        Self { icons: HashMap::new() }
    }

    pub fn rebuild(&mut self, app: &AppHandle, settings: &Settings) {
        let wanted: Vec<&TrayReadout> = settings.tray.iter().filter(|r| r.enabled).collect();

        for (id, _) in self.icons.clone() {
            if !wanted.iter().any(|r| tray_id(r) == id) {
                app.remove_tray_by_id(&id);
                self.icons.remove(&id);
            }
        }

        for readout in wanted {
            let id = tray_id(readout);
            if self.icons.contains_key(&id) {
                continue;
            }
            let built = (|| {
                let menu = crate::menu::tray_menu(app)?;
                let mut builder = TrayIconBuilder::with_id(&id)
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .tooltip(tooltip(readout.metric, "—"));
                if let Some(icon) = app.default_window_icon() {
                    builder = builder.icon(icon.clone());
                }
                builder
                    .on_menu_event(crate::menu::on_menu_event)
                    .on_tray_icon_event(crate::menu::on_tray_event)
                    .build(app)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })();
            match built {
                Ok(_) => {
                    self.icons.insert(id.clone(), readout.metric.key().to_string());
                }
                Err(error) => eprintln!("tray {id}: {error}"),
            }
        }
    }

    pub fn update(&self, app: &AppHandle, snapshot: &Snapshot, settings: &Settings) {
        let dark_taskbar = match settings.theme {
            Theme::Light => false,
            Theme::Dark => true,
            // The Windows taskbar is dark for most installs, and a light guess on a dark
            // bar would make the readout disappear entirely.
            Theme::Auto => true,
        };
        let icon_height = crate::tray_render::icon_height();
        for readout in settings.tray.iter().filter(|r| r.enabled) {
            let id = tray_id(readout);
            let Some(tray) = app.tray_by_id(&id) else { continue };
            let text = value_text(readout, snapshot);
            let _ = tray.set_tooltip(Some(tooltip(readout.metric, &text)));
            let show_value = matches!(readout.style, TrayStyle::Value | TrayStyle::Both);
            let show_graph = matches!(readout.style, TrayStyle::Graph | TrayStyle::Both);
            let rendered = crate::tray_render::render(
                &text,
                &series_for(readout.metric, snapshot),
                hue(readout.metric),
                show_value,
                show_graph,
                dark_taskbar,
                icon_height,
            );
            let image = Image::new(&rendered.rgba, rendered.width, rendered.height);
            if let Err(error) = tray.set_icon(Some(image)) {
                eprintln!("tray icon {id}: {error}");
            }
            #[cfg(debug_assertions)]
            if let Ok(dir) = app.path().app_data_dir() {
                crate::tray_render::dump_debug(&id, &rendered, &dir.join("dev"));
            }
        }
    }

}

fn tray_id(readout: &TrayReadout) -> String {
    format!("tray-{}", readout.id)
}

fn tooltip(metric: MetricKind, value: &str) -> String {
    format!("{} {}", label(metric), value)
}

fn label(metric: MetricKind) -> &'static str {
    match metric {
        MetricKind::Cpu => "CPU",
        MetricKind::Memory => "Memory",
        MetricKind::Disk => "Disk",
        MetricKind::Network => "Network",
        MetricKind::Gpu => "GPU",
        MetricKind::Battery => "Battery",
        MetricKind::Temperature => "Temperature",
    }
}

pub fn value_text(readout: &TrayReadout, snapshot: &Snapshot) -> String {
    match readout.metric {
        MetricKind::Cpu => format!("{}%", snapshot.cpu.total_percent.round()),
        MetricKind::Memory => {
            let used = snapshot.memory.in_use_bytes;
            let total = snapshot.memory.total_bytes.max(1);
            if readout.invert {
                format!("{}%", (snapshot.memory.available_bytes as f64 / total as f64 * 100.0).round())
            } else {
                format!("{}%", (used as f64 / total as f64 * 100.0).round())
            }
        }
        MetricKind::Disk => format!(
            "{} {}  {}",
            compact(snapshot.disk.read_bytes_per_second),
            "\u{2191}",
            compact(snapshot.disk.write_bytes_per_second)
        ),
        MetricKind::Network => format!(
            "{} {} {}",
            "\u{2193}",
            compact(snapshot.network.download_bytes_per_second),
            compact(snapshot.network.upload_bytes_per_second)
        ),
        MetricKind::Gpu => format!(
            "{}%",
            snapshot.gpu.total_percent.unwrap_or(0.0).round()
        ),
        MetricKind::Battery => format!(
            "{}%",
            snapshot.battery.charge_percent.unwrap_or(0.0).round()
        ),
        MetricKind::Temperature => match snapshot.temperature.cpu_celsius {
            Some(c) => format!("{}°", c.round()),
            None => "—".to_string(),
        },
    }
}

fn compact(bytes_per_second: f64) -> String {    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes_per_second;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{:.0}{}", value, UNITS[unit])
    } else {
        format!("{:.1}{}", value, UNITS[unit])
    }
}

fn hue(metric: MetricKind) -> (u8, u8, u8) {
    match metric {
        MetricKind::Cpu => (94, 203, 143),
        MetricKind::Memory => (100, 169, 255),
        MetricKind::Disk => (240, 179, 95),
        MetricKind::Network => (79, 210, 218),
        MetricKind::Gpu => (234, 122, 198),
        MetricKind::Battery => (163, 215, 95),
        MetricKind::Temperature => (255, 139, 94),
    }
}

fn series_for(metric: MetricKind, snapshot: &Snapshot) -> Vec<f64> {
    match metric {
        MetricKind::Cpu => snapshot.cpu.history.clone(),
        MetricKind::Memory => snapshot.memory.history.clone(),
        MetricKind::Disk => snapshot.disk.history_write.clone(),
        MetricKind::Network => snapshot.network.history_download.clone(),
        MetricKind::Gpu => snapshot.gpu.history.clone(),
        MetricKind::Battery => snapshot.battery.history.clone(),
        MetricKind::Temperature => snapshot.temperature.history.clone(),
    }
}
