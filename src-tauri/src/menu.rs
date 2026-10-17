//! Tray menu, and where the panel and the window open.

use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewWindow, Wry,
};
use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

use crate::i18n::{self, Lang, Text};
use crate::ipc::AppState;

pub fn tray_menu(app: &AppHandle, lang: Lang) -> Result<Menu<Wry>, String> {
    let open: MenuItem<Wry> = MenuItem::with_id(app, "open", i18n::text(lang, Text::OpenMeerkat), true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let settings: MenuItem<Wry> =
        MenuItem::with_id(app, "settings", i18n::text(lang, Text::Settings), true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let separator: PredefinedMenuItem<Wry> =
        PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let quit: MenuItem<Wry> =
        MenuItem::with_id(app, "quit", i18n::text(lang, Text::QuitMeerkat), true, None::<&str>)
            .map_err(|e| e.to_string())?;
    Menu::with_items(app, &[&open, &settings, &separator, &quit]).map_err(|e| e.to_string())
}

pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        "open" => show_main(app, None),
        "settings" => show_main(app, Some("settings")),
        "quit" => app.exit(0),
        _ => {}
    }
}

/// Registered once for every tray icon (see `main`), so rebuilding the icons never stacks
/// handlers.
pub fn on_tray_event(app: &AppHandle, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        rect,
        ..
    } = event
    {
        // Pressing the icon moved focus off the panel, which closed it before this release
        // arrived; opening it again here would make the icon impossible to close the panel with.
        if let Some(state) = app.try_state::<AppState>() {
            if is_close_click(state.panel_blurred_at.load(Ordering::Relaxed), now_ms()) {
                return;
            }
        }
        toggle_panel(app, icon_rect(&rect));
    }
}

// ---------------------------------------------------------------------------------------------
// Geometry. Everything below is in physical pixels of the virtual screen, whose origin is the
// primary monitor's top-left corner, so a monitor to its left or above it has negative values.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Rect {
    fn right(self) -> i32 {
        self.x + self.w
    }

    fn bottom(self) -> i32 {
        self.y + self.h
    }

    fn centre(self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// The screen edge the notification area is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Which edge of the monitor the clicked icon belongs to. The distance from the icon's centre to
/// each edge of the WORK area is signed: the taskbar is not part of the work area, so the icon is
/// outside it (a negative distance) on exactly the edge the taskbar sits on, however far along
/// that edge the icon is. With an auto-hiding taskbar the work area is the whole monitor and the
/// nearest edge wins. A tie goes to a horizontal bar, which is what Windows 11 offers.
pub fn taskbar_edge(work: Rect, icon: Rect) -> Edge {
    let (cx, cy) = icon.centre();
    [
        (Edge::Bottom, work.bottom() - cy),
        (Edge::Top, cy - work.y),
        (Edge::Left, cx - work.x),
        (Edge::Right, work.right() - cx),
    ]
    .into_iter()
    .min_by_key(|(_, distance)| *distance)
    .map(|(edge, _)| edge)
    .unwrap_or(Edge::Bottom)
}

/// Where the panel's top-left corner goes. The panel sits against the icon on the side away from
/// its taskbar (above a bottom bar, below a top bar, beside a vertical one), `gap` pixels clear
/// of it, centred on the icon along the bar, and is then pushed whole into the work area on both
/// axes. A panel larger than the work area keeps its top-left corner on screen.
pub fn place_panel(work: Rect, icon: Rect, panel: (i32, i32), gap: i32) -> Point {
    let (width, height) = panel;
    let (cx, cy) = icon.centre();
    let (x, y) = match taskbar_edge(work, icon) {
        Edge::Bottom => (cx - width / 2, icon.y - gap - height),
        Edge::Top => (cx - width / 2, icon.bottom() + gap),
        Edge::Left => (icon.right() + gap, cy - height / 2),
        Edge::Right => (icon.x - gap - width, cy - height / 2),
    };
    Point {
        x: x.min(work.right() - width).max(work.x),
        y: y.min(work.bottom() - height).max(work.y),
    }
}

/// What the panel was last opened against, kept so that a resize while it is open keeps the
/// edge next to the icon where it is.
#[derive(Clone, Copy, Debug)]
pub struct PanelAnchor {
    pub icon: Rect,
    pub work: Rect,
    pub scale: f64,
}

impl PanelAnchor {
    /// About 8 logical pixels of air between the icon and the panel.
    fn gap(&self) -> i32 {
        (8.0 * self.scale).round() as i32
    }

    /// The panel's whole rectangle for a requested size in logical pixels. The height is clamped
    /// to the work area: the webview asks for its own content height, and a short screen would
    /// otherwise push the tab strip off the bottom edge.
    pub fn panel_rect(&self, logical: (u32, u32)) -> Rect {
        let w = ((logical.0 as f64 * self.scale).round() as i32).max(1);
        let h = ((logical.1 as f64 * self.scale).round() as i32).max(1).min(self.work.h);
        let at = place_panel(self.work, self.icon, (w, h), self.gap());
        Rect { x: at.x, y: at.y, w, h }
    }
}

/// How long after a focus loss a left-click release still counts as the click that caused it.
const CLOSE_CLICK_MS: u64 = 300;

/// Milliseconds on a monotonic clock, never 0 (0 means "never" in the atomics that store it).
pub fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

/// `blurred_at` is when the panel was last hidden by losing focus (0 = never).
pub fn is_close_click(blurred_at: u64, now: u64) -> bool {
    blurred_at != 0 && now.saturating_sub(blurred_at) < CLOSE_CLICK_MS
}

/// Called when the panel was hidden because it lost focus.
pub fn note_panel_blur(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.panel_blurred_at.store(now_ms(), Ordering::Relaxed);
    }
}

