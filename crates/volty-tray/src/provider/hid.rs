use super::name_cache::NameCache;
use crate::state::DeviceState;
use volty_battery::{
    BatteryProvider, DeviceId, DeviceLabel, ProviderEvent, ProviderPoll, ScanMode, SinkClosed,
    SourceReading,
};
use volty_hid::{
    enumerate_hid_interfaces, resolve_hid_device, stable_interface_keys, BatteryProtocol,
    HidInterfaceInfo, HidppError, ProtocolDeviceReading,
};

type Sink<'a> = &'a mut dyn FnMut(ProviderEvent) -> Result<(), SinkClosed>;

const DIRECT_DEVICE_INDEX: u8 = 0xff;

/// HID battery source: enumerates interfaces, resolves them through the
/// `volty-hid` catalog, and probes receiver slots. Device identity is
/// path-independent, so cached state survives USB re-enumeration.
pub struct HidProvider {
    state: Option<DeviceState>,
    names: NameCache,
}

impl HidProvider {
    pub fn new(state: Option<DeviceState>) -> Self {
        Self {
            state,
            names: NameCache::default(),
        }
    }
}

pub(super) fn hid_device_id(interface_key: &str, device_index: u8) -> DeviceId {
    DeviceId::new(format!("hid:{interface_key}#{device_index}"))
}

fn slot_qualifier(device_index: u8) -> String {
    if device_index == DIRECT_DEVICE_INDEX {
        "direct".to_owned()
    } else {
        format!("slot {device_index}")
    }
}

impl BatteryProvider for HidProvider {
    fn poll(&mut self, mode: ScanMode, sink: Sink<'_>) -> Result<ProviderPoll, SinkClosed> {
        let mut outcome = ProviderPoll::default();
        let interfaces = match enumerate_hid_interfaces() {
            Ok(interfaces) => interfaces,
            Err(error) => {
                outcome.errors.push(format!("HID discovery: {error}"));
                Vec::new()
            }
        };
        let keys = stable_interface_keys(&interfaces);
        for (interface, key) in interfaces.iter().zip(&keys) {
            self.poll_interface(interface, key, mode, sink, &mut outcome)?;
        }
        Ok(outcome)
    }
}

impl HidProvider {
    fn poll_interface(
        &mut self,
        interface: &HidInterfaceInfo,
        key: &str,
        mode: ScanMode,
        sink: Sink<'_>,
        outcome: &mut ProviderPoll,
    ) -> Result<(), SinkClosed> {
        match resolve_hid_device(interface) {
            Ok(Some(device)) => {
                outcome.recognised += 1;
                let protocol = device.protocol();
                let fallback_name = interface
                    .product_string
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .map_or_else(|| device.name().to_owned(), str::to_owned);
                sink(ProviderEvent::Trace(format!(
                    "HID discovery matched {fallback_name:?} to {} using {} (VID=0x{:04x}, PID=0x{:04x}, usage_page={:?}, usage={:?}, interface={})",
                    device.name(),
                    protocol.name(),
                    interface.vendor_id,
                    interface.product_id,
                    interface.usage_page,
                    interface.usage,
                    interface.interface_number
                )))?;
                let (known_indices, device_indices) =
                    self.device_indices(key, protocol.device_indices(), mode, sink)?;
                for device_index in device_indices {
                    let probe = SlotProbe {
                        interface,
                        protocol,
                        fallback_name: &fallback_name,
                        key,
                        device_index,
                        was_known_slot: known_indices.contains(&device_index),
                        mode,
                    };
                    self.query_slot(&probe, sink, outcome)?;
                }
                Ok(())
            }
            Ok(None) => sink(ProviderEvent::Trace(format!(
                "HID discovery skipped an unsupported interface (VID=0x{:04x}, PID=0x{:04x}, usage_page={:?}, usage={:?}, interface={}): no supported HID protocol matched its interface metadata",
                interface.vendor_id,
                interface.product_id,
                interface.usage_page,
                interface.usage,
                interface.interface_number
            ))),
            Err(error) => {
                outcome.errors.push(format!(
                    "Ambiguous HID backend match for {:04x}:{:04x}: {error}",
                    interface.vendor_id, interface.product_id
                ));
                Ok(())
            }
        }
    }

