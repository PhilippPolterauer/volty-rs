//! Small, UI-free helpers for Logitech HID++ 2.0 devices.
//!
//! Frame construction and battery response parsing work without native
//! dependencies. Enable the `hidapi` feature to enumerate Logitech USB HID
//! interfaces and query a paired device. A receiver may expose several paired
//! devices, so [`query_battery`] requires the HID++ device index separately
//! from the USB device selection.

mod battery;
mod protocol;

pub use battery::*;
pub use protocol::*;

#[cfg(feature = "hidapi")]
mod hid {
    use super::*;
    use hidapi::{DeviceInfo, HidApi, HidDevice};
    use std::ffi::CString;

    /// A Logitech HID interface and its identifiers.
    #[derive(Clone, Debug)]
    pub struct LogitechDeviceInfo {
        pub vendor_id: u16,
        pub product_id: u16,
        pub usage_page: u16,
        pub usage: u16,
        pub interface_number: i32,
        pub product_string: Option<String>,
        pub serial_number: Option<String>,
        path: CString,
    }

    impl LogitechDeviceInfo {
        fn from_hidapi(info: &DeviceInfo) -> Self {
            #[cfg(windows)]
            let (usage_page, usage) = (info.usage_page(), info.usage());
            #[cfg(not(windows))]
            let (usage_page, usage) = (0, 0);

            Self {
                vendor_id: info.vendor_id(),
                product_id: info.product_id(),
                usage_page,
                usage,
                interface_number: info.interface_number(),
                product_string: info.product_string().map(str::to_owned),
                serial_number: info.serial_number().map(str::to_owned),
                path: info.path().to_owned(),
            }
        }
    }

    /// Enumerate Logitech HID++ vendor-defined interfaces.
    ///
    /// This lists USB HID interfaces only. Some Bluetooth devices or receiver
    /// channels may not appear as independently addressable entries.
    pub fn enumerate_logitech_devices() -> Result<Vec<LogitechDeviceInfo>, HidppError> {
        let api = HidApi::new().map_err(HidppError::HidApi)?;
        let mut devices = Vec::new();
        for device in api.device_list() {
            log_hid_interface(device);
            if device.vendor_id() == LOGITECH_VENDOR_ID && is_hidpp_interface(device) {
                devices.push(LogitechDeviceInfo::from_hidapi(device));
            }
        }
        Ok(devices)
    }

    fn log_hid_interface(info: &DeviceInfo) {
        #[cfg(windows)]
        println!(
            "VID={:04x} PID={:04x} usage_page={:04x} usage={:04x}",
            info.vendor_id(),
            info.product_id(),
            info.usage_page(),
            info.usage()
        );
        #[cfg(not(windows))]
        println!(
            "VID={:04x} PID={:04x} usage_page=unavailable usage=unavailable",
            info.vendor_id(),
            info.product_id()
        );
        println!(
            "HID interface={} input_report_length=unavailable output_report_length=unavailable feature_report_length=unavailable (not exposed by hidapi DeviceInfo)",
            info.interface_number()
        );
    }

    #[cfg(windows)]
    fn is_hidpp_interface(info: &DeviceInfo) -> bool {
        info.usage_page() == LOGITECH_HIDPP_USAGE_PAGE && matches!(info.usage(), 1 | 2)
    }

    #[cfg(not(windows))]
    fn is_hidpp_interface(_info: &DeviceInfo) -> bool {
        true
    }

