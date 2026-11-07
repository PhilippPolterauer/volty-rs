use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use toml_edit::DocumentMut;

use crate::state::{TrayIconStyle, TrayIconTheme};

pub const DEFAULT_LOW_BATTERY_THRESHOLDS: &[u8] = &[20, 10, 5];
pub const DEFAULT_LOW_BATTERY_COOLDOWN_MINUTES: u64 = 120;
pub const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 120;
pub const DEFAULT_RETRY_INTERVAL_SECONDS: u64 = 15;
pub const DEFAULT_BATTERY_ETA_WINDOW_MINUTES: u64 = 10;
pub const DEFAULT_STALE_AFTER_MINUTES: u64 = 15;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AppTheme {
    #[default]
    System,
    Light,
    Dark,
}

impl AppTheme {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DashboardSettingsUpdate {
    pub app_theme: String,
    pub battery_color_red_max_percent: u8,
    pub battery_color_yellow_max_percent: u8,
    pub battery_color_red: String,
    pub battery_color_yellow: String,
    pub battery_color_green: String,
    pub low_battery_thresholds: Vec<u8>,
    pub low_battery_cooldown_minutes: u64,
    pub notifications_enabled: bool,
    pub tray_icon_style: String,
    pub tray_icon_theme: String,
    pub show_percentage_in_battery_icon: bool,
    pub pinned_device: Option<String>,
    pub start_on_boot: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryColorMap {
    pub red_max_percent: u8,
    pub yellow_max_percent: u8,
    pub red: [u8; 3],
    pub yellow: [u8; 3],
    pub green: [u8; 3],
}

impl BatteryColorMap {
    pub fn color_for(self, percentage: u8) -> [u8; 3] {
        let percentage = percentage.min(100);
        if percentage <= self.red_max_percent {
            self.red
        } else if percentage <= self.yellow_max_percent {
            interpolate_color(
                self.red,
                self.yellow,
                u16::from(percentage - self.red_max_percent),
                u16::from(self.yellow_max_percent - self.red_max_percent),
            )
        } else {
            interpolate_color(
                self.yellow,
                self.green,
                u16::from(percentage - self.yellow_max_percent),
                u16::from(100 - self.yellow_max_percent),
            )
        }
    }
}

fn interpolate_color(start: [u8; 3], end: [u8; 3], position: u16, span: u16) -> [u8; 3] {
    let channel = |from: u8, to: u8| {
        let from = u16::from(from);
        let to = u16::from(to);
        ((from * (span - position) + to * position + span / 2) / span) as u8
    };
    [
        channel(start[0], end[0]),
        channel(start[1], end[1]),
        channel(start[2], end[2]),
    ]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub app_theme: AppTheme,
    pub low_battery_thresholds: Vec<u8>,
    pub low_battery_cooldown_minutes: u64,
    pub poll_interval_seconds: u64,
    pub retry_interval_seconds: u64,
    pub battery_eta_window_minutes: u64,
    pub stale_after_minutes: u64,
    pub notifications_enabled: bool,
    pub log_level: LogLevel,
    pub battery_color_map: BatteryColorMap,
    pub tray_icon_style: Option<TrayIconStyle>,
    pub tray_icon_theme: Option<TrayIconTheme>,
    pub show_percentage_in_battery_icon: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            low_battery_thresholds: DEFAULT_LOW_BATTERY_THRESHOLDS.to_vec(),
            low_battery_cooldown_minutes: DEFAULT_LOW_BATTERY_COOLDOWN_MINUTES,
            poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
            retry_interval_seconds: DEFAULT_RETRY_INTERVAL_SECONDS,
            battery_eta_window_minutes: DEFAULT_BATTERY_ETA_WINDOW_MINUTES,
            stale_after_minutes: DEFAULT_STALE_AFTER_MINUTES,
            notifications_enabled: true,
            log_level: LogLevel::Info,
            battery_color_map: BatteryColorMap::default(),
            tray_icon_style: None,
            tray_icon_theme: None,
            app_theme: AppTheme::System,
            show_percentage_in_battery_icon: false,
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
        if config_file_exists(path)? {
            return Ok(path.to_path_buf());
        }

        let defaults = Self::default();
        let contents = format!(
            "# Volty user configuration\n\
            # Battery percentages that trigger an alert, highest first (each 1-100).\n\
             # low_battery_thresholds = {:?}\n\
             # Minutes before repeating an alert at or below the lowest threshold; 0 disables repeats.\n\
             # low_battery_cooldown_minutes = {}\n\
             # Seconds between successful battery readings (10-3600).\n\
             # poll_interval_seconds = {}\n\
             # Seconds between retries after a polling error (5-300).\n\
             # retry_interval_seconds = {}\n\
             # Minutes of discharge history used to estimate battery runtime (1-180).\n\
             # battery_eta_window_minutes = {}\n\
             # Minutes without a new reading before a device is marked stale (1-1440).\n\
             # stale_after_minutes = {}\n\
             # Set to false to disable low-battery toast notifications.\n\
             # notifications_enabled = {}\n\
             # error, warn, info, debug, or trace.\n\
             # log_level = \"{}\"\n\
             # Tray icon style: \"battery\" or \"percentage\". Overrides the saved tray choice.\n\
             # tray_icon_style = \"battery\"\n\
             # Tray icon theme: \"system\", \"light\", or \"dark\". Overrides the saved tray choice.\n\
             # tray_icon_theme = \"system\"\n\
             # Draw the percentage over the battery icon when its style is \"battery\".\n\
             # show_percentage_in_battery_icon = false\n\
             # Uncomment this table to override the default tray battery colors.\n\
             # Red stays solid through red_max_percent; yellow_max_percent is the yellow blend point.\n\
             # Colors blend red to yellow, then yellow to green at 100%; red must be below yellow.\n\
             # [battery_color_map]\n\
             # red_max_percent = 15\n\
             # yellow_max_percent = 30\n\
             # red = \"#d23030\"\n\
             # yellow = \"#ebbe23\"\n\
             # green = \"#2daa4b\"\n\
             # Per-device monitoring choices are stored in local SQLite state.\n",
            defaults.low_battery_thresholds,
            defaults.low_battery_cooldown_minutes,
            defaults.poll_interval_seconds,
            defaults.retry_interval_seconds,
            defaults.battery_eta_window_minutes,
            defaults.stale_after_minutes,
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

/// Validates and normalizes configured low-battery thresholds: at least one
/// value, each 1-100 percent, sorted from highest to lowest without duplicates.
pub fn normalize_low_battery_thresholds(thresholds: &[u8]) -> Result<Vec<u8>, ConfigError> {
    if thresholds.is_empty() {
        return Err(ConfigError::new(
            "low_battery_thresholds must contain at least one threshold",
        ));
    }
    if thresholds
        .iter()
        .any(|threshold| !(1..=100).contains(threshold))
    {
        return Err(ConfigError::new(
            "low_battery_thresholds entries must be between 1 and 100 percent",
        ));
    }
    let mut thresholds = thresholds.to_vec();
    thresholds.sort_unstable_by(|left, right| right.cmp(left));
    thresholds.dedup();
    Ok(thresholds)
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

fn config_file_exists(path: &Path) -> Result<bool, ConfigError> {
    path.try_exists().map_err(|error| {
        ConfigError::new(format!(
            "could not inspect configuration {}: {error}",
            path.display()
        ))
    })
}

fn load_legacy_monitored_devices_from(
    path: impl AsRef<Path>,
) -> Result<BTreeMap<String, bool>, ConfigError> {
    let path = path.as_ref();
    if !config_file_exists(path)? {
        return Ok(BTreeMap::new());
    }
    let contents = fs::read_to_string(path).map_err(|error| {
        ConfigError::new(format!(
            "could not read configuration {}: {error}",
            path.display()
        ))
    })?;
    let document = toml::from_str::<toml::Value>(&contents).map_err(|error| {
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

pub fn update_dashboard_settings(settings: &DashboardSettingsUpdate) -> Result<(), ConfigError> {
    let path = config_path()
        .ok_or_else(|| ConfigError::new("could not determine the user configuration folder"))?;
    update_dashboard_settings_at(path, settings)
}

fn update_dashboard_settings_at(
    path: impl AsRef<Path>,
    settings: &DashboardSettingsUpdate,
) -> Result<(), ConfigError> {
    let thresholds = normalize_low_battery_thresholds(&settings.low_battery_thresholds)?;
    let cooldown = i64::try_from(settings.low_battery_cooldown_minutes).map_err(|_| {
        ConfigError::new("low_battery_cooldown_minutes must not exceed 9223372036854775807")
    })?;
    let color_map = BatteryColorMap {
        red_max_percent: settings.battery_color_red_max_percent,
        yellow_max_percent: settings.battery_color_yellow_max_percent,
        red: parse_dashboard_hex_color(&settings.battery_color_red, "red")?,
        yellow: parse_dashboard_hex_color(&settings.battery_color_yellow, "yellow")?,
        green: parse_dashboard_hex_color(&settings.battery_color_green, "green")?,
    };
    if color_map.red_max_percent > 99
        || color_map.yellow_max_percent == 0
        || color_map.yellow_max_percent > 100
        || color_map.red_max_percent >= color_map.yellow_max_percent
    {
        return Err(ConfigError::new(
            "battery color cutoffs must satisfy 0 ≤ red < yellow ≤ 100",
        ));
    }
    let app_theme = AppTheme::parse(&settings.app_theme)
        .ok_or_else(|| ConfigError::new("app_theme must be system, light, or dark"))?;
    let style = TrayIconStyle::parse(&settings.tray_icon_style)
        .ok_or_else(|| ConfigError::new("tray_icon_style must be battery or percentage"))?;
    let theme = TrayIconTheme::parse(&settings.tray_icon_theme)
        .ok_or_else(|| ConfigError::new("tray_icon_theme must be system, light, or dark"))?;
    update_config_document(path, |document| {
        let mut values = toml_edit::Array::new();
        for threshold in &thresholds {
            values.push(i64::from(*threshold));
        }
        document["low_battery_thresholds"] = toml_edit::value(values);
        document["low_battery_cooldown_minutes"] = toml_edit::value(cooldown);
        document["notifications_enabled"] = toml_edit::value(settings.notifications_enabled);
        document["tray_icon_style"] = toml_edit::value(style.as_str());
        document["tray_icon_theme"] = toml_edit::value(theme.as_str());
        document["app_theme"] = toml_edit::value(app_theme.as_str());
        let table = document
            .entry("battery_color_map")
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .expect("validated battery color map is a table");
        table["red_max_percent"] = toml_edit::value(i64::from(color_map.red_max_percent));
        table["yellow_max_percent"] = toml_edit::value(i64::from(color_map.yellow_max_percent));
        for (name, color) in [
            ("red", color_map.red),
            ("yellow", color_map.yellow),
            ("green", color_map.green),
        ] {
            table[name] =
                toml_edit::value(format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]));
        }
        document["show_percentage_in_battery_icon"] =
            toml_edit::value(settings.show_percentage_in_battery_icon);
    })
}

fn parse_dashboard_hex_color(value: &str, name: &str) -> Result<[u8; 3], ConfigError> {
    if value.len() != 7 || !value.starts_with('#') {
        return Err(ConfigError::new(format!(
            "battery {name} must use #RRGGBB format"
        )));
    }
    let channel = |range| {
        u8::from_str_radix(&value[range], 16)
            .map_err(|_| ConfigError::new(format!("battery {name} must use #RRGGBB format")))
    };
    Ok([channel(1..3)?, channel(3..5)?, channel(5..7)?])
}

fn update_config_document(
    path: impl AsRef<Path>,
    update: impl FnOnce(&mut DocumentMut),
) -> Result<(), ConfigError> {
    let path = AppConfig::ensure_config_file_at(path)?;
    let contents = fs::read_to_string(&path).map_err(|error| {
        ConfigError::new(format!(
            "could not read configuration {}: {error}",
            path.display()
        ))
    })?;
    parse_toml(&contents, &path)?;
    let mut document = contents.parse::<DocumentMut>().map_err(|error| {
        ConfigError::new(format!("invalid configuration {}: {error}", path.display()))
    })?;
    update(&mut document);
    let updated = document.to_string();
    parse_toml(&updated, &path)?;
    fs::write(&path, updated).map_err(|error| {
        ConfigError::new(format!(
            "could not update configuration {}: {error}",
            path.display()
        ))
    })
}

fn remove_legacy_monitored_devices_from(path: impl AsRef<Path>) -> Result<(), ConfigError> {
    let path = path.as_ref();
    if !config_file_exists(path)? {
        return Ok(());
    }
    let contents = fs::read_to_string(path).map_err(|error| {
        ConfigError::new(format!(
            "could not read configuration {}: {error}",
            path.display()
        ))
    })?;
    let mut document = contents.parse::<DocumentMut>().map_err(|error| {
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
    let value = toml::from_str::<toml::Value>(contents).map_err(|error| {
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
            "low_battery_thresholds" => {
                let values = value.as_array().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: low_battery_thresholds must be an array of integers",
                        path.display()
                    ))
                })?;
                let mut thresholds = Vec::with_capacity(values.len());
                for value in values {
                    let threshold = value.as_integer().ok_or_else(|| {
                        ConfigError::new(format!(
                            "invalid configuration {}: low_battery_thresholds entries must be integers",
                            path.display()
                        ))
                    })?;
                    if !(1..=100).contains(&threshold) {
                        return Err(ConfigError::new(format!(
                            "invalid configuration {}: low_battery_thresholds entries must be between 1 and 100 percent",
                            path.display()
                        )));
                    }
                    thresholds.push(threshold as u8);
                }
                config.low_battery_thresholds = normalize_low_battery_thresholds(&thresholds)
                    .map_err(|error| {
                        ConfigError::new(format!(
                            "invalid configuration {}: {error}",
                            path.display()
                        ))
                    })?;
            }
            "low_battery_cooldown_minutes" => {
                let minutes = value.as_integer().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: low_battery_cooldown_minutes must be an integer",
                        path.display()
                    ))
                })?;
                if minutes < 0 {
                    return Err(ConfigError::new(format!(
                        "invalid configuration {}: low_battery_cooldown_minutes must be zero or greater",
                        path.display()
                    )));
                }
                config.low_battery_cooldown_minutes = minutes as u64;
            }
            "low_battery_threshold" => {
                return Err(ConfigError::new(format!(
                    "invalid configuration {}: low_battery_threshold has been replaced by low_battery_thresholds (for example low_battery_thresholds = [20, 10, 5])",
                    path.display()
                )))
            }
            "poll_interval_seconds" => {
                config.poll_interval_seconds =
                    parse_interval(value, "poll_interval_seconds", 10, 3600, "seconds", path)?;
            }
            "retry_interval_seconds" => {
                config.retry_interval_seconds =
                    parse_interval(value, "retry_interval_seconds", 5, 300, "seconds", path)?;
            }
            "battery_eta_window_minutes" => {
                config.battery_eta_window_minutes =
                    parse_interval(value, "battery_eta_window_minutes", 1, 180, "minutes", path)?;
            }
            "stale_after_minutes" => {
                config.stale_after_minutes =
                    parse_interval(value, "stale_after_minutes", 1, 1440, "minutes", path)?;
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
            "tray_icon_style" => {
                let style = value.as_str().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: tray_icon_style must be a string",
                        path.display()
                    ))
                })?;
                config.tray_icon_style = Some(TrayIconStyle::parse(style).ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: tray_icon_style must be \"battery\" or \"percentage\"",
                        path.display()
                    ))
                })?);
            }
            "app_theme" => {
                config.app_theme = value.as_str().and_then(AppTheme::parse)
                    .ok_or_else(|| ConfigError::new("app_theme must be system, light, or dark"))?;
            }
            "tray_icon_theme" => {
                let theme = value.as_str().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: tray_icon_theme must be a string",
                        path.display()
                    ))
                })?;
                config.tray_icon_theme = Some(TrayIconTheme::parse(theme).ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: tray_icon_theme must be \"system\", \"light\", or \"dark\"",
                        path.display()
                    ))
                })?);
            }
            "show_percentage_in_battery_icon" => {
                config.show_percentage_in_battery_icon = value.as_bool().ok_or_else(|| {
                    ConfigError::new(format!(
                        "invalid configuration {}: show_percentage_in_battery_icon must be a boolean",
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
    unit: &str,
    path: &Path,
) -> Result<u64, ConfigError> {
    let interval = value.as_integer().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: {key} must be an integer",
            path.display()
        ))
    })?;
    if !(minimum..=maximum).contains(&interval) {
        return Err(ConfigError::new(format!(
            "invalid configuration {}: {key} must be between {minimum} and {maximum} {unit}",
            path.display()
        )));
    }
    Ok(interval as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_theme_defaults_to_system_and_validates_config_values() {
        let path = Path::new("config.toml");
        assert_eq!(parse_toml("", path).unwrap().app_theme, AppTheme::System);
        for (value, expected) in [
            ("system", AppTheme::System),
            ("light", AppTheme::Light),
            ("dark", AppTheme::Dark),
        ] {
            assert_eq!(
                parse_toml(&format!("app_theme = {value:?}"), path)
                    .unwrap()
                    .app_theme,
                expected
            );
        }
        for invalid in ["app_theme = 1", "app_theme = \"blue\""] {
            assert!(parse_toml(invalid, path).is_err());
        }
    }

    #[test]
    fn toml_config_accepts_defaults_and_threshold_values() {
        let path = Path::new("config.toml");
        assert_eq!(AppConfig::default().battery_eta_window_minutes, 10);
        assert_eq!(AppConfig::default().stale_after_minutes, 15);
        assert_eq!(AppConfig::default().low_battery_thresholds, [20, 10, 5]);
        assert_eq!(AppConfig::default().low_battery_cooldown_minutes, 120);
        assert_eq!(
            parse_toml("# Volty configuration\n", path).unwrap(),
            AppConfig::default()
        );
        assert_eq!(
            parse_toml("low_battery_thresholds = [15]\n", path).unwrap(),
            AppConfig {
                low_battery_thresholds: vec![15],
                ..AppConfig::default()
            }
        );
        assert_eq!(
            parse_toml(
                "low_battery_thresholds = [5, 20, 10, 20]\nlow_battery_cooldown_minutes = 0\n",
                path
            )
            .unwrap(),
            AppConfig {
                low_battery_thresholds: vec![20, 10, 5],
                low_battery_cooldown_minutes: 0,
                ..AppConfig::default()
            }
        );
        assert_eq!(
            parse_toml(
                "low_battery_thresholds = [80]\nlow_battery_cooldown_minutes = 240\npoll_interval_seconds = 300\nretry_interval_seconds = 30\nbattery_eta_window_minutes = 20\nstale_after_minutes = 30\nnotifications_enabled = false\nlog_level = \"debug\"\n",
                path
            )
            .unwrap(),
            AppConfig {
                low_battery_thresholds: vec![80],
                low_battery_cooldown_minutes: 240,
                poll_interval_seconds: 300,
                retry_interval_seconds: 30,
                battery_eta_window_minutes: 20,
                stale_after_minutes: 30,
                notifications_enabled: false,
                log_level: LogLevel::Debug,
                battery_color_map: BatteryColorMap::default(),
                app_theme: AppTheme::System,
                tray_icon_style: None,
                tray_icon_theme: None,
                show_percentage_in_battery_icon: false,
            }
        );
        assert_eq!(
            parse_toml("[monitored_devices]\n\"bluetooth:test\" = false\n", path).unwrap(),
            AppConfig::default()
        );
        assert_eq!(
            parse_toml("poll_interval_seconds = 10\n", path).unwrap(),
            AppConfig {
                poll_interval_seconds: 10,
                ..AppConfig::default()
            }
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
    fn toml_config_parses_stale_after_minutes() {
        let path = Path::new("config.toml");
        assert_eq!(
            parse_toml("stale_after_minutes = 45\n", path)
                .unwrap()
                .stale_after_minutes,
            45
        );
        assert_eq!(
            parse_toml("stale_after_minutes = 1\n", path)
                .unwrap()
                .stale_after_minutes,
            1
        );
        assert_eq!(
            parse_toml("stale_after_minutes = 1440\n", path)
                .unwrap()
                .stale_after_minutes,
            1440
        );
        for invalid in ["stale_after_minutes = 0", "stale_after_minutes = 1441"] {
            assert!(parse_toml(invalid, path).is_err(), "{invalid}");
        }
        assert!(parse_toml("stale_after_minutes = \"15\"\n", path).is_err());
    }

    #[test]
    fn toml_config_accepts_tray_icon_settings() {
        let config = parse_toml(
            "tray_icon_style = \"percentage\"\ntray_icon_theme = \"dark\"\nshow_percentage_in_battery_icon = true\n",
            Path::new("config.toml"),
        )
        .unwrap();
        assert_eq!(config.tray_icon_style, Some(TrayIconStyle::Percentage));
        assert_eq!(config.tray_icon_theme, Some(TrayIconTheme::Dark));
        assert_eq!(config.app_theme, AppTheme::System);
        assert!(config.show_percentage_in_battery_icon);

        let config = parse_toml(
            "tray_icon_style = \"battery\"\ntray_icon_theme = \"light\"\n",
            Path::new("config.toml"),
        )
        .unwrap();
        assert_eq!(config.tray_icon_style, Some(TrayIconStyle::Battery));
        assert_eq!(config.tray_icon_theme, Some(TrayIconTheme::Light));
        assert!(!config.show_percentage_in_battery_icon);
        assert_eq!(
            parse_toml("tray_icon_theme = \"system\"\n", Path::new("config.toml"))
                .unwrap()
                .tray_icon_theme,
            Some(TrayIconTheme::System)
        );
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
        assert_eq!(config.battery_color_map.color_for(11), [7, 9, 11]);
        assert_eq!(config.battery_color_map.color_for(25), [86, 104, 121]);
        assert_eq!(config.battery_color_map.color_for(40), [171, 205, 239]);
        assert_eq!(config.battery_color_map.color_for(41), [172, 206, 239]);
        assert_eq!(config.battery_color_map.color_for(100), [255, 255, 255]);
    }

    #[test]
    fn toml_config_rejects_malformed_and_unsupported_settings() {
        let path = Path::new("config.toml");
        for malformed in [
            "low_battery_thresholds =",
            "low_battery_thresholds = \"20\"",
            "low_battery_thresholds = []",
            "low_battery_thresholds = [0]",
            "low_battery_thresholds = [101]",
            "low_battery_thresholds = [20, \"10\"]",
            "low_battery_thresholds = [20, -1]",
            "low_battery_cooldown_minutes = -1",
            "low_battery_cooldown_minutes = \"120\"",
            "low_battery_threshold = 20",
            "poll_interval_seconds = 9",
            "poll_interval_seconds = 3601",
            "poll_interval_seconds = \"120\"",
            "retry_interval_seconds = 4",
            "retry_interval_seconds = 301",
            "battery_eta_window_minutes = 0",
            "battery_eta_window_minutes = 181",
            "battery_eta_window_minutes = \"10\"",
            "battery_eta_window_minutes = 0",
            "battery_eta_window_minutes = 181",
            "battery_eta_window_minutes = \"10\"",
            "stale_after_minutes = 0",
            "stale_after_minutes = 1441",
            "stale_after_minutes = \"15\"",
            "notifications_enabled = \"true\"",
            "log_level = 5",
            "log_level = \"verbose\"",
            "tray_icon_style = 5",
            "tray_icon_style = \"digits\"",
            "tray_icon_theme = 5",
            "tray_icon_theme = \"sepia\"",
            "show_percentage_in_battery_icon = \"true\"",
            "battery_color_map = \"invalid\"",
            "[battery_color_map]\nred_max_percent = -1",
            "[battery_color_map]\nred_max_percent = 100",
            "[battery_color_map]\nyellow_max_percent = 0",
            "[battery_color_map]\nred_max_percent = 30\nyellow_max_percent = 30",
            "[battery_color_map]\nred = \"red\"",
            "[battery_color_map]\nyellow = \"#12345g\"",
            "[battery_color_map]\nunknown = 3",
            "unknown_setting = 20",
            "low_battery_thresholds = [20]\nlow_battery_thresholds = [15]",
        ] {
            assert!(parse_toml(malformed, path).is_err(), "{malformed}");
        }
        let old_key_error = parse_toml("low_battery_threshold = 20\n", path).unwrap_err();
        assert!(
            old_key_error
                .to_string()
                .contains("replaced by low_battery_thresholds"),
            "{old_key_error}"
        );
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
        let contents = fs::read_to_string(created_path).unwrap();
        assert!(contents.lines().all(|line| line.starts_with('#')));
        assert!(contents.contains("# low_battery_thresholds = [20, 10, 5]"));
        assert!(contents.contains("# low_battery_cooldown_minutes = 120"));
        assert!(contents.contains("# stale_after_minutes = 15"));

        fs::remove_file(created_path).unwrap();
        assert_eq!(AppConfig::load_from(&path).unwrap(), AppConfig::default());
        let recreated_contents = fs::read_to_string(created_path).unwrap();
        assert!(recreated_contents.lines().all(|line| line.starts_with('#')));
        assert!(recreated_contents.contains("# low_battery_thresholds = [20, 10, 5]"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dashboard_settings_update_persists_values_and_preserves_unrelated_config() {
        let directory = std::env::temp_dir().join(format!(
            "volty-dashboard-settings-test-{}-{}",
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
            "# Keep this comment\npoll_interval_seconds = 300\n\n[battery_color_map]\nred_max_percent = 12\n",
        )
        .unwrap();

        update_dashboard_settings_at(
            &path,
            &DashboardSettingsUpdate {
                app_theme: "dark".to_owned(),
                battery_color_red_max_percent: 15,
                battery_color_yellow_max_percent: 30,
                battery_color_red: "#D23030".to_owned(),
                battery_color_yellow: "#EBBE23".to_owned(),
                battery_color_green: "#2DAA4B".to_owned(),
                low_battery_thresholds: vec![5, 15, 15],
                low_battery_cooldown_minutes: 45,
                notifications_enabled: false,
                tray_icon_style: "percentage".to_owned(),
                tray_icon_theme: "dark".to_owned(),
                show_percentage_in_battery_icon: true,
                pinned_device: Some("hid:mouse".to_owned()),
                start_on_boot: Some(true),
            },
        )
        .unwrap();

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(config.low_battery_thresholds, [15, 5]);
        assert_eq!(config.low_battery_cooldown_minutes, 45);
        assert!(!config.notifications_enabled);
        assert_eq!(config.tray_icon_style, Some(TrayIconStyle::Percentage));
        assert_eq!(config.tray_icon_theme, Some(TrayIconTheme::Dark));
        assert_eq!(config.app_theme, AppTheme::Dark);
        assert!(config.show_percentage_in_battery_icon);
        assert_eq!(config.poll_interval_seconds, 300);
        assert_eq!(config.battery_color_map.red_max_percent, 15);
        assert_eq!(config.battery_color_map.yellow_max_percent, 30);
        assert_eq!(config.battery_color_map.red, [210, 48, 48]);
        assert_eq!(config.battery_color_map.yellow, [235, 190, 35]);
        assert_eq!(config.battery_color_map.green, [45, 170, 75]);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("# Keep this comment"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_dashboard_settings_do_not_change_config() {
        let directory = std::env::temp_dir().join(format!(
            "volty-invalid-dashboard-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("config.toml");
        fs::create_dir_all(&directory).unwrap();
        let original = "# Keep this comment\npoll_interval_seconds = 300\n";
        fs::write(&path, original).unwrap();

        let result = update_dashboard_settings_at(
            &path,
            &DashboardSettingsUpdate {
                app_theme: "dark".to_owned(),
                battery_color_red_max_percent: 15,
                battery_color_yellow_max_percent: 30,
                battery_color_red: "#D23030".to_owned(),
                battery_color_yellow: "#EBBE23".to_owned(),
                battery_color_green: "#2DAA4B".to_owned(),
                low_battery_thresholds: vec![0],
                low_battery_cooldown_minutes: 45,
                notifications_enabled: true,
                tray_icon_style: "battery".to_owned(),
                tray_icon_theme: "system".to_owned(),
                show_percentage_in_battery_icon: false,
                pinned_device: None,
                start_on_boot: None,
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
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
            "# User comment\nlow_battery_thresholds = [25, 15]\nlow_battery_cooldown_minutes = 60\n\n[monitored_devices]\n\"bluetooth:test\" = false\n\n[battery_color_map]\nred_max_percent = 10\n",
        )
        .unwrap();

        assert_eq!(
            load_legacy_monitored_devices_from(&path).unwrap(),
            BTreeMap::from([("bluetooth:test".to_owned(), false)])
        );
        remove_legacy_monitored_devices_from(&path).unwrap();

        let reloaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(reloaded.low_battery_thresholds, [25, 15]);
        assert_eq!(reloaded.low_battery_cooldown_minutes, 60);
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
