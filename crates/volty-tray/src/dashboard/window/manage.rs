use eframe::egui::{
    self, vec2, Align, Button, ComboBox, Frame, Label, Layout, RichText, ScrollArea, Sense, Ui,
};

use super::{
    accent,
    analytics::{battery_bar, cell_ui, device_icon, heading, table_row},
    assets::Icon,
    card, day_picker, live_device, live_percent, platform, settings, Action, CleanupKind,
    Dashboard, DashboardData, GAP,
};
use crate::dashboard::model::DashboardInventoryDevice;

#[derive(Default)]
pub(super) struct ManageView {
    search: String,
    filter: Filter,
    sort: usize,
    reverse: bool,
    focus_row: Option<String>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Monitored,
    Paused,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All devices",
            Self::Monitored => "Monitored",
            Self::Paused => "Paused",
        }
    }
}

impl Dashboard {
    pub(super) fn manage(&mut self, ui: &mut Ui) {
        let snapshot = self.snapshot.clone();
        ScrollArea::vertical()
            .id_salt("manage-data")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let available = ui.available_width();
                let width = (available - 40.0).min(1240.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space((available - width) / 2.0);
                    ui.allocate_ui_with_layout(
                        vec2(width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(width);
                            ui.spacing_mut().item_spacing = egui::Vec2::splat(GAP as f32);
                            ui.add_space(12.0);
                            section(
                                ui,
                                "Manage data",
                                "Control which devices Volty monitors and what history is kept.",
                                27.0,
                            );
                            let Some(snapshot) = snapshot else {
                                if self.loading {
                                    ui.spinner();
                                    ui.label("Loading device data…");
                                } else {
                                    ui.label("Dashboard data unavailable.");
                                }
                                return;
                            };
                            let data = &snapshot.data;
                            if !data.management_allowed {
                                ui.label("Read-only mode for the selected database.");
                            }
                            ui.add_space(8.0);
                            self.overview(ui, data);
                            ui.add_space(8.0);
                            let enabled = !self.busy
                                && self.confirmation.is_none()
                                && !self.management_refresh
                                && data.management_allowed;
                            let wide = width >= 860.0;
                            let mut action = None;
                            pair(ui, wide, |ui, right| {
                                if right {
                                    action = self.device_details(ui, data, enabled);
                                } else {
                                    self.device_table(ui, data);
                                }
                            });
                            ui.add_space(8.0);
                            pair(ui, wide, |ui, right| {
                                let next = if right {
                                    self.diagnostics(ui, enabled)
                                } else {
                                    self.cleanup(ui, enabled)
                                };
                                action = next.or(action.take());
                            });
                            ui.add_space(20.0);
                            if let Some(action) = action {
                                self.action_focus = ui.memory(|memory| memory.focused());
                                self.request(action);
                            }
                        },
                    );
                });
            });
    }

    fn device_table(&mut self, ui: &mut Ui, data: &DashboardData) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    section(
                        ui,
                        "Devices",
                        "Select a device to manage its history.",
                        16.0,
                    )
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    settings::field(
                        ui,
                        &mut self.manage_view.search,
                        "Search devices…",
                        "",
                        160.0,
                    );
                });
            });
            ComboBox::from_id_salt("management-filter")
                .selected_text(self.manage_view.filter.label())
                .show_ui(ui, |ui| {
                    for filter in [Filter::All, Filter::Monitored, Filter::Paused] {
                        ui.selectable_value(&mut self.manage_view.filter, filter, filter.label());
                    }
                });
            let query = self.manage_view.search.trim().to_lowercase();
            let mut rows: Vec<_> = data
                .inventory
                .iter()
                .filter(|item| {
                    let matches_filter = match self.manage_view.filter {
                        Filter::All => true,
                        Filter::Monitored => self.monitored(item),
                        Filter::Paused => !self.monitored(item),
                    };
                    matches_filter
                        && (item.name.to_lowercase().contains(&query)
                            || item.identity.to_lowercase().contains(&query))
                })
                .collect();
            rows.sort_by(|a, b| {
                let name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
                let order = match self.manage_view.sort {
                    1 => self.monitored(b).cmp(&self.monitored(a)),
                    2 => self.recording(b).cmp(&self.recording(a)),
                    3 => b.samples.cmp(&a.samples),
                    _ => name(),
                }
                .then_with(name)
                .then_with(|| a.identity.cmp(&b.identity));
                if self.manage_view.reverse {
                    order.reverse()
                } else {
                    order
                }
            });

            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, 0, ui.visuals().widgets.inactive.weak_bg_fill);
                for (index, (cell, title)) in cells(rect)
                    .into_iter()
                    .zip(["Device", "Monitoring", "History", "Readings"])
                    .enumerate()
                {
                    let label = if self.manage_view.sort == index {
                        let ascending = (index == 0) != self.manage_view.reverse;
                        format!("{title} {}", if ascending { "↑" } else { "↓" })
                    } else {
                        title.to_owned()
                    };
                    let mut ui = cell_ui(ui, cell);
                    ui.spacing_mut().button_padding.x = 0.0;
                    if ui
                        .add(
                            Button::new(RichText::new(label).size(12.0).weak())
                                .frame(false)
                                .truncate(),
                        )
                        .clicked()
                    {
                        if self.manage_view.sort == index {
                            self.manage_view.reverse = !self.manage_view.reverse;
                        } else {
                            self.manage_view.sort = index;
                            self.manage_view.reverse = false;
                        }
                    }
                }
                if rows.is_empty() {
                    ui.add_space(12.0);
                    ui.label(if data.inventory.is_empty() {
                        "No registered devices"
                    } else {
                        "No matching devices"
                    });
                    ui.add_space(12.0);
                }
                ScrollArea::vertical()
                    .id_salt("managed-devices")
                    .max_height(270.0)
                    .min_scrolled_height(270.0)
                    .show_rows(ui, 54.0, rows.len(), |ui, range| {
                        let row_id =
                            |ui: &Ui, index: usize| ui.scope_id().with(&rows[index].identity);
                        if let Some(index) = (range.clone())
                            .find(|&index| ui.memory(|memory| memory.has_focus(row_id(ui, index))))
                        {
                            ui.memory_mut(|memory| {
                                memory.set_focus_lock_filter(
                                    row_id(ui, index),
                                    egui::EventFilter {
                                        vertical_arrows: true,
                                        ..Default::default()
                                    },
                                )
                            });
                            let step = ui.input(|input| {
                                isize::from(input.key_pressed(egui::Key::ArrowDown))
                                    - isize::from(input.key_pressed(egui::Key::ArrowUp))
                            });
                            let next = index.saturating_add_signed(step).min(rows.len() - 1);
                            if next != index {
                                self.selected = Some(rows[next].identity.clone());
                                let y =
                                    ui.cursor().top() + (next as f32 - range.start as f32) * 54.0;
                                let target = egui::Rect::from_min_size(
                                    egui::pos2(ui.cursor().left(), y),
                                    vec2(1.0, 54.0),
                                );
                                ui.scroll_to_rect(target, None);
                                self.manage_view.focus_row = Some(rows[next].identity.clone());
                            }
                        }
                        for item in &rows[range] {
                            self.device_row(ui, data, item);
                        }
                    });
            });
            ui.label(
                RichText::new(format!(
                    "{} of {} registered devices",
                    rows.len(),
                    data.inventory.len()
                ))
                .size(12.0)
                .weak(),
            );
        });
    }

    fn device_row(&mut self, ui: &mut Ui, data: &DashboardData, item: &DashboardInventoryDevice) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::hover());
        let selected = self.selected.as_deref() == Some(&item.identity);
        let response = ui.interact(rect, ui.scope_id().with(&item.identity), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, &item.name)
        });
        if self.manage_view.focus_row.as_deref() == Some(&item.identity) {
            response.request_focus();
            self.manage_view.focus_row = None;
        }
        if response.clicked() {
            self.selected = Some(item.identity.clone());
        }
        table_row(ui, rect, selected || response.hovered());
        if selected {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                0,
                accent(ui),
            );
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let [device, monitoring, history, count] = cells(rect);
        let mut cell = cell_ui(ui, device);
        let color = if self.contrast.is_some() {
            ui.visuals().text_color()
        } else {
            self.analytics.series_color(ui, &item.identity)
        };
        device_icon(&item.identity, &item.name)
            .unwrap_or(Icon::Battery)
            .show(&mut cell, 22.0, color);
        let (status, _) = self.connection(data, item);
        let width = cell.available_width();
        cell.allocate_ui_with_layout(vec2(width, 32.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.add(Label::new(heading(ui, &item.name, 13.0)).truncate());
            ui.add(
                Label::new(
                    RichText::new(format!("{status} · {}", last_reading(item)))
                        .size(11.0)
                        .weak(),
                )
                .truncate(),
            );
        });
        let monitored = self.monitored(item);
        cell_ui(ui, monitoring).label(if monitored { "Enabled" } else { "Paused" });
        cell_ui(ui, history).label(self.recording_status(item));
        cell_ui(ui, count).label(grouped(item.samples));
        response.on_hover_text(format!(
            "{}\n{status} · Last reading {}",
            item.name,
            last_reading(item)
        ));
    }

    fn connection(
        &self,
        data: &DashboardData,
        item: &DashboardInventoryDevice,
    ) -> (&'static str, Option<super::LiveReading>) {
        if let Some(device) = live_device(self.live.as_deref(), &item.identity) {
            let status = match device.reading {
                Some(reading) if reading.charging => "Charging",
                Some(_) => "Online",
                None => "Detected",
            };
            return (status, device.reading);
        }
        let online = self.live.is_none()
            && item.last_sample_at.is_some_and(|at| {
                data.generated_at.saturating_sub(at)
                    <= crate::dashboard::model::OFFLINE_AFTER_SECONDS
            });
        (if online { "Recently seen" } else { "Offline" }, None)
    }

    fn device_details(&self, ui: &mut Ui, data: &DashboardData, enabled: bool) -> Option<Action> {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.label(heading(ui, "Device details", 16.0));
            ui.separator();
            let Some(item) = super::selected_inventory(data, self.selected.as_deref()) else {
                ui.label("Select a registered device to see its settings.");
                return None;
            };
            let (status, reading) = self.connection(data, item);
            let color = if self.contrast.is_some() {
                ui.visuals().text_color()
            } else {
                self.analytics.series_color(ui, &item.identity)
            };
            ui.horizontal(|ui| {
                Frame::new()
                    .fill(ui.visuals().widgets.inactive.bg_fill)
                    .corner_radius(7)
                    .inner_margin(8)
                    .show(ui, |ui| {
                        device_icon(&item.identity, &item.name)
                            .unwrap_or(Icon::Battery)
                            .show(ui, 27.0, color)
                    });
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(heading(ui, &item.name, 18.0));
                    ui.horizontal(|ui| {
                        if status == "Charging" {
                            Icon::Bolt.show(ui, 12.0, super::analytics::charging_color(ui));
                        }
                        let label = if reading.is_some() {
                            format!("{status} · {}", live_percent(reading))
                        } else {
                            status.to_owned()
                        };
                        ui.label(RichText::new(label).size(12.0).weak());
                    });
                });
            });
            if let Some(level) = reading.and_then(|reading| reading.level.value()) {
                battery_bar(ui, ui.available_width(), Some(level), color);
            }
            ui.columns(2, |columns| {
                metric(
                    &mut columns[0],
                    "Stored readings",
                    grouped(item.samples),
                    16.0,
                );
                metric(&mut columns[1], "Last reading", last_reading(item), 16.0);
            });
            ui.separator();
            let mut action = None;
            ui.add_enabled_ui(enabled, |ui| {
                let mut monitored = self.monitored(item);
                settings::responsive_row(
                    ui,
                    "Monitor device",
                    "Track live battery state and show this device in the tray.",
                    70.0,
                    |ui| {
                        settings::toggle(ui, &mut monitored, "Monitor device");
                    },
                );
                if monitored != self.monitored(item) {
                    action = Some(Action::Monitoring {
                        identity: item.identity.clone(),
                        enabled: monitored,
                    });
                }
                ui.separator();
                let mut recording = self.recording(item);
                settings::responsive_row(
                    ui,
                    "Record battery history",
                    "Store new readings while monitoring is enabled.",
                    70.0,
                    |ui| {
                        settings::toggle(ui, &mut recording, "Record battery history");
                    },
                );
                if recording != self.recording(item) {
                    action = Some(Action::Recording {
                        identity: item.identity.clone(),
                        enabled: recording,
                    });
                }
            });
            ui.separator();
            ui.label(RichText::new("Destructive actions").size(12.0).weak());
            ui.horizontal_wrapped(|ui| {
                for (label, preferences) in [("Delete history…", false), ("Forget device…", true)]
                {
                    if ui
                        .add_enabled(
                            enabled && (preferences || item.samples > 0),
                            danger(ui, label),
                        )
                        .clicked()
                    {
                        action = Some(Action::PreviewDevice {
                            identity: item.identity.clone(),
                            name: item.name.clone(),
                            preferences,
                        });
                    }
                }
            });
            ui.label(RichText::new("Delete history removes stored readings. Forget device also resets saved preferences.").size(12.0).weak());
            action
        })
    }

    fn cleanup(&mut self, ui: &mut Ui, enabled: bool) -> Option<Action> {
        card(ui, |ui| {
            section(
                ui,
                "History cleanup",
                "Review the affected data before deleting anything.",
                16.0,
            );
            let mut action = None;
            for (label, description, days, kind, choices) in [
                (
                    "Old readings",
                    "Delete readings older than the selected period.",
                    &mut self.older_days,
                    CleanupKind::OlderThan,
                    &[1, 3, 7, 14, 21, 30][..],
                ),
                (
                    "Inactive devices",
                    "Delete history for devices with no recent readings.",
                    &mut self.inactive_days,
                    CleanupKind::InactiveDevices,
                    &[7, 14, 21][..],
                ),
            ] {
                ui.separator();
                ui.add_enabled_ui(enabled, |ui| {
                    settings::responsive_row(ui, label, description, 244.0, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            day_picker(ui, label, days, choices);
                            if ui.button("Preview cleanup").clicked() {
                                action = Some(Action::PreviewCleanup(kind, *days));
                            }
                        });
                    });
                });
            }
            action
        })
    }

    fn diagnostics(&self, ui: &mut Ui, enabled: bool) -> Option<Action> {
        card(ui, |ui| {
            section(
                ui,
                "Diagnostic files",
                "Locally stored logs and captures.",
                16.0,
            );
            ui.separator();
            let mut action = None;
            match &self.artifacts {
                Some(Ok(files)) if files.is_empty() => {
                    ui.horizontal(|ui| {
                        Icon::File.show(ui, 24.0, ui.visuals().weak_text_color());
                        ui.vertical(|ui| {
                            section(
                                ui,
                                "No managed diagnostic files",
                                "Logs and captures will appear here when available.",
                                14.0,
                            )
                        });
                    });
                }
                Some(Ok(files)) => {
                    if ui.add_enabled(enabled, danger(ui, "Delete all…")).clicked() {
                        action = Some(Action::ConfirmArtifacts(files.clone()));
                    }
                    ScrollArea::vertical()
                        .id_salt("artifacts")
                        .max_height(192.0)
                        .min_scrolled_height(192.0)
                        .show_rows(ui, 48.0, files.len(), |ui, range| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for file in &files[range] {
                                let (rect, _) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 48.0),
                                    Sense::hover(),
                                );
                                table_row(ui, rect, false);
                                let mut name = rect;
                                name.max.x -= 95.0;
                                let mut button = rect;
                                button.min.x = name.max.x + 8.0;
                                let mut cell = cell_ui(ui, name);
                                Icon::File.show(&mut cell, 18.0, ui.visuals().weak_text_color());
                                cell.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.add(Label::new(heading(ui, &file.name, 13.0)).truncate())
                                        .on_hover_text(&file.name);
                                    let date = file
                                        .created_at
                                        .and_then(|at| i64::try_from(at).ok())
                                        .map(platform::summary_date);
                                    let metadata =
                                        format!("{} · {} bytes", file.kind, grouped(file.bytes));
                                    ui.add(
                                        Label::new(RichText::new(&metadata).size(11.0).weak())
                                            .truncate(),
                                    )
                                    .on_hover_text(date.unwrap_or(metadata));
                                });
                                if cell_ui(ui, button)
                                    .add_enabled(enabled, danger(ui, "Delete…"))
                                    .clicked()
                                {
                                    action = Some(Action::ConfirmArtifacts(vec![file.clone()]));
                                }
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
            action
        })
    }

    fn monitored(&self, item: &DashboardInventoryDevice) -> bool {
        match &self.pending_preference {
            Some((identity, true, enabled)) if *identity == item.identity => *enabled,
            _ => item.monitored.unwrap_or(true),
        }
    }

    fn recording(&self, item: &DashboardInventoryDevice) -> bool {
        match &self.pending_preference {
            Some((identity, false, enabled)) if *identity == item.identity => *enabled,
            _ => item.history_recording_enabled,
        }
    }

    fn recording_status(&self, item: &DashboardInventoryDevice) -> &'static str {
        match (self.recording(item), self.monitored(item)) {
            (false, _) => "Off",
            (true, false) => "Paused",
            (true, true) => "Enabled",
        }
    }
}

