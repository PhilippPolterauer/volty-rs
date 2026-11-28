use std::{
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
};

use eframe::egui::Context;

use crate::{config::AppTheme, history::HistoryDeletion};

use super::{
    super::{
        management::{CleanupKind, CleanupResult, DashboardBackend, ManagedArtifact},
        model::{DashboardData, DashboardPoint, SECONDS_PER_DAY},
    },
    chart::Chart,
};

type BackendError = Box<dyn std::error::Error>;

pub(super) struct Snapshot {
    pub(super) data: DashboardData,
    pub(super) charts: Vec<Chart>,
    pub(super) artifacts: Option<Result<Vec<ManagedArtifact>, String>>,
}

pub(super) enum Action {
    Recording {
        identity: String,
        enabled: bool,
    },
    Monitoring {
        identity: String,
        enabled: bool,
    },
    PreviewDevice {
        identity: String,
        name: String,
        preferences: bool,
    },
    PreviewCleanup(CleanupKind, u32),
    ConfirmArtifacts(Vec<ManagedArtifact>),
    Execute(Operation),
}

pub(super) enum Operation {
    Cleanup(CleanupResult),
    Forget {
        identity: String,
        preferences: bool,
        expected: HistoryDeletion,
    },
    DeleteArtifacts(Vec<ManagedArtifact>),
}

pub(super) struct Confirmation {
    pub(super) title: String,
    pub(super) action: &'static str,
    pub(super) description: String,
    pub(super) operation: Operation,
}

pub(super) enum Reply {
    Confirm(Confirmation),
    Done(Option<String>, bool),
}

pub(super) struct Failure {
    pub(super) message: String,
    pub(super) refresh: bool,
}

pub(super) struct Worker {
    ctx: Context,
    launched: mpsc::Sender<io::Result<()>>,
    launches: mpsc::Receiver<io::Result<()>>,
    send: mpsc::SyncSender<Action>,
    receive: mpsc::Receiver<Result<Reply, Failure>>,
    load: mpsc::SyncSender<(u32, bool)>,
    snapshots: mpsc::Receiver<(u32, Result<Snapshot, String>)>,
    export: mpsc::SyncSender<(PathBuf, Arc<Snapshot>, usize)>,
    exports: mpsc::Receiver<Result<String, String>>,
}

impl Worker {
    pub(super) fn start(ctx: Context, backend: DashboardBackend) -> io::Result<Self> {
        let (send, requests) = mpsc::sync_channel(1);
        let (responses, receive) = mpsc::channel();
        let (load, loads) = mpsc::sync_channel(1);
        let (loaded, snapshots) = mpsc::channel();
        let (export, export_requests) = mpsc::sync_channel::<(PathBuf, Arc<Snapshot>, usize)>(1);
        let (exported, exports) = mpsc::channel();
        let (launched, launches) = mpsc::channel();
        let backend = Arc::new(backend);
        let read_backend = Arc::clone(&backend);
        let repaint = ctx.clone();
        thread::Builder::new()
            .name("volty-dashboard-history".to_owned())
            .spawn(move || {
                while let Ok((days, artifacts)) = loads.recv() {
                    let result = load_snapshot(&read_backend, days, artifacts)
                        .map_err(|error| error.to_string());
                    if loaded.send((days, result)).is_err() {
                        break;
                    }
                    repaint.request_repaint();
                }
            })?;
        let repaint = ctx.clone();
        thread::Builder::new()
            .name("volty-dashboard-export".to_owned())
            .spawn(move || {
                while let Ok((path, snapshot, index)) = export_requests.recv() {
                    let result = std::fs::File::create(&path)
                        .and_then(|file| {
                            write_csv(BufWriter::new(file), &snapshot.data.devices[index].points)
                        })
                        .map(|()| format!("Exported {}", path.display()))
                        .map_err(|error| error.to_string());
                    if exported.send(result).is_err() {
                        break;
                    }
                    repaint.request_repaint();
                }
            })?;
        let repaint = ctx.clone();
        thread::Builder::new()
            .name("volty-dashboard-data".to_owned())
            .spawn(move || {
                while let Ok(action) = requests.recv() {
                    let refresh = matches!(
                        action,
                        Action::Execute(_) | Action::Recording { .. } | Action::Monitoring { .. }
                    );
                    let response = handle(&backend, action).map_err(|error| Failure {
                        message: error.to_string(),
                        refresh,
                    });
                    if responses.send(response).is_err() {
                        break;
                    }
                    repaint.request_repaint();
                }
            })?;
        Ok(Self {
            ctx,
            launched,
            launches,
            send,
            receive,
            load,
            snapshots,
            export,
            exports,
        })
    }

