mod chart;
mod platform;
mod worker;

pub(crate) use platform::{notify_appearance, notify_dashboard, notify_live};

use std::{
    error::Error,
    sync::{atomic::Ordering, Arc},
};

use eframe::egui::{
    self, Align, Button, Color32, ComboBox, Frame, Layout, Modal, RichText, ScrollArea, Stroke,
    ThemePreference, Ui,
};

use crate::{
    app_icon,
    config::{AppTheme, BatteryColorMap},
};

use super::{
    design::{animation_seconds, Palette, CONTROL_HEIGHT, GAP, SECTION_PADDING, SECTION_RADIUS},
    live::{LiveDevice, LiveLevel, LiveReading},
    management::{CleanupKind, DashboardBackend},
    model::{DashboardData, DashboardDevice, DashboardInventoryDevice},
};
use platform::{LiveUpdate, Signal, SystemChanges, WindowSignals};
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
            let animation = animation_seconds();
            ctx.all_styles_mut(|style| {
                style.animation_time = animation;
                style.spacing.item_spacing = egui::vec2(GAP as f32, GAP as f32);
                style.spacing.interact_size.y = CONTROL_HEIGHT as f32;
                style.spacing.button_padding = egui::vec2(12.0, 4.0);
                for text_style in [egui::TextStyle::Body, egui::TextStyle::Button] {
                    style
                        .text_styles
                        .insert(text_style, egui::FontId::proportional(14.0));
                }
            });
            let contrast = Palette::system_contrast();
            apply_visuals(ctx, contrast);
            let system = platform::hwnd(cc)
                .and_then(|window| platform::listen_to_system(window, ctx.clone()).ok());
            let signals = WindowSignals::start(ctx.clone())?;
            let backend_is_local = backend.is_local();
            let worker = Worker::start(ctx.clone(), backend)?;
            let mut dashboard = Dashboard {
                worker,
                signals,
                system,
                contrast,
                title_theme: None,
                colors: BatteryColorMap::default(),
                live: None,
                show_live: backend_is_local,
                days,
                snapshot: None,
                selected: None,
                busy: false,
                loading: false,
                exporting: false,
                opening_settings: false,
                discard_load: false,
                artifacts_pending: false,
                artifacts: None,
                export_status: None,
                refresh_pending: false,
                status: None,
                confirmation: None,
                confirmation_focus: None,
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
    system: Option<Arc<SystemChanges>>,
    contrast: Option<Palette>,
    title_theme: Option<egui::Theme>,
    colors: BatteryColorMap,
    live: Option<Vec<LiveDevice>>,
    show_live: bool,
    days: u32,
    snapshot: Option<Arc<Snapshot>>,
    selected: Option<String>,
    busy: bool,
    loading: bool,
    exporting: bool,
    opening_settings: bool,
    discard_load: bool,
    artifacts_pending: bool,
    artifacts: Option<Result<Vec<super::management::ManagedArtifact>, String>>,
    export_status: Option<Result<String, String>>,
    refresh_pending: bool,
    status: Option<Result<String, String>>,
    confirmation: Option<Confirmation>,
    confirmation_focus: Option<Option<egui::Id>>,
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
        self.refresh_pending = true;
        if self.loading {
            return;
        }
        self.refresh_pending = false;
        self.discard_load = false;
        match self.worker.load(self.days, self.artifacts_pending) {
            Ok(()) => {
                self.loading = true;
                self.artifacts_pending = false;
            }
            Err(error) => self.status = Some(Err(error.to_string())),
        }
    }

    fn set_snapshot(&mut self, snapshot: Snapshot) {
        let data = &snapshot.data;
        if !self.selected.as_deref().is_some_and(|id| {
            data.devices.iter().any(|device| device.identity == id)
                || data.inventory.iter().any(|device| device.identity == id)
                || live_device(self.live.as_deref(), id).is_some()
        }) {
            self.selected = self
                .live
                .as_deref()
                .and_then(|items| items.first())
                .map(|device| device.identity.clone())
                .or_else(|| data.devices.first().map(|device| device.identity.clone()))
                .or_else(|| data.inventory.first().map(|device| device.identity.clone()));
        }
        self.snapshot = Some(Arc::new(snapshot));
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
                if self.artifacts.is_none() {
                    self.artifacts_pending = true;
                    self.refresh();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!self.opening_settings, Button::new("Settings"))
                    .clicked()
                {
                    match self.worker.launch_settings() {
                        Ok(()) => self.opening_settings = true,
                        Err(error) => self.status = Some(Err(error.to_string())),
                    }
                }
                if ui.button("Refresh").clicked() {
                    self.status = None;
                    self.artifacts_pending |= self.page == Page::Manage;
                    self.refresh();
                }
            });
        });
        ui.separator();
        if self.page == Page::History {
            if let Some(snapshot) = &self.snapshot {
                let devices = &snapshot.data.devices;
                let presence = self.live.as_deref().map_or_else(
                    || "Monitor status unavailable".to_owned(),
                    |devices| {
                        format!(
                            "{} online",
                            devices.iter().filter(|item| item.reading.is_some()).count()
                        )
                    },
                );
                let readings: u64 = devices.iter().map(|device| device.samples).sum();
                ui.label(format!(
                    "{} devices with history  ·  {presence}  ·  {readings} readings",
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
        if let Some(status) = &self.export_status {
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
                            true,
                        );
                        let query = self.search.trim().to_lowercase();
                        let matches = |name: &str, identity: &str| {
                            query.is_empty()
                                || name.to_lowercase().contains(&query)
                                || identity.to_lowercase().contains(&query)
                        };
                        ScrollArea::vertical().id_salt("devices").show(ui, |ui| {
                            let presence = self.live.as_deref();
                            for device in presence
                                .into_iter()
                                .flatten()
                                .filter(|device| matches(&device.name, &device.identity))
                            {
                                let (level, status) =
                                    device.reading.map_or((None, "Detected"), |reading| {
                                        (reading.level.value(), live_status(reading))
                                    });
                                device_button(
                                    ui,
                                    &mut self.selected,
                                    &device.identity,
                                    &device.name,
                                    &format!("{}  ·  {status}", live_percent(device.reading)),
                                    level,
                                );
                            }
                            if let Some(snapshot) = &self.snapshot {
                                let data = &snapshot.data;
                                for device in data.devices.iter().filter(|device| {
                                    matches(&device.name, &device.identity)
                                        && live_device(presence, &device.identity).is_none()
                                }) {
                                    device_button(
                                        ui,
                                        &mut self.selected,
                                        &device.identity,
                                        &device.name,
                                        &format!(
                                            "{}  ·  {}",
                                            percent(device.latest_percent),
                                            device_status(device, presence.is_some())
                                        ),
                                        device.latest_percent,
                                    );
                                }
                                for device in data.inventory.iter().filter(|item| {
                                    matches(&item.name, &item.identity)
                                        && live_device(presence, &item.identity).is_none()
                                        && data.devices.iter().all(|d| d.identity != item.identity)
                                }) {
                                    device_button(
                                        ui,
                                        &mut self.selected,
                                        &device.identity,
                                        &device.name,
                                        "No samples in this range",
                                        None,
                                    );
                                }
                            }
                        });
                    },
                );
                ui.add_space(6.0);
                let live = self
                    .selected
                    .as_deref()
                    .and_then(|id| live_device(self.live.as_deref(), id));
                let Some(snapshot) = &self.snapshot else {
                    if let Some(device) = live {
                        ui.heading(&device.name);
                        ui.label(format!(
                            "{}  ·  {}",
                            live_percent(device.reading),
                            device.reading.map_or("Detected", live_status)
                        ));
                    }
                    ui.label(if self.loading {
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
                                    if snapshot.data.window_days != self.days {
                                        ui.horizontal(|ui| {
                                            if self.loading {
                                                ui.spinner();
                                            }
                                            ui.label(format!(
                                                "Showing the last {} days",
                                                snapshot.data.window_days
                                            ));
                                        });
                                    }
                                    history_detail(
                                        ui,
                                        snapshot,
                                        index,
                                        !self.exporting,
                                        live,
                                        self.live.is_some(),
                                        self.colors,
                                    )
                                } else {
                                    ui.heading(
                                        live.map(|item| item.name.as_str())
                                            .or_else(|| inventory.map(|item| item.name.as_str()))
                                            .unwrap_or("Battery history"),
                                    );
                                    if let Some(reading) = live.and_then(|device| device.reading) {
                                        ui.label(format!(
                                            "{}  ·  {}",
                                            live_percent(Some(reading)),
                                            live_status(reading)
                                        ));
                                    }
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
            self.discard_load = self.loading;
            self.status = None;
            self.refresh();
        }
        if export {
            let Some((snapshot, index)) = self.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .data
                    .devices
                    .iter()
                    .position(|item| Some(item.identity.as_str()) == self.selected.as_deref())
                    .map(|index| (Arc::clone(snapshot), index))
            }) else {
                return;
            };
            match platform::save_path(frame) {
                Ok(Some(path)) => match self.worker.export(path, snapshot, index) {
                    Ok(()) => {
                        self.exporting = true;
                        self.export_status = Some(Ok("Exporting readings…".to_owned()));
                    }
                    Err(error) => self.export_status = Some(Err(error.to_string())),
                },
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
        if self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Updating data…");
            });
        }
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
                            let mut monitored = item.monitored.unwrap_or(true);
                            if ui.checkbox(&mut monitored, "Monitor device").changed() {
                                action = Some(Action::Monitoring {
                                    identity: item.identity.clone(),
                                    enabled: monitored,
                                });
                            }
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
                    match &self.artifacts {
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
                let cancel = ui.button("Cancel");
                if self.confirmation_focus.is_none() {
                    self.confirmation_focus = Some(ctx.memory(|memory| memory.focused()));
                    cancel.request_focus();
                }
                let action = ui.add(Button::new(
                    RichText::new(confirmation.action).color(ui.visuals().error_fg_color),
                ));
                if cancel.clicked() {
                    Some(false)
                } else if action.clicked() {
                    Some(true)
                } else {
                    None
                }
            })
            .inner
        });
        if response.inner.is_some() || response.should_close() {
            if let Some(Some(previous)) = self.confirmation_focus.take() {
                ctx.memory_mut(|memory| memory.request_focus(previous));
            }
            if let Some(confirmation) = self.confirmation.take() {
                if response.inner == Some(true) {
                    self.request(Action::Execute(confirmation.operation));
                }
            }
        }
    }
}

