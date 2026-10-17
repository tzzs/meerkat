//! The things Windows wrote down that explain a stretch of the timeline: an app that crashed or
//! stopped responding, a blue screen, power that went away, the machine going to sleep and what
//! woke it, a graphics driver that had to be reset, a hardware error, an update installed.
//!
//! All of it comes from the System and Application logs, which any signed-in user may read.
//! Windows' own Reliability Monitor shows part of this, a few clicks deep and never next to what
//! the machine was doing at the time; here it sits on the same axis as the CPU and memory.

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::System::EventLog::{
    EvtClose, EvtNext, EvtQuery, EvtQueryChannelPath, EvtQueryReverseDirection, EvtRender, EvtRenderEventXml,
    EVT_HANDLE,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemEventKind {
    AppCrash,
    AppHang,
    BlueScreen,
    PowerLoss,
    Sleep,
    Wake,
    Boot,
    Shutdown,
    DriverReset,
    HardwareError,
    UpdateInstalled,
    UpdateFailed,
    DiskError,
    ServiceCrash,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemEvent {
    pub at_ms: u64,
    pub kind: SystemEventKind,
    /// What it happened to: the app, the driver, the update, the wake source. May be empty.
    pub subject: String,
    /// A second fact when there is one worth showing: a stop code, an error type.
    pub detail: String,
}

/// (channel, provider, event ids). Each becomes one `Select` of a structured query.
const SOURCES: &[(&str, &str, &[u32])] = &[
    ("Application", "Application Error", &[1000]),
    ("Application", "Application Hang", &[1002]),
    ("System", "Microsoft-Windows-WER-SystemErrorReporting", &[1001]),
    ("System", "Microsoft-Windows-Kernel-Power", &[41, 42, 107]),
    ("System", "Microsoft-Windows-Power-Troubleshooter", &[1]),
    ("System", "Microsoft-Windows-Kernel-General", &[12, 13]),
    ("System", "Display", &[4101]),
    ("System", "Microsoft-Windows-WHEA-Logger", &[17, 18, 19, 46, 47]),
    ("System", "Microsoft-Windows-WindowsUpdateClient", &[19, 20]),
    ("System", "disk", &[7, 51, 153]),
    ("System", "Service Control Manager", &[7031, 7034]),
];

fn query_xml(range_ms: u64) -> String {
    let mut selects = String::new();
    for (channel, provider, ids) in SOURCES {
        let ids = ids.iter().map(|id| format!("EventID={id}")).collect::<Vec<_>>().join(" or ");
        selects.push_str(&format!(
            "<Select Path=\"{channel}\">*[System[Provider[@Name='{provider}'] and ({ids}) and TimeCreated[timediff(@SystemTime) &lt;= {range_ms}]]]</Select>"
        ));
    }
    format!("<QueryList><Query Id=\"0\">{selects}</Query></QueryList>")
}

/// Every event of interest in the last `range_ms`, newest first. Empty if the log service
/// refuses, which on a normal machine it does not.
pub fn recent(range_ms: u64, limit: usize) -> Vec<SystemEvent> {
    let query: Vec<u16> = query_xml(range_ms).encode_utf16().chain(std::iter::once(0)).collect();
    let flags = EvtQueryChannelPath.0 | EvtQueryReverseDirection.0;
    let Ok(results) = (unsafe { EvtQuery(None, PCWSTR::null(), PCWSTR(query.as_ptr()), flags) }) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut handles = [0isize; 64];
    'batches: loop {
        let mut returned = 0u32;
        if unsafe { EvtNext(results, &mut handles, 2_000, 0, &mut returned) }.is_err() || returned == 0 {
            break;
        }
        for raw in &handles[..returned as usize] {
            let event = EVT_HANDLE(*raw);
            if let Some(parsed) = render(event).as_deref().and_then(parse_event) {
                out.push(parsed);
            }
            let _ = unsafe { EvtClose(event) };
        }
        if out.len() >= limit {
            break 'batches;
        }
    }
    let _ = unsafe { EvtClose(results) };
    // The query spans two channels, each in its own reverse order; one timeline needs one order.
    out.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    out.truncate(limit);
    out
}

fn render(event: EVT_HANDLE) -> Option<String> {
    let mut used = 0u32;
    let mut properties = 0u32;
    let _ = unsafe { EvtRender(None, event, EvtRenderEventXml.0, 0, None, &mut used, &mut properties) };
    if used == 0 || used > 1 << 20 {
        return None;
    }
    let mut buffer = vec![0u16; (used as usize).div_ceil(2)];
    unsafe {
        EvtRender(
            None,
            event,
            EvtRenderEventXml.0,
            (buffer.len() * 2) as u32,
            Some(buffer.as_mut_ptr().cast()),
            &mut used,
            &mut properties,
        )
    }
    .ok()?;
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// Turns one event's XML into a timeline entry. Only the handful of fields each kind needs are
/// read; the XML is Windows' own and its shape is stable.
pub fn parse_event(xml: &str) -> Option<SystemEvent> {
    let provider = attribute(xml, "Provider", "Name")?;
    let id: u32 = between(xml, "<EventID", "</EventID>")?.rsplit('>').next()?.trim().parse().ok()?;
    let at_ms = attribute(xml, "TimeCreated", "SystemTime").and_then(parse_system_time)?;
    let data = |name: &str| named_data(xml, name).unwrap_or_default();
    let first = || unnamed_data(xml, 0).unwrap_or_default();

    let (kind, subject, detail) = match (provider.as_str(), id) {
        ("Application Error", 1000) => (SystemEventKind::AppCrash, first(), unnamed_data(xml, 6).unwrap_or_default()),
        ("Application Hang", 1002) => (SystemEventKind::AppHang, first(), String::new()),
        ("Microsoft-Windows-WER-SystemErrorReporting", 1001) => {
            (SystemEventKind::BlueScreen, String::new(), stop_code(&data("param1")))
        }
        ("Microsoft-Windows-Kernel-Power", 41) => (SystemEventKind::PowerLoss, String::new(), String::new()),
        ("Microsoft-Windows-Kernel-Power", 42) => (SystemEventKind::Sleep, String::new(), String::new()),
        ("Microsoft-Windows-Kernel-Power", 107) => (SystemEventKind::Wake, String::new(), String::new()),
        ("Microsoft-Windows-Power-Troubleshooter", 1) => (SystemEventKind::Wake, data("WakeSourceText"), String::new()),
        ("Microsoft-Windows-Kernel-General", 12) => (SystemEventKind::Boot, String::new(), String::new()),
        ("Microsoft-Windows-Kernel-General", 13) => (SystemEventKind::Shutdown, String::new(), String::new()),
        ("Display", 4101) => (SystemEventKind::DriverReset, first(), String::new()),
        ("Microsoft-Windows-WHEA-Logger", _) => (SystemEventKind::HardwareError, data("ErrorSource"), data("ErrorType")),
        ("Microsoft-Windows-WindowsUpdateClient", 19) => (SystemEventKind::UpdateInstalled, data("updateTitle"), String::new()),
        ("Microsoft-Windows-WindowsUpdateClient", 20) => (SystemEventKind::UpdateFailed, data("updateTitle"), data("errorCode")),
        ("disk", _) => (SystemEventKind::DiskError, first(), String::new()),
        ("Service Control Manager", _) => (SystemEventKind::ServiceCrash, data("param1"), String::new()),
        _ => return None,
    };
    Some(SystemEvent { at_ms, kind, subject: unescape(&subject), detail: unescape(&detail) })
}

fn between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)? + start;
    Some(&haystack[start..end])
}

