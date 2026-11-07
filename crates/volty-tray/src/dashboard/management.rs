use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    config::AppConfig,
    history::{unix_now, HistoryDeletion, HistoryRangePreview, HistoryStore},
    state::{DeviceState, ForgottenDevicePreferences, TrayIconStyle, TrayIconTheme},
};

use super::model::{build_data, DashboardData, DashboardInventoryDevice, DashboardSettings};

const SECONDS_PER_DAY: i64 = 86_400;
const MAX_DASHBOARD_DAYS: u32 = u32::MAX;
const MAX_CLEANUP_DAYS: u32 = 30;

pub(super) struct DashboardBackend {
    history_path: PathBuf,
    mutations_allowed: bool,
}

impl DashboardBackend {
    pub(super) fn new(history_path: PathBuf, mutations_allowed: bool) -> Self {
        Self {
            history_path,
            mutations_allowed,
        }
    }

    pub(super) fn get_dashboard_data(
        &self,
        days: u32,
    ) -> Result<DashboardData, Box<dyn std::error::Error>> {
        validate_days(days, MAX_DASHBOARD_DAYS)?;
        let generated_at = unix_now()?;
        let window_start =
            generated_at.saturating_sub(i64::from(days).saturating_mul(SECONDS_PER_DAY));
        let history = if self.history_path.exists() {
            Some(HistoryStore::open(&self.history_path)?)
        } else {
            None
        };
        let samples = match history.as_ref() {
            Some(store) => store.samples_since(window_start)?,
            None => Vec::new(),
        };
        let state = DeviceState::open_local()?;
        let tray_display = state.tray_display()?;
        let inventory = build_inventory(
            history
                .as_ref()
                .map(HistoryStore::inventory)
                .transpose()?
                .unwrap_or_default(),
            state.monitored_devices()?,
            state.history_recording_preferences()?,
            tray_display.pinned_device.clone(),
        );
        let mut data =
            self.build_dashboard_data(samples, days, generated_at, window_start, inventory)?;
        if self.mutations_allowed {
            data.settings = Some(self.load_settings(tray_display)?);
        }
        Ok(data)
    }

