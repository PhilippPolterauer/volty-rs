use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use windows::{
    core::{Interface, RuntimeType, HSTRING, PCWSTR},
    Devices::{
        Bluetooth::{
            BluetoothCacheMode, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice,
            GenericAttributeProfile::{
                GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids,
            },
            Rfcomm::{RfcommDeviceService, RfcommServiceId},
        },
        Enumeration::DeviceInformation,
    },
    Foundation::IPropertyValue,
    Storage::Streams::DataReader,
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
                SetupDiGetDeviceInstanceIdW, SetupDiGetDevicePropertyW, DIGCF_ALLCLASSES,
                DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA,
            },
            Properties::{
                DEVPKEY_Device_FriendlyName, DEVPKEY_Device_Parent, DEVPROPTYPE, DEVPROP_TYPE_BYTE,
                DEVPROP_TYPE_STRING,
            },
        },
        Foundation::{DEVPROPKEY, ERROR_INSUFFICIENT_BUFFER, ERROR_NOT_FOUND, ERROR_NO_MORE_ITEMS},
        System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    },
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
const HFP_HANDSFREE_SERVICE_ID: u32 = 0x111e;
const BATTERY_LIFE_PROPERTY_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: windows::core::GUID::from_u128(0x49cd1f76_5626_4b17_a4e8_18b4aa1a2213),
    pid: 10,
};

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
    match scan_setupapi_battery_property() {
        Ok(property_scan) => merge_scan(&mut scan, property_scan),
        Err(error) => scan
            .warnings
            .push(format!("Windows Bluetooth device-property scan: {error}")),
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
    for reading in source.readings.drain(..) {
        if !target
            .readings
            .iter()
            .any(|existing| existing.device_id == reading.device_id)
        {
            target.readings.push(reading);
        }
    }
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
        let battery_level = match read_windows_battery_life(&info) {
            Ok(Some(level)) => {
                scan.discovery_events.push(format!(
                    "{name:?}: matched battery support because Windows supplied a valid {WINDOWS_BATTERY_LIFE_PROPERTY} value"
                ));
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_id.to_string_lossy(),
                    name: name.clone(),
                    battery: battery_status(
                        level,
                        volty_battery::BatterySource::BluetoothDeviceProperty,
                    ),
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
            match scan_hfp_service_battery_property(&device_id, &name) {
                Ok(hfp_scan) => merge_scan(&mut scan, hfp_scan),
                Err(error) => scan.warnings.push(format!(
                    "{name}: Windows HFP service-property scan: {error}"
                )),
            }
        }
    }
    Ok(scan)
}