    pub(super) fn send(&self, action: Action) -> io::Result<()> {
        self.send.try_send(action).map_err(queue_error)
    }

    pub(super) fn poll(&self) -> Option<Result<Reply, Failure>> {
        self.receive.try_recv().ok()
    }

    pub(super) fn load(&self, days: u32, artifacts: bool) -> io::Result<()> {
        self.load.try_send((days, artifacts)).map_err(queue_error)
    }

    pub(super) fn snapshot(&self) -> Option<(u32, Result<Snapshot, String>)> {
        self.snapshots.try_recv().ok()
    }

    pub(super) fn export(
        &self,
        path: PathBuf,
        snapshot: Arc<Snapshot>,
        index: usize,
    ) -> io::Result<()> {
        self.export
            .try_send((path, snapshot, index))
            .map_err(queue_error)
    }

    pub(super) fn exported(&self) -> Option<Result<String, String>> {
        self.exports.try_recv().ok()
    }

    pub(super) fn launch_settings(&self, preview: Option<AppTheme>) -> io::Result<()> {
        use std::os::windows::process::CommandExt;
        let launched = self.launched.clone();
        let ctx = self.ctx.clone();
        thread::Builder::new()
            .name("volty-open-settings".to_owned())
            .spawn(move || {
                let result = std::env::current_exe().and_then(|executable| {
                    let mut command = std::process::Command::new(executable);
                    if let Some(theme) = preview {
                        command.args(["ui-preview", "settings", "--theme", theme.as_str()]);
                    } else {
                        command.args(["dashboard", "--settings"]);
                    }
                    command
                        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
                        .spawn()
                        .map(|_| ())
                });
                if launched.send(result).is_ok() {
                    ctx.request_repaint();
                }
            })?;
        Ok(())
    }

    pub(super) fn settings_launched(&self) -> Option<io::Result<()>> {
        self.launches.try_recv().ok()
    }
}

fn queue_error<T>(error: mpsc::TrySendError<T>) -> io::Error {
    match error {
        mpsc::TrySendError::Full(_) => io::Error::other("Dashboard worker is busy"),
        mpsc::TrySendError::Disconnected(_) => io::Error::other("Dashboard worker stopped"),
    }
}

fn load_snapshot(
    backend: &DashboardBackend,
    days: u32,
    include_artifacts: bool,
) -> Result<Snapshot, BackendError> {
    let data = backend.get_history_data(days)?;
    let start = data
        .generated_at
        .saturating_sub(i64::from(days) * SECONDS_PER_DAY);
    let charts = data
        .devices
        .iter()
        .map(|device| Chart::new(device, start, data.generated_at))
        .collect();
    let artifacts =
        include_artifacts.then(|| backend.list_artifacts().map_err(|err| err.to_string()));
    Ok(Snapshot {
        data,
        charts,
        artifacts,
    })
}

