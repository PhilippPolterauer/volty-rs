//! System-tray UI and small platform integrations for Volty.
//!
//! Call [`TrayController::process_menu_events`] from the application's event
//! loop. Menu events are received through tray-icon's process-global receiver,
//! so an application should own one controller.

mod icon;
mod startup;

use icon::{
    battery_icon_at_size_with_color_map, unknown_battery_icon_at_size, validate_icon_size,
    ICON_SIZE,
};
#[cfg(windows)]
pub use startup::reset_startup_preferences;
use startup::start_on_boot_for_first_run;
pub use startup::{confirm_reset, set_start_on_boot_enabled, start_on_boot_enabled};
use std::{
    error::Error,
    fmt,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
};

use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    TrayIcon, TrayIconBuilder,
};

use crate::config::BatteryColorMap;

const MAX_STATUS_CHARS: usize = 80;
const MAX_TOOLTIP_UTF16_UNITS: usize = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuItemId {
    Status,
    Discover,
    Configuration,
    LogFile,
    Diagnose,
    StartOnBoot,
    ResetVolty,
    MonitoredDevices,
    MonitoredDevice(u64),
    About,
    Quit,
}

impl MenuItemId {
    fn as_id(self) -> String {
        match self {
            Self::Status => "volty.status".to_owned(),
            Self::Discover => "volty.discover".to_owned(),
            Self::Configuration => "volty.configuration".to_owned(),
            Self::LogFile => "volty.open-log-file".to_owned(),
            Self::Diagnose => "volty.diagnose".to_owned(),
            Self::StartOnBoot => "volty.start-on-boot".to_owned(),
            Self::ResetVolty => "volty.reset".to_owned(),
            Self::MonitoredDevices => "volty.monitored-devices".to_owned(),
            Self::MonitoredDevice(sequence) => {
                format!("volty.monitored-device.{sequence}")
            }
            Self::About => "volty.about".to_owned(),
            Self::Quit => "volty.quit".to_owned(),
        }
    }

