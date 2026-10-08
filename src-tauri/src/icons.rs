//! Extracts the real icon of a Windows executable and hands it back as an RGBA bitmap,
//! so the app rows can show each program's own icon instead of a coloured initial.
//!
//! Everything here is Win32 GDI plus the shell, and no new `windows` crate feature is
//! required: `Win32_UI_Shell`, `Win32_UI_WindowsAndMessaging`, `Win32_Graphics_Gdi`,
//! `Win32_Storage_FileSystem` and `Win32_System_Threading` already cover it.
//!
//! The two things that make this awkward, and how it is handled:
//!
//! 1. GDI will not give us the icon's alpha byte. A 32bpp DIBSection comes back with the
//!    alpha lane left at zero (`tray_render.rs` runs into the same thing), and a 32bpp icon
//!    resource may carry a per-pixel alpha that `DrawIconEx` has already baked into RGB.
//!    Reading such a buffer directly turns every transparent corner into an opaque black
//!    square. This is solved by drawing the icon twice, once onto a black field and once
//!    onto a white one, and solving the alpha back out. See `compose`.
//! 2. GDI handles have to be released on the error paths as well as the success path. Each
//!    helper takes its handles, releases them and returns; nothing is left to the caller and
//!    no early return skips a `DeleteObject`/`DeleteDC`/`DestroyIcon`. The trap worth writing
//!    down, because it was found by counting handles rather than by reading docs: `GetIconInfo`
//!    returns `hbmMask`/`hbmColor` as *new handles* and `DestroyIcon` does not free them.
//!    Measured with `GetGuiResources`, assuming otherwise costs exactly 2 GDI objects per icon,
//!    which never comes back down. See `render_icon`.
//!
//! Extraction is not free (the shell loads the module on first touch) and the UI asks for a
//! row's icon on every 1 Hz sample, so results — including the failures — are memoised.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DeleteDC,
    DeleteObject, DIB_USAGE, GetObjectW, HBITMAP, HGDIOBJ, HDC, SelectObject,
};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DrawIconEx, GetIconInfo, DI_NORMAL, HICON, ICONINFO,
};

/// One icon, decoded, row-major 8-bit RGBA. `rgba.len()` is always `width * height * 4`.
///
/// `Clone` is what lets the cache hand a copy out instead of giving away the only instance.
#[derive(Clone)]
pub struct IconImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Smallest and largest edge we are willing to render. The rows want 16-48px; clamping keeps
/// a stray `size` from making a 4096px icon (and a 64 MB cache entry) off the sampler thread.
const MIN_SIZE: i32 = 8;
const MAX_SIZE: i32 = 256;
const DEFAULT_SIZE: i32 = 32;

/// Bound on `CACHE`. Entries are small (a 48px icon is 9 KB, a 256px one is 256 KB), so the
/// cap is about holding a bounded set of distinct executables rather than about bytes.
const CACHE_CAP: usize = 192;

/// File extensions we will pull an icon resource from. A running process's image path is
/// always one of these; anything else is a document, a directory or a bad path, and for those
/// the shell would happily hand back a generic page icon that would look like a real result.
const ICON_EXTENSIONS: [&str; 6] = ["exe", "dll", "com", "scr", "cpl", "drv"];

pub fn icon_for(exe_path: &str, size: i32) -> Option<IconImage> {
    let size = clamp_size(size);
    let key = (exe_path.to_string(), size);

    if let Some(hit) = lookup(&key) {
        return hit;
    }

    let extracted = extract_from_file(exe_path, size);
    remember(key, extracted.clone())
}

/// There is deliberately no process-first lookup here. A row is an application, and the
/// executable that names it is the grouping's anchor (`chrome.exe`, not the renderer that
/// happens to be running), which is not the process a pid would resolve to.
fn clamp_size(size: i32) -> i32 {
    if size <= 0 {
        DEFAULT_SIZE
    } else {
        size.clamp(MIN_SIZE, MAX_SIZE)
    }
}

