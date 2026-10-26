use std::{
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use volty_hid::enumerate_hid_devices;

pub fn generate_debug_report() -> Result<PathBuf, Box<dyn Error>> {
    let devices = enumerate_hid_devices()?;
    let app_data = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("could not determine the user application data folder")?;
    let report_dir = app_data.join("Volty").join("reports");
    fs::create_dir_all(&report_dir).map_err(|error| {
        format!(
            "could not create debug report folder {}: {error}",
            report_dir.display()
        )
    })?;

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let report_path = report_dir.join(format!(
        "volty-debug-report-{}-{:09}.txt",
        timestamp.as_secs(),
        timestamp.subsec_nanos()
    ));

    let report = render_debug_report(&devices, timestamp)?;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report_path)
        .map_err(|error| {
            format!(
                "could not create debug report {}: {error}",
                report_path.display()
            )
        })?;
    file.write_all(report.as_bytes()).map_err(|error| {
        format!(
            "could not write debug report {}: {error}",
            report_path.display()
        )
    })?;
    Ok(report_path)
}

fn render_debug_report(
    devices: &[volty_hid::HidDeviceInfo],
    timestamp: std::time::Duration,
) -> Result<String, fmt::Error> {
    let mut report = format!(
        "Volty Debug Report\n\
         Generated (Unix time): {}.{:09} UTC\n\
         Volty version: {}\n\
         Operating system: {} ({})\n\
         Architecture: {}\n\
         \n\
         HID interfaces visible to hidapi: {}\n\
         Device paths, serial numbers, and user or computer names are not included.\n\
         This report is saved locally; Volty does not upload it.\n",
        timestamp.as_secs(),
        timestamp.subsec_nanos(),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::FAMILY,
        std::env::consts::ARCH,
        devices.len()
    );

    use std::fmt::Write as _;
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
    fn report_includes_generic_hid_devices_and_excludes_identifiers() {
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
        let report = render_debug_report(&[device], std::time::Duration::new(1, 2)).unwrap();

        assert!(report.contains("Bus: Bluetooth"));
        assert!(report.contains("Vendor ID: 0x1234"));
        assert!(report.contains("Product ID: 0xabcd"));
        assert!(report.contains("Product: \"Example Mouse\""));
        assert!(!report.contains("device-path"));
        assert!(!report.contains("serial-value"));
    }
}
