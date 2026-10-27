use super::*;
use super::{battery_eta::battery_eta_label, poll::PollResult};
use crate::tray::DeviceMonitorEntry;
pub(super) fn apply_poll_result(
    tray: &mut TrayController,
    readings: Vec<DeviceReading>,
    result: PollResult,
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    if let Err(error) = result {
        if readings.is_empty() {
            record_event(
                diagnostic_log,
                file_logger,
                started,
                LogLevel::Warn,
                &format!("Device polling error: {error}"),
            )?;
            let mut status = format!("Error: {error}");
            append_config_error(&mut status, notification_config_error);
            if let Err(tray_error) = tray.update_status("", &status, None) {
                record_event(
                    diagnostic_log,
                    file_logger,
                    started,
                    LogLevel::Error,
                    &format!("Tray update error: {tray_error}"),
                )?;
            }
            return Ok(());
        }
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Warn,
            &format!("Device polling partially failed: {error}"),
        )?;
    }

    update_tray_devices(
        tray,
        &readings,
        notification_config_error,
        diagnostic_log,
        file_logger,
        started,
    )
}
pub(super) fn update_tray_devices(
    tray: &mut TrayController,
    readings: &[DeviceReading],
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let mut device_statuses = Vec::with_capacity(readings.len());
    let mut lowest_percentage: Option<u8> = None;
    for reading in readings {
        let device_name = format_device_name(&reading.name, reading.device_index);
        let percentage = reading.battery.level_percent;
        lowest_percentage = match (lowest_percentage, percentage) {
            (Some(current), Some(value)) => Some(current.min(value)),
            (None, Some(value)) => Some(value),
            (current, None) => current,
        };
        let mut status = battery_status_label(&reading.battery);
        if let Some(eta_label) = battery_eta_label(reading) {
            status.push_str("; ");
            status.push_str(&eta_label);
        }
        if let Some(error) = &reading.notification_error {
            status.push_str("; notification error: ");
            status.push_str(error);
        }
        let percentage_label = percentage.map_or_else(
            || "percentage unavailable".to_owned(),
            |value| format!("{value}%"),
        );
        device_statuses.push(format!("{device_name}: {percentage_label} ({status})"));
    }
    let device_count = device_statuses.len();
    if let Some(error) = notification_config_error {
        device_statuses.push(format!("Notification configuration error: {error}"));
    }
    let mut summary = format!(
        "{device_count} device{} monitored",
        if device_count == 1 { "" } else { "s" }
    );
    append_config_error(&mut summary, notification_config_error);

    if let Err(error) = tray.update_devices(&summary, &device_statuses, lowest_percentage) {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Error,
            &format!("Tray update error: {error}"),
        )?;
    }
    Ok(())
}
pub(super) fn update_monitored_device_menu(
    tray: &mut TrayController,
    devices: &HashMap<DeviceIdentity, String>,
    monitored_devices: &BTreeMap<String, bool>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let entries: Vec<_> = devices
        .iter()
        .map(|(identity, name)| {
            let config_id = identity.config_id();
            DeviceMonitorEntry {
                monitored: is_device_monitored(monitored_devices, &config_id),
                identity: config_id,
                name: name.clone(),
            }
        })
        .collect();
    if let Err(error) = tray.update_monitored_devices(&entries) {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Error,
            &format!("Monitored-devices menu update error: {error}"),
        )?;
    }
    Ok(())
}
pub(super) fn update_visible_devices(
    tray: &mut TrayController,
    latest_readings: &HashMap<DeviceIdentity, DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let readings = monitored_readings(latest_readings, monitored_devices);
    update_tray_devices(
        tray,
        &readings,
        notification_config_error,
        diagnostic_log,
        file_logger,
        started,
    )
}
pub(super) fn monitored_readings(
    latest_readings: &HashMap<DeviceIdentity, DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
) -> Vec<DeviceReading> {
    let mut readings: Vec<_> = latest_readings
        .iter()
        .filter(|(identity, _)| is_device_monitored(monitored_devices, &identity.config_id()))
        .map(|(_, reading)| reading.clone())
        .collect();
    readings.sort_by_key(|reading| reading.identity.config_id());
    readings
}
pub(super) fn retain_monitored_readings(
    latest_readings: &mut HashMap<DeviceIdentity, DeviceReading>,
    pending_readings: &mut Vec<DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
) {
    latest_readings
        .retain(|identity, _| is_device_monitored(monitored_devices, &identity.config_id()));
    pending_readings
        .retain(|reading| is_device_monitored(monitored_devices, &reading.identity.config_id()));
}
pub(super) fn is_device_monitored(
    monitored_devices: &BTreeMap<String, bool>,
    identity: &str,
) -> bool {
    monitored_devices.get(identity).copied().unwrap_or(true)
}
#[cfg(test)]
mod tests {
    use super::super::DeviceIdentity;
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use volty_battery::{BatteryLevel, BatterySource, BatteryState, BatteryStatus};
    fn test_reading() -> DeviceReading {
        DeviceReading {
            identity: DeviceIdentity::Hid {
                interface_path: "test".to_owned(),
                device_index: 1,
            },
            name: "Test device".to_owned(),
            device_index: Some(1),
            battery: BatteryStatus {
                level_percent: Some(20),
                level: BatteryLevel::Low,
                raw_level: 2,
                state: BatteryState::Discharging,
                raw_status: Some(0),
                source: BatterySource::Unified,
            },
            estimated_time_remaining: None,
            notification_error: None,
        }
    }
    #[test]
    fn new_devices_are_monitored_by_default() {
        let hid = test_reading();
        let mut second_hid = test_reading();
        second_hid.identity = DeviceIdentity::Hid {
            interface_path: "second-interface".to_owned(),
            device_index: 2,
        };

        let latest_readings = HashMap::from([
            (hid.identity.clone(), hid),
            (second_hid.identity.clone(), second_hid),
        ]);
        let visible = monitored_readings(&latest_readings, &BTreeMap::new());

        assert_eq!(visible.len(), 2);
    }
    #[test]
    fn unmonitored_devices_are_excluded_from_visible_readings() {
        let included = test_reading();
        let mut excluded = test_reading();
        excluded.identity = DeviceIdentity::Hid {
            interface_path: "excluded-interface".to_owned(),
            device_index: 2,
        };
        excluded.name = "Excluded device".to_owned();

        let excluded_identity = excluded.identity.clone();
        let included_identity = included.identity.clone();
        let latest_readings = HashMap::from([
            (included_identity.clone(), included),
            (excluded_identity.clone(), excluded),
        ]);
        let monitored_devices = BTreeMap::from([(excluded_identity.config_id(), false)]);

        let visible = monitored_readings(&latest_readings, &monitored_devices);
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible[0].identity.config_id(),
            included_identity.config_id()
        );
    }
}
