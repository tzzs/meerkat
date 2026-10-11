//! Per-application audio control through the Windows Core Audio API
//! (`IMMDeviceEnumerator` / `IAudioSessionManager2` / `ISimpleAudioVolume` /
//! `IAudioMeterInformation` / `IAudioEndpointVolume`).
//!
//! Self-contained: depends only on the `windows` crate plus `serde`/`serde_json`. Drop it in
//! as `mod audio;`; it touches no other module.
//!
//! # Threading and COM apartments
//! Core Audio session pointers have apartment affinity and go stale the instant a process
//! stops producing audio — caching them across calls is a reliable access violation. So every
//! public entry point (a) makes sure COM is initialized on the **calling** thread, (b) does a
//! **fresh enumeration** inside that call, and (c) returns plain Rust data only. No COM pointer
//! escapes a function, so results can be moved to any thread (e.g. serialized to the frontend).
//!
//! Intended use: one dedicated audio thread calls [`init_com`] once, then polls [`endpoints`]
//! and [`sessions`] about once per second. Writes may come from any thread (a Tauri command
//! thread, say) because each re-enumerates on its own thread.
//!
//! # Session identity (`AppSession::id`) and why writes re-enumerate
//! `id` is only meaningful within the enumeration that produced it. Every [`sessions`] call
//! records `id -> { pid, session instance identifier }` in a process-wide mutex. The write
//! functions re-enumerate and match that snapshot by *instance identifier* — unique per live
//! session, whereas a bare position is not stable (sessions are dropped/inserted at any time,
//! so an index would let a slider meant for one app land on another) and a bare PID is not
//! either (one process may own several sessions). If the session is gone by the time the user
//! clicks, the write returns a descriptive error instead of silently adjusting the wrong app.
//! The registry is per-role and last-writer-wins, so two concurrent `sessions()` calls for the
//! same role from different threads invalidate each other's ids: keep one owner per role
//! (normally the audio thread) and let the write calls come from anywhere.
//!
//! # Capture (`Role::Capture`) semantics
//! A capture session means "this process has a capture stream open right now". Treat it as
//! best-effort: it disappears when the stream closes, and it does not imply audible input.
//! Windows does not reliably support per-app *input* volume, so it is deliberately not modelled
//! as a first-class control — [`set_session_volume`] with `Role::Capture` is passed straight to
//! `ISimpleAudioVolume` and may be ignored by the driver. Endpoint-level metering on a capture
//! device ([`Endpoint::peak`]) is reliable and does *not* open a capture stream, so it never
//! lights the microphone privacy indicator.
//!
//! # Persistence of per-app volume
//! Windows does not persist per-application volume: the session dies with the process. To
//! reproduce the macOS behaviour ("Chrome stays at 30% next launch"), [`remember_volume`]
//! stores a rule keyed by `exe name + endpoint id`, and [`apply_saved_volumes`] re-applies the
//! stored rules to the sessions that are live and, from a watcher thread it starts once, to
//! every session that appears later — each session once, when it is first met, so a volume the
//! user changes afterwards is not fought over. Callers own the state file path.
//!
//! # Errors
//! Nothing is swallowed at the write path: failures carry the Windows message text and the
//! HRESULT (e.g. `AUDCLNT_E_DEVICE_INVALIDATED`, `E_ACCESSDENIED` for protected sessions) so
//! the UI can surface them. Read paths degrade per item (a session that fails to report a peak
//! yields `peak = 0.0` rather than dropping the whole list) and are documented on each fn.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use windows::core::{GUID, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, EDataFlow, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

/// `QueryFullProcessImageNameW` buffer, `MAX_PATH` plus slack for long UNC names.
const PROCESS_PATH_CHARS: usize = 260 + 32;

/// Which direction of an audio endpoint a session or device belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Loudspeakers / headphones — playback sessions.
    Render,
    /// Microphones / line-in — capture sessions.
    Capture,
}

impl Role {
    fn dataflow(self) -> EDataFlow {
        match self {
            Role::Render => eRender,
            Role::Capture => eCapture,
        }
    }
}

/// One live application audio session.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSession {
    /// Enumeration-local handle, valid until the next [`sessions`] call. See the module docs.
    pub id: u32,
    /// Owning process id (`0` when the audio service could not tell us).
    pub pid: u32,
    /// Per-app friendly name as registered by the application; falls back to the executable
    /// file name, then to `"System Sounds"`, then to `"(unknown)"`.
    pub display_name: String,
    /// Session volume, linear `0.0..=1.0`.
    pub volume: f32,
    /// Session mute flag.
    pub muted: bool,
    /// Post-fader peak level, `0.0..=1.0`. `0.0` when the session exposes no meter.
    pub peak: f32,
    /// `true` when the session state is `AudioSessionStateActive` (i.e. it is currently
    /// producing/consuming audio, not merely resident).
    pub active: bool,
    /// Direction of the endpoint this session belongs to.
    pub role: Role,
}

/// An audio endpoint (device) and its master controls.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    /// Device id (`{0.0.0.00000000}.{...}`); part of the persistence key.
    pub id: String,
    /// Property-store friendly name, falling back to the device id.
    pub friendly_name: String,
    /// Master volume, linear `0.0..=1.0`. `0.0` when the endpoint exposes no master volume —
    /// common for capture devices and virtual cables; see [`Endpoint::peak`] for those.
    pub volume: f32,
    /// Master mute flag. `false` when the endpoint exposes no master volume.
    pub muted: bool,
    /// Endpoint peak level, `0.0..=1.0`. This is the trustworthy signal level for capture
    /// devices and does not open a recording stream.
    pub peak: f32,
    /// `true` when this is the default endpoint *for its own data flow*.
    pub is_default: bool,
}

/// `PKEY_Device_FriendlyName` (`Devices::FunctionDiscovery`), inlined so this module does not
/// need the `Win32_Devices_FunctionDiscovery` feature. Value taken verbatim from windows 0.62:
/// `fmtid = {A45C254E-DF1C-4EFD-8020-67D146A850E0}, pid = 14`.
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

const SYSTEM_SOUNDS_NAME: &str = "System Sounds";
const UNKNOWN_NAME: &str = "(unknown)";

// ---------------------------------------------------------------------------
// COM apartment bookkeeping
// ---------------------------------------------------------------------------

