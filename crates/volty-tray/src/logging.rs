use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::config::LogLevel;

const DEFAULT_MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

pub struct FileLogger {
    file: Option<File>,
    size: u64,
    path: PathBuf,
    level: LogLevel,
    max_file_size: u64,
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

        Self::open_at(directory.join("volty.log"), level, DEFAULT_MAX_FILE_SIZE).map(
            |mut logger| {
                logger.set_level(level);
                logger
            },
        )
    }

    fn open_at(path: impl Into<PathBuf>, level: LogLevel, max_file_size: u64) -> io::Result<Self> {
        let path = path.into();
        let file = open_append(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not open log file {}: {error}", path.display()),
            )
        })?;
        let size = file.metadata()?.len();

        Ok(Self {
            file: Some(file),
            size,
            path,
            level,
            max_file_size,
        })
    }

    pub fn log(&mut self, level: LogLevel, message: &str) -> io::Result<()> {
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

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
        volty_battery::set_trace_enabled(level.includes(LogLevel::Trace));
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_the_injected_limit_and_replaces_the_previous_backup() {
        let directory = std::env::temp_dir().join(format!(
            "volty-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("volty.log");
        let mut backup_path = path.as_os_str().to_os_string();
        backup_path.push(".1");
        let backup_path = PathBuf::from(backup_path);
        let mut logger = FileLogger::open_at(&path, LogLevel::Trace, 1024).unwrap();
        let first_entry = "a".repeat(700);
        let second_entry = "b".repeat(700);
        let third_entry = "c".repeat(700);

        logger.log(LogLevel::Trace, &first_entry).unwrap();
        assert!(!backup_path.exists());
        assert!(fs::read_to_string(&path).unwrap().contains(&first_entry));

        logger.log(LogLevel::Trace, &second_entry).unwrap();
        let backup = fs::read_to_string(&backup_path).unwrap();
        assert!(backup.contains(&first_entry));
        assert!(!backup.contains(&second_entry));
        assert!(fs::read_to_string(&path).unwrap().contains(&second_entry));

        logger.log(LogLevel::Trace, &third_entry).unwrap();
        let backup = fs::read_to_string(&backup_path).unwrap();
        assert!(backup.contains(&second_entry));
        assert!(!backup.contains(&first_entry));
        assert!(!backup.contains(&third_entry));
        assert!(fs::read_to_string(&path).unwrap().contains(&third_entry));

        drop(logger);
        fs::remove_dir_all(directory).unwrap();
    }
}
