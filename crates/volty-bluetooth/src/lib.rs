use std::{error::Error, fmt};

use volty_battery::{
    BatteryLevel, BatteryProvider, BatteryState, BatteryStatus, DeviceId, DeviceLabel,
    ProviderEvent, ProviderPoll, ScanMode, SinkClosed, SourceReading,
};

const BATTERY_LEVEL_MIN: u8 = 0;
const BATTERY_LEVEL_MAX: u8 = 100;
#[cfg(all(windows, feature = "windows-backend"))]
const FALLBACK_DEVICE_NAME: &str = "Bluetooth device";
/// Source label for a battery read from the standard GATT Battery Service.
pub const SOURCE_GATT: &str = "Bluetooth GATT Battery Service";
/// Source label for a battery read from a Windows device property.
pub const SOURCE_DEVICE_PROPERTY: &str = "Windows Bluetooth battery property";

#[derive(Debug)]
pub struct BluetoothError(String);

impl fmt::Display for BluetoothError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for BluetoothError {}

fn error(message: impl Into<String>) -> BluetoothError {
    BluetoothError(message.into())
}

#[cfg(all(windows, feature = "windows-backend"))]
fn normalize_device_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        FALLBACK_DEVICE_NAME.to_owned()
    } else {
        name.to_owned()
    }
}

/// Which Windows Bluetooth surface produced an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDeviceKind {
    /// A Windows Classic Bluetooth device or one of its profiles.
    Classic,
    /// A present Bluetooth PnP device node.
    Pnp,
    /// A paired Bluetooth Low Energy device.
    Ble,
}

impl BluetoothDeviceKind {
    /// The lowercase name used by captures and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Pnp => "pnp",
            Self::Ble => "ble",
        }
    }
}

/// Raw, privacy-safe evidence for one battery observation.
///
/// `device_id` is an opaque Windows identity and must not be included in logs
/// or diagnostic reports. Capture writers map it to a local identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothEvidence {
    pub device_id: String,
    pub name: String,
    pub kind: BluetoothDeviceKind,
    pub observation: BluetoothObservation,
}

/// The evidence behind one normalized Bluetooth reading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BluetoothObservation {
    /// A Windows property value, before range validation.
    Property {
        property: &'static str,
        property_type: &'static str,
        raw_value: u8,
    },
    /// A raw GATT Battery Level payload, before length or range validation.
    Gatt {
        service: &'static str,
        characteristic: &'static str,
        value: Vec<u8>,
    },
}

/// A Bluetooth battery reading. `device_id` is an opaque Windows identity and
/// must not be included in logs or diagnostic reports.
pub struct BluetoothBatteryReading {
    pub device_id: String,
    pub name: String,
    pub kind: BluetoothDeviceKind,
    pub battery: BatteryStatus,
}

#[derive(Default)]
pub struct BluetoothScan {
    pub readings: Vec<BluetoothBatteryReading>,
    pub evidence: Vec<BluetoothEvidence>,
    pub warnings: Vec<String>,
    pub discovery_events: Vec<String>,
    pub devices_examined: usize,
}

/// Decode the single-octet value defined by the BLE Battery Level characteristic.
///
/// # Errors
/// Returns an error when the payload is not exactly one byte or its value is outside the valid 0–100 range.
pub fn decode_battery_level(value: &[u8]) -> Result<u8, BluetoothError> {
    let [level] = value else {
        return Err(error(format!(
            "Battery Level must contain exactly one byte (received {})",
            value.len()
        )));
    };
    if !(BATTERY_LEVEL_MIN..=BATTERY_LEVEL_MAX).contains(level) {
        return Err(error(format!("Battery Level is outside 0..=100 ({level})")));
    }
    Ok(*level)
}

/// Builds the normalized reading for a Bluetooth battery percentage.
pub fn battery_status(level_percent: u8, source: &'static str) -> BatteryStatus {
    BatteryStatus {
        level_percent: Some(level_percent),
        level: BatteryLevel::Unknown,
        raw_level: level_percent,
        state: BatteryState::Unknown,
        raw_status: None,
        source,
    }
}

/// Decodes the single-byte value exposed through `System.Devices.BatteryLife`.
///
/// Values from 0 through 100 are percentages. The value 101 is Window's
/// unknown sentinel and returns `Ok(None)`.
///
/// # Errors
/// Returns an error for values outside 0 through 101.
pub fn decode_windows_battery_life(value: u8) -> Result<Option<u8>, BluetoothError> {
    match value {
        BATTERY_LEVEL_MIN..=BATTERY_LEVEL_MAX => Ok(Some(value)),
        101 => Ok(None),
        value => Err(error(format!(
            "Windows Bluetooth battery property is outside 0..=100 or the unknown sentinel ({value})"
        ))),
    }
}

#[cfg(all(windows, feature = "windows-backend"))]
mod windows_backend;

/// Reads battery levels from connected Bluetooth devices when Windows exposes
/// its battery property, including through present Bluetooth PnP nodes, and
/// from Windows-paired BLE devices. Paired BLE devices may be read even when
/// Windows does not report an active BLE connection.
///
/// # Errors
/// Returns an error if Windows Runtime initialization fails or the Windows
/// backend was not compiled in.
#[cfg(all(windows, feature = "windows-backend"))]
pub fn scan_battery_devices() -> Result<BluetoothScan, BluetoothError> {
    windows_backend::scan_battery_devices()
}

