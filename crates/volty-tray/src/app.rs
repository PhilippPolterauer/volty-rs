use std::{
    collections::VecDeque,
    error::Error,
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use volty_hid::{BatteryLevel, BatteryState, BatteryStatus};

use crate::{
    about::show_about_window,
    config::{config_path, AppConfig, LogLevel},
    diagnose::show_diagnose_window,
    logging::FileLogger,
    notify::{BatterySample, NotificationEngine},
    report::generate_diagnostic_report,
    tray::{set_start_on_boot_enabled, start_on_boot_enabled, TrayAction, TrayController},
};

const UI_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const MAX_DIAGNOSTIC_LOG_ENTRIES: usize = 200;

enum PollCommand {
    Refresh,
    UpdateIntervals { poll: Duration, retry: Duration },
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
    let started = Instant::now();
    let mut diagnostic_log = VecDeque::new();
    let (mut active_config, mut notification_config_error) = match AppConfig::load() {
        Ok(config) => (config, None),
        Err(error) => (AppConfig::default(), Some(error.to_string())),
    };
    let mut file_logger = FileLogger::open(active_config.log_level)?;
    record_event(
        &mut diagnostic_log,
        &mut file_logger,
        started,
        LogLevel::Info,
        "Volty started",
    )?;
    let mut tray = TrayController::new()?;
    tray.set_battery_color_map(active_config.battery_color_map)?;
    tray.update_status("", "Searching for supported mouse", None)?;
    let config_watch = watch_config_file()?;

    if let Some(message) = notification_config_error.as_deref() {
        record_event(
            &mut diagnostic_log,
            &mut file_logger,
            started,
            LogLevel::Error,
            &format!("Configuration error: {message}"),
        )?;
        tray.update_status("", &format!("Configuration error: {message}"), None)?;
    }
    let mut notifications = NotificationEngine::from_config(active_config)?;

    let (commands_tx, commands_rx) = mpsc::channel();
    let (results_tx, results_rx) = mpsc::channel();
    let poll_interval = Duration::from_secs(active_config.poll_interval_seconds);
    let retry_interval = Duration::from_secs(active_config.retry_interval_seconds);
    thread::Builder::new()
        .name("volty-device-poller".to_owned())
        .spawn(move || poll_worker(commands_rx, results_tx, poll_interval, retry_interval))?;
    let mut config_reload_due = None;

    loop {
        if !process_windows_messages() {
            record_event(
                &mut diagnostic_log,
                &mut file_logger,
                started,
                LogLevel::Info,
                "Windows requested application shutdown",
            )?;
            return Ok(());
        }

        if let Err(error) = tray.process_menu_events() {
            report_error(
                &mut tray,
                &mut diagnostic_log,
                &mut file_logger,
                started,
                "Tray menu error",
                &error.to_string(),
            )?;
        }

        loop {
            match tray.try_recv_action() {
                Ok(TrayAction::RefreshNow) => {
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Debug,
                        "Manual battery refresh requested",
                    )?;
                    reload_notification_config(
                        &mut tray,
                        &mut notifications,
                        &mut notification_config_error,
                        &mut active_config,
                        &commands_tx,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                    )?;
                    if commands_tx.send(PollCommand::Refresh).is_err() {
                        report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "Refresh error",
                            "device poller has stopped",
                        )?;
                    }
                }
                Ok(TrayAction::OpenLogFile) => {
                    let path = file_logger.path().to_owned();
                    match open_log_file(&path) {
                        Ok(()) => record_event(
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            LogLevel::Info,
                            &format!("Opened log file: {}", path.display()),
                        )?,
                        Err(error) => report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "Log file error",
                            &error.to_string(),
                        )?,
                    }
                }
                Ok(TrayAction::OpenConfiguration) => {
                    if let Err(error) = open_configuration() {
                        report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "Configuration error",
                            &error.to_string(),
                        )?;
                    }
                }
                Ok(TrayAction::Diagnose) => {
                    match generate_diagnostic_report(active_config, &diagnostic_log) {
                        Ok((path, report)) => {
                            let path_text = path.display().to_string();
                            match show_diagnose_window(&report) {
                                Ok(()) => {
                                    record_event(
                                        &mut diagnostic_log,
                                        &mut file_logger,
                                        started,
                                        LogLevel::Info,
                                        &format!("Diagnostic report saved: {path_text}"),
                                    )?;
                                    if let Err(error) = tray.update_status(
                                        "",
                                        &format!(
                                            "Diagnose window closed; report saved: {path_text}"
                                        ),
                                        None,
                                    ) {
                                        record_event(
                                            &mut diagnostic_log,
                                            &mut file_logger,
                                            started,
                                            LogLevel::Error,
                                            &format!("Tray update error: {error}"),
                                        )?;
                                    }
                                }
                                Err(error) => report_error(
                                    &mut tray,
                                    &mut diagnostic_log,
                                    &mut file_logger,
                                    started,
                                    "Diagnose window error",
                                    &format!("{error}; report saved at {path_text}"),
                                )?,
                            }
                        }
                        Err(error) => report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "Diagnostic report error",
                            &error.to_string(),
                        )?,
                    }
                }
                Ok(TrayAction::ToggleStartOnBoot) => match toggle_start_on_boot() {
                    Ok(enabled) => tray.set_start_on_boot_checked(enabled),
                    Err(error) => report_error(
                        &mut tray,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        "Start on Boot error",
                        &error.to_string(),
                    )?,
                },
                Ok(TrayAction::About) => {
                    if let Err(error) = show_about_window() {
                        report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "About window error",
                            &error.to_string(),
                        )?;
                    }
                }
                Ok(TrayAction::Quit) => {
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Info,
                        "Volty shutting down",
                    )?;
                    let _ = commands_tx.send(PollCommand::Quit);
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    report_error(
                        &mut tray,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        "Tray error",
                        "tray action channel disconnected",
                    )?;
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
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Trace,
                        "Configuration file change detected",
                    )?;
                    config_reload_due = Some(Instant::now() + CONFIG_RELOAD_DEBOUNCE);
                }
                Ok(_) => {}
                Err(error) => report_error(
                    &mut tray,
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    "Configuration watch error",
                    &error.to_string(),
                )?,
            }
        }

        if config_reload_due.is_some_and(|due| Instant::now() >= due) {
            config_reload_due = None;
            if reload_notification_config(
                &mut tray,
                &mut notifications,
                &mut notification_config_error,
                &mut active_config,
                &commands_tx,
                &mut diagnostic_log,
                &mut file_logger,
                started,
            )? && commands_tx.send(PollCommand::Refresh).is_err()
            {
                report_error(
                    &mut tray,
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    "Refresh error",
                    "device poller has stopped",
                )?;
            }
        }

        // Match explicitly so an unexpected worker disconnect is not confused
        // with an empty result queue.
        #[allow(clippy::while_let_loop)]
        loop {
            match results_rx.try_recv() {
                Ok(result) => {
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Trace,
                        "Received device polling result",
                    )?;
                    apply_poll_result(
                        &mut tray,
                        &mut notifications,
                        result,
                        notification_config_error.as_deref(),
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                    )?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Error,
                        "Device polling worker stopped unexpectedly",
                    )?;
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
    active_config: &mut AppConfig,
    commands: &mpsc::Sender<PollCommand>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> Result<bool, io::Error> {
    let config = match AppConfig::load() {
        Ok(config) => config,
        Err(error) => {
            let message = error.to_string();
            if current_error.as_deref() != Some(&message) {
                report_error(
                    tray,
                    diagnostic_log,
                    file_logger,
                    started,
                    "Configuration error",
                    &message,
                )?;
                *current_error = Some(message);
            }
            return Ok(false);
        }
    };

    if let Err(error) = tray.set_battery_color_map(config.battery_color_map) {
        let message = error.to_string();
        if current_error.as_deref() != Some(&message) {
            report_error(
                tray,
                diagnostic_log,
                file_logger,
                started,
                "Configuration error",
                &message,
            )?;
            *current_error = Some(message);
        }
        return Ok(false);
    }

    if let Err(error) = notifications.set_config(config) {
        let message = error.to_string();
        if current_error.as_deref() != Some(&message) {
            report_error(
                tray,
                diagnostic_log,
                file_logger,
                started,
                "Configuration error",
                &message,
            )?;
            *current_error = Some(message);
        }
        return Ok(false);
    }

    let intervals_changed = config.poll_interval_seconds != active_config.poll_interval_seconds
        || config.retry_interval_seconds != active_config.retry_interval_seconds;
    if intervals_changed
        && commands
            .send(PollCommand::UpdateIntervals {
                poll: Duration::from_secs(config.poll_interval_seconds),
                retry: Duration::from_secs(config.retry_interval_seconds),
            })
            .is_err()
    {
        report_error(
            tray,
            diagnostic_log,
            file_logger,
            started,
            "Configuration error",
            "device poller has stopped; polling intervals were not applied",
        )?;
        return Ok(false);
    }
    let config_changed = *active_config != config;
    *active_config = config;
    file_logger.set_level(config.log_level);
    let recovered = current_error.take().is_some();
    if recovered || config_changed {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Info,
            "Configuration reloaded successfully",
        )?;
    }
    Ok(recovered || config_changed)
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

fn open_log_file(path: &Path) -> Result<(), Box<dyn Error>> {
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

fn poll_worker(
    commands: mpsc::Receiver<PollCommand>,
    results: mpsc::Sender<PollResult>,
    mut poll_interval: Duration,
    mut retry_interval: Duration,
) {
    loop {
        let reading = query_devices();
        let successful_read = reading.is_ok();
        let next_poll = if successful_read {
            poll_interval
        } else {
            retry_interval
        };
        if results.send(reading).is_err() {
            return;
        }

        let mut wait_interval = next_poll;
        loop {
            match commands.recv_timeout(wait_interval) {
                Ok(PollCommand::Refresh) | Err(RecvTimeoutError::Timeout) => break,
                Ok(PollCommand::UpdateIntervals { poll, retry }) => {
                    poll_interval = poll;
                    retry_interval = retry;
                    wait_interval = if successful_read { poll } else { retry };
                }
                Ok(PollCommand::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn query_devices() -> PollResult {
    use volty_hid::{enumerate_hid_interfaces, resolve_mouse_model};

    let interfaces = enumerate_hid_interfaces().map_err(|error| error.to_string())?;

    let mut errors = Vec::new();
    let mut supported_interfaces = 0;
    for interface in interfaces {
        let Some(mouse) = resolve_mouse_model(&interface).map_err(|error| {
            format!(
                "Ambiguous backend match for HID interface {:04x}:{:04x}: {error}",
                interface.vendor_id, interface.product_id
            )
        })?
        else {
            continue;
        };
        supported_interfaces += 1;
        let protocol = mouse.protocol();
        let name = interface
            .product_string
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| mouse.name().to_owned());

        for &device_index in protocol.device_indices() {
            match protocol.query_battery(&interface, device_index) {
                Ok(battery) => return Ok(DeviceReading { name, battery }),
                Err(error) => errors.push(format!(
                    "{name} via {} (slot {device_index}): {error}",
                    protocol.name()
                )),
            }
        }
    }

    if supported_interfaces == 0 {
        return Err("No supported mouse found".to_owned());
    }

    Err(format!(
        "Could not read a supported mouse battery: {}",
        errors.join("; ")
    ))
}

fn apply_poll_result(
    tray: &mut TrayController,
    notifications: &mut NotificationEngine,
    result: PollResult,
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let reading = match result {
        Ok(reading) => reading,
        Err(error) => {
            record_event(
                diagnostic_log,
                file_logger,
                started,
                LogLevel::Warn,
                &format!("Device polling error: {error}"),
            )?;
            let mut status = format!("Error: {error}");
            append_config_error(&mut status, notification_config_error);
            if let Err(tray_error) = tray.update_status("", &status, None) {
                record_event(
                    diagnostic_log,
                    file_logger,
                    started,
                    LogLevel::Error,
                    &format!("Tray update error: {tray_error}"),
                )?;
            }
            return Ok(());
        }
    };

    let percentage = reading.battery.level_percent;
    let mut status = battery_status_label(&reading.battery);
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Debug,
        &format!(
            "Battery read for {}: {} ({})",
            reading.name,
            percentage.map_or_else(|| "unknown".to_owned(), |value| format!("{value}%")),
            status
        ),
    )?;
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
            record_event(
                diagnostic_log,
                file_logger,
                started,
                LogLevel::Error,
                &format!("Notification error: {error}"),
            )?;
            status.push_str("; notification error: ");
            status.push_str(&error);
        }
    }
    if percentage.is_none() {
        status.push_str("; percentage unavailable");
    }
    append_config_error(&mut status, notification_config_error);

    if let Err(error) = tray.update_status(&reading.name, &status, percentage) {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Error,
            &format!("Tray update error: {error}"),
        )?;
    }
    Ok(())
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

fn report_error(
    tray: &mut TrayController,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
    title: &str,
    message: &str,
) -> io::Result<()> {
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Error,
        &format!("{title}: {message}"),
    )?;
    if let Err(error) = tray.update_status("", &format!("{title}: {message}"), None) {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Error,
            &format!("Tray update error: {error}"),
        )?;
    }
    Ok(())
}

fn record_event(
    log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
    level: LogLevel,
    message: &str,
) -> io::Result<()> {
    file_logger.log(level, message)?;
    if log.len() == MAX_DIAGNOSTIC_LOG_ENTRIES {
        log.pop_front();
    }
    log.push_back(format!(
        "[+{}s] [{}] {message}",
        started.elapsed().as_secs(),
        level.as_str()
    ));
    Ok(())
}
