//! Small, UI-free helpers for Logitech HID++ 2.0 devices.
//!
//! Frame handling, feature discovery, and battery parsing work without native
//! dependencies. Enable `hidapi` for interface enumeration and sessions.

mod battery;
mod builtin_protocols;
mod command;
mod logitech_hidpp;
mod profile;
mod protocol;

pub use battery::*;
pub use builtin_protocols::{builtin_profile_names, builtin_profile_source};
pub use command::*;
pub use logitech_hidpp::*;
pub use profile::*;
pub use protocol::{
    resolve_mouse_model, AmbiguousMouseModel, BatteryProtocol, MouseModel, ProtocolError,
    SUPPORTED_MICE,
};

#[cfg(feature = "hidapi")]
use logitech_hidpp::{transact_io, ReportIo};
use std::ffi::CString;
use std::fmt;

/// Metadata for one enumerated HID interface. The path is intentionally omitted
/// from debug output; applications may display it explicitly when needed.
#[derive(Clone, Eq, PartialEq)]
pub struct HidInterfaceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub release_number: u16,
    pub usage_page: Option<u16>,
    pub usage: Option<u16>,
    pub interface_number: i32,
    pub bus_type: String,
    pub manufacturer_string: Option<String>,
    pub product_string: Option<String>,
    pub path: CString,
}

impl fmt::Debug for HidInterfaceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HidInterfaceInfo")
            .field("vendor_id", &self.vendor_id)
            .field("product_id", &self.product_id)
            .field("release_number", &self.release_number)
            .field("usage_page", &self.usage_page)
            .field("usage", &self.usage)
            .field("interface_number", &self.interface_number)
            .field("bus_type", &self.bus_type)
            .field("manufacturer_string", &self.manufacturer_string)
            .field("product_string", &self.product_string)
            .field("path", &"<redacted>")
            .finish()
    }
}

/// Optional exact-match constraints for choosing a single HID interface.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct HidInterfaceSelector {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub usage_page: Option<u16>,
    pub usage: Option<u16>,
    pub interface_number: Option<i32>,
    pub path: Option<String>,
}

impl HidInterfaceSelector {
    pub fn matches(&self, interface: &HidInterfaceInfo) -> bool {
        self.vendor_id
            .is_none_or(|value| value == interface.vendor_id)
            && self
                .product_id
                .is_none_or(|value| value == interface.product_id)
            && self
                .usage_page
                .is_none_or(|value| Some(value) == interface.usage_page)
            && self
                .usage
                .is_none_or(|value| Some(value) == interface.usage)
            && self
                .interface_number
                .is_none_or(|value| value == interface.interface_number)
            && self
                .path
                .as_ref()
                .is_none_or(|value| value.as_bytes() == interface.path.as_bytes())
    }

    fn description(&self) -> String {
        format!(
            "VID={:?}, PID={:?}, usage_page={:?}, usage={:?}, interface={:?}, path={}",
            self.vendor_id,
            self.product_id,
            self.usage_page,
            self.usage,
            self.interface_number,
            if self.path.is_some() {
                "<specified>"
            } else {
                "<any>"
            }
        )
    }
}

