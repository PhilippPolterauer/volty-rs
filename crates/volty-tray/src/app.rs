use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use volty_battery::{BatteryLevel, BatterySource, BatteryState, BatteryStatus};

use crate::{
    about::show_about_window,
    config::{
        config_path, load_legacy_monitored_devices, remove_legacy_monitored_devices, AppConfig,
        LogLevel,
    },
    diagnose::show_diagnose_window,
    logging::FileLogger,
    notify::{BatterySample, NotificationEngine, NotificationOutcome},
    report::generate_diagnostic_report,
    state::{local_state_path, DeviceState},
    tray::{
        confirm_reset, reset_startup_preferences, set_start_on_boot_enabled, start_on_boot_enabled,
        DeviceMonitorEntry, TrayAction, TrayController,
    },
};

const UI_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const MAX_DIAGNOSTIC_LOG_ENTRIES: usize = 200;

enum PollCommand {
    Discover,
    UpdateIntervals { poll: Duration, retry: Duration },
    Quit,
}

#[derive(Clone)]
struct DeviceReading {
    identity: DeviceIdentity,
    name: String,
    device_index: Option<u8>,
    battery: BatteryStatus,
    notification_error: Option<String>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum DeviceIdentity {
    Hid {
        interface_path: String,
        device_index: u8,
    },
    Bluetooth {
        device_id: String,
    },
}

impl DeviceIdentity {
    fn config_id(&self) -> String {
        match self {
            Self::Hid {
                interface_path,
                device_index,
            } => format!("hid:{device_index}:{interface_path}"),
            Self::Bluetooth { device_id } => format!("bluetooth:{device_id}"),
        }
    }

    fn display_name(&self, name: &str) -> String {
        match self {
            Self::Hid { device_index, .. } => format_device_name(name, Some(*device_index)),
            Self::Bluetooth { .. } => name.to_owned(),
        }
    }
}

#[derive(Default)]
struct DeviceNameCache {
    names: HashMap<DeviceIdentity, String>,
}

impl DeviceNameCache {
    fn get(&self, identity: &DeviceIdentity) -> Option<&str> {
        self.names.get(identity).map(String::as_str)
    }

    fn resolve(
        &mut self,
        identity: DeviceIdentity,
        fallback_name: &str,
        query_name: impl FnOnce() -> Result<Option<String>, String>,
    ) -> (String, Option<String>) {
        if let Some(name) = self.names.get(&identity) {
            return (name.clone(), None);
        }

        let (name, error) = match query_name() {
            Ok(Some(name)) if !name.trim().is_empty() => (name.trim().to_owned(), None),
            Ok(_) => (fallback_name.to_owned(), None),
            Err(error) => (fallback_name.to_owned(), Some(error)),
        };
        self.names.insert(identity, name.clone());
        (name, error)
    }

    fn forget(&mut self, identity: &DeviceIdentity) {
        self.names.remove(identity);
    }
}

struct DeviceNotifications {
    config: AppConfig,
    engines: HashMap<DeviceIdentity, NotificationEngine>,
    persisted_alerts: HashSet<String>,
    pending_alerts: HashMap<String, bool>,
}

impl DeviceNotifications {
    fn from_config(config: AppConfig) -> Result<Self, Box<dyn Error>> {
        NotificationEngine::from_config(config.clone())?;
        Ok(Self {
            config,
            engines: HashMap::new(),
            persisted_alerts: HashSet::new(),
            pending_alerts: HashMap::new(),
        })
    }

    fn restore_persisted_alerts(&mut self, alerts: HashSet<String>) {
        self.persisted_alerts = alerts;
    }

    fn set_config(&mut self, config: AppConfig) -> Result<(), Box<dyn Error>> {
        NotificationEngine::from_config(config.clone())?;
        for engine in self.engines.values_mut() {
            engine.set_config(config.clone())?;
        }
        self.config = config;
        Ok(())
    }

    fn retain_monitored_devices(&mut self, monitored_devices: &BTreeMap<String, bool>) {
        self.engines
            .retain(|identity, _| is_device_monitored(monitored_devices, &identity.config_id()));
        let stale_alerts: HashSet<_> = self
            .persisted_alerts
            .iter()
            .chain(self.pending_alerts.keys())
            .filter(|identity| !is_device_monitored(monitored_devices, identity))
            .cloned()
            .collect();
        for identity in stale_alerts {
            self.persisted_alerts.remove(&identity);
            self.pending_alerts.insert(identity, false);
        }
    }

    fn update(
        &mut self,
        identity: DeviceIdentity,
        device_name: &str,
        sample: BatterySample,
    ) -> Result<NotificationOutcome, Box<dyn Error>> {
        let identity_key = identity.config_id();
        let previously_notified = self.persisted_alerts.contains(&identity_key);
        let engine = match self.engines.entry(identity) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let mut engine = NotificationEngine::from_config(self.config.clone())?;
                engine.restore_discharge_notification(previously_notified);
                entry.insert(engine)
            }
        };
        let outcome = engine.update(device_name, sample)?;
        match outcome {
            NotificationOutcome::Notified => {
                self.persisted_alerts.insert(identity_key.clone());
                self.pending_alerts.insert(identity_key, true);
            }
            NotificationOutcome::CycleReset => {
                self.persisted_alerts.remove(&identity_key);
                self.pending_alerts.insert(identity_key, false);
            }
            _ => {}
        }
        Ok(outcome)
    }

    fn persist_pending_alerts(
        &mut self,
        state: Option<&mut DeviceState>,
    ) -> Result<(), Box<dyn Error>> {
        let Some(state) = state else {
            self.pending_alerts.clear();
            return Ok(());
        };
        for (identity, sent) in &self.pending_alerts {
            state.set_discharge_notification_sent(identity, *sent)?;
        }
        self.pending_alerts.clear();
        Ok(())
    }
}

type PollResult = Result<(), String>;
type PollWorker = (
    mpsc::Sender<PollCommand>,
    Receiver<PollUpdate>,
    thread::JoinHandle<()>,
);

enum PollUpdate {
    Reading(DeviceReading),
    DeviceName {
        identity: DeviceIdentity,
        name: String,
        error: Option<String>,
    },
    Warning(String),
    Trace(String),
    Finished(PollResult),
}

