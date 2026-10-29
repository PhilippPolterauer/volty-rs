use serde::{Deserialize, Serialize};

/// The capture file schema written by this crate.
pub const SCHEMA: &str = "volty.capture/v1";

/// How much hardware activity a capture contains.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// A one-shot inventory of the hardware state.
    Snapshot,
    /// A longer recording that includes samples and exchanges over time.
    Session,
}

/// How much human-readable information a capture is allowed to contain.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyProfile {
    /// Keep model-level manufacturer and product strings.
    Standard,
    /// Keep only numeric identifiers and hashes.
    Strict,
}

/// Result of matching an interface against a known device profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchOutcome {
    Matched,
    Unsupported,
    Ambiguous,
    Error,
}

/// Severity of a capture event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLevel {
    Info,
    Warning,
    Error,
}

/// Which HID report carries a capability or sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HidReportKind {
    Feature,
    Input,
}

/// One record in an append-only capture file.
///
/// The tag is serialized as `"type": "<snake_case variant>"`. The header and
/// footer are written by [`crate::CaptureWriter`] and validated by
/// [`crate::CaptureFile::validate`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CaptureRecord {
    Header(CaptureHeader),
    Interface(CaptureInterface),
    Match(CaptureMatch),
    HidppFeatures(CaptureHidppFeatures),
    HidLayout(CaptureHidLayout),
    HidppExchange(CaptureHidppExchange),
    HidSample(CaptureHidSample),
    Reading(CaptureReading),
    Event(CaptureEvent),
    Marker(CaptureMarker),
    Footer(CaptureFooter),
}

impl CaptureRecord {
    /// The snake-case name written in the record's `type` field.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Header(_) => "header",
            Self::Interface(_) => "interface",
            Self::Match(_) => "match",
            Self::HidppFeatures(_) => "hidpp_features",
            Self::HidLayout(_) => "hid_layout",
            Self::HidppExchange(_) => "hidpp_exchange",
            Self::HidSample(_) => "hid_sample",
            Self::Reading(_) => "reading",
            Self::Event(_) => "event",
            Self::Marker(_) => "marker",
            Self::Footer(_) => "footer",
        }
    }
}

/// First record of every capture file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureHeader {
    /// Must equal [`SCHEMA`].
    pub schema: String,
    /// Caller-generated identifier for this capture.
    pub capture_id: String,
    /// RFC 3339 timestamp supplied by the caller.
    pub created_at: String,
    pub app_version: String,
    pub app_commit: Option<String>,
    pub os: String,
    pub architecture: String,
    pub mode: CaptureMode,
    pub privacy: PrivacyProfile,
    /// Feature flags enabled while recording.
    pub features: Vec<String>,
}

/// One HID interface seen during a capture.
///
/// A device path and the serial number value are deliberately not part of this
/// record. Only [`Self::serial_present`] may be recorded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureInterface {
    /// Capture-local identifier such as `iface-1`.
    pub id: String,
    pub bus: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub release_number: u16,
    pub usage_page: Option<u16>,
    pub usage: Option<u16>,
    pub interface_number: i32,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    /// Whether a serial number was exposed, never the value itself.
    pub serial_present: bool,
}

/// Result of matching an interface against the supported device set.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureMatch {
    pub id: String,
    pub device_index: Option<u8>,
    pub backend: Option<String>,
    pub outcome: MatchOutcome,
    /// Short reason for the outcome.
    pub detail: Option<String>,
}

/// HID++ feature table discovered for one device.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureHidppFeatures {
    pub id: String,
    pub device_index: u8,
    pub features: Vec<HidppFeature>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidppFeature {
    pub logical_id: u16,
    pub runtime_index: u8,
    pub feature_type: u8,
    pub version: u8,
}

