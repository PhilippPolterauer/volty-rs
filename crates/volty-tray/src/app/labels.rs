use volty_battery::{BatteryLevel, BatterySource, BatteryState, BatteryStatus};
pub(super) fn format_device_name(name: &str, device_index: Option<u8>) -> String {
    match device_index {
        None => name.to_owned(),
        Some(0xff) => format!("{name} (direct)"),
        Some(device_index) => format!("{name} (slot {device_index})"),
    }
}
pub(super) fn append_config_error(status: &mut String, error: Option<&str>) {
    if let Some(error) = error {
        status.push_str("; notification config error: ");
        status.push_str(error);
    }
}
pub(super) fn battery_status_label(battery: &BatteryStatus) -> String {
    let state = match battery.state {
        BatteryState::Unknown => "Unknown charge state",
        BatteryState::Discharging => "Discharging",
        BatteryState::Charging => "Charging",
        BatteryState::ChargingNearlyFull => "Charging — nearly full",
        BatteryState::Charged => "Fully charged",
        BatteryState::ChargingSlow => "Charging slowly",
        BatteryState::InvalidBattery => "Battery error: invalid battery",
        BatteryState::ThermalError => "Battery error: thermal",
        BatteryState::ChargingError => "Battery error: charging",
    };
    let level = match battery.level {
        BatteryLevel::Unknown => None,
        BatteryLevel::Critical => Some("critical level"),
        BatteryLevel::Low => Some("low level"),
        BatteryLevel::Good => Some("good level"),
        BatteryLevel::Full => Some("full level"),
    };
    match level {
        Some(level) => format!("{state} ({level})"),
        None => state.to_owned(),
    }
}
pub(super) fn battery_source_label(source: BatterySource) -> &'static str {
    match source {
        BatterySource::Unified => "HID++ Unified Battery",
        BatterySource::LegacyBatteryStatus => "HID++ Battery Status",
        BatterySource::BluetoothGatt => "Bluetooth GATT Battery Service",
        BatterySource::BluetoothDeviceProperty => "Windows Bluetooth battery property",
    }
}
pub(super) fn battery_is_full(battery: &BatteryStatus) -> bool {
    matches!(battery.state, BatteryState::Charged)
        || battery
            .level_percent
            .is_some_and(|percentage| percentage >= 100)
}
#[cfg(test)]
mod tests {
    use super::*;
    use volty_battery::{BatteryLevel, BatterySource, BatteryState, BatteryStatus};
    #[test]
    fn coarse_full_level_does_not_mark_a_discharging_battery_as_full() {
        let battery = BatteryStatus {
            level_percent: Some(77),
            level: BatteryLevel::Full,
            raw_level: 8,
            state: BatteryState::Discharging,
            raw_status: Some(0),
            source: BatterySource::Unified,
        };

        assert!(!battery_is_full(&battery));
        assert_eq!(battery_status_label(&battery), "Discharging (full level)");
    }
    #[test]
    #[cfg(feature = "bluetooth")]
    fn bluetooth_battery_source_is_explicit_in_diagnostics() {
        assert_eq!(
            battery_source_label(BatterySource::BluetoothGatt),
            "Bluetooth GATT Battery Service"
        );
        assert_eq!(
            battery_source_label(BatterySource::BluetoothDeviceProperty),
            "Windows Bluetooth battery property"
        );
    }
}
