use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

use crate::profile::{Access, CommandProfile, Decoder, Endianness, Operation};
use crate::{
    parse_unified_battery, BatteryStatus, FrameFormat, HidppError, HidppFrame, HidppTransport,
    SoftwareIdAllocator, UnifiedBatteryCapabilities,
};

const FEATURE_ROOT_INDEX: u8 = 0;
const ROOT_GET_FEATURE_FUNCTION: u8 = 0;

/// Information returned by a HID++ root feature lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureMapping {
    pub logical_id: u16,
    pub runtime_index: u8,
    pub feature_type: u8,
    pub version: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodedOutput {
    Raw(Vec<u8>),
    UnsignedInteger(u64),
    BitFlags(BTreeMap<String, bool>),
    Enum {
        raw_value: u8,
        label: Option<String>,
    },
    UnifiedBatteryStatus(BatteryStatus),
}

/// Successful execution, including every HID++ report sent and received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandResult {
    pub profile_name: String,
    pub command_name: String,
    pub device_index: u8,
    pub request_frames: Vec<Vec<u8>>,
    pub response_frames: Vec<Vec<u8>>,
    pub feature: Option<FeatureMapping>,
    pub elapsed: Duration,
    pub decoded: DecodedOutput,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub struct CommandError {
    pub profile_name: String,
    pub command_name: String,
    pub kind: CommandErrorKind,
}

#[derive(Debug)]
pub enum CommandErrorKind {
    UnknownCommand,
    StateChangingDenied,
    InvalidSoftwareId(u8),
    Transport(HidppError),
    Protocol(HidppError),
    FeatureNotFound(u16),
    MalformedFeatureLookup,
    InvalidCommand(String),
    Decode(DecodeError),
}

#[derive(Debug)]
pub enum DecodeError {
    MalformedResponse {
        offset: usize,
        required: usize,
        actual: usize,
    },
    Battery(HidppError),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedResponse {
                offset,
                required,
                actual,
            } => write!(
                f,
                "malformed response parameters: need {required} byte(s) at offset {offset}, got {actual}"
            ),
            Self::Battery(error) => write!(f, "unified battery decoder failed: {error}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Battery(error) => Some(error),
            Self::MalformedResponse { .. } => None,
        }
    }
}

impl fmt::Display for CommandErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand => f.write_str("command is not present in the profile"),
            Self::StateChangingDenied => {
                f.write_str("state-changing command denied; explicit authorization is required")
            }
            Self::InvalidSoftwareId(id) => write!(
                f,
                "software ID allocator returned invalid ID 0x{id:02x}; expected 1..=15"
            ),
            Self::Transport(error) => write!(f, "HID++ transport failed: {error}"),
            Self::Protocol(error) => write!(f, "invalid HID++ protocol frame: {error}"),
            Self::FeatureNotFound(id) => {
                write!(f, "logical feature 0x{id:04x} was not found on this device")
            }
            Self::MalformedFeatureLookup => {
                f.write_str("root feature lookup response is missing index, type, or version")
            }
            Self::InvalidCommand(message) => f.write_str(message),
            Self::Decode(error) => write!(f, "response decoding failed: {error}"),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "profile {:?}, command {:?}: {}",
            self.profile_name, self.command_name, self.kind
        )
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            CommandErrorKind::Transport(error) | CommandErrorKind::Protocol(error) => Some(error),
            CommandErrorKind::Decode(error) => Some(error),
            _ => None,
        }
    }
}

/// Executes validated profile commands over an injected transport.
///
/// Feature indexes are resolved from the device for each feature call. A
/// fresh software ID is allocated for every root lookup and feature request.
pub struct CommandExecutor<T: HidppTransport> {
    transport: T,
    software_ids: SoftwareIdAllocator,
}

