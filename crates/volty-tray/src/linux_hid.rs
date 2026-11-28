use std::{
    collections::{BTreeMap, HashMap},
    ffi::CString,
};

use volty_battery::{
    BatteryProvider, DeviceId, DeviceLabel, ProviderEvent, ProviderPoll, ScanMode, SinkClosed,
    SourceReading,
};
use volty_hid::{
    enumerate_hid_interfaces, is_direct_only_hidpp, stable_interface_keys, BatteryProtocol,
    LOGITECH_HIDPP_20,
};

#[derive(Default)]
struct LinuxHid {
    known: HashMap<(String, u8), KnownDevice>,
}

struct KnownDevice {
    path: CString,
    id: DeviceId,
    label: DeviceLabel,
}

pub(crate) fn provider() -> Box<dyn BatteryProvider> {
    Box::<LinuxHid>::default()
}

impl BatteryProvider for LinuxHid {
    fn poll(
        &mut self,
        mode: ScanMode,
        sink: &mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>,
    ) -> Result<ProviderPoll, SinkClosed> {
        let mut outcome = ProviderPoll::default();
        let mut interfaces = match enumerate_hid_interfaces() {
            Ok(interfaces) => interfaces,
            Err(error) => {
                let message = format!("Linux HID enumeration failed: {error}");
                sink(ProviderEvent::Warning(message.clone()))?;
                outcome.errors.push(message);
                return Ok(outcome);
            }
        };
        interfaces.retain(|interface| LOGITECH_HIDPP_20.supports_interface(interface));
        let keys = stable_interface_keys(&interfaces);
        self.known.retain(|(key, _), device| {
            keys.iter()
                .zip(&interfaces)
                .any(|(present, interface)| key == present && device.path == interface.path)
        });

        let mut readings = BTreeMap::new();
        for (key, interface) in keys.into_iter().zip(&interfaces) {
            let wired = is_direct_only_hidpp(interface.vendor_id, interface.product_id);
            for &slot in LOGITECH_HIDPP_20.device_indices(interface) {
                let candidate = (key.clone(), slot);
                let known = self.known.get(&candidate);
                if mode == ScanMode::Refresh && known.is_none() {
                    continue;
                }
                outcome.recognised += 1;
                let result = if let (ScanMode::Refresh, Some(device)) = (mode, known) {
                    LOGITECH_HIDPP_20
                        .query_battery(interface, slot)
                        .map(|battery| SourceReading {
                            id: device.id.clone(),
                            label: device.label.clone(),
                            battery,
                        })
                } else {
                    LOGITECH_HIDPP_20
                        .query_device(interface, slot)
                        .map(|device| {
                            let (id, label) = if let Some(unit) = device.g502_x_plus_unit_id {
                                (
                                    DeviceId::new(format!("hid:g502-x-plus:{unit}")),
                                    DeviceLabel::new("G502 X PLUS"),
                                )
                            } else {
                                let name = device
                                    .name
                                    .ok()
                                    .flatten()
                                    .or_else(|| interface.product_string.clone())
                                    .unwrap_or_else(|| "Logitech HID++".to_owned());
                                let label = DeviceLabel::new(name);
                                let label = if slot == 0xff {
                                    label
                                } else {
                                    label.with_qualifier(format!("slot {slot}"))
                                };
                                (DeviceId::new(format!("hid:{key}#{slot}")), label)
                            };
                            self.known.insert(
                                candidate,
                                KnownDevice {
                                    path: interface.path.clone(),
                                    id: id.clone(),
                                    label: label.clone(),
                                },
                            );
                            SourceReading {
                                id,
                                label,
                                battery: device.battery,
                            }
                        })
                };
                match result {
                    Ok(reading) if wired || !readings.contains_key(&reading.id) => {
                        readings.insert(reading.id.clone(), reading);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if matches!(
                            error.downcast_ref::<volty_hid::HidppError>(),
                            Some(volty_hid::HidppError::HidApi(_))
                        ) {
                            let message = format!(
                                "HID++ {:04x}:{:04x}: {error}",
                                interface.vendor_id, interface.product_id
                            );
                            sink(ProviderEvent::Warning(message.clone()))?;
                            outcome.errors.push(message);
                            break;
                        }
                    }
                }
            }
        }
        for reading in readings.into_values() {
            sink(ProviderEvent::Reading(reading))?;
            outcome.readings += 1;
        }
        Ok(outcome)
    }
}
