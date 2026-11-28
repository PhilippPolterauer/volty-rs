use std::{
    io,
    sync::{mpsc, Arc},
    thread,
};

use eframe::egui::{self, vec2, Color32, Context, Frame, RichText, Ui};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::Controls::Dialogs::{ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW},
};

use super::super::{management::SettingsData, model::DashboardSettings};
use super::{accent, analytics, card, platform, DashboardBackend, DashboardInventoryDevice, Page};
use crate::{
    config::{
        normalize_low_battery_thresholds, AppTheme, BatteryColorMap, DashboardSettingsUpdate,
    },
    state::{TrayIconStyle, TrayIconTheme},
};

mod validation;

enum Action {
    Load,
    Save(Box<DashboardSettingsUpdate>),
    Test,
}

enum Reply {
    Loaded(Result<SettingsData, String>),
    Saved(Result<SettingsData, String>),
    SaveFailed {
        error: String,
        stored: Option<SettingsData>,
    },
    Tested(Result<(), String>),
}

pub(super) struct Worker {
    actions: mpsc::Sender<Action>,
    replies: mpsc::Receiver<Reply>,
}

impl Worker {
    pub(super) fn start(ctx: Context, backend: Arc<DashboardBackend>) -> io::Result<Self> {
        let (actions, requests) = mpsc::channel();
        let (responses, replies) = mpsc::channel();
        thread::Builder::new()
            .name("volty-settings".into())
            .spawn(move || {
                while let Ok(action) = requests.recv() {
                    let load = || backend.get_settings_data().map_err(|err| err.to_string());
                    let reply = match action {
                        Action::Load => Reply::Loaded(load()),
                        Action::Save(update) => {
                            let result = backend
                                .update_settings(&update)
                                .map_err(|err| err.to_string());
                            backend.notify_preferences();
                            match result {
                                Ok(()) => Reply::Saved(load()),
                                Err(error) => Reply::SaveFailed {
                                    error,
                                    stored: load().ok(),
                                },
                            }
                        }
                        Action::Test => Reply::Tested(
                            backend.test_notification().map_err(|err| err.to_string()),
                        ),
                    };
                    if responses.send(reply).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })?;
        Ok(Self { actions, replies })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Draft {
    preferences: DashboardSettings,
    thresholds: String,
    repeat: String,
    red_at: String,
    yellow_at: String,
}

impl Draft {
    fn new(preferences: DashboardSettings) -> Self {
        Self {
            thresholds: preferences
                .low_battery_thresholds
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            repeat: preferences.low_battery_cooldown_minutes.to_string(),
            red_at: preferences.battery_color_red_max_percent.to_string(),
            yellow_at: preferences.battery_color_yellow_max_percent.to_string(),
            preferences,
        }
    }

    fn colors(&self) -> Result<BatteryColorMap, &'static str> {
        let p = &self.preferences;
        validation::validate_color_map(
            &self.red_at,
            &self.yellow_at,
            &p.battery_color_red,
            &p.battery_color_yellow,
            &p.battery_color_green,
        )
    }

    fn thresholds(&self) -> Result<Vec<u8>, &'static str> {
        let values = validation::validate_thresholds(&self.thresholds)?;
        let normalized = normalize_low_battery_thresholds(&values)
            .map_err(|_| "Enter percentages from 1 to 100.")?;
        if normalized.len() != values.len() {
            return Err("Enter each threshold only once.");
        }
        Ok(normalized)
    }

    fn theme(&self) -> AppTheme {
        AppTheme::parse(&self.preferences.app_theme).unwrap_or(AppTheme::System)
    }

    fn update(&self, original: &Self) -> Result<DashboardSettingsUpdate, &'static str> {
        let p = &self.preferences;
        let colors = self.colors()?;
        Ok(DashboardSettingsUpdate {
            app_theme: p.app_theme.clone(),
            battery_color_red_max_percent: colors.red_max_percent,
            battery_color_yellow_max_percent: colors.yellow_max_percent,
            battery_color_red: validation::hex_color(colors.red),
            battery_color_yellow: validation::hex_color(colors.yellow),
            battery_color_green: validation::hex_color(colors.green),
            low_battery_thresholds: self.thresholds()?,
            low_battery_cooldown_minutes: validation::validate_cooldown(&self.repeat)?,
            notifications_enabled: p.notifications_enabled,
            tray_icon_style: p.tray_icon_style.clone(),
            tray_icon_theme: p.tray_icon_theme.clone(),
            show_percentage_in_battery_icon: p.show_percentage_in_battery_icon,
            pinned_device: p.pinned_device.clone(),
            start_on_boot: p
                .start_on_boot
                .filter(|enabled| original.preferences.start_on_boot != Some(*enabled)),
        })
    }
}