fn handle(backend: &DashboardBackend, action: Action) -> Result<Reply, BackendError> {
    Ok(match action {
        Action::Recording { identity, enabled } => {
            backend.set_history_recording(&identity, enabled)?;
            Reply::Done(
                Some(
                    if enabled {
                        "History recording resumed."
                    } else {
                        "History recording paused."
                    }
                    .to_owned(),
                ),
                true,
            )
        }
        Action::Monitoring { identity, enabled } => {
            backend.set_monitored(&identity, enabled)?;
            Reply::Done(
                Some(
                    if enabled {
                        "Monitoring enabled."
                    } else {
                        "Monitoring paused."
                    }
                    .to_owned(),
                ),
                true,
            )
        }
        Action::PreviewDevice {
            identity,
            name,
            preferences,
        } => {
            let preview =
                backend.preview_cleanup(CleanupKind::DeviceHistory, None, Some(&identity))?;
            let mut description = format!("{name}\n{}\n\n{}", identity, preview_text(&preview));
            if preferences {
                description.push_str("\nVolty will also reset the monitoring, recording, and tray preferences stored for this device.");
            } else {
                description.push_str("\nNew readings can appear if history recording is enabled.");
            }
            Reply::Confirm(Confirmation {
                action: if preferences {
                    "Forget device"
                } else {
                    "Delete history"
                },
                title: if preferences {
                    "Forget device?"
                } else {
                    "Delete device history?"
                }
                .to_owned(),
                description,
                operation: Operation::Forget {
                    identity,
                    preferences,
                    expected: preview.counts(),
                },
            })
        }
        Action::PreviewCleanup(kind, days) => {
            let preview = backend.preview_cleanup(kind, Some(days), None)?;
            if preview.samples == 0 {
                Reply::Done(Some("No history matches this cleanup.".to_owned()), false)
            } else {
                let title = match kind {
                    CleanupKind::OlderThan => format!("Delete samples older than {days} days?"),
                    CleanupKind::InactiveDevices => {
                        format!("Delete history for devices inactive for {days} days?")
                    }
                    CleanupKind::DeviceHistory => {
                        unreachable!("device history has a separate action")
                    }
                };
                Reply::Confirm(Confirmation {
                    title,
                    action: "Delete samples",
                    description: preview_text(&preview),
                    operation: Operation::Cleanup(preview),
                })
            }
        }
        Action::ConfirmArtifacts(files) => {
            let mut description = files
                .iter()
                .take(5)
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if files.len() > 5 {
                description.push_str(&format!("\nAnd {} more", files.len() - 5));
            }
            description.push_str("\n\nThis deletion is permanent.");
            Reply::Confirm(Confirmation {
                action: if files.len() == 1 {
                    "Delete file"
                } else {
                    "Delete files"
                },
                title: format!(
                    "Delete {} diagnostic file{}?",
                    files.len(),
                    if files.len() == 1 { "" } else { "s" }
                ),
                description,
                operation: Operation::DeleteArtifacts(files),
            })
        }
        Action::Execute(operation) => Reply::Done(Some(execute(backend, operation)?), true),
    })
}

fn preview_text(preview: &CleanupResult) -> String {
    let range = match (preview.first_at, preview.last_at) {
        (Some(first), Some(last)) => format!(
            "\n{} to {}",
            super::platform::date(first),
            super::platform::date(last)
        ),
        _ => String::new(),
    };
    format!(
        "{} {} across {} {}.{range}\nThis deletion is permanent.",
        preview.samples,
        if preview.samples == 1 {
            "sample"
        } else {
            "samples"
        },
        preview.devices,
        if preview.devices == 1 {
            "device"
        } else {
            "devices"
        }
    )
}

fn execute(backend: &DashboardBackend, operation: Operation) -> Result<String, BackendError> {
    match operation {
        Operation::Cleanup(preview) => {
            let deleted = backend.execute_cleanup(preview)?;
            Ok(format!(
                "Deleted {} samples across {} device(s).",
                deleted.samples, deleted.devices
            ))
        }
        Operation::Forget {
            identity,
            preferences,
            expected,
        } => {
            let result = backend.forget_device(&identity, preferences, expected)?;
            let suffix = if preferences {
                " Device preferences reset."
            } else {
                ""
            };
            Ok(format!(
                "Deleted {} samples.{suffix}",
                result.deleted_samples
            ))
        }
        Operation::DeleteArtifacts(files) => {
            let mut failures = Vec::new();
            for file in &files {
                if let Err(error) = backend.delete_artifact(&file.id) {
                    failures.push(format!("{}: {error}", file.name));
                }
            }
            if failures.is_empty() {
                return Ok(format!("Deleted {} diagnostic file(s).", files.len()));
            }
            Err(io::Error::other(format!(
                "Deleted {} of {} diagnostic files. {}",
                files.len() - failures.len(),
                files.len(),
                failures.join("\n"),
            ))
            .into())
        }
    }
}