/// Bluetooth battery discovery requires Windows and the `windows-backend`
/// feature.
///
/// # Errors
/// Always returns an error when the backend is unavailable.
#[cfg(not(all(windows, feature = "windows-backend")))]
pub fn scan_battery_devices() -> Result<BluetoothScan, BluetoothError> {
    Err(error(if cfg!(windows) {
        "Bluetooth backend is not enabled in this build"
    } else {
        "Bluetooth battery reads are supported only on Windows"
    }))
}

/// Bluetooth battery source for the tray. It scans fully on every poll because
/// Windows offers no cheaper way to refresh a known Bluetooth device.
#[derive(Debug, Default)]
pub struct BluetoothProvider;

impl BatteryProvider for BluetoothProvider {
    fn poll(
        &mut self,
        _mode: ScanMode,
        sink: &mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>,
    ) -> Result<ProviderPoll, SinkClosed> {
        let mut poll = ProviderPoll::default();
        match scan_battery_devices() {
            Ok(scan) => {
                for event in scan_events(scan, &mut poll) {
                    sink(event)?;
                }
            }
            Err(error) => poll.errors.push(format!("Bluetooth discovery: {error}")),
        }
        Ok(poll)
    }
}

fn scan_events(scan: BluetoothScan, poll: &mut ProviderPoll) -> Vec<ProviderEvent> {
    let mut events = vec![ProviderEvent::Trace(format!(
        "Bluetooth discovery examined {} device entries and found {} readable battery value(s) with {} warning(s)",
        scan.devices_examined,
        scan.readings.len(),
        scan.warnings.len()
    ))];
    events.extend(
        scan.discovery_events
            .into_iter()
            .map(|event| ProviderEvent::Trace(format!("Bluetooth discovery: {event}"))),
    );
    poll.errors.extend(
        scan.warnings
            .into_iter()
            .map(|warning| format!("Bluetooth battery read: {warning}")),
    );
    for reading in scan.readings {
        events.push(ProviderEvent::Reading(SourceReading {
            id: DeviceId::new(format!("bluetooth:{}", reading.device_id)),
            label: DeviceLabel::new(reading.name),
            battery: reading.battery,
        }));
        poll.readings += 1;
    }
    events
}

#[cfg(test)]
mod tests {
    use super::{
        battery_status, decode_battery_level, decode_windows_battery_life, scan_events,
        BluetoothBatteryReading, BluetoothDeviceKind, BluetoothScan, SOURCE_GATT,
    };
    use volty_battery::{BatteryState, ProviderEvent, ProviderPoll};

    #[test]
    fn scan_events_namespace_ids_and_surface_warnings_as_errors() {
        let scan = BluetoothScan {
            readings: vec![BluetoothBatteryReading {
                device_id: "opaque".to_owned(),
                name: "Headset".to_owned(),
                kind: BluetoothDeviceKind::Ble,
                battery: battery_status(55, SOURCE_GATT),
            }],
            evidence: Vec::new(),
            warnings: vec!["probe failed".to_owned()],
            discovery_events: vec!["found one".to_owned()],
            devices_examined: 3,
        };
        let mut poll = ProviderPoll::default();

        let events = scan_events(scan, &mut poll);

        assert_eq!(poll.readings, 1);
        assert_eq!(poll.errors, ["Bluetooth battery read: probe failed"]);
        assert!(matches!(&events[1], ProviderEvent::Trace(text) if text.contains("found one")));
        match &events[2] {
            ProviderEvent::Reading(reading) => {
                assert_eq!(reading.id.as_str(), "bluetooth:opaque");
                assert_eq!(reading.label.display(), "Headset");
            }
            other => panic!("expected a reading, got {other:?}"),
        }
    }
    #[test]
    fn battery_level_accepts_the_full_specified_range() {
        assert_eq!(decode_battery_level(&[0]).unwrap(), 0);
        assert_eq!(decode_battery_level(&[100]).unwrap(), 100);
        assert_eq!(decode_battery_level(&[55]).unwrap(), 55);
    }

    #[test]
    fn battery_level_rejects_invalid_payloads() {
        assert!(decode_battery_level(&[]).is_err());
        assert!(decode_battery_level(&[50, 51]).is_err());
        assert!(decode_battery_level(&[101]).is_err());
    }

    #[test]
    #[cfg(all(windows, feature = "windows-backend"))]
    fn device_name_normalization_trims_useful_names() {
        assert_eq!(super::normalize_device_name("  Headset \t"), "Headset");
    }

    #[test]
    #[cfg(all(windows, feature = "windows-backend"))]
    fn device_name_normalization_uses_the_fallback_for_blank_names() {
        assert_eq!(
            super::normalize_device_name(" \t\r\n"),
            super::FALLBACK_DEVICE_NAME
        );
    }

    #[test]
    fn bluetooth_reading_does_not_infer_charge_state() {
        let reading = battery_status(42, SOURCE_GATT);
        assert_eq!(reading.level_percent, Some(42));
        assert_eq!(reading.state, BatteryState::Unknown);
        assert_eq!(reading.raw_status, None);
        assert_eq!(reading.source, SOURCE_GATT);
    }

    #[test]
    fn windows_battery_property_uses_its_unknown_sentinel() {
        assert_eq!(decode_windows_battery_life(0).unwrap(), Some(0));
        assert_eq!(decode_windows_battery_life(100).unwrap(), Some(100));
        assert_eq!(decode_windows_battery_life(101).unwrap(), None);
        assert!(decode_windows_battery_life(102).is_err());
    }
}
