use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};

use crate::config::LogLevel;

const DEFAULT_MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;
const LOG_RETENTION_DAYS: u64 = 7;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

pub struct FileLogger {
    file: Option<File>,
    size: u64,
    path: PathBuf,
    directory: PathBuf,
    date: String,
    level: LogLevel,
    max_file_size: u64,
    retention_days: u64,
}

impl FileLogger {
    pub fn open(level: LogLevel) -> io::Result<Self> {
        let app_data = std::env::var_os("APPDATA").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine the user application data folder",
            )
        })?;
        let directory = PathBuf::from(app_data).join("Volty");
        fs::create_dir_all(&directory).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "could not create log folder {}: {error}",
                    directory.display()
                ),
            )
        })?;

        let date = local_date();
        let mut logger = Self::open_at(
            directory,
            &date,
            level,
            DEFAULT_MAX_FILE_SIZE,
            LOG_RETENTION_DAYS,
        )?;
        logger.set_level(level);
        Ok(logger)
    }

    fn open_at(
        directory: impl Into<PathBuf>,
        date: &str,
        level: LogLevel,
        max_file_size: u64,
        retention_days: u64,
    ) -> io::Result<Self> {
        let directory = directory.into();
        let path = daily_log_path(&directory, date);
        migrate_legacy_log(&directory, &path);
        let file = open_append(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not open log file {}: {error}", path.display()),
            )
        })?;
        let size = file.metadata()?.len();
        let logger = Self {
            file: Some(file),
            size,
            path,
            directory,
            date: date.to_owned(),
            level,
            max_file_size,
            retention_days,
        };
        logger.prune_old_logs();
        Ok(logger)
    }

    pub fn log(&mut self, level: LogLevel, message: &str) -> io::Result<()> {
        self.log_on_date(level, message, &local_date())
    }

    fn log_on_date(&mut self, level: LogLevel, message: &str, date: &str) -> io::Result<()> {
        if date != self.date {
            self.switch_date(date)?;
        }
        if !self.level.includes(level) {
            return Ok(());
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| io::Error::other(format!("system clock error: {error}")))?;
        let entry = format!(
            "{}.{:03} [{}] {message}\n",
            timestamp.as_secs(),
            timestamp.subsec_millis(),
            level.as_str()
        );
        let entry_len = entry.len() as u64;
        if self.size > 0 && self.size.saturating_add(entry_len) > self.max_file_size {
            self.rotate()?;
        }

        {
            let file = self
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("log file is unavailable"))?;
            file.write_all(entry.as_bytes())?;
            file.flush()?;
        }
        self.size = self.size.saturating_add(entry_len);
        if self.size > self.max_file_size {
            self.rotate()?;
        }
        Ok(())
    }

    fn switch_date(&mut self, date: &str) -> io::Result<()> {
        let path = daily_log_path(&self.directory, date);
        let file = open_append(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not open log file {}: {error}", path.display()),
            )
        })?;
        let size = file.metadata()?.len();
        if let Some(current) = self.file.as_mut() {
            current.flush()?;
        }
        self.file = Some(file);
        self.size = size;
        self.path = path;
        self.date = date.to_owned();
        self.prune_old_logs();
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is unavailable"))?
            .flush()?;
        drop(self.file.take());

        let mut rotated_path = self.path.as_os_str().to_os_string();
        rotated_path.push(".1");
        let rotated_path = PathBuf::from(rotated_path);

        if let Err(error) = fs::remove_file(&rotated_path) {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(self.reopen_after_rotation_failure(error));
            }
        }
        if let Err(error) = fs::rename(&self.path, &rotated_path) {
            return Err(self.reopen_after_rotation_failure(error));
        }

        match open_append(&self.path) {
            Ok(file) => {
                self.size = 0;
                self.file = Some(file);
                Ok(())
            }
            Err(error) => Err(self.reopen_after_rotation_failure(error)),
        }
    }

    fn reopen_after_rotation_failure(&mut self, error: io::Error) -> io::Error {
        match open_append(&self.path) {
            Ok(file) => {
                self.size = file.metadata().map_or(0, |metadata| metadata.len());
                self.file = Some(file);
                error
            }
            Err(reopen_error) => io::Error::new(
                error.kind(),
                format!(
                    "{error}; could not reopen log file {}: {reopen_error}",
                    self.path.display()
                ),
            ),
        }
    }

    fn prune_old_logs(&self) {
        let Some(cutoff) = SystemTime::now()
            .checked_sub(Duration::from_secs(self.retention_days * SECONDS_PER_DAY))
        else {
            return;
        };
        let Ok(entries) = fs::read_dir(&self.directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == self.path {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_daily_log_name(name) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let Ok(modified) = metadata.modified() else {
                continue;
            };
            if modified < cutoff {
                let _ = fs::remove_file(path);
            }
        }
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
        volty_battery::set_trace_enabled(level.includes(LogLevel::Trace));
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn daily_log_path(directory: &Path, date: &str) -> PathBuf {
    directory.join(format!("volty-{date}.log"))
}

fn is_daily_log_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_suffix(".log.1")
        .or_else(|| name.strip_suffix(".log"))
    else {
        return false;
    };
    let Some(date) = stem.strip_prefix("volty-") else {
        return false;
    };
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

fn local_date() -> String {
    // SAFETY: GetLocalTime only writes the current local time into the provided SYSTEMTIME,
    // which is initialized and valid for the duration of the call.
    let time = unsafe {
        let mut time = std::mem::zeroed::<SYSTEMTIME>();
        GetLocalTime(&mut time);
        time
    };
    format!("{:04}-{:02}-{:02}", time.wYear, time.wMonth, time.wDay)
}

fn migrate_legacy_log(directory: &Path, daily_path: &Path) {
    let legacy_path = directory.join("volty.log");
    if !daily_path.exists() && legacy_path.exists() {
        let _ = fs::rename(legacy_path, daily_path);
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "volty-log-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn rotates_at_the_injected_limit_and_replaces_the_previous_backup() {
        let directory = test_directory("rotation");
        let date = "2026-10-30";
        let path = daily_log_path(&directory, date);
        let mut backup_path = path.as_os_str().to_os_string();
        backup_path.push(".1");
        let backup_path = PathBuf::from(backup_path);
        let mut logger = FileLogger::open_at(&directory, date, LogLevel::Trace, 1024, 7).unwrap();
        let first_entry = "a".repeat(700);
        let second_entry = "b".repeat(700);
        let third_entry = "c".repeat(700);

        logger
            .log_on_date(LogLevel::Trace, &first_entry, date)
            .unwrap();
        assert!(!backup_path.exists());
        assert!(fs::read_to_string(&path).unwrap().contains(&first_entry));

        logger
            .log_on_date(LogLevel::Trace, &second_entry, date)
            .unwrap();
        let backup = fs::read_to_string(&backup_path).unwrap();
        assert!(backup.contains(&first_entry));
        assert!(!backup.contains(&second_entry));
        assert!(fs::read_to_string(&path).unwrap().contains(&second_entry));

        logger
            .log_on_date(LogLevel::Trace, &third_entry, date)
            .unwrap();
        let backup = fs::read_to_string(&backup_path).unwrap();
        assert!(backup.contains(&second_entry));
        assert!(!backup.contains(&first_entry));
        assert!(!backup.contains(&third_entry));
        assert!(fs::read_to_string(&path).unwrap().contains(&third_entry));

        drop(logger);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn writes_one_file_per_local_day() {
        let directory = test_directory("daily");
        let mut logger =
            FileLogger::open_at(&directory, "2026-10-30", LogLevel::Trace, 1024, 7).unwrap();

        logger
            .log_on_date(LogLevel::Trace, "first day", "2026-10-30")
            .unwrap();
        logger
            .log_on_date(LogLevel::Trace, "second day", "2026-10-31")
            .unwrap();

        let first = fs::read_to_string(daily_log_path(&directory, "2026-10-30")).unwrap();
        let second = fs::read_to_string(daily_log_path(&directory, "2026-10-31")).unwrap();
        assert!(first.contains("first day"));
        assert!(!first.contains("second day"));
        assert!(second.contains("second day"));
        assert_eq!(logger.path(), daily_log_path(&directory, "2026-10-31"));

        drop(logger);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn prunes_daily_logs_older_than_the_retention_window() {
        let directory = test_directory("prune");
        let old_path = daily_log_path(&directory, "2026-10-01");
        fs::write(&old_path, "old").unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(8 * SECONDS_PER_DAY);
        File::options()
            .write(true)
            .open(&old_path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();

        let recent_path = daily_log_path(&directory, "2026-10-28");
        fs::write(&recent_path, "recent").unwrap();
        let recent_time = SystemTime::now() - Duration::from_secs(2 * SECONDS_PER_DAY);
        File::options()
            .write(true)
            .open(&recent_path)
            .unwrap()
            .set_modified(recent_time)
            .unwrap();

        let unrelated_path = directory.join("notes.log");
        fs::write(&unrelated_path, "keep").unwrap();

        let logger =
            FileLogger::open_at(&directory, "2026-10-30", LogLevel::Trace, 1024, 7).unwrap();
        assert!(!old_path.exists());
        assert!(recent_path.exists());
        assert!(unrelated_path.exists());

        drop(logger);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn migrates_the_legacy_log_into_the_daily_file() {
        let directory = test_directory("migration");
        let legacy_path = directory.join("volty.log");
        fs::write(&legacy_path, "legacy entry\n").unwrap();

        let logger =
            FileLogger::open_at(&directory, "2026-10-30", LogLevel::Trace, 1024, 7).unwrap();
        assert!(!legacy_path.exists());
        let daily = fs::read_to_string(daily_log_path(&directory, "2026-10-30")).unwrap();
        assert!(daily.contains("legacy entry"));

        drop(logger);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recognizes_only_dated_daily_log_names() {
        assert!(is_daily_log_name("volty-2026-10-30.log"));
        assert!(is_daily_log_name("volty-2026-10-30.log.1"));
        assert!(!is_daily_log_name("volty.log"));
        assert!(!is_daily_log_name("volty-backup.log"));
        assert!(!is_daily_log_name("volty-2026-10-30.txt"));
    }
}