// ---------------------------------------------------------------------------
// cache
// ---------------------------------------------------------------------------

struct Entry {
    /// Monotonic touch stamp; the lowest one in the map is the least recently used entry.
    last_used: u64,
    image: Option<IconImage>,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<(String, i32), Entry>,
    clock: u64,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

/// A lock poisoned by a panicking thread must not take the whole UI down, so the poisoned
/// guard is taken anyway; the map is plain data and stays consistent.
fn lock_cache(guard: &Mutex<Cache>) -> std::sync::MutexGuard<'_, Cache> {
    guard.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `Some(Some(image))` is a cached hit, `Some(None)` is a cached miss, `None` is not cached.
fn lookup(key: &(String, i32)) -> Option<Option<IconImage>> {
    let cache = cache();
    let mut cache = lock_cache(cache);
    cache.clock += 1;
    let last_used = cache.clock;
    let entry = cache.entries.get_mut(key)?;
    entry.last_used = last_used;
    Some(entry.image.clone())
}

fn remember(key: (String, i32), image: Option<IconImage>) -> Option<IconImage> {
    let cache = cache();
    let mut cache = lock_cache(cache);
    cache.clock += 1;
    let last_used = cache.clock;

    if cache.entries.len() >= CACHE_CAP && !cache.entries.contains_key(&key) {
        // Least-recently-used eviction. The map is capped at 192 entries, so the linear scan
        // is cheaper than maintaining an intrusive list and is still exact.
        let oldest = cache
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(oldest_key, _)| oldest_key.clone());
        if let Some(oldest) = oldest {
            cache.entries.remove(&oldest);
        }
    }

    cache
        .entries
        .insert(key, Entry { last_used, image: image.clone() });
    image
}

// ---------------------------------------------------------------------------
// extraction
// ---------------------------------------------------------------------------

/// Asks the shell for the icon of an existing executable and renders it at `size`.
fn extract_from_file(exe_path: &str, size: i32) -> Option<IconImage> {
    if !looks_like_icon_source(exe_path) {
        return None;
    }
    let wide: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
    if wide.len() > 32_767 {
        return None;
    }

    unsafe {
        let mut info = SHFILEINFOW::default();
        // Above the shell's small-icon size the large icon is the one with detail worth
        // keeping. SHGFI_LARGEICON happens to be 0, so it is spelled out only to say so.
        let size_class = if size > small_icon_edge() {
            SHGFI_LARGEICON
        } else {
            SHGFI_SMALLICON
        };
        let returned = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | size_class,
        );
        // The icon handle the shell hands back is ours to own, so it is destroyed once the
        // render is done with it and also on the paths where the render never starts.
        if returned == 0 || info.hIcon.0.is_null() {
            return None;
        }
        let icon = info.hIcon;
        let rendered = render_icon(icon, size);
        let _ = DestroyIcon(icon);
        rendered
    }
}

/// The edge the shell uses for its small icons; DPI aware, so 16 becomes 20 or 24 when scaled.
fn small_icon_edge() -> i32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    let edge = unsafe { GetSystemMetrics(SM_CXSMICON) };
    if edge > 0 {
        edge
    } else {
        16
    }
}

/// A path only yields an icon if it is really an executable sitting on disk.
fn looks_like_icon_source(exe_path: &str) -> bool {
    let Ok(path) = std::path::Path::new(exe_path).canonicalize() else {
        return false;
    };
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    ICON_EXTENSIONS.contains(&extension.as_str())
}

