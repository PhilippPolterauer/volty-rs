use std::io;
use volty_battery::{
    BatteryLevel, BatteryPercentage, BatteryPercentageEstimateMethod, BatteryProvider,
    BatteryState, BatteryStatus, DeviceId, DeviceLabel, ProviderEvent, ProviderPoll, ScanMode,
    SinkClosed, SourceReading,
};
use windows_sys::Win32::{
    Foundation::{ERROR_DEVICE_NOT_CONNECTED, ERROR_SUCCESS},
    UI::Input::XboxController::{
        XInputGetBatteryInformation, XInputGetState, BATTERY_DEVTYPE_GAMEPAD, BATTERY_LEVEL_EMPTY,
        BATTERY_LEVEL_FULL, BATTERY_LEVEL_LOW, BATTERY_LEVEL_MEDIUM, BATTERY_TYPE_ALKALINE,
        BATTERY_TYPE_NIMH, XINPUT_BATTERY_INFORMATION, XINPUT_STATE,
    },
};

const XINPUT_USER_COUNT: u32 = 4;
const SOURCE_XINPUT: &str = "XInput coarse battery estimate";

/// XInput reports qualitative buckets rather than percentages. These values
/// are explicitly estimates, not values reported by the controller.
fn estimate_percentage(raw_level: u8) -> BatteryPercentage {
    let value = match raw_level {
        BATTERY_LEVEL_EMPTY => 0,
        BATTERY_LEVEL_LOW => 20,
        BATTERY_LEVEL_MEDIUM => 50,
        BATTERY_LEVEL_FULL => 90,
        _ => return BatteryPercentage::Unavailable,
    };
    BatteryPercentage::Estimated {
        value,
        method: BatteryPercentageEstimateMethod::XInputBucketMapping,
    }
}

#[derive(Debug, Default)]
pub(super) struct XInputProvider;

impl BatteryProvider for XInputProvider {
    fn poll(
        &mut self,
        _mode: ScanMode,
        sink: &mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>,
    ) -> Result<ProviderPoll, SinkClosed> {
        let mut outcome = ProviderPoll::default();
        for user_index in 0..XINPUT_USER_COUNT {
            let mut state = XINPUT_STATE::default();
            // SAFETY: the user index is in XInput's supported range and `state` is writable.
            let state_result = unsafe { XInputGetState(user_index, &mut state) };
            if state_result == ERROR_DEVICE_NOT_CONNECTED {
                continue;
            }
            if state_result != ERROR_SUCCESS {
                outcome
                    .errors
                    .push(api_error("XInputGetState", user_index, state_result));
                continue;
            }
            outcome.recognised += 1;

            let mut information = XINPUT_BATTERY_INFORMATION::default();
            // SAFETY: `user_index` is connected and `information` is writable.
            let battery_result = unsafe {
                XInputGetBatteryInformation(user_index, BATTERY_DEVTYPE_GAMEPAD, &mut information)
            };
            if battery_result == ERROR_DEVICE_NOT_CONNECTED {
                continue;
            }
            if battery_result != ERROR_SUCCESS {
                outcome.errors.push(api_error(
                    "XInputGetBatteryInformation",
                    user_index,
                    battery_result,
                ));
                continue;
            }

            match decode_battery(information.BatteryType, information.BatteryLevel) {
                Ok(Some(battery)) => {
                    sink(ProviderEvent::Reading(source_reading(user_index, battery)))?;
                    outcome.readings += 1;
                }
                Ok(None) => {}
                Err(error) => outcome.errors.push(format!(
                    "XInput battery for player {}: {error}",
                    user_index + 1
                )),
            }
        }
        Ok(outcome)
    }
}

fn decode_battery(battery_type: u8, raw_level: u8) -> Result<Option<BatteryStatus>, String> {
    if !matches!(battery_type, BATTERY_TYPE_ALKALINE | BATTERY_TYPE_NIMH) {
        return Ok(None);
    }
    let level = match raw_level {
        BATTERY_LEVEL_EMPTY => BatteryLevel::Critical,
        BATTERY_LEVEL_LOW => BatteryLevel::Low,
        BATTERY_LEVEL_MEDIUM => BatteryLevel::Good,
        BATTERY_LEVEL_FULL => BatteryLevel::Full,
        value => return Err(format!("unknown battery level {value}")),
    };
    Ok(Some(BatteryStatus {
        percentage: estimate_percentage(raw_level),
        level,
        raw_level,
        state: BatteryState::Unknown,
        raw_status: None,
        voltage_mv: None,
        source: SOURCE_XINPUT,
    }))
}