fn pair(ui: &mut Ui, wide: bool, mut show: impl FnMut(&mut Ui, bool)) {
    if wide {
        let width = ui.available_width() - GAP as f32;
        ui.horizontal_top(|ui| {
            for (right, fraction) in [(false, 0.59), (true, 0.41)] {
                let width = width * fraction;
                ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(width);
                    show(ui, right);
                });
            }
        });
    } else {
        show(ui, false);
        ui.add_space(8.0);
        show(ui, true);
    }
}

impl Dashboard {
    fn overview(&self, ui: &mut Ui, data: &DashboardData) {
        card(ui, |ui| {
            ui.columns(3, |columns| {
                let monitored = data
                    .inventory
                    .iter()
                    .filter(|item| self.monitored(item))
                    .count();
                let readings = data.inventory.iter().map(|item| item.samples).sum();
                for (column, (label, value)) in columns.iter_mut().zip([
                    ("Registered devices", data.inventory.len().to_string()),
                    (
                        "Monitoring enabled",
                        format!("{monitored} of {}", data.inventory.len()),
                    ),
                    ("Stored readings", grouped(readings)),
                ]) {
                    metric(column, label, value, 23.0);
                }
            });
        });
    }
}

fn metric(ui: &mut Ui, label: &str, value: String, size: f32) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.label(RichText::new(label).size(12.0).weak());
        ui.label(heading(ui, value, size));
    });
}

fn section(ui: &mut Ui, title: &str, subtitle: &str, size: f32) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.label(heading(ui, title, size));
        ui.label(RichText::new(subtitle).size(13.0).weak());
    });
}

fn cells(row: egui::Rect) -> [egui::Rect; 4] {
    let edges = [0.0, 0.46, 0.67, 0.83, 1.0];
    std::array::from_fn(|i| {
        egui::Rect::from_min_max(
            egui::pos2(row.left() + row.width() * edges[i] + 7.0, row.top()),
            egui::pos2(row.left() + row.width() * edges[i + 1] - 3.0, row.bottom()),
        )
    })
}

fn last_reading(item: &DashboardInventoryDevice) -> String {
    item.last_sample_at
        .map_or_else(|| "—".to_owned(), platform::summary_date)
}

pub(super) fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

fn danger<'a>(ui: &Ui, label: &'a str) -> Button<'a> {
    Button::new(RichText::new(label).color(ui.visuals().error_fg_color))
}
