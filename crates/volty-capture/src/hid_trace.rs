//! Separate, explicitly opted-in raw HID report traces.
//!
//! These traces can contain button or keyboard input and are intentionally not
//! part of the privacy-filtered battery snapshot schema.

use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};

use crate::{from_hex, to_hex, CaptureError};

pub const HID_TRACE_SCHEMA: &str = "volty.hid_trace/v1";
pub const MAX_HID_TRACE_REPORT_BYTES: usize = 4096;
pub const MAX_HID_TRACE_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_HID_TRACE_REPORTS: u64 = 100_000;
const MAX_HID_TRACE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// One complete HID interface description, without paths or serial numbers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidTraceInterface {
    pub bus: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub release_number: u16,
    pub usage_page: Option<u16>,
    pub usage: Option<u16>,
    pub interface_number: i32,
}

/// Metadata written before the first captured report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidTraceHeader {
    pub schema: String,
    pub trace_id: String,
    pub created_at: String,
    pub app_version: String,
    pub interface: HidTraceInterface,
}

/// Report operation observed by Volty's HID transport.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HidTraceDirection {
    Input,
    Output,
    FeatureRead,
    FeatureWrite,
}

/// One raw HID report. `data` is lowercase hex and includes the report ID byte.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidTraceReport {
    pub t_ms: u64,
    pub direction: HidTraceDirection,
    pub report_id: u8,
    pub data: String,
}

/// End-of-file counts and whether capture ended normally.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HidTraceFooter {
    pub schema: String,
    /// Number of records before the footer, including the header.
    pub records: u64,
    pub reports: u64,
    pub bytes: u64,
    pub complete: bool,
}

/// One JSONL record in a raw HID trace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HidTraceRecord {
    Header(HidTraceHeader),
    Report(HidTraceReport),
    Footer(HidTraceFooter),
}

/// Append-only writer for bounded raw HID traces.
pub struct HidTraceWriter<W: Write> {
    writer: BufWriter<W>,
    reports: u64,
    total_bytes: u64,
    last_t_ms: Option<u64>,
}

impl<W: Write> HidTraceWriter<W> {
    /// Starts a trace with its required header.
    pub fn new(writer: W, header: HidTraceHeader) -> Result<Self, CaptureError> {
        if header.schema != HID_TRACE_SCHEMA {
            return Err(CaptureError::new(format!(
                "HID trace schema {:?} does not match {HID_TRACE_SCHEMA}",
                header.schema
            )));
        }

        let mut trace = Self {
            writer: BufWriter::new(writer),
            reports: 0,
            total_bytes: 0,
            last_t_ms: None,
        };
        trace.write_record(&HidTraceRecord::Header(header))?;
        Ok(trace)
    }

    /// Appends one report after validating its bounded payload and timestamp.
    pub fn write_report(&mut self, report: &HidTraceReport) -> Result<(), CaptureError> {
        let bytes = decode_report(report)?;
        if self.last_t_ms.is_some_and(|last| report.t_ms < last) {
            return Err(CaptureError::new(
                "HID trace report timestamps must be nondecreasing",
            ));
        }
        if self.reports >= MAX_HID_TRACE_REPORTS {
            return Err(CaptureError::new(format!(
                "HID trace exceeds the {MAX_HID_TRACE_REPORTS} report limit"
            )));
        }
        let next_total = self
            .total_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| CaptureError::new("HID trace byte count overflowed"))?;
        if next_total > MAX_HID_TRACE_TOTAL_BYTES {
            return Err(CaptureError::new(format!(
                "HID trace exceeds the {} byte payload limit",
                MAX_HID_TRACE_TOTAL_BYTES
            )));
        }

        self.write_record(&HidTraceRecord::Report(report.clone()))?;
        self.reports += 1;
        self.total_bytes = next_total;
        self.last_t_ms = Some(report.t_ms);
        Ok(())
    }

    /// Writes a footer and flushes the trace.
    pub fn finish(mut self, complete: bool) -> Result<(), CaptureError> {
        self.write_record(&HidTraceRecord::Footer(HidTraceFooter {
            schema: HID_TRACE_SCHEMA.to_owned(),
            records: self.reports + 1,
            reports: self.reports,
            bytes: self.total_bytes,
            complete,
        }))?;
        self.writer
            .flush()
            .map_err(|error| CaptureError::new(format!("failed to flush HID trace: {error}")))
    }

    fn write_record(&mut self, record: &HidTraceRecord) -> Result<(), CaptureError> {
        serde_json::to_writer(&mut self.writer, record)
            .map_err(|error| CaptureError::new(format!("failed to encode HID trace: {error}")))?;
        self.writer
            .write_all(b"\n")
            .map_err(|error| CaptureError::new(format!("failed to write HID trace: {error}")))
    }
}

/// A parsed trace suitable for protocol analysis and decoder tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HidTraceFile {
    pub header: HidTraceHeader,
    pub reports: Vec<HidTraceReport>,
    pub footer: HidTraceFooter,
}

