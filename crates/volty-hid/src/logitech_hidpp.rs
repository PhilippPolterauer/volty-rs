//! Logitech HID++ 2.0 frame, transport, and protocol-specific definitions.
use crate::protocol::{BatteryProtocol, HidBatteryDevice, ProtocolDeviceReading, ProtocolError};
use crate::{BatteryStatus, HidInterfaceInfo};
use std::fmt;
use std::time::Duration;
#[cfg(any(feature = "hidapi", test))]
use std::time::Instant;

/// The Logitech HID++ 2.0 battery protocol implementation.
pub struct LogitechHidpp20;

impl BatteryProtocol for LogitechHidpp20 {
    fn name(&self) -> &'static str {
        "Logitech HID++ 2.0"
    }

    fn supports_interface(&self, interface: &HidInterfaceInfo) -> bool {
        interface.vendor_id == LOGITECH_VENDOR_ID
            && interface.usage_page == Some(LOGITECH_HIDPP_USAGE_PAGE)
            && interface.usage == Some(LOGITECH_HIDPP_LONG_USAGE)
    }

    fn device_indices(&self) -> &'static [u8] {
        &[0xff, 1, 2, 3, 4, 5, 6]
    }

    fn query_battery(
        &self,
        _interface: &HidInterfaceInfo,
        _device_index: u8,
    ) -> Result<BatteryStatus, ProtocolError> {
        #[cfg(feature = "hidapi")]
        {
            crate::query_battery_interface(_interface, _device_index)
                .map_err(|error| Box::new(error) as ProtocolError)
        }
        #[cfg(not(feature = "hidapi"))]
        {
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Logitech HID++ battery queries require the `hidapi` feature",
            )))
        }
    }

    fn query_device(
        &self,
        interface: &HidInterfaceInfo,
        device_index: u8,
    ) -> Result<ProtocolDeviceReading, ProtocolError> {
        #[cfg(feature = "hidapi")]
        {
            let (name, battery) = crate::query_device_interface(interface, device_index)
                .map_err(|error| Box::new(error) as ProtocolError)?;
            Ok(ProtocolDeviceReading {
                name: name.map_err(|error| Box::new(error) as ProtocolError),
                battery,
            })
        }
        #[cfg(not(feature = "hidapi"))]
        {
            let _ = (interface, device_index);
            Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Logitech HID++ battery queries require the `hidapi` feature",
            )))
        }
    }
}

/// Logitech HID++ 2.0 device family; battery support is confirmed by feature discovery.
pub struct LogitechHidppDevice;

impl HidBatteryDevice for LogitechHidppDevice {
    fn name(&self) -> &'static str {
        "Logitech HID++ mouse"
    }

    fn protocol(&self) -> &'static dyn BatteryProtocol {
        &LOGITECH_HIDPP_20
    }
}

pub static LOGITECH_HIDPP_20: LogitechHidpp20 = LogitechHidpp20;
pub static LOGITECH_HIDPP_DEVICE: LogitechHidppDevice = LogitechHidppDevice;

pub const LOGITECH_VENDOR_ID: u16 = 0x046d;
pub const FEATURE_ROOT: u16 = 0x0000;
pub const FEATURE_SET: u16 = 0x0001;
pub const FEATURE_DEVICE_NAME: u16 = 0x0005;
pub const FEATURE_BATTERY_STATUS: u16 = 0x1000;
pub const FEATURE_UNIFIED_BATTERY: u16 = 0x1004;
pub const LOGITECH_HIDPP_USAGE_PAGE: u16 = 0xff00;
pub const LOGITECH_HIDPP_LONG_USAGE: u16 = 0x0002;
pub const FUNCTION_GET_FEATURE: u8 = 0x00;
pub const FUNCTION_GET_FEATURE_SET_COUNT: u8 = 0x00;
pub const FUNCTION_GET_FEATURE_SET_ID: u8 = 0x01;
pub const FUNCTION_GET_DEVICE_NAME: u8 = 0x00;
pub const FUNCTION_GET_DEVICE_NAME_FRAGMENT: u8 = 0x01;
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

    pub fn payload_len(self) -> usize {
        match self {
            Self::Short => SHORT_PARAMS_LEN,
            Self::Long => LONG_PARAMS_LEN,
        }
    }

    pub fn frame_len(self) -> usize {
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

    #[cfg(any(feature = "hidapi", test))]
    fn is_error_response(&self) -> bool {
        self.feature_index == 0xff
    }
}