/// Renders an `HICON` into a `size` x `size` RGBA bitmap.
///
/// `GetIconInfo` is called to reject an icon that carries no bitmap at all. Its `hbmMask` and
/// `hbmColor` come back as *new handles* and `DestroyIcon` does not free them — measured with
/// `GetGuiResources`, leaving them behind costs exactly 2 GDI objects per icon, which is a
/// permanent leak once the rows refresh every second. They are therefore released as soon as
/// they have been measured, on every path. `hbmColor` is NULL for a monochrome icon.
fn render_icon(icon: HICON, size: i32) -> Option<IconImage> {
    unsafe {
        let mut icon_info = ICONINFO::default();
        if GetIconInfo(icon, &mut icon_info).is_err() {
            return None;
        }
        let extent = icon_native_extent(&icon_info);
        release_bitmap(icon_info.hbmMask);
        release_bitmap(icon_info.hbmColor);
        if extent.is_none() {
            return None;
        }

        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() {
            return None;
        }

        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                // A negative height asks for top-down rows, which is the row order the caller
                // wants; a positive height would hand back the image upside down.
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let byte_count = (size * size * 4) as usize;

        let mut dark_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(dark_bitmap) =
            CreateDIBSection(Some(dc), &header, DIB_USAGE(BI_RGB.0), &mut dark_ptr, None, 0)
        else {
            let _ = DeleteDC(dc);
            return None;
        };
        if dark_ptr.is_null() {
            let _ = DeleteObject(HGDIOBJ(dark_bitmap.0));
            let _ = DeleteDC(dc);
            return None;
        }

        let mut light_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(light_bitmap) =
            CreateDIBSection(Some(dc), &header, DIB_USAGE(BI_RGB.0), &mut light_ptr, None, 0)
        else {
            let _ = DeleteObject(HGDIOBJ(dark_bitmap.0));
            let _ = DeleteDC(dc);
            return None;
        };
        if light_ptr.is_null() {
            let _ = DeleteObject(HGDIOBJ(light_bitmap.0));
            let _ = DeleteObject(HGDIOBJ(dark_bitmap.0));
            let _ = DeleteDC(dc);
            return None;
        }

        let mut drawn = None;
        if paint(dc, dark_bitmap, dark_ptr.cast::<u8>(), byte_count, icon, size, false)
            && paint(dc, light_bitmap, light_ptr.cast::<u8>(), byte_count, icon, size, true)
        {
            // Both fields have to be read before the bitmaps are deleted. The mutable uses
            // above are finished, so these read-only views do not alias anything live.
            let dark = std::slice::from_raw_parts(dark_ptr.cast::<u8>(), byte_count);
            let light = std::slice::from_raw_parts(light_ptr.cast::<u8>(), byte_count);
            drawn = compose(dark, light, size);
        }

        let _ = DeleteObject(HGDIOBJ(light_bitmap.0));
        let _ = DeleteObject(HGDIOBJ(dark_bitmap.0));
        let _ = DeleteDC(dc);
        drawn
    }
}

/// Releases a bitmap handle that may be NULL, which is what `GetIconInfo` reports for the
/// colour plane of a monochrome icon.
unsafe fn release_bitmap(bitmap: HBITMAP) {
    if !bitmap.0.is_null() {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
    }
}

/// Paints one field with its background, draws the icon into it, restores the DC.
///
/// The background is written straight into the DIBSection's own bits instead of being blitted
/// with `PatBlt`. That also keeps the alpha lane at exactly zero — the value GDI itself leaves
/// there — so the two fields differ only in RGB, which is the assumption `compose` solves
/// against. Returns false if either the icon failed to draw, in which case the caller must not
/// trust either field.
unsafe fn paint(
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u8,
    byte_count: usize,
    icon: HICON,
    size: i32,
    white: bool,
) -> bool {
    if bits.is_null() {
        return false;
    }
    let value = if white { 255 } else { 0 };
    for lane in std::slice::from_raw_parts_mut(bits, byte_count).chunks_exact_mut(4) {
        lane[0] = value;
        lane[1] = value;
        lane[2] = value;
        lane[3] = 0;
    }

    let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
    let drawn = DrawIconEx(dc, 0, 0, icon, size, size, 0, None, DI_NORMAL).is_ok();
    SelectObject(dc, previous);
    drawn
}