impl<T: HidppTransport> CommandExecutor<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            software_ids: SoftwareIdAllocator::default(),
        }
    }

    /// Execute one profile command for a HID++ device address.
    ///
    /// State-changing commands are rejected unless `allow_state_changes` is
    /// explicitly true. A successful result always includes a response frame;
    /// a transport write without a correlated response is never success.
    pub fn execute(
        &mut self,
        profile: &CommandProfile,
        command_name: &str,
        device_index: u8,
        allow_state_changes: bool,
    ) -> Result<CommandResult, CommandError> {
        let command = profile
            .commands
            .get(command_name)
            .ok_or_else(|| self.error(profile, command_name, CommandErrorKind::UnknownCommand))?;
        if let Err(kind) = validate_command(command) {
            return Err(self.error(profile, command_name, kind));
        }
        if command.access == Access::StateChanging && !allow_state_changes {
            return Err(self.error(profile, command_name, CommandErrorKind::StateChangingDenied));
        }

        let started = Instant::now();
        let mut request_frames = Vec::new();
        let mut response_frames = Vec::new();
        let (response, feature) = match command.operation {
            Operation::FeatureLookup => {
                let (feature, response) = self.lookup_feature(
                    profile,
                    command_name,
                    command.feature_id,
                    device_index,
                    command.timeout,
                    &mut request_frames,
                    &mut response_frames,
                )?;
                (response, Some(feature))
            }
            Operation::FeatureCall => {
                let (feature, _) = self.lookup_feature(
                    profile,
                    command_name,
                    command.feature_id,
                    device_index,
                    command.timeout,
                    &mut request_frames,
                    &mut response_frames,
                )?;
                let request = self.feature_call_frame(
                    profile,
                    command_name,
                    device_index,
                    feature.runtime_index,
                    command.function_index,
                    command.report,
                    &command.request_params,
                )?;
                let raw_request = request.encode().map_err(|error| {
                    self.error(profile, command_name, CommandErrorKind::Protocol(error))
                })?;
                request_frames.push(raw_request);
                let response =
                    self.transport
                        .transact(&request, command.timeout)
                        .map_err(|error| {
                            self.error(profile, command_name, CommandErrorKind::Transport(error))
                        })?;
                let raw_response = response.encode().map_err(|error| {
                    self.error(profile, command_name, CommandErrorKind::Protocol(error))
                })?;
                response_frames.push(raw_response);
                (response, Some(feature))
            }
        };

        let decoded = decode_response(&command.decoder, &response)
            .map_err(|error| self.error(profile, command_name, CommandErrorKind::Decode(error)))?;
        let mut warnings = Vec::new();
        if matches!(
            &decoded,
            DecodedOutput::UnifiedBatteryStatus(BatteryStatus {
                level_percent: None,
                ..
            })
        ) {
            warnings.push("device did not report a battery percentage".to_owned());
        }

        Ok(CommandResult {
            profile_name: profile.name.clone(),
            command_name: command_name.to_owned(),
            device_index,
            request_frames,
            response_frames,
            feature,
            elapsed: started.elapsed(),
            decoded,
            warnings,
        })
    }

    fn lookup_feature(
        &mut self,
        profile: &CommandProfile,
        command_name: &str,
        logical_id: u16,
        device_index: u8,
        timeout: Duration,
        request_frames: &mut Vec<Vec<u8>>,
        response_frames: &mut Vec<Vec<u8>>,
    ) -> Result<(FeatureMapping, HidppFrame), CommandError> {
        let software_id = self.software_id(profile, command_name)?;
        let mut params = [0u8; 16];
        params[0] = (logical_id >> 8) as u8;
        params[1] = logical_id as u8;
        let request = HidppFrame::long(
            device_index,
            FEATURE_ROOT_INDEX,
            ROOT_GET_FEATURE_FUNCTION,
            software_id,
            params,
        );
        let raw_request = request.encode().map_err(|error| {
            self.error(profile, command_name, CommandErrorKind::Protocol(error))
        })?;
        request_frames.push(raw_request);
        let response = self
            .transport
            .transact(&request, timeout)
            .map_err(|error| {
                self.error(profile, command_name, CommandErrorKind::Transport(error))
            })?;
        let raw_response = response.encode().map_err(|error| {
            self.error(profile, command_name, CommandErrorKind::Protocol(error))
        })?;
        response_frames.push(raw_response);

        if response.params.len() < 3 {
            return Err(self.error(
                profile,
                command_name,
                CommandErrorKind::MalformedFeatureLookup,
            ));
        }
        let runtime_index = response.params[0];
        if runtime_index == 0 && logical_id != 0 {
            return Err(self.error(
                profile,
                command_name,
                CommandErrorKind::FeatureNotFound(logical_id),
            ));
        }
        let info = FeatureMapping {
            logical_id,
            runtime_index,
            feature_type: response.params[1],
            version: response.params[2],
        };
        Ok((info, response))
    }

    fn feature_call_frame(
        &mut self,
        profile: &CommandProfile,
        command_name: &str,
        device_index: u8,
        runtime_index: u8,
        function_index: u8,
        report: FrameFormat,
        request_params: &[u8],
    ) -> Result<HidppFrame, CommandError> {
        let software_id = self.software_id(profile, command_name)?;
        let params = match report {
            FrameFormat::Short => {
                let mut params = [0u8; 3];
                params[..request_params.len()].copy_from_slice(request_params);
                HidppFrame::short(
                    device_index,
                    runtime_index,
                    function_index,
                    software_id,
                    params,
                )
            }
            FrameFormat::Long => {
                let mut params = [0u8; 16];
                params[..request_params.len()].copy_from_slice(request_params);
                HidppFrame::long(
                    device_index,
                    runtime_index,
                    function_index,
                    software_id,
                    params,
                )
            }
        };
        Ok(params)
    }

    fn software_id(
        &mut self,
        profile: &CommandProfile,
        command_name: &str,
    ) -> Result<u8, CommandError> {
        let id = self.software_ids.allocate();
        if !(1..=15).contains(&id) {
            return Err(self.error(
                profile,
                command_name,
                CommandErrorKind::InvalidSoftwareId(id),
            ));
        }
        Ok(id)
    }

    fn error(
        &self,
        profile: &CommandProfile,
        command_name: &str,
        kind: CommandErrorKind,
    ) -> CommandError {
        CommandError {
            profile_name: profile.name.clone(),
            command_name: command_name.to_owned(),
            kind,
        }
    }
}