    /// Query unified battery first, falling back to the legacy Battery Status
    /// feature when unified battery is not advertised.
    ///
    /// `device_index` is the HID++ address of the paired device, not its USB
    /// interface number. On receivers it commonly differs for each pairing.
    pub fn query_battery(
        info: &LogitechDeviceInfo,
        device_index: u8,
    ) -> Result<BatteryStatus, HidppError> {
        let api = HidApi::new().map_err(HidppError::HidApi)?;
        let device = api.open_path(&info.path).map_err(HidppError::HidApi)?;

        if let Some(index) = find_feature(&device, device_index, FEATURE_UNIFIED_BATTERY)? {
            let capabilities = transact(
                &device,
                &HidppFrame::long(
                    device_index,
                    index,
                    FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES,
                    HIDPP_SOFTWARE_ID,
                    [0; 16],
                ),
            )?;
            let capabilities = parse_unified_battery_capabilities(&capabilities)?;
            let response = transact(
                &device,
                &HidppFrame::long(
                    device_index,
                    index,
                    FUNCTION_GET_UNIFIED_BATTERY_INFO,
                    HIDPP_SOFTWARE_ID,
                    [0; 16],
                ),
            )?;
            return parse_unified_battery(&response, capabilities);
        }
        if let Some(index) = find_feature(&device, device_index, FEATURE_BATTERY_STATUS)? {
            let response = transact(
                &device,
                &HidppFrame::long(
                    device_index,
                    index,
                    FUNCTION_GET_BATTERY_STATUS,
                    HIDPP_SOFTWARE_ID,
                    [0; 16],
                ),
            )?;
            return parse_legacy_battery_status(&response);
        }
        Err(HidppError::UnsupportedBatteryFeature)
    }

    fn find_feature(
        device: &HidDevice,
        device_index: u8,
        feature_id: u16,
    ) -> Result<Option<u8>, HidppError> {
        let mut params = [0; 16];
        params[0] = (feature_id >> 8) as u8;
        params[1] = feature_id as u8;
        let response = transact(
            device,
            &HidppFrame::long(
                device_index,
                FEATURE_ROOT as u8,
                FUNCTION_GET_FEATURE,
                HIDPP_SOFTWARE_ID,
                params,
            ),
        )?;
        let index = response
            .params
            .first()
            .copied()
            .ok_or(HidppError::UnexpectedResponse(
                "missing feature index in root response",
            ))?;
        Ok((index != 0).then_some(index))
    }

    fn transact(device: &HidDevice, request: &HidppFrame) -> Result<HidppFrame, HidppError> {
        let request_bytes = request.encode()?;
        println!(
            "HID++ TX report_id=0x{:02x} feature_index=0x{:02x} function_id=0x{:02x} device_index=0x{:02x} software_id=0x{:02x} bytes={:02x?}",
            request_bytes[0],
            request.feature_index,
            request.function_id,
            request.device_index,
            request.software_id,
            request_bytes
        );
        let written = device.write(&request_bytes).map_err(HidppError::HidApi)?;
        println!(
            "HID++ TX transport=HidDevice::write (Output report) wrote={written}/{} bytes",
            request_bytes.len()
        );
        if written != request_bytes.len() {
            return Err(HidppError::UnexpectedResponse(
                "short write while sending HID++ Output report",
            ));
        }

        let mut buffer = [0u8; 20];
        for _ in 0..8 {
            let read = device
                .read_timeout(&mut buffer, 1000)
                .map_err(HidppError::HidApi)?;
            if read == 0 {
                return Err(HidppError::Timeout);
            }
            println!("HID++ RX bytes={:02x?}", &buffer[..read]);
            let response = HidppFrame::parse(&buffer[..read])?;
            println!(
                "HID++ RX report_id=0x{:02x} feature_index=0x{:02x} function_id=0x{:02x} device_index=0x{:02x} software_id=0x{:02x}",
                response.format.report_id(),
                response.feature_index,
                response.function_id,
                response.device_index,
                response.software_id
            );
            if response.device_index == request.device_index
                && response.feature_index == request.feature_index
                && response.function_id == request.function_id
                && response.software_id == request.software_id
            {
                return Ok(response);
            }
            println!(
                "HID++ RX ignored: expected device_index=0x{:02x} feature_index=0x{:02x} function_id=0x{:02x} software_id=0x{:02x}",
                request.device_index,
                request.feature_index,
                request.function_id,
                request.software_id
            );
        }
        Err(HidppError::UnexpectedResponse(
            "no matching response among received reports",
        ))
    }
}

#[cfg(feature = "hidapi")]
pub use hid::{enumerate_logitech_devices, query_battery, LogitechDeviceInfo};

#[cfg(test)]
mod tests {
    use super::*;

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
