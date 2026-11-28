use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

use eframe::egui::{
    self, pos2, vec2, Align, Button, Color32, ComboBox, Frame, Layout, Rect, RichText, ScrollArea,
    Sense, Stroke, Ui, Widget,
};

use super::{
    super::model::{DashboardDevice, DashboardPoint, SECONDS_PER_DAY},
    accent,
    assets::Icon,
    canvas,
    chart::{self, ActivityHit, ActivityState, Forecast, Plot, Projection, Series, TimeRange},
    duration, live_device, live_percent, percent, platform, selected_inventory, Dashboard,
    Snapshot,
};

const ALL: u32 = u32::MAX;

pub(super) type Reading = (usize, usize);

struct SelectedReading {
    identity: String,
    point: DashboardPoint,
    // Distinguish identical samples within the timestamp without storing snapshot indices.
    occurrence: usize,
}

impl SelectedReading {
    fn new(device: &DashboardDevice, index: usize) -> Self {
        let point = device.points[index];
        let start = device.points.partition_point(|reading| reading.t < point.t);
        let occurrence = device.points[start..index]
            .iter()
            .filter(|reading| **reading == point)
            .count();
        Self {
            identity: device.identity.clone(),
            point,
            occurrence,
        }
    }

    fn resolve(&self, snapshot: &Snapshot) -> Option<Reading> {
        let (device, data) = snapshot
            .data
            .devices
            .iter()
            .enumerate()
            .find(|(_, device)| device.identity == self.identity)?;
        let start = data
            .points
            .partition_point(|reading| reading.t < self.point.t);
        let index = data.points[start..]
            .iter()
            .take_while(|reading| reading.t == self.point.t)
            .enumerate()
            .filter(|(_, reading)| **reading == self.point)
            .nth(self.occurrence)?
            .0;
        Some((device, start + index))
    }
}

pub(super) struct HistoryView {
    search: String,
    compared: BTreeSet<String>,
    forecasts: BTreeMap<String, [bool; 3]>,
    colors: BTreeMap<String, usize>,
    show_fit: bool,
    charging: bool,
    thresholds: bool,
    bridge_gaps: bool,
    expanded: bool,
    selecting_interval: bool,
    drag_start: Option<i64>,
    interval: Option<TimeRange>,
    readings_interval: Option<TimeRange>,
    jump_to_readings: bool,
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
    readings_focus: Option<usize>,
}

