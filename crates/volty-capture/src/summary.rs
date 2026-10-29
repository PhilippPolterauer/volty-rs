use crate::{
    CaptureBluetoothDevice, CaptureFile, CaptureInterface, CaptureMode, CaptureReading,
    CaptureRecord, PrivacyProfile,
};

const MAX_LISTED_INTERFACES: usize = 20;
const MAX_LISTED_BLUETOOTH_DEVICES: usize = 20;
const MAX_LISTED_READINGS: usize = 40;

#[derive(Default)]
struct RecordCounts {
    interfaces: usize,
    matches: usize,
    bluetooth_devices: usize,
    bluetooth_properties: usize,
    bluetooth_gatt: usize,
    hidpp_features: usize,
    hid_layouts: usize,
    hidpp_exchanges: usize,
    hid_samples: usize,
    readings: usize,
    events: usize,
    markers: usize,
}

impl RecordCounts {
    fn collect(records: &[CaptureRecord]) -> Self {
        let mut counts = Self::default();
        for record in records {
            match record {
                CaptureRecord::Interface(_) => counts.interfaces += 1,
                CaptureRecord::Match(_) => counts.matches += 1,
                CaptureRecord::BluetoothDevice(_) => counts.bluetooth_devices += 1,
                CaptureRecord::BluetoothProperty(_) => counts.bluetooth_properties += 1,
                CaptureRecord::BluetoothGatt(_) => counts.bluetooth_gatt += 1,
                CaptureRecord::HidppFeatures(_) => counts.hidpp_features += 1,
                CaptureRecord::HidLayout(_) => counts.hid_layouts += 1,
                CaptureRecord::HidppExchange(_) => counts.hidpp_exchanges += 1,
                CaptureRecord::HidSample(_) => counts.hid_samples += 1,
                CaptureRecord::Reading(_) => counts.readings += 1,
                CaptureRecord::Event(_) => counts.events += 1,
                CaptureRecord::Marker(_) => counts.markers += 1,
                CaptureRecord::Header(_) | CaptureRecord::Footer(_) => {}
            }
        }
        counts
    }
}

impl CaptureFile {
    /// Renders a concise, privacy-safe overview of the capture.
    ///
    /// The summary is bounded to roughly 80 lines; long captures list a prefix
    /// of their devices and readings.
    pub fn render_summary(&self) -> String {
        let counts = RecordCounts::collect(&self.records);

        let mut lines = vec![
            format!("schema: {}", self.header.schema),
            format!("capture_id: {}", self.header.capture_id),
            format!("app_version: {}", self.header.app_version),
            format!("os: {}", self.header.os),
            format!("architecture: {}", self.header.architecture),
            format!("mode: {}", mode_name(self.header.mode)),
            format!("privacy: {}", privacy_name(self.header.privacy)),
            format!("features: {}", join_or_dash(&self.header.features)),
            format!("records: {}", self.footer.records),
            format!("data_records: {}", self.records.len()),
            format!("interfaces: {}", counts.interfaces),
            format!("matches: {}", counts.matches),
            format!("bluetooth_devices: {}", counts.bluetooth_devices),
            format!("bluetooth_properties: {}", counts.bluetooth_properties),
            format!("bluetooth_gatt: {}", counts.bluetooth_gatt),
            format!("hidpp_features: {}", counts.hidpp_features),
            format!("hid_layouts: {}", counts.hid_layouts),
            format!("hidpp_exchanges: {}", counts.hidpp_exchanges),
            format!("hid_samples: {}", counts.hid_samples),
            format!("readings: {}", counts.readings),
            format!("events: {}", counts.events),
            format!("markers: {}", counts.markers),
        ];

        if counts.interfaces > 0 {
            lines.push("interface details:".to_owned());
            let mut listed = 0;
            for record in &self.records {
                if let CaptureRecord::Interface(interface) = record {
                    if listed == MAX_LISTED_INTERFACES {
                        break;
                    }
                    lines.push(render_interface(interface));
                    listed += 1;
                }
            }
            if counts.interfaces > listed {
                lines.push(format!(
                    "  ... {} more interfaces",
                    counts.interfaces - listed
                ));
            }
        }

        if counts.bluetooth_devices > 0 {
            lines.push("bluetooth device details:".to_owned());
            let mut listed = 0;
            for record in &self.records {
                if let CaptureRecord::BluetoothDevice(device) = record {
                    if listed == MAX_LISTED_BLUETOOTH_DEVICES {
                        break;
                    }
                    lines.push(render_bluetooth_device(device));
                    listed += 1;
                }
            }
            if counts.bluetooth_devices > listed {
                lines.push(format!(
                    "  ... {} more bluetooth devices",
                    counts.bluetooth_devices - listed
                ));
            }
        }

        if counts.readings > 0 {
            lines.push("reading details:".to_owned());
            let mut listed = 0;
            for record in &self.records {
                if let CaptureRecord::Reading(reading) = record {
                    if listed == MAX_LISTED_READINGS {
                        break;
                    }
                    lines.push(render_reading(reading));
                    listed += 1;
                }
            }
            if counts.readings > listed {
                lines.push(format!("  ... {} more readings", counts.readings - listed));
            }
        }

        lines.push(format!("footer: complete={}", self.footer.complete));
        lines.join("\n")
    }
}

