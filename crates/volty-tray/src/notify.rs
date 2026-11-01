//! Native low-battery notification policy.
//!
//! Feed each battery sample to [`NotificationEngine::update`] and surface
//! returned errors in the application's log or UI. Thresholds are read from a
//! per-user TOML config file (or can be supplied directly by the app).

use std::{
    collections::BTreeSet,
    error::Error,
    fmt,
    path::Path,
    time::{Duration, Instant},
};

use crate::config::{normalize_low_battery_thresholds, AppConfig};

/// Battery data needed by the low-battery notification policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatterySample {
    pub percentage: u8,
    pub charging: bool,
    pub full: bool,
}

/// Result of processing a sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOutcome {
    /// Notifications are disabled in the active configuration.
    NotificationsDisabled,
    /// The battery percentage is above every configured threshold.
    AboveThreshold,
    /// Every reached threshold was already notified and no repeat is due.
    AlreadyNotified,
    /// No notification was due for this sample.
    Unchanged,
    /// Charging or a full battery reset a previously consumed cycle.
    CycleReset,
    /// A low-battery notification was delivered.
    Notified,
}

/// Error returned when configuration or the native notification backend fails.
#[derive(Debug)]
pub struct NotificationError(String);

impl NotificationError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for NotificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for NotificationError {}

/// Tracks one alert per configured threshold in each discharge cycle and
/// optionally repeats the lowest alert on a cooldown.
#[derive(Debug)]
pub struct NotificationEngine {
    thresholds: Vec<u8>,
    notified_thresholds: BTreeSet<u8>,
    notifications_enabled: bool,
    cooldown: Duration,
    last_repeat: Option<Instant>,
}

impl NotificationEngine {
    /// Creates an engine from loaded or otherwise app-provided settings.
    pub fn from_config(config: &AppConfig) -> Result<Self, NotificationError> {
        Ok(Self {
            thresholds: configured_thresholds(config)?,
            notified_thresholds: BTreeSet::new(),
            notifications_enabled: config.notifications_enabled,
            cooldown: Duration::from_secs(config.low_battery_cooldown_minutes.saturating_mul(60)),
            last_repeat: None,
        })
    }

    /// Restores the thresholds already alerted in the current discharge cycle.
    /// A restored lowest threshold restarts the cooldown from now so a restart
    /// cannot immediately repeat an alert.
    pub(crate) fn restore_notified_thresholds(&mut self, notified: impl IntoIterator<Item = u8>) {
        self.notified_thresholds = notified
            .into_iter()
            .filter(|threshold| self.thresholds.contains(threshold))
            .collect();
        self.last_repeat = self
            .thresholds
            .last()
            .filter(|lowest| self.notified_thresholds.contains(lowest))
            .map(|_| Instant::now());
    }

    pub(crate) fn notified_thresholds(&self) -> BTreeSet<u8> {
        self.notified_thresholds.clone()
    }

    /// Returns the active alert thresholds, highest first.
    #[cfg(test)]
    pub fn thresholds(&self) -> &[u8] {
        &self.thresholds
    }

    /// Applies a validated configuration without resetting thresholds that are
    /// still configured and already alerted in this discharge cycle.
    pub fn set_config(&mut self, config: &AppConfig) -> Result<(), NotificationError> {
        let thresholds = configured_thresholds(config)?;
        self.notified_thresholds
            .retain(|threshold| thresholds.contains(threshold));
        self.thresholds = thresholds;
        self.notifications_enabled = config.notifications_enabled;
        self.cooldown = Duration::from_secs(config.low_battery_cooldown_minutes.saturating_mul(60));
        if !self
            .thresholds
            .last()
            .is_some_and(|lowest| self.notified_thresholds.contains(lowest))
        {
            self.last_repeat = None;
        }
        Ok(())
    }

