use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use volty_battery::{
    BatteryLevel, BatteryPercentage, BatteryProvider, BatteryState, BatteryStatus, DeviceId,
    DeviceLabel, ProviderEvent, ProviderPoll, ScanMode, SinkClosed, SourceReading,
};

use crate::os_battery::{bluez_battery, upower_battery, BLUEZ_PROPERTY};
use volty_capture::{
    CaptureBluetoothDevice, CaptureBluetoothKind, CaptureBluetoothProperty, CaptureEvent,
    CaptureReading, CaptureRecord, CaptureUpowerBattery, CaptureWriter, EventLevel,
};

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_DEVICE: &str = "org.freedesktop.UPower.Device";
const UPOWER_ROOT: &str = "/org/freedesktop/UPower";
const BLUEZ: &str = "org.bluez";
const MAX_UPOWER_AGE: u64 = 24 * 60 * 60;

pub(crate) fn provider() -> Box<dyn BatteryProvider> {
    Box::new(LinuxPower {
        call: system_bus,
        sysfs: "/sys/class/power_supply".into(),
        known: BTreeSet::new(),
    })
}

struct LinuxPower {
    call: fn(&[&str]) -> Result<Value, String>,
    sysfs: PathBuf,
    known: BTreeSet<DeviceId>,
}

fn system_bus(args: &[&str]) -> Result<Value, String> {
    busctl("--system", args)
}

fn busctl(bus: &str, args: &[&str]) -> Result<Value, String> {
    let output = Command::new("busctl")
        .args([
            bus,
            "--json=short",
            "--timeout=2s",
            "--auto-start=no",
            "call",
        ])
        .args(args)
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|err| err.to_string())?;
    value["data"]
        .as_array()
        .filter(|data| data.len() == 1)
        .map(|data| data[0].clone())
        .ok_or_else(|| "unexpected busctl response".into())
}

fn property<'a>(properties: &'a Value, key: &str, signature: &str) -> &'a Value {
    let value = &properties[key];
    if value["type"].as_str() == Some(signature) {
        &value["data"]
    } else {
        &Value::Null
    }
}

fn mac(value: &str) -> Option<String> {
    let value = value.replace('_', ":");
    (value.len() == 17
        && value.split(':').count() == 6
        && value
            .split(':')
            .all(|octet| octet.len() == 2 && octet.bytes().all(|digit| digit.is_ascii_hexdigit()))
        && value != "00:00:00:00:00:00")
        .then(|| value.to_ascii_lowercase())
}

fn sysfs_mac(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| mac(name.strip_prefix("hid-")?))
        .or_else(|| {
            path.ancestors()
                .take_while(|parent| *parent != Path::new("/sys/devices"))
                .find_map(|parent| {
                    let name = parent.file_name()?.to_str()?;
                    name.strip_prefix("dev_").and_then(mac).or_else(|| {
                        // Bluetooth HID devices expose the remote address as HID_UNIQ.
                        if name.starts_with("0005:") {
                            fs::read_to_string(parent.join("uevent"))
                                .ok()?
                                .lines()
                                .find_map(|line| mac(line.strip_prefix("HID_UNIQ=")?))
                        } else {
                            None
                        }
                    })
                })
        })
}

fn reading(key: String, name: Option<&str>, battery: BatteryStatus) -> Option<SourceReading> {
    if battery.percentage == BatteryPercentage::Unavailable
        && battery.level == BatteryLevel::Unknown
    {
        return None;
    }
    Some(SourceReading {
        id: if key.starts_with("hid:") {
            DeviceId::new(key.clone())
        } else {
            DeviceId::new(format!("linux:{key}"))
        },
        label: DeviceLabel::new(
            name.map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(&key),
        ),
        battery,
    })
}

fn percentage(value: Option<u8>) -> BatteryPercentage {
    value
        .filter(|value| *value <= 100)
        .map_or(BatteryPercentage::Unavailable, BatteryPercentage::Exact)
}

