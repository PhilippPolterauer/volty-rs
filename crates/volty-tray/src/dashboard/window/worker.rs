use std::{
    io::{self, BufWriter, Write},
    path::PathBuf,
    sync::mpsc,
    thread,
};

use eframe::egui::Context;

use crate::config::{AppConfig, AppTheme, BatteryColorMap};

use super::{
    super::{
        management::{CleanupKind, CleanupResult, DashboardBackend, ManagedArtifact},
        model::{DashboardData, DashboardDevice, DashboardPoint},
    },
    chart::Chart,
};

type BackendError = Box<dyn std::error::Error>;

pub(super) struct Snapshot {
    pub(super) data: DashboardData,
    pub(super) charts: Vec<Chart>,
    pub(super) theme: AppTheme,
    pub(super) colors: BatteryColorMap,
    pub(super) artifacts: Option<Result<Vec<ManagedArtifact>, String>>,
}

pub(super) enum Action {
    Refresh(u32, bool),
    OpenSettings,
    Recording {
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
    Export(PathBuf, DashboardDevice),
}

pub(super) enum Operation {
    Cleanup(CleanupKind, u32),
    Forget { identity: String, preferences: bool },
    DeleteArtifacts(Vec<ManagedArtifact>),
}

pub(super) struct Confirmation {
    pub(super) title: String,
    pub(super) description: String,
    pub(super) operation: Operation,
}

pub(super) enum Reply {
    Snapshot(Box<Snapshot>),
    Confirm(Confirmation),
    Done(Option<String>, bool),
}

pub(super) struct Failure {
    pub(super) message: String,
    pub(super) refresh: bool,
}

pub(super) struct Worker {
    send: mpsc::Sender<Action>,
    receive: mpsc::Receiver<Result<Reply, Failure>>,
}

impl Worker {
    pub(super) fn start(ctx: Context, backend: DashboardBackend) -> io::Result<Self> {
        let (send, requests) = mpsc::channel();
        let (responses, receive) = mpsc::channel();
        thread::Builder::new()
            .name("volty-dashboard-data".to_owned())
            .spawn(move || {
                while let Ok(action) = requests.recv() {
                    let refresh = matches!(action, Action::Execute(_) | Action::Recording { .. });
                    let response = handle(&backend, action).map_err(|error| Failure {
                        message: error.to_string(),
                        refresh,
                    });
                    if responses.send(response).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })?;
        Ok(Self { send, receive })
    }

    pub(super) fn send(&self, action: Action) -> io::Result<()> {
        self.send
            .send(action)
            .map_err(|_| io::Error::other("Dashboard worker stopped"))
    }

    pub(super) fn poll(&self) -> Option<Result<Reply, Failure>> {
        self.receive.try_recv().ok()
    }
}

fn handle(backend: &DashboardBackend, action: Action) -> Result<Reply, BackendError> {
    Ok(match action {
        Action::Refresh(days, include_artifacts) => {
            let data = backend.get_history_data(days)?;
            let config = AppConfig::load().unwrap_or_default();
            let charts = data.devices.iter().map(Chart::new).collect();
            let artifacts =
                include_artifacts.then(|| backend.list_artifacts().map_err(|err| err.to_string()));
            Reply::Snapshot(Box::new(Snapshot {
                data,
                charts,
                theme: config.app_theme,
                colors: config.battery_color_map,
                artifacts,
            }))
        }
        Action::OpenSettings => {
            backend.open_settings()?;
            Reply::Done(None, false)
        }
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
                    description: preview_text(&preview),
                    operation: Operation::Cleanup(kind, days),
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
        Action::Export(path, device) => {
            let writer = BufWriter::new(std::fs::File::create(&path)?);
            write_csv(writer, &device.points)?;
            Reply::Done(Some(format!("Exported {}", path.display())), false)
        }
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
        "{} samples across {} device(s).{range}\nThis deletion is permanent.",
        preview.samples, preview.devices
    )
}

fn execute(backend: &DashboardBackend, operation: Operation) -> Result<String, BackendError> {
    match operation {
        Operation::Cleanup(kind, days) => {
            let deleted = backend.execute_cleanup(kind, Some(days), None)?;
            Ok(format!(
                "Deleted {} samples across {} device(s).",
                deleted.samples, deleted.devices
            ))
        }
        Operation::Forget {
            identity,
            preferences,
        } => {
            let result = backend.forget_device(&identity, preferences)?;
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
