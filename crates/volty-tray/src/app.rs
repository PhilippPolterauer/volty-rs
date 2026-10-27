mod battery_eta;
mod device_cache;
mod device_ui;
mod labels;
mod notifications;
mod poll;
mod startup;

use battery_eta::BatteryEtaEstimator;
use device_ui::{
    apply_poll_result, is_device_monitored, retain_monitored_readings,
    update_monitored_device_menu, update_tray_devices, update_visible_devices,
};
use labels::{
    append_config_error, battery_is_full, battery_source_label, battery_status_label,
    format_device_name,
};
use notifications::{process_device_reading, DeviceNotifications};
use poll::{request_device_discovery, spawn_poll_worker, PollCommand, PollUpdate};
use startup::{
    load_startup_state, open_configuration, open_log_file, process_windows_messages,
    remove_reset_files, toggle_start_on_boot, watch_config_file, ConfigWatch,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    io,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};
use volty_battery::BatteryStatus;

use crate::{
    about::show_about_window,
    config::{config_path, AppConfig, LogLevel},
    diagnose::show_diagnose_window,
    logging::FileLogger,
    report::generate_diagnostic_report,
    state::{local_state_path, DeviceState},
    tray::{
        confirm_reset, reset_startup_preferences, set_start_on_boot_enabled, start_on_boot_enabled,
        TrayAction, TrayController,
    },
};

const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const MAX_DIAGNOSTIC_LOG_ENTRIES: usize = 200;

#[derive(Clone)]
struct DeviceReading {
    identity: DeviceIdentity,
    name: String,
    device_index: Option<u8>,
    battery: BatteryStatus,
    estimated_time_remaining: Option<Duration>,
    notification_error: Option<String>,
}

#[derive(Clone, Eq, Hash, PartialEq)]
enum DeviceIdentity {
    Hid {
        interface_path: String,
        device_index: u8,
    },
    #[cfg(feature = "bluetooth")]
    Bluetooth { device_id: String },
}

impl DeviceIdentity {
    fn config_id(&self) -> String {
        match self {
            Self::Hid {
                interface_path,
                device_index,
            } => format!("hid:{device_index}:{interface_path}"),
            #[cfg(feature = "bluetooth")]
            Self::Bluetooth { device_id } => format!("bluetooth:{device_id}"),
        }
    }

    fn display_name(&self, name: &str) -> String {
        match self {
            Self::Hid { device_index, .. } => format_device_name(name, Some(*device_index)),
            #[cfg(feature = "bluetooth")]
            Self::Bluetooth { .. } => name.to_owned(),
        }
    }
}

