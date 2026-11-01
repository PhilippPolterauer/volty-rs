//! Versioned, append-only JSONL evidence logs for Volty hardware captures.
//!
//! Battery captures use [`CaptureHeader`] and exclude device paths and serial
//! number values. Opt-in raw HID traces use a separate schema because report
//! bytes can contain user input.

mod error;
mod hid_trace;
mod reader;
mod redact;
mod schema;
mod summary;
mod writer;

#[cfg(test)]
mod tests;

pub use error::CaptureError;
pub use hid_trace::{
    hid_trace_report, HidTraceDirection, HidTraceFile, HidTraceFooter, HidTraceHeader,
    HidTraceInterface, HidTraceRecord, HidTraceReport, HidTraceWriter, HID_TRACE_SCHEMA,
    MAX_HID_TRACE_REPORTS, MAX_HID_TRACE_REPORT_BYTES, MAX_HID_TRACE_TOTAL_BYTES,
};
pub use reader::{CaptureFile, CaptureReader};
pub use redact::Redactor;
pub use schema::{
    from_hex, to_hex, BatterySnapshot, CaptureBluetoothDevice, CaptureBluetoothGatt,
    CaptureBluetoothKind, CaptureBluetoothProperty, CaptureEvent, CaptureFooter, CaptureHeader,
    CaptureHidLayout, CaptureHidSample, CaptureHidppExchange, CaptureHidppFeatures,
    CaptureInterface, CaptureMarker, CaptureMatch, CaptureMode, CaptureReading, CaptureRecord,
    CapturedBatteryPercentage, CapturedPercentageEstimateMethod, EventLevel, HidButtonCapability,
    HidButtonSample, HidFieldRef, HidRatioRef, HidReportKind, HidSelectedLayout,
    HidValueCapability, HidValueSample, HidppFeature, MatchOutcome, PrivacyProfile, SCHEMA,
};
pub use writer::CaptureWriter;
