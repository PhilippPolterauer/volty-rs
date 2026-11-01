//! Battery decoding for Flydigi's XInput vendor-command response.
//!
//! The XInput report places command `0x10` at byte 15 and the packed battery
//! state/level at byte 23. The level is reported in 20% steps.

use crate::generic_battery::level_bucket;
use crate::{BatteryLevel, BatteryPercentage, BatteryState, BatteryStatus, ProtocolError};

const MIN_REPORT_LEN: usize = 24;
const RESPONSE_PREFIX_OFFSET: usize = 14;
const RESPONSE_PREFIX: u8 = 0xa5;
const COMMAND_OFFSET: usize = 15;
const BATTERY_OFFSET: usize = 23;
const BATTERY_COMMAND: u8 = 0x10;

pub const SOURCE_FLYDIGI_XINPUT: &str = "Flydigi XInput vendor report";

/// Decodes the battery byte in a Flydigi XInput command `0x10` response.
///
/// The byte's high nibble is the state (`0` discharging, `1` charging,
/// `2` charged); its low nibble is a level from zero to five 20% steps.
/// A zero byte is treated as an uninitialized reading rather than an empty
/// battery.
pub fn decode_flydigi_xinput_battery(report: &[u8]) -> Result<BatteryStatus, ProtocolError> {
    if report.len() < MIN_REPORT_LEN {
        return Err("Flydigi XInput battery response is too short".into());
    }
    if report[RESPONSE_PREFIX_OFFSET] != RESPONSE_PREFIX
        || report[COMMAND_OFFSET] != BATTERY_COMMAND
    {
        return Err("not a Flydigi XInput battery response".into());
    }

    let raw = report[BATTERY_OFFSET];
    let state_code = raw >> 4;
    let step = raw & 0x0f;
    let state = match state_code {
        0 => BatteryState::Discharging,
        1 => BatteryState::Charging,
        2 => BatteryState::Charged,
        other => return Err(format!("unknown Flydigi XInput charge state {other}").into()),
    };
    if step > 5 {
        return Err(format!("invalid Flydigi XInput battery step {step}").into());
    }

    if raw == 0 {
        return Ok(BatteryStatus {
            percentage: BatteryPercentage::Unavailable,
            level: BatteryLevel::Unknown,
            raw_level: step,
            state: BatteryState::Unknown,
            raw_status: Some(state_code),
            voltage_mv: None,
            source: SOURCE_FLYDIGI_XINPUT,
        });
    }

    let percent = step * 20;
    Ok(BatteryStatus {
        percentage: BatteryPercentage::Exact(percent),
        level: level_bucket(percent),
        raw_level: step,
        state,
        raw_status: Some(state_code),
        voltage_mv: None,
        source: SOURCE_FLYDIGI_XINPUT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(raw_battery: u8) -> Vec<u8> {
        let mut report = vec![0; MIN_REPORT_LEN];
        report[RESPONSE_PREFIX_OFFSET] = RESPONSE_PREFIX;
        report[COMMAND_OFFSET] = BATTERY_COMMAND;
        report[16] = 1;
        report[BATTERY_OFFSET] = raw_battery;
        report
    }

    #[test]
    fn decodes_twenty_percent_steps_and_charge_states() {
        let discharging = decode_flydigi_xinput_battery(&response(0x02)).unwrap();
        assert_eq!(discharging.percentage.exact(), Some(40));
        assert_eq!(discharging.level, BatteryLevel::Good);
        assert_eq!(discharging.raw_level, 2);
        assert_eq!(discharging.state, BatteryState::Discharging);
        assert_eq!(discharging.raw_status, Some(0));
        assert_eq!(discharging.source, SOURCE_FLYDIGI_XINPUT);

        assert_eq!(
            decode_flydigi_xinput_battery(&response(0x13))
                .unwrap()
                .state,
            BatteryState::Charging
        );
        let charged = decode_flydigi_xinput_battery(&response(0x25)).unwrap();
        assert_eq!(charged.percentage.exact(), Some(100));
        assert_eq!(charged.state, BatteryState::Charged);
    }

    #[test]
    fn zero_battery_byte_is_not_ready_not_empty() {
        let battery = decode_flydigi_xinput_battery(&response(0)).unwrap();
        assert_eq!(battery.percentage, BatteryPercentage::Unavailable);
        assert_eq!(battery.level, BatteryLevel::Unknown);
        assert_eq!(battery.state, BatteryState::Unknown);
        assert_eq!(battery.raw_level, 0);
    }

    #[test]
    fn rejects_short_wrong_command_unknown_state_and_invalid_step() {
        assert!(decode_flydigi_xinput_battery(&response(0x02)[..23]).is_err());

        let mut wrong_command = response(0x02);
        wrong_command[COMMAND_OFFSET] = 0;
        assert!(decode_flydigi_xinput_battery(&wrong_command).is_err());

        let mut wrong_prefix = response(0x02);
        wrong_prefix[RESPONSE_PREFIX_OFFSET] = 0;
        assert!(decode_flydigi_xinput_battery(&wrong_prefix).is_err());

        assert!(decode_flydigi_xinput_battery(&response(0x30)).is_err());
        assert!(decode_flydigi_xinput_battery(&response(0x06)).is_err());
    }
}
