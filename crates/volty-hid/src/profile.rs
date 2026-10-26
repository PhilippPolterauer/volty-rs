use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use crate::FrameFormat;

/// A validated, declarative set of HID++ commands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    pub schema_version: u32,
    pub name: String,
    pub commands: BTreeMap<String, Command>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    ReadOnly,
    StateChanging,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    FeatureLookup,
    FeatureCall,
}

/// An individual validated command from a profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub description: String,
    pub access: Access,
    pub operation: Operation,
    pub feature_id: u16,
    pub function_index: u8,
    pub report: FrameFormat,
    pub request_params: Vec<u8>,
    pub timeout: Duration,
    pub decoder: Decoder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endianness {
    Big,
    Little,
}

/// Supported response decoders. Offsets and lengths are measured in response
/// parameter bytes, excluding the HID++ report header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decoder {
    Raw {
        offset: usize,
        length: Option<usize>,
    },
    UnsignedInteger {
        offset: usize,
        length: usize,
        endianness: Endianness,
    },
    BitFlags {
        offset: usize,
        flags: BTreeMap<String, u8>,
    },
    Enum {
        offset: usize,
        values: BTreeMap<u64, String>,
    },
    UnifiedBatteryStatus {
        percentage: bool,
        rechargeable: bool,
        reported_levels: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileError {
    pub field: String,
    pub message: String,
}

impl ProfileError {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for ProfileError {}

impl Profile {
    /// Parse TOML and validate the complete profile before it can be executed.
    pub fn parse(source: &str) -> Result<Self, ProfileError> {
        Self::from_toml(source)
    }

    /// Parse TOML and validate the complete profile before it can be executed.
    pub fn from_toml(source: &str) -> Result<Self, ProfileError> {
        let value: toml::Value = toml::from_str(source)
            .map_err(|error| ProfileError::new("<document>", error.to_string()))?;
        Self::from_value(value)
    }

    /// Load and validate a profile from a UTF-8 TOML file.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, ProfileLoadError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|source| ProfileLoadError {
            path: path.to_owned(),
            source: ProfileLoadErrorSource::Io(source),
        })?;
        Self::from_toml(&source).map_err(|source| ProfileLoadError {
            path: path.to_owned(),
            source: ProfileLoadErrorSource::Profile(source),
        })
    }

    /// Validate an already parsed TOML value.
    pub fn from_value(value: toml::Value) -> Result<Self, ProfileError> {
        let root = value
            .as_table()
            .ok_or_else(|| ProfileError::new("<document>", "expected a TOML table"))?;
        reject_unknown(root, &["profile", "commands"], "<document>")?;

        let profile = required_table(root, "profile", "<document>")?;
        reject_unknown(profile, &["schema_version", "name"], "profile")?;

        let schema_version = required_integer(profile, "schema_version", "profile")?;
        if schema_version != 1 {
            return Err(ProfileError::new(
                "profile.schema_version",
                "unsupported schema version; expected 1",
            ));
        }

        let name = required_string(profile, "name", "profile")?;
        if name.trim().is_empty() {
            return Err(ProfileError::new(
                "profile.name",
                "must not be empty or whitespace",
            ));
        }

        let command_values = required_table(root, "commands", "<document>")?;
        if command_values.is_empty() {
            return Err(ProfileError::new(
                "commands",
                "must contain at least one command",
            ));
        }
        let mut commands = BTreeMap::new();
        for (name, value) in command_values {
            if name.trim().is_empty() {
                return Err(ProfileError::new(
                    "commands",
                    "command names must not be empty",
                ));
            }
            let path = format!("commands.{name}");
            let table = value
                .as_table()
                .ok_or_else(|| ProfileError::new(&path, "expected a command table"))?;
            let command = Command::from_table(table, &path)?;
            commands.insert(name.clone(), command);
        }

        Ok(Self {
            schema_version: 1,
            name,
            commands,
        })
    }
}

/// Preferred CLI-facing name for a parsed command profile.
pub type CommandProfile = Profile;

#[derive(Debug)]
pub struct ProfileLoadError {
    pub path: std::path::PathBuf,
    pub source: ProfileLoadErrorSource,
}

#[derive(Debug)]
pub enum ProfileLoadErrorSource {
    Io(std::io::Error),
    Profile(ProfileError),
}

impl fmt::Display for ProfileLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            ProfileLoadErrorSource::Io(error) => {
                write!(f, "failed to read profile {}: {error}", self.path.display())
            }
            ProfileLoadErrorSource::Profile(error) => {
                write!(f, "invalid profile {}: {error}", self.path.display())
            }
        }
    }
}

