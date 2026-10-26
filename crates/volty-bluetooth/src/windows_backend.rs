use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use windows::{
    core::{Interface, RuntimeType, HSTRING},
    Devices::{
        Bluetooth::{
            BluetoothCacheMode, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice,
            GenericAttributeProfile::{
                GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids,
            },
        },
        Enumeration::DeviceInformation,
    },
    Foundation::IPropertyValue,
    Storage::Streams::DataReader,
    Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

use crate::{
    battery_status, decode_battery_level, decode_windows_battery_life, error,
    normalize_device_name, BluetoothBatteryReading, BluetoothError, BluetoothScan,
    FALLBACK_DEVICE_NAME,
};

const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const PAIRED_BLE_SCAN_BUDGET: Duration = Duration::from_secs(30);
const WINDOWS_BATTERY_LIFE_PROPERTY: &str = "System.Devices.BatteryLife";

struct WinRtApartment;

impl WinRtApartment {
    fn initialize() -> Result<Self, BluetoothError> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| windows_error("could not initialize Windows Runtime", error))?;
        Ok(Self)
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

pub(super) fn scan_battery_devices() -> Result<BluetoothScan, BluetoothError> {
    let _apartment = WinRtApartment::initialize()?;
    let mut scan = BluetoothScan::default();
    match scan_windows_battery_property() {
        Ok(property_scan) => merge_scan(&mut scan, property_scan),
        Err(error) => scan
            .warnings
            .push(format!("Windows Bluetooth battery property: {error}")),
    }
    match scan_paired_ble_devices() {
        Ok(ble_scan) => merge_scan(&mut scan, ble_scan),
        Err(error) => scan
            .warnings
            .push(format!("Bluetooth GATT discovery: {error}")),
    }
    Ok(scan)
}

fn merge_scan(target: &mut BluetoothScan, mut source: BluetoothScan) {
    target.devices_examined += source.devices_examined;
    target.readings.append(&mut source.readings);
    target.discovery_events.append(&mut source.discovery_events);
    target.warnings.append(&mut source.warnings);
}

fn scan_windows_battery_property() -> Result<BluetoothScan, BluetoothError> {
    let selector = BluetoothDevice::GetDeviceSelectorFromConnectionStatus(
        BluetoothConnectionStatus::Connected,
    )
    .map_err(|error| windows_error("could not create a classic Bluetooth selector", error))?;
    let properties =
        windows_collections::IVectorView::from(vec![HSTRING::from(WINDOWS_BATTERY_LIFE_PROPERTY)]);
    let devices = await_operation(
        DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&selector, &properties)
            .map_err(|error| {
                windows_error("could not enumerate classic Bluetooth devices", error)
            })?,
        "classic Bluetooth device enumeration",
    )?;
    let mut scan = BluetoothScan::default();
    for index in 0..devices
        .Size()
        .map_err(|error| windows_error("could not count classic Bluetooth devices", error))?
    {
        scan.devices_examined += 1;
        let info = devices.GetAt(index).map_err(|error| {
            windows_error("could not read classic Bluetooth device information", error)
        })?;
        let (name, name_error) = device_name(&info);
        scan.discovery_events.push(format!(
            "{name:?}: matched the connected Bluetooth selector; checking {WINDOWS_BATTERY_LIFE_PROPERTY}"
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
                continue;
            }
        };
        match read_windows_battery_life(&info) {
            Ok(Some(level)) => {
                scan.discovery_events.push(format!(
                    "{name:?}: matched battery support because Windows supplied a valid {WINDOWS_BATTERY_LIFE_PROPERTY} value"
                ));
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_id.to_string_lossy(),
                    name,
                    battery: battery_status(
                        level,
                        volty_battery::BatterySource::BluetoothDeviceProperty,
                    ),
                });
            }
            Ok(None) => scan.discovery_events.push(format!(
                "{name:?}: no readable {WINDOWS_BATTERY_LIFE_PROPERTY} value"
            )),
            Err(error) => scan.warnings.push(format!("{name}: {error}")),
        }
    }
    Ok(scan)
}

