use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Painter, Rect, Sense, Shape, Stroke, Ui,
};

use super::{
    super::model::{DashboardDevice, DashboardPoint, OFFLINE_AFTER_SECONDS, SECONDS_PER_DAY},
    duration, percent, platform,
};

const COLUMNS: usize = 640;
static NEXT_CHART_ID: AtomicU64 = AtomicU64::new(0);
pub(super) const CHARGING: Color32 = Color32::from_rgb(250, 204, 21);
pub(super) const FORECAST_NAMES: [&str; 2] = ["Since last charge", "Current discharge rate"];

pub(super) struct Chart {
    revision: u64,
    identity: Arc<str>,
    points: Vec<DashboardPoint>,
    bridges: Vec<(DashboardPoint, DashboardPoint)>,
    charging: Vec<(i64, i64)>,
    drain_boundaries: Vec<i64>,
    online_prefix: Vec<i64>,
    pub(super) forecasts: [Option<Forecast>; 2],
}

#[derive(Clone, Copy)]
pub(super) struct Forecast {
    pub(super) start: DashboardPoint,
    pub(super) latest: DashboardPoint,
    pub(super) rate: f64,
}

impl Forecast {
    fn new(start: DashboardPoint, latest: DashboardPoint) -> Option<Self> {
        let hours = latest.t.saturating_sub(start.t);
        let drop = start.p?.checked_sub(latest.p?)?;
        (hours >= 3_600 && drop > 0).then(|| Self {
            start,
            latest,
            rate: f64::from(drop) * SECONDS_PER_DAY as f64 / hours as f64,
        })
    }

    pub(super) fn eta(self, target: u8) -> Option<i64> {
        let drop = self.latest.p?.checked_sub(target)?;
        Some(
            self.latest.t.saturating_add(
                (f64::from(drop) * SECONDS_PER_DAY as f64 / self.rate).round() as i64,
            ),
        )
    }
}

impl Chart {
    pub(super) fn new(device: &DashboardDevice) -> Self {
        let mut points = Vec::new();
        let start = device.first_at;
        let end = device.last_at;

        // Downsample within a contiguous observed run. Combining runs inside a
        // bucket would draw a recorded line through an unobserved interval.
        let mut previous_end = None;
        let mut index = 0;
        while index < device.points.len() {
            let first = device.points[index];
            if first.p.is_none() {
                index += 1;
                continue;
            }
            let column = bucket(start, end, first.t);
            let break_before = previous_end.is_none_or(|(previous_index, previous_time)| {
                index != previous_index + 1 || first.t - previous_time > OFFLINE_AFTER_SECONDS
            });
            let mut next = index + 1;
            while let Some(point) = device.points.get(next) {
                if point.p.is_none()
                    || point.t - device.points[next - 1].t > OFFLINE_AFTER_SECONDS
                    || bucket(start, end, point.t) != column
                {
                    break;
                }
                next += 1;
            }
            let group = &device.points[index..next];
            let min = (0..group.len()).min_by_key(|&i| group[i].p).unwrap_or(0);
            let max = (0..group.len()).max_by_key(|&i| group[i].p).unwrap_or(0);
            let mut indices = [0, min, max, group.len() - 1];
            indices.sort_unstable();
            if break_before && !points.is_empty() {
                points.push(DashboardPoint { p: None, ..first });
            }
            points.extend(indices.into_iter().enumerate().filter_map(|(i, sample)| {
                (i == 0 || indices[i - 1] != sample).then_some(group[sample])
            }));
            previous_end = Some((next - 1, group[group.len() - 1].t));
            index = next;
        }
        let charging = device
            .points
            .iter()
            .enumerate()
            .filter(|(_, point)| point.c)
            .fold(Vec::<(i64, i64)>::new(), |mut intervals, (index, point)| {
                let end = device
                    .points
                    .get(index + 1)
                    .map_or(point.t, |next| next.t)
                    .min(point.t.saturating_add(OFFLINE_AFTER_SECONDS));
                match intervals.last_mut() {
                    Some((_, previous_end)) if *previous_end == point.t => *previous_end = end,
                    _ => intervals.push((point.t, end)),
                }
                intervals
            });
        let mut forecasts = [None; 2];
        if let Some((index, &latest)) = device
            .points
            .iter()
            .enumerate()
            .rfind(|(_, p)| p.p.is_some())
        {
            let eligible = &device.points[..=index];
            let charging_now = device.points[index..].iter().any(|point| point.c);
            let since_charge = device
                .points
                .iter()
                .rposition(|p| p.c)
                .filter(|&charge| charge < index)
                .map(|charge| &eligible[charge + 1..]);
            let discharge = since_charge.unwrap_or(eligible);
            let discontinuity = discharge.windows(2).rposition(|pair| {
                pair[0]
                    .p
                    .zip(pair[1].p)
                    .is_none_or(|(left, right)| right > left)
            });
            if since_charge.is_some() && discontinuity.is_none() {
                forecasts[0] = discharge
                    .first()
                    .and_then(|&start| Forecast::new(start, latest));
            }
            let discharge = discontinuity.map_or(discharge, |index| &discharge[index + 1..]);
            let cutoff = latest.t.saturating_sub(SECONDS_PER_DAY);
            let recent = discharge.partition_point(|p| p.t < cutoff);
            if let Some(first) = discharge[recent..].iter().find(|p| p.p.is_some()) {
                forecasts[1] = Forecast::new(*first, latest);
            }
            if charging_now {
                forecasts = [None; 2];
            }
        }
        let mut online_prefix = vec![0];
        for session in &device.sessions {
            let total = online_prefix.last().copied().unwrap_or_default();
            online_prefix.push(
                total
                    + if session.online {
                        session.end - session.start
                    } else {
                        0
                    },
            );
        }
        Self {
            revision: NEXT_CHART_ID.fetch_add(1, Ordering::Relaxed),
            identity: Arc::from(device.identity.as_str()),
            points,
            drain_boundaries: device
                .points
                .windows(2)
                .filter_map(|pair| {
                    let [left, right] = [pair[0], pair[1]];
                    (left.c
                        || right.c
                        || left.p.is_none()
                        || right.p.is_none()
                        || left.p < right.p
                        || right.t - left.t > OFFLINE_AFTER_SECONDS)
                        .then_some(right.t)
                })
                .collect(),
            online_prefix,
            bridges: device
                .points
                .windows(2)
                .filter_map(|pair| {
                    let [left, right] = [pair[0], pair[1]];
                    (left.p.is_some()
                        && right.p.is_some()
                        && !left.c
                        && !right.c
                        && right.t - left.t > OFFLINE_AFTER_SECONDS
                        && left.p >= right.p)
                        .then_some((left, right))
                })
                .collect(),
            charging,
            forecasts,
        }
    }

