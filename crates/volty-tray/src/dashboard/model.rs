use std::{collections::BTreeMap, io};

use serde::Serialize;

use crate::history::{HistorySample, HistoryStore};

const OFFLINE_AFTER_SECONDS: i64 = 15 * 60;
const SECONDS_PER_DAY: i64 = 86_400;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct DashboardData {
    pub(super) generated_at: i64,
    pub(super) window_days: u32,
    pub(super) management_allowed: bool,
    pub(super) devices: Vec<DashboardDevice>,
    pub(super) inventory: Vec<DashboardInventoryDevice>,
    pub(super) settings: Option<DashboardSettings>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct DashboardSettings {
    pub(super) low_battery_thresholds: Vec<u8>,
    pub(super) low_battery_cooldown_minutes: u64,
    pub(super) notifications_enabled: bool,
    pub(super) tray_icon_style: String,
    pub(super) tray_icon_theme: String,
    pub(super) show_percentage_in_battery_icon: bool,
    pub(super) pinned_device: Option<String>,
    pub(super) start_on_boot: Option<bool>,
    pub(super) config_error: Option<String>,
    pub(super) startup_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct DashboardInventoryDevice {
    pub(super) identity: String,
    pub(super) name: String,
    pub(super) samples: u64,
    pub(super) last_sample_at: Option<i64>,
    pub(super) monitored: Option<bool>,
    pub(super) history_recording_enabled: bool,
    pub(super) pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct DashboardDevice {
    pub(super) identity: String,
    pub(super) name: String,
    pub(super) latest_percent: Option<u8>,
    pub(super) charging: bool,
    pub(super) first_at: i64,
    pub(super) last_at: i64,
    pub(super) samples: u64,
    pub(super) percent_per_day: Option<f64>,
    pub(super) eta_seconds: Option<i64>,
    pub(super) online: bool,
    pub(super) online_seconds: i64,
    pub(super) offline_seconds: i64,
    pub(super) sessions: Vec<DashboardSession>,
    pub(super) points: Vec<DashboardPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) struct DashboardSession {
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) online: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) struct DashboardPoint {
    pub(super) t: i64,
    pub(super) p: Option<u8>,
    pub(super) c: bool,
    pub(super) v: Option<u16>,
}

pub(super) fn build_data(
    samples: Vec<HistorySample>,
    window_days: u32,
    generated_at: i64,
    window_start: i64,
    management_allowed: bool,
    inventory: Vec<DashboardInventoryDevice>,
) -> Result<DashboardData, io::Error> {
    let summaries = HistoryStore::summaries_from_samples(&samples);
    let mut samples_by_identity = BTreeMap::<String, Vec<HistorySample>>::new();
    for sample in samples {
        samples_by_identity
            .entry(sample.identity.clone())
            .or_default()
            .push(sample);
    }

    let mut devices = Vec::with_capacity(summaries.len());
    for summary in summaries {
        let device_samples = samples_by_identity
            .remove(&summary.identity)
            .ok_or_else(|| io::Error::other("history summary has no matching samples"))?;
        let latest = device_samples
            .last()
            .ok_or_else(|| io::Error::other("history device has no latest sample"))?;
        let sessions = derive_sessions(&device_samples, generated_at, window_start);
        let (online_seconds, offline_seconds) = session_durations(&sessions);
        devices.push(DashboardDevice {
            identity: summary.identity,
            name: summary.name,
            latest_percent: summary.latest_percent,
            charging: latest.charging,
            first_at: summary.first_at,
            last_at: summary.last_at,
            samples: summary.samples,
            percent_per_day: summary.percent_per_day,
            eta_seconds: estimate_eta(latest.percent, latest.charging, summary.percent_per_day),
            online: is_online(summary.last_at, generated_at),
            online_seconds,
            offline_seconds,
            sessions,
            points: device_samples
                .into_iter()
                .map(|sample| DashboardPoint {
                    t: sample.recorded_at,
                    p: sample.percent,
                    c: sample.charging,
                    v: sample.voltage_mv,
                })
                .collect(),
        });
    }

    Ok(DashboardData {
        generated_at,
        window_days,
        management_allowed,
        devices,
        inventory,
        settings: None,
    })
}

fn derive_sessions(
    samples: &[HistorySample],
    generated_at: i64,
    window_start: i64,
) -> Vec<DashboardSession> {
    let Some(first_sample) = samples.first() else {
        return Vec::new();
    };

    let start = first_sample.recorded_at.max(window_start);
    let end = generated_at.max(start);
    let mut sessions = Vec::new();

    for (index, sample) in samples.iter().enumerate() {
        let sample_at = sample.recorded_at.max(start).min(end);
        let next_at = samples
            .get(index + 1)
            .map_or(end, |next| next.recorded_at.min(end));
        let online_until = sample
            .recorded_at
            .saturating_add(OFFLINE_AFTER_SECONDS)
            .min(next_at)
            .min(end);

        push_session(&mut sessions, sample_at, online_until, true);
        push_session(&mut sessions, online_until, next_at, false);
    }

    sessions
}

fn push_session(sessions: &mut Vec<DashboardSession>, start: i64, end: i64, online: bool) {
    if end <= start {
        return;
    }
    if let Some(previous) = sessions.last_mut() {
        if previous.online == online && previous.end == start {
            previous.end = end;
            return;
        }
    }
    sessions.push(DashboardSession { start, end, online });
}

fn session_durations(sessions: &[DashboardSession]) -> (i64, i64) {
    sessions
        .iter()
        .fold((0_i64, 0_i64), |(online, offline), session| {
            let duration = session.end.saturating_sub(session.start);
            if session.online {
                (online.saturating_add(duration), offline)
            } else {
                (online, offline.saturating_add(duration))
            }
        })
}

fn is_online(last_at: i64, generated_at: i64) -> bool {
    last_at <= generated_at && generated_at.saturating_sub(last_at) <= OFFLINE_AFTER_SECONDS
}

fn estimate_eta(
    latest_percent: Option<u8>,
    charging: bool,
    percent_per_day: Option<f64>,
) -> Option<i64> {
    if charging {
        return None;
    }
    let (Some(percent), Some(rate)) = (latest_percent, percent_per_day) else {
        return None;
    };
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    Some((f64::from(percent) * SECONDS_PER_DAY as f64 / rate).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(recorded_at: i64) -> HistorySample {
        HistorySample {
            identity: "hid:mouse".to_owned(),
            name: "Mouse".to_owned(),
            recorded_at,
            percent: Some(50),
            charging: false,
            voltage_mv: Some(3_800),
        }
    }

    #[test]
    fn sessions_cover_the_window_and_split_long_gaps() {
        let samples = [sample(1_000), sample(1_600), sample(3_000)];
        let sessions = derive_sessions(&samples, 3_600, 900);

        assert_eq!(
            sessions,
            [
                DashboardSession {
                    start: 1_000,
                    end: 2_500,
                    online: true,
                },
                DashboardSession {
                    start: 2_500,
                    end: 3_000,
                    online: false,
                },
                DashboardSession {
                    start: 3_000,
                    end: 3_600,
                    online: true,
                },
            ]
        );
        assert_eq!(session_durations(&sessions), (2_100, 500));
        assert!(is_online(3_000, 3_600));
        assert!(!is_online(2_000, 3_000));
    }

    #[test]
    fn eta_requires_discharge_and_a_positive_rate() {
        assert_eq!(estimate_eta(Some(50), false, Some(25.0)), Some(172_800));
        assert_eq!(estimate_eta(Some(50), true, Some(25.0)), None);
        assert_eq!(estimate_eta(Some(50), false, None), None);
        assert_eq!(estimate_eta(Some(50), false, Some(0.0)), None);
    }

    #[test]
    fn management_capability_is_serialized_for_both_read_only_and_interactive_data() {
        let read_only = build_data(Vec::new(), 7, 100, 0, false, Vec::new()).unwrap();
        let interactive = build_data(Vec::new(), 7, 100, 0, true, Vec::new()).unwrap();

        assert_eq!(
            serde_json::to_value(read_only).unwrap()["management_allowed"],
            false
        );
        assert_eq!(
            serde_json::to_value(interactive).unwrap()["management_allowed"],
            true
        );
    }
}
