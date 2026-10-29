//! Read-only hardware capture snapshots written by the tray and `volty capture`.

use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use volty_capture::{
    to_hex, BatterySnapshot, CaptureEvent, CaptureFile, CaptureHeader, CaptureHidLayout,
    CaptureHidSample, CaptureHidppExchange, CaptureHidppFeatures, CaptureInterface, CaptureMatch,
    CaptureMode, CaptureReading, CaptureRecord, CaptureWriter, EventLevel, HidButtonCapability,
    HidButtonSample, HidFieldRef, HidRatioRef, HidReportKind, HidSelectedLayout,
    HidValueCapability, HidValueSample, HidppFeature, MatchOutcome, PrivacyProfile, Redactor,
    SCHEMA,
};
use volty_hid::{
    enumerate_features, enumerate_hid_interfaces, query_battery_with_transport, resolve_hid_device,
    stable_interface_keys, BatteryLevel, BatteryProtocol, BatteryState, BatteryStatus,
    GenericBatteryButton, GenericBatteryButtonReading, GenericBatteryLayout, GenericBatteryValue,
    GenericBatteryValueReading, GenericHidBattery, GenericReportKind, HidInterfaceInfo,
    HidppSession, RecordingTransport, SoftwareIdAllocator, GENERIC_HID_BATTERY, LOGITECH_HIDPP_20,
};

use crate::state::DeviceState;

const CAPTURE_USAGE: &str = "Usage:\n  volty capture [--output <path>] [--strict]\n  volty capture inspect <capture.volty.jsonl>\n  volty capture replay <capture.volty.jsonl>\n\nWrites a read-only snapshot of the visible HID interfaces, or validates and\nreplays a previously written capture without opening a device. Nothing is\nuploaded automatically.";
const DIRECT_DEVICE_INDEX: u8 = 0xff;
const FEATURE_TIMEOUT: Duration = Duration::from_secs(2);
const DEVICE_PATH_MARKERS: [&str; 2] = ["\\\\?\\", "\\\\.\\"];

/// The path and validated summary of a written capture.
pub struct CaptureOutcome {
    pub path: PathBuf,
    pub summary: String,
}

/// Writes a snapshot to the default capture folder under `%APPDATA%\Volty`.
pub fn write_default_snapshot() -> Result<CaptureOutcome, Box<dyn Error>> {
    write_snapshot(&default_path()?, PrivacyProfile::Standard)
}

/// Writes a read-only snapshot of the current HID state to `path`.
pub fn write_snapshot(
    path: &Path,
    privacy: PrivacyProfile,
) -> Result<CaptureOutcome, Box<dyn Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            io::Error::other(format!(
                "could not create capture folder {}: {error}",
                parent.display()
            ))
        })?;
    }
    let (seconds, nanos) = unix_now()?;
    let writer = CaptureWriter::create(path)?;
    let result = write_capture(writer, path, privacy, seconds, nanos);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

