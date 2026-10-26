//! Native low-battery notification policy.
//!
//! Feed each battery sample to [`NotificationEngine::update`] and surface
//! returned errors in the application's log or UI. The threshold is read from
//! a per-user TOML config file (or can be supplied directly by the app).

use std::{error::Error, fmt};

use crate::config::NotificationConfig;

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
}

impl Default for NotificationEngine {
    fn default() -> Self {
        Self {
            notified_this_discharge: false,
            threshold_percent: Self::DEFAULT_THRESHOLD_PERCENT,
        }
    }
}

impl NotificationEngine {
    pub const DEFAULT_THRESHOLD_PERCENT: u8 = crate::config::DEFAULT_LOW_BATTERY_THRESHOLD;

    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an engine using the locally persisted configuration.
    pub fn from_local_config() -> Result<Self, NotificationError> {
        let config = NotificationConfig::load()
            .map_err(|error| NotificationError::new(error.to_string()))?;
        Self::from_config(config)
    }

    /// Creates an engine with a configured threshold from 1 through 100.
    pub fn with_threshold(threshold_percent: u8) -> Result<Self, NotificationError> {
        validate_threshold(threshold_percent)?;
        Ok(Self {
            notified_this_discharge: false,
            threshold_percent,
        })
    }

    /// Creates an engine from loaded or otherwise app-provided settings.
    pub fn from_config(config: NotificationConfig) -> Result<Self, NotificationError> {
        Self::with_threshold(config.low_battery_threshold)
    }

    /// Returns the active alert threshold.
    #[cfg(test)]
    pub fn threshold_percent(&self) -> u8 {
        self.threshold_percent
    }

    /// Updates the threshold without resetting the current discharge alert
    /// guard. Invalid values leave the active threshold unchanged.
    pub fn set_threshold(&mut self, threshold_percent: u8) -> Result<(), NotificationError> {
        validate_threshold(threshold_percent)?;
        self.threshold_percent = threshold_percent;
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
        let mut engine = NotificationEngine::new();
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
        let mut engine = NotificationEngine::with_threshold(10).unwrap();
        assert_eq!(engine.threshold_percent(), 10);
        assert!(NotificationEngine::with_threshold(0).is_err());
        assert!(engine.set_threshold(101).is_err());
        assert_eq!(engine.threshold_percent(), 10);
        engine.set_threshold(25).unwrap();
        assert_eq!(engine.threshold_percent(), 25);
    }

    #[test]
    fn reloading_threshold_preserves_discharge_notification_guard() {
        let mut engine = NotificationEngine::new();
        assert!(engine.consume_discharge_alert(20));
        engine.set_threshold(10).unwrap();
        assert!(!engine.consume_discharge_alert(10));
        assert_eq!(engine.threshold_percent(), 10);
    }
}
