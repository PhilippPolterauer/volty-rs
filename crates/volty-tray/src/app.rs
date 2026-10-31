mod battery_eta;
mod device_ui;
mod labels;
mod notifications;
mod poll;
mod session;
mod startup;
mod wake;

use device_ui::{
    apply_poll_result, is_device_monitored, update_monitored_device_menu, update_tray_devices,
    update_visible_devices,
};
use labels::{append_config_error, battery_is_full, battery_status_label};
use notifications::{process_device_reading, DeviceNotifications};
use poll::PollUpdate;
use session::DeviceSession;
use startup::{
    load_startup_state, open_configuration, open_log_file, process_windows_messages,
    remove_reset_files, toggle_start_on_boot, watch_config_file, ConfigWatch,
};
use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    io,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};
use volty_battery::{
    BatteryState, BatteryStatus, DeviceId, DeviceLabel, ProviderEvent, SourceReading,
};

use crate::{
    about::show_about_window,
    config::{config_path, AppConfig, LogLevel},
    diagnose::show_diagnose_window,
    history::{unix_now, HistorySample, HistoryStore},
    logging::FileLogger,
    report::generate_diagnostic_report,
    state::{local_state_path, DeviceState, TrayIconStyle},
    tray::{
        confirm_reset, reset_startup_preferences, start_on_boot_enabled, TrayAction, TrayController,
    },
};

const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const UI_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const MAX_DIAGNOSTIC_LOG_ENTRIES: usize = 200;

#[derive(Clone)]
struct DeviceReading {
    id: DeviceId,
    label: DeviceLabel,
    battery: BatteryStatus,
    estimated_time_remaining: Option<Duration>,
    notification_error: Option<String>,
    observed_at: Instant,
}

