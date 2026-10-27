use windows::{
    core::{GUID, PCWSTR},
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
    },
};

use super::{
    budget::{push_scan_budget_warning, ScanBudget},
    winrt::windows_error,
    WINDOWS_BATTERY_LIFE_PROPERTY,
};
use crate::{
    battery_status, decode_windows_battery_life, error, BluetoothBatteryReading, BluetoothError,
    BluetoothScan, FALLBACK_DEVICE_NAME,
};

const BATTERY_LIFE_PROPERTY_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x49cd1f76_5626_4b17_a4e8_18b4aa1a2213),
    pid: 10,
};
pub(super) fn scan_setupapi_battery_property(
    budget: &ScanBudget,
) -> Result<BluetoothScan, BluetoothError> {
    let device_info_set = unsafe {
        SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
    }
    .map_err(|error| windows_error("could not enumerate present PnP devices", error))?;
    let device_info_set = DeviceInfoSet(device_info_set);
    let mut scan = BluetoothScan::default();
    let mut counts = SetupApiScanCounts::default();
    let mut index = 0;

    while let Some(info) = next_device_info(device_info_set.0, index, budget, &mut scan)? {
        index += 1;
        let outcome = inspect_device(device_info_set.0, &info, &mut scan);
        counts.bluetooth_nodes_examined += usize::from(outcome.bluetooth_node);
        counts.battery_properties_found += usize::from(outcome.battery_property_found);
    }

    if counts.bluetooth_nodes_examined > 0 {
        scan.discovery_events.push(format!(
            "SetupAPI examined {} Bluetooth device node(s) and found {} {WINDOWS_BATTERY_LIFE_PROPERTY} property value(s)",
            counts.bluetooth_nodes_examined,
            counts.battery_properties_found
        ));
    }
    Ok(scan)
}

#[derive(Default)]
struct SetupApiScanCounts {
    bluetooth_nodes_examined: usize,
    battery_properties_found: usize,
}

#[derive(Default)]
struct DeviceScanOutcome {
    bluetooth_node: bool,
    battery_property_found: bool,
}

fn next_device_info(
    device_info_set: HDEVINFO,
    index: u32,
    budget: &ScanBudget,
    scan: &mut BluetoothScan,
) -> Result<Option<SP_DEVINFO_DATA>, BluetoothError> {
    if budget.is_exhausted() {
        push_scan_budget_warning(scan, "Windows Bluetooth device-property");
        return Ok(None);
    }
    let mut info = SP_DEVINFO_DATA {
        cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
        ..SP_DEVINFO_DATA::default()
    };
    match unsafe { SetupDiEnumDeviceInfo(device_info_set, index, &raw mut info) } {
        Ok(()) => {}
        Err(error) if windows_error_code(&error) == ERROR_NO_MORE_ITEMS.0 => return Ok(None),
        Err(_) if budget.is_exhausted() => {
            push_scan_budget_warning(scan, "Windows Bluetooth device-property");
            return Ok(None);
        }
        Err(error) => {
            return Err(windows_error(
                "could not enumerate present PnP devices",
                error,
            ))
        }
    }
    if budget.is_exhausted() {
        push_scan_budget_warning(scan, "Windows Bluetooth device-property");
        return Ok(None);
    }
    Ok(Some(info))
}

fn inspect_device(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    scan: &mut BluetoothScan,
) -> DeviceScanOutcome {
    let instance_id = match read_device_instance_id(device_info_set, info) {
        Ok(instance_id) => instance_id,
        Err(error) => {
            scan.warnings.push(error.to_string());
            return DeviceScanOutcome::default();
        }
    };
    match is_bluetooth_device_node(device_info_set, info, &instance_id) {
        Ok(true) => {}
        Ok(false) => return DeviceScanOutcome::default(),
        Err(error) => {
            scan.warnings.push(error.to_string());
            return DeviceScanOutcome::default();
        }
    }
    scan.devices_examined += 1;
    inspect_bluetooth_device(device_info_set, info, instance_id, scan)
}

fn inspect_bluetooth_device(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    instance_id: String,
    scan: &mut BluetoothScan,
) -> DeviceScanOutcome {
    let battery_property = match read_device_property(
        device_info_set,
        info,
        &BATTERY_LIFE_PROPERTY_KEY,
        "could not read Windows Bluetooth battery property",
    ) {
        Ok(property) => property,
        Err(error) => {
            scan.warnings.push(error.to_string());
            return DeviceScanOutcome {
                bluetooth_node: true,
                battery_property_found: false,
            };
        }
    };
    let Some((property_type, bytes)) = battery_property else {
        return DeviceScanOutcome {
            bluetooth_node: true,
            battery_property_found: false,
        };
    };
    let outcome = DeviceScanOutcome {
        bluetooth_node: true,
        battery_property_found: true,
    };
    if property_type != DEVPROP_TYPE_BYTE || bytes.len() != 1 {
        scan.warnings.push(format!(
            "Windows Bluetooth battery property has an unexpected type or size (type={}, bytes={})",
            property_type.0,
            bytes.len()
        ));
        return outcome;
    }
    let name = read_device_name(device_info_set, info, scan);
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
    outcome
}

fn read_device_name(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    scan: &mut BluetoothScan,
) -> String {
    match read_device_property(
        device_info_set,
        info,
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
    }
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
        SetupDiGetDeviceInstanceIdW(device_info_set, info, None, Some(&raw mut required_size))
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
            Some(&raw mut required_size),
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
    if !is_software_device_id(instance_id) {
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
            &raw mut property_type,
            None,
            Some(&raw mut required_size),
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
            &raw mut property_type,
            Some(&mut bytes),
            Some(&raw mut required_size),
            0,
        )
    }
    .map_err(|error| windows_error(context, error))?;
    bytes.truncate(required_size as usize);
    Ok(Some((property_type, bytes)))
}

fn decode_device_property_string(bytes: &[u8]) -> String {
    let (pairs, _) = bytes.as_chunks::<2>();
    let wide: Vec<_> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    String::from_utf16_lossy(&wide)
        .trim_end_matches('\0')
        .to_owned()
}

fn windows_error_code(error: &windows::core::Error) -> u32 {
    error.code().0 as u32 & 0xffff
}
fn is_software_device_id(instance_id: &str) -> bool {
    instance_id
        .split('\\')
        .next()
        .is_some_and(|enumerator| enumerator.eq_ignore_ascii_case("SWD"))
}

#[cfg(test)]
mod tests {
    use super::{decode_device_property_string, has_bluetooth_enumerator, is_software_device_id};

    #[test]
    fn decodes_property_strings_and_ignores_incomplete_trailing_bytes() {
        let mut bytes = "Headset\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        bytes.push(0x7f);
        assert_eq!(decode_device_property_string(&bytes), "Headset");
    }

    #[test]
    fn detects_bluetooth_and_software_enumerators() {
        assert!(has_bluetooth_enumerator("BTHENUM\\DEV_1234"));
        assert!(has_bluetooth_enumerator("bthusb\\device"));
        assert!(!has_bluetooth_enumerator("USB\\device"));
        assert!(is_software_device_id("swd\\RADIO\\device"));
        assert!(!is_software_device_id("USB\\device"));
    }
}