fn scan_hfp_service_battery_property(
    device_id: &HSTRING,
    name: &str,
) -> Result<BluetoothScan, BluetoothError> {
    let device = await_operation(
        BluetoothDevice::FromIdAsync(device_id)
            .map_err(|error| windows_error("could not open the Bluetooth device", error))?,
        "Bluetooth device lookup for HFP battery property",
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
    )?;
    let service_count = services
        .Size()
        .map_err(|error| windows_error("could not count HFP services", error))?;
    let mut scan = BluetoothScan {
        devices_examined: service_count as usize,
        ..BluetoothScan::default()
    };
    let mut battery_properties_found = 0;
    for index in 0..service_count {
        let service = services
            .GetAt(index)
            .map_err(|error| windows_error("could not read HFP service information", error))?;
        match read_windows_battery_life(&service) {
            Ok(Some(level)) => {
                battery_properties_found += 1;
                scan.readings.push(BluetoothBatteryReading {
                    device_id: device_id.to_string_lossy(),
                    name: name.to_owned(),
                    battery: battery_status(
                        level,
                        volty_battery::BatterySource::BluetoothDeviceProperty,
                    ),
                });
            }
            Ok(None) => {}
            Err(error) => scan.warnings.push(format!("{name}: {error}")),
        }
    }
    scan.discovery_events.push(format!(
        "{name:?}: HFP service-property lookup examined {service_count} RFCOMM service(s) and found {battery_properties_found} {WINDOWS_BATTERY_LIFE_PROPERTY} value(s); no RFCOMM connection was opened"
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

fn scan_setupapi_battery_property() -> Result<BluetoothScan, BluetoothError> {
    let device_info_set = unsafe {
        SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
    }
    .map_err(|error| windows_error("could not enumerate present PnP devices", error))?;
    let device_info_set = DeviceInfoSet(device_info_set);
    let mut scan = BluetoothScan::default();
    let mut bluetooth_nodes_examined = 0;
    let mut battery_properties_found = 0;
    let mut index = 0;
    loop {
        let mut info = SP_DEVINFO_DATA::default();
        info.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        match unsafe { SetupDiEnumDeviceInfo(device_info_set.0, index, &mut info) } {
            Ok(()) => {}
            Err(error) if windows_error_code(&error) == ERROR_NO_MORE_ITEMS.0 => break,
            Err(error) => {
                return Err(windows_error(
                    "could not enumerate present PnP devices",
                    error,
                ))
            }
        }
        index += 1;

        let instance_id = match read_device_instance_id(device_info_set.0, &info) {
            Ok(instance_id) => instance_id,
            Err(error) => {
                scan.warnings.push(error.to_string());
                continue;
            }
        };
        match is_bluetooth_device_node(device_info_set.0, &info, &instance_id) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                scan.warnings.push(error.to_string());
                continue;
            }
        }
        bluetooth_nodes_examined += 1;
        scan.devices_examined += 1;
        let battery_property = match read_device_property(
            device_info_set.0,
            &info,
            &BATTERY_LIFE_PROPERTY_KEY,
            "could not read Windows Bluetooth battery property",
        ) {
            Ok(property) => property,
            Err(error) => {
                scan.warnings.push(error.to_string());
                continue;
            }
        };
        let Some((property_type, bytes)) = battery_property else {
            continue;
        };
        battery_properties_found += 1;
        if property_type != DEVPROP_TYPE_BYTE || bytes.len() != 1 {
            scan.warnings.push(format!(
                "Windows Bluetooth battery property has an unexpected type or size (type={}, bytes={})",
                property_type.0,
                bytes.len()
            ));
            continue;
        }
        let name = match read_device_property(
            device_info_set.0,
            &info,
            &DEVPKEY_Device_FriendlyName,
            "could not read Bluetooth device name",
        ) {
            Ok(Some((property_type, bytes))) if property_type == DEVPROP_TYPE_STRING => {
                decode_device_property_string(&bytes)
            }
            Ok(_) => FALLBACK_DEVICE_NAME.to_owned(),
            Err(error) => {
                scan.warnings.push(error.to_string());
                FALLBACK_DEVICE_NAME.to_owned()
            }
        };
        match decode_windows_battery_life(bytes[0]) {
            Ok(Some(level)) => {
                scan.discovery_events.push(format!(
                    "{name:?}: read battery level from the Windows PnP {WINDOWS_BATTERY_LIFE_PROPERTY} property"
                ));
                scan.readings.push(BluetoothBatteryReading {
                    device_id: instance_id,
                    name,
                    battery: battery_status(
                        level,
                        volty_battery::BatterySource::BluetoothDeviceProperty,
                    ),
                });
            }
            Ok(None) => scan.discovery_events.push(format!(
                "{name:?}: Windows PnP {WINDOWS_BATTERY_LIFE_PROPERTY} property reports an unknown battery level"
            )),
            Err(error) => scan.warnings.push(format!("{name}: {error}")),
        }
    }
    if bluetooth_nodes_examined > 0 {
        scan.discovery_events.push(format!(
            "SetupAPI examined {bluetooth_nodes_examined} Bluetooth device node(s) and found {battery_properties_found} {WINDOWS_BATTERY_LIFE_PROPERTY} property value(s)"
        ));
    }
    Ok(scan)
}

struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn read_device_instance_id(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
) -> Result<String, BluetoothError> {
    let mut required_size = 0;
    match unsafe {
        SetupDiGetDeviceInstanceIdW(device_info_set, info, None, Some(&mut required_size))
    } {
        Ok(()) => {}
        Err(error) if windows_error_code(&error) == ERROR_INSUFFICIENT_BUFFER.0 => {}
        Err(error) => {
            return Err(windows_error("could not read PnP device identity", error));
        }
    }
    if required_size == 0 {
        return Err(error("PnP device identity had an empty size"));
    }
    let mut buffer = vec![0u16; required_size as usize];
    unsafe {
        SetupDiGetDeviceInstanceIdW(
            device_info_set,
            info,
            Some(&mut buffer),
            Some(&mut required_size),
        )
    }
    .map_err(|error| windows_error("could not read PnP device identity", error))?;
    Ok(String::from_utf16_lossy(&buffer)
        .trim_end_matches('\0')
        .to_owned())
}

fn is_bluetooth_device_node(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    instance_id: &str,
) -> Result<bool, BluetoothError> {
    if has_bluetooth_enumerator(instance_id) {
        return Ok(true);
    }
    if !instance_id
        .split('\\')
        .next()
        .is_some_and(|enumerator| enumerator.eq_ignore_ascii_case("SWD"))
    {
        return Ok(false);
    }
    match read_device_property(
        device_info_set,
        info,
        &DEVPKEY_Device_Parent,
        "could not read PnP device parent",
    )? {
        Some((property_type, bytes)) if property_type == DEVPROP_TYPE_STRING => Ok(
            has_bluetooth_enumerator(&decode_device_property_string(&bytes)),
        ),
        Some(_) => Err(error("PnP device parent property had an unexpected type")),
        None => Ok(false),
    }
}

fn has_bluetooth_enumerator(instance_id: &str) -> bool {
    instance_id
        .split('\\')
        .next()
        .is_some_and(|enumerator| enumerator.to_ascii_uppercase().starts_with("BTH"))
}

fn read_device_property(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
    context: &str,
) -> Result<Option<(DEVPROPTYPE, Vec<u8>)>, BluetoothError> {
    let mut property_type = DEVPROPTYPE::default();
    let mut required_size = 0;
    match unsafe {
        SetupDiGetDevicePropertyW(
            device_info_set,
            info,
            key,
            &mut property_type,
            None,
            Some(&mut required_size),
            0,
        )
    } {
        Ok(()) => return Ok(Some((property_type, Vec::new()))),
        Err(error) if windows_error_code(&error) == ERROR_NOT_FOUND.0 => return Ok(None),
        Err(error) if windows_error_code(&error) == ERROR_INSUFFICIENT_BUFFER.0 => {}
        Err(error) => return Err(windows_error(context, error)),
    }
    if required_size == 0 || required_size > 65_536 {
        return Err(error(format!(
            "{context}: Windows returned an invalid property size ({required_size} bytes)"
        )));
    }
    let mut bytes = vec![0; required_size as usize];
    unsafe {
        SetupDiGetDevicePropertyW(
            device_info_set,
            info,
            key,
            &mut property_type,
            Some(&mut bytes),
            Some(&mut required_size),
            0,
        )
    }
    .map_err(|error| windows_error(context, error))?;
    bytes.truncate(required_size as usize);
    Ok(Some((property_type, bytes)))
}

fn decode_device_property_string(bytes: &[u8]) -> String {
    let wide: Vec<_> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&wide)
        .trim_end_matches('\0')
        .to_owned()
}

fn windows_error_code(error: &windows::core::Error) -> u32 {
    error.code().0 as u32 & 0xffff
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
