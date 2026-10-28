use std::{error::Error, fmt};

use volty_battery::{
    BatteryLevel, BatteryProvider, BatteryState, BatteryStatus, DeviceId, DeviceLabel,
    ProviderEvent, ProviderPoll, ScanMode, SinkClosed, SourceReading,
};

const BATTERY_LEVEL_MIN: u8 = 0;
const BATTERY_LEVEL_MAX: u8 = 100;
const FALLBACK_DEVICE_NAME: &str = "Bluetooth device";
pub(crate) const SOURCE_GATT: &str = "Bluetooth GATT Battery Service";
pub(crate) const SOURCE_DEVICE_PROPERTY: &str = "Windows Bluetooth battery property";

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

fn normalize_device_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        FALLBACK_DEVICE_NAME.to_owned()
    } else {
        name.to_owned()
    }
}

/// A Bluetooth battery reading. `device_id` is an opaque Windows identity and
/// must not be included in logs or diagnostic reports.
pub struct BluetoothBatteryReading {
    pub device_id: String,
    pub name: String,
    pub battery: BatteryStatus,
}

#[derive(Default)]
pub struct BluetoothScan {
    pub readings: Vec<BluetoothBatteryReading>,
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

fn battery_status(level_percent: u8, source: &'static str) -> BatteryStatus {
    BatteryStatus {
        level_percent: Some(level_percent),
        level: BatteryLevel::Unknown,
        raw_level: level_percent,
        state: BatteryState::Unknown,
        raw_status: None,
        source,
    }
}

fn decode_windows_battery_life(value: u8) -> Result<Option<u8>, BluetoothError> {
    match value {
        BATTERY_LEVEL_MIN..=BATTERY_LEVEL_MAX => Ok(Some(value)),
        101 => Ok(None),
        value => Err(error(format!(
            "Windows Bluetooth battery property is outside 0..=100 or the unknown sentinel ({value})"
        ))),
    }
}

#[cfg(windows)]
mod windows_backend;

/// Reads battery levels from connected Bluetooth devices when Windows exposes
/// its battery property, including through present Bluetooth PnP nodes, and
/// from Windows-paired BLE devices. Paired BLE devices may be read even when
/// Windows does not report an active BLE connection.
///
/// # Errors
/// Returns an error if Windows Runtime initialization fails.
#[cfg(windows)]
pub fn scan_battery_devices() -> Result<BluetoothScan, BluetoothError> {
    windows_backend::scan_battery_devices()
}

/// Bluetooth battery discovery is available only on Windows.
///
/// # Errors
/// Always returns an error because Bluetooth battery reads require Windows.
#[cfg(not(windows))]
pub fn scan_battery_devices() -> Result<BluetoothScan, BluetoothError> {
    Err(error(
        "Bluetooth battery reads are supported only on Windows",
    ))
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
        battery_status, decode_battery_level, decode_windows_battery_life, normalize_device_name,
        scan_events, BluetoothBatteryReading, BluetoothScan, FALLBACK_DEVICE_NAME, SOURCE_GATT,
    };
    use volty_battery::{BatteryState, ProviderEvent, ProviderPoll};

    #[test]
    fn scan_events_namespace_ids_and_surface_warnings_as_errors() {
        let scan = BluetoothScan {
            readings: vec![BluetoothBatteryReading {
                device_id: "opaque".to_owned(),
                name: "Headset".to_owned(),
                battery: battery_status(55, SOURCE_GATT),
            }],
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
    fn device_name_normalization_trims_useful_names() {
        assert_eq!(normalize_device_name("  Headset \t"), "Headset");
    }

    #[test]
    fn device_name_normalization_uses_the_fallback_for_blank_names() {
        assert_eq!(normalize_device_name(" \t\r\n"), FALLBACK_DEVICE_NAME);
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