#[derive(Clone, Copy)]
pub(super) enum Destination {
    Page(Page),
    Close,
}

pub(super) struct Settings {
    worker: Worker,
    draft: Option<Draft>,
    original: Option<Draft>,
    inventory: Vec<DashboardInventoryDevice>,
    loading: bool,
    loaded: bool,
    invalid_preview: bool,
    saving: bool,
    testing: bool,
    status: Option<Result<String, String>>,
    pending: Option<Destination>,
    after_save: Option<Destination>,
    section: usize,
    jump: bool,
    level: u8,
    colors: BatteryColorMap,
    icon_key: Option<(BatteryColorMap, TrayIconStyle, TrayIconTheme, bool, u8, u32)>,
    previews: Vec<egui::TextureHandle>,
    icons: [egui::TextureHandle; 4],
    custom_colors: [u32; 16],
    taskbar_dark: bool,
}

const SECTIONS: [&str; 4] = [
    "Battery alerts",
    "Appearance & tray",
    "Battery colors",
    "Windows startup",
];
const ICONS: [&[u8]; 4] = [
    include_bytes!("../../../assets/settings/bell.png"),
    include_bytes!("../../../assets/settings/monitor.png"),
    include_bytes!("../../../assets/settings/palette.png"),
    include_bytes!("../../../assets/settings/power.png"),
];

