use super::{append_config_error, record_event, DeviceId, DeviceReading, FileLogger, LogLevel};
use super::{battery_eta::battery_eta_label, poll::PollResult};
use crate::tray::{DeviceMonitorEntry, TrayController};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    time::{Duration, Instant},
};

const LAST_SEEN_HIDE_AGE: Duration = Duration::from_secs(60);

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_poll_result(
    tray: &mut TrayController,
    readings: &[DeviceReading],
    pinned_device: Option<&str>,
    result: PollResult,
    notification_config_error: Option<&str>,
    stale_after: Duration,
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
        readings,
        pinned_device,
        notification_config_error,
        stale_after,
        diagnostic_log,
        file_logger,
        started,
    )
}

/// The reading that drives the tray icon and whether a configured pin was
/// missing from the latest readings.
pub(super) struct TrayIconDriver<'a> {
    pub(super) reading: Option<&'a DeviceReading>,
    pub(super) pinned_device_unavailable: bool,
}

/// Chooses the reading behind the tray icon: the pinned device when it is
/// present, otherwise the lowest monitored percentage. A pin is never cleared,
/// so a temporarily absent device is reported instead of silently ignored.
pub(super) fn tray_icon_driver<'a>(
    readings: &'a [DeviceReading],
    pinned_device: Option<&str>,
) -> TrayIconDriver<'a> {
    if let Some(reading) = pinned_device.and_then(|identity| {
        readings
            .iter()
            .find(|reading| reading.id.as_str() == identity)
    }) {
        return TrayIconDriver {
            reading: Some(reading),
            pinned_device_unavailable: false,
        };
    }

    let lowest = readings
        .iter()
        .filter_map(|reading| {
            reading
                .battery
                .level_percent
                .map(|percentage| (percentage, reading))
        })
        .min_by_key(|(percentage, _)| *percentage)
        .map(|(_, reading)| reading);
    TrayIconDriver {
        reading: lowest,
        pinned_device_unavailable: pinned_device.is_some(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_tray_devices(
    tray: &mut TrayController,
    readings: &[DeviceReading],
    pinned_device: Option<&str>,
    notification_config_error: Option<&str>,
    stale_after: Duration,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let now = Instant::now();
    let driver = tray_icon_driver(readings, pinned_device);
    let icon_percentage = driver
        .reading
        .and_then(|reading| reading.battery.level_percent);
    let pinned_index = pinned_device.and_then(|identity| {
        readings
            .iter()
            .position(|reading| reading.id.as_str() == identity)
    });
    let (summary, device_statuses, tooltip_statuses) = tray_status_view(
        readings,
        pinned_index,
        driver.pinned_device_unavailable,
        notification_config_error,
        now,
        stale_after,
    );

    if let Err(error) = tray.update_devices(
        &summary,
        &device_statuses,
        &tooltip_statuses,
        icon_percentage,
    ) {
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

/// Builds the compact status line, the extra menu rows, and the tooltip lines.
///
/// One monitored device is already the status line, so it gets no duplicate
/// row. A pinned device is summarized as `Pinned …` and omitted from the rows;
/// the other devices stay listed when more than one is monitored.
fn tray_status_view(
    readings: &[DeviceReading],
    pinned_index: Option<usize>,
    pinned_device_unavailable: bool,
    notification_config_error: Option<&str>,
    now: Instant,
    stale_after: Duration,
) -> (String, Vec<String>, Vec<String>) {
    let compact_statuses: Vec<_> = readings
        .iter()
        .map(|reading| {
            let last_seen = last_seen_label(reading.observed_at, now, stale_after);
            compact_device_status(reading, last_seen.as_deref())
        })
        .collect();
    let device_count = compact_statuses.len();
    let mut summary = if let Some(index) = pinned_index {
        format!("Pinned {}", compact_statuses[index])
    } else if pinned_device_unavailable {
        format!(
            "Pinned device unavailable · {device_count} device{} monitored",
            if device_count == 1 { "" } else { "s" }
        )
    } else if device_count == 1 {
        compact_statuses[0].clone()
    } else {
        format!(
            "{device_count} device{} monitored",
            if device_count == 1 { "" } else { "s" }
        )
    };
    append_config_error(&mut summary, notification_config_error);

    let single_device_is_the_summary =
        device_count == 1 && pinned_index.is_none() && !pinned_device_unavailable;
    let device_statuses = if readings.is_empty() {
        vec!["No device status available".to_owned()]
    } else if single_device_is_the_summary {
        Vec::new()
    } else {
        compact_statuses
            .iter()
            .enumerate()
            .filter(|(index, _)| pinned_index != Some(*index))
            .map(|(_, status)| status.clone())
            .collect()
    };
    let tooltip_statuses =
        prioritized_tooltip_statuses(compact_statuses, pinned_index, pinned_device_unavailable);
    (summary, device_statuses, tooltip_statuses)
}

/// Puts the pinned device first in the tooltip and announces a missing pin.
fn prioritized_tooltip_statuses(
    mut statuses: Vec<String>,
    pinned_index: Option<usize>,
    pinned_device_unavailable: bool,
) -> Vec<String> {
    if pinned_device_unavailable {
        statuses.insert(0, "Pinned device unavailable".to_owned());
    } else if let Some(index) = pinned_index {
        if let Some(status) = statuses.get(index).cloned() {
            statuses.remove(index);
            statuses.insert(0, status);
        }
    }
    statuses
}

fn compact_device_status(reading: &DeviceReading, last_seen: Option<&str>) -> String {
    let name = reading.label.display();
    let mut status = match reading.battery.level_percent {
        Some(percentage) => match battery_eta_label(reading) {
            Some(eta) => format!("{name}: {percentage}% · {eta}"),
            None => format!("{name}: {percentage}%"),
        },
        None => format!("{name}: battery unknown"),
    };
    if reading.notification_error.is_some() {
        status.push_str(" · notification failed");
    }
    if let Some(last_seen) = last_seen {
        status.push_str(" · ");
        status.push_str(last_seen);
    }
    status
}

/// Relative age of the newest reading, hidden for the first minute so fresh
/// polls do not add noise. At or past `stale_after` a stale marker leads.
pub(super) fn last_seen_label(
    observed_at: Instant,
    now: Instant,
    stale_after: Duration,
) -> Option<String> {
    let age = now.saturating_duration_since(observed_at);
    if age < LAST_SEEN_HIDE_AGE {
        return None;
    }
    let age_label = compact_age_label(age);
    if age >= stale_after {
        Some(format!("stale, last seen {age_label} ago"))
    } else {
        Some(format!("last seen {age_label} ago"))
    }
}

fn compact_age_label(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    if minutes < 24 * 60 {
        let (hours, remainder) = (minutes / 60, minutes % 60);
        return if remainder == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {remainder}m")
        };
    }
    let (days, hours) = (minutes / (24 * 60), (minutes % (24 * 60)) / 60);
    if hours == 0 {
        format!("{days}d")
    } else {
        format!("{days}d {hours}h")
    }
}

pub(super) fn update_monitored_device_menu(
    tray: &mut TrayController,
    devices: &HashMap<DeviceId, String>,
    monitored_devices: &BTreeMap<String, bool>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let entries: Vec<_> = devices
        .iter()
        .map(|(id, name)| DeviceMonitorEntry {
            monitored: is_device_monitored(monitored_devices, id.as_str()),
            identity: id.as_str().to_owned(),
            name: name.clone(),
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
#[allow(clippy::too_many_arguments)]
pub(super) fn update_visible_devices(
    tray: &mut TrayController,
    latest_readings: &HashMap<DeviceId, DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
    pinned_device: Option<&str>,
    notification_config_error: Option<&str>,
    stale_after: Duration,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let readings = monitored_readings(latest_readings, monitored_devices);
    update_tray_devices(
        tray,
        &readings,
        pinned_device,
        notification_config_error,
        stale_after,
        diagnostic_log,
        file_logger,
        started,
    )
}
pub(super) fn monitored_readings(
    latest_readings: &HashMap<DeviceId, DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
) -> Vec<DeviceReading> {
    let mut readings: Vec<_> = latest_readings
        .iter()
        .filter(|(id, _)| is_device_monitored(monitored_devices, id.as_str()))
        .map(|(_, reading)| reading.clone())
        .collect();
    readings.sort_by(|left, right| left.id.cmp(&right.id));
    readings
}
pub(super) fn retain_monitored_readings(
    latest_readings: &mut HashMap<DeviceId, DeviceReading>,
    pending_readings: &mut Vec<DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
) {
    latest_readings.retain(|id, _| is_device_monitored(monitored_devices, id.as_str()));
    pending_readings.retain(|reading| is_device_monitored(monitored_devices, reading.id.as_str()));
}
pub(super) fn is_device_monitored(
    monitored_devices: &BTreeMap<String, bool>,
    identity: &str,
) -> bool {
    monitored_devices.get(identity).copied().unwrap_or(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::time::Duration;
    use volty_battery::{BatteryLevel, BatteryState, BatteryStatus, DeviceLabel};
    fn test_reading() -> DeviceReading {
        DeviceReading {
            id: DeviceId::new("hid:test#1"),
            label: DeviceLabel::new("Test device").with_qualifier("slot 1"),
            battery: BatteryStatus {
                level_percent: Some(20),
                level: BatteryLevel::Low,
                raw_level: 2,
                state: BatteryState::Discharging,
                raw_status: Some(0),
                voltage_mv: None,
                source: "test",
            },
            estimated_time_remaining: None,
            notification_error: None,
            observed_at: Instant::now(),
        }
    }
    #[test]
    fn new_devices_are_monitored_by_default() {
        let hid = test_reading();
        let mut second_hid = test_reading();
        second_hid.id = DeviceId::new("hid:second-interface#2");

        let latest_readings =
            HashMap::from([(hid.id.clone(), hid), (second_hid.id.clone(), second_hid)]);
        let visible = monitored_readings(&latest_readings, &BTreeMap::new());

        assert_eq!(visible.len(), 2);
    }
    #[test]
    fn unmonitored_devices_are_excluded_from_visible_readings() {
        let included = test_reading();
        let mut excluded = test_reading();
        excluded.id = DeviceId::new("hid:excluded-interface#2");
        excluded.label = DeviceLabel::new("Excluded device");

        let excluded_id = excluded.id.clone();
        let included_id = included.id.clone();
        let latest_readings = HashMap::from([
            (included_id.clone(), included),
            (excluded_id.clone(), excluded),
        ]);
        let monitored_devices = BTreeMap::from([(excluded_id.as_str().to_owned(), false)]);

        let visible = monitored_readings(&latest_readings, &monitored_devices);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, included_id);
    }

    #[test]
    fn compact_device_status_keeps_percentage_and_eta_on_one_line() {
        let mut reading = test_reading();
        reading.estimated_time_remaining = Some(Duration::from_secs(90 * 60));

        assert_eq!(
            compact_device_status(&reading, None),
            "Test device (slot 1): 20% · ETA 1h 30m"
        );
    }

    #[test]
    fn compact_device_status_omits_charge_state_without_an_eta() {
        let mut reading = test_reading();
        reading.battery.level_percent = Some(87);
        reading.battery.level = BatteryLevel::Good;
        reading.battery.state = BatteryState::Charging;

        assert_eq!(
            compact_device_status(&reading, None),
            "Test device (slot 1): 87%"
        );
    }

    #[test]
    fn compact_device_status_reports_unknown_percentages() {
        let mut reading = test_reading();
        reading.battery.level_percent = None;
        reading.battery.state = BatteryState::Charged;

        assert_eq!(
            compact_device_status(&reading, None),
            "Test device (slot 1): battery unknown"
        );
    }

    #[test]
    fn compact_device_status_omits_the_eta_when_full() {
        let mut reading = test_reading();
        reading.battery.level_percent = Some(100);
        reading.battery.level = BatteryLevel::Unknown;
        reading.battery.state = BatteryState::Unknown;

        assert_eq!(
            compact_device_status(&reading, None),
            "Test device (slot 1): 100%"
        );
    }

    #[test]
    fn compact_device_status_marks_failed_notifications() {
        let mut reading = test_reading();
        reading.notification_error = Some("toast delivery failed".to_owned());

        assert_eq!(
            compact_device_status(&reading, None),
            "Test device (slot 1): 20% · ETA calculating · notification failed"
        );
    }

    #[test]
    fn compact_device_status_appends_last_seen_text() {
        let mut reading = test_reading();
        reading.estimated_time_remaining = Some(Duration::from_secs(90 * 60));

        assert_eq!(
            compact_device_status(&reading, Some("stale, last seen 18m ago")),
            "Test device (slot 1): 20% · ETA 1h 30m · stale, last seen 18m ago"
        );
    }

    #[test]
    fn last_seen_label_is_hidden_for_the_first_minute() {
        let now = Instant::now();
        let stale_after = Duration::from_secs(15 * 60);

        assert_eq!(last_seen_label(now, now, stale_after), None);
        assert_eq!(
            last_seen_label(now - Duration::from_secs(59), now, stale_after),
            None
        );
    }

    #[test]
    fn last_seen_label_formats_minutes_hours_and_days() {
        let now = Instant::now();
        let stale_after = Duration::from_secs(30 * 24 * 60 * 60);

        assert_eq!(
            last_seen_label(now - Duration::from_secs(5 * 60), now, stale_after),
            Some("last seen 5m ago".to_owned())
        );
        assert_eq!(
            last_seen_label(now - Duration::from_secs(65 * 60), now, stale_after),
            Some("last seen 1h 5m ago".to_owned())
        );
        assert_eq!(
            last_seen_label(
                now - Duration::from_secs((2 * 24 + 3) * 60 * 60),
                now,
                stale_after
            ),
            Some("last seen 2d 3h ago".to_owned())
        );
    }

    #[test]
    fn last_seen_label_marks_readings_stale_at_the_threshold() {
        let now = Instant::now();
        let stale_after = Duration::from_secs(15 * 60);

        assert_eq!(
            last_seen_label(
                now - (stale_after - Duration::from_secs(1)),
                now,
                stale_after
            ),
            Some("last seen 14m ago".to_owned())
        );
        assert_eq!(
            last_seen_label(now - stale_after, now, stale_after),
            Some("stale, last seen 15m ago".to_owned())
        );
        assert_eq!(
            last_seen_label(now - Duration::from_secs(18 * 60), now, stale_after),
            Some("stale, last seen 18m ago".to_owned())
        );
    }

    fn reading_with(id: &str, percentage: Option<u8>) -> DeviceReading {
        let mut reading = test_reading();
        reading.id = DeviceId::new(id);
        reading.battery.level_percent = percentage;
        reading
    }

    #[test]
    fn pinned_present_device_drives_the_icon_even_when_another_battery_is_lower() {
        let readings = vec![
            reading_with("hid:a#1", Some(80)),
            reading_with("hid:b#1", Some(20)),
        ];

        let driver = tray_icon_driver(&readings, Some("hid:a#1"));

        assert_eq!(
            driver.reading.map(|reading| reading.id.as_str()),
            Some("hid:a#1")
        );
        assert!(!driver.pinned_device_unavailable);
    }

    #[test]
    fn missing_pinned_device_falls_back_to_the_lowest_monitored_percentage() {
        let readings = vec![
            reading_with("hid:a#1", Some(80)),
            reading_with("hid:b#1", Some(20)),
        ];

        let driver = tray_icon_driver(&readings, Some("hid:gone#1"));

        assert_eq!(
            driver.reading.map(|reading| reading.id.as_str()),
            Some("hid:b#1")
        );
        assert!(driver.pinned_device_unavailable);
    }

    #[test]
    fn no_pin_uses_the_lowest_of_multiple_devices() {
        let readings = vec![
            reading_with("hid:a#1", Some(55)),
            reading_with("hid:b#1", Some(15)),
            reading_with("hid:c#1", Some(40)),
        ];

        let driver = tray_icon_driver(&readings, None);

        assert_eq!(
            driver.reading.map(|reading| reading.id.as_str()),
            Some("hid:b#1")
        );
        assert!(!driver.pinned_device_unavailable);
    }

    #[test]
    fn unmonitored_pinned_device_is_reported_unavailable() {
        let readings = vec![reading_with("hid:monitored#1", Some(70))];

        let driver = tray_icon_driver(&readings, Some("hid:unmonitored#1"));

        assert_eq!(
            driver.reading.map(|reading| reading.id.as_str()),
            Some("hid:monitored#1")
        );
        assert!(driver.pinned_device_unavailable);
    }

    #[test]
    fn pinned_device_without_a_percentage_still_drives_the_icon() {
        let readings = vec![
            reading_with("hid:a#1", None),
            reading_with("hid:b#1", Some(50)),
        ];

        let driver = tray_icon_driver(&readings, Some("hid:a#1"));

        assert_eq!(
            driver.reading.map(|reading| reading.id.as_str()),
            Some("hid:a#1")
        );
        assert_eq!(
            driver
                .reading
                .and_then(|reading| reading.battery.level_percent),
            None
        );
    }

    fn named_reading(id: &str, name: &str, percentage: u8, eta_minutes: u64) -> DeviceReading {
        let mut reading = reading_with(id, Some(percentage));
        reading.label = DeviceLabel::new(name);
        reading.estimated_time_remaining = Some(Duration::from_secs(eta_minutes * 60));
        reading
    }

    fn status_view(
        readings: &[DeviceReading],
        pinned_index: Option<usize>,
    ) -> (String, Vec<String>, Vec<String>) {
        tray_status_view(
            readings,
            pinned_index,
            false,
            None,
            Instant::now(),
            Duration::from_secs(15 * 60),
        )
    }

    #[test]
    fn one_monitored_device_uses_a_single_compact_status_line() {
        let readings = vec![named_reading("hid:a#1", "Mouse A", 47, 90)];

        let (summary, rows, tooltip) = status_view(&readings, None);

        assert_eq!(summary, "Mouse A: 47% · ETA 1h 30m");
        assert!(rows.is_empty());
        assert_eq!(tooltip, vec!["Mouse A: 47% · ETA 1h 30m"]);
    }

    #[test]
    fn no_devices_keep_the_placeholder_row() {
        let (summary, rows, tooltip) = status_view(&[], None);

        assert_eq!(summary, "0 devices monitored");
        assert_eq!(rows, vec!["No device status available".to_owned()]);
        assert!(tooltip.is_empty());
    }

    #[test]
    fn pinned_device_status_is_not_repeated_as_a_row() {
        let readings = vec![named_reading("hid:a#1", "Mouse A", 47, 90)];

        let (summary, rows, tooltip) = status_view(&readings, Some(0));

        assert_eq!(summary, "Pinned Mouse A: 47% · ETA 1h 30m");
        assert!(rows.is_empty());
        assert_eq!(tooltip, vec!["Mouse A: 47% · ETA 1h 30m"]);
    }

    #[test]
    fn multiple_devices_keep_the_summary_and_list_each_device() {
        let readings = vec![
            named_reading("hid:a#1", "Mouse A", 80, 120),
            named_reading("hid:b#1", "Mouse B", 20, 90),
        ];

        let (summary, rows, _) = status_view(&readings, None);

        assert_eq!(summary, "2 devices monitored");
        assert_eq!(
            rows,
            vec![
                "Mouse A: 80% · ETA 2h".to_owned(),
                "Mouse B: 20% · ETA 1h 30m".to_owned(),
            ]
        );
    }

    #[test]
    fn a_pinned_device_is_omitted_from_the_other_device_rows() {
        let readings = vec![
            named_reading("hid:a#1", "Mouse A", 80, 120),
            named_reading("hid:b#1", "Mouse B", 20, 90),
        ];

        let (summary, rows, tooltip) = status_view(&readings, Some(1));

        assert_eq!(summary, "Pinned Mouse B: 20% · ETA 1h 30m");
        assert_eq!(rows, vec!["Mouse A: 80% · ETA 2h".to_owned()]);
        assert_eq!(tooltip[0], "Mouse B: 20% · ETA 1h 30m");
    }

    #[test]
    fn an_unavailable_pin_keeps_the_device_rows_visible() {
        let readings = vec![named_reading("hid:a#1", "Mouse A", 47, 90)];
        let now = Instant::now();
        let (summary, rows, tooltip) = tray_status_view(
            &readings,
            None,
            true,
            None,
            now,
            Duration::from_secs(15 * 60),
        );

        assert_eq!(summary, "Pinned device unavailable · 1 device monitored");
        assert_eq!(rows, vec!["Mouse A: 47% · ETA 1h 30m".to_owned()]);
        assert_eq!(tooltip[0], "Pinned device unavailable");
    }

    #[test]
    fn notification_config_errors_stay_in_the_summary() {
        let readings = vec![named_reading("hid:a#1", "Mouse A", 47, 90)];
        let now = Instant::now();
        let (summary, rows, _) = tray_status_view(
            &readings,
            None,
            false,
            Some("invalid notification settings"),
            now,
            Duration::from_secs(15 * 60),
        );

        assert_eq!(
            summary,
            "Mouse A: 47% · ETA 1h 30m; notification config error: invalid notification settings"
        );
        assert!(rows.is_empty());
    }

    #[test]
    fn tooltip_lists_the_pinned_device_first_and_announces_a_missing_pin() {
        let statuses = vec![
            "Mouse A: 80%".to_owned(),
            "Mouse B: 20%".to_owned(),
            "Mouse C: 40%".to_owned(),
        ];

        assert_eq!(
            prioritized_tooltip_statuses(statuses.clone(), Some(1), false),
            vec![
                "Mouse B: 20%".to_owned(),
                "Mouse A: 80%".to_owned(),
                "Mouse C: 40%".to_owned(),
            ]
        );
        assert_eq!(
            prioritized_tooltip_statuses(statuses, None, true),
            vec![
                "Pinned device unavailable".to_owned(),
                "Mouse A: 80%".to_owned(),
                "Mouse B: 20%".to_owned(),
                "Mouse C: 40%".to_owned(),
            ]
        );
    }
}