/// Per-thread COM state. `depth` counts nested entries; `uninit_at_zero` records whether this
/// module took the initialization reference that should be released at depth 0 (it must not if
/// the thread was already initialized in a different apartment model, and must not at all once
/// [`init_com`] has been called, because then the caller owns the lifetime).
struct ComState {
    depth: u32,
    uninit_at_zero: bool,
    /// Set by `init_com` — the thread keeps COM initialized until it exits.
    owned_by_caller: bool,
}

thread_local! {
    static COM: Mutex<ComState> = const { Mutex::new(ComState { depth: 0, uninit_at_zero: false, owned_by_caller: false }) };
}

/// RAII guard that has initialized COM on the current thread for as long as it is alive.
struct ComGuard {
    active: bool,
}

impl ComGuard {
    /// Initializes COM on this thread if nothing is holding it open yet, then takes a nesting
    /// reference. Tolerates an apartment that is already initialized (`S_FALSE`) or initialized
    /// in the other model (`RPC_E_CHANGED_MODE`).
    fn enter() -> Result<ComGuard, String> {
        COM.with(|cell| {
            let mut state = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.depth == 0 {
                let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                if hr == RPC_E_CHANGED_MODE {
                    // Already up in another model: usable, but not ours to release.
                    state.uninit_at_zero = false;
                } else if hr.is_ok() {
                    // Both S_OK (fresh) and S_FALSE (nesting) took a reference to release.
                    state.uninit_at_zero = true;
                } else {
                    return Err(format!("CoInitializeEx failed: {hr} (0x{:08X})", hr.0));
                }
            }
            state.depth += 1;
            Ok(ComGuard { active: true })
        })
    }