struct ConfigWatch {
    _watcher: RecommendedWatcher,
    config_file: PathBuf,
    events: Receiver<notify::Result<Event>>,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let started = Instant::now();
    let mut diagnostic_log = VecDeque::new();
    let mut startup_state_warnings = Vec::new();
    let (legacy_monitored_devices, legacy_settings_available) =
        match load_legacy_monitored_devices() {
            Ok(devices) => (devices, true),
            Err(error) => {
                startup_state_warnings.push(format!(
                    "Could not read legacy monitored-device preferences: {error}"
                ));
                (BTreeMap::new(), false)
            }
        };
    let mut monitor_state = match DeviceState::open_local() {
        Ok(state) => Some(state),
        Err(error) => {
            startup_state_warnings.push(format!(
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
                        startup_state_warnings.push(format!(
                            "Could not remove migrated monitoring preferences from config: {error}"
                        ));
                    }
                }
                Err(error) => startup_state_warnings.push(format!(
                    "Could not migrate monitored-device preferences to local state: {error}"
                )),
            }
        }
    }
    let mut monitored_devices = match monitor_state.as_ref() {
        Some(state) => match state.monitored_devices() {
            Ok(devices) => devices,
            Err(error) => {
                startup_state_warnings.push(format!(
                    "Could not load local monitored-device preferences; using legacy settings for this run: {error}"
                ));
                legacy_monitored_devices.clone()
            }
        },
        None => legacy_monitored_devices,
    };
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
    for warning in startup_state_warnings {
        record_event(
            &mut diagnostic_log,
            &mut file_logger,
            started,
            LogLevel::Warn,
            &warning,
        )?;
    }
    let mut tray = TrayController::new()?;
    tray.set_battery_color_map(active_config.battery_color_map)?;
    tray.update_status("", "Searching for supported devices", None)?;
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
    let mut notifications = DeviceNotifications::from_config(active_config.clone())?;
    if let Some(state) = monitor_state.as_ref() {
        match state.notified_discharge_cycles() {
            Ok(alerts) => notifications.restore_persisted_alerts(alerts),
            Err(error) => record_event(
                &mut diagnostic_log,
                &mut file_logger,
                started,
                LogLevel::Warn,
                &format!("Could not load persisted low-battery notification cycles: {error}"),
            )?,
        }
    }
    notifications.retain_monitored_devices(&monitored_devices);
    if let Err(error) = notifications.persist_pending_alerts(monitor_state.as_mut()) {
        record_event(
            &mut diagnostic_log,
            &mut file_logger,
            started,
            LogLevel::Warn,
            &format!("Could not clear notification cycles for unmonitored devices: {error}"),
        )?;
    }
    let (mut commands_tx, mut results_rx, mut poller) = spawn_poll_worker(
        active_config,
        &mut diagnostic_log,
        &mut file_logger,
        started,
    )?;
    let mut config_reload_due = None;
    let mut pending_readings: Vec<DeviceReading> = Vec::new();
    let mut latest_readings: HashMap<DeviceIdentity, DeviceReading> = HashMap::new();
    let mut discovered_devices: HashMap<DeviceIdentity, String> = HashMap::new();
    let mut readings_seen_this_poll: HashSet<DeviceIdentity> = HashSet::new();
    let mut poll_had_warning = false;

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
                Ok(TrayAction::DiscoverDevicesNow) => {
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Info,
                        "Manual device discovery requested from tray",
                    )?;
                    let config_changed = reload_notification_config(
                        &mut tray,
                        &mut notifications,
                        &mut notification_config_error,
                        &mut active_config,
                        &commands_tx,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                    )?;
                    if config_changed {
                        retain_monitored_readings(
                            &mut latest_readings,
                            &mut pending_readings,
                            &monitored_devices,
                        );
                        update_monitored_device_menu(
                            &mut tray,
                            &discovered_devices,
                            &monitored_devices,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                        )?;
                        update_visible_devices(
                            &mut tray,
                            &latest_readings,
                            &monitored_devices,
                            notification_config_error.as_deref(),
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                        )?;
                    }
                    if commands_tx.send(PollCommand::Discover).is_err() {
                        report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "Discovery error",
                            "device discovery poller has stopped",
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
                    match generate_diagnostic_report(active_config.clone(), &diagnostic_log) {
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
                Ok(TrayAction::SetDeviceMonitored {
                    identity,
                    monitored,
                }) => {
                    let was_monitored = is_device_monitored(&monitored_devices, &identity);
                    if was_monitored == monitored {
                        tray.set_device_monitored_checked(&identity, was_monitored);
                        continue;
                    }

                    let save_result = match monitor_state.as_mut() {
                        Some(state) => state.set_device_monitored(&identity, monitored),
                        None => Err(io::Error::new(
                            io::ErrorKind::NotConnected,
                            "local SQLite monitoring state is unavailable",
                        )
                        .into()),
                    };
                    match save_result {
                        Ok(()) => {
                            monitored_devices.insert(identity.clone(), monitored);
                            let refresh_failed =
                                request_device_discovery(&commands_tx, monitored).is_err();
                            notifications.retain_monitored_devices(&monitored_devices);
                            if let Err(error) =
                                notifications.persist_pending_alerts(monitor_state.as_mut())
                            {
                                report_error(
                                    &mut tray,
                                    &mut diagnostic_log,
                                    &mut file_logger,
                                    started,
                                    "Notification state error",
                                    &error.to_string(),
                                )?;
                            }
                            let device_name = discovered_devices
                                .iter()
                                .find(|(device, _)| device.config_id() == identity)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("device");
                            record_event(
                                &mut diagnostic_log,
                                &mut file_logger,
                                started,
                                LogLevel::Info,
                                &format!(
                                    "{} monitoring for {device_name}",
                                    if monitored { "Enabled" } else { "Disabled" }
                                ),
                            )?;
                            retain_monitored_readings(
                                &mut latest_readings,
                                &mut pending_readings,
                                &monitored_devices,
                            );
                            update_monitored_device_menu(
                                &mut tray,
                                &discovered_devices,
                                &monitored_devices,
                                &mut diagnostic_log,
                                &mut file_logger,
                                started,
                            )?;
                            update_visible_devices(
                                &mut tray,
                                &latest_readings,
                                &monitored_devices,
                                notification_config_error.as_deref(),
                                &mut diagnostic_log,
                                &mut file_logger,
                                started,
                            )?;
                            if refresh_failed {
                                report_error(
                                    &mut tray,
                                    &mut diagnostic_log,
                                    &mut file_logger,
                                    started,
                                    "Discovery error",
                                    "device discovery poller has stopped",
                                )?;
                            }
                        }
                        Err(error) => {
                            tray.set_device_monitored_checked(&identity, was_monitored);
                            report_error(
                                &mut tray,
                                &mut diagnostic_log,
                                &mut file_logger,
                                started,
                                "Device monitoring state error",
                                &error.to_string(),
                            )?;
                        }
                    }
                }
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
                Ok(TrayAction::ResetVolty) => {
                    let configuration_file = config_path().ok_or_else(|| {
                        io::Error::other("could not determine the user configuration folder")
                    })?;
                    let local_state_file = local_state_path()?;
                    if !confirm_reset(&configuration_file, &local_state_file)? {
                        continue;
                    }

                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Info,
                        "Volty reset confirmed; deleting configuration and local device state",
                    )?;
                    let mut startup_reset_errors = Vec::new();
                    if let Err(error) = reset_startup_preferences() {
                        startup_reset_errors.push(error.to_string());
                    }
                    match start_on_boot_enabled() {
                        Ok(enabled) => tray.set_start_on_boot_checked(enabled),
                        Err(error) => startup_reset_errors.push(error.to_string()),
                    }
                    let _ = commands_tx.send(PollCommand::Quit);
                    poller.join().map_err(|_| {
                        io::Error::other("device polling worker panicked during reset")
                    })?;
                    drop(monitor_state.take());
                    remove_reset_files(&configuration_file, &local_state_file)?;
                    diagnostic_log.clear();

                    monitor_state = match DeviceState::open_local() {
                        Ok(state) => Some(state),
                        Err(error) => {
                            report_error(
                                &mut tray,
                                &mut diagnostic_log,
                                &mut file_logger,
                                started,
                                "Reset state error",
                                &format!("Could not recreate local state; using full device scans: {error}"),
                            )?;
                            None
                        }
                    };
                    monitored_devices = match monitor_state.as_ref() {
                        Some(state) => {
                            match state.monitored_devices() {
                                Ok(devices) => devices,
                                Err(error) => {
                                    report_error(
                                    &mut tray,
                                    &mut diagnostic_log,
                                    &mut file_logger,
                                    started,
                                    "Reset state error",
                                    &format!("Could not reload local monitoring preferences: {error}"),
                                )?;
                                    BTreeMap::new()
                                }
                            }
                        }
                        None => BTreeMap::new(),
                    };
                    (active_config, notification_config_error) = match AppConfig::load() {
                        Ok(config) => (config, None),
                        Err(error) => (AppConfig::default(), Some(error.to_string())),
                    };
                    file_logger.set_level(active_config.log_level);
                    notifications = DeviceNotifications::from_config(active_config)?;
                    if let Some(error) = notification_config_error.as_deref() {
                        record_event(
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            LogLevel::Error,
                            &format!("Configuration error after reset: {error}"),
                        )?;
                    }

                    pending_readings.clear();
                    latest_readings.clear();
                    discovered_devices.clear();
                    readings_seen_this_poll.clear();
                    poll_had_warning = false;
                    config_reload_due = None;
                    tray.set_battery_color_map(active_config.battery_color_map)?;
                    let reset_status = notification_config_error
                        .as_ref()
                        .map(|error| format!("Configuration error: {error}"))
                        .unwrap_or_else(|| "Searching for supported devices".to_owned());
                    tray.reset_status(&reset_status)?;
                    tray.update_monitored_devices(&[])?;

                    let (new_commands_tx, new_results_rx, new_poller) = spawn_poll_worker(
                        active_config,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                    )?;
                    commands_tx = new_commands_tx;
                    results_rx = new_results_rx;
                    poller = new_poller;
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Info,
                        "Volty reset completed; fresh configuration and local state are active",
                    )?;
                    if !startup_reset_errors.is_empty() {
                        report_error(
                            &mut tray,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            "Startup preference reset error",
                            &startup_reset_errors.join("; "),
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
            let config_changed = reload_notification_config(
                &mut tray,
                &mut notifications,
                &mut notification_config_error,
                &mut active_config,
                &commands_tx,
                &mut diagnostic_log,
                &mut file_logger,
                started,
            )?;
            if config_changed {
                retain_monitored_readings(
                    &mut latest_readings,
                    &mut pending_readings,
                    &monitored_devices,
                );
                update_monitored_device_menu(
                    &mut tray,
                    &discovered_devices,
                    &monitored_devices,
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                )?;
                update_visible_devices(
                    &mut tray,
                    &latest_readings,
                    &monitored_devices,
                    notification_config_error.as_deref(),
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                )?;
            }
            if config_changed && commands_tx.send(PollCommand::Discover).is_err() {
                report_error(
                    &mut tray,
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    "Discovery error",
                    "device discovery poller has stopped",
                )?;
            }
        }

        // Match explicitly so an unexpected worker disconnect is not confused
        // with an empty result queue.
        #[allow(clippy::while_let_loop)]
        loop {
            match results_rx.try_recv() {
                Ok(PollUpdate::Reading(reading)) => {
                    let identity = reading.identity.clone();
                    discovered_devices
                        .insert(identity.clone(), identity.display_name(&reading.name));
                    readings_seen_this_poll.insert(identity.clone());
                    if is_device_monitored(&monitored_devices, &identity.config_id()) {
                        pending_readings.push(reading);
                        update_tray_devices(
                            &mut tray,
                            &pending_readings,
                            notification_config_error.as_deref(),
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                        )?;
                        let reading = pending_readings.last_mut().ok_or_else(|| {
                            io::Error::other("pending battery reading disappeared")
                        })?;
                        process_device_reading(
                            &mut notifications,
                            monitor_state.as_mut(),
                            reading,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                        )?;
                        latest_readings.insert(identity, reading.clone());
                        if reading.notification_error.is_some() {
                            update_tray_devices(
                                &mut tray,
                                &pending_readings,
                                notification_config_error.as_deref(),
                                &mut diagnostic_log,
                                &mut file_logger,
                                started,
                            )?;
                        }
                    }
                    update_monitored_device_menu(
                        &mut tray,
                        &discovered_devices,
                        &monitored_devices,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                    )?;
                }
                Ok(PollUpdate::DeviceName {
                    identity,
                    name,
                    error,
                }) => {
                    let device_name = identity.display_name(&name);
                    let is_discovered = discovered_devices.contains_key(&identity);
                    discovered_devices.insert(identity.clone(), device_name.clone());
                    if let Some(reading) = pending_readings
                        .iter_mut()
                        .find(|reading| reading.identity == identity)
                    {
                        reading.name = name.clone();
                    } else if !is_discovered {
                        record_event(
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            LogLevel::Warn,
                            &format!(
                                "Received a device name without a pending battery reading for {device_name}"
                            ),
                        )?;
                    }
                    if let Some(reading) = latest_readings.get_mut(&identity) {
                        reading.name = name;
                    }
                    update_monitored_device_menu(
                        &mut tray,
                        &discovered_devices,
                        &monitored_devices,
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                    )?;
                    if let Some(error) = error {
                        record_event(
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                            LogLevel::Debug,
                            &format!(
                                "Protocol name discovery failed for {device_name}; using interface label: {error}"
                            ),
                        )?;
                    }
                }
                Ok(PollUpdate::Warning(message)) => {
                    poll_had_warning = true;
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Warn,
                        &message,
                    )?;
                }
                Ok(PollUpdate::Trace(message)) => record_event(
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    LogLevel::Trace,
                    &message,
                )?,
                Ok(PollUpdate::Finished(result)) => {
                    record_event(
                        &mut diagnostic_log,
                        &mut file_logger,
                        started,
                        LogLevel::Trace,
                        "Received device polling result",
                    )?;
                    if !poll_had_warning {
                        discovered_devices
                            .retain(|identity, _| readings_seen_this_poll.contains(identity));
                        latest_readings
                            .retain(|identity, _| readings_seen_this_poll.contains(identity));
                        update_monitored_device_menu(
                            &mut tray,
                            &discovered_devices,
                            &monitored_devices,
                            &mut diagnostic_log,
                            &mut file_logger,
                            started,
                        )?;
                    }
                    readings_seen_this_poll.clear();
                    poll_had_warning = false;
                    apply_poll_result(
                        &mut tray,
                        std::mem::take(&mut pending_readings),
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
    notifications: &mut DeviceNotifications,
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

    if let Err(error) = notifications.set_config(config.clone()) {
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
    file_logger.set_level(config.log_level);
    *active_config = config;
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

fn remove_reset_files(configuration_file: &Path, local_state_file: &Path) -> io::Result<()> {
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

fn spawn_poll_worker(
    active_config: AppConfig,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> Result<PollWorker, Box<dyn Error>> {
    let device_state = match DeviceState::open_local() {
        Ok(state) => Some(state),
        Err(error) => {
            record_event(
                diagnostic_log,
                file_logger,
                started,
                LogLevel::Warn,
                &format!("Discovery state disabled; using full device scans: {error}"),
            )?;
            None
        }
    };
    let (commands_tx, commands_rx) = mpsc::channel();
    let (results_tx, results_rx) = mpsc::channel();
    let poll_interval = Duration::from_secs(active_config.poll_interval_seconds);
    let retry_interval = Duration::from_secs(active_config.retry_interval_seconds);
    let poller = thread::Builder::new()
        .name("volty-device-poller".to_owned())
        .spawn(move || {
            poll_worker(
                commands_rx,
                results_tx,
                poll_interval,
                retry_interval,
                device_state,
            )
        })?;
    Ok((commands_tx, results_rx, poller))
}

fn toggle_start_on_boot() -> Result<bool, Box<dyn Error>> {
    let enabled = !start_on_boot_enabled()?;
    set_start_on_boot_enabled(enabled)?;
    Ok(enabled)
}

fn poll_worker(
    commands: mpsc::Receiver<PollCommand>,
    results: mpsc::Sender<PollUpdate>,
    poll_interval: Duration,
    retry_interval: Duration,
    mut device_state: Option<DeviceState>,
) {
    let mut name_cache = DeviceNameCache::default();
    poll_worker_with_query(
        commands,
        results,
        poll_interval,
        retry_interval,
        move |results| query_devices(results, &mut device_state, &mut name_cache),
    );
}

fn request_device_discovery(
    commands: &mpsc::Sender<PollCommand>,
    monitored: bool,
) -> Result<(), mpsc::SendError<PollCommand>> {
    if monitored {
        commands.send(PollCommand::Discover)
    } else {
        Ok(())
    }
}

fn poll_worker_with_query(
    commands: mpsc::Receiver<PollCommand>,
    results: mpsc::Sender<PollUpdate>,
    mut poll_interval: Duration,
    mut retry_interval: Duration,
    mut query: impl FnMut(&mpsc::Sender<PollUpdate>) -> PollResult,
) {
    loop {
        let reading = query(&results);
        let successful_read = reading.is_ok();
        let next_poll = if successful_read {
            poll_interval
        } else {
            retry_interval
        };
        if results.send(PollUpdate::Finished(reading)).is_err() {
            return;
        }

        let mut wait_interval = next_poll;
        loop {
            match commands.recv_timeout(wait_interval) {
                Ok(PollCommand::Discover) | Err(RecvTimeoutError::Timeout) => break,
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

fn query_devices(
    results: &mpsc::Sender<PollUpdate>,
    device_state: &mut Option<DeviceState>,
    name_cache: &mut DeviceNameCache,
) -> PollResult {
    use volty_hid::{enumerate_hid_interfaces, resolve_mouse_model};

    let mut errors = Vec::new();
    let interfaces = match enumerate_hid_interfaces() {
        Ok(interfaces) => interfaces,
        Err(error) => {
            errors.push(format!("HID discovery: {error}"));
            Vec::new()
        }
    };
    let mut supported_interfaces = 0;
    let mut reading_count = 0;
    for interface in interfaces {
        let mouse = match resolve_mouse_model(&interface) {
            Ok(Some(mouse)) => mouse,
            Ok(None) => {
                results
                    .send(PollUpdate::Trace(format!(
                        "HID discovery skipped an unsupported interface (VID=0x{:04x}, PID=0x{:04x}, usage_page={:?}, usage={:?}, interface={}): no supported HID protocol matched its interface metadata",
                        interface.vendor_id,
                        interface.product_id,
                        interface.usage_page,
                        interface.usage,
                        interface.interface_number
                    )))
                    .map_err(|_| "device polling result receiver disconnected".to_owned())?;
                continue;
            }
            Err(error) => {
                errors.push(format!(
                    "Ambiguous HID backend match for {:04x}:{:04x}: {error}",
                    interface.vendor_id, interface.product_id
                ));
                continue;
            }
        };
        supported_interfaces += 1;
        let protocol = mouse.protocol();
        let fallback_name = interface
            .product_string
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| mouse.name().to_owned());
        results
            .send(PollUpdate::Trace(format!(
                "HID discovery matched {fallback_name:?} to {} using {} (VID=0x{:04x}, PID=0x{:04x}, usage_page={:?}, usage={:?}, interface={})",
                mouse.name(),
                protocol.name(),
                interface.vendor_id,
                interface.product_id,
                interface.usage_page,
                interface.usage,
                interface.interface_number
            )))
            .map_err(|_| "device polling result receiver disconnected".to_owned())?;
        let interface_path = interface.path.to_string_lossy().into_owned();
        let preferred_indices = device_state
            .as_ref()
            .map(|state| state.ordered_device_indices(&interface_path, protocol.device_indices()));
        let device_indices = match preferred_indices {
            Some(Ok(indices)) => indices,
            Some(Err(error)) => {
                *device_state = None;
                results
                    .send(PollUpdate::Warning(format!(
                        "Could not read discovery state; continuing with a full scan: {error}"
                    )))
                    .map_err(|_| "device polling result receiver disconnected".to_owned())?;
                protocol.device_indices().to_vec()
            }
            None => protocol.device_indices().to_vec(),
        };

        for device_index in device_indices {
            let identity = DeviceIdentity::Hid {
                interface_path: interface_path.clone(),
                device_index,
            };
            let cached_name = name_cache.get(&identity).map(str::to_owned);
            let reading = match cached_name {
                Some(name) => protocol
                    .query_battery(&interface, device_index)
                    .map(|battery| (battery, name, None, false)),
                None => protocol
                    .query_device(&interface, device_index)
                    .map(|device| {
                        let volty_hid::ProtocolDeviceReading {
                            name: queried_name,
                            battery,
                        } = device;
                        let (name, name_error) =
                            name_cache.resolve(identity.clone(), &fallback_name, || {
                                queried_name.map_err(|error| error.to_string())
                            });
                        (battery, name, name_error, true)
                    }),
            };
            match reading {
                Ok((battery, name, name_error, name_was_queried)) => {
                    let reading = DeviceReading {
                        identity: identity.clone(),
                        name: name.clone(),
                        device_index: Some(device_index),
                        battery,
                        notification_error: None,
                    };
                    results
                        .send(PollUpdate::Reading(reading))
                        .map_err(|_| "device polling result receiver disconnected".to_owned())?;
                    reading_count += 1;
                    let state_error = device_state
                        .as_mut()
                        .and_then(|state| state.record(&interface_path, device_index).err());
                    if let Some(error) = state_error {
                        *device_state = None;
                        results
                            .send(PollUpdate::Warning(format!(
                                "Could not update discovery state; continuing with full scans: {error}"
                            )))
                            .map_err(|_| "device polling result receiver disconnected".to_owned())?;
                    }
                    if name_was_queried {
                        results
                            .send(PollUpdate::DeviceName {
                                identity,
                                name,
                                error: name_error,
                            })
                            .map_err(|_| {
                                "device polling result receiver disconnected".to_owned()
                            })?;
                    }
                }
                Err(error) => {
                    name_cache.forget(&identity);
                    errors.push(format!(
                        "{fallback_name} via {} (slot {device_index}): {error}",
                        protocol.name()
                    ));
                }
            }
        }
    }

    match volty_bluetooth::scan_battery_devices() {
        Ok(scan) => {
            results
                .send(PollUpdate::Trace(format!(
                    "Bluetooth discovery examined {} device entries and found {} readable battery value(s) with {} warning(s)",
                    scan.devices_examined,
                    scan.readings.len(),
                    scan.warnings.len()
                )))
                .map_err(|_| "device polling result receiver disconnected".to_owned())?;
            for event in scan.discovery_events {
                results
                    .send(PollUpdate::Trace(format!("Bluetooth discovery: {event}")))
                    .map_err(|_| "device polling result receiver disconnected".to_owned())?;
            }
            errors.extend(
                scan.warnings
                    .into_iter()
                    .map(|warning| format!("Bluetooth battery read: {warning}")),
            );
            for reading in scan.readings {
                let identity = DeviceIdentity::Bluetooth {
                    device_id: reading.device_id,
                };
                let reading = DeviceReading {
                    identity,
                    name: reading.name,
                    device_index: None,
                    battery: reading.battery,
                    notification_error: None,
                };
                results
                    .send(PollUpdate::Reading(reading))
                    .map_err(|_| "device polling result receiver disconnected".to_owned())?;
                reading_count += 1;
            }
        }
        Err(error) => errors.push(format!("Bluetooth discovery: {error}")),
    }

    if reading_count > 0 {
        if !errors.is_empty() {
            results
                .send(PollUpdate::Warning(format!(
                    "Some battery readings failed: {}",
                    errors.join("; ")
                )))
                .map_err(|_| "device polling result receiver disconnected".to_owned())?;
        }
        return Ok(());
    }

    if supported_interfaces == 0 && errors.is_empty() {
        return Err(
            "No readable battery found. Bluetooth support requires a Windows-reported battery property or the standard BLE Battery Service."
                .to_owned(),
        );
    }

    Err(if errors.is_empty() {
        "No supported battery devices found".to_owned()
    } else {
        errors.join("; ")
    })
}

fn apply_poll_result(
    tray: &mut TrayController,
    readings: Vec<DeviceReading>,
    result: PollResult,
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    if let Err(error) = result {
        if readings.is_empty() {
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
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Warn,
            &format!("Device polling partially failed: {error}"),
        )?;
    }

    update_tray_devices(
        tray,
        &readings,
        notification_config_error,
        diagnostic_log,
        file_logger,
        started,
    )
}

fn update_tray_devices(
    tray: &mut TrayController,
    readings: &[DeviceReading],
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let mut device_statuses = Vec::with_capacity(readings.len());
    let mut lowest_percentage: Option<u8> = None;
    for reading in readings {
        let device_name = format_device_name(&reading.name, reading.device_index);
        let percentage = reading.battery.level_percent;
        lowest_percentage = match (lowest_percentage, percentage) {
            (Some(current), Some(value)) => Some(current.min(value)),
            (None, Some(value)) => Some(value),
            (current, None) => current,
        };
        let mut status = battery_status_label(&reading.battery);
        if let Some(error) = &reading.notification_error {
            status.push_str("; notification error: ");
            status.push_str(&error);
        }
        let percentage_label = percentage.map_or_else(
            || "percentage unavailable".to_owned(),
            |value| format!("{value}%"),
        );
        device_statuses.push(format!("{device_name}: {percentage_label} ({status})"));
    }
    let device_count = device_statuses.len();
    if let Some(error) = notification_config_error {
        device_statuses.push(format!("Notification configuration error: {error}"));
    }
    let mut summary = format!(
        "{device_count} device{} monitored",
        if device_count == 1 { "" } else { "s" }
    );
    append_config_error(&mut summary, notification_config_error);

    if let Err(error) = tray.update_devices(&summary, &device_statuses, lowest_percentage) {
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

fn update_monitored_device_menu(
    tray: &mut TrayController,
    devices: &HashMap<DeviceIdentity, String>,
    monitored_devices: &BTreeMap<String, bool>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let entries: Vec<_> = devices
        .iter()
        .map(|(identity, name)| {
            let config_id = identity.config_id();
            DeviceMonitorEntry {
                monitored: is_device_monitored(monitored_devices, &config_id),
                identity: config_id,
                name: name.clone(),
            }
        })
        .collect();
    if let Err(error) = tray.update_monitored_devices(&entries) {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Error,
            &format!("Monitored-devices menu update error: {error}"),
        )?;
    }
    Ok(())
}

fn update_visible_devices(
    tray: &mut TrayController,
    latest_readings: &HashMap<DeviceIdentity, DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
    notification_config_error: Option<&str>,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let readings = monitored_readings(latest_readings, monitored_devices);
    update_tray_devices(
        tray,
        &readings,
        notification_config_error,
        diagnostic_log,
        file_logger,
        started,
    )
}

fn monitored_readings(
    latest_readings: &HashMap<DeviceIdentity, DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
) -> Vec<DeviceReading> {
    let mut readings: Vec<_> = latest_readings
        .iter()
        .filter(|(identity, _)| is_device_monitored(monitored_devices, &identity.config_id()))
        .map(|(_, reading)| reading.clone())
        .collect();
    readings.sort_by_key(|reading| reading.identity.config_id());
    readings
}

fn retain_monitored_readings(
    latest_readings: &mut HashMap<DeviceIdentity, DeviceReading>,
    pending_readings: &mut Vec<DeviceReading>,
    monitored_devices: &BTreeMap<String, bool>,
) {
    latest_readings
        .retain(|identity, _| is_device_monitored(monitored_devices, &identity.config_id()));
    pending_readings
        .retain(|reading| is_device_monitored(monitored_devices, &reading.identity.config_id()));
}

fn is_device_monitored(monitored_devices: &BTreeMap<String, bool>, identity: &str) -> bool {
    monitored_devices.get(identity).copied().unwrap_or(true)
}

fn process_device_reading(
    notifications: &mut DeviceNotifications,
    state: Option<&mut DeviceState>,
    reading: &mut DeviceReading,
    diagnostic_log: &mut VecDeque<String>,
    file_logger: &mut FileLogger,
    started: Instant,
) -> io::Result<()> {
    let device_name = format_device_name(&reading.name, reading.device_index);
    let percentage = reading.battery.level_percent;
    let status = battery_status_label(&reading.battery);
    record_event(
        diagnostic_log,
        file_logger,
        started,
        LogLevel::Debug,
        &format!(
            "Battery read for {device_name} via {}: {} ({status})",
            battery_source_label(reading.battery.source),
            percentage.map_or_else(|| "unknown".to_owned(), |value| format!("{value}%"))
        ),
    )?;
    let charging = matches!(
        reading.battery.state,
        BatteryState::Charging | BatteryState::ChargingNearlyFull | BatteryState::ChargingSlow
    );
    let full = battery_is_full(&reading.battery);
    if percentage.is_some() || charging || full {
        let sample = BatterySample {
            // Percentage is irrelevant while charging/full because the engine
            // resets that device's discharge-cycle guard before checking the threshold.
            percentage: percentage.unwrap_or(0),
            charging,
            full,
        };
        let threshold = notifications.config.low_battery_threshold;
        let enabled = notifications.config.notifications_enabled;
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Trace,
            &format!(
                "Evaluating low-battery notification for {device_name}: battery={}%, charging={charging}, full={full}, enabled={enabled}, threshold={threshold}%",
                sample.percentage
            ),
        )?;
        let notification_result =
            notifications.update(reading.identity.clone(), &device_name, sample);
        if let Err(error) = notifications.persist_pending_alerts(state) {
            record_event(
                diagnostic_log,
                file_logger,
                started,
                LogLevel::Warn,
                &format!(
                    "Could not persist low-battery notification cycle for {device_name}: {error}"
                ),
            )?;
        }
        match notification_result {
            Ok(outcome) => {
                let detail = match outcome {
                    NotificationOutcome::NotificationsDisabled => {
                        "skipped: disabled in configuration"
                    }
                    NotificationOutcome::AboveThreshold => {
                        "skipped: battery is above the threshold"
                    }
                    NotificationOutcome::AlreadyNotified => {
                        "skipped: already delivered during this discharge cycle"
                    }
                    NotificationOutcome::Unchanged => "no notification due",
                    NotificationOutcome::CycleReset => "discharge cycle reset",
                    NotificationOutcome::Notified => "delivered successfully",
                };
                record_event(
                    diagnostic_log,
                    file_logger,
                    started,
                    if outcome == NotificationOutcome::Notified {
                        LogLevel::Info
                    } else {
                        LogLevel::Trace
                    },
                    &format!(
                        "Low-battery notification for {device_name} at {}% (threshold={threshold}%): {detail}",
                        sample.percentage
                    ),
                )?;
            }
            Err(error) => {
                let error = error.to_string();
                record_event(
                    diagnostic_log,
                    file_logger,
                    started,
                    LogLevel::Error,
                    &format!("Notification error for {device_name}: {error}"),
                )?;
                reading.notification_error = Some(error);
            }
        }
    } else {
        record_event(
            diagnostic_log,
            file_logger,
            started,
            LogLevel::Trace,
            &format!(
                "Skipping low-battery notification for {device_name}: battery percentage unavailable and device is not charging or full"
            ),
        )?;
    }
    Ok(())
}

fn format_device_name(name: &str, device_index: Option<u8>) -> String {
    match device_index {
        None => name.to_owned(),
        Some(0xff) => format!("{name} (direct)"),
        Some(device_index) => format!("{name} (slot {device_index})"),
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
        BatteryLevel::Full => Some("full level"),
    };
    match level {
        Some(level) => format!("{state} ({level})"),
        None => state.to_owned(),
    }
}

fn battery_source_label(source: BatterySource) -> &'static str {
    match source {
        BatterySource::Unified => "HID++ Unified Battery",
        BatterySource::LegacyBatteryStatus => "HID++ Battery Status",
        BatterySource::BluetoothGatt => "Bluetooth GATT Battery Service",
        BatterySource::BluetoothDeviceProperty => "Windows Bluetooth battery property",
    }
}

fn battery_is_full(battery: &BatteryStatus) -> bool {
    matches!(battery.state, BatteryState::Charged)
        || battery
            .level_percent
            .is_some_and(|percentage| percentage >= 100)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

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

    #[test]
    fn coarse_full_level_does_not_mark_a_discharging_battery_as_full() {
        let battery = BatteryStatus {
            level_percent: Some(77),
            level: BatteryLevel::Full,
            raw_level: 8,
            state: BatteryState::Discharging,
            raw_status: Some(0),
            source: BatterySource::Unified,
        };

        assert!(!battery_is_full(&battery));
        assert_eq!(battery_status_label(&battery), "Discharging (full level)");
    }

    #[test]
    fn discovery_command_triggers_an_immediate_device_poll() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::channel();
        let query_count = Arc::new(AtomicUsize::new(0));
        let worker_query_count = Arc::clone(&query_count);
        let worker = thread::spawn(move || {
            poll_worker_with_query(
                commands_rx,
                results_tx,
                Duration::from_secs(60),
                Duration::from_secs(60),
                move |_| {
                    worker_query_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            );
        });

        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Discover).unwrap();
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Quit).unwrap();
        worker.join().unwrap();

        assert_eq!(query_count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn re_enabling_device_requests_immediate_discovery_but_disabling_does_not() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::channel();
        let query_count = Arc::new(AtomicUsize::new(0));
        let worker_query_count = Arc::clone(&query_count);
        let worker = thread::spawn(move || {
            poll_worker_with_query(
                commands_rx,
                results_tx,
                Duration::from_secs(60),
                Duration::from_secs(60),
                move |_| {
                    worker_query_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            );
        });

        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        request_device_discovery(&commands_tx, true).unwrap();
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        request_device_discovery(&commands_tx, false).unwrap();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(query_count.load(Ordering::Relaxed), 2);

        commands_tx.send(PollCommand::Quit).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn poll_worker_forwards_readings_before_the_query_finishes() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::channel();
        let (finish_query_tx, finish_query_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            poll_worker_with_query(
                commands_rx,
                results_tx,
                Duration::from_secs(60),
                Duration::from_secs(60),
                move |results| {
                    assert!(results.send(PollUpdate::Reading(test_reading())).is_ok());
                    finish_query_rx.recv().unwrap();
                    assert!(results
                        .send(PollUpdate::DeviceName {
                            identity: DeviceIdentity::Hid {
                                interface_path: "test".to_owned(),
                                device_index: 1,
                            },
                            name: "Resolved name".to_owned(),
                            error: None,
                        })
                        .is_ok());
                    Ok(())
                },
            );
        });

        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Reading(_)
        ));
        finish_query_tx.send(()).unwrap();
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::DeviceName { .. }
        ));
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Quit).unwrap();
        worker.join().unwrap();
    }

    fn test_reading() -> DeviceReading {
        DeviceReading {
            identity: DeviceIdentity::Hid {
                interface_path: "test".to_owned(),
                device_index: 1,
            },
            name: "Test device".to_owned(),
            device_index: Some(1),
            battery: BatteryStatus {
                level_percent: Some(20),
                level: BatteryLevel::Low,
                raw_level: 2,
                state: BatteryState::Discharging,
                raw_status: Some(0),
                source: BatterySource::Unified,
            },
            notification_error: None,
        }
    }

    #[test]
    fn device_names_are_requested_once_until_the_device_disconnects() {
        let identity = DeviceIdentity::Hid {
            interface_path: "receiver".to_owned(),
            device_index: 1,
        };
        let mut cache = DeviceNameCache::default();
        let mut query_count = 0;

        let first = cache.resolve(identity.clone(), "Interface label", || {
            query_count += 1;
            Ok(Some("  Mouse Name  ".to_owned()))
        });
        let second = cache.resolve(identity.clone(), "Changed interface label", || {
            query_count += 1;
            Ok(Some("Unexpected second name".to_owned()))
        });

        assert_eq!(first, ("Mouse Name".to_owned(), None));
        assert_eq!(second, ("Mouse Name".to_owned(), None));
        assert_eq!(query_count, 1);

        cache.forget(&identity);
        let after_disconnect = cache.resolve(identity, "New interface label", || {
            query_count += 1;
            Ok(Some("Replacement mouse".to_owned()))
        });

        assert_eq!(after_disconnect, ("Replacement mouse".to_owned(), None));
        assert_eq!(query_count, 2);
    }

    #[test]
    fn failed_device_name_lookup_is_cached_until_disconnect() {
        let identity = DeviceIdentity::Hid {
            interface_path: "receiver".to_owned(),
            device_index: 2,
        };
        let mut cache = DeviceNameCache::default();
        let mut query_count = 0;

        let first = cache.resolve(identity.clone(), "Interface label", || {
            query_count += 1;
            Err("name request timed out".to_owned())
        });
        let second = cache.resolve(identity, "Changed interface label", || {
            query_count += 1;
            Ok(Some("Unexpected retry".to_owned()))
        });

        assert_eq!(
            first,
            (
                "Interface label".to_owned(),
                Some("name request timed out".to_owned())
            )
        );
        assert_eq!(second, ("Interface label".to_owned(), None));
        assert_eq!(query_count, 1);
    }

    #[test]
    fn bluetooth_device_names_are_not_formatted_as_hid_slots() {
        let identity = DeviceIdentity::Bluetooth {
            device_id: "opaque-device-id".to_owned(),
        };

        assert_eq!(
            identity.display_name("Baseus Bass BS2 Lite"),
            "Baseus Bass BS2 Lite"
        );
        assert_eq!(
            format_device_name("Receiver mouse", Some(3)),
            "Receiver mouse (slot 3)"
        );
    }

    #[test]
    fn bluetooth_identity_does_not_collide_with_hid_identity() {
        let identities = std::collections::HashSet::from([
            DeviceIdentity::Hid {
                interface_path: "same-opaque-id".to_owned(),
                device_index: 1,
            },
            DeviceIdentity::Bluetooth {
                device_id: "same-opaque-id".to_owned(),
            },
        ]);

        assert_eq!(identities.len(), 2);
    }

    #[test]
    fn device_monitor_identity_is_stable_and_source_specific() {
        let hid = DeviceIdentity::Hid {
            interface_path: "receiver-path".to_owned(),
            device_index: 1,
        };
        let bluetooth = DeviceIdentity::Bluetooth {
            device_id: "receiver-path".to_owned(),
        };

        assert_eq!(hid.config_id(), "hid:1:receiver-path");
        assert_eq!(bluetooth.config_id(), "bluetooth:receiver-path");
        assert_ne!(hid.config_id(), bluetooth.config_id());
        assert_eq!(hid.config_id(), hid.clone().config_id());
    }

    #[test]
    fn new_devices_are_monitored_by_default() {
        let hid = test_reading();
        let mut bluetooth = test_reading();
        bluetooth.identity = DeviceIdentity::Bluetooth {
            device_id: "bluetooth-device".to_owned(),
        };
        bluetooth.name = "Bluetooth device".to_owned();

        let latest_readings = HashMap::from([
            (hid.identity.clone(), hid),
            (bluetooth.identity.clone(), bluetooth),
        ]);
        let visible = monitored_readings(&latest_readings, &BTreeMap::new());

        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn unmonitored_devices_are_excluded_from_visible_readings() {
        let included = test_reading();
        let mut excluded = test_reading();
        excluded.identity = DeviceIdentity::Bluetooth {
            device_id: "bluetooth-device".to_owned(),
        };
        excluded.name = "Excluded device".to_owned();

        let excluded_identity = excluded.identity.clone();
        let included_identity = included.identity.clone();
        let latest_readings = HashMap::from([
            (included_identity.clone(), included),
            (excluded_identity.clone(), excluded),
        ]);
        let monitored_devices = BTreeMap::from([(excluded_identity.config_id(), false)]);

        let visible = monitored_readings(&latest_readings, &monitored_devices);
        assert_eq!(visible.len(), 1);
        assert_eq!(
            visible[0].identity.config_id(),
            included_identity.config_id()
        );
    }

    #[test]
    fn disabled_devices_drop_notification_tracking_state() {
        let monitored = DeviceIdentity::Hid {
            interface_path: "monitored".to_owned(),
            device_index: 1,
        };
        let unmonitored = DeviceIdentity::Bluetooth {
            device_id: "unmonitored".to_owned(),
        };
        let mut notifications = DeviceNotifications::from_config(AppConfig::default()).unwrap();
        notifications.engines.insert(
            monitored.clone(),
            NotificationEngine::from_config(AppConfig::default()).unwrap(),
        );
        notifications.engines.insert(
            unmonitored.clone(),
            NotificationEngine::from_config(AppConfig::default()).unwrap(),
        );
        let monitored_devices = BTreeMap::from([(unmonitored.config_id(), false)]);
        let monitored_key = monitored.config_id();
        let unmonitored_key = unmonitored.config_id();
        notifications.restore_persisted_alerts(HashSet::from([
            monitored_key.clone(),
            unmonitored_key.clone(),
        ]));

        notifications.retain_monitored_devices(&monitored_devices);
        assert!(notifications.engines.contains_key(&monitored));
        assert!(!notifications.engines.contains_key(&unmonitored));
        assert!(notifications.persisted_alerts.contains(&monitored_key));
        assert!(!notifications.persisted_alerts.contains(&unmonitored_key));
        assert_eq!(
            notifications.pending_alerts.get(&unmonitored_key),
            Some(&false)
        );
    }

    #[test]
    fn bluetooth_battery_source_is_explicit_in_diagnostics() {
        assert_eq!(
            battery_source_label(BatterySource::BluetoothGatt),
            "Bluetooth GATT Battery Service"
        );
        assert_eq!(
            battery_source_label(BatterySource::BluetoothDeviceProperty),
            "Windows Bluetooth battery property"
        );
    }
}
