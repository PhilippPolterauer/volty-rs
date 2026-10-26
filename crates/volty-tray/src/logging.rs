use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::config::LogLevel;

pub struct FileLogger {
    file: File,
    path: PathBuf,
    level: LogLevel,
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

        let path = directory.join("volty.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("could not open log file {}: {error}", path.display()),
                )
            })?;

        Ok(Self { file, path, level })
    }

    pub fn log(&mut self, level: LogLevel, message: &str) -> io::Result<()> {
        if !self.level.includes(level) {
            return Ok(());
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| io::Error::other(format!("system clock error: {error}")))?;
        writeln!(
            self.file,
            "{}.{:03} [{}] {message}",
            timestamp.as_secs(),
            timestamp.subsec_millis(),
            level.as_str()
        )?;
        self.file.flush()
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