struct App {
    started: Instant,
    diagnostic_log: VecDeque<String>,
    file_logger: FileLogger,
    tray: TrayController,
    config_watch: ConfigWatch,
    monitor_state: Option<DeviceState>,
    monitored_devices: BTreeMap<String, bool>,
    active_config: AppConfig,
    notification_config_error: Option<String>,
    notifications: DeviceNotifications,
    commands: mpsc::Sender<PollCommand>,
    results: Receiver<PollUpdate>,
    poller: Option<thread::JoinHandle<()>>,
    config_reload_due: Option<Instant>,
    pending_readings: Vec<DeviceReading>,
    latest_readings: HashMap<DeviceIdentity, DeviceReading>,
    battery_eta_estimator: BatteryEtaEstimator,
    discovered_devices: HashMap<DeviceIdentity, String>,
    readings_seen_this_poll: HashSet<DeviceIdentity>,
    poll_had_warning: bool,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    App::start()?.run_loop()
}
impl App {
    fn start() -> Result<Self, Box<dyn Error>> {
        let started = Instant::now();
        let mut startup = load_startup_state();
        let mut diagnostic_log = VecDeque::new();
        let mut file_logger = FileLogger::open(startup.active_config.log_level)?;
        record_event(
            &mut diagnostic_log,
            &mut file_logger,
            started,
            LogLevel::Info,
            "Volty started",
        )?;
        for warning in &startup.warnings {
            record_event(
                &mut diagnostic_log,
                &mut file_logger,
                started,
                LogLevel::Warn,
                warning,
            )?;
        }

        let mut tray = TrayController::new()?;
        tray.set_battery_color_map(startup.active_config.battery_color_map)?;
        tray.update_status("", "Searching for supported devices", None)?;
        let config_watch = watch_config_file()?;

        if let Some(message) = startup.notification_config_error.as_deref() {
            record_event(
                &mut diagnostic_log,
                &mut file_logger,
                started,
                LogLevel::Error,
                &format!("Configuration error: {message}"),
            )?;
            tray.update_status("", &format!("Configuration error: {message}"), None)?;
        }

        let mut notifications = DeviceNotifications::from_config(startup.active_config)?;
        if let Some(state) = startup.monitor_state.as_ref() {
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
        notifications.retain_monitored_devices(&startup.monitored_devices);
        if let Err(error) = notifications.persist_pending_alerts(startup.monitor_state.as_mut()) {
            record_event(
                &mut diagnostic_log,
                &mut file_logger,
                started,
                LogLevel::Warn,
                &format!("Could not clear notification cycles for unmonitored devices: {error}"),
            )?;
        }
        let (commands, results, poller) = spawn_poll_worker(
            startup.active_config,
            &mut diagnostic_log,
            &mut file_logger,
            started,
        )?;

        Ok(Self {
            started,
            diagnostic_log,
            file_logger,
            tray,
            config_watch,
            monitor_state: startup.monitor_state,
            monitored_devices: startup.monitored_devices,
            active_config: startup.active_config,
            notification_config_error: startup.notification_config_error,
            notifications,
            commands,
            results,
            poller: Some(poller),
            config_reload_due: None,
            pending_readings: Vec::new(),
            latest_readings: HashMap::new(),
            battery_eta_estimator: BatteryEtaEstimator::default(),
            discovered_devices: HashMap::new(),
            readings_seen_this_poll: HashSet::new(),
            poll_had_warning: false,
        })
    }

    fn run_loop(&mut self) -> Result<(), Box<dyn Error>> {
        loop {
            if !process_windows_messages() {
                self.record_event(LogLevel::Info, "Windows requested application shutdown")?;
                return Ok(());
            }

            self.process_tray_menu_events()?;
            if !self.dispatch_tray_actions()? {
                return Ok(());
            }
            self.process_config_events()?;
            self.reload_config_if_due()?;
            self.process_poll_results()?;
            thread::sleep(Duration::from_millis(
                self.active_config.ui_poll_interval_ms,
            ));
        }
    }

    fn process_tray_menu_events(&mut self) -> io::Result<()> {
        if let Err(error) = self.tray.process_menu_events() {
            self.report_error("Tray menu error", &error.to_string())?;
        }
        Ok(())
    }

    fn dispatch_tray_actions(&mut self) -> Result<bool, Box<dyn Error>> {
        loop {
            match self.tray.try_recv_action() {
                Ok(action) => {
                    if !self.handle_tray_action(action)? {
                        return Ok(false);
                    }
                }
                Err(TryRecvError::Empty) => return Ok(true),
                Err(TryRecvError::Disconnected) => {
                    self.report_error("Tray error", "tray action channel disconnected")?;
                    return Ok(true);
                }
            }
        }
    }

    fn handle_tray_action(&mut self, action: TrayAction) -> Result<bool, Box<dyn Error>> {
        match action {
            TrayAction::DiscoverDevicesNow => self.discover_devices_now()?,
            TrayAction::OpenLogFile => self.open_log_file()?,
            TrayAction::OpenConfiguration => self.open_configuration()?,
            TrayAction::Diagnose => self.show_diagnostic_report()?,
            TrayAction::ToggleStartOnBoot => self.toggle_start_on_boot()?,
            TrayAction::SetDeviceMonitored {
                identity,
                monitored,
            } => self.set_device_monitored(identity, monitored)?,
            TrayAction::About => {
                if let Err(error) = show_about_window() {
                    self.report_error("About window error", &error.to_string())?;
                }
            }
            TrayAction::ResetVolty => self.reset_volty()?,
            TrayAction::Quit => {
                self.record_event(LogLevel::Info, "Volty shutting down")?;
                let _ = self.commands.send(PollCommand::Quit);
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn discover_devices_now(&mut self) -> Result<(), Box<dyn Error>> {
        self.record_event(
            LogLevel::Info,
            "Manual device discovery requested from tray",
        )?;
        if self.reload_notification_config()? {
            self.refresh_visible_devices()?;
        }
        if self.commands.send(PollCommand::Discover).is_err() {
            self.report_error("Discovery error", "device discovery poller has stopped")?;
        }
        Ok(())
    }

    fn open_log_file(&mut self) -> io::Result<()> {
        let path = self.file_logger.path().to_owned();
        match open_log_file(&path) {
            Ok(()) => self.record_event(
                LogLevel::Info,
                &format!("Opened log file: {}", path.display()),
            ),
            Err(error) => self.report_error("Log file error", &error.to_string()),
        }
    }

    fn open_configuration(&mut self) -> io::Result<()> {
        if let Err(error) = open_configuration() {
            self.report_error("Configuration error", &error.to_string())?;
        }
        Ok(())
    }
    fn show_diagnostic_report(&mut self) -> Result<(), Box<dyn Error>> {
        match generate_diagnostic_report(self.active_config, &self.diagnostic_log) {
            Ok((path, report)) => {
                let path_text = path.display().to_string();
                match show_diagnose_window(&report) {
                    Ok(()) => {
                        self.record_event(
                            LogLevel::Info,
                            &format!("Diagnostic report saved: {path_text}"),
                        )?;
                        if let Err(error) = self.tray.update_status(
                            "",
                            &format!("Diagnose window closed; report saved: {path_text}"),
                            None,
                        ) {
                            self.record_event(
                                LogLevel::Error,
                                &format!("Tray update error: {error}"),
                            )?;
                        }
                    }
                    Err(error) => self.report_error(
                        "Diagnose window error",
                        &format!("{error}; report saved at {path_text}"),
                    )?,
                }
            }
            Err(error) => self.report_error("Diagnostic report error", &error.to_string())?,
        }
        Ok(())
    }

    fn toggle_start_on_boot(&mut self) -> Result<(), Box<dyn Error>> {
        match toggle_start_on_boot() {
            Ok(enabled) => self.tray.set_start_on_boot_checked(enabled),
            Err(error) => self.report_error("Start on Boot error", &error.to_string())?,
        }
        Ok(())
    }
    fn set_device_monitored(
        &mut self,
        identity: String,
        monitored: bool,
    ) -> Result<(), Box<dyn Error>> {
        let was_monitored = is_device_monitored(&self.monitored_devices, &identity);
        if was_monitored == monitored {
            self.tray
                .set_device_monitored_checked(&identity, was_monitored);
            return Ok(());
        }

        let save_result = match self.monitor_state.as_mut() {
            Some(state) => state.set_device_monitored(&identity, monitored),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "local SQLite monitoring state is unavailable",
            )
            .into()),
        };
        match save_result {
            Ok(()) => {
                self.monitored_devices.insert(identity.clone(), monitored);
                let refresh_failed = request_device_discovery(&self.commands, monitored).is_err();
                self.notifications
                    .retain_monitored_devices(&self.monitored_devices);
                if let Err(error) = self
                    .notifications
                    .persist_pending_alerts(self.monitor_state.as_mut())
                {
                    self.report_error("Notification state error", &error.to_string())?;
                }
                let device_name = self
                    .discovered_devices
                    .iter()
                    .find(|(device, _)| device.config_id() == identity)
                    .map(|(_, name)| name.clone())
                    .unwrap_or_else(|| "device".to_owned());
                self.record_event(
                    LogLevel::Info,
                    &format!(
                        "{} monitoring for {device_name}",
                        if monitored { "Enabled" } else { "Disabled" }
                    ),
                )?;
                retain_monitored_readings(
                    &mut self.latest_readings,
                    &mut self.pending_readings,
                    &self.monitored_devices,
                );
                self.refresh_visible_devices()?;
                if refresh_failed {
                    self.report_error("Discovery error", "device discovery poller has stopped")?;
                }
            }
            Err(error) => {
                self.tray
                    .set_device_monitored_checked(&identity, was_monitored);
                self.report_error("Device monitoring state error", &error.to_string())?;
            }
        }
        Ok(())
    }

    fn reset_volty(&mut self) -> Result<(), Box<dyn Error>> {
        let configuration_file = config_path()
            .ok_or_else(|| io::Error::other("could not determine the user configuration folder"))?;
        let local_state_file = local_state_path()?;
        if !confirm_reset(&configuration_file, &local_state_file)? {
            return Ok(());
        }

        self.record_event(
            LogLevel::Info,
            "Volty reset confirmed; deleting configuration and local device state",
        )?;
        let mut startup_reset_errors = Vec::new();
        if let Err(error) = reset_startup_preferences() {
            startup_reset_errors.push(error.to_string());
        }
        match start_on_boot_enabled() {
            Ok(enabled) => self.tray.set_start_on_boot_checked(enabled),
            Err(error) => startup_reset_errors.push(error.to_string()),
        }
        let _ = self.commands.send(PollCommand::Quit);
        if let Some(poller) = self.poller.take() {
            poller
                .join()
                .map_err(|_| io::Error::other("device polling worker panicked during reset"))?;
        }
        drop(self.monitor_state.take());
        remove_reset_files(&configuration_file, &local_state_file)?;
        self.diagnostic_log.clear();

        self.monitor_state = match DeviceState::open_local() {
            Ok(state) => Some(state),
            Err(error) => {
                self.report_error(
                    "Reset state error",
                    &format!("Could not recreate local state; using full device scans: {error}"),
                )?;
                None
            }
        };
        self.monitored_devices = match self.monitor_state.as_ref() {
            Some(state) => match state.monitored_devices() {
                Ok(devices) => devices,
                Err(error) => {
                    self.report_error(
                        "Reset state error",
                        &format!("Could not reload local monitoring preferences: {error}"),
                    )?;
                    BTreeMap::new()
                }
            },
            None => BTreeMap::new(),
        };
        (self.active_config, self.notification_config_error) = match AppConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (AppConfig::default(), Some(error.to_string())),
        };
        self.file_logger.set_level(self.active_config.log_level);
        self.notifications = DeviceNotifications::from_config(self.active_config)?;
        if let Some(error) = self.notification_config_error.as_deref() {
            self.record_event(
                LogLevel::Error,
                &format!("Configuration error after reset: {error}"),
            )?;
        }

        self.pending_readings.clear();
        self.latest_readings.clear();
        self.battery_eta_estimator.clear();
        self.discovered_devices.clear();
        self.readings_seen_this_poll.clear();
        self.poll_had_warning = false;
        self.config_reload_due = None;
        self.tray
            .set_battery_color_map(self.active_config.battery_color_map)?;
        let reset_status = self
            .notification_config_error
            .as_ref()
            .map(|error| format!("Configuration error: {error}"))
            .unwrap_or_else(|| "Searching for supported devices".to_owned());
        self.tray.reset_status(&reset_status)?;
        self.tray.update_monitored_devices(&[])?;

        let (commands, results, poller) = spawn_poll_worker(
            self.active_config,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        self.commands = commands;
        self.results = results;
        self.poller = Some(poller);
        self.record_event(
            LogLevel::Info,
            "Volty reset completed; fresh configuration and local state are active",
        )?;
        if !startup_reset_errors.is_empty() {
            self.report_error(
                "Startup preference reset error",
                &startup_reset_errors.join("; "),
            )?;
        }
        Ok(())
    }
    fn process_config_events(&mut self) -> io::Result<()> {
        while let Ok(event) = self.config_watch.events.try_recv() {
            match event {
                Ok(event)
                    if event
                        .paths
                        .iter()
                        .any(|path| path == &self.config_watch.config_file) =>
                {
                    self.record_event(LogLevel::Trace, "Configuration file change detected")?;
                    self.config_reload_due = Some(Instant::now() + CONFIG_RELOAD_DEBOUNCE);
                }
                Ok(_) => {}
                Err(error) => {
                    self.report_error("Configuration watch error", &error.to_string())?;
                }
            }
        }
        Ok(())
    }

    fn reload_config_if_due(&mut self) -> Result<(), Box<dyn Error>> {
        if !self
            .config_reload_due
            .is_some_and(|due| Instant::now() >= due)
        {
            return Ok(());
        }
        self.config_reload_due = None;
        if self.reload_notification_config()? {
            self.refresh_visible_devices()?;
            if self.commands.send(PollCommand::Discover).is_err() {
                self.report_error("Discovery error", "device discovery poller has stopped")?;
            }
        }
        Ok(())
    }

    fn reload_notification_config(&mut self) -> Result<bool, io::Error> {
        let config = match AppConfig::load() {
            Ok(config) => config,
            Err(error) => {
                self.report_config_error(error.to_string())?;
                return Ok(false);
            }
        };
        if let Err(error) = self.tray.set_battery_color_map(config.battery_color_map) {
            self.report_config_error(error.to_string())?;
            return Ok(false);
        }
        if let Err(error) = self.notifications.set_config(config) {
            self.report_config_error(error.to_string())?;
            return Ok(false);
        }

        let intervals_changed = config.poll_interval_seconds
            != self.active_config.poll_interval_seconds
            || config.retry_interval_seconds != self.active_config.retry_interval_seconds;
        if intervals_changed
            && self
                .commands
                .send(PollCommand::UpdateIntervals {
                    poll: Duration::from_secs(config.poll_interval_seconds),
                    retry: Duration::from_secs(config.retry_interval_seconds),
                })
                .is_err()
        {
            self.report_error(
                "Configuration error",
                "device poller has stopped; polling intervals were not applied",
            )?;
            return Ok(false);
        }

        let config_changed = self.active_config != config;
        self.file_logger.set_level(config.log_level);
        self.active_config = config;
        let recovered = self.notification_config_error.take().is_some();
        if recovered || config_changed {
            self.record_event(LogLevel::Info, "Configuration reloaded successfully")?;
        }
        Ok(recovered || config_changed)
    }

    fn report_config_error(&mut self, message: String) -> io::Result<()> {
        if self.notification_config_error.as_deref() != Some(message.as_str()) {
            self.report_error("Configuration error", &message)?;
            self.notification_config_error = Some(message);
        }
        Ok(())
    }

    fn refresh_visible_devices(&mut self) -> io::Result<()> {
        retain_monitored_readings(
            &mut self.latest_readings,
            &mut self.pending_readings,
            &self.monitored_devices,
        );
        update_monitored_device_menu(
            &mut self.tray,
            &self.discovered_devices,
            &self.monitored_devices,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        update_visible_devices(
            &mut self.tray,
            &self.latest_readings,
            &self.monitored_devices,
            self.notification_config_error.as_deref(),
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )
    }
    fn process_poll_results(&mut self) -> Result<(), Box<dyn Error>> {
        #[allow(clippy::while_let_loop)]
        loop {
            match self.results.try_recv() {
                Ok(update) => self.handle_poll_update(update)?,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    self.record_event(
                        LogLevel::Error,
                        "Device polling worker stopped unexpectedly",
                    )?;
                    return Err("device polling worker stopped unexpectedly".into());
                }
            }
        }
    }

    fn handle_poll_update(&mut self, update: PollUpdate) -> Result<(), Box<dyn Error>> {
        match update {
            PollUpdate::Reading(reading) => self.handle_reading(reading),
            PollUpdate::DeviceName {
                identity,
                name,
                error,
            } => self.handle_device_name(identity, name, error),
            PollUpdate::Warning(message) => {
                self.poll_had_warning = true;
                self.record_event(LogLevel::Warn, &message)?;
                Ok(())
            }
            PollUpdate::Trace(message) => {
                self.record_event(LogLevel::Trace, &message)?;
                Ok(())
            }
            PollUpdate::Finished(result) => self.finish_poll(result),
        }
    }

    fn handle_reading(&mut self, mut reading: DeviceReading) -> Result<(), Box<dyn Error>> {
        let identity = reading.identity.clone();
        self.discovered_devices
            .insert(identity.clone(), identity.display_name(&reading.name));
        self.readings_seen_this_poll.insert(identity.clone());
        if is_device_monitored(&self.monitored_devices, &identity.config_id()) {
            reading.estimated_time_remaining = self.battery_eta_estimator.estimate(
                &identity,
                &reading.battery,
                Duration::from_secs(self.active_config.battery_eta_window_minutes * 60),
            );
            self.pending_readings.push(reading);
            update_tray_devices(
                &mut self.tray,
                &self.pending_readings,
                self.notification_config_error.as_deref(),
                &mut self.diagnostic_log,
                &mut self.file_logger,
                self.started,
            )?;
            let reading = self
                .pending_readings
                .last_mut()
                .ok_or_else(|| io::Error::other("pending battery reading disappeared"))?;
            process_device_reading(
                &mut self.notifications,
                self.monitor_state.as_mut(),
                reading,
                &mut self.diagnostic_log,
                &mut self.file_logger,
                self.started,
            )?;
            self.latest_readings.insert(identity, reading.clone());
            if reading.notification_error.is_some() {
                update_tray_devices(
                    &mut self.tray,
                    &self.pending_readings,
                    self.notification_config_error.as_deref(),
                    &mut self.diagnostic_log,
                    &mut self.file_logger,
                    self.started,
                )?;
            }
        } else {
            self.battery_eta_estimator.remove(&identity);
        }
        update_monitored_device_menu(
            &mut self.tray,
            &self.discovered_devices,
            &self.monitored_devices,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        Ok(())
    }

    fn handle_device_name(
        &mut self,
        identity: DeviceIdentity,
        name: String,
        error: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        let device_name = identity.display_name(&name);
        let is_discovered = self.discovered_devices.contains_key(&identity);
        self.discovered_devices
            .insert(identity.clone(), device_name.clone());
        let has_pending_reading = if let Some(reading) = self
            .pending_readings
            .iter_mut()
            .find(|reading| reading.identity == identity)
        {
            reading.name = name.clone();
            true
        } else {
            false
        };
        if !has_pending_reading && !is_discovered {
            self.record_event(
                LogLevel::Warn,
                &format!(
                    "Received a device name without a pending battery reading for {device_name}"
                ),
            )?;
        }
        if let Some(reading) = self.latest_readings.get_mut(&identity) {
            reading.name = name;
        }
        update_monitored_device_menu(
            &mut self.tray,
            &self.discovered_devices,
            &self.monitored_devices,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        if let Some(error) = error {
            self.record_event(
                LogLevel::Debug,
                &format!(
                    "Protocol name discovery failed for {device_name}; using interface label: {error}"
                ),
            )?;
        }
        Ok(())
    }

    fn finish_poll(&mut self, result: Result<(), String>) -> Result<(), Box<dyn Error>> {
        self.record_event(LogLevel::Trace, "Received device polling result")?;
        if !self.poll_had_warning {
            self.battery_eta_estimator
                .retain_devices(&self.readings_seen_this_poll);
            self.discovered_devices
                .retain(|identity, _| self.readings_seen_this_poll.contains(identity));
            self.latest_readings
                .retain(|identity, _| self.readings_seen_this_poll.contains(identity));
            update_monitored_device_menu(
                &mut self.tray,
                &self.discovered_devices,
                &self.monitored_devices,
                &mut self.diagnostic_log,
                &mut self.file_logger,
                self.started,
            )?;
        }
        self.readings_seen_this_poll.clear();
        self.poll_had_warning = false;
        apply_poll_result(
            &mut self.tray,
            std::mem::take(&mut self.pending_readings),
            result,
            self.notification_config_error.as_deref(),
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        Ok(())
    }

    fn record_event(&mut self, level: LogLevel, message: &str) -> io::Result<()> {
        record_event(
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
            level,
            message,
        )
    }

    fn report_error(&mut self, title: &str, message: &str) -> io::Result<()> {
        report_error(
            &mut self.tray,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
            title,
            message,
        )
    }
}

pub(super) fn report_error(
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

pub(super) fn record_event(
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