    fn leave(&self) {
        if !self.active {
            return;
        }
        COM.with(|cell| {
            if let Ok(mut s) = cell.lock() {
                s.depth = s.depth.saturating_sub(1);
                if s.depth == 0 && s.uninit_at_zero && !s.owned_by_caller {
                    unsafe { CoUninitialize() };
                    s.uninit_at_zero = false;
                }
            }
        });
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

/// Initializes COM on the calling thread (multithreaded apartment) and leaves it initialized
/// for the lifetime of that thread. Reference counted, so calling it repeatedly is cheap and
/// safe. Call this once at the top of the dedicated audio thread.
pub fn init_com() -> Result<(), String> {
    COM.with(|cell| {
        let mut s = cell
            .lock()
            .map_err(|_| "COM state was poisoned by a panic".to_string())?;
        if s.depth == 0 {
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if !hr.is_ok() && hr != RPC_E_CHANGED_MODE {
                return Err(format!(
                    "CoInitializeEx failed: {hr} (0x{:08X})",
                    hr.0
                ));
            }
            // Either we took the reference (S_OK/S_FALSE) or the host already had one; in both
            // cases this module must not release it behind the caller's back.
            s.uninit_at_zero = false;
            s.owned_by_caller = true;
            s.depth = 1;
        } else {
            s.owned_by_caller = true;
            s.uninit_at_zero = false;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// id -> session identity registry
// ---------------------------------------------------------------------------

/// Stable identity of a session within a data flow, captured at enumeration time. Plain data
/// only, so a write coming from another thread can use it safely.
#[derive(Clone, Debug)]
struct SessionKey {
    pid: u32,
    /// `IAudioSessionControl2::GetSessionInstanceIdentifier` — unique per live session.
    instance_id: String,
}

/// `(role, id) -> key` for the most recent `sessions()` call of that role.
static SESSION_KEYS: LazyLock<Mutex<HashMap<(Role, u32), SessionKey>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn remember_keys(role: Role, keys: Vec<(u32, SessionKey)>) {
    if let Ok(mut map) = SESSION_KEYS.lock() {
        map.retain(|(r, _), _| *r != role);
        for (id, key) in keys {
            map.insert((role, id), key);
        }
    }
}

fn lookup_key(role: Role, id: u32) -> Result<SessionKey, String> {
    SESSION_KEYS
        .lock()
        .map_err(|_| "session registry poisoned".to_string())?
        .get(&(role, id))
        .cloned()
        .ok_or_else(|| {
            format!(
                "audio session {id} ({role:?}) is not known: call sessions({role:?}) first and \
                 use an id from that result"
            )
        })
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn fmt_err(what: &str, err: windows::core::Error) -> String {
    format!("{what}: {err} (0x{:08X})", err.code().0)
}

fn clamp01(v: f32) -> f32 {
    if !v.is_finite() {
        0.0
    } else {
        v.clamp(0.0, 1.0)
    }
}

/// Copies a `PWSTR` allocated by the callee and frees it with `CoTaskMemFree`.
fn take_pwstr(value: PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    let out = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(value.as_ptr() as *const core::ffi::c_void)) };
    out
}

/// Reads a `VT_LPWSTR` property out of a `PROPVARIANT` and releases it.
fn propvariant_lpwstr(pv: &mut PROPVARIANT) -> String {
    let out = unsafe {
        let tagged = &pv.Anonymous.Anonymous;
        if tagged.vt == VT_LPWSTR {
            let text = tagged.Anonymous.pwszVal;
            if text.is_null() {
                String::new()
            } else {
                text.to_string().unwrap_or_default()
            }
        } else {
            String::new()
        }
    };
    // The property store allocated the string; hand it back before pv itself is dropped.
    unsafe {
        let _ = PropVariantClear(pv);
    }
    out
}

/// `C:\...\foo.exe` -> `foo.exe`, lower-cased: the persistence key for an application.
fn exe_key(path_or_name: &str) -> String {
    let name = path_or_name
        .rsplit_once(['\\', '/'])
        .map(|(_, tail)| tail)
        .unwrap_or(path_or_name);
    name.trim().to_lowercase()
}

/// Device id embedded in a session instance identifier: everything before the first `|`,
/// e.g. `{0.0.0.00000000}.{a1b2...}`. Empty when the format is unexpected.
fn endpoint_id_from_instance_id(instance_id: &str) -> String {
    match instance_id.split_once('|') {
        Some((head, _)) if head.starts_with('{') => head.to_string(),
        _ => String::new(),
    }
}

/// Picks the name to show for a session: the app-registered display name when it is usable,
/// otherwise `"System Sounds"` for the audio service's own sessions, otherwise the executable
/// file name, otherwise `"(unknown)"`.
fn resolve_display_name(raw: &str, system_sounds: bool, pid: u32, exe: &str) -> String {
    // Empty names and raw MUI resource references (`@%SystemRoot%\...,-202`) are both unusable.
    if !raw.trim().is_empty() && !raw.starts_with('@') {
        return raw.to_string();
    }
    if system_sounds || pid == 0 {
        return SYSTEM_SOUNDS_NAME.to_string();
    }
    if !exe.is_empty() {
        return exe.to_string();
    }
    UNKNOWN_NAME.to_string()
}

/// Lower-cased comparison of endpoint ids (device ids are GUID-shaped; Windows treats paths and
/// GUIDs case-insensitively).
fn same_endpoint(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && a.eq_ignore_ascii_case(b)
}

// ---------------------------------------------------------------------------
// enumerator / device plumbing
// ---------------------------------------------------------------------------

/// `CLSID_MMDeviceEnumerator`.
///
/// **Note (verified on this machine with windows 0.62.2):** there is no coclass wrapper type,
/// but the crate does export the class GUID as `Media::Audio::MMDeviceEnumerator`
/// (`{BCDE0395-E52F-467C-8E3D-C4579291692E}`), and that is the value actually registered by
/// `mmdevapi.dll`. The value published in `mmdeviceapi.h`
/// (`{BCDE0395-DDD5-4B20-99CB-BB4619D04AFA}`) is **not registered** here and fails `
/// CoCreateInstance` with `REGDB_E_CLASSNOTREG` (0x80040154), so hand-defining the classic
/// CLSID would break at runtime. `IMMDeviceEnumerator::IID` is supplied by `CoCreateInstance`
/// itself, so no IID constant needs to be hand-written either.
fn create_enumerator() -> Result<IMMDeviceEnumerator, String> {
    unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|e| fmt_err("CoCreateInstance(MMDeviceEnumerator)", e))
}

fn default_device(en: &IMMDeviceEnumerator, role: Role) -> Result<IMMDevice, String> {
    unsafe { en.GetDefaultAudioEndpoint(role.dataflow(), eConsole) }
        .map_err(|e| fmt_err("GetDefaultAudioEndpoint", e))
}

fn device_by_id(en: &IMMDeviceEnumerator, id: &str) -> Result<IMMDevice, String> {
    let raw = wide(id);
    unsafe { en.GetDevice(PCWSTR(raw.as_ptr())) }
        .map_err(|e| fmt_err("GetDevice", e))
}

fn device_id(device: &IMMDevice) -> String {
    unsafe { device.GetId() }
        .map(take_pwstr)
        .unwrap_or_default()
}

fn friendly_name(device: &IMMDevice) -> String {
    let store: IPropertyStore = match unsafe { device.OpenPropertyStore(STGM_READ) } {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    match unsafe { store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME) } {
        // `GetValue` hands us an owned PROPVARIANT; `propvariant_lpwstr` clears it.
        Ok(mut pv) => propvariant_lpwstr(&mut pv),
        Err(_) => String::new(),
    }
}

/// Session volume/meter interface activation is per-session best effort.
fn endpoint_volume(device: &IMMDevice) -> Result<IAudioEndpointVolume, String> {
    unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }
        .map_err(|e| fmt_err("Activate(IAudioEndpointVolume)", e))
}

fn endpoint_meter(device: &IMMDevice) -> Result<IAudioMeterInformation, String> {
    unsafe { device.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) }
        .map_err(|e| fmt_err("Activate(IAudioMeterInformation)", e))
}

fn build_endpoint(device: &IMMDevice, is_default: bool) -> Endpoint {
    let id = device_id(device);
    let name = friendly_name(device);
    let (volume, muted) = match endpoint_volume(device) {
        Ok(vol) => {
            let v = unsafe { vol.GetMasterVolumeLevelScalar() }.unwrap_or(0.0);
            let m = unsafe { vol.GetMute() }.map(|flag| flag.0 != 0).unwrap_or(false);
            (clamp01(v), m)
        }
        // Many capture endpoints and virtual drivers expose no master volume at all.
        Err(_) => (0.0, false),
    };
    let peak = endpoint_meter(device)
        .ok()
        .and_then(|meter| unsafe { meter.GetPeakValue() }.ok())
        .map(clamp01)
        .unwrap_or(0.0);
    Endpoint {
        id,
        friendly_name: if name.is_empty() {
            UNKNOWN_NAME.to_string()
        } else {
            name
        },
        volume,
        muted,
        peak,
        is_default,
    }
}

// ---------------------------------------------------------------------------
// session enumeration
// ---------------------------------------------------------------------------

/// Everything one live session can tell us, plus the still-alive volume control needed by the
/// write path. `ctl` is only valid while the enclosing enumeration is running, which is why it
/// never escapes `visit`.
struct SessionProbe<'a> {
    info: SessionInfo,
    ctl: Option<&'a ISimpleAudioVolume>,
}

#[derive(Clone, Debug)]
struct SessionInfo {
    index: u32,
    pid: u32,
    exe: String,
    display_name: String,
    volume: f32,
    muted: bool,
    peak: f32,
    active: bool,
    instance_id: String,
    endpoint_id: String,
}

/// Enumerates every session attached to the default endpoint of `role` and calls `visit` for
/// each, with the session's COM volume control still alive. Must be called with COM initialized
/// on the current thread (i.e. inside a [`ComGuard`]).
fn with_sessions<F>(role: Role, mut visit: F) -> Result<(), String>
where
    F: FnMut(&SessionProbe<'_>),
{
    let en = create_enumerator()?;
    let device = default_device(&en, role)?;
    let manager: IAudioSessionManager2 = unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| fmt_err("Activate(IAudioSessionManager2)", e))?;
    let enumerator = unsafe { manager.GetSessionEnumerator() }
        .map_err(|e| fmt_err("GetSessionEnumerator", e))?;
    // GetCount is an i32; it can transiently be 0 while the audio service rebuilds its list.
    let count = unsafe { enumerator.GetCount() }.map_err(|e| fmt_err("GetCount", e))?;

    for index in 0..count.max(0) {
        let control: IAudioSessionControl = match unsafe { enumerator.GetSession(index) } {
            Ok(c) => c,
            Err(_) => continue, // session died mid-sweep
        };

        let state = unsafe { control.GetState() }.unwrap_or_default();
        let active = state == AudioSessionStateActive;
        let raw_name = unsafe { control.GetDisplayName() }
            .map(take_pwstr)
            .unwrap_or_default();
        // One QueryInterface for the v2 interface, reused below.
        let control2: Option<IAudioSessionControl2> = control.cast().ok();
        let instance_id = control2
            .as_ref()
            .and_then(|c2| unsafe { c2.GetSessionInstanceIdentifier() }.ok())
            .map(take_pwstr)
            .unwrap_or_default();
        let (pid, system_sounds) = match control2.as_ref() {
            Some(c2) => (
                unsafe { c2.GetProcessId() }.unwrap_or(0),
                // IsSystemSoundsSession returns S_OK for the system sounds session and S_FALSE
                // (which is also "ok" as an HRESULT) for everything else, so compare to zero.
                unsafe { c2.IsSystemSoundsSession() }.0 == 0,
            ),
            None => (0, false),
        };

        let simple: Option<ISimpleAudioVolume> = control.cast().ok();
        let volume = simple
            .as_ref()
            .and_then(|v| unsafe { v.GetMasterVolume() }.ok())
            .map(clamp01)
            .unwrap_or(0.0);
        let muted = simple
            .as_ref()
            .and_then(|v| unsafe { v.GetMute() }.ok())
            .map(|flag| flag.0 != 0)
            .unwrap_or(false);
        let peak = match control.cast::<IAudioMeterInformation>() {
            Ok(meter) => unsafe { meter.GetPeakValue() }
                .map(clamp01)
                .unwrap_or(0.0),
            Err(_) => 0.0,
        };

        let exe = if pid == 0 {
            String::new()
        } else {
            exe_name(pid).unwrap_or_default()
        };
        // Applications may register no name at all, and the audio service hands back raw MUI
        // resource references (`@%SystemRoot%\System32\AudioSrv.Dll,-202`) for its own sessions.
        // Both are useless in a mixer UI, so fall back as documented.
        let display_name = resolve_display_name(&raw_name, system_sounds, pid, &exe);

        let endpoint_id = endpoint_id_from_instance_id(&instance_id);
        let info = SessionInfo {
            index: index as u32,
            pid,
            exe,
            display_name,
            volume,
            muted,
            peak,
            active,
            instance_id,
            endpoint_id,
        };
        visit(&SessionProbe {
            info,
            ctl: simple.as_ref(),
        });
    }
    Ok(())
}

/// Reads a `Vec<AppSession>` and refreshes the id registry as a side effect.
fn collect_sessions(role: Role) -> Result<(Vec<AppSession>, Vec<(u32, SessionKey)>), String> {
    let mut out = Vec::new();
    let mut keys = Vec::new();
    with_sessions(role, |probe| {
        let info = &probe.info;
        out.push(AppSession {
            id: info.index,
            pid: info.pid,
            display_name: info.display_name.clone(),
            volume: info.volume,
            muted: info.muted,
            peak: info.peak,
            active: info.active,
            role,
        });
        keys.push((
            info.index,
            SessionKey {
                pid: info.pid,
                instance_id: info.instance_id.clone(),
            },
        ));
    })?;
    Ok((out, keys))
}

/// Runs `action` against the volume control of the session that matches `key`, re-enumerating
/// first. Returns `Err` if the session is gone or the OS refuses the change.
fn with_matching_session(
    role: Role,
    key: &SessionKey,
    what: &str,
    action: impl Fn(&ISimpleAudioVolume) -> Result<(), windows::core::Error>,
) -> Result<(), String> {
    let mut result: Option<Result<(), String>> = None;
    with_sessions(role, |probe| {
        if result.is_some() {
            return;
        }
        let info = &probe.info;
        let matched = if key.instance_id.is_empty() {
            // No identifier recorded (very old drivers): fall back to the process.
            info.pid == key.pid && key.pid != 0
        } else {
            info.instance_id == key.instance_id
        };
        if !matched {
            return;
        }
        result = Some(match probe.ctl {
            Some(session) => action(session).map_err(|e| fmt_err(what, e)),
            None => Err(format!(
                "{what}: the session no longer exposes ISimpleAudioVolume \
                 (protected or exclusive-mode session)"
            )),
        });
    })?;
    result.unwrap_or_else(|| {
        Err(format!(
            "{what}: the audio session for pid {} has ended or the endpoint changed; \
             refresh the list and try again",
            key.pid
        ))
    })
}

/// `pid` -> `foo.exe` (lower-cased file name). Uses `QueryFullProcessImageNameW`, which works
/// across bitness and, with `PROCESS_QUERY_LIMITED_INFORMATION`, even for most protected
/// processes. `None` for pid 0, exited processes and access-denied cases.
pub fn exe_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let mut buffer = [0u16; PROCESS_PATH_CHARS];
    let mut size = buffer.len() as u32;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let queried = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        // HANDLE carries no Drop impl, so close it explicitly.
        let _ = CloseHandle(handle);
        if queried.is_err() {
            return None;
        }
        String::from_utf16(&buffer[..size as usize])
            .ok()
            .map(|path| exe_key(&path))
    }
}