    fn parse(id: &str) -> Option<Self> {
        match id {
            "volty.status" => Some(Self::Status),
            "volty.discover" => Some(Self::Discover),
            "volty.configuration" => Some(Self::Configuration),
            "volty.open-log-file" => Some(Self::LogFile),
            "volty.diagnose" => Some(Self::Diagnose),
            "volty.start-on-boot" => Some(Self::StartOnBoot),
            "volty.reset" => Some(Self::ResetVolty),
            "volty.monitored-devices" => Some(Self::MonitoredDevices),
            "volty.about" => Some(Self::About),
            "volty.quit" => Some(Self::Quit),
            _ => id
                .strip_prefix("volty.monitored-device.")
                .and_then(|sequence| sequence.parse().ok())
                .map(Self::MonitoredDevice),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceMonitorEntry {
    pub identity: String,
    pub name: String,
    pub monitored: bool,
}

struct MonitoredDeviceMenuItem {
    menu_id: MenuItemId,
    identity: String,
    item: CheckMenuItem,
}

/// Actions that the application should handle after polling the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    DiscoverDevicesNow,
    OpenConfiguration,
    OpenLogFile,
    Diagnose,
    ToggleStartOnBoot,
    ResetVolty,
    SetDeviceMonitored { identity: String, monitored: bool },
    About,
    Quit,
}

/// Errors returned while creating or updating the tray UI.
#[derive(Debug)]
pub struct TrayError(String);

impl TrayError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TrayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for TrayError {}

/// Owns the native tray icon, its menu and the receiver for application actions.
pub struct TrayController {
    icon: TrayIcon,
    menu: Menu,
    status_item: MenuItem,
    device_status_items: Vec<MenuItem>,
    monitored_devices_menu: Submenu,
    monitored_devices_empty_item: Option<MenuItem>,
    monitored_device_items: Vec<MonitoredDeviceMenuItem>,
    known_monitored_devices: Vec<DeviceMonitorEntry>,
    next_monitored_device_menu_id: u64,
    start_on_boot_item: CheckMenuItem,
    actions_tx: Sender<TrayAction>,
    actions_rx: Receiver<TrayAction>,
    percentage: Option<u8>,
    battery_color_map: BatteryColorMap,
}

struct TrayMenu {
    menu: Menu,
    status_item: MenuItem,
    monitored_devices_menu: Submenu,
    monitored_devices_empty_item: MenuItem,
    start_on_boot_item: CheckMenuItem,
}

struct TrayMenuItems {
    status_item: MenuItem,
    discover_item: MenuItem,
    configuration_item: MenuItem,
    open_log_item: MenuItem,
    diagnose_item: MenuItem,
    start_on_boot_item: CheckMenuItem,
    monitored_devices_menu: Submenu,
    monitored_devices_empty_item: MenuItem,
    about_item: MenuItem,
    reset_item: MenuItem,
    quit_item: MenuItem,
}

fn create_tray_menu(start_on_boot: bool) -> Result<TrayMenu, TrayError> {
    let status_item = MenuItem::with_id(
        MenuItemId::Status.as_id(),
        "Searching for supported devices",
        false,
        None,
    );
    let discover_item = MenuItem::with_id(
        MenuItemId::Discover.as_id(),
        "Discover Devices Now",
        true,
        None,
    );
    let configuration_item = MenuItem::with_id(
        MenuItemId::Configuration.as_id(),
        "Open Configuration",
        true,
        None,
    );
    let open_log_item = MenuItem::with_id(MenuItemId::LogFile.as_id(), "Open Log File", true, None);
    let diagnose_item = MenuItem::with_id(
        MenuItemId::Diagnose.as_id(),
        "Diagnose / Report an Issue",
        true,
        None,
    );
    let start_on_boot_item = CheckMenuItem::with_id(
        MenuItemId::StartOnBoot.as_id(),
        "Start on Boot",
        true,
        start_on_boot,
        None,
    );
    let monitored_devices_menu = Submenu::with_id(
        MenuItemId::MonitoredDevices.as_id(),
        "Monitored Devices",
        true,
    );
    let monitored_devices_empty_item =
        MenuItem::new("No supported devices discovered", false, None);
    monitored_devices_menu
        .append(&monitored_devices_empty_item)
        .map_err(|error| {
            TrayError::new(format!(
                "could not add empty monitored-devices status: {error}"
            ))
        })?;
    let items = TrayMenuItems {
        status_item,
        discover_item,
        configuration_item,
        open_log_item,
        diagnose_item,
        start_on_boot_item,
        monitored_devices_menu,
        monitored_devices_empty_item,
        about_item: MenuItem::with_id(MenuItemId::About.as_id(), "About Volty", true, None),
        reset_item: MenuItem::with_id(MenuItemId::ResetVolty.as_id(), "Reset Volty...", true, None),
        quit_item: MenuItem::with_id(MenuItemId::Quit.as_id(), "Quit", true, None),
    };
    let menu = Menu::new();
    append_tray_menu_items(&menu, &items)?;
    Ok(TrayMenu {
        menu,
        status_item: items.status_item,
        monitored_devices_menu: items.monitored_devices_menu,
        monitored_devices_empty_item: items.monitored_devices_empty_item,
        start_on_boot_item: items.start_on_boot_item,
    })
}

fn append_tray_menu_items(menu: &Menu, items: &TrayMenuItems) -> Result<(), TrayError> {
    menu.append(&items.status_item)
        .map_err(|error| TrayError::new(format!("could not add tray status: {error}")))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.monitored_devices_menu)
        .map_err(|error| {
            TrayError::new(format!("could not add monitored-devices menu: {error}"))
        })?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.discover_item).map_err(|error| {
        TrayError::new(format!("could not add device-discovery action: {error}"))
    })?;
    menu.append(&items.configuration_item)
        .map_err(|error| TrayError::new(format!("could not add configuration action: {error}")))?;
    menu.append(&items.open_log_item)
        .map_err(|error| TrayError::new(format!("could not add log-file action: {error}")))?;
    menu.append(&items.diagnose_item)
        .map_err(|error| TrayError::new(format!("could not add Diagnose action: {error}")))?;
    menu.append(&items.start_on_boot_item)
        .map_err(|error| TrayError::new(format!("could not add start-on-boot action: {error}")))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.about_item)
        .map_err(|error| TrayError::new(format!("could not add About action: {error}")))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.reset_item)
        .map_err(|error| TrayError::new(format!("could not add reset action: {error}")))?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|error| TrayError::new(format!("could not add tray separator: {error}")))?;
    menu.append(&items.quit_item)
        .map_err(|error| TrayError::new(format!("could not add quit action: {error}")))?;
    Ok(())
}