/// Runs `volty capture` after the command name has been removed from the arguments.
pub fn run_cli(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args.first().map(String::as_str) {
        Some("inspect") => {
            let path = subcommand_path("inspect", &args[1..])?;
            return crate::replay::inspect_cli(&path);
        }
        Some("replay") => {
            let path = subcommand_path("replay", &args[1..])?;
            return crate::replay::replay_cli(&path);
        }
        _ => {}
    }

    let mut output = None;
    let mut privacy = PrivacyProfile::Standard;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" | "help" => {
                println!("{CAPTURE_USAGE}");
                return Ok(());
            }
            "--strict" => privacy = PrivacyProfile::Strict,
            "--output" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(usage_error("--output requires a path"));
                };
                output = Some(PathBuf::from(value));
            }
            value if value.starts_with("--output=") => {
                let path = &value["--output=".len()..];
                if path.is_empty() {
                    return Err(usage_error("--output requires a path"));
                }
                output = Some(PathBuf::from(path));
            }
            other => {
                return Err(usage_error(&format!("unknown capture option {other:?}")));
            }
        }
        index += 1;
    }

    let path = match output {
        Some(path) => path,
        None => default_path()?,
    };
    let outcome = write_snapshot(&path, privacy)?;
    println!(
        "Wrote hardware capture: {}\nNothing was uploaded automatically.\n",
        outcome.path.display()
    );
    print!("{}", outcome.summary);
    if !outcome.summary.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn write_capture(
    mut writer: CaptureWriter,
    path: &Path,
    privacy: PrivacyProfile,
    seconds: u64,
    nanos: u32,
) -> Result<CaptureOutcome, Box<dyn Error>> {
    let started = Instant::now();
    let redactor = Redactor::new(privacy);
    let header = CaptureHeader {
        schema: SCHEMA.to_owned(),
        capture_id: format!("{seconds}-{nanos:09}"),
        created_at: unix_to_rfc3339(seconds),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        app_commit: option_env!("VOLTY_GIT_COMMIT").map(str::to_owned),
        os: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        mode: CaptureMode::Snapshot,
        privacy,
        features: enabled_features(),
    };
    writer.write(&CaptureRecord::Header(header))?;

    let interfaces = enumerate_hid_interfaces()?;
    let keys = stable_interface_keys(&interfaces);
    let known_state = DeviceState::open_local().ok();
    for (index, interface) in interfaces.iter().enumerate() {
        let id = format!("iface-{}", index + 1);
        writer.write(&CaptureRecord::Interface(capture_interface(
            &id, interface, &redactor,
        )))?;
        match resolve_hid_device(interface) {
            Ok(Some(device)) => {
                let backend = device.protocol().name();
                writer.write(&CaptureRecord::Match(CaptureMatch {
                    id: id.clone(),
                    device_index: None,
                    backend: Some(backend.to_owned()),
                    outcome: MatchOutcome::Matched,
                    detail: None,
                }))?;
                if backend == LOGITECH_HIDPP_20.name() {
                    let candidates = known_state
                        .as_ref()
                        .zip(keys.get(index))
                        .and_then(|(state, key)| {
                            state
                                .known_device_indices(key, LOGITECH_HIDPP_20.device_indices())
                                .ok()
                        })
                        .filter(|indices| !indices.is_empty());
                    capture_hidpp(
                        &mut writer,
                        started,
                        &id,
                        interface,
                        &redactor,
                        candidates
                            .as_deref()
                            .unwrap_or_else(|| LOGITECH_HIDPP_20.device_indices()),
                    )?;
                } else if backend == GENERIC_HID_BATTERY.name() {
                    capture_generic(&mut writer, started, &id, interface, &redactor)?;
                } else {
                    write_event(
                        &mut writer,
                        started,
                        Some(&id),
                        EventLevel::Warning,
                        format!("no capture recorder for backend {backend:?}"),
                    )?;
                }
            }
            Ok(None) => {
                writer.write(&CaptureRecord::Match(CaptureMatch {
                    id: id.clone(),
                    device_index: None,
                    backend: None,
                    outcome: MatchOutcome::Unsupported,
                    detail: None,
                }))?;
            }
            Err(error) => {
                writer.write(&CaptureRecord::Match(CaptureMatch {
                    id: id.clone(),
                    device_index: None,
                    backend: None,
                    outcome: MatchOutcome::Ambiguous,
                    detail: Some(error.matching_model_names.join(", ")),
                }))?;
            }
        }
    }

    writer.finish(true)?;
    let file = CaptureFile::read(path)?;
    Ok(CaptureOutcome {
        path: path.to_path_buf(),
        summary: file.render_summary(),
    })
}

