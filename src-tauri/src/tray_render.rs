//! Draws a tray readout into a square RGBA icon, on the sampler thread.
//!
//! The shell draws a notification icon into a square cell (SM_CXSMICON at the taskbar's DPI)
//! and rescales anything else, so the icon is exactly that square. It is built from coverage
//! masks, never from pixels of a colour:
//!
//! * text is drawn with GDI as WHITE ink on a black DIB, in grayscale anti-aliasing, so a
//!   pixel's brightness *is* the glyph's coverage there;
//! * the sparkline is rasterised by a small anti-aliased line renderer, no GDI involved;
//! * the layers are composited "over" a transparent canvas into straight-alpha RGBA, whose colour
//!   is the ink chosen for the taskbar's theme and whose alpha is the coverage.
//!
//! Deriving alpha from the brightness of coloured text, as this file used to, made dark ink on a
//! light taskbar 12 % opaque and the graph pen about 40 %.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, ERROR_SUCCESS};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GdiFlush, GetDC,
    GetTextMetricsW, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW, ANTIALIASED_QUALITY,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, HBITMAP,
    HDC, HGDIOBJ, OUT_TT_PRECIS, TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SM_CXSMICON};

/// Text on a dark bar, and on a light one. Not pure white and black: both vibrate on a bar.
const TEXT_ON_DARK: (u8, u8, u8) = (0xF2, 0xF4, 0xF8);
const TEXT_ON_LIGHT: (u8, u8, u8) = (0x1B, 0x1E, 0x24);

/// The smallest font, in pixels of em height, a label may be drawn in. Below it the digits blur
/// into each other and the sparkline is the better readout.
const MIN_EM: i32 = 9;
/// A label that is a little too wide is squeezed horizontally, down to this share of its width.
const MIN_CONDENSE: f32 = 0.75;
/// The area under the line is the same colour at this opacity.
const FILL_ALPHA: f32 = 0.30;
const LINE_WIDTH: f32 = 1.5;
/// One minute of one-second samples spans the sparkline's whole width.
const WINDOW_SAMPLES: usize = 60;
/// Ink fainter than this is anti-aliasing fringe and does not count when measuring a glyph.
const INK_THRESHOLD: u8 = 40;

pub struct Rendered {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Value,
    Graph,
    Both,
}

pub struct Frame<'a> {
    /// What is written on the icon: "16", "1.2M", "62°", "—". Empty draws no text.
    pub label: &'a str,
    /// Oldest first. Fewer than two usable points means there is nothing to draw as a line.
    pub series: &'a [f64],
    /// The values that sit on the bottom and the top of the sparkline.
    pub range: (f64, f64),
    pub hue: (u8, u8, u8),
    pub layout: Layout,
    /// The taskbar is light, so ink is dark and the pen is darkened.
    pub light_bar: bool,
    /// Side of the square, in pixels.
    pub size: i32,
}

// ---------------------------------------------------------------------------------------------
// What the shell asked for
// ---------------------------------------------------------------------------------------------

/// The side of the small-icon cell at the taskbar's DPI, which is the size the shell draws a
/// notification icon at; any other size is rescaled and comes out soft.
pub fn icon_size() -> i32 {
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, taskbar_dpi()) };
    size.max(16)
}

fn taskbar_dpi() -> u32 {
    unsafe {
        let class = wide("Shell_TrayWnd");
        if let Ok(taskbar) = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) {
            let dpi = GetDpiForWindow(taskbar);
            if dpi > 0 {
                return dpi;
            }
        }
        match GetDpiForSystem() {
            0 => 96,
            dpi => dpi,
        }
    }
}