    pub(super) fn update_with(
        &mut self,
        device_name: &str,
        sample: BatterySample,
        now: Instant,
        dispatch: impl FnOnce(&str, u8) -> Result<(), NotificationError>,
    ) -> Result<NotificationOutcome, NotificationError> {
        if sample.charging || sample.full || sample.percentage >= 100 {
            let had_cycle = !self.notified_thresholds.is_empty() || self.last_repeat.is_some();
            self.notified_thresholds.clear();
            self.last_repeat = None;
            return Ok(if had_cycle {
                NotificationOutcome::CycleReset
            } else {
                NotificationOutcome::Unchanged
            });
        }

        if !self.notifications_enabled {
            return Ok(NotificationOutcome::NotificationsDisabled);
        }

        if sample.percentage > self.thresholds[0] {
            return Ok(NotificationOutcome::AboveThreshold);
        }

        let reached: Vec<u8> = self
            .thresholds
            .iter()
            .copied()
            .filter(|threshold| {
                sample.percentage <= *threshold && !self.notified_thresholds.contains(threshold)
            })
            .collect();
        if !reached.is_empty() {
            // A single toast covers a jump past several thresholds; mark them
            // all so later polls cannot deliver stale higher-threshold alerts.
            self.notified_thresholds.extend(reached);
            if self
                .thresholds
                .last()
                .is_some_and(|lowest| self.notified_thresholds.contains(lowest))
            {
                self.last_repeat = Some(now);
            }
            dispatch(device_name, sample.percentage)?;
            return Ok(NotificationOutcome::Notified);
        }

        let lowest = *self.thresholds.last().expect("thresholds are not empty");
        if sample.percentage <= lowest
            && self.notified_thresholds.contains(&lowest)
            && !self.cooldown.is_zero()
        {
            let repeat_due = self
                .last_repeat
                .is_none_or(|last| now.saturating_duration_since(last) >= self.cooldown);
            if repeat_due {
                self.last_repeat = Some(now);
                dispatch(device_name, sample.percentage)?;
                return Ok(NotificationOutcome::Notified);
            }
        }

        Ok(NotificationOutcome::AlreadyNotified)
    }
}

fn configured_thresholds(config: &AppConfig) -> Result<Vec<u8>, NotificationError> {
    normalize_low_battery_thresholds(&config.low_battery_thresholds)
        .map_err(|error| NotificationError::new(error.to_string()))
}

#[cfg(windows)]
pub(super) fn show_low_battery_notification(
    device_name: &str,
    percentage: u8,
) -> Result<(), NotificationError> {
    notify_rust::Notification::new()
        .summary("Volty — Low Battery")
        .body(&format!(
            "{device_name} battery is at {percentage}%. Connect charging cable."
        ))
        .show()
        .map(|_| ())
        .map_err(|error| {
            NotificationError::new(format!("could not show low-battery toast: {error}"))
        })
}

#[cfg(windows)]
pub(super) fn show_update_available_notification(
    version: &str,
) -> Result<notify_rust::NotificationHandle, NotificationError> {
    notify_rust::Notification::new()
        .summary("Volty update available")
        .body(&format!(
            "Volty {version} is available. Click to review the update."
        ))
        .show()
        .map_err(|error| {
            NotificationError::new(format!("could not show update-available toast: {error}"))
        })
}

pub(crate) fn update_notification_opens_prompt(
    response: &notify_rust::NotificationResponse,
) -> bool {
    matches!(response, notify_rust::NotificationResponse::Default)
}

/// Shows a toast whose body or Open Capture action can open the saved file.
pub(crate) fn show_capture_saved_notification(
    path: &Path,
) -> Result<notify_rust::NotificationHandle, NotificationError> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "hardware capture".into());
    let body = format!("Saved {file_name}. Click to open it.");
    let mut notification = notify_rust::Notification::new();
    notification
        .summary("Volty — Hardware Capture Saved")
        .body(&body)
        .action("open_capture", "Open Capture");
    notification.show().map_err(|error| {
        NotificationError::new(format!("could not show hardware-capture toast: {error}"))
    })
}