fn g502_usb_key(path: &Path) -> Option<String> {
    // The USB hid-logitech-hidpp driver publishes the four bytes of DEVICE
    // INFORMATION's unit ID as a hyphen-separated power_supply serial_number.
    let hid = path.parent()?.parent()?;
    let (hardware, _) = hid.file_name()?.to_str()?.split_once('.')?;
    if !hardware.eq_ignore_ascii_case("0003:046d:c095")
        || fs::read_link(hid.join("driver")).ok()?.file_name()? != "logitech-hidpp-device"
    {
        return None;
    }
    let serial = fs::read_to_string(path.join("serial_number")).ok()?;
    let serial = serial.trim();
    if serial.len() != 11 {
        return None;
    }
    let unit: [u8; 4] = serial
        .split('-')
        .map(|byte| u8::from_str_radix(byte, 16))
        .collect::<Result<Vec<_>, _>>()
        .ok()?
        .try_into()
        .ok()?;
    let id = volty_hid::G502XPlusUnitId::from_unit_bytes(unit)?;
    Some(format!("hid:g502-x-plus:{id}"))
}

fn hidpp_offline(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("hidpp_battery_"))
        && fs::read_to_string(path.join("online")).is_ok_and(|online| online.trim() == "0")
}

fn upower_reading(
    path: &str,
    props: &Value,
    now: u64,
) -> Option<(SourceReading, CaptureUpowerBattery)> {
    let kind = property(props, "Type", "u").as_u64()?;
    if matches!(kind, 0 | 1 | 3)
        || path.ends_with("/DisplayDevice")
        || property(props, "PowerSupply", "b").as_bool() != Some(false)
        || property(props, "IsPresent", "b").as_bool() == Some(false)
    {
        return None;
    }
    let updated = property(props, "UpdateTime", "t").as_u64()?;
    if updated == 0 || updated > now || now - updated > MAX_UPOWER_AGE {
        return None;
    }
    let native = property(props, "NativePath", "s")
        .as_str()
        .unwrap_or_default();
    let serial = property(props, "Serial", "s").as_str().unwrap_or_default();
    // UPower can expose a kernel power-supply basename or an absolute sysfs path.
    let native_path = Path::new(native);
    let canonical = if native_path.starts_with("/sys") {
        fs::canonicalize(native_path).ok()
    } else if native_path
        .file_name()
        .is_some_and(|name| name == native_path.as_os_str())
    {
        fs::canonicalize(Path::new("/sys/class/power_supply").join(native_path)).ok()
    } else {
        None
    };
    if canonical.as_deref().is_some_and(hidpp_offline) {
        return None;
    }
    let address = mac(serial)
        .or_else(|| {
            native
                .rsplit('/')
                .find_map(|part| mac(part.strip_prefix("dev_")?))
        })
        .or_else(|| canonical.as_deref().and_then(sysfs_mac));
    let key = if let Some(address) = address {
        format!("bluetooth:{address}")
    } else if let Some(native) = canonical.filter(|path| path.starts_with("/sys")) {
        g502_usb_key(&native).unwrap_or_else(|| format!("sysfs:{}", native.display()))
    } else {
        format!("upower:{path}")
    };
    let evidence = CaptureUpowerBattery {
        id: String::new(),
        t_ms: 0,
        percentage: property(props, "Percentage", "d").as_f64(),
        battery_level: property(props, "BatteryLevel", "u")
            .as_u64()
            .and_then(|value| u32::try_from(value).ok()),
        state: property(props, "State", "u")
            .as_u64()
            .and_then(|value| u32::try_from(value).ok()),
    };
    let battery = upower_battery(evidence.percentage, evidence.battery_level, evidence.state)?;
    let reading = reading(key, property(props, "Model", "s").as_str(), battery)?;
    Some((reading, evidence))
}

