mod analytics;
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

use crate::{app_icon, config::AppTheme};

use super::{
    design::{animation_seconds, Palette, CONTROL_HEIGHT, GAP, SECTION_PADDING, SECTION_RADIUS},
    live::{LiveDevice, LiveLevel, LiveReading},
    management::{CleanupKind, DashboardBackend},
    model::{DashboardData, DashboardInventoryDevice},
};
use platform::{LiveUpdate, Signal, SystemChanges, WindowSignals};
use worker::{Action, Confirmation, Reply, Snapshot, Worker};

#[derive(Default)]
pub(super) struct WindowOptions {
    pub(super) size: Option<[u32; 2]>,
    pub(super) preview: Option<PreviewOptions>,
}

pub(super) struct PreviewOptions {
    pub(super) theme: AppTheme,
    pub(super) live: Vec<LiveDevice>,
    pub(super) manage: bool,
    pub(super) cleanup: bool,
}

pub(super) fn open_dashboard_window(
    backend: DashboardBackend,
    days: u32,
    launch: WindowOptions,
) -> Result<(), Box<dyn Error>> {
    let size = launch.size.unwrap_or([1080, 760]).map(|value| value as f32);
    let title = if backend.is_preview() {
        "Volty UI Preview — Dashboard (Loading)"
    } else {
        "Volty Dashboard"
    };
    let icon = app_icon::dashboard_icon()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size([660.0, 460.0])
            .with_icon(icon.clone()),
        ..Default::default()
    };
    Ok(eframe::run_native(
        title,
        options,
        Box::new(move |cc| {
            let ctx = &cc.egui_ctx;
            let animation = animation_seconds();
            install_heading_font(ctx);
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
            let signals = if backend.is_preview() {
                None
            } else {
                Some(WindowSignals::start(ctx.clone())?)
            };
            let preview_theme = launch.preview.as_ref().map(|preview| preview.theme);
            if let Some(theme) = preview_theme {
                apply_theme(ctx, theme);
            }
            let manage = launch
                .preview
                .as_ref()
                .is_some_and(|preview| preview.manage);
            let cleanup = launch
                .preview
                .as_ref()
                .is_some_and(|preview| preview.cleanup);
            let backend_is_local = backend.is_local();
            let worker = Worker::start(ctx.clone(), backend)?;
            let brand = ctx.load_texture(
                "Volty",
                egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                ),
                egui::TextureOptions::LINEAR,
            );
            let mut dashboard = Dashboard {
                analytics: analytics::HistoryView::default(),
                brand,
                worker,
                signals,
                system,
                contrast,
                title_theme: None,
                live: launch.preview.map(|preview| preview.live),
                preview_theme,
                preview_cleanup: cleanup,
                preview_ready: false,
                show_live: backend_is_local,
                days,
                snapshot: None,
                selected: None,
                busy: false,
                loading: false,
                exporting: false,
                opening_settings: false,
                discard_load: false,
                artifacts_pending: manage,
                artifacts: None,
                export_status: None,
                refresh_pending: false,
                status: None,
                confirmation: None,
                confirmation_focus: None,
                page: if manage { Page::Manage } else { Page::History },
                older_days: 7,
                inactive_days: 7,
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
    analytics: analytics::HistoryView,
    brand: egui::TextureHandle,
    worker: Worker,
    signals: Option<WindowSignals>,
    system: Option<Arc<SystemChanges>>,
    contrast: Option<Palette>,
    title_theme: Option<egui::Theme>,
    live: Option<Vec<LiveDevice>>,
    preview_theme: Option<AppTheme>,
    preview_cleanup: bool,
    preview_ready: bool,
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
                .iter()
                .flatten()
                .find(|device| {
                    data.devices
                        .iter()
                        .any(|history| history.identity == device.identity)
                })
                .map(|device| device.identity.clone())
                .or_else(|| data.devices.first().map(|device| device.identity.clone()))
                .or_else(|| {
                    self.live
                        .as_deref()
                        .and_then(|items| items.first())
                        .map(|device| device.identity.clone())
                })
                .or_else(|| data.inventory.first().map(|device| device.identity.clone()));
        }
        if self.snapshot.is_none() {
            self.analytics
                .initial_comparison(data, self.selected.as_deref());
        }
        self.analytics.invalidate_readings();
        self.snapshot = Some(Arc::new(snapshot));
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.image((self.brand.id(), egui::vec2(28.0, 28.0)));
            ui.label(analytics::heading(ui, "Volty", 22.0));
            ui.add_space(12.0);
            if ui
                .add(analytics::tab(
                    self.page == Page::History,
                    "Battery history",
                ))
                .clicked()
            {
                self.page = Page::History;
            }
            if ui
                .add(analytics::tab(self.page == Page::Manage, "Manage data"))
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
                    match self.worker.launch_settings(self.preview_theme) {
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
        if let Some(LiveUpdate(live)) = self
            .signals
            .as_ref()
            .and_then(|signals| signals.take_live(ctx))
        {
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
                install_heading_font(ctx);
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
        if self
            .signals
            .as_ref()
            .is_some_and(WindowSignals::take_change)
        {
            self.refresh_pending = true;
        }
        while let Some(signal) = self
            .signals
            .as_ref()
            .and_then(|signals| signals.receive.try_recv().ok())
        {
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
                Signal::Appearance(theme) => {
                    apply_theme(ctx, theme);
                    self.refresh_pending = true;
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
                        if std::mem::take(&mut self.preview_cleanup) {
                            self.send(Action::PreviewCleanup(
                                CleanupKind::OlderThan,
                                self.older_days,
                            ));
                        }
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
        Frame::new().fill(canvas(ui)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.spacing_mut().item_spacing.y = 0.0;
            Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 12))
                .show(ui, |ui| self.header(ui));
            ui.separator();
            match self.page {
                Page::History => self.history(ui, frame),
                Page::Manage => {
                    Frame::new().inner_margin(20).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = GAP as f32;
                        self.manage(ui);
                    });
                }
            }
        });
        self.confirm(ui.ctx());
        if self.preview_theme.is_some()
            && !self.preview_ready
            && self.snapshot.is_some()
            && !self.busy
        {
            self.preview_ready = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(
                "Volty UI Preview — Dashboard".to_owned(),
            ));
        }
    }
}