    pub(super) fn continuous_discharge(&self, range: TimeRange) -> bool {
        let index = self
            .drain_boundaries
            .partition_point(|&at| at <= range.start);
        self.drain_boundaries
            .get(index)
            .is_none_or(|&at| at > range.end)
    }

    pub(super) fn online_time(&self, device: &DashboardDevice, range: TimeRange) -> Option<i64> {
        let first = device
            .sessions
            .partition_point(|session| session.end <= range.start);
        if device
            .sessions
            .get(first)
            .is_none_or(|session| session.start >= range.end)
        {
            return None;
        }
        let elapsed = |at| {
            let index = device.sessions.partition_point(|session| session.end <= at);
            self.online_prefix[index]
                + device
                    .sessions
                    .get(index)
                    .filter(|session| session.online)
                    .map_or(0, |session| (at - session.start).max(0))
        };
        Some(elapsed(range.end) - elapsed(range.start))
    }

    pub(super) fn charging_at(&self, at: i64) -> Option<TimeRange> {
        let index = self.charging.partition_point(|&(_, end)| end < at);
        self.charging
            .get(index)
            .filter(|&&(start, _)| start <= at)
            .map(|&(start, end)| TimeRange { start, end })
    }
}

#[derive(Clone, PartialEq)]
struct TraceKey {
    revision: u64,
    range: TimeRange,
    plot: Rect,
    color: Color32,
    focused: bool,
}

#[derive(Clone)]
struct TraceGeometry {
    key: TraceKey,
    shapes: Arc<[Shape]>,
}

fn recorded_geometry(chart: &Chart, key: &TraceKey) -> Arc<[Shape]> {
    let x = |t| key.range.x(key.plot, t);
    let y = |level: u8| key.plot.bottom() - f32::from(level.min(100)) / 100.0 * key.plot.height();
    let width = if key.focused { 2.1 } else { 1.6 };
    let samples = &chart.points;
    let first = samples
        .partition_point(|point| point.t < key.range.start)
        .saturating_sub(1);
    let last = (samples.partition_point(|point| point.t <= key.range.end) + 1).min(samples.len());
    let mut shapes = Vec::new();
    for segment in samples[first..last].split(|point| point.p.is_none()) {
        let path: Vec<_> = segment
            .iter()
            .filter_map(|point| point.p.map(|level| pos2(x(point.t), y(level))))
            .collect();
        shapes.extend(
            path.iter()
                .filter(|&&point| key.plot.contains(point))
                .map(|&point| Shape::circle_filled(point, 1.7, key.color)),
        );
        if path.len() > 1 {
            shapes.push(Shape::line(path, Stroke::new(width, key.color)));
        }
    }
    shapes.into()
}

#[derive(Clone, Copy)]
pub(super) struct Series<'a> {
    pub(super) index: usize,
    pub(super) device: &'a DashboardDevice,
    pub(super) chart: &'a Chart,
    pub(super) color: Color32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct TimeRange {
    pub(super) start: i64,
    pub(super) end: i64,
}

impl TimeRange {
    pub(super) fn points(self, points: &[DashboardPoint]) -> &[DashboardPoint] {
        let start = points.partition_point(|point| point.t < self.start);
        let end = points.partition_point(|point| point.t <= self.end);
        &points[start..end]
    }

    pub(super) fn ordered(a: i64, b: i64) -> Self {
        Self {
            start: a.min(b),
            end: a.max(b),
        }
    }

    fn x(self, rect: Rect, time: i64) -> f32 {
        rect.left() + fraction(self.start, self.end, time) as f32 * rect.width()
    }

    fn at(self, rect: Rect, x: f32) -> i64 {
        self.start
            + (f64::from((x - rect.left()) / rect.width()) * (self.end - self.start) as f64).round()
                as i64
    }