// ---------------------------------------------------------------------------
// public read API
// ---------------------------------------------------------------------------

/// Every active endpoint of the given flow: default first, then the rest in driver order.
///
/// Read-only; a device whose master volume cannot be read reports `volume = 0.0, muted = false`
/// while still reporting a usable [`Endpoint::peak`].
pub fn endpoints_of(role: Role) -> Result<Vec<Endpoint>, String> {
    let _guard = ComGuard::enter()?;
    let en = create_enumerator()?;
    let default_id = default_device(&en, role)
        .map(|d| device_id(&d))
        .unwrap_or_default();
    let collection = unsafe { en.EnumAudioEndpoints(role.dataflow(), DEVICE_STATE_ACTIVE) }
        .map_err(|e| fmt_err("EnumAudioEndpoints", e))?;
    let count = unsafe { collection.GetCount() }.map_err(|e| fmt_err("GetCount", e))?;
    let mut out = Vec::with_capacity(count as usize);
    let mut fallback = Vec::new();
    for index in 0..count {
        let device = match unsafe { collection.Item(index) } {
            Ok(d) => d,
            Err(_) => continue,
        };
        let id = device_id(&device);
        let is_default = !default_id.is_empty() && same_endpoint(&id, &default_id);
        let endpoint = build_endpoint(&device, is_default);
        // Guarantee the default entry appears first even if the driver sorts oddly.
        if is_default {
            out.insert(0, endpoint);
        } else {
            fallback.push(endpoint);
        }
    }
    out.extend(fallback);
    Ok(out)
}