impl Settings {
    pub(super) fn new(ctx: &Context, worker: Worker, invalid_preview: bool) -> Self {
        let icons = std::array::from_fn(|i| {
            let png = image::load_from_memory(ICONS[i])
                .expect("bundled Settings icon")
                .into_rgba8();
            let mut pixels = png.into_raw();
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[..3].fill(255);
            }
            ctx.load_texture(
                SECTIONS[i],
                egui::ColorImage::from_rgba_unmultiplied([20, 20], &pixels),
                egui::TextureOptions::LINEAR,
            )
        });
        Self {
            worker,
            draft: None,
            original: None,
            inventory: Vec::new(),
            loading: false,
            loaded: false,
            invalid_preview,
            saving: false,
            testing: false,
            status: None,
            pending: None,
            after_save: None,
            section: 0,
            jump: false,
            level: 65,
            colors: BatteryColorMap::default(),
            icon_key: None,
            previews: Vec::new(),
            icons,
            custom_colors: [0; 16],
            taskbar_dark: crate::tray::system_taskbar_is_dark(),
        }
    }

    pub(super) fn refresh_taskbar_theme(&mut self) {
        self.taskbar_dark = crate::tray::system_taskbar_is_dark();
        self.icon_key = None;
    }

    pub(super) fn open(&mut self) {
        if self.loading || self.dirty() || self.saving {
            return;
        }
        self.loading = self.send(Action::Load);
    }

    fn send(&mut self, action: Action) -> bool {
        if let Err(err) = self.worker.actions.send(action) {
            self.status = Some(Err(err.to_string()));
            return false;
        }
        true
    }

    pub(super) fn ready(&self) -> bool {
        self.loaded
    }

    pub(super) fn dirty(&self) -> bool {
        self.draft != self.original
    }

    pub(super) fn intercept(&mut self, target: Destination) -> bool {
        if self.saving {
            self.after_save = Some(target);
            true
        } else if self.dirty() {
            self.pending = Some(target);
            true
        } else {
            false
        }
    }

    pub(super) fn preview_theme(&self) -> Option<AppTheme> {
        self.draft.as_ref().map(Draft::theme)
    }

    fn install(&mut self, data: SettingsData) {
        self.inventory = data.inventory;
        self.original = data.settings.map(Draft::new);
        if let Some(colors) = self.original.as_ref().and_then(|draft| draft.colors().ok()) {
            self.colors = colors;
        }
        self.draft = self.original.clone();
    }

    pub(super) fn poll(&mut self) -> (bool, Option<Destination>) {
        let mut saved = false;
        let mut destination = None;
        while let Ok(reply) = self.worker.replies.try_recv() {
            match reply {
                Reply::Loaded(result) => {
                    self.loading = false;
                    self.loaded = true;
                    match result {
                        Ok(data) => {
                            self.install(data);
                            if std::mem::take(&mut self.invalid_preview) {
                                if let Some(draft) = &mut self.draft {
                                    draft.thresholds = "120".into();
                                }
                            }
                        }
                        Err(error) => self.status = Some(Err(error)),
                    }
                }
                Reply::Saved(result) => {
                    self.saving = false;
                    match result {
                        Ok(data) => {
                            self.install(data);
                            self.status = Some(Ok("Settings saved.".into()));
                            destination = self.after_save.take();
                            saved = true;
                        }
                        Err(error) => {
                            self.after_save = None;
                            self.status =
                                Some(Err(format!("Could not finish saving settings. {error}")));
                        }
                    }
                }
                Reply::SaveFailed { error, stored } => {
                    self.saving = false;
                    self.after_save = None;
                    if let Some(stored) = stored {
                        self.original = stored.settings.map(Draft::new);
                        self.inventory = stored.inventory;
                    }
                    self.status = Some(Err(format!("Could not finish saving settings. {error}")));
                }
                Reply::Tested(result) => {
                    self.testing = false;
                    self.status = Some(result.map(|()| "Test notification sent.".into()));
                }
            }
        }
        (saved, destination)
    }

    fn update(&self) -> Option<DashboardSettingsUpdate> {
        let draft = self.draft.as_ref()?;
        if draft.preferences.config_error.is_some() {
            return None;
        }
        let saved = self.original.as_ref()?;
        self.dirty().then(|| draft.update(saved).ok()).flatten()
    }

    fn save(&mut self, destination: Option<Destination>) {
        if let Some(update) = self.update() {
            self.saving = self.send(Action::Save(Box::new(update)));
            if self.saving {
                self.after_save = destination;
            }
        }
    }

    fn discard(&mut self) {
        self.draft = self.original.clone();
        self.status = None;
        if let Some(colors) = self.draft.as_ref().and_then(|draft| draft.colors().ok()) {
            self.colors = colors;
        }
    }

    pub(super) fn confirm(&mut self, ctx: &Context) -> Option<Destination> {
        let target = self.pending?;
        let response = egui::Modal::new(egui::Id::unique("unsaved-settings")).show(ctx, |ui| {
            ui.heading("Unsaved settings");
            ui.label("Save or discard your changes before leaving Settings.");
            ui.horizontal(|ui| {
                if ui.button("Stay").clicked() {
                    self.pending = None;
                }
                if ui.button("Discard changes").clicked() {
                    self.discard();
                    self.pending = None;
                    return Some(target);
                }
                if ui
                    .add_enabled(self.update().is_some(), egui::Button::new("Save changes"))
                    .clicked()
                {
                    self.pending = None;
                    self.save(Some(target));
                }
                None
            })
            .inner
        });
        if response.should_close() {
            self.pending = None;
        }
        response.inner
    }

    fn index(&mut self, ui: &mut Ui, wide: bool) {
        if wide {
            ui.label(RichText::new("SETTINGS").size(12.0).strong().weak());
        }
        for (i, section) in SECTIONS.iter().enumerate() {
            let selected = self.section == i;
            let label = egui::Button::image_and_text(
                egui::Image::new((self.icons[i].id(), vec2(18.0, 18.0))).tint(if selected {
                    accent(ui)
                } else {
                    ui.visuals().weak_text_color()
                }),
                *section,
            )
            .selected(selected)
            .frame(selected);
            if ui
                .add(if wide {
                    label.min_size(vec2(188.0, 40.0))
                } else {
                    label
                })
                .clicked()
            {
                self.section = i;
                self.jump = true;
            }
        }
        if wide {
            ui.add_space(18.0);
            ui.separator();
            ui.label(
                RichText::new("Preferences apply across Volty. Changes are saved explicitly.")
                    .size(12.0)
                    .weak(),
            );
        }
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        frame: &eframe::Frame,
        inventory: Option<&[DashboardInventoryDevice]>,
    ) {
        if self.draft.is_none() {
            Frame::new().inner_margin(24).show(ui, |ui| {
                ui.heading("Settings");
                if self.loading {
                    ui.spinner();
                } else if let Some(Err(error)) = &self.status {
                    error_label(ui, error);
                    if ui.button("Retry").clicked() {
                        self.open();
                    }
                } else {
                    ui.label(
                        "Preferences are unavailable when viewing an external history database.",
                    );
                }
            });
            return;
        }
        let wide = ui.available_width() >= 1000.0;
        let owner = platform::hwnd(frame);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            if wide {
                Frame::new()
                    .fill(ui.visuals().faint_bg_color)
                    .inner_margin(16)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(188.0);
                            ui.set_min_height((height - 32.0).max(0.0));
                            ui.spacing_mut().item_spacing.y = 6.0;
                            self.index(ui, true);
                        });
                    });
            }
            ui.vertical(|ui| {
                if !wide {
                    Frame::new()
                        .inner_margin(egui::Margin::symmetric(18, 6))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| self.index(ui, false));
                        });
                    ui.separator();
                }
                let actions = 58.0;
                egui::ScrollArea::vertical()
                    .id_salt("settings-content")
                    .auto_shrink([false, false])
                    .max_height((ui.available_height() - actions).max(80.0))
                    .show(ui, |ui| {
                        Frame::new()
                            .inner_margin(egui::Margin::symmetric(if wide { 36 } else { 20 }, 22))
                            .show(ui, |ui| self.content(ui, owner, inventory));
                    });
                if actions != 0.0 {
                    ui.separator();
                    Frame::new()
                        .inner_margin(egui::Margin::symmetric(18, 8))
                        .show(ui, |ui| self.footer(ui));
                }
            });
        });
    }

    fn footer(&mut self, ui: &mut Ui) {
        let save = !self.saving && self.update().is_some();
        let save_shortcut =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S));
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fill = accent(ui);
                let text = ui.visuals().selection.stroke.color;
                let button =
                    egui::Button::new(RichText::new("Save changes").color(text)).fill(fill);
                if ui.add_enabled(save, button).clicked() || (save && save_shortcut) {
                    self.save(None);
                }
                if ui
                    .add_enabled(
                        self.dirty() && !self.saving,
                        egui::Button::new("Discard changes"),
                    )
                    .clicked()
                {
                    self.discard();
                }
                if self.saving {
                    ui.label("Saving…");
                } else if let Some(result) = &self.status {
                    match result {
                        Ok(message) => {
                            ui.label(message);
                        }
                        Err(error) => error_label(ui, error),
                    }
                } else if self.dirty() {
                    ui.label("Unsaved changes");
                } else {
                    ui.label(
                        RichText::new("Your preferences are up to date.")
                            .size(12.0)
                            .weak(),
                    );
                }
            });
        });
    }

    fn heading(&mut self, ui: &mut Ui, i: usize, title: &str) {
        ui.add_space(12.0);
        let response = ui
            .horizontal(|ui| {
                ui.add(
                    egui::Image::new((self.icons[i].id(), vec2(18.0, 18.0)))
                        .tint(ui.visuals().weak_text_color()),
                );
                ui.label(analytics::heading(ui, title, 18.0));
            })
            .response;
        if self.jump && self.section == i {
            response.scroll_to_me(Some(egui::Align::TOP));
            self.jump = false;
        }
    }

    fn content(
        &mut self,
        ui: &mut Ui,
        owner: Option<HWND>,
        inventory: Option<&[DashboardInventoryDevice]>,
    ) {
        let Some(mut draft) = self.draft.take() else {
            return;
        };
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(analytics::heading(ui, "Settings", 28.0));
        ui.label(RichText::new("Customize battery alerts, appearance and how Volty runs.").weak());
        if let Some(error) = &draft.preferences.config_error {
            error_label(ui, error);
        }
        ui.add_enabled_ui(
            !self.saving && draft.preferences.config_error.is_none(),
            |ui| {
                self.heading(ui, 0, SECTIONS[0]);
                self.alerts(ui, &mut draft);
                self.heading(ui, 1, SECTIONS[1]);
                appearance(
                    ui,
                    &mut draft.preferences,
                    inventory.unwrap_or(&self.inventory),
                );
                self.heading(ui, 2, "Battery color map");
                self.color_editor(ui, &mut draft, owner);
                self.heading(ui, 3, SECTIONS[3]);
                group(ui, |ui| {
                    row(
                        ui,
                        "Start with Windows",
                        "Automatically monitor batteries after you sign in.",
                        |ui| {
                            if let Some(enabled) = &mut draft.preferences.start_on_boot {
                                toggle(ui, enabled, "Start with Windows");
                            } else {
                                ui.label("Unavailable");
                            }
                        },
                    );
                    if let Some(error) = &draft.preferences.startup_error {
                        error_label(ui, error);
                    }
                });
            },
        );
        self.draft = Some(draft);
    }

    fn alerts(&mut self, ui: &mut Ui, draft: &mut Draft) {
        group(ui, |ui| {
            row(
                ui,
                "Low-battery notifications",
                "Notify me when a monitored battery reaches an alert threshold.",
                |ui| {
                    toggle(
                        ui,
                        &mut draft.preferences.notifications_enabled,
                        "Low-battery notifications",
                    );
                },
            );
            ui.separator();
            row(
                ui,
                "Alert thresholds",
                "Battery percentages that trigger a notification.",
                |ui| {
                    field(ui, &mut draft.thresholds, "Alert thresholds", "%", 166.0);
                },
            );
            if let Err(message) = draft.thresholds() {
                error_label(ui, message);
            }
            ui.separator();
            row(
                ui,
                "Repeat interval",
                "Time between alerts. Set to 0 to turn repeats off.",
                |ui| {
                    field(ui, &mut draft.repeat, "Repeat interval", "min", 86.0);
                },
            );
            if let Err(message) = validation::validate_cooldown(&draft.repeat) {
                error_label(ui, message);
            }
            ui.separator();
            row(
                ui,
                "Test notification",
                "Send a sample low-battery alert at 15%.",
                |ui| {
                    if ui
                        .add_enabled(!self.testing, egui::Button::new("Send test"))
                        .clicked()
                    {
                        self.testing = self.send(Action::Test);
                    }
                },
            );
        });
    }

    fn color_editor(&mut self, ui: &mut Ui, draft: &mut Draft, owner: Option<HWND>) {
        card(ui, |ui| {
            row(
                ui,
                "Color blend points",
                "Customize how battery charge maps to color.",
                |ui| {
                    if ui.button("Restore defaults").clicked() {
                        let colors = BatteryColorMap::default();
                        draft.red_at = colors.red_max_percent.to_string();
                        draft.yellow_at = colors.yellow_max_percent.to_string();
                        draft.preferences.battery_color_red = validation::hex_color(colors.red);
                        draft.preferences.battery_color_yellow =
                            validation::hex_color(colors.yellow);
                        draft.preferences.battery_color_green = validation::hex_color(colors.green);
                    }
                },
            );
            let p = &mut draft.preferences;
            let editors = [
                (
                    "Low battery · Red",
                    &mut p.battery_color_red,
                    Some(("Red through", &mut draft.red_at)),
                ),
                (
                    "Medium · Yellow",
                    &mut p.battery_color_yellow,
                    Some(("Yellow at", &mut draft.yellow_at)),
                ),
                ("Full battery · Green", &mut p.battery_color_green, None),
            ];
            let mut edit = |ui: &mut Ui,
                            (label, value, threshold): (
                _,
                &mut String,
                Option<(_, &mut String)>,
            )| {
                ui.label(RichText::new(label).size(12.0).weak());
                ui.horizontal(|ui| {
                    let rgb = validation::parse_hex_color(value);
                    let fill = rgb
                        .map(|[r, g, b]| Color32::from_rgb(r, g, b))
                        .unwrap_or(ui.visuals().extreme_bg_color);
                    let response = ui
                        .add_enabled(
                            owner.is_some(),
                            egui::Button::new(" ").fill(fill).min_size(vec2(34.0, 32.0)),
                        )
                        .on_hover_text(format!("Choose {label} color"));
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            ui.is_enabled() && owner.is_some(),
                            format!("Choose {label} color"),
                        )
                    });
                    if response.clicked() {
                        if let Some(result) = owner
                            .and_then(|window| choose_color(window, &mut self.custom_colors, rgb))
                        {
                            *value = validation::hex_color(result);
                        }
                    }
                    field(ui, value, label, "", ui.available_width());
                });
                if let Some((label, threshold)) = threshold {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(12.0).weak());
                        field(ui, threshold, label, "%", 58.0);
                    });
                } else {
                    ui.label(RichText::new("Green at 100%").size(12.0).weak());
                }
            };
            if ui.available_width() >= 540.0 {
                ui.columns(3, |cols| {
                    for (col, editor) in cols.iter_mut().zip(editors) {
                        edit(col, editor);
                    }
                });
            } else {
                for editor in editors {
                    edit(ui, editor);
                }
            }
            match draft.colors() {
                Ok(colors) => self.colors = colors,
                Err(error) => error_label(ui, error),
            }
            ui.add_space(10.0);
            gradient(ui, self.colors);
            ui.separator();
            ui.label(RichText::new("Live icon preview").strong());
            ui.label(
                RichText::new("Drag to simulate a battery level without changing your settings.")
                    .weak(),
            );
            self.preview(ui, &draft.preferences);
        });
    }

    fn preview(&mut self, ui: &mut Ui, p: &DashboardSettings) {
        let style = TrayIconStyle::parse(&p.tray_icon_style).unwrap_or(TrayIconStyle::Battery);
        let theme = TrayIconTheme::parse(&p.tray_icon_theme).unwrap_or(TrayIconTheme::System);
        let dark = match theme {
            TrayIconTheme::Dark => true,
            TrayIconTheme::Light => false,
            _ => self.taskbar_dark,
        };
        let key = (
            self.colors,
            style,
            theme,
            p.show_percentage_in_battery_icon,
            self.level,
            ui.pixels_per_point().to_bits(),
        );
        let wide = ui.available_width() > 540.0;
        let render = |this: &mut Self, ui: &mut Ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 76.0).max(100.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Slider::new(&mut this.level, 0..=100)
                        .show_value(false)
                        .trailing_fill(true),
                )
                .on_hover_text("Preview battery level");
                ui.label(
                    RichText::new(format!("{}%", this.level))
                        .size(23.0)
                        .strong(),
                );
            });
            if let Some(icon) = this.previews.first() {
                ui.horizontal(|ui| {
                    ui.image((icon.id(), vec2(56.0, 56.0)));
                    ui.label(
                        RichText::new(format!(
                            "{} · enlarged preview",
                            validation::hex_color(this.colors.color_for(this.level))
                        ))
                        .weak(),
                    );
                });
            }
        };
        let taskbars = |this: &Self, ui: &mut Ui| {
            ui.label(RichText::new("Taskbar appearance").size(12.0).weak());
            for (index, light) in [false, true].into_iter().enumerate() {
                let (bg, fg, label) = if light {
                    (
                        Color32::from_rgb(227, 231, 237),
                        Color32::from_gray(30),
                        "Light taskbar",
                    )
                } else {
                    (
                        Color32::from_rgb(29, 33, 40),
                        Color32::WHITE,
                        "Dark taskbar",
                    )
                };
                Frame::new()
                    .fill(bg)
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(label).size(12.0).color(fg));
                            if let Some(icon) = this.previews.get(index + 1) {
                                ui.image((icon.id(), vec2(23.0, 23.0)));
                            }
                        });
                    });
            }
        };
        if self.icon_key != Some(key) {
            self.icon_key = Some(key);
            let tray_size = if ui.pixels_per_point() > 1.0 { 32 } else { 16 };
            self.previews = [(32, dark), (tray_size, true), (tray_size, false)]
                .into_iter()
                .filter_map(|(size, sample_dark)| {
                    let theme = match theme {
                        TrayIconTheme::System if sample_dark => TrayIconTheme::Dark,
                        TrayIconTheme::System => TrayIconTheme::Light,
                        explicit => explicit,
                    };
                    let rgba = crate::tray::render_settings_preview(
                        self.level,
                        size,
                        self.colors,
                        theme,
                        style,
                        p.show_percentage_in_battery_icon,
                    )
                    .ok()?;
                    Some(ui.ctx().load_texture(
                        "tray-settings-preview",
                        egui::ColorImage::from_rgba_unmultiplied([size as usize; 2], &rgba),
                        egui::TextureOptions::LINEAR,
                    ))
                })
                .collect();
        }
        let level = self.level;
        if wide {
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width() - 178.0, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| render(self, ui),
                );
                ui.vertical(|ui| taskbars(self, ui));
            });
        } else {
            render(self, ui);
            ui.horizontal_wrapped(|ui| taskbars(self, ui));
        }
        if self.level != level {
            ui.ctx().request_repaint();
        }
    }
}

