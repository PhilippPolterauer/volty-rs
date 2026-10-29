//! Offline replay of captured HID++ exchanges and generic HID samples.
//!
//! Replay never opens a device. It decodes recorded frames with the same
//! protocol parsers the tray uses and checks that the normalized readings in the
//! capture can be reproduced from the recorded evidence.

use std::{collections::HashMap, error::Error, fmt::Write as _, io, path::Path};

use volty_bluetooth::{
    decode_battery_level, decode_windows_battery_life, SOURCE_DEVICE_PROPERTY, SOURCE_GATT,
};
use volty_capture::{
    from_hex, CaptureBluetoothGatt, CaptureBluetoothProperty, CaptureFile, CaptureHidLayout,
    CaptureHidppExchange, CaptureHidppFeatures, CaptureRecord, HidReportKind,
};
use volty_hid::{
    decode_generic_battery, parse_legacy_battery_status, parse_unified_battery,
    parse_unified_battery_capabilities, BatteryProtocol, BatteryStatus, GenericBatteryButton,
    GenericBatteryButtonReading, GenericBatteryLayout, GenericBatteryValue,
    GenericBatteryValueReading, GenericReportKind, HidppFrame, UnifiedBatteryCapabilities,
    FEATURE_BATTERY_STATUS, FEATURE_ROOT, FEATURE_SET, FEATURE_UNIFIED_BATTERY,
    FUNCTION_GET_BATTERY_STATUS, FUNCTION_GET_FEATURE, FUNCTION_GET_FEATURE_SET_COUNT,
    FUNCTION_GET_FEATURE_SET_ID, FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES,
    FUNCTION_GET_UNIFIED_BATTERY_INFO, GENERIC_HID_BATTERY, LOGITECH_HIDPP_20,
};

use crate::capture::battery_snapshot;

/// Counts produced by an offline replay.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReplayReport {
    pub exchanges: usize,
    pub transport_errors: usize,
    pub features: usize,
    pub readings: usize,
    pub layouts: usize,
    pub samples: usize,
    pub bluetooth_evidence: usize,
    pub bluetooth_readings: usize,
}

impl ReplayReport {
    /// Renders a short, privacy-safe replay result.
    pub fn render(&self) -> String {
        let mut text = format!(
            "replay: {} exchanges ({} transport errors)\nfeatures: {} verified\nreadings: {} matched\n",
            self.exchanges, self.transport_errors, self.features, self.readings
        );
        let _ = writeln!(
            text,
            "generic layouts: {} checked, {} samples matched",
            self.layouts, self.samples
        );
        let _ = writeln!(
            text,
            "bluetooth: {} evidence records, {} readings matched",
            self.bluetooth_evidence, self.bluetooth_readings
        );
        text
    }
}

/// Runs `volty capture replay <path>` without touching hardware.
pub fn replay_cli(path: &Path) -> Result<(), Box<dyn Error>> {
    let file = CaptureFile::read(path)?;
    let report = replay_capture(&file)?;
    println!("Replayed hardware capture: {}", path.display());
    print!("{}", report.render());
    Ok(())
}

/// Replays one validated capture file.
///
/// # Errors
/// Returns an error when a frame is malformed, a request is outside the
/// read-only allowlist, the recorded feature table does not match the Root
/// responses, or a normalized reading cannot be reproduced from the evidence.
pub fn replay_capture(file: &CaptureFile) -> Result<ReplayReport, Box<dyn Error>> {
    let mut report = ReplayReport::default();
    let mut features = FeatureContext::default();
    features.collect(file);

    let mut decoded = Vec::new();
    let mut observed = HashMap::new();
    let mut unified_capabilities = HashMap::new();

    for (index, record) in file.records.iter().enumerate() {
        let CaptureRecord::HidppExchange(exchange) = record else {
            continue;
        };
        let context = format!(
            "exchange {} ({}, device {})",
            index + 1,
            exchange.id,
            exchange.device_index
        );
        replay_exchange(
            exchange,
            &features,
            &mut observed,
            &mut unified_capabilities,
            &mut decoded,
            &mut report,
        )
        .map_err(|error| io::Error::other(format!("{context}: {error}")))?;
    }

    features.verify(file, &observed, &mut report)?;
    verify_hidpp_readings(file, &decoded, &mut report)?;
    replay_generic(file, &mut report)?;
    replay_bluetooth(file, &mut report)?;
    Ok(report)
}

#[derive(Default)]
struct FeatureContext {
    /// Runtime index to logical ID for each capture interface and device.
    runtime: HashMap<(String, u8, u8), u16>,
    /// Declared feature tables to verify against Root responses.
    declared: HashMap<(String, u8), CaptureHidppFeatures>,
}

impl FeatureContext {
    fn collect(&mut self, file: &CaptureFile) {
        for record in &file.records {
            let CaptureRecord::HidppFeatures(table) = record else {
                continue;
            };
            self.declared
                .insert((table.id.clone(), table.device_index), table.clone());
            for feature in &table.features {
                self.runtime.insert(
                    (table.id.clone(), table.device_index, feature.runtime_index),
                    feature.logical_id,
                );
            }
        }
    }