const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// Whether the taskbar is light. That is the Windows SYSTEM theme (the apps theme is another
/// value, and Meerkat's own theme setting is neither). A missing value, which is every Windows
/// before 1903, means dark. Read at most every 5 seconds.
pub fn taskbar_is_light() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, light)) = *cache {
        if at.elapsed() < Duration::from_secs(5) {
            return light;
        }
    }
    let light = read_dword(PERSONALIZE_KEY, "SystemUsesLightTheme") == Some(1);
    *cache = Some((Instant::now(), light));
    light
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_dword(key: &str, name: &str) -> Option<u32> {
    let key_w = wide(key);
    let name_w = wide(name);
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            PCWSTR(name_w.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast::<c_void>()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

// ---------------------------------------------------------------------------------------------
// Scale
// ---------------------------------------------------------------------------------------------

/// The ceiling of a rate's sparkline: the smallest 1, 2, 5 or 10 times a power of ten that is at
/// least the largest sample, and never below `floor`, so that a quiet line stays quiet instead
/// of being stretched to look busy.
pub fn nice_ceiling(max: f64, floor: f64) -> f64 {
    let max = if max.is_finite() { max.max(floor) } else { floor };
    let base = 10f64.powf(max.log10().floor());
    [1.0, 2.0, 5.0]
        .into_iter()
        .map(|step| step * base)
        .find(|ceiling| *ceiling >= max)
        .unwrap_or(10.0 * base)
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

/// A rectangle of the icon, in pixels.
#[derive(Clone, Copy, Debug)]
struct Area {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

pub fn render(frame: &Frame) -> Rendered {
    let size = frame.size.clamp(16, 64);
    let side = size as usize;
    let mut text_layer = vec![0u8; side * side];
    let mut line_layer = vec![0u8; side * side];
    let mut fill_layer = vec![0u8; side * side];

    let label = frame.label.trim();
    let has_series = frame.series.iter().filter(|v| v.is_finite()).count() >= 2;

    let full = Area { x: 0, y: 0, w: size, h: size };
    let upper_h = (size as f32 * 0.6).round() as i32;
    let upper = Area { x: 0, y: 0, w: size, h: upper_h };
    let lower = Area { x: 0, y: upper_h, w: size, h: size - upper_h };

    let (text_area, mut graph_area) = match frame.layout {
        Layout::Value => (Some(full), None),
        Layout::Graph => (None, has_series.then_some(full)),
        Layout::Both if has_series => (Some(upper), Some(lower)),
        Layout::Both => (Some(full), None),
    };

    let mut canvas = if label.is_empty() { None } else { Canvas::new(size * 5, size * 3) };
    let mut drawn = false;
    if let (Some(area), Some(canvas)) = (text_area, canvas.as_mut()) {
        drawn = draw_label(canvas, label, area, MIN_EM, &mut text_layer, size);
        // A label that cannot be read at this size gives way to the graph, which gets the whole
        // square; the tooltip still carries the figure.
        if !drawn && has_series {
            graph_area = Some(full);
        }
    }
    if !drawn && graph_area.is_none() {
        // Nothing to graph and nothing that fit: say what there is, as small as it has to be.
        if let Some(canvas) = canvas.as_mut() {
            draw_label(canvas, label, full, 6, &mut text_layer, size);
        }
    }

    if let Some(area) = graph_area {
        sparkline(&mut line_layer, &mut fill_layer, size, area, frame.series, frame.range);
    }

    let ink = if frame.light_bar { TEXT_ON_LIGHT } else { TEXT_ON_DARK };
    let pen = if frame.light_bar { darken(frame.hue, 0.58) } else { frame.hue };
    Rendered {
        rgba: composite(side, &text_layer, &line_layer, &fill_layer, ink, pen),
        width: size as u32,
        height: size as u32,
    }
}

fn darken((r, g, b): (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let scale = |v: u8| (v as f32 * factor).round().clamp(0.0, 255.0) as u8;
    (scale(r), scale(g), scale(b))
}

/// Layers over a transparent canvas into straight alpha: the line over its fill (one colour),
/// then the text over that.
fn composite(side: usize, text: &[u8], line: &[u8], fill: &[u8], ink: (u8, u8, u8), pen: (u8, u8, u8)) -> Vec<u8> {
    let mut rgba = vec![0u8; side * side * 4];
    for i in 0..side * side {
        let a_fill = fill[i] as f32 / 255.0 * FILL_ALPHA;
        let a_line = line[i] as f32 / 255.0;
        let a_text = text[i] as f32 / 255.0;
        let a_graph = a_line + a_fill * (1.0 - a_line);
        let alpha = a_text + a_graph * (1.0 - a_text);
        if alpha <= 0.0 {
            continue;
        }
        let w_text = a_text / alpha;
        let w_graph = a_graph * (1.0 - a_text) / alpha;
        let channel = |ink: u8, pen: u8| (ink as f32 * w_text + pen as f32 * w_graph).round() as u8;
        rgba[i * 4] = channel(ink.0, pen.0);
        rgba[i * 4 + 1] = channel(ink.1, pen.1);
        rgba[i * 4 + 2] = channel(ink.2, pen.2);
        rgba[i * 4 + 3] = (alpha * 255.0).round() as u8;
    }
    rgba
}

// ---- text ----------------------------------------------------------------------------------

/// A black 32-bit DIB with a device context, for drawing white text into.
struct Canvas {
    screen: HDC,
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u8,
    width: i32,
    height: i32,
}

impl Canvas {
    fn new(width: i32, height: i32) -> Option<Self> {
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return None;
            }
            let dc = CreateCompatibleDC(Some(screen));
            if dc.is_invalid() {
                ReleaseDC(None, screen);
                return None;
            }
            let header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &header, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bits.is_null() => bitmap,
                _ => {
                    let _ = DeleteDC(dc);
                    ReleaseDC(None, screen);
                    return None;
                }
            };
            let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            Some(Self { screen, dc, bitmap, previous, pixels: bits.cast::<u8>(), width, height })
        }
    }

    /// Clears the canvas and writes `text` in white at `em` pixels, bold, in grayscale
    /// anti-aliasing (ClearType's colour fringes would survive as coloured edges once
    /// brightness becomes alpha). `width` is the average character width, 0 for the face's own.
    /// Returns the face's natural average character width at that size.
    fn draw(&mut self, text: &str, em: i32, width: i32) -> i32 {
        unsafe {
            std::ptr::write_bytes(self.pixels, 0, (self.width * self.height * 4) as usize);
            let face = wide("Segoe UI");
            let font = CreateFontW(
                -em,
                width,
                0,
                0,
                700,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                0,
                PCWSTR(face.as_ptr()),
            );
            let previous_font = SelectObject(self.dc, HGDIOBJ(font.0));
            let mut metrics = TEXTMETRICW::default();
            let _ = GetTextMetricsW(self.dc, &mut metrics);
            let chars: Vec<u16> = text.encode_utf16().collect();
            let _ = TextOutW(self.dc, 2, 2, &chars);
            SelectObject(self.dc, previous_font);
            let _ = DeleteObject(HGDIOBJ(font.0));
            let _ = GdiFlush();
            metrics.tmAveCharWidth
        }
    }

    /// Coverage of one pixel. The ink is white on black, so any channel is the coverage.
    fn coverage(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        let at = ((y * self.width + x) * 4) as usize;
        let channel = |offset: usize| unsafe { *self.pixels.add(at + offset) };
        channel(0).max(channel(1)).max(channel(2))
    }

    /// The box around everything drawn: (left, top, right, bottom), right and bottom exclusive.
    fn ink(&self) -> Option<(i32, i32, i32, i32)> {
        let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.coverage(x, y) >= INK_THRESHOLD {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x + 1);
                    bottom = bottom.max(y + 1);
                }
            }
        }
        (right > left).then_some((left, top, right, bottom))
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.dc);
            ReleaseDC(None, self.screen);
        }
    }
}

