use std::{collections::BTreeSet, ops::Range, sync::Arc};

use eframe::egui::{
    self, pos2, vec2, Align, Button, Color32, ComboBox, Frame, Layout, Rect, RichText, ScrollArea,
    Sense, Stroke, Ui, Widget,
};

use super::{
    super::model::SECONDS_PER_DAY,
    accent, canvas, card,
    chart::{self, ActivityHit, ActivityState, Forecast, Plot, Series, TimeRange, FORECAST_NAMES},
    duration, live_device, live_percent, percent, platform, selected_inventory, Dashboard,
    Snapshot,
};

const ALL: u32 = u32::MAX;

pub(super) type Reading = (usize, usize);

struct SelectedReading {
    identity: String,
    at: i64,
}

impl SelectedReading {
    fn matches(&self, identity: &str, at: i64) -> bool {
        self.identity == identity && self.at == at
    }
}

pub(super) struct HistoryView {
    search: String,
    compared: BTreeSet<String>,
    forecast: [bool; 2],
    charging: bool,
    thresholds: bool,
    bridge_gaps: bool,
    expanded: bool,
    selecting_interval: bool,
    drag_start: Option<i64>,
    interval: Option<TimeRange>,
    readings_interval: Option<TimeRange>,
    jump_to_readings: bool,
    target: Option<u8>,
    zoom: Option<TimeRange>,
    pinned: Option<i64>,
    table_hover: Option<i64>,
    activity_hover: Option<ActivityHit>,
    pinned_activity: Option<ActivityHit>,
    oldest_first: bool,
    sort_column: usize,
    column_edges: [f32; 7],
    sorted_readings: Option<(Range<usize>, usize, Arc<Vec<Reading>>)>,
    selected_reading: Option<SelectedReading>,
    collapsed: bool,
    filter: ReadingsFilter,
    readings: Option<Arc<Vec<Reading>>>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self {
            search: String::new(),
            compared: BTreeSet::new(),
            forecast: [false; 2],
            charging: true,
            thresholds: true,
            bridge_gaps: false,
            expanded: false,
            selecting_interval: false,
            drag_start: None,
            interval: None,
            readings_interval: None,
            jump_to_readings: false,
            target: None,
            zoom: None,
            pinned: None,
            table_hover: None,
            activity_hover: None,
            pinned_activity: None,
            oldest_first: false,
            sort_column: 0,
            column_edges: [0.0, 0.25, 0.52, 0.63, 0.74, 0.86, 1.0],
            sorted_readings: None,
            selected_reading: None,
            collapsed: false,
            filter: ReadingsFilter::Visible,
            readings: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadingsFilter {
    Visible,
    Focused,
    Charging,
}

impl ReadingsFilter {
    fn label(self) -> &'static str {
        match self {
            Self::Visible => "Visible devices",
            Self::Focused => "Focused device",
            Self::Charging => "Charging only",
        }
    }
}

struct DeviceRow<'a> {
    identity: &'a str,
    name: &'a str,
    level: Option<u8>,
    level_label: String,
    index: Option<usize>,
}

pub(super) fn tab(active: bool, label: &'static str) -> impl Widget {
    move |ui: &mut Ui| {
        let response = ui.add(
            Button::new(RichText::new(label).color(if active {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            }))
            .frame(false),
        );
        if active {
            ui.painter().hline(
                response.rect.x_range(),
                response.rect.bottom() + 7.0,
                Stroke::new(2.0, accent(ui)),
            );
        }
        response
    }
}

