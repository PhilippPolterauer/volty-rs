use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use toml_edit::Document;

pub const DEFAULT_LOW_BATTERY_THRESHOLD: u8 = 20;
pub const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 120;
pub const DEFAULT_RETRY_INTERVAL_SECONDS: u64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryColorMap {
    pub red_max_percent: u8,
    pub yellow_max_percent: u8,
    pub red: [u8; 3],
    pub yellow: [u8; 3],
    pub green: [u8; 3],
}

impl BatteryColorMap {
    pub const fn color_for(self, percentage: u8) -> [u8; 3] {
        if percentage <= self.red_max_percent {
            self.red
        } else if percentage <= self.yellow_max_percent {
            self.yellow
        } else {
            self.green
        }
    }
}

impl Default for BatteryColorMap {
    fn default() -> Self {
        Self {
            red_max_percent: 15,
            yellow_max_percent: 30,
            red: [210, 48, 48],
            yellow: [235, 190, 35],
            green: [45, 170, 75],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }

    pub fn includes(self, event_level: Self) -> bool {
        event_level <= self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppConfig {
    pub low_battery_threshold: u8,
    pub poll_interval_seconds: u64,
    pub retry_interval_seconds: u64,
    pub notifications_enabled: bool,
    pub log_level: LogLevel,
    pub battery_color_map: BatteryColorMap,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            low_battery_threshold: DEFAULT_LOW_BATTERY_THRESHOLD,
            poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
            retry_interval_seconds: DEFAULT_RETRY_INTERVAL_SECONDS,
            notifications_enabled: true,
            log_level: LogLevel::Info,
            battery_color_map: BatteryColorMap::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        Self::load_from(path)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = Self::ensure_config_file_at(path)?;
        let contents = fs::read_to_string(&path).map_err(|error| {
            ConfigError::new(format!(
                "could not read configuration {}: {error}",
                path.display()
            ))
        })?;

        parse_toml(&contents, &path)
    }

    pub fn ensure_config_file() -> Result<PathBuf, ConfigError> {
        let path = config_path()
            .ok_or_else(|| ConfigError::new("could not determine the user configuration folder"))?;
        Self::ensure_config_file_at(path)
    }

    fn ensure_config_file_at(path: impl AsRef<Path>) -> Result<PathBuf, ConfigError> {
        let path = path.as_ref();
        if path.exists() {
            return Ok(path.to_path_buf());
        }

        let defaults = Self::default();
        let contents = format!(
            "# Volty user configuration\n\
             # Notify at or below this battery percentage (1-100).\n\
             # low_battery_threshold = {}\n\
             # Seconds between successful battery readings (30-3600).\n\
             # poll_interval_seconds = {}\n\
             # Seconds between retries after a polling error (5-300).\n\
             # retry_interval_seconds = {}\n\
             # Set to false to disable low-battery toast notifications.\n\
             # notifications_enabled = {}\n\
             # error, warn, info, debug, or trace.\n\
             # log_level = \"{}\"\n\
             # Uncomment this table to override the default tray battery colors.\n\
             # Thresholds are inclusive upper bounds; red must be below yellow.\n\
             # [battery_color_map]\n\
             # red_max_percent = 15\n\
             # yellow_max_percent = 30\n\
             # red = \"#d23030\"\n\
             # yellow = \"#ebbe23\"\n\
             # green = \"#2daa4b\"\n\
             # Per-device monitoring choices are stored in local SQLite state.\n",
            defaults.low_battery_threshold,
            defaults.poll_interval_seconds,
            defaults.retry_interval_seconds,
            defaults.notifications_enabled,
            defaults.log_level.as_str().to_ascii_lowercase()
        );
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| {
            ConfigError::new(format!(
                "could not create configuration folder {}: {error}",
                parent.display()
            ))
        })?;

        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(path.to_path_buf())
            }
            Err(error) => {
                return Err(ConfigError::new(format!(
                    "could not create configuration file {}: {error}",
                    path.display()
                )))
            }
        };
        file.write_all(contents.as_bytes()).map_err(|error| {
            ConfigError::new(format!(
                "could not write configuration file {}: {error}",
                path.display()
            ))
        })?;
        Ok(path.to_path_buf())
    }
}

pub fn config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|root| root.join("Volty").join("config.toml"))
    }

    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("volty").join("config.toml"))
    }
}

pub fn load_legacy_monitored_devices() -> Result<BTreeMap<String, bool>, ConfigError> {
    let Some(path) = config_path() else {
        return Ok(BTreeMap::new());
    };
    load_legacy_monitored_devices_from(path)
}

