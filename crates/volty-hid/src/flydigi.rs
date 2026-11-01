//! Flydigi vendor-protocol ("V2") battery decoding.
//!
//! The command layout follows the open-source SDL `SDL_hidapi_flydigi.c`
//! driver: a `5A A5 01` info request answered by a frame whose byte 11 packs
//! the charge state (high nibble) and a level step (low nibble). This path is
//! synthetic-tested only and not hardware-validated.
use crate::generic_battery::level_bucket;
use crate::protocol::{BatteryProtocol, HidBatteryDevice, ProtocolError};
use crate::{BatteryState, BatteryStatus, HidInterfaceInfo};

pub const FLYDIGI_VENDOR_ID_V2: u16 = 0x37d7;
const FIRST_VENDOR_USAGE_PAGE: u16 = 0xff00;

const MAGIC: [u8; 2] = [0x5a, 0xa5];
const COMMAND_GET_INFO: u8 = 0x01;
#[cfg(any(feature = "hidapi", test))]
const REQUEST_LEN: usize = 64;
const MIN_RESPONSE_LEN: usize = 31;
const STATUS_BYTE: usize = 11;

pub const SOURCE_FLYDIGI: &str = "Flydigi vendor report";

/// Builds the get-info request, padded to the interface's 64-byte output report.
///
/// Report ID 0 is used because these controllers expose an unnumbered report.
#[cfg(any(feature = "hidapi", test))]
pub(crate) fn info_request() -> [u8; REQUEST_LEN + 1] {
    let mut frame = [0u8; REQUEST_LEN + 1];
    frame[1] = MAGIC[0];
    frame[2] = MAGIC[1];
    frame[3] = COMMAND_GET_INFO;
    frame[4] = 2;
    frame
}

/// Decodes a get-info response into a battery reading.
///
/// Accepts the frame with or without a leading report ID. Level steps are
/// multiples of 20 % (five steps); state `2` means fully charged.
pub fn decode_flydigi_info(report: &[u8]) -> Result<BatteryStatus, ProtocolError> {
    let frame = match report.first() {
        Some(&MAGIC_FIRST) => report,
        Some(_) => report.get(1..).unwrap_or_default(),
        None => &[],
    };
    if frame.len() < MIN_RESPONSE_LEN || frame[..2] != MAGIC || frame[2] != COMMAND_GET_INFO {
        return Err("not a Flydigi info response".into());
    }

    let packed = frame[STATUS_BYTE];
    let (status, step) = (packed >> 4, packed & 0x0f);
    let (percent, state) = match status {
        0 => (step.min(5) * 20, BatteryState::Discharging),
        1 => (step.min(5) * 20, BatteryState::Charging),
        2 => (100, BatteryState::Charged),
        other => return Err(format!("unknown Flydigi charge state {other}").into()),
    };
    Ok(BatteryStatus {
        level_percent: Some(percent),
        level: level_bucket(percent),
        raw_level: step,
        state,
        raw_status: Some(status),
        voltage_mv: None,
        source: SOURCE_FLYDIGI,
    })
}

const MAGIC_FIRST: u8 = MAGIC[0];

/// Flydigi controllers that answer the vendor info request.
pub struct FlydigiVendorProtocol;

impl BatteryProtocol for FlydigiVendorProtocol {
    fn name(&self) -> &'static str {
        "Flydigi vendor protocol"
    }

    fn supports_interface(&self, interface: &HidInterfaceInfo) -> bool {
        interface.vendor_id == FLYDIGI_VENDOR_ID_V2
            && interface
                .usage_page
                .is_some_and(|page| page >= FIRST_VENDOR_USAGE_PAGE)
    }

    fn device_indices(&self) -> &'static [u8] {
        &[0xff]
    }

    fn query_battery(
        &self,
        _interface: &HidInterfaceInfo,
        _device_index: u8,
    ) -> Result<BatteryStatus, ProtocolError> {
        #[cfg(feature = "hidapi")]
        {
            crate::hid::query_flydigi_interface(_interface)
                .map_err(|error| Box::new(error) as ProtocolError)
        }
        #[cfg(not(feature = "hidapi"))]
        {
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Flydigi battery queries require the `hidapi` feature",
            )))
        }
    }
}

