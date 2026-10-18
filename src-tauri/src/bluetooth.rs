//! Connected Bluetooth devices, and what their batteries can honestly say.
//!
//! Windows only hands a battery percentage to a device that exposes the standard GATT Battery
//! Service (0x180F) with its Battery Level characteristic (0x2A19). That covers most mice,
//! keyboards and Xbox controllers; it does not cover AirPods (Apple's own protocol), Logitech
//! devices on a 2.4 GHz receiver (not Bluetooth at all), or most headsets. For those the honest
//! answer is "does not report a battery", never a made-up 0%.
//!
//! A GATT connect can block for seconds on a device that is asleep or out of range, so each
//! read runs on its own thread with a deadline, and every `BluetoothLEDevice` is closed the
//! moment its answer is read — a handle left open leaks the radio and makes the next refresh
//! fail. Nothing here runs on the sampler thread: the tab asks, and at most every couple of
//! minutes, because a scan per second would drain the very batteries it is reporting.

use std::cmp::Ordering;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::core::{GUID, HSTRING};
use windows::Devices::Bluetooth::BluetoothConnectionStatus;
use windows::Devices::Bluetooth::BluetoothDevice;
use windows::Devices::Bluetooth::BluetoothLEDevice;
use windows::Devices::Bluetooth::GenericAttributeProfile::GattCommunicationStatus;
use windows::Devices::Enumeration::DeviceInformation;
use windows::Storage::Streams::{DataReader, IBuffer};

/// The standard GATT Battery Service, 0x180F, expanded to the 128-bit UUID WinRT expects.
const BATTERY_SERVICE: GUID = GUID::from_u128(0x0000_180f_0000_1000_8000_0080_5f9b_34fb);
/// The Battery Level characteristic inside it, 0x2A19.
const BATTERY_LEVEL: GUID = GUID::from_u128(0x0000_2a19_0000_1000_8000_0080_5f9b_34fb);

/// How long one device gets to open and answer before it is reported as not answering.
const READ_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one scan's answer is reused before the radio is asked again.
const SCAN_TTL: Duration = Duration::from_secs(120);

/// Whether a device talks the modern low-energy protocol or the older classic one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Le,
    Classic,
}

/// What a device's battery reads as, or the honest reason there is no number to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Battery {
    /// One byte Windows read from the standard 0x2A19 characteristic, already in range.
    Percent { percent: u8 },
    /// The device does not expose the standard Battery Service, so there is nothing to read.
    NoService,
    /// It exposes the service but the read produced nothing usable: an error status, a value
    /// over 100, or bytes that are not one single percent.
    Unreadable,
    /// The device did not answer within the deadline, so there is no number either way.
    NoAnswer,
}

impl Battery {
    /// The percent when there is one, so devices with a reading sort before those without.
    fn percent(self) -> Option<u8> {
        match self {
            Battery::Percent { percent } => Some(percent),
            _ => None,
        }
    }
}

/// One connected device and its battery, as the frontend draws it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub name: String,
    pub kind: Kind,
    pub connected: bool,
    pub battery: Battery,
}

/// Decides a battery reading from the two facts a GATT read establishes: whether the device has
/// the standard Battery Service, and what its level characteristic returned. This is the part
/// that can be tested without a radio.
///
/// `read_ok` is false when the device answered with an error rather than bytes; `payload` is the
/// raw 0x2A19 value, `None` when there was nothing to read. A value over 100 is a device speaking
/// outside the specification, so the honest display is "no reading" rather than a number this
/// code made up — 101 is not clamped to 100.
pub fn decide_battery(has_battery_service: bool, read_ok: bool, payload: Option<&[u8]>) -> Battery {
    if !has_battery_service {
        return Battery::NoService;
    }
    if !read_ok {
        return Battery::Unreadable;
    }
    match payload.and_then(battery_percent) {
        Some(percent) => Battery::Percent { percent },
        None => Battery::Unreadable,
    }
}