/// The largest font in which `text` fits `area` (searched between `min_em` and what the area's
/// height allows), or the smallest one squeezed sideways if that is enough. The glyphs are left
/// on the canvas; the returned box is their ink.
fn fit_text(canvas: &mut Canvas, text: &str, area: Area, min_em: i32) -> Option<(i32, i32, i32, i32)> {
    let max_w = (area.w - 2).max(1);
    let max_h = (area.h - 2).max(1);

    fn measure(canvas: &mut Canvas, text: &str, em: i32, width: i32, max_w: i32, max_h: i32) -> Option<(i32, i32, i32, i32)> {
        canvas.draw(text, em, width);
        let ink = canvas.ink()?;
        ((ink.2 - ink.0) <= max_w && (ink.3 - ink.1) <= max_h).then_some(ink)
    }

    // Digits are about 0.7 em tall, so no font taller than this could fit the area.
    let cap = ((max_h as f32 / 0.6).ceil() as i32).clamp(min_em, 64);

    if let Some(mut ink) = measure(canvas, text, min_em, 0, max_w, max_h) {
        let (mut lo, mut hi) = (min_em, cap);
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            match measure(canvas, text, mid, 0, max_w, max_h) {
                Some(found) => {
                    lo = mid;
                    ink = found;
                }
                None => hi = mid - 1,
            }
        }
        canvas.draw(text, lo, 0);
        return canvas.ink().or(Some(ink));
    }

    // Too wide even at the smallest size: squeeze it, if a little is enough.
    let average = canvas.draw(text, min_em, 0);
    let natural = canvas.ink()?;
    let factor = max_w as f32 / (natural.2 - natural.0) as f32;
    if factor < MIN_CONDENSE || average <= 0 {
        return None;
    }
    let width = ((average as f32 * factor).floor() as i32).max(1);
    measure(canvas, text, min_em, width, max_w, max_h)
}