fn write_csv(mut writer: impl Write, points: &[DashboardPoint]) -> io::Result<()> {
    writeln!(writer, "time,percent,charging,millivolts\r")?;
    for point in points {
        let percent = point.p.map(|value| value.to_string()).unwrap_or_default();
        let voltage = point.v.map(|value| value.to_string()).unwrap_or_default();
        writeln!(
            writer,
            "{},{},{},{}\r",
            super::platform::date(point.t),
            percent,
            point.c,
            voltage
        )?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::HistoryStore;
    use std::time::{Duration, Instant};

    #[test]
    fn export_uses_displayed_snapshot_while_history_read_waits_for_sqlite() {
        let directory =
            std::env::temp_dir().join(format!("volty-dashboard-workers-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let database = directory.join("history.sqlite3");
        drop(HistoryStore::open(&database).unwrap());
        let writer = rusqlite::Connection::open(&database).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode = DELETE;
             WITH RECURSIVE counter(n) AS (
                 SELECT 1 UNION ALL SELECT n + 1 FROM counter WHERE n < 100000
             )
             INSERT INTO samples (identity, name, recorded_at, percent, charging)
             SELECT 'hid:fixture', 'Fixture', unixepoch() - 100000 + n, n % 101, (n / 200) % 2
             FROM counter;",
            )
            .unwrap();
        let backend = DashboardBackend::new(database, false);
        let start = Instant::now();
        let snapshot = Arc::new(load_snapshot(&backend, 7, false).unwrap());
        assert_eq!(snapshot.data.devices[0].points.len(), 100_000);
        eprintln!(
            "100,000 readings loaded and chart prepared in {:?}",
            start.elapsed()
        );

        writer.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let worker = Worker::start(Context::default(), backend).unwrap();
        worker.load(1, false).unwrap();
        let export = directory.join("history.csv");
        worker
            .export(export.clone(), Arc::clone(&snapshot), 0)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if let Some(result) = worker.exported() {
                break result;
            }
            assert!(Instant::now() < deadline, "CSV worker did not finish");
            thread::sleep(Duration::from_millis(5));
        };
        assert!(result.is_ok(), "{result:?}");
        writer.execute_batch("ROLLBACK").unwrap();
        let loaded = loop {
            if let Some((days, result)) = worker.snapshot() {
                assert_eq!(days, 1);
                break result;
            }
            assert!(Instant::now() < deadline, "History worker did not finish");
            thread::sleep(Duration::from_millis(5));
        };
        let readings = loaded.unwrap().data.devices[0].points.len();
        assert!(readings < 100_000);
        drop(worker);
        drop(writer);
        std::fs::remove_file(export).unwrap();
        std::fs::remove_file(directory.join("history.sqlite3")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn csv_preserves_missing_readings_and_the_dashboard_column_order() {
        let points = [
            DashboardPoint {
                t: 1_800_000_000,
                p: None,
                c: false,
                v: Some(3750),
            },
            DashboardPoint {
                t: 1_800_000_000,
                p: Some(75),
                c: true,
                v: None,
            },
        ];
        let mut bytes = Vec::new();
        write_csv(&mut bytes, &points).unwrap();
        let time = super::super::platform::date(points[0].t);
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            format!(
                "time,percent,charging,millivolts\r\n{time},,false,3750\r\n{time},75,true,\r\n"
            ),
        );
    }
}
