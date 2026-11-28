use volty_battery::{BatteryLevel, BatteryPercentage, BatteryState, BatteryStatus};

pub const BLUEZ_SOURCE: &str = "BlueZ Battery1";
pub const BLUEZ_PROPERTY: &str = "org.bluez.Battery1.Percentage";
pub const UPOWER_SOURCE: &str = "UPower";

pub fn bluez_battery(value: u8) -> Option<BatteryStatus> {
    (value <= 100).then(|| volty_bluetooth::battery_status(value, BLUEZ_SOURCE))
}

pub fn upower_battery(
    percentage: Option<f64>,
    battery_level: Option<u32>,
    state: Option<u32>,
) -> Option<BatteryStatus> {
    let raw_level = battery_level.unwrap_or(1);
    let level = match raw_level {
        3 => BatteryLevel::Low,
        4 | 5 => BatteryLevel::Critical,
        6 | 7 => BatteryLevel::Good,
        8 => BatteryLevel::Full,
        _ => BatteryLevel::Unknown,
    };
    let exact = percentage.filter(|value| raw_level == 1 && (0.0..=100.0).contains(value));
    if exact.is_none() && level == BatteryLevel::Unknown {
        return None;
    }
    let raw_status = state.and_then(|value| u8::try_from(value).ok());
    Some(BatteryStatus {
        percentage: exact.map_or(BatteryPercentage::Unavailable, |value| {
            BatteryPercentage::Exact(value.round() as u8)
        }),
        level,
        raw_level: u8::try_from(raw_level).unwrap_or_default(),
        state: match raw_status {
            Some(1) => BatteryState::Charging,
            Some(2 | 3) => BatteryState::Discharging,
            Some(4) => BatteryState::Charged,
            _ => BatteryState::Unknown,
        },
        raw_status,
        voltage_mv: None,
        source: UPOWER_SOURCE,
    })
}