impl TrayController {
    /// Creates the tray icon and menu.
    ///
    /// The Start on Boot state is initialized from the Windows Run key. On
    /// other platforms the check item starts unchecked and the action reports
    /// that autostart is unsupported when selected.
    pub fn new() -> Result<Self, TrayError> {
        let tray_menu = create_tray_menu(start_on_boot_for_first_run()?)?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Volty — Searching for supported devices")
            .with_icon(unknown_battery_icon_at_size(ICON_SIZE)?)
            .with_menu(Box::new(tray_menu.menu.clone()))
            .build()
            .map_err(|error| TrayError::new(format!("could not create tray icon: {error}")))?;
        let (actions_tx, actions_rx) = mpsc::channel();

        Ok(Self {
            icon,
            menu: tray_menu.menu,
            status_item: tray_menu.status_item,
            device_status_items: Vec::new(),
            monitored_devices_menu: tray_menu.monitored_devices_menu,
            monitored_devices_empty_item: Some(tray_menu.monitored_devices_empty_item),
            monitored_device_items: Vec::new(),
            known_monitored_devices: Vec::new(),
            next_monitored_device_menu_id: 0,
            start_on_boot_item: tray_menu.start_on_boot_item,
            actions_tx,
            actions_rx,
            percentage: None,
            battery_color_map: BatteryColorMap::default(),
        })
    }

    /// Updates the tray icon, menu, and tooltip with the current device status.
    ///
    /// For example, `update_status("G502 X", "Discharging", Some(85))`
    /// displays `G502 X: 85% (Discharging)`. Pass `None` when the device or
    /// percentage is unavailable; the last battery level is retained if one
    /// has already been rendered.
    pub fn update_status(
        &mut self,
        device_name: &str,
        status: &str,
        percentage: Option<u8>,
    ) -> Result<(), TrayError> {
        self.update_status_with_icon_size(device_name, status, percentage, ICON_SIZE)
    }

    pub fn reset_status(&mut self, status: &str) -> Result<(), TrayError> {
        self.percentage = None;
        let status = shorten_status(status);
        self.update_display(&status, &status, None, ICON_SIZE, &[])
    }

    /// Updates the tray status while rendering its icon at 16×16 or 32×32.
    pub fn update_status_with_icon_size(
        &mut self,
        device_name: &str,
        status: &str,
        percentage: Option<u8>,
        icon_size: u32,
    ) -> Result<(), TrayError> {
        validate_icon_size(icon_size)?;
        let percentage = percentage.map(|value| value.min(100));
        let icon_percentage = self.update_percentage(percentage, false);

        let live_status = match percentage {
            Some(percentage) => format!("{device_name}: {percentage}% ({status})"),
            None if device_name.is_empty() => status.to_owned(),
            None => format!("{device_name}: {status}"),
        };
        let live_status = shorten_status(&live_status);
        self.update_display(&live_status, &live_status, icon_percentage, icon_size, &[])
    }

    /// Displays each readable device at the top of the tray menu. The tooltip
    /// contains the full list when it fits.
    pub fn update_devices(
        &mut self,
        summary: &str,
        device_statuses: &[String],
        percentage: Option<u8>,
    ) -> Result<(), TrayError> {
        let percentage = percentage.map(|value| value.min(100));
        let icon_percentage = self.update_percentage(percentage, device_statuses.is_empty());

        let tooltip = tooltip_for_devices(summary, device_statuses);
        let empty_status = "No device status available".to_owned();
        let menu_device_statuses = if device_statuses.is_empty() {
            std::slice::from_ref(&empty_status)
        } else {
            device_statuses
        };
        self.update_display(
            &shorten_status(summary),
            &tooltip,
            icon_percentage,
            ICON_SIZE,
            menu_device_statuses,
        )
    }

    pub fn update_monitored_devices(
        &mut self,
        devices: &[DeviceMonitorEntry],
    ) -> Result<(), TrayError> {
        let Some(devices) =
            sorted_monitored_devices_if_changed(&self.known_monitored_devices, devices)
        else {
            return Ok(());
        };

        self.clear_monitored_device_items()?;
        if devices.is_empty() {
            self.show_empty_monitored_devices()?;
        } else {
            self.remove_empty_monitored_devices()?;
            for device in &devices {
                self.append_monitored_device(device)?;
            }
        }
        self.known_monitored_devices = devices;
        Ok(())
    }