/// All active endpoints, playback first then capture.
///
/// Because [`Endpoint`] carries no flow field, prefer [`endpoints_of`] when the UI needs the
/// All audio sessions on the default endpoint of `role`, in the order the audio service
/// reports them, and refreshes the `id -> session` registry used by the write calls.
///
/// Capture sessions are best-effort "process with an open capture stream"; see module docs.
pub fn sessions(role: Role) -> Result<Vec<AppSession>, String> {
    let _guard = ComGuard::enter()?;
    let (list, keys) = collect_sessions(role)?;
    remember_keys(role, keys);
    Ok(list)
}

// ---------------------------------------------------------------------------
// public write API
// ---------------------------------------------------------------------------

/// Sets the volume of one application session (`0.0..=1.0`, clamped).
///
/// `id` must come from the most recent [`sessions`] call for the same `role`. The session is
/// located by re-enumerating and matching its recorded instance identifier, so a stale id
/// produces an error rather than adjusting a different app. The error string carries the OS
/// message (`AUDCLNT_E_DEVICE_INVALIDATED`, `E_ACCESSDENIED`, "session has ended", …) and is
/// meant to be shown to the user.
pub fn set_session_volume(role: Role, id: u32, volume: f32) -> Result<(), String> {
    let _guard = ComGuard::enter()?;
    let key = lookup_key(role, id)?;
    let value = clamp01(volume);
    with_matching_session(role, &key, "SetMasterVolume", |session| unsafe {
        session.SetMasterVolume(value, core::ptr::null())
    })
}

/// Mutes or unmutes one application session. Same id rules as [`set_session_volume`].
pub fn set_session_mute(role: Role, id: u32, muted: bool) -> Result<(), String> {
    let _guard = ComGuard::enter()?;
    let key = lookup_key(role, id)?;
    with_matching_session(role, &key, "SetMute", |session| unsafe {
        session.SetMute(muted, core::ptr::null())
    })
}

/// Sets an endpoint's master volume by device id (`0.0..=1.0`, clamped).
///
/// Note that some drivers legitimately allow values above unity; they are not reachable here
/// because the public contract is `0.0..=1.0`.
pub fn set_endpoint_volume(endpoint_id: &str, volume: f32) -> Result<(), String> {
    let _guard = ComGuard::enter()?;
    let en = create_enumerator()?;
    let device = device_by_id(&en, endpoint_id)?;
    let master = endpoint_volume(&device)?;
    unsafe {
        master
            .SetMasterVolumeLevelScalar(clamp01(volume), core::ptr::null())
            .map_err(|e| fmt_err("SetMasterVolumeLevelScalar", e))
    }
}

/// Mutes or unmutes an endpoint by device id. Included because [`Endpoint::muted`] is readable
/// and a mixer UI needs the matching control.
pub fn set_endpoint_mute(endpoint_id: &str, muted: bool) -> Result<(), String> {
    let _guard = ComGuard::enter()?;
    let en = create_enumerator()?;
    let device = device_by_id(&en, endpoint_id)?;
    let master = endpoint_volume(&device)?;
    unsafe {
        master
            .SetMute(muted, core::ptr::null())
            .map_err(|e| fmt_err("IAudioEndpointVolume::SetMute", e))
    }
}

// ---------------------------------------------------------------------------
// per-application volume persistence
// ---------------------------------------------------------------------------

/// One remembered rule: "whatever process runs as `exe` on endpoint `endpoint_id` gets this
/// volume/mute". `exe` is a lower-cased file name; `endpoint_id` is lower-cased on write.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VolumeRule {
    pub exe: String,
    pub endpoint_id: String,
    /// `0.0..=1.0`; `None` means "leave the session volume alone".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    /// `None` means "leave the session mute state alone".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct VolumeStore {
    /// Format version; bumped only on an incompatible change.
    version: u32,
    #[serde(default)]
    rules: Vec<VolumeRule>,
}

const STORE_VERSION: u32 = 1;

fn load_store(state_path: &Path) -> Result<VolumeStore, String> {
    let path = state_path;
    if !path.exists() {
        return Ok(VolumeStore {
            version: STORE_VERSION,
            rules: Vec::new(),
        });
    }
    let text =
        fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(VolumeStore {
            version: STORE_VERSION,
            rules: Vec::new(),
        });
    }
    serde_json::from_str::<VolumeStore>(&text)
        .map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Writes the store atomically (temp file in the same directory + rename).
fn save_store(state_path: &Path, store: &VolumeStore) -> Result<(), String> {
    let path = state_path;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
    }
    let mut file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio-volumes.json".to_string());
    file_name.push_str(".tmp");
    let tmp = path.with_file_name(file_name);
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| format!("serialize audio volume store: {e}"))?;
    fs::write(&tmp, json).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), path.display()))
}

fn find_rule_mut<'a>(store: &'a mut VolumeStore, exe: &str, endpoint_id: &str) -> Option<&'a mut VolumeRule> {
    store
        .rules
        .iter_mut()
        .find(|r| r.exe == exe && r.endpoint_id == endpoint_id)
}

/// One read-modify-write of the store at a time. The commands that end up here run on worker
/// threads, so two of them (a mute on one row while another row is dragged) can overlap, and they
/// would share the temp file and lose each other's rule.
static STORE_WRITE: Mutex<()> = Mutex::new(());