fn install_heading_font(ctx: &egui::Context) {
    let Some(windows) = std::env::var_os("WINDIR") else {
        return;
    };
    let path = std::path::PathBuf::from(windows)
        .join("Fonts")
        .join("seguisb.ttf");
    let Ok(data) = std::fs::read(path) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    let family = egui::FontFamily::Name("Volty headings".into());
    let name = "Segoe UI Semibold".to_owned();
    fonts
        .font_data
        .insert(name.clone(), Arc::new(egui::FontData::from_owned(data)));
    let mut fallback = fonts.families[&egui::FontFamily::Proportional].clone();
    fallback.insert(0, name);
    fonts.families.insert(family.clone(), fallback);
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(18.0, family.clone()),
        );
    });
}

fn apply_theme(ctx: &egui::Context, theme: AppTheme) {
    ctx.set_theme(match theme {
        AppTheme::System => ThemePreference::System,
        AppTheme::Light => ThemePreference::Light,
        AppTheme::Dark => ThemePreference::Dark,
    });
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

fn live_percent(reading: Option<LiveReading>) -> String {
    match reading.map(|reading| reading.level) {
        Some(LiveLevel::Estimated(value)) => format!("≈{value}%"),
        Some(LiveLevel::Exact(value)) => format!("{value}%"),
        _ => "—".to_owned(),
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
        let palette = contrast.unwrap_or_else(|| {
            let mut palette = Palette::standard(dark);
            if dark {
                palette.canvas = [20, 26, 35];
                palette.surface = [29, 37, 48];
                palette.raised = [37, 47, 60];
                palette.hover = [51, 65, 81];
                palette.border = [52, 65, 81];
                palette.text = [242, 245, 248];
                palette.secondary = [170, 183, 198];
                palette.accent = [57, 201, 237];
            }
            palette
        });
        let mut visual = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        visual.override_text_color = Some(color(palette.text));
        visual.panel_fill = color(palette.canvas);
        visual.window_fill = color(palette.surface);
        visual.extreme_bg_color = color(palette.canvas);
        visual.window_stroke = Stroke::new(1.0, color(palette.border));
        visual.error_fg_color = color(palette.destructive);
        visual.weak_text_color = Some(color(palette.secondary));
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
        visual.widgets.inactive.bg_fill = color(palette.raised);
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
        _ => format!(
            "{}d {}h {}m",
            minutes / 1440,
            minutes % 1440 / 60,
            minutes % 60
        ),
    }
}
