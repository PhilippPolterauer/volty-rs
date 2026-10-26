use std::{
    error::Error,
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use volty_hid::{BatteryLevel, BatteryState, BatteryStatus};

use crate::{
    config::{config_path, NotificationConfig},
    notify::{BatterySample, NotificationEngine},
    report::generate_debug_report,
    tray::{set_start_on_boot_enabled, start_on_boot_enabled, TrayAction, TrayController},
};

const POLL_INTERVAL: Duration = Duration::from_secs(120);
const RECOVERY_INTERVAL: Duration = Duration::from_secs(15);
const UI_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);

enum PollCommand {
    Refresh,
    Quit,
}

struct DeviceReading {
    name: String,
    battery: BatteryStatus,
}

type PollResult = Result<DeviceReading, String>;

struct ConfigWatch {
    _watcher: RecommendedWatcher,
    config_file: PathBuf,
    events: Receiver<notify::Result<Event>>,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut tray = TrayController::new()?;
    tray.update_status("", "Searching for Logitech device", None)?;
    let config_watch = watch_config_file()?;

    let (mut notifications, mut notification_config_error) =
        match NotificationEngine::from_local_config() {
            Ok(engine) => (engine, None),
            Err(error) => {
                let message = error.to_string();
                eprintln!("Volty notification configuration error: {error}");
                tray.update_status("", &format!("Notification config error: {message}"), None)?;
                (NotificationEngine::new(), Some(message))
            }
        };

    let (commands_tx, commands_rx) = mpsc::channel();
    let (results_tx, results_rx) = mpsc::channel();
    thread::Builder::new()
        .name("volty-device-poller".to_owned())
        .spawn(move || poll_worker(commands_rx, results_tx))?;
    let mut config_reload_due = None;

    loop {
        if !process_windows_messages() {
            return Ok(());
        }

        if let Err(error) = tray.process_menu_events() {
            report_error(&mut tray, "Tray menu error", &error.to_string());
        }

        loop {
            match tray.try_recv_action() {
                Ok(TrayAction::RefreshNow) => {
                    reload_notification_config(
                        &mut tray,
                        &mut notifications,
                        &mut notification_config_error,
                    );
                    if commands_tx.send(PollCommand::Refresh).is_err() {
                        report_error(&mut tray, "Refresh error", "device poller has stopped");
                    }
                }
                Ok(TrayAction::OpenConfiguration) => {
                    if let Err(error) = open_configuration() {
                        report_error(&mut tray, "Configuration error", &error.to_string());
                    }
                }
                Ok(TrayAction::GenerateDebugReport) => match generate_debug_report() {
                    Ok(path) => {
                        let path_text = path.display().to_string();
                        match shell_open(path.as_os_str()) {
                            Ok(()) => {
                                if let Err(error) = tray.update_status(
                                    "",
                                    &format!("Debug report opened: {path_text}"),
                                    None,
                                ) {
                                    eprintln!("Volty tray update error: {error}");
                                }
                            }
                            Err(code) => report_error(
                                &mut tray,
                                "Debug report saved but could not be opened",
                                &format!("{path_text} (ShellExecuteW code {code})"),
                            ),
                        }
                    }
                    Err(error) => report_error(&mut tray, "Debug report error", &error.to_string()),
                },
                Ok(TrayAction::ToggleStartOnBoot) => match toggle_start_on_boot() {
                    Ok(enabled) => tray.set_start_on_boot_checked(enabled),
                    Err(error) => {
                        report_error(&mut tray, "Start on Boot error", &error.to_string())
                    }
                },
                Ok(TrayAction::OpenGitHub) => {
                    if let Err(error) = open_github() {
                        report_error(&mut tray, "GitHub error", &error.to_string());
                    }
                }
                Ok(TrayAction::Quit) => {
                    let _ = commands_tx.send(PollCommand::Quit);
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    report_error(&mut tray, "Tray error", "tray action channel disconnected");
                    break;
                }
            }
        }

        while let Ok(event) = config_watch.events.try_recv() {
            match event {
                Ok(event)
                    if event
                        .paths
                        .iter()
                        .any(|path| path == &config_watch.config_file) =>
                {
                    config_reload_due = Some(Instant::now() + CONFIG_RELOAD_DEBOUNCE);
                }
                Ok(_) => {}
                Err(error) => {
                    report_error(&mut tray, "Configuration watch error", &error.to_string())
                }
            }
        }

        if config_reload_due.is_some_and(|due| Instant::now() >= due) {
            config_reload_due = None;
            if reload_notification_config(
                &mut tray,
                &mut notifications,
                &mut notification_config_error,
            ) && commands_tx.send(PollCommand::Refresh).is_err()
            {
                report_error(&mut tray, "Refresh error", "device poller has stopped");
            }
        }

        // Match explicitly so an unexpected worker disconnect is not confused
        // with an empty result queue.
        #[allow(clippy::while_let_loop)]
        loop {
            match results_rx.try_recv() {
                Ok(result) => apply_poll_result(
                    &mut tray,
                    &mut notifications,
                    result,
                    notification_config_error.as_deref(),
                ),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("device polling worker stopped unexpectedly".into());
                }
            }
        }

