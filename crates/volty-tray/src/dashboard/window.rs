mod analytics;
mod assets;
mod chart;
mod manage;
mod platform;
mod settings;
mod worker;

pub(crate) use platform::{notify_appearance, notify_dashboard, notify_live};

use std::{
    error::Error,
    sync::{atomic::Ordering, Arc},
};

use eframe::egui::{
    self, Align, Button, Color32, ComboBox, Frame, Layout, Modal, RichText, Stroke,
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
    pub(super) settings: bool,
}

pub(super) struct PreviewOptions {
    pub(super) theme: AppTheme,
    pub(super) live: Vec<LiveDevice>,
    pub(super) manage: bool,
    pub(super) cleanup: bool,
    pub(super) invalid_settings: bool,
}

pub(super) fn open_dashboard_window(
    backend: DashboardBackend,
    days: u32,
    launch: WindowOptions,
) -> Result<(), Box<dyn Error>> {
    let size = launch.size.unwrap_or([1080, 850]).map(|value| value as f32);
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
            let signals = if backend.is_local() {
                Some(WindowSignals::start(ctx.clone())?)
            } else {
                None
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
            let (worker, settings_worker) = Worker::start(ctx.clone(), backend)?;
            let invalid_settings = launch
                .preview
                .as_ref()
                .is_some_and(|preview| preview.invalid_settings);
            let mut dashboard = Dashboard {
                analytics: analytics::HistoryView::default(),
                manage_view: manage::ManageView::default(),
                management_refresh: false,
                pending_preference: None,
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
                close_after_export: false,
                settings: settings::Settings::new(ctx, settings_worker, invalid_settings),
                committed_theme: preview_theme.unwrap_or(AppTheme::System),
                discard_load: false,
                artifacts_pending: manage,
                artifacts: None,
                export_status: None,
                refresh_pending: false,
                status: None,
                confirmation: None,
                confirmation_focus: None,
                action_focus: None,
                page: if launch.settings {
                    Page::Settings
                } else if manage {
                    Page::Manage
                } else {
                    Page::History
                },
                older_days: 7,
                inactive_days: 7,
            };
            dashboard.refresh();
            if launch.settings {
                dashboard.settings.open();
            }
            Ok(Box::new(dashboard))
        }),
    )?)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    History,
    Manage,
    Settings,
}

struct Dashboard {
    analytics: analytics::HistoryView,
    manage_view: manage::ManageView,
    management_refresh: bool,
    pending_preference: Option<(String, bool, bool)>,
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
    close_after_export: bool,
    settings: settings::Settings,
    committed_theme: AppTheme,
    discard_load: bool,
    artifacts_pending: bool,
    artifacts: Option<Result<Vec<super::management::ManagedArtifact>, String>>,
    export_status: Option<Result<String, String>>,
    refresh_pending: bool,
    status: Option<Result<String, String>>,
    confirmation: Option<Confirmation>,
    confirmation_focus: Option<Option<egui::Id>>,
    action_focus: Option<egui::Id>,
    page: Page,
    older_days: u32,
    inactive_days: u32,
}

impl Dashboard {
    fn request(&mut self, action: Action) {
        if self.busy {
            return;
        }
        self.status = None;
        let preference = match &action {
            Action::Monitoring { identity, enabled } => Some((identity.clone(), true, *enabled)),
            Action::Recording { identity, enabled } => Some((identity.clone(), false, *enabled)),
            _ => None,
        };
        self.send(action);
        if self.busy {
            self.pending_preference = preference;
        }
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
            self.selected = data
                .inventory
                .iter()
                .find(|item| item.pinned)
                .map(|item| item.identity.clone())
                .filter(|id| data.devices.iter().any(|device| device.identity == *id))
                .or_else(|| {
                    self.live
                        .iter()
                        .flatten()
                        .find(|device| {
                            data.devices
                                .iter()
                                .any(|history| history.identity == device.identity)
                        })
                        .map(|device| device.identity.clone())
                })
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
        if !self.busy {
            self.pending_preference = None;
            self.management_refresh = false;
        }
        self.analytics
            .register_colors(&data.inventory, self.selected.as_deref());
        self.analytics.invalidate_readings();
        self.snapshot = Some(Arc::new(snapshot));
    }

    fn navigate(&mut self, page: Page) {
        if self.page != page && !self.settings.intercept(settings::Destination::Page(page)) {
            self.set_page(page);
        }
    }

    fn set_page(&mut self, page: Page) {
        self.page = page;
        if page == Page::Settings {
            self.settings.open();
        } else if page == Page::Manage && self.artifacts.is_none() {
            self.artifacts_pending = true;
            self.refresh();
        }
    }

