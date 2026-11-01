//! Transport-neutral battery readings and the provider contract that backends
//! implement. Nothing in this crate knows about HID++, Bluetooth, or Windows.

mod provider;

pub use provider::{
    set_trace_enabled, trace_enabled, BatteryProvider, DeviceId, DeviceLabel, ProviderEvent,
    ProviderPoll, ScanMode, SinkClosed, SourceReading,
};

/// The known charging/discharging condition, or `Unknown` if unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryState {
    Unknown,
    Discharging,
    Charging,
    ChargingNearlyFull,
    Charged,
    ChargingSlow,
    InvalidBattery,
    ThermalError,
    ChargingError,
}

/// Coarse battery-level bucket reported by a device protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryLevel {
    Unknown,
    Critical,
    Low,
    Good,
    Full,
}

/// A percentage estimate's method. Estimates are never interchangeable with a
/// percentage reported directly by the battery source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryPercentageEstimateMethod {
    /// Approximation from a measured voltage and an assumed cell discharge curve.
    VoltageCurve,
    /// Representative percentage mapped from an XInput qualitative bucket.
    XInputBucketMapping,
}

/// Percentage information with its provenance kept explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryPercentage {
    Unavailable,
    Exact(u8),
    Estimated {
        value: u8,
        method: BatteryPercentageEstimateMethod,
    },
}

impl BatteryPercentage {
    /// Returns a display/aggregation value when available. Callers that make
    /// percentage-sensitive decisions must use [`Self::exact`].
    pub const fn value(self) -> Option<u8> {
        match self {
            Self::Unavailable => None,
            Self::Exact(value) | Self::Estimated { value, .. } => Some(value),
        }
    }

    /// Returns only percentages reported directly by the source.
    pub const fn exact(self) -> Option<u8> {
        match self {
            Self::Exact(value) => Some(value),
            Self::Unavailable | Self::Estimated { .. } => None,
        }
    }
}

/// Normalized battery reading. Unreported values remain unavailable. Raw
/// source fields and `source` retain the original value and its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatteryStatus {
    /// Percentage and provenance; use `level` for a native coarse bucket.
    pub percentage: BatteryPercentage,
    /// Coarse battery-level bucket, whether reported natively or derived.
    pub level: BatteryLevel,
    /// Raw level byte returned by the device protocol.
    pub raw_level: u8,
    pub state: BatteryState,
    /// Raw state byte, when the source reports one.
    pub raw_status: Option<u8>,
    /// Measured battery voltage in millivolts, when the source reports it.
    pub voltage_mv: Option<u16>,
    /// Human-readable description of where the reading came from, chosen by the
    /// backend that produced it (for example "HID++ Unified Battery").
    pub source: &'static str,
}
