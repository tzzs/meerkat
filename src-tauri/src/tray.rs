//! Tray readouts. Windows has no menu bar to draw into, so each readout becomes its own
//! notification-area icon, exactly the way TrafficMonitor and Stat Tray work.
//!
//! The icon bitmap is drawn by `tray_render` with GDI on the sampler thread; no webview is
//! involved, so the readout keeps updating while every window is hidden. The tooltip carries
//! the full value text, so the readout is never empty even before the first bitmap.

use tauri::{image::Image, tray::TrayIconBuilder, AppHandle, Manager};

use crate::i18n::{self, Lang, Text};
use crate::models::{MetricKind, Snapshot};
use crate::settings::{Settings, Theme, TrayReadout, TrayStyle};

/// The icon kept when no readout is enabled. Every way into Pulse (the panel, the window, the
/// menu with its Quit) hangs off a tray icon, and the window may be hidden and out of the
/// taskbar, so an app with none would be running and unreachable.
pub const FALLBACK_ID: &str = "tray-pulse";

pub struct TrayManager {
    /// Tray ids in the order the icons were created, which is the order Windows lists them.
    live: Vec<String>,
    /// The language the live menus were built in, so a changed setting rebuilds them.
    lang: Lang,
}

impl TrayManager {
    pub fn new() -> Self {
        Self { live: Vec::new(), lang: Lang::En }
    }

    /// Brings the icons in line with the settings. Windows appends every new icon to the end of
    /// the notification area, so the only way to honour the readouts' order is to recreate all
    /// of them in that order whenever the set or the order differs; a change of style, units or
    /// theme leaves the icons alone and only repaints them.
    pub fn rebuild(&mut self, app: &AppHandle, settings: &Settings) {
        let lang = i18n::resolve(settings.language);
        let wanted = wanted_ids(settings);

        if wanted != self.live {
            for id in self.live.drain(..) {
                app.remove_tray_by_id(&id);
            }
            for id in &wanted {
                let tip = match readout_for(settings, id) {
                    Some(readout) => tooltip(lang, readout.metric, "\u{2014}"),
                    None => i18n::text(lang, Text::Pulse).to_string(),
                };
                match build_icon(app, id, &tip, lang) {
                    Ok(()) => self.live.push(id.clone()),
                    Err(error) => eprintln!("tray {id}: {error}"),
                }
            }
            self.lang = lang;
        } else if lang != self.lang {
            // The menus are built once per icon, so a language change has to replace them.
            for id in &self.live {
                let Some(tray) = app.tray_by_id(id) else { continue };
                match crate::menu::tray_menu(app, lang) {
                    Ok(menu) => {
                        let _ = tray.set_menu(Some(menu));
                    }
                    Err(error) => eprintln!("tray menu {id}: {error}"),
                }
            }
            self.lang = lang;
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
        let lang = i18n::resolve(settings.language);
        for readout in settings.tray.iter().filter(|r| r.enabled) {
            let id = tray_id(readout);
            let Some(tray) = app.tray_by_id(&id) else { continue };
            let text = value_text(readout, snapshot);
            let _ = tray.set_tooltip(Some(tooltip(lang, readout.metric, &text)));
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

/// What the notification area should hold: one icon per enabled readout in settings order, or
/// the fallback alone when nothing is enabled. A readout listed twice is one icon.
fn wanted_ids(settings: &Settings) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for readout in settings.tray.iter().filter(|r| r.enabled) {
        let id = tray_id(readout);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        ids.push(FALLBACK_ID.to_string());
    }
    ids
}

fn readout_for<'a>(settings: &'a Settings, id: &str) -> Option<&'a TrayReadout> {
    settings.tray.iter().find(|r| r.enabled && tray_id(r) == id)
}

fn build_icon(app: &AppHandle, id: &str, tooltip: &str, lang: Lang) -> Result<(), String> {
    let menu = crate::menu::tray_menu(app, lang)?;
    let mut builder = TrayIconBuilder::with_id(id)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(tooltip);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map(|_| ()).map_err(|e| e.to_string())
}

fn tooltip(lang: Lang, metric: MetricKind, value: &str) -> String {
    format!("{} {}", i18n::metric(lang, metric), value)
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

fn compact(bytes_per_second: f64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn readout(id: &str, metric: MetricKind, enabled: bool, style: TrayStyle) -> TrayReadout {
        TrayReadout {
            id: id.into(),
            metric,
            style,
            show_icon: false,
            show_caption: false,
            invert: false,
            enabled,
        }
    }

    fn settings(readouts: Vec<TrayReadout>) -> Settings {
        let mut settings = Settings::default();
        settings.tray = readouts;
        settings
    }

    // The settings file this was reported from had every readout switched off, no taskbar entry
    // and a window that only hides: nothing was left to click or quit.
    #[test]
    fn with_every_readout_off_one_fallback_icon_remains() {
        let none = settings(vec![
            readout("cpu", MetricKind::Cpu, false, TrayStyle::Both),
            readout("memory", MetricKind::Memory, false, TrayStyle::Value),
        ]);
        assert_eq!(wanted_ids(&none), vec![FALLBACK_ID.to_string()]);
        assert_eq!(wanted_ids(&settings(Vec::new())), vec![FALLBACK_ID.to_string()]);
    }

    #[test]
    fn the_fallback_goes_away_once_a_readout_is_enabled() {
        let one = settings(vec![
            readout("cpu", MetricKind::Cpu, false, TrayStyle::Both),
            readout("network", MetricKind::Network, true, TrayStyle::Graph),
        ]);
        assert_eq!(wanted_ids(&one), vec!["tray-network".to_string()]);
    }

    #[test]
    fn icons_follow_the_order_of_the_readouts_in_settings() {
        let a = settings(vec![
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Both),
            readout("memory", MetricKind::Memory, true, TrayStyle::Value),
            readout("network", MetricKind::Network, true, TrayStyle::Graph),
        ]);
        let b = settings(vec![
            readout("network", MetricKind::Network, true, TrayStyle::Graph),
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Both),
            readout("memory", MetricKind::Memory, true, TrayStyle::Value),
        ]);
        assert_eq!(wanted_ids(&a), ["tray-cpu", "tray-memory", "tray-network"]);
        assert_eq!(wanted_ids(&b), ["tray-network", "tray-cpu", "tray-memory"]);
        assert_ne!(wanted_ids(&a), wanted_ids(&b), "a reorder differs from the live icons");
    }

    // Style and invert only repaint: recreating the icons for them would make every icon in the
    // tray blink and drop to the end of the area at each change.
    #[test]
    fn a_change_of_style_does_not_change_which_icons_are_wanted() {
        let before = settings(vec![
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Both),
            readout("memory", MetricKind::Memory, true, TrayStyle::Value),
        ]);
        let mut after = before.clone();
        after.tray[0].style = TrayStyle::Graph;
        after.tray[1].invert = true;
        assert_eq!(wanted_ids(&before), wanted_ids(&after));
    }

    #[test]
    fn a_readout_listed_twice_is_one_icon() {
        let twice = settings(vec![
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Both),
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Value),
        ]);
        assert_eq!(wanted_ids(&twice), ["tray-cpu"]);
    }
}
