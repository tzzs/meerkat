//! Tray readouts. Windows has no menu bar to draw into, so each readout becomes its own
//! notification-area icon, exactly the way TrafficMonitor and Stat Tray work.
//!
//! The icon bitmap is drawn by `tray_render` with GDI on the sampler thread; no webview is
//! involved, so the readout keeps updating while every window is hidden. The tooltip carries
//! the full value text, so the readout is never empty even before the first bitmap.
//!
//! # Threads
//!
//! A notification-area icon belongs to the main thread: creating, removing or changing one
//! (`set_icon`, `set_tooltip`, `set_menu`) is posted to it and the caller then waits for the
//! answer. A lock held across such a call is a deadlock waiting for the main thread to want it,
//! and an earlier version had exactly that: the sampler held the manager's mutex across
//! `set_icon` while `settings_set`, which Tauri ran on the main thread, waited for the mutex.
//!
//! So the manager's mutex is taken **on the main thread only**, inside the closures this module
//! posts (`reconcile` and `paint`), and even there only to decide and to record: it is released
//! before each call into the shell. Nothing waits for the main thread while holding any lock: the
//! sampler posts its frames and moves on, and `rebuild` waits for the main thread but holds
//! nothing while it does.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use tauri::{image::Image, tray::TrayIconBuilder, AppHandle, Manager};

use crate::i18n::{self, Lang, Text, Units};
use crate::ipc::AppState;
use crate::models::{Availability, MetricKind, Snapshot};
use crate::settings::{NetworkUnit, Settings, StorageUnit, TemperatureUnit, TrayReadout, TrayStyle};
use crate::tray_render::{self, Frame, Layout};

/// The icon kept when no readout is enabled. Every way into Pulse (the panel, the window, the
/// menu with its Quit) hangs off a tray icon, and the window may be hidden and out of the
/// taskbar, so an app with none would be running and unreachable.
pub const FALLBACK_ID: &str = "tray-pulse";

/// What a reading says when there is no reading. Never a zero: a machine without a battery is
/// not at 0 %.
const ABSENT: &str = "\u{2014}";

/// A rate's sparkline never zooms in below this, so a quiet line stays quiet.
const NETWORK_FLOOR: f64 = 100_000.0;
const DISK_FLOOR: f64 = 1_000_000.0;

/// How long `rebuild` waits for the main thread before it gives up on an answer. Far longer
/// than any icon takes; it only has to end a wait on an event loop that has already stopped.
const MAIN_THREAD_PATIENCE: Duration = Duration::from_secs(10);

/// What Pulse has put in the notification area, and what each icon is known to be showing.
///
/// Locked on the main thread only (see the module docs).
pub struct TrayManager {
    /// Tray ids in the order the icons were created, which is the order Windows lists them.
    live: Vec<String>,
    /// The language the live menus were built in, so a changed setting rebuilds them.
    lang: Lang,
    /// The last bitmap and tooltip handed to each icon. Every call into the shell is work for the
    /// main thread, and most seconds a readout is exactly what it was.
    shown: HashMap<String, Shown>,
}

#[derive(Default)]
struct Shown {
    rgba: Vec<u8>,
    tooltip: String,
}

/// One icon as it should look right now: drawn and worded, but not yet handed to the shell.
pub struct IconFrame {
    id: String,
    tooltip: String,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// The part of an `IconFrame` the shell does not have yet.
#[derive(Debug, PartialEq, Eq)]
struct IconWrite {
    id: String,
    tooltip: Option<String>,
    /// RGBA bytes, width, height.
    image: Option<(Vec<u8>, u32, u32)>,
}

/// What `reconcile` has to do to bring the live icons in line with the ones wanted.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// Already right.
    Keep,
    /// The set of icons or their order differs. Windows appends every new icon to the end of
    /// the notification area, so the only way to honour the readouts' order is to recreate all
    /// of them in that order; a change of style, units or theme leaves the icons alone and only
    /// repaints them.
    Recreate,
    /// The same icons in another language: their menus are replaced.
    Menus,
}

impl TrayManager {
    pub fn new() -> Self {
        Self { live: Vec::new(), lang: Lang::En, shown: HashMap::new() }
    }