fn icon_rect(rect: &tauri::Rect) -> Rect {
    let (x, y) = match rect.position {
        tauri::Position::Physical(p) => (p.x, p.y),
        tauri::Position::Logical(p) => (p.x.round() as i32, p.y.round() as i32),
    };
    let (w, h) = match rect.size {
        tauri::Size::Physical(s) => (s.width as i32, s.height as i32),
        tauri::Size::Logical(s) => (s.width.round() as i32, s.height.round() as i32),
    };
    Rect { x, y, w: w.max(1), h: h.max(1) }
}

fn rect_of(monitor: &Monitor) -> Rect {
    let area = monitor.work_area();
    Rect {
        x: area.position.x,
        y: area.position.y,
        w: area.size.width as i32,
        h: area.size.height as i32,
    }
}

/// The work area and scale factor of the monitor the icon is on. A point outside every monitor
/// (an icon the shell has just moved) falls back to the nearest one.
fn monitor_of(app: &AppHandle, icon: Rect) -> Option<(Rect, f64)> {
    let (cx, cy) = icon.centre();
    let monitor = app
        .monitor_from_point(cx as f64, cy as f64)
        .ok()
        .flatten()
        .or_else(|| {
            let monitors = app.available_monitors().ok()?;
            monitors.into_iter().min_by_key(|m| {
                let position = m.position();
                let size = m.size();
                let dx = (position.x - cx).max(cx - (position.x + size.width as i32)).max(0) as i64;
                let dy = (position.y - cy).max(cy - (position.y + size.height as i32)).max(0) as i64;
                dx * dx + dy * dy
            })
        })
        .or_else(|| app.primary_monitor().ok().flatten())?;
    Some((rect_of(&monitor), monitor.scale_factor()))
}

/// The panel's rectangle as the window system has it now.
fn current_rect(panel: &WebviewWindow) -> Option<Rect> {
    let position = panel.outer_position().ok()?;
    let size = panel.outer_size().ok()?;
    Some(Rect { x: position.x, y: position.y, w: size.width as i32, h: size.height as i32 })
}

