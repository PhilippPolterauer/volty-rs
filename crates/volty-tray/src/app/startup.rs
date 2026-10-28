use crate::{
    config::{
        config_path, load_legacy_monitored_devices, remove_legacy_monitored_devices, AppConfig,
    },
    state::DeviceState,
    tray::{set_start_on_boot_enabled, start_on_boot_enabled},
};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeMap,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
};

pub(super) struct StartupState {
    pub(super) monitor_state: Option<DeviceState>,
    pub(super) monitored_devices: BTreeMap<String, bool>,
    pub(super) active_config: AppConfig,
    pub(super) notification_config_error: Option<String>,
    pub(super) warnings: Vec<String>,
}

pub(super) fn load_startup_state() -> StartupState {
    let mut warnings = Vec::new();
    let (legacy_monitored_devices, legacy_settings_available) =
        match load_legacy_monitored_devices() {
            Ok(devices) => (devices, true),
            Err(error) => {
                warnings.push(format!(
                    "Could not read legacy monitored-device preferences: {error}"
                ));
                (BTreeMap::new(), false)
            }
        };
    let mut monitor_state = match DeviceState::open_local() {
        Ok(state) => Some(state),
        Err(error) => {
            warnings.push(format!(
                "Could not open local monitoring state; device choices cannot be changed this run: {error}"
            ));
            None
        }
    };
    if let Some(state) = monitor_state.as_mut() {
        if legacy_settings_available {
            match state.import_legacy_monitored_devices(&legacy_monitored_devices) {
                Ok(()) => {
                    if let Err(error) = remove_legacy_monitored_devices() {
                        warnings.push(format!(
                            "Could not remove migrated monitoring preferences from config: {error}"
                        ));
                    }
                }
                Err(error) => warnings.push(format!(
                    "Could not migrate monitored-device preferences to local state: {error}"
                )),
            }
        }
    }
    let monitored_devices = match monitor_state.as_ref() {
        Some(state) => match state.monitored_devices() {
            Ok(devices) => devices,
            Err(error) => {
                warnings.push(format!(
                    "Could not load local monitored-device preferences; using legacy settings for this run: {error}"
                ));
                legacy_monitored_devices
            }
        },
        None => legacy_monitored_devices,
    };
    let (active_config, notification_config_error) = match AppConfig::load() {
        Ok(config) => (config, None),
        Err(error) => (AppConfig::default(), Some(error.to_string())),
    };

    StartupState {
        monitor_state,
        monitored_devices,
        active_config,
        notification_config_error,
        warnings,
    }
}
pub(super) struct ConfigWatch {
    _watcher: RecommendedWatcher,
    pub(super) config_file: PathBuf,
    pub(super) events: Receiver<notify::Result<Event>>,
}
pub(super) fn watch_config_file() -> Result<ConfigWatch, Box<dyn Error>> {
    let config_file = config_path().ok_or("could not determine the user configuration folder")?;
    let config_dir = config_file
        .parent()
        .ok_or("configuration path has no parent folder")?;
    std::fs::create_dir_all(config_dir).map_err(|error| {
        format!(
            "could not create configuration folder {}: {error}",
            config_dir.display()
        )
    })?;

    let (events_tx, events_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        if events_tx.send(event).is_err() {
            eprintln!("Volty configuration watcher event receiver has stopped");
        }
        super::wake::signal();
    })?;
    watcher.watch(config_dir, RecursiveMode::NonRecursive)?;

    Ok(ConfigWatch {
        _watcher: watcher,
        config_file,
        events: events_rx,
    })
}
pub(super) fn process_windows_messages() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
    };

    // SAFETY: MSG is a plain Win32 output structure where every bit pattern is valid.
    let mut message: MSG = unsafe { std::mem::zeroed() };
    // SAFETY: PeekMessageW writes to the valid local MSG; null HWND requests this thread's queue.
    while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        if message.message == WM_QUIT {
            return false;
        }
        // SAFETY: PeekMessageW initialized this MSG before the nonzero result.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}
pub(super) fn open_configuration() -> Result<(), Box<dyn Error>> {
    let path = AppConfig::ensure_config_file()?;
    if let Err(code) = shell_open(path.as_os_str()) {
        std::process::Command::new("notepad.exe")
            .arg(&path)
            .spawn()
            .map_err(|error| {
                format!(
                    "could not open configuration {} with its associated editor (ShellExecuteW code {code}) or Notepad: {error}",
                    path.display()
                )
            })?;
    }

    Ok(())
}
pub(super) fn open_log_file(path: &Path) -> Result<(), Box<dyn Error>> {
    if let Err(code) = shell_open(path.as_os_str()) {
        std::process::Command::new("notepad.exe")
            .arg(path)
            .spawn()
            .map_err(|error| {
                format!(
                    "could not open log file {} with its associated application (ShellExecuteW code {code}) or Notepad: {error}",
                    path.display()
                )
            })?;
    }
    Ok(())
}
pub(super) fn shell_open(target: &std::ffi::OsStr) -> Result<(), isize> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = target.encode_wide().chain(Some(0)).collect();
    // SAFETY: verb and target are NUL-terminated buffers that remain live through ShellExecuteW.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    }
    .expose_provenance()
    .cast_signed();
    if result <= 32 {
        Err(result)
    } else {
        Ok(())
    }
}
pub(super) fn remove_reset_files(
    configuration_file: &Path,
    local_state_file: &Path,
) -> io::Result<()> {
    for (description, path) in [
        ("local device state", local_state_file),
        ("configuration", configuration_file),
    ] {
        if let Err(error) = fs::remove_file(path) {
            if error.kind() != io::ErrorKind::NotFound {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "could not delete Volty {description} file {}: {error}",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}
pub(super) fn toggle_start_on_boot() -> Result<bool, Box<dyn Error>> {
    let enabled = !start_on_boot_enabled()?;
    set_start_on_boot_enabled(enabled)?;
    Ok(enabled)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_removes_configuration_and_local_state_files() {
        let directory = std::env::temp_dir().join(format!(
            "volty-reset-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let configuration_file = directory.join("config.toml");
        let local_state_file = directory.join("device-state.sqlite3");
        fs::write(&configuration_file, "test config").unwrap();
        fs::write(&local_state_file, "test state").unwrap();

        remove_reset_files(&configuration_file, &local_state_file).unwrap();
        assert!(!configuration_file.exists());
        assert!(!local_state_file.exists());
        remove_reset_files(&configuration_file, &local_state_file).unwrap();

        fs::remove_dir_all(directory).unwrap();
    }
}