    fn zoom(self, full: Self, anchor: i64, scroll: f32, pan: bool) -> Self {
        let span = self.end - self.start;
        let max_span = full.end - full.start;
        let (span, start) = if pan {
            (
                span,
                self.start - (scroll as f64 * span as f64 / 700.0) as i64,
            )
        } else {
            let new_span = (span as f64 * (-f64::from(scroll) / 400.0).exp()) as i64;
            let new_span = new_span.clamp(max_span.min(900), max_span);
            let offset = fraction(self.start, self.end, anchor) * new_span as f64;
            (new_span, anchor - offset as i64)
        };
        let start = start.clamp(full.start, full.end - span);
        Self {
            start,
            end: start + span,
        }
    }
}

pub(super) fn gutter(width: f32) -> f32 {
    if width >= 750.0 {
        128.0
    } else {
        84.0
    }
}

pub(super) fn forecast_colors(ui: &Ui) -> [Color32; 2] {
    if ui.visuals().dark_mode {
        [
            Color32::from_rgb(199, 164, 246),
            Color32::from_rgb(255, 155, 132),
        ]
    } else {
        [
            Color32::from_rgb(101, 57, 164),
            Color32::from_rgb(160, 61, 45),
        ]
    }
}

pub(super) struct Plot<'a> {
    pub(super) series: &'a [Series<'a>],
    pub(super) focused: Series<'a>,
    pub(super) forecasts: [Option<Forecast>; 2],
    pub(super) target: u8,
    pub(super) thresholds: &'a [u8],
    pub(super) show_charging: bool,
    pub(super) bridge_gaps: bool,
    pub(super) expanded: bool,
    pub(super) recorded_end: i64,
    pub(super) full: TimeRange,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ActivityHit {
    pub(super) at: i64,
    pub(super) identity: Arc<str>,
    pub(super) interval: TimeRange,
    pub(super) state: ActivityState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivityState {
    Online,
    Offline,
    Charging,
    Unknown,
}

impl ActivityState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Online => "Online (inferred)",
            Self::Offline => "Offline (inferred)",
            Self::Charging => "Charging (observed samples)",
            Self::Unknown => "Unknown",
        }
    }
}