    fn logical(
        &self,
        observed: &HashMap<(String, u8, u16), (u8, u8, u8)>,
        id: &str,
        device_index: u8,
        runtime_index: u8,
    ) -> Option<u16> {
        if runtime_index == 0 {
            return Some(FEATURE_ROOT);
        }
        self.runtime
            .get(&(id.to_owned(), device_index, runtime_index))
            .copied()
            .or_else(|| {
                // Partial captures may contain Root responses without a
                // completed feature table. Resolve those from the evidence.
                observed.iter().find_map(
                    |((observed_id, observed_device, logical), (runtime, _, _))| {
                        (*observed_id == id
                            && *observed_device == device_index
                            && *runtime == runtime_index)
                            .then_some(*logical)
                    },
                )
            })
    }

    fn verify(
        &self,
        file: &CaptureFile,
        observed: &HashMap<(String, u8, u16), (u8, u8, u8)>,
        report: &mut ReplayReport,
    ) -> Result<(), Box<dyn Error>> {
        for record in &file.records {
            let CaptureRecord::HidppFeatures(table) = record else {
                continue;
            };
            for feature in &table.features {
                let key = (table.id.clone(), table.device_index, feature.logical_id);
                let Some(&(runtime, feature_type, version)) = observed.get(&key) else {
                    return Err(io::Error::other(format!(
                        "feature 0x{:04x} (device {}) was declared but not observed in a Root response",
                        feature.logical_id, table.device_index
                    ))
                    .into());
                };
                if (runtime, feature_type, version)
                    != (feature.runtime_index, feature.feature_type, feature.version)
                {
                    return Err(io::Error::other(format!(
                        "feature 0x{:04x} (device {}) was declared as index={}, type={}, version={} but Root returned index={runtime}, type={feature_type}, version={version}",
                        feature.logical_id,
                        table.device_index,
                        feature.runtime_index,
                        feature.feature_type,
                        feature.version
                    ))
                    .into());
                }
                report.features += 1;
            }
        }
        Ok(())
    }
}

type DecodedReading = (String, u8, u64, BatteryStatus);

fn replay_exchange(
    exchange: &CaptureHidppExchange,
    features: &FeatureContext,
    observed: &mut HashMap<(String, u8, u16), (u8, u8, u8)>,
    unified_capabilities: &mut HashMap<(String, u8, u16), UnifiedBatteryCapabilities>,
    decoded: &mut Vec<DecodedReading>,
    report: &mut ReplayReport,
) -> Result<(), Box<dyn Error>> {
    report.exchanges += 1;
    let request_bytes = from_hex(&exchange.request)?;
    let request = HidppFrame::parse(&request_bytes)?;
    if request.device_index != exchange.device_index {
        return Err(io::Error::other(format!(
            "request device index {} does not match the record ({})",
            request.device_index, exchange.device_index
        ))
        .into());
    }

    let logical = features
        .logical(
            observed,
            &exchange.id,
            exchange.device_index,
            request.feature_index,
        )
        .ok_or_else(|| {
            io::Error::other(format!(
                "feature index {} has no recorded feature table entry or Root response",
                request.feature_index
            ))
        })?;
    if !is_read_only(logical, request.function_id) {
        return Err(io::Error::other(format!(
            "request 0x{logical:04x}/{} is not on the read-only replay allowlist",
            request.function_id
        ))
        .into());
    }

    let Some(response_hex) = &exchange.response else {
        if exchange.error.is_none() {
            return Err(
                io::Error::other("exchange has neither a response nor a recorded error").into(),
            );
        }
        report.transport_errors += 1;
        return Ok(());
    };
    if exchange.error.is_some() {
        return Err(io::Error::other("exchange has both a response and a recorded error").into());
    }

    let response_bytes = from_hex(response_hex)?;
    let response = HidppFrame::parse(&response_bytes)?;
    if response.feature_index == 0xff {
        return Err(io::Error::other(
            "response is a remote error frame, but the exchange was recorded as successful",
        )
        .into());
    }
    if response.device_index != request.device_index
        || response.feature_index != request.feature_index
        || response.function_id != request.function_id
        || response.software_id != request.software_id
    {
        return Err(io::Error::other(format!(
            "response does not match the request (request {}:{}:{}, response {}:{}:{})",
            request.device_index,
            request.feature_index,
            request.function_id,
            response.device_index,
            response.feature_index,
            response.function_id
        ))
        .into());
    }

    match logical {
        FEATURE_ROOT => replay_root(exchange, &request, &response, observed)?,
        FEATURE_SET => replay_feature_set(&request, &response)?,
        FEATURE_UNIFIED_BATTERY => {
            replay_unified(exchange, &request, &response, unified_capabilities, decoded)?;
        }
        FEATURE_BATTERY_STATUS => {
            let battery = parse_legacy_battery_status(&response)?;
            decoded.push((
                exchange.id.clone(),
                exchange.device_index,
                exchange.t_ms,
                battery,
            ));
        }
        _ => {
            return Err(io::Error::other(format!(
                "no replay decoder for logical feature 0x{logical:04x}"
            ))
            .into());
        }
    }
    Ok(())
}

fn is_read_only(logical: u16, function: u8) -> bool {
    matches!(
        (logical, function),
        (FEATURE_ROOT, FUNCTION_GET_FEATURE)
            | (
                FEATURE_SET,
                FUNCTION_GET_FEATURE_SET_COUNT | FUNCTION_GET_FEATURE_SET_ID
            )
            | (FEATURE_BATTERY_STATUS, FUNCTION_GET_BATTERY_STATUS)
            | (
                FEATURE_UNIFIED_BATTERY,
                FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES | FUNCTION_GET_UNIFIED_BATTERY_INFO
            )
    )
}