/// The identity fields used when correlating a response to a request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameIdentity {
    pub format: FrameFormat,
    pub device_index: u8,
    pub feature_index: u8,
    pub function_id: u8,
    pub software_id: u8,
}

impl From<&HidppFrame> for FrameIdentity {
    fn from(frame: &HidppFrame) -> Self {
        Self {
            format: frame.format,
            device_index: frame.device_index,
            feature_index: frame.feature_index,
            function_id: frame.function_id,
            software_id: frame.software_id,
        }
    }
}

/// A feature's runtime index and metadata as returned by Root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FeatureInfo {
    pub feature_index: u8,
    pub feature_type: u8,
    pub version: u8,
}

/// Allocates HID++ software IDs 1 through 15, cycling without using 0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoftwareIdAllocator {
    next: u8,
}

impl SoftwareIdAllocator {
    pub const fn new() -> Self {
        Self { next: 1 }
    }

    pub fn allocate(&mut self) -> u8 {
        let allocated = self.next;
        self.next = if allocated == 0x0f { 1 } else { allocated + 1 };
        allocated
    }
}

impl Default for SoftwareIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// A low-level HID++ exchange. Implementations return only a correlated reply.
pub trait HidppTransport {
    fn transact(
        &mut self,
        request: &HidppFrame,
        timeout: Duration,
    ) -> Result<HidppFrame, HidppError>;
}

/// Errors returned by frame decoding, transport, battery parsing, or HID access.
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
    NoMatchingResponse {
        expected: FrameIdentity,
        last_received: FrameIdentity,
        ignored_count: usize,
    },
    RemoteError {
        device_index: u8,
        feature_index: u8,
        function_id: u8,
        software_id: u8,
        error_code: Option<u8>,
    },
    ShortWrite {
        expected: usize,
        actual: usize,
    },
    Timeout,
    InvalidBatteryLevel(u8),
    UnsupportedBatteryFeature,
    UnsupportedFeatureSet,
    NoMatchingInterface {
        selector: String,
    },
    AmbiguousInterface {
        selector: String,
        matching_paths: Vec<String>,
    },
    #[cfg(feature = "hidapi")]
    HidApi(hidapi::HidError),
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
            Self::NoMatchingResponse {
                expected,
                last_received,
                ignored_count,
            } => write!(
                f,
                "received {ignored_count} HID++ report(s), but none matched {expected:?}; last report was {last_received:?}"
            ),
            Self::RemoteError {
                device_index,
                feature_index,
                function_id,
                software_id,
                error_code,
            } => write!(
                f,
                "device returned HID++ error {:?} for device 0x{device_index:02x}, feature 0x{feature_index:02x}, function 0x{function_id:02x}, software ID 0x{software_id:02x}",
                error_code
            ),
            Self::ShortWrite { expected, actual } => write!(
                f,
                "short HID++ Output report write: expected {expected} bytes, wrote {actual}"
            ),
            Self::Timeout => f.write_str("timed out waiting for a HID++ response"),
            Self::InvalidBatteryLevel(level) => {
                write!(f, "invalid battery percentage {level}; expected 0..=100")
            }
            Self::UnsupportedBatteryFeature => {
                f.write_str("device does not expose a supported battery feature")
            }
            Self::UnsupportedFeatureSet => {
                f.write_str("device does not expose the HID++ FeatureSet feature")
            }
            Self::NoMatchingInterface { selector } => {
                write!(f, "no HID interface matched selector {selector}")
            }
            Self::AmbiguousInterface {
                selector,
                matching_paths,
            } => write!(
                f,
                "multiple HID interfaces matched selector {selector}; narrow selection by interface, usage, or path ({})",
                matching_paths.join(", ")
            ),
            #[cfg(feature = "hidapi")]
            Self::HidApi(error) => write!(f, "HID API error: {error}"),
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