fn capture_hidpp(
    writer: &mut CaptureWriter,
    started: Instant,
    id: &str,
    interface: &HidInterfaceInfo,
    redactor: &Redactor,
    device_indices: &[u8],
) -> Result<(), Box<dyn Error>> {
    let session = match HidppSession::open_for_interface(interface) {
        Ok(session) => session,
        Err(error) => {
            write_event(
                writer,
                started,
                Some(id),
                EventLevel::Error,
                format!(
                    "could not open the HID++ interface: {}",
                    safe_error(&error.to_string())
                ),
            )?;
            return Ok(());
        }
    };
    let mut recorder = RecordingTransport::new(session);
    let label = redactor.product(interface.product_string.as_deref());
    for &device_index in device_indices {
        let reading = query_battery_with_transport(&mut recorder, device_index);
        let exchanges = recorder.take_exchanges();
        write_exchanges(writer, started, id, device_index, &exchanges)?;
        match reading {
            Ok(battery) => {
                write_reading(
                    writer,
                    started,
                    id,
                    device_index,
                    LOGITECH_HIDPP_20.name(),
                    label.clone(),
                    &battery,
                )?;
                capture_features(writer, started, id, device_index, &mut recorder)?;
            }
            Err(error) => {
                write_event(
                    writer,
                    started,
                    Some(id),
                    EventLevel::Info,
                    format!(
                        "HID++ slot 0x{device_index:02x}: {}",
                        safe_error(&error.to_string())
                    ),
                )?;
            }
        }
    }
    Ok(())
}

fn capture_features(
    writer: &mut CaptureWriter,
    started: Instant,
    id: &str,
    device_index: u8,
    recorder: &mut RecordingTransport<HidppSession>,
) -> Result<(), Box<dyn Error>> {
    let result = enumerate_features(
        recorder,
        device_index,
        &mut SoftwareIdAllocator::new(),
        FEATURE_TIMEOUT,
    );
    let exchanges = recorder.take_exchanges();
    write_exchanges(writer, started, id, device_index, &exchanges)?;
    match result {
        Ok(features) => {
            writer.write(&CaptureRecord::HidppFeatures(CaptureHidppFeatures {
                id: id.to_owned(),
                device_index,
                features: features
                    .into_iter()
                    .map(|(logical_id, info)| HidppFeature {
                        logical_id,
                        runtime_index: info.feature_index,
                        feature_type: info.feature_type,
                        version: info.version,
                    })
                    .collect(),
            }))?;
        }
        Err(error) => {
            write_event(
                writer,
                started,
                Some(id),
                EventLevel::Info,
                format!(
                    "HID++ slot 0x{device_index:02x} feature table: {}",
                    safe_error(&error.to_string())
                ),
            )?;
        }
    }
    Ok(())
}

fn capture_generic(
    writer: &mut CaptureWriter,
    started: Instant,
    id: &str,
    interface: &HidInterfaceInfo,
    redactor: &Redactor,
) -> Result<(), Box<dyn Error>> {
    let Some(probe) = GenericHidBattery.probe(interface) else {
        write_event(
            writer,
            started,
            Some(id),
            EventLevel::Warning,
            "the interface no longer exposes a readable standard HID battery".to_owned(),
        )?;
        return Ok(());
    };
    writer.write(&CaptureRecord::HidLayout(CaptureHidLayout {
        id: id.to_owned(),
        device_index: None,
        values: probe.layout.values.iter().copied().map(hid_value).collect(),
        buttons: probe
            .layout
            .buttons
            .iter()
            .copied()
            .map(hid_button)
            .collect(),
        selected: Some(selected_layout(&probe.layout)),
    }))?;
    let label = redactor.product(interface.product_string.as_deref());
    match probe.sample {
        Ok(sample) => {
            writer.write(&CaptureRecord::HidSample(CaptureHidSample {
                id: id.to_owned(),
                device_index: None,
                t_ms: elapsed_ms(started),
                values: sample
                    .values
                    .iter()
                    .copied()
                    .map(hid_value_sample)
                    .collect(),
                buttons: sample
                    .buttons
                    .iter()
                    .copied()
                    .map(hid_button_sample)
                    .collect(),
                battery: battery_snapshot(&sample.battery),
            }))?;
            write_reading(
                writer,
                started,
                id,
                DIRECT_DEVICE_INDEX,
                GENERIC_HID_BATTERY.name(),
                label,
                &sample.battery,
            )?;
        }
        Err(error) => {
            write_event(
                writer,
                started,
                Some(id),
                EventLevel::Error,
                format!(
                    "could not read the standard HID battery: {}",
                    safe_error(&error.to_string())
                ),
            )?;
        }
    }
    Ok(())
}

