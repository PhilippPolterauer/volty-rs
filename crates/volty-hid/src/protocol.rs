use crate::{BatteryStatus, HidInterfaceInfo};
use std::error::Error;
use std::fmt;

/// An error returned by a protocol-specific battery query.
pub type ProtocolError = Box<dyn Error + Send + Sync>;

/// A battery reading with a best-effort device name from the protocol.
pub struct ProtocolDeviceReading {
    pub name: Result<Option<String>, ProtocolError>,
    pub battery: BatteryStatus,
}

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

    /// Queries identity before battery details where the protocol supports it.
    ///
    /// A name-discovery failure is kept separate so it does not discard a
    /// battery reading that was successfully obtained.
    fn query_device(
        &self,
        interface: &HidInterfaceInfo,
        device_index: u8,
    ) -> Result<ProtocolDeviceReading, ProtocolError> {
        Ok(ProtocolDeviceReading {
            name: Ok(None),
            battery: self.query_battery(interface, device_index)?,
        })
    }
}

/// A HID device family mapped to the protocol used to query its battery.
pub trait HidBatteryDevice: Sync {
    fn name(&self) -> &'static str;

    fn protocol(&self) -> &'static dyn BatteryProtocol;

    fn matches(&self, interface: &HidInterfaceInfo) -> bool {
        self.protocol().supports_interface(interface)
    }
}

/// An interface matches more than one registered HID device family.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AmbiguousHidDevice {
    /// Names of all matching families, in catalog order.
    pub matching_model_names: Vec<&'static str>,
}

impl fmt::Display for AmbiguousHidDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "multiple HID device families match this interface: {}",
            self.matching_model_names.join(", ")
        )
    }
}

impl Error for AmbiguousHidDevice {}

/// Resolve an enumerated HID interface to its unique registered device family.
///
/// Returns `Ok(None)` when no family matches and an error when multiple
/// families match the interface.
pub fn resolve_hid_device(
    interface: &HidInterfaceInfo,
) -> Result<Option<&'static dyn HidBatteryDevice>, AmbiguousHidDevice> {
    resolve_hid_device_from(SUPPORTED_HID_DEVICES, interface)
}

fn resolve_hid_device_from(
    catalog: &[&'static dyn HidBatteryDevice],
    interface: &HidInterfaceInfo,
) -> Result<Option<&'static dyn HidBatteryDevice>, AmbiguousHidDevice> {
    let mut matches = catalog
        .iter()
        .copied()
        .filter(|device| device.matches(interface));

    let Some(first) = matches.next() else {
        return Ok(None);
    };

    let Some(second) = matches.next() else {
        return Ok(Some(first));
    };

    let mut matching_model_names = vec![first.name(), second.name()];
    matching_model_names.extend(matches.map(|device| device.name()));
    Err(AmbiguousHidDevice {
        matching_model_names,
    })
}

/// HID device families currently known to Volty.
pub static SUPPORTED_HID_DEVICES: &[&dyn HidBatteryDevice] =
    &[&crate::logitech_hidpp::LOGITECH_HIDPP_DEVICE];

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    struct TestDevice {
        name: &'static str,
        vendor_id: u16,
    }

    impl HidBatteryDevice for TestDevice {
        fn name(&self) -> &'static str {
            self.name
        }

        fn protocol(&self) -> &'static dyn BatteryProtocol {
            &crate::logitech_hidpp::LOGITECH_HIDPP_20
        }

        fn matches(&self, interface: &HidInterfaceInfo) -> bool {
            self.vendor_id == interface.vendor_id
        }
    }

    static TEST_DEVICE_A: TestDevice = TestDevice {
        name: "Test Device A",
        vendor_id: 0x1234,
    };
    static TEST_DEVICE_B: TestDevice = TestDevice {
        name: "Test Device B",
        vendor_id: 0x5678,
    };
    static TEST_DEVICE_C: TestDevice = TestDevice {
        name: "Test Device C",
        vendor_id: 0x1234,
    };

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
            serial_number: None,
            path: CString::new("test").unwrap(),
        }
    }

    #[test]
    fn resolver_returns_none_when_no_catalog_device_matches() {
        let catalog: &[&'static dyn HidBatteryDevice] = &[&TEST_DEVICE_A];
        assert!(resolve_hid_device_from(catalog, &interface(0x9999))
            .unwrap()
            .is_none());
    }

    #[test]
    fn resolver_returns_the_unique_matching_device() {
        let catalog: &[&'static dyn HidBatteryDevice] = &[&TEST_DEVICE_A, &TEST_DEVICE_B];
        let matched = resolve_hid_device_from(catalog, &interface(0x1234))
            .unwrap()
            .unwrap();
        assert_eq!(matched.name(), "Test Device A");
    }

    #[test]
    fn resolver_reports_all_ambiguous_matches() {
        let catalog: &[&'static dyn HidBatteryDevice] = &[&TEST_DEVICE_A, &TEST_DEVICE_C];
        let error = match resolve_hid_device_from(catalog, &interface(0x1234)) {
            Err(error) => error,
            Ok(_) => panic!("expected an ambiguous device resolution"),
        };
        assert_eq!(
            error.matching_model_names,
            vec!["Test Device A", "Test Device C"]
        );
        assert_eq!(
            error.to_string(),
            "multiple HID device families match this interface: Test Device A, Test Device C"
        );
    }

    #[test]
    fn registry_resolves_the_registered_logitech_hidpp_family() {
        let matched = interface(0x046d);
        let device = resolve_hid_device(&matched).unwrap().unwrap();
        assert_eq!(device.name(), "Logitech HID++ mouse");
        assert_eq!(device.protocol().name(), "Logitech HID++ 2.0");
        assert_eq!(SUPPORTED_HID_DEVICES.len(), 1);

        let short_report_interface = HidInterfaceInfo {
            usage: Some(0x0001),
            ..matched
        };
        assert!(resolve_hid_device(&short_report_interface)
            .unwrap()
            .is_none());
        assert!(resolve_hid_device(&interface(0x1234)).unwrap().is_none());
    }
}
