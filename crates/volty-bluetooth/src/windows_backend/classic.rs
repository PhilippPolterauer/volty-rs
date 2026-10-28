use windows::{
    core::{Interface, HSTRING},
    Devices::{
        Bluetooth::{
            BluetoothConnectionStatus, BluetoothDevice,
            Rfcomm::{RfcommDeviceService, RfcommServiceId},
        },
        Enumeration::DeviceInformation,
    },
    Foundation::IPropertyValue,
};

use super::{
    budget::{push_scan_budget_warning, ScanBudget},
    merge_scan,
    winrt::{await_operation, windows_error},
    WINDOWS_BATTERY_LIFE_PROPERTY,
};
use crate::{
    battery_status, decode_windows_battery_life, normalize_device_name, BluetoothBatteryReading,
    BluetoothError, BluetoothScan, FALLBACK_DEVICE_NAME,
};

const HFP_HANDSFREE_SERVICE_ID: u32 = 0x111e;
pub(super) fn scan_windows_battery_property(
    budget: &ScanBudget,
) -> Result<BluetoothScan, BluetoothError> {
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
        budget,
    )?;
    let mut scan = BluetoothScan::default();
    let device_count = devices
        .Size()
        .map_err(|error| windows_error("could not count classic Bluetooth devices", error))?;
    for index in 0..device_count {
        if budget.is_exhausted() {
            push_scan_budget_warning(&mut scan, "Windows Bluetooth battery property");
            break;
        }
        scan.devices_examined += 1;
        let info = match devices.GetAt(index) {
            Ok(info) => info,
            Err(_) if budget.is_exhausted() => {
                push_scan_budget_warning(&mut scan, "Windows Bluetooth battery property");
                break;
            }
            Err(error) => {
                return Err(windows_error(
                    "could not read classic Bluetooth device information",
                    error,
                ));
            }
        };
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
                if budget.is_exhausted() {
                    push_scan_budget_warning(&mut scan, "Bluetooth device identity lookup");
                    break;
                }
                continue;
            }
        };
        let battery_level = match read_windows_battery_life(&info) {
            Ok(Some(level)) => {
                scan.discovery_events.push(format!(
                    "{name:?}: matched battery support because Windows supplied a valid {WINDOWS_BATTERY_LIFE_PROPERTY} value"
                ));
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_id.to_string_lossy(),
                    name: name.clone(),
                    battery: battery_status(level, crate::SOURCE_DEVICE_PROPERTY),
                });
                Some(level)
            }
            Ok(None) => {
                scan.discovery_events.push(format!(
                    "{name:?}: no readable Windows Bluetooth battery property"
                ));
                None
            }
            Err(error) => {
                scan.warnings.push(format!("{name}: {error}"));
                None
            }
        };
        if battery_level.is_none() {
            match scan_hfp_service_battery_property(&device_id, &name, budget) {
                Ok(hfp_scan) => merge_scan(&mut scan, hfp_scan),
                Err(error) => scan.warnings.push(format!(
                    "{name}: Windows HFP service-property scan: {error}"
                )),
            }
        }
        if budget.is_exhausted() {
            push_scan_budget_warning(&mut scan, "Windows Bluetooth battery property");
            break;
        }
    }
    Ok(scan)
}

fn scan_hfp_service_battery_property(
    device_id: &HSTRING,
    name: &str,
    budget: &ScanBudget,
) -> Result<BluetoothScan, BluetoothError> {
    let device = await_operation(
        BluetoothDevice::FromIdAsync(device_id)
            .map_err(|error| windows_error("could not open the Bluetooth device", error))?,
        "Bluetooth device lookup for HFP battery property",
        budget,
    )?;
    let service_id = RfcommServiceId::FromShortId(HFP_HANDSFREE_SERVICE_ID)
        .map_err(|error| windows_error("could not resolve the HFP service ID", error))?;
    let selector =
        RfcommDeviceService::GetDeviceSelectorForBluetoothDeviceAndServiceId(&device, &service_id)
            .map_err(|error| windows_error("could not create an HFP service selector", error))?;
    let properties =
        windows_collections::IVectorView::from(vec![HSTRING::from(WINDOWS_BATTERY_LIFE_PROPERTY)]);
    let services = await_operation(
        DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&selector, &properties)
            .map_err(|error| windows_error("could not enumerate HFP service properties", error))?,
        "HFP service-property enumeration",
        budget,
    )?;
    let service_count = services
        .Size()
        .map_err(|error| windows_error("could not count HFP services", error))?;
    let mut scan = BluetoothScan::default();
    let mut services_examined = 0;
    let mut battery_properties_found = 0;
    for index in 0..service_count {
        if budget.is_exhausted() {
            break;
        }
        let service = match services.GetAt(index) {
            Ok(service) => service,
            Err(_) if budget.is_exhausted() => break,
            Err(error) => {
                return Err(windows_error(
                    "could not read HFP service information",
                    error,
                ));
            }
        };
        services_examined += 1;
        scan.devices_examined += 1;
        match read_windows_battery_life(&service) {
            Ok(Some(level)) => {
                battery_properties_found += 1;
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_id.to_string_lossy(),
                    name: name.to_owned(),
                    battery: battery_status(level, crate::SOURCE_DEVICE_PROPERTY),
                });
            }
            Ok(None) => {}
            Err(error) => scan.warnings.push(format!("{name}: {error}")),
        }
    }
    scan.discovery_events.push(format!(
        "{name:?}: HFP service-property lookup examined {services_examined} RFCOMM service(s) and found {battery_properties_found} {WINDOWS_BATTERY_LIFE_PROPERTY} value(s); no RFCOMM connection was opened"
    ));
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
pub(super) fn device_name(info: &DeviceInformation) -> (String, Option<BluetoothError>) {
    match info.Name() {
        Ok(name) => (normalize_device_name(&name.to_string_lossy()), None),
        Err(error) => (
            FALLBACK_DEVICE_NAME.to_owned(),
            Some(windows_error("could not read Bluetooth device name", error)),
        ),
    }
}
