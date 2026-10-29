use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::*;

struct TempPath(PathBuf);

impl TempPath {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "volty-capture-{}-{tag}-{id}.jsonl",
            std::process::id()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn sample_header() -> CaptureHeader {
    CaptureHeader {
        schema: SCHEMA.to_owned(),
        capture_id: "capture-1".to_owned(),
        created_at: "2026-10-29T12:00:00Z".to_owned(),
        app_version: "0.2.1".to_owned(),
        app_commit: Some("c50ca42".to_owned()),
        os: "Windows 11".to_owned(),
        architecture: "x86_64".to_owned(),
        mode: CaptureMode::Snapshot,
        privacy: PrivacyProfile::Strict,
        features: vec!["hid".to_owned(), "hidpp".to_owned()],
    }
}

fn sample_battery() -> BatterySnapshot {
    BatterySnapshot {
        percent: Some(85),
        level: "good".to_owned(),
        state: "discharging".to_owned(),
        raw_level: 0x55,
        raw_status: Some(0x03),
        source: "HID++ Unified Battery".to_owned(),
    }
}

fn sample_field_ref() -> HidFieldRef {
    HidFieldRef {
        page: 0xff00,
        usage: 0x0001,
        report_id: 3,
        kind: HidReportKind::Feature,
    }
}

fn sample_records() -> Vec<CaptureRecord> {
    vec![
        CaptureRecord::Interface(CaptureInterface {
            id: "iface-1".to_owned(),
            bus: "hid".to_owned(),
            vendor_id: 0x046d,
            product_id: 0xc547,
            release_number: 0x0121,
            usage_page: Some(0xff00),
            usage: Some(0x0001),
            interface_number: 1,
            manufacturer: Some("Logitech".to_owned()),
            product: Some("G502 X Plus".to_owned()),
            serial_present: true,
        }),
        CaptureRecord::Match(CaptureMatch {
            id: "match-1".to_owned(),
            device_index: Some(1),
            backend: Some("logitech-hidpp".to_owned()),
            outcome: MatchOutcome::Matched,
            detail: Some("vendor and product id matched".to_owned()),
        }),
        CaptureRecord::HidppFeatures(CaptureHidppFeatures {
            id: "features-1".to_owned(),
            device_index: 1,
            features: vec![
                HidppFeature {
                    logical_id: 0x1004,
                    runtime_index: 0,
                    feature_type: 0x04,
                    version: 4,
                },
                HidppFeature {
                    logical_id: 0x1000,
                    runtime_index: 1,
                    feature_type: 0x00,
                    version: 0,
                },
            ],
        }),
        CaptureRecord::HidLayout(CaptureHidLayout {
            id: "layout-1".to_owned(),
            device_index: Some(1),
            values: vec![HidValueCapability {
                page: 0x0006,
                usage: 0x0020,
                report_id: 1,
                kind: HidReportKind::Input,
                logical_min: 0,
                logical_max: 100,
            }],
            buttons: vec![HidButtonCapability {
                page: 0x0009,
                usage: 0x0001,
                report_id: 2,
                kind: HidReportKind::Input,
            }],
            selected: Some(HidSelectedLayout {
                percent: Some(sample_field_ref()),
                ratio: Some(HidRatioRef {
                    remaining: sample_field_ref(),
                    full: HidFieldRef {
                        usage: 0x0002,
                        ..sample_field_ref()
                    },
                }),
                charging: Some(HidFieldRef {
                    usage: 0x0044,
                    ..sample_field_ref()
                }),
                discharging: None,
            }),
        }),
        CaptureRecord::HidppExchange(CaptureHidppExchange {
            id: "exchange-1".to_owned(),
            device_index: 1,
            t_ms: 500,
            request: to_hex(&[0x11, 0xff, 0x00, 0x0a, 0x00, 0x00, 0x00]),
            response: Some(to_hex(&[0x11, 0xff, 0x00, 0x0a, 0x00, 0x55, 0x00])),
            error: None,
            elapsed_ms: 4,
        }),
        CaptureRecord::HidSample(CaptureHidSample {
            id: "sample-1".to_owned(),
            device_index: Some(1),
            t_ms: 900,
            values: vec![HidValueSample {
                page: 0x0006,
                usage: 0x0020,
                report_id: 1,
                kind: HidReportKind::Input,
                raw_value: 85,
            }],
            battery: sample_battery(),
        }),
        CaptureRecord::Reading(CaptureReading {
            id: "reading-1".to_owned(),
            device_index: 1,
            t_ms: 1_000,
            backend: "logitech-hidpp".to_owned(),
            label: Some("G502 X Plus".to_owned()),
            battery: sample_battery(),
        }),
        CaptureRecord::Event(CaptureEvent {
            t_ms: 1_100,
            id: Some("event-1".to_owned()),
            level: EventLevel::Warning,
            message: "battery feature returned a partial response".to_owned(),
        }),
        CaptureRecord::Marker(CaptureMarker {
            t_ms: 1_200,
            label: "unplugged".to_owned(),
        }),
    ]
}

fn sample_footer(records: u64, complete: bool) -> CaptureFooter {
    CaptureFooter {
        schema: SCHEMA.to_owned(),
        records,
        complete,
    }
}

fn write_records(path: &Path, records: &[CaptureRecord]) {
    let mut text = String::new();
    for record in records {
        text.push_str(&serde_json::to_string(record).unwrap());
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn write_text(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
}

#[test]
fn every_record_variant_round_trips_through_writer_and_reader() {
    let path = TempPath::new("roundtrip");
    let header = sample_header();
    let records = sample_records();
    let mut writer = CaptureWriter::create(path.path()).unwrap();
    writer
        .write(&CaptureRecord::Header(header.clone()))
        .unwrap();
    for record in &records {
        writer.write(record).unwrap();
    }
    writer.finish(true).unwrap();

    let file = CaptureFile::read(path.path()).unwrap();
    assert_eq!(file.header, header);
    assert_eq!(file.records, records);
    assert_eq!(
        file.footer,
        sample_footer(u64::try_from(records.len()).unwrap() + 1, true)
    );
    file.validate().unwrap();
}

#[test]
fn footer_record_count_mismatch_is_rejected() {
    let path = TempPath::new("bad-count");
    write_records(
        path.path(),
        &[
            CaptureRecord::Header(sample_header()),
            CaptureRecord::Marker(CaptureMarker {
                t_ms: 1,
                label: "only marker".to_owned(),
            }),
            CaptureRecord::Footer(sample_footer(1, true)),
        ],
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(
        error.to_string().contains("footer records count"),
        "{error}"
    );
}

#[test]
fn missing_header_is_rejected() {
    let path = TempPath::new("missing-header");
    write_records(
        path.path(),
        &[
            CaptureRecord::Marker(CaptureMarker {
                t_ms: 1,
                label: "no header".to_owned(),
            }),
            CaptureRecord::Footer(sample_footer(1, true)),
        ],
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("start with a header"), "{error}");
}

#[test]
fn duplicate_header_is_rejected() {
    let path = TempPath::new("duplicate-header");
    write_records(
        path.path(),
        &[
            CaptureRecord::Header(sample_header()),
            CaptureRecord::Header(sample_header()),
            CaptureRecord::Footer(sample_footer(2, true)),
        ],
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(
        error.to_string().contains("more than one header"),
        "{error}"
    );
}

#[test]
fn missing_footer_is_rejected() {
    let path = TempPath::new("missing-footer");
    write_records(path.path(), &[CaptureRecord::Header(sample_header())]);

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("missing a footer"), "{error}");
}

#[test]
fn duplicate_footer_is_rejected() {
    let path = TempPath::new("duplicate-footer");
    write_records(
        path.path(),
        &[
            CaptureRecord::Header(sample_header()),
            CaptureRecord::Footer(sample_footer(1, true)),
            CaptureRecord::Footer(sample_footer(2, true)),
        ],
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(
        error.to_string().contains("more than one footer"),
        "{error}"
    );
}

#[test]
fn record_after_footer_is_rejected() {
    let path = TempPath::new("after-footer");
    write_records(
        path.path(),
        &[
            CaptureRecord::Header(sample_header()),
            CaptureRecord::Footer(sample_footer(1, true)),
            CaptureRecord::Marker(CaptureMarker {
                t_ms: 2,
                label: "too late".to_owned(),
            }),
        ],
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("end with a footer"), "{error}");
}

#[test]
fn wrong_schema_is_rejected() {
    let path = TempPath::new("wrong-schema");
    let mut header = sample_header();
    header.schema = "volty.capture/v2".to_owned();
    write_records(
        path.path(),
        &[
            CaptureRecord::Header(header),
            CaptureRecord::Footer(CaptureFooter {
                schema: "volty.capture/v2".to_owned(),
                records: 1,
                complete: true,
            }),
        ],
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("schema"), "{error}");
}

#[test]
fn malformed_json_reports_the_line_number() {
    let path = TempPath::new("malformed");
    let header = serde_json::to_string(&CaptureRecord::Header(sample_header())).unwrap();
    let footer = serde_json::to_string(&CaptureRecord::Footer(sample_footer(1, false))).unwrap();
    write_text(
        path.path(),
        &format!("{header}\n{{\"type\":\"marker\",\"t_ms\":1,\"label\":\n{footer}\n"),
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("line 2"), "{error}");
}

#[test]
fn unknown_record_type_is_rejected() {
    let path = TempPath::new("unknown-type");
    let header = serde_json::to_string(&CaptureRecord::Header(sample_header())).unwrap();
    let footer = serde_json::to_string(&CaptureRecord::Footer(sample_footer(1, false))).unwrap();
    write_text(
        path.path(),
        &format!(
            "{header}\n{}\n{footer}\n",
            serde_json::json!({ "type": "bogus", "t_ms": 1 })
        ),
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("unknown variant"), "{error}");
    assert!(error.to_string().contains("line 2"), "{error}");
}

#[test]
fn unknown_fields_are_rejected() {
    let path = TempPath::new("unknown-field");
    let header = serde_json::to_string(&CaptureRecord::Header(sample_header())).unwrap();
    let footer = serde_json::to_string(&CaptureRecord::Footer(sample_footer(2, false))).unwrap();
    write_text(
        path.path(),
        &format!(
            "{header}\n{}\n{footer}\n",
            serde_json::json!({
                "type": "marker",
                "t_ms": 1,
                "label": "checkpoint",
                "device_path": "\\\\?\\HID#VID_046D"
            })
        ),
    );

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
    assert!(error.to_string().contains("device_path"), "{error}");
}

#[test]
fn empty_file_is_rejected() {
    let path = TempPath::new("empty");
    write_text(path.path(), "");

    let error = CaptureFile::read(path.path()).unwrap_err();
    assert!(error.to_string().contains("empty"), "{error}");
}

#[test]
fn hex_round_trips_and_rejects_invalid_input() {
    let bytes = [0x00, 0x1a, 0x7f, 0xff];
    assert_eq!(to_hex(&bytes), "001a7fff");
    assert_eq!(from_hex("001a7fff").unwrap(), bytes);
    assert_eq!(from_hex("001A7FFF").unwrap(), bytes);
    assert!(from_hex("").unwrap().is_empty());

    for invalid in ["0", "abc", "zz", "0x00", "12 34", "abg0"] {
        assert!(from_hex(invalid).is_err(), "{invalid:?} should not decode");
    }
}

#[test]
fn standard_and_strict_redaction_differ_for_model_strings_only() {
    let standard = Redactor::new(PrivacyProfile::Standard);
    assert_eq!(
        standard.product(Some("G502 X Plus")),
        Some("G502 X Plus".to_owned())
    );
    assert_eq!(
        standard.manufacturer(Some("Logitech")),
        Some("Logitech".to_owned())
    );
    assert_eq!(standard.device_name(Some("Desk mouse")), None);
    assert_eq!(standard.product(None), None);

    let strict = Redactor::new(PrivacyProfile::Strict);
    assert_eq!(strict.product(Some("G502 X Plus")), None);
    assert_eq!(strict.manufacturer(Some("Logitech")), None);
    assert_eq!(strict.device_name(Some("Desk mouse")), None);
    assert_eq!(strict.profile(), PrivacyProfile::Strict);
}

#[test]
fn render_summary_includes_counts_and_sensitive_values_are_unrepresentable() {
    let records = sample_records();
    let file = CaptureFile {
        header: sample_header(),
        footer: sample_footer(u64::try_from(records.len()).unwrap() + 1, true),
        records,
    };
    file.validate().unwrap();

    let summary = file.render_summary();
    assert!(summary.contains("schema: volty.capture/v1"));
    assert!(summary.contains("app_version: 0.2.1"));
    assert!(summary.contains("os: Windows 11"));
    assert!(summary.contains("mode: snapshot"));
    assert!(summary.contains("privacy: strict"));
    assert!(summary.contains("interfaces: 1"));
    assert!(summary.contains("matches: 1"));
    assert!(summary.contains("readings: 1"));
    assert!(summary.contains("events: 1"));
    assert!(summary.contains("iface-1"));
    assert!(summary.contains("reading-1"));
    assert!(!summary.contains("SERIAL-SECRET"));
    assert!(!summary.contains("C:\\"));
    assert!(!summary.contains("\\\\?\\"));
    assert!(summary.lines().count() < 100, "{summary}");
}

#[test]
fn writer_requires_a_header_and_owns_the_footer() {
    let path = TempPath::new("writer-order");
    let mut writer = CaptureWriter::create(path.path()).unwrap();

    let error = writer
        .write(&CaptureRecord::Marker(CaptureMarker {
            t_ms: 1,
            label: "before header".to_owned(),
        }))
        .unwrap_err();
    assert!(error.to_string().contains("before the header"), "{error}");

    writer
        .write(&CaptureRecord::Header(sample_header()))
        .unwrap();
    let error = writer
        .write(&CaptureRecord::Header(sample_header()))
        .unwrap_err();
    assert!(
        error.to_string().contains("already has a header"),
        "{error}"
    );

    let error = writer
        .write(&CaptureRecord::Footer(sample_footer(1, true)))
        .unwrap_err();
    assert!(error.to_string().contains("finish"), "{error}");

    writer.finish(false).unwrap();
    let file = CaptureFile::read(path.path()).unwrap();
    assert!(!file.footer.complete);
    assert!(file.records.is_empty());
}

#[test]
fn writer_cannot_finish_without_a_header() {
    let path = TempPath::new("no-header-finish");
    let writer = CaptureWriter::create(path.path()).unwrap();

    let error = writer.finish(true).unwrap_err();
    assert!(error.to_string().contains("before a header"), "{error}");
}

#[test]
fn validate_rejects_footer_count_mismatch() {
    let file = CaptureFile {
        header: sample_header(),
        records: vec![CaptureRecord::Marker(CaptureMarker {
            t_ms: 1,
            label: "marker".to_owned(),
        })],
        footer: sample_footer(1, true),
    };

    let error = file.validate().unwrap_err();
    assert!(
        error.to_string().contains("footer records count"),
        "{error}"
    );
}

#[test]
fn validate_rejects_wrong_footer_schema() {
    let file = CaptureFile {
        header: sample_header(),
        records: Vec::new(),
        footer: CaptureFooter {
            schema: "volty.capture/v2".to_owned(),
            records: 1,
            complete: true,
        },
    };

    let error = file.validate().unwrap_err();
    assert!(error.to_string().contains("footer schema"), "{error}");
}

#[test]
fn validate_rejects_header_or_footer_inside_records() {
    let mut file = CaptureFile {
        header: sample_header(),
        records: vec![CaptureRecord::Marker(CaptureMarker {
            t_ms: 1,
            label: "marker".to_owned(),
        })],
        footer: sample_footer(2, true),
    };
    file.records.push(CaptureRecord::Header(sample_header()));

    let error = file.validate().unwrap_err();
    assert!(error.to_string().contains("header"), "{error}");
}