/// `<Element ... Attr='value'` with either quote, as Windows writes both.
fn attribute(xml: &str, element: &str, attr: &str) -> Option<String> {
    let start = xml.find(&format!("<{element} "))?;
    let tag = &xml[start..start + xml[start..].find('>')?];
    for quote in ['\'', '"'] {
        let needle = format!("{attr}={quote}");
        if let Some(at) = tag.find(&needle) {
            let rest = &tag[at + needle.len()..];
            return Some(rest[..rest.find(quote)?].to_string());
        }
    }
    None
}

fn named_data(xml: &str, name: &str) -> Option<String> {
    for quote in ['\'', '"'] {
        let open = format!("<Data Name={quote}{name}{quote}>");
        if let Some(value) = between(xml, &open, "</Data>") {
            return Some(value.to_string());
        }
    }
    None
}

/// The n-th `<Data>` of the event, named or not: classic providers (Application Error, Display)
/// write their parameters without names, and the first is the one that says what failed.
fn unnamed_data(xml: &str, index: usize) -> Option<String> {
    let body = between(xml, "<EventData>", "</EventData>")?;
    let mut rest = body;
    let mut seen = 0;
    while let Some(at) = rest.find("<Data") {
        rest = &rest[at + 5..];
        let close = rest.find('>')?;
        // A self-closing `<Data/>` is an empty parameter.
        let value = if rest[..close].ends_with('/') {
            String::new()
        } else {
            let body = &rest[close + 1..];
            body[..body.find("</Data>")?].to_string()
        };
        if seen == index {
            return Some(value);
        }
        seen += 1;
    }
    None
}