impl eframe::App for Dashboard {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(LiveUpdate(live)) = self.signals.take_live(ctx) {
            if self.show_live {
                if self.selected.is_none() {
                    self.selected = live
                        .as_deref()
                        .and_then(|items| items.first())
                        .map(|item| item.identity.clone());
                }
                self.live = live;
            }
        }
        if let Some(system) = &self.system {
            if system.appearance.swap(false, Ordering::AcqRel) {
                let animation = animation_seconds();
                ctx.all_styles_mut(|style| style.animation_time = animation);
                let contrast = Palette::system_contrast();
                if self.contrast != contrast {
                    self.contrast = contrast;
                    self.title_theme = None;
                    apply_visuals(ctx, contrast);
                }
            }
            self.refresh_pending |= system.resume.swap(false, Ordering::AcqRel);
        }
        if self.signals.take_change() {
            self.refresh_pending = true;
        }
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
                Signal::Appearance(theme, colors) => {
                    self.colors = colors;
                    ctx.set_theme(match theme {
                        AppTheme::System => ThemePreference::System,
                        AppTheme::Light => ThemePreference::Light,
                        AppTheme::Dark => ThemePreference::Dark,
                    });
                }
            }
        }
        if let Some(reply) = self.worker.poll() {
            self.busy = false;
            match reply {
                Ok(Reply::Confirm(confirmation)) => self.confirmation = Some(confirmation),
                Ok(Reply::Done(status, refresh)) => {
                    self.status = status.map(Ok);
                    self.refresh_pending |= refresh;
                    self.discard_load |= refresh;
                    self.artifacts_pending |= refresh && self.artifacts.is_some();
                }
                Err(error) => {
                    self.refresh_pending |= error.refresh;
                    self.discard_load |= error.refresh;
                    self.artifacts_pending |= error.refresh && self.artifacts.is_some();
                    self.status = Some(Err(error.message));
                }
            }
        }
        if let Some((days, result)) = self.worker.snapshot() {
            self.loading = false;
            match result {
                Ok(mut snapshot) => {
                    if let Some(artifacts) = snapshot.artifacts.take() {
                        if self.discard_load {
                            self.artifacts_pending = true;
                        } else {
                            self.artifacts = Some(artifacts);
                        }
                    }
                    if days == self.days && !self.discard_load {
                        self.set_snapshot(snapshot);
                    }
                }
                Err(error) if days == self.days && !self.discard_load => {
                    self.status = Some(Err(error));
                }
                Err(_) => {}
            }
        }
        if let Some(result) = self.worker.exported() {
            self.exporting = false;
            self.export_status = Some(result);
        }
        if let Some(result) = self.worker.settings_launched() {
            self.opening_settings = false;
            if let Err(error) = result {
                self.status = Some(Err(error.to_string()));
            }
        }
        if self.refresh_pending && !self.loading {
            self.refresh();
        }
        let theme = ctx.theme();
        if self.title_theme != Some(theme) {
            self.title_theme = Some(theme);
            platform::update_title_bar(frame, theme, self.contrast);
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

fn selected_inventory<'a>(
    data: &'a DashboardData,
    selected: Option<&str>,
) -> Option<&'a DashboardInventoryDevice> {
    data.inventory
        .iter()
        .find(|item| Some(item.identity.as_str()) == selected)
}