/// Draws `text` centred (by its ink, not by its line box) in `area` of `layer`.
fn draw_label(canvas: &mut Canvas, text: &str, area: Area, min_em: i32, layer: &mut [u8], side: i32) -> bool {
    let Some((left, top, right, bottom)) = fit_text(canvas, text, area, min_em) else {
        return false;
    };
    let dx = area.x + (area.w - (right - left)) / 2 - left;
    let dy = area.y + (area.h - (bottom - top)) / 2 - top;
    // One pixel more on each side, to carry the anti-aliasing fringe along.
    for y in (top - 1)..(bottom + 1) {
        for x in (left - 1)..(right + 1) {
            let (tx, ty) = (x + dx, y + dy);
            if tx < 0 || ty < 0 || tx >= side || ty >= side {
                continue;
            }
            let value = canvas.coverage(x, y);
            let at = (ty * side + tx) as usize;
            layer[at] = layer[at].max(value);
        }
    }
    true
}

// ---- sparkline -----------------------------------------------------------------------------

/// The last minute of samples as one value per pixel column, newest on the right. A column with
/// no sample behind it (the history is still filling) is `None`: the line starts where the data
/// does instead of being stretched across the icon.
fn resample(values: &[f64], columns: usize) -> Vec<Option<f64>> {
    let kept = values.len().min(WINDOW_SAMPLES);
    let tail = &values[values.len() - kept..];
    let first_slot = WINDOW_SAMPLES - kept;
    (0..columns)
        .map(|column| {
            let from = column * WINDOW_SAMPLES / columns;
            let to = ((column + 1) * WINDOW_SAMPLES).div_ceil(columns).min(WINDOW_SAMPLES);
            let from = from.max(first_slot);
            if from >= to {
                return None;
            }
            let usable: Vec<f64> = tail[from - first_slot..to - first_slot]
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .collect();
            (!usable.is_empty()).then(|| usable.iter().sum::<f64>() / usable.len() as f64)
        })
        .collect()
}