impl fmt::Debug for HidInterfaceSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HidInterfaceSelector")
            .field("vendor_id", &self.vendor_id)
            .field("product_id", &self.product_id)
            .field("usage_page", &self.usage_page)
            .field("usage", &self.usage)
            .field("interface_number", &self.interface_number)
            .field("path", &self.path.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Select exactly one item from an inventory, rejecting zero or ambiguous matches.
pub fn select_hid_interface_from(
    interfaces: &[HidInterfaceInfo],
    selector: &HidInterfaceSelector,
) -> Result<HidInterfaceInfo, HidppError> {
    let mut matches: Vec<_> = interfaces
        .iter()
        .filter(|interface| selector.matches(interface))
        .collect();
    matches.sort_by(|left, right| left.path.cmp(&right.path));
    match matches.len() {
        0 => Err(HidppError::NoMatchingInterface {
            selector: selector.description(),
        }),
        1 => Ok(matches[0].clone()),
        _ => Err(HidppError::AmbiguousInterface {
            selector: selector.description(),
            matching_paths: matches
                .into_iter()
                .map(|item| item.path.to_string_lossy().into_owned())
                .collect(),
        }),
    }
}

#[cfg(feature = "hidapi")]
mod hid {
    use super::*;
    use hidapi::{DeviceInfo, HidApi, HidDevice};
    use std::time::Duration;

    /// Generic metadata for a HID interface, excluding its path and serial number.
    #[derive(Clone, Debug)]
    pub struct HidDeviceInfo {
        pub vendor_id: u16,
        pub product_id: u16,
        pub release_number: u16,
        pub usage_page: Option<u16>,
        pub usage: Option<u16>,
        pub interface_number: i32,
        pub bus_type: String,
        pub manufacturer_string: Option<String>,
        pub product_string: Option<String>,
    }

    impl HidDeviceInfo {
        fn from_interface(info: &HidInterfaceInfo) -> Self {
            Self {
                vendor_id: info.vendor_id,
                product_id: info.product_id,
                release_number: info.release_number,
                usage_page: info.usage_page,
                usage: info.usage,
                interface_number: info.interface_number,
                bus_type: info.bus_type.clone(),
                manufacturer_string: info.manufacturer_string.clone(),
                product_string: info.product_string.clone(),
            }
        }
    }

    impl HidInterfaceInfo {
        fn from_hidapi(info: &DeviceInfo) -> Self {
            #[cfg(windows)]
            let (usage_page, usage) = (Some(info.usage_page()), Some(info.usage()));
            #[cfg(not(windows))]
            let (usage_page, usage) = (None, None);

            Self {
                vendor_id: info.vendor_id(),
                product_id: info.product_id(),
                release_number: info.release_number(),
                usage_page,
                usage,
                interface_number: info.interface_number(),
                bus_type: format!("{:?}", info.bus_type()),
                manufacturer_string: info.manufacturer_string().map(str::to_owned),
                product_string: info.product_string().map(str::to_owned),
                path: info.path().to_owned(),
            }
        }
    }

    /// Enumerate all visible HID interfaces in deterministic order.
    pub fn enumerate_hid_interfaces() -> Result<Vec<HidInterfaceInfo>, HidppError> {
        let api = HidApi::new().map_err(HidppError::HidApi)?;
        let mut interfaces: Vec<_> = api
            .device_list()
            .map(HidInterfaceInfo::from_hidapi)
            .collect();
        interfaces.sort_by(|left, right| {
            (
                left.vendor_id,
                left.product_id,
                left.interface_number,
                left.usage_page,
                left.usage,
                &left.path,
            )
                .cmp(&(
                    right.vendor_id,
                    right.product_id,
                    right.interface_number,
                    right.usage_page,
                    right.usage,
                    &right.path,
                ))
        });
        Ok(interfaces)
    }

    /// Select exactly one enumerated interface; zero or multiple matches are errors.
    pub fn select_hid_interface(
        selector: &HidInterfaceSelector,
    ) -> Result<HidInterfaceInfo, HidppError> {
        let interfaces = enumerate_hid_interfaces()?;
        select_hid_interface_from(&interfaces, selector)
    }

    /// A HID++ transport session bound to one explicitly selected interface.
    pub struct HidppSession {
        device: HidDevice,
        info: HidInterfaceInfo,
        software_ids: SoftwareIdAllocator,
    }

    impl HidppSession {
        pub fn open(selector: &HidInterfaceSelector) -> Result<Self, HidppError> {
            let api = HidApi::new().map_err(HidppError::HidApi)?;
            let interfaces: Vec<_> = api
                .device_list()
                .map(HidInterfaceInfo::from_hidapi)
                .collect();
            let info = select_hid_interface_from(&interfaces, selector)?;
            let device = api.open_path(&info.path).map_err(HidppError::HidApi)?;
            Ok(Self {
                device,
                info,
                software_ids: SoftwareIdAllocator::new(),
            })
        }

        pub fn info(&self) -> &HidInterfaceInfo {
            &self.info
        }

        /// Look up a logical feature through Root while preserving its metadata.
        pub fn get_feature(
            &mut self,
            device_index: u8,
            logical_feature_id: u16,
            timeout: Duration,
        ) -> Result<Option<FeatureInfo>, HidppError> {
            let software_id = self.software_ids.allocate();
            get_feature(self, device_index, logical_feature_id, software_id, timeout)
        }

        /// Enumerate the device's advertised features via FeatureSet and Root.
        pub fn enumerate_features(
            &mut self,
            device_index: u8,
            timeout: Duration,
        ) -> Result<Vec<(u16, FeatureInfo)>, HidppError> {
            let mut software_ids = self.software_ids.clone();
            let result = enumerate_features(self, device_index, &mut software_ids, timeout);
            self.software_ids = software_ids;
            result
        }
    }

    impl ReportIo for HidDevice {
        fn write_output(&mut self, bytes: &[u8]) -> Result<usize, HidppError> {
            self.write(bytes).map_err(HidppError::HidApi)
        }

        fn read_timeout(
            &mut self,
            buffer: &mut [u8],
            timeout: Duration,
        ) -> Result<usize, HidppError> {
            let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
            if timeout_ms == 0 {
                return Ok(0);
            }
            HidDevice::read_timeout(self, buffer, timeout_ms).map_err(HidppError::HidApi)
        }
    }

    impl HidppTransport for HidppSession {
        fn transact(
            &mut self,
            request: &HidppFrame,
            timeout: Duration,
        ) -> Result<HidppFrame, HidppError> {
            transact_io(&mut self.device, request, timeout)
        }
    }

    /// Enumerate all HID interfaces visible to hidapi, regardless of vendor or bus.
    pub fn enumerate_hid_devices() -> Result<Vec<HidDeviceInfo>, HidppError> {
        enumerate_hid_interfaces().map(|interfaces| {
            interfaces
                .iter()
                .map(HidDeviceInfo::from_interface)
                .collect()
        })
    }

    pub(crate) fn query_battery_interface(
        info: &HidInterfaceInfo,
        device_index: u8,
    ) -> Result<BatteryStatus, HidppError> {
        let api = HidApi::new().map_err(HidppError::HidApi)?;
        let device = api.open_path(&info.path).map_err(HidppError::HidApi)?;
        let mut session = HidppSession {
            device,
            info: info.clone(),
            software_ids: SoftwareIdAllocator::new(),
        };

        if let Some(feature) = session.get_feature(
            device_index,
            FEATURE_UNIFIED_BATTERY,
            Duration::from_secs(2),
        )? {
            let capabilities = transact_feature(
                &mut session,
                device_index,
                feature.feature_index,
                FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES,
                Duration::from_secs(2),
            )?;
            let capabilities = parse_unified_battery_capabilities(&capabilities)?;
            let response = transact_feature(
                &mut session,
                device_index,
                feature.feature_index,
                FUNCTION_GET_UNIFIED_BATTERY_INFO,
                Duration::from_secs(2),
            )?;
            return parse_unified_battery(&response, capabilities);
        }
        if let Some(feature) =
            session.get_feature(device_index, FEATURE_BATTERY_STATUS, Duration::from_secs(2))?
        {
            let response = transact_feature(
                &mut session,
                device_index,
                feature.feature_index,
                FUNCTION_GET_BATTERY_STATUS,
                Duration::from_secs(2),
            )?;
            return parse_legacy_battery_status(&response);
        }
        Err(HidppError::UnsupportedBatteryFeature)
    }

    fn transact_feature(
        session: &mut HidppSession,
        device_index: u8,
        feature_index: u8,
        function_id: u8,
        timeout: Duration,
    ) -> Result<HidppFrame, HidppError> {
        let request = HidppFrame::long(
            device_index,
            feature_index,
            function_id,
            session.software_ids.allocate(),
            [0; 16],
        );
        session.transact(&request, timeout)
    }
}

/// Query a logical feature through Root using a caller-selected software ID.
pub fn get_feature(
    transport: &mut impl HidppTransport,
    device_index: u8,
    logical_feature_id: u16,
    software_id: u8,
    timeout: std::time::Duration,
) -> Result<Option<FeatureInfo>, HidppError> {
    let mut params = [0; 16];
    params[0] = (logical_feature_id >> 8) as u8;
    params[1] = logical_feature_id as u8;
    let request = HidppFrame::long(
        device_index,
        FEATURE_ROOT as u8,
        FUNCTION_GET_FEATURE,
        software_id,
        params,
    );
    let response = transport.transact(&request, timeout)?;
    verify_response(&request, &response)?;
    let feature_index = *response
        .params
        .first()
        .ok_or(HidppError::UnexpectedResponse(
            "missing feature index in Root response",
        ))?;
    if feature_index == 0 && logical_feature_id != FEATURE_ROOT {
        return Ok(None);
    }
    if response.params.len() < 3 {
        return Err(HidppError::UnexpectedResponse(
            "Root response is missing feature type or version",
        ));
    }
    Ok(Some(FeatureInfo {
        feature_index,
        feature_type: response.params[1],
        version: response.params[2],
    }))
}

/// Enumerate FeatureSet entries and resolve each through Root for its runtime index.
///
/// `software_ids` is caller-owned so command engines can share their allocator.
pub fn enumerate_features(
    transport: &mut impl HidppTransport,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
    timeout: std::time::Duration,
) -> Result<Vec<(u16, FeatureInfo)>, HidppError> {
    let feature_set = get_feature(
        transport,
        device_index,
        FEATURE_SET,
        software_ids.allocate(),
        timeout,
    )?
    .ok_or(HidppError::UnsupportedFeatureSet)?;
    let count_request = HidppFrame::long(
        device_index,
        feature_set.feature_index,
        FUNCTION_GET_FEATURE_SET_COUNT,
        software_ids.allocate(),
        [0; 16],
    );
    let count_response = transport.transact(&count_request, timeout)?;
    verify_response(&count_request, &count_response)?;
    let count = *count_response
        .params
        .first()
        .ok_or(HidppError::UnexpectedResponse(
            "FeatureSet count response is missing its count",
        ))?;

    let mut features = Vec::with_capacity(usize::from(count));
    for entry_index in 0..count {
        let mut params = [0; 16];
        params[0] = entry_index;
        let request = HidppFrame::long(
            device_index,
            feature_set.feature_index,
            FUNCTION_GET_FEATURE_SET_ID,
            software_ids.allocate(),
            params,
        );
        let response = transport.transact(&request, timeout)?;
        verify_response(&request, &response)?;
        if response.params.len() < 4 {
            return Err(HidppError::UnexpectedResponse(
                "FeatureSet entry is missing feature ID, type, or version",
            ));
        }
        let logical_id = u16::from_be_bytes([response.params[0], response.params[1]]);
        let feature = get_feature(
            transport,
            device_index,
            logical_id,
            software_ids.allocate(),
            timeout,
        )?
        .ok_or(HidppError::UnexpectedResponse(
            "FeatureSet entry is not present in Root",
        ))?;
        features.push((logical_id, feature));
    }
    Ok(features)
}

fn verify_response(request: &HidppFrame, response: &HidppFrame) -> Result<(), HidppError> {
    if response.device_index == request.device_index
        && response.feature_index == request.feature_index
        && response.function_id == request.function_id
        && response.software_id == request.software_id
    {
        Ok(())
    } else {
        Err(HidppError::UnexpectedResponse(
            "transport returned a response that does not match the request",
        ))
    }
}

#[cfg(feature = "hidapi")]
pub use hid::{
    enumerate_hid_devices, enumerate_hid_interfaces, select_hid_interface, HidDeviceInfo,
    HidppSession,
};

#[cfg(feature = "hidapi")]
pub(crate) use hid::query_battery_interface;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Duration;

    #[derive(Default)]
    struct MockTransport {
        requests: Vec<HidppFrame>,
        responses: VecDeque<HidppFrame>,
    }

    impl HidppTransport for MockTransport {
        fn transact(
            &mut self,
            request: &HidppFrame,
            _timeout: Duration,
        ) -> Result<HidppFrame, HidppError> {
            self.requests.push(request.clone());
            self.responses.pop_front().ok_or(HidppError::Timeout)
        }
    }

    fn root_reply(
        device_index: u8,
        software_id: u8,
        feature_index: u8,
        feature_type: u8,
        version: u8,
    ) -> HidppFrame {
        let mut params = [0; 16];
        params[..3].copy_from_slice(&[feature_index, feature_type, version]);
        HidppFrame::long(device_index, 0, 0, software_id, params)
    }

    #[test]
    fn interface_selectors_match_explicit_metadata_without_debugging_paths() {
        let interface = HidInterfaceInfo {
            vendor_id: 0x046d,
            product_id: 0xc547,
            release_number: 1,
            usage_page: Some(0xff00),
            usage: Some(2),
            interface_number: 3,
            bus_type: "Usb".to_owned(),
            manufacturer_string: Some("Logitech".to_owned()),
            product_string: Some("Receiver".to_owned()),
            path: CString::new("private-hid-path").unwrap(),
        };
        let selector = HidInterfaceSelector {
            vendor_id: Some(0x046d),
            product_id: Some(0xc547),
            usage_page: Some(0xff00),
            usage: Some(2),
            interface_number: Some(3),
            path: Some("private-hid-path".to_owned()),
        };
        assert!(selector.matches(&interface));
        assert!(!HidInterfaceSelector {
            usage: Some(1),
            ..selector.clone()
        }
        .matches(&interface));
        assert!(!format!("{interface:?}").contains("private-hid-path"));
        assert!(!format!("{selector:?}").contains("private-hid-path"));
        assert_eq!(
            select_hid_interface_from(&[interface.clone()], &selector).unwrap(),
            interface
        );

        let second = HidInterfaceInfo {
            path: CString::new("another-hid-path").unwrap(),
            ..interface.clone()
        };
        let broad_selector = HidInterfaceSelector {
            path: None,
            ..selector.clone()
        };
        assert!(matches!(
            select_hid_interface_from(&[interface.clone(), second], &broad_selector),
            Err(HidppError::AmbiguousInterface {
                matching_paths,
                ..
            }) if matching_paths == ["another-hid-path", "private-hid-path"]
        ));
        assert!(matches!(
            select_hid_interface_from(
                &[interface],
                &HidInterfaceSelector {
                    product_id: Some(0xffff),
                    ..HidInterfaceSelector::default()
                }
            ),
            Err(HidppError::NoMatchingInterface { .. })
        ));
    }

    #[test]
    fn observed_root_and_battery_frames_match_exact_trace() {
        let mut params = [0; 16];
        params[..2].copy_from_slice(&[0x00, 0x03]);
        let root = HidppFrame::long(1, 0, 0, 9, params);
        assert_eq!(
            root.encode().unwrap(),
            [
                0x11, 0x01, 0x00, 0x09, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00
            ]
        );
        let root_response = HidppFrame::parse(&[
            0x11, 0x01, 0x00, 0x09, 0x02, 0x00, 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
        .unwrap();
        assert_eq!(root_response, root_reply(1, 9, 2, 0, 4));
        assert_eq!(
            HidppFrame::long(1, 6, 1, 3, [0; 16]).encode().unwrap(),
            [
                0x11, 0x01, 0x06, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00
            ]
        );
        let battery_reply = HidppFrame::parse(&[
            0x11, 0x01, 0x06, 0x13, 0x4e, 0x08, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ])
        .unwrap();
        assert_eq!(&battery_reply.params[..3], &[78, 8, 0]);
    }

    #[test]
    fn root_feature_lookup_preserves_type_and_version_and_big_endian_id() {
        let mut transport = MockTransport::default();
        transport.responses.push_back(root_reply(1, 9, 2, 0, 4));
        let feature = get_feature(&mut transport, 1, 0x0003, 9, Duration::from_secs(1)).unwrap();
        assert_eq!(
            feature,
            Some(FeatureInfo {
                feature_index: 2,
                feature_type: 0,
                version: 4,
            })
        );
        assert_eq!(
            transport.requests[0].encode().unwrap(),
            [0x11, 1, 0, 9, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn root_feature_itself_is_a_valid_index_zero_mapping() {
        let mut transport = MockTransport::default();
        transport.responses.push_back(root_reply(1, 9, 0, 0, 4));
        assert_eq!(
            get_feature(&mut transport, 1, FEATURE_ROOT, 9, Duration::from_secs(1)).unwrap(),
            Some(FeatureInfo {
                feature_index: 0,
                feature_type: 0,
                version: 4,
            })
        );
    }

    #[test]
    fn unsupported_root_feature_returns_none() {
        let mut transport = MockTransport::default();
        transport.responses.push_back(root_reply(1, 2, 0, 0, 0));
        assert_eq!(
            get_feature(
                &mut transport,
                1,
                FEATURE_UNIFIED_BATTERY,
                2,
                Duration::from_secs(1)
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn feature_set_enumeration_uses_advertised_ids_and_root_runtime_metadata() {
        let mut transport = MockTransport::default();
        transport.responses.extend([
            root_reply(1, 1, 5, 0, 1),
            {
                let mut params = [0; 16];
                params[0] = 3;
                HidppFrame::long(1, 5, 0, 2, params)
            },
            {
                let mut params = [0; 16];
                params[..4].copy_from_slice(&[0x00, 0x00, 0, 4]);
                HidppFrame::long(1, 5, 1, 3, params)
            },
            root_reply(1, 4, 0, 0, 4),
            {
                let mut params = [0; 16];
                params[..4].copy_from_slice(&[0x00, 0x03, 0, 4]);
                HidppFrame::long(1, 5, 1, 5, params)
            },
            root_reply(1, 6, 2, 0, 4),
            {
                let mut params = [0; 16];
                params[..4].copy_from_slice(&[0x10, 0x04, 0, 3]);
                HidppFrame::long(1, 5, 1, 7, params)
            },
            root_reply(1, 8, 6, 0, 3),
        ]);
        let mut software_ids = SoftwareIdAllocator::new();
        let features =
            enumerate_features(&mut transport, 1, &mut software_ids, Duration::from_secs(1))
                .unwrap();
        assert_eq!(
            features,
            [
                (
                    FEATURE_ROOT,
                    FeatureInfo {
                        feature_index: 0,
                        feature_type: 0,
                        version: 4
                    }
                ),
                (
                    0x0003,
                    FeatureInfo {
                        feature_index: 2,
                        feature_type: 0,
                        version: 4
                    }
                ),
                (
                    0x1004,
                    FeatureInfo {
                        feature_index: 6,
                        feature_type: 0,
                        version: 3
                    }
                )
            ]
        );
        assert_eq!(transport.requests.len(), 8);
        assert_eq!(transport.requests[0].params[..2], [0, 1]);
        assert_eq!(transport.requests[2].params[0], 0);
        assert_eq!(transport.requests[4].params[0], 1);
        assert_eq!(transport.requests[6].params[0], 2);
    }

    #[test]
    fn feature_set_unsupported_is_explicit() {
        let mut transport = MockTransport::default();
        transport.responses.push_back(root_reply(1, 1, 0, 0, 0));
        assert!(matches!(
            enumerate_features(
                &mut transport,
                1,
                &mut SoftwareIdAllocator::new(),
                Duration::from_secs(1)
            ),
            Err(HidppError::UnsupportedFeatureSet)
        ));
    }

    #[test]
    fn short_frame_round_trips() {
        let frame = HidppFrame::short(0xff, 0x07, 0x02, 0x05, [0x12, 0x34, 0x56]);
        let encoded = frame.encode().unwrap();
        assert_eq!(encoded, [0x10, 0xff, 0x07, 0x25, 0x12, 0x34, 0x56]);
        assert_eq!(HidppFrame::parse(&encoded).unwrap(), frame);
    }

    #[test]
    fn long_frame_round_trips() {
        let params = [0xa5; 16];
        let frame = HidppFrame::long(1, 0x42, 0x03, 0x07, params);
        let encoded = frame.encode().unwrap();
        assert_eq!(encoded.len(), 20);
        assert_eq!(&encoded[..5], [0x11, 1, 0x42, 0x37, 0xa5]);
        assert_eq!(HidppFrame::parse(&encoded).unwrap(), frame);
    }

    #[test]
    fn invalid_frames_return_clear_errors() {
        assert!(matches!(
            HidppFrame::parse(&[0x10, 0, 0]),
            Err(HidppError::InvalidFrameLength { .. })
        ));
        assert!(matches!(
            HidppFrame::parse(&[0x12, 0, 0, 0, 0, 0, 0]),
            Err(HidppError::InvalidReportId(0x12))
        ));
        assert!(matches!(
            HidppFrame::parse(&[0x11; 19]),
            Err(HidppError::InvalidFrameLength {
                expected: 20,
                actual: 19
            })
        ));
        let mut invalid = HidppFrame::short(1, 1, 0, 1, [0; 3]);
        invalid.params.pop();
        assert!(matches!(
            invalid.encode(),
            Err(HidppError::InvalidPayloadLength {
                expected: 3,
                actual: 2
            })
        ));
    }

    #[test]
    fn unified_battery_capabilities_are_decoded() {
        let response = HidppFrame::short(1, 0x05, 0x00, 0x01, [0x0f, 0x03, 0]);
        assert_eq!(
            parse_unified_battery_capabilities(&response).unwrap(),
            UnifiedBatteryCapabilities {
                percentage: true,
                rechargeable: true,
                reported_levels: 0x0f,
            }
        );
    }

    #[test]
    fn unified_battery_info_is_normalized() {
        let response = HidppFrame::short(1, 0x05, 0x01, 0x01, [87, 0x04, 0x01]);
        let battery = parse_unified_battery(
            &response,
            UnifiedBatteryCapabilities {
                percentage: true,
                rechargeable: true,
                reported_levels: 0x0f,
            },
        )
        .unwrap();
        assert_eq!(battery.level_percent, Some(87));
        assert_eq!(battery.level, BatteryLevel::Good);
        assert_eq!(battery.state, BatteryState::Charging);
        assert_eq!(battery.source, BatterySource::Unified);
    }

    #[test]
    fn legacy_battery_status_is_normalized() {
        let response = HidppFrame::short(1, 0x03, 0, 0x01, [42, 40, 3]);
        let battery = parse_legacy_battery_status(&response).unwrap();
        assert_eq!(battery.level_percent, None);
        assert_eq!(battery.raw_level, 42);
        assert_eq!(battery.state, BatteryState::Charged);
        assert_eq!(battery.source, BatterySource::LegacyBatteryStatus);
    }

    #[test]
    fn unified_battery_handles_no_percentage_support_and_invalid_levels() {
        let response = HidppFrame::short(1, 4, 1, 1, [0, 8, 3]);
        let parsed = parse_unified_battery(
            &response,
            UnifiedBatteryCapabilities {
                percentage: false,
                rechargeable: false,
                reported_levels: 0x08,
            },
        )
        .unwrap();
        assert_eq!(parsed.level_percent, None);
        assert_eq!(parsed.level, BatteryLevel::Full);
        assert_eq!(parsed.state, BatteryState::Charged);

        let invalid = HidppFrame::short(1, 4, 1, 1, [101, 1, 0]);
        let capabilities = UnifiedBatteryCapabilities {
            percentage: true,
            rechargeable: true,
            reported_levels: 0x0f,
        };
        assert!(matches!(
            parse_unified_battery(&invalid, capabilities),
            Err(HidppError::InvalidBatteryLevel(101))
        ));
        assert!(parse_unified_battery(
            &HidppFrame::short(1, 4, 1, 1, [1, 1, 0]),
            UnifiedBatteryCapabilities {
                percentage: true,
                rechargeable: true,
                reported_levels: 0x0f,
            }
        )
        .is_ok());
    }
}