/// Moves and resizes the panel in a single call, so a panel that grows upward never has a frame
/// at the old size in the new place. Moving a window between monitors of different density can
/// make Windows rescale it as it lands, so the result is read back and applied once more.
fn apply_rect(panel: &WebviewWindow, rect: Rect) {
    let Ok(hwnd) = panel.hwnd() else {
        let _ = panel.set_size(PhysicalSize::new(rect.w.max(1) as u32, rect.h.max(1) as u32));
        let _ = panel.set_position(PhysicalPosition::new(rect.x, rect.y));
        return;
    };
    for _ in 0..2 {
        let _ = unsafe { SetWindowPos(hwnd, None, rect.x, rect.y, rect.w, rect.h, SWP_NOZORDER | SWP_NOACTIVATE) };
        if current_rect(panel) == Some(rect) {
            break;
        }
    }
}

/// Opens the panel next to the icon that was clicked, or closes it if it is open.
fn toggle_panel(app: &AppHandle, icon: Rect) {
    let Some(panel) = app.get_webview_window("panel") else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        let _ = panel.hide();
        crate::ipc::refresh_visibility(app);
        return;
    }

    let (work, scale) = monitor_of(app, icon).unwrap_or_else(|| {
        // No monitor information at all: behave as if the bar were at the bottom of a screen that
        // reaches as far as it needs to, so the panel still opens beside the icon.
        let work = Rect { x: icon.x - 10_000, y: icon.y - 10_000, w: 20_000, h: 10_000 + icon.h };
        (work, 1.0)
    });
    let anchor = PanelAnchor { icon, work, scale };

    let logical = app
        .try_state::<AppState>()
        .and_then(|state| {
            *state.panel_anchor.lock().unwrap_or_else(|p| p.into_inner()) = Some(anchor);
            *state.panel_size.lock().unwrap_or_else(|p| p.into_inner())
        })
        .or_else(|| {
            let size = panel.outer_size().ok()?;
            Some(((size.width as f64 / scale).round() as u32, (size.height as f64 / scale).round() as u32))
        })
        .unwrap_or((400, 580));
    apply_rect(&panel, anchor.panel_rect(logical));

    let _ = panel.show();
    let _ = panel.set_focus();
    let _ = app.emit_to("panel", "panel-shown", ());
    crate::ipc::refresh_visibility(app);
}

/// The webview measures its content after it is shown and on every update, and asks for a size
/// in logical pixels. While the panel is open next to an icon the new rectangle is computed from
/// the remembered anchor, so the edge beside the icon stays put and the panel grows away from it.
pub fn resize_panel(app: &AppHandle, state: &AppState, width: u32, height: u32) -> Result<(), String> {
    let panel = app.get_webview_window("panel").ok_or("no panel")?;
    let anchor = *state.panel_anchor.lock().unwrap_or_else(|p| p.into_inner());
    match anchor {
        Some(anchor) if panel.is_visible().unwrap_or(false) => {
            let rect = anchor.panel_rect((width, height));
            if current_rect(&panel) != Some(rect) {
                apply_rect(&panel, rect);
            }
            Ok(())
        }
        _ => panel
            .set_size(LogicalSize::new(width as f64, height as f64))
            .map_err(|e| e.to_string()),
    }
}