fn replay_root(
    exchange: &CaptureHidppExchange,
    request: &HidppFrame,
    response: &HidppFrame,
    observed: &mut HashMap<(String, u8, u16), (u8, u8, u8)>,
) -> Result<(), Box<dyn Error>> {
    if request.params.len() < 2 {
        return Err(io::Error::other("Root request is missing the logical feature ID").into());
    }
    if response.params.len() < 3 {
        return Err(io::Error::other("Root response is missing feature metadata").into());
    }
    let logical = u16::from_be_bytes([request.params[0], request.params[1]]);
    let metadata = (response.params[0], response.params[1], response.params[2]);
    // Index zero means "unsupported" for an ordinary feature, but it is the
    // real runtime index of Root when a device lists Root in FeatureSet.
    if metadata.0 != 0 || logical == FEATURE_ROOT {
        observed.insert(
            (exchange.id.clone(), exchange.device_index, logical),
            metadata,
        );
    }
    Ok(())
}

fn replay_feature_set(request: &HidppFrame, response: &HidppFrame) -> Result<(), Box<dyn Error>> {
    match request.function_id {
        FUNCTION_GET_FEATURE_SET_COUNT if response.params.is_empty() => {
            return Err(io::Error::other("FeatureSet count response is missing its count").into());
        }
        FUNCTION_GET_FEATURE_SET_ID if response.params.len() < 4 => {
            return Err(
                io::Error::other("FeatureSet entry is missing its ID, type, or version").into(),
            );
        }
        _ => {}
    }
    Ok(())
}

fn replay_unified(
    exchange: &CaptureHidppExchange,
    request: &HidppFrame,
    response: &HidppFrame,
    capabilities: &mut HashMap<(String, u8, u16), UnifiedBatteryCapabilities>,
    decoded: &mut Vec<DecodedReading>,
) -> Result<(), Box<dyn Error>> {
    let key = (
        exchange.id.clone(),
        exchange.device_index,
        FEATURE_UNIFIED_BATTERY,
    );
    match request.function_id {
        FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES => {
            capabilities.insert(key, parse_unified_battery_capabilities(response)?);
        }
        FUNCTION_GET_UNIFIED_BATTERY_INFO => {
            let capabilities = capabilities.get(&key).ok_or_else(|| {
                io::Error::other("unified battery info appears before its capabilities response")
            })?;
            let battery = parse_unified_battery(response, *capabilities)?;
            decoded.push((
                exchange.id.clone(),
                exchange.device_index,
                exchange.t_ms,
                battery,
            ));
        }
        _ => {}
    }
    Ok(())
}

fn verify_hidpp_readings(
    file: &CaptureFile,
    decoded: &[DecodedReading],
    report: &mut ReplayReport,
) -> Result<(), Box<dyn Error>> {
    for record in &file.records {
        let CaptureRecord::Reading(reading) = record else {
            continue;
        };
        if reading.backend != LOGITECH_HIDPP_20.name() {
            continue;
        }
        let device_index = reading.device_index.ok_or_else(|| {
            io::Error::other(format!(
                "HID++ reading for {} has no HID device index",
                reading.id
            ))
        })?;
        let battery = decoded
            .iter()
            .filter(|(id, device, t_ms, _)| {
                id == &reading.id && *device == device_index && *t_ms <= reading.t_ms
            })
            .max_by_key(|(_, _, t_ms, _)| *t_ms)
            .map(|(_, _, _, battery)| battery)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "reading for {} (device {}) has no decoded HID++ response",
                    reading.id, device_index
                ))
            })?;
        if battery_snapshot(battery) != reading.battery {
            return Err(io::Error::other(format!(
                "reading for {} (device {}) does not match the decoded HID++ response",
                reading.id, device_index
            ))
            .into());
        }
        report.readings += 1;
    }
    Ok(())
}

fn replay_generic(file: &CaptureFile, report: &mut ReplayReport) -> Result<(), Box<dyn Error>> {
    let mut layouts = HashMap::new();
    for record in &file.records {
        if let CaptureRecord::HidLayout(layout) = record {
            build_generic_layout(layout)?;
            layouts.insert(layout.id.clone(), layout);
            report.layouts += 1;
        }
    }

    for record in &file.records {
        let CaptureRecord::HidSample(sample) = record else {
            continue;
        };
        let layout = layouts.get(&sample.id).ok_or_else(|| {
            io::Error::other(format!("sample for {} has no recorded layout", sample.id))
        })?;
        let (generic_layout, values, buttons) = captured_sample(layout, sample)?;
        let battery = decode_generic_battery(&generic_layout, &values, &buttons)
            .map_err(|error| io::Error::other(error.to_string()))?;
        if battery_snapshot(&battery) != sample.battery {
            return Err(io::Error::other(format!(
                "sample for {} does not match the decoded generic HID values",
                sample.id
            ))
            .into());
        }
        verify_generic_reading(file, sample.id.as_str(), sample.t_ms, &battery)?;
        report.samples += 1;
        report.readings += 1;
    }
    Ok(())
}

fn verify_generic_reading(
    file: &CaptureFile,
    id: &str,
    sample_t_ms: u64,
    battery: &BatteryStatus,
) -> Result<(), Box<dyn Error>> {
    let reading = file
        .records
        .iter()
        .filter_map(|record| match record {
            CaptureRecord::Reading(reading)
                if reading.id == id
                    && reading.backend == GENERIC_HID_BATTERY.name()
                    && reading.t_ms >= sample_t_ms =>
            {
                Some(reading)
            }
            _ => None,
        })
        .min_by_key(|reading| reading.t_ms)
        .ok_or_else(|| {
            io::Error::other(format!(
                "sample for {id} has no matching generic HID reading"
            ))
        })?;
    if battery_snapshot(battery) != reading.battery {
        return Err(io::Error::other(format!(
            "reading for {id} does not match the decoded generic HID sample"
        ))
        .into());
    }
    Ok(())
}