impl Dashboard {
    pub(super) fn history(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        let snapshot = self.snapshot.clone();
        let mut rows = Vec::new();
        let data = snapshot.as_ref().map(|snapshot| &snapshot.data);
        for device in self.live.iter().flatten() {
            let index = data.and_then(|data| {
                data.devices
                    .iter()
                    .position(|item| item.identity == device.identity)
            });
            let live_level = device.reading.and_then(|reading| reading.level.value());
            let level =
                live_level.or_else(|| index.and_then(|index| data?.devices[index].latest_percent));
            rows.push(DeviceRow {
                identity: &device.identity,
                name: &device.name,
                index,
                level,
                level_label: if live_level.is_some() {
                    live_percent(device.reading)
                } else {
                    percent(level)
                },
            });
        }
        if let Some(data) = data {
            for (index, device) in data.devices.iter().enumerate() {
                if live_device(self.live.as_deref(), &device.identity).is_none() {
                    rows.push(DeviceRow {
                        identity: &device.identity,
                        name: &device.name,
                        index: Some(index),
                        level: device.latest_percent,
                        level_label: percent(device.latest_percent),
                    });
                }
            }
            for item in &data.inventory {
                if rows.iter().all(|row| row.identity != item.identity) {
                    rows.push(DeviceRow {
                        identity: &item.identity,
                        name: &item.name,
                        index: None,
                        level: None,
                        level_label: percent(None),
                    });
                }
            }
        }

        rows.sort_by_key(|row| (row.index.is_none(), row.identity));
        let history = &mut self.analytics;
        let focus = &mut self.selected;
        let mut days = self.days;
        let height = ui.available_height();
        let sidebar_width = if ui.available_width() < 900.0 {
            176.0
        } else {
            220.0
        };
        let export = ui
            .horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(
                    vec2(sidebar_width, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        Frame::new()
                            .fill(sidebar_fill(ui))
                            .inner_margin(egui::Margin::symmetric(16, 20))
                            .show(ui, |ui| {
                                ui.set_width(sidebar_width - 32.0);
                                ui.set_height((height - 40.0).max(0.0));
                                history.sidebar(ui, &rows, focus);
                            });
                    },
                );
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.separator();
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_salt("analytics")
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                let margin = if sidebar_width == 176.0 { 12 } else { 24 };
                                Frame::new()
                                    .inner_margin(egui::Margin::symmetric(margin, 20))
                                    .show(ui, |ui| {
                                        history.content(
                                            ui,
                                            snapshot.as_deref(),
                                            &rows,
                                            focus,
                                            &mut days,
                                            self.loading,
                                            !self.exporting,
                                        )
                                    })
                                    .inner
                            })
                            .inner
                    },
                )
                .inner
            })
            .inner;
        if days != self.days {
            history.reset_view();
            self.days = days;
            self.discard_load = self.loading;
            self.status = None;
            self.refresh();
        }
        if let (Some(export), Some(snapshot)) = (export, snapshot) {
            match platform::save_path(frame) {
                Ok(Some(path)) => match self.worker.export(path, snapshot, export) {
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
}

impl HistoryView {
    #[allow(clippy::too_many_arguments)]
    fn content(
        &mut self,
        ui: &mut Ui,
        snapshot: Option<&Snapshot>,
        rows: &[DeviceRow<'_>],
        focus: &mut Option<String>,
        days: &mut u32,
        loading: bool,
        can_export: bool,
    ) -> Option<Export> {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(8.0, 12.0);
        let focused = rows
            .iter()
            .position(|row| Some(row.identity) == focus.as_deref());
        let Some(snapshot) = snapshot else {
            ui.heading("Battery history");
            ui.label(if loading {
                "Loading battery history…"
            } else {
                "Battery history unavailable."
            });
            return None;
        };
        let focus_index = focused.and_then(|row| rows[row].index);
        let Some((reference_row, reference_index)) = focused.zip(focus_index).or_else(|| {
            rows.iter().enumerate().find_map(|(index, row)| {
                Some((index, row.index?)).filter(|_| self.visible(row.identity))
            })
        }) else {
            ui.horizontal_wrapped(|ui| {
                ui.heading(focused.map_or("Battery history", |index| rows[index].name));
                range_picker(ui, days);
            });
            if let Some(index) = focused {
                let row = &rows[index];
                ui.label(&row.level_label);
            }
            let paused = selected_inventory(&snapshot.data, focus.as_deref())
                .is_some_and(|item| !item.history_recording_enabled);
            ui.label(if paused {
                "History recording is paused for this device."
            } else {
                "No battery history recorded in this range."
            });
            return None;
        };
        let series = |index: usize, row: usize, ui: &Ui| Series {
            index,
            device: &snapshot.data.devices[index],
            chart: &snapshot.charts[index],
            color: series_color(ui, rows[row].identity),
        };
        let reference = series(reference_index, reference_row, ui);
        let focused_row = focused.unwrap_or(reference_row);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(heading(ui, rows[focused_row].name, 25.0)).truncate());
                if focus_index.is_some() {
                    ui.label(caption(format!(
                        "{} · Last reading {} · {} visible",
                        rows[focused_row].level_label,
                        platform::summary_date(reference.device.last_at),
                        rows.iter()
                            .filter(|row| row.index.is_some() && self.visible(row.identity))
                            .count()
                    )));
                } else {
                    ui.label(caption("No readings for the focused device in this range"));
                }
            });
        });
        if snapshot.data.window_days != *days {
            ui.horizontal(|ui| {
                if loading {
                    ui.spinner();
                }
                ui.label(caption(format!(
                    "Showing {} history",
                    range_label(snapshot.data.window_days)
                )));
            });
        }
        let mut visible = self.series(ui, snapshot, rows);
        let (range, hovered) = self.trend(
            ui,
            snapshot,
            rows,
            reference,
            focus_index.is_some(),
            &mut visible,
            days,
        );
        self.readings(
            ui,
            snapshot,
            &visible,
            focus_index,
            range,
            hovered,
            can_export,
        )
    }

    pub(super) fn initial_comparison(&mut self, data: &super::DashboardData, focus: Option<&str>) {
        self.compared.extend(focus.map(str::to_owned));
        if let Some(other) = data
            .devices
            .iter()
            .find(|device| Some(device.identity.as_str()) != focus)
        {
            self.compared.insert(other.identity.clone());
        }
    }
    pub(super) fn invalidate_readings(&mut self) {
        self.readings = None;
        self.sorted_readings = None;
    }

    fn reset_view(&mut self) {
        self.zoom = None;
        self.pinned = None;
        self.table_hover = None;
        self.activity_hover = None;
        self.pinned_activity = None;
        self.interval = None;
        self.readings_interval = None;
        self.drag_start = None;
        self.selected_reading = None;
    }

    fn series<'a>(
        &self,
        ui: &Ui,
        snapshot: &'a Snapshot,
        rows: &[DeviceRow<'_>],
    ) -> Vec<Series<'a>> {
        rows.iter()
            .filter(|row| self.visible(row.identity))
            .filter_map(|item| {
                let index = item.index?;
                Some(Series {
                    index,
                    device: &snapshot.data.devices[index],
                    chart: &snapshot.charts[index],
                    color: series_color(ui, item.identity),
                })
            })
            .collect()
    }

    fn visible(&self, id: &str) -> bool {
        self.compared.contains(id)
    }

    fn set_visible(&mut self, id: &str, show: bool) {
        if show {
            self.compared.insert(id.to_owned());
        } else {
            self.compared.remove(id);
        }
        self.invalidate_readings();
    }
    fn sidebar(&mut self, ui: &mut Ui, rows: &[DeviceRow<'_>], focus: &mut Option<String>) {
        ui.spacing_mut().item_spacing = vec2(8.0, 7.0);
        ui.horizontal(|ui| {
            ui.label(heading(ui, "Devices", 18.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                badge(
                    ui,
                    format!(
                        "{} shown",
                        rows.iter()
                            .filter(|row| row.index.is_some() && self.visible(row.identity))
                            .count()
                    ),
                );
            });
        });
        ui.label(caption("Choose devices to compare"));
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search devices")
                .margin(egui::Margin::symmetric(9, 9))
                .desired_width(ui.available_width()),
        );
        let query = self.search.trim().to_lowercase();
        ScrollArea::vertical().id_salt("analytics-devices").auto_shrink([false, true]).show(ui, |ui| {
            for row in rows.iter().filter(|row| {
                query.is_empty() || row.name.to_lowercase().contains(&query) || row.identity.to_lowercase().contains(&query)
            }) {
                let active = focus.as_deref() == Some(row.identity);
                let mut visible = self.visible(row.identity);
                let color = series_color(ui, row.identity);
                let fill = if active { sidebar_fill(ui).lerp_to_gamma(accent(ui), 0.14) } else { Color32::TRANSPARENT };
                let card = Frame::new().fill(fill).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 9)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let check = layer_toggle(ui, &mut visible, &format!("Show {} on the chart", row.name), None, None, row.index.is_some());
                        if check.on_hover_text(format!("Show {} on the chart", row.name)).changed() {
                            self.set_visible(row.identity, visible);
                        }
                        let label = ui.allocate_ui_with_layout(vec2(ui.available_width(), 46.0), Layout::top_down(Align::Min), |ui| {
                            let width = ui.available_width();
                            ui.set_width(width);
                            ui.set_max_width(width);
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.spacing_mut().interact_size.y = 15.0;
                            let name = ui.add(egui::Label::new(heading(ui, row.name, 14.0)).truncate().sense(Sense::click()));
                            ui.horizontal(|ui| {
                                battery_bar(ui, (width - 35.0).max(12.0), row.level, color);
                                ui.label(RichText::new(&row.level_label).color(color).size(12.0).strong());
                            });
                            ui.label(caption(source(row.identity)));
                            name
                        });
                        if label.inner.on_hover_text(row.name).clicked()
                            || label.response.interact(Sense::click()).clicked()
                        {
                            *focus = Some(row.identity.to_owned());
                            self.invalidate_readings();
                            self.reset_view();
                        }
                    });
                });
                if active {
                    let bar = Rect::from_min_size(card.response.rect.left_top() + vec2(0.0, 3.0), vec2(3.0, card.response.rect.height() - 6.0));
                    ui.painter().rect_filled(bar, 2, focus_marker(ui));
                }
            }
            ui.add_space(8.0);
            ui.separator();
            ui.label(caption("Select a device to focus its forecasts. Checkboxes control which devices appear in the chart."));
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn trend<'a>(
        &mut self,
        ui: &mut Ui,
        snapshot: &'a Snapshot,
        rows: &[DeviceRow<'_>],
        focused: Series<'a>,
        focus_has_readings: bool,
        visible: &mut Vec<Series<'a>>,
        days: &mut u32,
    ) -> (TimeRange, Option<i64>) {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let mut reset = false;
            let target = self
                .target
                .filter(|value| snapshot.thresholds.contains(value))
                .or_else(|| {
                    snapshot
                        .thresholds
                        .iter()
                        .min_by_key(|level| level.abs_diff(10))
                        .copied()
                })
                .unwrap_or(0);
            let colors = chart::forecast_colors(ui);
            let fits = if focus_has_readings {
                focused.chart.forecasts
            } else {
                [None; 2]
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(heading(ui, "Battery trend", 18.0));
                range_picker(ui, days);
                let active = self.forecast.iter().filter(|enabled| **enabled).count();
                ComboBox::from_id_salt("analytics-forecast")
                    .width(104.0)
                    .selected_text(if active == 0 {
                        "Forecast · Off".to_owned()
                    } else {
                        format!("Forecast · {active} on")
                    })
                    .show_ui(ui, |ui| {
                        ui.set_min_width(260.0);
                        for (index, (enabled, name)) in
                            self.forecast.iter_mut().zip(FORECAST_NAMES).enumerate()
                        {
                            ui.checkbox(enabled, name);
                            if *enabled {
                                forecast_card(
                                    ui,
                                    name,
                                    fits[index],
                                    target,
                                    colors[index],
                                    snapshot.data.generated_at + 14 * SECONDS_PER_DAY,
                                );
                            }
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Show both").clicked() {
                                self.forecast = [true; 2];
                            }
                            if ui.button("Hide all").clicked() {
                                self.forecast = [false; 2];
                            }
                        });
                        if focus_has_readings
                            && focused.device.last_at
                                < snapshot.data.generated_at
                                    - super::super::model::OFFLINE_AFTER_SECONDS
                        {
                            ui.label(caption("Based on a stale reading"));
                        }
                        ui.separator();
                        ui.label(caption("Forecast target"));
                        for &threshold in &snapshot.thresholds {
                            ui.selectable_value(
                                &mut self.target,
                                Some(threshold),
                                format!("{threshold}%"),
                            );
                        }
                    });
                reset |= ui
                    .add_enabled(self.zoom.is_some(), Button::new("Reset view"))
                    .clicked();
                if ui
                    .button(if self.expanded {
                        "Compact chart"
                    } else {
                        "Expand chart"
                    })
                    .clicked()
                {
                    self.expanded = !self.expanded;
                }
                ui.toggle_value(&mut self.selecting_interval, "Select interval");
            });
            if reset {
                self.reset_view();
            }
            let mut visibility_changed = false;
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 20.0;
                ui.spacing_mut().item_spacing = vec2(7.0, 4.0);
                ui.horizontal_wrapped(|ui| {
                    for row in rows
                        .iter()
                        .filter(|row| row.index.is_some())
                    {
                        let mut shown = self.visible(row.identity);
                        let mark = LegendMark::Line(series_color(ui, row.identity));
                        if layer_toggle(
                            ui,
                            &mut shown,
                            row.name,
                            Some(row.name),
                            Some(mark),
                            true,
                        )
                        .changed()
                        {
                            self.set_visible(row.identity, shown);
                            visibility_changed = true;
                        }
                    }
                    let (rule, _) = ui.allocate_exact_size(vec2(1.0, 16.0), Sense::hover());
                    ui.painter()
                        .vline(rule.center().x, rule.y_range(), ui.visuals().window_stroke);
                    let charging = "Charging overlay";
                    let thresholds = "Alert thresholds";
                    let muted = ui.visuals().weak_text_color();
                    layer_toggle(
                        ui,
                        &mut self.charging,
                        charging,
                        Some(charging),
                        Some(LegendMark::Dot(chart::CHARGING)),
                        true,
                    );
                    layer_toggle(
                        ui,
                        &mut self.thresholds,
                        thresholds,
                        Some(thresholds),
                        Some(LegendMark::Dashed(muted)),
                        true,
                    );
                    layer_toggle(ui, &mut self.bridge_gaps, "Bridge gaps", Some("Bridge gaps"), Some(LegendMark::Dashed(muted)), true)
                        .on_hover_text("Dashed links mark unobserved intervals. They contain no recorded readings.");
                });
            });
            if visibility_changed {
                *visible = self.series(ui, snapshot, rows);
            }
            let focused_visible = visible.iter().any(|series| series.index == focused.index);
            let forecasts = std::array::from_fn(|i| {
                if self.forecast[i] && focused_visible {
                    fits[i]
                } else {
                    None
                }
            });
            let latest = visible
                .iter()
                .map(|series| series.device.last_at)
                .max()
                .unwrap_or(focused.device.last_at);
            let generated = snapshot.data.generated_at.max(latest);
            let start = if snapshot.data.window_days == ALL {
                visible
                    .iter()
                    .map(|series| series.device.first_at)
                    .min()
                    .unwrap_or(latest)
            } else {
                snapshot
                    .data
                    .generated_at
                    .saturating_sub(i64::from(snapshot.data.window_days) * SECONDS_PER_DAY)
            }
            .min(latest.saturating_sub(60));
            let projected = forecasts
                .iter()
                .flatten()
                .filter_map(|fit| fit.eta(0))
                .max()
                .unwrap_or(generated);
            let end = if projected > generated {
                projected
                    .saturating_add(1800)
                    .min(generated.saturating_add(14 * SECONDS_PER_DAY))
            } else {
                generated
            };
            let full = TimeRange { start, end };
            let mut range = self
                .zoom
                .and_then(|zoom| {
                    let clipped = TimeRange {
                        start: zoom.start.max(start),
                        end: zoom.end.min(end),
                    };
                    (clipped.start < clipped.end).then_some(clipped)
                })
                .unwrap_or(full);
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.pinned = None;
                self.pinned_activity = None;
                self.interval = None;
                self.readings_interval = None;
                self.drag_start = None;
            }
            ui.horizontal(|ui| {
                ui.label(caption("Actual readings"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(caption(if self.selecting_interval {
                            "Drag to select a range"
                        } else {
                            "Ctrl + wheel to zoom · Shift + wheel to pan"
                        }))
                        .truncate(),
                    );
                });
            });
            let plot = Plot {
                series: visible,
                focused,
                forecasts,
                target,
                thresholds: if self.thresholds {
                    &snapshot.thresholds
                } else {
                    &[]
                },
                show_charging: self.charging,
                bridge_gaps: self.bridge_gaps,
                expanded: self.expanded,
                recorded_end: if end > generated { latest } else { generated },
                full,
            };
            let previous = range;
            let old_pin = self.pinned;
            let hovered = plot.trend(
                ui,
                &mut range,
                &mut self.pinned,
                self.activity_hover
                    .as_ref()
                    .map(|hit| hit.at)
                    .or(self.table_hover),
                &mut self.interval,
                &mut self.drag_start,
                self.selecting_interval,
                self.pinned_activity
                    .as_ref()
                    .or(self.activity_hover.as_ref()),
            );
            if old_pin != self.pinned {
                self.pinned_activity = None;
            }
            if previous != range {
                self.zoom = (range != full).then_some(range);
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(heading(ui, "Activity", 14.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (label, color) in [
                            ("Unknown", ui.visuals().window_stroke.color),
                            (
                                "Offline",
                                ui.visuals().weak_text_color().gamma_multiply(0.4),
                            ),
                            ("Charging", chart::CHARGING),
                            ("Online (device color)", accent(ui)),
                        ] {
                            ui.label(caption(label));
                            let (rect, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                            ui.painter().rect_filled(rect, 2, color);
                        }
                    });
                });
            });
            let (activity_time, activity_hover) = plot.activity(
                ui,
                range,
                &mut self.pinned,
                hovered.or(self.table_hover),
                self.interval,
                &mut self.pinned_activity,
                self.activity_hover.as_ref(),
            );
            if self.activity_hover != activity_hover {
                self.activity_hover = activity_hover;
                ui.ctx().request_repaint();
            }
            let hovered = activity_time.or(hovered);
            ui.separator();
            self.inspector(
                ui,
                focused,
                visible,
                self.pinned.or(hovered).or(self.table_hover),
                &forecasts,
                range,
                target,
            );
            if let Some(interval) = self.interval {
                ui.horizontal_wrapped(|ui| {
                    ui.label(caption(interval_details(
                        focused,
                        interval,
                        self.drag_start.is_some(),
                    )));
                    if ui.button("Zoom to selection").clicked() {
                        self.zoom = Some(interval);
                        ui.ctx().request_repaint();
                    }
                    if ui.button("View readings").clicked() {
                        self.show_interval_readings(interval);
                    }
                    if ui.button("Clear").clicked() {
                        self.interval = None;
                        self.readings_interval = None;
                    }
                });
            }
            (range, hovered)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn inspector(
        &mut self,
        ui: &mut Ui,
        focused: Series<'_>,
        visible: &[Series<'_>],
        at: Option<i64>,
        forecasts: &[Option<Forecast>; 2],
        range: TimeRange,
        target: u8,
    ) {
        Frame::new()
            .fill(sidebar_fill(ui))
            .inner_margin(10)
            .corner_radius(5)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.spacing_mut().interact_size.y = 22.0;
                let activity = self.pinned_activity.clone().or_else(|| {
                    self.activity_hover
                        .clone()
                        .filter(|_| self.pinned.is_none())
                });
                let session = activity
                    .as_ref()
                    .filter(|hit| hit.state == ActivityState::Charging && self.pinned.is_some())
                    .map(|hit| hit.interval)
                    .or_else(|| {
                        self.pinned
                            .filter(|_| self.pinned_activity.is_none())
                            .and_then(|at| focused.chart.charging_at(at))
                    });
                ui.horizontal(|ui| {
                    let state = if self.pinned.is_some() {
                        "Pinned"
                    } else if at.is_some() {
                        "Inspecting"
                    } else {
                        "Time inspector"
                    };
                    ui.label(heading(ui, state, 13.0));
                    ui.add(
                        egui::Label::new(caption(
                            at.map_or_else(|| focused.device.name.clone(), platform::exact_date),
                        ))
                        .truncate(),
                    );
                    if ui
                        .add_enabled(self.pinned.is_some(), Button::new("Unpin"))
                        .clicked()
                    {
                        self.pinned = None;
                        self.pinned_activity = None;
                    }
                });
                let details = activity.map_or_else(
                    || {
                        session.map_or_else(
                            || {
                                format!(
                                    "{} readings · {}",
                                    range.points(&focused.device.points).len(),
                                    online_summary(focused, range)
                                )
                            },
                            |interval| interval_details(focused, interval, false),
                        )
                    },
                    |hit| {
                        let summary = visible
                            .iter()
                            .find(|series| series.device.identity == hit.identity.as_ref())
                            .map_or_else(
                                || duration(hit.interval.end - hit.interval.start),
                                |series| interval_details(*series, hit.interval, false),
                            );
                        format!("{} · {summary}", hit.state.label())
                    },
                );
                ui.horizontal(|ui| {
                    ui.set_min_height(24.0);
                    ui.add(egui::Label::new(caption(details)).truncate());
                    if let Some(session) = session {
                        if ui.button("View session readings").clicked() {
                            self.show_interval_readings(session);
                        }
                    }
                });
                ScrollArea::vertical()
                    .id_salt("time-inspector")
                    .max_height(58.0)
                    .min_scrolled_height(58.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for series in visible {
                            let text = at.map_or_else(
                                || {
                                    format!(
                                        "{} readings",
                                        range.points(&series.device.points).len()
                                    )
                                },
                                |at| chart::reading_at(*series, at),
                            );
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(&series.device.name)
                                        .size(12.0)
                                        .color(series.color),
                                );
                                ui.add(egui::Label::new(caption(text)).truncate());
                            });
                        }
                        if let Some(at) = at {
                            for (index, fit) in forecasts
                                .iter()
                                .enumerate()
                                .filter_map(|(i, fit)| Some((i, fit.as_ref()?)))
                            {
                                if let Some(label) =
                                    chart::forecast_details(*fit, FORECAST_NAMES[index], at, target)
                                {
                                    ui.add(egui::Label::new(caption(&label)).truncate())
                                        .on_hover_text(label);
                                }
                            }
                        }
                    });
            });
    }

    fn show_interval_readings(&mut self, interval: TimeRange) {
        self.readings_interval = Some(interval);
        self.jump_to_readings = true;
        self.collapsed = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn readings(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        visible: &[Series<'_>],
        focused: Option<usize>,
        range: TimeRange,
        hovered: Option<i64>,
        can_export: bool,
    ) -> Option<Export> {
        let mut export = false;
        let mut jump = false;
        let mut table_hover = None;
        let old_pinned = self.pinned;
        let old_hover = self.table_hover;
        let cursor = old_pinned.or(hovered).or(old_hover);
        let result = Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(8)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_min_width(ui.available_width());
                let mut filter = self.filter;
                let mut row_badge = None;
                let (count_rect, painter) = Frame::new()
                    .inner_margin(egui::Margin::symmetric(16, 9))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(heading(ui, "Readings", 18.0));
                            row_badge = Some(ui.painter().add(egui::Shape::Noop));
                            let count_rect = ui.allocate_space(vec2(66.0, 24.0)).1;
                            ComboBox::from_id_salt("analytics-readings-filter")
                                .width(110.0)
                                .selected_text(filter.label())
                                .show_ui(ui, |ui| {
                                    for choice in [
                                        ReadingsFilter::Visible,
                                        ReadingsFilter::Focused,
                                        ReadingsFilter::Charging,
                                    ] {
                                        ui.selectable_value(&mut filter, choice, choice.label());
                                    }
                                });
                            jump = ui
                                .add_enabled(
                                    cursor.is_some() && !self.collapsed,
                                    Button::new("Go to cursor"),
                                )
                                .clicked();
                            export = ui
                                .add_enabled(can_export, Button::new("Export CSV…"))
                                .clicked();
                            let selected = self.selected_reading.as_ref().and_then(|selection| {
                                let device = snapshot
                                    .data
                                    .devices
                                    .iter()
                                    .find(|device| device.identity == selection.identity)?;
                                let index = device
                                    .points
                                    .partition_point(|point| point.t < selection.at);
                                device
                                    .points
                                    .get(index)
                                    .filter(|point| point.t == selection.at)
                                    .map(|point| (device, point))
                            });
                            if ui
                                .add_enabled(selected.is_some(), Button::new("Copy reading"))
                                .clicked()
                            {
                                if let Some((device, point)) = selected {
                                    ui.ctx().copy_text(format!(
                                        "{}\t{}\t{}\t{}\t{}\tOnline",
                                        platform::exact_date(point.t),
                                        device.name,
                                        percent(point.p),
                                        chart::voltage(point.v),
                                        if point.c { "Charging" } else { "Not charging" }
                                    ));
                                }
                            }
                            if self.readings_interval.is_some()
                                && ui.button("Clear interval filter").clicked()
                            {
                                self.readings_interval = None;
                            }
                            if ui
                                .button(if self.collapsed { "Expand" } else { "Collapse" })
                                .clicked()
                            {
                                self.collapsed = !self.collapsed;
                            }
                            (count_rect, ui.painter().clone())
                        })
                        .inner
                    })
                    .inner;
                if filter != self.filter {
                    self.filter = filter;
                    self.invalidate_readings();
                }
                let rows = self.readings.get_or_insert_with(|| {
                    let mut rows = Vec::new();
                    for series in visible {
                        let device = series.index;
                        if filter == ReadingsFilter::Focused && Some(device) != focused {
                            continue;
                        }
                        rows.extend(
                            snapshot.data.devices[device]
                                .points
                                .iter()
                                .enumerate()
                                .filter_map(|(point, reading)| {
                                    (filter != ReadingsFilter::Charging || reading.c)
                                        .then_some((device, point))
                                }),
                        );
                    }
                    rows.sort_unstable_by_key(|&(device, point)| {
                        (snapshot.data.devices[device].points[point].t, device, point)
                    });
                    Arc::new(rows)
                });
                let time =
                    |&(device, point): &Reading| snapshot.data.devices[device].points[point].t;
                let scope = self.readings_interval.map_or(range, |interval| TimeRange {
                    start: range.start.max(interval.start),
                    end: range.end.min(interval.end),
                });
                let start = rows.partition_point(|row| time(row) < scope.start);
                let end = rows
                    .partition_point(|row| time(row) <= scope.end)
                    .max(start);
                let slice = &rows[start..end];
                if let Some(index) = row_badge {
                    let galley = painter.layout_no_wrap(
                        format!("{} rows", slice.len()),
                        egui::FontId::proportional(11.0),
                        ui.visuals().weak_text_color(),
                    );
                    let rect = Rect::from_min_size(
                        count_rect.left_center() - vec2(0.0, 10.0),
                        galley.size() + vec2(12.0, 6.0),
                    );
                    painter.set(
                        index,
                        egui::Shape::rect_stroke(
                            rect,
                            4,
                            ui.visuals().window_stroke,
                            egui::StrokeKind::Inside,
                        ),
                    );
                    painter.galley(
                        rect.min + vec2(6.0, 3.0),
                        galley,
                        ui.visuals().weak_text_color(),
                    );
                }

                let mut edges = self.column_edges;
                let compact = ui.available_width() < 565.0;
                if compact {
                    edges[5] = edges[4];
                }
                if !self.collapsed {
                    let (header, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
                    ui.painter()
                        .rect_filled(header, 0, ui.visuals().widgets.inactive.weak_bg_fill);
                    let labels = [
                        "Local time",
                        "Device / source",
                        "Battery",
                        "Voltage",
                        if compact { "" } else { "Charging" },
                        if compact { "State" } else { "Connection" },
                    ];
                    for (index, (cell, label)) in reading_cells(header, edges)
                        .into_iter()
                        .zip(labels)
                        .enumerate()
                    {
                        if label.is_empty() {
                            continue;
                        }
                        let mut header_ui = cell_ui(ui, cell);
                        header_ui.spacing_mut().button_padding.x = 0.0;
                        let label = if self.sort_column == index {
                            format!("{label} {}", if self.oldest_first { "↑" } else { "↓" })
                        } else {
                            label.to_owned()
                        };
                        if header_ui
                            .add(Button::new(caption(label)).frame(false).truncate())
                            .clicked()
                        {
                            if self.sort_column == index {
                                self.oldest_first = !self.oldest_first;
                            } else {
                                self.sort_column = index;
                                self.oldest_first = index == 1;
                            }
                        }
                    }
                    for index in 1..6 {
                        if compact && index == 5 {
                            continue;
                        }
                        let x = header.left() + header.width() * edges[index];
                        let handle = Rect::from_center_size(
                            pos2(x, header.center().y),
                            vec2(8.0, header.height()),
                        );
                        let response = ui
                            .interact(
                                handle,
                                ui.scope_id().with(("reading-column", index)),
                                Sense::drag(),
                            )
                            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                        if response.dragged() && header.width() > 220.0 {
                            let minimum = (44.0 / header.width()).min(0.08);
                            let minimum_edge = edges[index - 1] + minimum;
                            let next = if compact && index == 4 { 6 } else { index + 1 };
                            let maximum_edge = edges[next] - minimum;
                            if minimum_edge < maximum_edge {
                                self.column_edges[index] = (edges[index]
                                    + response.drag_delta().x / header.width())
                                .clamp(minimum_edge, maximum_edge);
                            }
                        }
                    }
                }
                let sorted = if self.sort_column == 0 {
                    None
                } else {
                    let key = start..end;
                    if !self
                        .sorted_readings
                        .as_ref()
                        .is_some_and(|(range, column, _)| {
                            *range == key && *column == self.sort_column
                        })
                    {
                        let mut values = slice.to_vec();
                        values.sort_by(|&(a, a_row), &(b, b_row)| {
                            let a = &snapshot.data.devices[a];
                            let b = &snapshot.data.devices[b];
                            let first = a.points[a_row];
                            let second = b.points[b_row];
                            match self.sort_column {
                                1 => a
                                    .name
                                    .cmp(&b.name)
                                    .then_with(|| a.identity.cmp(&b.identity)),
                                2 => first.p.cmp(&second.p),
                                3 => first.v.cmp(&second.v),
                                4 => first.c.cmp(&second.c),
                                5 if compact => first.c.cmp(&second.c),
                                _ => first.t.cmp(&second.t),
                            }
                        });
                        self.sorted_readings = Some((key, self.sort_column, Arc::new(values)));
                    }
                    self.sorted_readings.as_ref().map(|(_, _, rows)| rows)
                };
                let displayed = sorted.map_or(slice, |rows| rows.as_slice());
                if !self.collapsed {
                    let mut scroll = ScrollArea::vertical()
                        .id_salt("analytics-reading-rows")
                        .max_height(216.0)
                        .min_scrolled_height(216.0)
                        .auto_shrink([false, true]);
                    let nearest = cursor.filter(|_| !slice.is_empty()).map(|at| {
                        let after = slice
                            .partition_point(|row| time(row) < at)
                            .min(slice.len() - 1);
                        let before = after.saturating_sub(1);
                        if time(&slice[before]).abs_diff(at) < time(&slice[after]).abs_diff(at) {
                            before
                        } else {
                            after
                        }
                    });
                    let display_index = |index| {
                        if self.oldest_first {
                            index
                        } else {
                            displayed.len() - 1 - index
                        }
                    };
                    let keyboard = ui.scope_id().with("reading-grid-keyboard");
                    let keyboard_rect =
                        Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 216.0));
                    let keyboard_response =
                        ui.interact(keyboard_rect, keyboard, Sense::focusable_noninteractive());
                    keyboard_response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Grid,
                            true,
                            "Battery readings. Use arrow keys to select a reading.",
                        )
                    });
                    if keyboard_response.has_focus() {
                        ui.memory_mut(|memory| {
                            memory.set_focus_lock_filter(
                                keyboard,
                                egui::EventFilter {
                                    vertical_arrows: true,
                                    ..egui::EventFilter::default()
                                },
                            )
                        });
                        ui.painter().rect_stroke(
                            keyboard_rect,
                            2,
                            ui.visuals().selection.stroke,
                            egui::StrokeKind::Inside,
                        );
                    }
                    let step = if ui.memory(|memory| memory.has_focus(keyboard)) {
                        ui.input(|input| {
                            i32::from(input.key_pressed(egui::Key::ArrowDown))
                                - i32::from(input.key_pressed(egui::Key::ArrowUp))
                        })
                    } else {
                        0
                    };
                    let mut scroll_index = None;
                    if let Some(nearest) = nearest.filter(|_| jump) {
                        scroll_index = displayed
                            .iter()
                            .position(|row| *row == slice[nearest])
                            .map(display_index);
                    }
                    if step != 0 && !displayed.is_empty() {
                        let current = self
                            .selected_reading
                            .as_ref()
                            .and_then(|selection| {
                                displayed.iter().position(|&(device, point)| {
                                    let device = &snapshot.data.devices[device];
                                    selection.matches(&device.identity, device.points[point].t)
                                })
                            })
                            .map(display_index);
                        let index = current.map_or(0, |i| {
                            i.saturating_add_signed(step as isize)
                                .min(displayed.len() - 1)
                        });
                        let row = displayed[display_index(index)];
                        self.selected_reading = Some(SelectedReading {
                            identity: snapshot.data.devices[row.0].identity.clone(),
                            at: time(&row),
                        });
                        self.pinned = Some(time(&row));
                        self.pinned_activity = None;
                        scroll_index = Some(index);
                    }
                    if let Some(index) = scroll_index {
                        scroll =
                            scroll.vertical_scroll_offset((index as f32 * 34.0 - 85.0).max(0.0));
                    }
                    scroll.show_rows(ui, 34.0, displayed.len(), |ui, indices| {
                        for index in indices {
                            let row = displayed[display_index(index)];
                            let (device, point) = row;
                            let device_data = &snapshot.data.devices[device];
                            let point = device_data.points[point];
                            let color = visible
                                .iter()
                                .find(|series| series.index == device)
                                .map_or_else(|| accent(ui), |series| series.color);
                            let (rect, response) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 34.0),
                                Sense::click(),
                            );
                            if response.hovered() {
                                table_hover = Some(point.t);
                            }
                            if response.clicked() {
                                ui.memory_mut(|memory| memory.request_focus(keyboard));
                                self.selected_reading = Some(SelectedReading {
                                    identity: device_data.identity.clone(),
                                    at: point.t,
                                });
                                self.pinned_activity = None;
                                self.pinned = Some(point.t);
                            }
                            if response.hovered()
                                || self.selected_reading.as_ref().is_some_and(|selection| {
                                    selection.matches(&device_data.identity, point.t)
                                }) && self.pinned == Some(point.t)
                                || nearest.is_some_and(|i| slice[i] == row)
                            {
                                ui.painter()
                                    .rect_filled(rect, 0, ui.visuals().selection.bg_fill);
                            }
                            ui.painter().hline(
                                rect.x_range(),
                                rect.bottom(),
                                ui.visuals().window_stroke,
                            );
                            let cells = reading_cells(rect, edges);
                            let labels = [
                                RichText::new(if compact {
                                    platform::summary_date(point.t)
                                } else {
                                    platform::date(point.t)
                                }),
                                RichText::new(format!(
                                    "{} · {}",
                                    device_data.name,
                                    source(&device_data.identity)
                                )),
                                RichText::new(percent(point.p)).strong(),
                                RichText::new(chart::voltage(point.v)),
                                RichText::new(if point.c { "Yes" } else { "No" }),
                                RichText::new(if compact && point.c {
                                    "Online · Charging"
                                } else {
                                    "Online"
                                }),
                            ];
                            for (index, (mut cell, label)) in
                                cells.into_iter().zip(labels).enumerate()
                            {
                                if compact && index == 4 {
                                    continue;
                                }
                                if matches!(index, 1 | 5) {
                                    ui.painter().rect_filled(
                                        Rect::from_center_size(
                                            cell.left_center() + vec2(4.0, 0.0),
                                            vec2(7.0, 7.0),
                                        ),
                                        2,
                                        color,
                                    );
                                    cell.min.x += 12.0;
                                }
                                cell_ui(ui, cell).add(
                                    egui::Label::new(label.size(12.0))
                                        .truncate()
                                        .sense(Sense::empty()),
                                );
                            }
                        }
                    });
                    if slice.is_empty() {
                        Frame::new()
                            .inner_margin(12)
                            .show(ui, |ui| ui.label(caption("No readings match this view.")));
                    }
                }
                Frame::new()
                    .inner_margin(egui::Margin::symmetric(16, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(caption(format!(
                                "{} recorded samples · {} visible devices",
                                slice.len(),
                                visible.len()
                            )));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(caption("Recorded readings only"));
                            });
                        });
                    });
                (export && !slice.is_empty()).then(|| Export {
                    rows: Arc::clone(sorted.unwrap_or(rows)),
                    range: if sorted.is_some() {
                        0..displayed.len()
                    } else {
                        start..end
                    },
                    oldest_first: self.oldest_first,
                })
            });
        self.table_hover = table_hover;
        if std::mem::take(&mut self.jump_to_readings) {
            result.response.scroll_to_me(Some(Align::BOTTOM));
        }
        if old_hover != self.table_hover || old_pinned != self.pinned {
            ui.ctx().request_repaint();
        }
        result.inner
    }
}