        thread::sleep(UI_POLL_INTERVAL);
    }
}

fn watch_config_file() -> Result<ConfigWatch, Box<dyn Error>> {
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
    })?;
    watcher.watch(config_dir, RecursiveMode::NonRecursive)?;

    Ok(ConfigWatch {
        _watcher: watcher,
        config_file,
        events: events_rx,
    })
}

fn reload_notification_config(
    tray: &mut TrayController,
    notifications: &mut NotificationEngine,
    current_error: &mut Option<String>,
) -> bool {
    let config = match NotificationConfig::load() {
        Ok(config) => config,
        Err(error) => {
            let message = error.to_string();
            if current_error.as_deref() != Some(&message) {
                report_error(tray, "Configuration error", &message);
                *current_error = Some(message);
            }
            return false;
        }
    };

    if let Err(error) = notifications.set_threshold(config.low_battery_threshold) {
        let message = error.to_string();
        if current_error.as_deref() != Some(&message) {
            report_error(tray, "Configuration error", &message);
            *current_error = Some(message);
        }
        return false;
    }

    let recovered = current_error.take().is_some();
    if recovered {
        eprintln!("Volty configuration reloaded successfully");
    }
    recovered
}

fn process_windows_messages() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, WM_QUIT,
    };

    let mut message: MSG = unsafe { std::mem::zeroed() };
    while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        if message.message == WM_QUIT {
            return false;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}

fn open_configuration() -> Result<(), Box<dyn Error>> {
    let path = NotificationConfig::ensure_config_file()?;
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

fn open_github() -> Result<(), Box<dyn Error>> {
    const GITHUB_URL: &str = "https://github.com/PhilippPolterauer/volty-rs";
    shell_open(std::ffi::OsStr::new(GITHUB_URL)).map_err(|code| {
        format!("could not open GitHub page {GITHUB_URL} (ShellExecuteW code {code})").into()
    })
}

fn shell_open(target: &std::ffi::OsStr) -> Result<(), isize> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let target: Vec<u16> = target.encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        Err(result)
    } else {
        Ok(())
    }
}

fn toggle_start_on_boot() -> Result<bool, Box<dyn Error>> {
    let enabled = !start_on_boot_enabled()?;
    set_start_on_boot_enabled(enabled)?;
    Ok(enabled)
}