/// `0x0000009f (0x..., ...)` → `0x0000009F`: the stop code is what people search for.
fn stop_code(param: &str) -> String {
    param.split_whitespace().next().unwrap_or("").to_uppercase().replacen("0X", "0x", 1)
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// `2026-10-12T17:50:17.1234567Z` as Unix milliseconds.
fn parse_system_time(text: String) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (year, month, day) = (d.next()??, d.next()??, d.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = clock.split(':').map(|p| p.parse::<i64>().ok());
    let (hour, minute, second) = (t.next()??, t.next()??, t.next()??);
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    let ms = ((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1000) + millis;
    u64::try_from(ms).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRASH: &str = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Application Error' Guid='{a0e9b465-b939-57d7-b27d-95d8e925ff57}'/><EventID Qualifiers='0'>1000</EventID><Level>2</Level><TimeCreated SystemTime='2026-10-12T13:14:15.5000000Z'/><Channel>Application</Channel></System><EventData><Data Name='AppName'>DeltaForceClient-Win64-Shipping.exe</Data><Data Name='AppVersion'>1.0.0.0</Data><Data Name='AppTimeStamp'>00000000</Data><Data Name='ModuleName'>nvwgf2umx.dll</Data><Data Name='ModuleVersion'>32.0.15.6094</Data><Data Name='ModTimeStamp'>00000000</Data><Data Name='ExceptionCode'>c0000005</Data></EventData></Event>"#;

    const WAKE: &str = r#"<Event><System><Provider Name="Microsoft-Windows-Power-Troubleshooter" Guid="{cdc05e28}"/><EventID>1</EventID><TimeCreated SystemTime="2026-10-12T19:02:03.0000000Z"/></System><EventData><Data Name="SleepTime">2026-10-12T15:00:00Z</Data><Data Name="WakeSourceText">Intel(R) Ethernet Controller I226-V</Data></EventData></Event>"#;

    const RESET: &str = r#"<Event><System><Provider Name='Display'/><EventID Qualifiers='0'>4101</EventID><TimeCreated SystemTime='2026-10-12T13:14:00Z'/></System><EventData><Data>nvlddmkm</Data><Data></Data></EventData></Event>"#;

    const BUGCHECK: &str = r#"<Event><System><Provider Name='Microsoft-Windows-WER-SystemErrorReporting' Guid='{abce23e7}' EventSourceName='BugCheck'/><EventID Qualifiers='16384'>1001</EventID><TimeCreated SystemTime='2026-10-01T02:00:00.000Z'/></System><EventData><Data Name='param1'>0x0000009f (0x0000000000000003, 0xffffb38f)</Data><Data Name='param2'>C:\Windows\MEMORY.DMP</Data></EventData></Event>"#;

    #[test]
    fn an_app_crash_names_the_app_and_the_module() {
        let event = parse_event(CRASH).unwrap();
        assert_eq!(event.kind, SystemEventKind::AppCrash);
        assert_eq!(event.subject, "DeltaForceClient-Win64-Shipping.exe");
        assert_eq!(event.detail, "c0000005", "the seventh parameter is the exception code");
        assert_eq!(event.at_ms % 1000, 500, "milliseconds survive");
    }

    #[test]
    fn a_wake_names_what_woke_the_machine() {
        let event = parse_event(WAKE).unwrap();
        assert_eq!(event.kind, SystemEventKind::Wake);
        assert_eq!(event.subject, "Intel(R) Ethernet Controller I226-V");
    }

    #[test]
    fn a_driver_reset_names_the_driver() {
        let event = parse_event(RESET).unwrap();
        assert_eq!((event.kind, event.subject.as_str()), (SystemEventKind::DriverReset, "nvlddmkm"));
    }

    #[test]
    fn a_blue_screen_carries_its_stop_code() {
        let event = parse_event(BUGCHECK).unwrap();
        assert_eq!((event.kind, event.detail.as_str()), (SystemEventKind::BlueScreen, "0x0000009F"));
    }

    #[test]
    fn unknown_providers_are_ignored() {
        assert!(parse_event(&CRASH.replace("Application Error", "Something Else")).is_none());
    }

    #[test]
    fn the_query_escapes_its_comparison() {
        let xml = query_xml(1000);
        assert!(xml.contains("&lt;= 1000"), "a raw < would make the query XML invalid");
        assert_eq!(xml.matches("<Select").count(), SOURCES.len());
    }

    /// Any user may read the System log, and every boot writes Kernel-General 12 to it.
    #[test]
    fn the_live_logs_answer_an_ordinary_user() {
        let events = recent(90 * 86_400_000, 500);
        assert!(events.iter().any(|e| e.kind == SystemEventKind::Boot), "this machine has booted within 90 days");
        assert!(events.windows(2).all(|w| w[0].at_ms >= w[1].at_ms), "newest first");
    }
}