impl std::error::Error for ProfileLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.source {
            ProfileLoadErrorSource::Io(error) => Some(error),
            ProfileLoadErrorSource::Profile(error) => Some(error),
        }
    }
}

impl Command {
    fn from_table(
        table: &toml::map::Map<String, toml::Value>,
        path: &str,
    ) -> Result<Self, ProfileError> {
        const FIELDS: &[&str] = &[
            "description",
            "access",
            "operation",
            "feature_id",
            "function_index",
            "report",
            "request_params",
            "timeout_ms",
            "decoder",
        ];
        reject_unknown(table, FIELDS, path)?;

        let description = required_string(table, "description", path)?;
        if description.trim().is_empty() {
            return Err(field_error(path, "description", "must not be empty"));
        }

        let access = match required_string(table, "access", path)?.as_str() {
            "read_only" => Access::ReadOnly,
            "state_changing" => Access::StateChanging,
            other => {
                return Err(field_error(
                    path,
                    "access",
                    format!("unknown access {other:?}; expected read_only or state_changing"),
                ))
            }
        };

        let operation = match required_string(table, "operation", path)?.as_str() {
            "feature_lookup" => Operation::FeatureLookup,
            "feature_call" => Operation::FeatureCall,
            other => {
                return Err(field_error(
                    path,
                    "operation",
                    format!("unknown operation {other:?}"),
                ))
            }
        };

        // Both operations need a logical ID: a lookup command queries this ID
        // directly, while a call resolves it afresh before every feature call.
        let feature_id = required_integer(table, "feature_id", path)?;
        if !(0..=u16::MAX as i64).contains(&feature_id) {
            return Err(field_error(
                path,
                "feature_id",
                "must be in the range 0..=65535",
            ));
        }

        let function_index = required_integer(table, "function_index", path)?;
        if !(0..=15).contains(&function_index) {
            return Err(field_error(
                path,
                "function_index",
                "must fit in the HID++ function nibble (0..=15)",
            ));
        }

        let report = match required_string(table, "report", path)?.as_str() {
            "short" => FrameFormat::Short,
            "long" => FrameFormat::Long,
            other => {
                return Err(field_error(
                    path,
                    "report",
                    format!("unknown report {other:?}; expected short or long"),
                ))
            }
        };
        let capacity = match report {
            FrameFormat::Short => 3,
            FrameFormat::Long => 16,
        };
        let request_params = required_byte_array(table, "request_params", path)?;
        if request_params.len() > capacity {
            return Err(field_error(
                path,
                "request_params",
                format!(
                    "{} parameter bytes exceed the {report:?} report capacity of {capacity}",
                    request_params.len()
                ),
            ));
        }

        let timeout_ms = required_integer(table, "timeout_ms", path)?;
        if !(1..=120_000).contains(&timeout_ms) {
            return Err(field_error(
                path,
                "timeout_ms",
                "must be in the range 1..=120000 milliseconds",
            ));
        }

        let decoder_value = table
            .get("decoder")
            .ok_or_else(|| field_error(path, "decoder", "is required"))?;
        let decoder = Decoder::from_value(decoder_value, path)?;

        if operation == Operation::FeatureLookup {
            if report != FrameFormat::Long {
                return Err(field_error(
                    path,
                    "report",
                    "feature_lookup always uses the long root report",
                ));
            }
            if function_index != 0 {
                return Err(field_error(
                    path,
                    "function_index",
                    "feature_lookup always uses root function 0",
                ));
            }
            if !request_params.is_empty() {
                return Err(field_error(
                    path,
                    "request_params",
                    "feature_lookup fills its own logical feature ID; use an empty list",
                ));
            }
        }

        Ok(Self {
            description,
            access,
            operation,
            feature_id: feature_id as u16,
            function_index: function_index as u8,
            report,
            request_params,
            timeout: Duration::from_millis(timeout_ms as u64),
            decoder,
        })
    }
}