/// Remembers the volume/mute for `exe_name` on `endpoint_id`, creating the state file's parent
/// directories as needed. `exe_name` may be a full path or a bare file name; it is normalized
/// to a lower-cased file name. Passing an empty `endpoint_id` means "every endpoint". Call this
/// right after a successful [`set_session_volume`] / [`set_session_mute`] so the store mirrors
/// what the user asked for.
///
/// Errors mean the preference was **not** persisted (bad path, permissions, disk full) — show
/// them; the live session has already been changed at that point.
pub fn remember_volume(
    state_path: &Path,
    exe_name: &str,
    endpoint_id: &str,
    volume: f32,
    muted: bool,
) -> Result<(), String> {
    let exe = exe_key(exe_name);
    if exe.is_empty() {
        return Err("remember_volume: empty application name".to_string());
    }
    let endpoint = if endpoint_id.is_empty() {
        String::new()
    } else {
        endpoint_id.to_lowercase()
    };
    let _only_writer = STORE_WRITE.lock().unwrap_or_else(|p| p.into_inner());
    let mut store = load_store(state_path)?;
    let volume = Some(clamp01(volume));
    let muted = Some(muted);
    if let Some(rule) = find_rule_mut(&mut store, &exe, &endpoint) {
        rule.volume = volume;
        rule.muted = muted;
    } else {
        store.rules.push(VolumeRule {
            exe,
            endpoint_id: endpoint,
            volume,
            muted,
        });
    }
    store.version = STORE_VERSION;
    save_store(state_path, &store)
}

/// What a remembered rule asks of one session.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Adjustment {
    volume: Option<f32>,
    muted: Option<bool>,
}

/// The write that makes a session match the rule for its `exe` and endpoint, or `None` when no
/// rule applies or the session already matches (a value that is already right is not written:
/// a write is a change the app and the user can both see).
///
/// A rule for the session's own endpoint wins over one for "every endpoint" (empty endpoint),
/// whichever is listed first. A session whose endpoint cannot be determined only ever matches
/// the latter, so nothing is guessed across devices.
fn adjustment_for(
    rules: &[VolumeRule],
    exe: &str,
    endpoint_id: &str,
    volume: f32,
    muted: bool,
) -> Option<Adjustment> {
    let for_exe = |rule: &&VolumeRule| rule.exe.eq_ignore_ascii_case(exe);
    let rule = rules
        .iter()
        .filter(for_exe)
        .find(|rule| same_endpoint(&rule.endpoint_id, endpoint_id))
        .or_else(|| rules.iter().filter(for_exe).find(|rule| rule.endpoint_id.is_empty()))?;
    let volume = rule.volume.filter(|wanted| (wanted - volume).abs() > 0.001);
    let muted = rule.muted.filter(|wanted| *wanted != muted);
    (volume.is_some() || muted.is_some()).then_some(Adjustment { volume, muted })
}

