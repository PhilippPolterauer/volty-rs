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

/// A mouse family mapped to the protocol used to query its battery.
pub trait MouseModel: Sync {
    fn name(&self) -> &'static str;

    fn protocol(&self) -> &'static dyn BatteryProtocol;

    fn matches(&self, interface: &HidInterfaceInfo) -> bool {
        self.protocol().supports_interface(interface)
    }
}

/// An interface matches more than one registered mouse model.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AmbiguousMouseModel {
    /// Names of all matching models, in catalog order.
    pub matching_model_names: Vec<&'static str>,
}

impl fmt::Display for AmbiguousMouseModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "multiple mouse models match this HID interface: {}",
            self.matching_model_names.join(", ")
        )
    }
}

impl Error for AmbiguousMouseModel {}

/// Resolve an enumerated HID interface to its unique registered mouse model.
///
/// Returns `Ok(None)` when no model matches and an error when multiple models
/// match the interface.
pub fn resolve_mouse_model(
    interface: &HidInterfaceInfo,
) -> Result<Option<&'static dyn MouseModel>, AmbiguousMouseModel> {
    resolve_mouse_model_from(SUPPORTED_MICE, interface)
}

fn resolve_mouse_model_from(
    catalog: &[&'static dyn MouseModel],
    interface: &HidInterfaceInfo,
) -> Result<Option<&'static dyn MouseModel>, AmbiguousMouseModel> {
    let mut matches = catalog
        .iter()
        .copied()
        .filter(|mouse| mouse.matches(interface));

    let Some(first) = matches.next() else {
        return Ok(None);
    };

    let Some(second) = matches.next() else {
        return Ok(Some(first));
    };

    let mut matching_model_names = vec![first.name(), second.name()];
    matching_model_names.extend(matches.map(|mouse| mouse.name()));
    Err(AmbiguousMouseModel {
        matching_model_names,
    })
}

/// Mouse families currently known to Volty.
pub static SUPPORTED_MICE: &[&dyn MouseModel] = &[&crate::logitech_hidpp::LOGITECH_HIDPP_MOUSE];

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    struct TestMouseModel {
        name: &'static str,
        vendor_id: u16,
    }

    impl MouseModel for TestMouseModel {
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

    static TEST_MOUSE_A: TestMouseModel = TestMouseModel {
        name: "Test Mouse A",
        vendor_id: 0x1234,
    };
    static TEST_MOUSE_B: TestMouseModel = TestMouseModel {
        name: "Test Mouse B",
        vendor_id: 0x5678,
    };
    static TEST_MOUSE_C: TestMouseModel = TestMouseModel {
        name: "Test Mouse C",
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
            path: CString::new("test").unwrap(),
        }
    }

    #[test]
    fn resolver_returns_none_when_no_catalog_model_matches() {
        let catalog: &[&'static dyn MouseModel] = &[&TEST_MOUSE_A];
        assert!(resolve_mouse_model_from(catalog, &interface(0x9999))
            .unwrap()
            .is_none());
    }

    #[test]
    fn resolver_returns_the_unique_matching_model() {
        let catalog: &[&'static dyn MouseModel] = &[&TEST_MOUSE_A, &TEST_MOUSE_B];
        let matched = resolve_mouse_model_from(catalog, &interface(0x1234))
            .unwrap()
            .unwrap();
        assert_eq!(matched.name(), "Test Mouse A");
    }

    #[test]
    fn resolver_reports_all_ambiguous_matches() {
        let catalog: &[&'static dyn MouseModel] = &[&TEST_MOUSE_A, &TEST_MOUSE_C];
        let error = match resolve_mouse_model_from(catalog, &interface(0x1234)) {
            Err(error) => error,
            Ok(_) => panic!("expected an ambiguous model resolution"),
        };
        assert_eq!(
            error.matching_model_names,
            vec!["Test Mouse A", "Test Mouse C"]
        );
        assert_eq!(
            error.to_string(),
            "multiple mouse models match this HID interface: Test Mouse A, Test Mouse C"
        );
    }

    #[test]
    fn registry_resolves_the_registered_logitech_hidpp_family() {
        let matched = interface(0x046d);
        let mouse = resolve_mouse_model(&matched).unwrap().unwrap();
        assert_eq!(mouse.name(), "Logitech HID++ mouse");
        assert_eq!(mouse.protocol().name(), "Logitech HID++ 2.0");
        assert_eq!(SUPPORTED_MICE.len(), 1);

        let short_report_interface = HidInterfaceInfo {
            usage: Some(0x0001),
            ..matched
        };
        assert!(resolve_mouse_model(&short_report_interface)
            .unwrap()
            .is_none());
        assert!(resolve_mouse_model(&interface(0x1234)).unwrap().is_none());
    }
}
