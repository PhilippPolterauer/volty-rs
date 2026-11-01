use windows::{
    core::{GUID, PCWSTR},
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
                SetupDiGetDeviceInstanceIdW, SetupDiGetDevicePropertyW, SetupDiOpenDeviceInfoW,
                DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA,
            },
            Properties::{
                DEVPKEY_Device_FriendlyName, DEVPKEY_Device_Parent, DEVPROPTYPE, DEVPROP_TYPE_BYTE,
                DEVPROP_TYPE_STRING, DEVPROP_TYPE_UINT32,
            },
        },
        Foundation::{DEVPROPKEY, ERROR_INSUFFICIENT_BUFFER, ERROR_NOT_FOUND, ERROR_NO_MORE_ITEMS},
    },
};

use super::{
    budget::{push_scan_budget_warning, ScanBudget},
    winrt::windows_error,
    ConnectedClassicDevice, WINDOWS_BATTERY_LIFE_PROPERTY,
};
use crate::{
    battery_status, decode_windows_battery_life, error, BluetoothBatteryReading,
    BluetoothDeviceKind, BluetoothError, BluetoothEvidence, BluetoothObservation, BluetoothScan,
    FALLBACK_DEVICE_NAME,
};

const MAX_DEVICE_INSTANCE_ID_UNITS: u32 = 65_536;
// System.Devices.BatteryLife, exposed by PnP as DEVPKEY_Device_BatteryLevel.
// {104EA319-6EE2-4701-BD47-8DDBF425BBE5}, 2
const BATTERY_LIFE_PROPERTY_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};
// DEVPKEY_Bluetooth_DeviceFlags, the BDIF_* state flags of a Bluetooth device.
// {2BD67D8B-8BEB-48D5-87E0-6CDA3428040A}, 3
const BLUETOOTH_DEVICE_FLAGS_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a),
    pid: 3,
};
// DEVPKEY_Bluetooth_DeviceAddress.
// {2BD67D8B-8BEB-48D5-87E0-6CDA3428040A}, 1
const BLUETOOTH_DEVICE_ADDRESS_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a),
    pid: 1,
};
const BDIF_CONNECTED: u32 = 0x0000_0020;
const BDIF_LE_CONNECTED: u32 = 0x0100_0000;
const MAX_BLUETOOTH_PARENT_DEPTH: usize = 8;
pub(super) fn scan_setupapi_battery_property(
    budget: &ScanBudget,
    connected_classic_devices: &[ConnectedClassicDevice],
) -> Result<BluetoothScan, BluetoothError> {
    // SAFETY: null filters request all present devices; the returned set is owned by DeviceInfoSet below.
    let device_info_set = unsafe {
        SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
    }
    .map_err(|error| windows_error("could not enumerate present PnP devices", error))?;
    let device_info_set = DeviceInfoSet(device_info_set);
    let mut scan = BluetoothScan::default();
    let mut counts = SetupApiScanCounts::default();
    let mut index = 0;

    while let Some(info) = next_device_info(device_info_set.0, index, budget, &mut scan)? {
        index = index
            .checked_add(1)
            .ok_or_else(|| error("PnP device index overflow"))?;
        let outcome = inspect_device(
            device_info_set.0,
            &info,
            connected_classic_devices,
            &mut scan,
        );
        counts.bluetooth_nodes_examined += usize::from(outcome.bluetooth_node);
        counts.battery_properties_found += usize::from(outcome.battery_property_found);
        counts.disconnected_nodes_skipped += usize::from(outcome.disconnected_node);
    }

    if counts.bluetooth_nodes_examined > 0 {
        scan.discovery_events.push(format!(
            "SetupAPI examined {} Bluetooth device node(s), skipped {} not connected, and found {} {WINDOWS_BATTERY_LIFE_PROPERTY} property value(s)",
            counts.bluetooth_nodes_examined,
            counts.disconnected_nodes_skipped,
            counts.battery_properties_found
        ));
    }
    Ok(scan)
}

