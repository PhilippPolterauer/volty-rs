use super::{
    battery_eta::BatteryEtaEstimator,
    device_ui::retain_monitored_readings,
    poll::{request_device_discovery, spawn_poll_worker, PollCommand, PollUpdate, PollWorker},
    AppConfig, DeviceReading, FileLogger,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    error::Error,
    io,
    sync::mpsc::{self, Receiver, SendError, TryRecvError},
    thread,
    time::{Duration, Instant},
};
use volty_battery::{DeviceId, DeviceLabel, SourceReading};

pub(super) struct Renamed {
    pub display: String,
    pub was_discovered: bool,
    pub had_pending_reading: bool,
}

/// Everything the tray knows about the devices behind the battery providers:
/// the background poller, the readings it produced and which devices exist.
/// The UI layer consumes this and never talks to a provider directly.
pub(super) struct DeviceSession {
    commands: mpsc::Sender<PollCommand>,
    results: Receiver<PollUpdate>,
    poller: Option<thread::JoinHandle<()>>,
    pub(super) pending_readings: Vec<DeviceReading>,
    pub(super) latest_readings: HashMap<DeviceId, DeviceReading>,
    pub(super) discovered_devices: HashMap<DeviceId, String>,
    eta_estimator: BatteryEtaEstimator,
    seen_this_poll: HashSet<DeviceId>,
    poll_had_warning: bool,
}

impl DeviceSession {
    pub(super) fn start(
        config: AppConfig,
        diagnostic_log: &mut VecDeque<String>,
        file_logger: &mut FileLogger,
        started: Instant,
    ) -> Result<Self, Box<dyn Error>> {
        let (commands, results, poller) =
            spawn_poll_worker(config, diagnostic_log, file_logger, started)?;
        Ok(Self::with_worker((commands, results, poller)))
    }

    fn with_worker((commands, results, poller): PollWorker) -> Self {
        Self {
            commands,
            results,
            poller: Some(poller),
            pending_readings: Vec::new(),
            latest_readings: HashMap::new(),
            discovered_devices: HashMap::new(),
            eta_estimator: BatteryEtaEstimator::default(),
            seen_this_poll: HashSet::new(),
            poll_had_warning: false,
        }
    }

    pub(super) fn try_recv(&self) -> Result<PollUpdate, TryRecvError> {
        self.results.try_recv()
    }

    pub(super) fn discover(&self) -> Result<(), SendError<PollCommand>> {
        self.commands.send(PollCommand::Discover)
    }

    pub(super) fn discover_if_monitored(
        &self,
        monitored: bool,
    ) -> Result<(), SendError<PollCommand>> {
        request_device_discovery(&self.commands, monitored)
    }

    pub(super) fn update_intervals(
        &self,
        poll: Duration,
        retry: Duration,
    ) -> Result<(), SendError<PollCommand>> {
        self.commands
            .send(PollCommand::UpdateIntervals { poll, retry })
    }

    pub(super) fn request_quit(&self) {
        let _ = self.commands.send(PollCommand::Quit);
    }

    /// Replace this session with a fresh one. Call [`Self::shutdown`] first.
    pub(super) fn restart(
        &mut self,
        config: AppConfig,
        diagnostic_log: &mut VecDeque<String>,
        file_logger: &mut FileLogger,
        started: Instant,
    ) -> Result<(), Box<dyn Error>> {
        *self = Self::start(config, diagnostic_log, file_logger, started)?;
        Ok(())
    }

    /// Stop the poller and wait until it has released its devices and state.
    pub(super) fn shutdown(&mut self, context: &str) -> io::Result<()> {
        self.request_quit();
        if let Some(poller) = self.poller.take() {
            poller.join().map_err(|_| {
                io::Error::other(format!("device polling worker panicked {context}"))
            })?;
        }
        Ok(())
    }

    pub(super) fn mark_warning(&mut self) {
        self.poll_had_warning = true;
    }

    /// Register a reading. Returns `true` when the device is monitored and the
    /// reading was queued for display.
    pub(super) fn observe(
        &mut self,
        reading: SourceReading,
        monitored: bool,
        eta_window: Duration,
    ) -> bool {
        self.discovered_devices
            .insert(reading.id.clone(), reading.label.display());
        self.seen_this_poll.insert(reading.id.clone());
        if !monitored {
            self.eta_estimator.remove(&reading.id);
            return false;
        }
        let estimated_time_remaining =
            self.eta_estimator
                .estimate(&reading.id, &reading.battery, eta_window);
        self.pending_readings.push(DeviceReading {
            id: reading.id,
            label: reading.label,
            battery: reading.battery,
            estimated_time_remaining,
            notification_error: None,
        });
        true
    }

    pub(super) fn rename(&mut self, id: &DeviceId, label: DeviceLabel) -> Renamed {
        let display = label.display();
        let was_discovered = self.discovered_devices.contains_key(id);
        self.discovered_devices.insert(id.clone(), display.clone());
        let pending = self
            .pending_readings
            .iter_mut()
            .find(|reading| reading.id == *id);
        let had_pending_reading = pending.is_some();
        if let Some(reading) = pending {
            reading.label = label.clone();
        }
        if let Some(reading) = self.latest_readings.get_mut(id) {
            reading.label = label;
        }
        Renamed {
            display,
            was_discovered,
            had_pending_reading,
        }
    }

    pub(super) fn device_name(&self, config_id: &str) -> Option<&str> {
        self.discovered_devices
            .iter()
            .find(|(id, _)| id.as_str() == config_id)
            .map(|(_, name)| name.as_str())
    }

    pub(super) fn retain_monitored(&mut self, monitored_devices: &BTreeMap<String, bool>) {
        retain_monitored_readings(
            &mut self.latest_readings,
            &mut self.pending_readings,
            monitored_devices,
        );
    }

    /// Close out a poll and hand back the readings it produced. Devices that
    /// did not answer are forgotten, unless the poll had warnings: a partial
    /// poll must not make a device vanish.
    pub(super) fn finish_poll(&mut self) -> (Vec<DeviceReading>, bool) {
        let pruned = !self.poll_had_warning;
        if pruned {
            self.eta_estimator.retain_devices(&self.seen_this_poll);
            self.discovered_devices
                .retain(|id, _| self.seen_this_poll.contains(id));
            self.latest_readings
                .retain(|id, _| self.seen_this_poll.contains(id));
        }
        self.seen_this_poll.clear();
        self.poll_had_warning = false;
        (std::mem::take(&mut self.pending_readings), pruned)
    }
}
