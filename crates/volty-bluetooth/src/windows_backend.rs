mod ble;
mod budget;
mod classic;
mod setupapi;
mod winrt;

use crate::{BluetoothError, BluetoothScan};
use budget::{push_scan_budget_warning, ScanBudget, BLUETOOTH_SCAN_BUDGET};

pub(super) const WINDOWS_BATTERY_LIFE_PROPERTY: &str = "System.Devices.BatteryLife";

pub(super) fn scan_battery_devices() -> Result<BluetoothScan, BluetoothError> {
    let budget = ScanBudget::new(BLUETOOTH_SCAN_BUDGET);
    let _apartment = winrt::WinRtApartment::initialize()?;
    let mut scan = BluetoothScan::default();
    run_scan_phase(
        &mut scan,
        &budget,
        "Windows Bluetooth battery property",
        classic::scan_windows_battery_property,
    );
    run_scan_phase(
        &mut scan,
        &budget,
        "Windows Bluetooth device-property",
        setupapi::scan_setupapi_battery_property,
    );
    run_scan_phase(
        &mut scan,
        &budget,
        "Bluetooth GATT discovery",
        ble::scan_paired_ble_devices,
    );
    Ok(scan)
}

fn run_scan_phase(
    target: &mut BluetoothScan,
    budget: &ScanBudget,
    phase: &str,
    scan_phase: impl FnOnce(&ScanBudget) -> Result<BluetoothScan, BluetoothError>,
) {
    if budget.is_exhausted() {
        push_scan_budget_warning(target, phase);
        return;
    }
    match scan_phase(budget) {
        Ok(source) => merge_scan(target, source),
        Err(_) if budget.is_exhausted() => push_scan_budget_warning(target, phase),
        Err(error) => target.warnings.push(format!("{phase}: {error}")),
    }
}

fn merge_scan(target: &mut BluetoothScan, mut source: BluetoothScan) {
    target.devices_examined += source.devices_examined;
    for reading in source.readings.drain(..) {
        if !target
            .readings
            .iter()
            .any(|existing| existing.device_id == reading.device_id)
        {
            target.readings.push(reading);
        }
    }
    target.discovery_events.append(&mut source.discovery_events);
    target.warnings.append(&mut source.warnings);
}

#[cfg(test)]
mod tests {
    use super::merge_scan;
    use crate::{battery_status, BluetoothBatteryReading, BluetoothScan};
    use volty_battery::BatterySource;

    fn reading(device_id: &str, level: u8) -> BluetoothBatteryReading {
        BluetoothBatteryReading {
            device_id: device_id.to_owned(),
            name: format!("device-{device_id}"),
            battery: battery_status(level, BatterySource::BluetoothGatt),
        }
    }

    #[test]
    fn merge_scan_keeps_first_reading_and_appends_scan_metadata() {
        let mut target = BluetoothScan {
            readings: vec![reading("same", 20)],
            warnings: vec!["first warning".to_owned()],
            discovery_events: vec!["first event".to_owned()],
            devices_examined: 2,
        };
        let source = BluetoothScan {
            readings: vec![reading("same", 80), reading("new", 60)],
            warnings: vec!["second warning".to_owned()],
            discovery_events: vec!["second event".to_owned()],
            devices_examined: 3,
        };

        merge_scan(&mut target, source);

        assert_eq!(target.devices_examined, 5);
        assert_eq!(target.readings.len(), 2);
        assert_eq!(target.readings[0].battery.level_percent, Some(20));
        assert_eq!(target.readings[1].device_id, "new");
        assert_eq!(target.warnings, ["first warning", "second warning"]);
        assert_eq!(target.discovery_events, ["first event", "second event"]);
    }
}