/// HID capability layout and the fields selected as the battery source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureHidLayout {
    pub id: String,
    pub device_index: Option<u8>,
    pub values: Vec<HidValueCapability>,
    pub buttons: Vec<HidButtonCapability>,
    pub selected: Option<HidSelectedLayout>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidValueCapability {
    pub page: u16,
    pub usage: u16,
    pub report_id: u8,
    pub kind: HidReportKind,
    pub logical_min: i32,
    pub logical_max: i32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidButtonCapability {
    pub page: u16,
    pub usage: u16,
    pub report_id: u8,
    pub kind: HidReportKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidSelectedLayout {
    pub percent: Option<HidFieldRef>,
    pub ratio: Option<HidRatioRef>,
    pub charging: Option<HidFieldRef>,
    pub discharging: Option<HidFieldRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidFieldRef {
    pub page: u16,
    pub usage: u16,
    pub report_id: u8,
    pub kind: HidReportKind,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidRatioRef {
    pub remaining: HidFieldRef,
    pub full: HidFieldRef,
}

/// One HID++ request/response exchange.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureHidppExchange {
    pub id: String,
    pub device_index: u8,
    pub t_ms: u64,
    /// Lowercase hex without a `0x` prefix.
    pub request: String,
    /// Lowercase hex without a `0x` prefix.
    pub response: Option<String>,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

/// Raw HID values read at one point in time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureHidSample {
    pub id: String,
    pub device_index: Option<u8>,
    pub t_ms: u64,
    pub values: Vec<HidValueSample>,
    /// State bits are optional for captures written before they were recorded.
    #[serde(default)]
    pub buttons: Vec<HidButtonSample>,
    pub battery: BatterySnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidValueSample {
    pub page: u16,
    pub usage: u16,
    pub report_id: u8,
    pub kind: HidReportKind,
    pub raw_value: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidButtonSample {
    pub page: u16,
    pub usage: u16,
    pub report_id: u8,
    pub kind: HidReportKind,
    pub pressed: bool,
}

/// Normalized battery reading emitted by a backend.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureReading {
    pub id: String,
    pub device_index: u8,
    pub t_ms: u64,
    pub backend: String,
    pub label: Option<String>,
    pub battery: BatterySnapshot,
}

/// Privacy-safe battery state for one observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatterySnapshot {
    pub percent: Option<u8>,
    /// One of `unknown`, `critical`, `low`, `good`, or `full`.
    pub level: String,
    /// One of `unknown`, `discharging`, `charging`, `charging_nearly_full`,
    /// `charged`, `charging_slow`, `invalid_battery`, `thermal_error`, or
    /// `charging_error`.
    pub state: String,
    pub raw_level: u8,
    pub raw_status: Option<u8>,
    pub source: String,
}

/// Diagnostic message recorded during a capture.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureEvent {
    pub t_ms: u64,
    pub id: Option<String>,
    pub level: EventLevel,
    pub message: String,
}

/// User-supplied marker that makes a capture easier to interpret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureMarker {
    pub t_ms: u64,
    pub label: String,
}

/// Last record of every capture file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureFooter {
    /// Must equal the header schema and [`SCHEMA`].
    pub schema: String,
    /// Number of records before this footer, including the header.
    pub records: u64,
    /// Whether the capture was closed deliberately.
    pub complete: bool,
}

/// Encodes bytes as lowercase hex without a `0x` prefix.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    text
}

/// Decodes hex without a `0x` prefix.
///
/// Both uppercase and lowercase digits are accepted; [`to_hex`] always emits
/// lowercase.
///
/// # Errors
/// Returns an error for an odd number of digits or a non-hex character.
pub fn from_hex(text: &str) -> Result<Vec<u8>, crate::CaptureError> {
    if !text.len().is_multiple_of(2) {
        return Err(crate::CaptureError::new(
            "hex string must have an even number of digits",
        ));
    }

    let mut bytes = Vec::with_capacity(text.len() / 2);
    for (index, pair) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let high = hex_digit(pair[0]).ok_or_else(|| {
            crate::CaptureError::new(format!(
                "invalid hex digit at byte {}: {:?}",
                index * 2,
                pair[0] as char
            ))
        })?;
        let low = hex_digit(pair[1]).ok_or_else(|| {
            crate::CaptureError::new(format!(
                "invalid hex digit at byte {}: {:?}",
                index * 2 + 1,
                pair[1] as char
            ))
        })?;
        bytes.push(high << 4 | low);
    }
    Ok(bytes)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