fn load_legacy_monitored_devices_from(
    path: impl AsRef<Path>,
) -> Result<BTreeMap<String, bool>, ConfigError> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| {
        ConfigError::new(format!(
            "could not read configuration {}: {error}",
            path.display()
        ))
    })?;
    let document = contents.parse::<toml::Value>().map_err(|error| {
        ConfigError::new(format!("invalid configuration {}: {error}", path.display()))
    })?;
    let Some(value) = document
        .as_table()
        .and_then(|table| table.get("monitored_devices"))
    else {
        return Ok(BTreeMap::new());
    };
    let devices = value.as_table().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: legacy monitored_devices must be a table",
            path.display()
        ))
    })?;
    let mut monitored_devices = BTreeMap::new();
    for (identity, value) in devices {
        if identity.is_empty() {
            return Err(ConfigError::new(format!(
                "invalid configuration {}: monitored device identities must not be empty",
                path.display()
            )));
        }
        let monitored = value.as_bool().ok_or_else(|| {
            ConfigError::new(format!(
                "invalid configuration {}: monitored device entries must be booleans",
                path.display()
            ))
        })?;
        monitored_devices.insert(identity.clone(), monitored);
    }
    Ok(monitored_devices)
}

pub fn remove_legacy_monitored_devices() -> Result<(), ConfigError> {
    let Some(path) = config_path() else {
        return Ok(());
    };
    remove_legacy_monitored_devices_from(path)
}

fn remove_legacy_monitored_devices_from(path: impl AsRef<Path>) -> Result<(), ConfigError> {
    let path = path.as_ref();
    if !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(path).map_err(|error| {
        ConfigError::new(format!(
            "could not read configuration {}: {error}",
            path.display()
        ))
    })?;
    let mut document = contents.parse::<Document>().map_err(|error| {
        ConfigError::new(format!("invalid configuration {}: {error}", path.display()))
    })?;
    if document.remove("monitored_devices").is_some() {
        fs::write(path, document.to_string()).map_err(|error| {
            ConfigError::new(format!(
                "could not remove legacy monitored_devices from {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct ConfigError(String);

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ConfigError {}

fn parse_toml(contents: &str, path: &Path) -> Result<AppConfig, ConfigError> {
    let value = contents.parse::<toml::Value>().map_err(|error| {
        ConfigError::new(format!("invalid configuration {}: {error}", path.display()))
    })?;
    let table = value.as_table().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: expected a TOML table",
            path.display()
        ))
    })?;

    let mut config = AppConfig::default();
    for (key, value) in table {
        match key.as_str() {
            "low_battery_threshold" => {
                let threshold = value.as_integer().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: low_battery_threshold must be an integer",
                        path.display()
                    ))
                })?;
                if !(1..=100).contains(&threshold) {
                    return Err(ConfigError::new(format!(
                        "invalid configuration {}: low_battery_threshold must be between 1 and 100 percent",
                        path.display()
                    )));
                }
                config.low_battery_threshold = threshold as u8;
            }
            "poll_interval_seconds" => {
                config.poll_interval_seconds =
                    parse_interval(value, "poll_interval_seconds", 30, 3600, path)?;
            }
            "retry_interval_seconds" => {
                config.retry_interval_seconds =
                    parse_interval(value, "retry_interval_seconds", 5, 300, path)?;
            }
            "notifications_enabled" => {
                config.notifications_enabled = value.as_bool().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: notifications_enabled must be a boolean",
                        path.display()
                    ))
                })?;
            }
            "log_level" => {
                let level = value.as_str().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: log_level must be a string",
                        path.display()
                    ))
                })?;
                config.log_level = LogLevel::parse(level).ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: log_level must be one of error, warn, info, debug, or trace",
                        path.display()
                    ))
                })?;
            }
            "battery_color_map" => {
                config.battery_color_map =
                    parse_battery_color_map(value, config.battery_color_map, path)?;
            }
            "monitored_devices" => {}
            _ => {
                return Err(ConfigError::new(format!(
                    "invalid configuration {}: unknown setting {key:?}",
                    path.display()
                )))
            }
        }
    }

    Ok(config)
}

