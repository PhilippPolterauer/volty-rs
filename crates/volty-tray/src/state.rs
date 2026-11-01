use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// Version 4 adds persisted tray display preferences next to the per-device
/// monitoring and notification state from version 3.
const SCHEMA_VERSION: i64 = 4;

const TRAY_ICON_STYLE_KEY: &str = "tray_icon_style";
const PINNED_DEVICE_KEY: &str = "pinned_device";
const ADVANCED_OPTIONS_VISIBLE_KEY: &str = "advanced_options_visible";

/// How the tray draws its battery percentage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrayIconStyle {
    /// The battery glyph that fills according to the percentage.
    #[default]
    Battery,
    /// The percentage as digits drawn from a built-in bitmap font.
    Percentage,
}

impl TrayIconStyle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Battery => "battery",
            Self::Percentage => "percentage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "battery" => Some(Self::Battery),
            "percentage" => Some(Self::Percentage),
            _ => None,
        }
    }
}

/// Tray display preferences stored in local SQLite state, not `config.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayDisplaySettings {
    pub icon_style: TrayIconStyle,
    /// A device identity that drives the tray icon; `None` means the lowest
    /// monitored percentage.
    pub pinned_device: Option<String>,
}

/// Battery percentage points lost while Volty was alive and watching a device
/// discharge, and how long that watching took. Time Volty was not running is
/// never counted, so the ratio is the device's real drain rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DischargeRate {
    pub lost_percent: f64,
    pub alive_seconds: f64,
}

pub struct DeviceState {
    connection: Connection,
}

impl DeviceState {
    pub fn open_local() -> Result<Self, Box<dyn std::error::Error>> {
        let path = local_state_path()?;
        Self::open(&path).map_err(|error| {
            io::Error::other(format!(
                "could not open discovery state file {}: {error}",
                path.display()
            ))
            .into()
        })
    }