fn bluez_readings(
    objects: &Value,
    disconnected: &mut BTreeSet<String>,
) -> Vec<(SourceReading, u8)> {
    let mut readings = Vec::new();
    let mut connected = BTreeSet::new();
    for (path, interfaces) in objects.as_object().into_iter().flatten() {
        let device = &interfaces["org.bluez.Device1"];
        let Some(address) = property(device, "Address", "s")
            .as_str()
            .and_then(mac)
            .or_else(|| mac(path.rsplit('/').next()?.strip_prefix("dev_")?))
        else {
            continue;
        };
        let key = format!("linux:bluetooth:{address}");
        match property(device, "Connected", "b").as_bool() {
            Some(true) => {
                connected.insert(key);
            }
            Some(false) => {
                disconnected.insert(key);
                continue;
            }
            None => continue,
        }
        let exact = property(&interfaces["org.bluez.Battery1"], "Percentage", "y")
            .as_u64()
            .and_then(|value| u8::try_from(value).ok());
        let name = property(device, "Alias", "s")
            .as_str()
            .or_else(|| property(device, "Name", "s").as_str());
        if let Some((raw, battery)) = exact.and_then(|raw| Some((raw, bluez_battery(raw)?))) {
            if let Some(reading) = reading(format!("bluetooth:{address}"), name, battery) {
                readings.push((reading, raw));
            }
        }
    }
    disconnected.retain(|key| !connected.contains(key));
    readings
}

fn sysfs_reading(path: &Path) -> Option<SourceReading> {
    let canonical = fs::canonicalize(path).ok()?;
    let read = |name| {
        fs::read_to_string(path.join(name))
            .ok()
            .map(|s| s.trim().to_owned())
    };
    if read("type").as_deref() != Some("Battery")
        || read("scope").as_deref() != Some("Device")
        || read("present").is_some_and(|present| present != "1")
        || hidpp_offline(&canonical)
    {
        return None;
    }
    let key = sysfs_mac(&canonical).map_or_else(
        || g502_usb_key(&canonical).unwrap_or_else(|| format!("sysfs:{}", canonical.display())),
        |address| format!("bluetooth:{address}"),
    );
    let exact = read("capacity").and_then(|value| value.parse::<u8>().ok());
    let level = match read("capacity_level").as_deref() {
        Some("Critical") => BatteryLevel::Critical,
        Some("Low") => BatteryLevel::Low,
        Some("Normal" | "High") => BatteryLevel::Good,
        Some("Full") => BatteryLevel::Full,
        _ => BatteryLevel::Unknown,
    };
    reading(
        key,
        read("model_name")
            .as_deref()
            .or_else(|| path.file_name()?.to_str()),
        BatteryStatus {
            percentage: percentage(exact),
            level,
            raw_level: exact.unwrap_or_default(),
            state: match read("status").as_deref() {
                Some("Charging") => BatteryState::Charging,
                Some("Discharging") => BatteryState::Discharging,
                Some("Full") => BatteryState::Charged,
                _ => BatteryState::Unknown,
            },
            raw_status: None,
            voltage_mv: read("voltage_now")
                .and_then(|value| value.parse::<u64>().ok())
                .and_then(|value| u16::try_from(value / 1000).ok()),
            source: "Linux power_supply",
        },
    )
}

#[derive(Default)]
struct OsScan {
    upower: Vec<(SourceReading, CaptureUpowerBattery)>,
    bluez: Vec<(SourceReading, u8)>,
    disconnected: BTreeSet<String>,
    errors: Vec<(&'static str, String)>,
}

impl OsScan {
    /// Keep the same source in the tray and in replayable Bluetooth snapshots.
    fn reconcile(&mut self) {
        let bluez_ids: BTreeSet<_> = self.bluez.iter().map(|(r, _)| &r.id).collect();
        self.upower.retain(|(reading, _)| {
            !self.disconnected.contains(reading.id.as_str())
                && (reading.battery.percentage.exact().is_some()
                    || !bluez_ids.contains(&reading.id))
        });
        let upower_ids: BTreeSet<_> = self.upower.iter().map(|(r, _)| &r.id).collect();
        self.bluez
            .retain(|(reading, _)| !upower_ids.contains(&reading.id));
    }
}

fn scan_os(call: fn(&[&str]) -> Result<Value, String>) -> OsScan {
    let mut scan = OsScan::default();
    match call(&[UPOWER, UPOWER_ROOT, UPOWER, "EnumerateDevices"]) {
        Ok(paths) => {
            for path in paths
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                match call(&[
                    UPOWER,
                    path,
                    "org.freedesktop.DBus.Properties",
                    "GetAll",
                    "s",
                    UPOWER_DEVICE,
                ]) {
                    Ok(props) => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        scan.upower.extend(upower_reading(path, &props, now));
                    }
                    Err(err) => scan.errors.push((
                        "UPower device query failed",
                        format!("UPower {path}: {err}"),
                    )),
                }
            }
        }
        Err(err) => scan
            .errors
            .push(("UPower enumeration failed", format!("UPower: {err}"))),
    }
    match call(&[
        BLUEZ,
        "/",
        "org.freedesktop.DBus.ObjectManager",
        "GetManagedObjects",
    ]) {
        Ok(objects) => scan.bluez = bluez_readings(&objects, &mut scan.disconnected),
        Err(err) => scan
            .errors
            .push(("BlueZ discovery failed", format!("BlueZ: {err}"))),
    }
    scan
}

