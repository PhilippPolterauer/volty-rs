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

/// Origin of a parsed battery status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatterySource {
    Unified,
    LegacyBatteryStatus,
    BluetoothGatt,
    BluetoothDeviceProperty,
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
    pub source: BatterySource,
}
