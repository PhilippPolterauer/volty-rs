use super::{
    battery_is_full, battery_source_label, battery_status_label, format_device_name,
    is_device_monitored, record_event, AppConfig, DeviceIdentity, DeviceReading, DeviceState,
    FileLogger, LogLevel,
};
use crate::notify::{BatterySample, NotificationEngine, NotificationOutcome};
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    io,
    time::Instant,
};
use volty_battery::BatteryState;
pub(super) struct DeviceNotifications {
    config: AppConfig,
    engines: HashMap<DeviceIdentity, NotificationEngine>,
    persisted_alerts: HashSet<String>,
    pending_alerts: HashMap<String, bool>,
}
impl DeviceNotifications {
    pub(super) fn from_config(config: AppConfig) -> Result<Self, Box<dyn Error>> {
        NotificationEngine::from_config(config)?;
        Ok(Self {
            config,
            engines: HashMap::new(),
            persisted_alerts: HashSet::new(),
            pending_alerts: HashMap::new(),
        })
    }

    pub(super) fn restore_persisted_alerts(&mut self, alerts: HashSet<String>) {
        self.persisted_alerts = alerts;
    }

    pub(super) fn set_config(&mut self, config: AppConfig) -> Result<(), Box<dyn Error>> {
        NotificationEngine::from_config(config)?;
        for engine in self.engines.values_mut() {
            engine.set_config(config)?;
        }
        self.config = config;
        Ok(())
    }

    pub(super) fn retain_monitored_devices(&mut self, monitored_devices: &BTreeMap<String, bool>) {
        self.engines
            .retain(|identity, _| is_device_monitored(monitored_devices, &identity.config_id()));
        let stale_alerts: HashSet<_> = self
            .persisted_alerts
            .iter()
            .chain(self.pending_alerts.keys())
            .filter(|identity| !is_device_monitored(monitored_devices, identity))
            .cloned()
            .collect();
        for identity in stale_alerts {
            self.persisted_alerts.remove(&identity);
            self.pending_alerts.insert(identity, false);
        }
    }

    pub(super) fn update(
        &mut self,
        identity: DeviceIdentity,
        device_name: &str,
        sample: BatterySample,
    ) -> Result<NotificationOutcome, Box<dyn Error>> {
        let identity_key = identity.config_id();
        let previously_notified = self.persisted_alerts.contains(&identity_key);
        let engine = match self.engines.entry(identity) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let mut engine = NotificationEngine::from_config(self.config)?;
                engine.restore_discharge_notification(previously_notified);
                entry.insert(engine)
            }
        };
        let outcome = engine.update(device_name, sample)?;
        match outcome {
            NotificationOutcome::Notified => {
                self.persisted_alerts.insert(identity_key.clone());
                self.pending_alerts.insert(identity_key, true);
            }
            NotificationOutcome::CycleReset => {
                self.persisted_alerts.remove(&identity_key);
                self.pending_alerts.insert(identity_key, false);
            }
            _ => {}
        }
        Ok(outcome)
    }

    pub(super) fn persist_pending_alerts(
        &mut self,
        state: Option<&mut DeviceState>,
    ) -> Result<(), Box<dyn Error>> {
        let Some(state) = state else {
            self.pending_alerts.clear();
            return Ok(());
        };
        for (identity, sent) in &self.pending_alerts {
            state.set_discharge_notification_sent(identity, *sent)?;
        }
        self.pending_alerts.clear();
        Ok(())
    }
}
pub(super) fn process_device_reading(
    notifications: &mut DeviceNotifications,
    state: Option<&mut DeviceState>,
    reading: &mut DeviceReading,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let device_name = format_device_name(&reading.name, reading.device_index);
    let percentage = reading.battery.level_percent;
    let status = battery_status_label(&reading.battery);
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Debug,
        &format!(
            "Battery read for {device_name} via {}: {} ({status})",
            battery_source_label(reading.battery.source),
            percentage.map_or_else(|| "unknown".to_owned(), |value| format!("{value}%"))
        ),
    )?;

    let charging = matches!(
        reading.battery.state,
        BatteryState::Charging | BatteryState::ChargingNearlyFull | BatteryState::ChargingSlow
    );
    let full = battery_is_full(&reading.battery);
    if percentage.is_some() || charging || full {
        process_notification(
            notifications,
            state,
            reading,
            BatterySample {
                // Percentage is irrelevant while charging/full because the engine
                // resets that device's discharge-cycle guard before checking the threshold.
                percentage: percentage.unwrap_or(0),
                charging,
                full,
            },
            diagnostic_log,
            file_logger,
            started,
        )?;
    } else {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Trace,
            &format!(
                "Skipping low-battery notification for {device_name}: battery percentage unavailable and device is not charging or full"
            ),
        )?;
    }
    Ok(())
}

