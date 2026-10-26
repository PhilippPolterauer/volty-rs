//! Native low-battery notification policy.
//!
//! Feed each battery sample to [`NotificationEngine::update`] and surface
//! returned errors in the application's log or UI. The threshold is read from
//! a small per-user `config.ini` file (or can be supplied directly by the app).

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

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

/// Persistable low-battery notification settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationConfig {
    /// Alert when the battery reaches this percentage or lower (1–100).
    pub low_battery_threshold: u8,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            low_battery_threshold: NotificationEngine::DEFAULT_THRESHOLD_PERCENT,
        }
    }
}

impl NotificationConfig {
    /// Loads `low_battery_threshold` from the platform's Volty config file.
    /// Missing config files use the default 20 percent threshold.
    pub fn load() -> Result<Self, NotificationError> {
        match config_path() {
            Some(path) => Self::load_from(path),
            None => Ok(Self::default()),
        }
    }

    /// Loads settings from an explicit path.
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, NotificationError> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => {
                return Err(NotificationError::new(format!(
                    "could not read notification config {}: {error}",
                    path.display()
                )))
            }
        };

        parse_config(&contents, path)
    }
}

/// Returns the default per-user config file location, when available.
pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|root| root.join("Volty").join("config.ini"))
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("volty").join("config.ini"))
    }
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
    pub const DEFAULT_THRESHOLD_PERCENT: u8 = 20;

    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an engine using the locally persisted configuration.
    pub fn from_local_config() -> Result<Self, NotificationError> {
        Self::from_config(NotificationConfig::load()?)
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

    /// Updates the threshold for this process. The setting on disk is edited
    /// separately in the user's `config.ini`.
    #[cfg(test)]
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

fn parse_config(contents: &str, path: &Path) -> Result<NotificationConfig, NotificationError> {
    let mut config = NotificationConfig::default();
    let mut threshold_seen = false;

    for (line_index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let invalid = |reason: &str| {
            NotificationError::new(format!(
                "invalid notification config {} at line {}: {reason}",
                path.display(),
                line_index + 1
            ))
        };
        let Some((key, value)) = line.split_once('=') else {
            return Err(invalid("expected key=value"));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            return Err(invalid("key and value must not be empty"));
        }
        if key != "low_battery_threshold" {
            return Err(invalid(&format!("unknown setting {key:?}")));
        }
        if threshold_seen {
            return Err(invalid("low_battery_threshold is specified more than once"));
        }
        threshold_seen = true;

        let threshold = value.parse::<u8>().map_err(|error| {
            invalid(&format!(
                "invalid low_battery_threshold value {value:?}: {error}"
            ))
        })?;
        if !(1..=100).contains(&threshold) {
            return Err(invalid(
                "low_battery_threshold must be between 1 and 100 percent",
            ));
        }
        config.low_battery_threshold = threshold;
    }

    Ok(config)
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
        engine.set_threshold(25).unwrap();
        assert_eq!(engine.threshold_percent(), 25);
    }

    #[test]
    fn config_parser_accepts_defaults_and_threshold_values() {
        let path = Path::new("config.ini");
        assert_eq!(
            parse_config("# comment\n", path).unwrap(),
            NotificationConfig::default()
        );
        assert_eq!(
            parse_config("low_battery_threshold = 15\n", path).unwrap(),
            NotificationConfig {
                low_battery_threshold: 15
            }
        );
    }

    #[test]
    fn config_parser_rejects_malformed_and_unsupported_settings() {
        let path = Path::new("config.ini");
        for malformed in [
            "low_battery_threshold\n",
            "=20\n",
            "low_battery_threshold=\n",
            "low_battery_threshold=not-a-number\n",
            "low_battery_threshold=0\n",
            "low_battery_threshold=101\n",
            "unknown_setting=20\n",
            "low_battery_threshold=20\nlow_battery_threshold=15\n",
        ] {
            let error = parse_config(malformed, path).unwrap_err();
            assert!(
                error.to_string().contains("line"),
                "missing location in {error}"
            );
        }
    }
}
