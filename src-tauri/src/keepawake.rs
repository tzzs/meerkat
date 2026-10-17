//! Keeps Windows from sleeping while Meerkat runs, when the user has asked it to.

use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED, EXECUTION_STATE, SetThreadExecutionState};

/// The execution-state flags one wish asks Windows for. `ES_CONTINUOUS` turns the call into a
/// standing request that stays in force until it is cleared or the calling thread ends, instead
/// of a one-tick nudge; `ES_SYSTEM_REQUIRED` is the bit that actually holds the machine awake.
fn execution_state(wanted: bool) -> EXECUTION_STATE {
    if wanted {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    }
}

/// Makes the request Windows keeps, and reports whether it was taken.
fn apply(wanted: bool) -> bool {
    // A zero return is Windows saying it did not take the request.
    let previous = unsafe { SetThreadExecutionState(execution_state(wanted)) };
    previous != EXECUTION_STATE(0)
}

/// Asks for, or clears, the standing request.
///
/// Both halves always happen on the main thread — `release` included — because Windows attaches
/// the request to the thread that made it, so a request applied here and cleared from anywhere
/// else would leave the machine held awake by a thread nobody owns any more. The sampler reads the
/// setting and calls this only when it changes.
///
/// Nothing here waits: the caller is the sampler loop, and a call into the shell that blocked on
/// its queue would stall the sampling with it.
pub fn request(app: &AppHandle, wanted: bool) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if apply(wanted) || !wanted {
            return;
        }
        // Windows refused, so the switch must not go on claiming a hold the system never gave.
        crate::diag::warn("keep awake: Windows did not take the request");
        let state = handle.state::<crate::ipc::AppState>();
        let mut settings = state.settings.get();
        if !settings.keep_awake {
            return;
        }
        settings.keep_awake = false;
        if let Err(error) = state.settings.set(settings) {
            crate::diag::warn(format!("keep awake: could not record the refusal: {error}"));
        }
        let _ = handle.emit("settings-changed", ());
    }) {
        crate::diag::warn(format!("keep awake: could not reach the main thread: {error}"));
    }
}

/// Clears the standing request. `on_exit` is the one place every clean way out passes through, and
/// it runs on the thread that made the request; releasing here keeps a laptop from being left held
/// awake by a program that has already gone.
pub fn release() {
    apply(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    // `request` and `release` both funnel through `execution_state`, so covering the flag
    // arithmetic covers everything a test can assert without holding a real machine awake.
    #[test]
    fn each_request_asks_for_the_right_flags() {
        assert_eq!(execution_state(true), ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        // Turning it off, and releasing on exit, keep only ES_CONTINUOUS: that clears the
        // standing request rather than nudging the machine awake for one tick.
        assert_eq!(execution_state(false), ES_CONTINUOUS);
        assert!(!execution_state(false).contains(ES_SYSTEM_REQUIRED));
    }
}