fn read_windows_battery_life(info: &DeviceInformation) -> Result<Option<u8>, BluetoothError> {
    let properties = info
        .Properties()
        .map_err(|error| windows_error("could not read Bluetooth device properties", error))?;
    let property_name = HSTRING::from(WINDOWS_BATTERY_LIFE_PROPERTY);
    if !properties
        .HasKey(&property_name)
        .map_err(|error| windows_error("could not check Bluetooth battery property", error))?
    {
        return Ok(None);
    }
    let property = match properties.Lookup(&property_name) {
        Ok(property) => property,
        Err(error) if error.code().is_ok() => return Ok(None),
        Err(error) => {
            return Err(windows_error(
                "could not retrieve Bluetooth battery property",
                error,
            ))
        }
    };
    if property.as_raw().is_null() {
        return Ok(None);
    }
    let property_value = property.cast::<IPropertyValue>().map_err(|error| {
        windows_error("Bluetooth battery property has an unsupported type", error)
    })?;
    let level = property_value
        .GetUInt8()
        .map_err(|error| windows_error("could not decode Bluetooth battery property", error))?;
    decode_windows_battery_life(level)
}

fn scan_paired_ble_devices() -> Result<BluetoothScan, BluetoothError> {
    let deadline = Instant::now() + PAIRED_BLE_SCAN_BUDGET;
    let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)
        .map_err(|error| windows_error("could not create a paired Bluetooth LE selector", error))?;
    let devices = await_operation_until(
        DeviceInformation::FindAllAsyncAqsFilter(&selector)
            .map_err(|error| windows_error("could not enumerate Bluetooth devices", error))?,
        "Bluetooth device enumeration",
        deadline,
    )?;

    let mut scan = BluetoothScan::default();
    let device_count = devices
        .Size()
        .map_err(|error| windows_error("could not count Bluetooth devices", error))?;
    for index in 0..device_count {
        if Instant::now() >= deadline {
            push_scan_budget_warning(&mut scan, device_count - index);
            break;
        }
        scan.devices_examined += 1;
        let info = devices
            .GetAt(index)
            .map_err(|error| windows_error("could not read Bluetooth device information", error))?;
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
                continue;
            }
        };
        if Instant::now() >= deadline {
            push_scan_budget_warning(&mut scan, device_count - index - 1);
            break;
        }
        match read_battery(&device_id, deadline) {
            Ok(Some(battery)) => {
                scan.discovery_events.push(format!(
                    "{name:?}: matched the standard GATT Battery Service (0x180F) and Battery Level characteristic (0x2A19)"
                ));
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_id.to_string_lossy(),
                    name,
                    battery,
                });
            }
            Ok(None) => scan.discovery_events.push(format!(
                "{name:?}: no standard GATT Battery Service (0x180F) was found"
            )),
            Err(error) => scan.warnings.push(format!("{name}: {error}")),
        }
        if Instant::now() >= deadline {
            push_scan_budget_warning(&mut scan, device_count - index - 1);
            break;
        }
    }
    Ok(scan)
}

fn device_name(info: &DeviceInformation) -> (String, Option<BluetoothError>) {
    match info.Name() {
        Ok(name) => (normalize_device_name(&name.to_string_lossy()), None),
        Err(error) => (
            FALLBACK_DEVICE_NAME.to_owned(),
            Some(windows_error("could not read Bluetooth device name", error)),
        ),
    }
}

fn push_scan_budget_warning(scan: &mut BluetoothScan, remaining_devices: u32) {
    let remaining = if remaining_devices == 0 {
        " after the final paired device"
    } else {
        "; remaining paired devices were skipped"
    };
    scan.warnings.push(format!(
        "Bluetooth LE scan budget of {} seconds exhausted{remaining}",
        PAIRED_BLE_SCAN_BUDGET.as_secs()
    ));
}

