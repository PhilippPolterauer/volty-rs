use std::{error::Error, fmt};

/// Failure while writing, reading, or validating a capture file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureError {
    message: String,
}

impl CaptureError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The human-readable failure description.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CaptureError {}
