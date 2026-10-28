//! Standard HID battery reporting for devices that expose a charge level through
//! the Generic Device Controls or Battery System usage pages.
#![cfg_attr(not(all(windows, feature = "hidapi")), allow(dead_code))]

#[cfg(all(windows, feature = "hidapi"))]
mod native;

use crate::protocol::{BatteryProtocol, HidBatteryDevice, ProtocolError};
use crate::{BatteryLevel, BatteryState, BatteryStatus, HidInterfaceInfo};

pub const SOURCE_HID_BATTERY: &str = "HID Battery";

const PAGE_GENERIC_DEVICE: u16 = 0x06;
const PAGE_BATTERY_SYSTEM: u16 = 0x85;
const USAGE_BATTERY_STRENGTH: u16 = 0x20;
const USAGE_CHARGING: u16 = 0x44;
const USAGE_DISCHARGING: u16 = 0x45;
const USAGE_RELATIVE_STATE_OF_CHARGE: u16 = 0x64;
const USAGE_ABSOLUTE_STATE_OF_CHARGE: u16 = 0x65;
const USAGE_REMAINING_CAPACITY: u16 = 0x66;
const USAGE_FULL_CHARGE_CAPACITY: u16 = 0x67;
/// Vendor-defined pages start here and carry no standard battery semantics.
const VENDOR_PAGE_START: u16 = 0xff00;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReportKind {
    Feature,
    Input,
}

/// A numeric usage found in a device's report descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ValueField {
    pub page: u16,
    pub usage: u16,
    pub report_id: u8,
    pub kind: ReportKind,
    pub logical_min: i32,
    pub logical_max: i32,
}

/// A single-bit usage found in a device's report descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ButtonField {
    pub usage: u16,
    pub report_id: u8,
    pub kind: ReportKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LevelSource {
    Percent(ValueField),
    Ratio {
        remaining: ValueField,
        full: ValueField,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BatteryLayout {
    pub level: LevelSource,
    pub charging: Option<ButtonField>,
    pub discharging: Option<ButtonField>,
}

pub(crate) fn is_battery_value(page: u16, usage: u16) -> bool {
    matches!(
        (page, usage),
        (PAGE_GENERIC_DEVICE, USAGE_BATTERY_STRENGTH)
            | (
                PAGE_BATTERY_SYSTEM,
                USAGE_RELATIVE_STATE_OF_CHARGE
                    | USAGE_ABSOLUTE_STATE_OF_CHARGE
                    | USAGE_REMAINING_CAPACITY
                    | USAGE_FULL_CHARGE_CAPACITY
            )
    )
}

pub(crate) fn is_battery_button(page: u16, usage: u16) -> bool {
    page == PAGE_BATTERY_SYSTEM && matches!(usage, USAGE_CHARGING | USAGE_DISCHARGING)
}

/// Picks the best battery level source among the fields a device declares.
///
/// Earlier entries win among equal usages, so callers list feature reports
/// before input reports.
pub(crate) fn select_layout(
    values: &[ValueField],
    buttons: &[ButtonField],
) -> Option<BatteryLayout> {
    let find = |page: u16, usage: u16| {
        values
            .iter()
            .copied()
            .find(|field| field.page == page && field.usage == usage && has_usable_range(field))
    };
    let level = [
        (PAGE_BATTERY_SYSTEM, USAGE_RELATIVE_STATE_OF_CHARGE),
        (PAGE_BATTERY_SYSTEM, USAGE_ABSOLUTE_STATE_OF_CHARGE),
        (PAGE_GENERIC_DEVICE, USAGE_BATTERY_STRENGTH),
    ]
    .into_iter()
    .find_map(|(page, usage)| find(page, usage))
    .map(LevelSource::Percent)
    .or_else(|| {
        Some(LevelSource::Ratio {
            remaining: find(PAGE_BATTERY_SYSTEM, USAGE_REMAINING_CAPACITY)?,
            full: find(PAGE_BATTERY_SYSTEM, USAGE_FULL_CHARGE_CAPACITY)?,
        })
    })?;
    let button = |usage: u16| buttons.iter().copied().find(|field| field.usage == usage);
    Some(BatteryLayout {
        level,
        charging: button(USAGE_CHARGING),
        discharging: button(USAGE_DISCHARGING),
    })
}

fn has_usable_range(field: &ValueField) -> bool {
    field.logical_min >= 0 && field.logical_max > field.logical_min
}

/// Scales a raw report value onto 0–100 using the field's logical range.
pub(crate) fn scale_percent(raw: u32, logical_min: i32, logical_max: i32) -> Option<u8> {
    let min = i64::from(logical_min);
    let span = i64::from(logical_max) - min;
    if min < 0 || span <= 0 {
        return None;
    }
    let offset = (i64::from(raw) - min).clamp(0, span);
    u8::try_from((offset * 100 + span / 2) / span).ok()
}

/// Remaining capacity as a percentage of full-charge capacity.
pub(crate) fn ratio_percent(remaining: u32, full: u32) -> Option<u8> {
    if full == 0 {
        return None;
    }
    u8::try_from((u64::from(remaining.min(full)) * 100 + u64::from(full) / 2) / u64::from(full))
        .ok()
}

fn level_bucket(percent: u8) -> BatteryLevel {
    match percent {
        0..=10 => BatteryLevel::Critical,
        11..=30 => BatteryLevel::Low,
        31..=99 => BatteryLevel::Good,
        _ => BatteryLevel::Full,
    }
}

pub(crate) fn battery_status(
    percent: u8,
    charging: Option<bool>,
    discharging: Option<bool>,
) -> BatteryStatus {
    let state = match (charging, discharging) {
        (Some(true), _) if percent >= 100 => BatteryState::Charged,
        (Some(true), _) => BatteryState::Charging,
        (_, Some(true)) => BatteryState::Discharging,
        _ => BatteryState::Unknown,
    };
    BatteryStatus {
        level_percent: Some(percent),
        level: level_bucket(percent),
        raw_level: percent,
        state,
        raw_status: None,
        source: SOURCE_HID_BATTERY,
    }
}

/// Whether the interface's top-level collection is worth probing for a battery.
fn is_probe_candidate(interface: &HidInterfaceInfo) -> bool {
    interface
        .usage_page
        .is_some_and(|page| page < VENDOR_PAGE_START)
}

/// The standard HID battery protocol; the layout is read from the report descriptor.
pub struct GenericHidBattery;

impl BatteryProtocol for GenericHidBattery {
    fn name(&self) -> &'static str {
        "HID battery reports"
    }

    fn supports_interface(&self, interface: &HidInterfaceInfo) -> bool {
        is_probe_candidate(interface) && cached_layout(interface).is_some()
    }

    fn device_indices(&self) -> &'static [u8] {
        &[0xff]
    }

    fn query_battery(
        &self,
        interface: &HidInterfaceInfo,
        _device_index: u8,
    ) -> Result<BatteryStatus, ProtocolError> {
        #[cfg(all(windows, feature = "hidapi"))]
        {
            let layout = cached_layout(interface).ok_or_else(|| {
                ProtocolError::from("the interface no longer reports a battery level")
            })?;
            native::read_battery(&interface.path, &layout)
        }
        #[cfg(not(all(windows, feature = "hidapi")))]
        {
            let _ = interface;
            Err("HID battery reports require Windows and the `hidapi` feature".into())
        }
    }
}

