use tauri::{AppHandle, Manager};
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PeekMessageW, MSG, PM_NOREMOVE, WM_HOTKEY};

use crate::ipc::AppState;

const HOTKEY_ID: i32 = 1;

pub fn spawn(app: AppHandle) {
    if let Err(error) = std::thread::Builder::new()
        .name("meerkat-hotkey".into())
        .spawn(move || listen(&app))
    {
        crate::diag::warn(format!("hotkey thread: {error}"));
    }
}

fn listen(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        crate::diag::warn("hotkey: app state is missing");
        return;
    };
    let mut message = MSG::default();
    unsafe {
        // PeekMessageW creates the queue that RegisterHotKey needs on this thread.
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        if let Err(error) = RegisterHotKey(None, HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x50) {
            crate::diag::warn(format!("Ctrl+Alt+P could not be registered: {error}"));
            return;
        }
    }
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        if message.message == WM_HOTKEY {
            crate::perf::toggle_foreground(app, &state);
        }
    }
}