/// Brings the main window up, optionally on one of its pages. Every way in (the tray menu, a
/// second launch, a toast) comes through here; "window-shown" is not sent from here but by
/// `refresh_visibility`, which sees the window go from hidden or minimised to shown no matter
/// who showed it, so un-minimising from the taskbar is covered too.
pub fn show_main(app: &AppHandle, destination: Option<&str>) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(destination) = destination {
            let _ = app.emit_to("main", "navigate", destination);
        }
    }
    crate::ipc::refresh_visibility(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    // The reported machine: a 2560x1440 primary monitor at 125%, so the 48 px (logical) taskbar
    // is 60 physical pixels and the work area ends at 1380. The panel is about 440x692 physical.
    const PRIMARY_WORK: Rect = rect(0, 0, 2560, 1380);
    const PANEL: (i32, i32) = (440, 692);
    const GAP: i32 = 10;

    #[test]
    fn a_bottom_taskbar_opens_the_panel_above_the_icon_not_below_the_work_area() {
        // The old code put the top at cursor.y + icon height, which on this machine left the panel
        // 6 px above the bottom of the screen, under the taskbar.
        let icon = rect(2300, 1392, 24, 24);
        assert_eq!(taskbar_edge(PRIMARY_WORK, icon), Edge::Bottom);
        let at = place_panel(PRIMARY_WORK, icon, PANEL, GAP);
        assert_eq!(at, Point { x: 2312 - 220, y: 1380 - 692 }, "bottom edge flush with the work area");
        assert!(at.y + PANEL.1 <= PRIMARY_WORK.bottom());
    }

    #[test]
    fn an_icon_in_a_flyout_above_the_bar_keeps_the_panel_attached_to_the_icon() {
        let icon = rect(2200, 1100, 24, 24);
        let at = place_panel(PRIMARY_WORK, icon, PANEL, GAP);
        assert_eq!(at.y + PANEL.1, icon.y - GAP, "the panel's bottom is just above the icon");
        assert_eq!(at.x, 2212 - 220);
    }

    #[test]
    fn a_top_taskbar_opens_the_panel_below_the_icon() {
        let work = rect(0, 60, 2560, 1380);
        let icon = rect(2300, 18, 24, 24);
        assert_eq!(taskbar_edge(work, icon), Edge::Top);
        let at = place_panel(work, icon, PANEL, GAP);
        assert_eq!(at, Point { x: 2092, y: 60 }, "pushed down to the work area, never under the bar");
        let flyout = rect(2300, 200, 24, 24);
        assert_eq!(place_panel(work, flyout, PANEL, GAP).y, 200 + 24 + GAP);
    }

    #[test]
    fn a_left_taskbar_opens_the_panel_to_the_right_of_the_icon() {
        let work = rect(60, 0, 2500, 1440);
        let icon = rect(18, 1380, 24, 24);
        assert_eq!(taskbar_edge(work, icon), Edge::Left);
        let at = place_panel(work, icon, PANEL, GAP);
        assert_eq!(at.x, 60, "beside the bar, inside the work area");
        assert_eq!(at.y + PANEL.1, 1440, "an icon at the bottom of the bar pushes the panel up to fit");
    }

    #[test]
    fn a_right_taskbar_opens_the_panel_to_the_left_of_the_icon() {
        let work = rect(0, 0, 2500, 1440);
        let icon = rect(2518, 1380, 24, 24);
        assert_eq!(taskbar_edge(work, icon), Edge::Right);
        let at = place_panel(work, icon, PANEL, GAP);
        assert_eq!(at.x + PANEL.0, 2500, "flush with the work area's right edge");
        assert_eq!(at.y + PANEL.1, 1440);
    }

    #[test]
    fn a_monitor_left_of_the_primary_has_negative_coordinates_and_is_clamped_to_itself() {
        // 1920x1080 at 100% to the left of the primary, taskbar 48 px at its bottom.
        let work = rect(-1920, 0, 1920, 1032);
        let icon = rect(-60, 1044, 20, 20);
        assert_eq!(taskbar_edge(work, icon), Edge::Bottom);
        let panel = (400, 600);
        let at = place_panel(work, icon, panel, 8);
        assert_eq!(at.x + panel.0, 0, "clamped against this monitor's right edge, not the primary's");
        assert_eq!(at.y + panel.1, 1032);
        assert!(at.x >= -1920);

        let far_left = rect(-1900, 1044, 20, 20);
        assert_eq!(place_panel(work, far_left, panel, 8).x, -1920);
    }

    #[test]
    fn a_panel_taller_than_the_work_area_keeps_its_top_edge_on_screen() {
        let icon = rect(2300, 1392, 24, 24);
        let at = place_panel(PRIMARY_WORK, icon, (440, 1500), GAP);
        assert_eq!(at.y, 0);
        let wide = place_panel(PRIMARY_WORK, icon, (3000, 600), GAP);
        assert_eq!(wide.x, 0);
    }

    #[test]
    fn an_icon_at_the_far_right_edge_still_gets_a_whole_panel() {
        let icon = rect(2536, 1392, 24, 24);
        // 24 px from the right edge and 12 below the work area: the bar is the bottom one.
        assert_eq!(taskbar_edge(PRIMARY_WORK, icon), Edge::Bottom);
        let at = place_panel(PRIMARY_WORK, icon, PANEL, GAP);
        assert_eq!(at.x, 2560 - 440);
    }

    #[test]
    fn an_auto_hiding_taskbar_is_told_by_the_nearest_edge() {
        // The work area is the whole monitor; the icon sits in the bar that has slid in.
        let work = rect(0, 0, 2560, 1440);
        assert_eq!(taskbar_edge(work, rect(2300, 1410, 24, 24)), Edge::Bottom);
        assert_eq!(taskbar_edge(work, rect(2300, 6, 24, 24)), Edge::Top);
        assert_eq!(taskbar_edge(work, rect(6, 700, 24, 24)), Edge::Left);
        assert_eq!(taskbar_edge(work, rect(2530, 700, 24, 24)), Edge::Right);
    }

    // The webview asks for its size after it is shown. An upward-opened panel must grow upward:
    // the bottom edge, beside the icon, stays where it was.
    #[test]
    fn a_resize_keeps_the_edge_next_to_the_icon_attached() {
        let anchor = PanelAnchor { icon: rect(2300, 1392, 24, 24), work: PRIMARY_WORK, scale: 1.25 };
        let small = anchor.panel_rect((352, 400));
        let tall = anchor.panel_rect((352, 554));
        assert_eq!(small.h, 500);
        assert_eq!(tall.h, 693); // 554 * 1.25 = 692.5, rounded
        assert_eq!(small.y + small.h, PRIMARY_WORK.bottom());
        assert_eq!(tall.y + tall.h, PRIMARY_WORK.bottom());
        assert_eq!(small.x, tall.x);
        assert_eq!(small.w, 440);

        let top = PanelAnchor { icon: rect(2300, 18, 24, 24), work: rect(0, 60, 2560, 1380), scale: 1.25 };
        assert_eq!(top.panel_rect((352, 400)).y, 60);
        assert_eq!(top.panel_rect((352, 554)).y, 60, "a panel below a top bar grows downward");
    }

    // The webview sizes the window to its own content, which a short work area cannot hold; the
    // height is capped so the tab strip stays on screen.
    #[test]
    fn a_panel_rect_never_exceeds_the_work_area() {
        // A 1366x768 laptop at 125%: 582 logical pixels of work area is 728 physical.
        let anchor = PanelAnchor { icon: rect(2300, 740, 24, 24), work: rect(0, 0, 2560, 728), scale: 1.25 };
        let full = anchor.panel_rect((400, 760));
        assert_eq!(full.h, 728, "clamped to the work area, not the 950 physical pixels requested");
        assert_eq!(full.y + full.h, 728, "still placed whole inside it");
    }

    #[test]
    fn the_gap_scales_with_the_monitor() {
        let anchor = |scale| PanelAnchor { icon: rect(1000, 1000, 24, 24), work: rect(0, 0, 2000, 1000), scale };
        assert_eq!(anchor(1.0).gap(), 8);
        assert_eq!(anchor(1.25).gap(), 10);
        assert_eq!(anchor(2.0).gap(), 16);
    }

    // Pressing the icon while the panel is open takes focus (which hides the panel) before the
    // release arrives. That release must not open it again.
    #[test]
    fn a_release_just_after_the_panel_lost_focus_is_the_click_that_closed_it() {
        assert!(!is_close_click(0, 10_000), "never blurred");
        assert!(is_close_click(5_000, 5_010));
        assert!(is_close_click(5_000, 5_299));
        assert!(!is_close_click(5_000, 5_300), "a later click is a new click");
        assert!(!is_close_click(5_000, 60_000));
        assert!(is_close_click(5_000, 4_000), "a clock that did not advance is still the same click");
    }
}