    /// Returns the slots already known for this interface and the slots to probe.
    fn device_indices(
        &mut self,
        key: &str,
        candidates: &[u8],
        mode: ScanMode,
        sink: Sink<'_>,
    ) -> Result<(Vec<u8>, Vec<u8>), SinkClosed> {
        let mut known_indices = match self
            .state
            .as_ref()
            .map(|state| state.known_device_indices(key, candidates))
        {
            Some(Ok(indices)) => indices,
            Some(Err(error)) => {
                self.disable_state(
                    &format!(
                        "Could not read discovery state; continuing with a full scan: {error}"
                    ),
                    sink,
                )?;
                Vec::new()
            }
            None => Vec::new(),
        };
        for &device_index in candidates {
            if self.names.is_known(&hid_device_id(key, device_index))
                && !known_indices.contains(&device_index)
            {
                known_indices.push(device_index);
            }
        }

        let device_indices = if mode == ScanMode::Refresh && !known_indices.is_empty() {
            known_indices.clone()
        } else {
            match self
                .state
                .as_ref()
                .map(|state| state.ordered_device_indices(key, candidates))
            {
                Some(Ok(indices)) => indices,
                Some(Err(error)) => {
                    self.disable_state(
                        &format!(
                            "Could not read discovery state; continuing with a full scan: {error}"
                        ),
                        sink,
                    )?;
                    candidates.to_vec()
                }
                None => candidates.to_vec(),
            }
        };
        Ok((known_indices, device_indices))
    }

    fn disable_state(&mut self, warning: &str, sink: Sink<'_>) -> Result<(), SinkClosed> {
        self.state = None;
        sink(ProviderEvent::Warning(warning.to_owned()))
    }

    fn query_slot(
        &mut self,
        probe: &SlotProbe<'_>,
        sink: Sink<'_>,
        outcome: &mut ProviderPoll,
    ) -> Result<(), SinkClosed> {
        let id = hid_device_id(probe.key, probe.device_index);
        let cached_name = self.names.get(&id).map(str::to_owned);
        let was_known = cached_name.is_some() || probe.was_known_slot;
        let reading = match cached_name {
            Some(name) => probe
                .protocol
                .query_battery(probe.interface, probe.device_index)
                .map(|battery| (battery, name, None, false)),
            None => probe
                .protocol
                .query_device(probe.interface, probe.device_index)
                .map(|device| {
                    let ProtocolDeviceReading {
                        name: queried_name,
                        battery,
                    } = device;
                    let (name, name_error) =
                        self.names.resolve(id.clone(), probe.fallback_name, || {
                            queried_name.map_err(|error| error.to_string())
                        });
                    (battery, name, name_error, true)
                }),
        };

        match reading {
            Ok((battery, name, name_error, name_was_queried)) => {
                let newly_known = self.names.remember_device(id.clone());
                let label =
                    DeviceLabel::new(name).with_qualifier(slot_qualifier(probe.device_index));
                sink(ProviderEvent::Reading(SourceReading {
                    id: id.clone(),
                    label: label.clone(),
                    battery,
                }))?;
                outcome.readings += 1;
                // Recency only steers scan order, so persist it once per session instead of every poll.
                let state_error = self
                    .state
                    .as_mut()
                    .filter(|_| newly_known)
                    .and_then(|state| state.record(probe.key, probe.device_index).err());
                if let Some(error) = state_error {
                    self.disable_state(
                        &format!(
                            "Could not update discovery state; continuing with full scans: {error}"
                        ),
                        sink,
                    )?;
                }
                if name_was_queried {
                    sink(ProviderEvent::DeviceName {
                        id,
                        label,
                        error: name_error,
                    })?;
                }
            }
            Err(error) => {
                self.names.forget(&id);
                let empty_discovery_slot = probe.mode == ScanMode::Discover
                    && !was_known
                    && matches!(
                        error.downcast_ref::<HidppError>(),
                        Some(HidppError::Timeout)
                    );
                if empty_discovery_slot {
                    sink(ProviderEvent::Trace(format!(
                        "HID++ discovery address {} did not respond",
                        probe.device_index
                    )))?;
                } else {
                    outcome.errors.push(format!(
                        "{} via {} (slot {}): {error}",
                        probe.fallback_name,
                        probe.protocol.name(),
                        probe.device_index
                    ));
                }
            }
        }
        Ok(())
    }
}

struct SlotProbe<'a> {
    interface: &'a HidInterfaceInfo,
    protocol: &'a dyn BatteryProtocol,
    fallback_name: &'a str,
    key: &'a str,
    device_index: u8,
    was_known_slot: bool,
    mode: ScanMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_are_source_tagged_and_slot_specific() {
        let id = hid_device_id("046d:c547:2:ff00:0002:-", 1);
        assert_eq!(id.as_str(), "hid:046d:c547:2:ff00:0002:-#1");
        assert_ne!(id, hid_device_id("046d:c547:2:ff00:0002:-", 2));
        assert_ne!(id, DeviceId::new("bluetooth:046d:c547:2:ff00:0002:-#1"));
    }

    #[test]
    fn slot_qualifier_names_direct_connections() {
        assert_eq!(slot_qualifier(0xff), "direct");
        assert_eq!(slot_qualifier(3), "slot 3");
    }
}
