//! UI-free protocol helpers for HID battery devices.
//!
//! Frame handling, feature discovery, battery parsing, and report recording
//! work without native dependencies. Enable `hidapi` for interface
//! enumeration and sessions, and `profile` for TOML command profiles used by
//! the `volty hid` diagnostics.

mod battery;
mod battery_query;
#[cfg(feature = "profile")]
mod command;
mod device;
mod flydigi;
mod flydigi_xinput;
mod generic_battery;
#[cfg(feature = "hidapi")]
mod hid;
mod interface;
mod logitech_hidpp;
#[cfg(feature = "profile")]
mod profile;
mod protocol;
mod recording;
mod report_io;

pub use battery::*;
pub use battery_query::{query_battery_with_transport, query_device_with_transport};
#[cfg(feature = "profile")]
pub use command::*;
pub use device::*;
pub use flydigi::{decode_flydigi_info, FLYDIGI_DEVICE, FLYDIGI_VENDOR, SOURCE_FLYDIGI};
pub use flydigi_xinput::{decode_flydigi_xinput_battery, SOURCE_FLYDIGI_XINPUT};
pub use generic_battery::{
    decode_generic_battery, GenericBatteryButton, GenericBatteryButtonReading,
    GenericBatteryLayout, GenericBatteryProbe, GenericBatterySample, GenericBatteryValue,
    GenericBatteryValueReading, GenericHidBattery, GenericHidBatteryDevice, GenericReportKind,
    GENERIC_HID_BATTERY, GENERIC_HID_BATTERY_DEVICE, SOURCE_HID_BATTERY,
};
#[cfg(feature = "hidapi")]
pub use hid::{
    enumerate_hid_devices, enumerate_hid_interfaces, select_hid_interface, HidDeviceInfo,
    HidReportSession, HidppSession,
};
pub use interface::{
    select_hid_interface_from, stable_interface_keys, HidInterfaceInfo, HidInterfaceSelector,
};
pub use logitech_hidpp::*;
#[cfg(feature = "profile")]
pub use profile::*;
pub use protocol::{
    resolve_hid_device, AmbiguousHidDevice, BatteryProtocol, HidBatteryDevice,
    ProtocolDeviceReading, ProtocolError, SUPPORTED_HID_DEVICES,
};
pub use recording::{HidppExchange, RecordingTransport};
pub use report_io::{
    HidReportDirection, HidReportEvent, HidReportIo, HidReportIoError, RecordingHidIo,
};

#[cfg(feature = "hidapi")]
pub(crate) use hid::{query_battery_interface, query_device_interface};

#[cfg(test)]
mod tests;