/// The percent a 0x2A19 payload means, or `None` when it is not one single byte in 0..=100.
fn battery_percent(payload: &[u8]) -> Option<u8> {
    if payload.len() != 1 {
        return None;
    }
    (payload[0] <= 100).then_some(payload[0])
}

/// The last scan and when it ran, so the tab can re-read its answer without re-opening every
/// radio link. Battery percentages move over minutes.
static LAST_SCAN: Mutex<Option<(Instant, Vec<Device>)>> = Mutex::new(None);

/// The connected devices and their batteries, reusing a scan younger than [`SCAN_TTL`]. An empty
/// list means nothing is connected, or the radio cannot be reached — not an error.
pub fn scan() -> Vec<Device> {
    if let Ok(guard) = LAST_SCAN.lock() {
        if let Some((at, devices)) = guard.as_ref() {
            if at.elapsed() < SCAN_TTL {
                return devices.clone();
            }
        }
    }
    let devices = scan_now();
    if let Ok(mut guard) = LAST_SCAN.lock() {
        *guard = Some((Instant::now(), devices.clone()));
    }
    devices
}

/// One pass over both selectors, lowest battery first. WinRT needs COM on this thread.
fn scan_now() -> Vec<Device> {
    let _ = crate::audio::init_com();
    let mut devices = Vec::new();
    devices.extend(scan_le());
    devices.extend(scan_classic());
    devices.sort_by(|a, b| match (a.battery.percent(), b.battery.percent()) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.name.cmp(&b.name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });
    devices
}

/// Low-energy devices, the ones that can carry a battery level.
fn scan_le() -> Vec<Device> {
    // Every paired device, not the "connected" selector: Windows only marks a device connected
    // while it is actively talking, so a paired controller or headset that is merely on sits
    // outside that filter and would vanish from a tab whose whole purpose is to list it. The
    // real state comes back from each device's own ConnectionStatus.
    let Ok(selector) = BluetoothLEDevice::GetDeviceSelector() else {
        return Vec::new();
    };
    let Ok(collection) = DeviceInformation::FindAllAsyncAqsFilter(&selector).and_then(|op| op.join())
    else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for info in collection {
        let Ok(id) = info.Id() else { continue };
        let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
        let (connected, battery) = read_le(&id.to_string());
        devices.push(Device { name, kind: Kind::Le, connected, battery });
    }
    devices
}

/// Classic devices, which expose no GATT battery Windows can read. They are listed so the tab is
/// complete, and each one honestly says it reports no battery rather than showing a 0%.
fn scan_classic() -> Vec<Device> {
    let Ok(selector) = BluetoothDevice::GetDeviceSelector() else {
        return Vec::new();
    };
    let Ok(collection) = DeviceInformation::FindAllAsyncAqsFilter(&selector).and_then(|op| op.join())
    else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for info in collection {
        let Ok(id) = info.Id() else { continue };
        let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
        // A classic device exposes no battery Windows can read, so its row is honest by saying
        // so; whether it is connected still comes from the device itself rather than a guess.
        let connected = BluetoothDevice::FromIdAsync(&HSTRING::from(id.to_string()))
            .and_then(|op| op.join())
            .ok()
            .and_then(|device| {
                let status = device.ConnectionStatus().ok();
                let _ = device.Close();
                status
            })
            .map(|status| status == BluetoothConnectionStatus::Connected)
            .unwrap_or(false);
        devices.push(Device { name, kind: Kind::Classic, connected, battery: Battery::NoService });
    }
    devices
}

/// Opens one low-energy device on its own thread, reads its battery with a deadline, and answers
/// `NoAnswer` when that deadline passes. A device that never answers must not hold up the rest.
fn read_le(id: &str) -> (bool, Battery) {
    let id = id.to_string();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("meerkat-bt-read".into()).spawn(move || {
        // WinRT needs COM on whichever thread touches it, and this is a fresh one.
        let _ = crate::audio::init_com();
        let _ = tx.send(read_le_inner(&id));
    });
    if spawned.is_err() {
        return (true, Battery::Unreadable);
    }
    rx.recv_timeout(READ_TIMEOUT).unwrap_or((true, Battery::NoAnswer))
}