fn poll_worker(commands: mpsc::Receiver<PollCommand>, results: mpsc::Sender<PollResult>) {
    loop {
        let reading = query_devices();
        let next_poll = if reading.is_ok() {
            POLL_INTERVAL
        } else {
            RECOVERY_INTERVAL
        };
        if results.send(reading).is_err() {
            return;
        }

        match commands.recv_timeout(next_poll) {
            Ok(PollCommand::Refresh) => {}
            Ok(PollCommand::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn query_devices() -> PollResult {
    use volty_hid::{enumerate_logitech_devices, query_battery};

    let devices = enumerate_logitech_devices().map_err(|error| error.to_string())?;
    if devices.is_empty() {
        return Err("No Logitech HID device found".to_owned());
    }

    let mut errors = Vec::new();
    for device in devices {
        let name = device
            .product_string
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Logitech device {:04x}", device.product_id));

        // The HID++ API accepts a paired-device index but doesn't enumerate
        // receiver pairings. Probe the common device slots so receivers work too.
        for device_index in [0xff, 1, 2, 3, 4, 5, 6] {
            match query_battery(&device, device_index) {
                Ok(battery) => return Ok(DeviceReading { name, battery }),
                Err(error) => errors.push(format!("{name} (slot {device_index}): {error}")),
            }
        }
    }

    Err(format!(
        "Could not read a Logitech battery: {}",
        errors.join("; ")
    ))
}

fn apply_poll_result(
    tray: &mut TrayController,
    notifications: &mut NotificationEngine,
    result: PollResult,
    notification_config_error: Option<&str>,
) {
    let reading = match result {
        Ok(reading) => reading,
        Err(error) => {
            eprintln!("Volty device polling error: {error}");
            let mut status = format!("Error: {error}");
            append_config_error(&mut status, notification_config_error);
            if let Err(tray_error) = tray.update_status("", &status, None) {
                eprintln!("Volty tray update error: {tray_error}");
            }
            return;
        }
    };

    let percentage = reading.battery.level_percent;
    let mut status = battery_status_label(&reading.battery);
    let charging = matches!(
        reading.battery.state,
        BatteryState::Charging | BatteryState::ChargingNearlyFull | BatteryState::ChargingSlow
    );
    let full = matches!(reading.battery.state, BatteryState::Charged)
        || reading.battery.level == BatteryLevel::Full
        || percentage.is_some_and(|percentage| percentage >= 100);
    if percentage.is_some() || charging || full {
        let sample = BatterySample {
            // Percentage is irrelevant while charging/full because the engine
            // resets its discharge-cycle guard before checking the threshold.
            percentage: percentage.unwrap_or(0),
            charging,
            full,
        };
        if let Err(error) = notifications.update(sample) {
            let error = error.to_string();
            eprintln!("Volty notification error: {error}");
            status.push_str("; notification error: ");
            status.push_str(&error);
        }
    }
    if percentage.is_none() {
        status.push_str("; percentage unavailable");
    }
    append_config_error(&mut status, notification_config_error);

    if let Err(error) = tray.update_status(&reading.name, &status, percentage) {
        eprintln!("Volty tray update error: {error}");
    }
}

fn append_config_error(status: &mut String, error: Option<&str>) {
    if let Some(error) = error {
        status.push_str("; notification config error: ");
        status.push_str(error);
    }
}

fn battery_status_label(battery: &BatteryStatus) -> String {
    let state = match battery.state {
        BatteryState::Unknown => "Unknown charge state",
        BatteryState::Discharging => "Discharging",
        BatteryState::Charging => "Charging",
        BatteryState::ChargingNearlyFull => "Charging — nearly full",
        BatteryState::Charged => "Fully charged",
        BatteryState::ChargingSlow => "Charging slowly",
        BatteryState::InvalidBattery => "Battery error: invalid battery",
        BatteryState::ThermalError => "Battery error: thermal",
        BatteryState::ChargingError => "Battery error: charging",
    };
    let level = match battery.level {
        BatteryLevel::Unknown => None,
        BatteryLevel::Critical => Some("critical level"),
        BatteryLevel::Low => Some("low level"),
        BatteryLevel::Good => Some("good level"),
        BatteryLevel::Full => Some("full"),
    };
    match level {
        Some(level) => format!("{state} ({level})"),
        None => state.to_owned(),
    }
}

fn report_error(tray: &mut TrayController, title: &str, message: &str) {
    eprintln!("Volty {title}: {message}");
    if let Err(error) = tray.update_status("", &format!("{title}: {message}"), None) {
        eprintln!("Volty tray update error: {error}");
    }
}