fn appearance(ui: &mut Ui, p: &mut DashboardSettings, inventory: &[DashboardInventoryDevice]) {
    group(ui, |ui| {
        row(
            ui,
            "App theme",
            "Choose the appearance of the Volty window.",
            |ui| {
                choices(
                    ui,
                    "App theme",
                    &mut p.app_theme,
                    &[
                        ("system", "Match Windows"),
                        ("light", "Light"),
                        ("dark", "Dark"),
                    ],
                );
            },
        );
        ui.separator();
        row(
            ui,
            "Icon contents",
            "Choose what is shown in the notification area.",
            |ui| {
                choices(
                    ui,
                    "Icon contents",
                    &mut p.tray_icon_style,
                    &[
                        ("battery", "Battery level"),
                        ("percentage", "Percentage only"),
                    ],
                );
            },
        );
        ui.separator();
        row(
            ui,
            "Tray icon theme",
            "Choose an icon treatment that suits your taskbar.",
            |ui| {
                choices(
                    ui,
                    "Tray icon theme",
                    &mut p.tray_icon_theme,
                    &[
                        ("system", "Match Windows"),
                        ("light", "Light taskbar"),
                        ("dark", "Dark taskbar"),
                    ],
                );
            },
        );
        ui.separator();
        row(
            ui,
            "Percentage inside battery icon",
            "Show the numeric charge alongside the battery shape.",
            |ui| {
                ui.add_enabled_ui(p.tray_icon_style == "battery", |ui| {
                    toggle(
                        ui,
                        &mut p.show_percentage_in_battery_icon,
                        "Percentage inside battery icon",
                    )
                });
            },
        );
        ui.separator();
        row(
            ui,
            "Tray battery",
            "Choose which monitored battery supplies the tray reading.",
            |ui| {
                let label = p
                    .pinned_device
                    .as_ref()
                    .map(|id| {
                        inventory
                            .iter()
                            .find(|item| &item.identity == id)
                            .map_or("Pinned device unavailable", |item| item.name.as_str())
                    })
                    .unwrap_or("Lowest monitored battery");
                egui::ComboBox::from_id_salt("Tray battery")
                    .width(200.0)
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.pinned_device, None, "Lowest monitored battery");
                        for device in inventory
                            .iter()
                            .filter(|item| item.monitored != Some(false))
                        {
                            ui.selectable_value(
                                &mut p.pinned_device,
                                Some(device.identity.clone()),
                                &device.name,
                            );
                        }
                    });
            },
        );
    });
}

