include!(concat!(env!("OUT_DIR"), "/builtin_profiles.rs"));

/// Return the embedded TOML for a built-in profile ID.
///
/// IDs are the path under `protocols/` without the `.toml` extension, using
/// forward slashes between directories.
pub fn builtin_profile_source(name: &str) -> Option<&'static str> {
    BUILTIN_PROFILES
        .iter()
        .find_map(|(profile_name, source)| (*profile_name == name).then_some(*source))
}

/// Iterate over the IDs of profiles embedded from `protocols/`.
pub fn builtin_profile_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_PROFILES.iter().map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CommandProfile;

    #[test]
    fn every_embedded_profile_is_valid() {
        assert!(BUILTIN_PROFILES
            .iter()
            .any(|(name, _)| *name == "logitech-hidpp/unified-battery"));

        for (name, source) in BUILTIN_PROFILES {
            CommandProfile::from_toml(source)
                .unwrap_or_else(|error| panic!("embedded profile {name} is invalid: {error}"));
        }
    }

    #[test]
    fn profile_lookup_and_name_listing_match() {
        let names: Vec<_> = builtin_profile_names().collect();
        assert!(builtin_profile_source("logitech-hidpp/unified-battery").is_some());
        assert!(names.contains(&"logitech-hidpp/unified-battery"));
        assert!(builtin_profile_source("missing/profile").is_none());
    }
}
