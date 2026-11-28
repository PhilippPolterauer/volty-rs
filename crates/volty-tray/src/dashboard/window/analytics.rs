use std::{collections::BTreeSet, ops::Range, sync::Arc};

use eframe::egui::{
    self, pos2, vec2, Align, Button, Color32, ComboBox, Frame, Layout, Rect, RichText, ScrollArea,
    Sense, Stroke, Ui, Widget,
};

use super::{
    super::model::SECONDS_PER_DAY,
    accent, canvas, card,
    chart::{self, Forecast, Plot, Series, TimeRange},
    duration, live_device, live_percent, percent, platform, selected_inventory, Dashboard,
    Snapshot,
};

const ALL: u32 = u32::MAX;
const FORECAST_NAMES: [&str; 2] = ["Since last charge", "Current discharge rate"];

pub(super) type Reading = (usize, usize);

pub(super) struct HistoryView {
    search: String,
    compared: BTreeSet<String>,
    comparing: bool,
    forecast: [bool; 2],
    charging: bool,
    thresholds: bool,
    target: Option<u8>,
    zoom: Option<TimeRange>,
    pinned: Option<i64>,
    table_hover: Option<i64>,
    activity_hover: Option<i64>,
    oldest_first: bool,
    collapsed: bool,
    filter: ReadingsFilter,
    readings: Option<Arc<Vec<Reading>>>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self {
            search: String::new(),
            compared: BTreeSet::new(),
            comparing: false,
            forecast: [true; 2],
            charging: true,
            thresholds: true,
            target: None,
            zoom: None,
            pinned: None,
            table_hover: None,
            activity_hover: None,
            oldest_first: false,
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
        let Some((focused_row, focused_index)) =
            focused.and_then(|row| Some((row, rows[row].index?)))
        else {
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
            color: series_color(ui, row),
        };
        let focused = series(focused_index, focused_row, ui);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(heading(ui, rows[focused_row].name, 25.0)).truncate());
                ui.label(caption(format!(
                    "Focused device · Last reading {} · {} visible",
                    platform::summary_date(focused.device.last_at),
                    rows.iter()
                        .filter(|row| row.index.is_some() && self.visible(row.identity, focus))
                        .count()
                )));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(Button::new("Compare devices").selected(self.comparing))
                    .clicked()
                {
                    self.comparing = !self.comparing;
                    if self.comparing
                        && !rows.iter().any(|row| {
                            row.index.is_some()
                                && Some(row.identity) != focus.as_deref()
                                && self.compared.contains(row.identity)
                        })
                    {
                        if let Some(other) = rows.iter().find(|row| {
                            row.index.is_some() && Some(row.identity) != focus.as_deref()
                        }) {
                            self.compared.insert(other.identity.to_owned());
                        }
                    }
                    self.invalidate_readings();
                    self.reset_view();
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
        self.metrics(ui, focused, &rows[focused_row]);
        let mut visible = self.series(ui, snapshot, rows, focus);
        let (range, hovered) = self.trend(ui, snapshot, rows, focus, focused, &mut visible, days);
        self.readings(
            ui,
            snapshot,
            &visible,
            focused_index,
            range,
            hovered,
            can_export,
        )
    }

    pub(super) fn initial_comparison(&mut self, data: &super::DashboardData, focus: Option<&str>) {
        if let Some(other) = data
            .devices
            .iter()
            .find(|device| Some(device.identity.as_str()) != focus)
        {
            self.comparing = true;
            self.compared.insert(other.identity.clone());
        }
    }

    pub(super) fn invalidate_readings(&mut self) {
        self.readings = None;
    }

    fn reset_view(&mut self) {
        self.zoom = None;
        self.pinned = None;
        self.table_hover = None;
        self.activity_hover = None;
    }

