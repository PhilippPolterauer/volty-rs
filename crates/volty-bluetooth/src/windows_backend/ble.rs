use windows::{
    core::HSTRING,
    Devices::{
        Bluetooth::{
            BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
            GenericAttributeProfile::{
                GattCharacteristic, GattCharacteristicUuids, GattCommunicationStatus,
                GattDeviceService, GattServiceUuids,
            },
        },
        Enumeration::DeviceInformation,
    },
    Storage::Streams::DataReader,
};

use super::{
    budget::{push_scan_budget_warning, ScanBudget},
    classic::device_name,
    winrt::{await_operation, windows_error},
};
use crate::{
    battery_status, decode_battery_level, error, BluetoothBatteryReading, BluetoothDeviceKind,
    BluetoothError, BluetoothEvidence, BluetoothObservation, BluetoothScan, SOURCE_GATT,
};
pub(super) fn scan_paired_ble_devices(
    budget: &ScanBudget,
) -> Result<BluetoothScan, BluetoothError> {
    let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)
        .map_err(|error| windows_error("could not create a paired Bluetooth LE selector", error))?;
    let devices = await_operation(
        DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|error| windows_error("could not enumerate Bluetooth devices", error))?,
        "Bluetooth device enumeration",
        budget,
    )?;

    let mut scan = BluetoothScan::default();
    let device_count = devices
        .Size()
        .map_err(|error| windows_error("could not count Bluetooth devices", error))?;
    for index in 0..device_count {
        if budget.is_exhausted() {
            push_scan_budget_warning(&mut scan, "Bluetooth GATT discovery");
            break;
        }
        scan.devices_examined += 1;
        let info = match devices.GetAt(index) {
            Ok(info) => info,
            Err(_) if budget.is_exhausted() => {
                push_scan_budget_warning(&mut scan, "Bluetooth GATT discovery");
                break;
            }
            Err(error) => {
                return Err(windows_error(
                    "could not read Bluetooth device information",
                    error,
                ));
            }
        };
        let (name, name_error) = device_name(&info);
        scan.discovery_events.push(format!(
            "{name:?}: matched the Windows-paired Bluetooth LE selector"
        ));
        if let Some(error) = name_error {
            scan.warnings.push(format!("{name}: {error}"));
        }
        let device_id = match info.Id() {
            Ok(device_id) => device_id,
            Err(error) => {
                scan.warnings.push(format!(
                    "{name}: {}",
                    windows_error("could not read Bluetooth device identity", error)
                ));
                if budget.is_exhausted() {
                    push_scan_budget_warning(&mut scan, "Bluetooth device identity lookup");
                    break;
                }
                continue;
            }
        };
        if budget.is_exhausted() {
            push_scan_budget_warning(&mut scan, "Bluetooth GATT discovery");
            break;
        }
        match read_battery(&device_id, budget) {
            Ok(BatteryProbe::Reading(gatt)) => {
                scan.discovery_events.push(format!(
                    "{name:?}: matched the standard GATT Battery Service (0x180F) and Battery Level characteristic (0x2A19)"
                ));
                let device_key = device_id.to_string_lossy();
                scan.evidence.push(BluetoothEvidence {
                    device_id: device_key.clone(),
                    name: name.clone(),
                    kind: BluetoothDeviceKind::Ble,
                    observation: BluetoothObservation::Gatt {
                        service: "180f",
                        characteristic: "2a19",
                        value: gatt.payload,
                    },
                });
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_key,
                    name,
                    kind: BluetoothDeviceKind::Ble,
                    battery: battery_status(gatt.level, SOURCE_GATT),
                });
            }
            Ok(BatteryProbe::NoBatteryService) => scan.discovery_events.push(format!(
                "{name:?}: no standard GATT Battery Service (0x180F) was found"
            )),
            Ok(BatteryProbe::Disconnected) => scan.discovery_events.push(format!(
                "{name:?}: skipped because Windows reports the Bluetooth LE device as not connected"
            )),
            Err(error) => scan.warnings.push(format!("{name}: {error}")),
        }
        if budget.is_exhausted() {
            push_scan_budget_warning(&mut scan, "Bluetooth GATT discovery");
            break;
        }
    }
    Ok(scan)
}

struct GattBatteryReading {
    payload: Vec<u8>,
    level: u8,
}

enum BatteryProbe {
    Reading(GattBatteryReading),
    NoBatteryService,
    Disconnected,
}

fn read_battery(device_id: &HSTRING, budget: &ScanBudget) -> Result<BatteryProbe, BluetoothError> {
    let device = await_operation(
        BluetoothLEDevice::FromIdAsync(device_id)
            .map_err(|error| windows_error("could not open Bluetooth LE device", error))?,
        "Bluetooth LE connection",
        budget,
    )?;
    if device
        .ConnectionStatus()
        .map_err(|error| windows_error("could not read Bluetooth LE connection status", error))?
        != BluetoothConnectionStatus::Connected
    {
        return Ok(BatteryProbe::Disconnected);
    }
    match read_battery_level(&device, budget)? {
        Some(reading) => Ok(BatteryProbe::Reading(reading)),
        None => Ok(BatteryProbe::NoBatteryService),
    }
}