fn source_reading(user_index: u32, battery: BatteryStatus) -> SourceReading {
    SourceReading {
        id: DeviceId::new(format!("xinput:{user_index}")),
        label: DeviceLabel::new("Xbox controller")
            .with_qualifier(format!("player {}", user_index + 1)),
        battery,
    }
}

fn api_error(function: &str, user_index: u32, error_code: u32) -> String {
    format!(
        "{function} for player {}: {}",
        user_index + 1,
        io::Error::from_raw_os_error(error_code as i32)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::XboxController::{
        BATTERY_TYPE_DISCONNECTED, BATTERY_TYPE_UNKNOWN, BATTERY_TYPE_WIRED,
    };

    #[test]
    fn maps_documented_xinput_battery_buckets_to_explicit_estimates() {
        for (raw_level, expected_level, expected_percent) in [
            (BATTERY_LEVEL_EMPTY, BatteryLevel::Critical, 0),
            (BATTERY_LEVEL_LOW, BatteryLevel::Low, 20),
            (BATTERY_LEVEL_MEDIUM, BatteryLevel::Good, 50),
            (BATTERY_LEVEL_FULL, BatteryLevel::Full, 90),
        ] {
            let battery = decode_battery(BATTERY_TYPE_ALKALINE, raw_level)
                .unwrap()
                .unwrap();
            assert_eq!(battery.level, expected_level);
            assert_eq!(
                battery.percentage,
                BatteryPercentage::Estimated {
                    value: expected_percent,
                    method: BatteryPercentageEstimateMethod::XInputBucketMapping,
                }
            );
            assert_eq!(battery.raw_level, raw_level);
            assert_eq!(battery.state, BatteryState::Unknown);
            assert_eq!(battery.raw_status, None);
            assert_eq!(battery.source, SOURCE_XINPUT);
        }
    }

    #[test]
    fn accepts_alkaline_and_rechargeable_nimh_batteries() {
        assert!(decode_battery(BATTERY_TYPE_ALKALINE, BATTERY_LEVEL_FULL)
            .unwrap()
            .is_some());
        assert!(decode_battery(BATTERY_TYPE_NIMH, BATTERY_LEVEL_FULL)
            .unwrap()
            .is_some());
    }

    #[test]
    fn ignores_wired_disconnected_and_unknown_battery_types() {
        for battery_type in [
            BATTERY_TYPE_DISCONNECTED,
            BATTERY_TYPE_WIRED,
            BATTERY_TYPE_UNKNOWN,
        ] {
            assert_eq!(
                decode_battery(battery_type, BATTERY_LEVEL_FULL).unwrap(),
                None
            );
        }
    }

    #[test]
    fn rejects_levels_outside_the_xinput_enum() {
        assert_eq!(
            decode_battery(BATTERY_TYPE_ALKALINE, 4),
            Err("unknown battery level 4".to_owned())
        );
    }

    #[test]
    fn reading_identity_and_label_are_slot_specific() {
        let reading = source_reading(
            0,
            decode_battery(BATTERY_TYPE_ALKALINE, BATTERY_LEVEL_FULL)
                .unwrap()
                .unwrap(),
        );
        assert_eq!(reading.id.as_str(), "xinput:0");
        assert_eq!(reading.label.display(), "Xbox controller (player 1)");
    }

    #[test]
    #[ignore = "requires a connected Xbox controller with batteries"]
    fn reads_a_live_gamepad_battery() {
        let mut provider = XInputProvider;
        let mut readings = Vec::new();
        let outcome = provider
            .poll(ScanMode::Discover, &mut |event| {
                if let ProviderEvent::Reading(reading) = event {
                    readings.push(reading);
                }
                Ok(())
            })
            .unwrap();

        assert!(outcome.recognised > 0, "no connected XInput gamepad found");
        assert!(!readings.is_empty(), "no XInput battery value was reported");
        for reading in readings {
            println!(
                "{}: {:?} (raw level {})",
                reading.label.display(),
                reading.battery.level,
                reading.battery.raw_level
            );
        }
    }
}
