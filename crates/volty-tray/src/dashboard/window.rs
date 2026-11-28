mod chart;
mod platform;
mod worker;

use std::error::Error;

use eframe::egui::{
    self, Align, Button, Color32, ComboBox, Frame, Layout, Modal, RichText, ScrollArea, Stroke,
    ThemePreference, Ui,
};

use crate::{app_icon, config::AppTheme};

use super::{
    management::{CleanupKind, DashboardBackend},
    model::{DashboardData, DashboardDevice, DashboardInventoryDevice},
};
use platform::{Signal, WindowSignals};
use worker::{Action, Confirmation, Reply, Snapshot, Worker};

pub(super) fn open_dashboard_window(
    backend: DashboardBackend,
    days: u32,
) -> Result<(), Box<dyn Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 760.0])
            .with_min_inner_size([660.0, 460.0])
            .with_icon(app_icon::dashboard_icon()?),
        ..Default::default()
    };
    Ok(eframe::run_native(
        "Volty Dashboard",
        options,
        Box::new(move |cc| {
            let ctx = &cc.egui_ctx;
            ctx.all_styles_mut(|style| {
                style.spacing.item_spacing = egui::vec2(10.0, 8.0);
                style.spacing.interact_size.y = 30.0;
                style.spacing.button_padding = egui::vec2(12.0, 4.0);
                style.visuals.widgets.noninteractive.fg_stroke.color = if style.visuals.dark_mode {
                    Color32::from_rgb(212, 223, 236)
                } else {
                    Color32::from_rgb(45, 58, 73)
                };
                for text_style in [egui::TextStyle::Body, egui::TextStyle::Button] {
                    style
                        .text_styles
                        .insert(text_style, egui::FontId::proportional(14.0));
                }
            });
            let signals = WindowSignals::start(ctx.clone())?;
            let worker = Worker::start(ctx.clone(), backend)?;
            let mut dashboard = Dashboard {
                worker,
                signals,
                days,
                snapshot: None,
                selected: None,
                busy: false,
                refresh_pending: false,
                status: None,
                confirmation: None,
                page: Page::History,
                older_days: 7,
                inactive_days: 7,
                search: String::new(),
            };
            dashboard.refresh();
            Ok(Box::new(dashboard))
        }),
    )?)
}

#[derive(PartialEq, Eq)]
enum Page {
    History,
    Manage,
}

struct Dashboard {
    worker: Worker,
    signals: WindowSignals,
    days: u32,
    snapshot: Option<Snapshot>,
    selected: Option<String>,
    busy: bool,
    refresh_pending: bool,
    status: Option<Result<String, String>>,
    confirmation: Option<Confirmation>,
    page: Page,
    older_days: u32,
    inactive_days: u32,
    search: String,
}

impl Dashboard {
    fn request(&mut self, action: Action) {
        self.status = None;
        self.send(action);
    }

    fn send(&mut self, action: Action) {
        match self.worker.send(action) {
            Ok(()) => self.busy = true,
            Err(error) => self.status = Some(Err(error.to_string())),
        }
    }

    fn refresh(&mut self) {
        if self.busy || self.confirmation.is_some() {
            self.refresh_pending = true;
        } else {
            self.refresh_pending = false;
            self.send(Action::Refresh(self.days, self.page == Page::Manage));
        }
    }