    fn finish_settings_navigation(
        &mut self,
        ctx: &egui::Context,
        destination: settings::Destination,
    ) {
        match destination {
            settings::Destination::Page(page) => self.set_page(page),
            settings::Destination::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            assets::Icon::Brand.show(ui, 25.0, Color32::WHITE);
            ui.label(analytics::heading(ui, "Volty", 21.0));
            ui.add_space(12.0);
            for (page, label) in [
                (Page::History, "Battery history"),
                (Page::Manage, "Manage data"),
                (Page::Settings, "Settings"),
            ] {
                if ui.add(analytics::tab(self.page == page, label)).clicked() {
                    self.navigate(page);
                }
            }
            if self.page != Page::Settings {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Refresh").clicked() {
                        self.status = None;
                        self.artifacts_pending |= self.page == Page::Manage;
                        self.refresh();
                    }
                    if self.page == Page::History && self.live.is_some() {
                        ui.label(
                            RichText::new(if self.preview_theme.is_some() {
                                "Monitoring · Demo"
                            } else {
                                "Monitoring"
                            })
                            .weak()
                            .size(11.0),
                        );
                    }
                });
            }
        });

        if self.page == Page::Manage && (self.busy || self.management_refresh) {
            ui.label("Updating data…");
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

    fn confirm(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = &self.confirmation else {
            return;
        };
        let response = Modal::new(egui::Id::unique("confirm dashboard change"))
            .frame(Frame::popup(&ctx.style_of(ctx.theme())).inner_margin(SECTION_PADDING as i8))
            .show(ctx, |ui| {
                ui.set_width(410.0);
                ui.heading(&confirmation.title);
                ui.label(&confirmation.description);
                if !confirmation.impact.is_empty() {
                    Frame::new()
                        .fill(ui.visuals().faint_bg_color)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .max_height(150.0)
                                .show(ui, |ui| {
                                    for device in &confirmation.impact {
                                        ui.label(device);
                                    }
                                });
                        });
                }
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "This action is permanent and cannot be undone.",
                );
                ui.horizontal(|ui| {
                    let cancel = ui.button("Cancel");
                    if self.confirmation_focus.is_none() {
                        self.confirmation_focus = Some(
                            self.action_focus
                                .take()
                                .or_else(|| ctx.memory(|memory| memory.focused())),
                        );
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
                self.settings.refresh_taskbar_theme();
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
                Signal::Activate | Signal::Settings => {
                    if matches!(signal, Signal::Settings) {
                        self.navigate(Page::Settings);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.refresh();
                }
                Signal::Shutdown => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                Signal::Appearance(theme) => {
                    self.committed_theme = theme;
                    self.refresh_pending = true;
                }
            }
        }
        if let Some(reply) = self.worker.poll() {
            self.busy = false;
            match reply {
                Ok(Reply::Confirm(confirmation)) => {
                    let current_target = match &confirmation.operation {
                        worker::Operation::Forget { identity, .. } => {
                            self.selected.as_ref() == Some(identity)
                        }
                        _ => true,
                    };
                    if self.page == Page::Manage && current_target {
                        self.confirmation = Some(confirmation);
                    }
                }
                Ok(Reply::Done(status, refresh)) => {
                    self.status = status.map(Ok);
                    self.management_refresh = refresh;
                    if refresh && self.page == Page::Settings {
                        self.settings.open();
                    }
                    self.refresh_pending |= refresh;
                    self.discard_load |= refresh;
                    self.artifacts_pending |= refresh && self.artifacts.is_some();
                }
                Err(error) => {
                    self.pending_preference = None;
                    self.management_refresh = error.refresh;
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
                    self.management_refresh = false;
                    self.pending_preference = None;
                    self.status = Some(Err(error));
                }
                Err(_) => {}
            }
        }
        let close_after_export = if let Some(result) = self.worker.exported() {
            self.exporting = false;
            let close = std::mem::take(&mut self.close_after_export) && result.is_ok();
            self.export_status = Some(result);
            close
        } else {
            false
        };
        if !self.busy && !self.management_refresh && self.confirmation.is_none() {
            if let Some(focus) = self
                .action_focus
                .take()
                .filter(|_| self.page == Page::Manage)
            {
                ctx.memory_mut(|memory| {
                    if memory.focused().is_none() {
                        memory.request_focus(focus);
                    }
                });
            }
        }
        let (saved, destination) = self.settings.poll();
        if saved {
            if let Some(theme) = self.settings.preview_theme() {
                self.committed_theme = theme;
            }
            self.refresh_pending = true;
            self.discard_load = true;
        }
        if let Some(destination) = destination {
            self.finish_settings_navigation(ctx, destination);
        }
        let close_requested = ctx.input(|input| input.viewport().close_requested());
        if close_requested || close_after_export {
            if self.exporting {
                self.close_after_export = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.settings.intercept(settings::Destination::Close) {
                if close_requested {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            } else if close_after_export && !close_requested {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let theme = if self.page == Page::Settings {
            self.settings
                .preview_theme()
                .unwrap_or(self.committed_theme)
        } else {
            self.committed_theme
        };
        apply_theme(ctx, theme);
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
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| self.header(ui));
            ui.separator();
            match self.page {
                Page::History => self.history(ui, frame),
                Page::Settings => self.settings.ui(
                    ui,
                    frame,
                    self.snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.data.inventory.as_slice()),
                ),
                Page::Manage => self.manage(ui),
            }
        });
        self.confirm(ui.ctx());
        if let Some(destination) = self.settings.confirm(ui.ctx()) {
            self.finish_settings_navigation(ui.ctx(), destination);
        }
        if self.preview_theme.is_some()
            && !self.preview_ready
            && self.snapshot.is_some()
            && !self.busy
            && (self.page != Page::Settings || self.settings.ready())
        {
            self.preview_ready = true;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(
                if self.page == Page::Settings {
                    "Volty UI Preview — Settings"
                } else {
                    "Volty UI Preview — Dashboard"
                }
                .to_owned(),
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

fn day_picker(ui: &mut Ui, id: &str, days: &mut u32, choices: &[u32]) {
    let label = |days| {
        if days == 1 {
            "1 day".to_owned()
        } else {
            format!("{days} days")
        }
    };
    ComboBox::from_id_salt(id)
        .selected_text(label(*days))
        .show_ui(ui, |ui| {
            for &choice in choices {
                ui.selectable_value(days, choice, label(choice));
            }
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
                palette.hover = [48, 61, 75];
                palette.border = [56, 68, 82];
                palette.text = [242, 245, 248];
                palette.secondary = [184, 195, 207];
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