fn live_device<'a>(devices: Option<&'a [LiveDevice]>, identity: &str) -> Option<&'a LiveDevice> {
    let devices = devices?;
    devices
        .binary_search_by(|device| device.identity.as_str().cmp(identity))
        .ok()
        .map(|index| &devices[index])
}

fn live_status(reading: LiveReading) -> &'static str {
    if reading.charging {
        "Charging"
    } else {
        "Online"
    }
}

fn live_percent(reading: Option<LiveReading>) -> String {
    match reading.map(|reading| reading.level) {
        Some(LiveLevel::Estimated(value)) => format!("≈{value}%"),
        Some(LiveLevel::Exact(value)) => format!("{value}%"),
        _ => "—".to_owned(),
    }
}

fn device_status(device: &DashboardDevice, monitor_active: bool) -> &'static str {
    if monitor_active || !device.online {
        "Last known"
    } else {
        "Recently recorded"
    }
}

fn device_button(
    ui: &mut Ui,
    selected: &mut Option<String>,
    identity: &str,
    name: &str,
    subtitle: &str,
    level: Option<u8>,
) {
    let active = selected.as_deref() == Some(identity);
    ui.push_id(identity, |ui| {
        ui.spacing_mut().button_padding.x = 26.0;
        let label = Button::new(format!("{name}\n{subtitle}"))
            .selected(active)
            .wrap()
            .min_size(egui::vec2(ui.available_width(), 50.0));
        let response = ui.add(label).on_hover_text(format!("{name}\n{identity}"));
        if response.clicked() {
            *selected = Some(identity.to_owned());
        }
        let rect = response.rect;
        paint_device_icon(ui, rect, identity, active);
        if let Some(level) = level {
            let width = (rect.width() - 8.0).max(0.0);
            let start = egui::pos2(rect.left() + 4.0, rect.bottom() - 3.0);
            ui.painter().line_segment(
                [start, start + egui::vec2(width, 0.0)],
                ui.visuals().window_stroke,
            );
            ui.painter().line_segment(
                [
                    start,
                    start + egui::vec2(width * f32::from(level) / 100.0, 0.0),
                ],
                Stroke::new(2.0, accent(ui)),
            );
        }
    });
}