fn replay_bluetooth(file: &CaptureFile, report: &mut ReplayReport) -> Result<(), Box<dyn Error>> {
    let devices: std::collections::HashSet<_> = file
        .records
        .iter()
        .filter_map(|record| match record {
            CaptureRecord::BluetoothDevice(device) => Some(device.id.as_str()),
            _ => None,
        })
        .collect();

    for record in &file.records {
        match record {
            CaptureRecord::BluetoothProperty(property) => {
                report.bluetooth_evidence += 1;
                if !devices.contains(property.id.as_str()) {
                    return Err(io::Error::other(format!(
                        "property evidence for {} has no device record",
                        property.id
                    ))
                    .into());
                }
                if property.property.is_empty() || property.property_type.is_empty() {
                    return Err(io::Error::other(format!(
                        "property evidence for {} has an empty property name or type",
                        property.id
                    ))
                    .into());
                }
            }
            CaptureRecord::BluetoothGatt(gatt) => {
                report.bluetooth_evidence += 1;
                if !devices.contains(gatt.id.as_str()) {
                    return Err(io::Error::other(format!(
                        "GATT evidence for {} has no device record",
                        gatt.id
                    ))
                    .into());
                }
                if gatt.service.is_empty() || gatt.characteristic.is_empty() {
                    return Err(io::Error::other(format!(
                        "GATT evidence for {} has an empty service or characteristic",
                        gatt.id
                    ))
                    .into());
                }
                from_hex(&gatt.value).map_err(|error| {
                    io::Error::other(format!("GATT evidence for {}: {error}", gatt.id))
                })?;
            }
            _ => {}
        }
    }

    for record in &file.records {
        let CaptureRecord::Reading(reading) = record else {
            continue;
        };
        if reading.device_index.is_some() {
            continue;
        }
        if !devices.contains(reading.id.as_str()) {
            return Err(io::Error::other(format!(
                "Bluetooth reading for {} has no device record",
                reading.id
            ))
            .into());
        }
        let battery = match reading.backend.as_str() {
            SOURCE_DEVICE_PROPERTY => {
                let property =
                    latest_property(file, &reading.id, reading.t_ms).ok_or_else(|| {
                        io::Error::other(format!(
                            "Bluetooth reading for {} has no property evidence at or before {} ms",
                            reading.id, reading.t_ms
                        ))
                    })?;
                let level = decode_windows_battery_life(property.raw_value).map_err(|error| {
                    io::Error::other(format!(
                        "property evidence for {} is invalid: {error}",
                        reading.id
                    ))
                })?;
                let level = level.ok_or_else(|| {
                    io::Error::other(format!(
                        "property evidence for {} is the unknown sentinel, so no reading is possible",
                        reading.id
                    ))
                })?;
                bluetooth_battery(level, SOURCE_DEVICE_PROPERTY)
            }
            SOURCE_GATT => {
                let gatt = latest_gatt(file, &reading.id, reading.t_ms).ok_or_else(|| {
                    io::Error::other(format!(
                        "Bluetooth reading for {} has no GATT evidence at or before {} ms",
                        reading.id, reading.t_ms
                    ))
                })?;
                let payload = from_hex(&gatt.value)?;
                let level = decode_battery_level(&payload).map_err(|error| {
                    io::Error::other(format!(
                        "GATT evidence for {} is invalid: {error}",
                        reading.id
                    ))
                })?;
                bluetooth_battery(level, SOURCE_GATT)
            }
            other => {
                return Err(io::Error::other(format!(
                    "reading for {} has no HID device index and an unknown backend {other:?}",
                    reading.id
                ))
                .into())
            }
        };
        if battery != reading.battery {
            return Err(io::Error::other(format!(
                "Bluetooth reading for {} does not match its recorded evidence",
                reading.id
            ))
            .into());
        }
        report.bluetooth_readings += 1;
    }
    Ok(())
}

fn latest_property<'a>(
    file: &'a CaptureFile,
    id: &str,
    t_ms: u64,
) -> Option<&'a CaptureBluetoothProperty> {
    file.records
        .iter()
        .filter_map(|record| match record {
            CaptureRecord::BluetoothProperty(property)
                if property.id == id && property.t_ms <= t_ms =>
            {
                Some(property)
            }
            _ => None,
        })
        .max_by_key(|property| property.t_ms)
}

fn latest_gatt<'a>(file: &'a CaptureFile, id: &str, t_ms: u64) -> Option<&'a CaptureBluetoothGatt> {
    file.records
        .iter()
        .filter_map(|record| match record {
            CaptureRecord::BluetoothGatt(gatt) if gatt.id == id && gatt.t_ms <= t_ms => Some(gatt),
            _ => None,
        })
        .max_by_key(|gatt| gatt.t_ms)
}

fn bluetooth_battery(level: u8, source: &'static str) -> volty_capture::BatterySnapshot {
    battery_snapshot(&volty_bluetooth::battery_status(level, source))
}

