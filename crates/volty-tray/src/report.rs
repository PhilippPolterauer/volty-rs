use std::{
    collections::VecDeque,
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use volty_hid::enumerate_hid_devices;

use crate::{capture, config::AppConfig};

/// The text report and the optional capture written by the Diagnose action.
pub struct DiagnosticExport {
    pub report_path: PathBuf,
    pub report: String,
    pub capture_path: Option<PathBuf>,
    pub capture_error: Option<String>,
}

pub fn generate_diagnostic_report(
    config: AppConfig,
    log: &VecDeque<String>,
) -> Result<DiagnosticExport, Box<dyn Error>> {
    let devices = enumerate_hid_devices()?;
    let app_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("could not determine the user application data folder")?;
    let report_dir = app_data.join("Volty").join("reports");
    fs::create_dir_all(&report_dir).map_err(|error| {
        format!(
            "could not create diagnostic report folder {}: {error}",
            report_dir.display()
        )
    })?;

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let report_path = report_dir.join(format!(
        "volty-diagnose-{}-{:09}.txt",
        timestamp.as_secs(),
        timestamp.subsec_nanos()
    ));

    let mut report = render_diagnostic_report(&devices, timestamp, config, log)?;
    let (capture_path, capture_error, capture_note) = match capture::write_default_snapshot() {
        Ok(capture) => (
            Some(capture.path.clone()),
            None,
            format!(
                "Saved: {}\nReview the capture before sharing it. Nothing is uploaded automatically.",
                capture.path.display()
            ),
        ),
        Err(error) => (None, Some(error.to_string()), format!("Not created: {error}")),
    };
    use std::fmt::Write as _;
    write!(
        report,
        "\n\n[Hardware capture]\n{capture_note}\n\
         The capture contains HID interface metadata, read-only protocol exchanges, and normalized readings.\n\
         It never contains device paths, serial numbers, usernames, computer names, or full input reports."
    )?;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report_path)
        .map_err(|error| {
            format!(
                "could not create diagnostic report {}: {error}",
                report_path.display()
            )
        })?;
    file.write_all(report.as_bytes()).map_err(|error| {
        format!(
            "could not write diagnostic report {}: {error}",
            report_path.display()
        )
    })?;
    Ok(DiagnosticExport {
        report_path,
        report,
        capture_path,
        capture_error,
    })
}