    fn clear_monitored_device_items(&mut self) -> Result<(), TrayError> {
        for entry in self.monitored_device_items.drain(..) {
            self.monitored_devices_menu
                .remove(&entry.item)
                .map_err(|error| {
                    TrayError::new(format!("could not clear monitored devices: {error}"))
                })?;
        }
        Ok(())
    }

    fn show_empty_monitored_devices(&mut self) -> Result<(), TrayError> {
        if self.monitored_devices_empty_item.is_none() {
            let item = MenuItem::new("No supported devices discovered", false, None);
            self.monitored_devices_menu.append(&item).map_err(|error| {
                TrayError::new(format!(
                    "could not update empty monitored-devices status: {error}"
                ))
            })?;
            self.monitored_devices_empty_item = Some(item);
        }
        Ok(())
    }

    fn remove_empty_monitored_devices(&mut self) -> Result<(), TrayError> {
        if let Some(item) = self.monitored_devices_empty_item.take() {
            self.monitored_devices_menu.remove(&item).map_err(|error| {
                TrayError::new(format!(
                    "could not clear empty monitored-devices status: {error}"
                ))
            })?;
        }
        Ok(())
    }

    fn append_monitored_device(&mut self, device: &DeviceMonitorEntry) -> Result<(), TrayError> {
        let sequence = self.next_monitored_device_menu_id;
        self.next_monitored_device_menu_id = sequence
            .checked_add(1)
            .ok_or_else(|| TrayError::new("monitored-device menu ID space exhausted"))?;
        let menu_id = MenuItemId::MonitoredDevice(sequence);
        let item =
            CheckMenuItem::with_id(menu_id.as_id(), &device.name, true, device.monitored, None);
        self.monitored_devices_menu
            .append(&item)
            .map_err(|error| TrayError::new(format!("could not add monitored device: {error}")))?;
        self.monitored_device_items.push(MonitoredDeviceMenuItem {
            menu_id,
            identity: device.identity.clone(),
            item,
        });
        Ok(())
    }

    fn update_display(
        &mut self,
        menu_status: &str,
        tooltip: &str,
        percentage: Option<u8>,
        icon_size: u32,
        device_statuses: &[String],
    ) -> Result<(), TrayError> {
        self.status_item.set_text(menu_status);
        self.replace_device_status_items(device_statuses)?;
        let icon = match percentage {
            Some(percentage) => {
                battery_icon_at_size_with_color_map(percentage, icon_size, self.battery_color_map)?
            }
            None => unknown_battery_icon_at_size(icon_size)?,
        };
        self.icon
            .set_icon(Some(icon))
            .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
        self.icon
            .set_tooltip(Some(tooltip))
            .map_err(|error| TrayError::new(format!("could not update tray tooltip: {error}")))?;
        Ok(())
    }

    fn replace_device_status_items(&mut self, device_statuses: &[String]) -> Result<(), TrayError> {
        for item in self.device_status_items.drain(..) {
            self.menu.remove(&item).map_err(|error| {
                TrayError::new(format!("could not clear tray device status: {error}"))
            })?;
        }

        for (index, status) in device_statuses.iter().enumerate() {
            let item = MenuItem::new(status, false, None);
            self.menu.insert(&item, index + 1).map_err(|error| {
                TrayError::new(format!("could not update tray device status: {error}"))
            })?;
            self.device_status_items.push(item);
        }
        Ok(())
    }

    /// Processes pending native menu events and returns the number handled.
    ///
    /// A Windows Run-key error is returned to the caller so it can be logged
    /// or displayed rather than silently leaving the check item out of sync.
    pub fn process_menu_events(&mut self) -> Result<usize, TrayError> {
        let mut handled = 0;
        #[allow(clippy::while_let_loop)]
        loop {
            let event = match MenuEvent::receiver().try_recv() {
                Ok(event) => event,
                Err(error) if error.is_empty() => break,
                Err(error) => {
                    return Err(TrayError::new(format!(
                        "tray menu event channel disconnected: {error}"
                    )))
                }
            };
            let Some(id) = MenuItemId::parse(event.id.0.as_str()) else {
                continue;
            };
            let Some(action) = self.action_for_menu_id(id) else {
                continue;
            };
            self.actions_tx
                .send(action)
                .map_err(|_| TrayError::new("tray action receiver has been dropped"))?;
            handled += 1;
        }
        Ok(handled)
    }

