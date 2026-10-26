use crate::protocol::{
    HidppError, HidppFrame, FUNCTION_GET_BATTERY_STATUS, FUNCTION_GET_UNIFIED_BATTERY_INFO,
};

/// The known charging/discharging condition, or `Unknown` if unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryState {
    Unknown,
    Discharging,
    Charging,
    ChargingNearlyFull,
    Charged,
    ChargingSlow,
    InvalidBattery,
    ThermalError,
    ChargingError,
}

/// Coarse battery-level bucket reported by Unified Battery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryLevel {
    Unknown,
    Critical,
    Low,
    Good,
    Full,
}

/// Origin of a parsed battery status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatterySource {
    Unified,
    LegacyBatteryStatus,
}

/// Normalized battery reading. Level is `None` when the device reports an
/// unknown/sentinel value; `raw_status` preserves the device status byte.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatteryStatus {
    /// A percentage only when the device advertises percentage support.
    pub level_percent: Option<u8>,
    /// Coarse Unified Battery bucket, when available.
    pub level: BatteryLevel,
    /// Raw level byte returned by the device. Legacy Battery Status values are
    /// device-specific charge readings and are not assumed to be percentages.
    pub raw_level: u8,
    pub state: BatteryState,
    pub raw_status: u8,
    pub source: BatterySource,
}

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
        level_percent,
        level,
        raw_level,
        state,
        raw_status,
        source: BatterySource::Unified,
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
        level_percent: None,
        level: BatteryLevel::Unknown,
        raw_level,
        state: parse_charge_status(raw_status),
        raw_status,
        source: BatterySource::LegacyBatteryStatus,
    })
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