fn render_diagnostic_report(
    devices: &[volty_hid::HidDeviceInfo],
    timestamp: std::time::Duration,
    config: AppConfig,
    log: &VecDeque<String>,
) -> Result<String, fmt::Error> {
    let mut report = format!(
        "Volty Diagnose Log\n\
         Generated (Unix time): {}.{:09} UTC\n\
         Volty version: {}\n\
         Operating system: {} ({})\n\
         Architecture: {}\n\
         \n\
         [Configuration]\n\
         Low-battery threshold: {}%\n\
         Poll interval: {} seconds\n\
         Retry interval: {} seconds\n\
         Battery ETA window: {} minutes\n\
         Log level: {}\n\
         Low-battery notifications: {}\n\
         Tray color map: red <= {}% ({}), yellow <= {}% ({}), green > {}% ({})\n\
         \n\
         [Recent application log]\n\
         The log contains the latest {} events.\n",
        timestamp.as_secs(),
        timestamp.subsec_nanos(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::FAMILY,
        std::env::consts::ARCH,
        config.low_battery_threshold,
        config.poll_interval_seconds,
        config.retry_interval_seconds,
        config.battery_eta_window_minutes,
        config.log_level.as_str(),
        if config.notifications_enabled {
            "enabled"
        } else {
            "disabled"
        },
        config.battery_color_map.red_max_percent,
        format_color(config.battery_color_map.red),
        config.battery_color_map.yellow_max_percent,
        format_color(config.battery_color_map.yellow),
        config.battery_color_map.yellow_max_percent,
        format_color(config.battery_color_map.green),
        log.len()
    );

    use std::fmt::Write as _;
    for entry in log {
        writeln!(report, "{entry}")?;
    }
    let bluetooth_status = if cfg!(feature = "bluetooth") {
        "Bluetooth integration is enabled (opt-in build). Hardware validation is limited to one Bluetooth Classic headset; BLE GATT is not yet hardware-validated. Bluetooth device IDs and addresses are not included; Bluetooth read results and errors may appear in recent logs."
    } else {
        "Bluetooth integration is disabled in this build."
    };
    writeln!(
        report,
        "\n[Battery sources]\n\
         Logitech HID++ 2.0 is supported.\n\
         {bluetooth_status}\n\
         \n[HID interfaces visible to hidapi: {}]\n\
         Device paths and serial numbers are not included.\n\
         Review device names and log messages before sharing. Nothing is uploaded automatically.",
        devices.len()
    )?;
    for (index, device) in devices.iter().enumerate() {
        writeln!(
            report,
            "\n[HID interface {}]\n\
             Bus: {}\n\
             Vendor ID: 0x{:04x}\n\
             Product ID: 0x{:04x}\n\
             Release number: 0x{:04x}\n\
             Usage page: {}\n\
             Usage: {}\n\
             Interface number: {}\n\
             Manufacturer: {}\n\
             Product: {}",
            index + 1,
            device.bus_type,
            device.vendor_id,
            device.product_id,
            device.release_number,
            format_optional_hex(device.usage_page),
            format_optional_hex(device.usage),
            device.interface_number,
            format_optional_string(device.manufacturer_string.as_deref()),
            format_optional_string(device.product_string.as_deref()),
        )?;
    }
    Ok(report)
}

fn format_color(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn format_optional_hex(value: Option<u16>) -> String {
    value.map_or_else(
        || "unavailable".to_owned(),
        |value| format!("0x{value:04x}"),
    )
}

fn format_optional_string(value: Option<&str>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| format!("{value:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_includes_diagnostics_and_excludes_device_paths() {
        let device = volty_hid::HidDeviceInfo {
            vendor_id: 0x1234,
            product_id: 0xabcd,
            release_number: 0x0102,
            usage_page: Some(0x0001),
            usage: Some(0x0002),
            interface_number: 3,
            bus_type: "Bluetooth".to_owned(),
            manufacturer_string: Some("Example Maker".to_owned()),
            product_string: Some("Example Mouse".to_owned()),
        };
        let mut log = VecDeque::new();
        log.push_back("[+1s] Battery poll succeeded".to_owned());
        let report = render_diagnostic_report(
            &[device],
            std::time::Duration::new(1, 2),
            AppConfig::default(),
            &log,
        )
        .unwrap();

        assert!(report.contains("Volty Diagnose Log"));
        assert!(report.contains("Low-battery threshold: 20%"));
        assert!(report.contains("Battery ETA window: 10 minutes"));
        assert!(report.contains(
            "Tray color map: red <= 15% (#d23030), yellow <= 30% (#ebbe23), green > 30% (#2daa4b)"
        ));
        assert!(report.contains("[Battery sources]"));
        if cfg!(feature = "bluetooth") {
            assert!(report.contains(
                "Bluetooth integration is enabled (opt-in build). Hardware validation is limited to one Bluetooth Classic headset; BLE GATT is not yet hardware-validated."
            ));
        } else {
            assert!(report.contains("Bluetooth integration is disabled in this build."));
        }
        assert!(report.contains("[+1s] Battery poll succeeded"));
        assert!(report.contains("Bus: Bluetooth"));
        assert!(report.contains("Vendor ID: 0x1234"));
        assert!(report.contains("Product ID: 0xabcd"));
        assert!(report.contains("Product: \"Example Mouse\""));
        assert!(!report.contains("device-path"));
        assert!(!report.contains("serial-value"));
    }
}