fn paint_device_icon(ui: &Ui, row: egui::Rect, identity: &str, active: bool) {
    let center = egui::pos2(row.left() + 13.0, row.center().y);
    let color = if active {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let stroke = Stroke::new(1.2, color);
    let point = |x, y| center + egui::vec2(x, y);
    let painter = ui.painter();
    if identity.starts_with("bluetooth:") {
        painter.add(egui::Shape::line(
            [
                (-4.0, -4.0),
                (4.0, 4.0),
                (0.0, 7.0),
                (0.0, -7.0),
                (4.0, -4.0),
                (-4.0, 4.0),
            ]
            .map(|(x, y)| point(x, y))
            .to_vec(),
            stroke,
        ));
    } else if identity.starts_with("xinput:") {
        painter.rect_stroke(
            egui::Rect::from_center_size(center, egui::vec2(16.0, 10.0)),
            4.0,
            stroke,
            egui::StrokeKind::Inside,
        );
        painter.line_segment([point(-5.0, 0.0), point(-1.0, 0.0)], stroke);
        painter.line_segment([point(-3.0, -2.0), point(-3.0, 2.0)], stroke);
        painter.circle_filled(point(3.5, 0.0), 1.0, color);
    } else {
        painter.rect_stroke(
            egui::Rect::from_center_size(center, egui::vec2(13.0, 10.0)),
            2.0,
            stroke,
            egui::StrokeKind::Inside,
        );
        painter.line_segment([point(-3.0, 0.0), point(3.0, 0.0)], stroke);
    }
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
    ui.visuals().panel_fill
}

fn accent(ui: &Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

fn card<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(ui.visuals().window_stroke)
        .corner_radius(SECTION_RADIUS as u8)
        .inner_margin(SECTION_PADDING as i8)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui)
        })
        .inner
}