pub(crate) fn capture_notification_opens_file(
    response: &notify_rust::NotificationResponse,
) -> bool {
    match response {
        notify_rust::NotificationResponse::Default => true,
        notify_rust::NotificationResponse::Action(action) => action == "open_capture",
        notify_rust::NotificationResponse::Reply(_)
        | notify_rust::NotificationResponse::Closed(_) => false,
    }
}

#[cfg(not(windows))]
pub(super) fn show_low_battery_notification(
    _device_name: &str,
    _percentage: u8,
) -> Result<(), NotificationError> {
    Err(NotificationError::new(
        "native low-battery notifications are only supported on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_notification_opens_only_for_the_body_or_open_button() {
        use notify_rust::{CloseReason, NotificationResponse};

        assert!(capture_notification_opens_file(
            &NotificationResponse::Default
        ));
        assert!(capture_notification_opens_file(
            &NotificationResponse::Action("open_capture".to_owned())
        ));
        assert!(!capture_notification_opens_file(
            &NotificationResponse::Action("other".to_owned())
        ));
        assert!(!capture_notification_opens_file(
            &NotificationResponse::Closed(CloseReason::Dismissed)
        ));
        assert!(!capture_notification_opens_file(
            &NotificationResponse::Reply("not used".to_owned())
        ));
    }

    #[test]
    fn update_notification_opens_prompt_only_when_clicked() {
        use notify_rust::{CloseReason, NotificationResponse};

        assert!(update_notification_opens_prompt(
            &NotificationResponse::Default
        ));
        assert!(!update_notification_opens_prompt(
            &NotificationResponse::Action("other".to_owned())
        ));
        assert!(!update_notification_opens_prompt(
            &NotificationResponse::Closed(CloseReason::Dismissed)
        ));
    }

    fn discharging(percentage: u8) -> BatterySample {
        BatterySample {
            percentage,
            charging: false,
            full: false,
        }
    }

    fn config(cooldown_minutes: u64) -> AppConfig {
        AppConfig {
            low_battery_cooldown_minutes: cooldown_minutes,
            ..AppConfig::default()
        }
    }

    #[test]
    fn multi_threshold_alerts_fire_once_per_discharge_cycle() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        assert_eq!(engine.thresholds(), [20, 10, 5]);
        let now = Instant::now();
        let mut fired = Vec::new();
        let mut outcomes = Vec::new();
        for percentage in [25, 19, 19, 9, 9, 4, 4] {
            outcomes.push(
                engine
                    .update_with("Test device", discharging(percentage), now, |_, value| {
                        fired.push(value);
                        Ok(())
                    })
                    .unwrap(),
            );
        }

        assert_eq!(
            outcomes,
            [
                NotificationOutcome::AboveThreshold,
                NotificationOutcome::Notified,
                NotificationOutcome::AlreadyNotified,
                NotificationOutcome::Notified,
                NotificationOutcome::AlreadyNotified,
                NotificationOutcome::Notified,
                NotificationOutcome::AlreadyNotified,
            ]
        );
        assert_eq!(fired, [19, 9, 4]);
        assert_eq!(engine.notified_thresholds(), BTreeSet::from([5, 10, 20]));
    }

    #[test]
    fn jumping_past_thresholds_sends_one_alert_and_suppresses_stale_ones() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        let now = Instant::now();
        let mut fired = Vec::new();

        assert_eq!(
            engine
                .update_with("Test device", discharging(4), now, |_, value| {
                    fired.push(value);
                    Ok(())
                })
                .unwrap(),
            NotificationOutcome::Notified
        );
        assert_eq!(fired, [4]);
        assert_eq!(engine.notified_thresholds(), BTreeSet::from([5, 10, 20]));
        assert_eq!(
            engine
                .update_with("Test device", discharging(4), now, |_, _| panic!(
                    "stale thresholds must not be delivered after a jump"
                ))
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
    }

    #[test]
    fn charging_and_full_samples_reset_every_notified_threshold() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        let now = Instant::now();
        for percentage in [19, 9, 4] {
            assert!(matches!(
                engine
                    .update_with("Test device", discharging(percentage), now, |_, _| Ok(()))
                    .unwrap(),
                NotificationOutcome::Notified
            ));
        }

        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    BatterySample {
                        percentage: 4,
                        charging: true,
                        full: false,
                    },
                    now,
                    |_, _| panic!("charging must not dispatch a notification"),
                )
                .unwrap(),
            NotificationOutcome::CycleReset
        );
        assert!(engine.notified_thresholds().is_empty());
        assert_eq!(
            engine
                .update_with("Test device", discharging(4), now, |_, _| Ok(()))
                .unwrap(),
            NotificationOutcome::Notified
        );

        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    BatterySample {
                        percentage: 100,
                        charging: false,
                        full: false,
                    },
                    now,
                    |_, _| panic!("a full battery must not dispatch a notification"),
                )
                .unwrap(),
            NotificationOutcome::CycleReset
        );
        assert!(engine.notified_thresholds().is_empty());
        assert_eq!(
            engine
                .update_with("Test device", discharging(19), now, |_, _| Ok(()))
                .unwrap(),
            NotificationOutcome::Notified
        );
    }

    #[test]
    fn cooldown_repeats_the_lowest_threshold_with_an_injected_clock() {
        let mut engine = NotificationEngine::from_config(&config(120)).unwrap();
        let start = Instant::now();
        for percentage in [19, 9, 4] {
            assert!(matches!(
                engine
                    .update_with("Test device", discharging(percentage), start, |_, _| Ok(()))
                    .unwrap(),
                NotificationOutcome::Notified
            ));
        }

        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(4),
                    start + Duration::from_secs(119 * 60),
                    |_, _| panic!("the cooldown has not elapsed"),
                )
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(4),
                    start + Duration::from_secs(120 * 60),
                    |_, _| Ok(()),
                )
                .unwrap(),
            NotificationOutcome::Notified
        );
        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(4),
                    start + Duration::from_secs(239 * 60),
                    |_, _| panic!("the repeat cooldown restarted after the last repeat"),
                )
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(4),
                    start + Duration::from_secs(240 * 60),
                    |_, _| Ok(()),
                )
                .unwrap(),
            NotificationOutcome::Notified
        );
    }

    #[test]
    fn zero_cooldown_disables_repeats() {
        let mut engine = NotificationEngine::from_config(&config(0)).unwrap();
        let start = Instant::now();
        for percentage in [19, 9, 4] {
            assert!(matches!(
                engine
                    .update_with("Test device", discharging(percentage), start, |_, _| Ok(()))
                    .unwrap(),
                NotificationOutcome::Notified
            ));
        }

        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(4),
                    start + Duration::from_secs(365 * 24 * 60 * 60),
                    |_, _| panic!("zero cooldown disables repeats"),
                )
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
    }

    #[test]
    fn restored_thresholds_suppress_duplicate_alerts() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        engine.restore_notified_thresholds([20]);
        let now = Instant::now();
        assert_eq!(
            engine
                .update_with("Test device", discharging(19), now, |_, _| panic!(
                    "a restored alert must not be dispatched again"
                ))
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
        assert_eq!(
            engine
                .update_with("Test device", discharging(9), now, |_, _| Ok(()))
                .unwrap(),
            NotificationOutcome::Notified
        );
    }

    #[test]
    fn thresholds_are_configurable_validated_and_sorted() {
        let config = AppConfig {
            low_battery_thresholds: vec![5, 20, 10, 20],
            ..AppConfig::default()
        };
        let mut engine = NotificationEngine::from_config(&config).unwrap();
        assert_eq!(engine.thresholds(), [20, 10, 5]);
        assert!(NotificationEngine::from_config(&AppConfig {
            low_battery_thresholds: Vec::new(),
            ..AppConfig::default()
        })
        .is_err());
        assert!(NotificationEngine::from_config(&AppConfig {
            low_battery_thresholds: vec![0],
            ..AppConfig::default()
        })
        .is_err());
        assert!(engine
            .set_config(&AppConfig {
                low_battery_thresholds: vec![101],
                ..AppConfig::default()
            })
            .is_err());
        assert_eq!(engine.thresholds(), [20, 10, 5]);
        engine
            .set_config(&AppConfig {
                low_battery_thresholds: vec![25, 15, 25],
                ..AppConfig::default()
            })
            .unwrap();
        assert_eq!(engine.thresholds(), [25, 15]);
    }

    #[test]
    fn configured_threshold_dispatches_low_battery_notification_once() {
        let config = AppConfig {
            low_battery_thresholds: vec![100],
            ..AppConfig::default()
        };
        let mut engine = NotificationEngine::from_config(&config).unwrap();
        let sample = discharging(78);
        let now = Instant::now();
        let mut dispatched = None;

        assert_eq!(
            engine
                .update_with("Test device", sample, now, |name, percentage| {
                    dispatched = Some((name.to_owned(), percentage));
                    Ok(())
                })
                .unwrap(),
            NotificationOutcome::Notified
        );
        assert_eq!(dispatched, Some(("Test device".to_owned(), 78)));
        assert_eq!(
            engine
                .update_with("Test device", sample, now, |_, _| {
                    panic!("a discharge cycle must only dispatch one notification per threshold")
                })
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
    }

    #[test]
    fn failed_dispatch_consumes_the_reached_thresholds() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        let now = Instant::now();
        let error = engine
            .update_with("Test device", discharging(10), now, |_, _| {
                Err(NotificationError::new("dispatch failed"))
            })
            .unwrap_err();

        assert_eq!(error.to_string(), "dispatch failed");
        assert_eq!(engine.notified_thresholds(), BTreeSet::from([10, 20]));
        assert_eq!(
            engine
                .update_with("Test device", discharging(10), now, |_, _| panic!(
                    "a failed dispatch must not be retried in the same cycle"
                ))
                .unwrap(),
            NotificationOutcome::AlreadyNotified
        );
    }

    #[test]
    fn above_threshold_sample_reports_skipped_notification() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        let outcome = engine
            .update_with("Test device", discharging(21), Instant::now(), |_, _| {
                panic!("an above-threshold sample must not dispatch a notification")
            })
            .unwrap();

        assert_eq!(outcome, NotificationOutcome::AboveThreshold);
    }

    #[test]
    fn reloading_thresholds_preserves_matching_discharge_guards() {
        let mut engine = NotificationEngine::from_config(&AppConfig::default()).unwrap();
        let now = Instant::now();
        assert_eq!(
            engine
                .update_with("Test device", discharging(19), now, |_, _| Ok(()))
                .unwrap(),
            NotificationOutcome::Notified
        );
        engine
            .set_config(&AppConfig {
                low_battery_thresholds: vec![10],
                ..AppConfig::default()
            })
            .unwrap();
        assert_eq!(engine.thresholds(), [10]);
        assert!(engine.notified_thresholds().is_empty());
        assert_eq!(
            engine
                .update_with("Test device", discharging(9), now, |_, _| Ok(()))
                .unwrap(),
            NotificationOutcome::Notified
        );
    }

    #[test]
    fn disabled_notifications_do_not_consume_the_discharge_guard() {
        let mut engine = NotificationEngine::from_config(&AppConfig {
            notifications_enabled: false,
            ..AppConfig::default()
        })
        .unwrap();
        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(10),
                    Instant::now(),
                    |_, _| panic!("disabled notifications must not dispatch"),
                )
                .unwrap(),
            NotificationOutcome::NotificationsDisabled
        );
        assert!(engine.notified_thresholds().is_empty());

        engine.set_config(&AppConfig::default()).unwrap();
        assert_eq!(
            engine
                .update_with(
                    "Test device",
                    discharging(10),
                    Instant::now(),
                    |_, _| Ok(())
                )
                .unwrap(),
            NotificationOutcome::Notified
        );
    }
}