fn parse_battery_color_map(
    value: &toml::Value,
    mut color_map: BatteryColorMap,
    path: &Path,
) -> Result<BatteryColorMap, ConfigError> {
    let table = value.as_table().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: battery_color_map must be a table",
            path.display()
        ))
    })?;

    for (key, value) in table {
        match key.as_str() {
            "red_max_percent" => {
                color_map.red_max_percent =
                    parse_color_threshold(value, "battery_color_map.red_max_percent", 0, 99, path)?;
            }
            "yellow_max_percent" => {
                color_map.yellow_max_percent = parse_color_threshold(
                    value,
                    "battery_color_map.yellow_max_percent",
                    1,
                    100,
                    path,
                )?;
            }
            "red" => color_map.red = parse_hex_color(value, "battery_color_map.red", path)?,
            "yellow" => {
                color_map.yellow = parse_hex_color(value, "battery_color_map.yellow", path)?
            }
            "green" => color_map.green = parse_hex_color(value, "battery_color_map.green", path)?,
            _ => {
                return Err(ConfigError::new(format!(
                    "invalid configuration {}: unknown battery_color_map setting {key:?}",
                    path.display()
                )))
            }
        }
    }

    if color_map.red_max_percent >= color_map.yellow_max_percent {
        return Err(ConfigError::new(format!(
            "invalid configuration {}: battery_color_map.red_max_percent must be less than yellow_max_percent",
            path.display()
        )));
    }
    Ok(color_map)
}

fn parse_color_threshold(
    value: &toml::Value,
    key: &str,
    minimum: i64,
    maximum: i64,
    path: &Path,
) -> Result<u8, ConfigError> {
    let threshold = value.as_integer().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: {key} must be an integer",
            path.display()
        ))
    })?;
    if !(minimum..=maximum).contains(&threshold) {
        return Err(ConfigError::new(format!(
            "invalid configuration {}: {key} must be between {minimum} and {maximum} percent",
            path.display()
        )));
    }
    Ok(threshold as u8)
}

fn parse_hex_color(value: &toml::Value, key: &str, path: &Path) -> Result<[u8; 3], ConfigError> {
    let value = value.as_str().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: {key} must be a #RRGGBB string",
            path.display()
        ))
    })?;
    let hex = value.strip_prefix('#').ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: {key} must be a #RRGGBB string",
            path.display()
        ))
    })?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::new(format!(
            "invalid configuration {}: {key} must be a #RRGGBB string",
            path.display()
        )));
    }
    let channel = |start| {
        u8::from_str_radix(&hex[start..start + 2], 16).map_err(|_| {
            ConfigError::new(format!(
                "invalid configuration {}: {key} must be a #RRGGBB string",
                path.display()
            ))
        })
    };
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

