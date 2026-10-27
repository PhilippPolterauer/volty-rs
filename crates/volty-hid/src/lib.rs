//! Small, UI-free helpers for Logitech HID++ 2.0 devices.
//!
//! Frame handling, feature discovery, and battery parsing work without native
//! dependencies. Enable `hidapi` for interface enumeration and sessions.

mod battery;
mod command;
mod device;
#[cfg(feature = "hidapi")]
mod hid;
mod interface;
mod logitech_hidpp;
mod profile;
mod protocol;

pub use battery::*;
pub use command::*;
pub use device::*;
#[cfg(feature = "hidapi")]
pub use hid::{
    enumerate_hid_devices, enumerate_hid_interfaces, select_hid_interface, HidDeviceInfo,
    HidppSession,
};
pub use interface::{select_hid_interface_from, HidInterfaceInfo, HidInterfaceSelector};
pub use logitech_hidpp::*;
pub use profile::*;
pub use protocol::{
    resolve_mouse_model, AmbiguousMouseModel, BatteryProtocol, MouseModel, ProtocolDeviceReading,
    ProtocolError, SUPPORTED_MICE,
};

#[cfg(feature = "hidapi")]
pub(crate) use hid::{query_battery_interface, query_device_interface};

#[cfg(test)]
mod tests;