    fn set_snapshot(&mut self, ctx: &egui::Context, mut snapshot: Snapshot) {
        ctx.set_theme(match snapshot.theme {
            AppTheme::System => ThemePreference::System,
            AppTheme::Light => ThemePreference::Light,
            AppTheme::Dark => ThemePreference::Dark,
        });
        let data = &snapshot.data;
        if !self.selected.as_deref().is_some_and(|id| {
            data.devices.iter().any(|device| device.identity == id)
                || data.inventory.iter().any(|device| device.identity == id)
        }) {
            self.selected = data
                .devices
                .first()
                .map(|device| device.identity.clone())
                .or_else(|| data.inventory.first().map(|device| device.identity.clone()));
        }
        if snapshot.artifacts.is_none() {
            snapshot.artifacts = self.snapshot.as_mut().and_then(|old| old.artifacts.take());
        }
        self.snapshot = Some(snapshot);
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Volty").size(27.0).strong());
            ui.add_space(12.0);
            if ui
                .selectable_label(self.page == Page::History, "Battery history")
                .clicked()
            {
                self.page = Page::History;
            }
            if ui
                .selectable_label(self.page == Page::Manage, "Manage data")
                .clicked()
                && self.page != Page::Manage
            {
                self.page = Page::Manage;
                self.refresh();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!self.busy, Button::new("Settings"))
                    .clicked()
                {
                    self.request(Action::OpenSettings);
                }
                if ui.add_enabled(!self.busy, Button::new("Refresh")).clicked() {
                    self.status = None;
                    self.refresh();
                }
                if self.busy {
                    ui.spinner();
                }
            });
        });
        ui.separator();
        if self.page == Page::History {
            if let Some(snapshot) = &self.snapshot {
                let devices = &snapshot.data.devices;
                let online = devices.iter().filter(|device| device.online).count();
                let readings: u64 = devices.iter().map(|device| device.samples).sum();
                ui.label(format!(
                    "{} devices with readings  ·  {online} online  ·  {readings} readings",
                    devices.len()
                ));
            }
        }
        if let Some(status) = &self.status {
            match status {
                Ok(message) => {
                    ui.label(message);
                }
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        }
    }

    fn history(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        let mut requested_days = self.days;
        let height = ui.available_height();
        let export = ui
            .horizontal_top(|ui| {
                // Bound the sidebar independently of long device names.
                ui.allocate_ui_with_layout(
                    egui::vec2(200.0, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(200.0);
                        ui.set_max_width(200.0);
                        ui.label(RichText::new("Devices").size(18.0).strong());
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Search devices")
                                .desired_width(168.0),
                        );
                        day_picker(
                            ui,
                            "history-range",
                            &mut requested_days,
                            &[1, 3, 7, 14, 30],
                            !self.busy,
                        );
                        if let Some(snapshot) = &self.snapshot {
                            let query = self.search.trim().to_lowercase();
                            let matches = |name: &str, identity: &str| {
                                query.is_empty()
                                    || name.to_lowercase().contains(&query)
                                    || identity.to_lowercase().contains(&query)
                            };
                            ScrollArea::vertical().id_salt("devices").show(ui, |ui| {
                                let data = &snapshot.data;
                                for device in data
                                    .devices
                                    .iter()
                                    .filter(|device| matches(&device.name, &device.identity))
                                {
                                    device_button(
                                        ui,
                                        &mut self.selected,
                                        &device.identity,
                                        &device.name,
                                        &format!(
                                            "{}  ·  {}",
                                            percent(device.latest_percent),
                                            device_status(device)
                                        ),
                                    );
                                }
                                for device in data.inventory.iter().filter(|item| {
                                    matches(&item.name, &item.identity)
                                        && data.devices.iter().all(|d| d.identity != item.identity)
                                }) {
                                    device_button(
                                        ui,
                                        &mut self.selected,
                                        &device.identity,
                                        &device.name,
                                        "No samples in this range",
                                    );
                                }
                            });
                        }
                    },
                );
                ui.add_space(6.0);
                let Some(snapshot) = &self.snapshot else {
                    ui.label(if self.busy {
                        "Loading battery history…"
                    } else {
                        "Battery history unavailable."
                    });
                    return false;
                };
                let data = &snapshot.data;
                let device = data
                    .devices
                    .iter()
                    .position(|item| Some(item.identity.as_str()) == self.selected.as_deref());
                let inventory = selected_inventory(data, self.selected.as_deref());
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_salt(("history", &self.selected))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if let Some(index) = device {
                                    history_detail(ui, snapshot, index, !self.busy)
                                } else {
                                    ui.heading(
                                        inventory
                                            .map_or("Battery history", |item| item.name.as_str()),
                                    );
                                    ui.label(
                                        if inventory
                                            .is_some_and(|item| !item.history_recording_enabled)
                                        {
                                            "History recording is paused for this device."
                                        } else {
                                            "No battery history recorded in this range."
                                        },
                                    );
                                    false
                                }
                            })
                            .inner
                    },
                )
                .inner
            })
            .inner;
        if requested_days != self.days {
            self.days = requested_days;
            self.status = None;
            self.refresh();
        }
        if export {
            match platform::save_path(frame) {
                Ok(Some(path)) => {
                    if let Some(device) = self
                        .snapshot
                        .as_ref()
                        .and_then(|s| selected_device(&s.data, self.selected.as_deref()))
                    {
                        self.request(Action::Export(path, device.clone()));
                    }
                }
                Ok(None) => {}
                Err(error) => self.status = Some(Err(error.to_string())),
            }
        }
    }

    fn manage(&mut self, ui: &mut Ui) {
        let Some(snapshot) = &self.snapshot else {
            ui.label("Dashboard data unavailable.");
            return;
        };
        let data = &snapshot.data;
        let enabled = !self.busy && data.management_allowed;
        let mut action = None;
        ScrollArea::vertical()
            .id_salt("manage-data")
            .show(ui, |ui| {
                ui.heading("Manage data");
                if !data.management_allowed {
                    ui.label("Read-only mode for the selected database.");
                }
                card(ui, |ui| {
                    ui.heading("Device history");
                    let mut selected = self.selected.clone();
                    let current = selected_inventory(data, selected.as_deref());
                    ComboBox::from_id_salt("managed-device")
                        .width(260.0)
                        .selected_text(current.map_or("Select a device", |item| item.name.as_str()))
                        .show_ui(ui, |ui| {
                            for item in &data.inventory {
                                ui.selectable_value(
                                    &mut selected,
                                    Some(item.identity.clone()),
                                    &item.name,
                                )
                                .on_hover_text(&item.identity);
                            }
                        });
                    self.selected = selected;
                    if let Some(item) = selected_inventory(data, self.selected.as_deref()) {
                        let monitoring = match item.monitored {
                            Some(true) => "Monitored",
                            Some(false) => "Monitoring disabled",
                            None => "Monitoring default",
                        };
                        let pin = if item.pinned {
                            "  ·  Pinned to tray"
                        } else {
                            ""
                        };
                        ui.label(format!("{}  ·  {} samples{pin}", monitoring, item.samples));
                        ui.label(format!(
                            "Last reading  {}",
                            item.last_sample_at
                                .map_or_else(|| "—".to_owned(), platform::date)
                        ));
                        ui.add_enabled_ui(enabled, |ui| {
                            let mut recording = item.history_recording_enabled;
                            if ui
                                .checkbox(&mut recording, "Record battery history")
                                .changed()
                            {
                                action = Some(Action::Recording {
                                    identity: item.identity.clone(),
                                    enabled: recording,
                                });
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Delete history…").clicked() {
                                    action = Some(Action::PreviewDevice {
                                        identity: item.identity.clone(),
                                        name: item.name.clone(),
                                        preferences: false,
                                    });
                                }
                                if ui.button("Forget device…").clicked() {
                                    action = Some(Action::PreviewDevice {
                                        identity: item.identity.clone(),
                                        name: item.name.clone(),
                                        preferences: true,
                                    });
                                }
                            });
                        });
                    }
                });
                card(ui, |ui| {
                    ui.heading("History cleanup");
                    ui.add_enabled_ui(enabled, |ui| {
                        for (label, days, kind, choices) in [
                            (
                                "Samples older than",
                                &mut self.older_days,
                                CleanupKind::OlderThan,
                                &[1, 3, 7, 14, 21, 30][..],
                            ),
                            (
                                "Devices inactive for",
                                &mut self.inactive_days,
                                CleanupKind::InactiveDevices,
                                &[7, 14, 21][..],
                            ),
                        ] {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(label);
                                day_picker(ui, label, days, choices, true);
                                if ui.button("Preview cleanup").clicked() {
                                    action = Some(Action::PreviewCleanup(kind, *days));
                                }
                            });
                        }
                    });
                });
                card(ui, |ui| {
                    ui.heading("Diagnostic files");
                    match &snapshot.artifacts {
                        Some(Ok(files)) if files.is_empty() => {
                            ui.label("No managed diagnostic files.");
                        }
                        Some(Ok(files)) => {
                            if ui
                                .add_enabled(enabled, Button::new("Delete all…"))
                                .clicked()
                            {
                                action = Some(Action::ConfirmArtifacts(files.clone()));
                            }
                            ScrollArea::vertical()
                                .id_salt("artifacts")
                                .max_height(250.0)
                                .show_rows(ui, 30.0, files.len(), |ui, rows| {
                                    for file in &files[rows] {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add_enabled(enabled, Button::new("Delete…"))
                                                .clicked()
                                            {
                                                action = Some(Action::ConfirmArtifacts(vec![
                                                    file.clone()
                                                ]));
                                            }
                                            ui.label(format!(
                                                "{}  ·  {}  ·  {} bytes",
                                                file.name, file.kind, file.bytes
                                            ))
                                            .on_hover_text(
                                                file.created_at
                                                    .and_then(|at| i64::try_from(at).ok())
                                                    .map_or_else(
                                                        || file.name.clone(),
                                                        platform::date,
                                                    ),
                                            );
                                        });
                                    }
                                });
                        }
                        Some(Err(error)) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        None => {
                            ui.label("Loading diagnostic files…");
                        }
                    }
                });
            });
        if let Some(action) = action {
            self.request(action);
        }
    }

    fn confirm(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let response = Modal::new(egui::Id::unique("confirm dashboard change")).show(ctx, |ui| {
            ui.set_width(390.0);
            ui.heading(&confirmation.title);
            ui.label(&confirmation.description);
            ui.horizontal(|ui| {
                ui.button("Cancel")
                    .clicked()
                    .then_some(false)
                    .or_else(|| ui.button("Confirm deletion").clicked().then_some(true))
            })
            .inner
        });
        if response.inner == Some(true) {
            let Some(confirmation) = self.confirmation.take() else {
                return;
            };
            self.request(Action::Execute(confirmation.operation));
        } else if response.inner == Some(false) || response.should_close() {
            self.confirmation = None;
        }
    }
}