fn build_generic_layout(layout: &CaptureHidLayout) -> Result<GenericBatteryLayout, Box<dyn Error>> {
    let Some(selected) = &layout.selected else {
        return Err(io::Error::other(format!(
            "layout for {} has no selected battery source",
            layout.id
        ))
        .into());
    };
    let percent = selected
        .percent
        .as_ref()
        .map(|field| find_value(&layout.values, field))
        .transpose()?;
    let ratio = selected
        .ratio
        .as_ref()
        .map(|ratio| {
            Ok::<_, Box<dyn Error>>((
                find_value(&layout.values, &ratio.remaining)?,
                find_value(&layout.values, &ratio.full)?,
            ))
        })
        .transpose()?;
    if percent.is_some() == ratio.is_some() {
        return Err(io::Error::other(format!(
            "layout for {} must select exactly one level source",
            layout.id
        ))
        .into());
    }
    Ok(GenericBatteryLayout {
        values: layout.values.iter().map(generic_value).collect(),
        buttons: layout.buttons.iter().map(generic_button).collect(),
        percent,
        ratio,
        charging: selected
            .charging
            .as_ref()
            .map(|field| find_button(&layout.buttons, field))
            .transpose()?,
        discharging: selected
            .discharging
            .as_ref()
            .map(|field| find_button(&layout.buttons, field))
            .transpose()?,
    })
}

fn find_value(
    values: &[volty_capture::HidValueCapability],
    field: &volty_capture::HidFieldRef,
) -> Result<GenericBatteryValue, Box<dyn Error>> {
    values
        .iter()
        .find(|value| {
            value.page == field.page
                && value.usage == field.usage
                && value.report_id == field.report_id
                && value.kind == field.kind
        })
        .map(generic_value)
        .ok_or_else(|| {
            io::Error::other(format!(
                "selected field {:04x}:{:04x} is missing from the layout values",
                field.page, field.usage
            ))
            .into()
        })
}

fn find_button(
    buttons: &[volty_capture::HidButtonCapability],
    field: &volty_capture::HidFieldRef,
) -> Result<GenericBatteryButton, Box<dyn Error>> {
    buttons
        .iter()
        .find(|button| {
            button.page == field.page
                && button.usage == field.usage
                && button.report_id == field.report_id
                && button.kind == field.kind
        })
        .map(generic_button)
        .ok_or_else(|| {
            io::Error::other(format!(
                "selected button {:04x}:{:04x} is missing from the layout buttons",
                field.page, field.usage
            ))
            .into()
        })
}

type CapturedGenericSample = (
    GenericBatteryLayout,
    Vec<GenericBatteryValueReading>,
    Vec<GenericBatteryButtonReading>,
);

fn captured_sample(
    layout: &CaptureHidLayout,
    sample: &volty_capture::CaptureHidSample,
) -> Result<CapturedGenericSample, Box<dyn Error>> {
    Ok((
        build_generic_layout(layout)?,
        sample
            .values
            .iter()
            .map(|value| GenericBatteryValueReading {
                page: value.page,
                usage: value.usage,
                report_id: value.report_id,
                kind: generic_report_kind(value.kind),
                raw_value: value.raw_value,
            })
            .collect(),
        sample
            .buttons
            .iter()
            .map(|button| GenericBatteryButtonReading {
                page: button.page,
                usage: button.usage,
                report_id: button.report_id,
                kind: generic_report_kind(button.kind),
                pressed: button.pressed,
            })
            .collect(),
    ))
}

fn generic_value(value: &volty_capture::HidValueCapability) -> GenericBatteryValue {
    GenericBatteryValue {
        page: value.page,
        usage: value.usage,
        report_id: value.report_id,
        kind: generic_report_kind(value.kind),
        logical_min: value.logical_min,
        logical_max: value.logical_max,
    }
}

fn generic_button(button: &volty_capture::HidButtonCapability) -> GenericBatteryButton {
    GenericBatteryButton {
        page: button.page,
        usage: button.usage,
        report_id: button.report_id,
        kind: generic_report_kind(button.kind),
    }
}

const fn generic_report_kind(kind: HidReportKind) -> GenericReportKind {
    match kind {
        HidReportKind::Feature => GenericReportKind::Feature,
        HidReportKind::Input => GenericReportKind::Input,
    }
}

