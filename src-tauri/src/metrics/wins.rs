//! Top-level window inventory. Used both to name apps after what the user sees and to
//! tell a foreground app from a background process. Chromium browsers keep one hidden
//! top-level window per tab, which is what makes the per-tab split possible later.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};

#[derive(Clone, Debug)]
pub struct TopWindow {
    pub pid: u32,
    pub title: String,
    pub visible: bool,
    pub hwnd: isize,
}

struct Collector {
    items: Vec<TopWindow>,
}

unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let collector = &mut *(lparam.0 as *mut Collector);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == 0 {
        return true.into();
    }
    let mut buffer = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut buffer);
    if len <= 0 {
        return true.into();
    }
    let title = String::from_utf16_lossy(&buffer[..len as usize]);
    let title = title.trim().to_string();
    if title.is_empty() {
        return true.into();
    }
    collector.items.push(TopWindow {
        pid,
        visible: IsWindowVisible(hwnd).as_bool(),
        title,
        hwnd: hwnd.0 as isize,
    });
    true.into()
}

pub fn toplevel_windows() -> Vec<TopWindow> {
    let mut collector = Collector { items: Vec::new() };
    unsafe {
        let _ = EnumWindows(
            Some(callback),
            LPARAM(&mut collector as *mut Collector as isize),
        );
    }
    collector.items
}
