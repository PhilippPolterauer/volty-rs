use super::DeviceReading;
use crate::state::DischargeRate;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use volty_battery::{BatteryState, BatteryStatus, DeviceId};

/// Long-run rates need this much watched discharge time before they are trusted.
const MIN_RATE_ALIVE: Duration = Duration::from_secs(60 * 60);
const MIN_RATE_LOST_PERCENT: f64 = 2.0;
/// Older history is scaled down so the rate follows changing usage habits.
const MAX_RATE_ALIVE_SECONDS: f64 = 14.0 * 24.0 * 3600.0;
const DEFAULT_MAX_SAMPLE_GAP: Duration = Duration::from_secs(10 * 60);

pub(super) struct BatteryEtaEstimator {
    samples: HashMap<DeviceId, VecDeque<BatteryEtaSample>>,
    rates: HashMap<DeviceId, RateTally>,
    max_sample_gap: Duration,
}

impl Default for BatteryEtaEstimator {
    fn default() -> Self {
        Self {
            samples: HashMap::new(),
            rates: HashMap::new(),
            max_sample_gap: DEFAULT_MAX_SAMPLE_GAP,
        }
    }
}

#[derive(Clone, Copy)]
struct BatteryEtaSample {
    measured_at: Instant,
    level_percent: u8,
}

/// Long-run discharge tally of one device, fed only by consecutive samples
/// taken while Volty was running.
#[derive(Default)]
struct RateTally {
    rate: DischargeRate,
    last: Option<BatteryEtaSample>,
    lost_changed: bool,
    time_changed: bool,
}

impl RateTally {
    fn track(&mut self, sample: BatteryEtaSample, max_gap: Duration) {
        if let Some(previous) = self.last {
            let elapsed = sample
                .measured_at
                .saturating_duration_since(previous.measured_at);
            if sample.level_percent <= previous.level_percent && elapsed <= max_gap {
                let lost = f64::from(previous.level_percent - sample.level_percent);
                self.rate.lost_percent += lost;
                self.rate.alive_seconds += elapsed.as_secs_f64();
                self.lost_changed |= lost > 0.0;
                self.time_changed |= !elapsed.is_zero();
                if self.rate.alive_seconds > MAX_RATE_ALIVE_SECONDS {
                    let scale = MAX_RATE_ALIVE_SECONDS / self.rate.alive_seconds;
                    self.rate.lost_percent *= scale;
                    self.rate.alive_seconds = MAX_RATE_ALIVE_SECONDS;
                }
            }
        }
        self.last = Some(sample);
    }

    fn remaining(&self, level_percent: u8) -> Option<Duration> {
        let DischargeRate {
            lost_percent,
            alive_seconds,
        } = self.rate;
        if alive_seconds < MIN_RATE_ALIVE.as_secs_f64() || lost_percent < MIN_RATE_LOST_PERCENT {
            return None;
        }
        Some(Duration::from_secs_f64(
            alive_seconds * f64::from(level_percent) / lost_percent,
        ))
    }
}

impl BatteryEtaEstimator {
    /// Samples further apart than this are not attributed to a drain: Volty
    /// was not watching the device in between.
    pub(super) fn set_max_sample_gap(&mut self, gap: Duration) {
        self.max_sample_gap = gap;
    }

    pub(super) fn load_rates(&mut self, saved: HashMap<DeviceId, DischargeRate>) {
        for (identity, rate) in saved {
            self.rates.entry(identity).or_default().rate = rate;
        }
    }

    /// Rates worth persisting: every drop in level, or anything changed when `force`.
    pub(super) fn take_unsaved_rates(&mut self, force: bool) -> Vec<(DeviceId, DischargeRate)> {
        let mut unsaved = Vec::new();
        for (identity, tally) in &mut self.rates {
            if tally.lost_changed || (force && tally.time_changed) {
                unsaved.push((identity.clone(), tally.rate));
                tally.lost_changed = false;
                tally.time_changed = false;
            }
        }
        unsaved
    }

    pub(super) fn estimate(
        &mut self,
        identity: &DeviceId,
        battery: &BatteryStatus,
        window: Duration,
    ) -> Option<Duration> {
        self.estimate_at(identity, battery, Instant::now(), window)
    }

