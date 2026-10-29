//! Versioned, append-only JSONL evidence log for Volty hardware captures.
//!
//! A capture starts with a [`CaptureHeader`], contains zero or more data
//! records, and ends with a [`CaptureFooter`]. Device paths and serial number
//! values are not representable in the schema.

mod error;
mod reader;
mod redact;
mod schema;
mod summary;
mod writer;

#[cfg(test)]
mod tests;

pub use error::CaptureError;
pub use reader::{CaptureFile, CaptureReader};
pub use redact::Redactor;
pub use schema::{
    from_hex, to_hex, BatterySnapshot, CaptureEvent, CaptureFooter, CaptureHeader,
    CaptureHidLayout, CaptureHidSample, CaptureHidppExchange, CaptureHidppFeatures,
    CaptureInterface, CaptureMarker, CaptureMatch, CaptureMode, CaptureReading, CaptureRecord,
    EventLevel, HidButtonCapability, HidFieldRef, HidRatioRef, HidReportKind, HidSelectedLayout,
    HidValueCapability, HidValueSample, HidppFeature, MatchOutcome, PrivacyProfile, SCHEMA,
};
pub use writer::CaptureWriter;