impl HidTraceFile {
    /// Reads and validates a bounded HID trace.
    pub fn read(path: impl AsRef<Path>) -> Result<Self, CaptureError> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|error| {
            CaptureError::new(format!(
                "failed to read HID trace {}: {error}",
                path.display()
            ))
        })?;
        let length = file
            .metadata()
            .map_err(|error| CaptureError::new(format!("could not inspect HID trace: {error}")))?
            .len();
        if length > MAX_HID_TRACE_FILE_BYTES {
            return Err(CaptureError::new(format!(
                "HID trace file exceeds the {MAX_HID_TRACE_FILE_BYTES} byte limit"
            )));
        }
        Self::from_reader(BufReader::new(file))
    }

    fn from_reader(reader: impl BufRead) -> Result<Self, CaptureError> {
        let mut header = None;
        let mut reports = Vec::new();
        let mut footer = None;
        let mut total_bytes = 0u64;
        let mut last_t_ms = None;

        for (index, line) in reader.lines().enumerate() {
            let line_number = index + 1;
            let line = line.map_err(|error| {
                CaptureError::new(format!(
                    "could not read HID trace line {line_number}: {error}"
                ))
            })?;
            if line.trim().is_empty() {
                return Err(CaptureError::new(format!(
                    "line {line_number}: empty HID trace record"
                )));
            }
            let record = serde_json::from_str::<HidTraceRecord>(&line).map_err(|error| {
                CaptureError::new(format!(
                    "line {line_number}: invalid HID trace record: {error}"
                ))
            })?;
            match record {
                HidTraceRecord::Header(record) => {
                    if line_number != 1 || header.replace(record).is_some() {
                        return Err(CaptureError::new(
                            "HID trace must contain one header as its first record",
                        ));
                    }
                }
                HidTraceRecord::Report(report) => {
                    if header.is_none() || footer.is_some() {
                        return Err(CaptureError::new(format!(
                            "line {line_number}: HID report outside the header/footer"
                        )));
                    }
                    let bytes = decode_report(&report)?;
                    if last_t_ms.is_some_and(|last| report.t_ms < last) {
                        return Err(CaptureError::new(format!(
                            "line {line_number}: HID trace timestamps must be nondecreasing"
                        )));
                    }
                    if reports.len() as u64 >= MAX_HID_TRACE_REPORTS {
                        return Err(CaptureError::new(format!(
                            "HID trace exceeds the {MAX_HID_TRACE_REPORTS} report limit"
                        )));
                    }
                    total_bytes = total_bytes
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(|| CaptureError::new("HID trace byte count overflowed"))?;
                    if total_bytes > MAX_HID_TRACE_TOTAL_BYTES {
                        return Err(CaptureError::new(format!(
                            "HID trace exceeds the {} byte payload limit",
                            MAX_HID_TRACE_TOTAL_BYTES
                        )));
                    }
                    last_t_ms = Some(report.t_ms);
                    reports.push(report);
                }
                HidTraceRecord::Footer(record) => {
                    if header.is_none() || footer.replace(record).is_some() {
                        return Err(CaptureError::new(
                            "HID trace must contain one footer after its header",
                        ));
                    }
                }
            }
        }

        let header = header.ok_or_else(|| CaptureError::new("HID trace is missing its header"))?;
        let footer = footer.ok_or_else(|| CaptureError::new("HID trace is missing its footer"))?;
        let file = Self {
            header,
            reports,
            footer,
        };
        file.validate(total_bytes)?;
        Ok(file)
    }

    fn validate(&self, total_bytes: u64) -> Result<(), CaptureError> {
        if self.header.schema != HID_TRACE_SCHEMA {
            return Err(CaptureError::new(format!(
                "HID trace header schema {:?} does not match {HID_TRACE_SCHEMA}",
                self.header.schema
            )));
        }
        if self.footer.schema != HID_TRACE_SCHEMA {
            return Err(CaptureError::new(format!(
                "HID trace footer schema {:?} does not match {HID_TRACE_SCHEMA}",
                self.footer.schema
            )));
        }
        let report_count = u64::try_from(self.reports.len())
            .map_err(|_| CaptureError::new("HID trace report count overflowed"))?;
        if self.footer.reports != report_count || self.footer.records != report_count + 1 {
            return Err(CaptureError::new(format!(
                "HID trace footer counts do not match {report_count} reports"
            )));
        }
        if self.footer.bytes != total_bytes {
            return Err(CaptureError::new(format!(
                "HID trace footer byte count {} does not match {total_bytes}",
                self.footer.bytes
            )));
        }
        Ok(())
    }
}