/// The edge the icon resource really has, used only to reject a malformed icon.
fn icon_native_extent(icon_info: &ICONINFO) -> Option<(i32, i32)> {
    let candidate = if !icon_info.hbmColor.0.is_null() {
        icon_info.hbmColor
    } else {
        icon_info.hbmMask
    };
    let mut bitmap = BITMAP::default();
    let copied = unsafe {
        GetObjectW(
            HGDIOBJ(candidate.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bitmap as *mut BITMAP as *mut core::ffi::c_void),
        )
    };
    if copied != std::mem::size_of::<BITMAP>() as i32 {
        return None;
    }
    // A monochrome icon stacks its XOR and AND planes in one bitmap, so the mask is twice as
    // tall as the image; a colour icon's mask is the same height as the image.
    let height = if icon_info.hbmColor.0.is_null() {
        bitmap.bmHeight / 2
    } else {
        bitmap.bmHeight
    };
    if bitmap.bmWidth <= 0 || height <= 0 {
        return None;
    }
    Some((bitmap.bmWidth, height))
}

/// Recovers straight RGBA from the black-background and white-background renders.
///
/// With `a` the coverage and `c` the icon's own colour, `DrawIconEx` gives a premultiplied
/// blend, so the two fields hold:
///
/// ```text
/// dark  = c * a                 (blended over black)
/// light = c * a + 255 * (1 - a) (blended over white)
/// ```
///
/// Subtracting removes the unknown colour entirely: `light - dark = 255 * (1 - a)`, so
/// `a = 255 - (light - dark)` and then `c = dark / a`. Doing it this way rather than trusting
/// the alpha lane means a resource with an all-zero alpha channel still comes out with the
/// right shape, and it keeps the partial coverage on antialiased edges instead of flattening
/// it to on/off the way an AND mask would. That is the whole reason for the second draw.
fn compose(dark: &[u8], light: &[u8], size: i32) -> Option<IconImage> {
    let count = (size * size) as usize;
    if dark.len() < count * 4 || light.len() < count * 4 {
        return None;
    }
    let mut rgba = vec![0u8; count * 4];
    let mut any_covered = false;

    for index in 0..count {
        let offset = index * 4;
        // GDI stores a 32bpp BI_RGB DIB as B, G, R, unused; the output is R, G, B, A.
        let dark_lane = [
            dark[offset] as u16,
            dark[offset + 1] as u16,
            dark[offset + 2] as u16,
        ];
        let mut alpha = 0u16;
        for channel in 0..3 {
            // `light` can only sit above `dark` (the white field adds coverage); a negative
            // difference would be a GDI quirk, and reading it as zero keeps the pixel opaque,
            // which is the safe direction for a hole in a solid body.
            let lift = (light[offset + channel] as i32 - dark_lane[channel] as i32).clamp(0, 255) as u16;
            let channel_alpha = 255u16 - lift;
            if channel_alpha > alpha {
                alpha = channel_alpha;
            }
        }
        if alpha == 0 {
            continue;
        }
        any_covered = true;
        let mut colour = [0u8; 3];
        for channel in 0..3 {
            if alpha == 255 {
                colour[channel] = dark_lane[channel].min(255) as u8;
            } else {
                // Un-premultiply with rounding so a 50%-covered edge does not drift darker
                // every time it is decoded. Both terms stay inside u16 (255*255 + 127).
                let straight = (dark_lane[channel] * 255 + alpha / 2) / alpha;
                colour[channel] = straight.min(255) as u8;
            }
        }
        rgba[offset] = colour[2];
        rgba[offset + 1] = colour[1];
        rgba[offset + 2] = colour[0];
        rgba[offset + 3] = alpha as u8;
    }

    // An icon that renders as nothing is not a result worth caching as success; the caller
    // showing its own fallback looks better than a row with an invisible square.
    if !any_covered {
        return None;
    }
    Some(IconImage {
        rgba,
        width: size as u32,
        height: size as u32,
    })
}