impl Default for HistoryView {
    fn default() -> Self {
        Self {
            search: String::new(),
            compared: BTreeSet::new(),
            forecasts: BTreeMap::new(),
            colors: BTreeMap::new(),
            show_fit: true,
            charging: true,
            thresholds: true,
            bridge_gaps: false,
            expanded: false,
            selecting_interval: false,
            drag_start: None,
            interval: None,
            readings_interval: None,
            jump_to_readings: false,
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
            readings_focus: None,
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
    charging: bool,
    discharging: bool,
    icon: Option<Icon>,
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
        let now =
            crate::history::unix_now().unwrap_or_else(|_| data.map_or(0, |data| data.generated_at));
        if let Some(expiry) = snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .charts
                .iter()
                .flat_map(|chart| chart.forecasts.into_iter().flatten())
                .filter_map(forecast_expiry)
                .filter(|&expiry| expiry > now)
                .min()
        }) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs((expiry - now) as u64));
        }
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
                charging: device
                    .reading
                    .map(|reading| reading.charging)
                    .or_else(|| Some(data?.devices[index?].charging))
                    .unwrap_or(false),
                discharging: index.is_some_and(|index| {
                    snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.charts[index].forecasts[..2]
                            .iter()
                            .flatten()
                            .any(|fit| forecast_available(*fit, now))
                    })
                }),
                icon: device_icon(&device.identity, &device.name),
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
                        charging: device.charging && device.online,
                        discharging: snapshot.as_ref().is_some_and(|snapshot| {
                            snapshot.charts[index].forecasts[..2]
                                .iter()
                                .flatten()
                                .any(|fit| forecast_available(*fit, now))
                        }),
                        icon: device_icon(&device.identity, &device.name),
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
                        charging: false,
                        discharging: false,
                        icon: device_icon(&item.identity, &item.name),
                        level: None,
                        level_label: percent(None),
                    });
                }
            }
        }

        rows.sort_by_key(|row| (row.index.is_none(), row.identity));
        rows.sort_by_key(|row| {
            self.analytics
                .colors
                .get(row.identity)
                .copied()
                .unwrap_or(usize::MAX)
        });
        let history = &mut self.analytics;
        let focus = &mut self.selected;
        let mut days = self.days;
        let export = if ui.available_width() < 800.0 {
            ScrollArea::vertical()
                .id_salt("analytics-compact")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    history.sidebar_panel(ui, &rows, focus, true);
                    ui.separator();
                    Frame::new()
                        .inner_margin(10)
                        .show(ui, |ui| {
                            history.content(
                                ui,
                                snapshot.as_deref(),
                                &rows,
                                focus,
                                &mut days,
                                now,
                                self.loading,
                                !self.exporting,
                            )
                        })
                        .inner
                })
                .inner
        } else {
            let height = ui.available_height();
            let width = ui.available_width();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.allocate_ui_with_layout(
                    vec2(218.0, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        history.sidebar_panel(ui, &rows, focus, false);
                    },
                );
                let (divider, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
                ui.painter().vline(
                    divider.center().x,
                    divider.y_range(),
                    ui.visuals().window_stroke,
                );
                ui.allocate_ui_with_layout(
                    vec2(width - 219.0, height),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ScrollArea::vertical()
                            .id_salt("analytics")
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                Frame::new()
                                    .inner_margin(15)
                                    .show(ui, |ui| {
                                        history.content(
                                            ui,
                                            snapshot.as_deref(),
                                            &rows,
                                            focus,
                                            &mut days,
                                            now,
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
            .inner
        };
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
        now: i64,
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
            rows.iter()
                .enumerate()
                .filter(|(_, row)| self.visible(row.identity))
                .find_map(|(index, row)| row.index.map(|device| (index, device)))
        }) else {
            ui.horizontal_wrapped(|ui| {
                ui.heading(focused.map_or("Battery history", |index| rows[index].name));
                range_picker(ui, days);
            });
            if let Some(index) = focused {
                let row = &rows[index];
                summary(
                    ui,
                    row,
                    self.series_color(ui, row.identity),
                    None,
                    None,
                    now,
                );
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
            color: self.series_color(ui, rows[row].identity),
        };
        let reference = series(reference_index, reference_row, ui);
        let focused_row = focused.unwrap_or(reference_row);
        let row = &rows[focused_row];
        ui.horizontal(|ui| {
            if let Some(icon) = row.icon {
                icon.show(ui, 29.0, ui.visuals().weak_text_color());
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(heading(ui, row.name, 24.0)).truncate());
                let details = if focus_index.is_some() {
                    format!(
                        "{} · Last reading {}",
                        source(row.identity),
                        platform::summary_date(reference.device.last_at)
                    )
                } else {
                    "No readings for the focused device in this range".to_owned()
                };
                ui.label(caption(details));
            });
        });
        let fits = focus_index.map(|_| {
            if row.charging {
                &reference.chart.forecasts[2..]
            } else {
                &reference.chart.forecasts[..2]
            }
        });
        let fit = fits
            .into_iter()
            .flatten()
            .flatten()
            .copied()
            .find(|fit| forecast_available(*fit, now));
        summary(
            ui,
            row,
            self.series_color(ui, row.identity),
            fit,
            focus_index.map(|_| reference.device.samples),
            now,
        );
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
        let visible = self.series(ui, snapshot, rows);
        let (range, hovered) = self.trend(ui, snapshot, reference, &visible, days, now);
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
        self.compared.extend(
            data.devices
                .iter()
                .filter(|device| Some(device.identity.as_str()) != focus)
                .take(if focus.is_some() { 2 } else { 3 })
                .map(|device| device.identity.clone()),
        );
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
                    color: self.series_color(ui, item.identity),
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
    pub(super) fn register_colors(
        &mut self,
        inventory: &[super::DashboardInventoryDevice],
        focus: Option<&str>,
    ) {
        if self.colors.is_empty() {
            self.colors
                .extend(focus.map(|identity| (identity.to_owned(), 0)));
        }
        for item in inventory {
            let slot = self.colors.len();
            self.colors.entry(item.identity.clone()).or_insert(slot);
        }
    }

    pub(super) fn series_color(&self, ui: &Ui, identity: &str) -> Color32 {
        let palette = if ui.visuals().dark_mode {
            [
                [57, 201, 237],
                [94, 214, 160],
                [144, 168, 255],
                [233, 155, 212],
                [255, 183, 124],
            ]
        } else {
            [
                [0, 103, 138],
                [17, 110, 77],
                [58, 79, 163],
                [147, 49, 122],
                [152, 74, 13],
            ]
        };
        let slot = self.colors.get(identity).copied().unwrap_or_default();
        let [r, g, b] = palette[slot % palette.len()];
        Color32::from_rgb(r, g, b)
    }

    fn sidebar_panel(
        &mut self,
        ui: &mut Ui,
        rows: &[DeviceRow<'_>],
        focus: &mut Option<String>,
        compact: bool,
    ) {
        let height = ui.available_height();
        Frame::new()
            .fill(sidebar_fill(ui))
            .inner_margin(egui::Margin::symmetric(12, 15))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(7.0, 7.0);
                if !compact {
                    ui.set_min_height((height - 30.0).max(0.0));
                }
                ui.horizontal(|ui| {
                    ui.label(heading(ui, "Devices", 16.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let count = rows
                            .iter()
                            .filter(|row| row.index.is_some() && self.visible(row.identity))
                            .count();
                        ui.label(caption(format!("{count} visible")));
                    });
                });
                if rows.len() > 8 {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Search devices")
                            .desired_width(ui.available_width()),
                    );
                }
                let query = self.search.trim().to_lowercase();
                let cards = |ui: &mut Ui| {
                    let filtered = rows.iter().filter(|row| {
                        query.is_empty()
                            || row.name.to_lowercase().contains(&query)
                            || row.identity.to_lowercase().contains(&query)
                    });
                    if compact {
                        let rows: Vec<_> = filtered.collect();
                        ui.columns(3, |columns| {
                            for (index, row) in rows.into_iter().enumerate() {
                                let column = &mut columns[index % 3];
                                self.device_card(column, row, focus, column.available_width());
                            }
                        });
                    } else {
                        for row in filtered {
                            self.device_card(ui, row, focus, ui.available_width());
                        }
                    }
                };
                ScrollArea::vertical()
                    .id_salt("analytics-devices")
                    .max_height(if compact {
                        122.0
                    } else {
                        (height - 120.0).max(80.0)
                    })
                    .show(ui, cards);
                ui.label(caption(
                    "Check to compare. Select a device to focus its overview.",
                ));
            });
    }

    fn device_card(
        &mut self,
        ui: &mut Ui,
        row: &DeviceRow<'_>,
        focus: &mut Option<String>,
        width: f32,
    ) {
        let active = focus.as_deref() == Some(row.identity);
        let mut visible = self.visible(row.identity);
        let color = self.series_color(ui, row.identity);
        let surface = ui.visuals().window_fill;
        let fill = if active {
            surface.lerp_to_gamma(accent(ui), 0.09)
        } else {
            surface
        };
        let border = if active {
            ui.visuals()
                .window_stroke
                .color
                .lerp_to_gamma(accent(ui), 0.32)
        } else {
            Color32::TRANSPARENT
        };
        let item = Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(8, 8))
            .show(ui, |ui| {
                let inner = (width - 18.0).max(90.0);
                ui.set_width(inner);
                ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                let toggle = ui
                    .horizontal(|ui| {
                        let name = format!("Show {} on the chart", row.name);
                        if layer_toggle(ui, &mut visible, &name, None, None, row.index.is_some())
                            .on_hover_text(name)
                            .changed()
                        {
                            self.set_visible(row.identity, visible);
                        }
                        if let Some(icon) = row.icon {
                            icon.show(ui, 18.0, ui.visuals().weak_text_color());
                        }
                        let level = ui.painter().layout_no_wrap(
                            row.level_label.clone(),
                            egui::FontId::proportional(12.0),
                            color,
                        );
                        let leading = if row.icon.is_some() { 47.0 } else { 24.0 };
                        let name_width = (inner - leading - level.size().x).max(12.0);
                        let label = ui
                            .allocate_ui_with_layout(
                                vec2(name_width, 26.0),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.set_width(name_width);
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.spacing_mut().interact_size.y = 14.0;
                                    let label = ui.add(
                                        egui::Label::new(heading(ui, row.name, 12.0))
                                            .truncate()
                                            .sense(Sense::click()),
                                    );
                                    label.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::Role::Button,
                                            true,
                                            active,
                                            row.name,
                                        )
                                    });
                                    state_label(ui, row);
                                    label.on_hover_text(row.name).clicked()
                                },
                            )
                            .inner;
                        ui.label(RichText::new(&row.level_label).color(color).size(12.0));
                        label
                    })
                    .inner;
                battery_bar(ui, inner, row.level, color);
                toggle
            });
        if active {
            let rect = item.response.rect.shrink2(vec2(1.0, 4.0));
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                2,
                accent(ui),
            );
        }
        if item.inner {
            *focus = Some(row.identity.to_owned());
            self.invalidate_readings();
        }
    }

    fn projections<'a>(&self, visible: &[Series<'a>], now: i64) -> Vec<Projection<'a>> {
        visible
            .iter()
            .flat_map(|&series| {
                let enabled = self
                    .forecasts
                    .get(&series.device.identity)
                    .copied()
                    .unwrap_or_default();
                series
                    .chart
                    .forecasts
                    .into_iter()
                    .enumerate()
                    .filter_map(move |(model, fit)| {
                        let fit = fit?;
                        (enabled[model] && forecast_available(fit, now)).then_some(Projection {
                            series,
                            fit,
                            model,
                        })
                    })
            })
            .collect()
    }

    fn trend<'a>(
        &mut self,
        ui: &mut Ui,
        snapshot: &'a Snapshot,
        focused: Series<'a>,
        visible: &[Series<'a>],
        days: &mut u32,
        now: i64,
    ) -> (TimeRange, Option<i64>) {
        let forecasts = self.projections(visible, now);
        let outlook_width = 230.0;
        let side = !forecasts.is_empty() && ui.available_width() >= 925.0;
        let width = ui.available_width();
        let chart_width = if side {
            width - outlook_width - 10.0
        } else {
            width
        };
        let (result, active) = ui
            .horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (result, active) = ui
                    .allocate_ui_with_layout(
                        vec2(chart_width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            panel(ui, 12, |ui| {
                                self.trend_panel(ui, snapshot, focused, visible, days, now)
                            })
                        },
                    )
                    .inner;
                if side && !active.is_empty() {
                    ui.allocate_ui_with_layout(
                        vec2(outlook_width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| outlook(ui, &active, now),
                    );
                }
                (result, active)
            })
            .inner;
        if !side && !active.is_empty() {
            outlook(ui, &active, now);
        }
        result
    }

    fn trend_panel<'a>(
        &mut self,
        ui: &mut Ui,
        snapshot: &'a Snapshot,
        focused: Series<'a>,
        visible: &[Series<'a>],
        days: &mut u32,
        now: i64,
    ) -> ((TimeRange, Option<i64>), Vec<Projection<'a>>) {
        ui.spacing_mut().item_spacing.y = 6.0;
        let active = self.projections(visible, now).len();
        ui.horizontal(|ui| {
            ui.label(heading(ui, "Battery trend", 16.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("View", |ui| {
                    if ui
                        .add_enabled(self.zoom.is_some(), Button::new("Reset view"))
                        .clicked()
                    {
                        self.reset_view();
                        ui.close();
                    }
                    ui.checkbox(&mut self.expanded, "Expand chart");
                    ui.checkbox(&mut self.selecting_interval, "Select interval");
                    ui.checkbox(&mut self.bridge_gaps, "Bridge gaps")
                        .on_hover_text("Dashed links mark unobserved intervals.");
                });
                ComboBox::from_id_salt("analytics-forecast")
                    .width(106.0)
                    .height(410.0)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .selected_text(if active == 0 {
                        "Forecast · Off".to_owned()
                    } else {
                        format!("Forecast · {active} on")
                    })
                    .show_ui(ui, |ui| {
                        ui.set_min_width(280.0);
                        ui.spacing_mut().interact_size.y = 19.0;
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.horizontal(|ui| {
                            ui.label(heading(ui, "Forecasts", 13.0));
                            if ui.button("Hide all").clicked() {
                                self.forecasts.clear();
                            }
                        });
                        ui.label(caption("Models for visible devices. Off by default."));
                        ui.checkbox(&mut self.show_fit, "Historical calculation lines");
                        for &series in visible {
                            ui.separator();
                            ui.label(heading(ui, &series.device.name, 13.0).color(series.color));
                            let mut has_model = false;
                            for (model, fit) in series.chart.forecasts.into_iter().enumerate() {
                                if let Some(fit) = fit.filter(|fit| forecast_available(*fit, now)) {
                                    has_model = true;
                                    let enabled = self
                                        .forecasts
                                        .entry(series.device.identity.clone())
                                        .or_default();
                                    let projection = Projection { series, fit, model };
                                    ui.checkbox(
                                        &mut enabled[model],
                                        RichText::new(projection.name())
                                            .color(projection.color(ui)),
                                    );
                                    ui.horizontal(|ui| {
                                        ui.add_space(19.0);
                                        ui.label(caption(format!(
                                            "{} · From {}",
                                            fit.rate_label(),
                                            platform::summary_date(fit.start.t)
                                        )));
                                    });
                                }
                            }
                            if !has_model {
                                ui.label(caption("No reliable current trend"));
                            }
                        }
                    });
                range_picker(ui, days);
            });
        });
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 17.0;
            ui.spacing_mut().item_spacing = vec2(8.0, 3.0);
            ui.horizontal_wrapped(|ui| {
                for series in visible {
                    let mark = LegendMark::Line(series.color);
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(mark.width(), 12.0), Sense::hover());
                    mark.paint(ui, rect);
                    ui.label(caption(&series.device.name));
                }
                let muted = ui.visuals().weak_text_color();
                layer_toggle(
                    ui,
                    &mut self.charging,
                    "Charging overlay",
                    Some("Charging"),
                    Some(LegendMark::Bolt),
                    true,
                );
                layer_toggle(
                    ui,
                    &mut self.thresholds,
                    "Alert thresholds",
                    Some("Thresholds"),
                    Some(LegendMark::Dashed(muted)),
                    true,
                );
                if self.show_fit && active > 0 {
                    ui.label(caption("Historical fit"));
                }
            });
        });
        let forecasts = self.projections(visible, now);
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
            .filter_map(|projection| projection.fit.eta())
            .max()
            .unwrap_or(generated);
        let end = if projected > generated {
            projected.saturating_add(((projected - generated) / 20).max(300))
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
        if self.selecting_interval {
            ui.label(caption("Drag to select a range"));
        }
        let plot = Plot {
            series: visible,
            focused,
            forecasts: &forecasts,
            show_fit: self.show_fit,
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
        ((range, hovered), forecasts)
    }

    #[allow(clippy::too_many_arguments)]
    fn inspector(
        &mut self,
        ui: &mut Ui,
        focused: Series<'_>,
        visible: &[Series<'_>],
        at: Option<i64>,
        forecasts: &[Projection<'_>],
        range: TimeRange,
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
                    if self.pinned.is_some() && ui.small_button("Unpin").clicked() {
                        self.pinned = None;
                        self.pinned_activity = None;
                    }
                });
                if at.is_none() {
                    ui.label(caption(
                        "Hover over the chart or activity to inspect. Click to pin.",
                    ));
                    return;
                }
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
                    ui.add(egui::Label::new(caption(details)).truncate());
                    if let Some(session) = session {
                        if ui.button("View session readings").clicked() {
                            self.show_interval_readings(session);
                        }
                    }
                });
                if let Some(at) = at {
                    for projection in forecasts {
                        if let Some(label) = projection.details(at, self.show_fit) {
                            ui.add(egui::Label::new(caption(&label)).truncate())
                                .on_hover_text(label);
                        }
                    }
                }
                ScrollArea::vertical()
                    .id_salt("time-inspector")
                    .max_height(50.0)
                    .min_scrolled_height(22.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
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
                        });
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
        let selected_row = self
            .selected_reading
            .as_ref()
            .and_then(|row| row.resolve(snapshot));
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
                            ui.label(heading(ui, "Readings", 13.0));
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
                            if ui
                                .add_enabled(selected_row.is_some(), Button::new("Copy reading"))
                                .clicked()
                            {
                                if let Some((device, point)) = selected_row {
                                    let device = &snapshot.data.devices[device];
                                    let point = device.points[point];
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
                let readings_focus = focused.filter(|_| filter == ReadingsFilter::Focused);
                if filter != self.filter || readings_focus != self.readings_focus {
                    self.readings_focus = readings_focus;
                    self.filter = filter;
                    self.invalidate_readings();
                }
                let rows = self.readings.get_or_insert_with(|| {
                    let mut rows = Vec::new();
                    let devices = if filter == ReadingsFilter::Focused {
                        &[]
                    } else {
                        visible
                    };
                    for device in devices
                        .iter()
                        .map(|series| series.index)
                        .chain(readings_focus)
                    {
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
                        .max_height(175.0)
                        .min_scrolled_height(216.0)
                        .auto_shrink([false, true]);
                    let selected_index = |selected: Reading| {
                        slice
                            .binary_search_by_key(&(time(&selected), selected), |row| {
                                (time(row), *row)
                            })
                            .ok()
                    };
                    let mut pinned_selection = selected_row
                        .filter(|row| self.pinned == Some(time(row)))
                        .and_then(selected_index);
                    let nearest = pinned_selection.or_else(|| {
                        cursor
                            .filter(|at| {
                                slice
                                    .first()
                                    .zip(slice.last())
                                    .is_some_and(|(first, last)| {
                                        (time(first)..=time(last)).contains(at)
                                    })
                            })
                            .and_then(|at| {
                                let after = slice
                                    .partition_point(|row| time(row) < at)
                                    .min(slice.len() - 1);
                                let before = after.saturating_sub(1);
                                let nearest = if time(&slice[before]).abs_diff(at)
                                    < time(&slice[after]).abs_diff(at)
                                {
                                    before
                                } else {
                                    after
                                };
                                (time(&slice[nearest]).abs_diff(at)
                                    <= (super::super::model::OFFLINE_AFTER_SECONDS / 2) as u64)
                                    .then_some(nearest)
                            })
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
                        let current = selected_row
                            .and_then(|selected| displayed.iter().position(|row| *row == selected))
                            .map(display_index);
                        let index = current.map_or(0, |i| {
                            i.saturating_add_signed(step as isize)
                                .min(displayed.len() - 1)
                        });
                        let row = displayed[display_index(index)];
                        pinned_selection = selected_index(row);
                        self.selected_reading =
                            Some(SelectedReading::new(&snapshot.data.devices[row.0], row.1));
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
                                pinned_selection = selected_index(row);
                                self.selected_reading =
                                    Some(SelectedReading::new(device_data, row.1));
                                self.pinned_activity = None;
                                self.pinned = Some(point.t);
                            }
                            table_row(
                                ui,
                                rect,
                                response.hovered()
                                    || pinned_selection
                                        .or(nearest)
                                        .is_some_and(|i| slice[i] == row),
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

pub(super) fn table_row(ui: &Ui, rect: Rect, highlighted: bool) {
    if highlighted {
        let fill = ui.visuals().selection.bg_fill;
        if fill == accent(ui) {
            ui.painter()
                .rect_stroke(rect, 0, Stroke::new(1.0, fill), egui::StrokeKind::Inside);
        } else {
            ui.painter().rect_filled(rect, 0, fill);
        }
    }
    ui.painter()
        .hline(rect.x_range(), rect.bottom(), ui.visuals().window_stroke);
}

pub(super) fn cell_ui(ui: &mut Ui, rect: Rect) -> Ui {
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
    Bolt,
    Line(Color32),
    Dashed(Color32),
}

impl LegendMark {
    fn width(self) -> f32 {
        match self {
            Self::Bolt => 14.0,
            Self::Line(_) | Self::Dashed(_) => 23.0,
        }
    }

    fn paint(self, ui: &Ui, rect: Rect) {
        let y = rect.center().y;
        match self {
            Self::Bolt => Icon::Bolt.paint(ui.painter(), rect.center(), 14.0, charging_color(ui)),
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

fn panel<R>(ui: &mut Ui, padding: i8, contents: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(ui.visuals().window_stroke)
        .corner_radius(7)
        .inner_margin(padding)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui)
        })
        .inner
}

fn forecast_expiry(fit: Forecast) -> Option<i64> {
    Some(
        fit.eta()?.min(
            fit.latest
                .t
                .saturating_add(super::super::model::OFFLINE_AFTER_SECONDS + 1),
        ),
    )
}

fn forecast_available(fit: Forecast, now: i64) -> bool {
    fit.latest.t <= now && forecast_expiry(fit).is_some_and(|expiry| expiry > now)
}

pub(super) fn charging_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        chart::CHARGING
    } else {
        Color32::from_rgb(122, 85, 0)
    }
}

pub(super) fn device_icon(identity: &str, name: &str) -> Option<Icon> {
    let name = name.to_ascii_lowercase();
    if name.contains("mouse") || name.contains("g502") {
        Some(Icon::Mouse)
    } else if name.contains("headset") || name.contains("headphone") {
        Some(Icon::Headset)
    } else if identity.starts_with("xinput:")
        || name.contains("controller")
        || name.contains("gamepad")
    {
        Some(Icon::Controller)
    } else {
        None
    }
}

fn state_label(ui: &mut Ui, row: &DeviceRow<'_>) {
    if row.charging {
        let color = charging_color(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            Icon::Bolt.show(ui, 12.0, color);
            ui.label(caption("Charging").size(10.0).color(color));
        });
    } else {
        ui.label(
            caption(if row.discharging {
                "Discharging"
            } else if row.level.is_some() {
                "No current trend"
            } else {
                source(row.identity)
            })
            .size(10.0),
        );
    }
}

fn summary(
    ui: &mut Ui,
    row: &DeviceRow<'_>,
    color: Color32,
    fit: Option<Forecast>,
    readings: Option<u64>,
    now: i64,
) {
    panel(ui, 10, |ui| {
        let eta = fit
            .and_then(Forecast::eta)
            .map_or_else(|| "—".into(), |end| duration(end - now));
        let (eta_title, rate_title) = if row.charging {
            ("Time until full", "Charging rate")
        } else {
            ("Time remaining", "Discharge rate")
        };
        let basis = fit.map_or("No reliable trend", |fit| {
            if fit.latest.c {
                "Session estimate · 100%"
            } else {
                "Discharge estimate · 0%"
            }
        });
        let rate = fit.map_or_else(
            || "—".into(),
            |fit| {
                format!(
                    "{:.1}",
                    fit.rate.abs() / if row.charging { 24.0 } else { 1.0 }
                )
            },
        );
        let count = readings.map_or_else(|| "—".into(), |count| count.to_string());
        let cells = if ui.available_width() < 470.0 || ui.ctx().content_rect().width() < 800.0 {
            2
        } else {
            4
        };
        let width = (ui.available_width() - (cells - 1) as f32 * 9.0) / cells as f32;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(9.0, 9.0);
            for (index, (title, value, detail)) in [
                ("Battery level", "", ""),
                (eta_title, eta.as_str(), basis),
                (
                    rate_title,
                    rate.as_str(),
                    if row.charging {
                        "Percentage points / hour"
                    } else {
                        "Percentage points / day"
                    },
                ),
                ("Readings", count.as_str(), "Focused source"),
            ]
            .into_iter()
            .enumerate()
            {
                let column = ui.allocate_ui_with_layout(
                    vec2(width, 58.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        if index == 0 {
                            ui.horizontal(|ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                                let center = rect.center();
                                let radius = 23.0;
                                ui.painter().circle_stroke(
                                    center,
                                    radius,
                                    Stroke::new(4.0, ui.visuals().window_stroke.color),
                                );
                                if let Some(level) = row.level.filter(|&level| level > 0) {
                                    let steps = usize::from(level).div_ceil(2);
                                    let arc = (0..=steps)
                                        .map(|step| {
                                            let turn = step as f32 / steps as f32
                                                * f32::from(level)
                                                / 100.0;
                                            let angle = turn * std::f32::consts::TAU
                                                - std::f32::consts::FRAC_PI_2;
                                            center + radius * vec2(angle.cos(), angle.sin())
                                        })
                                        .collect();
                                    let ring_color = if row.charging {
                                        charging_color(ui)
                                    } else {
                                        color
                                    };
                                    ui.painter()
                                        .add(egui::Shape::line(arc, Stroke::new(4.0, ring_color)));
                                }
                                ui.painter().text(
                                    center,
                                    egui::Align2::CENTER_CENTER,
                                    &row.level_label,
                                    egui::FontId::proportional(14.0),
                                    ui.visuals().text_color(),
                                );
                                ui.vertical(|ui| {
                                    ui.add(egui::Label::new(caption(title)).truncate());
                                    state_label(ui, row);
                                });
                            });
                        } else {
                            ui.add(egui::Label::new(caption(title)).truncate());
                            ui.add(egui::Label::new(heading(ui, value, 24.0)).truncate());
                            ui.add(egui::Label::new(caption(detail)).truncate())
                                .on_hover_text(detail);
                        }
                    },
                );
                if index % cells != 0 {
                    let rect = column.response.rect;
                    ui.painter().vline(
                        rect.left() - 4.0,
                        rect.y_range().shrink(3.0),
                        ui.visuals().window_stroke,
                    );
                }
            }
        });
    });
}

fn outlook(ui: &mut Ui, forecasts: &[Projection<'_>], now: i64) {
    panel(ui, 12, |ui| {
        ui.label(heading(ui, "Forecast outlook", 16.0));
        ui.label(caption("Automatic endpoint · per source"));
        let row = |ui: &mut Ui, projection: Projection<'_>| {
            let fit = projection.fit;
            let Some(end) = fit.eta() else { return };
            let color = projection.color(ui);
            ui.separator();
            ui.label(
                heading(ui, &projection.series.device.name, 13.0).color(projection.series.color),
            );
            ui.label(caption(projection.name()).color(color));
            ui.label(heading(ui, duration(end - now), 21.0).color(color));
            let endpoint = if fit.latest.c { "Full" } else { "Empty" };
            ui.label(caption(format!(
                "{endpoint} · {}",
                platform::summary_date(end)
            )));
            ui.label(caption(format!(
                "{} · from {}",
                fit.rate_label(),
                platform::summary_date(fit.start.t)
            )));
        };
        if ui.available_width() >= 440.0 && forecasts.len() > 1 {
            ui.columns(2, |columns| {
                for (index, &projection) in forecasts.iter().enumerate() {
                    row(&mut columns[index % 2], projection);
                }
            });
        } else {
            for &projection in forecasts {
                row(ui, projection);
            }
        }
        ui.label(caption(
            "Estimates change with usage. Charging may slow near full.",
        ));
    });
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
                let mut choices = [(1, "24h"), (7, "7d"), (30, "30d"), (ALL, "All")];
                if ui.layout().main_dir == egui::Direction::RightToLeft {
                    choices.reverse();
                }
                for (value, label) in choices {
                    if ui
                        .add(
                            Button::new(label)
                                .frame_when_inactive(*days == value)
                                .selected(*days == value)
                                .min_size(vec2(28.0, 23.0)),
                        )
                        .clicked()
                    {
                        *days = value;
                    }
                }
            });
        });
}

pub(super) fn battery_bar(ui: &mut Ui, width: f32, level: Option<u8>, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
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

pub(super) fn heading(ui: &Ui, text: impl Into<String>, size: f32) -> RichText {
    let mut font = egui::TextStyle::Heading.resolve(ui.style());
    font.size = size;
    RichText::new(text.into()).font(font).strong()
}

fn caption(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(11.0).weak()
}

fn source(identity: &str) -> &'static str {
    match identity.split(':').next() {
        Some("bluetooth") => "Bluetooth",
        Some("hid") => "HID",
        Some("xinput") => "XInput",
        _ => "Battery monitor",
    }
}

fn sidebar_fill(ui: &Ui) -> Color32 {
    canvas(ui).lerp_to_gamma(ui.visuals().window_fill, 0.42)
}