fn choices(ui: &mut Ui, id: &str, value: &mut String, options: &[(&str, &str)]) {
    let label = options
        .iter()
        .find(|(key, _)| key == value)
        .map_or(value.as_str(), |(_, label)| label);
    egui::ComboBox::from_id_salt(id)
        .width(200.0)
        .selected_text(label)
        .show_ui(ui, |ui| {
            for &(key, label) in options {
                ui.selectable_value(value, key.into(), label);
            }
        });
}

fn group(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(ui.visuals().window_stroke)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            content(ui);
        });
}

fn row(ui: &mut Ui, name: &str, description: &str, control: impl FnOnce(&mut Ui)) {
    Frame::new()
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            responsive_row(ui, name, description, 244.0, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.horizontal(control);
                });
            })
        });
}

pub(super) fn responsive_row(
    ui: &mut Ui,
    name: &str,
    description: &str,
    control_width: f32,
    control: impl FnOnce(&mut Ui),
) {
    let label = |ui: &mut Ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.label(RichText::new(name).strong());
        ui.label(RichText::new(description).size(13.0).weak());
    };
    let label_width = ui.available_width() - control_width - ui.spacing().item_spacing.x;
    if label_width < 170.0 {
        ui.scope(label);
        ui.add_space(5.0);
        control(ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                vec2(label_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(label_width);
                    label(ui);
                },
            );
            ui.allocate_ui_with_layout(
                vec2(control_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                control,
            );
        });
    }
}