fn write_reading(
    writer: &mut CaptureWriter,
    started: Instant,
    id: &str,
    device_index: u8,
    backend: &str,
    label: Option<String>,
    battery: &BatteryStatus,
) -> Result<(), Box<dyn Error>> {
    writer.write(&CaptureRecord::Reading(CaptureReading {
        id: id.to_owned(),
        device_index,
        t_ms: elapsed_ms(started),
        backend: backend.to_owned(),
        label,
        battery: battery_snapshot(battery),
    }))?;
    Ok(())
}

fn write_exchanges(
    writer: &mut CaptureWriter,
    started: Instant,
    id: &str,
    device_index: u8,
    exchanges: &[volty_hid::HidppExchange],
) -> Result<(), Box<dyn Error>> {
    let total: Duration = exchanges.iter().map(|exchange| exchange.elapsed).sum();
    let mut elapsed = started.elapsed().saturating_sub(total);
    for exchange in exchanges {
        elapsed += exchange.elapsed;
        writer.write(&CaptureRecord::HidppExchange(CaptureHidppExchange {
            id: id.to_owned(),
            device_index,
            t_ms: duration_ms(elapsed),
            request: to_hex(&exchange.request),
            response: exchange.response.as_deref().map(to_hex),
            error: exchange.error.as_deref().map(safe_error),
            elapsed_ms: duration_ms(exchange.elapsed),
        }))?;
    }
    Ok(())
}

fn write_event(
    writer: &mut CaptureWriter,
    started: Instant,
    id: Option<&str>,
    level: EventLevel,
    message: String,
) -> Result<(), Box<dyn Error>> {
    writer.write(&CaptureRecord::Event(CaptureEvent {
        t_ms: elapsed_ms(started),
        id: id.map(str::to_owned),
        level,
        message,
    }))?;
    Ok(())
}

fn capture_interface(
    id: &str,
    interface: &HidInterfaceInfo,
    redactor: &Redactor,
) -> CaptureInterface {
    CaptureInterface {
        id: id.to_owned(),
        bus: interface.bus_type.clone(),
        vendor_id: interface.vendor_id,
        product_id: interface.product_id,
        release_number: interface.release_number,
        usage_page: interface.usage_page,
        usage: interface.usage,
        interface_number: interface.interface_number,
        manufacturer: redactor.manufacturer(interface.manufacturer_string.as_deref()),
        product: redactor.product(interface.product_string.as_deref()),
        serial_present: interface
            .serial_number
            .as_deref()
            .is_some_and(|serial| !serial.trim().is_empty()),
    }
}

fn selected_layout(layout: &GenericBatteryLayout) -> HidSelectedLayout {
    HidSelectedLayout {
        percent: layout.percent.map(hid_field_ref),
        ratio: layout.ratio.map(|(remaining, full)| HidRatioRef {
            remaining: hid_field_ref(remaining),
            full: hid_field_ref(full),
        }),
        charging: layout.charging.map(hid_button_ref),
        discharging: layout.discharging.map(hid_button_ref),
    }
}

fn hid_value(value: GenericBatteryValue) -> HidValueCapability {
    HidValueCapability {
        page: value.page,
        usage: value.usage,
        report_id: value.report_id,
        kind: hid_report_kind(value.kind),
        logical_min: value.logical_min,
        logical_max: value.logical_max,
    }
}