#[cfg(any(feature = "hidapi", test))]
pub(crate) trait ReportIo {
    fn write_output(&mut self, bytes: &[u8]) -> Result<usize, HidppError>;
    fn read_timeout(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize, HidppError>;
}

#[cfg(any(feature = "hidapi", test))]
pub(crate) fn transact_io(
    io: &mut impl ReportIo,
    request: &HidppFrame,
    timeout: Duration,
) -> Result<HidppFrame, HidppError> {
    let started = Instant::now();
    let request_bytes = request.encode()?;
    let written = io.write_output(&request_bytes)?;
    if written != request_bytes.len() {
        return Err(HidppError::ShortWrite {
            expected: request_bytes.len(),
            actual: written,
        });
    }

    let mut buffer = [0u8; LONG_FRAME_LEN];
    let mut last_received = None;
    let mut ignored_count = 0;
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return match last_received {
                Some(last_received) => Err(HidppError::NoMatchingResponse {
                    expected: FrameIdentity::from(request),
                    last_received,
                    ignored_count,
                }),
                None => Err(HidppError::Timeout),
            };
        }
        let bytes_read = io.read_timeout(&mut buffer, remaining)?;
        if bytes_read == 0 {
            return match last_received {
                Some(last_received) => Err(HidppError::NoMatchingResponse {
                    expected: FrameIdentity::from(request),
                    last_received,
                    ignored_count,
                }),
                None => Err(HidppError::Timeout),
            };
        }
        let response = HidppFrame::parse(&buffer[..bytes_read])?;
        let response_function_software = (response.function_id << 4) | response.software_id;
        let request_function_software = (request.function_id << 4) | request.software_id;
        if response.is_error_response()
            && response.device_index == request.device_index
            && response_function_software == request.feature_index
            && response.params.first() == Some(&request_function_software)
        {
            return Err(HidppError::RemoteError {
                device_index: request.device_index,
                feature_index: request.feature_index,
                function_id: request.function_id,
                software_id: request.software_id,
                error_code: response.params.get(1).copied(),
            });
        }

        if response.device_index == request.device_index
            && response.feature_index == request.feature_index
            && response.function_id == request.function_id
            && response.software_id == request.software_id
        {
            return Ok(response);
        }
        last_received = Some(FrameIdentity::from(&response));
        ignored_count += 1;
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct MockIo {
        writes: Vec<Vec<u8>>,
        reads: VecDeque<Vec<u8>>,
        timeouts: Vec<Duration>,
        short_write: Option<usize>,
    }

    impl ReportIo for MockIo {
        fn write_output(&mut self, bytes: &[u8]) -> Result<usize, HidppError> {
            self.writes.push(bytes.to_vec());
            Ok(self.short_write.unwrap_or(bytes.len()))
        }

        fn read_timeout(
            &mut self,
            _buffer: &mut [u8],
            timeout: Duration,
        ) -> Result<usize, HidppError> {
            self.timeouts.push(timeout);
            let Some(bytes) = self.reads.pop_front() else {
                return Ok(0);
            };
            _buffer[..bytes.len()].copy_from_slice(&bytes);
            Ok(bytes.len())
        }
    }

    fn response(request: &HidppFrame) -> HidppFrame {
        match request.format {
            FrameFormat::Short => HidppFrame::short(
                request.device_index,
                request.feature_index,
                request.function_id,
                request.software_id,
                [0x12, 0x34, 0x56],
            ),
            FrameFormat::Long => HidppFrame::long(
                request.device_index,
                request.feature_index,
                request.function_id,
                request.software_id,
                [0x12; 16],
            ),
        }
    }

