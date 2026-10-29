use volty_battery::{BatteryLevel, BatteryState, BatteryStatus};
pub(super) fn append_config_error(status: &mut String, error: Option<&str>) {
    if let Some(error) = error {
        status.push_str("; notification config error: ");
        status.push_str(error);
    }
}
pub(super) fn battery_state_label(battery: &BatteryStatus) -> &'static str {
    match battery.state {
        BatteryState::Unknown => "Unknown charge state",
        BatteryState::Discharging => "Discharging",
        BatteryState::Charging => "Charging",
        BatteryState::ChargingNearlyFull => "Charging — nearly full",
        BatteryState::Charged => "Fully charged",
        BatteryState::ChargingSlow => "Charging slowly",
        BatteryState::InvalidBattery => "Battery error: invalid battery",
        BatteryState::ThermalError => "Battery error: thermal",
        BatteryState::ChargingError => "Battery error: charging",
    }
}
pub(super) fn battery_status_label(battery: &BatteryStatus) -> String {
    let state = battery_state_label(battery);
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
pub(super) fn battery_is_full(battery: &BatteryStatus) -> bool {
    matches!(battery.state, BatteryState::Charged)
        || battery
            .level_percent
            .is_some_and(|percentage| percentage >= 100)
}
#[cfg(test)]
mod tests {
    use super::*;
    use volty_battery::{BatteryLevel, BatteryState, BatteryStatus};
    #[test]
    fn coarse_full_level_does_not_mark_a_discharging_battery_as_full() {
        let battery = BatteryStatus {
            level_percent: Some(77),
            level: BatteryLevel::Full,
            raw_level: 8,
            state: BatteryState::Discharging,
            raw_status: Some(0),
            source: "test",
        };

        assert!(!battery_is_full(&battery));
        assert_eq!(battery_status_label(&battery), "Discharging (full level)");
    }
}
