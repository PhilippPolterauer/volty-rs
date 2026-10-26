//! Native low-battery notification policy.
//!
//! Feed each battery sample to [`NotificationEngine::update`] and surface
//! returned errors in the application's log or UI. The threshold is read from
//! a per-user TOML config file (or can be supplied directly by the app).

use std::{error::Error, fmt};

use crate::config::AppConfig;

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
    /// No notification was due.
    Unchanged,
    /// Charging or a full battery reset a previously consumed cycle.
    CycleReset,
    /// The low-battery notification was delivered.
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

/// Tracks one alert per discharge cycle, firing at or below the configured
/// threshold. The default threshold is 20 percent.
#[derive(Debug)]
pub struct NotificationEngine {
    notified_this_discharge: bool,
    threshold_percent: u8,
    notifications_enabled: bool,
}

impl NotificationEngine {
    /// Creates an engine from loaded or otherwise app-provided settings.
    pub fn from_config(config: AppConfig) -> Result<Self, NotificationError> {
        validate_threshold(config.low_battery_threshold)?;
        Ok(Self {
            notified_this_discharge: false,
            threshold_percent: config.low_battery_threshold,
            notifications_enabled: config.notifications_enabled,
        })
    }

    /// Returns the active alert threshold.
    #[cfg(test)]
    pub fn threshold_percent(&self) -> u8 {
        self.threshold_percent
    }

    /// Applies a validated configuration without resetting the current
    /// discharge alert guard.
    pub fn set_config(&mut self, config: AppConfig) -> Result<(), NotificationError> {
        validate_threshold(config.low_battery_threshold)?;
        self.threshold_percent = config.low_battery_threshold;
        self.notifications_enabled = config.notifications_enabled;
        Ok(())
    }

    /// Processes one battery sample and attempts a native toast when due.
    ///
    /// A sample that is charging or full resets the one-shot guard. The guard
    /// is set before dispatch, making failures visible to the caller without
    /// repeatedly attempting the same toast on every polling iteration.
    pub fn update(
        &mut self,
        sample: BatterySample,
    ) -> Result<NotificationOutcome, NotificationError> {
        if sample.charging || sample.full || sample.percentage >= 100 {
            let was_notified = self.notified_this_discharge;
            self.notified_this_discharge = false;
            return Ok(if was_notified {
                NotificationOutcome::CycleReset
            } else {
                NotificationOutcome::Unchanged
            });
        }

        if !self.notifications_enabled {
            return Ok(NotificationOutcome::Unchanged);
        }

        if !self.consume_discharge_alert(sample.percentage) {
            return Ok(NotificationOutcome::Unchanged);
        }

        show_low_battery_notification(sample.percentage)?;
        Ok(NotificationOutcome::Notified)
    }

    fn consume_discharge_alert(&mut self, percentage: u8) -> bool {
        if percentage > self.threshold_percent || self.notified_this_discharge {
            return false;
        }
        self.notified_this_discharge = true;
        true
    }
}

fn validate_threshold(threshold_percent: u8) -> Result<(), NotificationError> {
    if (1..=100).contains(&threshold_percent) {
        Ok(())
    } else {
        Err(NotificationError::new(
            "low-battery threshold must be between 1 and 100 percent",
        ))
    }
}

#[cfg(windows)]
fn show_low_battery_notification(percentage: u8) -> Result<(), NotificationError> {
    notify_rust::Notification::new()
        .summary("Volty — Low Battery")
        .body(&format!(
            "Logitech G502 X battery is at {percentage}%. Connect charging cable."
        ))
        .show()
        .map(|_| ())
        .map_err(|error| {
            NotificationError::new(format!("could not show low-battery toast: {error}"))
        })
}

#[cfg(not(windows))]
fn show_low_battery_notification(_percentage: u8) -> Result<(), NotificationError> {
    Err(NotificationError::new(
        "native low-battery notifications are only supported on Windows",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_and_threshold_policy_are_one_shot_per_discharge() {
        let mut engine = NotificationEngine::from_config(AppConfig::default()).unwrap();
        assert_eq!(engine.threshold_percent(), 20);
        assert!(!engine.consume_discharge_alert(21));
        assert!(engine.consume_discharge_alert(20));
        assert!(!engine.consume_discharge_alert(10));
        assert_eq!(
            engine
                .update(BatterySample {
                    percentage: 19,
                    charging: true,
                    full: false,
                })
                .unwrap(),
            NotificationOutcome::CycleReset
        );
        assert!(engine.consume_discharge_alert(10));
    }

    #[test]
    fn threshold_is_configurable_and_validated() {
        let low_threshold = AppConfig {
            low_battery_threshold: 10,
            ..AppConfig::default()
        };
        let mut engine = NotificationEngine::from_config(low_threshold).unwrap();
        assert_eq!(engine.threshold_percent(), 10);
        assert!(NotificationEngine::from_config(AppConfig {
            low_battery_threshold: 0,
            ..AppConfig::default()
        })
        .is_err());
        assert!(engine
            .set_config(AppConfig {
                low_battery_threshold: 101,
                ..AppConfig::default()
            })
            .is_err());
        assert_eq!(engine.threshold_percent(), 10);
        engine
            .set_config(AppConfig {
                low_battery_threshold: 25,
                ..AppConfig::default()
            })
            .unwrap();
        assert_eq!(engine.threshold_percent(), 25);
    }

    #[test]
    fn reloading_threshold_preserves_discharge_notification_guard() {
        let mut engine = NotificationEngine::from_config(AppConfig::default()).unwrap();
        assert!(engine.consume_discharge_alert(20));
        engine
            .set_config(AppConfig {
                low_battery_threshold: 10,
                ..AppConfig::default()
            })
            .unwrap();
        assert!(!engine.consume_discharge_alert(10));
        assert_eq!(engine.threshold_percent(), 10);
    }

    #[test]
    fn disabled_notifications_do_not_consume_the_discharge_guard() {
        let mut engine = NotificationEngine::from_config(AppConfig {
            notifications_enabled: false,
            ..AppConfig::default()
        })
        .unwrap();
        assert_eq!(
            engine
                .update(BatterySample {
                    percentage: 10,
                    charging: false,
                    full: false,
                })
                .unwrap(),
            NotificationOutcome::Unchanged
        );

        engine.set_config(AppConfig::default()).unwrap();
        assert!(engine.consume_discharge_alert(10));
    }
}
