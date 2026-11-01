use volty_battery::{BatteryLevel, BatteryState, BatteryStatus};
pub(super) fn append_config_error(status: &mut String, error: Option<&str>) {
    if let Some(error) = error {
        status.push_str("; notification config error: ");
        status.push_str(error);
    }
}
pub(super) fn battery_status_label(battery: &BatteryStatus) -> String {
    let state = match battery.state {
        BatteryState::Unknown => None,
        BatteryState::Discharging => Some("Discharging"),
        BatteryState::Charging => Some("Charging"),
        BatteryState::ChargingNearlyFull => Some("Charging — nearly full"),
        BatteryState::Charged => Some("Fully charged"),
        BatteryState::ChargingSlow => Some("Charging slowly"),
        BatteryState::InvalidBattery => Some("Battery error: invalid battery"),
        BatteryState::ThermalError => Some("Battery error: thermal"),
        BatteryState::ChargingError => Some("Battery error: charging"),
    };
    let level = match battery.level {
        BatteryLevel::Unknown => None,
        BatteryLevel::Critical => Some("critical level"),
        BatteryLevel::Low => Some("low level"),
        BatteryLevel::Good => Some("good level"),
        BatteryLevel::Full => Some("full level"),
    };
    match (state, level) {
        (Some(state), Some(level)) => format!("{state} ({level})"),
        (Some(state), None) => state.to_owned(),
        (None, Some(level)) => level.to_owned(),
        (None, None) => "Unknown charge state".to_owned(),
    }
}
pub(super) fn battery_is_full(battery: &BatteryStatus) -> bool {
    matches!(battery.state, BatteryState::Charged)
        || battery
            .percentage
            .exact()
            .is_some_and(|percentage| percentage >= 100)
}
#[cfg(test)]
mod tests {
    use super::*;
    use volty_battery::{BatteryLevel, BatteryPercentage, BatteryState, BatteryStatus};
    #[test]
    fn coarse_full_level_does_not_mark_a_discharging_battery_as_full() {
        let battery = BatteryStatus {
            percentage: BatteryPercentage::Exact(77),
            level: BatteryLevel::Full,
            raw_level: 8,
            state: BatteryState::Discharging,
            raw_status: Some(0),
            voltage_mv: None,
            source: "test",
        };

        assert!(!battery_is_full(&battery));
        assert_eq!(battery_status_label(&battery), "Discharging (full level)");
    }

    #[test]
    fn coarse_level_is_shown_when_charge_state_is_unknown() {
        let battery = BatteryStatus {
            percentage: BatteryPercentage::Unavailable,
            level: BatteryLevel::Low,
            raw_level: 1,
            state: BatteryState::Unknown,
            raw_status: None,
            voltage_mv: None,
            source: "test",
        };

        assert_eq!(battery_status_label(&battery), "low level");
    }

    #[test]
    fn estimated_full_percentage_is_not_treated_as_an_exact_full_reading() {
        let battery = BatteryStatus {
            percentage: BatteryPercentage::Estimated {
                value: 100,
                method: volty_battery::BatteryPercentageEstimateMethod::VoltageCurve,
            },
            level: BatteryLevel::Full,
            raw_level: 0,
            state: BatteryState::Unknown,
            raw_status: None,
            voltage_mv: Some(4200),
            source: "test",
        };

        assert!(!battery_is_full(&battery));
    }
}