fn hid_button(button: GenericBatteryButton) -> HidButtonCapability {
    HidButtonCapability {
        page: button.page,
        usage: button.usage,
        report_id: button.report_id,
        kind: hid_report_kind(button.kind),
    }
}

fn hid_field_ref(value: GenericBatteryValue) -> HidFieldRef {
    HidFieldRef {
        page: value.page,
        usage: value.usage,
        report_id: value.report_id,
        kind: hid_report_kind(value.kind),
    }
}

fn hid_button_ref(button: GenericBatteryButton) -> HidFieldRef {
    HidFieldRef {
        page: button.page,
        usage: button.usage,
        report_id: button.report_id,
        kind: hid_report_kind(button.kind),
    }
}

fn hid_value_sample(value: GenericBatteryValueReading) -> HidValueSample {
    HidValueSample {
        page: value.page,
        usage: value.usage,
        report_id: value.report_id,
        kind: hid_report_kind(value.kind),
        raw_value: value.raw_value,
    }
}

fn hid_button_sample(button: GenericBatteryButtonReading) -> HidButtonSample {
    HidButtonSample {
        page: button.page,
        usage: button.usage,
        report_id: button.report_id,
        kind: hid_report_kind(button.kind),
        pressed: button.pressed,
    }
}

const fn hid_report_kind(kind: GenericReportKind) -> HidReportKind {
    match kind {
        GenericReportKind::Feature => HidReportKind::Feature,
        GenericReportKind::Input => HidReportKind::Input,
    }
}

pub(crate) fn battery_snapshot(battery: &BatteryStatus) -> BatterySnapshot {
    BatterySnapshot {
        percent: battery.level_percent,
        level: level_name(battery.level).to_owned(),
        state: state_name(battery.state).to_owned(),
        raw_level: battery.raw_level,
        raw_status: battery.raw_status,
        source: battery.source.to_owned(),
    }
}

const fn level_name(level: BatteryLevel) -> &'static str {
    match level {
        BatteryLevel::Unknown => "unknown",
        BatteryLevel::Critical => "critical",
        BatteryLevel::Low => "low",
        BatteryLevel::Good => "good",
        BatteryLevel::Full => "full",
    }
}

const fn state_name(state: BatteryState) -> &'static str {
    match state {
        BatteryState::Unknown => "unknown",
        BatteryState::Discharging => "discharging",
        BatteryState::Charging => "charging",
        BatteryState::ChargingNearlyFull => "charging_nearly_full",
        BatteryState::Charged => "charged",
        BatteryState::ChargingSlow => "charging_slow",
        BatteryState::InvalidBattery => "invalid_battery",
        BatteryState::ThermalError => "thermal_error",
        BatteryState::ChargingError => "charging_error",
    }
}

/// Truncates an error before a Windows device path so captures never contain one.
fn safe_error(text: &str) -> String {
    if let Some(index) = DEVICE_PATH_MARKERS
        .iter()
        .filter_map(|marker| text.find(marker))
        .min()
    {
        let mut safe = text[..index].trim_end().to_owned();
        if !safe.is_empty() {
            safe.push(' ');
        }
        safe.push_str("<device path omitted>");
        return safe;
    }
    text.to_owned()
}

fn elapsed_ms(started: Instant) -> u64 {
    duration_ms(started.elapsed())
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn enabled_features() -> Vec<String> {
    let mut features = vec!["hidapi".to_owned(), "profile".to_owned()];
    if cfg!(feature = "bluetooth") {
        features.push("bluetooth".to_owned());
    }
    features
}

fn default_path() -> Result<PathBuf, Box<dyn Error>> {
    let app_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("could not determine the user application data folder"))?;
    let directory = app_data.join("Volty").join("captures");
    fs::create_dir_all(&directory).map_err(|error| {
        io::Error::other(format!(
            "could not create capture folder {}: {error}",
            directory.display()
        ))
    })?;
    let (seconds, nanos) = unix_now()?;
    Ok(directory.join(format!("volty-capture-{seconds}-{nanos:09}.volty.jsonl")))
}