    fn load_settings(
        &self,
        tray_display: crate::state::TrayDisplaySettings,
    ) -> Result<DashboardSettings, Box<dyn std::error::Error>> {
        let (config, config_error) = match AppConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (AppConfig::default(), Some(error.to_string())),
        };
        let (start_on_boot, startup_error) = match crate::tray::start_on_boot_enabled() {
            Ok(enabled) => (Some(enabled), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Ok(DashboardSettings {
            app_theme: config.app_theme.as_str().to_owned(),
            battery_color_red_max_percent: config.battery_color_map.red_max_percent,
            battery_color_yellow_max_percent: config.battery_color_map.yellow_max_percent,
            battery_color_red: format!(
                "#{:02X}{:02X}{:02X}",
                config.battery_color_map.red[0],
                config.battery_color_map.red[1],
                config.battery_color_map.red[2]
            ),
            battery_color_yellow: format!(
                "#{:02X}{:02X}{:02X}",
                config.battery_color_map.yellow[0],
                config.battery_color_map.yellow[1],
                config.battery_color_map.yellow[2]
            ),
            battery_color_green: format!(
                "#{:02X}{:02X}{:02X}",
                config.battery_color_map.green[0],
                config.battery_color_map.green[1],
                config.battery_color_map.green[2]
            ),
            low_battery_thresholds: config.low_battery_thresholds,
            low_battery_cooldown_minutes: config.low_battery_cooldown_minutes,
            notifications_enabled: config.notifications_enabled,
            tray_icon_style: config
                .tray_icon_style
                .unwrap_or(tray_display.icon_style)
                .as_str()
                .to_owned(),
            tray_icon_theme: config
                .tray_icon_theme
                .unwrap_or(tray_display.icon_theme)
                .as_str()
                .to_owned(),
            show_percentage_in_battery_icon: config.show_percentage_in_battery_icon,
            pinned_device: tray_display.pinned_device,
            start_on_boot,
            config_error,
            startup_error,
        })
    }

    fn build_dashboard_data(
        &self,
        samples: Vec<crate::history::HistorySample>,
        days: u32,
        generated_at: i64,
        window_start: i64,
        inventory: Vec<DashboardInventoryDevice>,
    ) -> Result<DashboardData, Box<dyn std::error::Error>> {
        Ok(build_data(
            samples,
            days,
            generated_at,
            window_start,
            self.mutations_allowed,
            inventory,
        )?)
    }

    pub(super) fn preview_cleanup(
        &self,
        kind: CleanupKind,
        days: Option<u32>,
        identity: Option<&str>,
    ) -> Result<CleanupResult, Box<dyn std::error::Error>> {
        validate_identity(identity)?;
        if kind == CleanupKind::DeviceHistory {
            let identity = identity.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "device_history preview requires an exact identity",
                )
            })?;
            let range = match open_history_for_read(&self.history_path)? {
                Some(store) => store.preview_identity_history(identity)?,
                None => HistoryRangePreview::default(),
            };
            return Ok(CleanupResult::for_identity_history(identity, range));
        }
        let days = days.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "cleanup preview requires a days value",
            )
        })?;
        validate_days(days, MAX_CLEANUP_DAYS)?;
        let cutoff = cutoff_for_days(days)?;
        let store = open_history_for_read(&self.history_path)?;
        let counts = match (kind, store) {
            (_, None) => HistoryDeletion::default(),
            (CleanupKind::OlderThan, Some(store)) => store.preview_older_than(cutoff, identity)?,
            (CleanupKind::InactiveDevices, Some(store)) => {
                store.preview_inactive(cutoff, identity)?
            }
            (CleanupKind::DeviceHistory, Some(_)) => {
                unreachable!("identity preview returned above")
            }
        };
        Ok(CleanupResult::new(kind, days, cutoff, counts, false))
    }

    pub(super) fn execute_cleanup(
        &self,
        kind: CleanupKind,
        days: Option<u32>,
        identity: Option<&str>,
    ) -> Result<CleanupResult, Box<dyn std::error::Error>> {
        self.require_mutations()?;
        let days = days.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "cleanup execution requires a days value",
            )
        })?;
        validate_days(days, MAX_CLEANUP_DAYS)?;
        validate_identity(identity)?;
        if kind == CleanupKind::DeviceHistory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "device_history is a preview-only cleanup kind; use forget_device to delete history",
            )
            .into());
        }
        let cutoff = cutoff_for_days(days)?;
        if !self.history_path.exists() {
            return Ok(CleanupResult::new(
                kind,
                days,
                cutoff,
                HistoryDeletion::default(),
                true,
            ));
        }
        let mut store = HistoryStore::open(&self.history_path)?;
        let counts = match kind {
            CleanupKind::OlderThan => store.delete_older_than(cutoff, identity)?,
            CleanupKind::InactiveDevices => store.delete_inactive(cutoff, identity)?,
            CleanupKind::DeviceHistory => unreachable!("preview-only kind rejected above"),
        };
        Ok(CleanupResult::new(kind, days, cutoff, counts, true))
    }

    pub(super) fn forget_device(
        &self,
        identity: &str,
        include_preferences: bool,
    ) -> Result<ForgetResult, Box<dyn std::error::Error>> {
        self.require_mutations()?;
        validate_identity(Some(identity))?;
        let deletion = if self.history_path.exists() {
            HistoryStore::open(&self.history_path)?.delete_identity(identity)?
        } else {
            HistoryDeletion::default()
        };
        if !include_preferences {
            return Ok(ForgetResult {
                identity: identity.to_owned(),
                deleted_samples: deletion.samples,
                deleted_devices: deletion.devices,
                preferences_cleared: None,
                history_recording_enabled: None,
                note: "Existing history was deleted only. If this device is still monitored and history recording remains enabled, new samples can appear. Disable history recording separately to stop future samples.".to_owned(),
            });
        }

        let preferences_cleared = match DeviceState::open_local()
            .and_then(|mut state| state.forget_device_preferences(identity))
        {
            Ok(cleared) => cleared,
            Err(error) => {
                return Err(io::Error::other(format!(
                    "Deleted {} history samples for this identity, but could not clear its app preferences: {error}",
                    deletion.samples
                ))
                .into());
            }
        };
        Ok(ForgetResult {
            identity: identity.to_owned(),
            deleted_samples: deletion.samples,
            deleted_devices: deletion.devices,
            preferences_cleared: Some(preferences_cleared),
            history_recording_enabled: Some(true),
            note: "Volty history and the requested per-device app preferences were cleared. Physical devices, OS entries, and pairing state were not changed. Future samples can be recorded again if the device remains monitored.".to_owned(),
        })
    }

    pub(super) fn set_history_recording(
        &self,
        identity: &str,
        enabled: bool,
    ) -> Result<HistoryRecordingResult, Box<dyn std::error::Error>> {
        self.require_mutations()?;
        validate_identity(Some(identity))?;
        DeviceState::open_local()?.set_history_recording_enabled(identity, enabled)?;
        Ok(HistoryRecordingResult {
            identity: identity.to_owned(),
            enabled,
            monitoring_unchanged: true,
        })
    }

    pub(super) fn update_settings(
        &self,
        update: &crate::config::DashboardSettingsUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.require_mutations()?;
        validate_identity(update.pinned_device.as_deref())?;
        crate::config::update_dashboard_settings(update)?;

        let mut state = DeviceState::open_local().map_err(|error| {
            io::Error::other(format!(
                "configuration was saved, but pinned-device state could not be opened: {error}"
            ))
        })?;
        match update.pinned_device.as_deref() {
            Some(identity) => {
                state.set_pinned_device(identity).map_err(|error| {
                    io::Error::other(format!(
                        "configuration was saved, but the pinned device could not be updated: {error}"
                    ))
                })?;
            }
            None => state.clear_pinned_device().map_err(|error| {
                io::Error::other(format!(
                    "configuration was saved, but the pinned device could not be cleared: {error}"
                ))
            })?,
        }
        let style = TrayIconStyle::parse(&update.tray_icon_style)
            .ok_or_else(|| io::Error::other("invalid tray icon style"))?;
        state.set_tray_icon_style(style).map_err(|error| {
            io::Error::other(format!(
                "configuration was saved, but tray icon style could not be stored: {error}"
            ))
        })?;
        let theme = TrayIconTheme::parse(&update.tray_icon_theme)
            .ok_or_else(|| io::Error::other("invalid tray icon theme"))?;
        state.set_tray_icon_theme(theme).map_err(|error| {
            io::Error::other(format!(
                "configuration was saved, but tray icon theme could not be stored: {error}"
            ))
        })?;

        if let Some(enabled) = update.start_on_boot {
            crate::tray::set_start_on_boot_enabled(enabled).map_err(|error| {
                io::Error::other(format!(
                    "configuration and pinned-device settings were saved, but Windows startup could not be updated: {error}"
                ))
            })?;
        }
        Ok(())
    }

    pub(super) fn test_notification(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.require_mutations()?;
        crate::notify::show_low_battery_notification("Volty test", 15)?;
        Ok(())
    }

    pub(super) fn open_settings(&self) -> Result<(), Box<dyn std::error::Error>> {
        let executable = env::current_exe()?;
        use std::os::windows::process::CommandExt;
        std::process::Command::new(executable)
            .args(["dashboard", "--settings"])
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()?;
        Ok(())
    }

    pub(super) fn list_artifacts(
        &self,
    ) -> Result<Vec<ManagedArtifact>, Box<dyn std::error::Error>> {
        Ok(artifact_inventory()?
            .into_iter()
            .map(|entry| entry.artifact)
            .collect())
    }

    pub(super) fn delete_artifact(
        &self,
        id: &str,
    ) -> Result<ArtifactDeletion, Box<dyn std::error::Error>> {
        self.require_mutations()?;
        if id.is_empty() || id.len() > 128 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid artifact id").into());
        }
        let matches = artifact_inventory()?
            .into_iter()
            .filter(|entry| entry.artifact.id == id)
            .collect::<Vec<_>>();
        let [entry] = matches.as_slice() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "artifact is no longer in Volty's managed artifact inventory",
            )
            .into());
        };
        fs::remove_file(&entry.path)?;
        Ok(ArtifactDeletion {
            id: entry.artifact.id.clone(),
            deleted: true,
        })
    }

    fn require_mutations(&self) -> Result<(), io::Error> {
        if self.mutations_allowed {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "data changes are disabled when the dashboard reads an explicitly selected database",
            ))
        }
    }
}

