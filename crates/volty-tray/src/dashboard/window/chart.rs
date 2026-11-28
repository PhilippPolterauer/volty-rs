use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Painter, Rect, Sense, Shape, Stroke, Ui,
};

use super::{
    super::model::{DashboardDevice, DashboardPoint, OFFLINE_AFTER_SECONDS, SECONDS_PER_DAY},
    duration, platform,
};

const COLUMNS: usize = 640;
pub(super) const CHARGING: Color32 = Color32::from_rgb(250, 204, 21);

pub(super) struct Chart {
    points: Vec<DashboardPoint>,
    charging: Vec<(i64, i64)>,
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
        let mut remaining = device.points.as_slice();
        let mut previous_group_end = None;

        for column in 0..COLUMNS {
            let count = remaining.partition_point(|point| bucket(start, end, point.t) <= column);
            let (group, rest) = remaining.split_at(count);
            remaining = rest;
            if group.is_empty() {
                continue;
            }
            let valid = || {
                group
                    .iter()
                    .enumerate()
                    .filter_map(|(index, point)| point.p.map(|p| (p, index)))
            };
            let first_gap = if previous_group_end
                .is_some_and(|previous| group[0].t.saturating_sub(previous) > OFFLINE_AFTER_SECONDS)
            {
                Some(0)
            } else {
                group
                    .windows(2)
                    .position(|pair| pair[1].t.saturating_sub(pair[0].t) > OFFLINE_AFTER_SECONDS)
                    .map(|index| index + 1)
            };
            previous_group_end = Some(group[group.len() - 1].t);
            let mut indices = [
                Some(0),
                Some(group.len() - 1),
                valid().min().map(|(_, index)| index),
                valid().max().map(|(_, index)| index),
                group.iter().position(|point| point.p.is_none()),
                first_gap.and_then(|index| index.checked_sub(1)),
                first_gap,
            ];
            indices.sort_unstable();
            let mut previous = None;
            for index in indices.into_iter().flatten() {
                if previous != Some(index) {
                    if first_gap == Some(index) {
                        points.push(DashboardPoint {
                            p: None,
                            ..group[index]
                        });
                    }
                    points.push(group[index]);
                    previous = Some(index);
                }
            }
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
        if let Some(&latest) = device.points.last().filter(|p| !p.c && p.p.is_some()) {
            let since_charge = device
                .points
                .iter()
                .rposition(|p| p.c)
                .map(|index| &device.points[index + 1..]);
            if let Some(cycle) =
                since_charge.and_then(|points| points.iter().find(|p| p.p.is_some()))
            {
                forecasts[0] = Forecast::new(*cycle, latest);
            }
            let discharge = since_charge.unwrap_or(&device.points);
            let cutoff = latest.t.saturating_sub(SECONDS_PER_DAY);
            let recent = discharge.partition_point(|p| p.t < cutoff);
            if let Some(first) = discharge[recent..].iter().find(|p| p.p.is_some()) {
                forecasts[1] = Forecast::new(*first, latest);
            }
        }
        Self {
            points,
            charging,
            forecasts,
        }
    }
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
    pub(super) recorded_end: i64,
    pub(super) full: TimeRange,
}