fn process_notification(
    notifications: &mut DeviceNotifications,
    state: Option<&mut DeviceState>,
    reading: &mut DeviceReading,
    sample: BatterySample,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let device_name = format_device_name(&reading.name, reading.device_index);
    let threshold = notifications.config.low_battery_threshold;
    let enabled = notifications.config.notifications_enabled;
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Trace,
        &format!(
            "Evaluating low-battery notification for {device_name}: battery={}%, charging={}, full={}, enabled={enabled}, threshold={threshold}%",
            sample.percentage, sample.charging, sample.full
        ),
    )?;

    let notification_result = notifications.update(reading.identity.clone(), &device_name, sample);
    if let Err(error) = notifications.persist_pending_alerts(state) {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Warn,
            &format!("Could not persist low-battery notification cycle for {device_name}: {error}"),
        )?;
    }
    record_notification_outcome(
        notification_result,
        reading,
        sample,
        threshold,
        diagnostic_log,
        file_logger,
        started,
    )
}

fn record_notification_outcome(
    notification_result: Result<NotificationOutcome, Box<dyn Error>>,
    reading: &mut DeviceReading,
    sample: BatterySample,
    threshold: u8,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let device_name = format_device_name(&reading.name, reading.device_index);
    match notification_result {
        Ok(outcome) => {
            let detail = match outcome {
                NotificationOutcome::NotificationsDisabled => "skipped: disabled in configuration",
                NotificationOutcome::AboveThreshold => "skipped: battery is above the threshold",
                NotificationOutcome::AlreadyNotified => {
                    "skipped: already delivered during this discharge cycle"
                }
                NotificationOutcome::Unchanged => "no notification due",
                NotificationOutcome::CycleReset => "discharge cycle reset",
                NotificationOutcome::Notified => "delivered successfully",
            };
            record_event(
                diagnostic_log,
                file_logger,
                started,
                if outcome == NotificationOutcome::Notified {
                    LogLevel::Info
                } else {
                    LogLevel::Trace
                },
                &format!(
                    "Low-battery notification for {device_name} at {}% (threshold={threshold}%): {detail}",
                    sample.percentage
                ),
            )
        }
        Err(error) => {
            let error = error.to_string();
            record_event(
                diagnostic_log,
                file_logger,
                started,
                LogLevel::Error,
                &format!("Notification error for {device_name}: {error}"),
            )?;
            reading.notification_error = Some(error);
            Ok(())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_devices_drop_notification_tracking_state() {
        let monitored = DeviceIdentity::Hid {
            interface_path: "monitored".to_owned(),
            device_index: 1,
        };
        let unmonitored = DeviceIdentity::Hid {
            interface_path: "unmonitored".to_owned(),
            device_index: 2,
        };
        let mut notifications = DeviceNotifications::from_config(AppConfig::default()).unwrap();
        notifications.engines.insert(
            monitored.clone(),
            NotificationEngine::from_config(AppConfig::default()).unwrap(),
        );
        notifications.engines.insert(
            unmonitored.clone(),
            NotificationEngine::from_config(AppConfig::default()).unwrap(),
        );
        let monitored_devices = BTreeMap::from([(unmonitored.config_id(), false)]);
        let monitored_key = monitored.config_id();
        let unmonitored_key = unmonitored.config_id();
        notifications.restore_persisted_alerts(HashSet::from([
            monitored_key.clone(),
            unmonitored_key.clone(),
        ]));

        notifications.retain_monitored_devices(&monitored_devices);
        assert!(notifications.engines.contains_key(&monitored));
        assert!(!notifications.engines.contains_key(&unmonitored));
        assert!(notifications.persisted_alerts.contains(&monitored_key));
        assert!(!notifications.persisted_alerts.contains(&unmonitored_key));
        assert_eq!(
            notifications.pending_alerts.get(&unmonitored_key),
            Some(&false)
        );
    }
}