impl Decoder {
    fn from_value(value: &toml::Value, command_path: &str) -> Result<Self, ProfileError> {
        let field = format!("{command_path}.decoder");
        if let Some(name) = value.as_str() {
            return match name {
                "raw" => Ok(Self::Raw {
                    offset: 0,
                    length: None,
                }),
                "unified_battery_status" => Ok(Self::UnifiedBatteryStatus {
                    percentage: true,
                    rechargeable: false,
                    reported_levels: 0,
                }),
                other => Err(ProfileError::new(
                    field,
                    format!("unknown decoder {other:?}"),
                )),
            };
        }

        let table = value
            .as_table()
            .ok_or_else(|| ProfileError::new(&field, "expected a decoder name or table"))?;
        let decoder_type = required_string(table, "type", &field)?;
        match decoder_type.as_str() {
            "raw" => {
                reject_unknown(table, &["type", "offset", "length"], &field)?;
                let offset = optional_usize(table, "offset", 0, &field)?;
                let length = optional_usize_option(table, "length", &field)?;
                Ok(Self::Raw { offset, length })
            }
            "unsigned_integer" => {
                reject_unknown(table, &["type", "offset", "length", "endianness"], &field)?;
                let offset = optional_usize(table, "offset", 0, &field)?;
                let length = required_integer(table, "length", &field)?;
                if !matches!(length, 1 | 2 | 4 | 8) {
                    return Err(ProfileError::new(
                        format!("{field}.length"),
                        "must be one of 1, 2, 4, or 8 bytes",
                    ));
                }
                let endianness = match required_string(table, "endianness", &field)?.as_str() {
                    "big" => Endianness::Big,
                    "little" => Endianness::Little,
                    other => {
                        return Err(ProfileError::new(
                            format!("{field}.endianness"),
                            format!("unknown byte order {other:?}; expected big or little"),
                        ))
                    }
                };
                Ok(Self::UnsignedInteger {
                    offset,
                    length: length as usize,
                    endianness,
                })
            }
            "bit_flags" => {
                reject_unknown(table, &["type", "offset", "flags"], &field)?;
                let offset = optional_usize(table, "offset", 0, &field)?;
                let flags_value = table
                    .get("flags")
                    .ok_or_else(|| ProfileError::new(format!("{field}.flags"), "is required"))?;
                let flags_table = flags_value.as_table().ok_or_else(|| {
                    ProfileError::new(
                        format!("{field}.flags"),
                        "expected a table of flag bit positions",
                    )
                })?;
                if flags_table.is_empty() {
                    return Err(ProfileError::new(
                        format!("{field}.flags"),
                        "must contain at least one named flag",
                    ));
                }
                let mut flags = BTreeMap::new();
                for (name, bit_value) in flags_table {
                    if name.trim().is_empty() {
                        return Err(ProfileError::new(
                            format!("{field}.flags"),
                            "flag names must not be empty",
                        ));
                    }
                    let bit = bit_value.as_integer().ok_or_else(|| {
                        ProfileError::new(
                            format!("{field}.flags.{name}"),
                            "expected an integer bit position",
                        )
                    })?;
                    if !(0..=7).contains(&bit) {
                        return Err(ProfileError::new(
                            format!("{field}.flags.{name}"),
                            "bit position must be in the range 0..=7",
                        ));
                    }
                    flags.insert(name.clone(), bit as u8);
                }
                Ok(Self::BitFlags { offset, flags })
            }
            "enum" => {
                reject_unknown(table, &["type", "offset", "values"], &field)?;
                let offset = optional_usize(table, "offset", 0, &field)?;
                let values_value = table
                    .get("values")
                    .ok_or_else(|| ProfileError::new(format!("{field}.values"), "is required"))?;
                let values_table = values_value.as_table().ok_or_else(|| {
                    ProfileError::new(
                        format!("{field}.values"),
                        "expected a table of integer values",
                    )
                })?;
                if values_table.is_empty() {
                    return Err(ProfileError::new(
                        format!("{field}.values"),
                        "must contain at least one enum value",
                    ));
                }
                let mut values = BTreeMap::new();
                for (raw_value, label) in values_table {
                    let numeric_value = raw_value.parse::<u64>().map_err(|_| {
                        ProfileError::new(
                            format!("{field}.values.{raw_value}"),
                            "enum keys must be unsigned decimal integers",
                        )
                    })?;
                    let label = label.as_str().ok_or_else(|| {
                        ProfileError::new(
                            format!("{field}.values.{raw_value}"),
                            "expected a string label",
                        )
                    })?;
                    if label.trim().is_empty() {
                        return Err(ProfileError::new(
                            format!("{field}.values.{raw_value}"),
                            "enum labels must not be empty",
                        ));
                    }
                    if values.insert(numeric_value, label.to_owned()).is_some() {
                        return Err(ProfileError::new(
                            format!("{field}.values"),
                            format!("duplicate enum numeric value {numeric_value}"),
                        ));
                    }
                }
                Ok(Self::Enum { offset, values })
            }
            "unified_battery_status" => {
                reject_unknown(
                    table,
                    &["type", "percentage", "rechargeable", "reported_levels"],
                    &field,
                )?;
                let percentage = optional_bool(table, "percentage", true, &field)?;
                let rechargeable = optional_bool(table, "rechargeable", false, &field)?;
                let reported_levels = optional_u8(table, "reported_levels", 0, &field)?;
                Ok(Self::UnifiedBatteryStatus {
                    percentage,
                    rechargeable,
                    reported_levels,
                })
            }
            other => Err(ProfileError::new(
                field,
                format!("unknown decoder type {other:?}"),
            )),
        }
    }
}