impl Plot<'_> {
    pub(super) fn trend(
        &self,
        ui: &mut Ui,
        range: &mut TimeRange,
        cursor: &mut Option<i64>,
        external_cursor: Option<i64>,
    ) -> Option<i64> {
        let (area, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 266.0), Sense::click());
        if !ui.is_rect_visible(area) {
            return None;
        }
        let plot = Rect::from_min_max(
            area.min + vec2(gutter(area.width()), 20.0),
            area.max - vec2(14.0, 30.0),
        );
        let hover = response.hover_pos().filter(|pos| plot.contains(*pos));
        if let Some(pos) = hover {
            let scroll =
                ui.input(|input| input.smooth_scroll_delta.y + input.smooth_scroll_delta.x);
            if scroll != 0.0 {
                let pan = ui.input(|input| input.modifiers.shift);
                *range = range.zoom(self.full, range.at(plot, pos.x), scroll, pan);
                ui.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
            }
        }
        let hovered_at = hover.map(|pos| range.at(plot, pos.x));
        if response.clicked() {
            if let Some(at) = response
                .interact_pointer_pos()
                .filter(|pos| plot.contains(*pos))
                .map(|pos| range.at(plot, pos.x))
            {
                *cursor = if cursor.is_some() { None } else { Some(at) };
            }
        }
        if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            *cursor = None;
        }

        let painter = ui.painter();
        let clipped = painter.with_clip_rect(plot.intersect(ui.clip_rect()));
        let label = ui.visuals().weak_text_color();
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
                FontId::proportional(11.0),
                label,
            );
        }
        for &level in self.thresholds {
            dash(
                &clipped,
                pos2(plot.left(), y(level)),
                pos2(plot.right(), y(level)),
                label.gamma_multiply(0.55),
                2.0,
                5.0,
            );
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
            let width = if series.device.identity == self.focused.device.identity {
                2.1
            } else {
                1.6
            };
            for segment in series.chart.points.split(|point| point.p.is_none()) {
                let path: Vec<_> = segment
                    .iter()
                    .filter_map(|p| p.p.map(|level| pos2(x(p.t), y(level))))
                    .collect();
                if path.len() > 1 {
                    clipped.add(Shape::line(path.clone(), Stroke::new(width, series.color)));
                }
                for point in path {
                    if plot.contains(point) {
                        clipped.circle_filled(point, 1.7, series.color);
                    }
                }
            }
        }
        for series in self.series {
            if let Some(point) = series.device.points.last() {
                if let Some(level) = point.p {
                    let center = pos2(x(point.t), y(level));
                    if plot.contains(center) {
                        clipped.circle(
                            center,
                            3.7,
                            series.color,
                            Stroke::new(1.25, ui.visuals().window_fill),
                        );
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
                label.gamma_multiply(0.65),
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
        let ticks = ((plot.width() / 105.0) as i64).clamp(2, 9);
        for tick in 0..=ticks {
            let at = range.start + (range.end - range.start) * tick / ticks;
            painter.text(
                pos2(x(at), plot.bottom() + 6.0),
                match tick {
                    0 => Align2::LEFT_TOP,
                    n if n == ticks => Align2::RIGHT_TOP,
                    _ => Align2::CENTER_TOP,
                },
                platform::chart_date(at, range.end - range.start < 3 * SECONDS_PER_DAY),
                FontId::proportional(11.0),
                label,
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
        hovered_at
    }

    pub(super) fn activity(
        &self,
        ui: &mut Ui,
        range: TimeRange,
        cursor: &mut Option<i64>,
        external_cursor: Option<i64>,
    ) -> Option<i64> {
        let mut hovered = None;
        for series in self.series {
            let (area, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), 19.0), Sense::click());
            if !ui.is_rect_visible(area) {
                continue;
            }
            let track = Rect::from_min_max(
                area.min + vec2(gutter(area.width()), 1.0),
                area.max - vec2(14.0, 1.0),
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
            let sessions = &series.device.sessions;
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
                            ui.visuals().weak_text_color().gamma_multiply(0.30)
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
                paint_charging(&painter, track, range, series.chart, CHARGING);
            }
            if response.clicked() {
                if let Some(pos) = response
                    .interact_pointer_pos()
                    .filter(|pos| track.contains(*pos))
                {
                    let at = range.at(track, pos.x);
                    *cursor = if *cursor == Some(at) { None } else { Some(at) };
                }
            }
            if let Some(pos) = response.hover_pos().filter(|pos| track.contains(*pos)) {
                let at = range.at(track, pos.x);
                hovered = Some(at);
                let index = sessions.partition_point(|s| s.end <= at);
                if let Some(session) = sessions
                    .get(index)
                    .filter(|s| s.start <= at && at <= self.recorded_end)
                {
                    response.on_hover_text(format!(
                        "{} · {} (inferred)\n{} to {}",
                        duration(session.end - session.start),
                        if session.online { "Online" } else { "Offline" },
                        platform::exact_date(session.start),
                        platform::exact_date(session.end)
                    ));
                }
            }
            if let Some(at) = cursor
                .or(hovered)
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
        hovered
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