impl Plot<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn trend(
        &self,
        ui: &mut Ui,
        range: &mut TimeRange,
        cursor: &mut Option<i64>,
        external_cursor: Option<i64>,
        selection: &mut Option<TimeRange>,
        drag_start: &mut Option<i64>,
        select_interval: bool,
        activity: Option<&ActivityHit>,
    ) -> Option<i64> {
        let height = if self.expanded {
            (ui.ctx().content_rect().height() - 130.0).max(360.0)
        } else {
            let available = ui.ctx().content_rect().height();
            (available * 0.48).clamp(if available < 650.0 { 270.0 } else { 330.0 }, 440.0)
        };
        let (area, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), height),
            if select_interval {
                Sense::click_and_drag()
            } else {
                Sense::click()
            },
        );
        if !ui.is_rect_visible(area) {
            return None;
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Group,
                true,
                "Battery trend. Use left and right arrow keys to inspect readings.",
            )
        });
        let plot = Rect::from_min_max(
            area.min + vec2(gutter(area.width()), 20.0),
            area.max - vec2(42.0, 30.0),
        );
        let hover = response.hover_pos().filter(|pos| plot.contains(*pos));
        if let Some(pos) = hover {
            let (scroll, pan) = ui.input(|input| {
                let pan = input.modifiers.shift;
                let zoom = input.modifiers.ctrl || input.modifiers.command;
                let scroll = if zoom || pan {
                    input.smooth_scroll_delta.y + input.smooth_scroll_delta.x
                } else {
                    0.0
                };
                (scroll, pan)
            });
            if scroll != 0.0 {
                *range = range.zoom(self.full, range.at(plot, pos.x), scroll, pan);
                ui.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        let hovered_at = hover.map(|pos| range.at(plot, pos.x));
        if select_interval {
            if response.drag_started() {
                *drag_start = ui
                    .input(|input| input.pointer.press_origin())
                    .filter(|pos| plot.contains(*pos))
                    .map(|pos| {
                        if let Some(interval) = *selection {
                            if (range.x(plot, interval.start) - pos.x).abs() <= 7.0 {
                                return interval.end;
                            }
                            if (range.x(plot, interval.end) - pos.x).abs() <= 7.0 {
                                return interval.start;
                            }
                        }
                        range.at(plot, pos.x).min(self.recorded_end)
                    });
            }
            if let Some(start) = *drag_start {
                if let Some(pos) = response.interact_pointer_pos() {
                    let end = range
                        .at(plot, pos.x.clamp(plot.left(), plot.right()))
                        .min(self.recorded_end);
                    *selection = (start != end).then(|| TimeRange::ordered(start, end));
                }
            }
            if response.drag_stopped() {
                *drag_start = None;
            }
        }
        if response.clicked() {
            if let Some(at) = response
                .interact_pointer_pos()
                .filter(|pos| plot.contains(*pos))
                .map(|pos| range.at(plot, pos.x))
            {
                *cursor = (*cursor != Some(at)).then_some(at);
            }
        }
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            *cursor = None;
        }

        let painter = ui.painter();
        let clipped = painter.with_clip_rect(plot.intersect(ui.clip_rect()));
        let label = ui.visuals().weak_text_color();
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        ..egui::EventFilter::default()
                    },
                )
            });
            painter.rect_stroke(
                plot,
                0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
            let step = ui.input(|input| {
                i64::from(input.key_pressed(egui::Key::ArrowRight))
                    - i64::from(input.key_pressed(egui::Key::ArrowLeft))
            });
            if step != 0 {
                let at = cursor.unwrap_or(range.start.saturating_sub(1));
                let points = &self.focused.device.points;
                let next = if step > 0 {
                    points.get(points.partition_point(|point| point.t <= at))
                } else {
                    points
                        .partition_point(|point| point.t < at)
                        .checked_sub(1)
                        .and_then(|index| points.get(index))
                };
                if let Some(point) =
                    next.filter(|point| (range.start..=range.end).contains(&point.t))
                {
                    if select_interval && ui.input(|input| input.modifiers.shift) {
                        let anchor = selection.map_or(at.max(range.start), |interval| {
                            if interval.start == at {
                                interval.end
                            } else {
                                interval.start
                            }
                        });
                        *selection =
                            (anchor != point.t).then(|| TimeRange::ordered(anchor, point.t));
                    }
                    *cursor = Some(point.t);
                }
            }
        }
        let rule = ui.visuals().window_stroke;
        let x = |time| range.x(plot, time);
        let y = |level: u8| plot.bottom() - f32::from(level.min(100)) / 100.0 * plot.height();
        let future = x(self.recorded_end).max(plot.left());
        if future < plot.right() && self.recorded_end < self.full.end {
            clipped.rect_filled(
                Rect::from_min_max(pos2(future, plot.top()), plot.right_bottom()),
                0,
                ui.visuals().widgets.inactive.weak_bg_fill,
            );
            painter.text(
                pos2(future + 8.0, plot.top() - 3.0),
                Align2::LEFT_BOTTOM,
                "FORECAST",
                FontId::proportional(10.0),
                label,
            );
            if future > plot.left() + 70.0 {
                painter.text(
                    pos2(future - 5.0, plot.top() - 3.0),
                    Align2::RIGHT_BOTTOM,
                    "Latest readings",
                    FontId::proportional(11.0),
                    label,
                );
            }
            dash(
                &clipped,
                pos2(future, plot.top()),
                pos2(future, plot.bottom()),
                label,
                3.0,
                4.0,
            );
        }
        for level in [0, 25, 50, 75, 100] {
            painter.hline(plot.x_range(), y(level), rule);
            painter.text(
                pos2(plot.left() - 8.0, y(level)),
                Align2::RIGHT_CENTER,
                format!("{level}%"),
                FontId::proportional(12.0),
                label,
            );
        }
        let mut levels = self.thresholds.to_vec();
        levels.sort_unstable();
        levels.dedup();
        let mut previous_y = plot.bottom() + 14.0;
        for level in levels {
            let color = if level <= 10 {
                Color32::from_rgb(197, 108, 114)
            } else {
                Color32::from_rgb(174, 127, 92)
            };
            dash(
                &clipped,
                pos2(plot.left(), y(level)),
                pos2(plot.right(), y(level)),
                color.gamma_multiply(0.68),
                2.0,
                5.0,
            );
            let label_y = y(level).min(previous_y - 14.0);
            if label_y >= plot.top() {
                painter.text(
                    pos2(plot.right() + 4.0, label_y),
                    Align2::LEFT_CENTER,
                    format!("{level}%"),
                    FontId::proportional(12.0),
                    color,
                );
                previous_y = label_y;
            }
        }
        if self.show_charging {
            paint_charging(
                &clipped,
                plot,
                *range,
                self.focused.chart,
                CHARGING.gamma_multiply(0.18),
            );
        }
        for series in self.series {
            if self.bridge_gaps {
                let first = series
                    .chart
                    .bridges
                    .partition_point(|&(_, right)| right.t < range.start);
                for &(left, right) in &series.chart.bridges[first..] {
                    if left.t > range.end {
                        break;
                    }
                    dash(
                        &clipped,
                        pos2(x(left.t), y(left.p.unwrap_or_default())),
                        pos2(x(right.t), y(right.p.unwrap_or_default())),
                        series.color.gamma_multiply(0.48),
                        4.0,
                        4.0,
                    );
                }
            }
            let key = TraceKey {
                revision: series.chart.revision,
                range: *range,
                plot,
                color: series.color,
                focused: series.device.identity == self.focused.device.identity,
            };
            let id = response
                .id
                .with(("recorded-geometry", &series.device.identity));
            let cached = ui.data(|data| data.get_temp::<TraceGeometry>(id));
            let geometry = cached
                .filter(|cached| cached.key == key)
                .unwrap_or_else(|| {
                    let shapes = recorded_geometry(series.chart, &key);
                    let geometry = TraceGeometry { key, shapes };
                    ui.data_mut(|data| data.insert_temp(id, geometry.clone()));
                    geometry
                });
            clipped.extend(geometry.shapes.iter().cloned());
        }
        for series in self.series {
            if let Some(point) = series.device.points.iter().rfind(|point| point.p.is_some()) {
                if let Some(level) = point.p {
                    let center = pos2(x(point.t), y(level));
                    if plot.contains(center) {
                        let markers =
                            painter.with_clip_rect(plot.expand(4.0).intersect(ui.clip_rect()));
                        markers.circle(
                            center,
                            3.7,
                            series.color,
                            Stroke::new(1.25, ui.visuals().window_fill),
                        );
                        if series.device.identity == self.focused.device.identity {
                            let text_y = if center.y - plot.top() < 22.0 {
                                18.0
                            } else {
                                -18.0
                            };
                            clipped.text(
                                center + vec2(-7.0, text_y),
                                Align2::RIGHT_BOTTOM,
                                format!("{level}% latest"),
                                FontId::proportional(12.0),
                                series.color,
                            );
                        }
                    }
                }
            }
        }
        for (index, (forecast, color)) in self
            .forecasts
            .into_iter()
            .zip(forecast_colors(ui))
            .enumerate()
        {
            let Some(fit) = forecast else { continue };
            let start = pos2(x(fit.start.t), y(fit.start.p.unwrap_or(0)));
            let latest = pos2(x(fit.latest.t), y(fit.latest.p.unwrap_or(0)));
            let end = pos2(x(fit.eta(0).unwrap_or(fit.latest.t)), plot.bottom());
            let length = if index == 0 { 7.0 } else { 2.0 };
            dash(
                &clipped,
                start,
                latest,
                color.lerp_to_gamma(label, 0.45).gamma_multiply(0.62),
                length,
                4.0,
            );
            dash(&clipped, latest, end, color, length, 4.0);
            clipped.circle_stroke(start, 4.0, Stroke::new(1.5, label));
            let label_x = start.x + 6.0;
            if start.x >= plot.left() && label_x < plot.right() - 65.0 {
                clipped.text(
                    pos2(label_x, (start.y - 5.0).max(plot.top() + 13.0)),
                    Align2::LEFT_BOTTOM,
                    if index == 0 {
                        "Last charge"
                    } else {
                        "Recent window"
                    },
                    FontId::proportional(10.0),
                    label,
                );
            }
            if start.x < plot.left() && latest.x > plot.left() + 20.0 {
                clipped.text(
                    pos2(plot.left() + 5.0, plot.top() + 5.0 + index as f32 * 14.0),
                    Align2::LEFT_TOP,
                    if index == 0 {
                        "← Charge fit"
                    } else {
                        "← Recent fit"
                    },
                    FontId::proportional(11.0),
                    color,
                );
            }
            for &target in self.thresholds {
                if let Some(at) = fit.eta(target) {
                    clipped.circle_filled(pos2(x(at), y(target)), 2.8, color);
                }
            }
            if let Some(at) = fit
                .eta(self.target)
                .filter(|at| *at > fit.latest.t && *at <= range.end && *at >= range.start)
            {
                let point = pos2(x(at), y(self.target));
                clipped.circle_filled(point, 3.2, color);
                let offset = if index == 0 { -7.0 } else { 15.0 };
                clipped.text(
                    point + vec2(-5.0, offset),
                    Align2::RIGHT_BOTTOM,
                    format!("{}% · {}", self.target, platform::chart_date(at, false)),
                    FontId::proportional(10.0),
                    color,
                );
            }
        }
        let ticks = platform::chart_ticks(range.start, range.end, plot.width());
        for (at, tick) in ticks {
            painter.text(
                pos2(x(at), plot.bottom() + 6.0),
                Align2::CENTER_TOP,
                tick,
                FontId::proportional(12.0),
                label,
            );
        }
        if let Some(selection) = *selection {
            paint_interval(
                &clipped,
                plot,
                *range,
                selection,
                ui.visuals().selection.stroke.color,
                true,
            );
        }
        if let Some(activity) = activity {
            paint_interval(
                &clipped,
                plot,
                *range,
                activity.interval,
                ui.visuals().selection.stroke.color,
                false,
            );
        }
        if let Some(at) = cursor
            .or(hovered_at)
            .or(external_cursor)
            .filter(|at| (range.start..=range.end).contains(at))
        {
            let cx = x(at);
            dash(
                &clipped,
                pos2(cx, plot.top()),
                pos2(cx, plot.bottom()),
                label,
                4.0,
                3.0,
            );
        }
        if let Some(at) = hovered_at.filter(|_| drag_start.is_none()) {
            response.on_hover_ui_at_pointer(|ui| {
                ui.set_max_width(390.0);
                ui.label(platform::exact_date(at));
                for series in self.series {
                    ui.label(format!(
                        "{} · {}",
                        series.device.name,
                        reading_at(*series, at)
                    ));
                }
                for (index, fit) in self.forecasts.into_iter().enumerate() {
                    let Some(fit) = fit else { continue };
                    if let Some(details) =
                        forecast_details(fit, FORECAST_NAMES[index], at, self.target)
                    {
                        ui.label(details);
                        for &threshold in self
                            .thresholds
                            .iter()
                            .filter(|&&level| level != self.target)
                        {
                            if let Some(eta) = fit.eta(threshold) {
                                ui.label(format!("{threshold}% at {}", platform::exact_date(eta)));
                            }
                        }
                    }
                }
            });
        }
        hovered_at
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn activity(
        &self,
        ui: &mut Ui,
        range: TimeRange,
        cursor: &mut Option<i64>,
        external_cursor: Option<i64>,
        selection: Option<TimeRange>,
        pinned_activity: &mut Option<ActivityHit>,
        previous_hover: Option<&ActivityHit>,
    ) -> (Option<i64>, Option<ActivityHit>) {
        let mut hovered = None;
        let mut active_hit = None;
        for series in self.series {
            let (area, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            if !ui.is_rect_visible(area) {
                continue;
            }
            let track = Rect::from_min_max(
                area.min + vec2(gutter(area.width()), 1.0),
                area.max - vec2(42.0, 1.0),
            );
            let name = Rect::from_min_max(area.min, pos2(track.left() - 6.0, area.bottom()));
            ui.put(
                name,
                egui::Label::new(
                    egui::RichText::new(&series.device.name)
                        .size(11.0)
                        .color(series.color),
                )
                .truncate(),
            );
            let painter = ui.painter().with_clip_rect(track.intersect(ui.clip_rect()));
            painter.rect_filled(track, 0, ui.visuals().extreme_bg_color);
            let known_end = range.x(track, self.recorded_end).min(track.right());
            if known_end > track.left() {
                let known = Rect::from_min_max(track.min, pos2(known_end, track.bottom()));
                let hatch = painter.with_clip_rect(known.intersect(ui.clip_rect()));
                for offset in (0..known.width() as usize + 24).step_by(8) {
                    let left = known.left() + offset as f32;
                    hatch.line_segment(
                        [pos2(left, known.top()), pos2(left - 22.0, known.bottom())],
                        ui.visuals().window_stroke,
                    );
                }
            }
            let sessions = &series.device.sessions;
            let activity_at = |at| {
                if at > self.recorded_end {
                    return None;
                }
                let index = sessions.partition_point(|session| session.end <= at);
                let (interval, state) = if let Some(interval) =
                    series.chart.charging_at(at).filter(|_| self.show_charging)
                {
                    (interval, ActivityState::Charging)
                } else if let Some(session) = sessions.get(index).filter(|s| s.start <= at) {
                    (
                        TimeRange::ordered(session.start, session.end),
                        if session.online {
                            ActivityState::Online
                        } else {
                            ActivityState::Offline
                        },
                    )
                } else {
                    let start = index
                        .checked_sub(1)
                        .map_or(range.start, |i| sessions[i].end);
                    let end = sessions
                        .get(index)
                        .map_or(self.recorded_end, |session| session.start);
                    (TimeRange::ordered(start, end), ActivityState::Unknown)
                };
                Some(ActivityHit {
                    at,
                    identity: Arc::clone(&series.chart.identity),
                    interval,
                    state,
                })
            };
            let first = sessions.partition_point(|s| s.end <= range.start);
            // Draw at most one segment per pixel column, even for very dense histories.
            let columns = (track.width().min(COLUMNS as f32) as usize).max(1);
            let at_column = |column| track.left() + track.width() * column as f32 / columns as f32;
            let mut session_index = first;
            let mut run_start = 0;
            let mut run_state = None;
            for column in 0..=columns {
                let left = at_column(column);
                let state = if column == columns || left >= known_end {
                    None
                } else {
                    let at = range.at(track, (left + at_column(column + 1)) * 0.5);
                    while sessions.get(session_index).is_some_and(|s| s.end <= at) {
                        session_index += 1;
                    }
                    sessions
                        .get(session_index)
                        .filter(|s| s.start <= at)
                        .map(|s| s.online)
                };
                if state != run_state {
                    if let Some(online) = run_state {
                        let color = if online {
                            series.color
                        } else {
                            ui.visuals()
                                .extreme_bg_color
                                .lerp_to_gamma(ui.visuals().weak_text_color(), 0.34)
                        };
                        painter.rect_filled(
                            Rect::from_min_max(
                                pos2(at_column(run_start), track.top()),
                                pos2(left.min(known_end), track.bottom()),
                            ),
                            0,
                            color,
                        );
                    }
                    run_start = column;
                    run_state = state;
                }
            }
            if self.show_charging {
                let overlay =
                    Rect::from_min_max(pos2(track.left(), track.bottom() - 5.0), track.max);
                paint_charging(&painter, overlay, range, series.chart, CHARGING);
                let first = series
                    .chart
                    .charging
                    .partition_point(|&(_, end)| end < range.start);
                for &(start, end) in &series.chart.charging[first..] {
                    if start > range.end {
                        break;
                    }
                    let left = range.x(track, start).max(track.left());
                    let right = range.x(track, end).min(track.right());
                    if right - left >= 23.0 {
                        let center = pos2((left + right) * 0.5, track.center().y - 2.0);
                        let bolt = [
                            center + vec2(1.0, -5.0),
                            center + vec2(-2.0, 0.0),
                            center + vec2(2.0, 0.0),
                            center + vec2(-1.0, 5.0),
                        ];
                        painter.add(Shape::line(bolt.to_vec(), Stroke::new(2.0, Color32::BLACK)));
                        painter.add(Shape::line(bolt.to_vec(), Stroke::new(1.0, CHARGING)));
                    }
                }
            }
            if let Some(selection) = selection {
                paint_interval(
                    &painter,
                    track,
                    range,
                    selection,
                    ui.visuals().selection.stroke.color,
                    false,
                );
            }
            if response.clicked() {
                if let Some(pos) = response
                    .interact_pointer_pos()
                    .filter(|pos| track.contains(*pos))
                {
                    let at = range.at(track, pos.x);
                    *pinned_activity = activity_at(at);
                    *cursor = Some(at);
                }
            }
            if let Some(pos) = response.hover_pos().filter(|pos| track.contains(*pos)) {
                let at = range.at(track, pos.x);
                hovered = Some(at);
                active_hit = activity_at(at);
                if let Some(hit) = &active_hit {
                    let samples = hit.interval.points(&series.device.points);
                    let endpoints = samples
                        .first()
                        .zip(samples.last())
                        .and_then(|(start, end)| Some((start.p?, end.p?)));
                    let battery = endpoints.map_or_else(
                        || "Battery unavailable".to_owned(),
                        |(start, end)| {
                            format!(
                                "{start}% to {end}% · {:+} pp",
                                i16::from(end) - i16::from(start)
                            )
                        },
                    );
                    response.on_hover_text(format!(
                        "{} · {} · {}\n{} to {}\n{} · {} readings",
                        series.device.name,
                        hit.state.label(),
                        duration(hit.interval.end - hit.interval.start),
                        platform::exact_date(hit.interval.start),
                        platform::exact_date(hit.interval.end),
                        battery,
                        samples.len()
                    ));
                }
            }
            if let Some(hit) = pinned_activity
                .as_ref()
                .or(active_hit.as_ref())
                .or(previous_hover)
                .filter(|hit| hit.identity == series.chart.identity)
            {
                paint_interval(
                    &painter,
                    track,
                    range,
                    hit.interval,
                    ui.visuals().selection.stroke.color,
                    false,
                );
            }
            if let Some(at) = cursor
                .or(hovered)
                .or(previous_hover.map(|hit| hit.at))
                .or(external_cursor)
                .filter(|at| (range.start..=range.end).contains(at))
            {
                painter.vline(
                    range.x(track, at),
                    track.y_range(),
                    Stroke::new(1.0, ui.visuals().text_color()),
                );
            }
        }
        (hovered, active_hit)
    }
}

