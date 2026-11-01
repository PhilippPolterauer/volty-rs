use crate::logitech_hidpp::{
    HidppError, HidppFrame, FUNCTION_GET_BATTERY_STATUS, FUNCTION_GET_BATTERY_VOLTAGE,
    FUNCTION_GET_UNIFIED_BATTERY_INFO,
};
pub use volty_battery::{
    BatteryLevel, BatteryPercentage, BatteryPercentageEstimateMethod, BatteryState, BatteryStatus,
};

pub const SOURCE_UNIFIED_BATTERY: &str = "HID++ Unified Battery";
pub const SOURCE_BATTERY_STATUS: &str = "HID++ Battery Status";
pub const SOURCE_BATTERY_VOLTAGE: &str = "HID++ Battery Voltage (estimated)";

/// Single-cell LiPo discharge curve used to estimate a percentage from a
/// measured battery voltage. Values are ordered from highest to lowest
/// millivolts. The curve is an approximation: real cells differ by chemistry,
/// load, temperature, and vendor calibration.
const BATTERY_VOLTAGE_CURVE: [(u16, u8); 12] = [
    (4200, 100),
    (4100, 95),
    (4000, 90),
    (3900, 80),
    (3800, 70),
    (3700, 60),
    (3600, 50),
    (3500, 40),
    (3400, 30),
    (3300, 20),
    (3200, 10),
    (3000, 0),
];

/// Unified Battery feature capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnifiedBatteryCapabilities {
    pub percentage: bool,
    pub rechargeable: bool,
    /// Bit mask for the level buckets supported by the device.
    pub reported_levels: u8,
}

/// Parse the response to Unified Battery's `getBatteryCapabilities` (function 0).
pub fn parse_unified_battery_capabilities(
    frame: &HidppFrame,
) -> Result<UnifiedBatteryCapabilities, HidppError> {
    if frame.feature_index == 0 || frame.function_id != 0 || frame.params.len() < 2 {
        return Err(HidppError::UnexpectedResponse(
            "invalid unified battery capabilities response",
        ));
    }
    Ok(UnifiedBatteryCapabilities {
        reported_levels: frame.params[0] & 0x0f,
        rechargeable: frame.params[1] & 0x01 != 0,
        percentage: frame.params[1] & 0x02 != 0,
    })
}

/// Parse a response to Unified Battery's `getBatteryInfo` (function 1).
///
/// The percentage field is meaningful only when `capabilities.percentage` is
/// true. Byte 1 is a coarse battery-level bit value; byte 2 is the charging
/// status enum. Unknown status values are preserved and mapped to `Unknown`.
pub fn parse_unified_battery(
    frame: &HidppFrame,
    capabilities: UnifiedBatteryCapabilities,
) -> Result<BatteryStatus, HidppError> {
    if frame.feature_index == 0 || frame.function_id != FUNCTION_GET_UNIFIED_BATTERY_INFO {
        return Err(HidppError::UnexpectedResponse(
            "not a unified battery info response",
        ));
    }
    if frame.params.len() < 3 {
        return Err(HidppError::UnexpectedResponse(
            "missing unified battery info payload",
        ));
    }
    let level_percent = if capabilities.percentage {
        parse_level(frame.params[0])?
    } else {
        None
    };
    let raw_level = frame.params[1];
    let level = match raw_level {
        1 => BatteryLevel::Critical,
        2 => BatteryLevel::Low,
        4 => BatteryLevel::Good,
        8 => BatteryLevel::Full,
        _ => BatteryLevel::Unknown,
    };
    let raw_status = frame.params[2];
    let state = parse_charge_status(raw_status);
    Ok(BatteryStatus {
        percentage: level_percent.map_or(BatteryPercentage::Unavailable, BatteryPercentage::Exact),
        level,
        raw_level,
        state,
        raw_status: Some(raw_status),
        voltage_mv: None,
        source: SOURCE_UNIFIED_BATTERY,
    })
}