fn render_interface(interface: &CaptureInterface) -> String {
    let usage = match (interface.usage_page, interface.usage) {
        (Some(page), Some(usage)) => format!("{page:04x}:{usage:04x}"),
        _ => "-".to_owned(),
    };
    format!(
        "  - {}: bus={} vid={:04x} pid={:04x} release={:04x} interface={} usage={} manufacturer={} product={} serial_present={}",
        interface.id,
        interface.bus,
        interface.vendor_id,
        interface.product_id,
        interface.release_number,
        interface.interface_number,
        usage,
        interface.manufacturer.as_deref().unwrap_or("-"),
        interface.product.as_deref().unwrap_or("-"),
        interface.serial_present,
    )
}

fn render_bluetooth_device(device: &CaptureBluetoothDevice) -> String {
    format!(
        "  - {}: kind={} name={}",
        device.id,
        bluetooth_kind_name(device.kind),
        device.name.as_deref().unwrap_or("-"),
    )
}

fn render_reading(reading: &CaptureReading) -> String {
    let percent = reading
        .battery
        .percent
        .map_or_else(|| "-".to_owned(), |percent| format!("{percent}%"));
    let device = reading
        .device_index
        .map_or_else(|| "-".to_owned(), |index| index.to_string());
    let mut line = format!(
        "  - {}: device={} t={}ms backend={} battery={} level={} state={} source={}",
        reading.id,
        device,
        reading.t_ms,
        reading.backend,
        percent,
        reading.battery.level,
        reading.battery.state,
        reading.battery.source,
    );
    if let Some(label) = &reading.label {
        line.push_str(" label=");
        line.push_str(label);
    }
    line
}

fn mode_name(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Snapshot => "snapshot",
        CaptureMode::Session => "session",
    }
}

fn privacy_name(privacy: PrivacyProfile) -> &'static str {
    match privacy {
        PrivacyProfile::Standard => "standard",
        PrivacyProfile::Strict => "strict",
    }
}

fn bluetooth_kind_name(kind: crate::CaptureBluetoothKind) -> &'static str {
    match kind {
        crate::CaptureBluetoothKind::Classic => "classic",
        crate::CaptureBluetoothKind::Pnp => "pnp",
        crate::CaptureBluetoothKind::Ble => "ble",
    }
}

fn join_or_dash(features: &[String]) -> String {
    if features.is_empty() {
        "-".to_owned()
    } else {
        features.join(",")
    }
}