pub(crate) fn capture(
    writer: &mut CaptureWriter,
    t_ms: u64,
) -> Result<(), volty_capture::CaptureError> {
    for record in capture_records(scan_os(system_bus), t_ms) {
        writer.write(&record)?;
    }
    Ok(())
}

fn capture_records(mut scan: OsScan, t_ms: u64) -> Vec<CaptureRecord> {
    scan.reconcile();
    let mut records = Vec::new();
    let mut seen = BTreeSet::new();
    for (reading, raw) in scan.bluez {
        if seen.insert(reading.id) {
            let id = format!("bt-{}", seen.len());
            records.extend(device_records(
                &id,
                t_ms,
                CaptureBluetoothKind::Bluez,
                CaptureRecord::BluetoothProperty(CaptureBluetoothProperty {
                    id: id.clone(),
                    t_ms,
                    property: BLUEZ_PROPERTY.to_owned(),
                    property_type: "y".to_owned(),
                    raw_value: raw,
                }),
                &reading.battery,
            ));
        }
    }
    for (reading, mut evidence) in scan.upower {
        if reading.id.as_str().starts_with("linux:bluetooth:")
            && !scan.disconnected.contains(reading.id.as_str())
            && seen.insert(reading.id)
        {
            let id = format!("bt-{}", seen.len());
            evidence.id = id.clone();
            evidence.t_ms = t_ms;
            records.extend(device_records(
                &id,
                t_ms,
                CaptureBluetoothKind::Upower,
                CaptureRecord::UpowerBattery(evidence),
                &reading.battery,
            ));
        }
    }
    for (message, _) in scan.errors {
        records.push(CaptureRecord::Event(CaptureEvent {
            id: None,
            t_ms,
            level: EventLevel::Warning,
            message: message.to_owned(),
        }));
    }
    records
}

fn device_records(
    id: &str,
    t_ms: u64,
    kind: CaptureBluetoothKind,
    evidence: CaptureRecord,
    battery: &BatteryStatus,
) -> [CaptureRecord; 3] {
    [
        CaptureRecord::BluetoothDevice(CaptureBluetoothDevice {
            id: id.to_owned(),
            kind,
            name: None,
        }),
        evidence,
        CaptureRecord::Reading(CaptureReading {
            id: id.to_owned(),
            device_index: None,
            t_ms,
            backend: battery.source.to_owned(),
            label: None,
            battery: crate::capture::battery_snapshot(battery),
        }),
    ]
}