fn paint_interval(
    painter: &Painter,
    rect: Rect,
    range: TimeRange,
    interval: TimeRange,
    color: Color32,
    handles: bool,
) {
    let left = range.x(rect, interval.start);
    let right = range.x(rect, interval.end);
    let highlight =
        Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())).intersect(rect);
    if highlight.is_positive() {
        painter.rect_filled(highlight, 0, color.gamma_multiply(0.22));
        if handles {
            for x in [left, right]
                .into_iter()
                .filter(|x| rect.x_range().contains(*x))
            {
                painter.vline(x, rect.y_range(), Stroke::new(1.0, color));
                painter.rect_filled(
                    Rect::from_center_size(pos2(x, rect.center().y), vec2(5.0, 18.0)),
                    2,
                    color,
                );
            }
        }
    }
}

fn paint_charging(painter: &Painter, rect: Rect, range: TimeRange, chart: &Chart, color: Color32) {
    let start = chart
        .charging
        .partition_point(|&(_, end)| end < range.start);
    let mut painted_until = rect.left();
    for &(start, end) in &chart.charging[start..] {
        if start > range.end {
            break;
        }
        let left = range
            .x(rect, start)
            .max(painted_until)
            .floor()
            .max(painted_until);
        let right = range.x(rect, end).ceil().max(left + 1.0).min(rect.right());
        if right > left {
            painter.rect_filled(
                Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
                0,
                color,
            );
            painted_until = right;
        }
    }
}