    fn plan(&self, wanted: &[String], lang: Lang) -> Plan {
        if wanted != self.live.as_slice() {
            Plan::Recreate
        } else if lang != self.lang {
            Plan::Menus
        } else {
            Plan::Keep
        }
    }

    /// Every live icon is about to be removed: nothing is live and nothing is known to be shown.
    fn retire_all(&mut self) -> Vec<String> {
        self.shown.clear();
        std::mem::take(&mut self.live)
    }

    /// Of this tick's frames, the writes the icons actually need: a frame equal to what an icon
    /// already shows costs nothing. What is being written is remembered at once; `rejected`
    /// takes it back when the shell refuses, so the next tick tries again.
    fn select_writes(&mut self, frames: Vec<IconFrame>) -> Vec<IconWrite> {
        let mut writes = Vec::new();
        for frame in frames {
            // An icon that was never created has nowhere to be drawn.
            if !self.live.contains(&frame.id) {
                continue;
            }
            let shown = self.shown.entry(frame.id.clone()).or_default();
            let tooltip = if shown.tooltip != frame.tooltip {
                shown.tooltip.clone_from(&frame.tooltip);
                Some(frame.tooltip)
            } else {
                None
            };
            let image = if shown.rgba != frame.rgba {
                shown.rgba.clone_from(&frame.rgba);
                Some((frame.rgba, frame.width, frame.height))
            } else {
                None
            };
            if tooltip.is_some() || image.is_some() {
                writes.push(IconWrite { id: frame.id, tooltip, image });
            }
        }
        writes
    }

    /// The shell did not take a write: forget what was assumed shown.
    fn rejected(&mut self, id: &str) {
        self.shown.remove(id);
    }
}

/// Runs `work` on the main thread and hands back what it returned, or `None` when the event
/// loop has stopped or does not answer. On the main thread itself it runs at once, which is how
/// Tauri's own `run_on_main_thread` behaves. The caller must hold no lock while it waits.
fn on_main_thread<T: Send + 'static>(app: &AppHandle, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(work());
    })
    .ok()?;
    rx.recv_timeout(MAIN_THREAD_PATIENCE).ok()
}

/// Brings the icons in line with the settings the store holds *now*, paints them from the
/// latest reading, and returns once the main thread is done. Reading the settings where the
/// work happens, not where it was asked for, is what keeps two quick saves from leaving the
/// icons at the older one.
pub fn rebuild(app: &AppHandle) {
    let handle = app.clone();
    let done = on_main_thread(app, move || {
        let Some(state) = handle.try_state::<AppState>() else {
            return;
        };
        let settings = state.settings.get();
        reconcile(&handle, &settings);
        let frames = state
            .latest
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|snapshot| frames_for(snapshot, &settings));
        if let Some(frames) = frames {
            paint(&handle, frames);
        }
    });
    if done.is_none() {
        crate::diag::warn("tray: the main thread did not answer the rebuild");
    }
}

/// Draws this tick's readouts and posts them to the main thread. Returns at once: the sampler
/// never waits for the shell.
pub fn update(app: &AppHandle, snapshot: &Snapshot, settings: &Settings) {
    let frames = frames_for(snapshot, settings);
    if frames.is_empty() {
        return;
    }
    let handle = app.clone();
    // A failure means the event loop has stopped, and there is nothing left to paint on.
    let _ = app.run_on_main_thread(move || paint(&handle, frames));
}

/// Every enabled readout drawn from `snapshot`, in settings order. A readout listed twice is
/// one icon, and the first one speaks for it.
fn frames_for(snapshot: &Snapshot, settings: &Settings) -> Vec<IconFrame> {
    let lang = i18n::resolve(settings.language);
    let units = Units::of(settings);
    // The taskbar's theme is Windows' own system theme: Pulse's theme setting is about its
    // windows, and a dark default on a light bar is how the readout used to vanish.
    let light_bar = tray_render::taskbar_is_light();
    let size = tray_render::icon_size();

    let mut frames: Vec<IconFrame> = Vec::new();
    for readout in settings.tray.iter().filter(|r| r.enabled) {
        let id = tray_id(readout);
        if frames.iter().any(|f| f.id == id) {
            continue;
        }
        let reading = reading(readout, snapshot, lang, units);
        let rendered = tray_render::render(&Frame {
            label: &reading.label,
            series: &reading.series,
            range: reading.range,
            hue: hue(readout.metric),
            layout: layout_of(readout.style),
            light_bar,
            size,
        });
        frames.push(IconFrame {
            id,
            tooltip: reading.tooltip,
            rgba: rendered.rgba,
            width: rendered.width,
            height: rendered.height,
        });
    }
    frames
}