fn decode_report(report: &HidTraceReport) -> Result<Vec<u8>, CaptureError> {
    if report.data.len() > MAX_HID_TRACE_REPORT_BYTES * 2 {
        return Err(CaptureError::new(format!(
            "HID report exceeds the {MAX_HID_TRACE_REPORT_BYTES} byte limit"
        )));
    }
    let bytes = from_hex(&report.data)?;
    if bytes.is_empty() {
        return Err(CaptureError::new(
            "HID reports must contain a report ID byte",
        ));
    }
    if bytes.len() > MAX_HID_TRACE_REPORT_BYTES {
        return Err(CaptureError::new(format!(
            "HID report exceeds the {MAX_HID_TRACE_REPORT_BYTES} byte limit"
        )));
    }
    if bytes[0] != report.report_id {
        return Err(CaptureError::new(format!(
            "HID report ID {} does not match its first data byte {}",
            report.report_id, bytes[0]
        )));
    }
    if report.data != to_hex(&bytes) {
        return Err(CaptureError::new(
            "HID report data must use lowercase hexadecimal",
        ));
    }
    Ok(bytes)
}

/// Builds a trace report from a raw HID I/O observation.
pub fn hid_trace_report(
    t_ms: u64,
    direction: HidTraceDirection,
    bytes: &[u8],
) -> Result<HidTraceReport, CaptureError> {
    let report_id = bytes
        .first()
        .copied()
        .ok_or_else(|| CaptureError::new("HID reports must contain a report ID byte"))?;
    if bytes.len() > MAX_HID_TRACE_REPORT_BYTES {
        return Err(CaptureError::new(format!(
            "HID report exceeds the {MAX_HID_TRACE_REPORT_BYTES} byte limit"
        )));
    }
    Ok(HidTraceReport {
        t_ms,
        direction,
        report_id,
        data: to_hex(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header() -> HidTraceHeader {
        HidTraceHeader {
            schema: HID_TRACE_SCHEMA.to_owned(),
            trace_id: "trace-1".to_owned(),
            created_at: "2026-11-01T10:00:00Z".to_owned(),
            app_version: "0.5.0".to_owned(),
            interface: HidTraceInterface {
                bus: "Usb".to_owned(),
                vendor_id: 0x1234,
                product_id: 0x5678,
                release_number: 1,
                usage_page: Some(0xff00),
                usage: Some(1),
                interface_number: 2,
            },
        }
    }

    #[test]
    fn trace_writer_and_reader_round_trip_raw_reports() {
        let mut bytes = Vec::new();
        let mut writer = HidTraceWriter::new(&mut bytes, header()).unwrap();
        let report = hid_trace_report(17, HidTraceDirection::Input, &[2, 3, 4]).unwrap();
        writer.write_report(&report).unwrap();
        writer.finish(true).unwrap();

        let serialized = String::from_utf8(bytes.clone()).unwrap();
        assert!(serialized.contains("\"schema\":\"volty.hid_trace/v1\""));
        assert!(!serialized.contains("path"));
        assert!(!serialized.contains("serial"));

        let trace = HidTraceFile::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(trace.header, header());
        assert_eq!(trace.reports, [report]);
        assert_eq!(trace.footer.reports, 1);
        assert_eq!(trace.footer.bytes, 3);
        assert!(trace.footer.complete);
    }

    #[test]
    fn writer_rejects_mismatched_ids_nonmonotonic_time_and_oversized_reports() {
        let mut bytes = Vec::new();
        let mut writer = HidTraceWriter::new(&mut bytes, header()).unwrap();
        let mismatched = HidTraceReport {
            t_ms: 10,
            direction: HidTraceDirection::Input,
            report_id: 1,
            data: "0001".to_owned(),
        };
        assert!(writer.write_report(&mismatched).is_err());

        writer
            .write_report(&hid_trace_report(10, HidTraceDirection::Input, &[0, 1]).unwrap())
            .unwrap();
        assert!(writer
            .write_report(&hid_trace_report(9, HidTraceDirection::Input, &[0, 2]).unwrap())
            .is_err());
        let oversized = HidTraceReport {
            t_ms: 11,
            direction: HidTraceDirection::Input,
            report_id: 0,
            data: to_hex(&vec![0; MAX_HID_TRACE_REPORT_BYTES + 1]),
        };
        assert!(writer.write_report(&oversized).is_err());
    }

    #[test]
    fn reader_rejects_footer_count_mismatches() {
        let text = concat!(
            "{\"type\":\"header\",\"schema\":\"volty.hid_trace/v1\",\"trace_id\":\"trace-1\",\"created_at\":\"2026-11-01T10:00:00Z\",\"app_version\":\"0.5.0\",\"interface\":{\"bus\":\"Usb\",\"vendor_id\":4660,\"product_id\":22136,\"release_number\":1,\"usage_page\":65280,\"usage\":1,\"interface_number\":2}}\n",
            "{\"type\":\"footer\",\"schema\":\"volty.hid_trace/v1\",\"records\":2,\"reports\":1,\"bytes\":0,\"complete\":true}\n"
        );
        assert!(HidTraceFile::from_reader(Cursor::new(text)).is_err());
    }
}