fn online_summary(series: Series<'_>, interval: TimeRange) -> String {
    series
        .chart
        .online_time(series.device, interval)
        .map_or_else(
            || "Activity unavailable".to_owned(),
            |seconds| format!("{} observed online", duration(seconds)),
        )
}

fn interval_details(series: Series<'_>, interval: TimeRange, dragging: bool) -> String {
    let points = interval.points(&series.device.points);
    let elapsed = interval.end - interval.start;
    let mut result = format!("{} elapsed · {} readings", duration(elapsed), points.len());
    if dragging {
        return result;
    }
    use std::fmt::Write;
    let _ = write!(result, " · {}", online_summary(series, interval));
    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        if let (Some(start), Some(end)) = (first.p, last.p) {
            let change = i16::from(end) - i16::from(start);
            let sample_span = last.t - first.t;
            if sample_span > 0 {
                let _ = write!(result, " · {change:+} pp between readings");
                if change <= 0
                    && !first.c
                    && !last.c
                    && series
                        .chart
                        .continuous_discharge(TimeRange::ordered(first.t, last.t))
                {
                    let rate = -f64::from(change) * SECONDS_PER_DAY as f64 / sample_span as f64;
                    let _ = write!(result, " · {rate:.1} pp/day elapsed");
                }
            }
        }
    }
    result
}

