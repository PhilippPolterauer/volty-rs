//! Small, UI-free helpers for Logitech HID++ 2.0 devices.
//!
//! Frame handling, feature discovery, and battery parsing work without native
//! dependencies. Enable `hidapi` for interface enumeration and sessions, and
//! `profile` for the TOML command profiles used by the `volty hid` diagnostics.

mod battery;
#[cfg(feature = "profile")]
mod command;
mod device;
mod generic_battery;
#[cfg(feature = "hidapi")]
mod hid;
mod interface;
mod logitech_hidpp;
#[cfg(feature = "profile")]
mod profile;
mod protocol;

pub use battery::*;
#[cfg(feature = "profile")]
pub use command::*;
pub use device::*;
pub use generic_battery::{
    GenericHidBattery, GenericHidBatteryDevice, GENERIC_HID_BATTERY, GENERIC_HID_BATTERY_DEVICE,
    SOURCE_HID_BATTERY,
};
#[cfg(feature = "hidapi")]
pub use hid::{
    enumerate_hid_devices, enumerate_hid_interfaces, select_hid_interface, HidDeviceInfo,
    HidppSession,
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

#[cfg(feature = "hidapi")]
pub(crate) use hid::{query_battery_interface, query_device_interface};

#[cfg(test)]
mod tests;