impl eframe::App for Dashboard {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(signal) = self.signals.receive.try_recv() {
            match signal {
                Signal::Activate => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.refresh();
                }
                Signal::Shutdown => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
            }
        }
        if let Some(reply) = self.worker.poll() {
            self.busy = false;
            match reply {
                Ok(Reply::Snapshot(snapshot)) => self.set_snapshot(ctx, *snapshot),
                Ok(Reply::Confirm(confirmation)) => self.confirmation = Some(confirmation),
                Ok(Reply::Done(status, refresh)) => {
                    self.status = status.map(Ok);
                    self.refresh_pending |= refresh;
                }
                Err(error) => {
                    self.refresh_pending |= error.refresh;
                    self.status = Some(Err(error.message));
                }
            }
        }
        if self.refresh_pending && !self.busy && self.confirmation.is_none() {
            self.refresh();
        }
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        Frame::new()
            .fill(canvas(ui))
            .inner_margin(20)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.header(ui);
                match self.page {
                    Page::History => self.history(ui, frame),
                    Page::Manage => self.manage(ui),
                }
            });
        self.confirm(ui.ctx());
    }
}

fn selected_device<'a>(
    data: &'a DashboardData,
    selected: Option<&str>,
) -> Option<&'a DashboardDevice> {
    data.devices
        .iter()
        .find(|item| Some(item.identity.as_str()) == selected)
}