pub(super) struct Export {
    pub(super) rows: Arc<Vec<Reading>>,
    pub(super) range: Range<usize>,
    pub(super) oldest_first: bool,
}

fn cell_ui(ui: &mut Ui, rect: Rect) -> Ui {
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    cell.set_clip_rect(ui.clip_rect().intersect(rect));
    cell
}

fn reading_cells(rect: Rect, fractions: [f32; 7]) -> [Rect; 6] {
    std::array::from_fn(|index| {
        Rect::from_min_max(
            pos2(
                rect.left() + fractions[index] * rect.width() + if index <= 1 { 12.0 } else { 5.0 },
                rect.top(),
            ),
            pos2(
                rect.left() + fractions[index + 1] * rect.width() - 3.0,
                rect.bottom(),
            ),
        )
    })
}

#[derive(Clone, Copy)]
enum LegendMark {
    Dot(Color32),
    Line(Color32),
    Dashed(Color32),
}

impl LegendMark {
    fn width(self) -> f32 {
        match self {
            Self::Dot(_) => 8.0,
            Self::Line(_) | Self::Dashed(_) => 23.0,
        }
    }

    fn paint(self, ui: &Ui, rect: Rect) {
        let y = rect.center().y;
        match self {
            Self::Dot(color) => {
                ui.painter().rect_filled(
                    Rect::from_center_size(rect.center(), vec2(8.0, 8.0)),
                    2,
                    color,
                );
            }
            Self::Line(color) => {
                ui.painter()
                    .hline(rect.x_range(), y, Stroke::new(1.6, color));
            }
            Self::Dashed(color) => {
                for offset in [0.0, 8.0, 16.0] {
                    let left = rect.left() + offset;
                    ui.painter()
                        .hline(left..=left + 5.0, y, Stroke::new(1.6, color));
                }
            }
        }
    }
}

