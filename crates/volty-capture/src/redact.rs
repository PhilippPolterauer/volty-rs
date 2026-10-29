use crate::PrivacyProfile;

/// Applies a privacy profile to optional human-readable device strings.
///
/// A HID device path or serial number value must never be passed to any method
/// on this type. Those values are not representable in the capture schema; only
/// numeric identifiers and the [`crate::CaptureInterface::serial_present`] flag
/// may be recorded.
#[derive(Clone, Copy, Debug)]
pub struct Redactor {
    profile: PrivacyProfile,
}

impl Redactor {
    pub const fn new(profile: PrivacyProfile) -> Self {
        Self { profile }
    }

    pub const fn profile(&self) -> PrivacyProfile {
        self.profile
    }

    /// Keeps the product string for [`PrivacyProfile::Standard`] only.
    pub fn product(&self, value: Option<&str>) -> Option<String> {
        self.model_string(value)
    }

    /// Keeps the manufacturer string for [`PrivacyProfile::Standard`] only.
    pub fn manufacturer(&self, value: Option<&str>) -> Option<String> {
        self.model_string(value)
    }

    /// Custom HID++ device names are excluded from every profile.
    pub fn device_name(&self, _value: Option<&str>) -> Option<String> {
        None
    }

    fn model_string(&self, value: Option<&str>) -> Option<String> {
        match self.profile {
            PrivacyProfile::Standard => value.map(str::to_owned),
            PrivacyProfile::Strict => None,
        }
    }
}