    fn action_for_menu_id(&self, id: MenuItemId) -> Option<TrayAction> {
        match id {
            MenuItemId::Discover => Some(TrayAction::DiscoverDevicesNow),
            MenuItemId::Configuration => Some(TrayAction::OpenConfiguration),
            MenuItemId::LogFile => Some(TrayAction::OpenLogFile),
            MenuItemId::Diagnose => Some(TrayAction::Diagnose),
            MenuItemId::StartOnBoot => Some(TrayAction::ToggleStartOnBoot),
            MenuItemId::MonitoredDevice(menu_id) => self
                .monitored_device_items
                .iter()
                .find(|entry| entry.menu_id == MenuItemId::MonitoredDevice(menu_id))
                .map(|entry| TrayAction::SetDeviceMonitored {
                    identity: entry.identity.clone(),
                    monitored: entry.item.is_checked(),
                }),
            MenuItemId::About => Some(TrayAction::About),
            MenuItemId::ResetVolty => Some(TrayAction::ResetVolty),
            MenuItemId::Quit => Some(TrayAction::Quit),
            MenuItemId::Status | MenuItemId::MonitoredDevices => None,
        }
    }
    /// Gets the next app action, if one has been queued.
    pub fn try_recv_action(&self) -> Result<TrayAction, TryRecvError> {
        self.actions_rx.try_recv()
    }

    pub fn set_start_on_boot_checked(&self, checked: bool) {
        self.start_on_boot_item.set_checked(checked);
    }

    pub fn set_device_monitored_checked(&self, identity: &str, checked: bool) {
        if let Some(entry) = self
            .monitored_device_items
            .iter()
            .find(|entry| entry.identity == identity)
        {
            entry.item.set_checked(checked);
        }
    }

    pub fn set_battery_color_map(
        &mut self,
        battery_color_map: BatteryColorMap,
    ) -> Result<(), TrayError> {
        if self.battery_color_map == battery_color_map {
            return Ok(());
        }
        if let Some(percentage) = self.percentage {
            let icon =
                battery_icon_at_size_with_color_map(percentage, ICON_SIZE, battery_color_map)?;
            self.icon
                .set_icon(Some(icon))
                .map_err(|error| TrayError::new(format!("could not update tray icon: {error}")))?;
        }
        self.battery_color_map = battery_color_map;
        Ok(())
    }
    fn update_percentage(
        &mut self,
        percentage: Option<u8>,
        clear_when_missing: bool,
    ) -> Option<u8> {
        match percentage {
            Some(percentage) => self.percentage = Some(percentage),
            None if clear_when_missing => self.percentage = None,
            None => {}
        }
        percentage.or(self.percentage)
    }
}

fn sorted_monitored_devices_if_changed(
    known_devices: &[DeviceMonitorEntry],
    devices: &[DeviceMonitorEntry],
) -> Option<Vec<DeviceMonitorEntry>> {
    let mut devices = devices.to_vec();
    devices.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.identity.cmp(&right.identity))
    });
    (devices != known_devices).then_some(devices)
}

fn tooltip_for_devices(summary: &str, device_statuses: &[String]) -> String {
    let details = device_statuses.join("; ");
    if details.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS {
        return details;
    }
    shorten_tooltip(&format!("{summary}; right-click for all device stats"))
}

fn shorten_tooltip(status: &str) -> String {
    let mut tooltip = String::new();
    let mut utf16_units = 0;
    for character in status.chars() {
        let character_units = character.len_utf16();
        if utf16_units + character_units > MAX_TOOLTIP_UTF16_UNITS - 1 {
            tooltip.push('…');
            return tooltip;
        }
        tooltip.push(character);
        utf16_units += character_units;
    }
    tooltip
}

