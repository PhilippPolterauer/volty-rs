use std::collections::{HashMap, HashSet};
use volty_battery::DeviceId;

/// Per-session memory of resolved device names and devices seen answering, so
/// timed polls skip name queries and unknown addresses.
#[derive(Default)]
pub(super) struct NameCache {
    names: HashMap<DeviceId, String>,
    known_devices: HashSet<DeviceId>,
}

impl NameCache {
    pub(super) fn get(&self, id: &DeviceId) -> Option<&str> {
        self.names.get(id).map(String::as_str)
    }

    pub(super) fn resolve(
        &mut self,
        id: DeviceId,
        fallback_name: &str,
        query_name: impl FnOnce() -> Result<Option<String>, String>,
    ) -> (String, Option<String>) {
        if let Some(name) = self.names.get(&id) {
            return (name.clone(), None);
        }

        let (name, error) = match query_name() {
            Ok(Some(name)) if !name.trim().is_empty() => (name.trim().to_owned(), None),
            Ok(_) => (fallback_name.to_owned(), None),
            Err(error) => (fallback_name.to_owned(), Some(error)),
        };
        self.names.insert(id, name.clone());
        (name, error)
    }

    pub(super) fn forget(&mut self, id: &DeviceId) {
        self.names.remove(id);
    }

    pub(super) fn remember_device(&mut self, id: DeviceId) -> bool {
        self.known_devices.insert(id)
    }

    pub(super) fn is_known(&self, id: &DeviceId) -> bool {
        self.known_devices.contains(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_requested_once_until_the_device_disconnects() {
        let id = DeviceId::new("hid:receiver#1");
        let mut cache = NameCache::default();
        let mut query_count = 0;

        let first = cache.resolve(id.clone(), "Interface label", || {
            query_count += 1;
            Ok(Some("  Mouse Name  ".to_owned()))
        });
        let second = cache.resolve(id.clone(), "Changed interface label", || {
            query_count += 1;
            Ok(Some("Unexpected second name".to_owned()))
        });

        assert_eq!(first, ("Mouse Name".to_owned(), None));
        assert_eq!(second, ("Mouse Name".to_owned(), None));
        assert_eq!(query_count, 1);

        cache.forget(&id);
        let after_disconnect = cache.resolve(id, "New interface label", || {
            query_count += 1;
            Ok(Some("Replacement mouse".to_owned()))
        });

        assert_eq!(after_disconnect, ("Replacement mouse".to_owned(), None));
        assert_eq!(query_count, 2);
    }

    #[test]
    fn failed_device_name_lookup_is_cached_until_disconnect() {
        let id = DeviceId::new("hid:receiver#2");
        let mut cache = NameCache::default();
        let mut query_count = 0;

        let first = cache.resolve(id.clone(), "Interface label", || {
            query_count += 1;
            Err("name request timed out".to_owned())
        });
        let second = cache.resolve(id, "Changed interface label", || {
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
}
