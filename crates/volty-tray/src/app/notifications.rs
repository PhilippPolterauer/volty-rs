use super::{
    battery_is_full, battery_status_label, is_device_monitored, record_event, AppConfig,
    DeviceReading, DeviceState, FileLogger, LogLevel,
};
use crate::notify::{BatterySample, NotificationEngine, NotificationOutcome};
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    io,
    time::Instant,
};
use volty_battery::{BatteryState, DeviceId};

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingAlertState {
    /// Replace the persisted thresholds with exactly this set.
    Set(HashSet<u8>),
    /// Remove all persisted thresholds for the device.
    Clear,
}

pub(super) struct DeviceNotifications {
    config: AppConfig,
    engines: HashMap<DeviceId, NotificationEngine>,
    persisted_alerts: HashMap<String, HashSet<u8>>,
    pending_alerts: HashMap<String, PendingAlertState>,
}
impl DeviceNotifications {
    pub(super) fn from_config(config: AppConfig) -> Result<Self, Box<dyn Error>> {
        NotificationEngine::from_config(&config)?;
        Ok(Self {
            config,
            engines: HashMap::new(),
            persisted_alerts: HashMap::new(),
            pending_alerts: HashMap::new(),
        })
    }

    pub(super) fn restore_persisted_alerts(&mut self, alerts: HashMap<String, HashSet<u8>>) {
        self.persisted_alerts = alerts;
    }

    pub(super) fn set_config(&mut self, config: &AppConfig) -> Result<(), Box<dyn Error>> {
        NotificationEngine::from_config(config)?;
        for engine in self.engines.values_mut() {
            engine.set_config(config)?;
        }
        self.config = config.clone();
        Ok(())
    }

    pub(super) fn retain_monitored_devices(&mut self, monitored_devices: &BTreeMap<String, bool>) {
        self.engines
            .retain(|id, _| is_device_monitored(monitored_devices, id.as_str()));
        let stale_alerts: HashSet<_> = self
            .persisted_alerts
            .keys()
            .chain(self.pending_alerts.keys())
            .filter(|identity| !is_device_monitored(monitored_devices, identity))
            .cloned()
            .collect();
        for identity in stale_alerts {
            self.persisted_alerts.remove(&identity);
            self.pending_alerts
                .insert(identity, PendingAlertState::Clear);
        }
    }