/// Main thread only: creates, removes and re-menus icons until they are the ones wanted.
///
/// The manager's lock is held to decide and to record, never across a call into the shell.
fn reconcile(app: &AppHandle, settings: &Settings) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let lang = i18n::resolve(settings.language);
    let wanted = wanted_ids(settings);

    let (plan, retired, live) = {
        let mut manager = state.tray.lock().unwrap_or_else(|p| p.into_inner());
        let plan = manager.plan(&wanted, lang);
        let retired = if plan == Plan::Recreate { manager.retire_all() } else { Vec::new() };
        (plan, retired, manager.live.clone())
    };

    match plan {
        Plan::Keep => {}
        Plan::Recreate => {
            for id in retired {
                app.remove_tray_by_id(&id);
            }
            let mut created = Vec::with_capacity(wanted.len());
            for id in &wanted {
                let tip = match readout_for(settings, id) {
                    Some(readout) => format!("{} {ABSENT}", i18n::metric(lang, readout.metric)),
                    None => i18n::text(lang, Text::Pulse).to_string(),
                };
                match build_icon(app, id, &tip, lang) {
                    Ok(()) => created.push(id.clone()),
                    Err(error) => crate::diag::error(format!("tray {id}: {error}")),
                }
            }
            let mut manager = state.tray.lock().unwrap_or_else(|p| p.into_inner());
            manager.live = created;
            manager.lang = lang;
        }
        Plan::Menus => {
            // The menus are built once per icon, so a language change has to replace them.
            for id in &live {
                let Some(tray) = app.tray_by_id(id) else { continue };
                match crate::menu::tray_menu(app, lang) {
                    Ok(menu) => {
                        let _ = tray.set_menu(Some(menu));
                    }
                    Err(error) => crate::diag::warn(format!("tray menu {id}: {error}")),
                }
            }
            state.tray.lock().unwrap_or_else(|p| p.into_inner()).lang = lang;
        }
    }
}