fn layer_toggle(
    ui: &mut Ui,
    checked: &mut bool,
    label: &str,
    text: Option<&str>,
    mark: Option<LegendMark>,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let text = text.map(|text| {
            let mut job = egui::text::LayoutJob::simple_singleline(
                text.to_owned(),
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
            job.wrap.max_width = 110.0;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            ui.painter().layout_job(job)
        });
        let width = 14.0
            + mark.map_or(0.0, |mark| mark.width() + 6.0)
            + text.as_ref().map_or(0.0, |text| text.size().x + 6.0);
        let (rect, mut response) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click());
        if response.clicked() {
            *checked = !*checked;
            response.mark_changed();
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, *checked, label)
        });
        let icon = Rect::from_center_size(rect.left_center() + vec2(7.0, 0.0), vec2(13.0, 13.0));
        let fill = if *checked {
            accent(ui)
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        ui.painter().rect(
            icon,
            2,
            fill,
            ui.visuals().window_stroke,
            egui::StrokeKind::Inside,
        );
        if *checked {
            let ink = if ui.visuals().dark_mode {
                canvas(ui)
            } else {
                Color32::WHITE
            };
            let point = |x, y| icon.min + vec2(x, y);
            ui.painter().add(egui::Shape::line(
                vec![point(3.0, 6.5), point(5.5, 9.0), point(10.0, 4.0)],
                Stroke::new(1.6, ink),
            ));
        }
        if response.has_focus() || response.hovered() {
            ui.painter().rect_stroke(
                icon.expand(2.0),
                3,
                Stroke::new(1.0, accent(ui)),
                egui::StrokeKind::Outside,
            );
        }
        let mut left = icon.right() + 6.0;
        if let Some(mark) = mark {
            mark.paint(
                ui,
                Rect::from_min_size(pos2(left, rect.top()), vec2(mark.width(), rect.height())),
            );
            left += mark.width() + 6.0;
        }
        if let Some(text) = text {
            let top = rect.center().y - text.size().y * 0.5;
            ui.painter()
                .galley(pos2(left, top), text, ui.visuals().text_color());
        }
        response
    })
    .inner
}