fn parse_interval(
    value: &toml::Value,
    key: &str,
    minimum: i64,
    maximum: i64,
    path: &Path,
) -> Result<u64, ConfigError> {
    let seconds = value.as_integer().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: {key} must be an integer",
            path.display()
        ))
    })?;
    if !(minimum..=maximum).contains(&seconds) {
        return Err(ConfigError::new(format!(
            "invalid configuration {}: {key} must be between {minimum} and {maximum} seconds",
            path.display()
        )));
    }
    Ok(seconds as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_config_accepts_defaults_and_threshold_values() {
        let path = Path::new("config.toml");
        assert_eq!(
            parse_toml("# Volty configuration\n", path).unwrap(),
            AppConfig::default()
        );
        assert_eq!(
            parse_toml("low_battery_threshold = 15\n", path).unwrap(),
            AppConfig {
                low_battery_threshold: 15,
                ..AppConfig::default()
            }
        );
        assert_eq!(
            parse_toml(
                "low_battery_threshold = 80\npoll_interval_seconds = 300\nretry_interval_seconds = 30\nnotifications_enabled = false\nlog_level = \"debug\"\n",
                path
            )
            .unwrap(),
            AppConfig {
                low_battery_threshold: 80,
                poll_interval_seconds: 300,
                retry_interval_seconds: 30,
                notifications_enabled: false,
                log_level: LogLevel::Debug,
                battery_color_map: BatteryColorMap::default(),
            }
        );
        assert_eq!(
            parse_toml("[monitored_devices]\n\"bluetooth:test\" = false\n", path).unwrap(),
            AppConfig::default()
        );
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            assert_eq!(LogLevel::parse(level.as_str()), Some(level));
        }
        assert_eq!(LogLevel::parse("INFO"), Some(LogLevel::Info));
    }

    #[test]
    fn toml_config_accepts_custom_battery_color_map() {
        let config = parse_toml(
            "[battery_color_map]\nred_max_percent = 10\nyellow_max_percent = 40\nred = \"#010203\"\nyellow = \"#aBcDeF\"\ngreen = \"#ffffff\"\n",
            Path::new("config.toml"),
        )
        .unwrap();
        assert_eq!(
            config.battery_color_map,
            BatteryColorMap {
                red_max_percent: 10,
                yellow_max_percent: 40,
                red: [1, 2, 3],
                yellow: [171, 205, 239],
                green: [255, 255, 255],
            }
        );
        assert_eq!(config.battery_color_map.color_for(10), [1, 2, 3]);
        assert_eq!(config.battery_color_map.color_for(11), [171, 205, 239]);
        assert_eq!(config.battery_color_map.color_for(40), [171, 205, 239]);
        assert_eq!(config.battery_color_map.color_for(41), [255, 255, 255]);
    }

    #[test]
    fn toml_config_rejects_malformed_and_unsupported_settings() {
        let path = Path::new("config.toml");
        for malformed in [
            "low_battery_threshold =",
            "low_battery_threshold = \"20\"",
            "low_battery_threshold = 0",
            "low_battery_threshold = 101",
            "poll_interval_seconds = 29",
            "poll_interval_seconds = 3601",
            "poll_interval_seconds = \"120\"",
            "retry_interval_seconds = 4",
            "retry_interval_seconds = 301",
            "notifications_enabled = \"true\"",
            "log_level = 5",
            "log_level = \"verbose\"",
            "battery_color_map = \"invalid\"",
            "[battery_color_map]\nred_max_percent = -1",
            "[battery_color_map]\nred_max_percent = 100",
            "[battery_color_map]\nyellow_max_percent = 0",
            "[battery_color_map]\nred_max_percent = 30\nyellow_max_percent = 30",
            "[battery_color_map]\nred = \"red\"",
            "[battery_color_map]\nyellow = \"#12345g\"",
            "[battery_color_map]\nunknown = 3",
            "unknown_setting = 20",
            "low_battery_threshold = 20\nlow_battery_threshold = 15",
        ] {
            assert!(parse_toml(malformed, path).is_err(), "{malformed}");
        }
    }

    #[test]
    fn log_levels_filter_less_severe_events() {
        assert!(LogLevel::Info.includes(LogLevel::Error));
        assert!(LogLevel::Info.includes(LogLevel::Warn));
        assert!(LogLevel::Info.includes(LogLevel::Info));
        assert!(!LogLevel::Info.includes(LogLevel::Debug));
        assert!(!LogLevel::Info.includes(LogLevel::Trace));
        assert!(LogLevel::Trace.includes(LogLevel::Debug));
    }

    #[test]
    fn missing_config_is_created_with_all_settings_commented_out() {
        let directory = std::env::temp_dir().join(format!(
            "volty-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("config.toml");

        assert_eq!(AppConfig::load_from(&path).unwrap(), AppConfig::default());
        let created_path = path.as_path();
        let contents = fs::read_to_string(&created_path).unwrap();
        assert!(contents.lines().all(|line| line.starts_with('#')));

        fs::remove_file(&created_path).unwrap();
        assert_eq!(AppConfig::load_from(&path).unwrap(), AppConfig::default());
        let recreated_contents = fs::read_to_string(&created_path).unwrap();
        assert!(recreated_contents.lines().all(|line| line.starts_with('#')));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_monitor_preferences_are_loaded_then_removed_without_losing_comments() {
        let directory = std::env::temp_dir().join(format!(
            "volty-legacy-monitor-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("config.toml");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            &path,
            "# User comment\nlow_battery_threshold = 25\n\n[monitored_devices]\n\"bluetooth:test\" = false\n\n[battery_color_map]\nred_max_percent = 10\n",
        )
        .unwrap();

        assert_eq!(
            load_legacy_monitored_devices_from(&path).unwrap(),
            BTreeMap::from([("bluetooth:test".to_owned(), false)])
        );
        remove_legacy_monitored_devices_from(&path).unwrap();

        let reloaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(reloaded.low_battery_threshold, 25);
        assert_eq!(reloaded.battery_color_map.red_max_percent, 10);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# User comment"));
        assert!(!contents.contains("[monitored_devices]"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_legacy_monitor_preferences_are_reported() {
        let directory = std::env::temp_dir().join(format!(
            "volty-invalid-legacy-monitor-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        fs::write(&path, "[monitored_devices]\ndevice = \"false\"\n").unwrap();

        assert!(load_legacy_monitored_devices_from(&path).is_err());

        fs::remove_dir_all(directory).unwrap();
    }
}