/// Parse the legacy HID++ Battery Status (0x1000) response.
///
/// The response contains the device's discharge level, next lower threshold,
/// and status (0 discharging, 1 recharging, 2 almost full, 3 full, 4 slow
/// recharge, 5 invalid battery, 6 thermal error, 7 other error). The legacy
/// level is device-specific and is not assumed to be a percentage.
pub fn parse_legacy_battery_status(frame: &HidppFrame) -> Result<BatteryStatus, HidppError> {
    if frame.feature_index == 0 || frame.function_id != FUNCTION_GET_BATTERY_STATUS {
        return Err(HidppError::UnexpectedResponse(
            "not a legacy battery status response",
        ));
    }
    if frame.params.len() < 3 {
        return Err(HidppError::UnexpectedResponse(
            "missing legacy battery status payload",
        ));
    }
    let raw_level = frame.params[0];
    let raw_status = frame.params[2];
    Ok(BatteryStatus {
        percentage: BatteryPercentage::Unavailable,
        level: BatteryLevel::Unknown,
        raw_level,
        state: parse_charge_status(raw_status),
        raw_status: Some(raw_status),
        voltage_mv: None,
        source: SOURCE_BATTERY_STATUS,
    })
}

/// Parse the HID++ Battery Voltage (0x1001) response.
///
/// The response carries a big-endian 16-bit millivolt value in
/// `params[0..2]`. The device reports no percentage or charging state, so the
/// percentage is estimated from a single-cell LiPo curve and the state stays
/// `Unknown`. A zero reading is rejected because it cannot come from a
/// connected, discharging cell.
pub fn parse_battery_voltage(frame: &HidppFrame) -> Result<BatteryStatus, HidppError> {
    if frame.feature_index == 0 || frame.function_id != FUNCTION_GET_BATTERY_VOLTAGE {
        return Err(HidppError::UnexpectedResponse(
            "not a battery voltage response",
        ));
    }
    if frame.params.len() < 2 {
        return Err(HidppError::UnexpectedResponse(
            "missing battery voltage payload",
        ));
    }
    let voltage_mv = u16::from_be_bytes([frame.params[0], frame.params[1]]);
    if voltage_mv == 0 {
        return Err(HidppError::UnexpectedResponse(
            "battery voltage response reports 0 mV",
        ));
    }
    let level_percent = estimate_percent_from_voltage(voltage_mv);
    Ok(BatteryStatus {
        percentage: BatteryPercentage::Estimated {
            value: level_percent,
            method: BatteryPercentageEstimateMethod::VoltageCurve,
        },
        level: voltage_level(level_percent),
        raw_level: 0,
        state: BatteryState::Unknown,
        raw_status: None,
        voltage_mv: Some(voltage_mv),
        source: SOURCE_BATTERY_VOLTAGE,
    })
}

/// Estimate a percentage from a measured battery voltage (in millivolts) by
/// linearly interpolating the documented single-cell LiPo curve. Values above
/// the top point clamp to 100 and values below the bottom point clamp to 0.
pub(crate) fn estimate_percent_from_voltage(voltage_mv: u16) -> u8 {
    let (top_mv, top_percent) = BATTERY_VOLTAGE_CURVE[0];
    if voltage_mv >= top_mv {
        return top_percent;
    }
    for window in BATTERY_VOLTAGE_CURVE.windows(2) {
        let (high_mv, high_percent) = window[0];
        let (low_mv, low_percent) = window[1];
        if voltage_mv >= low_mv {
            let span_mv = u32::from(high_mv - low_mv);
            let span_percent = u32::from(high_percent - low_percent);
            let offset_mv = u32::from(voltage_mv - low_mv);
            let offset_percent = (offset_mv * span_percent + span_mv / 2) / span_mv;
            let percent = u32::from(low_percent) + offset_percent;
            return u8::try_from(percent.min(100)).unwrap_or(100);
        }
    }
    0
}

fn voltage_level(percent: u8) -> BatteryLevel {
    match percent {
        0..=10 => BatteryLevel::Critical,
        11..=20 => BatteryLevel::Low,
        21..=99 => BatteryLevel::Good,
        _ => BatteryLevel::Full,
    }
}

fn parse_charge_status(raw_status: u8) -> BatteryState {
    match raw_status {
        0 => BatteryState::Discharging,
        1 => BatteryState::Charging,
        2 => BatteryState::ChargingNearlyFull,
        3 => BatteryState::Charged,
        4 => BatteryState::ChargingSlow,
        5 => BatteryState::InvalidBattery,
        6 => BatteryState::ThermalError,
        7 => BatteryState::ChargingError,
        _ => BatteryState::Unknown,
    }
}

fn parse_level(level: u8) -> Result<Option<u8>, HidppError> {
    match level {
        0..=100 => Ok(Some(level)),
        value => Err(HidppError::InvalidBatteryLevel(value)),
    }
}