fn reject_unknown(
    table: &toml::map::Map<String, toml::Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), ProfileError> {
    if let Some(key) = table.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ProfileError::new(
            if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            },
            "unknown field",
        ));
    }
    Ok(())
}

fn required_table<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    key: &str,
    path: &str,
) -> Result<&'a toml::map::Map<String, toml::Value>, ProfileError> {
    table
        .get(key)
        .ok_or_else(|| field_error(path, key, "is required"))?
        .as_table()
        .ok_or_else(|| field_error(path, key, "expected a table"))
}

fn required_string(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    path: &str,
) -> Result<String, ProfileError> {
    table
        .get(key)
        .ok_or_else(|| field_error(path, key, "is required"))?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| field_error(path, key, "expected a string"))
}

fn required_integer(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    path: &str,
) -> Result<i64, ProfileError> {
    table
        .get(key)
        .ok_or_else(|| field_error(path, key, "is required"))?
        .as_integer()
        .ok_or_else(|| field_error(path, key, "expected an integer"))
}

fn required_byte_array(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    path: &str,
) -> Result<Vec<u8>, ProfileError> {
    let value = table
        .get(key)
        .ok_or_else(|| field_error(path, key, "is required"))?;
    let array = value
        .as_array()
        .ok_or_else(|| field_error(path, key, "expected an array of byte integers"))?;
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let byte = value.as_integer().ok_or_else(|| {
                field_error(
                    path,
                    key,
                    format!("element {index} must be an integer in the range 0..=255"),
                )
            })?;
            u8::try_from(byte).map_err(|_| {
                field_error(
                    path,
                    key,
                    format!("element {index} must be in the range 0..=255"),
                )
            })
        })
        .collect()
}

fn optional_usize(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: usize,
    path: &str,
) -> Result<usize, ProfileError> {
    let Some(value) = table.get(key) else {
        return Ok(default);
    };
    let integer = value
        .as_integer()
        .ok_or_else(|| field_error(path, key, "expected a nonnegative integer"))?;
    usize::try_from(integer).map_err(|_| field_error(path, key, "must be a nonnegative integer"))
}

fn optional_usize_option(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    path: &str,
) -> Result<Option<usize>, ProfileError> {
    table
        .contains_key(key)
        .then(|| optional_usize(table, key, 0, path))
        .transpose()
}

fn optional_bool(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: bool,
    path: &str,
) -> Result<bool, ProfileError> {
    match table.get(key) {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| field_error(path, key, "expected a boolean")),
        None => Ok(default),
    }
}

fn optional_u8(
    table: &toml::map::Map<String, toml::Value>,
    key: &str,
    default: u8,
    path: &str,
) -> Result<u8, ProfileError> {
    match table.get(key) {
        Some(value) => {
            let integer = value.as_integer().ok_or_else(|| {
                field_error(path, key, "expected an integer in the range 0..=255")
            })?;
            u8::try_from(integer)
                .map_err(|_| field_error(path, key, "must be in the range 0..=255"))
        }
        None => Ok(default),
    }
}

fn field_error(path: &str, field: &str, message: impl Into<String>) -> ProfileError {
    ProfileError::new(format!("{path}.{field}"), message)
}

#[cfg(test)]
mod tests {
    use super::{Access, CommandProfile, Decoder, Endianness, Operation};

    const VALID_PROFILE: &str = r#"
        [profile]
        schema_version = 1
        name = "battery"

        [commands.status]
        description = "Read battery status"
        access = "read_only"
        operation = "feature_call"
        feature_id = 4100
        function_index = 1
        report = "long"
        request_params = []
        timeout_ms = 1000
        decoder = "unified_battery_status"
    "#;

