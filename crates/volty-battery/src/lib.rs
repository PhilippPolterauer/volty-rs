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

/// Normalized battery reading. Unreported values remain unavailable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatteryStatus {
    /// A percentage only when the device advertises percentage support.
    pub level_percent: Option<u8>,
    /// Coarse protocol level bucket, when available.
    pub level: BatteryLevel,
    /// Raw level byte returned by the device protocol.
    pub raw_level: u8,
    pub state: BatteryState,
    /// Raw state byte, when the source reports one.
    pub raw_status: Option<u8>,
    /// Human-readable description of where the reading came from, chosen by the
    /// backend that produced it (for example "HID++ Unified Battery").
    pub source: &'static str,
}