fn forecast_card(
    ui: &mut Ui,
    name: &str,
    forecast: Option<Forecast>,
    target: u8,
    color: Color32,
    horizon: i64,
) {
    let frame = Frame::new()
        .fill(sidebar_fill(ui))
        .stroke(ui.visuals().window_stroke)
        .inner_margin(egui::Margin {
            left: 26,
            right: 10,
            top: 9,
            bottom: 9,
        })
        .corner_radius(7)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(caption(format!("{name} · ETA {target}%")));
            ui.label(
                RichText::new(forecast.map_or_else(
                    || "Unavailable".into(),
                    |fit| {
                        fit.eta(target).map_or_else(
                            || format!("Below {target}%"),
                            |eta| {
                                if eta > horizon {
                                    format!("{} · beyond chart", platform::summary_date(eta))
                                } else {
                                    platform::summary_date(eta)
                                }
                            },
                        )
                    },
                ))
                .strong(),
            );
            if let Some(fit) = forecast {
                ui.add(
                    egui::Label::new(caption(format!(
                        "From {} · {:.1} pp/day elapsed",
                        platform::summary_date(fit.start.t),
                        fit.rate
                    )))
                    .truncate(),
                );
            } else {
                ui.label(caption("More discharge data needed"));
            }
        });
    ui.painter().rect_filled(
        Rect::from_center_size(
            pos2(
                frame.response.rect.left() + 14.0,
                frame.response.rect.center().y,
            ),
            vec2(4.0, 32.0),
        ),
        2,
        color,
    );
}