#[derive(Default)]
struct SetupApiScanCounts {
    bluetooth_nodes_examined: usize,
    battery_properties_found: usize,
    disconnected_nodes_skipped: usize,
}

#[derive(Default)]
struct DeviceScanOutcome {
    bluetooth_node: bool,
    battery_property_found: bool,
    disconnected_node: bool,
}

enum PnpConnectionMatch<'a> {
    ConnectedByFlags,
    ConnectedClassic(&'a ConnectedClassicDevice),
    Disconnected,
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
        cbSize: u32::try_from(std::mem::size_of::<SP_DEVINFO_DATA>())
            .expect("SP_DEVINFO_DATA size fits DWORD"),
        ..SP_DEVINFO_DATA::default()
    };
    // SAFETY: the set is live and info is an initialized SP_DEVINFO_DATA with its required cbSize.
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
    connected_classic_devices: &[ConnectedClassicDevice],
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
    inspect_bluetooth_device(
        device_info_set,
        info,
        instance_id,
        connected_classic_devices,
        scan,
    )
}

fn inspect_bluetooth_device(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    instance_id: String,
    connected_classic_devices: &[ConnectedClassicDevice],
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
                ..DeviceScanOutcome::default()
            };
        }
    };
    let Some((property_type, bytes)) = battery_property else {
        return DeviceScanOutcome {
            bluetooth_node: true,
            ..DeviceScanOutcome::default()
        };
    };
    let outcome = DeviceScanOutcome {
        bluetooth_node: true,
        battery_property_found: true,
        ..DeviceScanOutcome::default()
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
    let (device_id, name) = match bluetooth_connection_state(
        device_info_set,
        info,
        &instance_id,
        connected_classic_devices,
    ) {
        Ok(PnpConnectionMatch::ConnectedByFlags) => (instance_id.clone(), name),
        Ok(PnpConnectionMatch::ConnectedClassic(device)) => {
            scan.discovery_events.push(format!(
                "{name:?}: matched connected Classic device {:?} by Bluetooth address; using its device identity",
                device.name
            ));
            (device.device_id.clone(), device.name.clone())
        }
        Ok(PnpConnectionMatch::Disconnected) => {
            scan.discovery_events.push(format!(
                "{name:?}: found a Windows PnP battery property, but it did not match a connected Bluetooth address and no connected flag was present"
            ));
            return DeviceScanOutcome {
                disconnected_node: true,
                ..outcome
            };
        }
        Err(error) => {
            scan.warnings.push(error.to_string());
            return DeviceScanOutcome {
                disconnected_node: true,
                ..outcome
            };
        }
    };
    scan.evidence.push(BluetoothEvidence {
        device_id: device_id.clone(),
        name: name.clone(),
        kind: BluetoothDeviceKind::Pnp,
        observation: BluetoothObservation::Property {
            property: "DEVPKEY_Device_BatteryLevel",
            property_type: "byte",
            raw_value: bytes[0],
        },
    });
    match decode_windows_battery_life(bytes[0]) {
        Ok(Some(level)) => {
            scan.discovery_events.push(format!(
                "{name:?}: read battery level from the Windows PnP {WINDOWS_BATTERY_LIFE_PROPERTY} property"
            ));
            scan.readings.push(BluetoothBatteryReading {
                device_id,
                name,
                kind: BluetoothDeviceKind::Pnp,
                battery: battery_status(level, crate::SOURCE_DEVICE_PROPERTY),
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

/// Matches the node or an ancestor to a connected device, preferring its
/// canonical identity over per-profile PnP identities.
fn bluetooth_connection_state<'a>(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
    instance_id: &str,
    connected_classic_devices: &'a [ConnectedClassicDevice],
) -> Result<PnpConnectionMatch<'a>, BluetoothError> {
    let mut current = *info;
    let mut flags_connected = false;
    if let Some(address) = bluetooth_address_from_instance_id(instance_id) {
        if let Some(device) =
            connected_classic_device_for_address(address, connected_classic_devices)
        {
            return Ok(PnpConnectionMatch::ConnectedClassic(device));
        }
    }
    for _ in 0..MAX_BLUETOOTH_PARENT_DEPTH {
        if let Some((property_type, bytes)) = read_device_property(
            device_info_set,
            &current,
            &BLUETOOTH_DEVICE_ADDRESS_KEY,
            "could not read Bluetooth device address",
        )? {
            let address = decode_bluetooth_device_address(property_type, &bytes)?;
            if let Some(device) =
                connected_classic_device_for_address(address, connected_classic_devices)
            {
                return Ok(PnpConnectionMatch::ConnectedClassic(device));
            }
        }
        if let Some((property_type, bytes)) = read_device_property(
            device_info_set,
            &current,
            &BLUETOOTH_DEVICE_FLAGS_KEY,
            "could not read Bluetooth device flags",
        )? {
            flags_connected |= decode_bluetooth_device_flags(property_type, &bytes)?;
        }
        let Some(parent_id) = read_device_parent(device_info_set, &current)? else {
            return Ok(if flags_connected {
                PnpConnectionMatch::ConnectedByFlags
            } else {
                PnpConnectionMatch::Disconnected
            });
        };
        if !has_bluetooth_enumerator(&parent_id) {
            return Ok(if flags_connected {
                PnpConnectionMatch::ConnectedByFlags
            } else {
                PnpConnectionMatch::Disconnected
            });
        }
        if let Some(address) = bluetooth_address_from_instance_id(&parent_id) {
            if let Some(device) =
                connected_classic_device_for_address(address, connected_classic_devices)
            {
                return Ok(PnpConnectionMatch::ConnectedClassic(device));
            }
        }
        let parent_id = windows::core::HSTRING::from(parent_id);
        let mut parent = SP_DEVINFO_DATA {
            cbSize: u32::try_from(std::mem::size_of::<SP_DEVINFO_DATA>())
                .expect("SP_DEVINFO_DATA size fits DWORD"),
            ..SP_DEVINFO_DATA::default()
        };
        // SAFETY: the set is live, parent_id is a valid NUL-terminated HSTRING, and parent is an initialized SP_DEVINFO_DATA.
        unsafe {
            SetupDiOpenDeviceInfoW(device_info_set, &parent_id, None, 0, Some(&raw mut parent))
        }
        .map_err(|error| windows_error("could not open Bluetooth parent device", error))?;
        current = parent;
    }
    Ok(if flags_connected {
        PnpConnectionMatch::ConnectedByFlags
    } else {
        PnpConnectionMatch::Disconnected
    })
}

fn connected_classic_device_for_address(
    address: u64,
    connected_devices: &[ConnectedClassicDevice],
) -> Option<&ConnectedClassicDevice> {
    connected_devices
        .iter()
        .find(|device| device.address == address)
}

fn decode_bluetooth_device_flags(
    property_type: DEVPROPTYPE,
    bytes: &[u8],
) -> Result<bool, BluetoothError> {
    if property_type != DEVPROP_TYPE_UINT32 || bytes.len() != 4 {
        return Err(error(format!(
            "Bluetooth device flags have an unexpected type or size (type={}, bytes={})",
            property_type.0,
            bytes.len()
        )));
    }
    let flags = u32::from_le_bytes(bytes.try_into().expect("checked four flag bytes"));
    Ok(bluetooth_flags_are_connected(flags))
}

fn bluetooth_flags_are_connected(flags: u32) -> bool {
    flags & (BDIF_CONNECTED | BDIF_LE_CONNECTED) != 0
}

fn decode_bluetooth_device_address(
    property_type: DEVPROPTYPE,
    bytes: &[u8],
) -> Result<u64, BluetoothError> {
    if property_type != DEVPROP_TYPE_STRING {
        return Err(error(format!(
            "Bluetooth device address has an unexpected type (type={})",
            property_type.0
        )));
    }
    let address = decode_device_property_string(bytes);
    let hex_digits: String = address.chars().filter(char::is_ascii_hexdigit).collect();
    if hex_digits.len() != 12
        || !address
            .chars()
            .all(|character| character.is_ascii_hexdigit() || matches!(character, ':' | '-' | ' '))
    {
        return Err(error("Bluetooth device address has an invalid format"));
    }
    u64::from_str_radix(&hex_digits, 16)
        .map_err(|_| error("Bluetooth device address could not be decoded"))
}

fn bluetooth_address_from_instance_id(instance_id: &str) -> Option<u64> {
    let mut parts = instance_id.split('\\');
    let enumerator = parts.next()?;
    let (device_prefix, address) = parts.next()?.split_once('_')?;
    if !enumerator.eq_ignore_ascii_case("BTHENUM")
        || !device_prefix.eq_ignore_ascii_case("DEV")
        || address.len() != 12
        || !address
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    u64::from_str_radix(address, 16).ok()
}

struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the valid HDEVINFO and Drop attempts destruction on every exit path.
        if let Err(error) = unsafe { SetupDiDestroyDeviceInfoList(self.0) } {
            eprintln!("could not destroy Windows device-info set: {error}");
        }
    }
}

fn read_device_instance_id(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
) -> Result<String, BluetoothError> {
    let mut required_size = 0;
    // SAFETY: device_info_set and info are live; None performs the documented size query and required_size is writable.
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
    if required_size > MAX_DEVICE_INSTANCE_ID_UNITS {
        return Err(error(format!(
            "PnP device identity size is unreasonable ({required_size} UTF-16 units)"
        )));
    }
    let buffer_len = usize::try_from(required_size)
        .map_err(|_| error("PnP device identity size does not fit memory"))?;
    let mut buffer = vec![0u16; buffer_len];
    // SAFETY: the live device info is paired with an output slice sized by the successful size query.
    unsafe {
        SetupDiGetDeviceInstanceIdW(
            device_info_set,
            info,
            Some(&mut buffer),
            Some(&raw mut required_size),
        )
    }
    .map_err(|error| windows_error("could not read PnP device identity", error))?;
    let written = checked_written_size(required_size, buffer.len(), "PnP device identity")?;
    Ok(String::from_utf16_lossy(&buffer[..written])
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
    Ok(read_device_parent(device_info_set, info)?
        .is_some_and(|parent| has_bluetooth_enumerator(&parent)))
}

fn read_device_parent(
    device_info_set: HDEVINFO,
    info: &SP_DEVINFO_DATA,
) -> Result<Option<String>, BluetoothError> {
    match read_device_property(
        device_info_set,
        info,
        &DEVPKEY_Device_Parent,
        "could not read PnP device parent",
    )? {
        Some((DEVPROP_TYPE_STRING, bytes)) => Ok(Some(decode_device_property_string(&bytes))),
        Some(_) => Err(error("PnP device parent property had an unexpected type")),
        None => Ok(None),
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
    // SAFETY: device and key are valid; null output queries byte size into writable local outputs.
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
    let buffer_len = usize::try_from(required_size)
        .map_err(|_| error(format!("{context}: property size does not fit memory")))?;
    let mut bytes = vec![0; buffer_len];
    // SAFETY: bytes is a writable allocation of required_size bytes, and property_type/required_size are writable locals.
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
    let written = checked_written_size(required_size, bytes.len(), context)?;
    bytes.truncate(written);
    Ok(Some((property_type, bytes)))
}

fn checked_written_size(
    size: u32,
    capacity: usize,
    context: &str,
) -> Result<usize, BluetoothError> {
    let size = usize::try_from(size)
        .map_err(|_| error(format!("{context}: returned size does not fit memory")))?;
    if size > capacity {
        return Err(error(format!(
            "{context}: Windows reported {size} bytes for a {capacity}-byte buffer"
        )));
    }
    Ok(size)
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
    use super::{
        bluetooth_address_from_instance_id, bluetooth_flags_are_connected, checked_written_size,
        connected_classic_device_for_address, decode_bluetooth_device_address,
        decode_bluetooth_device_flags, decode_device_property_string, has_bluetooth_enumerator,
        is_software_device_id, BDIF_CONNECTED, BDIF_LE_CONNECTED,
    };
    use crate::windows_backend::ConnectedClassicDevice;
    use windows::Win32::Devices::Properties::{DEVPROP_TYPE_STRING, DEVPROP_TYPE_UINT32};

    #[test]
    fn rejects_api_lengths_larger_than_the_provided_buffer() {
        assert_eq!(checked_written_size(3, 4, "property").unwrap(), 3);
        assert!(checked_written_size(5, 4, "property").is_err());
    }

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

    #[test]
    fn accepts_only_connected_bluetooth_device_flags() {
        assert!(bluetooth_flags_are_connected(BDIF_CONNECTED));
        assert!(bluetooth_flags_are_connected(BDIF_LE_CONNECTED));
        assert!(bluetooth_flags_are_connected(
            BDIF_CONNECTED | BDIF_LE_CONNECTED
        ));
        // A paired but disconnected device keeps the other BDIF_* flags.
        assert!(!bluetooth_flags_are_connected(0x0400_439f));
        assert!(!bluetooth_flags_are_connected(0));
    }

    #[test]
    fn decodes_classic_bluetooth_addresses_from_device_instance_ids() {
        assert_eq!(
            bluetooth_address_from_instance_id("BTHENUM\\DEV_001122AABBCC\\8&123"),
            Some(0x001122aabbcc)
        );
        assert_eq!(
            bluetooth_address_from_instance_id("bthenum\\dev_001122AABBCC\\8&123"),
            Some(0x001122aabbcc)
        );
        assert_eq!(
            bluetooth_address_from_instance_id("BTHHFENUM\\DEV_001122AABBCC\\8&123"),
            None
        );
    }

    #[test]
    fn maps_connected_addresses_to_the_canonical_classic_device() {
        let device = ConnectedClassicDevice {
            address: 0x001122aabbcc,
            device_id: "classic-device-id".to_owned(),
            name: "Baseus Bass BS2 Lite".to_owned(),
        };
        assert_eq!(
            connected_classic_device_for_address(device.address, std::slice::from_ref(&device))
                .map(|device| device.device_id.as_str()),
            Some("classic-device-id")
        );
        assert!(connected_classic_device_for_address(
            0x001122aabbcd,
            std::slice::from_ref(&device)
        )
        .is_none());
    }

    #[test]
    fn decodes_bluetooth_address_properties() {
        let address = "00:11:22:AA:BB:CC\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            decode_bluetooth_device_address(DEVPROP_TYPE_STRING, &address).unwrap(),
            0x001122aabbcc
        );
        assert!(decode_bluetooth_device_address(DEVPROP_TYPE_STRING, &[]).is_err());
        assert!(decode_bluetooth_device_address(DEVPROP_TYPE_UINT32, &address).is_err());
    }

    #[test]
    fn decodes_bluetooth_device_flags_from_a_uint32_property() {
        let bytes = BDIF_CONNECTED.to_le_bytes();
        assert!(decode_bluetooth_device_flags(DEVPROP_TYPE_UINT32, &bytes).unwrap());
        assert!(!decode_bluetooth_device_flags(DEVPROP_TYPE_UINT32, &0u32.to_le_bytes()).unwrap());
        assert!(decode_bluetooth_device_flags(DEVPROP_TYPE_STRING, &bytes).is_err());
        assert!(decode_bluetooth_device_flags(DEVPROP_TYPE_UINT32, &bytes[..2]).is_err());
    }
}