/// Any HID interface whose report descriptor declares a battery level.
pub struct GenericHidBatteryDevice;

impl HidBatteryDevice for GenericHidBatteryDevice {
    fn name(&self) -> &'static str {
        "HID battery device"
    }

    fn protocol(&self) -> &'static dyn BatteryProtocol {
        &GENERIC_HID_BATTERY
    }
}

pub static GENERIC_HID_BATTERY: GenericHidBattery = GenericHidBattery;
pub static GENERIC_HID_BATTERY_DEVICE: GenericHidBatteryDevice = GenericHidBatteryDevice;

#[cfg(all(windows, feature = "hidapi"))]
fn cached_layout(interface: &HidInterfaceInfo) -> Option<BatteryLayout> {
    native::cached_layout(&interface.path)
}

#[cfg(not(all(windows, feature = "hidapi")))]
fn cached_layout(_interface: &HidInterfaceInfo) -> Option<BatteryLayout> {
    None
}

/// Forgets probe results of interfaces that are no longer present.
#[cfg(all(windows, feature = "hidapi"))]
pub(crate) fn forget_absent_interfaces(interfaces: &[HidInterfaceInfo]) {
    native::forget_absent_interfaces(interfaces);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(page: u16, usage: u16, kind: ReportKind, min: i32, max: i32) -> ValueField {
        ValueField {
            page,
            usage,
            report_id: 1,
            kind,
            logical_min: min,
            logical_max: max,
        }
    }

    fn button(usage: u16) -> ButtonField {
        ButtonField {
            usage,
            report_id: 2,
            kind: ReportKind::Input,
        }
    }

    #[test]
    fn scaling_maps_the_logical_range_onto_percent() {
        assert_eq!(scale_percent(0, 0, 100), Some(0));
        assert_eq!(scale_percent(55, 0, 100), Some(55));
        assert_eq!(scale_percent(127, 0, 255), Some(50));
        assert_eq!(scale_percent(50, 10, 110), Some(40));
        assert_eq!(scale_percent(999, 0, 100), Some(100));
        assert_eq!(scale_percent(3, 10, 110), Some(0));
        assert_eq!(scale_percent(5, 0, 0), None);
        assert_eq!(scale_percent(5, -10, 10), None);
    }

    #[test]
    fn ratio_clamps_to_full_capacity_and_rejects_zero() {
        assert_eq!(ratio_percent(2500, 5000), Some(50));
        assert_eq!(ratio_percent(6000, 5000), Some(100));
        assert_eq!(ratio_percent(1, 0), None);
    }

    #[test]
    fn layout_prefers_relative_state_of_charge_then_battery_strength() {
        let relative = value(
            PAGE_BATTERY_SYSTEM,
            USAGE_RELATIVE_STATE_OF_CHARGE,
            ReportKind::Feature,
            0,
            100,
        );
        let strength = value(
            PAGE_GENERIC_DEVICE,
            USAGE_BATTERY_STRENGTH,
            ReportKind::Input,
            0,
            255,
        );
        let layout = select_layout(&[strength, relative], &[]).unwrap();
        assert_eq!(layout.level, LevelSource::Percent(relative));
        let layout = select_layout(&[strength], &[]).unwrap();
        assert_eq!(layout.level, LevelSource::Percent(strength));
    }

    #[test]
    fn layout_uses_capacity_ratio_only_with_both_halves() {
        let remaining = value(
            PAGE_BATTERY_SYSTEM,
            USAGE_REMAINING_CAPACITY,
            ReportKind::Feature,
            0,
            65535,
        );
        let full = value(
            PAGE_BATTERY_SYSTEM,
            USAGE_FULL_CHARGE_CAPACITY,
            ReportKind::Feature,
            0,
            65535,
        );
        assert!(select_layout(&[remaining], &[]).is_none());
        let layout = select_layout(&[remaining, full], &[]).unwrap();
        assert_eq!(layout.level, LevelSource::Ratio { remaining, full });
    }

    #[test]
    fn layout_skips_fields_without_a_usable_range_and_unrelated_usages() {
        let degenerate = value(
            PAGE_GENERIC_DEVICE,
            USAGE_BATTERY_STRENGTH,
            ReportKind::Input,
            0,
            0,
        );
        let unrelated = value(0x01, 0x30, ReportKind::Input, 0, 255);
        assert!(select_layout(&[degenerate, unrelated], &[]).is_none());
    }

    #[test]
    fn layout_collects_state_buttons() {
        let strength = value(
            PAGE_GENERIC_DEVICE,
            USAGE_BATTERY_STRENGTH,
            ReportKind::Input,
            0,
            100,
        );
        let layout = select_layout(
            &[strength],
            &[button(USAGE_DISCHARGING), button(USAGE_CHARGING)],
        )
        .unwrap();
        assert_eq!(layout.charging, Some(button(USAGE_CHARGING)));
        assert_eq!(layout.discharging, Some(button(USAGE_DISCHARGING)));
    }

    #[test]
    fn status_maps_percent_and_state() {
        let charging = battery_status(40, Some(true), Some(false));
        assert_eq!(charging.level_percent, Some(40));
        assert_eq!(charging.state, BatteryState::Charging);
        assert_eq!(charging.level, BatteryLevel::Good);
        assert_eq!(charging.source, SOURCE_HID_BATTERY);

        assert_eq!(
            battery_status(100, Some(true), None).state,
            BatteryState::Charged
        );
        assert_eq!(
            battery_status(20, Some(false), Some(true)).state,
            BatteryState::Discharging
        );
        assert_eq!(battery_status(5, None, None).state, BatteryState::Unknown);
        assert_eq!(battery_status(5, None, None).level, BatteryLevel::Critical);
        assert_eq!(battery_status(25, None, None).level, BatteryLevel::Low);
        assert_eq!(battery_status(100, None, None).level, BatteryLevel::Full);
    }

    #[test]
    fn vendor_defined_pages_are_not_probed() {
        let mut interface = HidInterfaceInfo {
            vendor_id: 0x1234,
            product_id: 0x5678,
            release_number: 1,
            usage_page: Some(0xff00),
            usage: Some(1),
            interface_number: 0,
            bus_type: "Bluetooth".to_owned(),
            manufacturer_string: None,
            product_string: None,
            serial_number: None,
            path: std::ffi::CString::new("test").unwrap(),
        };
        assert!(!is_probe_candidate(&interface));
        interface.usage_page = Some(0x01);
        assert!(is_probe_candidate(&interface));
        interface.usage_page = None;
        assert!(!is_probe_candidate(&interface));
    }
}