fn apply_visuals(ctx: &egui::Context, contrast: Option<Palette>) {
    let color = |[r, g, b]: [u8; 3]| Color32::from_rgb(r, g, b);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let dark = theme == egui::Theme::Dark;
        let palette = contrast.unwrap_or_else(|| Palette::standard(dark));
        let mut visual = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visual.panel_fill = color(palette.canvas);
        visual.window_fill = color(palette.surface);
        visual.extreme_bg_color = color(palette.canvas);
        visual.window_stroke = Stroke::new(1.0, color(palette.border));
        visual.error_fg_color = color(palette.destructive);
        visual.hyperlink_color = color(palette.accent);
        visual.selection.bg_fill = if contrast.is_some() {
            color(palette.accent)
        } else {
            color(palette.accent).linear_multiply(0.24)
        };
        visual.selection.stroke.color = color(if contrast.is_some() {
            palette.on_accent
        } else {
            palette.text
        });
        visual.widgets.noninteractive.fg_stroke.color = color(palette.secondary);
        visual.widgets.noninteractive.bg_stroke.color = color(palette.border);
        visual.widgets.inactive.weak_bg_fill = color(palette.raised);
        for widget in [
            &mut visual.widgets.inactive,
            &mut visual.widgets.hovered,
            &mut visual.widgets.active,
        ] {
            widget.fg_stroke.color = color(palette.text);
            widget.bg_stroke = Stroke::new(1.0, color(palette.border));
            widget.corner_radius = egui::CornerRadius::same(5);
        }
        visual.widgets.hovered.bg_fill = color(palette.hover);
        visual.widgets.hovered.weak_bg_fill = color(palette.hover);
        visual.widgets.active.bg_stroke.color = color(palette.accent);
        visual.collapsing_header_frame = true;
        ctx.set_visuals_of(theme, visual);
    }
}