/// Runs `volty capture inspect <path>` without touching hardware.
pub fn inspect_cli(path: &Path) -> Result<(), Box<dyn Error>> {
    let file = CaptureFile::read(path)?;
    println!("Capture: {}", path.display());
    println!();
    print!("{}", file.render_summary());
    if !file.render_summary().ends_with('\n') {
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use volty_capture::{
        to_hex, BatterySnapshot, CaptureBluetoothDevice, CaptureBluetoothGatt,
        CaptureBluetoothKind, CaptureBluetoothProperty, CaptureFooter, CaptureHeader,
        CaptureHidSample, CaptureMatch, CaptureMode, CaptureReading, CaptureWriter,
        HidButtonCapability, HidFieldRef, HidSelectedLayout, HidValueCapability, HidValueSample,
        HidppFeature, MatchOutcome, PrivacyProfile, SCHEMA,
    };

    fn header() -> CaptureHeader {
        CaptureHeader {
            schema: SCHEMA.to_owned(),
            capture_id: "test".to_owned(),
            created_at: "2026-10-29T00:00:00Z".to_owned(),
            app_version: "0.2.1".to_owned(),
            app_commit: None,
            os: "windows".to_owned(),
            architecture: "x86_64".to_owned(),
            mode: CaptureMode::Snapshot,
            privacy: PrivacyProfile::Standard,
            features: vec!["hidapi".to_owned()],
        }
    }

    fn status(
        percent: u8,
        level: &str,
        state: &str,
        raw_level: u8,
        raw_status: Option<u8>,
        source: &str,
    ) -> BatterySnapshot {
        BatterySnapshot {
            percent: Some(percent),
            level: level.to_owned(),
            state: state.to_owned(),
            raw_level,
            raw_status,
            source: source.to_owned(),
        }
    }

    fn unified_frame(
        device: u8,
        feature: u8,
        function: u8,
        software: u8,
        params: [u8; 16],
    ) -> String {
        to_hex(
            &HidppFrame::long(device, feature, function, software, params)
                .encode()
                .unwrap(),
        )
    }

    fn unified_exchange(
        request: [u8; 16],
        response: [u8; 16],
        t_ms: u64,
        id: &str,
        device: u8,
        function: u8,
        software: u8,
    ) -> CaptureRecord {
        CaptureRecord::HidppExchange(CaptureHidppExchange {
            id: id.to_owned(),
            device_index: device,
            t_ms,
            request: unified_frame(device, 6, function, software, request),
            response: Some(unified_frame(device, 6, function, software, response)),
            error: None,
            elapsed_ms: 1,
        })
    }

    fn unified_capture() -> CaptureFile {
        let mut records = vec![
            CaptureRecord::Interface(volty_capture::CaptureInterface {
                id: "iface-1".to_owned(),
                bus: "Usb".to_owned(),
                vendor_id: 0x046d,
                product_id: 0xc547,
                release_number: 0x0402,
                usage_page: Some(0xff00),
                usage: Some(0x0002),
                interface_number: 2,
                manufacturer: Some("Logitech".to_owned()),
                product: Some("USB Receiver".to_owned()),
                serial_present: false,
            }),
            CaptureRecord::Match(CaptureMatch {
                id: "iface-1".to_owned(),
                device_index: None,
                backend: Some(LOGITECH_HIDPP_20.name().to_owned()),
                outcome: MatchOutcome::Matched,
                detail: None,
            }),
            CaptureRecord::HidppFeatures(CaptureHidppFeatures {
                id: "iface-1".to_owned(),
                device_index: 1,
                features: vec![HidppFeature {
                    logical_id: FEATURE_UNIFIED_BATTERY,
                    runtime_index: 6,
                    feature_type: 0,
                    version: 1,
                }],
            }),
        ];

        let mut root_params = [0; 16];
        root_params[..2].copy_from_slice(&FEATURE_UNIFIED_BATTERY.to_be_bytes());
        let mut root_response = [0; 16];
        root_response[..3].copy_from_slice(&[6, 0, 1]);
        records.push(CaptureRecord::HidppExchange(CaptureHidppExchange {
            id: "iface-1".to_owned(),
            device_index: 1,
            t_ms: 1,
            request: unified_frame(1, 0, FUNCTION_GET_FEATURE, 1, root_params),
            response: Some(unified_frame(1, 0, FUNCTION_GET_FEATURE, 1, root_response)),
            error: None,
            elapsed_ms: 1,
        }));

        let mut capabilities = [0; 16];
        capabilities[..2].copy_from_slice(&[0x0f, 0x03]);
        records.push(unified_exchange(
            [0; 16],
            capabilities,
            2,
            "iface-1",
            1,
            FUNCTION_GET_UNIFIED_BATTERY_CAPABILITIES,
            2,
        ));

        let mut battery = [0; 16];
        battery[..3].copy_from_slice(&[87, 4, 1]);
        records.push(unified_exchange(
            [0; 16],
            battery,
            3,
            "iface-1",
            1,
            FUNCTION_GET_UNIFIED_BATTERY_INFO,
            3,
        ));
        records.push(CaptureRecord::Reading(CaptureReading {
            id: "iface-1".to_owned(),
            device_index: Some(1),
            t_ms: 4,
            backend: LOGITECH_HIDPP_20.name().to_owned(),
            label: None,
            battery: status(87, "good", "charging", 4, Some(1), "HID++ Unified Battery"),
        }));

        CaptureFile {
            header: header(),
            records,
            footer: CaptureFooter {
                schema: SCHEMA.to_owned(),
                records: 7,
                complete: true,
            },
        }
    }

    fn generic_capture() -> CaptureFile {
        let value = HidValueCapability {
            page: 0x0006,
            usage: 0x0020,
            report_id: 1,
            kind: HidReportKind::Input,
            logical_min: 0,
            logical_max: 255,
        };
        let discharging = HidButtonCapability {
            page: 0x0085,
            usage: 0x0045,
            report_id: 2,
            kind: HidReportKind::Input,
        };
        let field = HidFieldRef {
            page: value.page,
            usage: value.usage,
            report_id: value.report_id,
            kind: value.kind,
        };
        let button = HidFieldRef {
            page: discharging.page,
            usage: discharging.usage,
            report_id: discharging.report_id,
            kind: discharging.kind,
        };
        CaptureFile {
            header: header(),
            records: vec![
                CaptureRecord::Interface(volty_capture::CaptureInterface {
                    id: "iface-1".to_owned(),
                    bus: "Usb".to_owned(),
                    vendor_id: 0x1234,
                    product_id: 0x5678,
                    release_number: 1,
                    usage_page: Some(0x0001),
                    usage: Some(0x0006),
                    interface_number: 0,
                    manufacturer: None,
                    product: None,
                    serial_present: false,
                }),
                CaptureRecord::Match(CaptureMatch {
                    id: "iface-1".to_owned(),
                    device_index: None,
                    backend: Some(GENERIC_HID_BATTERY.name().to_owned()),
                    outcome: MatchOutcome::Matched,
                    detail: None,
                }),
                CaptureRecord::HidLayout(CaptureHidLayout {
                    id: "iface-1".to_owned(),
                    device_index: None,
                    values: vec![value.clone()],
                    buttons: vec![discharging.clone()],
                    selected: Some(HidSelectedLayout {
                        percent: Some(field),
                        ratio: None,
                        charging: None,
                        discharging: Some(button),
                    }),
                }),
                CaptureRecord::HidSample(CaptureHidSample {
                    id: "iface-1".to_owned(),
                    device_index: None,
                    t_ms: 1,
                    values: vec![HidValueSample {
                        page: value.page,
                        usage: value.usage,
                        report_id: value.report_id,
                        kind: value.kind,
                        raw_value: 128,
                    }],
                    buttons: vec![volty_capture::HidButtonSample {
                        page: discharging.page,
                        usage: discharging.usage,
                        report_id: discharging.report_id,
                        kind: discharging.kind,
                        pressed: true,
                    }],
                    battery: status(50, "good", "discharging", 50, None, "HID Battery"),
                }),
                CaptureRecord::Reading(CaptureReading {
                    id: "iface-1".to_owned(),
                    device_index: Some(0xff),
                    t_ms: 2,
                    backend: GENERIC_HID_BATTERY.name().to_owned(),
                    label: None,
                    battery: status(50, "good", "discharging", 50, None, "HID Battery"),
                }),
            ],
            footer: CaptureFooter {
                schema: SCHEMA.to_owned(),
                records: 6,
                complete: true,
            },
        }
    }

    #[test]
    fn unified_frames_replay_to_the_recorded_reading() {
        let report = replay_capture(&unified_capture()).unwrap();
        assert_eq!(report.exchanges, 3);
        assert_eq!(report.features, 1);
        assert_eq!(report.readings, 1);
        assert_eq!(report.transport_errors, 0);
    }

    #[test]
    fn root_feature_entry_is_verified_from_its_own_root_response() {
        let mut file = unified_capture();
        let CaptureRecord::HidppFeatures(table) = &mut file.records[2] else {
            panic!("expected feature table");
        };
        table.features.push(HidppFeature {
            logical_id: FEATURE_ROOT,
            runtime_index: 0,
            feature_type: 0,
            version: 2,
        });
        let mut response = [0; 16];
        response[..3].copy_from_slice(&[0, 0, 2]);
        file.records.insert(
            4,
            CaptureRecord::HidppExchange(CaptureHidppExchange {
                id: "iface-1".to_owned(),
                device_index: 1,
                t_ms: 2,
                request: unified_frame(1, 0, FUNCTION_GET_FEATURE, 2, [0; 16]),
                response: Some(unified_frame(1, 0, FUNCTION_GET_FEATURE, 2, response)),
                error: None,
                elapsed_ms: 1,
            }),
        );

        let report = replay_capture(&file).unwrap();
        assert_eq!(report.features, 2);
        assert_eq!(report.readings, 1);
    }

    #[test]
    fn capture_without_a_feature_table_replays_from_root_responses() {
        let mut file = unified_capture();
        file.records
            .retain(|record| !matches!(record, CaptureRecord::HidppFeatures(_)));
        let report = replay_capture(&file).unwrap();
        assert_eq!(report.exchanges, 3);
        assert_eq!(report.features, 0);
        assert_eq!(report.readings, 1);
    }

    #[test]
    fn generic_layout_and_sample_replay_to_the_recorded_reading() {
        let report = replay_capture(&generic_capture()).unwrap();
        assert_eq!(report.layouts, 1);
        assert_eq!(report.samples, 1);
        assert_eq!(report.readings, 1);
    }

    #[test]
    fn replay_rejects_state_changing_requests() {
        let mut file = unified_capture();
        let CaptureRecord::HidppExchange(exchange) = &mut file.records[3] else {
            panic!("expected exchange");
        };
        exchange.request = unified_frame(1, 0, 1, 1, [0; 16]);
        exchange.response = Some(unified_frame(1, 0, 1, 1, [0; 16]));
        let error = replay_capture(&file).unwrap_err().to_string();
        assert!(error.contains("read-only"));
    }

    #[test]
    fn replay_detects_a_reading_that_the_frames_do_not_support() {
        let mut file = unified_capture();
        let CaptureRecord::Reading(reading) = &mut file.records[6] else {
            panic!("expected reading");
        };
        reading.battery = status(
            10,
            "critical",
            "discharging",
            1,
            Some(0),
            "HID++ Unified Battery",
        );
        let error = replay_capture(&file).unwrap_err().to_string();
        assert!(error.contains("does not match"));
    }

    fn bluetooth_capture(
        kind: CaptureBluetoothKind,
        backend: &'static str,
        evidence: CaptureRecord,
        battery: BatterySnapshot,
    ) -> CaptureFile {
        let mut records = vec![
            CaptureRecord::BluetoothDevice(CaptureBluetoothDevice {
                id: "bt-1".to_owned(),
                kind,
                name: Some("Headset".to_owned()),
            }),
            evidence,
            CaptureRecord::Reading(CaptureReading {
                id: "bt-1".to_owned(),
                device_index: None,
                t_ms: 10,
                backend: backend.to_owned(),
                label: Some("Headset".to_owned()),
                battery,
            }),
        ];
        let footer = CaptureFooter {
            schema: SCHEMA.to_owned(),
            records: u64::try_from(records.len()).unwrap() + 1,
            complete: true,
        };
        CaptureFile {
            header: header(),
            records: std::mem::take(&mut records),
            footer,
        }
    }

    #[test]
    fn bluetooth_property_evidence_replays_to_the_recorded_reading() {
        let file = bluetooth_capture(
            CaptureBluetoothKind::Pnp,
            SOURCE_DEVICE_PROPERTY,
            CaptureRecord::BluetoothProperty(CaptureBluetoothProperty {
                id: "bt-1".to_owned(),
                t_ms: 5,
                property: "DEVPKEY_Device_BatteryLevel".to_owned(),
                property_type: "byte".to_owned(),
                raw_value: 100,
            }),
            status(100, "unknown", "unknown", 100, None, SOURCE_DEVICE_PROPERTY),
        );

        let report = replay_capture(&file).unwrap();
        assert_eq!(report.bluetooth_evidence, 1);
        assert_eq!(report.bluetooth_readings, 1);
        assert_eq!(report.readings, 0);
    }

    #[test]
    fn bluetooth_gatt_evidence_replays_to_the_recorded_reading() {
        let file = bluetooth_capture(
            CaptureBluetoothKind::Ble,
            SOURCE_GATT,
            CaptureRecord::BluetoothGatt(CaptureBluetoothGatt {
                id: "bt-1".to_owned(),
                t_ms: 5,
                service: "180f".to_owned(),
                characteristic: "2a19".to_owned(),
                value: "64".to_owned(),
            }),
            status(100, "unknown", "unknown", 100, None, SOURCE_GATT),
        );

        let report = replay_capture(&file).unwrap();
        assert_eq!(report.bluetooth_evidence, 1);
        assert_eq!(report.bluetooth_readings, 1);
    }

    #[test]
    fn bluetooth_reading_must_match_its_evidence() {
        let mut file = bluetooth_capture(
            CaptureBluetoothKind::Pnp,
            SOURCE_DEVICE_PROPERTY,
            CaptureRecord::BluetoothProperty(CaptureBluetoothProperty {
                id: "bt-1".to_owned(),
                t_ms: 5,
                property: "DEVPKEY_Device_BatteryLevel".to_owned(),
                property_type: "byte".to_owned(),
                raw_value: 100,
            }),
            status(55, "unknown", "unknown", 55, None, SOURCE_DEVICE_PROPERTY),
        );
        let CaptureRecord::Reading(reading) = &mut file.records[2] else {
            panic!("expected reading");
        };
        reading.battery.percent = Some(54);
        let error = replay_capture(&file).unwrap_err().to_string();
        assert!(error.contains("does not match"), "{error}");
    }

    #[test]
    fn bluetooth_reading_requires_a_device_record() {
        let mut file = bluetooth_capture(
            CaptureBluetoothKind::Pnp,
            SOURCE_DEVICE_PROPERTY,
            CaptureRecord::BluetoothProperty(CaptureBluetoothProperty {
                id: "bt-1".to_owned(),
                t_ms: 5,
                property: "DEVPKEY_Device_BatteryLevel".to_owned(),
                property_type: "byte".to_owned(),
                raw_value: 100,
            }),
            status(100, "unknown", "unknown", 100, None, SOURCE_DEVICE_PROPERTY),
        );
        file.records.remove(0);
        let error = replay_capture(&file).unwrap_err().to_string();
        assert!(error.contains("no device record"), "{error}");
    }

    #[test]
    fn bluetooth_evidence_requires_a_device_record() {
        let mut file = bluetooth_capture(
            CaptureBluetoothKind::Pnp,
            SOURCE_DEVICE_PROPERTY,
            CaptureRecord::BluetoothProperty(CaptureBluetoothProperty {
                id: "bt-1".to_owned(),
                t_ms: 5,
                property: "DEVPKEY_Device_BatteryLevel".to_owned(),
                property_type: "byte".to_owned(),
                raw_value: 100,
            }),
            status(100, "unknown", "unknown", 100, None, SOURCE_DEVICE_PROPERTY),
        );
        file.records.remove(0);
        file.records
            .retain(|record| !matches!(record, CaptureRecord::Reading(_)));
        let error = replay_capture(&file).unwrap_err().to_string();
        assert!(error.contains("no device record"), "{error}");
    }

    #[test]
    fn fixture_files_validate_and_replay() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/hardware");
        for name in [
            "synthetic-g502x-unified.volty.jsonl",
            "g502x-plus-unified.volty.jsonl",
            "synthetic-generic-battery.volty.jsonl",
            "synthetic-bluetooth-headset.volty.jsonl",
        ] {
            let file = CaptureFile::read(root.join(name)).unwrap();
            let report = replay_capture(&file).unwrap();
            assert!(
                report.exchanges > 0 || report.samples > 0 || report.bluetooth_readings > 0,
                "{name}"
            );
        }
    }

    #[test]
    fn inspect_reads_without_opening_a_device() {
        let file = unified_capture();
        let path = std::env::temp_dir().join(format!(
            "volty-replay-test-{}.volty.jsonl",
            std::process::id()
        ));
        let mut writer = CaptureWriter::create(&path).unwrap();
        writer
            .write(&CaptureRecord::Header(file.header.clone()))
            .unwrap();
        for record in &file.records {
            writer.write(record).unwrap();
        }
        writer.finish(true).unwrap();
        let read = CaptureFile::read(&path).unwrap();
        assert_eq!(read.records.len(), file.records.len());
        std::fs::remove_file(path).unwrap();
    }
}
