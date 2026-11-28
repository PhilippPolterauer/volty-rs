use eframe::egui::{self, pos2, vec2, Align2, FontId, Rect, Sense, Stroke, Ui};

use super::{
    super::model::{DashboardDevice, DashboardPoint},
    accent, duration, percent, platform,
};

// Keep drawing cost bounded even when a database contains millions of readings.
const COLUMNS: usize = 640;

pub(super) struct Chart {
    points: Vec<DashboardPoint>,
    charging: Vec<(usize, usize)>,
    activity: Vec<(usize, usize, bool)>,
}

impl Chart {
    pub(super) fn has_charging(&self) -> bool {
        !self.charging.is_empty()
    }

    pub(super) fn new(device: &DashboardDevice) -> Self {
        let mut points = Vec::new();
        let mut charging = Vec::new();
        let mut activity = Vec::new();
        let start = device.first_at;
        let end = device.last_at;
        let mut remaining = device.points.as_slice();

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
            let mut indices = [
                Some(0),
                Some(group.len() - 1),
                valid().min().map(|(_, index)| index),
                valid().max().map(|(_, index)| index),
                group.iter().position(|point| point.p.is_none()),
            ];
            indices.sort_unstable();
            let mut previous = None;
            for index in indices.into_iter().flatten() {
                if previous != Some(index) {
                    points.push(group[index]);
                    previous = Some(index);
                }
            }
        }

        if let (Some(first), Some(last)) = (device.sessions.first(), device.sessions.last()) {
            for column in 0..COLUMNS {
                let at = interpolate(
                    first.start,
                    last.end,
                    (column as f64 + 0.5) / COLUMNS as f64,
                );
                let index = device.sessions.partition_point(|session| session.end <= at);
                if let Some(session) = device.sessions.get(index) {
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
        for (time, position, align) in [
            (device.first_at, plot.left_bottom(), Align2::LEFT_TOP),
            (device.last_at, plot.right_bottom(), Align2::RIGHT_TOP),
        ] {
            painter.text(
                position + vec2(0.0, 5.0),
                align,
                platform::date(time),
                FontId::proportional(12.0),
                label_color,
            );
            if device.first_at == device.last_at {
                break;
            }
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
                response.on_hover_text(format!(
                    "{}\n{}  ·  {}{}",
                    platform::date(point.t),
                    percent(point.p),
                    voltage(point.v),
                    if point.c { "  ·  Charging" } else { "" }
                ));
            }
        }
    }

    pub(super) fn timeline(&self, ui: &mut Ui, device: &DashboardDevice) {
        if self.activity.is_empty() {
            ui.label("No activity intervals in this range.");
            return;
        }
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        let online_color = accent(ui);
        let offline_color = ui.visuals().widgets.inactive.bg_fill;
        for &(from, to, online) in &self.activity {
            ui.painter().rect_filled(
                band_rect(rect, from, to),
                0,
                if online { online_color } else { offline_color },
            );
        }
        if let (Some(pos), Some(first), Some(last)) = (
            response.hover_pos(),
            device.sessions.first(),
            device.sessions.last(),
        ) {
            let at = interpolate(
                first.start,
                last.end,
                f64::from((pos.x - rect.left()) / rect.width()),
            );
            let index = device.sessions.partition_point(|session| session.end <= at);
            if let Some(session) = device.sessions.get(index) {
                response.on_hover_text(format!(
                    "{}  ·  {}\n{} to {}",
                    if session.online { "Online" } else { "Offline" },
                    duration(session.end - session.start),
                    platform::date(session.start),
                    platform::date(session.end)
                ));
            }
        }
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