fn unix_now() -> Result<(u64, u32), Box<dyn Error>> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            io::Error::other(format!(
                "the system clock is before the Unix epoch: {error}"
            ))
        })?;
    Ok((duration.as_secs(), duration.subsec_nanos()))
}

fn unix_to_rfc3339(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds_of_day / 3_600,
        seconds_of_day % 3_600 / 60,
        seconds_of_day % 60
    )
}

/// Converts days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

fn subcommand_path(command: &str, args: &[String]) -> Result<PathBuf, Box<dyn Error>> {
    match args {
        [path] => Ok(PathBuf::from(path)),
        [] => Err(usage_error(&format!("{command} requires a capture path"))),
        _ => Err(usage_error(&format!(
            "{command} accepts exactly one capture path"
        ))),
    }
}

fn usage_error(message: &str) -> Box<dyn Error> {
    io::Error::other(format!("{message}\n{CAPTURE_USAGE}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_timestamps_are_rfc3339() {
        assert_eq!(unix_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_to_rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(unix_to_rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn battery_state_and_level_names_match_the_schema() {
        let battery = BatteryStatus {
            level_percent: Some(73),
            level: BatteryLevel::Good,
            raw_level: 73,
            state: BatteryState::ChargingNearlyFull,
            raw_status: Some(4),
            source: "HID++ Unified Battery",
        };
        let snapshot = battery_snapshot(&battery);
        assert_eq!(snapshot.percent, Some(73));
        assert_eq!(snapshot.level, "good");
        assert_eq!(snapshot.state, "charging_nearly_full");
        assert_eq!(snapshot.raw_level, 73);
        assert_eq!(snapshot.raw_status, Some(4));
        assert_eq!(snapshot.source, "HID++ Unified Battery");
    }

    #[test]
    fn device_paths_are_removed_from_recorded_errors() {
        let error = r"failed to open \\?\hid#vid_046d&pid_c547#7&1a2b3c4d&0&0000";
        assert_eq!(safe_error(error), "failed to open <device path omitted>");
        assert_eq!(
            safe_error(r"failed to open \\.\HID#vid_046d"),
            "failed to open <device path omitted>"
        );
        assert_eq!(safe_error("timed out"), "timed out");
    }

    #[test]
    fn interface_capture_redacts_serial_values_and_model_strings() {
        let interface = HidInterfaceInfo {
            vendor_id: 0x046d,
            product_id: 0xc547,
            release_number: 0x0402,
            usage_page: Some(0xff00),
            usage: Some(0x0002),
            interface_number: 2,
            bus_type: "Usb".to_owned(),
            manufacturer_string: Some("Logitech".to_owned()),
            product_string: Some("USB Receiver".to_owned()),
            serial_number: Some("secret-serial".to_owned()),
            path: std::ffi::CString::new(r"\\?\hid#secret-path").unwrap(),
        };

        let standard = capture_interface(
            "iface-1",
            &interface,
            &Redactor::new(PrivacyProfile::Standard),
        );
        assert_eq!(standard.product.as_deref(), Some("USB Receiver"));
        assert_eq!(standard.manufacturer.as_deref(), Some("Logitech"));
        assert!(standard.serial_present);
        let debug = format!("{standard:?}");
        assert!(!debug.contains("secret-serial"));
        assert!(!debug.contains("secret-path"));

        let strict = capture_interface(
            "iface-1",
            &interface,
            &Redactor::new(PrivacyProfile::Strict),
        );
        assert_eq!(strict.product, None);
        assert_eq!(strict.manufacturer, None);
        assert!(strict.serial_present);
    }
}