fn shorten_status(status: &str) -> String {
    let mut characters = status.chars();
    let mut shortened: String = characters.by_ref().take(MAX_STATUS_CHARS).collect();
    if characters.next().is_some() {
        shortened.pop();
        shortened.push('…');
    }
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_shortened_only_when_over_limit() {
        let short = "Battery status";
        assert_eq!(shorten_status(short), short);

        let exact_limit = "x".repeat(MAX_STATUS_CHARS);
        assert_eq!(shorten_status(&exact_limit), exact_limit);

        let long = "x".repeat(MAX_STATUS_CHARS + 10);
        let shortened = shorten_status(&long);
        assert_eq!(shortened.chars().count(), MAX_STATUS_CHARS);
        assert!(shortened.ends_with('…'));
    }

    #[test]
    fn menu_item_ids_round_trip() {
        for id in [
            MenuItemId::Status,
            MenuItemId::Discover,
            MenuItemId::Configuration,
            MenuItemId::LogFile,
            MenuItemId::Diagnose,
            MenuItemId::StartOnBoot,
            MenuItemId::ResetVolty,
            MenuItemId::MonitoredDevices,
            MenuItemId::MonitoredDevice(42),
            MenuItemId::About,
            MenuItemId::Quit,
        ] {
            assert_eq!(MenuItemId::parse(&id.as_id()), Some(id));
        }
        assert_eq!(
            MenuItemId::parse("volty.monitored-device.not-a-number"),
            None
        );
        assert_eq!(MenuItemId::parse("volty.monitored-device."), None);
        assert_eq!(
            MenuItemId::parse(&format!("volty.monitored-device.{}", u64::MAX)),
            Some(MenuItemId::MonitoredDevice(u64::MAX))
        );
        assert_eq!(
            MenuItemId::parse("volty.monitored-device.18446744073709551616"),
            None
        );
    }

    fn monitor_entry(identity: &str, name: &str, monitored: bool) -> DeviceMonitorEntry {
        DeviceMonitorEntry {
            identity: identity.to_owned(),
            name: name.to_owned(),
            monitored,
        }
    }

    #[test]
    fn monitored_device_refresh_sorts_entries_and_detects_only_real_changes() {
        let sorted = vec![
            monitor_entry("a", "Mouse A", true),
            monitor_entry("z", "Mouse A", false),
            monitor_entry("b", "Mouse B", true),
        ];
        let incoming = vec![sorted[2].clone(), sorted[1].clone(), sorted[0].clone()];

        assert_eq!(
            sorted_monitored_devices_if_changed(&sorted, &incoming),
            None
        );

        let mut changed = incoming;
        changed[1].monitored = true;
        let mut expected = sorted.clone();
        expected[1].monitored = true;
        assert_eq!(
            sorted_monitored_devices_if_changed(&sorted, &changed),
            Some(expected)
        );
    }

    #[test]
    fn tooltip_includes_all_device_statuses_when_they_fit() {
        let statuses = vec![
            "Mouse A (slot 1): 85% (Discharging)".to_owned(),
            "Mouse B (slot 2): 42% (Charging)".to_owned(),
        ];

        assert_eq!(
            tooltip_for_devices("2 devices monitored", &statuses),
            statuses.join("; ")
        );
    }

    #[test]
    fn tooltip_respects_utf16_length_boundaries() {
        let exact_ascii = vec!["x".repeat(MAX_TOOLTIP_UTF16_UNITS)];
        assert_eq!(tooltip_for_devices("summary", &exact_ascii), exact_ascii[0]);

        let exact_unicode = vec![format!("{}{}", "🙂".repeat(32), "x".repeat(63))];
        assert_eq!(
            exact_unicode[0].encode_utf16().count(),
            MAX_TOOLTIP_UTF16_UNITS
        );
        assert_eq!(
            tooltip_for_devices("summary", &exact_unicode),
            exact_unicode[0]
        );

        let overflow = vec![format!("{}{}", "🙂".repeat(32), "x".repeat(64))];
        let tooltip = tooltip_for_devices("summary", &overflow);
        assert!(tooltip.contains("right-click for all device stats"));
        assert!(tooltip.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS);
    }

    #[test]
    fn long_tooltip_points_to_full_device_status_menu() {
        let statuses = vec!["x".repeat(MAX_TOOLTIP_UTF16_UNITS); 2];
        let tooltip = tooltip_for_devices("2 devices monitored", &statuses);

        assert!(tooltip.encode_utf16().count() <= MAX_TOOLTIP_UTF16_UNITS);
        assert!(tooltip.contains("right-click for all device stats"));
    }
}
