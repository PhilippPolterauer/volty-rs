use super::*;
#[derive(Default)]
pub(super) struct DeviceNameCache {
    names: HashMap<DeviceIdentity, String>,
    known_devices: HashSet<DeviceIdentity>,
}
impl DeviceNameCache {
    pub(super) fn get(&self, identity: &DeviceIdentity) -> Option<&str> {
        self.names.get(identity).map(String::as_str)
    }

    pub(super) fn resolve(
        &mut self,
        identity: DeviceIdentity,
        fallback_name: &str,
        query_name: impl FnOnce() -> Result<Option<String>, String>,
    ) -> (String, Option<String>) {
        if let Some(name) = self.names.get(&identity) {
            return (name.clone(), None);
        }

        let (name, error) = match query_name() {
            Ok(Some(name)) if !name.trim().is_empty() => (name.trim().to_owned(), None),
            Ok(_) => (fallback_name.to_owned(), None),
            Err(error) => (fallback_name.to_owned(), Some(error)),
        };
        self.names.insert(identity, name.clone());
        (name, error)
    }

    pub(super) fn forget(&mut self, identity: &DeviceIdentity) {
        self.names.remove(identity);
    }

    pub(super) fn remember_device(&mut self, identity: DeviceIdentity) {
        self.known_devices.insert(identity);
    }

    pub(super) fn known_device_indices(&self, interface_path: &str, candidates: &[u8]) -> Vec<u8> {
        candidates
            .iter()
            .copied()
            .filter(|device_index| {
                self.known_devices.contains(&DeviceIdentity::Hid {
                    interface_path: interface_path.to_owned(),
                    device_index: *device_index,
                })
            })
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::super::labels::format_device_name;
    use super::super::DeviceIdentity;
    use super::*;
    #[test]
    fn device_names_are_requested_once_until_the_device_disconnects() {
        let identity = DeviceIdentity::Hid {
            interface_path: "receiver".to_owned(),
            device_index: 1,
        };
        let mut cache = DeviceNameCache::default();
        let mut query_count = 0;

        let first = cache.resolve(identity.clone(), "Interface label", || {
            query_count += 1;
            Ok(Some("  Mouse Name  ".to_owned()))
        });
        let second = cache.resolve(identity.clone(), "Changed interface label", || {
            query_count += 1;
            Ok(Some("Unexpected second name".to_owned()))
        });

        assert_eq!(first, ("Mouse Name".to_owned(), None));
        assert_eq!(second, ("Mouse Name".to_owned(), None));
        assert_eq!(query_count, 1);

        cache.forget(&identity);
        let after_disconnect = cache.resolve(identity, "New interface label", || {
            query_count += 1;
            Ok(Some("Replacement mouse".to_owned()))
        });

        assert_eq!(after_disconnect, ("Replacement mouse".to_owned(), None));
        assert_eq!(query_count, 2);
    }
    #[test]
    fn failed_device_name_lookup_is_cached_until_disconnect() {
        let identity = DeviceIdentity::Hid {
            interface_path: "receiver".to_owned(),
            device_index: 2,
        };
        let mut cache = DeviceNameCache::default();
        let mut query_count = 0;

        let first = cache.resolve(identity.clone(), "Interface label", || {
            query_count += 1;
            Err("name request timed out".to_owned())
        });
        let second = cache.resolve(identity, "Changed interface label", || {
            query_count += 1;
            Ok(Some("Unexpected retry".to_owned()))
        });

        assert_eq!(
            first,
            (
                "Interface label".to_owned(),
                Some("name request timed out".to_owned())
            )
        );
        assert_eq!(second, ("Interface label".to_owned(), None));
        assert_eq!(query_count, 1);
    }
    #[test]
    #[cfg(feature = "bluetooth")]
    fn bluetooth_device_names_are_not_formatted_as_hid_slots() {
        let identity = DeviceIdentity::Bluetooth {
            device_id: "opaque-device-id".to_owned(),
        };

        assert_eq!(
            identity.display_name("Baseus Bass BS2 Lite"),
            "Baseus Bass BS2 Lite"
        );
        assert_eq!(
            format_device_name("Receiver mouse", Some(3)),
            "Receiver mouse (slot 3)"
        );
    }
    #[test]
    #[cfg(feature = "bluetooth")]
    fn bluetooth_identity_does_not_collide_with_hid_identity() {
        let identities = std::collections::HashSet::from([
            DeviceIdentity::Hid {
                interface_path: "same-opaque-id".to_owned(),
                device_index: 1,
            },
            DeviceIdentity::Bluetooth {
                device_id: "same-opaque-id".to_owned(),
            },
        ]);

        assert_eq!(identities.len(), 2);
    }
    #[test]
    fn device_monitor_identity_is_stable_and_source_specific() {
        let hid = DeviceIdentity::Hid {
            interface_path: "receiver-path".to_owned(),
            device_index: 1,
        };
        assert_eq!(hid.config_id(), "hid:1:receiver-path");
        assert_eq!(hid.config_id(), hid.clone().config_id());

        #[cfg(feature = "bluetooth")]
        let bluetooth = DeviceIdentity::Bluetooth {
            device_id: "receiver-path".to_owned(),
        };

        #[cfg(feature = "bluetooth")]
        assert_eq!(bluetooth.config_id(), "bluetooth:receiver-path");
        #[cfg(feature = "bluetooth")]
        assert_ne!(hid.config_id(), bluetooth.config_id());
    }
}
