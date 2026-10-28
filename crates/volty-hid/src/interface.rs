use crate::HidppError;
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
    pub serial_number: Option<String>,
    pub path: CString,
}

impl HidInterfaceInfo {
    /// Path-independent identity: the same physical interface keeps this value
    /// when Windows re-enumerates it under a different device path. The serial
    /// number, when present, is hashed so it never appears in logs or state.
    fn identity_base(&self) -> String {
        let serial = self
            .serial_number
            .as_deref()
            .map(str::trim)
            .filter(|serial| !serial.is_empty())
            .map_or_else(
                || "-".to_owned(),
                |serial| format!("{:016x}", fnv1a(serial)),
            );
        format!(
            "{:04x}:{:04x}:{}:{}:{}:{serial}",
            self.vendor_id,
            self.product_id,
            self.interface_number,
            self.usage_page
                .map_or_else(|| "-".to_owned(), |value| format!("{value:04x}")),
            self.usage
                .map_or_else(|| "-".to_owned(), |value| format!("{value:04x}")),
        )
    }
}

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Stable, path-independent keys for an inventory, in the same order as the input.
///
/// Interfaces that are indistinguishable by metadata (identical devices without
/// a serial number) are told apart by their order within the sorted paths; the
/// first of each group keeps the bare key so plugging in a twin later does not
/// rename the original.
pub fn stable_interface_keys(interfaces: &[HidInterfaceInfo]) -> Vec<String> {
    let bases: Vec<String> = interfaces
        .iter()
        .map(HidInterfaceInfo::identity_base)
        .collect();
    let mut order: Vec<usize> = (0..interfaces.len()).collect();
    order.sort_by(|&left, &right| {
        bases[left]
            .cmp(&bases[right])
            .then_with(|| interfaces[left].path.cmp(&interfaces[right].path))
    });
    let mut keys = vec![String::new(); interfaces.len()];
    let mut previous_base: Option<&str> = None;
    let mut ordinal = 0;
    for index in order {
        let base = bases[index].as_str();
        ordinal = if previous_base == Some(base) {
            ordinal + 1
        } else {
            0
        };
        previous_base = Some(base);
        keys[index] = if ordinal == 0 {
            base.to_owned()
        } else {
            format!("{base}~{ordinal}")
        };
    }
    keys
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
            .field(
                "serial_number",
                &self.serial_number.as_ref().map(|_| "<redacted>"),
            )
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
