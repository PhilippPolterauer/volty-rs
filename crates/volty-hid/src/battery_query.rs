//! Transport-generic HID++ battery discovery and reads.
//!
//! These helpers work with any [`HidppTransport`], so callers can query a live
//! HID++ session or a recorded/replayed transport with the same protocol logic.

use crate::{
    get_feature, parse_legacy_battery_status, parse_unified_battery,
    parse_unified_battery_capabilities, query_device_name, BatteryStatus, HidppError, HidppFrame,
    HidppTransport, SoftwareIdAllocator, UnifiedBatteryCapabilities, FEATURE_BATTERY_STATUS,
    FEATURE_UNIFIED_BATTERY, FUNCTION_GET_BATTERY_STATUS,
    FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES, FUNCTION_GET_UNIFIED_BATTERY_INFO,
};
use std::time::Duration;

/// Only the first Root lookup can hit an empty feature slot.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// Every query after feature discovery gets the full timeout.
pub(crate) const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Where a device keeps its battery, resolved once so later polls skip the
/// Root and capability round trips.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BatteryFeature {
    Unified {
        index: u8,
        capabilities: UnifiedBatteryCapabilities,
    },
    Legacy {
        index: u8,
    },
}

/// Discover and read the battery of `device_index` over any HID++ transport.
///
/// Uses a fresh software ID allocator and the same probe/query timeouts as a
/// live session.
pub fn query_battery_with_transport<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
) -> Result<BatteryStatus, HidppError> {
    query_battery_with_allocator(transport, device_index, &mut SoftwareIdAllocator::new())
}

/// Discover the device name and read the battery over any HID++ transport.
///
/// The name lookup runs before the battery read, and a name failure is kept
/// separate so it does not discard a successful battery reading.
pub fn query_device_with_transport<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
) -> Result<(Result<Option<String>, HidppError>, BatteryStatus), HidppError> {
    query_device_with_allocator(transport, device_index, &mut SoftwareIdAllocator::new())
}

pub(crate) fn query_battery_with_allocator<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
) -> Result<BatteryStatus, HidppError> {
    let feature = discover_battery_feature(transport, device_index, software_ids)?;
    read_battery(transport, device_index, software_ids, feature)
}

pub(crate) fn query_device_with_allocator<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
) -> Result<(Result<Option<String>, HidppError>, BatteryStatus), HidppError> {
    let feature = discover_battery_feature(transport, device_index, software_ids)?;
    let name = query_device_name(transport, device_index, software_ids, QUERY_TIMEOUT);
    let battery = read_battery(transport, device_index, software_ids, feature)?;
    Ok((name, battery))
}

/// Resolve Unified Battery `0x1004` first, then fall back to legacy `0x1000`.
pub(crate) fn discover_battery_feature<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
) -> Result<BatteryFeature, HidppError> {
    // Only the first request can hit an empty slot, so only it uses the short probe timeout.
    if let Some(feature) = get_feature(
        transport,
        device_index,
        FEATURE_UNIFIED_BATTERY,
        software_ids.allocate(),
        PROBE_TIMEOUT,
    )? {
        let response = transact_feature(
            transport,
            device_index,
            feature.feature_index,
            FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES,
            software_ids,
            QUERY_TIMEOUT,
        )?;
        return Ok(BatteryFeature::Unified {
            index: feature.feature_index,
            capabilities: parse_unified_battery_capabilities(&response)?,
        });
    }
    if let Some(feature) = get_feature(
        transport,
        device_index,
        FEATURE_BATTERY_STATUS,
        software_ids.allocate(),
        QUERY_TIMEOUT,
    )? {
        return Ok(BatteryFeature::Legacy {
            index: feature.feature_index,
        });
    }
    Err(HidppError::UnsupportedBatteryFeature)
}

pub(crate) fn read_battery<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
    software_ids: &mut SoftwareIdAllocator,
    feature: BatteryFeature,
) -> Result<BatteryStatus, HidppError> {
    match feature {
        BatteryFeature::Unified {
            index,
            capabilities,
        } => {
            let response = transact_feature(
                transport,
                device_index,
                index,
                FUNCTION_GET_UNIFIED_BATTERY_INFO,
                software_ids,
                QUERY_TIMEOUT,
            )?;
            parse_unified_battery(&response, capabilities)
        }
        BatteryFeature::Legacy { index } => {
            read_legacy_battery(transport, device_index, index, software_ids)
        }
    }
}