    pub(super) fn update(
        &mut self,
        id: DeviceId,
        device_name: &str,
        sample: BatterySample,
    ) -> Result<NotificationOutcome, Box<dyn Error>> {
        let identity_key = id.as_str().to_owned();
        let previously_notified = self
            .persisted_alerts
            .get(&identity_key)
            .cloned()
            .unwrap_or_default();
        let (outcome, notified_thresholds) = {
            let engine = match self.engines.entry(id) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let mut engine = NotificationEngine::from_config(&self.config)?;
                    engine.restore_notified_thresholds(previously_notified);
                    entry.insert(engine)
                }
            };
            let outcome = engine.update(device_name, sample)?;
            let notified_thresholds: HashSet<u8> =
                engine.notified_thresholds().into_iter().collect();
            (outcome, notified_thresholds)
        };
        match outcome {
            NotificationOutcome::Notified => {
                self.persisted_alerts
                    .insert(identity_key.clone(), notified_thresholds.clone());
                self.pending_alerts
                    .insert(identity_key, PendingAlertState::Set(notified_thresholds));
            }
            NotificationOutcome::CycleReset => {
                self.persisted_alerts.remove(&identity_key);
                self.pending_alerts
                    .insert(identity_key, PendingAlertState::Clear);
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
        for (identity, pending) in &self.pending_alerts {
            state.clear_notified_thresholds(identity)?;
            if let PendingAlertState::Set(thresholds) = pending {
                for &threshold in thresholds {
                    state.mark_threshold_notified(identity, threshold)?;
                }
            }
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
    let device_name = reading.label.display();
    let percentage = reading.battery.level_percent;
    let status = battery_status_label(&reading.battery);
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Debug,
        &format!(
            "Battery read for {device_name} via {}: {} ({status})",
            reading.battery.source,
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
                // resets that device's discharge-cycle guard before checking thresholds.
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
    let device_name = reading.label.display();
    let thresholds = notifications.config.low_battery_thresholds.clone();
    let cooldown_minutes = notifications.config.low_battery_cooldown_minutes;
    let enabled = notifications.config.notifications_enabled;
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Trace,
        &format!(
            "Evaluating low-battery notification for {device_name}: battery={}%, charging={}, full={}, enabled={enabled}, thresholds={thresholds:?}, cooldown_minutes={cooldown_minutes}",
            sample.percentage, sample.charging, sample.full
        ),
    )?;

    let notification_result = notifications.update(reading.id.clone(), &device_name, sample);
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
        &thresholds,
        diagnostic_log,
        file_logger,
        started,
    )
}

fn record_notification_outcome(
    notification_result: Result<NotificationOutcome, Box<dyn Error>>,
    reading: &mut DeviceReading,
    sample: BatterySample,
    thresholds: &[u8],
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let device_name = reading.label.display();
    match notification_result {
        Ok(outcome) => {
            let (detail, level) = match outcome {
                NotificationOutcome::NotificationsDisabled => (
                    "skipped: disabled in configuration".to_owned(),
                    LogLevel::Trace,
                ),
                NotificationOutcome::AboveThreshold => (
                    "skipped: battery is above every threshold".to_owned(),
                    LogLevel::Trace,
                ),
                NotificationOutcome::AlreadyNotified => (
                    "skipped: every reached threshold was already delivered and no repeat is due"
                        .to_owned(),
                    LogLevel::Trace,
                ),
                NotificationOutcome::Unchanged => {
                    ("no notification due".to_owned(), LogLevel::Trace)
                }
                NotificationOutcome::CycleReset => {
                    ("discharge cycle reset".to_owned(), LogLevel::Trace)
                }
                NotificationOutcome::Notified => {
                    ("delivered successfully".to_owned(), LogLevel::Info)
                }
            };
            record_event(
                diagnostic_log,
                file_logger,
                started,
                level,
                &format!(
                    "Low-battery notification for {device_name} at {}% (thresholds={thresholds:?}): {detail}",
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
        let monitored = DeviceId::new("hid:monitored#1");
        let unmonitored = DeviceId::new("hid:unmonitored#2");
        let mut notifications = DeviceNotifications::from_config(AppConfig::default()).unwrap();
        notifications.engines.insert(
            monitored.clone(),
            NotificationEngine::from_config(&AppConfig::default()).unwrap(),
        );
        notifications.engines.insert(
            unmonitored.clone(),
            NotificationEngine::from_config(&AppConfig::default()).unwrap(),
        );
        let monitored_devices = BTreeMap::from([(unmonitored.as_str().to_owned(), false)]);
        let monitored_key = monitored.as_str().to_owned();
        let unmonitored_key = unmonitored.as_str().to_owned();
        notifications.restore_persisted_alerts(HashMap::from([
            (monitored_key.clone(), HashSet::from([20])),
            (unmonitored_key.clone(), HashSet::from([20, 10])),
        ]));

        notifications.retain_monitored_devices(&monitored_devices);
        assert!(notifications.engines.contains_key(&monitored));
        assert!(!notifications.engines.contains_key(&unmonitored));
        assert!(notifications.persisted_alerts.contains_key(&monitored_key));
        assert!(!notifications
            .persisted_alerts
            .contains_key(&unmonitored_key));
        assert_eq!(
            notifications.pending_alerts.get(&unmonitored_key),
            Some(&PendingAlertState::Clear)
        );
    }

    #[test]
    fn persisted_thresholds_suppress_replayed_alerts_after_restart() {
        let device = DeviceId::new("hid:restored#1");
        let identity = device.as_str().to_owned();
        let mut notifications = DeviceNotifications::from_config(AppConfig::default()).unwrap();
        notifications
            .restore_persisted_alerts(HashMap::from([(identity.clone(), HashSet::from([20]))]));

        assert_eq!(
            notifications
                .update(
                    device.clone(),
                    "Test device",
                    BatterySample {
                        percentage: 19,
                        charging: false,
                        full: false,
                    },
                )
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
        assert_eq!(
            notifications
                .update(
                    device,
                    "Test device",
                    BatterySample {
                        percentage: 9,
                        charging: false,
                        full: false,
                    },
                )
                .unwrap(),
            NotificationOutcome::Notified
        );
        assert_eq!(
            notifications.pending_alerts.get(&identity),
            Some(&PendingAlertState::Set(HashSet::from([10, 20])))
        );
    }
}