    #[test]
    fn parses_typed_profile_and_unified_battery_decoder() {
        let profile = CommandProfile::from_toml(VALID_PROFILE).unwrap();
        let command = &profile.commands["status"];
        assert_eq!(profile.name, "battery");
        assert_eq!(command.access, Access::ReadOnly);
        assert_eq!(command.operation, Operation::FeatureCall);
        assert_eq!(command.feature_id, 0x1004);
        assert_eq!(
            command.decoder,
            Decoder::UnifiedBatteryStatus {
                percentage: true,
                rechargeable: false,
                reported_levels: 0
            }
        );
    }

    #[test]
    fn root_feature_id_zero_is_valid() {
        let source = VALID_PROFILE.replace("feature_id = 4100", "feature_id = 0");
        assert_eq!(
            CommandProfile::from_toml(&source).unwrap().commands["status"].feature_id,
            0
        );
    }

    #[test]
    fn accepts_integer_flags_and_enum_decoder_definitions() {
        let unsigned = VALID_PROFILE.replace(
            r#"decoder = "unified_battery_status""#,
            r#"decoder = { type = "unsigned_integer", offset = 0, length = 2, endianness = "little" }"#,
        );
        assert!(matches!(
            CommandProfile::from_toml(&unsigned).unwrap().commands["status"].decoder,
            Decoder::UnsignedInteger {
                length: 2,
                endianness: Endianness::Little,
                ..
            }
        ));

        let flags = VALID_PROFILE.replace(
            r#"decoder = "unified_battery_status""#,
            r#"decoder = { type = "bit_flags", flags = { charging = 0, percentage = 1 } }"#,
        );
        assert!(matches!(
            CommandProfile::from_toml(&flags).unwrap().commands["status"].decoder,
            Decoder::BitFlags { .. }
        ));

        let enumeration = VALID_PROFILE.replace(
            r#"decoder = "unified_battery_status""#,
            r#"decoder = { type = "enum", values = { "0" = "off", "1" = "on" } }"#,
        );
        assert!(matches!(
            CommandProfile::from_toml(&enumeration).unwrap().commands["status"].decoder,
            Decoder::Enum { .. }
        ));
    }

    #[test]
    fn rejects_wrong_types_unknown_fields_and_unknown_decoders() {
        let cases = [
            (
                VALID_PROFILE.replace("feature_id = 4100", r#"feature_id = "4100""#),
                "commands.status.feature_id",
            ),
            (
                VALID_PROFILE.replace(
                    "name = \"battery\"",
                    "name = \"battery\"\n        protocol = \"hidpp20\"",
                ),
                "profile.protocol",
            ),
            (
                VALID_PROFILE.replace("description = ", "extra = 1\n        description = "),
                "commands.status.extra",
            ),
            (
                VALID_PROFILE.replace(
                    r#"decoder = "unified_battery_status""#,
                    r#"decoder = "run_script""#,
                ),
                "commands.status.decoder",
            ),
            (
                VALID_PROFILE.replace(
                    r#"operation = "feature_call""#,
                    r#"operation = "feature_set""#,
                ),
                "commands.status.operation",
            ),
        ];
        for (source, field) in cases {
            let error = CommandProfile::from_toml(&source).unwrap_err();
            assert!(
                error.to_string().contains(field),
                "expected field {field:?} in error {error}"
            );
        }
    }

    #[test]
    fn rejects_invalid_ids_access_report_sizes_and_timeouts() {
        let cases = [
            (
                VALID_PROFILE.replace("feature_id = 4100", "feature_id = -1"),
                "feature_id",
            ),
            (
                VALID_PROFILE.replace(r#"access = "read_only""#, r#"access = "admin""#),
                "access",
            ),
            (
                VALID_PROFILE
                    .replace(r#"report = "long""#, r#"report = "short""#)
                    .replace("request_params = []", "request_params = [0, 1, 2, 3]"),
                "request_params",
            ),
            (
                VALID_PROFILE.replace("timeout_ms = 1000", "timeout_ms = 0"),
                "timeout_ms",
            ),
        ];
        for (source, field) in cases {
            let error = CommandProfile::from_toml(&source).unwrap_err();
            assert!(
                error.to_string().contains(field),
                "expected field {field:?} in error {error}"
            );
        }
    }

    #[test]
    fn accepts_exact_short_and_long_parameter_boundaries() {
        let short = VALID_PROFILE
            .replace(r#"report = "long""#, r#"report = "short""#)
            .replace("request_params = []", "request_params = [1, 2, 3]");
        assert!(CommandProfile::from_toml(&short).is_ok());

        let long = VALID_PROFILE.replace(
            "request_params = []",
            "request_params = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]",
        );
        assert!(CommandProfile::from_toml(&long).is_ok());
    }
}