    pub(super) fn estimate_at(
        &mut self,
        identity: &DeviceId,
        battery: &BatteryStatus,
        measured_at: Instant,
        window: Duration,
    ) -> Option<Duration> {
        let Some(level_percent) = battery.level_percent else {
            self.remove(identity);
            return None;
        };
        if !matches!(
            battery.state,
            BatteryState::Discharging | BatteryState::Unknown
        ) || window.is_zero()
        {
            self.remove(identity);
            return None;
        }

        let sample = BatteryEtaSample {
            measured_at,
            level_percent,
        };
        let tally = self.rates.entry(identity.clone()).or_default();
        tally.track(sample, self.max_sample_gap);
        let long_run = tally.remaining(level_percent);
        self.windowed_estimate(identity, sample, window)
            .or(long_run)
    }

    /// Projects from the drain inside the configured window; it needs a full
    /// window of discharge history first.
    fn windowed_estimate(
        &mut self,
        identity: &DeviceId,
        sample: BatteryEtaSample,
        window: Duration,
    ) -> Option<Duration> {
        let BatteryEtaSample {
            measured_at,
            level_percent,
        } = sample;
        let samples = self.samples.entry(identity.clone()).or_default();
        if samples
            .back()
            .is_some_and(|previous| level_percent > previous.level_percent)
        {
            samples.clear();
        }
        samples.push_back(sample);

        if let Some(cutoff) = measured_at.checked_sub(window) {
            while samples.len() > 1
                && samples
                    .get(1)
                    .is_some_and(|sample| sample.measured_at <= cutoff)
            {
                samples.pop_front();
            }
        }

        let baseline = samples.front()?;
        let elapsed = measured_at.saturating_duration_since(baseline.measured_at);
        if elapsed < window {
            return None;
        }
        let percentage_lost = baseline.level_percent.saturating_sub(level_percent);
        if percentage_lost == 0 {
            return None;
        }

        let seconds_remaining =
            elapsed.as_secs_f64() * f64::from(level_percent) / f64::from(percentage_lost);
        Some(Duration::from_secs_f64(seconds_remaining))
    }

    /// Forgets the in-progress samples of devices that were not seen. Their
    /// long-run rate stays; the next sample just starts a new baseline.
    pub(super) fn retain_devices(&mut self, devices: &HashSet<DeviceId>) {
        self.samples
            .retain(|identity, _| devices.contains(identity));
        for (identity, tally) in &mut self.rates {
            if !devices.contains(identity) {
                tally.last = None;
            }
        }
    }

