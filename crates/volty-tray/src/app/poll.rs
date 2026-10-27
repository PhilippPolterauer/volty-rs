use super::{
    device_cache::DeviceNameCache, record_event, AppConfig, DeviceIdentity, DeviceReading,
    DeviceState, FileLogger, LogLevel,
};
use std::{
    collections::VecDeque,
    error::Error,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};
pub(super) enum PollCommand {
    Discover,
    UpdateIntervals { poll: Duration, retry: Duration },
    Quit,
}
pub(super) type PollResult = Result<(), String>;
pub(super) type PollWorker = (
    mpsc::Sender<PollCommand>,
    Receiver<PollUpdate>,
    thread::JoinHandle<()>,
);
pub(super) enum PollUpdate {
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
pub(super) fn spawn_poll_worker(
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
                &commands_rx,
                &results_tx,
                poll_interval,
                retry_interval,
                device_state,
            )
        })?;
    Ok((commands_tx, results_rx, poller))
}
pub(super) fn poll_worker(
    commands: &mpsc::Receiver<PollCommand>,
    results: &mpsc::Sender<PollUpdate>,
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
        move |results, full_scan| {
            query_devices(results, &mut device_state, &mut name_cache, full_scan)
        },
    );
}
pub(super) fn poll_worker_with_query(
    commands: &mpsc::Receiver<PollCommand>,
    results: &mpsc::Sender<PollUpdate>,
    mut poll_interval: Duration,
    mut retry_interval: Duration,
    mut query: impl FnMut(&mpsc::Sender<PollUpdate>, bool) -> PollResult,
) {
    let mut full_scan = true;
    loop {
        let reading = query(results, full_scan);
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
                Ok(PollCommand::Discover) => {
                    full_scan = true;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    full_scan = false;
                    break;
                }
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

pub(super) fn request_device_discovery(
    commands: &mpsc::Sender<PollCommand>,
    monitored: bool,
) -> Result<(), mpsc::SendError<PollCommand>> {
    if monitored {
        commands.send(PollCommand::Discover)
    } else {
        Ok(())
    }
}
pub(super) fn query_devices(
    results: &mpsc::Sender<PollUpdate>,
    device_state: &mut Option<DeviceState>,
    name_cache: &mut DeviceNameCache,
    full_scan: bool,
) -> PollResult {
    let mut query = DeviceQuery::new(results, device_state, name_cache, full_scan);
    query.query_hid_interfaces()?;
    #[cfg(feature = "bluetooth")]
    query.query_bluetooth_devices()?;
    query.finish()
}

struct DeviceQuery<'a> {
    results: &'a mpsc::Sender<PollUpdate>,
    device_state: &'a mut Option<DeviceState>,
    name_cache: &'a mut DeviceNameCache,
    full_scan: bool,
    errors: Vec<String>,
    supported_interfaces: usize,
    reading_count: usize,
}

impl<'a> DeviceQuery<'a> {
    fn new(
        results: &'a mpsc::Sender<PollUpdate>,
        device_state: &'a mut Option<DeviceState>,
        name_cache: &'a mut DeviceNameCache,
        full_scan: bool,
    ) -> Self {
        Self {
            results,
            device_state,
            name_cache,
            full_scan,
            errors: Vec::new(),
            supported_interfaces: 0,
            reading_count: 0,
        }
    }

    fn query_hid_interfaces(&mut self) -> PollResult {
        use volty_hid::{enumerate_hid_interfaces, resolve_mouse_model};

        let interfaces = match enumerate_hid_interfaces() {
            Ok(interfaces) => interfaces,
            Err(error) => {
                self.errors.push(format!("HID discovery: {error}"));
                Vec::new()
            }
        };
        for interface in interfaces {
            match resolve_mouse_model(&interface) {
                Ok(Some(mouse)) => {
                    self.supported_interfaces += 1;
                    let protocol = mouse.protocol();
                    let fallback_name = interface
                        .product_string
                        .as_deref()
                        .filter(|name| !name.trim().is_empty())
                        .map_or_else(|| mouse.name().to_owned(), str::to_owned);
                    self.send(PollUpdate::Trace(format!(
                        "HID discovery matched {fallback_name:?} to {} using {} (VID=0x{:04x}, PID=0x{:04x}, usage_page={:?}, usage={:?}, interface={})",
                        mouse.name(),
                        protocol.name(),
                        interface.vendor_id,
                        interface.product_id,
                        interface.usage_page,
                        interface.usage,
                        interface.interface_number
                    )))?;
                    let interface_path = interface.path.to_string_lossy().into_owned();
                    let (known_indices, device_indices) =
                        self.device_indices(&interface_path, protocol.device_indices())?;
                    for device_index in device_indices {
                        self.query_hid_device(
                            &interface,
                            protocol,
                            &fallback_name,
                            &interface_path,
                            device_index,
                            &known_indices,
                        )?;
                    }
                }
                Ok(None) => self.send(PollUpdate::Trace(format!(
                    "HID discovery skipped an unsupported interface (VID=0x{:04x}, PID=0x{:04x}, usage_page={:?}, usage={:?}, interface={}): no supported HID protocol matched its interface metadata",
                    interface.vendor_id,
                    interface.product_id,
                    interface.usage_page,
                    interface.usage,
                    interface.interface_number
                )))?,
                Err(error) => self.errors.push(format!(
                    "Ambiguous HID backend match for {:04x}:{:04x}: {error}",
                    interface.vendor_id, interface.product_id
                )),
            }
        }
        Ok(())
    }

    fn send(&self, update: PollUpdate) -> PollResult {
        self.results
            .send(update)
            .map_err(|_| "device polling result receiver disconnected".to_owned())
    }
}

impl DeviceQuery<'_> {
    fn device_indices(
        &mut self,
        interface_path: &str,
        candidates: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut known_indices = match self
            .device_state
            .as_ref()
            .map(|state| state.known_device_indices(interface_path, candidates))
        {
            Some(Ok(indices)) => indices,
            Some(Err(error)) => {
                *self.device_state = None;
                self.send(PollUpdate::Warning(format!(
                    "Could not read discovery state; continuing with a full scan: {error}"
                )))?;
                Vec::new()
            }
            None => Vec::new(),
        };
        for device_index in self
            .name_cache
            .known_device_indices(interface_path, candidates)
        {
            if !known_indices.contains(&device_index) {
                known_indices.push(device_index);
            }
        }

        let device_indices = if !self.full_scan && !known_indices.is_empty() {
            known_indices.clone()
        } else {
            match self
                .device_state
                .as_ref()
                .map(|state| state.ordered_device_indices(interface_path, candidates))
            {
                Some(Ok(indices)) => indices,
                Some(Err(error)) => {
                    *self.device_state = None;
                    self.send(PollUpdate::Warning(format!(
                        "Could not read discovery state; continuing with a full scan: {error}"
                    )))?;
                    candidates.to_vec()
                }
                None => candidates.to_vec(),
            }
        };
        Ok((known_indices, device_indices))
    }

    fn query_hid_device(
        &mut self,
        interface: &volty_hid::HidInterfaceInfo,
        protocol: &dyn volty_hid::BatteryProtocol,
        fallback_name: &str,
        interface_path: &str,
        device_index: u8,
        known_indices: &[u8],
    ) -> PollResult {
        let identity = DeviceIdentity::Hid {
            interface_path: interface_path.to_owned(),
            device_index,
        };
        let cached_name = self.name_cache.get(&identity).map(str::to_owned);
        let was_known = cached_name.is_some() || known_indices.contains(&device_index);
        let reading = match cached_name {
            Some(name) => protocol
                .query_battery(interface, device_index)
                .map(|battery| (battery, name, None, false)),
            None => protocol
                .query_device(interface, device_index)
                .map(|device| {
                    let volty_hid::ProtocolDeviceReading {
                        name: queried_name,
                        battery,
                    } = device;
                    let (name, name_error) =
                        self.name_cache
                            .resolve(identity.clone(), fallback_name, || {
                                queried_name.map_err(|error| error.to_string())
                            });
                    (battery, name, name_error, true)
                }),
        };

        match reading {
            Ok((battery, name, name_error, name_was_queried)) => {
                self.name_cache.remember_device(identity.clone());
                let reading = DeviceReading {
                    identity: identity.clone(),
                    name: name.clone(),
                    device_index: Some(device_index),
                    battery,
                    estimated_time_remaining: None,
                    notification_error: None,
                };
                self.send(PollUpdate::Reading(reading))?;
                self.reading_count += 1;
                let state_error = self
                    .device_state
                    .as_mut()
                    .and_then(|state| state.record(interface_path, device_index).err());
                if let Some(error) = state_error {
                    *self.device_state = None;
                    self.send(PollUpdate::Warning(format!(
                        "Could not update discovery state; continuing with full scans: {error}"
                    )))?;
                }
                if name_was_queried {
                    self.send(PollUpdate::DeviceName {
                        identity,
                        name,
                        error: name_error,
                    })?;
                }
            }
            Err(error) => {
                self.name_cache.forget(&identity);
                let empty_discovery_slot = self.full_scan
                    && !was_known
                    && matches!(
                        error.downcast_ref::<volty_hid::HidppError>(),
                        Some(volty_hid::HidppError::Timeout)
                    );
                if empty_discovery_slot {
                    self.send(PollUpdate::Trace(format!(
                        "HID++ discovery address {device_index} did not respond"
                    )))?;
                } else {
                    self.errors.push(format!(
                        "{fallback_name} via {} (slot {device_index}): {error}",
                        protocol.name()
                    ));
                }
            }
        }
        Ok(())
    }
}
impl DeviceQuery<'_> {
    #[cfg(feature = "bluetooth")]
    fn query_bluetooth_devices(&mut self) -> PollResult {
        match volty_bluetooth::scan_battery_devices() {
            Ok(scan) => {
                self.send(PollUpdate::Trace(format!(
                    "Bluetooth discovery examined {} device entries and found {} readable battery value(s) with {} warning(s)",
                    scan.devices_examined,
                    scan.readings.len(),
                    scan.warnings.len()
                )))?;
                for event in scan.discovery_events {
                    self.send(PollUpdate::Trace(format!("Bluetooth discovery: {event}")))?;
                }
                self.errors.extend(
                    scan.warnings
                        .into_iter()
                        .map(|warning| format!("Bluetooth battery read: {warning}")),
                );
                for reading in scan.readings {
                    let identity = DeviceIdentity::Bluetooth {
                        device_id: reading.device_id,
                    };
                    self.send(PollUpdate::Reading(DeviceReading {
                        identity,
                        name: reading.name,
                        device_index: None,
                        battery: reading.battery,
                        estimated_time_remaining: None,
                        notification_error: None,
                    }))?;
                    self.reading_count += 1;
                }
            }
            Err(error) => self.errors.push(format!("Bluetooth discovery: {error}")),
        }
        Ok(())
    }

    fn finish(self) -> PollResult {
        if self.reading_count > 0 {
            if !self.errors.is_empty() {
                self.send(PollUpdate::Warning(format!(
                    "Some battery readings failed: {}",
                    self.errors.join("; ")
                )))?;
            }
            return Ok(());
        }
        if self.supported_interfaces == 0 && self.errors.is_empty() {
            return Err("No readable battery found".to_owned());
        }
        Err(if self.errors.is_empty() {
            "No supported battery devices found".to_owned()
        } else {
            self.errors.join("; ")
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use volty_battery::{BatteryLevel, BatterySource, BatteryState, BatteryStatus};
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
            estimated_time_remaining: None,
            notification_error: None,
        }
    }
    #[test]
    fn discovery_command_triggers_an_immediate_device_poll() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::channel();
        let (full_scan_tx, full_scan_rx) = mpsc::channel();
        let query_count = Arc::new(AtomicUsize::new(0));
        let worker_query_count = Arc::clone(&query_count);
        let worker = thread::spawn(move || {
            poll_worker_with_query(
                &commands_rx,
                &results_tx,
                Duration::from_secs(60),
                Duration::from_secs(60),
                move |_, full_scan| {
                    full_scan_tx.send(full_scan).unwrap();
                    worker_query_count.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
            );
        });

        assert!(full_scan_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Discover).unwrap();
        assert!(full_scan_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Quit).unwrap();
        worker.join().unwrap();

        assert_eq!(query_count.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn timed_polls_reuse_discovered_devices_until_a_full_discovery_is_requested() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::channel();
        let (full_scan_tx, full_scan_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            poll_worker_with_query(
                &commands_rx,
                &results_tx,
                Duration::from_millis(20),
                Duration::from_millis(20),
                move |_, full_scan| {
                    full_scan_tx.send(full_scan).unwrap();
                    Ok(())
                },
            );
        });

        assert!(full_scan_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        assert!(!full_scan_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));

        commands_tx.send(PollCommand::Discover).unwrap();
        assert!(full_scan_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Quit).unwrap();
        worker.join().unwrap();
    }
    #[test]
    fn re_enabling_device_requests_immediate_discovery_but_disabling_does_not() {
        let (commands_tx, commands_rx) = mpsc::channel();
        let (results_tx, results_rx) = mpsc::channel();
        let query_count = Arc::new(AtomicUsize::new(0));
        let worker_query_count = Arc::clone(&query_count);
        let worker = thread::spawn(move || {
            poll_worker_with_query(
                &commands_rx,
                &results_tx,
                Duration::from_secs(60),
                Duration::from_secs(60),
                move |_, _| {
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
                &commands_rx,
                &results_tx,
                Duration::from_secs(60),
                Duration::from_secs(60),
                move |results, _| {
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
}
