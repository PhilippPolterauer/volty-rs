//! Capture command dispatch and shared conversion to the capture schema.

mod live;

#[cfg(windows)]
pub use live::write_default_snapshot;
pub(crate) use live::{unix_now, unix_to_rfc3339};

use std::{error::Error, io, path::PathBuf};

use volty_battery::{
    BatteryLevel, BatteryPercentage, BatteryPercentageEstimateMethod, BatteryState, BatteryStatus,
};
use volty_capture::{BatterySnapshot, CapturedBatteryPercentage, CapturedPercentageEstimateMethod};

const CAPTURE_USAGE: &str = "Usage:\n  volty capture [--output <path>] [--strict]\n  volty capture inspect <capture.volty.jsonl>\n  volty capture replay <capture.volty.jsonl>\n\nWrites a read-only snapshot of the visible HID interfaces, or validates and\nreplays a previously written capture without opening a device. Nothing is\nuploaded automatically.";

pub fn run_cli(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args.first().map(String::as_str) {
        Some("inspect") => crate::replay::inspect_cli(&subcommand_path("inspect", &args[1..])?),
        Some("replay") => crate::replay::replay_cli(&subcommand_path("replay", &args[1..])?),
        Some("-h" | "--help" | "help") => {
            println!("{CAPTURE_USAGE}");
            Ok(())
        }
        _ => live::run_cli(args),
    }
}

fn subcommand_path(command: &str, args: &[String]) -> Result<PathBuf, Box<dyn Error>> {
    match args {
        [path] => Ok(PathBuf::from(path)),
        [] => Err(usage_error(&format!("{command} requires a capture path"))),
        _ => Err(usage_error(&format!(
            "{command} accepts exactly one capture path"
        ))),
    }
}

fn usage_error(message: &str) -> Box<dyn Error> {
    io::Error::other(format!("{message}\n{CAPTURE_USAGE}")).into()
}

pub(crate) fn battery_snapshot(battery: &BatteryStatus) -> BatterySnapshot {
    BatterySnapshot {
        percentage: match battery.percentage {
            BatteryPercentage::Unavailable => CapturedBatteryPercentage::Unavailable,
            BatteryPercentage::Exact(value) => CapturedBatteryPercentage::Exact { value },
            BatteryPercentage::Estimated { value, method } => {
                CapturedBatteryPercentage::Estimated {
                    value,
                    method: match method {
                        BatteryPercentageEstimateMethod::VoltageCurve => {
                            CapturedPercentageEstimateMethod::VoltageCurve
                        }
                        BatteryPercentageEstimateMethod::XInputBucketMapping => {
                            CapturedPercentageEstimateMethod::XInputBucketMapping
                        }
                    },
                }
            }
        },
        level: level_name(battery.level).to_owned(),
        state: state_name(battery.state).to_owned(),
        raw_level: battery.raw_level,
        raw_status: battery.raw_status,
        source: battery.source.to_owned(),
        voltage_mv: battery.voltage_mv,
    }
}

const fn level_name(level: BatteryLevel) -> &'static str {
    match level {
        BatteryLevel::Unknown => "unknown",
        BatteryLevel::Critical => "critical",
        BatteryLevel::Low => "low",
        BatteryLevel::Good => "good",
        BatteryLevel::Full => "full",
    }
}

const fn state_name(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Unknown => "unknown",
        BatteryState::Discharging => "discharging",
        BatteryState::Charging => "charging",
        BatteryState::ChargingNearlyFull => "charging_nearly_full",
        BatteryState::Charged => "charged",
        BatteryState::ChargingSlow => "charging_slow",
        BatteryState::InvalidBattery => "invalid_battery",
        BatteryState::ThermalError => "thermal_error",
        BatteryState::ChargingError => "charging_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_state_and_level_names_match_the_schema() {
        let battery = BatteryStatus {
            percentage: BatteryPercentage::Exact(73),
            level: BatteryLevel::Good,
            raw_level: 73,
            state: BatteryState::ChargingNearlyFull,
            raw_status: Some(4),
            voltage_mv: None,
            source: "HID++ Unified Battery",
        };
        let snapshot = battery_snapshot(&battery);
        assert_eq!(
            snapshot.percentage,
            CapturedBatteryPercentage::Exact { value: 73 }
        );
        assert_eq!(snapshot.level, "good");
        assert_eq!(snapshot.state, "charging_nearly_full");
        assert_eq!(snapshot.raw_level, 73);
        assert_eq!(snapshot.raw_status, Some(4));
        assert_eq!(snapshot.source, "HID++ Unified Battery");
    }
}