impl BatteryProvider for LinuxPower {
    fn poll(
        &mut self,
        mode: ScanMode,
        sink: &mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>,
    ) -> Result<ProviderPoll, SinkClosed> {
        let mut poll = ProviderPoll::default();
        let mut candidates = Vec::new();
        let mut scan = scan_os(self.call);
        scan.reconcile();
        let OsScan {
            upower,
            bluez,
            disconnected,
            errors,
        } = scan;
        candidates.extend(upower.into_iter().map(|(reading, _)| reading));
        candidates.extend(bluez.into_iter().map(|(reading, _)| reading));
        poll.errors
            .extend(errors.into_iter().map(|(_, detail)| detail));
        match fs::read_dir(&self.sysfs) {
            Ok(entries) => candidates.extend(
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| sysfs_reading(&entry.path())),
            ),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => poll.errors.push(format!("Linux power_supply: {err}")),
        }
        let mut unique = BTreeMap::new();
        for candidate in candidates {
            if !disconnected.contains(candidate.id.as_str()) {
                unique.entry(candidate.id.clone()).or_insert(candidate);
            }
        }
        for error in &poll.errors {
            if volty_battery::trace_enabled() {
                sink(ProviderEvent::Trace(error.clone()))?;
            }
        }
        for (id, reading) in unique {
            if mode == ScanMode::Discover || self.known.contains(&id) {
                self.known.insert(id);
                poll.recognised += 1;
                sink(ProviderEvent::Reading(reading))?;
                poll.readings += 1;
            }
        }
        Ok(poll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upower(now: u64, battery_level: u64) -> Value {
        json!({
            "Type": {"type":"u","data":5},
            "PowerSupply": {"type":"b","data":false},
            "UpdateTime": {"type":"t","data":now},
            "BatteryLevel": {"type":"u","data":battery_level},
            "Percentage": {"type":"d","data":55.4},
            "State": {"type":"u","data":1},
            "Serial": {"type":"s","data":"AA:BB:CC:DD:EE:FF"}
        })
    }

    #[test]
    fn upower_filters_system_stale_and_malformed_readings() {
        let mut props = upower(100, 3);
        let (reading, _) = upower_reading("/mouse", &props, 100).unwrap();
        assert_eq!(reading.battery.percentage, BatteryPercentage::Unavailable);
        assert_eq!(reading.battery.level, BatteryLevel::Low);
        assert_eq!(reading.battery.state, BatteryState::Charging);
        assert!(upower_reading("/mouse", &props, 100 + MAX_UPOWER_AGE + 1).is_none());

        props["BatteryLevel"]["data"] = json!(1);
        props["Percentage"]["data"] = json!(101);
        assert!(upower_reading("/mouse", &props, 100).is_none());
        props["Percentage"]["data"] = json!(0);
        assert_eq!(
            upower_reading("/mouse", &props, 100)
                .unwrap()
                .0
                .battery
                .percentage,
            BatteryPercentage::Exact(0)
        );
        props["PowerSupply"]["data"] = json!(true);
        assert!(upower_reading("/mouse", &props, 100).is_none());
    }

    fn mock_bus(args: &[&str]) -> Result<Value, String> {
        match args[3] {
            "EnumerateDevices" => Ok(json!(["/mouse"])),
            "GetAll" => Ok(upower(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                1,
            )),
            "GetManagedObjects" => Ok(json!({
                "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF": {
                    "org.bluez.Device1": {"Connected": {"type":"b","data":true}},
                    "org.bluez.Battery1": {"Percentage": {"type":"y","data":60}}
                },
                "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_00": {
                    "org.bluez.Device1": {"Connected": {"type":"b","data":false}},
                    "org.bluez.Battery1": {"Percentage": {"type":"y","data":100}}
                }
            })),
            _ => unreachable!(),
        }
    }

    #[test]
    fn os_capture_round_trips_and_omits_private_bus_data() {
        use volty_capture::{CaptureFile, CaptureHeader, CaptureMode, PrivacyProfile, SCHEMA};

        for privacy in [PrivacyProfile::Standard, PrivacyProfile::Strict] {
            let mut scan = scan_os(mock_bus);
            // Give the UPower observation a distinct test identity so both providers are recorded.
            scan.upower[0].0.id = DeviceId::new("linux:bluetooth:aa:bb:cc:dd:ee:01");
            scan.upower[0].0.label = DeviceLabel::new("Private headset name");
            scan.errors.push((
                "UPower device query failed",
                "/private/path Private headset name".into(),
            ));
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("os.volty.jsonl");
            let mut writer = CaptureWriter::create(&path).unwrap();
            writer
                .write(&CaptureRecord::Header(CaptureHeader {
                    schema: SCHEMA.into(),
                    capture_id: "test".into(),
                    created_at: "2026-11-28T00:00:00Z".into(),
                    app_version: "test".into(),
                    app_commit: None,
                    os: "linux".into(),
                    architecture: "x86_64".into(),
                    mode: CaptureMode::Snapshot,
                    privacy,
                    features: Vec::new(),
                }))
                .unwrap();
            for record in capture_records(scan, 10) {
                writer.write(&record).unwrap();
            }
            writer.finish(true).unwrap();

            let text = fs::read_to_string(&path).unwrap();
            for secret in [
                "aa:bb:",
                "AA:BB:",
                "AA_BB_",
                "/org/bluez",
                "/private/path",
                "Private headset name",
            ] {
                assert!(!text.contains(secret));
            }
            let file = CaptureFile::read(path).unwrap();
            let report = crate::replay::replay_capture(&file).unwrap();
            assert_eq!(report.bluetooth_readings, 2);

            let mut invalid = file.clone();
            for record in &mut invalid.records {
                if let CaptureRecord::BluetoothProperty(property) = record {
                    property.raw_value = 101;
                }
            }
            assert!(crate::replay::replay_capture(&invalid)
                .unwrap_err()
                .to_string()
                .contains("invalid BlueZ"));

            let mut invalid = file.clone();
            for record in &mut invalid.records {
                if let CaptureRecord::UpowerBattery(evidence) = record {
                    evidence.percentage = Some(101.0);
                }
            }
            assert!(crate::replay::replay_capture(&invalid)
                .unwrap_err()
                .to_string()
                .contains("invalid UPower"));

            for index in [2, 5] {
                let mut tampered = file.clone();
                let CaptureRecord::Reading(reading) = &mut tampered.records[index] else {
                    panic!("expected an OS battery reading");
                };
                reading.battery.raw_level = 0;
                assert!(crate::replay::replay_capture(&tampered)
                    .unwrap_err()
                    .to_string()
                    .contains("does not match"));
            }
        }
    }

    #[test]
    fn g502_identity_requires_the_kernel_driver_and_unit_serial() {
        let root = std::env::temp_dir().join(format!("volty-g502-{}", std::process::id()));
        let hid = root.join("0003:046D:C095.0001");
        let battery = hid.join("power_supply/hidpp_battery_0");
        fs::create_dir_all(&battery).unwrap();
        fs::write(battery.join("serial_number"), "01-02-03-04\n").unwrap();
        assert!(g502_usb_key(&battery).is_none());
        std::os::unix::fs::symlink(
            "/sys/bus/hid/drivers/logitech-hidpp-device",
            hid.join("driver"),
        )
        .unwrap();
        assert_eq!(
            g502_usb_key(&battery).as_deref(),
            Some("hid:g502-x-plus:be7a5e775165785d")
        );
        fs::write(battery.join("serial_number"), "00-00-00-00\n").unwrap();
        assert!(g502_usb_key(&battery).is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn poll_deduplicates_and_falls_back_to_sysfs_when_services_disappear() {
        let root = std::env::temp_dir().join(format!("volty-power-{}", std::process::id()));
        let mouse = root.join("hid-AA:BB:CC:DD:EE:FF");
        let offline = root.join("hid-AA:BB:CC:DD:EE:00");
        let system = root.join("BAT0");
        for (path, scope) in [
            (&mouse, "Device"),
            (&offline, "Device"),
            (&system, "System"),
        ] {
            fs::create_dir_all(path).unwrap();
            for (key, value) in [("type", "Battery"), ("scope", scope), ("capacity", "62")] {
                fs::write(path.join(key), value).unwrap();
            }
        }

        let mut backend = LinuxPower {
            call: mock_bus,
            sysfs: root.clone(),
            known: BTreeSet::new(),
        };
        let mut readings = Vec::new();
        let mut sink = |event| {
            if let ProviderEvent::Reading(reading) = event {
                readings.push(reading);
            }
            Ok(())
        };
        assert_eq!(
            backend
                .poll(ScanMode::Discover, &mut sink)
                .unwrap()
                .readings,
            1
        );
        backend.call = |_| Err("service unavailable".into());
        fs::remove_dir_all(offline).unwrap();
        let result = backend.poll(ScanMode::Refresh, &mut sink).unwrap();
        fs::remove_dir_all(root).unwrap();
        assert_eq!(result.errors.len(), 2);
        assert_eq!(result.readings, 1);
        assert_eq!(readings[0].id, readings[1].id);
        assert_eq!(readings[0].battery.source, "UPower");
        assert_eq!(readings[1].battery.source, "Linux power_supply");
    }
}