fn dash(
    painter: &Painter,
    start: egui::Pos2,
    end: egui::Pos2,
    color: Color32,
    length: f32,
    gap: f32,
) {
    // Clip before tessellation so zooming cannot create thousands of off-screen dashes.
    let clip = painter.clip_rect();
    let direction = end - start;
    let mut from = 0.0_f32;
    let mut to = 1.0_f32;
    for (origin, delta, min, max) in [
        (start.x, direction.x, clip.left(), clip.right()),
        (start.y, direction.y, clip.top(), clip.bottom()),
    ] {
        if delta == 0.0 {
            if origin < min || origin > max {
                return;
            }
        } else {
            let a = (min - origin) / delta;
            let b = (max - origin) / delta;
            from = from.max(a.min(b));
            to = to.min(a.max(b));
        }
    }
    if from < to {
        painter.extend(Shape::dashed_line(
            &[start + direction * from, start + direction * to],
            Stroke::new(1.5, color),
            length,
            gap,
        ));
    }
}

fn fraction(start: i64, end: i64, time: i64) -> f64 {
    time.saturating_sub(start) as f64 / end.saturating_sub(start).max(1) as f64
}

fn bucket(start: i64, end: i64, time: i64) -> usize {
    ((fraction(start, end, time) * COLUMNS as f64) as usize).min(COLUMNS - 1)
}