fn read_battery(
    device_id: &HSTRING,
    deadline: Instant,
) -> Result<Option<volty_battery::BatteryStatus>, BluetoothError> {
    let device = await_operation_until(
        BluetoothLEDevice::FromIdAsync(device_id)
            .map_err(|error| windows_error("could not open Bluetooth LE device", error))?,
        "Bluetooth LE connection",
        deadline,
    )?;

    let service_uuid = GattServiceUuids::Battery()
        .map_err(|error| windows_error("could not resolve the Battery Service UUID", error))?;
    let services = await_operation_until(
        device
            .GetGattServicesForUuidWithCacheModeAsync(service_uuid, BluetoothCacheMode::Uncached)
            .map_err(|error| windows_error("could not start GATT service discovery", error))?,
        "GATT service discovery",
        deadline,
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
        0 => return Ok(None),
        1 => {}
        _ => {
            return Err(error(
                "multiple Battery Services found; battery component is ambiguous",
            ))
        }
    }
    let service = services
        .GetAt(0)
        .map_err(|error| windows_error("could not open the Battery Service", error))?;
    let characteristic_uuid = GattCharacteristicUuids::BatteryLevel()
        .map_err(|error| windows_error("could not resolve the Battery Level UUID", error))?;
    let characteristics = await_operation_until(
        service
            .GetCharacteristicsForUuidWithCacheModeAsync(
                characteristic_uuid,
                BluetoothCacheMode::Uncached,
            )
            .map_err(|error| {
                windows_error("could not start GATT characteristic discovery", error)
            })?,
        "GATT characteristic discovery",
        deadline,
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
        0 => return Err(error("Battery Level characteristic was not found")),
        1 => {}
        _ => {
            return Err(error(
                "multiple Battery Level characteristics found; battery component is ambiguous",
            ))
        }
    }
    let characteristic = characteristics
        .GetAt(0)
        .map_err(|error| windows_error("could not open the Battery Level characteristic", error))?;
    let value = await_operation_until(
        characteristic
            .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)
            .map_err(|error| windows_error("could not start Battery Level read", error))?,
        "Battery Level read",
        deadline,
    )?;
    check_gatt_status(
        value
            .Status()
            .map_err(|error| windows_error("could not read Battery Level status", error))?,
    )?;
    let reader = DataReader::FromBuffer(
        &value
            .Value()
            .map_err(|error| windows_error("could not access Battery Level data", error))?,
    )
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
    Ok(Some(battery_status(
        level,
        volty_battery::BatterySource::BluetoothGatt,
    )))
}

fn await_operation<T>(
    operation: IAsyncOperation<T>,
    operation_name: &str,
) -> Result<T, BluetoothError>
where
    T: RuntimeType + Clone + Send + 'static,
{
    await_operation_with_timeout(operation, operation_name, OPERATION_TIMEOUT)
}

fn await_operation_until<T>(
    operation: IAsyncOperation<T>,
    operation_name: &str,
    deadline: Instant,
) -> Result<T, BluetoothError>
where
    T: RuntimeType + Clone + Send + 'static,
{
    let timeout = deadline.saturating_duration_since(Instant::now());
    if timeout.is_zero() {
        let _ = operation.Cancel();
        return Err(error(format!(
            "{operation_name} skipped because the Bluetooth LE scan budget was exhausted"
        )));
    }
    let result =
        await_operation_with_timeout(operation, operation_name, OPERATION_TIMEOUT.min(timeout));
    if result.is_err() && Instant::now() >= deadline {
        return Err(error(format!(
            "{operation_name} timed out because the Bluetooth LE scan budget was exhausted"
        )));
    }
    result
}

fn await_operation_with_timeout<T>(
    operation: IAsyncOperation<T>,
    operation_name: &str,
    timeout: Duration,
) -> Result<T, BluetoothError>
where
    T: RuntimeType + Clone + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    operation
        .SetCompleted(&AsyncOperationCompletedHandler::new(move |completed, _| {
            let result = completed.ok()?.GetResults();
            let _ = sender.send(result);
            Ok(())
        }))
        .map_err(|error| windows_error("could not observe Windows Runtime operation", error))?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => {
            result.map_err(|error| windows_error(&format!("{operation_name} failed"), error))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = operation.Cancel();
            Err(error(format!("{operation_name} timed out")))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(error(format!(
            "{operation_name} completed without returning a result"
        ))),
    }
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

fn windows_error(context: &str, windows_error: windows::core::Error) -> BluetoothError {
    error(format!(
        "{context} (HRESULT 0x{:08x})",
        windows_error.code().0 as u32
    ))
}