fn read_le_inner(id: &str) -> (bool, Battery) {
    let Ok(device) = BluetoothLEDevice::FromIdAsync(&HSTRING::from(id)).and_then(|op| op.join()) else {
        return (true, Battery::Unreadable);
    };
    let connected = device
        .ConnectionStatus()
        .map(|status| status == BluetoothConnectionStatus::Connected)
        .unwrap_or(true);
    let battery = read_gatt(&device);
    // The handle must be closed, or the radio leaks and the next refresh fails.
    let _ = device.Close();
    (connected, battery)
}

/// Reads the standard battery level out of one already-open device. `NoService` when the device
/// has no Battery Service at all; `Unreadable` when it has one but nothing usable came back.
fn read_gatt(device: &BluetoothLEDevice) -> Battery {
    let Ok(result) = device.GetGattServicesForUuidAsync(BATTERY_SERVICE).and_then(|op| op.join()) else {
        return Battery::Unreadable;
    };
    let Ok(status) = result.Status() else { return Battery::Unreadable };
    if status != GattCommunicationStatus::Success {
        return Battery::Unreadable;
    }
    let Ok(services) = result.Services() else { return Battery::Unreadable };
    // No service of that UUID means the device has nothing Windows can read a battery from.
    if services.Size().unwrap_or(0) == 0 {
        return Battery::NoService;
    }
    for service in services {
        let Ok(chars) = service.GetCharacteristicsForUuidAsync(BATTERY_LEVEL).and_then(|op| op.join())
        else {
            continue;
        };
        let Ok(status) = chars.Status() else { continue };
        if status != GattCommunicationStatus::Success {
            continue;
        }
        let Ok(characteristics) = chars.Characteristics() else { continue };
        for characteristic in characteristics {
            let Ok(read) = characteristic.ReadValueAsync().and_then(|op| op.join()) else {
                continue;
            };
            let Ok(status) = read.Status() else { continue };
            if status != GattCommunicationStatus::Success {
                return Battery::Unreadable;
            }
            let Ok(buffer) = read.Value() else { return Battery::Unreadable };
            let Some(bytes) = read_bytes(&buffer) else { return Battery::Unreadable };
            return decide_battery(true, true, Some(&bytes));
        }
    }
    // The service is there but no characteristic answered with anything usable.
    Battery::Unreadable
}

/// The raw bytes of an `IBuffer`, via the `DataReader` that knows how to copy them out.
fn read_bytes(buffer: &IBuffer) -> Option<Vec<u8>> {
    let length = buffer.Length().ok()? as usize;
    let reader = DataReader::FromBuffer(buffer).ok()?;
    let mut bytes = vec![0u8; length];
    if length > 0 {
        reader.ReadBytes(&mut bytes).ok()?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_valid_level_byte_is_that_percent() {
        assert_eq!(decide_battery(true, true, Some(&[0])), Battery::Percent { percent: 0 });
        assert_eq!(decide_battery(true, true, Some(&[100])), Battery::Percent { percent: 100 });
        assert_eq!(decide_battery(true, true, Some(&[57])), Battery::Percent { percent: 57 });
    }

    #[test]
    fn an_out_of_range_value_is_not_clamped() {
        assert_eq!(decide_battery(true, true, Some(&[101])), Battery::Unreadable);
        assert_eq!(decide_battery(true, true, Some(&[255])), Battery::Unreadable);
    }

    #[test]
    fn a_two_byte_payload_is_not_a_percent() {
        assert_eq!(decide_battery(true, true, Some(&[50, 51])), Battery::Unreadable);
        assert_eq!(decide_battery(true, true, Some(&[])), Battery::Unreadable);
    }

    #[test]
    fn a_device_without_the_battery_service_reports_none() {
        assert_eq!(decide_battery(false, true, Some(&[50])), Battery::NoService);
    }

    #[test]
    fn a_device_that_answered_with_an_error_has_no_reading() {
        assert_eq!(decide_battery(true, false, None), Battery::Unreadable);
        assert_eq!(decide_battery(true, false, Some(&[50])), Battery::Unreadable);
    }
}