pub(super) fn nearest(points: &[DashboardPoint], time: i64) -> Option<DashboardPoint> {
    let index = points.partition_point(|point| point.t < time);
    let previous = index.checked_sub(1).and_then(|i| points.get(i));
    previous
        .into_iter()
        .chain(points.get(index))
        .min_by_key(|point| time.abs_diff(point.t))
        .copied()
}

pub(super) fn reading_at(series: Series<'_>, at: i64) -> String {
    let points = &series.device.points;
    let next = points.partition_point(|point| point.t < at);
    let exact = points.get(next).filter(|point| point.t == at);
    let nearby = exact.copied().or_else(|| {
        let before = next.checked_sub(1).and_then(|index| points.get(index))?;
        let after = points.get(next)?;
        (before.p.is_some()
            && after.p.is_some()
            && before.c == after.c
            && after.t - before.t <= super::super::model::OFFLINE_AFTER_SECONDS)
            .then(|| nearest(points, at))
            .flatten()
    });
    nearby.map_or_else(
        || "No recorded reading".into(),
        |point| {
            format!(
                "{}{} · {} · {} · {}",
                if exact.is_some() { "" } else { "Nearest: " },
                percent(point.p),
                if point.c {
                    "Online, charging"
                } else {
                    "Online"
                },
                voltage(point.v),
                platform::exact_date(point.t),
            )
        },
    )
}

