use crate::BatteryStatus;
use std::fmt;

/// Opaque, stable identity of a battery-powered device.
///
/// Providers prefix their ids with a source tag (`hid:`, `bluetooth:`) so ids
/// from different sources never collide. The same id is used as the key for
/// per-device settings and persisted state, so it must survive re-enumeration.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a device is presented to the user: a name plus an optional qualifier
/// such as `slot 2` that disambiguates devices sharing one name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceLabel {
    pub name: String,
    pub qualifier: Option<String>,
}

impl DeviceLabel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            qualifier: None,
        }
    }

    #[must_use]
    pub fn with_qualifier(mut self, qualifier: impl Into<String>) -> Self {
        self.qualifier = Some(qualifier.into());
        self
    }

    pub fn display(&self) -> String {
        match &self.qualifier {
            Some(qualifier) => format!("{} ({qualifier})", self.name),
            None => self.name.clone(),
        }
    }
}

/// One battery reading attributed to a stable device identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReading {
    pub id: DeviceId,
    pub label: DeviceLabel,
    pub battery: BatteryStatus,
}

/// Progress reported by a provider while it polls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderEvent {
    Reading(SourceReading),
    /// A better name for an already-reported device became known.
    DeviceName {
        id: DeviceId,
        label: DeviceLabel,
        /// Why the preferred name could not be resolved, if it could not.
        error: Option<String>,
    },
    Warning(String),
    Trace(String),
}

/// How much work a poll should do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanMode {
    /// Look for devices that were not seen before.
    Discover,
    /// Only refresh devices that are already known.
    Refresh,
}

/// The event consumer went away; the provider should stop polling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SinkClosed;

/// Outcome of one provider poll.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct ProviderPoll {
    /// Readings emitted by this poll.
    pub readings: usize,
    /// Candidate devices this provider recognised, whether or not they answered.
    pub recognised: usize,
    /// Failures that did not stop the poll.
    pub errors: Vec<String>,
}

/// A battery source (HID protocol family, Bluetooth, ...). The tray only
/// consumes this trait and never sees protocol details.
pub trait BatteryProvider: Send {
    /// Poll this source, streaming events into `sink`.
    ///
    /// # Errors
    /// Returns [`SinkClosed`] when `sink` reports that nobody is listening.
    fn poll(
        &mut self,
        mode: ScanMode,
        sink: &mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>,
    ) -> Result<ProviderPoll, SinkClosed>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_display_includes_the_qualifier_only_when_present() {
        assert_eq!(DeviceLabel::new("Headset").display(), "Headset");
        assert_eq!(
            DeviceLabel::new("Mouse").with_qualifier("slot 2").display(),
            "Mouse (slot 2)"
        );
    }

    #[test]
    fn device_ids_from_different_sources_do_not_collide() {
        assert_ne!(DeviceId::new("hid:a"), DeviceId::new("bluetooth:a"));
        assert_eq!(DeviceId::new("hid:a").to_string(), "hid:a");
    }
}