/// Main thread only: hands the shell what it does not already show. As in `reconcile`, the lock
/// is held to decide what to write and to take back what was refused, not while writing.
fn paint(app: &AppHandle, frames: Vec<IconFrame>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let writes = state.tray.lock().unwrap_or_else(|p| p.into_inner()).select_writes(frames);

    let mut refused: Vec<String> = Vec::new();
    for IconWrite { id, tooltip, image } in writes {
        let Some(tray) = app.tray_by_id(&id) else {
            refused.push(id);
            continue;
        };
        let mut failed = false;
        if let Some(tooltip) = tooltip {
            if let Err(error) = tray.set_tooltip(Some(tooltip)) {
                crate::diag::warn(format!("tray tooltip {id}: {error}"));
                failed = true;
            }
        }
        if let Some((rgba, width, height)) = image {
            // What the taskbar was given, for looking at without a screenshot of the shell.
            #[cfg(debug_assertions)]
            crate::devmirror::publish_bytes(
                &state.data_dir.join("dev"),
                &format!("{id}.rgba"),
                crate::devmirror::LATEST_EVERY,
                || Some(rgba.clone()),
            );
            if let Err(error) = tray.set_icon(Some(Image::new_owned(rgba, width, height))) {
                crate::diag::warn(format!("tray icon {id}: {error}"));
                failed = true;
            }
        }
        if failed {
            refused.push(id);
        }
    }

    if !refused.is_empty() {
        let mut manager = state.tray.lock().unwrap_or_else(|p| p.into_inner());
        for id in &refused {
            manager.rejected(id);
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

fn layout_of(style: TrayStyle) -> Layout {
    match style {
        TrayStyle::Value => Layout::Value,
        TrayStyle::Graph => Layout::Graph,
        TrayStyle::Both => Layout::Both,
    }
}

// ---------------------------------------------------------------------------------------------
// What a readout says
// ---------------------------------------------------------------------------------------------

/// One readout, ready to draw: the sentence for its tooltip, the few characters that fit on the
/// icon, and the samples of its sparkline.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub tooltip: String,
    pub label: String,
    /// Empty when the metric has no reading: a flat zero line would be a made-up one.
    pub series: Vec<f64>,
    /// The values on the bottom and the top of the sparkline.
    pub range: (f64, f64),
}

fn absent(lang: Lang, metric: MetricKind) -> Reading {
    Reading {
        tooltip: format!("{} {ABSENT}", i18n::metric(lang, metric)),
        label: ABSENT.to_string(),
        series: Vec::new(),
        range: (0.0, 100.0),
    }
}

/// A share of something, 0-100, on the fixed scale every percentage uses.
fn percentage(lang: Lang, metric: MetricKind, percent: f64, suffix: Option<&str>, series: Vec<f64>) -> Reading {
    let percent = percent.clamp(0.0, 100.0);
    let mut tooltip = format!("{} {}", i18n::metric(lang, metric), i18n::percent(percent));
    if let Some(suffix) = suffix {
        tooltip.push(' ');
        tooltip.push_str(suffix);
    }
    Reading { tooltip, label: format!("{}", percent.round() as i64), series, range: (0.0, 100.0) }
}

pub fn reading(readout: &TrayReadout, snapshot: &Snapshot, lang: Lang, units: Units) -> Reading {
    let metric = readout.metric;
    let name = i18n::metric(lang, metric);
    match metric {
        MetricKind::Cpu => {
            let total = snapshot.cpu.total_percent;
            if !total.is_finite() {
                return absent(lang, metric);
            }
            percentage(lang, metric, total, None, snapshot.cpu.history.clone())
        }
        MetricKind::Memory => {
            let memory = &snapshot.memory;
            if memory.total_bytes == 0 {
                return absent(lang, metric);
            }
            let total = memory.total_bytes as f64;
            if readout.invert {
                let available = memory.available_bytes as f64 / total * 100.0;
                let inverted = memory.history.iter().map(|v| 100.0 - v).collect();
                percentage(lang, metric, available, Some(i18n::text(lang, Text::Available)), inverted)
            } else {
                let used = memory.in_use_bytes as f64 / total * 100.0;
                percentage(lang, metric, used, None, memory.history.clone())
            }
        }
        MetricKind::Disk => {
            let disk = &snapshot.disk;
            if disk.availability == Availability::Unavailable {
                return absent(lang, metric);
            }
            let series = larger_of(&disk.history_read, &disk.history_write);
            let ceiling = tray_render::nice_ceiling(series.iter().cloned().fold(0.0, f64::max), DISK_FLOOR);
            Reading {
                tooltip: format!(
                    "{name} {} {} \u{b7} {} {}",
                    i18n::text(lang, Text::Reading),
                    i18n::rate(disk.read_bytes_per_second, units, false),
                    i18n::text(lang, Text::Writing),
                    i18n::rate(disk.write_bytes_per_second, units, false),
                ),
                label: compact_rate(disk.read_bytes_per_second.max(disk.write_bytes_per_second), units, false),
                series,
                range: (0.0, ceiling),
            }
        }
        MetricKind::Network => {
            let network = &snapshot.network;
            let series = larger_of(&network.history_download, &network.history_upload);
            let ceiling = tray_render::nice_ceiling(series.iter().cloned().fold(0.0, f64::max), NETWORK_FLOOR);
            Reading {
                tooltip: format!(
                    "{name} \u{2193} {} \u{b7} \u{2191} {}",
                    i18n::rate(network.download_bytes_per_second, units, true),
                    i18n::rate(network.upload_bytes_per_second, units, true),
                ),
                label: compact_rate(
                    network.download_bytes_per_second.max(network.upload_bytes_per_second),
                    units,
                    true,
                ),
                series,
                range: (0.0, ceiling),
            }
        }
        MetricKind::Gpu => match snapshot.gpu.total_percent {
            Some(percent) if percent.is_finite() => {
                percentage(lang, metric, percent, None, snapshot.gpu.history.clone())
            }
            _ => absent(lang, metric),
        },
        MetricKind::Battery => match snapshot.battery.charge_percent {
            Some(percent) if snapshot.battery.has_battery && percent.is_finite() => {
                percentage(lang, metric, percent, None, snapshot.battery.history.clone())
            }
            _ => absent(lang, metric),
        },
        MetricKind::Temperature => match snapshot.temperature.hottest {
            Some(celsius) if celsius.is_finite() => {
                let shown = match units.temperature {
                    TemperatureUnit::Fahrenheit => celsius * 1.8 + 32.0,
                    TemperatureUnit::Celsius => celsius,
                };
                Reading {
                    tooltip: format!("{name} {}", i18n::temperature(celsius, units)),
                    label: format!("{}\u{b0}", shown.round() as i64),
                    series: snapshot.temperature.history.clone(),
                    // Celsius whatever the unit shown: the sparkline's scale is a shape, not a figure.
                    range: (0.0, 100.0),
                }
            }
            _ => absent(lang, metric),
        },
    }
}

/// The per-sample larger of two aligned histories, which is what a combined readout's number is.
/// The newest samples line up, so a shorter history is matched from the end.
fn larger_of(a: &[f64], b: &[f64]) -> Vec<f64> {
    let len = a.len().min(b.len());
    a[a.len() - len..]
        .iter()
        .zip(&b[b.len() - len..])
        .map(|(x, y)| x.max(*y))
        .collect()
}

/// A rate in as few characters as it can be read in: "0B", "512B", "1.2M", "345K", "12G". Steps
/// and units follow the settings (decimal or binary storage, bytes or bits for the network);
/// the exact figure is in the tooltip.
pub fn compact_rate(bytes_per_second: f64, units: Units, network: bool) -> String {
    let (mut value, step, letters) = if network && units.network == NetworkUnit::Bits {
        (bytes_per_second * 8.0, 1000.0, ["b", "k", "M", "G", "T"])
    } else {
        let step = match units.storage {
            StorageUnit::Binary => 1024.0,
            StorageUnit::Decimal => 1000.0,
        };
        (bytes_per_second, step, ["B", "K", "M", "G", "T"])
    };
    let mut index = 0;
    while value.round() >= 1000.0 && index < letters.len() - 1 {
        value /= step;
        index += 1;
    }
    let number = if value < 0.05 {
        "0".to_string()
    } else if (value * 10.0).round() / 10.0 < 10.0 {
        format!("{value:.1}").trim_end_matches(".0").to_string()
    } else {
        format!("{value:.0}")
    };
    format!("{number}{}", letters[index])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        BatteryMetric, CpuMetric, DiskMetric, GpuMetric, MachineInfo, MemoryMetric, NetworkMetric,
        TemperatureMetric,
    };

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

    fn snapshot() -> Snapshot {
        Snapshot {
            taken_at_ms: 0,
            machine: MachineInfo {
                hostname: "host".into(),
                os_version: "Windows 11".into(),
                chip_name: "CPU".into(),
                physical_cores: Some(8),
                logical_cores: 16,
                total_memory_bytes: 32_000_000_000,
                uptime_seconds: 0,
            },
            cpu: CpuMetric { total_percent: 16.4, history: vec![10.0, 20.0, 16.4], ..Default::default() },
            memory: MemoryMetric {
                total_bytes: 32_000_000_000,
                in_use_bytes: 20_000_000_000,
                available_bytes: 12_000_000_000,
                history: vec![60.0, 62.5],
                ..Default::default()
            },
            disk: DiskMetric {
                read_bytes_per_second: 12_300_000.0,
                write_bytes_per_second: 4_500_000.0,
                history_read: vec![1.0e6, 12.3e6],
                history_write: vec![2.0e6, 4.5e6],
                ..Default::default()
            },
            network: NetworkMetric {
                download_bytes_per_second: 1_234_567.0,
                upload_bytes_per_second: 45_000.0,
                history_download: vec![1.0e6, 1_234_567.0],
                history_upload: vec![10_000.0, 45_000.0],
                ..Default::default()
            },
            gpu: GpuMetric::default(),
            battery: BatteryMetric::default(),
            temperature: TemperatureMetric::default(),
            apps: Vec::new(),
            process_count: 0,
            sampler_cost_microseconds: 0,
        }
    }

    fn read(metric: MetricKind) -> Reading {
        reading(&readout("x", metric, true, TrayStyle::Both), &snapshot(), Lang::En, Units::default())
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

    #[test]
    fn percentages_are_whole_numbers_on_the_icon_and_carry_the_sign_in_the_tooltip() {
        let cpu = read(MetricKind::Cpu);
        assert_eq!(cpu.label, "16");
        assert_eq!(cpu.tooltip, "CPU 16%");
        assert_eq!(cpu.range, (0.0, 100.0), "a percentage is always on the 0-100 scale");
        let memory = read(MetricKind::Memory);
        assert_eq!(memory.label, "63");
        assert_eq!(memory.tooltip, "Memory 63%");
        assert_eq!(memory.series, vec![60.0, 62.5]);
    }

    #[test]
    fn inverted_memory_reads_what_is_free_and_says_so() {
        let mut flipped = readout("memory", MetricKind::Memory, true, TrayStyle::Value);
        flipped.invert = true;
        let reading = reading(&flipped, &snapshot(), Lang::En, Units::default());
        assert_eq!(reading.label, "38");
        assert_eq!(reading.tooltip, "Memory 38% available");
        assert_eq!(reading.series, vec![40.0, 37.5], "the sparkline is inverted with the number");
        let zh = super::reading(&flipped, &snapshot(), Lang::ZhHans, Units::default());
        assert_eq!(zh.tooltip, "\u{5185}\u{5b58} 38% \u{53ef}\u{7528}");
    }

    // The old text printed "0%" for a machine without a battery and for a GPU with no reading,
    // and drew a flat line from the zero its history was padded with.
    #[test]
    fn a_missing_reading_is_a_dash_and_has_no_graph() {
        for metric in [MetricKind::Battery, MetricKind::Gpu, MetricKind::Temperature] {
            let reading = read(metric);
            assert_eq!(reading.label, "\u{2014}", "{metric:?}");
            assert!(reading.series.is_empty(), "{metric:?} has no line to draw");
            assert!(reading.tooltip.ends_with('\u{2014}'), "{}", reading.tooltip);
        }
        let mut with_battery = snapshot();
        with_battery.battery.has_battery = true;
        with_battery.battery.charge_percent = Some(80.0);
        with_battery.battery.history = vec![79.0, 80.0];
        let battery = readout("b", MetricKind::Battery, true, TrayStyle::Both);
        let present = reading(&battery, &with_battery, Lang::En, Units::default());
        assert_eq!((present.label.as_str(), present.tooltip.as_str()), ("80", "Battery 80%"));

        let mut unmeasured = snapshot();
        unmeasured.memory.total_bytes = 0;
        let memory = readout("m", MetricKind::Memory, true, TrayStyle::Both);
        let none = reading(&memory, &unmeasured, Lang::En, Units::default());
        assert_eq!(none.label, "\u{2014}");
    }

    #[test]
    fn temperature_is_the_hottest_sensor_in_the_chosen_unit() {
        let mut hot = snapshot();
        hot.temperature.cpu_celsius = Some(48.0);
        hot.temperature.hottest = Some(71.4);
        hot.temperature.history = vec![60.0, 71.4];
        let readout = readout("t", MetricKind::Temperature, true, TrayStyle::Both);
        let celsius = reading(&readout, &hot, Lang::En, Units::default());
        assert_eq!(celsius.label, "71\u{b0}");
        assert_eq!(celsius.tooltip, "Temperature 71\u{b0}C");
        let fahrenheit = Units { temperature: TemperatureUnit::Fahrenheit, ..Units::default() };
        let f = reading(&readout, &hot, Lang::En, fahrenheit);
        assert_eq!(f.label, "161\u{b0}");
        assert_eq!(f.tooltip, "Temperature 161\u{b0}F");
        // The headline used to be cpu_celsius, which is absent on most machines and not the hottest.
        hot.temperature.cpu_celsius = None;
        assert_eq!(reading(&readout, &hot, Lang::En, Units::default()).label, "71\u{b0}");
    }

    #[test]
    fn rates_follow_the_storage_and_network_units() {
        assert_eq!(compact_rate(0.0, Units::default(), false), "0B");
        assert_eq!(compact_rate(512.0, Units::default(), false), "512B");
        assert_eq!(compact_rate(1_234_567.0, Units::default(), true), "1.2M");
        assert_eq!(compact_rate(345_000.0, Units::default(), true), "345K");
        assert_eq!(compact_rate(12_400_000_000.0, Units::default(), false), "12G");
        assert_eq!(compact_rate(999_600.0, Units::default(), false), "1M", "never 1000K");
        assert_eq!(compact_rate(1_000_000.0, Units::default(), false), "1M");

        let binary = Units { storage: StorageUnit::Binary, ..Units::default() };
        assert_eq!(compact_rate(1_048_576.0, binary, false), "1M");
        assert_eq!(compact_rate(1_000.0, binary, false), "1K", "1000 B is 0.98 KiB");
        assert_eq!(compact_rate(1_024.0 * 1_000.0, binary, false), "1M", "1000 KiB is 0.98 MiB, one decimal of which is 1.0");

        let bits = Units { network: NetworkUnit::Bits, ..Units::default() };
        assert_eq!(compact_rate(1_500_000.0, bits, true), "12M", "12 Mbit/s, not 1.5 MB/s");
        assert_eq!(compact_rate(1_500_000.0, bits, false), "1.5M", "a disk rate is never in bits");
    }

    #[test]
    fn tooltips_carry_both_directions_in_the_users_units() {
        let network = read(MetricKind::Network);
        assert_eq!(network.label, "1.2M", "the larger direction");
        assert_eq!(network.tooltip, "Network \u{2193} 1.23\u{a0}MB/s \u{b7} \u{2191} 45.0\u{a0}KB/s");
        let bits = Units { network: NetworkUnit::Bits, ..Units::default() };
        let in_bits = reading(&readout("n", MetricKind::Network, true, TrayStyle::Both), &snapshot(), Lang::En, bits);
        assert_eq!(in_bits.label, "9.9M");
        assert!(in_bits.tooltip.contains("9.88\u{a0}Mbit/s"), "{}", in_bits.tooltip);

        let disk = read(MetricKind::Disk);
        assert_eq!(disk.label, "12M");
        assert_eq!(disk.tooltip, "Disk Reading 12.3\u{a0}MB/s \u{b7} Writing 4.50\u{a0}MB/s");
    }

    #[test]
    fn a_combined_rate_graphs_the_larger_direction_per_sample_with_a_round_ceiling() {
        let disk = read(MetricKind::Disk);
        assert_eq!(disk.series, vec![2.0e6, 12.3e6]);
        assert_eq!(disk.range, (0.0, 20_000_000.0));
        let network = read(MetricKind::Network);
        assert_eq!(network.series, vec![1.0e6, 1_234_567.0]);
        assert_eq!(network.range, (0.0, 2_000_000.0));
        // Histories of different lengths are matched from the newest end.
        assert_eq!(larger_of(&[1.0, 5.0, 2.0], &[3.0, 1.0]), vec![5.0, 2.0]);
    }

    // ---- What reaches the shell. These are the decisions the main thread acts on, so they are
    // ---- tested without a Tauri app.

    fn manager_with(ids: &[&str]) -> TrayManager {
        let mut manager = TrayManager::new();
        manager.live = ids.iter().map(|id| id.to_string()).collect();
        manager
    }

    fn icon(id: &str, tooltip: &str, fill: u8) -> IconFrame {
        IconFrame { id: id.into(), tooltip: tooltip.into(), rgba: vec![fill; 16], width: 2, height: 2 }
    }

    #[test]
    fn a_new_icon_gets_its_tooltip_and_bitmap_and_an_unchanged_frame_costs_nothing() {
        let mut manager = manager_with(&["tray-cpu"]);
        let first = manager.select_writes(vec![icon("tray-cpu", "CPU 16%", 1)]);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].tooltip.as_deref(), Some("CPU 16%"));
        assert_eq!(first[0].image, Some((vec![1; 16], 2, 2)));

        let again = manager.select_writes(vec![icon("tray-cpu", "CPU 16%", 1)]);
        assert!(again.is_empty(), "the shell already shows exactly this: {again:?}");
    }

    #[test]
    fn only_the_part_that_changed_is_written() {
        let mut manager = manager_with(&["tray-cpu"]);
        manager.select_writes(vec![icon("tray-cpu", "CPU 16%", 1)]);

        let tooltip_only = manager.select_writes(vec![icon("tray-cpu", "CPU 17%", 1)]);
        assert_eq!(tooltip_only[0].tooltip.as_deref(), Some("CPU 17%"));
        assert_eq!(tooltip_only[0].image, None);

        let image_only = manager.select_writes(vec![icon("tray-cpu", "CPU 17%", 2)]);
        assert_eq!(image_only[0].tooltip, None);
        assert_eq!(image_only[0].image, Some((vec![2; 16], 2, 2)));
    }

    #[test]
    fn a_frame_for_an_icon_that_does_not_exist_is_dropped() {
        // A readout whose icon could not be created, or one switched off a moment ago, has no icon
        // to draw on, and nothing is remembered for it.
        let mut manager = manager_with(&["tray-cpu"]);
        let writes = manager.select_writes(vec![icon("tray-memory", "Memory 63%", 1), icon("tray-cpu", "CPU", 1)]);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].id, "tray-cpu");
        assert!(!manager.shown.contains_key("tray-memory"));
    }

    #[test]
    fn a_write_the_shell_refused_is_tried_again_on_the_next_tick() {
        let mut manager = manager_with(&["tray-cpu"]);
        manager.select_writes(vec![icon("tray-cpu", "CPU 16%", 1)]);
        manager.rejected("tray-cpu");
        let retry = manager.select_writes(vec![icon("tray-cpu", "CPU 16%", 1)]);
        assert_eq!(retry.len(), 1);
        assert!(retry[0].tooltip.is_some() && retry[0].image.is_some(), "both are written again");
    }

    #[test]
    fn recreated_icons_start_from_nothing_shown() {
        // A new icon carries the application icon and a placeholder tooltip, whatever the old one
        // showed, so a frame equal to the old one still has to be written.
        let mut manager = manager_with(&["tray-cpu", "tray-memory"]);
        manager.select_writes(vec![icon("tray-cpu", "CPU", 1), icon("tray-memory", "Memory", 2)]);
        let retired = manager.retire_all();
        assert_eq!(retired, ["tray-cpu", "tray-memory"], "removed in the order they were listed");
        assert!(manager.live.is_empty() && manager.shown.is_empty());

        manager.live = vec!["tray-cpu".into()];
        let writes = manager.select_writes(vec![icon("tray-cpu", "CPU", 1)]);
        assert_eq!(writes.len(), 1);
    }

    #[test]
    fn the_plan_follows_the_set_the_order_and_the_language_of_the_icons() {
        let live = ["tray-cpu".to_string(), "tray-memory".to_string()];
        let mut manager = TrayManager::new();
        manager.live = live.to_vec();
        manager.lang = Lang::En;

        assert_eq!(manager.plan(&live, Lang::En), Plan::Keep, "nothing changed");
        let reordered = ["tray-memory".to_string(), "tray-cpu".to_string()];
        assert_eq!(manager.plan(&reordered, Lang::En), Plan::Recreate, "windows lists icons in creation order");
        let fewer = ["tray-cpu".to_string()];
        assert_eq!(manager.plan(&fewer, Lang::En), Plan::Recreate);
        assert_eq!(manager.plan(&live, Lang::ZhHans), Plan::Menus, "only the menus carry words");

        // An icon that failed to build is not live, so the next save tries the whole set again.
        manager.live = vec!["tray-cpu".to_string()];
        assert_eq!(manager.plan(&live, Lang::En), Plan::Recreate);
    }

    #[test]
    fn a_readout_listed_twice_is_drawn_once_and_the_fallback_is_never_drawn() {
        let mut twice = settings(vec![
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Both),
            readout("memory", MetricKind::Memory, false, TrayStyle::Value),
            readout("cpu", MetricKind::Cpu, true, TrayStyle::Value),
            readout("network", MetricKind::Network, true, TrayStyle::Graph),
        ]);
        twice.language = crate::settings::Language::En;
        let frames = frames_for(&snapshot(), &twice);
        let ids: Vec<&str> = frames.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["tray-cpu", "tray-network"], "settings order, one frame per icon, none for a disabled readout");
        assert!(frames.iter().all(|f| f.rgba.len() == (f.width * f.height * 4) as usize));
        assert_eq!(frames[0].tooltip, "CPU 16%");

        let none = settings(vec![readout("cpu", MetricKind::Cpu, false, TrayStyle::Both)]);
        assert!(frames_for(&snapshot(), &none).is_empty(), "the fallback icon keeps its own picture");
    }
}
