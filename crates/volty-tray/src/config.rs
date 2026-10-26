use std::{
    error::Error,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const DEFAULT_LOW_BATTERY_THRESHOLD: u8 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationConfig {
    pub low_battery_threshold: u8,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            low_battery_threshold: DEFAULT_LOW_BATTERY_THRESHOLD,
        }
    }
}

impl NotificationConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        Self::load_from(path)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => {
                return Err(ConfigError::new(format!(
                    "could not read configuration {}: {error}",
                    path.display()
                )))
            }
        };

        parse_toml(&contents, path)
    }

    pub fn ensure_config_file() -> Result<PathBuf, ConfigError> {
        let path = config_path()
            .ok_or_else(|| ConfigError::new("could not determine the user configuration folder"))?;
        if path.exists() {
            return Ok(path);
        }

        let contents = format!(
            "# Volty user configuration\nlow_battery_threshold = {}\n",
            Self::default().low_battery_threshold
        );
        let parent = path
            .parent()
            .ok_or_else(|| ConfigError::new("configuration path has no parent folder"))?;
        fs::create_dir_all(parent).map_err(|error| {
            ConfigError::new(format!(
                "could not create configuration folder {}: {error}",
                parent.display()
            ))
        })?;

        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(path),
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
        Ok(path)
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

fn parse_toml(contents: &str, path: &Path) -> Result<NotificationConfig, ConfigError> {
    let value = contents.parse::<toml::Value>().map_err(|error| {
        ConfigError::new(format!("invalid configuration {}: {error}", path.display()))
    })?;
    let table = value.as_table().ok_or_else(|| {
        ConfigError::new(format!(
            "invalid configuration {}: expected a TOML table",
            path.display()
        ))
    })?;

    for key in table.keys() {
        if key != "low_battery_threshold" {
            return Err(ConfigError::new(format!(
                "invalid configuration {}: unknown setting {key:?}",
                path.display()
            )));
        }
    }

    let Some(threshold) = table.get("low_battery_threshold") else {
        return Ok(NotificationConfig::default());
    };
    let threshold = threshold.as_integer().ok_or_else(|| {
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

    Ok(NotificationConfig {
        low_battery_threshold: threshold as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_config_accepts_defaults_and_threshold_values() {
        let path = Path::new("config.toml");
        assert_eq!(
            parse_toml("# Volty configuration\n", path).unwrap(),
            NotificationConfig::default()
        );
        assert_eq!(
            parse_toml("low_battery_threshold = 15\n", path).unwrap(),
            NotificationConfig {
                low_battery_threshold: 15
            }
        );
    }

    #[test]
    fn toml_config_rejects_malformed_and_unsupported_settings() {
        let path = Path::new("config.toml");
        for malformed in [
            "low_battery_threshold =",
            "low_battery_threshold = \"20\"",
            "low_battery_threshold = 0",
            "low_battery_threshold = 101",
            "unknown_setting = 20",
            "low_battery_threshold = 20\nlow_battery_threshold = 15",
        ] {
            assert!(parse_toml(malformed, path).is_err(), "{malformed}");
        }
    }
}