fn validate_command(command: &crate::profile::Command) -> Result<(), CommandErrorKind> {
    if command.function_index > 0x0f {
        return Err(CommandErrorKind::InvalidCommand(
            "function_index must fit in a 4-bit HID++ function field".to_owned(),
        ));
    }
    if command.timeout.is_zero() || command.timeout > Duration::from_secs(120) {
        return Err(CommandErrorKind::InvalidCommand(
            "timeout must be in the range 1..=120000 milliseconds".to_owned(),
        ));
    }
    let capacity = match command.report {
        FrameFormat::Short => 3,
        FrameFormat::Long => 16,
    };
    if command.request_params.len() > capacity {
        return Err(CommandErrorKind::InvalidCommand(format!(
            "{} request parameter bytes exceed report capacity {capacity}",
            command.request_params.len()
        )));
    }
    if command.operation == Operation::FeatureLookup
        && (command.report != FrameFormat::Long
            || command.function_index != ROOT_GET_FEATURE_FUNCTION
            || !command.request_params.is_empty())
    {
        return Err(CommandErrorKind::InvalidCommand(
            "feature_lookup requires a long report, function 0, and no extra parameters".to_owned(),
        ));
    }
    match &command.decoder {
        Decoder::Raw { offset, length } => {
            if length.is_some_and(|length| offset.checked_add(length).is_none()) {
                return Err(CommandErrorKind::InvalidCommand(
                    "raw decoder offset plus length overflows".to_owned(),
                ));
            }
        }
        Decoder::UnsignedInteger { length, offset, .. } => {
            if !matches!(*length, 1 | 2 | 4 | 8) || offset.checked_add(*length).is_none() {
                return Err(CommandErrorKind::InvalidCommand(
                    "unsigned_integer decoder has an invalid length or offset".to_owned(),
                ));
            }
        }
        Decoder::BitFlags { offset, flags } => {
            if offset.checked_add(1).is_none() || flags.values().any(|bit| *bit > 7) {
                return Err(CommandErrorKind::InvalidCommand(
                    "bit_flags decoder has an invalid offset or bit position".to_owned(),
                ));
            }
        }
        Decoder::Enum { offset, .. } if offset.checked_add(1).is_none() => {
            return Err(CommandErrorKind::InvalidCommand(
                "enum decoder offset overflows".to_owned(),
            ));
        }
        Decoder::Enum { .. } | Decoder::UnifiedBatteryStatus { .. } => {}
    }
    Ok(())
}