fn open_history_for_read(path: &Path) -> Result<Option<HistoryStore>, Box<dyn std::error::Error>> {
    if path.exists() {
        Ok(Some(HistoryStore::open(path)?))
    } else {
        Ok(None)
    }
}

fn build_inventory(
    history: Vec<crate::history::HistoryInventoryEntry>,
    monitored_devices: BTreeMap<String, bool>,
    history_preferences: BTreeMap<String, bool>,
    pinned_identity: Option<String>,
) -> Vec<DashboardInventoryDevice> {
    let mut inventory = BTreeMap::<String, DashboardInventoryDevice>::new();
    for device in history {
        inventory.insert(
            device.identity.clone(),
            DashboardInventoryDevice {
                identity: device.identity,
                name: if device.name.is_empty() {
                    "Unknown device".to_owned()
                } else {
                    device.name
                },
                samples: device.samples,
                last_sample_at: Some(device.last_at),
                monitored: None,
                history_recording_enabled: true,
                pinned: false,
            },
        );
    }
    for (identity, monitored) in monitored_devices {
        inventory
            .entry(identity.clone())
            .or_insert_with(|| inventory_entry(identity))
            .monitored = Some(monitored);
    }
    for (identity, enabled) in history_preferences {
        inventory
            .entry(identity.clone())
            .or_insert_with(|| inventory_entry(identity))
            .history_recording_enabled = enabled;
    }
    if let Some(identity) = pinned_identity {
        inventory
            .entry(identity.clone())
            .or_insert_with(|| inventory_entry(identity))
            .pinned = true;
    }
    inventory.into_values().collect()
}