struct App {
    started: Instant,
    diagnostic_log: VecDeque<String>,
    file_logger: FileLogger,
    tray: TrayController,
    config_watch: ConfigWatch,
    monitor_state: Option<DeviceState>,
    monitored_devices: BTreeMap<String, bool>,
    tray_icon_style: TrayIconStyle,
    pinned_device: Option<String>,
    active_config: AppConfig,
    notification_config_error: Option<String>,
    notifications: DeviceNotifications,
    session: DeviceSession,
    config_reload_due: Option<Instant>,
    next_ui_refresh: Instant,
    history: Option<HistoryStore>,
    history_error: Option<String>,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let mut app = App::start()?;
    let result = app.run_loop();
    let flushed = app.flush_discharge_rates(true);
    result.and(flushed.map_err(Into::into))
}
impl App {
    fn start() -> Result<Self, Box<dyn Error>> {
        wake::init()?;
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
        let history = match HistoryStore::open_local() {
            Ok(store) => Some(store),
            Err(error) => {
                record_event(
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    LogLevel::Warn,
                    &format!(
                        "Could not open battery history; samples will not be recorded this run: {error}"
                    ),
                )?;
                None
            }
        };
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
        tray.set_tray_icon_style(startup.tray_display.icon_style);
        tray.set_pinned_device_checked(startup.tray_display.pinned_device.as_deref());
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

        let mut notifications = DeviceNotifications::from_config(startup.active_config.clone())?;
        if let Some(state) = startup.monitor_state.as_ref() {
            match state.notified_thresholds() {
                Ok(alerts) => notifications.restore_persisted_alerts(alerts),
                Err(error) => record_event(
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    LogLevel::Warn,
                    &format!(
                        "Could not load persisted low-battery notification thresholds: {error}"
                    ),
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
        let mut session = DeviceSession::start(
            &startup.active_config,
            &mut diagnostic_log,
            &mut file_logger,
            started,
        )?;
        if let Some(state) = startup.monitor_state.as_ref() {
            match state.discharge_rates() {
                Ok(rates) => session.load_discharge_rates(rates),
                Err(error) => record_event(
                    &mut diagnostic_log,
                    &mut file_logger,
                    started,
                    LogLevel::Warn,
                    &format!("Could not load saved discharge rates: {error}"),
                )?,
            }
        }

        Ok(Self {
            started,
            diagnostic_log,
            file_logger,
            tray,
            config_watch,
            monitor_state: startup.monitor_state,
            monitored_devices: startup.monitored_devices,
            tray_icon_style: startup.tray_display.icon_style,
            pinned_device: startup.tray_display.pinned_device,
            active_config: startup.active_config,
            notification_config_error: startup.notification_config_error,
            notifications,
            session,
            config_reload_due: None,
            next_ui_refresh: Instant::now() + UI_REFRESH_INTERVAL,
            history,
            history_error: None,
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
            self.refresh_ui_if_due()?;
            let ui_wait = self
                .next_ui_refresh
                .saturating_duration_since(Instant::now());
            let wait = self.config_reload_due.map_or(ui_wait, |due| {
                due.saturating_duration_since(Instant::now()).min(ui_wait)
            });
            wake::wait(Some(wait))?;
        }
    }

    /// Repaints the tray labels even when no device reports, so last-seen ages
    /// and stale markers keep advancing.
    fn refresh_ui_if_due(&mut self) -> io::Result<()> {
        let now = Instant::now();
        if now < self.next_ui_refresh {
            return Ok(());
        }
        self.next_ui_refresh = now + UI_REFRESH_INTERVAL;
        self.refresh_visible_devices()
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
            } => self.set_device_monitored(&identity, monitored)?,
            TrayAction::SetTrayIconStyle { style } => self.set_tray_icon_style(style)?,
            TrayAction::SetPinnedDevice { identity } => {
                self.set_pinned_device(identity.as_deref())?
            }
            TrayAction::About => {
                if let Err(error) = show_about_window() {
                    self.report_error("About window error", &error.to_string())?;
                }
            }
            TrayAction::ResetVolty => self.reset_volty()?,
            TrayAction::Quit => {
                self.record_event(LogLevel::Info, "Volty shutting down")?;
                self.session.request_quit();
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
        if self.session.discover().is_err() {
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
        match generate_diagnostic_report(&self.active_config, &self.diagnostic_log) {
            Ok(export) => {
                let path_text = export.report_path.display().to_string();
                let capture_status = match (&export.capture_path, &export.capture_error) {
                    (Some(path), _) => format!("; capture saved: {}", path.display()),
                    (None, Some(error)) => format!("; capture not created: {error}"),
                    (None, None) => String::new(),
                };
                if let Some(error) = &export.capture_error {
                    self.record_event(
                        LogLevel::Warn,
                        &format!("Hardware capture not created: {error}"),
                    )?;
                }
                match show_diagnose_window(&export.report) {
                    Ok(()) => {
                        self.record_event(
                            LogLevel::Info,
                            &format!("Diagnostic report saved: {path_text}{capture_status}"),
                        )?;
                        if let Err(error) = self.tray.update_status(
                            "",
                            &format!(
                                "Diagnose window closed; report saved: {path_text}{capture_status}"
                            ),
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
                        &format!("{error}; report saved at {path_text}{capture_status}"),
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
        identity: &str,
        monitored: bool,
    ) -> Result<(), Box<dyn Error>> {
        let was_monitored = is_device_monitored(&self.monitored_devices, identity);
        if was_monitored == monitored {
            self.tray
                .set_device_monitored_checked(identity, was_monitored);
            return Ok(());
        }

        let save_result = match self.monitor_state.as_mut() {
            Some(state) => state.set_device_monitored(identity, monitored),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "local SQLite monitoring state is unavailable",
            )
            .into()),
        };
        match save_result {
            Ok(()) => {
                self.monitored_devices
                    .insert(identity.to_owned(), monitored);
                let refresh_failed = self.session.discover_if_monitored(monitored).is_err();
                self.notifications
                    .retain_monitored_devices(&self.monitored_devices);
                if let Err(error) = self
                    .notifications
                    .persist_pending_alerts(self.monitor_state.as_mut())
                {
                    self.report_error("Notification state error", &error.to_string())?;
                }
                let device_name = self
                    .session
                    .device_name(identity)
                    .unwrap_or("device")
                    .to_owned();
                self.record_event(
                    LogLevel::Info,
                    &format!(
                        "{} monitoring for {device_name}",
                        if monitored { "Enabled" } else { "Disabled" }
                    ),
                )?;
                self.session.retain_monitored(&self.monitored_devices);
                self.refresh_visible_devices()?;
                if refresh_failed {
                    self.report_error("Discovery error", "device discovery poller has stopped")?;
                }
            }
            Err(error) => {
                self.tray
                    .set_device_monitored_checked(identity, was_monitored);
                self.report_error("Device monitoring state error", &error.to_string())?;
            }
        }
        Ok(())
    }

    fn set_tray_icon_style(&mut self, style: TrayIconStyle) -> Result<(), Box<dyn Error>> {
        if style == self.tray_icon_style {
            self.tray.set_tray_icon_style(style);
            return Ok(());
        }

        let save_result = match self.monitor_state.as_mut() {
            Some(state) => state.set_tray_icon_style(style),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "local SQLite display state is unavailable",
            )
            .into()),
        };
        match save_result {
            Ok(()) => {
                self.tray_icon_style = style;
                self.tray.set_tray_icon_style(style);
                self.record_event(
                    LogLevel::Info,
                    &format!("Tray icon style set to {}", style.as_str()),
                )?;
                self.refresh_visible_devices()?;
            }
            Err(error) => {
                self.tray.set_tray_icon_style(self.tray_icon_style);
                self.report_error("Tray display state error", &error.to_string())?;
            }
        }
        Ok(())
    }

    fn set_pinned_device(&mut self, identity: Option<&str>) -> Result<(), Box<dyn Error>> {
        if identity == self.pinned_device.as_deref() {
            self.tray
                .set_pinned_device_checked(self.pinned_device.as_deref());
            return Ok(());
        }

        let save_result = match self.monitor_state.as_mut() {
            Some(state) => match identity {
                Some(identity) => state.set_pinned_device(identity),
                None => state.clear_pinned_device(),
            },
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "local SQLite display state is unavailable",
            )
            .into()),
        };
        match save_result {
            Ok(()) => {
                self.pinned_device = identity.map(str::to_owned);
                self.tray
                    .set_pinned_device_checked(self.pinned_device.as_deref());
                let message = match identity {
                    Some(identity) => {
                        let name = self
                            .session
                            .device_name(identity)
                            .unwrap_or(identity)
                            .to_owned();
                        format!("Tray icon pinned to {name}")
                    }
                    None => "Tray icon shows the lowest battery".to_owned(),
                };
                self.record_event(LogLevel::Info, &message)?;
                self.refresh_visible_devices()?;
            }
            Err(error) => {
                self.tray
                    .set_pinned_device_checked(self.pinned_device.as_deref());
                self.report_error("Tray display state error", &error.to_string())?;
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
        self.session.shutdown("during reset")?;
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
        self.tray_icon_style = TrayIconStyle::Battery;
        self.pinned_device = None;
        (self.active_config, self.notification_config_error) = match AppConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (AppConfig::default(), Some(error.to_string())),
        };
        self.file_logger.set_level(self.active_config.log_level);
        self.notifications = DeviceNotifications::from_config(self.active_config.clone())?;
        if let Some(error) = self.notification_config_error.as_deref() {
            self.record_event(
                LogLevel::Error,
                &format!("Configuration error after reset: {error}"),
            )?;
        }

        self.config_reload_due = None;
        self.tray
            .set_battery_color_map(self.active_config.battery_color_map)?;
        self.tray.set_tray_icon_style(TrayIconStyle::Battery);
        self.tray.set_pinned_device_checked(None);
        let reset_status = self.notification_config_error.as_ref().map_or_else(
            || "Searching for supported devices".to_owned(),
            |error| format!("Configuration error: {error}"),
        );
        self.tray.reset_status(&reset_status)?;
        self.tray.update_monitored_devices(&[])?;

        self.session.restart(
            &self.active_config,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
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
            if self.session.discover().is_err() {
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
        if let Err(error) = self.notifications.set_config(&config) {
            self.report_config_error(error.to_string())?;
            return Ok(false);
        }

        let intervals_changed = config.poll_interval_seconds
            != self.active_config.poll_interval_seconds
            || config.retry_interval_seconds != self.active_config.retry_interval_seconds;
        if intervals_changed
            && self
                .session
                .update_intervals(
                    Duration::from_secs(config.poll_interval_seconds),
                    Duration::from_secs(config.retry_interval_seconds),
                )
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

    fn stale_after(&self) -> Duration {
        Duration::from_secs(self.active_config.stale_after_minutes * 60)
    }

    fn refresh_visible_devices(&mut self) -> io::Result<()> {
        let stale_after = self.stale_after();
        self.session.retain_monitored(&self.monitored_devices);
        update_monitored_device_menu(
            &mut self.tray,
            &self.session.discovered_devices,
            &self.monitored_devices,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        update_visible_devices(
            &mut self.tray,
            &self.session.latest_readings,
            &self.monitored_devices,
            self.pinned_device.as_deref(),
            self.notification_config_error.as_deref(),
            stale_after,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )
    }
    fn process_poll_results(&mut self) -> Result<(), Box<dyn Error>> {
        #[allow(clippy::while_let_loop)]
        loop {
            match self.session.try_recv() {
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
            PollUpdate::Event(ProviderEvent::Reading(reading)) => self.handle_reading(reading),
            PollUpdate::Event(ProviderEvent::DeviceName { id, label, error }) => {
                self.handle_device_name(&id, label, error)
            }
            PollUpdate::Event(ProviderEvent::Warning(message)) => {
                self.session.mark_warning();
                self.record_event(LogLevel::Warn, &message)?;
                Ok(())
            }
            PollUpdate::Event(ProviderEvent::Trace(message)) => {
                self.record_event(LogLevel::Trace, &message)?;
                Ok(())
            }
            PollUpdate::Finished(result) => self.finish_poll(result),
        }
    }

    fn handle_reading(&mut self, reading: SourceReading) -> Result<(), Box<dyn Error>> {
        let id = reading.id.clone();
        let monitored = is_device_monitored(&self.monitored_devices, id.as_str());
        let eta_window = Duration::from_secs(self.active_config.battery_eta_window_minutes * 60);
        let stale_after = self.stale_after();
        if self.session.observe(reading, monitored, eta_window) {
            let newest_reading = self.session.pending_readings.last().cloned();
            if let Some(newest_reading) = newest_reading {
                self.record_history_sample(&newest_reading)?;
            }
            update_tray_devices(
                &mut self.tray,
                &self.session.pending_readings,
                self.pinned_device.as_deref(),
                self.notification_config_error.as_deref(),
                stale_after,
                &mut self.diagnostic_log,
                &mut self.file_logger,
                self.started,
            )?;
            let reading = self
                .session
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
            self.session.latest_readings.insert(id, reading.clone());
            if reading.notification_error.is_some() {
                update_tray_devices(
                    &mut self.tray,
                    &self.session.pending_readings,
                    self.pinned_device.as_deref(),
                    self.notification_config_error.as_deref(),
                    stale_after,
                    &mut self.diagnostic_log,
                    &mut self.file_logger,
                    self.started,
                )?;
            }
        }
        update_monitored_device_menu(
            &mut self.tray,
            &self.session.discovered_devices,
            &self.monitored_devices,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        Ok(())
    }

    fn handle_device_name(
        &mut self,
        id: &DeviceId,
        label: DeviceLabel,
        error: Option<String>,
    ) -> Result<(), Box<dyn Error>> {
        let renamed = self.session.rename(id, label);
        let device_name = renamed.display;
        if !renamed.had_pending_reading && !renamed.was_discovered {
            self.record_event(
                LogLevel::Warn,
                &format!(
                    "Received a device name without a pending battery reading for {device_name}"
                ),
            )?;
        }
        update_monitored_device_menu(
            &mut self.tray,
            &self.session.discovered_devices,
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
        self.flush_discharge_rates(false)?;
        let (readings, pruned) = self.session.finish_poll();
        if pruned {
            update_monitored_device_menu(
                &mut self.tray,
                &self.session.discovered_devices,
                &self.monitored_devices,
                &mut self.diagnostic_log,
                &mut self.file_logger,
                self.started,
            )?;
        }
        let stale_after = self.stale_after();
        apply_poll_result(
            &mut self.tray,
            &readings,
            self.pinned_device.as_deref(),
            result,
            self.notification_config_error.as_deref(),
            stale_after,
            &mut self.diagnostic_log,
            &mut self.file_logger,
            self.started,
        )?;
        Ok(())
    }

    /// Saves discharge rates that changed; `force` also saves rates that only
    /// gained watched time, which is done on exit.
    fn flush_discharge_rates(&mut self, force: bool) -> io::Result<()> {
        let rates = self.session.take_unsaved_discharge_rates(force);
        if rates.is_empty() {
            return Ok(());
        }
        let Some(state) = self.monitor_state.as_mut() else {
            return Ok(());
        };
        let saved = state
            .save_discharge_rates(rates.iter().map(|(id, rate)| (id.as_str(), *rate)))
            .map_err(|error| error.to_string());
        match saved {
            Ok(()) => Ok(()),
            Err(error) => self.record_event(
                LogLevel::Warn,
                &format!("Could not save discharge rates: {error}"),
            ),
        }
    }

    /// Records the freshest reading in the local battery history. A failure is
    /// logged but never interrupts polling; identical errors are logged once.
    fn record_history_sample(&mut self, reading: &DeviceReading) -> io::Result<()> {
        if self.history.is_none() {
            return Ok(());
        }
        let recorded_at = match unix_now() {
            Ok(recorded_at) => recorded_at,
            Err(error) => {
                return self.report_history_error(format!(
                    "Could not timestamp battery history for {}: {error}",
                    reading.label.display()
                ));
            }
        };
        let sample = HistorySample {
            identity: reading.id.as_str().to_owned(),
            name: reading.label.display(),
            recorded_at,
            percent: reading.battery.level_percent,
            charging: matches!(
                reading.battery.state,
                BatteryState::Charging
                    | BatteryState::ChargingNearlyFull
                    | BatteryState::ChargingSlow
            ),
            voltage_mv: reading.battery.voltage_mv,
        };
        let result = match self.history.as_mut() {
            Some(history) => history.record(&sample),
            None => return Ok(()),
        };
        match result {
            Ok(_) => {
                self.history_error = None;
                Ok(())
            }
            Err(error) => self.report_history_error(format!(
                "Could not record battery history for {}: {error}",
                sample.name
            )),
        }
    }

    fn report_history_error(&mut self, message: String) -> io::Result<()> {
        if self.history_error.as_deref() == Some(message.as_str()) {
            return Ok(());
        }
        self.record_event(LogLevel::Warn, &message)?;
        self.history_error = Some(message);
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
