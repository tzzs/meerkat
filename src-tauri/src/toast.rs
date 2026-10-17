//! Toasts under Meerkat's own name.
//!
//! Windows files a toast under an AppUserModelID and heads it with the name registered for
//! that ID. The notification plugin only set one when the executable did not live in a
//! `target\debug` or `target\release` directory, and otherwise fell back to PowerShell's, so
//! every toast from a build directory (dev, release check, an unpacked portable zip) arrived as
//! "Windows PowerShell". Meerkat now registers its own ID per user and always shows toasts with it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

use tauri::AppHandle;
use tauri_winrt_notification::Toast;
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

/// The Tauri identifier doubles as the AppUserModelID; the installer's shortcut uses it too.
pub const AUMID: &str = "com.tzz.meerkat";

const ICON: &[u8] = include_bytes!("../icons/icon.png");

struct Note {
    title: String,
    body: String,
}

/// Hands toasts to a thread of their own. Showing one goes through WinRT and can take tens of
/// milliseconds; the sampler only ever pays for a channel send.
pub struct Toaster {
    queue: Sender<Note>,
}

impl Toaster {
    pub fn start(app: AppHandle) -> Self {
        let (queue, notes) = channel::<Note>();
        let spawned = std::thread::Builder::new().name("meerkat-toast".into()).spawn(move || {
            for note in notes {
                show(&app, &note);
            }
        });
        if let Err(error) = spawned {
            crate::diag::error(format!("toast thread: {error}"));
        }
        Self { queue }
    }

    /// Never blocks. A toast that cannot be queued is dropped: it is a courtesy, the insight
    /// itself is already in the window's list.
    pub fn show(&self, title: String, body: String) {
        let _ = self.queue.send(Note { title, body });
    }
}

fn show(app: &AppHandle, note: &Note) {
    let handle = app.clone();
    let shown = Toast::new(AUMID)
        .title(&note.title)
        .text1(&note.body)
        // The notification plugin was silent, and a monitor that chimes at every threshold
        // crossing is one people switch off.
        .sound(None)
        .on_activated(move |_| {
            crate::menu::show_main(&handle, Some("overview"));
            Ok(())
        })
        .show();
    if let Err(error) = shown {
        crate::diag::warn(format!("toast: {error}"));
    }
}

/// Registers Meerkat's name and icon for its AppUserModelID under HKCU, which needs no rights and
/// is rewritten on every start, so a moved or reinstalled copy fixes itself.
pub fn register(app_data: &Path) {
    let icon = match write_icon(app_data) {
        Ok(path) => Some(path),
        Err(error) => {
            crate::diag::warn(format!("toast icon: {error}"));
            None
        }
    };
    if let Err(error) = write_identity(&identity_key(AUMID), "Meerkat", icon.as_deref()) {
        crate::diag::warn(format!("toast identity: {error}"));
    }
}

fn identity_key(aumid: &str) -> String {
    format!(r"Software\Classes\AppUserModelId\{aumid}")
}

/// The shell reads the icon from a file path, so the embedded one is written out once.
fn write_icon(app_data: &Path) -> Result<PathBuf, String> {
    let dir = app_data.join("icons");
    let path = dir.join("meerkat.png");
    let current = std::fs::metadata(&path).map(|m| m.len() == ICON.len() as u64).unwrap_or(false);
    if !current {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, ICON).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn write_identity(key: &str, display_name: &str, icon: Option<&Path>) -> Result<(), String> {
    set_string(key, "DisplayName", display_name)?;
    if let Some(icon) = icon {
        set_string(key, "IconUri", &icon.display().to_string())?;
    }
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// RegSetKeyValueW creates the key on the way, so one call is the whole write.
fn set_string(key: &str, name: &str, value: &str) -> Result<(), String> {
    let key_w = wide(key);
    let name_w = wide(name);
    let data = wide(value);
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key_w.as_ptr()),
            PCWSTR(name_w.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{key}\\{name}: error {}", status.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::{RegDeleteTreeW, RegGetValueW, RRF_RT_REG_SZ};

    fn read(key: &str, name: &str) -> Option<String> {
        let key_w = wide(key);
        let name_w = wide(name);
        let mut buffer = [0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(key_w.as_ptr()),
                PCWSTR(name_w.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        (status == ERROR_SUCCESS).then(|| {
            let end = buffer.iter().position(|c| *c == 0).unwrap_or(0);
            String::from_utf16_lossy(&buffer[..end])
        })
    }

    /// Deletes the throw-away key even when an assertion fails half way.
    struct Scratch(String);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let key = wide(&self.0);
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
        }
    }

    #[test]
    fn the_identity_is_written_where_the_shell_reads_it_and_rewriting_is_harmless() {
        let scratch = Scratch(identity_key(&format!("meerkat-test-{}", std::process::id())));
        let icon = std::env::temp_dir().join("meerkat-test-icon.png");

        write_identity(&scratch.0, "Meerkat", Some(&icon)).expect("HKCU is writable without rights");
        write_identity(&scratch.0, "Meerkat", Some(&icon)).expect("a second start writes the same values");
        assert_eq!(read(&scratch.0, "DisplayName").as_deref(), Some("Meerkat"));
        assert_eq!(read(&scratch.0, "IconUri"), Some(icon.display().to_string()));

        let key = scratch.0.clone();
        drop(scratch);
        assert_eq!(read(&key, "DisplayName"), None, "the throw-away key is gone afterwards");
    }

    // The ID is spelled out in AUMID because the log needs it before Tauri has read its config.
    // A rename that changed the identifier and missed this would file every toast under the
    // old name, which nothing else would notice.
    #[test]
    fn the_aumid_is_the_tauri_identifier() {
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], AUMID);
        assert_eq!(identity_key(AUMID), r"Software\Classes\AppUserModelId\com.tzz.meerkat");
    }

    #[test]
    fn the_icon_is_written_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("meerkat-toast-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write_icon(&dir).expect("a temp dir to write into");
        assert_eq!(std::fs::read(&path).unwrap(), ICON);
        let again = write_icon(&dir).expect("an existing icon is fine");
        assert_eq!(again, path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
