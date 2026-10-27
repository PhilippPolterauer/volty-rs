use super::*;
use std::collections::VecDeque;
use std::ffi::CString;
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
        select_hid_interface_from(std::slice::from_ref(&interface), &selector).unwrap(),
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
fn device_name_feature_reads_fragmented_names() {
    let expected_name = "Logitech G502 X Plus";
    let name_bytes = expected_name.as_bytes();
    let mut transport = MockTransport::default();
    transport.responses.push_back(root_reply(1, 1, 7, 0, 1));
    let mut length_params = [0; 16];
    length_params[0] = name_bytes.len() as u8;
    transport
        .responses
        .push_back(HidppFrame::long(1, 7, 0, 2, length_params));
    let mut first_fragment = [0; 16];
    first_fragment.copy_from_slice(&name_bytes[..16]);
    transport
        .responses
        .push_back(HidppFrame::long(1, 7, 1, 3, first_fragment));
    let mut second_fragment = [0; 16];
    second_fragment[..name_bytes.len() - 16].copy_from_slice(&name_bytes[16..]);
    transport
        .responses
        .push_back(HidppFrame::long(1, 7, 1, 4, second_fragment));

    let name = query_device_name(
        &mut transport,
        1,
        &mut SoftwareIdAllocator::new(),
        Duration::from_secs(1),
    )
    .unwrap();

    assert_eq!(name.as_deref(), Some(expected_name));
    assert_eq!(transport.requests[0].params[..2], [0, 5]);
    assert_eq!(
        transport.requests[2].function_id,
        FUNCTION_GET_DEVICE_NAME_FRAGMENT
    );
    assert_eq!(transport.requests[2].params[0], 0);
    assert_eq!(transport.requests[3].params[0], 16);
}

#[test]
fn device_name_feature_returns_none_when_not_advertised() {
    let mut transport = MockTransport::default();
    transport.responses.push_back(root_reply(1, 1, 0, 0, 0));

    assert_eq!(
        query_device_name(
            &mut transport,
            1,
            &mut SoftwareIdAllocator::new(),
            Duration::from_secs(1),
        )
        .unwrap(),
        None
    );
    assert_eq!(transport.requests.len(), 1);
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
        enumerate_features(&mut transport, 1, &mut software_ids, Duration::from_secs(1)).unwrap();
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
