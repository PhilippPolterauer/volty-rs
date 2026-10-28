use super::{record_event, AppConfig, DeviceState, FileLogger, LogLevel};
use crate::provider::default_providers;
use std::{
    collections::VecDeque,
    error::Error,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};
use volty_battery::{BatteryProvider, ProviderEvent, ProviderPoll, ScanMode, SinkClosed};

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
    Event(ProviderEvent),
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
                default_providers(device_state),
            );
        })?;
    Ok((commands_tx, results_rx, poller))
}
pub(super) fn poll_worker(
    commands: &mpsc::Receiver<PollCommand>,
    results: &mpsc::Sender<PollUpdate>,
    poll_interval: Duration,
    retry_interval: Duration,
    mut providers: Vec<Box<dyn BatteryProvider>>,
) {
    poll_worker_with_query(
        commands,
        results,
        poll_interval,
        retry_interval,
        move |results, full_scan| query_providers(results, &mut providers, full_scan),
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
        if send_update(results, PollUpdate::Finished(reading)).is_err() {
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

/// Sends an update and wakes the blocked UI loop to consume it.
fn send_update(
    results: &mpsc::Sender<PollUpdate>,
    update: PollUpdate,
) -> Result<(), mpsc::SendError<PollUpdate>> {
    results.send(update)?;
    super::wake::signal();
    Ok(())
}

const RECEIVER_DISCONNECTED: &str = "device polling result receiver disconnected";

pub(super) fn query_providers(
    results: &mpsc::Sender<PollUpdate>,
    providers: &mut [Box<dyn BatteryProvider>],
    full_scan: bool,
) -> PollResult {
    let mode = if full_scan {
        ScanMode::Discover
    } else {
        ScanMode::Refresh
    };
    let mut sink = |event| send_update(results, PollUpdate::Event(event)).map_err(|_| SinkClosed);
    let mut total = ProviderPoll::default();
    for provider in providers {
        let poll = provider
            .poll(mode, &mut sink)
            .map_err(|SinkClosed| RECEIVER_DISCONNECTED.to_owned())?;
        total.readings += poll.readings;
        total.recognised += poll.recognised;
        total.errors.extend(poll.errors);
    }

    if total.readings > 0 {
        if !total.errors.is_empty() {
            sink(ProviderEvent::Warning(format!(
                "Some battery readings failed: {}",
                total.errors.join("; ")
            )))
            .map_err(|SinkClosed| RECEIVER_DISCONNECTED.to_owned())?;
        }
        return Ok(());
    }
    if total.recognised == 0 && total.errors.is_empty() {
        return Err("No readable battery found".to_owned());
    }
    Err(if total.errors.is_empty() {
        "No supported battery devices found".to_owned()
    } else {
        total.errors.join("; ")
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use volty_battery::{
        BatteryLevel, BatteryState, BatteryStatus, DeviceId, DeviceLabel, SourceReading,
    };

    fn test_reading() -> SourceReading {
        SourceReading {
            id: DeviceId::new("hid:test#1"),
            label: DeviceLabel::new("Test device").with_qualifier("slot 1"),
            battery: BatteryStatus {
                level_percent: Some(20),
                level: BatteryLevel::Low,
                raw_level: 2,
                state: BatteryState::Discharging,
                raw_status: Some(0),
                source: "test",
            },
        }
    }

    struct FakeProvider {
        readings: usize,
        recognised: usize,
        errors: Vec<String>,
    }

    impl BatteryProvider for FakeProvider {
        fn poll(
            &mut self,
            _mode: ScanMode,
            sink: &mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>,
        ) -> Result<ProviderPoll, SinkClosed> {
            for _ in 0..self.readings {
                sink(ProviderEvent::Reading(test_reading()))?;
            }
            Ok(ProviderPoll {
                readings: self.readings,
                recognised: self.recognised,
                errors: self.errors.clone(),
            })
        }
    }

    fn fake(readings: usize, recognised: usize, errors: &[&str]) -> Box<dyn BatteryProvider> {
        Box::new(FakeProvider {
            readings,
            recognised,
            errors: errors.iter().map(|error| (*error).to_owned()).collect(),
        })
    }

    #[test]
    fn provider_results_are_aggregated_across_sources() {
        let (results_tx, results_rx) = mpsc::channel();
        let mut providers = vec![fake(0, 1, &["HID discovery: boom"]), fake(1, 1, &[])];

        assert_eq!(query_providers(&results_tx, &mut providers, true), Ok(()));
        let events: Vec<_> = results_rx.try_iter().collect();
        assert!(matches!(
            events.as_slice(),
            [
                PollUpdate::Event(ProviderEvent::Reading(_)),
                PollUpdate::Event(ProviderEvent::Warning(message))
            ] if message.contains("HID discovery: boom")
        ));
    }

    #[test]
    fn no_recognised_device_and_no_error_means_no_readable_battery() {
        let (results_tx, _results_rx) = mpsc::channel();

        assert_eq!(
            query_providers(&results_tx, &mut [fake(0, 0, &[])], true),
            Err("No readable battery found".to_owned())
        );
        assert_eq!(
            query_providers(&results_tx, &mut [fake(0, 2, &[])], true),
            Err("No supported battery devices found".to_owned())
        );
        assert_eq!(
            query_providers(&results_tx, &mut [fake(0, 0, &["a", "b"])], true),
            Err("a; b".to_owned())
        );
    }

    #[test]
    fn closed_receiver_stops_the_poll() {
        let (results_tx, results_rx) = mpsc::channel();
        drop(results_rx);

        assert_eq!(
            query_providers(&results_tx, &mut [fake(1, 1, &[])], true),
            Err(RECEIVER_DISCONNECTED.to_owned())
        );
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
                    assert!(results
                        .send(PollUpdate::Event(ProviderEvent::Reading(test_reading())))
                        .is_ok());
                    finish_query_rx.recv().unwrap();
                    assert!(results
                        .send(PollUpdate::Event(ProviderEvent::DeviceName {
                            id: DeviceId::new("hid:test#1"),
                            label: DeviceLabel::new("Resolved name"),
                            error: None,
                        }))
                        .is_ok());
                    Ok(())
                },
            );
        });

        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Event(ProviderEvent::Reading(_))
        ));
        finish_query_tx.send(()).unwrap();
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Event(ProviderEvent::DeviceName { .. })
        ));
        assert!(matches!(
            results_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PollUpdate::Finished(Ok(()))
        ));
        commands_tx.send(PollCommand::Quit).unwrap();
        worker.join().unwrap();
    }
}