fn percent(value: Option<u8>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value}%"))
}

fn duration(seconds: i64) -> String {
    if seconds == 0 {
        return "0m".to_owned();
    }
    let minutes = seconds / 60;
    match minutes {
        0 => "<1m".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h {}m", minutes / 60, minutes % 60),
        _ => format!("{}d {}h", minutes / 1440, minutes % 1440 / 60),
    }
}

fn history_detail(
    ui: &mut Ui,
    snapshot: &Snapshot,
    index: usize,
    enabled: bool,
    live: Option<&LiveDevice>,
    monitor_active: bool,
    colors: BatteryColorMap,
) -> bool {
    let device = &snapshot.data.devices[index];
    let chart = &snapshot.charts[index];
    let live_reading = live.and_then(|device| device.reading);
    ui.heading(live.map_or(device.name.as_str(), |device| device.name.as_str()));
    ui.label(format!(
        "{}  ·  Last reading {}",
        live_reading.map_or_else(|| device_status(device, monitor_active), live_status),
        platform::date(device.last_at)
    ));
    card(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let level = live_reading.map_or(device.latest_percent, |reading| reading.level.value());
            let level_color = level.map_or_else(
                || accent(ui),
                |value| {
                    let [r, g, b] = colors.color_for(value);
                    Color32::from_rgb(r, g, b)
                },
            );
            metric(
                ui,
                "Battery",
                RichText::new(if live_reading.is_some() {
                    live_percent(live_reading)
                } else {
                    percent(level)
                })
                .color(level_color),
            );
            metric(
                ui,
                "Est. remaining",
                RichText::new(
                    live_reading
                        .map_or(device.eta_seconds, |reading| {
                            reading
                                .remaining_seconds
                                .and_then(|seconds| i64::try_from(seconds).ok())
                        })
                        .map_or_else(|| "—".to_owned(), duration),
                ),
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
        ui.horizontal_wrapped(|ui| {
            let observed = device.online_seconds + device.offline_seconds;
            if observed > 0 {
                ui.label(format!(
                    "{:.0}% online",
                    device.online_seconds as f64 / observed as f64 * 100.0
                ))
                .on_hover_text(
                    "Estimated from recorded samples and gaps. Unknown time is excluded.",
                );
            }
            ui.colored_label(
                accent(ui),
                format!("Online {}", duration(device.online_seconds)),
            );
            ui.label(format!("Offline {}", duration(device.offline_seconds)));
            let unknown = chart.unknown_seconds(device);
            if unknown > 0 {
                ui.label(format!("Unknown {}", duration(unknown)));
            }
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
            let sort = ui.scope_id().with("readings-order");
            let mut oldest_first = ui.data(|data| data.get_temp::<bool>(sort)).unwrap_or(false);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(if oldest_first {
                        "Local time ↑"
                    } else {
                        "Local time ↓"
                    })
                    .clicked()
                {
                    oldest_first = !oldest_first;
                    ui.data_mut(|data| data.insert_temp(sort, oldest_first));
                }
                ui.monospace("Battery · Voltage · Charging");
            });
            ScrollArea::vertical()
                .id_salt("reading rows")
                .max_height(240.0)
                .show_rows(
                    ui,
                    ui.text_style_height(&egui::TextStyle::Monospace),
                    device.points.len(),
                    |ui, rows| {
                        for row in rows {
                            let index = if oldest_first {
                                row
                            } else {
                                device.points.len() - 1 - row
                            };
                            let point = &device.points[index];
                            ui.monospace(format!(
                                "{:<19}  {:>7}  {:>9}  {:>9}",
                                platform::exact_date(point.t),
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
