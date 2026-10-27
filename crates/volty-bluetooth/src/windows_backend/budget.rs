use std::time::{Duration, Instant};

use crate::BluetoothScan;

const OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const BLUETOOTH_SCAN_BUDGET: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub(super) struct ScanBudget {
    deadline: Instant,
}

impl ScanBudget {
    pub(super) fn new(duration: Duration) -> Self {
        Self {
            deadline: Instant::now() + duration,
        }
    }

    pub(super) fn is_exhausted(self) -> bool {
        self.is_exhausted_at(Instant::now())
    }

    fn is_exhausted_at(self, now: Instant) -> bool {
        now >= self.deadline
    }

    pub(super) fn operation_timeout(self) -> Option<Duration> {
        self.operation_timeout_at(Instant::now())
    }

    fn operation_timeout_at(self, now: Instant) -> Option<Duration> {
        let remaining = self.deadline.saturating_duration_since(now);
        (!remaining.is_zero()).then(|| OPERATION_TIMEOUT.min(remaining))
    }
}

pub(super) fn push_scan_budget_warning(scan: &mut BluetoothScan, phase: &str) {
    scan.warnings.push(format!(
        "{phase} stopped when the shared Bluetooth scan budget of {} seconds was exhausted",
        BLUETOOTH_SCAN_BUDGET.as_secs()
    ));
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{ScanBudget, OPERATION_TIMEOUT};

    #[test]
    fn scan_budget_caps_each_operation_at_five_seconds_and_deadline() {
        let start = Instant::now();
        let budget = ScanBudget {
            deadline: start + Duration::from_secs(30),
        };

        assert_eq!(budget.operation_timeout_at(start), Some(OPERATION_TIMEOUT));
        assert_eq!(
            budget.operation_timeout_at(start + Duration::from_secs(29)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(budget.operation_timeout_at(budget.deadline), None);
        assert!(!budget.is_exhausted_at(start + Duration::from_secs(29)));
        assert!(budget.is_exhausted_at(budget.deadline));
    }
}
