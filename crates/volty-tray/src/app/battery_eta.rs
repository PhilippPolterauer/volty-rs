use super::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use volty_battery::{BatteryState, BatteryStatus};
#[derive(Default)]
pub(super) struct BatteryEtaEstimator {
    samples: HashMap<DeviceIdentity, VecDeque<BatteryEtaSample>>,
}
#[derive(Clone, Copy)]
struct BatteryEtaSample {
    measured_at: Instant,
    level_percent: u8,
}
impl BatteryEtaEstimator {
    pub(super) fn estimate(
        &mut self,
        identity: &DeviceIdentity,
        battery: &BatteryStatus,
        window: Duration,
    ) -> Option<Duration> {
        self.estimate_at(identity, battery, Instant::now(), window)
    }

    pub(super) fn estimate_at(
        &mut self,
        identity: &DeviceIdentity,
        battery: &BatteryStatus,
        measured_at: Instant,
        window: Duration,
    ) -> Option<Duration> {
        let Some(level_percent) = battery.level_percent else {
            self.samples.remove(identity);
            return None;
        };
        if !matches!(
            battery.state,
            BatteryState::Discharging | BatteryState::Unknown
        ) || window.is_zero()
        {
            self.samples.remove(identity);
            return None;
        }

        let samples = self.samples.entry(identity.clone()).or_default();
        if samples
            .back()
            .is_some_and(|previous| level_percent > previous.level_percent)
        {
            samples.clear();
        }
        samples.push_back(BatteryEtaSample {
            measured_at,
            level_percent,
        });

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

    pub(super) fn retain_devices(&mut self, devices: &HashSet<DeviceIdentity>) {
        self.samples
            .retain(|identity, _| devices.contains(identity));
    }

    pub(super) fn remove(&mut self, identity: &DeviceIdentity) {
        self.samples.remove(identity);
    }

    pub(super) fn clear(&mut self) {
        self.samples.clear();
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
    use super::super::DeviceIdentity;
    use super::*;
    use volty_battery::{BatteryLevel, BatterySource};
    fn eta_battery(level_percent: u8, state: BatteryState) -> BatteryStatus {
        BatteryStatus {
            level_percent: Some(level_percent),
            level: BatteryLevel::Good,
            raw_level: level_percent,
            state,
            raw_status: None,
            source: BatterySource::Unified,
        }
    }
    #[test]
    fn battery_eta_projects_remaining_time_after_the_configured_window() {
        let mut estimator = BatteryEtaEstimator::default();
        let identity = DeviceIdentity::Hid {
            interface_path: "receiver".to_owned(),
            device_index: 1,
        };
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
        let identity = DeviceIdentity::Hid {
            interface_path: "receiver".to_owned(),
            device_index: 1,
        };
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
    fn battery_eta_is_rounded_to_human_readable_minutes() {
        assert_eq!(format_battery_eta(Duration::from_secs(90 * 60)), "1h 30m");
        assert_eq!(
            format_battery_eta(Duration::from_secs(25 * 60 * 60)),
            "1d 1h"
        );
    }
}
