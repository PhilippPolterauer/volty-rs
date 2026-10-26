use std::error::Error;

use volty_hid::{enumerate_logitech_devices, query_battery};

fn main() -> Result<(), Box<dyn Error>> {
    let mut devices = enumerate_logitech_devices()?;
    devices.sort_by_key(|device| {
        !device
            .product_string
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().contains("g502"))
    });

    if devices.is_empty() {
        return Err("no Logitech HID interfaces were enumerated".into());
    }

    for device in devices {
        println!(
            "Device: {} (VID {:04x}, PID {:04x}, usage page {:04x}, usage {:04x}, interface {})",
            device.product_string.as_deref().unwrap_or("unknown"),
            device.vendor_id,
            device.product_id,
            device.usage_page,
            device.usage,
            device.interface_number
        );
        for device_index in [0xff, 1, 2, 3, 4, 5, 6] {
            match query_battery(&device, device_index) {
                Ok(battery) => {
                    println!(
                        "Battery: {:?}% / {:?}, state {:?}, source {:?}, device index 0x{device_index:02x}",
                        battery.level_percent,
                        battery.level,
                        battery.state,
                        battery.source
                    );
                    return Ok(());
                }
                Err(error) => println!("  device index 0x{device_index:02x}: {error}"),
            }
        }
    }

    Err("no Logitech battery feature responded at the probed device indices".into())
}