fn decode_response(decoder: &Decoder, response: &HidppFrame) -> Result<DecodedOutput, DecodeError> {
    let params = &response.params;
    match decoder {
        Decoder::Raw { offset, length } => {
            let length = length.unwrap_or_else(|| params.len().saturating_sub(*offset));
            let bytes = response_slice(params, *offset, length)?;
            Ok(DecodedOutput::Raw(bytes.to_vec()))
        }
        Decoder::UnsignedInteger {
            offset,
            length,
            endianness,
        } => {
            let bytes = response_slice(params, *offset, *length)?;
            let mut value = 0u64;
            match endianness {
                Endianness::Big => {
                    for byte in bytes {
                        value = (value << 8) | u64::from(*byte);
                    }
                }
                Endianness::Little => {
                    for (shift, byte) in bytes.iter().enumerate() {
                        value |= u64::from(*byte) << (shift * 8);
                    }
                }
            }
            Ok(DecodedOutput::UnsignedInteger(value))
        }
        Decoder::BitFlags { offset, flags } => {
            let byte = response_slice(params, *offset, 1)?[0];
            Ok(DecodedOutput::BitFlags(
                flags
                    .iter()
                    .map(|(name, bit)| (name.clone(), byte & (1u8 << *bit) != 0))
                    .collect(),
            ))
        }
        Decoder::Enum { offset, values } => {
            let raw_value = response_slice(params, *offset, 1)?[0];
            Ok(DecodedOutput::Enum {
                raw_value,
                label: values.get(&u64::from(raw_value)).cloned(),
            })
        }
        Decoder::UnifiedBatteryStatus {
            percentage,
            rechargeable,
            reported_levels,
        } => {
            let capabilities = UnifiedBatteryCapabilities {
                percentage: *percentage,
                rechargeable: *rechargeable,
                reported_levels: *reported_levels,
            };
            let status =
                parse_unified_battery(response, capabilities).map_err(DecodeError::Battery)?;
            Ok(DecodedOutput::UnifiedBatteryStatus(status))
        }
    }
}

fn response_slice(params: &[u8], offset: usize, length: usize) -> Result<&[u8], DecodeError> {
    let end = offset
        .checked_add(length)
        .ok_or(DecodeError::MalformedResponse {
            offset,
            required: length,
            actual: params.len(),
        })?;
    params
        .get(offset..end)
        .ok_or(DecodeError::MalformedResponse {
            offset,
            required: length,
            actual: params.len(),
        })
}

#[cfg(test)]
mod tests {
    use super::{CommandErrorKind, CommandExecutor, DecodedOutput, FeatureMapping};
    use crate::profile::CommandProfile;
    use crate::{HidppError, HidppFrame, HidppTransport};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct MockState {
        requests: Vec<Vec<u8>>,
        responses: VecDeque<Result<HidppFrame, HidppError>>,
    }

    #[derive(Clone, Default)]
    struct MockTransport(Arc<Mutex<MockState>>);

    impl HidppTransport for MockTransport {
        fn transact(
            &mut self,
            request: &HidppFrame,
            _timeout: Duration,
        ) -> Result<HidppFrame, HidppError> {
            self.0.lock().unwrap().requests.push(request.encode()?);
            self.0
                .lock()
                .unwrap()
                .responses
                .pop_front()
                .expect("test configured a response")
        }
    }

    fn profile(decoder: &str, access: &str) -> CommandProfile {
        let source = format!(
            r#"
                [profile]
                schema_version = 1
                name = "trace"
                protocol = "hidpp20"

                [commands.root]
                description = "Look up the root feature"
                access = "read_only"
                operation = "feature_lookup"
                feature_id = 3
                function_index = 0
                report = "long"
                request_params = []
                timeout_ms = 1000
                decoder = "raw"

                [commands.status]
                description = "Read unified battery status"
                access = "{access}"
                operation = "feature_call"
                feature_id = 4100
                function_index = 1
                report = "long"
                request_params = []
                timeout_ms = 1000
                decoder = {decoder}
                "#
        );
        CommandProfile::from_toml(&source).unwrap()
    }