    fn open(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "discovery state file has no parent directory",
            )
        })?;
        fs::create_dir_all(parent)?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(1))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;",
        )?;
        if version < 2 {
            // Interface-path keys from older versions can never match a stable key again.
            connection.execute_batch("DROP TABLE IF EXISTS recent_devices;")?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS recent_devices (
                interface_key TEXT NOT NULL,
                device_index INTEGER NOT NULL
                    CHECK (device_index BETWEEN 0 AND 255),
                seen_order INTEGER NOT NULL,
                PRIMARY KEY (interface_key, device_index)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS monitored_devices (
                identity TEXT PRIMARY KEY,
                monitored INTEGER NOT NULL CHECK (monitored IN (0, 1))
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS notified_thresholds (
                identity TEXT NOT NULL,
                threshold INTEGER NOT NULL
                    CHECK (threshold BETWEEN 1 AND 100),
                PRIMARY KEY (identity, threshold)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS discharge_rates (
                identity TEXT PRIMARY KEY,
                lost_percent REAL NOT NULL CHECK (lost_percent >= 0),
                alive_seconds REAL NOT NULL CHECK (alive_seconds >= 0)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) WITHOUT ROWID;",
        )?;
        if version < 2 {
            connection.execute_batch(
                "DELETE FROM monitored_devices WHERE identity LIKE 'hid:%';
                DELETE FROM notified_thresholds WHERE identity LIKE 'hid:%';",
            )?;
        }
        if version < 3 {
            // The previous schema only stored one boolean per device; the
            // exact thresholds that already fired are unknown.
            connection.execute_batch("DROP TABLE IF EXISTS notified_discharge_cycles;")?;
        }
        if version < SCHEMA_VERSION {
            connection.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        Ok(Self { connection })
    }

    pub fn ordered_device_indices(
        &self,
        interface_key: &str,
        defaults: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut indices = self.known_device_indices(interface_key, defaults)?;
        let mut seen: HashSet<_> = indices.iter().copied().collect();
        for &index in defaults {
            if seen.insert(index) {
                indices.push(index);
            }
        }
        Ok(indices)
    }

    pub fn known_device_indices(
        &self,
        interface_key: &str,
        candidates: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut statement = self.connection.prepare(
            "SELECT device_index
             FROM recent_devices
             WHERE interface_key = ?1
             ORDER BY seen_order DESC, device_index",
        )?;
        let rows = statement.query_map([interface_key], |row| row.get::<_, u8>(0))?;
        let mut indices = Vec::with_capacity(candidates.len());
        let mut seen = HashSet::with_capacity(candidates.len());
        for row in rows {
            let index = row?;
            if candidates.contains(&index) && seen.insert(index) {
                indices.push(index);
            }
        }
        Ok(indices)
    }

    pub fn record(
        &mut self,
        interface_key: &str,
        device_index: u8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seen_order = transaction.query_row(
            "SELECT COALESCE(MAX(seen_order), 0) + 1
             FROM recent_devices
             WHERE interface_key = ?1",
            [interface_key],
            |row| row.get::<_, i64>(0),
        )?;
        transaction.execute(
            "INSERT INTO recent_devices (interface_key, device_index, seen_order)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (interface_key, device_index)
             DO UPDATE SET seen_order = excluded.seen_order",
            params![interface_key, i64::from(device_index), seen_order],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn monitored_devices(&self) -> Result<BTreeMap<String, bool>, Box<dyn std::error::Error>> {
        let mut statement = self
            .connection
            .prepare("SELECT identity, monitored FROM monitored_devices ORDER BY identity")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })?;
        let mut devices = BTreeMap::new();
        for row in rows {
            let (identity, monitored) = row?;
            devices.insert(identity, monitored);
        }
        Ok(devices)
    }

    pub fn set_device_monitored(
        &mut self,
        identity: &str,
        monitored: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if identity.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device identity must not be empty",
            )
            .into());
        }
        self.connection.execute(
            "INSERT INTO monitored_devices (identity, monitored)
             VALUES (?1, ?2)
             ON CONFLICT (identity)
             DO UPDATE SET monitored = excluded.monitored",
            params![identity, monitored],
        )?;
        Ok(())
    }

    pub fn notified_thresholds(
        &self,
    ) -> Result<HashMap<String, HashSet<u8>>, Box<dyn std::error::Error>> {
        let mut statement = self
            .connection
            .prepare("SELECT identity, threshold FROM notified_thresholds")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u8>(1)?))
        })?;
        let mut devices: HashMap<String, HashSet<u8>> = HashMap::new();
        for row in rows {
            let (identity, threshold) = row?;
            devices.entry(identity).or_default().insert(threshold);
        }
        Ok(devices)
    }

    pub fn mark_threshold_notified(
        &mut self,
        identity: &str,
        threshold: u8,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if identity.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device identity must not be empty",
            )
            .into());
        }
        if !(1..=100).contains(&threshold) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "notified battery threshold must be between 1 and 100 percent",
            )
            .into());
        }
        self.connection.execute(
            "INSERT OR IGNORE INTO notified_thresholds (identity, threshold)
             VALUES (?1, ?2)",
            params![identity, threshold],
        )?;
        Ok(())
    }

    pub fn clear_notified_thresholds(
        &mut self,
        identity: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if identity.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device identity must not be empty",
            )
            .into());
        }
        self.connection.execute(
            "DELETE FROM notified_thresholds WHERE identity = ?1",
            [identity],
        )?;
        Ok(())
    }

    pub fn discharge_rates(
        &self,
    ) -> Result<HashMap<String, DischargeRate>, Box<dyn std::error::Error>> {
        let mut statement = self
            .connection
            .prepare("SELECT identity, lost_percent, alive_seconds FROM discharge_rates")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                DischargeRate {
                    lost_percent: row.get(1)?,
                    alive_seconds: row.get(2)?,
                },
            ))
        })?;
        Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
    }

    pub fn save_discharge_rates<'a>(
        &mut self,
        rates: impl IntoIterator<Item = (&'a str, DischargeRate)>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (identity, rate) in rates {
            transaction.execute(
                "INSERT INTO discharge_rates (identity, lost_percent, alive_seconds)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (identity) DO UPDATE SET
                    lost_percent = excluded.lost_percent,
                    alive_seconds = excluded.alive_seconds",
                params![identity, rate.lost_percent, rate.alive_seconds],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn tray_display(&self) -> Result<TrayDisplaySettings, Box<dyn std::error::Error>> {
        let mut statement = self.connection.prepare("SELECT key, value FROM settings")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut settings = TrayDisplaySettings::default();
        for row in rows {
            let (key, value) = row?;
            match key.as_str() {
                TRAY_ICON_STYLE_KEY => {
                    settings.icon_style = TrayIconStyle::parse(&value).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unknown tray icon style {value:?}"),
                        )
                    })?;
                }
                PINNED_DEVICE_KEY if !value.is_empty() => {
                    settings.pinned_device = Some(value);
                }
                _ => {}
            }
        }
        Ok(settings)
    }

    pub fn set_tray_icon_style(
        &mut self,
        style: TrayIconStyle,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![TRAY_ICON_STYLE_KEY, style.as_str()],
        )?;
        Ok(())
    }

    pub fn set_pinned_device(&mut self, identity: &str) -> Result<(), Box<dyn std::error::Error>> {
        if identity.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pinned device identity must not be empty",
            )
            .into());
        }
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![PINNED_DEVICE_KEY, identity],
        )?;
        Ok(())
    }

    pub fn clear_pinned_device(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES (?1, '')
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [PINNED_DEVICE_KEY],
        )?;
        Ok(())
    }

    pub fn advanced_options_visible(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [ADVANCED_OPTIONS_VISIBLE_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match value.as_deref() {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            Some(value) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown advanced-options visibility {value:?}"),
            )
            .into()),
        }
    }

    pub fn set_advanced_options_visible(
        &mut self,
        visible: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![ADVANCED_OPTIONS_VISIBLE_KEY, visible.to_string()],
        )?;
        Ok(())
    }

    pub fn import_legacy_monitored_devices(
        &mut self,
        devices: &BTreeMap<String, bool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (identity, monitored) in devices {
            transaction.execute(
                "INSERT OR IGNORE INTO monitored_devices (identity, monitored)
                 VALUES (?1, ?2)",
                params![identity, monitored],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub fn local_state_path() -> Result<PathBuf, io::Error> {
    let local_app_data = env::var_os("LOCALAPPDATA").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not determine the local application data folder",
        )
    })?;
    Ok(PathBuf::from(local_app_data)
        .join("Volty")
        .join("device-state.sqlite3"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn recent_devices_are_prioritized_and_persisted() {
        let path = std::env::temp_dir().join(format!(
            "volty-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let defaults = [0xff, 1, 2, 3, 4, 5, 6];

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert_eq!(
                state
                    .ordered_device_indices("receiver-a", &defaults)
                    .unwrap(),
                defaults
            );
            assert!(state
                .known_device_indices("receiver-a", &defaults)
                .unwrap()
                .is_empty());
            state.record("receiver-a", 1).unwrap();
            state.record("receiver-a", 3).unwrap();
            state.record("receiver-a", 0xff).unwrap();
            state.record("receiver-b", 2).unwrap();

            assert_eq!(
                state.known_device_indices("receiver-a", &defaults).unwrap(),
                [0xff, 3, 1]
            );
            assert_eq!(
                state
                    .known_device_indices("receiver-a", &[1, 3, 3, 1, 0xff])
                    .unwrap(),
                [0xff, 3, 1]
            );
            assert_eq!(
                state
                    .ordered_device_indices("receiver-a", &defaults)
                    .unwrap(),
                [0xff, 3, 1, 2, 4, 5, 6]
            );
            state.record("receiver-a", 1).unwrap();
        }

        let state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.known_device_indices("receiver-a", &defaults).unwrap(),
            [1, 0xff, 3]
        );
        assert_eq!(
            state
                .ordered_device_indices("receiver-a", &defaults)
                .unwrap(),
            [1, 0xff, 3, 2, 4, 5, 6]
        );
        assert_eq!(
            state
                .ordered_device_indices("receiver-b", &defaults)
                .unwrap(),
            [2, 0xff, 1, 3, 4, 5, 6]
        );
        drop(state);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn opening_a_pre_stable_identity_database_drops_path_keyed_hid_state() {
        let path = std::env::temp_dir().join(format!(
            "volty-migrate-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE recent_devices (
                        interface_path TEXT NOT NULL,
                        device_index INTEGER NOT NULL,
                        seen_order INTEGER NOT NULL,
                        PRIMARY KEY (interface_path, device_index)
                    );
                    INSERT INTO recent_devices VALUES ('\\\\?\\hid#old', 1, 1);
                    CREATE TABLE monitored_devices (
                        identity TEXT PRIMARY KEY,
                        monitored INTEGER NOT NULL
                    );
                    INSERT INTO monitored_devices VALUES ('hid:1:\\\\?\\hid#old', 0);
                    INSERT INTO monitored_devices VALUES ('bluetooth:keep', 0);
                    CREATE TABLE notified_discharge_cycles (identity TEXT PRIMARY KEY);
                    INSERT INTO notified_discharge_cycles VALUES ('hid:1:\\\\?\\hid#old');",
                )
                .unwrap();
        }

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert!(state
                .known_device_indices("\\\\?\\hid#old", &[1])
                .unwrap()
                .is_empty());
            assert_eq!(
                state.monitored_devices().unwrap(),
                BTreeMap::from([("bluetooth:keep".to_owned(), false)])
            );
            assert!(state.notified_thresholds().unwrap().is_empty());
            state.record("046d:c547:2:ff00:0002:-", 1).unwrap();
            state.set_device_monitored("hid:new#1", false).unwrap();
        }

        let state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state
                .known_device_indices("046d:c547:2:ff00:0002:-", &[1])
                .unwrap(),
            [1]
        );
        assert_eq!(state.monitored_devices().unwrap().len(), 2);
        drop(state);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn monitored_devices_are_persisted_and_legacy_import_does_not_override() {
        let path = std::env::temp_dir().join(format!(
            "volty-monitor-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = BTreeMap::from([
            ("device-a".to_owned(), false),
            ("device-b".to_owned(), true),
        ]);

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert_eq!(
                state
                    .set_device_monitored("", true)
                    .unwrap_err()
                    .to_string(),
                "device identity must not be empty"
            );
            assert_eq!(
                state
                    .mark_threshold_notified("", 20)
                    .unwrap_err()
                    .to_string(),
                "device identity must not be empty"
            );
            assert_eq!(
                state
                    .mark_threshold_notified("device-a", 0)
                    .unwrap_err()
                    .to_string(),
                "notified battery threshold must be between 1 and 100 percent"
            );
            assert_eq!(
                state.clear_notified_thresholds("").unwrap_err().to_string(),
                "device identity must not be empty"
            );
            state.import_legacy_monitored_devices(&legacy).unwrap();
            state.set_device_monitored("device-a", true).unwrap();
            assert_eq!(
                state.monitored_devices().unwrap(),
                BTreeMap::from([("device-a".to_owned(), true), ("device-b".to_owned(), true),])
            );
            state.import_legacy_monitored_devices(&legacy).unwrap();
        }

        let state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.monitored_devices().unwrap(),
            BTreeMap::from([("device-a".to_owned(), true), ("device-b".to_owned(), true),])
        );
        drop(state);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn notified_thresholds_are_persisted_per_device_until_the_cycle_resets() {
        let path = std::env::temp_dir().join(format!(
            "volty-notification-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let identity = "bluetooth:device-a";
        let other_identity = "bluetooth:device-b";

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert!(state.notified_thresholds().unwrap().is_empty());
            state.mark_threshold_notified(identity, 20).unwrap();
            state.mark_threshold_notified(identity, 10).unwrap();
            state.mark_threshold_notified(identity, 10).unwrap();
            state.mark_threshold_notified(other_identity, 5).unwrap();
        }

        let mut state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.notified_thresholds().unwrap(),
            HashMap::from([
                (identity.to_owned(), HashSet::from([10, 20])),
                (other_identity.to_owned(), HashSet::from([5])),
            ])
        );
        state.clear_notified_thresholds(identity).unwrap();
        assert_eq!(
            state.notified_thresholds().unwrap(),
            HashMap::from([(other_identity.to_owned(), HashSet::from([5]))])
        );
        drop(state);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn opening_a_schema_two_database_keeps_discovery_and_monitoring_state() {
        let path = std::env::temp_dir().join(format!(
            "volty-migrate-notification-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE recent_devices (
                        interface_key TEXT NOT NULL,
                        device_index INTEGER NOT NULL,
                        seen_order INTEGER NOT NULL,
                        PRIMARY KEY (interface_key, device_index)
                    ) WITHOUT ROWID;
                    INSERT INTO recent_devices VALUES ('stable-key', 2, 1);
                    CREATE TABLE monitored_devices (
                        identity TEXT PRIMARY KEY,
                        monitored INTEGER NOT NULL
                    ) WITHOUT ROWID;
                    INSERT INTO monitored_devices VALUES ('hid:keep#1', 0);
                    CREATE TABLE notified_discharge_cycles (identity TEXT PRIMARY KEY)
                        WITHOUT ROWID;
                    INSERT INTO notified_discharge_cycles VALUES ('hid:keep#1');
                    PRAGMA user_version = 2;",
                )
                .unwrap();
        }

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert_eq!(
                state
                    .known_device_indices("stable-key", &[0, 1, 2])
                    .unwrap(),
                [2]
            );
            assert_eq!(
                state.monitored_devices().unwrap(),
                BTreeMap::from([("hid:keep#1".to_owned(), false)])
            );
            assert!(state.notified_thresholds().unwrap().is_empty());
            state.mark_threshold_notified("hid:keep#1", 10).unwrap();
        }

        let state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.notified_thresholds().unwrap(),
            HashMap::from([("hid:keep#1".to_owned(), HashSet::from([10]))])
        );
        drop(state);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn discharge_rates_are_persisted_and_updated_in_place() {
        let path = std::env::temp_dir().join(format!(
            "volty-rate-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = DischargeRate {
            lost_percent: 3.0,
            alive_seconds: 7200.0,
        };
        let second = DischargeRate {
            lost_percent: 4.0,
            alive_seconds: 9000.0,
        };

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert!(state.discharge_rates().unwrap().is_empty());
            state
                .save_discharge_rates([("hid:a#1", first), ("bluetooth:b", first)])
                .unwrap();
            state.save_discharge_rates([("hid:a#1", second)]).unwrap();
        }

        let state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.discharge_rates().unwrap(),
            HashMap::from([
                ("hid:a#1".to_owned(), second),
                ("bluetooth:b".to_owned(), first)
            ])
        );
        drop(state);
        fs::remove_file(path).unwrap();
    }

    fn fresh_state_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn fresh_database_uses_default_tray_display_settings() {
        let path = fresh_state_path("volty-fresh-tray-state");
        let state = DeviceState::open(&path).unwrap();
        assert!(!state.advanced_options_visible().unwrap());
        assert_eq!(
            state.tray_display().unwrap(),
            TrayDisplaySettings {
                icon_style: TrayIconStyle::Battery,
                pinned_device: None,
            }
        );
        let version: i64 = state
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        drop(state);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn advanced_options_visibility_is_persisted_and_validated() {
        let path = fresh_state_path("volty-advanced-options-state");

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert!(!state.advanced_options_visible().unwrap());
            state.set_advanced_options_visible(true).unwrap();
        }

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert!(state.advanced_options_visible().unwrap());
            state.set_advanced_options_visible(false).unwrap();
            state
                .connection
                .execute(
                    "INSERT INTO settings (key, value) VALUES (?1, 'sometimes')
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    [ADVANCED_OPTIONS_VISIBLE_KEY],
                )
                .unwrap();
            assert_eq!(
                state.advanced_options_visible().unwrap_err().to_string(),
                "unknown advanced-options visibility \"sometimes\""
            );
        }

        drop(DeviceState::open(&path).unwrap());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn tray_display_preferences_persist_and_validate_stored_styles() {
        let path = fresh_state_path("volty-tray-display-state");

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert_eq!(
                state.set_pinned_device("").unwrap_err().to_string(),
                "pinned device identity must not be empty"
            );
            state
                .set_tray_icon_style(TrayIconStyle::Percentage)
                .unwrap();
            state.set_pinned_device("bluetooth:headset").unwrap();
        }

        {
            let state = DeviceState::open(&path).unwrap();
            assert_eq!(
                state.tray_display().unwrap(),
                TrayDisplaySettings {
                    icon_style: TrayIconStyle::Percentage,
                    pinned_device: Some("bluetooth:headset".to_owned()),
                }
            );
        }

        {
            let mut state = DeviceState::open(&path).unwrap();
            state.clear_pinned_device().unwrap();
            assert_eq!(state.tray_display().unwrap().pinned_device, None);
            state
                .connection
                .execute(
                    "INSERT INTO settings (key, value) VALUES ('tray_icon_style', 'neon')
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    [],
                )
                .unwrap();
            assert_eq!(
                state.tray_display().unwrap_err().to_string(),
                "unknown tray icon style \"neon\""
            );
        }

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn opening_a_schema_three_database_preserves_data_and_adds_tray_defaults() {
        let path = fresh_state_path("volty-migrate-tray-state");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE recent_devices (
                        interface_key TEXT NOT NULL,
                        device_index INTEGER NOT NULL,
                        seen_order INTEGER NOT NULL,
                        PRIMARY KEY (interface_key, device_index)
                    ) WITHOUT ROWID;
                    INSERT INTO recent_devices VALUES ('stable-key', 2, 1);
                    CREATE TABLE monitored_devices (
                        identity TEXT PRIMARY KEY,
                        monitored INTEGER NOT NULL CHECK (monitored IN (0, 1))
                    ) WITHOUT ROWID;
                    INSERT INTO monitored_devices VALUES ('hid:keep#1', 0);
                    CREATE TABLE notified_thresholds (
                        identity TEXT NOT NULL,
                        threshold INTEGER NOT NULL CHECK (threshold BETWEEN 1 AND 100),
                        PRIMARY KEY (identity, threshold)
                    ) WITHOUT ROWID;
                    INSERT INTO notified_thresholds VALUES ('hid:keep#1', 10);
                    CREATE TABLE discharge_rates (
                        identity TEXT PRIMARY KEY,
                        lost_percent REAL NOT NULL CHECK (lost_percent >= 0),
                        alive_seconds REAL NOT NULL CHECK (alive_seconds >= 0)
                    ) WITHOUT ROWID;
                    INSERT INTO discharge_rates VALUES ('hid:keep#1', 1.5, 3600.0);
                    PRAGMA user_version = 3;",
                )
                .unwrap();
        }

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert_eq!(
                state
                    .known_device_indices("stable-key", &[0, 1, 2])
                    .unwrap(),
                [2]
            );
            assert_eq!(
                state.monitored_devices().unwrap(),
                BTreeMap::from([("hid:keep#1".to_owned(), false)])
            );
            assert_eq!(
                state.notified_thresholds().unwrap(),
                HashMap::from([("hid:keep#1".to_owned(), HashSet::from([10]))])
            );
            assert_eq!(
                state.discharge_rates().unwrap(),
                HashMap::from([(
                    "hid:keep#1".to_owned(),
                    DischargeRate {
                        lost_percent: 1.5,
                        alive_seconds: 3600.0,
                    }
                )])
            );
            assert_eq!(
                state.tray_display().unwrap(),
                TrayDisplaySettings::default()
            );
            let version: i64 = state
                .connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap();
            assert_eq!(version, 4);
            state
                .set_tray_icon_style(TrayIconStyle::Percentage)
                .unwrap();
            state.set_pinned_device("hid:keep#1").unwrap();
        }

        let state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.monitored_devices().unwrap(),
            BTreeMap::from([("hid:keep#1".to_owned(), false)])
        );
        assert_eq!(
            state.tray_display().unwrap(),
            TrayDisplaySettings {
                icon_style: TrayIconStyle::Percentage,
                pinned_device: Some("hid:keep#1".to_owned()),
            }
        );
        drop(state);
        fs::remove_file(path).unwrap();
    }
}