pub(super) fn field(ui: &mut Ui, value: &mut String, name: &str, suffix: &str, width: f32) {
    ui.horizontal(|ui| {
        let rtl = ui.layout().prefer_right_to_left();
        if rtl && !suffix.is_empty() {
            ui.label(RichText::new(suffix).size(12.0).weak());
        }
        let response = ui.add_sized(
            vec2(width, 34.0),
            egui::TextEdit::singleline(value)
                .id_salt(name)
                .hint_text(name),
        );
        if !rtl && !suffix.is_empty() {
            ui.label(RichText::new(suffix).size(12.0).weak());
        }
        response.on_hover_text(name);
    });
}

pub(super) fn toggle(ui: &mut Ui, enabled: &mut bool, name: &str) {
    let (rect, response) = ui.allocate_exact_size(vec2(67.0, 32.0), egui::Sense::click());
    if response.clicked() {
        *enabled = !*enabled;
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *enabled, name)
    });
    let track = egui::Rect::from_min_size(rect.left_center() - vec2(0.0, 10.0), vec2(40.0, 20.0))
        .shrink(0.5);
    let visual = ui.style().interact(&response);
    let fill = if *enabled { accent(ui) } else { visual.bg_fill };
    ui.painter().rect(
        track,
        10.0,
        fill,
        visual.fg_stroke,
        egui::StrokeKind::Inside,
    );
    let thumb = track.center() + vec2(if *enabled { 10.0 } else { -10.0 }, 0.0);
    ui.painter().circle_filled(
        thumb,
        6.5,
        if *enabled {
            ui.visuals().selection.stroke.color
        } else {
            visual.fg_stroke.color
        },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            track.expand(3.0),
            12.0,
            egui::Stroke::new(2.0, accent(ui)),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter().text(
        rect.left_center() + vec2(48.0, 0.0),
        egui::Align2::LEFT_CENTER,
        if *enabled { "On" } else { "Off" },
        egui::FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
}

fn gradient(ui: &mut Ui, colors: BatteryColorMap) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), egui::Sense::hover());
    let x = |percent| egui::lerp(rect.x_range(), f32::from(percent) / 100.0);
    for percent in 0..100u8 {
        let [r, g, b] = colors.color_for(percent);
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x(percent), rect.top() + 2.0),
                egui::pos2(x(percent + 1) + 0.3, rect.top() + 16.0),
            ),
            0.0,
            Color32::from_rgb(r, g, b),
        );
    }
    let font = egui::FontId::proportional(11.0);
    let color = ui.visuals().weak_text_color();
    let mut last = 0.0_f32;
    for (percent, align) in [
        (0, egui::Align2::LEFT_TOP),
        (colors.red_max_percent, egui::Align2::CENTER_TOP),
        (colors.yellow_max_percent, egui::Align2::CENTER_TOP),
        (100, egui::Align2::RIGHT_TOP),
    ] {
        let position = x(percent);
        if percent != 0 && percent != 100 {
            ui.painter().vline(
                position,
                rect.top()..=rect.top() + 18.0,
                egui::Stroke::new(1.0, ui.visuals().text_color()),
            );
            if position < last + 37.0 || position > rect.right() - 42.0 {
                continue;
            }
        }
        ui.painter().text(
            egui::pos2(position, rect.top() + 20.0),
            align,
            format!("{percent}%"),
            font.clone(),
            color,
        );
        last = position;
    }
}

fn error_label(ui: &mut Ui, error: &str) {
    ui.colored_label(ui.visuals().error_fg_color, error);
}

fn choose_color(
    owner: HWND,
    custom_colors: &mut [u32; 16],
    initial: Option<[u8; 3]>,
) -> Option<[u8; 3]> {
    let [r, g, b] = initial.unwrap_or([0; 3]);
    let mut picker = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: u32::from_le_bytes([r, g, b, 0]),
        lpCustColors: custom_colors.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    // SAFETY: the HWND is the live parent, and the dialog and custom colors remain valid.
    (unsafe { ChooseColorW(&mut picker) } != 0)
        .then(|| picker.rgbResult.to_le_bytes()[..3].try_into().unwrap())
}
