use eframe::egui::{self, pos2, vec2, Align2, FontId, Rect, Sense, Stroke, Ui};

use super::{
    super::model::{DashboardDevice, DashboardPoint, OFFLINE_AFTER_SECONDS},
    accent, duration, percent, platform,
};

// Keep drawing cost bounded even when a database contains millions of readings.
const COLUMNS: usize = 640;

pub(super) struct Chart {
    points: Vec<DashboardPoint>,
    charging: Vec<(usize, usize)>,
    activity: Vec<(usize, usize, bool)>,
    activity_start: i64,
    activity_end: i64,
    labels: Vec<(i64, String)>,
}

impl Chart {
    pub(super) fn has_charging(&self) -> bool {
        !self.charging.is_empty()
    }

    pub(super) fn new(device: &DashboardDevice, activity_start: i64, activity_end: i64) -> Self {
        let mut points = Vec::new();
        let mut charging = Vec::new();
        let mut activity = Vec::new();
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
            if group.iter().any(|point| point.c) {
                push_band(&mut charging, column);
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

        if !device.sessions.is_empty() {
            for column in 0..COLUMNS {
                let at = interpolate(
                    activity_start,
                    activity_end,
                    (column as f64 + 0.5) / COLUMNS as f64,
                );
                let index = device.sessions.partition_point(|session| session.end <= at);
                if let Some(session) = device
                    .sessions
                    .get(index)
                    .filter(|session| session.start <= at)
                {
                    match activity.last_mut() {
                        Some((_, right, online))
                            if *online == session.online && *right == column =>
                        {
                            *right += 1
                        }
                        _ => activity.push((column, column + 1, session.online)),
                    }
                }
            }
        }
        Self {
            points,
            charging,
            activity,
            activity_start,
            activity_end,
            labels: [0.0, 0.25, 0.5, 0.75, 1.0]
                .into_iter()
                .map(|fraction| interpolate(start, end, fraction))
                .map(|time| (time, platform::date(time)))
                .collect(),
        }
    }

    pub(super) fn trend(&self, ui: &mut Ui, device: &DashboardDevice) {
        if self.points.iter().all(|point| point.p.is_none()) {
            ui.label("No percentage readings in this range.");
            return;
        }
        let (area, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 220.0), Sense::hover());
        if !ui.is_rect_visible(area) {
            return;
        }
        let plot = Rect::from_min_max(area.min + vec2(42.0, 9.0), area.max - vec2(8.0, 24.0));
        let painter = ui.painter();
        let color = accent(ui);
        let label_color = ui.visuals().text_color();
        let x =
            |t| plot.left() + fraction(device.first_at, device.last_at, t) as f32 * plot.width();
        let y = |p: u8| plot.bottom() - f32::from(p.min(100)) / 100.0 * plot.height();
        for level in [0_u8, 25, 50, 75, 100] {
            painter.line_segment(
                [pos2(plot.left(), y(level)), pos2(plot.right(), y(level))],
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
            painter.text(
                pos2(plot.left() - 7.0, y(level)),
                Align2::RIGHT_CENTER,
                format!("{level}%"),
                FontId::proportional(12.0),
                label_color,
            );
        }
        for &(from, to) in &self.charging {
            painter.rect_filled(band_rect(plot, from, to), 0, color.gamma_multiply(0.14));
        }
        for segment in self.points.split(|point| point.p.is_none()) {
            let path: Vec<_> = segment
                .iter()
                .filter_map(|point| point.p.map(|p| pos2(x(point.t), y(p))))
                .collect();
            match path.as_slice() {
                [] => {}
                [point] => {
                    painter.circle_filled(*point, 3.0, color);
                }
                _ => {
                    painter.add(egui::Shape::line(path, Stroke::new(2.3, color)));
                }
            }
        }
        for (index, (time, label)) in self.labels.iter().enumerate() {
            let align = match index {
                0 => Align2::LEFT_TOP,
                4 => Align2::RIGHT_TOP,
                2 if plot.width() >= 480.0 => Align2::CENTER_TOP,
                1 | 3 if plot.width() >= 850.0 => Align2::CENTER_TOP,
                _ => continue,
            };
            if index > 0 && device.first_at == device.last_at {
                break;
            }
            painter.text(
                pos2(x(*time), plot.bottom() + 5.0),
                align,
                label,
                FontId::proportional(12.0),
                label_color,
            );
        }
        if let Some(pos) = response.hover_pos().filter(|pos| plot.contains(*pos)) {
            let at = interpolate(
                device.first_at,
                device.last_at,
                f64::from((pos.x - plot.left()) / plot.width()),
            );
            if let Some(point) = nearest(&device.points, at) {
                let x = x(point.t);
                painter.vline(
                    x,
                    plot.y_range(),
                    Stroke::new(1.0, color.gamma_multiply(0.55)),
                );
                if let Some(p) = point.p {
                    painter.circle_filled(pos2(x, y(p)), 4.0, color);
                }
                response.on_hover_text_at_pointer(format!(
                    "{}\n{}  ·  {}{}",
                    platform::exact_date(point.t),
                    percent(point.p),
                    voltage(point.v),
                    if point.c { "  ·  Charging" } else { "" }
                ));
            }
        }
    }

    pub(super) fn timeline(&self, ui: &mut Ui, device: &DashboardDevice) {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        let online_color = accent(ui);
        let offline_color = ui.visuals().widgets.noninteractive.bg_stroke.color;
        ui.painter()
            .rect_filled(rect, 0, ui.visuals().extreme_bg_color);
        for &(from, to, online) in &self.activity {
            ui.painter().rect_filled(
                band_rect(rect, from, to),
                0,
                if online { online_color } else { offline_color },
            );
        }
        if let Some(pos) = response.hover_pos() {
            let at = interpolate(
                self.activity_start,
                self.activity_end,
                f64::from((pos.x - rect.left()) / rect.width()),
            );
            let index = device.sessions.partition_point(|session| session.end <= at);
            if let Some(session) = device
                .sessions
                .get(index)
                .filter(|session| session.start <= at)
            {
                response.on_hover_text_at_pointer(format!(
                    "{}  ·  {}\n{} to {}",
                    if session.online {
                        "Online (inferred)"
                    } else {
                        "Offline (inferred)"
                    },
                    duration(session.end - session.start),
                    platform::exact_date(session.start),
                    platform::exact_date(session.end)
                ));
            } else if let Some(first) = device.sessions.first() {
                response.on_hover_text_at_pointer(format!(
                    "Unknown  ·  {}\n{} to {}",
                    duration(first.start.saturating_sub(self.activity_start)),
                    platform::date(self.activity_start),
                    platform::date(first.start),
                ));
            }
        }
    }

    pub(super) fn unknown_seconds(&self, device: &DashboardDevice) -> i64 {
        device
            .sessions
            .first()
            .map_or(self.activity_end, |first| first.start)
            .saturating_sub(self.activity_start)
    }
}

fn fraction(start: i64, end: i64, time: i64) -> f64 {
    time.saturating_sub(start) as f64 / end.saturating_sub(start).max(1) as f64
}

fn interpolate(start: i64, end: i64, fraction: f64) -> i64 {
    start.saturating_add((end.saturating_sub(start) as f64 * fraction) as i64)
}

fn bucket(start: i64, end: i64, time: i64) -> usize {
    ((fraction(start, end, time) * COLUMNS as f64) as usize).min(COLUMNS - 1)
}

fn push_band(bands: &mut Vec<(usize, usize)>, column: usize) {
    match bands.last_mut() {
        Some((_, end)) if *end == column => *end += 1,
        _ => bands.push((column, column + 1)),
    }
}

fn band_rect(plot: Rect, start: usize, end: usize) -> Rect {
    let x = |column| plot.left() + column as f32 / COLUMNS as f32 * plot.width();
    Rect::from_min_max(pos2(x(start), plot.top()), pos2(x(end), plot.bottom()))
}

fn nearest(points: &[DashboardPoint], time: i64) -> Option<DashboardPoint> {
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