    fn lookup_response(request: &HidppFrame, feature_index: u8, version: u8) -> HidppFrame {
        let mut params = [0u8; 16];
        params[..3].copy_from_slice(&[feature_index, 0, version]);
        HidppFrame::long(
            request.device_index,
            request.feature_index,
            request.function_id,
            request.software_id,
            params,
        )
    }

    fn battery_response(request: &HidppFrame, params: [u8; 16]) -> HidppFrame {
        HidppFrame::long(
            request.device_index,
            request.feature_index,
            request.function_id,
            request.software_id,
            params,
        )
    }

    #[test]
    fn feature_lookup_uses_root_and_returns_runtime_mapping() {
        let source = r#"
            [profile]
            schema_version = 1
            name = "root"
            protocol = "hidpp20"
            [commands.lookup]
            description = "Look up Root logical feature"
            access = "read_only"
            operation = "feature_lookup"
            feature_id = 3
            function_index = 0
            report = "long"
            request_params = []
            timeout_ms = 1000
            decoder = "raw"
        "#;
        let profile = CommandProfile::from_toml(source).unwrap();
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut params = [0u8; 16];
        params[..3].copy_from_slice(&[0x02, 0x00, 0x04]);
        state
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(HidppFrame::long(1, 0, 0, 1, params)));
        let mut executor = CommandExecutor::new(MockTransport(state.clone()));