fn selected_inventory<'a>(
    data: &'a DashboardData,
    selected: Option<&str>,
) -> Option<&'a DashboardInventoryDevice> {
    data.inventory
        .iter()
        .find(|item| Some(item.identity.as_str()) == selected)
}

fn device_status(device: &DashboardDevice) -> &'static str {
    match (device.online, device.charging) {
        (true, true) => "Charging",
        (true, false) => "Online",
        (false, _) => "Offline",
    }
}

fn device_button(
    ui: &mut Ui,
    selected: &mut Option<String>,
    identity: &str,
    name: &str,
    subtitle: &str,
) {
    let active = selected.as_deref() == Some(identity);
    ui.push_id(identity, |ui| {
        let label = Button::new(format!("{name}\n{subtitle}"))
            .selected(active)
            .wrap()
            .min_size(egui::vec2(ui.available_width(), 50.0));
        if ui.add(label).on_hover_text(identity).clicked() {
            *selected = Some(identity.to_owned());
        }
    });
}

fn day_picker(ui: &mut Ui, id: &str, days: &mut u32, choices: &[u32], enabled: bool) {
    let label = |days| {
        if days == 1 {
            "1 day".to_owned()
        } else {
            format!("{days} days")
        }
    };
    ui.add_enabled_ui(enabled, |ui| {
        ComboBox::from_id_salt(id)
            .selected_text(label(*days))
            .show_ui(ui, |ui| {
                for &choice in choices {
                    ui.selectable_value(days, choice, label(choice));
                }
            });
    });
}