    pub(super) fn remove(&mut self, identity: &DeviceId) {
        self.samples.remove(identity);
        if let Some(tally) = self.rates.get_mut(identity) {
            tally.last = None;
        }
    }
}
pub(super) fn battery_eta_label(reading: &DeviceReading) -> Option<String> {
    if let Some(eta) = reading.estimated_time_remaining {
        return Some(format!("ETA ~{}", format_battery_eta(eta)));
    }
    if reading.battery.level_percent.is_some()
        && matches!(
            reading.battery.state,
            BatteryState::Discharging | BatteryState::Unknown
        )
    {
        return Some("ETA calculating".to_owned());
    }
    None
}
pub(super) fn format_battery_eta(eta: Duration) -> String {
    let total_minutes = eta.as_secs().div_ceil(60);
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;
    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{minutes}m")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use volty_battery::BatteryLevel;
    fn eta_battery(level_percent: u8, state: BatteryState) -> BatteryStatus {
        BatteryStatus {
            level_percent: Some(level_percent),
            level: BatteryLevel::Good,
            raw_level: level_percent,
            state,
            raw_status: None,
            source: "test",
        }
    }
    #[test]
    fn battery_eta_projects_remaining_time_after_the_configured_window() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        let start = Instant::now();
        let window = Duration::from_secs(10 * 60);

        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(80, BatteryState::Unknown),
                start,
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(75, BatteryState::Unknown),
                start + window - Duration::from_secs(60),
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(75, BatteryState::Unknown),
                start + window,
                window,
            ),
            Some(Duration::from_secs(150 * 60))
        );
    }
    #[test]
    fn battery_eta_resets_during_charging_and_after_a_level_increase() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        let start = Instant::now();
        let window = Duration::from_secs(10 * 60);

        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(80, BatteryState::Discharging),
                start,
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(79, BatteryState::Discharging),
                start + Duration::from_secs(5 * 60),
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(79, BatteryState::Charging),
                start + Duration::from_secs(6 * 60),
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(80, BatteryState::Discharging),
                start + Duration::from_secs(10 * 60),
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(82, BatteryState::Discharging),
                start + Duration::from_secs(15 * 60),
                window,
            ),
            None
        );
        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(78, BatteryState::Discharging),
                start + Duration::from_secs(25 * 60),
                window,
            ),
            Some(Duration::from_secs(195 * 60))
        );
    }
    #[test]
    fn persisted_rate_estimates_right_after_start() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        estimator.load_rates(HashMap::from([(
            identity.clone(),
            DischargeRate {
                lost_percent: 20.0,
                alive_seconds: 20.0 * 3600.0,
            },
        )]));

        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(50, BatteryState::Discharging),
                Instant::now(),
                Duration::from_secs(10 * 60),
            ),
            Some(Duration::from_secs(50 * 3600))
        );
    }

    #[test]
    fn thin_history_does_not_produce_a_long_run_estimate() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        estimator.load_rates(HashMap::from([(
            identity.clone(),
            DischargeRate {
                lost_percent: 1.0,
                alive_seconds: 600.0,
            },
        )]));

        assert_eq!(
            estimator.estimate_at(
                &identity,
                &eta_battery(50, BatteryState::Discharging),
                Instant::now(),
                Duration::from_secs(10 * 60),
            ),
            None
        );
    }

    #[test]
    fn time_while_volty_was_not_watching_is_not_counted() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        let start = Instant::now();
        let window = Duration::from_secs(10 * 60);
        for (offset, level) in [(0, 80), (5 * 60, 79), (3 * 3600, 60)] {
            estimator.estimate_at(
                &identity,
                &eta_battery(level, BatteryState::Discharging),
                start + Duration::from_secs(offset),
                window,
            );
        }

        assert_eq!(
            estimator.take_unsaved_rates(true),
            vec![(
                identity,
                DischargeRate {
                    lost_percent: 1.0,
                    alive_seconds: 300.0,
                }
            )]
        );
    }

    #[test]
    fn rates_are_saved_on_level_drops_and_otherwise_only_when_forced() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        let start = Instant::now();
        let feed = |estimator: &mut BatteryEtaEstimator, minutes: u64, level: u8| {
            estimator.estimate_at(
                &identity,
                &eta_battery(level, BatteryState::Discharging),
                start + Duration::from_secs(minutes * 60),
                Duration::from_secs(10 * 60),
            );
        };

        feed(&mut estimator, 0, 80);
        feed(&mut estimator, 1, 80);
        assert!(estimator.take_unsaved_rates(false).is_empty());
        feed(&mut estimator, 2, 79);
        assert_eq!(estimator.take_unsaved_rates(false).len(), 1);
        assert!(estimator.take_unsaved_rates(false).is_empty());
        feed(&mut estimator, 3, 79);
        assert!(estimator.take_unsaved_rates(false).is_empty());
        assert_eq!(estimator.take_unsaved_rates(true).len(), 1);
        assert!(estimator.take_unsaved_rates(true).is_empty());
    }

    #[test]
    fn charging_and_level_increases_restart_the_rate_baseline() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceId::new("hid:receiver#1");
        let start = Instant::now();
        let window = Duration::from_secs(10 * 60);
        for (minutes, level, state) in [
            (0, 80, BatteryState::Discharging),
            (1, 78, BatteryState::Discharging),
            (2, 78, BatteryState::Charging),
            (3, 90, BatteryState::Discharging),
            (4, 89, BatteryState::Discharging),
        ] {
            estimator.estimate_at(
                &identity,
                &eta_battery(level, state),
                start + Duration::from_secs(minutes * 60),
                window,
            );
        }

        assert_eq!(
            estimator.take_unsaved_rates(true),
            vec![(
                identity,
                DischargeRate {
                    lost_percent: 3.0,
                    alive_seconds: 120.0,
                }
            )]
        );
    }

    #[test]
    fn battery_eta_is_rounded_to_human_readable_minutes() {
        assert_eq!(format_battery_eta(Duration::from_secs(90 * 60)), "1h 30m");
        assert_eq!(
            format_battery_eta(Duration::from_secs(25 * 60 * 60)),
            "1d 1h"
        );
    }
}