        let result = executor.execute(&profile, "lookup", 1, false).unwrap();
        assert_eq!(
            result.feature,
            Some(FeatureMapping {
                logical_id: 0x0003,
                runtime_index: 0x02,
                feature_type: 0x00,
                version: 0x04,
            })
        );
        assert_eq!(
            result.request_frames[0],
            vec![0x11, 0x01, 0x00, 0x01, 0x00, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,]
        );
    }

    #[test]
    fn root_logical_feature_zero_maps_to_runtime_index_zero() {
        let mut profile = profile(r#""raw""#, "read_only");
        let command = profile.commands.get_mut("status").unwrap();
        command.feature_id = 0;
        command.operation = crate::Operation::FeatureLookup;
        command.function_index = 0;

        let state = Arc::new(Mutex::new(MockState::default()));
        state
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(lookup_response(
                &HidppFrame::long(1, 0, 0, 1, [0; 16]),
                0,
                4,
            )));
        let mut executor = CommandExecutor::new(MockTransport(state));

        let result = executor.execute(&profile, "status", 1, false).unwrap();
        assert_eq!(
            result.feature,
            Some(FeatureMapping {
                logical_id: 0,
                runtime_index: 0,
                feature_type: 0,
                version: 4,
            })
        );
    }

    #[test]
    fn performs_dynamic_lookup_and_decodes_observed_battery_trace() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let transport = MockTransport(state.clone());
        let mut executor = CommandExecutor::new(transport.clone());
        let profile = profile(r#""unified_battery_status""#, "read_only");

        transport
            .0
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(lookup_response(
                &HidppFrame::long(1, 0, 0, 1, [0; 16]),
                0x02,
                0x04,
            )));
        transport
            .0
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(lookup_response(
                &HidppFrame::long(1, 0, 0, 2, [0; 16]),
                0x06,
                0x03,
            )));
        let mut battery_params = [0u8; 16];
        battery_params[..3].copy_from_slice(&[78, 8, 0]);
        transport
            .0
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(battery_response(
                &HidppFrame::long(1, 6, 1, 3, [0; 16]),
                battery_params,
            )));

        let root_result = executor.execute(&profile, "root", 1, false).unwrap();
        assert_eq!(
            root_result.feature,
            Some(FeatureMapping {
                logical_id: 0x0003,
                runtime_index: 0x02,
                feature_type: 0,
                version: 0x04,
            })
        );
        let result = executor.execute(&profile, "status", 1, false).unwrap();
        assert_eq!(
            result.feature,
            Some(FeatureMapping {
                logical_id: 0x1004,
                runtime_index: 0x06,
                feature_type: 0,
                version: 0x03,
            })
        );
        assert_eq!(result.request_frames.len(), 2);
        assert_eq!(result.response_frames.len(), 2);
        assert_eq!(
            result.request_frames[0],
            vec![0x11, 0x01, 0x00, 0x02, 0x10, 0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,]
        );
        assert_eq!(result.request_frames[1][..4], [0x11, 0x01, 0x06, 0x13]);
        assert_eq!(result.request_frames[1][4..], [0; 16]);
        assert_eq!(result.response_frames[1][4], 78);
        assert_eq!(
            &result.response_frames[1][..7],
            &[0x11, 0x01, 0x06, 0x13, 0x4e, 0x08, 0x00]
        );
        assert!(matches!(
            result.decoded,
            DecodedOutput::UnifiedBatteryStatus(ref status)
                if status.level_percent == Some(78)
        ));
    }

    #[test]
    fn denies_state_changes_before_transport_access() {
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut executor = CommandExecutor::new(MockTransport(state.clone()));
        let error = executor
            .execute(&profile(r#""raw""#, "state_changing"), "status", 1, false)
            .unwrap_err();
        assert!(matches!(error.kind, CommandErrorKind::StateChangingDenied));
        assert!(state.lock().unwrap().requests.is_empty());
    }

    #[test]
    fn returns_contextual_transport_and_decode_errors() {
        let state = Arc::new(Mutex::new(MockState::default()));
        state
            .lock()
            .unwrap()
            .responses
            .push_back(Err(HidppError::UnexpectedResponse("mock failure")));
        let mut executor = CommandExecutor::new(MockTransport(state.clone()));
        let error = executor
            .execute(&profile(r#""raw""#, "read_only"), "status", 1, false)
            .unwrap_err();
        assert_eq!(error.profile_name, "trace");
        assert_eq!(error.command_name, "status");
        assert!(matches!(error.kind, CommandErrorKind::Transport(_)));

        let mut root = [0u8; 16];
        root[..3].copy_from_slice(&[6, 0, 3]);
        state
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(HidppFrame::long(1, 0, 0, 2, root)));
        let response = HidppFrame::long(1, 6, 1, 3, [0; 16]);
        state.lock().unwrap().responses.push_back(Ok(response));
        let malformed = profile(
            r#"{ type = "unsigned_integer", offset = 16, length = 2, endianness = "big" }"#,
            "read_only",
        );
        let error = executor
            .execute(&malformed, "status", 1, false)
            .unwrap_err();
        assert!(matches!(error.kind, CommandErrorKind::Decode(_)));
    }

    #[test]
    fn builds_short_requests_with_zero_padding() {
        let source = r#"
                [profile]
                schema_version = 1
                name = "short"
                protocol = "hidpp20"
                [commands.read]
                description = "Short report"
                access = "read_only"
                operation = "feature_call"
                feature_id = 4100
                function_index = 2
                report = "short"
                request_params = [0xaa]
                timeout_ms = 1000
                decoder = "raw"
            "#;
        let profile = CommandProfile::from_toml(source).unwrap();
        let state = Arc::new(Mutex::new(MockState::default()));
        let mut root = [0u8; 16];
        root[..3].copy_from_slice(&[6, 0, 3]);
        state
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(HidppFrame::long(1, 0, 0, 1, root)));
        state
            .lock()
            .unwrap()
            .responses
            .push_back(Ok(HidppFrame::short(1, 6, 2, 2, [0x11, 0, 0])));
        let mut executor = CommandExecutor::new(MockTransport(state.clone()));
        let result = executor.execute(&profile, "read", 1, false).unwrap();
        assert_eq!(result.request_frames[1][..4], [0x10, 1, 6, 0x22]);
        assert_eq!(result.request_frames[1][4..], [0xaa, 0, 0]);
        assert_eq!(result.decoded, DecodedOutput::Raw(vec![0x11, 0, 0]));
    }
}