    #[test]
    fn allocator_cycles_through_nonzero_software_ids() {
        let mut ids = SoftwareIdAllocator::new();
        let values: Vec<_> = (0..17).map(|_| ids.allocate()).collect();
        assert_eq!(
            values,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 1, 2]
        );
    }

    #[test]
    fn transport_writes_short_and_long_reports_and_requires_a_reply() {
        let short = HidppFrame::short(1, 2, 3, 4, [5, 6, 7]);
        let mut short_io = MockIo::default();
        short_io.reads.push_back(response(&short).encode().unwrap());
        assert_eq!(
            transact_io(&mut short_io, &short, Duration::from_secs(1)).unwrap(),
            response(&short)
        );
        assert_eq!(short_io.writes[0], [0x10, 1, 2, 0x34, 5, 6, 7]);

        let long = HidppFrame::long(1, 2, 3, 4, [5; 16]);
        let mut long_io = MockIo::default();
        long_io.reads.push_back(response(&long).encode().unwrap());
        assert_eq!(
            transact_io(&mut long_io, &long, Duration::from_secs(1)).unwrap(),
            response(&long)
        );
        assert_eq!(long_io.writes[0].len(), 20);
    }

    #[test]
    fn transport_rejects_short_writes_even_if_a_reply_is_queued() {
        let request = HidppFrame::short(1, 2, 3, 4, [5, 6, 7]);
        let mut io = MockIo {
            short_write: Some(6),
            ..MockIo::default()
        };
        io.reads.push_back(response(&request).encode().unwrap());
        assert!(matches!(
            transact_io(&mut io, &request, Duration::from_secs(1)),
            Err(HidppError::ShortWrite {
                expected: 7,
                actual: 6
            })
        ));
        assert!(io.timeouts.is_empty());
    }

    #[test]
    fn transport_ignores_only_until_deadline_and_reports_last_mismatch() {
        let request = HidppFrame::long(1, 6, 1, 3, [0; 16]);
        let mut wrong = vec![
            HidppFrame::long(1, 6, 1, 4, [0; 16]),
            HidppFrame::long(2, 6, 1, 3, [0; 16]),
            HidppFrame::long(1, 7, 1, 3, [0; 16]),
            HidppFrame::long(1, 6, 2, 3, [0; 16]),
        ];
        let expected = FrameIdentity::from(&request);
        let last = FrameIdentity::from(wrong.last().unwrap());
        let mut io = MockIo {
            reads: wrong
                .drain(..)
                .map(|frame| frame.encode().unwrap())
                .collect(),
            ..MockIo::default()
        };
        assert!(matches!(
            transact_io(&mut io, &request, Duration::from_secs(1)),
            Err(HidppError::NoMatchingResponse {
                expected: actual_expected,
                last_received,
                ignored_count: 4
            }) if actual_expected == expected && last_received == last
        ));
    }

    #[test]
    fn transport_returns_protocol_errors_instead_of_success_frames() {
        let request = HidppFrame::long(1, 6, 1, 3, [0; 16]);
        let mut error_params = [0u8; 16];
        error_params[..2].copy_from_slice(&[0x13, 0x04]);
        let error = HidppFrame::long(1, 0xff, 0, 6, error_params);
        assert_eq!(
            &error.encode().unwrap()[..6],
            &[0x11, 0x01, 0xff, 0x06, 0x13, 0x04]
        );
        let mut io = MockIo::default();
        io.reads.push_back(error.encode().unwrap());
        assert!(matches!(
            transact_io(&mut io, &request, Duration::from_secs(1)),
            Err(HidppError::RemoteError {
                device_index: 1,
                feature_index: 6,
                function_id: 1,
                software_id: 3,
                error_code: Some(4)
            })
        ));
    }

    #[test]
    fn function_index_f_is_not_mistaken_for_an_error() {
        let request = HidppFrame::long(1, 6, 0x0f, 3, [0; 16]);
        let response = HidppFrame::long(1, 6, 0x0f, 3, [0x42; 16]);
        let mut io = MockIo::default();
        io.reads.push_back(response.encode().unwrap());
        assert!(matches!(
            transact_io(&mut io, &request, Duration::from_secs(1)),
            Ok(actual) if actual == response
        ));
    }

    #[test]
    fn mismatched_error_response_is_not_correlated() {
        let request = HidppFrame::long(1, 6, 1, 3, [0; 16]);
        let mut error_params = [0u8; 16];
        error_params[..2].copy_from_slice(&[0x12, 0x04]);
        let error = HidppFrame::long(1, 0xff, 0, 6, error_params);
        let mut io = MockIo::default();
        io.reads.push_back(error.encode().unwrap());
        assert!(matches!(
            transact_io(&mut io, &request, Duration::from_secs(1)),
            Err(HidppError::NoMatchingResponse {
                last_received: FrameIdentity {
                    feature_index: 0xff,
                    ..
                },
                ignored_count: 1,
                ..
            })
        ));
    }

    #[test]
    fn timeout_is_passed_as_a_total_remaining_budget() {
        let request = HidppFrame::short(1, 2, 3, 4, [5, 6, 7]);
        let mut io = MockIo::default();
        assert!(matches!(
            transact_io(&mut io, &request, Duration::from_millis(25)),
            Err(HidppError::Timeout)
        ));
        assert_eq!(io.timeouts.len(), 1);
        assert!(io.timeouts[0] <= Duration::from_millis(25));
        assert!(!io.timeouts[0].is_zero());
    }
}