fn inventory_entry(identity: String) -> DashboardInventoryDevice {
    DashboardInventoryDevice {
        name: identity.clone(),
        identity,
        samples: 0,
        last_sample_at: None,
        monitored: None,
        history_recording_enabled: true,
        pinned: false,
    }
}

fn validate_days(days: u32, max: u32) -> Result<(), io::Error> {
    if (1..=max).contains(&days) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("days must be between 1 and {max}"),
        ))
    }
}

fn validate_identity(identity: Option<&str>) -> Result<(), io::Error> {
    if identity.is_some_and(|identity| identity.is_empty() || identity.len() > 4096) {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "device identity must be non-empty and no longer than 4096 bytes",
        ))
    } else {
        Ok(())
    }
}

fn cutoff_for_days(days: u32) -> Result<i64, Box<dyn std::error::Error>> {
    let now = unix_now()?;
    Ok(now.saturating_sub(i64::from(days).saturating_mul(SECONDS_PER_DAY)))
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CleanupKind {
    OlderThan,
    InactiveDevices,
    DeviceHistory,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct CleanupResult {
    pub(super) kind: CleanupKind,
    pub(super) identity: Option<String>,
    pub(super) days: Option<u32>,
    pub(super) cutoff: Option<i64>,
    pub(super) samples: u64,
    pub(super) devices: u64,
    pub(super) first_at: Option<i64>,
    pub(super) last_at: Option<i64>,
    pub(super) date_range: Option<String>,
    pub(super) deleted: bool,
    pub(super) note: &'static str,
}

impl CleanupResult {
    fn new(
        kind: CleanupKind,
        days: u32,
        cutoff: i64,
        counts: HistoryDeletion,
        deleted: bool,
    ) -> Self {
        Self {
            kind,
            identity: None,
            days: Some(days),
            cutoff: Some(cutoff),
            samples: counts.samples,
            devices: counts.devices,
            first_at: None,
            last_at: None,
            date_range: None,
            deleted,
            note: "This operation changes existing history only; it does not disable history recording. An actively monitored device may produce new samples.",
        }
    }

    fn for_identity_history(identity: &str, preview: HistoryRangePreview) -> Self {
        Self {
            kind: CleanupKind::DeviceHistory,
            identity: Some(identity.to_owned()),
            days: None,
            cutoff: None,
            samples: preview.samples,
            devices: preview.devices,
            first_at: preview.first_at,
            last_at: preview.last_at,
            date_range: (preview.samples == 0)
                .then(|| format!("No samples recorded for {identity}")),
            deleted: false,
            note: "This exact-identity preview does not delete data. Forgetting history does not stop future recording while recording remains enabled.",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ForgetResult {
    pub(super) identity: String,
    pub(super) deleted_samples: u64,
    pub(super) deleted_devices: u64,
    pub(super) preferences_cleared: Option<ForgottenDevicePreferences>,
    pub(super) history_recording_enabled: Option<bool>,
    pub(super) note: String,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct HistoryRecordingResult {
    pub(super) identity: String,
    pub(super) enabled: bool,
    pub(super) monitoring_unchanged: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ManagedArtifact {
    pub(super) id: String,
    pub(super) kind: &'static str,
    pub(super) name: String,
    pub(super) bytes: u64,
    pub(super) created_at: Option<u64>,
    pub(super) device_identity: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ArtifactDeletion {
    pub(super) id: String,
    pub(super) deleted: bool,
}

struct ArtifactEntry {
    artifact: ManagedArtifact,
    path: PathBuf,
}

#[derive(Clone, Copy)]
struct ArtifactRoot {
    directory: &'static str,
    kind: &'static str,
}

const ARTIFACT_ROOTS: [ArtifactRoot; 2] = [
    ArtifactRoot {
        directory: "captures",
        kind: "capture",
    },
    ArtifactRoot {
        directory: "reports",
        kind: "report",
    },
];

fn artifact_inventory() -> Result<Vec<ArtifactEntry>, Box<dyn std::error::Error>> {
    let app_data = env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("could not determine the user application data folder"))?;
    let volty_directory = app_data.join("Volty");
    let mut artifacts = Vec::new();
    for root in ARTIFACT_ROOTS {
        let directory = volty_directory.join(root.directory);
        let root_metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Volty artifact root is not a managed directory",
            )
            .into());
        }
        let canonical_root = fs::canonicalize(&directory)?;
        let canonical_volty = fs::canonicalize(&volty_directory)?;
        if canonical_root.parent() != Some(canonical_volty.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Volty artifact root resolves outside the managed application directory",
            )
            .into());
        }
        for item in fs::read_dir(&directory)? {
            let item = item?;
            let metadata = item.file_type()?;
            if !metadata.is_file() {
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if !is_managed_artifact_name(root.kind, &name) {
                continue;
            }
            let path = item.path();
            let full_metadata = fs::symlink_metadata(&path)?;
            if full_metadata.file_type().is_symlink() || !full_metadata.is_file() {
                continue;
            }
            let canonical_path = fs::canonicalize(&path)?;
            if canonical_path.parent() != Some(canonical_root.as_path()) {
                continue;
            }
            let modified_at = full_metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            let id = artifact_id(root.kind, &canonical_root, &name, &full_metadata);
            artifacts.push(ArtifactEntry {
                artifact: ManagedArtifact {
                    id,
                    kind: root.kind,
                    name,
                    bytes: full_metadata.len(),
                    created_at: modified_at,
                    device_identity: None,
                },
                path,
            });
        }
    }
    artifacts.sort_by(|left, right| {
        left.artifact
            .kind
            .cmp(right.artifact.kind)
            .then_with(|| left.artifact.name.cmp(&right.artifact.name))
    });
    Ok(artifacts)
}

fn is_managed_artifact_name(kind: &str, name: &str) -> bool {
    match kind {
        "capture" => name.starts_with("volty-capture-") && name.ends_with(".volty.jsonl"),
        "report" => name.starts_with("volty-diagnose-") && name.ends_with(".txt"),
        _ => false,
    }
}

fn artifact_id(kind: &str, root: &Path, name: &str, metadata: &fs::Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_nanos());
    let input = format!(
        "{kind}\0{}\0{name}\0{}\0{modified}",
        root.to_string_lossy(),
        metadata.len()
    );
    let digest = Sha256::digest(input.as_bytes());
    let mut id = String::with_capacity(32);
    for byte in &digest[..16] {
        use std::fmt::Write as _;
        let _ = write!(id, "{byte:02x}");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_capability_follows_backend_mutation_permission() {
        let data = DashboardBackend::new(PathBuf::from("history.sqlite3"), false)
            .build_dashboard_data(Vec::new(), 7, 100, 0, Vec::new())
            .unwrap();
        assert!(!data.management_allowed);

        let data = DashboardBackend::new(PathBuf::from("history.sqlite3"), true)
            .build_dashboard_data(Vec::new(), 7, 100, 0, Vec::new())
            .unwrap();
        assert!(data.management_allowed);
    }

    #[test]
    fn inventory_merges_exact_persisted_identities_and_preferences() {
        let inventory = build_inventory(
            vec![crate::history::HistoryInventoryEntry {
                identity: "hid:keyboard#1".to_owned(),
                name: "Keyboard".to_owned(),
                samples: 4,
                last_at: 20,
            }],
            BTreeMap::from([("hid:keyboard#10".to_owned(), true)]),
            BTreeMap::from([("hid:keyboard#1".to_owned(), false)]),
            Some("bluetooth:headset".to_owned()),
        );

        assert_eq!(inventory.len(), 3);
        let first = inventory
            .iter()
            .find(|device| device.identity == "hid:keyboard#1")
            .unwrap();
        assert_eq!(first.samples, 4);
        assert!(!first.history_recording_enabled);
        let similarly_named = inventory
            .iter()
            .find(|device| device.identity == "hid:keyboard#10")
            .unwrap();
        assert_eq!(similarly_named.monitored, Some(true));
        assert_eq!(similarly_named.samples, 0);
    }

    #[test]
    fn artifact_catalog_accepts_only_volty_managed_names() {
        assert!(is_managed_artifact_name(
            "capture",
            "volty-capture-1-000000001.volty.jsonl"
        ));
        assert!(is_managed_artifact_name(
            "report",
            "volty-diagnose-1-000000001.txt"
        ));
        assert!(!is_managed_artifact_name(
            "capture",
            "user-hid-sniff.volty.jsonl"
        ));
        assert!(!is_managed_artifact_name(
            "report",
            "volty-diagnose-1.txt.exe"
        ));
    }

    #[test]
    fn cleanup_bounds_reject_zero_and_excessive_days() {
        assert!(validate_days(1, MAX_CLEANUP_DAYS).is_ok());
        assert!(validate_days(0, MAX_CLEANUP_DAYS).is_err());
        assert!(validate_days(MAX_CLEANUP_DAYS + 1, MAX_CLEANUP_DAYS).is_err());
        assert!(validate_identity(Some("")).is_err());
    }
}
