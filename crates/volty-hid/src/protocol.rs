use std::fmt;

pub const LOGITECH_VENDOR_ID: u16 = 0x046d;
pub const FEATURE_ROOT: u16 = 0x0000;
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
pub const LOGITECH_HIDPP_USAGE_PAGE: u16 = 0xff00;
pub const FUNCTION_GET_FEATURE: u8 = 0x00;
pub const FUNCTION_GET_BATTERY_STATUS: u8 = 0x00;
pub const FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES: u8 = 0x00;
pub const FUNCTION_GET_UNIFIED_BATTERY_INFO: u8 = 0x01;
pub const HIDPP_SOFTWARE_ID: u8 = 0x01;

const REPORT_ID_SHORT: u8 = 0x10;
const REPORT_ID_LONG: u8 = 0x11;
const SHORT_FRAME_LEN: usize = 7;
const LONG_FRAME_LEN: usize = 20;
const SHORT_PARAMS_LEN: usize = 3;
const LONG_PARAMS_LEN: usize = 16;

/// The HID++ report format determines its payload capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameFormat {
    Short,
    Long,
}

impl FrameFormat {
    pub fn report_id(self) -> u8 {
        match self {
            Self::Short => REPORT_ID_SHORT,
            Self::Long => REPORT_ID_LONG,
        }
    }

    fn payload_len(self) -> usize {
        match self {
            Self::Short => SHORT_PARAMS_LEN,
            Self::Long => LONG_PARAMS_LEN,
        }
    }

    fn frame_len(self) -> usize {
        match self {
            Self::Short => SHORT_FRAME_LEN,
            Self::Long => LONG_FRAME_LEN,
        }
    }
}

/// A decoded HID++ short or long frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HidppFrame {
    pub format: FrameFormat,
    pub device_index: u8,
    pub feature_index: u8,
    /// Function ID encoded in the high nibble of byte 3.
    pub function_id: u8,
    /// Software ID encoded in the low nibble of byte 3.
    pub software_id: u8,
    pub params: Vec<u8>,
}

impl HidppFrame {
    /// Build a short frame. `params` must contain exactly 3 bytes.
    pub fn short(
        device_index: u8,
        feature_index: u8,
        function_id: u8,
        software_id: u8,
        params: [u8; SHORT_PARAMS_LEN],
    ) -> Self {
        Self {
            format: FrameFormat::Short,
            device_index,
            feature_index,
            function_id,
            software_id,
            params: params.to_vec(),
        }
    }

    /// Build a long frame. `params` must contain exactly 16 bytes.
    pub fn long(
        device_index: u8,
        feature_index: u8,
        function_id: u8,
        software_id: u8,
        params: [u8; LONG_PARAMS_LEN],
    ) -> Self {
        Self {
            format: FrameFormat::Long,
            device_index,
            feature_index,
            function_id,
            software_id,
            params: params.to_vec(),
        }
    }

    /// Decode a complete HID++ report, including its report-id byte.
    pub fn parse(bytes: &[u8]) -> Result<Self, HidppError> {
        let format = match bytes.first().copied() {
            Some(REPORT_ID_SHORT) => FrameFormat::Short,
            Some(REPORT_ID_LONG) => FrameFormat::Long,
            Some(id) => return Err(HidppError::InvalidReportId(id)),
            None => {
                return Err(HidppError::InvalidFrameLength {
                    expected: 1,
                    actual: 0,
                })
            }
        };
        let expected = format.frame_len();
        if bytes.len() != expected {
            return Err(HidppError::InvalidFrameLength {
                expected,
                actual: bytes.len(),
            });
        }

        Ok(Self {
            format,
            device_index: bytes[1],
            feature_index: bytes[2],
            function_id: bytes[3] >> 4,
            software_id: bytes[3] & 0x0f,
            params: bytes[4..].to_vec(),
        })
    }

    /// Encode this frame as a complete HID report.
    pub fn encode(&self) -> Result<Vec<u8>, HidppError> {
        let expected = self.format.payload_len();
        if self.params.len() != expected {
            return Err(HidppError::InvalidPayloadLength {
                expected,
                actual: self.params.len(),
            });
        }
        if self.function_id > 0x0f || self.software_id > 0x0f {
            return Err(HidppError::InvalidFunctionSoftwareId {
                function_id: self.function_id,
                software_id: self.software_id,
            });
        }

        let mut bytes = Vec::with_capacity(self.format.frame_len());
        bytes.extend_from_slice(&[
            self.format.report_id(),
            self.device_index,
            self.feature_index,
            (self.function_id << 4) | self.software_id,
        ]);
        bytes.extend_from_slice(&self.params);
        Ok(bytes)
    }
}

/// Errors returned by frame decoding, battery parsing, or optional HID access.
#[derive(Debug)]
pub enum HidppError {
    InvalidReportId(u8),
    InvalidFrameLength {
        expected: usize,
        actual: usize,
    },
    InvalidPayloadLength {
        expected: usize,
        actual: usize,
    },
    InvalidFunctionSoftwareId {
        function_id: u8,
        software_id: u8,
    },
    UnexpectedResponse(&'static str),
    InvalidBatteryLevel(u8),
    UnsupportedBatteryFeature,
    #[cfg(feature = "hidapi")]
    HidApi(hidapi::HidError),
    #[cfg(feature = "hidapi")]
    Timeout,
}

impl fmt::Display for HidppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidReportId(id) => write!(f, "unsupported HID++ report id 0x{id:02x}"),
            Self::InvalidFrameLength { expected, actual } => {
                write!(
                    f,
                    "invalid HID++ frame length: expected {expected}, got {actual}"
                )
            }
            Self::InvalidPayloadLength { expected, actual } => {
                write!(
                    f,
                    "invalid HID++ payload length: expected {expected}, got {actual}"
                )
            }
            Self::InvalidFunctionSoftwareId {
                function_id,
                software_id,
            } => write!(
                f,
                "invalid HID++ function/software id: function 0x{function_id:02x}, software 0x{software_id:02x}; both must fit in a nibble"
            ),
            Self::UnexpectedResponse(message) => write!(f, "unexpected HID++ response: {message}"),
            Self::InvalidBatteryLevel(level) => {
                write!(f, "invalid battery percentage {level}; expected 0..=100")
            }
            Self::UnsupportedBatteryFeature => {
                f.write_str("device does not expose a supported battery feature")
            }
            #[cfg(feature = "hidapi")]
            Self::HidApi(error) => write!(f, "HID API error: {error}"),
            #[cfg(feature = "hidapi")]
            Self::Timeout => f.write_str("timed out waiting for a HID++ response"),
        }
    }
}

impl std::error::Error for HidppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        #[cfg(feature = "hidapi")]
        if let Self::HidApi(error) = self {
            return Some(error);
        }
        None
    }
}
