//! Top-level window inventory. Used both to name apps after what the user sees and to
//! tell a foreground app from a background process.
//!
//! Only titled windows are kept, and only the ones a person would call a window of the app: the
//! input-method machinery (`IME`, `MSCTFIME UI`) and tool windows are left out, or every app
//! with a text field would list "Default IME" among its windows.
//!
//! What this does not give is the tabs of a browser. Measured on a machine with 556 top-level
//! windows: Chrome had one titled window (the browser process, showing the active tab) and
//! 23 renderers with no title at all, and with site isolation a renderer is not a tab anyway.
//! A browser is split by process type instead, and carries one tab title at most.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
};

#[derive(Clone, Debug)]
pub struct TopWindow {
    pub pid: u32,
    pub title: String,
    pub visible: bool,
    pub hwnd: isize,
}

/// Window classes of the text-input infrastructure. Every GUI thread that takes text input gets
/// one of each, titled "Default IME" and "MSCTFIME UI", and none of them is a window of the app.
const NOISE_CLASSES: [&str; 2] = ["IME", "MSCTFIME UI"];

/// Whether a window is plumbing rather than something the user would call a window of the app:
/// an input-method window, or a tool window (floating palettes, which Windows itself leaves out
/// of the taskbar and of Alt+Tab).
fn is_noise(class: &str, ex_style: u32) -> bool {
    ex_style & WS_EX_TOOLWINDOW.0 != 0
        || NOISE_CLASSES.iter().any(|noise| class.eq_ignore_ascii_case(noise))
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

    // Asked only of windows that have a title, which are few.
    let mut class = [0u16; 128];
    let class_len = GetClassNameW(hwnd, &mut class);
    let class = String::from_utf16_lossy(&class[..class_len.max(0) as usize]);
    let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    if is_noise(&class, ex_style) {
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

#[cfg(test)]
mod tests {
    use super::*;

    const WS_EX_APPWINDOW: u32 = 0x0004_0000;
    const WS_EX_LAYERED: u32 = 0x0008_0000;

    #[test]
    fn input_method_windows_are_not_windows_of_the_app() {
        assert!(is_noise("IME", 0));
        assert!(is_noise("MSCTFIME UI", 0));
        assert!(is_noise("ime", 0), "class names compare without case");
    }

    #[test]
    fn tool_windows_are_left_out_whatever_their_class() {
        assert!(is_noise("Afx:00400000:b:00010011:00000006:00000000", WS_EX_TOOLWINDOW.0));
        assert!(is_noise("Chrome_WidgetWin_1", WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED));
    }

    #[test]
    fn ordinary_windows_are_kept() {
        assert!(!is_noise("Chrome_WidgetWin_1", 0));
        assert!(!is_noise("Notepad", WS_EX_APPWINDOW));
        assert!(!is_noise("CabinetWClass", WS_EX_APPWINDOW | WS_EX_LAYERED));
        // "IME" has to be the whole class, not part of one.
        assert!(!is_noise("TImeEditor", 0));
        assert!(!is_noise("", 0));
    }

    // The live check: this machine's desktop is enumerated for real, and the two titles the
    // input-method machinery gives its windows must not come out of it.
    #[test]
    fn the_live_inventory_has_no_input_method_windows() {
        for window in toplevel_windows() {
            assert_ne!(window.title, "Default IME", "pid {}", window.pid);
            assert_ne!(window.title, "MSCTFIME UI", "pid {}", window.pid);
        }
    }
}