fn range_label(days: u32) -> String {
    match days {
        ALL => "all recorded".to_owned(),
        1 => "24 hours of".to_owned(),
        days => format!("{days} days of"),
    }
}

fn range_picker(ui: &mut Ui, days: &mut u32) {
    Frame::new()
        .fill(canvas(ui))
        .stroke(ui.visuals().window_stroke)
        .corner_radius(6)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.spacing_mut().button_padding.x = 8.0;
                for (value, label) in [(1, "24h"), (7, "7d"), (30, "30d"), (ALL, "All")] {
                    if ui
                        .add(
                            Button::new(label)
                                .frame_when_inactive(*days == value)
                                .selected(*days == value)
                                .min_size(vec2(34.0, 26.0)),
                        )
                        .clicked()
                    {
                        *days = value;
                    }
                }
            });
        });
}

fn battery_bar(ui: &mut Ui, width: f32, level: Option<u8>, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 5.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 2, ui.visuals().window_stroke.color);
    if let Some(level) = level {
        let fill = Rect::from_min_size(
            rect.min,
            vec2(width * f32::from(level.min(100)) / 100.0, rect.height()),
        );
        ui.painter().rect_filled(fill, 2, color);
    }
}

fn badge(ui: &mut Ui, label: String) {
    Frame::new()
        .stroke(ui.visuals().window_stroke)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(5, 3))
        .show(ui, |ui| {
            ui.label(caption(label).size(11.0));
        });
}

