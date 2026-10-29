use crate::{
    CaptureFile, CaptureInterface, CaptureMode, CaptureReading, CaptureRecord, PrivacyProfile,
};

const MAX_LISTED_INTERFACES: usize = 20;
const MAX_LISTED_READINGS: usize = 40;

impl CaptureFile {
    /// Renders a concise, privacy-safe overview of the capture.
    ///
    /// The summary is bounded to roughly 80 lines; long captures list a prefix
    /// of their interfaces and readings.
    pub fn render_summary(&self) -> String {
        let mut counts = [0usize; 10];
        for record in &self.records {
            let slot = match record {
                CaptureRecord::Interface(_) => Some(0),
                CaptureRecord::Match(_) => Some(1),
                CaptureRecord::HidppFeatures(_) => Some(2),
                CaptureRecord::HidLayout(_) => Some(3),
                CaptureRecord::HidppExchange(_) => Some(4),
                CaptureRecord::HidSample(_) => Some(5),
                CaptureRecord::Reading(_) => Some(6),
                CaptureRecord::Event(_) => Some(7),
                CaptureRecord::Marker(_) => Some(8),
                CaptureRecord::Header(_) | CaptureRecord::Footer(_) => None,
            };
            if let Some(slot) = slot {
                counts[slot] += 1;
            }
        }
        counts[9] = self.records.len();

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
            format!("data_records: {}", counts[9]),
            format!("interfaces: {}", counts[0]),
            format!("matches: {}", counts[1]),
            format!("hidpp_features: {}", counts[2]),
            format!("hid_layouts: {}", counts[3]),
            format!("hidpp_exchanges: {}", counts[4]),
            format!("hid_samples: {}", counts[5]),
            format!("readings: {}", counts[6]),
            format!("events: {}", counts[7]),
            format!("markers: {}", counts[8]),
        ];

        if counts[0] > 0 {
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
            if counts[0] > listed {
                lines.push(format!("  ... {} more interfaces", counts[0] - listed));
            }
        }

        if counts[6] > 0 {
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
            if counts[6] > listed {
                lines.push(format!("  ... {} more readings", counts[6] - listed));
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

fn render_reading(reading: &CaptureReading) -> String {
    let percent = reading
        .battery
        .percent
        .map_or_else(|| "-".to_owned(), |percent| format!("{percent}%"));
    let mut line = format!(
        "  - {}: device={} t={}ms backend={} battery={} level={} state={} source={}",
        reading.id,
        reading.device_index,
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

fn join_or_dash(features: &[String]) -> String {
    if features.is_empty() {
        "-".to_owned()
    } else {
        features.join(",")
    }
}