fn read_legacy_battery<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
    feature_index: u8,
    software_ids: &mut SoftwareIdAllocator,
) -> Result<BatteryStatus, HidppError> {
    let response = transact_feature(
        transport,
        device_index,
        feature_index,
        FUNCTION_GET_BATTERY_STATUS,
        software_ids,
        Duration::from_secs(2),
    )?;
    parse_legacy_battery_status(&response)
}

fn transact_feature<T: HidppTransport>(
    transport: &mut T,
    device_index: u8,
    feature_index: u8,
    function_id: u8,
    software_ids: &mut SoftwareIdAllocator,
    timeout: Duration,
) -> Result<HidppFrame, HidppError> {
    let request = HidppFrame::long(
        device_index,
        feature_index,
        function_id,
        software_ids.allocate(),
        [0; 16],
    );
    transport.transact(&request, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FUNCTION_GET_DEVICE_NAME, FUNCTION_GET_DEVICE_NAME_FRAGMENT, FUNCTION_GET_FEATURE,
        SOURCE_BATTERY_STATUS, SOURCE_UNIFIED_BATTERY,
    };

    struct MockTransport<F> {
        requests: Vec<(HidppFrame, Duration)>,
        respond: F,
    }

    impl<F> MockTransport<F> {
        fn new(respond: F) -> Self {
            Self {
                requests: Vec::new(),
                respond,
            }
        }
    }

    impl<F> HidppTransport for MockTransport<F>
    where
        F: FnMut(&HidppFrame) -> Result<HidppFrame, HidppError>,
    {
        fn transact(
            &mut self,
            request: &HidppFrame,
            timeout: Duration,
        ) -> Result<HidppFrame, HidppError> {
            self.requests.push((request.clone(), timeout));
            (self.respond)(request)
        }
    }

    fn reply(request: &HidppFrame, params: [u8; 16]) -> HidppFrame {
        HidppFrame::long(
            request.device_index,
            request.feature_index,
            request.function_id,
            request.software_id,
            params,
        )
    }

    fn root_reply(request: &HidppFrame, feature_index: u8) -> HidppFrame {
        let mut params = [0; 16];
        params[..3].copy_from_slice(&[feature_index, 0, 3]);
        reply(request, params)
    }

    fn unified_capabilities() -> [u8; 16] {
        let mut params = [0; 16];
        params[..2].copy_from_slice(&[0x0f, 0x03]);
        params
    }

    fn unified_battery_info() -> [u8; 16] {
        let mut params = [0; 16];
        params[..3].copy_from_slice(&[87, 4, 1]);
        params
    }

    #[test]
    fn unified_battery_success_uses_probe_then_query_timeouts() {
        let mut transport = MockTransport::new(|request: &HidppFrame| {
            match (request.feature_index, request.function_id) {
                (0, FUNCTION_GET_FEATURE) => {
                    let logical = u16::from_be_bytes([request.params[0], request.params[1]]);
                    Ok(root_reply(
                        request,
                        if logical == FEATURE_UNIFIED_BATTERY {
                            6
                        } else {
                            0
                        },
                    ))
                }
                (6, FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES) => {
                    Ok(reply(request, unified_capabilities()))
                }
                (6, FUNCTION_GET_UNIFIED_BATTERY_INFO) => {
                    Ok(reply(request, unified_battery_info()))
                }
                _ => Err(HidppError::UnexpectedResponse("unexpected request")),
            }
        });

        let battery = query_battery_with_transport(&mut transport, 1).unwrap();
        assert_eq!(battery.level_percent, Some(87));
        assert_eq!(battery.state, crate::BatteryState::Charging);
        assert_eq!(battery.source, SOURCE_UNIFIED_BATTERY);
        assert_eq!(transport.requests.len(), 3);
        assert_eq!(transport.requests[0].1, PROBE_TIMEOUT);
        assert_eq!(transport.requests[1].1, QUERY_TIMEOUT);
        assert_eq!(transport.requests[2].1, QUERY_TIMEOUT);
        assert_eq!(transport.requests[0].0.params[..2], [0x10, 0x04]);
        assert_eq!(transport.requests[1].0.feature_index, 6);
        assert_eq!(
            transport.requests[1].0.function_id,
            FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES
        );
    }

    #[test]
    fn legacy_battery_status_is_used_when_unified_is_absent() {
        let mut transport = MockTransport::new(|request: &HidppFrame| {
            match (request.feature_index, request.function_id) {
                (0, FUNCTION_GET_FEATURE) => {
                    let logical = u16::from_be_bytes([request.params[0], request.params[1]]);
                    let index = match logical {
                        FEATURE_UNIFIED_BATTERY => 0,
                        FEATURE_BATTERY_STATUS => 3,
                        _ => 0,
                    };
                    Ok(root_reply(request, index))
                }
                (3, FUNCTION_GET_BATTERY_STATUS) => {
                    let mut params = [0; 16];
                    params[..3].copy_from_slice(&[42, 40, 3]);
                    Ok(reply(request, params))
                }
                _ => Err(HidppError::UnexpectedResponse("unexpected request")),
            }
        });

        let battery = query_battery_with_transport(&mut transport, 1).unwrap();
        assert_eq!(battery.level_percent, None);
        assert_eq!(battery.raw_level, 42);
        assert_eq!(battery.state, crate::BatteryState::Charged);
        assert_eq!(battery.source, SOURCE_BATTERY_STATUS);
        assert_eq!(transport.requests.len(), 3);
        assert_eq!(transport.requests[0].1, PROBE_TIMEOUT);
        assert_eq!(transport.requests[1].1, QUERY_TIMEOUT);
        assert_eq!(transport.requests[2].1, QUERY_TIMEOUT);
    }

    #[test]
    fn device_query_reads_the_name_before_the_battery() {
        const NAME: &[u8] = b"G502 X Plus";
        let mut transport = MockTransport::new(|request: &HidppFrame| {
            match (request.feature_index, request.function_id) {
                (0, FUNCTION_GET_FEATURE) => {
                    let logical = u16::from_be_bytes([request.params[0], request.params[1]]);
                    let index = match logical {
                        FEATURE_UNIFIED_BATTERY => 6,
                        crate::FEATURE_DEVICE_NAME => 7,
                        _ => 0,
                    };
                    Ok(root_reply(request, index))
                }
                (6, FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES) => {
                    Ok(reply(request, unified_capabilities()))
                }
                (6, FUNCTION_GET_UNIFIED_BATTERY_INFO) => {
                    Ok(reply(request, unified_battery_info()))
                }
                (7, FUNCTION_GET_DEVICE_NAME) => {
                    let mut params = [0; 16];
                    params[0] = NAME.len() as u8;
                    Ok(reply(request, params))
                }
                (7, FUNCTION_GET_DEVICE_NAME_FRAGMENT) => {
                    let mut params = [0; 16];
                    params[..NAME.len()].copy_from_slice(NAME);
                    Ok(reply(request, params))
                }
                _ => Err(HidppError::UnexpectedResponse("unexpected request")),
            }
        });

        let (name, battery) = query_device_with_transport(&mut transport, 1).unwrap();
        assert_eq!(name.unwrap().as_deref(), Some("G502 X Plus"));
        assert_eq!(battery.level_percent, Some(87));
        let functions: Vec<_> = transport
            .requests
            .iter()
            .map(|(request, _)| (request.feature_index, request.function_id))
            .collect();
        assert_eq!(
            functions,
            [
                (0, FUNCTION_GET_FEATURE),
                (6, FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES),
                (0, FUNCTION_GET_FEATURE),
                (7, FUNCTION_GET_DEVICE_NAME),
                (7, FUNCTION_GET_DEVICE_NAME_FRAGMENT),
                (6, FUNCTION_GET_UNIFIED_BATTERY_INFO),
            ]
        );
    }

    #[test]
    fn transport_errors_are_propagated_without_a_false_success() {
        let mut transport = MockTransport::new(|request: &HidppFrame| {
            match (request.feature_index, request.function_id) {
                (0, FUNCTION_GET_FEATURE) => Ok(root_reply(request, 6)),
                (6, FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES) => {
                    Ok(reply(request, unified_capabilities()))
                }
                (6, FUNCTION_GET_UNIFIED_BATTERY_INFO) => Err(HidppError::Timeout),
                _ => Err(HidppError::UnexpectedResponse("unexpected request")),
            }
        });

        assert!(matches!(
            query_battery_with_transport(&mut transport, 1),
            Err(HidppError::Timeout)
        ));
    }
}