pub(super) fn forecast_details(fit: Forecast, model: &str, at: i64, target: u8) -> Option<String> {
    let estimate =
        f64::from(fit.latest.p?) - fit.rate * (at - fit.latest.t) as f64 / SECONDS_PER_DAY as f64;
    if at < fit.start.t || !(0.0..=100.0).contains(&estimate) {
        return None;
    }
    let kind = if at <= fit.latest.t {
        "Historical fit"
    } else {
        "Forecast"
    };
    let eta = fit.eta(target).map_or_else(
        || format!("{target}% already passed"),
        |eta| format!("{target}% at {}", platform::summary_date(eta)),
    );
    Some(format!(
        "{kind} · {model} · {estimate:.0}% · {:.1} pp/day elapsed · basis {} to {} · {eta}",
        fit.rate,
        platform::summary_date(fit.start.t),
        platform::summary_date(fit.latest.t)
    ))
}
pub(super) fn voltage(value: Option<u16>) -> String {
    value.map_or_else(
        || "—".to_owned(),
        |v| format!("{:.3} V", f32::from(v) / 1000.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps_survive_downsampling_and_only_eligible_endpoints_are_bridged() {
        use crate::history::HistorySample;
        let samples = [
            (0, Some(94), false),
            (60, Some(93), false),
            (1_000, Some(91), false),
            (2_000, Some(90), false),
            (3_000, Some(95), true),
            (4_000, Some(80), false),
            (4_100, None, false),
            (4_200, Some(79), false),
            (10_000_000, Some(70), false),
        ]
        .into_iter()
        .map(|(recorded_at, percent, charging)| HistorySample {
            identity: "hid:fixture".to_owned(),
            name: "Fixture".to_owned(),
            recorded_at,
            percent,
            charging,
            voltage_mv: None,
        })
        .collect();
        let data =
            super::super::super::model::build_data(samples, 365, 10_000_000, 0, false, vec![])
                .unwrap();
        let device = &data.devices[0];
        let chart = Chart::new(device);
        assert_eq!(
            bucket(device.first_at, device.last_at, 0),
            bucket(device.first_at, device.last_at, 4_200)
        );
        let starts = chart
            .points
            .split(|point| point.p.is_none())
            .filter_map(|run| run.first().map(|point| point.t))
            .collect::<Vec<_>>();
        assert_eq!(starts, [0, 1_000, 2_000, 3_000, 4_000, 4_200, 10_000_000]);
        let bridges = chart
            .bridges
            .iter()
            .map(|(left, right)| (left.t, right.t))
            .collect::<Vec<_>>();
        assert_eq!(bridges, [(60, 1_000), (1_000, 2_000), (4_200, 10_000_000)]);
        let series = Series {
            index: 0,
            device,
            chart: &chart,
            color: Color32::WHITE,
        };
        assert_eq!(reading_at(series, 500), "No recorded reading");
        assert!(reading_at(series, 30).starts_with("Nearest:"));
        assert!(!reading_at(series, 60).starts_with("Nearest:"));
        assert!(!chart.continuous_discharge(TimeRange::ordered(60, 1_000)));
    }

    #[test]
    fn discharge_forecasts_use_the_latest_cycle_and_ignore_earlier_charging() {
        use crate::history::HistorySample;
        let hour = 3_600;
        let samples = |latest_charging| {
            [
                (0, 40, false),
                (1, 99, true),
                (2, 96, false),
                (26, 80, false),
                (50, 72, latest_charging),
            ]
            .into_iter()
            .map(|(offset, percent, charging)| HistorySample {
                identity: "hid:forecast-fixture".to_owned(),
                name: "Forecast fixture".to_owned(),
                recorded_at: offset * hour,
                percent: Some(percent),
                charging,
                voltage_mv: None,
            })
            .collect()
        };
        let build = |charging| {
            super::super::super::model::build_data(
                samples(charging),
                7,
                50 * hour,
                0,
                false,
                vec![],
            )
            .unwrap()
        };
        let data = build(false);
        let [cycle, recent] = Chart::new(&data.devices[0]).forecasts.map(Option::unwrap);
        assert_eq!(cycle.start.t, 2 * hour);
        assert_eq!(cycle.rate, 12.0);
        assert_eq!(recent.start.t, 26 * hour);
        assert_eq!(recent.rate, 8.0);
        assert_eq!(recent.eta(64), Some(74 * hour));
        assert!(Chart::new(&build(true).devices[0])
            .forecasts
            .iter()
            .all(Option::is_none));
    }

    #[test]
    fn chart_buckets_cover_the_endpoints_and_zero_span() {
        assert_eq!(bucket(10, 20, 10), 0);
        assert_eq!(bucket(10, 20, 20), COLUMNS - 1);
        assert_eq!(bucket(10, 10, 10), 0);
    }

    #[test]
    fn hover_chooses_the_closest_original_reading() {
        let sample = |t| DashboardPoint {
            t,
            p: Some(50),
            c: false,
            v: None,
        };
        let points = [sample(20), sample(40), sample(80)];
        assert_eq!(nearest(&points, 2), Some(points[0]));
        assert_eq!(nearest(&points, 54), Some(points[1]));
        assert_eq!(nearest(&points, 74), Some(points[2]));
        assert_eq!(nearest(&points, 90), Some(points[2]));
    }
}
