use std::{
    collections::{BTreeMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{params, Connection, TransactionBehavior};

/// Version 2 keys HID state by a path-independent interface key.
const SCHEMA_VERSION: i64 = 2;

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
        if version < SCHEMA_VERSION {
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
            CREATE TABLE IF NOT EXISTS notified_discharge_cycles (
                identity TEXT PRIMARY KEY
            ) WITHOUT ROWID;",
        )?;
        if version < SCHEMA_VERSION {
            connection.execute_batch(&format!(
                "DELETE FROM monitored_devices WHERE identity LIKE 'hid:%';
                DELETE FROM notified_discharge_cycles WHERE identity LIKE 'hid:%';
                PRAGMA user_version = {SCHEMA_VERSION};"
            ))?;
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

    pub fn notified_discharge_cycles(&self) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
        let mut statement = self
            .connection
            .prepare("SELECT identity FROM notified_discharge_cycles")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<HashSet<_>, _>>()?)
    }

    pub fn set_discharge_notification_sent(
        &mut self,
        identity: &str,
        sent: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if identity.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device identity must not be empty",
            )
            .into());
        }
        if sent {
            self.connection.execute(
                "INSERT OR IGNORE INTO notified_discharge_cycles (identity) VALUES (?1)",
                [identity],
            )?;
        } else {
            self.connection.execute(
                "DELETE FROM notified_discharge_cycles WHERE identity = ?1",
                [identity],
            )?;
        }
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
            assert!(state.notified_discharge_cycles().unwrap().is_empty());
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
                    .set_discharge_notification_sent("", true)
                    .unwrap_err()
                    .to_string(),
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
    fn discharge_notification_latches_are_persisted_until_the_cycle_resets() {
        let path = std::env::temp_dir().join(format!(
            "volty-notification-state-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let identity = "bluetooth:device-a";

        {
            let mut state = DeviceState::open(&path).unwrap();
            assert!(state.notified_discharge_cycles().unwrap().is_empty());
            state
                .set_discharge_notification_sent(identity, true)
                .unwrap();
        }

        let mut state = DeviceState::open(&path).unwrap();
        assert_eq!(
            state.notified_discharge_cycles().unwrap(),
            HashSet::from([identity.to_owned()])
        );
        state
            .set_discharge_notification_sent(identity, false)
            .unwrap();
        assert!(state.notified_discharge_cycles().unwrap().is_empty());
        drop(state);
        fs::remove_file(path).unwrap();
    }
}