pub struct FlydigiDevice;

impl HidBatteryDevice for FlydigiDevice {
    fn name(&self) -> &'static str {
        "Flydigi controller"
    }

    fn protocol(&self) -> &'static dyn BatteryProtocol {
        &FLYDIGI_VENDOR
    }
}

pub static FLYDIGI_VENDOR: FlydigiVendorProtocol = FlydigiVendorProtocol;
pub static FLYDIGI_DEVICE: FlydigiDevice = FlydigiDevice;

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn response(packed: u8) -> Vec<u8> {
        let mut frame = vec![0u8; 32];
        frame[..3].copy_from_slice(&[0x5a, 0xa5, 0x01]);
        frame[STATUS_BYTE] = packed;
        frame
    }

    #[test]
    fn request_is_an_unnumbered_info_command() {
        let frame = info_request();
        assert_eq!(frame.len(), 65);
        assert_eq!(&frame[..6], &[0, 0x5a, 0xa5, 0x01, 2, 0]);
    }

    #[test]
    fn decodes_discharging_levels_in_twenty_percent_steps() {
        let battery = decode_flydigi_info(&response(0x03)).unwrap();
        assert_eq!(battery.level_percent, Some(60));
        assert_eq!(battery.state, BatteryState::Discharging);
        assert_eq!(battery.raw_level, 3);
        assert_eq!(battery.raw_status, Some(0));
        assert_eq!(battery.source, SOURCE_FLYDIGI);
    }

    #[test]
    fn decodes_charging_and_charged() {
        let charging = decode_flydigi_info(&response(0x12)).unwrap();
        assert_eq!(charging.level_percent, Some(40));
        assert_eq!(charging.state, BatteryState::Charging);

        let charged = decode_flydigi_info(&response(0x25)).unwrap();
        assert_eq!(charged.level_percent, Some(100));
        assert_eq!(charged.state, BatteryState::Charged);
    }

    #[test]
    fn clamps_out_of_range_steps() {
        let battery = decode_flydigi_info(&response(0x0f)).unwrap();
        assert_eq!(battery.level_percent, Some(100));
    }

    #[test]
    fn accepts_a_leading_report_id() {
        let mut frame = vec![0x00];
        frame.extend(response(0x04));
        assert_eq!(decode_flydigi_info(&frame).unwrap().level_percent, Some(80));
    }

    #[test]
    fn rejects_other_commands_short_frames_and_unknown_states() {
        let mut other = response(0x03);
        other[2] = 0x10;
        assert!(decode_flydigi_info(&other).is_err());
        assert!(decode_flydigi_info(&response(0x03)[..20]).is_err());
        assert!(decode_flydigi_info(&[]).is_err());
        assert!(decode_flydigi_info(&response(0x30)).is_err());
    }

    #[test]
    fn matches_only_vendor_pages_of_the_flydigi_vendor() {
        let interface = |vendor_id, usage_page| HidInterfaceInfo {
            vendor_id,
            product_id: 0x2401,
            release_number: 0,
            usage_page,
            usage: Some(1),
            interface_number: 1,
            bus_type: "Usb".to_owned(),
            manufacturer_string: None,
            product_string: None,
            serial_number: None,
            path: CString::new("test").unwrap(),
        };
        assert!(FLYDIGI_VENDOR.supports_interface(&interface(0x37d7, Some(0xffa0))));
        assert!(!FLYDIGI_VENDOR.supports_interface(&interface(0x37d7, Some(0x0001))));
        assert!(!FLYDIGI_VENDOR.supports_interface(&interface(0x37d7, None)));
        assert!(!FLYDIGI_VENDOR.supports_interface(&interface(0x046d, Some(0xffa0))));
    }
}