pub(super) fn heading(ui: &Ui, text: impl Into<String>, size: f32) -> RichText {
    let mut font = egui::TextStyle::Heading.resolve(ui.style());
    font.size = size;
    RichText::new(text.into()).font(font).strong()
}

fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(12.0).weak()
}

fn source(identity: &str) -> &'static str {
    match identity.split(':').next() {
        Some("bluetooth") => "Bluetooth",
        Some("hid") => "HID",
        Some("xinput") => "XInput",
        _ => "Battery monitor",
    }
}

fn series_color(ui: &Ui, identity: &str) -> Color32 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hash = DefaultHasher::new();
    identity.hash(&mut hash);
    let index = (hash.finish() % 13) as usize;
    if index == 0 {
        return accent(ui);
    }
    let palette = if ui.visuals().dark_mode {
        [
            [91, 214, 165],
            [148, 168, 255],
            [233, 155, 212],
            [255, 183, 124],
        ]
    } else {
        [[17, 110, 77], [58, 79, 163], [147, 49, 122], [152, 74, 13]]
    };
    let [r, g, b] = palette[(index - 1) % palette.len()];
    Color32::from_rgb(r, g, b)
}

fn sidebar_fill(ui: &Ui) -> Color32 {
    canvas(ui).lerp_to_gamma(ui.visuals().window_fill, 0.42)
}

fn focus_marker(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(255, 160, 73)
    } else {
        Color32::from_rgb(161, 76, 14)
    }
}