fn read_battery_level(
    device: &BluetoothLEDevice,
    budget: &ScanBudget,
) -> Result<Option<GattBatteryReading>, BluetoothError> {
    let service_uuid = GattServiceUuids::Battery()
        .map_err(|error| windows_error("could not resolve the Battery Service UUID", error))?;
    let services = await_operation(
        device
            .GetGattServicesForUuidWithCacheModeAsync(service_uuid, BluetoothCacheMode::Uncached)
            .map_err(|error| windows_error("could not start GATT service discovery", error))?,
        "GATT service discovery",
        budget,
    )?;
    check_gatt_status(
        services.Status().map_err(|error| {
            windows_error("could not read GATT service discovery status", error)
        })?,
    )?;
    let services = services
        .Services()
        .map_err(|error| windows_error("could not read GATT services", error))?;
    let service_count = services
        .Size()
        .map_err(|error| windows_error("could not count GATT services", error))?;
    match service_count {
        0 => Ok(None),
        1 => {
            let service = services
                .GetAt(0)
                .map_err(|error| windows_error("could not open the Battery Service", error))?;
            read_battery_characteristic(&service, budget).map(Some)
        }
        _ => Err(error(
            "multiple Battery Services found; battery component is ambiguous",
        )),
    }
}

fn read_battery_characteristic(
    service: &GattDeviceService,
    budget: &ScanBudget,
) -> Result<GattBatteryReading, BluetoothError> {
    let characteristic_uuid = GattCharacteristicUuids::BatteryLevel()
        .map_err(|error| windows_error("could not resolve the Battery Level UUID", error))?;
    let characteristics = await_operation(
        service
            .GetCharacteristicsForUuidWithCacheModeAsync(
                characteristic_uuid,
                BluetoothCacheMode::Uncached,
            )
            .map_err(|error| {
                windows_error("could not start GATT characteristic discovery", error)
            })?,
        "GATT characteristic discovery",
        budget,
    )?;
    check_gatt_status(characteristics.Status().map_err(|error| {
        windows_error("could not read GATT characteristic discovery status", error)
    })?)?;
    let characteristics = characteristics
        .Characteristics()
        .map_err(|error| windows_error("could not read GATT characteristics", error))?;
    let characteristic_count = characteristics
        .Size()
        .map_err(|error| windows_error("could not count GATT characteristics", error))?;
    match characteristic_count {
        0 => Err(error("Battery Level characteristic was not found")),
        1 => {
            let characteristic: GattCharacteristic = characteristics.GetAt(0).map_err(|error| {
                windows_error("could not open the Battery Level characteristic", error)
            })?;
            read_battery_characteristic_value(&characteristic, budget)
        }
        _ => Err(error(
            "multiple Battery Level characteristics found; battery component is ambiguous",
        )),
    }
}

fn read_battery_characteristic_value(
    characteristic: &GattCharacteristic,
    budget: &ScanBudget,
) -> Result<GattBatteryReading, BluetoothError> {
    let value = await_operation(
        characteristic
            .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)
            .map_err(|error| windows_error("could not start Battery Level read", error))?,
        "Battery Level read",
        budget,
    )?;
    check_gatt_status(
        value
            .Status()
            .map_err(|error| windows_error("could not read Battery Level status", error))?,
    )?;
    let buffer = value
        .Value()
        .map_err(|error| windows_error("could not access Battery Level data", error))?;
    let reader = DataReader::FromBuffer(&buffer)
        .map_err(|error| windows_error("could not decode Battery Level data", error))?;
    let mut payload = vec![
        0;
        reader.UnconsumedBufferLength().map_err(|error| {
            windows_error("could not read Battery Level payload length", error)
        })? as usize
    ];
    reader
        .ReadBytes(&mut payload)
        .map_err(|error| windows_error("could not read Battery Level payload", error))?;
    let level = decode_battery_level(&payload)?;
    Ok(GattBatteryReading { payload, level })
}

fn check_gatt_status(status: GattCommunicationStatus) -> Result<(), BluetoothError> {
    match status {
        GattCommunicationStatus::Success => Ok(()),
        GattCommunicationStatus::Unreachable => Err(error("Bluetooth device is unreachable")),
        GattCommunicationStatus::AccessDenied => Err(error("Bluetooth access was denied")),
        GattCommunicationStatus::ProtocolError => Err(error("Bluetooth GATT protocol error")),
        _ => Err(error("Bluetooth GATT returned an unknown status")),
    }
}