fn canvas(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(17, 22, 31)
    } else {
        Color32::from_rgb(241, 245, 249)
    }
}

fn accent(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(112, 211, 237)
    } else {
        Color32::from_rgb(8, 112, 146)
    }
}

fn card<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    let (fill, border) = if ui.visuals().dark_mode {
        (Color32::from_rgb(24, 32, 45), Color32::from_rgb(49, 63, 82))
    } else {
        (Color32::WHITE, Color32::from_rgb(217, 226, 235))
    };
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(10)
        .inner_margin(16)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui)
        })
        .inner
}

fn percent(value: Option<u8>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value}%"))
}

fn duration(seconds: i64) -> String {
    let minutes = seconds / 60;
    match minutes {
        0 => "<1m".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h {}m", minutes / 60, minutes % 60),
        _ => format!("{}d {}h", minutes / 1440, minutes % 1440 / 60),
    }
}

fn history_detail(ui: &mut Ui, snapshot: &Snapshot, index: usize, enabled: bool) -> bool {
    let device = &snapshot.data.devices[index];
    let chart = &snapshot.charts[index];
    ui.heading(&device.name);
    ui.label(format!(
        "{}  ·  Last reading {}",
        device_status(device),
        platform::date(device.last_at)
    ));
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let level_color = device.latest_percent.map_or_else(
                || accent(ui),
                |value| {
                    let [r, g, b] = snapshot.colors.color_for(value);
                    Color32::from_rgb(r, g, b)
                },
            );
            metric(
                ui,
                "Battery",
                RichText::new(percent(device.latest_percent)).color(level_color),
            );
            metric(
                ui,
                "Est. remaining",
                RichText::new(device.eta_seconds.map_or_else(|| "—".to_owned(), duration)),
            );
            metric(
                ui,
                "Drain per day",
                RichText::new(
                    device
                        .percent_per_day
                        .map_or_else(|| "—".to_owned(), |rate| format!("{rate:.1}%")),
                ),
            );
            metric(ui, "Readings", RichText::new(device.samples.to_string()));
        });
    });
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Battery trend");
            if chart.has_charging() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.colored_label(accent(ui), "Shaded areas show charging");
                });
            }
        });
        chart.trend(ui, device);
    });
    card(ui, |ui| {
        ui.heading("Activity");
        chart.timeline(ui, device);
        ui.horizontal(|ui| {
            ui.colored_label(
                accent(ui),
                format!("Online {}", duration(device.online_seconds)),
            );
            ui.label(format!("Offline {}", duration(device.offline_seconds)));
        });
    });
    card(ui, |ui| {
        let export = ui
            .horizontal(|ui| {
                ui.heading("Readings");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled(enabled, Button::new("Export CSV…"))
                        .clicked()
                })
                .inner
            })
            .inner;
        egui::CollapsingHeader::new("View readings").show(ui, |ui| {
            ui.monospace(format!(
                "{:<19}  {:>7}  {:>9}  {:>9}",
                "Local time", "Battery", "Voltage", "Charging"
            ));
            ScrollArea::vertical()
                .id_salt("reading rows")
                .max_height(240.0)
                .show_rows(
                    ui,
                    ui.text_style_height(&egui::TextStyle::Monospace),
                    device.points.len(),
                    |ui, rows| {
                        for row in rows {
                            let point = &device.points[device.points.len() - 1 - row];
                            ui.monospace(format!(
                                "{:<19}  {:>7}  {:>9}  {:>9}",
                                platform::date(point.t),
                                percent(point.p),
                                chart::voltage(point.v),
                                if point.c { "Yes" } else { "No" }
                            ));
                        }
                    },
                );
        });
        export
    })
}

fn metric(ui: &mut Ui, label: &str, value: RichText) {
    ui.vertical(|ui| {
        ui.set_min_width(100.0);
        ui.label(label);
        ui.label(value.size(25.0).strong());
    });
}
