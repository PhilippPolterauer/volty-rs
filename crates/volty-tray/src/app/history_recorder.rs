use std::{
    io,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread::{self, JoinHandle},
};

use crate::{
    history::{HistorySample, HistoryStore},
    state::DeviceState,
};

/// Owns the SQLite writer independently of device enumeration and tray messages.
pub(super) struct HistoryRecorder {
    samples: SyncSender<HistorySample>,
    errors: Receiver<Option<String>>,
    thread: JoinHandle<()>,
}

impl HistoryRecorder {
    pub(super) fn start() -> io::Result<Self> {
        let (samples, incoming) = mpsc::sync_channel::<HistorySample>(256);
        let (report, errors) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("volty-history-recorder".to_owned())
            .spawn(move || {
                let run = || -> Result<(), Box<dyn std::error::Error>> {
                    let mut history = HistoryStore::open_local()?;
                    let preferences = DeviceState::open_local()?;
                    let mut previous_error = None;
                    while let Ok(sample) = incoming.recv() {
                        let result = preferences
                            .history_recording_enabled(&sample.identity)
                            .and_then(|enabled| {
                                if enabled {
                                    history.record(&sample)
                                } else {
                                    Ok(false)
                                }
                            });
                        let error = match result {
                            Ok(inserted) => {
                                if inserted {
                                    crate::dashboard::notify_dashboard();
                                }
                                None
                            }
                            Err(error) => Some(format!(
                                "Could not record battery history for {}: {error}",
                                sample.name
                            )),
                        };
                        if error != previous_error {
                            previous_error = error.clone();
                            if report.send(error).is_err() {
                                break;
                            }
                            super::wake::signal();
                        }
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    let _ = report.send(Some(format!(
                        "Could not start the history recorder: {error}"
                    )));
                    super::wake::signal();
                }
            })?;
        Ok(Self {
            samples,
            errors,
            thread,
        })
    }

    pub(super) fn record(&self, sample: HistorySample) -> io::Result<()> {
        self.samples.try_send(sample).map_err(|error| match error {
            TrySendError::Full(_) => {
                io::Error::other("Battery history queue is full. A sample could not be recorded.")
            }
            TrySendError::Disconnected(_) => {
                io::Error::other("Battery history recorder is unavailable")
            }
        })
    }

    pub(super) fn poll_error(&self) -> Option<Option<String>> {
        self.errors.try_recv().ok()
    }

    pub(super) fn finish(self) -> io::Result<()> {
        drop(self.samples);
        self.thread
            .join()
            .map_err(|_| io::Error::other("Battery history recorder panicked"))
    }
}
