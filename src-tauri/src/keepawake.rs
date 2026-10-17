//! Keeps Windows from sleeping while Meerkat runs, when the user has asked it to.

use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED, EXECUTION_STATE};

/// The execution-state flags one wish asks Windows for. `ES_CONTINUOUS` turns the call into a
/// standing request that stays in force until it is cleared or the thread ends, instead of a
/// one-tick nudge; `ES_SYSTEM_REQUIRED` is the bit that actually holds the machine awake.
fn execution_state(wanted: bool) -> EXECUTION_STATE {
    if wanted {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    }
}

/// Applies or clears the keep-awake request, and reports whether Windows accepted the call.
///
/// The call is made on the sampler thread on purpose: the request Windows keeps is tied to the
/// thread that made it and dies with that thread, and the sampler is the one thread that lives
/// for the whole process. A request made from a pooled worker would vanish the moment that
/// worker was reused, and the machine would drift back to sleep mid-compile.
pub fn set(wanted: bool) -> bool {
    // A zero return is Windows saying it did not take the request.
    let previous = unsafe { SetThreadExecutionState(execution_state(wanted)) };
    previous != EXECUTION_STATE(0)
}

/// Clears the standing request, for the exit path. `on_exit` is the one place every clean way
/// out passes through; releasing here keeps a laptop from being left held awake by a process
/// that has already gone.
pub fn release() {
    unsafe {
        SetThreadExecutionState(execution_state(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `set` and `release` both funnel through `execution_state`, so covering the flag arithmetic
    // covers everything a test could assert without a real Windows session.
    #[test]
    fn each_request_asks_for_the_right_flags() {
        assert_eq!(execution_state(true), ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        // Turning it off, and releasing on exit, keep only ES_CONTINUOUS: that clears the
        // standing request rather than nudging the machine awake for one tick.
        assert_eq!(execution_state(false), ES_CONTINUOUS);
        assert!(!execution_state(false).contains(ES_SYSTEM_REQUIRED));
    }
}