fn write_adjustment(session: &ISimpleAudioVolume, change: Adjustment) -> Result<(), String> {
    let mut errs = Vec::new();
    if let Some(volume) = change.volume {
        if let Err(e) = unsafe { session.SetMasterVolume(clamp01(volume), core::ptr::null()) } {
            errs.push(fmt_err("SetMasterVolume", e));
        }
    }
    if let Some(muted) = change.muted {
        if let Err(e) = unsafe { session.SetMute(muted, core::ptr::null()) } {
            errs.push(fmt_err("SetMute", e));
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// How often the watcher looks for new sessions. An app that starts playing is at its own
/// volume until the next look, so this is also the longest a restored volume is late.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Sweeps a session may be absent from the enumeration before it is forgotten. The audio
/// service can report an empty list for a moment while it rebuilds it; forgetting everything
/// then would make every session look new, and the rules would be applied over whatever the
/// user had set since.
const FORGET_AFTER_SWEEPS: u64 = 30;

/// The sessions that have been met, so a rule is applied to a session once — when it appears —
/// and never again over what the user does to it afterwards.
struct SeenSessions {
    sweep: u64,
    last_met: HashMap<(Role, String), u64>,
}

impl SeenSessions {
    fn new() -> Self {
        Self { sweep: 0, last_met: HashMap::new() }
    }

    /// True the first time this session is met; every call records that it was met now.
    fn is_new(&mut self, role: Role, session: &str) -> bool {
        self.last_met.insert((role, session.to_string()), self.sweep).is_none()
    }

    /// Ends a sweep of the enumeration, forgetting sessions that have not been met for a while.
    fn end_sweep(&mut self) {
        let now = self.sweep;
        self.last_met.retain(|_, met| now - *met < FORGET_AFTER_SWEEPS);
        self.sweep += 1;
    }
}

/// One session's identity for [`SeenSessions`]: the instance identifier, which is unique per
/// live session (a restarted app, a stream opened again, and a session moved to another device
/// each get a new one); the process when a driver gives none.
fn session_identity(info: &SessionInfo) -> String {
    if info.instance_id.is_empty() {
        format!("pid:{}", info.pid)
    } else {
        info.instance_id.clone()
    }
}

/// One look at the live sessions of both directions: every session not met before gets the rule
/// that matches it. Returns how many were adjusted; an `Err` only when something failed and
/// nothing was adjusted, which keeps a partial success usable.
fn restore_new_sessions(rules: &[VolumeRule], seen: &mut SeenSessions) -> Result<usize, String> {
    let _guard = ComGuard::enter()?;
    let mut applied = 0usize;
    let mut problems: Vec<String> = Vec::new();

    for role in [Role::Render, Role::Capture] {
        let looked = with_sessions(role, |probe| {
            let info = &probe.info;
            // No executable, no rule: it may be resolvable next time, so it is not recorded.
            if info.exe.is_empty() || !seen.is_new(role, &session_identity(info)) {
                return;
            }
            let Some(change) = adjustment_for(rules, &info.exe, &info.endpoint_id, info.volume, info.muted)
            else {
                return;
            };
            let Some(session) = probe.ctl else {
                problems.push(format!("{}: session exposes no volume control", info.display_name));
                return;
            };
            match write_adjustment(session, change) {
                Ok(()) => applied += 1,
                Err(e) => problems.push(format!("{}: {e}", info.display_name)),
            }
        });
        // No default device in this direction (a machine with no microphone) is nothing to
        // look at, and not a reason to skip the other direction.
        if let Err(e) = looked {
            problems.push(format!("{role:?}: {e}"));
        }
    }
    seen.end_sweep();

    if applied == 0 && !problems.is_empty() {
        return Err(problems.join(" | "));
    }
    Ok(applied)
}

static WATCHER: OnceLock<()> = OnceLock::new();

/// Looks for sessions that have appeared since the last look, every [`WATCH_INTERVAL`], for the
/// life of the process. The rules are read again each time, so one remembered after start-up
/// applies to the next session of that app. Failures are printed when they change, not every
/// two seconds.
fn watch(state_path: PathBuf, mut seen: SeenSessions) {
    if let Err(e) = init_com() {
        eprintln!("audio: the volume watcher could not start COM: {e}");
        return;
    }
    let mut rules: Vec<VolumeRule> = Vec::new();
    let mut last_problem = String::new();
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        match load_store(&state_path) {
            Ok(store) => rules = store.rules,
            Err(e) => {
                if e != last_problem {
                    eprintln!("audio: saved volumes not read, keeping the last ones: {e}");
                    last_problem = e;
                }
            }
        }
        match restore_new_sessions(&rules, &mut seen) {
            Ok(0) => last_problem.clear(),
            Ok(count) => {
                last_problem.clear();
                eprintln!("audio: restored the saved volume of {count} new session(s)");
            }
            Err(e) => {
                if e != last_problem {
                    eprintln!("audio: restoring saved volumes: {e}");
                    last_problem = e;
                }
            }
        }
    }
}

/// Re-applies remembered volumes to the sessions that are live now, and keeps doing so for
/// sessions that appear later.
///
/// Windows drops per-app volume when the process exits, so this is what makes a setting
/// "stick": for each session whose `exe + endpoint` has a rule and whose current volume or
/// mute differs from it, the value is written again. The first call restores what is already
/// playing and returns the number of sessions adjusted; it also starts, once per process, a
/// background thread that looks at the sessions every couple of seconds and applies the rules
/// to the ones it has not met before. A session is only ever adjusted when it is first met, so
/// a volume changed afterwards, in Pulse or in Windows, is left alone. Later calls only
/// restore once more; they do not start another watcher, and the first call's `state_path` is
/// the one it keeps reading.
///
/// A session whose endpoint cannot be determined (unexpected instance-identifier format) is
/// matched on `exe` alone against rules with an empty endpoint, so nothing is guessed across
/// devices. Individual write failures are collected and reported in an `Err` only if *no*
/// session could be adjusted, keeping the common partial-success case usable.
pub fn apply_saved_volumes(state_path: &Path) -> Result<usize, String> {
    let mut seen = SeenSessions::new();
    let first = load_store(state_path).and_then(|store| restore_new_sessions(&store.rules, &mut seen));

    if WATCHER.set(()).is_ok() {
        let path = state_path.to_path_buf();
        let started = std::thread::Builder::new()
            .name("pulse-audio-restore".into())
            .spawn(move || watch(path, seen));
        if let Err(e) = started {
            eprintln!("audio: the volume watcher could not start: {e}");
        }
    }
    first
}

// ---------------------------------------------------------------------------
// tests (pure logic only — no COM, so they run in `cargo test` on any host)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_key_accepts_paths_and_case() {
        assert_eq!(exe_key("C:\\Program Files\\Foo\\Chrome.EXE"), "chrome.exe");
        assert_eq!(exe_key("chrome.exe"), "chrome.exe");
        assert_eq!(exe_key("/opt/bin/App"), "app");
        assert_eq!(exe_key("   "), "");
    }

    #[test]
    fn endpoint_id_is_the_leading_guid_blob() {
        let instance = "{0.0.0.00000000}.{9a1b2c3d-1111-2222-3333-444455556666}|\
                        App::sysaudiodrv.dll,204|Category\\Audio|Role\\0|\r|Process:4242|\r";
        assert_eq!(
            endpoint_id_from_instance_id(instance),
            "{0.0.0.00000000}.{9a1b2c3d-1111-2222-3333-444455556666}"
        );
        assert_eq!(endpoint_id_from_instance_id(""), "");
        assert_eq!(endpoint_id_from_instance_id("nonsense"), "");
    }

    #[test]
    fn same_endpoint_ignores_case() {
        assert!(same_endpoint("{0.0.0.00000000}.{AA}", "{0.0.0.00000000}.{aa}"));
        assert!(!same_endpoint("", "{0.0.0.00000000}.{aa}"));
        assert!(!same_endpoint("{a}", "{b}"));
    }

    #[test]
    fn clamp01_rejects_nan_and_out_of_range() {
        assert_eq!(clamp01(-1.0), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
        assert_eq!(clamp01(f32::NAN), 0.0);
        assert_eq!(clamp01(0.3), 0.3);
    }

    #[test]
    fn store_roundtrip_and_upsert() {
        let dir = std::env::temp_dir().join(format!("pulse-audio-test-{}", std::process::id()));
        let path = dir.join("volumes.json");
        let _ = fs::remove_dir_all(&dir);

        remember_volume(&path, "C:\\x\\Chrome.exe", "{0.0.0.00000000}.{AAA}", 0.3, true).unwrap();
        remember_volume(&path, "chrome.exe", "{0.0.0.00000000}.{aaa}", 0.7, false).unwrap();
        remember_volume(&path, "spotify.exe", "{0.0.0.00000000}.{AAA}", 0.2, false).unwrap();

        let store = load_store(&path).unwrap();
        assert_eq!(store.version, STORE_VERSION);
        assert_eq!(store.rules.len(), 2, "upsert must not duplicate a rule");
        let chrome = store.rules.iter().find(|r| r.exe == "chrome.exe").unwrap();
        assert_eq!(chrome.volume, Some(0.7));
        assert_eq!(chrome.muted, Some(false));
        // Endpoint ids are normalized, so a remembered rule matches a session id of any case.
        assert_eq!(chrome.endpoint_id, "{0.0.0.00000000}.{aaa}");

        // A corrupt file must be reported, not silently ignored.
        fs::write(&path, "{ not json").unwrap();
        assert!(load_store(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn display_name_falls_back_in_the_documented_order() {
        assert_eq!(resolve_display_name("Google Chrome", false, 12, "chrome.exe"), "Google Chrome");
        // Unregistered name -> executable file name.
        assert_eq!(resolve_display_name("", false, 12, "chrome.exe"), "chrome.exe");
        assert_eq!(resolve_display_name("   ", false, 12, "chrome.exe"), "chrome.exe");
        // Unresolved MUI reference from the audio service -> "System Sounds".
        assert_eq!(
            resolve_display_name("@%SystemRoot%\\System32\\AudioSrv.Dll,-202", false, 0, ""),
            "System Sounds"
        );
        assert_eq!(resolve_display_name("", true, 4, ""), "System Sounds");
        // Nothing to go on at all.
        assert_eq!(resolve_display_name("", false, 42, ""), "(unknown)");
    }

    #[test]
    fn missing_state_file_is_not_an_error() {
        let path = std::env::temp_dir().join("pulse-audio-does-not-exist/none.json");
        assert_eq!(load_store(&path).unwrap().rules.len(), 0);
    }

    const SPEAKERS: &str = "{0.0.0.00000000}.{aaaa}";
    const HEADSET: &str = "{0.0.0.00000000}.{bbbb}";

    fn rule(exe: &str, endpoint: &str, volume: Option<f32>, muted: Option<bool>) -> VolumeRule {
        VolumeRule { exe: exe.to_string(), endpoint_id: endpoint.to_string(), volume, muted }
    }

    #[test]
    fn a_rule_applies_to_its_exe_on_its_endpoint() {
        let rules = [rule("chrome.exe", SPEAKERS, Some(0.3), Some(false))];
        assert_eq!(
            adjustment_for(&rules, "chrome.exe", SPEAKERS, 1.0, false),
            Some(Adjustment { volume: Some(0.3), muted: None })
        );
        // Endpoint ids are compared ignoring case, as Windows does.
        assert!(adjustment_for(&rules, "chrome.exe", &SPEAKERS.to_uppercase(), 1.0, false).is_some());
        assert_eq!(adjustment_for(&rules, "chrome.exe", HEADSET, 1.0, false), None, "another device");
        assert_eq!(adjustment_for(&rules, "firefox.exe", SPEAKERS, 1.0, false), None, "another app");
    }

    #[test]
    fn an_empty_endpoint_means_every_endpoint() {
        let rules = [rule("chrome.exe", "", Some(0.5), None)];
        for endpoint in [SPEAKERS, HEADSET, ""] {
            assert_eq!(
                adjustment_for(&rules, "chrome.exe", endpoint, 1.0, false),
                Some(Adjustment { volume: Some(0.5), muted: None }),
                "{endpoint:?}"
            );
        }
    }

    #[test]
    fn a_session_whose_endpoint_is_unknown_only_matches_rules_for_every_endpoint() {
        let rules = [rule("chrome.exe", SPEAKERS, Some(0.3), None)];
        assert_eq!(adjustment_for(&rules, "chrome.exe", "", 1.0, false), None);
    }

    #[test]
    fn the_rule_for_the_sessions_own_endpoint_beats_the_rule_for_all() {
        let rules = [
            rule("chrome.exe", "", Some(0.9), None),
            rule("chrome.exe", HEADSET, Some(0.2), None),
        ];
        assert_eq!(
            adjustment_for(&rules, "chrome.exe", HEADSET, 1.0, false).and_then(|a| a.volume),
            Some(0.2)
        );
        assert_eq!(
            adjustment_for(&rules, "chrome.exe", SPEAKERS, 1.0, false).and_then(|a| a.volume),
            Some(0.9),
            "a device with no rule of its own takes the general one"
        );
    }

    #[test]
    fn a_session_that_already_matches_is_not_written() {
        let rules = [rule("chrome.exe", SPEAKERS, Some(0.3), Some(true))];
        assert_eq!(adjustment_for(&rules, "chrome.exe", SPEAKERS, 0.3, true), None);
        // Volumes come back from the audio engine a hair off; that is still equal.
        assert_eq!(adjustment_for(&rules, "chrome.exe", SPEAKERS, 0.3004, true), None);
        // Only the half that differs is asked for.
        assert_eq!(
            adjustment_for(&rules, "chrome.exe", SPEAKERS, 0.3, false),
            Some(Adjustment { volume: None, muted: Some(true) })
        );
        assert_eq!(
            adjustment_for(&rules, "chrome.exe", SPEAKERS, 0.8, true),
            Some(Adjustment { volume: Some(0.3), muted: None })
        );
    }

    #[test]
    fn a_rule_that_leaves_one_half_alone_leaves_it_alone() {
        let rules = [rule("chrome.exe", SPEAKERS, None, Some(true))];
        assert_eq!(
            adjustment_for(&rules, "chrome.exe", SPEAKERS, 0.77, false),
            Some(Adjustment { volume: None, muted: Some(true) })
        );
        assert_eq!(adjustment_for(&rules, "chrome.exe", SPEAKERS, 0.77, true), None);
    }

    #[test]
    fn a_session_is_new_once_and_the_rule_is_not_applied_again_over_the_user() {
        let mut seen = SeenSessions::new();
        assert!(seen.is_new(Role::Render, "session-1"));
        seen.end_sweep();
        // The user changes its volume; the next looks find the same session and leave it be.
        for _ in 0..5 {
            assert!(!seen.is_new(Role::Render, "session-1"));
            seen.end_sweep();
        }
        // The same identifier in the other direction is another session.
        assert!(seen.is_new(Role::Capture, "session-1"));
        assert!(seen.is_new(Role::Render, "session-2"));
    }

    #[test]
    fn a_session_is_remembered_through_a_short_gap_and_forgotten_after_a_long_one() {
        let mut seen = SeenSessions::new();
        assert!(seen.is_new(Role::Render, "a"));
        seen.end_sweep();
        // The audio service reports nothing for a few sweeps while it rebuilds its list.
        for _ in 0..FORGET_AFTER_SWEEPS - 2 {
            seen.end_sweep();
        }
        assert!(!seen.is_new(Role::Render, "a"), "still known: it would be restored over the user");
        seen.end_sweep();
        for _ in 0..FORGET_AFTER_SWEEPS {
            seen.end_sweep();
        }
        assert!(seen.is_new(Role::Render, "a"), "gone long enough to be a new session");
    }

    #[test]
    fn a_session_with_no_instance_identifier_is_identified_by_its_process() {
        let info = |instance_id: &str, pid: u32| SessionInfo {
            index: 0,
            pid,
            exe: "chrome.exe".into(),
            display_name: "Chrome".into(),
            volume: 1.0,
            muted: false,
            peak: 0.0,
            active: true,
            instance_id: instance_id.into(),
            endpoint_id: String::new(),
        };
        assert_eq!(session_identity(&info("{0.0.0.00000000}.{aaaa}|x|1", 10)), "{0.0.0.00000000}.{aaaa}|x|1");
        assert_eq!(session_identity(&info("", 4242)), "pid:4242");
    }
}
