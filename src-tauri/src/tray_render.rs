//! Draws a tray readout bitmap with GDI, on the sampler thread.
//!
//! A canvas in the panel webview could do this too, but a hidden webview gets its timers
//! throttled and the taskbar corner has to keep updating while nothing is open. GDI also
//! uses the real system font. Windows leaves the alpha byte of a DIB at zero, so
//! transparency is rebuilt from pixel brightness.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, POINT};
use windows::Win32::Graphics::Gdi::{
    ANSI_CHARSET, ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DC_PEN,
    DEFAULT_GUI_FONT, DIB_USAGE, GetDC, GetStockObject, HGDIOBJ, OUT_DEFAULT_PRECIS, Polyline,
    ReleaseDC, SelectObject, SetBkMode, SetDCPenColor, SetTextColor, TextOutW, TRANSPARENT,
};

/// The GDI `RGB()` macro, which the windows crate does not expose.
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};

pub struct Rendered {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The shell asks for icons at `SM_CXSMICON` logical pixels scaled by the current DPI;
/// drawing at a fixed height would be rescaled by the taskbar and come out blurry.
pub fn icon_height() -> i32 {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if size > 0 { size } else { 16 }
}

pub fn render(
    label: &str,
    series: &[f64],
    hue: (u8, u8, u8),
    show_value: bool,
    show_graph: bool,
    dark_taskbar: bool,
    height: i32,
) -> Rendered {
    let font_height = -((height as f32 * 0.62).round() as i32).clamp(9, 20);
    let char_width = (height as f32 * 0.52).round() as i32;
    let width: i32 = if show_value {
        (height / 2 + label.chars().count() as i32 * char_width).clamp(height + 8, height * 6)
    } else {
        height + 8
    };
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        if dc.is_invalid() {
            ReleaseDC(None, screen);
            return Rendered { rgba, width: width as u32, height: height as u32 };
        }
        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &header, DIB_USAGE(BI_RGB.0), &mut pixels, None, 0) else {
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            return Rendered { rgba, width: width as u32, height: height as u32 };
        };
        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
        SetBkMode(dc, TRANSPARENT);

        // The stock pen is black; the device-context pen is what lets us recolour it.
        SelectObject(dc, GetStockObject(DC_PEN));

        let ink = if dark_taskbar {
            COLORREF(0x00F2_F4F8)
        } else {
            COLORREF(0x0018_1A1E)
        };
        SetTextColor(dc, ink);

        if show_value && !label.is_empty() {
            let face: Vec<u16> = "Segoe UI Variable Text"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let font = CreateFontW(
                font_height,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                ANSI_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                // Grayscale antialiasing, not ClearType: the subpixel fringes would
                // survive the brightness-to-alpha step as coloured edges.
                ANTIALIASED_QUALITY,
                DEFAULT_GUI_FONT.0 as u32,
                PCWSTR(face.as_ptr()),
            );
            let previous_font = SelectObject(dc, HGDIOBJ(font.0));
            let wide: Vec<u16> = label.encode_utf16().collect();
            let top = if show_graph { 1 } else { (height - font_height.abs()) / 2 };
            let _ = TextOutW(dc, height / 5, top, &wide);
            SelectObject(dc, previous_font);
            let _ = DeleteObject(HGDIOBJ(font.0));
        }

        if show_graph && series.len() > 1 {
            let (r, g, b) = hue;
            let pen = if dark_taskbar {
                rgb(r, g, b)
            } else {
                rgb(r / 2, g / 2, b / 2)
            };
            SetDCPenColor(dc, pen);
            let points = chart_points(series, width, height, show_value);
            let _ = Polyline(dc, &points);
        }

        SelectObject(dc, previous);
        if !pixels.is_null() {
            let source = std::slice::from_raw_parts(pixels.cast::<u8>(), rgba.len());
            for (i, pixel) in source.chunks(4).enumerate() {
                let brightness = pixel[0].max(pixel[1]).max(pixel[2]);
                rgba[i * 4] = pixel[0];
                rgba[i * 4 + 1] = pixel[1];
                rgba[i * 4 + 2] = pixel[2];
                rgba[i * 4 + 3] = brightness;
            }
        }
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
    }

    Rendered {
        rgba,
        width: width as u32,
        height: height as u32,
    }
}

fn chart_points(series: &[f64], width: i32, height: i32, inset_top: bool) -> Vec<POINT> {
    let window = &series[series.len().saturating_sub(24)..];
    if window.len() < 2 {
        return Vec::new();
    }
    let top = if inset_top { height - 9 } else { 3 };
    let bottom = height - 3;
    let max = window.iter().cloned().fold(1f64, f64::max).max(1.0);
    window
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let x = 3.0 + (i as f64 / (window.len() - 1) as f64) * (width - 6) as f64;
            let y = bottom as f64 - (value / max) * (bottom - top) as f64;
            POINT { x: x as i32, y: y as i32 }
        })
        .collect()
}

/// Saves the last rendered bitmap so the taskbar output can be inspected without a
/// screenshot of the shell. Debug builds only.
#[cfg(debug_assertions)]
pub fn dump_debug(id: &str, image: &Rendered, dir: &std::path::Path) {
    use std::fs;
    let path = dir.join(format!("tray-{id}.rgba"));
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(path, &image.rgba);
}
