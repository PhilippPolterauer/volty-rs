use crate::{BatteryStatus, HidInterfaceInfo};
use std::error::Error;

/// An error returned by a protocol-specific battery query.
pub type ProtocolError = Box<dyn Error + Send + Sync + 'static>;

/// A transport/protocol implementation that can produce normalized battery readings.
pub trait BatteryProtocol: Sync {
    fn name(&self) -> &'static str;

    fn supports_interface(&self, interface: &HidInterfaceInfo) -> bool;

    /// Protocol-specific device addresses to try, in preferred order.
    fn device_indices(&self) -> &'static [u8];

    fn query_battery(
        &self,
        interface: &HidInterfaceInfo,
        device_index: u8,
    ) -> Result<BatteryStatus, ProtocolError>;
}

/// A mouse family mapped to the protocol used to query its battery.
pub trait MouseModel: Sync {
    fn name(&self) -> &'static str;

    fn protocol(&self) -> &'static dyn BatteryProtocol;

    fn matches(&self, interface: &HidInterfaceInfo) -> bool {
        self.protocol().supports_interface(interface)
    }
}

/// Find the registered mouse family for an enumerated HID interface.
pub fn find_supported_mouse(interface: &HidInterfaceInfo) -> Option<&'static dyn MouseModel> {
    SUPPORTED_MICE
        .iter()
        .copied()
        .find(|mouse| mouse.matches(interface))
}

/// Mouse families currently known to Volty.
pub static SUPPORTED_MICE: &[&dyn MouseModel] = &[&crate::logitech_hidpp::LOGITECH_HIDPP_MOUSE];

/// Protocol implementations currently registered with Volty.
pub static SUPPORTED_PROTOCOLS: &[&dyn BatteryProtocol] =
    &[&crate::logitech_hidpp::LOGITECH_HIDPP_20];

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn interface(vendor_id: u16) -> HidInterfaceInfo {
        HidInterfaceInfo {
            vendor_id,
            product_id: 0xc547,
            release_number: 1,
            usage_page: Some(0xff00),
            usage: Some(2),
            interface_number: 1,
            bus_type: "Usb".to_owned(),
            manufacturer_string: None,
            product_string: Some("HID++ receiver".to_owned()),
            path: CString::new("test").unwrap(),
        }
    }

    #[test]
    fn registry_matches_only_the_registered_logitech_hidpp_family() {
        let matched = interface(0x046d);
        let mouse = find_supported_mouse(&matched).unwrap();
        assert_eq!(mouse.name(), "Logitech HID++ mouse");
        assert_eq!(mouse.protocol().name(), "Logitech HID++ 2.0");
        assert_eq!(SUPPORTED_PROTOCOLS.len(), 1);
        assert_eq!(SUPPORTED_MICE.len(), 1);

        assert!(find_supported_mouse(&interface(0x1234)).is_none());
    }
}