fn distance_to_segment(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared <= f32::EPSILON {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// An anti-aliased polyline with the area under it, inside `area` and one pixel clear of its
/// edges. Coverage goes into `line` and `fill`.
fn sparkline(line: &mut [u8], fill: &mut [u8], side: i32, area: Area, series: &[f64], range: (f64, f64)) {
    let (inner_x, inner_y) = (area.x + 1, area.y + 1);
    let (inner_w, inner_h) = (area.w - 2, area.h - 2);
    if inner_w < 2 || inner_h < 2 {
        return;
    }
    let (low, high) = range;
    let span = (high - low).max(f64::EPSILON);
    let top = inner_y as f32 + 0.5;
    let bottom = (inner_y + inner_h) as f32 - 0.5;

    let columns = resample(series, inner_w as usize);
    let points: Vec<(f32, f32)> = columns
        .iter()
        .enumerate()
        .filter_map(|(column, value)| {
            let value = (*value)?;
            let level = ((value - low) / span).clamp(0.0, 1.0) as f32;
            Some((inner_x as f32 + column as f32 + 0.5, bottom - level * (bottom - top)))
        })
        .collect();
    if points.is_empty() {
        return;
    }

    let rows = area.y.max(0)..(area.y + area.h).min(side);
    // The line may reach a pixel beyond the plot on each side.
    for y in rows.clone() {
        for x in (inner_x - 1).max(0)..(inner_x + inner_w + 1).min(side) {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let distance = if points.len() == 1 {
                distance_to_segment(cx, cy, points[0], points[0])
            } else {
                points
                    .windows(2)
                    .map(|pair| distance_to_segment(cx, cy, pair[0], pair[1]))
                    .fold(f32::MAX, f32::min)
            };
            let coverage = (LINE_WIDTH * 0.5 + 0.5 - distance).clamp(0.0, 1.0);
            line[(y * side + x) as usize] = (coverage * 255.0).round() as u8;
        }
    }

    // The fill: for every column that has a sample, everything below the line's centre down to
    // the plot's bottom edge, with the row the line crosses partly covered.
    let plot_bottom = inner_y + inner_h;
    for (x, y_line) in &points {
        let column = x.floor() as i32;
        if column < 0 || column >= side {
            continue;
        }
        for y in inner_y.max(0)..plot_bottom.min(side) {
            let coverage = ((y + 1) as f32 - y_line).clamp(0.0, 1.0);
            if coverage > 0.0 {
                fill[(y * side + column) as usize] = (coverage * 255.0).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{RegDeleteTreeW, RegSetKeyValueW, REG_DWORD};

    const GREEN: (u8, u8, u8) = (57, 135, 229);

    fn frame<'a>(label: &'a str, series: &'a [f64], layout: Layout, light_bar: bool, size: i32) -> Frame<'a> {
        Frame { label, series, range: (0.0, 100.0), hue: GREEN, layout, light_bar, size }
    }

    fn alpha(image: &Rendered, x: u32, y: u32) -> u8 {
        image.rgba[((y * image.width + x) * 4 + 3) as usize]
    }

    fn pixel(image: &Rendered, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        [image.rgba[at], image.rgba[at + 1], image.rgba[at + 2], image.rgba[at + 3]]
    }

    fn opaque_pixels(image: &Rendered, rows: std::ops::Range<u32>) -> usize {
        rows.flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .filter(|(x, y)| alpha(image, *x, *y) > 0)
            .count()
    }

    #[test]
    fn the_icon_is_square_at_every_size_the_shell_asks_for() {
        for size in [16, 20, 24, 32, 48] {
            for layout in [Layout::Value, Layout::Graph, Layout::Both] {
                let series: Vec<f64> = (0..60).map(|i| (i * 3 % 100) as f64).collect();
                let image = render(&frame("16", &series, layout, false, size));
                assert_eq!((image.width, image.height), (size as u32, size as u32));
                assert_eq!(image.rgba.len(), (size * size * 4) as usize);
            }
        }
        assert_eq!(render(&frame("16", &[], Layout::Value, false, 5)).width, 16, "never below the 16 px cell");
    }

    #[test]
    fn nothing_to_draw_is_fully_transparent() {
        for light in [false, true] {
            let image = render(&frame("", &[], Layout::Both, light, 20));
            assert!(image.rgba.iter().all(|byte| *byte == 0), "no label, no series: no pixel");
            let one = render(&frame("", &[42.0], Layout::Graph, light, 20));
            assert!(one.rgba.iter().all(|byte| *byte == 0), "one sample is not a line");
        }
    }

    // The old renderer took alpha from max(r, g, b): ink #181A1E over black came out 12 % opaque
    // and the light bar's text was close to invisible.
    #[test]
    fn a_glyph_is_opaque_in_the_ink_of_the_taskbar_it_is_drawn_for() {
        for (light, ink) in [(false, TEXT_ON_DARK), (true, TEXT_ON_LIGHT)] {
            let image = render(&frame("88", &[], Layout::Value, light, 20));
            let strongest = image.rgba.chunks(4).map(|p| p[3]).max().unwrap();
            assert!(strongest >= 240, "light={light}: strongest alpha {strongest}");
            let solid = image.rgba.chunks(4).find(|p| p[3] >= 240).unwrap();
            assert_eq!((solid[0], solid[1], solid[2]), ink, "light={light}");
            let covered = image.rgba.chunks(4).filter(|p| p[3] >= 128).count();
            assert!(covered >= 30, "light={light}: only {covered} well-covered pixels for two digits");
        }
    }

    #[test]
    fn text_is_centred_by_its_ink() {
        let image = render(&frame("88", &[], Layout::Value, false, 24));
        let xs: Vec<u32> = (0..24).flat_map(|y| (0..24).map(move |x| (x, y))).filter(|(x, y)| alpha(&image, *x, *y) > 40).map(|(x, _)| x).collect();
        let ys: Vec<u32> = (0..24).flat_map(|y| (0..24).map(move |x| (x, y))).filter(|(x, y)| alpha(&image, *x, *y) > 40).map(|(_, y)| y).collect();
        let (left, right) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
        let (top, bottom) = (*ys.iter().min().unwrap(), *ys.iter().max().unwrap());
        assert!((left as i32 - (23 - right as i32)).abs() <= 2, "horizontally centred: {left}..{right}");
        assert!((top as i32 - (23 - bottom as i32)).abs() <= 2, "vertically centred: {top}..{bottom}");
    }

    #[test]
    fn a_rising_series_ends_high_on_the_right_and_an_idle_one_lies_on_the_floor() {
        let rising: Vec<f64> = (0..60).map(|i| i as f64 * 100.0 / 59.0).collect();
        let image = render(&frame("", &rising, Layout::Graph, false, 24));
        // The right-hand column reaches the top rows, the left-hand one only the bottom.
        let right_top = (0..8).map(|y| alpha(&image, 21, y)).max().unwrap();
        let left_top = (0..8).map(|y| alpha(&image, 2, y)).max().unwrap();
        assert!(right_top >= 150, "newest sample at 100 % touches the top: {right_top}");
        assert_eq!(left_top, 0, "oldest sample at 0 % does not");

        // An idle CPU on the fixed 0-100 scale is a line on the floor, not a mountain.
        let idle = vec![1.0; 60];
        let flat = render(&frame("", &idle, Layout::Graph, false, 24));
        assert_eq!(opaque_pixels(&flat, 0..14), 0, "nothing above the bottom third");
        assert!(opaque_pixels(&flat, 19..24) > 0);
    }

    #[test]
    fn the_pen_is_the_metric_hue_and_is_darkened_on_a_light_bar() {
        let series = vec![60.0; 60];
        let dark = render(&frame("", &series, Layout::Graph, false, 24));
        let light = render(&frame("", &series, Layout::Graph, true, 24));
        // A 1.5 px line that lands between two pixel rows covers each of them about 85 %.
        let line_dark = dark.rgba.chunks(4).find(|p| p[3] >= 200).expect("a line pixel on the dark bar");
        let line_light = light.rgba.chunks(4).find(|p| p[3] >= 200).expect("a line pixel on the light bar");
        assert_eq!((line_dark[0], line_dark[1], line_dark[2]), GREEN);
        assert!(line_light[0] < GREEN.0 && line_light[1] < GREEN.1 && line_light[2] < GREEN.2);
        assert!(line_light[3] >= 200, "darkened, not faded");
    }

    #[test]
    fn both_puts_the_value_above_and_the_graph_below() {
        let series: Vec<f64> = (0..60).map(|i| 20.0 + (i % 7) as f64 * 10.0).collect();
        let image = render(&frame("16", &series, Layout::Both, false, 24));
        // The text occupies the upper 60 % (rows 0..14); the lower rows hold only graph colour.
        let upper = (0..14).flat_map(|y| (0..24).map(move |x| (x, y))).filter(|(x, y)| pixel(&image, *x, *y)[3] > 200).count();
        assert!(upper > 10, "text pixels in the upper part: {upper}");
        for y in 15..24 {
            for x in 0..24 {
                let p = pixel(&image, x, y);
                if p[3] > 0 {
                    assert_eq!((p[0], p[1], p[2]), GREEN, "only the pen below the value, row {y}");
                }
            }
        }
        assert!(opaque_pixels(&image, 15..24) > 0);
    }

    #[test]
    fn a_label_that_cannot_be_read_gives_way_to_the_graph() {
        let series: Vec<f64> = (0..60).map(|i| (i % 9) as f64 * 11.0).collect();
        let image = render(&frame("123456789", &series, Layout::Value, false, 16));
        // No glyph-coloured pixel anywhere: the whole square is the sparkline.
        for chunk in image.rgba.chunks(4) {
            if chunk[3] > 0 {
                assert_eq!((chunk[0], chunk[1], chunk[2]), GREEN);
            }
        }
        assert!(opaque_pixels(&image, 0..16) > 0);
    }

    #[test]
    fn a_missing_reading_still_says_something() {
        let image = render(&frame("\u{2014}", &[], Layout::Graph, false, 20));
        assert!(opaque_pixels(&image, 0..20) > 0, "a dash, not an empty icon");
        let both = render(&frame("\u{2014}", &[], Layout::Both, true, 20));
        assert!(both.rgba.chunks(4).any(|p| p[3] >= 240));
    }

    #[test]
    fn the_rate_ceiling_is_a_round_number_and_never_below_the_floor() {
        assert_eq!(nice_ceiling(0.0, 100_000.0), 100_000.0);
        assert_eq!(nice_ceiling(30_000.0, 100_000.0), 100_000.0);
        assert_eq!(nice_ceiling(1_300_000.0, 100_000.0), 2_000_000.0);
        assert_eq!(nice_ceiling(5_000_000.0, 100_000.0), 5_000_000.0);
        assert_eq!(nice_ceiling(5_000_001.0, 100_000.0), 10_000_000.0);
        assert_eq!(nice_ceiling(f64::NAN, 100_000.0), 100_000.0);
    }

    #[test]
    fn a_short_history_is_not_stretched_across_the_icon() {
        let columns = resample(&[10.0, 20.0, 30.0], 12);
        assert!(columns[..9].iter().all(Option::is_none), "no samples yet for the older columns");
        assert!(columns[11].is_some());
        let full: Vec<f64> = (0..240).map(|i| i as f64).collect();
        let columns = resample(&full, 12);
        assert!(columns.iter().all(Option::is_some));
        assert!(columns[11].unwrap() > columns[0].unwrap(), "newest on the right");
        assert!(columns[0].unwrap() >= 180.0, "only the last minute is shown");
    }

    #[test]
    fn the_taskbar_theme_is_read_as_a_dword_and_missing_means_dark() {
        struct Scratch(String);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let key = wide(&self.0);
                let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
            }
        }
        let scratch = Scratch(format!(r"Software\Classes\AppUserModelId\meerkat-test-{}", std::process::id()));
        assert_eq!(read_dword(&scratch.0, "SystemUsesLightTheme"), None);

        let write = |value: u32| {
            let key = wide(&scratch.0);
            let name = wide("SystemUsesLightTheme");
            let status = unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    REG_DWORD.0,
                    Some((&value as *const u32).cast::<c_void>()),
                    4,
                )
            };
            assert_eq!(status, ERROR_SUCCESS);
        };
        write(1);
        assert_eq!(read_dword(&scratch.0, "SystemUsesLightTheme"), Some(1));
        write(0);
        assert_eq!(read_dword(&scratch.0, "SystemUsesLightTheme"), Some(0));
    }

    /// Writes a contact sheet of sample icons on a light and a dark bar to the temp directory,
    /// scaled up, so the output can be looked at without the shell. Run with
    /// `cargo test tray_render::tests::sample_sheet -- --nocapture` to see the path.
    #[test]
    fn sample_sheet() {
        let wave: Vec<f64> = (0..120).map(|i| 16.0 + 14.0 * ((i as f64) / 7.0).sin().abs() + (i % 5) as f64).collect();
        let idle = vec![3.0; 120];
        let climbing: Vec<f64> = (0..120).map(|i| 10.0 + i as f64 * 0.6).collect();
        let network: Vec<f64> = (0..120).map(|i| 200_000.0 + ((i * 37) % 11) as f64 * 150_000.0).collect();
        let network_range = (0.0, nice_ceiling(2_000_000.0, 100_000.0));

        struct Sample<'a> {
            label: &'a str,
            series: &'a [f64],
            range: (f64, f64),
            hue: (u8, u8, u8),
            layout: Layout,
        }
        let samples = [
            Sample { label: "16", series: &wave, range: (0.0, 100.0), hue: (94, 203, 143), layout: Layout::Both },
            Sample { label: "62", series: &climbing, range: (0.0, 100.0), hue: (100, 169, 255), layout: Layout::Value },
            Sample { label: "1.2M", series: &network, range: network_range, hue: (79, 210, 218), layout: Layout::Value },
            Sample { label: "1.2M", series: &network, range: network_range, hue: (79, 210, 218), layout: Layout::Both },
            Sample { label: "3", series: &idle, range: (0.0, 100.0), hue: (94, 203, 143), layout: Layout::Graph },
            Sample { label: "100", series: &climbing, range: (0.0, 100.0), hue: (163, 215, 95), layout: Layout::Value },
            Sample { label: "62\u{b0}", series: &wave, range: (0.0, 100.0), hue: (255, 139, 94), layout: Layout::Both },
            Sample { label: "\u{2014}", series: &[], range: (0.0, 100.0), hue: (234, 122, 198), layout: Layout::Both },
        ];

        let zoom = 6u32;
        let cell = 32 * zoom + 8;
        let sizes = [16, 20, 24, 32];
        let backgrounds = [([0x20u8, 0x20, 0x20], false), ([0xF3u8, 0xF3, 0xF3], true)];
        let columns = samples.len() as u32;
        let rows = (sizes.len() * backgrounds.len()) as u32;
        let (sheet_w, sheet_h) = (columns * cell, rows * cell);
        let mut sheet = image::RgbaImage::new(sheet_w, sheet_h);

        let mut row = 0u32;
        for size in sizes {
            for (background, light) in backgrounds {
                for (column, sample) in samples.iter().enumerate() {
                    let rendered = render(&Frame {
                        label: sample.label,
                        series: sample.series,
                        range: sample.range,
                        hue: sample.hue,
                        layout: sample.layout,
                        light_bar: light,
                        size,
                    });
                    let (origin_x, origin_y) = (column as u32 * cell, row * cell);
                    for y in 0..cell {
                        for x in 0..cell {
                            sheet.put_pixel(origin_x + x, origin_y + y, image::Rgba([background[0], background[1], background[2], 255]));
                        }
                    }
                    for y in 0..rendered.height {
                        for x in 0..rendered.width {
                            let [r, g, b, a] = pixel(&rendered, x, y);
                            let blend = |ink: u8, back: u8| ((ink as u32 * a as u32 + back as u32 * (255 - a as u32)) / 255) as u8;
                            let out = image::Rgba([blend(r, background[0]), blend(g, background[1]), blend(b, background[2]), 255]);
                            for dy in 0..zoom {
                                for dx in 0..zoom {
                                    sheet.put_pixel(origin_x + 4 + x * zoom + dx, origin_y + 4 + y * zoom + dy, out);
                                }
                            }
                        }
                    }
                }
                row += 1;
            }
        }
        let path = std::env::temp_dir().join("meerkat-tray-samples.png");
        sheet.save(&path).expect("the temp directory is writable");
        println!("tray samples: {}", path.display());
    }
}