    fn series<'a>(
        &self,
        ui: &Ui,
        snapshot: &'a Snapshot,
        rows: &[DeviceRow<'_>],
        focus: &Option<String>,
    ) -> Vec<Series<'a>> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| self.visible(row.identity, focus))
            .filter_map(|(row, item)| {
                let index = item.index?;
                Some(Series {
                    index,
                    device: &snapshot.data.devices[index],
                    chart: &snapshot.charts[index],
                    color: series_color(ui, row),
                })
            })
            .collect()
    }

    fn visible(&self, id: &str, focus: &Option<String>) -> bool {
        focus.as_deref() == Some(id) || (self.comparing && self.compared.contains(id))
    }

    fn set_visible(
        &mut self,
        id: &str,
        show: bool,
        rows: &[DeviceRow<'_>],
        focus: &mut Option<String>,
    ) {
        if show {
            self.comparing = true;
            self.compared.insert(id.to_owned());
        } else if focus.as_deref() != Some(id) {
            self.compared.remove(id);
        } else if let Some(next) = rows.iter().find(|row| {
            row.index.is_some() && row.identity != id && self.visible(row.identity, focus)
        }) {
            self.compared.remove(id);
            *focus = Some(next.identity.to_owned());
            self.reset_view();
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
                            .filter(|row| row.index.is_some() && self.visible(row.identity, focus))
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
            for (i, row) in rows.iter().enumerate().filter(|(_, row)| {
                query.is_empty() || row.name.to_lowercase().contains(&query) || row.identity.to_lowercase().contains(&query)
            }) {
                let active = focus.as_deref() == Some(row.identity);
                let mut visible = self.visible(row.identity, focus);
                let color = series_color(ui, i);
                let fill = if active { sidebar_fill(ui).lerp_to_gamma(accent(ui), 0.14) } else { Color32::TRANSPARENT };
                let card = Frame::new().fill(fill).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 9)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let can_hide = !active || rows.iter().any(|other| other.index.is_some() && other.identity != row.identity && self.visible(other.identity, focus));
                        let check = layer_toggle(ui, &mut visible, &format!("Show {} on the chart", row.name), None, None, row.index.is_some() && can_hide);
                        if check.on_hover_text(format!("Show {} on the chart", row.name)).changed() {
                            self.set_visible(row.identity, visible, rows, focus);
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
                            if self.comparing {
                                if let Some(previous) = focus.as_ref() {
                                    self.compared.insert(previous.clone());
                                }
                            }
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

    fn metrics(&self, ui: &mut Ui, series: Series<'_>, row: &DeviceRow<'_>) {
        let cycle = series.chart.forecasts[0];
        let values = [
            ("Current battery", row.level_label.clone(), ""),
            (
                "Est. remaining · cycle",
                cycle
                    .and_then(|fit| fit.eta(0).map(|at| duration(at - fit.latest.t)))
                    .unwrap_or_else(|| "—".into()),
                if cycle.is_some() {
                    "Until estimated 0%"
                } else {
                    "Cycle estimate unavailable"
                },
            ),
            (
                "Drain per day · cycle",
                cycle.map_or_else(|| "—".into(), |fit| format!("{:.1}%", fit.rate)),
                "Percentage points / day",
            ),
            (
                "Recorded readings",
                series.device.samples.to_string(),
                "Selected history range",
            ),
        ];
        card(ui, |ui| {
            let columns = if ui.available_width() < 620.0 { 2 } else { 4 };
            for group in values.chunks(columns) {
                ui.columns(columns, |uis| {
                    for (column, (ui, (label, value, note))) in
                        uis.iter_mut().zip(group).enumerate()
                    {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        let current = *label == "Current battery";
                        if column > 0 {
                            let x = ui.min_rect().left() - 4.0;
                            let y = ui.cursor().top();
                            ui.painter()
                                .vline(x, y..=y + 77.0, ui.visuals().window_stroke);
                        }
                        ui.label(caption(*label));
                        let value = heading(ui, value, if current { 34.0 } else { 27.0 });
                        ui.label(if current {
                            value.color(series.color)
                        } else {
                            value
                        });
                        if current {
                            battery_bar(
                                ui,
                                ui.available_width().min(150.0),
                                row.level,
                                series.color,
                            );
                        } else {
                            ui.label(caption(*note));
                        }
                    }
                });
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn trend<'a>(
        &mut self,
        ui: &mut Ui,
        snapshot: &'a Snapshot,
        rows: &[DeviceRow<'_>],
        focus: &mut Option<String>,
        focused: Series<'a>,
        visible: &mut Vec<Series<'a>>,
        days: &mut u32,
    ) -> (TimeRange, Option<i64>) {
        card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let mut reset = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(heading(ui, "Battery trend", 18.0));
                if ui.available_size_before_wrap().x > 440.0 {
                    ui.add_space(ui.available_size_before_wrap().x - 440.0);
                }
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
                        for (enabled, name) in self.forecast.iter_mut().zip(FORECAST_NAMES) {
                            reset |= ui.checkbox(enabled, name).changed();
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
                reset |= ui.button("Reset view").clicked();
            });
            if reset {
                self.reset_view();
            }
            let mut visibility_changed = false;
            let fit_colors = chart::forecast_colors(ui);
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 20.0;
                ui.spacing_mut().item_spacing = vec2(7.0, 4.0);
                ui.horizontal_wrapped(|ui| {
                    for (row_index, row) in rows
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.index.is_some())
                    {
                        let mut shown = self.visible(row.identity, focus);
                        let mark = LegendMark::Line(series_color(ui, row_index));
                        if layer_toggle(
                            ui,
                            &mut shown,
                            row.name,
                            Some(row.name),
                            Some(mark),
                            Some(row.identity) != focus.as_deref(),
                        )
                        .changed()
                        {
                            self.set_visible(row.identity, shown, rows, focus);
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
                    let mut any_fit = false;
                    for (index, color) in fit_colors.into_iter().enumerate() {
                        if self.forecast[index] && focused.chart.forecasts[index].is_some() {
                            let label = if index == 0 {
                                "Since last charge"
                            } else {
                                "Current rate"
                            };
                            legend_key(ui, label, LegendMark::Dashed(color));
                            any_fit = true;
                        }
                    }
                    if any_fit {
                        legend_key(
                            ui,
                            "Historical fit",
                            LegendMark::Dashed(muted.gamma_multiply(0.7)),
                        );
                    }
                });
            });
            if visibility_changed {
                *visible = self.series(ui, snapshot, rows, focus);
                self.zoom = None;
            }
            let forecasts = std::array::from_fn(|i| {
                if self.forecast[i] {
                    focused.chart.forecasts[i]
                } else {
                    None
                }
            });
            let target = self
                .target
                .filter(|value| snapshot.thresholds.contains(value))
                .unwrap_or_else(|| {
                    snapshot
                        .thresholds
                        .iter()
                        .min_by_key(|threshold| threshold.abs_diff(10))
                        .copied()
                        .unwrap_or(10)
                });
            let colors = fit_colors;
            if self.forecast.iter().any(|enabled| *enabled) {
                let cards = self
                    .forecast
                    .iter()
                    .enumerate()
                    .filter(|(_, enabled)| **enabled)
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                ui.columns(cards.len(), |columns| {
                    for (column, i) in columns.iter_mut().zip(cards) {
                        forecast_card(column, FORECAST_NAMES[i], forecasts[i], target, colors[i]);
                    }
                });
            }
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
                .filter(|zoom| zoom.start >= start.saturating_sub(60) && zoom.end <= end)
                .unwrap_or(full);
            ui.horizontal(|ui| {
                ui.label(caption("Actual readings"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(caption("Scroll to zoom · Shift + wheel to pan"))
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
                recorded_end: if end > generated { latest } else { generated },
                full,
            };
            let previous = range;
            let hovered = plot.trend(
                ui,
                &mut range,
                &mut self.pinned,
                self.activity_hover.or(self.table_hover),
            );
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
            let activity_hover =
                plot.activity(ui, range, &mut self.pinned, hovered.or(self.table_hover));
            if self.activity_hover != activity_hover {
                self.activity_hover = activity_hover;
                ui.ctx().request_repaint();
            }
            let hovered = activity_hover.or(hovered);
            ui.separator();
            self.inspector(
                ui,
                visible,
                self.pinned.or(hovered).or(self.table_hover),
                latest,
                &forecasts,
            );
            ui.label(caption(if self.pinned.is_some() {
                "Timestamp pinned · Click the chart to clear"
            } else {
                "Hover to inspect · Click to pin a timestamp"
            }));
            (range, hovered)
        })
    }

    fn inspector(
        &self,
        ui: &mut Ui,
        visible: &[Series<'_>],
        at: Option<i64>,
        latest: i64,
        forecasts: &[Option<Forecast>; 2],
    ) {
        Frame::new()
            .fill(sidebar_fill(ui))
            .inner_margin(10)
            .corner_radius(5)
            .show(ui, |ui| {
                let columns = if ui.available_width() < 720.0 { 2 } else { 3 };
                for (group, chunk) in visible.chunks(columns - 1).enumerate() {
                    ui.columns(columns, |uis| {
                        if group == 0 {
                            let ui = &mut uis[0];
                            ui.label(heading(ui, "Time inspector", 13.0));
                            ui.label(caption(at.map_or_else(
                                || {
                                    "Hover over the chart, an activity lane, or a reading."
                                        .to_owned()
                                },
                                platform::date,
                            )));
                            if let Some(at) = at.filter(|&at| at > latest) {
                                for (i, fit) in forecasts
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(i, forecast)| Some((i, forecast.as_ref()?)))
                                {
                                    let estimate = f64::from(fit.latest.p.unwrap_or(0))
                                        - fit.rate * (at - fit.latest.t) as f64
                                            / SECONDS_PER_DAY as f64;
                                    ui.label(caption(format!(
                                        "{} · {:.1}%",
                                        FORECAST_NAMES[i],
                                        estimate.max(0.0)
                                    )));
                                }
                            }
                        }
                        for (ui, series) in uis.iter_mut().skip(1).zip(chunk) {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&series.device.name)
                                        .size(13.0)
                                        .strong()
                                        .color(series.color),
                                )
                                .truncate(),
                            );
                            if let Some(at) = at {
                                let nearby = (at <= latest)
                                    .then(|| chart::nearest(&series.device.points, at))
                                    .flatten()
                                    .filter(|point| point.t.abs_diff(at) <= 6 * 3600);
                                ui.label(caption(nearby.map_or_else(
                                    || "No nearby reading".into(),
                                    |point| {
                                        format!(
                                            "{} · {}{}",
                                            percent(point.p),
                                            platform::date(point.t),
                                            if point.c { " · Charging" } else { "" }
                                        )
                                    },
                                )));
                            } else {
                                let total =
                                    series.device.online_seconds + series.device.offline_seconds;
                                ui.label(caption(if total > 0 {
                                    format!(
                                        "{:.0}% online · {} online",
                                        100.0 * series.device.online_seconds as f64 / total as f64,
                                        duration(series.device.online_seconds)
                                    )
                                } else {
                                    "Activity unavailable".into()
                                }));
                            }
                        }
                    });
                }
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn readings(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        visible: &[Series<'_>],
        focused: usize,
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
                            if ui.available_size_before_wrap().x > 464.0 {
                                ui.add_space(ui.available_size_before_wrap().x - 464.0);
                            }
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
                        if filter == ReadingsFilter::Focused && device != focused {
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
                let start = rows.partition_point(|row| time(row) < range.start);
                let end = rows.partition_point(|row| time(row) <= range.end);
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

                if !self.collapsed {
                    let (header, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
                    ui.painter()
                        .rect_filled(header, 0, ui.visuals().widgets.inactive.weak_bg_fill);
                    let cells = reading_cells(header);
                    let mut sort_cell = cell_ui(ui, cells[0]);
                    sort_cell.spacing_mut().button_padding.x = 0.0;
                    if sort_cell
                        .add(
                            Button::new(if self.oldest_first {
                                "Local time ↑"
                            } else {
                                "Local time ↓"
                            })
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.oldest_first = !self.oldest_first;
                    }
                    let labels = [
                        "Device / source",
                        "Battery",
                        "Voltage",
                        "Charging",
                        "Connection",
                    ];
                    for (cell, label) in cells.into_iter().skip(1).zip(labels) {
                        cell_ui(ui, cell).add(egui::Label::new(caption(label)).truncate());
                    }
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
                    if let Some(nearest) = nearest.filter(|_| jump) {
                        let index = if self.oldest_first {
                            nearest
                        } else {
                            slice.len() - 1 - nearest
                        };
                        scroll =
                            scroll.vertical_scroll_offset((index as f32 * 34.0 - 85.0).max(0.0));
                    }
                    scroll.show_rows(ui, 34.0, slice.len(), |ui, indices| {
                        for index in indices {
                            let (device, point) = slice[if self.oldest_first {
                                index
                            } else {
                                slice.len() - 1 - index
                            }];
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
                                self.pinned = if self.pinned == Some(point.t) {
                                    None
                                } else {
                                    Some(point.t)
                                };
                            }
                            if response.hovered()
                                || nearest.is_some_and(|i| time(&slice[i]) == point.t)
                            {
                                ui.painter()
                                    .rect_filled(rect, 0, ui.visuals().selection.bg_fill);
                            }
                            ui.painter().hline(
                                rect.x_range(),
                                rect.bottom(),
                                ui.visuals().window_stroke,
                            );
                            let cells = reading_cells(rect);
                            let labels = [
                                RichText::new(platform::date(point.t)),
                                RichText::new(format!(
                                    "{} · {}",
                                    device_data.name,
                                    source(&device_data.identity)
                                )),
                                RichText::new(percent(point.p)).strong(),
                                RichText::new(chart::voltage(point.v)),
                                RichText::new(if point.c { "Yes" } else { "No" }),
                                RichText::new("Online"),
                            ];
                            for (index, (mut cell, label)) in
                                cells.into_iter().zip(labels).enumerate()
                            {
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
                    rows: Arc::clone(rows),
                    range: start..end,
                    oldest_first: self.oldest_first,
                })
            })
            .inner;
        self.table_hover = table_hover;
        if old_hover != self.table_hover || old_pinned != self.pinned {
            ui.ctx().request_repaint();
        }
        result
    }
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

fn reading_cells(rect: Rect) -> [Rect; 6] {
    let fractions = [0.0, 0.22, 0.52, 0.63, 0.74, 0.86, 1.0];
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

fn legend_key(ui: &mut Ui, label: &str, mark: LegendMark) {
    let label = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(11.0),
        ui.visuals().weak_text_color(),
    );
    let (rect, _) = ui.allocate_exact_size(
        vec2(mark.width() + 6.0 + label.size().x, 20.0),
        Sense::hover(),
    );
    mark.paint(
        ui,
        Rect::from_min_size(rect.min, vec2(mark.width(), rect.height())),
    );
    let text_top = rect.center().y - label.size().y * 0.5;
    ui.painter().galley(
        pos2(rect.left() + mark.width() + 6.0, text_top),
        label,
        ui.visuals().weak_text_color(),
    );
}

fn forecast_card(ui: &mut Ui, name: &str, forecast: Option<Forecast>, target: u8, color: Color32) {
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
                        fit.eta(target)
                            .map_or_else(|| format!("Below {target}%"), platform::summary_date)
                    },
                ))
                .strong(),
            );
            if let Some(fit) = forecast {
                ui.add(
                    egui::Label::new(caption(format!(
                        "Fit from {} · {:.1} pp/day",
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

fn series_color(ui: &Ui, index: usize) -> Color32 {
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
